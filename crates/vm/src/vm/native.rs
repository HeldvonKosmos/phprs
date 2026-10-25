//! Native classes: registration, calls, property handlers, conversions.
//! (`RUNTIME_ARCHITECTURE.md` §8, `NATIVE_CLASSES.md` §3.)

use std::any::Any;
use std::rc::Rc;

use indexmap::IndexMap;

use super::class::{ConstInfo, MethodInfo, RtClass};
use super::Vm;
use crate::bytecode::{ClassKind, Init, TypeHint, Visibility};
use crate::error::Throw;
use crate::value::*;

/// Module index used for native members (they have no code module).
pub const NATIVE_MODULE: u32 = u32::MAX;

/// One native method call.
pub struct Call {
    pub this: Option<ObjHandle>,
    /// The called class (late static binding).
    pub class: ClassId,
    pub args: Vec<Val>,
    pub named: Vec<(Vec<u8>, Val)>,
}

impl Call {
    pub fn arg(&self, i: usize) -> Val {
        self.args.get(i).cloned().unwrap_or(Val::Null)
    }
    /// Strict argument access (TYP3, DIALECT §5.2): no scalar coercion, only
    /// int -> float; `what` names the method in the TypeError.
    pub fn int(&self, i: usize, what: &str) -> Result<i64, Throw> {
        match self.args.get(i) {
            Some(Val::Int(n)) => Ok(*n),
            Some(o) => Err(Throw::type_error(format!("{what}(): Argument #{} must be of type int, {} given", i + 1, o.type_name()))),
            None => Err(Throw::argument_count(format!("{what}(): Argument #{} is required", i + 1))),
        }
    }
    pub fn float(&self, i: usize, what: &str) -> Result<f64, Throw> {
        match self.args.get(i) {
            Some(Val::Int(n)) => Ok(*n as f64),
            Some(Val::Float(f)) => Ok(*f),
            Some(o) => Err(Throw::type_error(format!("{what}(): Argument #{} must be of type float, {} given", i + 1, o.type_name()))),
            None => Err(Throw::argument_count(format!("{what}(): Argument #{} is required", i + 1))),
        }
    }
    pub fn string(&self, i: usize, what: &str) -> Result<crate::value::PhpString, Throw> {
        match self.args.get(i) {
            Some(Val::Str(s)) => Ok(s.clone()),
            Some(o) => Err(Throw::type_error(format!("{what}(): Argument #{} must be of type string, {} given", i + 1, o.type_name()))),
            None => Err(Throw::argument_count(format!("{what}(): Argument #{} is required", i + 1))),
        }
    }
    pub fn this(&self) -> Result<ObjHandle, Throw> {
        self.this.ok_or_else(|| Throw::error("Using $this when not in object context"))
    }
}

pub type NativeMethodFn = fn(&mut Vm, Call) -> Result<Val, Throw>;
pub type PropGetFn = fn(&mut Vm, ObjHandle, &[u8]) -> Result<Option<Val>, Throw>;
pub type PropSetFn = fn(&mut Vm, ObjHandle, &[u8], Val) -> Result<bool, Throw>;
pub type PropIssetFn = fn(&mut Vm, ObjHandle, &[u8]) -> Result<Option<bool>, Throw>;
pub type PropUnsetFn = fn(&mut Vm, ObjHandle, &[u8]) -> Result<bool, Throw>;
pub type PayloadFn = fn() -> Box<dyn Any>;
/// `compare(vm, this, other)`: ordering of a native value type against any value
/// (`None` = incomparable). Used for `==`, `<`, `>`, `<=>` on objects of the class.
pub type CompareFn = fn(&mut Vm, ObjHandle, &Val) -> Result<Option<std::cmp::Ordering>, Throw>;
/// Arithmetic on a native value type (`$decimal + 1`, `$duration * 2`):
/// `(vm, this, op, other, reversed)`; `reversed` = the object was the right
/// operand. `None` = not handled (falls through to the normal TypeError).
pub type ArithFn = fn(&mut Vm, ObjHandle, crate::bytecode::BinOp, &Val, bool) -> Result<Option<Val>, Throw>;
/// Copy of a native payload for `clone $obj`.
pub type CloneFn = fn(&dyn Any) -> Option<Box<dyn Any>>;

/// Object handlers of a native class; `None` falls through to the normal
/// property machinery (declared slots, `__get`/`__set`).
#[derive(Clone, Copy, Default)]
pub struct ObjectHandlers {
    pub get: Option<PropGetFn>,
    pub set: Option<PropSetFn>,
    pub isset: Option<PropIssetFn>,
    pub unset: Option<PropUnsetFn>,
    pub compare: Option<CompareFn>,
    pub arith: Option<ArithFn>,
    pub clone: Option<CloneFn>,
    /// MEM1b: reports the values the payload holds; a payload without it
    /// stops in-request collection while such an object is alive.
    pub trace: Option<super::gc::TraceFn>,
}

