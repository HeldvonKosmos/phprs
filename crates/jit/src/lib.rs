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
use grd_vm::vm::Vm;
use grd_vm::vm::jit::{Callee, CompileRequest, JitBackend, JitClasses, JitCode, JitStatus, TAG_ARRAY, TAG_OBJECT};

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

// ---- runtime functions for objects (JITO1) -----------------------------------
//
// Compiled code holds objects as their handle (`TAG_OBJECT`) and passes the
// VM it runs for through to these functions: `CraneliftJit::run` gets
// `&mut Vm` from the VM (which has taken the backend out of itself, so
// nothing else refers to the VM while compiled code runs) and hands it to
// the code as a raw pointer, unchanged down every native call. Each function
// below makes one short-lived reference from it; none is alive when it
// returns, so the references never overlap. The logic lives in safe code in
// grd-vm (`Vm::jit_prop_get`, `jit_prop_set`, `jit_new`), which checks
// handles and slots itself.

/// `$obj->prop`: kind (`KIND_*`), bits written to `out`.
pub extern "C" fn grd_rt_prop_get(vm: *mut Vm, h: u64, slot: u64, out: *mut u64) -> u64 {
    // SAFETY: `vm` is the VM of the running code (see above), only read
    // here; `out` points at an 8-byte stack slot of the calling code.
    let (kind, bits) = unsafe { &*vm }.jit_prop_get(h, slot);
    unsafe { *out = bits };
    kind
}

/// `$this->prop = value` (type, visibility, readonly checked by the analysis).
pub extern "C" fn grd_rt_prop_set(vm: *mut Vm, h: u64, slot: u64, tag: u64, bits: u64) {
    // SAFETY: `vm` is the VM of the running code (see above); this is the
    // only reference to it while the call lasts.
    unsafe { &mut *vm }.jit_prop_set(h, slot, tag, bits);
}

/// `new C(...)`: `args` holds `n` (slot, tag, bits) triples; returns the
/// handle, or 0 if nothing was done (compiled code then leaves).
pub extern "C" fn grd_rt_new(vm: *mut Vm, class: u64, args: *const u64, n: u64) -> u64 {
    // SAFETY: `args` points at a stack slot of the calling code with 3 * n
    // initialized words (codegen `Op::New`); `vm` as above, the only
    // reference to the VM while the call lasts.
    let args = unsafe { std::slice::from_raw_parts(args, 3 * n as usize) };
    unsafe { &mut *vm }.jit_new(class as u32, args)
}

/// The runtime functions, imported into the function being built.
fn runtime(module: &mut JITModule, b: &mut FunctionBuilder) -> Option<codegen::Runtime> {
    use cranelift_codegen::ir::{types, AbiParam};
    let sig = |module: &mut JITModule, params: usize, ret: bool| {
        let mut s = module.make_signature();
        s.params.extend((0..params).map(|_| AbiParam::new(types::I64)));
        if ret {
            s.returns.push(AbiParam::new(types::I64));
        }
        s
    };
    let mut import = |module: &mut JITModule, name: &str, params: usize, ret: bool| -> Option<cranelift_codegen::ir::FuncRef> {
        let s = sig(module, params, ret);
        let f = module.declare_function(name, cranelift_module::Linkage::Import, &s).ok()?;
        Some(module.declare_func_in_func(f, b.func))
    };
    Some(codegen::Runtime {
        dim: import(module, "grd_rt_dim", 3, true)?,
        count: import(module, "grd_rt_count", 1, true)?,
        prop_get: import(module, "grd_rt_prop_get", 4, true)?,
        prop_set: import(module, "grd_rt_prop_set", 5, false)?,
        new: import(module, "grd_rt_new", 4, true)?,
    })
}

fn runtime_symbols(b: &mut JITBuilder) {
    b.symbol("grd_rt_dim", grd_rt_dim as *const u8);
    b.symbol("grd_rt_count", grd_rt_count as *const u8);
    b.symbol("grd_rt_prop_get", grd_rt_prop_get as *const u8);
    b.symbol("grd_rt_prop_set", grd_rt_prop_set as *const u8);
    b.symbol("grd_rt_new", grd_rt_new as *const u8);
}

