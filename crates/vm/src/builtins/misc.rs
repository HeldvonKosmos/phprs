//! filter_var, password hashing, serialize, URL helpers, version_compare.

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.register_native("filter_var", filter_var);
    vm.register_native("password_hash", |_, a| {
        let pw = args::string(a, 0, "password_hash")?;
        let cost = match a.get(2) {
            Some(Val::Array(o)) => o.get(&ArrayKey::from_str("cost")).map(|c| c.to_int()).transpose()?.unwrap_or(10) as u32,
            _ => 10,
        };
        let h = bcrypt::hash(pw.as_bytes(), cost.clamp(4, 31)).map_err(|e| Throw::value_error(e.to_string()))?;
        Ok(Val::from(h.replacen("$2b$", "$2y$", 1)))
    });
    vm.register_native("password_verify", |_, a| {
        let pw = args::string(a, 0, "password_verify")?;
        let hash = args::string(a, 1, "password_verify")?.to_string_lossy().replacen("$2y$", "$2b$", 1);
        Ok(Val::Bool(bcrypt::verify(pw.as_bytes(), &hash).unwrap_or(false)))
    });
    vm.register_native("password_needs_rehash", |_, a| {
        let hash = args::string(a, 0, "password_needs_rehash")?.to_string_lossy();
        let cost = match a.get(2) { Some(Val::Array(o)) => o.get(&ArrayKey::from_str("cost")).map(|c| c.to_int()).transpose()?.unwrap_or(10), _ => 10 };
        let current: i64 = hash.get(4..6).and_then(|c| c.parse().ok()).unwrap_or(0);
        Ok(Val::Bool(!hash.starts_with("$2y$") || current != cost))
    });
    vm.register_native("password_get_info", |_, a| {
        let hash = args::string(a, 0, "password_get_info")?.to_string_lossy();
        let mut out = PhpArray::new();
        let bcrypt = hash.starts_with("$2y$");
        out.insert(ArrayKey::from_str("algo"), if bcrypt { Val::str("2y") } else { Val::Null });
        out.insert(ArrayKey::from_str("algoName"), Val::str(if bcrypt { "bcrypt" } else { "unknown" }));
        let mut opts = PhpArray::new();
        if bcrypt {
            opts.insert(ArrayKey::from_str("cost"), Val::Int(hash.get(4..6).and_then(|c| c.parse().ok()).unwrap_or(0)));
        }
        out.insert(ArrayKey::from_str("options"), Val::array(opts));
        Ok(Val::array(out))
    });
    vm.register_native("serialize", |vm, a| {
        args::count(a, 1, "serialize")?;
        let mut out = Vec::new();
        serialize(vm, &a[0], &mut out, &mut Serialized::default())?;
        Ok(Val::bytes(&out))
    });
    vm.register_native("unserialize", |vm, a| {
        let s = args::string(a, 0, "unserialize")?;
        if s.is_empty() {
            return Ok(Val::Bool(false)); // PHP: false without a warning
        }
        let mut pos = 0;
        // PHP warns and returns false; the dialect throws instead (DIALECT §5.3)
        match unserialize(vm, s.as_bytes(), &mut pos, &mut Vec::new()) {
            Ok(v) => Ok(v),
            Err(()) => Err(Throw::value_error(format!("unserialize(): Error at offset {} of {} bytes", pos.min(s.len()), s.len()))),
        }
    });
    vm.register_native("urlencode", |_, a| Ok(Val::from(url_encode(args::string(a, 0, "urlencode")?.as_bytes(), true))));
    vm.register_native("rawurlencode", |_, a| Ok(Val::from(url_encode(args::string(a, 0, "rawurlencode")?.as_bytes(), false))));
    vm.register_native("urldecode", |_, a| Ok(Val::bytes(&url_decode(args::string(a, 0, "urldecode")?.as_bytes(), true))));
    vm.register_native("rawurldecode", |_, a| Ok(Val::bytes(&url_decode(args::string(a, 0, "rawurldecode")?.as_bytes(), false))));
    vm.register_native("http_build_query", |vm, a| {
        let mut visiting = Vec::new();
        let data = match a.first() {
            Some(Val::Array(arr)) => (**arr).clone(),
            Some(Val::Object(h)) => {
                visiting.push(*h);
                public_props(vm, *h)
            }
            _ => return Err(Throw::type_error("http_build_query(): Argument #1 ($data) must be of type array")),
        };
        let prefix = match a.get(1) { Some(Val::Str(s)) => s.to_string_lossy(), _ => String::new() };
        let sep = match a.get(2) { Some(Val::Str(s)) => s.to_string_lossy(), _ => "&".into() };
        let raw = args::int_opt(a, 3, "http_build_query", 1)? == 2;
        let mut parts = Vec::new();
        build_query(vm, &data, None, &prefix, raw, &mut parts, &mut visiting)?;
        Ok(Val::from(parts.join(&sep)))
    });
    vm.register_native("parse_str", |_, a| {
        // returns the parsed array (no by-ref)
        let s = args::string(a, 0, "parse_str")?;
        let mut out = PhpArray::new();
        for pair in s.as_bytes().split(|b| *b == b'&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = match pair.iter().position(|b| *b == b'=') {
                Some(p) => (&pair[..p], &pair[p + 1..]),
                None => (pair, &b""[..]),
            };
            let key = String::from_utf8_lossy(&url_decode(k, true)).into_owned();
            let val = Val::bytes(&url_decode(v, true));
            insert_query_key(&mut out, &key, val);
        }
        Ok(Val::array(out))
    });
    vm.register_native("parse_url", |_, a| {
        let s = args::string(a, 0, "parse_url")?.to_string_lossy();
        let component = args::int_opt(a, 1, "parse_url", -1)?;
        let parts = parse_url(&s);
        let Some(parts) = parts else { return Ok(Val::Bool(false)) };
        if component >= 0 {
            let key = ["scheme", "host", "port", "user", "pass", "path", "query", "fragment"].get(component as usize).copied().unwrap_or("");
            return Ok(parts.get(&ArrayKey::from_str(key)).cloned().unwrap_or(Val::Null));
        }
        Ok(Val::array(parts))
    });
    vm.register_native("version_compare", |_, a| {
        let x = args::string(a, 0, "version_compare")?.to_string_lossy();
        let y = args::string(a, 1, "version_compare")?.to_string_lossy();
        let ord = version_cmp(&x, &y);
        match a.get(2) {
            Some(Val::Str(op)) => Ok(Val::Bool(match op.to_string_lossy().as_str() {
                "<" | "lt" => ord < 0,
                "<=" | "le" => ord <= 0,
                ">" | "gt" => ord > 0,
                ">=" | "ge" => ord >= 0,
                "==" | "eq" => ord == 0,
                "!=" | "<>" | "ne" => ord != 0,
                _ => return Err(Throw::value_error("version_compare(): Argument #3 ($operator) must be a valid comparison operator")),
            })),
            _ => Ok(Val::Int(ord)),
        }
    });
    vm.register_native("class_implements", |vm, a| {
        let class = match a.first() {
            Some(Val::Object(h)) => vm.objects.get(*h).class,
            Some(Val::Str(s)) => match vm.lookup_class(s.as_bytes()) { Some(c) => c, None => return Ok(Val::Bool(false)) },
            _ => return Ok(Val::Bool(false)),
        };
        let mut out = PhpArray::new();
        for i in &vm.class(class).interfaces {
            let n = vm.class(*i).name.clone();
            out.insert(ArrayKey::Str(PhpString::from(n.as_slice())), Val::bytes(&n));
        }
        Ok(Val::array(out))
    });
    vm.register_native("class_parents", |vm, a| {
        let class = match a.first() {
            Some(Val::Object(h)) => vm.objects.get(*h).class,
            Some(Val::Str(s)) => match vm.lookup_class(s.as_bytes()) { Some(c) => c, None => return Ok(Val::Bool(false)) },
            _ => return Ok(Val::Bool(false)),
        };
        let mut out = PhpArray::new();
        let mut cur = vm.class(class).parent;
        while let Some(p) = cur {
            let n = vm.class(p).name.clone();
            out.insert(ArrayKey::Str(PhpString::from(n.as_slice())), Val::bytes(&n));
            cur = vm.class(p).parent;
        }
        Ok(Val::array(out))
    });
    vm.register_native("array_key_exists_placeholder", |_, _| Ok(Val::Null));
    vm.register_native("php_sapi_name", |_, _| Ok(Val::str("cli")));
    vm.register_native("php_uname", |_, _| Ok(Val::str("Linux")));
    vm.register_native("memory_get_usage", |_, _| Ok(Val::Int(2 * 1024 * 1024)));
    vm.register_native("memory_get_peak_usage", |_, _| Ok(Val::Int(2 * 1024 * 1024)));
    vm.register_native("getenv", |_, a| match a.first() {
        Some(Val::Str(s)) => Ok(std::env::var(s.to_string_lossy()).map(Val::from).unwrap_or(Val::Bool(false))),
        _ => Ok(Val::empty_array()),
    });
    vm.register_native("gc_collect_cycles", |_, _| Ok(Val::Int(0)));
    vm.register_native("set_time_limit", |_, _| Ok(Val::Bool(true)));
    vm.register_native("debug_zval_refcount", |_, _| Ok(Val::Int(1)));
    vm.register_native("array_key_exists_placeholder2", |_, _| Ok(Val::Null));
}