pub struct NativeMethodDef {
    pub name: &'static str,
    pub is_static: bool,
    pub visibility: Visibility,
    pub handler: NativeMethodFn,
}

pub struct NativeClassDef {
    pub name: &'static str,
    pub parent: Option<&'static str>,
    pub interfaces: &'static [&'static str],
    pub is_abstract: bool,
    pub is_final: bool,
    pub payload: Option<PayloadFn>,
    pub methods: Vec<NativeMethodDef>,
    pub constants: Vec<(&'static str, Val)>,
    pub handlers: ObjectHandlers,
}

impl NativeClassDef {
    pub fn new(name: &'static str) -> Self {
        NativeClassDef {
            name,
            parent: None,
            interfaces: &[],
            is_abstract: false,
            is_final: false,
            payload: None,
            methods: Vec::new(),
            constants: Vec::new(),
            handlers: ObjectHandlers::default(),
        }
    }
    pub fn method(mut self, name: &'static str, handler: NativeMethodFn) -> Self {
        self.methods.push(NativeMethodDef { name, is_static: false, visibility: Visibility::Public, handler });
        self
    }
    pub fn static_method(mut self, name: &'static str, handler: NativeMethodFn) -> Self {
        self.methods.push(NativeMethodDef { name, is_static: true, visibility: Visibility::Public, handler });
        self
    }
    pub fn protected_method(mut self, name: &'static str, handler: NativeMethodFn) -> Self {
        self.methods.push(NativeMethodDef { name, is_static: false, visibility: Visibility::Protected, handler });
        self
    }
    pub fn payload(mut self, f: PayloadFn) -> Self {
        self.payload = Some(f);
        self
    }
    pub fn handlers(mut self, h: ObjectHandlers) -> Self {
        let trace = self.handlers.trace;
        self.handlers = h;
        self.handlers.trace = self.handlers.trace.or(trace);
        self
    }
    /// MEM1b: how the payload reports the values it holds (`gc::trace_nothing`
    /// for payloads without values).
    pub fn trace(mut self, f: super::gc::TraceFn) -> Self {
        self.handlers.trace = Some(f);
        self
    }
    pub fn constant(mut self, name: &'static str, v: Val) -> Self {
        self.constants.push((name, v));
        self
    }
    pub fn abstract_class(mut self) -> Self {
        self.is_abstract = true;
        self
    }
    pub fn final_class(mut self) -> Self {
        self.is_final = true;
        self
    }
    pub fn extends(mut self, parent: &'static str) -> Self {
        self.parent = Some(parent);
        self
    }
    pub fn implements(mut self, interfaces: &'static [&'static str]) -> Self {
        self.interfaces = interfaces;
        self
    }
}

/// Native part of a runtime class.
#[derive(Clone, Copy)]
pub struct NativeInfo {
    pub payload: Option<PayloadFn>,
    pub handlers: ObjectHandlers,
}

impl std::fmt::Debug for NativeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NativeInfo {{ payload: {} }}", self.payload.is_some())
    }
}

/// A callable value (closure, function name, `[obj, 'method']`, invokable).
pub struct Callable(pub Val);

impl Callable {
    pub fn call(&self, vm: &mut Vm, args: Vec<Val>) -> Result<Val, Throw> {
        vm.call_value(&self.0, args)
    }
}

/// Convert a dialect value into a Rust type (strict, DIALECT §5.2).
pub trait FromVal: Sized {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw>;
}

/// Convert a Rust value into a dialect value.
pub trait IntoVal {
    fn into_val(self, vm: &mut Vm) -> Result<Val, Throw>;
}

fn mismatch(expected: &str, v: &Val, vm: &Vm) -> Throw {
    Throw::type_error(format!("Expected {expected}, {} given", vm.value_type_name(v)))
}

