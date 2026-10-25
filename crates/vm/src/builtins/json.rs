//! json_encode / json_decode with PHP flag semantics.

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

const UNESCAPED_SLASHES: i64 = 64;
const PRETTY_PRINT: i64 = 128;
const UNESCAPED_UNICODE: i64 = 256;
const PRESERVE_ZERO_FRACTION: i64 = 1024;
const FORCE_OBJECT: i64 = 16;
const THROW_ON_ERROR: i64 = 4194304;
const OBJECT_AS_ARRAY: i64 = 1;

pub fn register(vm: &mut Vm) {
    vm.register_native("json_encode", |vm, a| {
        args::count(a, 1, "json_encode")?;
        let flags = args::int_opt(a, 1, "json_encode", 0)?;
        let depth = args::int_opt(a, 2, "json_encode", 512)?;
        let mut out = Vec::new();
        match encode(vm, &a[0], flags, 0, depth as usize, &mut out) {
            Ok(()) => {
                vm.json_error = None;
                Ok(Val::bytes(&out))
            }
            Err(JsonErr::Php(t)) => Err(t),
            Err(JsonErr::Json(code, msg)) => {
                if flags & THROW_ON_ERROR != 0 {
                    return Err(Throw { class: "JsonException".into(), message: msg, code, object: None });
                }
                vm.json_error = Some((code, msg));
                Ok(Val::Bool(false))
            }
        }
    });
    vm.register_native("json_decode", |vm, a| {
        let s = args::string(a, 0, "json_decode")?;
        let flags = args::int_opt(a, 3, "json_decode", 0)?;
        let assoc = match a.get(1) { Some(Val::Bool(b)) => *b, Some(Val::Null) | None => flags & OBJECT_AS_ARRAY != 0, Some(v) => v.to_bool() };
        let text = match s.as_str() {
            Some(t) => t,
            None => {
                let (code, msg) = (5, "Malformed UTF-8 characters, possibly incorrectly encoded".to_string());
                if flags & THROW_ON_ERROR != 0 {
                    return Err(Throw { class: "JsonException".into(), message: msg, code, object: None });
                }
                vm.json_error = Some((code, msg));
                return Ok(Val::Null);
            }
        };
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(v) => {
                vm.json_error = None;
                decode(vm, v, assoc)
            }
            Err(_) => {
                let (code, msg) = (4, "Syntax error".to_string());
                if flags & THROW_ON_ERROR != 0 {
                    return Err(Throw { class: "JsonException".into(), message: msg, code, object: None });
                }
                vm.json_error = Some((code, msg));
                Ok(Val::Null)
            }
        }
    });
    vm.register_native("json_last_error", |vm, _| Ok(Val::Int(vm.json_error.as_ref().map(|e| e.0).unwrap_or(0))));
    vm.register_native("json_last_error_msg", |vm, _| Ok(Val::from(vm.json_error.as_ref().map(|e| e.1.clone()).unwrap_or_else(|| "No error".into()))));
    vm.register_native("json_validate", |_, a| Ok(Val::Bool(args::string(a, 0, "json_validate")?.as_str().is_some_and(|t| serde_json::from_str::<serde_json::Value>(t).is_ok()))));
}

enum JsonErr {
    Php(Throw),
    Json(i64, String),
}

impl From<Throw> for JsonErr {
    fn from(t: Throw) -> Self {
        JsonErr::Php(t)
    }
}

fn encode_string(s: &[u8], flags: i64, out: &mut Vec<u8>) -> Result<(), JsonErr> {
    let text = std::str::from_utf8(s).map_err(|_| JsonErr::Json(5, "Malformed UTF-8 characters, possibly incorrectly encoded".into()))?;
    out.push(b'"');
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '/' if flags & UNESCAPED_SLASHES == 0 => out.extend_from_slice(b"\\/"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\x08' => out.extend_from_slice(b"\\b"),
            '\x0c' => out.extend_from_slice(b"\\f"),
            c if (c as u32) < 0x20 => out.extend_from_slice(format!("\\u{:04x}", c as u32).as_bytes()),
            c if (c as u32) > 0x7f && flags & UNESCAPED_UNICODE == 0 => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.extend_from_slice(format!("\\u{:04x}", u).as_bytes());
                }
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
    Ok(())
}

fn newline(flags: i64, depth: usize, out: &mut Vec<u8>) {
    if flags & PRETTY_PRINT != 0 {
        out.push(b'\n');
        out.extend(std::iter::repeat_n(b' ', depth * 4));
    }
}

fn encode(vm: &mut Vm, v: &Val, flags: i64, depth: usize, max_depth: usize, out: &mut Vec<u8>) -> Result<(), JsonErr> {
    if depth > max_depth {
        return Err(JsonErr::Json(1, "Maximum stack depth exceeded".into()));
    }
    match v {
        Val::Null => out.extend_from_slice(b"null"),
        Val::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Val::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Val::Float(f) => {
            if !f.is_finite() {
                return Err(JsonErr::Json(7, "Inf and NaN cannot be JSON encoded".into()));
            }
            let mut s = crate::value::repr_float(*f, b'e');
            if flags & PRESERVE_ZERO_FRACTION != 0 && !s.contains(['.', 'e']) {
                s.push_str(".0");
            }
            out.extend_from_slice(s.as_bytes());
        }
        Val::Str(s) => encode_string(s.as_bytes(), flags, out)?,
        Val::Array(a) => {
            if a.is_list() && flags & FORCE_OBJECT == 0 {
                out.push(b'[');
                for (i, val) in a.values().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    newline(flags, depth + 1, out);
                    encode(vm, val, flags, depth + 1, max_depth, out)?;
                }
                if !a.is_empty() {
                    newline(flags, depth, out);
                }
                out.push(b']');
            } else {
                encode_map(vm, a.iter().map(|(k, v)| (key_bytes(k), v.clone())).collect(), flags, depth, max_depth, out)?;
            }
        }
        Val::Object(h) => {
            // an object being encoded is protected, as in PHP: meeting it again
            // (a cycle, or json_encode($this) inside jsonSerialize) is an error
            if vm.json_visiting.contains(h) {
                return Err(JsonErr::Json(6, "Recursion detected".into()));
            }
            vm.json_visiting.push(*h);
            let r = encode_object(vm, *h, flags, depth, max_depth, out);
            vm.json_visiting.pop();
            r?;
        }
    }
    Ok(())
}

