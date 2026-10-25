//! Declarative schema (DECISIONS "Schema-Verwaltung deklarativ", R4): the
//! `.surql` sources of a project are merged (`build_surql`) and applied with
//! the embedded SurrealKit (`ops`, `kit`). There is no imperative path: the
//! dialect's `Schema` class only *generates* statements (`Schema::sql`) and
//! reads the current state (`hasTable`, `hasColumn`, `columns`); it never
//! changes the database.
//!
//! **Deferred (BP1, D10):** the Blueprint part (`Schema::table`,
//! `Schema::sql`, `Blueprint`, `ColumnDefinition`) stays as it is and is not
//! developed further. Module schemas are written by hand in
//! `modules/<m>/database/schema/schema.surql`.

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, ObjHandle, PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, Vm};
use surrealdb::types::{ToSql, Value};

pub mod ast;
pub mod dbinfo;
pub mod generator;
pub mod kit;
pub mod merge;
pub mod ops;
pub mod parser;
pub mod preflight;
pub mod sources;

/// Merge all schema sources of a project in their fixed order and generate
/// the `temporary.surql` text (R4b). `framework` is the embedded fragment
/// (R4e). Warnings list what the parser skipped.
pub fn build_surql(root: &std::path::Path, framework: Option<&str>) -> std::io::Result<(String, Vec<ast::Warning>)> {
    let mut schema = ast::Schema::default();
    for src in sources::collect(root, framework)? {
        schema.merge_source(&src.text, &src.label);
    }
    let text = generator::generate(&schema);
    Ok((text, schema.warnings))
}

use super::value::to_surreal;
use crate::native::string_arg;

pub const SCHEMA: &str = "OpenGrid\\Database\\Schema";
pub const BLUEPRINT: &str = "OpenGrid\\Database\\Blueprint";
pub const COLUMN: &str = "OpenGrid\\Database\\ColumnDefinition";

#[derive(Clone, Debug, Default)]
pub struct Column {
    pub name: String,
    /// SurrealQL type, e.g. `string`, `option<datetime>`, `record<clients>`; empty = emits nothing (`id()`).
    pub ty: String,
    pub nullable: bool,
    pub default: Option<Value>,
    pub assert: Option<String>,
    pub unique: bool,
    pub index: bool,
    /// `->change()`: no effect on the statement — in the declarative files
    /// the later definition wins anyway; kept for Laravel-style code.
    pub change: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BlueprintData {
    pub table: String,
    pub columns: Vec<Column>,
    /// (index name, fields, unique)
    pub indexes: Vec<(String, Vec<String>, bool)>,
    pub drop_columns: Vec<String>,
    pub drop_indexes: Vec<String>,
    pub schemafull: bool,
}

impl BlueprintData {
    pub fn new(table: &str) -> Self {
        BlueprintData { table: table.to_string(), ..Default::default() }
    }

    fn add(&mut self, name: &str, ty: &str) -> usize {
        self.columns.push(Column { name: name.to_string(), ty: ty.to_string(), ..Default::default() });
        self.columns.len() - 1
    }

    pub fn index_name(&self, fields: &[String], unique: bool) -> String {
        format!("{}_{}_{}", self.table, fields.join("_"), if unique { "unique" } else { "index" })
    }

