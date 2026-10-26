//! Runtime classes: linking of `ClassDecl`s into `RtClass` (inherited layout,
//! method tables), property lookup with visibility, instantiation.

use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;

use super::Vm;
use crate::bytecode::*;
use crate::error::Throw;
use crate::value::*;

#[derive(Clone, Debug)]
pub struct PropInfo {
    pub name: Vec<u8>,
    pub declaring: ClassId,
    pub visibility: Visibility,
    pub readonly: bool,
    pub ty: Option<TypeHint>,
    pub default: Option<Init>,
    pub module: u32,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Clone, Debug)]
pub struct MethodInfo {
    pub name: Vec<u8>,
    pub module: u32,
    pub unit: Option<UnitId>,
    /// Signature of an abstract method (see `MethodDecl::sig_unit`).
    pub sig_unit: Option<UnitId>,
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_final: bool,
    pub declaring: ClassId,
    pub attributes: Vec<AttributeDecl>,
    /// Native handler (unit is None).
    pub native: Option<super::native::NativeMethodFn>,
}

#[derive(Clone, Debug)]
pub struct ConstInfo {
    pub value: Option<Val>,
    pub init: Init,
    pub module: u32,
    pub visibility: Visibility,
    pub declaring: ClassId,
    pub is_final: bool,
}

#[derive(Clone, Debug)]
pub struct StaticPropInfo {
    pub declaring: ClassId,
    pub visibility: Visibility,
    pub init: Option<Init>,
    pub module: u32,
    pub ty: Option<TypeHint>,
}

#[derive(Debug)]
pub struct RtClass {
    pub id: ClassId,
    pub name: Vec<u8>,
    pub kind: ClassKind,
    pub parent: Option<ClassId>,
    pub interfaces: Vec<ClassId>,
    pub is_abstract: bool,
    pub is_final: bool,
    pub module: u32,
    pub consts: IndexMap<Vec<u8>, ConstInfo>,
    pub props: Vec<PropInfo>,
    pub prop_index: super::FastMap<Vec<u8>, usize>,
    pub static_props: IndexMap<Vec<u8>, StaticPropInfo>,
    pub methods: super::FastMap<Vec<u8>, MethodInfo>,
    pub cases: Vec<(Vec<u8>, Option<Init>)>,
    pub enum_backing: Option<TypeHint>,
    pub attributes: Vec<AttributeDecl>,
    pub line: u32,
    /// Property defaults, evaluated on first instantiation.
    pub default_slots: Option<Vec<Option<Val>>>,
    /// Set for native classes (payload factory, handlers).
    pub native: Option<super::native::NativeInfo>,
    /// Link-step tables: selector → method table index / property slot.
    pub methods_by_sel: super::FastMap<SelId, u32>,
    pub props_by_sel: super::FastMap<SelId, u32>,
}

impl RtClass {
    pub fn find_method(&self, name: &[u8]) -> Option<&MethodInfo> {
        self.methods.get(&name.to_ascii_lowercase())
    }
    pub fn is_enum(&self) -> bool {
        self.kind == ClassKind::Enum
    }
}

pub(crate) enum PropLookup {
    Slot(usize),
    Inaccessible,
    Undeclared,
}

impl Vm {
    /// Register and link all classes and functions of a module, then link
    /// its ops (names → ids) and store it.
    pub fn load_module(&mut self, mut module: Module) -> Result<(), Throw> {
        let midx = self.modules.len() as u32;
        for f in &module.functions {
            let name = module.str(f.name).to_vec();
            self.define_function(&name, super::FuncRef::User { module: midx, unit: f.unit });
        }
        self.load_classes(&module, midx)?;
        self.link_module(&mut module);
        let module = Rc::new(module);
        self.modules.push(module.clone());
        self.ic_base.push(self.ics.len());
        for u in &module.units {
            self.ics.push(vec![super::IcEntry::default(); u.ops.len()]);
            self.profiles.push(super::CodeProfile { types: vec![0; u.ops.len()], jit_no_loop: vec![false; u.ops.len()], ..Default::default() });
            self.jit_funcs.push(super::jit::JitSlotState::Untried);
        }
        Ok(())
    }

