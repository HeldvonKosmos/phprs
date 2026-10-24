//! `.surql` file → statements (port of `SurqlFileParser`, from its
//! description). Behaviour kept as in the PHP original, including its known
//! gaps (AUFTRAG R4 b): the splitter knows only `--` comments, single and
//! double quotes know only the doubled quote as escape, unsupported DEFINE
//! kinds and non-DEFINE statements are skipped. Unlike the original, every
//! skip is reported as a [`Warning`]; the output is unchanged by that.

use super::ast::{Kind, Statement, Warning};

/// Split a file into statements at `;` outside quotes, backticks and `--`
/// comments. Comments are removed, each statement is trimmed; empty
/// statements are dropped.
pub fn split_statements(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' | '"' => {
                // quoted string; the doubled quote is the only escape
                cur.push(c);
                i += 1;
                while i < chars.len() {
                    cur.push(chars[i]);
                    if chars[i] == c {
                        if i + 1 < chars.len() && chars[i + 1] == c {
                            cur.push(c);
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            '`' => {
                cur.push(c);
                i += 1;
                while i < chars.len() {
                    cur.push(chars[i]);
                    if chars[i] == '`' {
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            '-' if chars.get(i + 1) == Some(&'-') => {
                // comment to end of line (the newline stays as separator)
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ';' => {
                push_stmt(&mut out, &mut cur);
                i += 1;
            }
            _ => {
                cur.push(c);
                i += 1;
            }
        }
    }
    push_stmt(&mut out, &mut cur);
    out
}

fn push_stmt(out: &mut Vec<String>, cur: &mut String) {
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    cur.clear();
}

/// Words of a statement: whitespace-separated, a backtick-quoted name is one
/// word, quoted strings are one word.
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

fn unquote(w: &str) -> String {
    w.strip_prefix('`').and_then(|x| x.strip_suffix('`')).unwrap_or(w).to_string()
}

fn is(w: Option<&String>, kw: &str) -> bool {
    w.is_some_and(|x| x.eq_ignore_ascii_case(kw))
}

/// Parse one statement. `Ok(None)` for an empty statement, `Err` with the
/// reason for anything the schema model does not hold.
pub fn parse_statement(stmt: &str, source: &str) -> Result<Option<Statement>, Warning> {
    let text = stmt.trim().trim_end_matches(';').trim();
    if text.is_empty() {
        return Ok(None);
    }
    let w = words(text);
    let warn = |m: String| Warning { source: source.to_string(), message: m };
    if !is(w.first(), "DEFINE") {
        return Err(warn(format!("skipped non-DEFINE statement: {}", first_line(text))));
    }
    let kind = match w.get(1).map(|k| k.to_ascii_uppercase()).as_deref() {
        Some("TABLE") => Kind::Table,
        Some("FIELD") => Kind::Field,
        Some("INDEX") => Kind::Index,
        Some("EVENT") => Kind::Event,
        Some(other) => return Err(warn(format!("unsupported DEFINE {other} skipped: {}", first_line(text)))),
        None => return Err(warn(format!("incomplete statement skipped: {text}"))),
    };
    let mut i = 2;
    if is(w.get(i), "OVERWRITE") {
        i += 1;
    } else if is(w.get(i), "IF") && is(w.get(i + 1), "NOT") && is(w.get(i + 2), "EXISTS") {
        i += 3;
    }
    let Some(name) = w.get(i).map(|n| unquote(n)) else {
        return Err(warn(format!("DEFINE {} without a name skipped", kind.keyword())));
    };
    let table = if kind == Kind::Table {
        name.clone()
    } else {
        let mut j = i + 1;
        if !is(w.get(j), "ON") {
            return Err(warn(format!("DEFINE {} {name} without ON <table> skipped", kind.keyword())));
        }
        j += 1;
        if is(w.get(j), "TABLE") {
            j += 1;
        }
        match w.get(j) {
            Some(t) => unquote(t),
            None => return Err(warn(format!("DEFINE {} {name} without a table skipped", kind.keyword()))),
        }
    };
    Ok(Some(Statement { kind, name, table, text: text.to_string(), source: source.to_string() }))
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
    fn doubled_quote_is_the_only_escape() {
        // as in the PHP original: '' escapes, \' does not (known finding)
        assert_eq!(split_statements("DEFINE FIELD a ON t DEFAULT 'it''s;x';"), vec!["DEFINE FIELD a ON t DEFAULT 'it''s;x'"]);
        assert_eq!(split_statements("DEFINE FIELD a ON t DEFAULT 'it\\'s;x';").len(), 2);
    }

    #[test]
    fn only_dash_comments_are_known() {
        // known finding: `//`, `#` and `/* */` are not comments for the splitter
        assert_eq!(split_statements("// a; b\nDEFINE TABLE t;").len(), 2);
    }

    #[test]
    fn multiline_statements_keep_their_lines() {
        let s = split_statements("DEFINE FIELD a ON t\n    TYPE string\n    -- note\n    DEFAULT 'x';");
        assert_eq!(s, vec!["DEFINE FIELD a ON t\n    TYPE string\n    \n    DEFAULT 'x'"]);
    }

    #[test]
    fn keys_of_statements() {
        let p = |s: &str| parse_statement(s, "f").unwrap().unwrap();
        let t = p("define table if not exists `tasks` SCHEMALESS");
        assert_eq!((t.kind, t.name.as_str(), t.table.as_str()), (Kind::Table, "tasks", "tasks"));
        let f = p("DEFINE FIELD OVERWRITE address.city ON TABLE `tasks` TYPE string");
        assert_eq!((f.kind, f.name.as_str(), f.table.as_str()), (Kind::Field, "address.city", "tasks"));
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
