//! Dialect rules (`DIALECT.md` §4.2 and the assignment §3): every rejected
//! construct carries a stable `GRDxxx` code. Codes GRD001/002/004/010 are
//! inherited from the phprs fork; the others are new (DECISIONS.md).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub code: &'static str,
    pub reason: &'static str,
    pub hint: &'static str,
}

pub const INCLUDE: Rule = Rule {
    code: "GRD001",
    reason: "`include`/`require` sind im OpenGrid-Dialekt nicht erlaubt",
    hint: "Klassen werden per Namensraum automatisch geladen (DIALECT.md §2).",
};
pub const GLOBAL: Rule = Rule {
    code: "GRD002",
    reason: "`global` ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "Geteilter Zustand gehört in den Container oder in Klassen-Properties.",
};
pub const VARIABLE_VARIABLE: Rule = Rule {
    code: "GRD003",
    reason: "variable Variablen (`$$x`, `${…}`) sind im OpenGrid-Dialekt nicht erlaubt",
    hint: "Nutze ein Array.",
};
pub const LONG_ARRAY: Rule = Rule {
    code: "GRD004",
    reason: "die `array(...)`-Schreibweise ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "Schreibe `[...]`.",
};
pub const LIST: Rule = Rule {
    code: "GRD005",
    reason: "`list()` ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "Schreibe `[$a, $b] = …`.",
};
pub const REFERENCE: Rule = Rule {
    code: "GRD006",
    reason: "Referenzen (`&$x`, `function &f`, `foreach (… as &$v)`) sind im OpenGrid-Dialekt nicht erlaubt",
    hint: "Out-Parameter werden Rückgabewerte: `$m = preg_match(...)`, `$a = sort($a)`.",
};
pub const HEREDOC: Rule = Rule {
    code: "GRD007",
    reason: "Heredoc/Nowdoc sind im OpenGrid-Dialekt nicht erlaubt",
    hint: "Nutze normale Strings oder Templates.",
};
pub const STATIC_CLOSURE: Rule = Rule {
    code: "GRD008",
    reason: "`static function`/`static fn` sind im OpenGrid-Dialekt nicht erlaubt",
    hint: "Lass `static` weg.",
};
pub const DECLARE: Rule = Rule {
    code: "GRD009",
    reason: "`declare(...)` außer `strict_types=1` ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "`strict_types=1` gilt immer.",
};
pub const FORBIDDEN_BUILTIN: Rule = Rule {
    code: "GRD010",
    reason: "diese Funktion ist im OpenGrid-Dialekt nicht verfügbar",
    hint: "Prozess, Ausgabe, Fehlerbehandlung und Autoloading gehören dem Rust-Host (DIALECT.md §4.2).",
};
pub const GOTO: Rule = Rule {
    code: "GRD011",
    reason: "`goto` ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "",
};
pub const SUPERGLOBAL: Rule = Rule {
    code: "GRD012",
    reason: "Superglobals (`$_GET`, `$_POST`, `$GLOBALS`, …) sind im OpenGrid-Dialekt nicht erlaubt",
    hint: "Der Request ist ein Objekt: `Request`, `Session`, `Cookie`.",
};
pub const STATIC_VAR: Rule = Rule {
    code: "GRD013",
    reason: "`static $x` in Funktionen ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "Kein prozessweiter Zustand im Skript (DIALECT.md §5.4).",
};
pub const EVAL: Rule = Rule {
    code: "GRD014",
    reason: "`eval` ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "",
};
pub const EXIT: Rule = Rule {
    code: "GRD015",
    reason: "`exit`/`die` sind im OpenGrid-Dialekt nicht erlaubt",
    hint: "Wirf eine Exception oder gib eine Response zurück.",
};
pub const HALT_COMPILER: Rule = Rule {
    code: "GRD016",
    reason: "`__halt_compiler` ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "",
};
pub const INLINE_HTML: Rule = Rule {
    code: "GRD017",
    reason: "Inline-HTML außerhalb von `<?php` ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "Eine Datei, eine Klasse; Ausgabe ist ein Rückgabewert.",
};
pub const SHELL_EXEC: Rule = Rule {
    code: "GRD018",
    reason: "Backtick-Ausführung ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "",
};

pub const BYREF_POP: Rule = Rule {
    code: "GRD019",
    reason: "`array_pop`/`array_shift` verändern ihr Argument und sind im OpenGrid-Dialekt nicht verfügbar",
    hint: "Nutze `[$wert, $rest] = Arr::pop($a)` bzw. `Arr::shift($a)`.",
};
pub const DISCARDED_RESULT: Rule = Rule {
    code: "GRD020",
    reason: "der Rückgabewert dieser Funktion wird verworfen; im OpenGrid-Dialekt liefert sie das Ergebnis statt ihr Argument zu verändern",
    hint: "Schreibe `$a = sort($a);` bzw. `$m = preg_match(...)` (DIALECT.md §5.6).",
};

/// Builtins that mutate a by-reference argument in PHP and return the new
/// value in the dialect instead; discarding that value is a compile error.
pub const RETURNS_INSTEAD_OF_BYREF: &[&str] = &[
    "array_multisort",
    "array_push",
    "array_splice",
    "array_unshift",
    "arsort",
    "asort",
    "krsort",
    "ksort",
    "natcasesort",
    "natsort",
    "parse_str",
    "preg_match",
    "preg_match_all",
    "rsort",
    "shuffle",
    "sort",
    "uasort",
    "uksort",
    "usort",
];

pub fn returns_instead_of_byref(name: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(name).to_ascii_lowercase();
    RETURNS_INSTEAD_OF_BYREF.binary_search(&lower.as_str()).is_ok()
}

pub fn is_byref_pop(name: &[u8]) -> bool {
    name.eq_ignore_ascii_case(b"array_pop") || name.eq_ignore_ascii_case(b"array_shift")
}

/// Builtins rejected at compile time (sorted).
pub const FORBIDDEN_FUNCTIONS: &[&str] = &[
    "apply_filters",
    "compact",
    "constant",
    "define",
    "defined",
    "die",
    "do_action",
    "eval",
    "exit",
    "extract",
    "ini_get",
    "ini_set",
    "ob_clean",
    "ob_end_clean",
    "ob_end_flush",
    "ob_flush",
    "ob_get_clean",
    "ob_get_contents",
    "ob_get_flush",
    "ob_get_level",
    "ob_implicit_flush",
    "ob_start",
    "phpinfo",
    "phpversion",
    "register_shutdown_function",
    "set_error_handler",
    "set_exception_handler",
    "shortcode_atts",
    "spl_autoload_functions",
    "spl_autoload_register",
    "spl_autoload_unregister",
];

pub fn is_forbidden_function(name: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(name).to_ascii_lowercase();
    FORBIDDEN_FUNCTIONS.binary_search(&lower.as_str()).is_ok()
}

pub const SUPERGLOBALS: &[&[u8]] =
    &[b"_GET", b"_POST", b"_SERVER", b"_SESSION", b"_COOKIE", b"_FILES", b"_ENV", b"_REQUEST", b"GLOBALS"];

pub fn is_superglobal(name: &[u8]) -> bool {
    SUPERGLOBALS.contains(&name)
}
