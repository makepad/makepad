//! Git identities sampled by the live watcher, never by an application thread.
//! A linked worktree owns HEAD and index.lock; its refs live in commondir.
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Default)]
pub(super) struct Repositories {
    dirs: BTreeSet<PathBuf>,
    parents: HashMap<PathBuf, Option<PathBuf>>,
    /// each git dir's worktree (the folder its `.git` is in)
    roots: HashMap<PathBuf, PathBuf>,
}

#[derive(PartialEq, Eq, Clone)]
pub(super) struct Snapshot {
    /// (repository, file, its stamp, its text): what a git operation writes, per repository (its
    /// git dir)
    identities: Vec<(PathBuf, PathBuf, super::inputs::Stamp, Option<String>)>,
    /// (repository, file, made): its locks (index.lock, HEAD.lock, a ref's), each with its change
    /// time, and a HEAD that names nothing (unreadable, a ref cycle), with none
    holds: Vec<(PathBuf, PathBuf, Option<SystemTime>)>,
}

impl Repositories {
    pub fn include<'a>(&mut self, paths: impl Iterator<Item = &'a str>) {
        for path in paths {
            // (a store's file is in no repository on disk; a folder mount's is in its checkout's)
            if crate::vfs::in_store(path) { continue; }
            let disk = crate::vfs::on_disk(path);
            let mut at = Path::new(&*disk).parent();
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
                        // the same for a `gitdir:` that names no repository (a worktree or a
                        // submodule checkout without its main repository): read, its HEAD never
                        // resolved and every save waited for a quiet window that never came (relax#33)
                        if gitdir.join("HEAD").is_file() {
                            self.roots.insert(gitdir.clone(), dir.to_path_buf());
                            found = Some(gitdir);
                            break;
                        }
                    }
                }
                at = dir.parent();
            }
            for dir in visited { self.parents.insert(dir, found.clone()); }
            if let Some(dir) = found { self.dirs.insert(dir); }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut out = Snapshot { identities: Vec::new(), holds: Vec::new() };
        for dir in &self.dirs {
            let common = out.read(dir, dir.join("commondir")).map(|p| dir.join(p.trim())).unwrap_or_else(|| dir.clone());
            // Sampling the index identity also notices an operation that began
            // and ended between polls without moving HEAD (checkout of paths).
            for base in [dir, &common] {
                out.lock(dir, base.join("index.lock"));
                out.note(dir, base.join("index"));
            }
            let mut value = out.read(dir, dir.join("HEAD"));
            if value.is_none() { out.holds.push((dir.clone(), dir.join("HEAD"), None)); }
            out.lock(dir, dir.join("HEAD.lock"));
            let mut refs = BTreeSet::new();
            while let Some(name) = value.as_deref().and_then(|s| s.trim().strip_prefix("ref:")).map(str::trim) {
                let name = name.to_string();
                if !refs.insert(name.clone()) {
                    out.holds.push((dir.clone(), common.join(&name), None));
                    break;
                }
                let path = common.join(&name);
                out.lock(dir, path.with_file_name(format!("{}.lock", path.file_name().unwrap_or_default().to_string_lossy())));
                value = out.read(dir, path);
                if value.is_none() {
                    // A ref can be packed; both its value and the packed file's
                    // identity participate in the quiet window.
                    value = out.read(dir, common.join("packed-refs")).and_then(|text| text.lines().find_map(|line| {
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
    fn note(&mut self, repo: &Path, path: PathBuf) {
        let stamp = super::stamp(&path.to_string_lossy());
        self.identities.push((repo.to_path_buf(), path, stamp, None));
    }

    fn read(&mut self, repo: &Path, path: PathBuf) -> Option<String> {
        let stamp = super::stamp(&path.to_string_lossy());
        let text = std::fs::read_to_string(&path).ok();
        self.identities.push((repo.to_path_buf(), path, stamp, text.clone()));
        text
    }

    fn lock(&mut self, repo: &Path, path: PathBuf) {
        if let Ok(m) = std::fs::metadata(&path) {
            self.holds.push((repo.to_path_buf(), path, m.modified().ok()));
        }
    }

    /// The lock that holds a save back (or the HEAD that names nothing): one a git operation can
    /// be writing for it. A lock made at or after the save's first change (`save`) can be its
    /// writer's; an older one holds only while its repository's HEAD, refs or index moved since
    /// the last settled snapshot (`settled`: an operation under way). An older lock in a
    /// repository that stands still is a crashed operation's (git starts none until it goes):
    /// the save is the person's, and it applies. Read as busy, such a lock held every save back
    /// for a quiet window that never came (relax#33).
    pub fn holding(&self, save: SystemTime, settled: &Snapshot) -> Option<&Path> {
        self.holds.iter()
            .find(|(repo, _, made)| made.is_some_and(|t| t >= save) || self.moved_since(repo, settled))
            .map(|(_, path, _)| path.as_path())
    }

    /// Whether `repo`'s identities differ from `settled`'s (a repository `settled` did not
    /// sample has not moved: it was added to the watch since)
    fn moved_since(&self, repo: &Path, settled: &Snapshot) -> bool {
        settled.identities.iter().any(|i| i.0 == repo)
            && self.identities.iter().filter(|i| i.0 == repo).ne(settled.identities.iter().filter(|i| i.0 == repo))
    }

    /// The first file whose identity or lock differs from `before`'s: what moved, for the line
    /// of a long wait
    pub fn moved_from<'a>(&'a self, before: &'a Snapshot) -> Option<&'a Path> {
        fn first<'a, T: PartialEq>(a: &'a [T], b: &'a [T], path: impl Fn(&'a T) -> &'a Path) -> Option<&'a Path> {
            (0..a.len().max(b.len())).find(|&k| a.get(k) != b.get(k)).and_then(|k| a.get(k).or(b.get(k))).map(path)
        }
        first(&self.identities, &before.identities, |i| i.1.as_path()).or_else(|| first(&self.holds, &before.holds, |h| h.1.as_path()))
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
        let now = repos.snapshot();
        assert!(now.holding(std::time::UNIX_EPOCH, &now).is_none(), "a repository with HEAD and its ref holds nothing");

        // without the outer repository the search goes on up as it does from the scratch folder
        // (a repository above it, if the temp folder sits in one; none, and so not busy, elsewhere)
        std::fs::remove_dir_all(outer.join(".git")).unwrap();
        let mut above = Repositories::default();
        above.include(std::iter::once(root.join("probe").to_str().unwrap()));
        let mut repos = Repositories::default();
        repos.include(std::iter::once(file.to_str().unwrap()));
        assert_eq!(repos.dirs, above.dirs, "the repositories above the scratch folder, never the empty .git");
        if above.dirs.is_empty() {
            let now = repos.snapshot();
            assert!(now.holding(std::time::UNIX_EPOCH, &now).is_none(), "no repository holds nothing");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
