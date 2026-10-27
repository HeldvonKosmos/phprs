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
/// An array (JIT2b): the bits are `Rc::as_ptr` of its `Rc<PhpArray>`. The VM
/// only hands out the address; reading through it is the backend's job.
pub const TAG_ARRAY: u64 = 4;
/// An object (JITO1): the bits are its `ObjHandle`. Handles carry no
/// reference count and stay valid for the whole request; compiled code
/// never runs a collection, so a handle it holds cannot go stale.
pub const TAG_OBJECT: u64 = 5;
/// Any other value (object, string, null) — only in entry tags.
pub const TAG_OTHER: u64 = u64::MAX;

/// Kinds of array elements, as recorded in `CodeProfile::types` at the
/// dim-fetch ops and returned by `dim_int`.
pub const KIND_NONE: u64 = 0;
pub const KIND_INT: u64 = 1;
pub const KIND_FLOAT: u64 = 2;
pub const KIND_BOOL: u64 = 4;
pub const KIND_ARRAY: u64 = 8;
pub const KIND_OTHER: u64 = 16;
/// A property read by compiled code that holds an object (JITO1).
pub const KIND_OBJECT: u64 = 32;

#[inline]
pub fn elem_kind(v: &crate::value::Val) -> u8 {
    (match v {
        crate::value::Val::Int(_) => KIND_INT,
        crate::value::Val::Float(_) => KIND_FLOAT,
        crate::value::Val::Bool(_) => KIND_BOOL,
        crate::value::Val::Array(_) => KIND_ARRAY,
        _ => KIND_OTHER,
    }) as u8
}

/// JIT2b runtime read `$a[$key]` with an int key: (kind, bits). Scalars as in
/// the state buffer, nested arrays as their address (borrowed: valid while
/// `a` is alive and unchanged), anything else or a missing key `KIND_NONE`.
pub fn dim_int(a: &crate::value::PhpArray, key: i64) -> (u64, u64) {
    use crate::value::{ArrayKey, Val};
    match a.get(&ArrayKey::Int(key)) {
        Some(Val::Int(i)) => (KIND_INT, *i as u64),
        Some(Val::Float(f)) => (KIND_FLOAT, f.to_bits()),
        Some(Val::Bool(b)) => (KIND_BOOL, *b as u64),
        Some(Val::Array(inner)) => (KIND_ARRAY, Rc::as_ptr(inner) as u64),
        _ => (KIND_NONE, 0),
    }
}

/// Buffer layout in `u64` words: `[0]` exit ip, `[1]` operand-stack depth,
/// `[2]` return tag, `[3]` return bits, `[4]` the handle of `$this` (methods,
/// JITO1), then two words (tag, bits) per slot of the unit, then two words
/// per operand-stack entry.
pub const HDR: usize = 5;
pub const THIS_WORD: usize = 4;

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

// ---- classes as compiled code sees them (JITO1) -----------------------------

/// Type of a property or parameter that compiled code can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JitTy {
    Int,
    Float,
    Bool,
    /// An object of this class or a subclass (never null).
    Obj(u32),
}

/// A declared property with a type compiled code can hold.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JitProp {
    pub slot: u32,
    pub ty: JitTy,
    pub public: bool,
    pub readonly: bool,
}

/// What compiled code may assume about a linked class. A subclass keeps the
/// properties of its parent at the same slots and with the same types
/// (prefix layout, JIT_OBJECTS P0), so a slot found for a class is right for
/// every object of a subclass too.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct JitClass {
    pub parent: Option<u32>,
    pub is_final: bool,
    /// Property selector → property, sorted by selector.
    pub props: Vec<(u32, JitProp)>,
    /// `new` without the interpreter: the class is concrete, not native,
    /// not throwable, and has no constructor or one that only promotes its
    /// parameters; one (type, slot) per parameter.
    pub new: Option<Vec<(JitTy, u32)>>,
}

/// The classes of a VM for the JIT, indexed by `ClassId`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct JitClasses {
    pub classes: Vec<JitClass>,
    /// Lowercase name → id, sorted; classes only (no interfaces, traits, enums).
    pub by_name: Vec<(Vec<u8>, u32)>,
}

