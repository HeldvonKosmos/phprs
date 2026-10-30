//! Every construct the dialect removes is a compile error with a stable GRD code.

use grd_vm::compile::compile_source;

fn code_of(src: &str) -> Option<&'static str> {
    match compile_source("t.grd", src.as_bytes()) {
        Ok(_) => None,
        Err(e) => e.code,
    }
}

fn compiles(src: &str) {
    if let Err(e) = compile_source("t.grd", src.as_bytes()) {
        panic!("expected to compile: {src}\n{e}");
    }
}

#[test]
fn rejected_constructs_carry_codes() {
    let cases: &[(&str, &str)] = &[
        ("GRD001", "<?php include 'x.php';"),
        ("GRD001", "<?php require_once 'x.php';"),
        ("GRD002", "<?php function f() { global $x; }"),
        ("GRD003", "<?php $x = 'a'; echo $$x;"),
        ("GRD004", "<?php $a = array(1, 2);"),
        ("GRD005", "<?php list($a, $b) = [1, 2];"),
        ("GRD006", "<?php function f(array &$a) {}"),
        ("GRD006", "<?php $a = 1; $b = &$a;"),
        ("GRD006", "<?php foreach ([1] as &$v) {}"),
        ("GRD006", "<?php $f = function() use (&$x) {};"),
        ("GRD007", "<?php echo <<<TXT\nhi\nTXT;"),
        ("GRD008", "<?php $f = static fn($x) => $x;"),
        ("GRD008", "<?php $f = static function() {};"),
        ("GRD009", "<?php declare(ticks=1);"),
        ("GRD010", "<?php compact('a');"),
        ("GRD010", "<?php define('X', 1);"),
        ("GRD010", "<?php ob_start();"),
        ("GRD011", "<?php goto a; a: echo 1;"),
        ("GRD012", "<?php echo $_GET['x'];"),
        ("GRD012", "<?php echo $GLOBALS['x'];"),
        ("GRD013", "<?php function f() { static $n = 0; }"),
        ("GRD014", "<?php eval('1;');"),
        ("GRD015", "<?php exit(1);"),
        ("GRD015", "<?php die();"),
        ("GRD016", "<?php echo 1; __halt_compiler(); raw data"),
        ("GRD017", "<p>hello</p><?php echo 1;"),
        ("GRD017", "<?php echo 1; ?>\n<p>tail</p>"),
        ("GRD018", "<?php $x = `ls`;"),
        ("GRD019", "<?grd $x = array_pop($a);"),
        ("GRD019", "<?grd while ($x = array_shift($q)) {}"),
        ("GRD020", "<?grd sort($a);"),
        ("GRD020", "<?grd preg_match('/x/', $s, $m);"),
        ("GRD020", "<?grd (usort($a, fn($x, $y) => $x <=> $y));"),
    ];
    for (code, src) in cases {
        assert_eq!(code_of(src), Some(*code), "source: {src}");
    }
}

/// GRD021 (D10): a local keeps the type of its first assignment.
#[test]
fn local_variables_keep_their_type() {
    let rejected: &[&str] = &[
        "<?grd $x = 1; $x = 'a';",
        "<?grd $x = 0; $x = 1.5;",
        "<?grd $x = 0.0; $x = 1;",
        "<?grd $n = 10; $n /= 4;",
        "<?grd $i = 1; $i .= 'x';",
        "<?grd $x = 5; $x = null;",
        "<?grd $x = null; $x = 1; $x = 'a';",
        "<?grd function f(int $a) { $a = 'x'; }",
        "<?grd $f = function () { $y = []; $y = 1; };",
    ];
    for src in rejected {
        assert_eq!(code_of(src), Some("GRD021"), "source: {src}");
    }
    let err = compile_source("t.grd", b"<?grd $x = 0; $x = 1.5;").unwrap_err();
    assert!(err.hint.unwrap_or("").contains("0.0"), "hint names 0.0 / (float): {err}");
    // allowed: ?T after a first null, same types, foreach rebinding, separate scopes,
    // untyped parameters, values whose type is not evident (checked at run time)
    compiles("<?grd $x = null; $x = 1; $x = null; $x = 2;");
    compiles("<?grd $t = 0.0; $t += 1; $t = $t * 2; $i = 0; $i += 1; $s = ''; $s .= 1;");
    compiles("<?grd foreach ([1, 'a', null] as $v) { echo $v; } foreach (['a' => 1, 2 => 'b'] as $k => $v) {}");
    compiles("<?grd $x = 1; function f() { $x = 'a'; } $g = fn() => 1;");
    compiles("<?grd function f(?int $a = null, float $b = 1) { $a = 3; $a = null; $b = 2.5; }");
    compiles("<?grd $r = 10 / 4; $r = 1.0; $q = intdiv(10, 4); $q = 3;");
}

