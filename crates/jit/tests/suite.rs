//! JIT correctness and tiering (JIT1b, JIT2c): every frozen probe of grd-vm
//! runs with the JIT compiling as eagerly as possible (threshold 1) and must
//! print exactly what PHP 8.4 printed, once with tier 1 only, once with
//! tier 2 swapped in before the code runs and once with tier 2 and constant
//! collection; each run must have compiled a minimum of code (TEST2.4). A
//! further test shows the asynchronous hot swap.

#[path = "suite/depth.rs"]
mod depth;
#[path = "suite/fuzz.rs"]
mod fuzz;
#[path = "suite/oracle.rs"]
mod oracle;
#[path = "suite/stress.rs"]
mod stress;

use std::path::Path;
use std::time::{Duration, Instant};

use grd_jit::{CraneliftJit, ForceExit};
use grd_vm::vm::Vm;

fn vm_with(background: bool, sync: bool, threshold: u32) -> Vm {
    vm_forcing(background, sync, threshold, ForceExit::Off)
}

fn vm_forcing(background: bool, sync: bool, threshold: u32, force: ForceExit) -> Vm {
    let mut vm = Vm::with_prelude().expect("prelude");
    vm.set_jit(Box::new(CraneliftJit::with_options(background, sync, force)));
    vm.set_jit_threshold(threshold, threshold);
    vm
}

/// What the JIT did over all probes: probes with compiled code, compiled
/// units, units on tier 2.
struct Compiled {
    probes: usize,
    names: Vec<String>,
    units: usize,
    tier2: usize,
}

fn run_probes(background: bool, sync: bool, gc_threshold: Option<usize>, force: ForceExit) -> Compiled {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../vm/tests/probes");
    let mut probes: Vec<_> = std::fs::read_dir(&dir).expect("probes dir").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "grd")).collect();
    probes.sort();
    let mut failures = Vec::new();
    let mut compiled = Compiled { probes: 0, names: Vec::new(), units: 0, tier2: 0 };
    for probe in &probes {
        let source = std::fs::read(probe).expect("read probe");
        let mut vm = vm_forcing(background, sync, 1, force);
        if let Some(t) = gc_threshold {
            vm.set_gc_threshold(t);
        }
        let r = vm.run_file(&probe.display().to_string(), &source);
        let mut out = String::from_utf8_lossy(&vm.output).into_owned();
        if let Err(e) = r {
            out.push_str(&format!("{e}"));
        }
        let expected = std::fs::read_to_string(probe.with_extension("out")).expect("expected output");
        if out.trim_end() != expected.trim_end() {
            failures.push(format!("{}\n  expected: {:?}\n  got:      {:?}", probe.display(), expected, out));
        }
        let tiers = vm.jit_tiers();
        compiled.probes += !tiers.is_empty() as usize;
        if !tiers.is_empty() {
            compiled.names.push(probe.file_stem().unwrap().to_string_lossy().into_owned());
        }
        compiled.units += tiers.len();
        compiled.tier2 += tiers.iter().filter(|t| **t == 2).count();
    }
    assert!(failures.is_empty(), "{} of {} probes differ (background {background}, sync {sync}):\n{}", failures.len(), probes.len(), failures.join("\n"));
    eprintln!("JIT over the probes (background {background}, sync {sync}, gc {gc_threshold:?}, {force:?}): {} probes with compiled code, {} units, {} on tier 2", compiled.probes, compiled.units, compiled.tier2);
    compiled
}

#[test]
fn probes_match_php_with_tier1_only() {
    let c = run_probes(false, false, None, ForceExit::Off);
    assert_floor(&c);
    assert_eq!(c.tier2, 0, "tier 1 only");
}

#[test]
fn probes_match_php_with_tier2() {
    let c = run_probes(true, true, None, ForceExit::Off);
    assert_floor(&c);
    assert!(c.tier2 >= c.units * 9 / 10, "tier 2 was swapped in for only {} of {} units", c.tier2, c.units);
}

/// The JIT probes with a collection whenever the live objects have doubled
/// (MEM1b): compiled code must keep its values reachable for the GC.
#[test]
fn probes_match_php_with_jit_and_constant_collection() {
    let c = run_probes(true, true, Some(1), ForceExit::Off);
    assert_floor(&c);
}

/// JITD2 §8c: every exit point of the compiled probe code is taken (the
/// interpreter continues or reruns the call) and the output stays PHP's.
#[test]
fn probes_match_php_with_every_exit_forced() {
    let c = run_probes(false, false, None, ForceExit::All);
    assert_floor(&c);
}

/// Floors for the probe runs (JITC1, 26.10.: 3 of 270 probes, 15 units).
/// The JIT takes only int/float functions and loops, so most probes run
/// interpreted; `jit_int_float_kernels` exists to give these runs compiled
/// code. The probes are named so that losing one fails even if another
/// starts compiling.
const COMPILED_PROBES: &[&str] = &["a79_return_widening", "jit_int_float_kernels", "perf2_int_builtins"];
const MIN_UNITS: usize = 15;

fn assert_floor(c: &Compiled) {
    let missing: Vec<_> = COMPILED_PROBES.iter().filter(|p| !c.names.iter().any(|n| n == *p)).collect();
    assert!(missing.is_empty(), "no compiled code in {missing:?} (compiled: {:?})", c.names);
    assert!(c.probes >= COMPILED_PROBES.len() && c.units >= MIN_UNITS, "the JIT compiled too little: {} probes, {} units", c.probes, c.units);
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
    assert!(!second.jit_tiers().is_empty() && second.jit_tiers().iter().all(|t| *t == 2), "second VM starts on tier 2: {:?} (tier-2 admitted, budget, cache: {:?})", second.jit_tiers(), grd_jit::tier2_stats());
}
