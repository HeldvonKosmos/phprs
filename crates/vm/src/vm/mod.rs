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
mod link;
pub mod native;
mod inherit;
pub mod project;
pub(crate) mod ops;
mod path;
mod run;
pub mod jit;
pub mod gc;
pub mod inject;
#[cfg(feature = "opstats")]
pub mod opstats;

use std::collections::HashMap;
use std::rc::Rc;

use crate::bytecode::*;
use crate::error::Throw;
use crate::value::*;

pub use call::CallOutcome;
pub use native::{ArithFn, Call, Callable, CloneFn, CompareFn, FromVal, IntoVal, NativeClassDef, NativeMethodDef, ObjectHandlers};
pub use class::{MethodInfo, PropInfo, RtClass};
pub use generator::GeneratorState;
pub use iter::IterState;

pub type NativeFn = fn(&mut Vm, &[Val]) -> Result<Val, Throw>;

/// Per-unit profile (RUNTIME_ARCHITECTURE §7.1): counters and type feedback
/// for the later JIT tier. Worker-local, no atomics.
#[derive(Clone, Debug, Default)]
pub struct CodeProfile {
    pub calls: u32,
    pub backedges: u32,
    pub hot: bool,
    /// Per op: element kinds seen at dim fetches (`jit::KIND_*`, JIT2b);
    /// other ops leave it 0.
    pub types: Vec<u8>,
    /// Per op: loop region at this header could not be compiled (JIT).
    pub jit_no_loop: Vec<bool>,
}

pub const HOT_CALLS: u32 = 1_000;
pub const HOT_BACKEDGES: u32 = 10_000;

/// Hash map for the VM's own tables (names, selectors, classes): foldhash
/// instead of SipHash (HASH1). Keys come from code, not from requests.
pub type FastMap<K, V> = HashMap<K, V, foldhash::fast::RandomState>;

/// `IcEntry::b` of a method site whose target is a native handler; `a` is
/// then the index into `Vm::method_table` (CALL1).
pub(crate) const IC_NATIVE: u32 = u32::MAX;
/// `IcEntry::b` of a `new` site whose class has no constructor (FRAME1).
pub(crate) const IC_NO_CTOR: u32 = u32::MAX - 1;

/// Monomorphic inline cache entry (RUNTIME_ARCHITECTURE §7.1).
#[derive(Clone, Copy, Debug, Default)]
pub struct IcEntry {
    pub class: Option<ClassId>,
    /// Method sites: (module, unit, declaring class, is_static); property sites: slot index in `a`.
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub flag: bool,
}

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
    Jump(u32),
}

pub struct Frame {
    pub module: Rc<Module>,
    pub module_idx: u32,
    pub unit: UnitId,
    pub ip: usize,
    /// Window `[slot_base, slot_base + num_slots)` in `Vm::slot_stack`.
    pub slot_base: usize,
    pub num_slots: u32,
    pub this: Option<ObjHandle>,
    /// Scope class (`self::`, visibility).
    pub class: Option<ClassId>,
    /// Late static binding (`static::`).
    pub static_class: Option<ClassId>,
    pub stack_base: usize,
    /// Bit i set = parameter i was passed (parameters beyond 64 count as passed).
    pub arg_present: u64,
    pub(crate) pending: Pending,
    pub generator: Option<ObjHandle>,
    /// Index of this unit's inline-cache table in `Vm::ics`.
    pub ic: usize,
    /// Value pushed on return instead of the return value (`new`).
    pub return_override: Option<Val>,
}

