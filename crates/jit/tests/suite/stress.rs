//! JIT5: code lifetime under load. Several worker threads keep rebuilding
//! their VM (as on a worker restart or hot reload) and run program variants
//! whose hot functions and loops get tier 1 at once and tier 2 from the
//! background compiler while they run; the output must equal the
//! interpreter's every time, and the process-wide tier-2 budget holds.

use grd_jit::CraneliftJit;
use grd_vm::vm::Vm;

fn variant(k: usize) -> Vec<u8> {
    format!(
        "<?php
function f{k}(int $n, int $m = {k}): int {{ $s = 0; for ($i = 0; $i < $n; $i++) {{ $s = ($s + $i * $m) % 1000003; }} return $s; }}
function g{k}(float $x): float {{ return $x * 0.5 + {k}.25; }}
$acc = 0; $u = 1.0;
for ($r = 0; $r < 300; $r++) {{ $acc = ($acc + f{k}($r % 97)) % 1000003; $u = g{k}($u); }}
echo $acc, ' ', $u, \"\\n\";
"
    )
    .into_bytes()
}

fn interpret(src: &[u8]) -> Vec<u8> {
    let mut vm = Vm::with_prelude().expect("prelude");
    vm.run_file("variant.grd", src).expect("run");
    vm.output
}

#[test]
fn workers_rebuild_and_swap_tiers_under_load() {
    let variants: Vec<Vec<u8>> = (0..6).map(variant).collect();
    let expected: Vec<Vec<u8>> = variants.iter().map(|v| interpret(v)).collect();
    let workers: Vec<_> = (0..4)
        .map(|t| {
            let (variants, expected) = (variants.clone(), expected.clone());
            std::thread::spawn(move || {
                for round in 0..6 {
                    let k = (t + round) % variants.len();
                    // a fresh VM each round: tier-1 code goes with the old one
                    let mut vm = Vm::with_prelude().expect("prelude");
                    vm.set_jit(Box::new(CraneliftJit::with_tiers(true, false)));
                    vm.set_jit_threshold(20, 20);
                    vm.run_file("variant.grd", &variants[k]).expect("run");
                    assert_eq!(String::from_utf8_lossy(&vm.output), String::from_utf8_lossy(&expected[k]), "worker {t}, round {round}, variant {k}");
                    assert!(!vm.jit_tiers().is_empty(), "variant {k} was compiled");
                }
            })
        })
        .collect();
    for w in workers {
        w.join().expect("worker");
    }
    let (admitted, limit, cache) = grd_jit::tier2_stats();
    assert!(admitted <= limit, "tier-2 budget exceeded: {admitted} > {limit}");
    assert!(cache <= admitted, "cache entries {cache} without a compilation ({admitted})");
    // tier 2 really happened (the test binary shares the process with
    // other JIT tests, so only a lower bound is exact here)
    assert!(admitted > 0 && cache > 0, "no tier-2 compilation under load: admitted {admitted}, cache {cache}");
}

const BUDGET: usize = 4;

/// TEST2.4: a small tier-2 budget in a process of its own. The six variants
/// have more hot units than the budget, so exactly `BUDGET` compilations are
/// admitted, the rest keeps tier 1, and every output is still right.
#[test]
fn tier2_budget_is_used_up_exactly() {
    let out = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "stress::tier2_budget_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env("GRD_JIT_TIER2_LIMIT", BUDGET.to_string())
        .env("GRD_TIER2_BUDGET_CHILD", "1")
        .output()
        .expect("run child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "child failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("budget child: ok"), "child did not run its checks:\n{stdout}");
}

#[test]
#[ignore = "child process of tier2_budget_is_used_up_exactly"]
fn tier2_budget_child() {
    if std::env::var("GRD_TIER2_BUDGET_CHILD").is_err() {
        return;
    }
    let (mut tier1, mut tier2) = (0, 0);
    for round in 0..2 {
        for k in 0..6 {
            let src = variant(k);
            let mut vm = Vm::with_prelude().expect("prelude");
            // synchronous: an admitted unit is on tier 2 before it runs
            vm.set_jit(Box::new(CraneliftJit::with_tiers(true, true)));
            vm.set_jit_threshold(20, 20);
            vm.run_file("variant.grd", &src).expect("run");
            assert_eq!(vm.output, interpret(&src), "round {round}, variant {k}");
            tier1 += vm.jit_tiers().iter().filter(|t| **t == 1).count();
            tier2 += vm.jit_tiers().iter().filter(|t| **t == 2).count();
        }
    }
    let (admitted, limit, cache) = grd_jit::tier2_stats();
    assert_eq!(limit, BUDGET, "GRD_JIT_TIER2_LIMIT read");
    assert_eq!(admitted, BUDGET, "the budget is used up exactly");
    assert!(cache <= admitted, "cache {cache} > admitted {admitted}");
    assert!(tier1 > 0, "past the budget, hot code keeps tier 1");
    // the second round finds the admitted units in the cache again
    assert!(tier2 >= 2 * cache && cache > 0, "tier-2 code shared from the cache: {tier2} units on tier 2, {cache} cached");
    println!("budget child: ok (admitted {admitted}, tier 1 {tier1}, tier 2 {tier2}, cache {cache})");
}

