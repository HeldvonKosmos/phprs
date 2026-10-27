//! `OpenGrid\Database\DB`: raw queries, table builders, transactions.

use std::collections::BTreeMap;

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, Vm};
use surrealdb::types::Value;

use super::builder;
use super::value::{from_surreal, to_surreal};
use super::surreal::is_retriable;
use super::{QUERY_EXCEPTION, query};
use crate::native::{ctx, string_arg};

pub const CLASS: &str = "OpenGrid\\Database\\DB";

fn bindings(vm: &mut Vm, c: &Call) -> Result<BTreeMap<String, Value>, Throw> {
    let mut out = BTreeMap::new();
    if let Val::Array(a) = c.arg(1) {
        for (k, v) in a.iter() {
            let key = match k {
                ArrayKey::Int(i) => i.to_string(),
                ArrayKey::Str(s) => s.to_string_lossy(),
            };
            out.insert(key, to_surreal(vm, v)?);
        }
    }
    Ok(out)
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        // all statements → array of results
        .static_method("query", |vm, c| {
            let sql = string_arg(&c, 0, "DB::query")?;
            let vars = bindings(vm, &c)?;
            let results = query(vm, &sql, vars)?;
            let mut out = PhpArray::with_capacity(results.len());
            for r in results {
                let v = from_surreal(vm, r)?;
                out.push(v);
            }
            Ok(Val::array(out))
        })
        // first statement → rows
        .static_method("select", |vm, c| {
            let sql = string_arg(&c, 0, "DB::select")?;
            let vars = bindings(vm, &c)?;
            let first = query(vm, &sql, vars)?.into_iter().next().unwrap_or(Value::None);
            match from_surreal(vm, first)? {
                Val::Null => Ok(Val::empty_array()),
                v @ Val::Array(_) => Ok(v),
                other => Ok(Val::array(PhpArray::from_values([other]))),
            }
        })
        .static_method("statement", |vm, c| {
            let sql = string_arg(&c, 0, "DB::statement")?;
            let vars = bindings(vm, &c)?;
            query(vm, &sql, vars)?;
            Ok(Val::Bool(true))
        })
        // D23: a raw-SQL connection (SQLite) by name, Laravel's DB::connection
        .static_method("connection", |vm, c| {
            let name = string_arg(&c, 0, "DB::connection")?;
            super::sqlite::connection(vm, &name)
        })
        .static_method("table", |vm, c| {
            let table = string_arg(&c, 0, "DB::table")?;
            Ok(Val::Object(builder::new_for_table(vm, &table, false, None)?))
        })
        // `DB::transaction(fn, $attempts = 1)` like Laravel: a write conflict that
        // SurrealDB reports as retriable (optimistic transactions) runs the whole
        // closure again while attempts remain (TX1, D12); everything else is
        // passed on. The default of 1 attempt means no retry, as in Laravel;
        // fewer than 1 attempt runs nothing and returns null, as Laravel's loop.
        .static_method("transaction", |vm, c| {
            let closure = c.arg(0);
            let attempts = match c.arg(1) {
                Val::Null => 1,
                Val::Int(n) if n >= 1 => n,
                Val::Int(_) => return Ok(Val::Null),
                other => return Err(Throw::type_error(format!("DB::transaction(): Argument #2 ($attempts) must be of type int, {} given", other.type_name()))),
            };
            let db = ctx(vm).db.clone().ok_or_else(|| Throw::error("no database configured"))?;
            if ctx(vm).txn.is_some() {
                // nested: run inside the outer transaction
                return vm.call_value(&closure, Vec::new());
            }
            let mut attempt = 1;
            loop {
                let txn = db.begin().map_err(|e| Throw::new(QUERY_EXCEPTION, format!("Surreal: {e}")))?;
                ctx(vm).txn = Some(txn);
                let result = vm.call_value(&closure, Vec::new());
                let txn = ctx(vm).txn.take().expect("transaction still open");
                let conflict = match result {
                    Ok(v) => match db.commit(txn) {
                        Ok(()) => return Ok(v),
                        Err(e) if attempt < attempts && is_retriable(&e) => e,
                        Err(e) => return Err(Throw::new(QUERY_EXCEPTION, format!("Surreal commit: {e}"))),
                    },
                    Err(t) => {
                        if let Err(e) = db.cancel(txn) {
                            tracing::warn!(target: "opengrid", error = %e, "transaction cancel failed");
                        }
                        let message = match t.object {
                            Some(h) => vm.get_property(h, "message").map(|m| m.to_php_string().map(|s| s.to_string_lossy()).unwrap_or_default()).unwrap_or_default(),
                            None => t.message.clone(),
                        };
                        if attempt < attempts && is_retriable(&message) {
                            message
                        } else {
                            return Err(t);
                        }
                    }
                };
                tracing::debug!(target: "opengrid", attempt, error = %conflict, "transaction conflict, running the closure again");
                db.note_retry();
                attempt += 1;
            }
        });
    vm.register_class(def)?;
    Ok(())
}
