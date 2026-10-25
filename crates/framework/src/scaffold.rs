//! `opengrid new <dir> [--name <Name>]` (TP1): a new project in the
//! Modules-Only structure that starts without further work — embedded
//! database, declarative schema sources, one module with a route.

use std::path::{Path, PathBuf};

/// Template files (path in the project, content); `__NAME__` is replaced.
const FILES: &[(&str, &str)] = &[
    (".gitignore", include_str!("../templates/new/gitignore")),
    ("config/development.yaml", include_str!("../templates/new/config/development.yaml")),
    ("config/test.yaml", include_str!("../templates/new/config/test.yaml")),
    ("config/production.yaml", include_str!("../templates/new/config/production.yaml")),
    ("database/sources/main.surql", include_str!("../templates/new/database/sources/main.surql")),
    ("modules/welcome/app/Controllers/WelcomeController.grd", include_str!("../templates/new/modules/welcome/app/Controllers/WelcomeController.grd")),
    ("modules/welcome/routes/web.grd", include_str!("../templates/new/modules/welcome/routes/web.grd")),
    ("modules/welcome/module.toml", include_str!("../templates/new/modules/welcome/module.toml")),
];

/// Create the project in `dir` (must not exist or be empty). `name` is the
/// application name (`settings.app.name`); default: the directory name.
pub fn new_project(dir: &Path, name: Option<&str>) -> Result<Vec<PathBuf>, String> {
    if dir.exists() && std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.next().is_some() {
        return Err(format!("{} exists and is not empty", dir.display()));
    }
    let name = match name {
        Some(n) => n.to_string(),
        None => dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "OpenGrid".into()),
    };
    if name.is_empty() || name.chars().any(|c| c.is_control() || c == '"' || c == '\\' || c == ':') {
        return Err(format!("invalid application name {name:?}"));
    }
    let mut created = Vec::new();
    for (path, content) in FILES {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&target, content.replace("__NAME__", &name)).map_err(|e| format!("{}: {e}", target.display()))?;
        created.push(PathBuf::from(path));
    }
    Ok(created)
}
