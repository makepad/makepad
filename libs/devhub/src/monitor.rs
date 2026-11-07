//! Worktree dirty sets from the OS file monitor (libs/filesystem_watcher: FSEvents on the Mac,
//! inotify on Linux, ReadDirectoryChangesW on Windows), so a job submit or `devhub lanes` knows
//! what a worktree holds on top of its HEAD without scanning it.
//!
//! A tree (a worktree root; a nested repository's `apps/<name>` is a tree of its own) is baselined once
//! with a status (makepad-git, in process) when it is first asked about and again when its HEAD
//! moves (HEAD is read from the files under `.git`); from then on every file event under it joins
//! its touched set. Its dirty set is the baseline plus what was touched, minus ignored paths and
//! directories. Touched files get a status of those paths only at the next ask and join the
//! baseline or leave it: an edit undone is no work (a reverted file kept a lane dirty for good,
//! refusing `lane remove`, 11-02). Lost events (an overflow, the root changing) re-baseline every
//! tree. Nothing here runs git. One monitor serves the master's lane worktrees and a node's tree
//! alike.
//!
//! The OS delivers events late (FSEvents after its 0.1 s window), so a set read right after a write
//! can miss it: a snapshot first asks [`Monitor::settle`], which writes a cookie file into the
//! watched root and waits to hear it. A root's events come in order, so every earlier write of
//! the tree has been heard by then.

mod tracked;

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
    tracked: Option<tracked::Tracked>,
    tracked_touched: BTreeSet<String>,
    tracked_stale: bool,
    head: String,
    base: BTreeSet<String>,
    /// `base` was read at `head` (a tree asked about for the first time has none yet)
    based: bool,
    /// paths with events since their status was last read, and the event's number
    touched: BTreeMap<String, u64>,
    /// file events so far
    seq: u64,
    stale: bool,
    /// the wall time of the last file event under it
    last_ms: u64,
}

/// Hears every file event in a tree it follows: (the tree, the path in it).
pub type Listener = Arc<dyn Fn(&Path, &str) + Send + Sync>;

#[derive(Default)]
struct Inner {
    trees: BTreeMap<PathBuf, Tree>,
    listeners: Vec<Listener>,
    /// cookies heard that their [`Monitor::settle`] has not taken yet
    cookies: BTreeSet<u64>,
}

/// A cookie's file name is this and its number.
const COOKIE_PREFIX: &str = ".devhub-settle-";
/// What a cookie unheard this long says: the monitor lost its events, so the trees are read again.
const COOKIE_LOST: std::time::Duration = std::time::Duration::from_secs(10);

pub struct Monitor {
    inner: Arc<Mutex<Inner>>,
    /// a cookie was heard
    heard: Arc<std::sync::Condvar>,
    next_cookie: std::sync::atomic::AtomicU64,
    watcher: Option<FileSystemWatcher>,
    /// what it watches: a tree outside them is re-baselined on every ask
    roots: Vec<PathBuf>,
    /// why the OS monitor is not running (every ask then re-baselines)
    pub error: Option<String>,
    /// the trees whose baseline a background read reads next, and whether that reader runs
    /// ([`Self::dirty_known`])
    reads: Mutex<(Vec<PathBuf>, bool)>,
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

fn repo(tree: &Path) -> Result<makepad_git::Repository, String> {
    let e = |e: makepad_git::GitError| format!("{}: {e}", tree.display());
    let p = makepad_git::repository_paths(tree).map_err(e)?.ok_or_else(|| format!("{}: not a git worktree", tree.display()))?;
    crate::gitread::worktree(p).map_err(e)
}

/// The tree's status: every path that differs from HEAD or is untracked (not ignored).
fn baseline(tree: &Path) -> Result<BTreeSet<String>, String> {
    let status = repo(tree)?.status().map_err(|e| format!("{}: status: {e}", tree.display()))?;
    Ok(status.entries.into_iter().map(|e| e.path).collect())
}

impl Monitor {
    /// Watches `roots` (directories holding worktrees, or a tree itself).
    pub fn start(roots: &[PathBuf]) -> Arc<Monitor> {
        let inner = Arc::new(Mutex::new(Inner::default()));
        let heard = Arc::new(std::sync::Condvar::new());
        let (i2, h2) = (inner.clone(), heard.clone());
        let dirs: Vec<PathBuf> = roots.iter().filter(|r| r.is_dir()).map(|r| r.canonicalize().unwrap_or_else(|_| r.clone())).collect();
        let roots: Vec<WatchRoot> = dirs.iter().map(|d| WatchRoot { mount: "lanes".into(), path: d.clone() }).collect();
        let config = WatcherConfig { exclude: ExcludePolicy::default().dir_name("local").dir_name("node_modules"), startup_rescan: false, ..WatcherConfig::default() };
        let started = FileSystemWatcher::start_with_journal(roots, config, move |e: &FileSystemEvent| {
            if !on_cookie(&i2, &h2, e) { on_event(&i2, e); }
            true
        });
        let (watcher, error) = match started {
            Ok((w, _)) => (Some(w), None),
            Err(e) => (None, Some(e)),
        };
        Arc::new(Monitor { inner, heard, next_cookie: Default::default(), watcher, roots: dirs, error, reads: Mutex::new((Vec::new(), false)) })
    }

