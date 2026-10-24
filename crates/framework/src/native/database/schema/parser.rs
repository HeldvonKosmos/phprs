//! `.surql` file → statements: port of the platform's `SurqlFileParser`
//! (R4b2, byte-identical to PHP 8.4 apart from the fixed findings, checked
//! by the golden tests). Comments: `--`, `//`, `#` and `/* … */` (R4h; the
//! original knew only `--`). In single quotes `''` and the backslash escape,
//! in double quotes the backslash (R4h: the original took `\'` as the end
//! of the string); statements are
//! trimmed with PHP's `trim` (space, \t, \n, \r, \0, \x0B — not Unicode
//! spaces); the classifier matches `^DEFINE\s+(TABLE|FIELD|INDEX|EVENT)`
//! case-insensitively. Fixed findings (R4h): `OVERWRITE` and `IF NOT
//! EXISTS` are modifiers, not names; a name is a backtick or `⟨…⟩` name or
//! any run of non-space characters (`address.city`, `tags.*`, `größe`) —
//! the original allowed only ASCII `\w`, cut `café` to `caf` and dropped
//! dotted field names.
//! Unlike the original, every skipped statement is reported as a
//! [`Warning`]; the output is unchanged by that.

use std::sync::OnceLock;

use regex::bytes::Regex;

use super::ast::{Kind, Statement, Warning};

