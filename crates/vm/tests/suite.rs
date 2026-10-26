//! One integration-test binary per crate (NACHRICHT 24.10. 5c): every test file
//! links the whole SurrealDB engine, so one binary instead of one per file.

#[path = "suite/dialect.rs"]
mod dialect;
#[path = "suite/gc.rs"]
mod gc;
#[path = "suite/inheritance.rs"]
mod inheritance;
#[path = "suite/names.rs"]
mod names;
#[path = "suite/native_classes.rs"]
mod native_classes;
#[path = "suite/oracle.rs"]
mod oracle;
#[path = "suite/probes.rs"]
mod probes;
#[path = "suite/project.rs"]
mod project;
#[path = "suite/support.rs"]
mod support;
#[path = "suite/statics.rs"]
mod statics;
#[path = "suite/types.rs"]
mod types;
