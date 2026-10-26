//! PKG1: the module manager. A module is a directory with a `module.toml`
//! (MOD1); `add` fetches one from a Git repository into `modules/<name>`,
//! pinned to the commit it got, and records it in `modules.lock` with a
//! checksum over its files. `verify` finds local changes to installed
//! modules, `remove` takes one out again. Checking that the project still
//! compiles and links with the new module (the GRD check) is the caller's
//! part: it needs the framework (`opengrid task module cmd:add …`).
//!
//! Git runs as the `git` command (shallow clone of a tag or branch; a
//! commit id is fetched in full and checked out).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use grd_vm::vm::project::{read_manifest, Manifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const LOCK_FILE: &str = "modules.lock";

/// One installed module in `modules.lock`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Locked {
    pub name: String,
    pub version: String,
    pub source: String,
    pub commit: String,
    /// sha256 over the module's files (relative path and content, sorted)
    pub checksum: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Lock {
    #[serde(default, rename = "module")]
    pub modules: Vec<Locked>,
}

impl Lock {
    pub fn read(root: &Path) -> Result<Lock, String> {
        let p = root.join(LOCK_FILE);
        if !p.exists() {
            return Ok(Lock::default());
        }
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        toml::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
    }

    pub fn write(&self, root: &Path) -> Result<(), String> {
        let mut text = String::from("# Written by `opengrid task module`; do not edit.\n\n");
        text.push_str(&toml::to_string(self).map_err(|e| e.to_string())?);
        std::fs::write(root.join(LOCK_FILE), text).map_err(|e| e.to_string())
    }

    pub fn get(&self, name: &str) -> Option<&Locked> {
        self.modules.iter().find(|m| m.name == name)
    }
}

