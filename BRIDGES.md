# OpenGrid ↔ Loco Bridges (Design)

Basis: loco-rs **1.2.0** (Quelle `/home/claude/loco-src`), axum **0.8.9**, axum-extra 0.10.3,
tokio 1.52, sea-orm **2.0.0**, sqlx **0.9.0**, tower-http 0.6.11, http 1.4, MSRV **1.94**
(`Cargo.toml` `rust-version = "1.94"`, Edition 2024; Versionen aus `examples/demo/Cargo.lock`).
VM-Seite: `phprs` (`src/engine/types.rs`: `Val { value: PhpValue, type_info, u2 }`,
`PhpValue::{Long,Double,Bool,String,Array,Object,…}`; **kein `Null`-Variant** — Null ist
`type_info == IS_NULL`, Konstruktor `engine::facade::factory::null_val()`).

## 1. Loco-Lifecycle & `Hooks`

`src/app.rs:433` `pub trait Hooks: Send` (async_trait). Relevante Signaturen:

| Hook | Signatur (app.rs) | Zeitpunkt (boot.rs) |
|---|---|---|
| `boot` | `async fn boot(mode: StartMode, env: &Environment, config: Config) -> Result<BootResult>` | Einstieg, ruft `create_app::<Self, Migrator>` (boot.rs:447) |
| `load_config` | `async fn load_config(env: &Environment) -> Result<Config>` | vor `create_context` |
| `after_context` | `async fn after_context(ctx: AppContext) -> Result<AppContext>` | Ende von `create_context` (boot.rs:438), DB steht |
| `before_run` | `async fn before_run(ctx: &AppContext) -> Result<()>` | `run_app` (boot.rs:482) |
| `initializers` | `async fn initializers(ctx: &AppContext) -> Result<Vec<Box<dyn Initializer>>>` | danach |
| `before_routes` | `async fn before_routes(ctx: &AppContext) -> Result<AxumRouter<AppContext>>` | `setup_routes` (boot.rs:559) |
| `routes` | `fn routes(ctx: &AppContext) -> AppRoutes` | → `to_router::<H>(ctx.clone(), app)` |
| `middlewares` | `fn middlewares(ctx: &AppContext) -> Vec<Box<dyn MiddlewareLayer>>` | innerhalb `to_router` |
| `after_routes` | `async fn after_routes(router: AxumRouter, ctx: &AppContext) -> Result<AxumRouter>` | nach `to_router`, Router hat **keinen State mehr** (`Router<()>`) |
| `Initializer::after_routes` | `async fn after_routes(&self, router: AxumRouter, ctx) -> Result<AxumRouter>` | nach `Hooks::after_routes` |
| `connect_workers` | `async fn connect_workers(ctx: &AppContext, queue: &Queue) -> Result<()>` | nur bei `WorkerMode::BackgroundQueue` |
| `register_tasks` | `fn register_tasks(tasks: &mut Tasks)` | CLI `task` |
| `truncate` / `seed` / `dump` | `async fn truncate(ctx)`, `async fn seed(ctx, path: &Path)`, `async fn dump(ctx, base: &Path)` | `cargo loco db …`, `db::converge` |
| `on_shutdown` | `async fn on_shutdown(ctx: &AppContext)` | graceful shutdown |

`AppContext` (app.rs:257): `#[derive(Clone, FromRef)] #[non_exhaustive]` mit `environment`,
`db: DatabaseConnection`, `queue_provider`, `config: Config`, `mailer`, `storage`, `cache`,
`shared_store: Arc<SharedStore>`. Handler bekommen ihn als `State(ctx): State<AppContext>`;
`to_router` ruft `app.with_state(ctx)` (app_routes.rs:310).

**VM-Pool-Initialisierung**: in `after_context` — `ctx.shared_store.insert(VmPool::new(..))`
(`SharedStore::insert<T: 'static + Send + Sync>`, app.rs:66). Zugriff im Handler via
`ctx.shared_store.get_ref::<VmPool>()` oder dem Extractor
`SharedStore<T>` (`src/controller/extractor/shared_store.rs`, verlangt `T: Clone` → `Arc<VmPool>`
ablegen). Kein eigenes Feld in `AppContext` möglich (non_exhaustive; `into_builder()` nur für
bestehende Komponenten).