/// PHP `trim()` with its default characters.
fn php_trim(b: &[u8]) -> &[u8] {
    let ws = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\0' | b'\x0b');
    let start = b.iter().position(|c| !ws(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|c| !ws(c)).map_or(start, |e| e + 1);
    &b[start..end]
}

/// Split a file into statements (literal port of `splitStatements`).
pub fn split_statements(src: &str) -> Vec<String> {
    let content = src.as_bytes();
    let len = content.len();
    let mut statements = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let push = |current: &mut Vec<u8>, statements: &mut Vec<String>| {
        let t = php_trim(current);
        if !t.is_empty() {
            statements.push(String::from_utf8_lossy(t).into_owned());
        }
        current.clear();
    };
    let mut i = 0;
    while i < len {
        let c = content[i];
        // `--`, `//` and `#` comments to the end of the line (R4h: `//`, `#`)
        if (c == b'-' && content.get(i + 1) == Some(&b'-')) || (c == b'/' && content.get(i + 1) == Some(&b'/')) || c == b'#' {
            while i < len && content[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // `/* … */` comment (R4h); it separates words like a space
        if c == b'/' && content.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < len && !(content[i] == b'*' && content.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(len);
            current.push(b' ');
            continue;
        }
        // 'single quoted': '' is an escaped quote, a backslash escapes the
        // next character (R4h)
        if c == b'\'' {
            current.push(c);
            i += 1;
            while i < len {
                let sc = content[i];
                current.push(sc);
                i += 1;
                if sc == b'\\' && i < len {
                    current.push(content[i]);
                    i += 1;
                    continue;
                }
                if sc == b'\'' {
                    if content.get(i) == Some(&b'\'') {
                        current.push(content[i]);
                        i += 1;
                    } else {
                        break;
                    }
                }
            }
            continue;
        }
        // "double quoted": backslash escapes the next character
        if c == b'"' {
            current.push(c);
            i += 1;
            while i < len {
                let sc = content[i];
                current.push(sc);
                i += 1;
                if sc == b'\\' && i < len {
                    current.push(content[i]);
                    i += 1;
                    continue;
                }
                if sc == b'"' {
                    break;
                }
            }
            continue;
        }
        // `backtick identifier`
        if c == b'`' {
            current.push(c);
            i += 1;
            while i < len && content[i] != b'`' {
                current.push(content[i]);
                i += 1;
            }
            if i < len {
                current.push(content[i]);
                i += 1;
            }
            continue;
        }
        if c == b';' {
            push(&mut current, &mut statements);
            i += 1;
            continue;
        }
        current.push(c);
        i += 1;
    }
    push(&mut current, &mut statements);
    statements
}

/// The classifier's patterns (`classify`): PCRE without `u`, so `\w` and
/// `\s` and case folding are ASCII.
fn patterns() -> &'static [(Kind, Regex); 4] {
    static P: OnceLock<[(Kind, Regex); 4]> = OnceLock::new();
    P.get_or_init(|| {
        let id = r"(`[^`]+`|⟨.+?⟩|[^\s`]+)";
        // R4h: the original read `IF`/`OVERWRITE` as the name
        let modifier = r"(?:OVERWRITE\s+|IF\s+NOT\s+EXISTS\s+)?";
        let re = |s: String| Regex::new(&format!("(?is-u){s}")).expect("pattern");
        [
            (Kind::Table, re(format!(r"^DEFINE\s+TABLE\s+{modifier}{id}"))),
            (Kind::Field, re(format!(r"^DEFINE\s+FIELD\s+{modifier}{id}\s+ON\s+(?:TABLE\s+)?{id}"))),
            (Kind::Index, re(format!(r"^DEFINE\s+INDEX\s+{modifier}{id}\s+ON\s+(?:TABLE\s+)?{id}"))),
            (Kind::Event, re(format!(r"^DEFINE\s+EVENT\s+{modifier}{id}\s+ON\s+(?:TABLE\s+)?{id}"))),
        ]
    })
}

/// `trim($identifier, '`')`, and `⟨name⟩` → `name` (R4h).
fn unquote(id: &[u8]) -> String {
    if let Some(inner) = id.strip_prefix("⟨".as_bytes()).and_then(|x| x.strip_suffix("⟩".as_bytes())) {
        return String::from_utf8_lossy(inner).into_owned();
    }
    let start = id.iter().position(|c| *c != b'`').unwrap_or(id.len());
    let end = id.iter().rposition(|c| *c != b'`').map_or(start, |e| e + 1);
    String::from_utf8_lossy(&id[start..end]).into_owned()
}

/// Classify one statement (as produced by [`split_statements`]). `Ok(None)`
/// for an empty statement, `Err` with the reason for anything the original
/// silently ignores.
pub fn parse_statement(stmt: &str, source: &str) -> Result<Option<Statement>, Warning> {
    if stmt.is_empty() {
        return Ok(None);
    }
    for (kind, re) in patterns() {
        if let Some(m) = re.captures(stmt.as_bytes()) {
            let first = unquote(&m[1]);
            let (name, table) = match kind {
                Kind::Table => (first.clone(), first),
                _ => (first, unquote(&m[2])),
            };
            return Ok(Some(Statement { kind: *kind, name, table, text: stmt.to_string(), source: source.to_string() }));
        }
    }
    let warn = |m: String| Warning { source: source.to_string(), message: m };
    let mut words = stmt.split_ascii_whitespace();
    if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("DEFINE")) {
        return Err(warn(format!("skipped non-DEFINE statement: {}", first_line(stmt))));
    }
    match words.next().map(|k| k.to_ascii_uppercase()) {
        Some(k) if ["TABLE", "FIELD", "INDEX", "EVENT"].contains(&k.as_str()) => Err(warn(format!("unreadable DEFINE {k} skipped: {}", first_line(stmt)))),
        Some(k) => Err(warn(format!("unsupported DEFINE {k} skipped: {}", first_line(stmt)))),
        None => Err(warn(format!("incomplete statement skipped: {stmt}"))),
    }
}

/// All supported statements of a file plus the warnings for skipped ones.
pub fn parse_file(src: &str, source: &str) -> (Vec<Statement>, Vec<Warning>) {
    let mut stmts = Vec::new();
    let mut warnings = Vec::new();
    for s in split_statements(src) {
        match parse_statement(&s, source) {
            Ok(Some(st)) => stmts.push(st),
            Ok(None) => {}
            Err(w) => warnings.push(w),
        }
    }
    (stmts, warnings)
}

/// Words of a statement for [`clause_type`]: whitespace-separated, a
/// backtick-quoted name or a quoted string is one word.
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(&c) = it.peek() {
        if c.is_whitespace() {
            it.next();
            continue;
        }
        let mut w = String::new();
        if c == '`' || c == '\'' || c == '"' {
            w.push(c);
            it.next();
            for d in it.by_ref() {
                w.push(d);
                if d == c {
                    break;
                }
            }
        } else {
            while let Some(&d) = it.peek() {
                if d.is_whitespace() {
                    break;
                }
                w.push(d);
                it.next();
            }
        }
        out.push(w);
    }
    out
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or(s)
}

