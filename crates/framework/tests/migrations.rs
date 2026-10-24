//! Schema builder and migration runner against `mem://`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::native::database::migrator;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::{Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool(db: &str) -> Arc<Pool> {
    Arc::new(Pool::start(PoolOptions::new(root()).workers(1).database(DbConfig::mem("test", db))).expect("pool starts"))
}

fn run(pool: &Pool, src: &'static str) -> String {
    pool.run_sync(move |vm| {
        let r = vm.run_file("t.grd", src.as_bytes());
        let mut out = String::from_utf8_lossy(&vm.output).into_owned();
        if let Err(e) = r {
            out.push_str(&format!("ERR {e}"));
        }
        vm.reset_request();
        out
    })
    .expect("worker answers")
}

#[test]
fn migrate_status_rollback() {
    let pool = pool("mig");
    let r = pool.run_sync(|vm| migrator::migrate(vm, &root()).map_err(|t| t.message)).unwrap().expect("migrate");
    assert_eq!(r.executed, vec!["2026_10_24_000001_create_tasks", "2026_10_24_000002_add_task_notes", "2026_10_24_000003_create_comments"]);

    let r = pool.run_sync(|vm| migrator::status(vm, &root()).map_err(|t| t.message)).unwrap().expect("status");
    assert_eq!(r.ran.len(), 3);
    assert!(r.pending.is_empty());
    assert_eq!(r.ran[0].1, 1, "both in batch 1");

    let out = run(
        &pool,
        r#"<?grd
use OpenGrid\Database\Schema;
use Tasks\Models\Task;
echo Schema::hasTable('tasks') ? 'table' : 'none', ' ', Schema::hasColumn('tasks', 'notes') ? 'notes' : 'no-notes', ' ', Schema::hasColumn('tasks', 'nope') ? 'x' : 'ok', "\n";
Task::create(['title' => 'Unique']);
try { Task::create(['title' => 'Unique']); echo "dup allowed\n"; } catch (\RuntimeException $e) { echo "unique enforced\n"; }
try { Task::create(['title' => 'Typed', 'priority' => 'high']); echo "type ignored\n"; } catch (\RuntimeException $e) { echo "type enforced\n"; }
echo Task::count(), "\n";
"#,
    );
    assert_eq!(out, "table notes ok\nunique enforced\ntype enforced\n1\n");

    // second run: nothing to do
    let r = pool.run_sync(|vm| migrator::migrate(vm, &root()).map_err(|t| t.message)).unwrap().expect("migrate again");
    assert!(r.executed.is_empty());

    let r = pool.run_sync(|vm| migrator::rollback(vm, &root(), 1).map_err(|t| t.message)).unwrap().expect("rollback");
    assert_eq!(r.executed, vec!["2026_10_24_000003_create_comments", "2026_10_24_000002_add_task_notes", "2026_10_24_000001_create_tasks"], "one batch holds all");
    assert_eq!(r.pending.len(), 3);
    let out = run(&pool, "<?grd\nuse OpenGrid\\Database\\Schema;\necho Schema::hasTable('tasks') ? 'table' : 'none', \"\\n\";");
    assert_eq!(out, "none\n");

    // migrate once more; the classes are already loaded in this worker
    let r = pool.run_sync(|vm| migrator::migrate(vm, &root()).map_err(|t| t.message)).unwrap().expect("migrate third");
    assert_eq!(r.executed.len(), 3);
    assert_eq!(r.ran.iter().map(|(_, b)| *b).max(), Some(1), "table was empty again, numbering restarts");
}

#[test]
fn schema_sql_from_the_dialect() {
    let pool = pool("sql");
    let out = run(
        &pool,
        r#"<?grd
use OpenGrid\Database\Blueprint;
use OpenGrid\Database\Schema;
$sql = Schema::sql('clients', function (Blueprint $t) {
    $t->id();
    $t->string('name')->unique();
    $t->record('owner_id', 'users')->nullable();
    $t->array('tags', 'string');
    $t->decimal('rate')->default(0.5)->assert('$value >= 0');
    $t->schemafull();
});
echo implode("\n", $sql), "\n";
"#,
    );
    assert_eq!(
        out,
        "DEFINE TABLE IF NOT EXISTS clients SCHEMAFULL\n\
         DEFINE FIELD IF NOT EXISTS name ON TABLE clients TYPE string\n\
         DEFINE INDEX IF NOT EXISTS clients_name_unique ON TABLE clients FIELDS name UNIQUE\n\
         DEFINE FIELD IF NOT EXISTS owner_id ON TABLE clients TYPE option<record<users>>\n\
         DEFINE FIELD IF NOT EXISTS tags ON TABLE clients TYPE array<string>\n\
         DEFINE FIELD IF NOT EXISTS rate ON TABLE clients TYPE decimal DEFAULT 0.5f ASSERT $value >= 0\n"
    );
}
