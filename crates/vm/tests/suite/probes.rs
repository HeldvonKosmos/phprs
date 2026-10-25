//! Frozen probes: every `tests/probes/*.grd` must produce exactly the
//! output in the matching `.out` file (captured from PHP 8.4).

use std::path::Path;

fn run(path: &Path, gc_threshold: Option<usize>) -> String {
    let source = std::fs::read(path).expect("read probe");
    let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
    if let Some(t) = gc_threshold {
        vm.set_gc_threshold(t);
    }
    let result = vm.run_file(path.to_string_lossy().as_ref(), &source);
    let mut out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = result {
        out.push_str(&e.to_string());
    }
    out
}

#[test]
fn frozen_probes_match_php_output() {
    check_probes(None);
}

/// MEM1b: the same probes with a collection whenever the live objects have
/// doubled from one: a missing GC root shows up as a wrong result or a
/// dangling handle.
#[test]
fn frozen_probes_survive_constant_collection() {
    check_probes(Some(1));
}

fn check_probes(gc_threshold: Option<usize>) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/probes");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("probes dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "grd"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty());
    let mut failures = Vec::new();
    for probe in &entries {
        let expected = std::fs::read_to_string(probe.with_extension("out")).expect("expected output");
        let got = run(probe, gc_threshold);
        if got.trim_end() != expected.trim_end() {
            failures.push(format!(
                "{}\n  expected: {:?}\n  got:      {:?}",
                probe.file_name().unwrap().to_string_lossy(),
                expected,
                got
            ));
        }
    }
    assert!(failures.is_empty(), "{} probe(s) regressed:\n{}", failures.len(), failures.join("\n"));
}