    fn load_classes(&mut self, module: &Module, midx: u32) -> Result<(), Throw> {
        // link classes in dependency order
        let mut pending: Vec<usize> = (0..module.classes.len()).collect();
        while !pending.is_empty() {
            let mut progressed = false;
            let mut next = Vec::new();
            for &ci in &pending {
                let decl = &module.classes[ci];
                let deps_ready = decl
                    .parent
                    .iter()
                    .chain(decl.interfaces.iter())
                    .chain(decl.traits.iter())
                    .all(|d| self.lookup_class(module.str(*d)).is_some());
                if deps_ready {
                    self.link_class(&module, midx, ci)?;
                    progressed = true;
                } else {
                    next.push(ci);
                }
            }
            if !progressed {
                let decl = &module.classes[next[0]];
                let missing = decl
                    .parent
                    .iter()
                    .chain(decl.interfaces.iter())
                    .chain(decl.traits.iter())
                    .find(|d| self.lookup_class(module.str(**d)).is_none())
                    .map(|d| module.str_lossy(*d))
                    .unwrap_or_default();
                return Err(Throw::error(format!(
                    "Class \"{}\" not found (needed by {})",
                    missing,
                    module.str_lossy(decl.name)
                )));
            }
            pending = next;
        }
        Ok(())
    }

