//! J2: background jobs in the server process (D5). `Job::dispatch(...$args)`
//! (prelude_queue.grd) calls `opengrid_job_dispatch($class, $args)`: the
//! arguments are kept as JSON and the job goes into the pool's own task
//! channel, so some worker runs it after its current work, outside any
//! request and with its own object arena (the request boundary). A failing
//! job is logged and run again up to `$tries` times. Jobs live in memory;
//! a restart drops pending ones (durable jobs: J4).

use std::sync::atomic::Ordering;

use grd_vm::error::Throw;
use grd_vm::value::Val;
use grd_vm::vm::Vm;

use super::ctx;

fn run_job(vm: &mut Vm, class: &str, args: &serde_json::Value, attempt: u32) {
    if let Some(c) = vm.host::<crate::worker::WorkerCtx>() {
        c.request = None;
    }
    // Err((message, tries)): tries is 1 when the job could not even be built
    let result = (|| -> Result<(), (String, u32)> {
        let fail = |m: String| (m, 1);
        let id = vm.lookup_class(class.as_bytes()).ok_or_else(|| fail(format!("job class {class} not found")))?;
        let base = vm.lookup_class(b"OpenGrid\\Queue\\Job").ok_or_else(|| fail("Job class missing".into()))?;
        if !vm.instance_of(id, base) {
            return Err(fail(format!("{class} does not extend OpenGrid\\Queue\\Job")));
        }
        let args: Vec<Val> = match grd_vm::json::from_json(vm, args.clone()).map_err(|t| fail(t.message))? {
            Val::Array(a) => a.values().cloned().collect(),
            _ => Vec::new(),
        };
        let job = vm.instantiate(id, args).map_err(|t| fail(format!("{}: {}", t.class, t.message)))?;
        let tries = match vm.call_method_sync(job, b"maxTries", Vec::new()) {
            Ok(Val::Int(n)) => n.max(1) as u32,
            _ => 1,
        };
        vm.call_method_sync_gc_safe(job, b"handle", Vec::new()).map(|_| ()).map_err(|t| (format!("{}: {}", t.class, t.message), tries))
    })();
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

/// Queue a job by class name and JSON arguments (the scheduler, J3).
pub fn push_job(services: &crate::services::Services, class: String, args: serde_json::Value) -> Result<(), String> {
    services.jobs.push(Box::new(move |vm| run_job(vm, &class, &args, 1)))
}

pub fn register(vm: &mut Vm) {
    vm.register_native("opengrid_job_dispatch", |vm, a| {
        let class = match a.first() {
            Some(Val::Str(s)) => s.to_string_lossy(),
            _ => return Err(Throw::type_error("opengrid_job_dispatch(): class name expected")),
        };
        let bytes = grd_vm::json::to_json(vm, &a.get(1).cloned().unwrap_or(Val::empty_array()), false)?;
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
    // [pending, done, failed] of this pool's job queue
    vm.register_native("opengrid_job_stats", |vm, _| {
        let j = &ctx(vm).services.jobs;
        let (p, d, f) = (j.pending.load(Ordering::SeqCst), j.done.load(Ordering::SeqCst), j.failed.load(Ordering::SeqCst));
        Ok(Val::array(grd_vm::value::PhpArray::from_values([Val::Int(p as i64), Val::Int(d as i64), Val::Int(f as i64)])))
    });
}
