//! Object-model fundamentals that Laravel-style code relies on. Each of these
//! was broken or missing in the inherited interpreter (see KNOWN_ISSUES.md).

use phprs::engine::compile::compile_string_with_functions;
use phprs::engine::vm::{ExecuteData, execute_ex};
use phprs::php::output::{php_output_end, php_output_start};
use std::sync::Arc;

fn run(code: &str) -> String {
    let (op_array, fns) = compile_string_with_functions(code, "oop.grd").expect("compile");
    let _ = php_output_start();
    let mut ed = ExecuteData::new();
    ed.function_table = Some(Arc::new(fns));
    let _ = execute_ex(&mut ed, &op_array);
    let out = php_output_end().unwrap_or_default();
    if let Some(thrown) = ed.pending_exception.take() {
        let (class, msg) = phprs::engine::vm::exception_dispatch::thrown_class_and_message(&thrown);
        panic!("uncaught {class}: {msg}\noutput so far: {out:?}");
    }
    out
}

#[test]
fn exception_subclasses_keep_message_and_code() {
    let out = run(r#"<?php
class V extends Exception {}
class W extends RuntimeException {
    public function __construct(string $m, private string $extra = "e") { parent::__construct("[" . $m . "]", 7); }
    public function extra(): string { return $this->extra; }
}
class X extends W {}
try { throw new V("plain", 3); } catch (V $e) { echo $e->getMessage(), "/", $e->getCode(), "|"; }
try { throw new W("w"); } catch (Exception $e) { echo $e->getMessage(), "/", $e->getCode(), "/", $e->extra(), "|"; }
try { throw new X("x"); } catch (Throwable $e) { echo get_class($e), ":", $e->getMessage(); }
"#);
    assert_eq!(out, "plain/3|[w]/7/e|X:[x]");
}

#[test]
fn methods_are_inherited_and_late_static_binding_works() {
    let out = run(r#"<?php
class A {
    public static function f() { return 1; }
    public function g() { return 5; }
    public function h() { return static::f(); }
    public function s() { return self::f(); }
    public static function make(): static { return new static(); }
}
class B extends A { public static function f() { return 10; } public static function k() { return parent::f(); } }
echo (new B)->g(), (new B)->h(), (new B)->s(), B::k(), get_class(B::make()), get_class(A::make());
"#);
    assert_eq!(out, "51011BA");
}

#[test]
fn self_and_parent_resolve_at_compile_time() {
    let out = run(r#"<?php
class A { const X = "a"; public static function who() { return self::X; } }
class B extends A { const X = "b"; public static function both() { return self::X . parent::X . parent::who(); } }
echo B::both();
"#);
    assert_eq!(out, "baa");
}

#[test]
fn method_return_types_parse() {
    let out = run(r#"<?php
class R {
    public function a(): string { return "s"; }
    public function b(): ?int { return null; }
    public function c(): static { return $this; }
    public function d(): array { return []; }
    public function e(): self|null { return null; }
    public function f(): \Foo\Bar|int { return 1; }
}
$r = new R; echo $r->a(), is_null($r->b()) ? "n" : "?", get_class($r->c()), count($r->d()), $r->f();
"#);
    assert_eq!(out, "snR01");
}

#[test]
fn default_parameter_values() {
    let out = run(r#"<?php
function d(int $x = 3, string $s = "ab", ?array $a = null, bool $b = true, float $f = -1.5) {
    return $x . $s . ($a === null ? "N" : count($a)) . ($b ? "T" : "F") . $f;
}
class K { const D = 4; public function m(int $d = self::D, array $e = [1, 2]) { return $d . count($e); } }
$c = function (int $x = 2, int $y = 3) { return $x * $y; };
$a = fn(int $x = 7) => $x + 1;
echo d(), "|", d(1, "z", [1], false, 2.5), "|", (new K)->m(), (new K)->m(9), "|", $c(), $c(5), $c(5, 5), "|", $a(), $a(0);
"#);
    assert_eq!(out, "3abNT-1.5|1z1F2.5|4292|61525|81");
}

#[test]
fn constructor_runs_without_arguments() {
    let out = run(r#"<?php
class E { public string $v = ""; public function __construct(string $v = "dflt") { $this->v = $v; } }
class Q { public function __call($n, $a) { return "called:" . $n; } }
$x = new E(); $y = new E; $z = new E("given"); $q = new Q();
echo $x->v, "|", $y->v, "|", $z->v, "|", $q->anything();
"#);
    assert_eq!(out, "dflt|dflt|given|called:anything");
}

#[test]
fn closures_capture_by_value_and_bind_this() {
    let out = run(r#"<?php
$k = 10;
$m = function ($v) use ($k) { return $v * $k; };
$k = 99;
$arrow = fn($v) => $v + $k;
function outer() { $q = 2; return function ($v) use ($q) { return $v * $q; }; }
$o = outer();
class T { private int $f = 5; public function go(): int { return array_sum(array_map(fn($x) => $x * $this->f, [1, 2])); } }
$nested = fn($a) => fn($b) => $a + $b;
echo $m(3), "|", $arrow(1), "|", $o(4), "|", (new T)->go(), "|", $nested(1)(2), "|", ($m instanceof Closure ? "C" : "-"), (is_callable($m) ? "c" : "-"), "|", call_user_func($arrow, 1);
"#);
    assert_eq!(out, "30|100|8|15|3|Cc|100");
}

#[test]
fn closures_stored_in_properties_and_arrays() {
    let out = run(r#"<?php
class R {
    public array $h = [];
    public function on(string $n, callable $cb): void { $this->h[$n] = $cb; }
    public function fire(string $n, ...$a) { return ($this->h[$n])(...$a); }
}
$r = new R; $pre = ">";
$r->on("x", function ($v) use ($pre) { return $pre . $v; });
$list = ["k" => fn($v) => strtoupper($v)];
echo $r->fire("x", "hi"), "|", $list["k"]("up"), "|", ($list["k"])("p");
"#);
    assert_eq!(out, ">hi|UP|P");
}

#[test]
fn argument_unpacking_in_all_call_forms() {
    let out = run(r#"<?php
function s(...$a) { return implode(",", $a); }
class M { public function i(...$a) { return count($a); } public static function t($x, ...$a) { return $x . ":" . count($a); } }
$v = [1, 2, 3]; $m = new M;
echo s(...$v), "|", s(0, ...$v), "|", s(...$v, ...$v), "|", $m->i(...$v), "|", M::t("k", ...$v), "|", (new M)->i(9, ...$v);
"#);
    assert_eq!(out, "1,2,3|0,1,2,3|1,2,3,1,2,3|3|k:3|4");
}

#[test]
fn variadic_methods_and_static_methods() {
    let out = run(r#"<?php
class M { public function s(string $n, ...$a) { return $n . count($a); } public static function t(string $n, ...$a) { return $n . count($a); } }
$m = new M; echo $m->s("x"), $m->s("x", 1), $m->s("x", 1, 2), M::t("y"), M::t("y", 1, 2);
"#);
    assert_eq!(out, "x0x1x2y0y2");
}

#[test]
fn instanceof_operator() {
    let out = run(r#"<?php
namespace App\Models;
class A {} class B extends A {}
$b = new B;
$r = [$b instanceof A, $b instanceof B, $b instanceof \App\Models\A, $b instanceof \stdClass, "x" instanceof A,
      new \Exception instanceof \Throwable, (new \RuntimeException) instanceof \Exception, (fn() => 1) instanceof \Closure];
echo implode("", array_map(fn($x) => $x ? "1" : "0", $r));
"#);
    assert_eq!(out, "11100111");
}

#[test]
fn namespaces_qualified_names_and_class_constant() {
    let out = run(r#"<?php
namespace App\Services;
use App\Models\User;
use App\Models\Order as O;
class Local {}
function helper() { return __NAMESPACE__; }
echo User::class, "|", O::class, "|", Local::class, "|", \strlen("abc"), "|", helper(), "|", \App\Services\helper(), "|", Models\Thing::class;
"#);
    assert_eq!(out, "App\\Models\\User|App\\Models\\Order|App\\Services\\Local|3|App\\Services|App\\Services|App\\Services\\Models\\Thing");
}

#[test]
fn static_class_and_class_constants_in_methods() {
    let out = run(r#"<?php
class K { public function n() { return static::class . "|" . self::class . "|" . __CLASS__; } }
class L extends K {}
echo (new L)->n(), "|", (new K)->n();
"#);
    assert_eq!(out, "L|K|K|K|K|K");
}

#[test]
fn expression_statements_may_start_with_parenthesis() {
    let out = run(r#"<?php
class M { public function s(...$a) { echo count($a); } }
(new M)->s(1, 2);
(fn($x) => print($x))("p");
"#);
    assert_eq!(out, "2p");
}
