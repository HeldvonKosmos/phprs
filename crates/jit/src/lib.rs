//! grd-jit: the JIT tier of the grd VM (RUNTIME_ARCHITECTURE Stufe 2,
//! DECISIONS D6 / "Cranelift direkt"). Hot pure-numeric functions and hot
//! loops are translated from VM bytecode to Cranelift IR and run as machine
//! code. This crate is the only place with `unsafe`: calling the generated
//! code (`CraneliftJit::run`).
//!
//! Tiers (DECISIONS D8): the interpreter counts calls and back-edges; a hot
//! function or loop is compiled at once on the VM thread with
//! `opt_level = "none"` (tier 1, low latency) and handed to the process-wide
//! background compiler (`service`), which recompiles it with
//! `opt_level = "speed"` (tier 2) and swaps the code pointer atomically.

mod analysis;
mod codegen;
mod service;
pub use service::{set_tier2_limit, tier2_stats};

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Module as _};

use grd_vm::bytecode::{Module, Op};
use grd_vm::value::{PhpArray, Val};
use grd_vm::vm::jit::{Callee, CompileRequest, JitBackend, JitCode, JitStatus, TAG_ARRAY};

// ---- runtime functions (JIT2b) ----------------------------------------------
//
// Compiled code holds arrays as the address of their `PhpArray` (`TAG_ARRAY`
// bits, `Rc::as_ptr`). Such an address comes from the VM (a slot or an
// argument, alive for the whole run and not changed by it: the analysis
// admits no op that writes an array or calls back into the VM) or from these
// functions (an element of such an array, alive as long as its container).

/// `$a[$key]`: element kind (`KIND_*`), bits written to `out`.
extern "C" fn grd_rt_dim(arr: u64, key: i64, out: *mut u64) -> u64 {
    // SAFETY: `arr` is the address of a live, unchanged `PhpArray` (see
    // above) and `out` points at an 8-byte stack slot of the calling code.
    let a = unsafe { &*(arr as *const PhpArray) };
    let (kind, bits) = grd_vm::vm::jit::dim_int(a, key);
    unsafe { *out = bits };
    kind
}

/// `count($a)`
extern "C" fn grd_rt_count(arr: u64) -> i64 {
    // SAFETY: as in `grd_rt_dim`.
    let a = unsafe { &*(arr as *const PhpArray) };
    a.len() as i64
}

/// The runtime functions, imported into the function being built.
fn runtime(module: &mut JITModule, b: &mut FunctionBuilder) -> Option<codegen::Runtime> {
    use cranelift_codegen::ir::{types, AbiParam};
    let mut dim = module.make_signature();
    dim.params.extend([AbiParam::new(types::I64), AbiParam::new(types::I64), AbiParam::new(types::I64)]);
    dim.returns.push(AbiParam::new(types::I64));
    let mut count = module.make_signature();
    count.params.push(AbiParam::new(types::I64));
    count.returns.push(AbiParam::new(types::I64));
    let fd = module.declare_function("grd_rt_dim", cranelift_module::Linkage::Import, &dim).ok()?;
    let fc = module.declare_function("grd_rt_count", cranelift_module::Linkage::Import, &count).ok()?;
    Some(codegen::Runtime { dim: module.declare_func_in_func(fd, b.func), count: module.declare_func_in_func(fc, b.func) })
}

fn runtime_symbols(b: &mut JITBuilder) {
    b.symbol("grd_rt_dim", grd_rt_dim as *const u8);
    b.symbol("grd_rt_count", grd_rt_count as *const u8);
}

use analysis::{analyse, declared_sig, Analysis, CallKind, Ctx, Sig};
use codegen::{buf_words, signature, Gen, NativeCallee, ST_EXIT, ST_RETURN};

/// A function compiled for native calls.
#[derive(Clone)]
struct NativeFn {
    func: FuncId,
    buf_words: usize,
    param_slots: Vec<u32>,
}

