# OpenGrid Roadmap

Stand: 2026-10-23. Grundlage: `HeldvonKosmos/phprs` (Interpreter, Tag
`upstream-baseline` + 6 Commits Aufräumen), `HeldvonKosmos/loco` (Fork von
loco-rs 1.2.0), `inertiajs/inertia-omega` (Alpha), `DIALECT.md`, `BRIDGES.md`,
`INVENTORY.md`, `KNOWN_ISSUES.md`.

## 0. Das Ziel in einem Satz

Ein Rust-Framework (Loco/Axum/SeaORM), dessen Anwendungsschicht in einem
strikten PHP-Dialekt (`.grd`) geschrieben wird, der auf einem eingebetteten
Interpreter läuft und über eine feste, framework-neutrale Host-Schnittstelle
auf alle Dienste des Frameworks zugreift.

"Framework-agnostisch" heißt konkret: Der Interpreter kennt Loco nicht. Er
kennt nur eine Host-ABI (Traits + Registry). Loco ist der erste Host, der
diese ABI implementiert. Jeder andere Host (nacktes Axum, Actix, ein
CLI-Runner, ein Test-Harness) kann dieselben `.grd`-Dateien ausführen, wenn er
die ABI erfüllt. Diese Trennung ist der Kern des Plans und zieht sich durch
jeden Meilenstein.

## 1. Repositories und Crates

### 1.1 Empfohlene Aufteilung

```
HeldvonKosmos/phprs      → Interpreter-Crate        (umbenennen: opengrid-core)
HeldvonKosmos/loco       → Loco-Fork, nur Patches    (upstream-tracking, siehe §6)
HeldvonKosmos/opengrid   → NEU: Framework-Workspace  (das "Hauptprojekt")
```

Das Hauptprojekt ist nicht der Loco-Fork, sondern ein eigener Workspace, der
beide als Abhängigkeit zieht. Begründung: Wer Anwendungscode in den Loco-Fork
legt, muss bei jedem Upstream-Merge Konflikte in fremdem Code lösen. Der Fork
bleibt eine dünne Patch-Serie, die sich per `git rebase upstream/master`
pflegen lässt.

```
opengrid/                         (Cargo-Workspace)
├── crates/
│   ├── opengrid-host/            Host-ABI: Traits, Value-Brücke, Registry-Makros   ← framework-neutral
│   ├── opengrid-loco/            Loco-Adapter: implementiert die ABI mit AppContext ← Loco-spezifisch
│   ├── opengrid-inertia/         Inertia-Bindung (nutzt inertia-omega)
│   ├── opengrid-data/            Polars/ndarray-Klassen (M5)
│   └── opengrid-cli/             `opengrid` CLI: new, serve, make:*, pkg, db
├── examples/test-app/            Referenzanwendung (Modules-Only-Struktur)
└── Cargo.toml                    workspace: core = { git = "…/phprs" }, loco-rs = { git = "…/loco" }
```

`opengrid-core` (der Interpreter) hat keine Abhängigkeit auf `opengrid-host`;
die Richtung ist `host → core`. So bleibt der Interpreter auch ohne Framework
testbar (`cargo test` in phprs läuft weiter in Sekunden).

### 1.2 Was in welchen Repo-Fork gehört

| Änderung | Repo |
|---|---|
| Native Registry, Objekt-Payload, Dialektprüfung, Bugfixes | phprs |
| Host-Traits, `Val ↔ serde_json::Value`, `#[php_class]`-Makro | opengrid/crates/opengrid-host |
| Catch-all-Handler, VM-Pool, DB-Bridge, Worker/Task/Mailer-Bridges | opengrid/crates/opengrid-loco |
| Patches an Loco selbst (nur wenn unvermeidbar, §6) | loco-Fork |
| Test-App, Modul-Manager, Generatoren | opengrid |

## 2. Die Host-ABI (beide Seiten)

Das ist die Schnittstelle, die "framework-agnostisch" einlöst. Sie hat zwei
Hälften, die sich spiegeln.