fn filter_var(_: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    args::count(a, 1, "filter_var")?;
    let filter = args::int_opt(a, 1, "filter_var", 516)?;
    let (flags, options) = match a.get(2) {
        Some(Val::Array(o)) => (
            o.get(&ArrayKey::from_str("flags")).map(|f| f.to_int()).transpose()?.unwrap_or(0),
            o.get(&ArrayKey::from_str("options")).cloned(),
        ),
        Some(Val::Int(f)) => (*f, None),
        _ => (0, None),
    };
    let null_on_failure = flags & 134217728 != 0;
    let fail = |default: Option<Val>| -> Val { default.unwrap_or(if null_on_failure { Val::Null } else { Val::Bool(false) }) };
    let default = match &options { Some(Val::Array(o)) => o.get(&ArrayKey::from_str("default")).cloned(), _ => None };
    let v = &a[0];
    let s = match v {
        Val::Str(s) => s.to_string_lossy(),
        Val::Int(i) => i.to_string(),
        Val::Float(f) => format_float(*f),
        Val::Bool(true) => "1".into(),
        Val::Bool(false) | Val::Null => String::new(),
        _ => return Ok(fail(default)),
    };
    let t = s.trim();
    Ok(match filter {
        516 => Val::from(s),
        274 => {
            // email: one @, non-empty local, dotted domain without spaces
            let ok = match t.split_once('@') {
                Some((l, d)) => !l.is_empty() && l.len() <= 64 && !l.contains(' ') && d.contains('.') && !d.starts_with('.') && !d.ends_with('.') && !d.contains("..") && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') && l.chars().all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+/=?^_`{|}~.-".contains(c)) && !l.starts_with('.') && !l.ends_with('.') && !l.contains(".."),
                None => false,
            };
            if ok { Val::from(t) } else { fail(default) }
        }
        257 => {
            let (min, max) = match &options {
                Some(Val::Array(o)) => (o.get(&ArrayKey::from_str("min_range")).map(|x| x.to_int()).transpose()?, o.get(&ArrayKey::from_str("max_range")).map(|x| x.to_int()).transpose()?),
                _ => (None, None),
            };
            let parsed = if flags & 2 != 0 && (t.starts_with("0x") || t.starts_with("0X")) {
                i64::from_str_radix(&t[2..], 16).ok()
            } else if flags & 1 != 0 && t.starts_with('0') && t.len() > 1 {
                i64::from_str_radix(&t[1..], 8).ok()
            } else {
                let body = t.strip_prefix('+').unwrap_or(t);
                if body.len() > 1 && body.starts_with('0') || body.starts_with("-0") && body.len() > 2 { None } else { t.parse::<i64>().ok() }
            };
            match parsed {
                Some(i) if min.is_none_or(|m| i >= m) && max.is_none_or(|m| i <= m) => Val::Int(i),
                _ => fail(default),
            }
        }
        259 => match parse_numeric(t.as_bytes()) {
            Some((n, true)) => Val::Float(n.as_f64()),
            _ => fail(default),
        },
        258 => match t.to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Val::Bool(true),
            "0" | "false" | "off" | "no" | "" => {
                if t.is_empty() && null_on_failure { Val::Bool(false) } else { Val::Bool(false) }
            }
            _ => if null_on_failure { Val::Null } else { Val::Bool(false) },
        },
        273 => {
            let ok = parse_url(t).is_some_and(|p| p.contains_key(&ArrayKey::from_str("scheme")) && p.contains_key(&ArrayKey::from_str("host"))) && !t.contains(' ');
            if ok { Val::from(t) } else { fail(default) }
        }
        275 => {
            let v4 = t.parse::<std::net::Ipv4Addr>().is_ok();
            let v6 = t.parse::<std::net::Ipv6Addr>().is_ok();
            let want_v4 = flags & 1048576 != 0;
            let want_v6 = flags & 2097152 != 0;
            let ok = (v4 && (!want_v6 || want_v4)) || (v6 && (!want_v4 || want_v6));
            if ok { Val::from(t) } else { fail(default) }
        }
        // FILTER_SANITIZE_NUMBER_INT / _FLOAT: keep digits and signs; the
        // float filter keeps `.`, `,`, `eE` only with ALLOW_FRACTION,
        // ALLOW_THOUSAND, ALLOW_SCIENTIFIC
        519 | 520 => {
            let keep = |c: char| {
                c.is_ascii_digit() || c == '+' || c == '-' || filter == 520 && (c == '.' && flags & 4096 != 0 || c == ',' && flags & 8192 != 0 || (c == 'e' || c == 'E') && flags & 16384 != 0)
            };
            Val::from(s.chars().filter(|c| keep(*c)).collect::<String>())
        }
        _ => Val::from(s),
    })
}

