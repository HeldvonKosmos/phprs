//! J2: background jobs in the server process (D5). `Job::dispatch(...$args)`
//! (prelude_queue.grd) calls `opengrid_job_dispatch($class, $args)`: the
//! arguments are kept as JSON and the job goes into the pool's own task
//! channel, so some worker runs it after its current work, outside any
//! request and with its own object arena (the request boundary). A failing
//! job is logged and run again up to `$tries` times.
//!
//! J4: with a database, jobs are durable, as Laravel's database queue: a row
//! in `jobs` (queue, class, args, attempts, max_tries, backoff, reserved_at,
//! available_at, created_at). A worker claims a row with a conditional
//! update (`reserved_at = NONE`), so a job runs once; success deletes the
//! row, a failure either releases it for another try after `$backoff`
//! seconds or moves it to `failed_jobs` (with the exception). Only the
//! server runs them (`start_runner`): it releases reservations left by a
//! crash and polls every second for due rows (delayed jobs, retries, rows
//! written in a transaction before it committed). Task pools only write rows.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, Val};
use grd_vm::vm::Vm;
use surrealdb::types::{Datetime, Object, RecordId, RecordIdKey, Value};

use super::ctx;
use super::database::query;
use super::database::value::{from_surreal, to_surreal};
use crate::services::Services;

pub const JOBS: &str = "jobs";
pub const FAILED_JOBS: &str = "failed_jobs";

/// Build and run a job: Err((message, tries)); tries is 1 when the job
/// could not even be built.
fn execute(vm: &mut Vm, class: &str, args: Vec<Val>) -> Result<(), (String, u32)> {
    let fail = |m: String| (m, 1);
    let id = vm.lookup_class(class.as_bytes()).ok_or_else(|| fail(format!("job class {class} not found")))?;
    let base = vm.lookup_class(b"OpenGrid\\Queue\\Job").ok_or_else(|| fail("Job class missing".into()))?;
    if !vm.instance_of(id, base) {
        return Err(fail(format!("{class} does not extend OpenGrid\\Queue\\Job")));
    }
    let job = match vm.instantiate(id, args) {
        Ok(j) => j,
        Err(t) => return Err(fail(describe(vm, t))),
    };
    let tries = match vm.call_method_sync(job, b"maxTries", Vec::new()) {
        Ok(Val::Int(n)) => n.max(1) as u32,
        _ => 1,
    };
    match vm.call_method_sync_gc_safe(job, b"handle", Vec::new()) {
        Ok(_) => Ok(()),
        Err(t) => Err((describe(vm, t), tries)),
    }
}

/// "Class: message" of a throw; a dialect exception keeps both in its object.
fn describe(vm: &mut Vm, t: Throw) -> String {
    match vm.describe_throw(t) {
        grd_vm::vm::RunError::Uncaught { class, message, .. } => format!("{class}: {message}"),
        other => other.to_string(),
    }
}

fn leave_request(vm: &mut Vm) {
    if let Some(c) = vm.host::<crate::worker::WorkerCtx>() {
        c.request = None;
    }
}

fn run_job(vm: &mut Vm, class: &str, args: &serde_json::Value, attempt: u32) {
    leave_request(vm);
    let result = match grd_vm::json::from_json(vm, args.clone()) {
        Ok(Val::Array(a)) => execute(vm, class, a.values().cloned().collect()),
        Ok(_) => execute(vm, class, Vec::new()),
        Err(t) => Err((t.message, 1)),
    };
    let services = ctx(vm).services.clone();
    match result {
        Ok(()) => {
            services.jobs.done.fetch_add(1, Ordering::SeqCst);
        }
        Err((msg, tries)) if attempt < tries => {
            tracing::warn!(target: "opengrid", job = class, attempt, tries, "job failed, retrying: {msg}");
            let (class, args) = (class.to_string(), args.clone());
            if services.jobs.push(Box::new(move |vm| run_job(vm, &class, &args, attempt + 1))).is_err() {
                services.jobs.failed.fetch_add(1, Ordering::SeqCst);
            }
        }
        Err((msg, _)) => {
            tracing::error!(target: "opengrid", job = class, attempt, "job failed: {msg}");
            services.jobs.failed.fetch_add(1, Ordering::SeqCst);
        }
    }
    services.jobs.pending.fetch_sub(1, Ordering::SeqCst);
    vm.reset_request();
}

