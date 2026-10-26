//! B2c (D12): the database layer against a real SurrealDB server (ws://), the
//! path production uses when the database is not embedded. Uses the
//! throw-away server of `server.rs`; skipped without a `surreal` binary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::native::database::schema::kit::SyncOptions;
use opengrid_framework::native::database::schema::ops;
use opengrid_framework::{Pool, PoolOptions};

use crate::server::TestServer;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool(server: &TestServer, db: &str, workers: usize) -> Arc<Pool> {
    let pool = Arc::new(Pool::start(PoolOptions::new(root()).workers(workers).database(server.config(db))).expect("pool starts"));
    let d = pool.db().expect("database");
    d.block_on(ops::push(d.client(), &root(), SyncOptions::default())).expect("schema push");
    pool
}

fn run(pool: &Pool, src: String) -> String {
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

/// The server prints `option<int>` as `none | int` in INFO FOR TABLE; the
/// filter cast reads that form, so a query-string value `'2'` matches ints.
#[test]
fn typed_filters_read_the_servers_field_types() {
    let Some(server) = TestServer::start() else { return };
    let pool = pool(&server, "filters", 1);
    let out = run(&pool, r#"<?grd
use OpenGrid\Database\DB;
use Tasks\Models\Task;
foreach ([1, 2, 2] as $i => $p) { Task::create(['title' => 't' . $i, 'priority' => $p]); }
$info = DB::query('INFO FOR TABLE tasks');
echo str_contains(json_encode($info), 'TYPE none | int') ? 'server-form ' : 'other-form ', Task::query()->filter(['priority' => '2'])->count();
"#.to_string());
    assert_eq!(out, "server-form 2");
}

/// CRUD, soft delete and pagination over the server connection.
#[test]
fn crud_and_soft_delete_over_ws() {
    let Some(server) = TestServer::start() else { return };
    let pool = pool(&server, "crud", 1);
    let out = run(&pool, r#"<?grd
use Tasks\Models\Task;
$a = Task::create(['title' => 'Alpha', 'priority' => 1]);
Task::create(['title' => 'Beta', 'priority' => 2]);
$a->update(['done' => true]);
echo Task::find($a->id)->done ? 'updated ' : 'stale ';
$a->delete();
echo Task::find($a->id) === null ? 'gone ' : 'visible ', Task::query()->withTrashed()->count(), ' ', Task::query()->count(), ' ';
$p = Task::query()->paginate(1, 1);
echo $p['total'], '/', $p['last_page'];
"#.to_string());
    assert_eq!(out, "updated gone 2 1 1/1");
}

/// Concurrent writes to one record from several workers (as concurrent
/// requests with one token do) all succeed over the server connection too.
#[test]
fn concurrent_writes_to_one_record_over_ws() {
    let Some(server) = TestServer::start() else { return };
    let pool = pool(&server, "conflicts", 4);
    assert_eq!(run(&pool, "<?grd\nuse OpenGrid\\Database\\DB;\nDB::statement('CREATE counter:1 SET n = 0'); echo 'ok';".into()), "ok");
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let pool = pool.clone();
            std::thread::spawn(move || {
                (0..25)
                    .map(|i| run(&pool, format!("<?grd\nuse OpenGrid\\Database\\DB;\nDB::statement('UPDATE counter:1 MERGE {{ n: {} }}'); echo 'ok';", t * 100 + i)))
                    .filter(|o| o != "ok")
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let failed: Vec<String> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
    assert!(failed.is_empty(), "{} writes failed, first: {:?}", failed.len(), failed.first());
    // the record holds the last write of one of the threads
    let n = run(&pool, "<?grd\nuse OpenGrid\\Database\\DB;\necho DB::select('SELECT n FROM counter:1')[0]['n'];".into());
    assert!((0..8).any(|t| n == (t * 100 + 24).to_string()), "final value {n} is not a thread's last write");
    // conflicts are counted where they are retried; over the server the
    // writes may also simply queue, so only report the number
    eprintln!("concurrent_writes_to_one_record_over_ws: {} conflicts retried", pool.db().expect("database").conflict_retries());
}
