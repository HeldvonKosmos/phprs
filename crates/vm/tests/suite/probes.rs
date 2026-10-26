//! Frozen probes: every `tests/probes/*.grd` must produce exactly the
//! output in the matching `.out` file (captured from PHP 8.4).
//!
//! `probes_are_php_output` runs PHP itself on every probe (a `<?php` copy):
//! PHP must print the `.out` exactly, except for the probes listed with their
//! DIALECT section in `tests/probes_deviations.txt`, where it must differ.
//! `GRD_PROBES_WRITE=1` rewrites the `.out` of every other probe from PHP
//! (review the diff before committing).

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn probe_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/probes");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("probes dir")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "grd"))
        .collect();
    entries.sort();
    assert!(!entries.is_empty());
    entries
}

fn check_probes(gc_threshold: Option<usize>) {
    let entries = probe_files();
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

/// Probe name -> DIALECT section, from `tests/probes_deviations.txt`.
fn deviations() -> Vec<(String, String)> {
    let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/probes_deviations.txt")).expect("probes_deviations.txt");
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut f = l.split('\t');
            let (name, section) = (f.next().unwrap_or(""), f.next().unwrap_or(""));
            assert!(section.starts_with('§') && f.next().is_some_and(|w| !w.is_empty()), "probes_deviations.txt: `{l}` needs name, DIALECT section and reason");
            (name.to_string(), section.to_string())
        })
        .collect()
}

/// PHP's output for a probe: a `<?php` copy under the test's temp dir, run
/// with every error shown, as the `.out` files were captured.
fn php_output(probe: &Path, tmp: &Path) -> String {
    let source = std::fs::read_to_string(probe).expect("read probe");
    let copy = tmp.join(probe.file_name().unwrap()).with_extension("php");
    std::fs::write(&copy, source.replacen("<?grd", "<?php", 1)).expect("write php copy");
    let out = Command::new("php").args(["-d", "display_errors=1", "-d", "error_reporting=E_ALL", "-d", "log_errors=0"]).arg(&copy).output().expect("run php");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Line ends and the end of the file do not count (as in check_probes).
fn normalized(s: &str) -> String {
    s.trim_end().lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
}

#[test]
fn probes_are_php_output() {
    if !Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success()) {
        crate::oracle::missing(crate::oracle::Oracle::Php, "php not available: probes not rechecked against PHP");
        return;
    }
    let write = std::env::var("GRD_PROBES_WRITE").as_deref() == Ok("1");
    let deviations = deviations();
    let entries = probe_files();
    for (name, _) in &deviations {
        assert!(entries.iter().any(|p| p.file_stem().is_some_and(|s| s == name.as_str())), "probes_deviations.txt names `{name}`, which is not a probe");
    }
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("probes-php");
    std::fs::create_dir_all(&tmp).expect("temp dir");
    // PHP starts take most of the time: a few probes per thread
    let outputs: Vec<(PathBuf, String)> = std::thread::scope(|s| {
        let handles: Vec<_> = entries.chunks(entries.len().div_ceil(8)).map(|chunk| s.spawn(|| chunk.iter().map(|p| (p.clone(), php_output(p, &tmp))).collect::<Vec<_>>())).collect();
        handles.into_iter().flat_map(|h| h.join().expect("php thread")).collect()
    });
    let mut failures = Vec::new();
    for (probe, php) in outputs {
        let name = probe.file_stem().unwrap().to_string_lossy().into_owned();
        let out_file = probe.with_extension("out");
        // a new probe gets its .out from PHP in write mode
        let expected = match std::fs::read_to_string(&out_file) {
            Ok(e) => e,
            Err(_) if write => String::new(),
            Err(e) => panic!("{}: {e}", out_file.display()),
        };
        let same = normalized(&php) == normalized(&expected);
        match deviations.iter().find(|(n, _)| *n == name) {
            Some((_, section)) if same => failures.push(format!("{name}: listed as deviating (DIALECT {section}), but PHP prints the .out")),
            Some(_) => {}
            None if same => {}
            None if write => std::fs::write(&out_file, &php).expect("write .out"),
            None => failures.push(format!("{name}: .out is not PHP's output (fix the probe, regenerate with GRD_PROBES_WRITE=1, or list it in probes_deviations.txt)\n  php: {php:?}\n  out: {expected:?}")),
        }
    }
    assert!(failures.is_empty(), "{} probe(s) disagree with PHP:\n{}", failures.len(), failures.join("\n"));
}
