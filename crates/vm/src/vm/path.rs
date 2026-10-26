//! Lvalue paths: `$a['x'][] = v`, `$this->items[$k]->v = 1`, `unset(...)`.

use super::Vm;
use crate::bytecode::*;
use crate::error::Throw;
use crate::value::*;

/// Where a write starts.
enum Loc {
    Slot(Slot),
    Obj(ObjHandle, Vec<u8>),
    Static(ClassId, Vec<u8>),
    Temp(Val),
}

impl Vm {
    pub(crate) fn to_array_key(&self, v: &Val) -> Result<ArrayKey, Throw> {
        Ok(match v {
            Val::Int(i) => ArrayKey::Int(*i),
            Val::Str(s) => ArrayKey::from_string(s),
            Val::Bool(b) => ArrayKey::Int(*b as i64),
            Val::Float(f) => ArrayKey::Int(*f as i64),
            Val::Null => ArrayKey::Str(PhpString::new()),
            other => return Err(Throw::type_error(format!("Illegal offset type {}", other.type_name()))),
        })
    }

    /// Apply a write mode: returns (stored value, expression result).
    fn apply_mode(&mut self, mode: WriteMode, old: Val, value: Val) -> Result<(Val, Val), Throw> {
        Ok(match mode {
            WriteMode::Assign => (value.clone(), value),
            WriteMode::Compound(op) => {
                let new = self.binary(op, old, value)?;
                (new.clone(), new)
            }
            WriteMode::PreInc => {
                let new = self.inc_dec(old, true)?;
                (new.clone(), new)
            }
            WriteMode::PreDec => {
                let new = self.inc_dec(old, false)?;
                (new.clone(), new)
            }
            WriteMode::PostInc => {
                let new = self.inc_dec(old.clone(), true)?;
                (new, old)
            }
            WriteMode::PostDec => {
                let new = self.inc_dec(old.clone(), false)?;
                (new, old)
            }
        })
    }

    fn resolve_base(&mut self, base: &PathBase, module: u32) -> Result<Loc, Throw> {
        Ok(match base {
            PathBase::Slot(s) => Loc::Slot(*s),
            PathBase::This => {
                let this = self.frame().this.ok_or_else(|| Throw::error("Using $this when not in object context"))?;
                Loc::Temp(Val::Object(this))
            }
            PathBase::StaticProp(cr, name) => {
                let class = self.resolve_class_ref(cr, module)?;
                Loc::Static(class, self.str_of(module, *name).to_vec())
            }
            PathBase::Stack => Loc::Temp(self.pop()),
        })
    }

    /// Stack: [base] keys… value → result
    pub(crate) fn store_path(&mut self, base: &PathBase, segs: &[PathSeg], mode: WriteMode, module: u32) -> Result<Val, Throw> {
        let value = self.pop();
        let n_keys = segs.iter().filter(|s| matches!(s, PathSeg::Dim | PathSeg::PropDyn)).count();
        let keys = self.pop_n(n_keys);
        let mut loc = self.resolve_base(base, module)?;
        let mut key_i = 0usize;
        let mut i = 0usize;
        loop {
            let run_end = segs[i..].iter().position(|s| matches!(s, PathSeg::Prop(_) | PathSeg::PropSel(_) | PathSeg::PropDyn)).map(|p| i + p).unwrap_or(segs.len());
            let dims = &segs[i..run_end];
            let n_dim_keys = dims.iter().filter(|s| matches!(s, PathSeg::Dim)).count();
            let dim_keys: Vec<Val> = keys[key_i..key_i + n_dim_keys].to_vec();
            key_i += n_dim_keys;
            if run_end == segs.len() {
                return self.write_at(loc, dims, &dim_keys, mode, value);
            }
            let prop_name = match &segs[run_end] {
                PathSeg::Prop(id) => self.str_of(module, *id).to_vec(),
                PathSeg::PropSel(sel) => self.sel_name(*sel).to_vec(),
                PathSeg::PropDyn => {
                    let k = keys[key_i].clone();
                    key_i += 1;
                    self.to_string_val(&k)?.as_bytes().to_vec()
                }
                _ => unreachable!(),
            };
            let target = self.read_at(&loc, dims, &dim_keys)?;
            match target {
                Val::Object(h) => loc = Loc::Obj(h, prop_name),
                other => {
                    return Err(Throw::error(format!(
                        "Attempt to assign property \"{}\" on {}",
                        String::from_utf8_lossy(&prop_name),
                        other.type_name()
                    )));
                }
            }
            i = run_end + 1;
        }
    }

