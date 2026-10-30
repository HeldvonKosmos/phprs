//! Project loading: the "modules only" tree of DIALECT §2/§3.
//!
//! ```text
//! root/
//! ├── modules/<module>/app/<Folder>/<Class>.grd   class <Module>\<Folder>\<Class>
//! ├── modules/<module>/routes/*.grd               top-level code (routes)
//! └── routes/*.grd                                global routes
//! ```
//! Files are compiled, checked against the naming convention (GRD101 to
//! GRD103 are link errors), ordered by class dependencies across files and
//! loaded; then the route files run.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::Vm;
use crate::bytecode::Module;
use crate::compile::{compile_source, CompileError};
use crate::error::Throw;

#[derive(Debug)]
pub enum ProjectError {
    Io(String),
    Compile(CompileError),
    Link { code: &'static str, message: String, file: String },
    Runtime(Throw),
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectError::Io(m) => write!(f, "{m}"),
            ProjectError::Compile(e) => write!(f, "{e}"),
            ProjectError::Link { code, message, file } => write!(f, "error[{code}]: {message}\n  --> {file}"),
            ProjectError::Runtime(t) => write!(f, "{t}"),
        }
    }
}

/// MOD1: `modules/<m>/module.toml`. With a manifest a module may only use
/// classes of itself, of the modules in `depends`, of the framework
/// (`OpenGrid\…`) and global classes; without one nothing is enforced yet.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub depends: Vec<String>,
}

