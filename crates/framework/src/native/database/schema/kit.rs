//! SurrealKit embedded as a crate (DECISIONS D4, Roadmap R4c): schema sync and
//! rollouts run in the framework's process on the same SDK client. Nothing
//! here starts a process, so credentials never appear on a command line.
//!
//! Layout follows the platform and SurrealKit's CLI: the merged schema is
//! written to `<root>/database/schema/temporary.surql` and tracked under the
//! key `schema/temporary.surql`, which is the key the CLI uses for that file
//! (relative to its `database` folder). A database synced by the platform's
//! `surrealkit sync` is therefore recognised, not re-applied or pruned.
//!
//! Every call runs on its own Tokio task, so a panic inside SurrealKit
//! (it has `expect()`s on invariants) comes back as an error, never unwinds
//! into a worker.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use surrealkit::anyhow;
use surrealkit::rollout::{RolloutExecutionOpts, RolloutPlanOpts, run_baseline, run_complete, run_plan, run_rollback, run_start};
use surrealkit::{EmbeddedSchemaFile, Module, Rollouts, TemplateVars};

/// SurrealKit's folder below the project root (CLI default `./database`).
pub const FOLDER: &str = "database";
/// The generated schema file, relative to the project root.
pub const TEMPORARY: &str = "database/schema/temporary.surql";
/// Tracking key of the generated file in `__entity` (relative to `FOLDER`).
pub const TRACKING_KEY: &str = "schema/temporary.surql";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitError(pub String);

impl std::fmt::Display for KitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for KitError {}

#[derive(Debug, Clone, Copy)]
pub struct SyncOptions {
    /// Remove managed objects that are no longer in the schema (SurrealKit
    /// prunes only objects it manages itself).
    pub prune: bool,
    pub dry_run: bool,
    /// Allow a prune with an empty schema (removes every managed object).
    pub allow_empty: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions { prune: true, dry_run: false, allow_empty: false }
    }
}

/// SurrealKit's folder of a project.
pub fn folder(root: &Path) -> PathBuf {
    root.join(FOLDER)
}

/// Write the merged schema where the platform and SurrealKit expect it.
pub fn write_temporary(root: &Path, text: &str) -> Result<PathBuf, KitError> {
    let p = root.join(TEMPORARY);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| KitError(format!("create {}: {e}", dir.display())))?;
    }
    std::fs::write(&p, text).map_err(|e| KitError(format!("write {}: {e}", p.display())))?;
    Ok(p)
}

/// Write `temporary.surql` and bring the database to it.
pub async fn sync(db: &Surreal<Any>, root: &Path, text: &str, opts: SyncOptions) -> Result<(), KitError> {
    if text.trim().is_empty() && opts.prune && !opts.allow_empty {
        return Err(KitError("the merged schema is empty; refusing a sync that would prune every managed object (allow_empty)".into()));
    }
    warn_on_env();
    write_temporary(root, text)?;
    let sql = intern(text);
    let db = db.clone();
    guarded("schema sync", async move {
        let files = [EmbeddedSchemaFile { path: TRACKING_KEY, sql }];
        surrealkit::Sync::embedded(&files).prune(opts.prune).dry_run(opts.dry_run).allow_empty_prune(opts.allow_empty).run(&db).await
    })
    .await
}

/// Record the current schema files as the starting point for rollouts
/// (`surrealkit rollout baseline`).
pub async fn rollout_baseline(db: &Surreal<Any>, root: &Path) -> Result<(), KitError> {
    let (db, f) = (db.clone(), folder_str(root));
    guarded("rollout baseline", async move { run_baseline(&db, &f, &Module::default_module()).await }).await
}