/// Whether a user function allocates or writes properties, directly or in
/// a function it calls (JITO1, V3 rule); unknown depth counts as yes.
fn unit_effects(modules: &[&Module], functions: &[Callee], m: u32, u: u32, depth: u32) -> bool {
    if depth > 8 {
        return true;
    }
    let Some(unit) = modules.get(m as usize).and_then(|x| x.units.get(u as usize)) else { return true };
    unit.ops.iter().any(|op| match op {
        Op::New { .. } | Op::StorePath(_) => true,
        Op::CallFuncId { id, .. } => match functions.get(id.0 as usize) {
            Some(Callee::User { module, unit: cu }) => (*module, *cu) != (m, u) && unit_effects(modules, functions, *module, *cu, depth + 1),
            _ => false,
        },
        _ => false,
    })
}

use analysis::{analyse, declared_sig, Analysis, CallKind, Ctx, Sig};

/// Test mode (JITD2 §8c): compiled code leaves at exit points although
/// nothing failed, so that every exit is checked against the interpreter.
/// `GRD_JIT_FORCE_EXIT=all` takes every exit point, `=<seed>` about half of
/// them, chosen by the seed. Applied when code is compiled; off costs nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ForceExit {
    #[default]
    Off,
    All,
    Seed(u64),
}

impl ForceExit {
    pub fn from_env() -> Self {
        match std::env::var("GRD_JIT_FORCE_EXIT").as_deref() {
            Ok("all") => ForceExit::All,
            Ok(v) => v.parse().map(ForceExit::Seed).unwrap_or(ForceExit::Off),
            Err(_) => ForceExit::Off,
        }
    }

    /// Whether the exit point at `ip` of the unit `name` is taken always.
    pub(crate) fn forces(self, name: &str, ip: usize) -> bool {
        match self {
            ForceExit::Off => false,
            ForceExit::All => true,
            ForceExit::Seed(s) => hash_of((s, name, ip)) & 1 == 1,
        }
    }
}
use codegen::{buf_words, signature, Gen, NativeCallee, ST_DEEP, ST_EXIT, ST_RETURN};

/// A function compiled for native calls.
#[derive(Clone)]
struct NativeFn {
    func: FuncId,
    buf_words: usize,
    param_slots: Vec<u32>,
}

/// Class each `TAG_OBJECT` parameter must be an instance of (`u32::MAX` for
/// the others).
fn param_classes(sig: &Sig) -> Vec<u32> {
    sig.params.iter().map(|t| if let analysis::Ty::Obj(c) = t { *c } else { u32::MAX }).collect()
}

/// Layout facts of compiled code; both tiers must agree on them, because the
/// VM fills the state buffer once and either tier's code may read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Meta {
    buf_words: usize,
    guards: Vec<(u32, u64)>,
    params: Vec<u64>,
    param_classes: Vec<u32>,
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
    force: ForceExit,
    this_class: Option<u32>,
    classes: &'a JitClasses,
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
        let sig_of = |mi: u32, ui: u32| -> Option<Sig> { declared_sig(req.modules[mi as usize], &req.modules[mi as usize].units[ui as usize], req.classes) };
        let effects_of = |mi: u32, ui: u32| unit_effects(&req.modules, req.functions, mi, ui, 0);
        // only the requested unit is a method; callees compiled along are functions
        let this_class = if (module, unit_id) == (req.module, req.unit) { req.this_class } else { None };
        let types: &[u8] = req.types.get(module as usize).and_then(|m| m.get(unit_id as usize)).map(Vec::as_slice).unwrap_or(&[]);
        let cx = Ctx { module: m, unit, functions: req.functions, sig_of: &sig_of, types, this_class, classes: req.classes, effects_of: &effects_of };
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
            let mut g = Gen::new(&mut b, m, unit, &an, whole, callees, rt, req.force);
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
        } else if self.log {
            let unit = &req.modules[req.module as usize].units[req.unit as usize];
            eprintln!("grd-jit: tier {} rejected {} {} at {}", self.tier, if req.end_ip.is_none() { "function" } else { "loop" }, unit.name, req.entry_ip);
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
            let sig = declared_sig(req.modules[req.module as usize], unit, req.classes)?;
            self.module.finalize_definitions().ok()?;
            let ptr = self.module.get_finalized_function(f.func) as usize;
            return Some((ptr, Meta { buf_words: f.buf_words, guards: Vec::new(), params: sig.params.iter().map(|t| t.tag()).collect(), param_classes: param_classes(&sig) }));
        }
        let mut in_progress = Vec::new();
        let (func, an) = self.build(req, req.module, req.unit, req.entry_ip, req.end_ip, req.entry_tags, &mut in_progress)?;
        if self.module.finalize_definitions().is_err() {
            return None;
        }
        let ptr = self.module.get_finalized_function(func) as usize;
        let words = buf_words(unit, &an);
        let (guards, params, pclasses) = match &an.sig {
            Some(sig) => (Vec::new(), sig.params.iter().map(|t| t.tag()).collect(), param_classes(sig)),
            None => {
                let g = (0..unit.num_slots)
                    .filter(|i| an.slot_ty[*i as usize].is_some())
                    .map(|i| (i, req.entry_tags.get(i as usize).copied().unwrap_or(0)))
                    .collect();
                (g, Vec::new(), Vec::new())
            }
        };
        Some((ptr, Meta { buf_words: words, guards, params, param_classes: pclasses }))
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
    force: ForceExit,
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
        Self::with_options(background, sync, ForceExit::from_env())
    }

    /// As `with_tiers`, with the forced-exit test mode given explicitly.
    pub fn with_options(background: bool, sync: bool, force: ForceExit) -> Self {
        let tiers = Tiers { background, sync };
        CraneliftJit {
            tier1: if background { Compiler::new("none", 1) } else { Compiler::new("speed", 1) },
            codes: Vec::new(),
            snapshots: HashMap::new(),
            tiers,
            force,
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
        (arcs, hash_of((&hashes, &names, &**req.classes)))
    }

    fn push_code(&mut self, slot: Arc<CodeSlot>, meta: Meta) -> JitCode {
        let id = self.codes.len() as u32;
        self.codes.push(slot);
        JitCode { id, buf_words: meta.buf_words, guards: meta.guards, params: meta.params, param_classes: meta.param_classes }
    }
}

