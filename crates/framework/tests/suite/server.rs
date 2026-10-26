//! A throw-away SurrealDB server for the server-mode tests (B2c, D12): the
//! `surreal` binary (same version as the SDK) on a free port with in-memory
//! storage and a random root password passed only through the environment.
//! `OPENGRID_TEST_DB_URL` (+ `_USER`, `_PASS`) selects an existing server
//! instead. Without either the tests are skipped with a note.

use std::hash::{BuildHasher, Hasher};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use opengrid_framework::native::database::surreal::DbConfig;

pub struct TestServer {
    child: Option<Child>,
    url: String,
    user: Option<String>,
    pass: Option<String>,
}

impl TestServer {
    pub fn start() -> Option<TestServer> {
        if let Ok(url) = std::env::var("OPENGRID_TEST_DB_URL") {
            return Some(TestServer { child: None, url, user: std::env::var("OPENGRID_TEST_DB_USER").ok(), pass: std::env::var("OPENGRID_TEST_DB_PASS").ok() });
        }
        if Command::new("surreal").arg("version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true) {
            crate::oracle::missing(crate::oracle::Oracle::Surreal, "no `surreal` binary on PATH and OPENGRID_TEST_DB_URL not set");
            return None;
        }
        let port = std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).expect("free port").port();
        let random = |salt: u64| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(salt ^ std::process::id() as u64);
            h.finish()
        };
        let pass = format!("{:016x}{:016x}", random(1), random(2));
        let child = Command::new("surreal")
            .args(["start", "--bind", &format!("127.0.0.1:{port}"), "memory"])
            .env("SURREAL_USER", "root")
            .env("SURREAL_PASS", &pass)
            .env("SURREAL_LOG", "warn")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start surreal");
        let server = TestServer { child: Some(child), url: format!("ws://127.0.0.1:{port}"), user: Some("root".into()), pass: Some(pass) };
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "surreal did not listen on {port}");
            std::thread::sleep(Duration::from_millis(50));
        }
        Some(server)
    }

    /// A connection to its own database on the server.
    pub fn config(&self, database: &str) -> DbConfig {
        DbConfig {
            url: self.url.clone(),
            namespace: "opengrid_test".into(),
            database: format!("{database}_{}", std::process::id()),
            username: self.user.clone(),
            password: self.pass.clone(),
            ..Default::default()
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}
