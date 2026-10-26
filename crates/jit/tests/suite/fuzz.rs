//! Differential test (D9): random small programs run in the interpreter and
//! with the JIT (synchronous, and advanced with tier 2), output and
//! exceptions must be identical. Programs use what the JIT compiles —
//! functions with int/float signatures, loops over int/float/bool locals —
//! and the edge cases where JIT and interpreter could differ: overflow at
//! PHP_INT_MAX/PHP_INT_MIN, modulo with negative operands, division and
//! modulo by zero, shifts by negative or large amounts, int/float mixing,
//! float formatting.
//!
//! The scalar programs without an `ArithmeticError` (no overflow, where the
//! dialect throws and PHP turns to float, D10) are also run by PHP 8.4 with
//! `strict_types=1`, which must print the same (TEST2.4). The array programs
//! are not: PHP warns and goes on where the dialect throws (DIALECT §5.2/§5.3).
//!
//! The seed is printed and can be set: `GRD_FUZZ_SEED=<n>` (default fixed),
//! `GRD_FUZZ_CASES=<n>` (default 400; longer runs are optional).

use std::process::Command;

use grd_jit::{CraneliftJit, ForceExit};
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
    /// an int / int division was generated (float in the dialect, D10; PHP
    /// gives an int when it divides evenly)
    int_div: bool,
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
            5 => {
                self.int_div = true;
                format!("({} / {})", self.int(d), self.divisor(d))
            }
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

/// One random program, and whether PHP can serve as its oracle (no int / int
/// division).
fn program(rng: &mut Rng) -> (String, bool) {
    let mut int_div = false;
    let mut src = String::from("<?php\n");
    let nfuncs = 1 + rng.below(3);
    // JIT2a: g{f} has a default parameter in every other program; compiled code
    // runs for calls with all arguments, the others stay in the interpreter
    let with_default = rng.below(2) == 0;
    for f in 0..nfuncs {
        let mut g = Gen { rng: &mut *rng, ints: vec!["$a".into(), "$b".into()], floats: vec!["$x".into()], int_div: false };
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
        int_div |= g.int_div;
        src.push_str(&format!("function f{f}(int $a, int $b, float $x): int {{ {body} }}\n"));
        let mut g = Gen { rng: &mut *rng, ints: vec!["$a".into()], floats: vec!["$x".into(), "$y".into()], int_div: false };
        let e = g.float(3);
        let third = if with_default { format!("int $a = {}", g.rng.pick(INT_SMALL)) } else { "int $a".to_string() };
        int_div |= g.int_div;
        src.push_str(&format!("function g{f}(float $x, float $y, {third}): float {{ return {e}; }}\n"));
    }
    let nloops = 1 + rng.below(3);
    for l in 0..nloops {
        let mut g = Gen { rng: &mut *rng, ints: vec!["$i".into(), "$p".into(), "$q".into()], floats: vec!["$u".into()], int_div: false };
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
        int_div |= g.int_div;
        src.push_str(&format!(
            "try {{ $p = {p0}; $q = {q0}; $u = {u0}; $c = 0;\n  for ($i = 0; $i < {n}; $i++) {{ $p = {pe}; $q = {qe}; $u = {ue}; if ($p > $q) {{ $c++; }} }}\n  echo \"L{l} \", $p, ' ', $q, ' ', $u, ' ', $c, \"\\n\";\n}} catch (Throwable $e) {{ echo \"L{l} \", get_class($e), ': ', $e->getMessage(), \"\\n\"; }}\n"
        ));
    }
    (src, !int_div)
}

/// Output (with an uncaught error appended) and the number of compiled
/// functions/loops.
fn run(src: &str, jit: Option<bool>) -> (String, usize) {
    run_with(src, jit, ForceExit::Off, None)
}

/// `run` with forced exits (JITD2 §8c) and a GC threshold (MEM1b).
fn run_with(src: &str, jit: Option<bool>, force: ForceExit, gc: Option<usize>) -> (String, usize) {
    let mut vm = Vm::with_prelude().expect("prelude");
    if let Some(advanced) = jit {
        vm.set_jit(Box::new(CraneliftJit::with_options(advanced, true, force)));
        vm.set_jit_threshold(1, 1);
    }
    if let Some(t) = gc {
        vm.set_gc_threshold(t);
    }
    let r = vm.run_file("fuzz.grd", src.as_bytes());
    let mut out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = r {
        out.push_str(&format!("\nuncaught: {e}"));
    }
    (out, vm.jit_tiers().len())
}

