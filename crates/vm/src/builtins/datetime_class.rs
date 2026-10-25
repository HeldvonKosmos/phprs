//! Native `DateTimeImmutable` and `DateTime` (DECISIONS 2026-10-24, "SurrealDB-Typen
//! nativ"): one payload, two classes. `DateTimeImmutable` returns new objects
//! from its modifiers, `DateTime` changes itself. Both implement the prelude's
//! `DateTimeInterface`; `DateTimeZone` and `DateInterval` stay dialect code.
//! Registered after the prelude (`Vm::with_prelude`), because the interface
//! is declared there.

use std::any::Any;

use crate::error::Throw;
use crate::value::{ArrayKey, ClassId, ObjHandle, PhpArray, Val};
use crate::vm::{Call, NativeClassDef, ObjectHandlers, Vm};

use super::datetime::{Zone, format_date, now_micros, parse_datetime, parse_with_format, date_add, date_diff, date_set};

pub const IMMUTABLE: &str = "DateTimeImmutable";
pub const MUTABLE: &str = "DateTime";

/// Payload: UTC instant (seconds), microseconds, zone name, mutability of the class.
#[derive(Clone, Debug)]
pub struct DateTimeData {
    pub ts: i64,
    pub us: i64,
    pub tz: String,
    pub mutable: bool,
}

impl Default for DateTimeData {
    fn default() -> Self {
        DateTimeData { ts: 0, us: 0, tz: "UTC".into(), mutable: false }
    }
}

fn data<'a>(vm: &'a mut Vm, h: ObjHandle) -> Result<&'a mut DateTimeData, Throw> {
    vm.native_payload_mut::<DateTimeData>(h).ok_or_else(|| Throw::error("DateTime payload missing"))
}

fn this_data(vm: &mut Vm, c: &Call) -> Result<DateTimeData, Throw> {
    let h = c.this()?;
    Ok(data(vm, h)?.clone())
}

fn zone_of(_vm: &Vm, name: &str) -> Result<Zone, Throw> {
    Zone::parse(name).ok_or_else(|| Throw::new("Exception", format!("DateTimeZone::__construct(): Unknown or bad timezone ({name})")))
}

fn default_tz(vm: &Vm) -> String {
    vm.default_timezone.clone()
}

/// A new object of `class` carrying `d`, without running the constructor.
pub fn new_object(vm: &mut Vm, class: ClassId, mut d: DateTimeData) -> Result<ObjHandle, Throw> {
    let h = vm.alloc_object(class)?;
    let mutable = vm.lookup_class(MUTABLE.as_bytes()).is_some_and(|m| vm.instance_of(class, m));
    d.mutable = mutable;
    *data(vm, h)? = d;
    Ok(h)
}

/// Framework entry: a `DateTimeImmutable` for a UTC instant.
pub fn new_immutable(vm: &mut Vm, ts: i64, us: i64, tz: &str) -> Result<Val, Throw> {
    let class = vm.lookup_class(IMMUTABLE.as_bytes()).ok_or_else(|| Throw::error("DateTimeImmutable class missing"))?;
    Ok(Val::Object(new_object(vm, class, DateTimeData { ts, us, tz: tz.to_string(), mutable: false })?))
}

/// `(ts, us)` of any `DateTimeInterface` object, native or dialect.
pub fn instant_of(vm: &mut Vm, h: ObjHandle) -> Result<(i64, i64), Throw> {
    if let Some(d) = vm.native_payload::<DateTimeData>(h) {
        return Ok((d.ts, d.us));
    }
    let ts = vm.call_method_sync(h, b"getTimestamp", Vec::new())?.to_int()?;
    Ok((ts, 0))
}

