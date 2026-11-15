//! The repo's file list, built on a worker thread (git-ignore aware: the
//! root and nested `.gitignore` files, `.git/info/exclude`, the user's
//! excludes file; tracked files always count) and ranked by a fuzzy match
//! for quick-open, the file filter and the chat's @-search.
//!
//! - `start(root, wake)` returns at once; the walk runs on a thread and calls
//!   `wake` when done (the window passes its UI signal);
//! - `files()` is the current list: relative paths, `/` separators, sorted;
//! - `version()` bumps each time the list changes;
//! - `search(query, limit)` ranks by the fuzzy score, best first, with the
//!   byte offsets of the matched characters;
//! - `refresh()` walks again (after a create / rename / delete); calls made
//!   while a walk runs fold into one more walk.
//!
//! After its first walk the worker also watches the folder
//! (`makepad-filesystem-watcher`) and walks once more, so changes made
//! outside Code show up on their own:
//! - a file or folder that appears, disappears or is renamed outside what
//!   git ignores wakes the worker, as does a `.gitignore` change or a lost-
//!   events signal (a queue overflow, dropped records, a changed root); a
//!   file's content changing does not, since it leaves the list as it is;
//! - wakes are coalesced: the worker waits for 30 ms of quiet (at most
//!   `max(0.5 s, 10 walks)` under steady churn) and walks once, and wakes the
//!   window only when the list changed;
//! - the watcher leaves out `.git` and `target` folders at any depth, the
//!   folders git ignores (as the last walk found them, or as they appear)
//!   and the home folders a walk skips, so their churn never reaches the
//!   worker;
//! - without a watcher (one that cannot start) the worker walks again every
//!   `max(2 s, 20 walks)` and says so once in the log. The Linux watcher
//!   itself polls the folders it has no inotify watch left for.
//! Changes only inside `.git` (the tracked set, `.git/info/exclude`) wait
//! for the next walk.

use makepad_filesystem_watcher::{
    ExcludePolicy, FileSystemEvent, FileSystemEventKind, FileSystemWatcher, JournalConfig, WatchRoot, WatcherConfig,
};
use makepad_git::ignore::{GitIgnore, IgnoreRules};
use crate::port::Wake;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// Walks done, whether or not they changed the list.
    walks: AtomicU64,
    /// The last walk's ignore rules: the watcher drops events on what they
    /// ignore. `None` until the first walk.
    rules: Mutex<Option<Arc<IgnoreRules>>>,
}

/// What the worker is told.
enum Msg {
    /// Walk again: asked for, or the watcher saw a change that may alter the list.
    Walk,
    /// A folder git ignores appeared: leave it out of the watch (no walk needed).
    Skip(String),
    /// The index is gone.
    Stop,
}

/// How long the worker waits for quiet after a wake before it walks.
const QUIET: Duration = Duration::from_millis(30);

#[derive(Default)]
pub struct FileIndex {
    shared: Option<Arc<Shared>>,
    wake: Option<Sender<Msg>>,
}

impl FileIndex {
    pub fn start(root: &Path, wake: Wake) -> Self {
        let shared = Arc::new(Shared {
            snap: Mutex::new(Arc::new(Snapshot::default())),
            version: AtomicU64::new(0),
            ready: AtomicBool::new(false),
            walks: AtomicU64::new(0),
            rules: Mutex::new(None),
        });
        let (tx, rx) = channel::<Msg>();
        let root = root.to_path_buf();
        let worker = shared.clone();
        let events = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("code-file-index".into())
            .spawn(move || run_worker(&root, &worker, rx, events, wake));
        if spawned.is_err() {
            return FileIndex::default();
        }
        let _ = tx.send(Msg::Walk);
        FileIndex { shared: Some(shared), wake: Some(tx) }
    }

    /// Walks done so far, whether or not they changed the list.
    pub fn walks(&self) -> u64 {
        self.shared.as_ref().map_or(0, |s| s.walks.load(Ordering::SeqCst))
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
            let _ = tx.send(Msg::Walk);
        }
    }
}

impl Drop for FileIndex {
    fn drop(&mut self) {
        // The watcher holds a sender too: say stop, or the worker waits forever.
        if let Some(tx) = &self.wake {
            let _ = tx.send(Msg::Stop);
        }
    }
}

