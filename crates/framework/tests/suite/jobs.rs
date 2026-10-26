//! J2: background jobs in the server process. A request dispatches; a worker
//! runs the job afterwards with database access but without the request
//! (request boundary); failing jobs run again up to `$tries` times.
//! J4: with a database jobs are rows in `jobs` until they succeed; only a
//! runner (the server) takes them, releases reservations left by a crash,
//! moves jobs that fail for good to `failed_jobs`; delayed jobs wait.

use std::sync::Arc;
use std::time::{Duration, Instant};

use opengrid_framework::control::run_task;
use opengrid_framework::dispatch::handle;
use opengrid_framework::native::jobs::start_runner;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::scaffold::new_project;
use opengrid_framework::{HttpRequest, Pool, PoolOptions};

const FILES: &[(&str, &str)] = &[
    (
        "modules/notes/app/Jobs/Record.grd",
        r#"<?grd
namespace Notes\Jobs;

use OpenGrid\Database\DB;
use OpenGrid\Queue\Job;

final class Record extends Job
{
    public function __construct(private string $title, private int $n) {}

    public function handle(): void
    {
        try { request(); $seen = 'request'; } catch (\Throwable $e) { $seen = 'no request'; }
        DB::table('job_log')->insert(['title' => $this->title, 'n' => $this->n, 'seen' => $seen]);
    }
}
"#,
    ),
    (
        "modules/notes/app/Jobs/Flaky.grd",
        r#"<?grd
namespace Notes\Jobs;

use OpenGrid\Database\DB;
use OpenGrid\Queue\Job;

final class Flaky extends Job
{
    public int $tries = 3;

    public function handle(): void
    {
        DB::table('attempts')->insert(['job' => 'flaky']);
        if (DB::table('attempts')->count() < 3) {
            throw new \RuntimeException('not yet');
        }
    }
}
"#,
    ),
    (
        "modules/notes/app/Jobs/Broken.grd",
        r#"<?grd
namespace Notes\Jobs;

use OpenGrid\Queue\Job;

final class Broken extends Job
{
    public int $tries = 2;

    public function handle(): void { throw new \LogicException('always'); }
}
"#,
    ),
    (
        "modules/notes/app/Controllers/JobController.grd",
        r#"<?grd
namespace Notes\Controllers;

use Notes\Jobs\Broken;
use Notes\Jobs\Flaky;
use Notes\Jobs\Record;
use OpenGrid\Http\Response;

final class JobController
{
    public function queue(): Response
    {
        Record::dispatch('first', 1);
        Record::dispatch('second', 2);
        Flaky::dispatch();
        Broken::dispatch();
        return Response::json(['queued' => 4]);
    }
}
"#,
    ),
    ("modules/notes/routes/web.grd", "<?grd\nuse Notes\\Controllers\\JobController;\nuse OpenGrid\\Routing\\Route;\nRoute::post('/queue', [JobController::class, 'queue']);\n"),
];

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
fn jobs_run_after_the_request_with_retries() {
    let root = std::env::temp_dir().join(format!("opengrid-jobs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    new_project(&root, Some("Jobs")).unwrap();
    for (path, src) in FILES {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    let pool = Arc::new(Pool::start(PoolOptions::new(root.clone()).workers(2).database(DbConfig::mem("test", "jobs"))).expect("pool"));
    start_runner(&pool, Duration::from_millis(50));
    let resp = pool.run_sync(|vm| handle(vm, HttpRequest::post("/queue", "application/json", b"{}"))).unwrap();
    assert_eq!((resp.status, resp.body_str()), (200, r#"{"queued":4}"#.to_string()));

    let deadline = Instant::now() + Duration::from_secs(20);
    let stats = loop {
        let s = run(&pool, "<?grd echo json_encode(OpenGrid\\Queue\\Job::stats());");
        if s.starts_with("[0,") || Instant::now() > deadline {
            break s;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // pending, done (2 records + flaky), failed (broken)
    assert_eq!(stats, "[0,3,1]");
    let out = run(&pool, "<?grd use OpenGrid\\Database\\DB; echo json_encode(DB::table('job_log')->orderBy('n')->select('title, n, seen')->get()), ' ', DB::table('attempts')->count();");
    assert_eq!(out, r#"[{"n":1,"seen":"no request","title":"first"},{"n":2,"seen":"no request","title":"second"}] 3"#);
    drop(pool);
    let _ = std::fs::remove_dir_all(&root);
}

fn project(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("opengrid-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    new_project(&root, Some("Jobs")).unwrap();
    for (path, src) in FILES {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    root
}

fn wait_for(pool: &Pool, src: &'static str, want: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let s = run(pool, src);
        if s == want || Instant::now() > deadline {
            return s;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

const STATS: &str = "<?grd echo json_encode(OpenGrid\\Queue\\Job::stats());";
const LOGGED: &str = "<?grd echo OpenGrid\\Database\\DB::table('job_log')->count();";

#[test]
fn durable_jobs_wait_for_the_runner_and_fail_into_failed_jobs() {
    let root = project("jobs-j4");
    let pool = Arc::new(Pool::start(PoolOptions::new(root.clone()).workers(2).database(DbConfig::mem("test", "jobs4"))).expect("pool"));
    let args = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<std::collections::BTreeMap<_, _>>();

    // no runner yet (a task pool): the jobs are rows, nothing runs
    let resp = pool.run_sync(|vm| handle(vm, HttpRequest::post("/queue", "application/json", b"{}"))).unwrap();
    assert_eq!(resp.status, 200, "{}", resp.body_str());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(run(&pool, STATS), "[4,0,0]");
    let logged = run(&pool, LOGGED);
    assert!(logged.contains("does not exist"), "no job has run yet: {logged}");
    // a reservation left behind by a crashed server
    pool.run_sync(|vm| {
        opengrid_framework::native::database::query(vm, "UPDATE jobs SET reserved_at = time::now() WHERE args[0] = 'first'", Default::default()).unwrap();
        vm.reset_request();
    })
    .unwrap();

    // the server starts: releases the reservation, runs everything once
    start_runner(&pool, Duration::from_millis(50));
    assert_eq!(wait_for(&pool, STATS, "[0,3,1]"), "[0,3,1]");
    assert_eq!(run(&pool, LOGGED), "2");
    let failed = run_task(&pool, "queue_failed", &args(&[])).unwrap();
    assert!(failed.contains("Notes\\Jobs\\Broken") && failed.contains("LogicException: always"), "{failed}");

    // delayed: waits, then runs
    run(&pool, "<?grd Notes\\Jobs\\Record::dispatchLater(1, 'later', 3);");
    assert_eq!(run(&pool, LOGGED), "2", "not before its time");
    assert_eq!(wait_for(&pool, LOGGED, "3"), "3");

    // retry the failed job: it runs its tries again and fails again
    assert_eq!(run_task(&pool, "queue_retry", &args(&[("id", "all")])).unwrap(), "1 job(s) queued again\n");
    // the retried row is gone from failed_jobs, the new failure is there
    assert_eq!(wait_for(&pool, STATS, "[0,4,1]"), "[0,4,1]");
    assert!(run_task(&pool, "queue_retry", &args(&[("id", "nope")])).unwrap_err().contains("no failed job nope"));
    assert_eq!(run_task(&pool, "queue_flush", &args(&[])).unwrap(), "1 failed job(s) deleted\n");
    assert_eq!(run(&pool, STATS), "[0,4,0]");
    drop(pool);
    let _ = std::fs::remove_dir_all(&root);
}
