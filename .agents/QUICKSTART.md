# OpenGrid — Quickstart (from zero to the first request)

As of 2026-10-24 (TP4, TP5). Replaces the upstream phprs guide. The model is the test project
`phprs/examples/test-app`; everything here can be seen running there.

## 1. Build the binary

```fish
cd phprs
cargo build -p opengrid-framework --bin opengrid
set -x PATH $PWD/target/debug $PATH      # bash: export PATH=$PWD/target/debug:$PATH
```

`opengrid` is the framework (Loco CLI with the OpenGrid tasks). For scripts without the framework there is
`grd run file.grd` (crate `grd-jit`).

## 2. Create and start a project

```fish
opengrid new my-app --name MyApp
cd my-app
opengrid start
curl http://localhost:5150/            # {"app":"MyApp","status":"ok"}
```

`opengrid new` creates:

```
my-app/
├── .gitignore                  /.opengrid/ (embedded database) and /storage/
├── config/
│   ├── development.yaml        database embedded, no setting required
│   ├── test.yaml               mem:// (in-memory)
│   └── production.yaml
├── database/sources/main.surql base schema of the project
└── modules/welcome/
    ├── app/Controllers/WelcomeController.grd
    └── routes/web.grd
```

The database is embedded (`surrealkv://.opengrid/db`) and survives restarts. To use a SurrealDB server
instead: `settings.opengrid.database.url` or `SURREAL_DB_*`.

## 3. Write a module

A module is a folder under `modules/`. The namespace follows the folder: `modules/notes/app/Models/Note.grd`
contains `namespace Notes\Models; final class Note …`. One class per file, no code outside classes.

**Schema** (`modules/notes/database/schema/schema.surql`, hand-written, declarative):

```sql
DEFINE TABLE notes SCHEMAFULL PERMISSIONS NONE;
DEFINE FIELD title ON TABLE notes TYPE string;
DEFINE FIELD done ON TABLE notes TYPE bool DEFAULT false;
```

Manual adjustments (for example from Surrealist) belong in `manually.surql` next to it; they are read after all
`schema.surql` files and win. Applying in development:

```fish
opengrid task migrate_push        # merges everything → database/schema/temporary.surql, syncs
opengrid task migrate_pull        # current state of the database in the same format
```

**Model** (`app/Models/Note.grd`):

```php
<?grd
namespace Notes\Models;

use OpenGrid\Database\Model;

final class Note extends Model
{
    protected static string $table = 'notes';
    protected array $fillable = ['title', 'done'];
    protected array $filterable = ['done'];
}
```

**Controller** (`app/Controllers/NoteController.grd`) — returning an array → JSON, a string → HTML,
`Response` for status and headers:

```php
<?grd
namespace Notes\Controllers;

use Notes\Models\Note;
use OpenGrid\Http\Response;

final class NoteController
{
    public function index(): Response
    {
        return Response::json(Note::query()->filter()->sort()->get());   // ?filters[done]=true&sort=title
    }

    public function store(): Response
    {
        $data = request()->validate(['title' => 'required|string|max:120', 'done' => 'nullable|boolean']);
        return Response::json(Note::create($data), 201);
    }

    public function show(string $id): Response
    {
        return Response::json(Note::findOrFail($id));                    // 404 if it does not exist
    }
}
```

**Routes** (`routes/web.grd`), with middleware:

```php
<?grd
use Notes\Controllers\NoteController;
use Notes\Http\Middleware\JsonWrites;
use OpenGrid\Routing\Route;

Route::group(['prefix' => '/notes', 'middleware' => [JsonWrites::class]], function () {
    Route::get('', [NoteController::class, 'index']);
    Route::post('', [NoteController::class, 'store']);
    Route::get('/{id}', [NoteController::class, 'show']);
});
```

Middleware is given as a class name in the route; aliases exist only for the framework's own (`auth`, `csrf`, `throttle`),
`Route::aliasMiddleware` is deprecated for project middleware (MW1, warning in development). A middleware
extends `OpenGrid\Http\Middleware` and implements
`handle(Request $request, \Closure $next): mixed` (example: `examples/test-app/modules/notes/app/Http/Middleware/JsonWrites.grd`).
Parameters follow after a colon and arrive as strings after `$next`, as in Laravel:
`->middleware('throttle:60,1')` allows 60 requests per minute per user or IP, after that 429 with `Retry-After`.
Login is throttled without configuration (AU2): `Auth::attempt` answers with 429 after 5 failed attempts per e-mail and IP
within 60 s (`settings.auth.login_throttle.max_attempts`/`decay_seconds`, `0` switches it off). Custom
limits with `OpenGrid\Support\RateLimiter` (`hit`, `tooManyAttempts`, `availableIn`, `clear`, `attempt`).
Tokens do not expire as long as `settings.auth.token_expiration` (minutes) is 0; otherwise like Sanctum, counted from their creation.
Individual tokens get their own expiry with `Auth::createToken($user, 'name', new DateTimeImmutable('+30 days'))`.

