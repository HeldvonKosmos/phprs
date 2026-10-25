//! Type inspection, class/function introspection, callables.

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.register_native("gettype", gettype);
    vm.register_native("get_debug_type", get_debug_type);
    vm.register_native("is_int", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Int(_))))));
    vm.register_native("is_integer", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Int(_))))));
    vm.register_native("is_long", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Int(_))))));
    vm.register_native("is_float", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Float(_))))));
    vm.register_native("is_double", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Float(_))))));
    vm.register_native("is_string", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(_))))));
    vm.register_native("is_bool", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Bool(_))))));
    vm.register_native("is_array", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Array(_))))));
    vm.register_native("is_object", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Object(_))))));
    vm.register_native("is_null", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Null) | None))));
    vm.register_native("is_scalar", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Int(_) | Val::Float(_) | Val::Str(_) | Val::Bool(_))))));
    vm.register_native("is_numeric", is_numeric);
    vm.register_native("is_iterable", |vm, a| Ok(Val::Bool(match a.first() { Some(Val::Array(_)) => true, Some(Val::Object(h)) => vm.instance_of(vm.objects.get(*h).class, vm.wk().traversable), _ => false })));
    vm.register_native("is_countable", |vm, a| Ok(Val::Bool(match a.first() { Some(Val::Array(_)) => true, Some(Val::Object(h)) => vm.instance_of(vm.objects.get(*h).class, vm.wk().countable), _ => false })));
    vm.register_native("is_callable", |vm, a| Ok(Val::Bool(a.first().is_some_and(|v| vm.is_callable(v)))));
    vm.register_native("intval", intval);
    vm.register_native("floatval", |_, a| Ok(Val::Float(a.first().unwrap_or(&Val::Null).to_float()?)));
    vm.register_native("doubleval", |_, a| Ok(Val::Float(a.first().unwrap_or(&Val::Null).to_float()?)));
    vm.register_native("strval", |vm, a| Ok(Val::Str(vm.to_string_val(a.first().unwrap_or(&Val::Null))?)));
    vm.register_native("boolval", |_, a| Ok(Val::Bool(a.first().is_some_and(Val::to_bool))));
    vm.register_native("get_class", get_class);
    vm.register_native("get_parent_class", get_parent_class);
    vm.register_native("get_object_vars", |vm, a| match a.first() { Some(Val::Object(h)) => Ok(Val::array(vm.object_to_array(*h))), _ => Err(Throw::type_error("get_object_vars(): Argument #1 ($object) must be of type object")) });
    vm.register_native("get_class_methods", get_class_methods);
    vm.register_native("get_class_vars", get_class_vars);
    // class_alias('Original', 'Alias'): the alias names the same class from now on
    vm.register_native("class_alias", |vm, a| {
        let original = args::string(a, 0, "class_alias")?;
        let alias = args::string(a, 1, "class_alias")?;
        let class = vm.lookup_class(original.as_bytes()).ok_or_else(|| Throw::error(format!("Class \"{}\" not found", original.to_string_lossy())))?;
        let key = alias.as_bytes().strip_prefix(b"\\").unwrap_or(alias.as_bytes()).to_ascii_lowercase();
        if vm.class_by_name.contains_key(&key) {
            return Err(Throw::error(format!("Cannot declare class {}, because the name is already in use", alias.to_string_lossy())));
        }
        vm.class_by_name.insert(key, class);
        Ok(Val::Bool(true))
    });
    vm.register_native("method_exists", method_exists);
    vm.register_native("property_exists", property_exists);
    // number of arguments the calling user function received
    vm.register_native("func_num_args", |vm, _| {
        let f = vm.frames.last().ok_or_else(|| Throw::error("func_num_args() must be called from a function"))?;
        Ok(Val::Int(f.arg_present.count_ones() as i64))
    });
    // the arguments the calling user function received: the passed parameters
    // in order (current values, as in PHP), then the variadic ones
    vm.register_native("func_get_args", |vm, _| {
        let f = vm.frames.last().ok_or_else(|| Throw::error("func_get_args() cannot be called from the global scope"))?;
        let unit = &f.module.units[f.unit as usize];
        let mut out = PhpArray::new();
        for (i, p) in unit.params.iter().enumerate() {
            let value = vm.slot_stack[f.slot_base + p.slot as usize].clone().unwrap_or(Val::Null);
            if p.variadic {
                if let Val::Array(rest) = value {
                    for v in rest.values() {
                        out.push(v.clone());
                    }
                }
            } else if i < 64 && f.arg_present & (1u64 << i) != 0 {
                out.push(value);
            } else {
                break;
            }
        }
        Ok(Val::array(out))
    });
    vm.register_native("class_exists", |vm, a| Ok(Val::Bool(args::string(a, 0, "class_exists").ok().and_then(|s| vm.lookup_class(s.as_bytes())).is_some_and(|c| vm.class(c).kind == crate::bytecode::ClassKind::Class))));
    vm.register_native("interface_exists", |vm, a| Ok(Val::Bool(args::string(a, 0, "interface_exists").ok().and_then(|s| vm.lookup_class(s.as_bytes())).is_some_and(|c| vm.class(c).kind == crate::bytecode::ClassKind::Interface))));
    vm.register_native("trait_exists", |vm, a| Ok(Val::Bool(args::string(a, 0, "trait_exists").ok().and_then(|s| vm.lookup_class(s.as_bytes())).is_some_and(|c| vm.class(c).kind == crate::bytecode::ClassKind::Trait))));
    vm.register_native("enum_exists", |vm, a| Ok(Val::Bool(args::string(a, 0, "enum_exists").ok().and_then(|s| vm.lookup_class(s.as_bytes())).is_some_and(|c| vm.class(c).kind == crate::bytecode::ClassKind::Enum))));
    vm.register_native("function_exists", |vm, a| Ok(Val::Bool(args::string(a, 0, "function_exists").ok().is_some_and(|s| vm.functions.contains_key(&s.as_bytes().to_ascii_lowercase())))));
    vm.register_native("is_a", |vm, a| is_a(vm, a, false));
    vm.register_native("is_subclass_of", |vm, a| is_a(vm, a, true));
    vm.register_native("call_user_func", |vm, a| {
        args::count(a, 1, "call_user_func")?;
        vm.call_value(&a[0], a[1..].to_vec())
    });
    vm.register_native("call_user_func_array", |vm, a| {
        args::count(a, 2, "call_user_func_array")?;
        let arr = args::array(a, 1, "call_user_func_array")?;
        let mut pos = Vec::new();
        let mut named = Vec::new();
        for (k, v) in arr.iter() {
            match k {
                ArrayKey::Int(_) => pos.push(v.clone()),
                ArrayKey::Str(s) => named.push((s.as_bytes().to_vec(), v.clone())),
            }
        }
        vm.call_value_named(&a[0], pos, named)
    });
    vm.register_native("spl_object_id", |_, a| match a.first() { Some(Val::Object(h)) => Ok(Val::Int(h.0 as i64)), _ => Err(Throw::type_error("spl_object_id(): Argument #1 ($object) must be of type object")) });
    vm.register_native("spl_object_hash", |_, a| match a.first() { Some(Val::Object(h)) => Ok(Val::from(format!("{:032x}", h.0))), _ => Err(Throw::type_error("spl_object_hash(): Argument #1 ($object) must be of type object")) });
    vm.register_native("iterator_to_array", |vm, a| {
        args::count(a, 1, "iterator_to_array")?;
        let preserve = args::boolean(a, 1, true)?;
        let items = vm.iterate_to_vec(a[0].clone())?;
        let mut arr = PhpArray::new();
        for (k, v) in items {
            if preserve {
                let key = vm.to_array_key(&k)?;
                arr.insert(key, v);
            } else {
                arr.push(v);
            }
        }
        Ok(Val::array(arr))
    });
    vm.register_native("iterator_count", |vm, a| {
        args::count(a, 1, "iterator_count")?;
        Ok(Val::Int(vm.iterate_to_vec(a[0].clone())?.len() as i64))
    });
    vm.register_native("usleep", |_, _| Ok(Val::Null));
    vm.register_native("sleep", |_, _| Ok(Val::Int(0)));
    vm.register_native("error_log", |_, _| Ok(Val::Bool(true)));
    vm.register_native("trigger_error", |_, a| Err(Throw::new("ErrorException", args::string(a, 0, "trigger_error")?.to_string())));
    vm.register_native("assert", |_, a| if a.first().is_some_and(Val::to_bool) { Ok(Val::Bool(true)) } else { Err(Throw::new("AssertionError", "assert(false)")) });
}