pub(crate) fn url_encode(b: &[u8], plus_for_space: bool) -> String {
    let mut out = String::new();
    for &c in b {
        match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => out.push(c as char),
            b'~' if !plus_for_space => out.push('~'),
            b' ' if plus_for_space => out.push('+'),
            _ => out.push_str(&format!("%{c:02X}")),
        }
    }
    out
}

pub(crate) fn url_decode(b: &[u8], plus_as_space: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' if plus_as_space => out.push(b' '),
            b'%' if i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 || (b[i] == b'%' && i + 2 < b.len()) => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Public properties of an object (http_build_query sees only those, like PHP).
fn public_props(vm: &Vm, h: ObjHandle) -> PhpArray {
    let obj = vm.objects.get(h);
    let class = vm.class(obj.class);
    let mut out = PhpArray::new();
    for (i, p) in class.props.iter().enumerate() {
        if p.visibility == crate::bytecode::Visibility::Public
            && let Some(v) = &obj.slots[i]
        {
            out.insert(ArrayKey::from_str(&String::from_utf8_lossy(&p.name)), v.clone());
        }
    }
    for (k, v) in &obj.dynamic {
        out.insert(ArrayKey::Str(k.clone()), v.clone());
    }
    out
}

/// `visiting`: objects on the current path; one met again is skipped (PHP).
fn build_query(vm: &mut Vm, data: &PhpArray, prefix_key: Option<String>, num_prefix: &str, raw: bool, out: &mut Vec<String>, visiting: &mut Vec<ObjHandle>) -> Result<(), Throw> {
    for (k, v) in data.iter() {
        let key = match (k, &prefix_key) {
            (ArrayKey::Int(i), None) => format!("{num_prefix}{i}"),
            (ArrayKey::Str(s), None) => s.to_string(),
            (ArrayKey::Int(i), Some(p)) => format!("{p}[{i}]"),
            (ArrayKey::Str(s), Some(p)) => format!("{p}[{s}]"),
        };
        match v {
            Val::Array(inner) => build_query(vm, inner, Some(key), num_prefix, raw, out, visiting)?,
            Val::Object(h) => {
                if visiting.contains(h) {
                    continue;
                }
                let inner = public_props(vm, *h);
                visiting.push(*h);
                let r = build_query(vm, &inner, Some(key), num_prefix, raw, out, visiting);
                visiting.pop();
                r?
            }
            Val::Null => {}
            other => {
                let s = vm.to_string_val(other)?;
                let enc = |b: &[u8]| if raw { url_encode(b, false) } else { url_encode(b, true) };
                out.push(format!("{}={}", enc(key.as_bytes()), enc(s.as_bytes())));
            }
        }
    }
    Ok(())
}

/// One `key=value` of a query string into `out`, as PHP registers request
/// variables (`php_register_variable_ex`): leading spaces of the name are
/// skipped, ` ` and `.` before the first `[` become `_`, an empty name is
/// dropped, a first `[` without `]` becomes `_` (the name is then plain),
/// `[]` appends, and anything after a `]` that is not `[` is ignored.
pub fn insert_query_key(out: &mut PhpArray, key: &str, val: Val) {
    let key = key.trim_start_matches(' ');
    let (base, rest) = match key.find('[') {
        Some(p) => (&key[..p], &key[p..]),
        None => (key, ""),
    };
    let base: String = base.chars().map(|c| if c == ' ' || c == '.' { '_' } else { c }).collect();
    if base.is_empty() {
        return;
    }
    // the indexes; a first `[` without `]` makes the whole name plain
    let mut path: Vec<&str> = Vec::new();
    let mut r = rest;
    while let Some(after) = r.strip_prefix('[') {
        let Some(end) = after.find(']') else {
            if path.is_empty() {
                let plain: String = after.chars().map(|c| if matches!(c, ' ' | '.' | '[') { '_' } else { c }).collect();
                out.insert(ArrayKey::from_str(&format!("{base}_{plain}")), val);
                return;
            }
            break;
        };
        path.push(&after[..end]);
        r = &after[end + 1..];
    }
    let mut cur = out.entry_mut(ArrayKey::from_str(&base));
    for (n, part) in path.iter().enumerate() {
        if !matches!(cur, Val::Array(_)) {
            *cur = Val::empty_array();
        }
        let arr = cur.array_mut().expect("array");
        let last = n + 1 == path.len();
        if part.is_empty() {
            if last {
                arr.push(val);
                return;
            }
            arr.push(Val::empty_array());
            cur = arr.last_mut().expect("pushed");
        } else {
            if last {
                arr.insert(ArrayKey::from_str(part), val);
                return;
            }
            cur = arr.entry_mut(ArrayKey::from_str(part));
        }
    }
    *cur = val;
}

fn parse_url(s: &str) -> Option<PhpArray> {
    let mut out = PhpArray::new();
    let mut rest = s;
    if let Some(p) = rest.find("://") {
        let scheme = &rest[..p];
        if scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') && !scheme.is_empty() {
            out.insert(ArrayKey::from_str("scheme"), Val::from(scheme));
            rest = &rest[p + 3..];
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let authority = &rest[..end];
            rest = &rest[end..];
            let (userinfo, hostport) = match authority.rfind('@') {
                Some(a) => (Some(&authority[..a]), &authority[a + 1..]),
                None => (None, authority),
            };
            let (host, port) = match hostport.rfind(':') {
                Some(c) if hostport[c + 1..].chars().all(|ch| ch.is_ascii_digit()) && !hostport[c + 1..].is_empty() && !hostport.ends_with(']') => (&hostport[..c], Some(&hostport[c + 1..])),
                _ => (hostport, None),
            };
            if host.is_empty() {
                return None;
            }
            out.insert(ArrayKey::from_str("host"), Val::from(host));
            if let Some(p) = port {
                out.insert(ArrayKey::from_str("port"), Val::Int(p.parse().unwrap_or(0)));
            }
            if let Some(ui) = userinfo {
                let (u, p) = match ui.split_once(':') { Some((u, p)) => (u, Some(p)), None => (ui, None) };
                out.insert(ArrayKey::from_str("user"), Val::from(u));
                if let Some(p) = p {
                    out.insert(ArrayKey::from_str("pass"), Val::from(p));
                }
            }
        }
    } else if let Some(p) = rest.find(':') && rest[..p].chars().all(|c| c.is_ascii_alphabetic()) && !rest[..p].is_empty() && !rest[p + 1..].starts_with("//") {
        out.insert(ArrayKey::from_str("scheme"), Val::from(&rest[..p]));
        rest = &rest[p + 1..];
    }
    let (before_frag, frag) = match rest.find('#') { Some(h) => (&rest[..h], Some(&rest[h + 1..])), None => (rest, None) };
    let (path, query) = match before_frag.find('?') { Some(q) => (&before_frag[..q], Some(&before_frag[q + 1..])), None => (before_frag, None) };
    if !path.is_empty() {
        out.insert(ArrayKey::from_str("path"), Val::from(path));
    }
    if let Some(q) = query {
        out.insert(ArrayKey::from_str("query"), Val::from(q));
    }
    if let Some(f) = frag {
        out.insert(ArrayKey::from_str("fragment"), Val::from(f));
    }
    Some(out)
}