    fn read_at(&mut self, loc: &Loc, dims: &[PathSeg], keys: &[Val]) -> Result<Val, Throw> {
        let mut cur = match loc {
            Loc::Slot(s) => self.slot_stack[self.frames.last().expect("frame").slot_base + *s as usize].clone().unwrap_or(Val::Null),
            Loc::Obj(h, name) => self.read_prop(*h, name, false)?,
            Loc::Static(c, name) => self.static_prop_get(*c, name)?,
            Loc::Temp(v) => v.clone(),
        };
        let mut ki = 0;
        for seg in dims {
            match seg {
                PathSeg::Dim => {
                    let k = &keys[ki];
                    ki += 1;
                    cur = self.fetch_dim(cur, k.clone(), false)?;
                }
                PathSeg::Append => return Err(Throw::error("Cannot use [] for reading")),
                _ => unreachable!(),
            }
        }
        Ok(cur)
    }

    fn take_loc(&mut self, loc: &Loc) -> Result<Val, Throw> {
        Ok(match loc {
            Loc::Slot(s) => self.slot_stack[self.frames.last().expect("frame").slot_base + *s as usize].take().unwrap_or(Val::Null),
            Loc::Obj(h, name) => {
                let class = self.objects.get(*h).class;
                let scope = self.scope_class();
                match self.lookup_prop(class, name, scope) {
                    super::class::PropLookup::Slot(idx) => self.objects.get_mut(*h).slots[idx].take().unwrap_or(Val::Null),
                    super::class::PropLookup::Undeclared => {
                        let key = PhpString::from(name.as_slice());
                        if let Some(v) = self.objects.get_mut(*h).dynamic.get_mut(&key) {
                            std::mem::take(v)
                        } else if self.class(class).find_method(b"__get").is_some() && !self.magic_guarded(*h, name, MAGIC_GET) {
                            return Err(Throw::error(format!("Indirect modification of overloaded property {}::${}", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(name))));
                        } else {
                            Val::Null
                        }
                    }
                    super::class::PropLookup::Inaccessible => {
                        return Err(Throw::error(format!("Cannot access property {}::${}", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(name))));
                    }
                }
            }
            Loc::Static(c, name) => {
                self.static_prop_get(*c, name)?;
                let owner = self.static_prop_owner(*c, name).map(|(o, _)| o).expect("static");
                self.static_values[owner.0 as usize].remove(name).unwrap_or(Val::Null)
            }
            Loc::Temp(v) => match v {
                Val::Object(_) => v.clone(),
                _ => return Err(Throw::error("Cannot use temporary expression in write context")),
            },
        })
    }

    fn put_loc(&mut self, loc: &Loc, v: Val) -> Result<(), Throw> {
        match loc {
            Loc::Slot(s) => self.slot_stack[self.frames.last().expect("frame").slot_base + *s as usize] = Some(v),
            Loc::Obj(h, name) => {
                let class = self.objects.get(*h).class;
                let scope = self.scope_class();
                match self.lookup_prop(class, name, scope) {
                    super::class::PropLookup::Slot(idx) => self.objects.get_mut(*h).slots[idx] = Some(v),
                    _ => {
                        if class == self.wk().stdclass || self.objects.get(*h).dynamic.contains_key(&PhpString::from(name.as_slice())) {
                            self.objects.get_mut(*h).dynamic.insert(PhpString::from(name.as_slice()), v);
                        } else {
                            return Err(Throw::error(format!("Cannot create dynamic property {}::${}", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(name))));
                        }
                    }
                }
            }
            Loc::Static(c, name) => self.static_prop_set(*c, name, v)?,
            Loc::Temp(_) => {}
        }
        Ok(())
    }

