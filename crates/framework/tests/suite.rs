//! One integration-test binary per crate (NACHRICHT 24.10. 5c): every test file
//! links the whole SurrealDB engine, so one binary instead of one per file.

#[path = "suite/api_script.rs"]
mod api_script;
#[path = "suite/auth.rs"]
mod auth;
#[path = "suite/control.rs"]
mod control;
#[path = "suite/database.rs"]
mod database;
#[path = "suite/db_embedded.rs"]
mod db_embedded;
#[path = "suite/errors.rs"]
mod errors;
#[path = "suite/http.rs"]
mod http;
#[path = "suite/inject.rs"]
mod inject;
#[path = "suite/jobs.rs"]
mod jobs;
#[path = "suite/mail_storage.rs"]
mod mail_storage;
#[path = "suite/modules.rs"]
mod modules;
#[path = "suite/middleware.rs"]
mod middleware;
#[path = "suite/oracle.rs"]
mod oracle;
#[path = "suite/php_oracles.rs"]
mod php_oracles;
#[path = "suite/relations.rs"]
mod relations;
#[path = "suite/reload.rs"]
mod reload;
#[path = "suite/scaffold.rs"]
mod scaffold;
#[path = "suite/schedule.rs"]
mod schedule;
#[path = "suite/schema_files.rs"]
mod schema_files;
#[path = "suite/schema_golden.rs"]
mod schema_golden;
#[path = "suite/schema_kit.rs"]
mod schema_kit;
#[path = "suite/schema_ops.rs"]
mod schema_ops;
#[path = "suite/server.rs"]
mod server;
#[path = "suite/server_mode.rs"]
mod server_mode;
#[path = "suite/validation_spec.rs"]
mod validation_spec;
#[path = "suite/sqlite.rs"]
mod sqlite;
#[path = "suite/session.rs"]
mod session;
#[path = "suite/transactions.rs"]
mod transactions;
#[path = "suite/types.rs"]
mod types;
#[path = "suite/test_app.rs"]
mod test_app;
