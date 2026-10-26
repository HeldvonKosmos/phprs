//! `OpenGrid\Database\Model`: the native base class (DATABASE_LAYER.md §2.2).
//! Metadata comes from the dialect subclass's declarations, once per class.

use std::collections::BTreeMap;

use grd_vm::error::Throw;
use grd_vm::value::{ArrayKey, ClassId, ObjHandle, PhpArray, Val};
use grd_vm::vm::{Call, NativeClassDef, ObjectHandlers, Vm};
use indexmap::IndexMap;
use surrealdb::types::{Datetime, Object, RecordId, Value};

use super::builder::{self, BuilderData, data_object, hydrate};
use super::record_id;
use super::value::from_surreal;
use super::query_one;
use crate::native::ctx;

pub const CLASS: &str = "OpenGrid\\Database\\Model";

#[derive(Default, Clone)]
pub struct ModelData {
    /// Converted attributes (set by code, or materialised from `pending` on access).
    pub attributes: IndexMap<String, Val>,
    /// Fields of the loaded record not yet converted (lazy, DECISIONS 24.10.).
    pub pending: BTreeMap<String, Value>,
    pub dirty: Vec<String>,
    pub exists: bool,
    pub id: Option<RecordId>,
}

impl ModelData {
    pub fn has(&self, key: &str) -> bool {
        self.attributes.contains_key(key) || self.pending.contains_key(key)
    }
}

/// Convert one pending field into `attributes` (if any) and return it.
fn materialize(vm: &mut Vm, h: ObjHandle, key: &str) -> Result<Option<Val>, Throw> {
    if let Some(v) = data(vm, h)?.attributes.get(key) {
        return Ok(Some(v.clone()));
    }
    let Some(raw) = data(vm, h)?.pending.remove(key) else { return Ok(None) };
    let val = from_surreal(vm, raw)?;
    let d = data(vm, h)?;
    d.attributes.insert(key.to_string(), val.clone());
    Ok(Some(val))
}

/// Convert every pending field (for `toArray`, JSON, saves).
fn materialize_all(vm: &mut Vm, h: ObjHandle) -> Result<(), Throw> {
    let keys: Vec<String> = data(vm, h)?.pending.keys().cloned().collect();
    for k in keys {
        materialize(vm, h, &k)?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct ModelMeta {
    pub table: String,
    pub fillable: Vec<String>,
    pub hidden: Vec<String>,
    pub dates: Vec<String>,
    pub filterable: Vec<String>,
    pub soft_deletes: bool,
    pub timestamps: bool,
}

fn string_list(v: Option<Val>) -> Vec<String> {
    match v {
        Some(Val::Array(a)) => a.values().filter_map(|x| x.to_php_string().ok()).map(|s| s.to_string_lossy()).collect(),
        _ => Vec::new(),
    }
}

/// Declarations of the subclass, cached per class in the worker.
pub fn meta(vm: &mut Vm, class: ClassId) -> Result<ModelMeta, Throw> {
    if let Some(m) = ctx(vm).model_meta.get(&class) {
        return Ok(m.clone());
    }
    let table = match vm.static_default(class, "table") {
        Some(Val::Str(s)) => s.to_string_lossy(),
        _ => return Err(Throw::error(format!("{} declares no `protected static string $table`", vm.class_name(class)))),
    };
    let m = ModelMeta {
        table,
        fillable: string_list(vm.instance_default(class, "fillable")),
        hidden: string_list(vm.instance_default(class, "hidden")),
        dates: string_list(vm.instance_default(class, "dates")),
        filterable: string_list(vm.instance_default(class, "filterable")),
        soft_deletes: vm.instance_default(class, "softDeletes").is_some_and(|v| v.to_bool()),
        timestamps: vm.instance_default(class, "timestamps").is_none_or(|v| v.to_bool()),
    };
    ctx(vm).model_meta.insert(class, m.clone());
    Ok(m)
}

pub fn data<'a>(vm: &'a mut Vm, h: ObjHandle) -> Result<&'a mut ModelData, Throw> {
    vm.native_payload_mut::<ModelData>(h).ok_or_else(|| Throw::error("Model payload missing"))
}

pub fn new_builder(vm: &mut Vm, class: ClassId) -> Result<ObjHandle, Throw> {
    let m = meta(vm, class)?;
    builder::new_for_table(vm, &m.table, m.soft_deletes, Some(class))
}

/// A model instance from a record object.
pub fn from_record(vm: &mut Vm, class: ClassId, record: Object) -> Result<Val, Throw> {
    let h = vm.instantiate(class, Vec::new())?;
    load_record(vm, h, record)?;
    Ok(Val::Object(h))
}

