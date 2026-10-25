//! `foreach` iteration over arrays, generators, Iterator/IteratorAggregate
//! objects and plain objects.

use std::rc::Rc;

use super::Vm;
use crate::error::Throw;
use crate::value::*;

pub enum IterState {
    Array { arr: Rc<PhpArray>, pos: usize },
    Generator { g: ObjHandle, first: bool },
    Iterator { obj: ObjHandle, first: bool },
    Props { entries: Vec<(Val, Val)>, pos: usize },
}

impl Vm {
    pub(crate) fn iter_init(&mut self, v: Val) -> Result<IterState, Throw> {
        match v {
            Val::Array(arr) => Ok(IterState::Array { arr, pos: 0 }),
            Val::Object(h) => {
                let class = self.objects.get(h).class;
                if class == self.wk().generator {
                    return Ok(IterState::Generator { g: h, first: true });
                }
                if self.instance_of(class, self.wk().iterator_aggregate) {
                    let inner = self.call_method_sync(h, b"getIterator", Vec::new())?;
                    return self.iter_init(inner);
                }
                if self.instance_of(class, self.wk().iterator) {
                    self.call_method_sync(h, b"rewind", Vec::new())?;
                    return Ok(IterState::Iterator { obj: h, first: true });
                }
                let scope = self.frames.last().and_then(|f| f.class);
                let obj = self.objects.get(h);
                let rt = self.class(class);
                let mut entries = Vec::new();
                for (i, p) in rt.props.iter().enumerate() {
                    let visible = match p.visibility {
                        crate::bytecode::Visibility::Public => true,
                        crate::bytecode::Visibility::Protected => scope.is_some_and(|s| self.instance_of(s, p.declaring) || self.instance_of(p.declaring, s)),
                        crate::bytecode::Visibility::Private => scope == Some(p.declaring),
                    };
                    if visible && let Some(val) = &obj.slots[i] {
                        entries.push((Val::bytes(&p.name), val.clone()));
                    }
                }
                for (k, val) in &obj.dynamic {
                    entries.push((Val::Str(k.clone()), val.clone()));
                }
                Ok(IterState::Props { entries, pos: 0 })
            }
            other => Err(Throw::type_error(format!("foreach() argument must be of type array|object, {} given", other.type_name()))),
        }
    }

    pub(crate) fn iter_next(&mut self, state: &mut IterState) -> Result<Option<(Val, Val)>, Throw> {
        match state {
            IterState::Array { arr, pos } => {
                let r = arr.entry_at(*pos).map(|(k, v)| (k.to_val(), v.clone()));
                *pos += 1;
                Ok(r)
            }
            IterState::Props { entries, pos } => {
                let r = entries.get(*pos).cloned();
                *pos += 1;
                Ok(r)
            }
            IterState::Generator { g, first } => {
                let g = *g;
                if *first {
                    *first = false;
                    self.generator_ensure_started(g)?;
                } else {
                    self.generator_resume(g, Val::Null)?;
                }
                let st = self.objects.get(g).native_ref::<super::GeneratorState>().expect("gen");
                if st.finished {
                    return Ok(None);
                }
                Ok(st.current.clone())
            }
            IterState::Iterator { obj, first } => {
                let obj = *obj;
                if !*first {
                    self.call_method_sync(obj, b"next", Vec::new())?;
                }
                *first = false;
                let valid = self.call_method_sync(obj, b"valid", Vec::new())?;
                if !valid.to_bool() {
                    return Ok(None);
                }
                let v = self.call_method_sync(obj, b"current", Vec::new())?;
                let k = self.call_method_sync(obj, b"key", Vec::new())?;
                Ok(Some((k, v)))
            }
        }
    }

    /// Materialise any iterable into (key, value) pairs.
    pub(crate) fn iterate_to_vec(&mut self, v: Val) -> Result<Vec<(Val, Val)>, Throw> {
        let mut state = self.iter_init(v)?;
        let mut out = Vec::new();
        while let Some(kv) = self.iter_next(&mut state)? {
            out.push(kv);
        }
        Ok(out)
    }

    pub(crate) fn alloc_iter(&mut self, state: IterState) -> usize {
        // a slot whose iterator is taken out while it advances (IterNext) looks
        // empty but is in use: a foreach inside the generator it resumes must
        // not get it
        let free = (0..self.iters.len()).find(|i| self.iters[*i].is_none() && !self.iters_taken.contains(i));
        if let Some(i) = free {
            self.iters[i] = Some(state);
            i
        } else {
            self.iters.push(Some(state));
            self.iters.len() - 1
        }
    }
}
