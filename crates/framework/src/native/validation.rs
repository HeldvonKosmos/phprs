//! Validation with Laravel-style rules (ROADMAP §3 "Validation"): own rule
//! parser, `Request::validate([...])`, `Validator::validate($data, $rules)`.
//! Failures throw `OpenGrid\Validation\ValidationException` (422).

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, PhpArray, Val};
use grd_vm::vm::{NativeClassDef, Vm};

pub const EXCEPTION: &str = "OpenGrid\\Validation\\ValidationException";
pub const CLASS: &str = "OpenGrid\\Validation\\Validator";

pub struct Errors(pub Vec<(String, Vec<String>)>);

fn rules_of(v: &Val) -> Result<Vec<String>, Throw> {
    match v {
        Val::Str(s) => Ok(s.to_string_lossy().split('|').map(str::trim).filter(|r| !r.is_empty()).map(str::to_string).collect()),
        Val::Array(a) => a.values().map(|x| Ok(x.to_php_string()?.to_string_lossy())).collect(),
        other => Err(Throw::type_error(format!("validation rules must be string or array, {} given", other.type_name()))),
    }
}

/// Laravel's displayable attribute: `_` becomes a space, dots of nested
/// and wildcard paths stay (`items.1.name`).
fn human(field: &str) -> String {
    field.replace('_', " ")
}

/// The value at a dot path (`user.name`, `items.0.name`) in nested arrays.
fn lookup<'a>(data: &'a PhpArray, path: &str) -> Option<&'a Val> {
    let mut parts = path.split('.');
    let mut cur = data.get(&ArrayKey::from_str(parts.next()?))?;
    for p in parts {
        match cur {
            Val::Array(a) => cur = a.get(&ArrayKey::from_str(p))?,
            _ => return None,
        }
    }
    Some(cur)
}

/// Laravel's wildcard expansion: every `*` stands for each key of the array
/// at that point of the data (`items.*.name` → `items.0.name`, …); nothing
/// when the array is missing.
fn expand(data: &PhpArray, field: &str) -> Vec<String> {
    let Some(star) = field.find('*') else { return vec![field.to_string()] };
    let head = field[..star].trim_end_matches('.');
    let tail = &field[star + 1..];
    let parent = if head.is_empty() { None } else { lookup(data, head) };
    let Some(Val::Array(arr)) = parent else { return Vec::new() };
    arr.iter()
        .flat_map(|(k, _)| {
            let key = match k {
                ArrayKey::Int(i) => i.to_string(),
                ArrayKey::Str(s) => s.to_string_lossy(),
            };
            expand(data, &format!("{head}.{key}{tail}"))
        })
        .collect()
}

/// Put `value` at a dot path, creating the arrays on the way.
fn set_path(out: &mut PhpArray, path: &str, value: Val) {
    match path.split_once('.') {
        None => out.insert(ArrayKey::from_str(path), value),
        Some((first, rest)) => {
            let slot = out.entry_mut(ArrayKey::from_str(first));
            if !matches!(slot, Val::Array(_)) {
                *slot = Val::empty_array();
            }
            if let Some(inner) = slot.array_mut() {
                set_path(inner, rest, value);
            }
        }
    }
}

/// Laravel's `validateRequired` on another field: there and not empty.
fn filled(data: &PhpArray, field: &str) -> bool {
    lookup(data, field).is_some_and(|v| !is_empty(v))
}

/// How Laravel compares another field's value with the listed values of
/// `required_if`/`required_unless` (booleans as `true`/`false`, null as `null`).
fn shown(v: Option<&Val>) -> String {
    match v {
        None | Some(Val::Null) => "null".into(),
        Some(Val::Bool(b)) => if *b { "true".into() } else { "false".into() },
        Some(v) => str_of(v),
    }
}