/// JITD2 §8c: the program leaves compiled code at every exit point, and at
/// a seeded half of them, and still prints what the interpreter printed.
fn forced_exits_agree(src: &str, want: &str, seed: u64, case: u64) {
    for force in [ForceExit::All, ForceExit::Seed(seed ^ case)] {
        let (got, _) = run_with(src, Some(false), force, None);
        assert_eq!(got, want, "seed {seed}, case {case}, {force:?}\n--- program ---\n{src}");
    }
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
    let mut for_php = Vec::new();
    for case in 0..cases {
        let (src, php_comparable) = program(&mut rng);
        let (want, _) = run(&src, None);
        // D10: overflow (ArithmeticError) and locals keeping their type
        // (GRD021) are where the dialect differs from PHP on purpose
        if php_comparable && !want.contains("ArithmeticError") && !want.contains("GRD021") && !want.contains("uncaught") {
            for_php.push((case, src.clone(), want.clone()));
        }
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
        forced_exits_agree(&src, &want, seed, case);
    }
    eprintln!("differential test: exceptions by kind {kinds:?}");
    eprintln!("differential test: {exceptions} programs with exceptions, {compiled_programs} with compiled code, {compiled_total} compiled functions/loops");
    // the JIT must actually run the programs, and the error paths be reached
    assert!(compiled_programs > cases * 3 / 4, "only {compiled_programs} of {cases} programs had compiled code");
    assert!(exceptions > cases / 20, "only {exceptions} of {cases} programs raised an exception");
    php_agrees(seed, &for_php, cases);
}