/// The manifest of a module directory, if it has one.
pub fn read_manifest(module_dir: &Path) -> Result<Option<Manifest>, String> {
    let path = module_dir.join("module.toml");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

/// Namespaces of other modules that `m` mentions (class references, imports,
/// `::class` and string constants are fully qualified after compiling).
fn mentioned_modules(m: &Module, namespaces: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let texts = m.strings.iter().map(|b| String::from_utf8_lossy(b).into_owned()).chain(m.consts.iter().filter_map(|c| match c {
        crate::bytecode::Const::Str(b) => Some(String::from_utf8_lossy(b).into_owned()),
        _ => None,
    }));
    for t in texts {
        let t = t.trim_start_matches('\\');
        let Some((first, _)) = t.split_once('\\') else { continue };
        if let Some(module) = namespaces.get(&first.to_ascii_lowercase()) {
            out.push((module.clone(), t.to_string()));
        }
    }
    out
}

#[derive(Debug, Default)]
pub struct ProjectSummary {
    pub modules: Vec<String>,
    pub class_files: usize,
    pub route_files: usize,
    pub classes: Vec<String>,
}

/// `tasks` → `Tasks`, `master_data` / `master-data` → `MasterData`.
pub fn studly(s: &str) -> String {
    s.split(['_', '-', ' '])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn list_dir(p: &Path) -> Result<Vec<PathBuf>, ProjectError> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(p)
        .map_err(|e| ProjectError::Io(format!("{}: {e}", p.display())))?
        .flatten()
        .map(|e| e.path())
        .collect();
    out.sort();
    Ok(out)
}

fn walk_grd(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ProjectError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for p in list_dir(dir)? {
        if p.is_dir() {
            walk_grd(&p, out)?;
        } else if p.extension().is_some_and(|e| e == "grd") {
            out.push(p);
        }
    }
    Ok(())
}

struct ClassFile {
    path: PathBuf,
    expected: String,
    module: Module,
}

impl Vm {
    /// Load a project tree and run its route files. Classes are linked in
    /// dependency order across files.
    pub fn load_project(&mut self, root: &Path) -> Result<ProjectSummary, ProjectError> {
        let mut summary = ProjectSummary::default();
        let mut class_files: Vec<ClassFile> = Vec::new();
        let mut route_files: Vec<PathBuf> = Vec::new();
        let mut manifests: HashMap<String, Option<Manifest>> = HashMap::new();
        let modules_dir = root.join("modules");
        if modules_dir.is_dir() {
            for m in list_dir(&modules_dir)? {
                if !m.is_dir() {
                    continue;
                }
                let mod_name = m.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                summary.modules.push(mod_name.clone());
                let manifest = read_manifest(&m).map_err(|e| ProjectError::Link { code: "GRD107", message: e, file: m.join("module.toml").to_string_lossy().into_owned() })?;
                if let Some(mf) = &manifest
                    && mf.name != mod_name
                {
                    return Err(ProjectError::Link { code: "GRD107", message: format!("the manifest names `{}`, the directory is called `{mod_name}`", mf.name), file: m.join("module.toml").to_string_lossy().into_owned() });
                }
                manifests.insert(mod_name.clone(), manifest);
                let app = m.join("app");
                let mut files = Vec::new();
                walk_grd(&app, &mut files)?;
                for f in files {
                    let rel = f.strip_prefix(&app).unwrap_or(&f).with_extension("");
                    let mut parts = vec![studly(&mod_name)];
                    for c in rel.components() {
                        parts.push(c.as_os_str().to_string_lossy().into_owned());
                    }
                    let expected = parts.join("\\");
                    let source = std::fs::read(&f).map_err(|e| ProjectError::Io(format!("{}: {e}", f.display())))?;
                    let module = compile_source(&f.to_string_lossy(), &source).map_err(ProjectError::Compile)?;
                    class_files.push(ClassFile { path: f, expected, module });
                }
                walk_grd(&m.join("routes"), &mut route_files)?;
            }
        }
        walk_grd(&root.join("routes"), &mut route_files)?;

        // naming convention
        for cf in &class_files {
            let file = cf.path.to_string_lossy().into_owned();
            if cf.module.classes.len() != 1 {
                return Err(ProjectError::Link {
                    code: "GRD102",
                    message: format!("one file, one class: this one has {}", cf.module.classes.len()),
                    file,
                });
            }
            let declared = cf.module.str_lossy(cf.module.classes[0].name);
            if !declared.eq_ignore_ascii_case(&cf.expected) {
                return Err(ProjectError::Link {
                    code: "GRD101",
                    message: format!("class `{declared}` does not match the path; expected `{}`", cf.expected),
                    file,
                });
            }
            let main = &cf.module.units[cf.module.main as usize];
            // a class file's main unit is only `Line…` ops plus the implicit return
            if main.ops.iter().any(|op| !matches!(op, crate::bytecode::Op::Line(_) | crate::bytecode::Op::PushNull | crate::bytecode::Op::Return | crate::bytecode::Op::Nop)) {
                return Err(ProjectError::Link { code: "GRD103", message: "class files contain no executable code".into(), file });
            }
        }

        // MOD1: module boundaries (only modules with a manifest)
        let namespaces: HashMap<String, String> = manifests.keys().map(|m| (studly(m).to_ascii_lowercase(), m.clone())).collect();
        for (m, mf) in &manifests {
            if let Some(mf) = mf {
                for d in &mf.depends {
                    if !manifests.contains_key(d) {
                        return Err(ProjectError::Link { code: "GRD107", message: format!("module `{m}` depends on `{d}`, which does not exist"), file: modules_dir.join(m).join("module.toml").to_string_lossy().into_owned() });
                    }
                }
            }
        }
        let boundary = |module_name: &str, compiled: &Module, file: &Path| -> Result<(), ProjectError> {
            let Some(Some(mf)) = manifests.get(module_name) else { return Ok(()) };
            for (other, name) in mentioned_modules(compiled, &namespaces) {
                if other != module_name && !mf.depends.contains(&other) {
                    return Err(ProjectError::Link {
                        code: "GRD106",
                        message: format!("module `{module_name}` uses `{name}` from module `{other}` without naming it under depends in modules/{module_name}/module.toml"),
                        file: file.to_string_lossy().into_owned(),
                    });
                }
            }
            Ok(())
        };
        for cf in &class_files {
            let module_name = cf.path.strip_prefix(&modules_dir).ok().and_then(|p| p.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
            boundary(&module_name, &cf.module, &cf.path)?;
        }

        // dependency order across files (parent, interfaces, traits)
        let mut owner: HashMap<Vec<u8>, usize> = HashMap::new();
        for (i, cf) in class_files.iter().enumerate() {
            let m = &cf.module;
            for c in &m.classes {
                owner.insert(m.str(c.name).to_ascii_lowercase(), i);
            }
        }
        let mut deps: Vec<HashSet<usize>> = vec![HashSet::new(); class_files.len()];
        for (i, cf) in class_files.iter().enumerate() {
            let m = &cf.module;
            for c in &m.classes {
                for d in c.parent.iter().chain(c.interfaces.iter()).chain(c.traits.iter()) {
                    let name = m.str(*d).to_ascii_lowercase();
                    if self.lookup_class(&name).is_some() {
                        continue;
                    }
                    match owner.get(&name) {
                        Some(&j) if j != i => {
                            deps[i].insert(j);
                        }
                        Some(_) => {}
                        None => {
                            return Err(ProjectError::Link {
                                code: "GRD104",
                                message: format!("class `{}` not found (no matching path under modules/*/app)", m.str_lossy(*d)),
                                file: cf.path.to_string_lossy().into_owned(),
                            });
                        }
                    }
                }
            }
        }
        let mut loaded = vec![false; class_files.len()];
        let mut order = Vec::new();
        while order.len() < class_files.len() {
            let before = order.len();
            for i in 0..class_files.len() {
                if !loaded[i] && deps[i].iter().all(|j| loaded[*j]) {
                    loaded[i] = true;
                    order.push(i);
                }
            }
            if order.len() == before {
                let i = loaded.iter().position(|l| !*l).expect("pending");
                return Err(ProjectError::Link {
                    code: "GRD105",
                    message: "zyklische Vererbung zwischen Dateien".into(),
                    file: class_files[i].path.to_string_lossy().into_owned(),
                });
            }
        }
        summary.class_files = class_files.len();
        let mut modules: Vec<Option<Module>> = class_files.into_iter().map(|cf| Some(cf.module)).collect();
        for i in order {
            let m = modules[i].take().expect("module");
            for c in &m.classes {
                summary.classes.push(m.str_lossy(c.name));
            }
            self.load_module(m).map_err(ProjectError::Runtime)?;
        }

        // routes: compile, load, run
        for rf in &route_files {
            let source = std::fs::read(rf).map_err(|e| ProjectError::Io(format!("{}: {e}", rf.display())))?;
            let module = compile_source(&rf.to_string_lossy(), &source).map_err(ProjectError::Compile)?;
            if let Ok(rel) = rf.strip_prefix(&modules_dir)
                && let Some(m) = rel.components().next()
            {
                boundary(&m.as_os_str().to_string_lossy(), &module, rf)?;
            }
            self.load_module(module).map_err(ProjectError::Runtime)?;
            let idx = (self.modules.len() - 1) as u32;
            self.run_main(idx).map_err(ProjectError::Runtime)?;
        }
        summary.route_files = route_files.len();
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn studly_case() {
        assert_eq!(super::studly("tasks"), "Tasks");
        assert_eq!(super::studly("master_data"), "MasterData");
        assert_eq!(super::studly("master-data"), "MasterData");
    }
}
