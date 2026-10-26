//! `OpenGrid\Database\Builder`: port of the platform's `Query\Builder`
//! (DATABASE_LAYER.md §2.1). `to_sql` renders
//! `SELECT … FROM … WHERE … GROUP BY … ORDER BY … LIMIT n START m FETCH …`.

use std::collections::BTreeMap;

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, ClassId, ObjHandle, PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, Vm};
use surrealdb::types::{Array, Datetime, Object, Value};

use super::inflect::foreign_key;
use super::model::{self, ModelMeta};
use super::value::{from_surreal, to_surreal};
use super::{query, query_one};
use crate::native::{ctx, int_arg_opt, string_arg};

pub const CLASS: &str = "OpenGrid\\Database\\Builder";
const SOFT_DELETE_CLAUSE: &str = "deleted_at IS NONE";

#[derive(Clone, Debug, Default)]
pub struct BuilderData {
    pub table: String,
    pub wheres: Vec<String>,
    pub bindings: BTreeMap<String, Value>,
    pub orders: Vec<String>,
    pub fetch: Vec<String>,
    pub projection: Option<String>,
    pub groups: Vec<String>,
    pub soft_deletes: bool,
    pub limit: Option<i64>,
    pub start: i64,
    pub model: Option<ClassId>,
    /// Dot-path sort: path → alias projected as `path AS alias` (DATABASE_LAYER §2.1).
    pub order_projections: BTreeMap<String, String>,
    /// Side selects and aggregates appended to the projection (`withAll`, `withCount`, `withSum`, …).
    pub extra_projections: Vec<String>,
    /// Prefix for binding names, so a sub-builder's bindings merge into its parent's.
    pub prefix: String,
    /// Q1: closed OR groups of user conditions (`where a ->where b ->orWhere c`
    /// is `(a AND b) OR c`, as in Laravel); the current group lives in `wheres`.
    pub or_groups: Vec<Vec<String>>,
}

impl BuilderData {
    pub fn new(table: &str, soft_deletes: bool) -> Self {
        let mut b = BuilderData { table: table.to_string(), soft_deletes, ..Default::default() };
        if soft_deletes {
            b.wheres.push(SOFT_DELETE_CLAUSE.into());
        }
        b
    }

    pub fn bind(&mut self, v: Value) -> String {
        let name = format!("{}f{}", self.prefix, self.bindings.len());
        self.bindings.insert(name.clone(), v);
        name
    }

    /// A field for ORDER BY: dot paths get a projected alias (only with the
    /// default projection; an explicit `select` orders by the raw path).
    fn order_field(&mut self, field: &str) -> String {
        if field.contains('.') && self.projection.is_none() {
            let alias = format!("__sort_{}", field.replace('.', "_"));
            self.order_projections.insert(field.to_string(), alias.clone());
            alias
        } else {
            field.to_string()
        }
    }

    /// `(SELECT VALUE count() FROM comments WHERE task_id = $parent.id GROUP ALL)[0] ?? 0 AS comments_count`
    pub fn with_count(&mut self, table: &str, fk: Option<&str>) -> &mut Self {
        let fk = fk.map(str::to_string).unwrap_or_else(|| foreign_key(&self.table));
        self.extra_projections.push(format!("((SELECT VALUE count() FROM {table} WHERE {fk} = $parent.id GROUP ALL)[0] ?? 0) AS {table}_count"));
        self
    }

    /// `math::sum` / `math::mean` / `math::max` / `math::min` of `field` over the related rows.
    pub fn with_aggregate(&mut self, func: &str, label: &str, table: &str, field: &str, fk: Option<&str>) -> &mut Self {
        let fk = fk.map(str::to_string).unwrap_or_else(|| foreign_key(&self.table));
        self.extra_projections.push(format!(
            "((SELECT VALUE math::{func}({field}) FROM {table} WHERE {fk} = $parent.id GROUP ALL)[0] ?? 0) AS {table}_{label}_{field}"
        ));
        self
    }

    /// Side table: the related rows as a nested array, filtered by a sub-builder.
    pub fn with_all(&mut self, table: &str, sub: &BuilderData, fk: Option<&str>) -> &mut Self {
        let fk = fk.map(str::to_string).unwrap_or_else(|| foreign_key(&self.table));
        let mut sql = format!("SELECT {} FROM {table} WHERE {fk} = $parent.id", sub.projection.as_deref().unwrap_or("*"));
        for w in &sub.effective_wheres() {
            sql.push_str(" AND ");
            sql.push_str(w);
        }
        if !sub.orders.is_empty() {
            sql.push_str(" ORDER BY ");
            sql.push_str(&sub.orders.join(", "));
        }
        if let Some(l) = sub.limit {
            sql.push_str(&format!(" LIMIT {} START {}", l.max(0), sub.start.max(0)));
        }
        self.extra_projections.push(format!("({sql}) AS {table}"));
        self.bindings.extend(sub.bindings.iter().map(|(k, v)| (k.clone(), v.clone())));
        self
    }

