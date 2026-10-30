# JIT_OBJECTS — Objects, Arrays and Strings in the JIT (draft JITD1, 2026-10-26)

Draft following D16 3.2. **Status 2026-10-26 late: steps 0 and 1 built (f10f325), result and reasoning in §9/§10; steps 2/3 are decided by Jonas.** Jonas decides on the build and on whether D8 ("strictly
typed, no type guards, no deopt") is relaxed for objects. Basis: JIT.md (status JIT2b phase 1),
D8, measurements archiv/BENCHMARKS_bis_2026-10-26.md §3.4 (1825c1e) (not in this repository).

## 1. Starting point in numbers

| Measurement (c3, comparison C, profiling build) | PHP JIT | grd JIT | Factor | Share of grd |
|---|---:|---:|---:|---:|
| Sum of the phases | 173 ms | 423 ms | 2.4× | 100 % |
| `objects` (properties, methods, `new`) | 32.7 ms | 222.7 ms | 6.8× | **53 %** |
| `int_loops` (compiled today) | 105.8 ms | 157.5 ms | 1.5× | 37 % |
| `arrays` (writing; JIT2b phase 1 only reads) | 5.0 ms | 20.3 ms | 4.1× | 5 % |
| `calls`, `recursion`, `float_math` (compiled) | 29.2 ms | 22.6 ms | 0.8× | 5 % |

Without the JIT (comparison A) c3 is at 3.1×. The JIT brings the numeric parts below or close to PHP, the
object phase hardly at all (no region with objects is compiled).

**Framework (BM2):** The workers run without the JIT (`OPENGRID_JIT=on` enables it). According to DBP1 a `show` request spends
about 73 % in the embedded SurrealDB; the dialect part is controller, model hydration, middleware,
JSON. Objects in the JIT would hit only part of the remaining good 25 % there, and even that only for hot code
(threshold 1 000 calls). **Expected gain in the framework: small (estimated under 10 % per request).** The gain
lies in compute-heavy dialect code (evaluations, data preparation over objects), as c3 models it.

## 2. What can be compiled without speculation (compatible with D8)

D8 requires static types. For objects, the dialect provides more than has been used so far:

1. **Typed properties on typed receivers.** `$this->x` in a method of `C` and `$p->x` for a
   parameter/local with declared class type `C`: the property's type from the declaration, the **slot index** from the
   layout of `C`. Condition: subclasses place the parent class's properties at the same indices
   (prefix layout). **Checked (P0, 2026-10-26):** `vm/class.rs::link_class` starts a subclass with a copy of the
   parent class's property list (`rt.props = pc.props.clone()`), redeclarations replace in place; the
   prefix layout holds.
   Reading and writing go through runtime functions (`grd_rt_prop_get/set`, D8 point 2), which take over reference
   counting and the type check on write (TYP3); Cranelift only gets the index.
2. **Closed world per VM.** `load_project` loads all classes at start, none are added at run time;
   hot reload rebuilds the VM (§5). So for every method it is known whether a subclass overrides it.
   **Non-overridden methods** (and all `final`/`private` ones) can be **called directly**, without an inline
   cache and without an assumption that could later become false. Overridden methods go through a
   runtime function with a selector (`grd_rt_method_target`), which returns the code pointer of the compiled version
   or "not compiled" (then exit before the op).
3. **`new C(...)` with known `C`** via `grd_rt_new` (allocation in the object store, constructor only if it
   is compiled itself, otherwise exit before the op).
4. **Strings** as handles: length, comparison, concatenation, `===` via runtime functions; result handles in the
   buffer with a kind tag like the arrays today (`TAG_ARRAY`).
5. **Array writes** (`$a[] = v`, `$a[$i] = v`) via `grd_rt_dim_set`: copy-on-write (`Rc::make_mut`) lives in
   the runtime function, not in the generated code.

Common pattern: **Every runtime function checks first and acts afterwards.** If the operation would throw
(missing property, wrong type, overflow, `null` receiver), it returns "exit" **without** changing
anything; the interpreter then executes the op itself and throws exactly as without the JIT. This keeps exceptions and
messages identical, as today for overflow and division by zero.

## 3. Exits without deopt

Today's JIT exits **before** an op and hands over slots and operand stack in the interpreter's layout
(one region, one frame, no reordering, no inlining across frames). That carries over to objects:

- Handles in the buffer become `Val` on exit (like `ref_val` for arrays: increment the reference count, build the
  value). The VM is single-threaded and runs no GC during compiled code (no callbacks into the VM from
  the generated code); so the cycle collector never sees the handles half-way.
- Calls between compiled functions remain native calls; an exit in the callee re-executes (as today
  for whole functions) the **call** in the interpreter. That is only correct if the callee had no visible side effects before the
  exit. With objects that no longer holds (a property write before
  the exit). **Consequence:** functions that write do not exit "as a whole" but hand their own
  frame to the interpreter (exit in the middle of the function). For that the interpreter needs an entry point that
  accepts a frame from slots, stack and `ip`; loop OSR already has this in the opposite direction. That is not
  deopt in the proper sense (no reconstruction of inlined frames), but new work (§6, step 2).

## 4. What would really need deopt

Only speculation needs deoptimisation: an assumption that holds at compile time and later no longer does.

| Optimisation | why deopt | needed in the dialect? |
|---|---|---|
| Type assumption for untyped properties/locals from the profile | a value of another kind shows up | no: GRD021 and typed properties provide the types; untyped ones (`mixed`) stay interpreter |
| Monomorphic inline caches for overridden methods, inlining of the target | different receiver type | **yes, for overridden methods**; without deopt an indirect call via `grd_rt_method_target` remains (costs one call, no speculation) |
| Inlining across frame boundaries | exit has to build several interpreter frames | only for large gains on small methods (getters); replaceable by direct native calls |
| Scalar replacement (object exists only in registers) | exit has to create the object first | no for a start |
| Assumptions about class hierarchy/constants | a class is added or changes | no: closed world per VM (§2.2), hot reload rebuilds (§5) |

**Result:** For properties, `new`, strings, array writes and non-overridden methods, D8 suffices as it
is. Deopt would only enable inline caches with inlining for overridden methods and inlining across frames.
That is the larger part of the work (metadata per exit point, reconstruction of frames, invalidation) for the
smaller part of the gain.

## 5. Hot reload

- A reload rebuilds the worker VM (`watcher_reloads_on_change`); tier-1 code goes with the old VM (JIT.md §5).
- Tier-2 code is process-wide and never released; the content key covers all modules (deterministic since 17abd3b).
  Assumptions from the closed world (e.g. "not overridden") depend on the whole program
  and are therefore already contained in the key: new program, new key, new compilation. Old code
  stays unused in memory (budget JIT5).
- What would be added: the closed world must really be closed. Checkpoint P1: Are there ways to load
  classes at run time (anonymous classes do not count, they are known at compile time)? If so, a
  late load disables devirtualisation for the affected VM (counter `generation` per VM in the key).

## 6. Build steps, effort, risk

| Step | Content | Effort (estimated) | expected in c3 C `objects` |
|---|---|---|---|
| 0 | Checkpoint P1 (closed world; P0 is done) | ½ day | – |
| 1 | Handles in the buffer for objects/strings; `grd_rt_prop_get/set` for typed properties on typed receivers; `new` of known classes; exit before the op | 4–6 days | Regions with objects become compilable; property access becomes one call with an index access instead of ≈ 30–60 ns in the interpreter |
| 2 | Exit in the middle of a function (hand the frame to the interpreter), so that writing functions may be compiled | 3–5 days | Prerequisite for methods that set properties |
| 3 | Direct calls of non-overridden methods, indirect ones via `grd_rt_method_target` | 3–4 days | Method calls native (today 3.5–5× PHP in the interpreter) |
| 4 | Strings and array writes via runtime functions | 3–5 days | `arrays`, string-heavy loops |
| (5) | Deopt with inline caches and inlining | 2–4 weeks | Getter/setter-heavy code; **only after measuring 1–4** |

Estimate for 1–4: c3 `objects` from 6.8× to **2–3× PHP JIT** (the runtime functions remain calls with
reference counting; PHP's JIT accesses `zval`s directly). That is an estimate from the share of
dispatch/lookup costs in the interpreter profile, not a measurement.

**Risks:**
- **Memory safety:** handles and reference counts in the generated code. Mitigation: the generated code never touches `Rc`
  itself, all count changes live in runtime functions in `grd-jit` (D6, `// SAFETY:` at each site);
  these functions can be tested under Miri without the JIT.
- **Correctness:** the differential test must generate objects (classes with typed properties, inheritance,
  overridden and non-overridden methods, exceptions in constructors). That is an extension of
  `fuzz.rs` of about one day; without it no step (D9).
- **GC:** no collection during compiled code; as soon as a runtime function calls back into the VM (constructor in the
  interpreter), the buffer would have to count as a root. Therefore no callback in 1–3, only exit.
- **Budget/tier 2:** more compiled regions fill the budget faster (JIT5); the measurement in `tier2_stats`
  shows whether 4 096 suffice.

## 7. Recommendation

Build steps 0–3 without changing D8, each with the extended differential test and a measurement of c3 C `objects`. For the
framework, first measure whether `OPENGRID_JIT=on` with objects brings anything at all (BM2 with and without JIT) before
step 4 comes. Deopt (step 5) only when, after 1–4, getter/setter calls make up the rest of the time.

---

## 8. JITD2: revision after D19/D20 (points a–h)

### 8a. The gap between step 2 and step 3

Step 3 makes calls between compiled functions native. If the **callee** then exits (error path,
non-compilable op), the calling machine code is still waiting on the Rust stack. Today the problem does not exist,
because compiled functions are side-effect free: an exit discards the whole native call chain and re-executes the
outermost call in the interpreter. With objects (property write, `new`) that no longer works.

| Way | How | Effort | Consequences for D8 | Risk |
|---|---|---|---|---|
| **V1 Nested interpreter** | The callee hands its own frame to a nested interpreter run (`run(depth)` as for callbacks); its result returns into the waiting machine code, which continues normally | 5–8 days | none: no deopt, no reconstruction of foreign frames | During the nested run the GC may collect: the handles of **all** waiting JIT frames must be roots (root stack per JIT activation, `Rc` counts held in the buffer). The runtime function needs access to the VM while the VM's machine code lies on the stack (re-entry; `unsafe` in grd-jit with a clear rule: the waiting code holds no Rust reference into the VM) |
| **V2 Multi-frame hand-over** | On exit, interpreter frames are built for the whole native chain (slots, stack, `ip` per level), after which only the interpreter runs | 10–15 days | close to deopt: metadata per call site (return `ip`, buffer layout of the caller), reconstruction of several frames; D8 rules out deopt, that would have to be clarified | Errors in the reconstruction are silently wrong results; every call site needs metadata |
| V3 Exit only before the first effect | Only a function whose possible exits **all** lie before its first visible effect (checked statically) is called natively; then "re-execute the call" remains correct | 1–2 days | none | covers little code (every method that writes and then computes further drops out) |

**Recommendation for steps 2/3: V1**, because exits occur only on error paths (rare), D8 remains untouched and
the mechanism (nested run) already exists for callbacks. V2 only if V1 fails on the GC question.
**For step 1, V3 applies** (D20: no exit in the middle of a function), see §10.

### 8b. Invariant per exit point

For every point at which compiled code can exit: **The interpreter's state (slots, operand stack,
`ip`) is fully recoverable from the buffer alone, and the interpreter continuing from `ip` behaves
exactly as if it had computed everything itself from the start.** From this follow:
- No op is half executed at the time of the exit ("check first, then act", §8f).
- Every value the interpreter can read from `ip` on (slots, stack up to the depth at `ip`) is in the buffer in its
  interpreter form (kind tag plus bits, or handle); handles in the buffer hold a reference count.
- For whole functions (V3) additionally: no visible effect has happened before the exit, the interpreter may
  re-execute the call.
- Verification: the test mode from §8c forces every exit and compares with the interpreter.

### 8c. Forced exits (`GRD_JIT_FORCE_EXIT`)

Test mode, the only new environment variable: `GRD_JIT_FORCE_EXIT=all` exits at **every** exit point,
`GRD_JIT_FORCE_EXIT=<seed>` at a subset chosen by seed (same generator as the differential test). The
output must equal the interpreter's. The differential test additionally runs every program with `all` and with a
seed; the JIT probe runs also get a run with `all`. The switch is evaluated at compile time (the
code then gets an unconditional jump to the exit at every exit point); in a normal run it costs nothing.

### 8d. Scalar properties

For `int`, `float` and `bool` properties, dedicated runtime functions that return or accept the raw value
(`grd_rt_prop_get_i64/f64/bool`, `grd_rt_prop_set_i64/f64/bool`): no `Val`, no reference counting; the slot index is
static (P0). The general functions (`grd_rt_prop_get_ref/set_ref`) only for reference types (objects, strings,
arrays, `?T`), which return handles with a count.

### 8e. Model attributes (later step)

Models (`OpenGrid\Database\Model`) hold their attributes not in slots but via property handlers of the native
base class; step 1 does not cover them (receivers with a native base or `__get`/`__set` are not compiled).
Possible later step: a fast path via a runtime function `grd_rt_model_attr`, which calls the handler without the
interpreter. **Share of TP5:** A `show` request spends ≈ 73 % in SurrealDB (DBP1); of the rest, the
largest part goes to routing, middleware, hydration and JSON. Attribute accesses in the controller are few per request.
Estimate: **under 3 % of the request time**, and only with `OPENGRID_JIT=on`. Measure before every build (BM2 with and without JIT).

### 8f. Check first, then act

Every runtime function checks completely before it changes anything, and otherwise reports "exit" without effect:
- **Memory limit per request (MEM1a):** `grd_rt_new` checks `check_object_limit` before the allocation; above the limit
  exit, the interpreter throws the same exception.
- **Uninitialized typed properties:** reading an empty slot is an exit (the interpreter throws
  "must not be accessed before initialization").
- **readonly:** writing a readonly property is an exit, except for the first write within the scope of the
  declaring class (constructor); the check needs the class context of the compiled code, which is static.
- Type on write (TYP3), visibility (static from the context), `null` receiver: exit before the op.

### 8g. Closed world (P1), extended

- **`class_alias`** (since BI1) only creates a second name for an existing class; no new class, no new
  layout. Harmless.
- **`new $name` (dynamic class name):** is not compiled (exit before the op); step 1 compiles only `new C`
  with a literal name.
- **Late loading at run time:** `Vm::run_file`/`load_module` can add classes after start (tests, CLI tasks).
  Harmless for step 1: property accesses depend only on the prefix layout (P0), which new subclasses also respect,
  and property types are invariant in subclasses (PHP rule, checked at link time). **Only for step 3**
  (devirtualisation "not overridden") would that be an assumption that can become false; there, call only `final` and
  `private` methods directly, everything else via `grd_rt_method_target`.

### 8h. Abort criterion

The basis is c3 `objects` in comparison C under the conditions of the BENCH0 repetition (governor `performance`,
clock cap 4.5 GHz). **Abort value (proposed by Jonas, adopted): If `objects` after step 1 is not better than
4.5× PHP JIT, we stop and report**, and Jonas redefines steps 2/3. Honest forecast in advance: the hot part
of `objects` is the method calls `$p->step()` and within them `add`/`scale` (step 3); step 1 can only compile the small
`Vec` methods as whole functions. Reaching 4.5× is therefore **uncertain**; the measurement decides.

---

## 9. Result JITD2 (2026-10-26 late, D20)

Points a–h from §8 are decided or implemented:

| Point | Result |
|---|---|
| a) Gap 2/3 | **V1 (nested interpreter)** recommended for steps 2/3; step 1 uses **V3** (exit only before the first effect), checked statically in the analysis |
| b) Invariant | holds for every exit point; verified by the forced exit at every point (c) in the differential test and in the probe runs |
| c) `GRD_JIT_FORCE_EXIT` | built (91ad783): `all` or seed; evaluated at compile time, part of the tier-2 key; probe run with `all`, differential tests with `all` and seed. Finding while building step 1: for ops whose runtime call has an effect (`new`, call of a compiled function), the forced exit lay **after** the call and doubled the effect; it now lies before it, the real guard after it (only the test mode was affected, the real guard fires only if nothing happened) |
| d) Scalar properties | **one** read function `grd_rt_prop_get(vm, handle, slot, out) -> kind` instead of six typed ones: same price (one call, no `Val`, no counting), the code checks the kind as with `grd_rt_dim`. Objects are handles without a reference count, `_ref` variants with counting are dropped for them; strings and arrays as property values are not in step 1 |
| e) Model attributes | not in step 1 (classes with a native base are not compiled); the estimate < 3 % of TP5 stands, measure before every build |
| f) Check first, then act | `grd_rt_new` checks the object limit (MEM1a) and the presence of property defaults **before** the allocation, otherwise returns 0 without effect; uninitialized properties: exit before the op; readonly: writing is not compiled (only promotion in the constructor via `grd_rt_new`); type and visibility static |
| g) Closed world | class table `JitClasses` from the linked classes; if classes are added (`run_file`) it is rebuilt (key: class count). Compiled code depends only on the prefix layout and on property types, which do not change after linking; `new C` uses the class id resolved at link time, `new $name` is not compiled; no devirtualisation in step 1 |
| h) Abort value | 4.5× PHP JIT for c3 `objects` (comparison C); **measured 3.7–3.9×** (before 6.9–7.2×): not missed, no abort |

