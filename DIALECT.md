# OpenGrid-Dialekt (`.grd`)

Verbindliche Sprach- und Strukturvorgaben für `loco-php-core`, den bereinigten
Fork von phprs, und für Anwendungen, die darauf laufen. Dieses Dokument ersetzt
`SPEC.md` als Single Source of Truth. Was hier nicht steht, ist nicht Teil des
Dialekts.

Stand: Fork von `yingkitw/phprs` 0.1.19 (Tag `upstream-baseline`).

## 1. Leitidee

OpenGrid ist kein PHP-Interpreter. Es ist eine strikte PHP-8-Teilmenge, die
als eingebettete Skriptsprache in einem Rust-Webframework (Loco/Axum) läuft.

- **Rust trägt:** HTTP, Routing, Datenbank, Sessions, Queues, Mailer, Auth,
  Konfiguration, Hot-Reload, Data Science (Polars/ndarray), alle
  Basisklassen.
- **Der Dialekt trägt:** Anwendungslogik in Controllern, Models, Middlewares,
  Services und Routen, geschrieben im Laravel-Stil.
- **Kompatibilitätsziel:** Laravel-Code soll sich mit überschaubarem Aufwand
  portieren lassen. Zend-PHP-Kompatibilität ist kein Ziel. Composer-Pakete
  laufen nicht.

Zwei Prinzipien für jede Entscheidung:

1. **Strikt statt tolerant.** Wo PHP implizit umwandelt oder stillschweigend
   fortsetzt, wirft der Dialekt einen Fehler. Das macht Code vorhersagbar und
   gibt einem späteren JIT saubere Annahmen.
2. **Konvention statt Konfiguration.** Ordnerstruktur, Namensräume und
   Dateiendungen sind festgelegt, nicht konfigurierbar.

## 2. Dateien und Namensräume

- Dateiendung: **`.grd`**, ausnahmslos. `.php`-Dateien werden nicht geladen.
- Öffnungstag `<?php` bleibt erlaubt (Editor-Support), `<?grd` wird
  ebenfalls akzeptiert. Kein schließendes Tag, kein Inline-HTML.
- Jede Datei beginnt mit `declare(strict_types=1);` oder gar nicht; der
  Compiler behandelt jede Datei so, als stünde es dort.
- Eine Datei, eine Klasse (bzw. eine Datei mit Routen oder Konfiguration).
- **Autoloading per Konvention.** Der Rust-Autoloader bildet Klassennamen
  1:1 auf Pfade ab. Für ein Modul `billing`:

  | Klasse | Datei |
  |---|---|
  | `Billing\Controllers\InvoiceController` | `modules/billing/app/Controllers/InvoiceController.grd` |
  | `Billing\Models\Invoice` | `modules/billing/app/Models/Invoice.grd` |
  | `Billing\Middlewares\EnsureOwner` | `modules/billing/app/Middlewares/EnsureOwner.grd` |
  | `Billing\Services\TaxCalculator` | `modules/billing/app/Services/TaxCalculator.grd` |

  Der Namensraum der ersten Ebene ist der Modulname in StudlyCase, die zweite
  Ebene ist der Ordner unter `app/`. Abweichungen sind Compilerfehler.
- `include`, `require`, `include_once`, `require_once`: **entfernt.**
  Dateien werden ausschließlich über den Autoloader geladen.
- `spl_autoload_register`: entfernt, es gibt genau einen Autoloader.

## 3. Projektstruktur ("Modules-Only")

```
my-app/
├── config/                 # app.yaml, development.yaml, vite.config.ts, tailwind.config.js, .env
├── database/               # Migrationen und Seeds (Loco / SeaORM / SQLx)
├── modules/                # Die Anwendung
│   └── billing/
│       ├── module.json     # Manifest: name, version, routePrefix, dependencies
│       ├── Cargo.toml      # optional: natives Rust-Crate des Moduls
│       ├── src/            # optional: #[php_class]-Klassen in Rust
│       ├── app/
│       │   ├── Controllers/
│       │   ├── Models/
│       │   ├── Middlewares/
│       │   └── Services/
│       ├── routes/web.grd
│       └── resources/js/Pages/   # Inertia-Pages (Vue/React)
├── routes/web.grd          # globale Routen und Fallbacks
├── storage/                # Caches, Logs, kompilierte Modul-Plugins (.so/.dylib)
├── public/                 # Web-Root
├── Cargo.toml              # Workspace
└── phprs.lock              # Lock-File des Modul-Managers
```

