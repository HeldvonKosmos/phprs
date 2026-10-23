//! Comparisons per DIALECT §5.1: `==` is `===`; ordering only between two
//! numbers or two strings (bytewise); anything else is a `TypeError`.

use std::cmp::Ordering;
use std::rc::Rc;

use super::Val;
use crate::error::Throw;

/// `===`
pub fn identical(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Null, Val::Null) => true,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::Float(x), Val::Float(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => x == y,
        (Val::Object(x), Val::Object(y)) => x == y,
        (Val::Array(x), Val::Array(y)) => {
            if Rc::ptr_eq(x, y) {
                return true;
            }
            if x.len() != y.len() {
                return false;
            }
            x.iter().zip(y.iter()).all(|((ka, va), (kb, vb))| ka == kb && identical(va, vb))
        }
        _ => false,
    }
}

/// Ordering for `<`, `>`, `<=`, `>=`, `<=>`. `Ok(None)` means unordered
/// (NaN involved): relational operators are false, spaceship yields 1.
pub fn compare(a: &Val, b: &Val) -> Result<Option<Ordering>, Throw> {
    match (a, b) {
        (Val::Int(x), Val::Int(y)) => Ok(Some(x.cmp(y))),
        (Val::Int(x), Val::Float(y)) => Ok((*x as f64).partial_cmp(y)),
        (Val::Float(x), Val::Int(y)) => Ok(x.partial_cmp(&(*y as f64))),
        (Val::Float(x), Val::Float(y)) => Ok(x.partial_cmp(y)),
        (Val::Str(x), Val::Str(y)) => Ok(Some(x.cmp(y))),
        _ => Err(Throw::type_error(format!(
            "Cannot compare {} with {}",
            a.type_name(),
            b.type_name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{ObjHandle, PhpArray};

    #[test]
    fn strict_equality() {
        assert!(!identical(&Val::Int(1), &Val::Float(1.0)));
        assert!(!identical(&Val::str("1"), &Val::Int(1)));
        assert!(!identical(&Val::Null, &Val::Bool(false)));
        assert!(identical(&Val::str("a"), &Val::str("a")));
        assert!(identical(&Val::Object(ObjHandle(3)), &Val::Object(ObjHandle(3))));
        assert!(!identical(&Val::Object(ObjHandle(3)), &Val::Object(ObjHandle(4))));
        let a = Val::array(PhpArray::from_values([Val::Int(1), Val::str("x")]));
        let b = Val::array(PhpArray::from_values([Val::Int(1), Val::str("x")]));
        let c = Val::array(PhpArray::from_values([Val::Int(1), Val::Int(2)]));
        assert!(identical(&a, &b));
        assert!(!identical(&a, &c));
        assert!(!identical(&Val::Float(f64::NAN), &Val::Float(f64::NAN)));
    }

    #[test]
    fn ordering_rules() {
        assert_eq!(compare(&Val::Int(1), &Val::Float(1.5)).unwrap(), Some(Ordering::Less));
        assert_eq!(compare(&Val::str("10"), &Val::str("9")).unwrap(), Some(Ordering::Less)); // bytewise
        assert!(compare(&Val::Int(1), &Val::str("1")).is_err());
        assert!(compare(&Val::Null, &Val::Int(0)).is_err());
        assert_eq!(compare(&Val::Float(f64::NAN), &Val::Int(1)).unwrap(), None);
    }
}
