//! Classes, interfaces, traits, enums.

use mago_span::HasSpan;
use mago_syntax::cst::*;

use super::dialect;
use super::expr::var_name;
use super::stmt::{FnBody, FnSpec};
use super::{CResult, ClassCtx, Compiler};
use crate::bytecode::*;

struct Members {
    consts: Vec<ClassConstDecl>,
    props: Vec<PropDecl>,
    methods: Vec<MethodDecl>,
    cases: Vec<EnumCaseDecl>,
    traits: Vec<StrId>,
}

impl Compiler {
    pub(crate) fn compile_class(&mut self, c: &Class<'_>) -> CResult<()> {
        if reserved_class_name(c.name.value) {
            return self.err(c.name.span(), format!("Cannot use \"{}\" as a class name as it is reserved", String::from_utf8_lossy(c.name.value)));
        }
        let fq = self.names.declare(c.name.value);
        let parent = match &c.extends {
            Some(e) => {
                let first = e.types.first().ok_or(()).or_else(|_| self.err(e.span(), "`extends` ohne Klasse"))?;
                Some(self.names.resolve_class(first.value()))
            }
            None => None,
        };
        let mut is_abstract = false;
        let mut is_final = false;
        let mut is_readonly = false;
        for m in c.modifiers.iter() {
            let twice = |what: &str| format!("Multiple {what} modifiers are not allowed");
            match m {
                Modifier::Abstract(_) if is_abstract => return self.err(m.span(), twice("abstract")),
                Modifier::Final(_) if is_final => return self.err(m.span(), twice("final")),
                Modifier::Readonly(_) if is_readonly => return self.err(m.span(), twice("readonly")),
                Modifier::Abstract(_) => is_abstract = true,
                Modifier::Final(_) => is_final = true,
                Modifier::Readonly(_) => is_readonly = true,
                other => return self.err(other.span(), "invalid class modifier"),
            }
        }
        if is_abstract && is_final {
            return self.err(c.span(), "Cannot use the final modifier on an abstract class");
        }
        let interfaces = self.interface_names(c.implements.as_ref())?;
        let attributes = self.compile_attributes(&c.attribute_lists)?;
        let line = self.line(c.span());
        self.class_stack.push(ClassCtx { name: fq.clone(), parent: parent.clone(), kind: ClassKind::Class });
        let members = self.compile_members(&c.members);
        self.class_stack.pop();
        let members = members?;
        if is_readonly
            && let Some(p) = members.props.iter().find(|p| p.ty.is_none() && !p.is_static)
        {
            return self.err(c.span(), format!("Readonly property {}::${} must have type", String::from_utf8_lossy(&fq), String::from_utf8_lossy(self.module.str(p.name))));
        }
        let name = self.intern(&fq);
        let parent = parent.map(|p| self.intern(&p));
        self.module.classes.push(ClassDecl {
            name,
            kind: ClassKind::Class,
            parent,
            interfaces,
            traits: members.traits,
            is_abstract,
            is_final,
            is_readonly,
            consts: members.consts,
            props: members.props,
            methods: members.methods,
            cases: members.cases,
            enum_backing: None,
            attributes,
            line,
        });
        Ok(())
    }

