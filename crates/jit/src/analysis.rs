//! Type analysis of a whole function or a loop region before code
//! generation. Slots get one type for the whole region (int, float, bool,
//! or "num" = int or float at run time); whether a slot is defined is
//! tracked per op; the operand stack is typed per op. Anything outside the
//! supported subset makes the region uncompilable.

use std::collections::HashMap;

use grd_vm::bytecode::{BinOp, ClassRef, CodeUnit, Const, Init, Module, Op, PathBase, PathSeg, TypeHint, UnOp, WriteMode};
use grd_vm::vm::jit::{Callee, JitClasses, JitTy, KIND_ARRAY, KIND_BOOL, KIND_FLOAT, KIND_INT, TAG_ARRAY, TAG_BOOL, TAG_FLOAT, TAG_INT, TAG_OBJECT, TAG_OTHER, TAG_UNSET};

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
    /// JITO1: an object of this class or a subclass (its handle), never null;
    /// only in whole functions.
    Obj(u32),
}

impl Ty {
    pub fn numeric(self) -> bool {
        !matches!(self, Ty::Bool | Ty::Arr | Ty::Obj(_))
    }
    pub fn scalar(self) -> bool {
        !matches!(self, Ty::Arr | Ty::Obj(_))
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
            Ty::Obj(_) => TAG_OBJECT,
        }
    }
    pub fn from_jit(t: JitTy) -> Ty {
        match t {
            JitTy::Int => Ty::Int,
            JitTy::Float => Ty::Float,
            JitTy::Bool => Ty::Bool,
            JitTy::Obj(c) => Ty::Obj(c),
        }
    }
    /// A declared type, classes included (JITO1).
    pub fn from_decl(h: &TypeHint, module: &Module, classes: &JitClasses) -> Option<Ty> {
        match h {
            TypeHint::Class(name) => classes.lookup(module.str(*name)).map(Ty::Obj),
            h => Ty::from_hint(h),
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
    /// JITO1: an effect the interpreter could see (an object allocated, a
    /// property written) may have happened before this op. A whole function
    /// is rerun by the interpreter when it leaves, so after an effect no op
    /// may leave (JITD2 §8a V3).
    pub effect: bool,
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
    /// JITO1: property reads (slot, type), writes (slot) and `new` (class,
    /// (type, slot) per promoted parameter), by ip.
    pub props: HashMap<usize, (u32, Ty)>,
    pub stores: HashMap<usize, u32>,
    pub news: HashMap<usize, (u32, Vec<(Ty, u32)>)>,
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
    /// JITO1: class of `$this` (methods), the class table, and whether a
    /// user function has effects (allocates or writes properties).
    pub this_class: Option<u32>,
    pub classes: &'a JitClasses,
    pub effects_of: &'a dyn Fn(u32, u32) -> bool,
}

/// Signature from declared types: all parameters int/float/bool, a declared
/// int/float/bool return type, no variadics/generators. Parameters with a
/// default are allowed (JIT2a): compiled code is only called with all
/// arguments, other calls stay in the interpreter.
pub fn declared_sig(module: &Module, unit: &CodeUnit, classes: &JitClasses) -> Option<Sig> {
    if unit.is_generator || unit.params.iter().any(|p| p.variadic || p.promote.is_some()) {
        return None;
    }
    let params = unit.params.iter().map(|p| p.ty.as_ref().and_then(|h| Ty::from_decl(h, module, classes))).collect::<Option<Vec<_>>>()?;
    // arrays only as parameters: compiled code returns scalars and objects
    let ret = unit.return_type.as_ref().and_then(|h| Ty::from_decl(h, module, classes)).filter(|t| *t != Ty::Arr)?;
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

/// Whether a binary op leaves compiled code on some input (a guard).
fn bin_exits(op: BinOp, a: Ty, b: Ty) -> bool {
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul => a != Ty::Float && b != Ty::Float,
        BinOp::Div | BinOp::Mod => true,
        _ => false,
    }
}

/// A value of type `v` may be stored where `want` is declared without a
/// conversion (JITO1: property writes, which leave the value on the stack).
fn same_or_sub(classes: &JitClasses, want: Ty, v: Ty) -> bool {
    match (want, v) {
        (Ty::Obj(k), Ty::Obj(d)) => classes.is_sub(d, k),
        _ => want == v && v != Ty::Num,
    }
}

/// A value of type `v` may be passed for a parameter of type `want`
/// (int is widened to float).
fn fits_param(classes: &JitClasses, want: Ty, v: Ty) -> bool {
    match (want, v) {
        (Ty::Float, Ty::Int | Ty::Num) => true,
        _ => same_or_sub(classes, want, v),
    }
}

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
    let sig = if whole { Some(declared_sig(cx.module, cx.unit, cx.classes)?) } else { None };
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
        states[entry] = Some(State { def: entry_def.clone(), stack: Vec::new(), effect: false });
        let mut work = vec![entry];
        let mut calls = HashMap::new();
        let mut dim_ty = HashMap::new();
        let (mut props, mut stores, mut news) = (HashMap::new(), HashMap::new(), HashMap::new());
        let mut max_stack = 0usize;
        // slots the region touches: they must not be "other" at entry
        while let Some(ip) = work.pop() {
            let st = states[ip].clone().expect("queued state");
            max_stack = max_stack.max(st.stack.len());
            let op = &cx.unit.ops[ip];
            let mut s = st.clone();
            let mut succ: Vec<usize> = Vec::new();
            let mut falls = true;
            // this op may leave compiled code / has a visible effect (JITO1)
            let (mut exits, mut effect) = (false, false);
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
                    // arrays are read only: a slot keeps the array it had at entry;
                    // objects only in whole functions (loop exits carry no handles)
                    if t == Ty::Arr || (!whole && matches!(t, Ty::Obj(_))) {
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
                    exits = bin_exits(*b, x, y);
                }
                Op::BinSS { a, b, op: bop } => {
                    let (x, y) = (load!(*a), load!(*b));
                    s.stack.push(num_result(*bop, x, y).ok()?);
                    exits = bin_exits(*bop, x, y);
                }
                Op::BinSI { a, op: bop, .. } => {
                    let x = load!(*a);
                    s.stack.push(num_result(*bop, x, Ty::Int).ok()?);
                    exits = bin_exits(*bop, x, Ty::Int);
                }
                Op::BinSC { a, c, op: bop } => {
                    let x = load!(*a);
                    let y = const_ty(cx.module.consts.get(*c as usize)?)?;
                    s.stack.push(num_result(*bop, x, y).ok()?);
                    exits = bin_exits(*bop, x, y);
                }
                Op::BinTS { s: sl, op: bop } => {
                    let y = load!(*sl);
                    let x = pop!();
                    s.stack.push(num_result(*bop, x, y).ok()?);
                    exits = bin_exits(*bop, x, y);
                }
                Op::BinTI { op: bop, .. } => {
                    let x = pop!();
                    s.stack.push(num_result(*bop, x, Ty::Int).ok()?);
                    exits = bin_exits(*bop, x, Ty::Int);
                }
                Op::BinTC { c, op: bop } => {
                    let x = pop!();
                    let y = const_ty(cx.module.consts.get(*c as usize)?)?;
                    s.stack.push(num_result(*bop, x, y).ok()?);
                    exits = bin_exits(*bop, x, y);
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
                    exits = matches!(u, UnOp::Neg) && matches!(x, Ty::Int | Ty::Num);
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
                    exits = t != Ty::Float;
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
                    exits = bin_exits(*bop, x, y);
                    store!(*slot, r);
                    if !*discard {
                        s.stack.push(slot_ty[*slot as usize]?);
                    }
                }
                Op::Return => {
                    let t = pop!();
                    if let Some(sig) = &sig {
                        exits = sig.ret == Ty::Int && t == Ty::Num;
                        let ok = match (sig.ret, t) {
                            (Ty::Int, Ty::Int | Ty::Num) => true,
                            (Ty::Float, Ty::Int | Ty::Float | Ty::Num) => true,
                            (Ty::Bool, Ty::Bool) => true,
                            (Ty::Obj(c), Ty::Obj(d)) => cx.classes.is_sub(d, c),
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
                    exits = true;
                }
                Op::FetchDimS(k) => {
                    let key = load!(*k);
                    if pop!() != Ty::Arr || key != Ty::Int {
                        return None;
                    }
                    let t = Ty::from_kinds(*cx.types.get(ip)?)?;
                    dim_ty.insert(ip, t);
                    s.stack.push(t);
                    exits = true;
                }
                Op::FetchDim => {
                    let key = pop!();
                    if pop!() != Ty::Arr || key != Ty::Int {
                        return None;
                    }
                    let t = Ty::from_kinds(*cx.types.get(ip)?)?;
                    dim_ty.insert(ip, t);
                    s.stack.push(t);
                    exits = true;
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
                            ("intdiv", [Ty::Int, Ty::Int]) => {
                                exits = true;
                                (CallKind::IntDiv, Ty::Int)
                            }
                            ("max", [Ty::Int, Ty::Int]) => (CallKind::Max, Ty::Int),
                            ("max", [Ty::Float, Ty::Float]) => (CallKind::Max, Ty::Float),
                            ("min", [Ty::Int, Ty::Int]) => (CallKind::Min, Ty::Int),
                            ("min", [Ty::Float, Ty::Float]) => (CallKind::Min, Ty::Float),
                            ("abs", [Ty::Int]) => {
                                exits = true;
                                (CallKind::Abs, Ty::Int)
                            }
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
                                    (Ty::Obj(c), Ty::Obj(d)) => cx.classes.is_sub(*d, *c),
                                    _ => false,
                                };
                                if !ok {
                                    return None;
                                }
                            }
                            let ret = sig.ret;
                            if !whole && matches!(ret, Ty::Obj(_)) {
                                return None;
                            }
                            // the callee may leave (the caller then leaves too) and
                            // may have effects of its own
                            exits = true;
                            effect = (cx.effects_of)(*module, *unit);
                            (CallKind::User { module: *module, unit: *unit, sig }, ret)
                        }
                    };
                    calls.insert(ip, kind);
                    s.stack.push(ret);
                }
                // JITO1: `$this` in a method
                Op::LoadThis if whole => s.stack.push(Ty::Obj(cx.this_class?)),
                // JITO1: a typed property of an object of known class; an unset
                // slot leaves compiled code before the op
                Op::FetchPropSel(sel) if whole => {
                    let Ty::Obj(c) = pop!() else { return None };
                    let p = cx.classes.prop(c, sel.0)?;
                    if !p.public && !(cx.this_class == Some(c) && cx.classes.get(c)?.is_final) {
                        return None;
                    }
                    let t = Ty::from_jit(p.ty);
                    props.insert(ip, (p.slot, t));
                    s.stack.push(t);
                    exits = true;
                }
                // JITO1: `$this->p = v` with a value of exactly the declared type
                Op::StorePath(path) if whole && path.base == PathBase::This && path.mode == WriteMode::Assign => {
                    let [PathSeg::PropSel(sel)] = path.segs.as_slice() else { return None };
                    let c = cx.this_class?;
                    let p = cx.classes.prop(c, sel.0)?;
                    if p.readonly || (!p.public && !cx.classes.get(c)?.is_final) {
                        return None;
                    }
                    let v = *s.stack.last()?;
                    if !same_or_sub(cx.classes, Ty::from_jit(p.ty), v) {
                        return None;
                    }
                    stores.insert(ip, p.slot);
                    effect = true;
                }
                // JITO1: `new C(...)` when C's constructor only promotes its
                // parameters; the object limit leaves compiled code before the op
                Op::New { class: ClassRef::Id(cid), spec } if whole => {
                    let aspec = cx.unit.arg_specs.get(*spec as usize)?;
                    if !aspec.is_simple() {
                        return None;
                    }
                    let n = aspec.count();
                    if s.stack.len() < n {
                        return None;
                    }
                    let args: Vec<Ty> = s.stack.split_off(s.stack.len() - n);
                    let params: Vec<(Ty, u32)> = cx.classes.get(cid.0)?.new.as_ref()?.iter().map(|(t, slot)| (Ty::from_jit(*t), *slot)).collect();
                    if params.len() != n || params.iter().zip(&args).any(|((t, _), a)| !fits_param(cx.classes, *t, *a)) {
                        return None;
                    }
                    news.insert(ip, (cid.0, params));
                    s.stack.push(Ty::Obj(cid.0));
                    exits = true;
                    effect = true;
                }
                _ => return None,
            }
            if whole && st.effect && exits {
                return None;
            }
            s.effect |= effect;
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
                        if s.effect && !old.effect {
                            old.effect = true;
                            changed = true;
                        }
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
            return Some(Analysis { lo, hi, entry, states, slot_ty, needs_flag, max_stack, calls, dim_ty, sig, props, stores, news });
        }
    }
    None
}
