//! Bytecode → CLIF for an analysed function or loop region.
//!
//! Every compiled function has the signature `fn(buf: *mut u64, vm: *mut
//! Vm, depth: i64) -> i64` (layout in `grd_vm::vm::jit`): 0 = exit (resume
//! the interpreter at `buf[0]`), 1 = return (value in `buf[2..4]`), 2 =
//! bailout (nothing to see), 3 = too deep (bailout because native calls
//! nested past `MAX_NATIVE_DEPTH`). `vm` only passes through to the runtime
//! functions; `depth` counts native calls between compiled functions.

use std::collections::{BTreeSet, HashMap};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{types, AbiParam, Block, BlockArg, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, Value};
use cranelift_frontend::{FunctionBuilder, Variable};

use grd_vm::bytecode::{BinOp, CodeUnit, Const, Module, Op, UnOp, WriteMode};
use grd_vm::vm::jit::{KIND_OBJECT, TAG_OBJECT, THIS_WORD};
use grd_vm::vm::jit::{slot_word, HDR, KIND_ARRAY, KIND_BOOL, KIND_FLOAT, KIND_INT, TAG_ARRAY, TAG_BOOL, TAG_FLOAT, TAG_INT, TAG_UNSET};

use crate::analysis::{Analysis, CallKind, Ty};

pub const ST_EXIT: i64 = 0;
pub const ST_RETURN: i64 = 1;
pub const ST_BAILOUT: i64 = 2;
pub const ST_DEEP: i64 = 3;

/// Native calls between compiled functions run on the machine stack (the
/// interpreter's frames are on the heap). Past this depth a whole function
/// reports `ST_DEEP` at entry, before any effect, and the VM runs the deeper
/// levels in the interpreter (JIT_OBJECTS §11b). A level takes a few hundred
/// bytes, so this stays well inside a 2 MB thread stack.
pub const MAX_NATIVE_DEPTH: i64 = 1000;

/// A value in compiled code.
#[derive(Clone, Copy, Debug)]
pub enum V {
    I(Value),
    F(Value),
    B(Value),
    /// kind (i8: 0 int, 1 float), int payload, float payload
    N(Value, Value, Value),
    /// JIT2b: address of a `PhpArray` (i64), read only
    A(Value),
    /// JITO1: object handle (i64)
    O(Value),
}

#[derive(Clone, Copy)]
enum SV {
    I(Variable),
    F(Variable),
    B(Variable),
    N(Variable, Variable, Variable),
    A(Variable),
    O(Variable),
}

/// Runtime functions of the VM callable from compiled code (JIT2b).
#[derive(Clone, Copy)]
pub struct Runtime {
    /// `grd_rt_dim(arr, key, out) -> kind`
    pub dim: cranelift_codegen::ir::FuncRef,
    /// `grd_rt_count(arr) -> int`
    pub count: cranelift_codegen::ir::FuncRef,
    /// JITO1 `grd_rt_prop_get(vm, handle, slot, out) -> kind`
    pub prop_get: cranelift_codegen::ir::FuncRef,
    /// JITO1 `grd_rt_prop_set(vm, handle, slot, tag, bits)`
    pub prop_set: cranelift_codegen::ir::FuncRef,
    /// JITO1 `grd_rt_new(vm, class, args, n) -> handle` (0 = leave)
    pub new: cranelift_codegen::ir::FuncRef,
}

/// Native callee available to calls: Cranelift function reference and the
/// size of its state buffer.
pub struct NativeCallee {
    pub fref: cranelift_codegen::ir::FuncRef,
    pub buf_words: usize,
    pub param_slots: Vec<u32>,
}

pub struct Gen<'a, 'b> {
    pub b: &'a mut FunctionBuilder<'b>,
    pub module: &'a Module,
    pub unit: &'a CodeUnit,
    pub an: &'a Analysis,
    pub whole: bool,
    pub buf: Value,
    vm: Value,
    depth: Value,
    slots: Vec<Option<SV>>,
    flags: Vec<Option<Variable>>,
    blocks: HashMap<usize, Block>,
    pub callees: HashMap<usize, NativeCallee>,
    rt: Runtime,
    force: crate::ForceExit,
}

fn mf() -> MemFlagsData {
    MemFlagsData::trusted()
}

