//! Loco `Hooks` for OpenGrid: the pool goes into the shared store on boot,
//! a catch-all route hands every request to a worker.

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::any;
use loco_rs::app::{AppContext, Hooks};
use loco_rs::bgworker::Queue;
use loco_rs::boot::{BootResult, StartMode, create_app};
use loco_rs::config::Config;
use loco_rs::controller::AppRoutes;
use loco_rs::environment::Environment;
use loco_rs::prelude::Routes;
use loco_rs::task::{Task, TaskInfo, Tasks, Vars};
use loco_rs::Result;

use crate::dispatch;
use crate::http::{HttpResponse, from_axum, into_axum};

use crate::native::database::schema::ops as schema_ops;
use crate::worker::{Pool, PoolOptions};

/// The process's VM pool, started on first use (DB1): Loco builds the app
/// context once before the logger exists and once more in `create_app`;
/// starting the pool lazily — in `before_run` for the server, in `run` for a
/// task — means it starts once, after logging is set up, and an embedded
/// database is opened only once per process.
#[derive(Clone)]
pub struct PoolHandle(pub Arc<LazyPool>);

pub struct LazyPool {
    options: PoolOptions,
    pool: tokio::sync::OnceCell<Arc<Pool>>,
}

impl LazyPool {
    pub fn options(&self) -> &PoolOptions {
        &self.options
    }

    pub async fn get(&self) -> Result<Arc<Pool>> {
        self.pool
            .get_or_try_init(|| async {
                let opts = self.options.clone();
                tracing::info!(target: "opengrid", root = %opts.root.display(), workers = opts.workers, debug = opts.debug, watch = opts.watch, "starting VM pool");
                // Pool::start blocks (workers load the project, the database
                // connects with block_on), so it must not run on a Tokio thread.
                let p = tokio::task::spawn_blocking(move || Pool::start(opts))
                    .await
                    .map_err(|e| loco_rs::Error::Message(format!("pool start: {e}")))?
                    .map_err(loco_rs::Error::Message)?;
                Ok::<_, loco_rs::Error>(Arc::new(p))
            })
            .await
            .cloned()
    }
}

pub struct OpenGridApp;

/// `settings.opengrid.root` / `OPENGRID_ROOT` (default `.`),
/// `settings.opengrid.workers` (default: cores), `debug` and `watch`
/// (default: `dev`, i.e. the development environment).
pub fn pool_options(settings: Option<&serde_json::Value>, dev: bool) -> PoolOptions {
    let og = settings.and_then(|s| s.get("opengrid"));
    let root = std::env::var("OPENGRID_ROOT")
        .ok()
        .or_else(|| og.and_then(|o| o.get("root")).and_then(|r| r.as_str()).map(str::to_string))
        .unwrap_or_else(|| ".".to_string());
    // absolute, so an embedded database path does not depend on the working directory
    let root = std::env::current_dir().map(|d| d.join(&root)).unwrap_or_else(|_| root.into());
    let root = root.canonicalize().unwrap_or(root);
    let mut opts = PoolOptions::new(root.clone()).settings(settings.cloned().unwrap_or(serde_json::Value::Null));
    if let Some(n) = og.and_then(|o| o.get("workers")).and_then(|w| w.as_u64()) {
        opts = opts.workers(n as usize);
    }
    // DB1: the embedded surrealkv database in `.opengrid/db` unless configured
    // (settings.opengrid.database, OPENGRID_DB_URL, SURREAL_DB_*)
    use crate::native::database::surreal::{DEFAULT_LOCK_WAIT, DbConfig};
    let mut cfg = DbConfig::from_settings(settings).unwrap_or_else(|| DbConfig::embedded_default(&root)).resolve(&root);
    let wait = og.and_then(|o| o.get("database")).and_then(|d| d.get("lock_wait")).and_then(|v| v.as_u64()).map(std::time::Duration::from_secs).unwrap_or(DEFAULT_LOCK_WAIT);
    // a CLI task does not wait for a running server; it reports the lock at once
    cfg.lock_wait = if crate::is_task_mode() { std::time::Duration::ZERO } else { wait };
    opts = opts.database(cfg);
    let flag = |k: &str| og.and_then(|o| o.get(k)).and_then(|v| v.as_bool());
    opts = opts.debug(flag("debug").unwrap_or(dev)).watch(flag("watch").unwrap_or(dev));
    opts = opts.session(crate::session::SessionConfig::from_settings(settings));
    opts
}

