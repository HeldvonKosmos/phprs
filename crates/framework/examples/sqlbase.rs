//! Database baseline of the framework core (THESIS1, BENCHMARKS §0.4 method
//! for SQLite): the statements one request of a core endpoint runs, without
//! the framework, through rusqlite with the pragmas and the prepare-per-call
//! of `DB::connection(...)` (native/database/sqlite.rs), one connection. The
//! same statements as BenchToken and the bench controllers; every cell is
//! read, as the framework does when it builds the rows. The token's sha256
//! is passed in (hashing is request work, not database work).
//!
//!   sqlbase <database file> <token hash> <endpoint> <warmup s> <duration s>
//!   endpoint: health | list | show:<id> | create | hydrate:<n> | collect:<n>
//!             | validate:<n> | json:<n>   (validate and json: token only)
//! Prints one JSON line: ops, ops/s, p50 and p99 in ms.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

fn q(c: &Connection, sql: &str, bind: &[Value]) -> Vec<Vec<Value>> {
    let mut st = c.prepare(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    let cols = st.column_count();
    let mut rows = st.query(params_from_iter(bind.iter())).expect("query");
    let mut out = Vec::new();
    while let Some(r) = rows.next().expect("row") {
        out.push((0..cols).map(|i| r.get::<_, Value>(i).expect("cell")).collect::<Vec<_>>());
    }
    out
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Integer(i) => *i,
        other => panic!("not an integer: {other:?}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, path, hash, endpoint, warmup, duration] = &args[..] else {
        eprintln!("usage: sqlbase <database file> <token hash> <endpoint> <warmup s> <duration s>");
        std::process::exit(2);
    };
    let c = Connection::open(path).expect("open");
    c.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA busy_timeout = 5000;").expect("pragmas");
    let (name, arg) = endpoint.split_once(':').unwrap_or((endpoint, "0"));
    let arg: i64 = arg.parse().expect("number after ':'");
    let one = || {
        if name == "health" {
            return;
        }
        let row = q(&c, "SELECT t.id AS token_id, t.last_used_at, u.id, u.name, u.email FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ? LIMIT 1", &[Value::Text(hash.clone())]).remove(0);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs() as i64;
        if now - int(&row[1]) >= 1 {
            q(&c, "UPDATE tokens SET last_used_at = ? WHERE id = ?", &[Value::Integer(now), row[0].clone()]);
        }
        let uid = row[2].clone();
        match name {
            "list" => {
                q(&c, "SELECT count(*) AS total FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL", &[uid.clone(), Value::Integer(0)]);
                q(&c, "SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE user_id = ? AND done = ? AND deleted_at IS NULL ORDER BY priority DESC, id ASC LIMIT ? OFFSET ?", &[uid, Value::Integer(0), Value::Integer(15), Value::Integer(0)]);
            }
            "show" => {
                q(&c, "SELECT id, title, body, done, priority, created_at, updated_at FROM notes WHERE id = ? AND user_id = ? AND deleted_at IS NULL LIMIT 1", &[Value::Integer(arg), uid]);
            }
            "create" => {
                let ts = Value::Text(chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string());
                q(&c, "INSERT INTO notes (user_id, title, body, done, priority, created_at, updated_at) VALUES (?, ?, ?, 0, ?, ?, ?) RETURNING id", &[uid, Value::Text("Bench".into()), Value::Text("created by bm2".into()), Value::Integer(2), ts.clone(), ts]);
            }
            "hydrate" | "collect" => {
                q(&c, "SELECT n.id, n.title, n.body, n.done, n.priority, n.created_at, u.id AS user_id, u.name AS user_name, u.email AS user_email FROM notes n JOIN users u ON u.id = n.user_id WHERE n.user_id = ? AND n.deleted_at IS NULL ORDER BY n.id LIMIT ?", &[uid, Value::Integer(arg)]);
            }
            "validate" | "json" => {}
            other => panic!("unknown endpoint {other}"),
        }
    };
    let secs = |s: &str| Duration::from_secs_f64(s.parse().expect("seconds"));
    let end = Instant::now() + secs(warmup);
    while Instant::now() < end {
        one();
    }
    let (start, stop) = (Instant::now(), secs(duration));
    let mut lat = Vec::new();
    loop {
        let t = Instant::now();
        one();
        lat.push(t.elapsed());
        if start.elapsed() >= stop {
            break;
        }
    }
    lat.sort();
    let n = lat.len();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!("{{\"ops\":{n},\"ops_s\":{},\"p50_ms\":{},\"p99_ms\":{}}}", n as f64 / start.elapsed().as_secs_f64(), ms(lat[n / 2]), ms(lat[(n * 99 / 100).min(n - 1)]));
}
