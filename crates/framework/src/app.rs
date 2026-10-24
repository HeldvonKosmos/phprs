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
use crate::worker::{Pool, PoolOptions};

#[derive(Clone)]
pub struct PoolHandle(pub Arc<Pool>);

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
        // Pool::start blocks (workers load the project, the database connects
        // with block_on), so it must not run on a Tokio thread.
        let pool = tokio::task::spawn_blocking(move || Pool::start(opts))
            .await
            .map_err(|e| loco_rs::Error::Message(format!("pool start: {e}")))?
            .map_err(loco_rs::Error::Message)?;
        ctx.shared_store.insert(PoolHandle(Arc::new(pool)));
        Ok(ctx)
    }

    fn routes(_ctx: &AppContext) -> AppRoutes {
        AppRoutes::with_default_routes().add_route(Routes::new().add("/", any(catch_all)).add("/{*path}", any(catch_all)))
    }

    async fn connect_workers(_ctx: &AppContext, _queue: &Queue) -> Result<()> {
        Ok(())
    }

    fn register_tasks(tasks: &mut Tasks) {
        tasks.register(MigrateTask { action: MigrateAction::Run });
        tasks.register(MigrateTask { action: MigrateAction::Status });
        tasks.register(MigrateTask { action: MigrateAction::Rollback });
    }
}

#[derive(Clone, Copy)]
enum MigrateAction {
    Run,
    Status,
    Rollback,
}

/// `opengrid task migrate`, `opengrid task migrate_status`,
/// `opengrid task migrate_rollback steps:2`.
struct MigrateTask {
    action: MigrateAction,
}

#[async_trait]
impl Task for MigrateTask {
    fn task(&self) -> TaskInfo {
        let (name, detail) = match self.action {
            MigrateAction::Run => ("migrate", "run pending .grd migrations"),
            MigrateAction::Status => ("migrate_status", "list ran and pending migrations"),
            MigrateAction::Rollback => ("migrate_rollback", "roll back the last batch(es): steps:N"),
        };
        TaskInfo { name: name.into(), detail: detail.into() }
    }

    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let Some(PoolHandle(pool)) = ctx.shared_store.get::<PoolHandle>() else {
            return Err(loco_rs::Error::Message("VM pool not started".into()));
        };
        let root = pool.options().root.clone();
        let action = self.action;
        let steps = vars.cli_arg("steps").ok().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1);
        let report = pool
            .run(move |vm| {
                use crate::native::database::migrator;
                let r = match action {
                    MigrateAction::Run => migrator::migrate(vm, &root),
                    MigrateAction::Status => migrator::status(vm, &root),
                    MigrateAction::Rollback => migrator::rollback(vm, &root, steps),
                };
                vm.reset_request();
                r.map_err(|t| format!("{}: {}", t.class, t.message))
            })
            .await
            .map_err(|_| loco_rs::Error::Message("worker failed".into()))?
            .map_err(loco_rs::Error::Message)?;
        for (name, batch) in &report.ran {
            println!("ran      {name} (batch {batch})");
        }
        for name in &report.pending {
            println!("pending  {name}");
        }
        for name in &report.executed {
            println!("executed {name}");
        }
        Ok(())
    }
}