## 2. Dynamisches Routing

`Routes` (`src/controller/routes.rs:9`): `prefix: Option<String>`, `handlers: Vec<Handler { uri,
method: MethodRouter<AppContext>, actions }>`; `add(uri, MethodRouter)`, `get/post/…`, `layer(L)`,
`nest`, `merge`. `AppRoutes` (`app_routes.rs:24`) sammelt sie; `to_router` (app_routes.rs:278)
macht `app.route(uri, method)` pro Eintrag, dann Middlewares (LIFO-Zwiebel), dann `with_state`.

Loco kennt keine dynamische Dispatch-Schicht; wir registrieren **einen** Catch-all:

```rust
// src/bridge/dispatch.rs
pub struct RouteTable { routes: matchit::Router<ScriptAction> } // oder eigener Matcher
pub struct ScriptRouter(pub ArcSwap<RouteTable>);               // in shared_store

pub async fn dispatch(
    State(ctx): State<AppContext>,
    method: Method, uri: Uri, headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,   // oder Vec<(String,String)> für a[]=
    jar: CookieJar,                                  // axum_extra::extract::CookieJar
    req: Request,                                    // Body zuletzt: Multipart/Form/Json daraus
) -> Result<Response> { ... }

pub fn routes() -> Routes {
    Routes::new()
        .add("/", any(dispatch))
        .add("/{*path}", any(dispatch))   // axum 0.8 Syntax: `{*path}`; "/" matcht nicht
}
```
`Hooks::routes` liefert `AppRoutes::with_default_routes().add_route(bridge::routes())`
(`/_health`, `/_ping` aus `monitoring::routes()` bleiben statisch und gewinnen, da axum
spezifischere Routen bevorzugt). Alternative: `before_routes` →
`Ok(Router::new().fallback(dispatch))` — aber `fallback`/`static_assets`-Middleware rufen
`app.fallback(..)`/`app.fallback_service(..)` (fallback.rs:94, static_assets.rs:127) und
würden ihn **überschreiben**; daher: `fallback.enable: false` in YAML und Catch-all-Route statt
Router-Fallback. Static Assets mit `folder.uri != "/"` (`nest_service`) bleiben kompatibel.

Middleware-Reihenfolge (`middleware/mod.rs:86` `default_middleware_stack`, Vec-Reihenfolge =
innen→außen): limit_payload, cors, catch_panic, etag, remote_ip, compression, timeout_request,
static_assets, secure_headers, logger, request_id, fallback, powered_by. Anpassen über
`Hooks::middlewares` + `MiddlewareStackExt` (`insert_before/after/replace/delete`).

## 3. Request → `Val`

Loco exportiert (`src/prelude.rs`): `axum::extract::{Form, Multipart, Path, Query, State}`,
`axum_extra::extract::cookie` (`Cookie`, `CookieJar`), eigenes `loco_rs::controller::Json<T>`
(mod.rs:174, Rejection → `Error`), Validierungs-Extractors (`JsonValidate`, `FormValidate`,
`QueryValidate`, `extractor/validate.rs`), `Format`/`RespondTo` (Header-Sniffing,
`middleware/format.rs`), `RemoteIP`, `auth::JWT`/`ApiToken`. **Nicht vorhanden**: kein
`src/request_context/`, kein `RequestContext`, keine Sessions, kein Flash (grep leer).

Im Catch-all extrahieren wir nicht typisiert, sondern bauen `$request` als Array:

