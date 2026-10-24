//! TP3: smoke test of the test project `examples/test-app` — loads, applies
//! its declarative schema, and serves CRUD with validation, middleware and
//! 404 through the real dispatcher. Runs on a copy (migrate_push writes
//! `database/schema/temporary.surql`) against `mem://`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::native::database::schema::kit::SyncOptions;
use opengrid_framework::native::database::schema::ops;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::{HttpRequest, HttpResponse, Pool, PoolOptions};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == ".opengrid") {
            continue;
        }
        let t = to.join(e.file_name());
        if p.is_dir() { copy_dir(&p, &t) } else { std::fs::copy(&p, &t).map(|_| ()).unwrap() }
    }
}

fn send(pool: &Pool, req: HttpRequest) -> HttpResponse {
    pool.run_sync(move |vm| handle(vm, req)).expect("worker answers")
}

fn json(r: &HttpResponse) -> serde_json::Value {
    serde_json::from_str(&r.body_str()).unwrap_or_else(|e| panic!("{e}: {}", r.body_str()))
}

#[test]
fn test_app_crud() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/test-app");
    let root: PathBuf = std::env::temp_dir().join(format!("opengrid-test-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&src, &root);

    let opts = PoolOptions::new(root.clone()).workers(2).database(DbConfig::mem("test", "test_app")).settings(serde_json::json!({ "app": { "name": "TestApp" } }));
    let pool = Arc::new(Pool::start(opts).expect("test app loads"));
    let db = pool.db().expect("database");
    let (_, warnings) = db.block_on(ops::push(db.client(), &root, SyncOptions::default())).expect("migrate_push");
    assert!(warnings.is_empty(), "{warnings:?}");

    let r = send(&pool, HttpRequest::get("/"));
    assert_eq!(r.body_str(), r#"{"app":"TestApp","status":"ok"}"#);

    let r = send(&pool, HttpRequest::post("/notes", "application/json", br#"{"title":"Erste","priority":2}"#));
    assert_eq!(r.status, 201, "{}", r.body_str());
    assert_eq!(r.header("x-module"), Some("notes"));
    let first = json(&r);
    assert_eq!((first["title"].as_str(), first["done"].as_bool(), first["priority"].as_i64()), (Some("Erste"), Some(false), Some(2)));
    let id = first["id"].as_str().unwrap().split_once(':').unwrap().1.to_string();
    let r = send(&pool, HttpRequest::post("/notes", "application/json", br#"{"title":"Zweite","done":true}"#));
    assert_eq!(r.status, 201);
    assert_eq!(json(&r)["priority"].as_i64(), Some(3), "schema default");

    // validation, middleware
    let r = send(&pool, HttpRequest::post("/notes", "application/json", br#"{"priority":9}"#));
    assert_eq!(r.status, 422);
    let errors = &json(&r)["errors"];
    assert_eq!(errors["title"], serde_json::json!(["The title field is required."]));
    assert_eq!(errors["priority"], serde_json::json!(["The priority field must be between 1 and 5."]));
    let r = send(&pool, HttpRequest::post("/notes", "application/x-www-form-urlencoded", b"title=x"));
    assert_eq!(r.status, 415);

    // list with request-fed filter and sort
    let r = send(&pool, HttpRequest::get("/notes?sort=priority"));
    let titles: Vec<String> = json(&r).as_array().unwrap().iter().map(|n| n["title"].as_str().unwrap().to_string()).collect();
    assert_eq!(titles, vec!["Erste", "Zweite"]);
    let r = send(&pool, HttpRequest::get("/notes?filters%5Bdone%5D=true"));
    assert_eq!(json(&r).as_array().unwrap().len(), 1);

    // update, show, delete, 404
    let mut put = HttpRequest::post(&format!("/notes/{id}"), "application/json", br#"{"done":true}"#);
    put.method = "PUT".into();
    let r = send(&pool, put);
    assert_eq!(r.status, 200, "{}", r.body_str());
    assert_eq!(json(&send(&pool, HttpRequest::get(&format!("/notes/{id}"))))["done"].as_bool(), Some(true));
    let mut del = HttpRequest::post(&format!("/notes/{id}"), "application/json", b"");
    del.method = "DELETE".into();
    assert_eq!(send(&pool, del).status, 200);
    let r = send(&pool, HttpRequest::get(&format!("/notes/{id}")));
    assert_eq!(r.status, 404, "{}", r.body_str());
    assert_eq!(json(&r)["error"].as_str(), Some("ModelNotFoundException"));
    let _ = std::fs::remove_dir_all(&root);
}
