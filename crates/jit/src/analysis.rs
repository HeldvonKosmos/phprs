//! Type analysis of a whole function or a loop region before code
//! generation. Slots get one type for the whole region (int, float, bool,
//! or "num" = int or float at run time); whether a slot is defined is
//! tracked per op; the operand stack is typed per op. Anything outside the
//! supported subset makes the region uncompilable.

use std::collections::HashMap;

use grd_vm::bytecode::{BinOp, CodeUnit, Const, Init, Module, Op, TypeHint, UnOp, WriteMode};
use grd_vm::vm::jit::{Callee, KIND_ARRAY, KIND_BOOL, KIND_FLOAT, KIND_INT, TAG_ARRAY, TAG_BOOL, TAG_FLOAT, TAG_INT, TAG_OTHER, TAG_UNSET};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Ty {
    Int,
    Float,
    Bool,
    /// Int or float, decided at run time.
    Num,
    /// JIT2b: an array, read only in compiled code (the address of its
    /// `PhpArray`, borrowed from a slot, a parameter or an outer array).
    Arr,
}

impl Ty {
    pub fn numeric(self) -> bool {
        !matches!(self, Ty::Bool | Ty::Arr)
    }
    pub fn scalar(self) -> bool {
        !matches!(self, Ty::Arr)
    }
    pub fn join(self, o: Ty) -> Option<Ty> {
        if self == o {
            Some(self)
        } else if self.numeric() && o.numeric() {
            Some(Ty::Num)
        } else {
            None
        }
    }
    pub fn tag(self) -> u64 {
        match self {
            Ty::Int => TAG_INT,
            Ty::Float => TAG_FLOAT,
            Ty::Bool => TAG_BOOL,
            Ty::Num => TAG_OTHER,
            Ty::Arr => TAG_ARRAY,
        }
    }
    pub fn from_tag(t: u64) -> Option<Ty> {
        match t {
            TAG_INT => Some(Ty::Int),
            TAG_FLOAT => Some(Ty::Float),
            TAG_BOOL => Some(Ty::Bool),
            TAG_ARRAY => Some(Ty::Arr),
            _ => None,
        }
    }
    /// The element type a dim fetch saw (profile byte); only one kind counts.
    pub fn from_kinds(k: u8) -> Option<Ty> {
        match k as u64 {
            KIND_INT => Some(Ty::Int),
            KIND_FLOAT => Some(Ty::Float),
            KIND_BOOL => Some(Ty::Bool),
            KIND_ARRAY => Some(Ty::Arr),
            _ => None,
        }
    }
    pub fn from_hint(h: &TypeHint) -> Option<Ty> {
        match h {
            TypeHint::Int => Some(Ty::Int),
            TypeHint::Float => Some(Ty::Float),
            TypeHint::Bool => Some(Ty::Bool),
            TypeHint::Array => Some(Ty::Arr),
            _ => None,
        }
    }
}

/// Definedness of a slot at an op.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Def {
    No,
    Maybe,
    Yes,
}

impl Def {
    fn join(self, o: Def) -> Def {
        if self == o { self } else { Def::Maybe }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct State {
    pub def: Vec<Def>,
    pub stack: Vec<Ty>,
}

/// Signature of a function that can be called natively.
#[derive(Clone, Debug, PartialEq)]
pub struct Sig {
    pub params: Vec<Ty>,
    pub ret: Ty,
    /// Constant default of each parameter (JIT2a): compiled callers fill in
    /// omitted arguments with it; None = the argument must be passed.
    pub defaults: Vec<Option<Lit>>,
}

/// A scalar constant in compiled code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lit {
    I(i64),
    F(f64),
    B(bool),
}

/// What a call op does in compiled code.
#[derive(Clone, Debug, PartialEq)]
pub enum CallKind {
    IntDiv,
    Max,
    Min,
    Abs,
    /// `count($array)` (JIT2b)
    Count,
    User { module: u32, unit: u32, sig: Sig },
}

pub struct Analysis {
    pub lo: usize,
    pub hi: usize,
    pub entry: usize,
    /// State before each op in `lo..=hi` that is reachable.
    pub states: Vec<Option<State>>,
    /// Region type of each slot that is used (`None` = not used).
    pub slot_ty: Vec<Option<Ty>>,
    /// Slots that may be undefined somewhere (need a defined-flag).
    pub needs_flag: Vec<bool>,
    pub max_stack: usize,
    pub calls: HashMap<usize, CallKind>,
    /// Element type of each dim fetch (JIT2b).
    pub dim_ty: HashMap<usize, Ty>,
    /// For whole functions: parameter and return types.
    pub sig: Option<Sig>,
}

pub struct Ctx<'a> {
    pub module: &'a Module,
    pub unit: &'a CodeUnit,
    pub functions: &'a [Callee],
    /// Signature of a callable user function, if it is eligible.
    pub sig_of: &'a dyn Fn(u32, u32) -> Option<Sig>,
    /// Profile of the unit: element kinds seen at dim fetches (JIT2b);
    /// empty for callees compiled along (their reads then stay uncompiled).
    pub types: &'a [u8],
}

