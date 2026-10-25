//! MEM1b: collecting request objects inside a request (ANALYSE_D13 §B).
//!
//! Objects live in the request arena (`ObjectStore`) and all go at the end of
//! the request. A request that creates many short-lived objects collects them
//! here: mark from the roots, sweep the rest. Safety rests on three rules:
//!
//! 1. Collection happens only at safepoints of the interpreter (`new`, loop
//!    back-edges), never inside native code.
//! 2. Only while every active `run` is GC-safe: a native that calls back into
//!    the VM (array_map with a closure, a magic method, jsonSerialize) may hold
//!    values in Rust locals the collector cannot see, so its nested run blocks
//!    collection. Safe are the script itself (`run_main`) and calls the host
//!    marks with `call_*_gc_safe` because everything they hold is passed in.
//! 3. Native payloads report their values through `ObjectHandlers::trace`; a
//!    live native object with a payload but no trace hook stops the collection
//!    (the safe fallback). The host context reports its values through
//!    `Vm::host_tracer`.
//!
//! Slots are not reused within a request, so a stale handle stays an error
//! ("dangling object handle") and never reaches another object.

use std::collections::HashSet;
use std::rc::Rc;

use super::generator::{Delegate, GeneratorState};
use super::iter::IterState;
use super::{ClosureData, Frame, Pending, Vm};
use crate::value::{ObjHandle, PhpArray, Val};

/// Reports the values a native payload (or the host context) holds.
pub type TraceFn = fn(&dyn std::any::Any, &mut Vec<Val>);

/// Trace hook for payloads that hold no values (dates, decimals, ids …).
pub fn trace_nothing(_: &dyn std::any::Any, _: &mut Vec<Val>) {}

/// First collection after this many live objects (`GRD_GC_THRESHOLD`, 0 = off).
pub(crate) fn default_gc_threshold() -> usize {
    std::env::var("GRD_GC_THRESHOLD").ok().and_then(|v| v.parse().ok()).unwrap_or(100_000)
}

/// Counters for tests and logs.
#[derive(Clone, Copy, Debug, Default)]
pub struct GcStats {
    pub runs: u64,
    pub freed: u64,
    /// Collections given up because a native payload had no trace hook.
    pub opaque: u64,
}

fn push_frame_roots(f: &Frame, out: &mut Vec<Val>) {
    if let Some(t) = f.this {
        out.push(Val::Object(t));
    }
    if let Some(g) = f.generator {
        out.push(Val::Object(g));
    }
    match &f.pending {
        Pending::Return(v) => out.push(v.clone()),
        Pending::Throw(h) => out.push(Val::Object(*h)),
        _ => {}
    }
    if let Some(v) = &f.return_override {
        out.push(v.clone());
    }
}

impl Vm {
    /// Called at interpreter safepoints; collects when the arena has grown
    /// past the threshold and no unsafe native callback is active.
    #[inline]
    pub(crate) fn gc_safepoint(&mut self) {
        if self.unsafe_runs == 0 && self.gc_threshold != 0 && self.objects.len() >= self.gc_next {
            self.collect_garbage();
        }
    }

    /// Collect once this many request objects are alive (0 switches it off).
    pub fn set_gc_threshold(&mut self, n: usize) {
        self.gc_threshold = n;
        self.gc_next = n;
    }

    /// Call a method from host code, allowing collection inside the call.
    /// Only for callers that hold no values except what they pass in.
    pub fn call_method_sync_gc_safe(&mut self, this: ObjHandle, name: &[u8], args: Vec<Val>) -> Result<Val, crate::error::Throw> {
        self.gc_safe_next = true;
        let r = self.call_method_sync(this, name, args);
        self.gc_safe_next = false;
        r
    }

