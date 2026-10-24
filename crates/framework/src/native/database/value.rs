//! `Val` ↔ `surrealdb::types::Value` (DECISIONS 2026-10-24 "SurrealDB-Typen nativ"):
//! every SurrealDB type has a native class, the type of the value decides the
//! binding (no guessing), `null` is written as `NONE`.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::Vm;
use surrealdb::types::{Array, Datetime, Number, Object, RecordId, ToSql, Value};

use super::model::ModelData;
use super::record_id;
use super::types;

pub fn to_surreal(vm: &mut Vm, v: &Val) -> Result<Value, Throw> {
    Ok(match v {
        // null means "no value": NONE (absent field / removed on MERGE)
        Val::Null => Value::None,
        Val::Bool(b) => Value::Bool(*b),
        Val::Int(i) => Value::Number(Number::Int(*i)),
        Val::Float(f) => Value::Number(Number::Float(*f)),
        Val::Str(s) => Value::String(s.to_string_lossy()),
        Val::Array(a) => {
            if a.is_list() {
                let mut arr = Array::new();
                for item in a.values() {
                    arr.push(to_surreal(vm, item)?);
                }
                Value::Array(arr)
            } else {
                let mut obj = Object::new();
                for (k, item) in a.iter() {
                    let key = match k {
                        ArrayKey::Int(i) => i.to_string(),
                        ArrayKey::Str(s) => s.to_string_lossy(),
                    };
                    let val = to_surreal(vm, item)?;
                    obj.insert(key, val);
                }
                Value::Object(obj)
            }
        }
        Val::Object(h) => {
            if let Some(r) = vm.native_payload::<RecordId>(*h) {
                return Ok(Value::RecordId(r.clone()));
            }
            if let Some(m) = vm.native_payload::<ModelData>(*h) {
                return Ok(m.id.clone().map(Value::RecordId).unwrap_or(Value::None));
            }
            if let Some(v) = types::typed_value(vm, *h) {
                return Ok(v);
            }
            let class = vm.class_of(*h);
            if let Some(dti) = vm.lookup_class(b"DateTimeInterface")
                && vm.instance_of(class, dti)
            {
                let (ts, us) = grd_vm::builtins::datetime_class::instant_of(vm, *h)?;
                let dt = DateTime::<Utc>::from_timestamp(ts, (us.clamp(0, 999_999) * 1000) as u32).ok_or_else(|| Throw::value_error("datetime out of range"))?;
                return Ok(Value::Datetime(Datetime::from(dt)));
            }
            return Err(Throw::type_error(format!("cannot bind an object of class {} to a query", vm.class_name(class))));
        }
    })
}

pub fn from_surreal(vm: &mut Vm, v: Value) -> Result<Val, Throw> {
    Ok(match v {
        Value::None | Value::Null => Val::Null,
        Value::Bool(b) => Val::Bool(b),
        Value::Number(Number::Int(i)) => Val::Int(i),
        Value::Number(Number::Float(f)) => Val::Float(f),
        Value::Number(Number::Decimal(d)) => types::new_decimal(vm, d)?,
        Value::String(s) => Val::str(&s),
        Value::Bytes(b) => types::new_bytes(vm, b)?,
        Value::Datetime(d) => {
            let dt = d.into_inner();
            grd_vm::builtins::datetime_class::new_immutable(vm, dt.timestamp(), dt.timestamp_subsec_micros() as i64, "UTC")?
        }
        Value::Duration(d) => types::new_duration(vm, d)?,
        Value::Uuid(u) => types::new_uuid(vm, u)?,
        Value::Geometry(g) => types::new_geometry(vm, g)?,
        Value::RecordId(r) => record_id::new_object(vm, r)?,
        Value::Array(a) => {
            let items = a.into_inner();
            let mut arr = PhpArray::with_capacity(items.len());
            for item in items {
                let val = from_surreal(vm, item)?;
                arr.push(val);
            }
            Val::array(arr)
        }
        Value::Object(o) => {
            let mut arr = PhpArray::new();
            for (k, item) in o.into_inner() {
                let val = from_surreal(vm, item)?;
                arr.insert(ArrayKey::from_str(&k), val);
            }
            Val::array(arr)
        }
        other => Val::str(&other.to_sql()),
    })
}

