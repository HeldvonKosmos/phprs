# Decisions (grd-vm, option B)

Append only, never rewrite. Entries marked `[provisional]` await Jonas.

## 2026-10-23 — Parser: mago-syntax 1.58.2
Context: §2 of the task requires mago as the parser; a parser of our own is ruled out.
Checked: crates.io `mago-syntax` 1.58.2 (plus `mago-allocator`, `mago-database`, `mago-span` in the same version). License MIT OR Apache-2.0. `rust-version = 1.99.0`, the container has 1.99.1. API: `mago_syntax::parser::parse_file(&LocalArena, &File) -> Program<'arena>` with `program.errors: Vec<ParseError>` and `program.statements`; CST in `mago_syntax::cst`, walker in `mago_syntax::walker`. Arena allocation (`mago-allocator::LocalArena`), nodes carry the lifetime `'arena`. Error-tolerant parsing (`ParserSettings::recover_incomplete`), off for us: a syntax error is a compile error.
Options: (a) mago-syntax, (b) php-rust-tools/parser (archived 2024), (c) our own parser (forbidden).
Decision: (a). Versions pinned with `=`, upgrades deliberate.
Consequences: crate `grd-vm` depends on four mago crates; CST types with `'arena` must not end up in `OpArray` (OpArray is pure data, RUNTIME_ARCHITECTURE §4).
Status: [final]

## 2026-10-23 — Crate layout: `phprs/crates/vm` as `grd-vm`, binary `grd`
Context: the task names `phprs/crates/vm/`, working name `grd-vm`. The phprs repo is already a workspace (`.`, `bin/phprs`).
Decision: `crates/vm` as the third workspace member; lib `grd_vm`, binary `grd` with `grd run <file>` (same invocation form as `phprs run`, so the harness under `.agents/feasibility/` can keep using `$BIN run` unchanged) and `grd parse <files…>` for phase 0. The old core keeps building untouched.
Status: [final]

## 2026-10-23 — Strings: `Rc<Vec<u8>>` (bytes, copy-on-write), not `Rc<str>`
Context: PHP strings are byte sequences (`strlen` counts bytes, `mb_*` characters); `json_encode`, `hash`, `base64` work on bytes. `.=` in loops must be able to append in place.
Options: (a) `Rc<str>` with UTF-8 guarantee, (b) `Rc<[u8]>` immutable, (c) `Rc<Vec<u8>>` with `Rc::make_mut`.
Decision: (c). Appending copies only if the buffer is shared; `as_str()` returns `Option<&str>`, `mb_*` validate.
Consequences: one more indirection per read access; if measurements (phase 3/4) show it, small-string optimization later.
Status: [final]

## 2026-10-23 — Arithmetic only with `int`/`float`
Context: DIALECT §5.2 forbids implicit string→number conversion; PHP allows `"5" + 1`, `null + 1`, `true + 1`.
Decision: operands of `+ - * / % **` and comparisons must be `int` or `float`, otherwise `TypeError` (also `null` and `bool`). `(int)`, `intval`, `is_numeric` remain the explicit path. Laravel code with `$x ?? 0` is not affected.
Consequences: `Val::as_number(op)` in `value/convert.rs`. If the combined probes show that `null`/`bool` in arithmetic is common, this can be relaxed.
Status: [provisional — Jonas to confirm]

## 2026-10-23 — New GRD codes for the constructs dropped in §3
Context: the fork knew GRD001 (include), GRD002 (global), GRD004 (`array()`), GRD010 (forbidden builtins). The task additionally drops references, `list()`, heredoc, `static` closures, `declare(ticks)`.
Decision: GRD003 variable variables, GRD005 `list()`, GRD006 references (all forms), GRD007 heredoc/nowdoc, GRD008 `static fn`/`static function`, GRD009 `declare` except `strict_types`, GRD011 `goto`, GRD012 superglobals, GRD013 `static $x` in functions, GRD014 `eval`, GRD015 `exit`/`die`, GRD016 `__halt_compiler`, GRD017 inline HTML, GRD018 backticks. Global `const X = …;` is rejected like `define()` (GRD010, DIALECT §4.2 "global constants"). `"${name}"` interpolation (deprecated since PHP 8.2) falls under GRD003. Dynamic member names (`$o->$name`, `$o->{$expr}`) remain allowed (Laravel models need them).
Consequences: `crates/vm/src/compile/dialect.rs`, tests in `crates/vm/tests/dialect.rs`. `docs/errors.md` (DIALECT §7) is generated from this once the crate layout is settled.
Status: [final] (Jonas 2026-10-23: GRD003–018 confirmed, dynamic member names remain allowed)

## 2026-10-23 — Bytecode: stack machine with slot variables, lvalue paths as one opcode
Context: RUNTIME_ARCHITECTURE §4 requires `OpArray` as pure data with IDs instead of names. Nested writes (`$a['x'][] = 1`, `$this->items[$k]->v = 2`) need a `&mut` path with copy-on-write arrays.
Decision: operands on a stack per frame, local variables in slots (assigned at compile time, `$this` separately). A write access is **one** opcode `StorePath { base, segs, mode }` (base: slot/`$this`/static property/stack value; segments: Dim, Append, Prop, PropDyn); the VM walks the path with `&mut` through arrays and switches container at object handles. The same for `+=`, `++`, `unset`. Read accesses are normal fetch opcodes. Names (classes, methods, properties, functions) are `StrId`s; the VM resolves them at load/call time, inline caches go on the side table (§3.1).
Consequences: `bytecode.rs` (~120 opcodes), `compile/`. Serde derivation follows in phase 7.
Status: [final]

## 2026-10-23 — VM layout: one dispatch loop, explicit frame stack, nested run only for Rust callbacks
Context: RUNTIME_ARCHITECTURE §1/§8 require an own call stack without Rust recursion per dialect call. Native builtins with callbacks (`usort`, `array_map`), magic methods (`__toString`, `__get`), the iterator protocol and generators, however, need "call dialect code and wait for the result".
Decision: `Vm::run(base_depth)` runs until the frame stack falls back to `base_depth`. Dialect→dialect calls only push a frame (`Flow::Switch`). Rust→dialect (`call_value`, `call_method_sync`, `instantiate`) pushes a frame and calls `run` nested; the Rust depth is bounded by the nesting of such constructs, not by the script's recursion depth. Generators are a detached `Frame` in the object payload; `yield` detaches it, `resume` reattaches it.
Consequences: `fib(25)` and recursion depth 100,000 run without stack overflow (measured, debug build 1.2 s for both together). The old opcode upper limit is gone.
Status: [final]

## 2026-10-23 — Dialect-related deviations in the probes (confirmed, see decisions of 2026-10-23 below)
Context: four probes expect PHP behavior that DIALECT §5 deliberately changes: `a45` (`clone` → `==` true), `a65`/`18` (`1 == 1.0`, `"1" == "01"`, `null == false` true), `a55` (`usort` sorts in place). Under the dialect `==` is identical to `===` and `usort` returns the sorted array.
Decision: these probes count as "deviating due to the dialect", not as errors, and are counted separately in STATUS. The by-ref builtins (`sort` family, `array_push`, `array_unshift`, `array_splice`, `shuffle`) return the new array; `array_pop`/`array_shift` — **replaced by GRD019** (decisions of 2026-10-23, item 2): both are rejected, the replacements `Arr::pop`/`Arr::shift` return `[$value, $rest]`. `preg_match` returns the match array (empty on no match).
Status: [final] (Jonas 2026-10-23; supplemented by the entries "No references" and "One equality" below)

## 2026-10-23 — `<?grd` as opening tag
Context: Jonas' wish to replace `<?php` at the top with `<?grd`; DIALECT §2 allows both.
Decision: the compiler replaces a leading `<?grd` with `<?php` before parsing (same length, offsets and line numbers stay valid); mago stays unchanged. The prelude begins with `<?grd`. The probes under `.agents/feasibility/` keep `<?php`, because `php` must execute them as reference; the copies under `crates/vm/tests/probes/` likewise (their `.out` files come from PHP). *(Superseded for `crates/vm/tests/probes/`: see entry 2026-10-26 TEST2.3.)*
Status: [final]

## 2026-10-23 — RUNTIME_ARCHITECTURE revision 2 and DATABASE_LAYER adopted
Context: Jonas has completely rewritten the runtime architecture (revision 2) and settled the database layer on SurrealDB with a native port of an existing `App\Database` layer (a private reference implementation, not part of this repository).
Decision: both documents lie unchanged in `.agents/` (`RUNTIME_ARCHITECTURE.md` replaces revision 1, backup in the scratchpad; `DATABASE_LAYER.md` new). ROADMAP M3, NATIVE_CLASSES §3.5a and DIALECT §6 refer to them. Deviations of today's implementation from revision 2 that will be aligned by phase 7: method lookup via `HashMap<name>` instead of vtable (phase 5 with ICs), frame slots as a separate `Vec` instead of a window in the shared slot stack, `Rc<Vec<u8>>` instead of `Rc<[u8]>`, `arg_present` vector instead of a sentinel, names instead of IDs in the ops (link step phase 7). The phase 6 acceptance is now `Model`/`Builder` per DATABASE_LAYER §4; for that `tests/database/builder_cases.php` from the private reference implementation is still missing (Jonas).
Status: [final]

## 2026-10-23 — Numeric strings are allowed in arithmetic
Context: DIALECT §5.2 says "arithmetic with non-numeric strings throws TypeError"; my entry from this morning had excluded all strings. `preg_match` hits are strings, `$m[0] * 2` is common Laravel code.
Decision: fully numeric strings (`"12"`, `" 1e3 "`) are treated as numbers in arithmetic and comparisons, leading-numeric (`"12abc"`) and non-numeric ones throw `TypeError`. `null` and `bool` remain excluded.
Status: **replaced by the decision of 2026-10-23** — Jonas: not allowed. The entry "Arithmetic only with int/float" applies again: request input arrives typed from the native Request class, SurrealDB delivers typed values; `(int) $m[0]` makes the intent visible. Code rolled back (`Val::as_number`).

## 2026-10-23 — Native classes without proc macros (phase 6)
Context: NATIVE_CLASSES §3.6 provides for `#[php_class]`/`#[php_impl]`. For the phase 6 acceptance the underlying API suffices; the macros will later generate exactly these calls.
Decision: `NativeClassDef::new("Name").extends(..).payload(..).method("m", fn).static_method(..).handlers(..)` as builder API; handler signature `fn(&mut Vm, Call) -> Result<Val, Throw>` with `Call { this, class, args, named }`. Native methods are stored as `MethodInfo { native: Some(fn), unit: None }` in the same class table as dialect methods and are therefore overridable/inheritable; `parent::m()` from the subclass calls the native handler. Property handlers (`get`/`set`/`isset`/`unset`) apply after declared slots and before `__get`/`__set`. The payload factory of the nearest native base runs on `new` of every subclass. `FromVal`/`IntoVal` without `TYPE` constants (they come with the macros/stubs).
Consequences: `crates/vm/src/vm/native.rs`, test `tests/native_classes.rs` with `Model`/`Builder`. `class_meta` is covered by `static_default`/`instance_default`/`has_method`/`is_overridden`/`property_type`; a summarizing struct follows when the framework code needs it.
Status: [final]

## 2026-10-23 — Only actively maintained dependencies; bincode replaced by postcard
Context: Jonas' rule: no unmaintained crates in the project; for small dead crates decide whether we maintain them ourselves or take an alternative. `bincode` was pinned as 1.3 (2021); meanwhile there is bincode 3.0 (active), but switching to a serde-native format is simpler anyway.
Review of the direct dependencies of `grd-vm`: mago-* 1.58 (active, 2026), indexmap 2, serde/serde_json 1, chrono 0.4, chrono-tz 0.10.4 (current), fancy-regex 0.14 → 0.19.2, bcrypt 0.17 → 0.19.3, getrandom 0.2 → 0.4.3, `regex` was unused.
Decision: `postcard` 1.1 (serde-compatible, actively maintained, no_std-capable) for the `Module` serialization; getrandom/fancy-regex/bcrypt raised to the current majors; `regex` removed. Maintaining any of these crates ourselves is not worthwhile. Rule from now on: before every `cargo add` check the maintenance state and record it here.
Status: [final]

## 2026-10-23 — Decisions by Jonas (feedback on the five open points)
1. **GRD003–018 confirmed.** Dynamic member names remain allowed.
2. **No references, as a principle.** The dialect has no references: no `&$param`, no `foreach (… as &$v)`, no `use (&$x)`. DIALECT §5.6 describes this including the table "PHP signature → dialect signature". The `sort` family, `array_push`, `array_unshift`, `array_splice`, `shuffle`, `parse_str` return the new array; `preg_match`/`preg_match_all` the match array (empty on no match). `array_pop`/`array_shift` are **not** defused (silent endless loop in `while ($x = array_pop($s))`), but rejected with **GRD019**; the replacements `Arr::pop($a)`/`Arr::shift($a)` return `[$value, $rest]` (prelude class `Arr`, later `OpenGrid\Support\Arr`). Safety net **GRD020**: the return value of a function from this list must not be discarded (`sort($a);` is a compile error).
3. **One equality.** `==` is strict; `switch`, `in_array`, `array_search`, `array_keys($a, $v)`, `array_unique` also compare strictly (implemented, `array_unique` was still loose). Objects: `==` is identity. Native classes register a **compare handler** (`ObjectHandlers.compare`) so that `<`, `>`, `==`, `<=>` work on value types (DateTime, Decimal, RecordId); added in NATIVE_CLASSES §6, built into `Vm::binary`.
4. **Numeric strings in arithmetic: not allowed** (see above, rolled back).
5. **`builder_cases.php`** will be delivered by Jonas; until then phase 6 counts as "checked against our own expectation", not as accepted.
6. **Merge `vm-core` → `main`** after Jonas' review; the merge does not conclude the task. The five deviations from RUNTIME_ARCHITECTURE revision 2 (names instead of IDs in the ops, no link step, HashMap lookup instead of vtable, frame slots as a separate Vec, `arg_present` vector) are worked off **before** the framework crate. Order: link step and vtables → autoloader with module root → crate `framework` → database layer.
Status: [final]

## 2026-10-23 — Performance guideline (Jonas)
It is currently **not** the goal to be faster than PHP. The interpreter VM is slower than PHP 8.4, which is to be expected. The goal of this stage is a foundation that the later Cranelift JIT can use well; speed comes in tier 2.
- Factors against PHP are observations, not acceptance criteria; do not optimize for them.
- Yardstick: opcodes with clear, typable semantics; fixed IDs instead of names (link step); vtables and stable class IDs; inline caches and type feedback in the CodeProfile; frame layout with fixed slots; no aliasing possibilities.
- Micro-optimizations that worsen the opcode design only after consultation and with an entry here. Review of the existing superinstructions: `BinSS`/`BinSI`/`BinSC` are `LoadSlot; LoadSlot|PushInt|PushConst; Bin`, `JumpUnlessSS`/`SI` are `BinSS|SI; JumpIfFalse` — all decompose into base opcodes for the JIT, no hidden type assumptions (the int fast path has the same semantics as `Bin`). `CompoundSlot`/`IncSlotDiscard` are `LoadSlot; …; StoreSlot`. So they stay.
- Regressions (arrays/objects 1× → 1.7×) are narrowed down with `git bisect`, as the question "has the opcode design gotten worse?". Benchmarks remain trend line and regression guard; a jump of about 50 % between two commits is a signal.
Status: [final]

## 2026-10-23 — Bisect arrays/objects 1× → 1.7×: start time, not an opcode problem
Context: Jonas wanted the regression narrowed down with `git bisect` (question: has the opcode design gotten worse?).
Measurement (release builds per commit, wall time incl. start): b4_arrays 16 ms (phase 3) → 21 ms (phase 4) → 36 ms (phase 5a, reflection) → 34 ms (5b); b7_objects 26 → 30 → 46 → 48 ms. The jump lies in phase 5a, which only added reflection classes to the prelude (now 562 lines) and introspection natives — no change to ops or dispatch. Start time with an empty file: grd 24–32 ms, PHP 26 ms. The prelude is compiled and linked at every start; the benchmarks (20–40 ms of work) were dominated by it.
Decision: no opcode design error. From now on the bench harness subtracts the start time (empty file) and takes the median of three runs. The prelude remains dialect code; for CLI start time the prod path (embed a precompiled prelude via postcard, `opengrid build`) is the solution, not now. In the worker model the start happens once per process.
Status: [final]

## 2026-10-23 — Project loader: convention instead of autoload table
Context: DIALECT §2/§3 fixes namespace = `<Module>\<Folder>\<Class>` and "one file, one class". A PSR-4 autoloader with lazy loading (load a class only on first access) does not fit the link step, which must know all class IDs before the run.
Decision: no lazy autoload. `Vm::load_project(root)` reads the whole tree at start, checks the convention as **link errors** (GRD101 name≠path, GRD102 more than one class, GRD103 top-level code in a class file, GRD104 unknown parent class/interface/trait, GRD105 cycle across files), sorts the class files topologically by `extends`/`implements`/`use` and loads them in that order; afterwards the route files run (module routes before global ones). Module name → namespace via StudlyCase (`master_data` → `MasterData`). For `opengrid build` this is the same input as for the prod link.
Not done: files under `modules/*/app` without a class (functions) — DIALECT §3 does not provide for them; interfaces/traits/enums count as "class".
Status: [final]

## 2026-10-23 — Framework crate: Loco from crates.io, worker pool, request boundary
Context: order per Jonas (2026-10-23): after the autoloader the crate `framework`. ROADMAP §1 provides for a Loco fork (`HeldvonKosmos/loco`, branch `opengrid`) as a Git dependency.
Decisions:
1. **Loco 1.2.0 from crates.io**, not the fork. The fork only has `master` (unchanged upstream), and no patch was needed for the first request. Features: `default-features = false` + `cli`; no `with-db` (SeaORM/sqlx stay out, the database layer is SurrealDB per DATABASE_LAYER.md). In addition axum 0.8, matchit 0.9, tokio, tracing, async-trait — all actively maintained.
2. **Worker pool with std threads and mpsc** (RUNTIME_ARCHITECTURE §9): one `Vm` per thread, each loads prelude, native classes and the project itself; Tokio sends a `FnOnce(&mut Vm)` and waits on a oneshot. A panic in the job is caught, the VM rebuilt. No `Send` needed on the VM.
3. **Request boundary = `Vm::reset_request`**: object store, enum instances, stacks, output gone; code, classes, profiles, inline caches, static properties remain (DIALECT §5.4). The host context (`WorkerCtx`: route table, settings snapshot, log ring) remains; only its `request` part is set per request.
4. **Route handlers only as `[Controller::class, 'method']`**, no closures: a closure is an object and dies with the request in which `routes/*.grd` ran. `Route::get` throws a TypeError on closures. Every worker has its own identical route table (filled at load) — no `ArcSwap` needed as long as there is no hot reload.
5. **Namespaces of the native classes**: `OpenGrid\Routing\Route`, `OpenGrid\Http\Request`, `OpenGrid\Http\Response`, `OpenGrid\Support\Log`, `OpenGrid\Support\Config`; helpers `request()`, `response()`, `config()`, `env()`. Return value mapping per ROADMAP §3: array→JSON, string→HTML, `Response` object, `null`→echo output or 204; uncaught exceptions → JSON 500 with class, message, line (error page comes with the dev comfort).
6. **Configuration** = Loco's `settings:` block as a JSON snapshot per worker; `settings.opengrid.root` / `OPENGRID_ROOT` is the module root, `settings.opengrid.workers` the pool size. Loco needs `workers.mode: BackgroundAsync` without a queue.
Not done: middleware, validation, sessions, Inertia (M2), hot reload, error page, linker check "no object in a static property".
Status: [final]

## 2026-10-24 — Database layer: surrealdb 3.3, one client per pool, Builder/Model native
Context: DATABASE_LAYER.md (SurrealDB instead of SeaORM). `surrealdb` 3.3.4 is from 2026-10-13, releases weekly — maintained. Features only `kv-mem` in addition to the defaults (ws, rustls); RocksDB/SurrealKV are added as features when needed.
Decisions:
1. **One SDK client per pool, not per worker.** The client is `Clone + Send + Sync` and multiplexes; with `mem://` one connection per worker would moreover be a separate database per worker. The workers call `Runtime::block_on` on a small Tokio runtime of their own (2 threads, `grd-db`) — never on Loco's runtime, which is why `Pool::start` starts in `spawn_blocking`.
2. **Transactions via the SDK's client transactions** (`begin`/`commit`/`cancel`), held in `WorkerCtx.txn`; every query of the worker runs in this transaction as long as it is open. Nested `DB::transaction` run within the outer one.
3. **Values**: `RecordId` is a native class `OpenGrid\Database\RecordId` with compare handler (value comparison, `==`), `JsonSerializable` → `"table:key"`. **[provisional, superseded by the entry "SurrealDB types native" of 2026-10-24]** `Datetime` as RFC 3339 string, `Decimal` as float. `$dates` fields are parsed from strings on write.
4. **Builder format** per DATABASE_LAYER §2.1 and the phase 6 test: `SELECT … FROM … WHERE a AND b GROUP BY … ORDER BY … LIMIT n START m FETCH …`; `sort("f:desc")` → `f COLLATE DESC`; `whereLike` → `string::contains(string::lowercase(<string>f), $fN)`; bindings `f0, f1, …`; the soft-delete clause `deleted_at IS NONE` comes first. Writes: `CREATE table CONTENT $data`, `UPDATE $rid MERGE $data`, `DELETE … RETURN BEFORE`. Until `builder_cases.php` arrives: our own expectation (unit test `renders_like_the_php_builder`).
5. **Model metadata** is read once per class from the declarations and cached in the `WorkerCtx` (`static_default`/`instance_default`). `$timestamps` (default true) sets `created_at`/`updated_at` as `Datetime`. Hooks `creating/created/updating/updated/saved/deleting/deleted` only if the subclass declares them.
6. **Errors**: query errors are thrown as `RuntimeException` ("Surreal: …"); a dedicated `QueryException` class comes when native exceptions can be defined without a prelude detour. `filter()` with a disallowed field → `ValueError`.
Not done: `Schema`/migration runner, side tables (`withAll`), aggregates (`withSum`), dot-path sorting with sub-select, `filter()`/`sort()` without argument from the request, live queries, relations/`relate()`.
Status: [final]

## 2026-10-24 — Middleware pipeline in the dialect, validation in Rust
Context: ROADMAP §3 M2: script middleware `handle(Request $r, Closure $next)` **inside** the catch-all, validation with Laravel rules, own parser instead of the `validator` crate.
Decisions:
1. **Pipeline as dialect code** (`prelude_http.grd`), not in Rust: `$next` must be a real `Closure` (strict signature `Closure $next`), and the dialect builds a closure over `$this` in one line; the innermost step calls the native function `opengrid_run_controller`. The framework thus gets its own prelude, which runs after the native classes and before the project. Probe beforehand with `grd run`: `new $class()`, closures over `$this` and exceptions with extra arguments work in the dialect.
2. **Route objects**: `Route::get()` returns a `Route` instance (payload = index in the route table), `->middleware()`/`->name()` write into the entry; `Route::group(['prefix' => …, 'middleware' => […]], fn)` keeps a group stack in the `WorkerCtx`. Middleware names are class names or aliases (`Route::aliasMiddleware`).
3. **Errors → HTTP**: `OpenGrid\Http\HttpException(status, message, headers)` (dialect class) and `abort()` (native) deliver their status; `OpenGrid\Validation\ValidationException` delivers 422 with `{message, errors}`; everything else remains JSON 500 with class/message/line (error page follows with M4).
4. **Validation** (`native/validation.rs`): rules `a|b:1,2`, messages in Laravel's wording (English), `validate()` returns only the ruled, present fields; no casts (form values stay strings, as in Laravel). `regex` rule via the `regex` crate. Not included: `exists:`/`unique:` (need the database — second round), nested fields (`items.*.id`), custom messages/attributes.
Status: [final]

## 2026-10-24 — Hot reload as a whole VM per worker, error page from the dispatcher
Context: RUNTIME_ARCHITECTURE §10 provides for hot reload per file (swap the affected `Arc<OpArray>`s and `ClassDef`s, discard profiles) and an error page on the next request.
Decisions:
1. **Coarse first: rebuild the whole VM.** An `AtomicU64` "generation" in the pool; the watcher (notify 8, debounce 150 ms, only `.grd`) increments it, every worker compares before every job and on a mismatch rebuilds prelude + natives + project (≈ 30–60 ms). Running requests finish on the old code because the swap happens between two jobs. The fine-grained swap of individual modules comes when it is measurably needed (project size); the interface (`Pool::reload`) stays the same.
2. **Failed reload** (compile/link error with GRD code): old code stays, `WorkerCtx.stale_error` is set, every response shows the error until a reload succeeds — exactly the behavior from §10 ("error page on the next request").
3. **Backtrace in the core**: exception objects get a `trace` array on `new` (call site per frame, `class`/`type`/`function`), the outermost frame called from Rust with its declaration line. Cost only on exceptions.
4. **Error page** decides on the `Accept` header: `text/html` → HTML page (debug: location, source ±6 lines, trace, last 20 log lines; otherwise generic), everything else → JSON (`file`/`trace` only in debug). `debug`/`watch` follow the Loco environment (development = on), overridable in `settings.opengrid`.
Status: [final]

