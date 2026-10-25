//! PHP's inheritance checks when a class is linked (zend_inheritance.c):
//! overriding methods (static, visibility, signature compatibility),
//! abstract methods left unimplemented, redeclared properties, final
//! constants and `#[\Override]`. The messages are PHP's; all are Errors at
//! link time (PHP: compile-time fatal errors).

use super::Vm;
use super::class::{MethodInfo, PropInfo, RtClass};
use crate::bytecode::{ClassKind, CodeUnit as Unit, Const, Init, Module, TypeHint, UnitId, Visibility};
use crate::error::Throw;
use crate::value::ClassId;

/// A type with the module and class it was written in (names are module strings,
/// `self`/`static`/`parent` depend on the class).
#[derive(Clone, Copy)]
struct Ctx<'a> {
    module: &'a Module,
    class: Option<ClassId>,
    /// The class being linked (not yet in the class table).
    linking: Option<&'a RtClass>,
}

fn vis_rank(v: Visibility) -> u8 {
    match v {
        Visibility::Public => 0,
        Visibility::Protected => 1,
        Visibility::Private => 2,
    }
}

fn vis_name(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

/// The module being linked: not yet in `Vm::modules` while its classes link.
#[derive(Clone, Copy)]
pub(crate) struct Linking<'a> {
    pub idx: u32,
    pub module: &'a Module,
}

impl Vm {
    fn module_of<'a>(&'a self, idx: u32, cur: Linking<'a>) -> Option<&'a Module> {
        if idx == cur.idx { Some(cur.module) } else { self.modules.get(idx as usize).map(|m| &**m) }
    }

    fn class_name_str(&self, id: ClassId) -> String {
        String::from_utf8_lossy(&self.class(id).name).into_owned()
    }

