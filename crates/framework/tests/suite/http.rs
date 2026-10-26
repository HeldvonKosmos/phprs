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

    // IP1: behind Traefik the client address comes from X-Forwarded-For
    let mut req = HttpRequest::get("/ip");
    req.remote_addr = Some("10.0.0.2".into());
    assert_eq!(send(&pool, req.clone()).body_str(), r#"{"ip":"10.0.0.2"}"#);
    req.headers.push(("X-Forwarded-For".into(), "198.51.100.9, 203.0.113.7".into()));
    assert_eq!(send(&pool, req).body_str(), r#"{"ip":"203.0.113.7"}"#);

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
    assert_eq!(pool.workers(), 2);
    // both workers at once: two jobs that wait for each other cannot run on
    // the same worker, so each round has both workers handle a request
    let on_both = |f: Arc<dyn Fn(&mut grd_vm::vm::Vm) -> (usize, usize) + Send + Sync>| {
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let jobs: Vec<_> = (0..2)
            .map(|_| {
                let (pool, barrier, f) = (pool.clone(), barrier.clone(), f.clone());
                std::thread::spawn(move || {
                    pool.run_sync(move |vm| {
                        barrier.wait();
                        f(vm)
                    })
                    .expect("worker answers")
                })
            })
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap()).collect::<Vec<_>>()
    };
    for i in 0..25 {
        let results = on_both(Arc::new(move |vm| {
            let r = handle(vm, HttpRequest::get(&format!("/tasks/{i}")));
            (r.status as usize, vm as *const _ as usize)
        }));
        assert!(results.iter().all(|r| r.0 == 200), "{results:?}");
        assert_ne!(results[0].1, results[1].1, "both workers handled round {i}");
    }
    // nothing survives a request: no live objects on either worker
    let left = on_both(Arc::new(|vm| (vm.objects.len(), vm as *const _ as usize)));
    assert_ne!(left[0].1, left[1].1, "both workers checked");
    assert_eq!((left[0].0, left[1].0), (0, 0), "live objects after the requests");
    // the log ring is worker state and keeps what Log::info wrote
    let logs = on_both(Arc::new(|vm| (vm.host::<WorkerCtx>().map(|c| c.logs.len()).unwrap_or(0), 0)));
    assert!(logs.iter().all(|l| l.0 <= 200), "{logs:?}");
    let r = send(&pool, HttpRequest::get("/tasks"));
    assert_eq!(r.status, 200);
    // the worker that answered has the log line; look at both
    let last = on_both(Arc::new(|vm| {
        let hit = vm.host::<WorkerCtx>().and_then(|c| c.logs.last().cloned()).is_some_and(|(level, line)| level == "info" && line == r#"listing tasks {"count":2}"#);
        (hit as usize, 0)
    }));
    assert!(last.iter().any(|l| l.0 == 1), "the log line of the last request is on one worker");
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
    let opts = opengrid_framework::app::pool_options(cfg.settings.as_ref(), true);
    assert!(opts.debug && opts.watch, "development defaults");
    assert_eq!(opts.workers, 2);
    assert_eq!(cfg.settings.as_ref().and_then(|s| s.pointer("/app/name")).and_then(|v| v.as_str()), Some("Fixture"));
}
