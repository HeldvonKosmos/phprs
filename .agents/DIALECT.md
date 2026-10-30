# OpenGrid Dialect (`.grd`)

Binding language and structure rules for `loco-php-core`, the cleaned-up fork
of phprs, and for applications that run on it. This document replaces
`SPEC.md` (upstream, `archiv/upstream/`) as the single source of truth. Whatever is not written here is not part of the
dialect.

Status: fork of `yingkitw/phprs` 0.1.19 (tag `upstream-baseline`).

## 1. Guiding idea

OpenGrid is not a PHP interpreter. It is a strict subset of PHP 8 that runs
as an embedded scripting language inside a Rust web framework (Loco/Axum).

- **Rust carries:** HTTP, routing, database, sessions, queues, mailer, auth,
  configuration, hot reload, data science (Polars/ndarray), all
  base classes.
- **The dialect carries:** application logic in controllers, models, middlewares,
  services and routes, written in the Laravel style.
- **Compatibility goal:** Laravel code should be portable with manageable
  effort. Zend PHP compatibility is not a goal. Composer packages
  do not run.

Two principles for every decision:

1. **Strict rather than tolerant.** Where PHP converts implicitly or silently
   carries on, the dialect throws an error. That makes code predictable and
   gives a later JIT clean assumptions.
2. **Convention rather than configuration.** Folder structure, namespaces and
   file extensions are fixed, not configurable.

## 2. Files and namespaces

- File extension: **`.grd`**, without exception. `.php` files are not loaded.
- The opening tag `<?php` remains allowed (editor support); `<?grd` is
  accepted as well. No closing tag, no inline HTML.
- Every file begins with `declare(strict_types=1);` or not at all; the
  compiler treats every file as if it were there.
- One file, one class (or one file with routes or configuration).
- **Autoloading by convention.** The Rust autoloader maps class names
  1:1 onto paths. For a module `billing`:

  | Class | File |
  |---|---|
  | `Billing\Controllers\InvoiceController` | `modules/billing/app/Controllers/InvoiceController.grd` |
  | `Billing\Models\Invoice` | `modules/billing/app/Models/Invoice.grd` |
  | `Billing\Middlewares\EnsureOwner` | `modules/billing/app/Middlewares/EnsureOwner.grd` |
  | `Billing\Services\TaxCalculator` | `modules/billing/app/Services/TaxCalculator.grd` |

  The first namespace level is the module name in StudlyCase, the second
  level is the folder under `app/`. Deviations are compiler errors.
- `include`, `require`, `include_once`, `require_once`: **removed.**
  Files are loaded exclusively through the autoloader.
- `spl_autoload_register`: removed, there is exactly one autoloader.

## 3. Project structure ("modules-only")

```
my-app/
├── config/                 # app.yaml, development.yaml, vite.config.ts, tailwind.config.js, .env
├── database/               # migrations and seeds (Loco / SeaORM / SQLx)
├── modules/                # the application
│   └── billing/
│       ├── module.json     # Manifest: name, version, routePrefix, dependencies
│       ├── Cargo.toml      # optional: the module's native Rust crate
│       ├── src/            # optional: #[php_class] classes in Rust
│       ├── app/
│       │   ├── Controllers/
│       │   ├── Models/
│       │   ├── Middlewares/
│       │   └── Services/
│       ├── routes/web.grd
│       └── resources/js/Pages/   # Inertia pages (Vue/React)
├── routes/web.grd          # global routes and fallbacks
├── storage/                # caches, logs, compiled module plugins (.so/.dylib)
├── public/                 # web root
├── Cargo.toml              # Workspace
└── phprs.lock              # lock file of the module manager
```

Rules:

- The root contains only `Cargo.toml`, `phprs.lock`, `package.json` (Node tooling
  expects it there) and `.gitignore`. All build configuration and
  environment variables live in `config/`.
