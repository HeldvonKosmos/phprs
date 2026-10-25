//! Differential test (D9): random small programs run in the interpreter and
//! with the JIT (synchronous, and advanced with tier 2), output and
//! exceptions must be identical. Programs use what the JIT compiles —
//! functions with int/float signatures, loops over int/float/bool locals —
//! and the edge cases where JIT and interpreter could differ: overflow at
//! PHP_INT_MAX/PHP_INT_MIN, modulo with negative operands, division and
//! modulo by zero, shifts by negative or large amounts, int/float mixing,
//! float formatting.
//!
//! The seed is printed and can be set: `GRD_FUZZ_SEED=<n>` (default fixed),
//! `GRD_FUZZ_CASES=<n>` (default 400; longer runs are optional).

use grd_jit::CraneliftJit;
use grd_vm::vm::Vm;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len() as u64) as usize]
    }
}

const INT_SMALL: &[&str] = &["0", "1", "-1", "2", "-2", "3", "7", "-7", "13", "63", "64", "65", "-64", "100", "1000003"];
const INT_EDGES: &[&str] = &["9223372036854775807", "(-9223372036854775807 - 1)", "4611686018427387904", "-4611686018427387904", "3037000500", "-3037000500"];
const FLOAT_SMALL: &[&str] = &["0.5", "-1.5", "3.25", "1.0e15", "1.0e-7", "0.1", "100.0", "-7.75"];
const FLOAT_EDGES: &[&str] = &["0.0", "-0.0", "2.5e300", "-2.5e300"];

struct Gen<'a> {
    rng: &'a mut Rng,
    ints: Vec<String>,
    floats: Vec<String>,
}

impl Gen<'_> {
    fn int(&mut self, depth: u32) -> String {
        if depth == 0 || self.rng.below(4) == 0 {
            return if !self.ints.is_empty() && self.rng.below(2) == 0 {
                self.ints[self.rng.below(self.ints.len() as u64) as usize].clone()
            } else {
                // edge values (overflow) in one leaf of eight
                if self.rng.below(8) == 0 { self.rng.pick(INT_EDGES) } else { self.rng.pick(INT_SMALL) }.to_string()
            };
        }
        let d = depth - 1;
        match self.rng.below(12) {
            0..=3 => {
                let op = self.rng.pick(&["+", "-", "*", "&", "|", "^"]);
                format!("({} {op} {})", self.int(d), self.int(d))
            }
            4 => format!("({} % {})", self.int(d), self.divisor(d)),
            // shift amounts: mostly 0..69 (≥ 64 included), sometimes negative
            5 => format!("({} << {})", self.int(d), self.shift(d)),
            6 => format!("({} >> {})", self.int(d), self.shift(d)),
            7 => format!("intdiv({}, {})", self.int(d), self.divisor(d)),
            8 => format!("{}({}, {})", self.rng.pick(&["max", "min"]), self.int(d), self.int(d)),
            9 => format!("abs({})", self.int(d)),
            10 => format!("(- {})", self.int(d)),
            _ => format!("({} ? {} : {})", self.bool(d), self.int(d), self.int(d)),
        }
    }

    /// Mostly odd (never zero, sign kept); one in ten unchanged, so that
    /// division and modulo by zero stay covered.
    fn divisor(&mut self, depth: u32) -> String {
        if self.rng.below(10) == 0 { self.int(depth) } else { format!("({} | 1)", self.int(depth)) }
    }

    fn shift(&mut self, depth: u32) -> String {
        if self.rng.below(10) == 0 { format!("({} % 70)", self.int(depth)) } else { format!("(abs({}) % 70)", self.int(depth)) }
    }

    fn float(&mut self, depth: u32) -> String {
        if depth == 0 || self.rng.below(4) == 0 {
            return if !self.floats.is_empty() && self.rng.below(2) == 0 {
                self.floats[self.rng.below(self.floats.len() as u64) as usize].clone()
            } else {
                if self.rng.below(8) == 0 { self.rng.pick(FLOAT_EDGES) } else { self.rng.pick(FLOAT_SMALL) }.to_string()
            };
        }
        let d = depth - 1;
        match self.rng.below(8) {
            0..=2 => {
                let op = self.rng.pick(&["+", "-", "*"]);
                format!("({} {op} {})", self.float(d), self.float(d))
            }
            3 => {
                let den = if self.rng.below(10) == 0 { self.float(d) } else { format!("({} + 0.25)", self.float(d)) };
                format!("({} / {den})", self.float(d))
            }
            4 => format!("((float) {})", self.int(d)),
            5 => format!("({} / {})", self.int(d), self.divisor(d)),
            6 => format!("({} * {})", self.float(d), self.int(d)),
            _ => format!("(- {})", self.float(d)),
        }
    }

    fn bool(&mut self, depth: u32) -> String {
        let d = depth.saturating_sub(1);
        match self.rng.below(5) {
            0 | 1 => {
                let op = self.rng.pick(&["<", "<=", ">", ">=", "===", "!=="]);
                format!("({} {op} {})", self.int(d), self.int(d))
            }
            2 => {
                let op = self.rng.pick(&["<", ">", "<="]);
                format!("({} {op} {})", self.float(d), self.float(d))
            }
            3 => format!("(({} <=> {}) === {})", self.int(d), self.int(d), self.rng.pick(&["-1", "0", "1"])),
            _ => format!("!{}", self.bool(d)),
        }
    }
}

