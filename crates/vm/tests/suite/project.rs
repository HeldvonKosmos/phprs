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
    assert!(err.contains("GRD106") && err.contains("module `notes` uses `Auth\\Models\\User` from module `auth`"), "{err}");
    // with the dependency
    setup(Some("name = \"notes\"\ndepends = [\"auth\"]\n"));
    assert_eq!(load(), Ok(()));
    // unknown dependency, wrong name
    setup(Some("name = \"notes\"\ndepends = [\"billing\"]\n"));
    assert!(load().unwrap_err().contains("depends on `billing`, which does not exist"));
    setup(Some("name = \"todo\"\n"));
    assert!(load().unwrap_err().contains("GRD107"));
    // ::class strings count as references too
    setup(Some("name = \"notes\"\n"));
    write("modules/notes/app/Services/Owner.grd", "<?grd\nnamespace Notes\\Services;\nfinal class Owner { public function model(): string { return \\Auth\\Models\\User::class; } }\n");
    assert!(load().unwrap_err().contains("GRD106"));
    let _ = std::fs::remove_dir_all(&root);
}

/// A throwaway project tree under the temp dir.
struct TempProject(std::path::PathBuf);

impl TempProject {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("grd-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        TempProject(root)
    }
    fn write(&self, path: &str, text: &str) -> &Self {
        let p = self.0.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
        self
    }
    fn load(&self) -> Result<(Vm, grd_vm::vm::project::ProjectSummary), String> {
        let mut vm = Vm::with_prelude().expect("prelude");
        let s = vm.load_project(&self.0).map_err(|e| e.to_string())?;
        Ok((vm, s))
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The link errors of the project layout (TEST2.7): two classes in one file
/// (GRD102), executable code in a class file (GRD103), a class nobody
/// provides (GRD104), inheritance in a circle across files (GRD105).
#[test]
fn link_errors_of_the_layout() {
    let p = TempProject::new("grd102");
    p.write("modules/shop/app/Models/Item.grd", "<?grd\nnamespace Shop\\Models;\nfinal class Item {}\nfinal class Extra {}\n");
    let e = p.load().map(|_| ()).unwrap_err();
    assert!(e.starts_with("error[GRD102]: one file, one class: this one has 2") && e.contains("Item.grd"), "{e}");

    let p = TempProject::new("grd103");
    p.write("modules/shop/app/Models/Item.grd", "<?grd\nnamespace Shop\\Models;\nfinal class Item {}\necho 'side effect';\n");
    let e = p.load().map(|_| ()).unwrap_err();
    assert!(e.starts_with("error[GRD103]: class files contain no executable code"), "{e}");

    let p = TempProject::new("grd104");
    p.write("modules/shop/app/Models/Item.grd", "<?grd\nnamespace Shop\\Models;\nfinal class Item extends Base {}\n");
    let e = p.load().map(|_| ()).unwrap_err();
    assert!(e.starts_with("error[GRD104]: class `Shop\\Models\\Base` not found (no matching path under modules/*/app)"), "{e}");

    let p = TempProject::new("grd105");
    p.write("modules/shop/app/Models/A.grd", "<?grd\nnamespace Shop\\Models;\nclass A extends B {}\n");
    p.write("modules/shop/app/Models/B.grd", "<?grd\nnamespace Shop\\Models;\nclass B extends A {}\n");
    let e = p.load().map(|_| ()).unwrap_err();
    assert!(e.starts_with("error[GRD105]: zyklische Vererbung zwischen Dateien"), "{e}");
}

/// Several modules (TEST2.7): a class of `alpha` extends one of `zeta` and
/// implements an interface of `mid`, so classes load in dependency order
/// across modules, not in the alphabetical order of the folders; route files
/// run module by module (alphabetical), the global ones last.
#[test]
fn modules_load_in_dependency_order() {
    let p = TempProject::new("multi");
    p.write("modules/alpha/module.toml", "name = \"alpha\"\ndepends = [\"zeta\", \"mid\"]\n")
        .write("modules/alpha/app/Services/Report.grd", "<?grd\nnamespace Alpha\\Services;\nuse Zeta\\Support\\Base;\nuse Mid\\Contracts\\Named;\nfinal class Report extends Base implements Named { public function name(): string { return 'report from ' . $this->origin(); } }\n")
        .write("modules/alpha/routes/web.grd", "<?grd\necho 'routes:alpha ', (new Alpha\\Services\\Report())->name(), \"\\n\";\n")
        .write("modules/mid/module.toml", "name = \"mid\"\n")
        .write("modules/mid/app/Contracts/Named.grd", "<?grd\nnamespace Mid\\Contracts;\ninterface Named { public function name(): string; }\n")
        .write("modules/mid/routes/web.grd", "<?grd\necho \"routes:mid\\n\";\n")
        .write("modules/zeta/module.toml", "name = \"zeta\"\n")
        .write("modules/zeta/app/Support/Base.grd", "<?grd\nnamespace Zeta\\Support;\nabstract class Base { public function origin(): string { return 'zeta'; } }\n")
        .write("modules/zeta/routes/web.grd", "<?grd\necho \"routes:zeta\\n\";\n")
        .write("routes/web.grd", "<?grd\necho \"routes:global\\n\";\n");
    let (vm, summary) = p.load().expect("load");
    assert_eq!(summary.modules, vec!["alpha", "mid", "zeta"]);
    let pos = |c: &str| summary.classes.iter().position(|x| x == c).unwrap_or_else(|| panic!("{c} not loaded: {:?}", summary.classes));
    assert!(pos("Zeta\\Support\\Base") < pos("Alpha\\Services\\Report") && pos("Mid\\Contracts\\Named") < pos("Alpha\\Services\\Report"), "{:?}", summary.classes);
    assert_eq!(String::from_utf8_lossy(&vm.output), "routes:alpha report from zeta\nroutes:mid\nroutes:zeta\nroutes:global\n");
    // without the dependency in the manifest the boundary check refuses it
    p.write("modules/alpha/module.toml", "name = \"alpha\"\ndepends = [\"zeta\"]\n");
    let e = p.load().map(|_| ()).unwrap_err();
    assert!(e.contains("GRD106") && e.contains("Mid\\Contracts\\Named"), "{e}");
}
