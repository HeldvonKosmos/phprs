//! Arithmetic and other value operations with PHP semantics under the
//! dialect's strictness (DIALECT §5).

use std::cmp::Ordering;

use super::Vm;
use crate::bytecode::{BinOp, UnOp};
use crate::error::Throw;
use crate::value::*;

fn div_by_zero() -> Throw {
    Throw::new("DivisionByZeroError", "Division by zero")
}

/// Overflow of an int operation raises `ArithmeticError` (DIALECT §5.2,
/// D10): no silent switch to float as in PHP.
#[cold]
fn overflow(sym: &str, _php_float: f64) -> Result<Val, Throw> {
    Err(Throw::new("ArithmeticError", format!("Integer overflow in {sym}")))
}

fn modulo_by_zero() -> Throw {
    Throw::new("DivisionByZeroError", "Modulo by zero")
}

pub(crate) fn arith(op: BinOp, a: &Val, b: &Val) -> Result<Val, Throw> {
    let sym = match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Pow => "**",
        _ => "?",
    };
    let (Ok(x), Ok(y)) = (a.as_number(sym), b.as_number(sym)) else { return Err(operand_types(a, sym, b)) };
    Ok(match (op, x, y) {
        (BinOp::Add, Numeric::Int(i), Numeric::Int(j)) => match i.checked_add(j) {
            Some(v) => Val::Int(v),
            None => overflow(sym, i as f64 + j as f64)?,
        },
        (BinOp::Sub, Numeric::Int(i), Numeric::Int(j)) => match i.checked_sub(j) {
            Some(v) => Val::Int(v),
            None => overflow(sym, i as f64 - j as f64)?,
        },
        (BinOp::Mul, Numeric::Int(i), Numeric::Int(j)) => match i.checked_mul(j) {
            Some(v) => Val::Int(v),
            None => overflow(sym, i as f64 * j as f64)?,
        },
        (BinOp::Div, Numeric::Int(i), Numeric::Int(j)) => {
            if j == 0 {
                return Err(div_by_zero());
            }
            // `/` is always float in the dialect (D10); `intdiv()` divides ints
            Val::Float(i as f64 / j as f64)
        }
        (BinOp::Mod, Numeric::Int(i), Numeric::Int(j)) => {
            if j == 0 {
                return Err(modulo_by_zero());
            }
            if j == -1 { Val::Int(0) } else { Val::Int(i % j) }
        }
        (BinOp::Pow, Numeric::Int(i), Numeric::Int(j)) if j >= 0 => {
            match i.checked_pow(j.min(u32::MAX as i64) as u32) {
                Some(v) if j <= u32::MAX as i64 => Val::Int(v),
                // 0, 1 and -1 never overflow, whatever the exponent
                _ if i == 0 || i == 1 => Val::Int(i),
                _ if i == -1 => Val::Int(if j % 2 == 0 { 1 } else { -1 }),
                _ => overflow(sym, (i as f64).powf(j as f64))?,
            }
        }
        (BinOp::Add, _, _) => Val::Float(x.as_f64() + y.as_f64()),
        (BinOp::Sub, _, _) => Val::Float(x.as_f64() - y.as_f64()),
        (BinOp::Mul, _, _) => Val::Float(x.as_f64() * y.as_f64()),
        (BinOp::Div, _, _) => {
            if y.as_f64() == 0.0 {
                return Err(div_by_zero());
            }
            Val::Float(x.as_f64() / y.as_f64())
        }
        (BinOp::Pow, _, _) => Val::Float(x.as_f64().powf(y.as_f64())),
        // `%` only on ints (DIALECT §5.2, D20): PHP truncates a float silently
        (BinOp::Mod, _, _) => return Err(float_operand(a, sym, b, "fmod() or (int)")),
        _ => unreachable!(),
    })
}

/// PHP's TypeError for operands of the wrong type ("Unsupported operand
/// types: array + int"); with a string the dialect adds how to convert it
/// (DIALECT §5.2: arithmetic only on int and float).
fn operand_types(a: &Val, sym: &str, b: &Val) -> Throw {
    let hint = if matches!(a, Val::Str(_)) || matches!(b, Val::Str(_)) { "; use (int)/(float)" } else { "" };
    Throw::type_error(format!("Unsupported operand types: {} {sym} {}{hint}", a.type_name(), b.type_name()))
}

/// A float operand of `%` or a bit operation (DIALECT §5.2, D20): PHP
/// truncates it to int, the dialect asks for the conversion.
fn float_operand(a: &Val, sym: &str, b: &Val, hint: &str) -> Throw {
    Throw::type_error(format!("Unsupported operand types: {} {sym} {}; use {hint}", a.type_name(), b.type_name()))
}

