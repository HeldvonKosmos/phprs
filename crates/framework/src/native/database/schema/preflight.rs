//! Preflight check of a rollout (Jonas 24.10., DECISIONS "Rollouts nur
//! additiv"): a rollout may only *extend* the schema. Nothing that exists may
//! be removed or redefined, because that can touch the data stored in it.
//!
//! Two independent checks:
//! 1. **Manifest rules.** Steps that remove objects (`remove_entities`) are
//!    allowed only in the rollback phase (that is the undo of an addition);
//!    data-changing steps (`run_sql`) and renames are refused; unknown step
//!    kinds are refused (fail closed).
//! 2. **Engine comparison.** The rollout's frozen target schema is applied to
//!    a throw-away in-memory database of the same engine; every object the
//!    rollout started from (its catalog snapshot) must exist there with the
//!    same definition as in the live database. Both definitions come from
//!    `INFO FOR …` of the same engine, so SurrealDB's own normalisation makes
//!    them comparable; we neither guess nor copy SurrealKit's hashing.
//!    A new field on an existing table must be optional or have a DEFAULT /
//!    VALUE, otherwise existing rows no longer satisfy the table.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use surrealdb::Surreal;
use surrealdb::engine::any::Any;

use super::kit::{self, KitError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// A step in phase start/complete that removes or rewrites data/objects.
    ForbiddenStep { step: String, phase: String, kind: String },
    Rename(String),
    /// An existing object is missing from the target schema.
    Removes(String),
    /// An existing object has a different definition in the target schema.
    Modifies { entity: String, before: String, after: String },
    /// A new required field on a table that already exists.
    NewRequiredField { entity: String, definition: String },
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Violation::ForbiddenStep { step, phase, kind } => write!(f, "step `{step}` ({kind}, phase {phase}) changes existing objects or data; only additions are allowed"),
            Violation::Rename(r) => write!(f, "rename `{r}` is not allowed"),
            Violation::Removes(e) => write!(f, "{e} exists and would be removed"),
            Violation::Modifies { entity, before, after } => write!(f, "{entity} exists and would change:\n    before: {before}\n    after:  {after}"),
            Violation::NewRequiredField { entity, definition } => {
                write!(f, "{entity} is a new required field on an existing table (existing rows lack it); make it option<…> or give it a DEFAULT: {definition}")
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub id: String,
    pub violations: Vec<Violation>,
    /// Objects the rollout adds.
    pub additions: Vec<String>,
    /// Allowed but worth a look (e.g. a new UNIQUE index on existing data).
    pub warnings: Vec<String>,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut s = format!("preflight {}: {}", self.id, if self.passed() { "passed (only additions)" } else { "FAILED" });
        for v in &self.violations {
            s.push_str(&format!("\n  ✗ {v}"));
        }
        for w in &self.warnings {
            s.push_str(&format!("\n  ! {w}"));
        }
        for a in &self.additions {
            s.push_str(&format!("\n  + {a}"));
        }
        s
    }
}

fn err(m: impl Into<String>) -> KitError {
    KitError(m.into())
}

fn rollouts_dir(root: &Path) -> PathBuf {
    kit::folder(root).join("rollouts")
}

