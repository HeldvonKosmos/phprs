//! Native functions and constants. Registered into the VM's function table;
//! no `match` over names at call time.

mod array;
mod core;
pub(crate) mod datetime;
pub mod datetime_class;
pub mod json;
mod math;
mod misc;
mod output;
mod reflect;
mod regex;
mod string;

use crate::value::Val;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.define_constant("PHP_EOL", Val::str("\n"));
    vm.define_constant("PHP_INT_MAX", Val::Int(i64::MAX));
    vm.define_constant("PHP_INT_MIN", Val::Int(i64::MIN));
    vm.define_constant("PHP_INT_SIZE", Val::Int(8));
    vm.define_constant("PHP_FLOAT_EPSILON", Val::Float(f64::EPSILON));
    vm.define_constant("PHP_FLOAT_MAX", Val::Float(f64::MAX));
    vm.define_constant("PHP_FLOAT_MIN", Val::Float(f64::MIN_POSITIVE));
    vm.define_constant("PHP_VERSION", Val::str("8.4.0-grd"));
    vm.define_constant("PHP_MAJOR_VERSION", Val::Int(8));
    vm.define_constant("PHP_MINOR_VERSION", Val::Int(4));
    vm.define_constant("PHP_OS", Val::str("Linux"));
    vm.define_constant("PHP_OS_FAMILY", Val::str("Linux"));
    vm.define_constant("NAN", Val::Float(f64::NAN));
    vm.define_constant("INF", Val::Float(f64::INFINITY));
    vm.define_constant("M_PI", Val::Float(std::f64::consts::PI));
    vm.define_constant("M_E", Val::Float(std::f64::consts::E));
    vm.define_constant("E_ALL", Val::Int(32767));
    vm.define_constant("E_ERROR", Val::Int(1));
    vm.define_constant("E_WARNING", Val::Int(2));
    vm.define_constant("E_NOTICE", Val::Int(8));
    vm.define_constant("E_STRICT", Val::Int(2048));
    vm.define_constant("E_DEPRECATED", Val::Int(8192));
    vm.define_constant("E_USER_ERROR", Val::Int(256));
    vm.define_constant("E_USER_WARNING", Val::Int(512));
    vm.define_constant("E_USER_NOTICE", Val::Int(1024));
    vm.define_constant("E_USER_DEPRECATED", Val::Int(16384));
    vm.define_constant("SORT_REGULAR", Val::Int(0));
    vm.define_constant("SORT_NUMERIC", Val::Int(1));
    vm.define_constant("SORT_STRING", Val::Int(2));
    vm.define_constant("SORT_FLAG_CASE", Val::Int(8));
    vm.define_constant("SORT_NATURAL", Val::Int(6));
    vm.define_constant("COUNT_NORMAL", Val::Int(0));
    vm.define_constant("COUNT_RECURSIVE", Val::Int(1));
    vm.define_constant("ARRAY_FILTER_USE_KEY", Val::Int(2));
    vm.define_constant("ARRAY_FILTER_USE_BOTH", Val::Int(1));
    vm.define_constant("STR_PAD_RIGHT", Val::Int(1));
    vm.define_constant("STR_PAD_LEFT", Val::Int(0));
    vm.define_constant("STR_PAD_BOTH", Val::Int(2));
    vm.define_constant("JSON_HEX_TAG", Val::Int(1));
    vm.define_constant("JSON_HEX_AMP", Val::Int(2));
    vm.define_constant("JSON_HEX_APOS", Val::Int(4));
    vm.define_constant("JSON_HEX_QUOT", Val::Int(8));
    vm.define_constant("JSON_FORCE_OBJECT", Val::Int(16));
    vm.define_constant("JSON_NUMERIC_CHECK", Val::Int(32));
    vm.define_constant("JSON_UNESCAPED_SLASHES", Val::Int(64));
    vm.define_constant("JSON_PRETTY_PRINT", Val::Int(128));
    vm.define_constant("JSON_UNESCAPED_UNICODE", Val::Int(256));
    vm.define_constant("JSON_PARTIAL_OUTPUT_ON_ERROR", Val::Int(512));
    vm.define_constant("JSON_PRESERVE_ZERO_FRACTION", Val::Int(1024));
    vm.define_constant("JSON_OBJECT_AS_ARRAY", Val::Int(1));
    vm.define_constant("JSON_BIGINT_AS_STRING", Val::Int(2));
    vm.define_constant("JSON_INVALID_UTF8_IGNORE", Val::Int(1048576));
    vm.define_constant("JSON_INVALID_UTF8_SUBSTITUTE", Val::Int(2097152));
    vm.define_constant("JSON_THROW_ON_ERROR", Val::Int(4194304));
    vm.define_constant("JSON_ERROR_NONE", Val::Int(0));
    vm.define_constant("JSON_ERROR_DEPTH", Val::Int(1));
    vm.define_constant("JSON_ERROR_SYNTAX", Val::Int(4));
    vm.define_constant("JSON_ERROR_UTF8", Val::Int(5));
    vm.define_constant("PREG_SPLIT_NO_EMPTY", Val::Int(1));
    vm.define_constant("PREG_SPLIT_DELIM_CAPTURE", Val::Int(2));
    vm.define_constant("PREG_PATTERN_ORDER", Val::Int(1));
    vm.define_constant("PREG_SET_ORDER", Val::Int(2));
    vm.define_constant("ENT_QUOTES", Val::Int(3));
    vm.define_constant("ENT_HTML5", Val::Int(48));
    vm.define_constant("ENT_HTML401", Val::Int(0));
    vm.define_constant("ENT_NOQUOTES", Val::Int(0));
    vm.define_constant("ENT_COMPAT", Val::Int(2));
    vm.define_constant("ENT_SUBSTITUTE", Val::Int(8));
    vm.define_constant("PASSWORD_DEFAULT", Val::str("2y"));
    vm.define_constant("PASSWORD_BCRYPT", Val::str("2y"));
    vm.define_constant("FILTER_VALIDATE_EMAIL", Val::Int(274));
    vm.define_constant("FILTER_VALIDATE_INT", Val::Int(257));
    vm.define_constant("FILTER_VALIDATE_FLOAT", Val::Int(259));
    vm.define_constant("FILTER_VALIDATE_BOOL", Val::Int(258));
    vm.define_constant("FILTER_VALIDATE_BOOLEAN", Val::Int(258));
    vm.define_constant("FILTER_VALIDATE_URL", Val::Int(273));
    vm.define_constant("FILTER_VALIDATE_IP", Val::Int(275));
    vm.define_constant("FILTER_DEFAULT", Val::Int(516));
    vm.define_constant("FILTER_NULL_ON_FAILURE", Val::Int(134217728));

    core::register(vm);
    string::register(vm);
    array::register(vm);
    math::register(vm);
    output::register(vm);
    json::register(vm);
    regex::register(vm);
    datetime::register(vm);
    misc::register(vm);
    reflect::register(vm);
}

