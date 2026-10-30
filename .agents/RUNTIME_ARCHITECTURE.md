# Runtime architecture of the OpenGrid core

Binding. As of 2026-10-23, version 2 (complete). This document fixes
the data model and the execution model of the new VM (Option B from
`FEASIBILITY.md`). It is the basis for `PROMPT_OPTION_B.md`,
`NATIVE_CLASSES.md` and `DATABASE_LAYER.md`. Where other documents deviate,
this one prevails.

Goals from which every decision follows:

1. Java semantics for objects (references, identity), PHP semantics for
   arrays and strings (values, copy-on-write).
2. HHVM behaviour for paths: a worker gets warmer with every request.
3. Rust base classes from which dialect classes inherit, and Rust code that
   looks into dialect subclasses and calls their methods.
4. Development interprets from source files with hot reload; production
   embeds the finished opcodes in the binary.
5. Two tiers: interpreter first, Cranelift JIT later on the same opcodes,
   without changing this model.
6. No `unsafe` in the core. No raw pointers in values. `Val` is `Send`.

---

## 1. Two lifetimes

Everything in the core belongs to exactly one of two lifetimes.

```
Engine (one worker thread, lives for days)
├── code:       CodeTable   — Arc<OpArray> per function/method/file, shared with other workers
├── classes:    ClassTable  — ClassId → ClassDef (dialect and native classes), method/property layouts
├── functions:  FunctionTable — FunctionId → Arc<OpArray> | NativeFn
├── natives:    Registry    — native functions, classes, handlers
├── profiles:   HashMap<CodeId, CodeProfile> — counters, type feedback, inline caches (worker-local)
├── jit:        later: compiled entry points per CodeId
└── config:     snapshot of the application configuration

ExecuteData (one request / one job / one task, lives milliseconds to seconds)
├── objects:    ObjectStore — all objects of this execution
├── frames:     Vec<Frame>  — the VM's call stack (no Rust recursion)
├── temps/slots per frame, exception state, generator states
├── host:       Box<dyn Any> — the framework's request context (Request, Session, Auth), opaque to the core
└── io:         output buffer, tracing span
```

Rules:

- **Code and profile survive the request, state does not.** That is why
  paths get warm, and why there are no memory leaks across requests.
- A `Val` is only valid within its `ExecuteData`. Whatever leaves the
  execution (response, cache, queue, session) is converted into an external
  format (JSON, SurrealDB value, bytes). Native classes enforce this
  through their signatures.
- The `Engine` is never used by two threads at the same time. Only
  `Arc<OpArray>` (immutable) is shared. Profiles and caches are per engine.

---

## 2. Values

```rust
pub enum Val {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(PhpStr),          // Rc<[u8]>, UTF-8 validated when created from literals/JSON; byte-oriented like PHP
    Arr(PhpArr),          // Rc<ArrayData>, copy-on-write
    Obj(ObjHandle),       // u32 index into ExecuteData.objects
}
```

- 16 bytes, `Copy`-like cheap to clone (`Rc` increment for Str/Arr,
  copy for everything else). No `Box`, no raw pointer, no `unsafe`.
- `Val` is `Send` because `ObjHandle` is only an index and `Rc` never
  changes threads (conversion at the boundary creates new values).
- Strings are byte sequences. `strlen` counts bytes, `mb_*` counts characters.
  Source text is read as UTF-8 (no Latin-1 double encoding).
- Arrays: ordered hash map with int and string keys, PHP semantics for
  numeric strings. Writing to a shared array (`Rc::make_mut`) copies only
  then. `$a[] = $x` in a loop is amortised O(1).
- Comparisons per `DIALECT.md` §5.1: `==` is `===` for scalars and
  handles; `<`/`>` only between numbers or between strings, otherwise
  `TypeError`.

---

## 3. Object model: store and handles

### 3.1 Representation

```rust
pub struct ObjHandle(u32);                  // 0 = invalid

pub struct PhpObject {
    pub class: ClassId,
    pub slots: Vec<Val>,                    // declared properties according to the class layout
    pub dynamic: Option<Box<HashMap<PhpStr, Val>>>, // only stdClass and classes with __set fallback
    pub native: Option<Box<dyn Any>>,       // payload of native classes (Builder, RecordId, DataFrame, Request)
    pub flags: ObjFlags,                    // readonly status per slot, initialised, …
}

pub struct ObjectStore {
    slots: Vec<Option<PhpObject>>,          // index = handle
    free: Vec<u32>,                         // later for refcounting; unused for now
}
```

