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

/// MOD1: a module with a manifest may only use classes of the modules it
/// depends on (GRD106); unknown dependencies and a wrong name are GRD107;
/// without a manifest nothing is enforced.
#[test]
fn module_manifests_guard_module_boundaries() {
    let root = std::env::temp_dir().join(format!("grd-modules-{}", std::process::id()));
    let write = |path: &str, text: &str| {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    let setup = |notes_manifest: Option<&str>| {
        let _ = std::fs::remove_dir_all(&root);
        write("modules/auth/app/Models/User.grd", "<?grd\nnamespace Auth\\Models;\nfinal class User { public string $name = 'ada'; }\n");
        write("modules/auth/module.toml", "name = \"auth\"\nversion = \"0.1.0\"\n");
        write("modules/notes/app/Services/Owner.grd", "<?grd\nnamespace Notes\\Services;\nuse Auth\\Models\\User;\nfinal class Owner { public function name(): string { return (new User())->name; } }\n");
        if let Some(m) = notes_manifest {
            write("modules/notes/module.toml", m);
        }
    };
    let load = || {
        let mut vm = Vm::with_prelude().expect("prelude");
        vm.load_project(&root).map(|_| ()).map_err(|e| e.to_string())
    };
    // no manifest: not enforced
    setup(None);
    assert_eq!(load(), Ok(()));
    // manifest without the dependency
    setup(Some("name = \"notes\"\n"));
    let err = load().unwrap_err();
    assert!(err.contains("GRD106") && err.contains("Modul `notes` benutzt `Auth\\Models\\User` aus Modul `auth`"), "{err}");
    // with the dependency
    setup(Some("name = \"notes\"\ndepends = [\"auth\"]\n"));
    assert_eq!(load(), Ok(()));
    // unknown dependency, wrong name
    setup(Some("name = \"notes\"\ndepends = [\"billing\"]\n"));
    assert!(load().unwrap_err().contains("hängt von `billing` ab, das es nicht gibt"));
    setup(Some("name = \"todo\"\n"));
    assert!(load().unwrap_err().contains("GRD107"));
    // ::class strings count as references too
    setup(Some("name = \"notes\"\n"));
    write("modules/notes/app/Services/Owner.grd", "<?grd\nnamespace Notes\\Services;\nfinal class Owner { public function model(): string { return \\Auth\\Models\\User::class; } }\n");
    assert!(load().unwrap_err().contains("GRD106"));
    let _ = std::fs::remove_dir_all(&root);
}
