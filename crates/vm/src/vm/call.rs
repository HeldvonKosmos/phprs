//! Calls: frame setup, argument binding, closures, constructors, and the
//! nested run used by native callbacks and magic methods.

use std::rc::Rc;

use super::{ClosureData, Frame, FuncRef, Pending, Vm};
use crate::bytecode::*;
use crate::error::Throw;
use crate::value::*;

/// Result of starting a call from the dispatch loop.
pub enum CallOutcome {
    /// A frame was pushed; the loop must switch to it.
    Pushed,
    /// The call completed natively; this is its value.
    Value(Val),
}

pub(crate) struct CallTarget {
    pub module: u32,
    pub unit: UnitId,
    pub this: Option<ObjHandle>,
    pub class: Option<ClassId>,
    pub static_class: Option<ClassId>,
    pub captures: Vec<Option<Val>>,
    /// Value to push on return instead of the return value (`new`).
    pub return_override: Option<Val>,
}

impl Vm {
    // ---- argument handling -----------------------------------------------------

    /// Pop the arguments of a call from the stack according to `spec`.
    pub(crate) fn pop_args(&mut self, module: u32, spec: &ArgSpec) -> Result<(Vec<Val>, Vec<(Vec<u8>, Val)>), Throw> {
        // positional only (the common case): the popped values are the arguments
        if spec.is_simple() {
            return Ok((self.pop_n(spec.count()), Vec::new()));
        }
        let raw = self.pop_n(spec.count());
        let mut args = Vec::with_capacity(raw.len());
        let mut named = Vec::new();
        for (i, v) in raw.into_iter().enumerate() {
            if spec.spread[i] {
                match v {
                    Val::Array(a) => {
                        for (k, val) in a.iter() {
                            match k {
                                ArrayKey::Int(_) => {
                                    if !named.is_empty() {
                                        return Err(Throw::error("Cannot use positional argument after named argument during unpacking"));
                                    }
                                    args.push(val.clone());
                                }
                                ArrayKey::Str(s) => named.push((s.as_bytes().to_vec(), val.clone())),
                            }
                        }
                    }
                    Val::Object(h) => {
                        let items = self.iterate_to_vec(Val::Object(h))?;
                        for (_, val) in items {
                            args.push(val);
                        }
                    }
                    other => return Err(Throw::type_error(format!("Only arrays and Traversables can be unpacked, {} given", other.type_name()))),
                }
            } else if let Some(n) = spec.names[i] {
                named.push((self.str_of(module, n).to_vec(), v));
            } else {
                args.push(v);
            }
        }
        Ok((args, named))
    }

    /// Bind arguments to parameter slots. Returns (slots, arg_present).
    fn bind_args(
        &mut self,
        module: &Rc<Module>,
        unit: &CodeUnit,
        mut args: Vec<Val>,
        named: Vec<(Vec<u8>, Val)>,
        class_ctx: Option<ClassId>,
        base: usize,
    ) -> Result<u64, Throw> {
        // the window was already reserved by the caller
        let mut present: u64 = 0;
        let set = |present: &mut u64, i: usize| {
            if i < 64 {
                *present |= 1u64 << i;
            }
        };
        let is_set = |present: u64, i: usize| i >= 64 || present & (1u64 << i) != 0;
        let n_params = unit.params.len();
        let variadic_at = unit.params.iter().position(|p| p.variadic);
        let fixed = variadic_at.unwrap_or(n_params);
        let mut rest: Vec<Val> = Vec::new();
        if args.len() > fixed {
            rest = args.split_off(fixed);
        }
        for (i, v) in args.into_iter().enumerate() {
            let p = &unit.params[i];
            let v = self.coerce_param(module, unit, p, v, class_ctx)?;
            self.slot_stack[base + p.slot as usize] = Some(v);
            set(&mut present, i);
        }
        let mut extra_named: Vec<(Vec<u8>, Val)> = Vec::new();
        for (name, v) in named {
            match unit.params.iter().position(|p| module.str(p.name) == name.as_slice() && !p.variadic) {
                Some(i) => {
                    if is_set(present, i) {
                        return Err(Throw::error(format!("Named parameter ${} overwrites previous argument", String::from_utf8_lossy(&name))));
                    }
                    let p = &unit.params[i];
                    let v = self.coerce_param(module, unit, p, v, class_ctx)?;
                    self.slot_stack[base + p.slot as usize] = Some(v);
                    set(&mut present, i);
                }
                None => {
                    if variadic_at.is_some() {
                        extra_named.push((name, v));
                    } else {
                        return Err(Throw::error(format!("Unknown named parameter ${}", String::from_utf8_lossy(&name))));
                    }
                }
            }
        }
        if let Some(vi) = variadic_at {
            let p = &unit.params[vi];
            let mut arr = PhpArray::new();
            for v in rest {
                let v = self.coerce_param(module, unit, p, v, class_ctx)?;
                arr.push(v);
            }
            for (k, v) in extra_named {
                let v = self.coerce_param(module, unit, p, v, class_ctx)?;
                arr.insert(ArrayKey::Str(PhpString::from(k)), v);
            }
            self.slot_stack[base + p.slot as usize] = Some(Val::array(arr));
            set(&mut present, vi);
        }
        for (i, p) in unit.params.iter().enumerate() {
            if !is_set(present, i) && !p.has_default && !p.variadic {
                let given = present.count_ones();
                return Err(Throw::argument_count(format!(
                    "Too few arguments to function {}(), {} passed and at least {} expected",
                    unit.name, given, unit.required_params
                )));
            }
        }
        Ok(present)
    }

