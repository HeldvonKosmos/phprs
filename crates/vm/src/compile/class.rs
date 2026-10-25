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
            match m {
                Modifier::Abstract(_) => is_abstract = true,
                Modifier::Final(_) => is_final = true,
                Modifier::Readonly(_) => is_readonly = true,
                other => return self.err(other.span(), "ungültiger Klassen-Modifier"),
            }
        }
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
                    other => return self.err(other.span(), "ungültiges Argument"),
                }
            }
        }
        let spec = self.ub().arg_spec(spec);
        self.emit(Op::New { class: ClassRef::Named(name), spec });
        Ok(())
    }

    pub(crate) fn compile_interface(&mut self, i: &Interface<'_>) -> CResult<()> {
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
                        return self.err(c.span(), "Trait-Anpassungen (`insteadof`, `as`) sind nicht unterstützt");
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
                    return self.err(h.span(), "Property-Hooks sind nicht unterstützt");
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
        Ok(out)
    }

    /// (visibility, final, static, readonly)
    fn member_modifiers(&mut self, mods: &Sequence<'_, Modifier<'_>>) -> CResult<(Visibility, bool, bool, bool)> {
        let mut vis = Visibility::Public;
        let mut is_final = false;
        let mut is_static = false;
        let mut is_readonly = false;
        for m in mods.iter() {
            match m {
                Modifier::Public(_) => vis = Visibility::Public,
                Modifier::Protected(_) => vis = Visibility::Protected,
                Modifier::Private(_) => vis = Visibility::Private,
                Modifier::Final(_) => is_final = true,
                Modifier::Static(_) => is_static = true,
                Modifier::Readonly(_) => is_readonly = true,
                Modifier::Abstract(_) => {}
                other => return self.err(other.span(), "asymmetrische Sichtbarkeit ist nicht unterstützt"),
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
        Ok(MethodDecl { name, unit, sig_unit, visibility, is_static, is_final, attributes })
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

    fn const_fold(&mut self, e: &Expression<'_>) -> Option<Const> {
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