## 10. Reasoning JITO1 (steps 0 and 1)

**Recommended variant for the gap between step 2 and step 3: V1.** Exits are restricted to error paths and
therefore rare; V1 leaves D8 untouched (no deopt, no reconstruction of foreign frames) and uses the nested
run that already exists for callbacks. The price is the GC question (handles of all waiting JIT frames as roots) and
a re-entry into the VM from runtime functions. V2 is close to deopt and would need a change to D8; V3 covers
too little code for step 3 (every method that writes and then computes). Steps 2/3 and any change to D8
are decided by Jonas.

**Abort value:** 4.5× (proposed by Jonas, adopted). Result: 3.7–3.9×, step 1 stays.

**Chosen implementation of step 1** (f10f325, prerequisites 91ad783, 59806db):
- Step 0 (P1): checked, see §8g; harmless for step 1, because nothing depends on "not overridden".
- Objects as `TAG_OBJECT` (handle) in the buffer, `$this` in its own buffer word; whole methods are entered from the
  interpreter via the cached method call (`CallMethodSel`), arguments of class type are checked by the VM with
  `instance_of` before entry (otherwise interpreter, which throws the TypeError).
- Class knowledge as a table `JitClasses` from the VM (slot per selector, type, visibility, readonly, final, parent class,
  promoting constructor). Properties: read all typed ones (`int`, `float`, `bool`, class, not nullable),
  public or in a `final` class from the own class; write only `$this->p = v` with exactly the
  declared type (the value stays on the stack as the result, as in the interpreter).