/// The worker: walks `root` when told to or when its watcher saw a change
/// that may alter the list.
fn run_worker(root: &Path, shared: &Arc<Shared>, rx: Receiver<Msg>, tx: Sender<Msg>, wake: Wake) {
    let home = home_dir();
    // The watcher reports the folder's real path.
    let real_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut watcher: Option<FileSystemWatcher> = None;
    let mut started_watching = false;
    let mut skipped = BTreeSet::new();
    let mut walk_time = Duration::ZERO;
    'work: loop {
        // Without a watcher the folder is walked again now and then.
        let first = if watcher.is_some() || !started_watching {
            rx.recv().map_err(|_| ())
        } else {
            match rx.recv_timeout(Duration::from_secs(2).max(walk_time * 20)) {
                Ok(msg) => Ok(msg),
                Err(RecvTimeoutError::Timeout) => Ok(Msg::Walk),
                Err(RecvTimeoutError::Disconnected) => Err(()),
            }
        };
        let mut walk = false;
        let mut skips = BTreeSet::new();
        match first {
            Ok(Msg::Walk) => walk = true,
            Ok(Msg::Skip(rel)) => {
                skips.insert(rel);
            }
            Ok(Msg::Stop) | Err(()) => break,
        }
        // Wakes in a burst (a checkout, a tool writing many files) fold into one walk.
        let woke = Instant::now();
        let longest = Duration::from_millis(500).max(walk_time * 10);
        loop {
            let left = longest.saturating_sub(woke.elapsed());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(QUIET.min(left)) {
                Ok(Msg::Walk) => walk = true,
                Ok(Msg::Skip(rel)) => {
                    skips.insert(rel);
                }
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break 'work,
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        if !walk {
            // Only new ignored folders: the list is as it was; their churn leaves the watch.
            if watcher.is_some() && !skips.is_subset(&skipped) {
                skipped.extend(skips);
                if let Some(next) = start_watcher(&real_root, &skipped, shared, &tx) {
                    watcher = Some(next);
                }
            }
            continue;
        }
        let started = Instant::now();
        let walked = walk_in(home.as_deref(), root);
        walk_time = started.elapsed();
        shared.walks.fetch_add(1, Ordering::SeqCst);
        *shared.rules.lock().unwrap() = walked.rules.map(Arc::new);
        let old = shared.snap.lock().unwrap().clone();
        if *old.files != walked.files || !shared.ready.load(Ordering::SeqCst) {
            *shared.snap.lock().unwrap() = Arc::new(Snapshot::new(walked.files));
            shared.version.fetch_add(1, Ordering::SeqCst);
            shared.ready.store(true, Ordering::SeqCst);
            wake();
        }
        if !started_watching {
            // The first walk tells which folders to leave out of the watch. Once the
            // watch is on, walk again: a change during the first walk is not lost.
            started_watching = true;
            skipped = walked.skipped;
            watcher = start_watcher(&real_root, &skipped, shared, &tx);
            if watcher.is_some() {
                let _ = tx.send(Msg::Walk);
            }
        } else if watcher.is_some() && walked.skipped != skipped {
            // Folders git newly ignores leave the watch; one it no longer ignores joins
            // it. An ignored folder that is merely gone stays left out: that costs nothing.
            let joined = skipped.difference(&walked.skipped).any(|rel| root.join(rel).exists());
            if joined || !walked.skipped.is_subset(&skipped) {
                skipped = walked.skipped;
                // The new watcher starts before the old one stops. A folder that joins the
                // watch was not watched since this walk read it: walk once more.
                if let Some(next) = start_watcher(&real_root, &skipped, shared, &tx) {
                    watcher = Some(next);
                    if joined {
                        let _ = tx.send(Msg::Walk);
                    }
                }
            }
        }
    }
    drop(watcher);
}

/// A watcher on `root` that leaves out `.git`, `target` and the `skipped`
/// folders, and tells the worker about events that may change the list.
fn start_watcher(root: &Path, skipped: &BTreeSet<String>, shared: &Arc<Shared>, tx: &Sender<Msg>) -> Option<FileSystemWatcher> {
    let mut exclude = ExcludePolicy::none().dir_name(".git").dir_name("target");
    for rel in skipped {
        exclude = exclude.path(rel);
    }
    let config = WatcherConfig {
        exclude,
        // Every event is taken at once: the journal keeps a little history only.
        journal: JournalConfig { ring_bytes: 64 * 1024, spool: None },
        startup_rescan: false,
    };
    let (shared, tx, real_root) = (shared.clone(), tx.clone(), root.to_path_buf());
    let roots = vec![WatchRoot { mount: "root".into(), path: root.to_path_buf() }];
    match FileSystemWatcher::start_with_journal(roots, config, move |event| {
        if let Some(msg) = message_for(&real_root, &shared, event) {
            let _ = tx.send(msg);
        }
        true
    }) {
        Ok((watcher, _journal)) => Some(watcher),
        Err(error) => {
            eprintln!("code index: {} is not watched ({error}): it is walked again every 2 s or more instead", root.display());
            None
        }
    }
}

/// What an event asks of the worker: a walk when it can change the list (a
/// path git does not ignore appeared, went or was renamed; a `.gitignore`; a
/// sign events were lost), leaving out a folder git ignores that appeared, or
/// nothing.
fn message_for(root: &Path, shared: &Shared, event: &FileSystemEvent) -> Option<Msg> {
    // The path relative to the root when git ignores it.
    let ignored = |path: &Path| -> Option<String> {
        if path.file_name().is_some_and(|name| name == ".gitignore") {
            return None;
        }
        let rel = relative(root, path).filter(|rel| !rel.is_empty())?;
        let ignored = shared.rules.lock().unwrap().as_ref().is_some_and(|rules| rules.ignored(&rel));
        ignored.then_some(rel)
    };
    match &event.kind {
        FileSystemEventKind::RescanRequired { .. } => Some(Msg::Walk),
        // A file's content changing leaves the list as it is. A watcher that
        // can only poll says `Changed` for its root.
        FileSystemEventKind::Changed => {
            (event.path == root || event.path.file_name().is_some_and(|name| name == ".gitignore")).then_some(Msg::Walk)
        }
        FileSystemEventKind::Created => match ignored(&event.path) {
            None => Some(Msg::Walk),
            Some(rel) if event.is_dir == Some(true) => Some(Msg::Skip(rel)),
            Some(_) => None,
        },
        FileSystemEventKind::Removed => ignored(&event.path).is_none().then_some(Msg::Walk),
        FileSystemEventKind::Renamed { from } => {
            (ignored(&event.path).is_none() || ignored(from).is_none()).then_some(Msg::Walk)
        }
    }
}

/// `path` relative to `root` with `/` separators, if it is inside.
fn relative(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for part in rel.components() {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part.as_os_str().to_str()?);
    }
    Some(out)
}

