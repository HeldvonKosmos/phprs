//! Phase 6 acceptance: a native base class (`Model`) that reads the dialect
//! subclass's declarations, a native `Builder` mutated from closures, and
//! `toSql()` in the shape of DATABASE_LAYER.md §2.1.

use std::collections::BTreeMap;

use grd_vm::error::Throw;
use grd_vm::value::*;
use grd_vm::vm::{Call, FromVal, NativeClassDef, ObjectHandlers, Vm};

#[derive(Default)]
struct BuilderData {
    table: String,
    wheres: Vec<String>,
    bindings: BTreeMap<String, Val>,
    orders: Vec<String>,
    projection: Option<String>,
    groups: Vec<String>,
    fetch: Vec<String>,
    limit: Option<i64>,
    start: i64,
    model: Option<ClassId>,
}

impl BuilderData {
    fn to_sql(&self) -> String {
        let mut sql = format!("SELECT {} FROM {}", self.projection.as_deref().unwrap_or("*"), self.table);
        if !self.wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&self.wheres.join(" AND "));
        }
        if !self.groups.is_empty() {
            sql.push_str(" GROUP BY ");
            sql.push_str(&self.groups.join(", "));
        }
        if !self.orders.is_empty() {
            sql.push_str(" ORDER BY ");
            sql.push_str(&self.orders.join(", "));
        }
        if let Some(l) = self.limit {
            sql.push_str(&format!(" LIMIT {l} START {}", self.start));
        }
        for f in &self.fetch {
            sql.push_str(&format!(" FETCH {f}"));
        }
        sql
    }
}

#[derive(Default)]
struct ModelData {
    attributes: indexmap::IndexMap<String, Val>,
    dirty: Vec<String>,
}

#[allow(dead_code)]
fn unused(_: Call) {}

fn builder_mut<'a>(vm: &'a mut Vm, h: ObjHandle) -> &'a mut BuilderData {
    vm.native_payload_mut::<BuilderData>(h).expect("Builder payload")
}

fn new_builder(vm: &mut Vm, model: ClassId) -> Result<ObjHandle, Throw> {
    let builder_class = vm.lookup_class(b"OpenGrid\\Database\\Builder").expect("Builder registered");
    let h = vm.instantiate(builder_class, Vec::new())?;
    let table = vm.static_default(model, "table").and_then(|v| v.to_php_string().ok()).map(|s| s.to_string()).unwrap_or_else(|| "unknown".into());
    let soft = vm.instance_default(model, "softDeletes").is_some_and(|v| v.to_bool());
    let b = builder_mut(vm, h);
    b.table = table;
    b.model = Some(model);
    if soft {
        b.wheres.push("deleted_at IS NONE".into());
    }
    Ok(h)
}

