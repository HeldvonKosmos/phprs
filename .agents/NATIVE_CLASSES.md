# Native classes: making Rust code usable in the dialect

As of 2026-10-23. Orientation: `extphprs/ext-php-rs` 0.16.3 (commit
d6fd86c, 2026-10-16); studied in the upstream repository: `crates/macros/src/{class,impl_,function,
module}.rs`, `src/class.rs`, `src/convert.rs`, `src/builders/class.rs`,
`guide/src/macros/*.md`, `guide/src/types/*.md`, `guide/src/exceptions.md`.

This document describes how framework logic is written in Rust and made
visible to `.grd` code through the interpreter. It complements `ROADMAP.md` §2 (the
seam) and M1 (native registry) and `DIALECT.md` §6 (list of native
base classes). Section 5 is the gap list in working order; the
interpreter work items come first.

## 1. How ext-php-rs makes Rust available in PHP

ext-php-rs builds PHP extensions in Rust against the Zend API. What matters for us is not
the FFI but the form: which annotations a Rust developer writes,
which conventions turn them into PHP signatures, and how the layers are
separated.

### 1.1 Three layers

1. **Proc macros** (`crates/macros`): read Rust syntax, generate metadata
   and handler functions. No Zend call in generated code.
2. **Builders** (`ClassBuilder`, `FunctionBuilder`, `ModuleBuilder`): accept
   metadata, register at module start (MINIT).
3. **FFI** (`zend_register_internal_class_ex` etc.): the only layer that
   knows Zend.

For `#[php_class]` the macros generate an implementation of the trait
`RegisteredClass` (constants `CLASS_NAME`, `EXTENDS`, `IMPLEMENTS`, `FLAGS`,
`DOC_COMMENTS`, plus `get_metadata()`, `method_builders()`, `constructor()`,
`constants()`, `default_init()`, `clone_obj()`). For `#[php_impl]`, each
method yields a `FunctionBuilder` with handler plus parameter metadata
(name, type, nullable, default, required count, return type).

### 1.2 The Rust struct lives inside the PHP object

`ZendClassObject<T>` is `{ obj: T, std: zend_object }`. The Rust payload
is never copied or serialised; methods get `&self`/`&mut self`
directly on the embedded struct. Fields with `#[php(prop)]` become
PHP properties with generated getter/setter function pointers
(`PropertyDescriptor`) that are called through object handlers on property
access.

### 1.3 Conventions instead of annotations

| Rust signature | PHP side |
|---|---|
| `&self` / `&mut self` | instance method |
| no receiver | static method |
| `self_: &mut ZendClassObject<T>` | whole PHP object (for returning `$this`, properties) |
| `__construct(..) -> Self` or `Result<Self, E>` | constructor; `Err` is thrown |
| `Option<T>` parameter | nullable and omittable |
| `&[T]` as last parameter | variadic |
| `-> &Self` / `-> &mut Self` | fluent interface, returns `$this` |
| `-> Result<T, E: Into<PhpException>>` | exception in PHP |
| `snake_case` method | automatically `camelCase` |
| `const X` in the `impl` | class constant |

Sub-attributes in `#[php(...)]`: `name`, `change_case`, `vis = "protected"`,
`defaults(a = 5)`, `optional = x`, `getter`/`setter` (virtual properties
from `get_x`/`set_x`), `constructor`, `abstract`, `final`, `flags`,
`extends(Base)`, `implements(Iface)`, on fields `prop`, `static`,
`default`.

### 1.4 The generated handler

Each method yields an `extern fn handler(ex, retval)` that always runs the
same way: check argument count, read each argument from the frame, convert via
`FromZval`, throw `TypeError` on failure, fetch `this`, call the
Rust method, write the result back via `IntoZval`. Everything runs inside
`run_handler`, which catches panics and throws them as PHP `Error`. The conversion
is strict: `"12"` does not become `int`.

### 1.5 Conversion

Traits `FromZval` and `IntoZval` with `const TYPE: DataType` and
`const NULLABLE: bool`. Implemented for primitives, `String`, `&str`, `Vec`,
`HashMap`, `Option`, `Callable`, `Closure`, every registered class.
`#[derive(ZvalConvert)]` turns a plain struct into a DTO (fields ↔ properties
of a `stdClass` object).