## 4. Try it out

```fish
opengrid task migrate_push
opengrid start
curl -H 'Content-Type: application/json' -d '{"title":"First note"}' localhost:5150/notes     # 201
curl -H 'Content-Type: application/json' -d '{}' localhost:5150/notes                          # 422 with errors
curl 'localhost:5150/notes?sort=title'
```

In development, errors show an error page (HTML) or JSON with file, line and trace;
`.grd` changes are loaded without a restart.

## 5. The dialect in one paragraph

PHP syntax, but strict (`.agents/DIALECT.md`): no `include`, no superglobals, no references,
`==` compares strictly, errors are exceptions, results of `sort()` and friends must be assigned
(`$a = sort($a);`). The compiler reports violations with a GRD code and a hint. `grd check --report <path>`
counts the hits for existing PHP code.

## 6. Production

Startup in production does not sync the schema itself but only executes planned rollouts
(`opengrid task rollout cmd:plan name:…`, check the manifest, then deploy); rollouts may only
extend the schema, otherwise startup aborts (preflight). The JIT is off by default in the workers
(`OPENGRID_JIT=on` enables it, `.agents/JIT.md`).

## 7. The example application with login and API (TP5)

`phprs/examples/test-app` is a complete base application: registration, login with bearer token
(like Laravel Sanctum), logout and the user's own notes through a JSON API.

```
modules/auth/   users schema, model User, interface PasswordHasher → BcryptHasher (is injected),
                AuthService (injected into the controller), AuthController, routes /api/register … /api/me
modules/notes/  notes schema (owned by a user, soft delete), model Note, NoteController, middleware
config/         settings.auth.model = Auth\Models\User
tests/api.sh    automatic acceptance test (curl + jq)
```

**Start:**

```fish
cd phprs
cargo build -p opengrid-framework --bin opengrid
cd examples/test-app
../../target/debug/opengrid task migrate_push     # create the schema (embedded DB in .opengrid/)
../../target/debug/opengrid start
```

**Automatic test** (starts its own server on a free port with a fresh database,
plays through the whole flow, restarts and checks that the data is still there):

```fish
cd phprs
examples/test-app/tests/api.sh                      # PASS/FAIL per step, exit code ≠ 0 on failures
```

It also runs as part of the suite (`cargo test -p opengrid-framework`, test `api_script`).

**The same test against the Laravel replica** (LV1, only for comparing and measuring; the app lives outside
the repository under `OpenGrid_Framework/bench/laravel-app`):

```bash
bench/framework/laravel/setup.sh                                    # once: install Laravel + Sanctum + Octane
examples/test-app/tests/api.sh --app laravel                         # nginx + PHP-FPM
examples/test-app/tests/api.sh --app laravel --runtime octane        # Octane (RoadRunner)
```

Steps that apply only to OpenGrid (embedded database, restart on the same directory) are then
reported as `SKIP`.

**Replaying by hand** (bash; server running on port 5150):

```bash
B=http://localhost:5150
J='Content-Type: application/json'
curl -s $B/health
curl -s -H "$J" -d '{"name":"Ada","email":"ada@example.com","password":"secret-123","password_confirmation":"secret-123"}' $B/api/register
T=$(curl -s -H "$J" -d '{"email":"ada@example.com","password":"secret-123"}' $B/api/login | jq -r .token)
curl -s -H "Authorization: Bearer $T" $B/api/me
curl -s -H "Authorization: Bearer $T" -H "$J" -d '{"title":"Groceries","priority":2}' $B/api/notes
curl -s -H "Authorization: Bearer $T" "$B/api/notes?filter%5Bdone%5D=false&sort=priority:desc&per_page=10&page=1"
ID=$(curl -s -H "Authorization: Bearer $T" $B/api/notes | jq -r '.data[0].id | split(":")[1]')
curl -s -X PATCH -H "Authorization: Bearer $T" -H "$J" -d '{"done":true}' $B/api/notes/$ID
curl -s -X DELETE -H "Authorization: Bearer $T" -o /dev/null -w '%{http_code}\n' $B/api/notes/$ID   # 204
curl -s -X POST -H "Authorization: Bearer $T" -o /dev/null -w '%{http_code}\n' $B/api/logout        # 204
curl -s -H "Authorization: Bearer $T" $B/api/me                                                     # 401
```

