//! JIT hook (JIT1b, DECISIONS D6 "Cranelift direkt"): a backend — the crate
//! `grd-jit` — compiles hot code to machine code. The VM stays free of
//! `unsafe`: it never hands out pointers into its own structures. State goes
//! to the backend as plain `u64` words and comes back the same way.
//!
//! Two kinds of compiled code:
//! - **whole functions** (`end_ip == None`): pure numeric functions (int,
//!   float, bool); the VM calls them with the arguments and gets the return
//!   value. Any guard failure (overflow, division by zero, unexpected type)
//!   returns `Bailout`; because such functions have no side effects, the VM
//!   simply runs the call in the interpreter instead.
//! - **loop regions** (`end_ip == Some(back-edge ip)`, on-stack replacement):
//!   entered at a hot loop header with the current slot values; leaving the
//!   loop or failing a guard returns `Exit` with slots, operand stack and the
//!   ip where the interpreter continues.

use std::rc::Rc;

use crate::bytecode::{Module, Slot, UnitId};

use super::CodeProfile;

/// Slot not set / not written back.
pub const TAG_UNSET: u64 = 0;
pub const TAG_INT: u64 = 1;
/// Bits are `f64::to_bits`.
pub const TAG_FLOAT: u64 = 2;
/// Bits 0 or 1.
pub const TAG_BOOL: u64 = 3;
/// Any other value (array, object, string, null) — only in entry tags.
pub const TAG_OTHER: u64 = u64::MAX;

/// Buffer layout in `u64` words: `[0]` exit ip, `[1]` operand-stack depth,
/// `[2]` return tag, `[3]` return bits, then two words (tag, bits) per slot
/// of the unit, then two words per operand-stack entry.
pub const HDR: usize = 4;

#[inline]
pub fn slot_word(slot: usize) -> usize {
    HDR + 2 * slot
}

/// What a function id refers to, for the backend.
#[derive(Clone, Debug)]
pub enum Callee {
    User { module: u32, unit: UnitId },
    Native(String),
}

pub struct CompileRequest<'a> {
    pub modules: &'a [Rc<Module>],
    pub module: u32,
    pub unit: UnitId,
    /// 0 for a whole function, the loop header for a loop region.
    pub entry_ip: usize,
    /// `None`: whole function; `Some(ip)`: loop region `entry_ip..=ip`.
    pub end_ip: Option<usize>,
    /// Tag of every slot of the unit at entry (`TAG_*`); for whole functions
    /// only the parameters matter.
    pub entry_tags: &'a [u64],
    /// Function table of the VM, indexed by function id.
    pub functions: &'a [Callee],
    pub profile: &'a CodeProfile,
}

