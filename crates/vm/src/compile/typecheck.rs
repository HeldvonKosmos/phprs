//! GRD021 (DIALECT §5.2, D10): a local variable keeps the type of its first
//! assignment. This pass checks it at compile time wherever both types are
//! evident; the VM checks the remaining stores at run time (`StoreSlot`).
//!
//! Static and per scope (top level, function, method, closure, arrow
//! function), assignments in source order. A type is evident from literals,
//! casts, arithmetic of known numbers (`/` is always float), concatenation,
//! comparisons, `new`, array literals, typed parameters and variables already
//! typed. Rules:
//! - the first assignment fixes the type; a first `null` makes it `?T` with
//!   the first non-null type;
//! - a later assignment of another non-null type is GRD021, `null` into a
//!   variable that was not `?T` too;
//! - no widening int → float for locals (the message suggests `0.0` or
//!   `(float)`).
//! `foreach` and `catch` variables are bound per iteration and not checked.
//! The same pass counts for `grd check --report` (strict variant: also a
//! first `null` fixes the type).

use std::collections::{HashMap, HashSet};

use mago_span::HasSpan;
use mago_syntax::cst::*;
use mago_syntax::walker::{MutWalker, walk_program_mut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum T {
    Null,
    Int,
    Float,
    Bool,
    Str,
    Array,
    Object,
}

impl T {
    pub fn name(self) -> &'static str {
        match self {
            T::Null => "null",
            T::Int => "int",
            T::Float => "float",
            T::Bool => "bool",
            T::Str => "string",
            T::Array => "array",
            T::Object => "object",
        }
    }
}

/// One GRD021 violation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub line: u32,
    pub var: String,
    pub declared: T,
    pub assigned: T,
}

impl Violation {
    pub fn message(&self) -> String {
        format!("the local variable ${} is {}, but {} is assigned", self.var, self.declared.name(), self.assigned.name())
    }
    pub fn hint(&self) -> &'static str {
        match (self.declared, self.assigned) {
            (T::Float, T::Int) => "A local variable keeps the type of its first assignment; write the value as a float literal (1.0) or cast it with (float).",
            (T::Int, T::Float) => "A local variable keeps the type of its first assignment; start with 0.0 or (float) if it is to hold floats.",
            (_, T::Null) => "A local variable keeps the type of its first assignment; if it is to hold null, start with `$x = null;` (type ?T).",
            _ => "A local variable keeps the type of its first assignment; use a separate variable for the other value.",
        }
    }
}

/// Result for one file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grd021Counts {
    /// Violations of the strict variant (a first `null` fixes the type too).
    pub strict: u32,
    /// Violations of the rule in force (`?T` after a first `null`).
    pub tolerant: u32,
    /// Of `tolerant`, the int/float changes.
    pub int_float: u32,
    /// Assignments to plain variables whose type was judged.
    pub judged: u32,
    /// Assignments to plain variables whose type is not evident.
    pub not_judged: u32,
    /// Source lines of the first violations (rule in force).
    pub lines: Vec<u32>,
    /// All violations of the rule in force.
    pub violations: Vec<Violation>,
    /// Per scope (key: start offset of its parameter list; the top level is
    /// `u32::MAX`), the variables whose type is not static (`mixed`): the
    /// first non-null value had no evident type. The VM does not check them.
    pub mixed: HashMap<u32, HashSet<Vec<u8>>>,
}

#[derive(Default)]
struct Var {
    /// first assignment (strict variant)
    first: Option<T>,
    /// established non-null type
    ty: Option<T>,
    /// may hold null (`?T`, or first assignment `null`, or nullable parameter)
    nullable: bool,
    /// no static type: the first non-null value had no evident type
    mixed: bool,
}

#[derive(Default)]
struct Scope {
    key: u32,
    vars: HashMap<Vec<u8>, Var>,
}

struct Counter {
    scopes: Vec<Scope>,
    counts: Grd021Counts,
    lines: super::LineIndex,
}

fn var_name<'a>(e: &Expression<'a>) -> Option<&'a [u8]> {
    match e {
        Expression::Variable(Variable::Direct(d)) => Some(d.name.strip_prefix(b"$").unwrap_or(d.name)),
        _ => None,
    }
}

/// Type of a parameter hint: (type, nullable); `None` for anything else.
fn hint_type(h: &Hint<'_>) -> Option<(T, bool)> {
    Some(match h {
        Hint::Integer(_) => (T::Int, false),
        Hint::Float(_) => (T::Float, false),
        Hint::String(_) => (T::Str, false),
        Hint::Bool(_) | Hint::True(_) | Hint::False(_) => (T::Bool, false),
        Hint::Array(_) => (T::Array, false),
        Hint::Object(_) | Hint::Identifier(_) | Hint::Self_(_) | Hint::Static(_) | Hint::Parent(_) => (T::Object, false),
        Hint::Nullable(n) => (hint_type(n.hint)?.0, true),
        Hint::Parenthesized(p) => hint_type(p.hint)?,
        _ => return None,
    })
}

