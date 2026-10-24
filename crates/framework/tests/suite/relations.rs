//! Side tables and aggregates (`withAll`, `withCount`, `withSum`), dot-path
//! sorting, request-fed `filter()`/`sort()`, graph edges (`relate`/`related`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use opengrid_framework::dispatch::handle;
use opengrid_framework::native::database::schema::kit::SyncOptions;
use opengrid_framework::native::database::schema::ops;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::{HttpRequest, Pool, PoolOptions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app")
}

fn pool(db: &str) -> Arc<Pool> {
    let pool = Arc::new(Pool::start(PoolOptions::new(root()).workers(1).database(DbConfig::mem("test", db))).expect("pool starts"));
    let db = pool.db().expect("database");
    db.block_on(ops::push(db.client(), &root(), SyncOptions::default())).expect("schema push");
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
fn side_tables_aggregates_sorting_and_edges() {
    let pool = pool("rel");
    let out = run(
        &pool,
        r#"<?grd
use Tasks\Models\Task;
use Tasks\Models\Comment;

$a = Task::create(['title' => 'A']);
$b = Task::create(['title' => 'B']);
Comment::create(['body' => 'one', 'hours' => 1, 'task_id' => $a->id]);
Comment::create(['body' => 'two', 'hours' => 3, 'task_id' => $a->id]);
Comment::create(['body' => 'three', 'hours' => 5, 'task_id' => $b->id]);

$rows = Task::query()->withCount('comments')->withSum('comments', 'hours')
    ->withAll('comments', fn($q) => $q->where('hours', 2, '>')->orderBy('hours'))
    ->sort('title')->get();
foreach ($rows as $t) {
    echo $t->title, ' ', $t->comments_count, ' ', $t->comments_sum_hours, ' ', count($t->comments), ' ', implode(',', array_map(fn($c) => $c['body'], $t->comments)), "\n";
}
$c = Comment::query()->sort('task_id.title:desc,hours:asc')->get();
echo implode(',', array_map(fn($c) => $c->body, $c)), "\n";
echo Comment::query()->sort('task_id.title:desc')->toSql(), "\n";

$edge = $a->relate('refers', $b, ['since' => 2026]);
echo isset($edge['in']) && isset($edge['out']) ? 'edge' : 'no-edge', ' ', $edge['since'], "\n";
$related = $a->related('refers', Task::class);
echo count($related), ' ', $related[0]->title, ' ', $related[0] instanceof Task ? 'model' : 'array', "\n";
echo count($b->related('refers', Task::class)), "\n";
"#,
    );
    assert_eq!(
        out,
        "A 2 4 1 two\n\
         B 1 5 1 three\n\
         three,one,two\n\
         SELECT *, task_id.title AS __sort_task_id_title FROM comments ORDER BY __sort_task_id_title COLLATE DESC\n\
         edge 2026\n\
         1 B model\n\
         0\n"
    );
}

#[test]
fn request_fed_filter_and_sort() {
    let pool = pool("req");
    run(&pool, "<?grd\nuse Tasks\\Models\\Task;\nTask::create(['title' => 'A', 'done' => true]); Task::create(['title' => 'B', 'done' => false]); Task::create(['title' => 'C', 'done' => true]);");
    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/tasks-filter?filters[done]=true&sort=title:desc"))).unwrap();
    assert_eq!(r.status, 200, "{}", r.body_str());
    let body: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
    let titles: Vec<&str> = body.as_array().unwrap().iter().map(|t| t["title"].as_str().unwrap()).collect();
    assert_eq!(titles, vec!["C", "A"], "{body}");

    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/tasks-filter?filters[password]=x"))).unwrap();
    assert_eq!(r.status, 500, "non-filterable field is rejected: {}", r.body_str());
    assert!(r.body_str().contains("ValueError"), "{}", r.body_str());

    let r = pool.run_sync(|vm| handle(vm, HttpRequest::get("/tasks-filter"))).unwrap();
    assert_eq!(body_len(&r.body), 3);
}

fn body_len(body: &[u8]) -> usize {
    serde_json::from_slice::<serde_json::Value>(body).unwrap().as_array().unwrap().len()
}
