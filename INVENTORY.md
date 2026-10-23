# Inventur phprs → Fork-Grundlage

Stand: phprs 0.1.19, Commit `a616a5e` (upstream HEAD beim Klonen, 2026-10-23).
Nichts im Repo wurde verändert, nur diese Datei wurde ergänzt.

## 1. Baseline

Siehe Abschnitt 6 (Testlauf `cargo test --workspace --no-fail-fast`).

## 2. Größe und Struktur (Rust-Zeilen)

| Bereich | Zeilen | Anmerkung |
|---|---|---|
| `src/engine/vm/` | 15.120 | davon `dispatch_handlers.rs` 6.061, `builtins.rs` 3.721, `builtin_capability_tests.rs` 1.948 (Tests), `tests.rs` 675 |
| `src/php/` | 6.997 | Builtin-Module (datetime 828, hash 799, math 489, url 448, serialize 443, streams 401, mbstring 400, csv 363, regex 349, ...) |
| `src/engine/*.rs` (Top-Level) | 5.448 | types 538, opcode_cache 521, function_optimizer 517, benchmark 499, array_ops 451, jit 403, perf_alloc 371, ... |
| `src/engine/compile/` | ca. 5.100 | expression 2.035, statement 1.668, Rest 1.433 |
| `src/engine/lexer/` | 1.184 | |
| `src/engine/facade/` | 615 | |
| `bin/phprs/src/` | 892 | `server.rs` 795, `main.rs` 97 |
| `bin/phprs/src/pkg/` | ca. 1.900 | composer 734, commands 453, registry 355, resolver 202, Rest 159 |
| `tests/*.rs` | 4.619 | 17 Testdateien + `fixtures/` |

Gesamt 192 `.rs`/`.php`-Dateien in `src`, `bin`, `tests`, `examples`.

Der Kern (Lexer, Compiler, VM) ist klein und überschaubar. Der größte Brocken ist `dispatch_handlers.rs` mit 6.061 Zeilen.

## 3. Abhängigkeiten zwischen Modulen

`engine` ruft `php` auf (Builtins), die Gegenrichtung ist schwach (13 Dateien in `src/php` verweisen auf `engine`).

Referenzen von `src/engine` auf `src/php` nach Modul:

| Modul | Referenzen | Modul | Referenzen |
|---|---|---|---|
| output | 35 | url | 8 |
| math | 20 | compression | 6 |
| sqlite | 18 | session | 5 |
| hash | 13 | regex | 4 |
| datetime | 13 | serialize, openssl, csv | je 3 |
| filesystem | 11 | streams, http_stream, curl | je 1 |
| mbstring | 10 | xml | 8 |

Folgerung: `sqlite`, `session`, `curl`, `openssl`, `http_stream`, `streams`, `xml` und `csv` haben wenige Berührungspunkte und lassen sich per Cargo-Feature abtrennen. `output` (35) und `math` (20) sind eng verflochten, `output` bleibt im Kern.

## 4. Host-Schnittstelle (Voraussetzung für M1)

Das ist der wichtigste Befund für den weiteren Plan.

**Builtin-Funktionen haben keine Registry.**
- `is_builtin_function` (`vm/builtins.rs:321`) ist ein einziges `matches!` über Funktionsnamen.
- `execute_builtin_function` (`vm/builtins.rs:396`) ist ein riesiges `match name { ... }`.
- Aufrufstellen: `callable.rs:48`, `dispatch_handlers.rs:654, 940, 957`, `reflection.rs:497`, `builtins.rs:603, 1997, 2128, 2150`.
- Host-Funktionen wie `db_query` lassen sich damit nicht von außen registrieren.
- Naheliegende Lösung: `HashMap<&'static str, NativeFn>` in `ExecuteData` (oder einer geteilten Registry) mit `NativeFn = fn(&[Val], &mut ExecuteData) -> Result<Option<Val>, String>`, die vor dem `match` konsultiert wird. Das alte `match` bleibt zunächst als Fallback. Es sind wenige Aufrufstellen betroffen.

