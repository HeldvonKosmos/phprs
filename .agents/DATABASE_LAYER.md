# Database layer: SurrealDB natively in Rust, instead of SeaORM

As of 2026-10-23. The layer consists of a model base class, a connector,
a query builder, a filter compiler, aggregates and side tables. Replaces
the SeaORM/`sea_query` plan in `ROADMAP.md` M3. Loco's `with-db` feature
stays off; `AppContext.db` is not used.

## 1. Decision: the layer is Rust, the model is a declaration

The entire query and persistence layer is **written in Rust** and built
into the framework as native classes. A dialect model only declares what
it is:

```php
namespace MasterData\Models;

use OpenGrid\Database\Model;

class ClientAdditionalHour extends Model
{
    protected static string $table = 'masterdata_client_additional_hours';
    protected bool $softDeletes = true;
    protected array $dates = ['valid_from', 'valid_until'];
    protected array $fillable = ['reason', 'quantity', 'valid_from', 'valid_until', 'past', 'client_id'];
}
```

A model file contains nothing more than this declaration. Everything the
model *does* (`where`, `filter`, `sort`, `with`, `withAll`, `withSum`, `get`,
`first`, `find`, `paginate`, `create`, `update`, `delete`, `relate`,
`toArray`) comes from the native base class.

Reasons for Rust instead of a dialect library:

- Pagination, route-model binding, validation rules (`exists:`, `unique:`),
  policies and later live queries use the same builder internally, without
  re-entering the VM.
- String assembly, binding management and type mapping are cheap and
  type-safe in Rust (`RecordId` stays `RecordId`, no guessing of record casts).
- One implementation for framework internals and application code; the
  semantics are fixed by a reference implementation and testable through
  `toSql()` comparisons.

What stays in the dialect: the declaration (above), optional overrides
(`toArray()`, scopes as methods `scopeActive(Builder $q)`, hooks
`creating()`/`saved()`), and closures passed to the builder in
`withAll`/`withSum`/`when`.

## 2. Native classes (crates/framework/src/native/database/)

```
database/
├── surreal.rs        Surreal: connection(s), query(), transaction(), live() (M6), tracing span per query
├── value.rs          Val ↔ surrealdb::sql::Value; RecordId, Datetime, Duration, Decimal, Uuid, Geometry, Bytes
├── builder.rs        Builder  (query builder)
├── filter.rs         FilterCompiler (plus $filterable whitelist)
├── aggregates.rs     Aggregates (one grouped query per metric)
├── side_tables.rs    SideTables (withAll with sub-query closure)
├── model.rs          Model base class (including attribute, persistence and route-binding concerns)
├── schema.rs         Schema::sql (generates DEFINE statements), hasTable/hasColumn/columns (read); build_surql()
└── schema/           declarative schema management (DECISIONS "Schema management declarative", D4, R4)
    ├── sources.rs    sources in fixed order: framework.surql → database/sources/main.surql
    │                 → all modules/*/database/schema/blueprint.surql → all …/manually.surql
    ├── parser.rs, ast.rs, merge.rs, generator.rs   merge → database/schema/temporary.surql
    ├── dbinfo.rs     INFO FOR DB/TABLE → the same model (migrate_pull)
    ├── framework.surql  the framework's own tables (_opengrid_sessions)
    ├── kit.rs        SurrealKit embedded: sync, rollout baseline/plan/start/complete/rollback/status
    ├── preflight.rs  rollouts additive only: manifest rules + engine comparison
    └── ops.rs        migrate_push / migrate_pull / migrate_fresh, plan from the sources, production start
```

There is **no imperative migration runner** (removed 2026-10-24, commit 9504393). Development:
`opengrid task migrate_push` (sync without checks), `migrate_pull`, `migrate_fresh force:true [keep_db:true]`,
`rollout cmd:plan name:<n>` (plans from the sources, preflight). Production: server start only executes
planned rollouts after a passed preflight; errors → exit ≠ 0. Surrealist and manual adjustments in
`manually.surql` run against the development database or the server; connection via
`settings.opengrid.database`, `OPENGRID_DB_URL` or the `SURREAL_DB_*` variables.