/// Queue a job by class name and JSON arguments (the scheduler, J3). With a
/// database the job becomes a row first (J4), written by a worker.
pub fn push_job(services: &crate::services::Services, class: String, args: serde_json::Value) -> Result<(), String> {
    services.jobs.push(Box::new(move |vm| {
        if ctx(vm).db.is_some() {
            let spec = grd_vm::json::from_json(vm, args.clone()).map(|a| Spec { class: class.clone(), args: a, tries: 1, backoff: 0, delay: 0, queue: "default".into() });
            let done = spec.map_err(|t| t.message).and_then(|s| dispatch_durable(vm, s).map_err(|t| t.message));
            if let Err(e) = done {
                tracing::error!(target: "opengrid", job = %class, "job could not be queued: {e}");
            }
            ctx(vm).services.jobs.pending.fetch_sub(1, Ordering::SeqCst);
        } else {
            run_job(vm, &class, &args, 1)
        }
    }))
}

// ---- J4: durable jobs -------------------------------------------------------

/// What `Job::dispatch` hands over.
pub struct Spec {
    pub class: String,
    pub args: Val,
    pub tries: u32,
    pub backoff: i64,
    pub delay: i64,
    pub queue: String,
}

fn rid(table: &str, id: &str) -> Value {
    Value::RecordId(RecordId::new(table, RecordIdKey::String(id.to_string())))
}

fn at(secs: i64) -> Value {
    Value::Datetime(Datetime::from(chrono::Utc::now() + chrono::Duration::seconds(secs.max(0))))
}

fn int(vm: &mut Vm, n: i64) -> Result<Value, Throw> {
    to_surreal(vm, &Val::Int(n))
}

/// A table that was never written does not exist yet: read as empty.
fn missing_table(t: &Throw) -> bool {
    t.message.contains("does not exist")
}

/// Write the row; run it soon unless it is delayed (or no runner is here).
pub fn dispatch_durable(vm: &mut Vm, spec: Spec) -> Result<(), Throw> {
    let id = crate::session::new_id();
    let mut row = Object::new();
    row.insert("queue", Value::String(spec.queue));
    row.insert("class", Value::String(spec.class));
    row.insert("args", to_surreal(vm, &spec.args)?);
    row.insert("attempts", int(vm, 0)?);
    row.insert("max_tries", int(vm, spec.tries.max(1) as i64)?);
    row.insert("backoff", int(vm, spec.backoff.max(0))?);
    row.insert("available_at", at(spec.delay));
    row.insert("created_at", Value::Datetime(Datetime::now()));
    let mut vars = BTreeMap::new();
    vars.insert("rid".to_string(), rid(JOBS, &id));
    vars.insert("data".to_string(), Value::Object(row));
    query(vm, "CREATE $rid CONTENT $data", vars)?;
    if spec.delay <= 0 {
        let services = ctx(vm).services.clone();
        enqueue(&services, id);
    }
    Ok(())
}

/// Hand a row to a worker (once at a time), if this pool runs jobs.
fn enqueue(services: &Arc<Services>, id: String) {
    if !services.jobs.runner.load(Ordering::SeqCst) || !services.jobs.claim_slot(&id) {
        return;
    }
    let job_id = id.clone();
    if services.jobs.push(Box::new(move |vm| run_durable(vm, &job_id))).is_err() {
        services.jobs.release_slot(&id);
    }
}

fn field(row: &Val, name: &str) -> Val {
    match row {
        Val::Array(a) => a.get(&ArrayKey::from_str(name)).cloned().unwrap_or(Val::Null),
        _ => Val::Null,
    }
}

fn run_durable(vm: &mut Vm, id: &str) {
    leave_request(vm);
    let services = ctx(vm).services.clone();
    if let Err(e) = claim_and_run(vm, id, &services) {
        tracing::error!(target: "opengrid", job = id, "durable job: {}", e.message);
    }
    services.jobs.release_slot(id);
    services.jobs.pending.fetch_sub(1, Ordering::SeqCst);
    vm.reset_request();
}