/// Layout facts of compiled code; both tiers must agree on them, because the
/// VM fills the state buffer once and either tier's code may read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Meta {
    buf_words: usize,
    guards: Vec<(u32, u64)>,
    params: Vec<u64>,
}

/// A compile request with plain module references, so that the same
/// translation runs on the VM thread (modules behind `Rc`) and on the
/// background thread (modules behind `Arc`).
pub(crate) struct Req<'a> {
    modules: Vec<&'a Module>,
    module: u32,
    unit: u32,
    entry_ip: usize,
    end_ip: Option<usize>,
    entry_tags: &'a [u64],
    functions: &'a [Callee],
    /// Element kinds at the dim fetches, `[module][unit][ip]`.
    types: &'a [Vec<Vec<u8>>],
}

/// One Cranelift module at one optimisation level. `prog` identifies the
/// program (module contents and function table) so that one background
/// compiler can serve VMs whose module indices differ.
pub(crate) struct Compiler {
    /// Dropped by `Drop for Compiler`, which frees the code memory.
    module: ManuallyDrop<JITModule>,
    fctx: FunctionBuilderContext,
    natives: HashMap<(u64, u32, u32), NativeFn>,
    failed: HashSet<(u64, u32, u32)>,
    prog: u64,
    tier: u8,
    log: bool,
}

impl Drop for Compiler {
    fn drop(&mut self) {
        // SAFETY: (1) `ManuallyDrop::take` runs once, here, and `self.module`
        // is not used afterwards. (2) `free_memory` requires that none of the
        // module's functions is executing or called later. A `Compiler` is
        // dropped in two situations only: as tier 1 of a `CraneliftJit`,
        // which the VM drops together with itself — the VM is
        // single-threaded, so no compiled code of it runs during the drop,
        // and the only holders of its code pointers are this backend's code
        // slots and native table, dropped in the same step (the process-wide
        // tier-2 cache stores only tier-2 pointers); or never, as the tier-2
        // compiler, which lives until the process ends.
        unsafe { ManuallyDrop::take(&mut self.module).free_memory() };
    }
}

impl Compiler {
    pub(crate) fn new(opt_level: &str, tier: u8) -> Self {
        let mut flags = settings::builder();
        flags.set("opt_level", opt_level).expect("opt_level");
        let isa = cranelift_native::builder().expect("host isa").finish(settings::Flags::new(flags)).expect("isa");
        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        runtime_symbols(&mut builder);
        Compiler {
            module: ManuallyDrop::new(JITModule::new(builder)),
            fctx: FunctionBuilderContext::new(),
            natives: HashMap::new(),
            failed: HashSet::new(),
            prog: 0,
            tier,
            log: std::env::var("GRD_JIT_LOG").is_ok(),
        }
    }

