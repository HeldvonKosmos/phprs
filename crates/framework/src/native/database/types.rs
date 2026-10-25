//! Native value types of the database layer (DECISIONS 2026-10-24, "SurrealDB-Typen
//! nativ"): `OpenGrid\Database\{Decimal, Duration, Uuid, Bytes, Geometry}` with
//! the SDK type as payload. `RecordId` lives in `record_id.rs`, datetimes are
//! the core `DateTimeImmutable`/`DateTime`.

use std::any::Any;
use std::cmp::Ordering;
use std::str::FromStr;

use grd_vm::bytecode::BinOp;
use grd_vm::error::Throw;
use grd_vm::builtins::datetime_class::{self, DateTimeData};
use grd_vm::value::{ArrayKey, ObjHandle, PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, ObjectHandlers, Vm};
use rust_decimal::Decimal as Dec;
use rust_decimal::prelude::ToPrimitive;
use surrealdb::types::{Bytes as SBytes, Duration as SDuration, Geometry as SGeometry, Uuid as SUuid, Value};

use super::value::from_surreal;
use crate::native::string_arg;

pub const DECIMAL: &str = "OpenGrid\\Database\\Decimal";
pub const DURATION: &str = "OpenGrid\\Database\\Duration";
pub const UUID: &str = "OpenGrid\\Database\\Uuid";
pub const BYTES: &str = "OpenGrid\\Database\\Bytes";
pub const GEOMETRY: &str = "OpenGrid\\Database\\Geometry";

// ---- helpers -----------------------------------------------------------------

fn payload<'a, T: 'static>(vm: &'a mut Vm, h: ObjHandle, what: &str) -> Result<&'a mut T, Throw> {
    vm.native_payload_mut::<T>(h).ok_or_else(|| Throw::error(format!("{what} payload missing")))
}

fn this_payload<T: 'static + Clone>(vm: &mut Vm, c: &Call, what: &str) -> Result<T, Throw> {
    let h = c.this()?;
    Ok(payload::<T>(vm, h, what)?.clone())
}

/// A new object of the named class carrying `p`, without running the constructor.
fn new_with<T: 'static>(vm: &mut Vm, class_name: &str, p: T) -> Result<Val, Throw> {
    let class = vm.lookup_class(class_name.as_bytes()).ok_or_else(|| Throw::error(format!("{class_name} class missing")))?;
    let h = vm.instantiate(class, Vec::new())?;
    *payload::<T>(vm, h, class_name)? = p;
    Ok(Val::Object(h))
}

fn clone_of<T: 'static + Clone>(p: &dyn Any) -> Option<Box<dyn Any>> {
    p.downcast_ref::<T>().map(|d| Box::new(d.clone()) as Box<dyn Any>)
}

pub fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

// ---- Decimal -----------------------------------------------------------------

pub fn new_decimal(vm: &mut Vm, d: Dec) -> Result<Val, Throw> {
    new_with(vm, DECIMAL, d)
}

/// A decimal from a dialect value: Decimal object, int, float or numeric string.
pub fn decimal_of(vm: &mut Vm, v: &Val) -> Result<Option<Dec>, Throw> {
    Ok(match v {
        Val::Int(i) => Some(Dec::from(*i)),
        Val::Float(f) => Dec::from_f64_retain(*f),
        Val::Str(s) => Dec::from_str(s.to_string_lossy().trim()).ok(),
        Val::Object(h) => vm.native_payload::<Dec>(*h).cloned(),
        _ => None,
    })
}

fn decimal_arith(vm: &mut Vm, h: ObjHandle, op: BinOp, other: &Val, reversed: bool) -> Result<Option<Val>, Throw> {
    let a = match vm.native_payload::<Dec>(h) {
        Some(d) => *d,
        None => return Ok(None),
    };
    // strict: only Decimal, int and float take part
    let b = match other {
        Val::Int(_) | Val::Float(_) | Val::Object(_) => match decimal_of(vm, other)? {
            Some(d) => d,
            None => return Ok(None),
        },
        _ => return Ok(None),
    };
    let (x, y) = if reversed { (b, a) } else { (a, b) };
    let r = match op {
        BinOp::Add => x.checked_add(y),
        BinOp::Sub => x.checked_sub(y),
        BinOp::Mul => x.checked_mul(y),
        BinOp::Div => {
            if y.is_zero() {
                return Err(Throw::new("DivisionByZeroError", "Division by zero"));
            }
            x.checked_div(y)
        }
        BinOp::Mod => {
            if y.is_zero() {
                return Err(Throw::new("DivisionByZeroError", "Modulo by zero"));
            }
            x.checked_rem(y)
        }
        _ => return Ok(None),
    };
    let r = r.ok_or_else(|| Throw::new("ArithmeticError", "Decimal overflow"))?;
    Ok(Some(new_decimal(vm, r.normalize())?))
}