| Quelle (axum) | PHP-Schlüssel | PhpValue |
|---|---|---|
| `Method` | `method` | String |
| `Uri::path()` / `.query()` | `path`, `query_string` | String |
| `Query<Vec<(String,String)>>` (serde_urlencoded) | `query` | Array\<String,String\>, `k[]` → Array-Liste |
| Route-Params (eigener Matcher, nicht `Path`) | `params` | Array\<String,String\> |
| `HeaderMap` | `headers` | Array\<lowercase name, String\>; Mehrfachwerte → Liste |
| `CookieJar` | `cookies` | Array\<String,String\> |
| Body `application/json` → `serde_json::Value` | `json` | rekursiv: Object→Array(String-Keys), Array→Array(Long-Keys), Number→Long/Double, Bool, Null→`null_val()` |
| Body `x-www-form-urlencoded` → `Form<Vec<(String,String)>>` | `input` | wie query |
| Body `multipart/form-data` → `Multipart` | `input`, `files` | Felder String; Files Array{name,filename,mime,tmp_path,size} (zu `ctx.storage`/tmp schreiben) |
| Roh-Body (`Bytes`) | `raw_body` | String (binary-safe) |
| `ConnectInfo<SocketAddr>` / `RemoteIP` | `ip` | String |
| `State(ctx)` | – | nicht ins Skript; bleibt Host-seitig |

Konvertierer: `fn json_to_val(&serde_json::Value) -> Val` und `fn val_to_json(&Val) ->
serde_json::Value` (für Props/JSON-Antworten); Arrays mit lückenloser `0..n`-Long-Key-Folge →
JSON-Array, sonst Objekt (PHP-Semantik).

## 4. Response ← `Val`

`src/controller/format.rs`: `text`, `json<T: Serialize>`, `html`, `yaml`, `empty`,
`redirect(to)` (→ **303 See Other** + `Location`, format.rs:410), `view`, `template`, sowie
`render() -> RenderBuilder` mit `.status(..)`, `.header(k,v)`, `.etag(..)`,
`.cookies(&[Cookie])` (setzt `Set-Cookie`, format.rs:271), Terminals `.html/.json/.text/.redirect/
.empty`, Escape-Hatch `.response() -> http::response::Builder`. Fehler: `loco_rs::Error` ist
`IntoResponse` (controller/mod.rs:~200, `ErrorDetail` JSON), `not_found()`, `bad_request()`,
`unauthorized()`, `Error::CustomError(StatusCode, ErrorDetail)`.

Mapping des Skript-Rückgabewerts (Konvention der OpenGrid-Runtime, in Rust `enum ScriptResponse`):

| Skript gibt zurück | Rust | Antwort |
|---|---|---|
| String | `format::render().status(s).html(&s)` | `text/html` |
| Array (ohne `__type`) | `format::render().json(val_to_json(v))` | `application/json` |
| Objekt/Array `{__type:"response", status, headers, cookies, body, content_type}` | `RenderBuilder` Kette: `.status(u16)`, `.header(k,v)` je Eintrag, `.cookies(&[Cookie::build(..)])`, dann `.html/.json/.text` je `content_type` | beliebig |
| `{__type:"redirect", to, status?}` | `format::render().status(status.unwrap_or(302)).redirect(to)` (Achtung: `redirect()` erzwingt 303 → bei 301/302 `.response().status(..).header(LOCATION, to).body(Body::empty())`) | Redirect |
| `{__type:"inertia", component, props}` | siehe §7 | Inertia |
| `{__type:"view", template, data}` | `format::view(&ViewEngine<TeraView>, template, data)` (Extension aus `ViewEngineInitializer`) | Tera-HTML |
| Null | `format::empty()` → 200 leer | – |
| Skript wirft Exception | `Err(Error::CustomError(500, ErrorDetail::new("script_error", msg)))`; in Dev zusätzlich Stacktrace | 500 |

Cookies: `axum_extra::extract::cookie::Cookie::build((name, value)).path(..).http_only(..)
.same_site(..)`; der Request-`CookieJar` kann alternativ zurückgegeben werden
(`(jar, response).into_response()`).

## 5. DB-Bridge

