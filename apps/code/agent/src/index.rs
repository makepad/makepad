//! The repo's file list, built on a worker thread (git-ignore aware: the
//! root and nested `.gitignore` files, `.git/info/exclude`, the user's
//! excludes file; tracked files always count) and ranked by a fuzzy match
//! for quick-open, the file filter and the chat's @-search.
//!
//! - `start(root, wake)` returns at once; the walk runs on a thread and calls
//!   `wake` when done (the window passes its UI signal);
//! - `files()` is the current list: relative paths, `/` separators, sorted;
//! - `version()` bumps each time the list is replaced;
//! - `search(query, limit)` ranks by the fuzzy score, best first, with the
//!   byte offsets of the matched characters;
//! - `refresh()` walks again (after a create / rename / delete); calls made
//!   while a walk runs fold into one more walk.

use makepad_git::ignore::GitIgnore;
use crate::port::Wake;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// One ranked file: the path and the byte offsets of the matched characters
/// (for drawing them lit).
#[derive(Clone, Debug)]
pub struct FileMatch {
    pub path: String,
    pub score: i32,
    pub hits: Vec<usize>,
}

/// One walk's result: the paths, and a lowercased copy of all of them in
/// one buffer so ranking touches no allocation per candidate.
#[derive(Default)]
struct Snapshot {
    files: Arc<Vec<String>>,
    lower: Vec<u8>,
    /// `lower[starts[i]..starts[i + 1]]` is file `i`.
    starts: Vec<usize>,
}

impl Snapshot {
    fn new(files: Vec<String>) -> Self {
        let mut lower = Vec::with_capacity(files.iter().map(|f| f.len()).sum());
        let mut starts = Vec::with_capacity(files.len() + 1);
        for f in &files {
            starts.push(lower.len());
            lower.extend(f.bytes().map(|b| b.to_ascii_lowercase()));
        }
        starts.push(lower.len());
        Snapshot { files: Arc::new(files), lower, starts }
    }

    fn search(&self, query: &str, limit: usize) -> Vec<FileMatch> {
        let q: Vec<u8> = query.bytes().filter(|b| !b.is_ascii_whitespace()).map(|b| b.to_ascii_lowercase()).collect();
        if limit == 0 {
            return Vec::new();
        }
        if q.is_empty() {
            return self.files.iter().take(limit).map(|p| FileMatch { path: p.clone(), score: 0, hits: Vec::new() }).collect();
        }
        let mut scorer = Scorer::default();
        let mut found: Vec<(i32, u32)> = Vec::new();
        for i in 0..self.files.len() {
            let lower = &self.lower[self.starts[i]..self.starts[i + 1]];
            if !is_subsequence(&q, lower) {
                continue;
            }
            if let Some(s) = scorer.align(&q, lower, self.files[i].as_bytes(), false) {
                found.push((s, i as u32));
            }
        }
        let files = &self.files;
        let order = |a: &(i32, u32), b: &(i32, u32)| {
            b.0.cmp(&a.0)
                .then_with(|| files[a.1 as usize].len().cmp(&files[b.1 as usize].len()))
                .then_with(|| files[a.1 as usize].cmp(&files[b.1 as usize]))
        };
        if found.len() > limit {
            found.select_nth_unstable_by(limit - 1, order);
            found.truncate(limit);
        }
        found.sort_unstable_by(order);
        found
            .into_iter()
            .map(|(score, i)| {
                let i = i as usize;
                let lower = &self.lower[self.starts[i]..self.starts[i + 1]];
                scorer.align(&q, lower, self.files[i].as_bytes(), true);
                FileMatch { path: self.files[i].clone(), score, hits: scorer.hits.clone() }
            })
            .collect()
    }
}

fn is_subsequence(q: &[u8], text: &[u8]) -> bool {
    let mut it = text.iter();
    q.iter().all(|c| it.any(|t| t == c))
}

const NEG: i32 = i32::MIN / 4;
const MATCH: i32 = 16;
const CONSECUTIVE: i32 = 12;
const GAP_OPEN: i32 = -5;
const GAP_EXTEND: i32 = -1;
/// Per matched character inside the file name.
const IN_NAME: i32 = 6;

/// The best alignment of a query in a path: every query byte matched in
/// order; runs, word starts and the file name score; gaps cost. Buffers are
/// kept between candidates.
#[derive(Default)]
struct Scorer {
    score: Vec<i32>,
    from: Vec<u32>,
    bonus: Vec<i32>,
    hits: Vec<usize>,
}