    /// Analyse and translate one whole function or loop region; `None` if it
    /// uses anything outside the supported subset. Callees are compiled first.
    fn build(&mut self, req: &Req, module: u32, unit_id: u32, entry: usize, end: Option<usize>, tags: &[u64], in_progress: &mut Vec<(u32, u32)>) -> Option<(FuncId, Analysis)> {
        let m: &Module = req.modules[module as usize];
        let unit = &m.units[unit_id as usize];
        let whole = end.is_none();
        let prog = self.prog;
        let sig_of = |mi: u32, ui: u32| -> Option<Sig> { declared_sig(&req.modules[mi as usize], &req.modules[mi as usize].units[ui as usize]) };
        let types: &[u8] = req.types.get(module as usize).and_then(|m| m.get(unit_id as usize)).map(Vec::as_slice).unwrap_or(&[]);
        let cx = Ctx { module: m, unit, functions: req.functions, sig_of: &sig_of, types };
        let (lo, hi) = match end {
            None => (0, unit.ops.len().saturating_sub(1)),
            Some(h) => (entry, h),
        };
        let an = analyse(&cx, lo, hi, entry, tags, whole)?;
        let sig = signature(&*self.module);
        let func = self.module.declare_anonymous_function(&sig).ok()?;
        if whole {
            // known before the body so that self-recursion calls itself natively
            let param_slots = unit.params.iter().map(|p| p.slot).collect();
            self.natives.insert((prog, module, unit_id), NativeFn { func, buf_words: buf_words(unit, &an), param_slots });
        }
        in_progress.push((module, unit_id));
        // callees first
        let mut callee_fns: HashMap<usize, NativeFn> = HashMap::new();
        for (ip, kind) in &an.calls {
            if let CallKind::User { module: cm, unit: cu, .. } = kind {
                let key = (prog, *cm, *cu);
                let f = match self.natives.get(&key) {
                    Some(f) => f.clone(),
                    None => {
                        if self.failed.contains(&key) || in_progress.contains(&(*cm, *cu)) {
                            self.abandon(whole, module, unit_id, in_progress);
                            return None;
                        }
                        let cunit = &req.modules[*cm as usize].units[*cu as usize];
                        let ctags = vec![0u64; cunit.num_slots as usize];
                        match self.build(req, *cm, *cu, 0, None, &ctags, in_progress) {
                            Some(_) => self.natives.get(&key).cloned()?,
                            None => {
                                self.failed.insert(key);
                                self.abandon(whole, module, unit_id, in_progress);
                                return None;
                            }
                        }
                    }
                };
                callee_fns.insert(*ip, f);
            }
        }
        let mut ctx = self.module.make_context();
        ctx.func.signature = signature(&*self.module);
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut self.fctx);
            let mut callees = HashMap::new();
            for (ip, f) in &callee_fns {
                let fref = self.module.declare_func_in_func(f.func, b.func);
                callees.insert(*ip, NativeCallee { fref, buf_words: f.buf_words, param_slots: f.param_slots.clone() });
            }
            let rt = runtime(&mut self.module, &mut b)?;
            let mut g = Gen::new(&mut b, m, unit, &an, whole, callees, rt);
            g.emit(tags);
            b.seal_all_blocks();
            b.finalize(self.module.target_config());
        }
        if self.module.define_function(func, &mut ctx).is_err() {
            self.module.clear_context(&mut ctx);
            self.abandon(whole, module, unit_id, in_progress);
            return None;
        }
        self.module.clear_context(&mut ctx);
        in_progress.retain(|k| *k != (module, unit_id));
        if self.log {
            eprintln!("grd-jit: tier {} compiled {} {} ops {}..={} (stack {}, calls {})", self.tier, if whole { "function" } else { "loop" }, unit.name, lo, hi, an.max_stack, an.calls.len());
        }
        Some((func, an))
    }

    fn abandon(&mut self, whole: bool, module: u32, unit: u32, in_progress: &mut Vec<(u32, u32)>) {
        if whole {
            self.natives.remove(&(self.prog, module, unit));
            self.failed.insert((self.prog, module, unit));
        }
        in_progress.retain(|k| *k != (module, unit));
    }

    /// Compile a request; returns the entry address and the layout facts.
    pub(crate) fn compile(&mut self, req: &Req, prog: u64) -> Option<(usize, Meta)> {
        let t0 = std::time::Instant::now();
        let r = self.compile_inner(req, prog);
        if self.log && r.is_some() {
            eprintln!("grd-jit: tier {} took {} µs", self.tier, t0.elapsed().as_micros());
        }
        r
    }

    fn compile_inner(&mut self, req: &Req, prog: u64) -> Option<(usize, Meta)> {
        self.prog = prog;
        let unit = &req.modules[req.module as usize].units[req.unit as usize];
        let key = (prog, req.module, req.unit);
        if req.end_ip.is_none() && self.failed.contains(&key) {
            return None;
        }
        // a loop region must start at a loop header of this unit
        if let Some(end) = req.end_ip
            && !matches!(unit.ops.get(end), Some(Op::Jump(t)) if *t as usize == req.entry_ip)
        {
            return None;
        }
        // a whole function already compiled as a callee
        if req.end_ip.is_none()
            && let Some(f) = self.natives.get(&key).cloned()
        {
            let sig = declared_sig(&req.modules[req.module as usize], unit)?;
            self.module.finalize_definitions().ok()?;
            let ptr = self.module.get_finalized_function(f.func) as usize;
            return Some((ptr, Meta { buf_words: f.buf_words, guards: Vec::new(), params: sig.params.iter().map(|t| t.tag()).collect() }));
        }
        let mut in_progress = Vec::new();
        let (func, an) = self.build(req, req.module, req.unit, req.entry_ip, req.end_ip, req.entry_tags, &mut in_progress)?;
        if self.module.finalize_definitions().is_err() {
            return None;
        }
        let ptr = self.module.get_finalized_function(func) as usize;
        let words = buf_words(unit, &an);
        let (guards, params) = match &an.sig {
            Some(sig) => (Vec::new(), sig.params.iter().map(|t| t.tag()).collect()),
            None => {
                let g = (0..unit.num_slots)
                    .filter(|i| an.slot_ty[*i as usize].is_some())
                    .map(|i| (i, req.entry_tags.get(i as usize).copied().unwrap_or(0)))
                    .collect();
                (g, Vec::new())
            }
        };
        Some((ptr, Meta { buf_words: words, guards, params }))
    }
}