/// Check the planned rollout `id` against the live database `db`.
pub async fn check(db: &Surreal<Any>, root: &Path, id: &str) -> Result<Report, KitError> {
    let mut report = Report { id: id.to_string(), ..Default::default() };
    let manifest_path = rollouts_dir(root).join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| err(format!("preflight: read {}: {e}", manifest_path.display())))?;
    let manifest: toml::Value = toml::from_str(&text).map_err(|e| err(format!("preflight: parse {}: {e}", manifest_path.display())))?;
    manifest_rules(&manifest, &mut report);

    // the catalog the rollout starts from (written by `rollout plan`)
    let snap_path = rollouts_dir(root).join(id).join("snapshots/catalog_snapshot.json");
    let before: Vec<Entity> = match std::fs::read_to_string(&snap_path) {
        Ok(t) => entities(&serde_json::from_str(&t).map_err(|e| err(format!("preflight: parse {}: {e}", snap_path.display())))?),
        Err(_) => Vec::new(),
    };

    // target schema: frozen files plus inline DDL steps of phase start/complete
    let mut target = String::new();
    for f in surql_files(&rollouts_dir(root).join(id).join("schema")) {
        target.push_str(&std::fs::read_to_string(&f).map_err(|e| err(format!("preflight: read {}: {e}", f.display())))?);
        target.push('\n');
    }
    for step in steps(&manifest) {
        if step.kind == "apply_schema" && step.phase != "rollback"
            && let Some(sql) = step.raw.get("sql").and_then(|v| v.as_str())
        {
            target.push_str(sql);
            target.push('\n');
        }
    }

    // no frozen schema and no inline DDL: the rollout applies no schema, so
    // nothing can be removed or redefined except by steps (checked above)
    if target.trim().is_empty() {
        return Ok(report);
    }
    let scratch = surrealdb::engine::any::connect("mem://").await.map_err(|e| err(format!("preflight: scratch database: {e}")))?;
    scratch.use_ns("preflight").use_db("preflight").await.map_err(|e| err(format!("preflight: scratch database: {e}")))?;
    scratch
        .query(target.as_str())
        .await
        .and_then(|r| r.check())
        .map_err(|e| err(format!("preflight: the target schema of {id} does not apply cleanly: {e}")))?;

    let live = catalog(db).await?;
    let after = catalog(&scratch).await?;
    compare(&before, &live, &after, &mut report);
    Ok(report)
}

/// Fail with the report text unless the rollout passed.
pub async fn require(db: &Surreal<Any>, root: &Path, id: &str) -> Result<Report, KitError> {
    let r = check(db, root, id).await?;
    if r.passed() { Ok(r) } else { Err(err(r.summary())) }
}

struct Step {
    id: String,
    phase: String,
    kind: String,
    raw: toml::Value,
}