fn load_record(vm: &mut Vm, h: ObjHandle, record: Object) -> Result<(), Throw> {
    let d = data(vm, h)?;
    d.attributes.clear();
    d.pending.clear();
    d.id = None;
    for (k, v) in record.into_inner() {
        if k == "id"
            && let Value::RecordId(r) = &v
        {
            d.id = Some(r.clone());
        }
        // NONE/NULL fields read as null without conversion work
        if matches!(v, Value::None | Value::Null) {
            d.attributes.insert(k, Val::Null);
        } else {
            d.pending.insert(k, v);
        }
    }
    d.exists = true;
    d.dirty.clear();
    Ok(())
}

fn fill(vm: &mut Vm, h: ObjHandle, values: &Val) -> Result<(), Throw> {
    let Val::Array(attrs) = values else { return Ok(()) };
    let class = vm.class_of(h);
    let fillable = meta(vm, class)?.fillable;
    let d = data(vm, h)?;
    for (k, v) in attrs.iter() {
        let key = match k {
            ArrayKey::Int(i) => i.to_string(),
            ArrayKey::Str(s) => s.to_string_lossy(),
        };
        if fillable.is_empty() || fillable.contains(&key) {
            d.pending.remove(&key);
            d.attributes.insert(key.clone(), v.clone());
            if !d.dirty.contains(&key) {
                d.dirty.push(key);
            }
        }
    }
    Ok(())
}

fn call_hook(vm: &mut Vm, h: ObjHandle, name: &str) -> Result<(), Throw> {
    let class = vm.class_of(h);
    if vm.has_method(class, name) {
        vm.call_method_sync(h, name.as_bytes(), Vec::new())?;
    }
    Ok(())
}

fn attributes_array(vm: &mut Vm, h: ObjHandle, hide: bool) -> Result<Val, Throw> {
    materialize_all(vm, h)?;
    let class = vm.class_of(h);
    let hidden = if hide { meta(vm, class)?.hidden } else { Vec::new() };
    let d = data(vm, h)?;
    let mut a = PhpArray::new();
    for (k, v) in &d.attributes {
        if !hidden.contains(k) {
            a.insert(ArrayKey::from_str(k), v.clone());
        }
    }
    Ok(Val::array(a))
}

pub(crate) fn save(vm: &mut Vm, h: ObjHandle) -> Result<Val, Throw> {
    let class = vm.class_of(h);
    let m = meta(vm, class)?;
    let (exists, dirty, id, attrs) = {
        let d = data(vm, h)?;
        let mut attrs = PhpArray::new();
        for (k, v) in &d.attributes {
            if k != "id" && (!d.exists || d.dirty.contains(k)) {
                attrs.insert(ArrayKey::from_str(k), v.clone());
            }
        }
        (d.exists, d.dirty.clone(), d.id.clone(), Val::array(attrs))
    };
    if exists && dirty.is_empty() {
        return Ok(Val::Bool(true));
    }
    let mut obj = data_object(vm, &attrs, Some(&m))?;
    let now = Value::Datetime(Datetime::now());
    if m.timestamps {
        if !exists {
            obj.insert("created_at", now.clone());
        }
        obj.insert("updated_at", now);
    }
    let mut bindings = BTreeMap::new();
    bindings.insert("data".to_string(), Value::Object(obj));
    let rows = if !exists {
        call_hook(vm, h, "creating")?;
        let explicit = data(vm, h)?.attributes.get("id").cloned();
        match explicit {
            Some(idv) if !idv.is_null() => {
                let rid = record_id::from_val(vm, &m.table, &idv)?;
                bindings.insert("rid".into(), Value::RecordId(rid));
                query_one(vm, "CREATE $rid CONTENT $data", bindings)?
            }
            _ => query_one(vm, &format!("CREATE {} CONTENT $data", m.table), bindings)?,
        }
    } else {
        call_hook(vm, h, "updating")?;
        let rid = id.ok_or_else(|| Throw::error("model has no id; cannot update"))?;
        bindings.insert("rid".into(), Value::RecordId(rid));
        query_one(vm, "UPDATE $rid MERGE $data", bindings)?
    };
    let record = match rows {
        Value::Array(a) => a.into_inner().into_iter().next(),
        Value::Object(o) => Some(Value::Object(o)),
        _ => None,
    };
    match record {
        Some(Value::Object(o)) => load_record(vm, h, o)?,
        _ => return Err(Throw::error("save: the database returned no record")),
    }
    call_hook(vm, h, if exists { "updated" } else { "created" })?;
    call_hook(vm, h, "saved")?;
    Ok(Val::Bool(true))
}