    pub(crate) fn compile_anonymous_class(&mut self, c: &AnonymousClass<'_>) -> CResult<()> {
        let fq = self.next_anon_class_name();
        let parent = match &c.extends {
            Some(e) => {
                let first = e.types.first().ok_or(()).or_else(|_| self.err(e.span(), "`extends` ohne Klasse"))?;
                Some(self.names.resolve_class(first.value()))
            }
            None => None,
        };
        let interfaces = self.interface_names(c.implements.as_ref())?;
        let attributes = self.compile_attributes(&c.attribute_lists)?;
        let line = self.line(c.span());
        self.class_stack.push(ClassCtx { name: fq.clone(), parent: parent.clone(), kind: ClassKind::Class });
        let members = self.compile_members(&c.members);
        self.class_stack.pop();
        let members = members?;
        let name = self.intern(&fq);
        let parent = parent.map(|p| self.intern(&p));
        self.module.classes.push(ClassDecl {
            name,
            kind: ClassKind::Class,
            parent,
            interfaces,
            traits: members.traits,
            is_abstract: false,
            is_final: true,
            is_readonly: false,
            consts: members.consts,
            props: members.props,
            methods: members.methods,
            cases: members.cases,
            enum_backing: None,
            attributes,
            line,
        });
        // instantiate: arguments may only be positional/named here
        let mut spec = ArgSpec::default();
        if let Some(al) = &c.argument_list {
            for a in al.arguments.iter() {
                match a {
                    PartialArgument::Positional(p) => {
                        self.compile_expr(p.value)?;
                        spec.names.push(None);
                        spec.spread.push(p.ellipsis.is_some());
                    }
                    PartialArgument::Named(n) => {
                        self.compile_expr(n.value)?;
                        let nm = self.intern(n.name.value);
                        spec.names.push(Some(nm));
                        spec.spread.push(false);
                    }
                    other => return self.err(other.span(), "invalid argument"),
                }
            }
        }
        let spec = self.ub().arg_spec(spec);
        self.emit(Op::New { class: ClassRef::Named(name), spec });
        Ok(())
    }

    pub(crate) fn compile_interface(&mut self, i: &Interface<'_>) -> CResult<()> {
        if reserved_class_name(i.name.value) {
            return self.err(i.name.span(), format!("Cannot use \"{}\" as an interface name as it is reserved", String::from_utf8_lossy(i.name.value)));
        }
        let fq = self.names.declare(i.name.value);
        let mut interfaces = Vec::new();
        if let Some(e) = &i.extends {
            for t in e.types.iter() {
                let r = self.names.resolve_class(t.value());
                interfaces.push(self.intern(&r));
            }
        }
        let attributes = self.compile_attributes(&i.attribute_lists)?;
        let line = self.line(i.span());
        self.class_stack.push(ClassCtx { name: fq.clone(), parent: None, kind: ClassKind::Interface });
        let members = self.compile_members(&i.members);
        self.class_stack.pop();
        let members = members?;
        let name = self.intern(&fq);
        self.module.classes.push(ClassDecl {
            name,
            kind: ClassKind::Interface,
            parent: None,
            interfaces,
            traits: Vec::new(),
            is_abstract: true,
            is_final: false,
            is_readonly: false,
            consts: members.consts,
            props: Vec::new(),
            methods: members.methods,
            cases: Vec::new(),
            enum_backing: None,
            attributes,
            line,
        });
        Ok(())
    }

    pub(crate) fn compile_trait(&mut self, t: &Trait<'_>) -> CResult<()> {
        if reserved_class_name(t.name.value) {
            return self.err(t.name.span(), format!("Cannot use \"{}\" as a trait name as it is reserved", String::from_utf8_lossy(t.name.value)));
        }
        let fq = self.names.declare(t.name.value);
        let attributes = self.compile_attributes(&t.attribute_lists)?;
        let line = self.line(t.span());
        self.class_stack.push(ClassCtx { name: fq.clone(), parent: None, kind: ClassKind::Trait });
        let members = self.compile_members(&t.members);
        self.class_stack.pop();
        let members = members?;
        let name = self.intern(&fq);
        self.module.classes.push(ClassDecl {
            name,
            kind: ClassKind::Trait,
            parent: None,
            interfaces: Vec::new(),
            traits: members.traits,
            is_abstract: false,
            is_final: false,
            is_readonly: false,
            consts: members.consts,
            props: members.props,
            methods: members.methods,
            cases: Vec::new(),
            enum_backing: None,
            attributes,
            line,
        });
        Ok(())
    }