impl JitClasses {
    pub fn lookup(&self, name: &[u8]) -> Option<u32> {
        let n = name.strip_prefix(b"\\").unwrap_or(name).to_ascii_lowercase();
        self.by_name.binary_search_by(|(k, _)| k.as_slice().cmp(&n)).ok().map(|i| self.by_name[i].1)
    }
    /// `d` is `c` or extends it.
    pub fn is_sub(&self, d: u32, c: u32) -> bool {
        let mut cur = Some(d);
        while let Some(k) = cur {
            if k == c {
                return true;
            }
            cur = self.classes.get(k as usize).and_then(|x| x.parent);
        }
        false
    }
    pub fn prop(&self, class: u32, sel: u32) -> Option<&JitProp> {
        let c = self.classes.get(class as usize)?;
        c.props.binary_search_by_key(&sel, |(s, _)| *s).ok().map(|i| &c.props[i].1)
    }
    pub fn get(&self, class: u32) -> Option<&JitClass> {
        self.classes.get(class as usize)
    }
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
    /// JIT2b: element kinds at the dim fetches of every unit,
    /// `[module][unit][ip]`, so that callees compiled along are typed too.
    pub unit_types: &'a [Vec<Vec<u8>>],
    /// JITO1: the class of `$this` for a method (its declaring class).
    pub this_class: Option<u32>,
    pub classes: &'a std::sync::Arc<JitClasses>,
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
    /// For `TAG_OBJECT` parameters the class the argument must be an
    /// instance of (`u32::MAX` for the others).
    pub param_classes: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JitStatus {
    /// Continue interpreting at `buf[0]` with the slots and stack from the buffer.
    Exit,
    /// Whole function returned `buf[2..4]`.
    Return,
    /// Nothing happened that the interpreter can see: run the original code.
    Bailout,
    /// A bailout because native calls between compiled functions nested too
    /// deep: the interpreter runs the deeper levels of this call itself.
    TooDeep,
}

pub trait JitBackend {
    fn compile(&mut self, req: &CompileRequest) -> Option<JitCode>;
    /// Run compiled code; `vm` is handed to the runtime functions the code
    /// calls (properties, `new`), the backend itself is taken out of it.
    fn run(&mut self, id: u32, buf: &mut [u64], vm: &mut Vm) -> JitStatus;
    /// Current tier of every compiled code id (diagnostics, tests).
    fn tiers(&self) -> Vec<u8> {
        Vec::new()
    }
    /// A reference value the compiled code left in the buffer (`TAG_ARRAY`
    /// on the operand stack at an exit), as a VM value.
    fn ref_val(&self, _tag: u64, _bits: u64) -> Option<Val> {
        None
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

use crate::bytecode::{ClassKind, Op, TypeHint, Visibility};
use crate::value::{ClassId, ObjHandle, PhpObject, Val};

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
        Some(Val::Array(_)) => TAG_ARRAY,
        Some(_) => TAG_OTHER,
    }
}

fn bits_of(v: &Val) -> u64 {
    match v {
        Val::Int(i) => *i as u64,
        Val::Float(f) => f.to_bits(),
        Val::Bool(b) => *b as u64,
        Val::Array(a) => Rc::as_ptr(a) as u64,
        _ => 0,
    }
}

fn val_of(tag: u64, bits: u64) -> Option<Val> {
    match tag {
        TAG_INT => Some(Val::Int(bits as i64)),
        TAG_FLOAT => Some(Val::Float(f64::from_bits(bits))),
        TAG_BOOL => Some(Val::Bool(bits != 0)),
        TAG_OBJECT => Some(Val::Object(ObjHandle(bits as u32))),
        _ => None,
    }
}

/// A parameter or property type as compiled code sees it.
fn jit_ty(vm: &Vm, hint: Option<&TypeHint>, module: u32) -> Option<JitTy> {
    Some(match hint? {
        TypeHint::Int => JitTy::Int,
        TypeHint::Float => JitTy::Float,
        TypeHint::Bool => JitTy::Bool,
        TypeHint::Class(name) => {
            let c = vm.lookup_class(vm.str_of(module, *name))?;
            if vm.class(c).kind != ClassKind::Class {
                return None;
            }
            JitTy::Obj(c.0)
        }
        _ => return None,
    })
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
        let unit_types: Vec<Vec<Vec<u8>>> = self.modules.iter().enumerate().map(|(mi, m)| (0..m.units.len()).map(|u| self.profiles[self.ic_base[mi] + u].types.clone()).collect()).collect();
        let classes = self.jit_classes();
        let this_class = self.jit_this.take();
        let req = CompileRequest { modules: &modules, module, unit, entry_ip, end_ip, entry_tags, functions: &functions, profile: &profile, unit_types: &unit_types, this_class, classes: &classes };
        self.jit.as_mut()?.compile(&req)
    }

