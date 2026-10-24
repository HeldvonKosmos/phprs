//! Namespace and `use` resolution.

use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct Names {
    /// Current namespace without leading/trailing backslash; empty = global.
    pub namespace: Vec<u8>,
    classes: HashMap<Vec<u8>, Vec<u8>>, // lowercase alias → fully qualified
    functions: HashMap<Vec<u8>, Vec<u8>>,
    consts: HashMap<Vec<u8>, Vec<u8>>,
}

fn strip_leading(b: &[u8]) -> &[u8] {
    b.strip_prefix(b"\\").unwrap_or(b)
}

pub(crate) fn lower(b: &[u8]) -> Vec<u8> {
    b.to_ascii_lowercase()
}

fn last_segment(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|b| *b == b'\\') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

const SPECIAL: &[&[u8]] = &[b"self", b"static", b"parent"];
const BUILTIN_TYPES: &[&[u8]] = &[
    b"int", b"float", b"string", b"bool", b"array", b"object", b"mixed", b"void", b"never", b"null", b"callable",
    b"iterable", b"true", b"false",
];

impl Names {
    pub fn new() -> Self {
        Names::default()
    }

    pub fn enter_namespace(&mut self, name: Option<&[u8]>) {
        self.namespace = name.map(|n| strip_leading(n).to_vec()).unwrap_or_default();
        self.classes.clear();
        self.functions.clear();
        self.consts.clear();
    }

    pub fn use_class(&mut self, name: &[u8], alias: Option<&[u8]>) {
        let fq = strip_leading(name).to_vec();
        let alias = alias.unwrap_or_else(|| last_segment(&fq));
        self.classes.insert(lower(alias), fq);
    }
    pub fn use_function(&mut self, name: &[u8], alias: Option<&[u8]>) {
        let fq = strip_leading(name).to_vec();
        let alias = alias.unwrap_or_else(|| last_segment(&fq));
        self.functions.insert(lower(alias), fq);
    }
    pub fn use_const(&mut self, name: &[u8], alias: Option<&[u8]>) {
        let fq = strip_leading(name).to_vec();
        let alias = alias.unwrap_or_else(|| last_segment(&fq)).to_vec();
        self.consts.insert(alias, fq);
    }

    fn prefix_namespace(&self, name: &[u8]) -> Vec<u8> {
        if self.namespace.is_empty() {
            name.to_vec()
        } else {
            let mut v = self.namespace.clone();
            v.push(b'\\');
            v.extend_from_slice(name);
            v
        }
    }

    /// Resolve a class name as written to its fully qualified form.
    /// `self`/`static`/`parent` and builtin type names are returned as-is.
    pub fn resolve_class(&self, name: &[u8]) -> Vec<u8> {
        if let Some(rest) = name.strip_prefix(b"\\") {
            return rest.to_vec();
        }
        // `namespace\Foo` is relative to the current namespace
        if name.len() > 10 && name[..10].eq_ignore_ascii_case(b"namespace\\") {
            return self.prefix_namespace(&name[10..]);
        }
        let low = lower(name);
        if SPECIAL.contains(&low.as_slice()) || BUILTIN_TYPES.contains(&low.as_slice()) {
            return low;
        }
        match name.iter().position(|b| *b == b'\\') {
            None => self.classes.get(&low).cloned().unwrap_or_else(|| self.prefix_namespace(name)),
            Some(i) => {
                let first = lower(&name[..i]);
                match self.classes.get(&first) {
                    Some(fq) => {
                        let mut v = fq.clone();
                        v.extend_from_slice(&name[i..]);
                        v
                    }
                    None => self.prefix_namespace(name),
                }
            }
        }
    }

    /// Function name resolution: (primary, global fallback for unqualified names).
    pub fn resolve_function(&self, name: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
        if let Some(rest) = name.strip_prefix(b"\\") {
            return (rest.to_vec(), None);
        }
        match name.iter().position(|b| *b == b'\\') {
            None => {
                if let Some(fq) = self.functions.get(&lower(name)) {
                    return (fq.clone(), None);
                }
                if self.namespace.is_empty() {
                    (name.to_vec(), None)
                } else {
                    (self.prefix_namespace(name), Some(name.to_vec()))
                }
            }
            Some(i) => {
                let first = lower(&name[..i]);
                match self.classes.get(&first) {
                    Some(fq) => {
                        let mut v = fq.clone();
                        v.extend_from_slice(&name[i..]);
                        (v, None)
                    }
                    None => (self.prefix_namespace(name), None),
                }
            }
        }
    }

    pub fn resolve_const(&self, name: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
        if let Some(rest) = name.strip_prefix(b"\\") {
            return (rest.to_vec(), None);
        }
        match name.iter().position(|b| *b == b'\\') {
            None => {
                if let Some(fq) = self.consts.get(name) {
                    return (fq.clone(), None);
                }
                if self.namespace.is_empty() {
                    (name.to_vec(), None)
                } else {
                    (self.prefix_namespace(name), Some(name.to_vec()))
                }
            }
            Some(i) => {
                let first = lower(&name[..i]);
                match self.classes.get(&first) {
                    Some(fq) => {
                        let mut v = fq.clone();
                        v.extend_from_slice(&name[i..]);
                        (v, None)
                    }
                    None => (self.prefix_namespace(name), None),
                }
            }
        }
    }

    /// Name a declaration (class/function) in the current namespace.
    pub fn declare(&self, name: &[u8]) -> Vec<u8> {
        self.prefix_namespace(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution() {
        let mut n = Names::new();
        n.enter_namespace(Some(b"App\\Models"));
        n.use_class(b"Illuminate\\Support\\Str", None);
        n.use_class(b"Foo\\Bar", Some(b"Baz"));
        assert_eq!(n.resolve_class(b"User"), b"App\\Models\\User");
        assert_eq!(n.resolve_class(b"Str"), b"Illuminate\\Support\\Str");
        assert_eq!(n.resolve_class(b"Baz\\Qux"), b"Foo\\Bar\\Qux");
        assert_eq!(n.resolve_class(b"\\Exception"), b"Exception");
        assert_eq!(n.resolve_class(b"self"), b"self");
        assert_eq!(n.resolve_function(b"strlen"), (b"App\\Models\\strlen".to_vec(), Some(b"strlen".to_vec())));
        assert_eq!(n.declare(b"User"), b"App\\Models\\User");
    }
}