    #[inline(always)]
    pub(crate) fn coerce_param(&mut self, module: &Rc<Module>, unit: &CodeUnit, p: &Param, v: Val, class_ctx: Option<ClassId>) -> Result<Val, Throw> {
        match &p.ty {
            None => Ok(v),
            // FRAME1: the common case (int to int, …) needs neither a clone nor the full check
            Some(t) if super::path::scalar_type_matches(t, &v) => Ok(v),
            Some(_) => self.coerce_param_slow(module, unit, p, v, class_ctx),
        }
    }

    #[inline(never)]
    fn coerce_param_slow(&mut self, module: &Rc<Module>, unit: &CodeUnit, p: &Param, v: Val, class_ctx: Option<ClassId>) -> Result<Val, Throw> {
        match &p.ty {
            None => Ok(v),
            Some(t) => match self.check_type(&v, t, module, class_ctx) {
                Some(v) => Ok(v),
                None => Err(Throw::type_error(format!(
                    "{}(): Argument #{} (${}) must be of type {}, {} given",
                    unit.name,
                    unit.params.iter().position(|x| x.slot == p.slot).unwrap_or(0) + 1,
                    module.str_lossy(p.name),
                    self.type_name_of(t, module),
                    self.value_type_name(&v)
                ))),
            },
        }
    }

    /// The class a type hint names, resolved once per (module, name).
    pub(crate) fn hint_class(&mut self, module: &Rc<Module>, name: StrId) -> Option<ClassId> {
        let key = (Rc::as_ptr(module) as usize, name);
        if let Some(c) = self.hint_classes.get(&key) {
            return *c;
        }
        let hint = module.str(name);
        let hint = hint.strip_prefix(b"\\").unwrap_or(hint).to_vec();
        let c = self.lookup_class(&hint);
        // only remember found classes: one declared later must still be found
        if c.is_some() {
            self.hint_classes.insert(key, c);
        }
        c
    }

    /// `Some(value)` if `v` matches `t` (with int→float widening), else `None`.
    pub(crate) fn check_type(&mut self, v: &Val, t: &TypeHint, module: &Rc<Module>, class_ctx: Option<ClassId>) -> Option<Val> {
        match t {
            TypeHint::Mixed => Some(v.clone()),
            TypeHint::Null => v.is_null().then(|| v.clone()),
            TypeHint::Bool => matches!(v, Val::Bool(_)).then(|| v.clone()),
            TypeHint::True => matches!(v, Val::Bool(true)).then(|| v.clone()),
            TypeHint::False => matches!(v, Val::Bool(false)).then(|| v.clone()),
            TypeHint::Int => matches!(v, Val::Int(_)).then(|| v.clone()),
            TypeHint::Float => match v {
                Val::Float(_) => Some(v.clone()),
                Val::Int(i) => Some(Val::Float(*i as f64)),
                _ => None,
            },
            TypeHint::String => matches!(v, Val::Str(_)).then(|| v.clone()),
            TypeHint::Array => matches!(v, Val::Array(_)).then(|| v.clone()),
            TypeHint::Object => matches!(v, Val::Object(_)).then(|| v.clone()),
            TypeHint::Iterable => match v {
                Val::Array(_) => Some(v.clone()),
                Val::Object(h) => self.instance_of(self.objects.get(*h).class, self.wk().traversable).then(|| v.clone()),
                _ => None,
            },
            TypeHint::Callable => self.is_callable(v).then(|| v.clone()),
            TypeHint::Void | TypeHint::Never => None,
            TypeHint::Static | TypeHint::SelfT => match (v, class_ctx) {
                (Val::Object(h), Some(c)) => self.instance_of(self.objects.get(*h).class, c).then(|| v.clone()),
                _ => None,
            },
            TypeHint::Parent => match (v, class_ctx.and_then(|c| self.class(c).parent)) {
                (Val::Object(h), Some(p)) => self.instance_of(self.objects.get(*h).class, p).then(|| v.clone()),
                _ => None,
            },
            TypeHint::Class(name) => match v {
                Val::Object(h) => {
                    let class = self.objects.get(*h).class;
                    let target = self.hint_class(module, *name)?;
                    (class == target || self.instance_of(class, target)).then(|| v.clone())
                }
                _ => None,
            },
            TypeHint::Nullable(inner) => {
                if v.is_null() {
                    Some(Val::Null)
                } else {
                    self.check_type(v, inner, module, class_ctx)
                }
            }
            TypeHint::Union(parts) => {
                // exact match first, then widening
                for p in parts {
                    if let Some(r) = self.check_type(v, p, module, class_ctx)
                        && identical(&r, v)
                    {
                        return Some(r);
                    }
                }
                parts.iter().find_map(|p| self.check_type(v, p, module, class_ctx))
            }
            TypeHint::Intersection(parts) => {
                parts.iter().all(|p| self.check_type(v, p, module, class_ctx).is_some()).then(|| v.clone())
            }
        }
    }