### 1.6 Registration, interfaces, stubs

`#[php_module] fn get_module(m: ModuleBuilder) -> ModuleBuilder` with
`.class::<T>()`, `.function(wrap_function!(f))`, `.constant(..)`. Explicit
list, deterministic order. `#[php_interface]` on traits,
`#[php_impl_interface]` collects the mappings via `inventory`.
`cargo php stubs` generates PHP stubs for the IDE from the metadata.

## 2. Transferable or not

**Transferable (and to be adopted):** the attribute names, the table in 1.3,
the three-way split macro/builder/registration, `FromVal`/`IntoVal` with
`TYPE`/`NULLABLE`, strict argument conversion, `Result` → exception,
panic catching per call, explicit registration, stub generation, the
test strategy (`macrotest` for expansion snapshots, `trybuild` for
compile errors on wrong attributes).

**Not transferable:** everything from layer 3. We own the VM. There is no
Zend object layout, no offset trick, no `zend_fastcall`, no
arg_info tables, no MINIT memory model, no module globals, no
bailout guard. Our builder layer writes into `class_table`, the handler
is a safe Rust function.

**To be solved differently:** ext-php-rs keeps request state in module globals or
thread-locals. We attach the context to the `Engine` (3.5). ext-php-rs
joins the separate expansions of `#[php_class]` and `#[php_impl]`
through an autoref-specialisation trick (`PhpClassImplCollector`). We
require both traits at registration (3.2); then a forgotten
`impl` block is a compile error.

## 3. Target design for OpenGrid

> **Implementation status (2026-10-23, phase 6):** The API from §3.2 to §3.5
> is built in `crates/vm/src/vm/native.rs`, without proc macros: registration
> via the `NativeClassDef` builder, handler `fn(&mut Vm, Call) -> Result<Val, Throw>`,
> `Call { this, class, args, named }`, property handlers `ObjectHandlers`,
> `FromVal`/`IntoVal`, `Callable`, engine API `static_default`,
> `instance_default`, `has_method`, `is_overridden`, `native_payload(_mut)`,
> `get_property`/`set_property`, `host::<T>()`. Example: `tests/native_classes.rs`
> (`Model`/`Builder` per `DATABASE_LAYER.md`). The macros (§3.6) and the
> stub generation (§3.7) are still open; `This` is called `ObjHandle`.


### 3.1 What the Rust developer writes

```rust
use opengrid_core::prelude::*;

#[php_class]
#[php(name = "OpenGrid\\Support\\Log")]
pub struct Log;

#[php_impl]
impl Log {
    /// Writes an info line to the module's tracing log.
    pub fn info(engine: &mut Engine, message: String, context: Option<Val>) {
        let module = engine.ctx::<RequestCtx>().module_name();
        tracing::info!(target: module, %message, ?context);
    }
}

#[php_class]
#[php(name = "OpenGrid\\Http\\Controller", flags = ClassFlags::Abstract)]
#[derive(Default)]
pub struct Controller { middleware: Vec<String> }

#[php_impl]
impl Controller {
    #[php(vis = "protected")]
    pub fn middleware(self_: &This, name: String) -> Result<Val, Throw> {
        self_.with_native_mut(|c: &mut Controller| c.middleware.push(name))?;
        Ok(self_.to_val())            // $this for the fluent interface
    }
}

#[php_class]
#[php(name = "OpenGrid\\Database\\DB", flags = ClassFlags::Final)]
pub struct DB;

#[php_impl]
impl DB {
    pub fn table(engine: &mut Engine, name: String) -> Result<QueryBuilder, Throw> { .. }
    pub fn transaction(engine: &mut Engine, callback: Callable) -> Result<Val, Throw> {
        let tx = engine.ctx::<RequestCtx>().db.begin()?;      // block_on in the worker
        let result = callback.call(engine, &[])?;             // calls the .grd closure
        tx.commit()?;
        Ok(result)
    }
}
```

Conventions in addition to 1.3:

- A parameter of type `&mut Engine` is injected and is not a PHP parameter.
  It comes first (for instance methods, after the receiver).
