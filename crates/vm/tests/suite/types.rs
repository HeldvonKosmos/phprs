//! TYP1 (D13): strict typing anchored in the suite.
//!
//! 1. `strict_types_match_php`: a generated matrix of declared types × values ×
//!    positions (parameter, return, property, promoted property, static
//!    property) must give exactly PHP 8.4's answer under `strict_types=1`
//!    (`types/strict.out`, captured from PHP; rechecked when php is there;
//!    `GRD_TYPES_REGEN=1` rewrites it).
//! 2. `typed_operations`: operations on typed values (`types/ops.grd`) against
//!    `types/ops.out`; against PHP only the lines in `ops_deviations.txt`
//!    differ, each a documented dialect rule.
//! 3. `locals_keep_their_type`: GRD021 at run time, expectations computed from
//!    the rule in DIALECT §5.2, not from PHP (PHP has no such rule).

use std::path::{Path, PathBuf};
use std::process::Command;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/types")
}

fn run_grd(src: &str) -> String {
    let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
    let r = vm.run_file("types.grd", src.as_bytes());
    let mut out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = r {
        out.push_str(&format!("\nSCRIPT FAILED: {e}"));
    }
    out
}

fn php_available() -> bool {
    Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success())
}

/// Runs PHP source (with `strict_types=1`), warnings to stderr.
fn run_php(src: &str) -> String {
    let path = std::env::temp_dir().join(format!("grd-types-{}-{}.php", std::process::id(), src.len()));
    std::fs::write(&path, src).unwrap();
    let out = Command::new("php").args(["-d", "display_errors=stderr"]).arg(&path).output().expect("php");
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const TYPES: &[&str] = &[
    "int", "float", "string", "bool", "array", "?int", "int|string", "int|float", "float|string", "mixed", "object", "iterable", "callable", "\\stdClass", "?\\stdClass", "\\Countable", "int|null", "string|array", "false|int", "true",
];

/// (label, expression, is an object: objects may not live in static properties, DIALECT §5.4)
const VALUES: &[(&str, &str, bool)] = &[
    ("int", "1", false),
    ("float", "1.5", false),
    ("ifloat", "2.0", false),
    ("numstr", "'1'", false),
    ("str", "'abc'", false),
    ("true", "true", false),
    ("false", "false", false),
    ("null", "null", false),
    ("arr", "[1]", false),
    ("obj", "new \\stdClass()", true),
    ("closure", "fn () => 1", true),
    ("cnt", "new Cnt()", true),
    ("max", "PHP_INT_MAX", false),
    ("fnname", "'strlen'", false),
];

/// The matrix as PHP (`php = true`, with `declare(strict_types=1)`) or grd source.
fn strict_matrix(php: bool) -> String {
    let mut out = vec![
        if php { "<?php\ndeclare(strict_types=1);".to_string() } else { "<?grd".to_string() },
        "function show(mixed $v): string { return get_debug_type($v); }".into(),
        r#"function attempt(string $label, \Closure $f): void { try { echo $label, " ok ", show($f()), "\n"; } catch (\TypeError $e) { echo $label, " TypeError\n"; } }"#.into(),
        "final class Cnt implements \\Countable, \\IteratorAggregate { public function count(): int { return 1; } public function getIterator(): \\Iterator { return new \\ArrayIterator([]); } }".into(),
    ];
    for (ti, t) in TYPES.iter().enumerate() {
        out.push(format!("function pa{ti}({t} $x): mixed {{ return $x; }}"));
        out.push(format!("function re{ti}(mixed $x): {t} {{ return $x; }}"));
        if *t != "callable" {
            out.push(format!("final class P{ti} {{ public {t} $p; public function __construct() {{}} }}"));
            out.push(format!("final class Q{ti} {{ public function __construct(public {t} $p) {{}} }}"));
            out.push(format!("final class S{ti} {{ public static {t} $p; }}"));
        }
    }
    for (ti, t) in TYPES.iter().enumerate() {
        for (vn, v, is_object) in VALUES {
            out.push(format!("attempt('param {t} <- {vn}', fn () => pa{ti}({v}));"));
            out.push(format!("attempt('return {t} <- {vn}', fn () => re{ti}({v}));"));
            if *t == "callable" {
                continue;
            }
            out.push(format!("attempt('prop {t} <- {vn}', function () {{ $o = new P{ti}(); $o->p = {v}; return $o->p; }});"));
            out.push(format!("attempt('promoted {t} <- {vn}', fn () => (new Q{ti}({v}))->p);"));
            if !is_object {
                out.push(format!("attempt('static {t} <- {vn}', function () {{ S{ti}::$p = {v}; return S{ti}::$p; }});"));
            }
        }
    }
    out.join("\n") + "\n"
}

fn compare(expected: &str, got: &str) -> Vec<String> {
    let e: Vec<&str> = expected.lines().collect();
    let g: Vec<&str> = got.lines().collect();
    let mut diffs: Vec<String> = e.iter().zip(&g).filter(|(a, b)| a != b).map(|(a, b)| format!("expected: {a}\n     got: {b}")).collect();
    if e.len() != g.len() {
        diffs.push(format!("line count: expected {}, got {} (tail: {:?})", e.len(), g.len(), g.last()));
    }
    diffs
}

#[test]
fn strict_types_match_php() {
    // GRD_TYPES_REGEN=1 rewrites strict.out from PHP (after changing the matrix)
    if std::env::var("GRD_TYPES_REGEN").is_ok_and(|v| v == "1") {
        std::fs::write(dir().join("strict.out"), run_php(&strict_matrix(true))).unwrap();
    }
    let expected = std::fs::read_to_string(dir().join("strict.out")).expect("strict.out");
    let got = run_grd(&strict_matrix(false));
    let diffs = compare(&expected, &got);
    assert!(diffs.is_empty(), "{} of {} cases differ from PHP strict_types:\n{}", diffs.len(), expected.lines().count(), diffs.join("\n"));
    if php_available() {
        let php = run_php(&strict_matrix(true));
        assert_eq!(php, expected, "strict.out is stale; regenerate from PHP");
    } else {
        crate::oracle::missing(crate::oracle::Oracle::Php, "php not available: recheck against PHP strict_types skipped");
    }
}

#[test]
fn typed_operations() {
    let src = std::fs::read_to_string(dir().join("ops.grd")).unwrap();
    let expected = std::fs::read_to_string(dir().join("ops.out")).unwrap();
    let got = run_grd(&src);
    let diffs = compare(&expected, &got);
    assert!(diffs.is_empty(), "typed operations:\n{}", diffs.join("\n"));
    if !php_available() {
        crate::oracle::missing(crate::oracle::Oracle::Php, "php not available: recheck against PHP strict_types skipped");
        return;
    }
    let deviations: Vec<String> = std::fs::read_to_string(dir().join("ops_deviations.txt"))
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| l.split(" | ").next().unwrap().trim().to_string())
        .collect();
    let php = run_php(&src.replacen("<?grd", "<?php\ndeclare(strict_types=1);", 1));
    let mut differing: Vec<String> = Vec::new();
    for (a, b) in php.lines().zip(expected.lines()) {
        if a != b {
            let label = deviations.iter().find(|d| b.starts_with(&format!("{d} "))).cloned();
            differing.push(label.unwrap_or_else(|| format!("UNDOCUMENTED: php `{a}` grd `{b}`")));
        }
    }
    assert_eq!(php.lines().count(), expected.lines().count(), "line count differs from PHP");
    assert_eq!(differing, deviations, "lines differing from PHP must be exactly the documented deviations");
}

