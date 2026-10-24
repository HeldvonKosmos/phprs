//! `OpenGrid\Support\Log`: `Log::info($msg, $context)` → `tracing` with target `grd`.

use grd_vm::error::Throw;
use grd_vm::value::Val;
use grd_vm::vm::{Call, NativeClassDef, Vm};

use super::{ctx, string_arg};

pub const CLASS: &str = "OpenGrid\\Support\\Log";
const KEEP: usize = 200;

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .static_method("debug", |vm, c| emit(vm, c, "debug"))
        .static_method("info", |vm, c| emit(vm, c, "info"))
        .static_method("warning", |vm, c| emit(vm, c, "warning"))
        .static_method("error", |vm, c| emit(vm, c, "error"));
    vm.register_class(def)?;
    Ok(())
}

fn emit(vm: &mut Vm, c: Call, level: &str) -> Result<Val, Throw> {
    let msg = string_arg(&c, 0, "Log")?;
    let context = match c.args.get(1) {
        None | Some(Val::Null) => String::new(),
        Some(v) => String::from_utf8_lossy(&grd_vm::json::to_json(vm, v, false)?).into_owned(),
    };
    match level {
        "debug" => tracing::debug!(target: "opengrid", context = %context, "{msg}"),
        "info" => tracing::info!(target: "opengrid", context = %context, "{msg}"),
        "warning" => tracing::warn!(target: "opengrid", context = %context, "{msg}"),
        _ => tracing::error!(target: "opengrid", context = %context, "{msg}"),
    }
    let logs = &mut ctx(vm).logs;
    logs.push((level.to_string(), if context.is_empty() { msg } else { format!("{msg} {context}") }));
    if logs.len() > KEEP {
        logs.remove(0);
    }
    Ok(Val::Null)
}
