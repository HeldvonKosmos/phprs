//! `DB::connection('sqlite')`: raw SQL on SQLite, after Laravel's
//! `DB::connection(...)->select/insert/update/delete/statement/transaction`
//! (D23, DBR1). Used for the framework-core benchmark, where both frameworks
//! run the same raw statements; the SurrealDB layer is unchanged.
//!
//! - The URL comes from `OPENGRID_DB_<NAME>_URL` or
//!   `settings.opengrid.connections.<name>.url` (`sqlite://path`).
//! - One connection per worker thread (thread-local), opened on first use
//!   with `journal_mode = WAL`, `synchronous = FULL`, `busy_timeout = 5000`.
//! - Rows are assoc arrays; INTEGER/REAL/TEXT/NULL become int/float/
//!   string/null, a BLOB a string. Bindings keep their type (bool as 0/1,
//!   as PDO binds it for Laravel); a list binds `?`, string keys `:name`.
//! - Statements are prepared on every call, as PDO does for Laravel.

use std::cell::RefCell;
use std::collections::HashMap;

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, Vm};
use rusqlite::types::{Value as SqlValue, ValueRef};

use super::QUERY_EXCEPTION;
use crate::native::{ctx, string_arg};

pub const CLASS: &str = "OpenGrid\\Database\\Connection";

/// Pragmas of every connection (the benchmark's settings on both sides).
const PRAGMAS: &str = "PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000;";

#[derive(Default)]
struct ConnData {
    name: String,
    path: String,
    /// `transaction()` is running on this thread's connection
    in_txn: bool,
}

thread_local! {
    static CONNECTIONS: RefCell<HashMap<String, rusqlite::Connection>> = RefCell::new(HashMap::new());
}

fn sql_error(e: impl std::fmt::Display) -> Throw {
    Throw::new(QUERY_EXCEPTION, format!("SQLite: {e}"))
}

/// `sqlite://path` of a named connection.
fn path_of(vm: &mut Vm, name: &str) -> Result<String, Throw> {
    let env_key = format!("OPENGRID_DB_{}_URL", name.to_ascii_uppercase());
    let url = std::env::var(&env_key).ok().or_else(|| {
        ctx(vm).settings.get("opengrid").and_then(|o| o.get("connections")).and_then(|c| c.get(name)).and_then(|c| c.get("url")).and_then(|u| u.as_str()).map(str::to_string)
    });
    let url = url.ok_or_else(|| Throw::error(format!("database connection [{name}] not configured ({env_key} or settings.opengrid.connections.{name}.url)")))?;
    url.strip_prefix("sqlite://").map(str::to_string).ok_or_else(|| Throw::error(format!("database connection [{name}]: only sqlite:// is supported, got {url}")))
}

fn with_conn<T>(path: &str, f: impl FnOnce(&rusqlite::Connection) -> Result<T, Throw>) -> Result<T, Throw> {
    CONNECTIONS.with(|cell| {
        let mut map = cell.borrow_mut();
        if !map.contains_key(path) {
            let c = rusqlite::Connection::open(path).map_err(sql_error)?;
            c.execute_batch(PRAGMAS).map_err(sql_error)?;
            map.insert(path.to_string(), c);
        }
        f(map.get(path).expect("just opened"))
    })
}

fn to_sql(v: &Val) -> Result<SqlValue, Throw> {
    Ok(match v {
        Val::Null => SqlValue::Null,
        Val::Bool(b) => SqlValue::Integer(*b as i64),
        Val::Int(i) => SqlValue::Integer(*i),
        Val::Float(f) => SqlValue::Real(*f),
        Val::Str(s) => match std::str::from_utf8(s.as_bytes()) {
            Ok(t) => SqlValue::Text(t.to_string()),
            Err(_) => SqlValue::Blob(s.as_bytes().to_vec()),
        },
        other => return Err(Throw::type_error(format!("SQLite binding must be int, float, string, bool or null, {} given", other.type_name()))),
    })
}

/// Positional (`?`) or named (`:name`) bindings from the second argument.
fn bindings(c: &Call) -> Result<(Vec<SqlValue>, Vec<(String, SqlValue)>), Throw> {
    let (mut pos, mut named) = (Vec::new(), Vec::new());
    match c.arg(1) {
        Val::Null => {}
        Val::Array(a) => {
            for (k, v) in a.iter() {
                match k {
                    ArrayKey::Int(_) => pos.push(to_sql(v)?),
                    ArrayKey::Str(s) => {
                        let s = s.to_string_lossy();
                        named.push((if s.starts_with(':') { s } else { format!(":{s}") }, to_sql(v)?));
                    }
                }
            }
        }
        other => return Err(Throw::type_error(format!("bindings must be an array, {} given", other.type_name()))),
    }
    Ok((pos, named))
}

fn bind(stmt: &mut rusqlite::Statement, pos: &[SqlValue], named: &[(String, SqlValue)]) -> Result<(), Throw> {
    for (i, v) in pos.iter().enumerate() {
        stmt.raw_bind_parameter(i + 1, v).map_err(sql_error)?;
    }
    for (k, v) in named {
        let i = stmt.parameter_index(k).map_err(sql_error)?.ok_or_else(|| sql_error(format!("no parameter {k}")))?;
        stmt.raw_bind_parameter(i, v).map_err(sql_error)?;
    }
    Ok(())
}