/// The entry point of one compiled function or region. Tier 1 stores its
/// code here first; the background compiler later replaces it with the tier-2
/// code (hot swap). The VM loads the pointer on every call (`Acquire`), the
/// background thread publishes with `Release`, so a call sees either the old
/// or the new, fully finalized code. Old code stays valid: tier-1 code lives
/// as long as the VM's backend, tier-2 code as long as the process.
pub struct CodeSlot {
    ptr: AtomicUsize,
    tier: AtomicU8,
}

impl CodeSlot {
    fn new(ptr: usize, tier: u8) -> Arc<Self> {
        Arc::new(CodeSlot { ptr: AtomicUsize::new(ptr), tier: AtomicU8::new(tier) })
    }
    pub(crate) fn swap_in(&self, ptr: usize, tier: u8) {
        self.ptr.store(ptr, Ordering::Release);
        self.tier.store(tier, Ordering::Release);
    }
    pub fn tier(&self) -> u8 {
        self.tier.load(Ordering::Acquire)
    }
}

/// How the backend uses the tiers (environment, read once per backend).
#[derive(Clone, Copy, Debug)]
struct Tiers {
    /// Advanced JIT (`GRD_ADVANCED_JIT=on`, default off, D8/D9): tier 1 with
    /// `opt_level = "none"`, background recompilation with `"speed"`, hot
    /// swap, process-wide cache. Off: one synchronous compile with
    /// `"speed"` on the VM thread.
    background: bool,
    /// Wait for tier 2 before running (`GRD_JIT_SYNC=1`; tests, benchmarks).
    sync: bool,
}

/// `GRD_ADVANCED_JIT=on` (or `1`) turns the advanced JIT on.
pub fn advanced_from_env() -> bool {
    matches!(std::env::var("GRD_ADVANCED_JIT").as_deref(), Ok("on" | "1"))
}

/// The JIT backend of one VM: tier-1 compiler, code slots, module snapshots
/// for the background compiler.
pub struct CraneliftJit {
    tier1: Compiler,
    codes: Vec<Arc<CodeSlot>>,
    /// Module snapshots for the background thread, by `Rc` address: the
    /// `Arc` copy and a content hash (computed once per module).
    snapshots: HashMap<usize, (Arc<Module>, u64)>,
    tiers: Tiers,
    log: bool,
    /// With GRD_JIT_LOG: per code id, runs and (status, exit ip) counts.
    stats: Vec<HashMap<(i64, u64), u64>>,
}

