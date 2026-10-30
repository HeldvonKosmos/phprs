# DX_COVERAGE — migration cost of the dialect rules (DX1)

As of 2026-10-24, 22:00 UTC. Since D10 this file is **no longer a basis for decisions** but the
measurement of the migration cost: GRD021 and `ArithmeticError` are enforced. Update as soon as PL1
(the `app/**`, `routes/**` of a private reference implementation, not part of this repository) is available. Tool: `grd check --report <path>…` (counts, enforces nothing;
the compiler collects one error per statement).

## R2: Laravel documentation per chapter (2026-10-25, D12 phase 1)

Tool `phprs/tools/dx/r2.py` (reads the Laravel documentation at run time; the documentation is fetched separately and is not part of the repository; individual results in `phprs/target/refs/r2.json`).
For each PHP block of the 13 relevant chapters: dialect (`grd compile`: clean / GRD code / fragment),
Laravel APIs used against the APIs OpenGrid has (read from the preludes and the registered native
methods), and topics outside the scope. Verdict: not in scope → API missing → with dialect change → runs
unchanged. **Limits:** the API mapping is heuristic (patterns such as `Str::x`, `$request->x()`, `->x()` in the
Eloquent/Collection chapters); "runs unchanged" means "compiles and all recognised APIs exist", not
"executed" (the blocks are mostly excerpts). Methods that the examples define themselves (relations,
scopes) do not count as missing.

State: phprs 63de5f0, Laravel documentation at 36fa00d; 1604 PHP blocks in 13 chapters.

| Chapter | Blocks | runs unchanged | with dialect change | API missing | not in scope | of which fragments¹ |
|---|---:|---:|---:|---:|---:|---:|
| routing | 64 | 26 | 0 | 21 | 17 | 13 |
| controllers | 41 | 11 | 1 | 25 | 4 | 2 |
| requests | 81 | 30 | 0 | 44 | 7 | 8 |
| validation | 175 | 122 | 0 | 41 | 12 | 79 |
| eloquent | 115 | 52 | 0 | 56 | 7 | 6 |
| eloquent-relationships | 163 | 33 | 0 | 127 | 3 | 19 |
| queries | 129 | 28 | 0 | 97 | 4 | 0 |
| middleware | 24 | 21 | 0 | 1 | 2 | 9 |
| responses | 42 | 17 | 0 | 13 | 12 | 2 |
| collections | 228 | 114 | 0 | 103 | 11 | 1 |
| helpers | 256 | 96 | 0 | 142 | 18 | 10 |
| strings | 280 | 53 | 0 | 77 | 150 | 0 |
| configuration | 6 | 4 | 0 | 2 | 0 | 1 |
| **Total** | 1604 | 607 (38 %) | 1 (0 %) | 749 (47 %) | 247 (15 %) | 150 |

¹ Blocks that do not compile on their own (methods without a class, `...`); their APIs are checked nevertheless.

### Missing APIs (blocks, 60 most frequent)

`Validator::make` (32), `Builder::with` (15), `Route::resource` (13), `Builder::belongsto` (13), `Builder::belongstomany` (13), `Builder::orwhere` (9), `Builder::wherecolumn` (9), `Builder::hasone` (7), `Builder::has` (7), `Route::singleton` (6), `Builder::orderbydesc` (6), `Builder::hasmany` (6), `Builder::chaperone` (6), `Builder::morphto` (5), `DB::raw` (5), `retry()` (5), `Request::safe` (4), `Builder::chunkbyid` (4), `Builder::withattributes` (4), `Builder::ofmany` (4), `Builder::through` (4), `Builder::morphone` (4), `uri()` (4), `Route::apisingleton` (3), `Builder::increment` (3), `Builder::isproduction` (3), `Builder::chunk` (3), `Builder::restore` (3), `Builder::history` (3), `Builder::command` (3), `Builder::daily` (3), `Builder::wherebelongsto` (3), `Builder::latestofmany` (3), `Builder::using` (3), `Builder::wherehasmorph` (3), `Builder::attach` (3), `Builder::selectraw` (3), `Builder::join` (3), `Builder::on` (3), `Builder::from` (3), `Builder::wherevectorsimilarto` (3), `Collection::after` (3), `Collection::doesntcontain` (3), `Collection::eachspread` (3), `Collection::random` (3), `Collection::sliding` (3), `Collection::sole` (3), `Collection::splice` (3), `data_set()` (3), `route()` (3), `logger()` (3), `now()` (3), `rescue()` (3), `defer()` (3), `Str::doesntcontain` (3), `Str::plural` (3), `Route::where` (2), `Route::scopebindings` (2), `Route::firstorfail` (2), `Route::apiresource` (2)

### Not in scope (blocks per topic)

Fluent strings (S1b) (147), Redirects/Sessions with views (28), Carbon (24), Rate limiting (AU2) (13), LazyCollection (S1b) (12), Views/Blade (7), Storage/Uploads (MS1) (4), Cache (4), Authorization (Gates/Policies) (3), Events/Broadcasting (3), Localization (3), Hashing/Encryption facades (3), Migrations (schema is declarative) (2), Queues/Jobs (J2) (1)

### Dialect changes (GRD codes, blocks)

GRD008 (1), GRD010 (1), GRD006 (1)

