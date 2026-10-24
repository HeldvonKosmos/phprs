//! DB1: the embedded surrealkv database is the default, relative paths
//! resolve against the project, and a start waits (bounded) for a database
//! another process still holds — the hand-over during a deployment.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use opengrid_framework::native::database::surreal::{DEFAULT_LOCK_WAIT, Db, DbConfig};

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("og-db1-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p.canonicalize().unwrap()
}

#[test]
fn embedded_default_and_relative_paths() {
    let root = tmp("default");
    let settings = serde_json::json!({"opengrid": {"root": root.display().to_string()}});
    let opts = opengrid_framework::app::pool_options(Some(&settings), false);
    let db = opts.database.expect("a database by default");
    assert_eq!(db.url, format!("surrealkv://{}/.opengrid/db", root.display()));
    assert_eq!(db.lock_wait, DEFAULT_LOCK_WAIT);

    let settings = serde_json::json!({"opengrid": {"root": root.display().to_string(), "database": {"url": "surrealkv://var/data", "lock_wait": 5}}});
    let db = opengrid_framework::app::pool_options(Some(&settings), false).database.unwrap();
    assert_eq!(db.url, format!("surrealkv://{}/var/data", root.display()), "relative to the project, not the working directory");
    assert_eq!(db.lock_wait, Duration::from_secs(5));

    // connecting creates the directory
    let cfg = DbConfig { lock_wait: Duration::ZERO, ..DbConfig::embedded_default(&root) };
    let url = cfg.url.clone();
    let child = holder(&url, 0); // in a child: this process must not keep the lock
    assert!(child.success());
    assert!(root.join(".opengrid/db").is_dir());
    std::fs::remove_dir_all(&root).unwrap();
}

/// Run `db1_hold_child` in a child process: open `url`, write `<url>.held`,
/// keep the lock for `hold_ms`, exit.
fn spawn_holder(url: &str, hold_ms: u64) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "db_embedded::db1_hold_child", "--ignored", "--nocapture"])
        .env("OG_DB1_URL", url)
        .env("OG_DB1_HOLD_MS", hold_ms.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("child starts")
}

fn holder(url: &str, hold_ms: u64) -> std::process::ExitStatus {
    spawn_holder(url, hold_ms).wait().unwrap()
}

fn wait_held(url: &str) {
    let marker = PathBuf::from(format!("{}.held", url.trim_start_matches("surrealkv://")));
    let t = Instant::now();
    while !marker.exists() {
        assert!(t.elapsed() < Duration::from_secs(30), "child never took the lock");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "runs as child process of the DB1 tests"]
fn db1_hold_child() {
    let (Ok(url), Ok(ms)) = (std::env::var("OG_DB1_URL"), std::env::var("OG_DB1_HOLD_MS")) else { return };
    let db = Db::connect(DbConfig { url: url.clone(), namespace: "t".into(), database: "t".into(), ..Default::default() }).expect("child connects");
    std::fs::write(format!("{}.held", url.trim_start_matches("surrealkv://")), "").unwrap();
    std::thread::sleep(Duration::from_millis(ms.parse().unwrap()));
    drop(db);
}

#[test]
fn start_waits_for_the_previous_process_to_release_the_database() {
    let root = tmp("handover");
    let url = format!("surrealkv://{}/db", root.display());
    let mut child = spawn_holder(&url, 1500);
    wait_held(&url);
    let t = Instant::now();
    let db = Db::connect(DbConfig { url: url.clone(), namespace: "t".into(), database: "t".into(), lock_wait: Duration::from_secs(20), ..Default::default() }).expect("takes over once released");
    assert!(t.elapsed() >= Duration::from_millis(500), "it had to wait");
    assert!(child.wait().unwrap().success());
    drop(db);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn start_gives_up_with_a_clear_message() {
    let root = tmp("timeout");
    let url = format!("surrealkv://{}/db", root.display());
    let mut child = spawn_holder(&url, 4000);
    wait_held(&url);
    let t = Instant::now();
    let e = match Db::connect(DbConfig { url: url.clone(), namespace: "t".into(), database: "t".into(), lock_wait: Duration::from_secs(1), ..Default::default() }) {
        Ok(_) => panic!("must not open a database another process holds"),
        Err(e) => e,
    };
    assert!(e.contains("is in use by another process (is the server still running?)"), "{e}");
    assert!(t.elapsed() < Duration::from_secs(3), "gave up after lock_wait");
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&root);
}
