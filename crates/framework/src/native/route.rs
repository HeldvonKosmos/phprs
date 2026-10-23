//! `OpenGrid\Routing\Route`: `Route::get('/tasks', [TaskController::class, 'index'])`.
//! Handlers are `[Class::class, 'method']` pairs; closures would not survive
//! the request boundary (objects die with the request), so they are rejected.

use grd_vm::error::Throw;
use grd_vm::value::Val;
use grd_vm::vm::{Call, NativeClassDef, Vm};

use super::{ctx, string_arg};
use crate::routes::Handler;

pub const CLASS: &str = "OpenGrid\\Routing\\Route";

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .static_method("get", |vm, c| add(vm, c, "GET"))
        .static_method("post", |vm, c| add(vm, c, "POST"))
        .static_method("put", |vm, c| add(vm, c, "PUT"))
        .static_method("patch", |vm, c| add(vm, c, "PATCH"))
        .static_method("delete", |vm, c| add(vm, c, "DELETE"))
        .static_method("any", |vm, c| add(vm, c, "ANY"));
    vm.register_class(def)?;
    Ok(())
}

fn add(vm: &mut Vm, c: Call, method: &str) -> Result<Val, Throw> {
    let path = string_arg(&c, 0, "Route")?;
    let handler = parse_handler(c.arg(1))?;
    ctx(vm).routes.add(method, &path, handler).map_err(Throw::value_error)?;
    Ok(Val::Null)
}

fn parse_handler(v: Val) -> Result<Handler, Throw> {
    let bad = || Throw::type_error("Route handler must be [Controller::class, 'method']");
    let Val::Array(arr) = v else { return Err(bad()) };
    if arr.len() != 2 {
        return Err(bad());
    }
    let mut it = arr.values();
    let (Some(Val::Str(class)), Some(Val::Str(method))) = (it.next(), it.next()) else { return Err(bad()) };
    Ok(Handler { class: class.to_string_lossy(), method: method.to_string_lossy() })
}