    pub(crate) fn type_name_of(&self, t: &TypeHint, module: &Rc<Module>) -> String {
        match t {
            TypeHint::Mixed => "mixed".into(),
            TypeHint::Null => "null".into(),
            TypeHint::Bool => "bool".into(),
            TypeHint::True => "true".into(),
            TypeHint::False => "false".into(),
            TypeHint::Int => "int".into(),
            TypeHint::Float => "float".into(),
            TypeHint::String => "string".into(),
            TypeHint::Array => "array".into(),
            TypeHint::Object => "object".into(),
            TypeHint::Iterable => "iterable".into(),
            TypeHint::Callable => "callable".into(),
            TypeHint::Void => "void".into(),
            TypeHint::Never => "never".into(),
            TypeHint::Static => "static".into(),
            TypeHint::SelfT => "self".into(),
            TypeHint::Parent => "parent".into(),
            TypeHint::Class(n) => module.str_lossy(*n),
            TypeHint::Nullable(i) => format!("?{}", self.type_name_of(i, module)),
            TypeHint::Union(p) => p.iter().map(|x| self.type_name_of(x, module)).collect::<Vec<_>>().join("|"),
            TypeHint::Intersection(p) => p.iter().map(|x| self.type_name_of(x, module)).collect::<Vec<_>>().join("&"),
        }
    }

    pub(crate) fn value_type_name(&self, v: &Val) -> String {
        match v {
            Val::Object(h) => String::from_utf8_lossy(&self.class(self.objects.get(*h).class).name).into_owned(),
            other => other.type_name().to_string(),
        }
    }

    // ---- frames ------------------------------------------------------------------