/// A compiled unit or region.
#[derive(Clone, Debug)]
pub struct JitCode {
    pub id: u32,
    /// Words the state buffer needs.
    pub buf_words: usize,
    /// Slots whose entry tag must equal the given tag for the code to be
    /// entered (loop regions; whole functions check their parameters).
    pub guards: Vec<(Slot, u64)>,
    /// Parameter tags of a whole function, in order.
    pub params: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JitStatus {
    /// Continue interpreting at `buf[0]` with the slots and stack from the buffer.
    Exit,
    /// Whole function returned `buf[2..4]`.
    Return,
    /// Nothing happened that the interpreter can see: run the original code.
    Bailout,
}

pub trait JitBackend {
    fn compile(&mut self, req: &CompileRequest) -> Option<JitCode>;
    fn run(&mut self, id: u32, buf: &mut [u64]) -> JitStatus;
    /// Current tier of every compiled code id (diagnostics, tests).
    fn tiers(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// Compilation state of a unit or a loop region in the VM.
#[derive(Clone, Debug, Default)]
pub enum JitSlotState {
    #[default]
    Untried,
    Failed,
    Ready(JitCode),
}

/// Calls before a function is compiled; back-edges before a loop is.
pub const JIT_HOT_CALLS: u32 = 1_000;
pub const JIT_HOT_BACKEDGES: u32 = 2_000;

// ---- VM side ---------------------------------------------------------------

use crate::value::Val;

use super::{FuncRef, Vm};

/// What a loop-region run did.
pub(crate) enum LoopOutcome {
    /// Not entered (not hot, not compilable, guards do not match).
    NotEntered,
    /// Continue interpreting at this ip; slots and stack were written back.
    Resume(usize),
}

pub(crate) fn tag_of(v: Option<&Val>) -> u64 {
    match v {
        None => TAG_UNSET,
        Some(Val::Int(_)) => TAG_INT,
        Some(Val::Float(_)) => TAG_FLOAT,
        Some(Val::Bool(_)) => TAG_BOOL,
        Some(_) => TAG_OTHER,
    }
}

fn bits_of(v: &Val) -> u64 {
    match v {
        Val::Int(i) => *i as u64,
        Val::Float(f) => f.to_bits(),
        Val::Bool(b) => *b as u64,
        _ => 0,
    }
}

fn val_of(tag: u64, bits: u64) -> Option<Val> {
    match tag {
        TAG_INT => Some(Val::Int(bits as i64)),
        TAG_FLOAT => Some(Val::Float(f64::from_bits(bits))),
        TAG_BOOL => Some(Val::Bool(bits != 0)),
        _ => None,
    }
}

impl Vm {
    /// Install a JIT backend (the `grd` binary and the framework do this
    /// unless `GRD_JIT=0`).
    pub fn set_jit(&mut self, backend: Box<dyn JitBackend>) {
        self.jit = Some(backend);
        // GRD_JIT_HOT=n: compile after n calls / back-edges (tests use 1)
        if let Some(n) = std::env::var("GRD_JIT_HOT").ok().and_then(|v| v.parse::<u32>().ok()) {
            self.jit_hot = (n.max(1), n.max(1));
        }
    }

    /// Compile after `calls` calls of a function / `backedges` back-edges
    /// in a unit (defaults `JIT_HOT_CALLS`, `JIT_HOT_BACKEDGES`).
    pub fn set_jit_threshold(&mut self, calls: u32, backedges: u32) {
        self.jit_hot = (calls.max(1), backedges.max(1));
    }

    /// Tier of every compiled function or loop (1 = quick, 2 = optimized).
    pub fn jit_tiers(&self) -> Vec<u8> {
        self.jit.as_ref().map(|j| j.tiers()).unwrap_or_default()
    }

    pub fn jit_enabled(&self) -> bool {
        self.jit.is_some()
    }

    fn callee_table(&self) -> Vec<Callee> {
        self.function_table
            .iter()
            .map(|f| match f {
                FuncRef::User { module, unit } => Callee::User { module: *module, unit: *unit },
                FuncRef::Native(i) => Callee::Native(self.natives[*i as usize].0.clone()),
            })
            .collect()
    }

    fn jit_compile(&mut self, module: u32, unit: UnitId, entry_ip: usize, end_ip: Option<usize>, entry_tags: &[u64], ic: usize) -> Option<JitCode> {
        let functions = self.callee_table();
        let modules = self.modules.clone();
        let profile = self.profiles[ic].clone();
        let req = CompileRequest { modules: &modules, module, unit, entry_ip, end_ip, entry_tags, functions: &functions, profile: &profile };
        self.jit.as_mut()?.compile(&req)
    }

    /// Whole-function call through the JIT: the arguments are the top `n`
    /// values of the operand stack. `Some(value)` if compiled code ran and
    /// returned (the arguments are popped); `None` to call it normally.
    pub(crate) fn jit_call(&mut self, module: u32, unit: UnitId, ic: usize, n: usize) -> Option<Val> {
        if matches!(self.jit_funcs[ic], JitSlotState::Untried) {
            if self.profiles[ic].calls < self.jit_hot.0 {
                return None;
            }
            let num_slots = self.modules[module as usize].units[unit as usize].num_slots as usize;
            let at = self.stack.len() - n;
            let mut tags = vec![TAG_UNSET; num_slots];
            for (i, p) in self.modules[module as usize].units[unit as usize].params.iter().enumerate().take(n) {
                tags[p.slot as usize] = tag_of(Some(&self.stack[at + i]));
            }
            self.jit_funcs[ic] = match self.jit_compile(module, unit, 0, None, &tags, ic) {
                Some(code) => JitSlotState::Ready(code),
                None => JitSlotState::Failed,
            };
        }
        let JitSlotState::Ready(code) = &self.jit_funcs[ic] else { return None };
        if code.params.len() != n {
            return None;
        }
        let (id, words) = (code.id, code.buf_words);
        let at = self.stack.len() - n;
        let mut buf = std::mem::take(&mut self.jit_buf);
        buf.clear();
        buf.resize(words, 0);
        let unit_ref = &self.modules[module as usize].units[unit as usize];
        for (i, p) in unit_ref.params.iter().enumerate().take(n) {
            let v = &self.stack[at + i];
            let want = code.params[i];
            let (tag, bits) = match (want, v) {
                (TAG_FLOAT, Val::Int(x)) => (TAG_FLOAT, (*x as f64).to_bits()),
                _ if tag_of(Some(v)) == want => (want, bits_of(v)),
                _ => {
                    self.jit_buf = buf;
                    return None;
                }
            };
            let w = slot_word(p.slot as usize);
            buf[w] = tag;
            buf[w + 1] = bits;
        }
        let status = self.jit.as_mut().map(|j| j.run(id, &mut buf));
        let out = if status == Some(JitStatus::Return) { val_of(buf[2], buf[3]) } else { None };
        self.jit_buf = buf;
        if out.is_some() {
            self.stack.truncate(at);
        }
        out
    }

    /// Hot back-edge `backedge_ip -> header` in frame `fidx`: run the loop
    /// as compiled code when possible.
    pub(crate) fn jit_loop(&mut self, fidx: usize, ic: usize, slot_base: usize, header: usize, backedge_ip: usize) -> LoopOutcome {
        if self.stack.len() != self.frames[fidx].stack_base {
            return LoopOutcome::NotEntered;
        }
        let (module, unit) = (self.frames[fidx].module_idx, self.frames[fidx].unit);
        let num_slots = self.frames[fidx].num_slots as usize;
        let key = (ic, header);
        if !self.jit_loops.contains_key(&key) {
            let tags: Vec<u64> = (0..num_slots).map(|i| tag_of(self.slot_stack[slot_base + i].as_ref())).collect();
            let state = match self.jit_compile(module, unit, header, Some(backedge_ip), &tags, ic) {
                Some(code) => JitSlotState::Ready(code),
                None => JitSlotState::Failed,
            };
            self.jit_loops.insert(key, state);
        }
        let Some(JitSlotState::Ready(code)) = self.jit_loops.get(&key) else { return LoopOutcome::NotEntered };
        for (slot, tag) in &code.guards {
            if tag_of(self.slot_stack[slot_base + *slot as usize].as_ref()) != *tag {
                return LoopOutcome::NotEntered;
            }
        }
        let (id, words) = (code.id, code.buf_words);
        let mut buf = std::mem::take(&mut self.jit_buf);
        buf.clear();
        buf.resize(words, 0);
        for i in 0..num_slots {
            if let Some(v) = &self.slot_stack[slot_base + i] {
                let t = tag_of(Some(v));
                if t != TAG_OTHER {
                    buf[slot_word(i)] = t;
                    buf[slot_word(i) + 1] = bits_of(v);
                }
            }
        }
        let status = self.jit.as_mut().map(|j| j.run(id, &mut buf));
        let outcome = match status {
            Some(JitStatus::Exit) => {
                for i in 0..num_slots {
                    let w = slot_word(i);
                    if let Some(v) = val_of(buf[w], buf[w + 1]) {
                        self.slot_stack[slot_base + i] = Some(v);
                    }
                }
                let depth = buf[1] as usize;
                let sbase = slot_word(num_slots);
                for d in 0..depth {
                    let v = val_of(buf[sbase + 2 * d], buf[sbase + 2 * d + 1]).unwrap_or(Val::Null);
                    self.stack.push(v);
                }
                LoopOutcome::Resume(buf[0] as usize)
            }
            _ => LoopOutcome::NotEntered,
        };
        self.jit_buf = buf;
        outcome
    }
}
