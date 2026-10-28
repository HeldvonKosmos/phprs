# Deviations of the comparison apps (BENCH3)

**Deviations** — what each side does differently from the plain framework default, and why; all sides run the same raw SQL statements (no ORM), the same token check and answer byte-identically on the logic endpoints (hash table in bm2.md).

| Side | Deviation from the framework's default | Why |
|---|---|---|
| all | SQLite 3.45.2 with the reference compile options instead of the library a driver brings (Node: better-sqlite3 compiled from the same amalgamation with its `-D` options; Ruby: sqlite3 gem against the system library; Python, .NET: the library through `LD_LIBRARY_PATH`; Java: sqlite-jdbc 3.45.2.0 with its JNI part rebuilt against it; Phoenix: exqlite with `EXQLITE_USE_SYSTEM`) | the same database engine on every side, checked per target before measuring |
| all | journal_mode WAL, synchronous FULL, busy_timeout 5000 set per connection | the same durability everywhere |
| all | bcrypt cost 10; the seed stores hashes with the prefix `$2b$` (PHP's `$2y$` is the same algorithm) | Node's bcrypt rejects `$2y$` |
| all | Laravel's `required` semantics (an empty string or list fails) and Laravel's error form (keys, wording, order by rule then element, "(and N more errors)") | the same answer for the same invalid input (api.sh: rule test byte-identical on every side) |
| AdonisJS | the starter kit's session, shield and auth middleware not registered; raw SQL through Lucid's query client | the token check of the benchmark replaces auth on every side |
| NestJS | better-sqlite3 directly (Nest brings no database layer of its own); a cluster of N processes | Nest's documented way for a synchronous driver; one process per worker as on the other Node sides |
| Next.js | route handlers only, validation with zod; `next start` in a cluster of N processes | no pages (no frontend in the benchmark); zod is the common choice in Next apps |
| Rails | the pragmas of database.yml reset to SQLite's defaults (mmap_size 0, cache_size -2000, journal_size_limit -1); force_ssl off; JSON bodies parsed by Rails' params parser | Rails 8 sets its own tuning pragmas, the other sides run SQLite's defaults; no TLS in the benchmark |
| Django | persistent connections (CONN_MAX_AGE None); compact JSON separators; validation with a DRF serializer | Django otherwise opens a connection per request; the same bytes as the other sides |
| FastAPI | synchronous endpoints with one sqlite3 connection per thread | FastAPI's advice for a synchronous database library |
| Spring Boot | type checks before Bean Validation (to give Laravel's "must be a string/integer" messages); Hikari pool of 4; fixed heap 512 MiB, G1, `-XX:ActiveProcessorCount=N` | Jackson would coerce the types silently; the JVM sees as many cores as the others |
| ASP.NET Core | one SQLite connection per thread (pragmas set once per connection); type checks before DataAnnotations | Microsoft.Data.Sqlite's pool would run the pragmas on every open |
| Phoenix | the pragmas of ecto_sqlite3 reset to SQLite's defaults; queries through `Ecto.Adapters.SQL.query!`; validation with schemaless changesets | as Rails; raw SQL without schemas |

**Validate rule table** — the rules of `POST /api/bench/logic/validate` (Laravel's notation) and how each side expresses them; api.sh sends the same invalid input to every side and requires the identical error answer.

| Rule | OpenGrid, Laravel | AdonisJS | NestJS | Next.js | Rails | Django | FastAPI | Spring Boot | ASP.NET Core | Phoenix |
|---|---|---|---|---|---|---|---|---|---|---|
| `items` required, array | `required\|array` | `vine.array(…).minLength(1)` | `@IsArray @ArrayNotEmpty @ValidateNested` | `z.array(…).min(1)` | type check in the controller | `ListField(allow_empty=False)` | `list[Item]`, `min_length=1` | type check before Bean Validation | type check before DataAnnotations | list check in the controller |
| `items.*.title` required, string, max 120 | `required\|string\|max:120` | `vine.string().maxLength(120)` | `@IsString @MaxLength(120)` | `z.string().min(1).max(120)` | `presence`, string check, `length: { maximum: 120 }` | `StrictCharField(max_length=120)` | `StrictStr`, `min_length=1, max_length=120` | `@NotNull @Size(min = 1, max = 120)` | `[Required(AllowEmptyStrings = false), StringLength(120)]` | `cast` + `validate_required` + `validate_length(max: 120)` |
| `items.*.priority` required, integer, between 1 and 5 | `required\|integer\|between:1,5` | `vine.number().withoutDecimals().range([1, 5])` | `@IsInt @Min(1) @Max(5)` | `z.int().min(1).max(5)` | `presence`, `numericality: { only_integer: true, in: 1..5 }` | `IntegerField(min_value=1, max_value=5)` | `int`, `ge=1, le=5` | `@NotNull @Min(1) @Max(5)` | `[Required, Range(1, 5)]` | `cast` + `validate_number(1..5)` |
| `items.*.tags` array (optional) | `array` | `vine.array(…).optional()` | `@IsOptional @IsArray` | `z.array(…).optional()` | array check | `ListField(required=False)` | `Optional[list[…]]` | `List<…>` | check in the controller | check in the controller |
| `items.*.tags.*` string, max 20 | `string\|max:20` | `vine.string().maxLength(20)` | `@IsString({ each: true }) @MaxLength(20, { each: true })` | `z.string().max(20)` | per element check | `StrictCharField(max_length=20)` | `StrictStr`, `max_length=20` | `List<@Size(max = 20) String>` | per element check | per element check |
