//! Sessions: cookie round trip, flash data for exactly one request,
//! isolation between cookies, invalidate, CSRF middleware; memory and
//! database stores.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::session::{SessionConfig, SessionDriver};
use opengrid_framework::{HttpRequest, HttpResponse, Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn send(pool: &Pool, req: HttpRequest) -> HttpResponse {
    pool.run_sync(move |vm| handle(vm, req)).expect("worker answers")
}

fn cookie_of(r: &HttpResponse) -> Option<String> {
    r.headers.iter().find(|(k, _)| k == "set-cookie").map(|(_, v)| v.split(';').next().unwrap().to_string())
}

fn with_cookie(mut req: HttpRequest, cookie: &str) -> HttpRequest {
    req.headers.push(("cookie".into(), cookie.to_string()));
    req
}

fn json(r: &HttpResponse) -> serde_json::Value {
    serde_json::from_slice(&r.body).unwrap_or_else(|_| panic!("not json: {}", r.body_str()))
}

fn exercise(pool: &Pool) {
    // first request starts a session and sets the cookie
    let r = send(pool, HttpRequest::get("/session/put?name=jonas"));
    assert_eq!(r.status, 200, "{}", r.body_str());
    let cookie = cookie_of(&r).expect("set-cookie on a new session");
    assert!(cookie.starts_with("opengrid_session="), "{cookie}");
    assert!(r.header("set-cookie").unwrap().contains("HttpOnly"));
    assert_eq!(json(&r)["hits"], 1);

    // same cookie: data persists, hits counts up, no new cookie
    let r = send(pool, with_cookie(HttpRequest::get("/session/put"), &cookie));
    assert_eq!(json(&r)["hits"], 2);
    assert!(cookie_of(&r).is_none(), "existing session sets no cookie");
    let r = send(pool, with_cookie(HttpRequest::get("/session"), &cookie));
    assert_eq!(json(&r)["name"], "jonas");
    assert_eq!(json(&r)["hits"], 2);
    assert_eq!(json(&r)["msg"], serde_json::Value::Null);

    // another browser: isolated
    let r = send(pool, HttpRequest::get("/session"));
    assert_eq!(json(&r)["name"], serde_json::Value::Null);

    // flash: visible on the next request only
    let r = send(pool, with_cookie(HttpRequest::get("/session/flash"), &cookie));
    assert_eq!(r.status, 302);
    let r = send(pool, with_cookie(HttpRequest::get("/session"), &cookie));
    assert_eq!(json(&r)["msg"], "saved!");
    let r = send(pool, with_cookie(HttpRequest::get("/session"), &cookie));
    assert_eq!(json(&r)["msg"], serde_json::Value::Null, "flash consumed");
    assert_eq!(json(&r)["name"], "jonas", "ordinary data stays");

    // unknown cookie id: a fresh session with a new cookie
    let r = send(pool, with_cookie(HttpRequest::get("/session/put"), "opengrid_session=00000000000000000000000000000000"));
    assert_eq!(json(&r)["hits"], 1);
    assert!(cookie_of(&r).is_some());

    // SES1: an object coupled to the session, rebuilt from its stored form
    let r = send(pool, with_cookie(HttpRequest::get("/session/cart?item=apple"), &cookie));
    assert_eq!(json(&r), serde_json::json!({"items": ["apple"], "owner": "guest", "builds": 1, "same": true}), "{}", r.body_str());
    let r = send(pool, with_cookie(HttpRequest::get("/session/cart?item=pear"), &cookie));
    assert_eq!(json(&r)["items"], serde_json::json!(["apple", "pear"]), "changes of the last request kept");
    assert_eq!(json(&r)["builds"], 1, "restored, not built again");
    let r = send(pool, HttpRequest::get("/session/cart"));
    assert_eq!(json(&r)["items"], serde_json::json!([]), "another session, another cart");
    send(pool, with_cookie(HttpRequest::get("/session/cart/clear"), &cookie));
    let r = send(pool, with_cookie(HttpRequest::get("/session/cart"), &cookie));
    assert_eq!((json(&r)["items"].clone(), json(&r)["builds"].clone()), (serde_json::json!([]), serde_json::json!(2)), "forget drops the object");

    // CSRF
    let r = send(pool, with_cookie(HttpRequest::post("/session/protected", "application/json", b"{}"), &cookie));
    assert_eq!(r.status, 419, "{}", r.body_str());
    let token = json(&send(pool, with_cookie(HttpRequest::get("/session/token"), &cookie)))["token"].as_str().unwrap().to_string();
    let mut req = with_cookie(HttpRequest::post("/session/protected", "application/json", b"{}"), &cookie);
    req.headers.push(("x-csrf-token".into(), token.clone()));
    assert_eq!(send(pool, req).status, 200);
    let body = format!(r#"{{"_token":"{token}"}}"#);
    assert_eq!(send(pool, with_cookie(HttpRequest::post("/session/protected", "application/json", body.as_bytes()), &cookie)).status, 200);

    // invalidate: cookie cleared, data gone
    let r = send(pool, with_cookie(HttpRequest::get("/session/logout"), &cookie));
    assert!(r.header("set-cookie").unwrap().contains("Max-Age=0"));
    let r = send(pool, with_cookie(HttpRequest::get("/session"), &cookie));
    assert_eq!(json(&r)["name"], serde_json::Value::Null);
}

#[test]
fn memory_store() {
    let pool = Arc::new(Pool::start(PoolOptions::new(root()).workers(2)).expect("pool starts"));
    exercise(&pool);
    assert!(pool.sessions().count() >= 2);
}

#[test]
fn database_store() {
    let cfg = SessionConfig { driver: SessionDriver::Database, ..SessionConfig::default() };
    let pool = Arc::new(Pool::start(PoolOptions::new(root()).workers(2).database(DbConfig::mem("test", "sessions")).session(cfg)).expect("pool starts"));
    exercise(&pool);
    let rows = pool
        .run_sync(|vm| {
            let out = vm.run_file("t.grd", b"<?grd\nuse OpenGrid\\Database\\DB;\necho count(DB::select('SELECT * FROM _opengrid_sessions'));").map(|_| String::from_utf8_lossy(&vm.output).into_owned());
            vm.reset_request();
            out
        })
        .unwrap()
        .expect("count");
    assert!(rows.parse::<usize>().unwrap() >= 2, "{rows}");
}
