//! One integration-test binary per crate (NACHRICHT 24.10. 5c): every test file
//! links the whole SurrealDB engine, so one binary instead of one per file.

#[path = "suite/database.rs"]
mod database;
#[path = "suite/errors.rs"]
mod errors;
#[path = "suite/http.rs"]
mod http;
#[path = "suite/middleware.rs"]
mod middleware;
#[path = "suite/migrations.rs"]
mod migrations;
#[path = "suite/relations.rs"]
mod relations;
#[path = "suite/reload.rs"]
mod reload;
#[path = "suite/schema_files.rs"]
mod schema_files;
#[path = "suite/session.rs"]
mod session;
#[path = "suite/types.rs"]
mod types;