/// Value kinds GRD021 distinguishes, with a literal for each.
const LOCAL_VALUES: &[(&str, &str)] = &[("null", "null"), ("bool", "true"), ("int", "1"), ("float", "1.5"), ("string", "'a'"), ("array", "[1]"), ("object", "new \\stdClass()")];

/// One assignment: a literal (type known to the compiler) or a value read
/// from an array (type known only at run time).
#[derive(Clone, Copy)]
enum Assign {
    Lit(&'static str),
    Dyn(&'static str),
}

/// DIALECT §5.2 with the correction of 24.10. (DECISIONS 22:20): a local takes
/// the type of its first non-null assignment, `?T` after a first `null`;
/// if that type is not known statically the local is `mixed` and unchecked.
/// Returns, for every assignment after the first, whether it is allowed.
fn allowed(seq: &[Assign]) -> Vec<bool> {
    let (mut ty, mut nullable, mut mixed) = (None::<&str>, false, false);
    let mut out = Vec::new();
    for (i, a) in seq.iter().enumerate() {
        let ok = match (*a, ty) {
            _ if mixed => true,
            (Assign::Lit("null"), None) => {
                nullable = true;
                true
            }
            (Assign::Lit(t), None) => {
                ty = Some(t);
                true
            }
            (Assign::Dyn(_), None) => {
                mixed = true;
                true
            }
            (Assign::Lit(t) | Assign::Dyn(t), Some(cur)) => t == cur || (t == "null" && nullable),
        };
        if i > 0 {
            out.push(ok);
        }
    }
    out
}

#[test]
fn locals_keep_their_type() {
    let names: Vec<&'static str> = LOCAL_VALUES.iter().map(|(n, _)| *n).collect();
    let lit = |n: &str| LOCAL_VALUES.iter().find(|(m, _)| *m == n).unwrap().1;
    let idx = |n: &str| names.iter().position(|m| *m == n).unwrap();
    // later assignments are always run-time values: a mismatching literal would be a compile error
    let mut seqs: Vec<Vec<Assign>> = Vec::new();
    for a in &names {
        for b in &names {
            if *a != "null" {
                seqs.push(vec![Assign::Lit(a), Assign::Dyn(b)]);
            }
            seqs.push(vec![Assign::Lit("null"), Assign::Lit(a), Assign::Dyn(b)]);
            seqs.push(vec![Assign::Dyn(a), Assign::Dyn(b)]);
        }
    }
    let mut src = String::from("<?grd\nfunction vals(): array { return [null, true, 1, 1.5, 'a', [1], new \\stdClass()]; }\n");
    let mut cases: Vec<(String, Vec<bool>)> = Vec::new();
    for (n, seq) in seqs.iter().enumerate() {
        let label = seq.iter().map(|a| match a { Assign::Lit(t) => format!("lit:{t}"), Assign::Dyn(t) => format!("dyn:{t}") }).collect::<Vec<_>>().join("->");
        let mut body = format!("function t{n}(): string {{ $v = vals(); $out = '';");
        for (i, a) in seq.iter().enumerate() {
            let rhs = match a { Assign::Lit(t) => lit(t).to_string(), Assign::Dyn(t) => format!("$v[{}]", idx(t)) };
            if i == 0 || matches!(a, Assign::Lit(_)) {
                body.push_str(&format!(" $x = {rhs};"));
            } else {
                body.push_str(&format!(" try {{ $x = {rhs}; $out .= ' ok'; }} catch (TypeError $e) {{ $out .= str_contains($e->getMessage(), 'GRD021') ? ' GRD021' : ' TypeError'; }}"));
            }
        }
        body.push_str(" return $out; }\n");
        src.push_str(&body);
        src.push_str(&format!("echo '{label}', t{n}(), \"\\n\";\n"));
        // literal assignments after the first produce no output column
        let allow: Vec<bool> = allowed(seq).into_iter().zip(&seq[1..]).filter(|(_, a)| matches!(a, Assign::Dyn(_))).map(|(ok, _)| ok).collect();
        cases.push((label, allow));
    }
    let got = run_grd(&src);
    let mut failures = Vec::new();
    for ((label, allow), line) in cases.iter().zip(got.lines()) {
        let expected = std::iter::once(label.clone()).chain(allow.iter().map(|ok| if *ok { "ok".to_string() } else { "GRD021".to_string() })).collect::<Vec<_>>().join(" ");
        if line != expected {
            failures.push(format!("expected: {expected}\n     got: {line}"));
        }
    }
    assert_eq!(got.lines().count(), cases.len(), "output: {got}");
    assert!(failures.is_empty(), "{} of {} sequences break GRD021:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