**Klassen können nur PHP-Bytecode enthalten.**
- `ClassMethod` (`types.rs:516`) hat `op_array: OpArray` und keine native Variante.
- Außerhalb von `types.rs` kommt `ClassMethod` an nur 4 Stellen vor, aber `.op_array` wird in `dispatch_handlers.rs` an ca. 71 Stellen verwendet, ein Teil davon betrifft Methoden.
- Naheliegende Lösung: `enum MethodBody { Php(OpArray), Native(NativeMethod) }` und die Aufrufstelle für Methoden verzweigen lassen.

**Objekte haben keinen nativen Payload.**
- `PhpObject` (`types.rs:410`) besteht aus `class_name: String` und `properties: HashMap<String, Val>`.
- Für `DataFrame`, DB-Handle oder Request-Objekt in Rust braucht es ein Feld wie `native: Option<Box<dyn Any + Send>>`.
- `PhpObject::new` bzw. `PhpObject {` kommt an 33 Stellen vor.

**Eingebaute Klassen sind hartkodiert.** `Fiber`, `Generator` und `Reflection*` werden in `dispatch_handlers.rs` (9 Treffer), `exception_dispatch.rs` (2) und `reflection.rs` (3) über den Klassennamen als String behandelt. Das ist das Muster, das durch die Native-Registry ersetzt werden sollte.

**Vererbung von nativen Klassen** funktioniert über `ClassEntry.parent_name` (String) und `class_table: HashMap<String, ClassEntry>`. Eine native Basisklasse kann also als normaler `ClassEntry` mit nativen Methoden vorab in die `class_table` eingetragen werden, wenn die Methodenauflösung den nativen Body kennt.

## 5. Globaler Zustand (relevant für mehrere VMs pro Prozess)

Pro Lauf gehört der Zustand zu `ExecuteData`. Prozessweit bzw. thread-lokal liegen:

| Stelle | Art |
|---|---|
| `engine/opcode_cache.rs` `OPCODE_CACHE` | `OnceLock`, global |
| `engine/jit.rs` `GLOBAL_JIT` | `OnceLock<RwLock<..>>`, global |
| `engine/function_optimizer.rs` `FUNCTION_OPTIMIZER` | `OnceLock<RwLock<..>>`, global |
| `engine/string_intern.rs` `GLOBAL_INTERNER` | `OnceLock`, global |
| `engine/vm/execute.rs` `DISPATCH_TABLE` | `OnceLock`, unkritisch (read-only) |
| `php/globals.rs` `GLOBALS` | `OnceLock<Mutex<..>>`, global |
| `php/sqlite.rs` `CONNECTIONS`, `NEXT_ID` | global, fällt mit sqlite weg |
| `php/session.rs` `SAVE_PATH` | global, fällt mit Sessions weg |
| `php/ini.rs`, `php/output.rs`, `engine/perf_alloc.rs` | `thread_local!`, passt zu Worker-Threads |

Außerdem: `PhpValue` enthält Raw-Pointer-Varianten (`Ptr`, `ClassEntry`, `Function`) und ist mit `unsafe impl Send/Sync` markiert; der Kommentar in `types.rs` verlagert die Verantwortung auf den Aufrufer. Das betrifft den Plan, kompilierte `OpArray`s per `Arc` zwischen Worker-VMs zu teilen: Das ist möglich, aber die Send/Sync-Zusicherung ist unbelegt und sollte vor M3/M4 geprüft werden (z. B. ob Op-Arrays Raw-Pointer-Werte enthalten können).

`function_table` in `ExecuteData` ist ein typgelöschtes `Arc<dyn Any + Send + Sync>`; für Hot-Reload (M4) sollte das ein konkreter Typ werden.

## 6. Baseline-Testlauf

(wird nach Abschluss des Laufs ergänzt)

## 7. Schnittliste für das Aufräumen

Die Zuordnung folgt deiner Vorgabe (Teilmenge statt PHP-Nähe). Alles unter "Entscheidung" ist Sprachsemantik und liegt bei dir.

