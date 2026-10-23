//! Call / constructor / array arguments must accept full expression precedence
//! (comparison, logical, ternary) — not just additive level.

use phprs::engine::compile::compile_string_with_functions;
use phprs::engine::types::PhpResult;
use phprs::engine::vm::{execute_ex, ExecuteData};
use std::sync::Arc;

fn run_php(code: &str) -> Result<(PhpResult, String), String> {
    let (op_array, ft) = compile_string_with_functions(code, "call_arg_expressions.php")?;
    phprs::php::output::php_output_start().map_err(|e| e.to_string())?;
    let mut ed = ExecuteData::new();
    ed.function_table = Some(Arc::new(ft));
    let result = execute_ex(&mut ed, &op_array);
    let output = phprs::php::output::php_output_end().map_err(|e| e.to_string())?;
    Ok((result, output))
}

#[test]
fn test_comparison_in_function_call_args() {
    let (r, out) = run_php(
        r#"<?php
var_dump(1 < 2);
var_dump(3 > 1);
var_dump(1 <= 1);
var_dump(2 >= 3);
echo (1 == 1) ? "eq\n" : "neq\n";
"#,
    )
    .expect("run");
    assert!(matches!(r, PhpResult::Success), "vm: {r:?}, out: {out:?}");
    assert!(out.contains("bool(true)"), "out: {out:?}");
    assert!(out.contains("bool(false)"), "out: {out:?}");
    assert!(out.contains("eq"), "out: {out:?}");
}

#[test]
fn test_logical_and_ternary_in_call_args() {
    let (r, out) = run_php(
        r#"<?php
function id($x) { return $x; }
echo id(true && false) ? "t" : "f";
echo "|";
echo id(1 < 2 || 0) ? "t" : "f";
echo "|";
echo id(1 < 2 ? "yes" : "no");
"#,
    )
    .expect("run");
    assert!(matches!(r, PhpResult::Success), "vm: {r:?}, out: {out:?}");
    assert_eq!(out.trim(), "f|t|yes");
}

#[test]
fn test_comparison_in_array_and_ctor_args() {
    let (r, out) = run_php(
        r#"<?php
$a = [1 < 2, 3 > 4];
echo $a[0] ? "1" : "0";
echo $a[1] ? "1" : "0";
class Box {
    public $v;
    function __construct($v) { $this->v = $v; }
}
$b = new Box(5 > 3);
echo $b->v ? "1" : "0";
"#,
    )
    .expect("run");
    assert!(matches!(r, PhpResult::Success), "vm: {r:?}, out: {out:?}");
    assert_eq!(out.trim(), "101");
}

#[test]
fn test_spaceship_and_identical_in_call_args() {
    let (r, out) = run_php(
        r#"<?php
var_dump(1 <=> 2);
var_dump(1 === 1);
var_dump(1 !== "1");
"#,
    )
    .expect("run");
    assert!(matches!(r, PhpResult::Success), "vm: {r:?}, out: {out:?}");
    assert!(out.contains("int(-1)"), "out: {out:?}");
    assert!(out.contains("bool(true)"), "out: {out:?}");
}
