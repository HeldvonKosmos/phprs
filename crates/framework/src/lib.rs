//! OpenGrid framework crate (ROADMAP §1.2): Loco hooks, the VM worker pool,
//! catch-all routing into dialect controllers, and the native classes the
//! dialect sees. The interpreter lives in `grd-vm` and knows nothing of HTTP.

pub mod app;
pub mod dispatch;
pub mod http;
pub mod native;
pub mod routes;
pub mod worker;

pub use http::{HttpRequest, HttpResponse};
pub use worker::{Pool, PoolOptions, WorkerCtx};
