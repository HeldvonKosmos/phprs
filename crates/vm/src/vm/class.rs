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
}

#[derive(Clone, Debug)]
pub struct MethodInfo {
    pub name: Vec<u8>,
    pub module: u32,
    pub unit: Option<UnitId>,
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_final: bool,
    pub declaring: ClassId,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Clone, Debug)]
pub struct ConstInfo {
    pub value: Option<Val>,
    pub init: Init,
    pub module: u32,
    pub visibility: Visibility,
    pub declaring: ClassId,
}

#[derive(Clone, Debug)]
pub struct StaticPropInfo {
    pub declaring: ClassId,
    pub visibility: Visibility,
    pub init: Option<Init>,
    pub module: u32,
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
    pub prop_index: HashMap<Vec<u8>, usize>,
    pub static_props: IndexMap<Vec<u8>, StaticPropInfo>,
    pub methods: HashMap<Vec<u8>, MethodInfo>,
    pub cases: Vec<(Vec<u8>, Option<Init>)>,
    pub enum_backing: Option<TypeHint>,
    pub attributes: Vec<AttributeDecl>,
    pub line: u32,
    /// Property defaults, evaluated on first instantiation.
    pub default_slots: Option<Vec<Option<Val>>>,
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
    /// Register and link all classes and functions of a module.
    pub fn load_module(&mut self, module: Module) -> Result<(), Throw> {
        let module = Rc::new(module);
        let midx = self.modules.len() as u32;
        self.modules.push(module.clone());
        for f in &module.functions {
            let name = module.str(f.name).to_ascii_lowercase();
            self.functions.insert(name, super::FuncRef::User { module: midx, unit: f.unit });
        }
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

    fn link_class(&mut self, module: &Rc<Module>, midx: u32, ci: usize) -> Result<(), Throw> {
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
            prop_index: HashMap::new(),
            static_props: IndexMap::new(),
            methods: HashMap::new(),
            cases: Vec::new(),
            enum_backing: decl.enum_backing.clone(),
            attributes: decl.attributes.clone(),
            line: decl.line,
            default_slots: None,
        };
        // inherit
        if let Some(p) = parent {
            let pc = self.class(p);
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
            self.add_interface(&mut rt, iid);
        }
        // traits: copy members as if declared here
        for t in &decl.traits {
            let tid = self.lookup_class(module.str(*t)).expect("trait linked");
            let tc = self.class(tid);
            let tmod = tc.module;
            for (k, c) in &tc.consts {
                rt.consts.insert(k.clone(), ConstInfo { declaring: id, ..c.clone() });
            }
            for p in tc.props.clone() {
                let mut p = p;
                p.declaring = id;
                p.module = tmod;
                add_prop(&mut rt, p);
            }
            for (k, sp) in &tc.static_props {
                rt.static_props.insert(k.clone(), StaticPropInfo { declaring: id, ..sp.clone() });
            }
            for (k, m) in &tc.methods {
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
            rt.consts.insert(cname, ConstInfo { value: None, init: c.value.clone(), module: midx, visibility: c.visibility, declaring: id });
        }
        for p in &decl.props {
            let pname = module.str(p.name).to_vec();
            if p.is_static {
                rt.static_props.insert(pname, StaticPropInfo { declaring: id, visibility: p.visibility, init: p.default.clone(), module: midx });
            } else {
                add_prop(
                    &mut rt,
                    PropInfo {
                        name: pname,
                        declaring: id,
                        visibility: p.visibility,
                        readonly: p.is_readonly || decl.is_readonly,
                        ty: p.ty.clone(),
                        default: p.default.clone(),
                        module: midx,
                    },
                );
            }
        }
        for m in &decl.methods {
            let mname = module.str(m.name).to_vec();
            let key = mname.to_ascii_lowercase();
            if let Some(existing) = rt.methods.get(&key)
                && existing.is_final
                && existing.declaring != id
            {
                return Err(Throw::error(format!("Cannot override final method {}::{}", String::from_utf8_lossy(&self.class(existing.declaring).name), String::from_utf8_lossy(&mname))));
            }
            rt.methods.insert(
                key,
                MethodInfo {
                    name: mname,
                    module: midx,
                    unit: m.unit,
                    visibility: m.visibility,
                    is_static: m.is_static,
                    is_final: m.is_final,
                    declaring: id,
                    attributes: m.attributes.clone(),
                },
            );
        }
        for c in &decl.cases {
            rt.cases.push((module.str(c.name).to_vec(), c.value.clone()));
        }
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
        self.classes.push(rt);
        self.static_values.push(HashMap::new());
        self.class_by_name.insert(lname, id);
        Ok(())
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

    pub(crate) fn const_to_val(&self, module: u32, c: ConstId) -> Val {
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
        let v = self.eval_init(&info.init, info.module, Some(info.declaring))?;
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
        let h = self.objects.alloc(obj);
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
        let h = self.objects.alloc(obj);
        if self.is_throwable(class) {
            let line = self.current_line();
            let file = self.current_file();
            self.set_prop_raw(h, b"line", Val::Int(line as i64));
            self.set_prop_raw(h, b"file", Val::from(file));
        }
        Ok(h)
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
        let (owner, _) = self.static_prop_owner(class, name).ok_or_else(|| {
            Throw::error(format!("Access to undeclared static property {}::${}", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(name)))
        })?;
        self.static_values[owner.0 as usize].insert(name.to_vec(), v);
        Ok(())
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