fn claim_and_run(vm: &mut Vm, id: &str, services: &Arc<Services>) -> Result<(), Throw> {
    let mut vars = BTreeMap::new();
    vars.insert("rid".to_string(), rid(JOBS, id));
    let claimed = query(vm, "UPDATE $rid SET reserved_at = time::now(), attempts += 1 WHERE reserved_at = NONE AND available_at <= time::now() RETURN AFTER", vars.clone())?;
    let Some(Value::Array(rows)) = claimed.into_iter().next() else { return Ok(()) };
    let Some(row) = rows.into_inner().into_iter().next() else { return Ok(()) }; // taken, done or not due
    let row = from_surreal(vm, row)?;
    let class = field(&row, "class").to_php_string().map(|s| s.to_string_lossy()).unwrap_or_default();
    let args: Vec<Val> = match field(&row, "args") {
        Val::Array(a) => a.values().cloned().collect(),
        _ => Vec::new(),
    };
    let num = |v: Val| if let Val::Int(n) = v { n } else { 0 };
    let (attempts, max_tries, backoff) = (num(field(&row, "attempts")), num(field(&row, "max_tries")).max(1), num(field(&row, "backoff")));
    match execute(vm, &class, args) {
        Ok(()) => {
            query(vm, "DELETE $rid", vars)?;
            services.jobs.done.fetch_add(1, Ordering::SeqCst);
        }
        Err((msg, _)) if attempts < max_tries => {
            tracing::warn!(target: "opengrid", job = %class, attempts, max_tries, "job failed, retrying: {msg}");
            vars.insert("at".to_string(), at(backoff));
            query(vm, "UPDATE $rid SET reserved_at = NONE, available_at = $at", vars)?;
        }
        Err((msg, _)) => {
            tracing::error!(target: "opengrid", job = %class, attempts, "job failed: {msg}");
            let mut failed = Object::new();
            let uuid = crate::session::new_id();
            failed.insert("uuid", Value::String(uuid.clone()));
            failed.insert("job", Value::String(id.to_string()));
            failed.insert("queue", to_surreal(vm, &field(&row, "queue"))?);
            failed.insert("class", Value::String(class));
            failed.insert("args", to_surreal(vm, &field(&row, "args"))?);
            failed.insert("attempts", int(vm, attempts)?);
            failed.insert("max_tries", int(vm, max_tries)?);
            failed.insert("backoff", int(vm, backoff)?);
            failed.insert("exception", Value::String(msg));
            failed.insert("failed_at", Value::Datetime(Datetime::now()));
            vars.insert("fid".to_string(), rid(FAILED_JOBS, &uuid));
            vars.insert("data".to_string(), Value::Object(failed));
            query(vm, "BEGIN TRANSACTION; CREATE $fid CONTENT $data; DELETE $rid; COMMIT TRANSACTION;", vars)?;
            services.jobs.failed.fetch_add(1, Ordering::SeqCst);
        }
    }
    Ok(())
}

fn count(vm: &mut Vm, table: &str) -> Result<i64, Throw> {
    let rows = match query(vm, &format!("SELECT count() FROM {table} GROUP ALL"), BTreeMap::new()) {
        Ok(r) => r,
        Err(e) if missing_table(&e) => return Ok(0),
        Err(e) => return Err(e),
    };
    let first = rows.into_iter().next().unwrap_or(Value::None);
    let v = from_surreal(vm, first)?;
    Ok(match v {
        Val::Array(a) => a.values().next().map(|r| match field(r, "count") {
            Val::Int(n) => n,
            _ => 0,
        }).unwrap_or(0),
        _ => 0,
    })
}