fn first_row(vm: &mut Vm, rows: Value) -> Result<Val, Throw> {
    let first = match rows {
        Value::Array(a) => a.into_inner().into_iter().next().unwrap_or(Value::None),
        other => other,
    };
    from_surreal(vm, first)
}

fn this_id(vm: &mut Vm, c: &Call) -> Result<RecordId, Throw> {
    let h = c.this()?;
    data(vm, h)?.id.clone().ok_or_else(|| Throw::error("model is not persisted"))
}

fn builder_call(vm: &mut Vm, c: Call, method: &[u8]) -> Result<Val, Throw> {
    let b = new_builder(vm, c.class)?;
    vm.call_method_sync(b, method, c.args)
}

/// DBP1 (M1): an API token and its user in one query (`FETCH user`) instead
/// of two. `[token, user]` or null; a soft-deleted token or user counts as
/// missing, as with `find()`. The token keeps the user's record id, so it
/// behaves as if loaded alone.
fn token_with_user(vm: &mut Vm, a: &[Val]) -> Result<Val, Throw> {
    let text = |i: usize| match a.get(i) {
        Some(Val::Str(s)) => Ok(s.to_string_lossy()),
        _ => Err(Throw::type_error("opengrid_token_with_user(): hash, token class and user class expected")),
    };
    let (hash, token_class, user_class) = (text(0)?, text(1)?, text(2)?);
    let tc = vm.lookup_class(token_class.as_bytes()).ok_or_else(|| Throw::error(format!("class {token_class} not found")))?;
    let uc = vm.lookup_class(user_class.as_bytes()).ok_or_else(|| Throw::error(format!("class {user_class} not found")))?;
    let (tm, um) = (meta(vm, tc)?, meta(vm, uc)?);
    let mut sql = format!("SELECT * FROM {} WHERE token_hash = $h", tm.table);
    if tm.soft_deletes {
        sql.push_str(" AND deleted_at IS NONE");
    }
    sql.push_str(" LIMIT 1 FETCH user");
    let mut vars = BTreeMap::new();
    vars.insert("h".to_string(), Value::String(hash));
    let rows = query_one(vm, &sql, vars)?;
    let row = match rows {
        Value::Array(a) => a.into_inner().into_iter().next(),
        _ => None,
    };
    let Some(Value::Object(mut token)) = row else { return Ok(Val::Null) };
    let Some(Value::Object(user)) = token.get("user").cloned() else { return Ok(Val::Null) };
    if um.soft_deletes && user.get("deleted_at").is_some_and(|d| !matches!(d, Value::None | Value::Null)) {
        return Ok(Val::Null);
    }
    if let Some(id) = user.get("id").cloned() {
        token.insert("user", id);
    }
    let t = from_record(vm, tc, token)?;
    let u = from_record(vm, uc, user)?;
    Ok(Val::array(PhpArray::from_values([t, u])))
}

