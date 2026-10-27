//! D23 (DBR1): raw SQL on SQLite through `DB::connection('sqlite')`, after
//! Laravel's connection API: types, bindings, affected rows, RETURNING,
//! transactions, errors, the pragmas of every connection, one connection
//! per worker thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::{Pool, PoolOptions};

fn pool(name: &str) -> (Arc<Pool>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("opengrid-sqlite-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let db = dir.join("bench.sqlite");
    let settings = serde_json::json!({ "opengrid": { "connections": { "sqlite": { "url": format!("sqlite://{}", db.display()) } } } });
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app");
    let opts = PoolOptions::new(root).workers(2).settings(settings);
    (Arc::new(Pool::start(opts).expect("pool starts")), dir)
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
fn raw_sql_on_sqlite_with_typed_values_and_bindings() {
    let (pool, dir) = pool("types");
    let out = run(
        &pool,
        r#"<?grd
use OpenGrid\Database\DB;
$db = DB::connection('sqlite');
$db->statement('CREATE TABLE notes (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL, done INTEGER NOT NULL, score REAL, body TEXT)');
$db->statement('CREATE INDEX notes_done ON notes (done)');
var_dump($db->insert('INSERT INTO notes (title, done, score, body) VALUES (?, ?, ?, ?)', ['a', false, 1.5, null]));
$id = $db->select('INSERT INTO notes (title, done, score, body) VALUES (:t, :d, :s, :b) RETURNING id', ['t' => 'b', 'd' => true, ':s' => 2, 'b' => 'x'])[0]['id'];
var_dump($id);
$rows = $db->select('SELECT id, title, done, score, body FROM notes ORDER BY id');
var_dump($rows[0], $rows[1]['done'], $rows[1]['score']);
var_dump($db->update('UPDATE notes SET done = ? WHERE done = ?', [1, 0]), $db->delete('DELETE FROM notes WHERE id = ?', [$id]));
echo json_encode($db->select('SELECT count(*) AS n, typeof(score) AS t FROM notes')), "\n";
echo $db->select('PRAGMA journal_mode')[0]['journal_mode'], ' ', $db->select('PRAGMA synchronous')[0]['synchronous'], ' ', $db->select('PRAGMA busy_timeout')[0]['timeout'], "\n";
echo json_encode($db->select('SELECT 1 AS a, 2 AS b, 3 AS a, 4 AS "7"')), ' ', json_encode($db->select('SELECT title, done FROM notes UNION ALL SELECT \'z\', 9')), "\n";
try { $db->select('SELECT nope FROM notes'); } catch (RuntimeException $e) { echo get_class($e), ': ', $e->getMessage(), "\n"; }
try { $db->select('SELECT ?', [[1]]); } catch (TypeError $e) { echo 'TypeError: ', $e->getMessage(), "\n"; }
"#,
    );
    assert_eq!(
        out,
        "bool(true)\nint(2)\narray(5) {\n  [\"id\"]=>\n  int(1)\n  [\"title\"]=>\n  string(1) \"a\"\n  [\"done\"]=>\n  int(0)\n  [\"score\"]=>\n  float(1.5)\n  [\"body\"]=>\n  NULL\n}\nint(1)\nfloat(2)\nint(1)\nint(1)\n[{\"n\":1,\"t\":\"real\"}]\nwal 2 5000\n[{\"a\":3,\"b\":2,\"7\":4}] [{\"title\":\"a\",\"done\":1},{\"title\":\"z\",\"done\":9}]\nRuntimeException: SQLite: no such column: nope\nTypeError: SQLite binding must be int, float, string, bool or null, array given\n"
    );
    drop(pool);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn transactions_commit_and_roll_back() {
    let (pool, dir) = pool("txn");
    let out = run(
        &pool,
        r#"<?grd
use OpenGrid\Database\DB;
$db = DB::connection('sqlite');
$db->statement('CREATE TABLE t (n INTEGER)');
echo $db->transaction(function () use ($db): string { $db->insert('INSERT INTO t VALUES (1)'); return 'committed'; }), "\n";
try {
    $db->transaction(function () use ($db): void { $db->insert('INSERT INTO t VALUES (2)'); throw new RuntimeException('boom'); });
} catch (RuntimeException $e) { echo 'rolled back: ', $e->getMessage(), "\n"; }
echo json_encode($db->select('SELECT n FROM t')), ' ', $db->getName(), "\n";
try { DB::connection('other'); } catch (Error $e) { echo $e->getMessage(), "\n"; }
"#,
    );
    assert_eq!(out, "committed\nrolled back: boom\n[{\"n\":1}] sqlite\ndatabase connection [other] not configured (OPENGRID_DB_OTHER_URL or settings.opengrid.connections.other.url)\n");
    drop(pool);
    let _ = std::fs::remove_dir_all(&dir);
}
