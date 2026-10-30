# Tools.md – Tests, Probes and Tools of OpenGrid

As of 2026-10-26, phprs `vm-core` @ `2886d27`. Purpose (Jonas, D17): someone who does not know the code should, after reading,
know what every test checks, how it is written, **where its expectation comes from**, and what a green suite can and
cannot be relied on for. This document only describes; it changes no test. Weak, wrong or missing tests are listed in
§4 "Findings" (the starting point for TEST1).

The list of tests is generated from the code by `phprs/tools/tests_inventory.py`; the descriptions are written by hand.
`python3 tools/tests_inventory.py --check ../.agents/Tools.md` reports tests without a description and descriptions
without a test. Every test id appears below in backticks in the form `crate:path::function`, data groups as `group:<name>` (e.g. `group:probes`).

## 1. Overview

### 1.1 Kinds of checks and how they work together

There are seven kinds of checks (terms in §1.2):

1. **Unit tests** next to the code (`#[cfg(test)]` in `crates/*/src`), 70 of them: pure functions and data structures
   (array keys, conversions, schema parser, routes, validation rules).
2. **Integration tests**, one binary `crates/*/tests/suite.rs` per crate (D7), 98 of them: VM with prelude, the
   framework's worker pool, embedded SurrealDB (`mem://`, `surrealkv://`) and a throwaway SurrealDB server.
3. **Probes**: 259 scripts with PHP reference output, executed four times (interpreter, interpreter with GC
   threshold 1, JIT tier 1, JIT tier 2) and re-checked against PHP itself on every run (TEST2.3).
4. **Specification tests against external oracles**: type matrix against PHP `strict_types` (1301 cases), support classes against
   Laravel's `Illuminate\Support` (607), validation against Laravel's validator (132), inheritance and declaration checks
   against PHP's messages (136), schema port against the PHP classes of a private reference implementation (16 edge cases + real files).
5. **Differential tests** of the JIT: 700 random programs, interpreter against JIT tier 1 against JIT tier 2.
6. **Acceptance test** `api.sh`: 45 HTTP steps against a real `opengrid` server (in the suite), the same
   steps against the Laravel replica (by hand only).
7. **Benchmarks** with checksums (`bench.py`, `bm2.py`): measurements, not tests; the only correctness check is the
   checksum comparison with PHP (c1–c3) or the count of non-2xx responses (BM2).

**Workflow in interpreter development** (current practice):

```
Code change
  │  while working: targeted  cargo test -p <crate> --test suite <module>
  │                 behavior against PHP:  php -r '…'  or grd run file.grd
  ▼
before every commit:  commit.sh → green.sh → cargo test --workspace (all 168 tests, ≈ 2.5 min with build,
  │                   ≈ 50 s pure runtime sequentially); red = no commit; "Suite: N green" in the message
  │                   (commit.sh lives in the session scratchpad, is NOT versioned, no git hook, no CI; §3)
  ▼
after JIT changes:    additionally long differential runs by hand (GRD_FUZZ_SEED/GRD_FUZZ_CASES, e.g. 5 × 3000)
after performance changes: Callgrind and bench.py (c1–c3, checksums), bm2.py (HTTP); results in BENCHMARKS.md
after framework packages: regression guard (D16, short BM2 run show/create, > 10 % deviation is reported)
when needed:          tools/phpt/run.py (php-src probes, R1), tools/dx/r2.py (Laravel docs, R2), grd check --report (DX1),
                      api.sh --app laravel (Laravel fidelity of the HTTP responses)
before a release:     full benchmarks (A/B/C, BM2 all targets), PGO binaries pass api.sh and all probes (PGO1)
```

**Table of groups** (runtime: measured 2026-10-26, `cargo test --workspace -- --test-threads=1`, without build):

| Group | Count | Runtime | Location | Oracle | in the suite |
|---|---:|---:|---|---|---|
| Unit tests grd-vm | 20 | < 0.01 s | `crates/vm/src` | own expectation; 1 × test vectors (RFC/FIPS) | yes |
| Integration tests grd-vm (without probes, types, support) | 23 | 0.5 s | `crates/vm/tests/suite/` | own expectation; inheritance checks: PHP messages | yes |
| Probes (`group:probes`) | 258 × 4 runs | 2.4 s | `crates/vm/tests/probes/` | PHP 8.4 (255, re-checked on every run), dialect (4, `probes_deviations.txt`) | yes |
| Type matrix (`group:type-matrix`) | 1301 + 82 + 140 | 0.15 s | `crates/vm/tests/types/`, `suite/types.rs` | PHP strict (1301, 73 ops), dialect (9 ops, 140 GRD021) | yes |
| Support specification (`group:support-cases`) | 607 | 0.18 s | `crates/vm/tests/support/` | Laravel's own classes 13.38 | yes |
| Unit tests grd-jit / opengrid-pkg | 1 / 2 | < 0.1 s | `crates/jit/src`, `crates/pkg/src` | own expectation | yes |
| Differential tests JIT | 400 + 300 programs | 8.3 s | `crates/jit/tests/suite/fuzz.rs` | interpreter against JIT | yes |
| JIT integration (probes with JIT, tier swap, stress) | 4 | 1.3 s | `crates/jit/tests/suite.rs`, `stress.rs` | PHP probes or interpreter | yes |
| Unit tests opengrid-framework | 47 | 0.2 s | `crates/framework/src` | own expectation (literals), partly verified PHP-conformant | yes |
| Integration tests opengrid-framework | 62 (2 helper processes `#[ignore]`) | 37 s | `crates/framework/tests/suite/` | mostly own expectation; SurrealDB, reference PHP, Laravel validator in part | yes |
| Validation specification (`group:validation-cases`) | 132 | 0.03 s | `crates/framework/tests/validation_spec/` | Laravel's validator 13.38 | yes |
| Schema golden | 16 cases + reference files | 0.8 s | `crates/framework/tests/fixtures/schema_golden/` | reference PHP (11 cases), own R4h expectation (5) | yes (comparison with the reference implementation only where it is available locally) |
| Acceptance test `api.sh` (`group:api-sh`) | 64 steps | 2.6 s | `examples/test-app/tests/api.sh` | own expectation (jq), Laravel mode by hand only | yes (OpenGrid only) |
| Fixtures (`group:vm-fixtures`) | 7 files | – | `crates/vm/tests/fixtures/` | – | via `project.rs` |
| Benchmarks c1–c3, BM2 | 3 scripts, 5 endpoints | minutes | `bench/` | checksum against PHP | no |

**What a green suite can be relied on for:**
- The 259 probes print (with and without JIT, also under constant GC) exactly what PHP 8.4 prints (re-checked with `php`
  on every run), except for four probes with a dialect rule in `probes_deviations.txt`.
- Parameter, return and property types behave like PHP with `strict_types=1` (1301 combinations, recomputed against
  PHP on every run when `php` is present).
- `Str`/`Arr`/`Collection` return the same as Laravel's own classes for 607 expressions; 132 validation cases
  the same as Laravel's validator.
- Interpreter and JIT compute the same on 700 random programs (overflow, division by 0, shifts, arrays).
- The schema port prints, for 11 edge cases and the real reference files, byte for byte what the PHP classes of the
  private reference implementation print (only on this machine, because that implementation is confidential and not part of this repository).
- The test app passes the HTTP flow (auth, CRUD, tenant separation, restart, task over the control socket).

**What it cannot be relied on for:**
- Database layer, query builder, sessions, jobs, mail, storage, error pages, middleware: expectations are
  mostly the author's literals. "Like Laravel" there mostly means "as the Laravel documentation was read",
  not "checked against Laravel". The builder is explicitly own expectation until `builder_cases.php` (B0).
- Laravel fidelity of the HTTP responses: only checked by hand with `api.sh --app laravel`, not in the suite.
- Self-skipping oracle checks: without `php`, without Laravel `vendor`, without the private reference implementation or without
  the `surreal` binary some tests report green without checking (§4.3). On this machine everything is present.
- There is no versioned commit hook and no CI; "green before every commit" relies on a session script.

**Numbers on the oracles** (per test function, data groups as one entry each; counted over the "Oracle" lines in §2):
see §1.5 at the end of this section.

### 1.2 Glossary

- **Probe:** a small script that prints language behavior via `echo`/`var_dump` and whose output is compared with a
  reference. Example: `crates/vm/tests/probes/a40_intdiv_zero.grd` catches `DivisionByZeroError` from `intdiv(1, 0)`
  and prints `div`.
- **Frozen probe:** a probe whose reference output (`.out`) was taken from PHP once and then committed to the repo.
  The test compares against the file; since TEST2.3 `probes_are_php_output` additionally checks the file against
  PHP on every run. Example: all 259 files under `crates/vm/tests/probes/`.
- **Reference output:** the expected output of a script, as a file next to it (`.out`, `cases.out`, `strict.out`,
  `expected.out`). Where it comes from decides whether a test has an independent oracle.
- **Oracle:** the authority that defines what is correct. **Independent** means: not defined by the author of the code,
  but by PHP 8.4, PHP with `strict_types=1`, Laravel's own classes, the PHP classes of a private reference implementation, SurrealDB or
  a second implementation (interpreter against JIT). **Own expectation** means: the author defined the expected value,
  following DIALECT, DECISIONS, Laravel documentation or no source. Example independent:
  `group:support-cases` (Laravel); own expectation: `opengrid-framework:tests/suite/database.rs::query_builder_like_laravel`.
- **Dialect-induced deviation:** a place where grd deliberately prints differently from PHP because DIALECT §5 requires it.
  Example: `var_dump(10 / 2)` yields `float(5)` in the dialect (D10), `int(5)` in PHP (probe `legacy_integer_arithmetic_002`).
- **Differential test:** two implementations receive the same input, their outputs must be equal; no
  implementation is considered correct in advance. Example: `grd-jit:tests/suite/fuzz.rs::interpreter_and_jit_agree_on_random_programs`.
- **Type matrix:** generated cross table of declared types × values × positions whose result (`ok` or
  `TypeError`) is dictated by PHP with `strict_types=1`. Example: `param int <- 2.0` → `TypeError` (`group:type-matrix`).
- **Specification test:** a test whose cases enumerate the desired behavior completely, usually data-driven
  (case collection + reference). Example: `opengrid-framework:tests/suite/validation_spec.rs::validation_matches_laravel` (132 cases).
- **Golden test:** compares a generated output byte for byte with a frozen file. Example:
  `opengrid-framework:tests/suite/schema_golden.rs::schema_port_matches_php_on_edge_cases` (`expected.out`).
- **Integration test:** checks several parts together (VM + prelude, worker pool + database + routing). Example:
  `opengrid-framework:tests/suite/session.rs::database_store`.
- **Acceptance test:** checks a whole product from the outside, the way a user uses it. Example: `api.sh` over HTTP
  against a real `opengrid start`.
- **Benchmark checksum:** every benchmark script prints a checksum at the end; `bench.py` aborts when PHP and
  grd deliver different values. That is the only correctness check of the benchmarks.
- **Regression guard:** short BM2 run (show and create, embedded) after every completed package; if it deviates
  by more than 10 % from the last state, this is reported before work continues (D16). Not a script yet, a procedure.

### 1.3 Probes in interpreter development

**Origin.** The 259 probes (258 until TEST2.3) under `crates/vm/tests/probes/` come from five sources (counted from file names,
git history and comparison with `.agents/feasibility/`):

| Origin | Count | Recognizable by |
|---|---:|---|
| Feasibility study, feature probes (`.agents/feasibility/features/a01–a80`) | 65 | `a<NN>_*.grd`, byte-identical with the study; 15 of the 80 not adopted |
| Feasibility study, Laravel-style probes (`.agents/feasibility/laravel_style/01–25`) | 16 | `<NN>_*.grd`; `01_interface` rewritten dialect-conformant (`$t = 0.0`), 9 of the 25 not adopted |
| Harvest of the legacy tests of the old phprs core (2026-10-24, source `.agents/feasibility/legacy/scripts/*.php`) | 157 | `legacy_*`; 2 rewritten dialect-conformant (GRD021) |
| Regression probes from the php-src runs (R1) | 9 | e.g. `range_edges`, `wordwrap_and_strcmp`, `serialize_object_references`; written by hand, **no** copies of `.phpt` (license) |
| Further own regression probes for individual commits | 11 | e.g. `float_repr`, `func_get_args`, `fused_jump_label`, `bi1_builtins` |

The "combined probes" mentioned in DECISIONS are the study (80 feature + 25 Laravel probes); there is no separate group
of that name in the directory. The php-src tests themselves (`.phpt`) run only with `tools/phpt/run.py` outside the suite
and do not end up in the repo (§3).

**How the reference output is created.** There is no script in the repo that produces the `.out`; every `.out` was
taken once from PHP 8.4 and committed ("frozen"):
- Study: `features/run.sh` and `laravel_style/run.sh` ran PHP with a `<?php` copy of the probe
  (`sed "1s/^<?grd/<?php/"`, `php` without `-d` switches) and compared live with `grd run`. What was already equal in grd
  was adopted, with PHP's output as `.out`.
- Legacy tests: `legacy/php-ref.sh` ran with `php -d display_errors=1 -d error_reporting=E_ALL`; only clean scripts
  (exit 0, no Warning/Notice/Deprecated) received an `.out`. Of 175 clean ones, 157 were equal in grd.
- New regression probes: according to the commit messages "with PHP's output".
- Opening tag: the compiler reads a leading `<?grd` like `<?php`. Since TEST2.3 all probes start with `<?grd`
  (before: 94/164 mixed; DECISIONS 2026-10-26 TEST2.3); the recheck gives PHP a `<?php` copy.

**Where they live, how they run.** `.grd` and `.out` lie in pairs under `crates/vm/tests/probes/`. Four tests execute
all 259, each with a fresh VM, output and `.out` each with `trim_end`, runtime errors appended as text:
`grd-vm:tests/suite/probes.rs::frozen_probes_match_php_output` (interpreter),
`grd-vm:tests/suite/probes.rs::frozen_probes_survive_constant_collection` (GC threshold 1: collect at every doubling),
`grd-jit:tests/suite.rs::probes_match_php_with_tier1_only` (JIT threshold 1 for calls and loops, tier 1) and
`grd-jit:tests/suite.rs::probes_match_php_with_tier2` (threshold 1, tier 2 from the background compiler, swapped in
synchronously before the run). None of the four calls PHP. Since TEST2.3 a fifth test does, `grd-vm:tests/suite/probes.rs::probes_are_php_output`:
PHP on a `<?php` copy of every probe, `.out` must be equal, except for the probes in
`crates/vm/tests/probes_deviations.txt` (name, DIALECT section, reason), where PHP must deviate; `GRD_PROBES_WRITE=1`
regenerates the remaining `.out` from PHP.

