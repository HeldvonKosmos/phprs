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
    let Some((entry, params)) = ctx(vm).routes.find(&req.method, &req.path) else {
        return Err(error_json(404, "NotFound", format!("no route for {} {}", req.method, req.path), None));
    };
    tracing::debug!(target: "grd", method = %req.method, path = %req.path, class = %entry.handler.class, action = %entry.handler.method, "dispatch");
    let middleware = ctx(vm).routes.resolve_middleware(&entry.middleware);
    ctx(vm).request = Some(RequestState { req, handler: entry.handler.clone(), params, object: None, input: None });
    let result = if middleware.is_empty() {
        run_controller(vm).map_err(|t| throw_response(vm, t))?
    } else {
        run_pipeline(vm, &middleware).map_err(|t| throw_response(vm, t))?
    };
    to_response(vm, result).map_err(|t| throw_response(vm, t))
}

/// Instantiate the route's controller and call the action with the path
/// parameters. Also the innermost step of the middleware pipeline
/// (`opengrid_run_controller`).
pub fn run_controller(vm: &mut Vm) -> Result<Val, Throw> {
    let (handler, params) = {
        let s = ctx(vm).request.as_ref().ok_or_else(|| Throw::error("no active request"))?;
        (s.handler.clone(), s.params.clone())
    };
    let class = vm.lookup_class(handler.class.as_bytes()).ok_or_else(|| Throw::error(format!("controller class `{}` not found", handler.class)))?;
    let controller = vm.instantiate(class, Vec::new())?;
    let args: Vec<Val> = params.iter().map(|(_, v)| Val::str(v)).collect();
    vm.call_method_sync(controller, handler.method.as_bytes(), args)
}

fn run_pipeline(vm: &mut Vm, middleware: &[String]) -> Result<Val, Throw> {
    let request = vm.call_value(&Val::str("request"), Vec::new())?;
    let pipeline_class = vm.lookup_class(b"OpenGrid\\Http\\Pipeline").ok_or_else(|| Throw::error("Pipeline class missing"))?;
    let list = Val::array(grd_vm::value::PhpArray::from_values(middleware.iter().map(|m| Val::str(m))));
    let pipeline = vm.instantiate(pipeline_class, vec![list, Val::Int(0)])?;
    vm.call_method_sync(pipeline, b"proceed", vec![request])
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
    if let Some(h) = t.object
        && let Some(resp) = http_exception_response(vm, h)
    {
        return resp;
    }
    match vm.describe_throw(t) {
        RunError::Uncaught { class, message, line } => {
            tracing::error!(target: "grd", class = %class, line, "uncaught: {message}");
            error_json(500, &class, message, Some(line))
        }
        RunError::Compile(e) => error_json(500, "CompileError", e.to_string(), None),
    }
}

/// `HttpException` → its status; `ValidationException` → 422 with errors.
fn http_exception_response(vm: &mut Vm, h: grd_vm::value::ObjHandle) -> Option<HttpResponse> {
    let class = vm.class_of(h);
    let message = |vm: &mut Vm| vm.call_method_sync(h, b"getMessage", Vec::new()).ok().and_then(|m| m.to_php_string().ok()).map(|s| s.to_string_lossy()).unwrap_or_default();
    if let Some(ve) = vm.lookup_class(b"OpenGrid\\Validation\\ValidationException")
        && vm.instance_of(class, ve)
    {
        let errors = vm.call_method_sync(h, b"errors", Vec::new()).ok()?;
        let errors = grd_vm::json::to_json(vm, &errors, false).ok()?;
        let body = format!("{{\"message\":{},\"errors\":{}}}", serde_json::Value::String(message(vm)), String::from_utf8_lossy(&errors));
        return Some(HttpResponse::json(422, body.into_bytes()));
    }
    if let Some(he) = vm.lookup_class(b"OpenGrid\\Http\\HttpException")
        && vm.instance_of(class, he)
    {
        let status = vm.call_method_sync(h, b"getStatusCode", Vec::new()).ok()?.to_int().ok()? as u16;
        let mut msg = message(vm);
        if msg.is_empty() {
            msg = crate::native::reason_phrase(status).to_string();
        }
        let mut resp = error_json(status, "HttpException", msg, None);
        if let Ok(Val::Array(headers)) = vm.call_method_sync(h, b"getHeaders", Vec::new()) {
            for (k, v) in headers.iter() {
                if let (grd_vm::value::ArrayKey::Str(k), Val::Str(v)) = (k, v) {
                    resp.headers.push((k.to_string_lossy(), v.to_string_lossy()));
                }
            }
        }
        return Some(resp);
    }
    None
}

fn error_json(status: u16, class: &str, message: String, line: Option<u32>) -> HttpResponse {
    let mut body = serde_json::json!({ "error": class, "message": message });
    if let Some(l) = line {
        body["line"] = serde_json::json!(l);
    }
    HttpResponse::json(status, body.to_string().into_bytes())
}