fn register(vm: &mut Vm) {
    let builder = NativeClassDef::new("OpenGrid\\Database\\Builder")
        .final_class()
        .payload(|| Box::new(BuilderData::default()))
        .method("where", |vm, c| {
            let this = c.this()?;
            let field = c.arg(0).to_php_string()?.to_string();
            let value = c.arg(1);
            let op = if c.args.len() > 2 { c.arg(2).to_php_string()?.to_string() } else { "=".into() };
            let b = builder_mut(vm, this);
            let name = format!("f{}", b.bindings.len());
            b.wheres.push(format!("{field} {op} ${name}"));
            b.bindings.insert(name, value);
            Ok(Val::Object(this))
        })
        .method("whereIn", |vm, c| {
            let this = c.this()?;
            let field = c.arg(0).to_php_string()?.to_string();
            let b = builder_mut(vm, this);
            let name = format!("f{}", b.bindings.len());
            b.wheres.push(format!("{field} INSIDE ${name}"));
            b.bindings.insert(name, c.arg(1));
            Ok(Val::Object(this))
        })
        .method("orderBy", |vm, c| {
            let this = c.this()?;
            let field = c.arg(0).to_php_string()?.to_string();
            let dir = if c.args.len() > 1 { c.arg(1).to_php_string()?.to_string().to_uppercase() } else { "ASC".into() };
            builder_mut(vm, this).orders.push(format!("{field} {dir}"));
            Ok(Val::Object(this))
        })
        .method("select", |vm, c| {
            let this = c.this()?;
            builder_mut(vm, this).projection = Some(c.arg(0).to_php_string()?.to_string());
            Ok(Val::Object(this))
        })
        .method("groupBy", |vm, c| {
            let this = c.this()?;
            for a in &c.args {
                let f = a.to_php_string()?.to_string();
                builder_mut(vm, this).groups.push(f);
            }
            Ok(Val::Object(this))
        })
        .method("fetch", |vm, c| {
            let this = c.this()?;
            builder_mut(vm, this).fetch.push(c.arg(0).to_php_string()?.to_string());
            Ok(Val::Object(this))
        })
        .method("limit", |vm, c| {
            let this = c.this()?;
            let n = i64::from_val(&c.arg(0), vm)?; // strict: "zehn" is a TypeError
            builder_mut(vm, this).limit = Some(n);
            Ok(Val::Object(this))
        })
        .method("start", |vm, c| {
            let this = c.this()?;
            builder_mut(vm, this).start = c.arg(0).to_int()?;
            Ok(Val::Object(this))
        })
        .method("when", |vm, c| {
            let this = c.this()?;
            if c.arg(0).to_bool() {
                // the closure mutates this very builder: object identity at work
                vm.call_value(&c.arg(1), vec![Val::Object(this)])?;
            }
            Ok(Val::Object(this))
        })
        .method("toSql", |vm, c| {
            let this = c.this()?;
            Ok(Val::from(builder_mut(vm, this).to_sql()))
        })
        .method("getBindings", |vm, c| {
            let this = c.this()?;
            let mut a = PhpArray::new();
            for (k, v) in &builder_mut(vm, this).bindings {
                a.insert(ArrayKey::from_str(k), v.clone());
            }
            Ok(Val::array(a))
        })
        .method("getModel", |vm, c| {
            let this = c.this()?;
            Ok(builder_mut(vm, this).model.map(|m| Val::from(vm.class_name(m))).unwrap_or(Val::Null))
        });
    vm.register_class(builder).expect("register Builder");

    let model = NativeClassDef::new("OpenGrid\\Database\\Model")
        .abstract_class()
        .payload(|| Box::new(ModelData::default()))
        .handlers(ObjectHandlers {
            trace: None,
            get: Some(|vm, h, name| Ok(vm.native_payload::<ModelData>(h).and_then(|m| m.attributes.get(&String::from_utf8_lossy(name).into_owned()).cloned()))),
            set: Some(|vm, h, name, v| {
                let key = String::from_utf8_lossy(name).into_owned();
                let m = vm.native_payload_mut::<ModelData>(h).expect("Model payload");
                m.attributes.insert(key.clone(), v);
                m.dirty.push(key);
                Ok(true)
            }),
            isset: Some(|vm, h, name| Ok(Some(vm.native_payload::<ModelData>(h).is_some_and(|m| m.attributes.contains_key(&String::from_utf8_lossy(name).into_owned()))))),
            unset: None,
            compare: None,
            arith: None,
            clone: None,
        })
        .method("__construct", |vm, c| {
            let this = c.this()?;
            if let Val::Array(attrs) = c.arg(0) {
                let class = vm.class_of(this);
                let fillable: Vec<String> = vm.instance_default(class, "fillable").and_then(|v| if let Val::Array(a) = v { Some(a.values().filter_map(|x| x.to_php_string().ok()).map(|s| s.to_string()).collect()) } else { None }).unwrap_or_default();
                let m = vm.native_payload_mut::<ModelData>(this).expect("Model payload");
                for (k, v) in attrs.iter() {
                    let key = match k { ArrayKey::Str(s) => s.to_string(), ArrayKey::Int(i) => i.to_string() };
                    if fillable.is_empty() || fillable.contains(&key) {
                        m.attributes.insert(key, v.clone());
                    }
                }
            }
            Ok(Val::Null)
        })
        .static_method("query", |vm, c| Ok(Val::Object(new_builder(vm, c.class)?)))
        .static_method("where", |vm, c| {
            let b = new_builder(vm, c.class)?;
            vm.call_method_sync(b, b"where", c.args)?;
            Ok(Val::Object(b))
        })
        .static_method("find", |vm, c| {
            let b = new_builder(vm, c.class)?;
            vm.call_method_sync(b, b"where", vec![Val::str("id"), c.arg(0)])?;
            vm.call_method_sync(b, b"limit", vec![Val::Int(1)])?;
            Ok(Val::Object(b))
        })
        .method("getTable", |vm, c| {
            let class = vm.class_of(c.this()?);
            Ok(vm.static_default(class, "table").unwrap_or(Val::Null))
        })
        .method("toArray", |vm, c| {
            let this = c.this()?;
            let mut a = PhpArray::new();
            for (k, v) in &vm.native_payload::<ModelData>(this).expect("Model payload").attributes {
                a.insert(ArrayKey::from_str(k), v.clone());
            }
            Ok(Val::array(a))
        })
        .method("isDirty", |vm, c| {
            let this = c.this()?;
            Ok(Val::Bool(!vm.native_payload::<ModelData>(this).expect("Model payload").dirty.is_empty()))
        })
        .method("jsonSerialize", |vm, c| {
            let this = c.this()?;
            // honour a dialect override of toArray()
            vm.call_method_sync(this, b"toArray", Vec::new())
        });
    vm.register_class(model).expect("register Model");
}