fn encode_object(vm: &mut Vm, h: ObjHandle, flags: i64, depth: usize, max_depth: usize, out: &mut Vec<u8>) -> Result<(), JsonErr> {
    let class = vm.objects.get(h).class;
    if vm.instance_of(class, vm.wk().json_serializable) {
        let r = vm.call_method_sync(h, b"jsonSerialize", Vec::new())?;
        // `return $this;` encodes the object's properties (PHP does the same)
        if matches!(r, Val::Object(o) if o == h) {
            return encode_props(vm, h, flags, depth, max_depth, out);
        }
        return encode(vm, &r, flags, depth, max_depth, out);
    }
    if vm.class(class).is_enum() {
        return match vm.get_prop_raw(h, b"value") {
            Some(v) => encode(vm, &v, flags, depth, max_depth, out),
            None => Err(JsonErr::Json(11, "Non-backed enums have no default serialization".into())),
        };
    }
    encode_props(vm, h, flags, depth, max_depth, out)
}

/// Public properties only.
fn encode_props(vm: &mut Vm, h: ObjHandle, flags: i64, depth: usize, max_depth: usize, out: &mut Vec<u8>) -> Result<(), JsonErr> {
    let class = vm.objects.get(h).class;
    let obj = vm.objects.get(h);
    let rt = vm.class(class);
    let mut entries: Vec<(Vec<u8>, Val)> = Vec::new();
    for (i, p) in rt.props.iter().enumerate() {
        if p.visibility == crate::bytecode::Visibility::Public
            && let Some(val) = &obj.slots[i]
        {
            entries.push((p.name.clone(), val.clone()));
        }
    }
    for (k, val) in &obj.dynamic {
        entries.push((k.as_bytes().to_vec(), val.clone()));
    }
    encode_map(vm, entries, flags, depth, max_depth, out)
}

fn key_bytes(k: &ArrayKey) -> Vec<u8> {
    match k {
        ArrayKey::Int(i) => i.to_string().into_bytes(),
        ArrayKey::Str(s) => s.as_bytes().to_vec(),
    }
}

fn encode_map(vm: &mut Vm, entries: Vec<(Vec<u8>, Val)>, flags: i64, depth: usize, max_depth: usize, out: &mut Vec<u8>) -> Result<(), JsonErr> {
    out.push(b'{');
    let n = entries.len();
    for (i, (k, v)) in entries.into_iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        newline(flags, depth + 1, out);
        encode_string(&k, flags, out)?;
        out.push(b':');
        if flags & PRETTY_PRINT != 0 {
            out.push(b' ');
        }
        encode(vm, &v, flags, depth + 1, max_depth, out)?;
    }
    if n > 0 {
        newline(flags, depth, out);
    }
    out.push(b'}');
    Ok(())
}

fn decode(vm: &mut Vm, v: serde_json::Value, assoc: bool) -> Result<Val, Throw> {
    Ok(match v {
        serde_json::Value::Null => Val::Null,
        serde_json::Value::Bool(b) => Val::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Val::Int(i)
            } else {
                Val::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Val::from(s),
        serde_json::Value::Array(items) => {
            let mut arr = PhpArray::with_capacity(items.len());
            for item in items {
                let val = decode(vm, item, assoc)?;
                arr.push(val);
            }
            Val::array(arr)
        }
        serde_json::Value::Object(map) => {
            if assoc {
                let mut arr = PhpArray::with_capacity(map.len());
                for (k, item) in map {
                    let val = decode(vm, item, assoc)?;
                    arr.insert(ArrayKey::from_str(&k), val);
                }
                Val::array(arr)
            } else {
                let h = vm.alloc_object(vm.wk().stdclass)?;
                for (k, item) in map {
                    let val = decode(vm, item, assoc)?;
                    vm.objects.get_mut(h).dynamic.insert(PhpString::from(k), val);
                }
                Val::Object(h)
            }
        }
    })
}

/// Framework edge: encode a value as JSON (unescaped slashes and unicode).
pub fn to_json(vm: &mut Vm, v: &Val, pretty: bool) -> Result<Vec<u8>, Throw> {
    let flags = UNESCAPED_SLASHES | UNESCAPED_UNICODE | if pretty { PRETTY_PRINT } else { 0 };
    let mut out = Vec::new();
    match encode(vm, v, flags, 0, 512, &mut out) {
        Ok(()) => Ok(out),
        Err(JsonErr::Php(t)) => Err(t),
        Err(JsonErr::Json(code, msg)) => Err(Throw { class: "JsonException".into(), message: msg, code, object: None }),
    }
}

/// Framework edge: a JSON document as dialect value (objects become arrays).
pub fn from_json(vm: &mut Vm, v: serde_json::Value) -> Result<Val, Throw> {
    decode(vm, v, true)
}