/// Modifier result: the same object for `DateTime`, a copy for `DateTimeImmutable`.
fn with(vm: &mut Vm, this: ObjHandle, change: impl FnOnce(&mut DateTimeData)) -> Result<Val, Throw> {
    let mutable = data(vm, this)?.mutable;
    if mutable {
        change(data(vm, this)?);
        return Ok(Val::Object(this));
    }
    let mut copy = data(vm, this)?.clone();
    change(&mut copy);
    let class = vm.class_of(this);
    Ok(Val::Object(new_object(vm, class, copy)?))
}

fn tz_name_arg(vm: &mut Vm, v: &Val) -> Result<Option<String>, Throw> {
    match v {
        Val::Null => Ok(None),
        Val::Object(h) => Ok(Some(vm.call_method_sync(*h, b"getName", Vec::new())?.to_php_string()?.to_string_lossy())),
        other => Err(Throw::type_error(format!("DateTimeZone expected, {} given", other.type_name()))),
    }
}

fn construct(vm: &mut Vm, c: Call) -> Result<Val, Throw> {
    let this = c.this()?;
    let input = match c.args.first() {
        None | Some(Val::Null) => "now".to_string(),
        Some(Val::Str(s)) => s.to_string_lossy(),
        Some(o) => return Err(Throw::type_error(format!("DateTime::__construct(): Argument #1 ($datetime) must be of type string, {} given", o.type_name()))),
    };
    let tz = tz_name_arg(vm, &c.arg(1))?.unwrap_or_else(|| default_tz(vm));
    let zone = zone_of(vm, &tz)?;
    let (now, us) = now_micros();
    let (ts, explicit) = parse_datetime(&input, now, &zone).ok_or_else(|| Throw::new("Exception", format!("Failed to parse time string ({input})")))?;
    let class = vm.class_of(this);
    let mutable = vm.lookup_class(MUTABLE.as_bytes()).is_some_and(|m| vm.instance_of(class, m));
    let d = data(vm, this)?;
    d.ts = ts;
    d.us = if input == "now" || input.is_empty() { us } else { 0 };
    d.tz = explicit.map(|z| z.name()).unwrap_or(tz);
    d.mutable = mutable;
    Ok(Val::Null)
}

fn interval_parts(vm: &mut Vm, iv: ObjHandle) -> Result<[i64; 7], Throw> {
    let mut out = [0i64; 7];
    for (i, name) in ["y", "m", "d", "h", "i", "s", "invert"].iter().enumerate() {
        out[i] = vm.get_property(iv, name)?.to_int().unwrap_or(0);
    }
    Ok(out)
}

fn add_interval(vm: &mut Vm, c: &Call, sign: i64) -> Result<Val, Throw> {
    let this = c.this()?;
    let Val::Object(iv) = c.arg(0) else { return Err(Throw::type_error("DateInterval expected")) };
    let p = interval_parts(vm, iv)?;
    let sign = if p[6] != 0 { -sign } else { sign };
    let d = data(vm, this)?.clone();
    let zone = zone_of(vm, &d.tz)?;
    let ts = date_add(d.ts, [p[0], p[1], p[2], p[3], p[4], p[5]], sign, &zone);
    with(vm, this, |d| d.ts = ts)
}

fn make_interval(vm: &mut Vm, parts: [i64; 8]) -> Result<Val, Throw> {
    let class = vm.lookup_class(b"DateInterval").ok_or_else(|| Throw::error("DateInterval class missing"))?;
    let h = vm.instantiate(class, vec![Val::str("PT0S")])?;
    for (i, name) in ["y", "m", "d", "h", "i", "s", "days", "invert"].iter().enumerate() {
        vm.set_property(h, name, Val::Int(parts[i]))?;
    }
    Ok(Val::Object(h))
}