    pub(crate) fn compile_enum(&mut self, e: &Enum<'_>) -> CResult<()> {
        if reserved_class_name(e.name.value) {
            return self.err(e.name.span(), format!("Cannot use \"{}\" as an enum name as it is reserved", String::from_utf8_lossy(e.name.value)));
        }
        let fq = self.names.declare(e.name.value);
        let interfaces = self.interface_names(e.implements.as_ref())?;
        let backing = match &e.backing_type_hint {
            Some(b) => Some(self.type_hint(&b.hint)?),
            None => None,
        };
        let attributes = self.compile_attributes(&e.attribute_lists)?;
        let line = self.line(e.span());
        self.class_stack.push(ClassCtx { name: fq.clone(), parent: None, kind: ClassKind::Enum });
        let members = self.compile_members(&e.members);
        self.class_stack.pop();
        let members = members?;
        let ename = String::from_utf8_lossy(&fq).into_owned();
        if !members.props.is_empty() {
            return self.err(e.span(), format!("Enum {ename} cannot include properties"));
        }
        for c in &members.cases {
            let cname = String::from_utf8_lossy(self.module.str(c.name)).into_owned();
            match (&backing, &c.value) {
                (None, Some(_)) => return self.err(e.span(), format!("Case {cname} of non-backed enum {ename} must not have a value")),
                (Some(_), None) => return self.err(e.span(), format!("Case {cname} of backed enum {ename} must have a value")),
                _ => {}
            }
        }
        let name = self.intern(&fq);
        self.module.classes.push(ClassDecl {
            name,
            kind: ClassKind::Enum,
            parent: None,
            interfaces,
            traits: members.traits,
            is_abstract: false,
            is_final: true,
            is_readonly: false,
            consts: members.consts,
            props: Vec::new(),
            methods: members.methods,
            cases: members.cases,
            enum_backing: backing,
            attributes,
            line,
        });
        Ok(())
    }

    fn interface_names(&mut self, implements: Option<&Implements<'_>>) -> CResult<Vec<StrId>> {
        let mut out = Vec::new();
        if let Some(i) = implements {
            for t in i.types.iter() {
                let r = self.names.resolve_class(t.value());
                out.push(self.intern(&r));
            }
        }
        Ok(out)
    }

    fn compile_members(&mut self, members: &Sequence<'_, ClassLikeMember<'_>>) -> CResult<Members> {
        let mut out = Members { consts: Vec::new(), props: Vec::new(), methods: Vec::new(), cases: Vec::new(), traits: Vec::new() };
        for m in members.iter() {
            match m {
                ClassLikeMember::TraitUse(tu) => {
                    if let TraitUseSpecification::Concrete(c) = &tu.specification
                        && !c.adaptations.is_empty()
                    {
                        return self.err(c.span(), "trait adaptations (`insteadof`, `as`) are not supported");
                    }
                    for t in tu.trait_names.iter() {
                        let r = self.names.resolve_class(t.value());
                        out.traits.push(self.intern(&r));
                    }
                }
                ClassLikeMember::Constant(c) => {
                    let (visibility, is_final, _, _) = self.member_modifiers(&c.modifiers)?;
                    for item in c.items.iter() {
                        let value = self.init_expr(item.value)?;
                        let name = self.intern(item.name.value);
                        out.consts.push(ClassConstDecl { name, value, visibility, is_final });
                    }
                }
                ClassLikeMember::Property(Property::Plain(p)) => {
                    let (visibility, _, is_static, is_readonly) = self.member_modifiers(&p.modifiers)?;
                    let ty = match &p.hint {
                        Some(h) => Some(self.type_hint(h)?),
                        None => None,
                    };
                    let attributes = self.compile_attributes(&p.attribute_lists)?;
                    for item in p.items.iter() {
                        let (var, default) = match item {
                            PropertyItem::Abstract(a) => (&a.variable, None),
                            PropertyItem::Concrete(c) => (&c.variable, Some(self.init_expr(c.value)?)),
                        };
                        let name = self.intern(var_name(var.name));
                        // untyped properties without default are null; typed ones stay uninitialised
                        let default = match (default, &ty) {
                            (None, None) => Some(Init::Const(self.constant(Const::Null))),
                            (d, _) => d,
                        };
                        out.props.push(PropDecl {
                            name,
                            default,
                            visibility,
                            is_static,
                            is_readonly,
                            ty: ty.clone(),
                            attributes: attributes.clone(),
                        });
                    }
                }
                ClassLikeMember::Property(Property::Hooked(h)) => {
                    return self.err(h.span(), "property hooks are not supported");
                }
                ClassLikeMember::EnumCase(c) => {
                    let (name, value) = match &c.item {
                        EnumCaseItem::Unit(u) => (self.intern(u.name.value), None),
                        EnumCaseItem::Backed(b) => {
                            let v = self.init_expr(b.value)?;
                            (self.intern(b.name.value), Some(v))
                        }
                    };
                    out.cases.push(EnumCaseDecl { name, value });
                }
                ClassLikeMember::Method(m) => {
                    let decl = self.compile_method(m)?;
                    // promoted constructor parameters become properties
                    if m.name.value.eq_ignore_ascii_case(b"__construct")
                        && let Some(unit) = decl.unit
                    {
                        let params = self.module.units[unit as usize].params.clone();
                        for p in params {
                            if let Some(vis) = p.promote {
                                out.props.push(PropDecl {
                                    name: p.name,
                                    default: None,
                                    visibility: vis,
                                    is_static: false,
                                    is_readonly: p.promote_readonly,
                                    ty: p.ty.clone(),
                                    attributes: p.attributes.clone(),
                                });
                            }
                        }
                    }
                    out.methods.push(decl);
                }
            }
        }
        self.check_member_declarations(&out, members)?;
        Ok(out)
    }

