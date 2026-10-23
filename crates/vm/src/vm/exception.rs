//! Throwing and unwinding.

use super::{Pending, Vm};
use crate::error::Throw;
use crate::value::*;

impl Vm {
    /// Turn a `Throw` into an exception object.
    pub(crate) fn materialize(&mut self, t: Throw) -> Result<ObjHandle, Throw> {
        if let Some(h) = t.object {
            return Ok(h);
        }
        let class = self
            .lookup_class(t.class.as_bytes())
            .or_else(|| self.lookup_class(b"Error"))
            .ok_or_else(|| Throw { object: None, ..t.clone() })?;
        let args = vec![Val::from(t.message.clone()), Val::Int(t.code)];
        self.instantiate(class, args)
    }

    /// Unwind from the current frame until a handler is found. Frames below
    /// `base_depth` belong to a Rust caller: the exception is returned to it.
    pub(crate) fn unwind(&mut self, exc: ObjHandle, base_depth: usize) -> Result<(), Throw> {
        loop {
            if self.frames.len() <= base_depth {
                return Err(Throw::existing(exc));
            }
            let fidx = self.frames.len() - 1;
            let ip = self.frames[fidx].ip.saturating_sub(1) as u32;
            let module = self.frames[fidx].module.clone();
            let unit = &module.units[self.frames[fidx].unit as usize];
            let exc_class = self.objects.get(exc).class;
            for region in &unit.try_regions {
                if ip < region.start || ip >= region.end {
                    continue;
                }
                for c in &region.catches {
                    let matches = c.classes.iter().any(|cls| {
                        self.lookup_class(module.str(*cls)).is_some_and(|target| self.instance_of(exc_class, target))
                    });
                    if matches {
                        let base = self.frames[fidx].stack_base;
                        self.stack.truncate(base);
                        let sbase = self.frames[fidx].slot_base;
                        if let Some(slot) = c.slot {
                            self.slot_stack[sbase + slot as usize] = Some(Val::Object(exc));
                        }
                        let f = &mut self.frames[fidx];
                        f.ip = c.target as usize;
                        f.pending = Pending::None;
                        return Ok(());
                    }
                }
                if let Some(fin) = region.finally {
                    let base = self.frames[fidx].stack_base;
                    self.stack.truncate(base);
                    let f = &mut self.frames[fidx];
                    f.pending = Pending::Throw(exc);
                    f.ip = fin as usize;
                    return Ok(());
                }
            }
            // no handler in this frame: pop it
            let frame = self.frames.pop().expect("frame");
            self.stack.truncate(frame.stack_base);
            self.slot_stack.truncate(frame.slot_base);
            if let Some(g) = frame.generator {
                self.generator_abort(g);
            }
        }
    }

    pub(crate) fn exception_message(&self, h: ObjHandle) -> String {
        match self.get_prop_raw(h, b"message") {
            Some(Val::Str(s)) => s.to_string(),
            _ => String::new(),
        }
    }

    pub(crate) fn exception_line(&self, h: ObjHandle) -> u32 {
        match self.get_prop_raw(h, b"line") {
            Some(Val::Int(i)) => i as u32,
            _ => 0,
        }
    }
}
