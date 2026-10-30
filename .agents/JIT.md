# JIT — Status, Switches, Scope, Rules

Status 2026-10-24, phprs `vm-core`. Decisions: D6 (`unsafe` only in `grd-jit`), D8 (strictly typed,
reference types via runtime functions, tiered), D9 (conservative order, switches, differential test).
Goal for compute-heavy tasks: with a strict type system and Cranelift, get **as close as possible** to PHP,
not faster (Jonas, 2026-10-24).

## 1. Switches (single place)

| Switch | Default | Effect |
|---|---|---|
| `GRD_JIT=0` (or `off`) | — | **Emergency switch**: no JIT anywhere; wins over everything |
| `OPENGRID_JIT=on` (or `1`) | off | JIT in the framework workers. **Off means: interpreter only**, no machine code. The pool logs `JIT: off (interpreter only)` or the mode at startup |
| `GRD_ADVANCED_JIT=on` (or `1`) | off | advanced JIT: tier 1 with `opt_level none`, background compiler with `speed`, atomic swap, process-wide cache. Off: one synchronous compilation with `speed` in the VM thread |
| `GRD_JIT_SYNC=1` | off | only with the advanced JIT: wait for tier 2 (tests, measurements) |
| `GRD_JIT_HOT=n` | 1000 calls / 2000 back-edges | threshold (tests: 1) |
| `GRD_JIT_FORCE_EXIT=all` or `=<seed>` | off | test mode (JITD2 §8c): compiled code takes every exit point, or a half chosen by seed; evaluated at compile time, part of the tier-2 key; in code `ForceExit`, `CraneliftJit::with_options` |
| `GRD_JIT_LOG=1` | off | log: compiled sites, compile time, runs and exits per code |

The `grd` binary (crate `grd-jit`) has the JIT on by default (unless `GRD_JIT=0`); the framework workers
have it off by default. On by default in the workers only after a green differential test **and** Jonas' approval.

## 2. What the JIT compiles

**Whole functions** whose parameters and return type are declared `int`, `float` or `bool` (no
defaults, no variadics, no generators), after 1000 calls; recursion and calls between compiled
functions run natively.

**Loops** (on-stack replacement at the loop header) over `int`/`float`/`bool` variables after 2000
back-edges in the code unit, when the operand stack is empty at the header.

**Arrays, read-only (JIT2b phase 1, 1a6db20):** slots and `array` parameters that the region never writes are held as
the address of their `PhpArray`; `$a[$i]` with an int key (also nested, `$m[$i][$k]`) and `count($a)` go
through the runtime functions `grd_rt_dim`/`grd_rt_count`. The element type of a read is the single kind the
interpreter has seen there (profile `types[ip]`); any other kind or a missing key leaves the code before
the op, and the interpreter performs the read itself (with warning/error). Mixed kinds at one site: the region is not
compiled. Callees compiled along with the region get their own profile.

**Objects (JITO1 step 1, D20):** whole **methods** and functions with parameters/return of class type (not
nullable), called from the interpreter through the cached method call (`CallMethodSel`, not static). In code:
`$this` (word `THIS_WORD` in the buffer), reading typed properties (`int`, `float`, `bool`, class) via
`grd_rt_prop_get`, writing `$this->p = v` with exactly the declared type via `grd_rt_prop_set`, `new C(...)` for
classes without a constructor or with one that only promotes its parameters (`grd_rt_new`, checks before
allocation). Class knowledge comes from the VM's `JitClasses` table (slots by selector, prefix layout P0). Rule V3
(JITD2 §8a): after a visible effect (allocation, property write, call of a function with an effect) no op
may exit any more, otherwise the function is not compiled. Not compiled: method calls from compiled
code (step 3), objects in loop regions, non-public properties except in `final` classes, readonly
writes, nullable types, classes with a native base, exceptions, constructors with a body.

