//! Conversions with PHP semantics, restricted by DIALECT §5.2:
//! explicit casts follow PHP; implicit numeric conversion of strings does
//! not exist (the VM raises `TypeError` instead).

use super::{PhpString, Val};
use crate::error::Throw;

/// Result of parsing a numeric string.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Numeric {
    Int(i64),
    Float(f64),
}

impl Numeric {
    pub fn as_f64(self) -> f64 {
        match self {
            Numeric::Int(i) => i as f64,
            Numeric::Float(f) => f,
        }
    }
    pub fn to_val(self) -> Val {
        match self {
            Numeric::Int(i) => Val::Int(i),
            Numeric::Float(f) => Val::Float(f),
        }
    }
}

fn is_ws(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Parse the numeric prefix of `b` as PHP does. Returns the number and whether
/// the whole string (modulo surrounding whitespace) was numeric.
/// `"12abc"` → `Some((Int(12), false))`, `" 1e3 "` → `Some((Float(1000.0), true))`,
/// `"abc"` → `None`.
pub fn parse_numeric(b: &[u8]) -> Option<(Numeric, bool)> {
    let mut i = 0;
    while i < b.len() && is_ws(b[i]) {
        i += 1;
    }
    let start = i;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut is_float = false;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        let mut j = i + 1;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        frac_digits = j - (i + 1);
        if int_digits > 0 || frac_digits > 0 {
            is_float = true;
            i = j;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            is_float = true;
            i = j;
        }
    }
    let end = i;
    while i < b.len() && is_ws(b[i]) {
        i += 1;
    }
    let whole = i == b.len();
    let text = std::str::from_utf8(&b[start..end]).ok()?;
    if !is_float {
        if let Ok(v) = text.parse::<i64>() {
            return Some((Numeric::Int(v), whole));
        }
        // integer overflow: PHP falls back to float
    }
    let f: f64 = text.parse().ok()?;
    Some((Numeric::Float(f), whole))
}

/// Format a float the way `echo` does with `precision = 14`.
pub fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "NAN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "INF".to_string() } else { "-INF".to_string() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".to_string() } else { "0".to_string() };
    }
    let sci = format!("{:.13e}", f); // "-1.2345000000000e3"
    let (mant, exp) = sci.split_once('e').expect("exponent");
    let exp: i32 = exp.parse().expect("exponent");
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if exp < -4 || exp >= 14 {
        out.push_str(&digits[..1]);
        out.push('.');
        if digits.len() > 1 {
            out.push_str(&digits[1..]);
        } else {
            out.push('0');
        }
        out.push('E');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&exp.abs().to_string());
    } else if exp >= 0 {
        let int_len = exp as usize + 1;
        if digits.len() <= int_len {
            out.push_str(digits);
            for _ in digits.len()..int_len {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..int_len]);
            out.push('.');
            out.push_str(&digits[int_len..]);
        }
    } else {
        out.push_str("0.");
        for _ in 0..(-exp - 1) {
            out.push('0');
        }
        out.push_str(digits);
    }
    out
}

