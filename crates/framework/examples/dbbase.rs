//! Database baseline for BM2 (BENCHMARKS §0.4): the SurrealQL statements a
//! BM2 endpoint runs per request (from `bench/framework/querylog.py`), sent
//! without the framework to the database the benchmark target used —
//! embedded (`surrealkv://`, in this process) or the server (`ws://`).
//! Like OpenGrid's database layer: a runtime with 2 threads, and every
//! client thread waits for its query with `block_on`.
//!
//!   dbbase <url> <endpoint> <clients> <warmup s> <duration s>
//!   endpoint: list | show | create | login, optionally with `+touch` (the
//!   token use written on every request, as with token_touch_seconds = 0)
//! Credentials for `ws://` through SURREAL_DB_USERNAME/PASSWORD; namespace
//! and database through SURREAL_DB_NAMESPACE/DATABASE (default bench/bench,
//! as in bm2.py; embedded: opengrid/opengrid, the framework's default).
//! Prints one JSON line: ops, ops/s, p50 and p99 in ms.

use std::time::{Duration, Instant};

use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::opt::auth::Root;
use surrealdb::types::{Object, Value};

fn query(rt: &tokio::runtime::Handle, db: &Surreal<Any>, sql: &str, vars: Vec<(&str, Value)>) -> Vec<Value> {
    rt.block_on(async {
        let mut obj = Object::new();
        for (k, v) in vars {
            obj.insert(k.to_string(), v);
        }
        let mut resp = db.query(sql).bind(obj).await.expect("query");
        (0..resp.num_statements()).map(|i| resp.take::<Value>(i).expect("statement")).collect()
    })
}

fn field(v: &Value, name: &str) -> Value {
    match v {
        Value::Array(a) => field(a.first().expect("a row"), name),
        Value::Object(o) => o.get(name).cloned().unwrap_or(Value::None),
        other => panic!("not a record: {other:?}"),
    }
}

fn without_id(v: &Value) -> Value {
    match v {
        Value::Array(a) => without_id(a.first().expect("a row")),
        Value::Object(o) => {
            let mut o = o.clone();
            o.remove("id");
            Value::Object(o)
        }
        other => panic!("not a record: {other:?}"),
    }
}

/// What one request of the endpoint needs from the seeded data.
#[derive(Clone)]
struct Data {
    token_hash: Value,
    token_id: Value,
    user_id: Value,
    user_literal: String,
    email: Value,
    note_id: Value,
    note: Value,
    token: Value,
}

