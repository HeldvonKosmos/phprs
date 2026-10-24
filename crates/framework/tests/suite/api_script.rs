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
