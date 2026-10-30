//! Worktree dirty sets from the OS file monitor (libs/filesystem_watcher: FSEvents on the Mac,
//! inotify on Linux, ReadDirectoryChangesW on Windows), so a job submit or `devhub lanes` knows
//! what a worktree holds on top of its HEAD without scanning it.
//!
//! A tree (a worktree root; a nested `apps/commercial` is a tree of its own) is baselined once
//! with `git status` when it is first asked about and again when its HEAD moves (HEAD is read
//! from the files under `.git`, no git call); from then on every file event under it joins its
//! touched set. Its dirty set is the baseline plus what was touched, minus ignored paths (asked
//! of `git check-ignore` once per new path) and directories. A touched file whose content went
//! back to HEAD's is still listed (shipping it again is harmless). Lost events (an overflow, the
//! root changing) re-baseline every tree. One monitor serves the master's lane worktrees and a
//! node's tree alike.

use makepad_filesystem_watcher::{ExcludePolicy, FileSystemEvent, FileSystemEventKind, FileSystemWatcher, WatchRoot, WatcherConfig};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What a tree holds on top of its HEAD.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dirty {
    pub head: String,
    /// the checked-out branch (empty: detached)
    pub branch: String,
    /// files (and symlinks) that differ from HEAD or are untracked, relative to the tree
    pub changed: Vec<String>,
    /// tracked paths that are gone
    pub deleted: Vec<String>,
}

#[derive(Default)]
struct Tree {
    head: String,
    base: BTreeSet<String>,
    touched: BTreeSet<String>,
    ignored: BTreeMap<String, bool>,
    stale: bool,
    /// the wall time of the last file event under it
    last_ms: u64,
}

#[derive(Default)]
struct Inner {
    trees: BTreeMap<PathBuf, Tree>,
}

pub struct Monitor {
    inner: Arc<Mutex<Inner>>,
    watcher: Option<FileSystemWatcher>,
    /// what it watches: a tree outside them is re-baselined on every ask
    roots: Vec<PathBuf>,
    /// why the OS monitor is not running (every ask then re-baselines)
    pub error: Option<String>,
}

/// The tree's `.git` dir (a linked worktree's `.git` is a file naming it) and its common dir.
fn git_dirs(tree: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot = tree.join(".git");
    let dir = if dot.is_dir() {
        dot
    } else {
        let text = std::fs::read_to_string(&dot).ok()?;
        let p = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        if p.is_absolute() {
            p
        } else {
            tree.join(p)
        }
    };
    let common = match std::fs::read_to_string(dir.join("commondir")) {
        Ok(c) => {
            let c = PathBuf::from(c.trim());
            if c.is_absolute() {
                c
            } else {
                dir.join(c)
            }
        }
        Err(_) => dir.clone(),
    };
    Some((dir, common))
}

/// HEAD's commit and branch, from the files (loose ref, else packed-refs).
pub fn read_head(tree: &Path) -> Option<(String, String)> {
    let (dir, common) = git_dirs(tree)?;
    let head = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(r) = head.strip_prefix("ref:").map(str::trim) else { return Some((head.to_string(), String::new())) };
    let branch = r.strip_prefix("refs/heads/").unwrap_or(r).to_string();
    for base in [&dir, &common] {
        if let Ok(s) = std::fs::read_to_string(base.join(r)) {
            return Some((s.trim().to_string(), branch));
        }
    }
    let packed = std::fs::read_to_string(common.join("packed-refs")).ok()?;
    let sha = packed.lines().find_map(|l| l.strip_suffix(r).map(str::trim).filter(|s| s.len() == 40))?;
    Some((sha.to_string(), branch))
}

fn git(tree: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    use std::io::Write;
    let mut cmd = makepad_remote::group::command("git");
    cmd.arg("-C").arg(tree).args(args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    cmd.stdin(if input.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() });
    let mut child = cmd.spawn().map_err(|e| format!("git: {e}"))?;
    if let (Some(data), Some(mut sin)) = (input, child.stdin.take()) {
        let _ = sin.write_all(data);
    }
    let out = child.wait_with_output().map_err(|e| format!("git: {e}"))?;
    // check-ignore exits 1 when nothing is ignored
    if !out.status.success() && out.status.code() != Some(1) {
        return Err(format!("git {} failed in {}", args.first().unwrap_or(&""), tree.display()));
    }
    Ok(out.stdout)
}

