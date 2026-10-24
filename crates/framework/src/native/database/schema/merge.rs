//! Merging (port of `SurqlSchema`, R4b2, byte-identical to PHP 8.4):
//! every source file is parsed into a schema of its own (`addTable`,
//! `addField`, … with `ensureTable` placeholders), which is then merged into
//! the result (`merge`): a table new to the result is taken as it is; for an
//! existing table the later file's DEFINE TABLE wins, but a placeholder
//! (`DEFINE TABLE <t> TYPE NORMAL SCHEMAFULL PERMISSIONS NONE`) never
//! replaces a definition (R4h; the original let a file that only adds a
//! field reset the table to SCHEMAFULL) — and fields, indexes and events
//! are merged by name, the later file winning (R4h; the original's
//! `array_merge` renumbered integer-like names and kept them twice).

use indexmap::IndexMap;

use super::ast::{Kind, Schema, Statement, TableDef, Warning};

pub const PLACEHOLDER_SUFFIX: &str = "TYPE NORMAL SCHEMAFULL PERMISSIONS NONE";

/// Merge by name, `b` wins; new names are appended.
fn merge_by_name(a: &mut IndexMap<String, Statement>, b: IndexMap<String, Statement>) {
    for (k, v) in b {
        a.insert(k, v);
    }
}

impl Schema {
    /// Statements of one file into a schema of its own (`addTable` replaces
    /// the definition and keeps fields; `ensureTable` creates placeholders).
    fn from_file(statements: Vec<Statement>) -> Schema {
        let mut s = Schema::default();
        for st in statements {
            match st.kind {
                Kind::Table => {
                    let t = s.tables.entry(st.name.clone()).or_default();
                    t.define = Some(st);
                    t.placeholder = false;
                }
                Kind::Field | Kind::Index | Kind::Event => {
                    let t = s.ensure_table(&st.table, &st.source);
                    let map = match st.kind {
                        Kind::Field => &mut t.fields,
                        Kind::Index => &mut t.indexes,
                        _ => &mut t.events,
                    };
                    map.insert(st.name.clone(), st);
                }
            }
        }
        s
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

    /// `SurqlSchema::merge`: `other` (a later file) wins.
    pub fn merge_schema(&mut self, other: Schema) {
        for (name, ot) in other.tables {
            match self.tables.get_mut(&name) {
                None => {
                    self.tables.insert(name, ot);
                }
                Some(t) => {
                    if !ot.placeholder {
                        t.define = ot.define;
                        t.placeholder = false;
                    }
                    merge_by_name(&mut t.fields, ot.fields);
                    merge_by_name(&mut t.indexes, ot.indexes);
                    merge_by_name(&mut t.events, ot.events);
                }
            }
        }
        self.warnings.extend(other.warnings);
    }

    /// The statements of one file, merged as a file of their own.
    pub fn merge(&mut self, statements: Vec<Statement>, warnings: Vec<Warning>) {
        let mut file = Schema::from_file(statements);
        file.warnings = warnings;
        self.merge_schema(file);
    }

    /// Parse and merge one source file.
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
        assert_eq!(s.tables["comments"].define.as_ref().unwrap().text, "DEFINE TABLE comments TYPE NORMAL SCHEMAFULL PERMISSIONS NONE");
        s.merge_source("DEFINE TABLE comments SCHEMALESS;", "b");
        assert!(!s.tables["comments"].placeholder);
        assert_eq!(s.tables["comments"].indexes.len(), 1, "fields/indexes survive the table replacement");
    }

    #[test]
    fn a_later_placeholder_keeps_the_definition() {
        // R4h: the original let a file that only adds a field reset the table
        let mut s = Schema::default();
        s.merge_source("DEFINE TABLE t SCHEMALESS;", "blueprint");
        s.merge_source("DEFINE FIELD a ON t TYPE int;", "manually");
        assert_eq!(s.tables["t"].define.as_ref().unwrap().text, "DEFINE TABLE t SCHEMALESS");
        assert!(!s.tables["t"].placeholder);
        assert_eq!(s.tables["t"].fields.len(), 1);
    }

    #[test]
    fn integer_like_names_merge_by_name() {
        // R4h: the original's array_merge kept `DEFINE FIELD 1` from two files twice
        let mut s = Schema::default();
        s.merge_source("DEFINE TABLE t;\nDEFINE FIELD 1 ON t TYPE int;\nDEFINE FIELD 2 ON t TYPE int;", "a");
        s.merge_source("DEFINE TABLE t;\nDEFINE FIELD 1 ON t TYPE string;", "b");
        assert_eq!(s.tables["t"].fields.len(), 2);
        assert_eq!(s.tables["t"].fields["1"].text, "DEFINE FIELD 1 ON t TYPE string");
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