impl Scorer {
    fn align(&mut self, q: &[u8], lower: &[u8], orig: &[u8], want_hits: bool) -> Option<i32> {
        let (m, n) = (q.len(), lower.len());
        if m == 0 || m > n {
            return None;
        }
        let name = lower.iter().rposition(|&b| b == b'/').map_or(0, |p| p + 1);
        self.bonus.clear();
        for j in 0..n {
            let mut b = if j == 0 {
                10
            } else {
                match orig[j - 1] {
                    b'/' => 10,
                    b'_' | b'-' | b'.' | b' ' => 7,
                    p if p.is_ascii_lowercase() && orig[j].is_ascii_uppercase() => 7,
                    p if !p.is_ascii_digit() && orig[j].is_ascii_digit() => 3,
                    _ => 0,
                }
            };
            if j >= name {
                b += IN_NAME;
            }
            self.bonus.push(b);
        }
        self.score.clear();
        self.score.resize(m * n, NEG);
        self.from.clear();
        self.from.resize(m * n, u32::MAX);
        for i in 0..m {
            let row = i * n;
            // Best of the previous row ending two or more bytes back, with the
            // gap's cost so far.
            let mut gap = NEG;
            let mut gap_at = u32::MAX;
            for j in i..n {
                if i > 0 && j >= 2 {
                    let prev = self.score[row - n + j - 2];
                    gap += GAP_EXTEND;
                    if prev > NEG && prev + GAP_OPEN > gap {
                        gap = prev + GAP_OPEN;
                        gap_at = (j - 2) as u32;
                    }
                }
                if lower[j] != q[i] {
                    continue;
                }
                let here = MATCH + self.bonus[j];
                if i == 0 {
                    self.score[row + j] = here;
                    continue;
                }
                let run = if j >= 1 { self.score[row - n + j - 1] } else { NEG };
                let (best, from) = if run > NEG && run + CONSECUTIVE >= gap { (run + CONSECUTIVE, (j - 1) as u32) } else { (gap, gap_at) };
                if best > NEG / 2 {
                    self.score[row + j] = best + here;
                    self.from[row + j] = from;
                }
            }
        }
        let last = (m - 1) * n;
        let (end, mut total) = (0..n).map(|j| (j, self.score[last + j])).max_by_key(|&(j, s)| (s, std::cmp::Reverse(j)))?;
        if total <= NEG / 2 {
            return None;
        }
        let base = &lower[name..];
        if base == q {
            total += 40;
        } else if base.starts_with(q) {
            total += 20;
        }
        total -= (n as i32) / 4;
        if want_hits {
            self.hits.clear();
            let mut j = end;
            for i in (0..m).rev() {
                self.hits.push(j);
                if i > 0 {
                    j = self.from[i * n + j] as usize;
                }
            }
            self.hits.reverse();
        }
        Some(total)
    }
}

struct Shared {
    snap: Mutex<Arc<Snapshot>>,
    version: AtomicU64,
    ready: AtomicBool,
}

#[derive(Default)]
pub struct FileIndex {
    shared: Option<Arc<Shared>>,
    wake: Option<Sender<()>>,
}

impl FileIndex {
    pub fn start(root: &Path, wake: Wake) -> Self {
        let shared = Arc::new(Shared { snap: Mutex::new(Arc::new(Snapshot::default())), version: AtomicU64::new(0), ready: AtomicBool::new(false) });
        let (tx, rx) = channel::<()>();
        let root = root.to_path_buf();
        let worker = shared.clone();
        let spawned = std::thread::Builder::new().name("code-file-index".into()).spawn(move || {
            while rx.recv().is_ok() {
                // Several refreshes asked for while a walk ran: one walk.
                while rx.try_recv().is_ok() {}
                let files = walk(&root);
                let snap = Arc::new(Snapshot::new(files));
                *worker.snap.lock().unwrap() = snap;
                worker.version.fetch_add(1, Ordering::SeqCst);
                worker.ready.store(true, Ordering::SeqCst);
                wake();
            }
        });
        if spawned.is_err() {
            return FileIndex::default();
        }
        let _ = tx.send(());
        FileIndex { shared: Some(shared), wake: Some(tx) }
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        match &self.shared {
            Some(s) => s.snap.lock().unwrap().clone(),
            None => Arc::new(Snapshot::default()),
        }
    }

    /// Whether the first walk has finished.
    pub fn ready(&self) -> bool {
        self.shared.as_ref().is_some_and(|s| s.ready.load(Ordering::SeqCst))
    }

    /// Bumps each time the list changes, so a picker knows to re-rank.
    pub fn version(&self) -> u64 {
        self.shared.as_ref().map_or(0, |s| s.version.load(Ordering::SeqCst))
    }

    pub fn files(&self) -> Arc<Vec<String>> {
        self.snapshot().files.clone()
    }

    /// The `limit` best matches of `query`, best first; for an empty query
    /// the first `limit` files in order.
    pub fn search(&self, query: &str, limit: usize) -> Vec<FileMatch> {
        self.snapshot().search(query, limit)
    }