fn steps(m: &toml::Value) -> Vec<Step> {
    m.get("steps")
        .and_then(|s| s.as_array())
        .map(|a| {
            a.iter()
                .map(|s| Step {
                    id: s.get("id").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                    phase: s.get("phase").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                    kind: s.get("kind").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                    raw: s.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn manifest_rules(m: &toml::Value, report: &mut Report) {
    if let Some(r) = m.get("renames").and_then(|v| v.as_array()) {
        for x in r {
            report.violations.push(Violation::Rename(x.to_string()));
        }
    }
    for s in steps(m) {
        let allowed = match s.kind.as_str() {
            "apply_files" | "apply_schema" | "assert_sql" => true,
            "remove_entities" => s.phase == "rollback",
            _ => false, // run_sql and anything unknown
        };
        if !allowed {
            report.violations.push(Violation::ForbiddenStep { step: s.id, phase: s.phase, kind: s.kind });
        }
    }
}

#[derive(Debug, Clone)]
struct Entity {
    kind: String,
    scope: Option<String>,
    name: String,
}

impl Entity {
    fn label(&self) -> String {
        match &self.scope {
            Some(t) => format!("{} {}.{}", self.kind, t, self.name),
            None => format!("{} {}", self.kind, self.name),
        }
    }
}

fn entities(snapshot: &serde_json::Value) -> Vec<Entity> {
    snapshot["entities"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some(Entity {
                        kind: e["kind"].as_str()?.to_string(),
                        scope: e["scope"].as_str().map(str::to_string),
                        name: e["name"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn surql_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(surql_files(&p));
        } else if p.extension().is_some_and(|x| x == "surql") {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// table → (DEFINE TABLE, kind → name → definition), from `INFO FOR`.
type Catalog = BTreeMap<String, (String, BTreeMap<&'static str, BTreeMap<String, String>>)>;

async fn catalog(db: &Surreal<Any>) -> Result<Catalog, KitError> {
    let q = |e: surrealdb::Error| err(format!("preflight: INFO FOR: {e}"));
    let mut r = db.query("INFO FOR DB").await.map_err(q)?;
    let info: Option<serde_json::Value> = r.take(0).map_err(q)?;
    let mut out = Catalog::new();
    let Some(tables) = info.as_ref().and_then(|i| i["tables"].as_object()) else { return Ok(out) };
    for (name, def) in tables {
        if name.starts_with("__") {
            continue;
        }
        let mut r = db.query(format!("INFO FOR TABLE `{}`", name.replace('`', "\\`"))).await.map_err(q)?;
        let t: Option<serde_json::Value> = r.take(0).map_err(q)?;
        let mut parts = BTreeMap::new();
        for (key, kind) in [("fields", "field"), ("indexes", "index"), ("events", "event")] {
            let m = t.as_ref().and_then(|t| t[key].as_object()).map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()).unwrap_or_default();
            parts.insert(kind, m);
        }
        out.insert(name.clone(), (def.as_str().unwrap_or_default().to_string(), parts));
    }
    Ok(out)
}

fn lookup<'a>(c: &'a Catalog, e: &Entity) -> Option<&'a str> {
    match (e.kind.as_str(), &e.scope) {
        ("table", _) => c.get(&e.name).map(|t| t.0.as_str()),
        (k @ ("field" | "index" | "event"), Some(table)) => {
            let k: &'static str = match k {
                "field" => "field",
                "index" => "index",
                _ => "event",
            };
            c.get(table)?.1.get(k)?.get(&e.name).map(String::as_str)
        }
        _ => None,
    }
}

fn compare(before: &[Entity], live: &Catalog, after: &Catalog, report: &mut Report) {
    for e in before {
        if !matches!(e.kind.as_str(), "table" | "field" | "index" | "event") {
            continue; // other kinds are covered by the manifest rules
        }
        let Some(now) = lookup(live, e) else { continue }; // planned but not applied here
        match lookup(after, e) {
            None => report.violations.push(Violation::Removes(e.label())),
            Some(next) if next != now => report.violations.push(Violation::Modifies { entity: e.label(), before: now.to_string(), after: next.to_string() }),
            Some(_) => {}
        }
    }
    // additions
    for (table, (_, parts)) in after {
        let existing = live.get(table);
        if existing.is_none() {
            report.additions.push(format!("table {table}"));
        }
        for (kind, items) in parts {
            for (name, def) in items {
                let known = existing.and_then(|t| t.1.get(kind)).is_some_and(|m| m.contains_key(name));
                if known {
                    continue;
                }
                let label = format!("{kind} {table}.{name}");
                if existing.is_some() && *kind == "field" && required(def) {
                    report.violations.push(Violation::NewRequiredField { entity: label.clone(), definition: def.clone() });
                }
                if existing.is_some() && *kind == "index" && def.to_ascii_uppercase().contains(" UNIQUE") {
                    report.warnings.push(format!("{label}: new UNIQUE index on an existing table fails if the data has duplicates"));
                }
                report.additions.push(label);
            }
        }
    }
}

/// A field definition existing rows would violate: a type that does not
/// admit NONE and no DEFAULT/VALUE.
fn required(def: &str) -> bool {
    let up = def.to_ascii_uppercase();
    if up.contains(" DEFAULT ") || up.contains(" VALUE ") {
        return false;
    }
    match super::parser::clause_type(def) {
        None => false,
        Some(ty) => {
            let t = ty.to_ascii_lowercase();
            !(t.starts_with("option<") || t.split('|').any(|p| matches!(p.trim(), "none" | "any")) || t == "any")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_fields() {
        assert!(required("DEFINE FIELD a ON t TYPE string PERMISSIONS FULL"));
        assert!(!required("DEFINE FIELD a ON t TYPE none | string PERMISSIONS FULL"));
        assert!(!required("DEFINE FIELD a ON t TYPE option<string>"));
        assert!(!required("DEFINE FIELD a ON t TYPE string DEFAULT 'x' PERMISSIONS FULL"));
        assert!(!required("DEFINE FIELD a ON t TYPE any PERMISSIONS FULL"));
        assert!(!required("DEFINE FIELD a ON t PERMISSIONS FULL"));
    }

    #[test]
    fn manifest_step_rules() {
        let m = r#"
renames = []
[[steps]]
id = "apply"
phase = "start"
kind = "apply_files"
[[steps]]
id = "drop_old"
phase = "complete"
kind = "remove_entities"
[[steps]]
id = "backfill"
phase = "complete"
kind = "run_sql"
[[steps]]
id = "undo"
phase = "rollback"
kind = "remove_entities"
"#;
        let m: toml::Value = toml::from_str(m).unwrap();
        let mut r = Report::default();
        manifest_rules(&m, &mut r);
        let bad: Vec<String> = r
            .violations
            .iter()
            .map(|v| match v {
                Violation::ForbiddenStep { step, .. } => step.clone(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(bad, vec!["drop_old", "backfill"]);
    }
}
