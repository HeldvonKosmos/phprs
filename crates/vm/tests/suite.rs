//! One integration-test binary per crate (NACHRICHT 24.10. 5c): every test file
//! links the whole SurrealDB engine, so one binary instead of one per file.

#[path = "suite/dialect.rs"]
mod dialect;
#[path = "suite/names.rs"]
mod names;
#[path = "suite/native_classes.rs"]
mod native_classes;
#[path = "suite/probes.rs"]
mod probes;
#[path = "suite/project.rs"]
mod project;
#[path = "suite/statics.rs"]
mod statics;