fn run(src: &str) -> String {
    let mut vm = Vm::with_prelude().expect("prelude");
    register(&mut vm);
    let r = vm.run_file("t.grd", src.as_bytes());
    let mut out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = r {
        out.push_str(&format!("ERR {e}"));
    }
    out
}

#[test]
fn dialect_model_extends_native_base() {
    let out = run(r#"<?grd
namespace App;
use OpenGrid\Database\Model;

class Task extends Model {
    protected static string $table = 'tasks';
    protected bool $softDeletes = true;
    protected array $fillable = ['title', 'done'];
    public function toArray(): array { $a = parent::toArray(); $a['extra'] = count($a); return $a; }
}
class Plain extends Model { protected static string $table = 'plains'; }

echo Task::where('done', false)->orderBy('id', 'desc')->limit(10)->toSql(), "\n";
echo Task::query()->when(true, fn($q) => $q->where('a', 1))->when(false, fn($q) => $q->where('b', 2))->select('id, title')->toSql(), "\n";
echo Plain::find(7)->toSql(), " ", Plain::find(7)->getModel(), "\n";
$t = new Task(['title' => 'x', 'secret' => 'no']);
$t->done = true;
echo $t->title, " ", isset($t->secret) ? 'y' : 'n', " ", json_encode($t->toArray()), " ", $t->getTable(), " ", $t instanceof Model ? 'y' : 'n', " ", $t->isDirty() ? 'dirty' : 'clean', "\n";
echo json_encode(Task::where('x', 'y')->getBindings()), "\n";
"#);
    assert_eq!(
        out,
        "SELECT * FROM tasks WHERE deleted_at IS NONE AND done = $f0 ORDER BY id DESC LIMIT 10 START 0\n\
         SELECT id, title FROM tasks WHERE deleted_at IS NONE AND a = $f0\n\
         SELECT * FROM plains WHERE id = $f0 LIMIT 1 START 0 App\\Plain\n\
         x n {\"title\":\"x\",\"done\":true,\"extra\":2} tasks y dirty\n\
         {\"f0\":\"y\"}\n"
    );
}

#[test]
fn native_errors_surface_as_exceptions() {
    let out = run(r#"<?grd
use OpenGrid\Database\Model;
try { new Model(); } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
class M extends Model { protected static string $table = 'm'; }
try { M::query()->limit('zehn'); } catch (TypeError $e) { echo "TypeError\n"; }
"#);
    assert_eq!(out, "Error: Cannot instantiate abstract class OpenGrid\\Database\\Model\nTypeError\n");
}


#[test]
fn profile_counts_calls_backedges_and_types() {
    let mut vm = Vm::with_prelude().expect("prelude");
    let src = b"<?grd\nfunction f($x) { return $x + 1.5; }\n$a = [1, 2]; $s = 0.0; for ($i = 0; $i < 2000; $i++) { $s = f($a[$i % 2]); }\necho $s;";
    vm.run_file("p.grd", src).expect("run");
    let hot: Vec<&grd_vm::vm::CodeProfile> = vm.profiles.iter().filter(|p| p.hot).collect();
    assert!(hot.iter().any(|p| p.calls >= 2000), "f() should be hot by calls");
    assert!(vm.profiles.iter().any(|p| p.backedges >= 2000), "the loop should count backedges");
    // JIT2b: `$a[$i % 2]` only ever read ints
    assert!(vm.profiles.iter().any(|p| p.types.iter().any(|t| *t as u64 == grd_vm::vm::jit::KIND_INT)), "element kinds at dim fetches");
}