impl Counter {
    fn scope(&mut self) -> &mut Scope {
        self.scopes.last_mut().expect("scope")
    }

    fn enter(&mut self, params: Option<&FunctionLikeParameterList<'_>>) {
        let mut scope = Scope { key: params.map(|p| p.span().start.offset).unwrap_or(u32::MAX), ..Default::default() };
        for p in params.into_iter().flat_map(|l| l.parameters.iter()) {
            let name = p.variable.name.strip_prefix(b"$").unwrap_or(p.variable.name).to_vec();
            let default_null = p.default_value.as_ref().is_some_and(|d| matches!(d.value, Expression::Literal(Literal::Null(_))));
            let var = match p.hint.as_ref().and_then(hint_type) {
                // variadic parameters are arrays
                _ if p.ellipsis.is_some() => Var { first: Some(T::Array), ty: Some(T::Array), nullable: false, mixed: false },
                Some((t, nullable)) => Var { first: Some(t), ty: Some(t), nullable: nullable || default_null, mixed: false },
                // untyped or a union: no static type
                None => Var { first: None, ty: None, nullable: true, mixed: true },
            };
            scope.vars.insert(name, var);
        }
        self.scopes.push(scope);
    }

    fn known(&self, name: &[u8]) -> Option<T> {
        let v = self.scopes.last()?.vars.get(name)?;
        if v.mixed { None } else { v.ty }
    }

    fn leave(&mut self) {
        if let Some(scope) = self.scopes.pop() {
            let mixed: HashSet<Vec<u8>> = scope.vars.into_iter().filter(|(_, v)| v.mixed).map(|(k, _)| k).collect();
            if !mixed.is_empty() {
                self.counts.mixed.insert(scope.key, mixed);
            }
        }
    }

    fn type_of(&self, e: &Expression<'_>) -> Option<T> {
        use BinaryOperator as B;
        Some(match e {
            Expression::Parenthesized(p) => return self.type_of(p.expression),
            Expression::Literal(l) => match l {
                Literal::String(_) => T::Str,
                Literal::Integer(_) => T::Int,
                Literal::Float(_) => T::Float,
                Literal::True(_) | Literal::False(_) => T::Bool,
                Literal::Null(_) => T::Null,
            },
            Expression::CompositeString(_) => T::Str,
            Expression::Array(_) | Expression::LegacyArray(_) => T::Array,
            Expression::Instantiation(_) | Expression::Closure(_) | Expression::ArrowFunction(_) | Expression::AnonymousClass(_) => T::Object,
            Expression::Variable(_) => return var_name(e).and_then(|n| if n == b"this" { Some(T::Object) } else { self.known(n) }),
            Expression::UnaryPrefix(u) => {
                use UnaryPrefixOperator as O;
                match &u.operator {
                    O::IntCast(..) | O::IntegerCast(..) => T::Int,
                    O::FloatCast(..) | O::DoubleCast(..) | O::RealCast(..) => T::Float,
                    O::StringCast(..) | O::BinaryCast(..) => T::Str,
                    O::BoolCast(..) | O::BooleanCast(..) | O::Not(_) => T::Bool,
                    O::ArrayCast(..) => T::Array,
                    O::ObjectCast(..) => T::Object,
                    O::Negation(_) | O::Plus(_) => match self.type_of(u.operand)? {
                        t @ (T::Int | T::Float) => t,
                        _ => return None,
                    },
                    O::BitwiseNot(_) => T::Int,
                    _ => return None,
                }
            }
            Expression::Binary(b) => match &b.operator {
                B::StringConcat(_) => T::Str,
                B::Equal(_) | B::Identical(_) | B::NotEqual(_) | B::NotIdentical(_) | B::AngledNotEqual(_) | B::LessThan(_) | B::LessThanOrEqual(_) | B::GreaterThan(_) | B::GreaterThanOrEqual(_) | B::And(_) | B::Or(_) | B::LowAnd(_) | B::LowOr(_) | B::LowXor(_) | B::Instanceof(_) => T::Bool,
                B::Spaceship(_) | B::Modulo(_) | B::BitwiseAnd(_) | B::BitwiseOr(_) | B::BitwiseXor(_) | B::LeftShift(_) | B::RightShift(_) => T::Int,
                B::Addition(_) | B::Subtraction(_) | B::Multiplication(_) | B::Exponentiation(_) => match (self.type_of(b.lhs)?, self.type_of(b.rhs)?) {
                    (T::Int, T::Int) => T::Int,
                    (T::Int | T::Float, T::Int | T::Float) => T::Float,
                    (T::Array, T::Array) if matches!(b.operator, B::Addition(_)) => T::Array,
                    _ => return None,
                },
                // always float in the dialect (D10)
                B::Division(_) => match (self.type_of(b.lhs)?, self.type_of(b.rhs)?) {
                    (T::Int | T::Float, T::Int | T::Float) => T::Float,
                    _ => return None,
                },
                _ => return None,
            },
            _ => return None,
        })
    }

