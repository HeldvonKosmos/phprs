//! Database layer (DATABASE_LAYER.md): SurrealDB behind native classes.
//! `Builder` and `Model` are Rust; a dialect model only declares
//! `$table`, `$fillable`, `$softDeletes`, `$dates`, `$hidden`, `$filterable`.

pub mod builder;
pub mod db;
pub mod sqlite;
pub mod inflect;
pub mod model;
pub mod record_id;
pub mod schema;
pub mod surreal;
pub mod types;
pub mod value;

use std::collections::BTreeMap;

use grd_vm::error::Throw;
use grd_vm::vm::Vm;
use surrealdb::types::Value;

use super::ctx;

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    record_id::register(vm)?;
    types::register(vm)?;
    builder::register(vm)?;
    model::register(vm)?;
    db::register(vm)?;
    sqlite::register(vm)?;
    schema::register(vm)?;
    Ok(())
}

pub const QUERY_EXCEPTION: &str = "RuntimeException";

/// Run SurrealQL on this worker's connection (inside the open transaction,
/// if any). One `Value` per statement.
pub fn query(vm: &mut Vm, sql: &str, vars: BTreeMap<String, Value>) -> Result<Vec<Value>, Throw> {
    let c = ctx(vm);
    let db = c.db.clone().ok_or_else(|| Throw::error("no database configured (settings.opengrid.database)"))?;
    db.query(c.txn.as_ref(), sql, vars).map_err(|e| Throw::new(QUERY_EXCEPTION, format!("Surreal: {e}")))
}

/// The first statement's result.
pub fn query_one(vm: &mut Vm, sql: &str, vars: BTreeMap<String, Value>) -> Result<Value, Throw> {
    Ok(query(vm, sql, vars)?.into_iter().next().unwrap_or(Value::None))
}
