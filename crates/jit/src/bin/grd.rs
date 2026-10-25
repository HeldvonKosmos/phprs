//! `grd` command line: `grd parse <files…>` and (later) `grd run <file>`.

use std::process::ExitCode;

/// mimalloc instead of the system allocator: the VM allocates many small
/// values (strings, arrays, frames); PERF2 measurement in BENCHMARKS.md.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
        Some("check") if args.len() > 3 && args[2] == "--report" => check_report(&args[3..]),
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
            eprintln!("usage: grd parse <file.grd>… | grd compile <file.grd>… | grd dump <file.grd> | grd run <file.grd> | grd app <root> [entry.grd] | grd check --report <path>…");
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

/// Sources of a path: `.php`/`.grd` files as they are, PHP code blocks of
/// `.md` files (with an opening tag added when missing).
fn collect_sources(path: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
        entries.sort();
        for e in entries {
            collect_sources(&e, out);
        }
        return;
    }
    let name = path.display().to_string();
    match path.extension().and_then(|e| e.to_str()) {
        Some("php" | "grd") => {
            if let Ok(src) = std::fs::read(path) {
                out.push((name, src));
            }
        }
        Some("md") => {
            let Ok(text) = std::fs::read_to_string(path) else { return };
            let mut block: Option<(usize, String)> = None;
            for (i, line) in text.lines().enumerate() {
                let t = line.trim_start();
                match &mut block {
                    None if t.starts_with("```php") => block = Some((i + 1, String::new())),
                    Some((start, code)) if t.starts_with("```") => {
                        let code = if code.trim_start().starts_with("<?php") { code.clone() } else { format!("<?php\n{code}") };
                        out.push((format!("{name}:{start}"), code.into_bytes()));
                        block = None;
                    }
                    Some((_, code)) => {
                        code.push_str(line);
                        code.push('\n');
                    }
                    None => {}
                }
            }
        }
        _ => {}
    }
}

/// `grd check --report <path>…` (DX1): counts dialect errors per GRD code
/// and the GRD021 measurement; enforces nothing. Exit code 0.
fn check_report(paths: &[String]) -> ExitCode {
    use std::collections::{BTreeMap, BTreeSet};
    let mut sources = Vec::new();
    for p in paths {
        collect_sources(std::path::Path::new(p), &mut sources);
    }
    let mut by_code: BTreeMap<String, (u32, BTreeSet<String>)> = BTreeMap::new();
    let mut uncoded: BTreeMap<String, u32> = BTreeMap::new();
    let (mut syntax, mut panicked, mut clean) = (0u32, 0u32, 0u32);
    let mut g = grd_vm::compile::typecheck::Grd021Counts::default();
    let (mut g_files_strict, mut g_files_tolerant) = (0u32, 0u32);
    let mut examples: Vec<String> = Vec::new();
    for (name, src) in &sources {
        let r = grd_vm::compile::check_source(name, src);
        if r.syntax_error.is_some() {
            syntax += 1;
            continue;
        }
        if r.panicked {
            panicked += 1;
        }
        if r.errors.is_empty() && !r.panicked {
            clean += 1;
        }
        for e in &r.errors {
            match e.code {
                Some(code) => {
                    let entry = by_code.entry(code.to_string()).or_default();
                    entry.0 += 1;
                    entry.1.insert(name.clone());
                }
                None => {
                    let msg: String = e.message.chars().take(70).collect();
                    *uncoded.entry(msg).or_default() += 1;
                }
            }
        }
        let c = &r.grd021;
        g.strict += c.strict;
        g.tolerant += c.tolerant;
        g.int_float += c.int_float;
        g.judged += c.judged;
        g.not_judged += c.not_judged;
        g_files_strict += (c.strict > 0) as u32;
        g_files_tolerant += (c.tolerant > 0) as u32;
        if examples.len() < 15 {
            for l in &c.lines {
                examples.push(format!("{name}:{l}"));
            }
        }
    }
    println!("# grd check --report");
    println!("sources {} | syntax errors {syntax} | without dialect errors {clean} | compiler panics {panicked}", sources.len());
    println!("\n## dialect errors by code (hits, files)");
    for (code, (n, files)) in &by_code {
        println!("{code}\t{n}\t{}", files.len());
    }
    println!("\n## errors without GRD code (hits)");
    let mut un: Vec<_> = uncoded.into_iter().collect();
    un.sort_by(|a, b| b.1.cmp(&a.1));
    for (m, n) in un.iter().take(25) {
        println!("{n}\t{m}");
    }
    println!("\n## GRD021 (static, compile time)");
    println!("strict\t{}\tfiles {g_files_strict}", g.strict);
    println!("tolerant (?T)\t{}\tfiles {g_files_tolerant}", g.tolerant);
    println!("of strict: int<->float\t{}", g.int_float);
    println!("assignments judged\t{}\tnot judged\t{}", g.judged, g.not_judged);
    println!("examples (strict): {}", examples.iter().take(15).cloned().collect::<Vec<_>>().join(", "));
    ExitCode::SUCCESS
}