/// One random program.
fn program(rng: &mut Rng) -> String {
    let mut src = String::from("<?php\n");
    let nfuncs = 1 + rng.below(3);
    // JIT2a: g{f} has a default parameter in every other program; compiled code
    // runs for calls with all arguments, the others stay in the interpreter
    let with_default = rng.below(2) == 0;
    for f in 0..nfuncs {
        let mut g = Gen { rng: &mut *rng, ints: vec!["$a".into(), "$b".into()], floats: vec!["$x".into()] };
        let body = if g.rng.below(3) == 0 {
            // loop inside a function
            let e = g.int(2);
            format!("$s = 0; for ($k = 0; $k < 5; $k++) {{ $s = ($s + {e}) % 1000003; }} return $s;")
        } else {
            let c = g.bool(2);
            let t = g.int(3);
            let e = g.int(3);
            format!("if ({c}) {{ return {t}; }} return {e};")
        };
        src.push_str(&format!("function f{f}(int $a, int $b, float $x): int {{ {body} }}\n"));
        let mut g = Gen { rng: &mut *rng, ints: vec!["$a".into()], floats: vec!["$x".into(), "$y".into()] };
        let e = g.float(3);
        let third = if with_default { format!("int $a = {}", g.rng.pick(INT_SMALL)) } else { "int $a".to_string() };
        src.push_str(&format!("function g{f}(float $x, float $y, {third}): float {{ return {e}; }}\n"));
    }
    let nloops = 1 + rng.below(3);
    for l in 0..nloops {
        let mut g = Gen { rng: &mut *rng, ints: vec!["$i".into(), "$p".into(), "$q".into()], floats: vec!["$u".into()] };
        let p0 = g.rng.pick(INT_SMALL);
        let q0 = g.rng.pick(INT_SMALL);
        let u0 = g.rng.pick(FLOAT_SMALL);
        let f = g.rng.below(nfuncs);
        let pe = g.int(3);
        let qe = if g.rng.below(2) == 0 { format!("f{f}($p, $i, $u)") } else { g.int(2) };
        let ue = if g.rng.below(2) == 0 {
            // with a default: also called without the last argument
            if with_default && g.rng.below(2) == 0 { format!("g{f}($u, 0.5)") } else { format!("g{f}($u, 0.5, $q)") }
        } else {
            g.float(2)
        };
        let n = 2 + g.rng.below(30);
        src.push_str(&format!(
            "try {{ $p = {p0}; $q = {q0}; $u = {u0}; $c = 0;\n  for ($i = 0; $i < {n}; $i++) {{ $p = {pe}; $q = {qe}; $u = {ue}; if ($p > $q) {{ $c++; }} }}\n  echo \"L{l} \", $p, ' ', $q, ' ', $u, ' ', $c, \"\\n\";\n}} catch (Throwable $e) {{ echo \"L{l} \", get_class($e), ': ', $e->getMessage(), \"\\n\"; }}\n"
        ));
    }
    src
}

/// Output (with an uncaught error appended) and the number of compiled
/// functions/loops.
fn run(src: &str, jit: Option<bool>) -> (String, usize) {
    let mut vm = Vm::with_prelude().expect("prelude");
    if let Some(advanced) = jit {
        vm.set_jit(Box::new(CraneliftJit::with_tiers(advanced, true)));
        vm.set_jit_threshold(1, 1);
    }
    let r = vm.run_file("fuzz.grd", src.as_bytes());
    let mut out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = r {
        out.push_str(&format!("\nuncaught: {e}"));
    }
    (out, vm.jit_tiers().len())
}

#[test]
fn interpreter_and_jit_agree_on_random_programs() {
    let seed: u64 = std::env::var("GRD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x5EED_2026_1024);
    let cases: u64 = std::env::var("GRD_FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(400);
    eprintln!("differential test: GRD_FUZZ_SEED={seed} GRD_FUZZ_CASES={cases}");
    let mut rng = Rng(seed.max(1));
    let mut exceptions = 0;
    let mut compiled_programs = 0;
    let mut compiled_total = 0;
    let mut kinds = std::collections::BTreeMap::new();
    for case in 0..cases {
        let src = program(&mut rng);
        let (want, _) = run(&src, None);
        if want.contains("Error") {
            exceptions += 1;
        }
        // GRD_FUZZ_DUMP=<text>: print the first program whose output contains it
        if let Ok(pat) = std::env::var("GRD_FUZZ_DUMP")
            && want.contains(&pat)
        {
            eprintln!("--- case {case} ---\n{src}\n--- output ---\n{want}");
            return;
        }
        for line in want.lines().filter(|l| l.contains("Error")) {
            let kind = line.split_whitespace().skip(1).take(4).collect::<Vec<_>>().join(" ");
            *kinds.entry(kind).or_insert(0u32) += 1;
        }
        for advanced in [false, true] {
            let (got, compiled) = run(&src, Some(advanced));
            assert_eq!(got, want, "seed {seed}, case {case}, advanced JIT {advanced}\n--- program ---\n{src}");
            if !advanced {
                compiled_total += compiled;
                compiled_programs += (compiled > 0) as u64;
            }
        }
    }
    eprintln!("differential test: exceptions by kind {kinds:?}");
    eprintln!("differential test: {exceptions} programs with exceptions, {compiled_programs} with compiled code, {compiled_total} compiled functions/loops");
    // the JIT must actually run the programs, and the error paths be reached
    assert!(compiled_programs > cases * 3 / 4, "only {compiled_programs} of {cases} programs had compiled code");
    assert!(exceptions > cases / 20, "only {exceptions} of {cases} programs raised an exception");
}
