//! J1: the control socket `<project>/.opengrid/control.sock` (D5: one
//! process). A running server listens on it; `opengrid task …` sends the
//! task there and prints what comes back, so tasks run inside the server
//! (the embedded database allows one process only). Without a server the
//! task runs on its own.
//!
//! Protocol: one JSON line per request, JSON lines back.
//!   `{"cmd":"task","name":"migrate_push","args":{"force":"true"}}`
//!   `{"cmd":"status"}`
//! Answers: `{"out":"…"}` (output, once) and last `{"ok":true}` or
//! `{"ok":false,"error":"…"}`; status answers `{"ok":true,"pid":…,"workers":…,"generation":…}`.
//! The socket file is created with mode 0600 (owner only).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::native::database::schema::kit::{self, SyncOptions};
use crate::native::database::schema::ops as schema_ops;
use crate::worker::Pool;

pub fn socket_path(root: &Path) -> PathBuf {
    root.join(".opengrid/control.sock")
}

/// Run a task to completion on this pool (blocking): the schema tasks of R4d
/// and dialect tasks (`run task:<module>:<name>`). The output on success,
/// output and error otherwise.
pub fn run_task(pool: &Arc<Pool>, name: &str, args: &BTreeMap<String, String>) -> Result<String, String> {
    match name {
        "migrate_push" | "migrate_pull" | "migrate_fresh" | "rollout" => schema_task(pool, name, args),
        "module" => module_task(pool.options(), args, Some(pool)),
        "queue_failed" | "queue_retry" | "queue_flush" => crate::native::jobs::queue_task(pool, name, args),
        "run" => {
            let sig = args.get("task").ok_or("run needs task:<module>:<name>")?;
            grd_task(pool, sig, args)
        }
        other => Err(format!("unknown task `{other}`")),
    }
}

/// PKG1: `module cmd:add url:<git> [rev:<tag|branch|commit>]`, `cmd:remove
/// name:<m>`, `cmd:list`, `cmd:verify`. After `add` the whole project must
/// still compile and link (GRD check), otherwise the module is taken out
/// again; a running server (`pool`) reloads its workers.
pub fn module_task(options: &crate::worker::PoolOptions, args: &BTreeMap<String, String>, pool: Option<&Arc<Pool>>) -> Result<String, String> {
    let root = &options.root;
    match args.get("cmd").map(String::as_str).unwrap_or("list") {
        "add" => {
            let url = args.get("url").ok_or("module cmd:add needs url:<git repository>")?;
            let added = opengrid_pkg::add(root, url, args.get("rev").map(String::as_str))?;
            if let Err(e) = crate::worker::check_project(options) {
                let _ = opengrid_pkg::remove(root, &added.locked.name);
                return Err(format!("module `{}` does not pass the check and was removed again: {e}", added.locked.name));
            }
            if let Some(p) = pool {
                p.reload();
            }
            Ok(format!("added {} {} ({} @ {})", added.locked.name, added.locked.version, added.locked.source, &added.locked.commit[..12.min(added.locked.commit.len())]))
        }
        "remove" => {
            let name = args.get("name").ok_or("module cmd:remove needs name:<module>")?;
            opengrid_pkg::remove(root, name)?;
            if let Some(p) = pool {
                p.reload();
            }
            Ok(format!("removed {name}"))
        }
        "list" => {
            let lock = opengrid_pkg::Lock::read(root)?;
            let mut out = String::new();
            for m in lock.modules {
                out.push_str(&format!("{} {} {} @ {}\n", m.name, m.version, m.source, &m.commit[..12.min(m.commit.len())]));
            }
            Ok(out)
        }
        "verify" => {
            let changed = opengrid_pkg::verify(root)?;
            if changed.is_empty() {
                Ok("all installed modules match modules.lock".into())
            } else {
                Err(changed.iter().map(|(n, r)| format!("{n}: {r}")).collect::<Vec<_>>().join("\n"))
            }
        }
        other => Err(format!("unknown module cmd `{other}` (add, remove, list, verify)")),
    }
}

fn flag(args: &BTreeMap<String, String>, key: &str) -> bool {
    args.get(key).is_some_and(|v| matches!(v.as_str(), "true" | "yes" | "1"))
}

