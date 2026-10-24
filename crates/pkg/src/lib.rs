//! OpenGrid module manager (Composer-compatible). Moved from the old
//! `bin/phprs/src/pkg`; the class scan no longer needs the legacy lexer.

pub mod commands;
pub mod composer;
pub mod config;
pub mod error;
pub mod registry;
pub mod resolver;

use clap::Subcommand;
use commands::{Build, Init, Install, Publish, Run, Update};

#[derive(Subcommand)]
pub enum PkgCommands {
    /// Initialize a new PHP project
    Init(Init),
    /// Install dependencies from composer.json
    Install(Install),
    /// Update dependencies to their latest versions
    Update(Update),
    /// Build PHP project to executable
    Build(Build),
    /// Run scripts defined in composer.json
    Run(Run),
    /// Publish package to registry
    Publish(Publish),
}

pub async fn execute(command: PkgCommands) -> anyhow::Result<()> {
    match command {
        PkgCommands::Init(cmd) => cmd.execute().await,
        PkgCommands::Install(cmd) => cmd.execute().await,
        PkgCommands::Update(cmd) => cmd.execute().await,
        PkgCommands::Build(cmd) => cmd.execute().await,
        PkgCommands::Run(cmd) => cmd.execute().await,
        PkgCommands::Publish(cmd) => cmd.execute().await,
    }
}