/// `2026-10-23`, `2026-10-23 14:00:00`, `2026-10-23T14:00:00Z`, RFC 3339.
pub fn parse_datetime(s: &str) -> Option<Datetime> {
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(Datetime::from(d.with_timezone(&Utc)));
    }
    if let Ok(d) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(Datetime::from(d.and_utc()));
    }
    if let Ok(d) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(Datetime::from(d.and_utc()));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(Datetime::from(d.and_hms_opt(0, 0, 0)?.and_utc()));
    }
    None
}

/// Cast a string filter value to the declared SurrealQL field type
/// (`bool`, `int`, `float`, `number`, `decimal`, `datetime`, `record<t>`,
/// `option<…>`); other types keep the string.
pub fn cast_to_field_type(vm: &mut Vm, ty: &str, s: &str) -> Result<Val, Throw> {
    let inner = base_type(ty);
    Ok(match inner.as_str() {
        "bool" => match s {
            "true" | "1" => Val::Bool(true),
            "false" | "0" => Val::Bool(false),
            _ => Val::str(s),
        },
        "int" => s.parse::<i64>().map(Val::Int).unwrap_or_else(|_| Val::str(s)),
        "float" | "number" => s.parse::<f64>().map(Val::Float).unwrap_or_else(|_| Val::str(s)),
        "decimal" => match rust_decimal::Decimal::from_str_exact(s) {
            Ok(d) => types::new_decimal(vm, d)?,
            Err(_) => Val::str(s),
        },
        "datetime" => match parse_datetime(s) {
            Some(d) => {
                let dt = d.into_inner();
                grd_vm::builtins::datetime_class::new_immutable(vm, dt.timestamp(), 0, "UTC")?
            }
            None => Val::str(s),
        },
        t if t.starts_with("record<") => {
            let table = t.trim_start_matches("record<").trim_end_matches('>').split('|').next().unwrap_or("").trim();
            let rid = record_id::parse(s).unwrap_or_else(|| RecordId::new(table, record_id::key_from_str(s)));
            record_id::new_object(vm, rid)?
        }
        "uuid" => match surrealdb::types::Uuid::from_str(s) {
            Ok(u) => types::new_uuid(vm, u)?,
            Err(_) => Val::str(s),
        },
        "duration" => match types::parse_duration(s) {
            Some(d) => types::new_duration(vm, d)?,
            None => Val::str(s),
        },
        _ => Val::str(s),
    })
}

/// The value type of a field declaration without its optional wrapper:
/// `option<int>`, `none | int`, `int | none`, `null | int` all give `int`.
/// A union of several real types is returned unchanged (no cast applies).
pub fn base_type(ty: &str) -> String {
    let ty = ty.trim();
    let ty = ty.strip_prefix("option<").and_then(|t| t.strip_suffix('>')).unwrap_or(ty);
    // split on `|` outside of `<…>`
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in ty.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                parts.push(ty[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(ty[start..].trim());
    let real: Vec<&str> = parts.into_iter().filter(|p| !p.is_empty() && *p != "none" && *p != "null").collect();
    match real.as_slice() {
        [one] => one.to_string(),
        _ => ty.to_string(),
    }
}

use std::str::FromStr as _;

#[cfg(test)]
mod tests {
    use super::base_type;

    #[test]
    fn base_types() {
        assert_eq!(base_type("int"), "int");
        assert_eq!(base_type("option<int>"), "int");
        assert_eq!(base_type("none | int"), "int");
        assert_eq!(base_type("int | none"), "int");
        assert_eq!(base_type("null | datetime"), "datetime");
        assert_eq!(base_type("none | record<tasks | users>"), "record<tasks | users>");
        assert_eq!(base_type("int | string"), "int | string");
    }
}
