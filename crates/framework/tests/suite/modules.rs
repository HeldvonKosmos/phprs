//! PKG1: `opengrid task module`. A module comes from a Git repository, is
//! pinned in `modules.lock`, and must leave the project compiling (GRD
//! check); a module that breaks it is taken out again.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use opengrid_framework::PoolOptions;
use opengrid_framework::control::module_task;
use opengrid_framework::scaffold::new_project;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").current_dir(dir).args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main"]).args(args).status().unwrap().success();
    assert!(ok, "git {args:?}");
}

fn repo(dir: &Path, files: &[(&str, &str)]) -> String {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    for (path, src) in files {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "init"]);
    format!("file://{}", dir.display())
}

fn args(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn modules_from_git_pass_the_check_or_are_removed() {
    let base = std::env::temp_dir().join(format!("opengrid-modules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let root = base.join("project");
    new_project(&root, Some("Mods")).unwrap();
    let options = PoolOptions::new(root.clone());

    let good = repo(&base.join("blog"), &[
        ("module.toml", "name = \"blog\"\nversion = \"0.1.0\"\n"),
        ("app/Models/Tag.grd", "<?grd\nnamespace Blog\\Models;\n\nfinal class Tag\n{\n    public function __construct(public string $name) {}\n}\n"),
    ]);
    let out = module_task(&options, &args(&[("cmd", "add"), ("url", &good)]), None).unwrap();
    assert!(out.starts_with("added blog 0.1.0"), "{out}");
    assert!(module_task(&options, &args(&[("cmd", "list")]), None).unwrap().starts_with("blog 0.1.0"));
    assert!(module_task(&options, &args(&[("cmd", "verify")]), None).is_ok());

    // a type error in the module: the GRD check fails, the module goes again
    let bad = repo(&base.join("shop"), &[
        ("module.toml", "name = \"shop\"\n"),
        ("app/Models/Cart.grd", "<?grd\nnamespace Shop\\Models;\n\nfinal class Cart\n{\n    public function total(): int\n    {\n        $sum = 0;\n        $sum = 'x';\n        return 1;\n    }\n}\n"),
    ]);
    let err = module_task(&options, &args(&[("cmd", "add"), ("url", &bad)]), None).unwrap_err();
    assert!(err.contains("module `shop` does not pass the check") && err.contains("GRD021"), "{err}");
    assert!(!root.join("modules/shop").exists());
    assert!(!module_task(&options, &args(&[("cmd", "list")]), None).unwrap().contains("shop"));

    assert_eq!(module_task(&options, &args(&[("cmd", "remove"), ("name", "blog")]), None).unwrap(), "removed blog");
    assert!(!root.join("modules/blog").exists());
    let _ = std::fs::remove_dir_all(&base);
}