### 2.1 Was der Interpreter dem Host anbietet (`opengrid-core`)

```rust
pub struct Engine { /* kompilierte Module, Klassen-Tabelle, Registry */ }

impl Engine {
    pub fn new(registry: NativeRegistry) -> Engine;
    pub fn load_module(&mut self, manifest: &ModulePath) -> Result<(), CompileError>;  // Autoloader-Wurzel
    pub fn reload_file(&mut self, path: &Path) -> Result<(), CompileError>;           // Hot-Reload
    pub fn instantiate(&self, class: &str, args: &[Val]) -> Result<ObjHandle, Throw>;
    pub fn call_method(&self, obj: ObjHandle, method: &str, args: &[Val]) -> Result<Val, Throw>;
    pub fn call_static(&self, class: &str, method: &str, args: &[Val]) -> Result<Val, Throw>;
    pub fn classes_with_attribute(&self, attr: &str) -> Vec<ClassInfo>;              // z.B. #[Route]
}

pub struct NativeRegistry;
impl NativeRegistry {
    pub fn function(&mut self, name: &str, f: NativeFn);
    pub fn class(&mut self, def: NativeClassDef);   // Name, Parent, Methoden, Konstanten, erbbar?
}
pub type NativeFn = fn(&mut HostCtx, &[Val]) -> Result<Val, Throw>;
```

`HostCtx` ist ein `&mut dyn Any`-artiger Zugriff auf den Host-Zustand des
aktuellen Requests (DB-Pool, Request, Session). Der Interpreter reicht ihn
nur durch, er interpretiert ihn nicht.

### 2.2 Was der Host dem Interpreter liefern muss (`opengrid-host`)

Traits, die der Loco-Adapter implementiert und die jeder andere Host
implementieren müsste:

| Trait | Methoden (Auszug) | Von welchen nativen Klassen genutzt |
|---|---|---|
| `HostDatabase` | `query(sql, binds) -> Rows`, `execute(sql, binds) -> Affected`, `transaction(f)`, `last_insert_id()` | `DB`, `Model`, `Schema` |
| `HostRequest` | `method()`, `path()`, `query()`, `headers()`, `cookies()`, `body_json()`, `form()`, `files()`, `ip()` | `Request` |
| `HostResponse` | Sink für `Response`, `Redirect`, `JsonResponse`, `InertiaResponse` | `Response`, `Inertia` |
| `HostSession` | `get`, `put`, `pull`, `flash`, `regenerate`, `destroy` | `Session`, `Inertia` (Flash/Errors) |
| `HostCache` | `get`, `put(ttl)`, `forget`, `remember` | `Cache` |
| `HostStorage` | `put`, `get`, `delete`, `exists`, `url` | `Storage` |
| `HostQueue` | `dispatch(class, payload, opts)`, `dispatch_unique`, `batch` | `Queue`, `Job::dispatch()` |
| `HostMailer` | `send(Mail)`, `send_later(Mail)` | `Mail`, `Mailable` |
| `HostAuth` | `hash`, `verify`, `issue_jwt(claims)`, `verify_jwt`, `current_user()` | `Auth`, `Hash` |
| `HostConfig` | `get(key)`, `env()` | `Config`, `config()` |
| `HostLog` | `log(level, msg, context)` | `Log` |
| `HostHttp` | `request(method, url, opts)` | `Http` (ausgehend) |
| `HostClock` | `now()`, `tz()` | `Carbon`-artige `DateTime`-Klasse |
| `HostFs` (optional) | sandboxed Lesen unter `storage/` | `Storage::local` |

Jeder Trait hat eine `Null`-Implementierung (für Tests) und eine
`Loco`-Implementierung. Die nativen Klassen in `opengrid-host` sind gegen die
Traits geschrieben, nicht gegen Loco. **Das ist die Stelle, an der ein
zweiter Host einsteigt.**

