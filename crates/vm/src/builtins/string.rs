//! String functions (byte semantics; `mb_*` on UTF-8 characters).

use super::args;
use crate::error::Throw;
use crate::value::*;
use crate::vm::Vm;

pub fn register(vm: &mut Vm) {
    vm.register_native("strlen", |_, a| Ok(Val::Int(args::string(a, 0, "strlen")?.len() as i64)));
    vm.register_native("mb_strlen", |_, a| Ok(Val::Int(args::string(a, 0, "mb_strlen")?.char_len() as i64)));
    vm.register_native("strtolower", |_, a| Ok(Val::bytes(&args::string(a, 0, "strtolower")?.as_bytes().to_ascii_lowercase())));
    vm.register_native("strtoupper", |_, a| Ok(Val::bytes(&args::string(a, 0, "strtoupper")?.as_bytes().to_ascii_uppercase())));
    vm.register_native("mb_strtolower", |_, a| Ok(Val::from(args::string(a, 0, "mb_strtolower")?.to_string_lossy().to_lowercase())));
    vm.register_native("mb_strtoupper", |_, a| Ok(Val::from(args::string(a, 0, "mb_strtoupper")?.to_string_lossy().to_uppercase())));
    vm.register_native("ucfirst", |_, a| {
        let mut b = args::string(a, 0, "ucfirst")?.as_bytes().to_vec();
        if let Some(f) = b.first_mut() {
            *f = f.to_ascii_uppercase();
        }
        Ok(Val::bytes(&b))
    });
    vm.register_native("lcfirst", |_, a| {
        let mut b = args::string(a, 0, "lcfirst")?.as_bytes().to_vec();
        if let Some(f) = b.first_mut() {
            *f = f.to_ascii_lowercase();
        }
        Ok(Val::bytes(&b))
    });
    vm.register_native("ucwords", |_, a| {
        let s = args::string(a, 0, "ucwords")?;
        let delims = match a.get(1) { Some(_) => args::string(a, 1, "ucwords")?.as_bytes().to_vec(), None => b" \t\r\n\x0c\x0b".to_vec() };
        let mut out = s.as_bytes().to_vec();
        let mut up = true;
        for b in out.iter_mut() {
            if up {
                *b = b.to_ascii_uppercase();
            }
            up = delims.contains(b);
        }
        Ok(Val::bytes(&out))
    });
    vm.register_native("trim", |_, a| trim(a, true, true));
    vm.register_native("ltrim", |_, a| trim(a, true, false));
    vm.register_native("rtrim", |_, a| trim(a, false, true));
    vm.register_native("chop", |_, a| trim(a, false, true));
    vm.register_native("str_repeat", |_, a| {
        let s = args::string(a, 0, "str_repeat")?;
        let n = args::int(a, 1, "str_repeat")?;
        if n < 0 {
            return Err(Throw::value_error("str_repeat(): Argument #2 ($times) must be greater than or equal to 0"));
        }
        Ok(Val::bytes(&s.as_bytes().repeat(n as usize)))
    });
    vm.register_native("strrev", |_, a| {
        let mut b = args::string(a, 0, "strrev")?.as_bytes().to_vec();
        b.reverse();
        Ok(Val::bytes(&b))
    });
    vm.register_native("str_contains", |_, a| Ok(Val::Bool(find(args::string(a, 0, "str_contains")?.as_bytes(), args::string(a, 1, "str_contains")?.as_bytes(), 0).is_some())));
    vm.register_native("str_starts_with", |_, a| Ok(Val::Bool(args::string(a, 0, "str_starts_with")?.as_bytes().starts_with(args::string(a, 1, "str_starts_with")?.as_bytes()))));
    vm.register_native("str_ends_with", |_, a| Ok(Val::Bool(args::string(a, 0, "str_ends_with")?.as_bytes().ends_with(args::string(a, 1, "str_ends_with")?.as_bytes()))));
    vm.register_native("strpos", |_, a| strpos(a, "strpos", false, false));
    vm.register_native("stripos", |_, a| strpos(a, "stripos", true, false));
    vm.register_native("strrpos", |_, a| strpos(a, "strrpos", false, true));
    vm.register_native("strripos", |_, a| strpos(a, "strripos", true, true));
    vm.register_native("mb_strrpos", |_, a| {
        let h = args::string(a, 0, "mb_strrpos")?.to_string_lossy();
        let n = args::string(a, 1, "mb_strrpos")?.to_string_lossy();
        Ok(match h.rfind(&*n) {
            Some(bi) => Val::Int(h[..bi].chars().count() as i64),
            None => Val::Bool(false),
        })
    });
    vm.register_native("mb_strpos", |_, a| {
        let h = args::string(a, 0, "mb_strpos")?.to_string_lossy();
        let n = args::string(a, 1, "mb_strpos")?.to_string_lossy();
        let off = args::int_opt(a, 2, "mb_strpos", 0)? as usize;
        let skipped: String = h.chars().skip(off).collect();
        Ok(match skipped.find(&n) {
            Some(bi) => Val::Int((skipped[..bi].chars().count() + off) as i64),
            None => Val::Bool(false),
        })
    });
    vm.register_native("substr", |_, a| {
        let s = args::string(a, 0, "substr")?;
        let (start, len) = (args::int(a, 1, "substr")?, match a.get(2) { None | Some(Val::Null) => None, _ => Some(args::int(a, 2, "substr")?) });
        Ok(Val::bytes(substr_bytes(s.as_bytes(), start, len)))
    });
    vm.register_native("mb_substr", |_, a| {
        let s = args::string(a, 0, "mb_substr")?.to_string_lossy();
        let chars: Vec<char> = s.chars().collect();
        let (start, len) = (args::int(a, 1, "mb_substr")?, match a.get(2) { None | Some(Val::Null) => None, _ => Some(args::int(a, 2, "mb_substr")?) });
        let (from, to) = substr_range(chars.len(), start, len);
        Ok(Val::from(chars[from..to].iter().collect::<String>()))
    });
    vm.register_native("substr_count", |_, a| {
        let h = args::string(a, 0, "substr_count")?;
        let n = args::string(a, 1, "substr_count")?;
        if n.is_empty() {
            return Err(Throw::value_error("substr_count(): Argument #2 ($needle) cannot be empty"));
        }
        let mut count = 0;
        let mut i = 0;
        while let Some(p) = find(h.as_bytes(), n.as_bytes(), i) {
            count += 1;
            i = p + n.len();
        }
        Ok(Val::Int(count))
    });
    vm.register_native("str_replace", |vm, a| str_replace(vm, a, false));
    vm.register_native("str_ireplace", |vm, a| str_replace(vm, a, true));
    vm.register_native("strtr", strtr);
    vm.register_native("substr_replace", |_, a| {
        let s = args::string(a, 0, "substr_replace")?;
        let r = args::string(a, 1, "substr_replace")?;
        let start = args::int(a, 2, "substr_replace")?;
        let len = match a.get(3) { None | Some(Val::Null) => None, _ => Some(args::int(a, 3, "substr_replace")?) };
        let (from, to) = substr_range(s.len(), start, len);
        let mut out = s.as_bytes()[..from].to_vec();
        out.extend_from_slice(r.as_bytes());
        out.extend_from_slice(&s.as_bytes()[to..]);
        Ok(Val::bytes(&out))
    });
    vm.register_native("explode", |_, a| {
        let d = args::string(a, 0, "explode")?;
        let s = args::string(a, 1, "explode")?;
        let limit = args::int_opt(a, 2, "explode", i64::MAX)?;
        if d.is_empty() {
            return Err(Throw::value_error("explode(): Argument #1 ($separator) cannot be empty"));
        }
        let mut parts: Vec<Vec<u8>> = Vec::new();
        let (hay, del) = (s.as_bytes(), d.as_bytes());
        let mut i = 0;
        while let Some(p) = find(hay, del, i) {
            parts.push(hay[i..p].to_vec());
            i = p + del.len();
        }
        parts.push(hay[i..].to_vec());
        let parts: Vec<Vec<u8>> = if limit > 0 && (parts.len() as i64) > limit {
            let keep = (limit - 1) as usize;
            let mut head: Vec<Vec<u8>> = parts[..keep].to_vec();
            head.push(parts[keep..].join(&del[..]));
            head
        } else if limit < 0 {
            let drop = (-limit) as usize;
            if drop >= parts.len() { Vec::new() } else { parts[..parts.len() - drop].to_vec() }
        } else {
            parts
        };
        Ok(Val::array(PhpArray::from_values(parts.into_iter().map(|p| Val::bytes(&p)))))
    });
    vm.register_native("implode", implode);
    vm.register_native("join", implode);
    vm.register_native("str_split", |_, a| {
        let s = args::string(a, 0, "str_split")?;
        let n = args::int_opt(a, 1, "str_split", 1)?.max(1) as usize;
        Ok(Val::array(PhpArray::from_values(s.as_bytes().chunks(n).map(Val::bytes))))
    });
    vm.register_native("mb_str_split", |_, a| {
        let s = args::string(a, 0, "mb_str_split")?.to_string_lossy();
        let n = args::int_opt(a, 1, "mb_str_split", 1)?.max(1) as usize;
        let chars: Vec<char> = s.chars().collect();
        Ok(Val::array(PhpArray::from_values(chars.chunks(n).map(|c| Val::from(c.iter().collect::<String>())))))
    });
    vm.register_native("str_pad", |_, a| {
        let s = args::string(a, 0, "str_pad")?;
        let len = args::int(a, 1, "str_pad")?;
        let pad = match a.get(2) { Some(_) => args::string(a, 2, "str_pad")?.as_bytes().to_vec(), None => b" ".to_vec() };
        let kind = args::int_opt(a, 3, "str_pad", 1)?;
        if pad.is_empty() {
            return Err(Throw::value_error("str_pad(): Argument #3 ($pad_string) must be a non-empty string"));
        }
        let cur = s.len() as i64;
        if len <= cur {
            return Ok(Val::Str(s));
        }
        let total = (len - cur) as usize;
        let (left, right) = match kind {
            0 => (total, 0),
            2 => (total / 2, total - total / 2),
            _ => (0, total),
        };
        let mk = |n: usize| -> Vec<u8> { pad.iter().cycle().take(n).copied().collect() };
        let mut out = mk(left);
        out.extend_from_slice(s.as_bytes());
        out.extend(mk(right));
        Ok(Val::bytes(&out))
    });
    vm.register_native("wordwrap", |_, a| {
        let text = args::string(a, 0, "wordwrap")?;
        let width = args::int_opt(a, 1, "wordwrap", 75)?;
        let brk = match a.get(2) { Some(_) => args::string(a, 2, "wordwrap")?.as_bytes().to_vec(), None => b"\n".to_vec() };
        let cut = args::boolean(a, 3, false)?;
        if text.is_empty() {
            return Ok(Val::str(""));
        }
        if brk.is_empty() {
            return Err(Throw::value_error("wordwrap(): Argument #3 ($break) must not be empty"));
        }
        if width == 0 && cut {
            return Err(Throw::value_error("wordwrap(): Argument #4 ($cut_long_words) cannot be true when argument #2 ($width) is 0"));
        }
        Ok(Val::bytes(&wordwrap(text.as_bytes(), width, &brk, cut)))
    });
    vm.register_native("nl2br", |_, a| Ok(Val::from(args::string(a, 0, "nl2br")?.to_string_lossy().replace("\r\n", "<br />\r\n").replace('\n', "<br />\n"))));
    vm.register_native("strcmp", |_, a| Ok(Val::Int(byte_diff(args::string(a, 0, "strcmp")?.as_bytes(), args::string(a, 1, "strcmp")?.as_bytes()))));
    vm.register_native("strcasecmp", |_, a| Ok(Val::Int(byte_diff(&args::string(a, 0, "strcasecmp")?.as_bytes().to_ascii_lowercase(), &args::string(a, 1, "strcasecmp")?.as_bytes().to_ascii_lowercase()))));
    vm.register_native("strncmp", |_, a| {
        let n = non_negative_length(a, "strncmp")?;
        let x = args::string(a, 0, "strncmp")?;
        let y = args::string(a, 1, "strncmp")?;
        Ok(Val::Int(byte_diff(&x.as_bytes()[..n.min(x.len())], &y.as_bytes()[..n.min(y.len())])))
    });
    vm.register_native("strncasecmp", |_, a| {
        let n = non_negative_length(a, "strncasecmp")?;
        let x = args::string(a, 0, "strncasecmp")?.as_bytes().to_ascii_lowercase();
        let y = args::string(a, 1, "strncasecmp")?.as_bytes().to_ascii_lowercase();
        Ok(Val::Int(byte_diff(&x[..n.min(x.len())], &y[..n.min(y.len())])))
    });
    vm.register_native("strnatcmp", |_, a| Ok(Val::Int(cmp_ord(natural_cmp(args::string(a, 0, "strnatcmp")?.as_bytes(), args::string(a, 1, "strnatcmp")?.as_bytes(), false)))));
    vm.register_native("strnatcasecmp", |_, a| Ok(Val::Int(cmp_ord(natural_cmp(args::string(a, 0, "strnatcasecmp")?.as_bytes(), args::string(a, 1, "strnatcasecmp")?.as_bytes(), true)))));
    vm.register_native("sprintf", |vm, a| Ok(Val::Str(sprintf(vm, a, "sprintf")?)));
    vm.register_native("vsprintf", |vm, a| {
        let f = args::string(a, 0, "vsprintf")?;
        let arr = args::array(a, 1, "vsprintf")?;
        let mut v = vec![Val::Str(f)];
        v.extend(arr.values().cloned());
        Ok(Val::Str(sprintf(vm, &v, "vsprintf")?))
    });
    vm.register_native("printf", |vm, a| {
        let s = sprintf(vm, a, "printf")?;
        vm.output.extend_from_slice(s.as_bytes());
        Ok(Val::Int(s.len() as i64))
    });
    vm.register_native("number_format_placeholder", |_, _| Ok(Val::Null));
    vm.register_native("htmlspecialchars", |_, a| Ok(Val::from(html_escape(&args::string(a, 0, "htmlspecialchars")?.to_string_lossy(), args::int_opt(a, 1, "htmlspecialchars", 3)?))));
    vm.register_native("htmlentities", |_, a| Ok(Val::from(html_escape(&args::string(a, 0, "htmlentities")?.to_string_lossy(), args::int_opt(a, 1, "htmlentities", 3)?))));
    vm.register_native("htmlspecialchars_decode", |_, a| Ok(Val::from(args::string(a, 0, "htmlspecialchars_decode")?.to_string_lossy().replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#039;", "'").replace("&amp;", "&"))));
    vm.register_native("html_entity_decode", |_, a| Ok(Val::from(args::string(a, 0, "html_entity_decode")?.to_string_lossy().replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#039;", "'").replace("&#39;", "'").replace("&nbsp;", "\u{a0}").replace("&amp;", "&"))));
    vm.register_native("strip_tags", |_, a| {
        let s = args::string(a, 0, "strip_tags")?.to_string_lossy();
        let mut out = String::new();
        let mut depth = 0;
        for c in s.chars() {
            match c {
                '<' => depth += 1,
                '>' if depth > 0 => depth -= 1,
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        Ok(Val::from(out))
    });
    vm.register_native("addslashes", |_, a| {
        let s = args::string(a, 0, "addslashes")?;
        let mut out = Vec::new();
        for b in s.as_bytes() {
            if matches!(b, b'\'' | b'"' | b'\\') {
                out.push(b'\\');
            }
            if *b == 0 {
                out.extend_from_slice(b"\\0");
            } else {
                out.push(*b);
            }
        }
        Ok(Val::bytes(&out))
    });
    vm.register_native("stripslashes", |_, a| {
        let s = args::string(a, 0, "stripslashes")?;
        let mut out = Vec::new();
        let b = s.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 1 < b.len() {
                out.push(if b[i + 1] == b'0' { 0 } else { b[i + 1] });
                i += 2;
            } else if b[i] != b'\\' {
                out.push(b[i]);
                i += 1;
            } else {
                i += 1;
            }
        }
        Ok(Val::bytes(&out))
    });
    vm.register_native("chr", |_, a| Ok(Val::bytes(&[(args::int(a, 0, "chr")?.rem_euclid(256)) as u8])));
    vm.register_native("ord", |_, a| Ok(Val::Int(args::string(a, 0, "ord")?.as_bytes().first().copied().unwrap_or(0) as i64)));
    vm.register_native("bin2hex", |_, a| Ok(Val::from(args::string(a, 0, "bin2hex")?.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>())));
    vm.register_native("hex2bin", |_, a| {
        let s = args::string(a, 0, "hex2bin")?;
        let b = s.as_bytes();
        if b.len() % 2 != 0 {
            return Ok(Val::Bool(false));
        }
        let mut out = Vec::new();
        for pair in b.chunks(2) {
            match u8::from_str_radix(std::str::from_utf8(pair).unwrap_or("zz"), 16) {
                Ok(v) => out.push(v),
                Err(_) => return Ok(Val::Bool(false)),
            }
        }
        Ok(Val::bytes(&out))
    });
    vm.register_native("dechex_placeholder", |_, _| Ok(Val::Null));
    vm.register_native("base64_encode", |_, a| Ok(Val::from(base64_encode(args::string(a, 0, "base64_encode")?.as_bytes()))));
    vm.register_native("base64_decode", |_, a| Ok(base64_decode(args::string(a, 0, "base64_decode")?.as_bytes()).map(|b| Val::bytes(&b)).unwrap_or(Val::Bool(false))));
    vm.register_native("crc32", |_, a| Ok(Val::Int(crc32(args::string(a, 0, "crc32")?.as_bytes()) as i64)));
    vm.register_native("uniqid", |_, a| {
        let prefix = match a.first() { Some(_) => args::string(a, 0, "uniqid")?.to_string(), None => String::new() };
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        Ok(Val::from(format!("{prefix}{:08x}{:05x}", t.as_secs(), t.subsec_micros())))
    });
    vm.register_native("lcg_placeholder", |_, _| Ok(Val::Null));
    vm.register_native("str_word_count", |_, a| Ok(Val::Int(args::string(a, 0, "str_word_count")?.to_string_lossy().split(|c: char| !c.is_alphabetic() && c != '\'' && c != '-').filter(|w| !w.is_empty()).count() as i64)));
    vm.register_native("similar_text", |_, a| {
        let x = args::string(a, 0, "similar_text")?;
        let y = args::string(a, 1, "similar_text")?;
        Ok(Val::Int(similar_text(x.as_bytes(), y.as_bytes()) as i64))
    });
    vm.register_native("levenshtein", |_, a| {
        let x = args::string(a, 0, "levenshtein")?;
        let y = args::string(a, 1, "levenshtein")?;
        Ok(Val::Int(levenshtein(x.as_bytes(), y.as_bytes()) as i64))
    });
    vm.register_native("ctype_digit", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(s)) if !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_digit)))));
    vm.register_native("ctype_alpha", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(s)) if !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_alphabetic)))));
    vm.register_native("ctype_alnum", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(s)) if !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_alphanumeric)))));
    vm.register_native("ctype_upper", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(s)) if !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_uppercase)))));
    vm.register_native("ctype_lower", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(s)) if !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_lowercase)))));
    vm.register_native("ctype_space", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(s)) if !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_whitespace)))));
    vm.register_native("ctype_punct", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(s)) if !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_punctuation)))));
    vm.register_native("ctype_xdigit", |_, a| Ok(Val::Bool(matches!(a.first(), Some(Val::Str(s)) if !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_hexdigit)))));
    vm.register_native("mb_convert_case", |_, a| {
        let s = args::string(a, 0, "mb_convert_case")?.to_string_lossy();
        // MB_CASE_UPPER/LOWER/TITLE/FOLD and their *_SIMPLE variants (4..7)
        Ok(Val::from(match args::int(a, 1, "mb_convert_case")? {
            0 | 4 => s.to_uppercase(),
            1 | 5 => s.to_lowercase(),
            2 | 6 => title_case(&s),
            3 => s.to_lowercase().replace('ß', "ss"),
            7 => s.to_lowercase(),
            m => return Err(Throw::value_error(format!("mb_convert_case(): Argument #2 ($mode) must be one of the MB_CASE_* constants, {m} given"))),
        }))
    });
    vm.register_native("mb_strwidth", |_, a| Ok(Val::Int(args::string(a, 0, "mb_strwidth")?.to_string_lossy().chars().map(char_width).sum())));
    vm.register_native("mb_substr_count", |_, a| Ok(Val::Int(args::string(a, 0, "mb_substr_count")?.to_string_lossy().matches(&*args::string(a, 1, "mb_substr_count")?.to_string_lossy()).count() as i64)));
    vm.register_native("mb_strrev_placeholder", |_, _| Ok(Val::Null));
    vm.register_native("mb_check_encoding", |_, a| Ok(Val::Bool(args::string(a, 0, "mb_check_encoding")?.as_str().is_some())));
    vm.register_native("mb_internal_encoding", |_, a| Ok(if a.is_empty() { Val::str("UTF-8") } else { Val::Bool(true) }));
    vm.register_native("mb_convert_encoding", |_, a| Ok(Val::Str(args::string(a, 0, "mb_convert_encoding")?)));
    vm.register_native("utf8_encode", |_, a| Ok(Val::from(args::string(a, 0, "utf8_encode")?.as_bytes().iter().map(|b| *b as char).collect::<String>())));
    vm.register_native("utf8_decode", |_, a| Ok(Val::bytes(&args::string(a, 0, "utf8_decode")?.to_string_lossy().chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect::<Vec<u8>>())));
    vm.register_native("quotemeta", |_, a| {
        let s = args::string(a, 0, "quotemeta")?;
        let mut out = Vec::new();
        for b in s.as_bytes() {
            if b".\\+*?[^]$()".contains(b) {
                out.push(b'\\');
            }
            out.push(*b);
        }
        Ok(Val::bytes(&out))
    });
    vm.register_native("nl_langinfo_placeholder", |_, _| Ok(Val::Null));
    vm.register_native("strstr", |_, a| {
        let h = args::string(a, 0, "strstr")?;
        let n = args::string(a, 1, "strstr")?;
        let before = args::boolean(a, 2, false)?;
        Ok(match find(h.as_bytes(), n.as_bytes(), 0) {
            Some(p) => Val::bytes(if before { &h.as_bytes()[..p] } else { &h.as_bytes()[p..] }),
            None => Val::Bool(false),
        })
    });
    vm.register_native("strrchr", |_, a| {
        let h = args::string(a, 0, "strrchr")?;
        let n = args::string(a, 1, "strrchr")?;
        let Some(c) = n.as_bytes().first() else { return Ok(Val::Bool(false)) };
        Ok(match h.as_bytes().iter().rposition(|b| b == c) {
            Some(p) => Val::bytes(&h.as_bytes()[p..]),
            None => Val::Bool(false),
        })
    });
    vm.register_native("stristr", |_, a| {
        let h = args::string(a, 0, "stristr")?;
        let n = args::string(a, 1, "stristr")?;
        Ok(match find(&h.as_bytes().to_ascii_lowercase(), &n.as_bytes().to_ascii_lowercase(), 0) {
            Some(p) => Val::bytes(&h.as_bytes()[p..]),
            None => Val::Bool(false),
        })
    });
    vm.register_native("strpbrk", |_, a| {
        let h = args::string(a, 0, "strpbrk")?;
        let chars = args::string(a, 1, "strpbrk")?;
        Ok(match h.as_bytes().iter().position(|b| chars.as_bytes().contains(b)) {
            Some(p) => Val::bytes(&h.as_bytes()[p..]),
            None => Val::Bool(false),
        })
    });
    vm.register_native("chunk_split", |_, a| {
        let s = args::string(a, 0, "chunk_split")?;
        let n = args::int_opt(a, 1, "chunk_split", 76)?.max(1) as usize;
        let end = match a.get(2) { Some(_) => args::string(a, 2, "chunk_split")?.as_bytes().to_vec(), None => b"\r\n".to_vec() };
        let mut out = Vec::new();
        for c in s.as_bytes().chunks(n) {
            out.extend_from_slice(c);
            out.extend_from_slice(&end);
        }
        Ok(Val::bytes(&out))
    });
    vm.register_native("money_placeholder", |_, _| Ok(Val::Null));
    // ---- BI1 (ANALYSE_D13 §A3) ---------------------------------------------
    vm.register_native("basename", |_, a| {
        let p = args::string(a, 0, "basename")?.as_bytes().to_vec();
        let suffix = match a.get(1) { Some(_) => args::string(a, 1, "basename")?.as_bytes().to_vec(), None => Vec::new() };
        let mut base = basename_bytes(&p).to_vec();
        if !suffix.is_empty() && base.len() > suffix.len() && base.ends_with(&suffix) {
            base.truncate(base.len() - suffix.len());
        }
        Ok(Val::bytes(&base))
    });
    vm.register_native("dirname", |_, a| {
        let mut p = args::string(a, 0, "dirname")?.as_bytes().to_vec();
        let levels = args::int_opt(a, 1, "dirname", 1)?;
        if levels < 1 {
            return Err(Throw::value_error("dirname(): Argument #2 ($levels) must be greater than or equal to 1"));
        }
        for _ in 0..levels {
            let next = dirname_bytes(&p);
            if next == p {
                break;
            }
            p = next;
        }
        Ok(Val::bytes(&p))
    });
    vm.register_native("pathinfo", |_, a| {
        let p = args::string(a, 0, "pathinfo")?.as_bytes().to_vec();
        let flags = args::int_opt(a, 1, "pathinfo", 15)?;
        let base = basename_bytes(&p).to_vec();
        let dot = base.iter().rposition(|&c| c == b'.');
        let mut out = PhpArray::new();
        // like php-src: the directory part only when it is not empty
        let dir = dirname_bytes(&p);
        if flags & 1 != 0 && !dir.is_empty() {
            out.insert(ArrayKey::from_str("dirname"), Val::bytes(&dir));
        }
        if flags & 2 != 0 {
            out.insert(ArrayKey::from_str("basename"), Val::bytes(&base));
        }
        if flags & 4 != 0 && let Some(d) = dot {
            out.insert(ArrayKey::from_str("extension"), Val::bytes(&base[d + 1..]));
        }
        if flags & 8 != 0 {
            out.insert(ArrayKey::from_str("filename"), Val::bytes(&base[..dot.unwrap_or(base.len())]));
        }
        // a single flag returns the element itself ("" when missing)
        if matches!(flags, 1 | 2 | 4 | 8) {
            return Ok(out.values().next().cloned().unwrap_or_else(|| Val::str("")));
        }
        Ok(Val::array(out))
    });
    vm.register_native("mb_str_pad", |_, a| {
        let s: Vec<char> = args::string(a, 0, "mb_str_pad")?.to_string_lossy().chars().collect();
        let len = args::int(a, 1, "mb_str_pad")?;
        let pad: Vec<char> = match a.get(2) { Some(_) => args::string(a, 2, "mb_str_pad")?.to_string_lossy().chars().collect(), None => vec![' '] };
        let kind = args::int_opt(a, 3, "mb_str_pad", 1)?;
        if pad.is_empty() {
            return Err(Throw::value_error("mb_str_pad(): Argument #3 ($pad_string) must be a non-empty string"));
        }
        if len <= s.len() as i64 {
            return Ok(Val::from(s.iter().collect::<String>()));
        }
        let total = len as usize - s.len();
        let (left, right) = match kind {
            0 => (total, 0),
            2 => (total / 2, total - total / 2),
            _ => (0, total),
        };
        let mk = |n: usize| -> String { pad.iter().cycle().take(n).collect() };
        Ok(Val::from(format!("{}{}{}", mk(left), s.iter().collect::<String>(), mk(right))))
    });
    vm.register_native("mb_strimwidth", |_, a| {
        let s: Vec<char> = args::string(a, 0, "mb_strimwidth")?.to_string_lossy().chars().collect();
        let start = args::int(a, 1, "mb_strimwidth")?;
        let width = args::int(a, 2, "mb_strimwidth")?;
        let marker: Vec<char> = match a.get(3) { Some(_) => args::string(a, 3, "mb_strimwidth")?.to_string_lossy().chars().collect(), None => Vec::new() };
        let start = if start < 0 { (s.len() as i64 + start).max(0) as usize } else { (start as usize).min(s.len()) };
        let rest = &s[start..];
        let total: i64 = rest.iter().map(|c| char_width(*c)).sum();
        let width = if width < 0 { total + width } else { width };
        if total <= width {
            return Ok(Val::from(rest.iter().collect::<String>()));
        }
        let marker_w: i64 = marker.iter().map(|c| char_width(*c)).sum();
        let mut out = String::new();
        let mut w = 0;
        for c in rest {
            if w + char_width(*c) > width - marker_w {
                break;
            }
            w += char_width(*c);
            out.push(*c);
        }
        out.extend(marker.iter());
        Ok(Val::from(out))
    });
    vm.register_native("mb_ucfirst", |_, a| {
        let s = args::string(a, 0, "mb_ucfirst")?.to_string_lossy();
        let mut c = s.chars();
        Ok(Val::from(match c.next() { Some(f) => title_case_char(f) + c.as_str(), None => String::new() }))
    });
    vm.register_native("mb_lcfirst", |_, a| {
        let s = args::string(a, 0, "mb_lcfirst")?.to_string_lossy();
        let mut c = s.chars();
        Ok(Val::from(match c.next() { Some(f) => f.to_lowercase().collect::<String>() + c.as_str(), None => String::new() }))
    });
    // strtok($string, $token) starts, strtok($token) continues (state per VM, cleared per request)
    vm.register_native("strtok", |vm, a| {
        let (src, pos, tokens) = if a.len() >= 2 {
            (args::string(a, 0, "strtok")?.as_bytes().to_vec(), 0usize, args::string(a, 1, "strtok")?.as_bytes().to_vec())
        } else {
            let tokens = args::string(a, 0, "strtok")?.as_bytes().to_vec();
            match vm.strtok.take() {
                Some((src, pos)) => (src, pos, tokens),
                None => return Ok(Val::Bool(false)),
            }
        };
        let mut i = pos;
        while i < src.len() && tokens.contains(&src[i]) {
            i += 1;
        }
        if i >= src.len() {
            vm.strtok = None;
            return Ok(Val::Bool(false));
        }
        let mut j = i;
        while j < src.len() && !tokens.contains(&src[j]) {
            j += 1;
        }
        let tok = src[i..j].to_vec();
        vm.strtok = Some((src, (j + 1).min(usize::MAX)));
        Ok(Val::bytes(&tok))
    });
    // sscanf returns the parsed values (the dialect has no by-reference arguments)
    vm.register_native("sscanf", |_, a| {
        let s = args::string(a, 0, "sscanf")?.as_bytes().to_vec();
        let f = args::string(a, 1, "sscanf")?.as_bytes().to_vec();
        Ok(sscanf(&s, &f))
    });
}

