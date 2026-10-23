//! `opengrid start` (Loco CLI): serves the project in the current directory
//! (or `OPENGRID_ROOT`, or `settings.opengrid.root`).

use loco_rs::cli;
use opengrid_framework::app::OpenGridApp;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    cli::main::<OpenGridApp>().await
}