### 2.3 Die Wertebrücke

Genau eine Konvertierung, in beide Richtungen, zentral in `opengrid-host`:

```
Val  ←→  serde_json::Value      (Arrays: lückenlose 0..n → JSON-Array, sonst Objekt)
Val  ←→  sea_query::Value       (DB-Bindings und Spaltenwerte; Datum/Zeit → String ISO-8601, Decimal → String)
```

Alles andere (Inertia-Props, Cache-Werte, Queue-Payloads, Konfiguration,
JSON-Responses) läuft über `serde_json::Value`. Objekte mit nativem Payload
(DB-Handle, DataFrame) sind nicht serialisierbar und werfen beim Versuch.

## 3. Dienst-für-Dienst: Loco → Dialekt

Was Loco 1.2 tatsächlich hat (aus dem Quelltext) und wie es im Dialekt
erscheint. Spalte "Mechanik" sagt, was der Adapter dafür baut.

| Loco-Dienst | Loco-API (Rust) | Dialekt-API (`.grd`) | Mechanik im Adapter | MS |
|---|---|---|---|---|
| HTTP-Routing | `Routes::add(uri, MethodRouter)`, `AppRoutes` | `Route::get('/tasks', [TaskController::class, 'index'])`, `Route::resource`, `->middleware()`, `->prefix()`, `->name()`; alternativ `#[Get('/tasks')]`-Attribute | Ein Catch-all `/{*path}` + `/` → `ArcSwap<RouteTable>` (matchit). `routes/web.grd` je Modul wird beim Start ausgeführt und füllt die Tabelle | M0 |
| Request | Axum-Extractors, `RemoteIP`, `CookieJar` | `Request $r`: `input()`, `query()`, `json()`, `header()`, `cookie()`, `file()`, `ip()`, `validate([...])` | Request wird einmal in ein natives `Request`-Objekt gewandelt (lazy Body-Parsing) | M2 |
| Response | `format::{json,html,text,redirect,render()}` | `return ['a'=>1]` (→ JSON), `return 'html'`, `response()->json()`, `->status()`, `->header()`, `->cookie()`, `redirect('/x')`, `back()` | Rückgabewert-Mapping, Cookies über `RenderBuilder::cookies` | M2 |
| Middleware | `MiddlewareLayer`, `Hooks::middlewares` | `class EnsureOwner extends Middleware { handle(Request $r, Closure $next) }` + Loco-Stack (CORS, Compression, Timeout …) aus YAML | Script-Middleware läuft **innerhalb** des Catch-all als Kette; Locos Tower-Middleware bleibt außen und wird nur konfiguriert | M2 |
| Validation | `validator`-Crate, `JsonValidate` | `$r->validate(['title' => 'required|string|max:120'])`, `ValidationException` → Inertia-Errors | Eigener Rule-Parser in `opengrid-host` (Laravel-Syntax), nicht `validator` | M2 |
| Inertia | `inertia-omega`: `InertiaLayer`, `Inertia::render(component, props)`, `defer/lazy/merge`, `back_with_errors`, `flash` | `Inertia::render('Tasks/Index', [...])`, `Inertia::share()`, `Inertia::defer(fn() => …)`, `Inertia::location()` | `InertiaLayer` in `Hooks::after_routes`; Closures im Dialekt → `inertia::lazy(|| …)` via VM-Rückruf im Worker | M2 |
| Sessions | **nicht in Loco** | `Session::get/put/flash` | `tower-sessions` mit Store (Memory/Redis/SQL) im Adapter; Inertia braucht sie für Errors/Flash | M2 |
| Datenbank (raw) | `DatabaseConnection::query_all_raw/execute_raw`, `Statement::from_sql_and_values` | `DB::select($sql, $binds)`, `DB::statement()`, `DB::transaction(fn)` | `HostDatabase` über SeaORM-Connection; Transaktion hält den Worker-Thread | M3 |
| Query-Builder | `sea_query` (dynamisch, kein Codegen) | `DB::table('tasks')->where('done', false)->orderBy('id')->limit(20)->get()`, `->first()`, `->insertGetId()`, `->update()`, `->delete()`, `->paginate(15)`, Joins, `whereIn`, Aggregates | Builder-Objekt in Rust (`sea_query::SelectStatement` etc.), Dialekt-Methoden mutieren es, Terminal-Methoden führen aus | M3 |
| Models | SeaORM-Entities (statisch, generiert) | `class Task extends Model { protected $table; $fillable; casts; }`, `Task::find()`, `::where()`, `::create()`, `$t->save()`, `->delete()`, Timestamps, Relationen `hasMany/belongsTo` (lazy + `with()`) | Models bauen auf dem Query-Builder, **nicht** auf Entities. Schema-Introspektion (`db/introspect.rs`) liefert Spalten für Casts/Fillable-Defaults | M3 |
| Migrationen | `sea-orm-migration` + `schema.rs`-Helfer (`create_table`, `add_column`, `add_reference`, `timestamps_tz`) | `database/migrations/2026_10_23_create_tasks.grd`: `Schema::create('tasks', fn(Blueprint $t) { $t->id(); $t->string('title'); $t->boolean('done')->default(false); $t->timestamps(); })` | `Blueprint` → `sea_query::TableCreateStatement`; Migrationsläufer in Rust (eigene Tabelle `_opengrid_migrations`), nutzt Locos `SchemaManager` | M3 |
| Seeds | `Hooks::seed(ctx, path)`, YAML-Seeds | `database/seeders/TaskSeeder.grd` | `Hooks::seed` ruft Script-Seeder; YAML-Seeds bleiben möglich | M3 |
| Multi-Tenancy | `TenantEntity`, `TenantQueryExt` (Feature) | `Model::$tenantColumn`, globaler Scope | Builder fügt `where tenant_id = ?` an; Tenant aus `HostCtx` | M6 |
| Background-Jobs | `BackgroundWorker<A>`, `Queue::enqueue/enqueue_unique/enqueue_batch`, Backends pg/sqlite/redis | `class SendReport extends Job { handle() }`, `SendReport::dispatch($args)`, `->unique($key)`, `->priority()` | **Ein** Rust-Worker `ScriptJob { class, payload }` registriert in `connect_workers`; `perform` holt eine VM aus dem Pool, instanziiert die Klasse, ruft `handle()` | M6 |
| Tasks (CLI) | `Task` trait, `Tasks::register`, `Vars` | `app/Tasks/Cleanup.grd`: `class Cleanup extends Task { run(array $vars) }`, Aufruf `opengrid task cleanup k=v` | `register_tasks` scannt Module und registriert je Klasse einen `ScriptTask` | M6 |
| Scheduler | `scheduler.yaml`: `jobs: { name: { run: "task", cron: "…" } }` | gleiche YAML in `config/`; `run:` zeigt auf Script-Tasks | keine Änderung nötig, Script-Tasks sind Tasks | M6 |
| Mailer | `Mailer` trait, `Email`, `mail_template(Dir)` (include_dir, compile-time) | `class Welcome extends Mailable { build() { return $this->view('mail.welcome', [...])->subject(); } }`, `Mail::to($u)->send(new Welcome)` | `Email` direkt befüllen; Templates aus `modules/*/resources/mail/` zur Laufzeit mit Tera rendern; `send_later` → Queue | M6 |
| Cache | `cache::Cache` (inmem/redis), `get/insert/insert_with_expiry/remove` | `Cache::get`, `::put($k,$v,$ttl)`, `::remember()`, `::forget()` | Werte als JSON | M3 |
| Storage | `storage::Storage`, Treiber local/S3/Azure/GCP, Strategien mirror/backup | `Storage::disk('s3')->put($path, $bytes)`, `->get()`, `->delete()`, `->exists()` | Bytes als binary-safe String | M6 |
| Auth | `auth::jwt` (`generate_token`, `validate`), `hash::{hash_password, verify_password}`, Extractor `auth::JWT` | `Hash::make/check`, `Auth::attempt()`, `Auth::user()`, `Auth::id()`, Middleware `auth` | JWT-Claims im `HostCtx`; `Auth::user()` lädt per Model | M6 |
| Config | `Config::settings<T>()`, YAML + `{env}.local.yaml`, `get_env()`-Templating | `config('app.name')`, `env('KEY')`, `settings:`-Block trägt `opengrid:` | read-only Snapshot pro VM | M0 |
| Logging | `tracing` | `Log::info($msg, $ctx)` | `tracing::event!` mit Modulname als Target | M0 |
| Health | `/_health`, `/_ping` | bleibt Rust | statische Routen gewinnen gegen Catch-all | M0 |
| Views (Tera) | `TeraView`, `ViewRenderer` | nur für Mail-Templates und SSR-Fallback; UI ist Inertia | | M6 |
| Testing | `testing::request`, `axum-test`, Snapshots | `tests/Feature/*.grd`? → **nein**, Feature-Tests in Rust gegen die Test-App (`request::<App>`); Unit-Tests für Dialektcode später | | M0 |
| Generatoren | `loco-gen` (Model, Scaffold, Controller, Task, Worker, Mailer) | `opengrid make:controller`, `make:model`, `make:migration`, `make:job`, `make:mailable`, `make:module` | eigene Templates (`.grd`), nicht loco-gen | M6 |
| Doctor | `doctor.rs` | `opengrid doctor` prüft Module, Manifeste, Plugins | | M6 |