## 2026-10-24 — Schema and migrations: blueprint → DEFINE, runner as a Loco task
Context: DATABASE_LAYER §2 (`schema.rs`, `migrator.rs`), ROADMAP M3 (migration runner, `opengrid db migrate`).
Decisions:
1. **Tables are SCHEMALESS with typed fields** unless the migration says otherwise (`$t->schemafull()`): `Model` writes `$fillable` fields and timestamps freely, SurrealDB still checks type and index of the defined fields (proven in the test: unique violation and wrong type throw). `nullable()` → `option<T>`, `timestamps()`/`softDeletes()` → `option<datetime>`. Idempotence through `IF NOT EXISTS`; `->change()` → `OVERWRITE`.
2. **Migrations are named classes** (`final class CreateTasks extends Migration`), one per file, order by file name (timestamp prefix), files under `database/migrations/` and `modules/*/database/migrations/`; the project loader does not touch them (only `app/` and `routes/`). Bookkeeping in `_opengrid_migrations` with batches like Laravel; `rollback(steps)` takes the last batches in reverse name order.
3. **CLI via Loco tasks** (`opengrid task migrate`, `migrate_status`, `migrate_rollback steps:N`): Loco's `db` subcommands depend on the `with-db` feature, which stays off; dedicated top-level commands come with the crate `cli` (`opengrid db migrate` as alias). The tasks run through the worker pool so that migrations see the same VM environment as requests.
4. **`Schema::sql()`** returns the statements without executing them — testability and later `opengrid db show`.
Status: [final]

## 2026-10-24 — Side tables/aggregates as correlated sub-selects, filter casting
Context: DATABASE_LAYER §2 (`aggregates.rs`: "one grouped query per metric", `side_tables.rs`: "withAll with sub-query closure"), without the PHP sources at hand.
Decisions:
1. **One query instead of N+1**: `withCount`/`withSum`/… and `withAll` are placed as sub-selects into the projection (`((SELECT VALUE math::sum(f) FROM rel WHERE fk = $parent.id GROUP ALL)[0] ?? 0) AS rel_sum_f`, `(SELECT * FROM rel WHERE fk = $parent.id AND …) AS rel`); SurrealDB handles the correlation via `$parent`. The sub-builder gets a binding prefix (`s1_f0`) so that its bindings fit into the parent query. If `builder_cases.php` shows another form (separate grouped queries), that is a rendering change, not an API change.
2. **Foreign key convention** `singular(table)_id` with a small English plural rule (`inflect.rs`), overridable via the third argument.
3. **Dot-path sorting** projects `path AS __sort_path` and sorts by the alias (only with `*` projection), as described in §2.1.
4. **Request-fed `filter()`/`sort()`**: query strings are parsed nested like `$_GET`; filter values from strings are converted to `true`/`false`/`null`/int, because SurrealDB does not match `done = 'true'` against a bool. This is deliberately guessing by value (not by field name); with schema introspection it becomes type-exact later.
5. **Graph edges** only minimal: `relate(edge, target, attrs)` and `related(edge, Class)`; traversals in queries (`->edge->table`) come when an application needs them (DATABASE_LAYER §5 open).
Status: [final]

## 2026-10-24 — Sessions in the worker instead of tower-sessions
Context: ROADMAP §3 provided for `tower-sessions` with a store (memory/Redis/SQL); Inertia needs sessions for errors/flash.
Decisions:
1. **The session lives in the worker, not in the tower stack.** The dialect code accesses `Session::` synchronously in the worker; a tower middleware would have to load the data completely before the dispatch and write it back afterwards — which is exactly what `session.rs` now does directly (load on first access, save after the response). No additional dependency, no duplicated state.
2. **Stores**: memory (process-wide across all workers, with expiry) for development and tests, SurrealDB table `_opengrid_sessions` (UPSERT by id, `expires_at`) for operation — SurrealDB is available anyway; Redis only comes if needed. The payload is a JSON string (a conversion the dialect can already do).
3. **Flash like Laravel**: keys from `_flash_new` are visible in the following request and are removed on save unless flashed again (`reflash`).
4. **CSRF as dialect middleware in the prelude** (`VerifyCsrfToken`, alias `csrf` pre-registered): checks `X-CSRF-TOKEN` or `_token` against `Session::token()`, otherwise 419. Not globally active — Inertia/forms attach it to their groups.
Not done: cookie encryption/signature (the id is 160 bits of randomness, the content lies server-side only), garbage collection of the DB table (expiry is checked on load; a cleanup task follows).
Status: [final]

## 2026-10-24 — Objects in static properties: runtime error; enum cases as engine singletons (Jonas)
Context: `reset_request` empties the object store, static properties remain (DIALECT §5.4). An object in a static property pointed to a foreign handle in the next request. Jonas: runtime error instead of linker check; enum cases must remain allowed (enum constants in classes).
Decision:
1. **Writing to a static property** (`Class::$x = …`, `static::$x = …`, also path writes like `Class::$x[] = …`) rejects an object value or an array that contains an object anywhere with `Error`: "Cannot store an object of class … in static property …::$…: static properties outlive the request; only enum cases are allowed". Checked in `static_prop_set`, the only write path.
2. **Enum cases are engine objects**: besides the request slots the object store has an engine area (handle top bit set); `reset_request` empties only the request slots. Enum cases are created there, remain identical singletons across requests and may be stored in static properties and class constants (`const DEFAULT = Status::Done`).
3. Test `crates/vm/tests/statics.rs`: rejection directly, via method, nested, via append; enum in static/constant remains the same instance after `reset_request`.
Not done: objects in class constants other than enums (PHP only allows `new` in initializers from 8.1 for parameters there, not for constants — no case).
Status: [final]

## 2026-10-24 — Legacy removal (Jonas), steps 1–3
Context: the old core (`legacy/core`, 33,700 lines, 609 tests) has no code connection to the new VM; Jonas: remove with conditions (tag, keep the package manager, harvest the test scripts, LICENSE/NOTICE, docs).
Decisions and results:
1. **Tag `legacy-core-last`** is on the last commit with the old core in the tree (after the harvest), in addition to `upstream-baseline`; study and probe comparison remain reproducible.
2. **Package manager to `crates/pkg`** (`opengrid-pkg`, lib + binary). The only old-core dependency was the lexer in the classmap scan; replaced by a regex scan, the mago CST pass is noted as a task in STATUS. Dependencies raised to maintained majors (zip 9, dirs 7, toml 1, reqwest 0.13, sha2 0.11).
3. **Harvest**: the reference is PHP 8.4, not the expectation of the old tests. 188 scripts; 175 PHP-clean, 157 identical in the new VM, 16 runtime gaps (SPL structures, Fiber, DirectoryIterator, individual builtins), 1 deviation, 1 GRD. The 157 are frozen as probes. Found and fixed along the way: `Name::class` was resolved at runtime (the class had to exist); now a compile-time string as in PHP.
4. **Steps 4 and 5 (2026-10-24, approved):** `legacy/core` and `bin/phprs` removed, LICENSE (Apache 2.0) stays, `NOTICE` names origin and test material; KNOWN_ISSUES/FEASIBILITY/README carry the fixed comparison values and the tag. STATUS only counts grd-vm, framework, pkg any more.
5. **pkg becomes the module manager** (Jonas): `.grd` modules from Git/registry into `modules/`, GRD check on install, lock file. Porting the classmap scan is dropped — third-party PHP does not run in the dialect, and for `.grd` modules the project loader derives class and namespace from the path; the Composer part goes as soon as the CLI crate embeds the manager.
6. **`Name::class`** is resolved by the compiler via namespace, `use` imports, aliases, group imports, `namespace\` prefix and fully qualified names; `self::`/`static::`/`parent::class` remain runtime. Test `crates/vm/tests/names.rs`.
Status: [final]

## 2026-10-24 — Open architecture question: file system in the dialect (noted)
Context: the legacy harvest shows scripts with `file_exists` and `DirectoryIterator`; GRD010 today forbids the I/O builtins of the PHP standard library (`ob_*`, `define`, …) as global side effects.
Question: does the dialect get direct file system access at all, and if so in what form — PHP builtins (`file_get_contents`, `file_exists`, `scandir`) or only the native class `Storage` (ROADMAP §3: `Storage::disk('local')->get/put/exists`, drivers local/S3)? For requests in the worker everything speaks for `Storage` (path sandbox, drivers, tests without a real file system); for CLI tasks and migrations a local driver would be the same interface. Proposal: no PHP file system builtins, `Storage` as the only path, decision once the actual file-access needs of a private reference implementation (not part of this repository) are known.
Status: [open]

## 2026-10-24 — SurrealDB types native (Jonas, addendum B)
Context: Decimal-as-float and Datetime-as-string were makeshift. Goal per DATABASE_LAYER: the SurrealDB types native, no generic PHP mapping.
Decisions:
1. **One native class per type with the SDK type as payload**, no new `Val` variant: `OpenGrid\Database\{Decimal, Duration, Uuid, Geometry, Bytes, RecordId}` in the framework crate. **Datetime is the exception**: it is the PHP core type. `DateTimeImmutable` and `DateTime` become native classes in `grd-vm` (payload: UTC instant, microseconds, time zone name; methods via the existing Rust helpers in `builtins/datetime.rs`), implement `DateTimeInterface` and replace the two dialect classes in the prelude (≈ 75 lines less, less start time). `DateTimeZone`/`DateInterval` remain dialect.
2. **The database delivers `DateTimeImmutable`.** Assessment of the alternative (mutable `DateTime`): attributes lie in COW arrays and model payloads and are shared; a mutable date changed in one place would silently change in all others — exactly the class of bugs PHP eliminated with `DateTimeImmutable`. A value type from the database must be immutable; whoever wants to mutate takes `DateTime::createFromImmutable`. `$dates` fields also deliver `DateTimeImmutable`; on write, `DateTimeInterface` objects (both classes) are bound as `Datetime`, strings in `$dates` fields are parsed.
3. **Writing without guessing**: the type of the value determines the binding (Int → int, Float → float, Decimal object → decimal, Datetime object → datetime, RecordId object → record). Strings stay strings — also `"tasks:abc"`; whoever means a record id takes `RecordId::parse()`/`RecordId::make()`. `Model::find($key)` takes a bare key for the model's table.
4. **Filter values** (`filter()`, also from the request) are converted according to the field type of the migration (`INFO FOR TABLE`, cached once per table in the worker): `bool`, `int`, `float|number|decimal`, `datetime`, `record<t>` (here `"t:key"`/bare key is allowed, because the type says so). Without schema information the string stays a string. Guessing by the look of the value (early 2026-10-24) is thereby withdrawn.
5. **Arithmetic handler** `ObjectHandlers.arith` only for native classes, analogous to the compare handler in `Vm::binary`: Decimal ± × ÷ with Decimal/int/float → Decimal; Duration ± Duration, Duration × ÷ int → Duration; `DateTimeImmutable` ± Duration → `DateTimeImmutable`. Everything else remains TypeError.
6. **NONE/NULL**: on read, `NONE` and `NULL` become `null`. On write, `null` is bound as `NONE` (omit the field or remove it via `MERGE`) — for `option<T>` fields the only valid form, and for all other fields "no value" means the same in the dialect. `whereNull()` asks `IS NONE`.
7. **JSON**: Decimal as string (no rounding), Datetime ISO 8601 (RFC 3339, UTC `Z` or offset), RecordId `"table:key"`, Duration as text (`1h30m`), Uuid as string, Bytes Base64, Geometry as GeoJSON object.
8. **Result rows lazy**: a model holds the SDK `Object` and converts fields into `Val`/handles only on access (cache per field); `toArray()` converts everything. `DB::select` continues to deliver finished arrays.
9. **Tests**: round trip per type against `mem://`; against the real server when `OPENGRID_TEST_DB_URL` (+ `_NS`, `_DB`, `_USER`, `_PASS`) is set, otherwise the test is reported as skipped.
Status: [final] (implementation in progress)

## 2026-10-24 — Integrate Cranelift directly, no Wasmtime, no Wasm intermediate layer (Jonas, R0)
Context: tier 2 of the runtime (RUNTIME_ARCHITECTURE) provides for a JIT. The choices were Cranelift via Wasmtime (opcodes → Wasm → Wasmtime), Cranelift directly (opcodes → CLIF), or an own template JIT.
Decision: **Cranelift is integrated directly** (`cranelift-frontend`, `cranelift-codegen`, `cranelift-jit`, `cranelift-module`, plus `cranelift-native` for target detection), not via Wasmtime and not via a Wasm intermediate layer (nor wasmi). The JIT translates opcodes from the OpArray to CLIF.
Consequences: the opcode format must remain typable and decomposable into basic blocks; superinstructions must remain decomposable. Fallback to the interpreter (deopt), guards and side exits we build ourselves; the models are YJIT and HHVM. Tier 2 is **not worked on now**; the references have only been reviewed and documented.
Probe (2026-10-24, `phprs/target/refs/cranelift-probe`, not in the repo): FunctionBuilder → machine code → call with guard and side-exit block runs on x86_64 with Cranelift 0.137.0; `cargo tree` contains neither `wasmtime` nor a Wasm crate. **Finding:** two helper crates from the Wasmtime workspace are transitive mandatory dependencies of `cranelift-codegen`/`cranelift-jit`: `wasmtime-internal-core` (≈7,000 lines of utilities, "INTERNAL") and `wasmtime-internal-jit-icache-coherence` (≈480 lines, cache invalidation for JIT code). Neither contains a Wasm runtime; they are unavoidable and accepted.
Versions: Cranelift 0.137.0 = Wasmtime tag v50.0.0 (Cranelift minor = Wasmtime major + 87); crates.io release 2026-10-20; four crates published independently, license Apache-2.0 WITH LLVM-exception.
Status: [final]

## 2026-10-24 — Schema management declarative with SurrealKit (Jonas, R0/R4 section 8)
Context: a private reference implementation (not part of this repository) does not manage its database schema with versioned migration classes but declaratively as a target state. Per module there are `blueprint.surql` (generated from the blueprint) and `manually.surql` (manual adjustments from Surrealist), plus a base file `main.surql` in the project. `migrate:push` parses all files (SurqlFileParser), merges them into one AST (SurqlSchema::merge, the later file wins per key), writes the sorted result as `database/schema/temporary.surql` (SurqlGenerator) and aligns the database with `surrealkit sync`. `migrate:pull` reads the actual state (`INFO FOR DB` / `INFO FOR TABLE`, SurqlDbParser) into the same form. `migrate:fresh` removes the database or all tables and calls push. For changes to running systems there is `surrealkit rollout` (plan, manifest `rollouts/*.toml` with review, start = expansion phase, cutover, complete or rollback). SurrealKit keeps its own tables `__entity` and `__rollout` (and `__seed`).
The migration runner of 2026-10-24 (migration classes, batches, `_opengrid_migrations`, `migrate`/`migrate_status`/`migrate_rollback`) is the Laravel model and does not fit this approach.
Options: (a) imperative migration classes as before; (b) declarative with SurrealKit like the reference implementation; (c) own sync mechanism instead of SurrealKit.
Decision: **(b)**. The target schema lies as `.surql` in the modules, SurrealKit aligns. Details:
 1. Files and order (exactly as in `MigratePush::collectSourceFiles`): `framework.surql` (embedded in the binary, see 4.) → `database/sources/main.surql` → all `modules/*/database/schema/blueprint.surql` (alphabetically by module) → all `modules/*/database/schema/manually.surql` (alphabetically by module). Later files win per key (table: whole definition; field/index/event: by name). First ALL blueprint files, then ALL manually files. PHP's glob sorts, Rust must sort explicitly. The module path replaces the vendor package path.
 2. The merging (parser, AST, generator, DB parser) is ported to Rust: `native/database/schema/{ast,parser,merge,generator,dbinfo}.rs`. The result must be BYTE-IDENTICAL to the PHP output (acceptance in R4).
 3. SurrealKit is initially called as an external process (as today via passthru), encapsulated in `schema/kit.rs`. Credentials via environment variables (`SURREALDB_PASSWORD` among others, proven in the probe), not on the command line. An own sync mechanism (c) is NOT planned. Embedding as a library: see the follow-up decision below.
 4. The framework ships its own schema fragment (`framework.surql`, at the start of the merge order) for its tables, e.g. `_opengrid_sessions`. **Probe (2026-10-24)**: `surrealkit sync` by default only removes objects that SurrealKit itself created and lists in `__entity` (ns `schema`); a table created externally survived two syncs, even one with prune. The fragment is still needed so that the framework tables are typed and under management. `--no-prune` reports outdated managed objects instead of deleting them. Internal tables `__entity`, `__rollout`, `__seed` are excluded on pull.
 5. Commands as Loco tasks with the same names as in Laravel: `migrate_push`, `migrate_pull`, `migrate_fresh` (later `migrate:push` etc. in the CLI crate). `migrate_fresh` requires `--force` and refuses in the production environment without `--force`; option `--keep-db` as in the reference implementation. For rollout a thin pass-through to SurrealKit.
 6. The connection configuration reads the same environment variables as the reference implementation: `SURREAL_DB_HOST`, `_PORT`, `_NAMESPACE`, `_DATABASE`, `_USERNAME`, `_PASSWORD` (defaults localhost, 8000, main, main, root, root). The mapping to the Loco settings is documented. (SurrealKit itself expects `SURREALDB_HOST`, `SURREALDB_NAMESPACE`, `SURREALDB_NAME`, `SURREALDB_USER`, `SURREALDB_PASSWORD`; `kit.rs` sets them for the child process.)
 7. The previous runner (batches, `_opengrid_migrations`, rollback) is not developed further and taken off the default path; the code stays marked as deprecated until the requirements analysis shows whether data migrations are needed. The blueprint (`Schema::table …`) remains as generator: `Schema::sql()` writes `blueprint.surql` per module. Jonas delivers the reference blueprint; until then our version is [provisional].
 8. Consequence for database types and filters (item B): the field type for filter values comes from the merged schema (`DEFINE FIELD … TYPE …`), not from migration classes. (B part 1 reads it from `INFO FOR TABLE` today; B part 2 switches to the schema.)
 9. The earlier decision "tables are SCHEMALESS with typed fields" is unverified: the placeholder in `SurqlSchema::ensureTable` uses SCHEMAFULL with PERMISSIONS NONE. Check against the actual output of the reference blueprint, report the finding.
Consequences: DATABASE_LAYER §2 (schema.rs, migrator.rs) and ROADMAP M3 are adjusted; the migration section in DIALECT/docs likewise. Implementation: R4 (after B part 2), golden tests against the PHP output first.
Status: [final] for the principle (declarative, SurrealKit, order, command names, environment variables); [provisional] for embedding SurrealKit and for blueprint and data migrations.

## 2026-10-24 — Follow-up decision: SurrealKit initially as a process, embedding possible (R0 probe)
Context: section 8.3 asks whether SurrealKit can be embedded as a library.
Finding: SurrealKit is a crate with library **and** CLI (`crates/surrealkit`, `[[bin]]` with feature `cli`); the library offers `run_sync(&Surreal<Any>, SyncOpts)` (`src/sync.rs:79`), `SyncOpts { prune, dry_run, … }`, `DbCfg::from_env`, `connect`. It depends on `surrealdb = 3.3.0` (Cargo unifies, our SDK is 3.3.4 → compatible). Version 1.0.0-beta.6 (2026-10-02), 4 betas in October, repository active (last push 2026-10-09), Apache-2.0, official SurrealDB project.
Decision: as an external process first (8.3), because the reference implementation uses it that way and the rollout commands are interactive anyway. Embedding via the library is technically possible and is considered when the start path (align the schema at start) is needed. Maintenance state: active, but beta — check behavior per version (the positional argument `rollout plan <id>` does not exist, for example, it is `--name`; `rollout start` without a prior `rollout baseline` returns an anyhow error with a hint at `--from <id>`; the CLI prints a stack backtrace with it, which at first looked like a panic).
Status: [provisional]

## 2026-10-24 — Reference versions (R0): Laravel 13.38.0 and SurrealKit 1.0.0-beta.6 [provisional]
Context: the Laravel and SurrealKit versions used by a private reference implementation (not part of this repository) are not yet available.
Decision: until delivery the current stable versions apply: `laravel/framework` v13.38.0 (tag 2026-10-20), `laravel/docs` branch 13.x (pinned 36fa00d), `laravel/laravel` v13.11.0; SurrealKit v1.0.0-beta.6. After delivery, re-pin the reference checkouts.
Status: [provisional]

## 2026-10-24 — D1: No service container and no service provider in the dialect (Jonas)
Context: Loco has no service container in the Laravel sense; there is `AppContext` (fixed fields), `SharedStore` (typed storage, key = Rust type, Loco's `src/app.rs:39`) and `Initializer` (`name`, `before_run`, `after_routes`, `check`). I had proposed a module service provider (`register`/`boot`) in the dialect with static bindings.
Options: (a) provider + container like Laravel; (b) provider without runtime container, static bindings in the link step (my proposal); (c) no provider, everything by convention.
Decision: **(c).** Rust level like Loco (AppContext, SharedStore, Initializer, no own container). In the dialect there is neither provider nor container: the module structure is deterministic by convention (Module\Folder\Class, GRD101–105); a provider would describe it a second time as freely shapeable code. Standards instead of flexibility.
Replacement for the provider duties:
 - Routes: `modules/<m>/routes/*.grd`.
 - Constructor injection: Rust resolves via type hints (DIALECT §6); concrete classes are instantiated recursively, without `bind()`.
 - Interfaces: exactly one implementation in the project → bound automatically; zero or several → link error with its own GRD code and hint. Instances live per request in the `WorkerCtx`, never process-wide. An attribute for selection only if usage figures from the private reference implementation show several implementations.
 - Middleware: class names in the routes; aliases only for framework-owned ones (`csrf`, later `auth`). `Route::aliasMiddleware` for project aliases is noted as deprecated, final after the usage figures.
 - Policies, listeners, commands: later by folder convention, do not build now.
 - Module dependencies: if needed declaratively in a module manifest (name, version, dependencies), designed together with pkg; the linker then checks that no module uses classes of an undeclared module. [provisional]
 - Static classes (`DB::`, `Route::`, `Session::`, `Log::`, `Config::`) replace the facades.
 - DIALECT §9: `app('surrealdb')` and the like become `DB::`; check `base_path()`, `env()`, `config()` as helpers.
Consequences: roadmap I1 (interface binding, constructor injection), MOD1 (manifest), MW1 (aliasMiddleware deprecated). Service provider under "Dropped".
Status: [final]; manifest and selection attribute [provisional].

## 2026-10-24 — D2: Database embedded by default (Jonas)
Context: the embedded engine is the same code as the server. So far `mem://` was only the test track, operation against the server.
Options: (a) server as default; (b) embedded as default, server selectable via the URL.
Decision: **(b).** The URL determines the engine: `mem://` (tests, volatile), `surrealkv://<path>` (persistent development, pure Rust), `ws://host:port` (server). If `SURREAL_DB_*` are set, server mode applies. One SDK client per pool stays (with `mem://` one client per worker would be a separate database). Log engine version and mode at start; in server mode query the server version and warn on a differing major version. Tests: `mem://` is the fast track; the track against the real server becomes the compatibility check before releases and after SDK updates (incl. test of the type parser `none | int` against real server output). Surrealist and `manually.surql` continue to run against the server (note it in the docs on `migrate_*`).
Finding surrealkv vs. RocksDB (probe 2026-10-24): SurrealDB 3.3.4 uses via `surrealdb-kvs-surrealkv` the engine `surrealkv ^0.21.4` (last 0.21.x on 2026-08-26), **not** the 1.0.0 released on 2026-10-13; repository Apache-2.0, active (push 2026-10-24, 18 open issues). RocksDB needs a C++ build (`librocksdb-sys`): considerably more build time and RAM, exactly what overloaded the host on 2026-10-24. Lock: surrealkv creates `LOCK`; a second process fails immediately with "Database at …/LOCK is already locked by another process", the first keeps running unaffected, after it ends the second opens normally.
Decision on the engine: `surrealkv://` becomes the default for persistent development [provisional, until a run with our pool and the tests is proven]; RocksDB only on explicit request (feature, not in the standard build).
Consequences: roadmap DB1 (URL engine selection, start log, version warning, feature `kv-surrealkv`), DB2 (seeds), B2c (server track). Limit: embedded means one instance, one process (see D4).
Status: [final] for the principle, [provisional] for surrealkv as default.

## 2026-10-24 — D3: Seeds via SurrealKit, embedded in the binary
Context: an embedded database is empty at the beginning.
Finding: SurrealKit offers idempotent seeding: every file is tracked in `__seed` by content hash and runs at first start or after a change (`surrealkit seed`, `--force`); as a library `Seed::from_dir(folder).run(&db)` or `Seed::embedded(&[EmbeddedSeedFile])` with macro `embed_seed!` (`crates/surrealkit/src/seed.rs:60–151`); template variables `${VAR}`.
Options: (a) own seed mechanism (`.grd` seeders like Laravel); (b) SurrealKit seeds as `.surql`.
Decision: **(b)**. Seeds lie as `.surql` in `modules/<m>/database/seed/` (alphabetically by module, like the schema), run after the schema sync at start and via task `db_seed` (`--force` repeats everything). `.grd` seeders (data with logic) only when usage figures demand them.
Consequences: roadmap DB2; `__seed` belongs to the pull exceptions.
Status: dropped (Jonas 2026-10-24, see "Seeds dropped").

## 2026-10-24 — D4: SurrealKit is embedded (Jonas; replaces "external process")
Context: sync and rollout shall run in the framework's process with the same SDK client; the program `surrealkit` is not needed at runtime and not shipped. Jonas' directive (2026-10-24): integrate SurrealDB and SurrealKit **only as crates**, never take over or fork source code.
Suitability probe (step 1, without rebuild; SurrealKit's sources, dependency probe `phprs/target/refs/kit-tree-probe`):
 a) The API takes an open client: `sync::run_sync(db: &Surreal<Any>, opts: SyncOpts)`, `sync::run_sync_with_filesystem_sources(db, opts, &Layout, &[SchemaFile])`; `rollout::run_baseline(db, folder, &Module)`, `run_plan(folder, RolloutPlanOpts)` (without DB), `run_start / run_complete / run_rollback(db, folder, RolloutExecutionOpts, &TemplateVars)`, `run_status(db, folder, selector)`, `run_repair`, `run_abandon_rollout`; connection optionally via `DbCfg::from_env` + `connect`. `SyncOpts { watch, debounce_ms, dry_run, fail_fast, prune, allow_shared_prune, allow_empty_prune, allow_all_statements, vars, folder, module, typegen_ts_* }`.
 b) Versions: SurrealKit requires `surrealdb ^3.3.0`; with our `=3.3.4` there is exactly one engine (`surrealdb-core v3.3.4` once in the tree), no duplication. `cargo tree -d` shows only the usual duplicate versions of small crates (e.g. `base64`). Important: SurrealKit's default features are `kv-mem` + `cli` (clap, inquire, rustls, tempfile) → `default-features = false, features = ["kv-mem"]` (later `kv-surrealkv`).
 c) Target schema as a directory (`folder`, default `./database/schema`, modules under `modules/<name>/schema`) or as a file list (`&[SchemaFile]`), plus macro `embed_schema!`. Our `temporary.surql` can be passed as a single `SchemaFile`.
 d) Library behavior (without `main.rs`, `tester/`, `typegen/`): no `process::exit`, no `println!`/`eprintln!`, no `panic!`/`unwrap()` outside tests; 13 `expect()` (invariants, among them 5 in `rollout/chain.rs`) → `catch_unwind` in `kit.rs` remains mandatory. Logging via the `log` crate (143 places) → bridge `tracing-log`. Global state: only one `OnceLock<Regex>` (cache, harmless). Tokio: one `tokio::spawn` (lease heartbeat of the rollout lock, `rollout.rs:3913`), no `block_on`/`Handle::current` → runs on the caller's runtime. The library reads `SURREALKIT_OWNER` and `SURREALKIT_SHARED_DB` from the environment (`sync.rs:738–757`, affects prune on shared DBs) → `kit.rs` must control that.
    **Correction:** `rollout start` without `rollout baseline` is **not a panic** but a regular error ("… no manifest starts from there … pass `--from <id>`"); the CLI appends a backtrace.
 e) Maintenance state: v1.0.0-beta.6 (2026-10-02), betas on 09-08, 09-12, 10-01, 10-02; 7 contributors, one of them with 132 of ~150 commits (bus factor 1); 12 open issues, among them #87 "--target is ignored by setup, seed, status, apply and typegen", #76 "--folder flag is ignored", #100 "DEFINE INDEX is rebuilt and times out on large table", #93 "SEQUENCE IF NOT EXISTS becomes OVERWRITE".