    fn write_at(&mut self, loc: Loc, dims: &[PathSeg], keys: &[Val], mode: WriteMode, value: Val) -> Result<Val, Throw> {
        if dims.is_empty() {
            return match loc {
                Loc::Slot(s) => {
                    let old = self.slot_stack[self.frames.last().expect("frame").slot_base + s as usize].clone().unwrap_or(Val::Null);
                    let (new, result) = self.apply_mode(mode, old, value)?;
                    self.slot_stack[self.frames.last().expect("frame").slot_base + s as usize] = Some(new);
                    Ok(result)
                }
                Loc::Obj(h, name) => self.write_prop(h, &name, mode, value),
                Loc::Static(c, name) => {
                    let old = self.static_prop_get(c, &name)?;
                    let (new, result) = self.apply_mode(mode, old, value)?;
                    self.static_prop_set(c, &name, new)?;
                    Ok(result)
                }
                Loc::Temp(_) => Err(Throw::error("Cannot use temporary expression in write context")),
            };
        }
        let mut root = self.take_loc(&loc)?;
        let r = self.write_dims(&mut root, dims, keys, mode, value);
        self.put_loc(&loc, root)?;
        r
    }

    fn write_dims(&mut self, root: &mut Val, dims: &[PathSeg], keys: &[Val], mode: WriteMode, value: Val) -> Result<Val, Throw> {
        let mut cur: &mut Val = root;
        let mut ki = 0usize;
        for (j, seg) in dims.iter().enumerate() {
            let last = j + 1 == dims.len();
            match cur {
                Val::Object(h) => {
                    let h = *h;
                    if !last {
                        return Err(Throw::error("Indirect modification of overloaded element"));
                    }
                    let class = self.objects.get(h).class;
                    if !self.instance_of(class, self.wk().array_access) {
                        return Err(Throw::error("Cannot use object as array"));
                    }
                    let key = match seg {
                        PathSeg::Dim => keys[ki].clone(),
                        _ => Val::Null,
                    };
                    let old = match mode {
                        WriteMode::Assign => Val::Null,
                        _ => self.call_method_sync(h, b"offsetGet", vec![key.clone()])?,
                    };
                    let (new, result) = self.apply_mode(mode, old, value)?;
                    self.call_method_sync(h, b"offsetSet", vec![key, new])?;
                    return Ok(result);
                }
                Val::Null => *cur = Val::empty_array(),
                Val::Array(_) => {}
                Val::Str(_) => return Err(Throw::error("String offset assignment is not supported")),
                other => return Err(Throw::error(format!("Cannot use a scalar value ({}) as an array", other.type_name()))),
            }
            let arr = cur.array_mut().expect("array");
            match seg {
                PathSeg::Append => {
                    if last {
                        let (new, result) = self.apply_mode(mode, Val::Null, value)?;
                        arr.push(new);
                        return Ok(result);
                    }
                    arr.push(Val::Null);
                    cur = arr.last_mut().expect("pushed");
                }
                PathSeg::Dim => {
                    let key = self.to_array_key(&keys[ki])?;
                    ki += 1;
                    if last {
                        let old = match mode {
                            WriteMode::Assign => Val::Null,
                            _ => arr.get(&key).cloned().unwrap_or(Val::Null),
                        };
                        let (new, result) = self.apply_mode(mode, old, value)?;
                        arr.insert(key, new);
                        return Ok(result);
                    }
                    cur = arr.entry_mut(key);
                }
                _ => unreachable!(),
            }
        }
        unreachable!("dims not empty")
    }

