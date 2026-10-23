//! The virtual machine: one `Vm` per worker/request (phase 3: per run).
//!
//! Frames live on an explicit stack (`frames`), operands on one shared
//! stack (`stack`) with a base per frame. User code never recurses in Rust;
//! only native callbacks (`call_value`) and magic methods run a nested
//! dispatch loop, bounded by the nesting of such constructs.

pub mod class;
mod call;
mod exception;
mod generator;
mod iter;
pub(crate) mod ops;
mod path;
mod run;

use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::*;
use crate::error::Throw;
use crate::value::*;

pub use class::{MethodInfo, PropInfo, RtClass};
pub use generator::GeneratorState;
pub use iter::IterState;

pub type NativeFn = fn(&mut Vm, &[Val]) -> Result<Val, Throw>;

#[derive(Clone, Debug)]
pub enum FuncRef {
    User { module: u32, unit: UnitId },
    Native(u32),
}

#[derive(Debug)]
pub(crate) enum Pending {
    None,
    Return(Val),
    Throw(ObjHandle),
}

pub struct Frame {
    pub module: Rc<Module>,
    pub module_idx: u32,
    pub unit: UnitId,
    pub ip: usize,
    pub slots: Vec<Option<Val>>,
    pub this: Option<ObjHandle>,
    /// Scope class (`self::`, visibility).
    pub class: Option<ClassId>,
    /// Late static binding (`static::`).
    pub static_class: Option<ClassId>,
    pub stack_base: usize,
    pub arg_present: Vec<bool>,
    pub(crate) pending: Pending,
    pub generator: Option<ObjHandle>,
    pub line: u32,
    /// Value pushed on return instead of the return value (`new`).
    pub return_override: Option<Val>,
}

/// Payload of `Closure` objects.
#[derive(Clone, Debug)]
pub enum ClosureData {
    User { module: u32, unit: UnitId, captures: Vec<Option<Val>>, this: Option<ObjHandle>, scope: Option<ClassId>, static_class: Option<ClassId> },
    BoundMethod { this: ObjHandle, name: Vec<u8> },
    StaticMethod { class: ClassId, name: Vec<u8> },
    Function(FuncRef),
}

pub struct WellKnown {
    pub closure: ClassId,
    pub generator: ClassId,
    pub stdclass: ClassId,
    pub throwable: ClassId,
    pub array_access: ClassId,
    pub countable: ClassId,
    pub iterator: ClassId,
    pub iterator_aggregate: ClassId,
    pub traversable: ClassId,
    pub stringable: ClassId,
    pub json_serializable: ClassId,
    pub unit_enum: ClassId,
    pub backed_enum: ClassId,
}