- `&self`/`&mut self` locks the object for the duration of the method. Whoever
  needs to call back into the engine (callbacks, instantiation) takes `self_: &This`
  and locks briefly on their own. This corresponds to ext-php-rs' `ZendClassObject` form.
- Returning `Self` or `T: NativeClass` creates a new object with payload.

### 3.2 Traits and definitions (crate `core`)

```rust
pub trait NativeClass: Any + Send + Sized {
    const NAME: &'static str;                    // fully qualified, "OpenGrid\\Http\\Controller"
    const PARENT: Option<&'static str>;
    const INTERFACES: &'static [&'static str];
    const FLAGS: ClassFlags;                     // Abstract, Final
    const DOC: &'static [&'static str];
    fn default_init() -> Option<Box<dyn Any + Send>>; // payload for `new` without a native constructor,
                                                      // also when a .grd subclass is instantiated
}

pub trait NativeMethods: NativeClass {           // generated by #[php_impl]
    fn methods() -> Vec<MethodDef>;
    fn constants() -> Vec<(&'static str, Val, Visibility)>;
}

pub struct ClassDef {                            // result of registry.class::<T>()
    pub name: &'static str, pub parent: Option<&'static str>,
    pub interfaces: &'static [&'static str], pub flags: ClassFlags,
    pub constants: Vec<(&'static str, Val, Visibility)>,
    pub methods: Vec<MethodDef>,
    pub default_init: Option<fn() -> Box<dyn Any + Send>>,
    pub doc: &'static [&'static str],
}

pub struct MethodDef {
    pub name: &'static str, pub visibility: Visibility,
    pub is_static: bool, pub is_abstract: bool, pub is_final: bool,
    pub params: Vec<ParamDef>, pub required: usize, pub returns: TypeInfo,
    pub handler: NativeHandler, pub doc: &'static [&'static str],
}

pub struct ParamDef {
    pub name: &'static str, pub ty: TypeInfo, pub nullable: bool,
    pub default: Option<Val>, pub variadic: bool, pub by_ref: bool,
}

pub struct FunctionDef { /* like MethodDef without the receiver fields */ }

pub type NativeHandler = fn(&mut Call<'_>) -> Result<Val, Throw>;

pub struct Call<'a> {
    pub engine: &'a mut Engine,
    pub this: Option<This>,                      // ObjHandle, not a borrow (RUNTIME_ARCHITECTURE.md §2.2)
    pub args: &'a [Val],
    pub called_class: &'a str,                   // for static::
}

pub struct NativeRegistry { functions: Vec<FunctionDef>, classes: Vec<fn() -> ClassDef> }
impl NativeRegistry {
    pub fn class<T: NativeClass + NativeMethods>(self) -> Self;
    pub fn function(self, def: FunctionDef) -> Self;
}
```

The registry is passed to `Engine::new(registry)`. At construction, each
`ClassDef` becomes a `ClassEntry` with `MethodBody::Native` and is in the
`class_table` **before** the first module is compiled. This is necessary because
the compiler already reads the `class_table` at compile time (parent classes,
`self::`/`parent::` binding in `compile/statement/oop.rs:249`).

### 3.3 Objects, payload, `This`

```rust
pub struct ObjHandle(pub u32);                           // index into ExecuteData.objects, RUNTIME_ARCHITECTURE.md §2

pub struct PhpObject {
    pub class: ClassId,
    pub properties: PropertyTable,
    pub native: Option<Box<dyn Any>>,                    // Rust payload
    pub handlers: Option<&'static ObjectHandlers>,
}

pub struct This(ObjHandle);                              // access via execute_data.obj(h) / obj_mut(h)
impl This {
    pub fn with_native<T: 'static, R>(&self, f: impl FnOnce(&T) -> R) -> Result<R, Throw>;
    pub fn with_native_mut<T: 'static, R>(&self, f: impl FnOnce(&mut T) -> R) -> Result<R, Throw>;
    pub fn get_prop(&self, name: &str) -> Option<Val>;
    pub fn set_prop(&self, name: &str, v: Val);
    pub fn class_name(&self) -> String;
    pub fn to_val(&self) -> Val;
}
```