    /// The statements; `create` adds the `DEFINE TABLE`. Plain `DEFINE …`
    /// without `IF NOT EXISTS`/`OVERWRITE`: the merge reads `blueprint.surql`
    /// like the platform's PHP classes, which take `IF`/`OVERWRITE` for the
    /// name (R4b2); SurrealKit compares and applies the definitions.
    pub fn statements(&self, create: bool) -> Vec<String> {
        let t = &self.table;
        let mut out = Vec::new();
        if create {
            out.push(format!("DEFINE TABLE {t} {}", if self.schemafull { "SCHEMAFULL" } else { "SCHEMALESS" }));
        }
        for c in &self.columns {
            if c.ty.is_empty() {
                continue;
            }
            let ty = if c.nullable && !c.ty.starts_with("option<") { format!("option<{}>", c.ty) } else { c.ty.clone() };
            let mut s = format!("DEFINE FIELD {} ON TABLE {t} TYPE {ty}", c.name);
            if let Some(d) = &c.default {
                s.push_str(&format!(" DEFAULT {}", d.to_sql()));
            }
            if let Some(a) = &c.assert {
                s.push_str(&format!(" ASSERT {a}"));
            }
            out.push(s);
            if c.unique || c.index {
                let fields = vec![c.name.clone()];
                out.push(format!(
                    "DEFINE INDEX {} ON TABLE {t} FIELDS {}{}",
                    self.index_name(&fields, c.unique),
                    c.name,
                    if c.unique { " UNIQUE" } else { "" }
                ));
            }
        }
        for (name, fields, unique) in &self.indexes {
            out.push(format!("DEFINE INDEX {name} ON TABLE {t} FIELDS {}{}", fields.join(", "), if *unique { " UNIQUE" } else { "" }));
        }
        for name in &self.drop_indexes {
            out.push(format!("REMOVE INDEX IF EXISTS {name} ON TABLE {t}"));
        }
        for name in &self.drop_columns {
            out.push(format!("REMOVE FIELD IF EXISTS {name} ON TABLE {t}"));
        }
        out
    }
}

/// Payload of a `ColumnDefinition`: which column of which blueprint.
pub struct ColRef {
    pub blueprint: Option<ObjHandle>,
    pub index: usize,
}

fn bp<'a>(vm: &'a mut Vm, h: ObjHandle) -> Result<&'a mut BlueprintData, Throw> {
    vm.native_payload_mut::<BlueprintData>(h).ok_or_else(|| Throw::error("Blueprint payload missing"))
}

fn column_object(vm: &mut Vm, blueprint: ObjHandle, index: usize) -> Result<Val, Throw> {
    let class = vm.lookup_class(COLUMN.as_bytes()).ok_or_else(|| Throw::error("ColumnDefinition class missing"))?;
    let h = vm.instantiate(class, Vec::new())?;
    *vm.native_payload_mut::<ColRef>(h).ok_or_else(|| Throw::error("ColumnDefinition payload missing"))? = ColRef { blueprint: Some(blueprint), index };
    Ok(Val::Object(h))
}

fn add_column(vm: &mut Vm, c: &Call, ty: &str) -> Result<Val, Throw> {
    let this = c.this()?;
    let name = string_arg(c, 0, "Blueprint")?;
    let index = bp(vm, this)?.add(&name, ty);
    column_object(vm, this, index)
}

fn col_mut<'a>(vm: &'a mut Vm, c: &Call) -> Result<&'a mut Column, Throw> {
    let this = c.this()?;
    let (blueprint, index) = {
        let r = vm.native_payload::<ColRef>(this).ok_or_else(|| Throw::error("ColumnDefinition payload missing"))?;
        (r.blueprint.ok_or_else(|| Throw::error("ColumnDefinition without blueprint"))?, r.index)
    };
    bp(vm, blueprint)?.columns.get_mut(index).ok_or_else(|| Throw::error("column index out of range"))
}

fn names_arg(c: &Call, i: usize) -> Result<Vec<String>, Throw> {
    match c.args.get(i) {
        Some(Val::Str(s)) => Ok(vec![s.to_string_lossy()]),
        Some(Val::Array(a)) => a.values().map(|v| Ok(v.to_php_string()?.to_string_lossy())).collect(),
        _ => Err(Throw::type_error("expected a field name or an array of field names")),
    }
}

/// Run a blueprint closure and return its statements.
fn build(vm: &mut Vm, table: &str, closure: &Val, create: bool) -> Result<Vec<String>, Throw> {
    let class = vm.lookup_class(BLUEPRINT.as_bytes()).ok_or_else(|| Throw::error("Blueprint class missing"))?;
    let h = vm.instantiate(class, Vec::new())?;
    *bp(vm, h)? = BlueprintData::new(table);
    vm.call_value(closure, vec![Val::Object(h)])?;
    Ok(bp(vm, h)?.statements(create))
}


