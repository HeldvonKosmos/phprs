//! The value model.
//!
//! `Val` is small (16 bytes), `Clone` is cheap: strings and arrays share
//! their buffer through `Rc` and copy only on write; objects are handles.

mod array;
mod compare;
mod convert;
mod object;
mod string;

pub use array::{ArrayKey, PhpArray};
pub use compare::{compare, identical};
pub use convert::{format_float, parse_numeric, repr_float, Numeric};
pub use object::{ClassId, ObjHandle, ObjectStore, PhpObject};
pub use string::PhpString;

use std::rc::Rc;

#[derive(Clone, Debug, Default)]
pub enum Val {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(PhpString),
    Array(Rc<PhpArray>),
    Object(ObjHandle),
}

impl Val {
    pub fn str(s: &str) -> Val {
        Val::Str(PhpString::from(s))
    }
    pub fn bytes(b: &[u8]) -> Val {
        Val::Str(PhpString::from(b))
    }
    pub fn array(a: PhpArray) -> Val {
        Val::Array(Rc::new(a))
    }
    pub fn empty_array() -> Val {
        Val::Array(Rc::new(PhpArray::new()))
    }

    /// PHP type name as reported by `gettype`-like diagnostics.
    pub fn type_name(&self) -> &'static str {
        match self {
            Val::Null => "null",
            Val::Bool(_) => "bool",
            Val::Int(_) => "int",
            Val::Float(_) => "float",
            Val::Str(_) => "string",
            Val::Array(_) => "array",
            Val::Object(_) => "object",
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Val::Null)
    }

    /// Mutable access to an array value, copying the buffer first if it is shared.
    /// Returns `None` if the value is not an array.
    pub fn array_mut(&mut self) -> Option<&mut PhpArray> {
        match self {
            Val::Array(rc) => Some(Rc::make_mut(rc)),
            _ => None,
        }
    }
}

impl From<i64> for Val {
    fn from(v: i64) -> Self {
        Val::Int(v)
    }
}
impl From<f64> for Val {
    fn from(v: f64) -> Self {
        Val::Float(v)
    }
}
impl From<bool> for Val {
    fn from(v: bool) -> Self {
        Val::Bool(v)
    }
}
impl From<&str> for Val {
    fn from(v: &str) -> Self {
        Val::str(v)
    }
}
impl From<String> for Val {
    fn from(v: String) -> Self {
        Val::Str(PhpString::from(v))
    }
}
impl From<PhpString> for Val {
    fn from(v: PhpString) -> Self {
        Val::Str(v)
    }
}
impl From<PhpArray> for Val {
    fn from(v: PhpArray) -> Self {
        Val::array(v)
    }
}
impl From<ObjHandle> for Val {
    fn from(v: ObjHandle) -> Self {
        Val::Object(v)
    }
}
