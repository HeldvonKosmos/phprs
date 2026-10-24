//! Migration runner (DATABASE_LAYER.md §2 "migrator.rs"): `.grd` classes
//! extending `OpenGrid\Database\Migration` under `database/migrations/` and
//! `modules/*/database/migrations/`, tracked in `_opengrid_migrations`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, ObjHandle, PhpArray, Val};
use grd_vm::vm::{NativeClassDef, Vm};
use surrealdb::types::{Datetime, Number, Object, Value};

use super::{query, query_one};
use crate::native::{ctx, int_arg_opt};

pub const TABLE: &str = "_opengrid_migrations";
pub const CLASS: &str = "OpenGrid\\Database\\Migrator";

#[derive(Debug, Clone)]
pub struct MigrationFile {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Default)]
pub struct Report {
    pub ran: Vec<(String, i64)>,
    pub pending: Vec<String>,
    /// What this call executed (up or down), in order.
    pub executed: Vec<String>,
}

impl Report {
    pub fn to_val(&self) -> Val {
        let mut a = PhpArray::new();
        a.insert(ArrayKey::from_str("ran"), Val::array(PhpArray::from_values(self.ran.iter().map(|(n, _)| Val::str(n)))));
        a.insert(ArrayKey::from_str("pending"), Val::array(PhpArray::from_values(self.pending.iter().map(|n| Val::str(n)))));
        a.insert(ArrayKey::from_str("executed"), Val::array(PhpArray::from_values(self.executed.iter().map(|n| Val::str(n)))));
        Val::array(a)
    }
}

/// All migration files, sorted by file name (the timestamp prefix orders them).
pub fn discover(root: &Path) -> Vec<MigrationFile> {
    let mut dirs = vec![root.join("database/migrations")];
    if let Ok(modules) = std::fs::read_dir(root.join("modules")) {
        for m in modules.flatten() {
            dirs.push(m.path().join("database/migrations"));
        }
    }
    let mut files = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "grd")
                && let Some(stem) = p.file_stem().and_then(|s| s.to_str())
            {
                files.push(MigrationFile { name: stem.to_string(), path: p });
            }
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

fn ensure_table(vm: &mut Vm) -> Result<(), Throw> {
    query(vm, &format!("DEFINE TABLE IF NOT EXISTS {TABLE} SCHEMALESS"), Default::default())?;
    Ok(())
}

fn ran(vm: &mut Vm) -> Result<Vec<(String, i64)>, Throw> {
    let v = query_one(vm, &format!("SELECT name, batch FROM {TABLE} ORDER BY name"), Default::default())?;
    let mut out = Vec::new();
    if let Value::Array(rows) = v {
        for r in rows.into_inner() {
            if let Value::Object(o) = r
                && let Some(Value::String(name)) = o.get("name")
            {
                let batch = match o.get("batch") {
                    Some(Value::Number(Number::Int(b))) => *b,
                    _ => 0,
                };
                out.push((name.clone(), batch));
            }
        }
    }
    Ok(out)
}

/// Compile the file, load its single class (once per VM), instantiate it.
fn load(vm: &mut Vm, file: &MigrationFile) -> Result<ObjHandle, Throw> {
    let source = std::fs::read(&file.path).map_err(|e| Throw::error(format!("{}: {e}", file.path.display())))?;
    let module = grd_vm::compile::compile_source(&file.path.to_string_lossy(), &source).map_err(|e| Throw::error(format!("migration {}: {e}", file.name)))?;
    if module.classes.len() != 1 {
        return Err(Throw::error(format!("migration {}: one file, one class (found {})", file.name, module.classes.len())));
    }
    let class_name = module.str_lossy(module.classes[0].name);
    let class = match vm.lookup_class(class_name.as_bytes()) {
        Some(c) => c,
        None => {
            vm.load_module(module)?;
            vm.lookup_class(class_name.as_bytes()).ok_or_else(|| Throw::error(format!("migration {}: class {class_name} not loaded", file.name)))?
        }
    };
    let base = vm.lookup_class(b"OpenGrid\\Database\\Migration").ok_or_else(|| Throw::error("Migration base class missing"))?;
    if !vm.instance_of(class, base) {
        return Err(Throw::error(format!("migration {}: {class_name} must extend OpenGrid\\Database\\Migration", file.name)));
    }
    vm.instantiate(class, Vec::new())
}

