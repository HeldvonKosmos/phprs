//! Middleware pipeline (dialect `Pipeline` over native `Request`), route
//! groups, `abort()`/`HttpException`, and validation with `ValidationException`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::{HttpRequest, HttpResponse, Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool() -> Arc<Pool> {
    Arc::new(Pool::start(PoolOptions::new(root()).workers(1)).expect("pool starts"))
}

fn send(pool: &Pool, req: HttpRequest) -> HttpResponse {
    pool.run_sync(move |vm| handle(vm, req)).expect("worker answers")
}

fn with_token(mut req: HttpRequest) -> HttpRequest {
    req.headers.push(("x-token".into(), "secret".into()));
    req
}

#[test]
fn middleware_groups_and_abort() {
    let pool = pool();
    let r = send(&pool, HttpRequest::get("/admin/tasks"));
    assert_eq!(r.status, 401, "{}", r.body_str());
    assert_eq!(r.body_str(), r#"{"error":"HttpException","message":"token required"}"#);

    let r = send(&pool, with_token(HttpRequest::get("/admin/tasks")));
    assert_eq!(r.status, 200, "{}", r.body_str());
    assert!(r.body_str().starts_with(r#"{"app":"#), "{}", r.body_str());

    let r = send(&pool, HttpRequest::get("/fail"));
    assert_eq!(r.status, 404);
    assert_eq!(r.body_str(), r#"{"error":"HttpException","message":"no such task"}"#);

    // the group's routes exist only under its prefix: without it, POST /tasks
    // reaches the ungrouped `store` (its own message), not `validated`
    let r = send(&pool, with_token(HttpRequest::post("/tasks", "application/json", b"{}")));
    assert_eq!((r.status, r.body_str()), (422, r#"{"error":"title required"}"#.to_string()));
    let r = send(&pool, with_token(HttpRequest::post("/admin/tasks", "application/json", b"{}")));
    assert_eq!(r.status, 422);
    assert!(r.body_str().contains(r#""errors":{"title""#), "{}", r.body_str());
    // the prefix and the middleware stay inside the group
    assert_eq!(send(&pool, with_token(HttpRequest::get("/admin/hello"))).status, 404);
    assert_eq!(send(&pool, HttpRequest::get("/tasks")).status, 200, "no token needed outside the group");
}

#[test]
fn validation_errors_and_validated_data() {
    let pool = pool();
    let r = send(&pool, with_token(HttpRequest::post("/admin/tasks", "application/json", br#"{"title":"","priority":9,"email":"nope"}"#)));
    assert_eq!(r.status, 422, "{}", r.body_str());
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    // Laravel's summary: first message plus the count of the others
    assert_eq!(body["message"], "The title field is required. (and 2 more errors)");
    assert_eq!(body["errors"]["title"][0], "The title field is required.");
    assert_eq!(body["errors"]["priority"][0], "The priority field must be between 1 and 5.");
    assert_eq!(body["errors"]["email"][0], "The email field must be a valid email address.");

    let r = send(&pool, with_token(HttpRequest::post("/admin/tasks", "application/json", br#"{"title":"Ok","priority":3,"extra":1}"#)));
    assert_eq!(r.status, 201, "{}", r.body_str());
    assert_eq!(r.header("x-middleware"), Some("token"), "middleware decorated the response");
    assert_eq!(r.body_str(), r#"{"validated":{"title":"Ok","priority":3}}"#);

    let r = send(&pool, with_token(HttpRequest::post("/admin/tasks", "application/x-www-form-urlencoded", b"title=Form&priority=2&done=1")));
    assert_eq!(r.status, 201, "{}", r.body_str());
    assert_eq!(r.body_str(), r#"{"validated":{"title":"Form","priority":"2","done":"1"}}"#);
}