    pub fn refresh(&self) {
        if let Some(tx) = &self.wake {
            let _ = tx.send(());
        }
    }
}

/// The working tree's files under `root` that git does not ignore, sorted,
/// relative with `/` separators. `.git` is always skipped; symlinked
/// folders are not followed.
pub fn walk(root: &Path) -> Vec<String> {
    let mut ignore = GitIgnore::with_user_excludes(root).ok();
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == ".git" {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            let is_dir = if kind.is_symlink() {
                match std::fs::metadata(entry.path()) {
                    Ok(m) if m.is_file() => false,
                    _ => continue,
                }
            } else {
                kind.is_dir()
            };
            let path = if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") };
            if let Some(ig) = ignore.as_mut() {
                if ig.ignored(&path, is_dir).unwrap_or(false) {
                    continue;
                }
            }
            if is_dir {
                stack.push((entry.path(), path));
            } else {
                out.push(path);
            }
        }
    }
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("code-index-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(root: &Path, rel: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn walk_honours_gitignore_nested_and_exclude() {
        let dir = scratch("walk");
        std::fs::create_dir_all(dir.join(".git/info")).unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(dir.join(".git/objects")).unwrap();
        std::fs::create_dir_all(dir.join(".git/refs")).unwrap();
        std::fs::write(dir.join(".git/info/exclude"), "secret.txt\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\n*.log\n").unwrap();
        put(&dir, "src/main.rs");
        put(&dir, "src/run.log");
        put(&dir, "target/release/app");
        put(&dir, "secret.txt");
        put(&dir, "lib/.gitignore");
        std::fs::write(dir.join("lib/.gitignore"), "gen/\n").unwrap();
        put(&dir, "lib/gen/out.rs");
        put(&dir, "lib/keep.rs");
        let files = walk(&dir);
        assert_eq!(files, [".gitignore", "lib/.gitignore", "lib/keep.rs", "src/main.rs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn snap(files: &[&str]) -> Snapshot {
        let mut v: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        v.sort();
        Snapshot::new(v)
    }

    fn top(s: &Snapshot, q: &str) -> String {
        s.search(q, 5).first().map(|m| m.path.clone()).unwrap_or_default()
    }

    #[test]
    fn file_name_matches_rank_first() {
        let s = snap(&[
            "apps/code/src/ide/shell.rs",
            "apps/code/src/ide/search.rs",
            "apps/shell_tools/src/lib.rs",
            "libs/shellexec/src/runner.rs",
            "apps/code/src/ui/view.rs",
        ]);
        assert_eq!(top(&s, "shell.rs"), "apps/code/src/ide/shell.rs");
        assert_eq!(top(&s, "shell"), "apps/code/src/ide/shell.rs");
        assert_eq!(top(&s, "view"), "apps/code/src/ui/view.rs");
        assert_eq!(top(&s, "idesea"), "apps/code/src/ide/search.rs");
        assert!(s.search("zzz", 5).is_empty());
    }

    #[test]
    fn shorter_paths_win_ties_and_hits_point_at_matches() {
        let s = snap(&["a/b/c/d/mod.rs", "x/mod.rs"]);
        assert_eq!(top(&s, "mod.rs"), "x/mod.rs");
        let m = &s.search("mod", 1)[0];
        assert_eq!(m.hits, [2, 3, 4]);
        let path = "apps/code/src/ide/shell.rs";
        let s = snap(&[path]);
        let m = &s.search("SHELL", 1)[0];
        let lit: String = m.hits.iter().map(|&i| path.as_bytes()[i] as char).collect();
        assert_eq!(lit, "shell");
        assert_eq!(m.hits[0], path.rfind("shell").unwrap());
    }

    #[test]
    fn limit_and_empty_query() {
        let names: Vec<String> = (0..100).map(|i| format!("dir/file{i:03}.rs")).collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let s = snap(&refs);
        assert_eq!(s.search("", 3).len(), 3);
        let hits = s.search("file", 10);
        assert_eq!(hits.len(), 10);
        assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn the_index_walks_on_its_worker() {
        let dir = scratch("worker");
        put(&dir, "a.rs");
        put(&dir, "b/c.rs");
        let index = FileIndex::start(&dir, Arc::new(|| {}));
        let t0 = std::time::Instant::now();
        while !index.ready() && t0.elapsed().as_secs() < 5 {
            std::thread::yield_now();
        }
        assert_eq!(*index.files(), ["a.rs", "b/c.rs"]);
        let v = index.version();
        put(&dir, "d.rs");
        index.refresh();
        let t0 = std::time::Instant::now();
        while index.version() == v && t0.elapsed().as_secs() < 5 {
            std::thread::yield_now();
        }
        assert_eq!(index.files().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
