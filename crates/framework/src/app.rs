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
use loco_rs::task::Tasks;
use loco_rs::Result;

use crate::dispatch;
use crate::http::{HttpResponse, from_axum, into_axum};
use crate::worker::{Pool, PoolOptions};

#[derive(Clone)]
pub struct PoolHandle(pub Arc<Pool>);

pub struct OpenGridApp;

/// `settings.opengrid.root` / `OPENGRID_ROOT` (default `.`) and
/// `settings.opengrid.workers` (default: cores).
pub fn pool_options(settings: Option<&serde_json::Value>) -> PoolOptions {
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
        let opts = pool_options(ctx.config.settings.as_ref());
        tracing::info!(target: "grd", root = %opts.root.display(), workers = opts.workers, "starting VM pool");
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

    fn register_tasks(_tasks: &mut Tasks) {}
}
