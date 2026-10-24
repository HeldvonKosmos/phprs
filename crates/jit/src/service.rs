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
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};

use grd_vm::bytecode::Module;
use grd_vm::vm::jit::Callee;

use crate::{CodeSlot, Compiler, Meta, Req};

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
