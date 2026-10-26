//! PHP's inheritance checks at link time (R1, D12): overriding methods
//! (static, visibility, signature compatibility), abstract methods left,
//! redeclared properties, final constants, #[\Override]. Expected messages
//! are PHP 8.4's; "ok" means PHP accepts the classes.

#[test]
fn inheritance_checks_match_php() {
    let cases = INHERITANCE_CHECKS_MATCH_PHP;
    let mut failures = Vec::new();
    for (src, expected) in cases {
        let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
        let code = format!("<?grd\n{src}\necho 'ok';");
        let got = run(&mut vm, &code);
        if got != *expected {
            failures.push(format!("{src}\n  expected: {expected}\n  got:      {got}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

/// The message of a failed run: a link-time Error ("PHP Fatal error:  Uncaught
/// Error: <message> on line N") or a compile error ("error: <message>\n  --> …").
fn run(vm: &mut grd_vm::vm::Vm, code: &str) -> String {
    match vm.run_file("t.grd", code.as_bytes()) {
        Ok(_) => String::from_utf8_lossy(&vm.output).into_owned(),
        Err(e) => {
            let t = e.to_string();
            let t = t.lines().next().unwrap_or("").to_string();
            let t = t.strip_prefix("error: ").unwrap_or(&t).to_string();
            let t = t.split_once("Uncaught Error: ").map(|(_, m)| m.to_string()).unwrap_or(t);
            t.rsplit_once(" on line ").map(|(m, _)| m.to_string()).unwrap_or(t)
        }
    }
}

/// PHP's compile-time checks of declarations: modifiers given twice, members
/// declared twice, untyped readonly properties, enum rules, magic method
/// signatures. Expected messages are PHP 8.4's.
#[test]
fn declaration_checks_match_php() {
    let cases = DECLARATION_CHECKS_MATCH_PHP;
    let mut failures = Vec::new();
    for (src, expected) in cases {
        let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
        let got = run(&mut vm, &format!("<?grd\n{src}\necho 'ok';"));
        if got != *expected {
            failures.push(format!("{src}\n  expected: {expected}\n  got:      {got}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

/// PHP's compile-time checks of types (?T, unions) and of `return` against the
/// declared return type. Expected messages are PHP 8.4's.
#[test]
fn type_and_return_checks_match_php() {
    let cases = TYPE_AND_RETURN_CHECKS_MATCH_PHP;
    let mut failures = Vec::new();
    for (src, expected) in cases {
        let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
        let got = run(&mut vm, &format!("<?grd\n{src}\necho 'ok';"));
        if got != *expected {
            failures.push(format!("{src}\n  expected: {expected}\n  got:      {got}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

/// PHP's checks of where namespaces, class names and self/static may appear,
/// of the kinds of parents, interfaces and traits, and of trait members that
/// clash with the class or another trait. Expected output is PHP 8.4's.
#[test]
fn placement_kind_and_trait_checks_match_php() {
    let cases = PLACEMENT_KIND_AND_TRAIT_CHECKS_MATCH_PHP;
    let mut failures = Vec::new();
    for (src, expected) in cases {
        let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
        let got = run(&mut vm, &format!("<?grd\n{src}\necho 'ok';"));
        if got != *expected {
            failures.push(format!("{src}\n  expected: {expected}\n  got:      {got}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

const INHERITANCE_CHECKS_MATCH_PHP: &[(&str, &str)] = &[
        (r#"class A { public function f(int $a) {} } class B extends A { public function f(string $a) {} }"#, r#"Declaration of B::f(string $a) must be compatible with A::f(int $a)"#),
        (r#"class A { public function f(int $a) {} } class B extends A { public function f(int $a, $b) {} }"#, r#"Declaration of B::f(int $a, $b) must be compatible with A::f(int $a)"#),
        (r#"class A { public function f(int $a, $b = 1) {} } class B extends A { public function f(int $a) {} }"#, r#"Declaration of B::f(int $a) must be compatible with A::f(int $a, $b = 1)"#),
        (r#"class A { public function f(): int { return 1; } } class B extends A { public function f(): string { return ''; } }"#, r#"Declaration of B::f(): string must be compatible with A::f(): int"#),
        (r#"class A { public function f(): int { return 1; } } class B extends A { public function f() { return 1; } }"#, r#"Declaration of B::f() must be compatible with A::f(): int"#),
        (r#"class A { public function f() {} } class B extends A { protected function f() {} }"#, r#"Access level to B::f() must be public (as in class A)"#),
        (r#"class A { protected function f() {} } class B extends A { private function f() {} }"#, r#"Access level to B::f() must be protected (as in class A) or weaker"#),
        (r#"class A { public function f() {} } class B extends A { public static function f() {} }"#, r#"Cannot make non static method A::f() static in class B"#),
        (r#"class A { public static function f() {} } class B extends A { public function f() {} }"#, r#"Cannot make static method A::f() non static in class B"#),
        (r#"interface I { public function f(int $a): ?string; } class C implements I { public function f(int $a): string { return ''; } }"#, r#"ok"#),
        (r#"interface I { public function f(int $a): string; } class C implements I { public function f(int $a): ?string { return ''; } }"#, r#"Declaration of C::f(int $a): ?string must be compatible with I::f(int $a): string"#),
        (r#"interface I { public function f(int $a); } class C implements I {}"#, r#"Class C contains 1 abstract method and must therefore be declared abstract or implement the remaining methods (I::f)"#),
        (r#"abstract class A { abstract public function f(); abstract protected function g(); } class B extends A {}"#, r#"Class B contains 2 abstract methods and must therefore be declared abstract or implement the remaining methods (A::f, A::g)"#),
        (r#"interface I { public function f(); public function g(); public function h(); public function i(); } class C implements I {}"#, r#"Class C contains 4 abstract methods and must therefore be declared abstract or implement the remaining methods (I::f, I::g, I::h, ...)"#),
        (r#"class A { public function __construct(int $a) {} } class B extends A { public function __construct(string $a, $b) {} }"#, r#"ok"#),
        (r#"abstract class A { abstract public function __construct(int $a); } class B extends A { public function __construct(string $a) {} }"#, r#"Declaration of B::__construct(string $a) must be compatible with A::__construct(int $a)"#),
        (r#"class A { public int $x = 0; } class B extends A { public string $x = ''; }"#, r#"Type of B::$x must be int (as in class A)"#),
        (r#"class A { public int $x = 0; } class B extends A { protected int $x = 0; }"#, r#"Access level to B::$x must be public (as in class A)"#),
        (r#"class A { public $x; } class B extends A { public int $x = 0; }"#, r#"Type of B::$x must not be defined (as in class A)"#),
        (r#"class A { public readonly int $x; } class B extends A { public int $x; }"#, r#"Cannot redeclare readonly property A::$x as non-readonly B::$x"#),
        (r#"class A { final public const X = 1; } class B extends A { public const X = 2; }"#, r#"B::X cannot override final constant A::X"#),
        (r#"class A {} class B extends A { #[\Override] public function f() {} }"#, r#"B::f() has #[\Override] attribute, but no matching parent method exists"#),
        (r#"class A { public function f() {} } class B extends A { #[\Override] public function f() {} }"#, r#"ok"#),
        (r#"class A { public function f(A $a): A { return $a; } } class B extends A { public function f(object $a): B { return $this; } }"#, r#"ok"#),
        (r#"class A { public function f(int ...$a) {} } class B extends A { public function f(int $x) {} }"#, r#"Declaration of B::f(int $x) must be compatible with A::f(int ...$a)"#),
        (r#"class A { public function f(int $x) {} } class B extends A { public function f(int ...$x) {} }"#, r#"ok"#),
        (r#"class A { public function f(): static { return $this; } } class B extends A { public function f(): self { return $this; } }"#, r#"Declaration of B::f(): B must be compatible with A::f(): static"#),
        (r#"class A { public function f(): self { return $this; } } class B extends A { public function f(): static { return $this; } }"#, r#"ok"#),
        (r#"class A { public function f(int|string $a): iterable { return []; } } class B extends A { public function f(mixed $a): array { return []; } }"#, r#"ok"#),
        (r#"class A { public function f(): void {} } class B extends A { public function f(): int { return 1; } }"#, r#"Declaration of B::f(): int must be compatible with A::f(): void"#),
        (r#"class A { private function f(int $a) {} } class B extends A { public function f(string $a) {} }"#, r#"ok"#),
        (r#"class A { public function f($a = 'a long default value') {} } class B extends A { public function f() {} }"#, r#"Declaration of B::f() must be compatible with A::f($a = 'a long def...')"#),
];

const DECLARATION_CHECKS_MATCH_PHP: &[(&str, &str)] = &[
        (r#"class A { public public $x; }"#, r#"Multiple access type modifiers are not allowed"#),
        (r#"class A { final final public function f() {} }"#, r#"Multiple final modifiers are not allowed"#),
        (r#"class A { public static static function f() {} }"#, r#"Multiple static modifiers are not allowed"#),
        (r#"class A { public readonly readonly int $x; }"#, r#"Multiple readonly modifiers are not allowed"#),
        (r#"abstract class A { abstract abstract function f(); }"#, r#"Multiple abstract modifiers are not allowed"#),
        (r#"class A { public readonly $x; }"#, r#"Readonly property A::$x must have type"#),
        (r#"readonly class A { public $x; }"#, r#"Readonly property A::$x must have type"#),
        (r#"class A { public function f() {} public function F() {} }"#, r#"Cannot redeclare A::F()"#),
        (r#"class A { public $x; public $x; }"#, r#"Cannot redeclare A::$x"#),
        (r#"class A { const X = 1; const X = 2; }"#, r#"Cannot redefine class constant A::X"#),
        (r#"enum E { public $x; case A; }"#, r#"Enum E cannot include properties"#),
        (r#"enum E { case A = 1; }"#, r#"Case A of non-backed enum E must not have a value"#),
        (r#"enum E: int { case A; }"#, r#"Case A of backed enum E must have a value"#),
        (r#"final abstract class A {}"#, r#"Cannot use the final modifier on an abstract class"#),
        (r#"class A { public function __get(int $n) {} }"#, r#"A::__get(): Parameter #1 ($n) must be of type string when declared"#),
        (r#"class A { public function __get($a, $b) {} }"#, r#"Method A::__get() must take exactly 1 argument"#),
        (r#"class A { public static function __get($n) {} }"#, r#"Method A::__get() cannot be static"#),
        (r#"class A { public function __set(string $n, $v): int { return 1; } }"#, r#"A::__set(): Return type must be void when declared"#),
        (r#"class A { public function __isset(string $n): string { return ''; } }"#, r#"A::__isset(): Return type must be bool when declared"#),
        (r#"class A { public function __call(string $n, int $a) {} }"#, r#"A::__call(): Parameter #2 ($a) must be of type array when declared"#),
        (r#"class A { public function __callStatic($n, $a) {} }"#, r#"Method A::__callStatic() must be static"#),
        (r#"class A { public function __toString(): int { return 1; } }"#, r#"A::__toString(): Return type must be string when declared"#),
        (r#"class A { public function __construct(): void {} }"#, r#"Method A::__construct() cannot declare a return type"#),
        (r#"class A { public function __destruct($a) {} }"#, r#"Method A::__destruct() cannot take arguments"#),
        (r#"class A { public function __clone(): int { return 1; } }"#, r#"A::__clone(): Return type must be void when declared"#),
        (r#"class A { public static function __construct() {} }"#, r#"Method A::__construct() cannot be static"#),
        (r#"class A { public function __debugInfo(): array { return []; } public function __toString(): string { return ''; } public function __get(string $n): mixed { return 1; } public function __set(string $n, mixed $v): void {} public function __isset(string $n): bool { return true; } public function __unset(string $n): void {} public function __call(string $n, array $a): mixed { return 1; } public static function __callStatic(string $n, array $a): mixed { return 1; } public function __serialize(): array { return []; } public function __unserialize(array $d): void {} public function __invoke(int ...$x) {} public function __clone(): void {} }"#, r#"ok"#),
        (r#"class A { public function __get($n): never { throw new Exception(); } public function __set(mixed $n, $v) {} }"#, r#"ok"#),
        (r#"enum E: string { case A = 'a'; const X = self::A; }"#, r#"ok"#),
        (r#"class A { public function __set_state(array $a) {} }"#, r#"Method A::__set_state() must be static"#),
];

const TYPE_AND_RETURN_CHECKS_MATCH_PHP: &[(&str, &str)] = &[
        (r#"function f(?mixed $x) {}"#, r#"Type mixed cannot be marked as nullable since mixed already includes null"#),
        (r#"function f(mixed|int $x) {}"#, r#"Type mixed can only be used as a standalone type"#),
        (r#"function f(?void $x) {}"#, r#"Void can only be used as a standalone type"#),
        (r#"function f(void|int $x) {}"#, r#"Void can only be used as a standalone type"#),
        (r#"function f(never|int $x) {}"#, r#"never can only be used as a standalone type"#),
        (r#"function f(int|int $x) {}"#, r#"Duplicate type int is redundant"#),
        (r#"function f(int|INT $x) {}"#, r#"Duplicate type int is redundant"#),
        (r#"function f(A|a $x) {}"#, r#"Duplicate type a is redundant"#),
        (r#"function f(bool|false $x) {}"#, r#"Duplicate type false is redundant"#),
        (r#"function f(false|bool $x) {}"#, r#"Duplicate type false is redundant"#),
        (r#"function f(true|false $x) {}"#, r#"Type contains both true and false, bool must be used instead"#),
        (r#"function f(iterable|array $x) {}"#, r#"Duplicate type array is redundant"#),
        (r#"function f(iterable|Traversable $x) {}"#, r#"Duplicate type Traversable is redundant"#),
        (r#"function f(object|A $x) {}"#, r#"Type A|object contains both object and a class type, which is redundant"#),
        (r#"function f(?null $x) {}"#, r#"null cannot be marked as nullable"#),
        (r#"function f(null $x) {}"#, r#"ok"#),
        (r#"function f(int|null $x) {}"#, r#"ok"#),
        (r#"function f(string|array|string $x) {}"#, r#"Duplicate type string is redundant"#),
        (r#"function f(callable|Closure $x) {}"#, r#"ok"#),
        (r#"function f(A|B|null $x) {}"#, r#"ok"#),
        (r#"function f(): void { return 1; }"#, r#"A void function must not return a value"#),
        (r#"function f(): void { return null; }"#, r#"A void function must not return a value (did you mean "return;" instead of "return null;"?)"#),
        (r#"function f(): void { return; }"#, r#"ok"#),
        (r#"function f(): never { return; }"#, r#"A never-returning function must not return"#),
        (r#"function f(): int { return; }"#, r#"A function with return type must return a value"#),
        (r#"function f(): ?int { return; }"#, r#"A function with return type must return a value (did you mean "return null;" instead of "return;"?)"#),
        (r#"function f(): int { yield 1; return; }"#, r#"Generator return type must be a supertype of Generator, int given"#),
        (r#"function f(): iterable { yield 1; return; }"#, r#"ok"#),
        (r#"function f() { return; }"#, r#"ok"#),
        (r#"$c = fn (): void => 1;"#, r#"A void function must not return a value"#),
        (r#"class A { function __construct() { return 1; } }"#, r#"ok"#),
        (r#"function f(): mixed { return; }"#, r#"A function with return type must return a value (did you mean "return null;" instead of "return;"?)"#),
        (r#"function f(): Generator { yield 1; return 5; }"#, r#"ok"#),
];

const PLACEMENT_KIND_AND_TRAIT_CHECKS_MATCH_PHP: &[(&str, &str)] = &[
        (r#"final final class A {}"#, r#"Multiple final modifiers are not allowed"#),
        (r#"abstract abstract class A {}"#, r#"Multiple abstract modifiers are not allowed"#),
        (r#"readonly readonly class A {}"#, r#"Multiple readonly modifiers are not allowed"#),
        (r#"switch (1) { default: break; default: break; }"#, r#"Switch statements may only contain one default clause"#),
        (r#"function __construct(public $p) {}"#, r#"Cannot declare promoted property outside a constructor"#),
        (r#"abstract class A { abstract public function __construct(public $p); }"#, r#"Cannot declare promoted property in an abstract constructor"#),
        (r#"interface I { public function __construct(public $p); }"#, r#"Cannot declare promoted property in an abstract constructor"#),
        (r#"class A { public function f(public $p) {} }"#, r#"Cannot declare promoted property outside a constructor"#),
        (r#"class A { public function __construct(public $p, public $p) {} }"#, r#"Redefinition of parameter $p"#),
        (r#"function f($a, $a) {}"#, r#"Redefinition of parameter $a"#),
        (r#"class A { public $p; public function __construct(public $p) {} }"#, r#"Cannot redeclare A::$p"#),
        (r#"class A { public function __construct(public ...$p) {} }"#, r#"Cannot declare variadic promoted property"#),
        (r#"class A { public function __construct(public callable $p) {} }"#, r#"Property A::$p cannot have type callable"#),
        (r#"class A { public function __construct(public int $p = 1, private readonly string $q = '') {} } echo (new A())->p;"#, r#"1ok"#),
        (r#"echo 1; namespace A;"#, r#"Namespace declaration statement has to be the very first statement or after any declare call in the script"#),
        (r#"declare(strict_types=1); namespace A;"#, r#"ok"#),
        (r#"namespace A; echo 1; namespace B;"#, r#"1ok"#),
        (r#"class int {}"#, r#"Cannot use "int" as a class name as it is reserved"#),
        (r#"class Mixed {}"#, r#"Cannot use "Mixed" as a class name as it is reserved"#),
        (r#"interface iterable {}"#, r#"Cannot use "iterable" as an interface name as it is reserved"#),
        (r#"trait never {}"#, r#"Cannot use "never" as a trait name as it is reserved"#),
        (r#"enum void {}"#, r#"Cannot use "void" as an enum name as it is reserved"#),
        (r#"class abc {} class A { use abc; }"#, r#"A cannot use abc - it is not a trait"#),
        (r#"interface I {} class A { use I; }"#, r#"A cannot use I - it is not a trait"#),
        (r#"trait T { const C = 1; } class A { use T; const C = 2; }"#, r#"A and T define the same constant (C) in the composition of A. However, the definition differs and is considered incompatible. Class was composed"#),
        (r#"trait T { const C = 1; } class A { use T; const C = 1; }"#, r#"ok"#),
        (r#"trait T { public $x = 1; } class A { use T; public $x = 2; }"#, r#"A and T define the same property ($x) in the composition of A. However, the definition differs and is considered incompatible. Class was composed"#),
        (r#"trait T { public $x = 1; } class A { use T; public $x = 1; }"#, r#"ok"#),
        (r#"trait T { public int $x = 1; } class A { use T; public string $x = '1'; }"#, r#"A and T define the same property ($x) in the composition of A. However, the definition differs and is considered incompatible. Class was composed"#),
        (r#"trait T1 { const C = 1; } trait T2 { const C = 2; } class A { use T1, T2; }"#, r#"T1 and T2 define the same constant (C) in the composition of A. However, the definition differs and is considered incompatible. Class was composed"#),
        (r#"trait T1 { const C = 1; } trait T2 { const C = 1; } class A { use T1, T2; }"#, r#"ok"#),
        (r#"function f(): self {}"#, r#"Cannot use "self" when no class scope is active"#),
        (r#"function f(): static {}"#, r#"Cannot use "static" when no class scope is active"#),
        (r#"function f(self $x) {}"#, r#"Cannot use "self" when no class scope is active"#),
        (r#"function f(parent $x) {}"#, r#"Cannot use "parent" when no class scope is active"#),
        (r#"$c = function (): static { return $this; };"#, r#"ok"#),
        (r#"$c = fn (self $x) => 1;"#, r#"ok"#),
        (r#"class A implements Iterator, IteratorAggregate {}"#, r#"Class A cannot implement both Iterator and IteratorAggregate at the same time"#),
        (r#"interface I {} class A extends I {}"#, r#"Class A cannot extend interface I"#),
        (r#"class A {} class B implements A {}"#, r#"B cannot implement A - it is not an interface"#),
        (r#"trait T {} class A extends T {}"#, r#"Class A cannot extend trait T"#),
];

/// The expectations above are PHP 8.4's output: run every case through `php`
/// (a `<?php` file each) and compare the message, so a wrong literal shows up
/// (TEST2.6).
#[test]
fn expectations_are_php_output() {
    use std::process::Command;
    if !Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success()) {
        crate::oracle::missing(crate::oracle::Oracle::Php, "php not available: inheritance messages not rechecked against PHP");
        return;
    }
    let all: Vec<(&str, &str)> = [INHERITANCE_CHECKS_MATCH_PHP, DECLARATION_CHECKS_MATCH_PHP, TYPE_AND_RETURN_CHECKS_MATCH_PHP, PLACEMENT_KIND_AND_TRAIT_CHECKS_MATCH_PHP].iter().flat_map(|c| c.iter().copied()).collect();
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("inheritance-php");
    std::fs::create_dir_all(&dir).expect("temp dir");
    let failures: Vec<String> = std::thread::scope(|s| {
        let handles: Vec<_> = all
            .chunks(all.len().div_ceil(8))
            .enumerate()
            .map(|(t, chunk)| {
                let dir = &dir;
                s.spawn(move || {
                    let mut bad = Vec::new();
                    for (i, (src, expected)) in chunk.iter().enumerate() {
                        let file = dir.join(format!("c{t}_{i}.php"));
                        std::fs::write(&file, format!("<?php\n{src}\necho 'ok';")).expect("write case");
                        let out = Command::new("php").args(["-d", "display_errors=1", "-d", "log_errors=0", "-d", "html_errors=0"]).arg(&file).output().expect("run php");
                        let got = php_message(&String::from_utf8_lossy(&out.stdout));
                        if got != *expected {
                            bad.push(format!("{src}\n  literal: {expected}\n  php:     {got}"));
                        }
                    }
                    bad
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("php thread")).collect()
    });
    assert!(failures.is_empty(), "{} of {} literals are not PHP's message:\n{}", failures.len(), all.len(), failures.join("\n"));
}

/// PHP's message from its output: "ok", or the text of "Fatal error: <msg> in
/// <file> on line N" (an uncaught Error without its class prefix).
fn php_message(out: &str) -> String {
    let out = out.trim();
    let Some((_, rest)) = out.split_once("Fatal error: ") else { return out.to_string() };
    let rest = rest.strip_prefix("Uncaught Error: ").unwrap_or(rest);
    let rest = rest.split(" in /").next().unwrap_or(rest);
    rest.trim().to_string()
}