fn gettype(_: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    Ok(Val::str(match a.first() {
        None | Some(Val::Null) => "NULL",
        Some(Val::Bool(_)) => "boolean",
        Some(Val::Int(_)) => "integer",
        Some(Val::Float(_)) => "double",
        Some(Val::Str(_)) => "string",
        Some(Val::Array(_)) => "array",
        Some(Val::Object(_)) => "object",
    }))
}

fn get_debug_type(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    Ok(Val::from(match a.first() {
        None | Some(Val::Null) => "null".to_string(),
        Some(v) => vm.value_type_name(v),
    }))
}

fn is_numeric(_: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    Ok(Val::Bool(match a.first() {
        Some(Val::Int(_) | Val::Float(_)) => true,
        Some(Val::Str(s)) => parse_numeric(s.as_bytes()).is_some_and(|(_, whole)| whole),
        _ => false,
    }))
}

fn intval(_: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let base = args::int_opt(a, 1, "intval", 10)?;
    match (a.first(), base) {
        (Some(Val::Str(s)), b) if b != 10 => {
            let t = s.to_string_lossy();
            let t = t.trim();
            let (neg, t) = match t.strip_prefix('-') {
                Some(r) => (true, r),
                None => (false, t.strip_prefix('+').unwrap_or(t)),
            };
            // base 0 detects it from the prefix like PHP (0x, 0o or 0, 0b, else 10);
            // other bases outside 2..36 give 0 as in PHP
            let b = if b == 0 {
                let l = t.to_ascii_lowercase();
                if l.starts_with("0x") { 16 } else if l.starts_with("0b") { 2 } else if l.starts_with("0o") || (l.starts_with('0') && l.len() > 1) { 8 } else { 10 }
            } else {
                b
            };
            if !(2..=36).contains(&b) {
                return Ok(Val::Int(0));
            }
            let t = match b {
                16 => t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t),
                8 => t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")).or_else(|| t.strip_prefix('0')).unwrap_or(t),
                2 => t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")).unwrap_or(t),
                _ => t,
            };
            let digits: String = t.chars().take_while(|c| c.is_digit(b as u32)).collect();
            let v = i64::from_str_radix(&digits, b as u32).unwrap_or(0);
            Ok(Val::Int(if neg { -v } else { v }))
        }
        (Some(v), _) => Ok(Val::Int(v.to_int()?)),
        (None, _) => Ok(Val::Int(0)),
    }
}