### 2.1 `Builder` (Rust)

```rust
pub struct Builder {
    table: String,
    wheres: Vec<String>,
    bindings: BTreeMap<String, sql::Value>,   // typed, no longer just strings
    orders: Vec<String>,
    order_projections: BTreeMap<String, String>, // dot path → alias for sub-select
    fetch: Vec<String>,
    projection: Option<String>,
    groups: Vec<String>,
    soft_deletes: bool,
}
impl Builder {
    pub fn new(table: &str, soft_deletes: bool) -> Self;        // adds `deleted_at IS NONE`
    pub fn r#where(&mut self, field: &str, value: sql::Value, op: &str) -> &mut Self;
    pub fn where_raw(&mut self, clause: &str, bindings: BTreeMap<String, sql::Value>) -> &mut Self;
    pub fn where_in(&mut self, field: &str, values: Vec<sql::Value>) -> &mut Self;
    pub fn where_like(&mut self, field: &str, value: &str) -> &mut Self;   // string::contains(string::lowercase(<string>f), …)
    pub fn filter(&mut self, filters: &Val, allowed: &[String]) -> Result<&mut Self, Throw>;
    pub fn with_trashed(&mut self) -> &mut Self;
    pub fn sort(&mut self, spec: &str) -> &mut Self;            // "field:desc,other" with COLLATE
    pub fn order_by(&mut self, field: &str, dir: &str) -> &mut Self;
    pub fn select(&mut self, projection: &str) -> &mut Self;
    pub fn group_by(&mut self, fields: &[&str]) -> &mut Self;
    pub fn fetch(&mut self, field: &str) -> &mut Self;
    pub fn to_sql(&self, per_page: usize, offset: usize) -> String; // LIMIT/START before FETCH, FETCH last
    pub fn count_sql(&self) -> String;                           // SELECT count() … GROUP ALL
    pub fn bindings(&self) -> &BTreeMap<String, sql::Value>;
}
```

The builder semantics are fixed in detail (sub-select for dot-path sorting,
`GROUP ALL`, projection without order sub-select, binding names
`f0, f1, …`). **Acceptance: for the chains stored in
`tests/database/builder_cases.php`, `to_sql()` is byte-identical to the
expected output, verified against a private reference implementation (not
part of this repository).**

Deliberate deviations from the conventional approach:

| Alternative | Rust | Reason |
|---|---|---|
| guessing record casts from the field name `_id` and a colon in the value | dropped; the type of the value determines the binding (`RecordId` object → record, `Decimal` → decimal, `DateTimeInterface` → datetime, `null` → NONE). Strings stay strings, even `"tasks:abc"`; whoever means a record ID uses `RecordId::parse()`/`make()`. Only filter values from the request are converted according to the field type from `INFO FOR TABLE` (DECISIONS 2026-10-24 "SurrealDB types native" items 3–4) | bindings are typed, no guessing |
| `$dates` plus cast on write | the database delivers `DateTimeImmutable` (native core class); `$dates` serves to cast string inputs (forms) on write | ditto |
| `request()->input('filters'/'sort')` as default | `filter()`/`sort()` without an argument read the current `Request` from the request context; without a request (job, task) empty | no global helper |
| `Str::singular` for `fetchField` | own pluralisation rule in Rust (English, exception list) | no Illuminate |
| field names from `?filters[...]` unchecked into the query | `$filterable` on the model (whitelist); without a declaration: only `$fillable` + `id` + dot paths to fetched fields; otherwise `GRD` runtime error | close a security hole |
| `catch 'already exists' → []` in the connector | belongs in `Schema::define…IfNotExists` | idempotence in the right place |
| a query collector service in the container | `tracing` span `surreal.query` with sql, ms, bindings (redacted) | debugging without a container |

### 2.2 `Model` (Rust base class, inheritable from the dialect)

The base class reads its metadata **from the dialect subclass**, once per
class on first access, then cached in the engine:

