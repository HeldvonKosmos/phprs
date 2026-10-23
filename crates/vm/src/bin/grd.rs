//! `grd` command line: `grd parse <files…>` and (later) `grd run <file>`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("parse") if args.len() > 2 => {
            let mut failed = 0usize;
            for path in &args[2..] {
                if !parse_one(path) {
                    failed += 1;
                }
            }
            println!("parsed {} files, {} with errors", args.len() - 2, failed);
            if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        Some("compile") if args.len() > 2 => {
            let mut failed = 0usize;
            for path in &args[2..] {
                let source = match std::fs::read(path) {
                    Ok(s) => s,
                    Err(e) => {
                        println!("ERR  {path}: {e}");
                        failed += 1;
                        continue;
                    }
                };
                match grd_vm::compile::compile_source(path, &source) {
                    Ok(m) => println!("OK   {path} ({} units, {} classes, {} functions)", m.units.len(), m.classes.len(), m.functions.len()),
                    Err(e) => {
                        println!("ERR  {path}: {}", e.to_string().replace('\n', " | "));
                        failed += 1;
                    }
                }
            }
            println!("compiled {} files, {} with errors", args.len() - 2, failed);
            if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        Some("run") => {
            eprintln!("grd: `run` is not implemented yet (phase 0: parser only)");
            ExitCode::from(2)
        }
        _ => {
            eprintln!("usage: grd parse <file.grd>… | grd compile <file.grd>… | grd run <file.grd>");
            ExitCode::from(64)
        }
    }
}

fn parse_one(path: &str) -> bool {
    let source = match std::fs::read(path) {
        Ok(s) => s,
        Err(e) => {
            println!("ERR  {path}: {e}");
            return false;
        }
    };
    let errors = grd_vm::parse::syntax_errors(path, &source);
    if errors.is_empty() {
        println!("OK   {path}");
        true
    } else {
        for e in &errors {
            println!("ERR  {path}:{}: {}", grd_vm::parse::line_of(&source, e.offset), e.message);
        }
        false
    }
}
