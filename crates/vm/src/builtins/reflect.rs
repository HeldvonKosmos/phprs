//! Native introspection behind the prelude's Reflection classes.

use super::args;
use crate::bytecode::{TypeHint, Visibility};
use crate::error::Throw;
use crate::value::*;
use crate::vm::{ClosureData, FuncRef, Vm};

pub fn register(vm: &mut Vm) {
    vm.register_native("__grd_class_info", class_info);
    vm.register_native("__grd_function_info", function_info);
    vm.register_native("__grd_eval_default", |vm, a| {
        // (class|null, method|function name, param index) → default value
        let class = match a.first() { Some(Val::Str(s)) => vm.lookup_class(s.as_bytes()), _ => None };
        let name = args::string(a, 1, "reflection")?;
        let index = args::int(a, 2, "reflection")? as usize;
        let (module, unit) = find_unit(vm, class, name.as_bytes())?;
        let unit_ref = &vm.modules[module as usize].units[unit as usize];
        let param = unit_ref.params.get(index).ok_or_else(|| Throw::new("ReflectionException", "Parameter has no default value"))?;
        match param.default_init.clone() {
            Some(init) => vm.eval_init(&init, module, class),
            None => Err(Throw::new("ReflectionException", "Internal error: Failed to retrieve the default value")),
        }
    });
    vm.register_native("__grd_eval_attr_args", |vm, a| {
        // (owner kind, class|fn name, member name|'' , index, attribute index) → array of args
        let kind = args::string(a, 0, "reflection")?.to_string_lossy();
        let owner = args::string(a, 1, "reflection")?;
        let member = args::string(a, 2, "reflection")?;
        let param_index = args::int(a, 3, "reflection")?;
        let attr_index = args::int(a, 4, "reflection")? as usize;
        let (attrs, module, class) = collect_attrs(vm, &kind, owner.as_bytes(), member.as_bytes(), param_index)?;
        let attr = attrs.get(attr_index).ok_or_else(|| Throw::new("ReflectionException", "Attribute not found"))?.clone();
        let mut out = PhpArray::new();
        for (name, init) in &attr.args {
            let v = vm.eval_init(init, module, class)?;
            match name {
                Some(n) => {
                    let n = vm.modules[module as usize].str(*n).to_vec();
                    out.insert(ArrayKey::Str(PhpString::from(n)), v);
                }
                None => out.push(v),
            }
        }
        Ok(Val::array(out))
    });
    vm.register_native("__grd_prop_get", |vm, a| {
        let Some(Val::Object(h)) = a.first() else { return Err(Throw::type_error("object expected")) };
        let name = args::string(a, 1, "reflection")?;
        Ok(vm.get_prop_raw(*h, name.as_bytes()).unwrap_or(Val::Null))
    });
    vm.register_native("__grd_prop_initialized", |vm, a| {
        let Some(Val::Object(h)) = a.first() else { return Err(Throw::type_error("object expected")) };
        let name = args::string(a, 1, "reflection")?;
        Ok(Val::Bool(vm.get_prop_raw(*h, name.as_bytes()).is_some()))
    });
    vm.register_native("__grd_prop_set", |vm, a| {
        let Some(Val::Object(h)) = a.first() else { return Err(Throw::type_error("object expected")) };
        let name = args::string(a, 1, "reflection")?;
        vm.set_prop_raw(*h, name.as_bytes(), a.get(2).cloned().unwrap_or(Val::Null));
        Ok(Val::Null)
    });
    vm.register_native("__grd_static_get", |vm, a| {
        let class = lookup(vm, &args::string(a, 0, "reflection")?)?;
        let name = args::string(a, 1, "reflection")?;
        vm.static_prop_get(class, name.as_bytes())
    });
    vm.register_native("__grd_static_set", |vm, a| {
        let class = lookup(vm, &args::string(a, 0, "reflection")?)?;
        let name = args::string(a, 1, "reflection")?;
        vm.static_prop_set(class, name.as_bytes(), a.get(2).cloned().unwrap_or(Val::Null))?;
        Ok(Val::Null)
    });
    vm.register_native("__grd_invoke_method", |vm, a| {
        // (object|null, class, method, args) with visibility bypassed
        let class = lookup(vm, &args::string(a, 1, "reflection")?)?;
        let name = args::string(a, 2, "reflection")?;
        let call_args: Vec<Val> = match a.get(3) { Some(Val::Array(arr)) => arr.values().cloned().collect(), _ => Vec::new() };
        let info = vm.find_method_info(class, name.as_bytes()).ok_or_else(|| Throw::new("ReflectionException", format!("Method {}::{}() does not exist", String::from_utf8_lossy(&vm.class(class).name), name)))?;
        let depth = vm.frames.len();
        let outcome = match a.first() {
            Some(Val::Object(h)) => vm.invoke_method(*h, name.as_bytes(), call_args, Vec::new(), Some(info.declaring))?,
            _ => vm.invoke_static(class, name.as_bytes(), call_args, Vec::new(), None, Some(info.declaring), class)?,
        };
        match outcome {
            crate::vm::CallOutcome::Value(v) => Ok(v),
            crate::vm::CallOutcome::Pushed => vm.run(depth),
        }
    });
    vm.register_native("__grd_new_without_ctor", |vm, a| {
        let class = lookup(vm, &args::string(a, 0, "reflection")?)?;
        Ok(Val::Object(vm.alloc_object(class)?))
    });
    vm.register_native("__grd_class_constant", |vm, a| {
        let class = lookup(vm, &args::string(a, 0, "reflection")?)?;
        let name = args::string(a, 1, "reflection")?;
        vm.class_constant(class, name.as_bytes())
    });
    vm.register_native("__grd_closure_info", |vm, a| {
        let Some(Val::Object(h)) = a.first() else { return Err(Throw::type_error("Closure expected")) };
        let data = vm.closure_data(*h);
        Ok(match data {
            ClosureData::User { module, unit, .. } => unit_info(vm, module, unit, None, b"{closure}"),
            ClosureData::Function(FuncRef::User { module, unit }) => {
                let name = vm.modules[module as usize].units[unit as usize].name.clone().into_bytes();
                unit_info(vm, module, unit, None, &name)
            }
            ClosureData::Function(FuncRef::Native(i)) => native_info(&vm.natives[i as usize].0.clone()),
            ClosureData::BoundMethod { this, name } => {
                let class = vm.objects.get(this).class;
                method_info_by_name(vm, class, &name)?
            }
            ClosureData::StaticMethod { class, name } => method_info_by_name(vm, class, &name)?,
        })
    });
}