### 3.2 Semantics

| Operation | Behaviour |
|---|---|
| `$b = $a`, parameter, return, closure capture | copies the handle |
| `$a === $b` | handle comparison |
| `$a == $b` | also handle comparison (strict); value comparison via methods |
| `clone $a` | new slot with copied `slots`/`dynamic`; `native` is copied via the native class's `Clone` handler or refused; call `__clone` |
| `spl_object_id($a)` | the handle |
| `$arr[0]->v = 1`, `$this->items[] = $o`, `foreach ($list as $o) $o->touch()` | work because arrays hold handles |
| `unset($a)` | removes the variable; the object lives until the end of the execution |

### 3.3 Access from Rust

```rust
impl ExecuteData {
    pub fn obj(&self, h: ObjHandle) -> &PhpObject;            // invalid handle = interpreter bug (panic)
    pub fn obj_mut(&mut self, h: ObjHandle) -> &mut PhpObject;
    pub fn alloc(&mut self, class: ClassId) -> ObjHandle;      // slots with the class defaults
    pub fn native<T: 'static>(&self, h: ObjHandle) -> Option<&T>;
    pub fn native_mut<T: 'static>(&mut self, h: ObjHandle) -> Option<&mut T>;
}
```

No lock, no `RefCell`. The borrow ends before the VM is re-entered:
property handlers (`__get`, native get) are decided, the value cloned,
and only then a call is made. Hence there are no deadlocks.

### 3.4 Property access

Order for `$o->x`:

1. Inline cache of the op (`ClassId → slot index`), hit: `slots[i]`.
2. Class layout: declared property → slot; check visibility.
3. Native class with `get` handler → handler (lazy materialisation, §8.3).
4. `dynamic` map (only if allowed).
5. `__get` of the dialect class (chain upwards).
6. `UndefinedPropertyError`.

Writing analogously with a `readonly` check (initialisable once, afterwards
`Error`).

### 3.5 Memory

Within an execution nothing is freed (arena). At the end the store is
discarded, cycles included. For long CLI tasks and jobs, reference counting
in `Val` comes later (increment/decrement on copy/drop, `free.push`
at 0). The design allows this; **do not build it now**.

---

## 4. Class model

### 4.1 ClassDef

```rust
pub struct ClassDef {
    pub id: ClassId,
    pub name: PhpStr,                       // fully qualified
    pub kind: ClassKind,                    // Class | Abstract | Interface | Trait | Enum | Native
    pub parent: Option<ClassId>,
    pub interfaces: Vec<ClassId>,           // resolved transitively at link time
    pub layout: Vec<PropDef>,               // slot index, name, visibility, type, default, readonly, static?
    pub statics: Vec<Val>,                  // static properties (per engine, not per request!)
    pub constants: Vec<(PhpStr, Val)>,
    pub methods: Vec<MethodDef>,            // own methods
    pub vtable: Vec<MethodId>,              // resolved: all callable methods incl. inherited, index stable
    pub attributes: Vec<Attribute>,         // for reflection: name, arguments
    pub native: Option<NativeClassDef>,     // handler table, payload constructor, inheritable?
    pub flags: ClassFlags,                  // final, abstract, enum backing type, …
}
```

- **Class IDs** are assigned at link time; method lookup is
  `vtable[idx]`, not `HashMap<String>`. Names remain only for reflection
  and error messages.
- Interfaces are real classes in the table; `instanceof` checks the parent
  chain and the interface list. The built-in interfaces (`Countable`,
  `ArrayAccess`, `IteratorAggregate`, `Stringable`, `JsonSerializable`,
  `Throwable`) and the exception hierarchy are registered classes, not
  string special cases in the VM.
- Enums are classes with fixed instances (`cases()` returns the handles
  from an engine-wide case table that is recreated per request).

**Caution, static properties:** they live in the `Engine` (per worker) and
survive the request. This is atypical for PHP and fixed in `DIALECT.md` §5.4:
static properties are configuration, not request state.
The linker sets them to their defaults; writable at runtime, but without
any guarantee across workers.

### 4.2 Inheritance in both directions

**Dialect inherits from Rust** (`class Invoice extends Model`):

