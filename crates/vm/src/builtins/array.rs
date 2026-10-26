//! Array functions. By-reference variants return the modified array
//! (DECISIONS.md: out-parameters become return values).

use std::cmp::Ordering;

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.register_native("count", count);
    vm.register_native("sizeof", count);
    vm.register_native("array_keys", |_, a| {
        let arr = args::array(a, 0, "array_keys")?;
        let out = match a.get(1) {
            Some(needle) => arr.iter().filter(|(_, v)| identical(v, needle)).map(|(k, _)| k.to_val()).collect(),
            None => arr.keys().map(ArrayKey::to_val).collect::<Vec<_>>(),
        };
        Ok(Val::array(PhpArray::from_values(out)))
    });
    vm.register_native("array_values", |_, a| Ok(Val::array(args::array(a, 0, "array_values")?.reindexed())));
    vm.register_native("array_merge", |_, a| {
        let total: usize = a.iter().map(|v| if let Val::Array(x) = v { x.len() } else { 0 }).sum();
        // PHP refuses before allocating (instead of running out of memory)
        if total >= 1 << 30 {
            return Err(Throw::error("The total number of elements must be lower than 1073741824"));
        }
        let mut out = PhpArray::with_capacity(total);
        for (i, v) in a.iter().enumerate() {
            let Val::Array(arr) = v else { return Err(Throw::type_error(format!("array_merge(): Argument #{} must be of type array, {} given", i + 1, v.type_name()))) };
            merge_into(&mut out, arr);
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_merge_recursive", |_, a| {
        let mut out = PhpArray::new();
        for v in a {
            let Val::Array(arr) = v else { return Err(Throw::type_error("array_merge_recursive(): arguments must be arrays")) };
            merge_recursive(&mut out, arr);
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_replace", |_, a| {
        let mut out = (*args::array(a, 0, "array_replace")?).clone();
        for v in &a[1..] {
            if let Val::Array(arr) = v {
                for (k, val) in arr.iter() {
                    out.insert(k.clone(), val.clone());
                }
            }
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_combine", |vm, a| {
        let keys = args::array(a, 0, "array_combine")?;
        let vals = args::array(a, 1, "array_combine")?;
        if keys.len() != vals.len() {
            return Err(Throw::value_error("array_combine(): Argument #1 ($keys) and argument #2 ($values) must have the same number of elements"));
        }
        let mut out = PhpArray::new();
        for (k, v) in keys.values().zip(vals.values()) {
            let key = vm.to_array_key(k)?;
            out.insert(key, v.clone());
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_flip", |vm, a| {
        let arr = args::array(a, 0, "array_flip")?;
        let mut out = PhpArray::new();
        for (k, v) in arr.iter() {
            let key = vm.to_array_key(v)?;
            out.insert(key, k.to_val());
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_key_exists", |vm, a| {
        args::count(a, 2, "array_key_exists")?;
        Ok(Val::Bool(match &a[1] {
            Val::Array(arr) => arr.contains_key(&vm.to_array_key(&a[0])?),
            Val::Object(h) => { let n = vm.to_string_val(&a[0])?; vm.get_prop_raw(*h, n.as_bytes()).is_some() }
            _ => false,
        }))
    });
    vm.register_native("key_exists", |vm, a| {
        args::count(a, 2, "key_exists")?;
        Ok(Val::Bool(matches!(&a[1], Val::Array(arr) if arr.contains_key(&vm.to_array_key(&a[0])?))))
    });
    vm.register_native("in_array", |_, a| {
        args::count(a, 2, "in_array")?;
        let arr = args::array(a, 1, "in_array")?;
        Ok(Val::Bool(arr.values().any(|v| identical(v, &a[0]))))
    });
    vm.register_native("array_search", |_, a| {
        args::count(a, 2, "array_search")?;
        let arr = args::array(a, 1, "array_search")?;
        Ok(arr.iter().find(|(_, v)| identical(v, &a[0])).map(|(k, _)| k.to_val()).unwrap_or(Val::Bool(false)))
    });
    vm.register_native("array_key_first", |_, a| Ok(args::array(a, 0, "array_key_first")?.first().map(|(k, _)| k.to_val()).unwrap_or(Val::Null)));
    vm.register_native("array_key_last", |_, a| Ok(args::array(a, 0, "array_key_last")?.last().map(|(k, _)| k.to_val()).unwrap_or(Val::Null)));
    vm.register_native("array_is_list", |_, a| Ok(Val::Bool(args::array(a, 0, "array_is_list")?.is_list())));
    vm.register_native("array_slice", |_, a| {
        let arr = args::array(a, 0, "array_slice")?;
        let offset = args::int(a, 1, "array_slice")?;
        let len = match a.get(2) { None | Some(Val::Null) => None, _ => Some(args::int(a, 2, "array_slice")?) };
        let preserve = args::boolean(a, 3, false)?;
        let (from, to) = super::string::substr_range(arr.len(), offset, len);
        let mut out = PhpArray::new();
        for i in from..to {
            let (k, v) = arr.entry_at(i).expect("in range");
            match k {
                ArrayKey::Int(_) if !preserve => out.push(v.clone()),
                _ => out.insert(k.clone(), v.clone()),
            }
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_splice", |_, a| {
        // returns the modified array (dialect: no by-ref)
        let arr = args::array(a, 0, "array_splice")?;
        let offset = args::int(a, 1, "array_splice")?;
        let len = match a.get(2) { None | Some(Val::Null) => None, _ => Some(args::int(a, 2, "array_splice")?) };
        let (from, to) = super::string::substr_range(arr.len(), offset, len);
        let mut out = PhpArray::new();
        for i in 0..from {
            let (k, v) = arr.entry_at(i).expect("in range");
            match k { ArrayKey::Int(_) => out.push(v.clone()), _ => out.insert(k.clone(), v.clone()) }
        }
        if let Some(rep) = a.get(3) {
            match rep {
                Val::Array(r) => for v in r.values() { out.push(v.clone()) },
                other => out.push(other.clone()),
            }
        }
        for i in to..arr.len() {
            let (k, v) = arr.entry_at(i).expect("in range");
            match k { ArrayKey::Int(_) => out.push(v.clone()), _ => out.insert(k.clone(), v.clone()) }
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_map", |vm, a| {
        args::count(a, 2, "array_map")?;
        let arrays: Vec<std::rc::Rc<PhpArray>> = a[1..].iter().enumerate().map(|(i, v)| match v { Val::Array(x) => Ok(x.clone()), o => Err(Throw::type_error(format!("array_map(): Argument #{} must be of type array, {} given", i + 2, o.type_name()))) }).collect::<Result<_, _>>()?;
        let mut out = PhpArray::new();
        if arrays.len() == 1 {
            let arr = &arrays[0];
            let keep_keys = !arr.is_list() || true;
            for (k, v) in arr.iter() {
                let r = if a[0].is_null() { v.clone() } else { vm.call_value(&a[0], vec![v.clone()])? };
                if keep_keys { out.insert(k.clone(), r) } else { out.push(r) }
            }
            return Ok(Val::array(out));
        }
        let n = arrays.iter().map(|x| x.len()).max().unwrap_or(0);
        for i in 0..n {
            let vals: Vec<Val> = arrays.iter().map(|x| x.entry_at(i).map(|(_, v)| v.clone()).unwrap_or(Val::Null)).collect();
            let r = if a[0].is_null() { Val::array(PhpArray::from_values(vals)) } else { vm.call_value(&a[0], vals)? };
            out.push(r);
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_filter", |vm, a| {
        let arr = args::array(a, 0, "array_filter")?;
        let mode = args::int_opt(a, 2, "array_filter", 0)?;
        let mut out = PhpArray::new();
        for (k, v) in arr.iter() {
            let keep = match a.get(1) {
                None | Some(Val::Null) => v.to_bool(),
                Some(cb) => {
                    let call_args = match mode { 2 => vec![k.to_val()], 1 => vec![v.clone(), k.to_val()], _ => vec![v.clone()] };
                    vm.call_value(cb, call_args)?.to_bool()
                }
            };
            if keep {
                out.insert(k.clone(), v.clone());
            }
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_reduce", |vm, a| {
        let arr = args::array(a, 0, "array_reduce")?;
        args::count(a, 2, "array_reduce")?;
        let mut acc = a.get(2).cloned().unwrap_or(Val::Null);
        for v in arr.values() {
            acc = vm.call_value(&a[1], vec![acc, v.clone()])?;
        }
        Ok(acc)
    });
    vm.register_native("array_walk", |vm, a| {
        let arr = args::array(a, 0, "array_walk")?;
        args::count(a, 2, "array_walk")?;
        for (k, v) in arr.iter() {
            let mut call_args = vec![v.clone(), k.to_val()];
            if let Some(extra) = a.get(2) {
                call_args.push(extra.clone());
            }
            vm.call_value(&a[1], call_args)?;
        }
        Ok(Val::Bool(true))
    });
    vm.register_native("array_column", |vm, a| {
        let arr = args::array(a, 0, "array_column")?;
        let col = a.get(1).cloned().unwrap_or(Val::Null);
        let idx = a.get(2).cloned().unwrap_or(Val::Null);
        let mut out = PhpArray::new();
        for row in arr.values() {
            let get = |vm: &mut Vm, row: &Val, key: &Val| -> Result<Option<Val>, Throw> {
                Ok(match row {
                    Val::Array(r) => r.get(&vm.to_array_key(key)?).cloned(),
                    Val::Object(h) => { let n = vm.to_string_val(key)?; vm.get_prop_raw(*h, n.as_bytes()) }
                    _ => None,
                })
            };
            let value = if col.is_null() { Some(row.clone()) } else { get(vm, row, &col)? };
            let Some(value) = value else { continue };
            if idx.is_null() {
                out.push(value);
            } else {
                match get(vm, row, &idx)? {
                    Some(k) => { let key = vm.to_array_key(&k)?; out.insert(key, value) }
                    None => out.push(value),
                }
            }
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_unique", |_, a| {
        // strict identity (DIALECT §5.1) through a hash set; arrays, objects
        // and NaN are compared directly (PERF1: was quadratic)
        #[derive(PartialEq, Eq, Hash)]
        enum Seen {
            Null,
            Bool(bool),
            Int(i64),
            Float(u64),
            Str(PhpString),
        }
        let arr = args::array(a, 0, "array_unique")?;
        let mut out = PhpArray::with_capacity(arr.len());
        let mut seen: std::collections::HashSet<Seen, foldhash::fast::RandomState> = std::collections::HashSet::with_capacity_and_hasher(arr.len(), Default::default());
        let mut others: Vec<Val> = Vec::new();
        for (k, v) in arr.iter() {
            let key = match v {
                Val::Null => Some(Seen::Null),
                Val::Bool(b) => Some(Seen::Bool(*b)),
                Val::Int(i) => Some(Seen::Int(*i)),
                Val::Float(f) if f.is_nan() => None,
                Val::Float(f) => Some(Seen::Float(if *f == 0.0 { 0 } else { f.to_bits() })),
                Val::Str(s) => Some(Seen::Str(s.clone())),
                _ => None,
            };
            let fresh = match key {
                Some(key) => seen.insert(key),
                None => {
                    if others.iter().any(|s| identical(s, v)) {
                        false
                    } else {
                        others.push(v.clone());
                        true
                    }
                }
            };
            if fresh {
                out.insert(k.clone(), v.clone());
            }
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_reverse", |_, a| {
        let arr = args::array(a, 0, "array_reverse")?;
        let preserve = args::boolean(a, 1, false)?;
        let mut out = PhpArray::new();
        for i in (0..arr.len()).rev() {
            let (k, v) = arr.entry_at(i).expect("in range");
            match k { ArrayKey::Int(_) if !preserve => out.push(v.clone()), _ => out.insert(k.clone(), v.clone()) }
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_fill", |_, a| {
        let start = args::int(a, 0, "array_fill")?;
        let n = args::int(a, 1, "array_fill")?;
        let v = a.get(2).cloned().unwrap_or(Val::Null);
        let mut out = PhpArray::new();
        for i in 0..n.max(0) {
            out.insert(ArrayKey::Int(start + i), v.clone());
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_fill_keys", |vm, a| {
        let keys = args::array(a, 0, "array_fill_keys")?;
        let v = a.get(1).cloned().unwrap_or(Val::Null);
        let mut out = PhpArray::new();
        for k in keys.values() {
            let key = vm.to_array_key(k)?;
            out.insert(key, v.clone());
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_pad", |_, a| {
        let arr = args::array(a, 0, "array_pad")?;
        let size = args::int(a, 1, "array_pad")?;
        let v = a.get(2).cloned().unwrap_or(Val::Null);
        let n = size.unsigned_abs() as usize;
        if n <= arr.len() {
            return Ok(Val::Array(arr));
        }
        let mut out = PhpArray::new();
        if size < 0 {
            for _ in 0..n - arr.len() { out.push(v.clone()); }
        }
        for (k, val) in arr.iter() {
            match k { ArrayKey::Int(_) => out.push(val.clone()), _ => out.insert(k.clone(), val.clone()) }
        }
        if size > 0 {
            for _ in 0..n - arr.len() { out.push(v.clone()); }
        }
        Ok(Val::array(out))
    });
    vm.register_native("range", range);
    vm.register_native("array_chunk", |_, a| {
        let arr = args::array(a, 0, "array_chunk")?;
        let size = args::int(a, 1, "array_chunk")?;
        if size < 1 {
            return Err(Throw::value_error("array_chunk(): Argument #2 ($length) must be greater than 0"));
        }
        let preserve = args::boolean(a, 2, false)?;
        let mut out = PhpArray::new();
        let mut cur = PhpArray::new();
        for (k, v) in arr.iter() {
            if preserve { cur.insert(k.clone(), v.clone()) } else { cur.push(v.clone()) }
            if cur.len() as i64 == size {
                out.push(Val::array(std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            out.push(Val::array(cur));
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_diff", |vm, a| set_op(vm, a, "array_diff", false, false, false));
    vm.register_native("array_diff_key", |vm, a| set_op(vm, a, "array_diff_key", true, false, false));
    vm.register_native("array_diff_assoc", |vm, a| set_op(vm, a, "array_diff_assoc", true, true, false));
    vm.register_native("array_intersect", |vm, a| set_op(vm, a, "array_intersect", false, false, true));
    vm.register_native("array_intersect_key", |vm, a| set_op(vm, a, "array_intersect_key", true, false, true));
    vm.register_native("array_intersect_assoc", |vm, a| set_op(vm, a, "array_intersect_assoc", true, true, true));
    vm.register_native("array_count_values", |vm, a| {
        let arr = args::array(a, 0, "array_count_values")?;
        let mut out = PhpArray::new();
        for v in arr.values() {
            let key = vm.to_array_key(v)?;
            let n = out.get(&key).and_then(|x| if let Val::Int(i) = x { Some(*i) } else { None }).unwrap_or(0);
            out.insert(key, Val::Int(n + 1));
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_flip_placeholder", |_, _| Ok(Val::Null));
    vm.register_native("array_push", |_, a| {
        let mut arr = (*args::array(a, 0, "array_push")?).clone();
        for v in &a[1..] { arr.push(v.clone()); }
        Ok(Val::array(arr))
    });
    vm.register_native("array_unshift", |_, a| {
        let arr = args::array(a, 0, "array_unshift")?;
        let mut out = PhpArray::new();
        for v in &a[1..] { out.push(v.clone()); }
        for (k, v) in arr.iter() {
            match k { ArrayKey::Int(_) => out.push(v.clone()), _ => out.insert(k.clone(), v.clone()) }
        }
        Ok(Val::array(out))
    });
    vm.register_native("end", |_, a| Ok(args::array(a, 0, "end")?.last().map(|(_, v)| v.clone()).unwrap_or(Val::Bool(false))));
    vm.register_native("reset", |_, a| Ok(args::array(a, 0, "reset")?.first().map(|(_, v)| v.clone()).unwrap_or(Val::Bool(false))));
    vm.register_native("current", |_, a| Ok(args::array(a, 0, "current")?.first().map(|(_, v)| v.clone()).unwrap_or(Val::Bool(false))));
    vm.register_native("key", |_, a| Ok(args::array(a, 0, "key")?.first().map(|(k, _)| k.to_val()).unwrap_or(Val::Null)));
    vm.register_native("sort", |vm, a| sort_impl(vm, a, SortKind::Values, false));
    vm.register_native("rsort", |vm, a| sort_impl(vm, a, SortKind::Values, true));
    vm.register_native("usort", |vm, a| sort_impl(vm, a, SortKind::UserValues, false));
    vm.register_native("asort", |vm, a| sort_impl(vm, a, SortKind::ValuesKeepKeys, false));
    vm.register_native("arsort", |vm, a| sort_impl(vm, a, SortKind::ValuesKeepKeys, true));
    vm.register_native("uasort", |vm, a| sort_impl(vm, a, SortKind::UserValuesKeepKeys, false));
    vm.register_native("ksort", |vm, a| sort_impl(vm, a, SortKind::Keys, false));
    vm.register_native("krsort", |vm, a| sort_impl(vm, a, SortKind::Keys, true));
    vm.register_native("uksort", |vm, a| sort_impl(vm, a, SortKind::UserKeys, false));
    vm.register_native("natsort", |vm, a| sort_impl(vm, a, SortKind::Natural, false));
    vm.register_native("natcasesort", |vm, a| sort_impl(vm, a, SortKind::NaturalCase, false));
    vm.register_native("shuffle", |_, a| {
        let arr = args::array(a, 0, "shuffle")?;
        let mut vals: Vec<Val> = arr.values().cloned().collect();
        for i in (1..vals.len()).rev() {
            let j = (super::math::rand_u64() % (i as u64 + 1)) as usize;
            vals.swap(i, j);
        }
        Ok(Val::array(PhpArray::from_values(vals)))
    });
    vm.register_native("array_rand", |_, a| {
        let arr = args::array(a, 0, "array_rand")?;
        let n = args::int_opt(a, 1, "array_rand", 1)?;
        if arr.is_empty() {
            return Err(Throw::value_error("array_rand(): Argument #1 ($array) cannot be empty"));
        }
        if n == 1 {
            let i = (super::math::rand_u64() % arr.len() as u64) as usize;
            return Ok(arr.entry_at(i).map(|(k, _)| k.to_val()).unwrap_or(Val::Null));
        }
        let mut keys: Vec<Val> = arr.keys().map(ArrayKey::to_val).collect();
        for i in (1..keys.len()).rev() {
            let j = (super::math::rand_u64() % (i as u64 + 1)) as usize;
            keys.swap(i, j);
        }
        keys.truncate(n.max(0) as usize);
        Ok(Val::array(PhpArray::from_values(keys)))
    });
    vm.register_native("array_walk_recursive", |vm, a| {
        fn walk(vm: &mut Vm, arr: &PhpArray, cb: &Val) -> Result<(), Throw> {
            for (k, v) in arr.iter() {
                match v {
                    Val::Array(inner) => walk(vm, inner, cb)?,
                    _ => { vm.call_value(cb, vec![v.clone(), k.to_val()])?; }
                }
            }
            Ok(())
        }
        let arr = args::array(a, 0, "array_walk_recursive")?;
        args::count(a, 2, "array_walk_recursive")?;
        walk(vm, &arr, &a[1])?;
        Ok(Val::Bool(true))
    });
    vm.register_native("array_find", |vm, a| {
        let arr = args::array(a, 0, "array_find")?;
        args::count(a, 2, "array_find")?;
        for (k, v) in arr.iter() {
            if vm.call_value(&a[1], vec![v.clone(), k.to_val()])?.to_bool() {
                return Ok(v.clone());
            }
        }
        Ok(Val::Null)
    });
    vm.register_native("array_find_key", |vm, a| {
        let arr = args::array(a, 0, "array_find_key")?;
        args::count(a, 2, "array_find_key")?;
        for (k, v) in arr.iter() {
            if vm.call_value(&a[1], vec![v.clone(), k.to_val()])?.to_bool() {
                return Ok(k.to_val());
            }
        }
        Ok(Val::Null)
    });
    vm.register_native("array_any", |vm, a| {
        let arr = args::array(a, 0, "array_any")?;
        args::count(a, 2, "array_any")?;
        for (k, v) in arr.iter() {
            if vm.call_value(&a[1], vec![v.clone(), k.to_val()])?.to_bool() {
                return Ok(Val::Bool(true));
            }
        }
        Ok(Val::Bool(false))
    });
    vm.register_native("array_all", |vm, a| {
        let arr = args::array(a, 0, "array_all")?;
        args::count(a, 2, "array_all")?;
        for (k, v) in arr.iter() {
            if !vm.call_value(&a[1], vec![v.clone(), k.to_val()])?.to_bool() {
                return Ok(Val::Bool(false));
            }
        }
        Ok(Val::Bool(true))
    });
    vm.register_native("compact_placeholder", |_, _| Ok(Val::Null));
    // ---- BI1 (ANALYSE_D13 §A3) ---------------------------------------------
    vm.register_native("array_replace_recursive", |_, a| {
        let mut out = (*args::array(a, 0, "array_replace_recursive")?).clone();
        for i in 1..a.len() {
            let with = args::array(a, i, "array_replace_recursive")?;
            out = replace_recursive(out, &with);
        }
        Ok(Val::array(out))
    });
    // values / keys compared: None = ignored, Str = as strings (PHP), User = callback(a, b) == 0
    vm.register_native("array_udiff", |vm, a| user_set_op(vm, a, "array_udiff", Cmp::User, Cmp::None, false));
    vm.register_native("array_uintersect", |vm, a| user_set_op(vm, a, "array_uintersect", Cmp::User, Cmp::None, true));
    vm.register_native("array_diff_ukey", |vm, a| user_set_op(vm, a, "array_diff_ukey", Cmp::None, Cmp::User, false));
    vm.register_native("array_intersect_ukey", |vm, a| user_set_op(vm, a, "array_intersect_ukey", Cmp::None, Cmp::User, true));
    vm.register_native("array_diff_uassoc", |vm, a| user_set_op(vm, a, "array_diff_uassoc", Cmp::Str, Cmp::User, false));
    vm.register_native("array_intersect_uassoc", |vm, a| user_set_op(vm, a, "array_intersect_uassoc", Cmp::Str, Cmp::User, true));
    vm.register_native("array_udiff_assoc", |vm, a| user_set_op(vm, a, "array_udiff_assoc", Cmp::User, Cmp::Str, false));
    vm.register_native("array_uintersect_assoc", |vm, a| user_set_op(vm, a, "array_uintersect_assoc", Cmp::User, Cmp::Str, true));
    vm.register_native("array_udiff_uassoc", |vm, a| user_set_op(vm, a, "array_udiff_uassoc", Cmp::User, Cmp::User, false));
    vm.register_native("array_uintersect_uassoc", |vm, a| user_set_op(vm, a, "array_uintersect_uassoc", Cmp::User, Cmp::User, true));
}

fn count(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let recursive = args::int_opt(a, 1, "count", 0)? == 1;
    fn rec(arr: &PhpArray) -> i64 {
        arr.values().map(|v| 1 + if let Val::Array(inner) = v { rec(inner) } else { 0 }).sum()
    }
    match a.first() {
        Some(Val::Array(arr)) => Ok(Val::Int(if recursive { rec(arr) } else { arr.len() as i64 })),
        Some(Val::Object(h)) => {
            let class = vm.objects.get(*h).class;
            if vm.instance_of(class, vm.wk().countable) {
                let r = vm.call_method_sync(*h, b"count", Vec::new())?;
                return Ok(Val::Int(r.to_int()?));
            }
            Err(Throw::type_error(format!("count(): Argument #1 ($value) must be of type Countable|array, {} given", vm.value_type_name(&Val::Object(*h)))))
        }
        Some(other) => Err(Throw::type_error(format!("count(): Argument #1 ($value) must be of type Countable|array, {} given", other.type_name()))),
        None => Err(Throw::argument_count("count() expects at least 1 argument, 0 given")),
    }
}

fn merge_into(out: &mut PhpArray, arr: &PhpArray) {
    for (k, v) in arr.iter() {
        match k {
            ArrayKey::Int(_) => out.push(v.clone()),
            ArrayKey::Str(_) => out.insert(k.clone(), v.clone()),
        }
    }
}

fn merge_recursive(out: &mut PhpArray, arr: &PhpArray) {
    for (k, v) in arr.iter() {
        match k {
            ArrayKey::Int(_) => out.push(v.clone()),
            ArrayKey::Str(_) => match (out.get(k).cloned(), v) {
                (Some(Val::Array(existing)), Val::Array(incoming)) => {
                    let mut merged = (*existing).clone();
                    merge_recursive(&mut merged, incoming);
                    out.insert(k.clone(), Val::array(merged));
                }
                (Some(existing), incoming) => {
                    let mut merged = match existing { Val::Array(e) => (*e).clone(), other => PhpArray::from_values([other]) };
                    match incoming { Val::Array(i) => merge_recursive(&mut merged, i), other => merged.push(other.clone()) }
                    out.insert(k.clone(), Val::array(merged));
                }
                (None, _) => out.insert(k.clone(), v.clone()),
            },
        }
    }
}

/// Largest array PHP builds (HT_MAX_SIZE on 64-bit); range() refuses before.
const MAX_ARRAY_SIZE: f64 = (1u64 << 30) as f64;

fn range(_: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    args::count(a, 2, "range")?;
    let step_v = a.get(2).cloned().unwrap_or(Val::Int(1));
    let increasing_negative = || Throw::value_error("range(): Argument #3 ($step) must be greater than 0 for increasing ranges");
    // two one-byte strings are a range of characters, digits included (PHP 8.3+)
    if let (Val::Str(s), Val::Str(e)) = (&a[0], &a[1])
        && s.len() == 1
        && e.len() == 1
    {
        let (x, y) = (s.as_bytes()[0] as i64, e.as_bytes()[0] as i64);
        let step = step_v.to_int()?;
        if step < 0 && x < y {
            return Err(increasing_negative());
        }
        let step = step.unsigned_abs().max(1) as i64;
        let mut out = PhpArray::new();
        let mut i = x;
        while if x <= y { i <= y } else { i >= y } {
            out.push(Val::bytes(&[i as u8]));
            i += if x <= y { step } else { -step };
        }
        return Ok(Val::array(out));
    }
    // PHP 8.3+: non-finite bounds and steps, a negative step for an increasing
    // range and results beyond the largest array are ValueErrors
    for (i, v) in [&a[0], &a[1], &step_v].into_iter().enumerate() {
        if let Val::Float(f) = v
            && !f.is_finite()
        {
            let arg = ["#1 ($start)", "#2 ($end)", "#3 ($step)"][i];
            return Err(Throw::value_error(format!("range(): Argument {arg} must be a finite number, {} provided", if f.is_nan() { "NAN" } else { "INF" })));
        }
    }
    let float_bound = |v: &Val| matches!(v, Val::Float(_)) || matches!(v, Val::Str(s) if s.as_bytes().iter().any(|c| matches!(c, b'.' | b'e' | b'E')));
    let fractional_step = matches!(step_v, Val::Float(f) if f.fract() != 0.0);
    let mut out = PhpArray::new();
    if float_bound(&a[0]) || float_bound(&a[1]) || fractional_step {
        let (x, y, step) = (a[0].to_float()?, a[1].to_float()?, step_v.to_float()?);
        if step == 0.0 {
            return Err(Throw::value_error("range(): Argument #3 ($step) cannot be 0"));
        }
        if step < 0.0 && x < y {
            return Err(increasing_negative());
        }
        let step = step.abs();
        let n = ((y - x).abs() / step).floor();
        if n >= MAX_ARRAY_SIZE - 1.0 {
            return Err(Throw::value_error(format!("The supplied range exceeds the maximum array size: start={x:.1} end={y:.1} step={step:.1}")));
        }
        for i in 0..=(n as i64) {
            out.push(Val::Float(if x <= y { x + i as f64 * step } else { x - i as f64 * step }));
        }
    } else {
        let (x, y, step) = (a[0].to_int()?, a[1].to_int()?, step_v.to_int()?);
        if step == 0 {
            return Err(Throw::value_error("range(): Argument #3 ($step) cannot be 0"));
        }
        if step == i64::MIN {
            return Err(Throw::value_error(format!("range(): Argument #3 ($step) must be greater than {}", i64::MIN)));
        }
        if step < 0 && x < y {
            return Err(increasing_negative());
        }
        let step = step.abs();
        let n = (x as i128 - y as i128).unsigned_abs() / step as u128;
        if n as f64 >= MAX_ARRAY_SIZE - 1.0 {
            return Err(Throw::value_error(format!("The supplied range exceeds the maximum array size: start={x} end={y} step={step}")));
        }
        // n + 1 values; computed from the start so nothing can overflow
        for i in 0..=(n as i64) {
            out.push(Val::Int(if x <= y { x + i * step } else { x - i * step }));
        }
    }
    Ok(Val::array(out))
}

fn set_op(vm: &mut Vm, a: &[Val], name: &str, by_key: bool, by_both: bool, intersect: bool) -> Result<Val, Throw> {
    let base = args::array(a, 0, name)?;
    let others: Vec<std::rc::Rc<PhpArray>> = a[1..].iter().map(|v| match v { Val::Array(x) => Ok(x.clone()), o => Err(Throw::type_error(format!("{name}(): arguments must be arrays, {} given", o.type_name()))) }).collect::<Result<_, _>>()?;
    let mut out = PhpArray::new();
    for (k, v) in base.iter() {
        let vs = if by_key && !by_both { None } else { Some(vm.to_string_val(v)?) };
        let in_other = |o: &PhpArray| -> bool {
            if by_key {
                match o.get(k) {
                    Some(ov) => !by_both || vs.as_ref().is_some_and(|s| ov.to_php_string().ok().as_ref() == Some(s)),
                    None => false,
                }
            } else {
                o.values().any(|ov| ov.to_php_string().ok().as_ref() == vs.as_ref())
            }
        };
        let present = if intersect { others.iter().all(|o| in_other(o)) } else { !others.iter().any(|o| in_other(o)) };
        if present {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Val::array(out))
}

#[derive(Clone, Copy, PartialEq)]
enum SortKind {
    Values,
    ValuesKeepKeys,
    UserValues,
    UserValuesKeepKeys,
    Keys,
    UserKeys,
    Natural,
    NaturalCase,
}

fn php_sort_cmp(a: &Val, b: &Val, flags: i64) -> Ordering {
    if flags & 2 == 2 {
        let (x, y) = (a.to_php_string().unwrap_or_default(), b.to_php_string().unwrap_or_default());
        return if flags & 8 == 8 { x.as_bytes().to_ascii_lowercase().cmp(&y.as_bytes().to_ascii_lowercase()) } else { x.cmp(&y) };
    }
    match (a, b) {
        (Val::Str(x), Val::Str(y)) => {
            // PHP compares numeric strings numerically
            match (parse_numeric(x.as_bytes()), parse_numeric(y.as_bytes())) {
                (Some((nx, true)), Some((ny, true))) => nx.as_f64().partial_cmp(&ny.as_f64()).unwrap_or(Ordering::Equal),
                _ => x.cmp(y),
            }
        }
        _ => compare(a, b).ok().flatten().unwrap_or_else(|| {
            let (x, y) = (a.to_php_string().unwrap_or_default(), b.to_php_string().unwrap_or_default());
            x.cmp(&y)
        }),
    }
}

fn sort_impl(vm: &mut Vm, a: &[Val], kind: SortKind, reverse: bool) -> Result<Val, Throw> {
    let arr = args::array(a, 0, "sort")?;
    let mut entries: Vec<(ArrayKey, Val)> = arr.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let flags = match kind { SortKind::Values | SortKind::ValuesKeepKeys | SortKind::Keys => args::int_opt(a, 1, "sort", 0)?, _ => 0 };
    let cb = a.get(1).cloned();
    let mut err: Option<Throw> = None;
    // descending sorts compare the other way round instead of reversing the
    // result: PHP 8 sorts are stable, equal elements keep their order (a
    // reversed result would put them back to front)
    let dir = |o: Ordering| if reverse { o.reverse() } else { o };
    // PERF3: only ints (the common case) compare as plain integers
    let int_values = flags == 0 && entries.iter().all(|e| matches!(e.1, Val::Int(_)));
    let int_keys = flags == 0 && entries.iter().all(|e| matches!(e.0, ArrayKey::Int(_)));
    let int_of = |v: &Val| if let Val::Int(i) = v { *i } else { 0 };
    let key_int = |k: &ArrayKey| if let ArrayKey::Int(i) = k { *i } else { 0 };
    match kind {
        SortKind::Values | SortKind::ValuesKeepKeys if int_values => entries.sort_by(|x, y| dir(int_of(&x.1).cmp(&int_of(&y.1)))),
        SortKind::Values | SortKind::ValuesKeepKeys => entries.sort_by(|x, y| dir(php_sort_cmp(&x.1, &y.1, flags))),
        SortKind::Keys if int_keys => entries.sort_by(|x, y| dir(key_int(&x.0).cmp(&key_int(&y.0)))),
        SortKind::Keys => entries.sort_by(|x, y| dir(php_sort_cmp(&x.0.to_val(), &y.0.to_val(), flags))),
        SortKind::Natural => entries.sort_by(|x, y| nat(&x.1, &y.1, false)),
        SortKind::NaturalCase => entries.sort_by(|x, y| nat(&x.1, &y.1, true)),
        SortKind::UserValues | SortKind::UserValuesKeepKeys | SortKind::UserKeys => {
            let cb = cb.ok_or_else(|| Throw::argument_count("usort() expects exactly 2 arguments, 1 given"))?;
            let use_keys = kind == SortKind::UserKeys;
            // merge sort with a fallible comparator
            let mut v = entries;
            let mut sorted: Vec<(ArrayKey, Val)> = Vec::with_capacity(v.len());
            let cmp = |vm: &mut Vm, x: &(ArrayKey, Val), y: &(ArrayKey, Val)| -> Result<Ordering, Throw> {
                let (p, q) = if use_keys { (x.0.to_val(), y.0.to_val()) } else { (x.1.clone(), y.1.clone()) };
                let r = vm.call_value(&cb, vec![p, q])?;
                Ok(match r { Val::Int(i) => i.cmp(&0), Val::Float(f) => f.partial_cmp(&0.0).unwrap_or(Ordering::Equal), Val::Bool(b) => if b { Ordering::Greater } else { Ordering::Equal }, _ => Ordering::Equal })
            };
            // insertion into sorted via binary search is O(n log n) comparisons
            for item in v.drain(..) {
                let mut lo = 0;
                let mut hi = sorted.len();
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    match cmp(vm, &item, &sorted[mid]) {
                        Ok(Ordering::Less) => hi = mid,
                        Ok(_) => lo = mid + 1,
                        Err(e) => { err = Some(e); hi = mid; lo = mid; }
                    }
                }
                sorted.insert(lo, item);
            }
            entries = sorted;
        }
    }
    if let Some(e) = err {
        return Err(e);
    }
    let keep_keys = matches!(kind, SortKind::ValuesKeepKeys | SortKind::UserValuesKeepKeys | SortKind::Keys | SortKind::UserKeys | SortKind::Natural | SortKind::NaturalCase);
    let mut out = PhpArray::new();
    for (k, v) in entries {
        if keep_keys { out.insert(k, v) } else { out.push(v) }
    }
    Ok(Val::array(out))
}

fn nat(a: &Val, b: &Val, ci: bool) -> Ordering {
    let (x, y) = (a.to_php_string().unwrap_or_default(), b.to_php_string().unwrap_or_default());
    let (x, y) = if ci { (x.as_bytes().to_ascii_lowercase(), y.as_bytes().to_ascii_lowercase()) } else { (x.as_bytes().to_vec(), y.as_bytes().to_vec()) };
    let sx = String::from_utf8_lossy(&x).to_string();
    let sy = String::from_utf8_lossy(&y).to_string();
    natord(&sx, &sy)
}

fn natord(a: &str, b: &str) -> Ordering {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < ab.len() && j < bb.len() {
        if ab[i].is_ascii_digit() && bb[j].is_ascii_digit() {
            let si = i;
            while i < ab.len() && ab[i].is_ascii_digit() { i += 1; }
            let sj = j;
            while j < bb.len() && bb[j].is_ascii_digit() { j += 1; }
            let na: u128 = a[si..i].parse().unwrap_or(0);
            let nb: u128 = b[sj..j].parse().unwrap_or(0);
            if na != nb { return na.cmp(&nb); }
        } else {
            if ab[i] != bb[j] { return ab[i].cmp(&bb[j]); }
            i += 1; j += 1;
        }
    }
    (ab.len() - i).cmp(&(bb.len() - j))
}

/// PHP's array_replace_recursive: arrays on both sides merge key by key.
fn replace_recursive(mut base: PhpArray, with: &PhpArray) -> PhpArray {
    for (k, v) in with.iter() {
        let merged = match (base.get(k), v) {
            (Some(Val::Array(old)), Val::Array(new)) => Val::array(replace_recursive((**old).clone(), new)),
            _ => v.clone(),
        };
        base.insert(k.clone(), merged);
    }
    base
}

#[derive(Clone, Copy, PartialEq)]
enum Cmp {
    None,
    Str,
    User,
}

/// The `array_u*` family: the callbacks are the last arguments (value callback
/// first when both are given); an entry is in another array when some entry
/// there compares equal on every compared part.
fn user_set_op(vm: &mut Vm, a: &[Val], name: &str, values: Cmp, keys: Cmp, intersect: bool) -> Result<Val, Throw> {
    let n_cb = (values == Cmp::User) as usize + (keys == Cmp::User) as usize;
    if a.len() < 1 + n_cb {
        return Err(Throw::argument_count(format!("{name}() expects at least {} arguments, {} given", 1 + n_cb, a.len())));
    }
    let callbacks = &a[a.len() - n_cb..];
    let (value_cb, key_cb) = match (values, keys) {
        (Cmp::User, Cmp::User) => (Some(callbacks[0].clone()), Some(callbacks[1].clone())),
        (Cmp::User, _) => (Some(callbacks[0].clone()), None),
        (_, Cmp::User) => (None, Some(callbacks[0].clone())),
        _ => (None, None),
    };
    let base = args::array(a, 0, name)?;
    let others: Vec<std::rc::Rc<PhpArray>> = a[1..a.len() - n_cb].iter().enumerate().map(|(i, v)| match v { Val::Array(x) => Ok(x.clone()), o => Err(Throw::type_error(format!("{name}(): Argument #{} must be of type array, {} given", i + 2, o.type_name()))) }).collect::<Result<_, _>>()?;
    let mut out = PhpArray::new();
    for (k, v) in base.iter() {
        let mut in_all = true;
        let mut in_any = false;
        for o in &others {
            let mut found = false;
            for (ok, ov) in o.iter() {
                let keys_equal = match keys {
                    Cmp::None => true,
                    Cmp::Str => k == ok,
                    Cmp::User => vm.call_value(key_cb.as_ref().expect("key callback"), vec![k.to_val(), ok.to_val()])?.to_int()? == 0,
                };
                if !keys_equal {
                    continue;
                }
                let values_equal = match values {
                    Cmp::None => true,
                    Cmp::Str => vm.to_string_val(v)? == vm.to_string_val(ov)?,
                    Cmp::User => vm.call_value(value_cb.as_ref().expect("value callback"), vec![v.clone(), ov.clone()])?.to_int()? == 0,
                };
                if values_equal {
                    found = true;
                    break;
                }
            }
            in_all &= found;
            in_any |= found;
        }
        if (intersect && in_all) || (!intersect && !in_any) {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Val::array(out))
}