fn lookup(vm: &Vm, name: &PhpString) -> Result<crate::value::ClassId, Throw> {
    vm.lookup_class(name.as_bytes()).ok_or_else(|| Throw::new("ReflectionException", format!("Class \"{name}\" does not exist")))
}

fn vis_str(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

fn type_info(vm: &Vm, t: &TypeHint, module: u32) -> Val {
    // types of native classes are not described here
    if module == crate::vm::native::NATIVE_MODULE {
        return Val::Null;
    }
    let m = vm.modules[module as usize].clone();
    fn named(name: String, nullable: bool, builtin: bool) -> Val {
        let mut a = PhpArray::new();
        a.insert(ArrayKey::from_str("kind"), Val::str("named"));
        a.insert(ArrayKey::from_str("name"), Val::from(name));
        a.insert(ArrayKey::from_str("nullable"), Val::Bool(nullable));
        a.insert(ArrayKey::from_str("builtin"), Val::Bool(builtin));
        Val::array(a)
    }
    match t {
        TypeHint::Nullable(inner) => {
            let v = type_info(vm, inner, module);
            if let Val::Array(a) = &v {
                let mut b = (**a).clone();
                b.insert(ArrayKey::from_str("nullable"), Val::Bool(true));
                return Val::array(b);
            }
            v
        }
        TypeHint::Union(parts) | TypeHint::Intersection(parts) => {
            let nullable = parts.iter().any(|p| matches!(p, TypeHint::Null));
            let mut a = PhpArray::new();
            a.insert(ArrayKey::from_str("kind"), Val::str(if matches!(t, TypeHint::Union(_)) { "union" } else { "intersection" }));
            a.insert(ArrayKey::from_str("name"), Val::from(vm.type_name_of(t, &m)));
            a.insert(ArrayKey::from_str("nullable"), Val::Bool(nullable));
            a.insert(ArrayKey::from_str("builtin"), Val::Bool(false));
            a.insert(ArrayKey::from_str("types"), Val::array(PhpArray::from_values(parts.iter().filter(|p| !matches!(p, TypeHint::Null)).map(|p| type_info(vm, p, module)))));
            Val::array(a)
        }
        TypeHint::Class(_) | TypeHint::SelfT | TypeHint::Static | TypeHint::Parent => named(vm.type_name_of(t, &m), false, false),
        TypeHint::Mixed | TypeHint::Null => named(vm.type_name_of(t, &m), true, true),
        other => named(vm.type_name_of(other, &m), false, true),
    }
}

fn attrs_to_val(vm: &Vm, attrs: &[crate::bytecode::AttributeDecl], module: u32) -> Val {
    // native classes (module NATIVE_MODULE) carry no attributes
    if attrs.is_empty() || module == crate::vm::native::NATIVE_MODULE {
        return Val::empty_array();
    }
    let m = &vm.modules[module as usize];
    Val::array(PhpArray::from_values(attrs.iter().map(|a| {
        let mut e = PhpArray::new();
        e.insert(ArrayKey::from_str("name"), Val::bytes(m.str(a.name)));
        e.insert(ArrayKey::from_str("argc"), Val::Int(a.args.len() as i64));
        Val::array(e)
    })))
}

fn params_info(vm: &Vm, module: u32, unit: crate::bytecode::UnitId) -> Val {
    let m = &vm.modules[module as usize];
    let u = &m.units[unit as usize];
    Val::array(PhpArray::from_values(u.params.iter().enumerate().map(|(i, p)| {
        let mut e = PhpArray::new();
        e.insert(ArrayKey::from_str("name"), Val::bytes(m.str(p.name)));
        e.insert(ArrayKey::from_str("position"), Val::Int(i as i64));
        e.insert(ArrayKey::from_str("type"), p.ty.as_ref().map(|t| type_info(vm, t, module)).unwrap_or(Val::Null));
        e.insert(ArrayKey::from_str("optional"), Val::Bool(p.has_default || p.variadic));
        e.insert(ArrayKey::from_str("hasDefault"), Val::Bool(p.default_init.is_some()));
        e.insert(ArrayKey::from_str("variadic"), Val::Bool(p.variadic));
        e.insert(ArrayKey::from_str("promoted"), Val::Bool(p.promote.is_some()));
        e.insert(ArrayKey::from_str("attributes"), attrs_to_val(vm, &p.attributes, module));
        Val::array(e)
    })))
}

fn unit_info(vm: &Vm, module: u32, unit: crate::bytecode::UnitId, declaring: Option<crate::value::ClassId>, name: &[u8]) -> Val {
    let u = &vm.modules[module as usize].units[unit as usize];
    let mut e = PhpArray::new();
    e.insert(ArrayKey::from_str("name"), Val::bytes(name));
    e.insert(ArrayKey::from_str("params"), params_info(vm, module, unit));
    e.insert(ArrayKey::from_str("required"), Val::Int(u.required_params as i64));
    e.insert(ArrayKey::from_str("returnType"), u.return_type.as_ref().map(|t| type_info(vm, t, module)).unwrap_or(Val::Null));
    e.insert(ArrayKey::from_str("generator"), Val::Bool(u.is_generator));
    if let Some(d) = declaring {
        e.insert(ArrayKey::from_str("class"), Val::bytes(&vm.class(d).name));
    }
    let attrs = vm.modules[module as usize].functions.iter().find(|f| f.unit == unit).map(|f| f.attributes.clone()).unwrap_or_default();
    e.insert(ArrayKey::from_str("attributes"), attrs_to_val(vm, &attrs, module));
    Val::array(e)
}

fn native_info(name: &str) -> Val {
    let mut e = PhpArray::new();
    e.insert(ArrayKey::from_str("name"), Val::from(name.to_string()));
    e.insert(ArrayKey::from_str("params"), Val::empty_array());
    e.insert(ArrayKey::from_str("required"), Val::Int(0));
    e.insert(ArrayKey::from_str("returnType"), Val::Null);
    e.insert(ArrayKey::from_str("generator"), Val::Bool(false));
    e.insert(ArrayKey::from_str("native"), Val::Bool(true));
    e.insert(ArrayKey::from_str("attributes"), Val::empty_array());
    Val::array(e)
}

fn method_info_by_name(vm: &Vm, class: crate::value::ClassId, name: &[u8]) -> Result<Val, Throw> {
    let m = vm.find_method_info(class, name).ok_or_else(|| Throw::new("ReflectionException", format!("Method {}::{}() does not exist", String::from_utf8_lossy(&vm.class(class).name), String::from_utf8_lossy(name))))?;
    Ok(method_info(vm, &m))
}

fn method_info(vm: &Vm, m: &crate::vm::MethodInfo) -> Val {
    let mut e = PhpArray::new();
    e.insert(ArrayKey::from_str("name"), Val::bytes(&m.name));
    e.insert(ArrayKey::from_str("visibility"), Val::str(vis_str(m.visibility)));
    e.insert(ArrayKey::from_str("static"), Val::Bool(m.is_static));
    e.insert(ArrayKey::from_str("abstract"), Val::Bool(m.unit.is_none()));
    e.insert(ArrayKey::from_str("final"), Val::Bool(m.is_final));
    e.insert(ArrayKey::from_str("class"), Val::bytes(&vm.class(m.declaring).name));
    match m.unit {
        Some(u) => {
            e.insert(ArrayKey::from_str("params"), params_info(vm, m.module, u));
            let unit = &vm.modules[m.module as usize].units[u as usize];
            e.insert(ArrayKey::from_str("required"), Val::Int(unit.required_params as i64));
            e.insert(ArrayKey::from_str("returnType"), unit.return_type.as_ref().map(|t| type_info(vm, t, m.module)).unwrap_or(Val::Null));
        }
        None => {
            e.insert(ArrayKey::from_str("params"), Val::empty_array());
            e.insert(ArrayKey::from_str("required"), Val::Int(0));
            e.insert(ArrayKey::from_str("returnType"), Val::Null);
        }
    }
    e.insert(ArrayKey::from_str("attributes"), attrs_to_val(vm, &m.attributes, m.module));
    Val::array(e)
}

fn class_info(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let class = match a.first() {
        Some(Val::Object(h)) => vm.objects.get(*h).class,
        Some(Val::Str(s)) => match vm.lookup_class(s.as_bytes()) { Some(c) => c, None => return Ok(Val::Bool(false)) },
        _ => return Ok(Val::Bool(false)),
    };
    let c = vm.class(class);
    let module = c.module;
    let mut e = PhpArray::new();
    let name = String::from_utf8_lossy(&c.name).into_owned();
    e.insert(ArrayKey::from_str("name"), Val::from(name.clone()));
    let (ns, short) = match name.rfind('\\') { Some(i) => (name[..i].to_string(), name[i + 1..].to_string()), None => (String::new(), name.clone()) };
    e.insert(ArrayKey::from_str("short"), Val::from(short));
    e.insert(ArrayKey::from_str("namespace"), Val::from(ns));
    e.insert(ArrayKey::from_str("kind"), Val::str(match c.kind { crate::bytecode::ClassKind::Class => "class", crate::bytecode::ClassKind::Interface => "interface", crate::bytecode::ClassKind::Trait => "trait", crate::bytecode::ClassKind::Enum => "enum" }));
    e.insert(ArrayKey::from_str("abstract"), Val::Bool(c.is_abstract));
    e.insert(ArrayKey::from_str("final"), Val::Bool(c.is_final));
    e.insert(ArrayKey::from_str("parent"), c.parent.map(|p| Val::bytes(&vm.class(p).name)).unwrap_or(Val::Null));
    e.insert(ArrayKey::from_str("interfaces"), Val::array(PhpArray::from_values(c.interfaces.iter().map(|i| Val::bytes(&vm.class(*i).name)))));
    let mut consts = PhpArray::new();
    let const_names: Vec<Vec<u8>> = c.consts.keys().cloned().collect();
    let cases: Vec<Vec<u8>> = c.cases.iter().map(|(n, _)| n.clone()).collect();
    let mut props = PhpArray::new();
    for p in &c.props {
        let mut pe = PhpArray::new();
        pe.insert(ArrayKey::from_str("name"), Val::bytes(&p.name));
        pe.insert(ArrayKey::from_str("visibility"), Val::str(vis_str(p.visibility)));
        pe.insert(ArrayKey::from_str("static"), Val::Bool(false));
        pe.insert(ArrayKey::from_str("readonly"), Val::Bool(p.readonly));
        pe.insert(ArrayKey::from_str("type"), p.ty.as_ref().map(|t| type_info(vm, t, p.module)).unwrap_or(Val::Null));
        pe.insert(ArrayKey::from_str("hasDefault"), Val::Bool(p.default.is_some()));
        pe.insert(ArrayKey::from_str("class"), Val::bytes(&vm.class(p.declaring).name));
        pe.insert(ArrayKey::from_str("attributes"), attrs_to_val(vm, &p.attributes, p.module));
        props.push(Val::array(pe));
    }
    for (n, sp) in &c.static_props {
        let mut pe = PhpArray::new();
        pe.insert(ArrayKey::from_str("name"), Val::bytes(n));
        pe.insert(ArrayKey::from_str("visibility"), Val::str(vis_str(sp.visibility)));
        pe.insert(ArrayKey::from_str("static"), Val::Bool(true));
        pe.insert(ArrayKey::from_str("readonly"), Val::Bool(false));
        pe.insert(ArrayKey::from_str("type"), Val::Null);
        pe.insert(ArrayKey::from_str("hasDefault"), Val::Bool(sp.init.is_some()));
        pe.insert(ArrayKey::from_str("class"), Val::bytes(&vm.class(sp.declaring).name));
        pe.insert(ArrayKey::from_str("attributes"), Val::empty_array());
        props.push(Val::array(pe));
    }
    let mut methods: Vec<crate::vm::MethodInfo> = c.methods.values().cloned().collect();
    methods.sort_by(|x, y| (x.declaring != class, x.name.to_ascii_lowercase()).cmp(&(y.declaring != class, y.name.to_ascii_lowercase())));
    let attrs = attrs_to_val(vm, &c.attributes, module);
    let method_vals: Vec<Val> = methods.iter().map(|m| method_info(vm, m)).collect();
    e.insert(ArrayKey::from_str("methods"), Val::array(PhpArray::from_values(method_vals)));
    e.insert(ArrayKey::from_str("properties"), Val::array(props));
    e.insert(ArrayKey::from_str("attributes"), attrs);
    e.insert(ArrayKey::from_str("cases"), Val::array(PhpArray::from_values(cases.iter().map(|n| Val::bytes(n)))));
    for n in const_names {
        let v = vm.class_constant(class, &n)?;
        consts.insert(ArrayKey::Str(PhpString::from(n)), v);
    }
    e.insert(ArrayKey::from_str("constants"), Val::array(consts));
    Ok(Val::array(e))
}

fn function_info(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    match a.first() {
        Some(Val::Str(s)) => {
            let f = vm.functions.get(&s.as_bytes().to_ascii_lowercase()).cloned().ok_or_else(|| Throw::new("ReflectionException", format!("Function {s}() does not exist")))?;
            Ok(match f {
                FuncRef::User { module, unit } => unit_info(vm, module, unit, None, s.as_bytes()),
                FuncRef::Native(i) => native_info(&vm.natives[i as usize].0.clone()),
            })
        }
        _ => Err(Throw::new("ReflectionException", "Function name expected")),
    }
}

fn find_unit(vm: &Vm, class: Option<crate::value::ClassId>, name: &[u8]) -> Result<(u32, crate::bytecode::UnitId), Throw> {
    match class {
        Some(c) => {
            let m = vm.find_method_info(c, name).ok_or_else(|| Throw::new("ReflectionException", "Method does not exist"))?;
            let u = m.unit.ok_or_else(|| Throw::new("ReflectionException", "Abstract method"))?;
            Ok((m.module, u))
        }
        None => match vm.functions.get(&name.to_ascii_lowercase()) {
            Some(FuncRef::User { module, unit }) => Ok((*module, *unit)),
            _ => Err(Throw::new("ReflectionException", "Function does not exist")),
        },
    }
}

/// Attributes of (kind: class|method|property|param|function) with module and class context.
fn collect_attrs(vm: &Vm, kind: &str, owner: &[u8], member: &[u8], param_index: i64) -> Result<(Vec<crate::bytecode::AttributeDecl>, u32, Option<crate::value::ClassId>), Throw> {
    match kind {
        "class" => {
            let c = vm.lookup_class(owner).ok_or_else(|| Throw::new("ReflectionException", "Class does not exist"))?;
            let rt = vm.class(c);
            Ok((rt.attributes.clone(), rt.module, Some(c)))
        }
        "method" => {
            let c = vm.lookup_class(owner).ok_or_else(|| Throw::new("ReflectionException", "Class does not exist"))?;
            let m = vm.find_method_info(c, member).ok_or_else(|| Throw::new("ReflectionException", "Method does not exist"))?;
            Ok((m.attributes.clone(), m.module, Some(m.declaring)))
        }
        "property" => {
            let c = vm.lookup_class(owner).ok_or_else(|| Throw::new("ReflectionException", "Class does not exist"))?;
            let rt = vm.class(c);
            let p = rt.props.iter().find(|p| p.name == member).ok_or_else(|| Throw::new("ReflectionException", "Property does not exist"))?;
            Ok((p.attributes.clone(), p.module, Some(p.declaring)))
        }
        "param" => {
            let class = if owner.is_empty() { None } else { Some(vm.lookup_class(owner).ok_or_else(|| Throw::new("ReflectionException", "Class does not exist"))?) };
            let (module, unit) = find_unit(vm, class, member)?;
            let p = vm.modules[module as usize].units[unit as usize].params.get(param_index as usize).ok_or_else(|| Throw::new("ReflectionException", "Parameter does not exist"))?;
            Ok((p.attributes.clone(), module, class))
        }
        "function" => {
            let (module, unit) = find_unit(vm, None, member)?;
            let attrs = vm.modules[module as usize].functions.iter().find(|f| f.unit == unit).map(|f| f.attributes.clone()).unwrap_or_default();
            Ok((attrs, module, None))
        }
        _ => Err(Throw::new("ReflectionException", "Unknown attribute owner")),
    }
}
