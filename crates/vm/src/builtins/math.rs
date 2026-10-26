//! Math and random.

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.register_native("abs", |_, a| Ok(match a.first() { Some(Val::Int(i)) => Val::Int(i.checked_abs().ok_or_else(|| Throw::new("ArithmeticError", "Integer overflow in abs()"))?), Some(_) => Val::Float(args::number(a, 0, "abs")?.abs()), None => return Err(Throw::argument_count("abs() expects exactly 1 argument, 0 given")) }));
    vm.register_native("max", |_, a| minmax(a, true));
    vm.register_native("min", |_, a| minmax(a, false));
    vm.register_native("floor", |_, a| Ok(Val::Float(args::number(a, 0, "floor")?.floor())));
    vm.register_native("ceil", |_, a| Ok(Val::Float(args::number(a, 0, "ceil")?.ceil())));
    vm.register_native("round", round);
    vm.register_native("sqrt", |_, a| Ok(Val::Float(args::float(a, 0, "sqrt")?.sqrt())));
    vm.register_native("pow", |vm, a| { args::count(a, 2, "pow")?; vm.binary(crate::bytecode::BinOp::Pow, a[0].clone(), a[1].clone()) });
    vm.register_native("intdiv", |_, a| {
        let x = args::int(a, 0, "intdiv")?;
        let y = args::int(a, 1, "intdiv")?;
        if y == 0 {
            return Err(Throw::new("DivisionByZeroError", "Division by zero"));
        }
        if x == i64::MIN && y == -1 {
            return Err(Throw::new("ArithmeticError", "Division of PHP_INT_MIN by -1 is not an integer"));
        }
        Ok(Val::Int(x / y))
    });
    vm.register_native("fmod", |_, a| Ok(Val::Float(args::float(a, 0, "fmod")? % args::float(a, 1, "fmod")?)));
    vm.register_native("fdiv", |_, a| Ok(Val::Float(args::float(a, 0, "fdiv")? / args::float(a, 1, "fdiv")?)));
    vm.register_native("exp", |_, a| Ok(Val::Float(args::float(a, 0, "exp")?.exp())));
    vm.register_native("log", |_, a| {
        let x = args::float(a, 0, "log")?;
        Ok(Val::Float(match a.get(1) { Some(_) => x.log(args::float(a, 1, "log")?), None => x.ln() }))
    });
    vm.register_native("log10", |_, a| Ok(Val::Float(args::float(a, 0, "log10")?.log10())));
    vm.register_native("log2", |_, a| Ok(Val::Float(args::float(a, 0, "log2")?.log2())));
    vm.register_native("sin", |_, a| Ok(Val::Float(args::float(a, 0, "sin")?.sin())));
    vm.register_native("cos", |_, a| Ok(Val::Float(args::float(a, 0, "cos")?.cos())));
    vm.register_native("tan", |_, a| Ok(Val::Float(args::float(a, 0, "tan")?.tan())));
    vm.register_native("asin", |_, a| Ok(Val::Float(args::float(a, 0, "asin")?.asin())));
    vm.register_native("acos", |_, a| Ok(Val::Float(args::float(a, 0, "acos")?.acos())));
    vm.register_native("atan", |_, a| Ok(Val::Float(args::float(a, 0, "atan")?.atan())));
    vm.register_native("atan2", |_, a| Ok(Val::Float(args::float(a, 0, "atan2")?.atan2(args::float(a, 1, "atan2")?))));
    vm.register_native("pi", |_, _| Ok(Val::Float(std::f64::consts::PI)));
    vm.register_native("hypot", |_, a| Ok(Val::Float(args::float(a, 0, "hypot")?.hypot(args::float(a, 1, "hypot")?))));
    vm.register_native("deg2rad", |_, a| Ok(Val::Float(args::float(a, 0, "deg2rad")?.to_radians())));
    vm.register_native("rad2deg", |_, a| Ok(Val::Float(args::float(a, 0, "rad2deg")?.to_degrees())));
    vm.register_native("is_nan", |_, a| Ok(Val::Bool(args::float(a, 0, "is_nan")?.is_nan())));
    vm.register_native("is_infinite", |_, a| Ok(Val::Bool(args::float(a, 0, "is_infinite")?.is_infinite())));
    vm.register_native("is_finite", |_, a| Ok(Val::Bool(args::float(a, 0, "is_finite")?.is_finite())));
    vm.register_native("rand", |_, a| random_int(a, "rand"));
    vm.register_native("mt_rand", |_, a| random_int(a, "mt_rand"));
    vm.register_native("random_int", |_, a| {
        args::count(a, 2, "random_int")?;
        random_int(a, "random_int")
    });
    vm.register_native("mt_srand", |_, _| Ok(Val::Null));
    vm.register_native("srand", |_, _| Ok(Val::Null));
    vm.register_native("mt_getrandmax", |_, _| Ok(Val::Int(2147483647)));
    vm.register_native("getrandmax", |_, _| Ok(Val::Int(2147483647)));
    vm.register_native("lcg_value", |_, _| Ok(Val::Float(rand_f64())));
    vm.register_native("random_bytes", |_, a| {
        let n = args::int(a, 0, "random_bytes")?.max(0) as usize;
        let mut buf = vec![0u8; n];
        getrandom::fill(&mut buf).map_err(|e| Throw::new("Exception", e.to_string()))?;
        Ok(Val::bytes(&buf))
    });
    vm.register_native("number_format", number_format);
    vm.register_native("base_convert", |_, a| {
        let s = args::string(a, 0, "base_convert")?;
        let from = args::int(a, 1, "base_convert")?;
        let to = args::int(a, 2, "base_convert")?;
        if !(2..=36).contains(&from) {
            return Err(Throw::value_error("base_convert(): Argument #2 ($from_base) must be between 2 and 36 (inclusive)"));
        }
        if !(2..=36).contains(&to) {
            return Err(Throw::value_error("base_convert(): Argument #3 ($to_base) must be between 2 and 36 (inclusive)"));
        }
        let (from, to) = (from as u32, to as u32);
        let v = u128::from_str_radix(&s.to_string_lossy().to_lowercase(), from).unwrap_or(0);
        Ok(Val::from(to_radix(v, to)))
    });
    vm.register_native("bindec", |_, a| Ok(Val::Int(i64::from_str_radix(&args::string(a, 0, "bindec")?.to_string_lossy(), 2).unwrap_or(0))));
    vm.register_native("decbin", |_, a| Ok(Val::from(format!("{:b}", args::int(a, 0, "decbin")?))));
    vm.register_native("hexdec", |_, a| Ok(Val::Int(i64::from_str_radix(&args::string(a, 0, "hexdec")?.to_string_lossy().trim_start_matches("0x"), 16).unwrap_or(0))));
    vm.register_native("dechex", |_, a| Ok(Val::from(format!("{:x}", args::int(a, 0, "dechex")?))));
    vm.register_native("octdec", |_, a| Ok(Val::Int(i64::from_str_radix(&args::string(a, 0, "octdec")?.to_string_lossy(), 8).unwrap_or(0))));
    vm.register_native("decoct", |_, a| Ok(Val::from(format!("{:o}", args::int(a, 0, "decoct")?))));
    vm.register_native("array_sum", |_, a| {
        let arr = args::array(a, 0, "array_sum")?;
        let mut acc = Val::Int(0);
        for v in arr.values() {
            let n = match v {
                Val::Int(_) | Val::Float(_) => v.clone(),
                Val::Str(s) => parse_numeric(s.as_bytes()).map(|(n, _)| n.to_val()).unwrap_or(Val::Int(0)),
                Val::Bool(b) => Val::Int(*b as i64),
                _ => Val::Int(0),
            };
            acc = crate::vm::ops::arith(crate::bytecode::BinOp::Add, &acc, &n)?;
        }
        Ok(acc)
    });
    vm.register_native("array_product", |_, a| {
        let arr = args::array(a, 0, "array_product")?;
        let mut acc = Val::Int(1);
        for v in arr.values() {
            let n = match v {
                Val::Int(_) | Val::Float(_) => v.clone(),
                Val::Str(s) => parse_numeric(s.as_bytes()).map(|(n, _)| n.to_val()).unwrap_or(Val::Int(0)),
                _ => Val::Int(0),
            };
            acc = crate::vm::ops::arith(crate::bytecode::BinOp::Mul, &acc, &n)?;
        }
        Ok(acc)
    });
}