fn version_cmp(a: &str, b: &str) -> i64 {
    fn canon(v: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut last_digit: Option<bool> = None;
        for c in v.chars() {
            let is_digit = c.is_ascii_digit();
            if "-_+.".contains(c) {
                if !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
                last_digit = None;
                continue;
            }
            if let Some(ld) = last_digit && ld != is_digit && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(c);
            last_digit = Some(is_digit);
        }
        if !cur.is_empty() { out.push(cur); }
        out
    }
    fn order(s: &str) -> i64 {
        let l = s.to_ascii_lowercase();
        if l.chars().all(|c| c.is_ascii_digit()) { return 10; }
        match l.as_str() { "dev" => 0, "alpha" | "a" => 1, "beta" | "b" => 2, "rc" | "c" => 3, "#" => 4, "pl" | "p" => 5, _ => -1 }
    }
    let (x, y) = (canon(a), canon(b));
    let n = x.len().max(y.len());
    for i in 0..n {
        let (p, q) = (x.get(i), y.get(i));
        let r = match (p, q) {
            (Some(p), Some(q)) => {
                let (op, oq) = (order(p), order(q));
                if op == 10 && oq == 10 {
                    p.parse::<i64>().unwrap_or(0).cmp(&q.parse::<i64>().unwrap_or(0)) as i64
                } else {
                    op.cmp(&oq) as i64
                }
            }
            (Some(p), None) => if order(p) == 10 { 1 } else { -1 },
            (None, Some(q)) => if order(q) == 10 { -1 } else { 1 },
            (None, None) => 0,
        };
        if r != 0 { return r; }
    }
    0
}

