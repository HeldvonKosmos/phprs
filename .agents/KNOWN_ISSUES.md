# Known gaps (grd-vm and framework)

Open points first, fixed ones below (each with commit). Record gaps as soon as they are found: snippet, expected PHP output,
location in the code (CLAUDE.md, rule 4). The entries about the old phprs core are in `archiv/KNOWN_ISSUES_alter_Kern.md`.

- JIT: the tier-2 cache key had included the profile since JIT2b; a second worker did not find the tier-2 code (test `tier2_is_swapped_in_asynchronously` red in roughly every second parallel run) → profile removed from the key (05b9a04).

- **No code regression:** BM2 database endpoints ≈ 30 % below d12a26b (found 2026-10-26 at 2886d27). Re-measurement (REG1, 2026-10-26): d12a26b and 98f6680 equal today (show 1480/1498, create 1004/1012 requests/s, each the median of 3); with the database in RAM (`TMPDIR=/dev/shm`) d12a26b 3821/3518 against a83d6b7 3839/3538. Cause: slower disk writes in the container since the morning of 2026-10-25 (every authenticated request writes `last_used_at`; Laravel FPM on SQLite likewise ≈ 15 % slower). No fix needed; the regression guard has measured disk and RAM separately since then (archiv/BENCHMARKS_bis_2026-10-26.md §4.z).

## Open

### Flaky test (2026-10-27): `api_script::test_app_api_script`, step `schema: migrate_push`
Red once under the load of the whole suite: `opengrid task migrate_push` ended with a panic (only the end of the backtrace
was in the log, `opengrid.rs:39`). Green three times on its own; every execution has its own directory and its own
embedded database. Cause open. Since 2026-10-27 `api.sh` prints the panic line itself so that the next occurrence names it.

### Noted from PERF3 (2026-10-26): regex classes without `/u`
`preg_replace('/(\w)(\d)/', '$2$1', 'é55')`: PHP without `/u` works bytewise, `\w` matches only ASCII (→ `é55`); grd
(fancy-regex on `&str`) always treats `\w`/`\d`/`\s` as Unicode-aware (→ `5é5`). Affects only non-ASCII text without `/u`.
Location: `crates/vm/src/builtins/regex.rs::compile`.

### Noted from PERF3 (2026-10-26): two small JSON deviations
- `json_decode('"unterminated')`: PHP reports error 3 ("Control character error, possibly incorrectly encoded"), grd 4
  ("Syntax error"); serde_json does not distinguish the cases. Location: `crates/vm/src/builtins/json.rs`.
- Objects from `json_decode` without `assoc`: PHP assigns the object numbers from the inside out (`var_dump` shows the
  outer one as `#3`), grd from the outside in. Visible only in `var_dump`.

### Open from TEST2.6 (2026-10-26): element fields with their own clauses and `migrate_pull`
For `DEFINE FIELD k ON t TYPE array<int>` SurrealDB 3 creates the element field `k.*` itself; a separate
`DEFINE FIELD k.* ON t TYPE int ASSERT $value > 0` then fails with "The field 'k.*' already exists" and only works with
`DEFINE FIELD OVERWRITE k.* …`. `ops::pull` emits such a field from `INFO FOR TABLE` without `OVERWRITE`; the
pulled file then cannot be applied to an empty database. (The automatic element fields have been omitted by `pull`
since TEST2.6.) Location: `crates/framework/src/native/database/schema/dbinfo.rs::from_info`. Not fixed, because the
code base of a private reference implementation (not part of this repository) uses no element fields with their own clauses; add `OVERWRITE` when generating if needed.

### SurrealDB 3.3.4 (upstream, worked around in DBP1)
- `ORDER BY field COLLATE` uses no index, even for int fields (all hits are sorted). Workaround: no `COLLATE` on non-string fields (af521d8).
- `count()` with bound parameters reads every matching row (`IndexScan` + `Aggregate`); with literals it counts from the index (`IndexCountScan`). Workaround: safe literals in count queries (e0e0ae4).
- No prepared-statement or plan cache in the SDK; parsing ≈ 7 %, planning ≈ 30 % of a request (DBP1 profile). Solvable only upstream.