    fn link_class(&mut self, module: &Module, midx: u32, ci: usize) -> Result<(), Throw> {
        let decl = &module.classes[ci];
        let name = module.str(decl.name).to_vec();
        let lname = name.to_ascii_lowercase();
        if self.class_by_name.contains_key(&lname) {
            return Err(Throw::error(format!("Cannot declare class {}, because the name is already in use", String::from_utf8_lossy(&name))));
        }
        let id = ClassId(self.classes.len() as u32);
        let parent = decl.parent.map(|p| self.lookup_class(module.str(p)).expect("parent linked"));
        let mut rt = RtClass {
            id,
            name: name.clone(),
            kind: decl.kind,
            parent,
            interfaces: Vec::new(),
            is_abstract: decl.is_abstract,
            is_final: decl.is_final,
            module: midx,
            consts: IndexMap::new(),
            props: Vec::new(),
            prop_index: Default::default(),
            static_props: IndexMap::new(),
            methods: Default::default(),
            cases: Vec::new(),
            enum_backing: decl.enum_backing.clone(),
            attributes: decl.attributes.clone(),
            line: decl.line,
            default_slots: None,
            native: None,
            methods_by_sel: Default::default(),
            props_by_sel: Default::default(),
        };
        // inherit
        if let Some(p) = parent {
            let pc = self.class(p);
            match pc.kind {
                ClassKind::Interface => return Err(Throw::error(format!("Class {} cannot extend interface {}", String::from_utf8_lossy(&name), String::from_utf8_lossy(&pc.name)))),
                ClassKind::Trait => return Err(Throw::error(format!("Class {} cannot extend trait {}", String::from_utf8_lossy(&name), String::from_utf8_lossy(&pc.name)))),
                _ => {}
            }
            if pc.is_final {
                return Err(Throw::error(format!("Class {} cannot extend final class {}", String::from_utf8_lossy(&name), String::from_utf8_lossy(&pc.name))));
            }
            rt.interfaces = pc.interfaces.clone();
            rt.consts = pc.consts.clone();
            rt.props = pc.props.clone();
            rt.prop_index = pc.prop_index.clone();
            rt.methods = pc.methods.clone();
        }
        for i in &decl.interfaces {
            let iid = self.lookup_class(module.str(*i)).expect("interface linked");
            if self.class(iid).kind != ClassKind::Interface {
                return Err(Throw::error(format!("{} cannot implement {} - it is not an interface", String::from_utf8_lossy(&name), String::from_utf8_lossy(&self.class(iid).name))));
            }
            self.add_interface(&mut rt, iid);
        }
        if decl.kind == ClassKind::Class
            && let (Some(it), Some(agg)) = (self.lookup_class(b"Iterator"), self.lookup_class(b"IteratorAggregate"))
            && rt.interfaces.contains(&it)
            && rt.interfaces.contains(&agg)
        {
            return Err(Throw::error(format!("Class {} cannot implement both Iterator and IteratorAggregate at the same time", String::from_utf8_lossy(&name))));
        }
        // members taken from traits: name → trait, to report conflicting definitions
        let mut trait_consts: HashMap<Vec<u8>, ClassId> = HashMap::new();
        let mut trait_props: HashMap<Vec<u8>, ClassId> = HashMap::new();
        // traits: copy members as if declared here
        for t in &decl.traits {
            let tid = self.lookup_class(module.str(*t)).expect("trait linked");
            if self.class(tid).kind != ClassKind::Trait {
                return Err(Throw::error(format!("{} cannot use {} - it is not a trait", String::from_utf8_lossy(&name), String::from_utf8_lossy(&self.class(tid).name))));
            }
            let tc = self.class(tid);
            let tmod = tc.module;
            for (k, c) in &tc.consts {
                if let Some(&other) = trait_consts.get(k)
                    && !self.same_const(&rt.consts[k], c, module, midx)
                {
                    return Err(trait_conflict(&self.class(other).name, &tc.name, "constant", k, &name));
                }
                trait_consts.insert(k.clone(), tid);
                rt.consts.insert(k.clone(), ConstInfo { declaring: id, ..c.clone() });
            }
            for p in tc.props.clone() {
                if let Some(&other) = trait_props.get(&p.name)
                    && let Some(&idx) = rt.prop_index.get(&p.name)
                    && !self.same_prop(&rt.props[idx], &p, module, midx)
                {
                    return Err(trait_conflict(&self.class(other).name, &tc.name, "property", &[b"$".as_slice(), &p.name].concat(), &name));
                }
                trait_props.insert(p.name.clone(), tid);
                let mut p = p;
                p.declaring = id;
                p.module = tmod;
                add_prop(&mut rt, p);
            }
            for (k, sp) in &tc.static_props {
                rt.static_props.insert(k.clone(), StaticPropInfo { declaring: id, ..sp.clone() });
            }
            for (k, m) in &tc.methods {
                // an abstract trait method does not replace an inherited implementation
                let abstract_m = m.unit.is_none() && m.native.is_none();
                if abstract_m && rt.methods.get(k).is_some_and(|e| e.unit.is_some() || e.native.is_some()) {
                    continue;
                }
                let mut m = m.clone();
                m.declaring = id;
                rt.methods.insert(k.clone(), m);
            }
            for iid in tc.interfaces.clone() {
                self.add_interface(&mut rt, iid);
            }
        }
        // own members
        for c in &decl.consts {
            let cname = module.str(c.name).to_vec();
            if let Some(&t) = trait_consts.get(&cname) {
                let own = ConstInfo { value: None, init: c.value.clone(), module: midx, visibility: c.visibility, declaring: id, is_final: c.is_final };
                if !self.same_const(&own, &rt.consts[&cname], module, midx) {
                    return Err(trait_conflict(&name, &self.class(t).name, "constant", &cname, &name));
                }
            }
            if let Some(existing) = rt.consts.get(&cname)
                && existing.is_final
                && existing.declaring != id
            {
                let n = String::from_utf8_lossy(&cname);
                return Err(Throw::error(format!("{}::{n} cannot override final constant {}::{n}", String::from_utf8_lossy(&name), String::from_utf8_lossy(&self.class(existing.declaring).name))));
            }
            rt.consts.insert(cname, ConstInfo { value: None, init: c.value.clone(), module: midx, visibility: c.visibility, declaring: id, is_final: c.is_final });
        }
        for p in &decl.props {
            let pname = module.str(p.name).to_vec();
            if p.is_static {
                rt.static_props.insert(pname, StaticPropInfo { declaring: id, visibility: p.visibility, init: p.default.clone(), module: midx, ty: p.ty.clone() });
            } else {
                let info = PropInfo {
                    name: pname.clone(),
                    declaring: id,
                    visibility: p.visibility,
                    readonly: p.is_readonly || decl.is_readonly,
                    ty: p.ty.clone(),
                    default: p.default.clone(),
                    module: midx,
                    attributes: p.attributes.clone(),
                };
                if let Some(&t) = trait_props.get(&pname)
                    && let Some(&idx) = rt.prop_index.get(&pname)
                    && !self.same_prop(&info, &rt.props[idx], module, midx)
                {
                    return Err(trait_conflict(&name, &self.class(t).name, "property", &[b"$".as_slice(), &pname].concat(), &name));
                }
                if let Some(&idx) = rt.prop_index.get(&pname)
                    && rt.props[idx].declaring != id
                {
                    self.check_property_redeclare(&rt, &rt.props[idx], &info, super::inherit::Linking { idx: midx, module })?;
                }
                add_prop(&mut rt, info);
            }
        }
        for m in &decl.methods {
            let mname = module.str(m.name).to_vec();
            let key = mname.to_ascii_lowercase();
            let info = MethodInfo {
                name: mname.clone(),
                module: midx,
                unit: m.unit,
                sig_unit: m.sig_unit,
                visibility: m.visibility,
                is_static: m.is_static,
                is_final: m.is_final,
                declaring: id,
                attributes: m.attributes.clone(),
                native: None,
            };
            // PHP's inheritance checks against the method this one overrides
            let overridden = rt.methods.get(&key).filter(|e| e.declaring != id).cloned();
            if let Some(parent) = &overridden {
                self.check_method_override(&rt, parent, &info, super::inherit::Linking { idx: midx, module })?;
            }
            // implementing an abstract method of a used trait counts as overriding (PHP 8.3)
            let from_trait_abstract = rt.methods.get(&key).is_some_and(|e| e.declaring == id && e.unit.is_none() && e.native.is_none());
            let overrides = from_trait_abstract || overridden.as_ref().is_some_and(|p| p.visibility != Visibility::Private || (p.unit.is_none() && p.native.is_none()));
            // in a trait, #[\Override] is checked where the trait is used (PHP)
            if !overrides && decl.kind != ClassKind::Trait && m.attributes.iter().any(|a| module.str(a.name).eq_ignore_ascii_case(b"Override")) {
                return Err(Throw::error(format!("{}::{}() has #[\\Override] attribute, but no matching parent method exists", String::from_utf8_lossy(&name), String::from_utf8_lossy(&mname))));
            }
            rt.methods.insert(key, info);
        }
        for c in &decl.cases {
            rt.cases.push((module.str(c.name).to_vec(), c.value.clone()));
        }
        self.check_abstract_left(&rt)?;
        if decl.kind == ClassKind::Enum {
            let wk_unit = self.lookup_class(b"UnitEnum");
            let wk_backed = self.lookup_class(b"BackedEnum");
            if let Some(u) = wk_unit {
                self.add_interface(&mut rt, u);
            }
            if decl.enum_backing.is_some()
                && let Some(b) = wk_backed
            {
                self.add_interface(&mut rt, b);
            }
        }
        self.build_sel_tables(&mut rt);
        self.classes.push(rt);
        self.static_values.push(Default::default());
        self.class_by_name.insert(lname, id);
        Ok(())
    }

