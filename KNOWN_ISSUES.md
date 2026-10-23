# Bekannte Interpreter-Lücken

Beim Aufräumen entdeckte Fehler im geerbten phprs-Kern. Sie betreffen
Grundfunktionen, die Laravel-artiger Code ständig nutzt, und gehören deshalb an
den Anfang von M0, noch vor der Native-Registry. Reproduzierbar mit
`cargo run -q -p phprs-cli -- run <datei>`.

## Behoben (M0)

Die folgenden Punkte sind seit den M0-Commits erledigt und durch
`tests/casts.rs` und `tests/oop_fundamentals.rs` abgedeckt:

- Cast-Syntax `(int)`, `(float)`, `(string)`, `(bool)`, `(array)`, `(object)`;
  String→Zahl nach PHP-Präfixregel (`"12abc"` → 12).
- Exception-Unterklassen behalten Nachricht und Code; `parent::__construct()`
  auf eingebauten Throwables; `catch (Exception $e)` fängt Unterklassen von
  `RuntimeException` & Co.
- **Methodenvererbung** (Methoden der Elternklasse wurden nie gefunden),
  `self::`/`parent::` (werden jetzt zur Compile-Zeit gebunden), `new static`,
  `static::class`, `self::class`, `__CLASS__`, `__NAMESPACE__`.
- Rückgabetypen an Methoden (`: string`, `: ?int`, `: static`, `: A|B`).
- Default-Parameterwerte (Funktionen, Methoden, Closures, Arrow Functions),
  auch mit `self::CONST` und Array-Literalen.
- `new Foo()` ohne Argumente ruft den Konstruktor auf (vorher übersprungen).
- Konstruktor-Promotion (`public readonly int $x`).
- **Echte Closures**: `Closure`-Objekte mit `use`-Captures (by value),
  gebundenem `$this`, Verschachtelung, Speicherung in Arrays/Properties,
  `is_callable`, `instanceof Closure`; Arrow Functions `fn($x) => …` mit
  automatischem Capture.
- `instanceof`, Argument-Unpacking `f(...$args)` in allen Aufrufformen,
  variadische Methoden (Metadaten gingen beim Kopieren der Klassen verloren),
  Expression-Statements mit führender Klammer `(new Foo)->bar();`.
- Qualifizierte Namen `\Foo\Bar`, `Models\User` relativ zu `use`-Imports
  und Namespace; namespaced Funktionen.

## Offen

### Default-Parameter und explizites `null`

Defaults werden als Prolog `$p = $p ?? (default)` kompiliert. Ein explizit
übergebenes `null` nimmt deshalb den Default an (PHP: bleibt `null`). Für den
Dialekt vertretbar, wird dokumentiert.

### `use (&$x)` (Capture by Reference) fehlt

Closures capturen nur by value. By-Reference-Captures werden beim Umbau der
Call-Frames (M1) nachgezogen.

### Interfaces werden beim Parsen übersprungen

`implements X` wird gelesen und verworfen; `instanceof Interface` ist daher
immer `false`. Für DI per Interface-Typ (M1/M2) nötig.

### Fehlende Builtins, bisher bemerkt

`var_export`, `ReflectionNamedType`, `array_key_first` existiert, `iterator_to_array` offen.

## 1. (erledigt) Cast-Syntax `(int) $x` wird nicht unterstützt

```php
$x = (int) "42";
var_dump($x);          // string(3) "int"   — erwartet: int(42)
```

Der Parser behandelt `(int)` als geklammerten Bezeichner und liefert den
String `"int"`. Betroffen sind alle Casts: `(int)`, `(float)`, `(string)`,
`(bool)`, `(array)`, `(object)`. `intval()` & Co. funktionieren.

Ort: Ausdrucksparser, vermutlich `compile/expression/primary.rs` bzw.
`operators.rs` (Klammer-Ausdruck). Schwere: **hoch**.

## 2. (erledigt) `getMessage()` auf Exception-Unterklassen ist leer

```php
class V extends Exception {}
try { throw new V("plain"); } catch (V $e) { echo $e->getMessage(); }  // ""
```

Auch mit explizitem Konstruktor und `parent::__construct($m)` geht die
Nachricht verloren. Direkt instanziierte `Exception`/`RuntimeException`
funktionieren. Betroffen sind damit alle eigenen Exception-Klassen.

Ort: Konstruktor-Dispatch für Klassen mit eingebauter Elternklasse
(`vm/dispatch_handlers.rs`, `exception_dispatch.rs`). Schwere: **hoch**.

## 3. `echo a, b` (Komma-Liste) wird als Funktionsaufruf `,()` geparst

```php
echo "a", "b";   // Warning: Call to undefined function ,()
```

Schwere: niedrig (im Dialekt ist `echo` ohnehin nur Debug-Ausgabe).

## 4. Globale Zustände im Prozess

Aus `INVENTORY.md` §5: Opcode-Cache, JIT, String-Interner und `GLOBALS`
liegen prozessweit in `OnceLock`s; `PhpValue` ist mit Raw-Pointern per
`unsafe impl Send/Sync` markiert. Vor dem Worker-Pool (M3) prüfen.

## 5. `global_imports`-Plumbing in den Call-Frames

`global` ist im Dialekt verboten (GRD002), der VM-Zustand
`global_imports`/`global_script_table` wird aber noch in jedem Call-Frame
gesichert und wiederhergestellt (`callable.rs`, `dispatch_handlers.rs`,
Generator-Frames). Beim Umbau der Frames für die Native-Registry (M1)
entfernen.