    pub fn r#where(&mut self, field: &str, value: Value, op: &str) -> &mut Self {
        let n = self.bind(value);
        self.wheres.push(format!("{field} {op} ${n}"));
        self
    }

    /// The comparison `field op value` as a condition (field and operator
    /// checked: they go into the query text, the value is bound).
    pub fn condition(&mut self, field: &str, op: &str, value: Value) -> Result<String, String> {
        let field = check_field(field)?;
        let (op, like) = check_op(op)?;
        if let Some(negate) = like {
            // SQL LIKE as a case-insensitive regex: % = any run, _ = one character
            let pattern = match value {
                Value::String(s) => like_regex(&s),
                _ => return Err("like needs a string pattern".into()),
            };
            let n = self.bind(Value::String(pattern));
            return Ok(format!("{}string::matches(<string>{field}, ${n})", if negate { "!" } else { "" }));
        }
        let n = self.bind(value);
        Ok(format!("{field} {op} ${n}"))
    }

    /// Q1: `orWhere`: the current group is closed, the condition starts the next.
    pub fn or_condition(&mut self, clause: String) -> &mut Self {
        let (system, user): (Vec<String>, Vec<String>) = std::mem::take(&mut self.wheres).into_iter().partition(|w| is_system_clause(w));
        self.wheres = system;
        if !user.is_empty() || !self.or_groups.is_empty() {
            self.or_groups.push(user);
        }
        self.wheres.push(clause);
        self
    }

    /// The WHERE conditions as written into the query: system clauses (soft
    /// deletes) AND the user conditions, OR groups in parentheses.
    pub fn effective_wheres(&self) -> Vec<String> {
        if self.or_groups.is_empty() {
            return self.wheres.clone();
        }
        let (mut system, current): (Vec<String>, Vec<String>) = self.wheres.iter().cloned().partition(|w| is_system_clause(w));
        let groups: Vec<String> = self
            .or_groups
            .iter()
            .chain(std::iter::once(&current))
            .filter(|g| !g.is_empty())
            .map(|g| if g.len() == 1 { g[0].clone() } else { format!("({})", g.join(" AND ")) })
            .collect();
        system.push(format!("({})", groups.join(" OR ")));
        system
    }

    /// A nested builder for `where(fn ($q) => …)`: same table, own binding prefix.
    pub fn nested(&self) -> BuilderData {
        BuilderData { table: self.table.clone(), prefix: format!("{}n{}_", self.prefix, self.bindings.len()), model: self.model, ..Default::default() }
    }

    /// The conditions of a nested builder as one parenthesized condition; its bindings move here.
    pub fn absorb(&mut self, sub: BuilderData) -> Option<String> {
        let w = sub.effective_wheres();
        self.bindings.extend(sub.bindings);
        (!w.is_empty()).then(|| format!("({})", w.join(" AND ")))
    }

    pub fn where_raw(&mut self, clause: &str, bindings: BTreeMap<String, Value>) -> &mut Self {
        self.wheres.push(clause.to_string());
        self.bindings.extend(bindings);
        self
    }

    pub fn where_in(&mut self, field: &str, values: Vec<Value>) -> &mut Self {
        let c = self.in_condition(field, values, false);
        self.wheres.push(c);
        self
    }

    pub fn in_condition(&mut self, field: &str, values: Vec<Value>, not: bool) -> String {
        let mut arr = Array::new();
        arr.extend(values);
        let n = self.bind(Value::Array(arr));
        format!("{field} {}INSIDE ${n}", if not { "NOT" } else { "" })
    }

    pub fn where_like(&mut self, field: &str, value: &str) -> &mut Self {
        let n = self.bind(Value::String(value.to_lowercase()));
        self.wheres.push(format!("string::contains(string::lowercase(<string>{field}), ${n})"));
        self
    }

    pub fn where_none(&mut self, field: &str, not: bool) -> &mut Self {
        self.wheres.push(format!("{field} IS {}NONE", if not { "NOT " } else { "" }));
        self
    }

    pub fn with_trashed(&mut self) -> &mut Self {
        self.wheres.retain(|w| w != SOFT_DELETE_CLAUSE);
        self
    }

    pub fn only_trashed(&mut self) -> &mut Self {
        self.with_trashed();
        self.wheres.insert(0, "deleted_at IS NOT NONE".into());
        self
    }

    /// `"field:desc,other"` → `ORDER BY field COLLATE DESC, other COLLATE ASC`.
    /// `"title:desc, id"`. Field names and directions go into the query, so
    /// both are checked: a field is an identifier path (`a.b_c`), a
    /// direction `asc` or `desc`; anything else is an error and the builder
    /// stays unchanged (the spec often comes from the request).
    pub fn try_sort(&mut self, spec: &str) -> Result<&mut Self, String> {
        self.try_sort_with(spec, |_| true)
    }

    /// `try_sort`, with `COLLATE` only where `collate(field)` says the
    /// order depends on it (DBP1: on a number, bool or date it changes
    /// nothing, but keeps SurrealDB from reading the order from an index).
    pub fn try_sort_with(&mut self, spec: &str, collate: impl Fn(&str) -> bool) -> Result<&mut Self, String> {
        let mut parsed = Vec::new();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (field, dir) = part.split_once(':').unwrap_or((part, "asc"));
            parsed.push((check_field(field.trim())?, check_dir(dir.trim())?));
        }
        for (field, dir) in parsed {
            let f = self.order_field(&field);
            if collate(&field) {
                self.orders.push(format!("{f} COLLATE {dir}"));
            } else {
                self.orders.push(format!("{f} {dir}"));
            }
        }
        Ok(self)
    }

    /// `try_sort` for specs written in code (tests); panics on an invalid one.
    pub fn sort(&mut self, spec: &str) -> &mut Self {
        self.try_sort(spec).expect("valid sort spec")
    }

    pub fn try_order_by(&mut self, field: &str, dir: &str) -> Result<&mut Self, String> {
        let (field, dir) = (check_field(field)?, check_dir(dir)?);
        let f = self.order_field(&field);
        self.orders.push(format!("{f} {dir}"));
        Ok(self)
    }

    pub fn order_by(&mut self, field: &str, dir: &str) -> &mut Self {
        self.try_order_by(field, dir).expect("valid order")
    }

    fn projection_sql(&self) -> String {
        let mut parts = vec![self.projection.clone().unwrap_or_else(|| "*".to_string())];
        if self.projection.is_none() {
            for (path, alias) in &self.order_projections {
                parts.push(format!("{path} AS {alias}"));
            }
        }
        parts.extend(self.extra_projections.iter().cloned());
        parts.join(", ")
    }

    pub fn select(&mut self, projection: &str) -> &mut Self {
        self.projection = Some(projection.to_string());
        self
    }

    pub fn group_by(&mut self, fields: &[&str]) -> &mut Self {
        self.groups.extend(fields.iter().map(|f| f.to_string()));
        self
    }

    pub fn fetch(&mut self, field: &str) -> &mut Self {
        self.fetch.push(field.to_string());
        self
    }

    fn head(&self, projection: &str) -> String {
        let mut sql = format!("SELECT {projection} FROM {}", self.table);
        let wheres = self.effective_wheres();
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql
    }

    /// `SELECT VALUE math::{func}(field) …`: aggregate over the matching rows.
    pub fn aggregate_sql(&self, func: &str, field: &str) -> String {
        let mut sql = self.head(&format!("math::{func}({field}) AS v"));
        sql.push_str(" GROUP ALL");
        sql
    }

    /// `UPDATE … SET a += $n, b = $m …` over the matching rows (increment/decrement).
    pub fn set_sql(&self, sets: &[String]) -> String {
        format!("UPDATE {} SET {}{}", self.table, sets.join(", "), self.where_sql())
    }


    fn tail(&self, sql: &mut String) {
        if !self.groups.is_empty() {
            sql.push_str(" GROUP BY ");
            sql.push_str(&self.groups.join(", "));
        }
        if !self.orders.is_empty() {
            sql.push_str(" ORDER BY ");
            sql.push_str(&self.orders.join(", "));
        }
    }

    fn fetch_tail(&self, sql: &mut String) {
        for f in &self.fetch {
            sql.push_str(&format!(" FETCH {f}"));
        }
    }

    /// LIMIT/START before FETCH, FETCH last.
    pub fn to_sql(&self, per_page: usize, offset: usize) -> String {
        let mut sql = self.head(&self.projection_sql());
        self.tail(&mut sql);
        sql.push_str(&format!(" LIMIT {per_page} START {offset}"));
        self.fetch_tail(&mut sql);
        sql
    }

    /// The query as configured (LIMIT/START only when `limit` was set).
    pub fn select_sql(&self) -> String {
        match self.limit {
            Some(l) => self.to_sql(l.max(0) as usize, self.start.max(0) as usize),
            None => {
                let mut sql = self.head(&self.projection_sql());
                self.tail(&mut sql);
                self.fetch_tail(&mut sql);
                sql
            }
        }
    }

    pub fn count_sql(&self) -> String {
        let mut sql = self.head("count() AS total");
        sql.push_str(" GROUP ALL");
        sql
    }

    fn where_sql(&self) -> String {
        let wheres = self.effective_wheres();
        if wheres.is_empty() { String::new() } else { format!(" WHERE {}", wheres.join(" AND ")) }
    }

    pub fn update_sql(&self) -> String {
        format!("UPDATE {} MERGE $data{}", self.table, self.where_sql())
    }

    pub fn delete_sql(&self) -> String {
        format!("DELETE FROM {}{} RETURN BEFORE", self.table, self.where_sql())
    }

    pub fn bindings(&self) -> &BTreeMap<String, Value> {
        &self.bindings
    }
}

pub fn data<'a>(vm: &'a mut Vm, h: ObjHandle) -> Result<&'a mut BuilderData, Throw> {
    vm.native_payload_mut::<BuilderData>(h).ok_or_else(|| Throw::error("Builder payload missing"))
}

fn this_data<'a>(vm: &'a mut Vm, c: &Call) -> Result<&'a mut BuilderData, Throw> {
    let h = c.this()?;
    data(vm, h)
}

