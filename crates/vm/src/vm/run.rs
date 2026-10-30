//! The dispatch loop.

use std::rc::Rc;

use super::call::CallOutcome;
use super::{ClosureData, FuncRef, IcEntry, Pending, Vm};
use crate::bytecode::*;
use crate::error::Throw;
use crate::value::*;

/// Nesting limit for calls from Rust back into the VM (see `Vm::run`).
pub(crate) const MAX_NESTED_RUNS: u32 = 1000;

pub(crate) enum Flow {
    Next,
    /// The frame stack changed: reload the current frame.
    Switch,
    /// The frame at `base_depth` returned (or yielded) with this value.
    Done(Val),
}

impl Vm {
    pub fn run_main(&mut self, module_idx: u32) -> Result<Val, Throw> {
        let module = self.modules[module_idx as usize].clone();
        let main = module.main;
        let depth = self.frames.len();
        let target = super::call::CallTarget { module: module_idx, unit: main, this: None, class: None, static_class: None, captures: Vec::new(), return_override: None };
        self.push_frame(target, Vec::new(), Vec::new())?;
        // the script itself holds nothing outside the VM: GC-safe
        self.gc_safe_next = true;
        self.run(depth)
    }

    /// Run until the frame stack drops back to `base_depth`.
    /// Run frames until the stack is back at `base_depth`. Every call from Rust
    /// back into the VM (callbacks, magic methods, `jsonSerialize`, …) nests one
    /// `run`, which uses native stack; past `MAX_NESTED_RUNS` the call throws
    /// Error like PHP's stack limit instead of overflowing the native stack and
    /// aborting the process.
    pub(crate) fn run(&mut self, base_depth: usize) -> Result<Val, Throw> {
        if self.nested_runs >= MAX_NESTED_RUNS {
            return Err(Throw::error("Maximum call stack size reached. Infinite recursion?"));
        }
        self.nested_runs += 1;
        // MEM1b: a run started by a native callback may leave values in Rust
        // locals; collection waits until all such runs are done (gc.rs)
        let safe = std::mem::take(&mut self.gc_safe_next);
        if !safe {
            self.unsafe_runs += 1;
        }
        let r = self.run_frames(base_depth);
        if !safe {
            self.unsafe_runs -= 1;
        }
        self.nested_runs -= 1;
        r
    }