```rust
pub struct ModelMeta {
    class: ClassId,
    table: String,                     // static::$table
    fillable: Vec<String>, hidden: Vec<String>, dates: Vec<String>, filterable: Vec<String>,
    relation: Option<(String, String)>,// static::$relation = ['in' => …, 'out' => …] (edge)
    soft_deletes: bool,
    overrides: ModelOverrides,         // has the subclass defined toArray()/hooks/scopes?
}
```

Static entry points (`Model::where(...)`, `::find()`, `::select()`,
`::raw()`, `::create()`) return a `Builder` bound to the class that finally
creates instances **of the subclass** (`new static`).
Instance methods (`save`, `update`, `delete`, `relate`, `toArray`,
`jsonSerialize`) work on `$attributes` in the native payload.
If the subclass overrides `toArray()` or defines
`scopeX(Builder $q)`/`creating()`/`saved()`, the base class calls them via
`call_method`.

Attributes: `$model->reason` reads from the payload (lazily from the
Surreal record), `$model->reason = …` writes into the payload and records
the dirty state. `$model->id` is a `RecordId`.

### 2.3 What the engine API must provide for this (input for NATIVE_CLASSES.md and Option B phase 6)

1. Native base class from which dialect classes inherit; `new static` creates
   the subclass with the native payload of the base.
2. **Reading declared properties of the subclass from Rust**:
   `engine.class_meta(class_id)` returns static and instance defaults
   (`$table`, `$fillable`, …) with types. This is new and must go into the API.
3. `engine.has_method(class_id, "toArray")`, `call_method(this, "toArray")`
   for overrides and hooks.
4. `Callable::invoke(&[Val]) -> Result<Val, Throw>` for closures in
   `withAll`, `withSum`, `when`, `transaction`; the closure receives a
   native `Builder` object and mutates it (object identity!).
5. Property handlers get/set/isset/unset on native objects (for
   `$model->field`), with fallback to dialect-side `__get`/`__set`.
6. `FromVal`/`IntoVal` for `surrealdb::sql::Value` including the type classes.

### 2.4 `Surreal`

One connection per worker from a pool, `block_on` on the async SDK.
Operating modes `mem://`, `rocksdb://` (tests, development without a server),
`ws://`/`http://` (an external server instance).
`transaction(Closure)` runs the closure in the worker between `BEGIN` and
`COMMIT`/`CANCEL`.

## 3. Consequences for other documents

- `ROADMAP.md` M3: "Database: native database layer in Rust
  (`native/database/`), `Model` as an inheritable native base class, declarative schema
  with embedded SurrealKit (no migration runner), SQLite/Postgres dropped". Acceptance: the test app uses
  a model exactly in the form of `ClientAdditionalHour`.
- `NATIVE_CLASSES.md`: adopt §2.3 above as requirements; `class_meta`
  is the most important addition.
- `PROMPT_OPTION_B.md` phase 6: the native test class is not `Counter`,
  but a minimal `Model` with `$table`/`$fillable` and a `Builder`
  whose `toSql()` is checked against `builder_cases.php`.
- `DIALECT.md` §6: `OpenGrid\Database\{Model, Builder, Surreal, RecordId, …}`
  into the table of native classes, `Model` and `Builder` as inheritable and
  mutable from closures respectively.
- `LOCO_VS_LARAVEL.md` §5 item 5 points here.

## 4. Test material

- `tests/database/builder_cases.php`: ten to twenty builder chains with
  expected SurrealQL (generated from a private reference implementation,
  not part of this repository). Rust unit tests compare `to_sql()` byte for byte.
- `tests/database/model_cases.grd`: `ClientAdditionalHour` and an
  edge model against `mem://`: create, find, filter, sort, paginate, withSum,
  withAll with closure, softDelete/withTrashed, relate.

## 5. Open points

- The remaining concerns (persistence, attributes, route-model binding) and
  the `Blueprint ModelGenerator`: to be completed in full, they are part of
  the layer.
- `relate()`/graph traversal: how are edges written and read?
- Live queries, events, permissions/scopes in SurrealDB: needed?
- Transactions spanning several queries?
- Which fields are actually filtered in practice (for the
  `$filterable` defaults)?
