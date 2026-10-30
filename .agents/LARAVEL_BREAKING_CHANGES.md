# Breaking changes compared to Laravel

**Purpose (D13):** OpenGrid has the look and feel of Laravel but is not fully compatible. This
document lists every deliberate deviation so that Laravel developers know what is different and ports
can be planned. Comparison base: Laravel 13.38 (LV1). **Maintenance rule:** every work package that
replicates, changes or deliberately omits a Laravel API records it here (column "since" = commit or package ID).

Legend: **dropped** = does not exist and will not come · **different** = exists with a different meaning or
syntax · **missing** = not built yet, planned (package ID) · **additional** = exists only in OpenGrid.

## 1. Language (details: DIALECT.md)

| Laravel/PHP | OpenGrid | Kind | Reason |
|---|---|---|---|
| `<?php`, `declare(strict_types=1)` optional | `<?grd`, strict types always on | different | DIALECT §5.2 |
| type coercion for scalars (without strict_types) | never; only `int → float` on parameters | different | §5.2 |
| a local variable changes its type | GRD021: a local variable keeps the type of its first assignment | different | §5.2, JIT without guards |
| `10 / 2` is `int(5)` | always `float` (`5.0`); integer division with `intdiv()` | different | D10 |
| integer overflow becomes `float` | `ArithmeticError` | different | D8/D10 |
| `%`, `&`, `\|`, `^`, `<<`, `>>`, `~` silently truncate float operands | `TypeError`; `fmod()` or `(int)` explicitly | different | §5.2, D20 (DIA2) |
| `abs(PHP_INT_MIN)` is `float` | `ArithmeticError` | different | §5.2, D20 (DIA2) |
| `null++` yields `1`, `"a"++` yields `"b"`, `"5"++` yields `6` | `++`/`--` only for int/float, otherwise `TypeError` | different | §5.2, D20 (DIA2) |
| `==` with type juggling, `in_array`/`array_search` loose | exactly one equality: strict; the `$strict` parameter is ignored | different | §5.1 |
| numeric strings and `null` in arithmetic | `TypeError`; explicit conversion (`(int)`) | different | §5.2 |
| warnings/notices (undefined key, undefined property …) | exceptions (`UndefinedKeyError` …); `??` for default values | different | §5.3 |
| references (`&$x`, `function &f()`, `use (&$x)`, `foreach (... as &$v)`) | dropped | dropped | §4.2 |
| `include`/`require`, `eval`, `global`, `$$x`, `extract`/`compact`, `goto`, `array()`, `list()` | dropped | dropped | §4.2 |
| superglobals, `exit`/`die`, `echo` as the response, `ob_*` | `Request`, `Session`, exceptions, return value | dropped | §4.2 |
| `define()`, global constants | class constants, `config()` | dropped | §4.2 |
| filesystem, network, PDO, `session_*`, `gz*`, `openssl_*` builtins | native classes (`Storage`, `DB`, `Session` …) | dropped | §4.2 (GRD010) |
| dynamic properties | `Error` | dropped | §4 |
| objects in static properties | runtime error | dropped | DECISIONS 2026-10-24 |
| `Class::$name()` (dynamic static method name) | dropped | dropped | DIALECT |
| `func_get_args()` sees extra arguments without a variadic parameter | only declared parameters | different | a6503f2 |
| objects live until the last reference | objects live at most until the end of the request (arena, collected within the request, MEM1b); across requests only serialised via the session cache (SES1); destructors (`__destruct`) do not run | different | D13, 45c9f6f |
| unlimited number of objects (up to `memory_limit`) | at most 1 million live objects per request (`opengrid.limits.objects`), then `Error` | different | 84248ac |
| internal functions coerce scalars (without strict_types) | strict as with `strict_types=1`, including the `implode` separator and bool options | different | TYP3 |
| `sscanf($s, $f, &$a, &$b)` | `sscanf($s, $f)` returns the values as an array | different | BI1 |
| `WeakMap` releases entries as soon as the object dies | entries live until the end of the request | different | BI1 |

## 2. Project structure, container, configuration

| Laravel | OpenGrid | Kind |
|---|---|---|
| `app/`, `routes/`, `config/*.php`, Composer autoload | modules-only: `modules/<m>/app/**`, `modules/<m>/routes/*.grd`; one class per file, name derived from the path (DIALECT) | different |
| service container, service providers, `app()->bind()` | dropped; constructor injection by convention, interfaces via naming convention (I1, D1) | dropped |
| `config/*.php`, `config('app.name')` | Loco YAML (`config/<env>.yaml`, section `settings`), `config('app.name')` reads from it | different |
| Composer packages | modules via the module manager (PKG1) | different / missing |
| Artisan | `opengrid` (Loco CLI): `start`, `task …`; tasks run in the server process (D5) | different |

## 3. Routing, controllers, middleware