    fn run_frames(&mut self, base_depth: usize) -> Result<Val, Throw> {
        'frames: loop {
            if self.frames.len() <= base_depth {
                return Ok(Val::Null);
            }
            let fidx = self.frames.len() - 1;
            let module = self.frames[fidx].module.clone();
            let midx = self.frames[fidx].module_idx;
            let unit_idx = self.frames[fidx].unit as usize;
            let unit = &module.units[unit_idx];
            let slot_base = self.frames[fidx].slot_base;
            let icx = self.frames[fidx].ic;
            // `ip` lives in a local while the fast path runs; it is written back
            // to the frame before anything else (exec, unwinding) can look at it
            let mut ip = self.frames[fidx].ip;
            loop {
                let op = &unit.ops[ip];
                // PERF1 fast path: the frequent simple ops without a call into
                // `exec`; every case that is not plainly numeric/simple falls
                // through to `exec`, which keeps all semantics and errors
                #[cfg(not(feature = "opstats"))]
                match op {
                    Op::Line(_) => {
                        // old serialized modules only; lines come from the line table
                        ip += 1;
                        continue;
                    }
                    Op::Nop => {
                        ip += 1;
                        continue;
                    }
                    Op::PushInt(i) => {
                        self.stack.push(Val::Int(*i));
                        ip += 1;
                        continue;
                    }
                    Op::PushNull => {
                        self.stack.push(Val::Null);
                        ip += 1;
                        continue;
                    }
                    Op::PushTrue => {
                        self.stack.push(Val::Bool(true));
                        ip += 1;
                        continue;
                    }
                    Op::PushFalse => {
                        self.stack.push(Val::Bool(false));
                        ip += 1;
                        continue;
                    }
                    Op::Pop => {
                        self.stack.pop();
                        ip += 1;
                        continue;
                    }
                    Op::LoadSlot(s) => {
                        if let Some(v) = &self.slot_stack[slot_base + *s as usize] {
                            let v = v.clone();
                            self.stack.push(v);
                            ip += 1;
                            continue;
                        }
                    }
                    Op::StoreSlot(s) => {
                        // GRD021: the value keeps the local's type; anything
                        // else goes to `exec`, which raises the error
                        let i = slot_base + *s as usize;
                        if slot_type_ok(self.slot_stack[i].as_ref(), self.stack.last().expect("store operand"), unit, *s) {
                            let v = self.stack.pop().expect("store operand");
                            self.slot_stack[i] = Some(v);
                            ip += 1;
                            continue;
                        }
                    }
                    // FRAME1: return from a plain frame whose value already has the
                    // declared type; generators and conversions go through `exec`
                    Op::Return if self.frames[fidx].generator.is_none() => {
                        let v = self.stack.last().expect("return value");
                        let fits = match &unit.return_type {
                            None | Some(TypeHint::Mixed) => true,
                            Some(TypeHint::Void) => v.is_null(),
                            Some(TypeHint::Nullable(t)) => v.is_null() || super::path::scalar_type_matches(t, v),
                            Some(t) => super::path::scalar_type_matches(t, v),
                        };
                        if fits {
                            let v = self.stack.pop().expect("return value");
                            let frame = self.frames.pop().expect("frame");
                            super::path::shrink(&mut self.stack, frame.stack_base);
                            super::path::shrink(&mut self.slot_stack, frame.slot_base);
                            let v = frame.return_override.unwrap_or(v);
                            if self.frames.len() <= base_depth {
                                return Ok(v);
                            }
                            self.stack.push(v);
                            continue 'frames;
                        }
                    }
                    // scalar constants and built strings/arrays already in the cache
                    Op::PushConst(c) => {
                        let v = match &module.consts[*c as usize] {
                            Const::Null => Some(Val::Null),
                            Const::Bool(b) => Some(Val::Bool(*b)),
                            Const::Int(n) => Some(Val::Int(*n)),
                            Const::Float(f) => Some(Val::Float(*f)),
                            Const::Str(_) | Const::Array(_) => self.const_cache.get(midx as usize).and_then(|c2| c2.get(*c as usize)).cloned().flatten(),
                        };
                        if let Some(v) = v {
                            self.stack.push(v);
                            ip += 1;
                            continue;
                        }
                    }
                    Op::ToBool => {
                        let v = self.stack.pop().expect("operand").to_bool();
                        self.stack.push(Val::Bool(v));
                        ip += 1;
                        continue;
                    }
                    // promoted constructor parameter through the inline cache filled by `exec`
                    Op::PromoteParam { slot, .. } => {
                        if let Some(this) = self.frames[fidx].this {
                            let e = self.ics[icx][ip];
                            if e.class == Some(self.objects.get(this).class) {
                                let v = self.slot_stack[slot_base + *slot as usize].clone().unwrap_or(Val::Null);
                                self.objects.get_mut(this).slots[e.a as usize] = Some(v);
                                ip += 1;
                                continue;
                            }
                        }
                    }
                    // property read through the inline cache filled by `exec` (FetchPropSel)
                    Op::FetchPropSel(_) | Op::FetchPropSelQuiet(_) => {
                        if let Some(Val::Object(h)) = self.stack.last() {
                            let h = *h;
                            let obj = self.objects.get(h);
                            let e = self.ics[icx][ip];
                            if e.class == Some(obj.class)
                                && let Some(v) = &obj.slots[e.a as usize]
                            {
                                let v = v.clone();
                                *self.stack.last_mut().expect("object") = v;
                                ip += 1;
                                continue;
                            }
                        }
                    }
                    Op::LoadThis => {
                        if let Some(this) = self.frames[fidx].this {
                            self.stack.push(Val::Object(this));
                            ip += 1;
                            continue;
                        }
                    }
                    Op::BindSlot(s) => {
                        let v = self.stack.pop().expect("store operand");
                        self.slot_stack[slot_base + *s as usize] = Some(v);
                        ip += 1;
                        continue;
                    }
                    Op::Jump(l) => {
                        let t = *l as usize;
                        if t <= ip {
                            self.gc_safepoint();
                            let prof = &mut self.profiles[icx];
                            prof.backedges = prof.backedges.saturating_add(1);
                            if prof.backedges >= super::HOT_BACKEDGES {
                                prof.hot = true;
                            }
                            // hot loop: run it as compiled code (JIT1b, OSR)
                            if prof.backedges >= self.jit_hot.1 && !prof.jit_no_loop[t] && self.jit.is_some() {
                                match self.jit_loop(fidx, icx, slot_base, t, ip) {
                                    super::jit::LoopOutcome::Resume(next) => {
                                        ip = next;
                                        continue;
                                    }
                                    super::jit::LoopOutcome::NotEntered => {
                                        if matches!(self.jit_loops.get(&(icx, t)), Some(super::jit::JitSlotState::Failed)) {
                                            self.profiles[icx].jit_no_loop[t] = true;
                                        }
                                    }
                                }
                            }
                        }
                        ip = t;
                        continue;
                    }
                    Op::JumpIfFalse(l) => {
                        let v = self.stack.pop().expect("condition");
                        ip = if v.to_bool() { ip + 1 } else { *l as usize };
                        continue;
                    }
                    Op::JumpIfTrue(l) => {
                        let v = self.stack.pop().expect("condition");
                        ip = if v.to_bool() { *l as usize } else { ip + 1 };
                        continue;
                    }
                    Op::JumpUnlessSI { a, imm, op: bop, target } => {
                        if let Some(Val::Int(x)) = &self.slot_stack[slot_base + *a as usize] {
                            let (x, imm) = (*x, *imm as i64);
                            let cond = match bop {
                                BinOp::Lt => x < imm,
                                BinOp::Le => x <= imm,
                                BinOp::Gt => x > imm,
                                BinOp::Ge => x >= imm,
                                BinOp::Identical => x == imm,
                                BinOp::NotIdentical => x != imm,
                                _ => unreachable!(),
                            };
                            ip = if cond { ip + 1 } else { *target as usize };
                            continue;
                        }
                    }
                    Op::JumpUnlessSS { a, b, op: bop, target } => {
                        if let (Some(x), Some(y)) = (&self.slot_stack[slot_base + *a as usize], &self.slot_stack[slot_base + *b as usize])
                            && let Some(r) = num_fast(*bop, x, y)
                        {
                            ip = if r.to_bool() { ip + 1 } else { *target as usize };
                            continue;
                        }
                    }
                    Op::Bin(bop) => {
                        let n = self.stack.len();
                        if let Some(r) = num_fast(*bop, &self.stack[n - 2], &self.stack[n - 1]) {
                            // both operands are numbers: pop and overwrite instead of
                            // truncate (an out-of-line drop loop)
                            self.stack.pop();
                            self.stack[n - 2] = r;
                            ip += 1;
                            continue;
                        }
                    }
                    Op::BinSI { a, imm, op: bop } => {
                        if let Some(x) = &self.slot_stack[slot_base + *a as usize]
                            && let Some(r) = num_fast(*bop, x, &Val::Int(*imm as i64))
                        {
                            self.stack.push(r);
                            ip += 1;
                            continue;
                        }
                    }
                    // OPT1: stack top with a slot, an immediate or a numeric constant
                    Op::BinTS { s, op: bop } => {
                        if let Some(y) = &self.slot_stack[slot_base + *s as usize]
                            && let Some(r) = num_fast(*bop, self.stack.last().expect("lhs"), y)
                        {
                            *self.stack.last_mut().expect("lhs") = r;
                            ip += 1;
                            continue;
                        }
                    }
                    Op::BinTI { imm, op: bop } => {
                        if let Some(r) = num_fast(*bop, self.stack.last().expect("lhs"), &Val::Int(*imm as i64)) {
                            *self.stack.last_mut().expect("lhs") = r;
                            ip += 1;
                            continue;
                        }
                    }
                    Op::BinTC { c, op: bop } => {
                        let y = match &module.consts[*c as usize] {
                            Const::Int(i) => Some(Val::Int(*i)),
                            Const::Float(f) => Some(Val::Float(*f)),
                            _ => None,
                        };
                        if let Some(y) = y
                            && let Some(r) = num_fast(*bop, self.stack.last().expect("lhs"), &y)
                        {
                            *self.stack.last_mut().expect("lhs") = r;
                            ip += 1;
                            continue;
                        }
                    }
                    Op::BinSS { a, b, op: bop } => {
                        if let (Some(x), Some(y)) = (&self.slot_stack[slot_base + *a as usize], &self.slot_stack[slot_base + *b as usize])
                            && let Some(r) = num_fast(*bop, x, y)
                        {
                            self.stack.push(r);
                            ip += 1;
                            continue;
                        }
                    }
                    Op::IncSlotDiscard { slot, mode } => {
                        if let Some(Val::Int(x)) = &mut self.slot_stack[slot_base + *slot as usize] {
                            let next = match mode {
                                WriteMode::PreInc | WriteMode::PostInc => x.checked_add(1),
                                _ => x.checked_sub(1),
                            };
                            if let Some(v) = next {
                                *x = v;
                                ip += 1;
                                continue;
                            }
                        }
                    }
                    Op::CompoundSlot { slot, op: bop, discard } => {
                        if let (Some(lhs), Some(rhs)) = (&self.slot_stack[slot_base + *slot as usize], self.stack.last())
                            && let Some(r) = num_fast(*bop, lhs, rhs)
                            && std::mem::discriminant(lhs) == std::mem::discriminant(&r)
                        {
                            self.stack.pop();
                            if !*discard {
                                self.stack.push(r.clone());
                            }
                            self.slot_stack[slot_base + *slot as usize] = Some(r);
                            ip += 1;
                            continue;
                        }
                    }
                    Op::FetchDimSS { a, k } => {
                        if let (Some(Val::Array(arr)), Some(key)) = (&self.slot_stack[slot_base + *a as usize], &self.slot_stack[slot_base + *k as usize]) {
                            let found = match key {
                                Val::Int(i) => arr.get(&ArrayKey::Int(*i)),
                                Val::Str(s) => arr.get(&ArrayKey::from_string(s)),
                                _ => None,
                            };
                            if let Some(v) = found {
                                // JIT2b: element kinds seen here type the compiled read (out of line:
                                // inline, the update made the whole dispatch loop dearer, c3 +3 % Ir)
                                note_kind(&mut self.profiles, icx, ip, v);
                                let v = v.clone();
                                self.stack.push(v);
                                ip += 1;
                                continue;
                            }
                        }
                    }
                    Op::FetchDimS(k) => {
                        if let (Some(Val::Array(arr)), Some(key)) = (self.stack.last(), &self.slot_stack[slot_base + *k as usize]) {
                            let found = match key {
                                Val::Int(i) => arr.get(&ArrayKey::Int(*i)),
                                Val::Str(s) => arr.get(&ArrayKey::from_string(s)),
                                _ => None,
                            };
                            if let Some(v) = found {
                                note_kind(&mut self.profiles, icx, ip, v);
                                let v = v.clone();
                                *self.stack.last_mut().expect("container") = v;
                                ip += 1;
                                continue;
                            }
                        }
                    }
                    Op::FetchDim | Op::FetchDimQuiet => {
                        let n = self.stack.len();
                        if let Val::Array(a) = &self.stack[n - 2] {
                            let key = match &self.stack[n - 1] {
                                Val::Int(i) => Some(ArrayKey::Int(*i)),
                                Val::Str(s) => Some(ArrayKey::from_string(s)),
                                _ => None,
                            };
                            if let Some(k) = key
                                && let Some(v) = a.get(&k)
                            {
                                note_kind(&mut self.profiles, icx, ip, v);
                                let v = v.clone();
                                self.stack.pop();
                                self.stack[n - 2] = v;
                                ip += 1;
                                continue;
                            }
                        }
                    }
                    // PROPW1: `$o->p = v` / `$this->p = v` through an inline cache
                    // (class -> slot), like property reads; values that need a
                    // type conversion or check failure take the generic path
                    Op::StorePath(path) if path.mode == WriteMode::Assign && matches!(path.segs.as_slice(), [PathSeg::PropSel(_)]) => {
                        let target = match path.base {
                            PathBase::Slot(s) => match &self.slot_stack[slot_base + s as usize] {
                                Some(Val::Object(h)) => Some(*h),
                                _ => None,
                            },
                            PathBase::This => self.frames[fidx].this,
                            _ => None,
                        };
                        if let (Some(h), [PathSeg::PropSel(sel)]) = (target, path.segs.as_slice()) {
                            let class = self.objects.get(h).class;
                            let e = self.ics[icx][ip];
                            if e.class != Some(class) {
                                if let Some(&slot) = self.class(class).props_by_sel.get(sel) {
                                    let p = &self.class(class).props[slot as usize];
                                    let scope = self.frames[fidx].class;
                                    let visible = match p.visibility {
                                        Visibility::Public => true,
                                        Visibility::Protected => scope.is_some_and(|sc| self.instance_of(sc, p.declaring) || self.instance_of(p.declaring, sc)),
                                        Visibility::Private => scope == Some(p.declaring),
                                    };
                                    if visible && !p.readonly {
                                        self.ics[icx][ip] = IcEntry { class: Some(class), a: slot, b: 0, c: 0, flag: false };
                                    }
                                }
                            }
                            let e = self.ics[icx][ip];
                            if e.class == Some(class) {
                                let value = self.stack.last().expect("value");
                                let value_class = match value {
                                    Val::Object(o) => Some(self.objects.get(*o).class),
                                    _ => None,
                                };
                                // scalars (also null for ?T) directly; `Node $p` / `?Node $p`
                                // with an object by class id (no name comparison)
                                let (mut fits, class_hint) = {
                                    let prop = &self.class(class).props[e.a as usize];
                                    match &prop.ty {
                                        None => (true, None),
                                        Some(t) => {
                                            let (inner, nullable) = match t {
                                                TypeHint::Nullable(i) => (&**i, true),
                                                t => (t, false),
                                            };
                                            let direct = (nullable && value.is_null()) || super::path::scalar_type_matches(inner, value);
                                            let hint = match (inner, value_class) {
                                                (TypeHint::Class(n), Some(_)) if !direct => Some((*n, prop.module)),
                                                _ => None,
                                            };
                                            (direct, hint)
                                        }
                                    }
                                };
                                if let (Some((n, m)), Some(vc)) = (class_hint, value_class) {
                                    let m = self.modules[m as usize].clone();
                                    fits = self.hint_class(&m, n).is_some_and(|c| c == vc || self.instance_of(vc, c));
                                }
                                if fits {
                                    let value = self.stack.last().expect("value").clone();
                                    self.objects.get_mut(h).slots[e.a as usize] = Some(value);
                                    ip += 1;
                                    continue;
                                }
                            }
                        }
                    }
                    Op::StorePath(path) if path.mode == WriteMode::Assign => {
                        // `$a[$k] = v` and `$a[] = v` on a local array
                        if let PathBase::Slot(s) = path.base {
                            let at = slot_base + s as usize;
                            let n = self.stack.len();
                            match path.segs.as_slice() {
                                [PathSeg::Dim] if matches!(self.slot_stack[at], Some(Val::Array(_))) => {
                                    let key = match &self.stack[n - 2] {
                                        Val::Int(i) => Some(ArrayKey::Int(*i)),
                                        Val::Str(s) => Some(ArrayKey::from_string(s)),
                                        _ => None,
                                    };
                                    if let Some(k) = key {
                                        let value = self.stack.pop().expect("value");
                                        self.stack.pop();
                                        if let Some(Val::Array(a)) = &mut self.slot_stack[at] {
                                            Rc::make_mut(a).insert(k, value.clone());
                                        }
                                        self.stack.push(value);
                                        ip += 1;
                                        continue;
                                    }
                                }
                                // `$a[$k][] = v` and `$a[$k1][$k2] = v`: the inner array is
                                // created when missing, as in PHP; anything else that
                                // is not an array takes the generic path
                                [PathSeg::Dim, inner @ (PathSeg::Append | PathSeg::Dim)] if matches!(self.slot_stack[at], Some(Val::Array(_))) => {
                                    let nk = if matches!(inner, PathSeg::Dim) { 2 } else { 1 };
                                    let key_of = |v: &Val| match v {
                                        Val::Int(i) => Some(ArrayKey::Int(*i)),
                                        Val::Str(s) => Some(ArrayKey::from_string(s)),
                                        _ => None,
                                    };
                                    let k1 = key_of(&self.stack[n - 1 - nk]);
                                    let k2 = if nk == 2 { key_of(&self.stack[n - 2]) } else { None };
                                    let inner_ok = match (&self.slot_stack[at], &k1) {
                                        (Some(Val::Array(outer)), Some(k)) => matches!(outer.get(k), None | Some(Val::Array(_))),
                                        _ => false,
                                    };
                                    if let Some(k1) = k1
                                        && inner_ok
                                        && (nk == 1 || k2.is_some())
                                    {
                                        let value = self.stack.pop().expect("value");
                                        self.stack.truncate(n - 1 - nk);
                                        if let Some(Val::Array(outer)) = &mut self.slot_stack[at] {
                                            let outer = Rc::make_mut(outer);
                                            let slot = outer.entry_mut(k1);
                                            if slot.is_null() {
                                                *slot = Val::empty_array();
                                            }
                                            if let Val::Array(innr) = slot {
                                                let innr = Rc::make_mut(innr);
                                                match k2 {
                                                    Some(k2) => innr.insert(k2, value.clone()),
                                                    None => innr.push(value.clone()),
                                                }
                                            }
                                        }
                                        self.stack.push(value);
                                        ip += 1;
                                        continue;
                                    }
                                }
                                [PathSeg::Append] if matches!(self.slot_stack[at], Some(Val::Array(_))) => {
                                    let value = self.stack.pop().expect("value");
                                    if let Some(Val::Array(a)) = &mut self.slot_stack[at] {
                                        Rc::make_mut(a).push(value.clone());
                                    }
                                    self.stack.push(value);
                                    ip += 1;
                                    continue;
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                self.frames[fidx].ip = ip + 1;
                #[cfg(feature = "opstats")]
                let started = std::time::Instant::now();
                let flow = self.exec(op, fidx, midx, &module, unit, base_depth);
                #[cfg(feature = "opstats")]
                super::opstats::record(op, started.elapsed());
                match flow {
                    Ok(Flow::Next) => ip = self.frames[fidx].ip,
                    Ok(Flow::Switch) => continue 'frames,
                    Ok(Flow::Done(v)) => return Ok(v),
                    Err(t) => {
                        let exc = self.materialize(t)?;
                        self.unwind(exc, base_depth)?;
                        continue 'frames;
                    }
                }
            }
        }
    }

    pub(crate) fn resolve_class_ref(&mut self, cr: &ClassRef, module: u32) -> Result<ClassId, Throw> {
        match cr {
            ClassRef::Id(c) => Ok(*c),
            ClassRef::Named(id) => {
                let name = self.str_of(module, *id).to_vec();
                self.lookup_class(&name).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", String::from_utf8_lossy(&name))))
            }
            ClassRef::SelfRef => self.scope_class().ok_or_else(|| Throw::error("Cannot use \"self\" when no class scope is active")),
            ClassRef::Parent => {
                let c = self.scope_class().ok_or_else(|| Throw::error("Cannot use \"parent\" when no class scope is active"))?;
                self.class(c).parent.ok_or_else(|| Throw::error("Cannot use \"parent\" when current class scope has no parent"))
            }
            ClassRef::Static => self.frame().static_class.or(self.scope_class()).ok_or_else(|| Throw::error("Cannot use \"static\" when no class scope is active")),
            ClassRef::Dynamic => {
                let v = self.pop();
                self.class_of_value(&v)
            }
        }
    }

    pub(crate) fn class_of_value(&self, v: &Val) -> Result<ClassId, Throw> {
        match v {
            Val::Object(h) => Ok(self.objects.get(*h).class),
            Val::Str(s) => self.lookup_class(s.as_bytes()).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", s))),
            other => Err(Throw::error(format!("Cannot use value of type {} as class name", other.type_name()))),
        }
    }

    /// `LoadSlot` with its error for an undefined variable.
    fn load_slot_checked(&self, fidx: usize, module: &Rc<Module>, unit: &CodeUnit, s: Slot) -> Result<Val, Throw> {
        match &self.slot_stack[self.frames[fidx].slot_base + s as usize] {
            Some(v) => Ok(v.clone()),
            None => {
                let name = unit.slot_names.get(s as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")))
            }
        }
    }

    fn do_return(&mut self, value: Val, base_depth: usize) -> Flow {
        let frame = self.frames.pop().expect("frame");
        self.stack.truncate(frame.stack_base);
        self.slot_stack.truncate(frame.slot_base);
        if let Some(g) = frame.generator {
            self.generator_finish(g, value.clone());
        }
        let value = frame.return_override.unwrap_or(value);
        if self.frames.len() <= base_depth {
            return Flow::Done(value);
        }
        self.push(value);
        Flow::Switch
    }

    #[allow(clippy::too_many_lines)]
    fn exec(&mut self, op: &Op, fidx: usize, midx: u32, module: &Rc<Module>, unit: &CodeUnit, base_depth: usize) -> Result<Flow, Throw> {
        let ip = self.frames[fidx].ip - 1;
        match op {
            Op::Nop => {}
            Op::Line(_) => {}
            Op::Pop => {
                self.pop();
            }
            Op::Dup => {
                let v = self.stack.last().cloned().expect("dup");
                self.push(v);
            }
            Op::PushConst(c) => {
                let v = self.const_to_val(midx, *c);
                self.push(v);
            }
            Op::PushNull => self.push(Val::Null),
            Op::PushTrue => self.push(Val::Bool(true)),
            Op::PushFalse => self.push(Val::Bool(false)),
            Op::PushInt(i) => self.push(Val::Int(*i)),

            Op::LoadSlot(s) => match &self.slot_stack[self.frames[fidx].slot_base + *s as usize] {
                Some(v) => {
                    let v = v.clone();
                    self.push(v);
                }
                None => {
                    let name = unit.slot_names.get(*s as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                    return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                }
            },
            Op::LoadSlotQuiet(s) => {
                let v = self.slot_stack[self.frames[fidx].slot_base + *s as usize].clone().unwrap_or(Val::Null);
                self.push(v);
            }
            Op::StoreSlot(s) => {
                let v = self.pop();
                let i = self.frames[fidx].slot_base + *s as usize;
                if !slot_type_ok(self.slot_stack[i].as_ref(), &v, unit, *s) {
                    return Err(grd021_error(module, unit, *s, self.slot_stack[i].as_ref(), &v));
                }
                self.slot_stack[i] = Some(v);
            }
            Op::BindSlot(s) => {
                let v = self.pop();
                self.slot_stack[self.frames[fidx].slot_base + *s as usize] = Some(v);
            }
            Op::UnsetSlot(s) => self.slot_stack[self.frames[fidx].slot_base + *s as usize] = None,
            Op::LoadThis => match self.frames[fidx].this {
                Some(h) => self.push(Val::Object(h)),
                None => return Err(Throw::error("Using $this when not in object context")),
            },

            Op::Bin(bop) => {
                let b = self.pop();
                let a = self.pop();
                if let Some(r) = num_fast(*bop, &a, &b) {
                    self.push(r);
                    return Ok(Flow::Next);
                }
                let r = self.binary(*bop, a, b)?;
                self.push(r);
            }
            Op::BinSS { a, b, op: bop } => {
                let x = self.slot_stack[self.frames[fidx].slot_base + *a as usize].clone();
                let y = self.slot_stack[self.frames[fidx].slot_base + *b as usize].clone();
                let (Some(x), Some(y)) = (x, y) else {
                    let name = unit.slot_names.get(if self.slot_stack[self.frames[fidx].slot_base + *a as usize].is_none() { *a } else { *b } as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                    return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                };
                let r = self.fast_bin(*bop, x, y)?;
                self.push(r);
            }
            Op::BinSI { a, imm, op: bop } => {
                let Some(x) = self.slot_stack[self.frames[fidx].slot_base + *a as usize].clone() else {
                    let name = unit.slot_names.get(*a as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                    return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                };
                let r = self.fast_bin(*bop, x, Val::Int(*imm as i64))?;
                self.push(r);
            }
            Op::BinSC { a, c, op: bop } => {
                let Some(x) = self.slot_stack[self.frames[fidx].slot_base + *a as usize].clone() else {
                    let name = unit.slot_names.get(*a as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                    return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                };
                let y = self.const_to_val(midx, *c);
                let r = self.fast_bin(*bop, x, y)?;
                self.push(r);
            }
            Op::BinTS { s, op: bop } => {
                let Some(y) = self.slot_stack[self.frames[fidx].slot_base + *s as usize].clone() else {
                    let name = unit.slot_names.get(*s as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                    return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                };
                let x = self.pop();
                let r = self.fast_bin(*bop, x, y)?;
                self.push(r);
            }
            Op::BinTI { imm, op: bop } => {
                let x = self.pop();
                let r = self.fast_bin(*bop, x, Val::Int(*imm as i64))?;
                self.push(r);
            }
            Op::BinTC { c, op: bop } => {
                let y = self.const_to_val(midx, *c);
                let x = self.pop();
                let r = self.fast_bin(*bop, x, y)?;
                self.push(r);
            }
            Op::JumpUnlessSS { a, b, op: bop, target } => {
                let x = self.slot_stack[self.frames[fidx].slot_base + *a as usize].clone();
                let y = self.slot_stack[self.frames[fidx].slot_base + *b as usize].clone();
                let (Some(x), Some(y)) = (x, y) else {
                    return Err(Throw::new("UndefinedVariableError", "Undefined variable"));
                };
                if !self.fast_bin(*bop, x, y)?.to_bool() {
                    self.frames[fidx].ip = *target as usize;
                }
            }
            Op::JumpUnlessSI { a, imm, op: bop, target } => {
                let cond = match &self.slot_stack[self.frames[fidx].slot_base + *a as usize] {
                    Some(Val::Int(x)) => {
                        let imm = *imm as i64;
                        match bop {
                            BinOp::Lt => *x < imm,
                            BinOp::Le => *x <= imm,
                            BinOp::Gt => *x > imm,
                            BinOp::Ge => *x >= imm,
                            BinOp::Identical => *x == imm,
                            BinOp::NotIdentical => *x != imm,
                            _ => unreachable!(),
                        }
                    }
                    Some(other) => {
                        let v = other.clone();
                        self.fast_bin(*bop, v, Val::Int(*imm as i64))?.to_bool()
                    }
                    None => {
                        let name = unit.slot_names.get(*a as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                        return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                    }
                };
                if !cond {
                    self.frames[fidx].ip = *target as usize;
                }
            }
            Op::Un(uop) => {
                let v = self.pop();
                let r = self.unary(*uop, v)?;
                self.push(r);
            }
            Op::ToBool => {
                let v = self.pop();
                self.push(Val::Bool(v.to_bool()));
            }
            Op::IsNull => {
                let v = self.pop();
                self.push(Val::Bool(v.is_null()));
            }
            Op::IsNotNull => {
                let v = self.pop();
                self.push(Val::Bool(!v.is_null()));
            }

            Op::Jump(l) => {
                if (*l as usize) <= ip {
                    self.gc_safepoint();
                    let prof = &mut self.profiles[self.frames[fidx].ic];
                    prof.backedges = prof.backedges.saturating_add(1);
                    if prof.backedges >= super::HOT_BACKEDGES {
                        prof.hot = true;
                    }
                }
                self.frames[fidx].ip = *l as usize;
            }
            Op::JumpIfFalse(l) => {
                let v = self.pop();
                if !v.to_bool() {
                    self.frames[fidx].ip = *l as usize;
                }
            }
            Op::JumpIfTrue(l) => {
                let v = self.pop();
                if v.to_bool() {
                    self.frames[fidx].ip = *l as usize;
                }
            }
            Op::JumpIfTrueKeep(l) => {
                if self.stack.last().is_some_and(Val::to_bool) {
                    self.frames[fidx].ip = *l as usize;
                } else {
                    self.pop();
                }
            }
            Op::JumpIfNullKeep(l) => {
                // `?->`: short-circuit on null, keep the value either way
                if self.stack.last().is_some_and(Val::is_null) {
                    self.frames[fidx].ip = *l as usize;
                }
            }
            Op::JumpIfNotNullKeep(l) => {
                if self.stack.last().is_some_and(|v| !v.is_null()) {
                    self.frames[fidx].ip = *l as usize;
                } else {
                    self.pop();
                }
            }

            Op::NewArray(n) => self.push(Val::array(PhpArray::with_capacity(*n as usize))),
            Op::ArrayAppend => {
                let v = self.pop();
                let mut arr = self.pop();
                arr.array_mut().expect("array literal").push(v);
                self.push(arr);
            }
            Op::ArrayList(n) => {
                let at = self.stack.len() - *n as usize;
                let arr = PhpArray::from_values(self.stack.drain(at..));
                self.push(Val::array(arr));
            }
            Op::ArrayTemplate { tpl, n } => {
                let Val::Array(t) = self.const_to_val(midx, *tpl) else { unreachable!("array template") };
                let mut arr = (*t).clone();
                let at = self.stack.len() - *n as usize;
                for (i, v) in self.stack.drain(at..).enumerate() {
                    arr.set_at(i, v);
                }
                self.push(Val::array(arr));
            }
            Op::ArrayInsert => {
                let v = self.pop();
                let k = self.pop();
                let mut arr = self.pop();
                let key = self.to_array_key(&k)?;
                arr.array_mut().expect("array literal").insert(key, v);
                self.push(arr);
            }
            Op::ArraySpread => {
                let it = self.pop();
                let mut arr = self.pop();
                let items = match it {
                    Val::Array(a) => a.iter().map(|(k, v)| (k.to_val(), v.clone())).collect::<Vec<_>>(),
                    other => self.iterate_to_vec(other)?,
                };
                let target = arr.array_mut().expect("array literal");
                for (k, v) in items {
                    match k {
                        Val::Str(s) => target.insert(ArrayKey::Str(s), v),
                        _ => target.push(v),
                    }
                }
                self.push(arr);
            }
            Op::FetchDim => {
                let k = self.pop();
                let c = self.pop();
                let v = self.fetch_dim(c, k, false)?;
                self.push(v);
            }
            Op::FetchDimQuiet => {
                let k = self.pop();
                let c = self.pop();
                let v = self.fetch_dim(c, k, true)?;
                self.push(v);
            }
            Op::FetchDimSS { a, k } => {
                let c = self.load_slot_checked(fidx, module, unit, *a)?;
                let k = self.load_slot_checked(fidx, module, unit, *k)?;
                let v = self.fetch_dim(c, k, false)?;
                self.push(v);
            }
            Op::FetchDimS(k) => {
                let k = self.load_slot_checked(fidx, module, unit, *k)?;
                let c = self.pop();
                let v = self.fetch_dim(c, k, false)?;
                self.push(v);
            }
            Op::FetchProp(name) | Op::FetchPropQuiet(name) => {
                let quiet = matches!(op, Op::FetchPropQuiet(_));
                let obj = self.pop();
                if let Val::Object(h) = obj {
                    let class = self.objects.get(h).class;
                    let icx = self.frames[fidx].ic;
                    let e = self.ics[icx][ip];
                    if e.class == Some(class)
                        && let Some(v) = &self.objects.get(h).slots[e.a as usize]
                    {
                        let v = v.clone();
                        self.push(v);
                        return Ok(Flow::Next);
                    }
                    let n = module.str(*name).to_vec();
                    let scope = self.frames[fidx].class;
                    if let super::class::PropLookup::Slot(idx) = self.lookup_prop(class, &n, scope) {
                        self.ics[icx][ip] = IcEntry { class: Some(class), a: idx as u32, b: 0, c: 0, flag: false };
                    }
                    let v = self.read_prop(h, &n, quiet)?;
                    self.push(v);
                } else {
                    let n = module.str(*name).to_vec();
                    let v = self.fetch_prop_value(obj, &n, quiet)?;
                    self.push(v);
                }
            }
            Op::FetchPropDyn | Op::FetchPropDynQuiet => {
                let quiet = matches!(op, Op::FetchPropDynQuiet);
                let name = self.pop();
                let obj = self.pop();
                let n = self.to_string_val(&name)?.as_bytes().to_vec();
                let v = self.fetch_prop_value(obj, &n, quiet)?;
                self.push(v);
            }
            Op::IssetProp(name) => {
                let obj = self.pop();
                let n = module.str(*name).to_vec();
                let set = self.isset_prop(obj, &n)?;
                self.push(Val::Bool(set));
            }
            Op::IssetPropDyn => {
                let name = self.pop();
                let obj = self.pop();
                let n = self.to_string_val(&name)?.as_bytes().to_vec();
                let set = self.isset_prop(obj, &n)?;
                self.push(Val::Bool(set));
            }
            Op::FetchStaticProp(cr, name) => {
                let class = self.resolve_class_ref(cr, midx)?;
                let n = module.str(*name).to_vec();
                let v = self.static_prop_get(class, &n)?;
                self.push(v);
            }
            Op::FetchClassConst(cr, name) => {
                let class = self.resolve_class_ref(cr, midx)?;
                let n = module.str(*name).to_vec();
                let v = self.class_constant(class, &n)?;
                self.push(v);
            }
            Op::FetchConst(name, fallback) => {
                let n = module.str(*name);
                let v = match self.constants.get(n) {
                    Some(v) => v.clone(),
                    None => match fallback.and_then(|f| self.constants.get(module.str(f))) {
                        Some(v) => v.clone(),
                        None => return Err(Throw::error(format!("Undefined constant \"{}\"", String::from_utf8_lossy(n)))),
                    },
                };
                self.push(v);
            }
            Op::FetchClassName(cr) => {
                let class = self.resolve_class_ref(cr, midx)?;
                let name = self.class(class).name.clone();
                self.push(Val::bytes(&name));
            }

            Op::StorePath(path) => {
                let PathOp { base, segs, mode } = &**path;
                // fast path: `$this->prop = v` / `$this->prop op= v` on a cached slot
                if let (PathBase::This, [PathSeg::Prop(_) | PathSeg::PropSel(_)]) = (base, segs.as_slice())
                    && let Some(this) = self.frames[fidx].this
                {
                    let class = self.objects.get(this).class;
                    let icx = self.frames[fidx].ic;
                    let e = self.ics[icx][ip];
                    if e.class == Some(class) && e.flag {
                        let value = self.pop();
                        let idx = e.a as usize;
                        let old = self.objects.get(this).slots[idx].clone().unwrap_or(Val::Null);
                        let (new, result) = match mode {
                            WriteMode::Assign => (value.clone(), value),
                            WriteMode::Compound(bop) => {
                                let n = self.fast_bin(*bop, old, value)?;
                                (n.clone(), n)
                            }
                            WriteMode::PreInc | WriteMode::PostInc => {
                                let n = self.inc_dec(old.clone(), true)?;
                                (n.clone(), if matches!(mode, WriteMode::PreInc) { n } else { old })
                            }
                            WriteMode::PreDec | WriteMode::PostDec => {
                                let n = self.inc_dec(old.clone(), false)?;
                                (n.clone(), if matches!(mode, WriteMode::PreDec) { n } else { old })
                            }
                        };
                        let ty_ok = match &self.class(class).props[idx].ty {
                            None => true,
                            Some(t) => super::path::scalar_type_matches(t, &new),
                        };
                        if ty_ok {
                            self.objects.get_mut(this).slots[idx] = Some(new);
                            self.push(result);
                            return Ok(Flow::Next);
                        }
                        // type needs coercion/check: fall back to the full path with the popped value restored
                        self.push(result);
                        self.objects.get_mut(this).slots[idx] = Some(new);
                        return Ok(Flow::Next);
                    }
                    // fill: plain, non-readonly declared property visible from this scope
                    let pname = match segs[0] {
                        PathSeg::Prop(n) => Some(module.str(n).to_vec()),
                        PathSeg::PropSel(sel) => Some(self.sel_name(sel).to_vec()),
                        _ => None,
                    };
                    if let Some(name) = pname {
                        let scope = self.frames[fidx].class;
                        if let super::class::PropLookup::Slot(idx) = self.lookup_prop(class, &name, scope)
                            && !self.class(class).props[idx].readonly
                        {
                            self.ics[icx][ip] = IcEntry { class: Some(class), a: idx as u32, b: 0, c: 0, flag: true };
                        }
                    }
                }
                let r = self.store_path(base, segs, *mode, midx)?;
                self.push(r);
            }
            Op::UnsetPath(path) => self.unset_path(&path.base, &path.segs, midx)?,
            Op::CompoundSlot { slot, op: bop, discard } => {
                let rhs = self.pop();
                let i = self.frames[fidx].slot_base + *slot as usize;
                let lhs = self.slot_stack[i].clone().unwrap_or(Val::Null);
                let r = self.binary(*bop, lhs, rhs)?;
                if !slot_type_ok(self.slot_stack[i].as_ref(), &r, unit, *slot) {
                    return Err(grd021_error(module, unit, *slot, self.slot_stack[i].as_ref(), &r));
                }
                if !*discard {
                    self.push(r.clone());
                }
                self.slot_stack[self.frames[fidx].slot_base + *slot as usize] = Some(r);
            }
            Op::IncSlotDiscard { slot, mode } => {
                let old = self.slot_stack[self.frames[fidx].slot_base + *slot as usize].take().unwrap_or(Val::Null);
                let new = match mode {
                    WriteMode::PreInc | WriteMode::PostInc => self.inc_dec(old, true)?,
                    _ => self.inc_dec(old, false)?,
                };
                self.slot_stack[self.frames[fidx].slot_base + *slot as usize] = Some(new);
            }
            Op::IncSlot { slot, mode } => {
                let old = self.slot_stack[self.frames[fidx].slot_base + *slot as usize].clone().unwrap_or(Val::Null);
                let (new, result) = match mode {
                    WriteMode::PreInc => {
                        let n = self.inc_dec(old, true)?;
                        (n.clone(), n)
                    }
                    WriteMode::PreDec => {
                        let n = self.inc_dec(old, false)?;
                        (n.clone(), n)
                    }
                    WriteMode::PostInc => (self.inc_dec(old.clone(), true)?, old),
                    WriteMode::PostDec => (self.inc_dec(old.clone(), false)?, old),
                    _ => unreachable!(),
                };
                self.slot_stack[self.frames[fidx].slot_base + *slot as usize] = Some(new);
                self.push(result);
            }

            Op::CallFuncId { id, spec } => {
                let aspec = &unit.arg_specs[*spec as usize];
                // native function with positional arguments: pass them through a
                // reused buffer, no allocation per call (PERF1)
                if let FuncRef::Native(i) = self.function_table[id.0 as usize]
                    && aspec.is_simple()
                {
                    let n = aspec.count();
                    if let Some(r) = self.fast_native(i, n) {
                        self.push(r);
                        return Ok(Flow::Next);
                    }
                    let func = self.natives[i as usize].1;
                    let mut buf = std::mem::take(&mut self.arg_scratch);
                    let at = self.stack.len() - n;
                    buf.extend(self.stack.drain(at..));
                    let r = func(self, &buf).map_err(|t| crate::builtins::polish_type_error(self, &self.natives[i as usize].0, &buf, t));
                    buf.clear();
                    if buf.capacity() > self.arg_scratch.capacity() {
                        self.arg_scratch = buf;
                    }
                    self.push(r?);
                    return Ok(Flow::Next);
                }
                // user function, positional arguments: bind them from the stack
                // straight into the new frame's slots (PERF1)
                if let FuncRef::User { module: tm, unit: tu } = self.function_table[id.0 as usize]
                    && aspec.is_simple()
                {
                    let n = aspec.count();
                    // compiled whole function (JIT1b): pure numeric code returns directly
                    if self.jit.is_some() {
                        let jic = self.ic_base[tm as usize] + tu as usize;
                        if !matches!(self.jit_funcs[jic], super::jit::JitSlotState::Failed)
                            && let Some(v) = self.jit_call(tm, tu, jic, n)
                        {
                            self.push(v);
                            return Ok(Flow::Next);
                        }
                    }
                    let tmod = self.modules[tm as usize].clone();
                    let tunit = &tmod.units[tu as usize];
                    if !tunit.is_generator && n <= tunit.params.len() && !tunit.params.iter().any(|p| p.variadic) {
                        let base = self.slot_stack.len();
                        super::path::grow_slots(&mut self.slot_stack, tunit.num_slots as usize);
                        let at = self.stack.len() - n;
                        for i in 0..n {
                            let v = std::mem::take(&mut self.stack[at + i]);
                            let p = &tunit.params[i];
                            match self.coerce_param(&tmod, tunit, p, v, None) {
                                Ok(v) => self.slot_stack[base + p.slot as usize] = Some(v),
                                Err(e) => {
                                    self.slot_stack.truncate(base);
                                    self.stack.truncate(at);
                                    return Err(e);
                                }
                            }
                        }
                        super::path::shrink(&mut self.stack, at);
                        if tunit.params[n..].iter().any(|p| !p.has_default) {
                            self.slot_stack.truncate(base);
                            return Err(Throw::argument_count(format!(
                                "Too few arguments to function {}(), {} passed and at least {} expected",
                                tunit.name, n, tunit.required_params
                            )));
                        }
                        let ic = self.ic_base[tm as usize] + tu as usize;
                        {
                            let prof = &mut self.profiles[ic];
                            prof.calls = prof.calls.saturating_add(1);
                            if prof.calls >= super::HOT_CALLS {
                                prof.hot = true;
                            }
                        }
                        let frame = super::Frame {
                            module: tmod.clone(),
                            module_idx: tm,
                            unit: tu,
                            ip: 0,
                            slot_base: base,
                            num_slots: tunit.num_slots,
                            this: None,
                            class: None,
                            static_class: None,
                            stack_base: self.stack.len(),
                            arg_present: if n >= 64 { u64::MAX } else { (1u64 << n) - 1 },
                            pending: Pending::None,
                            generator: None,
                            ic,
                            return_override: None,
                        };
                        self.frames.push(frame);
                        return Ok(Flow::Switch);
                    }
                }
                let (args, named) = self.pop_args(midx, aspec)?;
                let f = self.function_table[id.0 as usize].clone();
                match self.invoke_funcref(f, args, named)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::CallMethodSel { sel, spec } => {
                let argspec = &unit.arg_specs[*spec as usize];
                // FRAME1: cached user method with positional arguments: bind them from the stack
                if argspec.is_simple() {
                    let n = argspec.count();
                    if let Val::Object(h) = self.stack[self.stack.len() - n - 1] {
                        let class = self.objects.get(h).class;
                        let e = self.ics[self.frames[fidx].ic][ip];
                        // compiled method (JITO1)
                        if e.class == Some(class) && e.b != super::IC_NATIVE && !e.flag && self.jit.is_some() {
                            let jic = self.ic_base[e.a as usize] + e.b as usize;
                            if !matches!(self.jit_funcs[jic], super::jit::JitSlotState::Failed)
                                && let Some(v) = self.jit_call_method(e.a, e.b, jic, h, ClassId(e.c), n)
                            {
                                self.push(v);
                                return Ok(Flow::Next);
                            }
                        }
                        if e.class == Some(class) && e.b != super::IC_NATIVE && self.push_frame_from_stack(e.a, e.b, n, 1, if e.flag { None } else { Some(h) }, Some(ClassId(e.c)), Some(class), None)? {
                            return Ok(Flow::Switch);
                        }
                    }
                }
                let (args, named) = self.pop_args(midx, argspec)?;
                let obj = self.pop();
                let Val::Object(h) = obj else {
                    return Err(Throw::error(format!("Call to a member function {}() on {}", String::from_utf8_lossy(self.sel_name(*sel)), obj.type_name())));
                };
                let class = self.objects.get(h).class;
                let icx = self.frames[fidx].ic;
                let e = self.ics[icx][ip];
                // CALL1: cached native method, no name lookup and no MethodInfo clone
                if e.class == Some(class) && e.b == super::IC_NATIVE {
                    let handler = self.method_table[e.a as usize].native.expect("native IC");
                    let call = super::native::Call { this: if e.flag { None } else { Some(h) }, class, args, named };
                    let v = handler(self, call)?;
                    self.push(v);
                    return Ok(Flow::Next);
                }
                if e.class == Some(class) {
                    let target = super::call::CallTarget {
                        module: e.a,
                        unit: e.b,
                        this: if e.flag { None } else { Some(h) },
                        class: Some(ClassId(e.c)),
                        static_class: Some(class),
                        captures: Vec::new(),
                        return_override: None,
                    };
                    return match self.push_frame(target, args, named)? {
                        Some(v) => {
                            self.push(v);
                            Ok(Flow::Next)
                        }
                        None => Ok(Flow::Switch),
                    };
                }
                let scope = self.frames[fidx].class;
                // selector lookup: integer-keyed, no name hashing
                if let Some(&mi) = self.class(class).methods_by_sel.get(sel) {
                    let m = &self.method_table[mi as usize];
                    if let Some(handler) = m.native
                        && m.unit.is_none()
                        && self.method_accessible(m, scope)
                        && class != self.wk().closure
                        && class != self.wk().generator
                    {
                        let is_static = m.is_static;
                        self.ics[icx][ip] = IcEntry { class: Some(class), a: mi, b: super::IC_NATIVE, c: 0, flag: is_static };
                        let call = super::native::Call { this: if is_static { None } else { Some(h) }, class, args, named };
                        let v = handler(self, call)?;
                        self.push(v);
                        return Ok(Flow::Next);
                    }
                    if let (Some(u), None) = (m.unit, m.native)
                        && self.method_accessible(m, scope)
                        && class != self.wk().closure
                        && class != self.wk().generator
                    {
                        let (mm, md, ms) = (m.module, m.declaring, m.is_static);
                        self.ics[icx][ip] = IcEntry { class: Some(class), a: mm, b: u, c: md.0, flag: ms };
                        let target = super::call::CallTarget { module: mm, unit: u, this: if ms { None } else { Some(h) }, class: Some(md), static_class: Some(class), captures: Vec::new(), return_override: None };
                        return match self.push_frame(target, args, named)? {
                            Some(v) => {
                                self.push(v);
                                Ok(Flow::Next)
                            }
                            None => Ok(Flow::Switch),
                        };
                    }
                }
                let n = self.sel_name(*sel).to_vec();
                match self.invoke_method(h, &n, args, named, scope)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::CallStaticSel { class, sel, spec } => {
                let argspec = &unit.arg_specs[*spec as usize];
                // a dynamic class (`$class::m()`) sits on the stack below the arguments
                let early = if matches!(class, ClassRef::Dynamic) { None } else { Some(self.resolve_class_ref(class, midx)?) };
                let forwarding = matches!(class, ClassRef::SelfRef | ClassRef::Parent | ClassRef::Static);
                // FRAME1: cached user static method with positional arguments: bind them from the stack
                if let Some(cid) = early
                    && argspec.is_simple()
                {
                    let e = self.ics[self.frames[fidx].ic][ip];
                    if e.class == Some(cid) && e.b != super::IC_NATIVE {
                        let static_class = if forwarding { self.frames[fidx].static_class.unwrap_or(cid) } else { cid };
                        if self.push_frame_from_stack(e.a, e.b, argspec.count(), 0, None, Some(ClassId(e.c)), Some(static_class), None)? {
                            return Ok(Flow::Switch);
                        }
                    }
                }
                let (args, named) = self.pop_args(midx, argspec)?;
                let cid = match early {
                    Some(cid) => cid,
                    None => self.resolve_class_ref(class, midx)?,
                };
                // CALL1: public static methods are cached per call site and class
                // (`Route::get`, `DB::table`, `Str::lower`): visibility and scope do not matter
                let icx = self.frames[fidx].ic;
                let e = self.ics[icx][ip];
                if e.class == Some(cid) {
                    let static_class = if forwarding { self.frames[fidx].static_class.unwrap_or(cid) } else { cid };
                    if e.b == super::IC_NATIVE {
                        let handler = self.method_table[e.a as usize].native.expect("native IC");
                        let v = handler(self, super::native::Call { this: None, class: static_class, args, named })?;
                        self.push(v);
                        return Ok(Flow::Next);
                    }
                    let target = super::call::CallTarget { module: e.a, unit: e.b, this: None, class: Some(ClassId(e.c)), static_class: Some(static_class), captures: Vec::new(), return_override: None };
                    return match self.push_frame(target, args, named)? {
                        Some(v) => {
                            self.push(v);
                            Ok(Flow::Next)
                        }
                        None => Ok(Flow::Switch),
                    };
                }
                if let Some(&mi) = self.class(cid).methods_by_sel.get(sel) {
                    let m = &self.method_table[mi as usize];
                    let cacheable = m.is_static && m.visibility == Visibility::Public && !self.class(cid).is_enum() && cid != self.wk().closure;
                    if cacheable {
                        self.ics[icx][ip] = match (m.native, m.unit) {
                            (Some(_), _) => IcEntry { class: Some(cid), a: mi, b: super::IC_NATIVE, c: 0, flag: true },
                            (None, Some(u)) => IcEntry { class: Some(cid), a: m.module, b: u, c: m.declaring.0, flag: true },
                            (None, None) => self.ics[icx][ip],
                        };
                    }
                }
                let n = self.sel_name(*sel).to_vec();
                let scope = self.frames[fidx].class;
                let this = self.frames[fidx].this.filter(|t| self.instance_of(self.objects.get(*t).class, cid));
                let static_class = if forwarding { self.frames[fidx].static_class.unwrap_or(cid) } else { cid };
                match self.invoke_static(cid, &n, args, named, this, scope, static_class)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::FetchPropSel(sel) | Op::FetchPropSelQuiet(sel) => {
                let quiet = matches!(op, Op::FetchPropSelQuiet(_));
                let obj = self.pop();
                if let Val::Object(h) = obj {
                    let class = self.objects.get(h).class;
                    let icx = self.frames[fidx].ic;
                    let e = self.ics[icx][ip];
                    if e.class == Some(class)
                        && let Some(v) = &self.objects.get(h).slots[e.a as usize]
                    {
                        let v = v.clone();
                        self.push(v);
                        return Ok(Flow::Next);
                    }
                    let scope = self.frames[fidx].class;
                    if let Some(&slot) = self.class(class).props_by_sel.get(sel) {
                        let p = &self.class(class).props[slot as usize];
                        let visible = match p.visibility {
                            Visibility::Public => true,
                            Visibility::Protected => scope.is_some_and(|s| self.instance_of(s, p.declaring) || self.instance_of(p.declaring, s)),
                            Visibility::Private => scope == Some(p.declaring),
                        };
                        if visible {
                            self.ics[icx][ip] = IcEntry { class: Some(class), a: slot, b: 0, c: 0, flag: false };
                        }
                    }
                    let n = self.sel_name(*sel).to_vec();
                    let v = self.read_prop(h, &n, quiet)?;
                    self.push(v);
                } else {
                    let n = self.sel_name(*sel).to_vec();
                    let v = self.fetch_prop_value(obj, &n, quiet)?;
                    self.push(v);
                }
            }
            Op::CallFunc { name, fallback, spec } => {
                let (args, named) = self.pop_args(midx, &unit.arg_specs[*spec as usize])?;
                let n = module.str(*name).to_vec();
                let fb = fallback.map(|f| module.str(f).to_vec());
                match self.invoke_function(&n, fb.as_deref(), args, named)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::CallMethod { name, spec } => {
                let argspec = &unit.arg_specs[*spec as usize];
                let (args, named) = self.pop_args(midx, argspec)?;
                let obj = self.pop();
                let Val::Object(h) = obj else {
                    let n = module.str(*name);
                    return Err(Throw::error(format!("Call to a member function {}() on {}", String::from_utf8_lossy(n), obj.type_name())));
                };
                let class = self.objects.get(h).class;
                let icx = self.frames[fidx].ic;
                let e = self.ics[icx][ip];
                if e.class == Some(class) {
                    let target = super::call::CallTarget {
                        module: e.a,
                        unit: e.b,
                        this: if e.flag { None } else { Some(h) },
                        class: Some(ClassId(e.c)),
                        static_class: Some(class),
                        captures: Vec::new(),
                        return_override: None,
                    };
                    return match self.push_frame(target, args, named)? {
                        Some(v) => {
                            self.push(v);
                            Ok(Flow::Next)
                        }
                        None => Ok(Flow::Switch),
                    };
                }
                let n = module.str(*name).to_vec();
                let scope = self.frames[fidx].class;
                // fill the cache when the method is a plain accessible user method
                if let Some(m) = self.find_method_info(class, &n)
                    && let Some(u) = m.unit
                    && self.method_accessible(&m, scope)
                    && class != self.wk().closure
                    && class != self.wk().generator
                {
                    self.ics[icx][ip] = IcEntry { class: Some(class), a: m.module, b: u, c: m.declaring.0, flag: m.is_static };
                }
                match self.invoke_method(h, &n, args, named, scope)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::CallMethodDyn { spec } => {
                let (args, named) = self.pop_args(midx, &unit.arg_specs[*spec as usize])?;
                let name = self.pop();
                let obj = self.pop();
                let n = self.to_string_val(&name)?.as_bytes().to_vec();
                let Val::Object(h) = obj else {
                    return Err(Throw::error(format!("Call to a member function {}() on {}", String::from_utf8_lossy(&n), obj.type_name())));
                };
                let scope = self.frames[fidx].class;
                match self.invoke_method(h, &n, args, named, scope)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::CallStatic { class, name, spec } => {
                let (args, named) = self.pop_args(midx, &unit.arg_specs[*spec as usize])?;
                let cid = self.resolve_class_ref(class, midx)?;
                let n = module.str(*name).to_vec();
                let scope = self.frames[fidx].class;
                let forwarding = matches!(class, ClassRef::SelfRef | ClassRef::Parent | ClassRef::Static);
                let this = self.frames[fidx].this.filter(|t| self.instance_of(self.objects.get(*t).class, cid));
                let static_class = if forwarding { self.frames[fidx].static_class.unwrap_or(cid) } else { cid };
                match self.invoke_static(cid, &n, args, named, this, scope, static_class)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::CallValue { spec } => {
                let (args, named) = self.pop_args(midx, &unit.arg_specs[*spec as usize])?;
                let callable = self.pop();
                match self.invoke_callable(callable, args, named)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::New { class, spec } => {
                // arguments are still on the operand stack (roots)
                self.gc_safepoint();
                // FRAME1: the constructor of a named class is cached per site (no name
                // lookup, no MethodInfo clone) and its arguments bound from the stack
                let aspec = &unit.arg_specs[*spec as usize];
                if !matches!(class, ClassRef::Dynamic) && aspec.is_simple() {
                    let cid = self.resolve_class_ref(class, midx)?;
                    let icx = self.frames[fidx].ic;
                    if self.ics[icx][ip].class != Some(cid) {
                        match self.class(cid).find_method(b"__construct") {
                            Some(m) => {
                                if let (Some(u), None, Visibility::Public) = (m.unit, m.native, m.visibility) {
                                    self.ics[icx][ip] = IcEntry { class: Some(cid), a: m.module, b: u, c: m.declaring.0, flag: false };
                                }
                            }
                            None if self.native_info(cid).is_none() => self.ics[icx][ip] = IcEntry { class: Some(cid), a: 0, b: super::IC_NO_CTOR, c: 0, flag: false },
                            None => {}
                        }
                    }
                    let e = self.ics[icx][ip];
                    if e.class == Some(cid) && e.b == super::IC_NO_CTOR {
                        if aspec.count() == 0 {
                            let h = self.alloc_object(cid)?;
                            self.push(Val::Object(h));
                            return Ok(Flow::Next);
                        }
                    } else if e.class == Some(cid) {
                        let h = self.alloc_object(cid)?;
                        if self.push_frame_from_stack(e.a, e.b, aspec.count(), 0, Some(h), Some(ClassId(e.c)), Some(cid), Some(Val::Object(h)))? {
                            return Ok(Flow::Switch);
                        }
                        // unusual call shape: the unused object goes to the collector
                    }
                }
                let (args, named) = self.pop_args(midx, &unit.arg_specs[*spec as usize])?;
                let cid = self.resolve_class_ref(class, midx)?;
                match self.invoke_new(cid, args, named)? {
                    CallOutcome::Pushed => return Ok(Flow::Switch),
                    CallOutcome::Value(v) => self.push(v),
                }
            }
            Op::Return => {
                let v = self.pop();
                if let Some(t) = &unit.return_type
                    && !matches!(t, TypeHint::Mixed)
                    && !self.frames[fidx].generator.is_some()
                {
                    // the checked value: int widened to float for `: float`
                    let v = self.check_return(v, t, module, fidx, unit)?;
                    return Ok(self.do_return(v, base_depth));
                }
                return Ok(self.do_return(v, base_depth));
            }
            Op::ReturnViaFinally(l) => {
                let v = self.pop();
                let base = self.frames[fidx].stack_base;
                self.stack.truncate(base);
                self.frames[fidx].pending = Pending::Return(v);
                self.frames[fidx].ip = *l as usize;
            }
            Op::JumpViaFinally { finally, target } => {
                let base = self.frames[fidx].stack_base;
                self.stack.truncate(base);
                self.frames[fidx].pending = Pending::Jump(*target);
                self.frames[fidx].ip = *finally as usize;
            }
            Op::FinallyEnd { outer } => {
                let pending = std::mem::replace(&mut self.frames[fidx].pending, Pending::None);
                match pending {
                    Pending::None => {}
                    Pending::Jump(t) => self.frames[fidx].ip = t as usize,
                    Pending::Return(v) => match outer {
                        Some(l) => {
                            self.frames[fidx].pending = Pending::Return(v);
                            self.frames[fidx].ip = *l as usize;
                        }
                        None => {
                            let v = match &unit.return_type {
                                Some(t) if !matches!(t, TypeHint::Mixed) && self.frames[fidx].generator.is_none() => self.check_return(v, t, module, fidx, unit)?,
                                _ => v,
                            };
                            return Ok(self.do_return(v, base_depth));
                        }
                    },
                    Pending::Throw(e) => match outer {
                        Some(l) => {
                            self.frames[fidx].pending = Pending::Throw(e);
                            self.frames[fidx].ip = *l as usize;
                        }
                        None => return Err(Throw::existing(e)),
                    },
                }
            }

            Op::MakeClosure(cl) => {
                let ClosureOp { unit: u, captures, bind_this } = &**cl;
                let f = &self.frames[fidx];
                let base = f.slot_base;
                let caps: Vec<Option<Val>> = captures.iter().map(|c| self.slot_stack[base + *c as usize].clone()).collect();
                let data = ClosureData::User {
                    module: midx,
                    unit: *u,
                    captures: caps,
                    this: if *bind_this { f.this } else { None },
                    scope: f.class,
                    static_class: f.static_class,
                };
                let h = self.make_closure(data)?;
                self.push(Val::Object(h));
            }
            Op::CallableFromFunc { name, fallback } => {
                let n = module.str(*name);
                let f = match self.functions.get(&n.to_ascii_lowercase()) {
                    Some(f) => f.clone(),
                    None => match fallback.and_then(|fb| self.functions.get(&module.str(fb).to_ascii_lowercase())) {
                        Some(f) => f.clone(),
                        None => return Err(Throw::error(format!("Call to undefined function {}()", String::from_utf8_lossy(n)))),
                    },
                };
                let h = self.make_closure(ClosureData::Function(f))?;
                self.push(Val::Object(h));
            }
            Op::CallableFromMethod(name) => {
                let obj = self.pop();
                let Val::Object(h) = obj else { return Err(Throw::error("Cannot create callable from non-object")) };
                let c = self.make_closure(ClosureData::BoundMethod { this: h, name: module.str(*name).to_vec() })?;
                self.push(Val::Object(c));
            }
            Op::CallableFromStatic(cr, name) => {
                let class = self.resolve_class_ref(cr, midx)?;
                let c = self.make_closure(ClosureData::StaticMethod { class, name: module.str(*name).to_vec() })?;
                self.push(Val::Object(c));
            }

            Op::Throw => {
                let v = self.pop();
                match v {
                    Val::Object(h) if self.is_throwable(self.objects.get(h).class) => return Err(Throw::existing(h)),
                    _ => return Err(Throw::error("Can only throw objects")),
                }
            }
            Op::ThrowUnhandledMatch => {
                let v = self.pop();
                let shown = match &v {
                    Val::Str(s) => format!("'{s}'"),
                    Val::Object(_) => "of type object".to_string(),
                    other => other.to_php_string().map(|s| s.to_string()).unwrap_or_default(),
                };
                return Err(Throw::new("UnhandledMatchError", format!("Unhandled match case {shown}")));
            }

            Op::IterInit(slot) => {
                let v = self.pop();
                let state = self.iter_init(v)?;
                let idx = self.alloc_iter(state);
                self.slot_stack[self.frames[fidx].slot_base + *slot as usize] = Some(Val::Int(idx as i64));
            }
            Op::IterNext { iter, end, with_key } => {
                let idx = match self.slot_stack[self.frames[fidx].slot_base + *iter as usize] {
                    Some(Val::Int(i)) => i as usize,
                    _ => return Err(Throw::error("foreach iterator state missing")),
                };
                let mut state = self.iters[idx].take().expect("iterator");
                self.iters_taken.push(idx);
                let r = self.iter_next(&mut state);
                self.iters_taken.pop();
                self.iters[idx] = Some(state);
                match r? {
                    Some((k, v)) => {
                        if *with_key {
                            self.push(k);
                        }
                        self.push(v);
                    }
                    None => {
                        self.iters[idx] = None;
                        self.frames[fidx].ip = *end as usize;
                    }
                }
            }

            Op::InstanceOf(cr) => {
                let target = match cr {
                    ClassRef::Dynamic => {
                        let v = self.pop();
                        match v {
                            Val::Object(h) => Some(self.objects.get(h).class),
                            Val::Str(s) => self.lookup_class(s.as_bytes()),
                            _ => return Err(Throw::error("Class name must be a valid object or a string")),
                        }
                    }
                    ClassRef::Named(id) => self.lookup_class(module.str(*id)),
                    other => Some(self.resolve_class_ref(other, midx)?),
                };
                let v = self.pop();
                let r = match (v, target) {
                    (Val::Object(h), Some(t)) => self.instance_of(self.objects.get(h).class, t),
                    _ => false,
                };
                self.push(Val::Bool(r));
            }
            Op::Clone => {
                let v = self.pop();
                let Val::Object(h) = v else { return Err(Throw::error("__clone method called on non-object")) };
                self.check_object_limit()?;
                let new = self.objects.clone_object(h);
                // native payloads are cloned where possible
                if let Some(d) = self.objects.get(h).native_ref::<ClosureData>().cloned() {
                    self.objects.get_mut(new).native = Some(Box::new(d));
                } else if let Some(n) = self.native_info(self.objects.get(h).class)
                    && let Some(cl) = n.handlers.clone
                    && let Some(copy) = self.objects.get(h).native.as_deref().and_then(|p| cl(p))
                {
                    self.objects.get_mut(new).native = Some(copy);
                }
                let class = self.objects.get(new).class;
                if self.class(class).find_method(b"__clone").is_some() {
                    self.call_method_sync(new, b"__clone", Vec::new())?;
                }
                self.push(Val::Object(new));
            }
            Op::Echo => {
                let v = self.pop();
                let s = self.to_string_val(&v)?;
                self.output.extend_from_slice(s.as_bytes());
            }
            Op::Yield { with_key } => {
                let value = self.pop();
                let key = if *with_key { Some(self.pop()) } else { None };
                let Some(g) = self.frames[fidx].generator else { return Err(Throw::error("yield outside generator")) };
                self.generator_suspend(g, Some((key, value)));
                if self.frames.len() <= base_depth {
                    return Ok(Flow::Done(Val::Null));
                }
                return Ok(Flow::Switch);
            }
            Op::YieldFrom => {
                let it = self.pop();
                let Some(g) = self.frames[fidx].generator else { return Err(Throw::error("yield from outside generator")) };
                match self.generator_delegate(g, it)? {
                    Some(result) => self.push(result),
                    None => {
                        self.generator_suspend(g, None);
                        if self.frames.len() <= base_depth {
                            return Ok(Flow::Done(Val::Null));
                        }
                        return Ok(Flow::Switch);
                    }
                }
            }
            Op::JumpIfArgPresent { index, target } => {
                let i = *index as usize;
                if i >= 64 || self.frames[fidx].arg_present & (1u64 << i) != 0 {
                    self.frames[fidx].ip = *target as usize;
                }
            }
            Op::PromoteParam { slot, prop } => {
                let this = self.frames[fidx].this.expect("promotion needs $this");
                let v = self.slot_stack[self.frames[fidx].slot_base + *slot as usize].clone().unwrap_or(Val::Null);
                let name = module.str(*prop);
                let class = self.objects.get(this).class;
                if let Some(&idx) = self.class(class).prop_index.get(name) {
                    let icx = self.frames[fidx].ic;
                    self.ics[icx][ip] = IcEntry { class: Some(class), a: idx as u32, b: 0, c: 0, flag: false };
                }
                let name = name.to_vec();
                self.set_prop_raw(this, &name, v);
            }
        }
        Ok(Flow::Next)
    }

    /// Binary op with the int/int fast path inlined.
    #[inline]
    pub(crate) fn fast_bin(&mut self, bop: BinOp, a: Val, b: Val) -> Result<Val, Throw> {
        if let Some(r) = num_fast(bop, &a, &b) {
            return Ok(r);
        }
        self.binary(bop, a, b)
    }

    fn fetch_prop_value(&mut self, obj: Val, name: &[u8], quiet: bool) -> Result<Val, Throw> {
        match obj {
            Val::Object(h) => self.read_prop(h, name, quiet),
            Val::Null if quiet => Ok(Val::Null),
            other => {
                if quiet {
                    Ok(Val::Null)
                } else {
                    Err(Throw::error(format!("Attempt to read property \"{}\" on {}", String::from_utf8_lossy(name), other.type_name())))
                }
            }
        }
    }

    /// The return value after the type check (int → float widening for a
    /// float return type, as in PHP), or the TypeError.
    fn check_return(&mut self, v: Val, t: &TypeHint, module: &Rc<Module>, fidx: usize, unit: &CodeUnit) -> Result<Val, Throw> {
        if super::path::scalar_type_matches(t, &v) {
            return Ok(v);
        }
        let class_ctx = self.frames[fidx].class;
        let static_ctx = self.frames[fidx].static_class;
        let checked = match t {
            TypeHint::Void => v.is_null().then(|| v.clone()),
            TypeHint::Never => None,
            TypeHint::Static => match &v {
                Val::Object(h) if static_ctx.is_some_and(|c| self.instance_of(self.objects.get(*h).class, c)) => Some(v.clone()),
                _ => None,
            },
            _ => self.check_type(&v, t, module, class_ctx),
        };
        if let Some(v) = checked {
            Ok(v)
        } else {
            Err(Throw::type_error(format!(
                "{}(): Return value must be of type {}, {} returned",
                unit.name,
                self.type_name_of(t, module),
                self.value_type_name(&v)
            )))
        }
    }
}

/// GRD021 (D10): may `new` be stored over `old` in local `slot`? A local
/// keeps the type of its first non-null value; `null` only if the local
/// may hold it (`CodeUnit::slot_not_null`).
#[inline(always)]
pub(crate) fn slot_type_ok(old: Option<&Val>, new: &Val, unit: &CodeUnit, slot: Slot) -> bool {
    match old {
        None | Some(Val::Null) => true,
        Some(o) => {
            let same = if matches!(new, Val::Null) {
                !unit.slot_not_null.get(slot as usize).copied().unwrap_or(false)
            } else {
                std::mem::discriminant(o) == std::mem::discriminant(new)
            };
            // a local without a static type takes any value
            same || unit.slot_mixed.get(slot as usize).copied().unwrap_or(false)
        }
    }
}

#[cold]
pub(crate) fn grd021_error(module: &Module, unit: &CodeUnit, slot: Slot, old: Option<&Val>, new: &Val) -> Throw {
    let name = unit.slot_names.get(slot as usize).map(|n| module.str_lossy(*n)).unwrap_or_else(|| "?".into());
    let tn = |v: &Val| match v {
        Val::Null => "null",
        Val::Bool(_) => "bool",
        Val::Int(_) => "int",
        Val::Float(_) => "float",
        Val::Str(_) => "string",
        Val::Array(_) => "array",
        Val::Object(_) => "object",
    };
    let (was, now) = (old.map(tn).unwrap_or("null"), tn(new));
    let hint = match (was, now) {
        ("int", "float") => " (start with 0.0 or (float) if it is to hold floats)",
        ("float", "int") => " (write the value as a float literal or cast it with (float))",
        (_, "null") => " (soll sie null aufnehmen, beginne mit $x = null)",
        _ => "",
    };
    Throw::type_error(format!("GRD021: the local variable ${name} is {was}, but {now} is assigned{hint}"))
}

/// Arithmetic and comparison of two numbers without the generic path
/// (PERF1): int/int, float/float and mixed — `/` is always float (D10),
/// overflow goes to the generic path (ArithmeticError), `===` between int and float is false. `None` means: take the
/// generic path (overflow, division by zero, other types or operators).
#[inline(always)]
pub(crate) fn num_fast(bop: BinOp, a: &Val, b: &Val) -> Option<Val> {
    match (a, b) {
        (Val::Int(x), Val::Int(y)) => {
            let (x, y) = (*x, *y);
            match bop {
                BinOp::Add => x.checked_add(y).map(Val::Int),
                BinOp::Sub => x.checked_sub(y).map(Val::Int),
                BinOp::Mul => x.checked_mul(y).map(Val::Int),
                // always float in the dialect (D10)
                BinOp::Div if y != 0 => Some(Val::Float(x as f64 / y as f64)),
                BinOp::Mod if y != 0 && y != -1 => Some(Val::Int(x % y)),
                BinOp::Lt => Some(Val::Bool(x < y)),
                BinOp::Le => Some(Val::Bool(x <= y)),
                BinOp::Gt => Some(Val::Bool(x > y)),
                BinOp::Ge => Some(Val::Bool(x >= y)),
                BinOp::Identical => Some(Val::Bool(x == y)),
                BinOp::NotIdentical => Some(Val::Bool(x != y)),
                BinOp::BitAnd => Some(Val::Int(x & y)),
                BinOp::BitOr => Some(Val::Int(x | y)),
                BinOp::BitXor => Some(Val::Int(x ^ y)),
                BinOp::Spaceship => Some(Val::Int((x > y) as i64 - (x < y) as i64)),
                _ => None,
            }
        }
        (Val::Float(_), Val::Float(_)) | (Val::Int(_), Val::Float(_)) | (Val::Float(_), Val::Int(_)) => {
            let both_float = matches!((a, b), (Val::Float(_), Val::Float(_)));
            let f = |v: &Val| match v {
                Val::Int(i) => *i as f64,
                Val::Float(f) => *f,
                _ => unreachable!(),
            };
            let (x, y) = (f(a), f(b));
            match bop {
                BinOp::Add => Some(Val::Float(x + y)),
                BinOp::Sub => Some(Val::Float(x - y)),
                BinOp::Mul => Some(Val::Float(x * y)),
                BinOp::Div if y != 0.0 => Some(Val::Float(x / y)),
                BinOp::Lt => Some(Val::Bool(x < y)),
                BinOp::Le => Some(Val::Bool(x <= y)),
                BinOp::Gt => Some(Val::Bool(x > y)),
                BinOp::Ge => Some(Val::Bool(x >= y)),
                BinOp::Identical => Some(Val::Bool(both_float && x == y)),
                BinOp::NotIdentical => Some(Val::Bool(!(both_float && x == y))),
                _ => None,
            }
        }
        _ => None,
    }
}

impl Vm {
    /// `intdiv`, `abs`, `max`, `min` on integers straight on the stack (PERF2):
    /// the result PHP gives, or None to take the native function (other types,
    /// division by zero, overflow), which then also raises the errors.
    #[inline]
    fn fast_native(&mut self, native: u32, n: usize) -> Option<Val> {
        let f = &self.fast_natives;
        let len = self.stack.len();
        let r = if n == 2 && (native == f[0] || native == f[2] || native == f[3]) {
            let (Val::Int(a), Val::Int(b)) = (&self.stack[len - 2], &self.stack[len - 1]) else { return None };
            let (a, b) = (*a, *b);
            if native == f[0] {
                if b == 0 || (a == i64::MIN && b == -1) {
                    return None;
                }
                a / b
            } else if native == f[2] {
                if b > a { b } else { a }
            } else if b < a {
                b
            } else {
                a
            }
        } else if n == 1 && native == f[1] {
            let Val::Int(a) = &self.stack[len - 1] else { return None };
            a.checked_abs()?
        } else {
            return None;
        };
        for _ in 0..n {
            self.stack.pop();
        }
        Some(Val::Int(r))
    }
}

/// JIT2b: element kind seen at a dim fetch (see `run_frames`).
#[inline(never)]
fn note_kind(profiles: &mut [super::CodeProfile], icx: usize, ip: usize, v: &Val) {
    profiles[icx].types[ip] |= super::jit::elem_kind(v);
}
