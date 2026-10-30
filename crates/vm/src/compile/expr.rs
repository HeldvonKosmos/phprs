//! Expression compilation.

use mago_span::HasSpan;
use mago_syntax::cst::*;

use super::dialect;
use super::{CResult, Compiler};
use crate::bytecode::*;

/// How an expression's result is wanted. Statements discard it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Want {
    Value,
    Discard,
}

impl Compiler {
    /// Compile an expression and leave exactly one value on the stack.
    pub(crate) fn compile_expr(&mut self, e: &Expression<'_>) -> CResult<()> {
        self.compile_expr_w(e, Want::Value)
    }

    /// `Want::Discard` leaves nothing on the stack: assignments and
    /// increments use their discarding forms, every other expression is
    /// popped (statement `f();` must not leak its result).
    pub(crate) fn compile_expr_w(&mut self, e: &Expression<'_>, want: Want) -> CResult<()> {
        let self_discarding = match e {
            Expression::Parenthesized(_) | Expression::Assignment(_) | Expression::UnaryPostfix(_) => true,
            Expression::UnaryPrefix(u) => matches!(u.operator, UnaryPrefixOperator::ErrorControl(_) | UnaryPrefixOperator::PreIncrement(_) | UnaryPrefixOperator::PreDecrement(_)),
            _ => false,
        };
        self.compile_expr_inner(e, want)?;
        if want == Want::Discard && !self_discarding {
            self.emit(Op::Pop);
        }
        Ok(())
    }

    fn compile_expr_inner(&mut self, e: &Expression<'_>, want: Want) -> CResult<()> {
        let span = e.span();
        let line = self.line(span);
        self.ub().mark_line(line);
        match e {
            Expression::Parenthesized(p) => self.compile_expr_w(p.expression, want),
            Expression::Literal(l) => self.compile_literal(l),
            Expression::CompositeString(s) => self.compile_composite_string(s),
            Expression::Binary(b) => self.compile_binary(b),
            Expression::UnaryPrefix(u) => self.compile_unary_prefix(u, want),
            Expression::UnaryPostfix(u) => self.compile_unary_postfix(u, want),
            Expression::Assignment(a) => self.compile_assignment(a, want),
            Expression::Conditional(c) => self.compile_conditional(c),
            Expression::Array(a) => self.compile_array_literal(&a.elements),
            Expression::LegacyArray(a) => self.reject(dialect::LONG_ARRAY, a.span()),
            Expression::List(l) => self.reject(dialect::LIST, l.span()),
            Expression::ArrayAccess(_) | Expression::Access(_) | Expression::Call(_) => self.compile_chain(e),
            Expression::ArrayAppend(a) => self.err(a.span(), "`[]` kann nur als Zuweisungsziel stehen"),
            Expression::AnonymousClass(c) => self.compile_anonymous_class(c),
            Expression::Closure(c) => self.compile_closure(c),
            Expression::ArrowFunction(f) => self.compile_arrow_function(f),
            Expression::Variable(v) => self.compile_variable_read(v, false),
            Expression::ConstantAccess(c) => self.compile_constant_access(c.name.value(), c.span()),
            Expression::Identifier(id) => self.compile_constant_access(id.value(), id.span()),
            Expression::Match(m) => self.compile_match(m),
            Expression::Yield(y) => self.compile_yield(y),
            Expression::Construct(c) => self.compile_construct(c),
            Expression::Throw(t) => {
                self.compile_expr(t.exception)?;
                self.emit(Op::Throw);
                Ok(())
            }
            Expression::Clone(c) => {
                self.compile_expr(c.object)?;
                self.emit(Op::Clone);
                Ok(())
            }
            Expression::PartialApplication(p) => self.compile_partial_application(p),
            Expression::Parent(k) | Expression::Static(k) | Expression::Self_(k) => {
                self.err(k.span(), "`self`/`static`/`parent` sind hier kein Ausdruck")
            }
            Expression::Instantiation(i) => self.compile_instantiation(i),
            Expression::MagicConstant(m) => self.compile_magic_constant(m),
            Expression::Pipe(p) => {
                self.compile_expr(p.callable)?;
                self.compile_expr(p.input)?;
                let spec = self.ub().arg_spec(ArgSpec { names: vec![None], spread: vec![false] });
                self.emit(Op::CallValue { spec });
                Ok(())
            }
            Expression::Error(s) => self.err(*s, "Syntaxfehler"),
            _ => self.err(span, "unsupported expression"),
        }
    }

    // ---- literals ---------------------------------------------------------

    fn compile_literal(&mut self, l: &Literal<'_>) -> CResult<()> {
        match l {
            Literal::String(s) => {
                let bytes = match s.value {
                    Some(v) => v.to_vec(),
                    None => unquote_fallback(s.raw),
                };
                let c = self.constant(Const::Str(bytes));
                self.emit(Op::PushConst(c));
            }
            Literal::Integer(i) => match i.value {
                Some(v) if v <= i64::MAX as u64 => {
                    self.emit(Op::PushInt(v as i64));
                }
                _ => {
                    // overflow: PHP turns the literal into a float
                    let text = String::from_utf8_lossy(i.raw).replace('_', "");
                    let f: f64 = parse_int_literal_as_float(&text);
                    let c = self.constant(Const::Float(f));
                    self.emit(Op::PushConst(c));
                }
            },
            Literal::Float(f) => {
                let c = self.constant(Const::Float(f.value.0));
                self.emit(Op::PushConst(c));
            }
            Literal::True(_) => {
                self.emit(Op::PushTrue);
            }
            Literal::False(_) => {
                self.emit(Op::PushFalse);
            }
            Literal::Null(_) => {
                self.emit(Op::PushNull);
            }
        }
        Ok(())
    }

    fn compile_composite_string(&mut self, s: &CompositeString<'_>) -> CResult<()> {
        let parts = match s {
            CompositeString::Interpolated(i) => &i.parts,
            CompositeString::Document(d) => return self.reject(dialect::HEREDOC, d.span()),
            CompositeString::ShellExecute(x) => return self.reject(dialect::SHELL_EXEC, x.span()),
        };
        let mut first = true;
        for part in parts.iter() {
            match part {
                StringPart::Literal(lit) => {
                    let bytes = lit.value.map(|v| v.to_vec()).unwrap_or_else(|| lit.raw.to_vec());
                    let c = self.constant(Const::Str(bytes));
                    self.emit(Op::PushConst(c));
                }
                StringPart::Expression(e) => self.compile_interpolated_simple(e)?,
                StringPart::BracedExpression(b) => self.compile_expr(b.expression)?,
            }
            if !first {
                self.emit(Op::Bin(BinOp::Concat));
            }
            first = false;
        }
        if first {
            let c = self.constant(Const::Str(Vec::new()));
            self.emit(Op::PushConst(c));
        }
        Ok(())
    }

