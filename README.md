# OpenGrid

**A framework-integrated programming language that brings Laravel's developer
experience to native performance.**

OpenGrid is a web framework with a Rust core (Loco, Axum) whose application
layer is written in **grd**, a strict dialect of PHP. Controllers, models,
routes, middleware, validation, facades: you write them the way you would in
Laravel. They run on an embedded virtual machine inside one Rust process,
next to native Rust classes for everything performance-critical: HTTP,
routing, the database layer, sessions, queues, mail, auth.

> **Status: experimental.** OpenGrid is a research project. The language,
> the APIs and the file layout still change without notice, and there has
> been no security review. **Do not use it in production.** What it is good
> for today: trying the idea, measuring it, and helping to build it.

## The idea in one paragraph

PHP frameworks are loved for their developer experience and criticized for
their speed; systems languages are the other way round. OpenGrid keeps the
PHP syntax and the Laravel conventions, removes the parts of PHP that make
it slow and unsafe to optimize (dynamic typing of variables, `include`,
superglobals, Composer autoloading), and runs the result on a VM and a
Cranelift JIT that are compiled into the same binary as the framework. The
framework itself is Rust; the dialect only calls into it. The thesis is
that this gives Laravel-style code the throughput, memory footprint and
cold start of a compiled service, and the benchmark below is how we test
that thesis.

## Goals, and what is deliberately not a goal

**OpenGrid does not aim to be PHP-compatible.** The goal is to keep what
developers love about PHP and Laravel, and to drop what has accumulated
around it over thirty years.

What stays:

- **The developer experience of Laravel.** Routes, controllers,
  middleware, models with filters and pagination, validation with
  Laravel's rules and messages, facades, jobs, scheduler, mail, sessions,
  auth: the same names, the same flow, the same mental model. If you can
  read Laravel, you can read OpenGrid.
- **PHP's syntax** and its way of thinking in requests: `$variables`,
  arrays, classes, namespaces, closures, `foreach`, string interpolation.
- **Instant feedback.** Edit a file, the next request runs it; no build
  step in development.

What is left behind on purpose (the full list with error codes is in
`.agents/DIALECT.md` §4; every deviation from Laravel is recorded in
`.agents/LARAVEL_BREAKING_CHANGES.md`):

- **Dynamic typing of variables.** A variable keeps the type of its first
  assignment, `/` always yields a float, integer overflow throws. This is
  what makes the code analysable, lets the JIT compile without
  speculation, and catches a class of mistakes that AI-generated PHP
  produces in quantity.
- **`include`/`require`, Composer autoloading, superglobals, references,
  `eval`, `exit`, `goto`, `global`, static locals, variable variables,
  heredoc, inline HTML.** A project is a set of modules resolved by
  convention; the request is an object; output is a return value.
- **Functions that modify their arguments** (`sort`, `array_pop`, …) now
  return their result.
- **The standard library's inconsistencies.** The dialect's builtins are
  a curated, consistently named subset; `Str`, `Arr` and `Collection` are
  tested against Laravel's behaviour, not against PHP's quirks.
- **The service container and service providers.** Convention instead of
  registration code.

What is not a goal, at least for now: running existing PHP code
unchanged, a package registry compatible with Composer, or matching PHP
feature for feature. Porting a Laravel application to OpenGrid is meant
to be a mechanical, module-by-module exercise with the compiler pointing
at every line that relies on something left behind; it is not meant to be
a copy.

## Where this comes from

PHP started in 1995 as a set of tools for personal home pages: a script per
page, run from scratch for every request, forgotten when the response is
sent. That request-scoped model made PHP easy to deploy and easy to learn,
and it is still the model today, thirty years and several engine
generations later (PHP 3 and the Zend Engine in 1998, objects in PHP 5,
the twice-as-fast PHP 7 in 2015, the tracing JIT of PHP 8 in 2020). Web
frameworks grew on top of it, Laravel since 2011 the most popular of them,
and every one of them pays the same bill on every request: boot the
runtime, parse the framework, bind the container, resolve the pipeline,
and only then run the business logic. Application servers such as Octane,
RoadRunner and FrankenPHP keep the booted application in memory between
requests, which removes the bootstrap but not the interpreter underneath;
HHVM and Hack showed in the 2010s that a PHP-family language can get faster
while it runs when the engine watches real execution.

OpenGrid turns the stack around: the framework is not loaded by the
runtime, it *is* the runtime. Routing, request lifecycle, data access,
middleware, sessions and queues are compiled Rust; the dialect is embedded
in that binary and only calls into it. There is nothing to boot per
request, no translation layer between framework and engine, and the JIT
can treat framework calls as what they are: native functions. Everything
in the benchmark below follows from that one decision.