    fn assign(&mut self, name: &[u8], t: Option<T>, line: u32) {
        let Some(t) = t else {
            self.counts.not_judged += 1;
            // the first non-null value without an evident type: `mixed`
            let var = self.scope().vars.entry(name.to_vec()).or_default();
            if var.ty.is_none() {
                var.mixed = true;
                var.nullable = true;
            }
            return;
        };
        self.counts.judged += 1;
        let var = self.scope().vars.entry(name.to_vec()).or_default();
        if var.mixed {
            return;
        }
        let first = *var.first.get_or_insert(t);
        let strict_hit = first != t;
        let mut violation = None;
        if t == T::Null {
            if var.ty.is_none() && first == T::Null {
                var.nullable = true;
            } else if !var.nullable {
                violation = var.ty.map(|declared| (declared, t));
            }
        } else {
            match var.ty {
                None => var.ty = Some(t),
                Some(declared) if declared != t => violation = Some((declared, t)),
                _ => {}
            }
        }
        if strict_hit {
            self.counts.strict += 1;
        }
        if let Some((declared, assigned)) = violation {
            self.counts.tolerant += 1;
            if matches!((declared, assigned), (T::Int, T::Float) | (T::Float, T::Int)) {
                self.counts.int_float += 1;
            }
            if self.counts.lines.len() < 20 {
                self.counts.lines.push(line);
            }
            self.counts.violations.push(Violation { line, var: String::from_utf8_lossy(name).into_owned(), declared, assigned });
        }
    }

    fn line(&self, offset: u32) -> u32 {
        self.lines.line(offset)
    }
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Counter {
    fn walk_in_function(&mut self, f: &'ast Function<'arena>, _: &mut ()) {
        self.enter(Some(&f.parameter_list));
    }
    fn walk_out_function(&mut self, _: &'ast Function<'arena>, _: &mut ()) {
        self.leave();
    }
    fn walk_in_method(&mut self, m: &'ast Method<'arena>, _: &mut ()) {
        self.enter(Some(&m.parameter_list));
    }
    fn walk_out_method(&mut self, _: &'ast Method<'arena>, _: &mut ()) {
        self.leave();
    }
    fn walk_in_closure(&mut self, c: &'ast Closure<'arena>, _: &mut ()) {
        self.enter(Some(&c.parameter_list));
    }
    fn walk_out_closure(&mut self, _: &'ast Closure<'arena>, _: &mut ()) {
        self.leave();
    }
    fn walk_in_arrow_function(&mut self, a: &'ast ArrowFunction<'arena>, _: &mut ()) {
        self.enter(Some(&a.parameter_list));
    }
    fn walk_out_arrow_function(&mut self, _: &'ast ArrowFunction<'arena>, _: &mut ()) {
        self.leave();
    }
    fn walk_in_assignment(&mut self, a: &'ast Assignment<'arena>, _: &mut ()) {
        let Some(name) = var_name(a.lhs) else { return };
        if name == b"this" {
            return;
        }
        let rhs = self.type_of(a.rhs);
        let cur = self.known(name);
        let t = match &a.operator {
            AssignmentOperator::Assign(_) => rhs,
            AssignmentOperator::Concat(_) => Some(T::Str),
            AssignmentOperator::Addition(_) | AssignmentOperator::Subtraction(_) | AssignmentOperator::Multiplication(_) | AssignmentOperator::Exponentiation(_) => match (cur, rhs) {
                (Some(T::Int), Some(T::Int)) => Some(T::Int),
                (Some(T::Int | T::Float), Some(T::Int | T::Float)) => Some(T::Float),
                _ => None,
            },
            AssignmentOperator::Division(_) => match (cur, rhs) {
                (Some(T::Int | T::Float), Some(T::Int | T::Float)) => Some(T::Float),
                _ => None,
            },
            AssignmentOperator::Modulo(_) | AssignmentOperator::BitwiseAnd(_) | AssignmentOperator::BitwiseOr(_) | AssignmentOperator::BitwiseXor(_) | AssignmentOperator::LeftShift(_) | AssignmentOperator::RightShift(_) => Some(T::Int),
            AssignmentOperator::Coalesce(_) => None,
        };
        let line = self.line(a.span().start.offset);
        self.assign(name, t, line);
    }
}

/// Check / count GRD021 in a parsed program.
pub fn count_grd021(program: &Program<'_>, source: &[u8]) -> Grd021Counts {
    let mut c = Counter { scopes: vec![Scope { key: u32::MAX, ..Default::default() }], counts: Grd021Counts::default(), lines: super::LineIndex::new(source) };
    walk_program_mut(&mut c, program, &mut ());
    c.leave();
    c.counts
}