fn run_once(rt: &tokio::runtime::Handle, db: &Surreal<Any>, d: &Data, endpoint: &str, touch: bool, k: u64) {
    if endpoint == "login" {
        query(rt, db, "SELECT * FROM users WHERE email = $f0 LIMIT 1 START 0", vec![("f0", d.email.clone())]);
        let mut t = d.token.clone();
        if let Value::Object(o) = &mut t {
            o.insert("token_hash".to_string(), Value::from_t(format!("{:064x}", k)));
        }
        query(rt, db, "CREATE personal_access_tokens CONTENT $data", vec![("data", t)]);
        return;
    }
    query(rt, db, "SELECT * FROM personal_access_tokens WHERE token_hash = $h LIMIT 1 FETCH user", vec![("h", d.token_hash.clone())]);
    if touch {
        // as OpenGrid's `$token->update(['last_used_at' => $now])`: a new
        // value every time (a constant one would leave nothing to write)
        query(rt, db, "UPDATE $rid MERGE { last_used_at: time::now() }", vec![("rid", d.token_id.clone())]);
    }
    match endpoint {
        "list" => {
            query(rt, db, "INFO FOR TABLE notes", vec![]);
            query(rt, db, &format!("SELECT count() AS total FROM notes WHERE deleted_at IS NONE AND user = {} AND done = false GROUP ALL", d.user_literal), vec![]);
            query(rt, db, "SELECT * FROM notes WHERE deleted_at IS NONE AND user = $f0 AND done = $f1 ORDER BY priority DESC LIMIT 15 START 0", vec![("f0", d.user_id.clone()), ("f1", Value::from_t(false))]);
        }
        "show" => {
            query(rt, db, "SELECT * FROM notes WHERE deleted_at IS NONE AND id = $f0 LIMIT 1 START 0", vec![("f0", d.note_id.clone())]);
        }
        "create" => {
            query(rt, db, "CREATE notes CONTENT $data", vec![("data", d.note.clone())]);
        }
        other => panic!("unknown endpoint {other}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, url, endpoint, clients, warmup, duration] = args.as_slice() else {
        eprintln!("usage: dbbase <url> <endpoint[+touch]> <clients> <warmup s> <duration s>");
        std::process::exit(2);
    };
    let (endpoint, touch) = match endpoint.strip_suffix("+touch") {
        Some(e) => (e.to_string(), true),
        None => (endpoint.clone(), false),
    };
    let clients: usize = clients.parse().expect("clients");
    let (warmup, duration) = (Duration::from_secs_f64(warmup.parse().expect("warmup")), Duration::from_secs_f64(duration.parse().expect("duration")));
    let embedded = url.starts_with("surrealkv://");
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
    let (ns, dbname) = if embedded { (env("SURREAL_DB_NAMESPACE", "opengrid"), env("SURREAL_DB_DATABASE", "opengrid")) } else { (env("SURREAL_DB_NAMESPACE", "bench"), env("SURREAL_DB_DATABASE", "bench")) };
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
    let db = rt.block_on(async {
        let c = connect(url.as_str()).await.expect("connect");
        if let (Ok(u), Ok(p)) = (std::env::var("SURREAL_DB_USERNAME"), std::env::var("SURREAL_DB_PASSWORD")) {
            c.signin(Root { username: u, password: p }).await.expect("signin");
        }
        c.use_ns(ns.as_str()).use_db(dbname.as_str()).await.expect("use");
        c
    });
    let h = rt.handle().clone();
    // the seeded data (bm2.py: one user, the notes, the login token)
    let tok = query(&h, &db, "SELECT * FROM personal_access_tokens LIMIT 1", vec![]).remove(0);
    let note = query(&h, &db, "SELECT * FROM notes LIMIT 1 START 100", vec![]).remove(0);
    let user = query(&h, &db, "SELECT * FROM users LIMIT 1", vec![]).remove(0);
    let user_id = field(&user, "id");
    let user_literal = query(&h, &db, "RETURN <string> $u", vec![("u", user_id.clone())]).remove(0);
    let Value::String(user_literal) = user_literal else { panic!("record id as string") };
    let d = Data {
        token_hash: field(&tok, "token_hash"),
        token_id: field(&tok, "id"),
        user_id,
        user_literal,
        email: field(&user, "email"),
        note_id: field(&note, "id"),
        note: without_id(&note),
        token: without_id(&tok),
    };
    let start = Instant::now();
    let measure_from = start + warmup;
    let end = measure_from + duration;
    let results: Vec<Vec<f64>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..clients)
            .map(|c| {
                let (h, db, d, endpoint) = (h.clone(), &db, d.clone(), endpoint.clone());
                s.spawn(move || {
                    let mut lat = Vec::new();
                    // unique per run and client: token hashes of `login` must not
                    // repeat across runs (unique index)
                    let mut k = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_micros() as u64) ^ ((c as u64) << 56);
                    loop {
                        let t0 = Instant::now();
                        if t0 >= end {
                            break;
                        }
                        k += 1;
                        run_once(&h, db, &d, &endpoint, touch, k);
                        if t0 >= measure_from {
                            lat.push(t0.elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                    lat
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("client")).collect()
    });
    let mut all: Vec<f64> = results.into_iter().flatten().collect();
    all.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    let pct = |p: f64| all.get(((all.len() as f64 * p) as usize).min(all.len().saturating_sub(1))).copied().unwrap_or(0.0);
    println!(
        "{{\"ops\": {}, \"ops_per_s\": {:.1}, \"p50_ms\": {:.3}, \"p99_ms\": {:.3}}}",
        all.len(),
        all.len() as f64 / duration.as_secs_f64(),
        pct(0.5),
        pct(0.99)
    );
}
