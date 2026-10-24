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
use crate::native::database::schema::kit::{self, SyncOptions};
use crate::native::database::schema::ops as schema_ops;
use crate::worker::{Pool, PoolOptions};

#[derive(Clone)]
pub struct PoolHandle(pub Arc<Pool>);

pub struct OpenGridApp;

/// The process's VM pool, started by the first `after_context`.
static PROCESS_POOL: tokio::sync::Mutex<Option<Arc<Pool>>> = tokio::sync::Mutex::const_new(None);

/// `settings.opengrid.root` / `OPENGRID_ROOT` (default `.`),
/// `settings.opengrid.workers` (default: cores), `debug` and `watch`
/// (default: `dev`, i.e. the development environment).
pub fn pool_options(settings: Option<&serde_json::Value>, dev: bool) -> PoolOptions {
    let og = settings.and_then(|s| s.get("opengrid"));
    let root = std::env::var("OPENGRID_ROOT")
        .ok()
        .or_else(|| og.and_then(|o| o.get("root")).and_then(|r| r.as_str()).map(str::to_string))
        .unwrap_or_else(|| ".".to_string());
    let mut opts = PoolOptions::new(root).settings(settings.cloned().unwrap_or(serde_json::Value::Null));
    if let Some(n) = og.and_then(|o| o.get("workers")).and_then(|w| w.as_u64()) {
        opts = opts.workers(n as usize);
    }
    if let Some(cfg) = crate::native::database::surreal::DbConfig::from_settings(settings) {
        opts = opts.database(cfg);
    }
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

/// A plain Axum router with the catch-all, for tests and embedding.
pub fn router(pool: Arc<Pool>) -> Router {
    async fn h(State(pool): State<Arc<Pool>>, req: axum::extract::Request) -> Response {
        handle_axum(pool, req).await
    }
    Router::new().route("/", any(h)).route("/{*path}", any(h)).with_state(pool)
}

async fn catch_all(State(ctx): State<AppContext>, req: axum::extract::Request) -> Response {
    match ctx.shared_store.get::<PoolHandle>() {
        Some(PoolHandle(pool)) => handle_axum(pool, req).await,
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
        tracing::info!(target: "grd", root = %opts.root.display(), workers = opts.workers, debug = opts.debug, watch = opts.watch, "starting VM pool");
        // One pool (and one database connection) per process: Loco's CLI
        // builds the app context twice on `start` (once for the logger, once
        // in create_app), and an embedded surrealkv database can be opened
        // only once per process (DECISIONS: surrealkv keeps its lock).
        let mut slot = PROCESS_POOL.lock().await;
        let pool = match slot.as_ref() {
            Some(p) => Arc::clone(p),
            None => {
                // Pool::start blocks (workers load the project, the database
                // connects with block_on), so it must not run on a Tokio thread.
                let p = tokio::task::spawn_blocking(move || Pool::start(opts))
                    .await
                    .map_err(|e| loco_rs::Error::Message(format!("pool start: {e}")))?
                    .map_err(loco_rs::Error::Message)?;
                let p = Arc::new(p);
                *slot = Some(Arc::clone(&p));
                p
            }
        };
        drop(slot);
        ctx.shared_store.insert(PoolHandle(pool));
        Ok(ctx)
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes().add_route(Routes::new().add("/", any(catch_all)).add("/{*path}", any(catch_all)))
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
        if !matches!(ctx.environment, Environment::Production) {
            return Ok(());
        }
        let Some(PoolHandle(pool)) = ctx.shared_store.get::<PoolHandle>() else { return Ok(()) };
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
        for action in [SchemaAction::Push, SchemaAction::Pull, SchemaAction::Fresh, SchemaAction::Rollout] {
            tasks.register(SchemaTask { action });
        }
    }
}

#[derive(Clone, Copy)]
enum SchemaAction {
    Push,
    Pull,
    Fresh,
    Rollout,
}

/// Schema tasks (R4d; development tools, no preflight):
/// `opengrid task migrate_push`, `migrate_pull`, `migrate_fresh force:true [keep_db:true]`,
/// `opengrid task rollout cmd:baseline|plan|start|complete|rollback|status [name:…] [id:…]`.
struct SchemaTask {
    action: SchemaAction,
}

fn flag(vars: &Vars, key: &str) -> bool {
    vars.cli_arg(key).is_ok_and(|v| matches!(v, "true" | "yes" | "1"))
}

#[async_trait]
impl Task for SchemaTask {
    fn task(&self) -> TaskInfo {
        let (name, detail) = match self.action {
            SchemaAction::Push => ("migrate_push", "merge the modules' .surql schema and sync the database (development)"),
            SchemaAction::Pull => ("migrate_pull", "print the database's current schema in .surql form"),
            SchemaAction::Fresh => ("migrate_fresh", "empty the database and push the schema again: force:true [keep_db:true]"),
            SchemaAction::Rollout => ("rollout", "SurrealKit rollouts: cmd:baseline|plan|start|complete|rollback|status [name:…] [id:…]"),
        };
        TaskInfo { name: name.into(), detail: detail.into() }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let msg = loco_rs::Error::Message;
        let Some(PoolHandle(pool)) = ctx.shared_store.get::<PoolHandle>() else {
            return Err(msg("VM pool not started".into()));
        };
        let Some(db) = pool.db() else { return Err(msg("no database configured (settings.opengrid.database or SURREAL_DB_HOST)".into())) };
        let root = pool.options().root.clone();
        let database = db.config.database.clone();
        let action = self.action;
        let (force, keep_db) = (flag(vars, "force"), flag(vars, "keep_db"));
        let cmd = vars.cli_arg("cmd").ok().map(str::to_string).unwrap_or_default();
        let name = vars.cli_arg("name").ok().map(str::to_string);
        let id = vars.cli_arg("id").ok().map(str::to_string);
        if matches!(action, SchemaAction::Fresh) && !force {
            return Err(msg("migrate_fresh deletes all data; run it with force:true".into()));
        }
        let out = tokio::task::spawn_blocking(move || {
            db.block_on(async {
                let c = db.client();
                match action {
                    SchemaAction::Push => schema_ops::push(c, &root, SyncOptions::default()).await.map(|(_, w)| format!("schema in sync ({} skipped statements)", w.len())),
                    SchemaAction::Pull => schema_ops::pull(c).await,
                    SchemaAction::Fresh => schema_ops::fresh(c, &root, &database, keep_db).await.map(|_| "database emptied, schema pushed".to_string()),
                    SchemaAction::Rollout => match cmd.as_str() {
                        "baseline" => kit::rollout_baseline(c, &root).await.map(|_| "baseline recorded".to_string()),
                        "plan" => {
                            let n = name.ok_or_else(|| kit::KitError("rollout cmd:plan needs name:<name>".into()))?;
                            schema_ops::plan(c, &root, &n).await.map(|(id, r)| format!("planned {id}\n{}", r.summary()))
                        }
                        "start" => kit::rollout_start(c, &root, id.as_deref()).await.map(|_| "rollout started".to_string()),
                        "complete" => kit::rollout_complete(c, &root, id.as_deref()).await.map(|_| "rollout completed".to_string()),
                        "rollback" => kit::rollout_rollback(c, &root, id.as_deref()).await.map(|_| "rollout rolled back".to_string()),
                        "status" => kit::rollout_status(c, &root).await,
                        other => Err(kit::KitError(format!("unknown rollout cmd `{other}` (baseline, plan, start, complete, rollback, status)"))),
                    },
                }
            })
        })
        .await
        .map_err(|e| msg(format!("task: {e}")))?
        .map_err(|e| msg(e.0))?;
        println!("{out}");
        Ok(())
    }
}
