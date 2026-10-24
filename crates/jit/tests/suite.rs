//! JIT correctness (JIT1b): every frozen probe of grd-vm runs with the JIT
//! compiling as eagerly as possible (threshold 1) and must print exactly what
//! PHP 8.4 printed.

use std::path::Path;

#[test]
fn probes_match_php_with_the_jit() {
    // SAFETY: the only test in this binary; set before any VM reads it
    unsafe { std::env::set_var("GRD_JIT_HOT", "1") };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vm/tests/probes");
    let mut probes: Vec<_> = std::fs::read_dir(&dir).expect("probes dir").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "grd")).collect();
    probes.sort();
    let mut failures = Vec::new();
    let mut compiled = 0;
    for probe in &probes {
        let source = std::fs::read(probe).expect("read probe");
        let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
        grd_jit::install(&mut vm);
        let r = vm.run_file(&probe.display().to_string(), &source);
        let mut out = String::from_utf8_lossy(&vm.output).into_owned();
        if let Err(e) = r {
            out.push_str(&format!("{e}"));
        }
        let expected = std::fs::read_to_string(probe.with_extension("out")).expect("expected output");
        if out.trim_end() != expected.trim_end() {
            failures.push(format!("{}\n  expected: {:?}\n  got:      {:?}", probe.display(), expected, out));
        }
        compiled += 1;
    }
    assert!(failures.is_empty(), "{} of {} probes differ with the JIT:\n{}", failures.len(), compiled, failures.join("\n"));
}