    /// Fill `methods_by_sel` / `props_by_sel` from the name maps. Selectors
    /// are numbered in a fixed order (methods by name, properties by slot),
    /// not in the maps' per-instance hash order: linked code is then the same
    /// in every VM, which the JIT's process-wide tier-2 cache keys on.
    pub(crate) fn build_sel_tables(&mut self, rt: &mut RtClass) {
        let mut names: Vec<Vec<u8>> = rt.methods.keys().cloned().collect();
        names.sort_unstable();
        for key in names {
            let info = rt.methods[&key].clone();
            let sel = self.method_sel(&key);
            let idx = self.method_table.len() as u32;
            self.method_table.push(info);
            rt.methods_by_sel.insert(sel, idx);
        }
        let mut props: Vec<(Vec<u8>, usize)> = rt.prop_index.iter().map(|(k, v)| (k.clone(), *v)).collect();
        props.sort_unstable_by_key(|p| p.1);
        for (name, slot) in props {
            let sel = self.prop_sel(&name);
            rt.props_by_sel.insert(sel, slot as u32);
        }
    }

    fn add_interface(&self, rt: &mut RtClass, iid: ClassId) {
        if !rt.interfaces.contains(&iid) {
            rt.interfaces.push(iid);
        }
        let ic = self.class(iid);
        for sub in ic.interfaces.clone() {
            if !rt.interfaces.contains(&sub) {
                rt.interfaces.push(sub);
            }
        }
        // interface constants and abstract methods become visible
        for (k, c) in &ic.consts {
            rt.consts.entry(k.clone()).or_insert_with(|| c.clone());
        }
        for (k, m) in &ic.methods {
            rt.methods.entry(k.clone()).or_insert_with(|| m.clone());
        }
    }

    /// Resolve a property for reading/writing from `scope`.
    pub(crate) fn lookup_prop(&self, class: ClassId, name: &[u8], scope: Option<ClassId>) -> PropLookup {
        let c = self.class(class);
        match c.prop_index.get(name) {
            None => PropLookup::Undeclared,
            Some(&idx) => {
                let p = &c.props[idx];
                let ok = match p.visibility {
                    Visibility::Public => true,
                    Visibility::Protected => scope.is_some_and(|s| self.instance_of(s, p.declaring) || self.instance_of(p.declaring, s)),
                    Visibility::Private => scope == Some(p.declaring),
                };
                if ok { PropLookup::Slot(idx) } else { PropLookup::Inaccessible }
            }
        }
    }