    /// `"$a[k]"`: in the simple interpolation syntax a bare key is a string.
    fn compile_interpolated_simple(&mut self, e: &Expression<'_>) -> CResult<()> {
        if let Expression::ArrayAccess(a) = e
            && let Some(name) = match a.index {
                Expression::ConstantAccess(c) if matches!(c.name, Identifier::Local(_)) => Some(c.name.value()),
                Expression::Identifier(Identifier::Local(id)) => Some(id.value),
                _ => None,
            }
        {
            self.compile_expr(a.array)?;
            let k = self.constant(Const::Str(name.to_vec()));
            self.emit(Op::PushConst(k));
            self.emit(Op::FetchDim);
            return Ok(());
        }
        self.compile_expr(e)
    }

    fn compile_magic_constant(&mut self, m: &MagicConstant<'_>) -> CResult<()> {
        let value: Vec<u8> = match m {
            MagicConstant::Line(l) => {
                let line = self.line(l.span());
                self.emit(Op::PushInt(line as i64));
                return Ok(());
            }
            MagicConstant::File(_) => self.module.file.clone().into_bytes(),
            MagicConstant::Directory(_) => {
                let f = self.module.file.clone();
                match f.rfind('/') {
                    Some(i) => f[..i].as_bytes().to_vec(),
                    None => b".".to_vec(),
                }
            }
            MagicConstant::Class(_) => self.current_class().map(|c| c.name.clone()).unwrap_or_default(),
            MagicConstant::Trait(_) => self
                .current_class()
                .filter(|c| c.kind == ClassKind::Trait)
                .map(|c| c.name.clone())
                .unwrap_or_default(),
            MagicConstant::Namespace(_) => self.names.namespace.clone(),
            MagicConstant::Function(_) => self.ub().function_name.clone(),
            MagicConstant::Method(_) => {
                let f = self.ub().function_name.clone();
                let is_method = self.ub().is_method;
                match (self.current_class(), is_method) {
                    (Some(c), true) => {
                        let mut v = c.name.clone();
                        v.extend_from_slice(b"::");
                        v.extend_from_slice(&f);
                        v
                    }
                    _ => f,
                }
            }
            MagicConstant::Property(_) => Vec::new(),
        };
        let c = self.constant(Const::Str(value));
        self.emit(Op::PushConst(c));
        Ok(())
    }

    fn compile_constant_access(&mut self, name: &[u8], span: mago_span::Span) -> CResult<()> {
        match name.to_ascii_lowercase().as_slice() {
            b"true" => {
                self.emit(Op::PushTrue);
            }
            b"false" => {
                self.emit(Op::PushFalse);
            }
            b"null" => {
                self.emit(Op::PushNull);
            }
            _ => {
                let _ = span;
                let (primary, fallback) = self.names.resolve_const(name);
                let p = self.intern(&primary);
                let f = fallback.map(|f| self.intern(&f));
                self.emit(Op::FetchConst(p, f));
            }
        }
        Ok(())
    }

    // ---- operators --------------------------------------------------------

    fn compile_binary(&mut self, b: &Binary<'_>) -> CResult<()> {
        use BinaryOperator as O;
        let op = match &b.operator {
            O::And(_) | O::LowAnd(_) => {
                self.compile_expr(b.lhs)?;
                let j = self.emit(Op::JumpIfFalse(0));
                self.compile_expr(b.rhs)?;
                self.emit(Op::ToBool);
                let end = self.emit(Op::Jump(0));
                let l = self.ub().here();
                self.ub().patch(j, l);
                self.emit(Op::PushFalse);
                let e = self.ub().here();
                self.ub().patch(end, e);
                return Ok(());
            }
            O::Or(_) | O::LowOr(_) => {
                self.compile_expr(b.lhs)?;
                let j = self.emit(Op::JumpIfTrue(0));
                self.compile_expr(b.rhs)?;
                self.emit(Op::ToBool);
                let end = self.emit(Op::Jump(0));
                let l = self.ub().here();
                self.ub().patch(j, l);
                self.emit(Op::PushTrue);
                let e = self.ub().here();
                self.ub().patch(end, e);
                return Ok(());
            }
            O::LowXor(_) => {
                self.compile_expr(b.lhs)?;
                self.emit(Op::ToBool);
                self.compile_expr(b.rhs)?;
                self.emit(Op::ToBool);
                self.emit(Op::Bin(BinOp::NotIdentical));
                return Ok(());
            }
            O::NullCoalesce(_) => {
                self.compile_quiet(b.lhs)?;
                let j = self.emit(Op::JumpIfNotNullKeep(0));
                self.compile_expr(b.rhs)?;
                let l = self.ub().here();
                self.ub().patch(j, l);
                return Ok(());
            }
            O::Instanceof(_) => {
                self.compile_expr(b.lhs)?;
                match self.class_ref_of(b.rhs)? {
                    Some(cr) => {
                        self.emit(Op::InstanceOf(cr));
                    }
                    None => {
                        self.compile_expr(b.rhs)?;
                        self.emit(Op::InstanceOf(ClassRef::Dynamic));
                    }
                }
                return Ok(());
            }
            O::Addition(_) => BinOp::Add,
            O::Subtraction(_) => BinOp::Sub,
            O::Multiplication(_) => BinOp::Mul,
            O::Division(_) => BinOp::Div,
            O::Modulo(_) => BinOp::Mod,
            O::Exponentiation(_) => BinOp::Pow,
            O::BitwiseAnd(_) => BinOp::BitAnd,
            O::BitwiseOr(_) => BinOp::BitOr,
            O::BitwiseXor(_) => BinOp::BitXor,
            O::LeftShift(_) => BinOp::Shl,
            O::RightShift(_) => BinOp::Shr,
            O::Equal(_) | O::Identical(_) => BinOp::Identical,
            O::NotEqual(_) | O::NotIdentical(_) | O::AngledNotEqual(_) => BinOp::NotIdentical,
            O::LessThan(_) => BinOp::Lt,
            O::LessThanOrEqual(_) => BinOp::Le,
            O::GreaterThan(_) => BinOp::Gt,
            O::GreaterThanOrEqual(_) => BinOp::Ge,
            O::Spaceship(_) => BinOp::Spaceship,
            O::StringConcat(_) => BinOp::Concat,
        };
        self.compile_expr(b.lhs)?;
        self.compile_expr(b.rhs)?;
        self.emit(Op::Bin(op));
        Ok(())
    }

