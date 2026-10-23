//! PHP strings are byte strings. `PhpString` shares its buffer and copies on
//! write (`Rc::make_mut`), so `.=` on an unshared string appends in place.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

#[derive(Clone, Default)]
pub struct PhpString(Rc<Vec<u8>>);

impl PhpString {
    pub fn new() -> Self {
        PhpString(Rc::new(Vec::new()))
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// The string as `&str` if it is valid UTF-8.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }
    /// Lossy UTF-8 view for diagnostics and output.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
    /// Number of Unicode scalar values (what `mb_strlen` reports for UTF-8).
    pub fn char_len(&self) -> usize {
        String::from_utf8_lossy(&self.0).chars().count()
    }
    /// Append bytes; copies the buffer first only if it is shared.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        Rc::make_mut(&mut self.0).extend_from_slice(bytes);
    }
    pub fn push_str(&mut self, s: &str) {
        self.push_bytes(s.as_bytes());
    }
    /// True if both values share one buffer (used by tests).
    pub fn ptr_eq(a: &PhpString, b: &PhpString) -> bool {
        Rc::ptr_eq(&a.0, &b.0)
    }
}

impl From<&str> for PhpString {
    fn from(s: &str) -> Self {
        PhpString(Rc::new(s.as_bytes().to_vec()))
    }
}
impl From<String> for PhpString {
    fn from(s: String) -> Self {
        PhpString(Rc::new(s.into_bytes()))
    }
}
impl From<&[u8]> for PhpString {
    fn from(b: &[u8]) -> Self {
        PhpString(Rc::new(b.to_vec()))
    }
}
impl From<Vec<u8>> for PhpString {
    fn from(b: Vec<u8>) -> Self {
        PhpString(Rc::new(b))
    }
}

impl PartialEq for PhpString {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for PhpString {}
impl PartialOrd for PhpString {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for PhpString {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.as_slice().cmp(other.0.as_slice())
    }
}
impl Hash for PhpString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_slice().hash(state)
    }
}
impl fmt::Debug for PhpString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", String::from_utf8_lossy(&self.0))
    }
}
impl fmt::Display for PhpString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&String::from_utf8_lossy(&self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_bytes_and_chars() {
        let s = PhpString::from("äöü");
        assert_eq!(s.len(), 6);
        assert_eq!(s.char_len(), 3);
        assert_eq!(s.as_str(), Some("äöü"));
    }

    #[test]
    fn append_copies_only_when_shared() {
        let mut a = PhpString::from("ab");
        let b = a.clone();
        assert!(PhpString::ptr_eq(&a, &b));
        a.push_str("c");
        assert!(!PhpString::ptr_eq(&a, &b));
        assert_eq!(a.as_bytes(), b"abc");
        assert_eq!(b.as_bytes(), b"ab");
        // unshared: appends in place
        let before = a.0.as_ptr();
        a.push_str("d");
        assert_eq!(a.0.as_ptr(), before);
    }
}
