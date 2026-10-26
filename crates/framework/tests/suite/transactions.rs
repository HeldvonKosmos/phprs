//! TX1 (D12): `DB::transaction($fn, $attempts)` runs the closure again on a
//! retriable write conflict, like Laravel's attempts. The check is a lost
//! update: workers read a counter and write it back increased, each inside a
//! transaction; with retries every increment must be there at the end.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::native::database::schema::kit::SyncOptions;
use opengrid_framework::native::database::schema::ops;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::{Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool(cfg: DbConfig, workers: usize) -> Arc<Pool> {
    let pool = Arc::new(Pool::start(PoolOptions::new(root()).workers(workers).database(cfg)).expect("pool starts"));
    let d = pool.db().expect("database");
    d.block_on(ops::push(d.client(), &root(), SyncOptions::default())).expect("schema push");
    pool
}

fn run(pool: &Pool, src: &str) -> String {
    let src = src.to_string();
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

const INCREMENT: &str = r#"<?grd
use OpenGrid\Database\DB;
DB::transaction(function (): void {
    $rows = DB::select('SELECT n FROM counter:1');
    $n = (int) $rows[0]['n'];
    DB::statement('UPDATE counter:1 SET n = $n', ['n' => $n + 1]);
}, 200);
echo 'ok';
"#;

fn no_lost_updates(pool: Arc<Pool>) {
    assert_eq!(run(&pool, "<?grd\nuse OpenGrid\\Database\\DB;\nDB::statement('CREATE counter:1 SET n = 0'); echo 'ok';"), "ok");
    let (threads, each) = (6, 20);
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let pool = pool.clone();
            std::thread::spawn(move || (0..each).map(|_| run(&pool, INCREMENT)).filter(|o| o != "ok").collect::<Vec<_>>())
        })
        .collect();
    let failed: Vec<String> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
    assert!(failed.is_empty(), "{} transactions failed, first: {:?}", failed.len(), failed.first());
    let n = run(&pool, "<?grd\nuse OpenGrid\\Database\\DB;\necho DB::select('SELECT n FROM counter:1')[0]['n'];");
    assert_eq!(n, (threads * each).to_string(), "lost updates");
    // the closures really collided and ran again (else the test shows nothing)
    let retries = pool.db().expect("database").conflict_retries();
    assert!(retries > 0, "no write conflict occurred, the retry path did not run");
}

#[test]
fn transactions_retry_conflicts_embedded() {
    let dir = std::env::temp_dir().join(format!("og-tx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = DbConfig { url: format!("surrealkv://{}", dir.display()), ..DbConfig::mem("tx", "tx") };
    no_lost_updates(pool(cfg, 4));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transactions_retry_conflicts_on_a_server() {
    let Some(server) = crate::server::TestServer::start() else { return };
    no_lost_updates(pool(server.config("tx"), 4));
}

#[test]
fn transaction_arguments() {
    let pool = pool(DbConfig::mem("tx", "args"), 1);
    let out = run(&pool, r#"<?grd
use OpenGrid\Database\DB;
// Laravel: no attempt, the closure does not run, null comes back
var_dump(DB::transaction(function (): int { echo 'ran '; return 1; }, 0));
try { DB::transaction(fn () => 1, '2'); } catch (TypeError $e) { echo 'string rejected '; }
echo DB::transaction(fn () => DB::transaction(fn () => 'nested'), 3), ' ';
try { DB::transaction(function (): void { echo 'try '; throw new RuntimeException('boom'); }, 5); } catch (RuntimeException $e) { echo $e->getMessage(); }
"#);
    assert_eq!(out, "NULL\nstring rejected nested try boom");
}
