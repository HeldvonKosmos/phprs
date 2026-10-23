//! Link step (RUNTIME_ARCHITECTURE §6): names in ops become VM-wide ids.
//! Method and property names become selectors (`SelId`), statically named
//! classes become `ClassRef::Id`, known functions become `FunctionId`.
//! Dynamic forms keep their name-based ops.

use super::Vm;
use crate::bytecode::*;

impl Vm {
    /// Selector for a method name (case-insensitive).
    pub fn method_sel(&mut self, name: &[u8]) -> SelId {
        let key = name.to_ascii_lowercase();
        if let Some(s) = self.method_sels.get(&key) {
            return *s;
        }
        let id = SelId(self.sel_names.len() as u32);
        self.sel_names.push(name.to_vec());
        self.method_sels.insert(key, id);
        id
    }

    /// Selector for a property name (case-sensitive).
    pub fn prop_sel(&mut self, name: &[u8]) -> SelId {
        if let Some(s) = self.prop_sels.get(name) {
            return *s;
        }
        let id = SelId(self.sel_names.len() as u32);
        self.sel_names.push(name.to_vec());
        self.prop_sels.insert(name.to_vec(), id);
        id
    }

    pub(crate) fn sel_name(&self, sel: SelId) -> &[u8] {
        &self.sel_names[sel.0 as usize]
    }

    fn link_class_ref(&self, names: &[Vec<u8>], cr: &mut ClassRef) {
        if let ClassRef::Named(id) = cr
            && let Some(c) = self.lookup_class(&names[*id as usize])
        {
            *cr = ClassRef::Id(c);
        }
    }

    /// Rewrite the ops of every unit of `module` to linked forms.
    pub(crate) fn link_module(&mut self, module: &mut Module) {
        let names = module.strings.clone();
        for unit in &mut module.units {
            for op in &mut unit.ops {
                match op {
                    Op::CallMethod { name, spec } => {
                        let sel = self.method_sel(&names[*name as usize]);
                        *op = Op::CallMethodSel { sel, spec: *spec };
                    }
                    Op::FetchProp(name) => {
                        let sel = self.prop_sel(&names[*name as usize]);
                        *op = Op::FetchPropSel(sel);
                    }
                    Op::FetchPropQuiet(name) => {
                        let sel = self.prop_sel(&names[*name as usize]);
                        *op = Op::FetchPropSelQuiet(sel);
                    }
                    Op::StorePath(p) | Op::UnsetPath(p) => {
                        if let PathBase::StaticProp(cr, _) = &mut p.base {
                            self.link_class_ref(&names, cr);
                        }
                        for seg in &mut p.segs {
                            if let PathSeg::Prop(name) = seg {
                                let sel = self.prop_sel(&names[*name as usize]);
                                *seg = PathSeg::PropSel(sel);
                            }
                        }
                    }
                    Op::CallFunc { name, fallback, spec } => {
                        let primary = self.function_ids.get(&names[*name as usize].to_ascii_lowercase()).copied();
                        let fid = primary.or_else(|| fallback.and_then(|f| self.function_ids.get(&names[f as usize].to_ascii_lowercase()).copied()));
                        if let Some(id) = fid {
                            *op = Op::CallFuncId { id, spec: *spec };
                        }
                    }
                    Op::CallStatic { class, name, spec } => {
                        let mut c = class.clone();
                        self.link_class_ref(&names, &mut c);
                        let sel = self.method_sel(&names[*name as usize]);
                        *op = Op::CallStaticSel { class: c, sel, spec: *spec };
                    }
                    Op::New { class, .. }
                    | Op::InstanceOf(class)
                    | Op::FetchClassName(class)
                    | Op::FetchStaticProp(class, _)
                    | Op::FetchClassConst(class, _)
                    | Op::CallableFromStatic(class, _) => {
                        self.link_class_ref(&names, class);
                    }
                    _ => {}
                }
            }
        }
    }
}