fn methods(def: NativeClassDef) -> NativeClassDef {
    def.method("__construct", construct)
        .method("format", |vm, c| {
            let d = this_data(vm, &c)?;
            let fmt = c.string(0, "DateTime::format")?.to_string_lossy();
            let zone = zone_of(vm, &d.tz)?;
            Ok(Val::from(format_date(&fmt, d.ts, d.us, &zone)))
        })
        .method("getTimestamp", |vm, c| Ok(Val::Int(this_data(vm, &c)?.ts)))
        .method("setTimestamp", |vm, c| {
            let ts = c.int(0, "DateTime::setTimestamp")?;
            with(vm, c.this()?, |d| {
                d.ts = ts;
                d.us = 0;
            })
        })
        .method("getTimezone", |vm, c| {
            let d = this_data(vm, &c)?;
            let class = vm.lookup_class(b"DateTimeZone").ok_or_else(|| Throw::error("DateTimeZone class missing"))?;
            Ok(Val::Object(vm.instantiate(class, vec![Val::from(d.tz)])?))
        })
        .method("setTimezone", |vm, c| {
            let tz = tz_name_arg(vm, &c.arg(0))?.ok_or_else(|| Throw::type_error("DateTimeZone expected"))?;
            with(vm, c.this()?, |d| d.tz = tz)
        })
        .method("getOffset", |vm, c| {
            let d = this_data(vm, &c)?;
            let zone = zone_of(vm, &d.tz)?;
            Ok(Val::Int(zone.offset_at(d.ts) as i64))
        })
        .method("modify", |vm, c| {
            let d = this_data(vm, &c)?;
            let m = c.string(0, "DateTime::modify")?.to_string_lossy();
            let zone = zone_of(vm, &d.tz)?;
            let (ts, _) = parse_datetime(&m, d.ts, &zone).ok_or_else(|| Throw::new("Exception", format!("DateTime::modify(): Failed to parse time string ({m})")))?;
            with(vm, c.this()?, |d| d.ts = ts)
        })
        .method("add", |vm, c| add_interval(vm, &c, 1))
        .method("sub", |vm, c| add_interval(vm, &c, -1))
        .method("setDate", |vm, c| {
            let d = this_data(vm, &c)?;
            let zone = zone_of(vm, &d.tz)?;
            let (y, m, day) = (c.int(0, "DateTime::setDate")?, c.int(1, "DateTime::setDate")?, c.int(2, "DateTime::setDate")?);
            let ts = date_set(d.ts, [Some(y), Some(m), Some(day), None, None, None], &zone);
            with(vm, c.this()?, |d| d.ts = ts)
        })
        .method("setTime", |vm, c| {
            let d = this_data(vm, &c)?;
            let zone = zone_of(vm, &d.tz)?;
            let (h, mi) = (c.int(0, "DateTime::setTime")?, c.int(1, "DateTime::setTime")?);
            let s = match c.args.get(2) {
                None | Some(Val::Null) => 0,
                Some(_) => c.int(2, "DateTime::setTime")?,
            };
            let us = match c.args.get(3) {
                None | Some(Val::Null) => 0,
                Some(_) => c.int(3, "DateTime::setTime")?,
            };
            let ts = date_set(d.ts, [None, None, None, Some(h), Some(mi), Some(s)], &zone);
            with(vm, c.this()?, |d| {
                d.ts = ts;
                d.us = us;
            })
        })
        .method("diff", |vm, c| {
            let d = this_data(vm, &c)?;
            let Val::Object(other) = c.arg(0) else { return Err(Throw::type_error("DateTimeInterface expected")) };
            let (t2, _) = instant_of(vm, other)?;
            let absolute = c.arg(1).to_bool();
            let zone = zone_of(vm, &d.tz)?;
            let mut parts = date_diff(d.ts, t2, &zone);
            if absolute {
                parts[7] = 0;
            }
            make_interval(vm, parts)
        })
        .method("__serialize", |vm, c| {
            let d = this_data(vm, &c)?;
            let zone = zone_of(vm, &d.tz)?;
            let mut a = PhpArray::new();
            a.insert(ArrayKey::from_str("date"), Val::from(format_date("Y-m-d H:i:s.u", d.ts, d.us, &zone)));
            a.insert(ArrayKey::from_str("timezone"), Val::from(d.tz));
            Ok(Val::array(a))
        })
        // ISO 8601 for APIs (DECISIONS 24.10. JSON rule); PHP's object form is not reproduced
        .method("jsonSerialize", |vm, c| {
            let d = this_data(vm, &c)?;
            let zone = zone_of(vm, &d.tz)?;
            let fmt = if d.us != 0 { "Y-m-d\\TH:i:s.uP" } else { "Y-m-d\\TH:i:sP" };
            Ok(Val::from(format_date(fmt, d.ts, d.us, &zone)))
        })
        .static_method("createFromFormat", |vm, c| {
            let fmt = c.string(0, "DateTime::createFromFormat")?.to_string_lossy();
            let s = c.string(1, "DateTime::createFromFormat")?.to_string_lossy();
            let tz = tz_name_arg(vm, &c.arg(2))?.unwrap_or_else(|| default_tz(vm));
            let zone = zone_of(vm, &tz)?;
            match parse_with_format(&fmt, &s, &zone) {
                Some(ts) => Ok(Val::Object(new_object(vm, c.class, DateTimeData { ts, us: 0, tz, mutable: false })?)),
                None => Ok(Val::Bool(false)),
            }
        })
        .static_method("createFromInterface", |vm, c| from_other(vm, c))
        .static_method("createFromMutable", |vm, c| from_other(vm, c))
        .static_method("createFromImmutable", |vm, c| from_other(vm, c))
}

