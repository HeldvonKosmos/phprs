//! The connection: one SDK client per pool, a small Tokio runtime for
//! `block_on` from the worker threads (RUNTIME_ARCHITECTURE §9).

use std::collections::BTreeMap;
use std::time::Instant;

use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::method::Transaction;
use surrealdb::opt::auth::Root;
use surrealdb::types::{Object, Value};

#[derive(Clone, Debug)]
pub struct DbConfig {
    /// `mem://`, `rocksdb://path`, `ws://host:port`, `http://host:port`
    pub url: String,
    pub namespace: String,
    pub database: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl DbConfig {
    pub fn mem(namespace: &str, database: &str) -> Self {
        DbConfig { url: "mem://".into(), namespace: namespace.into(), database: database.into(), username: None, password: None }
    }

    /// `settings.opengrid.database.{url,namespace,database,username,password}`;
    /// `OPENGRID_DB_URL` overrides the url.
    pub fn from_settings(settings: Option<&serde_json::Value>) -> Option<DbConfig> {
        let d = settings?.get("opengrid")?.get("database")?;
        let s = |k: &str| d.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let url = std::env::var("OPENGRID_DB_URL").ok().or_else(|| s("url"))?;
        Some(DbConfig {
            url,
            namespace: s("namespace").unwrap_or_else(|| "opengrid".into()),
            database: s("database").unwrap_or_else(|| "opengrid".into()),
            username: s("username"),
            password: s("password"),
        })
    }
}

pub struct Db {
    rt: tokio::runtime::Runtime,
    client: Surreal<Any>,
    pub config: DbConfig,
}

impl Db {
    /// Connect (blocking). Must not be called on a Tokio thread.
    pub fn connect(config: DbConfig) -> Result<Db, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("grd-db")
            .enable_all()
            .build()
            .map_err(|e| format!("database runtime: {e}"))?;
        let cfg = config.clone();
        let client = rt
            .block_on(async move {
                let c = connect(cfg.url.as_str()).await?;
                if let (Some(u), Some(p)) = (&cfg.username, &cfg.password) {
                    c.signin(Root { username: u.clone(), password: p.clone() }).await?;
                }
                c.use_ns(cfg.namespace.as_str()).use_db(cfg.database.as_str()).await?;
                Ok::<_, surrealdb::Error>(c)
            })
            .map_err(|e| format!("database connect {}: {e}", config.url))?;
        tracing::info!(target: "grd", url = %config.url, ns = %config.namespace, db = %config.database, "database connected");
        Ok(Db { rt, client, config })
    }

    pub fn client(&self) -> &Surreal<Any> {
        &self.client
    }

    pub fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    /// Run a query; one `Value` per statement, the first failing statement
    /// is the error.
    pub fn query(&self, txn: Option<&Transaction<Any>>, sql: &str, vars: BTreeMap<String, Value>) -> Result<Vec<Value>, String> {
        let started = Instant::now();
        let result = self.rt.block_on(async {
            let mut obj = Object::new();
            for (k, v) in vars {
                obj.insert(k, v);
            }
            let mut resp = match txn {
                Some(t) => t.query(sql).bind(obj).await,
                None => self.client.query(sql).bind(obj).await,
            }
            .map_err(|e| e.to_string())?;
            let n = resp.num_statements();
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let v: Value = resp.take(i).map_err(|e| e.to_string())?;
                out.push(v);
            }
            Ok::<_, String>(out)
        });
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        match &result {
            Ok(_) => tracing::debug!(target: "grd", sql, ms, "surreal.query"),
            Err(e) => tracing::warn!(target: "grd", sql, ms, error = %e, "surreal.query failed"),
        }
        result
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