impl FromVal for Val {
    fn from_val(v: &Val, _: &Vm) -> Result<Self, Throw> {
        Ok(v.clone())
    }
}
impl FromVal for i64 {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        match v {
            Val::Int(i) => Ok(*i),
            _ => Err(mismatch("int", v, vm)),
        }
    }
}
impl FromVal for f64 {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        match v {
            Val::Float(f) => Ok(*f),
            Val::Int(i) => Ok(*i as f64),
            _ => Err(mismatch("float", v, vm)),
        }
    }
}
impl FromVal for bool {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        match v {
            Val::Bool(b) => Ok(*b),
            _ => Err(mismatch("bool", v, vm)),
        }
    }
}
impl FromVal for PhpString {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        match v {
            Val::Str(s) => Ok(s.clone()),
            _ => Err(mismatch("string", v, vm)),
        }
    }
}
impl FromVal for String {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        Ok(PhpString::from_val(v, vm)?.to_string_lossy())
    }
}
impl FromVal for Rc<PhpArray> {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        match v {
            Val::Array(a) => Ok(a.clone()),
            _ => Err(mismatch("array", v, vm)),
        }
    }
}
impl FromVal for ObjHandle {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        match v {
            Val::Object(h) => Ok(*h),
            _ => Err(mismatch("object", v, vm)),
        }
    }
}
impl FromVal for Callable {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        if vm.is_callable(v) { Ok(Callable(v.clone())) } else { Err(mismatch("callable", v, vm)) }
    }
}
impl<T: FromVal> FromVal for Option<T> {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        if v.is_null() { Ok(None) } else { T::from_val(v, vm).map(Some) }
    }
}
impl<T: FromVal> FromVal for Vec<T> {
    fn from_val(v: &Val, vm: &Vm) -> Result<Self, Throw> {
        let a = Rc::<PhpArray>::from_val(v, vm)?;
        a.values().map(|x| T::from_val(x, vm)).collect()
    }
}

impl IntoVal for Val {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(self)
    }
}
impl IntoVal for () {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::Null)
    }
}
impl IntoVal for i64 {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::Int(self))
    }
}
impl IntoVal for f64 {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::Float(self))
    }
}
impl IntoVal for bool {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::Bool(self))
    }
}
impl IntoVal for String {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::from(self))
    }
}
impl IntoVal for &str {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::str(self))
    }
}
impl IntoVal for PhpString {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::Str(self))
    }
}
impl IntoVal for PhpArray {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::array(self))
    }
}
impl IntoVal for ObjHandle {
    fn into_val(self, _: &mut Vm) -> Result<Val, Throw> {
        Ok(Val::Object(self))
    }
}
impl<T: IntoVal> IntoVal for Option<T> {
    fn into_val(self, vm: &mut Vm) -> Result<Val, Throw> {
        match self {
            Some(v) => v.into_val(vm),
            None => Ok(Val::Null),
        }
    }
}
impl<T: IntoVal> IntoVal for Vec<T> {
    fn into_val(self, vm: &mut Vm) -> Result<Val, Throw> {
        let mut a = PhpArray::with_capacity(self.len());
        for v in self {
            a.push(v.into_val(vm)?);
        }
        Ok(Val::array(a))
    }
}
impl<T: IntoVal, E: Into<Throw>> IntoVal for Result<T, E> {
    fn into_val(self, vm: &mut Vm) -> Result<Val, Throw> {
        match self {
            Ok(v) => v.into_val(vm),
            Err(e) => Err(e.into()),
        }
    }
}

impl Vm {
    /// Register a native class. Parents and interfaces must already exist.
    pub fn register_class(&mut self, def: NativeClassDef) -> Result<ClassId, Throw> {
        let lname = def.name.to_ascii_lowercase().into_bytes();
        if self.class_by_name.contains_key(&lname) {
            return Err(Throw::error(format!("Cannot declare class {}, because the name is already in use", def.name)));
        }
        let id = ClassId(self.classes.len() as u32);
        let parent = match def.parent {
            Some(p) => Some(self.lookup_class(p.as_bytes()).ok_or_else(|| Throw::error(format!("Class \"{p}\" not found")))?),
            None => None,
        };
        let mut rt = RtClass {
            id,
            name: def.name.as_bytes().to_vec(),
            kind: ClassKind::Class,
            parent,
            interfaces: Vec::new(),
            is_abstract: def.is_abstract,
            is_final: def.is_final,
            module: NATIVE_MODULE,
            consts: IndexMap::new(),
            props: Vec::new(),
            prop_index: Default::default(),
            static_props: IndexMap::new(),
            methods: Default::default(),
            cases: Vec::new(),
            enum_backing: None,
            attributes: Vec::new(),
            line: 0,
            default_slots: None,
            native: Some(NativeInfo { payload: def.payload, handlers: def.handlers }),
            methods_by_sel: Default::default(),
            props_by_sel: Default::default(),
        };
        if let Some(p) = parent {
            let pc = self.class(p);
            rt.interfaces = pc.interfaces.clone();
            rt.consts = pc.consts.clone();
            rt.props = pc.props.clone();
            rt.prop_index = pc.prop_index.clone();
            rt.methods = pc.methods.clone();
            if rt.native.as_ref().is_some_and(|n| n.payload.is_none())
                && let Some(pn) = pc.native
            {
                rt.native = Some(NativeInfo { payload: pn.payload, handlers: if def.handlers.get.is_none() && def.handlers.set.is_none() { pn.handlers } else { def.handlers } });
            }
        }
        for i in def.interfaces {
            let iid = self.lookup_class(i.as_bytes()).ok_or_else(|| Throw::error(format!("Interface \"{i}\" not found")))?;
            if !rt.interfaces.contains(&iid) {
                rt.interfaces.push(iid);
            }
            for sub in self.class(iid).interfaces.clone() {
                if !rt.interfaces.contains(&sub) {
                    rt.interfaces.push(sub);
                }
            }
        }
        for (name, v) in def.constants {
            rt.consts.insert(name.as_bytes().to_vec(), ConstInfo { value: Some(v), init: Init::Const(0), module: NATIVE_MODULE, visibility: Visibility::Public, declaring: id, is_final: false });
        }
        for m in def.methods {
            rt.methods.insert(
                m.name.to_ascii_lowercase().into_bytes(),
                MethodInfo {
                    name: m.name.as_bytes().to_vec(),
                    module: NATIVE_MODULE,
                    unit: None,
                    sig_unit: None,
                    visibility: m.visibility,
                    is_static: m.is_static,
                    is_final: false,
                    declaring: id,
                    attributes: Vec::new(),
                    native: Some(m.handler),
                },
            );
        }
        self.build_sel_tables(&mut rt);
        self.classes.push(rt);
        self.static_values.push(Default::default());
        self.class_by_name.insert(lname, id);
        Ok(id)
    }

