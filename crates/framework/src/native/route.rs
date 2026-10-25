//! `OpenGrid\Routing\Route`: `Route::get('/tasks', [TaskController::class, 'index'])
//! ->middleware(['auth'])->name('tasks.index')`, `Route::group([...], fn)`,
//! `Route::aliasMiddleware('auth', Auth::class)`. Handlers are
//! `[Class::class, 'method']` pairs; closures would not survive the request
//! boundary (objects die with the request), so they are rejected.

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, Val};
use grd_vm::vm::{Call, NativeClassDef, Vm};

use super::{ctx, string_arg};
use crate::routes::{Group, Handler};

pub const CLASS: &str = "OpenGrid\\Routing\\Route";

/// Payload of a `Route` instance: the entry it refers to.
#[derive(Default)]
pub struct RouteRef {
    pub index: Option<usize>,
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .payload(|| Box::new(RouteRef::default()))
        .trace(grd_vm::vm::gc::trace_nothing)
        .static_method("get", |vm, c| add(vm, c, "GET"))
        .static_method("post", |vm, c| add(vm, c, "POST"))
        .static_method("put", |vm, c| add(vm, c, "PUT"))
        .static_method("patch", |vm, c| add(vm, c, "PATCH"))
        .static_method("delete", |vm, c| add(vm, c, "DELETE"))
        .static_method("any", |vm, c| add(vm, c, "ANY"))
        .static_method("aliasMiddleware", |vm, c| {
            let name = string_arg(&c, 0, "Route::aliasMiddleware")?;
            let class = string_arg(&c, 1, "Route::aliasMiddleware")?;
            // MW1 (D1): aliases are for the framework's own middleware; a project
            // names its middleware class in the route. Deprecated, warned in development.
            if ctx(vm).debug && ctx(vm).routes.aliases.get(&name) != Some(&class) {
                tracing::warn!(target: "opengrid", alias = %name, class = %class, "Route::aliasMiddleware is deprecated for project middleware: put the class in the route ('middleware' => [{}::class])", class.rsplit('\\').next().unwrap_or(&class));
            }
            ctx(vm).routes.aliases.insert(name, class);
            Ok(Val::Null)
        })
        .static_method("group", |vm, c| {
            let mut group = Group::default();
            if let Val::Array(attrs) = c.arg(0) {
                if let Some(p) = attrs.get(&ArrayKey::from_str("prefix")) {
                    group.prefix = p.to_php_string()?.to_string_lossy();
                }
                if let Some(m) = attrs.get(&ArrayKey::from_str("middleware")) {
                    group.middleware = string_list(m)?;
                }
            }
            ctx(vm).routes.groups.push(group);
            let result = vm.call_value(&c.arg(1), Vec::new());
            ctx(vm).routes.groups.pop();
            result.map(|_| Val::Null)
        })
        .method("middleware", |vm, c| {
            let this = c.this()?;
            let mut names = Vec::new();
            for a in &c.args {
                names.extend(string_list(a)?);
            }
            let index = vm.native_payload::<RouteRef>(this).and_then(|r| r.index).ok_or_else(|| Throw::error("Route object without entry"))?;
            if let Some(e) = ctx(vm).routes.entry_mut(index) {
                e.middleware.extend(names);
            }
            Ok(Val::Object(this))
        })
        .method("name", |vm, c| {
            let this = c.this()?;
            let name = string_arg(&c, 0, "Route::name")?;
            let index = vm.native_payload::<RouteRef>(this).and_then(|r| r.index).ok_or_else(|| Throw::error("Route object without entry"))?;
            if let Some(e) = ctx(vm).routes.entry_mut(index) {
                e.name = Some(name);
            }
            Ok(Val::Object(this))
        });
    vm.register_class(def)?;
    Ok(())
}

fn string_list(v: &Val) -> Result<Vec<String>, Throw> {
    match v {
        Val::Str(s) => Ok(vec![s.to_string_lossy()]),
        Val::Array(a) => a.values().map(|x| Ok(x.to_php_string()?.to_string_lossy())).collect(),
        other => Err(Throw::type_error(format!("expected string or array of strings, {} given", other.type_name()))),
    }
}

fn add(vm: &mut Vm, c: Call, method: &str) -> Result<Val, Throw> {
    let path = string_arg(&c, 0, "Route")?;
    let handler = parse_handler(c.arg(1))?;
    let index = ctx(vm).routes.add(method, &path, handler).map_err(Throw::value_error)?;
    let h = vm.instantiate(c.class, Vec::new())?;
    if let Some(r) = vm.native_payload_mut::<RouteRef>(h) {
        r.index = Some(index);
    }
    Ok(Val::Object(h))
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