- `module.json` is mandatory. Without a manifest a module is not loaded.
- A module may ship a Rust crate. In dev mode it is compiled as a
  dynamic library into `storage/plugins/` and loaded via
  `libloading`; in prod mode it is linked statically into the workspace.
  The module manager (`phprs pkg`) handles both.
- The module manager loads modules from Git repositories. Composer/Packagist
  are not supported.

## 4. Language scope

### 4.1 Kept

| Area | Scope |
|---|---|
| Types | `int`, `float`, `string`, `bool`, `null`, `array`, objects, enums, closures |
| OOP | classes, abstract classes, interfaces, traits, enums (pure and backed), anonymous classes, `readonly`, constructor promotion, static members, late static binding, class constants, `final` |
| Magic methods | `__construct`, `__get`, `__set`, `__isset`, `__unset`, `__call`, `__callStatic`, `__toString`, `__invoke` (for facades, models, container) |
| Functions | named functions, closures, arrow functions, first-class callables, named arguments, variadics, default values, by-reference parameters |
| Type syntax | scalar, class, union, intersection, nullable types, `mixed`, `void`, `never`, `static`, `self` |
| Control flow | `if`, `match`, `switch`, `for`, `foreach` (value and key), `while`, `do-while`, `break`/`continue` with level, `return` |
| Errors | `try`/`catch`/`finally`, exception hierarchy, `throw` as an expression |
| Generators | `yield`, `yield from` |
| Attributes | `#[...]` with reflection access (for routes, validation, DI) |
| Reflection | `ReflectionClass`, `ReflectionMethod`, `ReflectionProperty`, `ReflectionFunction`, `ReflectionParameter`, `ReflectionNamedType`, reading attributes |
| Regex | `preg_match`, `preg_match_all`, `preg_replace`, `preg_replace_callback`, `preg_split`, `preg_quote` (Rust `regex`/`fancy-regex`, not full PCRE; document backreferences) |
| Builtins | string, array, math, JSON, `mb_*`, datetime, hash/random, `serialize`/`unserialize` for plain data, `sprintf` family, `var_dump`/`print_r` (debug) |
| Operators | arithmetic, logical, bitwise, `??`, `?->`, `...`, spaceship, `instanceof`, string interpolation |

### 4.2 Removed (compiler error with hint)

| Construct | Reason | Replacement |
|---|---|---|
| `include`/`require`/`*_once` | autoloader enforces structure | namespace + file |
| `eval` | security, JIT | none |
| `global`, `$GLOBALS` | no shared state | container/DI |
| variable variables `$$x`, `${'x'}` | not statically analysable | arrays |
| `extract`, `compact` | implicit variables | explicit arrays |
| `goto` | | |
| `array()` syntax | one notation | `[]` |
| `list()` | | `[$a, $b] = ...` |
| superglobals `$_GET`, `$_POST`, `$_SERVER`, `$_SESSION`, `$_COOKIE`, `$_FILES`, `$_ENV` | the request is an object | `Request`, `Session`, `Cookie` (native classes) |
| `echo`/`print` outside a debug context, output buffering (`ob_*`) | the response is a return value | `return response(...)` |
| `exit`/`die` | the process belongs to Rust | exception or response |
| `define()`, global constants | | class constants, `config()` |
| `set_error_handler`, `register_shutdown_function` | the lifecycle belongs to Rust | middlewares |
| filesystem builtins (`fopen`, `file_put_contents`, `fgetcsv`, ...) | Rust service | native `Storage` class |
| network (`file_get_contents('http://..')`, `curl_*`, sockets) | Rust service | native `Http` class |
| PDO, `mysqli`, `sqlite3` | Rust service | native `DB`/`Model` |
| `session_*` | Rust service | native `Session` |
| `openssl_*`, `gz*` | Rust service | native classes as needed |
| SimpleXML, DOM | | native module later |
| `ini_set`/`ini_get`, `phpinfo`, `phpversion` | no INI | `config()` |
| ticks, `declare(ticks)`, reference returns `function &f()` | | |