/// GRD021 at run time, where the compiler cannot see the type.
#[test]
fn local_variables_keep_their_type_at_run_time() {
    let run = |src: &str| -> String {
        let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
        let r = vm.run_file("t.grd", src.as_bytes());
        let mut out = String::from_utf8_lossy(&vm.output).into_owned();
        if let Err(e) = r {
            out.push_str(&format!("{e}"));
        }
        out
    };
    // a typed local (first value evident) takes only its type, also from values the compiler cannot see
    let out = run("<?grd $a = [1, 'x']; $v = 0; $v = $a[0]; try { $v = $a[1]; } catch (TypeError $e) { echo $e->getMessage(); }");
    assert!(out.contains("GRD021: the local variable $v is int, but string is assigned"), "{out}");
    // a local whose first value has no evident type is `mixed` (D10, 22:20)
    let out = run("<?grd $a = [5, null, 'x', [1]]; $v = $a[0]; $v = $a[1]; $v = $a[2]; $v = $a[3]; echo 'ok';");
    assert_eq!(out, "ok");
    let out = run("<?grd function f(string|array $keys) { $keys = (array) $keys; return count($keys); } echo f('a');");
    assert_eq!(out, "1", "union-typed parameters are mixed");
    let out = run("<?grd $x = 2; $y = [0.5]; try { $x += $y[0]; } catch (TypeError $e) { echo 'caught'; }");
    assert_eq!(out, "caught", "compound assignment that changes the type");
    let out = run("<?grd echo PHP_INT_MAX + 0; try { echo PHP_INT_MAX + 1; } catch (ArithmeticError $e) { echo ' ', $e->getMessage(); }");
    assert_eq!(out, "9223372036854775807 Integer overflow in +");
    assert_eq!(run("<?grd var_dump(10 / 2, intdiv(10, 2));"), "float(5)\nint(5)\n");
}

/// DIA2 (D20, DIALECT §5.2): `%` and bit operations take only ints, `++`
/// and `--` only ints and floats, and `abs(PHP_INT_MIN)` overflows like every
/// other int operation. PHP truncates the floats, increments null and strings
/// and gives a float for the absolute value.
#[test]
fn integer_only_operators_and_numeric_increments() {
    let src = r#"<?grd
function attempt(callable $f): string { try { return var_export($f(), true); } catch (Throwable $e) { return get_class($e) . ': ' . $e->getMessage(); } }
function inc(mixed $v): mixed { $v++; return $v; }
function dec(mixed $v): mixed { $v--; return $v; }
echo attempt(fn () => 7.5 % 2), "\n";
echo attempt(fn () => 7 % 2.0), "\n";
echo attempt(fn () => 6 & 3.0), "\n";
echo attempt(fn () => 1.0 << 2), "\n";
echo attempt(fn () => ~1.5), "\n";
echo attempt(fn () => 'a' % 2), "\n";
echo attempt(function () { $a = ['n' => 1.5]; $a['n'] %= 2; return $a; }), "\n";
echo attempt(fn () => 7 % 3), ' ', attempt(fn () => -7 % 3), ' ', attempt(fn () => 6 ^ 3), ' ', attempt(fn () => ~5), ' ', attempt(fn () => fmod(7.5, 2)), "\n";
echo attempt(fn () => abs(PHP_INT_MIN)), ' ', attempt(fn () => abs(PHP_INT_MIN + 1)), ' ', attempt(fn () => abs(-2.5)), "\n";
echo attempt(fn () => inc(null)), "\n";
echo attempt(fn () => dec(null)), "\n";
echo attempt(fn () => inc('a')), "\n";
echo attempt(fn () => dec('5')), "\n";
echo attempt(fn () => inc(true)), "\n";
echo attempt(fn () => inc(1.5)), ' ', attempt(fn () => dec(3)), "\n";
"#;
    let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
    let r = vm.run_file("t.grd", src.as_bytes());
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(
        String::from_utf8_lossy(&vm.output),
        "TypeError: Unsupported operand types: float % int; use fmod() or (int)
TypeError: Unsupported operand types: int % float; use fmod() or (int)
TypeError: Unsupported operand types: int & float; use (int)
TypeError: Unsupported operand types: float << int; use (int)
TypeError: Cannot perform bitwise not on float; use (int)
TypeError: Unsupported operand types: string % int; use (int)/(float)
TypeError: Unsupported operand types: float % int; use fmod() or (int)
1 -1 5 -6 1.5
ArithmeticError: Integer overflow in abs() 9223372036854775807 2.5
TypeError: Cannot increment null
TypeError: Cannot decrement null
TypeError: Cannot increment string
TypeError: Cannot decrement string
TypeError: Cannot increment bool
2.5 2
"
    );
}

