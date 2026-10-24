//! `Val` ↔ `surrealdb::types::Value` (DATABASE_LAYER.md §2, "value.rs").

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::Vm;
use surrealdb::types::{Array, Datetime, Number, Object, RecordId, ToSql, Value};

use super::model::ModelData;
use super::record_id;

pub fn to_surreal(vm: &mut Vm, v: &Val) -> Result<Value, Throw> {
    Ok(match v {
        Val::Null => Value::Null,
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
                return Ok(m.id.clone().map(Value::RecordId).unwrap_or(Value::Null));
            }
            let class = vm.class_of(*h);
            if let Some(dti) = vm.lookup_class(b"DateTimeInterface")
                && vm.instance_of(class, dti)
            {
                let s = vm.call_method_sync(*h, b"format", vec![Val::str("Y-m-d\\TH:i:s.uP")])?;
                let s = s.to_php_string()?.to_string_lossy();
                return parse_datetime(&s).map(Value::Datetime).ok_or_else(|| Throw::value_error(format!("cannot convert DateTime `{s}`")));
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
        Value::Number(Number::Decimal(d)) => Val::Float(d.to_string().parse().unwrap_or(0.0)),
        Value::String(s) => Val::str(&s),
        Value::Bytes(b) => Val::bytes(&b.into_inner()),
        Value::Datetime(d) => Val::str(&d.into_inner().to_rfc3339()),
        Value::Uuid(u) => Val::str(&u.into_inner().to_string()),
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