pub fn register(vm: &mut Vm) -> Result<(), Throw> {
    vm.register_native("opengrid_token_with_user", token_with_user);
    let def = NativeClassDef::new(CLASS)
        .abstract_class()
        .implements(&["JsonSerializable"])
        .payload(|| Box::new(ModelData::default()))
        .trace(|p, out| {
            if let Some(m) = p.downcast_ref::<ModelData>() {
                out.extend(m.attributes.values().cloned());
            }
        })
        .handlers(ObjectHandlers {
            trace: None,
            get: Some(|vm, h, name| materialize(vm, h, &String::from_utf8_lossy(name))),
            set: Some(|vm, h, name, v| {
                let key = String::from_utf8_lossy(name).into_owned();
                let m = data(vm, h)?;
                m.pending.remove(&key);
                m.attributes.insert(key.clone(), v);
                if !m.dirty.contains(&key) {
                    m.dirty.push(key);
                }
                Ok(true)
            }),
            isset: Some(|vm, h, name| {
                let key = String::from_utf8_lossy(name).into_owned();
                Ok(Some(materialize(vm, h, &key)?.is_some_and(|v| !v.is_null())))
            }),
            unset: Some(|vm, h, name| {
                let key = String::from_utf8_lossy(name).into_owned();
                let m = data(vm, h)?;
                m.attributes.shift_remove(&key);
                m.pending.remove(&key);
                Ok(true)
            }),
            compare: None,
            arith: None,
            // `clone $model` copies attributes, pending raw fields, dirty state and id
            clone: Some(|p| p.downcast_ref::<ModelData>().map(|d| Box::new(d.clone()) as Box<dyn std::any::Any>)),
        })
        .method("__construct", |vm, c| {
            let this = c.this()?;
            fill(vm, this, &c.arg(0))?;
            Ok(Val::Null)
        })
        // static entry points → Builder bound to the called class
        .static_method("query", |vm, c| Ok(Val::Object(new_builder(vm, c.class)?)))
        // Q1: any other static call goes to a fresh builder, as in Laravel
        // (`User::latest()->first()`, `User::firstOrCreate([...])`)
        .static_method("__callStatic", |vm, c| {
            let name = crate::native::string_arg(&c, 0, "Model::__callStatic")?;
            let args = match c.arg(1) {
                Val::Array(a) => a.values().cloned().collect(),
                _ => Vec::new(),
            };
            let b = new_builder(vm, c.class)?;
            vm.call_method_sync(b, name.as_bytes(), args)
        })
        .static_method("where", |vm, c| builder_call(vm, c, b"where"))
        .static_method("whereIn", |vm, c| builder_call(vm, c, b"whereIn"))
        .static_method("whereLike", |vm, c| builder_call(vm, c, b"whereLike"))
        .static_method("whereNull", |vm, c| builder_call(vm, c, b"whereNull"))
        .static_method("orderBy", |vm, c| builder_call(vm, c, b"orderBy"))
        .static_method("sort", |vm, c| builder_call(vm, c, b"sort"))
        .static_method("select", |vm, c| builder_call(vm, c, b"select"))
        .static_method("filter", |vm, c| builder_call(vm, c, b"filter"))
        .static_method("withTrashed", |vm, c| builder_call(vm, c, b"withTrashed"))
        .static_method("withCount", |vm, c| builder_call(vm, c, b"withCount"))
        .static_method("withSum", |vm, c| builder_call(vm, c, b"withSum"))
        .static_method("withAll", |vm, c| builder_call(vm, c, b"withAll"))
        .static_method("onlyTrashed", |vm, c| builder_call(vm, c, b"onlyTrashed"))
        .static_method("all", |vm, c| builder_call(vm, c, b"get"))
        .static_method("count", |vm, c| builder_call(vm, c, b"count"))
        .static_method("first", |vm, c| builder_call(vm, c, b"first"))
        .static_method("paginate", |vm, c| builder_call(vm, c, b"paginate"))
        .static_method("find", |vm, c| {
            let m = meta(vm, c.class)?;
            let rid = record_id::from_val(vm, &m.table, &c.arg(0))?;
            let b = new_builder(vm, c.class)?;
            builder::data(vm, b)?.r#where("id", Value::RecordId(rid), "=");
            vm.call_method_sync(b, b"first", Vec::new())
        })
        .static_method("findOrFail", |vm, c| {
            let m = meta(vm, c.class)?;
            let rid = record_id::from_val(vm, &m.table, &c.arg(0))?;
            let b = new_builder(vm, c.class)?;
            builder::data(vm, b)?.r#where("id", Value::RecordId(rid.clone()), "=");
            let found = vm.call_method_sync(b, b"first", Vec::new())?;
            if found.is_null() {
                return Err(Throw::new("OpenGrid\\Database\\ModelNotFoundException", format!("{} {} not found", vm.class_name(c.class), surrealdb::types::ToSql::to_sql(&rid))));
            }
            Ok(found)
        })
        .static_method("create", |vm, c| {
            let h = vm.instantiate(c.class, vec![c.arg(0)])?;
            save(vm, h)?;
            Ok(Val::Object(h))
        })
        // instance
        .method("fill", |vm, c| {
            let this = c.this()?;
            fill(vm, this, &c.arg(0))?;
            Ok(Val::Object(this))
        })
        .method("save", |vm, c| save(vm, c.this()?))
        .method("update", |vm, c| {
            let this = c.this()?;
            fill(vm, this, &c.arg(0))?;
            save(vm, this)
        })
        .method("delete", |vm, c| {
            let this = c.this()?;
            let class = vm.class_of(this);
            let m = meta(vm, class)?;
            let rid = this_id(vm, &c)?;
            call_hook(vm, this, "deleting")?;
            let mut bindings = BTreeMap::new();
            bindings.insert("rid".to_string(), Value::RecordId(rid));
            if m.soft_deletes {
                let mut obj = Object::new();
                obj.insert("deleted_at", Value::Datetime(Datetime::now()));
                bindings.insert("data".into(), Value::Object(obj));
                let rows = query_one(vm, "UPDATE $rid MERGE $data", bindings)?;
                if let Value::Array(a) = rows
                    && let Some(Value::Object(o)) = a.into_inner().into_iter().next()
                {
                    load_record(vm, this, o)?;
                }
            } else {
                query_one(vm, "DELETE $rid", bindings)?;
                data(vm, this)?.exists = false;
            }
            call_hook(vm, this, "deleted")?;
            Ok(Val::Bool(true))
        })
        .method("forceDelete", |vm, c| {
            let this = c.this()?;
            let rid = this_id(vm, &c)?;
            let mut bindings = BTreeMap::new();
            bindings.insert("rid".to_string(), Value::RecordId(rid));
            query_one(vm, "DELETE $rid", bindings)?;
            data(vm, this)?.exists = false;
            Ok(Val::Bool(true))
        })
        .method("refresh", |vm, c| {
            let this = c.this()?;
            let rid = this_id(vm, &c)?;
            let mut bindings = BTreeMap::new();
            bindings.insert("rid".to_string(), Value::RecordId(rid));
            let rows = query_one(vm, "SELECT * FROM $rid", bindings)?;
            if let Value::Array(a) = rows
                && let Some(Value::Object(o)) = a.into_inner().into_iter().next()
            {
                load_record(vm, this, o)?;
            }
            Ok(Val::Object(this))
        })
        // graph edges: $task->relate('refers', $other, ['since' => ...]) → RELATE $from->refers->$to
        .method("relate", |vm, c| {
            let edge = crate::native::string_arg(&c, 0, "Model::relate")?;
            if !edge.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                return Err(Throw::value_error(format!("relate: edge name `{edge}` must be an identifier")));
            }
            let from = this_id(vm, &c)?;
            let to = match c.arg(1) {
                Val::Object(h) => match vm.native_payload::<ModelData>(h).and_then(|m| m.id.clone()) {
                    Some(id) => id,
                    None => record_id::from_val(vm, "", &Val::Object(h))?,
                },
                other => record_id::from_val(vm, "", &other)?,
            };
            let mut bindings = BTreeMap::new();
            bindings.insert("from".to_string(), Value::RecordId(from));
            bindings.insert("to".to_string(), Value::RecordId(to));
            let sql = match c.args.get(2) {
                Some(attrs @ Val::Array(_)) => {
                    let obj = data_object(vm, attrs, None)?;
                    bindings.insert("data".to_string(), Value::Object(obj));
                    format!("RELATE $from->{edge}->$to CONTENT $data")
                }
                _ => format!("RELATE $from->{edge}->$to"),
            };
            let rows = query_one(vm, &sql, bindings)?;
            Ok(first_row(vm, rows)?)
        })
        // $task->related('refers', Task::class) → the records behind the edge, as models
        .method("related", |vm, c| {
            let edge = crate::native::string_arg(&c, 0, "Model::related")?;
            let target = crate::native::string_arg(&c, 1, "Model::related")?;
            let class = vm.lookup_class(target.as_bytes()).ok_or_else(|| Throw::error(format!("related: class {target} not found")))?;
            let table = meta(vm, class)?.table;
            if !edge.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                return Err(Throw::value_error(format!("related: edge name `{edge}` must be an identifier")));
            }
            let from = this_id(vm, &c)?;
            let mut bindings = BTreeMap::new();
            bindings.insert("from".to_string(), Value::RecordId(from));
            let rows = query_one(vm, &format!("SELECT VALUE ->{edge}->{table}.* FROM $from"), bindings)?;
            // one array per from-record; we asked for one record
            let inner = match rows {
                Value::Array(a) => a.into_inner().into_iter().next().unwrap_or(Value::None),
                other => other,
            };
            hydrate(vm, Some(class), inner)
        })
        .method("toArray", |vm, c| attributes_array(vm, c.this()?, true))
        .method("getAttributes", |vm, c| attributes_array(vm, c.this()?, false))
        .method("jsonSerialize", |vm, c| {
            // honours a dialect override of toArray()
            vm.call_method_sync(c.this()?, b"toArray", Vec::new())
        })
        .method("getTable", |vm, c| {
            let class = vm.class_of(c.this()?);
            Ok(Val::str(&meta(vm, class)?.table))
        })
        .method("getKey", |vm, c| {
            let this = c.this()?;
            Ok(materialize(vm, this, "id")?.unwrap_or(Val::Null))
        })
        .method("isDirty", |vm, c| {
            let this = c.this()?;
            Ok(Val::Bool(!data(vm, this)?.dirty.is_empty()))
        })
        .method("exists", |vm, c| {
            let this = c.this()?;
            Ok(Val::Bool(data(vm, this)?.exists))
        });
    vm.register_class(def)?;
    let _: Option<BuilderData> = None;
    Ok(())
}
