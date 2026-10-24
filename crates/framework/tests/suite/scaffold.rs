//! TP1: `opengrid new` creates a project that loads, answers a request and
//! merges its schema without further work.

use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::native::database::schema::build_surql;
use opengrid_framework::scaffold::new_project;
use opengrid_framework::{HttpRequest, Pool, PoolOptions};

#[test]
fn new_project_starts_and_answers() {
    let dir = std::env::temp_dir().join(format!("opengrid-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = new_project(&dir, Some("Demo")).expect("scaffold");
    assert!(files.iter().any(|f| f.ends_with("config/development.yaml")));
    let gitignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(gitignore.lines().any(|l| l == "/.opengrid/"), "embedded database is not committed");
    assert!(std::fs::read_to_string(dir.join("config/development.yaml")).unwrap().contains("name: Demo"));

    // a second `new` into the same directory refuses
    assert!(new_project(&dir, None).is_err());

    let opts = PoolOptions::new(dir.clone()).workers(1).settings(serde_json::json!({ "app": { "name": "Demo" } }));
    let pool = Arc::new(Pool::start(opts).expect("project loads"));
    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/"))).expect("worker answers");
    assert_eq!(r.status, 200, "{}", r.body_str());
    assert_eq!(r.body_str(), r#"{"app":"Demo","status":"ok"}"#);

    // schema sources merge (framework fragment + empty main.surql)
    let (text, warnings) = build_surql(&dir, Some("DEFINE TABLE _opengrid_sessions SCHEMALESS;")).expect("schema");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(text.contains("-- TABLE: _opengrid_sessions"));
    let _ = std::fs::remove_dir_all(&dir);
}
