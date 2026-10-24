//! Merging statements into the schema (port of `SurqlSchema::merge` and
//! `ensureTable`, from their description): a later statement wins per key —
//! the DEFINE TABLE statement as a whole, fields, indexes and events by name
//! within their table. A field/index/event of a table that has no DEFINE
//! TABLE yet creates a placeholder `DEFINE TABLE <t> SCHEMAFULL PERMISSIONS
//! NONE`, which a later DEFINE TABLE replaces.

use super::ast::{Kind, Schema, Statement, TableDef, Warning};

pub const PLACEHOLDER_SUFFIX: &str = "SCHEMAFULL PERMISSIONS NONE";

impl Schema {
    pub fn merge(&mut self, statements: Vec<Statement>, warnings: Vec<Warning>) {
        for st in statements {
            match st.kind {
                Kind::Table => {
                    let t = self.tables.entry(st.name.clone()).or_default();
                    t.define = Some(st);
                    t.placeholder = false;
                }
                Kind::Field | Kind::Index | Kind::Event => {
                    let t = self.ensure_table(&st.table, &st.source);
                    let map = match st.kind {
                        Kind::Field => &mut t.fields,
                        Kind::Index => &mut t.indexes,
                        _ => &mut t.events,
                    };
                    map.insert(st.name.clone(), st);
                }
            }
        }
        self.warnings.extend(warnings);
    }

    fn ensure_table(&mut self, table: &str, source: &str) -> &mut TableDef {
        let t = self.tables.entry(table.to_string()).or_default();
        if t.define.is_none() {
            t.define = Some(Statement {
                kind: Kind::Table,
                name: table.to_string(),
                table: table.to_string(),
                text: format!("DEFINE TABLE {table} {PLACEHOLDER_SUFFIX}"),
                source: source.to_string(),
            });
            t.placeholder = true;
        }
        t
    }

    /// Parse and merge one source text.
    pub fn merge_source(&mut self, text: &str, source: &str) {
        let (st, w) = super::parser::parse_file(text, source);
        self.merge(st, w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_wins_per_key() {
        let mut s = Schema::default();
        s.merge_source("DEFINE TABLE t SCHEMALESS;\nDEFINE FIELD a ON t TYPE string;\nDEFINE FIELD b ON t TYPE int;", "blueprint");
        s.merge_source("DEFINE FIELD a ON t TYPE option<string>;\nDEFINE TABLE t SCHEMAFULL;", "manually");
        let t = &s.tables["t"];
        assert_eq!(t.define.as_ref().unwrap().text, "DEFINE TABLE t SCHEMAFULL");
        assert_eq!(t.fields["a"].text, "DEFINE FIELD a ON t TYPE option<string>");
        assert_eq!(t.fields["a"].source, "manually");
        assert_eq!(t.fields["b"].text, "DEFINE FIELD b ON t TYPE int");
    }

    #[test]
    fn duplicate_keys_in_one_file() {
        let mut s = Schema::default();
        s.merge_source("DEFINE FIELD a ON t TYPE string;\nDEFINE FIELD a ON t TYPE int;", "f");
        assert_eq!(s.tables["t"].fields.len(), 1);
        assert_eq!(s.tables["t"].fields["a"].text, "DEFINE FIELD a ON t TYPE int");
    }

    #[test]
    fn placeholder_until_the_table_is_defined() {
        let mut s = Schema::default();
        s.merge_source("DEFINE INDEX i ON comments FIELDS x;", "a");
        assert!(s.tables["comments"].placeholder);
        assert_eq!(s.tables["comments"].define.as_ref().unwrap().text, "DEFINE TABLE comments SCHEMAFULL PERMISSIONS NONE");
        s.merge_source("DEFINE TABLE comments SCHEMALESS;", "b");
        assert!(!s.tables["comments"].placeholder);
        assert_eq!(s.tables["comments"].indexes.len(), 1, "fields/indexes survive the table replacement");
    }

    #[test]
    fn quoted_and_plain_names_are_the_same_key() {
        let mut s = Schema::default();
        s.merge_source("DEFINE TABLE `t` SCHEMALESS;\nDEFINE FIELD `a` ON `t` TYPE int;", "x");
        s.merge_source("DEFINE FIELD a ON t TYPE string;", "y");
        assert_eq!(s.tables.len(), 1);
        assert_eq!(s.tables["t"].fields["a"].source, "y");
    }

    #[test]
    fn field_type_lookup() {
        let mut s = Schema::default();
        s.merge_source("DEFINE FIELD c ON t TYPE option<int> DEFAULT NONE;", "x");
        assert_eq!(s.field_type("t", "c").as_deref(), Some("option<int>"));
        assert_eq!(s.field_type("t", "missing"), None);
    }
}
