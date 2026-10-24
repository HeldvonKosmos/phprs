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
        assert_eq!(tables(&db).await, vec!["_opengrid_sessions", "personal_access_tokens", "products"], "framework fragment included");
        assert!(text.contains("DEFINE FIELD note ON products TYPE option<string>;"));
        assert_eq!(warnings.len(), 1, "unsupported DEFINE FUNCTION reported: {warnings:?}");

        let pulled = ops::pull(&db).await.expect("pull");
        assert!(pulled.contains("-- TABLE: products") && pulled.contains("DEFINE FIELD sku ON products TYPE string"), "{pulled}");
        assert!(!pulled.contains("__entity"), "SurrealKit tables left out");

        db.query("CREATE products:1 SET sku = 'a'").await.unwrap().check().unwrap();
        ops::fresh(&db, &root, "dev", true).await.expect("fresh keep_db");
        let mut r = db.query("SELECT * FROM products").await.unwrap();
        assert!(r.take::<Vec<serde_json::Value>>(0).unwrap().is_empty(), "data gone");
        assert_eq!(tables(&db).await, vec!["_opengrid_sessions", "personal_access_tokens", "products"], "schema back");

        db.query("CREATE products:2 SET sku = 'b'").await.unwrap().check().unwrap();
        ops::fresh(&db, &root, "dev", false).await.expect("fresh");
        let mut r = db.query("SELECT * FROM products").await.unwrap();
        assert!(r.take::<Vec<serde_json::Value>>(0).unwrap().is_empty());
        assert_eq!(tables(&db).await, vec!["_opengrid_sessions", "personal_access_tokens", "products"]);
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
        assert_eq!(tables(&prod).await, vec!["_opengrid_sessions", "orders", "personal_access_tokens", "products"]);
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