/// Plan a rollout from the schema folder against the last snapshot
/// (`database/rollouts/<id>.toml`) and run the preflight check on it right
/// away: a rollout that would remove or redefine an existing object is
/// discarded again (manifest, directory and snapshots restored) and the
/// report comes back as the error. Changes to existing objects are never
/// planned (`allow_modified` stays off); rollouts only extend the schema.
pub async fn rollout_plan(db: &Surreal<Any>, root: &Path, name: &str) -> Result<(String, super::preflight::Report), KitError> {
    let f = folder_str(root);
    let before = manifests(root);
    let opts = RolloutPlanOpts { name: Some(name.to_string()), dry_run: false, allow_modified: false };
    let f2 = f.clone();
    guarded("rollout plan", async move { run_plan(&f2, opts).await }).await?;
    let id = manifests(root).into_iter().find(|m| !before.contains(m)).ok_or_else(|| KitError("rollout plan: no change against the last snapshot, nothing planned".into()))?;
    // SurrealKit writes an empty manifest (source == target, no steps) when
    // nothing changed; a rollout that does nothing is not kept
    if unchanged(root, &id) {
        let (f3, id2) = (f.clone(), id.clone());
        guarded("rollout discard", async move { surrealkit::rollout::run_discard(&f3, &id2, false) }).await?;
        return Err(KitError("rollout plan: the schema has not changed since the last rollout, nothing planned".into()));
    }
    let report = super::preflight::check(db, root, &id).await?;
    if !report.passed() {
        let (f3, id2) = (f.clone(), id.clone());
        guarded("rollout discard", async move { surrealkit::rollout::run_discard(&f3, &id2, false) }).await?;
        return Err(KitError(format!("{}
rollout {id} was discarded; rollouts may only add tables, fields, indexes and events", report.summary())));
    }
    Ok((id, report))
}

/// Expansion phase of a rollout (`surrealkit rollout start`).
pub async fn rollout_start(db: &Surreal<Any>, root: &Path, id: Option<&str>) -> Result<(), KitError> {
    // preflight on every start: manifests may be edited by hand or planned elsewhere
    let target = match id {
        Some(i) => i.to_string(),
        None => manifests(root).pop().ok_or_else(|| KitError("rollout start: no planned rollout".into()))?,
    };
    super::preflight::require(db, root, &target).await?;
    let (db, f, sel) = (db.clone(), folder_str(root), id.map(str::to_string));
    guarded("rollout start", async move { run_start(&db, &f, RolloutExecutionOpts::new(sel), &TemplateVars::default()).await }).await.map_err(hint_baseline)
}

/// Contract phase (`surrealkit rollout complete`).
pub async fn rollout_complete(db: &Surreal<Any>, root: &Path, id: Option<&str>) -> Result<(), KitError> {
    let (db, f, sel) = (db.clone(), folder_str(root), id.map(str::to_string));
    guarded("rollout complete", async move { run_complete(&db, &f, RolloutExecutionOpts::new(sel), &TemplateVars::default()).await }).await
}

/// Undo the expansion phase (`surrealkit rollout rollback`).
pub async fn rollout_rollback(db: &Surreal<Any>, root: &Path, id: Option<&str>) -> Result<(), KitError> {
    let (db, f, sel) = (db.clone(), folder_str(root), id.map(str::to_string));
    guarded("rollout rollback", async move { run_rollback(&db, &f, RolloutExecutionOpts::new(sel), &TemplateVars::default()).await }).await
}

/// Rollouts of the project the database has not run yet, as text.
pub async fn rollout_status(db: &Surreal<Any>, root: &Path) -> Result<String, KitError> {
    let (db, f) = (db.clone(), folder_str(root));
    guarded("rollout status", async move {
        let r = Rollouts::load(f)?.pending(&db).await?;
        Ok(format!("{r:?}"))
    })
    .await
}

/// Planned rollouts the database has not run yet, in order (an interrupted
/// or rolled-back one first).
pub async fn pending(db: &Surreal<Any>, root: &Path) -> Result<Vec<String>, KitError> {
    let (db, f) = (db.clone(), folder_str(root));
    guarded("rollout pending", async move { Ok(Rollouts::load(f)?.pending(&db).await?.pending) }).await
}

/// Ids of the planned rollouts (manifest file names without `.toml`), sorted.
pub fn manifests(root: &Path) -> Vec<String> {
    let mut ids: Vec<String> = std::fs::read_dir(folder(root).join("rollouts"))
        .map(|d| d.filter_map(|e| e.ok()).filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".toml").map(str::to_string)).collect())
        .unwrap_or_default();
    ids.sort();
    ids
}

/// Manifest whose source and target schema hash are equal (no change).
fn unchanged(root: &Path, id: &str) -> bool {
    let p = folder(root).join("rollouts").join(format!("{id}.toml"));
    let Ok(text) = std::fs::read_to_string(p) else { return false };
    let Ok(m) = toml::from_str::<toml::Value>(&text) else { return false };
    let h = |k: &str| m.get(k).and_then(|v| v.as_str()).map(str::to_string);
    h("source_schema_hash").is_some() && h("source_schema_hash") == h("target_schema_hash")
}

fn folder_str(root: &Path) -> String {
    folder(root).to_string_lossy().into_owned()
}

/// `rollout start` on a database whose schema came from a plain sync and
/// has no baseline answers "no manifest starts from there"; say what to do.
fn hint_baseline(e: KitError) -> KitError {
    if e.0.contains("no manifest starts from there") {
        KitError(format!("{} — run `rollout baseline` first (it records the starting point for rollouts)", e.0))
    } else {
        e
    }
}

/// Run a SurrealKit future on its own task: errors and panics come back as
/// `KitError`.
async fn guarded<T, F>(what: &str, fut: F) -> Result<T, KitError>
where
    T: Send + 'static,
    F: Future<Output = anyhow::Result<T>> + Send + 'static,
{
    match tokio::spawn(fut).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(KitError(format!("{what}: {e:#}"))),
        Err(j) if j.is_panic() => {
            let p = j.into_panic();
            let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown panic".into());
            Err(KitError(format!("{what}: SurrealKit panicked: {msg}")))
        }
        Err(j) => Err(KitError(format!("{what}: task cancelled: {j}"))),
    }
}

/// `EmbeddedSchemaFile` wants `&'static str`; the merged schema is built at
/// runtime. Each distinct text is kept once for the life of the process
/// (a few KB per schema change, only on sync).
fn intern(text: &str) -> &'static str {
    static TEXTS: OnceLock<Mutex<Vec<&'static str>>> = OnceLock::new();
    let mut texts = TEXTS.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = texts.iter().find(|t| **t == text) {
        return t;
    }
    let t: &'static str = Box::leak(text.to_string().into_boxed_str());
    texts.push(t);
    t
}

/// SurrealKit reads these from the process environment and changes how it
/// prunes; we do not set them, but say so when they are set.
fn warn_on_env() {
    for var in ["SURREALKIT_SHARED_DB", "SURREALKIT_OWNER"] {
        if let Ok(v) = std::env::var(var) {
            tracing::warn!(target: "opengrid", var, value = %v, "SurrealKit reads this environment variable; it changes prune behaviour");
        }
    }
}

/// Forward SurrealKit's `log` records into tracing (once per process; a
/// logger set by someone else stays).
pub fn forward_logs() {
    let _ = tracing_log::LogTracer::init();
}