### Noted from R1, still open (2026-10-25)
- `protected` via a common ancestor: PHP allows calling a `protected` method if it is declared in a
  common ancestor of both classes (php-src gh14009_004); grd rejects ("Call to protected method
  C::test() from scope D").
- Anonymous classes do not evaluate their modifiers (`new readonly class {}`: readonly has no effect, duplicate modifiers
  are not reported).
- Text before `<?php` (e.g. an empty line) is not output by grd; PHP outputs it.
- `var_dump` shows different object IDs than PHP, because grd does not free objects and does not reuse IDs
  (object lifetime, see retrospective; architecture question).
- Missing builtins with many hits in php-src: `error_reporting` (41), `pack` (19), `hash_init` (15),
  `debug_print_backtrace` (16); filesystem/streams are GRD010 (`get_class_vars`, `class_alias` present since BI1).
- GRD021 and accumulators: `$carry = $this; foreach (…) { $carry = $cb($carry); }` throws GRD021 as soon as a
  callback returns a different type (Laravel's `pipeThrough`, `reduce` with an object start value). Workaround in the
  prelude: `array_reduce` or closure parameters (`mixed` is allowed there). Watch for user code in DX_COVERAGE.
- `(new B())->p = 1` (writing to a parenthesised `new` expression): PHP rejects at compile time ("Cannot use
  temporary expression in write context"), grd accepts. `f()->p = 1` is allowed by both.
- TypeError messages of user functions lack PHP's suffix ", called in <file> on line <n>" (builtins identical in wording
  since 9a81b70, with parameter name and class).
- Runtime text "Cannot call abstract method": identical, but the trace format differs.

### From the D13 analysis (2026-10-25, ANALYSE_D13.md)
- `pack`/`unpack` are missing (BI1 remainder; unused in Laravel, 19 hits in php-src).
- Wall-clock measurements on this machine vary between runs by up to a factor of 2 (same binary,
  same input, 73 ms vs. 178 ms); therefore compare with Callgrind Ir or alternating series (D11).
- The machine runs with CPU governor `powersave`; wall-clock benchmarks scatter in the afternoon up to σ 56 % (c1), the
  process start varies 26–72 ms. Ratios within one run remain usable; Callgrind Ir is reliable.
- Test gap, found during the FRAME1 rework (61c2336): `$class::m()` (class name dynamic among the arguments) had
  no VM test, only the framework suite caught the error; now probe `dynamic_static_calls`.
- Build trap: `cargo build -q … | grep "^error"` swallows errors because of colour codes; always `--color never`.
  Second trap: a build in the Git worktree writes into the shared `target/` and masks the next build
  in the main checkout (binary from the old state, same timestamps); after worktree builds `touch` + rebuild.

### Noted from AU2 (2026-10-25)
- Rate-limiting counters live in the process memory (like Laravel's `array` store): a restart resets them, several
  instances count separately. For several instances, a store in SurrealDB (like the sessions).
  (The peer address behind the proxy is fixed with IP1, c7c0863: `X-Forwarded-For` from Traefik.)

### From the legacy harvest (2026-10-24, not built now; decision after the requirements of a private reference implementation are known)
- Missing: `SplStack`, `SplQueue`, `SplMaxHeap`, `SplMinHeap`, `SplPriorityQueue`, `mb_strrpos`, `soundex`, `ReflectionParameter(fn, name)`.
- `filter_var` with numeric filter IDs (513/519/520) is not recognised.
- Dialect-induced, not a gap: `preg_match_all` without a by-ref counter returns the match array; Fiber frozen (DIALECT §4.3);
  `file_exists`/`DirectoryIterator` are the architecture question "filesystem in the dialect" (GRD010, DECISIONS).

### Red once: `test_app_crud` and `api.sh` simultaneously "controller class `Auth\Controllers\AuthController` not found" (2026-10-25, 10:5x)
In one suite run both tests failed at the same time, both with the module `auth` missing (GET / from `welcome`
worked). Both copy `examples/test-app` (one in-process, the other as a server). Afterwards 2 full suites and
8 runs of the two tests green; not reproduced. Suspicion: incomplete read while copying, or a
load order that depends on hash order. The gate script now preserves red logs
(`red-*.log`) to see the next occurrence. Commit e84781f slipped past the red suite because
the command chain checked the exit code of `tail`; commits have since run through a script that checks the exit code of
the suite itself.


## Fixed

### Fixed (2026-10-26, TEST2)

- JITS1 (2026-10-27): deep recursion between compiled functions ran without a limit on the machine stack and terminated the process
  (stack overflow, SIGABRT; e.g. `function down(int $n): int { return $n === 0 ? 0 : 1 + down($n - 1); }` with `down(200000)` and
  JIT). Now a depth limit of 1000 per native call chain, below that the interpreter. Test `depth.rs`.
- 2026-10-27: `schedule::frequencies_are_laravels_cron_expressions` failed once because `serve.sh` (started by `api.sh --app laravel`)
  was simultaneously clearing and rebuilding the caches of the Laravel app (missing `routes-v7.php`). Both now take the same lock (2f52441).
- JITO1 differential test: uninitialised typed properties. The message named the class of the object instead of the
  declaring one (`Q::$z` instead of `P::$z`), `var_dump` showed `uninitialized(mixed)` instead of the type and counted the property
  (`(2)` instead of `(1)`). Fixed in `vm/path.rs::read_prop` and `builtins/output.rs`, probe `typed_props_uninitialized`.
- DIA2 (D20): the two open dialect questions from TEST2.4/TEST2.8 decided and implemented: `%` and bit operations with
  float operands throw `TypeError` (hint `fmod()`/`(int)`; this also removes the saturation of values that are too large),
  `abs(PHP_INT_MIN)` throws `ArithmeticError`, `++`/`--` on `null` (including `null--`) and strings throw `TypeError` (DIALECT §5.2).
- MUT2: `(float)"-0"` gave `0.0` instead of `-0.0`; `var_dump((float)"-0")` shows
  `float(-0)` in PHP. `to_float` now keeps the sign when a string with `-` parses to int 0 (`value/convert.rs`, probe `value_core_edges`).
- LV2 `opengrid-embedded-touch0`: 2 of 10 855 `create` requests not 2xx, because a failed write of
  `last_used_at` made the request fail → `Auth::noteUse` retries once, otherwise a warning in the log and the
  request is answered normally (TOK2).
- `preg_split` with empty matches deviated from PHP: `preg_split('//', 'abc')` gave `["a","b","c",""]` (PHP
  `["","a","b","c",""]`), `preg_split('/\s*/', 'h i')` dropped the empty match after a non-empty one; the
  constant `PREG_SPLIT_OFFSET_CAPTURE` was missing (the flag itself worked) → matches iterated like PCRE, probe
  `preg_split_flags` (PERF3).
- `json_encode(…, JSON_UNESCAPED_UNICODE)` emitted U+2028/U+2029 raw; PHP escapes them anyway (except with
  `JSON_UNESCAPED_LINE_TERMINATORS`, the constant was missing) → like PHP, probe `json_edges` (PERF3).
- `var_export(PHP_INT_MIN)` emitted `-9223372036854775808` (not a valid literal in PHP); PHP writes
  `-9223372036854775807-1` → like PHP (`crates/vm/src/builtins/output.rs`), found with `operators_slow_paths` (TEST2.8).
- Wrong operand type in arithmetic/bit ops: grd reported "Unsupported operand type array for +", PHP "Unsupported
  operand types: array + int" (both types) → PHP's form, for strings with the dialect hint "; use (int)/(float)";
  `~[]` like PHP "Cannot perform bitwise not on array". Found with the probe `operators_slow_paths` (TEST2.8).
- Query strings: the dialect's `parse_str` and the request parser (two separate implementations) deviated from PHP:
  `a.b=1&c d=2` gave keys `a.b`/`c d` (PHP: `a_b`/`c_d`), `a[=1` gave `a` (PHP: `a_`), `[k]=1` and `=c` were not
  discarded → one function modelled on `php_register_variable_ex` for both (`grd_vm::builtins::insert_query_key`), found with
  `php_oracles.rs::query_strings_parse_like_php`.
- `inflect::singular` (foreign-key convention) deviated from Laravel's `Str::singular` for 7 of 22 table names
  (`statuses` → `statuse`, `analyses`, `indices`, `quizzes`, `buses`, `wolves`, `mice`) → rules extended, 67 words checked against
  Laravel (`php_oracles.rs`).
- `migrate_pull` (`ops::pull`) emitted the element fields created by SurrealDB 3 itself (`l.*` for `array<string>`);
  the pulled file could not be applied again → `from_info` omits them (`schema_ops.rs::real_info_answers_become_the_model`).
- JIT: the process-wide tier-2 cache never hit. `build_sel_tables` assigned method and property selectors in the
  order of the `FastMap` (hash seed per instance), so the linked prelude modules had different `SelId`s in every VM
  and the content key never matched; every VM compiled tier 2 anew (budget used up faster, no sharing
  between workers). Found with `stress.rs::tier2_budget_is_used_up_exactly`; `tier2_is_swapped_in_asynchronously`
  did not notice because the second VM's own tier 2 arrived asynchronously before the assertion → selectors in a fixed
  order (`crates/vm/src/vm/class.rs`).
- `mb_strwidth("hello 世界")` gave 8 instead of 10 (wide East Asian characters count 2; `char_width` already existed, only `mb_strwidth` did not use it); `filter_var` did not know `FILTER_SANITIZE_NUMBER_INT`/`_FLOAT` and the flags `ALLOW_FRACTION/THOUSAND/SCIENTIFIC` (returned the input unchanged) → both like PHP, found during the re-check of the old tests (TEST2.3), probe `mb_strwidth_and_number_sanitizing` (`crates/vm/src/builtins/string.rs`, `misc.rs`).
- `unserialize` did not check delimiters: `unserialize('s:5:"abc";')` returned `string(5) "abc\";"` (PHP: warning, `false`), likewise `b:2;`, wrong class-name length, missing `}` → every delimiter checked, broken input throws `ValueError` (DIALECT §5.3) (4e23616, `crates/vm/src/builtins/misc.rs`).

### Fixed (2026-10-25 afternoon, D13 stability)

- **Security:** field names and operators in `where`/`whereIn`/`whereLike`/`whereNull` went unchecked into the SurrealQL query (injection if they come from request data). Now identifier path and operator list, otherwise `ValueError`; `whereRaw` stays raw (c68dc1e, Q1).

- Typed static properties without a check (`T::$i = 'x'` accepted) → checked like instance properties (dff3564).
- Builtins coerced scalars (`strlen(5)`, `str_repeat('x', 2.0)`, `implode(1, …)`, `abs('-3')`, bool options) → strict,
  only `int → float` (27bf234, 793c2cb).
- No freeing within a request → limit per request (84248ac) and mark-and-sweep at safe points (45c9f6f).
- Missing builtins/classes from the coverage analysis (`hash_hmac`, `hash_equals`, `ArrayObject`, `WeakMap` …) → b4820c3;
  `hrtime()` monotonic.
- Test error (not a VM error): `schedule::due_entries_run_in_process` red once in the full suite, "The table 'stamps' does not exist": the test counted before the first scheduled run had created the table → a missing table counts as 0 (b7ca787).

### Fixed (2026-10-25, S1b)

- **`if ($c ? f() : $a === $b)` took the wrong branch.** The peephole fusion (`compile/mod.rs` `try_fuse`)
  merged the `JumpIfFalse` after the ternary with the comparison of the else branch, although the then branch targeted
  exactly this jump; the then branch thus jumped past the test and always entered the body. Found via
  `Collection::after` with a closure. Fusion is now skipped when a label points to the position of the new op
  (7412da1, probe `fused_jump_label`). Also affected `while`/`for` conditions of the same shape.
- **`func_get_args` was missing** (a6503f2, probe `func_get_args`). Extra arguments without a variadic parameter
  are discarded by the VM when binding; `func_get_args` therefore does not see them (PHP does). Acceptable for the dialect.

### Fixed (2026-10-25, php-src probes R1, D12)

All 43 crashes and timeouts from `tools/phpt/run.py` (list and commits in PHPT_REPORT.md). Most important for the
framework: endless recursion through native code (callbacks, magic methods, `__toString`, constants, `json_encode`,
`serialize`) aborted the whole process, in the framework thus the server; now exceptions or PHP's behaviour.
`DateTime::add/sub` computed hours in local time (endless loop or skipped hour at daylight-saving transitions).

### Fixed (2026-10-25, found with LV1/BM2, D11)

- **Concurrent requests with the same token → 500.** Every authenticated request writes the token's `last_used_at`
  (like Sanctum); surrealkv rejects concurrent writes to the same record with
  "Transaction write conflict. This transaction can be retried". BM2, embedded, 16 connections: up to
  25 % 500s. `Db::query` now retries such conflicts outside explicit transactions (27534a3, test
  `concurrent_updates_of_one_record_are_retried`: without retry 191 of 320 writes rejected).
  Inside `DB::transaction()`: since c8b8043 with `$attempts` as in Laravel (TX1).
- **Messages of `min`/`max`/`size`/`between` without a unit:** "must be at least 8." instead of Laravel's "… 8
  characters." (strings) or "must have at least 2 items." (arrays) (f67d168).

### Fixed (2026-10-24 night, D10)

- `Builder::sort()`/`orderBy()`: field and direction went unchecked into the query, `?sort=` from the request
  likewise (SurrealQL injection). Now only identifier paths and asc/desc, invalid `?sort=` → 400 (0bfce6c).
- `exists:`/`unique:` on a table not yet defined threw a database error; they now count as empty.

### Fixed (2026-10-24, found through JIT work and the differential test)

- Expression statements without assignment (`f();`, `$p->step(0.5);`) left their
  result on the operand stack; in the main script the stack grew
  without bound (fed82d3).
- `function f(): float { return 5; }` returned `int(5)` instead of `float(5)` (PHP
  widens int on a float return); returns through `finally` were not checked at all (2df0b26, probe a79).
- `var_dump(9223372036854775808.0)` gave `float(9223372036854776000)` instead of
  `float(9.223372036854776E+18)`; `var_dump(-0.0)` gave `float(0)` (7382ee7,
  probe a80 with 23 boundary values).