| Laravel | OpenGrid | Kind | since |
|---|---|---|---|
| aliases for arbitrary middleware (`Kernel`/`bootstrap/app.php`) | middleware as a class name in the route; aliases only for the framework (`auth`, `csrf`, `throttle`); `Route::aliasMiddleware` deprecated | different | MW1 |
| middleware parameters `throttle:60,1` | same (strings after `$next`) | same | 44a0073 |
| `Route::resource`, `Route::apiResource` | missing | missing | R2 |
| views, Blade, Inertia | dropped (controllers answer with JSON or text) | dropped | plan |
| controller returning array/model | JSON; `null` → 204 or output | same | F1 |

## 4. Request, validation, responses

| Laravel | OpenGrid | Kind | since |
|---|---|---|---|
| `$request->ip()` with TrustProxies | last entry of `X-Forwarded-For` (Traefik), otherwise the peer address; header via `settings.opengrid.proxy.ip_header` (`false` = off); no TrustProxies, no list of trusted proxies | different | c7c0863, D13 |
| `$request->validate()` | same, rules as a subset (since TEST2.7 with wildcards `items.*.x` and the `required_if` family), messages like Laravel (specification of 167 cases, re-checked against Laravel on every run) | same | R3, TEST2.7 |
| `Validator::make()`, form requests | missing | missing | R2 |
| error response | `HttpException` → JSON `{message}`; `ValidationException` → 422 `{message, errors}` | same | F2 |

## 5. Database, Eloquent

| Laravel | OpenGrid | Kind | since |
|---|---|---|---|
| SQL databases, PDO | SurrealDB (embedded `surrealkv`, optionally a server) | different | D2 |
| migrations (`up`/`down`) | declarative target schema `modules/<m>/database/schema/schema.surql`, SurrealKit `sync`/`rollout`; rollouts only extending | dropped / different | R4 |
| Eloquent relations (`hasMany`, `belongsTo`, `belongsToMany`, `with()`, `load()`) | **dropped**; record links (`user` as `RecordId`), `relate()`/`related()` for graph edges, `withCount`/`withSum`/`withAvg`/`withMin`/`withMax`/`withAll` as sub-selects | dropped | D13 |
| primary key `id` as a number | record ID `table:id` (`RecordId`) | different | B1 |
| joins (`join`, `leftJoin`) | dropped (SurrealDB: record links, `FETCH`) | dropped | D13 |
| `DB::select('SELECT … FROM …')` | SurrealQL instead of SQL | different | DBL1 |
| `where`, `orWhere`, `where(fn ($q) => …)`, `whereNot`, `whereBetween`/`NotBetween`, `whereIn`/`NotIn`, `whereNull`/`NotNull`, `whereColumn`, `orderBy`/`orderByDesc`, `latest`/`oldest`, `take`/`skip`, `pluck`, `value`, `sum`/`avg`/`min`/`max`, `increment`/`decrement`, `chunk`, `firstOrCreate`, `updateOrCreate`, `doesntExist` | same; every builder method also statically on the model (`__callStatic`) | same | Q1 |
| `where($col, $op, $value)` | same; additionally the argument order `where($col, $value, $op)` of a private reference implementation (recognised by the operator) | additional | Q1 |
| arbitrary operators (`'like'`, `'ilike'`, database-specific) | fixed list: `= != <> < <= > >= like` (case-insensitive, as a regex) `not like contains containsnot containsany containsall in not in`; field names only as an identifier path (`a.b_c`); otherwise `ValueError` | different | Q1 (security) |
| `get()`, `pluck()` return a `Collection` | return an array (`collect(...)` if needed) | different | DBL1 |
| `chunk($n, fn ($rows, $page))` with a Collection | with an array | different | Q1 |
| query from the query string | additional: `filter()`/`sort()` without arguments read `?filters[...]`/`?sort` | additional | DBL1 |
| `DB::transaction($fn, $attempts)` | same (also `$attempts < 1`: the closure does not run, `null`); retries only on write conflicts; `$attempts` must be `int` (strict types, Laravel also accepts `'2'`) | same | TX1, TEST2.2c |
| soft deletes, `$fillable`, `$hidden`, timestamps | same (`protected array`, `protected bool $timestamps`) | same | DBL1 |

## 6. Authentication

| Laravel | OpenGrid | Kind | since |
|---|---|---|---|
| Sanctum expiry (`sanctum.expiration`, `createToken(…, expiresAt:)`) | `auth.token_expiration` (minutes, default 0 = never) and `Auth::createToken($user, $name, $expiresAt)`; check like Sanctum's `isValidAccessToken` | same | TEST2.7 |
| Sanctum writes `last_used_at` on every request | at most every `auth.token_touch_seconds` seconds (default 60; `0` = on every request like Sanctum); read requests then write nothing (DBP1: show on disk 4.2× faster) | different | DBP1 |
| guards (session/token), Sanctum | only bearer tokens following the Sanctum pattern (stored as SHA-256), middleware `auth` | different | AU1 |
| `Auth::attempt($credentials): bool` (logs in) | `Auth::attempt($email, $password): ?Model` (returns the user, does not log in); token with `Auth::createToken($user)` instead of `$user->createToken()` | different | AU1 |
| login throttle (Breeze: `RateLimiter` in the form request, **422** with `auth.throttle`) | built into `Auth::attempt`: **429** with `Retry-After` after 5 failed attempts per e-mail+IP, without configuration | different | AU2 |
| `RateLimiter` facade | same (`hit`, `attempts`, `tooManyAttempts`, `remaining`, `availableIn`, `clear`, `attempt`); counters in the process memory | same | AU2 |
| password reset, e-mail verification, policies/gates | missing | missing | — |

