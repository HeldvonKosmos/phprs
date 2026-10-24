//! grd-jit: the JIT tier of the grd VM (RUNTIME_ARCHITECTURE Stufe 2,
//! DECISIONS D6 / "Cranelift direkt"). Hot pure-numeric functions and hot
//! loops are translated from VM bytecode to Cranelift IR and run as machine
//! code. This crate is the only place with `unsafe`: calling the generated
//! code (`CraneliftJit::run`).

mod analysis;
mod codegen;

use std::collections::{HashMap, HashSet};

use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Module as _};

use grd_vm::bytecode::Op;
use grd_vm::vm::jit::{CompileRequest, JitBackend, JitCode, JitStatus};

use analysis::{analyse, declared_sig, Analysis, CallKind, Ctx, Sig};
use codegen::{buf_words, signature, Gen, NativeCallee, ST_EXIT, ST_RETURN};

/// A function compiled for native calls.
#[derive(Clone)]
struct NativeFn {
    func: FuncId,
    buf_words: usize,
    param_slots: Vec<u32>,
}

pub struct CraneliftJit {
    module: JITModule,
    fctx: FunctionBuilderContext,
    /// Entry points by code id.
    codes: Vec<*const u8>,
    /// Natively callable whole functions by (module, unit).
    natives: HashMap<(u32, u32), NativeFn>,
    failed: HashSet<(u32, u32)>,
    /// Count of compiled regions/functions (diagnostics).
    pub compiled: usize,
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
                eprintln!("grd-jit: code {id}: {}", v.iter().take(4).map(|((s, ip), n)| format!("status {s} ip {ip} x{n}")).collect::<Vec<_>>().join(", "));
            }
        }
    }
}

impl Default for CraneliftJit {
    fn default() -> Self {
        Self::new()
    }
}

impl CraneliftJit {
    pub fn new() -> Self {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").expect("opt_level");
        let isa = cranelift_native::builder().expect("host isa").finish(settings::Flags::new(flags)).expect("isa");
        let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        CraneliftJit {
            module: JITModule::new(builder),
            fctx: FunctionBuilderContext::new(),
            codes: Vec::new(),
            natives: HashMap::new(),
            failed: HashSet::new(),
            compiled: 0,
            log: std::env::var("GRD_JIT_LOG").is_ok(),
            stats: Vec::new(),
        }
    }

    /// Analyse and translate one whole function or loop region; `None` if it
    /// uses anything outside the supported subset. Callees are compiled first.
    fn build(&mut self, req: &CompileRequest, module: u32, unit_id: u32, entry: usize, end: Option<usize>, tags: &[u64], in_progress: &mut Vec<(u32, u32)>) -> Option<(FuncId, Analysis)> {
        let m = req.modules[module as usize].clone();
        let unit = &m.units[unit_id as usize];
        let whole = end.is_none();
        let sig_of = |mi: u32, ui: u32| -> Option<Sig> { declared_sig(&req.modules[mi as usize].units[ui as usize]) };
        let cx = Ctx { module: &m, unit, functions: req.functions, sig_of: &sig_of };
        let (lo, hi) = match end {
            None => (0, unit.ops.len().saturating_sub(1)),
            Some(h) => (entry, h),
        };
        let an = analyse(&cx, lo, hi, entry, tags, whole)?;
        let func = self.module.declare_anonymous_function(&signature(&self.module)).ok()?;
        if whole {
            // known before the body so that self-recursion calls itself natively
            let param_slots = unit.params.iter().map(|p| p.slot).collect();
            self.natives.insert((module, unit_id), NativeFn { func, buf_words: buf_words(unit, &an), param_slots });
        }
        in_progress.push((module, unit_id));
        // callees first
        let mut callee_fns: HashMap<usize, NativeFn> = HashMap::new();
        for (ip, kind) in &an.calls {
            if let CallKind::User { module: cm, unit: cu, .. } = kind {
                let key = (*cm, *cu);
                let f = match self.natives.get(&key) {
                    Some(f) => f.clone(),
                    None => {
                        if self.failed.contains(&key) || in_progress.contains(&key) {
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
        ctx.func.signature = signature(&self.module);
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut self.fctx);
            let mut callees = HashMap::new();
            for (ip, f) in &callee_fns {
                let fref = self.module.declare_func_in_func(f.func, b.func);
                callees.insert(*ip, NativeCallee { fref, buf_words: f.buf_words, param_slots: f.param_slots.clone() });
            }
            let mut g = Gen::new(&mut b, &m, unit, &an, whole, callees);
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
        self.compiled += 1;
        if self.log {
            eprintln!("grd-jit: compiled {} {} ops {}..={} (stack {}, calls {})", if whole { "function" } else { "loop" }, unit.name, lo, hi, an.max_stack, an.calls.len());
        }
        Some((func, an))
    }

    fn abandon(&mut self, whole: bool, module: u32, unit: u32, in_progress: &mut Vec<(u32, u32)>) {
        if whole {
            self.natives.remove(&(module, unit));
            self.failed.insert((module, unit));
        }
        in_progress.retain(|k| *k != (module, unit));
    }
}

impl JitBackend for CraneliftJit {
    fn compile(&mut self, req: &CompileRequest) -> Option<JitCode> {
        let unit = &req.modules[req.module as usize].units[req.unit as usize];
        if req.end_ip.is_none() && self.failed.contains(&(req.module, req.unit)) {
            return None;
        }
        // a loop region must start at a loop header of this unit
        if let Some(end) = req.end_ip
            && !matches!(unit.ops.get(end), Some(Op::Jump(t)) if *t as usize == req.entry_ip)
        {
            return None;
        }
        let mut in_progress = Vec::new();
        let (func, an) = match self.natives.get(&(req.module, req.unit)) {
            Some(f) if req.end_ip.is_none() => {
                let f = f.clone();
                let cx_unit = unit;
                let an_sig = declared_sig(cx_unit)?;
                let id = self.codes.len() as u32;
                self.module.finalize_definitions().ok()?;
                self.codes.push(self.module.get_finalized_function(f.func));
                return Some(JitCode { id, buf_words: f.buf_words, guards: Vec::new(), params: an_sig.params.iter().map(|t| t.tag()).collect() });
            }
            _ => self.build(req, req.module, req.unit, req.entry_ip, req.end_ip, req.entry_tags, &mut in_progress)?,
        };
        if self.module.finalize_definitions().is_err() {
            return None;
        }
        let ptr = self.module.get_finalized_function(func);
        let id = self.codes.len() as u32;
        self.codes.push(ptr);
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
        Some(JitCode { id, buf_words: words, guards, params })
    }

    fn run(&mut self, id: u32, buf: &mut [u64]) -> JitStatus {
        let ptr = self.codes[id as usize];
        // SAFETY: `ptr` is a function finalized by `self.module` with the
        // signature `extern "C" fn(*mut u64) -> i64` (codegen::signature, the
        // ISA's default C calling convention). It reads and writes only words
        // inside the state buffer, whose length the VM sized from
        // `JitCode::buf_words` (all offsets are below that bound by
        // construction in codegen), and the module — hence the code — lives as
        // long as `self`.
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

/// Install the JIT into a VM unless `GRD_JIT=0`.
pub fn install(vm: &mut grd_vm::vm::Vm) {
    if std::env::var("GRD_JIT").as_deref() == Ok("0") {
        return;
    }
    vm.set_jit(Box::new(CraneliftJit::new()));
}