    /// Direct property write with visibility, readonly and `__set`.
    pub(crate) fn write_prop(&mut self, h: ObjHandle, name: &[u8], mode: WriteMode, value: Val) -> Result<Val, Throw> {
        let class = self.objects.get(h).class;
        let scope = self.scope_class();
        match self.lookup_prop(class, name, scope) {
            super::class::PropLookup::Slot(idx) => {
                let (readonly, declaring, ty, info_module) = {
                    let info = &self.class(class).props[idx];
                    (info.readonly, info.declaring, info.ty.clone(), info.module)
                };
                if readonly {
                    let initialised = self.objects.get(h).slots[idx].is_some();
                    if scope != Some(declaring) || initialised {
                        return Err(Throw::error(format!("Cannot modify readonly property {}::${}", String::from_utf8_lossy(&self.class(class).name), String::from_utf8_lossy(name))));
                    }
                }
                let old = self.objects.get(h).slots[idx].clone().unwrap_or(Val::Null);
                let (new, result) = self.apply_mode(mode, old, value)?;
                let new = match &ty {
                    Some(t) if !scalar_type_matches(t, &new) => {
                        let module = self.modules[info_module as usize].clone();
                        match self.check_type(&new, t, &module, Some(declaring)) {
                            Some(v) => v,
                            None => {
                                return Err(Throw::type_error(format!(
                                    "Cannot assign {} to property {}::${} of type {}",
                                    self.value_type_name(&new),
                                    String::from_utf8_lossy(&self.class(class).name),
                                    String::from_utf8_lossy(name),
                                    self.type_name_of(t, &module)
                                )));
                            }
                        }
                    }
                    _ => new,
                };
                self.objects.get_mut(h).slots[idx] = Some(new);
                Ok(result)
            }
            lookup => {
                if let Some(n) = self.native_info(class)
                    && let Some(set) = n.handlers.set
                {
                    let old = match mode {
                        WriteMode::Assign => Val::Null,
                        _ => self.read_prop(h, name, true)?,
                    };
                    let (new, result) = self.apply_mode(mode, old, value.clone())?;
                    if set(self, h, name, new)? {
                        return Ok(result);
                    }
                }
                if self.class(class).find_method(b"__set").is_some() && !self.magic_guarded(h, name, MAGIC_SET) {
                    let old = match mode {
                        WriteMode::Assign => Val::Null,
                        _ => self.read_prop(h, name, true)?,
                    };
                    let (new, result) = self.apply_mode(mode, old, value)?;
                    self.call_magic(h, b"__set", MAGIC_SET, name, vec![Val::bytes(name), new])?;
                    return Ok(result);
                }
                match lookup {
                    super::class::PropLookup::Inaccessible => Err(Throw::error(format!(
                        "Cannot access property {}::${}",
                        String::from_utf8_lossy(&self.class(class).name),
                        String::from_utf8_lossy(name)
                    ))),
                    _ => {
                        let key = PhpString::from(name);
                        if class == self.wk().stdclass || self.objects.get(h).dynamic.contains_key(&key) {
                            let old = self.objects.get(h).dynamic.get(&key).cloned().unwrap_or(Val::Null);
                            let (new, result) = self.apply_mode(mode, old, value)?;
                            self.objects.get_mut(h).dynamic.insert(key, new);
                            Ok(result)
                        } else {
                            Err(Throw::error(format!(
                                "Cannot create dynamic property {}::${}",
                                String::from_utf8_lossy(&self.class(class).name),
                                String::from_utf8_lossy(name)
                            )))
                        }
                    }
                }
            }
        }
    }

    /// Property read with visibility and `__get`.
    pub(crate) fn read_prop(&mut self, h: ObjHandle, name: &[u8], quiet: bool) -> Result<Val, Throw> {
        let class = self.objects.get(h).class;
        let scope = self.scope_class();
        match self.lookup_prop(class, name, scope) {
            super::class::PropLookup::Slot(idx) => match &self.objects.get(h).slots[idx] {
                Some(v) => Ok(v.clone()),
                None => {
                    if quiet {
                        return Ok(Val::Null);
                    }
                    let info = &self.class(class).props[idx];
                    if info.ty.is_some() {
                        Err(Throw::error(format!(
                            "Typed property {}::${} must not be accessed before initialization",
                            String::from_utf8_lossy(&self.class(info.declaring).name),
                            String::from_utf8_lossy(name)
                        )))
                    } else {
                        Ok(Val::Null)
                    }
                }
            },
            lookup => {
                if matches!(lookup, super::class::PropLookup::Undeclared)
                    && let Some(v) = self.objects.get(h).dynamic.get(&PhpString::from(name))
                {
                    return Ok(v.clone());
                }
                if let Some(n) = self.native_info(class) {
                    if quiet && let Some(isset) = n.handlers.isset
                        && let Some(set) = isset(self, h, name)?
                        && !set
                    {
                        return Ok(Val::Null);
                    }
                    if let Some(get) = n.handlers.get
                        && let Some(v) = get(self, h, name)?
                    {
                        return Ok(v);
                    }
                }
                // inside __get/__isset for this property of this object the property is
                // read directly (PHP's guards), so a magic method can read its own name
                let has_get = self.class(class).find_method(b"__get").is_some() && !self.magic_guarded(h, name, MAGIC_GET);
                let has_isset = self.class(class).find_method(b"__isset").is_some() && !self.magic_guarded(h, name, MAGIC_ISSET);
                if quiet && has_isset {
                    let set = self.call_magic(h, b"__isset", MAGIC_ISSET, name, vec![Val::bytes(name)])?;
                    if !set.to_bool() {
                        return Ok(Val::Null);
                    }
                    if !has_get {
                        return Ok(Val::Bool(true));
                    }
                }
                if has_get {
                    return self.call_magic(h, b"__get", MAGIC_GET, name, vec![Val::bytes(name)]);
                }
                if quiet {
                    return Ok(Val::Null);
                }
                Err(Throw::error(format!(
                    "{} property: {}::${}",
                    if matches!(lookup, super::class::PropLookup::Inaccessible) { "Cannot access" } else { "Undefined" },
                    String::from_utf8_lossy(&self.class(class).name),
                    String::from_utf8_lossy(name)
                )))
            }
        }
    }