fn key_of(v: &Value) -> Option<String> {
    match v {
        Value::RecordId(r) => match &r.key {
            RecordIdKey::String(s) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// Hand every due, unreserved row to a worker; `recover` first releases
/// reservations (a server that starts owns no running job).
fn poll(vm: &mut Vm, recover: bool) {
    let services = ctx(vm).services.clone();
    if recover && let Err(e) = query(vm, "UPDATE jobs SET reserved_at = NONE WHERE reserved_at != NONE", BTreeMap::new())
        && !missing_table(&e)
    {
        tracing::warn!(target: "opengrid", "jobs: releasing reservations failed: {}", e.message);
    }
    let rows = match query(vm, "SELECT id, available_at FROM jobs WHERE reserved_at = NONE AND available_at <= time::now() ORDER BY available_at LIMIT 500", BTreeMap::new()) {
        Ok(r) => r,
        Err(e) if missing_table(&e) => return,
        Err(e) => {
            tracing::warn!(target: "opengrid", "jobs: poll failed: {}", e.message);
            return;
        }
    };
    if let Some(Value::Array(rows)) = rows.into_iter().next() {
        for row in rows.into_inner() {
            if let Value::Object(o) = row
                && let Some(id) = o.get("id").and_then(key_of)
            {
                enqueue(&services, id);
            }
        }
    }
    vm.reset_request();
}

/// J4 tasks, Laravel's `queue:failed`, `queue:retry`, `queue:flush`:
/// `queue_failed`, `queue_retry id:<uuid>|all`, `queue_flush`.
pub fn queue_task(pool: &crate::worker::Pool, name: &str, args: &BTreeMap<String, String>) -> Result<String, String> {
    if pool.options().database.is_none() {
        return Err(format!("{name}: jobs are durable only with a database"));
    }
    let (name, args) = (name.to_string(), args.clone());
    pool.run_sync(move |vm| {
        let r = queue_task_in(vm, &name, &args).map_err(|t| t.message);
        vm.reset_request();
        r
    })
    .map_err(|e| format!("{e:?}"))?
}

fn failed_rows(vm: &mut Vm, id: Option<&str>) -> Result<Vec<Val>, Throw> {
    let mut vars = BTreeMap::new();
    let sql = match id {
        Some(u) => {
            vars.insert("uuid".to_string(), Value::String(u.to_string()));
            "SELECT * FROM failed_jobs WHERE uuid = $uuid"
        }
        None => "SELECT * FROM failed_jobs ORDER BY failed_at",
    };
    let rows = match query(vm, sql, vars) {
        Ok(r) => r,
        Err(e) if missing_table(&e) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let first = rows.into_iter().next().unwrap_or(Value::None);
    Ok(match from_surreal(vm, first)? {
        Val::Array(a) => a.values().cloned().collect(),
        _ => Vec::new(),
    })
}

fn text(v: Val) -> String {
    v.to_php_string().map(|s| s.to_string_lossy()).unwrap_or_default()
}

fn queue_task_in(vm: &mut Vm, name: &str, args: &BTreeMap<String, String>) -> Result<String, Throw> {
    match name {
        "queue_failed" => {
            let mut out = String::new();
            for r in failed_rows(vm, None)? {
                out.push_str(&format!("{}  {}  {}  {}\n", text(field(&r, "uuid")), text(field(&r, "class")), text(field(&r, "failed_at")), text(field(&r, "exception"))));
            }
            Ok(if out.is_empty() { "no failed jobs\n".into() } else { out })
        }
        "queue_retry" => {
            let which = args.get("id").map(String::as_str).ok_or_else(|| Throw::error("queue_retry needs id:<uuid> or id:all"))?;
            let rows = failed_rows(vm, if which == "all" { None } else { Some(which) })?;
            if rows.is_empty() {
                return Err(Throw::error(format!("no failed job {which}")));
            }
            let n = rows.len();
            for r in rows {
                let num = |v: Val| if let Val::Int(n) = v { n } else { 0 };
                let spec = Spec {
                    class: text(field(&r, "class")),
                    args: field(&r, "args"),
                    tries: num(field(&r, "max_tries")).max(1) as u32,
                    backoff: num(field(&r, "backoff")),
                    delay: 0,
                    queue: text(field(&r, "queue")),
                };
                dispatch_durable(vm, spec)?;
                let mut vars = BTreeMap::new();
                vars.insert("uuid".to_string(), Value::String(text(field(&r, "uuid"))));
                query(vm, "DELETE failed_jobs WHERE uuid = $uuid", vars)?;
            }
            Ok(format!("{n} job(s) queued again\n"))
        }
        "queue_flush" => {
            let n = failed_rows(vm, None)?.len();
            if n > 0 {
                query(vm, "DELETE failed_jobs", BTreeMap::new())?;
            }
            Ok(format!("{n} failed job(s) deleted\n"))
        }
        other => Err(Throw::error(format!("unknown task `{other}`"))),
    }
}

/// J4: this pool runs durable jobs: release old reservations, then poll
/// every `interval` for due rows, for as long as the pool lives. Without a
/// database nothing happens (jobs stay in memory, J2).
pub fn start_runner(pool: &crate::worker::Pool, interval: std::time::Duration) {
    if pool.options().database.is_none() {
        return;
    }
    let services = pool.services_arc();
    services.jobs.runner.store(true, Ordering::SeqCst);
    let weak = Arc::downgrade(&services);
    drop(services);
    std::thread::Builder::new()
        .name("opengrid-jobs".into())
        .spawn(move || {
            let mut recover = true;
            loop {
                let Some(services) = weak.upgrade() else { return };
                let first = recover;
                if services.jobs.push_uncounted(Box::new(move |vm| poll(vm, first))).is_err() {
                    return;
                }
                recover = false;
                drop(services);
                std::thread::sleep(interval);
            }
        })
        .expect("spawn job runner");
}

pub fn register(vm: &mut Vm) {
    vm.register_native("opengrid_job_dispatch", |vm, a| {
        let class = match a.first() {
            Some(Val::Str(s)) => s.to_string_lossy(),
            _ => return Err(Throw::type_error("opengrid_job_dispatch(): class name expected")),
        };
        let raw = a.get(1).cloned().unwrap_or(Val::empty_array());
        // J4: [tries, backoff, delay, queue] from the job object
        let opt = |i: usize| match a.get(i) {
            Some(Val::Int(n)) => *n,
            _ => 0,
        };
        if ctx(vm).db.is_some() {
            let queue = match a.get(5) {
                Some(Val::Str(s)) => s.to_string_lossy(),
                _ => "default".to_string(),
            };
            // JSON round trip: only plain values go into the row
            let bytes = grd_vm::json::to_json(vm, &raw, false)?;
            let json: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| Throw::error(format!("job arguments: {e}")))?;
            let args = grd_vm::json::from_json(vm, json)?;
            dispatch_durable(vm, Spec { class, args, tries: opt(2).max(1) as u32, backoff: opt(3), delay: opt(4), queue })?;
            return Ok(Val::Null);
        }
        let bytes = grd_vm::json::to_json(vm, &raw, false)?;
        let args: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| Throw::error(format!("job arguments: {e}")))?;
        let services = ctx(vm).services.clone();
        services.jobs.push(Box::new(move |vm| run_job(vm, &class, &args, 1))).map_err(Throw::error)?;
        Ok(Val::Null)
    });
    // J3: entries of the schedule (prelude_schedule.grd)
    vm.register_native("opengrid_schedule_add", |vm, a| {
        let kind = match a.first() { Some(Val::Str(s)) => s.to_string_lossy(), _ => return Err(Throw::type_error("schedule kind expected")) };
        let target = match a.get(1) { Some(Val::Str(s)) => s.to_string_lossy(), _ => return Err(Throw::type_error("schedule target expected")) };
        let bytes = grd_vm::json::to_json(vm, &a.get(2).cloned().unwrap_or(Val::empty_array()), false)?;
        let args: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| Throw::error(format!("schedule arguments: {e}")))?;
        let schedule = &mut ctx(vm).schedule;
        schedule.push(crate::scheduler::Entry { kind, target, args, cron: "* * * * *".into() });
        Ok(Val::Int(schedule.len() as i64 - 1))
    });
    vm.register_native("opengrid_schedule_set", |vm, a| {
        let id = match a.first() { Some(Val::Int(i)) => *i as usize, _ => return Err(Throw::type_error("schedule id expected")) };
        let cron = match a.get(1) { Some(Val::Str(s)) => s.to_string_lossy(), _ => return Err(Throw::type_error("cron expression expected")) };
        if cron.parse::<croner::Cron>().is_err() {
            return Err(Throw::value_error(format!("invalid cron expression `{cron}`")));
        }
        let entry = ctx(vm).schedule.get_mut(id).ok_or_else(|| Throw::error("unknown schedule entry"))?;
        entry.cron = cron;
        Ok(Val::Null)
    });
    // [pending, done, failed]: with a database (J4) pending and failed are the
    // rows of `jobs` and `failed_jobs`, done counts this process
    vm.register_native("opengrid_job_stats", |vm, _| {
        let j = &ctx(vm).services.jobs;
        let (mut p, d, mut f) = (j.pending.load(Ordering::SeqCst) as i64, j.done.load(Ordering::SeqCst) as i64, j.failed.load(Ordering::SeqCst) as i64);
        if ctx(vm).db.is_some() {
            p = count(vm, JOBS)?;
            f = count(vm, FAILED_JOBS)?;
        }
        Ok(Val::array(grd_vm::value::PhpArray::from_values([Val::Int(p), Val::Int(d), Val::Int(f)])))
    });
}