- The native base class is a `ClassDef` with `native: Some(..)`,
  `inheritable: true`. The subclass receives the same payload factory; `new
  Invoice` calls `alloc(Invoice)` and the payload factory of the nearest native
  base in the chain.
- Native methods appear in the subclass's `vtable` like inherited
  dialect methods; the subclass can override them.
- `parent::__construct()` from the subclass calls the native constructor.

**Rust looks into the dialect** (the base class reads `$table`, calls
`toArray()` overrides, scopes, hooks):

```rust
impl Engine {
    pub fn class_meta(&self, class: ClassId) -> &ClassDef;                  // layout, defaults, statics, attributes
    pub fn static_default(&self, class: ClassId, name: &str) -> Option<&Val>; // static::$table
    pub fn instance_default(&self, class: ClassId, name: &str) -> Option<&Val>; // $fillable, $dates
    pub fn find_method(&self, class: ClassId, name: &str) -> Option<MethodId>; // incl. inherited; None = not defined
    pub fn is_overridden(&self, class: ClassId, name: &str, base: ClassId) -> bool;
}
impl Vm<'_> {   // Engine + ExecuteData
    pub fn call_method(&mut self, this: ObjHandle, m: MethodId, args: &[Val]) -> Result<Val, Throw>;
    pub fn call_static(&mut self, class: ClassId, m: MethodId, args: &[Val]) -> Result<Val, Throw>;
    pub fn instantiate(&mut self, class: ClassId, args: &[Val]) -> Result<ObjHandle, Throw>;
    pub fn invoke(&mut self, callable: &Val, args: &[Val]) -> Result<Val, Throw>; // Closure, "fn", [$o,'m'], invokable
}
```

Native base classes cache per `ClassId` what they have read from the
subclass (`ModelMeta`) in an engine-wide table; on hot reload of the class
the entry is discarded.

---

## 5. Execution

### 5.1 Frames and slots

```rust
pub struct Frame {
    code: CodeId,                   // → Arc<OpArray> in the engine
    pc: u32,
    slots: Range<u32>,              // window in the shared slot stack of the ExecuteData: parameters, locals, temps
    this: Option<ObjHandle>,
    class_scope: Option<ClassId>,   // lexical class (self::, parent::, visibility)
    called_class: Option<ClassId>,  // static:: (late static binding)
    captures: Option<Rc<[Val]>>,    // closures: bound values, reachable via slot indices
    try_stack: SmallVec<TryEntry>,
    ret_slot: Option<u32>,          // where the result is written in the caller
}
```

- **No Rust recursion per dialect call.** `call` creates a frame
  and jumps; `return` removes it. Recursion depth is limited by configuration
  (default 100,000 frames), not by the Rust stack.
- Variables are slots, resolved at compile time. No symbol table,
  no hashing of names at runtime. `$$x`, `extract`, `compact`,
  `get_defined_vars` are therefore not expressible and forbidden by the dialect.
- Native calls (builtins, native methods) run as a Rust function
  within the current frame; if they call back into the VM
  (`vm.invoke(closure)`), the VM creates a new frame and executes until
  it returns. This is the only place with bounded Rust recursion
  (native ↔ dialect alternating), and it is counted and limited.

### 5.2 Calling convention

- Arguments are written directly into the callee's slot window
  (no intermediate `call_args` list). Named arguments are mapped to
  positions at link time when the callee is statically known, otherwise at
  runtime via `MethodDef.params`.
- Defaults are evaluated as prologue opcodes in the callee when a slot
  is marked "not passed" (its own sentinel, not `Null`; an
  explicit `null` stays `null`).
- Variadics pack surplus arguments into an array. Spread (`...$a`)
  unpacks before the call.
- There are no reference parameters (dialect).

### 5.3 Closures

```rust
struct ClosureData { code: CodeId, captures: Rc<[Val]>, this: Option<ObjHandle>, scope: Option<ClassId> }
```

`Closure` is a native class; the instance holds `ClosureData` as its
payload. `fn($x) => …` captures the free variables automatically (by value),
`function () use ($a)` explicitly. `bindTo`/`call` create new closures with
a different `this`/`scope`. First-class callables (`strlen(...)`, `$o->m(...)`)
are closures without captures.

### 5.4 Exceptions

