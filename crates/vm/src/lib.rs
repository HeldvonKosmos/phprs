//! grd-vm: the OpenGrid dialect VM.
//!
//! Phase 0: front end only. `parse::parse_source` wraps mago's parser and
//! reports syntax errors. Compiler, value model and VM follow in later phases
//! (see `.agents/RUNTIME_ARCHITECTURE.md`).

pub mod parse;
