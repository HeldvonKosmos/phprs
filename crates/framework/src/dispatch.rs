//! One request on a worker: route → controller → response (RUNTIME_ARCHITECTURE §9).

use grd_vm::error::Throw;
use grd_vm::value::Val;
use grd_vm::vm::{RunError, Vm};

use crate::http::{HttpRequest, HttpResponse};
use crate::native::response::ResponseData;
use crate::worker::{RequestState, WorkerCtx};

pub fn handle(vm: &mut Vm, req: HttpRequest) -> HttpResponse {
    let mut resp = handle_inner(vm, req).unwrap_or_else(|r| r);
    if let Err(t) = crate::session::finish(vm, &mut resp) {
        tracing::error!(target: "opengrid", "session save failed: {}", t.message);
    }
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
    let html = req.header("accept").is_some_and(|a| a.contains("text/html"));
    if let Some(stale) = ctx(vm).stale_error.clone() {
        let info = ErrorInfo { status: 500, class: "CompileError".into(), message: stale, ..Default::default() };
        return Err(render_error(vm, info, html));
    }
    let Some((entry, params)) = ctx(vm).routes.find(&req.method, &req.path) else {
        let info = ErrorInfo { status: 404, class: "NotFound".into(), message: format!("no route for {} {}", req.method, req.path), ..Default::default() };
        return Err(render_error(vm, info, html));
    };
    tracing::debug!(target: "opengrid", method = %req.method, path = %req.path, class = %entry.handler.class, action = %entry.handler.method, "dispatch");
    let middleware = ctx(vm).routes.resolve_middleware(&entry.middleware);
    ctx(vm).request = Some(RequestState { req, handler: entry.handler.clone(), params, object: None, input: None, session: None, user: None });
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
    // I1: constructor injection (interfaces bind to their one implementation)
    let controller = vm.resolve(class)?;
    let args: Vec<Val> = params.iter().map(|(_, v)| Val::str(v)).collect();
    // GC-safe (MEM1b): everything held here is passed into the call
    vm.call_method_sync_gc_safe(controller, handler.method.as_bytes(), args)
}

fn run_pipeline(vm: &mut Vm, middleware: &[String]) -> Result<Val, Throw> {
    let request = vm.call_value(&Val::str("request"), Vec::new())?;
    let pipeline_class = vm.lookup_class(b"OpenGrid\\Http\\Pipeline").ok_or_else(|| Throw::error("Pipeline class missing"))?;
    let list = Val::array(grd_vm::value::PhpArray::from_values(middleware.iter().map(|m| Val::str(m))));
    let pipeline = vm.instantiate(pipeline_class, vec![list, Val::Int(0)])?;
    vm.call_method_sync_gc_safe(pipeline, b"proceed", vec![request])
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
    let html = wants_html(vm);
    let (file, trace) = match t.object {
        Some(h) => (
            vm.call_method_sync(h, b"getFile", Vec::new()).ok().and_then(|v| v.to_php_string().ok()).map(|s| s.to_string_lossy()),
            vm.call_method_sync(h, b"getTraceAsString", Vec::new()).ok().and_then(|v| v.to_php_string().ok()).map(|s| s.to_string_lossy()),
        ),
        None => (None, None),
    };
    match vm.describe_throw(t) {
        RunError::Uncaught { class, message, line } => {
            tracing::error!(target: "opengrid", class = %class, line, "uncaught: {message}");
            render_error(vm, ErrorInfo { status: 500, class, message, file, line: Some(line), trace }, html)
        }
        RunError::Compile(e) => render_error(vm, ErrorInfo { status: 500, class: "CompileError".into(), message: e.to_string(), ..Default::default() }, html),
    }
}

fn wants_html(vm: &mut Vm) -> bool {
    ctx(vm).request.as_ref().and_then(|s| s.req.header("accept")).is_some_and(|a| a.contains("text/html"))
}

#[derive(Default)]
pub struct ErrorInfo {
    pub status: u16,
    pub class: String,
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub trace: Option<String>,
}