- `new C(...)` only for classes without a constructor or with one that exclusively promotes its parameters
  (value classes like `Vec`); int→float widening in code.
- The VM passes itself to `run` (the backend is taken out of the VM for that); the runtime functions get the
  pointer, their logic is safe code in grd-vm (`Vm::jit_prop_get/set`, `jit_new`). `unsafe` only in grd-jit
  (5 new blocks with `// SAFETY:`), tested under Miri.
- Rule V3 in the analysis: state "effect happened" per op (union over edges); after an effect no op
  may exit any more; calls of compiled functions count as an effect if the callee (transitively) allocates or
  writes. This keeps "re-execute the call in the interpreter" exact.
- Before the build: differential test for objects (59806db, classes with typed properties, inheritance, overridden and
  inherited methods, throwing constructor, readonly, uninitialized property; per program interpreter, tier 1,
  tier 2, GC threshold 1, `ForceExit::All`, seed), green before and after the build; since step 1 with a minimum number
  of compiled units. Found along the way: two interpreter gaps (ccac03e) and the test-mode bug from §9c.

**Rejected alternatives:**
- Reading slots directly from the generated code (address of `PhpObject::slots`): needs a fixed memory layout of
  `Vec<Option<Val>>` and pointers into VM structures, i.e. `unsafe`-like promises in grd-vm (D6); the call costs little
  compared to the rest.