fn cell(v: ValueRef) -> Val {
    match v {
        ValueRef::Null => Val::Null,
        ValueRef::Integer(i) => Val::Int(i),
        ValueRef::Real(f) => Val::Float(f),
        ValueRef::Text(t) => Val::from(grd_vm::value::PhpString::from(t)),
        ValueRef::Blob(b) => Val::from(grd_vm::value::PhpString::from(b)),
    }
}

/// Run one statement; rows as assoc arrays and the number of changed rows.
fn run(path: &str, sql: &str, pos: &[SqlValue], named: &[(String, SqlValue)]) -> Result<(PhpArray, usize), Throw> {
    with_conn(path, |conn| {
        let mut stmt = conn.prepare(sql).map_err(sql_error)?;
        bind(&mut stmt, pos, named)?;
        let names: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
        // PERF4: the keys once per statement; every row is a clone of this
        // template, filled in column order (as an array literal, REST1).
        // Repeated column names: the last one wins, as with PDO, by insert.
        let mut template = PhpArray::new();
        for n in &names {
            template.insert(ArrayKey::from_str(n), Val::Null);
        }
        let by_position = template.len() == names.len();
        let mut out = PhpArray::new();
        let mut rows = stmt.raw_query();
        while let Some(row) = rows.next().map_err(sql_error)? {
            let mut r = template.clone();
            for (i, n) in names.iter().enumerate() {
                let v = cell(row.get_ref(i).map_err(sql_error)?);
                if by_position {
                    r.set_at(i, v);
                } else {
                    r.insert(ArrayKey::from_str(n), v);
                }
            }
            out.push(Val::array(r));
        }
        drop(rows);
        Ok((out, conn.changes() as usize))
    })
}

fn this_conn(vm: &mut Vm, c: &Call) -> Result<(String, bool), Throw> {
    let h = c.this()?;
    let d = vm.native_payload_mut::<ConnData>(h).ok_or_else(|| Throw::error("Connection payload missing"))?;
    Ok((d.path.clone(), d.in_txn))
}

fn exec(vm: &mut Vm, c: &Call, what: &str) -> Result<(PhpArray, usize), Throw> {
    let sql = string_arg(c, 0, what)?;
    let (pos, named) = bindings(c)?;
    let (path, _) = this_conn(vm, c)?;
    run(&path, &sql, &pos, &named)
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .payload(|| Box::new(ConnData::default()))
        .method("select", |vm, c| Ok(Val::array(exec(vm, &c, "Connection::select")?.0)))
        .method("insert", |vm, c| exec(vm, &c, "Connection::insert").map(|_| Val::Bool(true)))
        .method("update", |vm, c| exec(vm, &c, "Connection::update").map(|(_, n)| Val::Int(n as i64)))
        .method("delete", |vm, c| exec(vm, &c, "Connection::delete").map(|(_, n)| Val::Int(n as i64)))
        .method("statement", |vm, c| exec(vm, &c, "Connection::statement").map(|_| Val::Bool(true)))
        .method("getName", |vm, c| {
            let h = c.this()?;
            let d = vm.native_payload_mut::<ConnData>(h).ok_or_else(|| Throw::error("Connection payload missing"))?;
            Ok(Val::from(d.name.clone()))
        })
        // `transaction(fn)`: BEGIN, the closure, COMMIT; ROLLBACK and the
        // exception passed on when it throws. Nested: runs inside the outer one.
        .method("transaction", |vm, c| {
            let closure = c.arg(0);
            let h = c.this()?;
            let (path, in_txn) = this_conn(vm, &c)?;
            if in_txn {
                return vm.call_value(&closure, Vec::new());
            }
            with_conn(&path, |conn| conn.execute_batch("BEGIN").map_err(sql_error))?;
            vm.native_payload_mut::<ConnData>(h).expect("payload").in_txn = true;
            let result = vm.call_value(&closure, Vec::new());
            vm.native_payload_mut::<ConnData>(h).expect("payload").in_txn = false;
            match result {
                Ok(v) => {
                    with_conn(&path, |conn| conn.execute_batch("COMMIT").map_err(sql_error))?;
                    Ok(v)
                }
                Err(t) => {
                    if let Err(e) = with_conn(&path, |conn| conn.execute_batch("ROLLBACK").map_err(sql_error)) {
                        tracing::warn!(target: "opengrid", error = %e.message, "sqlite rollback failed");
                    }
                    Err(t)
                }
            }
        });
    vm.register_class(def)?;
    Ok(())
}

/// `DB::connection($name)`: a connection object for a configured SQLite
/// database (D23).
pub fn connection(vm: &mut Vm, name: &str) -> Result<Val, Throw> {
    let path = path_of(vm, name)?;
    let class = vm.lookup_class(CLASS.as_bytes()).ok_or_else(|| Throw::error("Connection class missing"))?;
    let h = vm.instantiate(class, Vec::new())?;
    let d = vm.native_payload_mut::<ConnData>(h).ok_or_else(|| Throw::error("Connection payload missing"))?;
    d.name = name.to_string();
    d.path = path;
    Ok(Val::Object(h))
}