`ctx.db: sea_orm::DatabaseConnection` (app.rs:265), verbunden in `src/db/connect.rs:96`
(`ConnectOptions`, Pool-Parameter aus `config.database`). Rohzugriff in sea-orm 2.0 (so von Loco
selbst genutzt, `src/db/introspect.rs`, `src/tests_cfg/db.rs`):
`db.query_all_raw(Statement) -> Vec<QueryResult>`, `db.query_one_raw(..)`,
`db.execute_raw(Statement) -> ExecResult` (`rows_affected()`, `last_insert_id()`),
`Statement::from_sql_and_values(backend, sql, Vec<sea_orm::Value>)`,
`db.get_database_backend()`, `row.column_names()`, `row.try_get::<T>("", col)`.
`db.inner` ist `DatabaseConnectionType` (connect.rs:66: `SqlxPostgresPoolConnection(_)`), d.h. der
sqlx-Pool ist erreichbar; sea-orm-1.x-Getter `get_postgres_connection_pool()` /
`get_sqlite_connection_pool()` konnte ich lokal nicht verifizieren (sea-orm-Quelle nicht im
Registry-Cache) — vor Nutzung prüfen. Loco's Queue-Worker (`bgworker/pg.rs`, `sqlt.rs`) öffnet
einen **eigenen** sqlx-Pool; wir brauchen keinen zweiten: alles über `ctx.db`.

Host-Funktionen (PHP-Signaturen): `db_query(string $sql, array $bindings = []): array`,
`db_execute(string $sql, array $bindings = []): array{affected:int,last_id:?int}`,
`db_scalar(...)`, `db_transaction(callable)` (später, benötigt `TransactionTrait::begin`).

```rust
async fn db_query(db: &DatabaseConnection, sql: &str, binds: Vec<Val>) -> Result<Val> {
    let values: Vec<sea_orm::Value> = binds.iter().map(val_to_sea_value).collect(); // Long→BigInt,
    // Double→Double, Bool→Bool, String→String(Some(Box)), Null→String(None), Array→Json
    let stmt = Statement::from_sql_and_values(db.get_database_backend(), sql, values);
    let rows = db.query_all_raw(stmt).await?;
    let mut out = array_val();
    for row in rows {
        let mut r = array_val();
        for col in row.column_names() { r.insert_string(&col, column_to_val(&row, &col)); }
        out.push(r);
    }
    Ok(out)
}
```
Platzhalter: Postgres `$1..`, SQLite `?` — ein Rewriter `?`→`$n` für PG (wie Laravel) ist Pflicht,
da Skripte portabel sein sollen. `column_to_val` als Probierkette wie introspect.rs:224ff:

| DB-Typ (try_get Reihenfolge) | PhpValue |
|---|---|
| NULL (`try_get::<Option<..>>` → None) | `null_val()` |
| bool (nur PG nativ; SQLite liefert i64 — Schema-Info nötig, vgl. `get_boolean_columns`) | Bool |
| i8/i16/i32/i64 | Long |
| f32/f64, Decimal (→ f64 oder String bei Präzision) | Double / String |
| String/text, Uuid (`to_string()`) | String |
| chrono `DateTime<Utc>`/`NaiveDateTime`/`NaiveDate` | String (RFC3339 / `Y-m-d H:i:s`) |
| `serde_json::Value` (json/jsonb) | rekursiv Array |
| `Vec<u8>` (bytea/blob) | String (binary) |

Ohne Spaltentyp-Metadaten in `QueryResult` ist die Reihenfolge der Versuche entscheidend
(Loco probiert JSON vor String — für uns String vor JSON, sonst wird `'{"a":1}'`-Text zu Array).
Für Postgres kann alternativ über den sqlx-Pool `PgRow::columns()[i].type_info()` gelesen werden
(exakte Typen) — Empfehlung für v2, v1 nimmt die Probierkette.

**Sync/Async**: die VM ist synchron, der Handler asynchron. Zwei Optionen:
(a) `tokio::task::spawn_blocking(move || vm.run(..))` pro Request, Host-Funktionen rufen
`tokio::runtime::Handle::current().block_on(db.query_all_raw(..))` auf dem Blocking-Thread
(erlaubt, da kein Worker-Thread blockiert wird). (b) fester Worker-Thread-Pool
(`N = cores`) mit `crossbeam`/`mpsc`-Kanal `Job { request: Val, reply: oneshot::Sender<Val> }`,
jeder Worker hält ein vorgewärmtes `ExecuteData` + `Handle` für `block_on`.
**Empfehlung: (b)**. Gründe: (1) `PhpValue` enthält rohe Pointer (`Ptr`, `ClassEntry`,
`Function`) mit `unsafe impl Send/Sync` — ein VM-Zustand soll den Thread nie wechseln;
`spawn_blocking` streut ihn über einen unbegrenzten Thread-Pool (Default 512). (2) Wir wollen
geladene/kompilierte `.grd`-OpArrays pro Thread cachen (Opcode-Cache `engine/opcode_cache.rs`),
was einen stabilen Thread verlangt. (3) Backpressure: Kanal-Kapazität = natürliche
Begrenzung, Timeout via `tokio::time::timeout` auf der `oneshot`. Der Handler macht
`pool.send(job).await?; reply.await?`. `block_on` in Workern ist sicher, weil Worker keine
Tokio-Threads sind; DB-Pool `max_connections` ≥ Worker-Anzahl setzen.

