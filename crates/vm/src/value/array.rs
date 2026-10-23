//! PHP arrays: ordered hash maps with int or string keys.
//! Shared through `Rc<PhpArray>` in `Val`; writers call `Rc::make_mut`.

use indexmap::IndexMap;

use super::{PhpString, Val};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ArrayKey {
    Int(i64),
    Str(PhpString),
}

impl ArrayKey {
    /// Normalise a string key the way PHP does: canonical decimal integers
    /// (`"1"`, `"-5"`, not `"01"`, `"1.0"`, `"-0"`) become int keys.
    pub fn from_bytes(b: &[u8]) -> ArrayKey {
        if let Some(i) = canonical_int(b) {
            ArrayKey::Int(i)
        } else {
            ArrayKey::Str(PhpString::from(b))
        }
    }
    pub fn from_str(s: &str) -> ArrayKey {
        ArrayKey::from_bytes(s.as_bytes())
    }
    pub fn to_val(&self) -> Val {
        match self {
            ArrayKey::Int(i) => Val::Int(*i),
            ArrayKey::Str(s) => Val::Str(s.clone()),
        }
    }
}

fn canonical_int(b: &[u8]) -> Option<i64> {
    if b.is_empty() || b.len() > 20 {
        return None;
    }
    let (neg, digits) = if b[0] == b'-' { (true, &b[1..]) } else { (false, b) };
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if digits.len() > 1 && digits[0] == b'0' {
        return None;
    }
    if neg && digits == b"0" {
        return None;
    }
    let s = std::str::from_utf8(b).ok()?;
    s.parse::<i64>().ok()
}

#[derive(Clone, Debug, Default)]
pub struct PhpArray {
    map: IndexMap<ArrayKey, Val>,
    /// Next index for `$a[] = v` (PHP: one more than the largest int key ever used).
    next_free: i64,
}

impl PhpArray {
    pub fn new() -> Self {
        PhpArray { map: IndexMap::new(), next_free: 0 }
    }
    pub fn with_capacity(n: usize) -> Self {
        PhpArray { map: IndexMap::with_capacity(n), next_free: 0 }
    }
    pub fn from_values(values: impl IntoIterator<Item = Val>) -> Self {
        let mut a = PhpArray::new();
        for v in values {
            a.push(v);
        }
        a
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn get(&self, key: &ArrayKey) -> Option<&Val> {
        self.map.get(key)
    }
    pub fn get_mut(&mut self, key: &ArrayKey) -> Option<&mut Val> {
        self.map.get_mut(key)
    }
    pub fn contains_key(&self, key: &ArrayKey) -> bool {
        self.map.contains_key(key)
    }
    pub fn insert(&mut self, key: ArrayKey, value: Val) {
        if let ArrayKey::Int(i) = key
            && i >= self.next_free
        {
            self.next_free = i.saturating_add(1);
        }
        self.map.insert(key, value);
    }
    /// `$a[] = value`
    pub fn push(&mut self, value: Val) {
        let k = self.next_free;
        self.insert(ArrayKey::Int(k), value);
    }
    /// `unset($a[k])`; keeps the order of the remaining elements.
    pub fn remove(&mut self, key: &ArrayKey) -> Option<Val> {
        self.map.shift_remove(key)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&ArrayKey, &Val)> {
        self.map.iter()
    }
    pub fn keys(&self) -> impl Iterator<Item = &ArrayKey> {
        self.map.keys()
    }
    pub fn values(&self) -> impl Iterator<Item = &Val> {
        self.map.values()
    }
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut Val> {
        self.map.values_mut()
    }
    pub fn entry_at(&self, index: usize) -> Option<(&ArrayKey, &Val)> {
        self.map.get_index(index)
    }
    pub fn first(&self) -> Option<(&ArrayKey, &Val)> {
        self.map.first()
    }
    pub fn last(&self) -> Option<(&ArrayKey, &Val)> {
        self.map.last()
    }
    /// True if keys are exactly 0..n-1 in order (`array_is_list`).
    pub fn is_list(&self) -> bool {
        self.map.keys().enumerate().all(|(i, k)| *k == ArrayKey::Int(i as i64))
    }
    pub fn clear(&mut self) {
        self.map.clear();
        self.next_free = 0;
    }
    /// Re-index with 0..n (used by `array_values`, `sort`, …).
    pub fn reindexed(&self) -> PhpArray {
        PhpArray::from_values(self.map.values().cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[test]
    fn key_normalisation() {
        assert_eq!(ArrayKey::from_str("1"), ArrayKey::Int(1));
        assert_eq!(ArrayKey::from_str("-5"), ArrayKey::Int(-5));
        assert_eq!(ArrayKey::from_str("01"), ArrayKey::Str("01".into()));
        assert_eq!(ArrayKey::from_str("-0"), ArrayKey::Str("-0".into()));
        assert_eq!(ArrayKey::from_str("1.0"), ArrayKey::Str("1.0".into()));
        assert_eq!(ArrayKey::from_str(""), ArrayKey::Str("".into()));
    }

    #[test]
    fn push_follows_largest_int_key() {
        let mut a = PhpArray::new();
        a.insert(ArrayKey::Int(5), Val::Int(1));
        a.push(Val::Int(2));
        a.insert(ArrayKey::from_str("k"), Val::Int(3));
        a.push(Val::Int(4));
        let keys: Vec<_> = a.keys().cloned().collect();
        assert_eq!(keys, vec![ArrayKey::Int(5), ArrayKey::Int(6), ArrayKey::Str("k".into()), ArrayKey::Int(7)]);
        assert!(!a.is_list());
        assert!(a.reindexed().is_list());
    }

    #[test]
    fn copy_on_write_only_on_write() {
        let mut a = Val::array(PhpArray::from_values([Val::Int(1), Val::Int(2)]));
        let b = a.clone();
        if let (Val::Array(x), Val::Array(y)) = (&a, &b) {
            assert!(Rc::ptr_eq(x, y));
        }
        a.array_mut().unwrap().push(Val::Int(3));
        if let (Val::Array(x), Val::Array(y)) = (&a, &b) {
            assert!(!Rc::ptr_eq(x, y));
            assert_eq!(x.len(), 3);
            assert_eq!(y.len(), 2);
        }
    }

    #[test]
    fn unset_keeps_order() {
        let mut a = PhpArray::from_values([Val::Int(1), Val::Int(2), Val::Int(3)]);
        a.remove(&ArrayKey::Int(1));
        let keys: Vec<_> = a.keys().cloned().collect();
        assert_eq!(keys, vec![ArrayKey::Int(0), ArrayKey::Int(2)]);
        a.push(Val::Int(9));
        assert_eq!(a.last().unwrap().0, &ArrayKey::Int(3));
    }
}