fn to_radix(mut v: u128, radix: u32) -> String {
    if v == 0 {
        return "0".into();
    }
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    while v > 0 {
        out.push(digits[(v % radix as u128) as usize]);
        v /= radix as u128;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

pub(crate) fn rand_u64() -> u64 {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    u64::from_le_bytes(b)
}

fn rand_f64() -> f64 {
    (rand_u64() >> 11) as f64 / (1u64 << 53) as f64
}

fn random_int(a: &[Val], name: &str) -> Result<Val, Throw> {
    let (lo, hi) = if a.len() >= 2 { (args::int(a, 0, name)?, args::int(a, 1, name)?) } else { (0, 2147483647) };
    if lo > hi {
        return Err(Throw::value_error(format!("{name}(): Argument #2 ($max) must be greater than or equal to argument #1 ($min)")));
    }
    let span = (hi as i128 - lo as i128 + 1) as u128;
    let r = (rand_u64() as u128) % span;
    Ok(Val::Int((lo as i128 + r as i128) as i64))
}

fn minmax(a: &[Val], want_max: bool) -> Result<Val, Throw> {
    let items: Vec<Val> = if a.len() == 1 {
        match &a[0] {
            Val::Array(arr) => arr.values().cloned().collect(),
            _ => return Err(Throw::type_error("max(): Argument #1 ($value) must be of type array")),
        }
    } else {
        a.to_vec()
    };
    let mut best: Option<Val> = None;
    for v in items {
        best = Some(match best {
            None => v,
            Some(b) => {
                let ord = compare(&v, &b)?;
                let take = matches!(ord, Some(std::cmp::Ordering::Greater)) == want_max && ord != Some(std::cmp::Ordering::Equal);
                if take { v } else { b }
            }
        });
    }
    best.ok_or_else(|| Throw::value_error("max(): Argument #1 ($value) must contain at least one element"))
}

fn round(_: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let x = args::number(a, 0, "round")?;
    let p = args::int_opt(a, 1, "round", 0)?;
    let m = 10f64.powi(p as i32);
    let scaled = x * m;
    // PHP rounds half away from zero, with a pre-rounding correction
    let r = if scaled.fract().abs() == 0.5 || (scaled.abs() - scaled.abs().trunc() - 0.5).abs() < 1e-9 {
        scaled.trunc() + scaled.signum()
    } else {
        scaled.round()
    };
    let v = r / m;
    if matches!(a.first(), Some(Val::Int(_))) && p >= 0 {
        return Ok(Val::Float(v));
    }
    Ok(Val::Float(v))
}

fn number_format(_: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let x = args::float(a, 0, "number_format")?;
    let decimals = args::int_opt(a, 1, "number_format", 0)?.max(0) as usize;
    let dec_point = match a.get(2) { Some(_) => args::string(a, 2, "number_format")?.to_string(), None => ".".into() };
    let thousands = match a.get(3) { Some(_) => args::string(a, 3, "number_format")?.to_string(), None => ",".into() };
    let m = 10f64.powi(decimals as i32);
    let rounded = (x * m).round() / m;
    let s = format!("{:.*}", decimals, rounded.abs());
    let (int_part, frac) = match s.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (s, None),
    };
    let mut grouped = String::new();
    let bytes = int_part.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            grouped.push_str(&thousands);
        }
        grouped.push(*b as char);
    }
    let mut out = String::new();
    if rounded < 0.0 && rounded != 0.0 {
        out.push('-');
    }
    out.push_str(&grouped);
    if let Some(f) = frac {
        out.push_str(&dec_point);
        out.push_str(&f);
    }
    Ok(Val::from(out))
}
