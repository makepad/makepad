//! Git identities sampled by the live watcher, never by an application thread.
//! A linked worktree owns HEAD and index.lock; its refs live in commondir.
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct Repositories {
    dirs: BTreeSet<PathBuf>,
    parents: HashMap<PathBuf, Option<PathBuf>>,
    /// each git dir's worktree (the folder its `.git` is in)
    roots: HashMap<PathBuf, PathBuf>,
}

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot {
    pub busy: bool,
    identities: Vec<(PathBuf, super::inputs::Stamp, Option<String>)>,
}

impl Repositories {
    pub fn include<'a>(&mut self, paths: impl Iterator<Item = &'a str>) {
        for path in paths {
            if crate::vfs::is_virtual(path) { continue; }
            let mut at = Path::new(path).parent();
            let mut visited = Vec::new();
            let mut found = None;
            while let Some(dir) = at {
                if let Some(cached) = self.parents.get(dir) {
                    found = cached.clone();
                    break;
                }
                visited.push(dir.to_path_buf());
                let dot = dir.join(".git");
                // as git: a `.git` folder without HEAD is no repository, and the search goes on
                // up (an empty one would read as busy forever and hold every edit back)
                if dot.is_dir() && dot.join("HEAD").is_file() {
                    self.roots.insert(dot.clone(), dir.to_path_buf());
                    found = Some(dot);
                    break;
                }
                if let Ok(text) = std::fs::read_to_string(&dot) {
                    if let Some(gitdir) = text.trim().strip_prefix("gitdir:") {
                        let gitdir = dir.join(gitdir.trim());
                        self.roots.insert(gitdir.clone(), dir.to_path_buf());
                        found = Some(gitdir);
                        break;
                    }
                }
                at = dir.parent();
            }
            for dir in visited { self.parents.insert(dir, found.clone()); }
            if let Some(dir) = found { self.dirs.insert(dir); }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut out = Snapshot { busy: false, identities: Vec::new() };
        for dir in &self.dirs {
            let common = out.read(dir.join("commondir")).map(|p| dir.join(p.trim())).unwrap_or_else(|| dir.clone());
            // Sampling the index identity also notices an operation that began
            // and ended between polls without moving HEAD (checkout of paths).
            for base in [dir, &common] {
                out.busy |= base.join("index.lock").exists();
                out.note(base.join("index"));
            }
            let mut value = out.read(dir.join("HEAD"));
            out.busy |= value.is_none() || dir.join("HEAD.lock").exists();
            let mut refs = BTreeSet::new();
            while let Some(name) = value.as_deref().and_then(|s| s.trim().strip_prefix("ref:")).map(str::trim) {
                let name = name.to_string();
                if !refs.insert(name.clone()) { out.busy = true; break; }
                let path = common.join(&name);
                out.busy |= path.with_file_name(format!("{}.lock", path.file_name().unwrap_or_default().to_string_lossy())).exists();
                value = out.read(path);
                if value.is_none() {
                    // A ref can be packed; both its value and the packed file's
                    // identity participate in the quiet window.
                    value = out.read(common.join("packed-refs")).and_then(|text| text.lines().find_map(|line| {
                        let (hash, reference) = line.split_once(' ')?;
                        (reference == name).then(|| hash.to_string())
                    }));
                }
            }
        }
        out
    }
}

