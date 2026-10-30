//! Database layer against the embedded SurrealDB (`mem://`): models declared
//! in the dialect, Builder/Model/DB native. Expectations are my own until
//! `builder_cases.php` arrives (DECISIONS: "checked against our own expectations").

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
    let pool = Arc::new(Pool::start(opts).expect("pool starts"));
    // TEST2.6: with the fixture's schema applied (field types, defaults,
    // indexes), as an application runs, not on tables that appear on insert
    let d = pool.db().expect("database");
    d.block_on(opengrid_framework::native::database::schema::ops::push(d.client(), &root(), Default::default())).expect("schema push");
    pool
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
    // the fixture schema has a UNIQUE index on tasks.title: the same title
    // again is refused by the database (before TEST2.6 the test ran without
    // the schema and counted 2)
    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/db"))).expect("worker answers");
    assert_eq!(r.status, 500, "{}", r.body_str());
    assert!(r.body_str().contains("Database index `tasks_title_unique` already contains 'from http'"), "{}", r.body_str());
}

#[test]
fn validation_exists_and_unique_against_the_database() {
    let pool = pool("v1");
    let out = run(
        &pool,
        r#"<?grd
use Tasks\Models\Task;
use OpenGrid\Validation\Validator;

$a = Task::create(['title' => 'Alpha', 'done' => false]);
Task::create(['title' => 'Beta', 'done' => true]);
$rules = ['title' => 'required|unique:tasks,title'];
echo json_encode(Validator::errors(['title' => 'Alpha'], $rules)), "\n";
echo json_encode(Validator::errors(['title' => 'Gamma'], $rules)), "\n";
// the column defaults to the field name; an id can be excluded (updates)
echo json_encode(Validator::errors(['title' => 'Alpha'], ['title' => 'unique:tasks'])), "\n";
echo json_encode(Validator::errors(['title' => 'Alpha'], ['title' => 'unique:tasks,title,' . $a->id->key()])), "\n";
echo json_encode(Validator::errors(['task' => 'Beta'], ['task' => 'exists:tasks,title'])), "\n";
echo json_encode(Validator::errors(['task' => 'Nope'], ['task' => 'exists:tasks,title'])), "\n";
echo json_encode(Validator::errors([], ['task' => 'nullable|exists:tasks,title'])), "\n";
try { Validator::errors(['x' => 1], ['x' => 'unique:tasks;drop,title']); } catch (ValueError $e) { echo "rejected\n"; }
"#,
    );
    assert_eq!(
        out,
        "{\"title\":[\"The title has already been taken.\"]}\n[]\n{\"title\":[\"The title has already been taken.\"]}\n[]\n[]\n{\"task\":[\"The selected task is invalid.\"]}\n[]\nrejected\n"
    );
}

