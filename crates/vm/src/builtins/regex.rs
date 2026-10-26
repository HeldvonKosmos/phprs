//! `preg_*` on `fancy-regex` (look-around and backreferences supported).
//! Out-parameters become return values: `preg_match` returns the match
//! array (empty when nothing matched), `preg_match_all` the matches.

use std::rc::Rc;

use fancy_regex::Regex;

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

const OFFSET_CAPTURE: i64 = 256;
const UNMATCHED_AS_NULL: i64 = 512;
const SET_ORDER: i64 = 2;
const SPLIT_NO_EMPTY: i64 = 1;
const SPLIT_DELIM_CAPTURE: i64 = 2;
const SPLIT_OFFSET_CAPTURE: i64 = 4;

pub fn register(vm: &mut Vm) {
    vm.register_native("preg_match", |vm, a| {
        let re = compile(vm, &args::string(a, 0, "preg_match")?)?;
        let s = args::string(a, 1, "preg_match")?;
        let flags = args::int_opt(a, 2, "preg_match", 0)?;
        let offset = args::int_opt(a, 3, "preg_match", 0)?.max(0) as usize;
        let text = s.to_string_lossy();
        match re.captures_from_pos(text.as_str(), offset.min(text.len())).map_err(rt_err)? {
            Some(c) => Ok(Val::array(captures_to_array(&re, &c, flags))),
            None => Ok(Val::empty_array()),
        }
    });
    vm.register_native("preg_match_all", |vm, a| {
        let re = compile(vm, &args::string(a, 0, "preg_match_all")?)?;
        let s = args::string(a, 1, "preg_match_all")?;
        let flags = args::int_opt(a, 2, "preg_match_all", 1)?;
        let text = s.to_string_lossy();
        let mut sets: Vec<PhpArray> = Vec::new();
        for c in re.captures_iter(text.as_str()) {
            let c = c.map_err(rt_err)?;
            sets.push(captures_to_array(&re, &c, flags & !SET_ORDER));
        }
        if flags & SET_ORDER != 0 {
            return Ok(Val::array(PhpArray::from_values(sets.into_iter().map(Val::array))));
        }
        // pattern order: one array per group
        let mut out = PhpArray::new();
        let names = group_names(&re);
        for (gi, name) in names.iter().enumerate() {
            let mut col = PhpArray::new();
            for set in &sets {
                col.push(set.get(&ArrayKey::Int(gi as i64)).cloned().unwrap_or_else(|| Val::str("")));
            }
            if let Some(n) = name {
                out.insert(ArrayKey::Str(PhpString::from(n.as_str())), Val::array(col.clone()));
            }
            out.insert(ArrayKey::Int(gi as i64), Val::array(col));
        }
        Ok(Val::array(out))
    });
    vm.register_native("preg_replace", |vm, a| preg_replace(vm, a, None));
    vm.register_native("preg_replace_callback", |vm, a| {
        args::count(a, 3, "preg_replace_callback")?;
        let cb = a[1].clone();
        let mut b = a.to_vec();
        b[1] = Val::Null;
        preg_replace(vm, &b, Some(cb))
    });
    vm.register_native("preg_split", |vm, a| {
        let re = compile(vm, &args::string(a, 0, "preg_split")?)?;
        let s = args::string(a, 1, "preg_split")?;
        let limit = args::int_opt(a, 2, "preg_split", -1)?;
        let flags = args::int_opt(a, 3, "preg_split", 0)?;
        let text = s.to_string_lossy();
        let mut out = PhpArray::new();
        let mut last = 0usize;
        let mut pieces = 0i64;
        let push = |out: &mut PhpArray, piece: &str, off: usize| {
            if flags & SPLIT_NO_EMPTY != 0 && piece.is_empty() {
                return;
            }
            if flags & SPLIT_OFFSET_CAPTURE != 0 {
                out.push(Val::array(PhpArray::from_values([Val::from(piece), Val::Int(off as i64)])));
            } else {
                out.push(Val::from(piece));
            }
        };
        // PERF3: the groups are needed only with PREG_SPLIT_DELIM_CAPTURE
        let matches = split_matches(&re, text.as_str(), flags & SPLIT_DELIM_CAPTURE != 0)?;
        for (start, end, groups) in matches {
            if limit > 0 && pieces + 1 >= limit {
                break;
            }
            push(&mut out, &text[last..start], last);
            pieces += 1;
            for (gs, ge) in groups {
                push(&mut out, &text[gs..ge], gs);
            }
            last = end;
        }
        push(&mut out, &text[last..], last);
        Ok(Val::array(out))
    });
    vm.register_native("preg_quote", |_, a| {
        let s = args::string(a, 0, "preg_quote")?;
        let delim = match a.get(1) { Some(Val::Str(d)) => d.as_bytes().first().copied(), _ => None };
        let mut out = Vec::new();
        for b in s.as_bytes() {
            if b".\\+*?[^]$(){}=!<>|:-#/".contains(b) || Some(*b) == delim {
                out.push(b'\\');
            }
            if *b == 0 {
                out.extend_from_slice(b"\\000");
            } else {
                out.push(*b);
            }
        }
        Ok(Val::bytes(&out))
    });
    vm.register_native("preg_grep", |vm, a| {
        let re = compile(vm, &args::string(a, 0, "preg_grep")?)?;
        let arr = args::array(a, 1, "preg_grep")?;
        let invert = args::int_opt(a, 2, "preg_grep", 0)? == 1;
        let mut out = PhpArray::new();
        for (k, v) in arr.iter() {
            let s = vm.to_string_val(v)?;
            let m = re.is_match(&s.to_string_lossy()).map_err(rt_err)?;
            if m != invert {
                out.insert(k.clone(), v.clone());
            }
        }
        Ok(Val::array(out))
    });
    vm.register_native("preg_last_error", |_, _| Ok(Val::Int(0)));
    vm.register_native("preg_last_error_msg", |_, _| Ok(Val::str("No error")));
}