fn schema_task(pool: &Arc<Pool>, name: &str, args: &BTreeMap<String, String>) -> Result<String, String> {
    let db = pool.db().ok_or("no database configured (settings.opengrid.database or SURREAL_DB_HOST)")?;
    let root = pool.options().root.clone();
    let database = db.config.database.clone();
    let (force, keep_db) = (flag(args, "force"), flag(args, "keep_db"));
    let cmd = args.get("cmd").cloned().unwrap_or_default();
    let (rname, id) = (args.get("name").cloned(), args.get("id").cloned());
    if name == "migrate_fresh" && !force {
        return Err("migrate_fresh deletes all data; run it with force:true".into());
    }
    db.block_on(async {
        let c = db.client();
        match name {
            "migrate_push" => schema_ops::push(c, &root, SyncOptions::default()).await.map(|(_, w)| {
                // R4h: every skipped statement with its file, not only the count
                let mut out = format!("schema in sync ({} skipped statements)", w.len());
                for x in &w {
                    out.push_str(&format!("\n  skipped in {}: {}", x.source, x.message));
                }
                out
            }),
            "migrate_pull" => schema_ops::pull(c).await,
            "migrate_fresh" => schema_ops::fresh(c, &root, &database, keep_db).await.map(|_| "database emptied, schema pushed".to_string()),
            _ => match cmd.as_str() {
                "baseline" => kit::rollout_baseline(c, &root).await.map(|_| "baseline recorded".to_string()),
                "plan" => {
                    let n = rname.ok_or_else(|| kit::KitError("rollout cmd:plan needs name:<name>".into()))?;
                    schema_ops::plan(c, &root, &n).await.map(|(id, r)| format!("planned {id}\n{}", r.summary()))
                }
                "start" => kit::rollout_start(c, &root, id.as_deref()).await.map(|_| "rollout started".to_string()),
                "complete" => kit::rollout_complete(c, &root, id.as_deref()).await.map(|_| "rollout completed".to_string()),
                "rollback" => kit::rollout_rollback(c, &root, id.as_deref()).await.map(|_| "rollout rolled back".to_string()),
                "status" => kit::rollout_status(c, &root).await,
                other => Err(kit::KitError(format!("unknown rollout cmd `{other}` (baseline, plan, start, complete, rollback, status)"))),
            },
        }
    })
    .map_err(|e| e.0)
}

/// `notes:prune` → `Notes\Tasks\Prune`, `users:send-digest` → `Users\Tasks\SendDigest`
/// (convention instead of registration, D1).
pub fn task_class(sig: &str) -> Option<String> {
    let (module, name) = sig.split_once(':')?;
    let studly = |s: &str| -> String {
        s.split(['-', '_']).filter(|p| !p.is_empty()).map(|p| {
            let mut c = p.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        }).collect()
    };
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    (ok(module) && ok(name)).then(|| format!("{}\\Tasks\\{}", studly(module), studly(name)))
}

/// A dialect task (`OpenGrid\Console\Task`) on a worker: `handle(array $args): int`,
/// 0 = success; what it prints is the output.
fn grd_task(pool: &Arc<Pool>, sig: &str, args: &BTreeMap<String, String>) -> Result<String, String> {
    let class = task_class(sig).ok_or_else(|| format!("invalid task `{sig}` (module:name)"))?;
    let args: Vec<(String, String)> = args.iter().filter(|(k, _)| k.as_str() != "task").map(|(k, v)| (k.clone(), v.clone())).collect();
    let sig = sig.to_string();
    pool.run_sync(move |vm| {
        let run = |vm: &mut grd_vm::vm::Vm| -> Result<i64, String> {
            let id = vm.lookup_class(class.as_bytes()).ok_or_else(|| format!("task `{sig}`: class {class} not found (modules/<module>/app/Tasks/)"))?;
            let base = vm.lookup_class(b"OpenGrid\\Console\\Task").ok_or("Task class missing")?;
            if !vm.instance_of(id, base) {
                return Err(format!("{class} does not extend OpenGrid\\Console\\Task"));
            }
            let task = vm.resolve(id).map_err(|t| t.message)?;
            let mut a = grd_vm::value::PhpArray::new();
            for (k, v) in &args {
                a.insert(grd_vm::value::ArrayKey::from_str(k), grd_vm::value::Val::str(v));
            }
            match vm.call_method_sync_gc_safe(task, b"handle", vec![grd_vm::value::Val::array(a)]).map_err(|t| format!("{}: {}", t.class, t.message))? {
                grd_vm::value::Val::Int(code) => Ok(code),
                other => Err(format!("{class}::handle() must return int, {} returned", other.type_name())),
            }
        };
        let r = run(vm);
        let out = String::from_utf8_lossy(&vm.output).into_owned();
        vm.reset_request();
        match r {
            Ok(0) => Ok(out),
            Ok(code) => Err(format!("{out}task exited with code {code}")),
            Err(e) => Err(format!("{out}{e}")),
        }
    })
    .map_err(|e| format!("worker: {e:?}"))?
}

