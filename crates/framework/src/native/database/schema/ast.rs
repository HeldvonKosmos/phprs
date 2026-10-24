//! Schema model of the declarative schema (DECISIONS "Schema-Verwaltung
//! deklarativ", R4b): tables with their DEFINE TABLE statement and the
//! FIELD/INDEX/EVENT statements keyed by name. Port of the platform's
//! `App\Database\Migration\Ast/*` (R4b2; byte-identity checked by the golden
//! tests in `tests/suite/schema_golden.rs`).

use indexmap::IndexMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Table,
    Field,
    Index,
    Event,
}

impl Kind {
    pub fn keyword(self) -> &'static str {
        match self {
            Kind::Table => "TABLE",
            Kind::Field => "FIELD",
            Kind::Index => "INDEX",
            Kind::Event => "EVENT",
        }
    }
}

/// One supported DEFINE statement. `text` is the statement without the
/// trailing `;` and without comments; `name`/`table` are unquoted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    pub kind: Kind,
    pub name: String,
    /// Owning table (equal to `name` for `DEFINE TABLE`).
    pub table: String,
    pub text: String,
    /// File (or `database`) the statement came from.
    pub source: String,
}

/// Something the parser skipped or could not read. The PHP original drops
/// these silently; we collect them so a caller can show them (R4 b, first
/// finding) without changing the generated output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub source: String,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct TableDef {
    /// The DEFINE TABLE statement; a placeholder when only fields/indexes/events
    /// named the table (`SurqlSchema::ensureTable`).
    pub define: Option<Statement>,
    pub placeholder: bool,
    pub fields: IndexMap<String, Statement>,
    pub indexes: IndexMap<String, Statement>,
    pub events: IndexMap<String, Statement>,
}

#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub tables: IndexMap<String, TableDef>,
    pub warnings: Vec<Warning>,
}

impl Schema {
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
    }

    /// Declared SurrealQL type of a field (`DEFINE FIELD … TYPE <type> …`),
    /// for typed filters (Schema-Entscheidung Punkt 8).
    pub fn field_type(&self, table: &str, field: &str) -> Option<String> {
        let st = self.tables.get(table)?.fields.get(field)?;
        super::parser::clause_type(&st.text)
    }
}