    /// Mark and sweep now. Returns the number of freed objects, or None when
    /// a native payload without trace hook made the collection unsafe.
    pub fn collect_garbage(&mut self) -> Option<usize> {
        let n = self.objects.slot_count();
        let mut marked = vec![false; n];
        let mut work: Vec<Val> = Vec::new();
        let mut seen_arrays: HashSet<*const PhpArray> = HashSet::new();

        // roots
        work.extend(self.stack.iter().cloned());
        work.extend(self.slot_stack.iter().flatten().cloned());
        for f in &self.frames {
            push_frame_roots(f, &mut work);
        }
        for it in self.iters.iter().flatten() {
            match it {
                IterState::Array { arr, .. } => work.push(Val::Array(arr.clone())),
                IterState::Generator { g, .. } => work.push(Val::Object(*g)),
                IterState::Iterator { obj, .. } => work.push(Val::Object(*obj)),
                IterState::Props { entries, .. } => entries.iter().for_each(|(k, v)| work.extend([k.clone(), v.clone()])),
            }
        }
        work.extend(self.magic_guards.iter().map(|(h, _, _)| Val::Object(*h)));
        work.extend(self.json_visiting.iter().chain(&self.dump_visiting).map(|h| Val::Object(*h)));
        for statics in &self.static_values {
            work.extend(statics.values().cloned());
        }
        work.extend(self.constants.values().cloned());
        work.extend(self.gc_roots.iter().cloned());
        if let (Some(tracer), Some(host)) = (self.host_tracer, self.host.as_deref()) {
            tracer(host, &mut work);
        }

        // mark
        while let Some(v) = work.pop() {
            match v {
                Val::Array(a) => {
                    if seen_arrays.insert(Rc::as_ptr(&a)) {
                        work.extend(a.values().filter(|v| matches!(v, Val::Object(_) | Val::Array(_))).cloned());
                    }
                }
                Val::Object(h) if !h.is_engine() => {
                    let i = h.0 as usize;
                    if i >= n || marked[i] {
                        continue;
                    }
                    marked[i] = true;
                    let obj = self.objects.get(h);
                    work.extend(obj.slots.iter().flatten().filter(|v| matches!(v, Val::Object(_) | Val::Array(_))).cloned());
                    work.extend(obj.dynamic.values().filter(|v| matches!(v, Val::Object(_) | Val::Array(_))).cloned());
                    if let Some(payload) = obj.native.as_deref() {
                        if let Some(c) = payload.downcast_ref::<ClosureData>() {
                            match c {
                                ClosureData::User { captures, this, .. } => {
                                    work.extend(captures.iter().flatten().cloned());
                                    work.extend(this.map(Val::Object));
                                }
                                ClosureData::BoundMethod { this, .. } => work.push(Val::Object(*this)),
                                ClosureData::StaticMethod { .. } | ClosureData::Function(_) => {}
                            }
                        } else if let Some(g) = payload.downcast_ref::<GeneratorState>() {
                            if let Some(f) = &g.frame {
                                push_frame_roots(f, &mut work);
                            }
                            work.extend(g.saved_stack.iter().cloned());
                            work.extend(g.saved_slots.iter().flatten().cloned());
                            if let Some((k, v)) = &g.current {
                                work.extend([k.clone(), v.clone()]);
                            }
                            if let Some((k, v, _)) = &g.last_yield {
                                work.extend([k.clone(), v.clone()]);
                            }
                            work.push(g.return_value.clone());
                            match &g.delegate {
                                Some(Delegate::Array { arr, .. }) => work.push(Val::Array(arr.clone())),
                                Some(Delegate::Generator(d)) => work.push(Val::Object(*d)),
                                None => {}
                            }
                        } else {
                            match self.native_info(obj.class).and_then(|ni| ni.handlers.trace) {
                                Some(trace) => trace(payload, &mut work),
                                None => {
                                    // unknown payload: it may hold handles we cannot see
                                    self.gc_stats.opaque += 1;
                                    self.gc_next = self.objects.len().saturating_mul(2).max(self.gc_threshold);
                                    return None;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let freed = self.objects.sweep(&marked);
        self.gc_stats.runs += 1;
        self.gc_stats.freed += freed as u64;
        self.gc_next = self.objects.len().saturating_mul(2).max(self.gc_threshold);
        Some(freed)
    }
}