/// Numbering of serialized values, as PHP does: every value (not the keys)
/// gets the next number; an object met again is written as `r:N;`, the
/// number of its first appearance, so cycles and shared objects survive.
#[derive(Default)]
struct Serialized {
    n: i64,
    seen: std::collections::HashMap<ObjHandle, i64>,
}

fn serialize_key(k: &ArrayKey, out: &mut Vec<u8>) {
    match k {
        ArrayKey::Int(i) => out.extend_from_slice(format!("i:{i};").as_bytes()),
        ArrayKey::Str(s) => {
            out.extend_from_slice(format!("s:{}:\"", s.len()).as_bytes());
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\";");
        }
    }
}

fn serialize(vm: &mut Vm, v: &Val, out: &mut Vec<u8>, st: &mut Serialized) -> Result<(), Throw> {
    st.n += 1;
    match v {
        Val::Null => out.extend_from_slice(b"N;"),
        Val::Bool(b) => out.extend_from_slice(format!("b:{};", *b as u8).as_bytes()),
        Val::Int(i) => out.extend_from_slice(format!("i:{i};").as_bytes()),
        Val::Float(f) => out.extend_from_slice(format!("d:{};", crate::value::repr_float(*f, b'E')).as_bytes()),
        Val::Str(s) => {
            out.extend_from_slice(format!("s:{}:\"", s.len()).as_bytes());
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(b"\";");
        }
        Val::Array(a) => {
            out.extend_from_slice(format!("a:{}:{{", a.len()).as_bytes());
            for (k, val) in a.iter() {
                serialize_key(&k, out);
                serialize(vm, val, out, st)?;
            }
            out.push(b'}');
        }
        Val::Object(h) => {
            if let Some(first) = st.seen.get(h) {
                out.extend_from_slice(format!("r:{first};").as_bytes());
                return Ok(());
            }
            st.seen.insert(*h, st.n);
            let class = vm.objects.get(*h).class;
            let name = vm.class(class).name.clone();
            let props = if vm.class(class).find_method(b"__serialize").is_some() {
                match vm.call_method_sync(*h, b"__serialize", Vec::new())? {
                    Val::Array(a) => (*a).clone(),
                    _ => return Err(Throw::type_error("__serialize() must return an array")),
                }
            } else {
                vm.object_to_array(*h)
            };
            out.extend_from_slice(format!("O:{}:\"{}\":{}:{{", name.len(), String::from_utf8_lossy(&name), props.len()).as_bytes());
            for (k, val) in props.iter() {
                serialize_key(&k, out);
                serialize(vm, val, out, st)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

/// One value; `table` holds the values read so far in PHP's numbering (keys
/// excluded), for `r:N;` back references.
fn unserialize(vm: &mut Vm, b: &[u8], pos: &mut usize, table: &mut Vec<Val>) -> Result<Val, ()> {
    unserialize_inner(vm, b, pos, table, false)
}

fn unserialize_inner(vm: &mut Vm, b: &[u8], pos: &mut usize, table: &mut Vec<Val>, is_key: bool) -> Result<Val, ()> {
    let read_until = |pos: &mut usize, stop: u8| -> Result<String, ()> {
        let start = *pos;
        while *pos < b.len() && b[*pos] != stop {
            *pos += 1;
        }
        if *pos >= b.len() {
            return Err(());
        }
        let s = String::from_utf8_lossy(&b[start..*pos]).into_owned();
        *pos += 1;
        Ok(s)
    };
    // a delimiter must be exactly there, not just skipped
    let expect = |pos: &mut usize, c: u8| -> Result<(), ()> {
        if b.get(*pos) != Some(&c) {
            return Err(());
        }
        *pos += 1;
        Ok(())
    };
    let tag = *b.get(*pos).ok_or(())?;
    *pos += 1;
    // scalars are numbered right away; arrays and objects take their number
    // before their contents, so a reference inside them can point back
    let scalar = |v: Val, table: &mut Vec<Val>| {
        if !is_key {
            table.push(v.clone());
        }
        Ok(v)
    };
    match tag {
        b'N' => {
            expect(pos, b';')?;
            scalar(Val::Null, table)
        }
        b'b' => {
            expect(pos, b':')?;
            let v = match read_until(pos, b';')?.as_str() {
                "0" => false,
                "1" => true,
                _ => return Err(()),
            };
            scalar(Val::Bool(v), table)
        }
        b'i' => {
            expect(pos, b':')?;
            let v = read_until(pos, b';')?.parse().map_err(|_| ())?;
            scalar(Val::Int(v), table)
        }
        b'd' => {
            expect(pos, b':')?;
            let v = read_until(pos, b';')?.parse().map_err(|_| ())?;
            scalar(Val::Float(v), table)
        }
        b's' => {
            expect(pos, b':')?;
            let len: usize = read_until(pos, b':')?.parse().map_err(|_| ())?;
            expect(pos, b'"')?;
            let s = b.get(*pos..*pos + len).ok_or(())?.to_vec();
            *pos += len;
            expect(pos, b'"')?;
            expect(pos, b';')?;
            scalar(Val::bytes(&s), table)
        }
        b'r' if !is_key => {
            expect(pos, b':')?;
            let n: usize = read_until(pos, b';')?.parse().map_err(|_| ())?;
            // `r:` points at an object (PHP rejects it for anything else)
            let v = n.checked_sub(1).and_then(|i| table.get(i)).filter(|v| matches!(v, Val::Object(_))).cloned().ok_or(())?;
            table.push(v.clone());
            Ok(v)
        }
        b'a' if !is_key => {
            expect(pos, b':')?;
            let n: usize = read_until(pos, b':')?.parse().map_err(|_| ())?;
            expect(pos, b'{')?;
            let slot = table.len();
            table.push(Val::Null);
            let mut arr = PhpArray::new();
            for _ in 0..n {
                let k = unserialize_inner(vm, b, pos, table, true)?;
                let v = unserialize_inner(vm, b, pos, table, false)?;
                let key = vm.to_array_key(&k).map_err(|_| ())?;
                arr.insert(key, v);
            }
            expect(pos, b'}')?;
            let v = Val::array(arr);
            table[slot] = v.clone();
            Ok(v)
        }
        b'O' if !is_key => {
            expect(pos, b':')?;
            let len: usize = read_until(pos, b':')?.parse().map_err(|_| ())?;
            expect(pos, b'"')?;
            let name = read_until(pos, b'"')?;
            if name.len() != len {
                return Err(());
            }
            expect(pos, b':')?;
            let n: usize = read_until(pos, b':')?.parse().map_err(|_| ())?;
            expect(pos, b'{')?;
            let class = vm.lookup_class(name.as_bytes()).ok_or(())?;
            let h = vm.alloc_object(class).map_err(|_| ())?;
            table.push(Val::Object(h));
            let mut props = PhpArray::new();
            for _ in 0..n {
                let k = unserialize_inner(vm, b, pos, table, true)?;
                let v = unserialize_inner(vm, b, pos, table, false)?;
                let key = vm.to_array_key(&k).map_err(|_| ())?;
                props.insert(key, v);
            }
            expect(pos, b'}')?;
            if vm.class(class).find_method(b"__unserialize").is_some() {
                vm.call_method_sync(h, b"__unserialize", vec![Val::array(props)]).map_err(|_| ())?;
            } else {
                for (k, v) in props.iter() {
                    if let ArrayKey::Str(s) = k {
                        vm.set_prop_raw(h, s.as_bytes(), v.clone());
                    }
                }
            }
            Ok(Val::Object(h))
        }
        _ => Err(()),
    }
}
