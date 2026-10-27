//! J3: the scheduler in the server process: routes files declare tasks and
//! jobs with a frequency; `tick` at a given minute runs what is due there,
//! tasks directly and jobs through the job queue, without child processes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{TimeZone, Utc};
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::scaffold::new_project;
use opengrid_framework::{scheduler, Pool, PoolOptions};

const FILES: &[(&str, &str)] = &[
    (
        "modules/notes/app/Tasks/Stamp.grd",
        r#"<?grd
namespace Notes\Tasks;

use OpenGrid\Console\Task;
use OpenGrid\Database\DB;

final class Stamp extends Task
{
    public function handle(array $args): int
    {
        DB::table('stamps')->insert(['by' => 'task', 'tag' => $args['tag'] ?? '']);
        return 0;
    }
}
"#,
    ),
    (
        "modules/notes/app/Jobs/StampJob.grd",
        r#"<?grd
namespace Notes\Jobs;

use OpenGrid\Database\DB;
use OpenGrid\Queue\Job;

final class StampJob extends Job
{
    public function __construct(private string $tag) {}
    public function handle(): void { DB::table('stamps')->insert(['by' => 'job', 'tag' => $this->tag]); }
}
"#,
    ),
    (
        "routes/console.grd",
        r#"<?grd
use Notes\Jobs\StampJob;
use OpenGrid\Scheduling\Schedule;

Schedule::task('notes:stamp', ['tag' => 'nightly'])->dailyAt('03:00');
Schedule::job(StampJob::class, ['quarter'])->everyFifteenMinutes();
Schedule::task('notes:stamp', ['tag' => 'monday'])->weeklyOn(1, '8:30');
"#,
    ),
];

fn count(pool: &Pool, by: &'static str) -> i64 {
    let src: &'static str = if by == "task" { "<?grd echo OpenGrid\\Database\\DB::table('stamps')->where('by', 'task')->count();" } else { "<?grd echo OpenGrid\\Database\\DB::table('stamps')->where('by', 'job')->count();" };
    pool.run_sync(move |vm| {
        // the table exists only after the first stamp: count 0 until then
        let n = match vm.run_file("c.grd", src.as_bytes()) {
            Ok(_) => String::from_utf8_lossy(&vm.output).parse().unwrap_or(-1),
            Err(e) if format!("{e:?}").contains("does not exist") => 0,
            Err(e) => panic!("{e:?}"),
        };
        vm.reset_request();
        n
    })
    .unwrap()
}