### 4.3 Frozen (code stays, is isolated, not extended)

- **JIT scaffolding:** `engine/jit.rs`, `engine/function_optimizer.rs`,
  `engine/opcode_cache.rs`, `engine/perf_alloc.rs`, `engine/string_intern.rs`,
  `engine/benchmark.rs`. These go behind a Cargo feature `experimental-jit`
  that is off by default. Cranelift is milestone M7 and starts
  with a measurement, not with this code.
- **Fibers:** the opcode and the `Fiber` class stay, feature `fibers`,
  off by default. Asynchrony is handled by the Rust host.

## 5. Semantic deviations from PHP

These points are deliberately different from PHP and must be taken into account
when porting Laravel code.

### 5.1 Strict comparisons

There is exactly one equality. `switch`, `match`, `in_array`, `array_search`,
`array_keys($a, $v)`, `array_unique` compare strictly. Objects compare
by identity; native value types (`DateTime`, `Decimal`, `RecordId`)
register a compare handler so that `<`, `>`, `==`, `<=>` work on them
(NATIVE_CLASSES §6).

- `==` and `!=` behave like `===` and `!==`. There is no type
  juggling. `0 == "a"`, `"1" == "01"`, `null == false` are all `false`.
- `<`, `>`, `<=`, `>=`, `<=>` are defined only between two numbers (`int`/`float`
  mixed is allowed) or two strings (bytewise). Other
  combinations throw `TypeError`.
- `switch` and `match` compare strictly (for `match` this is already PHP
  behaviour; `switch` follows suit).
- `in_array`, `array_search`, `array_keys($a, $v)` always work strictly; the
  `$strict` parameter is accepted and ignored.
- The same holds in `OpenGrid\Support`: the non-strict variants
  (`Collection::contains`, `duplicates`, `after`/`before`, `whereNotIn`,
  `Arr::exceptValues`/`onlyValues` …) compare strictly like their
  `*Strict` counterparts. `Arr::push` returns the new array, `Arr::pull`,
  like `Arr::pop`/`shift`, returns the pair `[value, rest]` (no references).
- Implementation: `IsEqual`/`IsNotEqual` compile to the same handlers
  as `IsIdentical`/`IsNotIdentical`; `zval_compare` in
  `engine/operators.rs` is reduced to the permitted type pairs.

### 5.2 Strict types everywhere

- `strict_types=1` cannot be switched off: scalar parameters are not
  coerced, `int → float` remains the only permitted widening (PHP behaviour
  under strict_types).
- Arithmetic with non-numeric strings throws `TypeError` instead of a warning.
- String-to-number conversion only explicitly (`(int)`, `intval`, `is_numeric`).
- **Local variables keep their type** (GRD021; D8, D10, enforced): a
  local variable has the type of its first assignment; assigning a
  different type is a compile error. If the first assignment is `null`, the
  type is `?T` with the type of the first non-null assignment; a further
  different type is GRD021, as is `null` into a variable that is not `?T`.
  The widening `int → float` does **not** apply to local variables (the message
  mentions `0.0` or `(float)`); for parameters it remains. Types: null, bool,
  int, float, string, array, object (classes are not distinguished).
  The compiler checks where both types follow from the source; otherwise
  the VM checks on write and throws `TypeError` with "GRD021". If the type of the
  first non-null assignment is statically unknown (array access, call,
  untyped parameter), the variable is `mixed` and is not checked
  (DECISIONS 2026-10-24, 22:20). Fixed in the suite: `types.rs`
  (`locals_keep_their_type`, TYP1). `foreach` and
  `catch` variables are rebound on every iteration and are exempt.
  Reason: the JIT knows every type statically and needs no type guards and
  no deoptimisation.