Options (step 2): A client hand-over → embed directly; B only own connection; C fork branch with client parameter; D copy parts.
Decision: **A.** The API takes `&Surreal<Any>`; one path for all modes (mem, surrealkv, server). C and D are out anyway (directive "only as crates"). If something is missing, propose it upstream and wait for the release, or write it against the SDK client ourselves.
Step 3 (part of R4): pin the version exactly (`=1.0.0-beta.6`), encapsulate the call in `schema/kit.rs`, no panics to the outside (`catch_unwind`, errors as `Result`), credentials never on a command line. Tests: sync against `mem://` and `surrealkv://`; a second sync changes nothing; prune affects only managed objects; rollout plan/start/complete/rollback against a persistent embedded DB; regression test "rollout start without baseline" → error with hint "rollout baseline first". The server start applies the schema.
Correction to "Schema management declarative" item 4: the prune concern is gone (probe: prune affects only objects managed by SurrealKit); pull exceptions are `__entity`, `__rollout`, `__seed`. Item 3 ("external process") and the follow-up decision "initially as a process" are hereby replaced.
Consequences: roadmap R4c; risk "SurrealKit is beta, bus factor 1".
Status: [final] (A); version [provisional] until the SurrealKit version used by the private reference implementation is known.

## 2026-10-24 — D5: Single-process model for jobs, scheduler and tasks (Jonas)
Context: an embedded engine belongs to exactly one process (probe: `LOCK`, second process fails immediately). Jonas needs background jobs and tasks with database access.
Decision:
 - **Jobs** run in the server process on the same DB pool (Loco `workers.mode: BackgroundAsync` or own in-process pool); jobs are `.grd` classes, executed in the worker VMs; job boundary = request boundary (`reset_request`). BackgroundAsync loses waiting jobs on restart (Loco: `tokio::spawn` per job, `bgworker/mod.rs:809`) → roadmap J4 job table in SurrealDB (pick-up via transaction, both modes), priority per usage figures.
 - **Scheduler** in the server process. Finding in Loco's source (`src/scheduler.rs`, `boot.rs:96`): the scheduler tick runs in-process (`opengrid start --all` starts it as a task next to server and workers), but **every job starts a new process**: `shell: true` → `/bin/sh -c <run>`, otherwise `<binary> task <name>` (`prepare_command`, ll. 160–176; execution via `duct`, ll. 193ff). With an embedded DB every job would fail at the lock — unless the child process hands the task back to the server via the control socket. Decision: own lean in-process scheduler that reads Loco's scheduler configuration (`scheduler.jobs`, cron and English expressions) and executes tasks directly on a worker; the same libraries (`tokio-cron-scheduler`, `english-to-cron`) are already transitive dependencies via Loco. [provisional]
 - **CLI tasks** (`opengrid task X`, also `migrate_*`): if a server is running, the task is handed to it and executed on a worker; output and exit code flow back. If none is running, the task boots itself: `surrealkv://` opens the file (on lock a clear message "Server running? Then via the control socket"), `mem://` reports that the database is empty.
 - **Forwarding** via a Unix domain socket `.opengrid/control.sock` in the project folder, permissions 0600, no TCP, protocol JSON lines, only `task` and `status`. Rationale smallest attack surface: file permissions of the operating system instead of own authentication, not reachable over the network, no HTTP stack with middleware, CSRF, routing and logging, two commands instead of a generic API. Alternative: local HTTP route (e.g. `POST /_opengrid/task` on 127.0.0.1) — rejected, because it would hang in the same router as the application, can accidentally become reachable on every interface and would need a token.
 - **Limit:** embedded = one instance, one process. Several instances or separate worker processes → server mode (queue backend Redis from `compose.yml` or later the job table).
Consequences: roadmap J1 (control socket + forwarding), J2 (jobs in-process), J3 (scheduler in-process), J4 (job table). Everything AFTER R4.
Status: [final] for the model; [provisional] for the own scheduler.

## 2026-10-24 — D6: `unsafe` only in the future crate grd-jit (Jonas)
Context: the task rule "no unsafe" collides with a JIT (executable memory, function pointers).
Decision: `grd-vm` keeps `#![forbid(unsafe_code)]`. The JIT later goes into its own crate `grd-jit`; there `unsafe` is limited to a few documented places (calling the generated code, passing pointers to VM structures), each with a `// SAFETY:` comment. The two helper crates `wasmtime-internal-core` and `wasmtime-internal-jit-icache-coherence` are accepted. The rule thus reads: "no `unsafe` except in `grd-jit`, there only at documented places".
Consequences: roadmap JIT1 (deferred).
Status: [final]

## 2026-10-24 — D8: JIT architecture: strictly typed, without deopt, multi-tier (Jonas)
Context: JIT1b tier 1 is built (Cranelift directly, commit dd3eef8): whole functions with declared int/float/bool signature and hot loops over int/float/bool slots via on-stack replacement. The loops today need entry guards on the slot types because local variables are untyped; exit on overflow, division by zero and integer division with remainder. Jonas has made two stipulations for the complete JIT.
Decision:
 1. **Strictly typed, no type guards, no deopt.** The JIT gets every type statically: parameters, return values and typed properties from the declaration, local variables from the inference in the compiler. Thus type guards, deoptimization and type fallbacks are gone; the codegen in Cranelift becomes simpler (one value = one Cranelift type). What is not a type question remains a value check with an error path (overflow, division by zero, index out of range). [final] for the goal.
 2. **Reference types via runtime functions.** Objects, arrays and strings stay in the Rust VM (reference counting via `Rc`, as in the interpreter). The generated code holds handles for them and calls helper functions of the VM ("runtime functions") for allocation, release, property and array access; Cranelift only provides offsets and pointer arithmetic. The helper functions live in `grd-jit` (D6: `unsafe` only there, `// SAFETY:` per place); `grd-vm` stays without `unsafe` and provides safe entry points. No own garbage collector; cycles as today.
 3. **Multi-tier like HHVM.** Tier 0 interpreter with counters (calls, back edges; today 1,000 and 2,000). Tier 1 fast translation with `opt_level = "none"` for low latency. Tier 2 background thread retranslates hot paths with `opt_level = "speed"`; the VM swaps the function pointer atomically (`AtomicPtr` per function), the next call uses the optimized code. Consequences from today's structure: `Module` hangs on `Rc` and is not `Send`, therefore the background thread gets its own copy of the affected code units (opcodes are pure data) and its own `JITModule`; the code lives as long as this module. Since every worker has its own VM, the background compiler shall be process-wide and give translated code to all workers.
Answers from Jonas (2026-10-24):
 - **Local variables:** "A local variable strictly keeps the type of its first assignment; a type change leads to a compile error." New rule GRD021 (DIALECT §5.2), implemented with JIT2a.
 - **Integer overflow:** "On an integer overflow there is no automatic fallback to float; instead a clean ArithmeticError is thrown." Deliberate deviation from PHP 8.4 (DIALECT §5.2), implemented with JIT2a identically in interpreter and JIT.
Addendum (D9): the extended JIT (item 3) is built but paused behind `GRD_ADVANCED_JIT`; the framework workers have the JIT only with `OPENGRID_JIT=on`. GRD021 and `ArithmeticError` are enforced only after the DX measurement.
Implementation of item 3 (JIT2c, commit 02a9981): tier 1 translates in the VM thread with `opt_level = "none"`, tier 2 in the process-wide thread `grd-jit-tier2` with its own `JITModule` and `opt_level = "speed"`. No `Rc` changes thread: the backend part of the VM holds `Arc` copies of the modules (bytecode is pure data, copied once per module) and sends self-contained jobs. Every translated path has a code slot with `AtomicUsize`; the background thread publishes with `Release`, the VM loads with `Acquire` on every call. Tier 2 is only swapped in if its buffer layout matches tier 1. Results lie in a cache by content key (module hashes, function table, region, entry types), so that further workers with the same project start directly on tier 2. The framework workers install the JIT (`build_vm`).
Finding: translation time in c3 on average 0.56 ms (tier 1) against 1.15 ms (tier 2); the runtime of both tiers is equal with today's codegen (c3 5.3× in both cases). The gain of the tiers today is the halved pause in the VM thread; the code only gets faster with JIT2a/JIT2b (no guards, reference types). Loop regions run after entry until exit in the old code; the new code takes effect on the next entry.
Consequences: roadmap JIT2a (GRD021, ArithmeticError, remove guards), JIT2b (reference types via runtime functions); JIT2c and JIT3 done.
Status: [final]

## 2026-10-26 — D18: TEST2 – follow-up of the tests per Tools.md §4 (Jonas)
Decision by Jonas, status [final]: after the running DBP1 and before PERF3 comes **TEST2**, one commit per item, Tools.md updated each time:
 1. Secure the commit flow (first): `commit.sh`/`green.sh` into the repo (`tools/`), Git hook via `core.hooksPath` (`.githooks/pre-commit`), in CLAUDE.md; CI (GitHub Actions) for vm-core and main with php 8.4 and surreal (the comparison against the private reference implementation stays local); switch `OPENGRID_REQUIRE_ORACLES=1` (missing oracle = failure; CI and commit.sh set it, `OPENGRID_ORACLES_EXCEPT=platform` in CI), without the switch a clear "SKIPPED" output.
 2. Deviations from §4.1 per Jonas' decision: a) `required|…` with null only "required" (like Laravel), case in validation_spec; b) `message` with "(and :count more errors)"; c) `DB::transaction($fn, 0)` like Laravel (closure does not run, null); d) broken `unserialize` throws (DIALECT §5.3), mark the probe as dialect-related; e) check `Storage::delete` on a missing file against Laravel/Flysystem in LV1, then align or record as a breaking change.
 3. Probes: recheck against PHP on every run, `probes_deviations.txt` (with DIALECT section; PHP deviates exactly there), regenerate the `.out` from PHP, recheck the 16 runtime gaps of the old tests and include them, resolve the contradiction `<?grd`/`<?php` in DECISIONS.
 4. JIT tests: assert a minimum number of translated units or tier 2; JIT probes also with GC threshold 1; differential test for overflow-free programs additionally against PHP; stress and budget test so that they can fail.
 5. Rebuild non-failing tests (prefix probe, preflight with exact messages, concurrency with conflict counters and final state, objects of both workers).
 6. Cheap independent oracles: real `INFO FOR` responses, recheck inheritance messages and validation_spec, Laravel messages against `lang/en`, cron against `Scheduling\Event`, `parse_str`/`Str::studly`/`singular`, `api.sh --app laravel` in the suite when the environment is present, DB tests with the fixture schema applied.
 7. Close gaps: GRD016/017, GRD102–105, forbidden builtins §4.2, module dependency order, token expiry and several tokens, validation with wildcards and the `required_if` family, job `backoff`, `verify` "missing" and name protection.
 8. Then the TEST1 remainder: coverage with cargo-llvm-cov, mutation tests with cargo-mutants on the core modules.
 Report at the end: per item done/open, new share with independent oracle, CI status. **Then empty the Cargo cache** (rebuild target). All before UI1, without check-ins.
Consequences: order DBP1 → follow-ups → TEST2 (incl. TEST1 remainder) → PERF3 → LV2 → JITD1 → MERGE1 → empty cache → report before UI1. The previous row TEST1 merges into TEST2.
 - [11:08 UTC] **DBP1 completed** (archiv/BENCHMARKS_bis_2026-10-26.md §4.w): 73 % of a show request lies in the SurrealDB engine; the DB runtime with 2 threads is not a bottleneck; the SDK has no prepared queries. Measures M1–M5 each with BM2 before/after. **M2 is a deliberate deviation from Sanctum** (`last_used_at` at most every 60 s, configurable, 0 = like Sanctum), documented in LARAVEL_BREAKING_CHANGES §6; durability unchanged. Settled for LV2: measure OpenGrid with both settings, Laravel gets the equivalent index.

## 2026-10-26 — D17: Transparency of the tests (Tools.md), TEST1 before PERF3, autonomous until before UI1 (Jonas)
Decision by Jonas, status [final]:
 1. **Pause for `.agents/Tools.md`** (before regression, DBP1 etc.): every test, every probe, every tool with subject under test, construction, origin of the expectation (independent oracle or own expectation), start and runtime; glossary; flow; peculiarities only noted, not fixed. Only code change: inventory script `tools/tests_inventory.py`.
 2. **Rule from now on** (also in CLAUDE.md): every new package records its tests and tools in Tools.md; a test without an entry counts as not finished.
 3. **TEST1** (review of test coverage and quality) follows after DBP1 and before PERF3; Tools.md is its step 1, §4 its work list.
 4. Jonas (message during the work): "when that is finished, feel free to continue directly … you can continue up to but excluding UI1 without check-ins." → order REG1 → DBP1 → follow-ups → TEST1 → PERF3 → LV2 → JITD1 → MERGE1 without interim reports; report before UI1.
Implementation (Claude): Tools.md with 174 entries (168 test functions, 6 data groups), inventory without gaps. Descriptions drafted with five reading agents, cross-read by the author; findings verified with PHP/Laravel (254/258 probes equal to PHP, three contradictions to Laravel found, §4.1).
 - [10:07 UTC] **REG1 completed: no code regression.** Calibration of the bisection showed d12a26b = 98f6680 (show 1480/1498, create 1004/1012); with the database in RAM equal as well (3821/3518 against 3839/3538). Cause of the lower numbers of 2026-10-26: write latency of the disk in the container (at the same time Laravel FPM on SQLite ≈ 15 % slower). The bisection was thus unnecessary. Consequence: the regression guard measures disk and RAM separately; BM2 numbers with embedded DB depend on the disk and are comparable only with the storage stated. Finding for DBP1: in RAM OpenGrid reaches ≈ 2.6× (show) and 3.5× (create) the disk value; the profile must separate writing and computing.

## 2026-10-26 — D16: Regression first, measure the database path, time before instructions (Jonas, 09:00 UTC)
Context: full measurement 2886d27 (archiv/BENCHMARKS_bis_2026-10-26.md §3, §4.y): OpenGrid embedded ahead of Octane, but DB endpoints ≈ 30 % slower than at d12a26b.
Decision by Jonas, status [final]:
 1. **Regression first:** `git bisect` between d12a26b (good) and 98f6680 (bad) with BM2 show/create embedded, at least three runs per step, fixed threshold; name the cause and fix it or leave it with justification, prove with measurement, close in KNOWN_ISSUES. **Regression guard:** after every package a short BM2 run (show, create, embedded); more than 10 % deviation from the last state is reported before continuing.
 2. **DBP1** profile the database path (measure first, then build): time shares of show/list/create (embedded and server), check the Tokio runtime `grd-db` with two threads as bottleneck, parsed queries/prepared statements, value conversion, thread hand-over; report, then one commit per measure with BM2 before/after. Do not weaken durability (surrealkv sync settings only with Jonas).
 3. **Decisions:** DISP1 without `unsafe` (D6 stays), instead check lengths once per frame and `u8` indices into fields with 256 entries, only on a gain above the noise. JIT2b phases 2/3 and JIT6 **deferred instead of dropped**, design `.agents/JIT_OBJECTS.md` (translatable without speculation, exits without deopt, what really needs deopt, hot reload, expected share, effort/risk). **Principle: decide measures by measured time, instruction counts only as a hint** — also retroactively for what was dropped by instructions (FRAME1 remainder → PERF3).
 4. **Follow-ups:** comparison A per phase against the target ≤ 2× (reached/open, with and without PGO); enlarge c2 so that PHP needs ≥ 200 ms; label Laravel BM2 on SQLite (to the archive after LV2).
 5. **PERF3** interpreter (after DBP1): calls (re-evaluate the FRAME1 remainder by time, prototype), PHP arrays (hasher, packed lists, hash in the string), report stack vs. register bytecode (report only), DISP1 checks; one measure per commit, time measurement per phase, revert without gain.
 6. **LV2** comparison of the standard setups: OpenGrid embedded against Laravel with PostgreSQL (own container, TCP), FPM+nginx and Octane; migrations users, personal_access_tokens, notes (soft delete), indexes; `api.sh --app laravel` green before the measurement; same workers, cores separated.
 7. **Merge to main:** tag on today's state of main, pull request vm-core → main with summary; no merge without Jonas' approval.
 8. **Order:** regression → DBP1 → follow-ups (parallel to the measurements) → PERF3 → LV2 and BM2 anew → JIT_OBJECTS.md → pull request (to be prepared in parallel) → UI1.
Consequences: ROADMAP §1 new; rows REG1 (regression), DBP1, PERF3, LV2, JITD1 (design JIT_OBJECTS.md), MERGE1 new; JIT2b remainder and JIT6 from "dropped" to "deferred"; FRAME1 remainder goes into PERF3; DISP1 into PERF3 d).

## 2026-10-25 — D15: Fundamentals before integration, J4 like Laravel, showcase with Inertia, D14 autonomous (Jonas, 19:54 UTC)
Decision by Jonas, status [final]:
 1. **PL1 deferred:** "the focus should be on the fundamentals and not on the later integration of my project."
 2. **J4 like Laravel:** durable jobs with their own table whose status can be tracked (Laravel: `jobs`, `failed_jobs`); no longer dependent on usage figures from the private reference implementation.
 3. **Showcase after performance:** when optimizations and benchmarks are good, Vue, Tailwind and Inertia into the example project (new UI1; replaces "no frontend for now").
 4. **D14 list steps 1–8 autonomously without questions;** an optimization with much complexity and comparatively little benefit is dropped and reported after all steps are completed. Commit and push to `origin vm-core` are part of the flow (no individual approval needed any more).
Consequences: ROADMAP §1 item 4 (UI1), J4 row new, PL1 to §4, §3 without deliveries from the private reference implementation.