/// A float as PHP prints it with `serialize_precision = -1` (var_dump,
/// var_export, json_encode, serialize): shortest round-trip digits like
/// zend_gcvt with 17 digits, integral values below 1e15 without fraction,
/// exponential form when the decimal exponent is below -4 or above 16;
/// `exp` is the exponent letter (`E`, json uses `e`).
pub fn repr_float(f: f64, exp_char: u8) -> String {
    if f.is_nan() {
        return "NAN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "INF".into() } else { "-INF".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    if f == f.trunc() && f.abs() < 1e15 {
        return format!("{}", f as i64);
    }
    let e = format!("{f:e}");
    let (m, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let ev: i32 = exp.parse().unwrap_or(0);
    if !(-4..=16).contains(&ev) {
        let m = if m.contains('.') { m.to_string() } else { format!("{m}.0") };
        return format!("{m}{}{}{}", exp_char as char, if ev < 0 { "-" } else { "+" }, ev.abs());
    }
    format!("{f}")
}

impl Val {
    /// PHP truthiness.
    pub fn to_bool(&self) -> bool {
        match self {
            Val::Null => false,
            Val::Bool(b) => *b,
            Val::Int(i) => *i != 0,
            Val::Float(f) => *f != 0.0,
            Val::Str(s) => !(s.is_empty() || s.as_bytes() == b"0"),
            Val::Array(a) => !a.is_empty(),
            Val::Object(_) => true,
        }
    }

    /// `(int)` cast. Objects are a `TypeError` in the dialect.
    pub fn to_int(&self) -> Result<i64, Throw> {
        Ok(match self {
            Val::Null => 0,
            Val::Bool(b) => *b as i64,
            Val::Int(i) => *i,
            Val::Float(f) => float_to_int(*f),
            Val::Str(s) => match parse_numeric(s.as_bytes()) {
                Some((Numeric::Int(i), _)) => i,
                Some((Numeric::Float(f), _)) => float_to_int(f),
                None => 0,
            },
            Val::Array(a) => !a.is_empty() as i64,
            Val::Object(_) => return Err(Throw::type_error("Cannot convert object to int")),
        })
    }

    /// `(float)` cast.
    pub fn to_float(&self) -> Result<f64, Throw> {
        Ok(match self {
            Val::Null => 0.0,
            Val::Bool(b) => *b as i64 as f64,
            Val::Int(i) => *i as f64,
            Val::Float(f) => *f,
            Val::Str(s) => match parse_numeric(s.as_bytes()) {
                // PHP reads the string as a float: "-0" keeps its sign
                Some((Numeric::Int(0), _)) if s.as_bytes().iter().find(|b| !is_ws(**b)) == Some(&b'-') => -0.0,
                Some((n, _)) => n.as_f64(),
                None => 0.0,
            },
            Val::Array(a) => (!a.is_empty()) as i64 as f64,
            Val::Object(_) => return Err(Throw::type_error("Cannot convert object to float")),
        })
    }

    /// String conversion for `echo`, concatenation and interpolation.
    /// Arrays are a `TypeError`; objects are handled by the VM (`__toString`).
    pub fn to_php_string(&self) -> Result<PhpString, Throw> {
        Ok(match self {
            Val::Null => PhpString::new(),
            Val::Bool(true) => PhpString::from("1"),
            Val::Bool(false) => PhpString::new(),
            Val::Int(i) => PhpString::from(i.to_string()),
            Val::Float(f) => PhpString::from(format_float(*f)),
            Val::Str(s) => s.clone(),
            Val::Array(_) => return Err(Throw::type_error("Array to string conversion")),
            Val::Object(_) => return Err(Throw::type_error("Object could not be converted to string")),
        })
    }

    /// Numeric operand for arithmetic (DIALECT §5.2): only int and float.
    pub fn as_number(&self, op: &str) -> Result<Numeric, Throw> {
        match self {
            Val::Int(i) => Ok(Numeric::Int(*i)),
            Val::Float(f) => Ok(Numeric::Float(*f)),
            Val::Str(s) => Err(Throw::type_error(format!("Unsupported operand type string (\"{s}\") for {op}; use (int)/(float)"))),
            other => Err(Throw::type_error(format!("Unsupported operand type {} for {}", other.type_name(), op))),
        }
    }
}

fn float_to_int(f: f64) -> i64 {
    if f.is_finite() { f as i64 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_strings() {
        assert_eq!(parse_numeric(b"12"), Some((Numeric::Int(12), true)));
        assert_eq!(parse_numeric(b" 12 "), Some((Numeric::Int(12), true)));
        assert_eq!(parse_numeric(b"12abc"), Some((Numeric::Int(12), false)));
        assert_eq!(parse_numeric(b"-3.5x"), Some((Numeric::Float(-3.5), false)));
        assert_eq!(parse_numeric(b"1e3"), Some((Numeric::Float(1000.0), true)));
        assert_eq!(parse_numeric(b".5"), Some((Numeric::Float(0.5), true)));
        assert_eq!(parse_numeric(b"abc"), None);
        assert_eq!(parse_numeric(b""), None);
        assert_eq!(parse_numeric(b"."), None);
        assert_eq!(parse_numeric(b"0x1A"), Some((Numeric::Int(0), false)));
    }

    #[test]
    fn float_formatting_matches_php_precision_14() {
        assert_eq!(format_float(1.0), "1");
        assert_eq!(format_float(0.1 + 0.2), "0.3");
        assert_eq!(format_float(1.0 / 3.0), "0.33333333333333");
        assert_eq!(format_float(1e14), "1.0E+14");
        assert_eq!(format_float(1e13), "10000000000000");
        assert_eq!(format_float(0.00001), "1.0E-5");
        assert_eq!(format_float(0.0001), "0.0001");
        assert_eq!(format_float(-0.0), "-0");
        assert_eq!(format_float(1.5e300), "1.5E+300");
        assert_eq!(format_float(123456789012345.0), "1.2345678901234E+14");
        assert_eq!(format_float(2.5), "2.5");
        assert_eq!(format_float(-1234.5678), "-1234.5678");
        assert_eq!(format_float(f64::NAN), "NAN");
        assert_eq!(format_float(f64::INFINITY), "INF");
    }

    #[test]
    fn casts() {
        assert_eq!(Val::str("12abc").to_int().unwrap(), 12);
        assert_eq!(Val::str("3.5x").to_float().unwrap(), 3.5);
        assert!(!Val::str("0").to_bool());
        assert!(Val::str("0.0").to_bool());
        assert!(!Val::empty_array().to_bool());
        assert_eq!(Val::Float(3.99).to_int().unwrap(), 3);
        assert_eq!(Val::Bool(true).to_php_string().unwrap().as_bytes(), b"1");
        assert_eq!(Val::Null.to_php_string().unwrap().as_bytes(), b"");
        assert!(Val::empty_array().to_php_string().is_err());
        assert!(Val::str("5").as_number("+").is_err());
        assert!(Val::Null.as_number("+").is_err());
    }
}