- A thrown exception is a handle; `ExecuteData.pending: Option<ObjHandle>`.
- `throw` searches the `try_stack` of the current frame, then the frames
  below; frames without a handler are removed (with `finally` execution).
- VM errors (undefined index, type error, division by zero) are
  created as objects of the built-in hierarchy and thrown, never printed as
  warnings (`DIALECT.md` §5.3).
- A native call returns `Result<Val, Throw>`; `Throw` is either
  a finished exception object (handle) or a constructor recipe
  (class + message) that the VM materialises.
- Unhandled at the end of the execution: the execution ends with the handle;
  the framework maps it to an HTTP status/error page.

### 5.5 Generators

A generator is an object whose payload holds a **detached frame**
(slots copied, `pc` saved). `next()`/`send()` re-attach the frame,
execute until the next `yield` and detach it again. `yield
from` delegates through a chain. There are no fibers (dialect).

### 5.6 Opcode limit

None. A request is bounded by the framework via a timeout (Tower layer);
the VM checks an abort flag at back-edges and calls, which the worker
can set (cooperative interruption).

---

## 6. Code: `OpArray` as pure data

```rust
pub struct OpArray {
    pub id: CodeId,
    pub name: PhpStr,                 // for stack traces
    pub file: FileId, pub lines: Vec<u32>,  // op index → line
    pub ops: Vec<Op>,                 // fixed format: opcode u8, a/b/c u32 (slots, constant index, IDs)
    pub consts: Vec<Const>,           // Int, Float, Str, Null, Bool, Arr (only from constants)
    pub slots: u32,                   // window size
    pub params: Vec<ParamDef>,        // name, slot, default prologue address, type (for reflection/DI)
    pub returns: Option<TypeRef>,
    pub try_table: Vec<TryEntry>,     // ranges → handler addresses
    pub flags: CodeFlags,             // generator, static, closure, …
}
```

- **No pointers, no names in ops.** Classes, methods, functions,
  constants and properties are IDs, resolved in the link step.
- `OpArray` is `Serialize + Deserialize` (bincode) and immutable; it
  lives in an `Arc` and is shared between workers.
- Profile and inline caches are **not** in the `OpArray` (§7), so that it
  can remain immutable.
- The link step resolves names, checks inheritance (abstract methods
  implemented, interfaces satisfied, `final` respected), builds `vtable`s
  and layouts, binds named arguments, and reports every violation as a
  compile error. Dev: per reload for the affected file and its
  dependents; Prod: once, whole program.

Pipeline: `mago` parser → AST → dialect pass (GRD errors) → codegen per
file (names still symbolic) → link (IDs, vtables, layouts) → `OpArray`s.

---

## 7. Profile, inline caches, tiers

### 7.1 Side table per worker

```rust
pub struct CodeProfile {
    calls: u32,
    loop_backedges: Vec<u32>,         // index = op index of the back-edge
    hot: bool,
    ics: Vec<InlineCache>,            // one entry per call site / property op / static op (index from the op)
    types: Vec<TypeFeedback>,         // per arithmetic/comparison op: observed type combinations
}
pub enum InlineCache { Empty, Mono(ClassId, u32), Poly([(ClassId, u32); 4]), Mega }
```

- Method call: check the IC (`ClassId` of the receiver → `vtable` index),
  otherwise look up and fill the IC. Property access: `ClassId → slot index`.
  On the second hit both are an array access.
- No atomics, no locks: the profile belongs to the worker.
- Thresholds (configurable): `hot` from 1,000 calls or 10,000 back-edges.

### 7.2 Invalidation

- Hot reload of a file: new `OpArray`s, new `ClassId`s for the
  affected classes; profile and ICs of this file are discarded; ICs of
  other files automatically become misses through the `ClassId` comparison.
- Never per request.

### 7.3 Tier 2: Cranelift (M7)

The same opcodes are the input. For code marked `hot`, Cranelift
generates machine code specialised on the type feedback (int addition
with overflow guard, direct jumps at monomorphic call sites, slot accesses
as stack offsets). Every specialisation has a guard; if it fails,
deoptimisation back into the interpreter at the same `pc`. The interpreter
remains the fallback tier. This document does not change because of that; only
`Engine.jit` is added.

Prerequisites that therefore apply **now**: ops in a fixed format, IDs
instead of names, values with a fixed representation (§2), profile as a table (§7.1).

---

## 8. Native interface (short version; details in NATIVE_CLASSES.md)