    pub(crate) fn method_accessible(&self, m: &MethodInfo, scope: Option<ClassId>) -> bool {
        match m.visibility {
            Visibility::Public => true,
            Visibility::Protected => scope.is_some_and(|s| self.instance_of(s, m.declaring) || self.instance_of(m.declaring, s)),
            Visibility::Private => scope == Some(m.declaring),
        }
    }

    /// Evaluate an `Init` in the context of a class (for `self::` references).
    pub(crate) fn eval_init(&mut self, init: &Init, module: u32, class: Option<ClassId>) -> Result<Val, Throw> {
        match init {
            Init::Const(c) => Ok(self.const_to_val(module, *c)),
            Init::Unit(u) => {
                let m = self.modules[module as usize].clone();
                self.call_unit_sync(m, module, *u, Vec::new(), Vec::new(), None, class, class, Vec::new())
            }
        }
    }

    /// The constant as a value. Strings and arrays are copy-on-write, so each
    /// constant is built once per module and cloned (a reference count) after
    /// that (PERF2); a write to the value copies it as for any shared value.
    pub(crate) fn const_to_val(&mut self, module: u32, c: ConstId) -> Val {
        let (m, i) = (module as usize, c as usize);
        match &self.modules[m].consts[i] {
            Const::Null => return Val::Null,
            Const::Bool(b) => return Val::Bool(*b),
            Const::Int(n) => return Val::Int(*n),
            Const::Float(f) => return Val::Float(*f),
            Const::Str(_) | Const::Array(_) => {}
        }
        if let Some(Some(v)) = self.const_cache.get(m).and_then(|cache| cache.get(i)) {
            return v.clone();
        }
        let v = self.build_const(module, c);
        if self.const_cache.len() <= m {
            self.const_cache.resize_with(m + 1, Vec::new);
        }
        let len = self.modules[m].consts.len().max(i + 1);
        let cache = &mut self.const_cache[m];
        if cache.len() <= i {
            cache.resize(len, None);
        }
        cache[i] = Some(v.clone());
        v
    }

    fn build_const(&self, module: u32, c: ConstId) -> Val {
        fn conv(c: &Const) -> Val {
            match c {
                Const::Null => Val::Null,
                Const::Bool(b) => Val::Bool(*b),
                Const::Int(i) => Val::Int(*i),
                Const::Float(f) => Val::Float(*f),
                Const::Str(s) => Val::bytes(s),
                Const::Array(items) => {
                    let mut a = PhpArray::with_capacity(items.len());
                    for (k, v) in items {
                        let key = match k {
                            ConstKey::Int(i) => ArrayKey::Int(*i),
                            ConstKey::Str(s) => ArrayKey::Str(PhpString::from(s.as_slice())),
                        };
                        a.insert(key, conv(v));
                    }
                    Val::array(a)
                }
            }
        }
        conv(&self.modules[module as usize].consts[c as usize])
    }

    /// Class constant (or enum case) by name, evaluated lazily.
    pub(crate) fn class_constant(&mut self, class: ClassId, name: &[u8]) -> Result<Val, Throw> {
        if self.class(class).is_enum()
            && self.class(class).cases.iter().any(|(n, _)| n == name)
        {
            return Ok(Val::Object(self.enum_case(class, name)?));
        }
        let info = match self.class(class).consts.get(name) {
            Some(i) => i.clone(),
            None => {
                return Err(Throw::error(format!(
                    "Undefined constant {}::{}",
                    String::from_utf8_lossy(&self.class(class).name),
                    String::from_utf8_lossy(name)
                )));
            }
        };
        if let Some(v) = info.value {
            return Ok(v);
        }
        // a constant that needs itself (`A = [...self::B]; B = [...self::A]`) is an
        // Error as in PHP, not an endless recursion that overflows the native stack
        if self.consts_in_progress.iter().any(|(c, n)| *c == class && n == name) {
            let (top_class, top_name) = self.consts_in_progress.last().cloned().expect("in progress");
            let owner = if top_class == class { "self".to_string() } else { String::from_utf8_lossy(&self.class(top_class).name).into_owned() };
            return Err(Throw::error(format!("Cannot declare self-referencing constant {owner}::{}", String::from_utf8_lossy(&top_name))));
        }
        self.consts_in_progress.push((class, name.to_vec()));
        let v = self.eval_init(&info.init, info.module, Some(info.declaring));
        self.consts_in_progress.pop();
        let v = v?;
        if let Some(slot) = self.class_mut(class).consts.get_mut(name) {
            slot.value = Some(v.clone());
        }
        Ok(v)
    }