For `Model`: `$task->title` is data and lives in `properties`, not
in the Rust struct. The payload holds only Rust state (query builder,
original attributes for dirty tracking). `Model` implements `__get`,
`__set`, `__isset` as native methods. `#[php(prop)]` on struct fields,
as ext-php-rs has it, is not needed for now (6.3).

A `.grd` class that inherits from a native one receives, on `new`, the payload
of the nearest native ancestor via `default_init()`. If the native ancestor
has a constructor and the subclass calls `parent::__construct(..)`, the
native constructor runs with `this` and sets the payload itself.

### 3.4 Conversion

```rust
pub trait FromVal: Sized {
    const TYPE: TypeInfo;
    const NULLABLE: bool = false;
    fn from_val(v: &Val, engine: &Engine) -> Result<Self, Throw>;   // Err = TypeError
    fn from_missing() -> Option<Self> { None }
}
pub trait IntoVal {
    const TYPE: TypeInfo;
    fn into_val(self, engine: &mut Engine) -> Result<Val, Throw>;
}
```

Implementations: `i64`, `f64` (also accepts `int`), `bool`, `String`,
`&str` (Into only), `Vec<T>`, `IndexMap<String, T>`, `Option<T>`, `Val`,
`ObjHandle`, `Callable`, `serde_json::Value` (cache/queue/JSON responses),
`T: NativeClass` (Into: new object with payload; From: downcast, otherwise
`TypeError`), `Result<T, E: Into<Throw>>` (Into only). No coercion except
`int → float` (DIALECT §5.2). The `Engine` is a parameter because objects need a
class and a handle.

### 3.5 Engine API, context, callables, exceptions

```rust
impl Engine {
    pub fn new(registry: NativeRegistry) -> Engine;
    pub fn load_module(&mut self, root: &Path) -> Result<(), CompileError>;
    pub fn reload_file(&mut self, path: &Path) -> Result<(), CompileError>;
    pub fn instantiate(&mut self, class: &str, args: &[Val]) -> Result<ObjHandle, Throw>;
    pub fn call_method(&mut self, this: ObjHandle, method: &str, args: &[Val]) -> Result<Val, Throw>;
    pub fn call_static(&mut self, class: &str, method: &str, args: &[Val]) -> Result<Val, Throw>;
    pub fn call(&mut self, callable: &Callable, args: &[Val]) -> Result<Val, Throw>;
    pub fn set_ctx(&mut self, ctx: Box<dyn Any + Send>);
    pub fn ctx<T: 'static>(&mut self) -> &mut T;           // panics if the type does not match: programming error
    pub fn classes_with_attribute(&self, attr: &str) -> Vec<ClassInfo>;
}

pub struct Callable(Val);                                  // Closure object or [obj, 'method'] or 'Class::m'
impl Callable { pub fn call(&self, engine: &mut Engine, args: &[Val]) -> Result<Val, Throw>; }

pub struct Throw { pub class: String, pub message: String, pub code: i64, pub previous: Option<ObjHandle> }
impl Throw {
    pub fn new(class: &str, message: impl Into<String>) -> Throw;
    pub fn type_error(msg) / value_error(msg) / argument_count(expected, got);
}
impl From<&str> for Throw { /* class = "Exception" */ }
impl From<String> for Throw {}
```

`Throw` becomes an exception object in the VM (`instantiate(class,
[message, code])`) and is passed on as `pending_exception` via the existing path
`propagate_after_call` (`vm/exception_dispatch.rs`). The
classes `TypeError`, `ValueError`, `ArgumentCountError` etc. from DIALECT
§5.3 must be real `ClassEntry`s for this.

`Callable` holds the value and calls back via `vm/callable.rs` (`invoke_callable`,
`invoke_closure`, `invoke_user_function`). The call is synchronous on
the same worker thread; no state leaves the worker (architecture version 1, now `archiv/ROADMAP_ausgeschnitten_2026-10-25.md`).

The request context (`RequestCtx { app: AppContext, request, session,
module }`) is set by the framework per request with `set_ctx` and read by native
methods with `engine.ctx::<RequestCtx>()`. No additional
handler parameter.

### 3.5a Requirements from `DATABASE_LAYER.md` §2.3 (2026-10-23)

