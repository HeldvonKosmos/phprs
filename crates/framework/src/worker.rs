//! Worker threads, one `Vm` each (RUNTIME_ARCHITECTURE §9). Requests travel
//! over a channel; a request stays on its worker, so the VM needs no `Send`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::Watcher;

use std::collections::HashMap;

use grd_vm::value::{ClassId, ObjHandle, Val};
use grd_vm::vm::Vm;

use crate::native::database::model::ModelMeta;
use crate::native::database::surreal::{Db, DbConfig};

use crate::http::HttpRequest;
use crate::routes::{Handler, RouteTable};
use crate::native::rate_limiter::RateLimits;
use crate::services::Services;
use crate::session::{SessionConfig, SessionState, SessionStore};

/// Host context of a worker VM: lives as long as the VM; `request` is
/// swapped per request.
pub struct WorkerCtx {
    /// Project root (modules/, routes/, database/).
    pub root: PathBuf,
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
    /// Field types per table from `INFO FOR TABLE`, for typed filters.
    pub schema_cache: HashMap<String, HashMap<String, String>>,
    /// Dev mode: error pages with file, line, trace and source.
    pub debug: bool,
    /// Session store shared by all workers of the pool.
    pub sessions: Arc<SessionStore>,
    /// Rate-limit counters shared by all workers of the pool (AU2).
    pub limits: Arc<RateLimits>,
    /// Mail and storage (MS1), shared by all workers of the pool.
    pub services: Arc<Services>,
    /// J3: scheduled tasks and jobs declared by the routes files (each worker
    /// loads them; the scheduler reads one worker's list).
    pub schedule: Vec<crate::scheduler::Entry>,
    /// Set when a hot reload failed: the worker keeps the old code and
    /// answers every request with this error until the next good reload.
    pub stale_error: Option<String>,
}

pub struct RequestState {
    pub req: HttpRequest,
    pub handler: Handler,
    pub params: Vec<(String, String)>,
    pub object: Option<ObjHandle>,
    pub input: Option<Val>,
    pub session: Option<SessionState>,
    /// The authenticated user of this request (AU1, set by `auth`).
    pub user: Option<Val>,
}

#[derive(Clone, Debug)]
pub struct PoolOptions {
    pub root: PathBuf,
    pub workers: usize,
    pub settings: serde_json::Value,
    pub database: Option<DbConfig>,
    pub debug: bool,
    /// Watch `modules/` and `routes/` for `.grd` changes and reload.
    pub watch: bool,
    pub session: SessionConfig,
}

impl PoolOptions {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        PoolOptions { root: root.into(), workers: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2), settings: serde_json::Value::Null, database: None, debug: true, watch: false, session: SessionConfig::default() }
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
    pub fn debug(mut self, on: bool) -> Self {
        self.debug = on;
        self
    }
    pub fn watch(mut self, on: bool) -> Self {
        self.watch = on;
        self
    }
    pub fn session(mut self, cfg: SessionConfig) -> Self {
        self.session = cfg;
        self
    }
}

/// Dialect part of the framework: Middleware, Pipeline, Controller, HttpException, ValidationException.
pub const FRAMEWORK_PRELUDE_HTTP: &str = include_str!("prelude_http.grd");
pub const FRAMEWORK_PRELUDE_VALIDATION: &str = include_str!("prelude_validation.grd");
pub const FRAMEWORK_PRELUDE_DATABASE: &str = include_str!("prelude_database.grd");
pub const FRAMEWORK_PRELUDE_AUTH: &str = include_str!("prelude_auth.grd");
pub const FRAMEWORK_PRELUDE_MAIL: &str = include_str!("prelude_mail.grd");
pub const FRAMEWORK_PRELUDE_CONSOLE: &str = include_str!("prelude_console.grd");
pub const FRAMEWORK_PRELUDE_QUEUE: &str = include_str!("prelude_queue.grd");
pub const FRAMEWORK_PRELUDE_SCHEDULE: &str = include_str!("prelude_schedule.grd");

pub type Job = Box<dyn FnOnce(&mut Vm) + Send>;

pub struct Pool {
    tx: mpsc::Sender<Job>,
    workers: usize,
    options: PoolOptions,
    /// Bumped on reload; a worker whose VM is older rebuilds it before its next job.
    generation: Arc<AtomicU64>,
    sessions: Arc<SessionStore>,
    services: Arc<Services>,
    /// The pool's one database connection (shared by all workers).
    db: Option<Arc<Db>>,
}

#[derive(Debug)]
pub enum PoolError {
    /// The worker panicked or exited before answering.
    WorkerFailed,
}

/// Build a worker VM: prelude, native classes, host context, project.
/// JIT in the workers (DECISIONS D9): off by default; `OPENGRID_JIT=on`
/// turns it on, `GRD_JIT=0` is the emergency switch that wins over
/// everything. Off means: interpreter only, no machine code. The advanced
/// JIT (tiers, background compiler) additionally needs `GRD_ADVANCED_JIT=on`.
pub fn jit_enabled() -> bool {
    matches!(std::env::var("OPENGRID_JIT").as_deref(), Ok("on" | "1")) && !grd_jit::disabled_by_env()
}

