//! `OpenGrid\Http\Request` and the `request()` helper. The object carries no
//! payload; everything is read from the worker's `RequestState`.

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, Vm};

use super::{ctx, pairs_to_array, string_arg};
use crate::http::parse_query;
use crate::worker::RequestState;

pub const CLASS: &str = "OpenGrid\\Http\\Request";

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .method("method", |vm, _| Ok(Val::str(&state(vm)?.req.method)))
        .method("path", |vm, _| Ok(Val::str(&state(vm)?.req.path)))
        .method("url", |vm, _| {
            let s = state(vm)?;
            Ok(Val::str(&if s.req.query.is_empty() { s.req.path.clone() } else { format!("{}?{}", s.req.path, s.req.query) }))
        })
        .method("isMethod", |vm, c| {
            let m = string_arg(&c, 0, "Request::isMethod")?;
            Ok(Val::Bool(state(vm)?.req.method.eq_ignore_ascii_case(&m)))
        })
        .method("query", |vm, c| {
            let pairs = state(vm)?.req.query_pairs();
            pick(pairs_to_array(&pairs), &c)
        })
        .method("input", |vm, c| {
            let all = input_all(vm)?;
            pick(all, &c)
        })
        .method("all", |vm, _| input_all(vm))
        .method("has", |vm, c| {
            let key = string_arg(&c, 0, "Request::has")?;
            let all = input_all(vm)?;
            let Val::Array(arr) = all else { return Ok(Val::Bool(false)) };
            Ok(Val::Bool(arr.contains_key(&ArrayKey::from_str(&key))))
        })
        .method("json", |vm, _| {
            let body = state(vm)?.req.body.clone();
            match serde_json::from_slice::<serde_json::Value>(&body) {
                Ok(v) => grd_vm::json::from_json(vm, v),
                Err(_) => Ok(Val::Null),
            }
        })
        .method("isJson", |vm, _| Ok(Val::Bool(is_json(state(vm)?))))
        .method("header", |vm, c| {
            let name = string_arg(&c, 0, "Request::header")?;
            Ok(match state(vm)?.req.header(&name) {
                Some(v) => Val::str(v),
                None => c.arg(1),
            })
        })
        .method("headers", |vm, _| Ok(pairs_to_array(&state(vm)?.req.headers)))
        .method("bearerToken", |vm, _| {
            Ok(match state(vm)?.req.header("authorization") {
                Some(v) if v.len() > 7 && v[..7].eq_ignore_ascii_case("bearer ") => Val::str(v[7..].trim()),
                _ => Val::Null,
            })
        })
        .method("route", |vm, c| {
            let params = state(vm)?.params.clone();
            pick(pairs_to_array(&params), &c)
        })
        .method("ip", |_, _| Ok(Val::Null))
        .method("body", |vm, _| Ok(Val::bytes(&state(vm)?.req.body)))
        .method("validate", |vm, c| {
            let all = input_all(vm)?;
            super::validation::validate_or_throw(vm, &all, &c.arg(0))
        });
    vm.register_class(def)?;
    vm.register_native("request", |vm, _| {
        let s = state(vm)?;
        if let Some(h) = s.object {
            return Ok(Val::Object(h));
        }
        let class = vm.lookup_class(CLASS.as_bytes()).ok_or_else(|| Throw::error("Request class missing"))?;
        let h = vm.instantiate(class, Vec::new())?;
        state(vm)?.object = Some(h);
        Ok(Val::Object(h))
    });
    Ok(())
}

fn state(vm: &mut Vm) -> Result<&mut RequestState, Throw> {
    ctx(vm).request.as_mut().ok_or_else(|| Throw::error("no active request"))
}

fn is_json(s: &RequestState) -> bool {
    s.req.header("content-type").is_some_and(|ct| ct.contains("json"))
}

/// `query()` returns all, `query('k')` one value, `query('k', $default)`.
fn pick(all: Val, c: &Call) -> Result<Val, Throw> {
    match c.args.first() {
        None | Some(Val::Null) => Ok(all),
        Some(_) => {
            let key = string_arg(c, 0, "Request")?;
            let Val::Array(arr) = &all else { return Ok(c.arg(1)) };
            Ok(arr.get(&ArrayKey::from_str(&key)).cloned().unwrap_or_else(|| c.arg(1)))
        }
    }
}

/// Query parameters overlaid by the body (JSON or form), cached per request.
fn input_all(vm: &mut Vm) -> Result<Val, Throw> {
    if let Some(v) = &state(vm)?.input {
        return Ok(v.clone());
    }
    let (query, body, json) = {
        let s = state(vm)?;
        (s.req.query_pairs(), s.req.body.clone(), is_json(s))
    };
    let mut arr = PhpArray::new();
    for (k, v) in &query {
        arr.insert(ArrayKey::from_str(k), Val::str(v));
    }
    if json {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_slice::<serde_json::Value>(&body) {
            for (k, v) in map {
                let val = grd_vm::json::from_json(vm, v)?;
                arr.insert(ArrayKey::from_str(&k), val);
            }
        }
    } else if !body.is_empty() {
        for (k, v) in parse_query(&String::from_utf8_lossy(&body)) {
            arr.insert(ArrayKey::from_str(&k), Val::str(&v));
        }
    }
    let all = Val::array(arr);
    state(vm)?.input = Some(all.clone());
    Ok(all)
}
