//! `OpenGrid\Support\Session` and `session()`/`csrf_token()` helpers.

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, Val};
use grd_vm::vm::{NativeClassDef, Vm};

use super::string_arg;
use crate::session::{ensure, new_id, token};

pub const CLASS: &str = "OpenGrid\\Support\\Session";

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .static_method("get", |vm, c| {
            let key = string_arg(&c, 0, "Session::get")?;
            let s = ensure(vm)?;
            Ok(s.data.get(&ArrayKey::from_str(&key)).cloned().unwrap_or_else(|| c.arg(1)))
        })
        .static_method("has", |vm, c| {
            let key = string_arg(&c, 0, "Session::has")?;
            let s = ensure(vm)?;
            Ok(Val::Bool(s.data.get(&ArrayKey::from_str(&key)).is_some_and(|v| !v.is_null())))
        })
        .static_method("put", |vm, c| {
            let key = string_arg(&c, 0, "Session::put")?;
            let s = ensure(vm)?;
            s.data.insert(ArrayKey::from_str(&key), c.arg(1));
            s.dirty = true;
            Ok(Val::Null)
        })
        .static_method("flash", |vm, c| {
            let key = string_arg(&c, 0, "Session::flash")?;
            let s = ensure(vm)?;
            s.data.insert(ArrayKey::from_str(&key), c.arg(1));
            if !s.flash_new.contains(&key) {
                s.flash_new.push(key);
            }
            s.dirty = true;
            Ok(Val::Null)
        })
        .static_method("reflash", |vm, _| {
            let s = ensure(vm)?;
            let old = s.flash_old.clone();
            for k in old {
                if !s.flash_new.contains(&k) {
                    s.flash_new.push(k);
                }
            }
            s.dirty = true;
            Ok(Val::Null)
        })
        .static_method("pull", |vm, c| {
            let key = string_arg(&c, 0, "Session::pull")?;
            let s = ensure(vm)?;
            let v = s.data.remove(&ArrayKey::from_str(&key)).unwrap_or_else(|| c.arg(1));
            s.dirty = true;
            Ok(v)
        })
        .static_method("forget", |vm, c| {
            let s = ensure(vm)?;
            for a in &c.args {
                if let Val::Str(k) = a {
                    s.data.remove(&ArrayKey::from_str(&k.to_string_lossy()));
                }
            }
            s.dirty = true;
            Ok(Val::Null)
        })
        .static_method("all", |vm, _| {
            let s = ensure(vm)?;
            Ok(Val::array(s.data.clone()))
        })
        .static_method("flush", |vm, _| {
            let s = ensure(vm)?;
            s.data.clear();
            s.flash_new.clear();
            s.dirty = true;
            Ok(Val::Null)
        })
        .static_method("invalidate", |vm, _| {
            let s = ensure(vm)?;
            s.destroyed = true;
            Ok(Val::Null)
        })
        .static_method("regenerate", |vm, _| {
            let s = ensure(vm)?;
            s.id = new_id();
            s.is_new = true;
            s.dirty = true;
            Ok(Val::str(&s.id))
        })
        .static_method("getId", |vm, _| Ok(Val::str(&ensure(vm)?.id)))
        .static_method("token", |vm, _| Ok(Val::str(&token(vm)?)));
    vm.register_class(def)?;
    vm.register_native("csrf_token", |vm, _| Ok(Val::str(&token(vm)?)));
    vm.register_native("session", |vm, a| match a.first() {
        None => {
            let s = ensure(vm)?;
            Ok(Val::array(s.data.clone()))
        }
        Some(Val::Str(k)) => {
            let key = k.to_string_lossy();
            let s = ensure(vm)?;
            Ok(s.data.get(&ArrayKey::from_str(&key)).cloned().unwrap_or_else(|| a.get(1).cloned().unwrap_or(Val::Null)))
        }
        Some(Val::Array(pairs)) => {
            let pairs = pairs.clone();
            let s = ensure(vm)?;
            for (k, v) in pairs.iter() {
                s.data.insert(k.clone(), v.clone());
            }
            s.dirty = true;
            Ok(Val::Null)
        }
        Some(other) => Err(Throw::type_error(format!("session(): string or array expected, {} given", other.type_name()))),
    });
    Ok(())
}
