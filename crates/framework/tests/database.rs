//! Database layer against the embedded SurrealDB (`mem://`): models declared
//! in the dialect, Builder/Model/DB native. Expectations are my own until
//! `builder_cases.php` arrives (DECISIONS: "geprüft gegen Eigenerwartung").

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::{HttpRequest, Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool(db: &str) -> Arc<Pool> {
    let opts = PoolOptions::new(root()).workers(2).database(DbConfig::mem("test", db));
    Arc::new(Pool::start(opts).expect("pool starts"))
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
fn model_crud_soft_deletes_and_pagination() {
    let pool = pool("crud");
    let out = run(
        &pool,
        r#"<?grd
use Tasks\Models\Task;
use OpenGrid\Database\RecordId;

$a = Task::create(['title' => 'Alpha', 'done' => false, 'priority' => 2, 'secret' => 'x']);
$b = Task::create(['title' => 'Beta', 'done' => true, 'priority' => 1]);
echo $a instanceof Task ? 'model' : 'no', ' ', $a->id instanceof RecordId ? 'rid' : 'norid', ' ', $a->id->table(), ' ', isset($a->secret) ? 'leak' : 'clean', ' ', $a->exists() ? 'exists' : 'new', "\n";
echo count(Task::all()), ' ', Task::where('done', true)->count(), ' ', Task::query()->sort('title:desc')->first()->title, "\n";
$found = Task::find($a->id);
echo $found->title, ' ', $found->id == $a->id ? 'same' : 'diff', ' ', $found->created_at != null ? 'ts' : 'nots', ' ', Task::find('tasks:nope') == null ? 'null' : 'found', "\n";
$found->title = 'Alpha 2';
echo $found->isDirty() ? 'dirty' : 'clean', ' ';
$found->save();
echo Task::find($a->id)->title, ' ', $found->isDirty() ? 'dirty' : 'clean', "\n";
$b->delete();
echo count(Task::all()), ' ', count(Task::query()->withTrashed()->get()), ' ', Task::query()->onlyTrashed()->first()->title, "\n";
$page = Task::query()->withTrashed()->sort('title')->paginate(1, 2);
echo json_encode(['total' => $page['total'], 'per_page' => $page['per_page'], 'current_page' => $page['current_page'], 'last_page' => $page['last_page'], 'titles' => array_map(fn($t) => $t->title, $page['data'])]), "\n";
echo json_encode(Task::query()->select('title, priority')->orderBy('priority')->get()), "\n";
echo Task::where('title', 'Alpha 2')->update(['priority' => 9]), ' ', Task::find($a->id)->priority, "\n";
echo Task::query()->whereLike('title', 'ALPH')->count(), ' ', Task::query()->whereIn('priority', [9, 1])->withTrashed()->count(), ' ', Task::query()->filter(['done' => false])->count(), "\n";
try { Task::query()->filter(['password' => 'x']); } catch (ValueError $e) { echo "filter rejected\n"; }
$a->forceDelete();
echo Task::query()->withTrashed()->count(), "\n";
"#,
    );
    assert_eq!(
        out,
        "model rid tasks clean exists\n\
         2 1 Beta\n\
         Alpha same ts null\n\
         dirty Alpha 2 clean\n\
         1 2 Beta\n\
         {\"total\":2,\"per_page\":1,\"current_page\":2,\"last_page\":2,\"titles\":[\"Beta\"]}\n\
         [{\"priority\":2,\"title\":\"Alpha 2\"}]\n\
         1 9\n\
         1 2 1\n\
         filter rejected\n\
         1\n"
    );
}

#[test]
fn raw_queries_and_transactions() {
    let pool = pool("raw");
    let out = run(
        &pool,
        r#"<?grd
use Tasks\Models\Task;
use OpenGrid\Database\DB;

Task::create(['title' => 'One']);
$rows = DB::select('SELECT count() AS n FROM tasks GROUP ALL');
echo $rows[0]['n'], ' ', json_encode(DB::select('SELECT title FROM tasks WHERE title = $t', ['t' => 'One'])), "\n";
$res = DB::transaction(function () { Task::create(['title' => 'Two']); return 'tx'; });
echo $res, ' ', Task::count(), "\n";
try {
    DB::transaction(function () { Task::create(['title' => 'Three']); throw new \RuntimeException('rollback'); });
} catch (\RuntimeException $e) {
    echo $e->getMessage(), ' ', Task::count(), "\n";
}
echo count(DB::table('tasks')->orderBy('title')->get()), ' ', DB::table('tasks')->where('title', 'Two')->first()['title'], "\n";
try { DB::query('SELEKT nonsense'); } catch (\RuntimeException $e) { echo "query error\n"; }
echo Task::count(), "\n";
"#,
    );
    assert_eq!(out, "1 [{\"title\":\"One\"}]\ntx 2\nrollback 2\n2 Two\nquery error\n2\n");
}

#[test]
fn database_over_http() {
    let pool = pool("http");
    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/db"))).expect("worker answers");
    assert_eq!(r.status, 200, "{}", r.body_str());
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(body["count"], 1);
    assert_eq!(body["first"], "from http");
    assert!(body["id"].as_str().unwrap().starts_with("tasks:"), "{}", body);
    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/db"))).expect("worker answers");
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(body["count"], 2);
}