/// Argument helpers shared by the builtin modules.
pub(crate) mod args {
    use crate::error::Throw;
    use crate::value::{PhpArray, PhpString, Val};
    use std::rc::Rc;

    pub fn count(args: &[Val], min: usize, name: &str) -> Result<(), Throw> {
        if args.len() < min {
            return Err(Throw::argument_count(format!("{name}() expects at least {min} argument{}, {} given", if min == 1 { "" } else { "s" }, args.len())));
        }
        Ok(())
    }
    pub fn string(args: &[Val], i: usize, name: &str) -> Result<PhpString, Throw> {
        match args.get(i) {
            Some(Val::Str(s)) => Ok(s.clone()),
            Some(Val::Int(n)) => Ok(PhpString::from(n.to_string())),
            Some(Val::Float(f)) => Ok(PhpString::from(crate::value::format_float(*f))),
            Some(Val::Bool(true)) => Ok(PhpString::from("1")),
            Some(Val::Bool(false)) | Some(Val::Null) => Ok(PhpString::new()),
            Some(other) => Err(Throw::type_error(format!("{name}(): Argument #{} must be of type string, {} given", i + 1, other.type_name()))),
            None => Err(Throw::argument_count(format!("{name}(): Argument #{} is required", i + 1))),
        }
    }
    pub fn int(args: &[Val], i: usize, name: &str) -> Result<i64, Throw> {
        match args.get(i) {
            Some(Val::Int(n)) => Ok(*n),
            Some(Val::Float(f)) if f.fract() == 0.0 => Ok(*f as i64),
            Some(Val::Bool(b)) => Ok(*b as i64),
            Some(Val::Null) => Ok(0),
            Some(other) => Err(Throw::type_error(format!("{name}(): Argument #{} must be of type int, {} given", i + 1, other.type_name()))),
            None => Err(Throw::argument_count(format!("{name}(): Argument #{} is required", i + 1))),
        }
    }
    pub fn int_opt(args: &[Val], i: usize, name: &str, default: i64) -> Result<i64, Throw> {
        match args.get(i) {
            None | Some(Val::Null) => Ok(default),
            _ => int(args, i, name),
        }
    }
    pub fn float(args: &[Val], i: usize, name: &str) -> Result<f64, Throw> {
        match args.get(i) {
            Some(Val::Int(n)) => Ok(*n as f64),
            Some(Val::Float(f)) => Ok(*f),
            Some(Val::Bool(b)) => Ok(*b as i64 as f64),
            Some(Val::Null) => Ok(0.0),
            Some(other) => Err(Throw::type_error(format!("{name}(): Argument #{} must be of type float, {} given", i + 1, other.type_name()))),
            None => Err(Throw::argument_count(format!("{name}(): Argument #{} is required", i + 1))),
        }
    }
    pub fn array(args: &[Val], i: usize, name: &str) -> Result<Rc<PhpArray>, Throw> {
        match args.get(i) {
            Some(Val::Array(a)) => Ok(a.clone()),
            Some(other) => Err(Throw::type_error(format!("{name}(): Argument #{} must be of type array, {} given", i + 1, other.type_name()))),
            None => Err(Throw::argument_count(format!("{name}(): Argument #{} is required", i + 1))),
        }
    }
    pub fn boolean(args: &[Val], i: usize, default: bool) -> bool {
        args.get(i).map(Val::to_bool).unwrap_or(default)
    }
}