## 2026-10-25 — D14: Performance by benefit per effort; gaps of the roadmap as packages (Jonas, 19:30 UTC)
Context: measurement c7c0863 (BENCHMARKS §2, §3): A 3.5× / 2.6× / 3.9×, C 4.1× / 2.6× / 2.8×, unchanged against 671da2a. Analysis of the open performance packages: comparison A is only moved by the FRAME1 remainder any more; JIT2b only acts in C; for `float_math` (A 5.2×, B 13.4×), the residual time of c2 (90 ms against 5 ms) and the dispatch of the fast loop there was no package; PGO1 is the largest measured lever (BM2 +6 to +36 %), but stood behind JIT2b/JIT6.
Decision by Jonas, status [final]: "it is important to me to create the greatest benefit with as little work as possible, so that the benchmarks look good on paper. so please include the gaps you named in the roadmap."
Implementation (Claude):
 1. **New packages:** REST1 (attribute the residual time of c2 in the profile and fix it), OPT1 (bytecode optimizer with static types from GRD021: fold constants, typed arithmetic ops without type branch), DISP1 (remove bounds checks of the dispatch loop in safe Rust; `unsafe` in grd-vm remains forbidden per D6, an exception would be Jonas' own decision).
 2. **Performance order by benefit per effort:** PGO1 → REST1 → OPT1 → JIT2b → FRAME1 remainder → DISP1 → JIT6 → PERF2 remainder. PGO1 first, because script (`bench/pgo.sh`) and measurement are available; REST1 is a profile with probably one cause (like mimalloc for c1); OPT1 hits the phase with the largest gap in B. JIT2b remains approved but moves behind the cheaper packages. FRAME1 remainder after JIT2b, because part 5 showed that the rebuild is expensive and the gain uncertain.
 3. **Measurement rule, so that the numbers on paper also hold in operation:** no measure that only takes effect in c1–c3 or BM2 (no special cases for benchmark patterns). PGO training never with the measured load itself: `grd` with suite probes, Support specification and `api.sh`, `opengrid` with `api.sh`; BM2 and c1–c3 measure. Benchmarks report the PGO binary as its own row next to the profiling build.
Consequences: ROADMAP §1 performance block re-sorted, rows REST1, OPT1, DISP1 new, PGO1 without dependency on PERF2, JIT2b status row in §1 corrected (was "waiting for Jonas").
 - [2026-10-26 05:41 UTC] **J4 implemented** (2886d27): tables `jobs`/`failed_jobs` like Laravel, but with fields instead of a JSON `payload` (SurrealDB can do objects, queries by class become possible). Pick-up via `UPDATE … WHERE reserved_at = NONE` (exactly once without an own lock). Only the server runs jobs (`start_runner` in `before_run`); task pools only write rows, otherwise a CLI task would execute jobs on the side or release the server's reservations. Poll every second instead of notification only, because a `dispatch` in a transaction is only visible after the commit, and delayed jobs and retries need a clock anyway.
 - [21:33 UTC] **D14 list completed, cuts per D15 item 4:** done PGO1 (cde49ec), REST1 (72af2dd), OPT1 (98f6680), JIT2b phase 1 (1a6db20), PERF2 remainder through PGO1. Dropped: JIT2b phases 2/3 (objects would need deoptimization, which D8 rules out; benefit only in C), FRAME1 remainder (gain below the noise between builds), DISP1 (not feasible in safe Rust; the option `unsafe` in grd-vm lies with Jonas), JIT6 (hardly any reach without objects in the JIT). Reasons in ROADMAP §4. Measurement cde49ec: A 3.0/2.7/3.1×, with PGO 2.6/2.1/2.9×; C 3.5/2.8/2.5×, with PGO 3.1/2.2/2.2× (c1/c2/c3). Next per D15: J4 (job tables like Laravel), then UI1.
 - [20:48 UTC] **REST1, OPT1, cache key** (72af2dd, 05b9a04, 98f6680): the residual time of c2 was for the most part the creation of the row literals (13 ops per row in the slow path); array literals with constant keys are now built from a key template, lists with one op. OPT1 implemented as a peephole optimizer at emit time (never across jump targets): conditions as jumps, fused stack operands; the rest of OPT1 (typed ops, constant folding) dropped because the benefit is small (D15 item 4). Finding on the side: since JIT2b the profile was part of the key of the process-wide tier-2 cache; workers therefore did not reliably find each other's code (the test only flickered with parallel tests). Element kinds are checked at runtime, they do not belong in the key.
 - [19:43 UTC] **JIT2b phase 1 (arrays reading) was already in progress when D14 arrived and is completed** (1a6db20); the order per D14 applies from now on, next package PGO1. Settled along the way: array elements have no static type in the dialect; the JIT types a read access with the **one** element kind the interpreter has seen at this place (profile) and leaves the translated code before the op on any other kind. That is a value check with error path like range checks (D8 item 1), not a type guard on local variables. **Measurement finding:** between two builds of the profile `profiling` (thin LTO, 16 codegen units) the instruction count of c3 in the interpreter fluctuates by about ±2 %, even if the dispatch loop does not change (control build without the new profile line: +1.8 %). Thus the revert of FRAME1 part 5 (c3 +1.9 %) was also within this noise; the revert stays (no proven gain), but Ir comparisons below about 2 % count in the future only with repetition over two builds or with the release/PGO build (PGO1).
 - [19:45 UTC] **Jonas confirms item 3:** "it should not only look good on paper. the numbers should be meaningful and that only works if the measurements are honest." The measurement rule is thus [final].
 - [19:45 UTC] **`.agents/` tidied up, archive instead of deletion** (Jonas: "that an archive is better"): completed tasks, upstream docs of the fork, inventory of the old core and the cut-out parts of BENCHMARKS, ROADMAP, STATUS and KNOWN_ISSUES lie under `.agents/archiv/` (overview `archiv/README.md`). Content is not lost; active documents only contain what is needed for the state and the next steps.

## 2026-10-25 — D13: Laravel feel instead of Laravel compatibility, objects live one request, robust VM, JIT against friction (Jonas, 12:20 UTC)
Context: feedback on AU2 and the list of open optimizations.
Decision by Jonas, status [final]:
 1. **No TrustProxies.** The upstream proxy will later pass the client IP through a header; `Request::ip()` then reads this header (name settled with the proxy). Until then the peer address.
 2. **No complete Laravel compatibility.** Laravel's look and feel stays (controllers, models, facades, routes, collections), but Eloquent **relations in the model are dropped** (model: the SurrealModel of a private reference implementation; relationships via record links, `relate()`/`related()`, `with*` sub-selects). **Every deviation from Laravel is documented as a breaking change** (own document).
 3. **Base classes after Loco I decide myself.** Where Loco functions need a different syntax than Laravel, it may be added; the Laravel feel remains the yardstick.
 4. **Object lifetime ends with the request as a rule.** Exception: objects can be coupled to a session via caching and thus outlive the request.
 5. **VM robust:** 90 % of use cases run without problems and without runtime errors. For that, check the test suite for missing cases; **the strict type check is firmly anchored in the suite.**
 6. **JIT analysis with the goal of minimizing the friction loss between dialect and Rust framework.**
 7. Analyze first, then revise the roadmap and sort by importance: **stability first, then performance**. Result to Jonas.
Consequences: Q1 shrinks to the builder part without relations and is approved (Jonas: "agree with all further points"). Analysis in ANALYSE_D13.md, roadmap §1 re-sorted, new packages TYP1–TYP3, MEM1a/b, BI1, BC1, SES1, IP1, CALL1, HASH1, PROPW1, FRAME1, JIT6.
My rulings on this (Claude):
 - [12:37 UTC] **Base classes: Laravel names, Loco as the engine** (ANALYSE_D13 §D1): `Mailable`/`Mail` on the Loco mailer (Tera templates instead of Blade), `Storage` on Loco storage (the only file access), `Cache` on Loco cache (in-memory), `Console\Task` with `$signature` on Loco `Task`, `Queue\Job::dispatch()` on our own in-process worker (not Loco's Redis/PG queue, D5), scheduler in `routes/console.grd`. Source: Loco's `src/{mailer,storage,cache,task.rs}`.
 - [19:11 UTC] **Jonas: JIT2b approved; IP1 via Traefik** ("I use the standard Traefik proxy and therefore also the header that is used for it"). IP1 (c7c0863): last entry of `X-Forwarded-For`, because Traefik appends the address it saw and replaces Forwarded headers from untrusted senders; without a proxy it can be switched off via `ip_header: false`. vm-core pushed up to 839f5d1.
 - [18:59 UTC] **M5 deferred, no Polars** (Jonas: "but I do not want to include Polars at this point, because data science is currently not part of the goal."). The started design (`OpenGrid\Data\DataFrame` behind feature `data`) was discarded completely, nothing committed, Polars not in Cargo.lock.
 - [18:40 UTC] **FRAME1 part 5 reverted** (839f5d1): calls in `run_frames` instead of via `exec` cost the same per call (≈ 420 Ir), but make the dispatch loop more expensive for all other ops (c3 +1.9 % Ir, outlined +2.3 %), c1 only −1.2 %. Per D11 no phase may get worse. Consequence: further gains at calls come not from the place of handling but from the frame itself (slots without `Option`, smaller `Frame`); that is a rebuild and is not done on the side. New arms in the fast loop only with a Callgrind comparison of c1 **and** c3.
 - [18:00 UTC] **PKG1: Git only, Composer part removed** (58ecc12): `crates/pkg` is now a library without its own binary; modules come via `opengrid task module cmd:add url:<git> rev:<tag|branch|commit>`, pinned in `modules.lock` (source, commit, content checksum sha256). Git via the CLI instead of `git2`/`gix` (no new large dependency; Git is on every development machine). A registry is deferred until there are more than a handful of modules. After `add` the whole project is compiled and linked with all framework preludes (without DB); if that fails, the module is removed again. The module manifests stay optional (MOD1, 2b5e36d): whoever writes none gets no boundary check, Git modules need one.
 - [17:26 UTC] **New PGO1 (PGO in the release build of `opengrid`)**: measurement 90e328c shows +6 to +36 % requests/s in BM2, more than any single measure in the interpreter. PGO for `grd` alone trained with probes, by contrast, brings almost nothing (d1b2b6a): the training must be framework traffic. PGO1 stands in the performance block after JIT2b/JIT6; the release pipeline itself does not exist yet, hence no fixed date.
 - [15:10 UTC] **JIT6 behind JIT2b, JIT5 before it** (measurement after JIT2a, comparison C, BENCHMARKS §2): with JIT, grd in c3 is faster than PHP's tracing JIT at calls and recursion (0.6×), close at integer/float loops (1.5×/1.3×); the whole rest of the gap lies in `objects` (7.7×) and `arrays` (10.5×), i.e. in reference types. A typed native interface (JIT6) without reference types reaches only scalar math functions and hardly any framework code; it therefore follows JIT2b. JIT5 (lifetime of the machine code, stress test) comes before, because it is a prerequisite for the JIT in the framework workers. JIT2b remains Jonas' decision, now with measurements.
 - [13:00 UTC] **Jonas: "continue with the VM and then the framework performance via the JIT and general performance and simply follow the roadmap."** Hence: stability (VM) → performance (CALL1 … JIT6) → function (Q1, MS1, J1–J4, MOD1, PKG1, SES1, IP1, M5). Replaces the following ruling.
 - [12:37 UTC, replaced 13:00] **"Function" between stability and performance:** D13 only names stability before performance; the function packages (Q1, MS1, J1–J4, MOD1, PKG1) stay before performance as in D12, because they do not touch stability and the performance packages are better measurable on a complete call picture.
 - [12:37 UTC] **Limit memory in the request first, then collect** (MEM1a before MEM1b): a hard limit immediately prevents the process OOM; the mark-sweep needs the `trace` hook for native payloads and is larger.

## 2026-10-25 — D12: Four phases: right first, then complete, then background, then fast (Jonas, 09:05 UTC)
Context: retrospective after D11; principle "make it work, make it right, make it fast" (D11, 08:15).
Decision by Jonas, status [final]:
 1. **Phase 1 – make it right:** B2c (server tests against the container server) and secure write conflicts in transactions; R1–R3 (php-src probes, Laravel docs, Laravel as behavioral specification) against semantic drift; MW1 (mark project aliases for middleware as deprecated).
 2. **Phase 2 – feature completeness and DX:** S1b (Str, Arr, Collection complete), AU2 (rate limiting login), MS1 (mail, storage), MOD1 and PKG1 (module manifest, module manager).
 3. **Phase 3 – background processing:** J1–J4 (control socket, jobs, scheduler, job table) in the server process.
 4. **Phase 4 – performance and JIT:** JIT2a and JIT5 (strict types on the basis of GRD021 and the verified server mode; memory limits of the extended JIT), M5 (Polars), resume PERF2 (PGO, code layout, property inline caches).
Consequences: ROADMAP §1 follows these phases. JIT2a moves from "depends only on B2c" (D10) to phase 4. The proposals from the retrospective (object lifetime, `last_used_at`, DB round trips per request) are not packages of their own; they are checked as findings in phase 1, architecture questions go to Jonas.
My rulings on this (Claude, each with timestamp):
 - [09:05 UTC] **B2c without a host server:** the server tests start a throwaway server themselves in the suite with the `surreal` binary (3.3.4, like the SDK) on a free port, password random and only via the environment; if the binary is missing, the test is skipped with a note (as before without `OPENGRID_TEST_DB_URL`, which continues to take precedence).
 - [09:09 UTC] **Jonas on PERF2 (phase 4): gaps against PHP are implementation and design questions, not a superiority of the Zend Engine; solution purely native in Rust.** (a) **Property inline caches** analogous to the method caches, to speed up property accesses (c3 `objects` 8×, in the profile SipHash in `set_prop_raw`/`lookup_prop`); (b) **optimized call frames and recursion**: stricter memory layout (flat memory, arena allocation) for better cache locality (c1 `recursion` 8×, call path ≈ 8 % of c1). Both items are the first measures when PERF2 is resumed in phase 4; the order of the phases (D12) stays.
 - [09:09 UTC] **TX1 after Laravel's model:** `DB::transaction($fn, $attempts = 1)` with the same meaning as Laravel's `$attempts` (default no retry); retried only on conflicts that SurrealDB reports as retryable (c8b8043).
 - [10:09 UTC] **Jonas: continue working independently, record all peculiarities and problems; take along optimizations I encounter right away, especially in the VM and in the Cranelift JIT.** Implementation: findings go into KNOWN_ISSUES (errors) or into a list "peculiarities" in STATUS; optimizations continue per the PERF2 rule (one measure per commit, measurement with Callgrind `Ir` and alternating wall-clock series, probes green; for the JIT additionally the differential test). This changes nothing about the phase order (D12) but allows optimizations on the side.
Status: [final]

## 2026-10-25 — D11: Benchmark method A/B/C, framework benchmark BM2, Laravel comparison LV1 (Jonas, 06:20 UTC)
Context: Jonas ran `api.sh` himself (43/43 green). The benchmarks shall remain comparable; in addition a framework benchmark over HTTP and a Laravel replica of TP5.
Decisions by Jonas, status [final]:
 1. **Three comparisons, always reported separately:** **A** (main comparison) PHP CLI without opcache and without JIT (`-d opcache.enable_cli=0`) against grd without JIT (`GRD_JIT=0`), without cached bytecode, translation time separate on both sides; **B** PHP with opcache without JIT against grd without JIT; **C** PHP with opcache JIT against grd with JIT. For all: same machine, PHP version noted, no Xdebug, `taskset` on fixed cores, median of at least five runs with spread, same checksums; every table names commit, date and comparison.
 2. **New baseline measurement** of all phases of c1–c3 per this method, A first; older numbers to the archive, do not mix.
 3. **BM2 framework benchmark** over HTTP with a load tool: `GET /health`, `GET /api/notes` with filter and sort, `GET /api/notes/{id}`, `POST /api/notes` with token, login separately (bcrypt, same cost). Data beforehand via the API, same amount. Requests/s, p50, p99 at 1/16/64 connections, warm-up phase, same worker count on the same cores. OpenGrid against the same SurrealDB server as Laravel (ws://), embedded as an additional row.
 4. **LV1 Laravel comparison application:** TP5 in Laravel with Sanctum, same responses; Laravel version as pinned by a private reference implementation (not part of this repository), otherwise the current stable; database via that implementation's SurrealDB layer (`App\Database`). **Confidential:** the app does not lie in the repo but under `bench/laravel-app/` next to it; a setup script in the repo generates it, installs the dependencies and copies the private layer in. No private code in commits. Runtimes PHP-FPM + nginx and Octane. `api.sh` selects the application via parameter; OpenGrid-only steps are reported as skipped for Laravel.
 5. **Order:** a) method and baseline measurement, b) LV1 and BM2, c) PERF2 (profile first, one measure per commit, measurement per phase; microbenchmarks after every measure, BM2 at start and end).
My rulings on this (Claude, each with timestamp):
 - [06:25 UTC] **Benchmarks into the repo:** the scripts c1–c3 (our own code, no private code) and the new measurement tool now lie in `phprs/bench/complex/` (`bench.py`), BM2 in `phprs/bench/framework/`. The old tools under `.agents/feasibility/bench/complex/` stay for the archive but are no longer used.
 - [06:25 UTC] **Measure translation time:** without opcache PHP has no "translate only" function except `php -l`, which translates the file completely (checked: compile errors like duplicate class constants are reported by `-l`). Measured is therefore on both sides a batch of 100 copies of the file (`php -l` or `grd compile`, both take several files) minus a batch of 100 empty files, divided by 100. A single measurement lay below the spread of the process start. The translation of the grd preludes (framework classes, `OpenGrid\Support`) happens once per process and is part of the "process start" (empty script), as with PHP the initialization of the extensions.
 - [06:25 UTC] **Wall clock and residual time:** besides the phases (from the scripts) the tool measures the wall clock per run and reports the residual time (wall clock − phases − start − translation: generating data, output, teardown). The runs of the variants alternate (against drift), 7 runs, cores 8–11, σ as relative standard deviation.
 - [06:25 UTC] **B and C without separate translation time:** with opcache `php -l` on 100 copies of the same file would hit the cache. In the CLI opcache keeps the bytecode only in process memory, so B and C also translate on every run; the time is part of the wall clock there.
 - [06:25 UTC] **Governor `powersave`** cannot be changed in the container; noise remains (σ mostly under 5 %), stated in every header.
 - [06:45 UTC] **LV1: the database layer of the private reference implementation is not yet available.** Only its schema tooling (migrate commands, `Migration/*`, `.surql` files) is present. Missing are: the model base class, the connector (or the connection/grammar/processor classes, if the layer is built as an Eloquent connection), the query builder, filter compiler, aggregates and side tables and, if present, the model's concerns/traits, a dedicated `PersonalAccessToken` model for Sanctum, the service provider that registers the connection, the database configuration (entry of the connection) — moreover the dependency lock file (Laravel version, SurrealDB PHP SDK with version). Searched everywhere, present nowhere. Until the files arrive: **preliminary stage of LV1 on SQLite** (Eloquent, embedded file, Sanctum unchanged). Everything else (controllers, routes, Sanctum, response formats, FPM/nginx, Octane, `api.sh`, BM2 tool) is created now and stays; when the layer arrives, only the base classes of the models and the connection are swapped. BM2 numbers with Laravel on SQLite stand in BENCHMARKS.md only as "preliminary stage, not the baseline measurement". Laravel version in the absence of a lock file: 13.38.0 (current stable).
 - [06:45 UTC] **Split repo / outside:** into the repo (`phprs/bench/framework/`) goes only our own code: setup script, start script, the Laravel application files (controllers, models, routes, configuration — written by me, no private code) as an overlay and the BM2 tool. The setup script generates `bench/laravel-app/` next to the repository (outside every repo), installs via Composer and copies the private database layer in as soon as it is available. That `bench/` folder lies outside `phprs` and is never committed.
 - [06:45 UTC] **Load tool oha** (1.16.0, `cargo binstall`), not wrk: oha outputs p50/p99 and requests/s directly as JSON, can do POST with header and body without a Lua script and runs with a fixed connection count (`-c`); wrk would need its own Lua scripts per endpoint for token, JSON body and evaluation. Warm-up phase as a separate oha run before every measurement.
 - [06:45 UTC] **Octane with RoadRunner**, not Swoole or FrankenPHP: RoadRunner starts the workers with the same `php` binary (8.4.21, same extensions and ini as FPM and the script benchmarks), needs no self-compiled C extension (Swoole is not available as a package for Fedora; a pecl build against the headers moreover changes the runtime with coroutine hooks) and brings no PHP version of its own (FrankenPHP embeds its own PHP, so the PHP version would no longer be the same). Swoole is often somewhat faster in others' measurements; that is noted as a limit of the measurement.
 - [06:45 UTC] **SurrealDB server for BM2:** `surreal` 3.3.4 (like the Rust SDK in `Cargo.lock`), official binary from download.surrealdb.com, SHA-256 checked, in `~/.local/bin`; only the binary, no source code (D4).
 - [09:00 UTC] **Jonas: PILOT1 is the test project** `phprs/examples/test-app` (TP5), no separate additional package. The row PILOT1 counts as done with TP5; the missing sections of the message of 2026-10-24 are no longer expected.
 - [08:40 UTC] **Jonas: Laravel on SQLite is fine as a comparison** and rather more meaningful, because SQLite is faster than external databases; a small advantage for SQLite over SurrealDB is negligible. The BM2 runs of 2026-10-25 (start of PERF2: 779d39d/0ecc358, end: d12a26b) thus count as the baseline measurement of BM2; LV1 is accepted with SQLite. The private `App\Database` layer remains optional (setup.sh copies it if present).
 - [08:15 UTC] **Jonas: "make it work, make it right, make it fast", in this order; tiers like V8/JVM/PyPy** (tier 1 clean VM with correct semantics, stability, exceptions, type checking; tier 2 JIT only on stable code for the hot paths). Implementation: PERF2 sticks to measures that do not touch semantics and are proven with probes/suite (six so far, one commit each); every new fast shortcut falls back to the previous path, which also throws the errors (example `intdiv`). Unproven micro-interventions are omitted: a hasher change for the name maps (08:10) was discarded. PERF2 pauses after measure 6 with a BM2 final measurement; correctness findings take precedence (so far in D11: write conflicts as 500, validation messages, FPM measurement error) and the Laravel baseline measurement as soon as the private database layer is available. The JIT stays as in D9 (tier 2, only behind switches, differential test).

## 2026-10-24 — D10: Task "base application up to TP5" (Jonas, 21:42 UTC)
Context: task of the evening of 2026-10-24, replaces the two previous messages. Independent run a)–i) up to the acceptance of TP5.
Decisions by Jonas, status [final]:
 1. **STRICT1 decided, strict types:** GRD021 is enforced (a local variable keeps the type of its first assignment; a deviating assignment = compile error). First assignment `null` → type `?T` with the first non-null type, fixed at compile time; a further different type is GRD021; narrowing after `if ($x !== null)` and `?->` is part of it. No widening int → float for local variables, the error message names `0.0` or `(float)`; for parameters the widening stays. DX_COVERAGE.md is a measurement of the migration effort (per source hits for GRD021 and ArithmeticError with three typical examples each; update as soon as PL1 is available), no longer a basis for decisions.
 2. **Integer overflow throws `ArithmeticError`**, identically in interpreter and JIT, enforced. DIALECT §9; in R1 "dialect-related".
 3. **Seeds stay dropped**; the tests generate test data themselves (e.g. via the API).
 4. **Blueprint not now:** R4g is dropped, R4 is completed without R4g. `Schema::table`/`Schema::sql()` stay in the code, deferred, not developed further; new package BP1 "blueprint" under "Deferred", without a date; the SCHEMALESS question rests with BP1. Module schemas are written by hand in `.surql`.
 5. **Dependencies:** JIT2a depends only on B2c any more. New packages TP5 (base application with login, logout, CRUD via the API, automated curl test) and PILOT1. AU2 (rate limiting login) newly into the roadmap.
 6. **Order without asking back:** a) STRICT1, b) R4 without R4g, c) B2 without B2c (DateTime native with DateTimeInterface, lazy rows), d) V1, e) AU1, f) I1 (use in TP5), g) S1 core scope, h) OPS1 (`GET /health`), i) TP5.
My rulings on this (Claude, each with timestamp):
 - [21:42 UTC] The inserted message ends in the middle of TP5 ("report with the"); sections 4 (PILOT1) and 5 (rules for the independent run) are missing. Assumption: the previous rules apply (suite green before every commit, one topic per commit, decisions with timestamp here, no destructive actions, the rules for the lookup sources). PILOT1 goes into the roadmap as a row without content ("description missing"). The TP5 report contains at least: result of `api.sh` (twice), suite, list of endpoints with status, deviations, open points.
 - [21:50 UTC] **GRD021, implementation:** the compiler reports GRD021 where both types are statically known (literals, casts, arithmetic of known types, comparisons, concatenation, `new`, array literals, typed parameters, already typed variables). Where the type of an expression is not statically known (array access, call, property), the VM checks on write into the local variable and throws `TypeError` with GRD021 in the text; so the rule stays binding there too. Types: null, bool, int, float, string, array, object (classes are not distinguished). `null` may go into a variable only if its first assignment was `null`, is statically unknown (may yield `null`) or it is declared as `?T`/default `null`.
 - [22:20 UTC] **Correction GRD021 at runtime (found with the S1 specification test):** if the type of the first non-null assignment of a variable is statically unknown (array access, call, untyped parameter), the variable is statically `mixed` and accepts every value; the VM does not check it. Checked at runtime is only where the type is statically known and a later assignment has no static type (`$n = 0; $n = $row['x'];` → `$row['x']` must be int). Reason: "the type is fixed at compile time" – with an unknown type that is `mixed`; the first version rejected typical patterns (traversal `$current = $current[$segment]`, `$keys = (array) $keys` with `string|array`). For the JIT, `mixed` variables are not statically typed (guards stay there).
 - [21:50 UTC] **`foreach` and `catch` variables** are rebound per iteration and do not fall under GRD021 (otherwise every loop over mixed request or database rows would be an error). Likewise the compiler's helper slots.
 - [21:50 UTC] **Narrowing (`if ($x !== null)`, `?->`):** GRD021 checks assignments, not uses; uses of `?T` are not rejected, therefore narrowing needs no rule of its own (no false error after `!== null`). A use check (`?int` in arithmetic) would be a later package.
 - [21:50 UTC] **`/` always yields `float` in the strict dialect** (`10 / 2` also gives `5.0`); integer division with `intdiv()`. Reason: otherwise the type would depend on the value (`$r = $a / $b` sometimes int, sometimes float), which contradicts "the type is fixed at compile time" and would trigger GRD021 depending on the data. Consequence for the JIT: the mixed type "int or float" is gone. Marked for review by Jonas.
 - [21:50 UTC] **Overflow without a switch:** `GRD_STRICT` is gone, the strict dialect is the only one.
 - [21:42 UTC] **File name for hand-written module schemas: `modules/<module>/database/schema/schema.surql`.** It is read in the place of the `blueprint.surql` (per module alphabetically first `blueprint.surql`, then `schema.surql`), before all `manually.surql`. `blueprint.surql` stays readable so that modules from the private reference implementation and a later BP1 fit; `manually.surql` stays for manual adjustments from Surrealist.

## 2026-10-24 — D9: JIT conservative: switches, differential test, measurement before enforcing (Jonas)
Context: feedback on the JIT report. Tier 2 brought nothing measurable (c3 5.3× in both tiers), the lifetime of the machine code was not regulated, a differential test was missing, GRD021 and `ArithmeticError` were meant to be enforced without knowing the costs.
Decision:
 1. **Extended JIT paused:** background compiler, atomic swap and shared cache stay in the code but are behind `GRD_ADVANCED_JIT` (default off) and are not expanded further. Without the switch the JIT translates once synchronously with `opt_level speed` (state of JIT1b). Before an activation: (a) JIT2a shows a measured gain, (b) the release rule and cache limit are documented and proven by a stress test (JIT5). Then Jonas decides anew.
    Reason why the infrastructure stays: it is tested (probes with tier 2, asynchronous swap, differential test also in extended mode), costs nothing when off, and the halved pause in the VM thread becomes important as soon as JIT2b translates larger regions. Why tier 2 brings nothing today (JIT.md §7): the generated code calls no Rust helper functions per operation, the values are typed; but only under 3 % of the c3 runtime lies in translated code.
 2. **Framework workers:** JIT off by default, `OPENGRID_JIT=on` switches it on, `GRD_JIT=0` is the emergency switch and wins; the pool logs the state at start. On by default only after a green differential test and Jonas' approval. All switches are in JIT.md §1.
 3. **Differential test** is a condition for every further JIT step (built, green; JIT.md §6).
 4. **Measure dialect rules first, then enforce:** `grd check --report` counts the hits per GRD code and GRD021 in two variants (strict: type of the first assignment; tolerant: first assignment `null` yields `?T`). Result in DX_COVERAGE.md. GRD021 and `ArithmeticError` are enforced only when Jonas has seen the numbers; until then only counting or the switch `GRD_STRICT=1`.
 5. **GRD021 in detail** (Jonas, as a decision, not "during construction"): first assignment `null` → type `?T` with the first non-null type, error only at two different non-null types; no widening `int → float` for local variables (for parameters it stays); error message with a hint at `0.0` or `(float)`. `ArithmeticError` on overflow is in DIALECT §9 (migration) and counts in R1 as "dialect-related".
 6. **Order:** the database tasks (R4, B2) take precedence. JIT2a begins only when R4 and B2 are completed and the differential test is green; differential test and report mode are exempt (safeguarding and measurement). Time box: the JIT runs until the end of JIT2a; then Jonas decides by the measurements whether JIT2b follows. Goal for compute-intensive tasks: as close to PHP as possible, not faster.
 7. **Lifetime of the machine code:** tier-1 code is released with its VM (nothing from it runs before); tier-2 code is never released. Open until JIT5: upper limit for the shared cache and the number of tier-2 translations (hot reload constantly creates new keys).
Finding on D6: `#![forbid(unsafe_code)]` was missing in all crates although documented; there was no `unsafe` code outside `grd-jit`. Added for grd-vm, framework, pkg (1709030). `grd-jit` has 2 `unsafe` blocks (JIT.md §4).
Consequences: roadmap JIT2a (waits for R4/B2), JIT2b (decision Jonas), JIT2c (behind switch), JIT3 (opt-in), JIT4 differential test (done), JIT5 lifetime/stress test (open), DX1 (done), STRICT1 (enforcing, waits for Jonas).
Status: [final]

## 2026-10-24 — R4b2 and R4h: port byte-identical, then the findings of the original fixed one by one
Context: the sources of the schema tooling of the private reference implementation had been available since the morning of 2026-10-24; R4a/R4b2 were nevertheless listed as "waiting for Jonas" (error in the reconciliation, noted in the journal).
Decision:
 1. Golden tests (R4a): an own PHP driver loads the classes of the private reference implementation at runtime (nothing copied). 16 own edge cases are frozen with the PHP 8.4 output; the real schema files of the reference implementation are, if present, compared PHP against Rust — they are identical.
 2. Byte identity (R4b2), deviations from the assumptions of the R4b entry: the placeholder is `DEFINE TABLE <t> TYPE NORMAL SCHEMAFULL PERMISSIONS NONE`; an empty schema yields `\n`; the splitter is ported literally (in double quotes the backslash escapes; PHP `trim` without Unicode whitespace); classification with the same regular expressions; every file becomes its own schema and is then merged per `SurqlSchema::merge`; texts from `INFO FOR` remain unchanged.
 3. Findings of the original, one commit with test per finding (R4h), after which the port deliberately deviates (`expected.r4h.out` next to the frozen PHP output; the reference files remain identical):
    - a later placeholder replaced a table definition (a `manually.surql` with only one field made a SCHEMALESS table SCHEMAFULL) → the definition stays;
    - `array_merge` renumbered integer names and kept them twice → merge by name;
    - `IF NOT EXISTS`/`OVERWRITE` were read as a name (table `IF`, fields discarded) → modifiers;
    - only ASCII `\w` as name (`café` → `caf`, `address.city` discarded) → backtick, `⟨…⟩` and all non-whitespace names;
    - only `--` as comment → also `//`, `#`, `/* */`;
    - in single quotes `\'` ended the string → the backslash escapes;
    - `migrate_push` only named the number of discarded statements → lists each with file and reason.
 4. `Schema::sql` and the framework fragment write plain `DEFINE` statements (without `IF NOT EXISTS`/`OVERWRITE`) so that the reference tooling reads them correctly too; `->change()` no longer changes the statement (in the declarative files the later definition wins anyway).