Was Loco **nicht** hat und wir mitbringen: Sessions/Flash, Validation mit
Laravel-Regeln, dynamische Models, Script-Migrationen, Hot-Reload, Modul-
System. Was wir **nicht** nachbauen: Locos statische Entities, `loco-gen`,
Tera-Views für UI.

## 4. Meilensteine

Jeder Meilenstein endet mit einem Abnahmetest in `examples/test-app` und
grüner Suite in beiden Repos. Reihenfolge ist Abhängigkeitsreihenfolge, nicht
Priorität.

### M0 — Fundament (Interpreter-Fixes, Workspace, erster Request)

Interpreter (phprs):
- `KNOWN_ISSUES` #1 Cast-Syntax `(int)`, `(float)`, `(string)`, `(bool)`, `(array)`.
- `KNOWN_ISSUES` #2 Exception-Unterklassen verlieren die Nachricht.
- Strikte Vergleiche: `IsEqual`/`IsNotEqual` → identisch; `zval_compare` auf
  erlaubte Typpaare; `switch` strikt; `in_array` & Co. strikt (`DIALECT.md` §5.1).
- Warnings → Exceptions (`UndefinedKeyError`, `UndefinedVariableError`,
  `TypeError` bei Arithmetik mit Strings) (`DIALECT.md` §5.3).