    /// Singleton object for an enum case (per VM).
    pub(crate) fn enum_case(&mut self, class: ClassId, name: &[u8]) -> Result<ObjHandle, Throw> {
        if let Some(h) = self.enum_cases.get(&(class, name.to_vec())) {
            return Ok(*h);
        }
        let (init, module) = {
            let c = self.class(class);
            let (_, init) = c.cases.iter().find(|(n, _)| n == name).expect("enum case");
            (init.clone(), c.module)
        };
        let value = match init {
            Some(i) => Some(self.eval_init(&i, module, Some(class))?),
            None => None,
        };
        let mut obj = PhpObject::new(class, 0);
        obj.dynamic.insert(PhpString::from("name"), Val::bytes(name));
        if let Some(v) = value {
            obj.dynamic.insert(PhpString::from("value"), v);
        }
        // enum cases are immutable singletons of the engine, not of the request
        let h = self.objects.alloc_engine(obj);
        self.enum_cases.insert((class, name.to_vec()), h);
        Ok(h)
    }

    /// All cases of an enum as objects, in declaration order.
    pub(crate) fn enum_cases_list(&mut self, class: ClassId) -> Result<Vec<ObjHandle>, Throw> {
        let names: Vec<Vec<u8>> = self.class(class).cases.iter().map(|(n, _)| n.clone()).collect();
        let mut out = Vec::new();
        for n in names {
            out.push(self.enum_case(class, &n)?);
        }
        Ok(out)
    }

    /// Allocate an object of `class` with property defaults, without
    /// running the constructor.
    pub(crate) fn alloc_object(&mut self, class: ClassId) -> Result<ObjHandle, Throw> {
        {
            let c = self.class(class);
            if c.is_abstract || c.kind != ClassKind::Class {
                let what = match c.kind {
                    ClassKind::Interface => "interface",
                    ClassKind::Trait => "trait",
                    ClassKind::Enum => "enum",
                    ClassKind::Class => "abstract class",
                };
                return Err(Throw::error(format!("Cannot instantiate {what} {}", String::from_utf8_lossy(&c.name))));
            }
        }
        let defaults = match &self.class(class).default_slots {
            Some(d) => d.clone(),
            None => {
                let props = self.class(class).props.clone();
                let mut slots = Vec::with_capacity(props.len());
                for p in &props {
                    let v = match &p.default {
                        Some(init) => Some(self.eval_init(init, p.module, Some(p.declaring))?),
                        None => None,
                    };
                    slots.push(v);
                }
                self.class_mut(class).default_slots = Some(slots.clone());
                slots
            }
        };
        let mut obj = PhpObject::new(class, defaults.len());
        obj.slots = defaults;
        if let Some(n) = self.native_info(class)
            && let Some(factory) = n.payload
        {
            obj.native = Some(factory());
        }
        // exceptions are exempt: the limit error itself is one
        if !self.is_throwable(class) {
            self.check_object_limit()?;
        }
        let h = self.objects.alloc(obj);
        if self.is_throwable(class) {
            let line = self.current_line();
            let file = self.current_file();
            let trace = self.build_trace();
            self.set_prop_raw(h, b"line", Val::Int(line as i64));
            self.set_prop_raw(h, b"file", Val::from(file));
            self.set_prop_raw(h, b"trace", trace);
        }
        Ok(h)
    }