Replaces: assumptions 1–6 of the entry "R4b: schema port from description".
Status: [final]

## 2026-10-24 — D7: Build resources and target size (Jonas, 5c)
Context: `target/` occupied 235 GB (`target/debug/deps` 221 GB, `incremental` 11 GB); every integration test binary had 2.1 GB because each linked the whole SurrealDB engine with full debuginfo. The build with `jobs = 8` had driven the host into OOM.
Decision: `[profile.dev|test] debug = "line-tables-only"`, `[profile.dev|test.package."*"] debug = false`; integration tests per crate in one binary `tests/suite.rs` (modules via `#[path]` from `tests/suite/`); `SCCACHE_CACHE_SIZE` or sccache configuration 20 GiB; `jobs = 2`, `taskset -c 0-7`; afterwards `cargo clean`. New integration tests come as a module into the existing suite, not as a new binary.
Measurements after the rebuild (2026-10-24, commit 5c774ee): largest test binary 0.27 GB (before 2.1 GB), `target/debug/deps` 2.9 GB (before 221 GB), full rebuild + tests ≈ 2 min 20 s with `jobs = 2`.
Status: [final]

## 2026-10-24 — B1 addendum: filter types at runtime from INFO FOR TABLE (5f)
The filter conversion today reads the field type at runtime from `INFO FOR TABLE` (cached per worker), because the merged target schema (`temporary.surql`) only exists with R4 and until then the database is the only source that knows all fields including manual adjustments; with R4 the type comes from the merged schema (schema decision item 8), `INFO FOR` remains the fallback for tables without a target schema.
Status: [final] as a transition until R4.

## 2026-10-24 — Roadmap as the single source for order and state (Jonas)
Context: order, state and dependencies were spread across ROADMAP, STATUS, DECISIONS and reports.
Decision: ROADMAP.md is the single source: work packages with fixed IDs, status, dependency, who, acceptance criterion, DECISIONS reference, date; sections "Waiting for Jonas", "Deferred and dropped", "Risks and assumptions", "Changes since the last revision". STATUS.md only refers to IDs any more. Order and scope change only with a DECISIONS entry. Every interim report names the changed roadmap rows. Partial states get their own rows.
Status: [final]

## 2026-10-24 — R4b: Schema port from description, assumptions until the golden acceptance
Context: R4b was to port the PHP classes of a private reference implementation (SurqlFileParser, SurqlSchema, SurqlGenerator, SurqlDbParser) faithfully. The sources were not yet available; Jonas approved R4b nevertheless.
Decision: port from the description in the task (section 8 and "R4 implementation"), modules `native/database/schema/{ast,parser,merge,generator,sources,dbinfo}.rs`, entry `schema::build_surql(root, framework)`. All layout decisions lie in `generator.rs`. Assumptions that R4a checks against the PHP output:
 1. Format after the pattern of the SurrealDB export: per table `-- ------------------------------` / `-- TABLE: <name>` / `-- ------------------------------`, blank line, DEFINE TABLE, then fields, indexes, events as groups each with a blank line before; blocks separated by two blank lines; one trailing newline; empty schema = empty text.
 2. Sorting: tables and within every group per PHP 8 `ksort` (SORT_REGULAR): integer keys numerically, numeric strings numerically, otherwise bytewise. Because this order is not transitive with mixed keys, an insertion sort sorts (Rust's `sort_by` could abort).
 3. Merging: DEFINE TABLE replaces the table definition, fields/indexes/events are preserved in the process; fields/indexes/events are replaced per name. Deviation from the presumed PHP behavior: `array_merge` would renumber purely numeric names and append instead of replace; the port always replaces by name. Finding for R4a.
 4. Placeholder for tables known only through fields/indexes/events: `DEFINE TABLE <t> SCHEMAFULL PERMISSIONS NONE` (like `ensureTable`); a later DEFINE TABLE replaces it.
 5. Comments are not carried over, multi-line statements keep their lines; names in backticks and without are the same key, in the text the spelling of the source stays. Names in `⟨…⟩` are not treated specially.
 6. Known gaps of the original stay for now (faithful): only `--` comments, only the doubled quote as escape, unsupported DEFINE kinds and non-DEFINE statements are discarded. Unlike the original, the port collects every discard as a warning (source + reason); the output does not change through that. The fixes follow as separate commits after R4a (roadmap R4h), so that the golden tests record the starting point first.
Evidence: 21 unit tests (splitter with `;` in strings and backticks, comments, multi-line statements, empty files, duplicate keys, order of the sources, ksort, placeholder, type query `none | int`) and the suite test `schema_files`: project with framework fragment, main, two modules, blueprint and manual adjustment → order and precedence are right, SurrealDB 3.3.4 (`mem://`) executes the generated file without errors.
Status: replaced by "R4b2 and R4h" (2026-10-24).

## 2026-10-24 — R4d: Schema at start only in production, and there only as planned (Jonas)
Context: the task (D4 step 3) provided: "The server start applies the schema." An automatic `sync` at start can change or remove objects unchecked in production.
Decision (Jonas): the schema alignment at server start runs **only in the production environment**, and there **not as `sync`**, but via SurrealKit's planned rollouts: changes are generated and reviewed beforehand with `rollout plan` as a manifest (`database/rollouts/*.toml`); the start only executes already planned, not yet run rollouts (library: `Rollouts::load(folder).up(db)`). Without a planned rollout the start changes nothing. In development and test there is no automatic alignment at start; `migrate_push` aligns on command (`sync`).
Open (question to Jonas): (1) Shall `up` at start also complete the newest rollout (`complete`), or does it stay `ready_to_complete` as in SurrealKit until the operator calls `rollout complete` after the cutover? Provisionally: SurrealKit default, the newest stays open. (2) Shall the start in production abort if the merged target schema deviates from the state after all rollouts (unplanned change)? Provisionally: warning in the log, no abort.
Consequences: roadmap R4d (start behavior), docs on `migrate_*`. The sentence "The server start applies the schema" from D4 step 3 is hereby replaced.
Status: [final] for production-only and planned rollouts; [provisional] for the two open points.

## 2026-10-24 — Rollouts additive only, with preflight check (Jonas)
Context: a rollout must not endanger data in production. SurrealKit by default plans additions **and** removals (step `remove_entities` in the phase `complete`); changes to existing objects only with `--allow-modified`.
Decision (Jonas): the schema is **only extended incrementally** via rollouts. No rollout may remove or redefine existing tables, fields, indexes or events. A preflight check verifies that before a rollout can run.
Implementation (`schema/preflight.rs`, commit ab0ee95):
 1. Manifest rules: `remove_entities` only in the phase `rollback` (dismantling an addition); `run_sql`, renames (`renames`) and unknown step kinds are rejected (fail closed); allowed are `apply_files`, `apply_schema`, `assert_sql`.
 2. Engine comparison: the frozen target schema of the rollout is applied in a throwaway database (`mem://`, same engine); every object from the catalog snapshot the rollout starts from must exist there with the same `INFO FOR` definition as in the live database. SurrealDB itself normalizes both sides; SurrealKit's hash logic is neither rebuilt nor copied.
 3. A new field on an existing table must be `option<…>`/`none | …` or have DEFAULT/VALUE (otherwise existing rows no longer satisfy the table). New UNIQUE indexes on existing tables are allowed but a warning (fail on duplicates).
 4. `kit::rollout_plan` never plans with `allow_modified` and checks immediately; an inadmissible plan is discarded (`run_discard`, snapshots reset). `kit::rollout_start` checks again before **every** start, so that manually edited or elsewhere planned manifests and frozen schemas are caught too. The production start (R4d) checks every pending rollout before `up`.
Evidence (suite test `preflight_allows_only_additions`): removing a table → rejected by manifest rule and engine comparison, plan discarded; retyping → SurrealKit does not plan without `allow_modified`; new required field → rejected; optional field + new table → passed; manually retyped frozen schema → start rejected ("field tasks.title exists and would change"); manually inserted `run_sql` step → start rejected, nothing applied.
Limit: the check applies to rollouts. `migrate_push` (sync, development only) continues to prune managed objects that disappear from the target schema — question to Jonas whether additive-only shall apply there too.
Consequences: roadmap R4i (done), R4d (production start checks before `up`). A data migration (rewrite data, remove a column) is thus not possible via rollouts; if an application needs one, that is a separate decision (open question "data migrations").
Status: [final]

## 2026-10-24 — Finding: surrealkv holds the file lock until the process ends
Probe (R4c test): after dropping client and Tokio runtime, the same `surrealkv://` file cannot be reopened in the same process even after 5 s ("LOCK is already locked by another process"). Consequence: a process owns an embedded database for its whole lifetime (fits D5). Reopening within the process (e.g. pool restart on hot reload) must not rebuild the client but keep it (as today). Tests check persistence via child processes. To be observed for DB1.
Status: finding

## 2026-10-24 — Production start: atomic or abort; deployment via Nomad canary (Jonas)
Context: in production, deployment is via Nomad with canary updates: the new version starts next to the old one; if it is not healthy, Nomad aborts the deployment and stays on or returns to the old version. The production start executes planned rollouts (entry "R4d: schema at start only in production").
Decision (Jonas):
 1. If the preflight check is negative or a step of the migration fails, **the whole server terminates** (exit code ≠ 0, no continuing with half a schema). For Nomad that is a failed canary.
 2. Before that the database is restored to the **last stable state**: changes of one start are atomic — all or nothing.
 3. Because rollouts are additive only, the extended schema is compatible with the old version; the old version keeps running unchanged during the canary.
Implementation (R4d, proposed): first check **all** pending rollouts via preflight — one negative → exit without any change. Then in order `start` + `complete`; if a step fails, `rollback` of the running rollout (SurrealKit removes its additions) and exit. [provisional] **Open:** rollouts that were already completed in the same start (`completed` is final in SurrealKit) cannot be rolled back with SurrealKit. Proposal: a production start processes at most one pending rollout per deployment and refuses (exit) on several; then every start is atomic. An empty new production database is the special case: there all rollouts run (there is no stable state before). Question to Jonas.
Consequences: roadmap R4d (acceptance: exit code ≠ 0 on preflight error and on migration error, database afterwards in the state before the start); new row OPS1 (health endpoint for Nomad checks). The earlier open question "complete the newest rollout at start?" is answered: yes, `start` + `complete`, because additive.
Status: [final] for exit and restore; [provisional] for the handling of several pending rollouts.

## 2026-10-24 — migrate_push/pull without checks; no data migrations (Jonas)
 1. `migrate_push`, `migrate_pull` (and `migrate_fresh`) are pure development tools; they need no preflight and may remove managed objects (sync with prune).
 2. Migrations only extend the schema. A data migration is a design error (schema not designed for extensibility); then the developers create the new schema by hand and copy the data over with their own script — not the framework's job.
Consequences: the open question "Are data migrations needed?" is answered: no. Thus the reason to keep the imperative migration runner is gone (schema decision item 7) → R4f removes it together with `_opengrid_migrations` and the tasks `migrate`/`migrate_status`/`migrate_rollback`, instead of only marking it as deprecated.
Status: [final]

## 2026-10-24 — Production start: fix forward instead of rolling back (Jonas)
Context: completed rollouts cannot be rolled back in SurrealKit; my proposal was "at most one pending rollout per deployment".
Decision (Jonas): a failure after a passed preflight is unlikely; completed rollouts are **not** rolled back (additive, does not disturb the old version), the developers push a corrected version with a new rollout. Several pending rollouts in one start are allowed.
R4d flow thus: check all pending rollouts via preflight (one negative → exit ≠ 0 without change); then in order `start` + `complete`; if a step fails, roll back only **this** rollout (SurrealKit allows only one open rollout, otherwise it blocks every further one) and exit ≠ 0. Replaces item 2 ("atomic") and the proposal "at most one" from the entry "Production start: atomic or abort".
Open (question to Jonas, my recommendation): for Nomad canary in production use the **server mode** (old and new version simultaneously on the same SurrealDB server; the embedded surrealkv file is locked until the process ends, a copy in the canary leads to diverging data). Optionally later a trial run of the rollouts against a backup copy as a CI step.
Status: [final] for "fix forward"; [open] database mode in production.

## 2026-10-24 — Database embedded in production too; no trial copy in the canary (Jonas)
Context: my recommendation "server mode for Nomad canary" (the embedded surrealkv file is locked until the process ends, old and new version cannot open it simultaneously) and the proposal "the canary rehearses on a copy, takes over the real file after v1 stops".
Decision (Jonas): the database stays **embedded** in the framework in production too. There is **no** trial copy of the data set in the canary (simpler, less confusion; can be added later). The production start works directly on the real file: preflight → rollouts → on error exit ≠ 0 (entry "fix forward").
Consequence for the deployment: because only one process can open the file, a new version only starts once the old one has released the lock (Nomad: stop the old allocation, then start the new one; `auto_revert` on an unhealthy start). The new version waits a bounded time at start for the free lock and terminates with a clear message if it does not become free. → roadmap DB1/OPS1.
Status: [final]; trial copy in the canary deferred.

## 2026-10-24 — R4 completed: fully SurrealKit, imperative runner removed (Jonas)
Decision (Jonas): switch completely to SurrealKit and test; remove the imperative mechanism completely. If the target schema deviates from the database state without a planned rollout, **nothing** happens (no warning, no abort).
Implemented (commit 9504393): removed are `migrator.rs`, the dialect class `Migration`, the tasks `migrate`/`migrate_status`/`migrate_rollback`, `_opengrid_migrations`, `Schema::create/table/drop/dropIfExists` and the `.grd` migrations of the fixture. `Schema::sql` (generator) and `hasTable/hasColumn/columns` (reading) stay. New: tasks `migrate_push`, `migrate_pull`, `migrate_fresh force:true [keep_db:true]`, `rollout cmd:…`; production start in `before_run` (only server/worker start, not for tasks); `framework.surql` with `_opengrid_sessions` (the session store no longer defines its table at runtime); `SURREAL_DB_*` → server mode with the server-mode defaults, `OPENGRID_DB_URL` takes precedence. The fixture uses `modules/tasks/database/schema/{blueprint,manually}.surql`.
Findings along the way: (1) A plan without change makes SurrealKit write an empty manifest (source = target hash, no steps, no frozen schema); the preflight read that as "everything removed". Empty plans are now discarded, a rollout without schema skips the engine comparison. (2) Loco's CLI builds the app context twice on `start` (logger, then `create_app`); thus the database was opened twice and surrealkv failed at its own lock. The pool is now started once per process. (3) Log messages with the target `grd` do not appear in the server log; Loco's logger filters on its own targets → OPS1.
Evidence end-to-end with the binary `opengrid`: push, plan two rollouts, production start on a fresh surrealkv database (both applied, server responds), start again (nothing pending), manipulated manifest (exit 1, nothing changed).
Status: [final]

## 2026-10-24 — Seeds dropped (Jonas)
Decision: seeders are not needed; roadmap DB2 is dropped. The entry D3 ("Seeds via SurrealKit") is thus void. The test project (TP2) does without start data or creates it via its own routes.
Status: [final]

## 2026-10-24 — DB1 implemented; RocksDB and TiKV not needed (Jonas)
Decision (Jonas): with surrealkv, RocksDB and TiKV are not needed; they are not built in. surrealkv is the embedded engine for development and production (before [provisional] in D2, now [final]).
Implemented (commit cebb7e2):
 1. Without a database setting: `surrealkv://<project>/.opengrid/db`, namespace and database `opengrid`. Relative `surrealkv://` paths are relative to the project directory. `.opengrid/` belongs in the project's `.gitignore` (TP1); the control socket (J1) will also live there later. `database/` remains SurrealKit's folder (schema, rollouts, snapshots).
 2. `Db::connect` waits for a database held by another process until `settings.opengrid.database.lock_wait` (default 30 s) expires; then exit with "database … is in use by another process (is the server still running?)". `opengrid task …` does not wait (the binary sets a task mode) and reports the lock immediately.
 3. Start log: mode (embedded/server), location, namespace/database, version; in server mode a warning on a major version differing from the built-in engine (compared via a `mem://` probe of the same engine).
 4. The VM pool starts on first use (`before_run` at server start, `run` for tasks) instead of in `after_context`: Loco calls `after_context` once before the logger setup; the database messages were lost there. Replaces the process-wide static variable from 9504393.
 5. Log target `opengrid` instead of `grd`, so that Loco's default filter (`<app_name>=<level>`) lets the messages through (OPS1 part done).
Evidence: suite `db_embedded` (default path, path resolution, takeover after release by a child process, abort with message after `lock_wait`); end-to-end with `opengrid` (default DB created, task with the server running, second server gives up after `lock_wait`, hand-over on stop of the first with HTTP 200).
Status: [final]

## 2026-10-24 — Performance goal: dialect as fast as PHP; two benchmarks (Jonas)
Context: the framework runs natively in Rust (queries already faster than PHP + ext-php-rs). For the dialect itself the goal is not "faster than PHP" but **"as fast as PHP"**; then the overall advantage is secured.
Decision: two separate benchmarks in `.agents/feasibility/bench/complex/` (one source per test, dialect version via `gen.sh`, checksum must be equal):
 1. `c2_runtime` — work in runtime functions where a JIT brings little (JSON, strings, regex, large arrays). Comparison **without JIT** on both sides: PHP interpreter against dialect interpreter. Goal PERF1: factor ≈ 1.
 2. `c3_compute` — work that a JIT accelerates (loops, int/float arithmetic, calls, recursion, objects, array indexing). Comparison **with JIT** on both sides: PHP opcache JIT against dialect with `grd-jit`. Goal JIT1: factor ≈ 1.
 `c1_request` (mixed request) stays as the overall picture.
Baseline (profile `profiling`, median of 3, ms): c2 PHP 181.6 / dialect 834.1 = 4.6× (json 2.7×, strings 4.8×, regex 3.6×, arrays 10.8×); c3 PHP JIT 176.7 / PHP without JIT 722.3 / dialect (interpreter) 6780.6 = 38.4× against PHP JIT, 9.4× against PHP without JIT (int_loops 33.5×, float_math 180×, calls 43×, recursion 33×, objects 13.5×, arrays 48×, each against PHP JIT).
Correction of my earlier statement "PHP's JIT brings only about 15 percent": that held for the mixed request c1. For pure computation (c3) PHP's JIT is 4.1× faster than its interpreter, for float arithmetic 14×.
Status: [final]

## 2026-10-26 11:28 UTC — TEST2.3: Probes begin with `<?grd`; PHP recheck on every run
Context: the entry "`<?grd` as opening tag" (2026-10-23) says the probes under `crates/vm/tests/probes/` keep `<?php`; the journal of the evening of 2026-10-23 says `.grd` files begin with `<?grd` and the harness gives PHP a `<?php` copy. The inventory had 94/164 mixed.
Decision (Claude, within D18 item 3): the later state applies. All probes begin with `<?grd`; `probes.rs::probes_are_php_output` generates the `<?php` copy (under `CARGO_TARGET_TMPDIR`) and runs PHP on it on every suite run. The `.out` must be PHP's output, except for the probes in `crates/vm/tests/probes_deviations.txt` (name, DIALECT section, reason), where PHP must deviate. `GRD_PROBES_WRITE=1` regenerates the remaining `.out` from PHP. The sentence in the entry of 2026-10-23 about the copies under `crates/vm/tests/probes/` is thus superseded; the probes under `.agents/feasibility/` keep `<?php`.
Status: [final]

## 2026-10-26 13:20 UTC — TEST2 completed (report per D18)
Per item done/open (commits on `vm-core`):
1. Commit flow: `tools/green.sh`, `tools/commit.sh` (since 2723ef8 tests exactly the staged tree), `.githooks/pre-commit`, CI `.github/workflows/suite.yml`, switches `OPENGRID_REQUIRE_ORACLES`/`OPENGRID_ORACLES_EXCEPT` (ce5dbcd). **CI status unconfirmed:** `gh` is not logged in in the container, the first run on GitHub cannot be viewed from here.
2. Deviations a–e done (1352194, a2f1926, 57739f9, 4e23616, e531675); `unserialize` did not check delimiters in the process (fixed).
3. Probes: recheck against PHP on every run, `probes_deviations.txt`, `GRD_PROBES_WRITE`, all `<?grd` (946812c); old tests rechecked, `mb_strwidth`/`filter_var` sanitize fixed (09a7834); SPL structures still missing.
4. JIT tests with minimum counts, GC variant, PHP as third oracle (29c9489). Found: JIT probe runs translated code in only 2 of 258 probes; the process-wide tier-2 cache never hit (selector order, fixed 17abd3b).
5. Non-failing tests rebuilt (0ddc794); via the server no write conflicts occur (there only the final state).
6. Cheap oracles (b67148b, 8a89d5a, 663cd0e, 25cce61); fixed in the process: `parse_str`/request parser against PHP, `singular` against Laravel, `everyThirtyMinutes`, `migrate_pull` with element fields; database tests with schema.
7. Gaps closed (7252f62, 2df043e, 0f59382); newly built: token expiry like Sanctum, validation with wildcards and the `required_*` family (27 cases, all equal to Laravel).
8. Coverage (llvm-cov): lines 78.8 %, regions 74.4 %, functions 71.5 %. Mutation tests (cargo-mutants) on `ops.rs`/`convert.rs`/`array.rs`: 391 mutants, catch rate 63 %; after the probe `operators_slow_paths` in `ops.rs` 95 → 27 missed (the rest almost only equivalent). Open: `convert.rs`, `array.rs`. Found: operand type message, `var_export(PHP_INT_MIN)`, Octane downloaded RoadRunner anew on every start (de03877, c76bea6, 85bf3b0).
Share of tests with an independent oracle (classification per the oracle rows in Tools.md §2, same script as for TOOLS1): before 23 of 174 (13 %), now 34 of 198 (17 %); with mixed forms 21 % → 26 %. More significant: probes (262), inheritance messages (136), validation cases (167), cron, query strings, inflection and 159 random programs are now recomputed against PHP/Laravel on every run instead of frozen once.
Open dialect questions for Jonas (KNOWN_ISSUES): lossy float→int conversion in `%`/bit ops and `abs(PHP_INT_MIN)`; `null--` without effect.
Status: [final]

## 2026-10-26 14:49 UTC — PERF3, LV2, JITD1, MERGE1, cache (conclusion before UI1)
- **PERF3** by time: adopted crc32 (7090652), JSON direct (a615763), `preg_replace`/`preg_split` without unnecessary groups (37e745e, c46f732), `memmem` (befdf36), int sorting (17e744f), callbacks via the direct-call path (a479f94, 6b24c99). **Reverted:** concatenation in the fast block (f34ef68 → 1825c1e), because c3 became 11 % slower; lesson: measure every change to `run_frames` against c1–c3. **Dropped** (little benefit): hash in the string, FRAME1 remainder, DISP1 in safe Rust, `sprintf`. c) as report (archiv/BENCHMARKS_bis_2026-10-26.md §3.3): no rebuild to register bytecode. Result 1825c1e: c2 A 2.5× → 1.9×, c1 3.1× → 2.9×, c3 unchanged. Fixed in the process: U+2028/2029 in `json_encode`, `preg_split` with empty matches, `PREG_SPLIT_OFFSET_CAPTURE`.
- **LV2** (e10940d, archiv/BENCHMARKS_bis_2026-10-26.md §4.v): Laravel on PostgreSQL 17 (Compose service `postgres`), own database per run, connection only via `PG*` variables. For PHP `pdo_pgsql` was missing: installed in the running container with `dnf install php-pgsql` and added to `.devpod/Containerfile` (survives the rebuild). Laravel migration with the indexes of the test app.
- **JITD1:** design `JIT_OBJECTS.md`; checkpoint P0 (prefix layout of the properties) met. Recommendation: steps 0–3 without changing D8, deopt only after measurement. Decision on building and D8 with Jonas.
- **MERGE1:** tag `main-before-vm-core` on 33f4900 pushed; PR text `MERGE1_PR.md`. The PR is not opened because `gh` is not logged in in the container; no merge without approval.
- **Cache:** `target/` (94 GB) emptied with `cargo clean`, `target-pgo/` (7 GB) deleted, sccache kept (does not influence the results, only speeds up the rebuild); suite rebuilt from scratch afterwards.
Status: [final]

## 2026-10-26 17:24 UTC — MERGE1 approved and executed (Jonas)
Jonas: "the git credentials are in the container, so please merge to main yourself." (Part 0 of his message applies; part 1 "I merge myself" is thus superseded.)
Executed: CI on `vm-core` (run 40342612117, 2063893) checked green; token only for the call from the Git credentials, not printed and not stored. `main` brought to 2063893 via `git merge --ff-only origin/vm-core` and pushed, no merge commit, commit IDs preserved. Tag `opengrid-0.1` on 2063893 pushed; `main-before-vm-core` stays.
Rule from now on: work happens on `vm-core`; `main` is only fast-forwarded after Jonas' approval.
Addendum: CI run on `main` (40368048629, 2063893) green.
New order (Jonas, parts 1/2 and 2/2): 0) merge and benchmark baseline → a) follow-ups (token usage without failure, mutation tests convert/array ≥ 80 %, PERF3 c) with profile, JIT probe count) → b) dialect decisions (`%`/bit ops only int, `abs(PHP_INT_MIN)` throws, `++`/`--` only int/float) → c) JITD2 (document) → d) UI1 → e) JIT_OBJECTS steps 0 and 1 after approval of JITD2.
Status: [final]

