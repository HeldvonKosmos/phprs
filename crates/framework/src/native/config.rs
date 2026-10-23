//! `OpenGrid\Support\Config` plus `config()` and `env()`: a read-only
//! snapshot of the `settings:` block of Loco's YAML (ROADMAP §3 "Config").

use grd_vm::error::Throw;
use grd_vm::value::Val;
use grd_vm::vm::{NativeClassDef, Vm};

use super::{ctx, string_arg};

pub const CLASS: &str = "OpenGrid\\Support\\Config";

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .static_method("get", |vm, c| {
            let key = string_arg(&c, 0, "Config::get")?;
            lookup(vm, &key, c.arg(1))
        })
        .static_method("has", |vm, c| {
            let key = string_arg(&c, 0, "Config::has")?;
            Ok(Val::Bool(dig(&ctx(vm).settings, &key).is_some()))
        })
        .static_method("all", |vm, _| {
            let all = ctx(vm).settings.clone();
            grd_vm::json::from_json(vm, all)
        });
    vm.register_class(def)?;
    vm.register_native("config", |vm, a| {
        let key = match a.first() {
            Some(Val::Str(s)) => s.to_string_lossy(),
            _ => return Err(Throw::type_error("config(): argument 1 must be string")),
        };
        lookup(vm, &key, a.get(1).cloned().unwrap_or(Val::Null))
    });
    vm.register_native("env", |_, a| {
        let key = match a.first() {
            Some(Val::Str(s)) => s.to_string_lossy(),
            _ => return Err(Throw::type_error("env(): argument 1 must be string")),
        };
        Ok(match std::env::var(&key) {
            Ok(v) => Val::str(&v),
            Err(_) => a.get(1).cloned().unwrap_or(Val::Null),
        })
    });
    Ok(())
}

fn lookup(vm: &mut Vm, key: &str, default: Val) -> Result<Val, Throw> {
    match dig(&ctx(vm).settings, key).cloned() {
        Some(v) => grd_vm::json::from_json(vm, v),
        None => Ok(default),
    }
}

/// `app.name` → settings["app"]["name"].
fn dig<'a>(root: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    let mut cur = root;
    for part in key.split('.') {
        cur = match cur {
            serde_json::Value::Object(map) => map.get(part)?,
            serde_json::Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}