    /// PHP-shaped backtrace of the live frames: entry k describes the call
    /// into frame k+1 made from frame k (file/line of the call site).
    pub(crate) fn build_trace(&self) -> Val {
        let mut arr = PhpArray::new();
        let n = self.frames.len();
        for k in (1..n).rev() {
            let callee = &self.frames[k];
            let caller = &self.frames[k - 1];
            let mut e = PhpArray::new();
            e.insert(ArrayKey::from_str("file"), Val::from(caller.module.file.clone()));
            e.insert(ArrayKey::from_str("line"), Val::Int(caller.line() as i64));
            let unit_name = callee.module.units[callee.unit as usize].name.clone();
            let function = unit_name.rsplit("::").next().unwrap_or(&unit_name).to_string();
            if let Some(c) = callee.class {
                e.insert(ArrayKey::from_str("class"), Val::from(String::from_utf8_lossy(&self.class(c).name).into_owned()));
                e.insert(ArrayKey::from_str("type"), Val::str(if callee.this.is_some() { "->" } else { "::" }));
            }
            e.insert(ArrayKey::from_str("function"), Val::from(function));
            arr.push(Val::array(e));
        }
        // The outermost frame, when it is a function or method entered from
        // Rust (controller action, callback): no call site, so its own
        // declaration line stands in.
        if let Some(root) = self.frames.first()
            && root.unit != root.module.main
        {
            let unit = &root.module.units[root.unit as usize];
            let mut e = PhpArray::new();
            e.insert(ArrayKey::from_str("file"), Val::from(root.module.file.clone()));
            e.insert(ArrayKey::from_str("line"), Val::Int(unit.line as i64));
            let function = unit.name.rsplit("::").next().unwrap_or(&unit.name).to_string();
            if let Some(c) = root.class {
                e.insert(ArrayKey::from_str("class"), Val::from(String::from_utf8_lossy(&self.class(c).name).into_owned()));
                e.insert(ArrayKey::from_str("type"), Val::str(if root.this.is_some() { "->" } else { "::" }));
            }
            e.insert(ArrayKey::from_str("function"), Val::from(function));
            arr.push(Val::array(e));
        }
        Val::array(arr)
    }

    /// Set a declared property without visibility checks.
    pub(crate) fn set_prop_raw(&mut self, h: ObjHandle, name: &[u8], v: Val) {
        let class = self.objects.get(h).class;
        match self.class(class).prop_index.get(name) {
            Some(&idx) => self.objects.get_mut(h).slots[idx] = Some(v),
            None => {
                self.objects.get_mut(h).dynamic.insert(PhpString::from(name), v);
            }
        }
    }

    pub(crate) fn get_prop_raw(&self, h: ObjHandle, name: &[u8]) -> Option<Val> {
        let obj = self.objects.get(h);
        match self.class(obj.class).prop_index.get(name) {
            Some(&idx) => obj.slots[idx].clone(),
            None => obj.dynamic.get(&PhpString::from(name)).cloned(),
        }
    }

    /// Static property storage lookup: walks the parent chain to the declaring class.
    pub(crate) fn static_prop_owner(&self, class: ClassId, name: &[u8]) -> Option<(ClassId, StaticPropInfo)> {
        let mut cur = Some(class);
        while let Some(c) = cur {
            if let Some(info) = self.class(c).static_props.get(name) {
                return Some((c, info.clone()));
            }
            cur = self.class(c).parent;
        }
        None
    }

    pub(crate) fn static_prop_get(&mut self, class: ClassId, name: &[u8]) -> Result<Val, Throw> {
        let (owner, info) = self.static_prop_owner(class, name).ok_or_else(|| {
            Throw::error(format!("Access to undeclared static property {}::${}", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(name)))
        })?;
        if let Some(v) = self.static_values[owner.0 as usize].get(name) {
            return Ok(v.clone());
        }
        let v = match &info.init {
            Some(init) => self.eval_init(init, info.module, Some(owner))?,
            None => Val::Null,
        };
        self.static_values[owner.0 as usize].insert(name.to_vec(), v.clone());
        Ok(v)
    }

    pub(crate) fn static_prop_set(&mut self, class: ClassId, name: &[u8], v: Val) -> Result<(), Throw> {
        let (owner, info) = self.static_prop_owner(class, name).ok_or_else(|| {
            Throw::error(format!("Access to undeclared static property {}::${}", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(name)))
        })?;
        // DIALECT §5.4: statics outlive the request, objects do not (enum cases are engine singletons)
        if let Some(h) = first_request_object(&v) {
            return Err(Throw::error(format!(
                "Cannot store an object of class {} in static property {}::${}: static properties outlive the request; only enum cases are allowed",
                String::from_utf8_lossy(&self.class(self.objects.get(h).class).name),
                String::from_utf8_lossy(&self.class(class).name),
                String::from_utf8_lossy(name)
            )));
        }
        // typed static properties are checked like instance properties (strict, int -> float)
        let v = match &info.ty {
            Some(t) if !super::path::scalar_type_matches(t, &v) => {
                let module = self.modules[info.module as usize].clone();
                match self.check_type(&v, t, &module, Some(owner)) {
                    Some(v) => v,
                    None => {
                        return Err(Throw::type_error(format!(
                            "Cannot assign {} to property {}::${} of type {}",
                            self.value_type_name(&v),
                            String::from_utf8_lossy(&self.class(owner).name),
                            String::from_utf8_lossy(name),
                            self.type_name_of(t, &module)
                        )));
                    }
                }
            }
            _ => v,
        };
        self.static_values[owner.0 as usize].insert(name.to_vec(), v);
        Ok(())
    }
}

