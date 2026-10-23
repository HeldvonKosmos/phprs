//! One request on a worker: route → controller → response (RUNTIME_ARCHITECTURE §9).

use grd_vm::error::Throw;
use grd_vm::value::Val;
use grd_vm::vm::{RunError, Vm};

use crate::http::{HttpRequest, HttpResponse};
use crate::native::response::ResponseData;
use crate::worker::{RequestState, WorkerCtx};

pub fn handle(vm: &mut Vm, req: HttpRequest) -> HttpResponse {
    let resp = handle_inner(vm, req).unwrap_or_else(|r| r);
    if let Some(ctx) = vm.host::<WorkerCtx>() {
        ctx.request = None;
    }
    vm.reset_request();
    resp
}

fn ctx(vm: &mut Vm) -> &mut WorkerCtx {
    vm.host::<WorkerCtx>().expect("worker VM has a WorkerCtx host")
}

fn handle_inner(vm: &mut Vm, req: HttpRequest) -> Result<HttpResponse, HttpResponse> {
    let Some((handler, params)) = ctx(vm).routes.find(&req.method, &req.path) else {
        return Err(error_json(404, "NotFound", format!("no route for {} {}", req.method, req.path), None));
    };
    tracing::debug!(target: "grd", method = %req.method, path = %req.path, class = %handler.class, action = %handler.method, "dispatch");
    ctx(vm).request = Some(RequestState { req, params: params.clone(), object: None, input: None });
    let class = vm
        .lookup_class(handler.class.as_bytes())
        .ok_or_else(|| error_json(500, "Error", format!("controller class `{}` not found", handler.class), None))?;
    let controller = vm.instantiate(class, Vec::new()).map_err(|t| throw_response(vm, t))?;
    let args: Vec<Val> = params.iter().map(|(_, v)| Val::str(v)).collect();
    let result = vm.call_method_sync(controller, handler.method.as_bytes(), args).map_err(|t| throw_response(vm, t))?;
    to_response(vm, result).map_err(|t| throw_response(vm, t))
}

/// Return-value mapping (ROADMAP §3 "Response"): array → JSON, string →
/// HTML, `Response` object → as built, null → echo output or 204.
pub fn to_response(vm: &mut Vm, v: Val) -> Result<HttpResponse, Throw> {
    Ok(match v {
        Val::Object(h) => {
            if let Some(r) = vm.native_payload::<ResponseData>(h) {
                r.clone().into()
            } else {
                let body = grd_vm::json::to_json(vm, &Val::Object(h), false)?;
                HttpResponse::json(200, body)
            }
        }
        Val::Array(_) => HttpResponse::json(200, grd_vm::json::to_json(vm, &v, false)?),
        Val::Null => {
            if vm.output.is_empty() {
                HttpResponse::empty(204)
            } else {
                HttpResponse::html(200, std::mem::take(&mut vm.output))
            }
        }
        Val::Str(s) => HttpResponse::html(200, s.as_bytes().to_vec()),
        other => HttpResponse::text(200, &other.to_php_string()?.to_string_lossy()),
    })
}

fn throw_response(vm: &mut Vm, t: Throw) -> HttpResponse {
    match vm.describe_throw(t) {
        RunError::Uncaught { class, message, line } => {
            tracing::error!(target: "grd", class = %class, line, "uncaught: {message}");
            error_json(500, &class, message, Some(line))
        }
        RunError::Compile(e) => error_json(500, "CompileError", e.to_string(), None),
    }
}

fn error_json(status: u16, class: &str, message: String, line: Option<u32>) -> HttpResponse {
    let mut body = serde_json::json!({ "error": class, "message": message });
    if let Some(l) = line {
        body["line"] = serde_json::json!(l);
    }
    HttpResponse::json(status, body.to_string().into_bytes())
}
