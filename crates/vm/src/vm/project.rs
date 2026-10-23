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
        let modules_dir = root.join("modules");
        if modules_dir.is_dir() {
            for m in list_dir(&modules_dir)? {
                if !m.is_dir() {
                    continue;
                }
                let mod_name = m.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                summary.modules.push(mod_name.clone());
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
                    message: format!("eine Datei, eine Klasse: hier {}", cf.module.classes.len()),
                    file,
                });
            }
            let declared = cf.module.str_lossy(cf.module.classes[0].name);
            if !declared.eq_ignore_ascii_case(&cf.expected) {
                return Err(ProjectError::Link {
                    code: "GRD101",
                    message: format!("Klasse `{declared}` passt nicht zum Pfad; erwartet `{}`", cf.expected),
                    file,
                });
            }
            let main = &cf.module.units[cf.module.main as usize];
            // a class file's main unit is only `Line…` ops plus the implicit return
            if main.ops.iter().any(|op| !matches!(op, crate::bytecode::Op::Line(_) | crate::bytecode::Op::PushNull | crate::bytecode::Op::Return | crate::bytecode::Op::Nop)) {
                return Err(ProjectError::Link { code: "GRD103", message: "Klassendateien enthalten keinen ausführbaren Code".into(), file });
            }
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
                                message: format!("Klasse `{}` nicht gefunden (kein passender Pfad unter modules/*/app)", m.str_lossy(*d)),
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