/// Listen on the control socket until the process ends (call from the
/// server's runtime). A stale socket file (no server behind it) is replaced.
pub async fn serve(pool: Arc<Pool>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let path = socket_path(&pool.options().root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if path.exists() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, format!("{} is in use by another server", path.display())));
        }
        std::fs::remove_file(&path)?;
    }
    let listener = tokio::net::UnixListener::bind(&path)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!(target: "opengrid", socket = %path.display(), "control socket ready");
    loop {
        let (stream, _) = listener.accept().await?;
        let pool = Arc::clone(&pool);
        tokio::spawn(async move {
            if let Err(e) = handle(pool, stream).await {
                tracing::warn!(target: "opengrid", "control connection: {e}");
            }
        });
    }
}

async fn handle(pool: Arc<Pool>, stream: tokio::net::UnixStream) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (r, mut w) = stream.into_split();
    let mut line = String::new();
    tokio::io::BufReader::new(r).read_line(&mut line).await?;
    let answer: Vec<serde_json::Value> = match serde_json::from_str::<serde_json::Value>(&line) {
        Err(e) => vec![serde_json::json!({ "ok": false, "error": format!("bad request: {e}") })],
        Ok(req) => match req.get("cmd").and_then(|c| c.as_str()) {
            Some("status") => vec![serde_json::json!({ "ok": true, "pid": std::process::id(), "workers": pool.options().workers, "generation": pool.generation() })],
            Some("task") => {
                let name = req.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                let args: BTreeMap<String, String> = req.get("args").and_then(|a| a.as_object()).map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect()).unwrap_or_default();
                tracing::info!(target: "opengrid", task = %name, "task from the control socket");
                let p = Arc::clone(&pool);
                match tokio::task::spawn_blocking(move || run_task(&p, &name, &args)).await {
                    Ok(Ok(out)) => vec![serde_json::json!({ "out": out }), serde_json::json!({ "ok": true })],
                    Ok(Err(e)) => vec![serde_json::json!({ "ok": false, "error": e })],
                    Err(e) => vec![serde_json::json!({ "ok": false, "error": format!("task panicked: {e}") })],
                }
            }
            _ => vec![serde_json::json!({ "ok": false, "error": "unknown cmd (task, status)" })],
        },
    };
    for a in answer {
        w.write_all(format!("{a}\n").as_bytes()).await?;
    }
    w.flush().await
}

/// Send a request to the server of `root`, if one listens (blocking).
/// None: no server; then the caller runs the task itself.
pub fn request(root: &Path, req: &serde_json::Value) -> Option<Result<Vec<serde_json::Value>, String>> {
    let path = socket_path(root);
    let mut stream = std::os::unix::net::UnixStream::connect(&path).ok()?;
    let mut go = || -> Result<Vec<serde_json::Value>, String> {
        stream.write_all(format!("{req}\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for line in BufReader::new(&stream).lines() {
            let line = line.map_err(|e| e.to_string())?;
            out.push(serde_json::from_str(&line).map_err(|e| e.to_string())?);
        }
        Ok(out)
    };
    Some(go())
}

/// `opengrid task` side: run the task in the running server. None: no
/// server; Some(output) or Some(error) from the server.
pub fn forward(root: &Path, name: &str, args: &BTreeMap<String, String>) -> Option<Result<String, String>> {
    let req = serde_json::json!({ "cmd": "task", "name": name, "args": args });
    let answer = match request(root, &req)? {
        Ok(a) => a,
        Err(e) => return Some(Err(format!("control socket: {e}"))),
    };
    let out: String = answer.iter().filter_map(|a| a.get("out").and_then(|o| o.as_str())).collect();
    let last = answer.last().cloned().unwrap_or_default();
    Some(if last.get("ok").and_then(|o| o.as_bool()) == Some(true) { Ok(out) } else { Err(last.get("error").and_then(|e| e.as_str()).unwrap_or("no answer from the server").to_string()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_names_follow_the_module_convention() {
        assert_eq!(task_class("notes:prune").as_deref(), Some("Notes\\Tasks\\Prune"));
        assert_eq!(task_class("users:send-digest").as_deref(), Some("Users\\Tasks\\SendDigest"));
        assert_eq!(task_class("billing:close_month").as_deref(), Some("Billing\\Tasks\\CloseMonth"));
        assert_eq!(task_class("prune"), None);
        assert_eq!(task_class("a:../x"), None);
    }
}
