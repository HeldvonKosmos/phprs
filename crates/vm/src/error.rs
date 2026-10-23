//! Errors that cross the VM boundary.

use std::fmt;

/// A PHP-level exception about to be thrown: class name plus message.
/// The VM turns it into an exception object in the store (phase 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Throw {
    pub class: String,
    pub message: String,
    pub code: i64,
    /// An already materialised exception object being re-raised.
    pub object: Option<crate::value::ObjHandle>,
}

impl Throw {
    pub fn new(class: &str, message: impl Into<String>) -> Self {
        Throw { class: class.to_string(), message: message.into(), code: 0, object: None }
    }
    pub fn type_error(message: impl Into<String>) -> Self {
        Throw::new("TypeError", message)
    }
    pub fn value_error(message: impl Into<String>) -> Self {
        Throw::new("ValueError", message)
    }
    pub fn error(message: impl Into<String>) -> Self {
        Throw::new("Error", message)
    }
    pub fn argument_count(message: impl Into<String>) -> Self {
        Throw::new("ArgumentCountError", message)
    }
    pub fn existing(object: crate::value::ObjHandle) -> Self {
        Throw { class: String::new(), message: String::new(), code: 0, object: Some(object) }
    }
}

impl fmt::Display for Throw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl From<&str> for Throw {
    fn from(m: &str) -> Self {
        Throw::new("Exception", m)
    }
}
impl From<String> for Throw {
    fn from(m: String) -> Self {
        Throw::new("Exception", m)
    }
}
