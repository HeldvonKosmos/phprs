//! MS1: mail and storage, shared by the workers of a pool. Loco is the
//! engine (mailer over lettre/SMTP, storage over opendal), the dialect sees
//! Laravel's `Mail` and `Storage` (ANALYSE_D13 §D1). Both are async in Loco;
//! a small runtime of their own runs them for the synchronous workers, like
//! the database client's.
//!
//! Settings (Loco YAML, `settings:`):
//! ```yaml
//! mailer:
//!   from: "OpenGrid <noreply@example.com>"
//!   smtp: { enable: true, host: localhost, port: 1025, secure: false }
//! storage:
//!   default: local
//!   disks:
//!     local: { driver: local, root: storage/app }
//!     scratch: { driver: memory }
//! ```
//! Without `mailer.smtp` (or with `enable: false`) nothing leaves the
//! process: mails are logged and kept in the pool's outbox (`Mail::sent()`).
//! Without `storage` there are two disks: `local` (`<project>/storage/app`)
//! and `memory`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use loco_rs::mailer::{EmailSender, MultiEmail};
use loco_rs::storage::{Storage, drivers};

/// J2: the pool's job queue. Jobs go into the workers' own task channel, so
/// a worker runs them between requests. The sender is dropped with the
/// pool (`close_jobs`), otherwise the workers would never see the channel close.
#[derive(Default)]
pub struct JobQueue {
    tx: Mutex<Option<std::sync::mpsc::Sender<crate::worker::Job>>>,
    pub pending: std::sync::atomic::AtomicUsize,
    pub done: std::sync::atomic::AtomicUsize,
    pub failed: std::sync::atomic::AtomicUsize,
    /// J4: this pool runs durable jobs (the server; not task pools).
    pub runner: std::sync::atomic::AtomicBool,
    /// J4: ids of durable jobs handed to a worker and not finished yet.
    queued: Mutex<std::collections::HashSet<String>>,
}

impl JobQueue {
    pub fn push(&self, job: crate::worker::Job) -> Result<(), String> {
        self.pending.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.push_uncounted(job).inspect_err(|_| {
            self.pending.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        })
    }

    /// Work for a worker that is not a job (the durable queue's poll).
    pub fn push_uncounted(&self, job: crate::worker::Job) -> Result<(), String> {
        let guard = self.tx.lock().map_err(|_| "job queue poisoned".to_string())?;
        let tx = guard.as_ref().ok_or("the pool is shutting down")?;
        tx.send(job).map_err(|_| "no worker takes jobs".to_string())
    }

    /// J4: mark a durable job as handed out; false if it already is.
    pub fn claim_slot(&self, id: &str) -> bool {
        self.queued.lock().map(|mut q| q.insert(id.to_string())).unwrap_or(false)
    }

    pub fn release_slot(&self, id: &str) {
        if let Ok(mut q) = self.queued.lock() {
            q.remove(id);
        }
    }
}

pub struct Services {
    pub jobs: JobQueue,
    rt: tokio::runtime::Runtime,
    mailer: Option<EmailSender>,
    pub mail_from: Option<String>,
    outbox: Mutex<Vec<serde_json::Value>>,
    disks: BTreeMap<String, Arc<Storage>>,
    pub default_disk: String,
    /// Project root (mail templates live in its modules).
    pub root: PathBuf,
}

