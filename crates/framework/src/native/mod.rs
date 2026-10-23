//! Native classes of the dialect (NATIVE_CLASSES.md). Each file registers one
//! class or helper family; all state lives in the `WorkerCtx` host.

pub mod config;
pub mod log;
pub mod request;
pub mod response;
pub mod route;

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
