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
        Some("run") if args.len() > 2 => run_file(&args[2]),
        Some("app") if args.len() > 2 => run_app(&args[2], args.get(3).map(String::as_str)),
        Some("dump") if args.len() > 2 => {
            let source = std::fs::read(&args[2]).expect("read");
            match grd_vm::compile::compile_source(&args[2], &source) {
                Ok(m) => {
                    for (ui, u) in m.units.iter().enumerate() {
                        println!("== unit {ui} {} (slots {}, params {})", u.name, u.num_slots, u.params.len());
                        for (i, op) in u.ops.iter().enumerate() {
                            println!("{i:5}  {op:?}");
                        }
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(1)
                }
            }
        }
        _ => {
            eprintln!("usage: grd parse <file.grd>… | grd compile <file.grd>… | grd dump <file.grd> | grd run <file.grd> | grd app <root> [entry.grd]");
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

fn run_file(path: &str) -> ExitCode {
    let source = match std::fs::read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("grd: cannot read {path}: {e}");
            return ExitCode::from(1);
        }
    };
    let mut vm = match grd_vm::vm::Vm::with_prelude() {
        Ok(mut vm) => {
            grd_jit::install(&mut vm);
            vm
        }
        Err(e) => {
            eprintln!("grd: prelude failed: {e}");
            return ExitCode::from(70);
        }
    };
    let result = vm.run_file(path, &source);
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&vm.output);
    let _ = stdout.flush();
    #[cfg(feature = "opstats")]
    eprint!("{}", grd_vm::vm::opstats::report());
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(grd_vm::vm::RunError::Compile(e)) => {
            eprintln!("{e}");
            ExitCode::from(255)
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(255)
        }
    }
}

fn run_app(root: &str, entry: Option<&str>) -> ExitCode {
    let mut vm = match grd_vm::vm::Vm::with_prelude() {
        Ok(vm) => vm,
        Err(e) => {
            eprintln!("grd: prelude failed: {e}");
            return ExitCode::from(70);
        }
    };
    let result = vm.load_project(std::path::Path::new(root));
    use std::io::Write;
    match result {
        Ok(summary) => {
            eprintln!("grd: loaded {} module(s), {} class file(s), {} route file(s)", summary.modules.len(), summary.class_files, summary.route_files);
        }
        Err(e) => {
            let _ = std::io::stdout().lock().write_all(&vm.output);
            eprintln!("{e}");
            return ExitCode::from(255);
        }
    }
    let mut code = ExitCode::SUCCESS;
    if let Some(path) = entry {
        let source = match std::fs::read(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("grd: cannot read {path}: {e}");
                return ExitCode::from(1);
            }
        };
        if let Err(e) = vm.run_file(path, &source) {
            eprintln!("{e}");
            code = ExitCode::from(255);
        }
    }
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&vm.output);
    let _ = stdout.flush();
    code
}
