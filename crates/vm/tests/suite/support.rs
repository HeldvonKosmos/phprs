//! S1: OpenGrid\Support (Str, Arr, Collection, collect, data_get) behaves
//! like Laravel's Illuminate\Support. `support/cases.txt` holds expressions;
//! `support/cases.out` is what Laravel's own classes printed for them
//! (`support/driver_vendor.php` runs them with the vendor directory of the
//! Laravel comparison application, PHP 8.4). Each line here must
//! print the same JSON. When php and refs are available, the expectations
//! are checked against Laravel again.

use std::path::Path;
use std::process::Command;

fn dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support")
}

#[test]
fn support_classes_match_laravel() {
    let cases: Vec<String> = std::fs::read_to_string(dir().join("cases.txt")).unwrap().lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    let expected: Vec<String> = std::fs::read_to_string(dir().join("cases.out")).unwrap().lines().map(str::to_string).collect();
    assert_eq!(cases.len(), expected.len(), "cases.out is stale");
    let mut src = String::from("<?grd\nuse OpenGrid\\Support\\Str;\nuse OpenGrid\\Support\\Arr;\nuse OpenGrid\\Support\\LazyCollection;\n");
    for c in &cases {
        src.push_str(&format!("try {{ echo json_encode({c}), \"\\n\"; }} catch (Throwable $e) {{ echo 'ERR ', substr(strrchr('\\\\' . get_class($e), '\\\\'), 1), ': ', $e->getMessage(), \"\\n\"; }}\n"));
    }
    let mut vm = grd_vm::vm::Vm::with_prelude().expect("prelude");
    let r = vm.run_file("support.grd", src.as_bytes());
    let out = String::from_utf8_lossy(&vm.output).into_owned();
    if let Err(e) = r {
        panic!("script failed: {e}\noutput so far:\n{out}");
    }
    let got: Vec<&str> = out.lines().collect();
    let mut failures = Vec::new();
    for (i, c) in cases.iter().enumerate() {
        let g = got.get(i).copied().unwrap_or("<missing>");
        if g != expected[i] {
            failures.push(format!("{c}\n    laravel: {}\n    dialect: {g}", expected[i]));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ from Laravel:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

#[test]
fn support_expectations_are_laravel_output() {
    // Laravel 13.38 with its real dependencies: the vendor directory of the
    // Laravel comparison application (bench/framework/laravel/setup.sh)
    let vendor = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bench/laravel-app/vendor");
    let php = Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success());
    if !php || !vendor.join("autoload.php").exists() {
        crate::oracle::missing(crate::oracle::Oracle::LaravelVendor, "php or bench/laravel-app/vendor not available (run bench/framework/laravel/setup.sh)");
        return;
    }
    let out = Command::new("php").arg(dir().join("driver_vendor.php")).arg(&vendor).arg(dir().join("cases.txt")).output().expect("php");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), std::fs::read_to_string(dir().join("cases.out")).unwrap(), "cases.out is stale; regenerate with driver_vendor.php");
}
