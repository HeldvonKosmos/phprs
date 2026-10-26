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
            pick(nested_array(&pairs), &c)
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
        .method("cookie", |vm, c| {
            let name = string_arg(&c, 0, "Request::cookie")?;
            let found = state(vm)?.req.header("cookie").and_then(|h| crate::session::parse_cookies(h).into_iter().find(|(k, _)| *k == name).map(|(_, v)| v));
            Ok(found.map(|v| Val::str(&v)).unwrap_or_else(|| c.arg(1)))
        })
        .method("cookies", |vm, _| {
            let pairs = state(vm)?.req.header("cookie").map(crate::session::parse_cookies).unwrap_or_default();
            Ok(pairs_to_array(&pairs))
        })
        // IP1: settings.opengrid.proxy.ip_header (default X-Forwarded-For, Traefik; false = off)
        .method("ip", |vm, _| {
            let header = match ctx(vm).settings.pointer("/opengrid/proxy/ip_header") {
                None => Some("X-Forwarded-For".to_string()),
                Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
                Some(_) => None,
            };
            Ok(state(vm)?.req.client_ip(header.as_deref()).map_or(Val::Null, |ip| Val::str(&ip)))
        })
        // AU1: the authenticated user of this request (Laravel's `$request->user()`)
        .method("user", |vm, _| Ok(state(vm)?.user.clone().unwrap_or(Val::Null)))
        .method("setUser", |vm, c| {
            let u = c.arg(0);
            state(vm)?.user = if u.is_null() { None } else { Some(u) };
            Ok(Val::Null)
        })
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
    let Val::Array(base) = nested_array(&query) else { unreachable!("nested_array returns an array") };
    let mut arr = (*base).clone();
    if json {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_slice::<serde_json::Value>(&body) {
            for (k, v) in map {
                let val = grd_vm::json::from_json(vm, v)?;
                arr.insert(ArrayKey::from_str(&k), val);
            }
        }
    } else if !body.is_empty() {
        let pairs = parse_query(&String::from_utf8_lossy(&body));
        if let Val::Array(form) = nested_array(&pairs) {
            for (k, v) in form.iter() {
                arr.insert(k.clone(), v.clone());
            }
        }
    }
    let all = Val::array(arr);
    state(vm)?.input = Some(all.clone());
    Ok(all)
}

/// `filters[done]=1&tags[]=a&tags[]=b&a[b][c]=x` → nested arrays, like PHP's
/// `$_GET` (the same rules as the dialect's `parse_str`).
pub fn nested_array(pairs: &[(String, String)]) -> Val {
    let mut root = PhpArray::new();
    for (key, value) in pairs {
        grd_vm::builtins::insert_query_key(&mut root, key, Val::str(value));
    }
    Val::array(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_query_keys() {
        let pairs = vec![("filters[done]".to_string(), "1".to_string()), ("tags[]".to_string(), "a".to_string()), ("tags[]".to_string(), "b".to_string()), ("sort".to_string(), "x".to_string())];
        let Val::Array(a) = nested_array(&pairs) else { panic!() };
        let Some(Val::Array(f)) = a.get(&ArrayKey::from_str("filters")) else { panic!("filters") };
        assert!(matches!(f.get(&ArrayKey::from_str("done")), Some(Val::Str(_))));
        let Some(Val::Array(t)) = a.get(&ArrayKey::from_str("tags")) else { panic!("tags") };
        assert_eq!(t.len(), 2);
        assert!(matches!(a.get(&ArrayKey::from_str("sort")), Some(Val::Str(_))));
    }
}