/// The first request-bound object inside `v` (arrays searched recursively).
fn first_request_object(v: &Val) -> Option<ObjHandle> {
    match v {
        Val::Object(h) if !h.is_engine() => Some(*h),
        Val::Array(a) => a.values().find_map(first_request_object),
        _ => None,
    }
}

fn add_prop(rt: &mut RtClass, p: PropInfo) {
    match rt.prop_index.get(&p.name) {
        Some(&idx) => rt.props[idx] = p,
        None => {
            rt.prop_index.insert(p.name.clone(), rt.props.len());
            rt.props.push(p);
        }
    }
}

/// PHP's error for a trait member that clashes with the class or another trait.
fn trait_conflict(first: &[u8], trait_name: &[u8], what: &str, member: &[u8], class: &[u8]) -> Throw {
    let (f, t, m, c) = (String::from_utf8_lossy(first), String::from_utf8_lossy(trait_name), String::from_utf8_lossy(member), String::from_utf8_lossy(class));
    Throw::error(format!("{f} and {t} define the same {what} ({m}) in the composition of {c}. However, the definition differs and is considered incompatible. Class was composed"))
}

impl Vm {
    /// The literal value of an initialiser, when it is one (`None` for expressions).
    fn init_literal(&self, init: &Init, module: u32, cur: &Module, cur_idx: u32) -> Option<Const> {
        let Init::Const(c) = init else { return None };
        let m: &Module = if module == cur_idx { cur } else { self.modules.get(module as usize)? };
        m.consts.get(*c as usize).cloned()
    }

    /// Two constant definitions are compatible: same visibility, finality and value.
    fn same_const(&self, a: &ConstInfo, b: &ConstInfo, cur: &Module, cur_idx: u32) -> bool {
        if a.visibility != b.visibility || a.is_final != b.is_final {
            return false;
        }
        match (self.init_literal(&a.init, a.module, cur, cur_idx), self.init_literal(&b.init, b.module, cur, cur_idx)) {
            (Some(x), Some(y)) => x == y,
            _ => true,
        }
    }

    /// Two property definitions are compatible: same visibility, readonly, type and default.
    fn same_prop(&self, a: &PropInfo, b: &PropInfo, cur: &Module, cur_idx: u32) -> bool {
        if a.visibility != b.visibility || a.readonly != b.readonly {
            return false;
        }
        let ty = |p: &PropInfo| -> Option<String> {
            let m: &Module = if p.module == cur_idx { cur } else { self.modules.get(p.module as usize)? };
            p.ty.as_ref().map(|t| type_key(t, m))
        };
        if ty(a) != ty(b) {
            return false;
        }
        match (&a.default, &b.default) {
            (None, None) => true,
            (Some(x), Some(y)) => match (self.init_literal(x, a.module, cur, cur_idx), self.init_literal(y, b.module, cur, cur_idx)) {
                (Some(x), Some(y)) => x == y,
                _ => true,
            },
            _ => false,
        }
    }
}

/// A type as text with class names resolved (to compare types of different modules).
fn type_key(t: &TypeHint, m: &Module) -> String {
    match t {
        TypeHint::Class(n) => String::from_utf8_lossy(m.str(*n)).to_ascii_lowercase(),
        TypeHint::Nullable(i) => format!("?{}", type_key(i, m)),
        TypeHint::Union(p) => {
            let mut v: Vec<String> = p.iter().map(|x| type_key(x, m)).collect();
            v.sort();
            v.join("|")
        }
        TypeHint::Intersection(p) => {
            let mut v: Vec<String> = p.iter().map(|x| type_key(x, m)).collect();
            v.sort();
            v.join("&")
        }
        other => format!("{other:?}"),
    }
}

