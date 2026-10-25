//! PHP's inheritance checks at link time (R1, D12): overriding methods
//! (static, visibility, signature compatibility), abstract methods left,
//! redeclared properties, final constants, #[\Override]. Expected messages
//! are PHP 8.4's; "ok" means PHP accepts the classes.

#[test]
fn inheritance_checks_match_php() {
    let cases: &[(&str, &str)] = &[
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
    let mut failures = Vec::new();
    for (src, expected) in cases {
        let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
        let code = format!("<?grd\n{src}\necho 'ok';");
        let got = match vm.run_file("t.grd", code.as_bytes()) {
            Ok(_) => String::from_utf8_lossy(&vm.output).into_owned(),
            // "PHP Fatal error:  Uncaught Error: <message> on line N"
            Err(e) => {
                let t = e.to_string();
                let t = t.split_once("Uncaught Error: ").map(|(_, m)| m).unwrap_or(&t);
                t.rsplit_once(" on line ").map(|(m, _)| m).unwrap_or(t).to_string()
            }
        };
        if got != *expected {
            failures.push(format!("{src}\n  expected: {expected}\n  got:      {got}"));
        }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