Regeln:

- Im Root liegen nur `Cargo.toml`, `phprs.lock`, `package.json` (Node-Tooling
  erwartet es dort) und `.gitignore`. Alle Build-Konfigurationen und
  Umgebungsvariablen liegen in `config/`.
- `module.json` ist Pflicht. Ohne Manifest wird ein Modul nicht geladen.
- Ein Modul darf ein Rust-Crate mitbringen. Im Dev-Modus wird es als
  dynamische Bibliothek nach `storage/plugins/` kompiliert und per
  `libloading` geladen, im Prod-Modus statisch in den Workspace eingebunden.
  Der Modul-Manager (`phprs pkg`) übernimmt beides.
- Der Modul-Manager lädt Module aus Git-Repositories. Composer/Packagist
  werden nicht unterstützt.

## 4. Sprachumfang

### 4.1 Behalten

| Bereich | Umfang |
|---|---|
| Typen | `int`, `float`, `string`, `bool`, `null`, `array`, Objekte, Enums, Closures |
| OOP | Klassen, abstrakte Klassen, Interfaces, Traits, Enums (pure und backed), anonyme Klassen, `readonly`, Konstruktor-Promotion, statische Member, Late Static Binding, Klassenkonstanten, `final` |
| Magic Methods | `__construct`, `__get`, `__set`, `__isset`, `__unset`, `__call`, `__callStatic`, `__toString`, `__invoke` (für Facades, Models, Container) |
| Funktionen | benannte Funktionen, Closures, Arrow Functions, First-Class-Callables, Named Arguments, Variadics, Default-Werte, by-reference-Parameter |
| Typen-Syntax | Skalar-, Klassen-, Union-, Intersection-, Nullable-Typen, `mixed`, `void`, `never`, `static`, `self` |
| Kontrollfluss | `if`, `match`, `switch`, `for`, `foreach` (Wert und Schlüssel), `while`, `do-while`, `break`/`continue` mit Ebene, `return` |
| Fehler | `try`/`catch`/`finally`, Exception-Hierarchie, `throw` als Ausdruck |
| Generatoren | `yield`, `yield from` |
| Attribute | `#[...]` mit Reflection-Zugriff (für Routen, Validierung, DI) |
| Reflection | `ReflectionClass`, `ReflectionMethod`, `ReflectionProperty`, `ReflectionFunction`, `ReflectionParameter`, `ReflectionNamedType`, Attribute lesen |
| Regex | `preg_match`, `preg_match_all`, `preg_replace`, `preg_replace_callback`, `preg_split`, `preg_quote` (Rust `regex`/`fancy-regex`, kein volles PCRE; Backreferences dokumentieren) |
| Builtins | String, Array, Math, JSON, `mb_*`, Datetime, Hash/Random, `serialize`/`unserialize` für Plain Data, `sprintf`-Familie, `var_dump`/`print_r` (Debug) |
| Operatoren | arithmetisch, logisch, bitweise, `??`, `?->`, `...`, Spaceship, `instanceof`, String-Interpolation |

### 4.2 Entfernt (Compilerfehler mit Hinweis)

| Konstrukt | Begründung | Ersatz |
|---|---|---|
| `include`/`require`/`*_once` | Autoloader erzwingt Struktur | Namensraum + Datei |
| `eval` | Sicherheit, JIT | keiner |
| `global`, `$GLOBALS` | kein geteilter Zustand | Container/DI |
| Variable Variables `$$x`, `${'x'}` | nicht statisch analysierbar | Arrays |
| `extract`, `compact` | implizite Variablen | explizite Arrays |
| `goto` | | |
| `array()`-Syntax | eine Schreibweise | `[]` |
| `list()` | | `[$a, $b] = ...` |
| Superglobals `$_GET`, `$_POST`, `$_SERVER`, `$_SESSION`, `$_COOKIE`, `$_FILES`, `$_ENV` | Request ist ein Objekt | `Request`, `Session`, `Cookie` (native Klassen) |
| `echo`/`print` außerhalb von Debug-Kontext, Output Buffering (`ob_*`) | Response ist ein Rückgabewert | `return response(...)` |
| `exit`/`die` | Prozess gehört Rust | Exception oder Response |
| `define()`, globale Konstanten | | Klassenkonstanten, `config()` |
| `set_error_handler`, `register_shutdown_function` | Lifecycle gehört Rust | Middlewares |
| Dateisystem-Builtins (`fopen`, `file_put_contents`, `fgetcsv`, ...) | Rust-Dienst | native `Storage`-Klasse |
| Netzwerk (`file_get_contents('http://..')`, `curl_*`, Sockets) | Rust-Dienst | native `Http`-Klasse |
| PDO, `mysqli`, `sqlite3` | Rust-Dienst | native `DB`/`Model` |
| `session_*` | Rust-Dienst | native `Session` |
| `openssl_*`, `gz*` | Rust-Dienst | native Klassen nach Bedarf |
| SimpleXML, DOM | | später natives Modul |
| `ini_set`/`ini_get`, `phpinfo`, `phpversion` | keine INI | `config()` |
| Ticks, `declare(ticks)`, Referenz-Rückgaben `function &f()` | | |