**Reading:** the dialect is hardly an obstacle (3 blocks). What is missing are APIs: Eloquent relations in the Laravel
style (`belongsTo`, `hasMany`, `belongsToMany`, `with`, `has`, `morphTo` …; the ported database layer works with `withAll`
and graph edges), query-builder methods (`orWhere`, `whereColumn`, `whereBetween`, `orderByDesc`, `latest`,
`pluck`, `chunk`, `join`, `increment`, `DB::raw` …), `Validator::make`, `Route::resource`/`singleton`, helpers
(`retry`, `now`, `data_set`, `route`, `logger`, `rescue`), individual Collection/Str methods and above all
fluent strings (`Str::of`, 147 blocks) — that is the scope of S1b and of a new package for the query builder.

## GRD021 and ArithmeticError per source (rule in force: `?T` after a first `null`, no int→float widening)

| Source | Size | GRD021 static (compile time) | GRD021 at run time | ArithmeticError |
|---|---|---|---|---|
| Private reference implementation (not part of this repository; so far only its schema tooling) | 9 files | 0 | not executable | not executable |
| Laravel documentation (PHP blocks, R2) | 4 749 blocks, 3 941 parsable | 0 (193 assignments assessable, 2 223 not) | not executable (fragments) | not executable |
| Legacy harvest (188 PHP scripts, executed) | 188 | 2 | 0 further | 0 |
| Probes (240) and benchmarks c1–c3 | 243 | 2 probes (rewritten to conform to the dialect) | 2 (01_interface, c1: variable first an array, then `foreach` with objects) | 0 |

Typical examples:
- **Legacy harvest (measured):** `$n = 10; … $n /= 8;` (`compound_assignment_001.php:6`) and `$total = 100; … $total /= 4;` (`examples_operators.php:32`) — both a consequence of "`/` is always float" (D10); solution `intdiv()`. There is no third example in this corpus.
- **Probes/benchmarks (measured):** `$t = 0; foreach (…) { $t += $sh->area(); }` with float areas (`01_interface`, solution `$t = 0.0`); `$c = $item['customer']` (array) and later `foreach ($customers as $id => $c)` (objects) in c1 (solution: a separate variable); `var_dump(10 / 2)` yields `float(5)` instead of `int(5)` (`legacy_integer_arithmetic_002`, expectation adjusted).
- **Laravel documentation and private reference implementation:** no hits. Expected patterns once that application code is available (not measured): `$total = 0; foreach ($items as $i) { $total += $i->price; }` with float prices (GRD021 at run time, solution `0.0`); `$id = $request->input('id'); $id = (int) $id;` (string → int, solution a separate variable); hash computations such as `$h = $h * 31 + ord($c)` that silently overflow to float in PHP (ArithmeticError, solution `% m` in every step).

**Assessment:** the static check assesses only 8 % of the assignments in the Laravel documentation; the rest
(calls, array accesses, properties) is checked at run time and cannot be counted without executable code.
Reliable numbers will only come with PL1.

## Corpora

| Corpus | Sources | Syntax errors¹ | without dialect errors | Remark |
|---|---|---|---|---|
| Laravel documentation (PHP code blocks of its `.md` files, R2; fetched separately, not part of the repository) | 4 749 | 808 | 3 930 | blocks are often fragments (methods without a class, `...`) |
| Private reference implementation (not part of this repository; numbers only) | 9 | 0 | 7 | so far only its schema tooling delivered; controllers, models, routes still missing |
| Own files for comparison: legacy harvest, probes, benchmarks | 441 | 2 | 436 | dialect code, expectedly almost clean |

¹ Blocks the parser does not accept are not counted further.

## Hits per GRD code (hits / files)

| Code | Meaning | Laravel docs | Reference impl. | own |
|---|---|---|---|---|
| GRD002 | `global` | 0 | 0 | 1 / 1 |
| GRD006 | references (`&$x`) | 1 / 1 | 0 | 0 |
| GRD007 | heredoc/nowdoc | 1 / 1 | 0 | 0 |
| GRD008 | `static function`/`static fn` | 2 / 2 | 0 | 0 |
| GRD010 | filesystem in the dialect | 1 / 1 | 0 | 0 |
| GRD017 | inline HTML outside `<?php` | 4 / 4 | 0 | 0 |
| GRD020 | result must not be discarded (`sort($a);`) | 0 | 4 / 2 | 5 / 2 |
| no code | partial application (`f(?, x)`) | 2 | 0 | 0 |

## Appendix: count before D10 (both variants, as of 2026-10-24 night)

Counted are assignments to simple variables in source order per scope
(top level, function, method, closure, arrow function). Only those whose type follows from the
expression itself are assessed (literals, arithmetic of known numbers, concatenation, comparisons, casts, `new`,
array literals, already typed variables). `int → float` counts as a change (D9).

| Corpus | strict | tolerant (`?T`) | of which int↔float | assessed | not assessable |
|---|---|---|---|---|---|
| Laravel docs | 0 | 0 | 0 | 193 | 2 223 |
| Reference impl. | 0 | 0 | 0 | 19 | 42 |
| own | 2 (1 file + its probe) | 0 | 0 | 660 | 303 |

**Assessment:** the static count assesses only 8–68 % of the assignments; the majority comes from
calls, array accesses and properties whose type is not known without type inference across functions.
The numbers are therefore a lower bound. A reliable number will only come with the
delivery of the reference implementation's `app/**` or a run-time counting mode (count type changes on write instead of throwing).

## `ArithmeticError` on integer overflow

Not countable statically (depends on values). Measured by executing with `GRD_STRICT=1` against the
default: all 240 probes and the benchmarks c1–c3 — **0 deviations**, no overflow occurs.
The reference implementation's code cannot be executed yet.

## Open points

- Delivery of `app/**`, `routes/**` of the private reference implementation for a meaningful value.
- Run-time counting mode for GRD021 (count instead of throw) if the static lower bound is not enough.
