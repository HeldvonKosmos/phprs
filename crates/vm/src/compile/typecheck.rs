//! GRD021 measurement (D8/D9, DX1): would "a local variable keeps the type
//! of its first assignment" reject this code? Counted only, never enforced.
//!
//! Static and approximate: assignments are taken in source order per scope
//! (top level, function, method, closure, arrow function); only assignments
//! whose value type is evident from the expression are judged (literals,
//! arithmetic of known numbers, concatenation, comparisons, casts, `new`,
//! array literals, variables already typed). Others count as "not judged".
//! Two variants:
//! - strict: the first assignment fixes the type, `null` included;
//! - tolerant: a first `null` makes the type `?T` with the first non-null
//!   type; only two different non-null types count.
//!
//! int → float is no widening for locals (D9): `$x = 0; $x = 1.5;` counts.

use std::collections::HashMap;

use mago_syntax::cst::*;
use mago_syntax::walker::{MutWalker, walk_program_mut};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum T {
    Null,
    Int,
    Float,
    Bool,
    Str,
    Array,
    Object,
}

/// Result of the measurement for one file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grd021Counts {
    /// Assignments that change the type (strict variant).
    pub strict: u32,
    /// Assignments that change the type (tolerant variant, `?T`).
    pub tolerant: u32,
    /// Of `strict`, the int/float changes (fix: `0.0` or `(float)`).
    pub int_float: u32,
    /// Assignments to plain variables whose type was judged.
    pub judged: u32,
    /// Assignments to plain variables whose type is not evident.
    pub not_judged: u32,
    /// Source lines of the strict hits (first 20).
    pub lines: Vec<u32>,
}

#[derive(Default)]
struct Scope {
    /// first type (strict variant)
    first: HashMap<Vec<u8>, T>,
    /// first non-null type (tolerant variant)
    first_non_null: HashMap<Vec<u8>, T>,
}

struct Counter<'s> {
    scopes: Vec<Scope>,
    counts: Grd021Counts,
    source: &'s [u8],
}

fn var_name<'a>(e: &Expression<'a>) -> Option<&'a [u8]> {
    match e {
        Expression::Variable(Variable::Direct(d)) => Some(d.name.strip_prefix(b"$").unwrap_or(d.name)),
        _ => None,
    }
}

impl Counter<'_> {
    fn scope(&mut self) -> &mut Scope {
        self.scopes.last_mut().expect("scope")
    }

    fn known(&self, name: &[u8]) -> Option<T> {
        let s = self.scopes.last()?;
        // the variable's established type: first non-null, else first
        s.first_non_null.get(name).or_else(|| s.first.get(name)).copied()
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
                // int / int is int or float depending on the values
                B::Division(_) => match (self.type_of(b.lhs)?, self.type_of(b.rhs)?) {
                    (T::Float, T::Int | T::Float) | (T::Int, T::Float) => T::Float,
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
            return;
        };
        self.counts.judged += 1;
        let scope = self.scope();
        let first = *scope.first.entry(name.to_vec()).or_insert(t);
        let strict_hit = first != t;
        let tolerant_hit = if t == T::Null {
            false
        } else {
            let fnn = *scope.first_non_null.entry(name.to_vec()).or_insert(t);
            fnn != t
        };
        if strict_hit {
            self.counts.strict += 1;
            if matches!((first, t), (T::Int, T::Float) | (T::Float, T::Int)) {
                self.counts.int_float += 1;
            }
            if self.counts.lines.len() < 20 {
                self.counts.lines.push(line);
            }
        }
        if tolerant_hit {
            self.counts.tolerant += 1;
        }
    }

    fn line(&self, offset: u32) -> u32 {
        1 + self.source[..(offset as usize).min(self.source.len())].iter().filter(|b| **b == b'\n').count() as u32
    }
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Counter<'_> {
    fn walk_in_function(&mut self, _: &'ast Function<'arena>, _: &mut ()) {
        self.scopes.push(Scope::default());
    }
    fn walk_out_function(&mut self, _: &'ast Function<'arena>, _: &mut ()) {
        self.scopes.pop();
    }
    fn walk_in_method(&mut self, _: &'ast Method<'arena>, _: &mut ()) {
        self.scopes.push(Scope::default());
    }
    fn walk_out_method(&mut self, _: &'ast Method<'arena>, _: &mut ()) {
        self.scopes.pop();
    }
    fn walk_in_closure(&mut self, _: &'ast Closure<'arena>, _: &mut ()) {
        self.scopes.push(Scope::default());
    }
    fn walk_out_closure(&mut self, _: &'ast Closure<'arena>, _: &mut ()) {
        self.scopes.pop();
    }
    fn walk_in_arrow_function(&mut self, _: &'ast ArrowFunction<'arena>, _: &mut ()) {
        self.scopes.push(Scope::default());
    }
    fn walk_out_arrow_function(&mut self, _: &'ast ArrowFunction<'arena>, _: &mut ()) {
        self.scopes.pop();
    }
    fn walk_in_assignment(&mut self, a: &'ast Assignment<'arena>, _: &mut ()) {
        use mago_span::HasSpan;
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
                (Some(T::Float), Some(T::Int | T::Float)) | (Some(T::Int), Some(T::Float)) => Some(T::Float),
                _ => None,
            },
            AssignmentOperator::Modulo(_) | AssignmentOperator::BitwiseAnd(_) | AssignmentOperator::BitwiseOr(_) | AssignmentOperator::BitwiseXor(_) | AssignmentOperator::LeftShift(_) | AssignmentOperator::RightShift(_) => Some(T::Int),
            AssignmentOperator::Coalesce(_) => None,
        };
        let line = self.line(a.span().start.offset);
        self.assign(name, t, line);
    }
}

/// Count GRD021 hits in a parsed program.
pub fn count_grd021(program: &Program<'_>, source: &[u8]) -> Grd021Counts {
    let mut c = Counter { scopes: vec![Scope::default()], counts: Grd021Counts::default(), source };
    walk_program_mut(&mut c, program, &mut ());
    c.counts
}
