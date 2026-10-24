//! JIT correctness and tiering (JIT1b, JIT2c): every frozen probe of grd-vm
//! runs with the JIT compiling as eagerly as possible (threshold 1) and must
//! print exactly what PHP 8.4 printed, once with tier 1 only and once with
//! tier 2 swapped in before the code runs. A further test shows the
//! asynchronous hot swap.

use std::path::Path;
use std::time::{Duration, Instant};

use grd_jit::CraneliftJit;
use grd_vm::vm::Vm;

fn vm_with(background: bool, sync: bool, threshold: u32) -> Vm {
    let mut vm = Vm::with_prelude().expect("prelude");
    vm.set_jit(Box::new(CraneliftJit::with_tiers(background, sync)));
    vm.set_jit_threshold(threshold, threshold);
    vm
}

fn run_probes(background: bool, sync: bool) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vm/tests/probes");
    let mut probes: Vec<_> = std::fs::read_dir(&dir).expect("probes dir").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "grd")).collect();
    probes.sort();
    let mut failures = Vec::new();
    for probe in &probes {
        let source = std::fs::read(probe).expect("read probe");
        let mut vm = vm_with(background, sync, 1);
        let r = vm.run_file(&probe.display().to_string(), &source);
        let mut out = String::from_utf8_lossy(&vm.output).into_owned();
        if let Err(e) = r {
            out.push_str(&format!("{e}"));
        }
        let expected = std::fs::read_to_string(probe.with_extension("out")).expect("expected output");
        if out.trim_end() != expected.trim_end() {
            failures.push(format!("{}\n  expected: {:?}\n  got:      {:?}", probe.display(), expected, out));
        }
    }
    assert!(failures.is_empty(), "{} of {} probes differ (background {background}, sync {sync}):\n{}", failures.len(), probes.len(), failures.join("\n"));
}

#[test]
fn probes_match_php_with_tier1_only() {
    run_probes(false, false);
}

#[test]
fn probes_match_php_with_tier2() {
    run_probes(true, true);
}

const HOT: &[u8] = b"<?php
function sq(int $n): int { return $n * $n; }
$sum = 0;
for ($r = 0; $r < 200; $r++) {
    for ($i = 0; $i < 5000; $i++) { $sum = ($sum + sq($i)) % 1000003; }
}
echo $sum, \"\\n\";
";

#[test]
fn tier2_is_swapped_in_asynchronously() {
    // tier 1 compiles on the VM thread, tier 2 arrives later from the
    // background thread while the VM keeps running
    let mut vm = vm_with(true, false, 1000);
    vm.run_file("hot.grd", HOT).expect("run");
    let expected = {
        let mut plain = Vm::with_prelude().expect("prelude");
        plain.run_file("hot.grd", HOT).expect("run");
        plain.output.clone()
    };
    assert_eq!(vm.output, expected, "same output as the interpreter");
    let tiers = vm.jit_tiers();
    assert!(!tiers.is_empty(), "the hot loop was compiled");
    let deadline = Instant::now() + Duration::from_secs(30);
    while vm.jit_tiers().iter().any(|t| *t != 2) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(vm.jit_tiers().iter().all(|t| *t == 2), "all code reached tier 2: {:?}", vm.jit_tiers());
    // a second VM with the same program takes the tier-2 code from the
    // process-wide cache without compiling tier 1
    let mut second = vm_with(true, false, 1000);
    second.run_file("hot.grd", HOT).expect("run");
    assert_eq!(second.output, expected);
    assert!(!second.jit_tiers().is_empty() && second.jit_tiers().iter().all(|t| *t == 2), "second VM starts on tier 2: {:?}", second.jit_tiers());
}