impl Drop for CraneliftJit {
    fn drop(&mut self) {
        if self.log {
            for (id, st) in self.stats.iter().enumerate() {
                let mut v: Vec<_> = st.iter().collect();
                v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
                eprintln!("grd-jit: code {id} (tier {}): {}", self.codes[id].tier(), v.iter().take(4).map(|((s, ip), n)| format!("status {s} ip {ip} x{n}")).collect::<Vec<_>>().join(", "));
            }
        }
    }
}

impl Default for CraneliftJit {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_of(x: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    x.hash(&mut h);
    h.finish()
}

impl CraneliftJit {
    /// Options from the environment: `GRD_ADVANCED_JIT=on` for the tiers
    /// with background compiler, `GRD_JIT_SYNC=1` waits for tier 2.
    pub fn new() -> Self {
        Self::with_tiers(advanced_from_env(), std::env::var("GRD_JIT_SYNC").as_deref() == Ok("1"))
    }

    /// `background` (advanced JIT): compile quickly with `opt_level =
    /// "none"` and recompile hot code with `"speed"` on the process-wide
    /// thread; `sync`: wait for that before running it. Without
    /// `background`: compile once with `"speed"`.
    pub fn with_tiers(background: bool, sync: bool) -> Self {
        let tiers = Tiers { background, sync };
        CraneliftJit {
            tier1: if background { Compiler::new("none", 1) } else { Compiler::new("speed", 1) },
            codes: Vec::new(),
            snapshots: HashMap::new(),
            tiers,
            log: std::env::var("GRD_JIT_LOG").is_ok(),
            stats: Vec::new(),
        }
    }

    /// `Arc` copies of all modules and the program fingerprint.
    fn snapshot(&mut self, req: &CompileRequest) -> (Vec<Arc<Module>>, u64) {
        let mut arcs = Vec::with_capacity(req.modules.len());
        let mut hashes = Vec::with_capacity(req.modules.len());
        for m in req.modules {
            let (arc, h) = self.snapshots.entry(Rc::as_ptr(m) as usize).or_insert_with(|| {
                let bytes = postcard::to_allocvec(&**m).unwrap_or_default();
                (Arc::new((**m).clone()), hash_of(&bytes))
            });
            arcs.push(arc.clone());
            hashes.push(*h);
        }
        let names: Vec<String> = req.functions.iter().map(|c| format!("{c:?}")).collect();
        (arcs, hash_of((&hashes, &names)))
    }

    fn push_code(&mut self, slot: Arc<CodeSlot>, meta: Meta) -> JitCode {
        let id = self.codes.len() as u32;
        self.codes.push(slot);
        JitCode { id, buf_words: meta.buf_words, guards: meta.guards, params: meta.params }
    }
}

impl JitBackend for CraneliftJit {
    fn compile(&mut self, req: &CompileRequest) -> Option<JitCode> {
        let (arcs, prog) = if self.tiers.background { self.snapshot(req) } else { (Vec::new(), 0) };
        let key = hash_of((prog, req.module, req.unit, req.entry_ip, req.end_ip, req.entry_tags, &req.profile.types));
        // another worker (or an earlier VM) already has tier-2 code for it
        if self.tiers.background
            && let Some((ptr, meta)) = service::lookup(key)
        {
            return Some(self.push_code(CodeSlot::new(ptr, 2), meta));
        }
        let r = Req { modules: req.modules.iter().map(|m| &**m).collect(), module: req.module, unit: req.unit, entry_ip: req.entry_ip, end_ip: req.end_ip, entry_tags: req.entry_tags, functions: req.functions, types: req.unit_types };
        let (ptr, meta) = self.tier1.compile(&r, prog)?;
        let slot = CodeSlot::new(ptr, 1);
        // JIT5: past the process budget, hot code stays at tier 1
        if self.tiers.background && service::admit() {
            let job = service::Job {
                key,
                prog,
                modules: arcs,
                module: req.module,
                unit: req.unit,
                entry_ip: req.entry_ip,
                end_ip: req.end_ip,
                tags: req.entry_tags.to_vec(),
                functions: req.functions.to_vec(),
                types: req.unit_types.to_vec(),
                meta: meta.clone(),
                slot: slot.clone(),
            };
            let done = service::submit(job);
            if self.tiers.sync {
                let _ = done.recv();
            }
        }
        Some(self.push_code(slot, meta))
    }