    /// PHP's compile errors for members declared twice and untyped readonly properties.
    fn check_member_declarations(&self, out: &Members, members: &Sequence<'_, ClassLikeMember<'_>>) -> CResult<()> {
        let Some(first) = members.iter().next() else { return Ok(()) };
        let span = first.span();
        let class = String::from_utf8_lossy(&self.current_class().map(|c| c.name.clone()).unwrap_or_default()).into_owned();
        let mut seen: std::collections::HashSet<Vec<u8>> = std::collections::HashSet::new();
        for m in &out.methods {
            let n = self.module.str(m.name);
            if !seen.insert(n.to_ascii_lowercase()) {
                return self.err(span, format!("Cannot redeclare {class}::{}()", String::from_utf8_lossy(n)));
            }
        }
        seen.clear();
        for p in &out.props {
            let n = self.module.str(p.name);
            if !seen.insert(n.to_vec()) {
                return self.err(span, format!("Cannot redeclare {class}::${}", String::from_utf8_lossy(n)));
            }
            if p.is_readonly && p.ty.is_none() {
                return self.err(span, format!("Readonly property {class}::${} must have type", String::from_utf8_lossy(n)));
            }
        }
        seen.clear();
        for c in &out.consts {
            let n = self.module.str(c.name);
            if !seen.insert(n.to_vec()) {
                return self.err(span, format!("Cannot redefine class constant {class}::{}", String::from_utf8_lossy(n)));
            }
        }
        Ok(())
    }

    /// (visibility, final, static, readonly)
    fn member_modifiers(&mut self, mods: &Sequence<'_, Modifier<'_>>) -> CResult<(Visibility, bool, bool, bool)> {
        let mut vis = Visibility::Public;
        let mut is_final = false;
        let mut is_static = false;
        let mut is_readonly = false;
        let (mut n_vis, mut n_abstract) = (0, 0);
        for m in mods.iter() {
            // a modifier given twice is a compile error in PHP
            let twice = |what: &str| format!("Multiple {what} modifiers are not allowed");
            match m {
                Modifier::Public(_) | Modifier::Protected(_) | Modifier::Private(_) => {
                    n_vis += 1;
                    if n_vis > 1 {
                        return self.err(m.span(), "Multiple access type modifiers are not allowed");
                    }
                    vis = match m {
                        Modifier::Public(_) => Visibility::Public,
                        Modifier::Protected(_) => Visibility::Protected,
                        _ => Visibility::Private,
                    };
                }
                Modifier::Final(_) if is_final => return self.err(m.span(), twice("final")),
                Modifier::Final(_) => is_final = true,
                Modifier::Static(_) if is_static => return self.err(m.span(), twice("static")),
                Modifier::Static(_) => is_static = true,
                Modifier::Readonly(_) if is_readonly => return self.err(m.span(), twice("readonly")),
                Modifier::Readonly(_) => is_readonly = true,
                Modifier::Abstract(_) => {
                    n_abstract += 1;
                    if n_abstract > 1 {
                        return self.err(m.span(), twice("abstract"));
                    }
                }
                other => return self.err(other.span(), "asymmetric visibility is not supported"),
            }
        }
        Ok((vis, is_final, is_static, is_readonly))
    }