fn decimal_compare(vm: &mut Vm, h: ObjHandle, other: &Val) -> Result<Option<Ordering>, Throw> {
    let a = match vm.native_payload::<Dec>(h) {
        Some(d) => *d,
        None => return Ok(None),
    };
    Ok(decimal_of(vm, other)?.map(|b| a.cmp(&b)))
}

fn register_decimal(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(DECIMAL)
        .final_class()
        .implements(&["JsonSerializable"])
        .payload(|| Box::new(Dec::ZERO))
        .trace(grd_vm::vm::gc::trace_nothing)
        .handlers(ObjectHandlers { compare: Some(decimal_compare), arith: Some(decimal_arith), clone: Some(clone_of::<Dec>), ..Default::default() })
        .method("__construct", |vm, c| {
            let this = c.this()?;
            let v = c.arg(0);
            let d = match &v {
                Val::Null => Dec::ZERO,
                other => decimal_of(vm, other)?.ok_or_else(|| Throw::value_error(format!("Decimal: cannot parse `{}`", other.to_php_string().map(|s| s.to_string_lossy()).unwrap_or_default())))?,
            };
            *payload::<Dec>(vm, this, "Decimal")? = d;
            Ok(Val::Null)
        })
        .static_method("make", |vm, c| {
            let d = decimal_of(vm, &c.arg(0))?.ok_or_else(|| Throw::value_error("Decimal::make: int, float, Decimal or numeric string expected"))?;
            new_decimal(vm, d)
        })
        .static_method("zero", |vm, _| new_decimal(vm, Dec::ZERO))
        .method("add", |vm, c| {
            let a = this_payload::<Dec>(vm, &c, "Decimal")?;
            let b = decimal_of(vm, &c.arg(0))?.ok_or_else(|| Throw::type_error("Decimal::add: numeric operand expected"))?;
            new_decimal(vm, a.checked_add(b).ok_or_else(|| Throw::new("ArithmeticError", "Decimal overflow"))?)
        })
        .method("sub", |vm, c| {
            let a = this_payload::<Dec>(vm, &c, "Decimal")?;
            let b = decimal_of(vm, &c.arg(0))?.ok_or_else(|| Throw::type_error("Decimal::sub: numeric operand expected"))?;
            new_decimal(vm, a.checked_sub(b).ok_or_else(|| Throw::new("ArithmeticError", "Decimal overflow"))?)
        })
        .method("mul", |vm, c| {
            let a = this_payload::<Dec>(vm, &c, "Decimal")?;
            let b = decimal_of(vm, &c.arg(0))?.ok_or_else(|| Throw::type_error("Decimal::mul: numeric operand expected"))?;
            new_decimal(vm, a.checked_mul(b).ok_or_else(|| Throw::new("ArithmeticError", "Decimal overflow"))?.normalize())
        })
        .method("div", |vm, c| {
            let a = this_payload::<Dec>(vm, &c, "Decimal")?;
            let b = decimal_of(vm, &c.arg(0))?.ok_or_else(|| Throw::type_error("Decimal::div: numeric operand expected"))?;
            if b.is_zero() {
                return Err(Throw::new("DivisionByZeroError", "Division by zero"));
            }
            new_decimal(vm, a.checked_div(b).ok_or_else(|| Throw::new("ArithmeticError", "Decimal overflow"))?.normalize())
        })
        .method("round", |vm, c| {
            let a = this_payload::<Dec>(vm, &c, "Decimal")?;
            let scale = if c.args.is_empty() { 0 } else { c.int(0, "Decimal::round")? }.clamp(0, 28) as u32;
            new_decimal(vm, a.round_dp(scale))
        })
        .method("abs", |vm, c| {
            let a = this_payload::<Dec>(vm, &c, "Decimal")?;
            new_decimal(vm, a.abs())
        })
        .method("negate", |vm, c| {
            let a = this_payload::<Dec>(vm, &c, "Decimal")?;
            new_decimal(vm, -a)
        })
        .method("isZero", |vm, c| Ok(Val::Bool(this_payload::<Dec>(vm, &c, "Decimal")?.is_zero())))
        .method("isNegative", |vm, c| Ok(Val::Bool(this_payload::<Dec>(vm, &c, "Decimal")?.is_sign_negative())))
        .method("scale", |vm, c| Ok(Val::Int(this_payload::<Dec>(vm, &c, "Decimal")?.scale() as i64)))
        .method("toFloat", |vm, c| Ok(Val::Float(this_payload::<Dec>(vm, &c, "Decimal")?.to_f64().unwrap_or(0.0))))
        .method("toInt", |vm, c| Ok(Val::Int(this_payload::<Dec>(vm, &c, "Decimal")?.trunc().to_i64().unwrap_or(0))))
        .method("toString", |vm, c| Ok(Val::from(this_payload::<Dec>(vm, &c, "Decimal")?.to_string())))
        .method("__toString", |vm, c| Ok(Val::from(this_payload::<Dec>(vm, &c, "Decimal")?.to_string())))
        .method("jsonSerialize", |vm, c| Ok(Val::from(this_payload::<Dec>(vm, &c, "Decimal")?.to_string())))
        .method("equals", |vm, c| {
            let a = this_payload::<Dec>(vm, &c, "Decimal")?;
            Ok(Val::Bool(decimal_of(vm, &c.arg(0))? == Some(a)))
        });
    vm.register_class(def)?;
    Ok(())
}