The engine API must additionally offer: `class_meta(class_id)` (layout,
static and instance defaults such as `$table`/`$fillable` with types,
attributes), `static_default`/`instance_default`, `find_method` and
`is_overridden` (for `toArray()` overrides, scopes, hooks),
property handlers get/set/isset/unset on native objects with fallback to
`__get`/`__set` of the subclass, and `FromVal`/`IntoVal` for
`surrealdb::sql::Value` including type classes. Signatures: `RUNTIME_ARCHITECTURE.md`
§4.2 and §8.1 (version 2 is authoritative where this document deviates).

### 3.6 Macros (crate `macros`)

- `#[php_class]` on a struct → `impl NativeClass for T` plus `IntoVal`/`FromVal`
  for `T`. Options in `#[php(..)]`: `name`, `extends`, `implements`, `flags`.
- `#[php_impl]` on an `impl` block → `impl NativeMethods for T` and per method
  a handler function `fn(&mut Call) -> Result<Val, Throw>` that fetches arguments
  via `FromVal`, checks `this`, calls the method, converts the result via
  `IntoVal`. Options as in ext-php-rs: `name`, `change_case`, `vis`,
  `defaults`, `optional`, `constructor`, `abstract`, `final`; `getter`/`setter`
  later.
- `#[php_function]` on free functions → `FunctionDef`, `wrap_function!`.
- `#[derive(ValConvert)]` for DTO structs. Open question 6.2 whether as array
  or object.
- Every handler runs inside `catch_unwind`; a panic becomes `Throw::new("Error",
  "Rust panic: ..")`, the worker thread lives on.
- Tests: `trybuild` (wrong attributes are compile errors with a good
  message), `macrotest` (expansion snapshots), integration tests in `core`
  that register a test class and run `.grd` code against it.

### 3.7 What the `.grd` developer sees

Nothing to declare. Native classes are present because they are in the
`class_table` before the module load:

```php
<?php
namespace Tasks\Controllers;

use OpenGrid\Http\Controller;
use OpenGrid\Support\Log;
use OpenGrid\Database\DB;

class TaskController extends Controller
{
    public function __construct()
    {
        $this->middleware('auth');
    }

    public function index(): array
    {
        Log::info('listing tasks');
        return DB::table('tasks')->where('done', false)->orderBy('id')->get();
    }
}
```

For editors and the dialect check, `opengrid stubs` generates stub files from the `ClassDef`s
(`storage/stubs/OpenGrid/Http/Controller.grd`) with signatures,
types and documentation, analogous to `cargo php stubs`. The dialect check can later
use them to report unknown classes as `GRD` errors.

### 3.8 Module plugins (M6)

A module crate uses the same macros. In dev mode it exports
`extern "C" fn opengrid_plugin() -> PluginDescriptor { api_version, classes:
fn() -> Vec<ClassDef> }`, loaded via `libloading`; in prod mode the
crate is linked statically and calls `registry.class::<T>()` directly. The
`fn() -> ClassDef` pattern is the same as `ModuleBuilder.classes` in
ext-php-rs.

## 4. Mapping onto today's phprs structures

| Today (`src/engine/`) | Target |
|---|---|
| `ExecuteData` carries engine state and the current frame mixed together (`vm/execute_data.rs:93`) | `Engine { class_table, functions, registry, ctx, objects }` + `Frame` per call |
| `PhpValue::Object(Box<PhpObject>)`, `clone_val` copies deeply (`facade/factory.rs:116`), `handle` always 0 | `PhpValue::Object(ObjHandle)`, store per request, cloning = copying the handle (`RUNTIME_ARCHITECTURE.md` §2) |
| `ClassMethod { op_array }` (`types.rs:516`) | `ClassMethod { body: MethodBody::{Php(OpArray), Native(MethodDef)} }` |
| `PhpObject { class_name, properties }` (`types.rs:410`) | plus `native: Option<Box<dyn Any + Send>>` |
| `is_builtin_function` as `matches!`, `execute_builtin_function` as `match` (`vm/builtins.rs:321, 388`) | `HashMap<&str, FunctionDef>` is consulted before the `match`; `match` remains as fallback |
| `execute_new_obj` hard-codes DateTime, DateTimeZone, ArrayIterator, Fiber, SplStack (`vm/dispatch_handlers.rs:1635ff`) | `default_init()` from `ClassEntry` |
| `execute_do_method_call` hard-codes Reflection*, Throwable getters (`:2246ff`) | `MethodBody::Native` |
| `execute_do_static_call`, `__callStatic` (`:4586, :4745`) | branch on `MethodBody::Native` (facades) |
| `__get`/`__set`/`__call`/`__toString`/`__invoke` each build an `OpArray` (`:34, :927, :1929, :2080, :4389`) | call via `lookup_method` + `MethodBody` |
| `facade/factory.rs` knows only constructors (`long_val`, `string_val`, `null_val`) | `FromVal`/`IntoVal` |
| `vm/callable.rs` (`invoke_callable`, `invoke_closure`, `invoke_user_function`) | stays, used by `Callable::call` |
| `pending_exception` + `propagate_after_call` | stays, `Throw` ends up there |
| parser skips `#[..]` (`compile/statement/oop.rs:42`, `skip_attribute_block`) | attributes on `ClassEntry`/`ClassMethod` for `#[Get('/x')]` (M2) |
| `function_table: Arc<dyn Any + Send + Sync>` | concrete type |