### 8.1 Registration

```rust
pub struct NativeClassDef {
    name: &'static str, parent: Option<&'static str>, interfaces: &'static [&'static str],
    inheritable: bool,
    constructor: Option<NativeFn>,
    payload: Option<fn() -> Box<dyn Any>>,            // payload factory, also for subclasses
    methods: &'static [NativeMethodDef],              // name, static?, visibility, params (for reflection/DI), handler
    constants: &'static [(&'static str, Const)],
    handlers: ObjectHandlers,                         // get, set, isset, unset, call_missing, to_string, clone, count, iterate, array_access, json
}
pub type NativeFn = fn(&mut Vm, Call) -> Result<Val, Throw>;
pub struct Call { this: Option<ObjHandle>, class: ClassId, args: Vec<Val>, named: Vec<(PhpStr, Val)> }
```

Macros (`#[php_class]`, `#[php_method]`, `#[php_function]`) generate these
tables from Rust impl blocks, including argument conversion
(`FromVal`/`IntoVal`) and a stub file (`.grd` signatures) for
tooling.

### 8.2 Context

`Vm` is `Engine` + `ExecuteData`. Through it a handler reaches objects,
calls methods, creates exceptions, and obtains the opaque host context
(`vm.host::<RequestContext>()`) that the framework deposits per request.
There is no host ABI; the framework code downcasts directly.

### 8.3 Lazily materialised foreign values (example SurrealDB)

A record from SurrealDB is not copied into dialect arrays. It is
created as an object of the class `OpenGrid\Database\Record` (or the model subclass)
whose payload is the `surrealdb::sql::Object`. The `get` handler
reads the field from the payload and converts only this one field:

- scalars → `Val` inline
- `Thing` → `RecordId` object (payload: the `Thing`)
- `Datetime`, `Duration`, `Decimal`, `Uuid`, `Geometry`, `Bytes` → native type classes with payload
- `Object` → nested `Record` object (lazy)
- `Array` → dialect array (materialised, since arrays are values), elements lazy by the same rules

Writing (`$model->x = …`) stores the value in the payload and marks it
`dirty`. `toArray()` materialises everything. Bindings in the other direction:
`Val` → `sql::Value`, where the type classes deliver their payload directly
(no string detour, no `<record>` cast).

### 8.4 Callables from Rust

`vm.invoke(&closure, &[builder_handle])`: the closure runs in a new
frame, receives the native `Builder` object as a handle and mutates its
payload. After it returns, Rust continues reading the payload. Object identity makes
this trivial; without it (value copy) it would be impossible.

---

## 9. Workers and requests

```
Axum/Loco (Tokio)                     worker threads (N = cores)
  Request ──► channel ──► worker: ExecuteData::new(host_ctx)
                                     route table → controller class/method
                                     vm.instantiate(controller) ; vm.call_method(action, args)
                                     return value → response conversion
              Response ◄── channel ◄─ discard ExecuteData
```

- One `Engine` per worker thread, `OpArray`s shared via `Arc`.
- Thread affinity: a request stays on its worker. That is why
  `Rc` and worker-local profiles suffice.
- Native async (SurrealDB SDK, HTTP client) is executed with `Handle::block_on` on
  the worker; the worker exists to block. Tokio threads
  never block.
- Jobs (queue) and tasks (CLI) use the same worker path with a different
  host context.

---

## 10. Development and production

| | Dev (`opengrid serve`) | Prod (`opengrid build` → binary) |
|---|---|---|
| Source | `.grd` in the file system | `include_bytes!("opcodes.bin")` + manifest |
| Parser (mago) in the process | yes | no (feature off; CLI tools keep it) |
| Link | per reload: file + dependents | once, whole program |
| Errors | error page on the next request, with GRD code, file, line | build aborts |
| Profile/ICs | per worker; discarded per file on reload | per worker; never discarded; optional start profile from production |
| Class IDs | dynamic | fixed; vtables indexed directly |
| Whole program | no | devirtualisation when there is a single implementation, constant folding, dead classes removed |
| Module crates | `.so` via `libloading` (dev plugins) | statically linked |

Hot reload: a watcher (`notify`) reports file changes to all workers
over a channel; between two requests each worker swaps the
affected `Arc<OpArray>`s and `ClassDef`s, discards profile, ICs and
`ModelMeta` caches of these classes, and rebuilds the route table when
`routes/*.grd` is affected. Running requests finish on the old code.

