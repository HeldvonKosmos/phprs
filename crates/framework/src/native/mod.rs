//! Native classes of the dialect (NATIVE_CLASSES.md). Each file registers one
//! class or helper family; all state lives in the `WorkerCtx` host.

pub mod config;
pub mod database;
pub mod log;
pub mod request;
pub mod response;
pub mod route;
pub mod validation;

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::{Call, Vm};

use crate::worker::WorkerCtx;

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    route::register(vm)?;
    request::register(vm)?;
    response::register(vm)?;
    log::register(vm)?;
    config::register(vm)?;
    database::register(vm)?;
    validation::register(vm)?;
    vm.register_native("opengrid_run_controller", |vm, _| crate::dispatch::run_controller(vm));
    // abort(404, 'message') → HttpException
    vm.register_native("abort", |vm, a| {
        let status = match a.first() {
            Some(v) => v.to_int()?,
            None => return Err(Throw::argument_count("abort(): status missing")),
        };
        let message = match a.get(1) {
            Some(Val::Str(s)) => s.to_string_lossy(),
            _ => reason_phrase(status as u16).to_string(),
        };
        let headers = a.get(2).cloned().unwrap_or(Val::empty_array());
        let class = vm.lookup_class(b"OpenGrid\\Http\\HttpException").ok_or_else(|| Throw::error("HttpException class missing"))?;
        let h = vm.instantiate(class, vec![Val::Int(status), Val::str(&message), headers])?;
        Err(Throw::existing(h))
    });
    Ok(())
}

pub(crate) fn ctx(vm: &mut Vm) -> &mut WorkerCtx {
    vm.host::<WorkerCtx>().expect("worker VM has a WorkerCtx host")
}

pub(crate) fn string_arg(c: &Call, i: usize, what: &str) -> Result<String, Throw> {
    match c.args.get(i) {
        Some(Val::Str(s)) => Ok(s.to_string_lossy()),
        Some(other) => Err(Throw::type_error(format!("{what}: argument {} must be string, {} given", i + 1, other.type_name()))),
        None => Err(Throw::argument_count(format!("{what}: argument {} missing", i + 1))),
    }
}

pub(crate) fn int_arg_opt(c: &Call, i: usize, default: i64) -> Result<i64, Throw> {
    match c.args.get(i) {
        None | Some(Val::Null) => Ok(default),
        Some(v) => v.to_int(),
    }
}

pub(crate) fn pairs_to_array(pairs: &[(String, String)]) -> Val {
    let mut arr = PhpArray::with_capacity(pairs.len());
    for (k, v) in pairs {
        arr.insert(ArrayKey::from_str(k), Val::str(v));
    }
    Val::array(arr)
}

pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        419 => "Page Expired",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}