impl Frame {
    /// Source line of the op this frame executes (or called from).
    pub fn line(&self) -> u32 {
        self.module.units[self.unit as usize].line_at(self.ip.saturating_sub(1))
    }
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
    /// Objects the host supplies for constructor injection (I1).
    pub(crate) injector: Option<inject::Injector>,
    pub class_by_name: FastMap<Vec<u8>, ClassId>,
    pub functions: FastMap<Vec<u8>, FuncRef>,
    /// Link-step ids: function name → index into `function_table`.
    pub function_ids: FastMap<Vec<u8>, FunctionId>,
    pub function_table: Vec<FuncRef>,
    /// Selectors for method (case-insensitive) and property (case-sensitive) names.
    pub method_sels: FastMap<Vec<u8>, SelId>,
    pub prop_sels: FastMap<Vec<u8>, SelId>,
    pub sel_names: Vec<Vec<u8>>,
    /// All methods of all classes; classes map selectors to indices here.
    pub method_table: Vec<MethodInfo>,
    pub natives: Vec<(String, NativeFn)>,
    pub constants: FastMap<Vec<u8>, Val>,
    pub objects: ObjectStore,
    /// MEM1a: most live request objects before `new` throws (0 = no limit).
    pub object_limit: usize,
    /// MEM1b (gc.rs): first collection after this many live objects (0 = off),
    /// the next one at `gc_next`.
    pub gc_threshold: usize,
    pub(crate) gc_next: usize,
    /// Active `run`s that are not GC-safe (native callbacks); collect only at 0.
    pub(crate) unsafe_runs: u32,
    /// Set by `call_*_gc_safe`: the next `run` is GC-safe.
    pub(crate) gc_safe_next: bool,
    /// Extra roots the host or a native can push while it holds values.
    pub gc_roots: Vec<Val>,
    /// Reports the values the host context holds (the framework's request state).
    pub host_tracer: Option<gc::TraceFn>,
    pub gc_stats: gc::GcStats,
    /// Class named by a type hint, per (module, name): `check_type` compares
    /// class ids instead of names (None: no such class).
    pub(crate) hint_classes: FastMap<(usize, StrId), Option<ClassId>>,
    pub(crate) frames: Vec<Frame>,
    /// Local variable slots of all live frames (RUNTIME_ARCHITECTURE §5.1).
    pub(crate) slot_stack: Vec<Option<Val>>,
    pub(crate) stack: Vec<Val>,
    /// Reused argument buffer for native calls (PERF1); taken while a native
    /// runs, so a nested call simply allocates its own.
    pub(crate) arg_scratch: Vec<Val>,
    /// Native ids of `intdiv`, `abs`, `max`, `min`: integer calls run inline in the
    /// interpreter (PERF2), everything else through the native function.
    pub(crate) fast_natives: [u32; 4],
    /// Class constants being evaluated (detects self-referencing constants).
    pub(crate) consts_in_progress: Vec<(ClassId, Vec<u8>)>,
    /// `run` calls currently nested (calls from Rust back into the VM).
    pub(crate) nested_runs: u32,
    /// Running magic property methods: (object, property, kind) — PHP's guards.
    pub(crate) magic_guards: Vec<(ObjHandle, Vec<u8>, u8)>,
    /// Objects json_encode is encoding right now (recursion detection).
    pub(crate) json_visiting: Vec<ObjHandle>,
    /// Objects var_dump/print_r/var_export are printing right now (cycles).
    pub(crate) dump_visiting: Vec<ObjHandle>,
    /// Built string and array constants per module (`const_to_val`).
    pub(crate) const_cache: Vec<Vec<Option<Val>>>,
    pub output: Vec<u8>,
    pub(crate) iters: Vec<Option<IterState>>,
    /// Iterator slots taken out while their iterator advances (see `alloc_iter`).
    pub(crate) iters_taken: Vec<usize>,
    /// Inline caches: one `Vec<IcEntry>` per unit (index = `ic_base[module] + unit`), indexed by op.
    pub(crate) ics: Vec<Vec<IcEntry>>,
    pub(crate) ic_base: Vec<usize>,
    /// Profiles, parallel to `ics` (same index).
    pub profiles: Vec<CodeProfile>,
    /// JIT backend (crate grd-jit), if installed; see `jit.rs`.
    pub(crate) jit: Option<Box<dyn jit::JitBackend>>,
    /// Whole-function JIT state per code unit (indexed like `profiles`).
    pub(crate) jit_funcs: Vec<jit::JitSlotState>,
    /// Loop-region JIT state per (unit index, loop header ip).
    pub(crate) jit_loops: FastMap<(usize, usize), jit::JitSlotState>,
    /// Reused state buffer for JIT calls.
    pub(crate) jit_buf: Vec<u64>,
    /// Frame depth of the last call whose native recursion was too deep;
    /// compiled code is not entered from deeper frames (`usize::MAX`: none).
    pub(crate) jit_deep_mark: usize,
    /// JITO1: class of `$this` for the next compile request (methods).
    pub(crate) jit_this: Option<u32>,
    /// JITO1: class table for the JIT and the class count it was built for.
    pub(crate) jit_class_table: Option<(usize, std::sync::Arc<jit::JitClasses>)>,
    /// (calls, back-edges) before compiling.
    pub(crate) jit_hot: (u32, u32),
    pub(crate) enum_cases: FastMap<(ClassId, Vec<u8>), ObjHandle>,
    pub(crate) static_values: Vec<FastMap<Vec<u8>, Val>>,
    pub well_known: Option<WellKnown>,
    pub json_error: Option<(i64, String)>,
    /// `strtok` state: the string and the position to continue from.
    pub strtok: Option<(Vec<u8>, usize)>,
    pub regex_cache: FastMap<Vec<u8>, Rc<fancy_regex::Regex>>,
    pub default_timezone: String,
    /// Opaque host context (framework request context).
    pub host: Option<Box<dyn std::any::Any>>,
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
            injector: None,
            class_by_name: Default::default(),
            functions: Default::default(),
            function_ids: Default::default(),
            function_table: Vec::new(),
            method_sels: Default::default(),
            prop_sels: Default::default(),
            sel_names: Vec::new(),
            method_table: Vec::new(),
            natives: Vec::new(),
            constants: Default::default(),
            objects: ObjectStore::new(),
            object_limit: default_object_limit(),
            gc_threshold: gc::default_gc_threshold(),
            gc_next: gc::default_gc_threshold(),
            unsafe_runs: 0,
            gc_safe_next: false,
            gc_roots: Vec::new(),
            host_tracer: None,
            gc_stats: gc::GcStats::default(),
            hint_classes: Default::default(),
            frames: Vec::new(),
            slot_stack: Vec::new(),
            stack: Vec::new(),
            arg_scratch: Vec::new(),
            fast_natives: [u32::MAX; 4],
            consts_in_progress: Vec::new(),
            nested_runs: 0,
            magic_guards: Vec::new(),
            json_visiting: Vec::new(),
            dump_visiting: Vec::new(),
            const_cache: Vec::new(),
            output: Vec::new(),
            iters: Vec::new(),
            iters_taken: Vec::new(),
            ics: Vec::new(),
            ic_base: Vec::new(),
            profiles: Vec::new(),
            jit: None,
            jit_funcs: Vec::new(),
            jit_loops: Default::default(),
            jit_buf: Vec::new(),
            jit_deep_mark: usize::MAX,
            jit_this: None,
            jit_class_table: None,
            jit_hot: (jit::JIT_HOT_CALLS, jit::JIT_HOT_BACKEDGES),
            enum_cases: Default::default(),
            static_values: Vec::new(),
            well_known: None,
            json_error: None,
            strtok: None,
            regex_cache: Default::default(),
            default_timezone: "UTC".to_string(),
            host: None,
        };
        crate::builtins::register(&mut vm);
        for (i, name) in ["intdiv", "abs", "max", "min"].iter().enumerate() {
            if let Some(FuncRef::Native(id)) = vm.functions.get(name.as_bytes()) {
                vm.fast_natives[i] = *id;
            }
        }
        vm
    }

    /// A VM with the prelude (exception hierarchy, SPL interfaces, Closure,
    /// Generator) loaded.
    pub fn with_prelude() -> Result<Self, Throw> {
        // the preludes are compiled once per process; each VM loads a copy
        static COMPILED: std::sync::OnceLock<Result<(Module, Module), String>> = std::sync::OnceLock::new();
        let (prelude, support) = COMPILED
            .get_or_init(|| {
                let p = crate::compile::compile_source("{prelude}", PRELUDE.as_bytes()).map_err(|e| format!("prelude: {e}"))?;
                // OpenGrid\Support: Str, Arr, Collection (S1)
                let s = crate::compile::compile_source("{prelude/support}", PRELUDE_SUPPORT.as_bytes()).map_err(|e| format!("prelude support: {e}"))?;
                Ok((p, s))
            })
            .clone()
            .map_err(Throw::error)?;
        let mut vm = Vm::new();
        vm.load_module(prelude)?;
        vm.run_main((vm.modules.len() - 1) as u32)?;
        vm.load_module(support)?;
        vm.run_main((vm.modules.len() - 1) as u32)?;
        // native classes that implement prelude interfaces
        crate::builtins::datetime_class::register(&mut vm)?;
        vm.resolve_well_known();
        Ok(vm)
    }

    pub fn register_native(&mut self, name: &str, f: NativeFn) {
        let idx = self.natives.len() as u32;
        self.natives.push((name.to_string(), f));
        self.define_function(name.as_bytes(), FuncRef::Native(idx));
    }

    /// Register a function under its (lowercased) name and give it an id.
    pub(crate) fn define_function(&mut self, name: &[u8], f: FuncRef) {
        let key = name.to_ascii_lowercase();
        self.functions.insert(key.clone(), f.clone());
        match self.function_ids.get(&key) {
            Some(id) => self.function_table[id.0 as usize] = f,
            None => {
                let id = FunctionId(self.function_table.len() as u32);
                self.function_table.push(f);
                self.function_ids.insert(key, id);
            }
        }
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

    /// End of a request (RUNTIME_ARCHITECTURE §1): request objects, output
    /// and stacks go; code, classes, profiles, inline caches, static
    /// properties and enum singletons (engine objects) stay. The host
    /// context stays too; the framework clears its per-request part itself.
    pub fn reset_request(&mut self) {
        self.objects.reset_request();
        self.frames.clear();
        self.slot_stack.clear();
        self.stack.clear();
        self.iters.clear();
        self.output.clear();
        self.json_error = None;
        self.strtok = None;
        self.gc_roots.clear();
        self.gc_next = self.gc_threshold;
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

    /// Scope class for visibility checks; without a running frame (calls from
    /// Rust between requests) the scope is global: only public members.
    pub(crate) fn scope_class(&self) -> Option<ClassId> {
        self.frames.last().and_then(|f| f.class)
    }

    #[allow(dead_code)]
    pub(crate) fn frame_mut(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("no frame")
    }

    /// Line of the op the current frame is executing (its `ip` already points
    /// past it while the op runs).
    pub(crate) fn current_line(&self) -> u32 {
        self.frames.last().map(Frame::line).unwrap_or(0)
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
pub const PRELUDE_SUPPORT: &str = include_str!("prelude_support.grd");

/// Default for `Vm::object_limit`: `GRD_OBJECT_LIMIT` or one million.
fn default_object_limit() -> usize {
    std::env::var("GRD_OBJECT_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(1_000_000)
}

impl Vm {
    /// MEM1a: objects live until the end of the request; a runaway loop hits
    /// this limit and gets an `Error` instead of taking the process down.
    pub(crate) fn check_object_limit(&self) -> Result<(), Throw> {
        if self.object_limit != 0 && self.objects.len() >= self.object_limit {
            return Err(Throw::error(format!(
                "Allowed object count of {} per request exhausted (objects live until the end of the request; limit: opengrid.limits.objects or GRD_OBJECT_LIMIT)",
                self.object_limit
            )));
        }
        Ok(())
    }
}

