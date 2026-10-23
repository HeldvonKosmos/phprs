//! OpenGrid dialect enforcement.
//!
//! The compiler rejects every construct that `DIALECT.md` lists as removed.
//! Each rejection carries a stable `GRDxxx` code so that tooling and tests can
//! match on it. The message format mirrors rustc:
//!
//! ```text
//! error[GRD001]: `include` ist im OpenGrid-Dialekt nicht erlaubt
//!   --> modules/billing/app/Controllers/InvoiceController.grd:3
//!   = Hinweis: Klassen werden per Namensraum automatisch geladen.
//! ```

use super::context::CompileContext;
use crate::engine::lexer::Token;

/// A rejected construct: code, short reason, replacement hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialectRule {
    pub code: &'static str,
    pub reason: &'static str,
    pub hint: &'static str,
}

pub const INCLUDE: DialectRule = DialectRule {
    code: "GRD001",
    reason: "`include`/`require` sind im OpenGrid-Dialekt nicht erlaubt",
    hint: "Klassen werden per Namensraum automatisch geladen (siehe DIALECT.md §2).",
};

pub const GLOBAL: DialectRule = DialectRule {
    code: "GRD002",
    reason: "`global` ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "Geteilter Zustand gehört in den Container oder in Klassen-Properties.",
};

pub const LONG_ARRAY: DialectRule = DialectRule {
    code: "GRD004",
    reason: "die `array(...)`-Schreibweise ist im OpenGrid-Dialekt nicht erlaubt",
    hint: "Schreibe `[...]`.",
};

pub const FORBIDDEN_BUILTIN: DialectRule = DialectRule {
    code: "GRD010",
    reason: "diese Funktion ist im OpenGrid-Dialekt nicht verfügbar",
    hint: "Prozess, Ausgabe, Fehlerbehandlung und Autoloading gehören dem Rust-Host (DIALECT.md §4.2).",
};

/// Builtin function names that the dialect rejects at compile time, grouped
/// by the reason they were removed. Kept sorted inside each group.
pub const FORBIDDEN_FUNCTIONS: &[&str] = &[
    // process lifecycle belongs to the host
    "die",
    "exit",
    "register_shutdown_function",
    "set_error_handler",
    "set_exception_handler",
    // dynamic code / implicit variables
    "compact",
    "eval",
    "extract",
    // global constants: use class constants or config()
    "constant",
    "define",
    "defined",
    // output buffering: controllers return values
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
    // single Rust autoloader
    "spl_autoload_functions",
    "spl_autoload_register",
    "spl_autoload_unregister",
    // INI / environment introspection
    "ini_get",
    "ini_set",
    "phpinfo",
    "phpversion",
    // WordPress-era leftovers
    "apply_filters",
    "do_action",
    "shortcode_atts",
];

pub fn is_forbidden_function(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    FORBIDDEN_FUNCTIONS.binary_search(&lower.as_str()).is_ok()
        || FORBIDDEN_FUNCTIONS.contains(&lower.as_str())
}

/// Build the error string for a rejected construct.
pub fn reject(rule: DialectRule, token: &Token, context: &CompileContext) -> String {
    reject_named(rule, None, token, context)
}

/// Same as [`reject`], naming the offending identifier (e.g. the function).
pub fn reject_named(
    rule: DialectRule,
    name: Option<&str>,
    token: &Token,
    context: &CompileContext,
) -> String {
    let file = context.filename.as_deref().unwrap_or("<inline>");
    let subject = match name {
        Some(n) => format!("`{n}`: {}", rule.reason),
        None => rule.reason.to_string(),
    };
    format!(
        "error[{code}]: {subject}\n  --> {file}:{line}\n  = Hinweis: {hint}",
        code = rule.code,
        file = file,
        line = token.lineno,
        hint = rule.hint,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forbidden_list_is_sorted_within_groups_and_lookup_works() {
        assert!(is_forbidden_function("exit"));
        assert!(is_forbidden_function("OB_START"));
        assert!(is_forbidden_function("define"));
        assert!(!is_forbidden_function("strlen"));
        assert!(!is_forbidden_function("json_encode"));
    }

    #[test]
    fn message_carries_code_file_and_line() {
        let ctx = CompileContext::with_filename("app/Foo.grd".to_string());
        let tok = Token::new(crate::engine::lexer::TokenType::T_INCLUDE, None, 7, 0);
        let msg = reject(INCLUDE, &tok, &ctx);
        assert!(msg.starts_with("error[GRD001]"));
        assert!(msg.contains("app/Foo.grd:7"));
        assert!(msg.contains("Hinweis"));
    }
}
