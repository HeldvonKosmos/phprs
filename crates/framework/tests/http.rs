//! The request path end to end: pool of worker VMs, route table filled by
//! `routes/web.grd`, controllers, native `Request`/`Response`/`Log`/`Config`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::app::router;
use opengrid_framework::dispatch::handle;
use opengrid_framework::worker::WorkerCtx;
use opengrid_framework::{HttpRequest, HttpResponse, Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool() -> Arc<Pool> {
    let opts = PoolOptions::new(root()).workers(2).settings(serde_json::json!({ "app": { "name": "Fixture" } }));
    Arc::new(Pool::start(opts).expect("pool starts"))
}

fn send(pool: &Pool, req: HttpRequest) -> HttpResponse {
    pool.run_sync(move |vm| handle(vm, req)).expect("worker answers")
}

#[test]
fn routes_controllers_and_natives() {
    let pool = pool();

    let r = send(&pool, HttpRequest::get("/tasks"));
    assert_eq!(r.status, 200, "{}", r.body_str());
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.body_str(), r#"{"app":"Fixture","tasks":[{"id":1,"title":"write"},{"id":2,"title":"test"}]}"#);

    let r = send(&pool, HttpRequest::get("/tasks/7?verbose=1"));
    assert_eq!(r.status, 200, "{}", r.body_str());
    assert_eq!(r.body_str(), r#"{"id":7,"verbose":true,"path":"/tasks/7","route":"7"}"#);

    let r = send(&pool, HttpRequest::post("/tasks", "application/x-www-form-urlencoded", b"title=Write+tests"));
    assert_eq!(r.status, 201, "{}", r.body_str());
    assert_eq!(r.header("x-task"), Some("created"));
    assert_eq!(r.body_str(), r#"{"created":"Write tests","json":false}"#);

    let r = send(&pool, HttpRequest::post("/tasks", "application/json", br#"{"title":"Via JSON"}"#));
    assert_eq!(r.status, 201, "{}", r.body_str());
    assert_eq!(r.body_str(), r#"{"created":"Via JSON","json":true}"#);

    let r = send(&pool, HttpRequest::post("/tasks", "application/json", b"{}"));
    assert_eq!(r.status, 422);

    let r = send(&pool, HttpRequest::get("/hello"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    assert_eq!(r.body_str(), "<h1>Hello</h1>");

    let r = send(&pool, HttpRequest::get("/echoed"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body_str(), "echoed Fixture");

    let r = send(&pool, HttpRequest::get("/old"));
    assert_eq!(r.status, 302);
    assert_eq!(r.header("location"), Some("/tasks"));

    let r = send(&pool, HttpRequest::get("/nope"));
    assert_eq!(r.status, 404, "{}", r.body_str());

    let r = send(&pool, HttpRequest::get("/boom"));
    assert_eq!(r.status, 500);
    assert!(r.body_str().contains("RuntimeException"), "{}", r.body_str());
    assert!(r.body_str().contains("kaputt"), "{}", r.body_str());
}

#[test]
fn requests_leave_no_objects_behind() {
    let pool = pool();
    for i in 0..50 {
        let r = send(&pool, HttpRequest::get(&format!("/tasks/{i}")));
        assert_eq!(r.status, 200);
    }
    let (objects, logs) = pool
        .run_sync(|vm| {
            let objects = vm.objects.len();
            let logs = vm.host::<WorkerCtx>().map(|c| c.logs.len()).unwrap_or(0);
            (objects, logs)
        })
        .expect("worker answers");
    // nothing survives a request: no live objects on the worker
    assert_eq!(objects, 0);
    // the log ring is worker state and keeps what Log::info wrote
    assert!(logs <= 200);
    let r = send(&pool, HttpRequest::get("/tasks"));
    assert_eq!(r.status, 200);
    let last = pool.run_sync(|vm| vm.host::<WorkerCtx>().and_then(|c| c.logs.last().cloned())).expect("worker answers");
    // one of the two workers handled the last request; the log line is there on that one
    if let Some((level, line)) = last {
        assert_eq!(level, "info");
        assert_eq!(line, r#"listing tasks {"count":2}"#);
    }
}

#[tokio::test]
async fn axum_catch_all() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let app = router(pool());
    let req = axum::http::Request::builder().uri("/tasks/3?verbose=0").body(axum::body::Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], br#"{"id":3,"verbose":false,"path":"/tasks/3","route":"3"}"#);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/tasks")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"title":"x"}"#))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 201);
}

#[test]
fn loco_config_of_the_fixture_loads() {
    use loco_rs::config::Config;
    use loco_rs::environment::Environment;
    let cfg = Config::from_folder(&Environment::Test, &root().join("config")).expect("config loads");
    let opts = opengrid_framework::app::pool_options(cfg.settings.as_ref());
    assert_eq!(opts.workers, 2);
    assert_eq!(cfg.settings.as_ref().and_then(|s| s.pointer("/app/name")).and_then(|v| v.as_str()), Some("Fixture"));
}