fn rt_err(e: fancy_regex::Error) -> Throw {
    Throw::value_error(format!("preg: {e}"))
}

/// Translate a PCRE pattern with delimiters and flags into a Rust regex.
pub(crate) fn compile(vm: &mut Vm, pattern: &PhpString) -> Result<Rc<Regex>, Throw> {
    if let Some(r) = vm.regex_cache.get(pattern.as_bytes()) {
        return Ok(r.clone());
    }
    let raw = pattern.to_string_lossy();
    let raw = raw.trim_start();
    let mut chars = raw.chars();
    let open = chars.next().ok_or_else(|| Throw::value_error("preg: Empty regular expression"))?;
    if open.is_alphanumeric() || open == '\\' {
        return Err(Throw::value_error("preg: Delimiter must not be alphanumeric, backslash, or NUL"));
    }
    let close = match open {
        '(' => ')',
        '{' => '}',
        '[' => ']',
        '<' => '>',
        c => c,
    };
    let body = &raw[open.len_utf8()..];
    let end = body.rfind(close).ok_or_else(|| Throw::value_error(format!("preg: No ending delimiter '{close}' found")))?;
    let (pat, flags) = (&body[..end], &body[end + close.len_utf8()..]);
    let mut prefix = String::new();
    let mut ungreedy = false;
    for f in flags.chars() {
        match f {
            'i' => prefix.push('i'),
            'm' => prefix.push('m'),
            's' => prefix.push('s'),
            'x' => prefix.push('x'),
            'u' | 'D' | 'S' | 'A' | 'J' | '\n' | '\r' | ' ' => {}
            'U' => ungreedy = true,
            other => return Err(Throw::value_error(format!("preg: Unknown modifier '{other}'"))),
        }
    }
    let mut translated = translate_pcre(pat);
    if ungreedy {
        translated = make_ungreedy(&translated);
    }
    let full = if prefix.is_empty() { translated } else { format!("(?{prefix}){translated}") };
    let re = Regex::new(&full).map_err(|e| Throw::value_error(format!("preg: {e}")))?;
    let rc = Rc::new(re);
    vm.regex_cache.insert(pattern.as_bytes().to_vec(), rc.clone());
    Ok(rc)
}