    fn compile_method(&mut self, m: &Method<'_>) -> CResult<MethodDecl> {
        if let Some(a) = m.ampersand {
            return self.reject(dialect::REFERENCE, a);
        }
        let (visibility, is_final, is_static, _) = self.member_modifiers(&m.modifiers)?;
        let is_abstract = m.modifiers.iter().any(|x| matches!(x, Modifier::Abstract(_)));
        let class_name = self.current_class().map(|c| c.name.clone()).unwrap_or_default();
        let display = format!("{}::{}", String::from_utf8_lossy(&class_name), String::from_utf8_lossy(m.name.value));
        let attributes = self.compile_attributes(&m.attribute_lists)?;
        let name = self.intern(m.name.value);
        let line = self.line(m.span());
        let mut sig_unit = None;
        let unit = match &m.body {
            MethodBody::Abstract(_) => {
                let _ = is_abstract;
                let spec = FnSpec {
                    name: m.name.value.to_vec(),
                    display,
                    params: &m.parameter_list,
                    ret: m.return_type_hint.as_ref(),
                    body: FnBody::Signature,
                    is_method: true,
                    is_closure: false,
                    uses: Vec::new(),
                    line,
                };
                sig_unit = Some(self.compile_function_like(spec)?);
                None
            }
            MethodBody::Concrete(block) => {
                let spec = FnSpec {
                    name: m.name.value.to_vec(),
                    display,
                    params: &m.parameter_list,
                    ret: m.return_type_hint.as_ref(),
                    body: FnBody::Block(block),
                    is_method: true,
                    is_closure: false,
                    uses: Vec::new(),
                    line,
                };
                Some(self.compile_function_like(spec)?)
            }
        };
        if let Some(u) = unit.or(sig_unit) {
            self.check_magic_method(&class_name, m.name.value, u, is_static, m.span())?;
        }
        Ok(MethodDecl { name, unit, sig_unit, visibility, is_static, is_final, attributes })
    }