- VM pointer via a thread-local variable instead of as an argument: hidden state and cost per access.
- Constructors with a body as native callees: an exit in the constructor after the allocation violates V3; that
  needs V1 (step 2).
- Six typed read functions (§8d): same price as one with a kind return.
- Objects in loop regions: the exit could return handles, but the entry would need class guards per
  slot; postponed until a measurement demands it.
- Type assumptions from the profile (speculation): ruled out by D8 and not needed, the types are declared.

**Measurement** (c3, comparison C, profiling build, cores 8–11, governor `performance`, clock cap 4.5 GHz, 7 runs each,
two alternating rounds; `before` = 59806db, `after` = f10f325):

| Phase | PHP JIT ms | grd JIT before ms | grd JIT after ms | Factor before → after |
|---|---:|---:|---:|---:|
| `objects` (round 1 / 2) | 27.2 / 27.5 | 196.6 / 190.3 | 105.6 / 102.5 | **7.2× → 3.9× / 6.9× → 3.7×** |
| Sum of the phases (round 1 / 2) | 147.1 / 146.9 | 369.5 / 358.5 | 276.1 / 272.8 | 2.5× → 1.9× / 2.4× → 1.9× |
| remaining phases | – | – | – | unchanged (±0.1×) |

Comparison A (without JIT) unchanged: `objects` 197.4 ms before, 201.7 ms after (+2 %, within the scatter).
The forecast from §8h ("uncertain") was too cautious: the largest part of the object phase was not the
method calls themselves but body and allocation of `add`/`scale` (two property reads, arithmetic, `new` with a
constructor frame); these now run as machine code, `Particle::step` and the loop remain interpreter.
Step 3 (method calls from compiled code) would hit `step` and the loop.