**Entfernen (geringe Verflechtung)**
- `examples/wordpress/`, `examples/codeigniter/`, `examples/drupal/`
- `bin/phprs/src/pkg/` (Composer-Paketmanager, ca. 1.900 Zeilen) samt Dependencies `semver`, `zip`, `tar`, `flate2`, `toml`, `dirs`
- `bin/phprs/src/server.rs` (795 Zeilen, ersetzt durch Axum/Loco)
- `src/php/`: `http_stream`, `curl`, `openssl`, `session`, `streams`, `xml`, `compression`, `csv`, `sapi`
- Cargo-Dependencies: `reqwest` (Achtung: wird auch in `vm/builtins.rs` und `php/url.rs` genutzt, nicht nur in `curl`/`http_stream`, dort müssen die Stellen mit entfernt werden), `rusqlite` (mit `bundled`), `tokio` (im `src/` nur in `php/http_stream.rs` verwendet), `pulldown-cmark`, `anyrepair`; `clap` wird in `src/` nicht verwendet, nur im CLI-Crate
- Rust-Beispiele `examples/rust/*` und zugehörige `[[example]]`-Einträge (Doku-Demos der Typen)
- `src/engine/benchmark.rs` (499 Zeilen), sofern es nicht für eigene Messungen genutzt wird

**Behalten**
- Lexer, Parser/AST, Compiler, VM, Opcodes
- OOP: Klassen, Interfaces, Traits, Enums, Closures, Magic Methods, Late Static Binding, Named Args, Variadics, Generatoren, Exceptions
- Builtins für String, Array, Math, JSON, `mb_*`, Datetime, Hash, Serialize
- Reflection (wird vom DI-Container gebraucht, siehe Entscheidung)

**Entscheidung bei dir**
- Reflection behalten? (Vorschlag: ja)
- `include`/`require`: eigener Autoloader in Rust statt dynamischer Includes?
- Regex (`preg_*` via `regex` + `fancy-regex`): behalten oder später als natives Modul?
- `==` mit Type Juggling: strikt machen oder wie PHP lassen?
- JIT-Gerüst (`jit.rs`, `function_optimizer.rs`, `opcode_cache.rs`, `perf_alloc.rs`, `string_intern.rs`): vorerst einfrieren oder entfernen? (Vorschlag: einfrieren, nicht erweitern; Cranelift ist M7)
- Fibers: behalten oder nicht?

**Dialektprüfung (neue Compiler-Phase, noch nicht vorhanden)**
Im Compiler und in `dispatch_handlers.rs` kommen u. a. `global` (`global_imports`) und `array()`-Syntax vor. Eine Prüfphase soll nicht erlaubte Konstrukte mit klaren Fehlermeldungen ablehnen, statt sie still zu unterstützen.

## 8. Die Test-App als Zielbild

`examples/test-app/` (neu, parallel zum Core):

```
examples/test-app/
  app/Controllers/TaskController.php
  app/Models/Task.php
  routes/web.php
  resources/js/Pages/Tasks/Index.vue (oder .tsx)
  database/migrations/
  tests/            # HTTP-Tests gegen den laufenden Server
```

Jeder Meilenstein bekommt hier ein Abnahmekriterium:

| Meilenstein | Abnahme in der Test-App |
|---|---|
| M0 | `GET /tasks` läuft durch PHP-Controller und gibt JSON zurück |
| M1 | `TaskController extends BaseController`, `BaseController` ist in Rust definiert |
| M2 | `Inertia::render('Tasks/Index', ...)` rendert die Seite |
| M3 | `Task`-Model: anlegen, auflisten, löschen gegen die DB |
| M4 | Controller ändern und speichern, nächster Request zeigt die Änderung ohne Neustart |
| M5 | Dashboard-Route aggregiert Task-Statistiken über Polars |

## 9. Vorgeschlagene Reihenfolge der ersten Schritte

1. Eigenen Fork auf GitHub anlegen, `upstream` als Remote behalten und den Stand als `upstream-baseline` taggen (der Klon hier ist ein normales Git-Repo, es ist noch kein Tag gesetzt).
2. `DIALECT.md` schreiben (aus den Entscheidungen in Abschnitt 7).
3. Löschen in Schichten (Beispiele und Paketmanager zuerst, dann `src/php`-Module, dann Dependencies), nach jeder Schicht `cargo test`.
4. Native-Registry (Abschnitt 4) als erste echte Neuerung, noch vor dem Umbau der Crate-Struktur.
5. Crate-Aufteilung `core` / `std` / `cli`.
6. Umbenennung.