## The interpreter alone is slower than PHP

Honesty first: measured as a pure language runtime, grd is **behind PHP**.
In the script benchmarks `c1`–`c3` (request-shaped work, runtime
functions, numeric compute; `bench/complex/`), the current grd binary with
PGO takes **1.7× to 4.0× the time of PHP 8.4 with opcache**, depending on
the workload (c2 1.7×, c1 2.8×, c3 4.0×); against PHP's own tracing JIT,
grd's JIT lands at 1.7×–3.1×. Interpreter work (array handling, method
calls, string building) is where the gap is, and it is being closed step
by step (the history of these factors is in `.agents/BENCHMARKS.md` §2).

The thesis behind the project was that this deficit would be more than
compensated once the framework lives inside the engine, because a web
request spends most of its time in routing, I/O, data access and
serialization, not in the script. The framework benchmark confirms it by
a wide margin: the same controller logic that runs 2–4× slower than PHP in
isolation answers 7–18× more requests per second than Laravel on
FrankenPHP once both sit inside their frameworks, with a fixed cost per
request of 0.2–0.6 ms against Laravel's 2.2–2.9 ms. Where the interpreter
still shows is the per-element cost at large payloads (table (f′) in
[BENCHMARKS.md](BENCHMARKS.md)), which is exactly what the next JIT work
packages target.

## What the code looks like

A module of the example application (`examples/test-app/modules/notes`):

```php
<?grd
namespace Notes\Controllers;

use Notes\Models\Note;
use OpenGrid\Auth\Auth;
use OpenGrid\Http\Request;
use OpenGrid\Http\Response;

final class NoteController
{
    public function __construct(private Request $request)
    {
    }

    /** GET /api/notes?filter[done]=false&sort=priority&page=1&per_page=15 */
    public function index(): Response
    {
        $query = Note::where('user', Auth::id());
        $filters = $this->request->query('filter', []);
        if (is_array($filters)) {
            $query = $query->filter($filters);
        }
        $perPage = (int) $this->request->query('per_page', '15');
        $page = (int) $this->request->query('page', '1');
        return Response::json($query->sort()->paginate($perPage, $page));
    }

    public function store(): Response
    {
        $data = $this->request->validate([
            'title' => 'required|string|max:120',
            'body' => 'nullable|string',
            'done' => 'nullable|boolean',
            'priority' => 'nullable|integer|between:1,5',
        ]);
        $data['user'] = Auth::id();
        return Response::json(Note::create($data), 201);
    }
}
```

```php
<?grd
namespace Notes\Models;

use OpenGrid\Database\Model;

final class Note extends Model
{
    protected static string $table = 'notes';
    protected bool $softDeletes = true;
    protected array $fillable = ['user', 'title', 'body', 'done', 'priority'];
    protected array $filterable = ['done', 'priority'];
    protected array $hidden = ['deleted_at'];
}
```

```php
<?grd
Route::group(['prefix' => '/api/notes', 'middleware' => ['auth', JsonWrites::class]], function () {
    Route::get('', [NoteController::class, 'index']);
    Route::post('', [NoteController::class, 'store']);
    Route::get('/{id}', [NoteController::class, 'show']);
    Route::patch('/{id}', [NoteController::class, 'update']);
    Route::delete('/{id}', [NoteController::class, 'destroy']);
});
```

If this reads like Laravel to you, that is the point. What is different is
underneath:

- **Strict by construction.** Every variable keeps the type it was given
  (`GRD021` is a compile error, not a warning), `/` always yields a float,
  integer overflow throws. The VM never has to guess a type, which is what
  lets the JIT emit machine code without speculation or deoptimization.
- **No Composer, no `include`, no superglobals.** A project is a set of
  modules (`modules/<name>/{app,routes,database}`), resolved by convention;
  the request is an object; there is no service container to configure.
- **One process.** HTTP server, worker pool, jobs, scheduler and the
  embedded database run in the `opengrid` binary. Hot reload in
  development is a file watcher (4 ms in the benchmark below); production
  has no reload at all.
- **The database is declarative.** Each module ships its schema as
  `schema.surql`; `opengrid task migrate_push` applies it. The default
  store is an embedded SurrealDB (SurrealKV); raw SQL on SQLite is
  available through the same `DB` facade.

The language is documented in `.agents/DIALECT.md`, the runtime model in
`.agents/RUNTIME_ARCHITECTURE.md`, the deliberate differences from Laravel
in `.agents/LARAVEL_BREAKING_CHANGES.md`.

## Benchmark (short version)