    /// Calls `f` on every file event in a followed tree, as it comes (live jobs ship edits so).
    pub fn listen(&self, f: Listener) {
        self.inner.lock().unwrap().listeners.push(f);
    }

    pub fn watching(&self) -> bool {
        self.watcher.is_some()
    }

    /// The tree's file events so far, as a mark for [`Self::quiet_since`]; the tree is followed
    /// from now on. None: the OS monitor does not watch it (nothing it says can be trusted then).
    pub fn mark(&self, tree: &Path) -> Option<u64> {
        let root = tree.canonicalize().ok()?;
        if self.watcher.is_none() || !self.roots.iter().any(|r| root.starts_with(r)) {
            return None;
        }
        Some(self.inner.lock().unwrap().trees.entry(root).or_default().seq)
    }

    /// No file event in the tree since `mark` (lost events count as one in every tree).
    pub fn quiet_since(&self, tree: &Path, mark: u64) -> bool {
        let Ok(root) = tree.canonicalize() else { return false };
        self.inner.lock().unwrap().trees.get(&root).is_some_and(|t| t.seq == mark)
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

    #[cfg(test)]
    pub(crate) fn set_last_event_ms(&self, tree: &Path, at: u64) {
        let root = tree.canonicalize().unwrap();
        self.inner.lock().unwrap().trees.entry(root).or_default().last_ms = at;
    }

    /// Stops following a tree (it was removed).
    pub fn forget(&self, tree: &Path) {
        let t = tree.canonicalize().unwrap_or_else(|_| tree.to_path_buf());
        self.inner.lock().unwrap().trees.remove(&t);
    }

    /// [`Self::dirty`] once the tree's baseline is known; until then None, and one background
    /// reader reads the baselines asked for, one tree after another. A command answers from what
    /// is known: after a master restart, `devhub lanes` first read every lane's baseline in a row
    /// (4.5 min, 11-03). A tree the OS monitor does not follow has no lasting baseline: it is read
    /// at the ask.
    pub fn dirty_known(self: &Arc<Self>, tree: &Path) -> Option<Result<Dirty, String>> {
        let root = match tree.canonicalize() {
            Ok(r) => r,
            Err(e) => return Some(Err(format!("{}: {e}", tree.display()))),
        };
        let Some((head, _)) = read_head(&root) else { return Some(Err(format!("{}: not a git worktree", root.display()))) };
        let watched = self.watcher.is_some() && self.roots.iter().any(|r| root.starts_with(r));
        if !watched || self.inner.lock().unwrap().trees.get(&root).is_some_and(|t| t.based && t.head == head && !t.stale) {
            return Some(self.dirty(&root));
        }
        let mut reads = self.reads.lock().unwrap();
        if !reads.0.contains(&root) {
            reads.0.push(root);
        }
        if !reads.1 {
            reads.1 = true;
            let me = self.clone();
            let spawned = std::thread::Builder::new().name("devhub-dirty-read".into()).spawn(move || loop {
                let next = {
                    let mut r = me.reads.lock().unwrap();
                    if r.0.is_empty() {
                        r.1 = false;
                        return;
                    }
                    r.0.remove(0)
                };
                let _ = me.dirty(&next);
            });
            if spawned.is_err() {
                reads.1 = false;
            }
        }
        None
    }

    /// Returns with every write made under `tree` before this call heard: a snapshot taken next
    /// holds them (a file written 100-260 ms before a job submit was left out of its snapshot on
    /// the Mac). A tree outside the watch is read at each ask anyway. Should the cookie not come,
    /// the events are lost: every tree is read again at its next ask.
    pub fn settle(&self, tree: &Path) -> Result<(), String> {
        let root = tree.canonicalize().map_err(|e| format!("{}: {e}", tree.display()))?;
        let Some(watched) = self.roots.iter().find(|r| root.starts_with(r)).filter(|_| self.watcher.is_some()) else { return Ok(()) };
        let n = self.next_cookie.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cookie = watched.join(format!("{COOKIE_PREFIX}{}-{n}", std::process::id()));
        std::fs::write(&cookie, b"").map_err(|e| format!("{}: {e}", cookie.display()))?;
        let mut g = self.inner.lock().unwrap();
        let start = std::time::Instant::now();
        while !g.cookies.remove(&n) {
            let Some(left) = COOKIE_LOST.checked_sub(start.elapsed()) else {
                for t in g.trees.values_mut() { t.stale = true; t.tracked_stale = true; t.seq += 1; }
                break;
            };
            g = self.heard.wait_timeout(g, left).unwrap().0;
        }
        drop(g);
        let _ = std::fs::remove_file(&cookie);
        Ok(())
    }

    /// The tree's dirty set now (`tree`: a worktree root; for the makepad tree, paths under a
    /// nested repository's `apps/<name>` belong to that tree and are left out).
    pub fn dirty(&self, tree: &Path) -> Result<Dirty, String> {
        let root = tree.canonicalize().map_err(|e| format!("{}: {e}", tree.display()))?;
        let (head, branch) = read_head(&root).ok_or_else(|| format!("{}: not a git worktree", root.display()))?;
        let fresh = {
            let mut g = self.inner.lock().unwrap();
            let t = g.trees.entry(root.clone()).or_default();
            let watched = self.watcher.is_some() && self.roots.iter().any(|r| root.starts_with(r));
            if t.head != head || t.stale || !watched {
                let last_ms = t.last_ms;
                let seq = t.seq + 1;
                *t = Tree { head: head.clone(), last_ms, seq, ..Default::default() };
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
                t.based = true;
            }
        }
        let nested = crate::nested::names(&root);
        let (cands, asked_at): (Vec<String>, u64) = {
            let g = self.inner.lock().unwrap();
            let t = g.trees.get(&root).ok_or("the tree was forgotten")?;
            let all: Vec<String> = t.base.iter().chain(t.touched.keys()).collect::<BTreeSet<_>>().into_iter().filter(|p| crate::nested::split(p, nested.iter().map(String::as_str)).is_none()).cloned().collect();
            (all, t.seq)
        };
        let mut d = Dirty { head, branch, ..Default::default() };
        // touched files and files gone get a status of their own; the rest of the baseline stands
        let mut ask = Vec::new();
        {
            let g = self.inner.lock().unwrap();
            let t = g.trees.get(&root).ok_or("the tree was forgotten")?;
            for p in cands {
                match std::fs::symlink_metadata(root.join(&p)) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) if !t.touched.contains_key(&p) => d.changed.push(p),
                    _ => ask.push(p),
                }
            }
        }
        if ask.is_empty() {
            return Ok(d);
        }
        let states = repo(&root)?.status_of_paths(&ask).map_err(|e| format!("{}: status: {e}", root.display()))?;
        {
            // an event after the read keeps a path touched for the next ask
            let mut g = self.inner.lock().unwrap();
            if let Some(t) = g.trees.get_mut(&root).filter(|t| t.head == d.head) {
                for s in &states {
                    if t.touched.get(&s.path).is_none_or(|n| *n <= asked_at) {
                        t.touched.remove(&s.path);
                        if s.status.is_some() { t.base.insert(s.path.clone()); } else { t.base.remove(&s.path); }
                    }
                }
            }
        }
        // a path that is gone counts only when HEAD has it (an untracked file came and went)
        for s in states.into_iter().filter(|s| s.status.is_some()) {
            if root.join(&s.path).symlink_metadata().is_ok() {
                d.changed.push(s.path);
            } else if s.in_head {
                d.deleted.push(s.path);
            }
        }
        d.changed.sort();
        Ok(d)
    }
}

