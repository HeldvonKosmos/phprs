//! Constructor injection (I1, D1 "Konvention statt Registrierung"): the
//! framework creates controllers and middleware with `Vm::resolve`, which
//! fills every constructor parameter typed with a class or interface by
//! resolving that type in turn. An interface or abstract class binds to its
//! one concrete implementation among the loaded classes; none or several is
//! an error with code GRD022 (no registration code, so the binding must be
//! unambiguous). Parameters that cannot be resolved use their default; a
//! host can supply objects for particular classes (`set_injector`, e.g. the
//! current request).

use crate::bytecode::{ClassKind, TypeHint};
use crate::error::Throw;
use crate::value::{ObjHandle, Val};

use super::{ClassId, Vm};

/// Supplies an object for a class (`Ok(None)`: resolve normally).
pub type Injector = fn(&mut Vm, ClassId) -> Result<Option<Val>, Throw>;

impl Vm {
    pub fn set_injector(&mut self, f: Injector) {
        self.injector = Some(f);
    }

    /// Create an instance of `class`, resolving its constructor's class and
    /// interface parameters recursively.
    pub fn resolve(&mut self, class: ClassId) -> Result<ObjHandle, Throw> {
        let mut stack = Vec::new();
        self.resolve_inner(class, &mut stack)
    }

    /// The class to instantiate for `class`: itself, or the one concrete
    /// implementation of an interface / abstract class (GRD022 otherwise).
    pub fn concrete_class(&self, class: ClassId) -> Result<ClassId, Throw> {
        let c = self.class(class);
        if c.kind == ClassKind::Class && !c.is_abstract {
            return Ok(class);
        }
        let impls: Vec<ClassId> = self.classes.iter().filter(|k| k.kind == ClassKind::Class && !k.is_abstract && k.id != class && self.instance_of(k.id, class)).map(|k| k.id).collect();
        let name = String::from_utf8_lossy(&c.name).into_owned();
        match impls.as_slice() {
            [one] => Ok(*one),
            [] => Err(Throw::error(format!("GRD022: {name} has no concrete implementation to inject"))),
            many => {
                let names: Vec<String> = many.iter().map(|k| String::from_utf8_lossy(&self.class(*k).name).into_owned()).collect();
                Err(Throw::error(format!("GRD022: {name} has several implementations ({}); inject the class you mean", names.join(", "))))
            }
        }
    }

    fn resolve_inner(&mut self, class: ClassId, stack: &mut Vec<ClassId>) -> Result<ObjHandle, Throw> {
        let class = self.concrete_class(class)?;
        if stack.contains(&class) {
            let chain: Vec<String> = stack.iter().chain(std::iter::once(&class)).map(|k| String::from_utf8_lossy(&self.class(*k).name).into_owned()).collect();
            return Err(Throw::error(format!("circular dependency: {}", chain.join(" -> "))));
        }
        stack.push(class);
        let mut named: Vec<(Vec<u8>, Val)> = Vec::new();
        if let Some(m) = self.find_method_info(class, b"__construct")
            && let Some(unit_id) = m.unit
        {
            let module = self.modules[m.module as usize].clone();
            let unit = &module.units[unit_id as usize];
            for p in &unit.params {
                if p.variadic {
                    break;
                }
                let pname = module.str(p.name).to_vec();
                let class_name = match &p.ty {
                    Some(TypeHint::Class(s)) => Some(module.str(*s).to_vec()),
                    Some(TypeHint::Nullable(inner)) => match inner.as_ref() {
                        TypeHint::Class(s) => Some(module.str(*s).to_vec()),
                        _ => None,
                    },
                    _ => None,
                };
                let resolved = match class_name.and_then(|n| self.lookup_class(&n)) {
                    Some(dep) => match self.injector.map(|f| f(self, dep)).transpose()?.flatten() {
                        Some(v) => Ok(v),
                        None => self.resolve_inner(dep, stack).map(Val::Object),
                    },
                    None => Err(Throw::type_error(format!(
                        "cannot inject ${} of {}::__construct(): only class and interface parameters are injected; give it a default",
                        String::from_utf8_lossy(&pname),
                        String::from_utf8_lossy(&self.class(class).name)
                    ))),
                };
                match resolved {
                    Ok(v) => named.push((pname, v)),
                    Err(_) if p.has_default => {}
                    Err(e) => {
                        stack.pop();
                        return Err(e);
                    }
                }
            }
        }
        stack.pop();
        let depth = self.frames.len();
        match self.invoke_new(class, Vec::new(), named)? {
            super::call::CallOutcome::Value(Val::Object(h)) => Ok(h),
            super::call::CallOutcome::Pushed => match self.run(depth)? {
                Val::Object(h) => Ok(h),
                _ => Err(Throw::error("constructor did not return the object")),
            },
            _ => Err(Throw::error("constructor did not return the object")),
        }
    }
}