/// The message of an active conditional `required_*` rule, or None when it
/// does not apply to `data`.
fn conditional_required(data: &PhpArray, name: &str, rule: &str, arg: Option<&str>) -> Option<String> {
    let args: Vec<&str> = arg.map(|a| a.split(',').map(str::trim).collect()).unwrap_or_default();
    let others = || args.iter().map(|a| human(a)).collect::<Vec<_>>().join(" / ");
    match rule {
        "required_if" => {
            let (other, values) = args.split_first()?;
            let v = shown(lookup(data, other));
            values.contains(&v.as_str()).then(|| format!("The {name} field is required when {} is {v}.", human(other)))
        }
        "required_unless" => {
            let (other, values) = args.split_first()?;
            let v = shown(lookup(data, other));
            (!values.contains(&v.as_str())).then(|| format!("The {name} field is required unless {} is in {}.", human(other), values.join(", ")))
        }
        "required_with" => args.iter().any(|a| filled(data, a)).then(|| format!("The {name} field is required when {} is present.", others())),
        "required_with_all" => args.iter().all(|a| filled(data, a)).then(|| format!("The {name} field is required when {} are present.", others())),
        "required_without" => args.iter().any(|a| !filled(data, a)).then(|| format!("The {name} field is required when {} is not present.", others())),
        "required_without_all" => args.iter().all(|a| !filled(data, a)).then(|| format!("The {name} field is required when none of {} are present.", others())),
        _ => None,
    }
}

fn is_empty(v: &Val) -> bool {
    match v {
        Val::Null => true,
        Val::Str(s) => s.as_bytes().iter().all(|b| b.is_ascii_whitespace()),
        Val::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// Laravel's `getSize`: with a numeric rule (`numeric`, `integer`,
/// `decimal`) a numeric value is its own size; an array its count; anything
/// else the length of its string form (`null` → 0).
fn size(v: &Val, numeric_rule: bool) -> Option<f64> {
    if numeric_rule && is_numeric(v) {
        return match v {
            Val::Int(i) => Some(*i as f64),
            Val::Float(f) => Some(*f),
            Val::Str(s) => s.to_string_lossy().trim().parse().ok(),
            _ => None,
        };
    }
    match v {
        Val::Null => Some(0.0),
        Val::Str(s) => Some(s.char_len() as f64),
        Val::Array(a) => Some(a.len() as f64),
        Val::Int(i) => Some(i.to_string().chars().count() as f64),
        Val::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Val::Float(_) => v.to_php_string().ok().map(|s| s.char_len() as f64),
        _ => None,
    }
}

fn is_numeric(v: &Val) -> bool {
    match v {
        Val::Int(_) | Val::Float(_) => true,
        Val::Str(s) => s.to_string_lossy().trim().parse::<f64>().is_ok(),
        _ => false,
    }
}

/// Laravel: `filter_var($value, FILTER_VALIDATE_INT) !== false` (true counts as 1).
fn is_integer(v: &Val) -> bool {
    match v {
        Val::Int(_) | Val::Bool(true) => true,
        Val::Float(f) => f.fract() == 0.0 && f.is_finite(),
        Val::Str(s) => s.to_string_lossy().trim().parse::<i64>().is_ok(),
        _ => false,
    }
}

/// Laravel's accepted values: true, false, 0, 1, "0", "1".
fn is_boolean(v: &Val) -> bool {
    match v {
        Val::Bool(_) => true,
        Val::Int(0 | 1) => true,
        Val::Str(s) => matches!(s.as_bytes(), b"0" | b"1"),
        _ => false,
    }
}

/// Laravel's `date` rule: strtotime understands it and date_parse finds a valid
/// day; besides ISO forms PHP reads d.m.Y, m/d/Y and Y/m/d.
fn is_date(s: &str) -> bool {
    if crate::native::database::value::parse_datetime(s).is_some() {
        return true;
    }
    let s = s.trim();
    let date = s.split_whitespace().next().unwrap_or("");
    ["%d.%m.%Y", "%m/%d/%Y", "%Y/%m/%d", "%d-%m-%Y"].iter().any(|f| chrono::NaiveDate::parse_from_str(date, f).is_ok())
}

fn str_of(v: &Val) -> String {
    match v {
        Val::Str(s) => s.to_string_lossy(),
        Val::Int(i) => i.to_string(),
        Val::Float(f) => f.to_string(),
        Val::Bool(b) => if *b { "1".into() } else { String::new() },
        _ => String::new(),
    }
}

fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Str(x), Val::Str(y)) => x.as_bytes() == y.as_bytes(),
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::Float(x), Val::Float(y)) => x == y,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Null, Val::Null) => true,
        _ => false,
    }
}