`opengrid build` is a `build.rs` step of the app crate: read modules,
parse, dialect pass, codegen, link, serialise to `OUT_DIR`; the
binary embeds `opcodes.bin` and `manifest.bin` (routes, class list,
hashes). Start-up time: deserialise, spin up workers.

Later, the same path continued: compile hot functions to object files with
Cranelift at build time and link them in (AOT, with a profile from production).

---

## 11. Error classes and diagnostics

- Compile time: syntax (mago), dialect (`GRD0xx`), link (`GRD1xx`:
  unknown class, unimplemented abstract method, visibility,
  `final`). All with file, line, hint.
- Runtime: exceptions of the built-in hierarchy; each has file and line
  from `OpArray.lines` and a stack trace from the frames.
- Interpreter bugs (invalid handle, slot outside the window) are
  `panic!` with context, never silent wrong values. The worker catches the panic,
  responds with 500 and logs; the engine remains usable because the
  `ExecuteData` is discarded.
- Tracing: one span per request, spans for database queries and native
  calls over 1 ms.

---

## 12. Limits, set deliberately

- No references (`&$x`), no fibers, no `eval`, no variable
  variables, no superglobals, no `include`. See `DIALECT.md` §4.2.
- Static properties are worker state, not request state.
- Objects do not survive a request; caching objects is a type error
  at the native interface, not a runtime error.
- A worker processes one request at a time; concurrency comes from
  the number of workers, not from the VM.

---

## 13. Build order (aligned with PROMPT_OPTION_B.md)

1. `Val`, `PhpStr`, `PhpArr` (COW), `ObjectStore`, `ObjHandle`, comparisons, conversions — phase 1
2. mago AST → dialect pass → codegen (slots, symbolic names) → link (IDs, vtables, layouts) — phase 2
3. Frames, slot stack, calls, closures, exceptions, objects, property access with layouts, generators — phase 3
4. Builtins on `Val`, registry, built-in classes/interfaces/exceptions as `ClassDef`s — phase 4
5. Create the profile side table and actively use ICs for methods/properties — phase 5 (speed-up is measurable, no JIT needed)
6. Native classes: registration, inheritable base class, `class_meta`, handlers, `invoke`; acceptance with `Model`/`Builder` per `DATABASE_LAYER.md` — phase 6
7. `OpArray` serialisation, link for prod, worker pool skeleton, hand-over to the framework — phase 7

---

## 14. Decisions and rationale (short form for DECISIONS.md)

| Decision | Rejected alternative | Rationale |
|---|---|---|
| Objects as handles into a store per execution | `Arc<Mutex<PhpObject>>`, `Rc<RefCell>` | lifetime separation (§1), cycles without GC, no lock across VM re-entry, `Val: Send` without `unsafe` |
| Arrays as `Rc` with COW | handle store for arrays too | PHP value semantics for arrays is what Laravel code expects; COW makes it cheap |
| Slots instead of symbol tables | name hashing | O(1), JIT-capable, automatically forbids `$$x`/`extract` |
| Own frame stack | Rust recursion | recursion depth, generators, deoptimisation need explicit frames |
| IDs instead of names in code, link step | runtime lookups | ICs, serialisation, whole-program optimisation |
| Profile as a worker-local side table | atomics in the `OpArray` | no sharing, no lock, `OpArray` stays immutable |
| Static properties per engine | per request | otherwise every class would have to be copied per request; the dialect rule makes it explicit |
| mago as parser dependency | own parser, fork | maintenance and syntax coverage; checkpoints in PROMPT_OPTION_B phase 0 |
| No opcode limit, timeout from the framework | counter in the VM | request limits belong to the host |
| Native async via `block_on` in the worker | async VM | VM stays simple; workers exist to block |

---

## 15. Open

- Reference counting for long tasks (§3.5): when, and whether at all.
- Enum instances: create per request (simple) or as immutable
  engine objects with special handles (faster). Per request for now.
- Arrays with objects in static properties (engine lifetime!): handles
  would be invalid in the next request. The linker forbids objects in static
  defaults; at runtime, writing a handle into a static
  property becomes an `Error`.
- `__destruct`: without refcounting it cannot be called at the right time;
  the dialect drops `__destruct` (resources are released at request end or
  explicitly).