    fn compile_unary_prefix(&mut self, u: &UnaryPrefix<'_>, want: Want) -> CResult<()> {
        use UnaryPrefixOperator as O;
        let op = match &u.operator {
            O::Reference(s) => return self.reject(dialect::REFERENCE, *s),
            O::ErrorControl(_) => return self.compile_expr_w(u.operand, want),
            O::PreIncrement(_) => return self.compile_incdec(u.operand, WriteMode::PreInc, want),
            O::PreDecrement(_) => return self.compile_incdec(u.operand, WriteMode::PreDec, want),
            O::Not(_) => UnOp::Not,
            O::Negation(_) => UnOp::Neg,
            O::Plus(_) => UnOp::Plus,
            O::BitwiseNot(_) => UnOp::BitNot,
            O::IntCast(..) | O::IntegerCast(..) => UnOp::CastInt,
            O::FloatCast(..) | O::DoubleCast(..) | O::RealCast(..) => UnOp::CastFloat,
            O::StringCast(..) | O::BinaryCast(..) => UnOp::CastString,
            O::BoolCast(..) | O::BooleanCast(..) => UnOp::CastBool,
            O::ArrayCast(..) => UnOp::CastArray,
            O::ObjectCast(..) => UnOp::CastObject,
            O::UnsetCast(s, _) | O::VoidCast(s, _) => return self.err(*s, "this cast is not allowed"),
        };
        self.compile_expr(u.operand)?;
        self.emit(Op::Un(op));
        Ok(())
    }

    fn compile_unary_postfix(&mut self, u: &UnaryPostfix<'_>, want: Want) -> CResult<()> {
        let mode = match u.operator {
            UnaryPostfixOperator::PostIncrement(_) => WriteMode::PostInc,
            UnaryPostfixOperator::PostDecrement(_) => WriteMode::PostDec,
        };
        self.compile_incdec(u.operand, mode, want)
    }

    fn compile_incdec(&mut self, target: &Expression<'_>, mode: WriteMode, want: Want) -> CResult<()> {
        if let Some(slot) = self.simple_slot_target(target)? {
            if want == Want::Discard {
                self.emit(Op::IncSlotDiscard { slot, mode });
            } else {
                self.emit(Op::IncSlot { slot, mode });
            }
            return Ok(());
        }
        let (base, segs) = self.build_path(target)?;
        self.emit(Op::PushNull); // value operand unused for inc/dec
        self.emit(Op::StorePath(Box::new(PathOp { base, segs, mode })));
        if want == Want::Discard {
            self.emit(Op::Pop);
        }
        Ok(())
    }

    /// OPT1: a condition compiled straight into jumps. Returns the jumps
    /// to patch to the "false" target; `a && b` and `a || b` jump without
    /// building a bool first (same truthiness and evaluation order).
    pub(crate) fn compile_false_jumps(&mut self, e: &Expression<'_>) -> CResult<Vec<usize>> {
        use BinaryOperator as O;
        match e {
            Expression::Parenthesized(p) => self.compile_false_jumps(p.expression),
            Expression::Binary(b) if matches!(b.operator, O::And(_) | O::LowAnd(_)) => {
                let mut jumps = self.compile_false_jumps(b.lhs)?;
                jumps.extend(self.compile_false_jumps(b.rhs)?);
                Ok(jumps)
            }
            Expression::Binary(b) if matches!(b.operator, O::Or(_) | O::LowOr(_)) => {
                self.compile_expr(b.lhs)?;
                let taken = self.emit(Op::JumpIfTrue(0));
                let jumps = self.compile_false_jumps(b.rhs)?;
                let t = self.ub().here();
                self.ub().patch(taken, t);
                Ok(jumps)
            }
            _ => {
                self.compile_expr(e)?;
                Ok(vec![self.emit(Op::JumpIfFalse(0))])
            }
        }
    }

    pub(crate) fn patch_all(&mut self, jumps: &[usize], label: crate::bytecode::Label) {
        for j in jumps {
            self.ub().patch(*j, label);
        }
    }