pub struct Vm {
    pub modules: Vec<Rc<Module>>,
    pub classes: Vec<RtClass>,
    pub class_by_name: HashMap<Vec<u8>, ClassId>,
    pub functions: HashMap<Vec<u8>, FuncRef>,
    pub natives: Vec<(String, NativeFn)>,
    pub constants: HashMap<Vec<u8>, Val>,
    pub objects: ObjectStore,
    pub(crate) frames: Vec<Frame>,
    pub(crate) stack: Vec<Val>,
    pub output: Vec<u8>,
    pub(crate) iters: Vec<Option<IterState>>,
    pub(crate) enum_cases: HashMap<(ClassId, Vec<u8>), ObjHandle>,
    pub(crate) static_values: Vec<HashMap<Vec<u8>, Val>>,
    pub well_known: Option<WellKnown>,
    pub json_error: Option<(i64, String)>,
    pub regex_cache: HashMap<Vec<u8>, Rc<fancy_regex::Regex>>,
    pub default_timezone: String,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        let mut vm = Vm {
            modules: Vec::new(),
            classes: Vec::new(),
            class_by_name: HashMap::new(),
            functions: HashMap::new(),
            natives: Vec::new(),
            constants: HashMap::new(),
            objects: ObjectStore::new(),
            frames: Vec::new(),
            stack: Vec::new(),
            output: Vec::new(),
            iters: Vec::new(),
            enum_cases: HashMap::new(),
            static_values: Vec::new(),
            well_known: None,
            json_error: None,
            regex_cache: HashMap::new(),
            default_timezone: "UTC".to_string(),
        };
        crate::builtins::register(&mut vm);
        vm
    }

    /// A VM with the prelude (exception hierarchy, SPL interfaces, Closure,
    /// Generator) loaded.
    pub fn with_prelude() -> Result<Self, Throw> {
        let mut vm = Vm::new();
        let module = crate::compile::compile_source("{prelude}", PRELUDE.as_bytes())
            .map_err(|e| Throw::error(format!("prelude: {e}")))?;
        vm.load_module(module)?;
        vm.run_main((vm.modules.len() - 1) as u32)?;
        vm.resolve_well_known();
        Ok(vm)
    }

    pub fn register_native(&mut self, name: &str, f: NativeFn) {
        let idx = self.natives.len() as u32;
        self.natives.push((name.to_string(), f));
        self.functions.insert(name.to_ascii_lowercase().into_bytes(), FuncRef::Native(idx));
    }

    pub fn define_constant(&mut self, name: &str, v: Val) {
        self.constants.insert(name.as_bytes().to_vec(), v);
    }

    fn resolve_well_known(&mut self) {
        let id = |vm: &Vm, n: &str| vm.class_by_name[&n.to_ascii_lowercase().into_bytes()];
        self.well_known = Some(WellKnown {
            closure: id(self, "Closure"),
            generator: id(self, "Generator"),
            stdclass: id(self, "stdClass"),
            throwable: id(self, "Throwable"),
            array_access: id(self, "ArrayAccess"),
            countable: id(self, "Countable"),
            iterator: id(self, "Iterator"),
            iterator_aggregate: id(self, "IteratorAggregate"),
            traversable: id(self, "Traversable"),
            stringable: id(self, "Stringable"),
            json_serializable: id(self, "JsonSerializable"),
            unit_enum: id(self, "UnitEnum"),
            backed_enum: id(self, "BackedEnum"),
        });
    }

    pub(crate) fn wk(&self) -> &WellKnown {
        self.well_known.as_ref().expect("prelude not loaded")
    }

    /// Compile, link and run a `.grd` file. Returns the main unit's return value.
    pub fn run_file(&mut self, name: &str, source: &[u8]) -> Result<Val, RunError> {
        let module = crate::compile::compile_source(name, source).map_err(RunError::Compile)?;
        self.load_module(module).map_err(|t| self.describe_throw(t))?;
        let idx = (self.modules.len() - 1) as u32;
        self.run_main(idx).map_err(|t| self.describe_throw(t))
    }

    pub fn lookup_class(&self, name: &[u8]) -> Option<ClassId> {
        let n = name.strip_prefix(b"\\").unwrap_or(name);
        self.class_by_name.get(&n.to_ascii_lowercase()).copied()
    }

    pub fn class(&self, id: ClassId) -> &RtClass {
        &self.classes[id.0 as usize]
    }

    pub fn class_mut(&mut self, id: ClassId) -> &mut RtClass {
        &mut self.classes[id.0 as usize]
    }

    /// True if `class` is `target` or extends/implements it.
    pub fn instance_of(&self, class: ClassId, target: ClassId) -> bool {
        if class == target {
            return true;
        }
        let c = self.class(class);
        if c.interfaces.contains(&target) {
            return true;
        }
        let mut cur = c.parent;
        while let Some(p) = cur {
            if p == target {
                return true;
            }
            cur = self.class(p).parent;
        }
        false
    }

    pub fn is_throwable(&self, class: ClassId) -> bool {
        self.well_known.as_ref().is_some_and(|w| self.instance_of(class, w.throwable))
    }

    pub(crate) fn str_of(&self, module: u32, id: StrId) -> &[u8] {
        self.modules[module as usize].str(id)
    }

    pub(crate) fn frame(&self) -> &Frame {
        self.frames.last().expect("no frame")
    }

    pub(crate) fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("no frame")
    }

    pub(crate) fn current_line(&self) -> u32 {
        self.frames.last().map(|f| f.line).unwrap_or(0)
    }

    pub(crate) fn current_file(&self) -> String {
        self.frames.last().map(|f| f.module.file.clone()).unwrap_or_default()
    }

    pub(crate) fn push(&mut self, v: Val) {
        self.stack.push(v);
    }

    pub(crate) fn pop(&mut self) -> Val {
        self.stack.pop().expect("operand stack underflow")
    }

    pub(crate) fn pop_n(&mut self, n: usize) -> Vec<Val> {
        let at = self.stack.len() - n;
        self.stack.split_off(at)
    }

    /// Describe an uncaught exception for the CLI.
    pub fn describe_throw(&mut self, t: Throw) -> RunError {
        match t.object {
            Some(h) => {
                let class = self.class(self.objects.get(h).class).name.clone();
                let message = self.exception_message(h);
                RunError::Uncaught { class: String::from_utf8_lossy(&class).into_owned(), message, line: self.exception_line(h) }
            }
            None => RunError::Uncaught { class: t.class, message: t.message, line: self.current_line() },
        }
    }
}

#[derive(Debug)]
pub enum RunError {
    Compile(crate::compile::CompileError),
    Uncaught { class: String, message: String, line: u32 },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Compile(e) => write!(f, "{e}"),
            RunError::Uncaught { class, message, line } => {
                write!(f, "PHP Fatal error:  Uncaught {class}: {message} on line {line}")
            }
        }
    }
}

pub const PRELUDE: &str = include_str!("prelude.grd");
