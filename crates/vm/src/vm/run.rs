//! The dispatch loop.

use std::rc::Rc;

use super::call::CallOutcome;
use super::{ClosureData, IcEntry, Pending, Vm};
use crate::bytecode::*;
use crate::error::Throw;
use crate::value::*;

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
        self.run(depth)
    }

    /// Run until the frame stack drops back to `base_depth`.
    pub(crate) fn run(&mut self, base_depth: usize) -> Result<Val, Throw> {
        'frames: loop {
            if self.frames.len() <= base_depth {
                return Ok(Val::Null);
            }
            let fidx = self.frames.len() - 1;
            let module = self.frames[fidx].module.clone();
            let midx = self.frames[fidx].module_idx;
            let unit_idx = self.frames[fidx].unit as usize;
            let unit = &module.units[unit_idx];
            loop {
                let ip = self.frames[fidx].ip;
                self.frames[fidx].ip = ip + 1;
                let op = &unit.ops[ip];
                match self.exec(op, fidx, midx, &module, unit, base_depth) {
                    Ok(Flow::Next) => {}
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
            ClassRef::Named(id) => {
                let name = self.str_of(module, *id).to_vec();
                self.lookup_class(&name).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", String::from_utf8_lossy(&name))))
            }
            ClassRef::SelfRef => self.frame().class.ok_or_else(|| Throw::error("Cannot use \"self\" when no class scope is active")),
            ClassRef::Parent => {
                let c = self.frame().class.ok_or_else(|| Throw::error("Cannot use \"parent\" when no class scope is active"))?;
                self.class(c).parent.ok_or_else(|| Throw::error("Cannot use \"parent\" when current class scope has no parent"))
            }
            ClassRef::Static => self.frame().static_class.or(self.frame().class).ok_or_else(|| Throw::error("Cannot use \"static\" when no class scope is active")),
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

    fn do_return(&mut self, value: Val, base_depth: usize) -> Flow {
        let frame = self.frames.pop().expect("frame");
        self.stack.truncate(frame.stack_base);
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
            Op::Line(n) => self.frames[fidx].line = *n,
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

            Op::LoadSlot(s) => match &self.frames[fidx].slots[*s as usize] {
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
                let v = self.frames[fidx].slots[*s as usize].clone().unwrap_or(Val::Null);
                self.push(v);
            }
            Op::StoreSlot(s) => {
                let v = self.pop();
                self.frames[fidx].slots[*s as usize] = Some(v);
            }
            Op::UnsetSlot(s) => self.frames[fidx].slots[*s as usize] = None,
            Op::LoadThis => match self.frames[fidx].this {
                Some(h) => self.push(Val::Object(h)),
                None => return Err(Throw::error("Using $this when not in object context")),
            },

            Op::Bin(bop) => {
                let b = self.pop();
                let a = self.pop();
                if let (Val::Int(x), Val::Int(y)) = (&a, &b) {
                    let (x, y) = (*x, *y);
                    let fast = match bop {
                        BinOp::Add => x.checked_add(y).map(Val::Int),
                        BinOp::Sub => x.checked_sub(y).map(Val::Int),
                        BinOp::Mul => x.checked_mul(y).map(Val::Int),
                        BinOp::Mod if y != 0 && y != -1 => Some(Val::Int(x % y)),
                        BinOp::Lt => Some(Val::Bool(x < y)),
                        BinOp::Le => Some(Val::Bool(x <= y)),
                        BinOp::Gt => Some(Val::Bool(x > y)),
                        BinOp::Ge => Some(Val::Bool(x >= y)),
                        BinOp::Identical => Some(Val::Bool(x == y)),
                        BinOp::NotIdentical => Some(Val::Bool(x != y)),
                        _ => None,
                    };
                    if let Some(r) = fast {
                        self.push(r);
                        return Ok(Flow::Next);
                    }
                }
                let r = self.binary(*bop, a, b)?;
                self.push(r);
            }
            Op::BinSS { a, b, op: bop } => {
                let x = self.frames[fidx].slots[*a as usize].clone();
                let y = self.frames[fidx].slots[*b as usize].clone();
                let (Some(x), Some(y)) = (x, y) else {
                    let name = unit.slot_names.get(if self.frames[fidx].slots[*a as usize].is_none() { *a } else { *b } as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                    return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                };
                let r = self.fast_bin(*bop, x, y)?;
                self.push(r);
            }
            Op::BinSI { a, imm, op: bop } => {
                let Some(x) = self.frames[fidx].slots[*a as usize].clone() else {
                    let name = unit.slot_names.get(*a as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                    return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                };
                let r = self.fast_bin(*bop, x, Val::Int(*imm as i64))?;
                self.push(r);
            }
            Op::BinSC { a, c, op: bop } => {
                let Some(x) = self.frames[fidx].slots[*a as usize].clone() else {
                    let name = unit.slot_names.get(*a as usize).map(|n| module.str_lossy(*n)).unwrap_or_default();
                    return Err(Throw::new("UndefinedVariableError", format!("Undefined variable ${name}")));
                };
                let y = self.const_to_val(midx, *c);
                let r = self.fast_bin(*bop, x, y)?;
                self.push(r);
            }
            Op::JumpUnlessSS { a, b, op: bop, target } => {
                let x = self.frames[fidx].slots[*a as usize].clone();
                let y = self.frames[fidx].slots[*b as usize].clone();
                let (Some(x), Some(y)) = (x, y) else {
                    return Err(Throw::new("UndefinedVariableError", "Undefined variable"));
                };
                if !self.fast_bin(*bop, x, y)?.to_bool() {
                    self.frames[fidx].ip = *target as usize;
                }
            }
            Op::JumpUnlessSI { a, imm, op: bop, target } => {
                let cond = match &self.frames[fidx].slots[*a as usize] {
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

            Op::Jump(l) => self.frames[fidx].ip = *l as usize,
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
                if let (PathBase::This, [PathSeg::Prop(_)]) = (base, segs.as_slice())
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
                    if let PathSeg::Prop(n) = segs[0] {
                        let name = module.str(n).to_vec();
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
                let lhs = self.frames[fidx].slots[*slot as usize].take().unwrap_or(Val::Null);
                let r = match self.binary(*bop, lhs, rhs) {
                    Ok(r) => r,
                    Err(e) => return Err(e),
                };
                if !*discard {
                    self.push(r.clone());
                }
                self.frames[fidx].slots[*slot as usize] = Some(r);
            }
            Op::IncSlotDiscard { slot, mode } => {
                let old = self.frames[fidx].slots[*slot as usize].take().unwrap_or(Val::Null);
                let new = match mode {
                    WriteMode::PreInc | WriteMode::PostInc => self.inc_dec(old, true)?,
                    _ => self.inc_dec(old, false)?,
                };
                self.frames[fidx].slots[*slot as usize] = Some(new);
            }
            Op::IncSlot { slot, mode } => {
                let old = self.frames[fidx].slots[*slot as usize].clone().unwrap_or(Val::Null);
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
                self.frames[fidx].slots[*slot as usize] = Some(new);
                self.push(result);
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
                    self.check_return(&v, t, module, fidx, unit)?;
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
                        None => return Ok(self.do_return(v, base_depth)),
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
                let caps: Vec<Option<Val>> = captures.iter().map(|c| f.slots[*c as usize].clone()).collect();
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
                self.frames[fidx].slots[*slot as usize] = Some(Val::Int(idx as i64));
            }
            Op::IterNext { iter, end, with_key } => {
                let idx = match self.frames[fidx].slots[*iter as usize] {
                    Some(Val::Int(i)) => i as usize,
                    _ => return Err(Throw::error("foreach iterator state missing")),
                };
                let mut state = self.iters[idx].take().expect("iterator");
                let r = self.iter_next(&mut state);
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
                let new = self.objects.clone_object(h);
                // native payloads are cloned where possible
                if let Some(d) = self.objects.get(h).native_ref::<ClosureData>().cloned() {
                    self.objects.get_mut(new).native = Some(Box::new(d));
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
                if self.frames[fidx].arg_present.get(*index as usize).copied().unwrap_or(false) {
                    self.frames[fidx].ip = *target as usize;
                }
            }
            Op::PromoteParam { slot, prop } => {
                let this = self.frames[fidx].this.expect("promotion needs $this");
                let v = self.frames[fidx].slots[*slot as usize].clone().unwrap_or(Val::Null);
                let name = module.str(*prop).to_vec();
                self.set_prop_raw(this, &name, v);
            }
        }
        Ok(Flow::Next)
    }

    /// Binary op with the int/int fast path inlined.
    #[inline]
    pub(crate) fn fast_bin(&mut self, bop: BinOp, a: Val, b: Val) -> Result<Val, Throw> {
        if let (Val::Int(x), Val::Int(y)) = (&a, &b) {
            let (x, y) = (*x, *y);
            let fast = match bop {
                BinOp::Add => x.checked_add(y).map(Val::Int),
                BinOp::Sub => x.checked_sub(y).map(Val::Int),
                BinOp::Mul => x.checked_mul(y).map(Val::Int),
                BinOp::Mod if y != 0 && y != -1 => Some(Val::Int(x % y)),
                BinOp::Lt => Some(Val::Bool(x < y)),
                BinOp::Le => Some(Val::Bool(x <= y)),
                BinOp::Gt => Some(Val::Bool(x > y)),
                BinOp::Ge => Some(Val::Bool(x >= y)),
                BinOp::Identical => Some(Val::Bool(x == y)),
                BinOp::NotIdentical => Some(Val::Bool(x != y)),
                _ => None,
            };
            if let Some(r) = fast {
                return Ok(r);
            }
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

    fn check_return(&mut self, v: &Val, t: &TypeHint, module: &Rc<Module>, fidx: usize, unit: &CodeUnit) -> Result<(), Throw> {
        let class_ctx = self.frames[fidx].class;
        let static_ctx = self.frames[fidx].static_class;
        let ok = match t {
            TypeHint::Void => v.is_null(),
            TypeHint::Never => false,
            TypeHint::Static => match v {
                Val::Object(h) => static_ctx.is_some_and(|c| self.instance_of(self.objects.get(*h).class, c)),
                _ => false,
            },
            _ => self.check_type(v, t, module, class_ctx).is_some(),
        };
        if ok {
            Ok(())
        } else {
            Err(Throw::type_error(format!(
                "{}(): Return value must be of type {}, {} returned",
                unit.name,
                self.type_name_of(t, module),
                self.value_type_name(v)
            )))
        }
    }
}