/// Folders directly in the home folder that macOS guards with privacy prompts.
#[cfg(target_os = "macos")]
const PROTECTED_HOME_DIRS: [&str; 9] = ["Desktop", "Documents", "Downloads", "Pictures", "Movies", "Music", "Library", "Public", ".Trash"];
#[cfg(not(target_os = "macos"))]
const PROTECTED_HOME_DIRS: [&str; 0] = [];

/// Whether a walk, a watcher or a restore must stay out of `dir`: a folder
/// directly in `home` that is hidden (a tool folder such as `.cargo`) or, on
/// macOS, one of the privacy-protected ones (`Documents`, `Library`, ...).
/// A `root` inside such a folder was chosen by the person, so it walks.
pub fn skips_home_dir(home: &Path, root: &Path, dir: &Path) -> bool {
    if dir.parent() != Some(home) || root.starts_with(dir) {
        return false;
    }
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else { return false };
    name.starts_with('.') || PROTECTED_HOME_DIRS.contains(&name)
}

/// `skips_home_dir` for the current user's home folder.
pub fn skips_dir(root: &Path, dir: &Path) -> bool {
    match home_dir() {
        Some(home) => skips_home_dir(&home, root, dir),
        None => false,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// The working tree's files under `root` that git does not ignore, sorted,
/// relative with `/` separators. `.git` is always skipped; symlinked
/// folders are not followed.
pub fn walk(root: &Path) -> Vec<String> {
    walk_in(home_dir().as_deref(), root).files
}

/// What one walk found.
struct Walked {
    files: Vec<String>,
    /// Folders left out: ignored by git, or guarded home folders.
    skipped: BTreeSet<String>,
    /// The ignore rules the walk used.
    rules: Option<IgnoreRules>,
}

fn walk_in(home: Option<&Path>, root: &Path) -> Walked {
    let mut ignore = GitIgnore::with_user_excludes(root).ok();
    let mut skipped = BTreeSet::new();
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
                    if is_dir {
                        skipped.insert(path);
                    }
                    continue;
                }
            }
            if is_dir {
                if home.is_some_and(|home| skips_home_dir(home, root, &entry.path())) {
                    skipped.insert(path);
                    continue;
                }
                stack.push((entry.path(), path));
            } else {
                out.push(path);
            }
        }
    }
    out.sort_unstable();
    Walked { files: out, skipped, rules: ignore.map(|ignore| ignore.snapshot()) }
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

    #[test]
    fn home_folders_are_skipped_unless_the_root_is_inside() {
        let home = scratch("home");
        let skipped = |root: &Path, rel: &str| skips_home_dir(&home, root, &home.join(rel));
        let mut guarded = vec![".cargo", ".makepad"];
        if cfg!(target_os = "macos") {
            guarded.extend(["Desktop", "Documents", "Downloads", "Pictures", "Movies", "Music", "Library", "Public", ".Trash"]);
        }
        for name in &guarded {
            assert!(skipped(&home, name), "{name} directly in home is skipped");
            assert!(!skipped(&home.join(name).join("proj"), name), "{name} holds the root, so it walks");
        }
        assert!(!skipped(&home, "src"), "an ordinary folder walks");
        assert!(!skips_home_dir(&home, &home, &home.join("src/.cache")), "only folders directly in home count");
        put(&home, ".cargo/registry/a.rs");
        put(&home, "src/main.rs");
        assert_eq!(walk_in(Some(&home), &home).files, ["src/main.rs"]);
        let _ = std::fs::remove_dir_all(&home);
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

    /// Waits up to 5 s for `done`, failing with `what`.
    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let t0 = Instant::now();
        while !done() {
            assert!(t0.elapsed() < Duration::from_secs(5), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_index_follows_the_folder_and_rests_when_nothing_changes() {
        let dir = scratch("watch");
        for git in [".git/objects", ".git/refs"] {
            std::fs::create_dir_all(dir.join(git)).unwrap();
        }
        std::fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "target/\nignored/\n*.log\n").unwrap();
        // A workspace of 100 folders of 20 files, a build folder and an ignored one.
        for folder in 0..100 {
            for file in 0..20 {
                put(&dir, &format!("src/m{folder:03}/f{file:02}.rs"));
            }
        }
        put(&dir, "target/debug/app");
        put(&dir, "ignored/cache.bin");
        let index = FileIndex::start(&dir, Arc::new(|| {}));
        let has = |path: &str| index.files().iter().any(|file| file == path);
        // The first walk, then the one once the watch is on.
        wait_until("the walks at the start", || index.walks() >= 2);
        assert_eq!(index.files().len(), 2001, "the first walk lists the workspace's files and its .gitignore");

        let (walks, version) = (index.walks(), index.version());
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(index.walks(), walks, "a workspace where nothing changes is not walked again");

        for i in 0..50 {
            put(&dir, &format!("target/debug/deps/lib{i}.rlib"));
            put(&dir, &format!(".git/objects/{i:02}/object"));
            put(&dir, &format!("ignored/cache{i}.bin"));
            put(&dir, &format!("src/run{i}.log"));
        }
        std::fs::write(dir.join("src/m000/f00.rs"), "edited").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(index.walks(), walks, "build output, git internals, ignored files and content edits wake no walk");
        assert_eq!(index.version(), version, "the list is unchanged");

        put(&dir, "src/new.rs");
        wait_until("a created file is listed", || has("src/new.rs"));
        std::fs::rename(dir.join("src/new.rs"), dir.join("src/renamed.rs")).unwrap();
        wait_until("a renamed file is listed by its new name only", || has("src/renamed.rs") && !has("src/new.rs"));
        std::fs::remove_file(dir.join("src/m001/f01.rs")).unwrap();
        wait_until("a removed file leaves the list", || !has("src/m001/f01.rs"));
        put(&dir, "docs/guide/intro.md");
        wait_until("a file in new folders is listed", || has("docs/guide/intro.md"));
        put(&dir, "docs/guide/next.md");
        wait_until("a new folder is watched", || has("docs/guide/next.md"));

        std::fs::write(dir.join(".gitignore"), "target/\nignored/\n*.log\nsrc/m002/\n").unwrap();
        wait_until("a folder git now ignores leaves the list", || !index.files().iter().any(|file| file.starts_with("src/m002/")));
        let walks = index.walks();
        put(&dir, "src/m002/later.rs");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(index.walks(), walks, "a folder git now ignores wakes no walk");
        std::fs::write(dir.join(".gitignore"), "target/\nignored/\n*.log\n").unwrap();
        wait_until("a folder git ignores no more is listed again", || has("src/m002/later.rs"));
        put(&dir, "src/m002/again.rs");
        wait_until("a folder git ignores no more is watched again", || has("src/m002/again.rs"));

        let walks = index.walks();
        index.refresh();
        wait_until("refresh walks", || index.walks() > walks);
        drop(index);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
