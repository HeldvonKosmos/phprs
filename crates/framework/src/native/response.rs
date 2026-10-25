//! `OpenGrid\Http\Response` and the `response()` helper.

use grd_vm::error::Throw;
use grd_vm::value::Val;
use grd_vm::vm::{Call, NativeClassDef, Vm};

use super::{int_arg_opt, string_arg};
use crate::http::HttpResponse;

pub const CLASS: &str = "OpenGrid\\Http\\Response";

#[derive(Debug, Clone)]
pub struct ResponseData {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Default for ResponseData {
    fn default() -> Self {
        ResponseData { status: 200, headers: Vec::new(), body: Vec::new() }
    }
}

impl From<ResponseData> for HttpResponse {
    fn from(r: ResponseData) -> Self {
        HttpResponse { status: r.status, headers: r.headers, body: r.body }
    }
}

impl ResponseData {
    fn set_header(&mut self, name: &str, value: &str) {
        if let Some(h) = self.headers.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(name)) {
            h.1 = value.to_string();
        } else {
            self.headers.push((name.to_string(), value.to_string()));
        }
    }
}

fn data<'a>(vm: &'a mut Vm, c: &Call) -> Result<&'a mut ResponseData, Throw> {
    let this = c.this()?;
    vm.native_payload_mut::<ResponseData>(this).ok_or_else(|| Throw::error("Response payload missing"))
}

fn new_response(vm: &mut Vm, class: grd_vm::value::ClassId) -> Result<grd_vm::value::ObjHandle, Throw> {
    vm.instantiate(class, Vec::new())
}

fn fill_json(vm: &mut Vm, h: grd_vm::value::ObjHandle, v: &Val, status: u16) -> Result<(), Throw> {
    let body = grd_vm::json::to_json(vm, v, false)?;
    let d = vm.native_payload_mut::<ResponseData>(h).ok_or_else(|| Throw::error("Response payload missing"))?;
    d.body = body;
    d.status = status;
    d.set_header("content-type", "application/json");
    Ok(())
}

fn fill_body(vm: &mut Vm, h: grd_vm::value::ObjHandle, body: &[u8], status: u16, content_type: &str) -> Result<(), Throw> {
    let d = vm.native_payload_mut::<ResponseData>(h).ok_or_else(|| Throw::error("Response payload missing"))?;
    d.body = body.to_vec();
    d.status = status;
    d.set_header("content-type", content_type);
    Ok(())
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .payload(|| Box::new(ResponseData::default()))
        .trace(grd_vm::vm::gc::trace_nothing)
        // static constructors
        .static_method("json", |vm, c| {
            let h = new_response(vm, c.class)?;
            let status = int_arg_opt(&c, 1, 200)? as u16;
            fill_json(vm, h, &c.arg(0), status)?;
            Ok(Val::Object(h))
        })
        .static_method("html", |vm, c| {
            let h = new_response(vm, c.class)?;
            let body = string_arg(&c, 0, "Response::html")?;
            fill_body(vm, h, body.as_bytes(), int_arg_opt(&c, 1, 200)? as u16, "text/html; charset=utf-8")?;
            Ok(Val::Object(h))
        })
        .static_method("text", |vm, c| {
            let h = new_response(vm, c.class)?;
            let body = string_arg(&c, 0, "Response::text")?;
            fill_body(vm, h, body.as_bytes(), int_arg_opt(&c, 1, 200)? as u16, "text/plain; charset=utf-8")?;
            Ok(Val::Object(h))
        })
        .static_method("redirect", |vm, c| {
            let h = new_response(vm, c.class)?;
            let to = string_arg(&c, 0, "Response::redirect")?;
            let d = vm.native_payload_mut::<ResponseData>(h).ok_or_else(|| Throw::error("Response payload missing"))?;
            d.status = int_arg_opt(&c, 1, 302)? as u16;
            d.set_header("location", &to);
            Ok(Val::Object(h))
        })
        .static_method("noContent", |vm, c| {
            let h = new_response(vm, c.class)?;
            vm.native_payload_mut::<ResponseData>(h).ok_or_else(|| Throw::error("Response payload missing"))?.status = 204;
            Ok(Val::Object(h))
        })
        // fluent instance methods
        .method("status", |vm, c| {
            let status = int_arg_opt(&c, 0, 200)? as u16;
            data(vm, &c)?.status = status;
            Ok(Val::Object(c.this()?))
        })
        .method("header", |vm, c| {
            let name = string_arg(&c, 0, "Response::header")?;
            let value = string_arg(&c, 1, "Response::header")?;
            data(vm, &c)?.set_header(&name, &value);
            Ok(Val::Object(c.this()?))
        })
        .method("withHeaders", |vm, c| {
            let Val::Array(arr) = c.arg(0) else { return Err(Throw::type_error("Response::withHeaders expects array")) };
            let pairs: Vec<(String, String)> = arr
                .iter()
                .map(|(k, v)| Ok((k.to_val().to_php_string()?.to_string_lossy(), v.to_php_string()?.to_string_lossy())))
                .collect::<Result<_, Throw>>()?;
            let d = data(vm, &c)?;
            for (k, v) in &pairs {
                d.set_header(k, v);
            }
            Ok(Val::Object(c.this()?))
        })
        .method("cookie", |vm, c| {
            let name = string_arg(&c, 0, "Response::cookie")?;
            let value = string_arg(&c, 1, "Response::cookie")?;
            let minutes = int_arg_opt(&c, 2, 0)?;
            let mut cookie = format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax");
            if minutes > 0 {
                cookie.push_str(&format!("; Max-Age={}", minutes * 60));
            }
            data(vm, &c)?.headers.push(("set-cookie".into(), cookie));
            Ok(Val::Object(c.this()?))
        })
        .method("setContent", |vm, c| {
            let body = string_arg(&c, 0, "Response::setContent")?;
            data(vm, &c)?.body = body.into_bytes();
            Ok(Val::Object(c.this()?))
        })
        .method("getStatusCode", |vm, c| Ok(Val::Int(data(vm, &c)?.status as i64)))
        .method("getContent", |vm, c| Ok(Val::bytes(&data(vm, &c)?.body)));
    vm.register_class(def)?;
    vm.register_native("response", |vm, a| {
        let class = vm.lookup_class(CLASS.as_bytes()).ok_or_else(|| Throw::error("Response class missing"))?;
        let h = new_response(vm, class)?;
        let status = match a.get(1) {
            None | Some(Val::Null) => 200,
            Some(Val::Int(n)) => *n as u16,
            Some(o) => return Err(Throw::type_error(format!("response(): Argument #2 ($status) must be of type int, {} given", o.type_name()))),
        };
        match a.first() {
            None | Some(Val::Null) => {
                vm.native_payload_mut::<ResponseData>(h).ok_or_else(|| Throw::error("Response payload missing"))?.status = status;
            }
            Some(Val::Str(s)) => fill_body(vm, h, s.as_bytes(), status, "text/html; charset=utf-8")?,
            Some(v) => fill_json(vm, h, v, status)?,
        }
        Ok(Val::Object(h))
    });
    Ok(())
}