/// A cookie's event ([`Monitor::settle`]) goes no further: no tree holds it. Its removal says nothing.
fn on_cookie(inner: &Mutex<Inner>, heard: &std::sync::Condvar, e: &FileSystemEvent) -> bool {
    let Some(name) = e.path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix(COOKIE_PREFIX)) else { return false };
    if !matches!(e.kind, FileSystemEventKind::Removed) {
        if let Some(n) = name.split_once('-').filter(|(pid, _)| *pid == std::process::id().to_string()).and_then(|(_, n)| n.parse().ok()) {
            inner.lock().unwrap().cookies.insert(n);
            heard.notify_all();
        }
    }
    true
}

fn on_event(inner: &Mutex<Inner>, e: &FileSystemEvent) {
    let mut heard: Vec<(PathBuf, String)> = Vec::new();
    let listeners = {
        let mut g = inner.lock().unwrap();
        touch(&mut g, e, &mut heard);
        if heard.is_empty() { Vec::new() } else { g.listeners.clone() }
    };
    for (root, rel) in &heard {
        for f in &listeners {
            f(root, rel);
        }
    }
}

fn touch(g: &mut Inner, e: &FileSystemEvent, heard: &mut Vec<(PathBuf, String)>) {
    if let FileSystemEventKind::RescanRequired { .. } = e.kind {
        for t in g.trees.values_mut() {
            t.stale = true;
            t.tracked_stale = true;
            t.seq += 1;
        }
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
            t.seq += 1;
            if e.is_dir == Some(true) { t.tracked_stale = true; continue; }
            if rel.rsplit("/").next() == Some(".gitattributes") { t.tracked_stale = true; }
            t.tracked_touched.insert(rel.clone());
            t.touched.insert(rel.clone(), t.seq);
            t.last_ms = crate::proto::wall_ms();
            heard.push((root, rel));
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

    /// Written, settled, read: no write of a moment ago is left out, whatever the OS's latency,
    /// and the cookie is nobody's file.
    #[test]
    fn a_settled_tree_holds_the_write_of_a_moment_ago() {
        let base = std::env::temp_dir().join(format!("devhub-monitor-settle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tree = base.join("lane");
        std::fs::create_dir_all(&tree).unwrap();
        g(&tree, &["init", "-q", "-b", "rc-x"]);
        g(&tree, &["config", "user.email", "t@t"]);
        g(&tree, &["config", "user.name", "t"]);
        std::fs::write(tree.join("a.rs"), "a").unwrap();
        g(&tree, &["add", "."]);
        g(&tree, &["commit", "-q", "-m", "x"]);
        let m = Monitor::start(&[base.clone()]);
        assert_eq!(m.dirty(&tree).unwrap().changed.len(), 0);
        for i in 0..8 {
            let name = format!("new{i}.rs");
            std::fs::write(tree.join(&name), "u").unwrap();
            m.settle(&tree).unwrap();
            let changed = m.dirty(&tree).unwrap().changed;
            assert!(changed.contains(&name), "{name}: {changed:?}");
            assert_eq!(changed.len(), i + 1, "{changed:?}");
        }
        // inside a tree that is itself the root too
        let m = Monitor::start(&[tree.clone()]);
        let before = m.dirty(&tree).unwrap().changed;
        std::fs::write(tree.join("last.rs"), "u").unwrap();
        m.settle(&tree).unwrap();
        let after = m.dirty(&tree).unwrap().changed;
        assert_eq!(after.len(), before.len() + 1, "{after:?}");
        assert!(!std::fs::read_dir(&base).unwrap().flatten().chain(std::fs::read_dir(&tree).unwrap().flatten()).any(|e| e.file_name().to_string_lossy().starts_with(COOKIE_PREFIX)), "no cookie stays");
        let _ = std::fs::remove_dir_all(&base);
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
        let spawned = crate::git_exec::on_this_thread();
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
        // an edit undone is no work
        std::fs::write(tree.join("src/a.rs"), "a").unwrap();
        let mut changed = Vec::new();
        for _ in 0..50 {
            changed = m.dirty(&tree).unwrap().changed;
            if changed == ["untracked.rs"] {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert_eq!(changed, ["untracked.rs"]);
        // a commit moves HEAD: the next ask re-baselines
        g(&tree, &["add", "-A"]);
        g(&tree, &["commit", "-q", "-m", "y"]);
        let d = m.dirty(&tree).unwrap();
        assert_eq!((d.changed.len(), d.deleted.len()), (0, 0), "{d:?}");
        // baselines, edits, ignored output and a HEAD move: no git process
        assert_eq!(crate::git_exec::on_this_thread(), spawned);
        drop(m);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A tree asked about for the first time answers at once ("not known yet") and its baseline
    /// is read behind the answer; the next ask answers from it.
    #[test]
    fn a_first_ask_answers_at_once_and_reads_the_baseline_behind_it() {
        let base = std::env::temp_dir().join(format!("devhub-monitor-known-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let tree = base.join("lane");
        std::fs::create_dir_all(&tree).unwrap();
        g(&tree, &["init", "-q", "-b", "rc-y"]);
        g(&tree, &["config", "user.email", "t@t"]);
        g(&tree, &["config", "user.name", "t"]);
        std::fs::write(tree.join("a.rs"), "a").unwrap();
        g(&tree, &["add", "."]);
        g(&tree, &["commit", "-q", "-m", "x"]);
        std::fs::write(tree.join("new.rs"), "n").unwrap();
        let m = Monitor::start(&[base.clone()]);
        let first = m.dirty_known(&tree);
        if m.watching() {
            assert!(first.is_none(), "not known yet: {first:?}");
            let mut known = None;
            for _ in 0..100 {
                known = m.dirty_known(&tree);
                if known.is_some() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            assert_eq!(known.unwrap().unwrap().changed, ["new.rs"]);
        } else {
            assert_eq!(first.unwrap().unwrap().changed, ["new.rs"], "an unwatched tree is read at the ask");
        }
        drop(m);
        let _ = std::fs::remove_dir_all(&base);
    }
}