Operations in compiled code:
- Stack/slots: `Nop`, `Pop`, `Dup`, `PushInt`, `PushTrue/False`, `PushConst` (int/float/bool), `LoadSlot`, `StoreSlot`, `IncSlot`, `CompoundSlot`
- Arithmetic: `+ - *` (int, float, mixed), `/` (always float, D10), `%`, `& | ^`, `<=>` (int)
- Comparisons: `< <= > >= === !==`
- Unary: negation, `+`, `!`, `(float)`, `(int)` on int, `(bool)`
- Jumps: `Jump`, `JumpIfFalse/True`, `JumpUnlessSS/SI` and the superinstructions `BinSS/SI/SC`
- Calls: `intdiv`, `max`, `min`, `abs` as intrinsics; user functions with a declared signature
- Objects (JITO1): `LoadThis`, `FetchPropSel`, `StorePath` (`$this->p = v`), `New` (promoting constructor)

## 3. What falls back to the interpreter

Everything else stays in the interpreter: strings, writes into arrays, string keys, method calls from compiled code, closures, `**`, `<<`/`>>`, `echo`, builtins
other than the four intrinsics, `try`, generators, functions without a fully declared scalar signature.

Exit from compiled code at run time (the interpreter then performs the operation itself — and throws
the exception if necessary, which is why exceptions are identical):
- integer overflow in `+ - *` and negation
- division or modulo by 0, `PHP_INT_MIN / -1`, `intdiv` edge cases
- in loops: entry types of the variables differ (guard), leaving the loop
- regions in which a local variable would change type are not compiled at all (the interpreter throws GRD021 there)
- whole functions: every exit re-executes the call in the interpreter; correct because every exit lies before the
  first visible effect (V3, checked statically)
- JITS1: native call depth between compiled functions above 1000 (`ST_DEEP`): the deeper levels run in the interpreter,
  compiled code is no longer entered from deeper frames until the interpreter is above that level again (protection against
  stack overflow; JIT_OBJECTS §11b)
- JITO1: property not initialized (or handle/slot invalid), per-request object limit reached, or
  the class's property defaults not yet evaluated at `new`

## 4. `unsafe` (D6)