    /// The class table for the JIT, rebuilt when classes were added.
    fn jit_classes(&mut self) -> std::sync::Arc<JitClasses> {
        if let Some((n, t)) = &self.jit_class_table
            && *n == self.classes.len()
        {
            return t.clone();
        }
        let mut t = JitClasses::default();
        for (i, c) in self.classes.iter().enumerate() {
            let id = ClassId(i as u32);
            let mut props: Vec<(u32, JitProp)> = c
                .props_by_sel
                .iter()
                .filter_map(|(sel, slot)| {
                    let p = c.props.get(*slot as usize)?;
                    let ty = jit_ty(self, p.ty.as_ref(), p.module)?;
                    Some((sel.0, JitProp { slot: *slot, ty, public: p.visibility == Visibility::Public, readonly: p.readonly }))
                })
                .collect();
            props.sort_by_key(|(s, _)| *s);
            let new = (c.kind == ClassKind::Class && !c.is_abstract && self.native_info(id).is_none() && !self.is_throwable(id)).then(|| self.promoting_ctor(id)).flatten();
            t.classes.push(JitClass { parent: c.parent.map(|p| p.0), is_final: c.is_final, props, new });
            if c.kind == ClassKind::Class {
                t.by_name.push((c.name.to_ascii_lowercase(), i as u32));
            }
        }
        t.by_name.sort();
        let t = std::sync::Arc::new(t);
        self.jit_class_table = Some((self.classes.len(), t.clone()));
        t
    }

    /// (type, slot) per constructor parameter when the constructor only
    /// promotes its parameters (or there is none); `None` otherwise.
    fn promoting_ctor(&self, class: ClassId) -> Option<Vec<(JitTy, u32)>> {
        let Some(m) = self.class(class).find_method(b"__construct") else { return Some(Vec::new()) };
        let (Some(u), None, Visibility::Public) = (m.unit, m.native, m.visibility) else { return None };
        let unit = &self.modules[m.module as usize].units[u as usize];
        if unit.params.iter().any(|p| p.variadic || p.has_default || p.promote.is_none()) {
            return None;
        }
        let mut out = Vec::new();
        let mut params = unit.params.iter();
        for op in &unit.ops {
            match op {
                Op::Nop | Op::Line(_) => {}
                Op::PromoteParam { slot, prop } => {
                    let p = params.next()?;
                    if p.slot != *slot {
                        return None;
                    }
                    let idx = *self.class(class).prop_index.get(self.str_of(m.module, *prop))?;
                    out.push((jit_ty(self, p.ty.as_ref(), m.module)?, idx as u32));
                }
                Op::PushNull => {}
                Op::Return => break,
                _ => return None,
            }
        }
        (out.len() == unit.params.len()).then_some(out)
    }

    /// JITO1 runtime read of property `slot` of the object with handle `h`
    /// for compiled code: (kind, bits) as in the state buffer; `KIND_NONE`
    /// for an unset slot (compiled code then leaves and the interpreter
    /// raises the error).
    pub fn jit_prop_get(&self, h: u64, slot: u64) -> (u64, u64) {
        let h = ObjHandle(h as u32);
        if !self.objects.is_valid(h) {
            return (KIND_NONE, 0);
        }
        match self.objects.get(h).slots.get(slot as usize) {
            Some(Some(Val::Int(i))) => (KIND_INT, *i as u64),
            Some(Some(Val::Float(f))) => (KIND_FLOAT, f.to_bits()),
            Some(Some(Val::Bool(b))) => (KIND_BOOL, *b as u64),
            Some(Some(Val::Object(o))) => (KIND_OBJECT, o.0 as u64),
            _ => (KIND_NONE, 0),
        }
    }

    /// JITO1 property write by compiled code. The analysis has proved type,
    /// visibility and that the property is not readonly, so there is nothing
    /// to fail; an invalid handle or slot (a JIT bug) writes nothing.
    pub fn jit_prop_set(&mut self, h: u64, slot: u64, tag: u64, bits: u64) {
        let (h, Some(v)) = (ObjHandle(h as u32), val_of(tag, bits)) else { return };
        if self.objects.is_valid(h)
            && let Some(s) = self.objects.get_mut(h).slots.get_mut(slot as usize)
        {
            *s = Some(v);
        }
    }

    /// JITO1 `new C(...)` by compiled code for a class whose constructor
    /// only promotes its parameters: `args` holds (slot, tag, bits) per
    /// parameter. Checks first, then allocates: 0 means nothing happened
    /// (object limit reached, property defaults not evaluated yet) and the
    /// interpreter runs `new` itself, raising what it raises.
    pub fn jit_new(&mut self, class: u32, args: &[u64]) -> u64 {
        let cid = ClassId(class);
        let Some(c) = self.classes.get(class as usize) else { return 0 };
        let mut slots = match &c.default_slots {
            Some(d) => d.clone(),
            None if c.props.iter().all(|p| p.default.is_none()) => vec![None; c.props.len()],
            None => return 0,
        };
        if self.check_object_limit().is_err() {
            return 0;
        }
        for a in args.chunks_exact(3) {
            if let (Some(s), Some(v)) = (slots.get_mut(a[0] as usize), val_of(a[1], a[2])) {
                *s = Some(v);
            }
        }
        let mut obj = PhpObject::new(cid, 0);
        obj.slots = slots;
        self.objects.alloc(obj).0 as u64
    }

