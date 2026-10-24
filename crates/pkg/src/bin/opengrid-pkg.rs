//! `opengrid-pkg <command>`: the module manager as its own binary (the
//! `opengrid` CLI will embed it as `opengrid pkg …`).

use clap::Parser;
use env_logger::Env;

#[derive(Parser)]
#[command(name = "opengrid-pkg", version, about = "OpenGrid module manager (Composer-compatible)")]
struct Cli {
    #[command(subcommand)]
    command: opengrid_pkg::PkgCommands,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::try_init_from_env(Env::default().filter_or("OPENGRID_LOG", "info"))?;
    opengrid_pkg::execute(Cli::parse().command).await
}
