# Bekannte Interpreter-Lücken

Beim Aufräumen entdeckte Fehler im geerbten phprs-Kern. Sie betreffen
Grundfunktionen, die Laravel-artiger Code ständig nutzt, und gehören deshalb an
den Anfang von M0, noch vor der Native-Registry. Reproduzierbar mit
`cargo run -q -p phprs-cli -- run <datei>`.

## 1. Cast-Syntax `(int) $x` wird nicht unterstützt

```php
$x = (int) "42";
var_dump($x);          // string(3) "int"   — erwartet: int(42)
```

Der Parser behandelt `(int)` als geklammerten Bezeichner und liefert den
String `"int"`. Betroffen sind alle Casts: `(int)`, `(float)`, `(string)`,
`(bool)`, `(array)`, `(object)`. `intval()` & Co. funktionieren.

Ort: Ausdrucksparser, vermutlich `compile/expression/primary.rs` bzw.
`operators.rs` (Klammer-Ausdruck). Schwere: **hoch**.

## 2. `getMessage()` auf Exception-Unterklassen ist leer

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