fn get_class(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    match a.first() {
        Some(Val::Object(h)) => Ok(Val::bytes(&vm.class(vm.objects.get(*h).class).name)),
        None => match vm.frames.last().and_then(|f| f.class) {
            Some(c) => Ok(Val::bytes(&vm.class(c).name)),
            None => Err(Throw::error("get_class() without arguments must be called from within a class")),
        },
        Some(other) => Err(Throw::type_error(format!("get_class(): Argument #1 ($object) must be of type object, {} given", other.type_name()))),
    }
}

fn get_parent_class(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let class = match a.first() {
        Some(Val::Object(h)) => Some(vm.objects.get(*h).class),
        Some(Val::Str(s)) => vm.lookup_class(s.as_bytes()),
        _ => vm.frames.last().and_then(|f| f.class),
    };
    Ok(match class.and_then(|c| vm.class(c).parent) {
        Some(p) => Val::bytes(&vm.class(p).name),
        None => Val::Bool(false),
    })
}

fn get_class_methods(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let class = match a.first() {
        Some(Val::Object(h)) => vm.objects.get(*h).class,
        Some(Val::Str(s)) => vm.lookup_class(s.as_bytes()).ok_or_else(|| Throw::type_error("get_class_methods(): Argument #1 must be an object or a valid class name"))?,
        _ => return Err(Throw::type_error("get_class_methods(): Argument #1 must be an object or a valid class name")),
    };
    let mut names: Vec<Vec<u8>> = vm.class(class).methods.values().map(|m| m.name.clone()).collect();
    names.sort();
    Ok(Val::array(PhpArray::from_values(names.into_iter().map(|n| Val::bytes(&n)))))
}

