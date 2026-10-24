//! Hot reload: workers rebuild their VM between two requests when the
//! generation changes (explicitly or through the file watcher). A broken
//! reload keeps the old code and shows the compile error until it is fixed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use opengrid_framework::dispatch::handle;
use opengrid_framework::{HttpRequest, HttpResponse, Pool, PoolOptions};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn temp_app(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opengrid-reload-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app"), &dir);
    dir
}

fn send(pool: &Pool, req: HttpRequest) -> HttpResponse {
    pool.run_sync(move |vm| handle(vm, req)).expect("worker answers")
}

fn controller(root: &Path) -> PathBuf {
    root.join("modules/tasks/app/Controllers/TaskController.grd")
}

#[test]
fn explicit_reload_and_broken_source() {
    let root = temp_app("explicit");
    let pool = Arc::new(Pool::start(PoolOptions::new(&root).workers(2)).expect("pool starts"));
    assert_eq!(send(&pool, HttpRequest::get("/hello")).body_str(), "<h1>Hello</h1>");

    let file = controller(&root);
    let original = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, original.replace("<h1>Hello</h1>", "<h1>Changed</h1>")).unwrap();
    pool.reload();
    for _ in 0..2 {
        assert_eq!(send(&pool, HttpRequest::get("/hello")).body_str(), "<h1>Changed</h1>");
    }

    // a class file with top-level code violates GRD103: old code stays, error shows
    std::fs::write(&file, format!("{original}\necho 'oops';\n")).unwrap();
    pool.reload();
    for _ in 0..2 {
        let r = send(&pool, HttpRequest::get("/hello"));
        assert_eq!(r.status, 500, "{}", r.body_str());
        assert!(r.body_str().contains("GRD103"), "{}", r.body_str());
    }

    std::fs::write(&file, &original).unwrap();
    pool.reload();
    for _ in 0..2 {
        assert_eq!(send(&pool, HttpRequest::get("/hello")).body_str(), "<h1>Hello</h1>");
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn watcher_reloads_on_change() {
    let root = temp_app("watch");
    let pool = Arc::new(Pool::start(PoolOptions::new(&root).workers(1).watch(true)).expect("pool starts"));
    assert_eq!(send(&pool, HttpRequest::get("/hello")).body_str(), "<h1>Hello</h1>");
    let file = controller(&root);
    let original = std::fs::read_to_string(&file).unwrap();
    std::thread::sleep(Duration::from_millis(300)); // watcher is up
    std::fs::write(&file, original.replace("<h1>Hello</h1>", "<h1>Watched</h1>")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut body = String::new();
    while Instant::now() < deadline {
        body = send(&pool, HttpRequest::get("/hello")).body_str();
        if body == "<h1>Watched</h1>" {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(body, "<h1>Watched</h1>", "generation {}", pool.generation());
    let _ = std::fs::remove_dir_all(&root);
}