fn from_other(vm: &mut Vm, c: Call) -> Result<Val, Throw> {
    let Val::Object(other) = c.arg(0) else { return Err(Throw::type_error("DateTimeInterface expected")) };
    let (ts, us) = instant_of(vm, other)?;
    let tz = match vm.native_payload::<DateTimeData>(other) {
        Some(d) => d.tz.clone(),
        None => {
            let z = vm.call_method_sync(other, b"getTimezone", Vec::new())?;
            tz_name_arg(vm, &z)?.unwrap_or_else(|| default_tz(vm))
        }
    };
    Ok(Val::Object(new_object(vm, c.class, DateTimeData { ts, us, tz, mutable: false })?))
}

fn compare(vm: &mut Vm, h: ObjHandle, other: &Val) -> Result<Option<std::cmp::Ordering>, Throw> {
    let Val::Object(o) = *other else { return Ok(None) };
    let a = match vm.native_payload::<DateTimeData>(h) {
        Some(d) => (d.ts, d.us),
        None => return Ok(None),
    };
    let b = match vm.native_payload::<DateTimeData>(o) {
        Some(d) => (d.ts, d.us),
        None => {
            // dialect DateTimeInterface implementations compare by timestamp
            let class = vm.class_of(o);
            let Some(iface) = vm.lookup_class(b"DateTimeInterface") else { return Ok(None) };
            if !vm.instance_of(class, iface) {
                return Ok(None);
            }
            instant_of(vm, o)?
        }
    };
    Ok(Some(a.cmp(&b)))
}

fn clone_payload(p: &dyn Any) -> Option<Box<dyn Any>> {
    p.downcast_ref::<DateTimeData>().map(|d| Box::new(d.clone()) as Box<dyn Any>)
}

/// Register both classes; call after the prelude is loaded.
pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let handlers = ObjectHandlers { compare: Some(compare), clone: Some(clone_payload), ..Default::default() };
    let immutable = methods(NativeClassDef::new(IMMUTABLE).implements(&["DateTimeInterface", "JsonSerializable"]).payload(|| Box::new(DateTimeData::default())).trace(crate::vm::gc::trace_nothing).handlers(handlers));
    vm.register_class(immutable)?;
    let mutable = methods(NativeClassDef::new(MUTABLE).implements(&["DateTimeInterface", "JsonSerializable"]).payload(|| Box::new(DateTimeData { mutable: true, ..Default::default() })).trace(crate::vm::gc::trace_nothing).handlers(handlers));
    vm.register_class(mutable)?;
    Ok(())
}