/// Small PCRE → Rust syntax adjustments.
fn translate_pcre(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let b: Vec<char> = p.chars().collect();
    let mut i = 0;
    let mut in_class = false;
    while i < b.len() {
        let c = b[i];
        if c == '\\' && i + 1 < b.len() {
            let n = b[i + 1];
            match n {
                'h' => out.push_str(if in_class { " \\t" } else { "[ \\t]" }),
                'H' => out.push_str("[^ \\t]"),
                'Z' => out.push_str("\\z"),
                'R' => out.push_str("(?:\\r\\n|\\n|\\r)"),
                'e' => out.push_str("\\x1b"),
                _ => {
                    out.push('\\');
                    out.push(n);
                }
            }
            i += 2;
            continue;
        }
        if c == '[' && !in_class {
            in_class = true;
            out.push(c);
            // a leading ']' or '^]' is literal in PCRE
            if i + 1 < b.len() && b[i + 1] == '^' {
                out.push('^');
                i += 1;
            }
            if i + 1 < b.len() && b[i + 1] == ']' {
                out.push_str("\\]");
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == ']' && in_class {
            in_class = false;
        }
        // possessive quantifiers → greedy
        if !in_class && (c == '+') && i > 0 && matches!(b[i - 1], '+' | '*' | '?' | '}') && !(i > 1 && b[i - 2] == '\\') {
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

fn make_ungreedy(p: &str) -> String {
    let mut out = String::new();
    let b: Vec<char> = p.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        out.push(c);
        if c == '\\' && i + 1 < b.len() {
            out.push(b[i + 1]);
            i += 2;
            continue;
        }
        if matches!(c, '*' | '+' | '?' | '}') {
            if i + 1 < b.len() && b[i + 1] == '?' {
                i += 2;
                continue;
            }
            out.push('?');
        }
        i += 1;
    }
    out
}

fn group_names(re: &Regex) -> Vec<Option<String>> {
    re.capture_names().map(|n| n.map(str::to_string)).collect()
}

fn captures_to_array(re: &Regex, c: &fancy_regex::Captures<'_, str>, flags: i64) -> PhpArray {
    let names = group_names(re);
    let mut out = PhpArray::new();
    // PHP drops trailing unmatched groups
    let last_matched = (0..c.len()).rev().find(|i| c.get(*i).is_some()).unwrap_or(0);
    for (gi, name) in names.iter().enumerate().take(last_matched + 1) {
        let v = match c.get(gi) {
            Some(m) => {
                if flags & OFFSET_CAPTURE != 0 {
                    Val::array(PhpArray::from_values([Val::from(m.as_str()), Val::Int(m.start() as i64)]))
                } else {
                    Val::from(m.as_str())
                }
            }
            None => {
                if flags & UNMATCHED_AS_NULL != 0 {
                    Val::Null
                } else if flags & OFFSET_CAPTURE != 0 {
                    Val::array(PhpArray::from_values([Val::str(""), Val::Int(-1)]))
                } else {
                    Val::str("")
                }
            }
        };
        if let Some(n) = name {
            out.insert(ArrayKey::Str(PhpString::from(n.as_str())), v.clone());
        }
        out.insert(ArrayKey::Int(gi as i64), v);
    }
    out
}

/// Expand `$1`, `\1`, `${1}`, `$name` is not supported by PHP; only numeric.
fn expand_replacement(rep: &str, c: &fancy_regex::Captures<'_, str>) -> String {
    let b: Vec<char> = rep.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let ch = b[i];
        if (ch == '$' || ch == '\\') && i + 1 < b.len() {
            let mut j = i + 1;
            let braced = ch == '$' && b[j] == '{';
            if braced {
                j += 1;
            }
            let start = j;
            while j < b.len() && b[j].is_ascii_digit() && j - start < 2 {
                j += 1;
            }
            if j > start && (!braced || (j < b.len() && b[j] == '}')) {
                let n: usize = b[start..j].iter().collect::<String>().parse().unwrap_or(0);
                if let Some(m) = c.get(n) {
                    out.push_str(m.as_str());
                }
                i = if braced { j + 1 } else { j };
                continue;
            }
            if ch == '\\' && b[i + 1] == '\\' {
                out.push('\\');
                i += 2;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// The matches preg_split cuts at, as PCRE walks them (Perl's /g): an empty
/// match right after a non-empty one counts too, and after an empty match
/// the next try is one character further. With `groups`, the spans of the
/// capture groups as well.
fn split_matches(re: &fancy_regex::Regex, text: &str, groups: bool) -> Result<Vec<(usize, usize, Vec<(usize, usize)>)>, Throw> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut empty_at = None;
    while pos <= text.len() {
        let found = if groups {
            re.captures_from_pos(text, pos).map_err(rt_err)?.map(|c| {
                let m = c.get(0).expect("group 0");
                (m.start(), m.end(), (1..c.len()).filter_map(|gi| c.get(gi).map(|g| (g.start(), g.end()))).collect())
            })
        } else {
            re.find_from_pos(text, pos).map_err(rt_err)?.map(|m| (m.start(), m.end(), Vec::new()))
        };
        let Some((start, end, g)) = found else { break };
        if start == end && empty_at == Some(start) {
            // the empty match again: one character on
            match text[start..].chars().next() {
                Some(c) => pos = start + c.len_utf8(),
                None => break,
            }
            continue;
        }
        empty_at = (start == end).then_some(end);
        pos = end;
        out.push((start, end, g));
    }
    Ok(out)
}

fn preg_replace(vm: &mut Vm, a: &[Val], callback: Option<Val>) -> Result<Val, Throw> {
    args::count(a, 3, "preg_replace")?;
    let limit = args::int_opt(a, 3, "preg_replace", -1)?;
    let patterns: Vec<PhpString> = match &a[0] {
        Val::Array(arr) => arr.values().map(|v| v.to_php_string()).collect::<Result<_, _>>()?,
        other => vec![other.to_php_string()?],
    };
    let replacements: Vec<Val> = match &a[1] {
        Val::Array(arr) => arr.values().cloned().collect(),
        other => vec![other.clone()],
    };
    let apply = |vm: &mut Vm, subject: &PhpString| -> Result<Val, Throw> {
        let mut text = subject.to_string_lossy();
        for (pi, p) in patterns.iter().enumerate() {
            let re = compile(vm, p)?;
            let rep: Option<String> = match &callback {
                Some(_) => None,
                None => Some(match (&a[1], replacements.get(pi)) {
                    (Val::Array(_), Some(r)) => r.to_php_string()?.to_string(),
                    (Val::Array(_), None) => String::new(),
                    (r, _) => r.to_php_string()?.to_string(),
                }),
            };
            let mut out = String::new();
            let mut last = 0;
            let mut n = 0i64;
            let snapshot = std::mem::take(&mut text);
            // PERF3: a replacement without `$n`/`\n` needs no capture groups,
            // finding the matches is enough
            if let Some(lit) = rep.as_deref().filter(|r| !r.contains(['$', '\\'])) {
                for m in re.find_iter(snapshot.as_str()) {
                    let m = m.map_err(rt_err)?;
                    if limit >= 0 && n >= limit {
                        break;
                    }
                    out.push_str(&snapshot[last..m.start()]);
                    out.push_str(lit);
                    last = m.end();
                    n += 1;
                }
                out.push_str(&snapshot[last..]);
                text = out;
                continue;
            }
            for c in re.captures_iter(snapshot.as_str()) {
                let c = c.map_err(rt_err)?;
                if limit >= 0 && n >= limit {
                    break;
                }
                let m = c.get(0).expect("group 0");
                out.push_str(&snapshot[last..m.start()]);
                match &callback {
                    Some(cb) => {
                        let arr = captures_to_array(&re, &c, 0);
                        let r = vm.call_value(cb, vec![Val::array(arr)])?;
                        out.push_str(&vm.to_string_val(&r)?.to_string_lossy());
                    }
                    None => out.push_str(&expand_replacement(rep.as_deref().unwrap_or(""), &c)),
                }
                last = m.end();
                n += 1;
            }
            out.push_str(&snapshot[last..]);
            text = out;
        }
        Ok(Val::from(text))
    };
    match &a[2] {
        Val::Array(arr) => {
            let mut out = PhpArray::new();
            for (k, v) in arr.iter() {
                let s = vm.to_string_val(v)?;
                out.insert(k.clone(), apply(vm, &s)?);
            }
            Ok(Val::array(out))
        }
        other => {
            let s = vm.to_string_val(other)?;
            apply(vm, &s)
        }
    }
}
