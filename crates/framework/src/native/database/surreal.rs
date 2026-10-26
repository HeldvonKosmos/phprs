//! The connection: one SDK client per pool, a small Tokio runtime for
//! `block_on` from the worker threads (RUNTIME_ARCHITECTURE §9).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::method::Transaction;
use surrealdb::opt::auth::Root;
use surrealdb::types::{Object, Value};

/// How often a retriable write conflict is retried (see [`Db::query`]).
const MAX_CONFLICT_RETRIES: u32 = 20;

/// SurrealDB marks optimistic transaction conflicts as retriable.
pub(crate) fn is_retriable(error: &str) -> bool {
    error.contains("can be retried")
}

#[derive(Clone, Debug, Default)]
pub struct DbConfig {
    /// `mem://`, `surrealkv://path` (embedded, persistent), `ws://host:port`,
    /// `http://host:port` (server). RocksDB and TiKV are not built in
    /// (DECISIONS DB1: surrealkv covers the embedded case).
    pub url: String,
    pub namespace: String,
    pub database: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// How long `Db::connect` waits for an embedded database that another
    /// process still holds (the old version during a deployment). Zero: fail
    /// at once (CLI tasks, tests).
    pub lock_wait: Duration,
}

/// Embedded database of a project when nothing is configured (DB1).
pub const DEFAULT_DB_DIR: &str = ".opengrid/db";
/// Default `lock_wait` of the server start.
pub const DEFAULT_LOCK_WAIT: Duration = Duration::from_secs(30);

impl DbConfig {
    pub fn mem(namespace: &str, database: &str) -> Self {
        DbConfig { url: "mem://".into(), namespace: namespace.into(), database: database.into(), ..Default::default() }
    }

    /// `surrealkv://<root>/.opengrid/db`, namespace and database `opengrid`.
    pub fn embedded_default(root: &Path) -> Self {
        DbConfig { url: format!("surrealkv://{}", root.join(DEFAULT_DB_DIR).display()), namespace: "opengrid".into(), database: "opengrid".into(), ..Default::default() }
    }

    /// Embedded path of a `surrealkv://` URL, if any.
    pub fn embedded_path(&self) -> Option<&str> {
        self.url.strip_prefix("surrealkv://")
    }

    pub fn is_embedded(&self) -> bool {
        self.url.starts_with("surrealkv://") || self.url.starts_with("mem://")
    }

    /// Resolve a relative `surrealkv://` path against the project root, so
    /// the database does not move with the working directory.
    pub fn resolve(mut self, root: &Path) -> Self {
        if let Some(p) = self.embedded_path()
            && !p.is_empty()
            && !Path::new(p).is_absolute()
        {
            self.url = format!("surrealkv://{}", root.join(p).display());
        }
        self
    }

    /// Connection from environment and settings (R4e). Order:
    /// 1. `OPENGRID_DB_URL` (with the rest from the settings);
    /// 2. the platform variables `SURREAL_DB_HOST`, `_PORT`, `_NAMESPACE`,
    ///    `_DATABASE`, `_USERNAME`, `_PASSWORD` — server mode; defaults
    ///    localhost, 8000, main, main, root, root, so an existing platform
    ///    `.env` works unchanged; only `SURREAL_DB_HOST` switches it on;
    /// 3. `settings.opengrid.database.{url,namespace,database,username,password}`
    ///    (embedded `mem://` / `surrealkv://…` or a server URL).
    pub fn from_settings(settings: Option<&serde_json::Value>) -> Option<DbConfig> {
        Self::from_sources(|k| std::env::var(k).ok(), settings)
    }

