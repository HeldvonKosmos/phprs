//! R4d: development tasks (push, pull, fresh) and the production start with
//! planned rollouts: all pending rollouts pass the preflight first (one
//! fails → nothing changes), then they run in order; a failing step rolls
//! back only that rollout and returns the error (the server exits).

use std::path::{Path, PathBuf};

use opengrid_framework::native::database::schema::kit::{self, SyncOptions};
use opengrid_framework::native::database::schema::ops;
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn project(name: &str, blueprint: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("og-ops-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    set_blueprint(&root, blueprint);
    root
}

fn set_blueprint(root: &Path, text: &str) {
    let p = root.join("modules/shop/database/schema/blueprint.surql");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

async fn open(db: &str) -> Surreal<Any> {
    let c = connect("mem://").await.unwrap();
    c.use_ns("opengrid_test").use_db(db).await.unwrap();
    c
}

async fn tables(db: &Surreal<Any>) -> Vec<String> {
    let mut r = db.query("INFO FOR DB").await.unwrap();
    let v: Option<serde_json::Value> = r.take(0).unwrap();
    let mut t: Vec<String> = v.unwrap()["tables"].as_object().unwrap().keys().filter(|k| !k.starts_with("__")).cloned().collect();
    t.sort();
    t
}

const V1: &str = "DEFINE TABLE products SCHEMAFULL PERMISSIONS NONE;\nDEFINE FIELD sku ON products TYPE string;\n";

#[test]
fn push_pull_fresh() {
    let root = project("dev", V1);
    std::fs::write(root.join("modules/shop/database/schema/manually.surql"), "DEFINE FIELD note ON products TYPE option<string>;\nDEFINE FUNCTION fn::x() { 1 };\n").unwrap();
    rt().block_on(async {
        let db = open("dev").await;
        let (text, warnings) = ops::push(&db, &root, SyncOptions::default()).await.expect("push");
        assert_eq!(tables(&db).await, vec!["_opengrid_sessions", "failed_jobs", "jobs", "personal_access_tokens", "products"], "framework fragment included");
        assert!(text.contains("DEFINE FIELD note ON products TYPE option<string>;"));
        assert_eq!(warnings.len(), 1, "unsupported DEFINE FUNCTION reported: {warnings:?}");

        let pulled = ops::pull(&db).await.expect("pull");
        assert!(pulled.contains("-- TABLE: products") && pulled.contains("DEFINE FIELD sku ON products TYPE string"), "{pulled}");
        assert!(!pulled.contains("__entity"), "SurrealKit tables left out");

        db.query("CREATE products:1 SET sku = 'a'").await.unwrap().check().unwrap();
        ops::fresh(&db, &root, "dev", true).await.expect("fresh keep_db");
        let mut r = db.query("SELECT * FROM products").await.unwrap();
        assert!(r.take::<Vec<serde_json::Value>>(0).unwrap().is_empty(), "data gone");
        assert_eq!(tables(&db).await, vec!["_opengrid_sessions", "failed_jobs", "jobs", "personal_access_tokens", "products"], "schema back");

        db.query("CREATE products:2 SET sku = 'b'").await.unwrap().check().unwrap();
        ops::fresh(&db, &root, "dev", false).await.expect("fresh");
        let mut r = db.query("SELECT * FROM products").await.unwrap();
        assert!(r.take::<Vec<serde_json::Value>>(0).unwrap().is_empty());
        assert_eq!(tables(&db).await, vec!["_opengrid_sessions", "failed_jobs", "jobs", "personal_access_tokens", "products"]);
    });
    std::fs::remove_dir_all(&root).unwrap();
}

/// Plan two rollouts on a development database, as a developer would.
async fn plan_two(root: &Path, second: &str) -> (String, String) {
    let dev = open("planning").await;
    ops::push(&dev, root, SyncOptions::default()).await.expect("push v1");
    let (a, _) = kit::rollout_plan(&dev, root, "init").await.expect("plan init");
    set_blueprint(root, &format!("{V1}{second}"));
    ops::push(&dev, root, SyncOptions::default()).await.expect("push v2");
    std::thread::sleep(std::time::Duration::from_millis(1100)); // ids have second resolution
    let (b, _) = kit::rollout_plan(&dev, root, "extend").await.expect("plan extend");
    (a, b)
}

#[test]
fn production_start_runs_planned_rollouts() {
    let root = project("prod-ok", V1);
    rt().block_on(async {
        let prod = open("prod").await;
        assert_eq!(ops::apply_planned(&prod, &root).await.unwrap().applied, Vec::<String>::new(), "no rollouts: nothing to do");

        let (a, b) = plan_two(&root, "DEFINE TABLE orders SCHEMAFULL PERMISSIONS NONE;\n").await;
        let report = ops::apply_planned(&prod, &root).await.expect("start");
        assert_eq!(report.applied, vec![a, b]);
        assert_eq!(tables(&prod).await, vec!["_opengrid_sessions", "failed_jobs", "jobs", "orders", "personal_access_tokens", "products"]);
        // a second start finds nothing pending
        assert!(ops::apply_planned(&prod, &root).await.unwrap().applied.is_empty());
    });
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn production_start_refuses_a_failing_preflight_without_changes() {
    let root = project("prod-preflight", V1);
    rt().block_on(async {
        let (_, b) = plan_two(&root, "DEFINE TABLE orders SCHEMAFULL PERMISSIONS NONE;\n").await;
        // the second manifest is edited by hand to change data
        let path = root.join("database/rollouts").join(format!("{b}.toml"));
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("\n[[steps]]\nid = \"wipe\"\nphase = \"complete\"\nkind = \"run_sql\"\nsql = \"DELETE products;\"\n");
        std::fs::write(&path, text).unwrap();

        let prod = open("prod").await;
        let e = ops::apply_planned(&prod, &root).await.unwrap_err();
        assert!(e.0.contains("nothing changed") && e.0.contains("`wipe`"), "{e}");
        assert!(tables(&prod).await.is_empty(), "not even the first rollout ran");
    });
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn production_start_rolls_back_a_failing_rollout() {
    let root = project("prod-fail", V1);
    rt().block_on(async {
        let (a, b) = plan_two(&root, "DEFINE INDEX sku_unique ON products FIELDS sku UNIQUE;\n").await;
        let prod = open("prod").await;
        // production already has the first rollout and data that the second cannot index
        kit::rollout_start(&prod, &root, Some(&a)).await.unwrap();
        kit::rollout_complete(&prod, &root, Some(&a)).await.unwrap();
        prod.query("CREATE products:1 SET sku = 'dup'; CREATE products:2 SET sku = 'dup';").await.unwrap().check().unwrap();

        let e = ops::apply_planned(&prod, &root).await.unwrap_err();
        assert!(e.0.contains(&b) && e.0.contains("rolled back"), "{e}");
        let mut r = prod.query("INFO FOR TABLE products").await.unwrap();
        let info: Option<serde_json::Value> = r.take(0).unwrap();
        assert!(info.unwrap()["indexes"].as_object().unwrap().is_empty(), "index removed again");
        let mut r = prod.query("SELECT VALUE status FROM __rollout ORDER BY id").await.unwrap();
        assert_eq!(r.take::<Vec<String>>(0).unwrap(), vec!["completed", "rolled_back"]);
        let mut r = prod.query("SELECT VALUE sku FROM products").await.unwrap();
        assert_eq!(r.take::<Vec<String>>(0).unwrap().len(), 2, "data untouched");
    });
    std::fs::remove_dir_all(&root).unwrap();
}

/// Regression (CLI smoke test 24.10.): planning reads the module sources
/// directly (no push needed), and a plan without a change is refused instead
/// of leaving an empty manifest that the preflight misread as "everything
/// removed".
#[test]
fn plan_from_sources_and_refuse_empty_plans() {
    let root = project("plan", V1);
    rt().block_on(async {
        let dev = open("plan").await;
        ops::push(&dev, &root, SyncOptions::default()).await.unwrap();
        let (_, r) = ops::plan(&dev, &root, "init").await.expect("first plan");
        assert!(r.passed());
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let e = ops::plan(&dev, &root, "nothing").await.unwrap_err();
        assert!(e.0.contains("has not changed"), "{e}");
        assert_eq!(kit::manifests(&root).len(), 1, "empty plan discarded");

        // edit the module schema and plan right away, without migrate_push
        set_blueprint(&root, &format!("{V1}DEFINE FIELD archived ON products TYPE option<bool>;\n"));
        let (_, r) = ops::plan(&dev, &root, "archived").await.expect("plan from sources");
        assert!(r.passed() && r.additions.contains(&"field products.archived".to_string()), "{}", r.summary());
    });
    std::fs::remove_dir_all(&root).unwrap();
}

/// TEST2.6: real `INFO FOR` answers of an embedded SurrealDB instead of
/// invented ones: every way of writing a field type comes back in
/// SurrealDB's own form, the model reads the type the definition meant, and
/// the pulled schema is a fixed point (applied to an empty database and
/// pulled again, it stays the same).
#[test]
fn real_info_answers_become_the_model() {
    use indexmap::IndexMap;
    use opengrid_framework::native::database::schema::dbinfo::{TableInfo, from_info};
    use opengrid_framework::native::database::value::base_type;
    const SCHEMA: &str = "DEFINE TABLE a SCHEMAFULL PERMISSIONS NONE;\nDEFINE TABLE b SCHEMALESS PERMISSIONS NONE;\nDEFINE TABLE t SCHEMAFULL PERMISSIONS NONE;\n\
        DEFINE FIELD n ON t TYPE int;\nDEFINE FIELD o ON t TYPE option<int>;\nDEFINE FIELD u ON t TYPE none | int;\nDEFINE FIELD nl ON t TYPE null | datetime;\n\
        DEFINE FIELD r ON t TYPE option<record<a | b>>;\nDEFINE FIELD l ON t TYPE array<string>;\nDEFINE FIELD f ON t TYPE object FLEXIBLE;\n\
        DEFINE FIELD d ON t TYPE decimal DEFAULT 0dec;\nDEFINE FIELD s ON t TYPE string ASSERT string::len($value) > 0;\nDEFINE FIELD m ON t TYPE int | string;\n\
        DEFINE FIELD g ON t TYPE option<set<record<a>>>;\nDEFINE FIELD k ON t TYPE array<int, 3>;\n\
        DEFINE INDEX t_s ON t FIELDS s UNIQUE;\nDEFINE INDEX t_nr ON t FIELDS n, r;\n";
    // field → the base type its definition means
    const MEANT: &[(&str, &str)] = &[("n", "int"), ("o", "int"), ("u", "int"), ("nl", "datetime"), ("r", "record<a | b>"), ("l", "array<string>"), ("f", "object"), ("d", "decimal"), ("s", "string"), ("m", "int | string")];
    rt().block_on(async {
        let db = open("real_info").await;
        db.query(SCHEMA).await.unwrap().check().expect("schema applies");
        let mut r = db.query("INFO FOR DB").await.unwrap();
        let info: Option<serde_json::Value> = r.take(0).unwrap();
        let db_tables: Vec<(String, String)> = info.unwrap()["tables"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        let mut tables = IndexMap::new();
        for (name, _) in &db_tables {
            let mut r = db.query(format!("INFO FOR TABLE {name}")).await.unwrap();
            let ti: Option<serde_json::Value> = r.take(0).unwrap();
            let ti = ti.unwrap();
            let list = |key: &str| ti[key].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect()).unwrap_or_default();
            tables.insert(name.clone(), TableInfo { fields: list("fields"), indexes: list("indexes"), events: list("events") });
        }
        let model = from_info(&db_tables, &tables);
        let wrong: Vec<String> = MEANT
            .iter()
            .filter_map(|(f, meant)| {
                let ty = model.field_type("t", f);
                let base = ty.as_deref().map(base_type);
                (base.as_deref() != Some(*meant)).then(|| format!("{f}: INFO type {ty:?} → base {base:?}, meant {meant}"))
            })
            .collect();
        assert!(wrong.is_empty(), "types read from real INFO answers:\n{}\n(INFO: {:?})", wrong.join("\n"), tables["t"].fields);
        // pull → apply to an empty database → pull again: the same text
        let first = ops::pull(&db).await.expect("pull");
        let fresh = open("real_info_again").await;
        fresh.query(&first).await.unwrap().check().expect("pulled schema applies");
        let second = ops::pull(&fresh).await.expect("pull again");
        assert_eq!(first, second, "pull is not a fixed point");
        // the element fields SurrealDB defines itself are left out
        assert!(!first.contains("l.*") && !first.contains("g.*") && !first.contains("k.*") && first.contains("DEFINE FIELD k ON t TYPE array<int, 3>"), "{first}");
    });
}