Responses: missing or invalid token → 401 JSON; validation error → 422 with `errors`; someone else's
or unknown note → 404 (not 403, so that foreign IDs are not confirmed); deleted note → 404.
Sorting with `sort=field` or `sort=field:desc` (several separated by comma); an invalid `sort` → 400.

## 8. Benchmarks

```bash
cd phprs
cargo build --profile profiling -p grd-jit --bin grd -p opengrid-framework --bin opengrid
python3 bench/complex/bench.py              # scripts c1–c3, comparisons A/B/C (method: .agents/BENCHMARKS.md §1)
python3 bench/framework/bm2.py              # framework over HTTP: OpenGrid against Laravel (BM2)
```

## Mail and files (MS1)

```php
// modules/users/app/Mail/WelcomeMail.grd
final class WelcomeMail extends \OpenGrid\Mail\Mailable
{
    public function __construct(private string $name) {}
    public function build(): void { $this->view('welcome', ['name' => $this->name]); }
}
// modules/users/resources/mail/welcome/subject.t, html.t, text.t  (Tera: {{ name }})
Mail::to($user->email)->send(new WelcomeMail($user->name));   // use OpenGrid\Mail\Mail;

Storage::put('reports/2026.csv', $csv);                       // use OpenGrid\Storage\Storage;
$csv = Storage::get('reports/2026.csv');                      // null if it is missing
Storage::disk('memory')->put('tmp.txt', 'x');
```

Settings (`config/<env>.yaml`, `settings:`): `mailer.from`, `mailer.smtp` (`host`, `port`, `secure`, `auth`);
without `mailer.smtp` mails are only logged (`Mail::sent()` in tests). `storage.default`, `storage.disks`
(`driver: local` with `root`, `driver: memory`); without a setting `local` = `<project>/storage/app`.

## Objects in the session (SES1)

```php
$cart = Session::remember('cart', fn (): Cart => new Cart());   // use OpenGrid\Support\Session;
$cart->add($item);   // serialised at the end of the request, rebuilt fresh in the next request
Session::forget('cart');
```
`Session::put` stores only JSON values; objects belong in `remember`.

## Tasks (J1)

```php
// modules/notes/app/Tasks/Prune.grd
namespace Notes\Tasks;
final class Prune extends \OpenGrid\Console\Task
{
    public function handle(array $args): int { $this->info('days: ' . ($args['days'] ?? '30')); return 0; }
}
```

`opengrid task run task:notes:prune days:30`. If a server is running for the project, it executes the task
(`.opengrid/control.sock`); otherwise it runs standalone. Likewise `opengrid task migrate_push` etc.

## Modules from Git (MOD1, PKG1)

```fish
cargo run -q -p opengrid-framework --bin opengrid -- task module cmd:add url:https://example.com/blog.git rev:v1.0.0
cargo run -q -p opengrid-framework --bin opengrid -- task module cmd:list
cargo run -q -p opengrid-framework --bin opengrid -- task module cmd:verify     # local changes?
cargo run -q -p opengrid-framework --bin opengrid -- task module cmd:remove name:blog
```
The repository needs a `module.toml` (`name`, `version`, `depends = ["auth"]`). After `add` the
whole project is checked; if it does not pass, the module is removed again. `modules.lock` belongs in the project repository.
A module may only use modules that it names in `depends` (GRD106).

## Jobs and schedule (J2, J3)

```php
final class SendDigest extends \OpenGrid\Queue\Job          // modules/users/app/Jobs/SendDigest.grd
{
    public int $tries = 3;
    public function __construct(private string $email) {}
    public function handle(): void { /* … */ }
}
SendDigest::dispatch($user->email);                            // runs after the request on a worker

// routes/console.grd
Schedule::task('notes:prune', ['days' => '30'])->dailyAt('03:00');   // use OpenGrid\Scheduling\Schedule;
Schedule::job(SendDigest::class, ['a@example.com'])->hourly();
```
The server checks the schedule every full minute (UTC).

With a database, jobs are rows in `jobs` and survive a restart (J4); whatever fails after `$tries`
attempts ends up in `failed_jobs` together with the exception:

```fish
opengrid task queue_failed            # failed jobs
opengrid task queue_retry id:all      # re-enqueue (or id:<uuid>)
opengrid task queue_flush             # delete failed jobs
```
`SendDigest::dispatchLater(600, $email)` starts after ten minutes at the earliest; `public int $backoff = 30;`
waits between two attempts.