/// Default values of the properties visible from the calling scope, statics
/// included; null where there is no default (as in PHP).
fn get_class_vars(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let name = args::string(a, 0, "get_class_vars")?;
    let Some(class) = vm.lookup_class(name.as_bytes()) else { return Ok(Val::Bool(false)) };
    let scope = vm.frames.last().and_then(|f| f.class);
    let visible = |vm: &Vm, vis: crate::bytecode::Visibility, declaring: ClassId| match vis {
        crate::bytecode::Visibility::Public => true,
        crate::bytecode::Visibility::Protected => scope.is_some_and(|s| vm.instance_of(s, declaring) || vm.instance_of(declaring, s)),
        crate::bytecode::Visibility::Private => scope == Some(declaring),
    };
    let mut out = PhpArray::new();
    for p in vm.class(class).props.clone() {
        if !visible(vm, p.visibility, p.declaring) {
            continue;
        }
        let v = match &p.default {
            Some(init) => vm.eval_init(init, p.module, Some(p.declaring))?,
            None => Val::Null,
        };
        out.insert(ArrayKey::from_str(&String::from_utf8_lossy(&p.name)), v);
    }
    let statics: Vec<(Vec<u8>, crate::vm::class::StaticPropInfo)> = vm.class(class).static_props.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (n, info) in statics {
        if !visible(vm, info.visibility, info.declaring) {
            continue;
        }
        let v = match &info.init {
            Some(init) => vm.eval_init(init, info.module, Some(info.declaring))?,
            None => Val::Null,
        };
        out.insert(ArrayKey::from_str(&String::from_utf8_lossy(&n)), v);
    }
    Ok(Val::array(out))
}

fn method_exists(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let class = match a.first() {
        Some(Val::Object(h)) => Some(vm.objects.get(*h).class),
        Some(Val::Str(s)) => vm.lookup_class(s.as_bytes()),
        _ => None,
    };
    let name = args::string(a, 1, "method_exists")?;
    Ok(Val::Bool(class.is_some_and(|c| vm.class(c).find_method(name.as_bytes()).is_some())))
}

fn property_exists(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let name = args::string(a, 1, "property_exists")?;
    Ok(Val::Bool(match a.first() {
        Some(Val::Object(h)) => {
            let obj = vm.objects.get(*h);
            vm.class(obj.class).prop_index.contains_key(name.as_bytes()) || obj.dynamic.contains_key(&name) || vm.class(obj.class).static_props.contains_key(name.as_bytes())
        }
        Some(Val::Str(s)) => vm.lookup_class(s.as_bytes()).is_some_and(|c| vm.class(c).prop_index.contains_key(name.as_bytes()) || vm.class(c).static_props.contains_key(name.as_bytes())),
        _ => false,
    }))
}

fn is_a(vm: &mut Vm, a: &[Val], strict_sub: bool) -> Result<Val, Throw> {
    let allow_string = args::boolean(a, 2, strict_sub)?;
    let class = match a.first() {
        Some(Val::Object(h)) => Some(vm.objects.get(*h).class),
        Some(Val::Str(s)) if allow_string => vm.lookup_class(s.as_bytes()),
        _ => None,
    };
    let target = args::string(a, 1, "is_a").ok().and_then(|s| vm.lookup_class(s.as_bytes()));
    Ok(Val::Bool(match (class, target) {
        (Some(c), Some(t)) => vm.instance_of(c, t) && !(strict_sub && c == t),
        _ => false,
    }))
}