## 5. What is missing, in working order

### 5.1 Interpreter (M0, before the registry)

1. **Object identity.** Objects have value semantics: `$b = $a; $b->v = 5;`
   leaves `$a->v` unchanged, likewise parameter passing, `use ($a)`,
   `$h->box->v = ..`, `$arr[0]` and returns from methods. All objects are
   `===` because `handle` is never set. Details and snippet in
   `KNOWN_ISSUES.md`. Without the fix no `Model` works, no
   `Blueprint $t` in `Schema::create`, no `Request`, no native payload.
   Fix: object store with `ObjHandle` per `RUNTIME_ARCHITECTURE.md` §2.5
   (steps 1–7), plus `Rc<PhpArray>` with copy-on-write and an explicit
   call stack (§8 there).
2. **Property operators.** `$o->v++`, `$o->v--`, `++$o->v` are lost
   (`+=` works). `$arr[0]->v = 9` is a compile error ("Expected
   '=' after array index assignment target"). Both in `KNOWN_ISSUES.md`.
3. **Exception hierarchy as classes.** `Error`, `TypeError`, `ValueError`,
   `ArgumentCountError`, `DivisionByZeroError`, `UndefinedKeyError`,
   `UndefinedVariableError` as real `ClassEntry`s, so that `Throw` from
   native code and the switch from warnings → exceptions (DIALECT §5.3)
   use the same classes.
4. **Strict comparisons** (DIALECT §5.1), already in ROADMAP M0.
5. **Interfaces.** `implements` is discarded (`KNOWN_ISSUES.md`). Needed for
   `INTERFACES` in `ClassDef`, `instanceof`, DI by interface type.
6. **Autoloader / `load_module`.** Today `compile_file_with_functions(file)`
   for a single file; `class_table` is created per compile run. Needed:
   module root, namespace→path (DIALECT §2), and `CompileContext.class_table`
   pre-populated with the native entries.
7. **Frame restructuring.** Separate `ExecuteData` into `Engine` and `Frame`;
   remove the `global_imports` plumbing (`KNOWN_ISSUES.md` §5); follow up
   `use (&$x)`. This is the prerequisite for `Call<'_>` and for callables from
   native code.
8. **Global state.** `OnceLock`s (opcode cache, JIT, interner, `GLOBALS`,
   archiv/INVENTORY §5) into the `Engine` or behind features; remove `unsafe impl Send/Sync`
   on `PhpValue` as soon as `ObjHandle` and the raw-pointer variants are
   settled (6.1).
9. **Store attributes** instead of skipping them (for M2, can wait).

### 5.2 Core API for native classes (M1)

1. `Engine`, `Call`, `This`, `Throw`, `Callable` from 3.2 to 3.5.
2. Function registry before the builtin `match` (9 call sites, archiv/INVENTORY §4).
3. `MethodBody::{Php, Native}` and the branch in `execute_do_method_call`,
   `execute_do_static_call`, `lookup_method` and the magic-method paths.
4. `PhpObject.native` and `default_init` in `execute_new_obj`.
5. `FromVal`/`IntoVal` for the types from 3.4.
6. Extend `ClassEntry`: `interfaces`, `attributes`, `doc`, `is_native`.
7. Proof: switch `Reflection*`, Throwable getters, `DateTime`, `ArrayIterator`,
   `SplStack`, `Fiber`, `Generator` from string comparisons to the registry.
   Afterwards `dispatch_handlers.rs` may no longer contain any class name
   as a literal.
8. `Log` and `Config` as the first native classes, `Controller` as the first
   inheritable one; acceptance as in ROADMAP M1.

### 5.3 Macro crate (M1)

`#[php_class]`, `#[php_impl]`, `#[php_function]`, `#[php(..)]`,
`#[derive(ValConvert)]`, `wrap_function!`, tests with `trybuild` and
`macrotest`. The macros only generate calls to the API from 5.2; they know
no VM internals.

### 5.4 Framework (M0 to M3)

The `framework` crate needs from `core` only `Engine`, `NativeRegistry`,
`Val`, `Throw`, `Callable` and the conversions. It provides:

1. `Hooks` implementation, VM pool (one `Engine` per worker thread, shared
   `Arc<OpArray>`), catch-all routes, `RouteTable` (ROADMAP M0, BRIDGES §1–§4).
2. `RequestCtx` and the edges from ROADMAP §2.2: HTTP body → `Val`,
   `Val` → `serde_json::Value`, DB rows → `Val`, `Val` → `sea_query::Value`.
3. Native classes in `framework/src/native/`: `Log`, `Config`, `Controller`,
   `Route` (M0/M1); `Request`, `Response`, `Middleware`, `Validator`,
   `Session` (M2); `DB`, `QueryBuilder`, `Model`, `Schema`, `Blueprint`,
   `Cache` (M3).
4. `opengrid stubs` in the `cli` crate (3.7), by M1 at the latest, otherwise no
   editor has completion for native classes.

## 6. Decisions and open questions

### 6.1 Decided

- Attribute names and conventions as in ext-php-rs (1.3), so that manual
  and vocabulary are reusable and module authors know the pattern.
- Handler signature `fn(&mut Call) -> Result<Val, Throw>`; context on the
  `Engine`, not as a parameter. This replaces `NativeFn = fn(&mut Engine,
  &mut dyn Any, &[Val])` from ROADMAP §2.1.
- `#[php_class]` and `#[php_impl]` generate two traits; `registry.class::<T>()`
  requires both.
- Explicit `NativeRegistry` instead of `#[php_module]`.
- Objects are `ObjHandle`s into a store per `ExecuteData` (request),
  no `Arc`, no `Mutex`, no `Rc`. Decided in
  `RUNTIME_ARCHITECTURE.md` §2 and §7; the earlier choice `Arc<Mutex<PhpObject>>`
  is thereby obsolete. Only code is shared between workers (`Arc<OpArray>`
  without raw pointers, §4 there); objects never leave their worker.
- `Model` data lives in `properties`, not in the Rust struct (3.3).

- **Compare handler** (Jonas 2026-10-23): `ObjectHandlers.compare:
  Option<fn(&mut Vm, ObjHandle, &Val) -> Result<Option<Ordering>, Throw>>`.
  `Vm::binary` calls it for `==`, `!=`, `<`, `<=`, `>`, `>=`, `<=>` when one
  side is an object of a native class with a handler (right-hand side:
  reversed ordering). Without a handler, `==` on objects remains identity.
  Intended for value types such as `DateTime`, `Decimal`, `RecordId`.

### 6.2 Open

- DTOs from Rust (`#[derive(ValConvert)]`): as array (fits Laravel code,
  `$r->validated()` is an array) or as object? Proposal: array, until the
  test app demands otherwise.
- Property hooks on native objects (`#[php(prop)]` as in ext-php-rs): only
  when a class needs them; `Request`/`Response` get by with methods.
- Named arguments (`Route::get(uri: '/x')`) to native methods: `ParamDef`
  has the names, the VM knows `call_arg_names`; check implementation in M2.
- Reference parameters (`by_ref`) to native functions (`preg_match(&$m)`):
  DIALECT §9 allows them for now; the handler then receives a
  `RefSlot` instead of `Val`. Decision together with DIALECT §9.