---

## 11. V1 in depth (Jonas 2026-10-27, document and one test only; steps 2/3 after UI1)

V1 means: compiled code calls, for a callee that exits (or for an op it cannot handle), a
**nested interpreter run** (`Vm::run(depth)`, as callbacks and magic methods do today), and its
result returns into the waiting machine code.

### 11a. GC during the nested run

**Today:** The cycle collector (MEM1b) runs only at safepoints of the interpreter and only when no "unsafe"
nested run is active (`gc_safepoint`: `unsafe_runs == 0`). Every run that Rust code starts without setting
`gc_safe_next` beforehand (callbacks from builtins, `__toString`, `call_method_sync`) increments `unsafe_runs`; the
reason is the same as here: values in Rust locals are not roots. Compiled code today never calls back into the
VM, so there is no collection during compiled code.

**Decision for V1:** The nested run from compiled code is an **unsafe run** (no collection while
it lasts), not a run with registered roots. Reasoning: (1) Objects are handles without a reference count; a
handle in the buffer stays valid as long as nothing is collected, nothing more is needed. (2) The mechanism exists and is
tested (`gc.rs`, `probes_survive_constant_collection`, JIT probe run with GC threshold 1). (3) Registering roots
would mean making the buffers and registers of all waiting JIT frames enumerable (stack maps per call site) – that is the
expensive part of deopt infrastructure that D8 wants to avoid. **Price:** A long nested run (e.g. a loop
in the interpreter under a compiled caller) does not collect; memory grows until the end of the run, bounded
by the per-request object limit (MEM1a), which is checked before every allocation. Way out, if that proves disruptive in measurements:
copy the buffer contents of the waiting code into a root stack of the VM before the nested run (handles only,
kind tags are known), write them back afterwards; only then.