    /// PHP's checks of magic method signatures (zend_check_magic_method_implementation):
    /// argument count, static or not, declared argument and return types.
    fn check_magic_method(&self, class: &[u8], method: &[u8], unit: UnitId, is_static: bool, span: mago_span::Span) -> CResult<()> {
        if !method.starts_with(b"__") {
            return Ok(());
        }
        let lc = method.to_ascii_lowercase();
        // (arguments or -1 for any, static: Some(required), arg types, return type)
        let (args, must_static, arg_types, ret): (i32, Option<bool>, &[(usize, MagicType)], Option<MagicType>) = match lc.as_slice() {
            b"__construct" => (-1, Some(false), &[], None),
            b"__destruct" => (0, Some(false), &[], None),
            b"__clone" => (0, Some(false), &[], Some(MagicType::Void)),
            b"__get" => (1, Some(false), &[(0, MagicType::String)], None),
            b"__set" => (2, Some(false), &[(0, MagicType::String)], Some(MagicType::Void)),
            b"__unset" => (1, Some(false), &[(0, MagicType::String)], Some(MagicType::Void)),
            b"__isset" => (1, Some(false), &[(0, MagicType::String)], Some(MagicType::Bool)),
            b"__call" => (2, Some(false), &[(0, MagicType::String), (1, MagicType::Array)], None),
            b"__callstatic" => (2, Some(true), &[(0, MagicType::String), (1, MagicType::Array)], None),
            b"__tostring" => (0, Some(false), &[], Some(MagicType::String)),
            b"__debuginfo" => (0, Some(false), &[], Some(MagicType::NullableArray)),
            b"__serialize" => (0, Some(false), &[], Some(MagicType::Array)),
            b"__unserialize" => (1, Some(false), &[(0, MagicType::Array)], Some(MagicType::Void)),
            b"__set_state" => (1, Some(true), &[(0, MagicType::Array)], Some(MagicType::Object)),
            b"__invoke" => (-1, Some(false), &[], None),
            b"__sleep" => (0, Some(false), &[], Some(MagicType::Array)),
            b"__wakeup" => (0, Some(false), &[], Some(MagicType::Void)),
            _ => return Ok(()),
        };
        let (c, m) = (String::from_utf8_lossy(class), String::from_utf8_lossy(method));
        let u = &self.module.units[unit as usize];
        if args >= 0 && u.params.len() as i32 != args {
            return self.err(span, match args {
                0 => format!("Method {c}::{m}() cannot take arguments"),
                1 => format!("Method {c}::{m}() must take exactly 1 argument"),
                n => format!("Method {c}::{m}() must take exactly {n} arguments"),
            });
        }
        match must_static {
            Some(false) if is_static => return self.err(span, format!("Method {c}::{m}() cannot be static")),
            Some(true) if !is_static => return self.err(span, format!("Method {c}::{m}() must be static")),
            _ => {}
        }
        for (i, want) in arg_types {
            if let Some(p) = u.params.get(*i)
                && let Some(t) = &p.ty
                && !want.admits_any(t)
            {
                let pname = String::from_utf8_lossy(self.module.str(p.name));
                return self.err(span, format!("{c}::{m}(): Parameter #{} (${pname}) must be of type {} when declared", i + 1, want.name()));
            }
        }
        if matches!(lc.as_slice(), b"__construct" | b"__destruct") && u.return_type.is_some() {
            return self.err(span, format!("Method {c}::{m}() cannot declare a return type"));
        }
        if let (Some(want), Some(t)) = (ret, &u.return_type)
            && !matches!(t, TypeHint::Never)
            && !want.contains_all(t)
        {
            return self.err(span, format!("{c}::{m}(): Return type must be {} when declared", want.name()));
        }
        Ok(())
    }

    /// Constant expression → `Init`: folded when it is made of literals,
    /// otherwise a tiny unit evaluated at link time.
    pub(crate) fn init_expr(&mut self, e: &Expression<'_>) -> CResult<Init> {
        if let Some(c) = self.const_fold(e) {
            return Ok(Init::Const(self.constant(c)));
        }
        let line = self.line(e.span());
        self.push_unit("{init}".to_string(), line);
        self.compile_expr(e)?;
        self.emit(Op::Return);
        Ok(Init::Unit(self.pop_unit()))
    }

    pub(crate) fn const_fold(&mut self, e: &Expression<'_>) -> Option<Const> {
        match e {
            Expression::Parenthesized(p) => self.const_fold(p.expression),
            Expression::Literal(l) => match l {
                Literal::String(s) => Some(Const::Str(s.value.map(|v| v.to_vec()).unwrap_or_else(|| s.raw.to_vec()))),
                Literal::Integer(i) => i.value.filter(|v| *v <= i64::MAX as u64).map(|v| Const::Int(v as i64)),
                Literal::Float(f) => Some(Const::Float(f.value.0)),
                Literal::True(_) => Some(Const::Bool(true)),
                Literal::False(_) => Some(Const::Bool(false)),
                Literal::Null(_) => Some(Const::Null),
            },
            Expression::UnaryPrefix(u) => match (&u.operator, self.const_fold(u.operand)?) {
                (UnaryPrefixOperator::Negation(_), Const::Int(i)) => Some(Const::Int(i.checked_neg()?)),
                (UnaryPrefixOperator::Negation(_), Const::Float(f)) => Some(Const::Float(-f)),
                (UnaryPrefixOperator::Plus(_), c @ (Const::Int(_) | Const::Float(_))) => Some(c),
                _ => None,
            },
            Expression::Array(a) => {
                let mut items = Vec::new();
                let mut next = 0i64;
                for el in a.elements.iter() {
                    match el {
                        ArrayElement::Value(v) => {
                            let c = self.const_fold(v.value)?;
                            items.push((ConstKey::Int(next), c));
                            next += 1;
                        }
                        ArrayElement::KeyValue(kv) => {
                            let k = match self.const_fold(kv.key)? {
                                Const::Int(i) => {
                                    if i >= next {
                                        next = i + 1;
                                    }
                                    ConstKey::Int(i)
                                }
                                Const::Str(s) => match crate::value::ArrayKey::from_bytes(&s) {
                                    crate::value::ArrayKey::Int(i) => {
                                        if i >= next {
                                            next = i + 1;
                                        }
                                        ConstKey::Int(i)
                                    }
                                    crate::value::ArrayKey::Str(_) => ConstKey::Str(s),
                                },
                                _ => return None,
                            };
                            let c = self.const_fold(kv.value)?;
                            items.push((k, c));
                        }
                        _ => return None,
                    }
                }
                Some(Const::Array(items))
            }
            Expression::Binary(b) => {
                if let BinaryOperator::StringConcat(_) = b.operator {
                    let l = self.const_fold(b.lhs)?;
                    let r = self.const_fold(b.rhs)?;
                    let mut s = const_to_bytes(&l)?;
                    s.extend(const_to_bytes(&r)?);
                    return Some(Const::Str(s));
                }
                None
            }
            _ => None,
        }
    }
}