/// PHP's output for the overflow-free programs must be the interpreter's.
fn php_agrees(seed: u64, programs: &[(u64, String, String)], cases: u64) {
    if !Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success()) {
        crate::oracle::missing(crate::oracle::Oracle::Php, "php not available: random programs not compared with PHP");
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("fuzz-php");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let results: Vec<(Vec<String>, usize)> = std::thread::scope(|s| {
        let handles: Vec<_> = programs
            .chunks(programs.len().div_ceil(8))
            .map(|chunk| {
                let dir = &dir;
                s.spawn(move || {
                    let (mut bad, mut lossy) = (Vec::new(), 0usize);
                    for (case, src, want) in chunk {
                        let file = dir.join(format!("case{case}.php"));
                        std::fs::write(&file, src.replacen("<?php\n", "<?php declare(strict_types=1);\n", 1)).expect("write program");
                        let out = Command::new("php").args(["-d", "display_errors=1", "-d", "error_reporting=E_ALL", "-d", "log_errors=0"]).arg(&file).output().expect("run php");
                        let php = String::from_utf8_lossy(&out.stdout);
                        // PHP converts a non-integral or out-of-range float to
                        // int with only a deprecation, the dialect silently
                        // (open question, KNOWN_ISSUES): not compared
                        if php.contains("Deprecated: Implicit conversion from float") {
                            lossy += 1;
                        } else if php.trim_end() != want.trim_end() {
                            bad.push(format!("seed {seed}, case {case}\n--- program ---\n{src}\n--- php ---\n{php}\n--- grd ---\n{want}"));
                        }
                    }
                    (bad, lossy)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("php thread")).collect()
    });
    let lossy: usize = results.iter().map(|r| r.1).sum();
    let mismatches: Vec<String> = results.into_iter().flat_map(|r| r.0).collect();
    eprintln!("differential test: {} programs compared with PHP ({lossy} skipped for a lossy float-to-int conversion), {} differ", programs.len() - lossy, mismatches.len());
    assert!(((programs.len() - lossy) as u64) > cases / 3, "only {} of {cases} programs compared with PHP", programs.len() - lossy);
    assert!(mismatches.is_empty(), "{} programs print differently in PHP:\n{}", mismatches.len(), mismatches.iter().take(3).cloned().collect::<Vec<_>>().join("\n"));
}

/// An array literal: mostly ints, sometimes one element of another kind
/// (float, string, null, nested array) or a hole at the end.
fn int_list(rng: &mut Rng, len: u64) -> String {
    let odd = if rng.below(3) == 0 { Some(rng.below(len.max(1))) } else { None };
    let items: Vec<String> = (0..len)
        .map(|i| {
            if Some(i) == odd {
                rng.pick(&["0.5", "'x'", "null", "[1]", "true", "9223372036854775807"]).to_string()
            } else {
                rng.pick(INT_SMALL).to_string()
            }
        })
        .collect();
    format!("[{}]", items.join(", "))
}

/// JIT2b: programs that read arrays in compiled loops and functions.
fn array_program(rng: &mut Rng) -> String {
    let mut src = String::from("<?php\n");
    let n = 1 + rng.below(12);
    src.push_str(&format!("$A = {};\n", int_list(rng, n)));
    let rows = 1 + rng.below(5);
    let m: Vec<String> = (0..rows).map(|_| { let l = 1 + rng.below(6); int_list(rng, l) }).collect();
    src.push_str(&format!("$M = [{}];\n", m.join(", ")));
    let fl: Vec<&str> = (0..1 + rng.below(8)).map(|_| rng.pick(FLOAT_SMALL)).collect();
    src.push_str(&format!("$F = [{}];\n", fl.join(", ")));
    // functions with array parameters; the bound may run past the end
    let bound = rng.pick(&["count($a)", "count($a)", "$n", "$n + 1"]);
    let mut g = Gen { rng: &mut *rng, ints: vec!["$a[$k]".into(), "$k".into(), "$s".into()], floats: vec![], int_div: false };
    let e = g.int(2);
    src.push_str(&format!("function s(array $a, int $n): int {{ $s = 0; for ($k = 0; $k < {bound}; $k++) {{ $s = ($s + {e}) % 1000003; }} return $s; }}\n"));
    src.push_str("function fs(array $a): float { $s = 0.0; $c = count($a); for ($k = 0; $k < $c; $k++) { $s = $s + $a[$k] * 0.5; } return $s; }\n");
    src.push_str("function cell(array $m, int $r, int $c): int { return $m[$r][$c]; }\n");
    let loops = 1 + rng.below(3);
    for l in 0..loops {
        let iters = 2 + rng.below(25);
        let body = match rng.below(5) {
            0 => format!("$t = ($t + s($A, {})) % 1000003;", rng.below(n + 2)),
            1 => "$u = $u + fs($F);".to_string(),
            2 => format!("$t = ($t + $M[$i % {rows}][$i % {}]) % 1000003;", 1 + rng.below(6)),
            3 => format!("$t = ($t + cell($M, $i % {rows}, {})) % 1000003;", rng.below(3)),
            _ => "$t = ($t + $A[$i % count($A)] * 3 + count($M)) % 1000003;".to_string(),
        };
        src.push_str(&format!(
            "try {{ $t = 0; $u = 0.0;\n  for ($i = 0; $i < {iters}; $i++) {{ {body} }}\n  echo \"L{l} \", $t, ' ', $u, \"\\n\";\n}} catch (Throwable $e) {{ echo \"L{l} \", get_class($e), ': ', $e->getMessage(), \"\\n\"; }}\n"
        ));
    }
    src
}

#[test]
fn interpreter_and_jit_agree_on_array_programs() {
    let seed: u64 = std::env::var("GRD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0xA77A_2026_1025);
    let cases: u64 = std::env::var("GRD_FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    eprintln!("array differential test: GRD_FUZZ_SEED={seed} GRD_FUZZ_CASES={cases}");
    let mut rng = Rng(seed.max(1));
    let (mut exceptions, mut compiled_programs) = (0u64, 0u64);
    for case in 0..cases {
        let src = array_program(&mut rng);
        let (want, _) = run(&src, None);
        exceptions += want.contains("Error") as u64;
        if let Ok(pat) = std::env::var("GRD_FUZZ_DUMP")
            && want.contains(&pat)
        {
            eprintln!("--- case {case} ---\n{src}\n--- output ---\n{want}");
            return;
        }
        for advanced in [false, true] {
            let (got, compiled) = run(&src, Some(advanced));
            assert_eq!(got, want, "seed {seed}, case {case}, advanced JIT {advanced}\n--- program ---\n{src}");
            if !advanced {
                compiled_programs += (compiled > 0) as u64;
            }
        }
        forced_exits_agree(&src, &want, seed, case);
    }
    eprintln!("array differential test: {exceptions} programs with exceptions, {compiled_programs} with compiled code");
    assert!(compiled_programs > cases / 2, "only {compiled_programs} of {cases} programs had compiled code");
    assert!(exceptions > cases / 20, "only {exceptions} of {cases} programs raised an exception");
}

/// JITO1: programs with objects — a base class with typed promoted and
/// plain properties (one without a default, read before and after it is
/// set), a subclass that overrides one method and inherits the others, a
/// final class whose methods build new objects (int arguments widened to
/// float), a readonly property, a constructor that throws, and overflow or
/// division by zero inside methods.
fn object_program(rng: &mut Rng) -> String {
    let mut src = String::from("<?php\n");
    let mut g = Gen { rng: &mut *rng, ints: vec!["$this->n".into()], floats: vec!["$this->x".into(), "$this->y".into()], int_div: false };
    let (sum, ni) = (g.float(2), g.int(2));
    let mut g = Gen { rng: &mut *rng, ints: vec!["$this->n".into(), "$o->n".into()], floats: vec!["$this->x".into(), "$o->x".into(), "$o->y".into()], int_div: false };
    let (mx, my, mn, near) = (g.float(2), g.float(1), g.int(2), g.bool(2));
    src.push_str(&format!(
        "class P {{\n  public float $z;\n  public bool $flag = false;\n  public function __construct(public float $x, public float $y, public int $n) {{}}\n  public function sum(): float {{ return {sum}; }}\n  public function ni(): int {{ return {ni}; }}\n  public function make(P $o): P {{ return new P({mx}, {my}, {mn}); }}\n  public function near(P $o): bool {{ return {near}; }}\n  public function withZ(): float {{ return $this->z + $this->x; }}\n  public function flagged(): bool {{ return $this->flag; }}\n  public function setFlag(bool $f): P {{ $this->flag = $f; return $this; }}\n  public function shift(float $d): P {{ $this->x = $this->x + $d; return $this; }}\n  public function bumpN(int $k): int {{ $this->n = $this->n + 1; return $this->n * $k; }}\n  public function t(): int {{ return (new T($this->n))->v; }}\n}}\n"
    ));
    let over = if rng.below(2) == 0 { "  public function sum(): float { return $this->x * 2.0 + (float) $this->extra; }\n" } else { "" };
    src.push_str(&format!("class Q extends P {{\n  public int $extra = 3;\n{over}  public function more(): int {{ return $this->extra + $this->n; }}\n}}\n"));
    let mut g = Gen { rng: &mut *rng, ints: vec!["$this->a".into(), "$k".into()], floats: vec!["$this->b".into()], int_div: false };
    let (fa, fb) = (g.int(2), g.float(2));
    src.push_str(&format!(
        "final class F {{\n  public function __construct(public int $a, public float $b, public bool $c) {{}}\n  public function bump(int $k): F {{ return new F({fa}, {fb}, !$this->c); }}\n  public function wide(): F {{ return new F($this->a, $this->a, $this->c); }}\n  public function val(): float {{ return $this->b + (float) $this->a; }}\n  private int $seen = 0;\n  public function seen(): int {{ return $this->seen + $this->a; }}\n}}\n"
    ));
    src.push_str("final class RO {\n  public function __construct(public readonly int $v) {}\n  public function next(): RO { return new RO($this->v + 1); }\n}\n");
    let lim = rng.pick(&["5", "50", "500"]);
    src.push_str(&format!("final class T {{\n  public function __construct(public int $v) {{ if ($v > {lim}) {{ throw new RangeException('too big: ' . $v); }} }}\n}}\nfunction mk(int $v): T {{ return new T($v); }}\n"));
    let int0 = |rng: &mut Rng| if rng.below(8) == 0 { rng.pick(INT_EDGES) } else { rng.pick(INT_SMALL) }.to_string();
    let (n0, n1, n2, a0) = (int0(rng), int0(rng), int0(rng), int0(rng));
    let (x0, x1, x2, y0, b0) = (rng.pick(FLOAT_SMALL), rng.pick(FLOAT_SMALL), rng.pick(FLOAT_SMALL), rng.pick(FLOAT_SMALL), rng.pick(FLOAT_SMALL));
    let r0 = rng.pick(&["0", "7", "9223372036854775800"]);
    let zk = rng.below(5);
    let iters = 3 + rng.below(30);
    src.push_str(&format!(
        "$ps = [new P({x0}, {y0}, {n0}), new Q({x1}, 0.25, {n1}), new P({x2}, -2.5, {n2})];\n$f = new F({a0}, {b0}, true);\n$r = new RO({r0});\n$m = $ps[0];\n"
    ));
    src.push_str(&format!(
        "for ($i = 0; $i < {iters}; $i++) {{\n  try {{\n    $p = $ps[$i % 3];\n    if ($i % 5 === {zk}) {{ $p->z = 0.5 * $i; }}\n    $s = $p->sum();\n    $k = $p->ni();\n    $m = $p->make($ps[($i + 1) % 3]);\n    $f = $f->bump($i % 4);\n    if ($i % 3 === 0) {{ $f = $f->wide(); }}\n    $r = $r->next();\n    echo $i, ' ', $s, ' ', $k, ' ', $m->x, ' ', $m->n, ' ', var_export($p->near($m), true), ' ', var_export($p->flagged(), true), ' ', $f->val(), ' ', $r->v, \"\\n\";\n    echo $p->withZ(), \"\\n\";\n    if ($i % 4 === 1) {{ echo $p->setFlag($i % 8 === 1)->shift(0.5)->x, ' ', $p->bumpN(3), ' ', $f->seen(), \"\\n\"; }}\n    if ($i % 6 === 2) {{ echo $p->t(), \"\\n\"; }}\n  }} catch (Throwable $e) {{ echo $i, ' ', get_class($e), ': ', $e->getMessage(), \"\\n\"; }}\n}}\n"
    ));
    let t = rng.pick(&["3", "40", "400"]);
    src.push_str(&format!("try {{ echo $ps[1]->more(), ' ', mk({t})->v, \"\\n\"; }} catch (Throwable $e) {{ echo get_class($e), ': ', $e->getMessage(), \"\\n\"; }}\nvar_dump($m, $f);\necho spl_object_id($r), \"\\n\";\n"));
    src
}

#[test]
fn interpreter_and_jit_agree_on_object_programs() {
    let seed: u64 = std::env::var("GRD_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x0B1E_2026_1026);
    let cases: u64 = std::env::var("GRD_FUZZ_CASES").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    eprintln!("object differential test: GRD_FUZZ_SEED={seed} GRD_FUZZ_CASES={cases}");
    let mut rng = Rng(seed.max(1));
    let (mut exceptions, mut compiled_total) = (0u64, 0usize);
    for case in 0..cases {
        let src = object_program(&mut rng);
        let (want, _) = run(&src, None);
        exceptions += want.contains("Error") as u64;
        if let Ok(pat) = std::env::var("GRD_FUZZ_DUMP")
            && want.contains(&pat)
        {
            eprintln!("--- case {case} ---\n{src}\n--- output ---\n{want}");
            return;
        }
        for advanced in [false, true] {
            let (got, compiled) = run(&src, Some(advanced));
            assert_eq!(got, want, "seed {seed}, case {case}, advanced JIT {advanced}\n--- program ---\n{src}");
            if !advanced {
                compiled_total += compiled;
            }
        }
        // MEM1b: a collection whenever the live objects have doubled
        let (got, _) = run_with(&src, Some(true), ForceExit::Off, Some(1));
        assert_eq!(got, want, "seed {seed}, case {case}, GC threshold 1\n--- program ---\n{src}");
        forced_exits_agree(&src, &want, seed, case);
    }
    eprintln!("object differential test: {exceptions} programs with exceptions, {compiled_total} compiled functions/loops");
    assert!(exceptions > cases / 5, "only {exceptions} of {cases} programs raised an exception");
    // JITO1: the methods must actually run as compiled code
    assert!(compiled_total >= cases as usize * 5, "only {compiled_total} compiled units in {cases} programs");
}