impl Services {
    pub fn new(root: &Path, settings: &serde_json::Value) -> Result<Services, String> {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).thread_name("opengrid-services").enable_all().build().map_err(|e| format!("services runtime: {e}"))?;
        let mailer = match settings.pointer("/mailer/smtp") {
            Some(smtp) if smtp.get("enable").and_then(|v| v.as_bool()).unwrap_or(true) => {
                let mut smtp = smtp.clone();
                if let Some(o) = smtp.as_object_mut() {
                    o.entry("enable").or_insert(serde_json::Value::Bool(true));
                    o.entry("secure").or_insert(serde_json::Value::Bool(false));
                }
                let cfg: loco_rs::config::SmtpMailer = serde_json::from_value(smtp).map_err(|e| format!("settings.mailer.smtp: {e}"))?;
                // lettre builds its transport inside a Tokio context
                let _guard = rt.enter();
                Some(EmailSender::smtp(&cfg).map_err(|e| format!("mailer: {e}"))?)
            }
            _ => None,
        };
        let mail_from = settings.pointer("/mailer/from").and_then(|v| v.as_str()).map(str::to_string);
        let mut disks = BTreeMap::new();
        let configured = settings.pointer("/storage/disks").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        if configured.is_empty() {
            disks.insert("local".to_string(), Arc::new(local_disk(&root.join("storage/app"))?));
            disks.insert("memory".to_string(), Arc::new(Storage::single(drivers::mem::new())));
        }
        for (name, d) in configured {
            let disk = match d.get("driver").and_then(|v| v.as_str()).unwrap_or("local") {
                "local" => {
                    let r = d.get("root").and_then(|v| v.as_str()).unwrap_or("storage/app");
                    let r = Path::new(r);
                    local_disk(&if r.is_absolute() { r.to_path_buf() } else { root.join(r) })?
                }
                "memory" => Storage::single(drivers::mem::new()),
                other => return Err(format!("settings.storage.disks.{name}: unknown driver `{other}` (local, memory)")),
            };
            disks.insert(name, Arc::new(disk));
        }
        let default_disk = settings.pointer("/storage/default").and_then(|v| v.as_str()).unwrap_or("local").to_string();
        Ok(Services { jobs: JobQueue::default(), rt, mailer, mail_from, outbox: Mutex::new(Vec::new()), disks, default_disk, root: root.to_path_buf() })
    }

    pub fn open_jobs(&self, tx: std::sync::mpsc::Sender<crate::worker::Job>) {
        if let Ok(mut g) = self.jobs.tx.lock() {
            *g = Some(tx);
        }
    }

    pub fn close_jobs(&self) {
        if let Ok(mut g) = self.jobs.tx.lock() {
            *g = None;
        }
    }

    pub fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    /// Send over SMTP, or (no mailer configured) log it and keep it in the outbox.
    pub fn send_mail(&self, email: MultiEmail) -> Result<(), String> {
        let record = serde_json::to_value(&email).unwrap_or_default();
        match &self.mailer {
            Some(sender) => self.rt.block_on(sender.mail_multi(&email)).map_err(|e| format!("mail could not be sent: {e}"))?,
            None => tracing::info!(target: "opengrid", to = ?email.to, subject = %email.subject, "mail (not sent: no settings.mailer.smtp)"),
        }
        if let Ok(mut o) = self.outbox.lock() {
            o.push(record);
            // keep the outbox bounded
            if o.len() > 1000 {
                o.remove(0);
            }
        }
        Ok(())
    }

    pub fn sent_mails(&self) -> Vec<serde_json::Value> {
        self.outbox.lock().map(|o| o.clone()).unwrap_or_default()
    }

    pub fn disk(&self, name: &str) -> Option<Arc<Storage>> {
        self.disks.get(name).cloned()
    }
}

fn local_disk(root: &Path) -> Result<Storage, String> {
    std::fs::create_dir_all(root).map_err(|e| format!("storage root {}: {e}", root.display()))?;
    Ok(Storage::single(drivers::local::new_with_prefix(root).map_err(|e| format!("storage root {}: {e}", root.display()))?))
}

/// A path inside a disk: relative, no `..`, no backslash or NUL (the disk
/// root must not be left, whatever the request sends).
pub fn check_path(p: &str) -> Result<PathBuf, String> {
    let bad = p.is_empty() || p.starts_with('/') || p.contains('\\') || p.contains('\0') || p.split('/').any(|s| s == "..");
    if bad {
        return Err(format!("invalid storage path `{p}` (relative, without ..)"));
    }
    Ok(PathBuf::from(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_disk() {
        for ok in ["a.txt", "dir/a.txt", "x/y/z", ".hidden"] {
            assert!(check_path(ok).is_ok(), "{ok}");
        }
        for bad in ["", "/etc/passwd", "../x", "a/../../b", "a\\b", "a\0b", "dir/.."] {
            assert!(check_path(bad).is_err(), "{bad:?}");
        }
    }
}