pub(crate) fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= hay.len()).then_some(from);
    }
    if from >= hay.len() || needle.len() > hay.len() - from {
        return None;
    }
    memchr::memmem::find(&hay[from..], needle).map(|p| p + from)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(hay.len());
    }
    if needle.len() > hay.len() {
        return None;
    }
    memchr::memmem::rfind(hay, needle)
}

/// strcmp's result in PHP: the difference of the first differing bytes, else
/// -1/0/1 by length.
fn byte_diff(x: &[u8], y: &[u8]) -> i64 {
    match x.iter().zip(y).find(|(a, b)| a != b) {
        Some((a, b)) => *a as i64 - *b as i64,
        None => cmp_ord(x.len().cmp(&y.len())),
    }
}

fn cmp_ord(o: std::cmp::Ordering) -> i64 {
    match o {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

fn strpos(a: &[Val], name: &str, ci: bool, reverse: bool) -> Result<Val, Throw> {
    let h = args::string(a, 0, name)?;
    let n = args::string(a, 1, name)?;
    let off = args::int_opt(a, 2, name, 0)?;
    let (hay, needle) = if ci { (h.as_bytes().to_ascii_lowercase(), n.as_bytes().to_ascii_lowercase()) } else { (h.as_bytes().to_vec(), n.as_bytes().to_vec()) };
    let len = hay.len() as i64;
    let start = if off < 0 { (len + off).max(0) } else { off.min(len) } as usize;
    let r = if reverse {
        if off < 0 {
            let end = (len + off + needle.len() as i64).clamp(0, len) as usize;
            rfind(&hay[..end], &needle)
        } else {
            rfind(&hay[start..], &needle).map(|p| p + start)
        }
    } else {
        find(&hay, &needle, start)
    };
    Ok(match r {
        Some(p) => Val::Int(p as i64),
        None => Val::Bool(false),
    })
}

pub(crate) fn substr_range(len: usize, start: i64, length: Option<i64>) -> (usize, usize) {
    let len_i = len as i64;
    let from = if start < 0 { (len_i + start).max(0) } else { start.min(len_i) };
    let to = match length {
        None => len_i,
        Some(l) if l < 0 => (len_i + l).max(from),
        Some(l) => (from + l).min(len_i),
    };
    (from as usize, to.max(from) as usize)
}

pub(crate) fn substr_bytes(b: &[u8], start: i64, length: Option<i64>) -> &[u8] {
    let (f, t) = substr_range(b.len(), start, length);
    &b[f..t]
}

fn trim(a: &[Val], left: bool, right: bool) -> Result<Val, Throw> {
    let s = args::string(a, 0, "trim")?;
    let chars: Vec<u8> = match a.get(1) {
        Some(_) => expand_char_ranges(args::string(a, 1, "trim")?.as_bytes()),
        None => b" \t\n\r\0\x0b".to_vec(),
    };
    let b = s.as_bytes();
    let mut from = 0;
    let mut to = b.len();
    if left {
        while from < to && chars.contains(&b[from]) {
            from += 1;
        }
    }
    if right {
        while to > from && chars.contains(&b[to - 1]) {
            to -= 1;
        }
    }
    Ok(Val::bytes(&b[from..to]))
}

fn expand_char_ranges(spec: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < spec.len() {
        if i + 3 < spec.len() + 1 && i + 2 < spec.len() && spec[i + 1] == b'.' && spec[i + 2] == b'.' && i + 3 < spec.len() {
            for c in spec[i]..=spec[i + 3] {
                out.push(c);
            }
            i += 4;
        } else {
            out.push(spec[i]);
            i += 1;
        }
    }
    out
}

fn replace_all(hay: &[u8], needle: &[u8], rep: &[u8], ci: bool, count: &mut i64) -> Vec<u8> {
    if needle.is_empty() {
        return hay.to_vec();
    }
    let (h, n) = if ci { (hay.to_ascii_lowercase(), needle.to_ascii_lowercase()) } else { (hay.to_vec(), needle.to_vec()) };
    let mut out = Vec::with_capacity(hay.len());
    let mut i = 0;
    // one searcher for all occurrences (PERF3)
    for p in memchr::memmem::find_iter(&h, &n) {
        out.extend_from_slice(&hay[i..p]);
        out.extend_from_slice(rep);
        i = p + n.len();
        *count += 1;
    }
    out.extend_from_slice(&hay[i..]);
    out
}

fn str_replace(vm: &mut Vm, a: &[Val], ci: bool) -> Result<Val, Throw> {
    args::count(a, 3, "str_replace")?;
    let mut count = 0i64;
    let subject = a[2].clone();
    let apply = |vm: &mut Vm, s: &[u8], count: &mut i64| -> Result<Vec<u8>, Throw> {
        let mut cur = s.to_vec();
        match (&a[0], &a[1]) {
            (Val::Array(search), Val::Array(rep)) => {
                let reps: Vec<Val> = rep.values().cloned().collect();
                for (i, n) in search.values().enumerate() {
                    let n = vm.to_string_val(n)?;
                    let r = match reps.get(i) { Some(r) => vm.to_string_val(r)?, None => PhpString::new() };
                    cur = replace_all(&cur, n.as_bytes(), r.as_bytes(), ci, count);
                }
            }
            (Val::Array(search), rep) => {
                let r = vm.to_string_val(rep)?;
                for n in search.values() {
                    let n = vm.to_string_val(n)?;
                    cur = replace_all(&cur, n.as_bytes(), r.as_bytes(), ci, count);
                }
            }
            (search, rep) => {
                let n = vm.to_string_val(search)?;
                let r = vm.to_string_val(rep)?;
                cur = replace_all(&cur, n.as_bytes(), r.as_bytes(), ci, count);
            }
        }
        Ok(cur)
    };
    match subject {
        Val::Array(arr) => {
            let mut out = PhpArray::new();
            for (k, v) in arr.iter() {
                let s = vm.to_string_val(v)?;
                out.insert(k.clone(), Val::bytes(&apply(vm, s.as_bytes(), &mut count)?));
            }
            Ok(Val::array(out))
        }
        other => {
            let s = vm.to_string_val(&other)?;
            Ok(Val::bytes(&apply(vm, s.as_bytes(), &mut count)?))
        }
    }
}

fn strtr(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let s = args::string(a, 0, "strtr")?;
    if a.len() == 2 {
        let Val::Array(map) = &a[1] else { return Err(Throw::type_error("strtr(): Argument #2 ($from) must be of type array, string given")) };
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (k, v) in map.iter() {
            let key = match k { ArrayKey::Int(i) => i.to_string().into_bytes(), ArrayKey::Str(s) => s.as_bytes().to_vec() };
            if key.is_empty() {
                continue;
            }
            pairs.push((key, vm.to_string_val(v)?.as_bytes().to_vec()));
        }
        pairs.sort_by(|x, y| y.0.len().cmp(&x.0.len()));
        let b = s.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        'outer: while i < b.len() {
            for (k, v) in &pairs {
                if b[i..].starts_with(k) {
                    out.extend_from_slice(v);
                    i += k.len();
                    continue 'outer;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        return Ok(Val::bytes(&out));
    }
    let from = args::string(a, 1, "strtr")?;
    let to = args::string(a, 2, "strtr")?;
    let n = from.len().min(to.len());
    let out: Vec<u8> = s.as_bytes().iter().map(|b| match from.as_bytes()[..n].iter().position(|c| c == b) { Some(p) => to.as_bytes()[p], None => *b }).collect();
    Ok(Val::bytes(&out))
}

fn implode(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let (sep, arr) = match (a.first(), a.get(1)) {
        (Some(Val::Array(arr)), None) => (PhpString::new(), arr.clone()),
        (Some(Val::Array(arr)), Some(Val::Str(s))) => (s.clone(), arr.clone()),
        (Some(Val::Str(s)), Some(Val::Array(arr))) => (s.clone(), arr.clone()),
        (Some(Val::Array(_)), Some(o)) | (Some(o), Some(Val::Array(_))) => return Err(Throw::type_error(format!("implode(): Argument #1 ($separator) must be of type string, {} given", o.type_name()))),
        (_, Some(o)) => return Err(Throw::type_error(format!("implode(): Argument #2 ($array) must be of type ?array, {} given", o.type_name()))),
        (o, None) => return Err(Throw::type_error(format!("implode(): Argument #1 ($array) must be of type array, {} given", o.map_or("null", |v| v.type_name())))),
    };
    let mut out = Vec::new();
    for (i, v) in arr.values().enumerate() {
        if i > 0 {
            out.extend_from_slice(sep.as_bytes());
        }
        out.extend_from_slice(vm.to_string_val(v)?.as_bytes());
    }
    Ok(Val::bytes(&out))
}

fn natural_cmp(a: &[u8], b: &[u8], ci: bool) -> std::cmp::Ordering {
    let (a, b) = if ci { (a.to_ascii_lowercase(), b.to_ascii_lowercase()) } else { (a.to_vec(), b.to_vec()) };
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let si = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let na: u128 = std::str::from_utf8(&a[si..i]).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
            let nb: u128 = std::str::from_utf8(&b[sj..j]).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
            if na != nb {
                return na.cmp(&nb);
            }
        } else {
            if a[i] != b[j] {
                return a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn html_escape(s: &str, flags: i64) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if flags & 2 != 0 => out.push_str("&quot;"),
            '\'' if flags & 1 != 0 => out.push_str("&#039;"),
            _ => out.push(c),
        }
    }
    out
}

/// PHP `sprintf`.
pub(crate) fn sprintf(vm: &mut Vm, a: &[Val], name: &str) -> Result<PhpString, Throw> {
    let fmt = args::string(a, 0, name)?;
    let f = fmt.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut argi = 1usize;
    let mut i = 0;
    while i < f.len() {
        if f[i] != b'%' {
            out.push(f[i]);
            i += 1;
            continue;
        }
        i += 1;
        if i >= f.len() {
            break;
        }
        if f[i] == b'%' {
            out.push(b'%');
            i += 1;
            continue;
        }
        // argnum$
        let mut argnum: Option<usize> = None;
        let save = i;
        let mut num = 0usize;
        let mut has = false;
        while i < f.len() && f[i].is_ascii_digit() {
            num = num * 10 + (f[i] - b'0') as usize;
            has = true;
            i += 1;
        }
        if has && i < f.len() && f[i] == b'$' {
            argnum = Some(num);
            i += 1;
        } else {
            i = save;
        }
        let mut left = false;
        let mut plus = false;
        let mut pad = b' ';
        loop {
            if i >= f.len() {
                break;
            }
            match f[i] {
                b'-' => left = true,
                b'+' => plus = true,
                b'0' => pad = b'0',
                b' ' => pad = b' ',
                b'\'' => {
                    i += 1;
                    pad = f.get(i).copied().unwrap_or(b' ');
                }
                _ => break,
            }
            i += 1;
        }
        let mut width = 0usize;
        while i < f.len() && f[i].is_ascii_digit() {
            width = width * 10 + (f[i] - b'0') as usize;
            i += 1;
        }
        let mut precision: Option<usize> = None;
        if i < f.len() && f[i] == b'.' {
            i += 1;
            let mut p = 0;
            while i < f.len() && f[i].is_ascii_digit() {
                p = p * 10 + (f[i] - b'0') as usize;
                i += 1;
            }
            precision = Some(p);
        }
        if i >= f.len() {
            break;
        }
        let spec = f[i];
        i += 1;
        let arg = match argnum {
            Some(n) => a.get(n).cloned(),
            None => {
                let v = a.get(argi).cloned();
                argi += 1;
                v
            }
        };
        let arg = arg.ok_or_else(|| Throw::argument_count(format!("{name}(): Too few arguments")))?;
        let body: Vec<u8> = match spec {
            b'd' | b'i' => {
                let n = arg.to_int()?;
                let mut s = n.abs().to_string();
                if n < 0 {
                    s.insert(0, '-');
                } else if plus {
                    s.insert(0, '+');
                }
                s.into_bytes()
            }
            b'u' => (arg.to_int()? as u64).to_string().into_bytes(),
            b'f' | b'F' => {
                let x = arg.to_float()?;
                let mut s = format!("{:.*}", precision.unwrap_or(6), x);
                if plus && x >= 0.0 {
                    s.insert(0, '+');
                }
                s.into_bytes()
            }
            b'e' | b'E' => {
                let x = arg.to_float()?;
                let p = precision.unwrap_or(6);
                let s = format!("{:.*e}", p, x);
                let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
                let ev: i32 = e.parse().unwrap_or(0);
                let r = format!("{}{}{}{}", m, if spec == b'e' { "e" } else { "E" }, if ev < 0 { "-" } else { "+" }, ev.abs());
                r.into_bytes()
            }
            b'g' | b'G' => format_float(arg.to_float()?).into_bytes(),
            b's' => {
                let s = vm.to_string_val(&arg)?;
                match precision {
                    Some(p) => s.as_bytes()[..p.min(s.len())].to_vec(),
                    None => s.as_bytes().to_vec(),
                }
            }
            b'x' => format!("{:x}", arg.to_int()?).into_bytes(),
            b'X' => format!("{:X}", arg.to_int()?).into_bytes(),
            b'o' => format!("{:o}", arg.to_int()?).into_bytes(),
            b'b' => format!("{:b}", arg.to_int()?).into_bytes(),
            b'c' => vec![arg.to_int()? as u8],
            other => return Err(Throw::value_error(format!("{name}(): Unknown format specifier \"{}\"", other as char))),
        };
        if body.len() < width {
            let fill = width - body.len();
            if left {
                out.extend_from_slice(&body);
                out.extend(std::iter::repeat_n(b' ', fill));
            } else if pad == b'0' && matches!(spec, b'd' | b'i' | b'f' | b'F' | b'e' | b'E') && (body.first() == Some(&b'-') || body.first() == Some(&b'+')) {
                out.push(body[0]);
                out.extend(std::iter::repeat_n(b'0', fill));
                out.extend_from_slice(&body[1..]);
            } else {
                out.extend(std::iter::repeat_n(pad, fill));
                out.extend_from_slice(&body);
            }
        } else {
            out.extend_from_slice(&body);
        }
    }
    Ok(PhpString::from(out))
}

fn similar_text(a: &[u8], b: &[u8]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let (mut best, mut pa, mut pb) = (0, 0, 0);
    for i in 0..a.len() {
        for j in 0..b.len() {
            let mut k = 0;
            while i + k < a.len() && j + k < b.len() && a[i + k] == b[j + k] {
                k += 1;
            }
            if k > best {
                best = k;
                pa = i;
                pb = j;
            }
        }
    }
    if best == 0 {
        return 0;
    }
    best + similar_text(&a[..pa], &b[..pb]) + similar_text(&a[pa + best..], &b[pb + best..])
}

fn levenshtein(a: &[u8], b: &[u8]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

pub(crate) fn base64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk.len();
        let v = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(v >> 18) as usize & 63] as char);
        out.push(T[(v >> 12) as usize & 63] as char);
        out.push(if n > 1 { T[(v >> 6) as usize & 63] as char } else { '=' });
        out.push(if n > 2 { T[v as usize & 63] as char } else { '=' });
    }
    out
}