### 4.3 Eingefroren (Code bleibt, wird isoliert, nicht erweitert)

- **JIT-Gerüst:** `engine/jit.rs`, `engine/function_optimizer.rs`,
  `engine/opcode_cache.rs`, `engine/perf_alloc.rs`, `engine/string_intern.rs`,
  `engine/benchmark.rs`. Werden hinter ein Cargo-Feature `experimental-jit`
  gelegt, das standardmäßig aus ist. Cranelift ist Meilenstein M7 und beginnt
  mit einer Messung, nicht mit diesem Code.
- **Fibers:** Opcode und `Fiber`-Klasse bleiben, Feature `fibers`,
  standardmäßig aus. Asynchronität löst der Rust-Host.

## 5. Semantische Abweichungen von PHP

Diese Punkte sind absichtlich anders als in PHP und müssen bei einer
Laravel-Portierung beachtet werden.

### 5.1 Strikte Vergleiche

- `==` und `!=` verhalten sich wie `===` und `!==`. Es gibt kein Type
  Juggling. `0 == "a"`, `"1" == "01"`, `null == false` sind alle `false`.
- `<`, `>`, `<=`, `>=`, `<=>` sind nur zwischen zwei Zahlen (`int`/`float`
  gemischt erlaubt) oder zwei Strings (bytewise) definiert. Andere
  Kombinationen werfen `TypeError`.
- `switch` und `match` vergleichen strikt (bei `match` ist das schon PHP-
  Verhalten; `switch` zieht nach).
- `in_array`, `array_search`, `array_keys($a, $v)` arbeiten immer strikt; der
  `$strict`-Parameter wird akzeptiert und ignoriert.
- Implementierung: `IsEqual`/`IsNotEqual` kompilieren auf dieselben Handler
  wie `IsIdentical`/`IsNotIdentical`; `zval_compare` in
  `engine/operators.rs` wird auf die zugelassenen Typpaare reduziert.

### 5.2 Strikte Typen überall

- `strict_types=1` ist nicht abschaltbar: Skalar-Parameter werden nicht
  koerziert, `int → float` bleibt als einzige Weitung erlaubt (PHP-Verhalten
  unter strict_types).
- Arithmetik mit nicht-numerischen Strings wirft `TypeError` statt Warnung.
- String-zu-Zahl-Konvertierung nur explizit (`(int)`, `intval`, `is_numeric`).

### 5.3 Fehler sind Exceptions

- Es gibt keine Warnings und Notices. Alles, was in PHP eine Warnung wäre
  (undefinierter Array-Schlüssel, undefinierte Variable, Division durch Null,
  falsche Argumentzahl), wirft eine Exception aus einer festen Hierarchie
  (`Error`, `TypeError`, `ValueError`, `ArgumentCountError`,
  `DivisionByZeroError`, `UndefinedKeyError`, `UndefinedVariableError`).
- Undefinierte Array-Schlüssel lesen: Exception. Wer `null` will, nutzt `??`.

### 5.4 Keine globalen Seiteneffekte

- Keine Ausgabe-Funktionen im Request-Pfad: ein Controller **gibt zurück**
  (String, Array, `Response`, `InertiaResponse`, `Redirect`). Die Rust-Seite
  wandelt um.
- Kein prozessweiter Zustand im Skript: statische Properties gelten pro
  VM-Instanz (also pro Worker) und dürfen nicht als Cache missbraucht werden.
  Für Caching gibt es die native `Cache`-Klasse.