pub(crate) fn bitwise(op: BinOp, a: &Val, b: &Val) -> Result<Val, Throw> {
    let (sym, f): (&str, fn(i64, i64) -> Result<i64, Throw>) = match op {
        BinOp::BitAnd => ("&", |x, y| Ok(x & y)),
        BinOp::BitOr => ("|", |x, y| Ok(x | y)),
        BinOp::BitXor => ("^", |x, y| Ok(x ^ y)),
        BinOp::Shl => ("<<", |x, y| {
            if y < 0 {
                return Err(Throw::new("ArithmeticError", "Bit shift by negative number"));
            }
            Ok(if y >= 64 { 0 } else { x.wrapping_shl(y as u32) })
        }),
        BinOp::Shr => (">>", |x, y| {
            if y < 0 {
                return Err(Throw::new("ArithmeticError", "Bit shift by negative number"));
            }
            Ok(if y >= 64 { if x < 0 { -1 } else { 0 } } else { x >> y })
        }),
        _ => unreachable!(),
    };
    let (Val::Int(x), Val::Int(y)) = (a, b) else {
        if matches!(a, Val::Int(_) | Val::Float(_)) && matches!(b, Val::Int(_) | Val::Float(_)) {
            return Err(float_operand(a, sym, b, "(int)"));
        }
        return Err(operand_types(a, sym, b));
    };
    let (x, y) = (*x, *y);
    Ok(Val::Int(f(x, y)?))
}

pub(crate) fn comparison(op: BinOp, a: &Val, b: &Val) -> Result<Val, Throw> {
    Ok(match op {
        BinOp::Identical => Val::Bool(identical(a, b)),
        BinOp::NotIdentical => Val::Bool(!identical(a, b)),
        BinOp::Spaceship => match compare(a, b)? {
            Some(Ordering::Less) => Val::Int(-1),
            Some(Ordering::Equal) => Val::Int(0),
            _ => Val::Int(1),
        },
        _ => {
            let ord = compare(a, b)?;
            Val::Bool(match (op, ord) {
                (BinOp::Lt, Some(Ordering::Less)) => true,
                (BinOp::Le, Some(Ordering::Less | Ordering::Equal)) => true,
                (BinOp::Gt, Some(Ordering::Greater)) => true,
                (BinOp::Ge, Some(Ordering::Greater | Ordering::Equal)) => true,
                _ => false,
            })
        }
    })
}