pub(crate) fn base64_decode(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for &c in data {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub(crate) fn crc32(data: &[u8]) -> u32 {
    crc32fast::hash(data)
}

/// Argument #3 `$length` of strncmp/strncasecmp: negative is a ValueError.
fn non_negative_length(a: &[Val], name: &str) -> Result<usize, Throw> {
    let n = args::int(a, 2, name)?;
    if n < 0 {
        return Err(Throw::value_error(format!("{name}(): Argument #3 ($length) must be greater than or equal to 0")));
    }
    Ok(n as usize)
}

/// PHP's wordwrap on bytes: an existing break starts a new line, a space at or
/// past the width becomes a break, a longer word breaks at its last space, and
/// with `cut` a word longer than the width is split. One-byte breaks without
/// `cut` replace spaces in place (PHP's fast path, slightly different rules).
fn wordwrap(text: &[u8], width: i64, brk: &[u8], cut: bool) -> Vec<u8> {
    let len = text.len() as i64;
    let (mut laststart, mut lastspace) = (0i64, 0i64);
    if brk.len() == 1 && !cut {
        let mut out = text.to_vec();
        for current in 0..len {
            let c = text[current as usize];
            if c == brk[0] {
                laststart = current + 1;
                lastspace = current + 1;
            } else if c == b' ' {
                if current - laststart >= width {
                    out[current as usize] = brk[0];
                    laststart = current + 1;
                }
                lastspace = current;
            } else if current - laststart >= width && laststart != lastspace {
                out[lastspace as usize] = brk[0];
                laststart = lastspace + 1;
            }
        }
        return out;
    }
    let mut out = Vec::with_capacity(text.len() + brk.len() * 4);
    let slice = |from: i64, to: i64| &text[from as usize..to as usize];
    let mut current = 0i64;
    while current < len {
        let c = text[current as usize];
        if c == brk[0] && current + (brk.len() as i64) < len && text[current as usize..].starts_with(brk) {
            out.extend_from_slice(slice(laststart, current + brk.len() as i64));
            current += brk.len() as i64 - 1;
            laststart = current + 1;
            lastspace = current + 1;
        } else if c == b' ' {
            if current - laststart >= width {
                out.extend_from_slice(slice(laststart, current));
                out.extend_from_slice(brk);
                laststart = current + 1;
            }
            lastspace = current;
        } else if current - laststart >= width && cut && laststart >= lastspace {
            out.extend_from_slice(slice(laststart, current));
            out.extend_from_slice(brk);
            laststart = current;
            lastspace = current;
        } else if current - laststart >= width && laststart < lastspace {
            out.extend_from_slice(slice(laststart, lastspace));
            out.extend_from_slice(brk);
            laststart = lastspace + 1;
            lastspace = laststart;
        }
        current += 1;
    }
    if laststart != current {
        out.extend_from_slice(slice(laststart, current));
    }
    out
}

/// MB_CASE_TITLE as PHP: a cased letter is title-cased when no cased letter
/// precedes it in the word; case-ignorable characters (apostrophes, periods,
/// colons, combining marks) keep the word going, anything else (digits, `_`,
/// `-`, spaces) ends it.
fn title_case(s: &str) -> String {
    let case_ignorable = |c: char| {
        matches!(c, '\'' | '.' | ':' | '\u{00B7}' | '\u{00AD}' | '\u{2018}' | '\u{2019}' | '\u{2024}' | '\u{FE52}' | '\u{FF07}' | '\u{FF0E}' | '\u{FF1A}' | '^' | '`' | '\u{00A8}' | '\u{00AF}' | '\u{00B4}' | '\u{00B8}')
            || ('\u{0300}'..='\u{036F}').contains(&c)
    };
    let mut out = String::with_capacity(s.len());
    let mut in_word = false;
    for c in s.chars() {
        let cased = c.is_uppercase() || c.is_lowercase();
        if cased {
            if in_word {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            in_word = true;
        } else {
            if !case_ignorable(c) {
                in_word = false;
            }
            out.push(c);
        }
    }
    out
}

/// PHP's basename on bytes: trailing slashes ignored, the part after the last '/'.
fn basename_bytes(p: &[u8]) -> &[u8] {
    let mut end = p.len();
    while end > 0 && p[end - 1] == b'/' {
        end -= 1;
    }
    let start = p[..end].iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    &p[start..end]
}

/// PHP's dirname for one level: "/a/b/" -> "/a", "a" -> ".", "/a" -> "/", "/" -> "/".
fn dirname_bytes(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return Vec::new();
    }
    let mut end = p.len();
    while end > 0 && p[end - 1] == b'/' {
        end -= 1;
    }
    if end == 0 {
        return b"/".to_vec();
    }
    match p[..end].iter().rposition(|&c| c == b'/') {
        None => b".".to_vec(),
        Some(i) => {
            let mut e = i;
            while e > 0 && p[e - 1] == b'/' {
                e -= 1;
            }
            if e == 0 { b"/".to_vec() } else { p[..e].to_vec() }
        }
    }
}

/// Display width as mbstring counts it: East Asian wide and fullwidth characters are 2.
fn char_width(c: char) -> i64 {
    let u = c as u32;
    let wide = matches!(u, 0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

/// Title case of one character (mb_ucfirst uses MB_CASE_TITLE for the first letter).
fn title_case_char(c: char) -> String {
    match c {
        'ǆ' | 'ǅ' | 'Ǆ' => "ǅ".into(),
        'ǉ' | 'ǈ' | 'Ǉ' => "ǈ".into(),
        'ǌ' | 'ǋ' | 'Ǌ' => "ǋ".into(),
        'ǳ' | 'ǲ' | 'Ǳ' => "ǲ".into(),
        _ => c.to_uppercase().collect(),
    }
}

/// sscanf subset: %d %i %u %x %X %o %f %e %g %s %c %[set] %n %%, widths and %*
/// (suppress). Literal characters must match, whitespace matches any run.
/// Unfilled conversions are null; null when the input ends before the first one.
fn sscanf(s: &[u8], f: &[u8]) -> Val {
    let mut out = PhpArray::new();
    let (mut si, mut fi) = (0usize, 0usize);
    let mut failed = false;
    let mut converted_any = false;
    while fi < f.len() {
        let c = f[fi];
        if c.is_ascii_whitespace() {
            while si < s.len() && s[si].is_ascii_whitespace() {
                si += 1;
            }
            fi += 1;
            continue;
        }
        if c != b'%' {
            if !failed && si < s.len() && s[si] == c {
                si += 1;
            } else {
                failed = true;
            }
            fi += 1;
            continue;
        }
        fi += 1;
        if fi < f.len() && f[fi] == b'%' {
            if !failed && si < s.len() && s[si] == b'%' { si += 1 } else { failed = true }
            fi += 1;
            continue;
        }
        let suppress = fi < f.len() && f[fi] == b'*';
        if suppress {
            fi += 1;
        }
        let mut width = 0usize;
        while fi < f.len() && f[fi].is_ascii_digit() {
            width = width * 10 + (f[fi] - b'0') as usize;
            fi += 1;
        }
        while fi < f.len() && matches!(f[fi], b'l' | b'h' | b'L') {
            fi += 1;
        }
        if fi >= f.len() {
            break;
        }
        let conv = f[fi];
        fi += 1;
        let mut set: Option<(bool, Vec<u8>)> = None;
        if conv == b'[' {
            let negate = fi < f.len() && f[fi] == b'^';
            if negate {
                fi += 1;
            }
            let mut chars = Vec::new();
            if fi < f.len() && f[fi] == b']' {
                chars.push(b']');
                fi += 1;
            }
            while fi < f.len() && f[fi] != b']' {
                if fi + 2 < f.len() && f[fi + 1] == b'-' && f[fi + 2] != b']' {
                    chars.extend(f[fi]..=f[fi + 2]);
                    fi += 3;
                } else {
                    chars.push(f[fi]);
                    fi += 1;
                }
            }
            fi += 1;
            set = Some((negate, chars));
        }
        if conv == b'n' {
            if !suppress {
                out.push(if failed { Val::Null } else { Val::Int(si as i64) });
            }
            continue;
        }
        if !matches!(conv, b'c' | b'[') {
            while !failed && si < s.len() && s[si].is_ascii_whitespace() {
                si += 1;
            }
        }
        if failed || si >= s.len() {
            if !failed && !converted_any && si >= s.len() {
                // input ended before the first conversion
                return Val::Null;
            }
            failed = true;
            if !suppress {
                out.push(Val::Null);
            }
            continue;
        }
        let max = if width == 0 { usize::MAX } else { width };
        let start = si;
        let take = |pred: &dyn Fn(u8, usize) -> bool, si: &mut usize| {
            let mut n = 0;
            while *si < s.len() && n < max && pred(s[*si], n) {
                *si += 1;
                n += 1;
            }
        };
        let v = match conv {
            b'd' | b'i' | b'u' => {
                take(&|b, n| b.is_ascii_digit() || (n == 0 && (b == b'-' || b == b'+')), &mut si);
                std::str::from_utf8(&s[start..si]).ok().and_then(|t| t.parse::<i64>().ok()).map(Val::Int)
            }
            b'x' | b'X' => {
                take(&|b, _| b.is_ascii_hexdigit(), &mut si);
                i64::from_str_radix(std::str::from_utf8(&s[start..si]).unwrap_or(""), 16).ok().map(Val::Int)
            }
            b'o' => {
                take(&|b, _| (b'0'..=b'7').contains(&b), &mut si);
                i64::from_str_radix(std::str::from_utf8(&s[start..si]).unwrap_or(""), 8).ok().map(Val::Int)
            }
            b'f' | b'e' | b'E' | b'g' => {
                take(&|b, n| b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'E' || (b == b'-' || b == b'+') && (n == 0 || matches!(s[start + n - 1], b'e' | b'E')), &mut si);
                std::str::from_utf8(&s[start..si]).ok().and_then(|t| t.parse::<f64>().ok()).map(Val::Float)
            }
            b's' => {
                take(&|b, _| !b.is_ascii_whitespace(), &mut si);
                Some(Val::bytes(&s[start..si]))
            }
            b'c' => {
                let n = if width == 0 { 1 } else { width };
                si = (si + n).min(s.len());
                Some(Val::bytes(&s[start..si]))
            }
            b'[' => {
                let (negate, chars) = set.clone().unwrap_or_default();
                take(&|b, _| chars.contains(&b) != negate, &mut si);
                if si == start { None } else { Some(Val::bytes(&s[start..si])) }
            }
            _ => None,
        };
        match v {
            Some(v) => {
                converted_any = true;
                if !suppress {
                    out.push(v);
                }
            }
            None => {
                failed = true;
                if !suppress {
                    out.push(Val::Null);
                }
            }
        }
    }
    Val::array(out)
}

