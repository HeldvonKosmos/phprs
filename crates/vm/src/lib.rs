//! grd-vm: the OpenGrid dialect VM.
//!
//! Layout follows `.agents/RUNTIME_ARCHITECTURE.md`:
//! - `value`: `Val` without raw pointers, strings and arrays with copy-on-write,
//!   objects as handles into a per-request `ObjectStore`.
//! - `parse`: mago front end.
//! Compiler and VM follow in later phases.

#![forbid(unsafe_code)]

pub mod builtins;
pub mod bytecode;
pub mod compile;
pub mod error;
pub mod parse;
pub mod value;
pub mod vm;

/// Framework edge: JSON conversion of dialect values.
pub mod json {
    pub use crate::builtins::json::{from_json, to_json};
}