Nine frameworks run the same small API (health check, list, show, create,
login with bcrypt) with **identical raw SQL statements on the same SQLite
3.53.4 build**, the same durability settings (WAL, `synchronous=FULL`),
four workers or four cores each, and every side in its best production
configuration (Laravel on a FrankenPHP built with opcache and the tracing
JIT, Spring Boot with the JDK 25 AOT cache, ASP.NET Core as ReadyToRun,
Rails with YJIT and jemalloc, and so on). Three full runs, median shown.
Requests per second at 16 connections:

| Framework (runtime) | health | list | show | create | login |
|---|---:|---:|---:|---:|---:|
| **OpenGrid (grd VM, PGO, JIT)** | 138 023 | **38 640** | 67 814 | **1 347** | 82 |
| ASP.NET Core 10 (ReadyToRun) | **277 815** | 36 896 | **92 861** | 1 261 | 76 |
| Spring Boot 4 (JDK 25, AOT cache) | 144 236 | 21 034 | 39 827 | 1 272 | 76 |
| Goravel 1.18 (Go 1.27, Gin) | 128 578 | 19 340 | 45 146 | 1 284 | 84 |
| Phoenix 1.8 (OTP 29, Bandit) | 74 262 | 6 729 | 13 056 | 1 150 | 93 |
| AdonisJS 7 (Bun 1.4) | 53 617 | 20 299 | 33 280 | 1 150 | 82 |
| FastAPI (Python 3.15, Uvicorn, uvloop) | 35 642 | 11 214 | 18 728 | 1 100 | 82 |
| Rails 8.1 (Ruby 4.0, YJIT, Puma) | 17 849 | 8 624 | 11 822 | 1 042 | **97** |
| Laravel 13 (PHP 8.5, Octane on FrankenPHP) | 7 549 | 5 137 | 6 053 | 849 | 94 |

Against Laravel, OpenGrid answers **18× more health checks, 7.5× more list
requests, 11× more show requests and 1.6× more creates** per second; login
is bound by bcrypt (cost 10 on every side) and is the same everywhere.
Against the whole field, OpenGrid is first on `list` and `create`, second
on `show` behind ASP.NET Core, and third on the empty `health` route.

Beyond throughput (details and all tables in [BENCHMARKS.md](BENCHMARKS.md)):

| | OpenGrid | Laravel | best other |
|---|---:|---:|---|
| Memory (PSS, idle / under load) | 142 / 142 MiB | 206 / 218 MiB | Goravel 36 / 44 MiB |
| Cold start to first response (4 workers) | 72 ms | 463 ms (caches built) | Goravel 29 ms |
| Hot reload (production) | 4 ms | 436 ms (`octane:reload`) | – |
| First second after a cold start, as share of steady throughput | 98 % | 90 % | Spring Boot 32 %, ASP.NET Core 53 % |
| Logic per request, cost per element (`hydrate`) | 10.7 µs | 14.0 µs | ASP.NET Core 11.6 µs, AdonisJS 6.5 µs |

**Memory footprint** of the whole stack (PSS over every process of a
target, MiB; `show` at 16 connections; sorted by idle):

| Framework | idle | under load (peak) | after 10 000 requests |
|---|---:|---:|---:|
| Goravel (Go) | 36 | 44 | 42 |
| ASP.NET Core | 69 | 98 | 96 |
| Rails (4 Puma workers) | 131 | 284 | 280 |
| **OpenGrid (4 workers, incl. its embedded SurrealDB)** | **142** | **142** | **136** |
| Phoenix (BEAM) | 165 | 175 | 168 |
| FastAPI (4 Uvicorn workers) | 189 | 198 | 200 |
| Laravel (FrankenPHP, 4 workers) | 206 | 218 | 218 |
| AdonisJS (4 Bun processes) | 230 | 423 | 333 |
| Spring Boot (JVM, heap fixed at 512 MiB) | 397 | 548 | 548 |

OpenGrid is the only side whose memory does not grow under load: the
object store is per request, and the 142 MiB include the embedded
SurrealDB it opens at start for its job poller (not used by this
benchmark).

How each runtime behaves right after a cold start, under constant load,
as a share of its own steady throughput (first 30 s, `hydrate:100`, 16
connections; the JIT runtimes climb, the compiled and interpreted ones
start at full speed):

![Warm-up curve after a cold start, hydrate:100](bench/results/2026-10-28-e0af77d5/warmcurve-hydrate-100.svg)

Read the numbers with these caveats, which the full report spells out:

- **The JIT translated nothing in this benchmark.** It covers numeric
  code today; the controller code (strings, arrays, method calls) ran in
  the interpreter. Extending it to framework code is the next work package.
- OpenGrid's cost per element at large `n` is 2–8× that of the JIT
  runtimes (Spring, .NET, Bun), which is where the interpreter shows (see
  "The interpreter alone is slower than PHP" above).
