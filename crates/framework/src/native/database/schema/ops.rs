//! Schema operations of the framework (R4d): `migrate_push`, `migrate_pull`,
//! `migrate_fresh` for development, and the production start that runs the
//! planned rollouts. All of them work on an open SDK client.
//!
//! Development (`migrate_*`) has no checks: push merges the project's schema
//! and syncs it, pruning managed objects that left the schema. Production
//! never syncs: the server start runs only planned rollouts, each must pass
//! the preflight check first; a failure ends the start with an error (the
//! server exits, Nomad keeps the old version). Abweichungen des Sollschemas
//! ohne geplanten Rollout bleiben unbeachtet (Jonas 24.10.).

use std::path::Path;

use indexmap::IndexMap;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use super::ast::Warning;
use super::dbinfo::{self, TableInfo};
use super::kit::{self, KitError, SyncOptions};

/// The framework's own schema fragment, first in the merge order.
pub const FRAMEWORK_SURQL: &str = include_str!("framework.surql");

fn err(m: impl Into<String>) -> KitError {
    KitError(m.into())
}

/// Merge the project's schema (with the framework fragment) and sync it.
pub async fn push(db: &Surreal<Any>, root: &Path, opts: SyncOptions) -> Result<(String, Vec<Warning>), KitError> {
    let (text, warnings) = super::build_surql(root, Some(FRAMEWORK_SURQL)).map_err(|e| err(format!("read schema sources: {e}")))?;
    for w in &warnings {
        tracing::warn!(target: "opengrid", source = %w.source, "{}", w.message);
    }
    kit::sync(db, root, &text, opts).await?;
    Ok((text, warnings))
}

/// Plan a rollout from the project's current schema sources: regenerate
/// `temporary.surql` first (so "edit the module schema → plan" works without
/// a push), then plan and preflight-check (`kit::rollout_plan`).
pub async fn plan(db: &Surreal<Any>, root: &Path, name: &str) -> Result<(String, super::preflight::Report), KitError> {
    let (text, warnings) = super::build_surql(root, Some(FRAMEWORK_SURQL)).map_err(|e| err(format!("read schema sources: {e}")))?;
    for w in &warnings {
        tracing::warn!(target: "opengrid", source = %w.source, "{}", w.message);
    }
    kit::write_temporary(root, &text)?;
    kit::rollout_plan(db, root, name).await
}

/// The current schema of the database in the generator's form (SurrealKit's
/// own tables left out).
pub async fn pull(db: &Surreal<Any>) -> Result<String, KitError> {
    let q = |e: surrealdb::Error| err(format!("INFO FOR: {e}"));
    let mut r = db.query("INFO FOR DB").await.map_err(q)?;
    let info: Option<serde_json::Value> = r.take(0).map_err(q)?;
    let mut db_tables = Vec::new();
    let mut tables = IndexMap::new();
    if let Some(t) = info.as_ref().and_then(|i| i["tables"].as_object()) {
        for (name, def) in t {
            db_tables.push((name.clone(), def.as_str().unwrap_or_default().to_string()));
            if dbinfo::INTERNAL_TABLES.contains(&name.as_str()) {
                continue;
            }
            let mut r = db.query(format!("INFO FOR TABLE `{}`", name.replace('`', "\\`"))).await.map_err(q)?;
            let ti: Option<serde_json::Value> = r.take(0).map_err(q)?;
            let list = |key: &str| -> Vec<(String, String)> {
                ti.as_ref().and_then(|t| t[key].as_object()).map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()).unwrap_or_default()
            };
            tables.insert(name.clone(), TableInfo { fields: list("fields"), indexes: list("indexes"), events: list("events") });
        }
    }
    Ok(super::generator::generate(&dbinfo::from_info(&db_tables, &tables)))
}

/// Empty the database and push the schema again. `keep_db`: remove every
/// table but keep the database (and its users, functions, params); else
/// remove and recreate the database. Development only; the caller asks for
/// `--force`.
pub async fn fresh(db: &Surreal<Any>, root: &Path, database: &str, keep_db: bool) -> Result<(String, Vec<Warning>), KitError> {
    let q = |e: surrealdb::Error| err(format!("migrate_fresh: {e}"));
    if keep_db {
        let mut r = db.query("INFO FOR DB").await.map_err(q)?;
        let info: Option<serde_json::Value> = r.take(0).map_err(q)?;
        let names: Vec<String> = info.as_ref().and_then(|i| i["tables"].as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default();
        for t in names {
            db.query(format!("REMOVE TABLE IF EXISTS `{}`", t.replace('`', "\\`"))).await.and_then(|r| r.check()).map_err(q)?;
        }
    } else {
        let d = database.replace('`', "\\`");
        db.query(format!("REMOVE DATABASE IF EXISTS `{d}`; DEFINE DATABASE `{d}`;")).await.and_then(|r| r.check()).map_err(q)?;
        db.use_db(database).await.map_err(q)?;
    }
    push(db, root, SyncOptions::default()).await
}

/// What the production start did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StartupReport {
    pub applied: Vec<String>,
}

/// Production start: run every planned rollout the database has not run
/// yet. All pending rollouts pass the preflight check before the first one
/// runs (one fails → nothing changes). Then each runs `start` + `complete`;
/// if a step fails, only that rollout is rolled back (SurrealKit allows one
/// open rollout) and the error is returned — the server must exit.
/// Completed rollouts are not taken back (fix forward, Jonas 24.10.).
pub async fn apply_planned(db: &Surreal<Any>, root: &Path) -> Result<StartupReport, KitError> {
    let mut report = StartupReport::default();
    if kit::manifests(root).is_empty() {
        return Ok(report);
    }
    let pending = kit::pending(db, root).await?;
    for id in &pending {
        super::preflight::require(db, root, id).await.map_err(|e| err(format!("production start refused, nothing changed:\n{e}")))?;
    }
    for id in pending {
        let run = async {
            kit::rollout_start(db, root, Some(&id)).await?;
            kit::rollout_complete(db, root, Some(&id)).await
        };
        if let Err(e) = run.await {
            let undo = kit::rollout_rollback(db, root, Some(&id)).await;
            let undo = match undo {
                Ok(()) => "rolled back".to_string(),
                Err(u) => format!("rollback failed too: {u}"),
            };
            return Err(err(format!("rollout {id} failed ({undo}); applied before it: {:?}\n{e}", report.applied)));
        }
        tracing::info!(target: "opengrid", rollout = %id, "rollout applied");
        report.applied.push(id);
    }
    Ok(report)
}