pub fn build_vm(options: &PoolOptions, db: Option<Arc<Db>>, sessions: Arc<SessionStore>, limits: Arc<RateLimits>, services: Arc<Services>) -> Result<(Vm, grd_vm::vm::project::ProjectSummary), String> {
    let mut vm = Vm::with_prelude().map_err(|t| format!("prelude: {}", t.message))?;
    if jit_enabled() {
        vm.set_jit(Box::new(grd_jit::CraneliftJit::new()));
    }
    crate::native::register(&mut vm).map_err(|t| format!("native classes: {}", t.message))?;
    for (name, src) in [("{framework/http}", FRAMEWORK_PRELUDE_HTTP), ("{framework/validation}", FRAMEWORK_PRELUDE_VALIDATION), ("{framework/database}", FRAMEWORK_PRELUDE_DATABASE), ("{framework/auth}", FRAMEWORK_PRELUDE_AUTH), ("{framework/mail}", FRAMEWORK_PRELUDE_MAIL), ("{framework/console}", FRAMEWORK_PRELUDE_CONSOLE), ("{framework/queue}", FRAMEWORK_PRELUDE_QUEUE), ("{framework/schedule}", FRAMEWORK_PRELUDE_SCHEDULE)] {
        vm.run_file(name, src.as_bytes()).map_err(|e| format!("framework prelude {name}: {e}"))?;
    }
    vm.set_host(Box::new(WorkerCtx { root: options.root.clone(), routes: RouteTable::default(), settings: options.settings.clone(), request: None, logs: Vec::new(), db, txn: None, model_meta: HashMap::new(), schema_cache: HashMap::new(), sessions, limits, services, schedule: Vec::new(), debug: options.debug, stale_error: None }));
    // default middleware aliases
    if let Some(c) = vm.host::<WorkerCtx>() {
        c.routes.aliases.insert("csrf".into(), "OpenGrid\\Http\\Middleware\\VerifyCsrfToken".into());
        c.routes.aliases.insert("auth".into(), "OpenGrid\\Auth\\Authenticate".into());
        c.routes.aliases.insert("throttle".into(), "OpenGrid\\Http\\Middleware\\ThrottleRequests".into());
    }
    // MEM1b: the request state holds values the collector must see
    vm.host_tracer = Some(trace_host);
    // MEM1a: live objects per request (settings.opengrid.limits.objects, 0 = none)
    if let Some(n) = options.settings.pointer("/opengrid/limits/objects").and_then(|v| v.as_u64()) {
        vm.object_limit = n as usize;
    }
    let summary = vm.load_project(&options.root).map_err(|e| e.to_string())?;
    vm.reset_request();
    Ok((vm, summary))
}

/// MEM1b roots of the host: request object, parsed input, user, session data.
fn trace_host(host: &dyn std::any::Any, out: &mut Vec<Val>) {
    let Some(ctx) = host.downcast_ref::<WorkerCtx>() else { return };
    if let Some(r) = &ctx.request {
        out.extend(r.object.map(Val::Object));
        out.extend(r.input.iter().cloned());
        out.extend(r.user.iter().cloned());
        if let Some(s) = &r.session {
            out.extend(s.data.values().cloned());
            out.extend(s.remembered.iter().map(|(_, h)| Val::Object(*h)));
        }
    }
}

/// PKG1: compile and link the whole project as a worker does (framework
/// preludes and natives), without opening the database.
pub fn check_project(options: &PoolOptions) -> Result<grd_vm::vm::project::ProjectSummary, String> {
    let mut opts = options.clone();
    opts.database = None;
    let services = Arc::new(Services::new(&opts.root, &opts.settings)?);
    build_vm(&opts, None, Arc::new(SessionStore::new(opts.session.clone())), Arc::new(RateLimits::new()), services).map(|(_, summary)| summary)
}