fn wait_for(pool: &Pool, by: &'static str, n: i64) -> i64 {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let c = count(pool, by);
        if c >= n || Instant::now() > deadline {
            return c;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn due_entries_run_in_process() {
    let root = std::env::temp_dir().join(format!("opengrid-schedule-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    new_project(&root, Some("Sched")).unwrap();
    for (path, src) in FILES {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    let pool = Arc::new(Pool::start(PoolOptions::new(root.clone()).workers(2).database(DbConfig::mem("test", "schedule"))).expect("pool"));
    // J4: with a database scheduled jobs are durable rows; the server runs them
    opengrid_framework::native::jobs::start_runner(&pool, Duration::from_millis(50));
    let entries = scheduler::entries(&pool);
    let crons: Vec<String> = entries.iter().map(|e| format!("{} {} {}", e.kind, e.target, e.cron)).collect();
    assert_eq!(crons, ["task notes:stamp 0 3 * * *", "job Notes\\Jobs\\StampJob */15 * * * *", "task notes:stamp 30 8 * * 1"]);

    // Monday 2026-10-26 03:00 UTC: the nightly task and the quarter-hour job
    let started = scheduler::tick(&pool, Utc.with_ymd_and_hms(2026, 10, 26, 3, 0, 0).unwrap());
    assert_eq!(started, ["task notes:stamp", "job Notes\\Jobs\\StampJob"]);
    assert_eq!((wait_for(&pool, "task", 1), wait_for(&pool, "job", 1)), (1, 1));
    // 03:07: nothing due
    assert!(scheduler::tick(&pool, Utc.with_ymd_and_hms(2026, 10, 26, 3, 7, 0).unwrap()).is_empty());
    // Monday 08:30: the weekly task, and the quarter-hour job (minute 30)
    let started = scheduler::tick(&pool, Utc.with_ymd_and_hms(2026, 10, 26, 8, 30, 0).unwrap());
    assert_eq!(started, ["job Notes\\Jobs\\StampJob", "task notes:stamp"]);
    assert_eq!((wait_for(&pool, "task", 2), wait_for(&pool, "job", 2)), (2, 2));
    let tags = pool
        .run_sync(|vm| {
            vm.run_file("t.grd", b"<?grd echo implode(',', OpenGrid\\Database\\DB::table('stamps')->where('by', 'task')->orderBy('tag')->pluck('tag'));").unwrap();
            let s = String::from_utf8_lossy(&vm.output).into_owned();
            vm.reset_request();
            s
        })
        .unwrap();
    assert_eq!(tags, "monday,nightly");
    // an invalid cron expression is refused where it is written
    let err = pool
        .run_sync(|vm| {
            let r = vm.run_file("bad.grd", b"<?grd OpenGrid\\Scheduling\\Schedule::task('x:y')->cron('every day');");
            vm.reset_request();
            r.err().map(|e| e.to_string()).unwrap_or_default()
        })
        .unwrap();
    assert!(err.contains("invalid cron expression"), "{err}");
    drop(pool);
    let _ = std::fs::remove_dir_all(&root);
}

/// Every frequency method gives the cron expression of Laravel's
/// `Scheduling\Event` (the Laravel comparison application's own Schedule,
/// TEST2.6).
#[test]
fn frequencies_are_laravels_cron_expressions() {
    use std::process::Command;
    let app = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bench/laravel-app");
    let php = Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success());
    if !php || !app.join("artisan").exists() {
        crate::oracle::missing(crate::oracle::Oracle::LaravelVendor, "php or bench/laravel-app not available: cron expressions not compared with Laravel");
        return;
    }
    const CALLS: &[&str] = &[
        "everyMinute()", "everyFiveMinutes()", "everyTenMinutes()", "everyFifteenMinutes()", "everyThirtyMinutes()", "hourly()", "hourlyAt(17)",
        "daily()", "dailyAt('03:00')", "dailyAt('8:30')", "dailyAt('23')", "weekly()", "weeklyOn(1, '8:30')", "weeklyOn(5)", "monthly()", "cron('5 4 * * 2')",
    ];
    let calls = CALLS.iter().map(|c| format!("$s->command('x')->{c}->getExpression()")).collect::<Vec<_>>().join(", ");
    // serve.sh rebuilds the app's caches under this lock (`api.sh --app
    // laravel` may start it at the same time); booting the app waits for it
    let out = Command::new("flock")
        .current_dir(&app)
        .args(["bootstrap/cache/.opengrid-optimize.lock", "php", "-r"])
        .arg(format!("require 'vendor/autoload.php'; $app = require 'bootstrap/app.php'; $app->make(Illuminate\\Contracts\\Console\\Kernel::class)->bootstrap(); $s = $app->make(Illuminate\\Console\\Scheduling\\Schedule::class); echo json_encode([{calls}]);"))
        .output()
        .expect("run php");
    let laravel: Vec<String> = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("Laravel's answer ({e}): {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app");
    let pool = Arc::new(Pool::start(PoolOptions::new(root).workers(1)).expect("pool"));
    let before = scheduler::entries(&pool).len();
    let src = format!("<?grd\nuse OpenGrid\\Scheduling\\Schedule;\n{}", CALLS.iter().map(|c| format!("Schedule::task('x:y')->{c};\n")).collect::<String>());
    pool.run_sync(move |vm| {
        vm.run_file("frequencies.grd", src.as_bytes()).expect("schedule script");
    })
    .unwrap();
    let ours: Vec<String> = scheduler::entries(&pool).into_iter().skip(before).map(|e| e.cron).collect();
    let pairs: Vec<String> = CALLS.iter().zip(ours.iter().zip(&laravel)).filter(|(_, (o, l))| o != l).map(|(c, (o, l))| format!("{c}: ours {o:?}, Laravel {l:?}")).collect();
    assert_eq!(ours.len(), CALLS.len());
    assert!(pairs.is_empty(), "cron expressions differ from Laravel's:\n{}", pairs.join("\n"));
}
