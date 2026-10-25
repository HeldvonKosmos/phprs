//! Generators: a suspended frame stored in the Generator object.

use super::{Frame, Vm};
use crate::error::Throw;
use crate::value::*;

pub enum Delegate {
    Array { arr: std::rc::Rc<PhpArray>, pos: usize },
    Generator(ObjHandle),
}

#[derive(Default)]
pub struct GeneratorState {
    pub frame: Option<Frame>,
    pub saved_stack: Vec<Val>,
    pub saved_slots: Vec<Option<Val>>,
    pub current: Option<(Val, Val)>,
    pub started: bool,
    pub finished: bool,
    pub running: bool,
    pub return_value: Val,
    pub auto_key: i64,
    pub delegate: Option<Delegate>,
    /// Set by `Yield` while suspending: `(key, value)` and whether the key was explicit.
    pub last_yield: Option<(Val, Val, bool)>,
}

impl Vm {
    pub(crate) fn make_generator(&mut self, frame: Frame, saved_slots: Vec<Option<Val>>) -> Result<ObjHandle, Throw> {
        let class = self.wk().generator;
        let mut obj = PhpObject::new(class, 0);
        let state = GeneratorState { frame: Some(frame), saved_slots, ..Default::default() };
        obj.native = Some(Box::new(state));
        self.check_object_limit()?;
        let h = self.objects.alloc(obj);
        self.objects.get_mut(h).native_mut::<GeneratorState>().expect("state").frame.as_mut().expect("frame").generator = Some(h);
        Ok(h)
    }

    fn gen_state(&mut self, h: ObjHandle) -> &mut GeneratorState {
        self.objects.get_mut(h).native_mut::<GeneratorState>().expect("generator payload")
    }

    pub(crate) fn generator_abort(&mut self, h: ObjHandle) {
        let s = self.gen_state(h);
        s.finished = true;
        s.running = false;
        s.current = None;
        s.frame = None;
    }

    /// Called by the `Yield` op (with the yielded key/value) or by `yield from`
    /// (with `None`, the delegate already set `current`): detach the running
    /// frame into the generator.
    pub(crate) fn generator_suspend(&mut self, h: ObjHandle, yielded: Option<(Option<Val>, Val)>) {
        let frame = self.frames.pop().expect("generator frame");
        let saved = self.stack.split_off(frame.stack_base);
        let saved_slots = self.slot_stack.split_off(frame.slot_base);
        let s = self.gen_state(h);
        s.saved_slots = saved_slots;
        if let Some((key, value)) = yielded {
            let key = match key {
                Some(k) => {
                    if let Val::Int(i) = k
                        && i >= s.auto_key
                    {
                        s.auto_key = i + 1;
                    }
                    k
                }
                None => {
                    let k = Val::Int(s.auto_key);
                    s.auto_key += 1;
                    k
                }
            };
            s.current = Some((key, value));
        }
        s.frame = Some(frame);
        s.saved_stack = saved;
        s.running = false;
    }

    /// Called by `Return` in a generator frame.
    pub(crate) fn generator_finish(&mut self, h: ObjHandle, value: Val) {
        let s = self.gen_state(h);
        s.finished = true;
        s.running = false;
        s.current = None;
        s.frame = None;
        s.return_value = value;
    }

    /// Resume the generator with `sent` as the result of the pending `yield`.
    pub(crate) fn generator_resume(&mut self, h: ObjHandle, sent: Val) -> Result<(), Throw> {
        // delegate (`yield from`) first
        loop {
            let s = self.gen_state(h);
            if s.finished {
                return Ok(());
            }
            if s.running {
                return Err(Throw::error("Cannot resume an already running generator"));
            }
            match s.delegate.take() {
                None => break,
                Some(Delegate::Array { arr, pos }) => {
                    let next = pos + 1;
                    if next < arr.len() {
                        let (k, v) = arr.entry_at(next).map(|(k, v)| (k.to_val(), v.clone())).expect("entry");
                        let s = self.gen_state(h);
                        s.current = Some((k, v));
                        s.delegate = Some(Delegate::Array { arr, pos: next });
                        return Ok(());
                    }
                    // exhausted: `yield from` evaluates to null, continue the frame
                    return self.generator_run(h, Val::Null);
                }
                Some(Delegate::Generator(inner)) => {
                    self.generator_ensure_started(inner)?;
                    self.generator_resume(inner, sent.clone())?;
                    let inner_state = self.gen_state(inner);
                    if !inner_state.finished {
                        let cur = inner_state.current.clone();
                        let s = self.gen_state(h);
                        s.current = cur;
                        s.delegate = Some(Delegate::Generator(inner));
                        return Ok(());
                    }
                    let rv = self.gen_state(inner).return_value.clone();
                    return self.generator_run(h, rv);
                }
            }
        }
        self.generator_run(h, sent)
    }

