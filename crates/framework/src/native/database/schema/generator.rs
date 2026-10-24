//! Schema → `temporary.surql` text (port of `SurqlGenerator`, R4b2,
//! byte-identical to PHP 8.4): one block per table, sorted like PHP `ksort`; a block has a
//! header framed by lines of 30 dashes, the DEFINE TABLE statement, then
//! fields, indexes and events (each group sorted, separated by one empty
//! line); blocks are separated by two empty lines; the file ends with one
//! newline. All layout decisions live in this file so R4a can align them
//! with the PHP output.

use std::cmp::Ordering;

use indexmap::IndexMap;

use super::ast::{Schema, Statement};

pub const RULE: &str = "-- ------------------------------";

pub fn generate(schema: &Schema) -> String {
    let mut blocks = Vec::new();
    for (name, table) in sorted(&schema.tables) {
        let mut lines = vec![RULE.to_string(), format!("-- TABLE: {name}"), RULE.to_string(), String::new()];
        if let Some(d) = &table.define {
            lines.push(stmt(d));
        }
        for group in [&table.fields, &table.indexes, &table.events] {
            if group.is_empty() {
                continue;
            }
            lines.push(String::new());
            lines.extend(sorted(group).into_iter().map(|(_, s)| stmt(s)));
        }
        blocks.push(lines.join("\n"));
    }
    // as in the original, also for an empty schema: implode + "\n"
    let mut out = blocks.join("\n\n\n");
    out.push('\n');
    out
}

fn stmt(s: &Statement) -> String {
    format!("{};", s.text)
}

/// Insertion sort with [`php_key_cmp`]: PHP's key order is not transitive for
/// mixed keys (`"1x" < "20" < "100" < "1x"`), and Rust's `sort_by` may panic on
/// such a comparator; insertion sort never does (and PHP's `zend_sort` also
/// uses insertion sort for small arrays).
fn sorted<V>(map: &IndexMap<String, V>) -> Vec<(&String, &V)> {
    let mut v: Vec<(&String, &V)> = Vec::with_capacity(map.len());
    for item in map.iter() {
        let pos = v.iter().position(|x| php_key_cmp(item.0, x.0) == Ordering::Less).unwrap_or(v.len());
        v.insert(pos, item);
    }
    v
}

/// Key order of PHP 8 `ksort($a)` (SORT_REGULAR): decimal integer strings
/// are integer keys; two integers compare numerically; two strings compare
/// numerically when both are numeric, else bytewise; an integer and a
/// non-numeric string compare as strings.
pub fn php_key_cmp(a: &str, b: &str) -> Ordering {
    match (int_key(a), int_key(b), numeric(a), numeric(b)) {
        (Some(x), Some(y), _, _) => x.cmp(&y),
        (_, _, Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        _ => a.as_bytes().cmp(b.as_bytes()),
    }
}

fn int_key(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let canonical = !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) && (digits == "0" || !digits.starts_with('0')) && s != "-0";
    if canonical { s.parse().ok() } else { None }
}

fn numeric(s: &str) -> Option<f64> {
    let t = s.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']).trim_end_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    if t.is_empty() || t.chars().any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E') {
        return None;
    }
    t.parse::<f64>().ok().filter(|f| f.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let mut s = Schema::default();
        s.merge_source(
            "DEFINE TABLE tasks SCHEMALESS;\nDEFINE FIELD title ON tasks TYPE string;\nDEFINE FIELD done ON tasks TYPE bool;\nDEFINE INDEX t ON tasks FIELDS title;\nDEFINE TABLE clients SCHEMAFULL;",
            "x",
        );
        let expected = "-- ------------------------------\n-- TABLE: clients\n-- ------------------------------\n\nDEFINE TABLE clients SCHEMAFULL;\n\n\n-- ------------------------------\n-- TABLE: tasks\n-- ------------------------------\n\nDEFINE TABLE tasks SCHEMALESS;\n\nDEFINE FIELD done ON tasks TYPE bool;\nDEFINE FIELD title ON tasks TYPE string;\n\nDEFINE INDEX t ON tasks FIELDS title;\n";
        assert_eq!(generate(&s), expected);
        assert_eq!(RULE.matches('-').count() - 2, 30);
    }

    #[test]
    fn empty_schema_is_a_newline() {
        assert_eq!(generate(&Schema::default()), "\n");
    }

    #[test]
    fn ksort_order() {
        let mut m = IndexMap::new();
        for k in ["b", "10", "a", "9", "B", "_x", "1.5", "-1", "01"] {
            m.insert(k.to_string(), ());
        }
        let keys: Vec<&str> = sorted(&m).into_iter().map(|(k, _)| k.as_str()).collect();
        // integers numerically, numeric strings numerically among themselves,
        // the rest bytewise (uppercase before lowercase, `_` after uppercase)
        assert_eq!(keys, vec!["-1", "01", "1.5", "9", "10", "B", "_x", "a", "b"]);
    }

    #[test]
    fn non_transitive_keys_do_not_panic() {
        let mut m = IndexMap::new();
        for k in ["100", "1x", "20"] {
            m.insert(k.to_string(), ());
        }
        assert_eq!(sorted(&m).len(), 3);
    }

    #[test]
    fn deterministic() {
        let src = "DEFINE FIELD z ON t TYPE int;\nDEFINE FIELD a ON t TYPE int;\nDEFINE TABLE t;";
        let (mut a, mut b) = (Schema::default(), Schema::default());
        a.merge_source(src, "1");
        b.merge_source(src, "2");
        assert_eq!(generate(&a), generate(&b));
    }
}