/// A builder object for `table` (no model).
/// A sort/order field: identifiers separated by dots.
fn check_field(field: &str) -> Result<String, String> {
    let ok = !field.is_empty()
        && field.split('.').all(|seg| seg.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    if ok { Ok(field.to_string()) } else { Err(format!("invalid sort field `{field}`")) }
}

/// Q1: comparison operators allowed in `where` (they go into the query
/// text). `<>` is `!=`; `like`/`not like` become a case-insensitive
/// `string::matches` (second value: Some(negated)).
fn check_op(op: &str) -> Result<(&'static str, Option<bool>), String> {
    Ok(match op.trim().to_ascii_lowercase().as_str() {
        "=" | "==" => ("=", None),
        "!=" | "<>" => ("!=", None),
        "<" => ("<", None),
        "<=" => ("<=", None),
        ">" => (">", None),
        ">=" => (">=", None),
        "like" => ("", Some(false)),
        "not like" => ("", Some(true)),
        "contains" => ("CONTAINS", None),
        "containsnot" => ("CONTAINSNOT", None),
        "containsany" => ("CONTAINSANY", None),
        "containsall" => ("CONTAINSALL", None),
        "inside" | "in" => ("INSIDE", None),
        "notinside" | "not in" => ("NOTINSIDE", None),
        _ => return Err(format!("invalid operator `{op}`")),
    })
}

fn is_operator(v: &Val) -> bool {
    matches!(v, Val::Str(s) if check_op(&s.to_string_lossy()).is_ok())
}

/// SQL LIKE pattern → anchored, case-insensitive regex.
fn like_regex(pattern: &str) -> String {
    let mut out = String::from("(?i)^");
    for c in pattern.chars() {
        match c {
            '%' => out.push_str(".*"),
            '_' => out.push('.'),
            c if "\\.^$|?*+()[]{}".contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    out
}

/// Clauses the builder adds itself (soft deletes); they stay outside OR groups.
fn is_system_clause(w: &str) -> bool {
    w == SOFT_DELETE_CLAUSE || w == "deleted_at IS NOT NONE"
}

fn check_dir(dir: &str) -> Result<&'static str, String> {
    match dir.to_ascii_lowercase().as_str() {
        "asc" => Ok("ASC"),
        "desc" => Ok("DESC"),
        _ => Err(format!("invalid sort direction `{dir}` (asc or desc)")),
    }
}

pub fn new_for_table(vm: &mut Vm, table: &str, soft_deletes: bool, model: Option<ClassId>) -> Result<ObjHandle, Throw> {
    let class = vm.lookup_class(CLASS.as_bytes()).ok_or_else(|| Throw::error("Builder class missing"))?;
    let h = vm.instantiate(class, Vec::new())?;
    let mut b = BuilderData::new(table, soft_deletes);
    b.model = model;
    *data(vm, h)? = b;
    Ok(h)
}

/// Rows → model objects (model-bound) or assoc arrays.
pub fn hydrate(vm: &mut Vm, model: Option<ClassId>, rows: Value) -> Result<Val, Throw> {
    let items = match rows {
        Value::Array(a) => a.into_inner(),
        Value::None | Value::Null => Vec::new(),
        other => vec![other],
    };
    let mut out = PhpArray::with_capacity(items.len());
    for item in items {
        let v = match (model, item) {
            (Some(class), Value::Object(o)) => model::from_record(vm, class, o)?,
            (_, other) => from_surreal(vm, other)?,
        };
        out.push(v);
    }
    Ok(Val::array(out))
}

/// Laravel's `where($column, $operator, $value)` and `where($column, $value)`;
/// the platform's order `where($column, $value, $operator)` is accepted too.
/// A Closure as first argument builds a nested group. None: nothing to add
/// (an empty group).
fn where_clause(vm: &mut Vm, c: &Call, what: &str) -> Result<Option<String>, Throw> {
    if let Val::Object(cl) = c.arg(0)
        && vm.is_callable(&Val::Object(cl))
    {
        let parent = this_data(vm, c)?.clone();
        let class = vm.lookup_class(CLASS.as_bytes()).ok_or_else(|| Throw::error("Builder class missing"))?;
        let sub = vm.instantiate(class, Vec::new())?;
        *data(vm, sub)? = parent.nested();
        vm.call_value(&Val::Object(cl), vec![Val::Object(sub)])?;
        let nested = std::mem::take(data(vm, sub)?);
        return Ok(this_data(vm, c)?.absorb(nested));
    }
    let field = string_arg(c, 0, what)?;
    let (op, value) = match c.args.len() {
        0 | 1 => return Err(Throw::argument_count(format!("{what}(): column and value expected"))),
        2 => ("=".to_string(), c.arg(1)),
        _ if is_operator(&c.args[1]) => (string_arg(c, 1, what)?, c.arg(2)),
        _ if is_operator(&c.args[2]) => (string_arg(c, 2, what)?, c.arg(1)),
        _ => return Err(Throw::value_error(format!("{what}(): invalid operator"))),
    };
    let value = to_surreal(vm, &value)?;
    this_data(vm, c)?.condition(&field, &op, value).map(Some).map_err(Throw::value_error)
}

fn checked(field: &str) -> Result<String, Throw> {
    check_field(field).map_err(|e| Throw::value_error(e.replace("sort field", "field")))
}

/// `[low, high]` of whereBetween as two bindings.
fn between(vm: &mut Vm, c: &Call, what: &str) -> Result<(String, String, String), Throw> {
    let field = checked(&string_arg(c, 0, what)?)?;
    let Val::Array(range) = c.arg(1) else { return Err(Throw::type_error(format!("{what}(): Argument #2 must be of type array"))) };
    let vals: Vec<Val> = range.values().cloned().collect();
    let [low, high] = vals.as_slice() else { return Err(Throw::value_error(format!("{what}(): two values expected"))) };
    let (low, high) = (to_surreal(vm, low)?, to_surreal(vm, high)?);
    let b = this_data(vm, c)?;
    Ok((field, b.bind(low), b.bind(high)))
}

/// The first value of an aggregate result `[{ v: … }]` (None when there are no rows).
fn aggregate_value(v: &Value) -> Option<Value> {
    match v {
        Value::Array(a) => match a.first() {
            Some(Value::Object(o)) => o.get("v").cloned(),
            _ => None,
        },
        _ => None,
    }
}

fn aggregate_call(vm: &mut Vm, c: &Call, func: &str, what: &str) -> Result<Val, Throw> {
    let field = checked(&string_arg(c, 0, what)?)?;
    let b = this_data(vm, c)?.clone();
    let v = query_one(vm, &b.aggregate_sql(func, &field), b.bindings.clone())?;
    match aggregate_value(&v) {
        // Laravel: sum of nothing is 0, avg/min/max of nothing is null
        None | Some(Value::None | Value::Null) => Ok(if func == "sum" { Val::Int(0) } else { Val::Null }),
        Some(v) => match from_surreal(vm, v)? {
            // math::mean of no rows is NaN
            Val::Float(f) if f.is_nan() => Ok(Val::Null),
            r => Ok(r),
        },
    }
}

fn increment_call(vm: &mut Vm, c: &Call, sign: &str, what: &str) -> Result<Val, Throw> {
    let field = checked(&string_arg(c, 0, what)?)?;
    let amount = match c.args.get(1) {
        None | Some(Val::Null) => to_surreal(vm, &Val::Int(1))?,
        Some(v @ (Val::Int(_) | Val::Float(_))) => to_surreal(vm, v)?,
        Some(o) => return Err(Throw::type_error(format!("{what}(): Argument #2 ($amount) must be of type int|float, {} given", o.type_name()))),
    };
    let extra = c.arg(2);
    let b = this_data(vm, c)?.clone();
    let timestamps = match b.model {
        Some(class) => model::meta(vm, class)?.timestamps,
        None => false,
    };
    let mut bindings = b.bindings.clone();
    let mut sets = vec![format!("{field} {sign}= $__amount")];
    bindings.insert("__amount".into(), amount);
    if let Val::Array(cols) = extra {
        for (i, (k, v)) in cols.iter().enumerate() {
            let key = match k {
                ArrayKey::Str(s) => checked(&s.to_string_lossy())?,
                ArrayKey::Int(_) => return Err(Throw::value_error(format!("{what}(): extra columns need names"))),
            };
            let name = format!("__extra{i}");
            bindings.insert(name.clone(), to_surreal(vm, v)?);
            sets.push(format!("{key} = ${name}"));
        }
    }
    if timestamps {
        sets.push("updated_at = time::now()".into());
    }
    let rows = query_one(vm, &b.set_sql(&sets), bindings)?;
    Ok(Val::Int(match rows {
        Value::Array(a) => a.len() as i64,
        _ => 0,
    }))
}

/// firstOrCreate / updateOrCreate: the row matching `$attributes`, created
/// with `$attributes + $values` when there is none.
fn first_matching(vm: &mut Vm, c: &Call, what: &str) -> Result<(BuilderData, Val), Throw> {
    let Val::Array(attrs) = c.arg(0) else { return Err(Throw::type_error(format!("{what}(): Argument #1 ($attributes) must be of type array"))) };
    let mut b = this_data(vm, c)?.clone();
    for (k, v) in attrs.iter() {
        let ArrayKey::Str(k) = k else { return Err(Throw::value_error(format!("{what}(): attributes need names"))) };
        let value = to_surreal(vm, v)?;
        let cond = b.condition(&k.to_string_lossy(), "=", value).map_err(Throw::value_error)?;
        b.wheres.push(cond);
    }
    let mut one = b.clone();
    one.limit = Some(1);
    let rows = query_one(vm, &one.select_sql(), one.bindings.clone())?;
    let found = first_of(hydrate(vm, one.model, rows)?);
    Ok((b, found))
}

fn create_with(vm: &mut Vm, b: &BuilderData, attrs: &Val, values: &Val) -> Result<Val, Throw> {
    let mut merged = match attrs {
        Val::Array(a) => (**a).clone(),
        _ => PhpArray::new(),
    };
    if let Val::Array(v) = values {
        for (k, x) in v.iter() {
            merged.insert(k.clone(), x.clone());
        }
    }
    match b.model {
        Some(class) => {
            let h = vm.instantiate(class, vec![Val::array(merged)])?;
            model::save(vm, h)?;
            Ok(Val::Object(h))
        }
        None => Err(Throw::error("firstOrCreate/updateOrCreate need a model")),
    }
}

fn first_of(v: Val) -> Val {
    match v {
        Val::Array(a) => a.values().next().cloned().unwrap_or(Val::Null),
        other => other,
    }
}

/// DBP1: the count query with safe scalar bindings written as literals.
/// SurrealDB 3.3 counts from an index only when the condition values are
/// literals (with `$f0` it reads every matching row: 54 ms against 7 ms for
/// 10 000 rows). Only bools, ints and record ids whose table and key are
/// plain identifiers are inlined (keys escaped as `⟨…⟩`); strings and every
/// other value stay parameters, so nothing from a request enters the text.
fn count_query(b: &BuilderData) -> (String, BTreeMap<String, Value>) {
    inline_literals(&b.count_sql(), &b.bindings)
}

fn inline_literals(sql: &str, bindings: &BTreeMap<String, Value>) -> (String, BTreeMap<String, Value>) {
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let literal = |v: &Value| -> Option<String> {
        match v {
            Value::Bool(x) => Some(x.to_string()),
            Value::Number(surrealdb::types::Number::Int(i)) => Some(i.to_string()),
            Value::RecordId(r) if ident(&r.table.to_string()) => match &r.key {
                surrealdb::types::RecordIdKey::String(k) if ident(k) => Some(format!("{}:⟨{}⟩", r.table, k)),
                surrealdb::types::RecordIdKey::Number(n) => Some(format!("{}:{}", r.table, n)),
                _ => None,
            },
            _ => None,
        }
    };
    let mut out = sql.to_string();
    let mut rest = BTreeMap::new();
    // longer names first, so `$f1` never matches inside `$f10`
    let mut names: Vec<&String> = bindings.keys().collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    for name in names {
        let v = &bindings[name];
        match literal(v) {
            Some(text) => {
                let pat = format!("${name}");
                let mut done = String::with_capacity(out.len());
                let mut i = 0;
                while let Some(pos) = out[i..].find(&pat) {
                    let at = i + pos;
                    let end = at + pat.len();
                    let boundary = out[end..].chars().next().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
                    done.push_str(&out[i..at]);
                    done.push_str(if boundary { &text } else { &pat });
                    i = end;
                }
                done.push_str(&out[i..]);
                out = done;
            }
            None => {
                rest.insert(name.clone(), v.clone());
            }
        }
    }
    (out, rest)
}

fn count_from(v: &Value) -> i64 {
    // `SELECT count() AS total … GROUP ALL` → [{ total: n }] or []
    if let Value::Array(a) = v
        && let Some(Value::Object(o)) = a.first()
        && let Some(Value::Number(n)) = o.get("total")
    {
        return match n {
            surrealdb::types::Number::Int(i) => *i,
            surrealdb::types::Number::Float(f) => *f as i64,
            surrealdb::types::Number::Decimal(d) => d.to_string().parse().unwrap_or(0),
        };
    }
    0
}

fn arg_values(vm: &mut Vm, v: &Val) -> Result<Vec<Value>, Throw> {
    match v {
        Val::Array(a) => a.values().map(|x| to_surreal(vm, x)).collect(),
        other => Ok(vec![to_surreal(vm, other)?]),
    }
}

/// `['title' => 'x']` → `$data` object; dates in `$dates` cast from strings.
pub fn data_object(vm: &mut Vm, values: &Val, meta: Option<&ModelMeta>) -> Result<Object, Throw> {
    let Val::Array(arr) = values else { return Err(Throw::type_error("expected an array of values")) };
    let mut obj = Object::new();
    for (k, v) in arr.iter() {
        let key = match k {
            ArrayKey::Int(i) => i.to_string(),
            ArrayKey::Str(s) => s.to_string_lossy(),
        };
        let is_date = meta.is_some_and(|m| m.dates.iter().any(|d| *d == key));
        let value = match (is_date, v) {
            (true, Val::Str(s)) => super::value::parse_datetime(&s.to_string_lossy()).map(Value::Datetime).unwrap_or(Value::String(s.to_string_lossy())),
            _ => to_surreal(vm, v)?,
        };
        obj.insert(key, value);
    }
    Ok(obj)
}

fn filter_allowed(vm: &mut Vm, b: &BuilderData, field: &str) -> Result<(), Throw> {
    let Some(class) = b.model else { return Ok(()) };
    let meta = model::meta(vm, class)?;
    let ok = if !meta.filterable.is_empty() {
        meta.filterable.iter().any(|f| f == field)
    } else {
        field == "id" || meta.fillable.iter().any(|f| f == field) || field.contains('.')
    };
    if ok { Ok(()) } else { Err(Throw::value_error(format!("filter on `{field}` is not allowed for {} (declare $filterable)", vm.class_name(class)))) }
}

/// Whether ordering by a field of this declared type depends on `COLLATE`:
/// false only when every part of the type is a non-string scalar.
pub fn needs_collate(ty: &str) -> bool {
    let t = ty.trim();
    let t = t.strip_prefix("option<").and_then(|r| r.strip_suffix('>')).unwrap_or(t);
    // split at top-level `|` only (`record<a | b>` is one part)
    let (mut parts, mut depth, mut start) = (Vec::new(), 0i32, 0usize);
    for (i, ch) in t.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth -= 1,
            '|' if depth == 0 => {
                parts.push(&t[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&t[start..]);
    !parts.iter().map(|p| p.trim()).all(|p| matches!(p, "none" | "null" | "int" | "float" | "number" | "decimal" | "bool" | "datetime" | "duration" | "uuid") || p.starts_with("record"))
}

/// Field types of a table from `INFO FOR TABLE` (`DEFINE FIELD x ON t TYPE bool …`),
/// cached per worker. Empty when the table has no schema.
pub fn schema_types(vm: &mut Vm, table: &str) -> Result<std::collections::HashMap<String, String>, Throw> {
    if let Some(t) = ctx(vm).schema_cache.get(table) {
        return Ok(t.clone());
    }
    let mut types = std::collections::HashMap::new();
    if ctx(vm).db.is_some() {
        let v = query_one(vm, &format!("INFO FOR TABLE {table}"), Default::default())?;
        if let Value::Object(o) = v
            && let Some(Value::Object(fields)) = o.get("fields")
        {
            for (name, def) in fields.iter() {
                if let Value::String(d) = def
                    && let Some(pos) = d.find(" TYPE ")
                {
                    // the type runs up to the next clause; SurrealDB 3 prints
                    // `option<int>` as `none | int` and `record<a | b>` with spaces
                    let rest = &d[pos + 6..];
                    let end = ["PERMISSIONS", "DEFAULT", "VALUE", "ASSERT", "READONLY", "COMMENT", "REFERENCE"]
                        .iter()
                        .filter_map(|k| rest.find(&format!(" {k}")))
                        .min()
                        .unwrap_or(rest.len());
                    types.insert(name.clone(), rest[..end].trim().to_string());
                }
            }
        }
    }
    ctx(vm).schema_cache.insert(table.to_string(), types.clone());
    Ok(types)
}

/// Filter values from query strings are text; the declared field type says
/// what they mean (DECISIONS 24.10.). Non-string values pass unchanged.
fn coerce_filter_value(vm: &mut Vm, table: &str, field: &str, v: &Val) -> Result<Val, Throw> {
    let Val::Str(s) = v else { return Ok(v.clone()) };
    let types = schema_types(vm, table)?;
    match types.get(field) {
        Some(ty) => super::value::cast_to_field_type(vm, ty, &s.to_string_lossy()),
        None => Ok(v.clone()),
    }
}

/// A query parameter of the active request (`?filters[done]=1&sort=title:desc`), if any.
fn request_param(vm: &mut Vm, name: &str) -> Option<Val> {
    let pairs = ctx(vm).request.as_ref()?.req.query_pairs();
    let Val::Array(all) = crate::native::request::nested_array(&pairs) else { return None };
    all.get(&ArrayKey::from_str(name)).cloned()
}

fn aggregate(vm: &mut Vm, c: Call, func: &str, label: &str) -> Result<Val, Throw> {
    let table = string_arg(&c, 0, "Builder::with*")?;
    let field = string_arg(&c, 1, "Builder::with*")?;
    let fk = if c.args.len() > 2 { Some(string_arg(&c, 2, "Builder::with*")?) } else { None };
    this_data(vm, &c)?.with_aggregate(func, label, &table, &field, fk.as_deref());
    Ok(Val::Object(c.this()?))
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    let def = NativeClassDef::new(CLASS)
        .final_class()
        .payload(|| Box::new(BuilderData::default()))
        .trace(grd_vm::vm::gc::trace_nothing)
        .method("where", |vm, c| {
            if let Some(clause) = where_clause(vm, &c, "Builder::where")? {
                this_data(vm, &c)?.wheres.push(clause);
            }
            Ok(Val::Object(c.this()?))
        })
        .method("orWhere", |vm, c| {
            if let Some(clause) = where_clause(vm, &c, "Builder::orWhere")? {
                this_data(vm, &c)?.or_condition(clause);
            }
            Ok(Val::Object(c.this()?))
        })
        .method("whereNot", |vm, c| {
            if let Some(clause) = where_clause(vm, &c, "Builder::whereNot")? {
                this_data(vm, &c)?.wheres.push(format!("!({clause})"));
            }
            Ok(Val::Object(c.this()?))
        })
        .method("orWhereNot", |vm, c| {
            if let Some(clause) = where_clause(vm, &c, "Builder::orWhereNot")? {
                this_data(vm, &c)?.or_condition(format!("!({clause})"));
            }
            Ok(Val::Object(c.this()?))
        })
        .method("whereBetween", |vm, c| {
            let (f, lo, hi) = between(vm, &c, "Builder::whereBetween")?;
            this_data(vm, &c)?.wheres.push(format!("({f} >= ${lo} AND {f} <= ${hi})"));
            Ok(Val::Object(c.this()?))
        })
        .method("whereNotBetween", |vm, c| {
            let (f, lo, hi) = between(vm, &c, "Builder::whereNotBetween")?;
            this_data(vm, &c)?.wheres.push(format!("({f} < ${lo} OR {f} > ${hi})"));
            Ok(Val::Object(c.this()?))
        })
        .method("whereColumn", |vm, c| {
            let first = checked(&string_arg(&c, 0, "Builder::whereColumn")?)?;
            let (op, second) = if c.args.len() > 2 { (string_arg(&c, 1, "Builder::whereColumn")?, string_arg(&c, 2, "Builder::whereColumn")?) } else { ("=".into(), string_arg(&c, 1, "Builder::whereColumn")?) };
            let (op, like) = check_op(&op).map_err(Throw::value_error)?;
            if like.is_some() {
                return Err(Throw::value_error("whereColumn(): like is not supported"));
            }
            let second = checked(&second)?;
            this_data(vm, &c)?.wheres.push(format!("{first} {op} {second}"));
            Ok(Val::Object(c.this()?))
        })
        .method("whereIn", |vm, c| {
            let field = checked(&string_arg(&c, 0, "Builder::whereIn")?)?;
            let values = arg_values(vm, &c.arg(1))?;
            this_data(vm, &c)?.where_in(&field, values);
            Ok(Val::Object(c.this()?))
        })
        .method("whereNotIn", |vm, c| {
            let field = checked(&string_arg(&c, 0, "Builder::whereNotIn")?)?;
            let values = arg_values(vm, &c.arg(1))?;
            let b = this_data(vm, &c)?;
            let cond = b.in_condition(&field, values, true);
            b.wheres.push(cond);
            Ok(Val::Object(c.this()?))
        })
        .method("orWhereIn", |vm, c| {
            let field = checked(&string_arg(&c, 0, "Builder::orWhereIn")?)?;
            let values = arg_values(vm, &c.arg(1))?;
            let b = this_data(vm, &c)?;
            let cond = b.in_condition(&field, values, false);
            b.or_condition(cond);
            Ok(Val::Object(c.this()?))
        })
        .method("orWhereNull", |vm, c| {
            let field = checked(&string_arg(&c, 0, "Builder::orWhereNull")?)?;
            this_data(vm, &c)?.or_condition(format!("{field} IS NONE"));
            Ok(Val::Object(c.this()?))
        })
        .method("orderByDesc", |vm, c| {
            let field = string_arg(&c, 0, "Builder::orderByDesc")?;
            this_data(vm, &c)?.try_order_by(&field, "desc").map_err(Throw::value_error)?;
            Ok(Val::Object(c.this()?))
        })
        .method("latest", |vm, c| {
            let field = if c.args.is_empty() { "created_at".to_string() } else { string_arg(&c, 0, "Builder::latest")? };
            this_data(vm, &c)?.try_order_by(&field, "desc").map_err(Throw::value_error)?;
            Ok(Val::Object(c.this()?))
        })
        .method("oldest", |vm, c| {
            let field = if c.args.is_empty() { "created_at".to_string() } else { string_arg(&c, 0, "Builder::oldest")? };
            this_data(vm, &c)?.try_order_by(&field, "asc").map_err(Throw::value_error)?;
            Ok(Val::Object(c.this()?))
        })
        .method("take", |vm, c| {
            let n = c.int(0, "Builder::take")?;
            this_data(vm, &c)?.limit = Some(n);
            Ok(Val::Object(c.this()?))
        })
        .method("skip", |vm, c| {
            let n = c.int(0, "Builder::skip")?;
            this_data(vm, &c)?.start = n;
            Ok(Val::Object(c.this()?))
        })
        .method("pluck", |vm, c| {
            let column = checked(&string_arg(&c, 0, "Builder::pluck")?)?;
            let key = if c.args.len() > 1 && !c.arg(1).is_null() { Some(checked(&string_arg(&c, 1, "Builder::pluck")?)?) } else { None };
            let mut b = this_data(vm, &c)?.clone();
            b.projection = Some(match &key {
                Some(k) => format!("{column} AS __v, {k} AS __k"),
                None => format!("{column} AS __v"),
            });
            let rows = query_one(vm, &b.select_sql(), b.bindings.clone())?;
            let mut out = PhpArray::new();
            if let Value::Array(rows) = rows {
                for row in rows.into_inner() {
                    let Value::Object(o) = row else { continue };
                    let v = from_surreal(vm, o.get("__v").cloned().unwrap_or(Value::None))?;
                    match o.get("__k") {
                        Some(k) if key.is_some() => {
                            let k = match from_surreal(vm, k.clone())? {
                                Val::Int(i) => ArrayKey::Int(i),
                                Val::Str(s) => ArrayKey::from_string(&s),
                                other => ArrayKey::from_string(&other.to_php_string()?),
                            };
                            out.insert(k, v);
                        }
                        _ => out.push(v),
                    }
                }
            }
            Ok(Val::array(out))
        })
        .method("value", |vm, c| {
            let column = checked(&string_arg(&c, 0, "Builder::value")?)?;
            let mut b = this_data(vm, &c)?.clone();
            b.projection = Some(format!("{column} AS __v"));
            b.limit = Some(1);
            let rows = query_one(vm, &b.select_sql(), b.bindings.clone())?;
            match rows {
                Value::Array(a) => match a.first() {
                    Some(Value::Object(o)) => from_surreal(vm, o.get("__v").cloned().unwrap_or(Value::None)),
                    _ => Ok(Val::Null),
                },
                _ => Ok(Val::Null),
            }
        })
        .method("sum", |vm, c| aggregate_call(vm, &c, "sum", "Builder::sum"))
        .method("avg", |vm, c| aggregate_call(vm, &c, "mean", "Builder::avg"))
        .method("min", |vm, c| aggregate_call(vm, &c, "min", "Builder::min"))
        .method("max", |vm, c| aggregate_call(vm, &c, "max", "Builder::max"))
        .method("increment", |vm, c| increment_call(vm, &c, "+", "Builder::increment"))
        .method("decrement", |vm, c| increment_call(vm, &c, "-", "Builder::decrement"))
        .method("doesntExist", |vm, c| {
            let b = this_data(vm, &c)?.clone();
            let v = { let (sql, vars) = count_query(&b); query_one(vm, &sql, vars) }?;
            Ok(Val::Bool(count_from(&v) == 0))
        })
        // chunk($size, fn ($rows, $page)): pages until a short page or the callback returns false
        .method("chunk", |vm, c| {
            let size = c.int(0, "Builder::chunk")?;
            if size < 1 {
                return Err(Throw::value_error("chunk(): size must be at least 1"));
            }
            let callback = c.arg(1);
            let b = this_data(vm, &c)?.clone();
            let mut page = 1i64;
            loop {
                let rows = query_one(vm, &b.to_sql(size as usize, ((page - 1) * size) as usize), b.bindings.clone())?;
                let rows = hydrate(vm, b.model, rows)?;
                let n = match &rows {
                    Val::Array(a) => a.len() as i64,
                    _ => 0,
                };
                if n == 0 {
                    break;
                }
                if matches!(vm.call_value(&callback, vec![rows, Val::Int(page)])?, Val::Bool(false)) {
                    return Ok(Val::Bool(false));
                }
                if n < size {
                    break;
                }
                page += 1;
            }
            Ok(Val::Bool(true))
        })
        .method("firstOrCreate", |vm, c| {
            let (b, found) = first_matching(vm, &c, "Builder::firstOrCreate")?;
            if !found.is_null() {
                return Ok(found);
            }
            create_with(vm, &b, &c.arg(0), &c.arg(1))
        })
        .method("updateOrCreate", |vm, c| {
            let (b, found) = first_matching(vm, &c, "Builder::updateOrCreate")?;
            if let Val::Object(h) = found {
                vm.call_method_sync(h, b"update", vec![c.arg(1)])?;
                return Ok(Val::Object(h));
            }
            create_with(vm, &b, &c.arg(0), &c.arg(1))
        })
        .method("whereLike", |vm, c| {
            let field = checked(&string_arg(&c, 0, "Builder::whereLike")?)?;
            let value = string_arg(&c, 1, "Builder::whereLike")?;
            this_data(vm, &c)?.where_like(&field, &value);
            Ok(Val::Object(c.this()?))
        })
        .method("whereRaw", |vm, c| {
            let clause = string_arg(&c, 0, "Builder::whereRaw")?;
            let mut bindings = BTreeMap::new();
            if let Val::Array(a) = c.arg(1) {
                for (k, v) in a.iter() {
                    let key = match k {
                        ArrayKey::Int(i) => i.to_string(),
                        ArrayKey::Str(s) => s.to_string_lossy(),
                    };
                    bindings.insert(key, to_surreal(vm, v)?);
                }
            }
            this_data(vm, &c)?.where_raw(&clause, bindings);
            Ok(Val::Object(c.this()?))
        })
        .method("whereNull", |vm, c| {
            let field = checked(&string_arg(&c, 0, "Builder::whereNull")?)?;
            this_data(vm, &c)?.where_none(&field, false);
            Ok(Val::Object(c.this()?))
        })
        .method("whereNotNull", |vm, c| {
            let field = checked(&string_arg(&c, 0, "Builder::whereNotNull")?)?;
            this_data(vm, &c)?.where_none(&field, true);
            Ok(Val::Object(c.this()?))
        })
        .method("withTrashed", |vm, c| {
            this_data(vm, &c)?.with_trashed();
            Ok(Val::Object(c.this()?))
        })
        .method("onlyTrashed", |vm, c| {
            this_data(vm, &c)?.only_trashed();
            Ok(Val::Object(c.this()?))
        })
        .method("orderBy", |vm, c| {
            let field = string_arg(&c, 0, "Builder::orderBy")?;
            let dir = if c.args.len() > 1 { string_arg(&c, 1, "Builder::orderBy")? } else { "asc".into() };
            this_data(vm, &c)?.try_order_by(&field, &dir).map_err(Throw::value_error)?;
            Ok(Val::Object(c.this()?))
        })
        .method("sort", |vm, c| {
            let from_request = matches!(c.args.first(), None | Some(Val::Null));
            let spec = if from_request {
                request_param(vm, "sort").and_then(|v| v.to_php_string().ok()).map(|s| s.to_string_lossy()).unwrap_or_default()
            } else {
                string_arg(&c, 0, "Builder::sort")?
            };
            // DBP1: no COLLATE on fields the schema declares as numbers, bools,
            // dates or records; unknown fields and paths keep it
            let table = this_data(vm, &c)?.table.clone();
            let types = schema_types(vm, &table)?;
            if let Err(e) = this_data(vm, &c)?.try_sort_with(&spec, |field| types.get(field).is_none_or(|t| needs_collate(t))) {
                // a bad `?sort=` is the client's error: 400
                if from_request {
                    vm.call_value(&Val::str("abort"), vec![Val::Int(400), Val::str(&e)])?;
                }
                return Err(Throw::value_error(e));
            }
            Ok(Val::Object(c.this()?))
        })
        .method("withCount", |vm, c| {
            let table = string_arg(&c, 0, "Builder::withCount")?;
            let fk = if c.args.len() > 1 { Some(string_arg(&c, 1, "Builder::withCount")?) } else { None };
            this_data(vm, &c)?.with_count(&table, fk.as_deref());
            Ok(Val::Object(c.this()?))
        })
        .method("withSum", |vm, c| aggregate(vm, c, "sum", "sum"))
        .method("withAvg", |vm, c| aggregate(vm, c, "mean", "avg"))
        .method("withMax", |vm, c| aggregate(vm, c, "max", "max"))
        .method("withMin", |vm, c| aggregate(vm, c, "min", "min"))
        .method("withAll", |vm, c| {
            let this = c.this()?;
            let table = string_arg(&c, 0, "Builder::withAll")?;
            let fk = if c.args.len() > 2 { Some(string_arg(&c, 2, "Builder::withAll")?) } else { None };
            // sub-builder with its own binding prefix; the closure mutates it
            let n = this_data(vm, &c)?.extra_projections.len() + 1;
            let sub = new_for_table(vm, &table, false, None)?;
            data(vm, sub)?.prefix = format!("s{n}_");
            if let Some(closure) = c.args.get(1)
                && !closure.is_null()
            {
                vm.call_value(closure, vec![Val::Object(sub)])?;
            }
            let sub_data = data(vm, sub)?.clone();
            data(vm, this)?.with_all(&table, &sub_data, fk.as_deref());
            Ok(Val::Object(this))
        })
        .method("select", |vm, c| {
            let projection = string_arg(&c, 0, "Builder::select")?;
            this_data(vm, &c)?.select(&projection);
            Ok(Val::Object(c.this()?))
        })
        .method("groupBy", |vm, c| {
            let fields: Vec<String> = (0..c.args.len()).map(|i| string_arg(&c, i, "Builder::groupBy")).collect::<Result<_, _>>()?;
            let refs: Vec<&str> = fields.iter().map(String::as_str).collect();
            this_data(vm, &c)?.group_by(&refs);
            Ok(Val::Object(c.this()?))
        })
        .method("fetch", |vm, c| {
            let field = string_arg(&c, 0, "Builder::fetch")?;
            this_data(vm, &c)?.fetch(&field);
            Ok(Val::Object(c.this()?))
        })
        .method("limit", |vm, c| {
            let n = c.int(0, "Builder::limit")?;
            this_data(vm, &c)?.limit = Some(n);
            Ok(Val::Object(c.this()?))
        })
        .method("start", |vm, c| {
            let n = c.int(0, "Builder::start")?;
            this_data(vm, &c)?.start = n;
            Ok(Val::Object(c.this()?))
        })
        .method("offset", |vm, c| {
            let n = c.int(0, "Builder::offset")?;
            this_data(vm, &c)?.start = n;
            Ok(Val::Object(c.this()?))
        })
        .method("when", |vm, c| {
            let this = c.this()?;
            if c.arg(0).to_bool() {
                vm.call_value(&c.arg(1), vec![Val::Object(this)])?;
            }
            Ok(Val::Object(this))
        })
        .method("filter", |vm, c| {
            let this = c.this()?;
            let filters = match c.args.first() {
                None | Some(Val::Null) => request_param(vm, "filters").unwrap_or(Val::Null),
                Some(v) => v.clone(),
            };
            let Val::Array(filters) = filters else { return Ok(Val::Object(this)) };
            for (k, v) in filters.iter() {
                let field = match k {
                    ArrayKey::Int(i) => i.to_string(),
                    ArrayKey::Str(s) => s.to_string_lossy(),
                };
                let b = data(vm, this)?.clone();
                filter_allowed(vm, &b, &field)?;
                match v {
                    Val::Null => {
                        data(vm, this)?.where_none(&field, false);
                    }
                    Val::Array(items) => {
                        let table = data(vm, this)?.table.clone();
                        let mut values = Vec::new();
                        for item in items.values() {
                            let c = coerce_filter_value(vm, &table, &field, item)?;
                            values.push(to_surreal(vm, &c)?);
                        }
                        data(vm, this)?.where_in(&field, values);
                    }
                    other => {
                        let table = data(vm, this)?.table.clone();
                        let c = coerce_filter_value(vm, &table, &field, other)?;
                        let value = to_surreal(vm, &c)?;
                        data(vm, this)?.r#where(&field, value, "=");
                    }
                }
            }
            Ok(Val::Object(this))
        })
        .method("toSql", |vm, c| Ok(Val::str(&this_data(vm, &c)?.select_sql())))
        .method("countSql", |vm, c| Ok(Val::str(&this_data(vm, &c)?.count_sql())))
        .method("getBindings", |vm, c| {
            let bindings = this_data(vm, &c)?.bindings.clone();
            let mut a = PhpArray::new();
            for (k, v) in bindings {
                let val = from_surreal(vm, v)?;
                a.insert(ArrayKey::from_str(&k), val);
            }
            Ok(Val::array(a))
        })
        .method("getModel", |vm, c| {
            let model = this_data(vm, &c)?.model;
            Ok(model.map(|m| Val::str(&vm.class_name(m))).unwrap_or(Val::Null))
        })
        // terminals
        .method("get", |vm, c| {
            let b = this_data(vm, &c)?.clone();
            let rows = query_one(vm, &b.select_sql(), b.bindings.clone())?;
            hydrate(vm, b.model, rows)
        })
        .method("first", |vm, c| {
            let mut b = this_data(vm, &c)?.clone();
            b.limit = Some(1);
            let rows = query_one(vm, &b.select_sql(), b.bindings.clone())?;
            Ok(first_of(hydrate(vm, b.model, rows)?))
        })
        .method("count", |vm, c| {
            let b = this_data(vm, &c)?.clone();
            let v = { let (sql, vars) = count_query(&b); query_one(vm, &sql, vars) }?;
            Ok(Val::Int(count_from(&v)))
        })
        .method("exists", |vm, c| {
            let b = this_data(vm, &c)?.clone();
            let v = { let (sql, vars) = count_query(&b); query_one(vm, &sql, vars) }?;
            Ok(Val::Bool(count_from(&v) > 0))
        })
        .method("paginate", |vm, c| {
            let per_page = int_arg_opt(&c, 0, 15)?.max(1);
            let page = int_arg_opt(&c, 1, 1)?.max(1);
            let b = this_data(vm, &c)?.clone();
            let total = count_from(&{ let (sql, vars) = count_query(&b); query_one(vm, &sql, vars) }?);
            let offset = (page - 1) * per_page;
            let rows = query_one(vm, &b.to_sql(per_page as usize, offset as usize), b.bindings.clone())?;
            let data = hydrate(vm, b.model, rows)?;
            let mut out = PhpArray::new();
            out.insert(ArrayKey::from_str("data"), data);
            out.insert(ArrayKey::from_str("total"), Val::Int(total));
            out.insert(ArrayKey::from_str("per_page"), Val::Int(per_page));
            out.insert(ArrayKey::from_str("current_page"), Val::Int(page));
            out.insert(ArrayKey::from_str("last_page"), Val::Int(((total + per_page - 1) / per_page).max(1)));
            Ok(Val::array(out))
        })
        .method("update", |vm, c| {
            let b = this_data(vm, &c)?.clone();
            let meta = match b.model {
                Some(class) => Some(model::meta(vm, class)?),
                None => None,
            };
            let mut obj = data_object(vm, &c.arg(0), meta.as_ref())?;
            if meta.as_ref().is_none_or(|m| m.timestamps) {
                obj.insert("updated_at", Value::Datetime(Datetime::now()));
            }
            let mut bindings = b.bindings.clone();
            bindings.insert("data".into(), Value::Object(obj));
            let rows = query_one(vm, &b.update_sql(), bindings)?;
            Ok(Val::Int(match rows {
                Value::Array(a) => a.len() as i64,
                _ => 0,
            }))
        })
        .method("insert", |vm, c| {
            let b = this_data(vm, &c)?.clone();
            let meta = match b.model {
                Some(class) => Some(model::meta(vm, class)?),
                None => None,
            };
            let mut obj = data_object(vm, &c.arg(0), meta.as_ref())?;
            if meta.as_ref().is_none_or(|m| m.timestamps) {
                let now = Value::Datetime(Datetime::now());
                obj.insert("created_at", now.clone());
                obj.insert("updated_at", now);
            }
            let mut bindings = BTreeMap::new();
            bindings.insert("data".to_string(), Value::Object(obj));
            let rows = query_one(vm, &format!("CREATE {} CONTENT $data", b.table), bindings)?;
            Ok(first_of(hydrate(vm, b.model, rows)?))
        })
        .method("delete", |vm, c| {
            let b = this_data(vm, &c)?.clone();
            if b.soft_deletes {
                let mut obj = Object::new();
                obj.insert("deleted_at", Value::Datetime(Datetime::now()));
                let mut bindings = b.bindings.clone();
                bindings.insert("data".into(), Value::Object(obj));
                let rows = query_one(vm, &b.update_sql(), bindings)?;
                return Ok(Val::Int(match rows {
                    Value::Array(a) => a.len() as i64,
                    _ => 0,
                }));
            }
            let rows = query_one(vm, &b.delete_sql(), b.bindings.clone())?;
            Ok(Val::Int(match rows {
                Value::Array(a) => a.len() as i64,
                _ => 0,
            }))
        })
        .method("forceDelete", |vm, c| {
            let mut b = this_data(vm, &c)?.clone();
            b.with_trashed();
            let rows = query_one(vm, &b.delete_sql(), b.bindings.clone())?;
            Ok(Val::Int(match rows {
                Value::Array(a) => a.len() as i64,
                _ => 0,
            }))
        });
    vm.register_class(def)?;
    let _ = query; // used by siblings
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn count_queries_inline_only_safe_literals() {
        let mut vars = BTreeMap::new();
        vars.insert("f0".to_string(), Value::RecordId(RecordId::new("users", RecordIdKey::String("u1x".into()))));
        vars.insert("f1".to_string(), Value::Bool(false));
        vars.insert("f10".to_string(), Value::Number(surrealdb::types::Number::Int(7)));
        vars.insert("f2".to_string(), Value::String("x' OR 1=1".into()));
        vars.insert("f3".to_string(), Value::RecordId(RecordId::new("users", RecordIdKey::String("a b".into()))));
        let (sql, rest) = inline_literals("WHERE user = $f0 AND done = $f1 AND n = $f10 AND t = $f2 AND u = $f3", &vars);
        assert_eq!(sql, "WHERE user = users:⟨u1x⟩ AND done = false AND n = 7 AND t = $f2 AND u = $f3");
        assert_eq!(rest.keys().cloned().collect::<Vec<_>>(), vec!["f2".to_string(), "f3".to_string()], "strings and unusual keys stay parameters");
    }

    #[test]
    fn collate_only_where_the_order_depends_on_it() {
        for t in ["int", "none | int", "option<float>", "bool", "datetime", "record<users>", "none | record<users | teams>", "decimal"] {
            assert!(!needs_collate(t), "{t}");
        }
        for t in ["string", "option<string>", "none | string", "int | string", "any", "array<string>", "object"] {
            assert!(needs_collate(t), "{t}");
        }
        let mut b = BuilderData::new("notes", false);
        b.try_sort_with("priority:desc,title", |f| f != "priority").unwrap();
        assert_eq!(b.orders, vec!["priority DESC".to_string(), "title COLLATE ASC".to_string()]);
    }

    #[test]
    fn sort_specs_are_checked() {
        // the spec comes from `?sort=`: nothing but identifier paths and asc/desc reach the query
        let mut b = BuilderData::new("t", false);
        for bad in ["title:desc;DELETE t", "title; DELETE t", "title:up", "ti tle", "(title)", "title:asc DROP", "1abc", "a..b", "a.-b"] {
            assert!(b.try_sort(bad).is_err(), "{bad}");
            assert!(b.orders.is_empty(), "{bad} left nothing behind");
        }
        assert!(b.try_order_by("title", "desc; DROP").is_err());
        b.try_sort("title:DESC, client.name:asc, _id").unwrap();
        assert_eq!(b.orders.len(), 3);
    }

    use super::*;
    use surrealdb::types::{RecordId, RecordIdKey};

    #[test]
    fn renders_like_the_php_builder() {
        let mut b = BuilderData::new("tasks", true);
        b.r#where("done", Value::Bool(false), "=").order_by("id", "desc");
        assert_eq!(b.to_sql(10, 0), "SELECT * FROM tasks WHERE deleted_at IS NONE AND done = $f0 ORDER BY id DESC LIMIT 10 START 0");
        assert_eq!(b.count_sql(), "SELECT count() AS total FROM tasks WHERE deleted_at IS NONE AND done = $f0 GROUP ALL");
        assert_eq!(b.bindings().len(), 1);

        let mut b = BuilderData::new("tasks", false);
        b.sort("title:desc, id").where_like("title", "Alp").fetch("client").select("id, title").limit = Some(5);
        assert_eq!(
            b.select_sql(),
            "SELECT id, title FROM tasks WHERE string::contains(string::lowercase(<string>title), $f0) ORDER BY title COLLATE DESC, id COLLATE ASC LIMIT 5 START 0 FETCH client"
        );
        assert_eq!(b.bindings()["f0"], Value::String("alp".into()));

        let mut b = BuilderData::new("tasks", true);
        b.with_trashed().where_in("id", vec![Value::Number(surrealdb::types::Number::Int(1))]).group_by(&["client"]);
        assert_eq!(b.select_sql(), "SELECT * FROM tasks WHERE id INSIDE $f0 GROUP BY client");
        assert_eq!(b.update_sql(), "UPDATE tasks MERGE $data WHERE id INSIDE $f0");
        assert_eq!(b.delete_sql(), "DELETE FROM tasks WHERE id INSIDE $f0 RETURN BEFORE");

        let mut b = BuilderData::new("tasks", true);
        b.only_trashed();
        assert_eq!(b.select_sql(), "SELECT * FROM tasks WHERE deleted_at IS NOT NONE");

        let mut b = BuilderData::new("comments", false);
        b.sort("task_id.title:desc,hours");
        assert_eq!(b.select_sql(), "SELECT *, task_id.title AS __sort_task_id_title FROM comments ORDER BY __sort_task_id_title COLLATE DESC, hours COLLATE ASC");

        let mut b = BuilderData::new("tasks", false);
        let mut sub = BuilderData::new("comments", false);
        sub.prefix = "s1_".into();
        sub.r#where("hours", Value::Number(surrealdb::types::Number::Int(2)), ">").order_by("hours", "asc");
        b.with_count("comments", None).with_aggregate("sum", "sum", "comments", "hours", None).with_all("comments", &sub, None);
        assert_eq!(
            b.select_sql(),
            "SELECT *, ((SELECT VALUE count() FROM comments WHERE task_id = $parent.id GROUP ALL)[0] ?? 0) AS comments_count, \
             ((SELECT VALUE math::sum(hours) FROM comments WHERE task_id = $parent.id GROUP ALL)[0] ?? 0) AS comments_sum_hours, \
             (SELECT * FROM comments WHERE task_id = $parent.id AND hours > $s1_f0 ORDER BY hours ASC) AS comments FROM tasks"
        );
        assert!(b.bindings().contains_key("s1_f0"));
    }
}
