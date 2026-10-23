//! PHP-RS Unified CLI
//!
//! Single binary with two commands:
//! - `phprs run <file>`   — Execute a script
//! - `phprs pkg <cmd>`    — Module manager

mod pkg;

use clap::{Parser, Subcommand};
use env_logger::Env;

#[derive(Parser)]
#[command(name = "phprs")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "OpenGrid dialect runtime and module manager")]
struct Cli {
    /// Sets the level of verbosity
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a PHP file
    Run {
        /// PHP file to execute
        file: String,
    },

    /// Module manager
    Pkg {
        #[command(subcommand)]
        command: pkg::PkgCommands,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let env = Env::default()
        .filter_or("PHPRS_LOG_LEVEL", "info")
        .write_style_or("PHPRS_LOG_STYLE", "always");
    env_logger::try_init_from_env(env)?;

    let cli = Cli::parse();

    match cli.command {
        Commands::Run { file } => cmd_run(&file),
        Commands::Pkg { command } => pkg::execute(command).await,
    }
}

/// Execute a PHP file
fn cmd_run(filename: &str) -> anyhow::Result<()> {
    use phprs::engine::compile::compile_file_with_functions;
    use phprs::engine::vm::{ExecuteData, execute_ex};
    use phprs::php::output::{php_output_end, php_output_start};
    use std::sync::Arc;

    let (op_array, function_table) = compile_file_with_functions(filename)
        .map_err(|e| anyhow::anyhow!("Compile error: {}", e))?;

    let _ = php_output_start();
    let mut ed = ExecuteData::new();
    ed.function_table = Some(Arc::new(function_table));
    let result = execute_ex(&mut ed, &op_array);
    let output = php_output_end().unwrap_or_default();

    print!("{}", output);
    if let Some(code) = ed.exit_requested {
        std::process::exit(code as i32);
    }
    if result == phprs::engine::types::PhpResult::Failure
        && let Some(thrown) = ed.pending_exception.take()
    {
        let (class, message) =
            phprs::engine::vm::exception_dispatch::thrown_class_and_message(&thrown);
        eprintln!("PHP Fatal error:  Uncaught {}: {}", class, message);
        std::process::exit(255);
    }
    Ok(())
}