- Autoloader-Hook: `Engine::load_module` mit Namensraum→Pfad-Konvention
  (`DIALECT.md` §2); `spl_autoload_*`-Code entfernen.
- Crate-Split `core` / `std` / `cli`; JIT-Dateien hinter Feature
  `experimental-jit`, Fibers hinter `fibers`.
- Umbenennung nach `opengrid-core`, Lizenz-Hinweise.

Workspace (opengrid):
- Workspace anlegen, `opengrid-host` mit Traits + `Null`-Impl + Wertebrücke.
- `opengrid-loco`: `Hooks`-Implementierung, VM-Pool im `shared_store`
  (`after_context`), Catch-all-Routen, `RouteTable`.
- Worker-Thread-Pool (N Threads, je eine `Engine`-Instanz, geteilte
  `Arc<OpArray>`), Request → Channel → Worker → Response; DB-Aufrufe per
  `Handle::block_on` (Begründung in `BRIDGES.md` §5).
- Test-App: `modules/tasks/routes/web.grd` + `TaskController::index()`
  gibt `['ok' => true]` zurück.

Abnahme: `GET /tasks` → JSON aus dem Dialekt; `cargo test` in phprs und
opengrid grün; Requests/s gegen Laravel Octane gemessen und notiert.

### M1 — Native Registry (die eigentliche Brücke)

