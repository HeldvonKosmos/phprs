//! Cast syntax `(int)`, `(float)`, `(string)`, `(bool)`, `(array)`, `(object)`
//! (KNOWN_ISSUES #1). Before this, `(int) $x` compiled to the string "int".

use phprs::engine::compile::compile_string_with_functions;
use phprs::engine::vm::{ExecuteData, execute_ex};
use phprs::php::output::{php_output_end, php_output_start};
use std::sync::Arc;

fn run(code: &str) -> String {
    let (op_array, fns) = compile_string_with_functions(code, "casts.grd").expect("compile");
    let _ = php_output_start();
    let mut ed = ExecuteData::new();
    ed.function_table = Some(Arc::new(fns));
    let _ = execute_ex(&mut ed, &op_array);
    php_output_end().unwrap_or_default()
}

#[test]
fn int_cast_from_strings_and_floats() {
    let out = run(r#"<?php echo (int) "42", "|", (int)"12abc", "|", (int) 3.9, "|", (int) "abc", "|", (int) "  -7x", "|", (int) "1e3";"#);
    assert_eq!(out, "42|12|3|0|-7|1000");
}

#[test]
fn float_string_bool_casts() {
    let out = run(r#"<?php echo (float) "1.5", "|", (string) 5, "|", (bool) 0 ? "t" : "f", "|", (bool) "a" ? "t" : "f", "|", (double) 2, "|", (integer) "8";"#);
    assert_eq!(out, "1.5|5|f|t|2|8");
}

#[test]
fn cast_binds_tighter_than_arithmetic() {
    let out = run(r#"<?php echo (int) "3" + 4, "|", (int) "3" * 2, "|", -(int) "5", "|", (int) !"";"#);
    assert_eq!(out, "7|6|-5|1");
}

#[test]
fn cast_works_in_call_arguments_and_returns() {
    let out = run(r#"<?php
function f(string $raw): int { return (int) $raw; }
echo strlen((string) 12345), "|", f("7"), "|", max((int) "2", (int) "9");"#);
    assert_eq!(out, "5|7|9");
}

#[test]
fn array_and_object_casts() {
    let out = run(r#"<?php
$a = (array) null; $b = (array) 7; $c = (array) ["x" => 1];
$o = (object) ["a" => 1, "b" => 2];
echo count($a), "|", $b[0], "|", $c["x"], "|", $o->a + $o->b, "|", count((array) $o);"#);
    assert_eq!(out, "0|7|1|3|2");
}

#[test]
fn whitespace_inside_cast_is_allowed() {
    let out = run(r#"<?php echo ( int ) "4", "|", (	string	) 9;"#);
    assert_eq!(out, "4|9");
}

#[test]
fn parenthesised_identifier_is_not_a_cast() {
    // `(strlen)` is not a cast; only the fixed type names are.
    let out = run(r#"<?php $x = "ab"; echo ($x), "|", (strlen($x));"#);
    assert_eq!(out, "ab|2");
}
