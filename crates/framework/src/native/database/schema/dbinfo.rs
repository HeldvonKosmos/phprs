//! Current database state → schema (port of `SurqlDbParser`, R4b2): `INFO FOR DB` gives the DEFINE TABLE statements, `INFO FOR
//! TABLE <t>` the DEFINE FIELD/INDEX/EVENT statements. SurrealKit's own
//! tables are left out, as in the platform. The query itself runs in R4d
//! (`migrate_pull`); this module only turns the answers into the model.

use indexmap::IndexMap;

use super::ast::{Kind, Schema, Statement, TableDef};

/// Tables SurrealKit maintains for itself (DECISIONS D4: `__entity`,
/// `__rollout`, `__seed`).
pub const INTERNAL_TABLES: [&str; 3] = ["__entity", "__rollout", "__seed"];

/// What `INFO FOR TABLE` returns, as name → DEFINE statement.
#[derive(Clone, Debug, Default)]
pub struct TableInfo {
    pub fields: Vec<(String, String)>,
    pub indexes: Vec<(String, String)>,
    pub events: Vec<(String, String)>,
}

pub const SOURCE: &str = "database";

/// `db_tables`: name → DEFINE TABLE statement from `INFO FOR DB`;
/// `tables`: name → the table's `INFO FOR TABLE` answer.
pub fn from_info(db_tables: &[(String, String)], tables: &IndexMap<String, TableInfo>) -> Schema {
    let mut schema = Schema::default();
    let st = |kind, name: &str, table: &str, text: &str| Statement {
        kind,
        name: name.to_string(),
        table: table.to_string(),
        // stored as INFO returns it, like SurqlDbParser
        text: text.to_string(),
        source: SOURCE.to_string(),
    };
    for (name, define) in db_tables {
        if INTERNAL_TABLES.contains(&name.as_str()) {
            continue;
        }
        let mut t = TableDef { define: Some(st(Kind::Table, name, name, define)), ..Default::default() };
        if let Some(info) = tables.get(name) {
            for (f, d) in &info.fields {
                // SurrealDB defines the element field of `array<T>`/`set<T>`
                // itself; written out, the schema would not apply again
                // ("field 'l.*' already exists")
                if let Some(parent) = f.strip_suffix(".*").or_else(|| f.strip_suffix("[*]"))
                    && let Some((_, pd)) = info.fields.iter().find(|(p, _)| p == parent)
                    && element_type(pd).is_some_and(|ty| *d == format!("DEFINE FIELD {f} ON {name} TYPE {ty} PERMISSIONS FULL"))
                {
                    continue;
                }
                t.fields.insert(f.clone(), st(Kind::Field, f, name, d));
            }
            for (i, d) in &info.indexes {
                t.indexes.insert(i.clone(), st(Kind::Index, i, name, d));
            }
            for (e, d) in &info.events {
                t.events.insert(e.clone(), st(Kind::Event, e, name, d));
            }
        }
        schema.tables.insert(name.clone(), t);
    }
    schema
}

/// `T` of a field defined as `TYPE array<T>`, `array<T, n>` or `set<T>`
/// (optionally inside `option<…>` or `none | …`).
fn element_type(define: &str) -> Option<String> {
    let ty = &define[define.find(" TYPE ")? + 6..];
    let ty = ty.strip_prefix("option<").or_else(|| ty.strip_prefix("none | ")).unwrap_or(ty);
    let inner = ty.strip_prefix("array<").or_else(|| ty.strip_prefix("set<"))?;
    let mut depth = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '<' => depth += 1,
            '>' if depth == 0 => return Some(inner[..i].to_string()),
            ',' if depth == 0 => return Some(inner[..i].to_string()),
            '>' => depth -= 1,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_answers_become_the_model() {
        let db = vec![
            ("tasks".to_string(), "DEFINE TABLE tasks TYPE NORMAL SCHEMALESS PERMISSIONS NONE".to_string()),
            ("__entity".to_string(), "DEFINE TABLE __entity TYPE NORMAL SCHEMAFULL PERMISSIONS NONE".to_string()),
            ("__seed".to_string(), "DEFINE TABLE __seed SCHEMAFULL".to_string()),
        ];
        let mut info = IndexMap::new();
        info.insert(
            "tasks".to_string(),
            TableInfo {
                fields: vec![("count".into(), "DEFINE FIELD count ON tasks TYPE none | int PERMISSIONS FULL".into())],
                indexes: vec![("t".into(), "DEFINE INDEX t ON tasks FIELDS title".into())],
                events: vec![],
            },
        );
        let s = from_info(&db, &info);
        assert_eq!(s.tables.keys().collect::<Vec<_>>(), vec!["tasks"]);
        assert_eq!(s.field_type("tasks", "count").as_deref(), Some("none | int"));
        let out = super::super::generator::generate(&s);
        assert!(out.contains("DEFINE FIELD count ON tasks TYPE none | int PERMISSIONS FULL;\n\nDEFINE INDEX t ON tasks FIELDS title;\n"), "{out}");
    }
}