    /// `$container[$key]` read.
    pub(crate) fn fetch_dim(&mut self, container: Val, key: Val, quiet: bool) -> Result<Val, Throw> {
        match container {
            Val::Array(a) => {
                let k = self.to_array_key(&key)?;
                match a.get(&k) {
                    Some(v) => Ok(v.clone()),
                    None => {
                        if quiet {
                            Ok(Val::Null)
                        } else {
                            Err(Throw::new(
                                "UndefinedKeyError",
                                match &k {
                                    ArrayKey::Int(i) => format!("Undefined array key {i}"),
                                    ArrayKey::Str(s) => format!("Undefined array key \"{s}\""),
                                },
                            ))
                        }
                    }
                }
            }
            Val::Str(s) => {
                let i = key.to_int()?;
                let len = s.len() as i64;
                let idx = if i < 0 { len + i } else { i };
                if idx < 0 || idx >= len {
                    if quiet {
                        return Ok(Val::Null);
                    }
                    return Err(Throw::new("UndefinedKeyError", format!("Uninitialized string offset {i}")));
                }
                Ok(Val::bytes(&s.as_bytes()[idx as usize..idx as usize + 1]))
            }
            Val::Object(h) => {
                let class = self.objects.get(h).class;
                if self.instance_of(class, self.wk().array_access) {
                    if quiet {
                        let exists = self.call_method_sync(h, b"offsetExists", vec![key.clone()])?;
                        if !exists.to_bool() {
                            return Ok(Val::Null);
                        }
                    }
                    return self.call_method_sync(h, b"offsetGet", vec![key]);
                }
                Err(Throw::error(format!("Cannot use object of type {} as array", String::from_utf8_lossy(&self.class(class).name))))
            }
            Val::Null if quiet => Ok(Val::Null),
            other => {
                if quiet {
                    Ok(Val::Null)
                } else {
                    Err(Throw::error(format!("Trying to access array offset on {}", other.type_name())))
                }
            }
        }
    }