/// `git status` of the tree: every path that differs from HEAD or is untracked (not ignored).
fn baseline(tree: &Path) -> Result<BTreeSet<String>, String> {
    let out = git(tree, &["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all", "--ignore-submodules=all"], None)?;
    Ok(String::from_utf8_lossy(&out).split('\0').filter(|e| e.len() > 3).map(|e| e[3..].to_string()).collect())
}

impl Monitor {
    /// Watches `roots` (directories holding worktrees, or a tree itself).
    pub fn start(roots: &[PathBuf]) -> Arc<Monitor> {
        let inner = Arc::new(Mutex::new(Inner::default()));
        let i2 = inner.clone();
        let dirs: Vec<PathBuf> = roots.iter().filter(|r| r.is_dir()).map(|r| r.canonicalize().unwrap_or_else(|_| r.clone())).collect();
        let roots: Vec<WatchRoot> = dirs.iter().map(|d| WatchRoot { mount: "lanes".into(), path: d.clone() }).collect();
        let config = WatcherConfig { exclude: ExcludePolicy::default().dir_name("local").dir_name("node_modules"), startup_rescan: false, ..WatcherConfig::default() };
        let started = FileSystemWatcher::start_with_journal(roots, config, move |e: &FileSystemEvent| {
            on_event(&i2, e);
            true
        });
        let (watcher, error) = match started {
            Ok((w, _)) => (Some(w), None),
            Err(e) => (None, Some(e)),
        };
        Arc::new(Monitor { inner, watcher, roots: dirs, error })
    }

    pub fn watching(&self) -> bool {
        self.watcher.is_some()
    }

    /// The trees asked about so far.
    pub fn trees(&self) -> Vec<PathBuf> {
        self.inner.lock().unwrap().trees.keys().cloned().collect()
    }

    /// The wall time of the last file event in a tree it follows (0: none seen).
    pub fn last_event_ms(&self, tree: &Path) -> u64 {
        let t = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
        self.inner.lock().unwrap().trees.get(&t).map_or(0, |t| t.last_ms)
    }

    /// Stops following a tree (it was removed).
    pub fn forget(&self, tree: &Path) {
        let t = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
        self.inner.lock().unwrap().trees.remove(&t);
    }

    /// The tree's dirty set now (`tree`: a worktree root; for the makepad tree, paths under a
    /// nested `apps/commercial` belong to that tree and are left out).
    pub fn dirty(&self, tree: &Path) -> Result<Dirty, String> {
        let root = tree.canonicalize().map_err(|e| format!("{}: {e}", tree.display()))?;
        let (head, branch) = read_head(&root).ok_or_else(|| format!("{}: not a git worktree", root.display()))?;
        let fresh = {
            let mut g = self.inner.lock().unwrap();
            let t = g.trees.entry(root.clone()).or_default();
            let watched = self.watcher.is_some() && self.roots.iter().any(|r| root.starts_with(r));
            if t.head != head || t.stale || !watched {
                let last_ms = t.last_ms;
                *t = Tree { head: head.clone(), last_ms, ..Default::default() };
                true
            } else {
                false
            }
        };
        if fresh {
            let base = baseline(&root)?;
            let mut g = self.inner.lock().unwrap();
            if let Some(t) = g.trees.get_mut(&root).filter(|t| t.head == head) {
                t.base = base;
            }
        }
        let nested = root.join("apps/commercial/.git").exists();
        let (cands, unknown): (Vec<String>, Vec<String>) = {
            let g = self.inner.lock().unwrap();
            let t = g.trees.get(&root).ok_or("the tree was forgotten")?;
            let all: Vec<String> = t.base.union(&t.touched).filter(|p| !(nested && p.starts_with("apps/commercial/"))).cloned().collect();
            let unknown = t.touched.iter().filter(|p| !t.base.contains(*p) && !t.ignored.contains_key(*p)).cloned().collect();
            (all, unknown)
        };
        if !unknown.is_empty() {
            let mut input = Vec::new();
            for p in &unknown {
                input.extend_from_slice(p.as_bytes());
                input.push(0);
            }
            let out = git(&root, &["check-ignore", "-z", "--stdin"], Some(&input))?;
            let ignored: BTreeSet<String> = String::from_utf8_lossy(&out).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect();
            let mut g = self.inner.lock().unwrap();
            if let Some(t) = g.trees.get_mut(&root) {
                for p in unknown {
                    let ig = ignored.contains(&p);
                    t.ignored.insert(p, ig);
                }
            }
        }
        let mut d = Dirty { head, branch, ..Default::default() };
        let mut missing = Vec::new();
        {
            let g = self.inner.lock().unwrap();
            let t = g.trees.get(&root).ok_or("the tree was forgotten")?;
            for p in cands {
                if t.ignored.get(&p) == Some(&true) {
                    continue;
                }
                match std::fs::symlink_metadata(root.join(&p)) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) => d.changed.push(p),
                    Err(_) => missing.push(p),
                }
            }
        }
        // a path that is gone counts only when HEAD has it (an untracked file came and went)
        if !missing.is_empty() {
            let mut args = vec!["ls-tree", "-z", "--name-only", "HEAD", "--"];
            args.extend(missing.iter().map(String::as_str));
            let out = git(&root, &args, None)?;
            let tracked: BTreeSet<String> = String::from_utf8_lossy(&out).split('\0').filter(|s| !s.is_empty()).map(str::to_string).collect();
            d.deleted = missing.into_iter().filter(|p| tracked.contains(p)).collect();
        }
        Ok(d)
    }
}