    fn compile_conditional(&mut self, c: &Conditional<'_>) -> CResult<()> {
        match c.then {
            Some(then) => {
                let j = self.compile_false_jumps(c.condition)?;
                self.compile_expr(then)?;
                let end = self.emit(Op::Jump(0));
                let l = self.ub().here();
                self.patch_all(&j, l);
                self.compile_expr(c.r#else)?;
                let e = self.ub().here();
                self.ub().patch(end, e);
            }
            None => {
                self.compile_expr(c.condition)?;
                let j = self.emit(Op::JumpIfTrueKeep(0));
                self.compile_expr(c.r#else)?;
                let e = self.ub().here();
                self.ub().patch(j, e);
            }
        }
        Ok(())
    }

    // ---- arrays -----------------------------------------------------------

    pub(crate) fn compile_array_literal(&mut self, elements: &TokenSeparatedSequence<'_, ArrayElement<'_>>) -> CResult<()> {
        // REST1: one op instead of one per element where the shape is static
        let n = elements.len();
        if n > 0 {
            if elements.iter().all(|el| matches!(el, ArrayElement::Value(_))) {
                for el in elements.iter() {
                    if let ArrayElement::Value(v) = el {
                        self.compile_expr(v.value)?;
                    }
                }
                self.emit(Op::ArrayList(n as u32));
                return Ok(());
            }
            if let Some(keys) = self.literal_keys(elements) {
                let tpl = self.constant(Const::Array(keys.into_iter().map(|k| (k, Const::Null)).collect()));
                for el in elements.iter() {
                    if let ArrayElement::KeyValue(kv) = el {
                        self.compile_expr(kv.value)?;
                    }
                }
                self.emit(Op::ArrayTemplate { tpl, n: n as u32 });
                return Ok(());
            }
        }
        self.emit(Op::NewArray(n as u32));
        for el in elements.iter() {
            match el {
                ArrayElement::Value(v) => {
                    self.compile_expr(v.value)?;
                    self.emit(Op::ArrayAppend);
                }
                ArrayElement::KeyValue(kv) => {
                    self.compile_expr(kv.key)?;
                    self.compile_expr(kv.value)?;
                    self.emit(Op::ArrayInsert);
                }
                ArrayElement::Variadic(v) => {
                    self.compile_expr(v.value)?;
                    self.emit(Op::ArraySpread);
                }
                ArrayElement::Missing(m) => return self.err(m.comma, "an empty array element is only allowed in destructuring"),
            }
        }
        Ok(())
    }

    /// Keys of `[k => v, …]` when every key is a constant and no two are the
    /// same after PHP's key normalisation ("5" is 5).
    fn literal_keys(&mut self, elements: &TokenSeparatedSequence<'_, ArrayElement<'_>>) -> Option<Vec<ConstKey>> {
        let mut keys: Vec<ConstKey> = Vec::new();
        for el in elements.iter() {
            let ArrayElement::KeyValue(kv) = el else { return None };
            let k = match self.const_fold(kv.key)? {
                Const::Int(i) => ConstKey::Int(i),
                Const::Str(s) => match crate::value::ArrayKey::from_bytes(&s) {
                    crate::value::ArrayKey::Int(i) => ConstKey::Int(i),
                    crate::value::ArrayKey::Str(_) => ConstKey::Str(s),
                },
                _ => return None,
            };
            if keys.contains(&k) {
                return None;
            }
            keys.push(k);
        }
        Some(keys)
    }

    // ---- variables and chains ----------------------------------------------

    pub(crate) fn compile_variable_read(&mut self, v: &Variable<'_>, quiet: bool) -> CResult<()> {
        match v {
            Variable::Direct(d) => {
                let name = var_name(d.name);
                if name == b"this" {
                    self.ub().unit.uses_this = true;
                    self.emit(Op::LoadThis);
                } else if dialect::is_superglobal(name) {
                    return self.reject(dialect::SUPERGLOBAL, d.span());
                } else {
                    let slot = self.ub().slot(name);
                    self.emit(if quiet { Op::LoadSlotQuiet(slot) } else { Op::LoadSlot(slot) });
                }
                Ok(())
            }
            Variable::Indirect(i) => self.reject(dialect::VARIABLE_VARIABLE, i.span()),
            Variable::Nested(n) => self.reject(dialect::VARIABLE_VARIABLE, n.span()),
        }
    }

    /// Quiet read for `isset`, `empty`, `??`: undefined variables, missing
    /// keys and missing properties read as null instead of throwing.
    pub(crate) fn compile_quiet(&mut self, e: &Expression<'_>) -> CResult<()> {
        match e {
            Expression::Parenthesized(p) => self.compile_quiet(p.expression),
            Expression::Variable(v) => self.compile_variable_read(v, true),
            Expression::ArrayAccess(a) => {
                self.compile_quiet(a.array)?;
                self.compile_expr(a.index)?;
                self.emit(Op::FetchDimQuiet);
                Ok(())
            }
            Expression::Access(Access::Property(p)) => {
                self.compile_quiet(p.object)?;
                self.compile_prop_fetch(&p.property, true)
            }
            Expression::Access(Access::NullSafeProperty(p)) => {
                self.compile_quiet(p.object)?;
                self.compile_prop_fetch(&p.property, true)
            }
            Expression::Access(Access::StaticProperty(_)) => self.compile_expr(e),
            _ => self.compile_expr(e),
        }
    }

    /// One value of `isset(...)` as a bool: a property at the end asks the object
    /// whether it is set (`__isset`, never `__get`), everything else is a quiet
    /// read compared with null.
    fn compile_isset_value(&mut self, e: &Expression<'_>) -> CResult<()> {
        let (object, sel) = match e {
            Expression::Parenthesized(p) => return self.compile_isset_value(p.expression),
            Expression::Access(Access::Property(p)) => (p.object, &p.property),
            Expression::Access(Access::NullSafeProperty(p)) => (p.object, &p.property),
            _ => {
                self.compile_quiet(e)?;
                self.emit(Op::IsNotNull);
                return Ok(());
            }
        };
        self.compile_quiet(object)?;
        match sel {
            ClassLikeMemberSelector::Identifier(id) => {
                let name = self.intern(id.value);
                self.emit(Op::IssetProp(name));
            }
            ClassLikeMemberSelector::Variable(v) => {
                self.compile_variable_read(v, false)?;
                self.emit(Op::IssetPropDyn);
            }
            ClassLikeMemberSelector::Expression(x) => {
                self.compile_expr(x.expression)?;
                self.emit(Op::IssetPropDyn);
            }
            ClassLikeMemberSelector::Missing(s) => return self.err(*s, "Property-Name fehlt"),
        }
        Ok(())
    }

    fn compile_prop_fetch(&mut self, sel: &ClassLikeMemberSelector<'_>, quiet: bool) -> CResult<()> {
        match sel {
            ClassLikeMemberSelector::Identifier(id) => {
                let name = self.intern(id.value);
                self.emit(if quiet { Op::FetchPropQuiet(name) } else { Op::FetchProp(name) });
            }
            ClassLikeMemberSelector::Variable(v) => {
                self.compile_variable_read(v, false)?;
                self.emit(if quiet { Op::FetchPropDynQuiet } else { Op::FetchPropDyn });
            }
            ClassLikeMemberSelector::Expression(x) => {
                self.compile_expr(x.expression)?;
                self.emit(if quiet { Op::FetchPropDynQuiet } else { Op::FetchPropDyn });
            }
            ClassLikeMemberSelector::Missing(s) => return self.err(*s, "Property-Name fehlt"),
        }
        Ok(())
    }

    /// Compiles member access / call chains, with `?->` short-circuiting the
    /// rest of the chain.
    fn compile_chain(&mut self, e: &Expression<'_>) -> CResult<()> {
        let mut nullsafe_jumps: Vec<usize> = Vec::new();
        self.compile_chain_inner(e, &mut nullsafe_jumps)?;
        if !nullsafe_jumps.is_empty() {
            let end = self.ub().here();
            for j in nullsafe_jumps {
                self.ub().patch(j, end);
            }
        }
        Ok(())
    }

    fn compile_chain_inner(&mut self, e: &Expression<'_>, ns: &mut Vec<usize>) -> CResult<()> {
        match e {
            Expression::Parenthesized(p) => {
                if matches!(p.expression, Expression::ArrayAccess(_) | Expression::Access(_) | Expression::Call(_)) {
                    self.compile_chain_inner(p.expression, ns)
                } else {
                    self.compile_expr(p.expression)
                }
            }
            Expression::ArrayAccess(a) => {
                self.compile_chain_inner(a.array, ns)?;
                self.compile_expr(a.index)?;
                self.emit(Op::FetchDim);
                Ok(())
            }
            Expression::Access(Access::Property(p)) => {
                self.compile_chain_inner(p.object, ns)?;
                self.compile_prop_fetch(&p.property, false)
            }
            Expression::Access(Access::NullSafeProperty(p)) => {
                self.compile_chain_inner(p.object, ns)?;
                let j = self.emit(Op::JumpIfNullKeep(0));
                ns.push(j);
                self.compile_prop_fetch(&p.property, false)
            }
            Expression::Access(Access::StaticProperty(sp)) => {
                let cr = self.class_ref_required(sp.class)?;
                let name = match &sp.property {
                    Variable::Direct(d) => self.intern(var_name(d.name)),
                    other => return self.reject(dialect::VARIABLE_VARIABLE, other.span()),
                };
                self.emit(Op::FetchStaticProp(cr, name));
                Ok(())
            }
            Expression::Access(Access::ClassConstant(cc)) => {
                // `Name::class` is resolved at compile time from the imports,
                // like PHP: the class need not exist.
                if let ClassLikeConstantSelector::Identifier(id) = &cc.constant
                    && id.value.eq_ignore_ascii_case(b"class")
                    && let Expression::Identifier(name) = cc.class
                    && let ClassRef::Named(sid) = self.named_class_ref(name.value())
                {
                    let fq = self.module.strings[sid as usize].clone();
                    let c = self.constant(Const::Str(fq));
                    self.emit(Op::PushConst(c));
                    return Ok(());
                }
                let cr = self.class_ref_required(cc.class)?;
                match &cc.constant {
                    ClassLikeConstantSelector::Identifier(id) => {
                        if id.value.eq_ignore_ascii_case(b"class") {
                            self.emit(Op::FetchClassName(cr));
                        } else {
                            let name = self.intern(id.value);
                            self.emit(Op::FetchClassConst(cr, name));
                        }
                    }
                    ClassLikeConstantSelector::Expression(x) => return self.err(x.span(), "dynamic class constants are not allowed"),
                    ClassLikeConstantSelector::Missing(s) => return self.err(*s, "Konstantenname fehlt"),
                }
                Ok(())
            }
            Expression::Call(c) => self.compile_call(c, ns),
            other => self.compile_expr(other),
        }
    }

    fn compile_args(&mut self, args: &ArgumentList<'_>) -> CResult<u32> {
        let mut spec = ArgSpec::default();
        for a in args.arguments.iter() {
            match a {
                Argument::Positional(p) => {
                    if !spec.names.is_empty() && spec.names.last().is_some_and(Option::is_some) && p.ellipsis.is_none() {
                        return self.err(p.span(), "positionales Argument nach benanntem Argument");
                    }
                    self.compile_expr(p.value)?;
                    spec.names.push(None);
                    spec.spread.push(p.ellipsis.is_some());
                }
                Argument::Named(n) => {
                    self.compile_expr(n.value)?;
                    let name = self.intern(n.name.value);
                    spec.names.push(Some(name));
                    spec.spread.push(false);
                }
            }
        }
        Ok(self.ub().arg_spec(spec))
    }

    fn compile_call(&mut self, c: &Call<'_>, ns: &mut Vec<usize>) -> CResult<()> {
        match c {
            Call::Function(f) => match f.function {
                Expression::Identifier(id) => {
                    let raw = id.value();
                    let bare = raw.strip_prefix(b"\\").unwrap_or(raw);
                    if !bare.contains(&b'\\') && dialect::is_forbidden_function(bare) {
                        return self.reject_named(dialect::FORBIDDEN_BUILTIN, bare, id.span());
                    }
                    if !bare.contains(&b'\\') && dialect::is_byref_pop(bare) {
                        return self.reject(dialect::BYREF_POP, id.span());
                    }
                    let (primary, fallback) = self.names.resolve_function(raw);
                    let name = self.intern(&primary);
                    let fallback = fallback.map(|f| self.intern(&f));
                    let spec = self.compile_args(&f.argument_list)?;
                    self.emit(Op::CallFunc { name, fallback, spec });
                    Ok(())
                }
                callee => {
                    self.compile_expr(callee)?;
                    let spec = self.compile_args(&f.argument_list)?;
                    self.emit(Op::CallValue { spec });
                    Ok(())
                }
            },
            Call::Method(m) => {
                self.compile_chain_inner(m.object, ns)?;
                self.compile_method_call(&m.method, &m.argument_list)
            }
            Call::NullSafeMethod(m) => {
                self.compile_chain_inner(m.object, ns)?;
                let j = self.emit(Op::JumpIfNullKeep(0));
                ns.push(j);
                self.compile_method_call(&m.method, &m.argument_list)
            }
            Call::StaticMethod(s) => {
                let cr = self.class_ref_required(s.class)?;
                match &s.method {
                    ClassLikeMemberSelector::Identifier(id) => {
                        let name = self.intern(id.value);
                        let spec = self.compile_args(&s.argument_list)?;
                        self.emit(Op::CallStatic { class: cr, name, spec });
                        Ok(())
                    }
                    other => self.err(other.span(), "dynamic static method names are not allowed"),
                }
            }
        }
    }

    fn compile_method_call(&mut self, sel: &ClassLikeMemberSelector<'_>, args: &ArgumentList<'_>) -> CResult<()> {
        match sel {
            ClassLikeMemberSelector::Identifier(id) => {
                let name = self.intern(id.value);
                let spec = self.compile_args(args)?;
                self.emit(Op::CallMethod { name, spec });
            }
            ClassLikeMemberSelector::Variable(v) => {
                self.compile_variable_read(v, false)?;
                let spec = self.compile_args(args)?;
                self.emit(Op::CallMethodDyn { spec });
            }
            ClassLikeMemberSelector::Expression(x) => {
                self.compile_expr(x.expression)?;
                let spec = self.compile_args(args)?;
                self.emit(Op::CallMethodDyn { spec });
            }
            ClassLikeMemberSelector::Missing(s) => return self.err(*s, "Methodenname fehlt"),
        }
        Ok(())
    }

    /// `ClassRef` for an expression in class position, or `None` if it is a
    /// runtime value (string or object) that must be compiled.
    pub(crate) fn class_ref_of(&mut self, e: &Expression<'_>) -> CResult<Option<ClassRef>> {
        Ok(match e {
            Expression::Identifier(id) => Some(self.named_class_ref(id.value())),
            Expression::Self_(_) => Some(ClassRef::SelfRef),
            Expression::Static(_) => Some(ClassRef::Static),
            Expression::Parent(_) => Some(ClassRef::Parent),
            Expression::Parenthesized(p) => return self.class_ref_of(p.expression),
            _ => None,
        })
    }

    pub(crate) fn named_class_ref(&mut self, name: &[u8]) -> ClassRef {
        let fq = self.names.resolve_class(name);
        match fq.as_slice() {
            b"self" => ClassRef::SelfRef,
            b"static" => ClassRef::Static,
            b"parent" => ClassRef::Parent,
            _ => ClassRef::Named(self.intern(&fq)),
        }
    }

    /// Like `class_ref_of`, but compiles a runtime class expression and
    /// returns `ClassRef::Dynamic`.
    pub(crate) fn class_ref_required(&mut self, e: &Expression<'_>) -> CResult<ClassRef> {
        match self.class_ref_of(e)? {
            Some(cr) => Ok(cr),
            None => {
                self.compile_expr(e)?;
                Ok(ClassRef::Dynamic)
            }
        }
    }

    fn compile_instantiation(&mut self, i: &Instantiation<'_>) -> CResult<()> {
        let cr = self.class_ref_required(i.class)?;
        let spec = match &i.argument_list {
            Some(args) => self.compile_args(args)?,
            None => self.ub().arg_spec(ArgSpec::default()),
        };
        self.emit(Op::New { class: cr, spec });
        Ok(())
    }

    // ---- assignment ---------------------------------------------------------

    /// Slot of a plain `$var` target, or `None` for anything else.
    fn simple_slot_target(&mut self, target: &Expression<'_>) -> CResult<Option<Slot>> {
        Ok(match target {
            Expression::Parenthesized(p) => return self.simple_slot_target(p.expression),
            Expression::Variable(Variable::Direct(d)) => {
                let name = var_name(d.name);
                if name == b"this" {
                    return self.err(d.span(), "`$this` cannot be assigned");
                }
                if dialect::is_superglobal(name) {
                    return self.reject(dialect::SUPERGLOBAL, d.span());
                }
                Some(self.ub().slot(name))
            }
            Expression::Variable(v) => return self.reject(dialect::VARIABLE_VARIABLE, v.span()),
            _ => None,
        })
    }

    /// Emits base (if from the stack) and keys of an lvalue path, returns the path.
    pub(crate) fn build_path(&mut self, target: &Expression<'_>) -> CResult<(PathBase, Vec<PathSeg>)> {
        match target {
            Expression::Parenthesized(p) => self.build_path(p.expression),
            Expression::Variable(Variable::Direct(d)) => {
                let name = var_name(d.name);
                if name == b"this" {
                    self.ub().unit.uses_this = true;
                    Ok((PathBase::This, Vec::new()))
                } else if dialect::is_superglobal(name) {
                    self.reject(dialect::SUPERGLOBAL, d.span())
                } else {
                    let slot = self.ub().slot(name);
                    Ok((PathBase::Slot(slot), Vec::new()))
                }
            }
            Expression::Variable(v) => self.reject(dialect::VARIABLE_VARIABLE, v.span()),
            Expression::ArrayAccess(a) => {
                let (base, mut segs) = self.build_path(a.array)?;
                self.compile_expr(a.index)?;
                segs.push(PathSeg::Dim);
                Ok((base, segs))
            }
            Expression::ArrayAppend(a) => {
                let (base, mut segs) = self.build_path(a.array)?;
                segs.push(PathSeg::Append);
                Ok((base, segs))
            }
            Expression::Access(Access::Property(p)) => {
                let (base, mut segs) = self.build_path(p.object)?;
                match &p.property {
                    ClassLikeMemberSelector::Identifier(id) => {
                        let name = self.intern(id.value);
                        segs.push(PathSeg::Prop(name));
                    }
                    ClassLikeMemberSelector::Variable(v) => {
                        self.compile_variable_read(v, false)?;
                        segs.push(PathSeg::PropDyn);
                    }
                    ClassLikeMemberSelector::Expression(x) => {
                        self.compile_expr(x.expression)?;
                        segs.push(PathSeg::PropDyn);
                    }
                    ClassLikeMemberSelector::Missing(s) => return self.err(*s, "Property-Name fehlt"),
                }
                Ok((base, segs))
            }
            Expression::Access(Access::NullSafeProperty(p)) => self.err(p.span(), "`?->` cannot be assigned"),
            Expression::Access(Access::StaticProperty(sp)) => {
                let cr = self.class_ref_required(sp.class)?;
                let name = match &sp.property {
                    Variable::Direct(d) => self.intern(var_name(d.name)),
                    other => return self.reject(dialect::VARIABLE_VARIABLE, other.span()),
                };
                Ok((PathBase::StaticProp(cr, name), Vec::new()))
            }
            Expression::Access(Access::ClassConstant(c)) => self.err(c.span(), "constants cannot be assigned"),
            Expression::List(l) => self.reject(dialect::LIST, l.span()),
            other => {
                // e.g. `getObj()->x = 1`, `(new Foo)->x = 1`
                self.compile_expr(other)?;
                Ok((PathBase::Stack, Vec::new()))
            }
        }
    }

    fn compile_assignment(&mut self, a: &Assignment<'_>, want: Want) -> CResult<()> {
        use AssignmentOperator as AO;
        let mode = match a.operator {
            AO::Assign(_) => WriteMode::Assign,
            AO::Addition(_) => WriteMode::Compound(BinOp::Add),
            AO::Subtraction(_) => WriteMode::Compound(BinOp::Sub),
            AO::Multiplication(_) => WriteMode::Compound(BinOp::Mul),
            AO::Division(_) => WriteMode::Compound(BinOp::Div),
            AO::Modulo(_) => WriteMode::Compound(BinOp::Mod),
            AO::Exponentiation(_) => WriteMode::Compound(BinOp::Pow),
            AO::Concat(_) => WriteMode::Compound(BinOp::Concat),
            AO::BitwiseAnd(_) => WriteMode::Compound(BinOp::BitAnd),
            AO::BitwiseOr(_) => WriteMode::Compound(BinOp::BitOr),
            AO::BitwiseXor(_) => WriteMode::Compound(BinOp::BitXor),
            AO::LeftShift(_) => WriteMode::Compound(BinOp::Shl),
            AO::RightShift(_) => WriteMode::Compound(BinOp::Shr),
            AO::Coalesce(_) => {
                // `a ??= b`: read quietly; assign only when null
                self.compile_quiet(a.lhs)?;
                let j = self.emit(Op::JumpIfNotNullKeep(0));
                self.compile_assign_to(a.lhs, a.rhs, WriteMode::Assign)?;
                let l = self.ub().here();
                self.ub().patch(j, l);
                if want == Want::Discard {
                    self.emit(Op::Pop);
                }
                return Ok(());
            }
        };
        // destructuring
        if let (Expression::Array(arr), WriteMode::Assign) = (a.lhs, mode) {
            self.compile_expr(a.rhs)?;
            self.compile_destructure(&arr.elements)?;
            if want == Want::Discard {
                self.emit(Op::Pop);
            }
            return Ok(());
        }
        if let (Expression::List(l), _) = (a.lhs, mode) {
            return self.reject(dialect::LIST, l.span());
        }
        // fast paths for plain variables in statement context
        if let Some(slot) = self.simple_slot_target(a.lhs)? {
            match mode {
                WriteMode::Assign => {
                    self.ub().note_assignment(slot, never_null(a.rhs));
                    self.compile_expr(a.rhs)?;
                    if want == Want::Discard {
                        self.emit(Op::StoreSlot(slot));
                    } else {
                        self.emit(Op::Dup);
                        self.emit(Op::StoreSlot(slot));
                    }
                }
                WriteMode::Compound(op) => {
                    self.compile_expr(a.rhs)?;
                    self.emit(Op::CompoundSlot { slot, op, discard: want == Want::Discard });
                }
                _ => unreachable!(),
            }
            return Ok(());
        }
        self.compile_assign_to(a.lhs, a.rhs, mode)?;
        if want == Want::Discard {
            self.emit(Op::Pop);
        }
        Ok(())
    }

    /// Leaves the assigned value on the stack.
    fn compile_assign_to(&mut self, target: &Expression<'_>, rhs: &Expression<'_>, mode: WriteMode) -> CResult<()> {
        if let Some(slot) = self.simple_slot_target(target)? {
            match mode {
                WriteMode::Assign => {
                    self.ub().note_assignment(slot, never_null(rhs));
                    self.compile_expr(rhs)?;
                    self.emit(Op::Dup);
                    self.emit(Op::StoreSlot(slot));
                }
                WriteMode::Compound(op) => {
                    self.compile_expr(rhs)?;
                    self.emit(Op::CompoundSlot { slot, op, discard: false });
                }
                _ => unreachable!(),
            }
            return Ok(());
        }
        let (base, segs) = self.build_path(target)?;
        self.compile_expr(rhs)?;
        self.emit(Op::StorePath(Box::new(PathOp { base, segs, mode })));
        Ok(())
    }

    /// Value to destructure is on the stack and stays there.
    pub(crate) fn compile_destructure(&mut self, elements: &TokenSeparatedSequence<'_, ArrayElement<'_>>) -> CResult<()> {
        let mut index: i64 = 0;
        for el in elements.iter() {
            let (key_is_expr, target) = match el {
                ArrayElement::Missing(_) => {
                    index += 1;
                    continue;
                }
                ArrayElement::Value(v) => (None, v.value),
                ArrayElement::KeyValue(kv) => (Some(kv.key), kv.value),
                ArrayElement::Variadic(v) => return self.err(v.span(), "`...` is not allowed in destructuring"),
            };
            self.emit(Op::Dup);
            match key_is_expr {
                Some(k) => self.compile_expr(k)?,
                None => {
                    self.emit(Op::PushInt(index));
                    index += 1;
                }
            }
            self.emit(Op::FetchDim);
            self.compile_store_popped(target)?;
        }
        Ok(())
    }

    /// Store the value on top of the stack into `target`, popping it.
    fn compile_store_popped(&mut self, target: &Expression<'_>) -> CResult<()> {
        match target {
            Expression::Array(arr) => {
                self.compile_destructure(&arr.elements)?;
                self.emit(Op::Pop);
                Ok(())
            }
            Expression::List(l) => self.reject(dialect::LIST, l.span()),
            _ => {
                if let Some(slot) = self.simple_slot_target(target)? {
                    self.ub().note_assignment(slot, false);
                    self.emit(Op::StoreSlot(slot));
                    return Ok(());
                }
                let tmp = self.ub().temp_slot();
                self.emit(Op::BindSlot(tmp));
                let (base, segs) = self.build_path(target)?;
                self.emit(Op::LoadSlot(tmp));
                self.emit(Op::StorePath(Box::new(PathOp { base, segs, mode: WriteMode::Assign })));
                self.emit(Op::Pop);
                Ok(())
            }
        }
    }

    // ---- match / yield / constructs -------------------------------------------

    fn compile_match(&mut self, m: &Match<'_>) -> CResult<()> {
        let subject = self.ub().temp_slot();
        self.compile_expr(m.expression)?;
        self.emit(Op::BindSlot(subject));
        let mut end_jumps = Vec::new();
        let mut default: Option<&Expression<'_>> = None;
        for arm in m.arms.iter() {
            match arm {
                MatchArm::Default(d) => {
                    if default.is_some() {
                        return self.err(d.span(), "mehrere `default`-Arme in `match`");
                    }
                    default = Some(d.expression);
                }
                MatchArm::Expression(a) => {
                    let mut body_jumps = Vec::new();
                    for cond in a.conditions.iter() {
                        self.emit(Op::LoadSlot(subject));
                        self.compile_expr(cond)?;
                        self.emit(Op::Bin(BinOp::Identical));
                        body_jumps.push(self.emit(Op::JumpIfTrue(0)));
                    }
                    let skip = self.emit(Op::Jump(0));
                    let body = self.ub().here();
                    for j in body_jumps {
                        self.ub().patch(j, body);
                    }
                    self.compile_expr(a.expression)?;
                    end_jumps.push(self.emit(Op::Jump(0)));
                    let next = self.ub().here();
                    self.ub().patch(skip, next);
                }
            }
        }
        match default {
            Some(d) => self.compile_expr(d)?,
            None => {
                self.emit(Op::LoadSlot(subject));
                self.emit(Op::ThrowUnhandledMatch);
            }
        }
        let end = self.ub().here();
        for j in end_jumps {
            self.ub().patch(j, end);
        }
        Ok(())
    }

    fn compile_yield(&mut self, y: &Yield<'_>) -> CResult<()> {
        self.ub().unit.is_generator = true;
        match y {
            Yield::Value(v) => {
                match v.value {
                    Some(e) => self.compile_expr(e)?,
                    None => {
                        self.emit(Op::PushNull);
                    }
                }
                self.emit(Op::Yield { with_key: false });
            }
            Yield::Pair(p) => {
                self.compile_expr(p.key)?;
                self.compile_expr(p.value)?;
                self.emit(Op::Yield { with_key: true });
            }
            Yield::From(f) => {
                self.compile_expr(f.iterator)?;
                self.emit(Op::YieldFrom);
            }
        }
        Ok(())
    }

    fn compile_construct(&mut self, c: &Construct<'_>) -> CResult<()> {
        match c {
            Construct::Isset(i) => {
                let mut false_jumps = Vec::new();
                let n = i.values.len();
                for (k, v) in i.values.iter().enumerate() {
                    self.compile_isset_value(v)?;
                    if k + 1 < n {
                        false_jumps.push(self.emit(Op::JumpIfFalse(0)));
                    }
                }
                if !false_jumps.is_empty() {
                    let end = self.emit(Op::Jump(0));
                    let l = self.ub().here();
                    for j in false_jumps {
                        self.ub().patch(j, l);
                    }
                    self.emit(Op::PushFalse);
                    let e = self.ub().here();
                    self.ub().patch(end, e);
                }
                Ok(())
            }
            Construct::Empty(e) => {
                self.compile_quiet(e.value)?;
                self.emit(Op::Un(UnOp::Not));
                Ok(())
            }
            Construct::Eval(e) => self.reject(dialect::EVAL, e.span()),
            Construct::Include(i) => self.reject(dialect::INCLUDE, i.span()),
            Construct::IncludeOnce(i) => self.reject(dialect::INCLUDE, i.span()),
            Construct::Require(i) => self.reject(dialect::INCLUDE, i.span()),
            Construct::RequireOnce(i) => self.reject(dialect::INCLUDE, i.span()),
            Construct::Print(p) => {
                self.compile_expr(p.value)?;
                self.emit(Op::Echo);
                self.emit(Op::PushInt(1));
                Ok(())
            }
            Construct::Exit(e) => self.reject(dialect::EXIT, e.span()),
            Construct::Die(e) => self.reject(dialect::EXIT, e.span()),
        }
    }

    fn compile_partial_application(&mut self, p: &PartialApplication<'_>) -> CResult<()> {
        // Only the first-class callable form `f(...)` is supported.
        fn is_fcc(args: &PartialArgumentList<'_>) -> bool {
            args.arguments.len() == 1 && matches!(args.arguments.first(), Some(PartialArgument::VariadicPlaceholder(_)))
        }
        match p {
            PartialApplication::Function(f) => {
                if !is_fcc(&f.argument_list) {
                    return self.err(f.span(), "partial application is not supported; only `f(...)`");
                }
                match f.function {
                    Expression::Identifier(id) => {
                        let (primary, fallback) = self.names.resolve_function(id.value());
                        let name = self.intern(&primary);
                        let fallback = fallback.map(|f| self.intern(&f));
                        self.emit(Op::CallableFromFunc { name, fallback });
                    }
                    other => {
                        self.compile_expr(other)?;
                    }
                }
            }
            PartialApplication::Method(m) => {
                if !is_fcc(&m.argument_list) {
                    return self.err(m.span(), "partial application is not supported; only `f(...)`");
                }
                self.compile_expr(m.object)?;
                match &m.method {
                    ClassLikeMemberSelector::Identifier(id) => {
                        let name = self.intern(id.value);
                        self.emit(Op::CallableFromMethod(name));
                    }
                    other => return self.err(other.span(), "dynamischer Methodenname in `(...)`"),
                }
            }
            PartialApplication::StaticMethod(s) => {
                if !is_fcc(&s.argument_list) {
                    return self.err(s.span(), "partial application is not supported; only `f(...)`");
                }
                let cr = self.class_ref_required(s.class)?;
                match &s.method {
                    ClassLikeMemberSelector::Identifier(id) => {
                        let name = self.intern(id.value);
                        self.emit(Op::CallableFromStatic(cr, name));
                    }
                    other => return self.err(other.span(), "dynamischer Methodenname in `(...)`"),
                }
            }
        }
        Ok(())
    }
}

/// `$name` → `name`
pub(crate) fn var_name(raw: &[u8]) -> &[u8] {
    raw.strip_prefix(b"$").unwrap_or(raw)
}

fn parse_int_literal_as_float(text: &str) -> f64 {
    let t = text.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u128::from_str_radix(h, 16).map(|v| v as f64).unwrap_or(f64::INFINITY);
    }
    if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        return u128::from_str_radix(b, 2).map(|v| v as f64).unwrap_or(f64::INFINITY);
    }
    if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
        return u128::from_str_radix(o, 8).map(|v| v as f64).unwrap_or(f64::INFINITY);
    }
    if t.len() > 1 && t.starts_with('0') && t.bytes().all(|b| b.is_ascii_digit()) {
        return u128::from_str_radix(&t[1..], 8).map(|v| v as f64).unwrap_or(f64::INFINITY);
    }
    t.parse::<f64>().unwrap_or(f64::INFINITY)
}

