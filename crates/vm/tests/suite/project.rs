//! Loading a modules-only project tree (DIALECT §2/§3).

use std::path::Path;

use grd_vm::value::Val;
use grd_vm::vm::Vm;

#[test]
fn loads_modules_in_dependency_order_and_runs_routes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app");
    let mut vm = Vm::with_prelude().expect("prelude");
    let summary = vm.load_project(&root).expect("load project");
    assert_eq!(summary.modules, vec!["tasks".to_string()]);
    assert_eq!(summary.class_files, 4);
    assert_eq!(summary.route_files, 2);
    // route files ran (module routes before global routes)
    assert_eq!(String::from_utf8_lossy(&vm.output), "routes:tasks routes:global\n");
    // the controller is callable from Rust: instantiate, call, read the result
    let class = vm.lookup_class(b"Tasks\\Controllers\\TaskController").expect("controller class");
    let controller = vm.instantiate(class, Vec::new()).expect("new controller");
    let result = vm.call_method_sync(controller, b"index", Vec::new()).expect("index()");
    let Val::Array(arr) = result else { panic!("array expected") };
    assert_eq!(arr.len(), 3);
    let tasks = arr.get(&grd_vm::value::ArrayKey::from_str("tasks")).expect("tasks");
    let Val::Array(tasks) = tasks else { panic!("tasks array") };
    assert_eq!(tasks.len(), 2);
}

#[test]
fn class_name_must_match_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bad");
    let mut vm = Vm::with_prelude().expect("prelude");
    let err = vm.load_project(&root).expect_err("GRD101 expected");
    let msg = err.to_string();
    assert!(msg.contains("GRD101"), "{msg}");
    assert!(msg.contains("Billing\\Models\\Invoice"), "{msg}");
}
