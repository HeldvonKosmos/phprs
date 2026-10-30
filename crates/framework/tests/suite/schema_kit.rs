//! R4c: SurrealKit embedded (DECISIONS D4, step 3): sync against mem:// and
//! surrealkv://, a second sync changes nothing, prune touches only managed
//! objects, rollout plan/start/complete/rollback against a persistent
//! embedded database, and `rollout start` without a baseline answers with a
//! hint instead of failing obscurely.

use std::path::PathBuf;

use opengrid_framework::native::database::schema::kit::{self, SyncOptions};
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("og-kit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

async fn open(url: &str) -> Surreal<Any> {
    let db = connect(url).await.expect("connect");
    db.use_ns("opengrid_test").use_db("kit").await.unwrap();
    db
}

async fn tables(db: &Surreal<Any>) -> Vec<String> {
    let mut r = db.query("INFO FOR DB").await.unwrap();
    let v: Option<serde_json::Value> = r.take(0).unwrap();
    let mut t: Vec<String> = v.unwrap()["tables"].as_object().unwrap().keys().filter(|k| !k.starts_with("__")).cloned().collect();
    t.sort();
    t
}

async fn managed(db: &Surreal<Any>) -> String {
    let mut r = db.query("SELECT key, val FROM __entity WHERE ns = 'schema' ORDER BY key").await.unwrap();
    let v: Vec<serde_json::Value> = r.take(0).unwrap();
    serde_json::to_string(&v).unwrap()
}

const V1: &str = "DEFINE TABLE tasks SCHEMAFULL PERMISSIONS NONE;\nDEFINE FIELD title ON tasks TYPE string;\nDEFINE TABLE notes SCHEMALESS PERMISSIONS NONE;\n";

#[test]
fn sync_mem_is_idempotent_and_prunes_only_managed() {
    let root = tmp("mem");
    rt().block_on(async {
        let db = open("mem://").await;
        kit::sync(&db, &root, V1, SyncOptions::default()).await.expect("first sync");
        assert_eq!(tables(&db).await, vec!["notes", "tasks"]);
        assert_eq!(std::fs::read_to_string(root.join(kit::TEMPORARY)).unwrap(), V1, "temporary.surql written");

        let before = managed(&db).await;
        db.query("CREATE tasks:1 SET title = 'keep'").await.unwrap().check().unwrap();
        kit::sync(&db, &root, V1, SyncOptions::default()).await.expect("second sync");
        assert_eq!(managed(&db).await, before, "second sync changes nothing");
        let mut r = db.query("SELECT VALUE title FROM tasks").await.unwrap();
        assert_eq!(r.take::<Vec<String>>(0).unwrap(), vec!["keep"]);

        // a table SurrealKit does not manage survives a prune; a managed one goes
        db.query("DEFINE TABLE foreign_tbl SCHEMALESS").await.unwrap().check().unwrap();
        let v2 = "DEFINE TABLE tasks SCHEMAFULL PERMISSIONS NONE;\nDEFINE FIELD title ON tasks TYPE string;\n";
        kit::sync(&db, &root, v2, SyncOptions::default()).await.expect("prune sync");
        assert_eq!(tables(&db).await, vec!["foreign_tbl", "tasks"]);

        // an empty schema does not prune everything by accident
        let e = kit::sync(&db, &root, "  \n", SyncOptions::default()).await.unwrap_err();
        assert!(e.0.contains("empty"), "{e}");
    });
    std::fs::remove_dir_all(&root).unwrap();
}

/// Persistence across a process restart, the way it happens in practice.
/// A surrealkv datastore keeps its file lock for the life of the process
/// (finding 24.10.: dropping client and runtime does not release it), so
/// both halves run as child processes of this test binary: first one syncs,
/// then a second one reopens the file and reads the schema.
#[test]
fn sync_surrealkv_persists() {
    let root = tmp("kv");
    let url = format!("surrealkv://{}", root.join("data").display());
    for mode in ["sync", "read"] {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "schema_kit::surrealkv_child", "--ignored", "--nocapture"])
            .env("OG_KV_URL", &url)
            .env("OG_KV_ROOT", &root)
            .env("OG_KV_MODE", mode)
            .output()
            .expect("child runs");
        let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && text.contains("1 passed"), "child `{mode}` failed:\n{text}");
    }
    std::fs::remove_dir_all(&root).unwrap();
}

