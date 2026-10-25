//! J3: the scheduler inside the server process (D5). `routes/console.grd`
//! (any routes file) declares what runs when, Laravel 11 style:
//!
//! ```php
//! Schedule::task('notes:prune', ['days' => '30'])->dailyAt('03:00');
//! Schedule::job(SendDigest::class, ['a@example.com'])->everyFiveMinutes();
//! Schedule::task('migrate_push')->cron('0 * * * *');
//! ```
//! Every minute (UTC) the due entries run: tasks through `control::run_task`
//! on their own thread (a long task does not hold up the clock), jobs into
//! the job queue (J2). No child processes.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Timelike, Utc};

use crate::worker::Pool;

#[derive(Clone, Debug)]
pub struct Entry {
    /// "task" or "job"
    pub kind: String,
    /// task name (`notes:prune` → `run task:notes:prune`; or a built-in task) or job class
    pub target: String,
    pub args: serde_json::Value,
    pub cron: String,
}

/// The schedule as one worker sees it (all workers load the same files).
pub fn entries(pool: &Pool) -> Vec<Entry> {
    pool.run_sync(|vm| vm.host::<crate::worker::WorkerCtx>().map(|c| c.schedule.clone()).unwrap_or_default()).unwrap_or_default()
}

/// Run what is due at `now` (minute precision); returns what was started.
pub fn tick(pool: &Arc<Pool>, now: DateTime<Utc>) -> Vec<String> {
    let mut started = Vec::new();
    for e in entries(pool) {
        let Ok(cron) = e.cron.parse::<croner::Cron>() else {
            tracing::error!(target: "opengrid", cron = %e.cron, target = %e.target, "schedule: invalid cron expression");
            continue;
        };
        if !cron.is_time_matching(&now).unwrap_or(false) {
            continue;
        }
        started.push(format!("{} {}", e.kind, e.target));
        match e.kind.as_str() {
            "job" => {
                if let Err(err) = crate::native::jobs::push_job(pool.services(), e.target.clone(), e.args.clone()) {
                    tracing::error!(target: "opengrid", job = %e.target, "schedule: {err}");
                }
            }
            _ => {
                let (pool, e) = (Arc::clone(pool), e.clone());
                std::thread::spawn(move || {
                    let mut args: std::collections::BTreeMap<String, String> = e.args.as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect()).unwrap_or_default();
                    // `module:name` is a dialect task, anything else a built-in one
                    let name = if e.target.contains(':') {
                        args.insert("task".into(), e.target.clone());
                        "run".to_string()
                    } else {
                        e.target.clone()
                    };
                    match crate::control::run_task(&pool, &name, &args) {
                        Ok(out) => tracing::info!(target: "opengrid", task = %e.target, "scheduled task done: {}", out.trim_end()),
                        Err(err) => tracing::error!(target: "opengrid", task = %e.target, "scheduled task failed: {err}"),
                    }
                });
            }
        }
    }
    started
}

/// The clock: a thread that ticks at every full minute for the life of the pool.
pub fn start(pool: Arc<Pool>) {
    let weak = Arc::downgrade(&pool);
    drop(pool);
    std::thread::Builder::new()
        .name("opengrid-scheduler".into())
        .spawn(move || {
            loop {
                let now = Utc::now();
                let wait = 60 - u64::from(now.second());
                std::thread::sleep(Duration::from_secs(wait.max(1)));
                let Some(pool) = weak.upgrade() else { return };
                let at = Utc::now().with_second(0).and_then(|t| t.with_nanosecond(0)).unwrap_or_else(Utc::now);
                let started = tick(&pool, at);
                if !started.is_empty() {
                    tracing::info!(target: "opengrid", ?started, "schedule");
                }
            }
        })
        .expect("spawn scheduler");
}