fn info_keys(vm: &mut Vm, sql: &str, section: &str) -> Result<Vec<String>, Throw> {
    let v = super::query_one(vm, sql, Default::default())?;
    if let Value::Object(o) = v
        && let Some(Value::Object(sec)) = o.get(section)
    {
        return Ok(sec.keys().cloned().collect());
    }
    Ok(Vec::new())
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let column = NativeClassDef::new(COLUMN)
        .final_class()
        .payload(|| Box::new(ColRef { blueprint: None, index: 0 }))
        .trace(|p, out| {
            if let Some(h) = p.downcast_ref::<ColRef>().and_then(|c| c.blueprint) {
                out.push(Val::Object(h));
            }
        })
        .method("nullable", |vm, c| {
            col_mut(vm, &c)?.nullable = true;
            Ok(Val::Object(c.this()?))
        })
        .method("default", |vm, c| {
            let v = to_surreal(vm, &c.arg(0))?;
            col_mut(vm, &c)?.default = Some(v);
            Ok(Val::Object(c.this()?))
        })
        .method("assert", |vm, c| {
            let expr = string_arg(&c, 0, "ColumnDefinition::assert")?;
            col_mut(vm, &c)?.assert = Some(expr);
            Ok(Val::Object(c.this()?))
        })
        .method("unique", |vm, c| {
            col_mut(vm, &c)?.unique = true;
            Ok(Val::Object(c.this()?))
        })
        .method("index", |vm, c| {
            col_mut(vm, &c)?.index = true;
            Ok(Val::Object(c.this()?))
        })
        .method("change", |vm, c| {
            col_mut(vm, &c)?.change = true;
            Ok(Val::Object(c.this()?))
        });
    vm.register_class(column)?;

    let blueprint = NativeClassDef::new(BLUEPRINT)
        .final_class()
        .payload(|| Box::new(BlueprintData::default()))
        .trace(grd_vm::vm::gc::trace_nothing)
        .method("id", |vm, c| {
            let this = c.this()?;
            let index = bp(vm, this)?.add("id", "");
            column_object(vm, this, index)
        })
        .method("string", |vm, c| add_column(vm, &c, "string"))
        .method("text", |vm, c| add_column(vm, &c, "string"))
        .method("integer", |vm, c| add_column(vm, &c, "int"))
        .method("int", |vm, c| add_column(vm, &c, "int"))
        .method("bigInteger", |vm, c| add_column(vm, &c, "int"))
        .method("float", |vm, c| add_column(vm, &c, "float"))
        .method("double", |vm, c| add_column(vm, &c, "float"))
        .method("decimal", |vm, c| add_column(vm, &c, "decimal"))
        .method("number", |vm, c| add_column(vm, &c, "number"))
        .method("boolean", |vm, c| add_column(vm, &c, "bool"))
        .method("bool", |vm, c| add_column(vm, &c, "bool"))
        .method("datetime", |vm, c| add_column(vm, &c, "datetime"))
        .method("timestamp", |vm, c| add_column(vm, &c, "datetime"))
        .method("date", |vm, c| add_column(vm, &c, "datetime"))
        .method("json", |vm, c| add_column(vm, &c, "object"))
        .method("object", |vm, c| add_column(vm, &c, "object"))
        .method("uuid", |vm, c| add_column(vm, &c, "uuid"))
        .method("duration", |vm, c| add_column(vm, &c, "duration"))
        .method("bytes", |vm, c| add_column(vm, &c, "bytes"))
        .method("array", |vm, c| {
            let inner = if c.args.len() > 1 { format!("array<{}>", string_arg(&c, 1, "Blueprint::array")?) } else { "array".into() };
            add_column(vm, &c, &inner)
        })
        .method("record", |vm, c| {
            let table = string_arg(&c, 1, "Blueprint::record")?;
            add_column(vm, &c, &format!("record<{table}>"))
        })
        .method("geometry", |vm, c| {
            let kind = if c.args.len() > 1 { string_arg(&c, 1, "Blueprint::geometry")? } else { "point".into() };
            add_column(vm, &c, &format!("geometry<{kind}>"))
        })
        .method("field", |vm, c| {
            let ty = string_arg(&c, 1, "Blueprint::field")?;
            add_column(vm, &c, &ty)
        })
        .method("timestamps", |vm, c| {
            let this = c.this()?;
            let b = bp(vm, this)?;
            for name in ["created_at", "updated_at"] {
                let i = b.add(name, "datetime");
                b.columns[i].nullable = true;
            }
            Ok(Val::Null)
        })
        .method("softDeletes", |vm, c| {
            let this = c.this()?;
            let b = bp(vm, this)?;
            let i = b.add("deleted_at", "datetime");
            b.columns[i].nullable = true;
            Ok(Val::Null)
        })
        .method("index", |vm, c| {
            let this = c.this()?;
            let fields = names_arg(&c, 0)?;
            let b = bp(vm, this)?;
            let name = if c.args.len() > 1 { string_arg(&c, 1, "Blueprint::index")? } else { b.index_name(&fields, false) };
            b.indexes.push((name, fields, false));
            Ok(Val::Null)
        })
        .method("unique", |vm, c| {
            let this = c.this()?;
            let fields = names_arg(&c, 0)?;
            let b = bp(vm, this)?;
            let name = if c.args.len() > 1 { string_arg(&c, 1, "Blueprint::unique")? } else { b.index_name(&fields, true) };
            b.indexes.push((name, fields, true));
            Ok(Val::Null)
        })
        .method("dropColumn", |vm, c| {
            let this = c.this()?;
            let names = names_arg(&c, 0)?;
            bp(vm, this)?.drop_columns.extend(names);
            Ok(Val::Null)
        })
        .method("dropIndex", |vm, c| {
            let this = c.this()?;
            let name = string_arg(&c, 0, "Blueprint::dropIndex")?;
            bp(vm, this)?.drop_indexes.push(name);
            Ok(Val::Null)
        })
        .method("schemafull", |vm, c| {
            bp(vm, c.this()?)?.schemafull = true;
            Ok(Val::Null)
        })
        .method("schemaless", |vm, c| {
            bp(vm, c.this()?)?.schemafull = false;
            Ok(Val::Null)
        })
        .method("getTable", |vm, c| Ok(Val::str(&bp(vm, c.this()?)?.table)))
        .method("toSql", |vm, c| {
            let statements = bp(vm, c.this()?)?.statements(true);
            Ok(Val::array(PhpArray::from_values(statements.iter().map(|s| Val::str(s)))))
        });
    vm.register_class(blueprint)?;

    let schema = NativeClassDef::new(SCHEMA)
        .final_class()
        .method("__construct", |_, _| Err(Throw::error("Schema is static")))
        .static_method("sql", |vm, c| {
            let table = string_arg(&c, 0, "Schema::sql")?;
            let create = c.args.get(2).is_none_or(|v| v.to_bool());
            let statements = build(vm, &table, &c.arg(1), create)?;
            Ok(Val::array(PhpArray::from_values(statements.iter().map(|s| Val::str(s)))))
        })
        .static_method("hasTable", |vm, c| {
            let table = string_arg(&c, 0, "Schema::hasTable")?;
            Ok(Val::Bool(info_keys(vm, "INFO FOR DB", "tables")?.contains(&table)))
        })
        .static_method("hasColumn", |vm, c| {
            let table = string_arg(&c, 0, "Schema::hasColumn")?;
            let column = string_arg(&c, 1, "Schema::hasColumn")?;
            Ok(Val::Bool(info_keys(vm, &format!("INFO FOR TABLE {table}"), "fields")?.contains(&column)))
        })
        .static_method("columns", |vm, c| {
            let table = string_arg(&c, 0, "Schema::columns")?;
            let keys = info_keys(vm, &format!("INFO FOR TABLE {table}"), "fields")?;
            let mut a = PhpArray::new();
            for k in keys {
                a.insert(ArrayKey::from_str(&k), Val::Bool(true));
            }
            Ok(Val::array(a))
        });
    vm.register_class(schema)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements() {
        let mut b = BlueprintData::new("tasks");
        b.add("title", "string");
        let i = b.add("done", "bool");
        b.columns[i].default = Some(Value::Bool(false));
        let i = b.add("client_id", "record<clients>");
        b.columns[i].nullable = true;
        b.columns[i].index = true;
        b.indexes.push((b.index_name(&["title".into()], true), vec!["title".into()], true));
        b.drop_columns.push("old".into());
        assert_eq!(
            b.statements(true),
            vec![
                "DEFINE TABLE tasks SCHEMALESS",
                "DEFINE FIELD title ON TABLE tasks TYPE string",
                "DEFINE FIELD done ON TABLE tasks TYPE bool DEFAULT false",
                "DEFINE FIELD client_id ON TABLE tasks TYPE option<record<clients>>",
                "DEFINE INDEX tasks_client_id_index ON TABLE tasks FIELDS client_id",
                "DEFINE INDEX tasks_title_unique ON TABLE tasks FIELDS title UNIQUE",
                "REMOVE FIELD IF EXISTS old ON TABLE tasks",
            ]
        );
    }
}