#[test]
fn allowed_constructs_compile() {
    compiles("<?grd declare(strict_types=1); echo 1;");
    compiles("<?grd $a = sort($a); [$v, $rest] = Arr::pop($a); $m = preg_match('/x/', $s); if (preg_match('/x/', $s)) {}");
    compiles("<?php echo 1;");
    compiles("<?php $f = fn($x) => $x + 1; $g = function($y) use ($f) { return $f($y); };");
    compiles("<?php [$a, [$b, $c]] = [1, [2, 3]]; ['k' => $k] = ['k' => 1]; [, $s] = [1, 2];");
    compiles("<?php namespace App; use Foo\\Bar as Baz; class A extends Baz implements \\Countable { public function count(): int { return 1; } }");
    compiles("<?php enum S: string { case A = 'a'; public function l(): string { return match($this) { self::A => 'x' }; } }");
    compiles("<?php try { f(); } catch (A | B $e) { } finally { echo 1; }");
    compiles("<?php $o?->a?->b(); $x = $y['a']['b'] ?? 'd'; $z ??= 3;");
    compiles("<?php $s = strlen(...); $m = $o->m(...); $c = C::m(...);");
    compiles("<?php function g() { $x = yield 1; yield from [2]; return 3; }");
    compiles("<?php #[Route('GET', '/t')] class C { #[Attr] public function i(#[Sensitive] int $p = 1): void {} }");
    compiles("<?php class P { public function __construct(private readonly int $a, protected string $b = 'x') {} }");
    compiles("<?php $o->items[$k]->v = 1; $this_is_fine = 1; $a['x']['y'][] = 2; $o->n++; ++$a['k'];");
}

/// Endless recursion through Rust (callbacks, magic methods, `__toString`)
/// throws Error at the nesting limit instead of overflowing the native stack
/// and aborting the process; the script goes on after catching it.
#[test]
fn native_recursion_throws_instead_of_aborting() {
    let src = r#"<?grd
function f(int $n): int { return array_map(fn (int $x): int => f($x + 1), [$n])[0]; }
try { f(0); } catch (Error $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
final class Loop { public function __toString(): string { return 'x' . $this; } }
try { echo (string) new Loop(); } catch (Error $e) { echo 'toString: ', $e->getMessage(), "\n"; }
echo array_sum(array_map(fn (int $x): int => $x * 2, [1, 2, 3])), "\n";
"#;
    // a thread with a small stack, so the limit must hold well below 8 MB
    let out = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
            let r = vm.run_file("t.grd", src.as_bytes());
            let mut out = String::from_utf8_lossy(&vm.output).into_owned();
            if let Err(e) = r {
                out.push_str(&format!("{e}"));
            }
            out
        })
        .unwrap()
        .join()
        .expect("no stack overflow");
    assert_eq!(out, "Error: Maximum call stack size reached. Infinite recursion?\ntoString: Maximum call stack size reached. Infinite recursion?\n12\n");
}

/// var_export of an object that contains itself: PHP warns and exports NULL;
/// in the dialect the warning is an Error (DIALECT §5).
#[test]
fn var_export_of_a_cycle_is_an_error() {
    let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
    let src = "<?grd final class N { public ?N $n = null; } $a = new N(); $a->n = $a; try { var_export($a); } catch (Error $e) { echo $e->getMessage(); }";
    vm.run_file("t.grd", src.as_bytes()).expect("runs");
    assert!(String::from_utf8_lossy(&vm.output).ends_with("var_export does not handle circular references"));
}

/// DIALECT §4.2: every builtin of the compile-time list is rejected with
/// GRD010 (`eval`, `exit`, `die` are constructs with their own codes), the
/// list stays sorted (it is searched binary), and the builtins of §4.2 that
/// are simply absent (file system, network, sessions, …) do not exist at run
/// time (TEST2.7).
#[test]
fn forbidden_builtins_are_rejected_or_absent() {
    let list = grd_vm::compile::dialect::FORBIDDEN_FUNCTIONS;
    assert!(list.windows(2).all(|w| w[0] < w[1]), "FORBIDDEN_FUNCTIONS must stay sorted");
    for name in list {
        let want = match *name {
            "eval" => "GRD014",
            "exit" | "die" => "GRD015",
            _ => "GRD010",
        };
        let src = format!("<?grd {name}('x');");
        assert_eq!(code_of(&src), Some(want), "{src}");
    }
    for name in ["fopen", "file_put_contents", "file_get_contents", "fgetcsv", "unlink", "curl_init", "fsockopen", "session_start", "mysqli_connect", "openssl_encrypt", "gzcompress", "exec", "shell_exec", "system", "passthru", "proc_open", "putenv", "header", "setcookie"] {
        let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
        let src = format!("<?grd {name}('x');");
        let err = vm.run_file("t.grd", src.as_bytes()).expect_err(name).to_string();
        assert!(err.contains(&format!("Call to undefined function {name}()")), "{name}: {err}");
    }
}
