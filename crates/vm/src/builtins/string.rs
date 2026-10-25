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
        let delims = match a.get(1) { Some(d) => d.to_php_string()?.as_bytes().to_vec(), None => b" \t\r\n\x0c\x0b".to_vec() };
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
        let pad = match a.get(2) { Some(v) => v.to_php_string()?.as_bytes().to_vec(), None => b" ".to_vec() };
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
        let brk = match a.get(2) { Some(v) => v.to_php_string()?.as_bytes().to_vec(), None => b"\n".to_vec() };
        let cut = args::boolean(a, 3, false);
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
    vm.register_native("md5", |_, a| Ok(Val::from(crate::builtins::string::md5_hex(args::string(a, 0, "md5")?.as_bytes()))));
    vm.register_native("sha1", |_, a| Ok(Val::from(sha1_hex(args::string(a, 0, "sha1")?.as_bytes()))));
    vm.register_native("crc32", |_, a| Ok(Val::Int(crc32(args::string(a, 0, "crc32")?.as_bytes()) as i64)));
    vm.register_native("hash", |_, a| {
        let algo = args::string(a, 0, "hash")?.to_string_lossy().to_lowercase();
        let data = args::string(a, 1, "hash")?;
        Ok(Val::from(match algo.as_str() {
            "md5" => md5_hex(data.as_bytes()),
            "sha1" => sha1_hex(data.as_bytes()),
            "sha256" => sha256_hex(data.as_bytes()),
            "crc32b" => format!("{:08x}", crc32(data.as_bytes())),
            other => return Err(Throw::value_error(format!("hash(): Argument #1 ($algo) must be a valid hashing algorithm, \"{other}\" is not supported"))),
        }))
    });
    vm.register_native("hash_algos", |_, _| Ok(Val::array(PhpArray::from_values(["md5", "sha1", "sha256", "crc32b"].map(Val::str)))));
    vm.register_native("uniqid", |_, a| {
        let prefix = match a.first() { Some(v) => v.to_php_string()?.to_string(), None => String::new() };
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
        Ok(Val::from(match args::int(a, 1, "mb_convert_case")? {
            0 => s.to_uppercase(),
            1 => s.to_lowercase(),
            _ => s.split(' ').map(|w| { let mut c = w.chars(); match c.next() { Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(), None => String::new() } }).collect::<Vec<_>>().join(" "),
        }))
    });
    vm.register_native("mb_strwidth", |_, a| Ok(Val::Int(args::string(a, 0, "mb_strwidth")?.char_len() as i64)));
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
        let before = args::boolean(a, 2, false);
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
        let end = match a.get(2) { Some(v) => v.to_php_string()?.as_bytes().to_vec(), None => b"\r\n".to_vec() };
        let mut out = Vec::new();
        for c in s.as_bytes().chunks(n) {
            out.extend_from_slice(c);
            out.extend_from_slice(&end);
        }
        Ok(Val::bytes(&out))
    });
    vm.register_native("money_placeholder", |_, _| Ok(Val::Null));
}

pub(crate) fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= hay.len()).then_some(from);
    }
    if from >= hay.len() || needle.len() > hay.len() - from {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(hay.len());
    }
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).rposition(|w| w == needle)
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
        Some(v) => expand_char_ranges(v.to_php_string()?.as_bytes()),
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
    while let Some(p) = find(&h, &n, i) {
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
        (Some(Val::Array(arr)), Some(s)) => (vm.to_string_val(s)?, arr.clone()),
        (Some(s), Some(Val::Array(arr))) => (vm.to_string_val(s)?, arr.clone()),
        _ => return Err(Throw::type_error("implode(): Argument #2 ($array) must be of type ?array")),
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
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

pub(crate) fn md5_hex(data: &[u8]) -> String {
    let s: [u32; 64] = [7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f2 = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f2.rotate_left(s[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = String::new();
    for v in [a0, b0, c0, d0] {
        for byte in v.to_le_bytes() {
            out.push_str(&format!("{byte:02x}"));
        }
    }
    out
}

pub(crate) fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i / 20 {
                0 => ((b & c) | (!b & d), 0x5A827999),
                1 => (b ^ c ^ d, 0x6ED9EBA1),
                2 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(v[i]);
        }
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
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