- **`/` always yields `float`** (D10): `10 / 2` is `5.0`, `var_dump` shows
  `float(5)`. Integer division with `intdiv()`. Reason: the type must not
  depend on the value.
- **Integer overflow throws `ArithmeticError`** (D8, D10, enforced) instead of
  switching to `float` like PHP 8.4. Applies to `+`, `-`, `*`, `**`, unary `-`,
  `++`, `--` and compound assignments, identically in interpreter and JIT.
- **`%` and bit operations only with int** [final] (D20, DIA2): `%`, `&`, `|`,
  `^`, `<<`, `>>`, `~` and their compound assignments throw `TypeError` on a
  float operand ("Unsupported operand types: float % int; use
  fmod() or (int)", for bit operations "use (int)"). PHP 8.4 silently truncates
  (with `Deprecated` for fractional parts) and wraps values that are too large. Remainder
  of floats with `fmod()`, otherwise an explicit `(int)`.
- **`abs(PHP_INT_MIN)` throws `ArithmeticError`** [final] (D20, DIA2) like every
  other integer overflow; PHP returns `float(9.2233720368547758E+18)`.
- **`++`/`--` only for int and float** [final] (D20, DIA2): on `null` (PHP:
  `null++` yields `1`, `null--` stays `null` with a warning), strings (PHP:
  `"a"++` yields `"b"`, `"5"++` yields `6`), bool, arrays and objects
  `TypeError` ("Cannot increment null").

### 5.3 Errors are exceptions

- There are no warnings and notices. Everything that would be a warning in PHP
  (undefined array key, undefined variable, division by zero,
  wrong argument count) throws an exception from a fixed hierarchy
  (`Error`, `TypeError`, `ValueError`, `ArgumentCountError`,
  `DivisionByZeroError`, `UndefinedKeyError`, `UndefinedVariableError`).
- Reading undefined array keys: exception. Whoever wants `null` uses `??`.
- `unserialize` with broken input throws `ValueError` ("unserialize(): Error at offset N of M bytes", offset
  approximately as in PHP) where PHP warns and returns `false`; an unknown class throws likewise (PHP:
  `__PHP_Incomplete_Class`). Empty input returns `false` as in PHP (TEST2.2d).

### 5.4 No global side effects

- No output functions in the request path: a controller **returns**
  (string, array, `Response`, `InertiaResponse`, `Redirect`). The Rust side
  converts.
- No process-wide state in the script: static properties are per
  VM instance (that is, per worker) and must not be misused as a cache.
  For caching there is the native `Cache` class.
- Static properties survive the request; an object value (or an array containing an object) in a static property is a runtime error (`Error`). Exception: enum cases, they are immutable engine singletons.

### 5.5 Arrays

- Arrays remain ordered hash maps with PHP semantics (copy-on-write,
  int/string keys). This is deliberate, because Laravel code relies on it
  heavily.
- The conversion `array ↔ serde_json::Value` is the only bridge to
  Rust: lists (keys 0..n-1 without gaps) become JSON arrays, everything else
  JSON objects. There are no `null` keys.

### 5.6 No references

The dialect has no references: no `&$param` in user functions,
no `foreach ($a as &$v)`, no `use (&$x)`, no reference returns
(compile error GRD006). Values are values, objects are handles; hence there
is no aliasing a JIT would have to respect. PHP's by-ref builtins
are only the visible part of this rule: they return their result
instead of modifying the argument. Discarding that result is a
compile error (GRD020).

| PHP signature | Dialect signature |
|---|---|
| `sort(array &$a): bool` (likewise `rsort`, `usort`, `asort`, `arsort`, `uasort`, `ksort`, `krsort`, `uksort`, `natsort`, `natcasesort`, `shuffle`) | `sort(array $a): array` — `$a = sort($a);` |
| `array_push(array &$a, mixed ...$v): int` | `array_push(array $a, mixed ...$v): array` |
| `array_unshift(array &$a, mixed ...$v): int` | `array_unshift(array $a, mixed ...$v): array` |
| `array_splice(array &$a, int $o, ?int $l, mixed $r): array` | `array_splice(array $a, int $o, ?int $l = null, mixed $r = []): array` (the new array) |
| `array_pop(array &$a): mixed`, `array_shift(array &$a): mixed` | **removed (GRD019)** — `[$value, $rest] = Arr::pop($a);`, `Arr::shift($a)` |
| `preg_match(string $p, string $s, array &$m = null, …): int\|false` | `preg_match(string $p, string $s, int $flags = 0, int $offset = 0): array` — empty on no match, so `if (preg_match(...))` works as usual |
| `preg_match_all(…, array &$m, …): int\|false` | `preg_match_all(string $p, string $s, int $flags = PREG_PATTERN_ORDER): array` |
| `parse_str(string $s, array &$result): void` | `parse_str(string $s): array` |
| `array_walk(array &$a, callable $f)` | `array_walk(array $a, callable $f): bool` — the callback cannot modify `$a`; use `array_map` |
| `settype`, `array_multisort`, `extract`, `compact` | removed |

## 6. Native base classes (Rust, via the registry)

The dialect takes these classes as given. They are defined in Rust
and entered into the `class_table` at VM start. Script classes may inherit from
them where marked.

| Class | Inheritable | Purpose |
|---|---|---|
| `OpenGrid\Http\Request` | no | method, path, query, body, headers, cookies, files, `input()`, `validate()` |
| `OpenGrid\Http\Response`, `Redirect`, `JsonResponse` | no | return types |
| `OpenGrid\Http\Controller` | **yes** | base for controllers, middleware registration |
| `OpenGrid\Http\Middleware` | **yes** | `handle(Request $r, Closure $next)` |
| `OpenGrid\Database\Surreal` | no | connection, `query()`, `transaction(Closure)` (SurrealDB, `DATABASE_LAYER.md`) |
| `OpenGrid\Database\Builder` | no, but mutable from closures | the query builder: `where`, `filter`, `sort`, `fetch`, `select`, `groupBy`, `toSql` |
| `OpenGrid\Database\Model` | **yes** | the model base class: declaration via `$table`/`$fillable`/`$dates`/`$softDeletes`, `find`, `where`, `filter`, `paginate`, `create`, `save`, `delete`, `relate`, `toArray` |
| `OpenGrid\Database\{RecordId, Datetime, Duration, Decimal, Uuid, Geometry, Bytes}` | no | type classes with native payload (`RUNTIME_ARCHITECTURE.md` §8.3) |
| `OpenGrid\Inertia\Inertia` | no | `render()`, `share()`, `location()` |
| `OpenGrid\Routing\Route` | no | `get()`, `post()`, `resource()`, `middleware()`, `prefix()`, `group()` |
| `OpenGrid\Support\Session`, `Cache`, `Config`, `Log`, `Storage`, `Http` | no | Rust services |
| `OpenGrid\Data\DataFrame`, `Series` | no | Polars binding (M5) |

Facades (`Route::get(...)`, `DB::table(...)`) are static methods of these
classes, not a proxy mechanism. **There is no service container and no
service provider** (DECISIONS D1, 2026-10-24): the module structure is convention.
Constructor parameters are resolved by Rust through the type hints; concrete classes are
instantiated recursively, an interface with exactly one implementation in the project
is bound automatically, none or several is a link error (its own
GRD code). Instances live per request, never process-wide. Middleware is given as a
class name in the routes; aliases exist only for framework middleware (`csrf`,
later `auth`).

## 7. Compiler phase "dialect check"

Between parser and codegen runs a check that rejects every violation of
section 4.2 with file, line and hint:

```
error[GRD001]: `include` is not allowed in the OpenGrid dialect
  --> modules/billing/app/Controllers/InvoiceController.grd:3:1
   = hint: classes are loaded automatically by namespace.
```

Error codes `GRD0xx` are maintained in `docs/errors.md`. The check cannot
be switched off.

## 8. Conformance and tests

- Every language feature from 4.1 has at least one test under
  `tests/dialect/`. Tests for removed constructs check that the correct
  `GRD` error is raised.
- `examples/test-app/` is the reference application. It is built on every
  commit and is the acceptance criterion for every milestone
  (see `archiv/INVENTORY.md`, section 8).
- `.phpt` tests from php-src can serve as templates but are adapted to the
  dialect, not taken over 1:1.

## 9. Migration list: Laravel → dialect

What has to be changed mechanically in existing Laravel code when moving to OpenGrid
(as of 2026-10-24; grows with every decision):

| Today (Laravel/PHP) | In the dialect | Reason |
|---|---|---|
| `Route::get('/x', fn() => …)` | controller method: `Route::get('/x', [XController::class, 'show'])` | closures are objects and die with the request in which `routes/*.grd` ran (DECISIONS 2026-10-23, framework crate). `Route::group(…, fn)` stays, because the closure runs at startup. |
| `&$param`, `foreach (… as &$v)`, `use (&$x)` | return values (§5.6 table) | no references (GRD006) |
| `sort($a); array_push($a, …)` | `$a = sort($a); $a = array_push($a, …)` | the result must not be discarded (GRD020) |
| `array_pop($a)`, `array_shift($a)` | `[$v, $a] = Arr::pop($a)` | GRD019 |
| `$a == $b` with type conversion, loose `in_array` | strict (`==` ≙ `===`), explicit casts | §5.1 |
| `"5" + 3`, `"abc" . 1` | `(int) "5" + 3`; string concatenation stays | arithmetic only int/float (§5.2) |
| `include`, `global`, `eval`, `list()`, heredoc, `static fn`, superglobals, `compact/extract`, `array()` | §4.2 | GRD001–GRD012 |
| `$n = PHP_INT_MAX + 1` (becomes `float`) | check for overflow or cast `(float)` beforehand: `(float) PHP_INT_MAX + 1` | overflow throws `ArithmeticError` (§5.2) |
| `$x = 0; … $x = 1.5;`, `$s = ''; … $s = 0;` | start with the same type: `$x = 0.0;` or `(float)`; otherwise a separate variable | a local variable keeps its type (GRD021, §5.2) |
| `$n /= 2`, `$avg = $sum / $count` used further as int | `intdiv($n, 2)` or compute with float | `/` always yields float (§5.2) |
| `app()->bind(Contract::class, Impl::class)`, service providers | constructor injection: an interface with exactly one implementation is bound automatically; with several, inject the concrete class | no registration (D1); ambiguous = GRD022 |
| objects in static properties (containers, singletons) | enum cases allowed; otherwise request state in the `WorkerCtx`/request object | statics survive the request (§5.4, DECISIONS 2026-10-24) |
| `app('surrealdb')`, `app(Foo::class)`, `resolve()`, service providers (`register`/`boot`), `bind`/`singleton` | `DB::…`; constructor injection; routes in `modules/<m>/routes/*.grd`; helpers `base_path()`, `env()`, `config()` (to be checked) | no container, no providers (DECISIONS D1) |
| project middleware aliases (`$middlewareAliases`) | class name in the route | aliases only for framework middleware (D1) |
| `Model::$casts`, SeaORM/Eloquent relations | `$dates`, `$filterable`, `withAll/withCount/withSum`, record links | DATABASE_LAYER |

## 10. Open points

To be decided as soon as the test app shows the need:

- Generics-like collections (`Collection<Model>`)? Untyped for now.
- Keep or remove reference parameters (`&$x`)? Kept for now,
  because `preg_match` and `array_walk` need them.
- `declare(strict_types=1)` line mandatory or optional? Optional for now
  (it is implied).
- Handling of `float` keys in arrays (PHP truncates to `int`): error
  for now.
