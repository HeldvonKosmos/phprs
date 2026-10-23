//! Front end: parse `.grd` source into mago's CST.

use std::borrow::Cow;

use mago_allocator::LocalArena;
use mago_database::file::File;
use mago_syntax::parser::parse_file;

/// One syntax error with a byte offset into the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub offset: u32,
    pub message: String,
}

/// Parse `source` (UTF-8 bytes) and return the syntax errors mago reports.
/// An empty vector means the file parsed completely.
///
/// The CST itself lives in an arena tied to this call; later phases keep the
/// arena alive for compilation. For phase 0 only the error list is needed.
pub fn syntax_errors(name: &str, source: &[u8]) -> Vec<SyntaxError> {
    let arena = LocalArena::new();
    let file = File::ephemeral(Cow::Owned(name.as_bytes().to_vec()), Cow::Owned(crate::compile::normalize_open_tag(source)));
    let program = parse_file(&arena, &file);
    program
        .errors
        .iter()
        .map(|e| SyntaxError {
            offset: mago_span::HasSpan::span(e).start.offset,
            message: e.to_string(),
        })
        .collect()
}

/// Convert a byte offset into a 1-based line number.
pub fn line_of(source: &[u8], offset: u32) -> usize {
    source[..(offset as usize).min(source.len())].iter().filter(|b| **b == b'\n').count() + 1
}