## 6. Migrationen / Seeds

Loco migriert über das `migration`-Crate des Starters (`examples/demo/migration/src/lib.rs`:
`pub struct Migrator; impl MigratorTrait { fn migrations() -> Vec<Box<dyn MigrationTrait>> }`),
übergeben an `create_app::<Self, Migrator>`; Ausführung in `src/db/migrate.rs`
(`converge` → `auto_migrate`/`dangerously_recreate`/`dangerously_truncate`, `migrate = M::up`,
`down`, `status`, `reset = M::fresh + up`); CLI `cargo loco db migrate|reset|status|entities|seed`
(`boot.rs:324 run_db`). Seeds: `Hooks::seed(ctx, base)` → `db::seed::<ActiveModel>(&ctx.db,
"users.yaml")` (seed.rs:42, YAML → Model → `insert_many`, `reset_autoincrement`);
`Hooks::truncate` → `db::truncate_table(&ctx.db, Entity)`.

OpenGrid `database/`: `database/migrations/*.grd` (Schema-Builder-DSL im PHP-Dialekt) werden
**nicht** in sea-orm-Migrationen übersetzt, sondern von einem einzigen Rust-`MigrationTrait`
ausgeführt: `struct ScriptMigration(PathBuf)` → `up()` führt das Skript in einer VM aus, deren
Host-Funktionen (`schema_create`, `schema_table`, `db_execute`) auf `manager.get_connection()`
arbeiten; `Migrator::migrations()` listet das Verzeichnis zur Laufzeit (Name = Dateiname, so
bleibt `seaql_migrations`-Tracking intakt). `database/seeders/*.grd` → `Hooks::seed` iteriert
Verzeichnis und führt aus; `Hooks::truncate` liest Tabellenliste aus `settings.opengrid.truncate`.

## 7. Inertia

`Hooks::after_routes(router: AxumRouter, ctx)` **existiert** (app.rs:527) und erhält den
Router bereits mit State; dort: `Ok(router.layer(InertiaLayer::new(cfg)))`. Da das Layer die
gesamte App umschließt, laufen Loco-Middlewares (bis auf `powered_by` etc., siehe Vec-Ordnung)
innerhalb — unproblematisch. Benötigt inertia-omega eine Session (XSRF, Flash, `errors`-Prop),
muss **wir** sie stellen: Loco liefert keine `tower-sessions`-Integration (nicht in `Cargo.toml`,
nicht in `src/`). Vorschlag: `tower_sessions::SessionManagerLayer` mit eigenem Store
(SQLite/PG-Tabelle über `ctx.db`, oder `tower-sessions-sqlx-store` mit dem Pool aus `db.inner`)
ebenfalls in `after_routes`, **vor** `InertiaLayer` angewendet (d.h. `.layer(session)` nach
`.layer(inertia)` aufrufen, da axum LIFO). Im Catch-all `Inertia`-Extractor als Parameter
aufnehmen (`inertia: Inertia`), Rückgabe `{__type:"inertia"}` →
`inertia.render(component, val_to_json(props))` (Props sind `serde_json::Value`, erfüllt
`Serialize`). Shared Props (`auth.user`, `flash`) werden host-seitig gemerged; Lazy-Props erst
in v2. Versions-Hash (`X-Inertia-Version`) aus `manifest.json` im `static_assets`-Ordner.

## 8. Config & Env