fn const_to_bytes(c: &Const) -> Option<Vec<u8>> {
    Some(match c {
        Const::Str(s) => s.clone(),
        Const::Int(i) => i.to_string().into_bytes(),
        Const::Null => Vec::new(),
        Const::Bool(true) => b"1".to_vec(),
        Const::Bool(false) => Vec::new(),
        _ => return None,
    })
}

/// Types PHP prescribes for magic methods.
#[derive(Clone, Copy)]
enum MagicType {
    Void,
    Bool,
    String,
    Array,
    NullableArray,
    Object,
}

impl MagicType {
    fn name(self) -> &'static str {
        match self {
            MagicType::Void => "void",
            MagicType::Bool => "bool",
            MagicType::String => "string",
            MagicType::Array => "array",
            MagicType::NullableArray => "?array",
            MagicType::Object => "object",
        }
    }

    /// One member of a union type `t`.
    fn admits(self, t: &TypeHint) -> bool {
        match (self, t) {
            (MagicType::Void, TypeHint::Void) => true,
            (MagicType::Bool, TypeHint::Bool | TypeHint::True | TypeHint::False) => true,
            (MagicType::String, TypeHint::String) => true,
            (MagicType::Array | MagicType::NullableArray, TypeHint::Array) => true,
            (MagicType::NullableArray, TypeHint::Null) => true,
            (MagicType::Object, TypeHint::Object | TypeHint::Class(_) | TypeHint::SelfT | TypeHint::Static | TypeHint::Parent) => true,
            _ => false,
        }
    }

    fn members(t: &TypeHint) -> Vec<&TypeHint> {
        match t {
            TypeHint::Nullable(inner) => {
                let mut v = vec![&TypeHint::Null];
                v.extend(Self::members(inner));
                v
            }
            TypeHint::Union(parts) => parts.iter().flat_map(Self::members).collect(),
            other => vec![other],
        }
    }

    /// A declared argument type may include more, but must include this one.
    fn admits_any(self, t: &TypeHint) -> bool {
        matches!(t, TypeHint::Mixed) || Self::members(t).iter().any(|m| self.admits(m))
    }

    /// A declared return type must not include anything else.
    fn contains_all(self, t: &TypeHint) -> bool {
        Self::members(t).iter().all(|m| self.admits(m))
    }
}

/// PHP's reserved class names (zend_compile.c reserved_class_names).
pub(crate) fn reserved_class_name(name: &[u8]) -> bool {
    const RESERVED: [&str; 15] = ["bool", "false", "float", "int", "null", "parent", "self", "static", "string", "true", "void", "never", "iterable", "object", "mixed"];
    RESERVED.iter().any(|r| r.as_bytes().eq_ignore_ascii_case(name))
}