const CLAUSES: [&str; 9] = ["DEFAULT", "VALUE", "ASSERT", "READONLY", "PERMISSIONS", "COMMENT", "REFERENCE", "FLEXIBLE", "ON"];

/// The type after `TYPE` up to the next clause (`none | int` stays whole).
pub fn clause_type(text: &str) -> Option<String> {
    let w = words(text);
    let pos = w.iter().position(|x| x.eq_ignore_ascii_case("TYPE"))?;
    let ty: Vec<&str> = w[pos + 1..].iter().take_while(|x| !CLAUSES.iter().any(|c| x.eq_ignore_ascii_case(c))).map(String::as_str).collect();
    if ty.is_empty() { None } else { Some(ty.join(" ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_outside_strings_backticks_and_comments() {
        let src = "-- head; comment\nDEFINE TABLE a SCHEMAFULL;\nDEFINE FIELD s ON a TYPE string DEFAULT 'x;y' ASSERT $value != \"p;q\";\nDEFINE FIELD `odd;name` ON a TYPE int; -- tail;\n\n;  ;\n";
        assert_eq!(
            split_statements(src),
            vec![
                "DEFINE TABLE a SCHEMAFULL",
                "DEFINE FIELD s ON a TYPE string DEFAULT 'x;y' ASSERT $value != \"p;q\"",
                "DEFINE FIELD `odd;name` ON a TYPE int",
            ]
        );
    }

    #[test]
    fn escapes() {
        // single quotes: '' and (R4h) the backslash; double quotes: the backslash
        assert_eq!(split_statements("DEFINE FIELD a ON t DEFAULT 'it''s;x';"), vec!["DEFINE FIELD a ON t DEFAULT 'it''s;x'"]);
        assert_eq!(split_statements("DEFINE FIELD a ON t DEFAULT 'it\\'s;x';"), vec!["DEFINE FIELD a ON t DEFAULT 'it\\'s;x'"]);
        assert_eq!(split_statements("DEFINE FIELD a ON t DEFAULT \"q;\\\"r\" ;"), vec!["DEFINE FIELD a ON t DEFAULT \"q;\\\"r\""]);
    }

    #[test]
    fn all_comment_forms() {
        // R4h: the original knew only `--`
        assert_eq!(split_statements("// a; b\nDEFINE TABLE t;"), vec!["DEFINE TABLE t"]);
        assert_eq!(split_statements("# a; b\nDEFINE TABLE t;"), vec!["DEFINE TABLE t"]);
        assert_eq!(split_statements("/* a;\n b */DEFINE TABLE t;"), vec!["DEFINE TABLE t"]);
        assert_eq!(split_statements("DEFINE FIELD a/* x */ON t TYPE int;"), vec!["DEFINE FIELD a ON t TYPE int"]);
        assert_eq!(split_statements("DEFINE FIELD a ON t VALUE '# // /* not comments';"), vec!["DEFINE FIELD a ON t VALUE '# // /* not comments'"]);
    }

    #[test]
    fn multiline_statements_keep_their_lines() {
        let s = split_statements("DEFINE FIELD a ON t\n    TYPE string\n    -- note\n    DEFAULT 'x';");
        assert_eq!(s, vec!["DEFINE FIELD a ON t\n    TYPE string\n    \n    DEFAULT 'x'"]);
    }

    #[test]
    fn keys_of_statements() {
        let p = |s: &str| parse_statement(s, "f").unwrap().unwrap();
        let t = p("define table `tasks` SCHEMALESS");
        assert_eq!((t.kind, t.name.as_str(), t.table.as_str()), (Kind::Table, "tasks", "tasks"));
        let f = p("DEFINE FIELD `address.city` ON TABLE `tasks` TYPE string");
        assert_eq!((f.kind, f.name.as_str(), f.table.as_str()), (Kind::Field, "address.city", "tasks"));
        // R4h: modifiers are not names
        assert_eq!(p("DEFINE TABLE IF NOT EXISTS w").name, "w");
        assert_eq!(p("define table overwrite v SCHEMALESS").name, "v");
        let f = p("DEFINE FIELD OVERWRITE b ON TABLE t TYPE int");
        assert_eq!((f.name.as_str(), f.table.as_str(), f.text.as_str()), ("b", "t", "DEFINE FIELD OVERWRITE b ON TABLE t TYPE int"));
        assert_eq!(p("DEFINE INDEX IF NOT EXISTS i ON t FIELDS a").name, "i");
        assert_eq!(p("DEFINE TABLE IF SCHEMALESS").name, "IF", "a table may be called IF");
        // R4h: names beyond ASCII \w
        let f = p("DEFINE FIELD address.city ON t TYPE string");
        assert_eq!((f.name.as_str(), f.table.as_str()), ("address.city", "t"));
        assert_eq!(p("DEFINE FIELD tags.* ON TABLE t TYPE string").name, "tags.*");
        assert_eq!(p("DEFINE TABLE café SCHEMAFULL").name, "café");
        let f = p("DEFINE FIELD ⟨my field⟩ ON ⟨t-1⟩ TYPE int");
        assert_eq!((f.name.as_str(), f.table.as_str()), ("my field", "t-1"));
        let i = p("DEFINE INDEX uniq ON tasks FIELDS title UNIQUE");
        assert_eq!((i.kind, i.table.as_str()), (Kind::Index, "tasks"));
        let e = p("DEFINE EVENT changed ON TABLE tasks WHEN $event = 'UPDATE' THEN {}");
        assert_eq!((e.kind, e.name.as_str()), (Kind::Event, "changed"));
    }

    #[test]
    fn unsupported_statements_are_reported() {
        let (st, w) = parse_file("DEFINE ANALYZER a TOKENIZERS blank;\nDEFINE FUNCTION fn::x() { 1 };\nREMOVE TABLE x;\nDEFINE TABLE t;", "m.surql");
        assert_eq!(st.len(), 1);
        assert_eq!(w.len(), 3);
        assert!(w[0].message.starts_with("unsupported DEFINE ANALYZER"));
        assert!(w[2].message.starts_with("skipped non-DEFINE"));
    }

    #[test]
    fn empty_input() {
        assert_eq!(parse_file("", "e").0.len(), 0);
        assert_eq!(parse_file("\n-- only a comment\n", "e").0.len(), 0);
    }

    #[test]
    fn field_types() {
        assert_eq!(clause_type("DEFINE FIELD c ON t TYPE none | int PERMISSIONS FULL").as_deref(), Some("none | int"));
        assert_eq!(clause_type("DEFINE FIELD c ON t TYPE option<record<a | b>> DEFAULT NONE").as_deref(), Some("option<record<a | b>>"));
        assert_eq!(clause_type("DEFINE FIELD c ON t FLEXIBLE TYPE object").as_deref(), Some("object"));
        assert_eq!(clause_type("DEFINE FIELD c ON t"), None);
    }
}