**Numbers (state before TEST2.3; since TEST2.3 `probes_are_php_output` checks this on every run; re-checked 2026-10-26 with PHP 8.4.21, `php -d display_errors=1 -d error_reporting=E_ALL`, first line rewritten to
`<?php`, comparison after removing trailing whitespace on each line):**
- **254 identical** with PHP, including warnings.
- **3 dialect-induced**: `legacy_integer_arithmetic_002` (`10 / 2` → `float(5)`, D10), `a79_return_widening`
  (`PHP_INT_MAX / 1` → float, D10), `250_stable_reverse_sorts` (sort functions return the array, DIALECT §5.6;
  PHP aborts there with `TypeError`, the `.out` corresponds to PHP's output of the in-place version).
- ~~1 deviating without a dialect rule~~ (fixed with TEST2.2d): `serialize_object_references` is now **dialect-induced**
  (DIALECT §5.3): a broken `unserialize` throws `ValueError` instead of PHP's `Warning` + `bool(false)`; the probe catches
  the exception and additionally checks four broken inputs (wrong string length, `b:2`, wrong class name length, missing
  `}`), which grd previously accepted silently (the delimiter check was missing), as well as `unserialize('')` → `false` like PHP.
- **0 rejected** (no `.out` contains a GRD code; rejections are checked by `dialect.rs`), **0 gaps** in the stock,
  because only what already matched was frozen. Gaps are recorded elsewhere: 16 runtime gaps, 1 deviation and
  1 GRD rejection in the legacy tests (`.agents/feasibility/legacy/classify.txt`), 15 + 9 study probes not adopted
  (references, `list()`, heredoc, `static fn`, `compact`, by-ref builtins, loose equality, among others).
- **Recheck of the legacy tests (TEST2.3, 2026-10-26, `legacy/run.sh` with grd e531675+):** 154 equal, 15 runtime errors, 3 deviations,
  3 GRD rejections (of 175 with reference output). Compared with before (157/16/1/1): `examples_mbstring` now runs
  (`mb_strrpos` from BI1) but deviated at `mb_strwidth` → fixed; `php8x_features_078` (`filter_var` sanitize) → fixed;
  both as probe `mb_strwidth_and_number_sanitizing`. Newly dialect-induced: `compound_assignment_001`, `examples_operators`
  (GRD021, enforced since D10), `integer_arithmetic_002` (`/` → float). Still open, deliberately out of scope: Fiber (4),
  `DirectoryIterator` (2), `file_exists` (file system), `SplStack`/`SplQueue`/`SplMinHeap`/`SplMaxHeap`/`SplPriorityQueue`
  (5; BI1 did **not** add them, only `ArrayObject`, `WeakMap`, `CachingIterator`), `soundex`, the public
  constructor `new ReflectionParameter('fn', 'name')` (the dialect builds parameters only via `getParameters()`),
  `preg_match_all` without out parameter (DIALECT §5.6, returns the matches). So nothing new ran without a code change.

**When grd prints differently from PHP** (practice according to DECISIONS, KNOWN_ISSUES, CLAUDE.md):
- **Bug in the interpreter:** entry in `.agents/KNOWN_ISSUES.md` (CLAUDE.md rule 4), fix it, add a probe with PHP output
  (examples: `dynamic_static_calls`, `fused_jump_label`, `func_get_args`).
- **Dialect-induced deviation:** only if DIALECT §5 provides for it (CLAUDE.md rule 2), with a DECISIONS entry. Probes
  are rewritten dialect-conformant where possible so that PHP's output stays the same (3 cases); where that is not possible,
  the `.out` is set to the dialect expectation (the 3 above). Rejected constructs (GRD code) are not frozen
  but checked as rejections in `crates/vm/tests/suite/dialect.rs`.
- **Out of scope** (Fiber, file system, missing SPL classes): no probe; entry in `legacy/classify.txt`,
  `.agents/PHPT_REPORT.md` or KNOWN_ISSUES.
- Who decides: the author (Claude) following DIALECT; new dialect rules only with a decision by Jonas (DECISIONS).

**Rule for changing an expectation.** There is no separate written rule for probe `.out` files. From CLAUDE.md
("PHP is the reference; deviations only if DIALECT §5 provides for them, then document them there") and from practice
(commits da03a24, 212752f) it follows: an `.out` may deviate from PHP only if a DIALECT rule with a DECISIONS entry
requires it. **Marking:** there is none in the probe files themselves; the three cases are documented only in
commit messages and, for one case, in DX_COVERAGE.md. The type matrix is different: there the nine
dialect-induced lines are listed with their DIALECT section in `ops_deviations.txt`, and the test checks that PHP deviates exactly there.

### 1.4 The other groups in brief

**Type matrix (`group:type-matrix`).** `grd-vm:tests/suite/types.rs::strict_types_match_php` generates 1301 cases from 20 types
× 14 values × 5 positions, runs them in grd and compares with `strict.out`. If `php` is present, the test recomputes
the same matrix with PHP (`declare(strict_types=1)`) on every run; `GRD_TYPES_REGEN=1` rewrites
`strict.out` from PHP. `typed_operations` checks 82 operations (`ops.grd`/`ops.out`); 73 lines must equal PHP,
the 9 dialect-induced ones from `ops_deviations.txt` must deviate exactly there. `locals_keep_their_type` checks
GRD021 on 140 assignment sequences against an own rule (`allowed()`); PHP does not know this rule.

**Differential test (JIT).** Two tests in `crates/jit/tests/suite/fuzz.rs` generate, with an own random generator
(fixed seed), 400 and 300 small programs respectively: arithmetic with edge values (`PHP_INT_MAX`, `PHP_INT_MIN`, √max),
division/modulo by 0, shifts ≥ 64, float formatting, functions with default parameters; and arrays with
elements of foreign kind, nested arrays of unequal length, reading past the end. Every program runs in the interpreter,
with tier 1 and with tier 2; the outputs including exceptions must be equal. More than 3/4 (or 1/2) of the programs
must contain compiled code, more than 1/20 an exception. Long runs by hand with `GRD_FUZZ_SEED`/`GRD_FUZZ_CASES`.
PHP is not an oracle here.

**Specification tests against Laravel.** `group:support-cases`: 607 expressions for `Str`, `Arr`, `Collection`,
`LazyCollection`, `data_get`, reference `cases.out` from Laravel's own classes (13.38) via `driver_vendor.php`; a
second test recomputes them when `vendor` is present. `group:validation-cases`: 132 rule/data cases, reference from
Laravel's real validator via `driver.php`; without recomputation in the suite.

**Golden tests of the schema port against the reference PHP.** 16 edge cases under
`crates/framework/tests/fixtures/schema_golden/cases/`; `expected.out` is the output of the PHP classes of a private reference implementation
(via `driver.php`; the classes are loaded at runtime from outside the repository, nothing is copied). For 5 cases the
deliberately deviating target `expected.r4h.out` applies (R4h, generated from the Rust output, own expectation). Two further tests
recompute the `expected.out` and the real reference files against PHP, only when `php` and the reference implementation are present.

**Integration tests of the framework.** 62 functions in one binary; a worker pool on the fixture
`crates/framework/tests/fixtures/app` or on a fresh project from `scaffold::new_project`; requests via
`dispatch::handle` without a socket (exceptions: Axum router, `api.sh`). Database `mem://`, `surrealkv://` in the
temp directory or a throwaway `surreal` server (`suite/server.rs`). Expectations mostly literals.

**`api.sh`.** See `group:api-sh`: 45 steps against a real server, in the suite via
`opengrid-framework:tests/suite/api_script.rs::test_app_api_script`; `--app laravel` against the Laravel replica by
hand only (39 steps, 6 skipped).

**Benchmarks with checksums.** `bench/complex/bench.py` measures c1–c3 against PHP in comparisons A/B/C and aborts on a
deviating checksum; `bench/framework/bm2.py` measures HTTP throughput and counts non-2xx responses, but checks no
response contents. Neither runs in the suite (§3).

### 1.5 Oracles in numbers

Over the 168 test functions (data groups not counted, they belong to their tests; classification by the
"Oracle" lines in §2, self-comparisons counted as own expectation):

| Oracle | Tests | Examples |
|---|---:|---|
| independent, external (PHP 8.4, PHP strict, Laravel, reference PHP, RFC/FIPS vectors) | 15 | probes (4 runs), type matrix, support (2), validation, inheritance (4), schema against reference (2), hash vectors |
| independent, second implementation (interpreter against JIT) | 2 | differential tests |
| invariant (sum of the increments = final value) | 2 | transactions embedded/server |
| mixed (partly independent, partly own expectation) | 15 | `typed_operations` (73/9), schema golden (11/5), `api_script`, `round_trip_*` |
| own expectation (the author's literals, following DIALECT/DECISIONS/Laravel docs) | 132 | builder, model, sessions, jobs, schema port unit tests, dialect rules |
| none (helper processes) | 2 | `db1_hold_child`, `surrealkv_child` |

Weighted by cases it looks different: the 15 externally checked tests carry 258 probes × 4, 1301 + 73 type cases,
607 support, 132 validation, 136 inheritance and 11 schema cases; the 132 tests with own expectation cover above
all the framework. Of the own expectations, the reading agents held quite a few against PHP/Laravel on 2026-10-26
and found them equal (noted in §2 as "re-checked"/"checked today"); that is not an automatic check.

## 2. The tests in detail

Per entry: what is checked, how the test is written, where the expectation comes from, how to run it. Runtimes measured on 2026-10-26 (sequentially, without build). The descriptions were drafted with the help of five reading agents and proofread by the author; claims about PHP and Laravel behavior on which a finding in §4 rests were re-checked on 2026-10-26.

### 2.1 grd-vm: unit tests, integration suite and data groups

Basis: complete reading of the test functions under `phprs/crates/vm/src/**` (`#[cfg(test)]`) and
`phprs/crates/vm/tests/suite/*.rs`, the data under `tests/{probes,types,support,fixtures}`, the git history of these
directories and of `.agents/{DECISIONS,DIALECT,DX_COVERAGE,PHPT_REPORT,ROADMAP}.md`. All integration tests
run in the single binary `crates/vm/tests/suite.rs`. Own counter-checks (read-only, PHP 8.4.21 in the container) are
noted at the respective place as "re-checked".

#### Unit tests in `src/`

#### `grd-vm:src/bytecode.rs::op_is_small`
- **Run:** `cargo test -p grd-vm --lib -- op_is_small`; runtime 0.00 s; in the suite: yes.
- **Checks:** that `Op` is at most 24 bytes and `Val` exactly 16 bytes (performance invariant from phase 5, "`Op` at 24 bytes", DECISIONS 2026-10-23).
- **Kind:** unit; `size_of` → comparison with constants.
- **Oracle:** `own expectation` (layout budget, no statement about behavior).

#### `grd-vm:src/bytecode.rs::module_round_trips_through_postcard`
- **Run:** `cargo test -p grd-vm --lib -- module_round_trips_through_postcard`; runtime 0.00 s; in the suite: yes.
- **Checks:** that a compiled `Module` (namespace, `use`, attribute, class with constant, nullable property, variadic parameters, `static` return, arrow fn, `try/finally`, generator, nullsafe, `??`) is serialized via `postcard` and read back unchanged.
- **Kind:** unit; compile → serialize → deserialize → `assert_eq!(m, back)`, plus `bytes.len() > 100`.
- **Oracle:** `independent` only in the sense of self-consistency (original against round trip via `PartialEq`); no content expectation about the bytecode.

#### `grd-vm:src/value/array.rs::key_normalisation`
- **Run:** `cargo test -p grd-vm --lib -- key_normalisation`; runtime 0.00 s; in the suite: yes.
- **Checks:** PHP's key normalization: `"1"`→1, `"-5"`→-5; `"01"`, `"-0"`, `"1.0"`, `""` stay strings.
- **Kind:** unit; `ArrayKey::from_str` → literal comparison.
- **Oracle:** `own expectation`, as a literal in the test; agrees with PHP (re-checked with `php -r 'var_dump(array_keys([...]))'`).

#### `grd-vm:src/value/array.rs::push_follows_largest_int_key`
- **Run:** `cargo test -p grd-vm --lib -- push_follows_largest_int_key`; runtime 0.00 s; in the suite: yes.
- **Checks:** that `push` numbers after the largest int key (5 → 6 → after a string key 7), `is_list` is false and `reindexed()` returns a list.
- **Kind:** unit; build array → compare key order.
- **Oracle:** `own expectation` (literal), corresponds to PHP semantics `$a[] = …`.

#### `grd-vm:src/value/array.rs::copy_on_write_only_on_write`
- **Run:** `cargo test -p grd-vm --lib -- copy_on_write_only_on_write`; runtime 0.00 s; in the suite: yes.
- **Checks:** copy-on-write: a cloned `Val::Array` shares the `Rc`, only writing separates them (3 against 2 elements).
- **Kind:** unit; clone → `Rc::ptr_eq` → write → check lengths/pointers.
- **Oracle:** `own expectation` (internal representation, DIALECT §5.5 "Copy-on-Write").

#### `grd-vm:src/value/array.rs::iterates_from_the_back_and_clears`
- **Run:** `cargo test -p grd-vm --lib -- iterates_from_the_back_and_clears`; runtime 0.00 s; in the suite: yes.
- **Checks:** backward iteration (`rev`) over packed and hashed arrays; `clear` empties and resets the next key (afterwards `push` → key 0). MUT2: covers mutations in `PhpArray::clear` and in the backward iterator.
- **Kind:** unit; build array → compare `iter().rev()` → `clear` → `push`.
- **Oracle:** `own expectation` (PHP: `array_reverse` order; after clearing, `[] =` starts at 0).

#### `grd-vm:src/value/array.rs::packed_until_a_gap_or_string_key`
- **Run:** `cargo test -p grd-vm --lib -- packed_until_a_gap_or_string_key`; runtime 0.00 s; in the suite: yes.
- **Checks:** that the array stays "packed" as long as keys are gapless (overwriting, appending at the end, removing the last element), switches to `Hash` after a push with a gap (0,1,3) or a string key; lookup with `"1"` and `-1`.
- **Kind:** unit; sequence of states → `matches!(store, …)` and key order.
- **Oracle:** `own expectation`; the visible key sequence `[0,1,3]` after `unset` of the last + push corresponds to PHP (re-checked).

#### `grd-vm:src/value/array.rs::unset_keeps_order`
- **Run:** `cargo test -p grd-vm --lib -- unset_keeps_order`; runtime 0.00 s; in the suite: yes.
- **Checks:** `remove` keeps the order, the next push takes key 3 (not 1).
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation`, PHP-conformant.

#### `grd-vm:src/value/compare.rs::strict_equality`
- **Run:** `cargo test -p grd-vm --lib -- strict_equality`; runtime 0.00 s; in the suite: yes.
- **Checks:** `identical` (=== and, in the dialect, also ==): 1 !== 1.0, "1" !== 1, null !== false, equal strings/handles/arrays identical, NaN !== NaN.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation`, corresponds to PHP's `===` (DIALECT §5.1).

#### `grd-vm:src/value/compare.rs::ordering_rules`
- **Run:** `cargo test -p grd-vm --lib -- ordering_rules`; runtime 0.00 s; in the suite: yes.
- **Checks:** ordering only between numbers or between strings: 1 < 1.5; `"10" < "9"` bytewise; int against string and null against int are errors; NaN incomparable (`None`).
- **Kind:** unit; `compare` → `Ordering`/`is_err`.
- **Oracle:** `own expectation` from DIALECT §5.1 (in PHP `"10" < "9"` is false because the comparison is numeric; `1 < "1"` is allowed in PHP). Deliberate deviation.

#### `grd-vm:src/value/convert.rs::numeric_strings`
- **Run:** `cargo test -p grd-vm --lib -- numeric_strings`; runtime 0.00 s; in the suite: yes.
- **Checks:** `parse_numeric`: leading/trailing whitespace, "leading numeric" (`"12abc"`, `"-3.5x"` with flag false), exponent, `.5`, empty/`.`/`abc` → None, `"0x1A"` → 0 (no hex).
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation`, agrees with PHP 8 (`is_numeric(" 12 ")`, `(int)"0x1A"` re-checked).

#### `grd-vm:src/value/convert.rs::float_formatting_matches_php_precision_14`
- **Run:** `cargo test -p grd-vm --lib -- float_formatting_matches_php_precision_14`; runtime 0.00 s; in the suite: yes.
- **Checks:** float to string like PHP's `precision=14` (e.g. `1e14`→`1.0E+14`, `1e13`→`10000000000000`, `0.00001`→`1.0E-5`, `-0.0`→`-0`, `1/3`→`0.33333333333333`, NAN/INF).
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation` as a literal, but taken from PHP (re-checked: `echo` in PHP 8.4 produces exactly these texts).

#### `grd-vm:src/value/convert.rs::casts`
- **Run:** `cargo test -p grd-vm --lib -- casts`; runtime 0.00 s; in the suite: yes.
- **Checks:** explicit casts (`"12abc"`→12, `"3.5x"`→3.5, `"0"` falsy, `"0.0"` truthy, empty array falsy, 3.99→3, true→"1", null→""), plus dialect rules: array→string is an error, `"5"` and `null` as a number in `+` are errors.
- **Kind:** unit; literal comparison.
- **Oracle:** mixed: casts `own expectation` PHP-conformant; `as_number`/array→string `own expectation` from DIALECT §5.2/§5.3 and DECISIONS 2026-10-23 ("arithmetic only with int/float").

#### `grd-vm:src/value/object.rs::handles_share_identity_and_clone_does_not`
- **Run:** `cargo test -p grd-vm --lib -- handles_share_identity_and_clone_does_not`; runtime 0.00 s; in the suite: yes.
- **Checks:** object handles share identity (`$b = $a`), `clone_object` creates an independent object; an invalid handle is detected.
- **Kind:** unit; `ObjectStore` directly.
- **Oracle:** `own expectation` (RUNTIME_ARCHITECTURE, object store per request).

#### `grd-vm:src/value/object.rs::engine_objects_survive_reset`
- **Run:** `cargo test -p grd-vm --lib -- engine_objects_survive_reset`; runtime 0.00 s; in the suite: yes.
- **Checks:** `reset_request` discards request objects, engine objects (enum singletons) stay valid; counters `len`/`engine_len`.
- **Kind:** unit.
- **Oracle:** `own expectation` (RUNTIME_ARCHITECTURE).

#### `grd-vm:src/value/string.rs::utf8_bytes_and_chars`
- **Run:** `cargo test -p grd-vm --lib -- utf8_bytes_and_chars`; runtime 0.00 s; in the suite: yes.
- **Checks:** `"äöü"`: 6 bytes, 3 characters, `as_str` succeeds.
- **Kind:** unit.
- **Oracle:** `own expectation` (trivial, UTF-8).

#### `grd-vm:src/value/string.rs::append_copies_only_when_shared`
- **Run:** `cargo test -p grd-vm --lib -- append_copies_only_when_shared`; runtime 0.00 s; in the suite: yes.
- **Checks:** copy-on-write for strings: a shared string is copied on append, an unshared one is extended in place (same pointer).
- **Kind:** unit.
- **Oracle:** `own expectation` (internal representation). The "in place" part depends on the buffer's capacity.

#### `grd-vm:src/compile/mod.rs::line_index_matches_counting`
- **Run:** `cargo test -p grd-vm --lib -- line_index_matches_counting`; runtime 0.00 s; in the suite: yes.
- **Checks:** that the fast `LineIndex` returns, for every offset (incl. `\r\n` and beyond the end), the same line as the naive counting `line_of`.
- **Kind:** differential test (two implementations against each other).
- **Oracle:** `independent` within the crate: reference implementation `line_of`.

#### `grd-vm:src/compile/names.rs::resolution`
- **Run:** `cargo test -p grd-vm --lib -- resolution`; runtime 0.00 s; in the suite: yes.
- **Checks:** name resolution: namespace-relative, `use` import, alias with sub-path (`Baz\Qux`), fully qualified, `self` unchanged, functions with fallback to global, declaration in the namespace.
- **Kind:** unit.
- **Oracle:** `own expectation` as a literal; corresponds to PHP's rules (fallback for unqualified functions).

#### `grd-vm:src/vm/project.rs::studly_case`
- **Run:** `cargo test -p grd-vm --lib -- studly_case`; runtime 0.00 s; in the suite: yes.
- **Checks:** module folder → namespace: `tasks`→`Tasks`, `master_data`/`master-data`→`MasterData`.
- **Kind:** unit.
- **Oracle:** `own expectation` (DIALECT §3, modules-only).

#### `grd-vm:src/builtins/hash.rs::known_vectors`
- **Run:** `cargo test -p grd-vm --lib -- known_vectors`; runtime 0.00 s; in the suite: yes.
- **Checks:** SHA-256 of `"abc"`, MD5 of the empty string, HMAC-SHA256 per RFC 4231 test case 2.
- **Kind:** unit; digest → hex → literal.
- **Oracle:** `independent`: published test vectors (FIPS 180, RFC 1321, RFC 4231).

#### Integration suite `tests/suite/`

#### `grd-vm:tests/suite/dialect.rs::rejected_constructs_carry_codes`
- **Run:** `cargo test -p grd-vm --test suite -- --exact dialect::rejected_constructs_carry_codes`; runtime 0.00 s; in the suite: yes.
- **Checks:** 30 forbidden constructs produce the expected GRD code when compiling: GRD001 (include/require), 002 (global), 003 ($$x), 004 (array()), 005 (list()), 006 (references in four forms), 007 (heredoc), 008 (static fn/function), 009 (ticks), 010 (compact/define/ob_start), 011 (goto), 012 (superglobals), 013 (static variable), 014 (eval), 015 (exit/die), 018 (backticks), 019 (array_pop/array_shift), 020 (discarded return value of sort/preg_match/usort).
- **Kind:** specification; `compile_source` → compare `e.code`.
- **Oracle:** `own expectation`: DIALECT §4.2 and DECISIONS 2026-10-23 ("no references", GRD019/GRD020). Only the code is checked, not message or hint.
- **Notes:** **TEST2.7:** plus GRD016 (`__halt_compiler`) and GRD017 (inline HTML before and after the PHP block).

#### `grd-vm:tests/suite/dialect.rs::forbidden_builtins_are_rejected_or_absent`
- **Run:** `cargo test -p grd-vm --test suite -- --exact dialect::forbidden_builtins_are_rejected_or_absent`; runtime < 0.1 s; in the suite: yes.
- **Checks:** every entry of `compile::dialect::FORBIDDEN_FUNCTIONS` is rejected at compile time (GRD010; `eval` GRD014, `exit`/`die` GRD015), the list is sorted (it is searched binarily), and 19 further builtins from DIALECT §4.2 (file system, network, sessions, processes, `header`, `setcookie` …) do not exist at runtime ("Call to undefined function").
- **Kind:** unit/integration; compile or execute per name.
- **Oracle:** `own expectation` (DIALECT §4.2 as specification).
- **Notes:** TEST2.7. `getenv` is deliberately present (helper `env()`), not in §4.2.

#### `grd-vm:tests/suite/dialect.rs::local_variables_keep_their_type`
- **Run:** `cargo test -p grd-vm --test suite -- --exact dialect::local_variables_keep_their_type`; runtime 0.00 s; in the suite: yes.
- **Checks:** GRD021 at compile time: 9 rejected retypings (int→string, int→float, float→int, `/=` on int, `.=` on int, int→null, ?int→string, typed parameter, in closure); the hint names `0.0`; 6 allowed patterns (?T after null, `+=` on float, foreach rebinding, separate scopes, `?int` parameter, `/` yields float).
- **Kind:** specification; compile → code or success.
- **Oracle:** `own expectation`: DIALECT §5.2, D10, DECISIONS 2026-10-24 22:20. PHP does not know the rule.

#### `grd-vm:tests/suite/dialect.rs::local_variables_keep_their_type_at_run_time`
- **Run:** `cargo test -p grd-vm --test suite -- --exact dialect::local_variables_keep_their_type_at_run_time`; runtime 0.02 s; in the suite: yes.
- **Checks:** GRD021 at runtime (value from an array into an int variable → TypeError with the GRD021 message), `mixed` for an unknown first type, union parameters are `mixed`, `+=` with type change, `PHP_INT_MAX + 1` → `ArithmeticError` "Integer overflow in +", `10 / 2` → `float(5)`.
- **Kind:** integration; script on `Vm::with_prelude` → output (`contains` or `assert_eq`).
- **Oracle:** `own expectation` (DIALECT §5.2, D10). The message texts are our own; PHP would return float on overflow and print `int(5)`.

#### `grd-vm:tests/suite/dialect.rs::integer_only_operators_and_numeric_increments`
- **Run:** `cargo test -p grd-vm --test suite -- --exact dialect::integer_only_operators_and_numeric_increments`; runtime 0.02 s; in the suite: yes.
- **Checks:** DIA2 (D20): `%`, `&`, `<<`, `~` and `%=` with a float operand → `TypeError` with the hint `fmod() or (int)` or `(int)`; strings in `%` → conversion hint; int cases and `fmod` unchanged; `abs(PHP_INT_MIN)` → `ArithmeticError`, `abs(PHP_INT_MIN + 1)` and `abs(-2.5)` normal; `++`/`--` on null, strings, bool → `TypeError`, on int/float normal.
- **Kind:** integration; script on `Vm::with_prelude` → whole output via `assert_eq`.
- **Oracle:** `own expectation` (DIALECT §5.2 [fixed]); PHP truncates floats, increments null/strings and returns float for `abs`. Counterpart with PHP reference: probe `dialect_numeric_edges` (in `probes_deviations.txt`, PHP must deviate there).

#### `grd-vm:tests/suite/dialect.rs::allowed_constructs_compile`
- **Run:** `cargo test -p grd-vm --test suite -- --exact dialect::allowed_constructs_compile`; runtime 0.00 s; in the suite: yes.
- **Checks:** 14 allowed constructs compile: `declare(strict_types)`, `$a = sort($a)`, `Arr::pop`, `preg_match` with used result, closures/`use`, destructuring, namespaces, enums with match, union catch/finally, nullsafe/`??=`, first-class callables, generators, attributes, promotion, nested assignments.
- **Kind:** specification; only `compile_source` must succeed, nothing is executed.
- **Oracle:** `own expectation` (DIALECT §4.1).

#### `grd-vm:tests/suite/dialect.rs::native_recursion_throws_instead_of_aborting`
- **Run:** `cargo test -p grd-vm --test suite -- --exact dialect::native_recursion_throws_instead_of_aborting`; runtime 0.02 s; in the suite: yes.
- **Checks:** endless recursion through Rust (callback in `array_map`, `__toString`) throws `Error` at the nesting limit instead of blowing the native stack; the script continues afterwards (`12`). Runs in a thread with an 8 MB stack.
- **Kind:** integration; thread → script → exact output comparison.
- **Oracle:** `own expectation`. Message modeled on PHP, but shortened: PHP 8.4 writes "Maximum call stack size of 8339456 bytes (zend.max_allowed_stack_size - zend.reserved_stack_size) reached. Infinite recursion?" (re-checked).
- **Notes:** the comment speaks of a "small stack"; 8 MB is set (default size for main threads).

#### `grd-vm:tests/suite/dialect.rs::var_export_of_a_cycle_is_an_error`
- **Run:** `cargo test -p grd-vm --test suite -- --exact dialect::var_export_of_a_cycle_is_an_error`; runtime 0.00 s; in the suite: yes.
- **Checks:** `var_export` of an object that contains itself throws `Error` "var_export does not handle circular references" (PHP: warning + NULL).
- **Kind:** integration; `ends_with` on the output.
- **Oracle:** mixed: message text from PHP (PHP's warning text, re-checked), exception instead of warning from DIALECT §5.3.

#### `grd-vm:tests/suite/gc.rs::short_lived_objects_are_collected`
- **Run:** `cargo test -p grd-vm --test suite -- --exact gc::short_lived_objects_are_collected`; runtime 0.08 s; in the suite: yes.
- **Checks:** 300 000 short-lived objects in a loop at GC threshold 10 000: result `44999850000`, GC ran, > 250 000 freed, < 30 000 live, no "opaque" objects.
- **Kind:** integration; script → output + `GcStats`.
- **Oracle:** `own expectation`: sum 0..299 999 (arithmetic), thresholds for statistics freely chosen (MEM1b).

#### `grd-vm:tests/suite/gc.rs::reachable_objects_survive`
- **Run:** `cargo test -p grd-vm --test suite -- --exact gc::reachable_objects_survive`; runtime 0.03 s; in the suite: yes.
- **Checks:** reachable things survive collections (threshold 1 000): objects in arrays, properties, self-references, linked list, closures over objects, generator with object, temporary objects in between.
- **Kind:** integration; the result is computed in the test from a formula (`3*kept+50`, chain 10, closure sum, `7 -1`).
- **Oracle:** `own expectation` (arithmetic). The module comment mentions static properties, but the script only declares the class `Registry` and does not use it.

#### `grd-vm:tests/suite/gc.rs::no_collection_inside_native_callbacks`
- **Run:** `cargo test -p grd-vm --test suite -- --exact gc::no_collection_inside_native_callbacks`; runtime 0.02 s; in the suite: yes.
- **Checks:** while `array_map` holds results in Rust, the 30 000 objects created in the callback are not collected (threshold 100): sum `450015000`.
- **Kind:** integration; output comparison.
- **Oracle:** `own expectation` (arithmetic).

#### `grd-vm:tests/suite/gc.rs::tiny_threshold_and_cycles`
- **Run:** `cargo test -p grd-vm --test suite -- --exact gc::tiny_threshold_and_cycles`; runtime 0.00 s; in the suite: yes.
- **Checks:** threshold 1 (collect at every doubling), self-referencing garbage is collected, the kept half stays intact: `999000 1000`, > 5 runs, > 500 freed, < 2000 live.
- **Kind:** integration.
- **Oracle:** `own expectation` (arithmetic, statistics thresholds freely chosen).

#### `grd-vm:tests/suite/inheritance.rs::inheritance_checks_match_php`
- **Run:** `cargo test -p grd-vm --test suite -- --exact inheritance::inheritance_checks_match_php`; runtime 0.06 s; in the suite: yes.
- **Checks:** 32 cases of the inheritance check at link time: signature compatibility (types, parameter count, defaults, variadics, `static`/`self`, covariance/contravariance), visibility, static/non-static, abstract methods (with abbreviation "..." from four on), constructors, property type/visibility/readonly, final constants, `#[\Override]`, private methods, shortened default display.
- **Kind:** specification, table-driven; script `<?grd …; echo 'ok';` → first error line normalized (prefix `error: `, `Uncaught Error: `, ` on line` removed) → comparison.
- **Oracle:** `independent` (PHP 8.4 messages), stored as literals in the test, without recheck in the test. Re-checked: all 136 cases of the four tests in `inheritance.rs` produce exactly the expected text or `ok` under `php` 8.4.21.

#### `grd-vm:tests/suite/inheritance.rs::declaration_checks_match_php`
- **Run:** `cargo test -p grd-vm --test suite -- --exact inheritance::declaration_checks_match_php`; runtime 0.05 s; in the suite: yes.
- **Checks:** 30 compile checks for declarations: duplicate modifiers, readonly without type, duplicate methods/properties/constants, enum rules, `final abstract`, signatures of the magic methods (types, argument count, static, return type) as well as two positive cases.
- **Kind:** specification, as above.
- **Oracle:** `independent` (PHP 8.4 messages, literals; re-checked, see above).

#### `grd-vm:tests/suite/inheritance.rs::type_and_return_checks_match_php`
- **Run:** `cargo test -p grd-vm --test suite -- --exact inheritance::type_and_return_checks_match_php`; runtime 0.05 s; in the suite: yes.
- **Checks:** 33 checks of type declarations (?mixed, mixed/void/never in unions, duplicate types also by case, true|false, iterable|array, object|class, ?null) and `return` against the return type (void, never, ?int with hint, generator return types, arrow fn, constructor).
- **Kind:** specification, as above.
- **Oracle:** `independent` (PHP 8.4 messages, literals; re-checked).

#### `grd-vm:tests/suite/inheritance.rs::placement_kind_and_trait_checks_match_php`
- **Run:** `cargo test -p grd-vm --test suite -- --exact inheritance::placement_kind_and_trait_checks_match_php`; runtime 0.07 s; in the suite: yes.
- **Checks:** 41 cases: duplicate class modifiers, duplicate `default`, promoted properties outside/abstract/variadic/callable/duplicate, namespace placement, reserved class names, trait/interface/class kinds in `use`/`extends`/`implements`, trait conflicts in constants and properties, `self`/`static`/`parent` without class context, Iterator+IteratorAggregate.
- **Kind:** specification, as above.
- **Oracle:** `independent` (PHP 8.4 messages, literals; re-checked).

#### `grd-vm:tests/suite/inheritance.rs::expectations_are_php_output`
- **Run:** `cargo test -p grd-vm --test suite -- --exact inheritance::expectations_are_php_output`; runtime ≈ 0.7 s (136 PHP starts on 8 threads); in the suite: yes.
- **Checks:** all 136 literals of the four inheritance/declaration tests are PHP's message: each case as a `<?php` file through `php` (`display_errors=1`); the message is taken from "Fatal error: <msg> in … on line N" (without "Uncaught Error: ") or "ok".
- **Kind:** recheck of the test expectations against an external oracle.
- **Oracle:** `independent` (PHP 8.4). Without `php`: missing oracle.
- **Notes:** TEST2.6. For this the cases live as constants (`INHERITANCE_CHECKS_MATCH_PHP` etc.) at module level. Counter-check during implementation: a deliberately wrong literal ("nonstatic") makes the test fail. State: all 136 equal PHP.

#### `grd-vm:tests/suite/names.rs::class_name_constant_follows_namespace_and_imports`
- **Run:** `cargo test -p grd-vm --test suite -- --exact names::class_name_constant_follows_namespace_and_imports`; runtime 0.00 s; in the suite: yes.
- **Checks:** `Name::class` as a compile-time string: import, alias, group import, local name, relative, fully qualified, `namespace\…`; `self/static/parent::class` at runtime; the class need not exist.
- **Kind:** integration; exact output comparison.
- **Oracle:** `own expectation` as a literal; identical with PHP (re-checked with the same script under `php`).

#### `grd-vm:tests/suite/names.rs::class_name_constant_in_global_namespace`
- **Run:** `cargo test -p grd-vm --test suite -- --exact names::class_name_constant_in_global_namespace`; runtime 0.00 s; in the suite: yes.
- **Checks:** the same in the global namespace (`Foo\Bar|Baz|Qux\Quux`).
- **Kind:** integration.
- **Oracle:** `own expectation`, identical with PHP (re-checked).

#### `grd-vm:tests/suite/native_classes.rs::dialect_model_extends_native_base`
- **Run:** `cargo test -p grd-vm --test suite -- --exact native_classes::dialect_model_extends_native_base`; runtime 0.00 s; in the suite: yes.
- **Checks:** acceptance of phase 6: a dialect class inherits from a native base class `Model` registered in the test (reads `static $table`, `$softDeletes`, `$fillable` of the subclass), static factory methods return a native `Builder` that is mutated from closures (`when`), `toSql()`/`getBindings()`, property handlers (get/set/isset), overriding `toArray()` with `parent::`, `instanceof`.
- **Kind:** integration; the test registers its own `NativeClassDef`s (Builder/Model are test code, not the framework classes) → script → exact comparison of five lines.
- **Oracle:** `own expectation` (SQL form per DATABASE_LAYER §2.1; DECISIONS 2026-10-23: "checked against own expectation", until `builder_cases.php` arrives). What is checked is the native-class API, not the real database layer.

#### `grd-vm:tests/suite/native_classes.rs::native_errors_surface_as_exceptions`
- **Run:** `cargo test -p grd-vm --test suite -- --exact native_classes::native_errors_surface_as_exceptions`; runtime 0.00 s; in the suite: yes.
- **Checks:** instantiating the abstract native class → `Error` with PHP message; strict conversion `limit('zehn')` → `TypeError`.
- **Kind:** integration.
- **Oracle:** `own expectation`; the message text corresponds to PHP's "Cannot instantiate abstract class …".

#### `grd-vm:tests/suite/native_classes.rs::profile_counts_calls_backedges_and_types`
- **Run:** `cargo test -p grd-vm --test suite -- --exact native_classes::profile_counts_calls_backedges_and_types`; runtime 0.00 s; in the suite: yes.
- **Checks:** the runtime profile counts calls (≥ 2000, function "hot"), back edges of the loop (≥ 2000) and registers the element type `int` at array accesses (JIT2b).
- **Kind:** integration; script → inspect `vm.profiles`.
- **Oracle:** `own expectation` (JIT.md); only lower bounds or "some profile contains".

#### `grd-vm:tests/suite/probes.rs::frozen_probes_match_php_output`
- **Run:** `cargo test -p grd-vm --test suite -- --exact probes::frozen_probes_match_php_output`; runtime 0.46 s; in the suite: yes.
- **Checks:** that each of the 259 probes `tests/probes/*.grd` produces exactly the output of its `.out` (see `group:probes`).
- **Kind:** golden; run every file with a fresh `Vm::with_prelude`, append the error text to the output, compare both sides with `trim_end`, collect all deviations.
- **Oracle:** predominantly `independent` (PHP 8.4 output), 4 of 259 with dialect expectation (`probes_deviations.txt`). The test itself does not call `php`; `probes_are_php_output` does.

#### `grd-vm:tests/suite/probes.rs::probes_are_php_output`
- **Run:** `cargo test -p grd-vm --test suite -- --exact probes::probes_are_php_output`; runtime ≈ 1.4 s (259 PHP starts on 8 threads); in the suite: yes.
- **Checks:** PHP 8.4 on a `<?php` copy of every probe (`-d display_errors=1 -d error_reporting=E_ALL`, stdout only, under `CARGO_TARGET_TMPDIR/probes-php`): the output must equal the `.out` (line endings and end of file without whitespace), except for the probes in `crates/vm/tests/probes_deviations.txt`; there PHP must deviate. Every line of the list needs name, DIALECT section (`§…`) and reason; a name without a probe is an error. With `GRD_PROBES_WRITE=1` the `.out` of unlisted probes are rewritten from PHP (since PERF3 also for a new probe without `.out`).
- **Kind:** differential/golden against an external oracle, recomputed every time.
- **Oracle:** `independent` (PHP 8.4 itself). Without `php` the test reports a missing oracle (`OPENGRID_REQUIRE_ORACLES=1` → red, otherwise SKIPPED).
- **Notes:** TEST2.3; since TEST2.8 262 probes, 5 in `probes_deviations.txt` (new: `comparisons_dialect`). Counter-check during implementation: removing `a79_return_widening` from the list and adding `range_edges` makes the test fail with both messages. Newly added the probe `mb_strwidth_and_number_sanitizing` (two gaps from the recheck of the legacy tests, see §1.3).

#### `grd-vm:tests/suite/probes.rs::frozen_probes_survive_constant_collection`
- **Run:** `cargo test -p grd-vm --test suite -- --exact probes::frozen_probes_survive_constant_collection`; runtime 0.45 s; in the suite: yes.
- **Checks:** the same 259 probes with GC threshold 1 (collect as soon as the live objects double); a missing GC root shows up as a wrong result or an invalid handle (MEM1b).
- **Kind:** golden, as above, with `set_gc_threshold(1)`.
- **Oracle:** as above.

#### `grd-vm:tests/suite/project.rs::loads_modules_in_dependency_order_and_runs_routes`
- **Run:** `cargo test -p grd-vm --test suite -- --exact project::loads_modules_in_dependency_order_and_runs_routes`; runtime 0.00 s; in the suite: yes.
- **Checks:** loading the fixture project `tests/fixtures/app`: module list `["tasks"]`, 4 class files, 2 route files, order module routes before global routes, instantiate the controller from Rust and call `index()` (3 keys, 2 tasks).
- **Kind:** integration; `load_project` → check summary, output and return value.
- **Oracle:** `own expectation` (DIALECT §2/§3). "Dependency order" in the name is not really checked with only one module.

#### `grd-vm:tests/suite/project.rs::class_name_must_match_path`
- **Run:** `cargo test -p grd-vm --test suite -- --exact project::class_name_must_match_path`; runtime 0.00 s; in the suite: yes.
- **Checks:** `tests/fixtures/bad`: file `Invoice.grd` declares `Receipt` → error GRD101, message names `Billing\Models\Invoice`.
- **Kind:** integration; `contains` on the error message.
- **Oracle:** `own expectation` (DIALECT §3).

#### `grd-vm:tests/suite/project.rs::module_manifests_guard_module_boundaries`
- **Run:** `cargo test -p grd-vm --test suite -- --exact project::module_manifests_guard_module_boundaries`; runtime 0.02 s; in the suite: yes.
- **Checks:** MOD1: without `module.toml` no check; manifest without dependency → GRD106; allowed with `depends`; unknown dependency → message "depends on `billing`, which does not exist"; wrong name → GRD107; `::class` strings also count as a reference.
- **Kind:** integration; writes a project to `$TMP/grd-modules-<pid>` and loads it in six variants.
- **Oracle:** `own expectation` (ROADMAP MOD1).
- **Notes:** writes to the temp directory; cleanup only at the end of a successful run (the next run deletes it beforehand).

#### `grd-vm:tests/suite/project.rs::link_errors_of_the_layout`
- **Run:** `cargo test -p grd-vm --test suite -- --exact project::link_errors_of_the_layout`; runtime < 0.1 s; in the suite: yes.
- **Checks:** the link errors of the project structure with code and text: two classes in one file (GRD102), executable code in a class file (GRD103), a parent class that nobody provides (GRD104), cyclic inheritance across two files (GRD105).
- **Kind:** integration; one throwaway project under the temp directory per case → `load_project` → error text.
- **Oracle:** `own expectation` (DIALECT §2/§3).
- **Notes:** TEST2.7.

#### `grd-vm:tests/suite/project.rs::modules_load_in_dependency_order`
- **Run:** `cargo test -p grd-vm --test suite -- --exact project::modules_load_in_dependency_order`; runtime < 0.1 s; in the suite: yes.
- **Checks:** three modules (`alpha` depends on `zeta` and `mid`): the class from `alpha` extends one from `zeta` and implements an interface from `mid`, so classes are loaded in dependency order across modules (not alphabetically); route files run per module alphabetically, the global ones last; without `mid` in the manifest GRD106 applies.
- **Kind:** integration; throwaway project → `load_project` → class order in the summary and output of the route files.
- **Oracle:** `own expectation` (DIALECT §3, MOD1).
- **Notes:** TEST2.7 (before, the fixture had only one module).

#### `grd-vm:tests/suite/statics.rs::objects_in_statics_are_rejected_enums_allowed`
- **Run:** `cargo test -p grd-vm --test suite -- --exact statics::objects_in_statics_are_rejected_enums_allowed`; runtime 0.00 s; in the suite: yes.
- **Checks:** DIALECT §5.4: object in a static property → `Error` with a fixed message, also via method, nested in an array and via `[]=`; enum cases, strings and arrays with enums are allowed; after `reset_request` the object store is empty, enum singletons in statics/constants are the same, `Status::from('a') == Status::Active`.
- **Kind:** integration; two requests on the same VM, exact comparison.
- **Oracle:** `own expectation` (DIALECT §5.4; PHP allows objects in statics).

#### `grd-vm:tests/suite/support.rs::support_classes_match_laravel`
- **Run:** `cargo test -p grd-vm --test suite -- --exact support::support_classes_match_laravel`; runtime 0.04 s; in the suite: yes.
- **Checks:** all 607 expressions from `support/cases.txt` (Str, Arr, Collection, LazyCollection, `collect`, `data_get`, `str()`) yield in the dialect the same JSON line as `support/cases.out` (see `group:support-cases`).
- **Kind:** specification, data-driven; a generated script with `try { echo json_encode(<expr>) } catch → "ERR <class>: <message>"` per line; line-by-line comparison; line-count check beforehand ("cases.out is stale").
- **Oracle:** `independent` (Laravel's own classes `Illuminate\Support` 13.38 under PHP 8.4), with documented adjustments in the driver (Arr::set/forget/push/pull without references).

#### `grd-vm:tests/suite/support.rs::support_expectations_are_laravel_output`
- **Run:** `cargo test -p grd-vm --test suite -- --exact support::support_expectations_are_laravel_output`; runtime 0.14 s; in the suite: yes.
- **Checks:** that `cases.out` still corresponds to the current Laravel output: `php driver_vendor.php <vendor> cases.txt` must produce `cases.out` byte for byte.
- **Kind:** differential test against the oracle (recheck of the golden file).
- **Oracle:** `independent` (Laravel 13.38 from `bench/laravel-app/vendor`, `composer.lock`: v13.38.0). Re-checked: output today identical with `cases.out`.
- **Notes:** needs `php` and `../bench/laravel-app/vendor/autoload.php` (outside the repo, created by `bench/framework/laravel/setup.sh`); if either is missing, the test only prints a message on stderr and **counts as passed**.

#### `grd-vm:tests/suite/types.rs::strict_types_match_php`
- **Run:** `cargo test -p grd-vm --test suite -- --exact types::strict_types_match_php`; runtime 0.08 s; in the suite: yes.
- **Checks:** generated matrix of 20 declared types × 14 values × position (parameter, return, property, promoted property, static property; `callable` only parameter/return; objects not in statics) = 1301 cases; every line "<position> <type> <- <value> ok <get_debug_type>" or "… TypeError" (see `group:type-matrix`).
- **Kind:** generated + golden; `strict_matrix(false)` on grd → comparison with `types/strict.out`; afterwards, if `php` is present, `strict_matrix(true)` under PHP against `strict.out` (stale check). `GRD_TYPES_REGEN=1` rewrites `strict.out` from PHP.
- **Oracle:** `independent` (PHP 8.4 with `declare(strict_types=1)`). Re-checked: the rebuilt generator under `php` 8.4.21 produces `strict.out` byte for byte.
- **Notes:** recheck skipped (stderr message only, test green) when `php` is missing.

#### `grd-vm:tests/suite/types.rs::typed_operations`
- **Run:** `cargo test -p grd-vm --test suite -- --exact types::typed_operations`; runtime 0.06 s; in the suite: yes.
- **Checks:** 82 operations on typed values from `types/ops.grd` (compound assignments on typed properties, ++/-- at the limits, readonly, static properties, unset/uninitialized, `??=`, destructuring, builtin arguments under strict, closures, error kinds such as `DivisionByZeroError`, `UnhandledMatchError`, `ArgumentCountError`) against `types/ops.out`.
- **Kind:** golden + differential test; grd against `ops.out`; if `php` is present: the same file with `<?php declare(strict_types=1);` under PHP, and the lines deviating from PHP must be **exactly** the 9 labels from `ops_deviations.txt` (in the same order).
- **Oracle:** mixed: 73 lines `independent` (PHP strict_types), 9 lines `own expectation` with a DIALECT section per line in `ops_deviations.txt`. Re-checked: exactly these 9 lines deviate from PHP today.
- **Notes:** PHP part skipped when `php` is missing (test green).

#### `grd-vm:tests/suite/types.rs::locals_keep_their_type`
- **Run:** `cargo test -p grd-vm --test suite -- --exact types::locals_keep_their_type`; runtime 0.01 s; in the suite: yes.
- **Checks:** GRD021 at runtime for all pairs of the 7 value kinds (null, bool, int, float, string, array, object) in three sequence forms: `Lit(a)→Dyn(b)` (a ≠ null), `Lit(null)→Lit(a)→Dyn(b)`, `Dyn(a)→Dyn(b)`; according to the code that is 140 sequences (STATUS/ROADMAP say 147). Expected per assignment "ok" or "GRD021".
- **Kind:** generated; script with one function per sequence, expectation from the Rust function `allowed()`.
- **Oracle:** `own expectation`: `allowed()` implements DIALECT §5.2 with the correction DECISIONS 2026-10-24 22:20, explicitly not from PHP (PHP does not have the rule).

#### Data groups

#### `group:probes`
**What:** 259 pairs `crates/vm/tests/probes/<name>.grd` + `<name>.out`. Every probe is a small script that prints language behavior via `echo`/`var_dump`. Executed by `probes.rs` (normal and with GC threshold 1) and in `crates/jit/tests/suite.rs` by `probes_match_php_with_tier1_only` (JIT threshold 1 for calls and loops, tier 1 only, synchronous) and `probes_match_php_with_tier2` (threshold 1, tier 2 from the background compiler, swapped in synchronously before the run). The JIT runs use the same `.out` and the same `trim_end` comparison, but without GC threshold. `bench/pgo.sh` additionally uses the probes as PGO training.

**Origin (counted from file names, git history and comparison with `.agents/feasibility/`):**

| Origin | Count | Recognizable by |
|---|---:|---|
| Feasibility study, feature probes `.agents/feasibility/features/a01–a80` | 65 | `a<NN>_*.grd`, byte-identical with the study (Python comparison); 15 of the 80 not adopted (see below) |
| Feasibility study, Laravel-style probes `.agents/feasibility/laravel_style/01–25` | 16 | `<NN>_*.grd`; 15 identical, `01_interface` rewritten dialect-conformant (`$t = 0` → `$t = 0.0`, GRD021; PHP output equal); 9 of the 25 not adopted |
| Harvest of the legacy tests of the old phprs core (commit c092bab, 2026-10-24) | 157 | `legacy_*`, source `.agents/feasibility/legacy/scripts/*.php`; 155 identical, 2 rewritten (`compound_assignment_001`, `examples_operators`, GRD021) |
| Regression probes from the php-src runs (R1) | 9 | `self_referencing_constants`, `magic_property_guards`, `cyclic_objects_output`, `panic_regressions`, `range_edges`, `wordwrap_and_strcmp`, `datetime_dst_arithmetic`, `serialize_object_references`, `http_build_query_objects` (commits according to PHPT_REPORT "one commit each with a probe against the PHP output"). Written by hand, **no** copies of `.phpt` (license PHP-3.01; probes derived from php-src are only generated under `target/refs/phpt`, never committed) |
| Further own regression probes for individual commits | 11 | `250_stable_reverse_sorts`, `a79_return_widening`, `a80_var_dump_floats` (carry `a79`/`a80` but are not the study probes of the same name), `bi1_builtins`, `builtin_type_errors`, `dynamic_static_calls`, `float_repr`, `func_get_args`, `fused_jump_label`, `perf2_int_builtins`, `static_typed_props` |

By "combined probes" DECISIONS means the study (80 feature + 25 Laravel probes); 81 come from there. There is no
separate "combined" group in the directory.

**Creation of the `.out` and freezing:** there is no script that produces the probe `.out` in the repo; the files
were taken once from PHP 8.4 and then frozen (committed). Routes:
- Study: `features/run.sh` and `laravel_style/run.sh` run PHP with a `<?php` copy (`sed "1s/^<?grd/<?php/"`,
  `php` without `-d` switches, stderr mixed into the output) and compare live with `grd run`. The probes whose grd
  output matched PHP were adopted, with PHP's output as `.out`.
- Legacy tests: `legacy/php-ref.sh` ran with `php -d display_errors=1 -d error_reporting=E_ALL`; only "clean" scripts
  (exit code 0, no Warning/Notice/Deprecated line) received an `.out`. Of 175 clean ones, 157 were identical in grd
  and were frozen with this `.out` (156 `.out` still byte-identical today, one changed for dialect reasons).
- New regression probes: according to the commit messages "with PHP's output".
- Opening tag (DECISIONS 2026-10-23 "`<?grd` as opening tag"): the compiler replaces a leading `<?grd` with `<?php`.
  94 probes start with `<?grd`, 164 with `<?php` (all 157 legacy tests and 7 newer regression probes). For PHP the
  first line of `<?grd` files has to be rewritten.

The suite test itself calls no PHP and has no recheck or regeneration switch (unlike type matrix and
support). A stale `.out` is only noticed when someone compares against PHP by hand.

**Verification against PHP:** see §1.3 (254 identical, 3 dialect-induced, 1 without PHP's warning).

**Study probes not adopted** (reasons are not recorded per file anywhere; inferred from the content and DECISIONS 2026-10-23):
references `a22–a24`, `08_references`; `list()` `a30`; heredoc/nowdoc `a51/a52`; `static fn` `a79_static_closure`;
`compact/extract` `a80_compact_extract`; by-ref builtins `a25_preg_match_out`, `a55_usort_spaceship`, `a58_sort_funcs`;
loose equality `a45_clone_eq`, `a65_loose_eq`, `18_types_strict`; presumably numeric strings in arithmetic
(`a68_preg_replace_callback`: `$m[0] * 2`) and global constants/functions (`a74`, `23_namespaces`); plus `09`, `13`–`16`, `20`.

**When grd deviates from PHP** (practice according to DECISIONS and KNOWN_ISSUES):
- Interpreter bug: record in `.agents/KNOWN_ISSUES.md` (CLAUDE.md rule 4), fix, add a probe with PHP output
  (examples: `dynamic_static_calls`, `fused_jump_label`, `func_get_args`, `a79`, `a80`).
- Dialect deviation: must be provided for in DIALECT §5; probes are rewritten dialect-conformant where possible,
  so that the PHP output stays the same (3 cases: `01_interface`, `legacy_compound_assignment_001`,
  `legacy_examples_operators`). Where that is not possible, the `.out` is set to the dialect expectation (the 3 above).
  DECISIONS 2026-10-23 "Dialect-induced deviations in the probes": they count as "dialect-induced deviations", not as
  errors, and are to be counted separately in STATUS. Rejected constructs (GRD code) are not frozen at all.
- Out of reach (Fiber frozen DIALECT §4.3, file system GRD010, missing SPL classes): not as a probe, but
  in `legacy/classify.txt`, `PHPT_REPORT.md` and KNOWN_ISSUES.

**Rule on when an expectation may change:** there is no separate written rule for probe `.out` files. From
CLAUDE.md ("PHP is the reference; deviations only if DIALECT §5 provides for them, then document them there") and the
practice in da03a24/212752f it follows: an `.out` deviates from PHP only if a DIALECT rule with a DECISIONS entry
requires it.

**Marking of dialect-induced expectations:** since TEST2.3 in `crates/vm/tests/probes_deviations.txt` (enforced by the
test). Before: **none** in the probes themselves: no comment, no marker, no
list like `ops_deviations.txt`. They were documented only in commit messages (da03a24 "two with dialect expectations",
212752f) and for one case in DX_COVERAGE.md. The test name "match_php_output" and the module comment "captured from PHP
8.4" thus applied to 255 of 258.

**Examples:** `a40_intdiv_zero.grd`: `try { intdiv(1, 0); } catch (DivisionByZeroError $e) { echo "div\n"; }` →
`.out` `div`. `legacy_bitwise_operators_001`: `echo 12 & 10, "|", 12 | 10, "|", 12 ^ 10;` → `8|14|6`.

**Feasibility harness:** `features/run.sh`, `laravel_style/run.sh` and `legacy/run.sh` + `php-ref.sh` lie outside the
repo and do not run in `cargo test`. They fed the probes **once** (copy of the matching files together with the
PHP output); there is no ongoing synchronization between harness and `tests/probes`. CLAUDE.md still lists
`features/run.sh` as a command ("80 probes against PHP"). It compares PHP's complete output with only the
first two lines of grd (`head -2`), so probes with more output lines count as failures there. New
regression probes (R1, own) exist only in the repo.

#### `group:type-matrix`
**Files:** `crates/vm/tests/types/strict.out` (1301 lines), `ops.grd` (105 lines), `ops.out` (82 lines),
`ops_deviations.txt` (9 entries + header line). Introduced in b3f289b (TYP1, 2026-10-25), adjusted in 27bf234/793c2cb (TYP3).

**Generator:** no separate file; the function `strict_matrix(php: bool)` in `tests/suite/types.rs` generates the script.
Constants `TYPES` (20: `int, float, string, bool, array, ?int, int|string, int|float, float|string, mixed, object,
iterable, callable, \stdClass, ?\stdClass, \Countable, int|null, string|array, false|int, true`) and `VALUES` (14: `1`,
`1.5`, `2.0`, `'1'`, `'abc'`, `true`, `false`, `null`, `[1]`, `new \stdClass()`, `fn () => 1`, `new Cnt()` (Countable +
IteratorAggregate), `PHP_INT_MAX`, `'strlen'`). Per type, functions `pa<i>`/`re<i>` and classes `P<i>`/`Q<i>`/`S<i>`
are generated, per combination an `attempt(label, closure)`. Count: 19 types × (11 non-objects × 5 + 3 objects × 4) + 1
(`callable`) × 14 × 2 = 1273 + 28 = 1301. No randomness, no seed. `strict.out` was written from PHP with `GRD_TYPES_REGEN=1`;
afterwards the test always compares anew against PHP when `php` is present. Distribution: 319 "ok", 982 "TypeError".
Example: `param int <- ifloat` → `param int <- ifloat ok param int <- ifloat TypeError` (2.0 is not an int under strict_types).
`return float <- int` → `ok float` (widening int→float).

Peculiarity of the line form: `echo $label, " ok ", show($f())` prints label and "ok" before `$f()` throws; that is why
TypeError lines read "X ok X TypeError". PHP does the same, the result remains unambiguous.

**`ops.grd`/`ops.out`:** hand-written operations (see `typed_operations`). `ops.out` is the dialect expectation; according to
the header comment equal to PHP strict except for the lines in `ops_deviations.txt`. The deviations with reasons:
overflow `int ++ max`, `int -- min`, `static ++ max` → `ArithmeticError` (§5.2; PHP: TypeError, because float into int property);
`numstr * 2`, `null + 1` → `TypeError` (§5.2); `undefined prop read`, `array key on int`, `string offset oob` → exception
(§5.3); `dynamic prop write` → `Error` (§4). Re-checked: PHP 8.4.21 deviates in exactly these 9 lines.
Example: `attempt('int /= 2', …)` → `int /= 2 TypeError` (result float into int property, also PHP).

#### `group:support-cases`
**Files:** `crates/vm/tests/support/cases.txt` (607 PHP expressions, one per line), `cases.out` (607 JSON lines),
`driver_vendor.php` (driver).

**Creation:** `php driver_vendor.php <vendor> cases.txt > cases.out`. The driver loads `autoload.php` of the
Laravel comparison application (`OpenGrid_Framework/bench/laravel-app/vendor`, `laravel/framework` v13.38.0 according to
`composer.lock`, with real dependencies such as doctrine/inflector), creates `Str`/`LazyCollection` as aliases and evaluates every
line with `eval('return …;')`, under `strict_types=1`. Output `json_encode` or `ERR <short class>: <message>`.
Adjustments that change the oracle: polyfills `array_first`/`array_last`/`SortDirection` (PHP 8.5 functions that
Laravel 13 uses) and a wrapper class `Arr` whose `set/forget/push` return the new array and whose `pull` returns the pair
`[value, rest]`, because the dialect has no references (DIALECT §5.1/§5.6). Strict equality in non-strict
collection methods (DIALECT §5.1) is **not** replicated; the cases therefore only show places where both yield
the same.

**Scope (prefix count):** Str 230 (incl. `Str::of`/fluent), `collect(...)` 183, Arr 112, LazyCollection 67, `data_get` 3,
rest (`str()`, `strlen(Str::random(20))`, `ctype_alnum(Str::random(40))`, immediately invoked closures for mutating
collection methods such as `splice`, `pull`, `transform`, `forget`, `getOrPut`). 8 expectations are `ERR …` lines.
Random functions are only checked via length/character class (deterministic).
Example: `Arr::pluck([['developer' => ['id' => 1, 'name' => 'Taylor']], …], 'developer.name', 'developer.id')` →
`{"1":"Taylor","2":"Abigail"}`.

Re-checked (2026-10-26): the driver against the existing vendor directory produces `cases.out` byte for byte.

#### `group:vm-fixtures`
**Files:** `crates/vm/tests/fixtures/app/` (valid modules-only project: `routes/web.grd` prints `routes:global`;
module `tasks` with `routes/web.grd` (`routes:tasks `), `app/Controllers/BaseController.grd` (abstract, `respond()` appends
`module`), `TaskController.grd` (constructor promotion with `new` as default, `index()` creates two tasks),
`Models/Task.grd` (readonly promoted `id`), `Services/TaskRepository.grd`) and `fixtures/bad/` (one file
`modules/billing/app/Models/Invoice.grd` that declares `class Receipt`). Used only by `project.rs`
(`loads_modules_in_dependency_order_and_runs_routes`, `class_name_must_match_path`). Hand-written, no
oracle data. `tools/tests_inventory.py` skips `fixtures/` when searching for Rust tests.


### 2.2 grd-jit and opengrid-pkg


#### `grd-jit:src/service.rs::budget_is_never_exceeded_under_contention`
- **Run:** `cargo test -p grd-jit --lib -- budget_is_never_exceeded_under_contention`; runtime 0.00 s; in the suite: yes.
- **Checks:** the admission function `admit_from(counter, limit)` of the tier-2 budget (JIT5): 8 threads each try 1000 times to take a slot from a budget of 100. Exactly 100 admissions must result and the counter must be exactly 100 afterwards, i.e. there is no overshoot under contention (the function uses `AtomicUsize::try_update` instead of read-then-write).
- **Kind:** unit; own `AtomicUsize` counter → 8 × 1000 calls of `admit_from` → sum of the `true` results and counter value against 100.
- **Oracle:** `own expectation` (specification of the budget from JIT.md "Upper limit (JIT5)"); the number 100 is trivially correct, an external oracle is not needed.
- **Notes:** since TEST2.4 the 8 threads start simultaneously via a `Barrier`, so that they really compete for the last slots; whether a non-atomic replacement would be noticed still depends on scheduling. `admit()` with the process-wide counter and `GRD_JIT_TIER2_LIMIT` is now checked by `grd-jit:tests/suite/stress.rs::tier2_budget_is_used_up_exactly`.

#### `grd-jit:tests/suite.rs::probes_match_php_with_tier1_only`
- **Run:** `cargo test -p grd-jit --test suite -- --exact probes_match_php_with_tier1_only`; runtime 0.47 s; in the suite: yes.
- **Checks:** all 259 VM probes (`crates/vm/tests/probes/*.grd`) run with the JIT enabled at threshold 1 (every function/loop is compiled on the first call), tier 1 only (`CraneliftJit::with_tiers(false, false)`, i.e. once with `opt_level = "speed"`), and must print exactly the `.out` file. A runtime error is appended to the output as text and compared as well.
- **Kind:** golden (data-driven); per probe a new VM with prelude + JIT → `run_file` → output (`trim_end`) against `.out` (`trim_end`); all deviations are collected and reported at the end.
- **Oracle:** predominantly `independent` (PHP 8.4 output): header comment of `crates/vm/tests/suite/probes.rs` ("captured from PHP 8.4") and DECISIONS (section on `<?grd`: "their `.out` files come from PHP"). Re-checked with `php`: see §1.3 (254 identical, 3 dialect-induced, 1 without PHP's warning). Origin of the probes: copies from `.agents/feasibility/` (157 `legacy_*` from `legacy/scripts`, 67 `a*` from `features/`, 25 numbered from `laravel_style/`, 9 individual probes). A generation script for the `.out` files is not in the repo (only `legacy/php-ref.sh` in `.agents/feasibility` for the legacy scripts).
- **Notes:** for the JIT the oracle is strictly speaking "interpreter = PHP" (the same files are checked by `grd-vm` without JIT; against PHP itself since TEST2.3). **Since TEST2.4** the test requires a minimum amount of compiled code (≥ 3 probes, ≥ 13 units) and no tier 2. The measurement during implementation showed: at threshold 1 the JIT compiled code in only **2 of 258** probes at all (2 units); most probes are OOP/string code that the JIT does not compile (it compiles only functions and loops with int/float/bool locals and, since JIT2b, array reads). The JIT probe runs thus checked almost only the interpreter. Added the probe `jit_int_float_kernels` (13 units, PHP output); **JITC1 (2026-10-26):** 3 of 269 probes (at the time; today 270) with compiled code, 15 units (`jit_int_float_kernels` 13, `a79_return_widening` 1, `perf2_int_builtins` 1); all three JIT probe runs (tier 1, tier 2, GC threshold 1) require these three probes by name (`COMPILED_PROBES`) and ≥ 15 units, so that the loss of one probe is noticed even if another is newly compiled. The actual JIT coverage comes from the differential tests (`fuzz.rs`).

#### `grd-jit:tests/suite.rs::probes_match_php_with_tier2`
- **Run:** `cargo test -p grd-jit --test suite -- --exact probes_match_php_with_tier2`; runtime 0.57 s; in the suite: yes.
- **Checks:** the same 259 probes with the advanced JIT (`with_tiers(true, true)`): tier 1 with `opt_level = "none"`, every compiled unit goes to the process-wide background compiler (tier 2, `"speed"`), and with `sync = true` the VM waits until tier 2 is swapped in before the code runs. Thus (as far as compiled) tier-2 code runs.
- **Kind:** golden, as above; additionally the background thread and the content cache (`service.rs`) are involved.
- **Oracle:** as `probes_match_php_with_tier1_only` (PHP 8.4 output; deviations see §1.3).
- **Notes:** shares the process-wide tier-2 cache and the budget (default 4096) with the other tests of the same binary. Since TEST2.4: minimum amount as above and at least 90 % of the compiled units at tier 2 (measured: 15 of 15).

#### `grd-jit:tests/suite.rs::tier2_is_swapped_in_asynchronously`
- **Run:** `cargo test -p grd-jit --test suite -- --exact tier2_is_swapped_in_asynchronously`; runtime 0.15 s; in the suite: yes.
- **Checks:** the asynchronous swap (JIT2c): a hot program (`sq()` in a double loop, 200 × 5000 iterations, result modulo 1000003) runs with `with_tiers(true, false)` and threshold 1000. (1) Output equal to that of the pure interpreter; (2) something was compiled; (3) within 30 s (polling every 5 ms) all compiled units reach tier 2; (4) a second VM with the same program gets the tier-2 code directly from the process-wide cache, without its own tier 1 (all `jit_tiers() == 2`).
- **Kind:** integration; two JIT VMs and one interpreter VM → `run_file` → output comparison and query of `vm.jit_tiers()`.
- **Oracle:** `independent` for the output (interpreter against JIT; the value itself is not stored as a literal; recomputed with `php`: `507576`), `own expectation` for the tier states (architecture from D8/JIT.md).
- **Notes:** time-dependent: polling with a 30 s deadline on a background thread. Depends on the process-wide state (budget, cache) that other tests in the same binary share (see findings). **Finding TEST2.4:** assertion (4) could not fail: the cache never hit (selector IDs of the prelude modules depended on hash order, different per VM), but the second VM got its own tier-2 code asynchronously before the assertion ran. The cache is now checked by `stress.rs::tier2_budget_is_used_up_exactly` in a separate process.

#### `grd-jit:tests/suite/depth.rs::deep_compiled_recursion_does_not_overflow_the_native_stack`
- **Run:** `cargo test -p grd-jit --test suite -- --exact depth::deep_compiled_recursion_does_not_overflow_the_native_stack`; runtime ≈ 1 s; in the suite: yes.
- **Checks:** deep recursion with the JIT enabled (threshold 1) does not overflow the machine stack: `down(200000)`, mutual `even/odd(150001)`, `sum(0.0, 100000)` in a thread with a 2 MB stack (workers have 64 MB, `grd` 8 MB); output equal to the interpreter, at least 4 compiled units.
- **Kind:** integration; two runs (without/with JIT) in their own thread, `join` must succeed (an overflow aborts the whole test process).
- **Oracle:** `differential` (interpreter) plus fixed expectation `3000 200000 false 50000`.
- **Notes:** JITS1 (JIT_OBJECTS §11b). Without the depth limit the test aborted the process with a stack overflow (checked 2026-10-27 before the fix).

#### `grd-jit:tests/suite/fuzz.rs::interpreter_and_jit_agree_on_random_programs`
- **Run:** `cargo test -p grd-jit --test suite -- --exact fuzz::interpreter_and_jit_agree_on_random_programs`; runtime 4.31 s; in the suite: yes.
- **Checks:** differential test (D9, JIT4): randomly generated small programs must produce character-for-character the same output in the interpreter, with tier-1 JIT and with the advanced JIT (tier 2, synchronous), including exceptions (class and message).
- **Kind:** generated / differential test. Flow per case: generate program → `run(src, None)` (interpreter, yields `want`) → `run(src, Some(false))` and `run(src, Some(true))` (JIT with `with_tiers(advanced, sync=true)`, threshold 1/1) → `assert_eq!(got, want)`; **since JITO1 step 0** additionally `forced_exits_agree`: tier 1 with `ForceExit::All` (every exit point is taken) and with `ForceExit::Seed(seed ^ case)` (a half chosen by seed), both must yield `want` (JITD2 §8c). An uncaught exception is appended as `\nuncaught: …` and compared as well.
- **Generator** (own random generator xorshift64*, no external crate):
  - **Seed and case count:** default `GRD_FUZZ_SEED = 0x5EED_2026_1024`, `GRD_FUZZ_CASES = 400`; both settable via the environment, the seed is printed on stderr. `GRD_FUZZ_DUMP=<text>` prints the first program whose output contains `<text>` and ends the test early (then without final checks, i.e. green).
  - **Program form:** 1–3 function pairs and 1–3 loop blocks.
    - `f{n}(int $a, int $b, float $x): int`: with probability 1/3 an inner loop `for ($k = 0; $k < 5; $k++) { $s = ($s + E) % 1000003; }`, otherwise `if (B) { return E1; } return E2;` (B = bool expression depth 2, E depth 3).
    - `g{n}(float $x, float $y, int $a [= c]): float { return F; }` (F = float expression depth 3); in every second program the third parameter has a default value (JIT2a), and calls partly omit it (those then run in the interpreter).
    - Loop block `L{l}`: `try { $p = …; $q = …; $u = …; $c = 0; for ($i = 0; $i < N; $i++) { $p = INT; $q = f…($p, $i, $u) | INT; $u = g…($u, 0.5[, $q]) | FLOAT; if ($p > $q) { $c++; } } echo "L{l} ", $p, ' ', $q, ' ', $u, ' ', $c, "\n"; } catch (Throwable $e) { echo "L{l} ", get_class($e), ': ', $e->getMessage(), "\n"; }`, N = 2…31.
  - **Int expressions:** leaves are variables or constants from `INT_SMALL` (0, ±1, ±2, 3, ±7, 13, 63, 64, 65, −64, 100, 1000003); in one of eight constant leaves an edge value from `INT_EDGES` (PHP_INT_MAX, PHP_INT_MIN as `(-9223372036854775807 - 1)`, ±2^62, ±3037000500 ≈ √PHP_INT_MAX). Nodes: `+ - * & | ^`, `%`, `<<`, `>>`, `intdiv`, `max`/`min`, `abs`, unary `-`, ternary. Divisors are mostly `(x | 1)` (odd, never 0), in one of ten cases unchanged, so that division/modulo by 0 occur. Shift widths mostly `abs(x) % 70` (thus also ≥ 64), in one of ten cases `x % 70` (negative possible).
  - **Float expressions:** leaves from `FLOAT_SMALL` (0.5, −1.5, 3.25, 1.0e15, 1.0e-7, 0.1, 100.0, −7.75), rarely `FLOAT_EDGES` (0.0, −0.0, ±2.5e300); nodes `+ - *`, `/` (denominator mostly `x + 0.25`, in 1/10 without protection), `(float) int`, `int / divisor`, `float * int`, unary `-`.
  - **Bool expressions:** int comparisons (`< <= > >= === !==`), float comparisons (`< > <=`), `(a <=> b) === -1|0|1`, negation.
- **Final checks:** more than 3/4 of the programs (> 300 of 400) must contain compiled code in the tier-1 run (`vm.jit_tiers().len() > 0`), and more than 1/20 (> 20) must raise an exception (output contains "Error"). On stderr: exceptions by kind, number of compiled functions/loops.
- **Example** (case 0 of the default seed, reconstructed from the generator in Python, not verified via cargo):
  ```php
  <?php
  function f0(int $a, int $b, float $x): int { if ((64 >= $a)) { return (intdiv(min($a, 2), ((- 63) | 1)) << (abs($b) % 70)); } return ($b ^ max(intdiv($b, ($b | 1)), (- 2))); }
  function g0(float $x, float $y, int $a = -7): float { return 1.0e-7; }
  function f1(int $a, int $b, float $x): int { if (((min($b, -7) <=> ($a << (abs(0) % 70))) === -1)) { return (((63 === -3037000500) ? ($a - 2) : 1) >> (abs(13) % 70)); } return abs((($b >> (abs($a) % 70)) << (abs((($a <= $a) ? $a : $a)) % 70))); }
  function g1(float $x, float $y, int $a = 2): float { return 2.5e300; }
  function f2(int $a, int $b, float $x): int { $s = 0; for ($k = 0; $k < 5; $k++) { $s = ($s + (- ($a % ($a | 1)))) % 1000003; } return $s; }
  function g2(float $x, float $y, int $a = 65): float { return (1.0e15 / (($y * -1.5) + 0.25)); }
  try { $p = -1; $q = -2; $u = 100.0; $c = 0;
    for ($i = 0; $i < 4; $i++) { $p = 63; $q = f1($p, $i, $u); $u = g1($u, 0.5); if ($p > $q) { $c++; } }
    echo "L0 ", $p, ' ', $q, ' ', $u, ' ', $c, "\n";
  } catch (Throwable $e) { echo "L0 ", get_class($e), ': ', $e->getMessage(), "\n"; }
  ```
  Expected output (determined with `php`; the program touches no dialect deviation): `L0 63 0 2.5E+300 4`. In the test itself, however, the expectation comes from the interpreter, not from PHP.
- **Oracle:** `independent` in the sense of "interpreter against JIT" (two implementations of the same semantics) and **since TEST2.4 additionally PHP 8.4** (`strict_types=1`, 8 threads) for the programs without dialect deviation: excluded are programs with `ArithmeticError` (overflow, D10), `GRD021` and an int/int division (`/` yields float, D10; the generator records this per program), furthermore programs for which PHP itself reports "Deprecated: Implicit conversion from float … to int" (lossy float→int conversion in `%`/bit ops, open dialect question, KNOWN_ISSUES). As of 2026-10-26: 159 of 400 programs compared with PHP, 2 omitted because of the conversion, 0 deviations. At least 1/3 of the cases must be compared. Without `php`: missing oracle (`OPENGRID_REQUIRE_ORACLES`).
- **Notes:** deterministic (fixed seed). Longer runs only by hand via `GRD_FUZZ_CASES`.

#### `grd-jit:tests/suite/fuzz.rs::interpreter_and_jit_agree_on_array_programs`
- **Run:** `cargo test -p grd-jit --test suite -- --exact fuzz::interpreter_and_jit_agree_on_array_programs`; runtime 4.03 s; in the suite: yes.
- **Checks:** differential test for JIT2b (array reads in compiled loops and functions): interpreter, tier-1 JIT and advanced JIT must produce the same output.
- **Kind:** generated / differential test, same flow and same `run` function as above, likewise with the two forced-exit runs (`forced_exits_agree`).
- **Generator:**
  - **Seed and case count:** default `GRD_FUZZ_SEED = 0xA77A_2026_1025`, `GRD_FUZZ_CASES = 300` (the same environment variables as the other differential test; whoever sets them changes both).
  - **Data:** `$A` = list of 1–12 elements, `$M` = 1–5 rows of 1–6 elements each, `$F` = 1–8 floats from `FLOAT_SMALL`. Lists are int lists from `INT_SMALL`; with probability 1/3 one element is "foreign": `0.5`, `'x'`, `null`, `[1]`, `true` or `PHP_INT_MAX` (type change and overflow in compiled code).
  - **Functions** (fixed except for two places): `s(array $a, int $n): int` sums `E` (int expression depth 2 over `$a[$k]`, `$k`, `$s`) modulo 1000003 up to a bound from `count($a)`, `count($a)`, `$n`, `$n + 1` (may read past the end); `fs(array $a): float` sums `$a[$k] * 0.5`; `cell(array $m, int $r, int $c): int` returns `$m[$r][$c]`.
  - **1–3 loop blocks** with 2–26 iterations, body one of five forms: `s($A, k)` with k up to `n+1`, `fs($F)`, `$M[$i % rows][$i % c]`, `cell($M, $i % rows, 0..2)`, `$A[$i % count($A)] * 3 + count($M)`; per block `try … echo "L{l} ", $t, ' ', $u … catch (Throwable $e) …`.
- **Final checks:** more than half of the programs (> 150) with compiled code, more than 1/20 (> 15) with an exception.
- **Example** (case 2 of the default seed, reconstructed in Python):
  ```php
  <?php
  $A = [63, 100, 100, 3, 13, 1000003, -2, -1, 1000003];
  $M = [[-1]];
  $F = [1.0e15, -1.5, -1.5, 0.5, 0.5, 1.0e15, 1.0e15, 0.1];
  function s(array $a, int $n): int { $s = 0; for ($k = 0; $k < count($a); $k++) { $s = ($s + $a[$k]) % 1000003; } return $s; }
  function fs(array $a): float { … }   function cell(array $m, int $r, int $c): int { return $m[$r][$c]; }
  try { $t = 0; $u = 0.0;
    for ($i = 0; $i < 23; $i++) { $t = ($t + cell($M, $i % 1, 0)) % 1000003; }
    echo "L0 ", $t, ' ', $u, "\n";
  } catch (Throwable $e) { echo "L0 ", get_class($e), ': ', $e->getMessage(), "\n"; }
  ```
  Expected output (with `php`): `L0 -23 0`. Case 0 of the same seed contains `'x'` in `$A` and ends in PHP with `L0 TypeError: Unsupported operand types: string * int`; which message grd prints there is irrelevant for the test as long as interpreter and JIT agree.
- **Oracle:** `independent` (interpreter against JIT), without PHP comparison: here PHP deliberately deviates almost everywhere (warning and `null`/continuing where the dialect throws on a missing key or a non-number, DIALECT §5.2/§5.3).
- **Notes:** as above.

#### `grd-jit:tests/suite/stress.rs::workers_rebuild_and_swap_tiers_under_load`
- **Run:** `cargo test -p grd-jit --test suite -- --exact stress::workers_rebuild_and_swap_tiers_under_load`; runtime 0.06 s; in the suite: yes.
- **Checks:** JIT5 (lifetime of the machine code): 4 threads each build a fresh VM in 6 rounds (like a worker restart) and run one of 6 program variants (`f{k}` with int loop and default parameter, `g{k}` with float arithmetic, 300 calls), JIT advanced and asynchronous (`with_tiers(true, false)`), threshold 20. Per round the output must equal the interpreter's and something must have been compiled. At the end: admitted tier-2 compilations ≤ budget and cache entries ≤ admissions.
- **Kind:** integration / stress test; expectations computed in the interpreter beforehand → 4 threads × 6 rounds → output comparison per round, `tier2_stats()` at the end.
- **Oracle:** `independent` for the outputs (interpreter against JIT), `own expectation` for the budget statements.
- **Notes:** concurrent; whether tier 2 arrives during a run is time-dependent. Since TEST2.4 additionally: at the end at least one tier-2 admission and one cache entry (both previously unchecked). `tier2_stats()` is process-wide; the exact budget statements are therefore made by `tier2_budget_is_used_up_exactly` in a separate process.

#### `grd-jit:src/lib.rs::runtime_reads_writes_and_allocates_through_the_vm_pointer`
- **Run:** `cargo test -p grd-jit --lib -- runtime_reads_writes`; runtime 0.00 s; in the suite: yes. **Under Miri:** `cargo +nightly miri test -p grd-jit --lib runtime` (≈ 45 s with build, test 11 s; nightly with component `miri`, in the container since 2026-10-26).
- **Checks:** the object runtime functions of JITO1 (`grd_rt_prop_get`, `grd_rt_prop_set`, `grd_rt_new`) directly via a `*mut Vm`, without compiled code: read float, int and object property; invalid handle and slot past the end → `KIND_NONE`; write and read back; writing to an invalid handle/slot does nothing; `new` with two argument triples returns the next handle with the values; unknown class and reached object limit → 0 and no allocation.
- **Kind:** unit; VM without prelude, small program with two final classes via `run_file`, then calls of the `extern "C"` functions.
- **Oracle:** `own expectation` (contract of the runtime functions, JITD2 §8f "check first, then act"); Miri checks the `unsafe` blocks (aliasing of the VM references, stack slot accesses).
- **Notes:** JITO1 (D20). Miri green; the only warnings come from the array functions (below).

#### `grd-jit:src/lib.rs::runtime_reads_arrays`
- **Run:** `cargo test -p grd-jit --lib -- runtime_reads_arrays`; runtime 0.00 s; in the suite: yes; under Miri as above.
- **Checks:** `grd_rt_dim` (element present/missing) and `grd_rt_count` on a real `PhpArray`.
- **Kind:** unit.
- **Oracle:** `own expectation`.
- **Notes:** Miri reports two warnings here, "integer-to-pointer cast" (`grd_rt_dim`, `grd_rt_count`): since JIT2b arrays are stored as an address in a `u64` in the buffer; Miri allows that under its default rule for pointer provenance (permissive provenance), it is not an error. The object functions receive the VM as a real pointer and produce no warning.

#### `grd-jit:tests/suite/fuzz.rs::interpreter_and_jit_agree_on_object_programs`
- **Run:** `cargo test -p grd-jit --test suite -- --exact fuzz::interpreter_and_jit_agree_on_object_programs`; runtime ≈ 4 s; in the suite: yes.
- **Checks:** differential test for objects (JITO1, D20 prerequisite before the build): interpreter against tier-1 JIT, advanced JIT, advanced JIT with GC threshold 1 (MEM1b), tier 1 with `ForceExit::All` and with `ForceExit::Seed(seed ^ case)`; all six outputs character-for-character equal.
- **Kind:** generated / differential test (`object_program`, same random generator and same `run_with` function as the other differential tests).
- **Generator:** seed `0x0B1E_2026_1026`, 300 cases (`GRD_FUZZ_SEED`/`GRD_FUZZ_CASES`/`GRD_FUZZ_DUMP` as above). Classes: `P` (promoted constructor `float $x, float $y, int $n`, `float $z` without default, `bool $flag = false`; methods `sum(): float`, `ni(): int` with random expressions over `$this->…` incl. overflow, `%`, `intdiv`, shifts; `make(P $o): P` builds a new `P` from expressions over `$this` and `$o`; `near(P $o): bool`; `withZ()` reads `$z`, which is set only in some iterations; `flagged()`), `Q extends P` (`int $extra = 3`, overrides `sum` in every second program, inherits the rest, `more()` can overflow), `final F` (`bump(int $k): F` and `wide(): F` build new objects, `wide` with int→float widening), `final RO` with `readonly int $v` and `next(): RO` (overflow at a large start value), `final T`, whose constructor throws `RangeException` above a limit, via `mk(int $v): T`. Main program: loop of 3–32 iterations over `P`/`Q`/`P`, every exception caught and printed; at the end `var_dump` of two objects (object ids) and `spl_object_id`.
- **Oracle:** `differential` (interpreter); not PHP, because grd does not reuse object ids within a request (D13) and `/` always yields float.
- **Notes:** before the build of JITO1 step 1 the JIT compiled nothing here (0 units). **Since step 1** the test requires ≥ 5 compiled units per program on average (state: 2558 in 300 programs); compiled are, among others, `P::sum/ni/make/near/withZ/flagged/setFlag/shift`, `F::bump/wide/val/seen`, `RO::next`; `bumpN` (read after write, V3) and `t` (constructor with body) remain interpreter. Extended by setters with `return $this`, writing followed by reading, private property in a `final` class, `new` with a throwing constructor in a method. Found during the build: forced exits after a call with an effect (`new`, call of a compiled function) doubled the effect; the forced exit now lies before the call, the real guard after it (test mode only affected). Found and fixed while writing: message and `var_dump` of uninitialized typed properties (ccac03e). Every program throws at least one exception (`withZ` before setting); > 1/5 is required.

#### `grd-jit:tests/suite.rs::probes_match_php_with_every_exit_forced`
- **Run:** `cargo test -p grd-jit --test suite -- --exact probes_match_php_with_every_exit_forced`; runtime ≈ 0.5 s; in the suite: yes.
- **Checks:** all probes with tier 1, threshold 1 and `ForceExit::All` (JITD2 §8c, environment `GRD_JIT_FORCE_EXIT=all`): compiled code bails out at **every** exit point (guards for overflow, division by zero, element kind, failed native call, return `Num`→int), loops hand slots and stack over to the interpreter, whole functions report "bailout" and rerun in the interpreter. Output equal to `.out`, minimum amount of compiled code as in the other runs.
- **Kind:** golden (data-driven), like `probes_match_php_with_tier1_only` with `CraneliftJit::with_options(false, false, ForceExit::All)`.
- **Oracle:** as `probes_match_php_with_tier1_only` (PHP 8.4 output, `probes_deviations.txt`).
- **Notes:** JITO1 step 0 (D20). Checked with `GRD_JIT_LOG=1`: the exits are actually taken (e.g. `jit_int_float_kernels`: exits at inner ips instead of only at the loop end, status 2 for whole functions).

#### `grd-jit:tests/suite.rs::probes_match_php_with_jit_and_constant_collection`
- **Run:** `cargo test -p grd-jit --test suite -- --exact probes_match_php_with_jit_and_constant_collection`; runtime ≈ 0.6 s; in the suite: yes.
- **Checks:** all probes with the advanced JIT (tier 2 synchronous, threshold 1) **and** GC threshold 1 (collect at every doubling, MEM1b): compiled code must keep its values reachable for the GC. Output equal to `.out`, minimum amount of compiled code as in the other JIT probe runs.
- **Kind:** golden (data-driven), like `probes_match_php_with_tier2` plus `set_gc_threshold(1)`.
- **Oracle:** as `probes_match_php_with_tier1_only` (PHP 8.4 output, `probes_deviations.txt`).
- **Notes:** TEST2.4.

#### `grd-jit:tests/suite/stress.rs::tier2_budget_is_used_up_exactly`
- **Run:** `cargo test -p grd-jit --test suite -- --exact stress::tier2_budget_is_used_up_exactly`; runtime ≈ 0.2 s; in the suite: yes.
- **Checks:** starts the test binary as a child process (`stress::tier2_budget_child`, with `GRD_JIT_TIER2_LIMIT=4`), so that budget and cache belong to this test alone, and requires success and the child's success message.
- **Kind:** integration via a child process.
- **Oracle:** `own expectation` (JIT5 specification: budget, continuing on tier 1, shared cache).
- **Notes:** TEST2.4. Found a bug during implementation: the process-wide tier-2 cache never hit, because the selector IDs of the prelude modules were assigned in hash order (different per VM); fixed in `grd-vm` (`build_sel_tables` now assigns them sorted).

#### `grd-jit:tests/suite/stress.rs::tier2_budget_child`
- **Run:** only as a child process of `tier2_budget_is_used_up_exactly` (`#[ignore]`; immediately green without `GRD_TIER2_BUDGET_CHILD`).
- **Checks:** with budget 4: two rounds over the 6 variants of the stress test, fresh VM each, tier 2 synchronous, threshold 20; every output equal to the interpreter's. Afterwards: budget read from `GRD_JIT_TIER2_LIMIT` (= 4), exactly 4 admissions, cache ≤ admissions, at least one unit stayed on tier 1 (budget exhausted), and the tier-2 units of the second round came from the cache (tier-2 units ≥ 2 × cache entries).
- **Kind:** integration; outputs against the interpreter, `tier2_stats()` and `jit_tiers()` against literals.
- **Oracle:** `independent` for the outputs (interpreter), `own expectation` for budget and cache.
- **Notes:** TEST2.4.

#### opengrid-pkg

#### `opengrid-pkg:src/lib.rs::add_lock_verify_remove`
- **Run:** `cargo test -p opengrid-pkg --lib -- add_lock_verify_remove`; runtime < 0.1 s; in the suite: yes.
- **Checks:** the module manager (PKG1) end to end: `add` fetches a module from a local git repository (tag `v1.0.0`) into `modules/blog` (without `.git`), writes `modules.lock` (name, version, 40-character commit, checksum); `verify` is empty afterwards; a second `add` is rejected with "already installed"; after changing a module file `verify` reports "changed locally"; `remove("auth")` is rejected because `blog` depends on it; `remove("blog")` removes directory and lock entry; `add` with the commit id pins the same commit.
- **Kind:** unit with a real file system and real `git` (temp directory, repository via `git init -b main`, commit, tag, URL `file://…`).
- **Oracle:** `own expectation` (specification in the module comment, PKG1/MOD1); error messages via `contains` on own texts.
- **Notes:** needs the `git` binary (with `init -b`, i.e. ≥ 2.28); no network.

#### `opengrid-pkg:src/lib.rs::refuses_repositories_without_manifest_or_dependencies`
- **Run:** `cargo test -p opengrid-pkg --lib -- refuses_repositories_without_manifest_or_dependencies`; runtime < 0.1 s; in the suite: yes.
- **Checks:** `add` rejects a repository without `module.toml` ("no module.toml") and one whose manifest names a dependency that is not installed ("needs `billing`"), and in the second case creates no `modules/shop`.
- **Kind:** unit with `git` as above.
- **Oracle:** `own expectation` (specification in the code).
- **Notes:** needs `git`.


### 2.3 opengrid-framework: integration tests (api_script … mail_storage)

All tests live in the single integration test binary `crates/framework/tests/suite.rs` (D7, one binary, because every file links the whole SurrealDB engine). Common pattern: a `Pool` of worker VMs is started, either on the fixture `crates/framework/tests/fixtures/app` (module `tasks`: `TaskController`, `SessionController`, model `Task` with soft deletes, `routes/web.grd`, Loco config `config/{development,test}.yaml`) or on a fresh project from `scaffold::new_project` in `$TMPDIR/opengrid-<tag>-<pid>`. Into this project the test writes dialect files that it brings along as string constants. Requests go via `dispatch::handle` directly to a worker, i.e. without a socket (exceptions: `axum_catch_all` uses the Axum router, `test_app_api_script` real HTTP). Scripts run with `vm.run_file`, the output is compared.

**Database in this part:** no test needs a `surreal` server or `OPENGRID_TEST_DB_URL`, and none skips itself. The tests with a database use embedded `mem://` (`DbConfig::mem("test", "<name>")`, a separate database per test) or `surrealkv://` in a temp directory (`db_embedded`, `api.sh`). `PoolOptions::new` has **no** database by default (`database: None`). `errors`, `http`, `inject` and `mail_storage` therefore run without a DB. A schema is actively pushed only in `auth` (`ops::push`) and in `api.sh` (`opengrid task migrate_push`). Since TEST2.6 the `database` tests apply the fixture schema (`ops::push`, among others a UNIQUE index on `tasks.title`); before, they ran on SCHEMALESS tables created on the first insert.

**Relation to Laravel:** no test in this part runs Laravel itself. Laravel message texts ("The :attribute has already been taken.", "Unauthenticated.", "Too many login attempts." …) and Laravel behavior (query builder, pagination fields) were written by the author as literals following the Laravel documentation or Laravel language files. The only exception is `api.sh`. The same script runs with `--app laravel` against the Laravel replica LV1 (Sanctum, SQLite precursor). According to DECISIONS (2026-10-25, part b LV1) it was green there. But that is a manual run outside the suite: in the suite only the OpenGrid side runs.

---

#### `opengrid-pkg:src/lib.rs::verify_reports_missing_modules_and_names_are_guarded`
- **Run:** `cargo test -p opengrid-pkg --lib -- verify_reports_missing_modules_and_names_are_guarded`; runtime ≈ 0.2 s; in the suite: yes.
- **Checks:** a module directory deleted by hand is reported by `verify` as `missing`; module names such as `../evil`, `Blog`, `a/b`, empty, with spaces or a dot are rejected by `add` (nothing is written outside `modules/`); `remove` follows no paths (`../outside`, `blog/../..`).
- **Kind:** unit with local git repositories (`file://`).
- **Oracle:** `own expectation`.
- **Notes:** TEST2.7.
#### `opengrid-framework:tests/suite/api_script.rs::test_app_api_script`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact api_script::test_app_api_script`; runtime 2.60 s; in the suite: yes.
- **Checks:** the acceptance test TP5 of the base application `examples/test-app` (modules `auth`, `notes`) end to end over real HTTP: `/health` (status, DB mode `embedded`), register (201, password hidden; 422 for a duplicate e-mail and for invalid data with exact Laravel messages), login (same 401 for unknown e-mail and wrong password, token with 64 characters), `me`, notes CRUD with filter, sorting (also rejection of `sort=priority;DELETE notes` → 400), pagination and tenant separation (second user gets 404 on foreign notes), logout revokes the token. Then restart on the same surrealkv file (data and token survive) and a task over the control socket (`notes: 2`). 43 steps in total.
- **Kind:** script/integration. The Rust test starts `bash examples/test-app/tests/api.sh` with `OPENGRID_BIN` = binary of this build. The script copies the app to `mktemp -d`, sets `OPENGRID_DB_URL=surrealkv://$WORK/db`, calls `opengrid task migrate_push`, starts `opengrid start` on a free port and checks every step with `curl` plus a `jq -e` expression. The Rust test requires exit code 0, "steps passed" in the output and no "FAIL ".
- **Oracle:** mixed. Status codes, JSON shapes and message texts are `own expectation` (literals in the script, based on Laravel/Sanctum behavior and the Laravel language file). They were confirmed independently only outside the suite: with `--app laravel` the same script runs against the Laravel replica (DECISIONS 2026-10-25, "the Laravel replica passed `api.sh` after one correction"). In the suite itself no Laravel is involved.
- **Notes:** needs `bash`, `curl`, `jq` and `python3` (free port). If they are missing, the script ends with exit 2 and the test turns **red**, it does not skip itself. Starts real server processes; the wait for `/health` is bounded (100 × 0.2 s). KNOWN_ISSUES mentions a one-time, unreproduced red run (2026-10-25, "controller class `Auth\Controllers\AuthController` not found", together with `test_app_crud`). Since TEST2.6 the Laravel variant runs as its own test `test_app_api_script_against_laravel`.

#### `opengrid-framework:tests/suite/api_script.rs::test_app_api_script_against_laravel`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact api_script::test_app_api_script_against_laravel`; runtime ≈ 10 s; in the suite: yes (with Laravel environment).
- **Checks:** `api.sh --app laravel --runtime octane` against the Laravel replica of the test app (LV1, SQLite): the same 39 steps (status codes, messages such as "Unauthenticated.", "The email has already been taken.", JSON shapes), 6 OpenGrid-only steps skipped.
- **Kind:** end to end against an external oracle.
- **Oracle:** `independent` (Laravel 13.38 itself answers the requests that the script sends to OpenGrid).
- **Notes:** TEST2.6. Needs `bench/laravel-app` with `rr` (RoadRunner) from `setup.sh`; otherwise missing oracle `laravel-vendor`. Found during implementation (TEST2.8): Octane 2.x considers RoadRunner 3.x older than 2023.3.0 and re-downloaded the binary on every start; an aborted start left only `rr.backup` → `setup.sh` places a wrapper in front of the binary, `serve.sh` restores `rr.backup`; start of the Laravel run afterwards ≈ 3 s instead of ≈ 10 s.

#### `opengrid-framework:tests/suite/auth.rs::token_use_is_noted_at_most_every_interval`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact auth::token_use_is_noted_at_most_every_interval`; runtime ≈ 0.1 s; in the suite: yes.
- **Checks:** DBP1 M2: a new token has `last_used_at` = null; the first authenticated request sets it, a second within 60 s does not change it. With `auth.token_touch_seconds = 0` every request writes a new value (like Sanctum).
- **Kind:** integration; two pools (default and 0) with `mem://`, register/login/`/api/me`, `last_used_at` read via `DB::select`.
- **Oracle:** `own expectation` (DBP1, LARAVEL_BREAKING_CHANGES §6; Sanctum behavior at 0 according to the Laravel source, not executed).
- **Notes:** found a bug in the new code on the first run (a missing field was read strictly).

#### `opengrid-framework:tests/suite/auth.rs::token_authentication`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact auth::token_authentication`; runtime 0.94 s; in the suite: yes.
- **Checks:** AU1/AU2 in Sanctum style. Register stores a bcrypt password that is not printed. `unique:users,email` returns 422 with "The email has already been taken.". Unknown e-mail and wrong password return 401 with a byte-identical body. The token has 64 hex characters, and `personal_access_tokens` contains only `token_hash`, not the plaintext. `auth` middleware: 401 without token and with a wrong token ("Unauthenticated."), 200 with a valid token. Logout returns 204 and revokes the token. Login throttle: after 5 failed attempts 429, also with the correct password and a differently cased e-mail (`ADA@…`), with `Retry-After` 1..60. Another e-mail has its own counter. `throttle:3,1` on `/api/ping`: called four times, the fourth returns 429 with `X-RateLimit-Limit: 3`, `X-RateLimit-Remaining: 0` and `Retry-After`.
- **Kind:** integration. Creates a fresh project with model, controller, routes and `.surql` schema as constants, starts a pool (2 workers, `mem://`) and pushes the schema (`ops::push`). The requests go via `dispatch::handle` to the workers; status, headers and JSON fields are checked.
- **Oracle:** `own expectation`. All expectations are literals in the test. The message texts correspond to Laravel's language files (validation.unique, "Unauthenticated.", beginning of auth.throttle). The deviation 429 instead of Breeze's 422 is documented in LARAVEL_BREAKING_CHANGES §AU2. That only the SHA-256 hash is stored in the token is checked only indirectly: the plaintext must not occur, and a column `token_hash` must exist. Whether the stored value is actually SHA-256 of the token is not checked.

#### `opengrid-framework:tests/suite/auth.rs::several_tokens_per_user_and_logout_revokes_one`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact auth::several_tokens_per_user_and_logout_revokes_one`; runtime ≈ 0.4 s; in the suite: yes.
- **Checks:** two logins of the same user return two different tokens, both valid; logout with the first revokes only that one (401), the second stays valid; exactly one token row remains.
- **Kind:** integration via HTTP handlers.
- **Oracle:** `own expectation` (Sanctum behavior).
- **Notes:** TEST2.7.

#### `opengrid-framework:tests/suite/auth.rs::noting_the_token_use_never_fails_a_request`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact auth::noting_the_token_use_never_fails_a_request`; runtime ≈ 0.3 s; in the suite: yes.
- **Checks:** TOK2: with `auth.token_touch_seconds = 0` 8 threads send 25 requests each with the same token (every one writes `last_used_at`), all 200. Then the database refuses every write of `last_used_at` (`ASSERT $value = NONE`); three further requests are nevertheless answered normally with 200 and the user.
- **Kind:** integration via HTTP handlers, concurrent and with a forced write error.
- **Oracle:** `own expectation` (D19: noting the use must never fail a request).
- **Notes:** counter-check during implementation: without the protection in `Auth::noteUse` the request answers with 500 ("field must conform to: $value = NONE").

#### `opengrid-framework:tests/suite/auth.rs::tokens_expire`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact auth::tokens_expire`; runtime ≈ 0.4 s; in the suite: yes.
- **Checks:** token expiry like Sanctum: with `auth.token_expiration = 60` a 59-minute-old token is valid, a 61-minute-old one is not (`created_at` set back via SQL); a token with `expires_at` in the past is invalid, one in the future is valid (`Auth::createToken($user, 'x', $expiresAt)`).
- **Kind:** integration via HTTP handlers.
- **Oracle:** `own expectation` (Sanctum's `isValidAccessToken`: `created_at` younger than `expiration` minutes and `expires_at` not passed).
- **Notes:** TEST2.7. Token expiry did not exist before; added with TEST2.7 (field `expires_at`, setting `auth.token_expiration`, default 0 = never).

#### `opengrid-framework:tests/suite/control.rs::tasks_run_inside_the_server`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact control::tasks_run_inside_the_server`; runtime 0.05 s; in the suite: yes.
- **Checks:** J1, the control socket. Without a server `control::forward` returns `None`, and the caller runs the task itself (`notes:prune` → `Notes\Tasks\Prune`, argument `days`). With a server `.opengrid/control.sock` has permissions 0600; the task output comes back; an exit code ≠ 0 is reported as an error with "ERROR: …" and "task exited with code 3"; an unknown class reports "class … not found". `migrate_push` answers "schema in sync", `status` returns its own PID. A second server for the same project fails with `AddrInUse`. A leftover socket file without a server counts as "no server".
- **Kind:** integration. Fresh project with two dialect tasks, pool with `mem://`, the server runs on a Tokio runtime in a thread. The test polls for connectability (max. 10 s) and compares return values exactly or via `contains`.
- **Oracle:** `own expectation` (specification J1/D5, literals in the test).
- **Notes:** polling with timeout. If the server is not running after 10 s, the test fails only at `metadata(&sock)` with an unclear message.

#### `opengrid-framework:tests/suite/database.rs::model_crud_soft_deletes_and_pagination`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact database::model_crud_soft_deletes_and_pagination`; runtime 0.02 s; in the suite: yes.
- **Checks:** the dialect model `Task` on SurrealDB. `create` returns a model with `RecordId` (table `tasks`); the field `secret` is not set (not in `$fillable`). Further checked: `all`/`where`/`count`, `sort('title:desc')`, `find` (same ID, `created_at` set, unknown ID → null), `isDirty` before and after `save`. Soft delete: `delete`, then `withTrashed`/`onlyTrashed`. `paginate(1, 2)` returns the fields total/per_page/current_page/last_page/data. Also `select(...)` + `orderBy`, mass `update` (returns the count), `whereLike` (case-insensitive), `whereIn` + `withTrashed`, `filter([...])`, rejection of a filter field `password` (ValueError) and `forceDelete`.
- **Kind:** integration/golden output. Pool on the fixture with `mem://`; a script with `echo` lines runs, the whole output is compared with a string literal.
- **Oracle:** `own expectation`. The file header says explicitly: "Expectations are my own until `builder_cases.php` arrives" (DECISIONS 2026-10-24 point 5: phase 6 "checked against own expectation"; ROADMAP B0 waits for Jonas). The order of the JSON keys (`priority` before `title`) comes from SurrealDB, so from its behavior as well.
- **Notes:** **TEST2.6:** runs with the fixture schema applied.

#### `opengrid-framework:tests/suite/database.rs::raw_queries_and_transactions`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact database::raw_queries_and_transactions`; runtime 0.01 s; in the suite: yes.
- **Checks:** `DB::select` raw (with `GROUP ALL` count and `$t` binding), `DB::transaction` (commit returns the return value; on an exception rollback, the count stays 2, the exception reaches the caller), `DB::table(...)->orderBy/where/first` and `DB::query` with a syntax error (throws `RuntimeException`).
- **Kind:** integration/golden output on the fixture with `mem://`, exact comparison of the whole output.
- **Oracle:** `own expectation` (literal). Whether the rollback takes effect is decided by SurrealDB itself, observed via `Task::count()`.
- **Notes:** **TEST2.6:** runs with the fixture schema applied.

#### `opengrid-framework:tests/suite/database.rs::database_over_http`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact database::database_over_http`; runtime 0.02 s; in the suite: yes.
- **Checks:** a controller (`GET /db`) writes to the database within the request. The response contains `count` (1, on the second call 2), `first` = "from http" and an `id` with prefix `tasks:`. With 2 workers this shows that both workers see the same DB if the second request lands on the other worker. This is not enforced.
- **Kind:** integration via `dispatch::handle`, JSON fields are checked.
- **Oracle:** `own expectation` (literals).
- **Notes:** **TEST2.6:** all `database.rs` tests now run with the fixture schema applied (`ops::push`): field types, defaults, indexes. This revealed that the second `GET /db` fails at the UNIQUE index `tasks_title_unique` (same title); the test now checks exactly this rejection by the database instead of "count 2".

#### `opengrid-framework:tests/suite/database.rs::validation_exists_and_unique_against_the_database`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact database::validation_exists_and_unique_against_the_database`; runtime 0.01 s; in the suite: yes.
- **Checks:** the validation rules `unique:tasks,title` and `unique:tasks` (column = field name), `unique` with an excluded ID (update case), `exists:tasks,title` (hit/no hit), `nullable|exists` with a missing field, and the rejection of a table name with `;` (ValueError). The messages are "The title has already been taken." and "The selected task is invalid.".
- **Kind:** integration/golden output (`Validator::errors` as JSON), fixture with `mem://`.
- **Oracle:** `own expectation`. The literals follow Laravel's rule syntax and language file (validation.unique/exists). Laravel itself is not executed; the 132-case specification from `validation_spec` does not cover these DB rules, as far as is visible here.
- **Notes:** **TEST2.6:** runs with the fixture schema applied.

#### `opengrid-framework:tests/suite/database.rs::query_builder_like_laravel`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact database::query_builder_like_laravel`; runtime 0.03 s; in the suite: yes.
- **Checks:** Q1, the query builder with Laravel API. Checked are:
  - flat `orWhere` chain (AND binds more tightly), the argument order `where(f, v, op)` of the private reference implementation, nested group via closure
  - `whereNot`, `whereBetween`/`NotBetween`, `whereNotIn`, `whereColumn`, `like`/`not like` case-insensitive
  - `orderByDesc`/`value`, `oldest`/`latest`, `pluck(v, k)`, the aggregates sum/avg/min/max (also on an empty set: avg → null, sum → 0)
  - `increment`/`decrement` (also with extra columns), `chunk` (page count, abort with `return false`), `firstOrCreate`, `updateOrCreate`
  - soft-delete clause before the OR group (`toSql()` as exact text), four injection attempts in field name, operator and sort direction (all → ValueError)
- **Kind:** integration/golden output, compared line by line with 13 literal lines. Five records on the fixture with `mem://`.
- **Oracle:** `own expectation`. Laravel behavior was rewritten by hand following the documentation (comparison in LARAVEL_BREAKING_CHANGES §Q1: "equal"). The `toSql()` form follows DECISIONS 2026-10-24 point 4 and is explicitly own expectation until `builder_cases.php` (B0). Neither Laravel nor the PHP builder of the reference implementation is executed.
- **Notes:** **TEST2.6:** runs with the fixture schema applied.

#### `opengrid-framework:tests/suite/db_embedded.rs::embedded_default_and_relative_paths`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact db_embedded::embedded_default_and_relative_paths`; runtime 0.05 s; in the suite: yes.
- **Checks:** DB1. Without configuration the database is `surrealkv://<root>/.opengrid/db` with `DEFAULT_LOCK_WAIT`; a relative URL `surrealkv://var/data` is resolved against the project (not against the working directory); `lock_wait: 5` is adopted. A child process opens the default DB, and afterwards the directory `.opengrid/db` exists.
- **Kind:** integration. `app::pool_options` with JSON settings, field comparison. For the creation the test starts itself as a child process (`db1_hold_child`, hold 0 ms), so that this process keeps no lock (DECISIONS: a surrealkv file cannot be reopened in the same process).
- **Oracle:** `own expectation` (DECISIONS 2026-10-24 "DB1 implemented").
- **Notes:** depends on the environment: `pool_options` reads `OPENGRID_ROOT`, `OPENGRID_DB_URL` and `SURREAL_DB_HOST`. If one of them is set in the shell, the test turns red.

#### `opengrid-framework:tests/suite/db_embedded.rs::db1_hold_child`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact db_embedded::db1_hold_child --ignored` (only meaningful as a child process); `#[ignore]`, skipped in the suite.
- **Checks:** nothing by itself. It is a helper function as a test: it opens `OG_DB1_URL`, writes a marker file `<path>.held`, holds the lock for `OG_DB1_HOLD_MS` ms and exits.
- **Kind:** helper process. The other DB1 tests start it with `--exact db_embedded::db1_hold_child --ignored`.
- **Oracle:** none (there is no check except "connect succeeds").
- **Notes:** `#[ignore = "runs as child process of the DB1 tests"]`. Without the two environment variables it returns immediately and is green. A `cargo test -- --ignored` thus reports it green without anything having been checked.

#### `opengrid-framework:tests/suite/db_embedded.rs::start_waits_for_the_previous_process_to_release_the_database`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact db_embedded::start_waits_for_the_previous_process_to_release_the_database`; runtime 1.63 s; in the suite: yes.
- **Checks:** the handover during deployment. A child process holds the surrealkv DB for 1.5 s; `Db::connect` with `lock_wait` 20 s waits and takes over after the release. Checked: duration ≥ 500 ms and child exited successfully.
- **Kind:** integration with two processes and a marker file for synchronization (polling ≤ 30 s).
- **Oracle:** `own expectation` (DECISIONS DB1). The locking behavior itself comes from surrealkv.
- **Notes:** time-dependent. The limit of 500 ms is generous for a hold duration of 1.5 s.

#### `opengrid-framework:tests/suite/db_embedded.rs::start_gives_up_with_a_clear_message`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact db_embedded::start_gives_up_with_a_clear_message`; runtime 4.06 s; in the suite: yes.
- **Checks:** if the lock is not released (the child holds 4 s, `lock_wait` 1 s), `Db::connect` fails with "is in use by another process (is the server still running?)", and does so in less than 3 s.
- **Kind:** integration with a child process, `contains` on the message, time limit.
- **Oracle:** `own expectation` (message text from DECISIONS 2026-10-24 DB1, point 2).
- **Notes:** time-dependent (upper limit 3 s at lock_wait 1 s); conceivably flaky under heavy load.

#### `opengrid-framework:tests/suite/errors.rs::debug_error_page_and_json`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact errors::debug_error_page_and_json`; runtime 0.01 s; in the suite: yes.
- **Checks:** in debug mode `/boom` (throws `RuntimeException('kaputt')`) returns, for `Accept: text/html`, an HTML page with status 500: "500 | Internal Server Error", class name, message, file `TaskController.grd`, frame `boom()`, source line with `throw` and highlighted line (`class="hl"`). Without Accept, JSON comes with `error`, `message`, `file` and `trace`. Also `abort(404, …)` as HTML page "404 | Not Found" and an unknown route ("no route for GET /nope").
- **Kind:** integration on the fixture without DB, check with `contains`.
- **Oracle:** `own expectation`. The page titles are modeled on Laravel's style (literals), Laravel itself is not executed.

#### `opengrid-framework:tests/suite/errors.rs::production_hides_details`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact errors::production_hides_details`; runtime 0.01 s; in the suite: yes.
- **Checks:** with `debug(false)` the HTML page contains only "500 | Internal Server Error" and "Something went wrong", without file name and frame. The JSON has `error`, but neither `file` nor `trace`.
- **Kind:** integration, `contains` and negative checks.
- **Oracle:** `own expectation`.
- **Notes:** whether `message` ("kaputt") remains visible in production is not checked.

#### `opengrid-framework:tests/suite/errors.rs::object_limit_per_request`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact errors::object_limit_per_request`; runtime 0.01 s; in the suite: yes.
- **Checks:** MEM1a. With `limits.objects = 5000` a loop that creates objects aborts at the 5001st object with a catchable `Error` ("Allowed object count of 5000 per request exhausted"); exactly 5000 objects are then in the array. The next request on the same worker (1 worker) can again create 4000 objects, so the arena was emptied.
- **Kind:** integration, `starts_with` or exact comparison of the output.
- **Oracle:** `own expectation`. The message is modeled on PHP's pattern "Allowed memory size … exhausted", but written by the author.

#### `opengrid-framework:tests/suite/http.rs::routes_controllers_and_natives`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact http::routes_controllers_and_natives`; runtime 0.01 s; in the suite: yes.
- **Checks:** the request path with the routes of the fixture. Checked are:
  - array return → JSON with `Config::get('app.name')`
  - `request()->ip()` with and without `X-Forwarded-For` (last entry wins, IP1)
  - route parameters, query, `path()`, `route('id')`
  - POST as form and as JSON (`input`, `isJson`, own header, 201), 422 from the controller
  - string return → HTML with content type
  - `echo` in the controller becomes the body
  - redirect 302 with Location, 404 for an unknown route, 500 on an exception
- **Kind:** integration on the fixture without DB (2 workers), exact body comparisons.
- **Oracle:** `own expectation`. The expectations mirror what the fixture controller returns (literals).

#### `opengrid-framework:tests/suite/http.rs::requests_leave_no_objects_behind`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact http::requests_leave_no_objects_behind`; runtime 0.01 s; in the suite: yes.
- **Checks:** after 50 requests a worker has 0 live objects (`vm.objects.len()`); the log ring stays at most 200 entries. After `GET /tasks` the last log entry is `info` / `listing tasks {"count":2}`, if the queried worker handled this request.
- **Kind:** integration with a look into the worker state (`run_sync`).
- **Oracle:** `own expectation` (invariant from RUNTIME_ARCHITECTURE: object store per request).
- **Notes:** with 2 workers `run_sync` checks only one of them. The log check is under `if let Some`: if the request lands on the other worker, it is silently omitted. **TEST2.5:** now checks both workers: per round two jobs that wait for each other via a `Barrier` and therefore must run on different workers (VM addresses differ); afterwards object count 0 on both workers and the log line of the last request on one of the two (before, every check could land on the same worker).

#### `opengrid-framework:tests/suite/http.rs::axum_catch_all`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact http::axum_catch_all`; runtime 0.01 s; in the suite: yes.
- **Checks:** the Axum router (`app::router`) forwards arbitrary paths to the pool: `GET /tasks/3?verbose=0` returns 200 with exact JSON, `POST /tasks` with a JSON body returns 201.
- **Kind:** integration via `tower::ServiceExt::oneshot` (without socket).
- **Oracle:** `own expectation` (literals).

#### `opengrid-framework:tests/suite/http.rs::loco_config_of_the_fixture_loads`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact http::loco_config_of_the_fixture_loads`; runtime 0.00 s; in the suite: yes.
- **Checks:** Loco loads `fixtures/app/config/test.yaml` (`Config::from_folder`). `pool_options(settings, dev = true)` then returns debug and watch enabled, `workers = 2` from `settings.opengrid.workers` and `app.name = "Fixture"`.
- **Kind:** integration against loco-rs (configuration loader).
- **Oracle:** `own expectation`. Whether the YAML file can be loaded is decided by Loco itself.
- **Notes:** `debug`/`watch` follow from the passed `dev = true` as long as the YAML sets no values. The "development defaults" check thus confirms almost only the argument.

#### `opengrid-framework:tests/suite/inject.rs::constructor_injection`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact inject::constructor_injection`; runtime 0.01 s; in the suite: yes.
- **Checks:** I1, constructor injection. It is recursive (controller → `Greeter` → interface `Clock` → only implementation `FixedClock`); a default parameter (`$greeting = 'Hallo'`) is preserved; `Request` is injected; middleware also receives dependencies (header `X-Clock`). An interface with two implementations yields 500 with GRD022 and the name `FileStore` in the body.
- **Kind:** integration. Fresh project with ten dialect files as constants, 1 worker, without DB.
- **Oracle:** `own expectation` (specification I1/D1 "convention instead of registration"; Laravel container behavior only in spirit).

#### `opengrid-framework:tests/suite/jobs.rs::jobs_run_after_the_request_with_retries`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact jobs::jobs_run_after_the_request_with_retries`; runtime 0.13 s; in the suite: yes.
- **Checks:** J2. A request dispatches four jobs. They run after the request on a worker with DB access but without a request (`request()` throws → "no request"). `Flaky` runs three times (`$tries = 3`, succeeds on the third attempt), `Broken` fails for good. `Job::stats()` = `[0,3,1]` (pending, done, failed), followed by `job_log` with both entries and `attempts` = 3.
- **Kind:** integration. Fresh project, `mem://`, runner with 50 ms tick (`start_runner`). Polling on the statistics until pending reaches 0 (max. 20 s), then exact comparison.
- **Oracle:** `own expectation` (specification J2, literals).
- **Notes:** polling with time limit. If the limit is exceeded, the test fails with the intermediate state (no silent assumption).

#### `opengrid-framework:tests/suite/jobs.rs::durable_jobs_wait_for_the_runner_and_fail_into_failed_jobs`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact jobs::durable_jobs_wait_for_the_runner_and_fail_into_failed_jobs`; runtime 1.34 s; in the suite: yes.
- **Checks:** J4. Without a runner the jobs are only rows (`[4,0,0]`), `job_log` does not exist yet (SurrealDB reports "does not exist"). A `reserved_at` set by hand (crash simulation) is released by the runner at start, and everything runs once (`[0,3,1]`). `queue_failed` shows class and "LogicException: always". `dispatchLater(1, …)` does not run immediately, but later. `queue_retry all` requeues the job ("1 job(s) queued again"), it fails again (`[0,4,1]`). `queue_retry nope` reports an error, `queue_flush` deletes ("1 failed job(s) deleted", `[0,4,0]`).
- **Kind:** integration. Fresh project, `mem://`, tasks via `control::run_task`, `wait_for` with polling (max. 20 s).
- **Oracle:** `own expectation`. The table names and task names are modeled on Laravel's database queue (LARAVEL_BREAKING_CHANGES §8), the outputs are own literals. The message "does not exist" comes from SurrealDB.
- **Notes:** time-dependent. "not before its time" checks directly after `dispatchLater(1, …)` that nothing has run yet. If more than 1 s passes between the two calls (heavily loaded machine), the test turns red. Plus a fixed `sleep(100 ms)`.

#### `opengrid-framework:tests/suite/jobs.rs::backoff_delays_the_next_try`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact jobs::backoff_delays_the_next_try`; runtime ≈ 2.5 s; in the suite: yes.
- **Checks:** a job with `$tries = 2`, `$backoff = 2` that succeeds only on the second attempt: after the first attempt nothing happens for one second, then the second runs; according to database time 2–5 seconds lie between the two; at the end `[0,1,0]` (done, not failed).
- **Kind:** integration with runner (`start_runner`, 50 ms) on `mem://`.
- **Oracle:** `own expectation` (Laravel's meaning of `$backoff`).
- **Notes:** TEST2.7. Time-dependent (1 s pause, window 2–5 s).

#### `opengrid-framework:tests/suite/mail_storage.rs::mail_through_templates_and_outbox`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact mail_storage::mail_through_templates_and_outbox`; runtime 0.01 s; in the suite: yes.
- **Checks:** MS1 mail without SMTP. `Mail::to()->cc()->send(Mailable)` with Tera templates (`subject.t`, `html.t` with loop, `text.t`) from `modules/users/resources/mail/welcome`. HTML is escaped (`Ada &lt;3`), subject and text are not. The sender comes from `mailer.from` or from `->from()` in `build()`. `Mail::sent()` returns the outbox. A missing template reports "mail view `nope` not found", a missing address "mail without recipient" (ValueError).
- **Kind:** integration. Fresh project, 1 worker, without DB, line-by-line comparison.
- **Oracle:** `own expectation`. The escaping behavior comes from Tera (Loco mailer), `\/` from `json_encode`. Laravel/Blade is not compared; the deviation Tera instead of Blade is in LARAVEL_BREAKING_CHANGES §8.

#### `opengrid-framework:tests/suite/mail_storage.rs::smtp_failure_is_an_error_not_a_crash`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact mail_storage::smtp_failure_is_an_error_not_a_crash`; runtime 0.01 s; in the suite: yes.
- **Checks:** with SMTP on `127.0.0.1:1` (nobody listens there) `send` throws a catchable `Error` whose message starts with "mail could not be sent". The worker does not crash.
- **Kind:** integration, exact comparison of the output `error`.
- **Oracle:** `own expectation`.
- **Notes:** assumes port 1 on localhost is closed (network/environment dependency, practically always satisfied in the container).

#### `opengrid-framework:tests/suite/mail_storage.rs::storage_disks`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact mail_storage::storage_disks`; runtime 0.02 s; in the suite: yes.
- **Checks:** MS1 storage. On the local disk: `put`/`exists`/`missing`/`get` (missing file → null)/`size`, `copy`, `move`, `files` (not recursive) against `allFiles` (recursive, sorted), `delete` with an array (true) and on something already deleted (true like Laravel, since TEST2.2e). The `memory` disk is separate from local (binary data). Rejected are `../`, absolute paths, `docs/../../x` and backslash (ValueError). An unknown disk reports "storage disk `s3` is not configured". The local disk lives under `<project>/storage/app` (checked in the file system); no escape from the directory.
- **Kind:** integration. Fresh project, without DB, line-by-line comparison with literals, plus a check in the file system.
- **Oracle:** `own expectation` (literals; API names after Laravel's `Storage`, deviations in LARAVEL_BREAKING_CHANGES §8); `delete` on a missing file re-checked on 2026-10-26 against Laravel 13.38 (local disk, Flysystem 3): `true`.

---


### 2.4 opengrid-framework: integration tests (middleware … validation_spec) and groups

All tests live in a single integration test binary: `crates/framework/tests/suite.rs` includes every file under `tests/suite/*.rs` as a module via `#[path]` (reason in the header comment: every file links the whole SurrealDB engine, hence one binary instead of one per file; D7). Shared helper: `suite/server.rs` (`TestServer`, a throwaway `surreal start memory` on a free port with a random password, or `OPENGRID_TEST_DB_URL`; without either, `start()` returns `None` and the server tests return silently).

Fixture `crates/framework/tests/fixtures/app`: a modules-only app with one module `tasks` (controllers `TaskController`, `SessionController`, middleware `RequireToken` (header `x-token: secret`, otherwise `abort(401)`, sets `X-Middleware: token`), models `Task` (soft delete), `Comment`, `Measurement`, `Support/Cart`, routes `routes/web.grd` with group `/admin` + middleware `token`, schema `blueprint.surql` (tables `tasks`, `comments`, `measurements` with all SurrealDB types) and `manually.surql`). Tests with a database run `ops::push` (schema sync) against `mem://` before the actual flow.

Common pattern of the VM tests: `pool.run_sync(|vm| vm.run_file("t.grd", src))` runs a dialect script in a worker, collects `vm.output`, appends an error as `ERR …` and compares the text with a literal in the test. HTTP tests call `dispatch::handle(vm, HttpRequest)` in-process (no socket).

#### `opengrid-framework:tests/suite/middleware.rs::middleware_groups_and_abort`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact middleware::middleware_groups_and_abort`; runtime 0.01 s; in the suite: yes.
- **Checks:** route group with prefix `/admin` and middleware alias `token`: without token 401 with JSON `{"error":"HttpException","message":"token required"}`, with token 200; `abort(404, …)` in the controller yields 404 with the same JSON format; a path without the group prefix yields 404.
- **Kind:** integration; pool (1 worker) on the fixture app → four in-process requests → status and body against literals.
- **Oracle:** `own expectation` (the JSON error format is OpenGrid's own, no Laravel page; literals in the test).
- **Notes:** — **TEST2.5:** the prefix probe went against a route that does not exist at all (`POST /tasks/validated` → 404). Now: `POST /tasks` without prefix reaches the ungrouped `store` action (422 with its own message), `POST /admin/tasks` the group action `validated` (422 with `errors`), `GET /admin/hello` is 404 (prefix stays within the group), `GET /tasks` needs no token (middleware stays within the group).

#### `opengrid-framework:tests/suite/middleware.rs::validation_errors_and_validated_data`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact middleware::validation_errors_and_validated_data`; runtime 0.01 s; in the suite: yes.
- **Checks:** `request()->validate([...])` in the controller: on errors 422 with `message` and `errors` per field (required, between, email), on success only the validated fields (unknown `extra` is dropped), middleware decorates the response (`X-Middleware`), a form-urlencoded body yields strings (`"2"`, `"1"`).
- **Kind:** integration; three POSTs to `/admin/tasks` → status, headers, JSON fields or whole body against literals.
- **Oracle:** mixed. The error texts correspond to Laravel's English language files (Laravel 13), but are literals in the test, not generated from Laravel; the systematic Laravel check is done by `validation_spec`. Since TEST2.2b `message` is Laravel's summary "first error (and 2 more errors)", re-checked against Laravel 13.38 (`ValidationException::summarize`).
- **Notes:** —

#### `opengrid-framework:tests/suite/modules.rs::modules_from_git_pass_the_check_or_are_removed`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact modules::modules_from_git_pass_the_check_or_are_removed`; runtime 0.18 s; in the suite: yes.
- **Checks:** PKG1 `opengrid task module`: `add <git-url>` clones a module (name/version from `module.toml`), `list` and `verify` work; a module with a type error (variable changes from int to string, GRD021) is rejected (`does not pass the check` + `GRD021`) and removed again (directory gone, not in `list`); `remove` deletes the module.
- **Kind:** integration; `new_project` in a temp directory, two local git repos via `git init/commit` (`file://` URL) → `control::module_task` → return texts and file system.
- **Oracle:** `own expectation` (GRD021 rule from DIALECT/D10; message texts as prefix/`contains`).
- **Notes:** needs the `git` binary (otherwise panic, no skip). `verify` is checked only for `is_ok()`; `modules.lock` is not inspected directly.

#### `opengrid-framework:tests/suite/relations.rs::side_tables_aggregates_sorting_and_edges`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact relations::side_tables_aggregates_sorting_and_edges`; runtime 0.05 s; in the suite: yes.
- **Checks:** `withCount`, `withSum`, `withAll` with closure filter and sorting, `sort('title')`, sorting over a record-link path (`task_id.title:desc,hours:asc`), the SurrealQL generated from it (`toSql()`), graph edges `relate()` (edge with `in`/`out`/property) and `related()` (returns models, directed: reverse direction 0).
- **Kind:** integration; pool with `mem://` + schema push → a dialect script creates 2 tasks and 3 comments and prints everything → whole output against a literal.
- **Oracle:** `own expectation`; the data results (counts, sums, order) can be recomputed by hand and are executed by SurrealDB, the expected SQL text is purely the author's specification. Laravel has no counterpart for this (relations dropped, LARAVEL_BREAKING_CHANGES §D13).
- **Notes:** —

#### `opengrid-framework:tests/suite/relations.rs::request_fed_filter_and_sort`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact relations::request_fed_filter_and_sort`; runtime 0.05 s; in the suite: yes.
- **Checks:** `filter()`/`sort()` without arguments read `?filters[...]` and `?sort` from the request (DBL1): `filters[done]=true&sort=title:desc` → `C, A`; a non-filterable field (`password`) is rejected with 500/`ValueError`; without parameters all 3 rows.
- **Kind:** integration; three tasks via script, then three GET requests to `/tasks-filter` → title list, status, count.
- **Oracle:** `own expectation` (Laravel addition according to LARAVEL_BREAKING_CHANGES "additional").
- **Notes:** the rejection is checked only as 500 + text `ValueError` (no 4xx).

#### `opengrid-framework:tests/suite/reload.rs::explicit_reload_and_broken_source`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact reload::explicit_reload_and_broken_source`; runtime 0.09 s; in the suite: yes.
- **Checks:** hot reload with 2 workers: after changing the controller file and `pool.reload()` both workers return the new text; a broken source (top-level code in a class file, GRD103) leads to 500 with `GRD103` in the body; after resetting, the original text again.
- **Kind:** integration; copies the fixture app to a temp directory, rewrites the file → two requests each to `/hello` (meant to hit both workers) → body/status.
- **Oracle:** `own expectation` (GRD103 from DIALECT).
- **Notes:** that the two requests really land on two different workers is not enforced, only assumed. The header comment says "old code stays", but only the display of the error (500) is checked, not that the old code keeps answering.

#### `opengrid-framework:tests/suite/reload.rs::watcher_reloads_on_change`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact reload::watcher_reloads_on_change`; runtime 0.54 s; in the suite: yes.
- **Checks:** the file watcher (`watch(true)`) detects a change of the controller file and reloads without an explicit `reload()`.
- **Kind:** integration; temp copy, wait 300 ms, write file → poll `/hello` every 100 ms up to 5 s → body must be `<h1>Watched</h1>`.
- **Oracle:** `own expectation`.
- **Notes:** time-dependent (sleep 300 ms "watcher is up", polling with a 5 s deadline); possibly flaky on an overloaded machine.

#### `opengrid-framework:tests/suite/scaffold.rs::new_project_starts_and_answers`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact scaffold::new_project_starts_and_answers`; runtime 0.01 s; in the suite: yes.
- **Checks:** TP1 `opengrid new`: creates `config/development.yaml` with the app name, `.gitignore` with `/.opengrid/`; a second `new` into the same directory fails; the new project starts, `GET /` returns `{"app":"Demo","status":"ok"}`; `build_surql` with the framework fragment produces no warnings and contains the sessions table.
- **Kind:** integration; `new_project` in a temp directory → read files, start pool, one request, build schema.
- **Oracle:** `own expectation` (literals).
- **Notes:** —

#### `opengrid-framework:tests/suite/schedule.rs::due_entries_run_in_process`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schedule::due_entries_run_in_process`; runtime 0.06 s; in the suite: yes.
- **Checks:** J3/J4 scheduler in the server process: `Schedule::task(...)->dailyAt('03:00')`, `::job(...)->everyFifteenMinutes()`, `->weeklyOn(1, '8:30')` become cron expressions (`0 3 * * *`, `*/15 * * * *`, `30 8 * * 1`); `scheduler::tick` at fixed points in time (Monday 2026-10-26 03:00, 03:07, 08:30 UTC) starts exactly the due entries in declaration order; the task runs directly, the job via the database queue (runner every 50 ms); arguments arrive (`monday,nightly`); an invalid cron (`cron('every day')`) is rejected when written.
- **Kind:** integration; new project + three dialect files (task, job, `routes/console.grd`), pool with 2 workers on `mem://` → `entries`, three `tick`s → return lists and counted rows in table `stamps`.
- **Oracle:** `own expectation`; the cron translation corresponds to Laravel's frequency methods (docs), but is a literal in the test, not generated against Laravel's `Schedule`.
- **Notes:** points in time are passed fixed (no clock dependency), but waiting for the job runner is polling (20 ms, deadline 15 s). `count()` returns -1 for unparsable output and 0 as long as the table is missing.

#### `opengrid-framework:tests/suite/schedule.rs::frequencies_are_laravels_cron_expressions`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schedule::frequencies_are_laravels_cron_expressions`; runtime ≈ 0.3 s; in the suite: yes.
- **Checks:** 16 frequency calls (`everyMinute` … `monthly`, `dailyAt('03:00'|'8:30'|'23')`, `weeklyOn(1, '8:30')`, `weeklyOn(5)`, `hourlyAt(17)`, `cron(…)`) yield the same cron expressions as Laravel's `Scheduling\Event::getExpression()` (from the `Schedule` of the LV1 app).
- **Kind:** differential against an external oracle; the entries are created at runtime on a worker and read via `scheduler::entries`.
- **Oracle:** `independent` (Laravel 13.38).
- **Notes:** TEST2.6. Found a deviation during implementation: `everyThirtyMinutes` was `0,30 * * * *`, Laravel writes `*/30 * * * *` (same meaning) → aligned.

#### `opengrid-framework:tests/suite/schema_files.rs::project_schema_merges_and_applies`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_files::project_schema_merges_and_applies`; runtime 0.01 s; in the suite: yes.
- **Checks:** R4b `build_surql`: merging of framework fragment, `database/sources/main.surql`, all `blueprint.surql`, all `manually.surql` in a fixed order; `manually` wins also across modules; placeholder `DEFINE TABLE invoices TYPE NORMAL SCHEMAFULL PERMISSIONS NONE;` for tables known only via fields; a later file with only one field leaves the table definition in place (R4h); tables after `ksort` (`_` before lowercase letters); comments are not carried over; unsupported `DEFINE ANALYZER` yields exactly one warning with source; SurrealDB (`mem://`) accepts the generated text without error and `INFO FOR TABLE` contains index and default.
- **Kind:** integration; temp project with four schema files → `build_surql` → warnings, table order, `contains` on individual lines → execution in embedded SurrealDB.
- **Oracle:** mixed: layout/order `own expectation` (derived from the behavior of the reference classes, which `schema_golden` checks independently); validity of the result `independent` through SurrealDB itself.
- **Notes:** —

#### `opengrid-framework:tests/suite/schema_golden.rs::schema_port_matches_php_on_edge_cases`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_golden::schema_port_matches_php_on_edge_cases`; runtime 0.00 s; in the suite: yes.
- **Checks:** the Rust port of the schema tool of a private reference implementation (parser, merge, generator; `Schema::merge_source` + `generate`) produces byte-identical frozen output for 16 own edge cases.
- **Kind:** golden; for every directory `fixtures/schema_golden/cases/NN_name/` the `*.surql` are merged in file-name order → Rust output → comparison with `expected.r4h.out` if present, otherwise `expected.out`; an error reports the first deviating line.
- **Oracle:** mixed. `expected.out` (all 16 cases) is the output of the **PHP classes of the private reference implementation** under PHP 8.4, generated with `fixtures/schema_golden/driver.php` (loads the reference classes at runtime from outside the repository via an autoloader, copies nothing; mode `files <file>…` merges in order and prints the generator output) – `independent`. For 5 cases (`03_comments`, `05_merge`, `07_variants`, `15_unicode`, `16_numeric_names`) `expected.r4h.out` lies next to it: the deliberately deviating target after fixing the findings of the original (R4h, DECISIONS 2026-10-24 "R4b2 and R4h": comments `//`, `#`, `/* */`; merge by name instead of `array_merge`; `IF NOT EXISTS`/`OVERWRITE`; non-ASCII/backtick names; `\'` escape; placeholder does not overwrite a definition) – these files are written with `GRD_GOLDEN_WRITE=1` **from the Rust output itself** (review by diff), so they are `own expectation`. Example `03_comments`: PHP keeps only table `t` and field `d` (the fields behind `//`, `#`, `/* */` are lost), `expected.r4h.out` contains `a`, `b`, `c`, `d`. Cases: basic, strings, comments, placeholder, merge (3 files), case_spacing, variants, other_statements, sorting, whitespace, unterminated, backticks, empty, crlf, unicode, numeric_names.
- **Notes:** always runs (no PHP needed). `GRD_GOLDEN_WRITE=1` switches the test into a write mode in which it checks nothing.

#### `opengrid-framework:tests/suite/schema_golden.rs::golden_expectations_are_current_php_output`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_golden::golden_expectations_are_current_php_output`; runtime 0.76 s; in the suite: yes.
- **Checks:** the frozen `expected.out` are still the current output of the reference PHP classes (protects against stale goldens when the reference sources change).
- **Kind:** differential test/golden maintenance; for every case `php driver.php <tooling> files <files>` → comparison with `expected.out`.
- **Oracle:** `independent` (reference PHP under `php`).
- **Notes:** skips itself (only `eprintln!`, test stays green) when `php` is missing or the reference implementation's `SurqlFileParser.php` does not exist. In the current container both are present, so it runs. The reference implementation is confidential and not in the repo: on every other machine (CI, clone) the test is a no-op.

#### `opengrid-framework:tests/suite/schema_golden.rs::schema_port_matches_php_on_platform_files`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_golden::schema_port_matches_php_on_platform_files`; runtime 0.08 s; in the suite: yes.
- **Checks:** with the real (confidential) schema files of the reference implementation, the Rust port produces the same output as the reference PHP classes, in the file order of MigratePush (`main.surql`, then all `blueprint.surql`, then all `manually.surql`, each glob-sorted).
- **Kind:** differential test; file list from the reference implementation's schema tooling at runtime → `php driver.php <tooling> project <tooling>` against `rust_output(files)`; nothing is stored.
- **Oracle:** `independent` (reference PHP against Rust on real data).
- **Notes:** same self-skipping as above (php + reference implementation). Because the R4h deviations only apply to edge cases, this test shows that the real reference files are not affected by them. The file order is rebuilt independently in Rust and in `driver.php`; if both agree but not with MigratePush, that would go unnoticed.

#### `opengrid-framework:tests/suite/schema_kit.rs::sync_mem_is_idempotent_and_prunes_only_managed`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_kit::sync_mem_is_idempotent_and_prunes_only_managed`; runtime 0.03 s; in the suite: yes.
- **Checks:** R4c/D4 embedded SurrealKit `kit::sync` against `mem://`: the first synchronization creates tables and writes `temporary.surql`; the second does not change the managed entries (`__entity`, ns `schema`) and leaves data in place; prune removes only managed tables, a foreign-created one (`foreign_tbl`) remains; an empty schema is rejected (`empty`) instead of deleting everything.
- **Kind:** integration; direct calls of `kit::sync` with schema strings → `INFO FOR DB`, `__entity` dump, data query.
- **Oracle:** `own expectation`; the state is read via SurrealDB itself (`INFO FOR DB`).
- **Notes:** —

#### `opengrid-framework:tests/suite/schema_kit.rs::sync_surrealkv_persists`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_kit::sync_surrealkv_persists`; runtime 0.21 s; in the suite: yes.
- **Checks:** a schema created with `kit::sync` in a `surrealkv://` file survives a process restart.
- **Kind:** integration via child processes; starts its own test binary twice (`--exact schema_kit::surrealkv_child --ignored --nocapture`) with `OG_KV_URL`, `OG_KV_ROOT`, `OG_KV_MODE=sync` and `read` respectively; checks exit status and `1 passed` in the output.
- **Oracle:** `own expectation` (table list `notes, tasks`).
- **Notes:** child processes are necessary because surrealkv holds its file lock for the lifetime of the process (finding 2026-10-24, comment in the test).

#### `opengrid-framework:tests/suite/schema_kit.rs::surrealkv_child`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_kit::surrealkv_child --ignored` (only meaningful as a child process); `#[ignore]`, skipped in the suite.
- **Checks:** half of `sync_surrealkv_persists`: in mode `sync` it synchronizes `V1` into the surrealkv file, in mode `read` it reopens the file and expects `notes, tasks`.
- **Kind:** helper test (child process); reads URL/root/mode from the environment.
- **Oracle:** as the parent test.
- **Notes:** `#[ignore = "runs as child process of sync_surrealkv_persists"]`; only started by the parent test with `--ignored`. Without the three environment variables it returns green immediately (so a manual `cargo test -- --ignored` is a no-op). The parent test detects success by `1 passed`, which would also appear on this early return; but the parent test always sets the variables.

#### `opengrid-framework:tests/suite/schema_kit.rs::rollout_cycle_on_persistent_embedded_db`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_kit::rollout_cycle_on_persistent_embedded_db`; runtime 1.28 s; in the suite: yes.
- **Checks:** rollout cycle on a persistent embedded DB: `baseline`, `plan` (report `passed`, addition `table invoices`, manifest `database/rollouts/<id>.toml`, id ends in `__add_invoices`), `start` (expand), `complete`; a second rollout `payments` with `start` and `rollback` removes the table again; `__rollout` status `completed`, `rolled_back`.
- **Kind:** integration; calls of the kit functions → `INFO FOR DB`, file system, `__rollout` table.
- **Oracle:** `own expectation` (SurrealKit semantics; state read via SurrealDB).
- **Notes:** sleep 1.1 s, because manifest ids have second resolution.

#### `opengrid-framework:tests/suite/schema_kit.rs::rollout_start_without_baseline_says_what_to_do`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_kit::rollout_start_without_baseline_says_what_to_do`; runtime 0.03 s; in the suite: yes.
- **Checks:** `rollout start` without a prior `rollout baseline` fails with the hint "run `rollout baseline` first" instead of an opaque error.
- **Kind:** integration; sync, plan, start → error text `contains`.
- **Oracle:** `own expectation`.

#### `opengrid-framework:tests/suite/schema_kit.rs::preflight_allows_only_additions`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_kit::preflight_allows_only_additions`; runtime 0.12 s; in the suite: yes.
- **Checks:** preflight (Jonas 2026-10-24, "rollouts may only extend"): `plan` rejects removing a table, retyping a field and a new required field on an existing table (the plan is not stored); an optional field + a new table pass; a frozen target schema changed by hand (retyping) and a manifest with an additional `run_sql` step (`DELETE tasks;`) are rejected at `start` without applying anything.
- **Kind:** integration on surrealkv; several `write_temporary` + `rollout_plan`/`rollout_start` → error texts, number of manifests, table list.
- **Oracle:** `own expectation`.
- **Notes:** the first check is very soft: `contains("table notes exists and would be removed") || contains("drop_") || contains("remove")`; the retyping check at plan time only checks `!e.0.is_empty()`. Calls `chmod` as a binary (return value ignored). **TEST2.5:** the messages are now compared exactly (rollout id masked as `<id>`): removing a table and a new required field as the whole preflight text; the type change via the first two lines of SurrealKit's own rejection (before: `contains("remove")` as an alternative, or only "not empty").

#### `opengrid-framework:tests/suite/schema_ops.rs::push_pull_fresh`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_ops::push_pull_fresh`; runtime 0.07 s; in the suite: yes.
- **Checks:** R4d developer tasks: `push` creates module schema + framework fragment (`_opengrid_sessions`, `failed_jobs`, `jobs`, `personal_access_tokens`) and reports `DEFINE FUNCTION` as one warning; `pull` reads the schema back as text (without SurrealKit tables); `fresh` with and without `keep_db` deletes data and restores the schema.
- **Kind:** integration; temp project with module `shop` → `ops::push/pull/fresh` against `mem://` → table lists, `contains`, empty data query.
- **Oracle:** `own expectation`.
- **Notes:** —

#### `opengrid-framework:tests/suite/schema_ops.rs::real_info_answers_become_the_model`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_ops::real_info_answers_become_the_model`; runtime ≈ 0.05 s; in the suite: yes.
- **Checks:** real `INFO FOR DB`/`INFO FOR TABLE` answers of a `mem://` SurrealDB instead of invented ones: 12 field type spellings (`option<int>`, `none | int`, `null | datetime`, `option<record<a | b>>`, `array<string>`, `object FLEXIBLE`, `decimal DEFAULT`, `ASSERT`, `int | string`, `option<set<record<a>>>`, `array<int, 3>` …) → `from_info` → `field_type` → `base_type` yields the intended base type; furthermore `ops::pull` is a fixed point (output applied to an empty DB and pulled again = equal), and the element fields created by SurrealDB itself (`l.*`) are absent from the output.
- **Kind:** integration against an external oracle (SurrealDB 3.3.4 itself).
- **Oracle:** `independent` for the INFO forms (SurrealDB), `own expectation` for the intended base types (from the definitions).
- **Notes:** TEST2.6. Found two things: (1) `pull` also wrote out the automatic element fields (`DEFINE FIELD l.* …`), the output then could not be reapplied ("field 'l.*' already exists") → `from_info` omits them when they exactly match the automatic definition. (2) SurrealDB 3 requires `TYPE object FLEXIBLE` (the unit test `parser.rs::field_types` still uses the 2.x order `FLEXIBLE TYPE`, which the parser continues to read). Open (KNOWN_ISSUES): an element field with its own clauses can only be defined in SurrealDB 3 with `OVERWRITE` and comes out of `pull` without it.

#### `opengrid-framework:tests/suite/schema_ops.rs::production_start_runs_planned_rollouts`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_ops::production_start_runs_planned_rollouts`; runtime 1.28 s; in the suite: yes.
- **Checks:** `apply_planned` (production start): without rollouts nothing to do; two rollouts planned on a development DB run in order on an empty production DB; a second start finds nothing more.
- **Kind:** integration; `plan_two` (push, plan, extend blueprint, push, plan) on DB `planning`, then `apply_planned` on DB `prod` (both `mem://`).
- **Oracle:** `own expectation`.
- **Notes:** sleep 1.1 s in `plan_two` (second-resolution ids).

#### `opengrid-framework:tests/suite/schema_ops.rs::production_start_refuses_a_failing_preflight_without_changes`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_ops::production_start_refuses_a_failing_preflight_without_changes`; runtime 1.18 s; in the suite: yes.
- **Checks:** if the preflight of a later rollout fails (manifest extended by hand with `DELETE products;`), the first one does not run either: the error contains `nothing changed` and `` `wipe` ``, the production DB stays without tables.
- **Kind:** integration; as above, edit manifest → `apply_planned` → error text, empty table list.
- **Oracle:** `own expectation`.
- **Notes:** —

#### `opengrid-framework:tests/suite/schema_ops.rs::production_start_rolls_back_a_failing_rollout`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_ops::production_start_rolls_back_a_failing_rollout`; runtime 1.26 s; in the suite: yes.
- **Checks:** a rollout whose step fails on real data (UNIQUE index on duplicate `sku`) is rolled back: the error names the id and `rolled back`, the index is removed, `__rollout` shows `completed, rolled_back`, data stays untouched.
- **Kind:** integration; first rollout applied by hand, two rows with the same `sku`, then `apply_planned`.
- **Oracle:** mixed: the failure itself comes `independent` from SurrealDB (unique violation); rollback behavior `own expectation`.
- **Notes:** —

#### `opengrid-framework:tests/suite/schema_ops.rs::plan_from_sources_and_refuse_empty_plans`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact schema_ops::plan_from_sources_and_refuse_empty_plans`; runtime 1.15 s; in the suite: yes.
- **Checks:** regression from the CLI smoke test of 2026-10-24: `ops::plan` reads the module sources directly (without a prior push), a plan without changes is rejected (`has not changed`) and leaves no empty manifest behind.
- **Kind:** integration; push, plan, second plan (error), change blueprint, plan → report with addition `field products.archived`.
- **Oracle:** `own expectation`.
- **Notes:** sleep 1.1 s.

#### `opengrid-framework:tests/suite/server_mode.rs::typed_filters_read_the_servers_field_types`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact server_mode::typed_filters_read_the_servers_field_types`; runtime 1.40 s; in the suite: yes.
- **Checks:** against a real SurrealDB server (ws://) `option<int>` appears in `INFO FOR TABLE` as `none | int`; the type cast of the filters reads this form, so that `filter(['priority' => '2'])` finds two int rows.
- **Kind:** integration; `TestServer`, pool with schema push → dialect script → output `server-form 2`.
- **Oracle:** mixed: the server form of the type comes `independent` from the SurrealDB server (queried and printed in the test); the filter count is own expectation.
- **Notes:** needs the `surreal` binary or `OPENGRID_TEST_DB_URL`, otherwise silent return (green, only `eprintln!`). In the container `surreal` is present. Starts a server process, waits up to 20 s for the port.

#### `opengrid-framework:tests/suite/server_mode.rs::crud_and_soft_delete_over_ws`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact server_mode::crud_and_soft_delete_over_ws`; runtime 1.36 s; in the suite: yes.
- **Checks:** CRUD over the server connection: create, update (`done`), soft delete (`find` returns null, `withTrashed()` counts 2, normal 1), `paginate(1, 1)` with `total`/`last_page`.
- **Kind:** integration; as above, output `updated gone 2 1 1/1`.
- **Oracle:** `own expectation` (Laravel soft-delete semantics, not checked against Laravel).
- **Notes:** as above (server or silent skip).

#### `opengrid-framework:tests/suite/server_mode.rs::concurrent_writes_to_one_record_over_ws`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact server_mode::concurrent_writes_to_one_record_over_ws`; runtime 1.78 s; in the suite: yes.
- **Checks:** 8 threads × 25 `UPDATE counter:1 MERGE` against one record over 4 workers and the server: no write fails.
- **Kind:** integration/load; threads → every output must be `ok`.
- **Oracle:** `own expectation`.
- **Notes:** as above. Only "no error" is checked, not the final value (with `MERGE` of fixed values that would not be meaningful either); concurrency depends on scheduling. **TEST2.5:** final state checked (last write value of a thread). Over the server **0** conflicts occurred during implementation (the server serializes the writes); the number is only printed, not required, so the test still shows only "no error, correct final state".

#### `opengrid-framework:tests/suite/session.rs::memory_store`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact session::memory_store`; runtime 0.01 s; in the suite: yes.
- **Checks:** sessions with the memory store via `exercise()`: cookie `opengrid_session=…` with `HttpOnly` for a new session, data and counter across requests, no new cookie for an existing session, isolation without cookie, flash for exactly one request, unknown session id → new session, an object bound to the session (`Cart`, SES1) is restored instead of rebuilt and discarded with `forget`, CSRF (419 without token, 200 with `x-csrf-token` or `_token` in the JSON body), `invalidate` (`Max-Age=0`, data gone); afterwards at least 2 sessions in the store.
- **Kind:** integration; pool with 2 workers, ~25 in-process requests to `SessionController` → status, headers, JSON fields.
- **Oracle:** `own expectation` (oriented on Laravel sessions/CSRF incl. status 419, but literals).
- **Notes:** —

#### `opengrid-framework:tests/suite/session.rs::database_store`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact session::database_store`; runtime 0.02 s; in the suite: yes.
- **Checks:** the same flow `exercise()` with `SessionDriver::Database` on `mem://`; afterwards at least 2 rows in `_opengrid_sessions`.
- **Kind:** integration; as above plus count via `DB::select`.
- **Oracle:** `own expectation`.
- **Notes:** —

#### `opengrid-framework:tests/suite/test_app.rs::test_app_crud`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact test_app::test_app_crud`; runtime 0.32 s; in the suite: yes.
- **Checks:** TP3/TP5 in-process smoke test of the test project `examples/test-app`: loads with `mem://`, schema push without warnings, `GET /`, `/health` (`status ok`, app name, `database.mode = embedded`, version starts with `3`), register 201, login 200 with token, notes without token 401, create note (201, header `x-module: notes`, default `priority = 3` from the schema), form body 415, list with `sort` and `filter[done]=false`, PATCH, DELETE 204, afterwards 404.
- **Kind:** integration; copy of the test project (without `.opengrid`) to temp, pool with 2 workers → requests via the real dispatcher.
- **Oracle:** `own expectation` (literals; the complete flow and the Laravel comparison live in `api.sh`, see `group:api-sh`).
- **Notes:** the SurrealDB version `3…` depends on the linked SDK version.

#### `opengrid-framework:tests/suite/transactions.rs::transactions_retry_conflicts_embedded`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact transactions::transactions_retry_conflicts_embedded`; runtime 0.27 s; in the suite: yes.
- **Checks:** TX1: `DB::transaction($fn, 200)` retries on a write conflict; 6 threads × 20 read-increment-write transactions on `counter:1` over 4 workers with `surrealkv://` yield exactly 120 (no lost update) and no failed transaction.
- **Kind:** integration/concurrency; pool on surrealkv in a temp directory → threads → final value.
- **Oracle:** `independent` in the sense of an invariant (sum of the increments = final value), no author literals except the arithmetic.
- **Notes:** concurrent, depends on scheduling; whether conflicts occur at all and the retry is thus exercised is not checked by the test. With very many conflicts 200 attempts might theoretically not suffice. **TEST2.5:** additionally `Db::conflict_retries()` must be > 0: the closures really collided and ran again (`DB::transaction` counts its retries there); 5 of 5 repetitions green.

#### `opengrid-framework:tests/suite/transactions.rs::transactions_retry_conflicts_on_a_server`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact transactions::transactions_retry_conflicts_on_a_server`; runtime 10.87 s; in the suite: yes.
- **Checks:** the same against a SurrealDB server (ws://).
- **Kind:** as above with `TestServer`.
- **Oracle:** as above.
- **Notes:** silent skip without `surreal`/`OPENGRID_TEST_DB_URL`. **TEST2.5:** additionally `Db::conflict_retries()` must be > 0: the closures really collided and ran again (`DB::transaction` counts its retries there); 5 of 5 repetitions green.

#### `opengrid-framework:tests/suite/transactions.rs::transaction_arguments`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact transactions::transaction_arguments`; runtime 0.04 s; in the suite: yes.
- **Checks:** `DB::transaction(fn, 0)` does not run the closure and returns `null` (since TEST2.2c, before `ValueError`); a string as attempt count is a `TypeError` (strict types); a nested transaction passes the inner return value through; an exception from the closure runs (with 5 attempts) exactly once and is passed on.
- **Kind:** integration; a script on `mem://` → output `NULL\nstring rejected nested try boom` (the single `try` shows the single run).
- **Oracle:** mixed: `attempts = 0` follows Laravel's loop `for ($currentAttempt = 1; $currentAttempt <= $attempts; …)` in `ManagesTransactions` (re-checked in vendor 13.38); the rest is own expectation.
- **Notes:** TEST2.2c.

#### `opengrid-framework:tests/suite/types.rs::round_trip_in_memory`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact types::round_trip_in_memory`; runtime 0.04 s; in the suite: yes.
- **Checks:** SurrealDB types as native classes (DECISIONS 2026-10-24 "SurrealDB types native"): round trip through a model for `Decimal`, `Duration`, `Uuid`, `Bytes`, `Geometry` (point), `DateTimeImmutable` (Europe/Berlin → UTC), `RecordId`, NONE/null, int, float; `json_encode` of every type; arithmetic via native handlers (Decimal ±×/, Duration +×>, DateTime + Duration), `Decimal + 'abc'` → TypeError; `whereNull`/`whereNotNull` on NONE, schema-type-aware filter casts (`'3'` → int, `'false'` → bool, RecordId string), setting and resetting to null.
- **Kind:** integration; a long dialect script on `mem://` → whole output against `EXPECTED` (the random record id masked via regex).
- **Oracle:** mixed: the stored/read representation comes from SurrealDB, the expected values are author literals. Individual values can be verified independently (timestamp `1792837800` and base64 `AAFiaW5hcnk=` agree with `php -r`); decimal formatting (`13` instead of `13.000`) and duration formats (`1h30m`, `2h`) follow SurrealDB/rust_decimal, without reference to a source in the test.
- **Notes:** —

#### `opengrid-framework:tests/suite/types.rs::round_trip_real_server`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact types::round_trip_real_server`; runtime 1.41 s; in the suite: yes.
- **Checks:** the same round trip against a SurrealDB server; cleans up the tables afterwards.
- **Kind:** as above with `TestServer`.
- **Oracle:** as above.
- **Notes:** silent skip without `surreal`/`OPENGRID_TEST_DB_URL`. The file's header comment mentions only `OPENGRID_TEST_DB_URL`; in fact `TestServer` also starts its own server.

#### `opengrid-framework:tests/suite/types.rs::lazy_rows`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact types::lazy_rows`; runtime 0.03 s; in the suite: yes.
- **Checks:** B2a: a loaded model holds the raw SDK fields (`pending`) and converts a field only on read (`amount` afterwards in `attributes`, `pending` one smaller); `toArray()` converts the rest (`pending = 0`); writing a field discards its raw value and marks it `dirty`; `clone` keeps the whole payload.
- **Kind:** unit-like white-box test via the Rust API (`vm.native_payload::<ModelData>`, `get_property`, `call_method_sync`, `set_property`) → state lines against literals.
- **Oracle:** `own expectation` (internal design, RUNTIME_ARCHITECTURE/B2a).
- **Notes:** bound to internal fields (`pending`, `attributes`, `dirty`); breaks on a rework of the model internals without any change in behavior.

#### `opengrid-framework:tests/suite/validation_spec.rs::validation_matches_laravel`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact validation_spec::validation_matches_laravel`; runtime 0.03 s; in the suite: yes.
- **Checks:** R3/D12: OpenGrid's `Validator::errors`/`Validator::validate` return, for every line of `validation_spec/cases.txt`, the same error messages or the same validated data as Laravel's validator.
- **Kind:** specification (data-driven); per case a dialect script (`json_decode` of rules and data, output `{"errors":…}` or `{"valid":…}`) → JSON comparison (`serde_json::Value`, object key order irrelevant, array order of the messages counts) with the line of the same name in `cases.out`; all deviations are collected and reported.
- **Oracle:** `independent`: Laravel's own classes (Laravel 13, `laravel/framework` v13.38.0 in the vendor of the comparison app), see `group:validation-cases`.
- **Notes:** line counts of `cases.txt` (without comments/blank lines) and `cases.out` must match, otherwise abort "cases.out is out of date". The oracle is not consulted at runtime: `cases.out` is frozen; a comparison with a new Laravel version requires manual regeneration. **TEST2.7:** 27 new cases for wildcards (`items.*.name`, `users.*.emails.*`, missing/empty arrays) and the `required_if`/`_unless`/`_with`/`_with_all`/`_without`/`_without_all` family; both were implemented for this and agree with Laravel right away.

#### `opengrid-framework:tests/suite/validation_spec.rs::cases_out_is_laravel_output`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact validation_spec::cases_out_is_laravel_output`; runtime ≈ 0.1 s; in the suite: yes.
- **Checks:** `cases.out` is Laravel's current answer: `driver.php` runs with the vendor of the LV1 app over `cases.txt`, line by line equal to `cases.out`. `GRD_SPEC_WRITE=1` rewrites `cases.out`.
- **Kind:** recheck of the frozen expectation against an external oracle.
- **Oracle:** `independent` (Laravel 13.38).
- **Notes:** TEST2.6. New in `cases.txt`: `alpha_dash` (twice), `bool`, `int` (the rules had no failing case so far).

#### `opengrid-framework:tests/suite/validation_spec.rs::database_rule_messages_are_laravels`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact validation_spec::database_rule_messages_are_laravels`; runtime ≈ 0.1 s; in the suite: yes.
- **Checks:** the messages of `unique` and `exists` (Laravel needs a database for them, hence not in `cases.txt`) equal the texts from Laravel's `lang/en/validation.php` (`:attribute` replaced); OpenGrid side on `mem://` with the fixture schema.
- **Kind:** integration against an external oracle (language file).
- **Oracle:** `independent` (Laravel 13.38, `lang/en/validation.php`).
- **Notes:** TEST2.6. All other rule texts are checked by `validation_matches_laravel` via Laravel's validator itself.

#### `opengrid-framework:tests/suite/php_oracles.rs::query_strings_parse_like_php`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact php_oracles::query_strings_parse_like_php`; runtime < 0.1 s; in the suite: yes.
- **Checks:** 16 query strings (nested, `[]`, duplicate keys, `.`/spaces in the name, open/leading brackets, empty names, percent encoding, numeric keys) yield in **both** parsers the same as PHP's `parse_str`: dialect `parse_str` (VM) and the request parser (`http::parse_query` + `request::nested_array`). Comparison as JSON with forced objects.
- **Kind:** differential against an external oracle.
- **Oracle:** `independent` (PHP 8.4).
- **Notes:** TEST2.6. Found four deviations in both parsers (`.`/spaces in the name not turned into `_`, open first bracket not turned into `_`, leading bracket and empty name not discarded) → both now use a function after PHP's `php_register_variable_ex` (`grd_vm::builtins::insert_query_key`).

#### `opengrid-framework:tests/suite/php_oracles.rs::table_names_singularize_like_laravel`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact php_oracles::table_names_singularize_like_laravel`; runtime < 0.1 s; in the suite: yes.
- **Checks:** `inflect::singular` (foreign key convention) for 67 table names equals Laravel's `Str::singular`.
- **Kind:** differential against an external oracle.
- **Oracle:** `independent` (Laravel 13.38, Doctrine inflector).
- **Notes:** TEST2.6. Found 7 deviations out of 22 (`statuses`, `analyses`, `indices`, `quizzes`, `buses`, `wolves`, `mice`) → `singular` extended following Laravel's rules for this word list (exceptions, irregular forms, endings; last word of a snake_case name). Words outside the list may still deviate.

#### `opengrid-framework:tests/suite/php_oracles.rs::task_classes_are_laravels_studly_names`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact php_oracles::task_classes_are_laravels_studly_names`; runtime < 0.1 s; in the suite: yes.
- **Checks:** `control::task_class` for 6 task names equals `Str::studly(module) . '\Tasks\' . Str::studly(name)`.
- **Kind:** differential against an external oracle.
- **Oracle:** `independent` (Laravel 13.38).
- **Notes:** TEST2.6. No deviation.

#### `group:validation-cases`
- **What:** `crates/framework/tests/validation_spec/` with `cases.txt` (one line per case: rules as JSON, tab, data as JSON; one comment line), `cases.out` (one JSON line per case) and `driver.php`.
- **Creation:** `php driver.php [vendor-dir] < cases.txt > cases.out`. The driver loads the Composer autoloader of the Laravel comparison application (default `../../../../../bench/laravel-app/vendor`, outside the repo, LV1/`setup.sh`), builds an `Illuminate\Validation\Factory` with `Translator`/`ArrayLoader` and Laravel's own `lang/en/validation.php`, and prints per case `{"errors": $v->errors()->toArray()}` or `{"valid": $v->validated()}`. So it is **Laravel's real validator** (v13.38.0), no database (no `exists`/`unique`). Nothing of Laravel lives in the repo, only the output.
- **Scope:** 132 cases. Rules (occurrences): `integer` 23, `string` 13, `numeric` 12, `required` 11, `boolean` 11, `min`/`max` 9 each, `accepted` 9, `array` 7, `email` 6, `alpha`/`alpha_num`/`alpha_dash` 6 each, `between` 6, `url` 5, `date` 5, `uuid`/`size`/`nullable`/`in`/`confirmed` 3 each, `sometimes`/`not_in`/`regex`/`same` 2 each, `different`/`bail` 1 each; rules as string and as array form; some multi-field cases (e.g. field `e-mail`, extra fields that are dropped in `valid`).
- **Examples:** `{"pw": "confirmed"}` + `{"pw": "a", "pw_confirmation": "b"}` → `{"errors":{"pw":["The pw field confirmation does not match."]}}`; `{"a": "integer|between:1,5"}` + `{"a": "3"}` → `{"valid":{"a":"3"}}` (string stays string); `{"a": "required"}` + `{"a": "0"}` → `{"valid":{"a":"0"}}`.
- **Not covered:** wildcards/nested fields (`items.*`, `a.b`), `exists`/`unique`, file rules, the `required_if`/`required_with` family, custom messages/attribute names.

#### `group:api-sh`
- **What:** `examples/test-app/tests/api.sh`, acceptance test TP5 of the test project (modules `auth`, `notes`) against a **real server process** over HTTP with `curl` and `jq`.
- **Integration into the suite:** `crates/framework/tests/suite/api_script.rs::test_app_api_script` starts `bash api.sh` with `OPENGRID_BIN = env!("CARGO_BIN_EXE_opengrid")` (the `opengrid` binary of this build) and requires exit 0, `steps passed` in the output and no `FAIL `. (`test_app.rs::test_app_crud` is the smaller in-process smoke test, no call of `api.sh`.)
- **Flow:** checks `curl`, `jq`, `python3` (if one is missing: `FAIL setup`, exit 2, i.e. the suite test fails, it does not skip); copies the project without `.opengrid` and `tests` to `mktemp -d`; fresh database `OPENGRID_DB_URL=surrealkv://$WORK/db`; free port via Python; the server is terminated in any case via `trap`, on errors the last 30 lines of the server log are shown. Helpers: `req METHOD PATH [JSON] [TOKEN]` sets `STATUS`/`BODY`, `check NAME STATUS [jq expression]` counts a step as PASS/FAIL; a FAIL does not abort (except in setup/start).
- **Steps (counted at runtime, OpenGrid mode): 45**, all expected as PASS:
  1. Setup (3): `opengrid task migrate_push`; server starts and `/health` answers (polling up to 100 × 0.2 s); `/health` with `status ok` and a non-empty DB version.
  2. `health: embedded database` (1): `database.mode == "embedded"`.
  3. Registration/login (8): register 201 without `password` in the body; duplicate e-mail 422 `The email has already been taken.`; invalid data 422 with e-mail message and exactly two password messages (min:8, confirmed) in Laravel's wording; wrong password 401; unknown e-mail 401; both 401 responses are byte-identical (no user enumeration leak, own step without `check`); login 200 with a 64-character token and user without `password`; `/api/me`.
  4. Notes (16): without token 401 `Unauthenticated.`; invalid token 401; create three notes (default `priority 3` from the schema, `done`); invalid data 422; list `total 3`, `current_page 1`; filter `done=true`/`false`; sorting `priority` ascending and descending; SQL-like `sort=priority;DELETE notes` → 400; pagination (`per_page=2&page=2`, last page with 1 entry `Urlaub`); read one note; PATCH (partial update leaves `body` in place); PATCH with `priority 0` → 422.
  5. Second user (7): register, login; read/modify/delete a foreign note 404 each; own list `total 0`; unknown note id 404 (with the first token).
  6. Delete/logout (5): DELETE 204, afterwards 404, list `total 2`, logout 204, `/api/me` with the old token 401.
  7. Restart on the same embedded DB (5): stop and restart the server; login after restart; data survived (`Steuer`, `Urlaub`); the second user's token stays valid; `opengrid task run task:notes:stats` runs over the control socket in the running server and prints `notes: 2` (J1/D5; on its own the task could not open the locked embedded DB).
  Loops exist only while waiting for `/health` and in the SKIP outputs; every `check` line and every direct `pass/fail` line is exactly one step.
- **Mode `--app laravel [--runtime fpm|octane]`:** the same requests against the Laravel replica (LV1; app under `OpenGrid_Framework/bench/laravel-app`, outside the repo, built by `bench/framework/laravel/setup.sh` with the overlay `bench/framework/laravel/overlay` (AuthController, NoteController, HealthController, middleware `JsonWrites`, models `User`/`Note`), started by `serve.sh` with nginx+PHP-FPM or Octane/RoadRunner, database SQLite via `DB_CONNECTION=sqlite`). Setup is `php artisan migrate --force` instead of `migrate_push`. Result: 39 steps, 6 SKIP. **OpenGrid only (SKIP in Laravel mode):** `health: embedded database` and the five restart steps (`restart on the same database`, `login after restart`, `data survived the restart`, `the second user's token survived the restart`, `task through the control socket`).
- **Oracle:** the expectations are `own expectation` as `jq` expressions in the script (message texts in Laravel's wording). The Laravel mode is a differential check "both implementations satisfy the same expectations"; but the Laravel side was also written by the project itself (overlay), does not run in the suite and needs the external app.
- **Notes:** needs `curl`, `jq`, `python3` (and for Laravel `php`, nginx/FPM or Octane); starts real processes and ports; waits up to 20 s for the start; cleanup via PID and `trap`.


### 2.5 opengrid-framework: unit tests

In general: all 47 tests are Rust unit tests directly next to the code. Their expectations are **without exception literals in the test**; none calls PHP, Laravel or the reference implementation at runtime. Where an independent oracle exists, it lives in **separate integration tests** outside this group:
- Schema port: `crates/framework/tests/suite/schema_golden.rs` (golden tests R4a: 16 edge cases under `tests/fixtures/schema_golden/cases/*/expected.out`, frozen with PHP 8.4 and the reference classes via `driver.php`; deviations after R4h in `expected.r4h.out`; the real reference files are compared PHP against Rust when `php` and the reference implementation's schema tooling are present).
- Validation: `crates/framework/tests/validation_spec/` (`cases.txt` → `cases.out`, generated by Laravel's own validator via `driver.php` from `bench/laravel-app/vendor`).

For this report I held the literals of the schema and validation tests once by hand against these oracles (reference driver `schema_golden/driver.php` or Laravel 13.38 from `bench/laravel-app`, PHP 8.4). The result is given under "Oracle" as "checked today". This check is **not part of the test**.

---

#### `opengrid-framework:src/control.rs::task_names_follow_the_module_convention`
- **Run:** `cargo test -p opengrid-framework --lib -- task_names_follow_the_module_convention`; runtime 0.00 s; in the suite: yes.
- **Checks:** `task_class(sig)` maps a task signature `module:name` to the dialect class `Module\Tasks\Name` (StudlyCase over `-` and `_`): `notes:prune` → `Notes\Tasks\Prune`, `users:send-digest` → `Users\Tasks\SendDigest`, `billing:close_month` → `Billing\Tasks\CloseMonth`. Without a colon (`prune`) and with path characters (`a:../x`) the result is `None`.
- **Kind:** unit; pure function → `assert_eq!` against literals.
- **Oracle:** `own expectation`: convention instead of registration (DECISIONS D1, doc comment on `task_class`). The StudlyCase parts agree with Laravel's `Str::studly` (checked today: `SendDigest`, `CloseMonth`), but the test does not compare against it.
- **Notes:** —

#### `opengrid-framework:src/http.rs::query_parsing`
- **Run:** `cargo test -p opengrid-framework --lib -- query_parsing`; runtime 0.00 s; in the suite: yes.
- **Checks:** `parse_query` splits `a=1&b=x%20y+z&c` into pairs (percent decoding, `+` → space, key without `=` → empty value). `percent_decode("100%")` leaves an incomplete `%` in place.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation`, agrees with PHP `parse_str`/`urldecode` (checked today: `{"a":"1","b":"x y z","c":""}`). The test contains no reference to PHP.
- **Notes:** —

#### `opengrid-framework:src/http.rs::client_ip_from_the_proxy_header`
- **Run:** `cargo test -p opengrid-framework --lib -- client_ip_from_the_proxy_header`; runtime 0.00 s; in the suite: yes.
- **Checks:** `HttpRequest::client_ip(header)` (IP1): with a proxy header set, the **last** entry of `X-Forwarded-For` applies (Traefik appends). IPv6 is accepted, a `:port` on IPv4 is cut off. If the header is missing, unusable (`evil, <script>`) or proxy headers are off (`None`), `remote_addr` applies.
- **Kind:** unit; the test builds an `HttpRequest` with `remote_addr` and header and compares with literals.
- **Oracle:** `own expectation`: doc comment "IP1" with the Traefik behavior as justification, no source in the test.
- **Notes:** —

#### `opengrid-framework:src/routes.rs::matches_params_and_any`
- **Run:** `cargo test -p opengrid-framework --lib -- matches_params_and_any`; runtime 0.00 s; in the suite: yes.
- **Checks:** `RouteTable::add`/`find`: methods are case-insensitive, `/tasks/` matches `/tasks` (trailing slash), `{id}` is returned as parameter `("id","42")`, `any` matches POST, an unknown method on a known path returns `None`, and a duplicate route is an error.
- **Kind:** unit; build table → `find` → compare handler name and parameters.
- **Oracle:** `own expectation` in Laravel style, without a source reference.
- **Notes:** —

#### `opengrid-framework:src/routes.rs::groups_prefix_and_middleware`
- **Run:** `cargo test -p opengrid-framework --lib -- groups_prefix_and_middleware`; runtime 0.00 s; in the suite: yes.
- **Checks:** middleware aliases are resolved and parameters are preserved (`throttle:5,1` → `App\Throttle:5,1`; unknown ones stay unchanged). Nested group prefixes (`/admin/` + `tasks` + `/`) are normalized to `/admin/tasks`. The group middleware comes before the route's own middleware and stays with the route after the groups are closed.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation` (Laravel route groups as model, not compared).
- **Notes:** —

#### `opengrid-framework:src/services.rs::paths_stay_inside_the_disk`
- **Run:** `cargo test -p opengrid-framework --lib -- paths_stay_inside_the_disk`; runtime 0.00 s; in the suite: yes.
- **Checks:** `check_path` (storage disk) allows relative paths including `.hidden` and rejects: empty, absolute, `..` at the beginning, in the middle or at the end, backslash and NUL.
- **Kind:** unit; two lists, `is_ok`/`is_err` each.
- **Oracle:** `own expectation`: security rule from the doc comment.
- **Notes:** —

#### `opengrid-framework:src/native/rate_limiter.rs::window_counts_and_expires`
- **Run:** `cargo test -p opengrid-framework --lib -- window_counts_and_expires`; runtime 0.04 s; in the suite: yes.
- **Checks:** `RateLimits`: `increment` counts within the window (1, then +2 = 3), `attempts` reads the counter, `available_in` is 59–60 s, `clear` resets both to 0. A 20 ms window has expired after 40 ms, and a new window starts again at 1.
- **Kind:** unit with real time (`thread::sleep(40 ms)`).
- **Oracle:** `own expectation`, modeled on Laravel's `RateLimiter` (hit/attempts/availableIn/clear), not compared.
- **Notes:** time-dependent. Because `sleep` is a minimum duration, flakiness practically does not occur; the interval `59..=60` catches second boundaries.

#### `opengrid-framework:src/native/request.rs::nested_query_keys`
- **Run:** `cargo test -p opengrid-framework --lib -- nested_query_keys`; runtime 0.00 s; in the suite: yes.
- **Checks:** `nested_array` builds PHP-like nested arrays from query pairs: `filters[done]=1` → `filters.done`, `tags[]=a&tags[]=b` → list with 2 entries, `sort` stays flat.
- **Kind:** unit; checks only **structure and type** (`Val::Str`, `len() == 2`), no values.
- **Oracle:** `own expectation`. PHP `parse_str` yields the same structure (checked today), but is not consulted.
- **Notes:** —

#### `opengrid-framework:src/native/validation.rs::missing_field_reports_only_required`
- **Run:** `cargo test -p opengrid-framework --lib -- missing_field_reports_only_required`; runtime 0.00 s; in the suite: yes.
- **Checks:** (1) a missing required field reports only "The title field is required.", and an optional field consisting of spaces (`note: "  "`) is skipped by non-required rules. (2) A **present `null`** with `required|string|max:3` yields only "required" (since TEST2.2a; before, two messages).
- **Kind:** unit; `validate(data, rules)` → compare error list with literals.
- **Oracle:** `own expectation` with Laravel as the declared model (comment "Laravel: non-implicit rules skip…"). Checked today against Laravel's validator (`validation_spec/driver.php`): both parts agree since TEST2.2a (Laravel's `shouldStopValidating`: after a failed implicit rule the check of the field ends); `validation_spec/cases.txt` has four cases for this (Laravel output in `cases.out`).
- **Notes:** see findings.

#### `opengrid-framework:src/native/validation.rs::message_summarizes_the_other_errors`
- **Run:** `cargo test -p opengrid-framework --lib -- message_summarizes_the_other_errors`; runtime 0.00 s; in the suite: yes.
- **Checks:** `summarize` (text of the `ValidationException`): one error → only the message; two → "… (and 1 more error)"; three over two fields → "… (and 2 more errors)"; none → "The given data was invalid.".
- **Kind:** unit; `validate` + `summarize` against literals.
- **Oracle:** `independent` (Laravel 13.38, `ValidationException::summarize`; the two multi-error cases printed on 2026-10-26 with the validator of the LV1 app), frozen as literals.
- **Notes:** TEST2.2b.

#### `opengrid-framework:src/native/validation.rs::rules`
- **Run:** `cargo test -p opengrid-framework --lib -- rules`; runtime 0.00 s; in the suite: yes.
- **Checks:** a rule set with `required|string|max`, `integer|between`, `nullable|email`, `nullable|date` (field missing) and `confirmed`. Errors exist for `title` (empty), `priority` (9 outside 1–5) and `pw` (confirmation differs), in this order; the messages for title and priority are checked. With valid data `validate` returns only the ruled, present fields (`title`, `priority`; `extra` is dropped). `"3"` as a string counts as integer.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation` (DECISIONS "Validation", messages in Laravel's wording). Checked today against Laravel: field order, messages and `validated()` = `{"title":"Ok","priority":"3"}` agree.
- **Notes:** the message for `pw` (confirmed) is not checked, only that the field has an error.

#### `opengrid-framework:src/native/validation.rs::size_messages_name_the_unit_like_laravel`
- **Run:** `cargo test -p opengrid-framework --lib -- size_messages_name_the_unit_like_laravel`; runtime 0.00 s; in the suite: yes.
- **Checks:** size rules choose the message variant by type: string (`min:8` → "at least 8 characters", `size:4` → "must be 4 characters"), numeric (`max:5` → "must not be greater than 5"), array (`min:2` → "at least 2 items", `between:2,3` → "between 2 and 3 items").
- **Kind:** unit; per field the first message against a literal.
- **Oracle:** `own expectation`, copied from the wording of Laravel's `lang/en/validation.php`. Checked today against Laravel's validator: all five messages identical.
- **Notes:** —

#### `opengrid-framework:src/native/database/builder.rs::collate_only_where_the_order_depends_on_it`
- **Run:** `cargo test -p opengrid-framework --lib -- collate_only_where_the_order_depends_on_it`; runtime < 0.01 s; in the suite: yes.
- **Checks:** DBP1 M4: `needs_collate` is false only for types whose parts are all non-string scalars (`int`, `none | int`, `option<float>`, `bool`, `datetime`, `decimal`, `record<…>` also with `|` inside the angle brackets); `string`, unions with `string`, `any`, arrays, objects need `COLLATE`. `try_sort_with` sets `COLLATE` only where the predicate demands it.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation`; that `COLLATE` on int does not change the order and SurrealDB uses the index without `COLLATE` was measured on 2026-10-26 against SurrealDB 3.3.4 (same result set, `SortTopKByKey` against `IndexScan`), but is not part of the test.
- **Notes:** —

#### `opengrid-framework:src/native/database/builder.rs::count_queries_inline_only_safe_literals`
- **Checks:** DBP1 M5: `inline_literals` inserts into count queries only bool, int and record ids with a simple table and key name (`users:⟨u1x⟩`) as literals; a string with quotes and a record id with a space in the key stay parameters; `$f1` is not replaced inside `$f10`.
- **Kind:** unit; literal comparison of SQL text and remaining parameters.
- **Oracle:** `own expectation` (security rule: nothing from a request is inserted as text). That SurrealDB reads `users:⟨…⟩` correctly is shown by the integration tests with pagination (`database.rs`, `api_script`).
- **Notes:** —

#### `opengrid-framework:src/native/database/builder.rs::sort_specs_are_checked`
- **Run:** `cargo test -p opengrid-framework --lib -- sort_specs_are_checked`; runtime 0.00 s; in the suite: yes.
- **Checks:** `try_sort`/`try_order_by` let through from `?sort=` only identifier paths and `asc`/`desc`. Nine attack and malformed forms (`;DELETE`, `title:up`, spaces, brackets, leading digit, `a..b`, `a.-b`) are rejected without leaving residues in `orders`. A valid list with three entries (mixed case, dot path, `_id`) is accepted.
- **Kind:** unit; checks error or success and the length of `orders`.
- **Oracle:** `own expectation`: security specification (comment in the test).
- **Notes:** for the valid list only the count (3) is checked, not direction or field names.

#### `opengrid-framework:src/native/database/builder.rs::renders_like_the_php_builder`
- **Run:** `cargo test -p opengrid-framework --lib -- renders_like_the_php_builder`; runtime 0.00 s; in the suite: yes.
- **Checks:** the SurrealQL of the query builder, byte for byte: soft-delete clause first, bindings `$f0…`, `ORDER BY … COLLATE`, `LIMIT/START` before `FETCH`, `count_sql` with `GROUP ALL`, `whereLike` as `string::contains(string::lowercase(<string>…))` with a lowercased binding, `INSIDE`, `UPDATE … MERGE $data`, `DELETE … RETURN BEFORE`, `onlyTrashed`, dot-path sorting via a projected alias `__sort_…`, `withCount`/`withSum`/`withAll` as correlated sub-selects with binding prefix `s1_`.
- **Kind:** unit / golden with literals; builder chains → `*_sql()` → `assert_eq!`.
- **Oracle:** `own expectation`, **despite the name**. DECISIONS (builder format, point 4): "Until `builder_cases.php` arrives: own expectation (unit test `renders_like_the_php_builder`)". The acceptance against the PHP builder of the reference implementation (ROADMAP B0, DATABASE_LAYER §2.1/§4) "waits for Jonas". The sub-select form is an own decision (DECISIONS "One query instead of N+1").
- **Notes:** —

#### `opengrid-framework:src/native/database/inflect.rs::singulars`
- **Run:** `cargo test -p opengrid-framework --lib -- singulars`; runtime 0.00 s; in the suite: yes.
- **Checks:** `singular` for regular forms and special cases (`entries` → `entry`, `boxes` → `box`, `addresses` → `address`, `people` → `person`, `status` stays, `hours` → `hour`) and `foreign_key("tasks")` = `task_id`.
- **Kind:** unit; table with literals.
- **Oracle:** `own expectation`. Laravel's `Str::singular` yields the same for all seven words (checked today); the tests of the support classes under `crates/vm/tests/support/` do not compare here, though.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema.rs::statements`
- **Run:** `cargo test -p opengrid-framework --lib -- statements`; runtime 0.00 s; in the suite: yes.
- **Checks:** `BlueprintData::statements(true)` generates `DEFINE TABLE … SCHEMALESS`, fields with `TYPE`, `DEFAULT false`, nullable as `option<record<clients>>`, a single index `tasks_client_id_index`, a unique index `tasks_title_unique` and `REMOVE FIELD IF EXISTS`. All without `IF NOT EXISTS`/`OVERWRITE` (DECISIONS "R4b2 and R4h", point 4).
- **Kind:** unit; list of statements against literals.
- **Oracle:** `own expectation`: index names after Laravel convention, syntax after SurrealQL. Whether SurrealDB accepts the statements is not checked by the test.
- **Notes:** blueprint is deferred (BP1). The code is still used by `Schema::sql`.

#### `opengrid-framework:src/native/database/surreal.rs::platform_variables_select_server_mode`
- **Run:** `cargo test -p opengrid-framework --lib -- platform_variables_select_server_mode`; runtime 0.00 s; in the suite: yes.
- **Checks:** `DbConfig::from_sources` with `SURREAL_DB_HOST` switches to server mode `ws://host:8000`, ns/db `main`, user and password `root`. All six `SURREAL_DB_*` are adopted.
- **Kind:** unit; environment injected as a closure, no real process env is read.
- **Oracle:** `own expectation` after DECISIONS 2026-10-23/24, point 6 ("the same environment variables as the private reference implementation … defaults localhost, 8000, main, main, root, root"). The reference defaults were transferred by hand; no comparison against that code.
- **Notes:** —

#### `opengrid-framework:src/native/database/surreal.rs::settings_and_override`
- **Run:** `cargo test -p opengrid-framework --lib -- settings_and_override`; runtime 0.00 s; in the suite: yes.
- **Checks:** `settings.opengrid.database` (url, namespace) is read, the database falls back to `opengrid`. `OPENGRID_DB_URL` takes precedence over settings and `SURREAL_DB_*`. Without any source the result is `None`.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation` (DECISIONS DB1, implementation "`OPENGRID_DB_URL` takes precedence").
- **Notes:** —

#### `opengrid-framework:src/native/database/surreal.rs::embedded_paths`
- **Run:** `cargo test -p opengrid-framework --lib -- embedded_paths`; runtime 0.00 s; in the suite: yes.
- **Checks:** the default is `surrealkv://<root>/.opengrid/db`. Relative `surrealkv://` paths are resolved against the project root, absolute ones stay unchanged, `mem://` stays `mem://`. `is_embedded` distinguishes embedded from `ws://`.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation` (guardrail D2/DB1).
- **Notes:** —

#### `opengrid-framework:src/native/database/surreal.rs::concurrent_updates_of_one_record_are_retried`
- **Run:** `cargo test -p opengrid-framework --lib -- concurrent_updates_of_one_record_are_retried`; runtime 0.17 s; in the suite: yes.
- **Checks:** `Db::query` retries retriable write conflicts (optimistic transactions) outside explicit transactions. 8 threads × 40 `UPDATE t:1 MERGE` on **one** record must let no error through to the caller.
- **Kind:** integration in the unit module: real embedded SurrealDB (`surrealkv://` in the temp directory, name with PID), threads, afterwards the number of errors is counted.
- **Oracle:** `own expectation` ("no errors"). Real conflicts are produced by the real engine, but the test checks neither that conflicts occurred at all nor the final value of `n`.
- **Notes:** writes to disk (temp directory) and starts a Tokio runtime. Under high load flakiness is conceivable if a statement suffers more than `MAX_CONFLICT_RETRIES` = 20 conflicts in a row (backoff 200 µs × attempt). **TEST2.5:** additionally: the new counter `Db::conflict_retries()` must be > 0 (conflicts occurred and were retried; 8 of 8 repetitions green), and the record holds the last write value of one of the threads (`w*100+39`).

#### `opengrid-framework:src/native/database/types.rs::durations_round_trip`
- **Run:** `cargo test -p opengrid-framework --lib -- durations_round_trip`; runtime 0.00 s; in the suite: yes.
- **Checks:** `parse_duration`/`format_duration` as a round trip for `1h30m`, `2d`, `45s`, `1w` (seconds and text). `1500 ms` is formatted as `1s500ms`. `abc` and a number without unit are rejected.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation` after the SurrealQL duration syntax (doc comment). An independent oracle would be SurrealDB's own formatting (`surrealdb::types::Duration`/`<string>`), but it is not used.
- **Notes:** —

#### `opengrid-framework:src/native/database/types.rs::base64`
- **Run:** `cargo test -p opengrid-framework --lib -- base64`; runtime 0.00 s; in the suite: yes.
- **Checks:** `base64_encode` for `hi`, `hello` (with padding) and empty input.
- **Kind:** unit; three literals.
- **Oracle:** `own expectation`, standard values after RFC 4648; identical with PHP `base64_encode` (checked today).
- **Notes:** —

#### `opengrid-framework:src/native/database/value.rs::base_types`
- **Run:** `cargo test -p opengrid-framework --lib -- base_types`; runtime 0.00 s; in the suite: yes.
- **Checks:** `base_type` removes the optional wrapper of a field type: `option<int>`, `none | int`, `int | none`, `null | datetime` → core type. A record union is preserved (`record<tasks | users>`), a real union `int | string` stays unchanged.
- **Kind:** unit; literal comparison.
- **Oracle:** `own expectation`. The input forms are hand-written. That `INFO FOR` of a real server produces these forms is, according to DECISIONS DB1 ("test of the type parser `none | int` against real server output"), to be checked only by the server track.
- **Notes:** —

#### Schema port (`native/database/schema/*`, R4b2/R4h)

On the origin: the port mimics the PHP classes of a private reference implementation (`SurqlFileParser`, `SurqlSchema`, `SurqlGenerator`, `SurqlDbParser`, `MigratePush::collectSourceFiles`). The unit tests below have **hand-written literals**. The basis is the behavior of the PHP classes as the author read it during the port or saw it through the golden tests, and for the R4h cases the deliberate corrections (DECISIONS "R4b2 and R4h", 2026-10-24). The independent safeguard against PHP 8.4 with the real reference classes is provided only by `tests/suite/schema_golden.rs`. I sent the inputs of the unit tests individually through `schema_golden/driver.php` today; the result is given per test.

#### `opengrid-framework:src/native/database/schema/dbinfo.rs::info_answers_become_the_model`
- **Run:** `cargo test -p opengrid-framework --lib -- info_answers_become_the_model`; runtime 0.00 s; in the suite: yes.
- **Checks:** `from_info` converts answers of `INFO FOR DB`/`INFO FOR TABLE` into the schema model. The internal SurrealKit tables (`__entity`, `__seed`) are dropped, the field type `none | int` can be queried, and the generator prints the INFO texts unchanged (field, blank line, index).
- **Kind:** unit; INFO answers by hand → model → `generate` → `contains` on a multi-line excerpt.
- **Oracle:** `own expectation`. The INFO texts are invented and not taken from a real SurrealDB answer; "texts from INFO FOR stay unchanged" comes from DECISIONS "R4b2 and R4h", point 2. A comparison with the PHP `SurqlDbParser` does not take place.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/generator.rs::layout`
- **Run:** `cargo test -p opengrid-framework --lib -- layout`; runtime 0.00 s; in the suite: yes.
- **Checks:** the complete layout of `temporary.surql`: table blocks sorted (clients before tasks), header of lines of 30 dashes, DEFINE TABLE, then fields (sorted: done before title) and indexes, the groups separated by one blank line, blocks by two blank lines, exactly one newline at the end.
- **Kind:** golden with literal; `merge_source` → `generate` → `assert_eq!` on the whole text.
- **Oracle:** `own expectation` (literal). Checked today against the reference classes: **byte-identical** with the PHP output.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/generator.rs::empty_schema_is_a_newline`
- **Run:** `cargo test -p opengrid-framework --lib -- empty_schema_is_a_newline`; runtime 0.00 s; in the suite: yes.
- **Checks:** an empty schema yields `"\n"`.
- **Kind:** unit; one literal.
- **Oracle:** `own expectation`, justified by the finding from R4b2 (DECISIONS "R4b2 and R4h", point 2: "empty schema yields `\n`"). Checked today: PHP also prints exactly `\n`. Golden case `13_empty` covers this independently.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/generator.rs::ksort_order`
- **Run:** `cargo test -p opengrid-framework --lib -- ksort_order`; runtime 0.00 s; in the suite: yes.
- **Checks:** the key order of `sorted` mimics PHP 8 `ksort` (SORT_REGULAR): integers numerically, numeric strings numerically among themselves, the rest bytewise. Expected: `-1, 01, 1.5, 9, 10, B, _x, a, b`.
- **Kind:** unit; literal list.
- **Oracle:** `own expectation` with PHP as the declared model (doc comment). Checked today with `php -r 'ksort(...)'`: identical order.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/generator.rs::non_transitive_keys_do_not_panic`
- **Run:** `cargo test -p opengrid-framework --lib -- non_transitive_keys_do_not_panic`; runtime 0.00 s; in the suite: yes.
- **Checks:** with non-transitive keys (`100`, `1x`, `20`) the sort does not abort. Rust's `sort_by` can panic on an inconsistent comparison.
- **Kind:** unit; checks only `len() == 3`.
- **Oracle:** `own expectation` (robustness). PHP yields `100, 1x, 20` here; the order is not compared.
- **Notes:** shows only that no panic occurs.

#### `opengrid-framework:src/native/database/schema/generator.rs::deterministic`
- **Run:** `cargo test -p opengrid-framework --lib -- deterministic`; runtime 0.00 s; in the suite: yes.
- **Checks:** the same source text with a different source label yields the same text, even if the fields are in reverse order and the table is defined last.
- **Kind:** unit; compares two outputs of the port with each other.
- **Oracle:** `own expectation` (self-comparison, no external oracle).
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/merge.rs::later_wins_per_key`
- **Run:** `cargo test -p opengrid-framework --lib -- later_wins_per_key`; runtime 0.00 s; in the suite: yes.
- **Checks:** with two files the later one wins: DEFINE TABLE (SCHEMAFULL replaces SCHEMALESS) and field `a` (text and `source` = `manually`). Field `b` from the first file is preserved.
- **Kind:** unit; model fields against literals.
- **Oracle:** `own expectation` after the ported `SurqlSchema::merge`. Checked today: PHP produces the same statements.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/merge.rs::duplicate_keys_in_one_file`
- **Run:** `cargo test -p opengrid-framework --lib -- duplicate_keys_in_one_file`; runtime 0.00 s; in the suite: yes.
- **Checks:** if a field is defined twice in the same file, one entry remains and the second definition wins.
- **Kind:** unit.
- **Oracle:** `own expectation`. Checked today: PHP behaves the same (additionally producing the placeholder DEFINE TABLE, which the test does not check).
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/merge.rs::placeholder_until_the_table_is_defined`
- **Run:** `cargo test -p opengrid-framework --lib -- placeholder_until_the_table_is_defined`; runtime 0.00 s; in the suite: yes.
- **Checks:** an index on an undefined table produces the placeholder `DEFINE TABLE comments TYPE NORMAL SCHEMAFULL PERMISSIONS NONE` (`placeholder = true`). A later real definition replaces it, and the index is preserved.
- **Kind:** unit.
- **Oracle:** `own expectation`. The placeholder text was adopted by R4b2 from the PHP output (DECISIONS "R4b2 and R4h", point 2). Checked today: PHP gives the same result.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/merge.rs::a_later_placeholder_keeps_the_definition`
- **Run:** `cargo test -p opengrid-framework --lib -- a_later_placeholder_keeps_the_definition`; runtime 0.00 s; in the suite: yes.
- **Checks:** R4h correction: a later file with only one field does **not** reset a SCHEMALESS table to the SCHEMAFULL placeholder.
- **Kind:** unit.
- **Oracle:** `own expectation`, **deliberately against the PHP original** (DECISIONS "R4b2 and R4h", point 3, first finding). Checked today: PHP prints the placeholder here, as the comment says.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/merge.rs::integer_like_names_merge_by_name`
- **Run:** `cargo test -p opengrid-framework --lib -- integer_like_names_merge_by_name`; runtime 0.00 s; in the suite: yes.
- **Checks:** R4h correction: field names like `1` are merged by name (2 fields, `1` from the later file), not renumbered and kept twice as with PHP `array_merge`.
- **Kind:** unit.
- **Oracle:** `own expectation`, deliberately against the original (DECISIONS "R4b2 and R4h", point 3). Checked today: PHP prints `DEFINE FIELD 1` twice.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/merge.rs::quoted_and_plain_names_are_the_same_key`
- **Run:** `cargo test -p opengrid-framework --lib -- quoted_and_plain_names_are_the_same_key`; runtime 0.00 s; in the suite: yes.
- **Checks:** `` `t` ``/`` `a` `` and `t`/`a` are the same key: one table, and field `a` comes from the later file.
- **Kind:** unit.
- **Oracle:** `own expectation`. Checked today: PHP also merges into one table and one field (but, as above, puts the placeholder in place of the SCHEMALESS definition; the test does not check that).
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/merge.rs::field_type_lookup`
- **Run:** `cargo test -p opengrid-framework --lib -- field_type_lookup`; runtime 0.00 s; in the suite: yes.
- **Checks:** `Schema::field_type` reads the TYPE expression of a field (`option<int>`) and returns `None` for a missing field.
- **Kind:** unit.
- **Oracle:** `own expectation`. The function exists only in Rust (filter types, DECISIONS B1 addendum); there is no PHP counterpart.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/parser.rs::splits_outside_strings_backticks_and_comments`
- **Run:** `cargo test -p opengrid-framework --lib -- splits_outside_strings_backticks_and_comments`; runtime 0.00 s; in the suite: yes.
- **Checks:** `split_statements` splits only at `;` outside `'…'`, `"…"`, backticks and `--` comments, trims, and discards empty statements (`;  ;`) as well as a header and trailing comment.
- **Kind:** unit; literal list.
- **Oracle:** `own expectation`. Checked today: the PHP classes produce the same three statements.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/parser.rs::escapes`
- **Run:** `cargo test -p opengrid-framework --lib -- escapes`; runtime 0.00 s; in the suite: yes.
- **Checks:** `''` in single quotes, `\'` in single quotes (R4h) and `\"` in double quotes do not end the string; the `;` inside therefore splits no statement.
- **Kind:** unit; three literals.
- **Oracle:** mixed. Cases 1 and 3: `own expectation`, checked today against PHP, identical. Case 2 (`\'`): deliberately against the original (R4h); PHP cuts off after `'it\'s` there (checked today).
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/parser.rs::all_comment_forms`
- **Run:** `cargo test -p opengrid-framework --lib -- all_comment_forms`; runtime 0.00 s; in the suite: yes.
- **Checks:** R4h: `//`, `#` and `/* … */` (also in the middle of a statement, there without whitespace replacement: `a/* x */ON` → `a ON`) are comments. Inside a string these characters are not a comment.
- **Kind:** unit; five literals.
- **Oracle:** `own expectation`, deliberately beyond the original (which knew only `--`; DECISIONS "R4b2 and R4h"). The comment syntax is modeled on the SurrealQL docs; there is no comparison with SurrealDB's parser.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/parser.rs::multiline_statements_keep_their_lines`
- **Run:** `cargo test -p opengrid-framework --lib -- multiline_statements_keep_their_lines`; runtime 0.00 s; in the suite: yes.
- **Checks:** a multi-line statement keeps line breaks and indentation. A `--` comment line inside is removed, its indentation remains (`"\n    \n    DEFAULT"`).
- **Kind:** unit; literal.
- **Oracle:** `own expectation`. Checked today: PHP produces byte for byte the same.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/parser.rs::keys_of_statements`
- **Run:** `cargo test -p opengrid-framework --lib -- keys_of_statements`; runtime 0.00 s; in the suite: yes.
- **Checks:** `parse_statement` determines kind, name and table: any case, backticks, `ON TABLE`. R4h cases: `IF NOT EXISTS` and `OVERWRITE` are modifiers (the text is preserved), a table may be called `IF`, names beyond ASCII (`address.city`, `tags.*`, `café`, `⟨my field⟩`/`⟨t-1⟩`). Plus INDEX and EVENT.
- **Kind:** unit; many tuples against literals.
- **Oracle:** mixed. Backtick and case variants agree with PHP (checked today). The R4h cases are deliberately different: PHP reads `IF` or `overwrite` as the name and shortens `café` to `caf` (checked today). For these cases the expectation is the author's own specification after DECISIONS "R4b2 and R4h", point 3.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/parser.rs::unsupported_statements_are_reported`
- **Run:** `cargo test -p opengrid-framework --lib -- unsupported_statements_are_reported`; runtime 0.00 s; in the suite: yes.
- **Checks:** `parse_file` keeps only the supported `DEFINE TABLE` and reports three warnings: `DEFINE ANALYZER`, `DEFINE FUNCTION` and the non-DEFINE `REMOVE TABLE`. Prefixes of the first and third message are checked.
- **Kind:** unit; count plus `starts_with`.
- **Oracle:** `own expectation`. The warnings exist only in Rust (the original discards silently; DECISIONS "R4b: …", point 6). That only the one statement remains corresponds to PHP.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/parser.rs::empty_input`
- **Run:** `cargo test -p opengrid-framework --lib -- empty_input`; runtime 0.00 s; in the suite: yes.
- **Checks:** empty input and a pure comment line yield no statements.
- **Kind:** unit.
- **Oracle:** `own expectation` (compatible with PHP; golden case `13_empty`).
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/parser.rs::field_types`
- **Run:** `cargo test -p opengrid-framework --lib -- field_types`; runtime 0.00 s; in the suite: yes.
- **Checks:** `clause_type` reads the TYPE expression up to the next keyword (`PERMISSIONS`, `DEFAULT`), with unions and nested `<…>` and also after `FLEXIBLE`. Without TYPE the result is `None`.
- **Kind:** unit.
- **Oracle:** `own expectation`. The function exists only in Rust; the inputs are hand-written and not taken from a real `INFO FOR` answer.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/preflight.rs::required_fields`
- **Run:** `cargo test -p opengrid-framework --lib -- required_fields`; runtime 0.00 s; in the suite: yes.
- **Checks:** `required(def)` recognizes a field that existing rows would violate: type without NONE and without DEFAULT/VALUE. Not "required" are `none | string`, `option<…>`, with DEFAULT, `any` and fields without TYPE.
- **Kind:** unit; six `assert!`.
- **Oracle:** `own expectation` after DECISIONS "rollouts only additive" (2026-10-24, Jonas), point 3. Whether SurrealDB actually rejects such a definition against existing rows is not checked by the test.
- **Notes:** `VALUE` is not checked, although the doc comment mentions it.

#### `opengrid-framework:src/native/database/schema/preflight.rs::manifest_step_rules`
- **Run:** `cargo test -p opengrid-framework --lib -- manifest_step_rules`; runtime 0.00 s; in the suite: yes.
- **Checks:** `manifest_rules` on a TOML manifest: `remove_entities` in phase `complete` and `run_sql` are forbidden, `apply_files` and `remove_entities` in phase `rollback` are allowed. Expected are exactly the violations `["drop_old", "backfill"]`.
- **Kind:** unit; manifest by hand → violation list.
- **Oracle:** `own expectation` (DECISIONS "rollouts only additive", point 1). The manifest format is written after SurrealKit and not generated by SurrealKit.
- **Notes:** —

#### `opengrid-framework:src/native/database/schema/sources.rs::merge_order`
- **Run:** `cargo test -p opengrid-framework --lib -- merge_order`; runtime 0.01 s; in the suite: yes.
- **Checks:** `collect` returns the schema sources in merge order: `{framework}` → `database/sources/main.surql` → per module sorted bytewise (`Alpha` < `billing` < `notes` < `zeta`) `blueprint.surql`, then `schema.surql` → afterwards all `manually.surql`. A module without schema files (`nodb`) is absent.
- **Kind:** unit with file system: tree in the temp directory (name with PID) → compare labels → clean up.
- **Oracle:** `own expectation` as a port of `MigratePush::collectSourceFiles`. The bytewise sorting is justified by PHP `glob` (doc comment). `schema.surql` (D10) and `{framework}` (R4e) are OpenGrid additions without a PHP counterpart. The `project` mode of `driver.php` replicates the reference implementation's paths (`vendor/…`), not `modules/`, and is not used here.
- **Notes:** writes to the temp directory.

#### `opengrid-framework:src/native/database/schema/sources.rs::missing_files_are_fine`
- **Run:** `cargo test -p opengrid-framework --lib -- missing_files_are_fine`; runtime 0.00 s; in the suite: yes.
- **Checks:** an empty project without a framework fragment yields an empty list and no error.
- **Kind:** unit with temp directory.
- **Oracle:** `own expectation`.
- **Notes:** writes to the temp directory.

---

## 3. Tools


Paths relative to `phprs/`, except where stated otherwise. "In the suite" means: `cargo test --workspace` runs it.

#### `tools/green.sh` (green rule, TEST2.1)
- **Purpose:** the whole suite the way the commit rule demands it: `cargo test --workspace --no-fail-fast` with `OPENGRID_REQUIRE_ORACLES=1` (oracle switch, see below), pinned with `taskset -c 0-7` (`GREEN_CORES`), time limit `GREEN_TIMEOUT` (3000 s).
- **Invocation:** `tools/green.sh`; log to `$GREEN_LOG` (default `../target/green.log`), a red run keeps a copy `green-red-HHMMSS.log`.
- **Output:** the result lines with tests, panics and `ORACLE MISSING`, closing `exit <rc>, passed <n>`; exit code of cargo.
- **In the suite:** is the suite; used by `commit.sh`, the hook and the CI.

#### `tools/commit.sh` (commit only when green)
- **Purpose:** `tools/commit.sh <msgfile> <paths…>`: stages the paths, runs `green.sh`, commits only on exit 0 and sets the line `Suite: N green` of the message to the number of passed tests. Exactly the staged tree is tested: other changes in the working tree are set aside by the script for the run (`git stash --keep-index --include-untracked`, then `pop`; do not change files during the run). Passes the checked tree to the hook (`OPENGRID_GREEN_TREE` = `git write-tree`), so that the suite does not run twice. On red everything stays staged, nothing is committed. (Until 09a7834 it tested the working tree, i.e. with unstaged changes not exactly the commit.)
- **In the suite:** no (tool).

#### `.githooks/pre-commit` (git hook)
- **Purpose:** no commit without a green suite, also for a direct `git commit`: if `OPENGRID_GREEN_TREE` does not match the staged tree, `green.sh` runs; red aborts the commit. Enable per clone: `git config core.hooksPath .githooks` (set in the workspace). Bypass only deliberately with `--no-verify`.
- **In the suite:** no.

#### `.github/workflows/suite.yml` (CI)
- **Purpose:** the whole suite on every push to `vm-core` and `main` and on PRs to `main`: Ubuntu 24.04, Rust stable, PHP 8.4 (setup-php, with Composer), `surreal` v3.3.4 (official install script), Laravel comparison app via `bench/framework/laravel/setup.sh` next to the repo (oracle for support and validation), then `tools/green.sh` with `OPENGRID_REQUIRE_ORACLES=1` and `OPENGRID_ORACLES_EXCEPT=platform` (the private reference implementation is never available there; that comparison stays local). A red run uploads the log as an artifact.
- **State:** created with TEST2.1; whether the first run is green cannot be checked locally (`gh` not logged in) and is stated in the final report.

#### `cargo mutants` (mutation tests, TEST2.8)
- **Purpose:** checks whether the tests notice bugs in the core at all: cargo-mutants (27.1.0, prebuilt via `cargo binstall`) changes one place per run (operator, comparison, return value, match arm) and runs the grd-vm tests; "missed" means no test failed.
- **Invocation:** `env -u CARGO_TARGET_DIR taskset -c 0-7 cargo mutants -p grd-vm -f crates/vm/src/vm/ops.rs -f crates/vm/src/value/convert.rs -f crates/vm/src/value/array.rs -f crates/vm/src/value/compare.rs -j 2 --timeout-multiplier 3 -o <dir>`. **`CARGO_TARGET_DIR` must be unset**, otherwise the copies of cargo-mutants all build into the shared `target/` and invalidate each other (and the main build).
- **Result 2026-10-26 (before TEST2.8):** 391 mutants in 18 min: 224 caught, **133 missed**, 26 unbuildable, 8 timeouts (catch rate 63 %). Cause of most misses in `vm/ops.rs`: the loop takes a fast path for int/float (`run.rs::num_fast`), the general paths (compound assignment on array elements/properties, `pow`, `array_sum`, comparisons of strings/DateTime, shifts at the limits) were reached by no grd-vm test; literals are moreover folded by the compiler. Thereupon probes `operators_slow_paths` (PHP output) and `comparisons_dialect` (DIALECT §5.1); two gaps found along the way (text of the operand type message, `var_export(PHP_INT_MIN)`). **Afterwards `ops.rs`:** 156 mutants, 113 caught, 27 missed (before 95), catch rate 81 %. The remaining 27 are almost all equivalent mutants (operator symbol and overflow value only go into the message, the int/int division arm equals the general one) as well as `native_arith`/`native_compare` for native number types, which exist only in the framework. `convert.rs` (among others `parse_numeric`, `format_float`, `to_bool`) and `array.rs` (`canonical_int`, iterator `size_hint`) keep their misses from the first run: open.
- **Result MUT2 (2026-10-26 late, D20; target per module ≥ 80 %):** new probes `value_core_edges` (PHP output: numeric strings, float truthiness, array casts, limits of float output, canonical keys, float `===`, nested append) and `dialect_numeric_edges` (grd output, DIALECT §5.1/§5.2: pow overflow, string arithmetic, DateTime `===`; since DIA2 also float in `%`/bit operations, `++`/`--` on null/strings, `abs(PHP_INT_MIN)`), `operators_slow_paths` extended (array operands in all operators, `/=` on array elements), unit test `array.rs::iterates_from_the_back_and_clears`. One gap found and fixed along the way: `(float)"-0"` gave `0.0` instead of `-0.0`. Catch rates (caught/missed): **ops 127/13 (90.7 %)**, **convert 139/11 (92.7 %)**, **array 68/8 (89.5 %)**, **compare 21/0 (100 %)**.
  Justification per surviving mutation (all equivalent or unreachable from grd-vm):
  - `ops.rs:40/44/48` (`+`/`-`/`*` in the float fallback value), `:245` (`-` in `unary`), `:326` (`±1.0` in `inc_dec`): the value only goes to `overflow(sym, _)`, which ignores it and throws `ArithmeticError` (D10) → equivalent.
  - `ops.rs:50` (int/int division arm deleted): falls into the general float `/` arm with the same division-by-zero check and the same result → equivalent (the arm is only the direct path).
  - `ops.rs:207` (`native_arith` → `Ok(None)`): native number types (Decimal) exist only in the framework; its tests cover the path, grd-vm tests do not reach it.
  - `convert.rs:51/59/60` (counters in `parse_numeric`): the result is re-checked at the end by `str::parse`, wrong counter values only change when that happens → equivalent.
  - `convert.rs:104` (`format_float`) and `:163` (`repr_float`), `f > 0.0` → `>=`: in the INF branches `f` is never `0.0` → equivalent.
  - `convert.rs:129` (`<` → `<=` in `format_float`): with an equal digit count both branches produce the same text → equivalent.
  - `convert.rs:168` (`f.abs() < 1e15` → `==`/`<=` in `repr_float`): for integral |f| < 1e16 both branches give the same text (PHP writes `1e15` as `1000000000000000`) → equivalent.
  - `array.rs:106` (`size_hint` ×6), `:138` (`with_capacity` → `Default`), `:153` (`+` → `*` in `to_hash`): affect only the pre-allocated capacity, never the content → equivalent.
- **In the suite:** no (tool; a run takes 10–20 min).

#### `cargo llvm-cov` (coverage, TEST2.8)
- **Purpose:** line and region coverage of the whole suite.
- **Invocation:** `taskset -c 0-7 cargo llvm-cov --workspace --no-fail-fast --summary-only` (own instrumented build under `target/llvm-cov-target`).
- **Result 2026-10-26 (after TEST2.7, without the Laravel `api.sh` run, which did not start under load):** lines **78.8 %**
  (25 392, 5 387 not executed), regions 74.4 %, functions 71.5 %. Per crate (lines): framework 81.0 %, vm 78.4 %,
  jit 70.2 %, pkg 93.5 %. Weakest (≥ 150 lines): `jit/src/bin/grd.rs` 0 % (CLI, no test calls it),
  `framework/src/native/database/schema.rs` 37.6 %, `vm/src/builtins/datetime.rs` 53.0 %, `vm/src/vm/native.rs` 55.7 %,
  `framework/src/native/response.rs` 58.0 %, `framework/src/native/session.rs` 59.6 %, `vm/src/builtins/reflect.rs` 62.8 %.
  Coverage only says what ran, not whether it was checked (for that, `cargo mutants`).
- **In the suite:** no.

#### Oracle switch `OPENGRID_REQUIRE_ORACLES` / `OPENGRID_ORACLES_EXCEPT` (TEST2.1)
- **Purpose:** tests with an independent oracle (php, Laravel vendor, `surreal`, a private reference implementation not part of this repository) report a missing oracle via `tests/suite/oracle.rs` (per crate). With `OPENGRID_REQUIRE_ORACLES=1` the test fails with `ORAKEL FEHLT (<kind>): …` ("oracle missing"); kinds in `OPENGRID_ORACLES_EXCEPT` (comma list: `php`, `laravel-vendor`, `surreal`, `platform`) are exempt. Without the switch the test stays green and writes clearly `*** SKIPPED (<kind>): … ***`.
- **Connected:** `grd-vm:tests/suite/types.rs` (php, 2×), `grd-vm:tests/suite/support.rs` (laravel-vendor), `opengrid-framework:tests/suite/schema_golden.rs` (php, platform), `opengrid-framework:tests/suite/server.rs` (surreal; covers `server_mode`, `transactions`, `types` with server).

#### `examples/test-app/tests/api.sh` (acceptance test TP5)
- **Purpose:** end-to-end test of the test app over HTTP (register, login, notes CRUD with filter/sorting/pagination, tenant separation of two users, logout, restart on the same embedded database, task over the control socket). With `--app laravel` the same flow runs against the Laravel replica (LV1); then the script is a real independent oracle for the Laravel fidelity of the responses (status codes, validation messages, JSON shape).
- **Invocation:** `examples/test-app/tests/api.sh` (binary `../../../target/debug/opengrid` or `OPENGRID_BIN=…`); `api.sh --app laravel [--runtime fpm|octane|frankenphp]` (needs `bench/laravel-app`, created with `setup.sh`, and `serve.sh`). With `--db pgsql` (LV2) the script creates its own database per run and deletes it at the end: in the PostgreSQL of `bench/framework/pg.sh` (started when needed; another server only via `PGHOST`); connection only via `PGHOST`/`PGPORT`/`PGUSER`/`PGPASSWORD`; needs `psql` and `php-pgsql`.
- **Input:** copy of the test app (without `.opengrid`, `tests`) to a temp directory; fresh DB (`OPENGRID_DB_URL=surrealkv://$WORK/db` or SQLite for Laravel); free port. Needs `curl`, `jq`, `python3`.
- **Output:** per step `PASS`/`FAIL`/`SKIP` (status + `jq` expression), closing `api.sh (<app>): all N steps passed`; exit 1 on failure, 2 on a missing tool. On failures the last 30 lines of the server log.
- **Results:** stdout only; the temp directory is deleted.
- **In the suite:** yes, but only the OpenGrid variant: `opengrid-framework:tests/suite/api_script.rs::test_app_api_script` (with the `opengrid` of this build run). `--app laravel` never runs automatically. The 5 restart/task steps are counted as SKIP for Laravel.

- **DBR1 (2026-10-27):** additional section "framework-core benchmark endpoints" (10 steps) for both apps with the same expectations: `/api/bench/sql/{health,login,notes,notes/{id}}` on a separate SQLite database (`bench/framework/sqlite/seed.py`, 4 notes), among others list sorted `[4, 2]`, `show` as exact object, `create` with id 5, 401/404/422 messages. Thus the responses of the raw-SQL endpoints (D23) are checked identically in OpenGrid and Laravel (Octane, FPM). OpenGrid 55 steps, Laravel 49 (+6 skipped).
- **THESIS1 (2026-10-27):** section "bench logic" (7 steps, both apps alike): `/api/bench/logic/hydrate?n=2` (ids `[1, 2]`, nested user as exact object), `hydrate?n=0` → empty list, `collect?n=5` (groups by priority as exact object: count, character sum, first title after `sortBy`), `validate` with two valid elements → `valid 2`, with `priority 9` → 422 and exactly the error key `items.0.priority`, `json?n=2` (element 1 as exact object, floating-point number 1.5), without token 401. OpenGrid 62 steps, Laravel 56 (+6 skipped); since 2026-10-28 64 and 58: "validate checks every rule" (one violated rule per element: title missing, title 121 characters, title not a string, priority not a number, priority 6, tags not an array, tag 21 characters; exactly these seven error keys) and "validate without items". The response can be saved with `VALIDATE_RULES_OUT=<file>`; OpenGrid and Laravel (FPM, RoadRunner, FrankenPHP) answer byte-identically. Byte identity of the large responses (n = 1000) is checked by `bm2.py` before every measurement (see there).
- **D24 (2026-10-27):** `--db pgsql` removed; Laravel runs only on SQLite anymore (WAL, `synchronous = FULL`, `busy_timeout` 5000).

#### `bench/complex/bench.py` (script benchmarks c1–c3, method D11)
- **Purpose:** PHP 8.4 against grd on three scripts (`c1_request`, `c2_runtime`, `c3_compute`), separated by comparison A (PHP without opcache against grd with `GRD_JIT=0`), B (opcache without JIT against grd without JIT), C (PHP tracing JIT against grd JIT); optional PGO binary (`--grd-pgo`).
- **Invocation:** `python3 bench/complex/bench.py [--grd BIN] [--grd-pgo BIN] [--runs 7] [--cores 8-11] [--compare A,B,C] [--bench c1_request,…]`; default binary `../target/profiling/grd`.
- **Input:** `c*.php` and the generated `c*.grd`; phase times from the scripts' own `hrtime` output (`<phase> <ms>`, `checksum <n>`). Additionally measures wall clock, process start (empty script) and, for A, the compile time (100 copies minus 100 empty files, `php -l` or `grd compile`). Runs interleaved, `taskset`, median and relative standard deviation. Aborts if Xdebug is loaded.
- **Output:** Markdown tables per script and comparison on stdout; exit 1 if the checksums of PHP and grd do not match (that is the only correctness check).
- **Results:** stdout; transferred by hand to `.agents/BENCHMARKS.md`.
- **In the suite:** no.

#### `bench/run_all.sh` (complete benchmark state of a commit)
- **Invocation:** `bench/run_all.sh [--skip-build] [--framework-only]` (`--framework-only`: only BM2 and database self-measurement, without c1–c3).
- **Purpose:** c1–c3 (comparisons A/B/C, with and without PGO, median of 7) and BM2 (all endpoints at 1/16/64 connections; OpenGrid embedded, with PGO, against SurrealDB server, each as default, `touch1` (same semantics as Sanctum) and `touch0` (stricter); Laravel FPM and Octane on PostgreSQL over the Unix socket, D21) in one run. `env.txt` states connection kind and durability settings of both databases.
- **Preconditions (otherwise abort):** governor `performance` on all cores, clean working tree, no other build/benchmark (`cargo`, `rustc`, `cargo-mutants`, `bench.py`, `bm2.py`, `oha`, servers). Builds the four binaries from HEAD one after another (`--skip-build` if they are already built from HEAD).
- **Output:** `../target/bench-tmp/<commit>-<UTC time>/` with `env.txt` (commit, start/end, governor before/after, volume, fill level, versions, connection kind, durability, binaries), `lang.md`, `bm2.md`, `bm2.json`, `bm2.err`, `dbbase.md`, `dbbase.json`, `thesis.md`, `thesis.json` (THESIS1: 5 core targets, 19 logic endpoints, 1/16 connections, 1000 notes; ≈ 45 min), `sqlbase.md`, `sqlbase.json`, `report.md`; databases under `TMPDIR=../target/bench-tmp/tmp` on the mounted volume.
- **In the suite:** no.

- **BENCH3 (2026-10-28):** stages per framework (`gates:<fw>`, `bm2:<fw>`, `thesis:<fw>`, `load:<fw>`, `curve:<fw>`, `memory:<fw>`, `coldstart:<fw>`, `hotreload:<fw>`) plus `lang`, `sqlite` (all app variants on the reference SQLite, otherwise abort), `warmcheck` (once per series, `warmup.txt`) and `report`; since the compact run (2026-10-28, evening) bm2 measures only the default variant per framework (all five operations, 1/16/64 connections), THESIS n = 0/10/100/1000 at 16 connections, load only `show`; `best-<fw>.txt` remains the mechanism. OpenGrid runs as a `perf` build with `target-cpu=native`, PGO and `OPENGRID_JIT=on`; the optimizations of the other sides are in DECISIONS (2026-10-28, evening) and in `bench/frameworks/DEVIATIONS.md`. `--resume <dir>` continues an aborted series (every stage finished with exit 0 is skipped), `--redo <pattern>` repeats stages (e.g. `bm2:rails`), `--frameworks a,b` restricts. Additional precondition: the chosen apps are built (`setup.sh`).
- **BENCH1 (2026-10-27):** additionally measures `load.py`, `memory.py`, `coldstart.py` (outputs `load.*`, `memory.*`, `coldstart.*`); quiet check `bench/quiet.py` over the whole run (`quiet.txt`, `quiet.log`, result in `env.txt`); `env.txt` states CPU, SMT and the topology of the core ranges; the precondition "no other benchmark" knows the new tools and `valgrind`.
- **2026-10-28 (Jonas): three runs in one invocation.** `--runs N` (default 3, `--smoke` 2): build once, then N runs one after another, each in `<result folder>/run-<i>/` with all logs, raw data, `env.txt`, gates, quiet check and `stages.txt`; then `series.py` (median, range) and `report.py` in the result folder, then the hook with the result folder (writes `BENCHMARKS.md`). With `--smoke` the hook does not run. `--plan` calculates with `--runs` (≈ 8 h for three runs).
- **Last step (2026-10-27):** runs `bench/local/after-run <result folder>` if present (local, in `.gitignore`); here the hook writes §0 of BENCHMARKS.md (`.agents/tools/benchmarks_s0.py`). A failure of the hook only warns, the measurement is complete by then.
- **BENCH2 trial run (2026-10-27):** gates with absolute paths, every gate counts; `stage` passes the exit code on and writes it to `stages.txt`; every stage runs on the load cores (`taskset -c 0-3`), the language benchmark with `--cores 8,10,12,14`.
- **D24 (2026-10-27):** without PostgreSQL (no `pg.sh start`, no PG lines in `env.txt`, SQLite version and durability instead) and without database self-measurement (`dbbase`, `sqlbase` removed); Laravel default targets `laravel-frankenphp`, `laravel-octane`, `laravel-fpm` on SQLite; gates SQLite only. BENCH2: prerequisite check, gates, `--smoke`, `--plan`, `stages.txt`.

#### `.agents/tools/benchmarks_s0.py` (BENCHMARKS.md §0 from a run, mechanical part)
- **2026-10-28, later (Jonas): summary in `BENCHMARKS.md`.** The runs lie individually in `run-<i>/` of the result folder; the script writes §0 of `BENCHMARKS.md` from the merged series (template for method and history: `BENCHMARKS.md`, otherwise the most recent `BENCHMARKS_<n>.md`), with a block **Disturbances** at the top (from `report.md`); the old §0 goes to the archive once. The following rule "one file per run" is thereby replaced; `BENCHMARKS_1.md` remains as the state of the FINAL run 70ba137.
- **2026-10-28 (Jonas): one file per run.** A run writes `BENCHMARKS_<n>.md` (next free number), a series `BENCHMARKS_median.md`; each file from preamble, new §0 and §1/§2 of the most recent existing file; a second invocation for the same run rewrites its file; no more archiving (the following description of archiving no longer applies).
- **Purpose:** replaces §0 of BENCHMARKS.md with a skeleton from a result folder of `run_all.sh`: header (commit, start/end), placeholder **Assessment** (by hand), conditions from `env.txt`, gates, quiet check (result, findings counted by kind), stage times with exit code, `report.md` and the complete tables per stage (headings indented), `env.txt` in full. The previous §0 is appended to `archiv/BENCHMARKS_bis_<date>.md`; a marker in the comment prevents double archiving on a second invocation for the same run.
- **Invocation:** automatically as the last step of `run_all.sh` via the local hook `phprs/bench/local/after-run` (in `.gitignore`, not in the repo; output `after-run.log` in the result folder); by hand `python3 .agents/tools/benchmarks_s0.py <result folder> [--dry-run]`; `BENCHMARKS_S0_AGENTS=<folder>` writes into a copy (for testing).
- **Checked 2026-10-27:** on the trial run 5543d85 against a copy, twice (idempotent, archived once, §1/§2 unchanged), also via the hook.
- **In the suite:** no.

#### `bench/series.py` (merge runs: median and range)
- **Purpose:** called by `run_all.sh` after the N runs: merges `<result folder>/run-<i>/` in the result folder. Per measurement point median of requests/s, p50, p99, "in flight"; error status and requests summed; range (min–max) of requests/s in `bm2.range.json`, `thesis.range.json`, `load.range.json`; `load.json`/`memory.json` median per number, `coldstart.json` with all start times of the runs together; `env.txt` of the first run plus `series N runs: run-1 …`, `stages.txt`/`gates.txt`/`quiet.txt` and `lang.md` per run.
- **Invocation:** `python3 bench/series.py <result folder> <run>…` (otherwise only via `run_all.sh`).
- **Checked 2026-10-28:** with two existing runs and in the trial run `run_all.sh --smoke` (2 runs).
- **In the suite:** no.
- **Removed 2026-10-28:** `bench/run_series.sh` (Jonas: `run_all.sh` does the runs itself).

#### `bench/plan.py` (plan and duration of `run_all.sh`, BENCH2)
- **Purpose:** lists the stages of `run_all.sh` with content and estimated duration (measurement time per point exactly from warm-up + duration; setup per target and builds from earlier runs). Must agree with `run_all.sh`; the real times are in `stages.txt` of every run.
- **Invocation:** `bench/run_all.sh --plan [--smoke] [--framework-only]` or `python3 bench/plan.py …`.
- **In the suite:** no.

#### `bench/quiet.py` (quiet check during a measurement, BENCH1)
- **Purpose:** shows that nothing else ran during the measurement: samples every `--interval` s (2) and logs every foreign process (everything except the benchmark's programs: oha, opengrid, grd, php, php-fpm, nginx, rr, frankenphp, postgres, surreal, measurement scripts, PGO tools) above `--threshold` % (5) of a core and every load above the threshold on the idle cores (default: SMT siblings of the app cores, 9, 11, 13, 15).
- **Invocation:** `python3 bench/quiet.py --log FILE [--interval 2] [--threshold 5] [--idle-cores 9,11,13,15]` in the background, ends with SIGTERM; prints `quiet: ok …` or `quiet: N findings (see …)`. Started by `run_all.sh`.
- **2026-10-28:** a finding "idle core" names the threads that ran on the core in the sample (process/thread, share of a core), otherwise "no thread found (interrupts or short-lived processes)": kernel threads and host processes are not visible in the container. Occasion: 111 findings (97 × core 11) in the FINAL run 70ba137 without Claude Code; averaged ≈ 0.16 % of a core.
- **Checked 2026-10-27:** load on core 9 is reported (100 %); Claude Code itself also appears as a foreign process (≈ 9 % of a core), which is intended.
- **In the suite:** no.

#### `bench/framework/load.py` (concurrency and open load, BENCH1)
- **Purpose:** targets like `bm2.py` (default: OpenGrid PGO core and PGO default, Laravel FrankenPHP/RoadRunner/FPM as core and on PostgreSQL) with closed load at 16/256/1024 connections and open load (fixed rate 25/50/75/90 % of the rate at 256 connections, `oha -q --latency-correction`, 256 connections, warm-up per level), knee, spike (25 % base load + 65 % for 4 s); p50/p99/p99.9, failed requests (non-2xx and connection errors), utilization of the load cores (marked above 90 %). Method BENCHMARKS §1.2.
- **Invocation:** `python3 bench/framework/load.py [--targets …] [--endpoints show,list] [--conc 16,256,1024] [--levels 25,50,75,90] [--duration 10] [--warmup 3] [--json FILE]`; ≈ 2 min per target and endpoint.
- **Finding during construction:** without `-w` oha counts the requests aborted at the end as errors (one per connection); without warm-up per level the connection setup dominates p99.9 at 25 %. Both fixed before any measurement.
- **In the suite:** no.
- **D24 (2026-10-27):** Laravel default targets on SQLite instead of PostgreSQL.
- **D25 (2026-10-28):** core targets only (OpenGrid PGO, FrankenPHP, RoadRunner, FPM).

#### `bench/framework/memory.py` (memory as PSS, BENCH1)
- **Purpose:** PSS of all processes of a stack (`/proc/<pid>/smaps_rollup`), app alone and app + database, at rest, under load (peak, mean) and after 10 000 requests. Process tree via the parent PIDs from `/proc/*/stat`; PostgreSQL via `postmaster.pid`; SurrealDB server as its own process; embedded databases count within the app process.
- **Invocation:** `python3 bench/framework/memory.py [--targets …] [--endpoint show] [--conc 16] [--duration 10] [--requests 10000] [--json FILE]`.
- **In the suite:** no.
- **D24 (2026-10-27):** without PostgreSQL; "app + database" differs from "app" only for the SurrealDB server anymore.
- **D25 (2026-10-28):** core targets only; "app + database" = "app" (SQLite in-process, for OpenGrid additionally the embedded SurrealDB of the job poller).

#### `bench/framework/coldstart.py` (cold start and bootstrap, BENCH1)
- **Purpose:** start until the first 200 of the empty route with 4/16 workers (Laravel with prebuilt caches and without), first request against warm, hot reload per stack (OpenGrid file watcher, FPM opcache check, Octane `octane:reload`). Restarts the app server the way `bm2.py` started it (the invocation is recorded on the first start); the data stays. For the hot reload it changes the health controller (in the copy of the test app or in the Laravel app) and restores it afterwards.
- **Invocation:** `python3 bench/framework/coldstart.py [--targets …] [--runs 5] [--workers 4,16] [--endpoint show] [--warm 50] [--json FILE]`.
- **Note:** RoadRunner needs ≈ 150 ms for the first request of every PHP worker; with fewer warm requests than workers every request hits a cold worker (default 50 at 16 workers).
- **In the suite:** no.

#### BENCH3: comparison apps and their tools (2026-10-28)
- **Purpose:** OpenGrid and Laravel (Octane/FrankenPHP) against AdonisJS (on Bun), Goravel, Rails, FastAPI, Spring Boot, ASP.NET Core and Phoenix (since 2026-10-28 afternoon; NestJS, Next.js, Django dropped), all with raw SQL (no ORM) on SQLite 3.53.4 with Bun's compile options, the same endpoints, responses and Laravel error shape. Apps under `bench/<fw>-app/` (lockfiles in the repo, dependencies and builds not; the pre-commit hook rejects `node_modules`, `.venv`, `_build`, `deps`, `dist`, `obj`, `build/`).
- **Oracle:** `api.sh --app <fw>` (with `API_VARIANT=<variant>`): the bench sections (22 steps, 10 skipped) with the same exact expectations as for OpenGrid/Laravel; the response of "validate checks every rule" is byte-identical with OpenGrid and Laravel (`VALIDATE_RULES_OUT`). `bm2.py` compares the hashes of the logic responses before every measurement (canonical JSON, integral floating-point numbers as integers, because JavaScript does not distinguish them).
- **Tools:**
  - `bench/frameworks/install_runtimes.sh` — runtimes with checksums to `~/.local/opt` (Bun 1.4.3, Ruby 4.0.9 with YJIT, Python 3.15.2 with PGO/LTO, Temurin 25.0.5, .NET 10.0.403, OTP 29.1.2, Elixir 1.20.4, Go 1.27.2 with air 1.67.4; `php`/`frankenphp`: PHP 8.5.11 ZTS with embed and FrankenPHP 1.13.0 from source against the benchmark SQLite, Laravel's `setup.sh` links it).
  - `bench/frameworks/setup.sh [app …]` — builds the apps reproducibly from their lockfiles (dependencies, SQLite parts, production build; Rails with all groups and Phoenix additionally `MIX_ENV=dev` for the hot reload). Needs network; never during a measurement.
  - `bench/frameworks/serve.sh <app> <port> <dir> [workers] [variant]` — starts an app in production mode (variants: rails `threads3`/`threads1`, spring `platform`/`vthreads`; fastapi only `uvicorn` with uvloop/httptools), variant `dev` in the framework's development mode (Adonis `bun --watch`, Goravel `air`).
  - `bench/frameworks/check_sqlite.py <app>[:<variant>] …` — starts the app, reads `/api/bench/sql/sqlite`, compares version and `pragma compile_options` with the reference (read via PHP's PDO); exit 1 on deviation. `bm2.py` does the same check for every target before measuring (also OpenGrid and Laravel, whose bench modules have the endpoint since 2026-10-28) and aborts.
  - `bench/frameworks/hotreload.py [--apps …] [--runs 3]` — hot reload in the development mode of every framework (marked as development mode): change the response of the empty route in the source file, time until the new response (20 ms polling), file back and wait for the old response. Trial 2026-10-28: Rails 12 ms, Phoenix 93, Adonis (Bun) 237, FastAPI 416, .NET 1156, Spring 2225, Goravel 3887 ms.
  - `bench/frameworks/DEVIATIONS.md` — deviation table and rule table for validate; `report.py` takes over both.
  - `bench/framework/targets.py` — frameworks, targets, variant labels, Spring as JIT reference; `of <fw>`, `best <bm2.json> <fw> <stage>` (fastest variant by the geometric mean of all bm2 points; OpenGrid/Laravel keep the target sets of 70ba137).
  - `bench/framework/warmcheck.py` — warm-up check: per target and endpoint (`list`, `hydrate:100`) fresh server, warm-up, then 3 × 10 s at 16 connections; sufficient at ≤ 3 % difference, otherwise next level (3, 10, 30, 60 s). The longest necessary warm-up applies to all sides (`warmup.txt`). Trial 2026-10-28: Spring still 27 % difference after 3 s.
  - `bench/framework/warmcurve.py [--targets …] [--endpoints list,hydrate:100] [--duration 60] [--conc 16]` — warm-up curve (Jonas 2026-10-28; run_all: only `hydrate:100`, 30 s): per target and endpoint a freshly started server, constant load without warm-up; oha writes every request as CSV (start, duration), counted per second after the end: requests/s, p50, p99; derived steady rate (median of the last 10 s), first second as a share of it, "warm from" (first second from which the sliding 3 s mean no longer falls below 90 % or 97 %). `report.py` (l) shows the shares and writes one SVG diagram `warmcurve-<endpoint>.svg` per endpoint; `series.py` takes the median of the runs per second. Trial 2026-10-28 (hydrate:100, 20 s): OpenGrid 98 % in the 1st s, Adonis/Bun 61 %, .NET 37 % (jump after 4 s), Spring 33 % (rises until ≈ 10 s), Goravel 99 %.
  - `bench/framework/merge_parts.py <run> <stage> …` — merges the parts per framework (`parts/<stage>-<fw>.json/.md`) into `<stage>.json/.md`.
- **Runtime:** api.sh per app 15–60 s; check_sqlite per variant 3–10 s; hotreload all 9 apps ≈ 6 min; warmcheck see `plan.py`.
- **In the suite:** no (measurement tools); the apps are checked in `run_all.sh` by the stages `gates:<fw>` and `sqlite`.

#### `bench/report.py` (tables from the raw data)
- **Purpose:** generates the tables of a measurement result from the raw data, so that BENCHMARKS.md (and later the README) never deviate from the measurement: framework in three labeled versions (default; same semantics with `touch1`; stricter with `touch0`), per endpoint requests/s and p99 at 16 connections, ratio embedded/Octane, error count; plus the database share (total time p50 at 1 connection, database time from `dbbase.py`, difference, share).
- **PERF4 b (2026-10-27):** PGO is the release default: OpenGrid's PGO column comes first and carries the ratio to the best Octane; "without PGO" remains its own column (`--short` omits it); without a PGO measurement the report falls back to the binary without PGO and labels it so.
- **THESIS1 (2026-10-27):** with `thesis.json` (bm2.py with logic endpoints) additionally (e) per workload a table n = 1/10/100/1000 × targets (requests/s, p99 at 16 connections), ratio OpenGrid without PGO to the faster Octane and the **crossover point** (linear interpolation of the ratio over log10 n between the neighboring points at which it falls below 1; "none up to n = 1000" otherwise); with `sqlbase.json` (f) the time breakdown per point at 1 connection: total p50, database (self-measurement of the matching driver), base cost (endpoint with n = 0 minus its database time; `validate` uses `json:0`), logic and output (rest).
- **Invocation:** `python3 bench/report.py <result folder> [--conc 16] [--short]` (reads `bm2.json`, if present `thesis.json`, `load.json`, `memory.json`, `coldstart.json`); `run_all.sh` writes `report.md`.
- **In the suite:** no.
- **2026-10-28: disturbances.** Block "Disturbances" under the header: error status per side with target, endpoint and connection count (OpenGrid: accepted by Jonas as long as Laravel has none; every Laravel error is reported as a warning), per run the quiet check with findings by kind, points with a saturated load generator, warnings of the measurement tools, failed gates. "Median of N runs" only from two runs on; ranges in square brackets.
- **D24 (2026-10-27):** (b) with Laravel on SQLite; section (c) database share and the PostgreSQL 17 counter-measurement removed; (f) time breakdown without database column (base cost at n = 0 and the part growing with n). BENCH2: (g)–(j) from `load.json`, `memory.json`, `coldstart.json`.
- **D25 (2026-10-28):** without (b); disturbances name the embedded SurrealDB of the OpenGrid targets opened at start.

#### `bench/framework/sqlite/` (`schema.sql`, `seed.py`, `build_sqlite.sh`; core benchmark D23)
- **Purpose:** basis of the framework core comparison with raw SQL: `schema.sql` (users, tokens, notes, same indexes), `seed.py <file> [notes]` (fresh database, 1 user with bcrypt cost 10 from PHP's `password_hash`, notes like `bm2.py`), `build_sqlite.sh` (since 2026-10-28: SQLite 3.53.4 from Bun's amalgamation of bun-v1.4.3 with Bun's defines, header from the official amalgamation, both checksums bound, to `~/.local/opt/sqlite-3.53.4` with `lib/pkgconfig/sqlite3.pc`; all 50 compile options equal to bun:sqlite; OpenGrid, Python and .NET load it via `LD_LIBRARY_PATH`, the other sides are linked against it).
- **Invocation:** called by `bm2.py` (targets `-core`) and `api.sh` (4 notes); standalone `python3 bench/framework/sqlite/seed.py /tmp/b.sqlite`.
- **In the suite:** indirectly (`api.sh` in `api_script.rs`).

#### `bench/framework/pg.sh` — **removed** (D24, 2026-10-27)
- Was: PostgreSQL in the work container (data in `.pgdata`, Unix socket for Laravel, D21). With D24 PostgreSQL, the `-pgsql` targets and `.pgdata` are removed; Laravel runs on SQLite.

#### `bench/complex/gen.sh`
- **Purpose:** generates from every `c*.php` the dialect version `c*.grd`: `<?php` → `<?grd`, and a code line with `// grd: CODE` is replaced by `CODE` (e.g. sort functions that return the array in the dialect). Header line "GENERATED … edit the .php file".
- **Invocation:** `bench/complex/gen.sh` (without arguments).
- **Input/output:** `bench/complex/c*.php` → `bench/complex/c*.grd` (overwrites, versioned).
- **In the suite:** no; that the `.grd` files are current is checked by nobody except the checksum in `bench.py`.

#### `bench/framework/bm2.py` (framework benchmark BM2)
- **Purpose:** HTTP load against OpenGrid (`opengrid-server` with SurrealDB server, `opengrid-embedded` with surrealkv, optionally `opengrid-embedded-pgo`) and Laravel (`laravel-fpm`, `laravel-octane`).
- **Invocation:** `python3 bench/framework/bm2.py [--targets …] [--endpoints health,list,show,create,login] [--workers 4] [--conc 1,16,64] [--duration 10] [--warmup 3] [--notes 200] [--app-cores 8-11] [--db-cores 4-7] [--load-cores 0-3] [--opengrid BIN] [--opengrid-pgo BIN] [--json FILE]`; default `../target/profiling/opengrid`. `opengrid-embedded-pgo` is not among the default targets.
- **Input:** per target a fresh copy of `examples/test-app` (config: worker count, log `warn`, no watch) or `bench/laravel-app` (`LARAVEL_APP`) with SQLite; schema via `opengrid task migrate_push` or `artisan migrate`; data via the API (1 user, `--notes` notes). Needs `surreal`, `oha`, for Laravel `php`, `php-fpm`, `nginx`.
- **Output:** progress on stderr, Markdown tables (requests/s, p50, p99, non-2xx, "in flight" per Little) on stdout; warning if fewer than 80 % of the connections are in flight on average. Correctness: only `assert`s when creating the data and the count of non-2xx responses; response contents are not checked.
- **Results:** stdout, optionally JSON (`--json`); by hand to BENCHMARKS.md.
- **In the suite:** no.
- **DBR1 (2026-10-27, D23):** target suffix `-core` (`opengrid-embedded-core`, `opengrid-embedded-pgo-core`, `laravel-octane-core`, `laravel-fpm-core`): per target a fresh SQLite database from `bench/framework/sqlite/seed.py`, endpoints `/api/bench/sql/…`, token via `/api/bench/sql/login`; the measurement header names the SQLite version (PHP pdo_sqlite) and the library linked by OpenGrid. `report.py` shows (a) core before (b) default and (c) database share.
- **THESIS1 (2026-10-27):** logic endpoints `<workload>:<n>` (`hydrate`, `collect`, `validate`, `json`; n = 0…1000, e.g. `hydrate:100`) only for `-core` targets, at `/api/bench/logic/…`; `validate` sends n elements (title, priority 1–5, two tags). With `--notes 1000` n = 1000 come from the notes of one user. **Response check:** before the measurement every logic response is fetched once; status, bytes and a hash of the canonical JSON are listed in a separate table and in the JSON (`answer|<endpoint>|<target>`); if a target deviates, "Difference" appears in the output and a warning on stderr. Function test 2026-10-27: all four workloads at n = 0 and 1000 byte-identical in OpenGrid, FrankenPHP, RoadRunner and FPM.
- **BENCH1 (2026-10-27):** cores centrally in `bm2.APP_CORES` (8, 10, 12, 14), `DB_CORES` (4–7), `LOAD_CORES` (0–3), topology per range in the measurement header; `patch_config` repeatable (before, only the first call took effect; affected only `startup.py`); RoadRunner/FrankenPHP get their own free side port (`SIDE_PORT`), on an early exit the server log is in the error message.
- **PERF4 d (2026-10-27):** `--opengrid-jit` sets `OPENGRID_JIT=on` for the OpenGrid targets (otherwise the variable is removed, D9); the measurement header names the JIT state.
- **Since 2026-10-27:** `--pg socket|tcp` (default socket, D21); measurement header with connection kind and durability settings; target suffix `-touchN` for every OpenGrid target (`touch1` = same semantics as Sanctum, `touch0` = stricter).
- **LV2 (e10940d):** targets `laravel-fpm-pgsql`, `laravel-octane-pgsql` (PostgreSQL of the compose service, own database per run, connection only via `PG*` variables) and `opengrid-embedded-touch0` (`auth.token_touch_seconds = 0`); `--label` sets the heading (e.g. "comparison of the default setups"). Measurement archiv/BENCHMARKS_bis_2026-10-26.md §4.v.
- **D25 (2026-10-28):** core targets only anymore (`opengrid-embedded-core`, `opengrid-embedded-pgo-core`, `laravel-frankenphp-core`, `laravel-octane-core`, `laravel-fpm-core`); everything else aborts with a hint. Removed: SurrealDB server, `-touchN`, Laravel default targets, filling via the API, `patch_config(touch=…)`. Measurement header: "SurrealDB not measured" with the note about the embedded SurrealDB opened at start.
- **2026-10-28:** the measurement header names the PHP version per target (`PHP 8.4.21 for FPM and RoadRunner, PHP 8.5.12 in FrankenPHP (own build)`); likewise `env.txt` (`php per target`) and `report.py`.
- **D24 (2026-10-27):** targets `-pgsql`, `--pg` and the PostgreSQL measurement header removed; Laravel default targets `laravel-fpm`, `laravel-octane`, `laravel-frankenphp` on fresh SQLite per run with the same SQLite 3.45.2 as the core (`LD_LIBRARY_PATH` for all Laravel targets).

#### `opengrid-framework:tests/suite/sqlite.rs::raw_sql_on_sqlite_with_typed_values_and_bindings`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact sqlite::raw_sql_on_sqlite_with_typed_values_and_bindings`; runtime ≈ 0.02 s; in the suite: yes.
- **Checks:** DBR1/D23 `DB::connection('sqlite')`: `statement` (CREATE TABLE/INDEX), `insert` → `true`, `select` with `INSERT … RETURNING id`, named (`:t`, also with a colon in the key) and positional bindings, types INTEGER/REAL/TEXT/NULL → int/float/string/null, bool as 0/1, `update`/`delete` → number of changed rows, `typeof` REAL for an int in a REAL column, pragmas of every connection (`journal_mode = wal`, `synchronous = 2` = FULL, `busy_timeout = 5000`), errors as `RuntimeException` with the SQLite message, array as binding → `TypeError`.
- **Kind:** integration; pool with the fixture project, connection via `settings.opengrid.connections.sqlite.url` to a temporary file.
- **Oracle:** `own expectation` (contract after Laravel's `DB::connection(...)`, types after SQLite).

#### `opengrid-framework:tests/suite/sqlite.rs::transactions_commit_and_roll_back`
- **Run:** `cargo test -p opengrid-framework --test suite -- --exact sqlite::transactions_commit_and_roll_back`; runtime ≈ 0.02 s; in the suite: yes.
- **Checks:** `transaction(fn)` commits on success (return value of the closure) and rolls back on an exception (the exception propagates); `getName()`; unknown connection → `Error` with a hint to `OPENGRID_DB_<NAME>_URL` or `settings.opengrid.connections.<name>.url`.
- **Kind:** integration, as above.
- **Oracle:** `own expectation` (Laravel's behavior of `DB::transaction`).

#### `bench/framework/sqlite/sqlbase.py`, `sqlbase.php`, `crates/framework/examples/sqlbase.rs` — **removed** (D24, 2026-10-27)
- Was: database share of the core and THESIS1 endpoints (SQLite statements alone via PDO and rusqlite). Jonas: the measurement of the database overhead is no longer needed (both sides use the same SQLite); the THESIS time breakdown shows only base cost (n = 0) and the part growing with n anymore.

#### `bench/framework/profile_core.py` (Callgrind profile of a core endpoint, PERF4 a)
- **Purpose:** where the time of an OpenGrid core endpoint goes: target `opengrid-embedded-core` set up as in `bm2.py` (1000 notes), but with **one** worker and only the server under Callgrind (`perf` is blocked in the container; `task migrate_push` runs natively). Instrumentation off at start and during warm-up (`--instr-atstart=no`), on only during the counted requests (`callgrind_control -i on/off`), then dump of the running server (`callgrind_control -d`).
- **Invocation:** `python3 bench/framework/profile_core.py <endpoint> [--requests 50] [--warmup 20] [--notes 1000] [--opengrid BIN] [--out DIR] [--top 40]`; endpoint as in `bm2.py` (`hydrate:1000`, `show`, …). Needs `valgrind` and `vgdb` (package `valgrind-gdb`, CONTAINER.md).
- **Output:** the top functions by self and inclusive instructions (`callgrind_annotate`) on stdout; file `../target/profile/callgrind.<endpoint>.out`. Instruction counts are stable between runs; inclusive values of recursive functions (`'2`) are over-counted in Callgrind.
- **In the suite:** no.

#### `tools/release.sh` (release binaries with PGO, PERF4 b)
- **Purpose:** PGO is the release default: builds `grd` and `opengrid` via `bench/pgo.sh` (training without the benchmark loads, D14), checks them and places them stripped with `MANIFEST` (commit, time, training, profiles, sha256) into `../target/dist/`.
- **Check:** `api.sh` completely with the PGO `opengrid` (62 steps); all probes `crates/vm/tests/probes/*.grd` with the PGO `grd` without and with JIT against their `.out`, compared as in the suite (output including error text, trailing whitespace does not count). Abort on any deviation.
- **Invocation:** `tools/release.sh [--no-pgo]` (`--no-pgo`: the same binaries without PGO, for comparison only); ≈ 20 min. Needs `llvm-tools-preview`, `cmp` (diffutils).
- **In the suite:** no.

#### `bench/framework/startup.py` (start time until the first response, PERF4 c / BENCH1)
- **Purpose:** start time of OpenGrid per worker count: test app copied, schema pushed (`task migrate_push`), then `opengrid start` with N workers on the app cores, time from process start until the first 200 of `/health` (polling every 2 ms), repeated; plus the time for compiling all `.grd` files of the project alone (`grd compile`).
- **Invocation:** `python3 bench/framework/startup.py [--workers 1,4,16] [--runs 5] [--opengrid BIN] [--grd BIN] [--cores 8,10,12,14]`; default the release binaries `../target/dist/`.
- **Output:** Markdown table (median, min, max per worker count) and the compile time on stdout.
- **In the suite:** no.

#### `bench/framework/querylog.py` — **removed** (D24, 2026-10-27)
- Was: queries per BM2 endpoint from the database logs (Laravel via PostgreSQL's `log_statement`). The finding remains valid and documented (BENCHMARKS §0.4, DECISIONS 2026-10-27): Sanctum writes `last_used_at` at most once per second, hence `touch1` as the same semantics.

#### `bench/framework/dbbase.py`, `dbbase.php`, `crates/framework/examples/dbbase.rs` — **removed** (D24, 2026-10-27)
- Was: database self-measurement §0.4 (SurrealDB embedded and server through the same SDK, PostgreSQL via PDO). Jonas: no longer needed for SQLite and SurrealDB; the results of run 77d4cc5 remain in BENCHMARKS §0.4 as the state.

#### `bench/framework/laravel/` (`setup.sh`, `serve.sh`, `overlay/`)
- **Purpose:** Laravel comparison application LV1 outside the repo.
  - `setup.sh [target directory]`: `composer create-project laravel/laravel:v13.11.0`, `laravel/framework 13.38.0`, Sanctum, Octane + RoadRunner; copies `overlay/` (own code: auth/note/health controllers, middleware `JsonWrites`, models `User`/`Note`, `AuthService`, routes, one migration); SQLite with WAL and busy timeout; `.env` (production, `BCRYPT_ROUNDS=10`); copies, if present, the database layer of a private reference implementation (not part of this repository; only into the app outside the repo). Refuses to install into `phprs/`.
  - `serve.sh <fpm|octane|frankenphp> <port> <workdir> [workers]` (`frankenphp`: Octane on FrankenPHP in worker mode, since 2026-10-27): `artisan optimize`, then nginx + PHP-FPM (`pm=static`, opcache on, JIT off) or Octane/RoadRunner in the foreground until SIGTERM.
- **Input:** network (Composer), `php`, `composer`; for `serve.sh` `php-fpm`, `nginx`.
- **Output:** app under `/workspaces/OpenGrid_Framework/bench/laravel-app` (never commit); `serve.sh` writes configuration, sockets and logs into the working directory.
- **In the suite:** no; used by `bm2.py` and `api.sh --app laravel`. The PHP drivers of the specification tests (`driver.php`, `driver_vendor.php`, see below) need the `vendor/` of this app.
- **BENCH1 (2026-10-27):** `serve.sh` reads `SERVE_CACHES` (`build` default: clear caches and rebuild; `keep`: use existing caches; `none`: clear only) and `SIDE_PORT` (RPC port of RoadRunner or admin port of FrankenPHP, default port + 1); it ends with the exit code of the server (before always 0) and shows on errors the end of `octane.log`, `php-fpm.log`, `nginx-error.log`.

#### `bench/pgo.sh` (PGO1)
- **Purpose:** profile-guided build of `grd` (default) or `opengrid` (`PGO_BIN=opengrid`): instrumented build, training, `llvm-profdata merge`, optimized build, in its own target directory `../target-pgo`.
- **Invocation:** `bench/pgo.sh [out-binary]`, environment `PGO_BIN`, `PGO_ROUNDS` (default 10), `PGO_TARGET_DIR`. Needs `rustup component add llvm-tools-preview`.
- **Training:** `grd`: support specification (`crates/vm/tests/support/cases.txt` as one script, `GRD_JIT=0`, 10 times) and all VM probes (without JIT and with `GRD_JIT_HOT=1`); errors are ignored. `opengrid`: `api.sh` 10 times with the instrumented binary. After D14 never with the measurement load (c1–c3, BM2).
- **Output:** `../target-pgo/<bin>-pgo`; profiles in `../target-pgo/profiles`.
- **In the suite:** no.

#### `tools/dx/r2.py` (R2)
- **Purpose:** classifies the PHP code blocks of 13 chapters of Laravel's documentation for OpenGrid: dialect (`grd compile`: clean, GRD code, or fragment), used Laravel APIs against what OpenGrid offers (read via regex from preludes, `register_native` calls, native classes and the rules in `validation.rs`), out of scope (facades/topics via regex). Verdict per block: "runs unchanged", "dialect change", "API missing", "out of scope".
- **Invocation:** `python3 tools/dx/r2.py [--grd BIN]` (default `../target/debug/grd`).
- **Output:** Markdown on stdout (table per chapter, most frequent missing APIs, GRD codes); details in `target/refs/r2.json`.
- **Results:** transferred by hand to ROADMAP (R2) or the DX documents.
- **In the suite:** no. The API mapping is heuristic (regex), the verdict an estimate, not a runtime proof.

#### Further files in `tools/dx/`
- There are none: `tools/dx/` contains only `r2.py`. The DX report per D9 comes from `grd check --report` (below).

#### `tools/phpt/run.py` (R1, php-src probes)
- **Purpose:** uses the `.phpt` tests of the php-src test suite (php-8.4.21) as probes: section `--FILE--` twice with PHP (non-deterministic tests are excluded) and once with `grd run` (`GRD_JIT=0`), each in its own temp directory with a 10 s CPU and 2 GB memory limit. Classification: `identical`, `dialect` (GRD code in the output or exception instead of warning, DIALECT §5), `gap` (missing function/class, different output, crash, timeout), `rejected` (both reject at compile time), `excluded` (SAPI/INI/ENV/STDIN sections, PHP error or timeout). The normalized output is compared (path replaced, trailing whitespace removed); the `--EXPECT--` sections of the phpt are not used, the reference is the local PHP.
- **Invocation:** `python3 tools/phpt/run.py [--grd BIN] [--jobs 8] [--cores 0-7] [--only SUBSTR] [--limit N]` (default `../target/profiling/grd`).
- **Output:** Markdown on stdout (for `.agents/PHPT_REPORT.md`), one JSON line per test in `target/refs/phpt/results.json`.
- **In the suite:** no. Crashes found end up in KNOWN_ISSUES; only individual cases were adopted as probes or tests.

#### `tools/gen_builtin_params.php`
- **Purpose:** generates `crates/vm/src/builtins/param_names.rs`: the parameter names of all PHP builtins registered in grd (found via regex `register_native("…")` in `crates/vm/src/builtins/*.rs`), read via reflection from the PHP in the container. Thus TypeErrors of the builtins name the parameter like PHP.
- **Invocation:** `php tools/gen_builtin_params.php > crates/vm/src/builtins/param_names.rs`.
- **Output:** versioned Rust file (header: "generated … from PHP 8.4.21 … Do not edit"). Functions PHP does not know are absent there.
- **In the suite:** no; whether the file matches the currently registered builtins is checked by nobody automatically.

#### `tools/tests_inventory.py`
- **Purpose:** inventory of all Rust test functions (id `crate:path::name`, unit/integration, `#[ignore]` with reason) and of the data-driven groups (`group:probes`, `group:support-cases`, `group:validation-cases`, `group:type-matrix`, `group:vm-fixtures`, `group:api-sh`). With `--check FILE` comparison against the descriptions in Tools.md (ids in backticks): reports tests without description and descriptions without test, exit 1 on deviation.
- **Invocation:** `python3 tools/tests_inventory.py [--markdown] [--check ../.agents/Tools.md]`.
- **Input:** sources under `crates/*/` (without `fixtures`, `target`); detects tests via regex (`#[test]`, `#[tokio::test]`).
- **Output:** stdout (count per crate or Markdown table).
- **In the suite:** no.

#### `grd` command line (`crates/jit/src/bin/grd.rs`, binary of the crate `grd-jit`)
- **`grd run <file>`:** VM with prelude and installed JIT (switches from the environment: `GRD_JIT=0`, `GRD_JIT_HOT`, `GRD_JIT_LOG`, `GRD_ADVANCED_JIT`, `GRD_JIT_SYNC`), runs the file, writes the output to stdout, errors to stderr; exit 0, 255 on a compile or runtime error, 70 on a prelude error.
- **`grd compile <file>…`:** compiles every file (parser, dialect rules, compiler) without execution; per file `OK <path> (n units, …)` or `ERR <path>: <message>`; exit 1 if one fails. Used by `bench.py` (compile time) and `r2.py`.
- **`grd parse <file>…`:** syntax check only (mago), `OK`/`ERR path:line: message`.
- **`grd dump <file>`:** bytecode per unit (`== unit i name (slots, params)`, then the opcodes with index).
- **`grd app <root> [entry.grd]`:** loads a project in module structure (`load_project`) and optionally runs a file in it (not mentioned in the task list, but exists).
- **`grd check --report <path>…` (DX1):** collects `.php`/`.grd` files and the PHP code blocks from `.md` files, checks each with `compile::check_source` and counts: syntax errors, files without dialect errors, compiler panics, hits per GRD code (hits, files), errors without code, GRD021 measurement (strict, tolerant `?T`, of which int↔float, judged/unjudged assignments, up to 15 examples). Enforces nothing, exit always 0. Result by hand to `.agents/DX_COVERAGE.md`.
- **Results:** stdout. **In the suite:** the binary itself is not (no test calls `CARGO_BIN_EXE_grd`); the functions behind it (`compile_source`, `check_source`, VM) are covered by library tests. The file's header comment is outdated ("(later) `grd run`").

#### Commit and green script (session scratchpad, **not versioned**)
Both lie under `/tmp/claude-1000/-workspaces-OpenGrid-Framework/69e64035-b258-414e-9d27-b5f2176c7715/scratchpad/`, i.e. outside any repo in a session-bound temp directory. They are lost with the scratchpad and apply to nobody except this work session.
- **`green.sh`:** runs `timeout 3000 taskset -c 0-7 cargo test --workspace --no-fail-fast --color never` in `phprs/`, log to `t.log` in the scratchpad; prints the `test result` lines with more than 0 passed tests and panics, then `exit <rc>, passed <sum>`. On a red run a copy as `red-HHMMSS.log`. Exit = exit of cargo.
- **`commit.sh <message file> <paths…>`:** calls `green.sh`; on red no commit. On green `git add <paths>` and `git commit -F`, with the line `Suite: N green` of the message filled with the counted number (work rule 5 of CLAUDE.md).
- **Git hooks:** none. `phprs/.git/hooks/` contains only the `*.sample` files, `core.hooksPath` is not set (`git config --get core.hooksPath` returns nothing). That the suite is green before every commit is thus ensured only by discipline or the unversioned `commit.sh`, no hook and no CI.

#### Harness scripts under `/workspaces/OpenGrid_Framework/.agents/feasibility/` (outside the repo)
All take the `grd` binary as the first argument. None runs in the suite; but the probes from them are frozen as copies in `crates/vm/tests/probes/` and run there.
- **`features/run.sh <grd>`:** 80 probes `a*.grd` (language features of the feasibility study). Per file: PHP output (`<?grd` → `<?php`, `timeout 10 php`) against the first 2 lines of `grd run` (without "Stack backtrace"); output `pass/fail/total` and list of deviations. Because of `head -2` meaningful only for probes with at most 2 output lines.
- **`laravel_style/run.sh <grd>`:** 25 Laravel-typical scripts (`01_interface` … `25_static_closure_scope`), same comparison with the first 3 lines; `PASS`/`FAIL` per file.
- **`legacy/php-ref.sh`:** generates the reference outputs of the 188 legacy scripts (`scripts/*.php`, harvested from the old phprs suite, tag `legacy-core-last`) with `php -d display_errors=1 -d error_reporting=E_ALL`: `*.out` (clean, 175), `*.fatal` (PHP exit ≠ 0, 11), `*.warn` (warnings/notices, 2).
- **`legacy/run.sh <grd>`:** compares `grd run` with every `*.out` (exact, exit 0) and writes `classify.txt` (`match`/`grd <code>`/`runtime`/`mismatch`); last stored state: 157 match, 1 mismatch, 1 grd, 16 runtime.
- **`bench/run.sh <grd>`:** micro-benchmarks `b1_loop` … `b7_objects`: median of 3, start time of an empty script subtracted, factor grd/PHP and output comparison (first line).
- **`bench/complex/run.sh <grd> [c1_request]`, `quick.sh <grd> [--php]`, `gen.sh`:** predecessors of `phprs/bench/complex/` (D11 move, DECISIONS 2026-10-25 06:25). `run.sh`: PHP (without/with opcache/with JIT) against grd, median of 3 per phase, checksum comparison. `quick.sh`: only grd against the PHP medians stored in `baseline.txt` (`--php` rewrites them). The `c*.php` are identical today with those in the repo (same SHA-1); `phprs/bench/complex/bench.py` is authoritative.

#### Further scripts in the repo
- **`crates/vm/tests/support/driver_vendor.php`:** specification driver S1/S1b (R3): evaluates every line of `cases.txt` with Laravel's own `Illuminate\Support` classes from the `vendor/` of LV1 (`declare(strict_types=1)`, polyfills `array_first`/`array_last`, adapters for `Arr::set/forget/push/pull`, because the dialect has no references) and writes `json_encode` per result; invocation `php driver_vendor.php <vendor-dir> <cases.txt> > cases.out`. Generates the expectation of the support tests (part of another section); does not run in the suite.
- **`crates/framework/tests/validation_spec/driver.php`:** specification driver R3 for validation: reads `cases.txt` (rules JSON TAB data JSON), evaluates with Laravel's own `Illuminate\Validation\Factory` and the English messages and writes per case `{"errors": …}` or `{"valid": …}`; invocation `php driver.php [vendor-dir] < cases.txt > cases.out`. Does not run in the suite.
- **`bench/complex/c1_request.php`, `c2_runtime.php`, `c3_compute.php`:** the benchmark scripts themselves (source; `.grd` via `gen.sh`).
- **`bench/framework/laravel/overlay/**/*.php`:** application code of the Laravel replica, not a tool.
- Otherwise there are no `.sh`, `.py` or `.php` files in the repo outside `tests/fixtures`.

## 4. Findings (only noted, not fixed; starting point for TEST1)

### 4.1 Contradictions found against an oracle (re-checked on 2026-10-26)

1. **Fixed with TEST2.2a.** `opengrid-framework:src/native/validation.rs::missing_field_reports_only_required` expected, for a present
   `null` with `required|string|max:3`, two messages ("required" and "must be a string"). Laravel's validator 13.38
   returns only `["The title field is required."]` (re-checked with `validation_spec/driver.php`). The test expectation and
   the code deviate from Laravel; recorded neither in LARAVEL_BREAKING_CHANGES nor in KNOWN_ISSUES; `cases.txt` has
   no such case.
2. **Fixed with TEST2.2b.** `opengrid-framework:tests/suite/middleware.rs::validation_errors_and_validated_data` expected `message` = first
   error message. With several errors Laravel appends "(and :count more errors)" (`ValidationException::summarize`,
   re-checked in vendor 13.38). Not in LARAVEL_BREAKING_CHANGES.
3. **Fixed with TEST2.2c.** `opengrid-framework:tests/suite/transactions.rs::transaction_arguments` expected a `ValueError` for `DB::transaction($fn, 0)`.
   Laravel then does not run the closure at all and returns `null` (loop from `$currentAttempt = 1`
   in `ManagesTransactions`, re-checked). LARAVEL_BREAKING_CHANGES lists `DB::transaction` as "equal".
4. **Fixed with TEST2.2d.** Probe `serialize_object_references` (`group:probes`): PHP prints a `Warning`; the `.out` freezes only
   `bool(false)`, grd silently returns `false`. DIALECT §5.3 demands an exception for warnings.
5. **Fixed with TEST2.2e** (re-checked: Laravel 13.38 returns `true`, also for an array with missing paths). `opengrid-framework:tests/suite/mail_storage.rs::storage_disks`: `delete` on a missing file returns `false`;
   Laravel (Flysystem 3, local) presumably returns `true`. **Not re-checked** (statement of a reading agent from
   memory), to be checked in TEST1.

### 4.2 Own expectation although an independent oracle would be possible

- Query builder and model (`opengrid-framework:tests/suite/database.rs::query_builder_like_laravel`,
  `…::model_crud_soft_deletes_and_pagination`, `opengrid-framework:src/native/database/builder.rs::renders_like_the_php_builder`):
  explicitly own expectation until `builder_cases.php` (B0); possible would be the builders of the private reference implementation or Laravel's builder on
  LV1 (result sets).
- ~~Laravel message texts as literals …~~ TEST2.6: `unique`/`exists` against `lang/en/validation.php`, "Unauthenticated." and others via `api.sh --app laravel` in the suite; "Too Many Attempts." (throttle) remains a literal.
- ~~HTTP behavior of the test app: `api.sh --app laravel` … but does not run in the suite.~~ Runs since TEST2.6 (`test_app_api_script_against_laravel`).
- ~~Cron translation of the scheduler …~~ TEST2.6: `frequencies_are_laravels_cron_expressions` (one deviation aligned).
- ~~Inheritance/declaration messages … without recheck~~ TEST2.6: `inheritance.rs::expectations_are_php_output`.
- Probes: no recheck against PHP and no regeneration switch (type matrix and support have both).
- ~~Differential tests JIT: the oracle is the own interpreter; for overflow-free programs PHP itself would be an oracle.~~ Since TEST2.4 also PHP for the scalar programs without dialect deviation.
- ~~`INFO FOR` forms … are invented~~ TEST2.6: `schema_ops.rs::real_info_answers_become_the_model` with a real `mem://` SurrealDB (the unit tests remain alongside).
- `types.rs::durations_round_trip` (SurrealDB's own duration formatting), `request.rs::nested_query_keys` (PHP
  `parse_str`), `inflect.rs::singulars` (`Str::singular`), `control.rs::task_names_follow_the_module_convention`
  (`Str::studly`): ~~PHP/Laravel-conformant today, but only literals~~ TEST2.6: `php_oracles.rs` (deviations found and fixed along the way; "conformant today" was not true for `parse_str` and `singular`).
- ~~`validation_spec`: `cases.out` frozen …~~ TEST2.6: `cases_out_is_laravel_output`.

### 4.3 Skipped, flaky, weak, without tests

**Self-skipping (green without checking; since TEST2.1 red instead of green with `OPENGRID_REQUIRE_ORACLES=1`):** `grd-vm:tests/suite/support.rs::support_expectations_are_laravel_output`,
`grd-vm:tests/suite/types.rs::strict_types_match_php` and `…::typed_operations` (PHP part without `php`);
`opengrid-framework:tests/suite/schema_golden.rs::golden_expectations_are_current_php_output` and
`…::schema_port_matches_php_on_platform_files` (without `php` or the private reference implementation, a no-op on every other machine);
`opengrid-framework:tests/suite/server_mode.rs::*`, `…/transactions.rs::transactions_retry_conflicts_on_a_server`,
`…/types.rs::round_trip_real_server` (without the `surreal` binary). The helper processes `db1_hold_child` and `surrealkv_child`
are `#[ignore]` and green without environment variables.

**Time-dependent / possibly flaky:** `grd-jit:tests/suite.rs::tier2_is_swapped_in_asynchronously` (polling 30 s, shares
budget and cache with other tests; flickered until 05b9a04 because of a code bug), `db_embedded.rs::start_gives_up_with_a_clear_message`
and `…::start_waits_for_the_previous_process…` (time limits, child processes), `reload.rs::watcher_reloads_on_change`,
`jobs.rs::durable_jobs_wait_for_the_runner…` ("not before its time" assumes < 1 s spacing), `schedule.rs` (polling),
`rate_limiter.rs::window_counts_and_expires`. KNOWN_ISSUES mentions an unreproduced red run of
`test_app_crud` and `api.sh` (2026-10-25).

**Can presumably not or hardly fail:** ~~`middleware.rs::middleware_groups_and_abort` (prefix probe against
a route that does not exist)~~ (TEST2.5), ~~`stress.rs` final checks (`admitted <= limit`, `cache <= admitted`),
`service.rs::budget_is_never_exceeded_under_contention` (race visible only with favorable scheduling),~~ (TEST2.4: budget checked exactly in its own process, barrier in the race test),
`schema/generator.rs::deterministic` (port against itself), `generator.rs::non_transitive_keys_do_not_panic` (only
"no panic"), `http.rs::loco_config_of_the_fixture_loads` (`debug`/`watch` follow from the argument),
~~`schema_kit.rs::preflight_allows_only_additions` (`contains("remove")` as an alternative), concurrency tests
(`transactions.rs`, `surreal.rs::concurrent_updates…`, `server_mode.rs::concurrent_writes…`), which do not show that
conflicts occurred at all~~ (TEST2.5: exact messages, conflict counter and final state; over the server no conflicts occur, there only final state). ~~JIT probe tests do not require that anything was compiled at all.~~ Fixed with TEST2.4 (minimum amounts; found along the way: only 2 of 258 probes had compiled code).

**Areas without tests:** ~~GRD016/GRD017 without rejection case; GRD102–GRD105 in grd-vm without test; forbidden builtins from
DIALECT §4.2 except for three; dependency order of modules (fixture has one module);~~ (TEST2.7) public `admit()` and
~~`GRD_JIT_TIER2_LIMIT`; JIT probes not combined with GC threshold 1;~~ (TEST2.4); ~~`backoff` for jobs~~ (TEST2.7); real SMTP delivery;
~~token expiry; validation with wildcards, `required_if` family~~ (TEST2.7, implemented and tested), custom messages; ~~`verify` "missing" and
name protection in the module manager~~ (TEST2.7); currency of generated files (`param_names.rs`, `bench/complex/c*.grd`).

**Tools:** ~~no CI, no git hook; the green rule depends on the unversioned `commit.sh` in the session scratchpad.~~ Fixed with TEST2.1 (`tools/green.sh`, `tools/commit.sh`, `.githooks/pre-commit`, CI, oracle switch; §3).
`.agents/feasibility/features/run.sh` compares PHP's full output with only two lines of grd (CLAUDE.md still lists the
command). ~~`bench/pgo.sh` has an outdated comment ("trains it with BM2 traffic").~~ (fixed 2063893)

### 4.4 Places where, according to the journal, the test expectation was wrong

| When, where | Test | What was changed | Why the expectation was wrong, not the code |
|---|---|---|---|
| 2026-10-23, DECISIONS (phase "Schema/Migrations") | test of the old imperative migration runner (removed with R4f on 2026-10-24) | expected batch number after a complete rollback | After the rollback the table is empty, the next batch starts again at 1; the author had expected continued counting. No oracle, justification only in the journal. |
| 2026-10-23, DECISIONS (crate `framework`) | first framework integration tests (today `opengrid-framework:tests/suite/http.rs::requests_leave_no_objects_behind`) | "empty object store counts 0, not 1" | After the request the arena is empty (RUNTIME_ARCHITECTURE, object store per request); the 1 was a counting error in the test. |
| 2026-10-23, DECISIONS (phase 6) | `grd-vm:tests/suite/native_classes.rs::dialect_model_extends_native_base` | `(int)` cast expectation replaced by strict conversion | Native arguments are converted strictly (dialect, later TYP3); the test had assumed PHP's lax cast. |
| 2026-10-24, DX_COVERAGE | probe `legacy_integer_arithmetic_002` | `.out` from `int(5)` to `float(5)` | Not a test error in the narrow sense: the rule "`/` yields float" (D10) was introduced; the expectation follows a new dialect rule. |
| 2026-10-25, this session, J3 | `opengrid-framework:tests/suite/schedule.rs::due_entries_run_in_process` | counts a still missing table as 0 (b7ca787) | Race in the test (counting before the first write), not in the scheduler. |
| 2026-10-25, this session, OPT1 | `grd-vm:tests/suite/native_classes.rs::profile_counts_calls_backedges_and_types` | checks element kinds instead of operand types (98f6680) | Not a test error: the checked behavior (type profile of the binary ops) was removed as dead work; test and code were changed together. |
| 2026-10-26, this session, J4 (before the commit) | `opengrid-framework:tests/suite/jobs.rs::durable_jobs_wait_for_the_runner_and_fail_into_failed_jobs` | two self-comparing assertions replaced, number of failed jobs 2 → 1 | Design error of the author, noticed before the commit: the assertions compared a value with itself; on retry the old row is deleted, hence 1. |

Otherwise the journal records no case in which an expectation was adapted to the code. The reverse (test right,
code wrong) for example: tier-2 cache key (05b9a04), exception text of failed jobs (2886d27).

### 4.5 Individual findings per part

The complete finding lists of the five parts (each with test id) follow unabridged; the most important are
summarized above.

#### Part A (grd-vm)

- `group:probes` / `grd-vm:tests/suite/probes.rs::frozen_probes_match_php_output`: for the probes there is neither a recheck against `php` nor a regeneration switch (type matrix and support have both). A wrongly frozen or stale `.out` goes unnoticed. An optional recheck as in `types.rs` (`<?grd`→`<?php`, `display_errors=stderr`, deviation list) would be cheap: my run took seconds.
- `group:probes`: the 3 dialect-induced expectations (`legacy_integer_arithmetic_002`, `a79_return_widening`, `250_stable_reverse_sorts`) are not marked in the files; test name and comment claim "PHP output". A list in the manner of `ops_deviations.txt` is missing. The separate count in STATUS demanded in DECISIONS 2026-10-23 is missing as well.
- `group:probes` (`serialize_object_references`): PHP prints a warning for `unserialize('a:2:{i:0;i:7;i:1;r:2;}')`; the `.out` freezes only stdout (`bool(false)`), grd silently returns `false`. According to DIALECT §5.3 ("everything that would be a warning in PHP throws an exception") grd would have to throw here; the probe pins down the opposite.
- `group:probes`: the stock contains only cases that already matched at freezing time (0 gaps by construction). ~~The 16 runtime gaps of the legacy tests … were not re-checked and followed up.~~ Re-checked with TEST2.3 (§1.3): the SPL structures are still missing (BI1 did not add them), two other gaps fixed and adopted as a probe.
- `group:probes`: the JIT runs (`crates/jit/tests/suite.rs`) do not combine JIT threshold 1 with GC threshold 1; a GC root error that occurs only in JIT code is thus not noticed.
- `group:probes` (harness): `features/run.sh` compares PHP's full output with `head -2` of the grd output; multi-line probes fail there wrongly. CLAUDE.md still lists the command as a check step.
- `grd-vm:tests/suite/support.rs::support_expectations_are_laravel_output`, `grd-vm:tests/suite/types.rs::strict_types_match_php`, `grd-vm:tests/suite/types.rs::typed_operations`: without `php` (or without `bench/laravel-app/vendor`) the oracle checks skip themselves and report green; only stderr reveals it. In a container without PHP the suite then checks only against the frozen files.
- `grd-vm:tests/suite/support.rs::support_classes_match_laravel`: the driver adapts Laravel (wrapper class `Arr`, polyfills). For methods that compare strictly instead of loosely in the dialect (DIALECT §5.1), Laravel is not an oracle; whether the cases avoid such places can only be seen case by case. Deviations are not kept as a separate list.
- `grd-vm:tests/suite/inheritance.rs::inheritance_checks_match_php` (and the three siblings): the PHP messages are literals in the test, without recheck against `php`. Today all 136 agree (re-checked); a recheck after the pattern of `types.rs` would be possible.
- `grd-vm:tests/suite/dialect.rs::rejected_constructs_carry_codes`: GRD016 (`__halt_compiler`) and GRD017 (inline HTML) are defined in `compile/dialect.rs` but have no rejection case. Only the code is checked, not message and hint. Builtins forbidden according to DIALECT §4.2 (`set_error_handler`, `ini_set`, file system, `extract`) are missing except for `compact`/`define`/`ob_start`.
- `grd-vm:tests/suite/dialect.rs::allowed_constructs_compile`: compiles only. That these constructs also run correctly is not shown by the test (the probes partly do that).
- `grd-vm:tests/suite/dialect.rs::native_recursion_throws_instead_of_aborting`: the message text is an own short form of PHP's message. The comment mentions a "small stack"; 8 MB is set, so no tightened condition.
- `grd-vm:tests/suite/project.rs::loads_modules_in_dependency_order_and_runs_routes`: the fixture has only one module, the dependency order promised in the name is not checked. GRD102–GRD105 from `vm/project.rs` have no test in this crate.
- `grd-vm:tests/suite/gc.rs::reachable_objects_survive`: according to the module comment statics are to be checked as roots; `Registry::$names` is never used (objects in statics are forbidden anyway, DIALECT §5.4). The GC statistics thresholds (`freed > 250_000`, `live < 30_000`, `runs > 5`) are freely chosen and depend on the collection strategy.
- `grd-vm:tests/suite/types.rs::locals_keep_their_type`: according to the code there are 140 sequences, STATUS.md and ROADMAP TYP1 say 147. Expectation and implementation both follow the same rule from DIALECT §5.2; a shared misunderstanding of the rule cannot be uncovered by the test.
- `grd-vm:src/bytecode.rs::module_round_trips_through_postcard`: `bytes.len() > 100` is a very weak additional check. The round trip itself is meaningful as long as `PartialEq` for `Module` compares all fields.
- `grd-vm:src/value/string.rs::append_copies_only_when_shared`: the pointer equality after `push_str("d")` depends on the remaining capacity of the buffer after the copy. Stable today, but bound to the allocation strategy.
- `grd-vm:tests/suite/native_classes.rs::profile_counts_calls_backedges_and_types`: checks only lower bounds or "some profile"; a wrong assignment of counters to functions would go undetected.

#### Part B (grd-jit, pkg, tools)

- `grd-jit:src/service.rs::budget_is_never_exceeded_under_contention`: whether a non-atomic implementation (load, compare, store) would actually fail here depends on scheduling (8 threads, 8000 attempts, budget 100: the race becomes visible only when several threads read the counter shortly before the limit at the same time, and the threads start staggered). So the test can be green even with a bug; it documents the current implementation rather than reliably finding regressions. The actual public function `admit()` and `GRD_JIT_TIER2_LIMIT` are untested.
- `grd-jit:tests/suite.rs::probes_match_php_with_tier1_only`, `grd-jit:tests/suite.rs::probes_match_php_with_tier2`: no assertion that any code was compiled (or reached tier 2) at all. Most probes contain no compilable code; with a JIT that silently switches itself off both would stay green. A minimum number of compiled units across all probes (as in the differential test) would catch that.
- **Fixed with TEST2.3** (recheck, `probes_deviations.txt`, uniform `<?grd`). `grd-jit:tests/suite.rs::probes_match_php_with_tier1_only` (applies equally to the VM probes): 4 of the 258 `.out` files deviate from PHP (`250_stable_reverse_sorts`, `a79_return_widening`, `legacy_integer_arithmetic_002`, `serialize_object_references`), so they were adapted to the dialect by hand. That is recorded nowhere next to the files; the comments speak sweepingly of "captured from PHP 8.4". No script in the repo regenerates the `.out` files or checks them against PHP. Moreover 94 probes start with `<?grd`, contradicting the DECISIONS entry ("the copies under `crates/vm/tests/probes/` keep `<?php`"); PHP can run them only after replacing the tag.
- `grd-jit:tests/suite.rs::tier2_is_swapped_in_asynchronously`: time-dependent (polling up to 30 s). It also depends on the process-wide tier-2 budget (default 4096) that all tests of the binary share. The differential tests consume, with threshold 1 and `advanced = true`, one slot for every newly compiled unit of 700 different programs. If they run first (e.g. with `--test-threads=1` in alphabetical order: `fuzz::…` before `tier2_…`), the budget could be exhausted, then the code would stay on tier 1 and the test would fail. That is a conjecture, not measured; in the logged run (`t.log`) the test finished before the differential tests.
- `grd-jit:tests/suite/fuzz.rs::interpreter_and_jit_agree_on_random_programs`, `grd-jit:tests/suite/fuzz.rs::interpreter_and_jit_agree_on_array_programs`: the oracle is the own interpreter. An independent oracle for overflow-free programs would be PHP itself (the programs are valid PHP, example above: PHP produces the same line); bugs that interpreter and JIT share are not found by the test. The exception count checks only `contains("Error")` on the output, so it does not distinguish by kind. `GRD_FUZZ_DUMP` ends the test early with green, without final checks; `GRD_FUZZ_CASES` sets the case count of both tests at once.
- `grd-jit:tests/suite/stress.rs::workers_rebuild_and_swap_tiers_under_load`: the final checks are almost tautological: `admitted <= limit` cannot fail with 6 variants and a budget of 4096 (and `admit` enforces it anyway), `cache <= admitted` holds by construction (only admitted jobs reach the cache). Whether tier 2 arrives and is swapped in during the run is not checked by the test; it shows above all that rebuilding VMs with a running background compiler produces no wrong results and no crash.
- `opengrid-pkg:src/lib.rs::add_lock_verify_remove`: the comment "a commit id pins exactly" stands before the step that changes a file, not before the pin step at the end. Unchecked remain `verify` with "missing", the rejection of invalid module names and the protection of `remove` against `..`/`/` in the name; `add` with a branch name is not checked.
- `opengrid-pkg:src/lib.rs::refuses_repositories_without_manifest_or_dependencies`: after the first failure it is not checked that nothing was created and no lock written (only in the second case for the module directory).
- Tools: `bench/pgo.sh` contradicts itself. The header says `opengrid` is trained with `api.sh` (D14), a later comment says "trains it with BM2 traffic"; the code uses `api.sh`. BENCHMARKS.md (commit d1b2b6a) still mentions training on c1–c3 (before D14).
- Tools: the suite green rule depends solely on `commit.sh`/`green.sh` in the session scratchpad; there is no versioned hook and no CI. Whoever commits outside this session has no automatic barrier.
- Tools: `api.sh --app laravel` would be the independent oracle for the Laravel fidelity of the HTTP responses, but never runs in the suite; the Laravel messages in the OpenGrid run are the author's literals there.
- Tools: `tools/gen_builtin_params.php` (→ `param_names.rs`) and `bench/complex/gen.sh` (→ `c*.grd`) generate versioned files whose currency nobody checks.
- Tools: `.agents/feasibility/features/run.sh` compares the full PHP output with only 2 lines of the grd output (`laravel_style/run.sh`: 3 lines); longer outputs always count as a deviation there.

#### Part C (framework integration 1)

- `opengrid-framework:tests/suite/database.rs::query_builder_like_laravel`, `…::model_crud_soft_deletes_and_pagination`: expectations come only from the author (the file header says so). Possible independent oracles: the builder chains from `builder_cases.php` of the private reference implementation (B0, waits for Jonas) or the same chains against Laravel's query builder on SQLite in the LV1 app (result sets, not SQL text). "like Laravel" is so far only a reading of the documentation.
- `opengrid-framework:tests/suite/database.rs::validation_exists_and_unique_against_the_database`, `opengrid-framework:tests/suite/auth.rs::token_authentication`: the message texts (validation.unique/exists, "Unauthenticated.", auth.throttle) were rewritten by the author. They could be checked automatically against Laravel's `lang/en/*.php` or the validator in the LV1 app.
- `opengrid-framework:tests/suite/api_script.rs::test_app_api_script`: in the suite only the OpenGrid side runs. The Laravel comparison (`--app laravel`) was done by hand and is not repeated; green thus means "as the script expects", not "like Laravel". Missing tools (`curl`/`jq`/`python3`) lead to red instead of skip. KNOWN_ISSUES mentions an unreproduced red run (possible flakiness).
- `opengrid-framework:tests/suite/db_embedded.rs::db1_hold_child`: a test without a check. Without environment variables it is always green (`return`), but counts as passed in `--ignored` runs.
- `opengrid-framework:tests/suite/db_embedded.rs::embedded_default_and_relative_paths`: depends on the environment (`OPENGRID_DB_URL`, `SURREAL_DB_HOST`, `OPENGRID_ROOT` in the shell make it red).
- `opengrid-framework:tests/suite/db_embedded.rs::start_gives_up_with_a_clear_message`, `…::start_waits_for_the_previous_process_to_release_the_database`: time limits (< 3 s, ≥ 500 ms) and child processes; conceivably flaky under load.
- `opengrid-framework:tests/suite/jobs.rs::durable_jobs_wait_for_the_runner_and_fail_into_failed_jobs`: the check "not before its time" assumes that less than 1 s passes between `dispatchLater(1, …)` and the next query (flakiness candidate). Plus a fixed `sleep(100 ms)` and polling up to 20 s. Same polling pattern in `…::jobs_run_after_the_request_with_retries`. Backoff between attempts (`backoff`) is not checked.
- `opengrid-framework:tests/suite/http.rs::requests_leave_no_objects_behind`: checks only one of the two workers. The log check is under `if let Some` and is silently omitted if the other worker handled the request; `logs <= 200` is weak.
- `opengrid-framework:tests/suite/http.rs::loco_config_of_the_fixture_loads`: `debug && watch` follows directly from the argument `dev = true` (almost tautological). Only `workers` and `app.name` are meaningful.
- `opengrid-framework:tests/suite/mail_storage.rs::mail_through_templates_and_outbox`: the line `try { Mail::send(new Plain()); } catch (Error $e) { echo 'x'; }` checks nothing. A printed `x` would stand before the next line, which is checked only via `contains`; the third send (then 3 entries in `Mail::sent()`) is not recounted. Real delivery (SMTP counterpart) is checked by no test, `smtp_failure_is_an_error_not_a_crash` only the failure case.
- `opengrid-framework:tests/suite/auth.rs::token_authentication`: "only the hash stored" is shown only negatively (plaintext absent). That `token_hash` = SHA-256(token) is not recomputed. Expiry (`expires_at`) and several tokens per user are missing.
- `opengrid-framework:tests/suite/errors.rs::production_hides_details`: whether the exception message ("kaputt") is visible in production in JSON/HTML remains unchecked (Laravel hides it on 500).
- `opengrid-framework:tests/suite/database.rs::*` (`model_crud…`, `raw_queries…`, `validation…`, `query_builder…`): the fixture schema (`.surql`, among others a UNIQUE index on `tasks.title`) is not applied; everything runs SCHEMALESS. Type checks/constraints of the database thus play no role here.
- `opengrid-framework:tests/suite/mail_storage.rs::storage_disks`: `delete` on a missing file returns `false` here. Laravel with Flysystem 3 on the local disk returns `true` (a missing file is not an error). This deviation is not in LARAVEL_BREAKING_CHANGES §8 (from memory against Laravel sources, not executed).

#### Part D (framework integration 2)

- `opengrid-framework:tests/suite/middleware.rs::validation_errors_and_validated_data`: expects `message == "The title field is required."` with three errors. Laravel returns `"The title field is required. (and 2 more errors)"` here (`ValidationException::summarize`). The deviation is not in `LARAVEL_BREAKING_CHANGES.md`. The error texts are literals; Laravel's validator would be available as an oracle (as in `validation_spec`).
- `opengrid-framework:tests/suite/middleware.rs::middleware_groups_and_abort`: the check "the group's prefix is required" (POST `/tasks/validated` → 404) cannot fail, because the route `/tasks/validated` does not exist in any form (the group route is POST `/admin/tasks`). A real prefix probe (group path without `/admin` that is routed nowhere else) is missing.
- `opengrid-framework:tests/suite/transactions.rs::transaction_arguments`: `attempts = 0` → `ValueError` deviates from Laravel (there: the closure does not run, return `null`); `LARAVEL_BREAKING_CHANGES.md` lists `DB::transaction` as "equal".
- `opengrid-framework:tests/suite/transactions.rs::transactions_retry_conflicts_embedded`, `…_on_a_server`: show only the result (no lost update), not that conflicts and retries actually occurred; if everything happened to run serially, the test would be just as green.
- `opengrid-framework:tests/suite/server_mode.rs::*`, `opengrid-framework:tests/suite/transactions.rs::transactions_retry_conflicts_on_a_server`, `opengrid-framework:tests/suite/types.rs::round_trip_real_server`: silent self-skipping without the `surreal` binary or `OPENGRID_TEST_DB_URL` (test green, only `eprintln!`). Present in the container; skipped invisibly elsewhere.
- `opengrid-framework:tests/suite/schema_golden.rs::golden_expectations_are_current_php_output`, `…::schema_port_matches_php_on_platform_files`: silent self-skipping without `php` or the private reference implementation; outside this workstation (confidential sources) always a no-op.
- `opengrid-framework:tests/suite/schema_golden.rs::schema_port_matches_php_on_edge_cases`: 5 of 16 cases compare against `expected.r4h.out`, which is generated via `GRD_GOLDEN_WRITE=1` from the Rust output itself; there the test is regression protection, not a PHP comparison. In write mode the test checks nothing.
- `opengrid-framework:tests/suite/schema_kit.rs::preflight_allows_only_additions`: very wide checks (`contains("remove")` as an alternative; `!e.0.is_empty()` on retyping) — any arbitrary error would suffice.
- `opengrid-framework:tests/suite/schema_kit.rs::surrealkv_child`: returns green without environment variables; the parent test detects success only by `1 passed`.
- `opengrid-framework:tests/suite/reload.rs::watcher_reloads_on_change`: time-dependent (300 ms sleep, 5 s polling), possibly flaky under load.
- `opengrid-framework:tests/suite/reload.rs::explicit_reload_and_broken_source`: whether the two requests each hit both workers is not enforced; "old code stays on a broken reload" (header comment) is not checked, only the error display.
- `opengrid-framework:tests/suite/schedule.rs::due_entries_run_in_process`: polling with a 15 s deadline on the job runner; the cron translation of the Laravel frequencies is an author literal, although Laravel's `Illuminate\Console\Scheduling\Event` (in the vendor of the comparison app) would be available as an oracle for the expressions.
- `opengrid-framework:tests/suite/server_mode.rs::concurrent_writes_to_one_record_over_ws`: checks only freedom from errors, no final state.
- `opengrid-framework:tests/suite/types.rs::lazy_rows`: binds itself to internal fields of `ModelData`; a pure implementation test.
- `opengrid-framework:tests/suite/types.rs::round_trip_real_server`: the header comment of `types.rs` mentions only `OPENGRID_TEST_DB_URL`, not the automatically started server (docs outdated). Likewise `fixtures/README.md` says "no database", although the fixture app has a schema with three tables.
- `opengrid-framework:tests/suite/validation_spec.rs::validation_matches_laravel`: `cases.out` is frozen; no test checks (as `golden_expectations_are_current_php_output` does for the schema) that it still matches the current Laravel output. Missing case groups: wildcards/nested fields, `required_if`/`required_with`, `exists`/`unique`, custom messages.
- `group:api-sh`: expectations are hand-written `jq` expressions; the Laravel mode compares against a replica that is likewise self-written and does not run in the suite. If `curl`/`jq`/`python3` are missing, `test_app_api_script` fails (no skip) — deliberately strict, but environment-dependent.
- `opengrid-framework:tests/suite/modules.rs::modules_from_git_pass_the_check_or_are_removed`: `modules.lock` (according to the header comment the pinning) is not inspected; `verify` only for `is_ok()`.

#### Part E (framework unit)

- `opengrid-framework:src/native/validation.rs::missing_field_reports_only_required`: **expectation contradicts Laravel.** For a present `null` with `required|string|max:3` the test expects two messages (required and "must be a string"). Laravel's validator (checked today via `tests/validation_spec/driver.php`) returns only "The title field is required.", because it stops after a failed implicit rule. The deviation is neither in `LARAVEL_BREAKING_CHANGES.md` nor in `KNOWN_ISSUES.md`, and `validation_spec/cases.txt` contains no case `required|<rule>` with `null`, otherwise it would have been noticed there. An independent oracle is available (the specification); the unit test does not use it.
- `opengrid-framework:src/native/database/builder.rs::renders_like_the_php_builder`: the name promises a comparison with the PHP builder, but the expectation is only own expectation (DECISIONS, builder format point 4; ROADMAP B0 "waits for Jonas", `builder_cases.php` is missing). With a green suite the byte equality with the reference implementation is **not** shown.
- Schema port in general (`schema/generator.rs::*`, `merge.rs::*`, `parser.rs::*`): all expectations are hand-written literals. The independent safeguard against the reference PHP classes lies only in `tests/suite/schema_golden.rs`. I sent the inputs of the unit tests through the reference driver today: where the test is not an R4h case, PHP agrees; the R4h cases deviate as documented. This check is not automated. The R4h cases (`a_later_placeholder_keeps_the_definition`, `integer_like_names_merge_by_name`, `escapes` case 2, `all_comment_forms`, `keys_of_statements` R4h part) are pure specification by the author.
- `opengrid-framework:src/native/database/schema/generator.rs::non_transitive_keys_do_not_panic`: shows only that no panic occurs (`len() == 3`).
- `opengrid-framework:src/native/database/schema/generator.rs::deterministic`: compares the port with itself. With `IndexMap` and explicit sorting it can practically not fail; `layout` already covers the same sorting with a literal.
- `opengrid-framework:src/native/request.rs::nested_query_keys`: checks only types and lengths, no values (`"1"`, `"a"`, `"b"`) and no order. PHP `parse_str` would be a direct oracle.
- `opengrid-framework:src/native/database/surreal.rs::concurrent_updates_of_one_record_are_retried`: does not demonstrate that conflicts occurred at all (if the engine serializes the writes, the test is trivially green). It also does not check the final value. Under load flakiness is conceivable if more than 20 conflicts occur in a row. It uses a real embedded database on disk.
- `opengrid-framework:src/native/rate_limiter.rs::window_counts_and_expires`: time-dependent (`sleep` 40 ms against a 20 ms window). The flakiness risk is low because `sleep` is a minimum duration.
- `opengrid-framework:src/native/database/schema/dbinfo.rs::info_answers_become_the_model`, `opengrid-framework:src/native/database/value.rs::base_types`, `opengrid-framework:src/native/database/schema/parser.rs::field_types`: the forms of the `INFO FOR` answers are invented. An independent oracle (a real `mem://` SurrealDB answering `INFO FOR TABLE`) would be cheaply available in-process; DECISIONS DB1 lists the server test of the type parser as open.
- `opengrid-framework:src/native/database/types.rs::durations_round_trip`: the formatting could be checked against SurrealDB's own `Duration` representation (the dependency is present); the test uses only own literals.
- `opengrid-framework:src/native/database/schema.rs::statements`: the generated statements are not parsed by SurrealDB; a syntax error in the generator would not be noticed here.
- `opengrid-framework:src/native/database/schema/preflight.rs::required_fields`: the `VALUE` clause (according to the doc comment also "not required") is not tested. `manifest_step_rules`: not tested are non-empty `renames`, unknown step kinds (fail closed) and the allowed `apply_schema`/`assert_sql`. The manifest is hand-written, not generated by SurrealKit.
- `opengrid-framework:src/native/database/builder.rs::sort_specs_are_checked`: for the valid sort list only the count is checked, not field and direction.
- `opengrid-framework:src/native/validation.rs::rules`: the message of the `confirmed` rule is not checked (Laravel: "The pw field confirmation does not match.").
- The expectations of the Laravel-adjacent tests (`control.rs::task_names_follow_the_module_convention`, `inflect.rs::singulars`, `http.rs::query_parsing`, `validation.rs::size_messages_name_the_unit_like_laravel`) agree today with `Str::studly`, `Str::singular`, `parse_str` or Laravel's validator, but are only literals. A comparison as in `crates/vm/tests/support/` or `validation_spec/` would be possible.

## 5. Maintenance

- **Inventory:** `phprs/tools/tests_inventory.py` generates the list of tests (id, unit/integration, `#[ignore]` with
  reason) and of the data groups from the code. `--check ../.agents/Tools.md` reports tests without description and
  descriptions without test (exit 1). `--markdown` prints the list as a table.
- **Rule from D17 on:** every new package records its tests and tools here (id in backticks, Checks/Kind/Oracle/Run);
  a test without an entry counts as not finished. The inventory script runs before a package is closed.
- **Runtimes** are re-measured with `RUSTC_BOOTSTRAP=1 cargo test --workspace -- --test-threads=1 -Z unstable-options --report-time`.
- Tools.md is step 1 of TEST1 (review of test coverage and quality); §4 is its work list.
