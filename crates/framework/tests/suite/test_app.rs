//! TP3/TP5: smoke test of the test project `examples/test-app` in-process —
//! loads, applies its declarative schema, health, register/login and the
//! signed-in user's notes through the real dispatcher (the full acceptance
//! flow against a server process is `api.sh`, see `api_script.rs`). Runs on a copy (migrate_push writes
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

    let opts = PoolOptions::new(root.clone()).workers(2).database(DbConfig::mem("test", "test_app")).settings(serde_json::json!({ "app": { "name": "TestApp" }, "auth": { "model": "Auth\\Models\\User" } }));
    let pool = Arc::new(Pool::start(opts).expect("test app loads"));
    let db = pool.db().expect("database");
    let (_, warnings) = db.block_on(ops::push(db.client(), &root, SyncOptions::default())).expect("migrate_push");
    assert!(warnings.is_empty(), "{warnings:?}");

    let r = send(&pool, HttpRequest::get("/"));
    assert_eq!(r.body_str(), r#"{"app":"TestApp","status":"ok"}"#);

    // OPS1: GET /health
    let h = opengrid_framework::app::health_response(&pool);
    assert_eq!(h.status, 200, "{}", h.body_str());
    let hj = json(&h);
    assert_eq!((hj["status"].as_str(), hj["app"].as_str(), hj["database"]["mode"].as_str()), (Some("ok"), Some("TestApp"), Some("embedded")));
    assert!(hj["database"]["version"].as_str().is_some_and(|v| v.starts_with('3')), "{hj}");

    // TP5: register, login, notes of the signed-in user (the full flow is api.sh)
    let j = "application/json";
    let r = send(&pool, HttpRequest::post("/api/register", j, br#"{"name":"Ada","email":"ada@example.com","password":"secret-123","password_confirmation":"secret-123"}"#));
    assert_eq!(r.status, 201, "{}", r.body_str());
    let r = send(&pool, HttpRequest::post("/api/login", j, br#"{"email":"ada@example.com","password":"secret-123"}"#));
    assert_eq!(r.status, 200, "{}", r.body_str());
    let token = json(&r)["token"].as_str().unwrap().to_string();
    let authed = |mut req: HttpRequest| {
        req.headers.push(("authorization".into(), format!("Bearer {token}")));
        req
    };
    assert_eq!(send(&pool, HttpRequest::get("/api/notes")).status, 401);
    let r = send(&pool, authed(HttpRequest::post("/api/notes", j, br#"{"title":"Erste","priority":2}"#)));
    assert_eq!(r.status, 201, "{}", r.body_str());
    assert_eq!(r.header("x-module"), Some("notes"));
    let first = json(&r);
    assert_eq!((first["title"].as_str(), first["done"].as_bool(), first["priority"].as_i64()), (Some("Erste"), Some(false), Some(2)));
    let id = first["id"].as_str().unwrap().split_once(':').unwrap().1.to_string();
    let r = send(&pool, authed(HttpRequest::post("/api/notes", j, br#"{"title":"Zweite","done":true}"#)));
    assert_eq!(json(&r)["priority"].as_i64(), Some(3), "schema default");
    let r = send(&pool, authed(HttpRequest::post("/api/notes", "application/x-www-form-urlencoded", b"title=x")));
    assert_eq!(r.status, 415);
    let r = send(&pool, authed(HttpRequest::get("/api/notes?sort=priority&filter%5Bdone%5D=false")));
    let page = json(&r);
    assert_eq!((page["total"].as_i64(), page["data"][0]["title"].as_str()), (Some(1), Some("Erste")), "{page}");
    let mut patch = authed(HttpRequest::post(&format!("/api/notes/{id}"), j, br#"{"done":true}"#));
    patch.method = "PATCH".into();
    assert_eq!(json(&send(&pool, patch))["done"].as_bool(), Some(true));
    let mut del = authed(HttpRequest::get(&format!("/api/notes/{id}")));
    del.method = "DELETE".into();
    assert_eq!(send(&pool, del).status, 204);
    let r = send(&pool, authed(HttpRequest::get(&format!("/api/notes/{id}"))));
    assert_eq!(r.status, 404, "{}", r.body_str());
    let _ = std::fs::remove_dir_all(&root);
}