    pub(crate) fn unset_path(&mut self, base: &PathBase, segs: &[PathSeg], module: u32) -> Result<(), Throw> {
        let n_keys = segs.iter().filter(|s| matches!(s, PathSeg::Dim | PathSeg::PropDyn)).count();
        let keys = self.pop_n(n_keys);
        let Some(last) = segs.last() else { return Ok(()) };
        let head = &segs[..segs.len() - 1];
        match last {
            PathSeg::Prop(_) | PathSeg::PropSel(_) | PathSeg::PropDyn => {
                let name = match last {
                    PathSeg::Prop(id) => self.str_of(module, *id).to_vec(),
                    PathSeg::PropSel(sel) => self.sel_name(*sel).to_vec(),
                    _ => self.to_string_val(&keys[n_keys - 1])?.as_bytes().to_vec(),
                };
                let n_head = head.iter().filter(|s| matches!(s, PathSeg::Dim | PathSeg::PropDyn)).count();
                let mut tmp_keys = keys[..n_head].to_vec();
                tmp_keys.reverse();
                // read the object through the head path
                let obj = self.read_through(base, head, &keys[..n_head], module)?;
                let Val::Object(h) = obj else { return Ok(()) };
                let class = self.objects.get(h).class;
                let scope = self.scope_class();
                match self.lookup_prop(class, &name, scope) {
                    super::class::PropLookup::Slot(idx) => self.objects.get_mut(h).slots[idx] = None,
                    _ => {
                        let key = PhpString::from(name.as_slice());
                        if self.objects.get_mut(h).dynamic.shift_remove(&key).is_none()
                            && self.class(class).find_method(b"__unset").is_some()
                            && !self.magic_guarded(h, &name, MAGIC_UNSET)
                        {
                            self.call_magic(h, b"__unset", MAGIC_UNSET, &name, vec![Val::Str(key)])?;
                        }
                    }
                }
                Ok(())
            }
            PathSeg::Append => Err(Throw::error("Cannot use [] for unsetting")),
            PathSeg::Dim => {
                let key = keys[n_keys - 1].clone();
                let n_head = head.iter().filter(|s| matches!(s, PathSeg::Dim | PathSeg::PropDyn)).count();
                // locate the array container: reuse store machinery with a special mode
                let mut loc = self.resolve_base(base, module)?;
                let mut key_i = 0usize;
                let mut i = 0usize;
                loop {
                    let run_end = head[i..].iter().position(|s| matches!(s, PathSeg::Prop(_) | PathSeg::PropSel(_) | PathSeg::PropDyn)).map(|p| i + p).unwrap_or(head.len());
                    let dims = &head[i..run_end];
                    let n_dim_keys = dims.iter().filter(|s| matches!(s, PathSeg::Dim)).count();
                    let dim_keys: Vec<Val> = keys[key_i..key_i + n_dim_keys].to_vec();
                    key_i += n_dim_keys;
                    if run_end == head.len() {
                        let _ = n_head;
                        return self.unset_dim_at(loc, dims, &dim_keys, key);
                    }
                    let prop_name = match &head[run_end] {
                        PathSeg::Prop(id) => self.str_of(module, *id).to_vec(),
                        PathSeg::PropSel(sel) => self.sel_name(*sel).to_vec(),
                        PathSeg::PropDyn => {
                            let k = keys[key_i].clone();
                            key_i += 1;
                            self.to_string_val(&k)?.as_bytes().to_vec()
                        }
                        _ => unreachable!(),
                    };
                    let target = self.read_at(&loc, dims, &dim_keys)?;
                    match target {
                        Val::Object(h) => loc = Loc::Obj(h, prop_name),
                        _ => return Ok(()),
                    }
                    i = run_end + 1;
                }
            }
        }
    }

    fn read_through(&mut self, base: &PathBase, segs: &[PathSeg], keys: &[Val], module: u32) -> Result<Val, Throw> {
        let mut cur = match base {
            PathBase::Slot(s) => self.slot_stack[self.frames.last().expect("frame").slot_base + *s as usize].clone().unwrap_or(Val::Null),
            PathBase::This => Val::Object(self.frame().this.ok_or_else(|| Throw::error("Using $this when not in object context"))?),
            PathBase::StaticProp(cr, name) => {
                let class = self.resolve_class_ref(cr, module)?;
                let n = self.str_of(module, *name).to_vec();
                self.static_prop_get(class, &n)?
            }
            PathBase::Stack => self.pop(),
        };
        let mut ki = 0;
        for seg in segs {
            cur = match seg {
                PathSeg::Dim => {
                    let k = keys[ki].clone();
                    ki += 1;
                    self.fetch_dim(cur, k, true)?
                }
                PathSeg::Prop(id) => {
                    let n = self.str_of(module, *id).to_vec();
                    match cur {
                        Val::Object(h) => self.read_prop(h, &n, true)?,
                        _ => Val::Null,
                    }
                }
                PathSeg::PropSel(sel) => {
                    let n = self.sel_name(*sel).to_vec();
                    match cur {
                        Val::Object(h) => self.read_prop(h, &n, true)?,
                        _ => Val::Null,
                    }
                }
                PathSeg::PropDyn => {
                    let k = keys[ki].clone();
                    ki += 1;
                    let n = self.to_string_val(&k)?.as_bytes().to_vec();
                    match cur {
                        Val::Object(h) => self.read_prop(h, &n, true)?,
                        _ => Val::Null,
                    }
                }
                PathSeg::Append => Val::Null,
            };
        }
        Ok(cur)
    }