`grd-vm`, `opengrid-framework` and `opengrid-pkg` carry `#![forbid(unsafe_code)]` (since 1709030; before that
the attribute was missing, but there was no `unsafe` code). All `unsafe` code lives in `grd-jit`:
**11 blocks**, all in `crates/jit/src/lib.rs` with a `// SAFETY:` comment (since JIT2b phase 1: three in the
runtime functions, one in `ref_val`; since JITO1 five in the object runtime functions). The runtime functions
are called directly in `runtime_tests`, also under Miri (`cargo +nightly miri test -p grd-jit --lib runtime`). Miri: green; warnings only "integer-to-pointer cast" in `grd_rt_dim`/`grd_rt_count` (array addresses as `u64` since JIT2b, permitted under Miri's default rule).

| Location | What | Justification |
|---|---|---|
| `CraneliftJit::run` | Convert the function pointer from the code slot into `extern "C" fn(*mut u64, *mut Vm) -> i64` and call it | The slot holds only fully compiled code with exactly this signature (published with Release, loaded with Acquire). The code reads and writes only words below `buf_words`, the length the VM gives the buffer; both tiers come from the same analysis, the background compiler checks the layout before swapping. Tier-1 code lives as long as the backend, tier-2 code as long as the process |
| `grd_rt_dim`, `grd_rt_count` | Read the address of a `PhpArray`, write the result into the caller's stack slot | The address comes from the VM (slot/argument, lives for the whole run, is not modified: the analysis admits no writing op and no callback into the VM) or from `grd_rt_dim` (element of such an array); `out` is an 8-byte stack slot of the compiled code |
| `grd_rt_prop_get`, `grd_rt_prop_set`, `grd_rt_new` (JITO1) | Convert `*mut Vm` into `&Vm`/`&mut Vm`, write the result into the stack slot, read the argument triples as a slice | `CraneliftJit::run` receives `&mut Vm` from the VM, which has taken the backend out of itself beforehand (`self.jit.take()`), so nothing else refers to the VM; the pointer passes unchanged through all native calls; each function forms a short-lived reference, none outlives its call. Handles and slots are checked by the safe logic in grd-vm (`Vm::jit_prop_get/set`, `jit_new`) itself; `out`/`args` are stack slots of the compiled code with 8 or 24·n bytes |
| `ref_val` | `Rc::increment_strong_count` + `Rc::from_raw` on an address from the buffer | Only addresses from `Rc::as_ptr` of live arrays end up in the buffer; the VM converts the exit stack before it modifies anything |
| `Drop for Compiler` | `ManuallyDrop::take` of the `JITModule` and `free_memory` | Only tier-1 compilers are torn down, and together with their VM; the VM is single-threaded, so no code from it is running, and the only holders of its pointers (code slots, native table) go in the same step. The process-wide cache holds only tier-2 pointers. The tier-2 compiler is never torn down |

## 5. Lifetime of machine code (release rule)

- **Tier 1** (per VM): released when the VM is torn down (worker rebuild, hot reload).
  No code from it runs before that, because the VM is single-threaded and only it executes the code.
- **Tier 2** (process-wide, advanced JIT only): **is never released.** A loop may keep running in the old code
  while the new one is swapped in; the old one stays valid.
- **Upper bound (JIT5, implemented):** Because tier-2 code is never released, the number of tier-2
  compilations per process is limited (`GRD_JIT_TIER2_LIMIT`, default 4096; each costs a cache entry and
  a few KB of code). Once the budget is used up, new hot code stays on tier 1, which is released with its VM;
  existing tier-2 compilations from the cache continue to be shared. Cache entries arise only from
  admitted compilations, so they are bounded as well. `grd_jit::tier2_stats()` returns (admitted, budget,
  cache entries), `set_tier2_limit(n)` sets the budget for embeddings.
- **Stress test (JIT5):** `crates/jit/tests/suite/stress.rs`: four worker threads rebuild their VM each round
  (like restart/hot reload) and run program variants while tier 2 swaps in the background; output
  equal to the interpreter every time, budget respected. The whole JIT suite is also correct with
  `GRD_JIT_TIER2_LIMIT=2` (only the test that forces tier 2 then fails, as expected).

## 6. Safeguards

- Probes: all 240 with threshold 1, once synchronous, once with the advanced JIT (tier 2 before the run).
- **Differential test** (`crates/jit/tests/suite/fuzz.rs`): random programs (arithmetic, comparisons,
  loops, negative modulo, division by 0, shifts, overflow at `PHP_INT_MAX`/`PHP_INT_MIN`,
  float formatting) in the interpreter, in the JIT and in the advanced JIT; output and exceptions must be
  equal. The seed is printed and configurable (`GRD_FUZZ_SEED`, `GRD_FUZZ_CASES`); suite: fixed seed,
  400 programs, more than three quarters of them with compiled code. Long runs on 2026-10-24: 5 seeds × 3000
  programs green. Bugs found: return value `int` for `: float` not widened (interpreter, fixed
  2df0b26); along the way `var_dump` formatting of large floats (fixed 7382ee7).
- **Array differential test** (`interpreter_and_jit_agree_on_array_programs`, JIT2b): int lists with individual elements
  of other kinds (float, string, null, array, bool, `PHP_INT_MAX`), nested arrays of unequal length, reads past the
  end, functions with `array` parameters; 300 programs in the suite, long runs 5 seeds × 3000 green (2026-10-25).
- Test for the asynchronous swap and the shared cache.

## 7. Why tier 2 brings nothing today

The generated code calls **no** Rust helper functions per operation: values are held typed in
Cranelift variables (`i64`, `f64`, `i8`), arithmetic and comparisons are machine instructions; calls exist
only between compiled functions. `perf` is not permitted in the container. Measured with `GRD_JIT_LOG`
and the phase times: the compiled phases of c3 (float, calls, recursion) take together about
25 ms of roughly 940 ms; the rest runs in the interpreter (sieve over arrays, objects, matrices). So less than 3 % of
the run time depends on compiled code, and `opt_level` can change little about that. More gain comes only from
reference types in the JIT (JIT2b) and the interpreter itself (PERF2).