- `NativeRegistry`: Funktionen (`HashMap` vor dem `match` in `builtins.rs`).
- `MethodBody::{Php(OpArray), Native(NativeMethod)}` in `ClassMethod`;
  Methodenaufruf verzweigt.
- `PhpObject.native: Option<Box<dyn Any + Send>>` für Rust-Payload.
- Native Klassen als `ClassEntry` in der `class_table`; `extends` auf native
  Klassen; `parent::` in native Methoden.
- `Fiber`, `Generator`, `Reflection*` von hartkodierten String-Vergleichen auf
  die Registry umstellen (Beweis, dass die Registry trägt).
- `#[php_class]` / `#[php_method]` Proc-Macro in `opengrid-host`, erzeugt
  `NativeClassDef` samt Typkonvertierung der Argumente.
- `Log`, `Config` als erste native Klassen.

Abnahme: `class TaskController extends Controller` mit `Controller` in Rust;
`Log::info()` aus dem Dialekt erscheint im Tracing-Log.

### M2 — Request, Response, Middleware, Validation, Inertia, Sessions

- Native `Request`, `Response`, `Redirect`, `JsonResponse`, `Middleware`.
- Script-Middleware-Kette (`handle($r, $next)`), Registrierung per Route und
  per Controller (`$this->middleware('auth')`).
- `Validator` mit Laravel-Regelsyntax (required, string, int, email, max, min,
  in, exists:table,col, unique:table,col, confirmed, nullable, array, date).
- `tower-sessions` im Adapter; `Session`-Klasse; Flash.
- `opengrid-inertia`: `Inertia::render/share/defer/lazy/location`,
  Validation-Errors → `back_with_errors`, Vite-Tags (`laravel-omega-vite`),
  SSR-Gateway optional.
- Test-App: Vue-Frontend `Pages/Tasks/Index.vue`, Formular mit
  Validierungsfehlern.

Abnahme: Seite rendert über Inertia, Formularfehler erscheinen im Frontend,
Flash-Nachricht nach Redirect.

### M3 — Datenbank, Query-Builder, Models, Migrationen, Cache

- `HostDatabase` über SeaORM-Connection (`query_all_raw`, `execute_raw`).
- `DB`-Facade mit `sea_query`-Builder (Select/Insert/Update/Delete, Joins,
  Aggregates, `paginate`), Transaktionen.
- `Model`-Basisklasse: Konventionen (`tasks`, `id`, `created_at`), `$fillable`,
  `$casts`, `find/all/where/create/save/delete/firstOrFail`, Relationen
  `hasMany/belongsTo/belongsToMany` mit `with()` (Eager Loading in zwei
  Queries), `toArray()` → Inertia-Props.
- `Schema`/`Blueprint` → `sea_query` Table-Statements; Migrationsläufer
  (`opengrid db migrate/rollback/status/fresh`), Loco-`SchemaManager`.
- Seeds als `.grd`.
- `Cache`-Klasse.
- Test-App: `Task`-Model, Migration, Seeder, CRUD vollständig.

Abnahme: `POST /tasks` legt an, `DELETE /tasks/{id}` löscht, Migration läuft
auf SQLite und Postgres, `paginate` liefert Inertia-kompatible Metadaten.

### M4 — Hot-Reload und Developer Experience

- File-Watcher (`notify`) auf `modules/**/*.grd`; Neu-Kompilierung einer
  Datei, Austausch des `Arc<OpArray>` in allen Worker-Engines zwischen zwei
  Requests; Routen-Tabelle neu aufbauen, wenn `routes/*.grd` betroffen.
