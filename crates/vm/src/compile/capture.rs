//! Free-variable collection for arrow functions: every `$name` used in the
//! body that has a slot in the enclosing scope is captured by value.

use mago_syntax::cst::{DirectVariable, Expression};
use mago_syntax::walker::{MutWalker, walk_expression_mut};

#[derive(Default)]
struct Collector {
    names: Vec<Vec<u8>>,
}

impl<'ast, 'arena> MutWalker<'ast, 'arena, ()> for Collector {
    fn walk_in_direct_variable(&mut self, v: &'ast DirectVariable<'arena>, _ctx: &mut ()) {
        let name = v.name.strip_prefix(b"$").unwrap_or(v.name);
        if name != b"this" && !self.names.iter().any(|n| n == name) {
            self.names.push(name.to_vec());
        }
    }
}

/// Names (without `$`) of all direct variables inside `e`, in first-use order.
pub(crate) fn variables_in(e: &Expression<'_>) -> Vec<Vec<u8>> {
    let mut c = Collector::default();
    walk_expression_mut(&mut c, e, &mut ());
    c.names
}