impl<'a, 'b> Gen<'a, 'b> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(b: &'a mut FunctionBuilder<'b>, module: &'a Module, unit: &'a CodeUnit, an: &'a Analysis, whole: bool, callees: HashMap<usize, NativeCallee>, rt: Runtime, force: crate::ForceExit) -> Self {
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let buf = b.block_params(entry)[0];
        let vm = b.block_params(entry)[1];
        let depth = b.block_params(entry)[2];
        let mut g = Gen { b, module, unit, an, whole, buf, vm, depth, slots: Vec::new(), flags: Vec::new(), blocks: HashMap::new(), callees, rt, force };
        g.declare_slots();
        g
    }

    fn declare_slots(&mut self) {
        let n = self.unit.num_slots as usize;
        self.slots = vec![None; n];
        self.flags = vec![None; n];
        for i in 0..n {
            let Some(t) = self.an.slot_ty[i] else { continue };
            let sv = match t {
                Ty::Int => SV::I(self.b.declare_var(types::I64)),
                Ty::Float => SV::F(self.b.declare_var(types::F64)),
                Ty::Bool => SV::B(self.b.declare_var(types::I8)),
                Ty::Num => SV::N(self.b.declare_var(types::I8), self.b.declare_var(types::I64), self.b.declare_var(types::F64)),
                Ty::Arr => SV::A(self.b.declare_var(types::I64)),
                Ty::Obj(_) => SV::O(self.b.declare_var(types::I64)),
            };
            self.slots[i] = Some(sv);
            if self.an.needs_flag[i] {
                self.flags[i] = Some(self.b.declare_var(types::I8));
            }
        }
    }

    // ---- constants and conversions ----------------------------------------

    fn ic(&mut self, v: i64) -> Value {
        self.b.ins().iconst(types::I64, v)
    }
    fn i8c(&mut self, v: i64) -> Value {
        self.b.ins().iconst(types::I8, v)
    }
    fn fc(&mut self, v: f64) -> Value {
        self.b.ins().f64const(v)
    }
    fn to_f(&mut self, v: V) -> Value {
        match v {
            V::F(f) => f,
            V::I(i) => self.b.ins().fcvt_from_sint(types::F64, i),
            V::N(k, i, f) => {
                let fi = self.b.ins().fcvt_from_sint(types::F64, i);
                self.b.ins().select(k, f, fi)
            }
            V::B(_) | V::A(_) | V::O(_) => unreachable!("not a number"),
        }
    }
    /// (kind, int, float) of a numeric value.
    fn parts(&mut self, v: V) -> (Value, Value, Value) {
        match v {
            V::N(k, i, f) => (k, i, f),
            V::I(i) => {
                let k = self.i8c(0);
                let f = self.fc(0.0);
                (k, i, f)
            }
            V::F(f) => {
                let k = self.i8c(1);
                let i = self.ic(0);
                (k, i, f)
            }
            V::B(_) | V::A(_) | V::O(_) => unreachable!("not a number"),
        }
    }
    fn coerce(&mut self, v: V, t: Ty) -> V {
        match (v, t) {
            (V::I(_), Ty::Int) | (V::F(_), Ty::Float) | (V::B(_), Ty::Bool) | (V::N(..), Ty::Num) | (V::A(_), Ty::Arr) | (V::O(_), Ty::Obj(_)) => v,
            (V::I(i), Ty::Float) => V::F(self.b.ins().fcvt_from_sint(types::F64, i)),
            (V::I(_) | V::F(_), Ty::Num) => {
                let (k, i, f) = self.parts(v);
                V::N(k, i, f)
            }
            _ => unreachable!("coerce {v:?} to {t:?}"),
        }
    }
    fn ty_of(v: V) -> Ty {
        match v {
            V::I(_) => Ty::Int,
            V::F(_) => Ty::Float,
            V::B(_) => Ty::Bool,
            V::N(..) => Ty::Num,
            V::A(_) => Ty::Arr,
            // the class does not matter where this is asked (arithmetic)
            V::O(_) => Ty::Obj(u32::MAX),
        }
    }
    fn truthy(&mut self, v: V) -> Value {
        match v {
            V::B(b) => b,
            V::I(i) => self.b.ins().icmp_imm_s(IntCC::NotEqual, i, 0),
            V::F(f) => {
                let z = self.fc(0.0);
                self.b.ins().fcmp(FloatCC::NotEqual, f, z)
            }
            V::N(k, i, f) => {
                let ti = self.b.ins().icmp_imm_s(IntCC::NotEqual, i, 0);
                let z = self.fc(0.0);
                let tf = self.b.ins().fcmp(FloatCC::NotEqual, f, z);
                self.b.ins().select(k, tf, ti)
            }
            V::A(_) | V::O(_) => unreachable!("analysis rejects arrays and objects as conditions"),
        }
    }

    // ---- slots --------------------------------------------------------------

    fn read_slot(&mut self, s: u32) -> V {
        match self.slots[s as usize].expect("analysed slot") {
            SV::I(v) => V::I(self.b.use_var(v)),
            SV::F(v) => V::F(self.b.use_var(v)),
            SV::B(v) => V::B(self.b.use_var(v)),
            SV::N(k, i, f) => V::N(self.b.use_var(k), self.b.use_var(i), self.b.use_var(f)),
            SV::A(v) => V::A(self.b.use_var(v)),
            SV::O(v) => V::O(self.b.use_var(v)),
        }
    }
    fn write_slot(&mut self, s: u32, v: V) {
        let t = self.an.slot_ty[s as usize].expect("analysed slot");
        let v = self.coerce(v, t);
        match (self.slots[s as usize].expect("analysed slot"), v) {
            (SV::I(var), V::I(x)) | (SV::F(var), V::F(x)) | (SV::B(var), V::B(x)) | (SV::A(var), V::A(x)) | (SV::O(var), V::O(x)) => self.b.def_var(var, x),
            (SV::N(kv, iv, fv), V::N(k, i, f)) => {
                self.b.def_var(kv, k);
                self.b.def_var(iv, i);
                self.b.def_var(fv, f);
            }
            _ => unreachable!(),
        }
        if let Some(fl) = self.flags[s as usize] {
            let one = self.i8c(1);
            self.b.def_var(fl, one);
        }
    }

    /// Initial values: slots defined at entry are loaded from the buffer.
    pub fn load_entry(&mut self, entry_tags: &[u64]) {
        for i in 0..self.slots.len() {
            let Some(sv) = self.slots[i] else { continue };
            let defined = if self.whole {
                self.unit.params.iter().any(|p| p.slot as usize == i)
            } else {
                matches!(entry_tags.get(i), Some(&TAG_INT | &TAG_FLOAT | &TAG_BOOL | &TAG_ARRAY))
            };
            let off = (slot_word(i) as i32 + 1) * 8;
            let entry_ty = if self.whole {
                self.an.sig.as_ref().and_then(|s| self.unit.params.iter().position(|p| p.slot as usize == i).map(|k| s.params[k]))
            } else {
                entry_tags.get(i).and_then(|t| Ty::from_tag(*t))
            };
            let v = if defined {
                let bits = self.b.ins().load(types::I64, mf(), self.buf, off);
                match entry_ty.expect("defined slot has a type") {
                    Ty::Int => V::I(bits),
                    Ty::Float => V::F(self.b.ins().bitcast(types::F64, MemFlagsData::new(), bits)),
                    Ty::Bool => V::B(self.b.ins().ireduce(types::I8, bits)),
                    Ty::Arr => V::A(bits),
                    Ty::Obj(_) => V::O(bits),
                    Ty::Num => unreachable!(),
                }
            } else {
                // undefined at entry: placeholder, guarded by the defined-flag
                match sv {
                    SV::I(_) => V::I(self.ic(0)),
                    SV::F(_) => V::F(self.fc(0.0)),
                    SV::B(_) => V::B(self.i8c(0)),
                    SV::A(_) => V::A(self.ic(0)),
                    SV::O(_) => V::O(self.ic(0)),
                    SV::N(..) => {
                        let (k, i0, f0) = (self.i8c(0), self.ic(0), self.fc(0.0));
                        V::N(k, i0, f0)
                    }
                }
            };
            let t = self.an.slot_ty[i].expect("typed");
            let v = self.coerce(v, t);
            match (sv, v) {
                (SV::I(var), V::I(x)) | (SV::F(var), V::F(x)) | (SV::B(var), V::B(x)) | (SV::A(var), V::A(x)) | (SV::O(var), V::O(x)) => self.b.def_var(var, x),
                (SV::N(kv, iv, fv), V::N(k, iq, f)) => {
                    self.b.def_var(kv, k);
                    self.b.def_var(iv, iq);
                    self.b.def_var(fv, f);
                }
                _ => unreachable!(),
            }
            if let Some(fl) = self.flags[i] {
                let d = self.i8c(defined as i64);
                self.b.def_var(fl, d);
            }
        }
    }

    // ---- state materialisation ----------------------------------------------

    fn store_v(&mut self, word: usize, v: V) {
        let (tag, bits) = match v {
            V::I(i) => (self.ic(TAG_INT as i64), i),
            V::F(f) => (self.ic(TAG_FLOAT as i64), self.b.ins().bitcast(types::I64, MemFlagsData::new(), f)),
            V::B(x) => (self.ic(TAG_BOOL as i64), self.b.ins().uextend(types::I64, x)),
            V::N(k, i, f) => {
                let ti = self.ic(TAG_INT as i64);
                let tf = self.ic(TAG_FLOAT as i64);
                let tag = self.b.ins().select(k, tf, ti);
                let fb = self.b.ins().bitcast(types::I64, MemFlagsData::new(), f);
                (tag, self.b.ins().select(k, fb, i))
            }
            V::A(p) => (self.ic(TAG_ARRAY as i64), p),
            V::O(h) => (self.ic(TAG_OBJECT as i64), h),
        };
        self.b.ins().store(mf(), tag, self.buf, (word * 8) as i32);
        self.b.ins().store(mf(), bits, self.buf, (word * 8 + 8) as i32);
    }

    /// Write slots, stack and ip, return "exit" (loop regions only).
    fn emit_exit(&mut self, ip: usize, stack: &[V]) {
        for i in 0..self.slots.len() {
            if self.slots[i].is_none() {
                continue;
            }
            let v = self.read_slot(i as u32);
            self.store_v(slot_word(i), v);
            if let Some(fl) = self.flags[i] {
                // undefined: leave the slot alone (tag unset)
                let f = self.b.use_var(fl);
                let w = slot_word(i) * 8;
                let cur = self.b.ins().load(types::I64, mf(), self.buf, w as i32);
                let unset = self.ic(TAG_UNSET as i64);
                let tag = self.b.ins().select(f, cur, unset);
                self.b.ins().store(mf(), tag, self.buf, w as i32);
            }
        }
        let base = slot_word(self.unit.num_slots as usize);
        for (d, v) in stack.iter().enumerate() {
            self.store_v(base + 2 * d, *v);
        }
        let ipv = self.ic(ip as i64);
        self.b.ins().store(mf(), ipv, self.buf, 0);
        let depth = self.ic(stack.len() as i64);
        self.b.ins().store(mf(), depth, self.buf, 8);
        let st = self.ic(ST_EXIT);
        self.b.ins().return_(&[st]);
    }

    fn emit_bailout(&mut self) {
        let st = self.ic(ST_BAILOUT);
        self.b.ins().return_(&[st]);
    }

    /// Branch to a failure path when `cond` holds: exit with the state before
    /// the op (regions) or bail out (functions). Every exit point goes
    /// through here, so the forced-exit test mode only needs this place.
    fn guard(&mut self, cond: Value, ip: usize, pre_stack: &[V]) {
        let cond = if self.force.forces(&self.unit.name, ip) { self.i8c(1) } else { cond };
        self.exit_if(cond, ip, pre_stack);
    }

    /// The forced exit of an op whose runtime call or callee may have an
    /// effect: taken before the call, because the guard after it only fires
    /// when nothing happened (check-then-act, JITD2 §8f).
    fn force_before_call(&mut self, ip: usize, pre_stack: &[V]) {
        if self.force.forces(&self.unit.name, ip) {
            let t = self.i8c(1);
            self.exit_if(t, ip, pre_stack);
        }
    }

    /// The guard after such a call: the real condition only.
    fn guard_after_call(&mut self, cond: Value, ip: usize, pre_stack: &[V]) {
        self.exit_if(cond, ip, pre_stack);
    }

    fn exit_if(&mut self, cond: Value, ip: usize, pre_stack: &[V]) {
        let fail = self.b.create_block();
        let cont = self.b.create_block();
        self.b.ins().brif(cond, fail, &[], cont, &[]);
        self.b.switch_to_block(fail);
        if self.whole {
            self.emit_bailout();
        } else {
            self.emit_exit(ip, pre_stack);
        }
        self.b.switch_to_block(cont);
    }

    // ---- control flow -----------------------------------------------------------

    fn block_for(&mut self, ip: usize) -> Block {
        if let Some(b) = self.blocks.get(&ip) {
            return *b;
        }
        let blk = self.b.create_block();
        let an = self.an;
        let st = an.states[ip].as_ref().expect("reachable");
        for t in &st.stack {
            match t {
                Ty::Int => {
                    self.b.append_block_param(blk, types::I64);
                }
                Ty::Float => {
                    self.b.append_block_param(blk, types::F64);
                }
                Ty::Bool => {
                    self.b.append_block_param(blk, types::I8);
                }
                Ty::Num => {
                    self.b.append_block_param(blk, types::I8);
                    self.b.append_block_param(blk, types::I64);
                    self.b.append_block_param(blk, types::F64);
                }
                Ty::Arr | Ty::Obj(_) => {
                    self.b.append_block_param(blk, types::I64);
                }
            }
        }
        self.blocks.insert(ip, blk);
        blk
    }

    fn block_args(&mut self, ip: usize, stack: &[V]) -> Vec<BlockArg> {
        let want = self.an.states[ip].as_ref().expect("reachable").stack.clone();
        let mut out = Vec::new();
        for (v, t) in stack.iter().zip(want) {
            match self.coerce(*v, t) {
                V::I(x) | V::F(x) | V::B(x) | V::A(x) | V::O(x) => out.push(x.into()),
                V::N(k, i, f) => {
                    out.push(k.into());
                    out.push(i.into());
                    out.push(f.into());
                }
            }
        }
        out
    }

    fn params_to_stack(&mut self, ip: usize, blk: Block) -> Vec<V> {
        let params = self.b.block_params(blk).to_vec();
        let st = self.an.states[ip].as_ref().expect("reachable").stack.clone();
        let mut out = Vec::new();
        let mut k = 0;
        for t in st {
            match t {
                Ty::Int => out.push(V::I(params[k])),
                Ty::Float => out.push(V::F(params[k])),
                Ty::Bool => out.push(V::B(params[k])),
                Ty::Arr => out.push(V::A(params[k])),
                Ty::Obj(_) => out.push(V::O(params[k])),
                Ty::Num => {
                    out.push(V::N(params[k], params[k + 1], params[k + 2]));
                    k += 2;
                }
            }
            k += 1;
        }
        out
    }

    fn in_region(&self, ip: usize) -> bool {
        ip >= self.an.lo && ip <= self.an.hi && self.an.states[ip].is_some()
    }

    /// Unconditional transfer to `target` with `stack`.
    fn goto(&mut self, target: usize, stack: &[V]) {
        if self.in_region(target) {
            let args = self.block_args(target, stack);
            let blk = self.block_for(target);
            self.b.ins().jump(blk, &args);
        } else {
            self.emit_exit(target, stack);
        }
    }

    /// `if cond { goto t_true } else { goto t_false }`
    fn branch(&mut self, cond: Value, t_true: usize, t_false: usize, stack: &[V]) {
        let bt = self.b.create_block();
        let bf = self.b.create_block();
        self.b.ins().brif(cond, bt, &[], bf, &[]);
        self.b.switch_to_block(bt);
        self.goto(t_true, stack);
        self.b.switch_to_block(bf);
        self.goto(t_false, stack);
    }

    // ---- arithmetic ---------------------------------------------------------------

    fn checked_int(&mut self, op: BinOp, x: Value, y: Value) -> (Value, Value) {
        let ins = self.b.ins();
        match op {
            BinOp::Add => {
                let r = ins.iadd(x, y);
                let a = self.b.ins().bxor(x, r);
                let c = self.b.ins().bxor(y, r);
                let both = self.b.ins().band(a, c);
                let of = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, both, 0);
                (r, of)
            }
            BinOp::Sub => {
                let r = ins.isub(x, y);
                let a = self.b.ins().bxor(x, y);
                let c = self.b.ins().bxor(x, r);
                let both = self.b.ins().band(a, c);
                let of = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, both, 0);
                (r, of)
            }
            BinOp::Mul => {
                let lo = ins.imul(x, y);
                let hi = self.b.ins().smulhi(x, y);
                let sign = self.b.ins().sshr_imm_s(lo, 63);
                let of = self.b.ins().icmp(IntCC::NotEqual, hi, sign);
                (lo, of)
            }
            _ => unreachable!(),
        }
    }

    fn fbin(&mut self, op: BinOp, x: Value, y: Value) -> Value {
        match op {
            BinOp::Add => self.b.ins().fadd(x, y),
            BinOp::Sub => self.b.ins().fsub(x, y),
            BinOp::Mul => self.b.ins().fmul(x, y),
            BinOp::Div => self.b.ins().fdiv(x, y),
            _ => unreachable!(),
        }
    }

    fn icmp_op(op: BinOp) -> IntCC {
        match op {
            BinOp::Lt => IntCC::SignedLessThan,
            BinOp::Le => IntCC::SignedLessThanOrEqual,
            BinOp::Gt => IntCC::SignedGreaterThan,
            BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
            BinOp::Identical => IntCC::Equal,
            BinOp::NotIdentical => IntCC::NotEqual,
            _ => unreachable!(),
        }
    }
    fn fcmp_op(op: BinOp) -> FloatCC {
        match op {
            BinOp::Lt => FloatCC::LessThan,
            BinOp::Le => FloatCC::LessThanOrEqual,
            BinOp::Gt => FloatCC::GreaterThan,
            BinOp::Ge => FloatCC::GreaterThanOrEqual,
            BinOp::Identical => FloatCC::Equal,
            BinOp::NotIdentical => FloatCC::NotEqual,
            _ => unreachable!(),
        }
    }

    /// Binary op with PHP semantics; guards jump out before any effect.
    fn bin(&mut self, op: BinOp, x: V, y: V, ip: usize, pre: &[V]) -> V {
        use BinOp::*;
        let (tx, ty) = (Self::ty_of(x), Self::ty_of(y));
        match op {
            Add | Sub | Mul => {
                if tx == Ty::Int && ty == Ty::Int {
                    let (V::I(a), V::I(c)) = (x, y) else { unreachable!() };
                    let (r, of) = self.checked_int(op, a, c);
                    self.guard(of, ip, pre);
                    V::I(r)
                } else if tx == Ty::Float || ty == Ty::Float {
                    let (a, c) = (self.to_f(x), self.to_f(y));
                    V::F(self.fbin(op, a, c))
                } else {
                    // num with int/num: int path when both are ints, else float
                    let (ka, ia, _) = self.parts(x);
                    let (kb, ib, _) = self.parts(y);
                    let k = self.b.ins().bor(ka, kb);
                    let (ri, of) = self.checked_int(op, ia, ib);
                    let nk = self.b.ins().icmp_imm_s(IntCC::Equal, k, 0);
                    let bad = self.b.ins().band(of, nk);
                    self.guard(bad, ip, pre);
                    let (fa, fb) = (self.to_f(x), self.to_f(y));
                    let rf = self.fbin(op, fa, fb);
                    V::N(k, ri, rf)
                }
            }
            Div => {
                // always float in the dialect (D10); division by zero leaves
                // compiled code so the interpreter raises the error
                let (a, c) = (self.to_f(x), self.to_f(y));
                let zero = self.fc(0.0);
                let z = self.b.ins().fcmp(FloatCC::Equal, c, zero);
                self.guard(z, ip, pre);
                V::F(self.b.ins().fdiv(a, c))
            }
            Mod => {
                let (V::I(a), V::I(c)) = (x, y) else { unreachable!() };
                let z = self.b.ins().icmp_imm_s(IntCC::Equal, c, 0);
                let m1 = self.b.ins().icmp_imm_s(IntCC::Equal, c, -1);
                let bad = self.b.ins().bor(z, m1);
                self.guard(bad, ip, pre);
                V::I(self.b.ins().srem(a, c))
            }
            BitAnd | BitOr | BitXor => {
                let (V::I(a), V::I(c)) = (x, y) else { unreachable!() };
                V::I(match op {
                    BitAnd => self.b.ins().band(a, c),
                    BitOr => self.b.ins().bor(a, c),
                    _ => self.b.ins().bxor(a, c),
                })
            }
            Spaceship => {
                let (V::I(a), V::I(c)) = (x, y) else { unreachable!() };
                let gt = self.b.ins().icmp(IntCC::SignedGreaterThan, a, c);
                let lt = self.b.ins().icmp(IntCC::SignedLessThan, a, c);
                let g = self.b.ins().uextend(types::I64, gt);
                let l = self.b.ins().uextend(types::I64, lt);
                V::I(self.b.ins().isub(g, l))
            }
            Identical | NotIdentical => {
                let eq = match (x, y) {
                    (V::I(a), V::I(c)) => self.b.ins().icmp(IntCC::Equal, a, c),
                    (V::F(a), V::F(c)) => self.b.ins().fcmp(FloatCC::Equal, a, c),
                    (V::B(a), V::B(c)) => self.b.ins().icmp(IntCC::Equal, a, c),
                    (V::I(_), V::F(_)) | (V::F(_), V::I(_)) | (V::B(_), _) | (_, V::B(_)) => self.i8c(0),
                    _ => {
                        let (ka, ia, fa) = self.parts(x);
                        let (kb, ib, fb) = self.parts(y);
                        let same_k = self.b.ins().icmp(IntCC::Equal, ka, kb);
                        let ie = self.b.ins().icmp(IntCC::Equal, ia, ib);
                        let fe = self.b.ins().fcmp(FloatCC::Equal, fa, fb);
                        let ve = self.b.ins().select(ka, fe, ie);
                        self.b.ins().band(same_k, ve)
                    }
                };
                if op == NotIdentical {
                    V::B(self.b.ins().bxor_imm_u(eq, 1))
                } else {
                    V::B(eq)
                }
            }
            Lt | Le | Gt | Ge => match (x, y) {
                (V::I(a), V::I(c)) => V::B(self.b.ins().icmp(Self::icmp_op(op), a, c)),
                (V::F(_), _) | (_, V::F(_)) => {
                    let (a, c) = (self.to_f(x), self.to_f(y));
                    V::B(self.b.ins().fcmp(Self::fcmp_op(op), a, c))
                }
                _ => {
                    let (ka, ia, _) = self.parts(x);
                    let (kb, ib, _) = self.parts(y);
                    let anyf = self.b.ins().bor(ka, kb);
                    let ci = self.b.ins().icmp(Self::icmp_op(op), ia, ib);
                    let (a, c) = (self.to_f(x), self.to_f(y));
                    let cf = self.b.ins().fcmp(Self::fcmp_op(op), a, c);
                    V::B(self.b.ins().select(anyf, cf, ci))
                }
            },
            _ => unreachable!("analysis lets only supported ops through"),
        }
    }

    fn const_v(&mut self, c: &Const) -> V {
        match c {
            Const::Int(i) => V::I(self.ic(*i)),
            Const::Float(f) => V::F(self.fc(*f)),
            Const::Bool(b) => V::B(self.i8c(*b as i64)),
            _ => unreachable!(),
        }
    }

    fn inc(&mut self, v: V, up: bool, ip: usize, pre: &[V]) -> V {
        match v {
            V::I(i) => {
                let one = self.ic(1);
                let (r, of) = self.checked_int(if up { BinOp::Add } else { BinOp::Sub }, i, one);
                self.guard(of, ip, pre);
                V::I(r)
            }
            V::F(f) => {
                let one = self.fc(1.0);
                V::F(if up { self.b.ins().fadd(f, one) } else { self.b.ins().fsub(f, one) })
            }
            V::N(..) => {
                let one = V::I(self.ic(1));
                self.bin(if up { BinOp::Add } else { BinOp::Sub }, v, one, ip, pre)
            }
            V::B(_) | V::A(_) | V::O(_) => unreachable!(),
        }
    }

    /// JIT2b: `arr[key]` through the runtime; any element other than the
    /// analysed type (or a missing key) leaves compiled code before the op,
    /// so the interpreter reads it (and warns or throws) itself.
    fn dim(&mut self, arr: V, key: V, ip: usize, pre: &[V]) -> V {
        let (V::A(a), V::I(k)) = (arr, key) else { unreachable!("analysed dim fetch") };
        let want = self.an.dim_ty[&ip];
        let ss = self.b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let out = self.b.ins().stack_addr(types::I64, ss, 0);
        let call = self.b.ins().call(self.rt.dim, &[a, k, out]);
        let kind = self.b.inst_results(call)[0];
        let expect = match want {
            Ty::Int => KIND_INT,
            Ty::Float => KIND_FLOAT,
            Ty::Bool => KIND_BOOL,
            Ty::Arr => KIND_ARRAY,
            Ty::Num | Ty::Obj(_) => unreachable!(),
        };
        let bad = self.b.ins().icmp_imm_s(IntCC::NotEqual, kind, expect as i64);
        self.guard(bad, ip, pre);
        let bits = self.b.ins().load(types::I64, mf(), out, 0);
        match want {
            Ty::Int => V::I(bits),
            Ty::Float => V::F(self.b.ins().bitcast(types::F64, MemFlagsData::new(), bits)),
            Ty::Bool => V::B(self.b.ins().ireduce(types::I8, bits)),
            Ty::Arr => V::A(bits),
            Ty::Num | Ty::Obj(_) => unreachable!(),
        }
    }

    /// (tag, bits) of a value as the state buffer and the runtime hold it.
    fn tag_bits(&mut self, v: V) -> (Value, Value) {
        match v {
            V::I(i) => (self.ic(TAG_INT as i64), i),
            V::F(f) => (self.ic(TAG_FLOAT as i64), self.b.ins().bitcast(types::I64, MemFlagsData::new(), f)),
            V::B(x) => (self.ic(TAG_BOOL as i64), self.b.ins().uextend(types::I64, x)),
            V::A(p) => (self.ic(TAG_ARRAY as i64), p),
            V::O(h) => (self.ic(TAG_OBJECT as i64), h),
            V::N(..) => unreachable!("coerced before"),
        }
    }

    /// JITO1: property read through the runtime; an unset slot (or a value
    /// of another kind) leaves compiled code before the op.
    fn prop_get(&mut self, obj: V, slot: u32, want: Ty, ip: usize, pre: &[V]) -> V {
        let V::O(h) = obj else { unreachable!("analysed receiver") };
        let ss = self.b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
        let out = self.b.ins().stack_addr(types::I64, ss, 0);
        let sl = self.ic(slot as i64);
        let call = self.b.ins().call(self.rt.prop_get, &[self.vm, h, sl, out]);
        let kind = self.b.inst_results(call)[0];
        let expect = match want {
            Ty::Int => KIND_INT,
            Ty::Float => KIND_FLOAT,
            Ty::Bool => KIND_BOOL,
            Ty::Obj(_) => KIND_OBJECT,
            Ty::Num | Ty::Arr => unreachable!(),
        };
        let bad = self.b.ins().icmp_imm_s(IntCC::NotEqual, kind, expect as i64);
        self.guard(bad, ip, pre);
        let bits = self.b.ins().load(types::I64, mf(), out, 0);
        match want {
            Ty::Int => V::I(bits),
            Ty::Float => V::F(self.b.ins().bitcast(types::F64, MemFlagsData::new(), bits)),
            Ty::Bool => V::B(self.b.ins().ireduce(types::I8, bits)),
            Ty::Obj(_) => V::O(bits),
            Ty::Num | Ty::Arr => unreachable!(),
        }
    }

    // ---- the ops ------------------------------------------------------------------------

    /// Emit the whole function body: entry jump, then every reachable block.
    pub fn emit(&mut self, entry_tags: &[u64]) {
        if self.whole {
            // too deep: leave before anything happened (not a forced exit)
            let deep = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, self.depth, MAX_NATIVE_DEPTH);
            let out = self.b.create_block();
            let go = self.b.create_block();
            self.b.ins().brif(deep, out, &[], go, &[]);
            self.b.switch_to_block(out);
            let st = self.ic(ST_DEEP);
            self.b.ins().return_(&[st]);
            self.b.switch_to_block(go);
        }
        self.load_entry(entry_tags);
        let entry_ip = self.an.entry;
        self.goto(entry_ip, &[]);
        // leaders: entry and every in-region branch target / fall-through after a branch
        let mut leaders: BTreeSet<usize> = BTreeSet::new();
        leaders.insert(entry_ip);
        for ip in self.an.lo..=self.an.hi {
            if self.an.states[ip].is_none() {
                continue;
            }
            let unit = self.unit;
            let (targets, ends): (Vec<usize>, bool) = match &unit.ops[ip] {
                Op::Jump(l) | Op::JumpIfArgPresent { target: l, .. } => (vec![*l as usize], true),
                Op::JumpIfFalse(l) | Op::JumpIfTrue(l) => (vec![*l as usize, ip + 1], true),
                Op::JumpUnlessSS { target, .. } | Op::JumpUnlessSI { target, .. } => (vec![*target as usize, ip + 1], true),
                Op::Return => (vec![], true),
                _ => (vec![], false),
            };
            for t in targets {
                if self.in_region(t) {
                    leaders.insert(t);
                }
            }
            if ends && self.in_region(ip + 1) {
                leaders.insert(ip + 1);
            }
        }
        let leaders: Vec<usize> = leaders.into_iter().collect();
        for (li, &start) in leaders.iter().enumerate() {
            let blk = self.block_for(start);
            self.b.switch_to_block(blk);
            let mut stack = self.params_to_stack(start, blk);
            let next_leader = leaders.get(li + 1).copied().unwrap_or(usize::MAX);
            let mut ip = start;
            loop {
                if ip != start && ip == next_leader {
                    self.goto(ip, &stack);
                    break;
                }
                if !self.in_region(ip) {
                    self.goto(ip, &stack);
                    break;
                }
                if self.op(ip, &mut stack) {
                    break; // terminator emitted
                }
                ip += 1;
            }
        }
    }

    /// Emit one op; true if it ended the block.
    fn op(&mut self, ip: usize, stack: &mut Vec<V>) -> bool {
        let pre = stack.clone();
        let unit = self.unit;
        let module = self.module;
        match &unit.ops[ip] {
            Op::Nop | Op::Line(_) => {}
            Op::Pop => {
                stack.pop();
            }
            Op::Dup => {
                let v = *stack.last().expect("dup");
                stack.push(v);
            }
            Op::PushInt(i) => {
                let v = self.ic(*i);
                stack.push(V::I(v));
            }
            Op::PushTrue => {
                let v = self.i8c(1);
                stack.push(V::B(v));
            }
            Op::PushFalse => {
                let v = self.i8c(0);
                stack.push(V::B(v));
            }
            Op::PushConst(c) => {
                let v = self.const_v(&module.consts[*c as usize]);
                stack.push(v);
            }
            Op::LoadSlot(s) | Op::LoadSlotQuiet(s) => {
                let v = self.read_slot(*s);
                stack.push(v);
            }
            Op::StoreSlot(s) | Op::BindSlot(s) => {
                let v = stack.pop().expect("store");
                self.write_slot(*s, v);
            }
            Op::Bin(bop) => {
                let y = stack.pop().expect("rhs");
                let x = stack.pop().expect("lhs");
                let r = self.bin(*bop, x, y, ip, &pre);
                stack.push(r);
            }
            Op::BinSS { a, b, op: bop } => {
                let (x, y) = (self.read_slot(*a), self.read_slot(*b));
                let r = self.bin(*bop, x, y, ip, &pre);
                stack.push(r);
            }
            Op::BinSI { a, imm, op: bop } => {
                let x = self.read_slot(*a);
                let y = V::I(self.ic(*imm as i64));
                let r = self.bin(*bop, x, y, ip, &pre);
                stack.push(r);
            }
            Op::BinSC { a, c, op: bop } => {
                let x = self.read_slot(*a);
                let y = self.const_v(&module.consts[*c as usize]);
                let r = self.bin(*bop, x, y, ip, &pre);
                stack.push(r);
            }
            Op::BinTS { s, op: bop } => {
                let x = stack.pop().expect("lhs");
                let y = self.read_slot(*s);
                let r = self.bin(*bop, x, y, ip, &pre);
                stack.push(r);
            }
            Op::BinTI { imm, op: bop } => {
                let x = stack.pop().expect("lhs");
                let y = V::I(self.ic(*imm as i64));
                let r = self.bin(*bop, x, y, ip, &pre);
                stack.push(r);
            }
            Op::BinTC { c, op: bop } => {
                let x = stack.pop().expect("lhs");
                let y = self.const_v(&module.consts[*c as usize]);
                let r = self.bin(*bop, x, y, ip, &pre);
                stack.push(r);
            }
            Op::JumpUnlessSS { a, b, op: bop, target } => {
                let (x, y) = (self.read_slot(*a), self.read_slot(*b));
                let V::B(c) = self.bin(*bop, x, y, ip, &pre) else { unreachable!() };
                self.branch(c, ip + 1, *target as usize, stack);
                return true;
            }
            Op::JumpUnlessSI { a, imm, op: bop, target } => {
                let x = self.read_slot(*a);
                let y = V::I(self.ic(*imm as i64));
                let V::B(c) = self.bin(*bop, x, y, ip, &pre) else { unreachable!() };
                self.branch(c, ip + 1, *target as usize, stack);
                return true;
            }
            Op::Jump(l) | Op::JumpIfArgPresent { target: l, .. } => {
                self.goto(*l as usize, stack);
                return true;
            }
            Op::JumpIfFalse(l) | Op::JumpIfTrue(l) => {
                let v = stack.pop().expect("cond");
                let c = self.truthy(v);
                if matches!(self.unit.ops[ip], Op::JumpIfTrue(_)) {
                    self.branch(c, *l as usize, ip + 1, stack);
                } else {
                    self.branch(c, ip + 1, *l as usize, stack);
                }
                return true;
            }
            Op::ToBool => {
                let v = stack.pop().expect("tobool");
                let c = self.truthy(v);
                stack.push(V::B(c));
            }
            Op::Un(u) => {
                let x = stack.pop().expect("unary");
                let r = match (u, x) {
                    (UnOp::Not, _) => {
                        let c = self.truthy(x);
                        V::B(self.b.ins().bxor_imm_u(c, 1))
                    }
                    (UnOp::CastBool, _) => V::B(self.truthy(x)),
                    (UnOp::Plus, _) | (UnOp::CastInt, V::I(_)) => x,
                    (UnOp::CastFloat, _) => V::F(self.to_f(x)),
                    (UnOp::Neg, V::I(i)) => {
                        let z = self.ic(0);
                        let (r, of) = self.checked_int(BinOp::Sub, z, i);
                        self.guard(of, ip, &pre);
                        V::I(r)
                    }
                    (UnOp::Neg, V::F(f)) => V::F(self.b.ins().fneg(f)),
                    (UnOp::Neg, V::N(..)) => {
                        let z = V::I(self.ic(0));
                        self.bin(BinOp::Sub, z, x, ip, &pre)
                    }
                    _ => unreachable!(),
                };
                stack.push(r);
            }
            Op::IncSlot { slot, mode } | Op::IncSlotDiscard { slot, mode } => {
                let old = self.read_slot(*slot);
                let up = matches!(mode, WriteMode::PreInc | WriteMode::PostInc);
                let new = self.inc(old, up, ip, &pre);
                self.write_slot(*slot, new);
                if matches!(self.unit.ops[ip], Op::IncSlot { .. }) {
                    let t = self.an.slot_ty[*slot as usize].expect("typed");
                    let shown = if matches!(mode, WriteMode::PreInc | WriteMode::PreDec) { new } else { old };
                    let v = self.coerce(shown, t);
                    stack.push(v);
                }
            }
            Op::CompoundSlot { slot, op: bop, discard } => {
                let y = stack.pop().expect("rhs");
                let x = self.read_slot(*slot);
                let r = self.bin(*bop, x, y, ip, &pre);
                self.write_slot(*slot, r);
                if !*discard {
                    let v = self.read_slot(*slot);
                    stack.push(v);
                }
            }
            Op::FetchDimSS { a, k } => {
                let (x, key) = (self.read_slot(*a), self.read_slot(*k));
                let r = self.dim(x, key, ip, &pre);
                stack.push(r);
            }
            Op::FetchDimS(k) => {
                let x = stack.pop().expect("array");
                let key = self.read_slot(*k);
                let r = self.dim(x, key, ip, &pre);
                stack.push(r);
            }
            Op::FetchDim => {
                let key = stack.pop().expect("key");
                let x = stack.pop().expect("array");
                let r = self.dim(x, key, ip, &pre);
                stack.push(r);
            }
            Op::Return => {
                if !self.whole {
                    self.emit_exit(ip, stack);
                    return true;
                }
                let v = stack.pop().expect("return value");
                let ret = self.an.sig.as_ref().expect("whole function").ret;
                let v = match (ret, v) {
                    (Ty::Int, V::N(k, i, _)) => {
                        self.guard(k, ip, &pre);
                        V::I(i)
                    }
                    (Ty::Float, _) => V::F(self.to_f(v)),
                    _ => v,
                };
                self.store_v(2, v);
                let st = self.ic(ST_RETURN);
                self.b.ins().return_(&[st]);
                return true;
            }
            Op::CallFuncId { spec, .. } => {
                let n = self.unit.arg_specs[*spec as usize].count();
                let args: Vec<V> = stack.split_off(stack.len() - n);
                let kind = self.an.calls.get(&ip).expect("analysed call").clone();
                let r = match kind {
                    CallKind::IntDiv => {
                        let (V::I(a), V::I(c)) = (args[0], args[1]) else { unreachable!() };
                        let z = self.b.ins().icmp_imm_s(IntCC::Equal, c, 0);
                        let m1 = self.b.ins().icmp_imm_s(IntCC::Equal, c, -1);
                        let mn = self.b.ins().icmp_imm_s(IntCC::Equal, a, i64::MIN);
                        let ov = self.b.ins().band(m1, mn);
                        let bad = self.b.ins().bor(z, ov);
                        self.guard(bad, ip, &pre);
                        V::I(self.b.ins().sdiv(a, c))
                    }
                    CallKind::Max | CallKind::Min => {
                        let want_max = matches!(kind, CallKind::Max);
                        match (args[0], args[1]) {
                            (V::I(a), V::I(c)) => {
                                // PHP keeps the first argument on ties
                                let take_second = self.b.ins().icmp(if want_max { IntCC::SignedLessThan } else { IntCC::SignedGreaterThan }, a, c);
                                V::I(self.b.ins().select(take_second, c, a))
                            }
                            (V::F(a), V::F(c)) => {
                                let take_second = self.b.ins().fcmp(if want_max { FloatCC::LessThan } else { FloatCC::GreaterThan }, a, c);
                                V::F(self.b.ins().select(take_second, c, a))
                            }
                            _ => unreachable!(),
                        }
                    }
                    CallKind::Count => {
                        let V::A(a) = args[0] else { unreachable!() };
                        let call = self.b.ins().call(self.rt.count, &[a]);
                        V::I(self.b.inst_results(call)[0])
                    }
                    CallKind::Abs => match args[0] {
                        V::I(a) => {
                            let mn = self.b.ins().icmp_imm_s(IntCC::Equal, a, i64::MIN);
                            self.guard(mn, ip, &pre);
                            let neg = self.b.ins().ineg(a);
                            let lt = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, a, 0);
                            V::I(self.b.ins().select(lt, neg, a))
                        }
                        V::F(f) => V::F(self.b.ins().fabs(f)),
                        _ => unreachable!(),
                    },
                    CallKind::User { sig, .. } => {
                        let callee = self.callees.get(&ip).expect("native callee");
                        let (fref, words, pslots) = (callee.fref, callee.buf_words, callee.param_slots.clone());
                        let ss = self.b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (words * 8) as u32, 3));
                        let addr = self.b.ins().stack_addr(types::I64, ss, 0);
                        // omitted arguments: the callee's constant defaults (JIT2a)
                        let mut all: Vec<V> = args.clone();
                        for d in &sig.defaults[args.len()..] {
                            all.push(match d.expect("analysed default") {
                                crate::analysis::Lit::I(i) => V::I(self.ic(i)),
                                crate::analysis::Lit::F(f) => V::F(self.b.ins().f64const(f)),
                                crate::analysis::Lit::B(b) => V::B(self.b.ins().iconst(types::I8, b as i64)),
                            });
                        }
                        for ((a, t), ps) in all.iter().zip(&sig.params).zip(&pslots) {
                            let v = self.coerce(*a, *t);
                            let w = slot_word(*ps as usize);
                            let (tag, bits) = self.tag_bits(v);
                            self.b.ins().store(mf(), tag, addr, (w * 8) as i32);
                            self.b.ins().store(mf(), bits, addr, (w * 8 + 8) as i32);
                        }
                        self.force_before_call(ip, &pre);
                        let d1 = self.b.ins().iadd_imm_s(self.depth, 1);
                        let call = self.b.ins().call(fref, &[addr, self.vm, d1]);
                        let status = self.b.inst_results(call)[0];
                        let failed = self.b.ins().icmp_imm_s(IntCC::NotEqual, status, ST_RETURN);
                        if self.whole {
                            // a whole function passes the callee's status up (bailout
                            // or too deep), so the VM learns why
                            let fail = self.b.create_block();
                            let cont = self.b.create_block();
                            self.b.ins().brif(failed, fail, &[], cont, &[]);
                            self.b.switch_to_block(fail);
                            self.b.ins().return_(&[status]);
                            self.b.switch_to_block(cont);
                        } else {
                            self.guard_after_call(failed, ip, &pre);
                        }
                        let bits = self.b.ins().load(types::I64, mf(), addr, 3 * 8);
                        match sig.ret {
                            Ty::Int => V::I(bits),
                            Ty::Float => V::F(self.b.ins().bitcast(types::F64, MemFlagsData::new(), bits)),
                            Ty::Bool => V::B(self.b.ins().ireduce(types::I8, bits)),
                            Ty::Obj(_) => V::O(bits),
                            Ty::Num | Ty::Arr => unreachable!(),
                        }
                    }
                };
                stack.push(r);
            }
            Op::LoadThis => {
                let h = self.b.ins().load(types::I64, mf(), self.buf, (THIS_WORD * 8) as i32);
                stack.push(V::O(h));
            }
            Op::FetchPropSel(_) => {
                let obj = stack.pop().expect("object");
                let (slot, t) = self.an.props[&ip];
                let r = self.prop_get(obj, slot, t, ip, &pre);
                stack.push(r);
            }
            Op::StorePath(_) => {
                // the value stays on the stack (the assignment's result)
                let v = *stack.last().expect("value");
                let slot = self.an.stores[&ip];
                let h = self.b.ins().load(types::I64, mf(), self.buf, (THIS_WORD * 8) as i32);
                let (tag, bits) = self.tag_bits(v);
                let sl = self.ic(slot as i64);
                self.b.ins().call(self.rt.prop_set, &[self.vm, h, sl, tag, bits]);
            }
            Op::New { spec, .. } => {
                let n = self.unit.arg_specs[*spec as usize].count();
                let args: Vec<V> = stack.split_off(stack.len() - n);
                let (class, params) = self.an.news[&ip].clone();
                // (slot, tag, bits) per parameter, in a stack slot
                let ss = self.b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, (24 * n.max(1)) as u32, 3));
                let addr = self.b.ins().stack_addr(types::I64, ss, 0);
                for (k, (a, (t, slot))) in args.iter().zip(&params).enumerate() {
                    let v = self.coerce(*a, *t);
                    let (tag, bits) = self.tag_bits(v);
                    let sl = self.ic(*slot as i64);
                    self.b.ins().store(mf(), sl, addr, (24 * k) as i32);
                    self.b.ins().store(mf(), tag, addr, (24 * k + 8) as i32);
                    self.b.ins().store(mf(), bits, addr, (24 * k + 16) as i32);
                }
                let c = self.ic(class as i64);
                let nv = self.ic(n as i64);
                self.force_before_call(ip, &pre);
                let call = self.b.ins().call(self.rt.new, &[self.vm, c, addr, nv]);
                let h = self.b.inst_results(call)[0];
                let failed = self.b.ins().icmp_imm_s(IntCC::Equal, h, 0);
                self.guard_after_call(failed, ip, &pre);
                stack.push(V::O(h));
            }
            other => unreachable!("analysis rejected {other:?}"),
        }
        false
    }
}

pub fn signature(module: &impl cranelift_module::Module) -> cranelift_codegen::ir::Signature {
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    sig
}

pub fn buf_words(unit: &CodeUnit, an: &Analysis) -> usize {
    HDR + 2 * unit.num_slots as usize + 2 * (an.max_stack + 1)
}