    /// Push the saved frame and run it until the next yield or return.
    fn generator_run(&mut self, h: ObjHandle, sent: Val) -> Result<(), Throw> {
        let (frame, saved, saved_slots) = {
            let s = self.gen_state(h);
            let frame = s.frame.take().expect("suspended frame");
            let saved = std::mem::take(&mut s.saved_stack);
            let saved_slots = std::mem::take(&mut s.saved_slots);
            s.running = true;
            s.started = true;
            (frame, saved, saved_slots)
        };
        let depth = self.frames.len();
        let mut frame = frame;
        frame.stack_base = self.stack.len();
        self.stack.extend(saved);
        frame.slot_base = self.slot_stack.len();
        self.slot_stack.extend(saved_slots);
        let resumed = frame.ip > 0;
        self.frames.push(frame);
        if resumed {
            // result of the `yield` expression
            self.push(sent);
        }
        match self.run(depth) {
            Ok(_) => {
                self.gen_state(h).running = false;
                // after a `yield from` suspended us, nothing more to do
                Ok(())
            }
            Err(t) => {
                self.generator_abort(h);
                Err(t)
            }
        }
    }

    pub(crate) fn generator_ensure_started(&mut self, h: ObjHandle) -> Result<(), Throw> {
        let started = self.gen_state(h).started;
        if !started {
            self.generator_resume(h, Val::Null)?;
        }
        Ok(())
    }

    /// `yield from`: called by the op with the iterable; the frame has
    /// already been suspended by the caller when this returns `true`.
    pub(crate) fn generator_delegate(&mut self, h: ObjHandle, iterable: Val) -> Result<Option<Val>, Throw> {
        match iterable {
            Val::Array(arr) => {
                if arr.is_empty() {
                    return Ok(Some(Val::Null));
                }
                let (k, v) = arr.entry_at(0).map(|(k, v)| (k.to_val(), v.clone())).expect("entry");
                let s = self.gen_state(h);
                s.current = Some((k, v));
                s.delegate = Some(Delegate::Array { arr, pos: 0 });
                Ok(None)
            }
            Val::Object(inner) if self.objects.get(inner).class == self.wk().generator => {
                self.generator_ensure_started(inner)?;
                if self.gen_state(inner).finished {
                    return Ok(Some(self.gen_state(inner).return_value.clone()));
                }
                let cur = self.gen_state(inner).current.clone();
                let s = self.gen_state(h);
                s.current = cur;
                s.delegate = Some(Delegate::Generator(inner));
                Ok(None)
            }
            Val::Object(o) => {
                let items = self.iterate_to_vec(Val::Object(o))?;
                let mut arr = PhpArray::new();
                for (k, v) in items {
                    match k {
                        Val::Int(i) => arr.insert(ArrayKey::Int(i), v),
                        Val::Str(s) => arr.insert(ArrayKey::Str(s), v),
                        _ => arr.push(v),
                    }
                }
                self.generator_delegate(h, Val::array(arr))
            }
            other => Err(Throw::error(format!("Can use \"yield from\" only with arrays and Traversables, {} given", other.type_name()))),
        }
    }

    /// Generator methods: current, key, next, send, valid, rewind, getReturn.
    pub(crate) fn generator_method(&mut self, h: ObjHandle, name: &[u8], args: Vec<Val>) -> Result<Val, Throw> {
        match name.to_ascii_lowercase().as_slice() {
            b"current" => {
                self.generator_ensure_started(h)?;
                Ok(self.gen_state(h).current.as_ref().map(|(_, v)| v.clone()).unwrap_or(Val::Null))
            }
            b"key" => {
                self.generator_ensure_started(h)?;
                Ok(self.gen_state(h).current.as_ref().map(|(k, _)| k.clone()).unwrap_or(Val::Null))
            }
            b"next" => {
                self.generator_ensure_started(h)?;
                self.generator_resume(h, Val::Null)?;
                Ok(Val::Null)
            }
            b"send" => {
                let v = args.into_iter().next().unwrap_or(Val::Null);
                let started = self.gen_state(h).started;
                if !started {
                    self.generator_resume(h, Val::Null)?;
                }
                self.generator_resume(h, v)?;
                Ok(self.gen_state(h).current.as_ref().map(|(_, v)| v.clone()).unwrap_or(Val::Null))
            }
            b"valid" => {
                self.generator_ensure_started(h)?;
                Ok(Val::Bool(!self.gen_state(h).finished))
            }
            b"rewind" => {
                let started = self.gen_state(h).started;
                if started && self.gen_state(h).current.is_none() {
                    return Err(Throw::new("Exception", "Cannot rewind a generator that was already run"));
                }
                self.generator_ensure_started(h)
                    .map(|_| Val::Null)
            }
            b"getreturn" => {
                let s = self.gen_state(h);
                if !s.finished {
                    return Err(Throw::new("Exception", "Cannot get return value of a generator that hasn't returned"));
                }
                Ok(s.return_value.clone())
            }
            b"throw" => {
                let v = args.into_iter().next().unwrap_or(Val::Null);
                let Val::Object(e) = v else { return Err(Throw::type_error("Generator::throw(): Argument #1 must be of type Throwable")) };
                Err(Throw::existing(e))
            }
            _ => Err(Throw::error(format!("Call to undefined method Generator::{}()", String::from_utf8_lossy(name)))),
        }
    }
}