    pub fn from_sources(env: impl Fn(&str) -> Option<String>, settings: Option<&serde_json::Value>) -> Option<DbConfig> {
        let d = settings.and_then(|s| s.get("opengrid")).and_then(|o| o.get("database"));
        let s = |k: &str| d.and_then(|d| d.get(k)).and_then(|v| v.as_str()).map(str::to_string);
        if let Some(url) = env("OPENGRID_DB_URL") {
            return Some(DbConfig {
                url,
                namespace: s("namespace").unwrap_or_else(|| "opengrid".into()),
                database: s("database").unwrap_or_else(|| "opengrid".into()),
                username: s("username"),
                password: s("password"),
                ..Default::default()
            });
        }
        if let Some(host) = env("SURREAL_DB_HOST") {
            let v = |k: &str, default: &str| env(k).filter(|x| !x.is_empty()).unwrap_or_else(|| default.to_string());
            let host = if host.is_empty() { "localhost".to_string() } else { host };
            let url = if host.contains("://") { host } else { format!("ws://{host}:{}", v("SURREAL_DB_PORT", "8000")) };
            return Some(DbConfig {
                url,
                namespace: v("SURREAL_DB_NAMESPACE", "main"),
                database: v("SURREAL_DB_DATABASE", "main"),
                username: Some(v("SURREAL_DB_USERNAME", "root")),
                password: Some(v("SURREAL_DB_PASSWORD", "root")),
                ..Default::default()
            });
        }
        Some(DbConfig {
            url: s("url")?,
            namespace: s("namespace").unwrap_or_else(|| "opengrid".into()),
            database: s("database").unwrap_or_else(|| "opengrid".into()),
            username: s("username"),
            password: s("password"),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod config_tests {
    use super::DbConfig;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn platform_variables_select_server_mode() {
        let c = DbConfig::from_sources(env(&[("SURREAL_DB_HOST", "db.internal")]), None).unwrap();
        assert_eq!((c.url.as_str(), c.namespace.as_str(), c.database.as_str()), ("ws://db.internal:8000", "main", "main"));
        assert_eq!((c.username.as_deref(), c.password.as_deref()), (Some("root"), Some("root")));
        let c = DbConfig::from_sources(env(&[("SURREAL_DB_HOST", "h"), ("SURREAL_DB_PORT", "30801"), ("SURREAL_DB_NAMESPACE", "n"), ("SURREAL_DB_DATABASE", "d"), ("SURREAL_DB_USERNAME", "u"), ("SURREAL_DB_PASSWORD", "p")]), None).unwrap();
        assert_eq!((c.url.as_str(), c.namespace.as_str(), c.database.as_str(), c.username.as_deref(), c.password.as_deref()), ("ws://h:30801", "n", "d", Some("u"), Some("p")));
    }

    #[test]
    fn settings_and_override() {
        let settings = serde_json::json!({"opengrid": {"database": {"url": "surrealkv://data/db", "namespace": "app"}}});
        let c = DbConfig::from_sources(env(&[]), Some(&settings)).unwrap();
        assert_eq!((c.url.as_str(), c.namespace.as_str(), c.database.as_str()), ("surrealkv://data/db", "app", "opengrid"));
        let c = DbConfig::from_sources(env(&[("OPENGRID_DB_URL", "mem://"), ("SURREAL_DB_HOST", "ignored")]), Some(&settings)).unwrap();
        assert_eq!(c.url, "mem://");
        assert!(DbConfig::from_sources(env(&[]), None).is_none());
    }

    #[test]
    fn embedded_paths() {
        let root = std::path::Path::new("/srv/app");
        assert_eq!(DbConfig::embedded_default(root).url, "surrealkv:///srv/app/.opengrid/db");
        let rel = DbConfig { url: "surrealkv://data/db".into(), ..Default::default() }.resolve(root);
        assert_eq!(rel.url, "surrealkv:///srv/app/data/db");
        let abs = DbConfig { url: "surrealkv:///var/db".into(), ..Default::default() }.resolve(root);
        assert_eq!(abs.url, "surrealkv:///var/db");
        assert_eq!(DbConfig::mem("a", "b").resolve(root).url, "mem://");
        assert!(DbConfig::embedded_default(root).is_embedded() && !DbConfig { url: "ws://h:1".into(), ..Default::default() }.is_embedded());
    }
}

pub struct Db {
    rt: tokio::runtime::Runtime,
    client: Surreal<Any>,
    pub config: DbConfig,
    /// Write conflicts retried so far (queries and `DB::transaction`);
    /// lets tests show that conflicts happened at all.
    retries: std::sync::atomic::AtomicU64,
}

impl Db {
    /// Connect (blocking). Must not be called on a Tokio thread.
    ///
    /// An embedded `surrealkv://` database is held by one process at a time
    /// (its file lock lasts until that process exits). If another process
    /// holds it, retry until `config.lock_wait` has passed — that is the
    /// hand-over during a deployment — then fail with a clear message.
    pub fn connect(config: DbConfig) -> Result<Db, String> {
        // DBP1: threads of the database runtime (the embedded engine runs its
        // work here); OPENGRID_DB_THREADS for measurements, default 2
        let threads = std::env::var("OPENGRID_DB_THREADS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0).unwrap_or(2);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(threads)
            .thread_name("grd-db")
            .enable_all()
            .build()
            .map_err(|e| format!("database runtime: {e}"))?;
        if let Some(p) = config.embedded_path()
            && let Some(parent) = Path::new(p).parent()
        {
            std::fs::create_dir_all(parent).map_err(|e| format!("database directory {}: {e}", parent.display()))?;
        }
        let started = Instant::now();
        let mut announced = false;
        let client = loop {
            let cfg = config.clone();
            let attempt = rt.block_on(async move {
                let c = connect(cfg.url.as_str()).await?;
                if let (Some(u), Some(p)) = (&cfg.username, &cfg.password) {
                    c.signin(Root { username: u.clone(), password: p.clone() }).await?;
                }
                c.use_ns(cfg.namespace.as_str()).use_db(cfg.database.as_str()).await?;
                Ok::<_, surrealdb::Error>(c)
            });
            match attempt {
                Ok(c) => break c,
                Err(e) if is_locked(&e.to_string()) => {
                    if started.elapsed() >= config.lock_wait {
                        return Err(format!(
                            "database {} is in use by another process (is the server still running?); waited {}s",
                            config.embedded_path().unwrap_or(&config.url),
                            config.lock_wait.as_secs()
                        ));
                    }
                    if !announced {
                        tracing::info!(target: "opengrid", path = config.embedded_path().unwrap_or(&config.url), wait_secs = config.lock_wait.as_secs(), "database is held by another process, waiting for it to be released");
                        announced = true;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                Err(e) => return Err(format!("database connect {}: {e}", config.url)),
            }
        };
        let db = Db { rt, client, config, retries: Default::default() };
        db.log_startup();
        Ok(db)
    }

    /// For the health endpoint (OPS1): mode (`embedded`/`server`) and the
    /// version the database answers with (`None`: it does not answer).
    pub fn status(&self) -> (&'static str, Option<String>) {
        let mode = if self.config.is_embedded() { "embedded" } else { "server" };
        let version = self.rt.block_on(async { self.client.version().await.ok().map(|v| v.to_string()) });
        (mode, version)
    }

    /// Mode, engine, location and versions; in server mode a warning when the
    /// server's major version differs from the built-in engine's.
    fn log_startup(&self) {
        let engine = self.rt.block_on(async {
            let e = connect("mem://").await.ok()?;
            e.version().await.ok().map(|v| v.to_string())
        });
        let connected = self.rt.block_on(async { self.client.version().await.ok().map(|v| v.to_string()) });
        let mode = if self.config.is_embedded() { "embedded" } else { "server" };
        let location = self.config.embedded_path().unwrap_or(&self.config.url).to_string();
        tracing::info!(
            target: "opengrid",
            mode,
            location = %location,
            ns = %self.config.namespace,
            db = %self.config.database,
            version = connected.as_deref().unwrap_or("?"),
            "database connected"
        );
        if mode == "server"
            && let (Some(server), Some(engine)) = (&connected, &engine)
            && major(server) != major(engine)
        {
            tracing::warn!(target: "opengrid", server = %server, sdk = %engine, "SurrealDB server major version differs from the built-in SDK");
        }
    }

    pub fn client(&self) -> &Surreal<Any> {
        &self.client
    }

    pub fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    /// Count one retried write conflict.
    pub fn note_retry(&self) {
        self.retries.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Write conflicts retried on this connection so far.
    pub fn conflict_retries(&self) -> u64 {
        self.retries.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Run a query; one `Value` per statement, the first failing statement
    /// is the error. Outside an explicit transaction a write conflict that
    /// SurrealDB reports as retriable (optimistic transactions, e.g. two
    /// requests updating the same token's `last_used_at`) is retried when it
    /// hit the first statement, so nothing has been committed yet.
    pub fn query(&self, txn: Option<&Transaction<Any>>, sql: &str, vars: BTreeMap<String, Value>) -> Result<Vec<Value>, String> {
        let started = Instant::now();
        let mut attempt = 0u32;
        let result = loop {
            match self.query_once(txn, sql, vars.clone()) {
                Err((0, e)) if txn.is_none() && attempt < MAX_CONFLICT_RETRIES && is_retriable(&e) => {
                    attempt += 1;
                    self.note_retry();
                    std::thread::sleep(std::time::Duration::from_micros(200 * u64::from(attempt)));
                }
                other => break other.map_err(|(_, e)| e),
            }
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        match &result {
            Ok(_) => tracing::debug!(target: "opengrid", sql, ms, retries = attempt, "surreal.query"),
            Err(e) => tracing::warn!(target: "opengrid", sql, ms, retries = attempt, error = %e, "surreal.query failed"),
        }
        result
    }

    /// One attempt; the error carries the index of the failing statement.
    fn query_once(&self, txn: Option<&Transaction<Any>>, sql: &str, vars: BTreeMap<String, Value>) -> Result<Vec<Value>, (usize, String)> {
        self.rt.block_on(async {
            let mut obj = Object::new();
            for (k, v) in vars {
                obj.insert(k, v);
            }
            let mut resp = match txn {
                Some(t) => t.query(sql).bind(obj).await,
                None => self.client.query(sql).bind(obj).await,
            }
            .map_err(|e| (0, e.to_string()))?;
            let n = resp.num_statements();
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let v: Value = resp.take(i).map_err(|e| (i, e.to_string()))?;
                out.push(v);
            }
            Ok(out)
        })
    }

    pub fn begin(&self) -> Result<Transaction<Any>, String> {
        let client = self.client.clone();
        self.rt.block_on(async move { client.begin().await }).map_err(|e| e.to_string())
    }

    pub fn commit(&self, txn: Transaction<Any>) -> Result<(), String> {
        self.rt.block_on(async move { txn.commit().await }).map(|_| ()).map_err(|e| e.to_string())
    }

    pub fn cancel(&self, txn: Transaction<Any>) -> Result<(), String> {
        self.rt.block_on(async move { txn.cancel().await }).map(|_| ()).map_err(|e| e.to_string())
    }
}

fn is_locked(message: &str) -> bool {
    message.contains("already locked")
}

/// Major version of `surrealdb-3.3.4`, `3.3.4` or `v3.3.4`.
fn major(v: &str) -> Option<u64> {
    v.trim_start_matches(|c: char| !c.is_ascii_digit()).split('.').next()?.parse().ok()
}

#[cfg(test)]
mod conflict_tests {
    use super::*;

    /// Concurrent writes to one record (as concurrent requests with one
    /// bearer token do with `last_used_at`) all succeed.
    #[test]
    fn concurrent_updates_of_one_record_are_retried() {
        let dir = std::env::temp_dir().join(format!("og-conflict-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = DbConfig { url: format!("surrealkv://{}", dir.display()), ..DbConfig::mem("t", "t") };
        let db = std::sync::Arc::new(Db::connect(cfg).expect("connect"));
        db.query(None, "CREATE t:1 SET n = 0", BTreeMap::new()).expect("create");
        let threads: Vec<_> = (0..8)
            .map(|w| {
                let db = db.clone();
                std::thread::spawn(move || {
                    (0..40).filter(|i| db.query(None, "UPDATE t:1 MERGE { n: $n }", BTreeMap::from([("n".to_string(), Value::from_t(w * 100 + i))])).is_err()).count()
                })
            })
            .collect();
        let failed: usize = threads.into_iter().map(|t| t.join().unwrap()).sum();
        let last = db.query(None, "SELECT VALUE n FROM ONLY t:1", BTreeMap::new()).expect("read").remove(0);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(failed, 0, "write conflicts reached the caller");
        // the conflicts happened and were retried, not avoided
        assert!(db.conflict_retries() > 0, "no write conflict occurred, the retry path did not run");
        // the record holds the last write of one of the threads
        let n: i64 = last.into_t().expect("int");
        assert!((0..8).any(|w| n == w * 100 + 39), "final value {n} is not a thread's last write");
    }
}