// ---- Duration ----------------------------------------------------------------

pub fn new_duration(vm: &mut Vm, d: SDuration) -> Result<Val, Throw> {
    new_with(vm, DURATION, d)
}

/// SurrealQL duration text: `1y2w3d4h5m6s7ms8us9ns` (any subset, in order).
pub fn parse_duration(s: &str) -> Option<SDuration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total = std::time::Duration::ZERO;
    let mut num = String::new();
    let mut chars = s.chars().peekable();
    let mut any = false;
    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            num.push(ch);
            continue;
        }
        let mut unit = String::from(ch);
        while let Some(&n) = chars.peek() {
            if n.is_ascii_alphabetic() {
                unit.push(n);
                chars.next();
            } else {
                break;
            }
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        let d = match unit.as_str() {
            "ns" => std::time::Duration::from_nanos(n),
            "us" | "µs" => std::time::Duration::from_micros(n),
            "ms" => std::time::Duration::from_millis(n),
            "s" => std::time::Duration::from_secs(n),
            "m" => std::time::Duration::from_secs(n.checked_mul(60)?),
            "h" => std::time::Duration::from_secs(n.checked_mul(3600)?),
            "d" => std::time::Duration::from_secs(n.checked_mul(86400)?),
            "w" => std::time::Duration::from_secs(n.checked_mul(7 * 86400)?),
            "y" => std::time::Duration::from_secs(n.checked_mul(365 * 86400)?),
            _ => return None,
        };
        total = total.checked_add(d)?;
        any = true;
    }
    if !num.is_empty() || !any {
        return None;
    }
    Some(SDuration::from_std(total))
}

/// SurrealQL duration text from a duration (`1h30m`, `0ns`).
pub fn format_duration(d: &SDuration) -> String {
    let std = d.clone().into_inner();
    let mut secs = std.as_secs();
    let nanos = std.subsec_nanos();
    if secs == 0 && nanos == 0 {
        return "0ns".into();
    }
    let mut out = String::new();
    for (unit, size) in [("y", 365 * 86400), ("w", 7 * 86400), ("d", 86400), ("h", 3600), ("m", 60), ("s", 1)] {
        if secs >= size {
            out.push_str(&format!("{}{unit}", secs / size));
            secs %= size;
        }
    }
    if nanos > 0 {
        if nanos % 1_000_000 == 0 {
            out.push_str(&format!("{}ms", nanos / 1_000_000));
        } else if nanos % 1_000 == 0 {
            out.push_str(&format!("{}us", nanos / 1_000));
        } else {
            out.push_str(&format!("{nanos}ns"));
        }
    }
    out
}

