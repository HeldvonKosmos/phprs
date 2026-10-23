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
        ("GRD018", "<?php $x = `ls`;"),
    ];
    for (code, src) in cases {
        assert_eq!(code_of(src), Some(*code), "source: {src}");
    }
}

#[test]
fn allowed_constructs_compile() {
    compiles("<?php declare(strict_types=1); echo 1;");
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
