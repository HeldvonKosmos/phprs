//! The compiler must reject every construct that DIALECT.md §4.2 removes,
//! with a stable `GRDxxx` code, a file name and a line number.

use phprs::engine::compile::compile_string_with_functions;

fn compile_err(code: &str) -> String {
    match compile_string_with_functions(code, "app/Controllers/Demo.grd") {
        Ok(_) => panic!("expected a dialect error, but the snippet compiled:\n{code}"),
        Err(e) => e,
    }
}

fn assert_rejected(code: &str, grd: &str, line: u32) {
    let err = compile_err(code);
    assert!(
        err.starts_with(&format!("error[{grd}]")),
        "expected {grd}, got:\n{err}"
    );
    assert!(
        err.contains(&format!("app/Controllers/Demo.grd:{line}")),
        "expected location line {line}, got:\n{err}"
    );
    assert!(err.contains("Hinweis"), "missing hint:\n{err}");
}

#[test]
fn include_family_is_rejected() {
    assert_rejected("<?php\n\ninclude 'x.php';", "GRD001", 3);
    assert_rejected("<?php\nrequire 'x.php';", "GRD001", 2);
    assert_rejected("<?php\ninclude_once 'x.php';", "GRD001", 2);
    assert_rejected("<?php\nrequire_once 'x.php';", "GRD001", 2);
}

#[test]
fn global_is_rejected() {
    assert_rejected(
        "<?php\nfunction f() {\n    global $x;\n    return $x;\n}",
        "GRD002",
        3,
    );
}

#[test]
fn long_array_syntax_is_rejected() {
    assert_rejected("<?php\n$a = array(1, 2);", "GRD004", 2);
    assert_rejected("<?php\n$a = 1 + array();", "GRD004", 2);
}

#[test]
fn short_array_syntax_still_compiles() {
    compile_string_with_functions("<?php\n$a = [1, 2, 'k' => [3]];", "ok.grd")
        .expect("short array literal must compile");
}

#[test]
fn array_type_hint_still_compiles() {
    compile_string_with_functions(
        "<?php\nfunction f(array $xs): array { return $xs; }",
        "ok.grd",
    )
    .expect("`array` as a type hint is not the array() literal");
}

#[test]
fn host_owned_builtins_are_rejected() {
    for call in [
        "exit(1);",
        "die();",
        "define('X', 1);",
        "ob_start();",
        "set_error_handler(fn() => true);",
        "register_shutdown_function(fn() => 1);",
        "spl_autoload_register(fn($c) => 1);",
        "eval('1;');",
        "extract(['a' => 1]);",
        "phpversion();",
    ] {
        let err = compile_err(&format!("<?php\n{call}"));
        assert!(
            err.starts_with("error[GRD010]"),
            "{call} should be GRD010, got:\n{err}"
        );
    }
}

#[test]
fn ordinary_builtins_still_compile() {
    compile_string_with_functions(
        "<?php\necho strlen('x') . json_encode([1]) . count([]);",
        "ok.grd",
    )
    .expect("regular builtins must stay available");
}
