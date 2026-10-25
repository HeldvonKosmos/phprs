//! MS1 mail natives behind `OpenGrid\Mail\Mail` (prelude_mail.grd):
//! `opengrid_mail_render($view, $data)` renders a mail template with Tera,
//! `opengrid_mail_send($message)` sends through the pool's services,
//! `opengrid_mail_sent()` lists what was sent (the outbox).
//!
//! A view `welcome` is the directory `modules/<module>/resources/mail/welcome/`
//! with `subject.t`, `html.t` and `text.t` (Loco's layout); `auth::welcome`
//! names the module, otherwise the first module that has it wins.

use grd_vm::error::Throw;
use grd_vm::value::{PhpArray, Val};
use grd_vm::vm::Vm;
use loco_rs::mailer::MultiEmail;

use super::ctx;

fn to_json(vm: &mut Vm, v: &Val) -> Result<serde_json::Value, Throw> {
    let bytes = grd_vm::json::to_json(vm, v, false)?;
    serde_json::from_slice(&bytes).map_err(|e| Throw::error(format!("mail data: {e}")))
}

fn template_dir(root: &std::path::Path, view: &str) -> Result<std::path::PathBuf, Throw> {
    let (module, name) = match view.split_once("::") {
        Some((m, n)) => (Some(m), n),
        None => (None, view),
    };
    let valid = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') && !s.contains("..");
    if !valid(name) || module.is_some_and(|m| !valid(m)) {
        return Err(Throw::value_error(format!("invalid mail view `{view}`")));
    }
    let modules = root.join("modules");
    let candidates: Vec<std::path::PathBuf> = match module {
        Some(m) => vec![modules.join(m)],
        None => {
            let mut dirs: Vec<_> = std::fs::read_dir(&modules).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            dirs.sort();
            dirs
        }
    };
    candidates
        .into_iter()
        .map(|m| m.join("resources/mail").join(name))
        .find(|d| d.is_dir())
        .ok_or_else(|| Throw::error(format!("mail view `{view}` not found (modules/<module>/resources/mail/{name}/)")))
}

pub fn register(vm: &mut Vm) {
    // [subject, html, text]; a missing file renders as ""
    vm.register_native("opengrid_mail_render", |vm, a| {
        let view = match a.first() {
            Some(Val::Str(s)) => s.to_string_lossy(),
            _ => return Err(Throw::type_error("opengrid_mail_render(): view name expected")),
        };
        let data = to_json(vm, &a.get(1).cloned().unwrap_or(Val::empty_array()))?;
        let data = if data.is_array() && data.as_array().is_some_and(|x| x.is_empty()) { serde_json::json!({}) } else { data };
        let dir = template_dir(&ctx(vm).services.root, &view)?;
        let context = tera::Context::from_serialize(&data).map_err(|e| Throw::error(format!("mail data: {e}")))?;
        let mut out = PhpArray::new();
        for (file, escape) in [("subject.t", false), ("html.t", true), ("text.t", false)] {
            let rendered = match std::fs::read_to_string(dir.join(file)) {
                Ok(src) => tera::Tera::one_off(&src, &context, escape).map_err(|e| Throw::error(format!("mail view `{view}` {file}: {e}")))?,
                Err(_) => String::new(),
            };
            out.push(Val::str(rendered.trim_end_matches('\n')));
        }
        Ok(Val::array(out))
    });
    vm.register_native("opengrid_mail_send", |vm, a| {
        let msg = to_json(vm, &a.first().cloned().unwrap_or(Val::Null))?;
        let list = |k: &str| -> Vec<String> { msg.get(k).and_then(|v| v.as_array()).map(|x| x.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default() };
        let text = |k: &str| msg.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let to = list("to");
        if to.is_empty() {
            return Err(Throw::value_error("mail without recipient"));
        }
        let services = ctx(vm).services.clone();
        let email = MultiEmail {
            from: text("from").or_else(|| services.mail_from.clone()),
            to,
            reply_to: text("reply_to"),
            subject: text("subject").unwrap_or_default(),
            text: text("text").unwrap_or_default(),
            html: text("html").unwrap_or_default(),
            bcc: list("bcc"),
            cc: list("cc"),
            headers: None,
        };
        services.send_mail(email).map_err(Throw::error)?;
        Ok(Val::Null)
    });
    vm.register_native("opengrid_mail_sent", |vm, _| {
        let sent = ctx(vm).services.sent_mails();
        let mut out = PhpArray::new();
        for m in sent {
            out.push(grd_vm::json::from_json(vm, m)?);
        }
        Ok(Val::array(out))
    });
}
