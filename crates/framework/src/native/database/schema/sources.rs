//! Source files of the schema in merge order (port of
//! `MigratePush::collectSourceFiles`): the framework fragment (embedded,
//! R4e) → `database/sources/main.surql` → per module
//! `modules/<m>/database/schema/blueprint.surql`, then `schema.surql` (the
//! hand-written module schema, D10; Blueprint is deferred, BP1) → every
//! `modules/*/database/schema/manually.surql`. Modules are sorted bytewise
//! by directory name (PHP `glob` sorts; `read_dir` does not). All
//! blueprint/schema files come before all manually files.

use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Path relative to the project root, or `{framework}`.
    pub label: String,
    pub text: String,
}

pub const MAIN: &str = "database/sources/main.surql";
pub const BLUEPRINT: &str = "blueprint.surql";
/// Hand-written schema of a module (D10).
pub const SCHEMA: &str = "schema.surql";
pub const MANUALLY: &str = "manually.surql";

pub fn collect(root: &Path, framework: Option<&str>) -> std::io::Result<Vec<Source>> {
    let mut out = Vec::new();
    if let Some(text) = framework {
        out.push(Source { label: "{framework}".into(), text: text.to_string() });
    }
    let main = root.join(MAIN);
    if main.is_file() {
        out.push(Source { label: MAIN.into(), text: std::fs::read_to_string(&main)? });
    }
    let mut modules: Vec<String> = Vec::new();
    let dir = root.join("modules");
    if dir.is_dir() {
        for e in std::fs::read_dir(&dir)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                modules.push(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    modules.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    for group in [&[BLUEPRINT, SCHEMA][..], &[MANUALLY][..]] {
        for m in &modules {
            for file in group {
                let rel = format!("modules/{m}/database/schema/{file}");
                let p = root.join(&rel);
                if p.is_file() {
                    out.push(Source { label: rel, text: std::fs::read_to_string(&p)? });
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, text: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn merge_order() {
        let root = std::env::temp_dir().join(format!("og-schema-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        write(&root, "database/sources/main.surql", "-- main");
        for m in ["zeta", "Alpha", "billing"] {
            write(&root, &format!("modules/{m}/database/schema/blueprint.surql"), "");
            write(&root, &format!("modules/{m}/database/schema/manually.surql"), "");
        }
        write(&root, "modules/billing/database/schema/schema.surql", "");
        write(&root, "modules/notes/database/schema/schema.surql", "");
        write(&root, "modules/nodb/app/X.grd", "");
        let labels: Vec<String> = collect(&root, Some("-- fw")).unwrap().into_iter().map(|s| s.label).collect();
        assert_eq!(
            labels,
            vec![
                "{framework}",
                "database/sources/main.surql",
                "modules/Alpha/database/schema/blueprint.surql",
                "modules/billing/database/schema/blueprint.surql",
                "modules/billing/database/schema/schema.surql",
                "modules/notes/database/schema/schema.surql",
                "modules/zeta/database/schema/blueprint.surql",
                "modules/Alpha/database/schema/manually.surql",
                "modules/billing/database/schema/manually.surql",
                "modules/zeta/database/schema/manually.surql",
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_files_are_fine() {
        let root = std::env::temp_dir().join(format!("og-schema-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(collect(&root, None).unwrap().is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