## 7. Support (Str, Arr, Collection)

| Laravel | OpenGrid | Kind | since |
|---|---|---|---|
| `Illuminate\Support\*` | `OpenGrid\Support\*`, behaviour tested against Laravel's classes (607 cases) | same | S1, S1b |
| non-strict variants (`contains`, `duplicates`, `after`, `whereNotIn`, `Arr::exceptValues` …) | compare strictly like the `*Strict` variants | different | fdc16ff |
| `Arr::set/forget/push(&$array, …)` | return the new array | different | S1 |
| `Arr::pull(&$array, $key)`, `array_pop`/`array_shift` | `Arr::pull/pop/shift` return `[value, rest]` | different | S1, fdc16ff |
| `LazyCollection::times()` without a count | count required (as in Laravel 13) | same | 860c133 |
| accumulators with a changing type (`$carry = $obj; $carry = $cb($carry)`) | GRD021; use `array_reduce` or closure parameters | different | KNOWN_ISSUES |

## 8. Sessions, mail, storage, jobs, scheduler, cache

| Laravel | OpenGrid | Kind | since |
|---|---|---|---|
| session drivers file/redis/database | memory or SurrealDB; cookie `opengrid_session`; CSRF via `X-CSRF-Token` | different | F3 |
| objects in the session (`session()->put('cart', $cart)`, PHP serialises implicitly) | `Session::put` stores JSON (objects become arrays); objects only via `Session::remember($key, fn () => new Cart())`: stored serialised, rebuilt per request, changes written back at the end of the request | different | 6b5ab6f |
| `Mailable`, `Mail::to()->send()` | `OpenGrid\Mail\{Mailable, Mail}`; `build()` with `subject/view/with/html/text`; **templates in Tera instead of Blade** (`modules/<m>/resources/mail/<name>/{subject,html,text}.t`); `envelope()`/`content()` style, attachments, Markdown mails, queueing are missing; without SMTP only log + `Mail::sent()` (instead of `Mail::fake()`) | different | 98b7812 |
| `Storage::disk()` | `OpenGrid\Storage\Storage` with `put/get/exists/missing/delete/copy/move/size/files/allFiles`; drivers `local`, `memory` (S3 and friends via Loco features later); paths only relative without `..`; `url()`, streams, visibility are missing; the only file access | different | 98b7812 |
| queues with drivers, `ShouldQueue`, `dispatch()`, model serialisation | `OpenGrid\Queue\Job::dispatch(...$args)`/`dispatchSync`, `$tries`; runs on the server's workers, no choice of driver; arguments only JSON values (pass models as IDs); no `delay`, no batches/chains, `failed()` hook missing; not durable until J4 | different | 2ceb19a |
| database queue (`jobs`, `failed_jobs`, `queue:failed`/`queue:retry`/`queue:flush`, `dispatch()->delay()`) | same table names in SurrealDB, but fields instead of a JSON `payload` (`class`, `args`, `max_tries`, `backoff`, `queue`); `Job::dispatchLater($seconds, …)` instead of `->delay()`; tasks `queue_failed`, `queue_retry id:<uuid>\|all`, `queue_flush`; no `queue:work`: the server works off all queues itself (D5), task processes only write rows; without a database jobs stay in memory | different | 2886d27 |
| Artisan commands (`$signature`, `php artisan x:y --opt`) | `OpenGrid\Console\Task` with `handle(array $args): int`, found by convention (`opengrid task run task:notes:prune days:30` → `Notes\Tasks\Prune`), arguments `key:value` as strings; runs in the running server (control socket); no interactive input/output, no `$signature` parser | different | df09507 |
| scheduler (`routes/console.php`, `Schedule::command`/`call`/`job`) | `OpenGrid\Scheduling\Schedule::task($name, $args)` and `::job($class, $args)` in any route file (e.g. `routes/console.grd`), frequencies as in Laravel, times in UTC; no `call(fn)`, no `withoutOverlapping`/`onOneServer`, no time zones | different | 2bdda1f |
| Composer packages, `composer require`, service-provider auto-discovery | modules from Git: `opengrid task module cmd:add url:<repo> rev:<tag>`, mandatory `module.toml` (name, version, `depends`), pinned in `modules.lock`; GRD check of the whole project after installation; no registry, no version ranges, no transitive resolution (missing dependency = error) | different | 58ecc12 |
| `Cache` facade, drivers | `Cache` with an in-memory store per process (Loco cache) | missing | D13 |
| events, listeners, broadcasting, notifications | missing, not planned | dropped | — |