`Environment` (`src/environment.rs:39`): `Production|Development|Test|Any(String)`, aus
`LOCO_ENV` (Fallback `RAILS_ENV`, `NODE_ENV`, Default `development`); Config-Ordner `config/`
oder `LOCO_CONFIG_FOLDER` (`env_vars.rs:14`); Dateien `{env}.yaml` + optional `{env}.local.yaml`
(deep-merge, `config/mod.rs:175 from_folder`); Templating `<%= get_env(name="X", default="y") %>`
(`config/template.rs`). `Config` (`config/mod.rs:64`, non_exhaustive): `logger, server
{binding, port, host, ident, middlewares}, database {uri, max_connections, auto_migrate, …},
cache, queue, auth, workers, mailer, initializers: BTreeMap<String, Value>, settings:
Option<serde_json::Value>, scheduler`. Zugriff: `let s: OpenGridSettings =
ctx.config.settings()?;` (`Config::settings<T: DeserializeOwned>`, mod.rs:133; fehlender Block =
leeres Objekt, fehlerhafter Block = Error).

```yaml
settings:
  opengrid:
    script_root: app            # controllers/, models/ (.grd)
    modules_dir: modules
    routes_file: routes/web.grd
    vm_workers: 8
    hot_reload: true
```

## 9. Hot Reload

Loco hat nichts Generisches; einzig `TeraView::from_custom_dir` (`controller/views/engine.rs:144`)
benutzt `notify::recommended_watcher` (notify 8.1 ist bereits Dependency) mit Dirty-Flag unter
`#[cfg(debug_assertions)]` — dasselbe Muster übernehmen wir: in `after_context` (oder
`Initializer::before_run`) einen `notify`-Watcher auf `script_root` + `routes_file` starten; bei
Events `RouteTable` neu laden und `ArcSwap::store`, Opcode-Cache der Worker invalidieren
(Generation-Counter im `VmPool`, Worker vergleichen pro Request). Nicht `connect_workers`
(läuft nur im Queue-Modus). Watcher-Handle im `shared_store` halten, sonst wird er gedroppt.

## 10. Offene Risiken

- **Send/Sync**: alles im `shared_store` muss `Send + Sync + 'static` sein; `PhpValue` erfüllt
  das nur per `unsafe impl` (types.rs:181) mit rohen Pointern → `Val` darf nie Threads wechseln;
  nur rein-datengetragene `Val`s (keine Objekte/Closures) über den Kanal reichen, besser:
  Transportformat `serde_json::Value`/eigenes `BridgeValue` und erst im Worker in `Val` wandeln.
- **Versionspins**: axum 0.8 (`{*path}`-Syntax, `Router::fallback`), axum-extra 0.10 (`CookieJar`),
  sea-orm 2.0 (`query_all_raw`/`execute_raw` statt 1.x `query_all`/`execute`), sqlx 0.9;
  inertia-omega und tower-sessions müssen gegen **axum 0.8 / http 1 / tower 0.5** kompilieren —
  prüfen, sonst zwei axum-Versionen im Baum (Typ-Inkompatibilität der Layer).
- **MSRV 1.94 / Edition 2024** (von sea-orm 2.0 erzwungen); phprs steht auf Edition 2024, ok.
- `AppContext`/`Config` sind `#[non_exhaustive]` — keine Struct-Literale, nur Builder/Hooks.
- `redirect()` fixiert 303; Status muss über `.response()` gesetzt werden.
- Catch-all kollidiert mit `fallback`- und `static_assets`-Middleware bei `uri: "/"`; in YAML
  `fallback.enable: false` und Assets unter `/assets` nesten.
- `block_on` in VM-Workern setzt voraus, dass Worker **keine** Tokio-Threads sind; nie aus einem
  `async fn` heraus `block_on` aufrufen (Panic „Cannot start a runtime from within a runtime“).
- `last_insert_id()` ist in sea-orm nur für SQLite/MySQL sinnvoll; für PG `RETURNING id` nutzen.
- Panics in der VM werden von `catch_panic` (tower-http) zu 500 abgefangen, aber ein Worker-Thread
  mit Panic ist tot → Worker müssen panics via `catch_unwind` abfangen und sich neu initialisieren.