- Fehlerseite im Dev-Modus mit `GRD`-Fehlern (Datei, Zeile, Hinweis) und
  Stacktrace des Dialekts.
- `opengrid serve` mit Vite-Dev-Server-Integration (Hot-File).
- Debug-Helfer `dd()`, `dump()` (nur Dev).

Abnahme: Controller-Text ändern, speichern, Reload im Browser zeigt die
Änderung, Server-Prozess lief durch; Syntaxfehler zeigt Fehlerseite statt
Absturz.

### M5 — Data Science (das Alleinstellungsmerkmal)

- `opengrid-data`: `DataFrame`, `Series`, `LazyFrame` als native Klassen über
  Polars; `DataFrame::fromQuery($builder)`, `::fromArray()`, `->filter()`,
  `->groupBy()->agg()`, `->join()`, `->sort()`, `->toArray()`, `->toInertia()`
  (spaltenorientiertes JSON für Charts), `->toCsv()`, `->toParquet()`.
- `ndarray`-Bindung für numerische Vektoren/Matrizen, Statistik-Helfer.
- Lange Berechnungen als Job (`M6`) mit Ergebnis im Storage.
- Test-App: Dashboard-Route, aggregiert Task-Statistiken, Chart im Frontend.

Abnahme: Aggregation über 1 Mio. Zeilen in Polars aus dem Dialekt gestartet,
Ergebnis als Inertia-Prop; Laufzeit gemessen.

### M6 — Framework-Vollständigkeit

- Jobs/Queue (`ScriptJob`-Worker, `dispatch`, unique, batch, Priorität).
- Tasks (`ScriptTask`), Scheduler-YAML.
- Mailer (`Mailable`, Tera-Templates aus Modul-Ressourcen, `send_later`).
- Storage (`Storage::disk()`), Auth (`Hash`, `Auth`, JWT-Middleware), Multi-Tenancy-Scope.
- Modul-Manager (`opengrid pkg`): `module.json`, Git-Quellen, `phprs.lock`,
  Abhängigkeiten zwischen Modulen, Dev-Modus `.so/.dylib` via `libloading`
  mit stabilem C-ABI für Plugin-Registrierung, Prod-Modus Workspace-Eintrag.
- Generatoren `make:*`, `opengrid doctor`, `opengrid new`.
- Test-App: Modul `billing` als zweites Modul mit nativem Rust-Crate
  (eine `#[php_class]` im Modul selbst).

Abnahme: Job läuft im Hintergrund, Mail geht über SMTP-Testserver raus,
zweites Modul wird per `pkg` aus Git installiert und lädt sein Plugin.

### M7 — Performance (gemessen, nicht geraten)

- Profiling-Harness: Request-Durchsatz, VM-Zeit pro Request, Allokationen.
- Opcode-Cache pro Datei (bereits als Gerüst vorhanden), typisierte Opcodes
  für `int`/`float`/`string`-Pfade.
- Erst danach Cranelift für heiße Funktionen mit stabilen Typen; Feature
  `jit`, standardmäßig aus.

Abnahme: dokumentierter Benchmark gegen Laravel Octane und reines Loco mit
derselben Test-App.

### M8 — Zweiter Host (Beweis der Agnostik)

- `opengrid-axum`: minimaler Host ohne Loco (Axum + sqlx), implementiert die
  Traits aus §2.2.
- Dieselbe Test-App läuft dort ohne Änderung an `.grd`-Dateien.

Abnahme: Test-App-Feature-Tests grün auf beiden Hosts.

## 5. Integration des Loco-Forks: konkrete Schritte

1. `HeldvonKosmos/loco`: `upstream` auf `loco-rs/loco` setzen, Tag
   `upstream-1.2.0` auf den Fork-Stand. Branch `opengrid` für Patches,
   `master` folgt upstream.
2. Im neuen Workspace: `loco-rs = { git = "https://github.com/HeldvonKosmos/loco", branch = "opengrid" }`
   und `opengrid-core = { git = "https://github.com/HeldvonKosmos/phprs" }`
   (später crates.io oder Path-Deps in einem Monorepo, je nach Geschmack).
