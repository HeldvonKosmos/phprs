//! var_dump, print_r, var_export, echo helpers.

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.register_native("var_dump", |vm, a| {
        for v in a {
            let mut out = Vec::new();
            var_dump(vm, v, 0, &mut out)?;
            vm.output.extend_from_slice(&out);
        }
        Ok(Val::Null)
    });
    vm.register_native("print_r", |vm, a| {
        args::count(a, 1, "print_r")?;
        let mut out = Vec::new();
        print_r(vm, &a[0], 0, &mut out)?;
        if args::boolean(a, 1, false) {
            Ok(Val::bytes(&out))
        } else {
            vm.output.extend_from_slice(&out);
            Ok(Val::Bool(true))
        }
    });
    vm.register_native("var_export", |vm, a| {
        args::count(a, 1, "var_export")?;
        let mut out = Vec::new();
        var_export(vm, &a[0], 0, &mut out)?;
        if args::boolean(a, 1, false) {
            Ok(Val::bytes(&out))
        } else {
            vm.output.extend_from_slice(&out);
            Ok(Val::Null)
        }
    });
    vm.register_native("dump", |vm, a| {
        for v in a {
            let mut out = Vec::new();
            var_dump(vm, v, 0, &mut out)?;
            vm.output.extend_from_slice(&out);
        }
        Ok(a.first().cloned().unwrap_or(Val::Null))
    });
}

fn indent(out: &mut Vec<u8>, n: usize) {
    out.extend(std::iter::repeat_n(b' ', n));
}

fn dump_float(f: f64) -> String {
    if f.is_nan() {
        return "NAN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "INF".into() } else { "-INF".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    if f == f.trunc() && f.abs() < 1e15 {
        return format!("{}", f as i64);
    }
    // shortest round-trip digits (serialize_precision = -1); like PHP's
    // zend_gcvt with 17 digits, exponential form when the decimal exponent
    // is below -4 or above 16, positional otherwise
    let e = format!("{f:e}");
    let (m, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let ev: i32 = exp.parse().unwrap_or(0);
    if !(-4..=16).contains(&ev) {
        let m = if m.contains('.') { m.to_string() } else { format!("{m}.0") };
        return format!("{m}E{}{}", if ev < 0 { "-" } else { "+" }, ev.abs());
    }
    format!("{f}")
}

pub(crate) fn var_dump(vm: &mut Vm, v: &Val, depth: usize, out: &mut Vec<u8>) -> Result<(), Throw> {
    match v {
        Val::Null => out.extend_from_slice(b"NULL\n"),
        Val::Bool(b) => out.extend_from_slice(format!("bool({b})\n").as_bytes()),
        Val::Int(i) => out.extend_from_slice(format!("int({i})\n").as_bytes()),
        Val::Float(f) => out.extend_from_slice(format!("float({})\n", dump_float(*f)).as_bytes()),
        Val::Str(s) => {
            out.extend_from_slice(format!("string({}) \"", s.len()).as_bytes());
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\"\n");
        }
        Val::Array(a) => {
            out.extend_from_slice(format!("array({}) {{\n", a.len()).as_bytes());
            for (k, val) in a.iter() {
                indent(out, depth + 2);
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}]=>\n").as_bytes()),
                    ArrayKey::Str(s) => out.extend_from_slice(format!("[\"{s}\"]=>\n").as_bytes()),
                }
                indent(out, depth + 2);
                var_dump(vm, val, depth + 2, out)?;
            }
            indent(out, depth);
            out.extend_from_slice(b"}\n");
        }
        Val::Object(h) => {
            let class = vm.objects.get(*h).class;
            let cname = String::from_utf8_lossy(&vm.class(class).name).into_owned();
            if vm.class(class).is_enum() {
                let name = vm.get_prop_raw(*h, b"name").map(|n| n.to_php_string().map(|s| s.to_string()).unwrap_or_default()).unwrap_or_default();
                out.extend_from_slice(format!("enum({cname}::{name})\n").as_bytes());
                return Ok(());
            }
            let props = object_props(vm, *h);
            out.extend_from_slice(format!("object({cname})#{} ({}) {{\n", h.0, props.len()).as_bytes());
            for (label, val) in props {
                indent(out, depth + 2);
                out.extend_from_slice(format!("[{label}]=>\n").as_bytes());
                indent(out, depth + 2);
                match val {
                    Some(v) => var_dump(vm, &v, depth + 2, out)?,
                    None => out.extend_from_slice(b"uninitialized(mixed)\n"),
                }
            }
            indent(out, depth);
            out.extend_from_slice(b"}\n");
        }
    }
    Ok(())
}

/// Property list with var_dump-style labels: `"name"`, `"name":protected`, `"name":"Class":private`.
fn object_props(vm: &Vm, h: ObjHandle) -> Vec<(String, Option<Val>)> {
    let obj = vm.objects.get(h);
    let class = vm.class(obj.class);
    let mut out = Vec::new();
    for (i, p) in class.props.iter().enumerate() {
        let name = String::from_utf8_lossy(&p.name);
        let label = match p.visibility {
            crate::bytecode::Visibility::Public => format!("\"{name}\""),
            crate::bytecode::Visibility::Protected => format!("\"{name}\":protected"),
            crate::bytecode::Visibility::Private => format!("\"{name}\":\"{}\":private", String::from_utf8_lossy(&vm.class(p.declaring).name)),
        };
        out.push((label, obj.slots[i].clone()));
    }
    for (k, v) in &obj.dynamic {
        out.push((format!("\"{k}\""), Some(v.clone())));
    }
    out
}