    /// FRAME1: push a frame whose positional arguments are the top `n` values
    /// of the operand stack, bound straight into the new slots (no argument
    /// Vec); `drop_below` more values under them (the receiver of a method
    /// call) are removed too. Returns false with the stack untouched when the
    /// callee needs the general path (generator, variadic, too many arguments).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn push_frame_from_stack(&mut self, module_idx: u32, unit_id: UnitId, n: usize, drop_below: usize, this: Option<ObjHandle>, class: Option<ClassId>, static_class: Option<ClassId>, return_override: Option<Val>) -> Result<bool, Throw> {
        let module = self.modules[module_idx as usize].clone();
        let unit = &module.units[unit_id as usize];
        if unit.is_generator || n > unit.params.len() || unit.params.iter().any(|p| p.variadic) {
            return Ok(false);
        }
        let num_slots = unit.num_slots;
        let base = self.slot_stack.len();
        super::path::grow_slots(&mut self.slot_stack, num_slots as usize);
        let at = self.stack.len() - n;
        for i in 0..n {
            let v = std::mem::take(&mut self.stack[at + i]);
            let p = &unit.params[i];
            match self.coerce_param(&module, unit, p, v, class) {
                Ok(v) => self.slot_stack[base + p.slot as usize] = Some(v),
                Err(e) => {
                    self.slot_stack.truncate(base);
                    self.stack.truncate(at - drop_below);
                    return Err(e);
                }
            }
        }
        super::path::shrink(&mut self.stack, at - drop_below);
        if unit.params[n..].iter().any(|p| !p.has_default) {
            self.slot_stack.truncate(base);
            return Err(Throw::argument_count(format!("Too few arguments to function {}(), {} passed and at least {} expected", unit.name, n, unit.required_params)));
        }
        let ic = self.ic_base[module_idx as usize] + unit_id as usize;
        let prof = &mut self.profiles[ic];
        prof.calls = prof.calls.saturating_add(1);
        if prof.calls >= super::HOT_CALLS {
            prof.hot = true;
        }
        self.frames.push(Frame {
            module,
            module_idx,
            unit: unit_id,
            ip: 0,
            slot_base: base,
            num_slots,
            this,
            class,
            static_class,
            stack_base: self.stack.len(),
            arg_present: if n >= 64 { u64::MAX } else { (1u64 << n) - 1 },
            pending: Pending::None,
            generator: None,
            ic,
            return_override,
        });
        Ok(true)
    }

    /// Push a frame for `target`. Generators are not started: a Generator
    /// object is returned instead.
    pub(crate) fn push_frame(&mut self, target: CallTarget, args: Vec<Val>, named: Vec<(Vec<u8>, Val)>) -> Result<Option<Val>, Throw> {
        let module = self.modules[target.module as usize].clone();
        let unit = &module.units[target.unit as usize];
        let (num_slots, pbase, is_generator) = (unit.num_slots, unit.params.len(), unit.is_generator);
        let base = self.slot_stack.len();
        self.slot_stack.resize(base + num_slots as usize, None);
        let present = match self.bind_args(&module, unit, args, named, target.class, base) {
            Ok(p) => p,
            Err(e) => {
                self.slot_stack.truncate(base);
                return Err(e);
            }
        };
        let ic = self.ic_base[target.module as usize] + target.unit as usize;
        {
            let prof = &mut self.profiles[ic];
            prof.calls = prof.calls.saturating_add(1);
            if prof.calls >= super::HOT_CALLS {
                prof.hot = true;
            }
        }
        for (i, c) in target.captures.into_iter().enumerate() {
            if pbase + i < num_slots as usize {
                self.slot_stack[base + pbase + i] = c;
            }
        }
        // the Rc moves into the frame (FRAME1: no second reference count round trip)
        let frame = Frame {
            module,
            module_idx: target.module,
            unit: target.unit,
            ip: 0,
            slot_base: base,
            num_slots,
            this: target.this,
            class: target.class,
            static_class: target.static_class,
            stack_base: self.stack.len(),
            arg_present: present,
            pending: Pending::None,
            generator: None,
            ic,
            return_override: target.return_override,
        };
        if is_generator {
            let saved = self.slot_stack.split_off(base);
            let h = self.make_generator(frame, saved)?;
            return Ok(Some(Val::Object(h)));
        }
        self.frames.push(frame);
        Ok(None)
    }

    /// Run a unit to completion on a nested dispatch loop.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn call_unit_sync(
        &mut self,
        _module: Rc<Module>,
        midx: u32,
        unit: UnitId,
        args: Vec<Val>,
        named: Vec<(Vec<u8>, Val)>,
        this: Option<ObjHandle>,
        class: Option<ClassId>,
        static_class: Option<ClassId>,
        captures: Vec<Option<Val>>,
    ) -> Result<Val, Throw> {
        let depth = self.frames.len();
        let target = CallTarget { module: midx, unit, this, class, static_class, captures, return_override: None };
        if let Some(v) = self.push_frame(target, args, named)? {
            return Ok(v);
        }
        self.run(depth)
    }

    /// Call any callable value and wait for the result.
    pub fn call_value(&mut self, callable: &Val, args: Vec<Val>) -> Result<Val, Throw> {
        let depth = self.frames.len();
        // PERF3: a user closure (callbacks of array_map, usort, …) takes the
        // path of a direct call: arguments on the stack, no Vec of named
        // arguments; captured values go into the slots after the parameters
        if let Val::Object(h) = callable
            && let Some(ClosureData::User { module, unit, captures, this, scope, static_class }) = self.objects.get(*h).native_ref::<ClosureData>()
        {
            let (module, unit, this, scope, static_class) = (*module, *unit, *this, *scope, *static_class);
            let captures = (!captures.is_empty()).then(|| captures.clone());
            let n = args.len();
            self.stack.extend(args);
            if self.push_frame_from_stack(module, unit, n, 0, this, scope, static_class, None)? {
                if let Some(captures) = captures {
                    let f = self.frames.last().expect("pushed frame");
                    let (base, num_slots) = (f.slot_base, f.num_slots as usize);
                    let pbase = self.modules[module as usize].units[unit as usize].params.len();
                    for (i, c) in captures.into_iter().enumerate() {
                        if pbase + i < num_slots {
                            self.slot_stack[base + pbase + i] = c;
                        }
                    }
                }
                return self.run(depth);
            }
            let at = self.stack.len() - n;
            let args = self.stack.split_off(at);
            return match self.invoke_callable(callable.clone(), args, Vec::new())? {
                CallOutcome::Value(v) => Ok(v),
                CallOutcome::Pushed => self.run(depth),
            };
        }
        match self.invoke_callable(callable.clone(), args, Vec::new())? {
            CallOutcome::Value(v) => Ok(v),
            CallOutcome::Pushed => self.run(depth),
        }
    }

    pub fn call_value_named(&mut self, callable: &Val, args: Vec<Val>, named: Vec<(Vec<u8>, Val)>) -> Result<Val, Throw> {
        let depth = self.frames.len();
        match self.invoke_callable(callable.clone(), args, named)? {
            CallOutcome::Value(v) => Ok(v),
            CallOutcome::Pushed => self.run(depth),
        }
    }

    pub fn call_method_sync(&mut self, this: ObjHandle, name: &[u8], args: Vec<Val>) -> Result<Val, Throw> {
        let depth = self.frames.len();
        let scope = Some(self.objects.get(this).class);
        match self.invoke_method(this, name, args, Vec::new(), scope)? {
            CallOutcome::Value(v) => Ok(v),
            CallOutcome::Pushed => self.run(depth),
        }
    }

    pub fn call_static_sync(&mut self, class: ClassId, name: &[u8], args: Vec<Val>) -> Result<Val, Throw> {
        let depth = self.frames.len();
        match self.invoke_static(class, name, args, Vec::new(), None, Some(class), class)? {
            CallOutcome::Value(v) => Ok(v),
            CallOutcome::Pushed => self.run(depth),
        }
    }

    /// `new C(...)` on a nested loop.
    pub fn instantiate(&mut self, class: ClassId, args: Vec<Val>) -> Result<ObjHandle, Throw> {
        let depth = self.frames.len();
        match self.invoke_new(class, args, Vec::new())? {
            CallOutcome::Value(Val::Object(h)) => Ok(h),
            CallOutcome::Value(_) => unreachable!(),
            CallOutcome::Pushed => match self.run(depth)? {
                Val::Object(h) => Ok(h),
                _ => unreachable!("constructor frame returns the object"),
            },
        }
    }

    // ---- invocation from the dispatch loop --------------------------------------

    pub(crate) fn invoke_function(&mut self, name: &[u8], fallback: Option<&[u8]>, args: Vec<Val>, named: Vec<(Vec<u8>, Val)>) -> Result<CallOutcome, Throw> {
        let f = match self.functions.get(&name.to_ascii_lowercase()) {
            Some(f) => f.clone(),
            None => match fallback.and_then(|fb| self.functions.get(&fb.to_ascii_lowercase())) {
                Some(f) => f.clone(),
                None => return Err(Throw::error(format!("Call to undefined function {}()", String::from_utf8_lossy(name)))),
            },
        };
        self.invoke_funcref(f, args, named)
    }

    pub(crate) fn invoke_funcref(&mut self, f: FuncRef, args: Vec<Val>, named: Vec<(Vec<u8>, Val)>) -> Result<CallOutcome, Throw> {
        match f {
            FuncRef::Native(i) => {
                if !named.is_empty() {
                    return Err(Throw::error(format!("Named arguments are not supported for native function {}()", self.natives[i as usize].0)));
                }
                let func = self.natives[i as usize].1;
                match func(self, &args) {
                    Ok(v) => Ok(CallOutcome::Value(v)),
                    Err(t) => Err(crate::builtins::polish_type_error(self, &self.natives[i as usize].0, &args, t)),
                }
            }
            FuncRef::User { module, unit } => {
                let target = CallTarget { module, unit, this: None, class: None, static_class: None, captures: Vec::new(), return_override: None };
                match self.push_frame(target, args, named)? {
                    Some(v) => Ok(CallOutcome::Value(v)),
                    None => Ok(CallOutcome::Pushed),
                }
            }
        }
    }

    pub(crate) fn find_method_info(&self, class: ClassId, name: &[u8]) -> Option<super::MethodInfo> {
        self.class(class).find_method(name).cloned()
    }

    /// `$obj->name(args)`
    pub(crate) fn invoke_method(&mut self, this: ObjHandle, name: &[u8], args: Vec<Val>, named: Vec<(Vec<u8>, Val)>, scope: Option<ClassId>) -> Result<CallOutcome, Throw> {
        let class = self.objects.get(this).class;
        let wk_closure = self.wk().closure;
        let wk_generator = self.wk().generator;
        if class == wk_closure {
            return self.closure_method(this, name, args);
        }
        if class == wk_generator {
            return Ok(CallOutcome::Value(self.generator_method(this, name, args)?));
        }
        match self.find_method_info(class, name) {
            Some(m) if self.method_accessible(&m, scope) => {
                if let Some(handler) = m.native {
                    let call = super::native::Call { this: if m.is_static { None } else { Some(this) }, class, args, named };
                    return Ok(CallOutcome::Value(handler(self, call)?));
                }
                let Some(unit) = m.unit else {
                    return Err(Throw::error(format!("Cannot call abstract method {}::{}()", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(&m.name))));
                };
                let target = CallTarget {
                    module: m.module,
                    unit,
                    this: if m.is_static { None } else { Some(this) },
                    class: Some(m.declaring),
                    static_class: Some(class),
                    captures: Vec::new(),
                    return_override: None,
                };
                match self.push_frame(target, args, named)? {
                    Some(v) => Ok(CallOutcome::Value(v)),
                    None => Ok(CallOutcome::Pushed),
                }
            }
            found => {
                // __call fallback
                if let Some(m) = self.find_method_info(class, b"__call")
                    && !name.eq_ignore_ascii_case(b"__call")
                    && let Some(handler) = m.native
                {
                    let mut arr = PhpArray::new();
                    for a in args {
                        arr.push(a);
                    }
                    let call = super::native::Call { this: Some(this), class, args: vec![Val::bytes(name), Val::array(arr)], named: Vec::new() };
                    return Ok(CallOutcome::Value(handler(self, call)?));
                }
                if let Some(m) = self.find_method_info(class, b"__call")
                    && let Some(unit) = m.unit
                    && !name.eq_ignore_ascii_case(b"__call")
                {
                    let mut arr = PhpArray::new();
                    for a in args {
                        arr.push(a);
                    }
                    let target = CallTarget { module: m.module, unit, this: Some(this), class: Some(m.declaring), static_class: Some(class), captures: Vec::new(), return_override: None };
                    let call_args = vec![Val::bytes(name), Val::array(arr)];
                    return match self.push_frame(target, call_args, Vec::new())? {
                        Some(v) => Ok(CallOutcome::Value(v)),
                        None => Ok(CallOutcome::Pushed),
                    };
                }
                let cname = String::from_utf8_lossy(&self.class(class).name).into_owned();
                Err(match found {
                    Some(m) => Throw::error(format!("Call to {} method {}::{}() from {}", vis_name(m.visibility), cname, String::from_utf8_lossy(name), scope_name(self, scope))),
                    None => Throw::error(format!("Call to undefined method {}::{}()", cname, String::from_utf8_lossy(name))),
                })
            }
        }
    }

    /// `Class::name(args)`; `this` is forwarded for `parent::`/`self::` calls
    /// to non-static methods.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn invoke_static(
        &mut self,
        class: ClassId,
        name: &[u8],
        args: Vec<Val>,
        named: Vec<(Vec<u8>, Val)>,
        this: Option<ObjHandle>,
        scope: Option<ClassId>,
        static_class: ClassId,
    ) -> Result<CallOutcome, Throw> {
        if class == self.wk().closure {
            return self.closure_static(name, args);
        }
        if self.class(class).is_enum() {
            if let Some(v) = self.enum_static(class, name, &args)? {
                return Ok(CallOutcome::Value(v));
            }
        }
        match self.find_method_info(class, name) {
            Some(m) if self.method_accessible(&m, scope) => {
                if let Some(handler) = m.native {
                    if !m.is_static && this.is_none() {
                        return Err(Throw::error(format!("Non-static method {}::{}() cannot be called statically", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(&m.name))));
                    }
                    let call = super::native::Call { this: if m.is_static { None } else { this }, class: static_class, args, named };
                    return Ok(CallOutcome::Value(handler(self, call)?));
                }
                let Some(unit) = m.unit else {
                    return Err(Throw::error(format!("Cannot call abstract method {}::{}()", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(&m.name))));
                };
                if !m.is_static && this.is_none() {
                    return Err(Throw::error(format!("Non-static method {}::{}() cannot be called statically", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(&m.name))));
                }
                let target = CallTarget {
                    module: m.module,
                    unit,
                    this: if m.is_static { None } else { this },
                    class: Some(m.declaring),
                    static_class: Some(static_class),
                    captures: Vec::new(),
                    return_override: None,
                };
                match self.push_frame(target, args, named)? {
                    Some(v) => Ok(CallOutcome::Value(v)),
                    None => Ok(CallOutcome::Pushed),
                }
            }
            found => {
                if let Some(m) = self.find_method_info(class, b"__callstatic")
                    && let Some(handler) = m.native
                {
                    let mut arr = PhpArray::new();
                    for a in args {
                        arr.push(a);
                    }
                    let call = super::native::Call { this: None, class: static_class, args: vec![Val::bytes(name), Val::array(arr)], named: Vec::new() };
                    return Ok(CallOutcome::Value(handler(self, call)?));
                }
                if let Some(m) = self.find_method_info(class, b"__callstatic")
                    && let Some(unit) = m.unit
                {
                    let mut arr = PhpArray::new();
                    for a in args {
                        arr.push(a);
                    }
                    let target = CallTarget { module: m.module, unit, this: None, class: Some(m.declaring), static_class: Some(static_class), captures: Vec::new(), return_override: None };
                    let call_args = vec![Val::bytes(name), Val::array(arr)];
                    return match self.push_frame(target, call_args, Vec::new())? {
                        Some(v) => Ok(CallOutcome::Value(v)),
                        None => Ok(CallOutcome::Pushed),
                    };
                }
                let cname = String::from_utf8_lossy(&self.class(class).name).into_owned();
                Err(match found {
                    Some(m) => Throw::error(format!("Call to {} method {}::{}() from {}", vis_name(m.visibility), cname, String::from_utf8_lossy(name), scope_name(self, scope))),
                    None => Throw::error(format!("Call to undefined method {}::{}()", cname, String::from_utf8_lossy(name))),
                })
            }
        }
    }

    /// `new C(args)`: the object is pushed when the constructor returns.
    pub(crate) fn invoke_new(&mut self, class: ClassId, args: Vec<Val>, named: Vec<(Vec<u8>, Val)>) -> Result<CallOutcome, Throw> {
        let h = self.alloc_object(class)?;
        match self.find_method_info(class, b"__construct") {
            Some(m) => {
                let scope = self.frames.last().and_then(|f| f.class);
                if !self.method_accessible(&m, scope) {
                    return Err(Throw::error(format!("Call to {} {}::__construct() from {}", vis_name(m.visibility), String::from_utf8_lossy(&self.class(class).name), scope_name(self, scope))));
                }
                if let Some(handler) = m.native {
                    let call = super::native::Call { this: Some(h), class, args, named };
                    handler(self, call)?;
                    return Ok(CallOutcome::Value(Val::Object(h)));
                }
                let Some(unit) = m.unit else {
                    return Err(Throw::error("Cannot call abstract constructor"));
                };
                let target = CallTarget {
                    module: m.module,
                    unit,
                    this: Some(h),
                    class: Some(m.declaring),
                    static_class: Some(class),
                    captures: Vec::new(),
                    return_override: Some(Val::Object(h)),
                };
                match self.push_frame(target, args, named)? {
                    Some(_) => Ok(CallOutcome::Value(Val::Object(h))),
                    None => Ok(CallOutcome::Pushed),
                }
            }
            None => Ok(CallOutcome::Value(Val::Object(h))),
        }
    }

    // ---- callables ---------------------------------------------------------------

    pub fn is_callable(&self, v: &Val) -> bool {
        match v {
            Val::Object(h) => {
                let class = self.objects.get(*h).class;
                class == self.wk().closure || self.class(class).find_method(b"__invoke").is_some()
            }
            Val::Str(s) => {
                let b = s.as_bytes();
                match b.windows(2).position(|w| w == b"::") {
                    Some(i) => self.lookup_class(&b[..i]).is_some_and(|c| self.class(c).find_method(&b[i + 2..]).is_some()),
                    None => self.functions.contains_key(&b.to_ascii_lowercase()),
                }
            }
            Val::Array(a) if a.len() == 2 => {
                let (Some((_, first)), Some((_, second))) = (a.entry_at(0), a.entry_at(1)) else { return false };
                let Val::Str(m) = second else { return false };
                match first {
                    Val::Object(h) => self.class(self.objects.get(*h).class).find_method(m.as_bytes()).is_some() || self.class(self.objects.get(*h).class).find_method(b"__call").is_some(),
                    Val::Str(c) => self.lookup_class(c.as_bytes()).is_some_and(|c| self.class(c).find_method(m.as_bytes()).is_some()),
                    _ => false,
                }
            }
            _ => false,
        }
    }

    pub(crate) fn invoke_callable(&mut self, callable: Val, args: Vec<Val>, named: Vec<(Vec<u8>, Val)>) -> Result<CallOutcome, Throw> {
        let scope = self.frames.last().and_then(|f| f.class);
        match callable {
            Val::Object(h) => {
                let class = self.objects.get(h).class;
                if class == self.wk().closure {
                    return self.invoke_closure(h, args, named);
                }
                if self.class(class).find_method(b"__invoke").is_some() {
                    return self.invoke_method(h, b"__invoke", args, named, scope);
                }
                Err(Throw::error("Object of class is not callable"))
            }
            Val::Str(s) => {
                let b = s.as_bytes().to_vec();
                match b.windows(2).position(|w| w == b"::") {
                    Some(i) => {
                        let class = self.lookup_class(&b[..i]).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", String::from_utf8_lossy(&b[..i]))))?;
                        self.invoke_static(class, &b[i + 2..], args, named, None, scope, class)
                    }
                    None => self.invoke_function(&b, None, args, named),
                }
            }
            Val::Array(a) if a.len() == 2 => {
                let first = a.entry_at(0).map(|(_, v)| v.clone()).unwrap_or(Val::Null);
                let second = a.entry_at(1).map(|(_, v)| v.clone()).unwrap_or(Val::Null);
                let Val::Str(m) = second else { return Err(Throw::error("Array callable must have exactly two elements")) };
                let m = m.as_bytes().to_vec();
                match first {
                    Val::Object(h) => self.invoke_method(h, &m, args, named, scope),
                    Val::Str(c) => {
                        let class = self.lookup_class(c.as_bytes()).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", c)))?;
                        let this = self.frames.last().and_then(|f| f.this).filter(|t| self.instance_of(self.objects.get(*t).class, class));
                        self.invoke_static(class, &m, args, named, this, scope, class)
                    }
                    _ => Err(Throw::error("Array callable must have exactly two elements")),
                }
            }
            other => Err(Throw::error(format!("Value of type {} is not callable", other.type_name()))),
        }
    }

    pub(crate) fn closure_data(&self, h: ObjHandle) -> ClosureData {
        self.objects.get(h).native_ref::<ClosureData>().cloned().expect("closure payload")
    }

    pub(crate) fn make_closure(&mut self, data: ClosureData) -> Result<ObjHandle, Throw> {
        let class = self.wk().closure;
        let mut obj = PhpObject::new(class, 0);
        obj.native = Some(Box::new(data));
        self.check_object_limit()?;
        Ok(self.objects.alloc(obj))
    }

    fn invoke_closure(&mut self, h: ObjHandle, args: Vec<Val>, named: Vec<(Vec<u8>, Val)>) -> Result<CallOutcome, Throw> {
        match self.closure_data(h) {
            ClosureData::User { module, unit, captures, this, scope, static_class } => {
                let target = CallTarget { module, unit, this, class: scope, static_class, captures, return_override: None };
                match self.push_frame(target, args, named)? {
                    Some(v) => Ok(CallOutcome::Value(v)),
                    None => Ok(CallOutcome::Pushed),
                }
            }
            ClosureData::BoundMethod { this, name } => {
                let scope = Some(self.objects.get(this).class);
                self.invoke_method(this, &name, args, named, scope)
            }
            ClosureData::StaticMethod { class, name } => self.invoke_static(class, &name, args, named, None, Some(class), class),
            ClosureData::Function(f) => self.invoke_funcref(f, args, named),
        }
    }

    /// Methods on Closure objects: `call`, `bindTo`, `__invoke`.
    fn closure_method(&mut self, h: ObjHandle, name: &[u8], mut args: Vec<Val>) -> Result<CallOutcome, Throw> {
        match name.to_ascii_lowercase().as_slice() {
            b"__invoke" => self.invoke_closure(h, args, Vec::new()),
            b"call" => {
                if args.is_empty() {
                    return Err(Throw::argument_count("Closure::call() expects at least 1 argument"));
                }
                let new_this = args.remove(0);
                let Val::Object(t) = new_this else { return Err(Throw::type_error("Closure::call(): Argument #1 ($newThis) must be of type object")) };
                let bound = self.bind_closure(h, Some(t), Some(self.objects.get(t).class))?;
                self.invoke_closure(bound, args, Vec::new())
            }
            b"bindto" => {
                let new_this = match args.first() {
                    Some(Val::Object(t)) => Some(*t),
                    _ => None,
                };
                let scope = self.scope_arg(args.get(1), new_this)?;
                Ok(CallOutcome::Value(Val::Object(self.bind_closure(h, new_this, scope)?)))
            }
            _ => Err(Throw::error(format!("Call to undefined method Closure::{}()", String::from_utf8_lossy(name)))),
        }
    }

    fn closure_static(&mut self, name: &[u8], args: Vec<Val>) -> Result<CallOutcome, Throw> {
        match name.to_ascii_lowercase().as_slice() {
            b"bind" => {
                let Some(Val::Object(h)) = args.first() else { return Err(Throw::type_error("Closure::bind(): Argument #1 ($closure) must be of type Closure")) };
                let new_this = match args.get(1) {
                    Some(Val::Object(t)) => Some(*t),
                    _ => None,
                };
                let scope = self.scope_arg(args.get(2), new_this)?;
                Ok(CallOutcome::Value(Val::Object(self.bind_closure(*h, new_this, scope)?)))
            }
            b"fromcallable" => {
                let Some(c) = args.first() else { return Err(Throw::argument_count("Closure::fromCallable() expects 1 argument")) };
                Ok(CallOutcome::Value(self.callable_to_closure(c.clone())?))
            }
            _ => Err(Throw::error(format!("Call to undefined method Closure::{}()", String::from_utf8_lossy(name)))),
        }
    }

    fn scope_arg(&self, arg: Option<&Val>, new_this: Option<ObjHandle>) -> Result<Option<ClassId>, Throw> {
        Ok(match arg {
            Some(Val::Str(s)) if s.as_bytes() == b"static" => None,
            Some(Val::Str(s)) => Some(self.lookup_class(s.as_bytes()).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", s)))?),
            Some(Val::Object(o)) => Some(self.objects.get(*o).class),
            _ => new_this.map(|t| self.objects.get(t).class),
        })
    }

    fn bind_closure(&mut self, h: ObjHandle, new_this: Option<ObjHandle>, scope: Option<ClassId>) -> Result<ObjHandle, Throw> {
        let data = match self.closure_data(h) {
            ClosureData::User { module, unit, captures, this: old_this, scope: old_scope, static_class } => {
                let scope = scope.or(old_scope);
                ClosureData::User {
                    module,
                    unit,
                    captures,
                    this: new_this.or(old_this),
                    scope,
                    static_class: new_this.map(|t| self.objects.get(t).class).or(scope).or(static_class),
                }
            }
            other => other,
        };
        self.make_closure(data)
    }

    pub(crate) fn callable_to_closure(&mut self, c: Val) -> Result<Val, Throw> {
        let data = match &c {
            Val::Object(h) if self.objects.get(*h).class == self.wk().closure => return Ok(c),
            Val::Object(h) => ClosureData::BoundMethod { this: *h, name: b"__invoke".to_vec() },
            Val::Str(s) => {
                let b = s.as_bytes();
                match b.windows(2).position(|w| w == b"::") {
                    Some(i) => {
                        let class = self.lookup_class(&b[..i]).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", String::from_utf8_lossy(&b[..i]))))?;
                        ClosureData::StaticMethod { class, name: b[i + 2..].to_vec() }
                    }
                    None => {
                        let f = self.functions.get(&b.to_ascii_lowercase()).cloned().ok_or_else(|| Throw::error(format!("Call to undefined function {}()", s)))?;
                        ClosureData::Function(f)
                    }
                }
            }
            Val::Array(a) if a.len() == 2 => {
                let first = a.entry_at(0).map(|(_, v)| v.clone()).unwrap_or(Val::Null);
                let Some((_, Val::Str(m))) = a.entry_at(1) else { return Err(Throw::type_error("Argument must be a valid callback")) };
                match first {
                    Val::Object(h) => ClosureData::BoundMethod { this: h, name: m.as_bytes().to_vec() },
                    Val::Str(cn) => {
                        let class = self.lookup_class(cn.as_bytes()).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", cn)))?;
                        ClosureData::StaticMethod { class, name: m.as_bytes().to_vec() }
                    }
                    _ => return Err(Throw::type_error("Argument must be a valid callback")),
                }
            }
            _ => return Err(Throw::type_error("Argument must be a valid callback")),
        };
        Ok(Val::Object(self.make_closure(data)?))
    }

    /// `Enum::from`, `tryFrom`, `cases`.
    fn enum_static(&mut self, class: ClassId, name: &[u8], args: &[Val]) -> Result<Option<Val>, Throw> {
        match name.to_ascii_lowercase().as_slice() {
            b"cases" => {
                let cases = self.enum_cases_list(class)?;
                Ok(Some(Val::array(PhpArray::from_values(cases.into_iter().map(Val::Object)))))
            }
            b"from" | b"tryfrom" => {
                let is_try = name.eq_ignore_ascii_case(b"tryfrom");
                let needle = args.first().cloned().unwrap_or(Val::Null);
                let cases = self.enum_cases_list(class)?;
                for h in cases {
                    if let Some(v) = self.get_prop_raw(h, b"value")
                        && identical(&v, &needle)
                    {
                        return Ok(Some(Val::Object(h)));
                    }
                }
                if is_try {
                    Ok(Some(Val::Null))
                } else {
                    let shown = match &needle {
                        Val::Str(s) => format!("\"{s}\""),
                        other => other.to_php_string().map(|s| s.to_string()).unwrap_or_default(),
                    };
                    Err(Throw::value_error(format!("{} is not a valid backing value for enum {}", shown, String::from_utf8_lossy(&self.class(class).name))))
                }
            }
            _ => Ok(None),
        }
    }
}

fn vis_name(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

fn scope_name(vm: &Vm, scope: Option<ClassId>) -> String {
    match scope {
        Some(c) => format!("scope {}", String::from_utf8_lossy(&vm.class(c).name)),
        None => "global scope".to_string(),
    }
}