**Test plan:**
- Extend the differential test for objects (`interpreter_and_jit_agree_on_object_programs`) with programs in which a
  compiled method calls a non-compilable one (e.g. with `try` or string concatenation) that creates and
  discards objects, under **GC threshold 1**; compare the output including `spl_object_id` and `var_dump` (object ids).
- The same programs with `ForceExit::All` and seed: every exit then leads into a nested run.
- A counting test: during a nested run from compiled code no collection takes place (`gc_runs` equal before/after),
  after its return at the next safepoint again.
- Miri for the new runtime function (`grd_rt_call_interp`), as for the object functions.

### 11b. Rust stack and depth limit

**Today (checked 2026-10-27, result see below):** Interpreter calls use no Rust stack (frames on the heap);
only nested runs do, and they are limited to `MAX_NESTED_RUNS` (Error "Maximum call stack size reached", test
`native_recursion_throws_instead_of_aborting` with an 8 MB stack). **Native calls between compiled functions**
(recursion like `fib`), by contrast, run on the machine stack: per level one frame of the generated code plus the
callee's state buffer as a stack slot. Worker threads have a 64 MB stack, the main thread of `grd` 8 MB.

**Result of the check (2026-10-27, JITS1):** There was **no** depth limit for native calls. The new test
`depth::deep_compiled_recursion_does_not_overflow_the_native_stack` (recursion `down(200000)`, mutual recursion
`even/odd(150001)`, float recursion `sum(0.0, 100000)` with JIT, threshold 1, in a thread with a 2 MB stack) **aborted the whole
process** ("has overflowed its stack", SIGABRT); in a worker that would have terminated the server. **Fixed:** Compiled functions
receive the native call depth as a third argument (`fn(buf, vm, depth)`), every native call passes on `depth + 1`, and a
whole function reports `ST_DEEP` on entry beyond `MAX_NATIVE_DEPTH = 1000` (before any effect). The status propagates upward through all
waiting compiled frames; the VM sets `jit_deep_mark` to the frame depth of the call and enters no compiled code from deeper
frames until the interpreter is above that level again. The deeper levels run in the interpreter (frames
on the heap); without the mark every level would recurse natively up to the limit again (O(n·L)). Cost: c3 C `recursion`
5.8 → 6.1 ms (+5 %, still 0.7× PHP JIT), otherwise unchanged.