impl Repositories {
    /// Each repository's worktree with the commit its HEAD names (40 hex digits): first those the
    /// `workspace` lies in, the innermost first (the first is the run's checkout: a repository in
    /// another's ignored folder has its own files), then the others. One whose HEAD names no
    /// commit (unborn, unreadable) is left out; None while a git operation runs in one (a lock is
    /// there).
    pub fn heads(&self, workspace: &str) -> Option<Vec<(String, String)>> {
        let mut out = Vec::new();
        for dir in &self.dirs {
            let common = std::fs::read_to_string(dir.join("commondir")).ok().map(|p| dir.join(p.trim())).unwrap_or_else(|| dir.clone());
            if dir.join("HEAD.lock").exists() || dir.join("index.lock").exists() || common.join("index.lock").exists() { return None; }
            let Ok(mut value) = std::fs::read_to_string(dir.join("HEAD")) else { continue };
            for _ in 0..8 {
                let Some(name) = value.trim().strip_prefix("ref:").map(|n| n.trim().to_string()) else { break };
                if common.join(format!("{name}.lock")).exists() { return None; }
                value = match std::fs::read_to_string(common.join(&name)) {
                    Ok(v) => v,
                    // a packed ref
                    Err(_) => std::fs::read_to_string(common.join("packed-refs")).ok().and_then(|t| t.lines()
                        .find_map(|line| line.split_once(' ').filter(|(_, r)| *r == name).map(|(h, _)| h.to_string()))).unwrap_or_default(),
                };
            }
            let sha = value.trim();
            if sha.len() != 40 || !sha.bytes().all(|c| c.is_ascii_hexdigit()) { continue; }
            // an input's path can hold `..` (the runner's `tools/run/runner/../src`): one worktree, one row
            let root = lexical(self.roots.get(dir).unwrap_or(dir)).to_string_lossy().into_owned();
            if !out.iter().any(|(r, _)| *r == root) { out.push((root, sha.to_string())); }
        }
        out.sort_by_key(|(root, _)| {
            let holds = Path::new(workspace).starts_with(root);
            (!holds, if holds { -(root.len() as i64) } else { root.len() as i64 })
        });
        Some(out)
    }
}

/// `p` without its `.` and `..` components, by the text (no file system look)
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => { if !out.pop() { out.push(".."); } }
            c => out.push(c),
        }
    }
    out
}

impl Snapshot {
    fn note(&mut self, path: PathBuf) {
        let stamp = super::stamp(&path.to_string_lossy());
        self.identities.push((path, stamp, None));
    }

    fn read(&mut self, path: PathBuf) -> Option<String> {
        let stamp = super::stamp(&path.to_string_lossy());
        let text = std::fs::read_to_string(&path).ok();
        self.identities.push((path, stamp, text.clone()));
        text
    }
}

#[cfg(test)]
mod tests {
    use super::Repositories;

    /// A `.git` folder without HEAD (git: not a repository) is passed over: the search goes on
    /// up, and the snapshot is not busy. It read as a repository whose HEAD never resolved, so
    /// every save waited for a quiet window that never came.
    #[test]
    fn a_git_folder_without_head_is_no_repository() {
        let root = std::env::temp_dir().join(format!("relax-git-no-head-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let outer = root.join("outer");
        std::fs::create_dir_all(outer.join(".git/refs/heads")).unwrap();
        std::fs::write(outer.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(outer.join(".git/refs/heads/main"), "0123456789abcdef0123456789abcdef01234567\n").unwrap();
        let inner = outer.join("inner");
        std::fs::create_dir_all(inner.join(".git")).unwrap();
        std::fs::create_dir_all(inner.join("src")).unwrap();
        let file = inner.join("src/main.rs");
        std::fs::write(&file, "fn main() {}\n").unwrap();

        let mut repos = Repositories::default();
        repos.include(std::iter::once(file.to_str().unwrap()));
        assert_eq!(repos.dirs.iter().cloned().collect::<Vec<_>>(), vec![outer.join(".git")], "the repository above the empty .git folder");
        assert!(!repos.snapshot().busy, "a repository with HEAD and its ref is not busy");

        // without the outer repository the search goes on up as it does from the scratch folder
        // (a repository above it, if the temp folder sits in one; none, and so not busy, elsewhere)
        std::fs::remove_dir_all(outer.join(".git")).unwrap();
        let mut above = Repositories::default();
        above.include(std::iter::once(root.join("probe").to_str().unwrap()));
        let mut repos = Repositories::default();
        repos.include(std::iter::once(file.to_str().unwrap()));
        assert_eq!(repos.dirs, above.dirs, "the repositories above the scratch folder, never the empty .git");
        if above.dirs.is_empty() {
            assert!(!repos.snapshot().busy, "no repository is not busy");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