/// Q1: the query builder as in Laravel, without relations (D13): OR groups
/// and nested groups, whereNot/Between/NotIn/Column, LIKE, latest/oldest,
/// pluck/value, aggregates, increment/decrement, chunk, firstOrCreate,
/// updateOrCreate, every builder method as a static call on the model;
/// field names and operators are checked (they go into the query text).
#[test]
fn query_builder_like_laravel() {
    let pool = pool("q1");
    let out = run(
        &pool,
        r#"<?grd
use Tasks\Models\Task;

foreach ([['Alpha', false, 1], ['Beta', true, 2], ['Gamma', false, 3], ['Delta', true, 4], ['Epsilon', false, 5]] as [$t, $d, $p]) {
    Task::create(['title' => $t, 'done' => $d, 'priority' => $p]);
}
$titles = fn (array $a): string => implode(',', $a);
// (done AND priority > 3) OR title = Alpha, as Laravel's flat chain
echo $titles(Task::where('done', true)->where('priority', '>', 3)->orWhere('title', 'Alpha')->orderBy('priority')->pluck('title')), "\n";
// the platform's argument order is accepted too
echo Task::where('priority', 3, '>=')->count(), ' ', Task::where('priority', '>=', 3)->count(), "\n";
// nested group
echo $titles(Task::where('done', false)->where(fn ($q) => $q->where('priority', 1)->orWhere('priority', 5))->orderBy('priority')->pluck('title')), "\n";
echo Task::whereNot('done', true)->count(), ' ', Task::whereBetween('priority', [2, 4])->count(), ' ', Task::whereNotBetween('priority', [2, 4])->count(), ' ', Task::whereNotIn('priority', [1, 2])->count(), "\n";
echo Task::whereColumn('created_at', '<=', 'updated_at')->count(), ' ', Task::where('title', 'like', '%TA')->count(), ' ', Task::where('title', 'not like', '%ta')->count(), "\n";
echo Task::query()->orderByDesc('priority')->value('title'), ' ', Task::oldest('priority')->value('title'), ' ', Task::latest('priority')->first()->title, "\n";
echo json_encode(Task::query()->orderBy('priority')->take(3)->pluck('priority', 'title')), "\n";
echo Task::query()->sum('priority'), ' ', Task::query()->avg('priority'), ' ', Task::query()->min('priority'), ' ', Task::query()->max('priority'), ' ', json_encode(Task::where('priority', '>', 99)->avg('priority')), ' ', Task::where('priority', '>', 99)->sum('priority'), "\n";
echo Task::where('title', 'Alpha')->increment('priority', 10), ' ', Task::where('title', 'Alpha')->value('priority'), ' ';
echo Task::where('title', 'Alpha')->decrement('priority'), ' ', Task::where('title', 'Alpha')->value('priority'), ' ';
Task::where('title', 'Gamma')->increment('priority', 1, ['done' => true]);
echo Task::where('title', 'Gamma')->value('priority'), ' ', json_encode(Task::where('title', 'Gamma')->value('done')), "\n";
$log = new ArrayObject();
Task::query()->orderBy('title')->chunk(2, function (array $rows, int $page) use ($log): void { $log->append($page . ':' . count($rows)); });
$first = new ArrayObject();
Task::query()->orderBy('title')->chunk(2, function (array $rows) use ($first): bool { $first->append($rows[0]->title); return false; });
echo implode(' ', $log->getArrayCopy()), ' | ', implode(',', $first->getArrayCopy()), "\n";
$a = Task::firstOrCreate(['title' => 'Alpha']);
$f = Task::firstOrCreate(['title' => 'Zeta'], ['priority' => 6]);
echo $a->priority, ' ', $f->priority, ' ', Task::query()->count(), ' ';
$u = Task::updateOrCreate(['title' => 'Zeta'], ['priority' => 7]);
echo $u->priority, ' ', Task::where('title', 'Zeta')->value('priority'), ' ', Task::query()->count(), "\n";
// a soft-deleted row stays out of OR groups
Task::where('title', 'Alpha')->first()->delete();
echo Task::where('title', 'Alpha')->orWhere('title', 'Beta')->count(), ' ', Task::where('title', 'Alpha')->orWhere('title', 'Beta')->toSql(), "\n";
foreach ([fn () => Task::where('priority; DELETE tasks', 1), fn () => Task::where('priority', 'OR 1=1', 3), fn () => Task::whereIn('id) OR (1', [1]), fn () => Task::query()->orderBy('priority', 'desc; DROP')] as $bad) {
    try { $bad(); echo "accepted "; } catch (ValueError $e) { echo 'rejected '; }
}
echo "\n";
"#,
    );
    let lines: Vec<&str> = out.lines().collect();
    let want = [
        "Alpha,Delta",
        "3 3",
        "Alpha,Epsilon",
        "3 3 2 3",
        "5 2 3",
        "Epsilon Alpha Epsilon",
        r#"{"Alpha":1,"Beta":2,"Gamma":3}"#,
        "15 3 1 5 null 0",
        "1 11 1 10 4 true",
        "1:2 2:2 3:1 | Alpha",
        "10 6 6 7 7 6",
        "1 SELECT * FROM tasks WHERE deleted_at IS NONE AND (title = $f0 OR title = $f1)",
        "rejected rejected rejected rejected ",
    ];
    for (i, w) in want.iter().enumerate() {
        assert_eq!(lines.get(i).copied(), Some(*w), "line {}; output:\n{out}", i + 1);
    }
}