/// Child half of `sync_surrealkv_persists`; ignored unless run by it.
#[test]
#[ignore = "runs as child process of sync_surrealkv_persists"]
fn surrealkv_child() {
    let (Ok(url), Ok(root), Ok(mode)) = (std::env::var("OG_KV_URL"), std::env::var("OG_KV_ROOT"), std::env::var("OG_KV_MODE")) else { return };
    rt().block_on(async {
        let db = open(&url).await;
        if mode == "sync" {
            kit::sync(&db, std::path::Path::new(&root), V1, SyncOptions::default()).await.expect("sync");
        } else {
            assert_eq!(tables(&db).await, vec!["notes", "tasks"], "schema survives a restart");
        }
    });
}

/// A preflight message with its rollout id (`<timestamp>__<name>`) as `<id>`.
fn masked(msg: &str, name: &str) -> String {
    let id = msg.strip_prefix("preflight ").and_then(|r| r.split(':').next()).unwrap_or_else(|| panic!("not a preflight message: {msg}"));
    assert!(id.ends_with(&format!("__{name}")), "rollout id {id}");
    msg.replace(id, "<id>")
}

fn with_table(extra: &str) -> String {
    format!("{V1}{extra}")
}

#[test]
fn rollout_cycle_on_persistent_embedded_db() {
    let root = tmp("rollout");
    let url = format!("surrealkv://{}", root.join("data").display());
    rt().block_on(async {
        let db = open(&url).await;
        kit::sync(&db, &root, V1, SyncOptions::default()).await.expect("sync");
        kit::rollout_baseline(&db, &root).await.expect("baseline");

        // plan, start (expand), complete
        kit::write_temporary(&root, &with_table("DEFINE TABLE invoices SCHEMAFULL PERMISSIONS NONE;\n")).unwrap();
        let (id, report) = kit::rollout_plan(&db, &root, "add-invoices").await.expect("plan");
        assert!(report.passed() && report.additions.contains(&"table invoices".to_string()), "{}", report.summary());
        assert!(id.ends_with("__add_invoices"), "{id}");
        assert!(root.join("database/rollouts").join(format!("{id}.toml")).is_file());
        kit::rollout_start(&db, &root, Some(&id)).await.expect("start");
        assert!(tables(&db).await.contains(&"invoices".to_string()), "expanded");
        kit::rollout_complete(&db, &root, Some(&id)).await.expect("complete");

        // second rollout, rolled back
        kit::write_temporary(&root, &with_table("DEFINE TABLE invoices SCHEMAFULL PERMISSIONS NONE;\nDEFINE TABLE payments SCHEMAFULL PERMISSIONS NONE;\n")).unwrap();
        // manifest ids carry a second-resolution timestamp
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let (id2, _) = kit::rollout_plan(&db, &root, "add-payments").await.expect("plan 2");
        kit::rollout_start(&db, &root, Some(&id2)).await.expect("start 2");
        assert!(tables(&db).await.contains(&"payments".to_string()));
        kit::rollout_rollback(&db, &root, Some(&id2)).await.expect("rollback");
        assert_eq!(tables(&db).await, vec!["invoices", "notes", "tasks"], "payments removed again");

        let mut r = db.query("SELECT VALUE status FROM __rollout ORDER BY id").await.unwrap();
        assert_eq!(r.take::<Vec<String>>(0).unwrap(), vec!["completed", "rolled_back"]);
    });
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn rollout_start_without_baseline_says_what_to_do() {
    let root = tmp("nobase");
    rt().block_on(async {
        let db = open("mem://").await;
        kit::sync(&db, &root, V1, SyncOptions::default()).await.expect("sync");
        kit::write_temporary(&root, &with_table("DEFINE TABLE extra SCHEMALESS;\n")).unwrap();
        let (id, _) = kit::rollout_plan(&db, &root, "x").await.expect("plan");
        let e = kit::rollout_start(&db, &root, Some(&id)).await.unwrap_err();
        assert!(e.0.contains("run `rollout baseline` first"), "{e}");
    });
    std::fs::remove_dir_all(&root).unwrap();
}


/// Rollouts may only extend the schema (Jonas 24.10.): the preflight check
/// discards a plan that removes or redefines an existing object or adds a
/// required field to an existing table, and refuses to start a manifest that
/// was edited to remove objects or change data.
#[test]
fn preflight_allows_only_additions() {
    let root = tmp("preflight");
    let url = format!("surrealkv://{}", root.join("data").display());
    rt().block_on(async {
        let db = open(&url).await;
        kit::sync(&db, &root, V1, SyncOptions::default()).await.expect("sync");
        kit::rollout_baseline(&db, &root).await.expect("baseline");
        let planned = |root: &std::path::Path| kit::manifests(root).len();

        // removing a table
        kit::write_temporary(&root, "DEFINE TABLE tasks SCHEMAFULL PERMISSIONS NONE;\nDEFINE FIELD title ON tasks TYPE string;\n").unwrap();
        let e = kit::rollout_plan(&db, &root, "drop-notes").await.unwrap_err();
        assert_eq!(
            masked(&e.0, "drop_notes"),
            "preflight <id>: FAILED\n  ✗ step `remove_legacy_entities` (remove_entities, phase complete) changes existing objects or data; only additions are allowed\n  ✗ table notes exists and would be removed\nrollout <id> was discarded; rollouts may only add tables, fields, indexes and events"
        );
        assert_eq!(planned(&root), 0, "rejected plan is discarded");

        // changing an existing field
        kit::write_temporary(&root, &V1.replace("TYPE string", "TYPE int")).unwrap();
        let e = kit::rollout_plan(&db, &root, "retype").await.unwrap_err();
        // SurrealKit's own refusal (its text, first lines)
        assert!(
            e.0.starts_with("rollout plan: rollout plan found 1 modified entity and will not plan it automatically without --allow-modified:\n  - field:tasks.title (schema/temporary.surql)\n"),
            "{e}"
        );
        assert_eq!(planned(&root), 0);

        // a new required field on an existing table
        kit::write_temporary(&root, &with_table("DEFINE FIELD owner ON tasks TYPE string;\n")).unwrap();
        let e = kit::rollout_plan(&db, &root, "owner").await.unwrap_err();
        assert_eq!(
            masked(&e.0, "owner"),
            "preflight <id>: FAILED\n  ✗ field tasks.owner is a new required field on an existing table (existing rows lack it); make it option<…> or give it a DEFAULT: DEFINE FIELD owner ON tasks TYPE string PERMISSIONS FULL\n  + field tasks.owner\nrollout <id> was discarded; rollouts may only add tables, fields, indexes and events"
        );
        assert_eq!(planned(&root), 0);

        // an optional field and a new table pass
        kit::write_temporary(&root, &with_table("DEFINE FIELD owner ON tasks TYPE option<string>;\nDEFINE TABLE labels SCHEMALESS PERMISSIONS NONE;\n")).unwrap();
        let (id, report) = kit::rollout_plan(&db, &root, "owner-optional").await.expect("additive plan passes");
        assert!(report.additions.contains(&"field tasks.owner".to_string()) && report.additions.contains(&"table labels".to_string()), "{}", report.summary());

        // a frozen target schema edited by hand to retype an existing field is refused at start
        let frozen = root.join("database/rollouts").join(&id).join("schema/temporary.surql");
        let original = std::fs::read_to_string(&frozen).unwrap();
        let _ = std::process::Command::new("chmod").arg("-R").arg("u+w").arg(root.join("database/rollouts")).status();
        std::fs::write(&frozen, original.replace("DEFINE FIELD title ON tasks TYPE string", "DEFINE FIELD title ON tasks TYPE int")).unwrap();
        let e = kit::rollout_start(&db, &root, Some(&id)).await.unwrap_err();
        assert!(e.0.contains("field tasks.title exists and would change"), "{e}");
        std::fs::write(&frozen, original).unwrap();

        // a manifest edited by hand to change data is refused at start
        let path = root.join("database/rollouts").join(format!("{id}.toml"));
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("\n[[steps]]\nid = \"wipe\"\nphase = \"complete\"\nkind = \"run_sql\"\nsql = \"DELETE tasks;\"\n");
        let _ = std::process::Command::new("chmod").arg("u+w").arg(&path).status();
        std::fs::write(&path, text).unwrap();
        let e = kit::rollout_start(&db, &root, Some(&id)).await.unwrap_err();
        assert!(e.0.contains("`wipe`"), "{e}");
        assert!(!tables(&db).await.contains(&"labels".to_string()), "nothing applied");
    });
    std::fs::remove_dir_all(&root).unwrap();
}