/// Signature from declared types: all parameters int/float/bool, a declared
/// int/float/bool return type, no variadics/generators. Parameters with a
/// default are allowed (JIT2a): compiled code is only called with all
/// arguments, other calls stay in the interpreter.
pub fn declared_sig(module: &Module, unit: &CodeUnit) -> Option<Sig> {
    if unit.is_generator || unit.params.iter().any(|p| p.variadic || p.promote.is_some()) {
        return None;
    }
    let params = unit.params.iter().map(|p| p.ty.as_ref().and_then(Ty::from_hint)).collect::<Option<Vec<_>>>()?;
    // arrays only as parameters: compiled code returns scalars
    let ret = unit.return_type.as_ref().and_then(Ty::from_hint).filter(|t| t.scalar())?;
    let defaults = unit
        .params
        .iter()
        .zip(&params)
        .map(|(p, t)| match (&p.default_init, t) {
            (Some(Init::Const(c)), t) if p.has_default => match (module.consts.get(*c as usize)?, t) {
                (Const::Int(i), Ty::Int) => Some(Lit::I(*i)),
                (Const::Int(i), Ty::Float) => Some(Lit::F(*i as f64)),
                (Const::Float(f), Ty::Float) => Some(Lit::F(*f)),
                (Const::Bool(b), Ty::Bool) => Some(Lit::B(*b)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    Some(Sig { params, ret, defaults })
}

type Fail = ();

fn num_result(op: BinOp, a: Ty, b: Ty) -> Result<Ty, Fail> {
    use BinOp::*;
    if !a.scalar() || !b.scalar() {
        return Err(());
    }
    if !a.numeric() || !b.numeric() {
        return match op {
            Identical | NotIdentical => Ok(Ty::Bool),
            _ => Err(()),
        };
    }
    let any_float = a == Ty::Float || b == Ty::Float;
    Ok(match op {
        Add | Sub | Mul => {
            if any_float {
                Ty::Float
            } else if a == Ty::Int && b == Ty::Int {
                Ty::Int
            } else {
                Ty::Num
            }
        }
        // always float in the dialect (D10)
        Div => Ty::Float,
        Mod | BitAnd | BitOr | BitXor | Spaceship => {
            if a == Ty::Int && b == Ty::Int {
                Ty::Int
            } else {
                return Err(());
            }
        }
        Lt | Le | Gt | Ge | Identical | NotIdentical => Ty::Bool,
        _ => return Err(()),
    })
}

fn const_ty(c: &Const) -> Option<Ty> {
    match c {
        Const::Int(_) => Some(Ty::Int),
        Const::Float(_) => Some(Ty::Float),
        Const::Bool(_) => Some(Ty::Bool),
        _ => None,
    }
}

/// Analyse `lo..=hi` entered at `entry` with the given entry slot tags.
pub fn analyse(cx: &Ctx, lo: usize, hi: usize, entry: usize, entry_tags: &[u64], whole: bool) -> Option<Analysis> {
    let n_slots = cx.unit.num_slots as usize;
    let sig = if whole { Some(declared_sig(cx.module, cx.unit)?) } else { None };
    // flow-insensitive slot types, iterated to a fixpoint together with the flow analysis
    let mut slot_ty: Vec<Option<Ty>> = vec![None; n_slots];
    let mut entry_def = vec![Def::No; n_slots];
    if let Some(sig) = &sig {
        for (p, t) in cx.unit.params.iter().zip(&sig.params) {
            slot_ty[p.slot as usize] = Some(*t);
            entry_def[p.slot as usize] = Def::Yes;
        }
    } else {
        for (i, t) in entry_tags.iter().enumerate().take(n_slots) {
            if let Some(ty) = Ty::from_tag(*t) {
                slot_ty[i] = Some(ty);
                entry_def[i] = Def::Yes;
            }
        }
    }
    let other: Vec<bool> = (0..n_slots).map(|i| !whole && entry_tags.get(i) == Some(&TAG_OTHER)).collect();
    let _ = TAG_UNSET;
    for _round in 0..8 {
        let before = slot_ty.clone();
        let mut states: Vec<Option<State>> = vec![None; hi + 1];
        states[entry] = Some(State { def: entry_def.clone(), stack: Vec::new() });
        let mut work = vec![entry];
        let mut calls = HashMap::new();
        let mut dim_ty = HashMap::new();
        let mut max_stack = 0usize;
        // slots the region touches: they must not be "other" at entry
        while let Some(ip) = work.pop() {
            let st = states[ip].clone().expect("queued state");
            max_stack = max_stack.max(st.stack.len());
            let op = &cx.unit.ops[ip];
            let mut s = st.clone();
            let mut succ: Vec<usize> = Vec::new();
            let mut falls = true;
            macro_rules! pop {
                () => {
                    s.stack.pop()?
                };
            }
            macro_rules! load {
                ($slot:expr) => {{
                    let sl = $slot as usize;
                    if other[sl] || s.def[sl] != Def::Yes {
                        return None;
                    }
                    slot_ty[sl]?
                }};
            }
            macro_rules! store {
                ($slot:expr, $t:expr) => {{
                    let sl = $slot as usize;
                    if other[sl] {
                        return None;
                    }
                    let t: Ty = $t;
                    // arrays are read only: a slot keeps the array it had at entry
                    if !t.scalar() {
                        return None;
                    }
                    // GRD021 (D10): a local keeps its type; a store of another
                    // type raises in the interpreter, so such code is not compiled
                    slot_ty[sl] = Some(match slot_ty[sl] {
                        None => t,
                        Some(o) if o == t => o,
                        Some(_) => return None,
                    });
                    s.def[sl] = Def::Yes;
                }};
            }
            match op {
                Op::Nop | Op::Line(_) => {}
                Op::Pop => {
                    pop!();
                }
                Op::Dup => {
                    let t = *s.stack.last()?;
                    s.stack.push(t);
                }
                Op::PushInt(_) => s.stack.push(Ty::Int),
                Op::PushTrue | Op::PushFalse => s.stack.push(Ty::Bool),
                Op::PushConst(c) => s.stack.push(const_ty(cx.module.consts.get(*c as usize)?)?),
                Op::LoadSlot(sl) | Op::LoadSlotQuiet(sl) => {
                    let t = load!(*sl);
                    s.stack.push(t);
                }
                Op::StoreSlot(sl) | Op::BindSlot(sl) => {
                    let t = pop!();
                    store!(*sl, t);
                }
                Op::Bin(b) => {
                    let y = pop!();
                    let x = pop!();
                    s.stack.push(num_result(*b, x, y).ok()?);
                }
                Op::BinSS { a, b, op: bop } => {
                    let (x, y) = (load!(*a), load!(*b));
                    s.stack.push(num_result(*bop, x, y).ok()?);
                }
                Op::BinSI { a, op: bop, .. } => {
                    let x = load!(*a);
                    s.stack.push(num_result(*bop, x, Ty::Int).ok()?);
                }
                Op::BinSC { a, c, op: bop } => {
                    let x = load!(*a);
                    let y = const_ty(cx.module.consts.get(*c as usize)?)?;
                    s.stack.push(num_result(*bop, x, y).ok()?);
                }
                Op::BinTS { s: sl, op: bop } => {
                    let y = load!(*sl);
                    let x = pop!();
                    s.stack.push(num_result(*bop, x, y).ok()?);
                }
                Op::BinTI { op: bop, .. } => {
                    let x = pop!();
                    s.stack.push(num_result(*bop, x, Ty::Int).ok()?);
                }
                Op::BinTC { c, op: bop } => {
                    let x = pop!();
                    let y = const_ty(cx.module.consts.get(*c as usize)?)?;
                    s.stack.push(num_result(*bop, x, y).ok()?);
                }
                Op::JumpUnlessSS { a, b, op: bop, target } => {
                    let (x, y) = (load!(*a), load!(*b));
                    if num_result(*bop, x, y).ok()? != Ty::Bool {
                        return None;
                    }
                    succ.push(*target as usize);
                }
                Op::JumpUnlessSI { a, op: bop, target, .. } => {
                    let x = load!(*a);
                    if num_result(*bop, x, Ty::Int).ok()? != Ty::Bool {
                        return None;
                    }
                    succ.push(*target as usize);
                }
                Op::Jump(l) => {
                    succ.push(*l as usize);
                    falls = false;
                }
                // JIT2a: a compiled whole function is only entered with every
                // argument (the VM checks the count), so the default-value
                // prologue is always skipped
                Op::JumpIfArgPresent { target, .. } if whole => {
                    succ.push(*target as usize);
                    falls = false;
                }
                Op::JumpIfFalse(l) | Op::JumpIfTrue(l) => {
                    if !pop!().scalar() {
                        return None;
                    }
                    succ.push(*l as usize);
                }
                Op::ToBool => {
                    if !pop!().scalar() {
                        return None;
                    }
                    s.stack.push(Ty::Bool);
                }
                Op::Un(u) => {
                    let x = pop!();
                    if !x.scalar() {
                        return None;
                    }
                    s.stack.push(match u {
                        UnOp::Neg | UnOp::Plus if x.numeric() => x,
                        UnOp::Not => Ty::Bool,
                        UnOp::CastFloat if x.numeric() => Ty::Float,
                        UnOp::CastInt if x == Ty::Int => Ty::Int,
                        UnOp::CastBool => Ty::Bool,
                        _ => return None,
                    });
                }
                Op::IncSlot { slot, mode } | Op::IncSlotDiscard { slot, mode } => {
                    let t = load!(*slot);
                    if !t.numeric() || matches!(mode, WriteMode::Assign | WriteMode::Compound(_)) {
                        return None;
                    }
                    store!(*slot, t);
                    if matches!(op, Op::IncSlot { .. }) {
                        s.stack.push(t);
                    }
                }
                Op::CompoundSlot { slot, op: bop, discard } => {
                    let y = pop!();
                    let x = load!(*slot);
                    let r = num_result(*bop, x, y).ok()?;
                    if r == Ty::Bool {
                        return None;
                    }
                    store!(*slot, r);
                    if !*discard {
                        s.stack.push(slot_ty[*slot as usize]?);
                    }
                }
                Op::Return => {
                    let t = pop!();
                    if let Some(sig) = &sig {
                        let ok = match (sig.ret, t) {
                            (Ty::Int, Ty::Int | Ty::Num) => true,
                            (Ty::Float, Ty::Int | Ty::Float | Ty::Num) => true,
                            (Ty::Bool, Ty::Bool) => true,
                            _ => false,
                        };
                        if !ok {
                            return None;
                        }
                    } else {
                        s.stack.push(t); // region: the interpreter returns
                    }
                    falls = false;
                }
                // JIT2b: `$a[$i]` with an int key on an array; the element type
                // is the one kind the interpreter saw here, any other value
                // leaves compiled code at this op
                Op::FetchDimSS { a, k } => {
                    let (x, key) = (load!(*a), load!(*k));
                    if x != Ty::Arr || key != Ty::Int {
                        return None;
                    }
                    let t = Ty::from_kinds(*cx.types.get(ip)?)?;
                    dim_ty.insert(ip, t);
                    s.stack.push(t);
                }
                Op::FetchDimS(k) => {
                    let key = load!(*k);
                    if pop!() != Ty::Arr || key != Ty::Int {
                        return None;
                    }
                    let t = Ty::from_kinds(*cx.types.get(ip)?)?;
                    dim_ty.insert(ip, t);
                    s.stack.push(t);
                }
                Op::FetchDim => {
                    let key = pop!();
                    if pop!() != Ty::Arr || key != Ty::Int {
                        return None;
                    }
                    let t = Ty::from_kinds(*cx.types.get(ip)?)?;
                    dim_ty.insert(ip, t);
                    s.stack.push(t);
                }
                Op::CallFuncId { id, spec } => {
                    let aspec = cx.unit.arg_specs.get(*spec as usize)?;
                    if !aspec.is_simple() {
                        return None;
                    }
                    let n = aspec.count();
                    if s.stack.len() < n {
                        return None;
                    }
                    let args: Vec<Ty> = s.stack.split_off(s.stack.len() - n);
                    let (kind, ret) = match cx.functions.get(id.0 as usize)? {
                        Callee::Native(name) => match (name.as_str(), args.as_slice()) {
                            ("intdiv", [Ty::Int, Ty::Int]) => (CallKind::IntDiv, Ty::Int),
                            ("max", [Ty::Int, Ty::Int]) => (CallKind::Max, Ty::Int),
                            ("max", [Ty::Float, Ty::Float]) => (CallKind::Max, Ty::Float),
                            ("min", [Ty::Int, Ty::Int]) => (CallKind::Min, Ty::Int),
                            ("min", [Ty::Float, Ty::Float]) => (CallKind::Min, Ty::Float),
                            ("abs", [Ty::Int]) => (CallKind::Abs, Ty::Int),
                            ("abs", [Ty::Float]) => (CallKind::Abs, Ty::Float),
                            ("count", [Ty::Arr]) => (CallKind::Count, Ty::Int),
                            _ => return None,
                        },
                        Callee::User { module, unit } => {
                            let sig = (cx.sig_of)(*module, *unit)?;
                            // omitted arguments need a constant default (filled in by the caller)
                            if n > sig.params.len() || sig.defaults[n..].iter().any(Option::is_none) {
                                return None;
                            }
                            for (p, a) in sig.params.iter().zip(&args) {
                                let ok = match (p, a) {
                                    (Ty::Int, Ty::Int) | (Ty::Bool, Ty::Bool) | (Ty::Float, Ty::Float | Ty::Int) | (Ty::Arr, Ty::Arr) => true,
                                    _ => false,
                                };
                                if !ok {
                                    return None;
                                }
                            }
                            let ret = sig.ret;
                            (CallKind::User { module: *module, unit: *unit, sig }, ret)
                        }
                    };
                    calls.insert(ip, kind);
                    s.stack.push(ret);
                }
                _ => return None,
            }
            max_stack = max_stack.max(s.stack.len());
            if falls {
                succ.push(ip + 1);
            }
            for t in succ {
                if t < lo || t > hi {
                    if whole {
                        return None;
                    }
                    continue; // region exit
                }
                match &mut states[t] {
                    slot @ None => {
                        *slot = Some(s.clone());
                        work.push(t);
                    }
                    Some(old) => {
                        if old.stack.len() != s.stack.len() {
                            return None;
                        }
                        let mut changed = false;
                        for (a, b) in old.stack.iter_mut().zip(&s.stack) {
                            let j = a.join(*b)?;
                            if j != *a {
                                *a = j;
                                changed = true;
                            }
                        }
                        for (a, b) in old.def.iter_mut().zip(&s.def) {
                            let j = a.join(*b);
                            if j != *a {
                                *a = j;
                                changed = true;
                            }
                        }
                        if changed {
                            work.push(t);
                        }
                    }
                }
            }
        }
        if slot_ty == before {
            let mut needs_flag = vec![false; n_slots];
            for st in states.iter().flatten() {
                for (i, d) in st.def.iter().enumerate() {
                    if *d != Def::Yes && slot_ty[i].is_some() {
                        needs_flag[i] = true;
                    }
                }
            }
            for i in 0..n_slots {
                if slot_ty[i].is_some() && entry_def[i] != Def::Yes {
                    needs_flag[i] = true;
                }
            }
            return Some(Analysis { lo, hi, entry, states, slot_ty, needs_flag, max_stack, calls, dim_ty, sig });
        }
    }
    None
}