## 2026-10-26 18:35 UTC — D19: Feedback on the report before UI1, dialect decisions, JITD2 (Jonas)
Decision by Jonas, status [final]:
 1. Merge (part 0, executed, entry "MERGE1 approved") and benchmark baseline on the merged commit (BENCH0, BENCHMARKS §0).
 2. Follow-ups: a) writing `last_used_at` must never fail a request (retry a conflict once, otherwise ignore and log; test with concurrent requests of the same token; LV2 again, goal 0 error statuses) → TOK2. b) Mutation tests `convert.rs`/`array.rs` finished, goal for ops/convert/array/compare ≥ 80 % detected, every surviving mutation with a test or justification in Tools.md → MUT2. c) PERF3 c) as a report with profile: what the stack machine costs compared to a register form → PERF3c. d) Name the number of probes with translated code and assert it as a minimum → JITC1.
 3. Dialect (DIALECT §5.2, [final]) → DIA2: a) `%` and bit operations only with int; a float operand is a TypeError (hint at `fmod` or `(int)`). b) `abs(PHP_INT_MIN)` throws ArithmeticError like every overflow per D10. c) `++` and `--` only for int and float; null and strings are TypeError (no `"a"++` either). Per item tests, affected probes as dialect-related, BC1.
 4. JIT_OBJECTS: before building, package JITD2 (document only) with items a–h (gap between steps 2 and 3: nested interpreter or multi-frame hand-over with effort and consequences for D8; invariant per exit point; test mode `GRD_JIT_FORCE_EXIT` in the differential test; scalar properties without `Val`/reference counting; model attributes as a later step with share in TP5; check-then-act also for MEM1a, uninitialized typed and readonly properties; P1 by `class_alias` and dynamic `new`; abort criterion after step 1, proposal objects not better than 4.5× PHP JIT). Approval for steps 0 and 1 after agreement to JITD2; 2 and 3 only after a) and the measurement from h). Prerequisite per step: differential test with objects (typed properties, inheritance, overridden and non-overridden methods, exceptions in constructors), also with GC threshold 1, runtime functions under Miri. No new environment variable except the test mode.
 5. Order: 0) merge and benchmark baseline → a) follow-ups → b) dialect → c) JITD2 → d) UI1 → e) JIT_OBJECTS steps 0 and 1 after approval.
Consequences: ROADMAP rows BENCH0, TOK2, MUT2, PERF3c, JITC1, DIA2, JITD2, JITO1 new; §1 extended by the sequence.
Status: [final]

## 2026-10-26 18:55 UTC — Repeat the benchmark baseline; PostgreSQL in the work container (Jonas)
Jonas switched the laptop from power-saving to performance mode during BENCH0. Finding: the language measurement (18:03) ran with governor `powersave`, the switch fell into the BM2 run (until 18:33); the TOK2 re-measurement (18:47, `performance`) was higher for all targets, Octane included (+24 %). BENCH0 therefore counts as invalid and is repeated with `bench/run_all.sh`; the script aborts if not all cores are on `performance`. The old results lie under `target/bench-tmp/old-26.10-powersave/`.
Jonas: PostgreSQL in the work container instead of a separate container; the Compose service `postgres` is not running and will not run → `postgresql-server` (Fedora: PostgreSQL 16.13) in the Containerfile, `bench/framework/pg.sh` (data in `.pgdata` next to the repository, socket only, cores 4–7), service, `depends_on` and volume `pgdata` removed from `.devpod/compose.yml` (the file stays for workbench, Redis and Mailpit), fallback to the service removed from `api.sh`/`bm2.py` (411f934). The running mutation test (MUT2) was paused for this and is restarted after the measurement.
Clock limit (Jonas): on the host `sudo cpupower frequency-set --max 4.2GHz` (≈ 82 % of 5.1 GHz), so that the cores do not run permanently at the limit; governor stays `performance`. Applies from now on for work and measurements. `bench/run_all.sh` aborts if the cores have different limits and writes the limit into `env.txt`; `bench.py` and `bm2.py` name it in the header. The setting only holds until the next reboot of the host and must be set again afterwards, otherwise measurements are not comparable.
Addendum (Jonas, 19:10 to 19:15): for the measurement the limit, after 4.7 GHz, is finally **4.5 GHz** (≈ 88 %); this limit applies for the repetition of BENCH0 and is in its `env.txt`.
Status: [final]

## 2026-10-26 19:24 UTC — D20: Autonomous up to but excluding UI1, new order (Jonas, changes D19)
Decision by Jonas, status [final]:
 1. MUT2: finish the mutation tests (ops, convert, array, compare each ≥ 80 %; every surviving mutation with a test or justification in Tools.md).
 2. PERF3c is done (archiv/BENCHMARKS_bis_2026-10-26.md §3.3, recommendation "no rebuild" accepted); instead CALL2 (item 7).
 3. JITC1: count probes with translated code and assert them as a minimum.
 4. DIA2: `%` and bit operations only int, `abs(PHP_INT_MIN)` throws, `++`/`--` only int/float; tests, marking of the probes, BC1.
 5. LV3: PostgreSQL in the workbench container (separate task).
 6. JITD2 (document only, items a–h) and directly afterwards JITO1 (steps 0 and 1). Limits: no change to D8, no exit in the middle of a function, no assumptions that can later become false; before building, differential test with objects (typed properties, inheritance, overridden and non-overridden methods, exceptions in constructors), also with GC threshold 1 and `GRD_JIT_FORCE_EXIT`, green; runtime functions under Miri, `unsafe` only in grd-jit with justification per place; the abort criterion from JITD2 h) applies automatically (gain missed → stop and report). Steps 2/3 and any change to D8 are decided by Jonas. Result of JITD2 and justification of JITO1 in detail in JIT_OBJECTS.md ("Result JITD2", "Justification JITO1"), as a decision here and in the report with measurements before and after step 1.
 7. CALL2: fast call path, prepared at link time (only positional arguments, matching count, scalar types), bypasses the general binder; measurement by time (recursion, calls).
 8. Then stop; UI1 only after approval.
Rules: one measure per commit, suite green, push to vm-core after every green commit, regression guard after every package, update ROADMAP/DECISIONS/STATUS/Tools.md; stop and ask on changes to [final] decisions, merge to main and anything irreversible.
Additionally (Jonas): BENCHMARKS.md always holds only the last result; older full measurements go to the archive.
Consequences: ROADMAP sequence new; PERF3c done; new LV3, CALL2.
Status: [final]

## 2026-10-26 21:30 UTC — DIA2 implemented: three number rules in the dialect (Jonas D19/D20)
Implemented as decided (DIALECT §5.2, BC1 rows in LARAVEL_BREAKING_CHANGES §1):
1. `%`, `&`, `|`, `^`, `<<`, `>>`, `~` (and their compound assignments) only with int; a float operand is a `TypeError` with hint (`use fmod() or (int)` or `use (int)`). PHP truncates silently (deprecated with fractional digits) and wraps values that are too large; grd saturated until now.
2. `abs(PHP_INT_MIN)` throws `ArithmeticError` ("Integer overflow in abs()") like every integer overflow (D10); PHP returns float.
3. `++`/`--` only for int and float; `null`, strings (also `"5"`, `"a"`), bool, arrays and objects `TypeError`.
The JIT was not affected: it translates `%`/bit operations only for int/int and leaves `abs` at `PHP_INT_MIN` to the interpreter, which now throws. No performance impact (the fast path `num_fast` never took floats for `%`). Tests: `dialect.rs::integer_only_operators_and_numeric_increments`, probe `dialect_numeric_edges` (listed as dialect-related), type matrix `?int ++/-- null` as deviation (§5.2). From the PHP probes `operators_slow_paths` and `perf2_int_builtins` the affected cases moved into the dialect probe, so that they continue to output exactly PHP.
Status: [final]

## 2026-10-26 22:00 UTC — JITD2 completed, JITO1 steps 0 and 1 built (D20)
JITD2 (JIT_OBJECTS §8, result §9): for the gap between steps 2 and 3, **V1 (nested interpreter)** is recommended (D8 untouched, exits only on error paths; open: handles of waiting JIT frames as GC roots). Step 1 uses **V3**: exit only before the first visible effect, statically checked. Abort value 4.5× for c3 `objects` (comparison C).
JITO1 (justification JIT_OBJECTS §10): test mode `GRD_JIT_FORCE_EXIT` (91ad783), differential test for objects before building (59806db; found two interpreter gaps, ccac03e), then step 1 (f10f325): whole methods with `$this`, reading typed properties, `$this->p = v` with exact type, `new` for promoting constructors, class table of the VM, runtime functions with the VM as argument (logic safe in grd-vm, `unsafe` only in grd-jit, green under Miri). Discarded: direct slot access from the code, thread-local VM pointer, constructors with a body, objects in loop regions, profile speculation. D8 unchanged.
Measurement (c3 C, two rounds, 7 runs each): `objects` 7.2× → 3.9× and 6.9× → 3.7×; c3 total 2.5× → 1.9×; comparison A unchanged. Abort value not reached, step 1 stays. Steps 2/3 and any D8 change: decision Jonas (ROADMAP "Waiting for Jonas").
Status: [final] for step 1; steps 2/3 open

## 2026-10-26 22:45 UTC — CALL2 measured and reverted (D20: "otherwise revert")
Fast call path for user functions with exactly their positional parameters and matching scalar types, form prepared per code unit when loading the module (`fast_sig`: slot and binding per parameter). Three variants, each against f10f325 in comparison A (profiling build, 7 runs, two alternating rounds):
1. Prepared form in the general binder (`push_frame_from_stack`, `CallFuncId` in `exec`): c3 `recursion` 141 → 149/150 ms (+6 %), `objects` 193 → 201 ms (+4 %), c1 `recursion` unchanged.
2. Own arm for `CallFuncId` in the fast part of `run_frames` (without `exec`, without binder): Callgrind −8.4 % Ir (fib, 152.8 → 140.0 million), but time c3 `recursion` 141 → 146 ms (+3 %), c1 `recursion` 291 → 296 ms (+2 %).
3. Like 2, body outside the loop (`#[inline(never)]`): −6 % Ir, time `recursion` +3 %, `calls` +4–5 % (calls of native functions also run through the new arm first).
Cause: the binder has not been the bottleneck since FRAME1; the time lies in the frame switch and in the shape of the fast loop, which every additional branch slows down (like the reverted concat fast path). No variant committed. If calls are to become faster, then via the JIT (step 3, method calls from translated code) or a rebuild of the frame switch, not via another fast path.
Status: [final] (CALL2 done as "reverted")

## 2026-10-27 — UI1 redrafted (Jonas), guideline dialect/core for the frontend
**Guideline:** the dialect only describes **what** is sent: component, props, shared data, optional/deferred/merged props, redirects, flash, errors. The Rust core handles the protocol and the connection to the frontend: headers, versioning, partial reloads, merge rules, SSR, asset tags, validation errors via the session. No protocol code in the dialect.
Scope UI1 (replaces the previous version): a) inertia-omega (protocol v3) as the basis in the framework crate, connected to worker pool, sessions, validation, commit pinned exactly; b) inertia-laravel as model and counterpart of a differential test (status, headers, page object), deviations in BC1; c) session login with CSRF/XSRF-TOKEN for web routes, token login for the API unchanged, renew the session ID on login, 409/303/only/except/shared props/errors/flash each with a test; d) SSR with deno_core in a separate crate behind a feature, one isolate per worker thread, time and heap limit per render, fallback to client rendering, dev via Vite (HttpGateway); e) vite-plus via `opengrid dev`/`opengrid build`, Vite manifest; f) contributions to inertia-omega via fork, PR texts by Claude, opened by Jonas, list in CONTRIBUTIONS.md; g) Playwright E2E with and without SSR; h) BM2 with an Inertia page against inertia-laravel/Octane; i) BC1, Tools.md. Start after the rebuild of the image; stop after UI1.
Preliminary checks 2026-10-27: deno_core continues to be published from `denoland/deno` (0.413.0, 2026-10-21) → no blocker. inertia-omega: active, but without a LICENSE file (only `license = "MIT"` in Cargo.toml), PRs only for collaborators, **issues and discussions switched off**, no CONTRIBUTING file; the clarification path via an issue thus does not exist (report to Jonas). Containerfile prepared: PostgreSQL 17 (Fedora `postgresql17-server` 17.9), vite-plus 1.4.0, Playwright 1.64.1 with headless Chromium, sccache and `jobs = 2` in the Cargo configuration; `pg.sh` creates a new data directory on a different major version and keeps the old one.
Status: [final] (guideline and scope)

## 2026-10-27 — Jonas: vite-plus only a development tool, JavaScript runtime (deno_core) fixed in the backend
Question from Jonas: integrate vite-plus "directly into the interpreter, because it is written in Rust". Finding: the Rust crates of vite-plus are the CLI `vp` (Node/package manager management), `publish = false`, version 0.0.0; dev server, build, Vue compiler and Tailwind run as JavaScript in Node (Vite on Rolldown). Decision Jonas: **vite-plus only for development**, installed only in the Containerfile or container, no code in the core. **deno_core** (embedded V8) is built firmly into the backend so that SSR for Inertia runs directly in the process in production.
Implementation: separate crate `opengrid-ssr`, enabled by default in the `opengrid` binary (feature `ssr` as default), building without it remains possible (CI, fast tests). Not in grd-vm: deno_core depends on Tokio, and the interpreter core stays without Tokio and without `unsafe` (CLAUDE.md, D6). One V8 isolate per worker thread as in UI1 d).
Status: [final]

## 2026-10-27 09:45 UTC — Full benchmark 0fa624f (Jonas part 1)
Since 0fa624f Laravel connects via TCP 127.0.0.1:5432 (pg.sh listens there in addition to the socket). BENCHMARKS §1/§1.1 cleaned up (governor `performance` on the host, Laravel row on PostgreSQL/TCP). Run `run_all.sh` on 0fa624f; two parts repeated: the language run (coincided with my `dnf repoquery`, c1/c2 σ up to 21 %) and the BM2 targets Octane and `touch0` (Octane "exited early", not reproducible). Lesson: no package queries or other loads during a measurement, not even "light" ones. Result in BENCHMARKS §0: c3 C 2.5× → 1.9× (`objects` 6.8× → 3.9×), otherwise ±0.1×; Laravel via TCP 3–12 % slower than via the socket, gaps unchanged.
Status: done

## 2026-10-27 — D21: Laravel in BM2 via the Unix socket to PostgreSQL (Jonas; relates to D11)
Laravel (FPM and Octane) reaches PostgreSQL in the framework benchmark again via the **Unix socket** of `pg.sh` (as until 6253a70), not via TCP (0fa624f, reverted from 800ae8b). Rationale: the fastest possible path for the opponent so that the result is not open to attack, and comparability with 6253a70. **Note on interpretation:** PostgreSQL runs on the same host as the application; the socket is thus an optimum for Laravel, not the typical production case with a separate database server (there the network stack would be added; measurement 0fa624f: Octane −3 to −8 %, FPM −4 to −12 %). Yardstick: production-typical setups are compared (Laravel + PostgreSQL against OpenGrid + SurrealDB); SQLite is deliberately not an opponent. The TCP measurement remains possible with `bm2.py --pg tcp`. Checked: with the socket directory as `DB_HOST` PostgreSQL reports `inet_client_addr() = NULL` (socket), with 127.0.0.1 TCP. The connection kind is in the measurement header and in `env.txt`.
Status: [final]

## 2026-10-27 — D11 supplemented: "same semantics" means `touch1` (Jonas)
Finding from the query log (`querylog.py`): Sanctum does not write `last_used_at` on every request but at most once per second and token, because Eloquent only saves changed attributes and compares date values with second precision (200 requests in 0.5 s → 1 UPDATE; Sanctum 4.3.3, `Guard::updateLastUsedAt`). The previous target `touch0` (write on every request) was thus stricter than Laravel. Decision Jonas: new target `<OpenGrid target>-touch1` (`auth.token_touch_seconds = 1`, checked: 200 requests → 1 UPDATE) as the row "same semantics"; `touch0` stays as "stricter than Laravel". Comment in `prelude_auth.grd` corrected.
Status: [final]

## 2026-10-27 — D22: Prove the thesis instead of the showcase; new order up to the conclusion (Jonas, parts 1/2 and 2/2)
**Thesis:** the design integrated into the framework (warm workers, Rust core, native database layer, embedded database) makes OpenGrid faster overall than Laravel at realistic web requests, although the interpreter itself is slower than PHP. It is proven in two versions, clearly labelled: standard against standard and same semantics.
Order: (0) the running task "keep BM2 comparable, report the database share" (steps 1–4) first; Jonas adds: do not strip out the database share, but report the speed of the databases themselves too, so that developers can judge the influence themselves. (1) Part 2b finished (a worker crash is a security problem). (2) THESIS1: THESIS.md (claim, proof, refutation), new endpoints with parameter n (hydrating with relationships, collection processing, nested validation, nested JSON) in test app and Laravel app, n = 1/10/100/1000, curves requests/s and p99 against Octane and FPM, intersection; time breakdown per point (database, framework) and profile per side. (3) PERF4: measure first, then build; yardstick THESIS1 endpoints at n = 100/1000 without JIT; profile; PGO as the default of the release build (numbers without PGO as a separate column); PAR1 (Rayon only for start/hot reload, not in the request path); JIT in the workers only as a measured quantity; time box three measures or until the largest spot is under 10 %. (4) Measurement tool: clarify the Octane abort, quiescence check in run_all.sh, one reproduction command with raw data. (5) Final measurement, THESIS.md with result (proven/partially/refuted, honest also where Laravel is ahead), stop.
Deferred: UI1 with SSR, vite-plus, contribution to inertia-omega; JIT_OBJECTS 2a/2c. PostgreSQL stays within the measurement series at the version of 6253a70/0fa624f (16.13); 17 only in the Containerfile, commands in CONTAINER.md, one counter-measurement at the end. Network queries with a neutral User-Agent and the project URL.
Status: [final]

## 2026-10-27 — REPO1 and README1 (Jonas)
REPO1: phprs becomes a self-contained, reproducible repository (fork of a public repository, hence public; every push is immediately visible). **Step 1 is a hard gate:** confidentiality check of all contents that are to go into the repository (Laravel app, .devpod, documents) for private code and internal names, hits with file and line to Jonas; check step in the pre-commit hook. Then a plan of the target structure (examples/laravel-app, .devpod with public compose.yml and private compose.override.yml, tools/refs, docs/) with every path change for approval; work on branch `repo1`, push only after approval. Proposal of public documents to docs/, proof of reproducibility in a fresh clone, instructions for the container build on the host; the `bench/` folder next to the repository and `bundles/` only report, do not delete.
README1 (after REPO1): English README as the public calling card, subtitle and note on the missing affiliation with Laravel, status alpha; benchmarks only from the last complete measurement, tables by script from the raw data (`bench/report.py`), method, three framework tables (standard, same semantics touch1, stricter touch0), database share, language with the explicit sentence that the interpreter is 2–3× slower than PHP, limits, reproduction. No private code, no marketing language, no internal documents linked; approval before main.
Order: after the running task (keep BM2 comparable), before JITS1/THESIS1.
Status: [final]

## 2026-10-27 — REPO1 supplemented: history, license, English, contact (Jonas)
1. **History:** the public repository begins with a single first commit from the cleaned-up state (orphan branch), without the history of the fork. **No Co-Authored-By or Claude session lines in commit messages, now or in the future** (applies immediately to all commits, also on vm-core); the pre-commit hook rejects such messages.
2. **License:** the whole work under AGPL-3.0-or-later (LICENSE in the root directory, `license` in all own crates, SPDX header in own source files). Files with code from phprs or Loco (copied or derived) keep their license and their copyright notice in the header; NOTICE lists them, license texts under LICENSES/. `examples/laravel-app` keeps the MIT license of the Laravel skeleton. Report: list of the affected files.
3. **English** throughout the repository: documents, comments, log and error messages (also GRD hints: "Hinweis:" → "help:"), test names and outputs, scripts; tests that check messages are adjusted. `.agents/` outside the repository stays German.
4. **README:** section "Status" at the very top: experimental, not for production use; issues and pull requests are not processed; contact address of the project if interested.
5. Furthermore: confidentiality check before the first push, plan for approval, proof of reproducibility in a fresh clone.
Status: [final]

## 2026-10-27 — REPO1: Jonas pushes the new repository himself
The new public repository lies under a new GitHub account for which there are no credentials in the container. Claude does not push it but reports "ready to push" (cleaned-up orphan state, confidentiality check passed, plan approved, reproducibility proven); Jonas pushes. Pushes to `vm-core` of the previous fork remain as before. Target: https://github.com/OpenGridSolutions/framework; Jonas has already placed the LICENSE there (first commit). The cleaned-up state is therefore built on this commit instead of as an unconnected orphan branch, so that Jonas can push without force; check beforehand that the LICENSE is AGPL-3.0-or-later.
Status: [final]