    fn unset_dim_at(&mut self, loc: Loc, dims: &[PathSeg], keys: &[Val], key: Val) -> Result<(), Throw> {
        let mut root = match self.take_loc(&loc) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        let r = (|| -> Result<(), Throw> {
            let mut cur: &mut Val = &mut root;
            let mut ki = 0;
            for seg in dims {
                match seg {
                    PathSeg::Dim => {
                        let k = self.to_array_key(&keys[ki])?;
                        ki += 1;
                        match cur {
                            Val::Array(_) => {
                                let arr = cur.array_mut().expect("array");
                                match arr.get_mut(&k) {
                                    Some(v) => cur = v,
                                    None => return Ok(()),
                                }
                            }
                            _ => return Ok(()),
                        }
                    }
                    _ => return Ok(()),
                }
            }
            match cur {
                Val::Array(_) => {
                    let k = self.to_array_key(&key)?;
                    cur.array_mut().expect("array").remove(&k);
                }
                Val::Object(h) => {
                    let h = *h;
                    if self.instance_of(self.objects.get(h).class, self.wk().array_access) {
                        self.call_method_sync(h, b"offsetUnset", vec![key])?;
                    }
                }
                _ => {}
            }
            Ok(())
        })();
        self.put_loc(&loc, root)?;
        r
    }
}


/// Cheap exact-type check for the common scalar cases (no coercion needed).
/// FRAME1: `Vec::truncate` and `Vec::resize` are out-of-line generic calls
/// (≈40–55 Ir for a frame of two slots); frames are small, so an inlined
/// loop is cheaper.
#[inline(always)]
pub(crate) fn shrink<T>(v: &mut Vec<T>, len: usize) {
    while v.len() > len {
        v.pop();
    }
}

#[inline(always)]
pub(crate) fn grow_slots(v: &mut Vec<Option<Val>>, n: usize) {
    v.reserve(n);
    for _ in 0..n {
        v.push(None);
    }
}

#[inline(always)]
pub(crate) fn scalar_type_matches(t: &TypeHint, v: &Val) -> bool {
    matches!(
        (t, v),
        (TypeHint::Int, Val::Int(_))
            | (TypeHint::Float, Val::Float(_))
            | (TypeHint::String, Val::Str(_))
            | (TypeHint::Bool, Val::Bool(_))
            | (TypeHint::Array, Val::Array(_))
            | (TypeHint::Mixed, _)
    )
}

/// Kinds of magic property methods, for their guards.
pub(crate) const MAGIC_GET: u8 = 0;
pub(crate) const MAGIC_SET: u8 = 1;
pub(crate) const MAGIC_ISSET: u8 = 2;
pub(crate) const MAGIC_UNSET: u8 = 3;

impl Vm {
    /// `isset($o->name)` as PHP's has_property: a visible property counts when it
    /// is not null, a native class answers through its handlers, otherwise
    /// `__isset` decides (unless it already runs for this property); `__get` is
    /// never called.
    pub(crate) fn isset_prop(&mut self, obj: Val, name: &[u8]) -> Result<bool, Throw> {
        let Val::Object(h) = obj else { return Ok(false) };
        let class = self.objects.get(h).class;
        let scope = self.scope_class();
        let lookup = self.lookup_prop(class, name, scope);
        if let super::class::PropLookup::Slot(idx) = lookup {
            return Ok(matches!(&self.objects.get(h).slots[idx], Some(v) if !v.is_null()));
        }
        if matches!(lookup, super::class::PropLookup::Undeclared)
            && let Some(v) = self.objects.get(h).dynamic.get(&PhpString::from(name))
        {
            return Ok(!v.is_null());
        }
        if let Some(n) = self.native_info(class) {
            if let Some(isset) = n.handlers.isset
                && let Some(set) = isset(self, h, name)?
            {
                return Ok(set);
            }
            if let Some(get) = n.handlers.get
                && let Some(v) = get(self, h, name)?
            {
                return Ok(!v.is_null());
            }
        }
        if self.class(class).find_method(b"__isset").is_some() && !self.magic_guarded(h, name, MAGIC_ISSET) {
            return Ok(self.call_magic(h, b"__isset", MAGIC_ISSET, name, vec![Val::bytes(name)])?.to_bool());
        }
        Ok(false)
    }

    /// Whether `__get`/`__set`/`__isset`/`__unset` already runs for this
    /// property of this object (then the access goes to the property itself).
    pub(crate) fn magic_guarded(&self, h: ObjHandle, name: &[u8], kind: u8) -> bool {
        self.magic_guards.iter().any(|(g, n, k)| *g == h && *k == kind && n == name)
    }

    /// Call a magic property method with its guard set.
    fn call_magic(&mut self, h: ObjHandle, method: &[u8], kind: u8, name: &[u8], args: Vec<Val>) -> Result<Val, Throw> {
        self.magic_guards.push((h, name.to_vec(), kind));
        let r = self.call_method_sync(h, method, args);
        self.magic_guards.pop();
        r
    }
}

