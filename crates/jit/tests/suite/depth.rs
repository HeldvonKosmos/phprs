//! Deep recursion with the JIT on (JIT_OBJECTS §11b): compiled functions
//! call each other natively on the machine stack, the interpreter keeps its
//! frames on the heap. However deep a program recurses, the process must
//! not overflow the native stack; the output equals the interpreter's.

use grd_jit::CraneliftJit;
use grd_vm::vm::Vm;

const DEEP: &str = "<?php
function down(int $n): int { return $n === 0 ? 0 : 1 + down($n - 1); }
function even(int $n): bool { return $n === 0 ? true : odd($n - 1); }
function odd(int $n): bool { return $n === 0 ? false : even($n - 1); }
function sum(float $x, int $n): float { return $n === 0 ? $x : sum($x + 0.5, $n - 1); }
$t = 0;
for ($r = 0; $r < 3; $r++) { $t += down(1000); }
echo $t, ' ', down(200000), ' ', var_export(even(150001), true), ' ', sum(0.0, 100000), \"\\n\";
";

fn run(src: &str, jit: bool) -> String {
    let src = src.to_string();
    // a thread with the default test stack (2 MB): the bound must hold well
    // below what workers (64 MB) and the grd binary (8 MB) have
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let mut vm = Vm::with_prelude().expect("prelude");
            if jit {
                vm.set_jit(Box::new(CraneliftJit::with_tiers(false, false)));
                vm.set_jit_threshold(1, 1);
            }
            let r = vm.run_file("deep.grd", src.as_bytes());
            let mut out = String::from_utf8_lossy(&vm.output).into_owned();
            if let Err(e) = r {
                out.push_str(&format!("\nuncaught: {e}"));
            }
            (out, vm.jit_tiers().len())
        })
        .expect("thread")
        .join()
        .map(|(out, compiled)| {
            if jit {
                assert!(compiled >= 4, "the recursive functions were compiled ({compiled} units)");
            }
            out
        })
        .expect("no stack overflow")
}

#[test]
fn deep_compiled_recursion_does_not_overflow_the_native_stack() {
    let want = run(DEEP, false);
    assert_eq!(want, "3000 200000 false 50000\n");
    assert_eq!(run(DEEP, true), want);
}