fn duration_of(vm: &mut Vm, v: &Val) -> Result<Option<SDuration>, Throw> {
    Ok(match v {
        Val::Object(h) => vm.native_payload::<SDuration>(*h).cloned(),
        Val::Str(s) => parse_duration(&s.to_string_lossy()),
        Val::Int(i) if *i >= 0 => Some(SDuration::from_secs(*i as u64)),
        _ => None,
    })
}

fn duration_arith(vm: &mut Vm, h: ObjHandle, op: BinOp, other: &Val, reversed: bool) -> Result<Option<Val>, Throw> {
    let a = match vm.native_payload::<SDuration>(h) {
        Some(d) => d.clone().into_inner(),
        None => return Ok(None),
    };
    match (op, other) {
        // `$datetime + $duration`, `$duration + $datetime`, `$datetime - $duration`
        // (DECISIONS 2026-10-24 B.5): the result keeps class and zone of the date.
        (BinOp::Add | BinOp::Sub, Val::Object(o)) if vm.native_payload::<DateTimeData>(*o).is_some() => {
            if op == BinOp::Sub && !reversed {
                return Ok(None); // duration - datetime has no meaning
            }
            let d = vm.native_payload::<DateTimeData>(*o).cloned().expect("checked");
            let total = d.ts as i128 * 1_000_000 + d.us as i128;
            let delta = a.as_micros() as i128;
            let total = if op == BinOp::Add { total + delta } else { total - delta };
            let (ts, us) = (total.div_euclid(1_000_000), total.rem_euclid(1_000_000));
            let (Ok(ts), Ok(us)) = (i64::try_from(ts), i64::try_from(us)) else { return Err(Throw::new("ArithmeticError", "datetime out of range")) };
            let class = vm.class_of(*o);
            Ok(Some(Val::Object(datetime_class::new_object(vm, class, DateTimeData { ts, us, tz: d.tz, mutable: d.mutable })?)))
        }
        (BinOp::Add | BinOp::Sub, Val::Object(o)) => {
            let Some(b) = vm.native_payload::<SDuration>(*o).map(|d| d.clone().into_inner()) else { return Ok(None) };
            let (x, y) = if reversed { (b, a) } else { (a, b) };
            let r = if op == BinOp::Add { x.checked_add(y) } else { x.checked_sub(y) };
            let r = r.ok_or_else(|| Throw::new("ArithmeticError", "Duration out of range"))?;
            Ok(Some(new_duration(vm, SDuration::from_std(r))?))
        }
        (BinOp::Mul, Val::Int(n)) if !reversed || true => {
            if *n < 0 {
                return Err(Throw::new("ArithmeticError", "Duration cannot be negative"));
            }
            let r = a.checked_mul(*n as u32).ok_or_else(|| Throw::new("ArithmeticError", "Duration out of range"))?;
            Ok(Some(new_duration(vm, SDuration::from_std(r))?))
        }
        (BinOp::Div, Val::Int(n)) if !reversed => {
            if *n <= 0 {
                return Err(Throw::new("DivisionByZeroError", "Division by zero"));
            }
            Ok(Some(new_duration(vm, SDuration::from_std(a / *n as u32))?))
        }
        _ => Ok(None),
    }
}

fn duration_compare(vm: &mut Vm, h: ObjHandle, other: &Val) -> Result<Option<Ordering>, Throw> {
    let a = match vm.native_payload::<SDuration>(h) {
        Some(d) => d.clone().into_inner(),
        None => return Ok(None),
    };
    Ok(duration_of(vm, other)?.map(|b| a.cmp(&b.into_inner())))
}

