//! grd-vm: the OpenGrid dialect VM.
//!
//! Layout follows `.agents/RUNTIME_ARCHITECTURE.md`:
//! - `value`: `Val` without raw pointers, strings and arrays with copy-on-write,
//!   objects as handles into a per-request `ObjectStore`.
//! - `parse`: mago front end.
//! Compiler and VM follow in later phases.

pub mod error;
pub mod parse;
pub mod value;
