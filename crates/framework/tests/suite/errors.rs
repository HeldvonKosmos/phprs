//! Error pages: HTML for browsers with file, line, trace and source; JSON
//! for API clients; details only in debug mode.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::{HttpRequest, HttpResponse, Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool(debug: bool) -> Arc<Pool> {
    Arc::new(Pool::start(PoolOptions::new(root()).workers(1).debug(debug)).expect("pool starts"))
}

fn send(pool: &Pool, req: HttpRequest) -> HttpResponse {
    pool.run_sync(move |vm| handle(vm, req)).expect("worker answers")
}

fn browser(path: &str) -> HttpRequest {
    let mut r = HttpRequest::get(path);
    r.headers.push(("accept".into(), "text/html,application/xhtml+xml".into()));
    r
}

#[test]
fn debug_error_page_and_json() {
    let pool = pool(true);
    let r = send(&pool, browser("/boom"));
    assert_eq!(r.status, 500);
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    let body = r.body_str();
    for needle in ["500 | Internal Server Error", "RuntimeException", "kaputt", "TaskController.grd", "boom()", "throw new \\RuntimeException(&#39;kaputt&#39;)".replace("&#39;", "'").as_str(), "class=\"hl\""] {
        assert!(body.contains(needle), "missing {needle:?} in\n{body}");
    }

    let r = send(&pool, HttpRequest::get("/boom"));
    assert_eq!(r.status, 500);
    let json: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(json["error"], "RuntimeException");
    assert_eq!(json["message"], "kaputt");
    assert!(json["file"].as_str().unwrap().ends_with("TaskController.grd"), "{json}");
    assert!(json["trace"].as_str().unwrap().contains("TaskController->boom()"), "{json}");

    let r = send(&pool, browser("/fail"));
    assert_eq!(r.status, 404);
    assert!(r.body_str().contains("404 | Not Found") && r.body_str().contains("no such task"), "{}", r.body_str());

    let r = send(&pool, browser("/nope"));
    assert_eq!(r.status, 404);
    assert!(r.body_str().contains("no route for GET /nope"));
}

#[test]
fn production_hides_details() {
    let pool = pool(false);
    let r = send(&pool, browser("/boom"));
    assert_eq!(r.status, 500);
    let body = r.body_str();
    assert!(body.contains("500 | Internal Server Error") && body.contains("Something went wrong"), "{body}");
    assert!(!body.contains("TaskController.grd") && !body.contains("boom()"), "{body}");

    let r = send(&pool, HttpRequest::get("/boom"));
    let json: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(json["error"], "RuntimeException");
    assert!(json.get("file").is_none() && json.get("trace").is_none(), "{json}");
}

/// MEM1a: a request that keeps creating objects hits the per-request limit
/// with an Error instead of exhausting the process; the worker goes on.
#[test]
fn object_limit_per_request() {
    let pool = Arc::new(Pool::start(PoolOptions::new(root()).workers(1).settings(serde_json::json!({ "opengrid": { "limits": { "objects": 5000 } } }))).expect("pool starts"));
    let src = b"<?grd\nfinal class Node { public function __construct(public int $n) {} }\n$keep = [];\ntry { for ($i = 0; $i < 100000; $i++) { $keep[] = new Node($i); } echo 'no limit'; } catch (Error $e) { echo count($keep), ' ', $e->getMessage(); }";
    let out = pool.run_sync(move |vm| {
        let r = vm.run_file("limit.grd", src).map(|_| String::from_utf8_lossy(&vm.output).into_owned());
        vm.reset_request();
        r
    }).unwrap().expect("script runs");
    assert!(out.starts_with("5000 Allowed object count of 5000 per request exhausted"), "{out}");
    // next request on the same worker: the arena is empty again
    let out = pool.run_sync(|vm| {
        let r = vm.run_file("again.grd", b"<?grd\n$a = []; for ($i = 0; $i < 4000; $i++) { $a[] = new \\stdClass(); } echo count($a);").map(|_| String::from_utf8_lossy(&vm.output).into_owned());
        vm.reset_request();
        r
    }).unwrap().expect("script runs");
    assert_eq!(out, "4000");
}
