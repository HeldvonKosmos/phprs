//! Objects live in a per-request `ObjectStore`; values hold `ObjHandle`s.
//! See `.agents/RUNTIME_ARCHITECTURE.md` §2.

use std::any::Any;

use indexmap::IndexMap;

use super::{PhpString, Val};

/// Index into the engine's class table (phase 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClassId(pub u32);

/// Index into `ObjectStore::slots`; 0 is never a valid object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjHandle(pub u32);

#[derive(Debug)]
pub struct PhpObject {
    pub class: ClassId,
    /// Declared properties by slot (layout from the class) and dynamic ones by name.
    pub slots: Vec<Val>,
    pub dynamic: IndexMap<PhpString, Val>,
    /// Payload of native classes (DataFrame, query builder, request, …).
    pub native: Option<Box<dyn Any>>,
}

impl PhpObject {
    pub fn new(class: ClassId, slot_count: usize) -> Self {
        PhpObject { class, slots: vec![Val::Null; slot_count], dynamic: IndexMap::new(), native: None }
    }
    pub fn native_ref<T: 'static>(&self) -> Option<&T> {
        self.native.as_ref()?.downcast_ref::<T>()
    }
    pub fn native_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.native.as_mut()?.downcast_mut::<T>()
    }
}

#[derive(Debug, Default)]
pub struct ObjectStore {
    slots: Vec<Option<PhpObject>>,
}

impl ObjectStore {
    pub fn new() -> Self {
        // Slot 0 stays empty so that ObjHandle(0) is never valid.
        ObjectStore { slots: vec![None] }
    }
    pub fn alloc(&mut self, object: PhpObject) -> ObjHandle {
        let h = ObjHandle(self.slots.len() as u32);
        self.slots.push(Some(object));
        h
    }
    /// Panics on an invalid handle: that is an interpreter bug, not a script error.
    pub fn get(&self, h: ObjHandle) -> &PhpObject {
        self.slots[h.0 as usize].as_ref().expect("dangling object handle")
    }
    pub fn get_mut(&mut self, h: ObjHandle) -> &mut PhpObject {
        self.slots[h.0 as usize].as_mut().expect("dangling object handle")
    }
    pub fn is_valid(&self, h: ObjHandle) -> bool {
        matches!(self.slots.get(h.0 as usize), Some(Some(_)))
    }
    pub fn len(&self) -> usize {
        self.slots.len() - 1
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// `clone $o`: a new slot with copied properties; `__clone` is the VM's job.
    pub fn clone_object(&mut self, h: ObjHandle) -> ObjHandle {
        let src = self.get(h);
        let copy = PhpObject { class: src.class, slots: src.slots.clone(), dynamic: src.dynamic.clone(), native: None };
        self.alloc(copy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_share_identity_and_clone_does_not() {
        let mut store = ObjectStore::new();
        let a = store.alloc(PhpObject::new(ClassId(1), 1));
        let b = a; // `$b = $a`
        store.get_mut(b).slots[0] = Val::Int(5);
        assert!(matches!(store.get(a).slots[0], Val::Int(5)));
        assert_eq!(a, b);
        let c = store.clone_object(a);
        assert_ne!(a, c);
        store.get_mut(c).slots[0] = Val::Int(7);
        assert!(matches!(store.get(a).slots[0], Val::Int(5)));
        assert_eq!(store.len(), 2);
        assert!(!store.is_valid(ObjHandle(0)));
    }
}
