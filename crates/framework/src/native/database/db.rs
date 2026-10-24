//! `OpenGrid\Database\DB`: raw queries, table builders, transactions.

use std::collections::BTreeMap;

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, Vm};
use surrealdb::types::Value;

use super::builder;
use super::value::{from_surreal, to_surreal};
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
        .static_method("table", |vm, c| {
            let table = string_arg(&c, 0, "DB::table")?;
            Ok(Val::Object(builder::new_for_table(vm, &table, false, None)?))
        })
        .static_method("transaction", |vm, c| {
            let closure = c.arg(0);
            let db = ctx(vm).db.clone().ok_or_else(|| Throw::error("no database configured"))?;
            if ctx(vm).txn.is_some() {
                // nested: run inside the outer transaction
                return vm.call_value(&closure, Vec::new());
            }
            let txn = db.begin().map_err(|e| Throw::new(QUERY_EXCEPTION, format!("Surreal: {e}")))?;
            ctx(vm).txn = Some(txn);
            let result = vm.call_value(&closure, Vec::new());
            let txn = ctx(vm).txn.take().expect("transaction still open");
            match result {
                Ok(v) => {
                    db.commit(txn).map_err(|e| Throw::new(QUERY_EXCEPTION, format!("Surreal commit: {e}")))?;
                    Ok(v)
                }
                Err(t) => {
                    if let Err(e) = db.cancel(txn) {
                        tracing::warn!(target: "opengrid", error = %e, "transaction cancel failed");
                    }
                    Err(t)
                }
            }
        });
    vm.register_class(def)?;
    Ok(())
}