pub(crate) fn print_r(vm: &mut Vm, v: &Val, depth: usize, out: &mut Vec<u8>) -> Result<(), Throw> {
    match v {
        Val::Array(a) => {
            out.extend_from_slice(b"Array\n");
            indent(out, depth);
            out.extend_from_slice(b"(\n");
            for (k, val) in a.iter() {
                indent(out, depth + 4);
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(format!("[{i}] => ").as_bytes()),
                    ArrayKey::Str(s) => out.extend_from_slice(format!("[{s}] => ").as_bytes()),
                }
                print_r(vm, val, depth + 8, out)?;
                out.push(b'\n');
            }
            indent(out, depth);
            out.extend_from_slice(b")\n");
        }
        Val::Object(h) => {
            let class = vm.objects.get(*h).class;
            let cname = String::from_utf8_lossy(&vm.class(class).name).into_owned();
            if vm.class(class).is_enum() {
                let name = vm.get_prop_raw(*h, b"name").map(|n| n.to_php_string().map(|s| s.to_string()).unwrap_or_default()).unwrap_or_default();
                out.extend_from_slice(format!("{cname} Enum ( [name] => {name}").as_bytes());
                if let Some(v) = vm.get_prop_raw(*h, b"value") {
                    out.extend_from_slice(b" [value] => ");
                    print_r(vm, &v, depth, out)?;
                }
                out.extend_from_slice(b" )\n");
                return Ok(());
            }
            out.extend_from_slice(format!("{cname} Object\n").as_bytes());
            indent(out, depth);
            out.extend_from_slice(b"(\n");
            let obj = vm.objects.get(*h);
            let props: Vec<(String, Option<Val>)> = vm.class(class).props.iter().enumerate().map(|(i, p)| {
                let name = String::from_utf8_lossy(&p.name).into_owned();
                let label = match p.visibility {
                    crate::bytecode::Visibility::Public => name,
                    crate::bytecode::Visibility::Protected => format!("{name}:protected"),
                    crate::bytecode::Visibility::Private => format!("{name}:{}:private", String::from_utf8_lossy(&vm.class(p.declaring).name)),
                };
                (label, obj.slots[i].clone())
            }).chain(obj.dynamic.iter().map(|(k, v)| (k.to_string(), Some(v.clone())))).collect();
            for (label, val) in props {
                indent(out, depth + 4);
                out.extend_from_slice(format!("[{label}] => ").as_bytes());
                if let Some(v) = val {
                    print_r(vm, &v, depth + 8, out)?;
                }
                out.push(b'\n');
            }
            indent(out, depth);
            out.extend_from_slice(b")\n");
        }
        other => {
            let s = vm.to_string_val(other)?;
            out.extend_from_slice(s.as_bytes());
        }
    }
    Ok(())
}

fn export_string(s: &PhpString) -> Vec<u8> {
    let mut out = vec![b'\''];
    for b in s.as_bytes() {
        match b {
            b'\'' => out.extend_from_slice(b"\\'"),
            b'\\' => out.extend_from_slice(b"\\\\"),
            _ => out.push(*b),
        }
    }
    out.push(b'\'');
    out
}

pub(crate) fn var_export(vm: &mut Vm, v: &Val, depth: usize, out: &mut Vec<u8>) -> Result<(), Throw> {
    match v {
        Val::Null => out.extend_from_slice(b"NULL"),
        Val::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Val::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Val::Float(f) => {
            let s = dump_float(*f);
            out.extend_from_slice(s.as_bytes());
            if !s.contains('.') && !s.contains('E') && !s.contains('N') && !s.contains('I') {
                out.extend_from_slice(b".0");
            }
        }
        Val::Str(s) => out.extend(export_string(s)),
        Val::Array(a) => {
            out.extend_from_slice(b"array (\n");
            for (k, val) in a.iter() {
                indent(out, depth + 2);
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(format!("{i} => ").as_bytes()),
                    ArrayKey::Str(s) => {
                        out.extend(export_string(s));
                        out.extend_from_slice(b" => ");
                    }
                }
                if matches!(val, Val::Array(_) | Val::Object(_)) {
                    out.push(b'\n');
                    indent(out, depth + 2);
                }
                var_export(vm, val, depth + 2, out)?;
                out.extend_from_slice(b",\n");
            }
            indent(out, depth);
            out.push(b')');
        }
        Val::Object(h) => {
            let class = vm.objects.get(*h).class;
            let cname = String::from_utf8_lossy(&vm.class(class).name).into_owned();
            if vm.class(class).is_enum() {
                let name = vm.get_prop_raw(*h, b"name").map(|n| n.to_php_string().map(|s| s.to_string()).unwrap_or_default()).unwrap_or_default();
                out.extend_from_slice(format!("\\{cname}::{name}").as_bytes());
                return Ok(());
            }
            out.extend_from_slice(format!("\\{cname}::__set_state(array(\n").as_bytes());
            let props = vm.object_to_array(*h);
            for (k, val) in props.iter() {
                indent(out, depth + 3);
                match k {
                    ArrayKey::Int(i) => out.extend_from_slice(format!("{i} => ").as_bytes()),
                    ArrayKey::Str(s) => {
                        out.extend(export_string(s));
                        out.extend_from_slice(b" => ");
                    }
                }
                if matches!(val, Val::Array(_) | Val::Object(_)) {
                    out.push(b'\n');
                    indent(out, depth + 2);
                }
                var_export(vm, val, depth + 2, out)?;
                out.extend_from_slice(b",\n");
            }
            indent(out, depth);
            out.extend_from_slice(b"))");
        }
    }
    Ok(())
}