fn register_duration(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(DURATION)
        .final_class()
        .implements(&["JsonSerializable"])
        .payload(|| Box::new(SDuration::from_secs(0)))
        .trace(grd_vm::vm::gc::trace_nothing)
        .handlers(ObjectHandlers { compare: Some(duration_compare), arith: Some(duration_arith), clone: Some(clone_of::<SDuration>), ..Default::default() })
        .method("__construct", |vm, c| {
            let this = c.this()?;
            let d = match c.arg(0) {
                Val::Null => SDuration::from_secs(0),
                v => duration_of(vm, &v)?.ok_or_else(|| Throw::value_error("Duration: text like `1h30m` or seconds expected"))?,
            };
            *payload::<SDuration>(vm, this, "Duration")? = d;
            Ok(Val::Null)
        })
        .static_method("make", |vm, c| {
            let d = duration_of(vm, &c.arg(0))?.ok_or_else(|| Throw::value_error("Duration::make: text like `1h30m` or seconds expected"))?;
            new_duration(vm, d)
        })
        .static_method("seconds", |vm, c| new_duration(vm, SDuration::from_secs(c.int(0, "Duration::seconds")?.max(0) as u64)))
        .static_method("minutes", |vm, c| new_duration(vm, SDuration::from_secs(c.int(0, "Duration::minutes")?.max(0) as u64 * 60)))
        .static_method("hours", |vm, c| new_duration(vm, SDuration::from_secs(c.int(0, "Duration::hours")?.max(0) as u64 * 3600)))
        .static_method("days", |vm, c| new_duration(vm, SDuration::from_secs(c.int(0, "Duration::days")?.max(0) as u64 * 86400)))
        .method("totalSeconds", |vm, c| Ok(Val::Int(this_payload::<SDuration>(vm, &c, "Duration")?.secs() as i64)))
        .method("totalMillis", |vm, c| Ok(Val::Int(this_payload::<SDuration>(vm, &c, "Duration")?.millis() as i64)))
        .method("totalMinutes", |vm, c| Ok(Val::Int(this_payload::<SDuration>(vm, &c, "Duration")?.mins() as i64)))
        .method("totalHours", |vm, c| Ok(Val::Int(this_payload::<SDuration>(vm, &c, "Duration")?.hours() as i64)))
        .method("totalDays", |vm, c| Ok(Val::Int(this_payload::<SDuration>(vm, &c, "Duration")?.days() as i64)))
        .method("toString", |vm, c| Ok(Val::from(format_duration(&this_payload::<SDuration>(vm, &c, "Duration")?))))
        .method("__toString", |vm, c| Ok(Val::from(format_duration(&this_payload::<SDuration>(vm, &c, "Duration")?))))
        .method("jsonSerialize", |vm, c| Ok(Val::from(format_duration(&this_payload::<SDuration>(vm, &c, "Duration")?))));
    vm.register_class(def)?;
    Ok(())
}

// ---- Uuid --------------------------------------------------------------------

pub fn new_uuid(vm: &mut Vm, u: SUuid) -> Result<Val, Throw> {
    new_with(vm, UUID, u)
}

fn uuid_compare(vm: &mut Vm, h: ObjHandle, other: &Val) -> Result<Option<Ordering>, Throw> {
    let a = match vm.native_payload::<SUuid>(h) {
        Some(u) => u.clone().into_inner(),
        None => return Ok(None),
    };
    Ok(match other {
        Val::Object(o) => vm.native_payload::<SUuid>(*o).map(|u| a.cmp(&u.clone().into_inner())),
        Val::Str(s) => uuid::Uuid::parse_str(&s.to_string_lossy()).ok().map(|u| a.cmp(&u)),
        _ => None,
    })
}

fn register_uuid(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(UUID)
        .final_class()
        .implements(&["JsonSerializable"])
        .payload(|| Box::new(SUuid::new_v4()))
        .trace(grd_vm::vm::gc::trace_nothing)
        .handlers(ObjectHandlers { compare: Some(uuid_compare), clone: Some(clone_of::<SUuid>), ..Default::default() })
        .method("__construct", |vm, c| {
            let this = c.this()?;
            if let Val::Str(s) = c.arg(0) {
                let u = SUuid::from_str(&s.to_string_lossy()).map_err(|_| Throw::value_error(format!("Uuid: `{}` is not a UUID", s.to_string_lossy())))?;
                *payload::<SUuid>(vm, this, "Uuid")? = u;
            }
            Ok(Val::Null)
        })
        .static_method("v4", |vm, _| new_uuid(vm, SUuid::new_v4()))
        .static_method("v7", |vm, _| new_uuid(vm, SUuid::new_v7()))
        .static_method("parse", |vm, c| {
            let s = string_arg(&c, 0, "Uuid::parse")?;
            let u = SUuid::from_str(&s).map_err(|_| Throw::value_error(format!("Uuid::parse: `{s}` is not a UUID")))?;
            new_uuid(vm, u)
        })
        .method("toString", |vm, c| Ok(Val::from(this_payload::<SUuid>(vm, &c, "Uuid")?.into_inner().to_string())))
        .method("__toString", |vm, c| Ok(Val::from(this_payload::<SUuid>(vm, &c, "Uuid")?.into_inner().to_string())))
        .method("jsonSerialize", |vm, c| Ok(Val::from(this_payload::<SUuid>(vm, &c, "Uuid")?.into_inner().to_string())));
    vm.register_class(def)?;
    Ok(())
}

