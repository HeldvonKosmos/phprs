//! `opengrid start` (Loco CLI): serves the project in the current directory
//! (or `OPENGRID_ROOT`, or `settings.opengrid.root`). `opengrid new <dir>`
//! creates a project (TP1).

use loco_rs::cli;
use opengrid_framework::app::OpenGridApp;

/// mimalloc instead of the system allocator: the VM allocates many small
/// values (strings, arrays, frames); PERF2 measurement in BENCHMARKS.md.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    // `opengrid new <dir> [--name <Name>]` runs before Loco, which needs a project
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("new") {
        let Some(dir) = args.get(2).filter(|a| !a.starts_with('-')) else {
            eprintln!("usage: opengrid new <dir> [--name <Name>]");
            std::process::exit(64);
        };
        let name = args.iter().position(|a| a == "--name").and_then(|i| args.get(i + 1)).map(String::as_str);
        match opengrid_framework::scaffold::new_project(std::path::Path::new(dir), name) {
            Ok(files) => {
                println!("created {dir}:");
                for f in files {
                    println!("  {}", f.display());
                }
                println!("\nnext: cd {dir} && opengrid start   (then: curl http://localhost:5150/)");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("opengrid new: {e}");
                std::process::exit(1);
            }
        }
    }
    opengrid_framework::set_task_mode(std::env::args().nth(1).as_deref() == Some("task"));
    cli::main::<OpenGridApp>().await
}
