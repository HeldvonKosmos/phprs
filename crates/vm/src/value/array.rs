//! PHP arrays: ordered hash maps with int or string keys.
//! Shared through `Rc<PhpArray>` in `Val`; writers call `Rc::make_mut`.

use indexmap::IndexMap;

/// Hasher of array keys (PERF1): foldhash is several times faster than the
/// default SipHash and keeps a per-process random seed, so keys from request
/// data cannot be chosen to collide.
type KeyHasher = foldhash::fast::RandomState;

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
    /// Like `from_bytes`, but a non-integer key shares the string instead of
    /// copying its bytes (PERF1: no allocation per `$a['name']`).
    pub fn from_string(s: &PhpString) -> ArrayKey {
        if let Some(i) = canonical_int(s.as_bytes()) {
            ArrayKey::Int(i)
        } else {
            ArrayKey::Str(s.clone())
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

/// Storage of a PHP array. `Packed` while the keys are exactly 0..n-1 in
/// order: values in a vector, no hashing (PHP's packed arrays); the keys are
/// kept beside them so that iteration can hand out `&ArrayKey`. The first
/// string key, gap or removal in the middle turns it into `Hash` for good.
#[derive(Clone, Debug)]
enum Store {
    Packed { keys: Vec<ArrayKey>, vals: Vec<Val> },
    Hash(IndexMap<ArrayKey, Val, KeyHasher>),
}

#[derive(Clone, Debug)]
pub struct PhpArray {
    store: Store,
    /// Next index for `$a[] = v` (PHP: one more than the largest int key ever used).
    next_free: i64,
}

impl Default for PhpArray {
    fn default() -> Self {
        PhpArray::new()
    }
}

/// Iterator over either storage (one concrete type for `impl Iterator`).
pub enum Either<A, B> {
    A(A),
    B(B),
}

impl<T, A: Iterator<Item = T>, B: Iterator<Item = T>> Iterator for Either<A, B> {
    type Item = T;
    #[inline]
    fn next(&mut self) -> Option<T> {
        match self {
            Either::A(a) => a.next(),
            Either::B(b) => b.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Either::A(a) => a.size_hint(),
            Either::B(b) => b.size_hint(),
        }
    }
}

impl<T, A: DoubleEndedIterator<Item = T>, B: DoubleEndedIterator<Item = T>> DoubleEndedIterator for Either<A, B> {
    fn next_back(&mut self) -> Option<T> {
        match self {
            Either::A(a) => a.next_back(),
            Either::B(b) => b.next_back(),
        }
    }
}

impl<T, A: ExactSizeIterator<Item = T>, B: ExactSizeIterator<Item = T>> ExactSizeIterator for Either<A, B> {}

/// Index of `key` in a packed array of length `len`.
#[inline]
fn packed_index(key: &ArrayKey, len: usize) -> Option<usize> {
    match key {
        ArrayKey::Int(i) if *i >= 0 && (*i as u64) < len as u64 => Some(*i as usize),
        _ => None,
    }
}

impl PhpArray {
    pub fn new() -> Self {
        PhpArray { store: Store::Packed { keys: Vec::new(), vals: Vec::new() }, next_free: 0 }
    }
    pub fn with_capacity(n: usize) -> Self {
        PhpArray { store: Store::Packed { keys: Vec::with_capacity(n), vals: Vec::with_capacity(n) }, next_free: 0 }
    }
    pub fn from_values(values: impl IntoIterator<Item = Val>) -> Self {
        let values = values.into_iter();
        let mut a = PhpArray::with_capacity(values.size_hint().0);
        for v in values {
            a.push(v);
        }
        a
    }
    /// Switch to hash storage (keeps order and keys).
    fn to_hash(&mut self) -> &mut IndexMap<ArrayKey, Val, KeyHasher> {
        if let Store::Packed { keys, vals } = &mut self.store {
            // REST1: keep the reserved size (an array literal reserves its
            // length; switching on the first string key must not shrink it)
            let mut map = IndexMap::with_capacity_and_hasher((vals.len() + 1).max(vals.capacity()), KeyHasher::default());
            for (k, v) in std::mem::take(keys).into_iter().zip(std::mem::take(vals)) {
                map.insert(k, v);
            }
            self.store = Store::Hash(map);
        }
        match &mut self.store {
            Store::Hash(m) => m,
            Store::Packed { .. } => unreachable!(),
        }
    }
    pub fn len(&self) -> usize {
        match &self.store {
            Store::Packed { vals, .. } => vals.len(),
            Store::Hash(m) => m.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[inline]
    pub fn get(&self, key: &ArrayKey) -> Option<&Val> {
        match &self.store {
            Store::Packed { vals, .. } => packed_index(key, vals.len()).map(|i| &vals[i]),
            Store::Hash(m) => m.get(key),
        }
    }
    #[inline]
    pub fn get_mut(&mut self, key: &ArrayKey) -> Option<&mut Val> {
        match &mut self.store {
            Store::Packed { vals, .. } => packed_index(key, vals.len()).map(|i| &mut vals[i]),
            Store::Hash(m) => m.get_mut(key),
        }
    }
    pub fn contains_key(&self, key: &ArrayKey) -> bool {
        self.get(key).is_some()
    }
    pub fn insert(&mut self, key: ArrayKey, value: Val) {
        if let ArrayKey::Int(i) = key
            && i >= self.next_free
        {
            let packed_append = matches!(&self.store, Store::Packed { vals, .. } if i as u64 == vals.len() as u64 && self.next_free == i);
            self.next_free = i.saturating_add(1);
            if packed_append && let Store::Packed { keys, vals } = &mut self.store {
                keys.push(key);
                vals.push(value);
                return;
            }
        }
        if let Store::Packed { vals, .. } = &mut self.store
            && let Some(i) = packed_index(&key, vals.len())
        {
            vals[i] = value;
            return;
        }
        self.to_hash().insert(key, value);
    }
    /// Set the value at position `i` in key order (REST1: filling an array
    /// literal's template). Out of range: no change.
    pub fn set_at(&mut self, i: usize, value: Val) {
        match &mut self.store {
            Store::Packed { vals, .. } => {
                if let Some(slot) = vals.get_mut(i) {
                    *slot = value;
                }
            }
            Store::Hash(m) => {
                if let Some((_, slot)) = m.get_index_mut(i) {
                    *slot = value;
                }
            }
        }
    }
    /// `$a[] = value`
    pub fn push(&mut self, value: Val) {
        let k = self.next_free;
        self.insert(ArrayKey::Int(k), value);
    }
    /// `unset($a[k])`; keeps the order of the remaining elements.
    pub fn remove(&mut self, key: &ArrayKey) -> Option<Val> {
        if let Store::Packed { keys, vals } = &mut self.store {
            match packed_index(key, vals.len()) {
                None => return None,
                // the last element: still 0..n-2 (next_free stays, so `[]` leaves a gap)
                Some(i) if i + 1 == vals.len() => {
                    keys.pop();
                    return vals.pop();
                }
                Some(_) => {}
            }
        }
        self.to_hash().shift_remove(key)
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&ArrayKey, &Val)> + ExactSizeIterator {
        match &self.store {
            Store::Packed { keys, vals } => Either::A(keys.iter().zip(vals.iter())),
            Store::Hash(m) => Either::B(m.iter()),
        }
    }
    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &ArrayKey> + ExactSizeIterator {
        match &self.store {
            Store::Packed { keys, .. } => Either::A(keys.iter()),
            Store::Hash(m) => Either::B(m.keys()),
        }
    }
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &Val> + ExactSizeIterator {
        match &self.store {
            Store::Packed { vals, .. } => Either::A(vals.iter()),
            Store::Hash(m) => Either::B(m.values()),
        }
    }
    pub fn values_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut Val> + ExactSizeIterator {
        match &mut self.store {
            Store::Packed { vals, .. } => Either::A(vals.iter_mut()),
            Store::Hash(m) => Either::B(m.values_mut()),
        }
    }
    pub fn entry_at(&self, index: usize) -> Option<(&ArrayKey, &Val)> {
        match &self.store {
            Store::Packed { keys, vals } => keys.get(index).zip(vals.get(index)),
            Store::Hash(m) => m.get_index(index),
        }
    }
    pub fn first(&self) -> Option<(&ArrayKey, &Val)> {
        self.entry_at(0)
    }
    pub fn last(&self) -> Option<(&ArrayKey, &Val)> {
        match &self.store {
            Store::Packed { keys, vals } => keys.last().zip(vals.last()),
            Store::Hash(m) => m.last(),
        }
    }
    pub fn last_mut(&mut self) -> Option<&mut Val> {
        match &mut self.store {
            Store::Packed { vals, .. } => vals.last_mut(),
            Store::Hash(m) => m.last_mut().map(|(_, v)| v),
        }
    }
    /// Mutable access to `key`, inserting `null` when missing.
    pub fn entry_mut(&mut self, key: ArrayKey) -> &mut Val {
        if !self.contains_key(&key) {
            self.insert(key.clone(), Val::Null);
        }
        self.get_mut(&key).expect("just inserted")
    }
    /// True if keys are exactly 0..n-1 in order (`array_is_list`).
    pub fn is_list(&self) -> bool {
        match &self.store {
            Store::Packed { .. } => true,
            Store::Hash(m) => m.keys().enumerate().all(|(i, k)| *k == ArrayKey::Int(i as i64)),
        }
    }
    pub fn clear(&mut self) {
        self.store = Store::Packed { keys: Vec::new(), vals: Vec::new() };
        self.next_free = 0;
    }
    /// Re-index with 0..n (used by `array_values`, `sort`, …).
    pub fn reindexed(&self) -> PhpArray {
        PhpArray::from_values(self.values().cloned())
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
    fn packed_until_a_gap_or_string_key() {
        let mut a = PhpArray::from_values([Val::Int(10), Val::Int(11)]);
        assert!(matches!(a.store, Store::Packed { .. }));
        a.insert(ArrayKey::Int(1), Val::Int(21));
        a.insert(ArrayKey::Int(2), Val::Int(12));
        assert!(matches!(a.store, Store::Packed { .. }) && a.len() == 3);
        assert!(matches!(a.get(&ArrayKey::Int(1)), Some(Val::Int(21))));
        assert!(matches!(a.get(&ArrayKey::from_str("1")), Some(Val::Int(21))));
        assert!(a.get(&ArrayKey::Int(-1)).is_none());
        // removing the last element keeps it packed, but the next push leaves a gap
        assert!(matches!(a.remove(&ArrayKey::Int(2)), Some(Val::Int(12))));
        assert!(matches!(a.store, Store::Packed { .. }));
        a.push(Val::Int(13));
        assert!(matches!(a.store, Store::Hash(_)));
        let keys: Vec<_> = a.keys().cloned().collect();
        assert_eq!(keys, vec![ArrayKey::Int(0), ArrayKey::Int(1), ArrayKey::Int(3)]);
        let mut b = PhpArray::from_values([Val::Int(1)]);
        b.insert(ArrayKey::from_str("k"), Val::Int(2));
        assert!(matches!(b.store, Store::Hash(_)));
        assert_eq!(b.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(), vec![ArrayKey::Int(0), ArrayKey::Str("k".into())]);
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

    /// MUT2: iteration from the back (the iterators are double-ended, packed
    /// and hashed), and `clear` empties the array and restarts the keys.
    #[test]
    fn iterates_from_the_back_and_clears() {
        let packed = PhpArray::from_values([Val::Int(1), Val::Int(2), Val::Int(3)]);
        let back: Vec<i64> = packed.iter().rev().map(|(_, v)| if let Val::Int(i) = v { *i } else { 0 }).collect();
        assert_eq!(back, vec![3, 2, 1]);
        let mut hashed = PhpArray::new();
        hashed.insert(ArrayKey::from_str("a"), Val::Int(1));
        hashed.insert(ArrayKey::from_str("b"), Val::Int(2));
        let keys: Vec<ArrayKey> = hashed.keys().rev().cloned().collect();
        assert_eq!(keys, vec![ArrayKey::from_str("b"), ArrayKey::from_str("a")]);
        hashed.clear();
        assert!(hashed.is_empty());
        hashed.push(Val::Int(7));
        assert_eq!(hashed.last().unwrap().0, &ArrayKey::Int(0));
    }
}
