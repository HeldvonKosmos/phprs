//! Worker threads, one `Vm` each (RUNTIME_ARCHITECTURE §9). Requests travel
//! over a channel; a request stays on its worker, so the VM needs no `Send`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use std::collections::HashMap;

use grd_vm::value::{ClassId, ObjHandle, Val};
use grd_vm::vm::Vm;

use crate::native::database::model::ModelMeta;
use crate::native::database::surreal::{Db, DbConfig};

use crate::http::HttpRequest;
use crate::routes::{Handler, RouteTable};

/// Host context of a worker VM: lives as long as the VM; `request` is
/// swapped per request.
pub struct WorkerCtx {
    pub routes: RouteTable,
    pub settings: serde_json::Value,
    pub request: Option<RequestState>,
    /// The last log lines (newest last), for tests and the error page.
    pub logs: Vec<(String, String)>,
    /// Database connection shared by all workers (`None`: not configured).
    pub db: Option<Arc<Db>>,
    /// Open client transaction of `DB::transaction(...)` on this worker.
    pub txn: Option<surrealdb::method::Transaction<surrealdb::engine::any::Any>>,
    /// Model metadata read once per class from the dialect declarations.
    pub model_meta: HashMap<ClassId, ModelMeta>,
}

pub struct RequestState {
    pub req: HttpRequest,
    pub handler: Handler,
    pub params: Vec<(String, String)>,
    pub object: Option<ObjHandle>,
    pub input: Option<Val>,
}

#[derive(Clone, Debug)]
pub struct PoolOptions {
    pub root: PathBuf,
    pub workers: usize,
    pub settings: serde_json::Value,
    pub database: Option<DbConfig>,
}

impl PoolOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        PoolOptions { root: root.into(), workers: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2), settings: serde_json::Value::Null, database: None }
    }
    pub fn workers(mut self, n: usize) -> Self {
        self.workers = n.max(1);
        self
    }
    pub fn settings(mut self, s: serde_json::Value) -> Self {
        self.settings = s;
        self
    }
    pub fn database(mut self, cfg: DbConfig) -> Self {
        self.database = Some(cfg);
        self
    }
}

/// Dialect part of the framework: Middleware, Pipeline, Controller, HttpException, ValidationException.
pub const FRAMEWORK_PRELUDE_HTTP: &str = include_str!("prelude_http.grd");
pub const FRAMEWORK_PRELUDE_VALIDATION: &str = include_str!("prelude_validation.grd");

type Job = Box<dyn FnOnce(&mut Vm) + Send>;

pub struct Pool {
    tx: mpsc::Sender<Job>,
    workers: usize,
    options: PoolOptions,
}

#[derive(Debug)]
pub enum PoolError {
    /// The worker panicked or exited before answering.
    WorkerFailed,
}

/// Build a worker VM: prelude, native classes, host context, project.
pub fn build_vm(options: &PoolOptions, db: Option<Arc<Db>>) -> Result<(Vm, grd_vm::vm::project::ProjectSummary), String> {
    let mut vm = Vm::with_prelude().map_err(|t| format!("prelude: {}", t.message))?;
    crate::native::register(&mut vm).map_err(|t| format!("native classes: {}", t.message))?;
    for (name, src) in [("{framework/http}", FRAMEWORK_PRELUDE_HTTP), ("{framework/validation}", FRAMEWORK_PRELUDE_VALIDATION)] {
        vm.run_file(name, src.as_bytes()).map_err(|e| format!("framework prelude {name}: {e}"))?;
    }
    vm.set_host(Box::new(WorkerCtx { routes: RouteTable::default(), settings: options.settings.clone(), request: None, logs: Vec::new(), db, txn: None, model_meta: HashMap::new() }));
    let summary = vm.load_project(&options.root).map_err(|e| e.to_string())?;
    vm.reset_request();
    Ok((vm, summary))
}

impl Pool {
    /// Start the workers; every worker loads the project itself. Fails when a
    /// worker cannot load it (compile or link error, with GRD code).
    pub fn start(options: PoolOptions) -> Result<Pool, String> {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<usize, String>>();
        // One connection for all workers: the SDK client multiplexes, and an
        // in-memory database must be the same for every worker.
        let db = match &options.database {
            Some(cfg) => Some(Arc::new(Db::connect(cfg.clone())?)),
            None => None,
        };
        for id in 0..options.workers {
            let rx = Arc::clone(&rx);
            let ready = ready_tx.clone();
            let opts = options.clone();
            let db = db.clone();
            std::thread::Builder::new()
                .name(format!("grd-worker-{id}"))
                .stack_size(64 * 1024 * 1024)
                .spawn(move || worker_main(id, opts, db, rx, ready))
                .map_err(|e| format!("spawn worker: {e}"))?;
        }
        drop(ready_tx);
        let mut ok = 0;
        while ok < options.workers {
            match ready_rx.recv() {
                Ok(Ok(_)) => ok += 1,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err("worker exited during startup".into()),
            }
        }
        Ok(Pool { tx, workers: options.workers, options })
    }

    pub fn workers(&self) -> usize {
        self.workers
    }

    pub fn options(&self) -> &PoolOptions {
        &self.options
    }

    fn submit(&self, job: Job) {
        // The receiver lives as long as any worker thread does.
        let _ = self.tx.send(job);
    }

    /// Run `f` on some worker and wait for the result (blocking; tests, CLI).
    pub fn run_sync<R: Send + 'static>(&self, f: impl FnOnce(&mut Vm) -> R + Send + 'static) -> Result<R, PoolError> {
        let (tx, rx) = mpsc::channel();
        self.submit(Box::new(move |vm| {
            let _ = tx.send(f(vm));
        }));
        rx.recv().map_err(|_| PoolError::WorkerFailed)
    }

    /// Run `f` on some worker and await the result (Tokio side).
    pub async fn run<R: Send + 'static>(&self, f: impl FnOnce(&mut Vm) -> R + Send + 'static) -> Result<R, PoolError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.submit(Box::new(move |vm| {
            let _ = tx.send(f(vm));
        }));
        rx.await.map_err(|_| PoolError::WorkerFailed)
    }
}

fn worker_main(id: usize, options: PoolOptions, db: Option<Arc<Db>>, rx: Arc<Mutex<mpsc::Receiver<Job>>>, ready: mpsc::Sender<Result<usize, String>>) {
    let mut vm = match build_vm(&options, db.clone()) {
        Ok((vm, summary)) => {
            tracing::info!(target: "grd", worker = id, modules = summary.modules.len(), classes = summary.class_files, routes = summary.route_files, "worker ready");
            let _ = ready.send(Ok(id));
            vm
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    loop {
        let job = {
            let guard = match rx.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            guard.recv()
        };
        let Ok(job) = job else { return };
        if catch_unwind(AssertUnwindSafe(|| job(&mut vm))).is_err() {
            // RUNTIME_ARCHITECTURE §11: the worker catches the panic; the
            // VM state is suspect afterwards, so rebuild it.
            tracing::error!(target: "grd", worker = id, "panic in worker; rebuilding VM");
            match build_vm(&options, db.clone()) {
                Ok((fresh, _)) => vm = fresh,
                Err(e) => {
                    tracing::error!(target: "grd", worker = id, error = %e, "cannot rebuild VM; worker exits");
                    return;
                }
            }
        }
    }
}
