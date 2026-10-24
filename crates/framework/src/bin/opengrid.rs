//! `opengrid start` (Loco CLI): serves the project in the current directory
//! (or `OPENGRID_ROOT`, or `settings.opengrid.root`).

use loco_rs::cli;
use opengrid_framework::app::OpenGridApp;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    opengrid_framework::set_task_mode(std::env::args().nth(1).as_deref() == Some("task"));
    cli::main::<OpenGridApp>().await
}
