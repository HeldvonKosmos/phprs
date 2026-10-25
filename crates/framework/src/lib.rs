//! OpenGrid framework crate (ROADMAP §1.2): Loco hooks, the VM worker pool,
//! catch-all routing into dialect controllers, and the native classes the
//! dialect sees. The interpreter lives in `grd-vm` and knows nothing of HTTP.

#![forbid(unsafe_code)]

pub mod app;
pub mod control;
pub mod dispatch;
pub mod http;
pub mod native;
pub mod routes;
pub mod scaffold;
pub mod scheduler;
pub mod services;
pub mod session;
pub mod worker;

pub use http::{HttpRequest, HttpResponse};
pub use worker::{Pool, PoolOptions, WorkerCtx};

static TASK_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set by the `opengrid` binary for `opengrid task …`: the process does not
/// wait for an embedded database held by a running server (DB1).
pub fn set_task_mode(on: bool) {
    TASK_MODE.store(on, std::sync::atomic::Ordering::SeqCst);
}

pub fn is_task_mode() -> bool {
    TASK_MODE.load(std::sync::atomic::Ordering::SeqCst)
}