### 5.5 Arrays

- Arrays bleiben geordnete Hash-Maps mit PHP-Semantik (Copy-on-Write,
  int/string-Schlüssel). Das ist bewusst, weil Laravel-Code massiv darauf
  baut.
- Die Konvertierung `Array ↔ serde_json::Value` ist die einzige Brücke zu
  Rust: Listen (Schlüssel 0..n-1 lückenlos) werden JSON-Arrays, alles andere
  JSON-Objekte. `null`-Schlüssel gibt es nicht.

## 6. Native Basisklassen (Rust, über die Registry)

Der Dialekt sieht diese Klassen als gegeben an. Sie werden in Rust definiert
und beim VM-Start in die `class_table` eingetragen. Skript-Klassen können von
ihnen erben, wo das markiert ist.

| Klasse | Erbbar | Zweck |
|---|---|---|
| `OpenGrid\Http\Request` | nein | Methode, Pfad, Query, Body, Header, Cookies, Dateien, `input()`, `validate()` |
| `OpenGrid\Http\Response`, `Redirect`, `JsonResponse` | nein | Rückgabetypen |
| `OpenGrid\Http\Controller` | **ja** | Basis für Controller, Middleware-Registrierung |
| `OpenGrid\Http\Middleware` | **ja** | `handle(Request $r, Closure $next)` |
| `OpenGrid\Database\DB` | nein | Query-Builder: `table()`, `where()`, `get()`, `first()`, `insert()`, `update()`, `delete()`, `raw()`, Transaktionen |
| `OpenGrid\Database\Model` | **ja** | Eloquent-artig: `find`, `all`, `where`, `create`, `save`, `delete`, Relationen (M3+) |
| `OpenGrid\Inertia\Inertia` | nein | `render()`, `share()`, `location()` |
| `OpenGrid\Routing\Route` | nein | `get()`, `post()`, `resource()`, `middleware()`, `prefix()`, `group()` |
| `OpenGrid\Support\Session`, `Cache`, `Config`, `Log`, `Storage`, `Http` | nein | Rust-Dienste |
| `OpenGrid\Data\DataFrame`, `Series` | nein | Polars-Bindung (M5) |

Facades (`Route::get(...)`, `DB::table(...)`) sind statische Methoden dieser
Klassen, kein Proxy-Mechanismus. Ein DI-Container in Rust löst
Konstruktor-Parameter per Reflection auf (Typ-Hint → Instanz).

## 7. Compiler-Phase "Dialektprüfung"

Zwischen Parser und Codegen läuft eine Prüfung, die jede Verletzung von
Abschnitt 4.2 mit Datei, Zeile und Hinweis ablehnt:

```
error[GRD001]: `include` ist im OpenGrid-Dialekt nicht erlaubt
  --> modules/billing/app/Controllers/InvoiceController.grd:3:1
   = Hinweis: Klassen werden per Namensraum automatisch geladen.
```

Fehlercodes `GRD0xx` werden in `docs/errors.md` gepflegt. Die Prüfung ist
nicht abschaltbar.

## 8. Konformität und Tests

- Jedes Sprachfeature aus 4.1 hat mindestens einen Test unter
  `tests/dialect/`. Tests für entfernte Konstrukte prüfen, dass der richtige
  `GRD`-Fehler kommt.
- `examples/test-app/` ist die Referenzanwendung. Sie wird bei jedem Commit
  gebaut und ist das Abnahmekriterium für jeden Meilenstein
  (siehe `INVENTORY.md`, Abschnitt 8).
- `.phpt`-Tests aus php-src können als Vorlage dienen, werden aber auf den
  Dialekt angepasst, nicht 1:1 übernommen.

## 9. Offene Punkte

Entschieden wird, sobald die Test-App den Bedarf zeigt:

- Generics-ähnliche Collections (`Collection<Model>`)? Vorerst untypisiert.
- Referenz-Parameter (`&$x`) behalten oder entfernen? Vorerst behalten,
  weil `preg_match` und `array_walk` sie brauchen.
- `declare(strict_types=1)`-Zeile verpflichtend oder optional? Vorerst
  optional (wird impliziert).
- Umgang mit `float`-Schlüsseln in Arrays (PHP kürzt auf `int`): Vorerst
  Fehler.