pub fn status(vm: &mut Vm, root: &Path) -> Result<Report, Throw> {
    ensure_table(vm)?;
    let ran = ran(vm)?;
    let pending = discover(root).into_iter().filter(|f| !ran.iter().any(|(n, _)| *n == f.name)).map(|f| f.name).collect();
    Ok(Report { ran, pending, executed: Vec::new() })
}

pub fn migrate(vm: &mut Vm, root: &Path) -> Result<Report, Throw> {
    ensure_table(vm)?;
    let ran = ran(vm)?;
    let batch = ran.iter().map(|(_, b)| *b).max().unwrap_or(0) + 1;
    let mut report = Report { ran: ran.clone(), ..Default::default() };
    for file in discover(root) {
        if ran.iter().any(|(n, _)| *n == file.name) {
            continue;
        }
        tracing::info!(target: "grd", migration = %file.name, "migrating");
        let m = load(vm, &file)?;
        vm.call_method_sync(m, b"up", Vec::new())?;
        let mut obj = Object::new();
        obj.insert("name", Value::String(file.name.clone()));
        obj.insert("batch", Value::Number(Number::Int(batch)));
        obj.insert("ran_at", Value::Datetime(Datetime::now()));
        let mut vars = BTreeMap::new();
        vars.insert("data".to_string(), Value::Object(obj));
        query(vm, &format!("CREATE {TABLE} CONTENT $data"), vars)?;
        report.ran.push((file.name.clone(), batch));
        report.executed.push(file.name);
    }
    Ok(report)
}

/// Roll back the last `steps` batches (newest first).
pub fn rollback(vm: &mut Vm, root: &Path, steps: usize) -> Result<Report, Throw> {
    ensure_table(vm)?;
    let ran = ran(vm)?;
    let mut batches: Vec<i64> = ran.iter().map(|(_, b)| *b).collect();
    batches.sort_unstable();
    batches.dedup();
    let targets: Vec<i64> = batches.into_iter().rev().take(steps.max(1)).collect();
    let files = discover(root);
    let mut report = Report { ran: ran.clone(), ..Default::default() };
    let mut victims: Vec<&(String, i64)> = ran.iter().filter(|(_, b)| targets.contains(b)).collect();
    victims.sort_by(|a, b| b.0.cmp(&a.0));
    for (name, _) in victims {
        let Some(file) = files.iter().find(|f| f.name == *name) else {
            return Err(Throw::error(format!("migration {name} was run but its file is missing")));
        };
        tracing::info!(target: "grd", migration = %name, "rolling back");
        let m = load(vm, file)?;
        vm.call_method_sync(m, b"down", Vec::new())?;
        let mut vars = BTreeMap::new();
        vars.insert("n".to_string(), Value::String(name.clone()));
        query(vm, &format!("DELETE FROM {TABLE} WHERE name = $n"), vars)?;
        report.ran.retain(|(n, _)| n != name);
        report.executed.push(name.clone());
    }
    report.pending = files.into_iter().filter(|f| !report.ran.iter().any(|(n, _)| *n == f.name)).map(|f| f.name).collect();
    Ok(report)
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .static_method("run", |vm, _| {
            let root = ctx(vm).root.clone();
            Ok(migrate(vm, &root)?.to_val())
        })
        .static_method("status", |vm, _| {
            let root = ctx(vm).root.clone();
            Ok(status(vm, &root)?.to_val())
        })
        .static_method("rollback", |vm, c| {
            let steps = int_arg_opt(&c, 0, 1)?.max(1) as usize;
            let root = ctx(vm).root.clone();
            Ok(rollback(vm, &root, steps)?.to_val())
        });
    vm.register_class(def)?;
    Ok(())
}