// ---- Bytes -------------------------------------------------------------------

pub fn new_bytes(vm: &mut Vm, b: SBytes) -> Result<Val, Throw> {
    new_with(vm, BYTES, b)
}

fn bytes_compare(vm: &mut Vm, h: ObjHandle, other: &Val) -> Result<Option<Ordering>, Throw> {
    let a = match vm.native_payload::<SBytes>(h) {
        Some(b) => b.clone().into_inner(),
        None => return Ok(None),
    };
    Ok(match other {
        Val::Object(o) => vm.native_payload::<SBytes>(*o).map(|b| a.as_ref().cmp(b.clone().into_inner().as_ref())),
        _ => None,
    })
}

fn register_bytes(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(BYTES)
        .final_class()
        .implements(&["JsonSerializable"])
        .payload(|| Box::new(SBytes::from(Vec::<u8>::new())))
        .trace(grd_vm::vm::gc::trace_nothing)
        .handlers(ObjectHandlers { compare: Some(bytes_compare), clone: Some(clone_of::<SBytes>), ..Default::default() })
        .method("__construct", |vm, c| {
            let this = c.this()?;
            if let Val::Str(s) = c.arg(0) {
                *payload::<SBytes>(vm, this, "Bytes")? = SBytes::from(s.as_bytes().to_vec());
            }
            Ok(Val::Null)
        })
        .static_method("make", |vm, c| {
            let Val::Str(s) = c.arg(0) else { return Err(Throw::type_error("Bytes::make: string expected")) };
            new_bytes(vm, SBytes::from(s.as_bytes().to_vec()))
        })
        .method("length", |vm, c| Ok(Val::Int(this_payload::<SBytes>(vm, &c, "Bytes")?.into_inner().len() as i64)))
        .method("toString", |vm, c| Ok(Val::bytes(&this_payload::<SBytes>(vm, &c, "Bytes")?.into_inner())))
        .method("__toString", |vm, c| Ok(Val::bytes(&this_payload::<SBytes>(vm, &c, "Bytes")?.into_inner())))
        .method("toBase64", |vm, c| Ok(Val::from(base64_encode(&this_payload::<SBytes>(vm, &c, "Bytes")?.into_inner()))))
        .method("jsonSerialize", |vm, c| Ok(Val::from(base64_encode(&this_payload::<SBytes>(vm, &c, "Bytes")?.into_inner()))));
    vm.register_class(def)?;
    Ok(())
}

// ---- Geometry ----------------------------------------------------------------

pub fn new_geometry(vm: &mut Vm, g: SGeometry) -> Result<Val, Throw> {
    new_with(vm, GEOMETRY, g)
}

fn geometry_compare(vm: &mut Vm, h: ObjHandle, other: &Val) -> Result<Option<Ordering>, Throw> {
    let a = match vm.native_payload::<SGeometry>(h) {
        Some(g) => g.clone(),
        None => return Ok(None),
    };
    Ok(match other {
        Val::Object(o) => vm.native_payload::<SGeometry>(*o).map(|g| if *g == a { Ordering::Equal } else { Ordering::Less }),
        _ => None,
    })
}

/// GeoJSON object with `type` before `coordinates` (the SDK's `as_object` is
/// key-sorted and would put `coordinates` first).
fn geojson(vm: &mut Vm, g: &SGeometry) -> Result<Val, Throw> {
    let mut arr = PhpArray::new();
    arr.insert(ArrayKey::from_str("type"), Val::str(g.as_type()));
    if let SGeometry::Collection(parts) = g {
        let mut list = PhpArray::new();
        for part in parts {
            let v = geojson(vm, part)?;
            list.push(v);
        }
        arr.insert(ArrayKey::from_str("geometries"), Val::array(list));
    } else {
        let coords = from_surreal(vm, g.as_coordinates())?;
        arr.insert(ArrayKey::from_str("coordinates"), coords);
    }
    Ok(Val::array(arr))
}