    /// The nearest native ancestor (or the class itself).
    pub(crate) fn native_info(&self, class: ClassId) -> Option<NativeInfo> {
        let mut cur = Some(class);
        while let Some(c) = cur {
            if let Some(n) = self.class(c).native {
                return Some(n);
            }
            cur = self.class(c).parent;
        }
        None
    }

    // ---- Engine API for native base classes ------------------------------------

    /// Value of `static::$name` as seen from `class` (declared defaults, cached).
    pub fn static_default(&mut self, class: ClassId, name: &str) -> Option<Val> {
        self.static_prop_get(class, name.as_bytes()).ok()
    }

    /// Declared default of an instance property (`$fillable = [...]`).
    pub fn instance_default(&mut self, class: ClassId, name: &str) -> Option<Val> {
        let idx = *self.class(class).prop_index.get(name.as_bytes())?;
        let (init, module, declaring) = {
            let p = &self.class(class).props[idx];
            (p.default.clone()?, p.module, p.declaring)
        };
        self.eval_init(&init, module, Some(declaring)).ok()
    }

    pub fn has_method(&self, class: ClassId, name: &str) -> bool {
        self.class(class).find_method(name.as_bytes()).is_some()
    }

    /// True if `name` is implemented below `base` in the class chain.
    pub fn is_overridden(&self, class: ClassId, name: &str, base: ClassId) -> bool {
        self.class(class).find_method(name.as_bytes()).is_some_and(|m| m.declaring != base && self.instance_of(m.declaring, base) || (m.declaring != base && !self.instance_of(base, m.declaring)))
    }

    pub fn native_payload<T: 'static>(&self, h: ObjHandle) -> Option<&T> {
        self.objects.get(h).native_ref::<T>()
    }

    pub fn native_payload_mut<T: 'static>(&mut self, h: ObjHandle) -> Option<&mut T> {
        self.objects.get_mut(h).native_mut::<T>()
    }

    pub fn class_name(&self, class: ClassId) -> String {
        String::from_utf8_lossy(&self.class(class).name).into_owned()
    }

    pub fn class_of(&self, h: ObjHandle) -> ClassId {
        self.objects.get(h).class
    }

    /// Public wrapper for property reads with full semantics.
    pub fn get_property(&mut self, h: ObjHandle, name: &str) -> Result<Val, Throw> {
        self.read_prop(h, name.as_bytes(), false)
    }

    pub fn set_property(&mut self, h: ObjHandle, name: &str, v: Val) -> Result<(), Throw> {
        self.write_prop(h, name.as_bytes(), crate::bytecode::WriteMode::Assign, v).map(|_| ())
    }

    pub fn throw_class(&self, class: &str, message: impl Into<String>) -> Throw {
        Throw::new(class, message)
    }

    /// Current host context (framework request context), if set.
    pub fn host<T: 'static>(&mut self) -> Option<&mut T> {
        self.host.as_mut()?.downcast_mut::<T>()
    }

    pub fn set_host(&mut self, host: Box<dyn Any>) {
        self.host = Some(host);
    }

    /// Type hint helper for native code reading declared property types.
    pub fn property_type(&self, class: ClassId, name: &str) -> Option<TypeHint> {
        let idx = *self.class(class).prop_index.get(name.as_bytes())?;
        self.class(class).props[idx].ty.clone()
    }
}