impl JitBackend for CraneliftJit {
    fn compile(&mut self, req: &CompileRequest) -> Option<JitCode> {
        let (arcs, prog) = if self.tiers.background { self.snapshot(req) } else { (Vec::new(), 0) };
        // element kinds of the profile are not part of the key: compiled reads
        // check them at run time, so shared code stays correct for any profile
        // (with them in the key, workers missed each other's tier-2 code)
        let key = hash_of((prog, req.module, req.unit, req.entry_ip, req.end_ip, req.entry_tags, self.force, req.this_class));
        // another worker (or an earlier VM) already has tier-2 code for it
        if self.tiers.background
            && let Some((ptr, meta)) = service::lookup(key)
        {
            return Some(self.push_code(CodeSlot::new(ptr, 2), meta));
        }
        let r = Req { modules: req.modules.iter().map(|m| &**m).collect(), module: req.module, unit: req.unit, entry_ip: req.entry_ip, end_ip: req.end_ip, entry_tags: req.entry_tags, functions: req.functions, types: req.unit_types, force: self.force, this_class: req.this_class, classes: req.classes };
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
                force: self.force,
                this_class: req.this_class,
                classes: req.classes.clone(),
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
        if tag == TAG_OBJECT {
            return Some(Val::Object(grd_vm::value::ObjHandle(bits as u32)));
        }
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

    fn run(&mut self, id: u32, buf: &mut [u64], vm: &mut Vm) -> JitStatus {
        let ptr = self.codes[id as usize].ptr.load(Ordering::Acquire);
        // SAFETY: `ptr` is a function finalized by a `Compiler`'s JITModule
        // with the signature `extern "C" fn(*mut u64, *mut Vm, i64) -> i64`
        // (codegen::signature, the ISA's default C calling convention): the
        // tier-1 module lives as long as `self`, the tier-2 module as long as
        // the process (background thread, never dropped), and a slot only
        // ever holds finalized code (published with Release, loaded with
        // Acquire). Both tiers were translated from the same analysis, so the
        // code reads and writes only words below `JitCode::buf_words`, the
        // length the VM gave `buf` (the background compiler checks the layout
        // before swapping).
        // `vm` goes to the runtime functions only (see "runtime functions for
        // objects").
        let f: extern "C" fn(*mut u64, *mut Vm, i64) -> i64 = unsafe { std::mem::transmute(ptr) };
        // native call depth 0: code entered from the interpreter
        let status = f(buf.as_mut_ptr(), vm as *mut Vm, 0);
        if self.log {
            if self.stats.len() <= id as usize {
                self.stats.resize_with(id as usize + 1, HashMap::new);
            }
            *self.stats[id as usize].entry((status, if status == ST_EXIT { buf[0] } else { 0 })).or_default() += 1;
        }
        match status {
            ST_EXIT => JitStatus::Exit,
            ST_RETURN => JitStatus::Return,
            ST_DEEP => JitStatus::TooDeep,
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

/// The runtime functions called directly, without compiled code, so that
/// Miri can check their `unsafe` blocks (`cargo +nightly miri test -p grd-jit
/// --lib runtime`); the same tests run in the normal suite.
#[cfg(test)]
mod runtime_tests {
    use super::*;
    use grd_vm::value::{ClassId, ObjHandle};
    use grd_vm::vm::jit::{KIND_FLOAT, KIND_INT, KIND_NONE, KIND_OBJECT, TAG_FLOAT, TAG_INT};

    /// A VM without prelude holding `$v = new V(1.5, 2)` and `$w = new W($v)`.
    fn vm() -> (Vm, ClassId, ObjHandle, ObjHandle) {
        let mut vm = Vm::new();
        let src = "<?grd final class V { public function __construct(public float $x, public int $n) {} }\nfinal class W { public function __construct(public V $v) {} }\n$v = new V(1.5, 2);\n$w = new W($v);\n";
        vm.run_file("t.grd", src.as_bytes()).expect("program runs");
        let class = vm.lookup_class(b"V").expect("class V");
        (vm, class, ObjHandle(1), ObjHandle(2))
    }

    fn slot(vm: &Vm, class: ClassId, name: &[u8]) -> u64 {
        *vm.class(class).prop_index.get(name).expect("property") as u64
    }

    #[test]
    fn runtime_reads_writes_and_allocates_through_the_vm_pointer() {
        let (mut vm, v, h, w) = vm();
        let (sx, sn) = (slot(&vm, v, b"x"), slot(&vm, v, b"n"));
        let wv = slot(&vm, vm.lookup_class(b"W").unwrap(), b"v");
        let p: *mut Vm = &mut vm;
        let mut out = 0u64;
        assert_eq!(grd_rt_prop_get(p, h.0 as u64, sx, &mut out), KIND_FLOAT);
        assert_eq!(f64::from_bits(out), 1.5);
        assert_eq!(grd_rt_prop_get(p, h.0 as u64, sn, &mut out), KIND_INT);
        assert_eq!(out, 2);
        assert_eq!(grd_rt_prop_get(p, w.0 as u64, wv, &mut out), KIND_OBJECT);
        assert_eq!(out, h.0 as u64);
        // invalid handle, slot past the end: nothing to read, compiled code leaves
        assert_eq!(grd_rt_prop_get(p, 999, sx, &mut out), KIND_NONE);
        assert_eq!(grd_rt_prop_get(p, h.0 as u64, 99, &mut out), KIND_NONE);
        grd_rt_prop_set(p, h.0 as u64, sx, TAG_FLOAT, 4.25f64.to_bits());
        grd_rt_prop_set(p, 999, sx, TAG_FLOAT, 0);
        grd_rt_prop_set(p, h.0 as u64, 99, TAG_INT, 0);
        assert_eq!(grd_rt_prop_get(p, h.0 as u64, sx, &mut out), KIND_FLOAT);
        assert_eq!(f64::from_bits(out), 4.25);
        let args = [sx, TAG_FLOAT, 0.5f64.to_bits(), sn, TAG_INT, 7];
        let made = grd_rt_new(p, v.0 as u64, args.as_ptr(), 2);
        assert_eq!(made, 3, "the next handle");
        assert_eq!(grd_rt_prop_get(p, made, sn, &mut out), KIND_INT);
        assert_eq!(out, 7);
        // unknown class: nothing allocated
        assert_eq!(grd_rt_new(p, 9999, args.as_ptr(), 0), 0);
        // object limit reached: nothing allocated, the interpreter raises
        vm.object_limit = vm.objects.len();
        let p: *mut Vm = &mut vm;
        assert_eq!(grd_rt_new(p, v.0 as u64, args.as_ptr(), 2), 0);
        assert_eq!(vm.objects.len(), 3);
    }

    #[test]
    fn runtime_reads_arrays() {
        let a = PhpArray::from_values([Val::Int(4), Val::Float(0.5)]);
        let mut out = 0u64;
        let p = &a as *const PhpArray as u64;
        assert_eq!(grd_rt_dim(p, 1, &mut out), grd_vm::vm::jit::KIND_FLOAT);
        assert_eq!(f64::from_bits(out), 0.5);
        assert_eq!(grd_rt_dim(p, 5, &mut out), KIND_NONE);
        assert_eq!(grd_rt_count(p), 2);
    }
}
