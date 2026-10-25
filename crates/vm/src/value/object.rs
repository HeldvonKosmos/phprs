//! Objects live in a per-request `ObjectStore`; values hold `ObjHandle`s.
//! See `.agents/RUNTIME_ARCHITECTURE.md` §2.

use std::any::Any;

use indexmap::IndexMap;

use super::{PhpString, Val};

/// Index into the engine's class table (phase 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct ClassId(pub u32);

/// Index into `ObjectStore::slots`; 0 is never a valid object. The top bit
/// marks engine objects (enum cases), which outlive the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjHandle(pub u32);

const ENGINE_BIT: u32 = 1 << 31;

impl ObjHandle {
    /// Engine objects (enum cases) survive `reset_request`.
    pub fn is_engine(self) -> bool {
        self.0 & ENGINE_BIT != 0
    }
}

#[derive(Debug)]
pub struct PhpObject {
    pub class: ClassId,
    /// Declared properties by slot (layout from the class) and dynamic ones by name.
    pub slots: Vec<Option<Val>>,
    pub dynamic: IndexMap<PhpString, Val>,
    /// Payload of native classes (DataFrame, query builder, request, …).
    pub native: Option<Box<dyn Any>>,
}

impl PhpObject {
    pub fn new(class: ClassId, slot_count: usize) -> Self {
        PhpObject { class, slots: vec![None; slot_count], dynamic: IndexMap::new(), native: None }
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
    /// Request objects; cleared by `reset_request`. Boxed, so a collected
    /// object (MEM1b) leaves only a null pointer behind; slots are not reused
    /// within a request, so a stale handle is an error, never another object.
    slots: Vec<Option<Box<PhpObject>>>,
    /// Engine objects (enum cases): immutable singletons that live as long as the VM.
    engine: Vec<Option<PhpObject>>,
    /// Request objects alive now (allocated minus collected).
    live: usize,
}

impl ObjectStore {
    pub fn new() -> Self {
        // Slot 0 stays empty so that ObjHandle(0) is never valid.
        ObjectStore { slots: vec![None], engine: vec![None], live: 0 }
    }
    pub fn alloc(&mut self, object: PhpObject) -> ObjHandle {
        let h = ObjHandle(self.slots.len() as u32);
        self.slots.push(Some(Box::new(object)));
        self.live += 1;
        h
    }
    /// An engine object: survives `reset_request` (RUNTIME_ARCHITECTURE §12, enum cases).
    pub fn alloc_engine(&mut self, object: PhpObject) -> ObjHandle {
        let h = ObjHandle(self.engine.len() as u32 | ENGINE_BIT);
        self.engine.push(Some(object));
        h
    }
    fn slot(&self, h: ObjHandle) -> Option<&PhpObject> {
        if h.is_engine() { self.engine.get((h.0 & !ENGINE_BIT) as usize).and_then(|s| s.as_ref()) } else { self.slots.get(h.0 as usize).and_then(|s| s.as_deref()) }
    }
    /// Panics on an invalid handle: that is an interpreter bug, not a script error.
    pub fn get(&self, h: ObjHandle) -> &PhpObject {
        self.slot(h).expect("dangling object handle")
    }
    pub fn get_mut(&mut self, h: ObjHandle) -> &mut PhpObject {
        let slot = if h.is_engine() { self.engine.get_mut((h.0 & !ENGINE_BIT) as usize).and_then(|s| s.as_mut()) } else { self.slots.get_mut(h.0 as usize).and_then(|s| s.as_deref_mut()) };
        slot.expect("dangling object handle")
    }
    pub fn is_valid(&self, h: ObjHandle) -> bool {
        self.slot(h).is_some()
    }
    /// Live request objects (engine objects not counted).
    pub fn len(&self) -> usize {
        self.live
    }
    /// Request slots handed out in this request (live and collected); handles are below this.
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }
    pub fn engine_len(&self) -> usize {
        self.engine.len() - 1
    }
    /// End of request: request objects go, engine objects stay.
    pub fn reset_request(&mut self) {
        self.slots.clear();
        self.slots.push(None);
        self.live = 0;
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// MEM1b sweep: drops every live request object whose index is not marked.
    /// Returns how many were freed.
    pub fn sweep(&mut self, marked: &[bool]) -> usize {
        let mut freed = 0;
        for (i, slot) in self.slots.iter_mut().enumerate().skip(1) {
            if slot.is_some() && !marked.get(i).copied().unwrap_or(false) {
                *slot = None;
                freed += 1;
            }
        }
        self.live -= freed;
        freed
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
        store.get_mut(b).slots[0] = Some(Val::Int(5));
        assert!(matches!(store.get(a).slots[0], Some(Val::Int(5))));
        assert_eq!(a, b);
        let c = store.clone_object(a);
        assert_ne!(a, c);
        store.get_mut(c).slots[0] = Some(Val::Int(7));
        assert!(matches!(store.get(a).slots[0], Some(Val::Int(5))));
        assert_eq!(store.len(), 2);
        assert!(!store.is_valid(ObjHandle(0)));
    }

    #[test]
    fn engine_objects_survive_reset() {
        let mut store = ObjectStore::new();
        let req = store.alloc(PhpObject::new(ClassId(1), 0));
        let eng = store.alloc_engine(PhpObject::new(ClassId(2), 0));
        assert!(eng.is_engine() && !req.is_engine());
        assert_ne!(req, eng);
        store.reset_request();
        assert!(!store.is_valid(req));
        assert!(store.is_valid(eng));
        assert_eq!(store.get(eng).class, ClassId(2));
        assert_eq!(store.len(), 0);
        assert_eq!(store.engine_len(), 1);
    }
}