impl Vm {
    pub(crate) fn binary(&mut self, op: BinOp, a: Val, b: Val) -> Result<Val, Throw> {
        match op {
            BinOp::Add if matches!((&a, &b), (Val::Array(_), Val::Array(_))) => {
                let (Val::Array(x), Val::Array(y)) = (a, b) else { unreachable!() };
                let mut out = (*x).clone();
                for (k, v) in y.iter() {
                    if !out.contains_key(k) {
                        out.insert(k.clone(), v.clone());
                    }
                }
                Ok(Val::array(out))
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => {
                // native value types (Decimal, Duration, DateTimeImmutable, …) through their handler
                if let Some(v) = self.native_arith(op, &a, &b)? {
                    return Ok(v);
                }
                arith(op, &a, &b)
            }
            BinOp::Concat => {
                let t = self.to_string_val(&b)?;
                let mut s = match a {
                    Val::Str(s) => s,
                    other => self.to_string_val(&other)?,
                };
                s.push_bytes(t.as_bytes());
                Ok(Val::Str(s))
            }
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => bitwise(op, &a, &b),
            BinOp::Identical | BinOp::NotIdentical | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Spaceship => {
                // native value types (DateTime, Decimal, RecordId, …) compare through their handler
                if let Some(ord) = self.native_compare(&a, &b)? {
                    return Ok(match op {
                        BinOp::Identical => Val::Bool(ord == Some(Ordering::Equal)),
                        BinOp::NotIdentical => Val::Bool(ord != Some(Ordering::Equal)),
                        BinOp::Spaceship => Val::Int(match ord { Some(Ordering::Less) => -1, Some(Ordering::Equal) => 0, _ => 1 }),
                        BinOp::Lt => Val::Bool(ord == Some(Ordering::Less)),
                        BinOp::Le => Val::Bool(matches!(ord, Some(Ordering::Less | Ordering::Equal))),
                        BinOp::Gt => Val::Bool(ord == Some(Ordering::Greater)),
                        BinOp::Ge => Val::Bool(matches!(ord, Some(Ordering::Greater | Ordering::Equal))),
                        _ => unreachable!(),
                    });
                }
                comparison(op, &a, &b)
            }
            BinOp::Coalesce => Ok(if a.is_null() { b } else { a }),
        }
    }

    /// `Some(value)` if one side is an object whose native class handles the operator.
    fn native_arith(&mut self, op: BinOp, a: &Val, b: &Val) -> Result<Option<Val>, Throw> {
        if let Val::Object(h) = a
            && let Some(n) = self.native_info(self.objects.get(*h).class)
            && let Some(f) = n.handlers.arith
            && let Some(v) = f(self, *h, op, b, false)?
        {
            return Ok(Some(v));
        }
        if let Val::Object(h) = b
            && let Some(n) = self.native_info(self.objects.get(*h).class)
            && let Some(f) = n.handlers.arith
        {
            return f(self, *h, op, a, true);
        }
        Ok(None)
    }

    /// `Some(ordering)` if one side is an object whose native class has a compare handler.
    fn native_compare(&mut self, a: &Val, b: &Val) -> Result<Option<Option<Ordering>>, Throw> {
        if let Val::Object(h) = a
            && let Some(n) = self.native_info(self.objects.get(*h).class)
            && let Some(cmp) = n.handlers.compare
        {
            return Ok(Some(cmp(self, *h, b)?));
        }
        if let Val::Object(h) = b
            && let Some(n) = self.native_info(self.objects.get(*h).class)
            && let Some(cmp) = n.handlers.compare
        {
            return Ok(Some(cmp(self, *h, a)?.map(Ordering::reverse)));
        }
        Ok(None)
    }

    pub(crate) fn unary(&mut self, op: UnOp, v: Val) -> Result<Val, Throw> {
        Ok(match op {
            UnOp::Neg => match v.as_number("-")? {
                Numeric::Int(i) => match i.checked_neg() {
                    Some(v) => Val::Int(v),
                    None => overflow("-", -(i as f64))?,
                },
                Numeric::Float(f) => Val::Float(-f),
            },
            UnOp::Plus => v.as_number("+")?.to_val(),
            UnOp::Not => Val::Bool(!v.to_bool()),
            UnOp::BitNot => match v {
                Val::Int(i) => Val::Int(!i),
                Val::Float(_) => return Err(Throw::type_error("Cannot perform bitwise not on float; use (int)")),
                _ => return Err(Throw::type_error(format!("Cannot perform bitwise not on {}", v.type_name()))),
            },
            UnOp::CastInt => Val::Int(v.to_int()?),
            UnOp::CastFloat => Val::Float(v.to_float()?),
            UnOp::CastBool => Val::Bool(v.to_bool()),
            UnOp::CastString => Val::Str(self.to_string_val(&v)?),
            UnOp::CastArray => match v {
                Val::Array(_) => v,
                Val::Null => Val::empty_array(),
                Val::Object(h) => Val::array(self.object_to_array(h)),
                other => Val::array(PhpArray::from_values([other])),
            },
            UnOp::CastObject => match v {
                Val::Object(_) => v,
                Val::Array(a) => {
                    let h = self.alloc_object(self.wk().stdclass)?;
                    for (k, val) in a.iter() {
                        let name = match k {
                            ArrayKey::Int(i) => PhpString::from(i.to_string()),
                            ArrayKey::Str(s) => s.clone(),
                        };
                        self.objects.get_mut(h).dynamic.insert(name, val.clone());
                    }
                    Val::Object(h)
                }
                Val::Null => Val::Object(self.alloc_object(self.wk().stdclass)?),
                other => {
                    let h = self.alloc_object(self.wk().stdclass)?;
                    self.objects.get_mut(h).dynamic.insert(PhpString::from("scalar"), other);
                    Val::Object(h)
                }
            },
        })
    }

    /// String conversion including `__toString` on objects.
    pub(crate) fn to_string_val(&mut self, v: &Val) -> Result<PhpString, Throw> {
        match v {
            Val::Object(h) => {
                let class = self.objects.get(*h).class;
                if self.class(class).find_method(b"__tostring").is_some() {
                    let r = self.call_method_sync(*h, b"__toString", Vec::new())?;
                    match r {
                        Val::Str(s) => Ok(s),
                        _ => Err(Throw::error(format!("{}::__toString(): Return value must be of type string", String::from_utf8_lossy(&self.class(class).name)))),
                    }
                } else {
                    Err(Throw::error(format!("Object of class {} could not be converted to string", String::from_utf8_lossy(&self.class(class).name))))
                }
            }
            other => other.to_php_string(),
        }
    }

    /// `(array)$obj` / `get_object_vars` view: declared then dynamic properties.
    pub(crate) fn object_to_array(&self, h: ObjHandle) -> PhpArray {
        let obj = self.objects.get(h);
        let class = self.class(obj.class);
        let mut a = PhpArray::new();
        for (i, p) in class.props.iter().enumerate() {
            if let Some(v) = &obj.slots[i] {
                a.insert(ArrayKey::Str(PhpString::from(p.name.as_slice())), v.clone());
            }
        }
        for (k, v) in &obj.dynamic {
            a.insert(ArrayKey::Str(k.clone()), v.clone());
        }
        a
    }

    pub(crate) fn inc_dec(&mut self, v: Val, inc: bool) -> Result<Val, Throw> {
        Ok(match v {
            Val::Int(i) => {
                let r = if inc { i.checked_add(1) } else { i.checked_sub(1) };
                match r {
                    Some(v) => Val::Int(v),
                    None => overflow(if inc { "++" } else { "--" }, if inc { i as f64 + 1.0 } else { i as f64 - 1.0 })?,
                }
            }
            Val::Float(f) => Val::Float(if inc { f + 1.0 } else { f - 1.0 }),
            // only int and float (DIALECT §5.2, D20): no `null++` to 1, no "a"++
            other => return Err(Throw::type_error(format!("Cannot {} {}", if inc { "increment" } else { "decrement" }, other.type_name()))),
        })
    }
}