/// Fallback unescaping when mago gives no decoded value.
fn unquote_fallback(raw: &[u8]) -> Vec<u8> {
    if raw.len() < 2 {
        return raw.to_vec();
    }
    let quote = raw[0];
    let inner = &raw[1..raw.len() - 1];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let b = inner[i];
        if b == b'\\' && i + 1 < inner.len() {
            let n = inner[i + 1];
            if quote == b'\'' {
                if n == b'\'' || n == b'\\' {
                    out.push(n);
                    i += 2;
                    continue;
                }
                out.push(b);
                i += 1;
                continue;
            }
            match n {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'v' => out.push(0x0b),
                b'f' => out.push(0x0c),
                b'e' => out.push(0x1b),
                b'0' => out.push(0),
                b'\\' => out.push(b'\\'),
                b'$' => out.push(b'$'),
                b'"' => out.push(b'"'),
                other => {
                    out.push(b'\\');
                    out.push(other);
                }
            }
            i += 2;
        } else {
            out.push(b);
            i += 1;
        }
    }
    out
}

/// GRD021 null rule: is the value of `e` evidently never `null`?
fn never_null(e: &Expression<'_>) -> bool {
    match e {
        Expression::Parenthesized(p) => never_null(p.expression),
        Expression::Literal(Literal::Null(_)) => false,
        Expression::Literal(_) | Expression::CompositeString(_) | Expression::Array(_) | Expression::LegacyArray(_) => true,
        Expression::Instantiation(_) | Expression::Closure(_) | Expression::ArrowFunction(_) | Expression::AnonymousClass(_) => true,
        Expression::Binary(b) => !matches!(b.operator, BinaryOperator::NullCoalesce(_)),
        Expression::UnaryPrefix(u) => !matches!(u.operator, UnaryPrefixOperator::ErrorControl(_) | UnaryPrefixOperator::Reference(_)),
        _ => false,
    }
}
