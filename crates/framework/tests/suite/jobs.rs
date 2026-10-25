//! J2: background jobs in the server process. A request dispatches; a worker
//! runs the job afterwards with database access but without the request
//! (request boundary); failing jobs run again up to `$tries` times.

use std::sync::Arc;
use std::time::{Duration, Instant};

use opengrid_framework::dispatch::handle;
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
