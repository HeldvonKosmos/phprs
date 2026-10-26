//! TP5: the test project's acceptance script `examples/test-app/tests/api.sh`
//! (curl + jq against a real server process, fresh embedded database,
//! restart) runs as part of the suite with the `opengrid` binary of this
//! build.

use std::path::Path;
use std::process::Command;

#[test]
fn test_app_api_script() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/test-app/tests/api.sh");
    let out = Command::new("bash").arg(&script).env("OPENGRID_BIN", env!("CARGO_BIN_EXE_opengrid")).output().expect("run api.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "api.sh failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("steps passed") && !stdout.contains("FAIL "), "{stdout}");
}

/// TEST2.6: the same script against the Laravel reproduction of the test app
/// (LV1, Octane): Laravel itself is the oracle for status codes, messages and
/// JSON shapes the script expects from OpenGrid.
#[test]
fn test_app_api_script_against_laravel() {
    let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bench/laravel-app");
    let php = Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success());
    if !php || !app.join("artisan").exists() || !app.join("rr").exists() {
        crate::oracle::missing(crate::oracle::Oracle::LaravelVendor, "php or bench/laravel-app with RoadRunner not available (bench/framework/laravel/setup.sh): api.sh not run against Laravel");
        return;
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/test-app/tests/api.sh");
    let out = Command::new("bash").arg(&script).args(["--app", "laravel", "--runtime", "octane"]).output().expect("run api.sh");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "api.sh --app laravel failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("steps passed") && !stdout.contains("FAIL "), "{stdout}");
}