- `create` is limited by SQLite's write lock on every side; `login` by
  bcrypt. Both measure the shared library more than the framework.
- The warm-up before each measured point was 60 s (one framework was not
  stable earlier); the results are "warm" results.
- One machine (AMD Ryzen 7 8845HS, 16 threads, clock capped at 4.5 GHz),
  one configuration. Every deviation of a comparison app from its
  framework's defaults is listed in `bench/frameworks/DEVIATIONS.md`.

## Reproducing the benchmark

Everything needed is in this repository; the runtimes are pinned with
checksums and installed under `~/.local/opt`. You need Linux x86-64,
ideally 16 logical cores (the method pins the application, the load
generator and the database to separate cores), 16 GB RAM, about 30 GB
of disk, and network access for the setup only.

```sh
# 1. environment (or use .devpod/Containerfile)
bench/frameworks/install_runtimes.sh      # Bun, Ruby, Python, JDK, .NET, OTP/Elixir, Go, PHP ZTS, FrankenPHP
bench/frameworks/setup.sh                 # builds the seven comparison apps from their lockfiles
bench/framework/laravel/setup.sh          # the Laravel comparison app

# 2. the measurement (CPU governor must be `performance`; ~45 min build + 3 runs)
bench/run_all.sh --plan                   # what will run and how long it is expected to take
bench/run_all.sh                          # results in target/bench-tmp/<commit>-<time>/report.md
```

`bench/run_all.sh --smoke` runs every stage with short durations first.
The raw data of the published run is in `bench/results/2026-10-28-e0af77d5/`.

## Building and running OpenGrid

```sh
cargo build -p opengrid-framework --bin opengrid
export PATH=$PWD/target/debug:$PATH
opengrid new my-app --name MyApp
cd my-app && opengrid start               # http://localhost:5150/
```

`.agents/QUICKSTART.md` walks from an empty directory to a module with a
model, a schema and an API. `cargo test --workspace` runs the suite (the
tests compare against a local PHP 8.4 and a Laravel installation where
they exist, and skip those comparisons otherwise).

## Where the project stands

**Works today:** routing, controllers, middleware, request validation with
Laravel's rules and messages, models with filters, sorting and pagination,
soft deletes, auth with API tokens, sessions, jobs and a scheduler, mail,
tasks over a control socket, modules installed from Git, the embedded
SurrealDB and raw SQL on SQLite, the `Str`/`Arr`/`Collection` support
classes tested against Laravel's behavior, hot reload, PGO builds, a
Cranelift JIT for numeric code.

**Does not exist yet:** a frontend story (Inertia/Vue is deferred),
Eloquent-style relations, a package registry, Windows or macOS builds, any
compatibility promise. The JIT does not yet reach framework code.

**Next:** extend the JIT to the operations that web code is made of
(method calls, strings, arrays, `foreach`) and translate the whole project
at server start so the interpreter becomes a fallback; then run this
benchmark again. The roadmap with work packages and acceptance criteria is
`.agents/ROADMAP.md`; decisions and their reasons are in
`.agents/DECISIONS.md`; known gaps in `.agents/KNOWN_ISSUES.md`.

## Help wanted

This is one person's research project, and it has reached the point
where it needs more hands and some funding to become a framework people
can rely on.

**Contributors.** Good first areas: the JIT (Cranelift, Rust), the
dialect's PHP compatibility (our probes compare against PHP 8.4, the long
tail is in `.agents/PHPT_REPORT.md`), the database layer, the comparison
apps of the benchmark (each is a small idiomatic app in its own
framework; corrections from people who know those frameworks are very
welcome), documentation and a frontend integration. Open an issue before
a large change; `.agents/Tools.md` explains how the test suite and the
commit gate work. Code and comments are in English; most project
documents in `.agents/` are in German for now, and translating them is a
contribution too.

**Sponsors.** The benchmark machine, the time to review contributions
and the remaining work on the JIT are the bottlenecks. If your company
would benefit from a Laravel-shaped framework with these numbers, or you
simply want to see where this goes, please get in touch through a GitHub
issue or discussion. <!-- TODO: sponsoring link -->

## License

OpenGrid is licensed under the **GNU Affero General Public License,
version 3 or later**, see [LICENSE](LICENSE). The project started as a
fork of [yingkitw/phprs](https://github.com/yingkitw/phprs); the
interpreter has since been replaced, and the files that still come from
the original project stay under the Apache License 2.0
([LICENSE-APACHE](LICENSE-APACHE)); [NOTICE](NOTICE) lists them. The
comparison applications under `bench/` keep the licenses of the starter
kits they were generated from.