/// JSON for API clients; an HTML page for browsers. In debug mode both carry
/// file, line, trace and (HTML) a source excerpt and the last log lines
/// (RUNTIME_ARCHITECTURE §10 "Fehlerseite").
pub fn render_error(vm: &mut Vm, info: ErrorInfo, html: bool) -> HttpResponse {
    let debug = ctx(vm).debug;
    if !html {
        let mut body = serde_json::json!({ "error": info.class, "message": info.message });
        if let Some(l) = info.line {
            body["line"] = serde_json::json!(l);
        }
        if debug {
            if let Some(f) = &info.file {
                body["file"] = serde_json::json!(f);
            }
            if let Some(t) = &info.trace {
                body["trace"] = serde_json::json!(t);
            }
        }
        return HttpResponse::json(info.status, body.to_string().into_bytes());
    }
    let title = format!("{} | {}", info.status, crate::native::reason_phrase(info.status));
    let mut page = String::new();
    page.push_str("<!doctype html><html><head><meta charset=\"utf-8\"><title>");
    page.push_str(&esc(&title));
    page.push_str("</title><style>body{font:15px/1.5 system-ui,sans-serif;margin:0;background:#f6f6f7;color:#222}header{background:#b3261e;color:#fff;padding:20px 32px}header h1{margin:0;font-size:22px}header p{margin:6px 0 0;opacity:.9}main{padding:24px 32px}section{background:#fff;border:1px solid #ddd;border-radius:6px;padding:16px 20px;margin-bottom:20px}h2{font-size:14px;text-transform:uppercase;letter-spacing:.04em;color:#666;margin:0 0 10px}pre{margin:0;overflow:auto;font:13px/1.45 ui-monospace,monospace}.src .hl{background:#fde8e6;display:block}.src span{display:block}.ln{color:#999;display:inline-block;width:3.5em;text-align:right;margin-right:1em;user-select:none}</style></head><body><header><h1>");
    page.push_str(&esc(&title));
    page.push_str("</h1><p>");
    page.push_str(&esc(&info.class));
    if !info.message.is_empty() {
        page.push_str(": ");
        page.push_str(&esc(&info.message));
    }
    page.push_str("</p></header><main>");
    if debug {
        if let Some(file) = &info.file {
            page.push_str("<section><h2>Location</h2><pre>");
            page.push_str(&esc(file));
            if let Some(l) = info.line {
                page.push_str(&format!(":{l}"));
            }
            page.push_str("</pre></section>");
            if let Some(l) = info.line
                && let Ok(src) = std::fs::read_to_string(file)
            {
                page.push_str("<section><h2>Source</h2><pre class=\"src\">");
                let l = l as usize;
                for (i, text) in src.lines().enumerate() {
                    let n = i + 1;
                    if n + 6 < l || n > l + 6 {
                        continue;
                    }
                    page.push_str(if n == l { "<span class=\"hl\">" } else { "<span>" });
                    page.push_str(&format!("<i class=\"ln\">{n}</i>{}", esc(text)));
                    page.push_str("</span>");
                }
                page.push_str("</pre></section>");
            }
        }
        if info.file.is_none() && !info.message.contains('\n') {
            // compile errors carry their location in the message; nothing more to show
        }
        if let Some(t) = &info.trace {
            page.push_str("<section><h2>Trace</h2><pre>");
            page.push_str(&esc(t));
            page.push_str("</pre></section>");
        }
        let logs: Vec<String> = ctx(vm).logs.iter().rev().take(20).map(|(lvl, line)| format!("[{lvl}] {line}")).collect();
        if !logs.is_empty() {
            page.push_str("<section><h2>Recent log</h2><pre>");
            page.push_str(&esc(&logs.join("\n")));
            page.push_str("</pre></section>");
        }
    } else {
        page.push_str("<section><p>Something went wrong. The error has been logged.</p></section>");
    }
    page.push_str("</main></body></html>");
    HttpResponse::html(info.status, page.into_bytes())
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `HttpException` → its status; `ValidationException` → 422 with errors;
/// `ModelNotFoundException` → 404.
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
    if let Some(mn) = vm.lookup_class(b"OpenGrid\\Database\\ModelNotFoundException")
        && vm.instance_of(class, mn)
    {
        let msg = message(vm);
        let html = wants_html(vm);
        return Some(if html {
            render_error(vm, ErrorInfo { status: 404, class: "ModelNotFoundException".into(), message: msg, ..Default::default() }, true)
        } else {
            error_json(404, "ModelNotFoundException", msg, None)
        });
    }
    if let Some(he) = vm.lookup_class(b"OpenGrid\\Http\\HttpException")
        && vm.instance_of(class, he)
    {
        let status = vm.call_method_sync(h, b"getStatusCode", Vec::new()).ok()?.to_int().ok()? as u16;
        let mut msg = message(vm);
        if msg.is_empty() {
            msg = crate::native::reason_phrase(status).to_string();
        }
        let html = wants_html(vm);
        let mut resp = if html {
            render_error(vm, ErrorInfo { status, class: "HttpException".into(), message: msg, ..Default::default() }, true)
        } else {
            error_json(status, "HttpException", msg, None)
        };
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
