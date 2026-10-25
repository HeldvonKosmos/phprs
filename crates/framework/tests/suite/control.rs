//! J1: the control socket. A server listens on `.opengrid/control.sock`
//! (0600); tasks sent there run inside it with output and exit status; a
//! dialect task is found by convention (`notes:prune` → `Notes\Tasks\Prune`);
//! without a server (also with a stale socket file) the caller runs it itself.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use opengrid_framework::control;
use opengrid_framework::native::database::surreal::DbConfig;
use opengrid_framework::scaffold::new_project;
use opengrid_framework::{Pool, PoolOptions};

const PRUNE: &str = r#"<?grd
namespace Notes\Tasks;

use OpenGrid\Console\Task;

final class Prune extends Task
{
    public function handle(array $args): int
    {
        $this->info('pruning notes older than ' . ($args['days'] ?? '30') . ' days');
        return 0;
    }
}
"#;

const FAIL: &str = r#"<?grd
namespace Notes\Tasks;

use OpenGrid\Console\Task;

final class Fail extends Task
{
    public function handle(array $args): int
    {
        $this->error('nothing to do');
        return 3;
    }
}
"#;

fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn tasks_run_inside_the_server() {
    let root = std::env::temp_dir().join(format!("opengrid-control-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    new_project(&root, Some("Ctl")).unwrap();
    for (path, src) in [("modules/notes/app/Tasks/Prune.grd", PRUNE), ("modules/notes/app/Tasks/Fail.grd", FAIL)] {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    let pool = Arc::new(Pool::start(PoolOptions::new(root.clone()).workers(1).database(DbConfig::mem("test", "control"))).expect("pool"));

    // no server yet: the caller runs the task itself
    assert!(control::forward(&root, "run", &args(&[("task", "notes:prune")])).is_none());
    assert_eq!(control::run_task(&pool, "run", &args(&[("task", "notes:prune"), ("days", "7")])).unwrap(), "pruning notes older than 7 days\n");

    // the server: its own runtime on a thread, as in `opengrid start`
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let p = Arc::clone(&pool);
    let server = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.spawn(async move { control::serve(p).await });
        let _ = stop_rx.recv();
        rt.shutdown_timeout(Duration::from_secs(2));
    });
    let sock = control::socket_path(&root);
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::os::unix::net::UnixStream::connect(&sock).is_err() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::metadata(&sock).unwrap().permissions().mode() & 0o777, 0o600, "owner only");

    let out = control::forward(&root, "run", &args(&[("task", "notes:prune"), ("days", "14")])).expect("server answers");
    assert_eq!(out.unwrap(), "pruning notes older than 14 days\n");
    let err = control::forward(&root, "run", &args(&[("task", "notes:fail")])).expect("server answers").unwrap_err();
    assert!(err.contains("ERROR: nothing to do") && err.contains("task exited with code 3"), "{err}");
    let err = control::forward(&root, "run", &args(&[("task", "notes:nope")])).expect("server answers").unwrap_err();
    assert!(err.contains("class Notes\\Tasks\\Nope not found"), "{err}");
    let out = control::forward(&root, "migrate_push", &BTreeMap::new()).expect("server answers").unwrap();
    assert!(out.starts_with("schema in sync"), "{out}");
    let status = control::request(&root, &serde_json::json!({ "cmd": "status" })).expect("server answers").unwrap();
    assert_eq!(status[0]["pid"], std::process::id());
    assert_eq!(status[0]["ok"], true);

    // a second server for the same project is refused
    let p2 = Arc::clone(&pool);
    let second = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async move { control::serve(p2).await });
    assert_eq!(second.unwrap_err().kind(), std::io::ErrorKind::AddrInUse);

    // server gone, socket file left behind: treated as no server
    stop_tx.send(()).unwrap();
    server.join().unwrap();
    assert!(sock.exists(), "stale socket file");
    assert!(control::forward(&root, "run", &args(&[("task", "notes:prune")])).is_none());
    let _ = std::fs::remove_dir_all(&root);
}