/// Counts rows for `exists:`/`unique:` (V1): `(table, column, value,
/// except)` → number of records with `column = value` (and an id other than
/// `except`). Without one, the database rules fail with an error.
pub type DbCount<'a> = &'a mut dyn FnMut(&str, &str, &Val, Option<&Val>) -> Result<u64, Throw>;

/// Checks one field against its rules; the failure messages.
fn check_field(data: &PhpArray, field: &str, rules: &[String], db: &mut Option<DbCount<'_>>) -> Result<Vec<String>, Throw> {
    let present = lookup(data, field).is_some();
    let value = lookup(data, field).cloned().unwrap_or(Val::Null);
    let nullable = rules.iter().any(|r| r == "nullable");
    let mut msgs = Vec::new();
    let name = human(field);
    if rules.iter().any(|r| r == "sometimes") && !present {
        return Ok(msgs);
    }
    // the `required_*` family: implicit once its condition holds (Laravel)
    if is_empty(&value) {
        for rule in rules {
            let (name_r, arg) = rule.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((rule.as_str(), None));
            if let Some(m) = conditional_required(data, &name, name_r, arg) {
                msgs.push(m);
                return Ok(msgs);
            }
        }
    }
    if !rules.iter().any(|r| r == "required") && is_empty(&value) {
        // optional and empty: only `present`/`filled` can complain
        if rules.iter().any(|r| r == "present") && !present {
            msgs.push(format!("The {name} field must be present."));
        }
        if rules.iter().any(|r| r == "filled") && present {
            msgs.push(format!("The {name} field must have a value."));
        }
        return Ok(msgs);
    }
    // Laravel's presentOrRuleIsImplicit: a missing field or a blank string is
    // checked only by implicit rules (`required`, `present`, `filled`)
    let blank = !present || matches!(&value, Val::Str(s) if s.as_bytes().iter().all(u8::is_ascii_whitespace));
    let numeric_rule = rules.iter().any(|r| matches!(r.split(':').next(), Some("numeric" | "integer" | "int" | "decimal")));
    for rule in rules {
        let (name_r, arg) = rule.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((rule.as_str(), None));
        if blank && !matches!(name_r, "required" | "present" | "filled") {
            continue;
        }
        let args: Vec<&str> = arg.map(|a| a.split(',').map(str::trim).collect()).unwrap_or_default();
        let fail = |m: String| m;
        let ok = match name_r {
            "required" => !is_empty(&value),
            "nullable" | "sometimes" | "present" | "filled" | "bail" => true,
            // checked above: with a value they hold
            n if n.starts_with("required_") => true,
            "string" => matches!(value, Val::Str(_)),
            "integer" | "int" => is_integer(&value),
            "numeric" => is_numeric(&value),
            "boolean" | "bool" => is_boolean(&value),
            "array" => matches!(value, Val::Array(_)),
            // Laravel's default `email` is RFC validation: no dot needed in the domain
            "email" => {
                let s = str_of(&value);
                let re = regex::Regex::new(r"^[^@\s]+@[^@\s.][^@\s]*$").expect("static regex");
                re.is_match(&s)
            }
            // any scheme with an authority (ftp://, ssh://, …), as Laravel's list of protocols
            "url" => {
                let s = str_of(&value);
                let re = regex::Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*://[^\s/?#]+[^\s]*$").expect("static regex");
                re.is_match(&s)
            }
            "uuid" => {
                let re = regex::Regex::new(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$").expect("static regex");
                re.is_match(&str_of(&value))
            }
            "date" => is_date(&str_of(&value)),
            "alpha" => !str_of(&value).is_empty() && str_of(&value).chars().all(|c| c.is_alphabetic()),
            "alpha_num" => !str_of(&value).is_empty() && str_of(&value).chars().all(|c| c.is_alphanumeric()),
            "alpha_dash" => !str_of(&value).is_empty() && str_of(&value).chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_'),
            "min" => args.first().and_then(|a| a.parse::<f64>().ok()).zip(size(&value, numeric_rule)).is_some_and(|(m, s)| s >= m),
            "max" => args.first().and_then(|a| a.parse::<f64>().ok()).zip(size(&value, numeric_rule)).is_some_and(|(m, s)| s <= m),
            "size" => args.first().and_then(|a| a.parse::<f64>().ok()).zip(size(&value, numeric_rule)).is_some_and(|(m, s)| s == m),
            "between" => {
                let lo = args.first().and_then(|a| a.parse::<f64>().ok());
                let hi = args.get(1).and_then(|a| a.parse::<f64>().ok());
                match (lo, hi, size(&value, numeric_rule)) {
                    (Some(lo), Some(hi), Some(s)) => s >= lo && s <= hi,
                    _ => false,
                }
            }
            "in" => args.iter().any(|a| *a == str_of(&value)),
            "not_in" => !args.iter().any(|a| *a == str_of(&value)),
            "regex" => {
                let pattern = arg.unwrap_or("");
                // `/…/flags` → body
                let body = pattern.strip_prefix('/').and_then(|p| p.rfind('/').map(|i| &p[..i])).unwrap_or(pattern);
                regex::Regex::new(body).map(|re| re.is_match(&str_of(&value))).unwrap_or(false)
            }
            "confirmed" => lookup(data, &format!("{field}_confirmation")).is_some_and(|c| same(c, &value)),
            "same" => args.first().and_then(|o| lookup(data, o)).is_some_and(|c| same(c, &value)),
            "different" => !args.first().and_then(|o| lookup(data, o)).is_some_and(|c| same(c, &value)),
            "accepted" => matches!(str_of(&value).as_str(), "1" | "true" | "yes" | "on") || matches!(value, Val::Bool(true)),
            // V1: `exists:table[,column]`, `unique:table[,column[,except]]`;
            // the column defaults to the field name (as in Laravel)
            "exists" | "unique" => {
                let table = args.first().copied().unwrap_or("");
                let column = args.get(1).copied().filter(|c| !c.is_empty()).unwrap_or(field);
                let except = args.get(2).copied().filter(|e| !e.is_empty() && !e.eq_ignore_ascii_case("null")).map(Val::str);
                let Some(count) = db.as_mut() else { return Err(Throw::error(format!("validation rule `{name_r}` needs the database"))) };
                let n = count(table, column, &value, except.as_ref())?;
                if name_r == "exists" { n > 0 } else { n == 0 }
            }
            _ => {
                msgs.push(fail(format!("Unknown validation rule `{name_r}` for {name}.")));
                true
            }
        };
        if !ok {
            let a0 = args.first().copied().unwrap_or("");
            let a1 = args.get(1).copied().unwrap_or("");
            // size rules name the unit like Laravel: numbers none, arrays items, everything else characters
            // Laravel picks the message by the rules, not the value (getAttributeType)
            let kind = if numeric_rule {
                0
            } else if rules.iter().any(|r| matches!(r.as_str(), "array" | "list")) {
                1
            } else {
                2
            };
            msgs.push(match name_r {
                "required" => format!("The {name} field is required."),
                "string" => format!("The {name} field must be a string."),
                "integer" | "int" => format!("The {name} field must be an integer."),
                "numeric" => format!("The {name} field must be a number."),
                "boolean" | "bool" => format!("The {name} field must be true or false."),
                "array" => format!("The {name} field must be an array."),
                "email" => format!("The {name} field must be a valid email address."),
                "url" => format!("The {name} field must be a valid URL."),
                "uuid" => format!("The {name} field must be a valid UUID."),
                "date" => format!("The {name} field must be a valid date."),
                "alpha" => format!("The {name} field must only contain letters."),
                "alpha_num" => format!("The {name} field must only contain letters and numbers."),
                "alpha_dash" => format!("The {name} field must only contain letters, numbers, dashes, and underscores."),
                "min" => match kind {
                    0 => format!("The {name} field must be at least {a0}."),
                    1 => format!("The {name} field must have at least {a0} items."),
                    _ => format!("The {name} field must be at least {a0} characters."),
                },
                "max" => match kind {
                    0 => format!("The {name} field must not be greater than {a0}."),
                    1 => format!("The {name} field must not have more than {a0} items."),
                    _ => format!("The {name} field must not be greater than {a0} characters."),
                },
                "size" => match kind {
                    0 => format!("The {name} field must be {a0}."),
                    1 => format!("The {name} field must contain {a0} items."),
                    _ => format!("The {name} field must be {a0} characters."),
                },
                "between" => match kind {
                    0 => format!("The {name} field must be between {a0} and {a1}."),
                    1 => format!("The {name} field must have between {a0} and {a1} items."),
                    _ => format!("The {name} field must be between {a0} and {a1} characters."),
                },
                "in" => format!("The selected {name} is invalid."),
                "not_in" => format!("The selected {name} is invalid."),
                "regex" => format!("The {name} field format is invalid."),
                "confirmed" => format!("The {name} field confirmation does not match."),
                "same" => format!("The {name} field must match {}.", human(a0)),
                "different" => format!("The {name} field and {} must be different.", human(a0)),
                "accepted" => format!("The {name} field must be accepted."),
                "unique" => format!("The {name} has already been taken."),
                "exists" => format!("The selected {name} is invalid."),
                other => format!("The {name} field failed the {other} rule."),
            });
            if nullable && value.is_null() {
                break;
            }
            // Laravel's shouldStopValidating: once an implicit rule failed,
            // the other rules of the field are not checked
            if matches!(name_r, "required" | "present" | "filled" | "accepted") || name_r.starts_with("required_") {
                break;
            }
            // `bail`: stop at the first failing rule of this field
            if rules.iter().any(|r| r == "bail") {
                break;
            }
        }
    }
    Ok(msgs)
}

/// Validate `data` against `rules` without database rules.
pub fn validate(data: &Val, rules: &Val) -> Result<Result<Val, Errors>, Throw> {
    validate_with(data, rules, None)
}

/// Validate `data` against `rules`; returns the validated subset or the errors.
pub fn validate_with(data: &Val, rules: &Val, mut db: Option<DbCount<'_>>) -> Result<Result<Val, Errors>, Throw> {
    let empty = PhpArray::new();
    let data = match data {
        Val::Array(a) => a.as_ref(),
        _ => &empty,
    };
    let Val::Array(rules) = rules else { return Err(Throw::type_error("validation rules must be an array")) };
    let mut errors = Vec::new();
    let mut validated = PhpArray::new();
    for (k, r) in rules.iter() {
        let field = match k {
            ArrayKey::Str(s) => s.to_string_lossy(),
            ArrayKey::Int(i) => i.to_string(),
        };
        let field_rules = rules_of(r)?;
        for path in expand(data, &field) {
            let msgs = check_field(data, &path, &field_rules, &mut db)?;
            if msgs.is_empty() {
                if let Some(v) = lookup(data, &path) {
                    set_path(&mut validated, &path, v.clone());
                }
            } else {
                errors.push((path, msgs));
            }
        }
    }
    Ok(if errors.is_empty() { Ok(Val::array(validated)) } else { Err(Errors(errors)) })
}

pub fn errors_array(errors: &Errors) -> Val {
    let mut out = PhpArray::new();
    for (field, msgs) in &errors.0 {
        out.insert(ArrayKey::from_str(field), Val::array(PhpArray::from_values(msgs.iter().map(|m| Val::str(m)))));
    }
    Val::array(out)
}

/// `exists:`/`unique:` against SurrealDB on this worker's connection (V1).
/// Table and column must be plain identifiers; they are put into the query,
/// the value and the excluded id are bound.
fn db_count(vm: &mut Vm, table: &str, column: &str, value: &Val, except: Option<&Val>) -> Result<u64, Throw> {
    let ident = |s: &str| !s.is_empty() && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ident(table) || !ident(column) {
        return Err(Throw::new("ValueError", format!("validation: invalid table or column `{table}`, `{column}`")));
    }
    let mut vars = std::collections::BTreeMap::new();
    vars.insert("v".to_string(), crate::native::database::value::to_surreal(vm, value)?);
    let mut cond = format!("`{column}` = $v");
    if let Some(e) = except {
        let rid = crate::native::database::record_id::from_val(vm, table, e)?;
        vars.insert("ex".to_string(), surrealdb::types::Value::RecordId(rid));
        cond.push_str(" AND id != $ex");
    }
    let r = match crate::native::database::query_one(vm, &format!("RETURN count((SELECT VALUE id FROM `{table}` WHERE {cond}))"), vars) {
        Ok(r) => r,
        // a table nobody defined yet holds no records
        Err(e) if e.message.contains("does not exist") => return Ok(0),
        Err(e) => return Err(e),
    };
    let n = crate::native::database::value::from_surreal(vm, r)?;
    Ok(n.to_int().unwrap_or(0).max(0) as u64)
}

/// Validate with the database rules on the worker's connection.
pub fn validate_vm(vm: &mut Vm, data: &Val, rules: &Val) -> Result<Result<Val, Errors>, Throw> {
    let mut count = |t: &str, c: &str, v: &Val, e: Option<&Val>| db_count(vm, t, c, v, e);
    validate_with(data, rules, Some(&mut count))
}

/// The exception message, as Laravel's `ValidationException::summarize`: the
/// first message, then "(and :count more error[s])" for all the others.
pub fn summarize(errors: &Errors) -> String {
    let mut all = errors.0.iter().flat_map(|(_, m)| m.iter());
    let Some(first) = all.next() else { return "The given data was invalid.".into() };
    match all.count() {
        0 => first.clone(),
        1 => format!("{first} (and 1 more error)"),
        n => format!("{first} (and {n} more errors)"),
    }
}

/// Validate or throw `ValidationException`.
pub fn validate_or_throw(vm: &mut Vm, data: &Val, rules: &Val) -> Result<Val, Throw> {
    match validate_vm(vm, data, rules)? {
        Ok(v) => Ok(v),
        Err(errors) => {
            let class = vm.lookup_class(EXCEPTION.as_bytes()).ok_or_else(|| Throw::error("ValidationException class missing"))?;
            let h = vm.instantiate(class, vec![Val::str(&summarize(&errors)), errors_array(&errors)])?;
            Err(Throw::existing(h))
        }
    }
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .static_method("validate", |vm, c| validate_or_throw(vm, &c.arg(0), &c.arg(1)))
        .static_method("errors", |vm, c| {
            Ok(match validate_vm(vm, &c.arg(0), &c.arg(1))? {
                Ok(_) => Val::empty_array(),
                Err(e) => errors_array(&e),
            })
        })
        .static_method("passes", |vm, c| Ok(Val::Bool(validate_vm(vm, &c.arg(0), &c.arg(1))?.is_ok())));
    vm.register_class(def)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arr(pairs: &[(&str, Val)]) -> Val {
        let mut a = PhpArray::new();
        for (k, v) in pairs {
            a.insert(ArrayKey::from_str(k), v.clone());
        }
        Val::array(a)
    }

    #[test]
    fn missing_field_reports_only_required() {
        // Laravel: non-implicit rules skip a missing field or a blank string
        let rules = arr(&[("title", Val::str("required|string|max:3")), ("note", Val::str("string|max:2"))]);
        let Err(errors) = validate(&arr(&[("note", Val::str("  "))]), &rules).unwrap() else { panic!("expected errors") };
        assert_eq!(errors.0, vec![("title".to_string(), vec!["The title field is required.".to_string()])]);
        // a present null: a failed implicit rule stops the field (Laravel's shouldStopValidating)
        let Err(errors) = validate(&arr(&[("title", Val::Null)]), &rules).unwrap() else { panic!("expected errors") };
        assert_eq!(errors.0[0].1, vec!["The title field is required.".to_string()]);
    }

    #[test]
    fn message_summarizes_the_other_errors() {
        // Laravel 13.38, ValidationException::summarize
        let rules = arr(&[("a", Val::str("required")), ("b", Val::str("required")), ("c", Val::str("integer|min:5"))]);
        let summary = |data: &Val| match validate(data, &rules).unwrap() {
            Err(e) => summarize(&e),
            Ok(_) => panic!("expected errors"),
        };
        assert_eq!(summary(&arr(&[("b", Val::Int(1))])), "The a field is required.");
        assert_eq!(summary(&arr(&[])), "The a field is required. (and 1 more error)");
        assert_eq!(summary(&arr(&[("c", Val::Int(1))])), "The a field is required. (and 2 more errors)");
        assert_eq!(summarize(&Errors(Vec::new())), "The given data was invalid.");
    }

    #[test]
    fn rules() {
        let data = arr(&[("title", Val::str("")), ("priority", Val::Int(9)), ("email", Val::str("x@y.de")), ("extra", Val::Int(1)), ("pw", Val::str("a")), ("pw_confirmation", Val::str("b"))]);
        let rules = arr(&[
            ("title", Val::str("required|string|max:20")),
            ("priority", Val::str("integer|between:1,5")),
            ("email", Val::str("nullable|email")),
            ("due", Val::str("nullable|date")),
            ("pw", Val::str("confirmed")),
        ]);
        let Err(errors) = validate(&data, &rules).unwrap() else { panic!("expected errors") };
        let fields: Vec<&str> = errors.0.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(fields, vec!["title", "priority", "pw"]);
        assert_eq!(errors.0[0].1, vec!["The title field is required."]);
        assert_eq!(errors.0[1].1, vec!["The priority field must be between 1 and 5."]);

        let good = arr(&[("title", Val::str("Ok")), ("priority", Val::str("3")), ("extra", Val::Int(1))]);
        let Ok(v) = validate(&good, &rules).unwrap() else { panic!("expected ok") };
        let Val::Array(v) = v else { panic!() };
        assert_eq!(v.len(), 2, "only ruled, present keys survive");
    }

    #[test]
    fn size_messages_name_the_unit_like_laravel() {
        let list = Val::array({
            let mut a = PhpArray::new();
            a.push(Val::Int(1));
            a
        });
        let data = arr(&[("pw", Val::str("short")), ("n", Val::Int(9)), ("tags", list.clone()), ("code", Val::str("abc")), ("ids", list)]);
        let rules = arr(&[
            ("pw", Val::str("string|min:8")),
            ("n", Val::str("integer|max:5")),
            ("tags", Val::str("array|min:2")),
            ("code", Val::str("string|size:4")),
            ("ids", Val::str("array|between:2,3")),
        ]);
        let Err(errors) = validate(&data, &rules).unwrap() else { panic!("expected errors") };
        let msgs: Vec<&str> = errors.0.iter().map(|(_, m)| m[0].as_str()).collect();
        assert_eq!(msgs, vec![
            "The pw field must be at least 8 characters.",
            "The n field must not be greater than 5.",
            "The tags field must have at least 2 items.",
            "The code field must be 4 characters.",
            "The ids field must have between 2 and 3 items.",
        ]);
    }
}