    /// The signature unit of a method (its body, or the signature of an abstract one).
    fn sig_of<'a>(&'a self, m: &MethodInfo, cur: Linking<'a>) -> Option<(&'a Module, UnitId)> {
        let unit = m.unit.or(m.sig_unit)?;
        Some((self.module_of(m.module, cur)?, unit))
    }

    /// An inherited method `parent` overridden by `child` in class `rt` (PHP's
    /// do_inheritance_check_on_method).
    pub(crate) fn check_method_override(&self, rt: &RtClass, parent: &MethodInfo, child: &MethodInfo, cur: Linking) -> Result<(), Throw> {
        let parent_abstract = parent.unit.is_none() && parent.native.is_none();
        let parent_class = self.class(parent.declaring);
        let from_interface = parent_class.kind == ClassKind::Interface;
        // private methods are not inherited (unless abstract, e.g. from a trait)
        if parent.visibility == Visibility::Private && !parent_abstract {
            return Ok(());
        }
        let pname = format!("{}::{}", self.class_name_str(parent.declaring), String::from_utf8_lossy(&parent.name));
        let cname = String::from_utf8_lossy(&rt.name).into_owned();
        if parent.is_final {
            return Err(Throw::error(format!("Cannot override final method {pname}()")));
        }
        if parent.is_static != child.is_static {
            return Err(Throw::error(if child.is_static {
                format!("Cannot make non static method {pname}() static in class {cname}")
            } else {
                format!("Cannot make static method {pname}() non static in class {cname}")
            }));
        }
        let child_abstract = child.unit.is_none() && child.native.is_none();
        if child_abstract && !parent_abstract {
            return Err(Throw::error(format!("Cannot make non abstract method {pname}() abstract in class {cname}")));
        }
        let is_ctor = child.name.eq_ignore_ascii_case(b"__construct");
        // a constructor may narrow its visibility unless the parent's is abstract
        if vis_rank(child.visibility) > vis_rank(parent.visibility) && (!is_ctor || parent_abstract) {
            return Err(Throw::error(format!(
                "Access level to {cname}::{}() must be {} (as in class {}){}",
                String::from_utf8_lossy(&child.name),
                vis_name(parent.visibility),
                self.class_name_str(parent.declaring),
                if parent.visibility == Visibility::Public { "" } else { " or weaker" }
            )));
        }
        // constructors are free unless the parent's is abstract or from an interface
        if is_ctor && !parent_abstract && !from_interface {
            return Ok(());
        }
        let (Some((pm, pu)), Some((cm, cu))) = (self.sig_of(parent, cur), self.sig_of(child, cur)) else { return Ok(()) };
        let (pu, cu) = (&pm.units[pu as usize], &cm.units[cu as usize]);
        let pctx = Ctx { module: pm, class: Some(parent.declaring), linking: Some(rt) };
        let cctx = Ctx { module: cm, class: Some(rt.id), linking: Some(rt) };
        // methods of internal classes (the base prelude, module 0) have tentative
        // return types in PHP: a different return type is only deprecated
        let tentative = parent.module == 0;
        if !self.signature_compatible(pu, pctx, cu, cctx, tentative) {
            return Err(Throw::error(format!(
                "Declaration of {} must be compatible with {}",
                self.render_signature(&cname, &child.name, cu, cctx),
                self.render_signature(&self.class_name_str(parent.declaring), &parent.name, pu, pctx)
            )));
        }
        Ok(())
    }

    /// zend_do_perform_implementation_check: the child accepts at least what the
    /// parent accepts (contravariant parameters) and returns no more (covariant).
    fn signature_compatible(&self, p: &Unit, pctx: Ctx, c: &Unit, cctx: Ctx, tentative_return: bool) -> bool {
        if c.required_params > p.required_params {
            return false;
        }
        let p_variadic = p.params.last().is_some_and(|x| x.variadic);
        let c_variadic = c.params.last().is_some_and(|x| x.variadic);
        let p_fixed = p.params.len() - p_variadic as usize;
        let c_fixed = c.params.len() - c_variadic as usize;
        if p_fixed > c_fixed && !c_variadic {
            return false;
        }
        if p_variadic && !c_variadic {
            return false;
        }
        let n = p_fixed.max(c_fixed) + (p_variadic || c_variadic) as usize;
        for i in 0..n {
            let pp = if i < p_fixed { Some(&p.params[i]) } else if p_variadic { p.params.last() } else { None };
            let cp = if i < c_fixed { Some(&c.params[i]) } else if c_variadic { c.params.last() } else { None };
            let (Some(pp), Some(cp)) = (pp, cp) else { continue };
            // parameter: the parent's type must fit into the child's
            if let Some(ct) = &cp.ty
                && !self.is_subtype(pp.ty.as_ref().unwrap_or(&TypeHint::Mixed), pctx, ct, cctx)
            {
                return false;
            }
        }
        // return: the child's type must fit into the parent's
        if tentative_return {
            return true;
        }
        match (&p.return_type, &c.return_type) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(pt), Some(ct)) => self.is_subtype(ct, cctx, pt, pctx),
        }
    }

    /// `a` ⊆ `b`: every value of type `a` is also of type `b`.
    fn is_subtype(&self, a: &TypeHint, actx: Ctx, b: &TypeHint, bctx: Ctx) -> bool {
        let mut atoms_a = Vec::new();
        flatten(a, &mut atoms_a);
        let mut atoms_b = Vec::new();
        flatten(b, &mut atoms_b);
        atoms_a.iter().all(|x| atoms_b.iter().any(|y| self.atom_subtype(x, actx, y, bctx)))
    }

    fn atom_subtype(&self, a: &TypeHint, actx: Ctx, b: &TypeHint, bctx: Ctx) -> bool {
        use TypeHint as T;
        // A&B fits where one of its parts fits; a type fits A&B if it fits each part
        if let T::Intersection(parts) = b {
            return parts.iter().all(|p| self.atom_subtype(a, actx, p, bctx));
        }
        if let T::Intersection(parts) = a {
            return parts.iter().any(|p| self.atom_subtype(p, actx, b, bctx));
        }
        match (a, b) {
            (T::Never, _) => true,
            (T::Void, T::Void) => true,
            (T::Void, _) | (_, T::Void) => false,
            (_, T::Mixed) => true,
            (T::Mixed, _) => false,
            (T::True | T::False, T::Bool) => true,
            (T::Array, T::Iterable) => true,
            (T::Static, T::Static) => true,
            (x, y) if std::mem::discriminant(x) == std::mem::discriminant(y) && !matches!(x, T::Class(_) | T::SelfT | T::Parent | T::Static) => true,
            _ => {
                // class-like types: resolve to classes where possible
                let ca = self.resolve_type_class(a, actx);
                match b {
                    T::Object => ca.is_some() || matches!(a, T::Class(_) | T::SelfT | T::Parent | T::Static),
                    T::Iterable => match ca {
                        Some(Ok(c)) => self.well_known.as_ref().is_some_and(|wk| self.subclass(c, wk.traversable, actx.linking)),
                        _ => false,
                    },
                    T::Callable => match ca {
                        Some(Ok(c)) => self.lookup_class(b"Closure").is_some_and(|cl| self.subclass(c, cl, actx.linking)),
                        _ => false,
                    },
                    T::Class(_) | T::SelfT | T::Parent => {
                        let cb = self.resolve_type_class(b, bctx);
                        match (ca, cb) {
                            (Some(Ok(x)), Some(Ok(y))) => self.subclass(x, y, actx.linking),
                            // a class that is not loaded yet: compare the names
                            (Some(Err(x)), Some(Err(y))) => x.eq_ignore_ascii_case(&y),
                            (Some(Err(_)), _) | (_, Some(Err(_))) => true,
                            _ => false,
                        }
                    }
                    _ => false,
                }
            }
        }
    }

    /// `x` is `y` or extends/implements it; the class being linked is not in the
    /// class table yet, so its parent and interfaces answer for it.
    fn subclass(&self, x: ClassId, y: ClassId, linking: Option<&RtClass>) -> bool {
        match linking {
            Some(rt) if x == rt.id => y == rt.id || rt.parent.is_some_and(|p| self.instance_of(p, y)) || rt.interfaces.iter().any(|i| *i == y || self.instance_of(*i, y)),
            Some(rt) if y == rt.id => false,
            _ => self.instance_of(x, y),
        }
    }

    fn ctx_class_name(&self, id: ClassId, ctx: Ctx) -> String {
        match ctx.linking {
            Some(rt) if rt.id == id => String::from_utf8_lossy(&rt.name).into_owned(),
            _ => self.class_name_str(id),
        }
    }

    /// The class of a class-like type: Ok(id), Err(name) when not loaded,
    /// None for other types.
    fn resolve_type_class(&self, t: &TypeHint, ctx: Ctx) -> Option<Result<ClassId, Vec<u8>>> {
        match t {
            TypeHint::Class(n) => {
                let name = ctx.module.str(*n);
                Some(self.lookup_class(name).ok_or_else(|| name.to_vec()))
            }
            TypeHint::SelfT | TypeHint::Static => ctx.class.map(Ok),
            TypeHint::Parent => ctx.class.and_then(|c| match ctx.linking { Some(rt) if rt.id == c => rt.parent, _ => self.class(c).parent }).map(Ok),
            _ => None,
        }
    }

    /// `Class::method(int $a = 1, string ...$rest): ?int` as PHP prints it.
    fn render_signature(&self, class: &str, method: &[u8], u: &Unit, ctx: Ctx) -> String {
        let params: Vec<String> = u
            .params
            .iter()
            .map(|p| {
                let mut s = String::new();
                if let Some(t) = &p.ty {
                    s.push_str(&self.render_type(t, ctx));
                    s.push(' ');
                }
                if p.variadic {
                    s.push_str("...");
                }
                s.push('$');
                s.push_str(&String::from_utf8_lossy(ctx.module.str(p.name)));
                if p.has_default && !p.variadic {
                    s.push_str(" = ");
                    s.push_str(&render_default(p.default_init.as_ref(), ctx.module));
                }
                s
            })
            .collect();
        let mut out = format!("{class}::{}({})", String::from_utf8_lossy(method), params.join(", "));
        if let Some(r) = &u.return_type {
            out.push_str(": ");
            out.push_str(&self.render_type(r, ctx));
        }
        out
    }

    fn render_type(&self, t: &TypeHint, ctx: Ctx) -> String {
        use TypeHint as T;
        match t {
            T::Mixed => "mixed".into(),
            T::Null => "null".into(),
            T::Bool => "bool".into(),
            T::True => "true".into(),
            T::False => "false".into(),
            T::Int => "int".into(),
            T::Float => "float".into(),
            T::String => "string".into(),
            T::Array => "array".into(),
            T::Object => "object".into(),
            T::Iterable => "iterable".into(),
            T::Callable => "callable".into(),
            T::Void => "void".into(),
            T::Never => "never".into(),
            T::Static => "static".into(),
            // PHP prints self and parent as the class they stand for
            T::SelfT => ctx.class.map(|c| self.ctx_class_name(c, ctx)).unwrap_or_else(|| "self".into()),
            T::Parent => match self.resolve_type_class(t, ctx) {
                Some(Ok(c)) => self.ctx_class_name(c, ctx),
                _ => "parent".into(),
            },
            T::Class(n) => String::from_utf8_lossy(ctx.module.str(*n)).into_owned(),
            T::Nullable(inner) => format!("?{}", self.render_type(inner, ctx)),
            T::Union(parts) => parts.iter().map(|p| self.render_type(p, ctx)).collect::<Vec<_>>().join("|"),
            T::Intersection(parts) => parts.iter().map(|p| self.render_type(p, ctx)).collect::<Vec<_>>().join("&"),
        }
    }

    /// Methods of a concrete class that are still abstract (PHP: "Class X
    /// contains N abstract method(s) and must therefore be declared abstract or
    /// implement the remaining methods (A::f, B::g, ...)").
    pub(crate) fn check_abstract_left(&self, rt: &RtClass) -> Result<(), Throw> {
        if rt.is_abstract || !matches!(rt.kind, ClassKind::Class | ClassKind::Enum) {
            return Ok(());
        }
        let mut left: Vec<&MethodInfo> = rt.methods.values().filter(|m| m.unit.is_none() && m.native.is_none()).collect();
        if left.is_empty() {
            return Ok(());
        }
        left.sort_by_key(|m| (m.declaring.0, m.name.clone()));
        let owner = |id: ClassId| if id == rt.id { String::from_utf8_lossy(&rt.name).into_owned() } else { self.class_name_str(id) };
        let names: Vec<String> = left.iter().take(3).map(|m| format!("{}::{}", owner(m.declaring), String::from_utf8_lossy(&m.name))).collect();
        let kind = if rt.kind == ClassKind::Enum { "Enum" } else { "Class" };
        Err(Throw::error(format!(
            "{kind} {} contains {} abstract method{} and must therefore be declared abstract or implement the remaining methods ({}{})",
            String::from_utf8_lossy(&rt.name),
            left.len(),
            if left.len() == 1 { "" } else { "s" },
            names.join(", "),
            if left.len() > 3 { ", ..." } else { "" }
        )))
    }

    /// A property redeclared in a child (PHP's do_inherit_property): visibility
    /// not weaker, readonly the same, type invariant.
    pub(crate) fn check_property_redeclare(&self, rt: &RtClass, parent: &PropInfo, child: &PropInfo, cur: Linking) -> Result<(), Throw> {
        if parent.visibility == Visibility::Private {
            return Ok(());
        }
        let pcls = self.class_name_str(parent.declaring);
        let cname = String::from_utf8_lossy(&rt.name).into_owned();
        let prop = String::from_utf8_lossy(&child.name).into_owned();
        if parent.readonly != child.readonly {
            return Err(Throw::error(if parent.readonly {
                format!("Cannot redeclare readonly property {pcls}::${prop} as non-readonly {cname}::${prop}")
            } else {
                format!("Cannot redeclare non-readonly property {pcls}::${prop} as readonly {cname}::${prop}")
            }));
        }
        if vis_rank(child.visibility) > vis_rank(parent.visibility) {
            return Err(Throw::error(format!(
                "Access level to {cname}::${prop} must be {} (as in class {pcls}){}",
                vis_name(parent.visibility),
                if parent.visibility == Visibility::Public { "" } else { " or weaker" }
            )));
        }
        let (Some(pm), Some(cm)) = (self.module_of(parent.module, cur), self.module_of(child.module, cur)) else { return Ok(()) };
        let pctx = Ctx { module: pm, class: Some(parent.declaring), linking: Some(rt) };
        let cctx = Ctx { module: cm, class: Some(rt.id), linking: Some(rt) };
        match (&parent.ty, &child.ty) {
            (None, None) => Ok(()),
            (None, Some(_)) => Err(Throw::error(format!("Type of {cname}::${prop} must not be defined (as in class {pcls})"))),
            (Some(pt), ct) => {
                let same = ct.as_ref().is_some_and(|ct| self.is_subtype(pt, pctx, ct, cctx) && self.is_subtype(ct, cctx, pt, pctx));
                if same { Ok(()) } else { Err(Throw::error(format!("Type of {cname}::${prop} must be {} (as in class {pcls})", self.render_type(pt, pctx)))) }
            }
        }
    }
}

/// Union members of a type (`?T` is `T|null`).
fn flatten<'a>(t: &'a TypeHint, out: &mut Vec<&'a TypeHint>) {
    match t {
        TypeHint::Nullable(inner) => {
            out.push(&TypeHint::Null);
            flatten(inner, out);
        }
        TypeHint::Union(parts) => parts.iter().for_each(|p| flatten(p, out)),
        other => out.push(other),
    }
}

/// A parameter default as PHP prints it in signatures.
fn render_default(init: Option<&Init>, module: &Module) -> String {
    match init {
        Some(Init::Const(c)) => match &module.consts[*c as usize] {
            Const::Null => "null".into(),
            Const::Bool(b) => if *b { "true" } else { "false" }.into(),
            Const::Int(i) => i.to_string(),
            Const::Float(f) => crate::value::format_float(*f),
            Const::Str(s) => {
                let s = String::from_utf8_lossy(s);
                if s.chars().count() > 10 { format!("'{}...'", s.chars().take(10).collect::<String>()) } else { format!("'{s}'") }
            }
            Const::Array(items) => if items.is_empty() { "[]" } else { "[...]" }.into(),
        },
        _ => "<expression>".into(),
    }
}
