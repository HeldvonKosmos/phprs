//! `OpenGrid\Database\RecordId`: a typed record id (`table:key`), compared by value.

use grd_vm::error::Throw;
use grd_vm::value::{ObjHandle, Val};
use grd_vm::vm::{Call, NativeClassDef, ObjectHandlers, Vm};
use surrealdb::types::{RecordId, RecordIdKey, ToSql};

use crate::native::string_arg;

pub const CLASS: &str = "OpenGrid\\Database\\RecordId";

pub fn new_object(vm: &mut Vm, r: RecordId) -> Result<Val, Throw> {
    let class = vm.lookup_class(CLASS.as_bytes()).ok_or_else(|| Throw::error("RecordId class missing"))?;
    let h = vm.instantiate(class, Vec::new())?;
    *vm.native_payload_mut::<RecordId>(h).ok_or_else(|| Throw::error("RecordId payload missing"))? = r;
    Ok(Val::Object(h))
}

/// `tasks:42` → key 42, `tasks:abc` → key "abc", `tasks:⟨a b⟩` → key "a b".
pub fn parse(s: &str) -> Option<RecordId> {
    let (table, key) = s.split_once(':')?;
    if table.is_empty() || key.is_empty() {
        return None;
    }
    let bare = key.trim_matches(|c| c == '⟨' || c == '⟩' || c == '`');
    let key = match bare.parse::<i64>() {
        Ok(i) if !bare.starts_with('+') && bare == key => RecordIdKey::Number(i),
        _ => RecordIdKey::String(bare.to_string()),
    };
    Some(RecordId::new(table, key))
}

/// A record id from a dialect value: a `RecordId` object, or a bare key
/// (int or string) for `table`. Strings are never parsed as `table:key`
/// here (DECISIONS 24.10.: no guessing) — use `RecordId::parse()`.
pub fn from_val(vm: &mut Vm, table: &str, v: &Val) -> Result<RecordId, Throw> {
    match v {
        Val::Object(h) => match vm.native_payload::<RecordId>(*h).cloned() {
            Some(r) => Ok(r),
            None => match vm.native_payload::<super::model::ModelData>(*h).and_then(|m| m.id.clone()) {
                Some(r) => Ok(r),
                None => Err(Throw::type_error("expected a RecordId")),
            },
        },
        Val::Int(i) => Ok(RecordId::new(table, RecordIdKey::Number(*i))),
        Val::Str(s) => Ok(RecordId::new(table, RecordIdKey::String(s.to_string_lossy()))),
        other => Err(Throw::type_error(format!("cannot make a record id from {}", other.type_name()))),
    }
}

/// Key from text: digits → number, else string (used where the schema says `record<t>`).
pub fn key_from_str(s: &str) -> RecordIdKey {
    match s.parse::<i64>() {
        Ok(i) if !s.starts_with('+') && !(s.len() > 1 && s.starts_with('0')) => RecordIdKey::Number(i),
        _ => RecordIdKey::String(s.to_string()),
    }
}

pub fn key_val(r: &RecordId) -> Val {
    match &r.key {
        RecordIdKey::Number(i) => Val::Int(*i),
        RecordIdKey::String(s) => Val::str(s),
        other => Val::str(&other.to_sql()),
    }
}

fn payload<'a>(vm: &'a mut Vm, h: ObjHandle) -> Result<&'a RecordId, Throw> {
    vm.native_payload::<RecordId>(h).ok_or_else(|| Throw::error("RecordId payload missing"))
}

fn this_payload<'a>(vm: &'a mut Vm, c: &Call) -> Result<&'a RecordId, Throw> {
    let h = c.this()?;
    payload(vm, h)
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .implements(&["JsonSerializable"])
        .payload(|| Box::new(RecordId::new("none", RecordIdKey::Number(0))))
        .trace(grd_vm::vm::gc::trace_nothing)
        .handlers(ObjectHandlers {
            trace: None,
            get: None,
            set: None,
            isset: None,
            unset: None,
            arith: None,
            clone: Some(|p| p.downcast_ref::<RecordId>().map(|r| Box::new(r.clone()) as Box<dyn std::any::Any>)),
            compare: Some(|vm, h, other| {
                let Val::Object(o) = other else { return Ok(None) };
                let a = vm.native_payload::<RecordId>(h).map(|r| r.to_sql());
                let b = vm.native_payload::<RecordId>(*o).map(|r| r.to_sql());
                Ok(match (a, b) {
                    (Some(a), Some(b)) => Some(a.cmp(&b)),
                    _ => None,
                })
            }),
        })
        .static_method("make", |vm, c| {
            let table = string_arg(&c, 0, "RecordId::make")?;
            let key = match c.arg(1) {
                Val::Int(i) => RecordIdKey::Number(i),
                Val::Str(s) => RecordIdKey::String(s.to_string_lossy()),
                other => return Err(Throw::type_error(format!("RecordId::make: key must be int or string, {} given", other.type_name()))),
            };
            new_object(vm, RecordId::new(table, key))
        })
        .static_method("parse", |vm, c| {
            let s = string_arg(&c, 0, "RecordId::parse")?;
            match parse(&s) {
                Some(r) => new_object(vm, r),
                None => Err(Throw::value_error(format!("RecordId::parse: `{s}` is not table:key"))),
            }
        })
        .method("table", |vm, c| Ok(Val::str(&String::from(this_payload(vm, &c)?.table.clone()))))
        .method("key", |vm, c| Ok(key_val(this_payload(vm, &c)?)))
        .method("__toString", |vm, c| Ok(Val::str(&this_payload(vm, &c)?.to_sql())))
        .method("toString", |vm, c| Ok(Val::str(&this_payload(vm, &c)?.to_sql())))
        .method("jsonSerialize", |vm, c| Ok(Val::str(&this_payload(vm, &c)?.to_sql())));
    vm.register_class(def)?;
    Ok(())
}