fn on_event(inner: &Mutex<Inner>, e: &FileSystemEvent) {
    let mut g = inner.lock().unwrap();
    if let FileSystemEventKind::RescanRequired { .. } = e.kind {
        for t in g.trees.values_mut() {
            t.stale = true;
        }
        return;
    }
    if e.is_dir == Some(true) {
        return;
    }
    let mut paths = vec![e.path.clone()];
    if let FileSystemEventKind::Renamed { from } = &e.kind {
        paths.push(from.clone());
    }
    for p in paths {
        // the deepest tree that holds it (a nested commercial tree wins over its parent)
        let Some((root, rel)) = p.ancestors().skip(1).find_map(|a| g.trees.contains_key(a).then(|| (a.to_path_buf(), p.strip_prefix(a).ok().map(|r| r.to_string_lossy().replace('\\', "/"))))) else { continue };
        let Some(rel) = rel.filter(|r| !r.is_empty()) else { continue };
        if let Some(t) = g.trees.get_mut(&root) {
            t.ignored.remove(&rel);
            t.touched.insert(rel);
            t.last_ms = crate::proto::wall_ms();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn dirty_sets_follow_edits_without_a_rescan() {
        let base = std::env::temp_dir().join(format!("devhub-monitor-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tree = base.join("lane");
        std::fs::create_dir_all(tree.join("src")).unwrap();
        g(&tree, &["init", "-q", "-b", "rc-x"]);
        g(&tree, &["config", "user.email", "t@t"]);
        g(&tree, &["config", "user.name", "t"]);
        std::fs::write(tree.join("src/a.rs"), "a").unwrap();
        std::fs::write(tree.join("b.rs"), "b").unwrap();
        std::fs::write(tree.join(".gitignore"), "out/\n").unwrap();
        g(&tree, &["add", "."]);
        g(&tree, &["commit", "-q", "-m", "x"]);
        let m = Monitor::start(&[base.clone()]);
        let d = m.dirty(&tree).unwrap();
        assert_eq!((d.branch.as_str(), d.head.len(), d.changed.len(), d.deleted.len()), ("rc-x", 40, 0, 0));
        std::fs::write(tree.join("untracked.rs"), "u").unwrap();
        std::fs::write(tree.join("src/a.rs"), "a2").unwrap();
        std::fs::remove_file(tree.join("b.rs")).unwrap();
        std::fs::create_dir_all(tree.join("out")).unwrap();
        std::fs::write(tree.join("out/gen.bin"), "x").unwrap();
        // the OS monitor delivers within its latency window; a monitor that could not start re-baselines
        let want = (vec!["src/a.rs".to_string(), "untracked.rs".to_string()], vec!["b.rs".to_string()]);
        let mut got = (Vec::new(), Vec::new());
        for _ in 0..50 {
            let d = m.dirty(&tree).unwrap();
            got = (d.changed, d.deleted);
            if got == want {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(got, want);
        // a commit moves HEAD: the next ask re-baselines
        g(&tree, &["add", "-A"]);
        g(&tree, &["commit", "-q", "-m", "y"]);
        let d = m.dirty(&tree).unwrap();
        assert_eq!((d.changed.len(), d.deleted.len()), (0, 0), "{d:?}");
        drop(m);
        let _ = std::fs::remove_dir_all(&base);
    }
}