3. `opengrid-loco::App` implementiert `Hooks` einmal und endgültig. Die
   Anwendung schreibt keinen Rust-Code mehr, es sei denn in Modul-Crates.
4. Konfiguration: `config/{env}.yaml` ist Locos Format; der Block
   `settings.opengrid` trägt `modules_dir`, `workers`, `hot_reload`,
   `session.store`.
5. `fallback.enable: false` in der YAML (sonst überschreibt Locos
   Fallback-Middleware den Catch-all, `BRIDGES.md` §2).

## 6. Erwartete Patches am Loco-Fork

Klein halten, jeder Patch mit Upstream-PR-Potenzial:

| Patch | Warum | Alternative ohne Patch |
|---|---|---|
| `AppContext` nicht `#[non_exhaustive]` oder `with_extension()` | VM-Pool als Feld statt `shared_store` | `shared_store` nutzen (reicht für M0, kein Patch nötig) |
| `Hooks::routes` als `async fn` oder `routes_with_ctx` | Routen-Tabelle aus Skript laden braucht VM | Routen im `after_context` vorladen, `routes()` liefert nur Catch-all (kein Patch nötig) |
| Fallback-Middleware respektiert vorhandene Catch-all-Route | sauberer als `fallback.enable: false` | YAML-Schalter (kein Patch nötig) |
| `Task::run` mit `&str`-Name statt Trait-Objekt pro Task | dynamische Script-Tasks | je Klasse ein `ScriptTask`-Wrapper (kein Patch nötig) |
| `Mailer::mail_template` mit Laufzeit-Verzeichnis statt `include_dir` | Templates aus Modulen | `Email` direkt füllen (kein Patch nötig) |

Stand heute: **voraussichtlich kein Patch zwingend.** Der Fork ist eine
Versicherung, kein Arbeitsort.

## 7. Risiken und Gegenmaßnahmen

| Risiko | Gegenmaßnahme |
|---|---|
| `PhpValue` mit Raw-Pointern und `unsafe impl Send/Sync` | Worker-Pool mit Thread-Affinität; `OpArray` vor dem Teilen auf Pointer-freie Werte prüfen (Audit in M0) |
| Prozessweite `OnceLock`s (Opcode-Cache, Interner, Globals) | in M0 nach `Engine` verschieben oder hinter `experimental-jit` |
| Interpreter-Korrektheit (zwei Fundamentalfehler schon gefunden) | Conformance-Suite in M0 beginnen: je Sprachfeature ein Test; Test-App als Smoke-Test |
| Closures aus dem Dialekt in async Rust (Inertia `defer`, Queue) | Closures werden im Worker-Thread synchron ausgeführt; Ergebnis als `serde_json::Value` ans Future übergeben |
| inertia-omega ist Alpha, 6 Commits | eigener Fork, wenn es sich bewegt; Protokoll ist stabil und klein |
| `sea_query`-Builder deckt nicht alle Laravel-Methoden | `DB::raw()` als Ventil von Anfang an |
| Plugin-ABI (`libloading`) zwischen Rust-Versionen | C-ABI mit Versionsfeld, Dev-only; Prod statisch |

## 8. Nächste Schritte (diese Woche)

1. phprs: Cast-Syntax und Exception-Nachrichten fixen (`KNOWN_ISSUES` #1, #2),
   strikte Vergleiche, Warnings → Exceptions. Conformance-Tests dazu.
2. opengrid-Workspace anlegen mit `opengrid-host` (Traits, Null-Impl,
   Wertebrücke) und `opengrid-loco` (Hooks, Pool, Catch-all).
3. Test-App-Skelett in der Modules-Only-Struktur mit dem ersten Controller.
4. Messung: `GET /tasks` Requests/s gegen Laravel Octane auf derselben
   Maschine, Ergebnis in `BENCHMARKS.md`.