    /// Whole-function call through the JIT: the arguments are the top `n`
    /// values of the operand stack. `Some(value)` if compiled code ran and
    /// returned (the arguments are popped); `None` to call it normally.
    pub(crate) fn jit_call(&mut self, module: u32, unit: UnitId, ic: usize, n: usize) -> Option<Val> {
        let at = self.stack.len() - n;
        let out = self.jit_enter(module, unit, ic, None, None, at, n);
        if out.is_some() {
            self.stack.truncate(at);
        }
        out
    }

    /// JITO1: a method call through the JIT with `$this` = `this` (method
    /// declared in `class`); the object and the `n` arguments are on top of
    /// the operand stack. `Some(value)` if compiled code ran and returned
    /// (object and arguments are popped); `None` to call it normally.
    pub(crate) fn jit_call_method(&mut self, module: u32, unit: UnitId, ic: usize, this: ObjHandle, class: ClassId, n: usize) -> Option<Val> {
        let at = self.stack.len() - n;
        let out = self.jit_enter(module, unit, ic, Some(this), Some(class.0), at, n);
        if out.is_some() {
            self.stack.truncate(at - 1);
        }
        out
    }

    /// Compile when hot, then fill the state buffer from `stack[at..at+n]`
    /// and run; the stack is left as it is.
    #[allow(clippy::too_many_arguments)]
    fn jit_enter(&mut self, module: u32, unit: UnitId, ic: usize, this: Option<ObjHandle>, this_class: Option<u32>, at: usize, n: usize) -> Option<Val> {
        // below a call that was too deep for native recursion the interpreter
        // keeps going (its frames are on the heap); entering compiled code at
        // every level again would cost the native depth each time
        if self.jit_deep_mark != usize::MAX {
            if self.frames.len() > self.jit_deep_mark {
                return None;
            }
            self.jit_deep_mark = usize::MAX;
        }
        if matches!(self.jit_funcs[ic], JitSlotState::Untried) {
            if self.profiles[ic].calls < self.jit_hot.0 {
                return None;
            }
            let num_slots = self.modules[module as usize].units[unit as usize].num_slots as usize;
            let mut tags = vec![TAG_UNSET; num_slots];
            for (i, p) in self.modules[module as usize].units[unit as usize].params.iter().enumerate().take(n) {
                tags[p.slot as usize] = tag_of(Some(&self.stack[at + i]));
            }
            self.jit_this = this_class;
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
        let mut buf = std::mem::take(&mut self.jit_buf);
        buf.clear();
        buf.resize(words, 0);
        buf[THIS_WORD] = this.map_or(0, |h| h.0 as u64);
        let unit_ref = &self.modules[module as usize].units[unit as usize];
        for (i, p) in unit_ref.params.iter().enumerate().take(n) {
            let v = &self.stack[at + i];
            let want = code.params[i];
            let (tag, bits) = match (want, v) {
                (TAG_FLOAT, Val::Int(x)) => (TAG_FLOAT, (*x as f64).to_bits()),
                // the compiled code relies on the class: anything else goes
                // to the interpreter, which raises the TypeError
                (TAG_OBJECT, Val::Object(h)) if self.instance_of(self.objects.get(*h).class, ClassId(code.param_classes[i])) => (TAG_OBJECT, h.0 as u64),
                _ if want != TAG_OBJECT && tag_of(Some(v)) == want => (want, bits_of(v)),
                _ => {
                    self.jit_buf = buf;
                    return None;
                }
            };
            let w = slot_word(p.slot as usize);
            buf[w] = tag;
            buf[w + 1] = bits;
        }
        // the backend is taken out so that its runtime functions can use the VM
        let mut jit = self.jit.take();
        let status = jit.as_mut().map(|j| j.run(id, &mut buf, self));
        self.jit = jit;
        if status == Some(JitStatus::TooDeep) {
            self.jit_deep_mark = self.frames.len();
        }
        let out = if status == Some(JitStatus::Return) { val_of(buf[2], buf[3]) } else { None };
        self.jit_buf = buf;
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
        let mut jit = self.jit.take();
        let status = jit.as_mut().map(|j| j.run(id, &mut buf, self));
        self.jit = jit;
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
                    let (tag, bits) = (buf[sbase + 2 * d], buf[sbase + 2 * d + 1]);
                    let v = match tag {
                        TAG_ARRAY => self.jit.as_ref().and_then(|j| j.ref_val(tag, bits)),
                        _ => val_of(tag, bits),
                    };
                    self.stack.push(v.unwrap_or(Val::Null));
                }
                LoopOutcome::Resume(buf[0] as usize)
            }
            _ => LoopOutcome::NotEntered,
        };
        self.jit_buf = buf;
        outcome
    }
}