fn register_geometry(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(GEOMETRY)
        .final_class()
        .implements(&["JsonSerializable"])
        .payload(|| Box::new(SGeometry::Point(geo::Point::new(0.0, 0.0))))
        .trace(grd_vm::vm::gc::trace_nothing)
        .handlers(ObjectHandlers { compare: Some(geometry_compare), clone: Some(clone_of::<SGeometry>), ..Default::default() })
        .static_method("point", |vm, c| {
            let (x, y) = (c.float(0, "Geometry::point")?, c.float(1, "Geometry::point")?);
            new_geometry(vm, SGeometry::Point(geo::Point::new(x, y)))
        })
        .static_method("line", |vm, c| {
            let Val::Array(points) = c.arg(0) else { return Err(Throw::type_error("Geometry::line: array of [x, y] expected")) };
            let mut coords = Vec::new();
            for p in points.values() {
                let Val::Array(xy) = p else { return Err(Throw::type_error("Geometry::line: array of [x, y] expected")) };
                let mut it = xy.values();
                let (Some(x), Some(y)) = (it.next(), it.next()) else { return Err(Throw::type_error("Geometry::line: array of [x, y] expected")) };
                coords.push(geo::Coord { x: x.to_float()?, y: y.to_float()? });
            }
            new_geometry(vm, SGeometry::Line(geo::LineString::new(coords)))
        })
        .static_method("fromGeoJson", |vm, c| {
            let Val::Array(_) = c.arg(0) else { return Err(Throw::type_error("Geometry::fromGeoJson: array expected")) };
            let v = super::value::to_surreal(vm, &c.arg(0))?;
            let Value::Object(o) = v else { return Err(Throw::type_error("Geometry::fromGeoJson: object expected")) };
            let g = SGeometry::try_from_object(&o).ok_or_else(|| Throw::value_error("Geometry::fromGeoJson: not a GeoJSON geometry"))?;
            new_geometry(vm, g)
        })
        .method("type", |vm, c| Ok(Val::str(this_payload::<SGeometry>(vm, &c, "Geometry")?.as_type())))
        .method("coordinates", |vm, c| {
            let g = this_payload::<SGeometry>(vm, &c, "Geometry")?;
            from_surreal(vm, g.as_coordinates())
        })
        .method("toGeoJson", |vm, c| {
            let g = this_payload::<SGeometry>(vm, &c, "Geometry")?;
            geojson(vm, &g)
        })
        .method("jsonSerialize", |vm, c| {
            let g = this_payload::<SGeometry>(vm, &c, "Geometry")?;
            geojson(vm, &g)
        });
    vm.register_class(def)?;
    Ok(())
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    register_decimal(vm)?;
    register_duration(vm)?;
    register_uuid(vm)?;
    register_bytes(vm)?;
    register_geometry(vm)?;
    Ok(())
}

/// Convert a dialect value into a typed `Value` where the object is one of
/// the native types; `None` when it is not one of ours.
pub fn typed_value(vm: &mut Vm, h: ObjHandle) -> Option<Value> {
    if let Some(d) = vm.native_payload::<Dec>(h) {
        return Some(Value::Number(surrealdb::types::Number::Decimal(*d)));
    }
    if let Some(d) = vm.native_payload::<SDuration>(h) {
        return Some(Value::Duration(d.clone()));
    }
    if let Some(u) = vm.native_payload::<SUuid>(h) {
        return Some(Value::Uuid(u.clone()));
    }
    if let Some(b) = vm.native_payload::<SBytes>(h) {
        return Some(Value::Bytes(b.clone()));
    }
    if let Some(g) = vm.native_payload::<SGeometry>(h) {
        return Some(Value::Geometry(g.clone()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_round_trip() {
        for (text, secs) in [("1h30m", 5400), ("2d", 172800), ("45s", 45), ("1w", 604800)] {
            let d = parse_duration(text).expect(text);
            assert_eq!(d.secs(), secs);
            assert_eq!(format_duration(&d), text);
        }
        assert_eq!(format_duration(&SDuration::from_millis(1500)), "1s500ms");
        assert!(parse_duration("abc").is_none());
        assert!(parse_duration("1").is_none());
    }

    #[test]
    fn base64() {
        assert_eq!(base64_encode(b"hi"), "aGk=");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b""), "");
    }
}