impl Pool {
    /// Start the workers; every worker loads the project itself. Fails when a
    /// worker cannot load it (compile or link error, with GRD code).
    pub fn start(options: PoolOptions) -> Result<Pool, String> {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<usize, String>>();
        let generation = Arc::new(AtomicU64::new(0));
        let sessions = Arc::new(SessionStore::new(options.session.clone()));
        let limits = Arc::new(RateLimits::new());
        let services = Arc::new(Services::new(&options.root, &options.settings)?);
        // One connection for all workers: the SDK client multiplexes, and an
        // in-memory database must be the same for every worker.
        let db = match &options.database {
            Some(cfg) => Some(Arc::new(Db::connect(cfg.clone())?)),
            None => None,
        };
        tracing::info!(target: "opengrid", "JIT: {}", grd_jit::describe(jit_enabled()));
        for id in 0..options.workers {
            let rx = Arc::clone(&rx);
            let ready = ready_tx.clone();
            let opts = options.clone();
            let db = db.clone();
            let generation = Arc::clone(&generation);
            let sessions = Arc::clone(&sessions);
            let limits = Arc::clone(&limits);
            let services = Arc::clone(&services);
            std::thread::Builder::new()
                .name(format!("grd-worker-{id}"))
                .stack_size(64 * 1024 * 1024)
                .spawn(move || worker_main(id, opts, db, sessions, limits, services, rx, ready, generation))
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
        if options.watch {
            start_watcher(options.root.clone(), Arc::clone(&generation));
        }
        services.open_jobs(tx.clone());
        Ok(Pool { tx, workers: options.workers, options, generation, sessions, services, db })
    }

    /// Ask every worker to rebuild its VM before its next job (hot reload).
    pub fn reload(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }

    pub fn workers(&self) -> usize {
        self.workers
    }

    pub fn options(&self) -> &PoolOptions {
        &self.options
    }

    /// The database connection, for schema operations outside a worker.
    pub fn db(&self) -> Option<Arc<Db>> {
        self.db.clone()
    }

    /// Mail, storage and the job queue of this pool.
    pub fn services(&self) -> &Services {
        &self.services
    }

    pub fn services_arc(&self) -> Arc<Services> {
        Arc::clone(&self.services)
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

impl Drop for Pool {
    fn drop(&mut self) {
        // the job queue holds a sender of the workers' channel: drop it, so
        // the workers end once the pool's own sender is gone
        self.services.close_jobs();
    }
}

fn worker_main(id: usize, options: PoolOptions, db: Option<Arc<Db>>, sessions: Arc<SessionStore>, limits: Arc<RateLimits>, services: Arc<Services>, rx: Arc<Mutex<mpsc::Receiver<Job>>>, ready: mpsc::Sender<Result<usize, String>>, generation: Arc<AtomicU64>) {
    let mut my_generation = generation.load(Ordering::SeqCst);
    let mut vm = match build_vm(&options, db.clone(), sessions.clone(), limits.clone(), services.clone()) {
        Ok((vm, summary)) => {
            tracing::info!(target: "opengrid", worker = id, modules = summary.modules.len(), classes = summary.class_files, routes = summary.route_files, "worker ready");
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
        let current = generation.load(Ordering::SeqCst);
        if current != my_generation {
            my_generation = current;
            match build_vm(&options, db.clone(), sessions.clone(), limits.clone(), services.clone()) {
                Ok((fresh, summary)) => {
                    tracing::info!(target: "opengrid", worker = id, classes = summary.class_files, routes = summary.route_files, "reloaded");
                    vm = fresh;
                }
                Err(e) => {
                    tracing::error!(target: "opengrid", worker = id, "reload failed: {e}");
                    if let Some(c) = vm.host::<WorkerCtx>() {
                        c.stale_error = Some(e);
                    }
                }
            }
        }
        if catch_unwind(AssertUnwindSafe(|| job(&mut vm))).is_err() {
            // RUNTIME_ARCHITECTURE §11: the worker catches the panic; the
            // VM state is suspect afterwards, so rebuild it.
            tracing::error!(target: "opengrid", worker = id, "panic in worker; rebuilding VM");
            match build_vm(&options, db.clone(), sessions.clone(), limits.clone(), services.clone()) {
                Ok((fresh, _)) => vm = fresh,
                Err(e) => {
                    tracing::error!(target: "opengrid", worker = id, error = %e, "cannot rebuild VM; worker exits");
                    return;
                }
            }
        }
    }
}

fn is_grd_event(ev: &notify::Result<notify::Event>) -> bool {
    ev.as_ref().map(|e| e.paths.iter().any(|p| p.extension().is_some_and(|x| x == "grd"))).unwrap_or(false)
}

/// Watch `root/modules` and `root/routes`; a burst of `.grd` changes bumps
/// the generation once (150 ms debounce).
fn start_watcher(root: PathBuf, generation: Arc<AtomicU64>) {
    let spawned = std::thread::Builder::new().name("grd-watch".into()).spawn(move || {
        let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!(target: "opengrid", "file watcher unavailable: {e}");
                return;
            }
        };
        for dir in ["modules", "routes"] {
            let p = root.join(dir);
            if p.is_dir()
                && let Err(e) = watcher.watch(&p, notify::RecursiveMode::Recursive)
            {
                tracing::warn!(target: "opengrid", path = %p.display(), "cannot watch: {e}");
            }
        }
        tracing::info!(target: "opengrid", root = %root.display(), "watching .grd files");
        while let Ok(ev) = rx.recv() {
            let mut relevant = is_grd_event(&ev);
            let deadline = Instant::now() + Duration::from_millis(150);
            while let Ok(ev) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                relevant |= is_grd_event(&ev);
            }
            if relevant {
                generation.fetch_add(1, Ordering::SeqCst);
                tracing::info!(target: "opengrid", "source changed; workers reload before their next request");
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!(target: "opengrid", "cannot start watcher thread: {e}");
    }
}