fn git(args: &[&str], dir: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    let out = cmd.args(args).env("GIT_TERMINAL_PROMPT", "0").output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn is_commit_id(rev: &str) -> bool {
    rev.len() >= 7 && rev.len() <= 40 && rev.chars().all(|c| c.is_ascii_hexdigit())
}

/// sha256 over the files below `dir` (relative path, NUL, content), `.git` excluded.
pub fn checksum(dir: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort();
    let mut h = Sha256::new();
    for rel in files {
        h.update(rel.to_string_lossy().as_bytes());
        h.update([0]);
        h.update(std::fs::read(dir.join(&rel)).map_err(|e| e.to_string())?);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if p.is_dir() {
            collect(base, &p, out)?;
        } else {
            out.push(p.strip_prefix(base).unwrap_or(&p).to_path_buf());
        }
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect(from, from, &mut files)?;
    for rel in files {
        let dest = to.join(&rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(from.join(&rel), &dest).map_err(|e| format!("{}: {e}", rel.display()))?;
    }
    Ok(())
}

/// What `add` installed (already written to `modules/` and the lock file).
#[derive(Debug, Clone)]
pub struct Added {
    pub locked: Locked,
    pub manifest: Manifest,
    pub dir: PathBuf,
}

/// Fetch the module at `url` (tag, branch or commit `rev`; default branch
/// when None) into `modules/<name>` and lock it. Fails when the repository
/// has no `module.toml`, the module is installed already, or a dependency
/// in its manifest is missing in the project.
pub fn add(root: &Path, url: &str, rev: Option<&str>) -> Result<Added, String> {
    let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
    let checkout = tmp.path().join("repo");
    let co = checkout.to_string_lossy().into_owned();
    match rev {
        Some(r) if is_commit_id(r) => {
            git(&["clone", "--quiet", url, &co], None)?;
            git(&["checkout", "--quiet", r], Some(&checkout))?;
        }
        Some(r) => {
            git(&["clone", "--quiet", "--depth", "1", "--branch", r, url, &co], None)?;
        }
        None => {
            git(&["clone", "--quiet", "--depth", "1", url, &co], None)?;
        }
    }
    let commit = git(&["rev-parse", "HEAD"], Some(&checkout))?;
    let manifest = read_manifest(&checkout)?.ok_or_else(|| format!("{url}: no module.toml at the top of the repository"))?;
    let valid = !manifest.name.is_empty() && manifest.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if !valid {
        return Err(format!("{url}: invalid module name `{}` (lowercase letters, digits, _ and -)", manifest.name));
    }
    let dir = root.join("modules").join(&manifest.name);
    if dir.exists() {
        return Err(format!("module `{}` is already installed (modules/{}); remove it first", manifest.name, manifest.name));
    }
    for d in &manifest.depends {
        if !root.join("modules").join(d).is_dir() {
            return Err(format!("module `{}` needs `{d}`, which is not installed", manifest.name));
        }
    }
    copy_tree(&checkout, &dir)?;
    let locked = Locked { name: manifest.name.clone(), version: manifest.version.clone(), source: url.to_string(), commit, checksum: checksum(&dir)? };
    let mut lock = Lock::read(root)?;
    lock.modules.retain(|m| m.name != locked.name);
    lock.modules.push(locked.clone());
    lock.modules.sort_by(|a, b| a.name.cmp(&b.name));
    lock.write(root)?;
    Ok(Added { locked, manifest, dir })
}

/// Take a module out of `modules/` and the lock file. Refused while another
/// installed module depends on it.
pub fn remove(root: &Path, name: &str) -> Result<(), String> {
    let modules = root.join("modules");
    let dir = modules.join(name);
    if !dir.is_dir() || name.contains('/') || name.contains("..") {
        return Err(format!("module `{name}` is not installed"));
    }
    for other in std::fs::read_dir(&modules).map_err(|e| e.to_string())?.flatten() {
        let other_name = other.file_name().to_string_lossy().into_owned();
        if other_name != name
            && let Ok(Some(m)) = read_manifest(&other.path())
            && m.depends.iter().any(|d| d == name)
        {
            return Err(format!("module `{other_name}` depends on `{name}`"));
        }
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut lock = Lock::read(root)?;
    lock.modules.retain(|m| m.name != name);
    lock.write(root)
}

/// Installed modules whose files differ from the lock (name → reason).
pub fn verify(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut changed = BTreeMap::new();
    for m in Lock::read(root)?.modules {
        let dir = root.join("modules").join(&m.name);
        if !dir.is_dir() {
            changed.insert(m.name, "missing".into());
        } else if checksum(&dir)? != m.checksum {
            changed.insert(m.name, "changed locally".into());
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(dir: &Path, files: &[(&str, &str)]) -> String {
        std::fs::create_dir_all(dir).unwrap();
        for (p, t) in files {
            let f = dir.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        }
        let g = |a: &[&str]| git(a, Some(dir)).unwrap();
        g(&["init", "--quiet", "-b", "main"]);
        g(&["-c", "user.email=t@example.com", "-c", "user.name=t", "add", "."]);
        g(&["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "--quiet", "-m", "v1"]);
        g(&["tag", "v1.0.0"]);
        format!("file://{}", dir.display())
    }

    #[test]
    fn add_lock_verify_remove() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project");
        std::fs::create_dir_all(root.join("modules/auth")).unwrap();
        std::fs::write(root.join("modules/auth/module.toml"), "name = \"auth\"\n").unwrap();
        let url = repo(&base.path().join("blog-repo"), &[("module.toml", "name = \"blog\"\nversion = \"1.0.0\"\ndepends = [\"auth\"]\n"), ("app/Models/Post.grd", "<?grd\nnamespace Blog\\Models;\nfinal class Post {}\n")]);

        let added = add(&root, &url, Some("v1.0.0")).unwrap();
        assert_eq!((added.locked.name.as_str(), added.locked.version.as_str(), added.locked.commit.len()), ("blog", "1.0.0", 40));
        assert!(root.join("modules/blog/app/Models/Post.grd").exists() && !root.join("modules/blog/.git").exists());
        assert_eq!(Lock::read(&root).unwrap().get("blog").unwrap(), &added.locked);
        assert!(verify(&root).unwrap().is_empty());
        assert!(add(&root, &url, None).unwrap_err().contains("already installed"));
        // a commit id pins exactly
        std::fs::write(root.join("modules/blog/app/Models/Post.grd"), "<?grd\nnamespace Blog\\Models;\nfinal class Post { public int $x = 1; }\n").unwrap();
        assert_eq!(verify(&root).unwrap().get("blog").map(String::as_str), Some("changed locally"));
        assert!(remove(&root, "auth").unwrap_err().contains("module `blog` depends on `auth`"));
        remove(&root, "blog").unwrap();
        assert!(!root.join("modules/blog").exists() && Lock::read(&root).unwrap().get("blog").is_none());
        let again = add(&root, &url, Some(&added.locked.commit)).unwrap();
        assert_eq!(again.locked.commit, added.locked.commit);
    }

    #[test]
    fn refuses_repositories_without_manifest_or_dependencies() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project");
        std::fs::create_dir_all(root.join("modules")).unwrap();
        let bare = repo(&base.path().join("bare"), &[("README.md", "x")]);
        assert!(add(&root, &bare, None).unwrap_err().contains("no module.toml"));
        let needy = repo(&base.path().join("needy"), &[("module.toml", "name = \"shop\"\ndepends = [\"billing\"]\n")]);
        assert!(add(&root, &needy, None).unwrap_err().contains("needs `billing`"));
        assert!(!root.join("modules/shop").exists());
    }

    /// TEST2.7: a module directory removed by hand is reported as missing;
    /// module names that could leave `modules/` or break the convention are
    /// refused when adding, and `remove` does not follow paths.
    #[test]
    fn verify_reports_missing_modules_and_names_are_guarded() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("project");
        std::fs::create_dir_all(root.join("modules")).unwrap();
        let url = repo(&base.path().join("blog-repo"), &[("module.toml", "name = \"blog\"\nversion = \"1.0.0\"\n")]);
        add(&root, &url, None).unwrap();
        std::fs::remove_dir_all(root.join("modules/blog")).unwrap();
        assert_eq!(verify(&root).unwrap().into_iter().collect::<Vec<_>>(), vec![("blog".to_string(), "missing".to_string())]);

        for bad in ["../evil", "Blog", "a/b", "", "with space", "x.y"] {
            let dir = base.path().join(format!("bad-{}", bad.len()));
            let _ = std::fs::remove_dir_all(&dir);
            let url = repo(&dir, &[("module.toml", &format!("name = \"{bad}\"\n"))]);
            let err = add(&root, &url, None).unwrap_err();
            assert!(err.contains(&format!("invalid module name `{bad}`")), "{bad}: {err}");
        }
        assert!(!base.path().join("evil").exists(), "nothing written outside modules/");
        // remove: only a plain installed name
        std::fs::create_dir_all(base.path().join("outside")).unwrap();
        for name in ["../outside", "../../outside", "blog/../.."] {
            assert!(remove(&root, name).unwrap_err().contains("is not installed"), "{name}");
        }
        assert!(base.path().join("outside").exists() && root.join("modules").exists());
    }
}
