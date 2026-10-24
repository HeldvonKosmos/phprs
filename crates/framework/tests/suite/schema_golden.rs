//! R4a/R4b2: the Rust schema port must produce byte-identical output to the
//! platform's PHP classes (SurqlFileParser, SurqlSchema, SurqlGenerator).
//!
//! - Edge cases (`fixtures/schema_golden/cases/*/N.surql`, merged in file
//!   order) against `expected.out`, which PHP 8.4 produced with the platform
//!   classes (`fixtures/schema_golden/driver.php`). Always runs.
//! - When `php` and `refs/platform/schema-tooling` exist: the expectations are
//!   still what PHP prints, and the real platform schema files (confidential,
//!   read at run time, nothing stored) give the same output in PHP and Rust.

use std::path::{Path, PathBuf};
use std::process::Command;

use opengrid_framework::native::database::schema::ast::Schema;
use opengrid_framework::native::database::schema::generator::generate;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/schema_golden")
}

fn tooling() -> Option<PathBuf> {
    let t = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../refs/platform/schema-tooling");
    let php = Command::new("php").arg("-v").output().is_ok_and(|o| o.status.success());
    (php && t.join("Migration/SurqlFileParser.php").exists()).then_some(t)
}

fn cases() -> Vec<(String, Vec<PathBuf>, PathBuf)> {
    let mut dirs: Vec<_> = std::fs::read_dir(golden_dir().join("cases")).expect("cases").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.into_iter()
        .map(|d| {
            let mut files: Vec<_> = std::fs::read_dir(&d).expect("case").filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "surql")).collect();
            files.sort();
            (d.file_name().unwrap().to_string_lossy().into_owned(), files, d.join("expected.out"))
        })
        .collect()
}

fn rust_output(files: &[PathBuf]) -> String {
    let mut schema = Schema::default();
    for f in files {
        let text = std::fs::read_to_string(f).expect("read source");
        schema.merge_source(&text, &f.display().to_string());
    }
    generate(&schema)
}

fn php_output(tool: &Path, args: &[String]) -> String {
    let out = Command::new("php").arg(golden_dir().join("driver.php")).arg(tool).args(args).output().expect("run php");
    assert!(out.status.success(), "php failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf-8")
}

/// First differing line (1-based) and both lines, for a short message.
fn first_diff(a: &str, b: &str) -> String {
    for (i, (x, y)) in a.split('\n').zip(b.split('\n')).enumerate() {
        if x != y {
            return format!("line {}: rust {:?} / php {:?}", i + 1, x, y);
        }
    }
    format!("length differs: rust {} bytes / php {} bytes", a.len(), b.len())
}

#[test]
fn schema_port_matches_php_on_edge_cases() {
    let mut failures = Vec::new();
    for (name, files, expected) in cases() {
        let want = std::fs::read_to_string(&expected).expect("expected.out");
        let got = rust_output(&files);
        if got != want {
            failures.push(format!("{name}: {}", first_diff(&got, &want)));
        }
    }
    assert!(failures.is_empty(), "{} edge cases differ from PHP:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn golden_expectations_are_current_php_output() {
    let Some(tool) = tooling() else {
        eprintln!("skipped: php or refs/platform/schema-tooling not available");
        return;
    };
    for (name, files, expected) in cases() {
        let mut args = vec!["files".to_string()];
        args.extend(files.iter().map(|f| f.display().to_string()));
        let php = php_output(&tool, &args);
        assert_eq!(php, std::fs::read_to_string(&expected).expect("expected"), "{name}: expected.out is stale, regenerate with driver.php");
    }
}

#[test]
fn schema_port_matches_php_on_platform_files() {
    let Some(tool) = tooling() else {
        eprintln!("skipped: php or refs/platform/schema-tooling not available");
        return;
    };
    // MigratePush's order: main, all blueprints, all manually (glob sorts)
    let mut files = Vec::new();
    let main = tool.join("database/sources/main.surql");
    if main.exists() {
        files.push(main);
    }
    for kind in ["blueprint.surql", "manually.surql"] {
        let mut more: Vec<_> = std::fs::read_dir(tool.join("vendor/intelliprocessapps"))
            .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path().join("database/schema").join(kind)).filter(|p| p.exists()).collect())
            .unwrap_or_default();
        more.sort();
        files.extend(more);
    }
    assert!(!files.is_empty(), "no platform schema files");
    let php = php_output(&tool, &["project".to_string(), tool.display().to_string()]);
    let rust = rust_output(&files);
    assert!(rust == php, "platform schema differs from PHP ({} files): {}", files.len(), first_diff(&rust, &php));
}
