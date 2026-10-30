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
    reason: "`include`/`require` are not allowed in the OpenGrid dialect",
    hint: "Klassen werden per Namensraum automatisch geladen (DIALECT.md §2).",
};
pub const GLOBAL: Rule = Rule {
    code: "GRD002",
    reason: "`global` is not allowed in the OpenGrid dialect",
    hint: "Shared state belongs in the container or in class properties.",
};
pub const VARIABLE_VARIABLE: Rule = Rule {
    code: "GRD003",
    reason: "variable variables (`$$x`, `${…}`) are not allowed in the OpenGrid dialect",
    hint: "Nutze ein Array.",
};
pub const LONG_ARRAY: Rule = Rule {
    code: "GRD004",
    reason: "the `array(...)` syntax is not allowed in the OpenGrid dialect",
    hint: "Schreibe `[...]`.",
};
pub const LIST: Rule = Rule {
    code: "GRD005",
    reason: "`list()` is not allowed in the OpenGrid dialect",
    hint: "Schreibe `[$a, $b] = …`.",
};
pub const REFERENCE: Rule = Rule {
    code: "GRD006",
    reason: "references (`&$x`, `function &f`, `foreach (… as &$v)`) are not allowed in the OpenGrid dialect",
    hint: "Out parameters become return values: `$m = preg_match(...)`, `$a = sort($a)`.",
};
pub const HEREDOC: Rule = Rule {
    code: "GRD007",
    reason: "heredoc/nowdoc are not allowed in the OpenGrid dialect",
    hint: "Nutze normale Strings oder Templates.",
};
pub const STATIC_CLOSURE: Rule = Rule {
    code: "GRD008",
    reason: "`static function`/`static fn` are not allowed in the OpenGrid dialect",
    hint: "Lass `static` weg.",
};
pub const DECLARE: Rule = Rule {
    code: "GRD009",
    reason: "`declare(...)` other than `strict_types=1` is not allowed in the OpenGrid dialect",
    hint: "`strict_types=1` gilt immer.",
};
pub const FORBIDDEN_BUILTIN: Rule = Rule {
    code: "GRD010",
    reason: "this function is not available in the OpenGrid dialect",
    hint: "Process, output, error handling and autoloading belong to the Rust host (DIALECT.md §4.2).",
};
pub const GOTO: Rule = Rule {
    code: "GRD011",
    reason: "`goto` is not allowed in the OpenGrid dialect",
    hint: "",
};
pub const SUPERGLOBAL: Rule = Rule {
    code: "GRD012",
    reason: "superglobals (`$_GET`, `$_POST`, `$GLOBALS`, …) are not allowed in the OpenGrid dialect",
    hint: "The request is an object: `Request`, `Session`, `Cookie`.",
};
pub const STATIC_VAR: Rule = Rule {
    code: "GRD013",
    reason: "`static $x` in functions is not allowed in the OpenGrid dialect",
    hint: "Kein prozessweiter Zustand im Skript (DIALECT.md §5.4).",
};
pub const EVAL: Rule = Rule {
    code: "GRD014",
    reason: "`eval` is not allowed in the OpenGrid dialect",
    hint: "",
};
pub const EXIT: Rule = Rule {
    code: "GRD015",
    reason: "`exit`/`die` are not allowed in the OpenGrid dialect",
    hint: "Throw an exception or return a response.",
};
pub const HALT_COMPILER: Rule = Rule {
    code: "GRD016",
    reason: "`__halt_compiler` is not allowed in the OpenGrid dialect",
    hint: "",
};
pub const INLINE_HTML: Rule = Rule {
    code: "GRD017",
    reason: "inline HTML outside `<?php` is not allowed in the OpenGrid dialect",
    hint: "One file, one class; output is a return value.",
};
pub const SHELL_EXEC: Rule = Rule {
    code: "GRD018",
    reason: "backtick execution is not allowed in the OpenGrid dialect",
    hint: "",
};

pub const BYREF_POP: Rule = Rule {
    code: "GRD019",
    reason: "`array_pop`/`array_shift` modify their argument and are not available in the OpenGrid dialect",
    hint: "Nutze `[$wert, $rest] = Arr::pop($a)` bzw. `Arr::shift($a)`.",
};
pub const DISCARDED_RESULT: Rule = Rule {
    code: "GRD020",
    reason: "the return value of this function is discarded; in the OpenGrid dialect it returns the result instead of modifying its argument",
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
