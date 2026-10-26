//! The process-wide background compiler (tier 2, DECISIONS D8).
//!
//! One thread owns its own Cranelift module with `opt_level = "speed"`. VM
//! threads send it jobs: an owned copy of everything the translation reads
//! (modules as `Arc` snapshots — bytecode is plain data, so no `Rc` crosses a
//! thread — function table, entry tags) plus the code slot to update. When
//! the optimized code is finalized, the thread swaps it into the slot. The
//! result is also cached by a content key, so every worker VM that loads the
//! same program and gets hot at the same place takes the tier-2 code
//! directly. The module is never dropped: tier-2 code lives as long as the
//! process.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};

use grd_vm::bytecode::Module;
use grd_vm::vm::jit::Callee;

use crate::{CodeSlot, Compiler, ForceExit, Meta, Req};

/// One recompilation, self-contained (`Send`).
pub(crate) struct Job {
    pub key: u64,
    pub prog: u64,
    pub modules: Vec<Arc<Module>>,
    pub module: u32,
    pub unit: u32,
    pub entry_ip: usize,
    pub end_ip: Option<usize>,
    pub tags: Vec<u64>,
    pub functions: Vec<Callee>,
    /// Element kinds seen at the unit's dim fetches (JIT2b).
    pub types: Vec<Vec<Vec<u8>>>,
    pub force: ForceExit,
    pub this_class: Option<u32>,
    pub classes: Arc<grd_vm::vm::jit::JitClasses>,
    /// Layout of the tier-1 code; tier 2 must match it to be swapped in.
    pub meta: Meta,
    pub slot: Arc<CodeSlot>,
}

struct Service {
    tx: Mutex<Sender<(Job, SyncSender<()>)>>,
    /// Content key → tier-2 entry and layout; `None` if tier 2 failed.
    cache: Mutex<HashMap<u64, Option<(usize, Meta)>>>,
}

static SERVICE: OnceLock<Service> = OnceLock::new();

/// JIT5: tier-2 code is never freed, so the number of tier-2 compilations per
/// process is bounded (`GRD_JIT_TIER2_LIMIT`, default 4096; each one adds a
/// cache entry and a few KB of code). Past it, new hot code keeps tier 1,
/// which goes with its VM; cached tier-2 code is still shared.
static TIER2_LIMIT: AtomicUsize = AtomicUsize::new(usize::MAX);
static TIER2_ADMITTED: AtomicUsize = AtomicUsize::new(0);

fn limit() -> usize {
    let l = TIER2_LIMIT.load(Ordering::Relaxed);
    if l != usize::MAX {
        return l;
    }
    let l = std::env::var("GRD_JIT_TIER2_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(4096);
    TIER2_LIMIT.store(l, Ordering::Relaxed);
    l
}

/// Take one tier-2 compilation from the budget; false when it is used up.
pub(crate) fn admit() -> bool {
    admit_from(&TIER2_ADMITTED, limit())
}

fn admit_from(counter: &AtomicUsize, limit: usize) -> bool {
    counter.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < limit).then_some(n + 1)).is_ok()
}

/// Set the tier-2 budget of the process (tests, embedding hosts).
pub fn set_tier2_limit(n: usize) {
    TIER2_LIMIT.store(n, Ordering::Relaxed);
}

/// (tier-2 compilations admitted, budget, cache entries).
pub fn tier2_stats() -> (usize, usize, usize) {
    let cache = SERVICE.get().and_then(|s| s.cache.lock().ok().map(|c| c.len())).unwrap_or(0);
    (TIER2_ADMITTED.load(Ordering::Acquire), limit(), cache)
}

fn service() -> &'static Service {
    SERVICE.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<(Job, SyncSender<()>)>();
        std::thread::Builder::new().name("grd-jit-tier2".into()).spawn(move || worker(rx)).expect("spawn tier-2 compiler");
        Service { tx: Mutex::new(tx), cache: Mutex::new(HashMap::new()) }
    })
}

/// Tier-2 code for a content key, if the background compiler has it.
pub(crate) fn lookup(key: u64) -> Option<(usize, Meta)> {
    service().cache.lock().ok()?.get(&key).cloned().flatten()
}

/// Queue a job; the receiver gets a message (or is disconnected) when the
/// job is finished, swapped in or not.
pub(crate) fn submit(job: Job) -> Receiver<()> {
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    if let Ok(tx) = service().tx.lock() {
        let _ = tx.send((job, done_tx));
    }
    done_rx
}

fn worker(rx: Receiver<(Job, SyncSender<()>)>) {
    let mut compiler = Compiler::new("speed", 2);
    let log = std::env::var("GRD_JIT_LOG").is_ok();
    for (job, done) in rx {
        let cached = service().cache.lock().ok().and_then(|c| c.get(&job.key).cloned());
        let result = match cached {
            Some(r) => r,
            None => {
                let req = Req {
                    modules: job.modules.iter().map(|m| &**m).collect(),
                    module: job.module,
                    unit: job.unit,
                    entry_ip: job.entry_ip,
                    end_ip: job.end_ip,
                    entry_tags: &job.tags,
                    functions: &job.functions,
                    types: &job.types,
                    force: job.force,
                    this_class: job.this_class,
                    classes: &job.classes,
                };
                // a Cranelift panic must not take the service down
                let r = catch_unwind(AssertUnwindSafe(|| compiler.compile(&req, job.prog))).ok().flatten();
                if let Ok(mut c) = service().cache.lock() {
                    c.insert(job.key, r.clone());
                }
                r
            }
        };
        match result {
            Some((ptr, meta)) if meta == job.meta => {
                job.slot.swap_in(ptr, 2);
                if log {
                    eprintln!("grd-jit: tier 2 swapped in (unit {} of module {}, entry {})", job.unit, job.module, job.entry_ip);
                }
            }
            Some(_) => {
                if log {
                    eprintln!("grd-jit: tier 2 layout differs from tier 1, kept tier 1");
                }
            }
            None => {}
        }
        let _ = done.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_never_exceeded_under_contention() {
        // all threads start together, so they race for the last slots
        let counter = Arc::new(AtomicUsize::new(0));
        let start = Arc::new(std::sync::Barrier::new(8));
        let admitted: usize = (0..8)
            .map(|_| {
                let (c, start) = (Arc::clone(&counter), Arc::clone(&start));
                std::thread::spawn(move || {
                    start.wait();
                    (0..1000).filter(|_| admit_from(&c, 100)).count()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .sum();
        assert_eq!(admitted, 100);
        assert_eq!(counter.load(Ordering::Acquire), 100);
    }
}