    fn tiers(&self) -> Vec<u8> {
        self.codes.iter().map(|c| c.tier()).collect()
    }

    fn ref_val(&self, tag: u64, bits: u64) -> Option<Val> {
        if tag != TAG_ARRAY || bits == 0 {
            return None;
        }
        let p = bits as *const PhpArray;
        // SAFETY: compiled code leaves only addresses it got from the VM or
        // from `grd_rt_dim` in the buffer: `Rc::as_ptr` of an `Rc<PhpArray>`
        // that is still alive (held by a slot or by an element of such an
        // array; the VM converts the exit stack before it changes anything).
        // Incrementing the count first makes `from_raw` produce a new owner.
        let rc = unsafe {
            std::rc::Rc::increment_strong_count(p);
            std::rc::Rc::from_raw(p)
        };
        Some(Val::Array(rc))
    }

    fn run(&mut self, id: u32, buf: &mut [u64]) -> JitStatus {
        let ptr = self.codes[id as usize].ptr.load(Ordering::Acquire);
        // SAFETY: `ptr` is a function finalized by a `Compiler`'s JITModule
        // with the signature `extern "C" fn(*mut u64) -> i64`
        // (codegen::signature, the ISA's default C calling convention): the
        // tier-1 module lives as long as `self`, the tier-2 module as long as
        // the process (background thread, never dropped), and a slot only
        // ever holds finalized code (published with Release, loaded with
        // Acquire). Both tiers were translated from the same analysis, so the
        // code reads and writes only words below `JitCode::buf_words`, the
        // length the VM gave `buf` (the background compiler checks the layout
        // before swapping).
        let f: extern "C" fn(*mut u64) -> i64 = unsafe { std::mem::transmute(ptr) };
        let status = f(buf.as_mut_ptr());
        if self.log {
            if self.stats.len() <= id as usize {
                self.stats.resize_with(id as usize + 1, HashMap::new);
            }
            *self.stats[id as usize].entry((status, if status == ST_EXIT { buf[0] } else { 0 })).or_default() += 1;
        }
        match status {
            ST_EXIT => JitStatus::Exit,
            ST_RETURN => JitStatus::Return,
            _ => JitStatus::Bailout,
        }
    }
}

/// `GRD_JIT=0`: emergency switch, no JIT anywhere (interpreter only).
pub fn disabled_by_env() -> bool {
    matches!(std::env::var("GRD_JIT").as_deref(), Ok("0" | "off"))
}

/// Install the JIT into a VM unless `GRD_JIT=0` (the `grd` binary; the
/// framework decides with `OPENGRID_JIT`, see `opengrid_framework::worker`).
pub fn install(vm: &mut grd_vm::vm::Vm) {
    if disabled_by_env() {
        return;
    }
    vm.set_jit(Box::new(CraneliftJit::new()));
}

/// One line for logs: the effective JIT mode of a process.
pub fn describe(enabled: bool) -> &'static str {
    match (enabled && !disabled_by_env(), advanced_from_env()) {
        (false, _) => "aus (nur Interpreter)",
        (true, false) => "an (Stufe 1, synchron)",
        (true, true) => "an (erweitert: Stufen, Hintergrund-Compiler)",
    }
}