## 2026-10-27 — BM2V completed (framework benchmark comparable, database share reported)
Steps: (1) Laravel via the Unix socket (D21, fbd4739; checked with `inet_client_addr()`). (2) Durability equivalent, nothing changed (commit only after WAL sync on both sides; in the measurement header and in env.txt, 23fb84f). (3) Queries per endpoint from the database logs (`querylog.py`, e0d2904); finding: Sanctum writes at most once per second → `touch1` as same semantics (62cdb31, Jonas); self-measurement `dbbase.py`/`dbbase.php`/`dbbase.rs` (54d725a), corrected after two own errors (key collision 57607a9; fixed value on token write and write frequency per target, 8db672e/f482c21, on Jonas' hint both sides by the same rule); tables via `bench/report.py` from the raw data (77d4cc5). (4) Full measurement `run_all.sh --framework-only` on 77d4cc5 (all 11 targets in one run, 0 error statuses out of 17.2 million requests), self-measurement repeated afterwards in the same result folder (run after f482c21). Result BENCHMARKS §0/§0.4: framework share per request OpenGrid 0.07–0.25 ms, Octane 2.4–3.0 ms, FPM 4.5–5.4 ms; PostgreSQL on its own as fast as or faster than SurrealDB. Deviations from the method: language part not re-measured (Jonas), BM2 has one run per measurement point (no σ); the Octane abort of the warm-up did not recur (cause still open, BENCH1).
Status: done

## 2026-10-27 — D23: Framework core comparison with raw SQL on SQLite (Jonas; changes D21 and "SQLite is deliberately not an opponent")
For the framework comparison both frameworks execute **the same raw SQL statements against SQLite in their own process**. Eloquent and Sanctum drop out for Laravel, the native model/builder layer and AU1 for OpenGrid likewise. Thus the comparison measures the framework core (routing, middleware, controllers, interpreter, JSON) and is rather conservative for OpenGrid (the embedded database and the native database layer do not count).
Implementation (package DBR1): (1) OpenGrid gets a small SQLite adapter with raw queries after Laravel's model (`DB::connection('sqlite')->select/insert/update/delete/statement/transaction`, results as arrays, INTEGER/REAL/TEXT/NULL → int/float/string/null, bindings type-exact, one connection per worker thread, selection via `sqlite://path`; the SurrealDB path remains unchanged; tests in the suite; check the maintenance state of rusqlite beforehand). (2) Benchmark endpoints list, show, create, login, health identical in both apps and marked as measurement tools, with exactly the same SQL statements also for the token check (own simple check instead of Sanctum or AU1, token usage at most once per second); same responses, checked with api.sh for both apps. (3) Same settings: the same SQLite version (noted), `journal_mode=WAL`, `synchronous=FULL`, same `busy_timeout`, same indexes, same data, one connection per worker; Laravel under Octane and FPM. (4) Presentation in BENCHMARKS and README in this order: (a) framework core with raw SQL on SQLite as the main figure, with the sentence that ORM and auth on both sides are deliberately replaced by the same raw statements; (b) standard setups (OpenGrid with model and embedded SurrealDB against Laravel with Eloquent, Sanctum and PostgreSQL) as the everyday picture; (c) framework share (§0.4) as a supplement; (d) language. (5) The THESIS1 curves (n = 1 to 1000) on this basis as well. D21 (socket) continues to apply for the standard setups (b).
Order: DBR1 before THESIS1. Implementation step 1 (2026-10-27): rusqlite checked (0.41.0 of 2026-10-24, releases every one to three months, MIT, active, 136 million downloads), integrated **0.40.2** (August, matured) **against the system SQLite instead of `bundled`**: PHP's pdo_sqlite uses the system library 3.47.2 (Fedora `sqlite-libs-3.47.2-5`), the one bundled by rusqlite is a different version; so both sides run on exactly the same library (deviation from the wording "built-in SQLite", in favor of "the same SQLite version").
Status: [final]

## 2026-10-27 — Laravel Octane also with FrankenPHP; concurrency, memory and cold start as measured quantities (Jonas)
1. **FrankenPHP as an additional target** (Jonas: in the repository of the PHP Foundation, the favorite of many developers for Laravel Octane): Octane on FrankenPHP in worker mode next to RoadRunner, with the same 4 workers, cores, endpoints (core on SQLite and standard setups). In BENCHMARKS and README the comparison is against the **faster** of the two Laravel setups (Laravel's best case), the other stays as a column. Pin the version; check that FrankenPHP's built-in PHP matches ours (8.4.21, opcache); report deviations openly in the measurement header.
2. **Measured quantities in BENCH1 (proposal accepted unless objected):** real concurrency (256/1024 connections; open load at a fixed rate at 25/50/75/90 % of the maximum throughput per side, p50/p99/p99.9, knee point; load spikes); memory (PSS over all processes of a stack from `smaps_rollup`, idle, under load as peak and mean, after 10,000 requests; "app alone" and "app + database" separately); cold start and bootstrap (start until the first successful response, with and without Laravel caches; first request against warmed up; empty route for FPM's bootstrap per request; start with 4 and 16 workers, hot reload). Limit: everything on one PC with separated cores; shared remain memory bandwidth, L3, clock and the kernel's local network.
Status: [final]

## 2026-10-27 — Measurement stopped; final benchmark only after BENCH1/BENCH2 (Jonas)
The DBR1 full measurement (8a8a2da) was stopped after compiling, before BM2 ran. First the remaining tasks (FrankenPHP, THESIS1, PERF4, BENCH1 with concurrency/memory/cold start, BENCH2 preparation), then recompile and run the complete benchmark once.
Status: [final]

## 2026-10-27 — FPHP1: FrankenPHP built in; SQLite 3.45.2 for all core targets (Jonas)
FrankenPHP v1.13.4 (official glibc build, checksum pinned, MIT, php/frankenphp) brings **PHP 8.5.12 (ZTS) and SQLite 3.45.2**; our other Laravel targets run on PHP 8.4.21 and SQLite 3.47.2. Decision Jonas: FrankenPHP in its optimal settings (official build, worker mode) with PHP 8.5.12; the PHP version is secondary, because the interpreter comparison is a separate benchmark (which stays against PHP 8.4.21). **OpenGrid uses the same SQLite version as FrankenPHP.** Implementation: SQLite 3.45.2 built from the official sources (`bench/framework/sqlite/build_sqlite.sh`, checksum pinned) with exactly the compile options of the FrankenPHP build (`PRAGMA compile_options` equal except for the compiler; the system SQLite 3.47.2 additionally has `SECURE_DELETE`, `DISABLE_DIRSYNC` among others); the core targets load it via `LD_LIBRARY_PATH`, **also PHP-FPM and RoadRunner**, so that all four core targets have the same SQLite (extension beyond the wording, in the spirit of D23 "the same SQLite version"). Opcache settings of all Laravel sides equal (on, JIT off, 128 MB, `validate_timestamps=1`); FrankenPHP with 4 workers, `--max-requests=1000000` like RoadRunner. The comparison is against the faster Octane (`report.py`: "OpenGrid / best Octane"). First probe (16 connections): FrankenPHP ahead of RoadRunner (core `show` ≈ 3,970 against ≈ 2,580, PostgreSQL `show` ≈ 1,870 against ≈ 1,330).
Status: [final]

## Journal

### 2026-10-24 (night, later) — c2/c3 and a VM bug
- While building c2 the checksum deviated: `arsort` (likewise `rsort`, `krsort`) sorted ascending and then reversed, whereby equal values stood backwards. PHP 8 sorts stably. Fixed in 212752f, new probe 250.
- `gen.sh` now works with markers (`// grd: …` in the PHP line); the comment line explaining the format was itself replaced first — markers now only apply in code lines.

### 2026-10-24 (night) — Complex request benchmark (direct scripts, without framework)
- `.agents/feasibility/bench/complex/`: `c1_request.php` (reference), `c1_request.grd` (generated via `gen.sh`, only opening tag and sort assignments differ), `run.sh <grd>` (median of 3, per phase, PHP without opcache / with opcache / with opcache JIT tracing). 30 simulated requests with 2000 orders each: build test data, JSON encode/decode, objects + regex validation, computing/grouping, sorting with closures, recursion (search tree), strings/hashes, JSON response. Checksum equal in all variants (869982327).
- Result (profile `profiling`, median ms): total PHP 879.7 / opcache 866.5 / JIT 754.7 / dialect 5847.0 → dialect 6.6× (JIT: 7.7×) slower. Per phase against PHP: build 9.1×, json_encode 4.9×, json_decode 3.2×, hydrate 6.5×, compute 8.8×, sort 5.8×, recursion 11.5×, strings 6.3×.
- Found while writing, both intended dialect behavior: `<=>` between arrays is a TypeError (DIALECT §5.1), `preg_match` returns the match array. The script uses the form valid in both languages.
- For the container: `php-opcache` installed (dnf), so that opcache/JIT are comparable.

### 2026-10-24 (night) — Measurement query latency and interpreter (profile `profiling`)
- Jonas' reference: PHP worker + ext-php-rs extension, `SELECT * FROM users LIMIT 1` in a loop: up to 0.3 ms per query.
- Dialect, the same loop (30× `DB::query` in one request, 100 users): embedded surrealkv median 0.056 ms (first 0.35–0.48 ms); server ws (local, 3.3.4) median 0.19 ms (first 0.6–0.7 ms).
- Pure SDK without VM (throwaway probe `target/refs/query-probe`): embedded median 0.05–0.07 ms, server 0.16–0.27 ms; first query likewise 0.3–0.9 ms. The dialect adds nothing measurable to the SDK call.
- A pause between queries (2–20 ms) already raises the median of the pure SDK to 0.2–0.8 ms, also with a current-thread runtime: CPU governor `powersave` (idle clock), not our code.
- One request per query over HTTP (curl, incl. connection setup): `/bench/one` median 0.57 ms embedded / 0.78 ms server; `/hello` without DB 0.30 ms.
- Interpreter against PHP 8.4 (without opcache/JIT): loop 12.1×, calls 8.8×, methods 10.0×, arrays 10.0×, strings 4.3×, array passing 2.0×, objects 5.0× slower.

### 2026-10-24 (late evening)
- DB1: only the end-to-end run showed that the database messages were missing; the cause was the order in Loco's CLI (`after_context` before `init_logger`).
- Mishap: while rebuilding `app.rs`, a text anchor first hit the newly inserted function instead of `after_context` and cut out a piece; file restored from the commit and reapplied with unambiguous anchors.

### 2026-10-24 (night, later)
- R4c: the first build with SurrealKit ran without compile errors; three of four tests green immediately. The fourth failed at reopening the surrealkv file in the same process (finding above).
- Surprise: `rustc` in the SurrealKit checkout hangs, because its `rust-toolchain.toml` (1.95.0) downloads a different toolchain; do not invoke tools inside the reference checkouts.
- Surprise: in `toml` 1.x `str::parse::<toml::Value>()` only parses a single value, not a document; manifests with `toml::from_str`.
- Preflight added on Jonas' hint during R4c.

### 2026-10-25 (morning, D12 phase 1)
- B2c (1a8e2e6): server tests against a throwaway `surreal` in the container; all six green on the first run except my test (unique title duplicated).
- TX1 (c8b8043): counter-check without retry: 86/120 (embedded) and 63/120 (server) transactions fail; with `$attempts` none, no lost update.
- R1 tool (943db5a) and first run: 11,502 tests in under 3 min; 43 crashes/timeouts, all fixed (PHPT_REPORT). Surprises: two class constants referencing each other terminated the process; `DateTime::sub(PT1H)` across the daylight-saving switch never progressed; `isset()` called `__get`; the `strcmp` family returns the byte difference in PHP 8.4.
- My errors: `cargo fmt` reformatted the whole crate (reverted, changes reapplied; the project uses no rustfmt configuration, hence no `cargo fmt`); one commit (e84781f) got past a red suite (pipe exit code, see KNOWN_ISSUES); estimated instead of measured timestamps corrected.

### 2026-10-25 (early, task D11)
- Part a: method A/B/C and baseline measurement (a7efda9). Surprises: grd starts in 41.5 ms against PHP's 17 ms, because the preludes (framework classes and `OpenGrid\Support`, considerably larger since S1) are translated per process; the compiler needs 2.3 ms for c1 against 0.17 ms; c1 spends 253 ms outside the measured phases (PHP 5 ms). `php -l` takes several files since 8.3, so the translation of both sides could be measured alike.
- Part b, LV1: the Laravel replica passed `api.sh` after one correction (Laravel redirects unauthenticated requests to the route `login`, which does not exist → 500; `redirectGuestsTo(fn () => null)`). Found during the comparison: OpenGrid's messages for `min`/`max`/`size`/`between` without unit (f67d168). The private `App\Database` layer is not yet available (list in D11, 06:45) → preliminary stage on SQLite.
- Part b, BM2: the first trial run found a real bug: concurrent requests with the same token ran into surrealkv write conflicts (`last_used_at`) and ended as 500, up to 25 % at 16 connections embedded (27534a3, KNOWN_ISSUES). Tools added in the container (php-fpm, nginx, composer, oha, surreal 3.3.4) and documented in the Containerfile.

### 2026-10-24 (night, task D10 up to TP5)
- [21:58] c) was already met: DateTime/DateTimeImmutable have been native classes with `DateTimeInterface` since b69c0fa, the prelude class is replaced; lazy rows since B2a. Checked, no change needed.
- [22:00] V1: SurrealDB (3.x) rejects `SELECT` on an undefined table as soon as a schema is applied; `exists:`/`unique:` count such a table as empty.
- [22:02] Mishap: a commit (AU1) went through although two schema tests were red, because my command chain did not check the test outcome; fixed with `--amend` before the push, since then I commit only via a script that aborts on a red suite.
- [22:05] I1: new code **GRD022** (runtime error on injection): an interface or an abstract class without or with several concrete implementations. No registration (D1), therefore the binding must be unambiguous; way out: inject the concrete class.
- [22:20] S1: Laravel's own classes run under PHP 8.4 directly from the reference checkout (autoloader in the driver, empty stand-ins for the contracts/traits not checked out, polyfills for PHP 8.5 functions); the specification test compares 215 expressions. It uncovered the too-strict first GRD021 version (correction above, "mixed"). Deviations that are dialect semantics: comparisons in `where`/`contains`/`search` strict, no arithmetic with `null`; `Arr::set`/`forget` return the new root array.
- [22:20] The preludes are compiled once per process (runtime of the grd-vm suite 9.5 s → 0.3 s, grd-jit 49 s → 3 s; faster worker starts).
- [22:25] **Security finding:** `sort()`/`orderBy()` put field and direction into the query unchecked, and `sort()` reads `?sort=` from the request — injecting SurrealQL was possible. Fixed (0bfce6c): only identifier paths and asc/desc, invalid `?sort=` → 400.
- [22:28] TP5 accepted: `api.sh` green twice in a row (43 steps), suite green (128 tests), application without GRD hits.

### 2026-10-24 (night, safeguarding and R4)
- Surprise: `#![forbid(unsafe_code)]` was in no crate, although D6 and CLAUDE.md named it. Added.
- Surprise: the schema tooling sources of the private reference implementation had been available since the morning; I had kept listing R4a/R4b2 as waiting. Done afterwards, with precedence over JIT work.
- The differential test found an interpreter bug on the first run (return value `int` for `: float` not widened, not checked at all via `finally`) and through it a second one (`var_dump` formatted 2^63 without exponent). The JIT was right in both cases.
- Generator bug in the differential test itself: `(-` before a negative literal gave `--7` (decrement). Fixed with a space.
- The golden test of the reference files was identical from the start; 9 of 16 own edge cases deviated, among them four real bugs of the original.

### 2026-10-24 (night, JIT)
- PERF1 and JIT1b tier 1 finished. c3_compute against PHP with JIT: 20.2× → 5.3×; calls and recursion faster than PHP (0.6×), float computation 1.3×.
- Surprise: the compiler left the result of every expression statement without assignment (`f();`, `$p->step(0.5);`) on the operand stack. In functions the return cleaned up, in the main script the stack grew (c3: 60,000 values per round). Found because the JIT only enters loops with an empty stack. Fixed in fed82d3.
- Proof: all 239 probes additionally run with JIT threshold 1 (`grd-jit` suite); checksums c1–c3 equal.

### 2026-10-24 (night)
- R4b built without PHP sources (assumptions in the entry above). All tests ran on the first run; the only mishap: `build` was already taken in `schema.rs` (blueprint), the new function is called `build_surql`.
- Surprise during the design: PHP's `ksort` is not transitive for mixed keys (`"1x" < "20" < "100" < "1x"`); Rust's sort may abort on such a comparison.

### 2026-10-24 (late)
- B2a: the lazy rows were already built with B1 (`ModelData.pending`), but untested and not noted. The new test found two bugs: `clone $model` lost the native payload (no clone handler, every field access afterwards failed), and writing left the raw value in `pending`.
- Surprise: `Vm::get_property` panicked with "no frame" when Rust called it between two scripts (visibility from the current frame). Now the global scope applies without a frame.
- B2b: Jonas' wording "DateTime family as a native class" I read in the sense of the decision "SurrealDB types native" item 1: DateTimeImmutable/DateTime native (done since b69c0fa), DateTimeZone/DateInterval remain dialect. Only eight dead helpers of the old dialect DateTime were removed. If DateTimeZone/DateInterval shall also become native, that is a separate package.

### 2026-10-24 (evening)
- 5a–5c done: `Claude.md` → `CLAUDE.md` (was not loaded until now); loco switched to upstream `loco-rs/loco` v1.2.0 (11 MB instead of 27 MB, fetchable without credentials); target from 235 GB to 2.9 GB `deps` (D7).
- Surprise: `cargo clean` reports "Device or resource busy" because `target/` is a mount; the content is deleted nevertheless.
- Surprise: in `tests/suite.rs` `mod x;` looks next to `suite.rs`, not in `tests/suite/`; solved with `#[path]`.
- Surprise: SurrealDB 3.3.4 still uses surrealkv 0.21.x, although surrealkv 1.0.0 has been out since 2026-10-13.
- Correction: the "panic" at `rollout start` without baseline was a regular error with the CLI's backtrace output.
- Model switch: from here on Claude Opus 5.5 continues (before Claude Fable 5.1).

### 2026-10-24 (afternoon)
- R0 (references) completed: 13 upstream reference checkouts (434 MB, shallow + sparse), fetch script (second run: all "unchanged"), write protection, documented review. loco moved into the reference set; path references existed only in `CLAUDE.md` (three lines; the file was named `Claude.md` until 2026-10-24 and was therefore not loaded), none in phprs.
- Surprises: (1) `php-8.4.21` is still under PHP-3.01, not BSD (the GitHub API shows the license of master). (2) `cranelift-*` pull in two "INTERNAL" helper crates from the Wasmtime workspace (`wasmtime-internal-core`, `wasmtime-internal-jit-icache-coherence`), but no runtime. (3) SurrealKit prunes only its own objects; foreign tables stay — the concern from section 8.4 (framework tables would be deleted) does not apply as such, the fragment remains sensible nevertheless. (4) `rollout start` without `rollout baseline` returns a regular error ("no manifest starts from there … pass `--from <id>`"); the CLI appends a backtrace, which is why I first took it for a panic. (5) The YJIT docs lie in `doc/jit/yjit.md`, not `doc/yjit`. (6) `laravel/laravel` has its own tags (v13.11.0), not those of the framework. (7) HeldvonKosmos/loco is not reachable without login (404); the existing clone stays.
- Network: hotspot, the connection came and went; fetching and downloads with retries, no blockage. The SurrealDB server on the host still not reachable → SurrealKit probe against a local throwaway server (`surreal start memory`, 3.3.4, ports 8100–8102 in the container).

### 2026-10-24 (noon)
- B part 1 (SurrealDB types native) completed. The first test run after the restart showed four deviations, three of them in my test itself: the expected timestamp was off by one day (the implementation was right), GeoJSON came from the SDK `as_object()` key-sorted (`coordinates` before `type`) — now own construction with `type` first and `geometries` for GeometryCollection — and `DateTimeImmutable + Duration` was missing, because the core date type does not know the framework class; the Duration handler takes over the reverse case and preserves class and zone of the date.
- Surprise: SurrealDB 3.3 outputs an `option<int>` in `INFO FOR TABLE` as `TYPE none | int`. The type parser took the first word (`none`), the filter conversion never kicked in. Now the type runs until the next clause (`PERMISSIONS`, `DEFAULT`, …) and `base_type()` removes `none`/`null` alternatives; unions of several real types remain unchanged (no cast).
- Environment: the build with `jobs = 8` drove the host into OOM, reboot needed; the devcontainer came up fresh in the process (sccache and Cargo config gone). Now `jobs = 2`, earlyoom on the host. The SurrealDB server on `host.containers.internal:30801` has not been reachable since the reboot.

### 2026-10-24 (early)
- Legacy removal completed (steps 4–5): workspace without the old core builds and tests; docs switched to fixed comparison values and the tag. `Name::class` test with imports, aliases, group imports and `namespace\` prefix added.
- Legacy harvest: zsh aborted a classification because of an empty glob (`rm` with `*.fail`), hence the harness scripts in Bash like the others. 157 of 175 old scripts run identically to PHP in the new VM; the only real compiler find was `Name::class`.
- Sessions: both stores correct on the first run, the only failure my fixture controller (overwrote `name` with the default).
- Relations: correlated sub-selects with `$parent.id` ran against `mem://` right away; the only failure the string filter `done=true` (string vs bool), solved by value casting.
- Schema/migrations: the only failure my expectation of the batch number after a complete rollback (empty → 1 again). SurrealQL DDL (`DEFINE … IF NOT EXISTS`, `REMOVE … IF EXISTS`, `INFO FOR TABLE`) was right at once.
- Error page + hot reload: the only failure was my trace, which omitted the outermost frame (controller method called by the Rust dispatcher) ("#0 {main}"); after the addition all tests green, the watcher test needs ≈ 0.5 s.
- Middleware/groups/abort/validation (≈900 lines of Rust + 90 lines of dialect prelude): build and all 14 framework tests green on the first pass.
- Database layer (≈1,300 lines) written; surrealdb 3.3.4 pulled 272 crates, first build ≈ 4 minutes. The only compile error: the SDK builders (`Begin`, `Commit`, `Cancel`) are `IntoFuture`, not `Future`. After that all three database tests (CRUD, soft delete, pagination, filter whitelist, rollback, HTTP) ran on the first pass against my expectations; the server smoke test with `mem://` likewise.

### 2026-10-23 (evening)
- Crate `framework` (≈1,100 lines): pool, dispatch, five native classes, Loco hooks, binary `opengrid`. The four integration tests ran on the first pass except for my test expectation (an empty object store counts 0, not 1). `opengrid start` only needed `workers.mode: BackgroundAsync` in the YAML; after that `/tasks`, `/tasks/9?verbose=1`, POST JSON, 404 and 500 responded as expected. ROADMAP M0 "first request" thereby met.
- Project loader (`vm/project.rs`, 240 lines) with fixture app, CLI `grd app`, 2 tests; ran on the first pass. The five deviations from revision 2 completed before that (link step, method table, slot window, arg bitmask, CodeProfile).
- Jonas' decisions incorporated: arithmetic again only int/float, `array_unique` strict, compare handler for native classes, GRD019/GRD020 with `Arr::pop`/`Arr::shift`, performance guideline. Date error corrected (container: 2026-10-23).
- Dependencies checked and updated (postcard instead of bincode, majors raised, `regex` out).
- Phase 7: serde/bincode on `Module`, old core to `legacy/core` (one test depended on the directory size), docs aligned. Task option B completed; hand-over checklist in STATUS.

### 2026-10-23
- Phase 6: native classes (registry, payload, handlers, engine API). The acceptance example (`.grd` model inherits from native `Model`, closure mutates native `Builder`, `toSql()`) ran on the first pass; the only error was my test expectation (`(int)` cast instead of strict conversion).
- Phase 5: reflection (prelude over native introspection, 5 probes), inline caches (first via HashMap: regression, then via frame index), superinstructions (`BinSI`, `JumpUnlessSI` …), `Op` to 24 bytes, `break`/`continue` through `finally`. Loop 9.9× → 6.5×, methods 7.2× → 4.9×. Surprise: the HashMap lookup for inline caches cost more than it saved; only the index in the frame brought the gain.
- Jonas: `.grd` files now begin with `<?grd` (probes, benchmarks, frozen tests); the harness gives PHP a `<?php` copy.
- Phase 4 begun: `preg_*` (fancy-regex), DateTime family (chrono/chrono-tz, classes in the prelude, native helpers), filter_var, password_hash (bcrypt), serialize, URL functions, version_compare. Probes 61/80 (probe a67 failed on "+1 day", which was read as time zone `+1`).
- Jonas: revision 2 of the runtime architecture and `DATABASE_LAYER.md` arrived and filed; `<?grd` as opening tag implemented.
- Branch `vm-core` from `main`; the upstream documents deleted in the working tree for a while (ARCHITECTURE.md, …) removed as a separate commit, they live in `.agents/`.
- mago checked (entry above), crate `grd-vm` created, `grd parse` via mago. Phase 0 accepted: 112/112 files parse.
- Phase 1: value model written and accepted (12 tests).
- Phase 3: VM written (~3,500 lines: class linker, dispatch, path writes, calls, exceptions, generators, iterators, closures, enums) plus ~2,000 lines of builtins (string, array, math, JSON, var_dump/print_r/var_export). First run: 57/80 and 12/25, after three fixes (nullsafe jump, `__isset`, `"$a[k]"`) 60/80 and 13/25. Surprise: the smoke test (recursion, identity, COW, closures, finally, generators) ran correctly at the first start; the only real VM bug was the stack discipline at `?->`.
- Phase 2: bytecode format and compiler (mago CST → Module) written, ~2,900 lines; 112/112 files compile or are rejected with the right GRD code. Surprise: mago delivers `LiteralString.value` already decoded, and `PartialApplication` is an enum with three variants (function/method/static), not a struct. Surprise: none, the float formatting with precision 14 (`1.0E+14`, `-0`, `0.33333333333333`) was right at the first comparison with `php -r`.

## 2026-10-27 — THESIS1 implemented (86685b9)

- **Workloads** as in THESIS.md §2: `hydrate`, `collect`, `validate`, `json` with n = 1/10/100/1000, line for line identical in both apps,
  behind the same token check of the core (D23), responses byte-identical (api.sh exactly, bm2.py hash before every measurement).
- **Base cost** per workload = the same endpoint with n = 0; for `validate` `json:0` applies, because `validate` without elements is a 422
  (`items` required). Database share from `sqlite/sqlbase.py` with the driver of the respective side (PDO or rusqlite) against
  the same SQLite 3.45.2.
- **Laravel's validator is not bypassed:** it grows strongly superlinearly with wildcard rules (function probe n = 1000 ≈ 120 ms per
  request). That is Laravel's default behavior; it stays in the result but is explicitly named there as the cause, so that the
  lead at `validate` is not misunderstood as an interpreter or framework advantage.
- **Intersection:** ratio of OpenGrid (without PGO) to the faster Octane server at 16 connections, interpolated linearly over log10 n between
  the neighboring points.

## 2026-10-27 — Finding: the benchmark's core layout is SMT (open question to Jonas, BENCH1)

- The fixed core ranges of BM2 (app 8–11, database 4–7, load 0–3) are on the Ryzen 7 8845HS each **two physical cores with
  two SMT threads** (siblings 8–9, 10–11 etc., `/sys/devices/system/cpu/cpu*/topology/thread_siblings_list`). Found in PERF4:
  at `hydrate:1000` with 16 connections the four workers are 97 % busy and need ≈ 6.8 ms CPU per request against ≈ 4.5 ms
  at one connection.
- Both sides lie on the same cores, the comparison stays fair; but "4 workers" means "two physical cores". **Nothing changed.**
  Proposal for BENCH1 (Jonas decides): report the topology in `env.txt` and in the measurement header; optionally app on 8, 10, 12, 14 (four
  physical cores, one thread each), database 4, 6, load 0, 2 — then every worker would be alone on its core. A switch changes all
  numbers and only applies to a new measurement series (FINAL).

## 2026-10-27 — PERF4 completed (protocol PERF4.md)

- **Kept:** measure 1 (SQLite rows from template, aa9ff5c), measure 2 (JSON objects without copies, 120451a). **Reverted:** measure 3
  (JSON byte table, no gain above the spread). Time box with three measures reached.
- **PGO is the release default** (`tools/release.sh`); in the report the PGO column leads, the numbers without PGO remain a separate column.
- **D14 sharpened:** the PGO training (`api.sh`) skips the sections with the benchmark endpoints (`/api/bench/sql`, `/api/bench/logic`),
  which had been in it since DBR1/THESIS1; otherwise the training would have seen exactly what the core targets and THESIS1 measure.
- **PAR1 dropped** (little benefit): start until the first 200 with 1/4/16 workers 65/74/104 ms, compiling all project files 8 ms; PAR1 saves at most the project compile share per worker (with 16 workers ≈ 30–40 ms). **Correction:** the first version ("≈ 70 ms independent of the worker count", also in the message of 3529b98) rested on a bug in `bm2.patch_config` (all three measurements ran with one worker); re-measured, decision unchanged.
  rayon would be maintained (1.12.0) and already in the tree; take it up again if `startup.py` shows a measurable compile share for a larger project.
  The integration would be: a short-lived rayon pool only in `Pool::start` and in the reload of the file watcher, result `Arc<Module>` to the worker threads,
  request path (Tokio accepts, worker threads with their own VM execute) unchanged.
- **JIT in the workers** measured once: no measurable effect on the THESIS endpoints; stays off (D9).

## 2026-10-27 — Core split: app on 8, 10, 12, 14 (Jonas)

- Jonas: "put the app on 8, 10, 12, 14, then continue with BENCH1." The app thus runs on four physical cores with one thread each; the
  SMT siblings 9, 11, 13, 15 stay free (the quiescence check `bench/quiet.py` reports load on them). Database 4–7 and load 0–3 were
  not part of the decision and stay (two physical cores with two threads each).
- Implemented in one place: `bm2.APP_CORES/DB_CORES/LOAD_CORES`; `dbbase.py`, `querylog.py`, `profile_core.py`, `startup.py`, `load.py`,
  `memory.py` and the quiescence check take them over. The measurement header and `env.txt` name the topology per range. The language benchmark
  (`bench/complex/bench.py`, one process per run) keeps `--cores 8-11`.
- Consequence: all numbers from FINAL on are measured with this split and are not directly comparable with earlier BM2 runs (applies to both sides).

## 2026-10-27 — D24: PostgreSQL dropped; standard setups (b) with Laravel on SQLite (Jonas)

- Jonas: "we no longer need PostgreSQL for the benchmarking, because both stacks now run on SQLite for better
  comparability, so please remove PostgreSQL from the container." Decided on inquiry: **(b) standard setups stay, Laravel
  (Eloquent + Sanctum) runs there on SQLite** instead of PostgreSQL (OpenGrid unchanged with model and embedded SurrealDB), and
  **PostgreSQL is removed completely**: packages from container and Containerfile, data directory `.pgdata`, `pg.sh`, `pg17_check.sh`,
  the targets `-pgsql`, `api.sh --db pgsql`, the PostgreSQL parts of self-measurement and query log.
- Consequences: D21 (Laravel via the Unix socket to PostgreSQL) and LV2 are void; the PostgreSQL 17 counter-measurement (LV3, part of
  FINAL) is dropped. The database self-measurement (§0.4) gets a SQLite version for Laravel with Eloquent's statements. SQLite is for
  Laravel as for the core the same library 3.45.2 (FrankenPHP built-in, otherwise via `LD_LIBRARY_PATH`).
- The previous measurements with PostgreSQL (BENCHMARKS §0, 77d4cc5) remain documented as the state; from FINAL on (b) applies with SQLite.
- **Addendum D24 (Jonas, 2026-10-27):** "then we also no longer need the measurements of the database overhead in the benchmark" and "also
  remove the measurement of the database overhead for SurrealDB". Removed: `dbbase.py`/`dbbase.php`/`examples/dbbase.rs` (SurrealDB embedded
  and server, PostgreSQL), `sqlite/sqlbase.py`/`sqlbase.php`/`examples/sqlbase.rs` (SQLite for the THESIS time breakdown), `querylog.py`
  (needed PostgreSQL's log). Report without section (c); THESIS time breakdown (f) shows base cost (n = 0) and the part growing with n.
  Thereby item (c) "framework share from §0.4" of the README outline (README1) is dropped too; the results of 77d4cc5 remain in
  BENCHMARKS §0.4 as the state.
- **Laravel SQLite equally durable:** the default connection `sqlite` of the Laravel app ran with `synchronous = normal` (own choice from LV1);
  for (b) now WAL, `synchronous = FULL`, `busy_timeout = 5000` like the core (checked: `PRAGMA synchronous` = 2). The skeleton would have the
  rollback journal, slower under concurrent writes, i.e. in OpenGrid's favor; WAL is the usual production setting.

## 2026-10-27 — BENCH2 completed: trial run `run_all.sh --smoke` (5543d85) and two findings

- Trial run of the whole pipeline with short durations: all stages ran through (gates 9 s, lang 20 s, bm2 9 min, thesis 7 min, load 6 min,
  memory 1 min, coldstart 38 s), no errors in the tools. Two findings a long run would have hidden:
  1. **Gates ineffective:** the OpenGrid gates failed (relative binary path, `api.sh` changes into the app directory), and the run continued
     anyway (`gates()` and `stage()` only returned the code of the last command). Fixed: absolute paths, every gate counts,
     `stage` passes the exit code on; also checked with a deliberately wrong path.
  2. **Quiescence check: 297 findings**, almost all the benchmark's own processes on the free SMT siblings (language benchmark still on 8–11;
     unpinned orchestration: Python tools, `artisan migrate`, measurement probes) and `rr.bin` (RoadRunner's actual process name).
     Fixed: every stage runs on the load cores 0–3 (servers and oha set their cores themselves), the language benchmark on 8, 10, 12, 14
     (one process per run, like the app cores; before 8–11), `rr.bin` counts as part of the benchmark. Left: one finding each `sccache` (remainder of the builds) and
     `claude` (Claude Code itself), which are named in the result.

## 2026-10-27 — BENCHMARKS.md §0 automatically from the run (Jonas)

- Jonas: "automate the mechanical part in .agents and add it as the last step to the run_all.sh benchmark script."
  Implementation: `.agents/tools/benchmarks_s0.py` generates §0 from the result folder (tables unchanged from the raw data, old §0 to the archive);
  the interpretation remains manual work (placeholder). `run_all.sh` calls a **local hook** `bench/local/after-run` as the last step,
  if present (in `.gitignore`); the hook here calls the script in `.agents`. So the public repo knows neither path nor name of the
  internal documents (README1/REPO1: do not link internal documents).

## 2026-10-28 — FINAL evaluated; clarifications before the README (Jonas' list)

- **Result** (70ba137, one run, started by Jonas without Claude Code): THESIS_1.md §4 **partially proven** (no refutation applies;
  intersection only at `collect`, n ≈ 725); interpretation in BENCHMARKS_1.md §0.
- **PHP per target in the measurement header** (6b29bb4): FrankenPHP 8.5.12 (own build), FPM and RoadRunner 8.4.21.
- **2 error statuses out of 33.6 million:** both OpenGrid, core `create`; reproduced: SQLite write lock after `busy_timeout` 5 s (writer
  starvation under sustained load). No change without Jonas (method question: the same longer timeout on both sides or leave as is).
- **Quiescence check (111 findings, core 11):** averaged ≈ 0.16 % of a core, not from the benchmark and not from the checker; source outside the
  container (host), not provable from the container. `quiet.py` now names the threads per finding (5109ca7).
- **Three runs:** `bench/run_series.sh` with `bench/series.py` (median, range of requests/s in the report; 5109ca7). Measurement pending.

## 2026-10-28 — Correction and proof on `validate`; one file per run (Jonas)

- **Correction:** Laravel's validator does **not** grow superlinearly (as stated in THESIS1, the D entry of 2026-10-27, and the first interpretation), but
  linearly with ≈ 105–109 µs per element (measurement in the Laravel app: n = 10/100/1000 → 1.07/10.4/108.8 ms; 5,001 expanded rules at n = 1000,
  ≈ 88 % in checking). OpenGrid ≈ 3.5 µs per element. Hence the factor ≈ 30.
- **Proof of equal rules:** new `api.sh` step that violates every rule once; OpenGrid and Laravel (FPM, RoadRunner, FrankenPHP) respond
  byte-identically. Found in the process: the overlay (since LV1) truncated Laravel's validation message to the first error (without "(and N more errors)");
  it now returns exactly Laravel's own JSON form (`Handler::invalidJson`). No influence on the measurement of the success cases.
- **One file per run** (Jonas: "every run should get its own name so that we can take three runs and form a median"):
  `benchmarks_s0.py` writes `BENCHMARKS_<n>.md` per run (next free number), `BENCHMARKS_median.md` per series; no more archiving.
  `run_series.sh` calls the hook after every run and at the end for the series. `BENCHMARKS_1.md` is the FINAL run 70ba137.

## 2026-10-28 — Three runs in `run_all.sh`, summary in BENCHMARKS.md, disturbances (Jonas)

- **`busy_timeout` stays** (Jonas: "that is accepted as long as it does not occur on the Laravel side"). The report counts error statuses per
  side; every error status on the Laravel side appears as a warning ("the run is not accepted as it stands").
- **Three runs in one call** (Jonas: "adapt run_all.sh so that three runs are made in sequence; the logs individually, then
  summarized in BENCHMARKS.md incl. median"): `run_all.sh --runs 3` (default), build once, every run in `run-<i>/`, then median and
  range (`series.py`), report and hook → `BENCHMARKS.md`. `run_series.sh` is dropped. The "one file per run" rule from the morning is thus
  replaced (runs are called `run-1`…`run-3` in the result folder); `BENCHMARKS_1.md` stays as the state of the FINAL run.
- **Document and display disturbances** (Jonas: "it is important that it is documented and later displayed as disturbances
  too"): fixed block in the report ("Disturbances", for the README) and in §0 of BENCHMARKS.md ("Disturbances"): error statuses per side and place,
  quiescence check per run, saturated load generator, tool warnings, gates.

## 2026-10-28 — D25: SurrealDB no longer in the benchmark; only the framework core on SQLite (Jonas)

- Jonas: "by the way, I want SurrealDB to be dropped completely as a database in the benchmark for OpenGrid. only SQLite shall be
  measured." Decided on inquiry: **(b) standard setups are dropped entirely** (also Laravel's Eloquent/Sanctum targets; OpenGrid's model and
  auth layer exist only on SurrealDB), and **no "without database" mode**: the core targets continue to open their embedded
  SurrealDB at start (job poller), without queries per request; that counts in OpenGrid's start and memory and stands as a disturbance in the report.
- Measured are only the core targets any more: OpenGrid without and with PGO, Laravel on FrankenPHP, RoadRunner, FPM, all with raw SQL on the same
  SQLite 3.45.2. Removed: `opengrid-embedded`, `opengrid-server`, `-touchN`, `laravel-fpm|octane|frankenphp` (standard), the SurrealDB server
  in `bm2.py`; `load.py`/`memory.py` core only. Three runs thus take ≈ 5 h instead of ≈ 8 h.
- Consequences: the token semantics versions (`touch1`, `touch0`) belong to the SurrealDB targets and are dropped; in the core the token check
  writes identically on both sides (at most once per second). THESIS refutation 2 ("the lead disappears when the database is aligned")
  is answered with the FINAL run 70ba137 (BENCHMARKS_1.md, THESIS_1.md §4) and is not re-measured. README outline: (b) is dropped.

## 2026-10-28 — Abort of the final run, main updated, BENCH3: seven frameworks on SQLite (Jonas)

- **Abort:** Jonas aborted the final run (9769757), "because I find it set up too directly as competition to Laravel". Cleaned up:
  result folder `target/bench-tmp/9769757-20261028T084118` (run 1 up to BM2, aborted in THESIS; the hook did not run, no document
  contains numbers from it), target folder `tmp/bm2-laravel-frankenphp-core-…`, `final-run.log`; the leftover FrankenPHP server
  (process group 4139479) terminated with SIGTERM, ports free. Older leftovers in `bench-tmp/tmp/` (2026-10-27, `dbbase-*` and others) not touched.
- **main** fast-forwarded to 9769757 (Jonas: "commit and push once beforehand and also merge to main"). 24 of the 49 commits still carry
  `Co-Authored-By` lines from the time before the rule (mornings of 2026-10-26/27); they were already public on vm-core, the history
  was not rewritten (no force push without approval). Since the rule all commits are without.
- **BENCH3** (Jonas' task "BENCH2: comparison with AdonisJS, Ruby on Rails, Django, Spring Boot and Phoenix"; the ID BENCH2 is taken by
  the preparation of the final run): five more frameworks by the same method, same cores, database, durability,
  endpoints and responses; Laravel remains the main statement, Spring Boot is marked as the upper JIT reference.
- **Scope (Jonas on inquiry):** "all frameworks including OpenGrid shall be fully benchmarked only with SQLite." So (a)
  and (b) for all seven, all on SQLite 3.45.2. For OpenGrid that means: model and token auth layer on SQLite (so far only SurrealDB);
  D25 (no SurrealDB in the benchmark) stays. Laravel targets as in 70ba137, i.e. the standard targets (Eloquent/Sanctum on SQLite) again too.
- **Addendum (Jonas, right after):** "no, for the other frameworks please also work only with raw queries without ORM overhead." Hence
  for **all seven** frameworks only the framework core with raw SQL on SQLite 3.45.2 applies: (a), THESIS (hydrate into simple classes of the respective
  language from the raw rows, like `NoteView` in OpenGrid; collect with the language's means; validate with the framework's validator; json with
  its serializer), concurrency, memory, cold start. (b) standard setups is dropped for all; OpenGrid needs no ORM on SQLite.
  The token check is on all sides the same raw SQL check as `BenchToken` (write at most once per second), password check
  at login bcrypt cost 10 everywhere.
- **Runtimes (state 2026-10-28, pinned):** Node 24.22.1 (LTS "Krypton"), Ruby 4.0.9 (YJIT), Python 3.15.2 (own build, dynamically linked against
  SQLite), Java 25 LTS (Eclipse Temurin 25.0.5+8), Erlang/OTP 29.1.2 (own build, JIT), Elixir 1.20.4 (for OTP 29); all into
  `~/.local/opt/`, checksums in the installation script `bench/frameworks/install_runtimes.sh` (Erlang and Elixir checked against the published
  SHA256 files, Node against `SHASUMS256.txt`, Ruby against `index.txt`, Temurin against the Adoptium API, Python: hash of the HTTPS download).
- **Addendum (Jonas):** "can we also include NestJS and Next.js in the benchmark, because many have liked that on GitHub?" Yes:
  **NestJS** (Express adapter as default, Fastify as a marked variant; `ValidationPipe` with class-validator) and **Next.js** (route
  handlers under `app/api`, `next build`, `next start`; validate with zod, since Next.js has no validator of its own; marked in the tables as
  "React framework, API via route handlers"). Both on Node 24.22.1 with 4 processes (`node:cluster`) and the same
  better-sqlite3 addon against SQLite 3.45.2. Thus seven comparison frameworks next to OpenGrid and Laravel.
- **SQLite in Node:** better-sqlite3 13.0.3 built from the amalgamation of the same 3.45.2 tarball, with the library's `-D` options as
  `#define` (better-sqlite3 only sets `SQLITE_ENABLE_COLUMN_METADATA` with its own amalgamation); the prebuilt Linux binaries are
  removed (the loader takes them first otherwise). Result: `pragma compile_options` identical to the library of the other sides (checked).
- **bcrypt:** Adonis' bcrypt driver only checks its PHC format (`$bcrypt$v=98$r=10$…`), not `$2b$`/`$2y$`; the core login therefore uses
  the package `bcrypt`, which the Adonis driver itself uses, directly (`bcrypt.compare`), cost 10 as everywhere.
- **Addendum (Jonas):** "and FastAPI too, gladly, because it is even more popular than Django." Yes: **FastAPI** on CPython 3.15.2, raw SQL via
  the `sqlite3` module (loads 3.45.2), validate with Pydantic (FastAPI's default), Uvicorn with 4 workers as default, Granian with 4 workers as a
  marked variant; the comparison is against the faster server. Thus eight comparison frameworks: AdonisJS, NestJS, Next.js, Rails,
  Django, FastAPI, Spring Boot, Phoenix.
- **Addendum (Jonas):** "if it is not too much work, .NET too, to take the Microsoft camp into account." Yes: **ASP.NET Core**
  (Minimal APIs) on the current .NET LTS, Kestrel as one process, limited to 4 cores (`DOTNET_PROCESSOR_COUNT=4`, `taskset`), raw SQL via
  `Microsoft.Data.Sqlite` with the SQLitePCLRaw provider for the system library (loads 3.45.2 via `LD_LIBRARY_PATH`; the default bundle
  brings its own SQLite), validate with the built-in Minimal API validation (DataAnnotations). Thus nine comparison frameworks:
  AdonisJS, NestJS, Next.js, Rails, Django, FastAPI, Spring Boot, ASP.NET Core, Phoenix.
- **Mishap (2026-10-28):** cf7db20 committed `bench/nest-app/node_modules` and `dist` along (14,846 files of third-party packages and build output; the
  Nest generator ran with `--skip-git` and wrote no `.gitignore`) and was pushed to `vm-core`. Content checked: only third-party packages and
  build files with the container path `/home/dev/…`, no keys, no personal data. b9b07de takes them out again and adds the
  `.gitignore`; since then the pre-commit hook rejects staged dependency and build directories (also via `tools/commit.sh`).
  The files remain in the history of `vm-core` (rewriting only with force push and approval); the new repo (REPO1) begins anyway with
  a cleaned-up first commit.
- **State BENCH3 (2026-10-28):** finished and checked with `api.sh` (22 steps, response of the rule check byte-identical with OpenGrid and Laravel):
  AdonisJS (6b1725e), FastAPI with Uvicorn/Granian (5967f1d), Django with Gunicorn/Granian (20b421e), NestJS with Express/Fastify (cf7db20,
  b9b07de). All runtimes installed (Node, Ruby/YJIT, Python, Java, .NET 10.0.14, Erlang/OTP 29 with JIT, Elixir). Open: Next.js, Rails,
  Spring Boot, ASP.NET Core, Phoenix; afterwards measurement tools (targets, variants, warm-up check, measurement header, resumable stages, report).
- **Measurement tools BENCH3 (2026-10-28):** all nine comparison apps finished and checked (Next.js 2595112, Rails 59cdefc, Spring Boot 4c6cd67,
  ASP.NET Core 8a7a84d, Phoenix 912e92c). Tools per Jonas' specification:
  - **Targets per framework** in `bench/framework/targets.py`; OpenGrid and Laravel targets unchanged (comparable with 70ba137). bm2 measures
    all server variants; THESIS, load, memory and cold start only the fastest variant per framework, determined from bm2 of run 1
    (geometric mean of all points) and fixed for the whole series (`best-<fw>.txt`), so that all three runs have the same targets.
  - **SQLite check** on every target before measuring (also OpenGrid and Laravel; both got `/api/bench/sql/sqlite` for that, not
    measured): version and compile options like the reference, otherwise abort. Exception `COMPILER=…`: FrankenPHP builds its built-in SQLite
    3.45.2 with gcc 16.2, the reference with gcc 15.2; the option only names the compiler and is logged, not compared.
  - **Warm-up rule:** `warmcheck.py` once per series (3 × 10 s, ≤ 3 %; steps 3/10/30/60 s); the longest needed warm-up applies to all
    sides and points. Probe: Spring is not yet stable after 3 s (27 %); the total duration depends heavily on it (see plan).
  - **Hot reload** of the nine apps in their framework's development mode (`hotreload.py`, marked); OpenGrid and Laravel as before in
    coldstart. For that: Spring Boot devtools as `developmentOnly` (not in the boot jar, lockfile without version change), Rails with all
    Bundler groups (production loads only its own), Phoenix also built for `MIX_ENV=dev`, Nest development output to `dist-dev`.
  - **Stages per framework**, individually repeatable and resumable after an abort (`run_all.sh --resume <dir> [--redo <pattern>]`).
  - **Report:** (a) keeps "OpenGrid / best Octane" and gets "OpenGrid / best other framework"; new (a′) ranking per endpoint with the
    fastest variant per framework and variant table, (e′) THESIS of all frameworks, (f′) G and k per framework and workload
    (regression line through p50 at 1 connection), (k) hot reload, runtimes per target, deviation and rule table
    (`bench/frameworks/DEVIATIONS.md`). Spring Boot marked as the upper JIT reference.
- **BENCH3 recut (Jonas, afternoon of 2026-10-28):**
  - **Dropped:** Laravel on PHP-FPM and RoadRunner (only Octane on FrankenPHP any more), NestJS, Next.js, Django (code removed from the repo,
    stays in the history). FastAPI only with Uvicorn, uvloop and httptools fixed (Granian is dropped).
  - **AdonisJS on Bun** instead of Node (Elysia was briefly built and discarded again: "the speed of Elysia is brutally fast, but
    it does not have the DX of Laravel"). Raw SQL via bun:sqlite instead of Lucid (knex/better-sqlite3 do not run under Bun), bcrypt via
    `Bun.password`.
  - **Goravel (Go) new**, so that the Go camp is represented: v1.18.0 with Gin, raw SQL via database/sql and mattn/go-sqlite3, Goravel's
    validation with Laravel's rules, spelled out per element (Goravel's wildcard rules do not report missing fields as "required").
  - **Staying:** OpenGrid, Laravel (FrankenPHP), Rails, FastAPI, Spring Boot, ASP.NET Core, Phoenix.
  - **Interpreter benchmark** (c1–c3, comparisons A/B/C) skipped by default ("no more benchmark for the interpreter itself"),
    can be switched on again with `run_all.sh --with-lang`.
  - **SQLite for all: 3.53.4 with Bun's compile options.** Bun has SQLite built in firmly under Linux (`setCustomSQLite` only applies under
    macOS, better-sqlite3 rejects Bun). No Bun version ever had 3.45.2 (only 3.45.0 in 1.0.23–1.1.37, Jonas: "then the runtime is too
    old"). Jonas: "then please do update … and please rebuild FrankenPHP with the new SQLite version as with Bun." Implementation:
    `build_sqlite.sh` builds Bun's amalgamation (bun-v1.4.3, officially 3.53.4 with a regenerated parser for UPDATE_DELETE_LIMIT) with Bun's
    defines; all 50 options equal to those in Bun. sqlite-jdbc to 3.53.4.0, the Rails gem, exqlite and Go linked against it.
  - **FrankenPHP from source:** the `-gnu` release too has 3.45.2 statically built in. FrankenPHP 1.13.0 is built against
    PHP 8.5.11 ZTS (`--enable-embed --disable-zend-signals`, pdo_sqlite against 3.53.4); Laravel stays outside the binary and is started as
    before by Octane. Versions at least two weeks old: therefore 1.13.0 instead of 1.13.4 and PHP 8.5.11 instead of 8.5.12. Without
    optional Caddy storage modules and file watcher (C library; Octane reloads via `octane:reload`).
  - **Consequence:** OpenGrid and Laravel numbers are no longer directly comparable with 70ba137 (different SQLite, own FrankenPHP build,
    without RoadRunner and FPM).
- **Warm-up curve instead of cold measurement (Jonas, 2026-10-28):** first "measure without warm-up time, so that the delay of the runtime itself
  flows in", after inquiry (the time until the *warm* server had not been measured so far, only start and first request):
  "rather a curve over time of how the requests get better with the different runtimes". The other measurements stay warm
  (warm-up check as before); new is the stage `curve:<fw>` with `warmcurve.py`: fresh server, 60 s load without warm-up, requests per
  second, sustained rate and "warm from" (90 %/97 %), in the report as table and SVG diagram (l). The started cold rebuild (restart per
  measurement point) is discarded, not committed.
- **Compact run and best optimizations per side (Jonas, evening of 2026-10-28):**
  - **Shortening** ("meaningful but manageable"): bm2 keeps all five operations and 1/16/64 connections, but measures only the
    default server variant per framework (Rails 1 thread and Spring virtual threads are dropped; serve.sh can still do them). THESIS:
    all four workloads, n = 0/10/100/1000, only 16 connections (G and k from four points at 16 connections). Load only `show`,
    warm-up curve only `hydrate:100` over 30 s, warm-up check only `list`. Plan: 6.8 h for three runs at 3 s warm-up (before 11.6 h).
  - **OpenGrid:** Cargo profile `perf` (fat LTO, one codegen unit, panic=abort) with `-C target-cpu=native`, PGO final build likewise
    (the instrumented build stays `profiling`, fat LTO did not fit into memory there); build 17 min, low point 2.95 GB free. Probe
    against `profiling`: list +9 %, hydrate:100 +21 %. **JIT on** (`OPENGRID_JIT=on` for all tools and the PGO training). No
    bytecode cache on disk (Jonas: cold start optimization excepted).
  - **Laravel/FrankenPHP:** php.ini via `php_ini` directives of a Caddyfile generated from Octane's stub (`--caddyfile`; `PHP_INI_SCAN_DIR`
    would have taken the extension inis away from artisan's system PHP): opcache 256 MB, `validate_timestamps=0`, `file_cache`, **JIT tracing
    128 MB**, realpath cache; Composer `--classmap-authoritative`. **`opcache.preload` off:** with PHP 8.5.11 ZTS in the FrankenPHP worker
    the start crashed twice with `zend_mm_heap corrupted`; stable without preload (switch `SERVE_PHP_PRELOAD=1` stays). For the
    hot-reload measurement `SERVE_PHP_DEV=1` (timestamp check on, no preload).
  - **Other sides:** Rails with jemalloc (`LD_PRELOAD`, package `jemalloc`), FastAPI with orjson (`default_response_class`), Spring Boot with
    the JDK 25 AOT cache (`-XX:AOTCacheOutput` at setup per thread variant, `-XX:AOTCache` at start), ASP.NET Core with ReadyToRun
    (`PublishReadyToRun`, RID linux-x64; the earlier `publish --locked-mode` was invalid, now `-p:RestoreLockedMode=true`), Goravel with
    `GOAMD64=v3`. **No Go PGO:** the app has only the measured endpoints, training on them would violate D14. Every side reports its
    optimization in `/api/bench/sql/sqlite` (aot_cache, goamd64, jemalloc, opcache_jit …), the report shows it per target.
  - **JIT finding (code and `GRD_JIT_LOG=1` under benchmark traffic):** 26 hot spots, all rejected, nothing translated (controllers,
    views, Pipeline, Collection). Reason: `analysis.rs` only translates functions with scalar/class signature and ops from the fixed
    list; strings, array writes, `foreach`, closures, method and builtin calls from translated code are missing. Jonas wants to expand Cranelift
    for the common framework tasks; proposal: profile first (`profile_core.py`), then JITD1 steps 2 (exit after an
    effect), 3 (calls, V1), 4 (strings, array writes, foreach), estimated 3–5 weeks; benchmark beforehand as the state without expansion and
    once more afterwards. Decision on the order open.
- **Goal for the JIT (Jonas, late 2026-10-28):** "optimize the JIT so far that it generates the machine code after the server start and
  can thereby at best bypass the VM completely, because the framework calls then happen natively." Not the AOT binary
  (the KPHP way; possible, but months and needs typed arrays), but the existing JIT with background compiler (JIT2c, paused)
  as the carrier. Work packages JITW1 (coverage = JITD1 steps 2–4, 3–5 weeks), JITW2 (eager mode after `load_project`, code
  shared process-wide), JITW3 (remaining ops generic), JITW4 (typed arrays, decision Jonas). Order: first BENCH3 as the state
  without expansion, then JITW1/JITW2, then BENCH3 once more. The JIT stays on in the benchmark (`OPENGRID_JIT=on`), even if today it
  translates nothing there; the report says so.
- **Addendum (Jonas, late 2026-10-28):** "Moreover it should be possible to cache the JIT code like with opcache, but with the machine code,
  so that the cold start time does not get worse because of it." → **JITW5**: relocatable code via `cranelift-object` with relocations
  (today's `JITModule` code has absolute process addresses and cannot be stored), loader at start, key per code unit from
  bytecode, class layout, build ID and JIT options, storage `.opengrid/jit-cache/`; after JITW2.
- **JITW5 dropped (Jonas, late 2026-10-28):** no machine-code cache; the JIT translates at start (JITW2), the interpreter is only the
  fallback. Jonas' reasoning: the start-up effort stays small, because the framework itself (Rust) is already compiled and only the
  dialect code is translated. Start-up costs are measured in JITW2 (coldstart.py).