**For V1:** A nested run from compiled code counts like any other against `MAX_NESTED_RUNS` and against the
native depth (section above); both limits throw or fall back to the interpreter before the stack runs out.

### 11c. Exceptions from the nested run

If the nested interpreter run throws (and does not catch it itself), `Err(Throw)` arrives at the runtime function.
Intended path:
1. The runtime function stores the exception in the VM (`pending_throw`) and returns a new status
   `ST_THROW` to the machine code.
2. Compiled code catches nothing (functions with `try` are not compiled); every waiting compiled frame passes
   `ST_THROW` upward unchanged, without further effect (no op after the call runs).
3. `jit_enter` sees `ST_THROW`, takes the exception from the VM and returns it as `Err` to the interpreter site that
   entered the compiled code; there the normal unwinding (`unwind`) runs with the interpreter's `catch`/`finally`.
4. **Effects** before the exception remain, as in the interpreter (no rollback). This distinguishes `ST_THROW` from
   `Bailout`: after `ST_THROW` **nothing** is re-executed; V3 does not apply to this path, because no repetition takes place.
5. **Deviation to be solved:** The exception's trace is built at throw time from `self.frames`; compiled frames
   are not in it. `getTrace()`/`getLine()` would omit the compiled callers. Solution before the build: the
   compiled code creates, on entering the nested run, a light placeholder frame per waiting
   compiled level (unit, line of the call site from the line table), or the differential test compares traces
   and the build waits until they are equal. Test plan: differential test with exceptions in non-compiled callees under
   compiled callers, comparing class, message, `getLine()` and `getTraceAsString()`.