/// The whole request path without Loco: used by tests and embedders.
pub async fn handle_axum(pool: Arc<Pool>, req: axum::extract::Request) -> Response {
    let req = match from_axum(req).await {
        Ok(r) => r,
        Err(resp) => return into_axum(resp),
    };
    match pool.run(move |vm| dispatch::handle(vm, req)).await {
        Ok(resp) => into_axum(resp),
        Err(_) => into_axum(HttpResponse::json(500, br#"{"error":"WorkerFailed","message":"the worker did not answer"}"#.to_vec())),
    }
}

/// `GET /health` (OPS1): 200 with the app name and the database's mode and
/// version once the workers and the database answer; 503 otherwise. Blocking
/// (the database client has its own runtime): call it off the async threads.
pub fn health_response(pool: &Pool) -> HttpResponse {
    let app = pool.options().settings["app"]["name"].as_str().unwrap_or("").to_string();
    let database = match pool.db() {
        Some(db) => {
            let (mode, version) = db.status();
            if version.is_none() {
                return HttpResponse::json(503, serde_json::json!({ "status": "unavailable", "app": app, "database": { "mode": mode } }).to_string().into_bytes());
            }
            serde_json::json!({ "mode": mode, "version": version })
        }
        None => serde_json::json!({ "mode": "none" }),
    };
    HttpResponse::json(200, serde_json::json!({ "status": "ok", "app": app, "database": database }).to_string().into_bytes())
}

async fn health_axum(pool: Arc<Pool>) -> Response {
    match tokio::task::spawn_blocking(move || health_response(&pool)).await {
        Ok(resp) => into_axum(resp),
        Err(_) => into_axum(HttpResponse::json(503, br#"{"status":"unavailable"}"#.to_vec())),
    }
}

/// A plain Axum router with the catch-all, for tests and embedding.
pub fn router(pool: Arc<Pool>) -> Router {
    async fn h(State(pool): State<Arc<Pool>>, req: axum::extract::Request) -> Response {
        handle_axum(pool, req).await
    }
    async fn health(State(pool): State<Arc<Pool>>) -> Response {
        health_axum(pool).await
    }
    Router::new().route("/health", axum::routing::get(health)).route("/", any(h)).route("/{*path}", any(h)).with_state(pool)
}

async fn health(State(ctx): State<AppContext>) -> Response {
    match ctx.shared_store.get::<PoolHandle>() {
        Some(PoolHandle(lazy)) => match lazy.get().await {
            Ok(pool) => health_axum(pool).await,
            Err(e) => into_axum(HttpResponse::json(503, serde_json::json!({ "status": "unavailable", "message": e.to_string() }).to_string().into_bytes())),
        },
        None => into_axum(HttpResponse::json(503, br#"{"status":"unavailable"}"#.to_vec())),
    }
}

async fn catch_all(State(ctx): State<AppContext>, req: axum::extract::Request) -> Response {
    match ctx.shared_store.get::<PoolHandle>() {
        Some(PoolHandle(lazy)) => match lazy.get().await {
            Ok(pool) => handle_axum(pool, req).await,
            Err(e) => into_axum(HttpResponse::json(503, format!("{{\"error\":\"PoolUnavailable\",\"message\":{:?}}}", e.to_string()).into_bytes())),
        },
        None => into_axum(HttpResponse::json(500, br#"{"error":"NoPool","message":"VM pool not started"}"#.to_vec())),
    }
}

#[async_trait]
impl Hooks for OpenGridApp {
    fn app_name() -> &'static str {
        "opengrid"
    }

    async fn boot(mode: StartMode, environment: &Environment, config: Config) -> Result<BootResult> {
        create_app::<Self>(mode, environment, config).await
    }

    async fn after_context(ctx: AppContext) -> Result<AppContext> {
        // SurrealKit logs through `log`; forward it into tracing (D4)
        crate::native::database::schema::kit::forward_logs();
        let dev = matches!(ctx.environment, Environment::Development);
        let opts = pool_options(ctx.config.settings.as_ref(), dev);
        ctx.shared_store.insert(PoolHandle(Arc::new(LazyPool { options: opts, pool: tokio::sync::OnceCell::new() })));
        Ok(ctx)
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes().add_route(Routes::new().add("/health", axum::routing::get(health)).add("/", any(catch_all)).add("/{*path}", any(catch_all)))
    }

    async fn connect_workers(_ctx: &AppContext, _queue: &Queue) -> Result<()> {
        Ok(())
    }

    /// Production start (runs only when the server or workers start, not for
    /// `opengrid task …`): run the planned rollouts after their preflight
    /// check. Any failure is returned, so the process exits with an error and
    /// the deployment (Nomad canary) fails; development and test never touch
    /// the schema on start (DECISIONS "R4d: Schema beim Start nur in Produktion").
    async fn before_run(ctx: &AppContext) -> Result<()> {
        let Some(PoolHandle(lazy)) = ctx.shared_store.get::<PoolHandle>() else { return Ok(()) };
        // start the pool (and open the database) now, with logging in place
        let pool = lazy.get().await?;
        // J1: `opengrid task …` reaches this process through the control socket
        let p = Arc::clone(&pool);
        tokio::spawn(async move {
            if let Err(e) = crate::control::serve(p).await {
                tracing::error!(target: "opengrid", "control socket: {e}");
            }
        });
        // J3: the schedule runs in this process, every full minute
        crate::scheduler::start(Arc::clone(&pool));
        // J4: durable jobs run in the server (task pools only write rows)
        crate::native::jobs::start_runner(&pool, std::time::Duration::from_secs(1));
        if !matches!(ctx.environment, Environment::Production) {
            return Ok(());
        }
        let Some(db) = pool.db() else { return Ok(()) };
        let root = pool.options().root.clone();
        let report = tokio::task::spawn_blocking(move || db.block_on(async { schema_ops::apply_planned(db.client(), &root).await }))
            .await
            .map_err(|e| loco_rs::Error::Message(format!("schema start: {e}")))?
            .map_err(|e| loco_rs::Error::Message(format!("schema start failed, server stops: {e}")))?;
        tracing::info!(target: "grd", applied = ?report.applied, "schema: planned rollouts applied");
        Ok(())
    }

    fn register_tasks(tasks: &mut Tasks) {
        for (name, detail) in TASKS {
            tasks.register(OpenGridTask { name, detail });
        }
    }
}

/// The tasks of `opengrid task …`. In a project with a running server they
/// run inside it (J1, control socket), otherwise in this process.
const TASKS: [(&str, &str); 9] = [
    ("queue_failed", "list jobs that failed for good (table failed_jobs)"),
    ("queue_retry", "queue failed jobs again: id:<uuid> | id:all"),
    ("queue_flush", "delete all failed jobs"),
    ("module", "modules from Git: cmd:add url:<repo> [rev:<tag|branch|commit>] | cmd:remove name:<m> | cmd:list | cmd:verify"),
    ("migrate_push", "merge the modules' .surql schema and sync the database (development)"),
    ("migrate_pull", "print the database's current schema in .surql form"),
    ("migrate_fresh", "empty the database and push the schema again: force:true [keep_db:true]"),
    ("rollout", "SurrealKit rollouts: cmd:baseline|plan|start|complete|rollback|status [name:…] [id:…]"),
    ("run", "a dialect task: task:<module>:<name> [key:value …] (class <Module>\\Tasks\\<Name>)"),
];

struct OpenGridTask {
    name: &'static str,
    detail: &'static str,
}

#[async_trait]
impl Task for OpenGridTask {
    fn task(&self) -> TaskInfo {
        TaskInfo { name: self.name.into(), detail: self.detail.into() }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let msg = loco_rs::Error::Message;
        let Some(PoolHandle(lazy)) = ctx.shared_store.get::<PoolHandle>() else {
            return Err(msg("VM pool not configured".into()));
        };
        let (name, args) = (self.name.to_string(), vars.cli.clone());
        let root = lazy.options().root.clone();
        // a running server first: the embedded database belongs to it
        let forwarded = {
            let (root, name, args) = (root.clone(), name.clone(), args.clone());
            tokio::task::spawn_blocking(move || crate::control::forward(&root, &name, &args)).await.map_err(|e| msg(format!("task: {e}")))?
        };
        let result = match forwarded {
            Some(r) => r,
            // the module manager needs no database: no pool
            None if name == "module" => {
                let options = lazy.options().clone();
                tokio::task::spawn_blocking(move || crate::control::module_task(&options, &args, None)).await.map_err(|e| msg(format!("task: {e}")))?
            }
            None => {
                let pool = lazy.get().await?;
                tokio::task::spawn_blocking(move || crate::control::run_task(&pool, &name, &args)).await.map_err(|e| msg(format!("task: {e}")))?
            }
        };
        match result {
            Ok(out) => {
                if !out.is_empty() {
                    println!("{}", out.trim_end());
                }
                Ok(())
            }
            Err(e) => Err(msg(e)),
        }
    }
}
