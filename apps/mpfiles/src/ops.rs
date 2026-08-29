//! The file-operations engine: copy, cut/paste, rename, new folder, and
//! move-to-Trash, all run on a single worker thread so a big copy never
//! stalls the UI. Progress and results come back through [`Ops::drain`];
//! everything that can be undone goes on the [`Journal`] as an [`Undo`].
//!
//! Pure `std`, on purpose: this module is unit-tested standalone (see the
//! command in the crate's contributing notes) and is meant to be dropped
//! into the app crate as `mod ops;` without pulling in makepad or any other
//! dependency along with it.

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

// ---------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------

/// What an operation does. The UI shows these words, so they are the
/// vocabulary of the progress row and the undo status line — change one
/// and the on-screen language changes with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Copy,
    Move,
    Trash,
    Rename,
    NewFolder,
    /// Erase, with no Trash behind it. Deliberately has no undo: that is what
    /// makes it different from Trash, and pretending otherwise would be a lie
    /// the user finds out about at the worst moment.
    Delete,
}

impl OpKind {
    /// The present-participle the progress row shows next to the file name,
    /// e.g. "Copying report.txt".
    pub fn verb(self) -> &'static str {
        match self {
            OpKind::Copy => "Copying",
            OpKind::Move => "Moving",
            OpKind::Trash => "Moving to Trash",
            OpKind::Rename => "Renaming",
            OpKind::NewFolder => "Creating",
            OpKind::Delete => "Deleting",
        }
    }
}

/// One job handed to the worker. A single request can carry many sources
/// (a multi-select copy/move/trash) but exactly one destination, because a
/// paste always lands in one folder at a time.
#[derive(Clone, Debug)]
pub struct OpRequest {
    /// Chosen by the caller (not the engine) so the caller can correlate a
    /// submitted job with the [`OpUpdate`]s that come back for it before
    /// the worker has even looked at it.
    pub id: u64,
    pub kind: OpKind,
    /// The files/folders acted on. Empty for [`OpKind::NewFolder`], which
    /// creates rather than consumes.
    pub sources: Vec<PathBuf>,
    /// Where they land (Copy/Move), the folder the new folder is made in
    /// (NewFolder), or the folder the rename happens in (Rename). Unused
    /// by Trash, which always has one true destination: the Trash itself.
    pub dest_dir: PathBuf,
    /// Rename's new name / NewFolder's name. Ignored otherwise.
    pub new_name: Option<String>,
    /// Trash needs the user's home to find `~/.Trash`; pass it in rather
    /// than reading the environment on a worker thread, which is a habit
    /// worth keeping even where it wouldn't currently race anything.
    pub home: PathBuf,
}

/// How to undo one finished operation. The [`Journal`] stores these; the
/// engine hands one back on every successful (or partially-cancelled)
/// [`OpUpdate::Done`].
#[derive(Clone, Debug, PartialEq)]
pub enum Undo {
    /// Put `to` back as `from` (rename and move — and a trash, which is
    /// just a move into a special folder — are all the same undo).
    Moved { pairs: Vec<(PathBuf, PathBuf)> },
    /// Delete what the copy (or new-folder) created. Kept separate from
    /// `Moved` because undoing a copy must never touch the original.
    Created { paths: Vec<PathBuf> },
}

impl Undo {
    /// A one-line description for the status bar, e.g. "Undo move of 3
    /// items". Singular items get their own name so a status line about
    /// one file reads like it is about that file, not a count of one.
    pub fn describe(&self) -> String {
        match self {
            Undo::Moved { pairs } => match pairs.as_slice() {
                [(_, to)] => format!("Undo move of \"{}\"", display_name(to)),
                pairs => format!("Undo move of {} items", pairs.len()),
            },
            Undo::Created { paths } => match paths.as_slice() {
                [path] => format!("Undo creation of \"{}\"", display_name(path)),
                paths => format!("Undo creation of {} items", paths.len()),
            },
        }
    }
}

fn moved_undo(pairs: Vec<(PathBuf, PathBuf)>) -> Option<Undo> {
    if pairs.is_empty() {
        None
    } else {
        Some(Undo::Moved { pairs })
    }
}

fn created_undo(paths: Vec<PathBuf>) -> Option<Undo> {
    if paths.is_empty() {
        None
    } else {
        Some(Undo::Created { paths })
    }
}

/// What the worker sends back. Drained on the UI thread via [`Ops::drain`].
#[derive(Clone, Debug)]
pub enum OpUpdate {
    /// `done`/`total` are bytes for Copy/Move/Trash (and their undos, which
    /// are just moves in the other direction); for undoing a Copy — which
    /// deletes rather than transfers — they are an item count instead,
    /// since there is no byte stream to measure.
    Progress {
        id: u64,
        kind: OpKind,
        done: u64,
        total: u64,
        current: String,
    },
    Done {
        id: u64,
        kind: OpKind,
        message: String,
        undo: Option<Undo>,
        touched: Vec<PathBuf>,
    },
    Failed {
        id: u64,
        kind: OpKind,
        message: String,
    },
}

// ---------------------------------------------------------------------
// Free functions — the parts with the interesting rules
// ---------------------------------------------------------------------

/// A path in `dir` for `name` that does not collide: "report.txt" becomes
/// "report (2).txt", then "report (3).txt". The suffix goes before the
/// extension so double-clicking the copy still opens it in the same
/// application; a name with no extension, and a dotfile like ".zshrc"
/// (which `Path` already treats as having none), get the suffix at the
/// very end instead.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = split_stem_ext(name);
    let mut n: u64 = 2;
    loop {
        let candidate_name = if ext.is_empty() {
            format!("{name} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let candidate = dir.join(&candidate_name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Splits `name` the way [`unique_path`] needs: a dotfile or an
/// extensionless name reports an empty extension, which is the signal to
/// put the disambiguating suffix at the very end rather than splicing it
/// into the middle of the only dot the name has.
fn split_stem_ext(name: &str) -> (String, String) {
    let path = Path::new(name);
    match (path.file_stem(), path.extension()) {
        (Some(stem), Some(ext)) => (stem.to_string_lossy().into_owned(), ext.to_string_lossy().into_owned()),
        _ => (name.to_string(), String::new()),
    }
}

/// Where `~/.Trash` is on this platform (macOS: `<home>/.Trash`, elsewhere
/// `<home>/.local/share/Trash/files`, the freedesktop.org convention).
/// This module cannot reuse the crate's own copy of this logic (see the
/// module doc comment on why), so it is duplicated deliberately rather
/// than imported.
pub fn trash_dir(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join(".Trash")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".local/share/Trash/files")
    }
}

/// Recursive byte total of a path — the size a folder reports in the
/// properties panel and the total a copy is measured against. Never
/// follows symlinks (a link's target is not this path's content, and
/// following it risks double-counting or walking outside the tree
/// entirely), and gives up early with whatever it has counted so far once
/// `cancel` is raised, so a huge folder does not block a cancel forever.
pub fn total_bytes(path: &Path, cancel: &AtomicBool) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.file_type().is_symlink() {
        return 0;
    }
    if !meta.is_dir() {
        return meta.len();
    }
    let mut total = 0u64;
    let Ok(read_dir) = fs::read_dir(path) else {
        return 0;
    };
    for entry in read_dir.flatten() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        total += total_bytes(&entry.path(), cancel);
    }
    total
}

/// Copy a file or a whole tree. Reports bytes as it goes through `on_bytes`
/// (called with the number of bytes just written, not the running total)
/// and gives up when `cancel` is raised, leaving whatever has already been
/// written in place — the caller decides whether to keep or discard a
/// cancelled copy's partial output. Symlinks are recreated as symlinks,
/// not followed, so copying a folder can never walk out of that folder and
/// copy the rest of the disk.
pub fn copy_tree(src: &Path, dst: &Path, cancel: &AtomicBool, on_bytes: &dyn Fn(u64)) -> io::Result<()> {
    if cancel.load(Ordering::SeqCst) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let link_target = fs::read_link(src)?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&link_target, dst)?;
        }
        #[cfg(windows)]
        {
            let points_at_dir = fs::metadata(src).map(|m| m.is_dir()).unwrap_or(false);
            if points_at_dir {
                std::os::windows::fs::symlink_dir(&link_target, dst)?;
            } else {
                std::os::windows::fs::symlink_file(&link_target, dst)?;
            }
        }
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            if cancel.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()), cancel, on_bytes)?;
        }
        Ok(())
    } else {
        copy_file_with_progress(src, dst, cancel, on_bytes)
    }
}

/// The single-file half of [`copy_tree`]: streamed in chunks so `on_bytes`
/// can report as it goes rather than only at the end, and so `cancel` is
/// checked between chunks instead of only between whole files.
fn copy_file_with_progress(src: &Path, dst: &Path, cancel: &AtomicBool, on_bytes: &dyn Fn(u64)) -> io::Result<()> {
    let mut reader = fs::File::open(src)?;
    let mut writer = fs::File::create(dst)?;
    let mut buf = [0u8; 256 * 1024];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
        on_bytes(n as u64);
    }
    // Best-effort: a permissions failure here shouldn't fail a copy whose
    // bytes already landed correctly.
    if let Ok(perm) = fs::metadata(src).map(|m| m.permissions()) {
        let _ = fs::set_permissions(dst, perm);
    }
    Ok(())
}

/// Move by rename when the two sit on the same volume — instant, and
/// atomic from the filesystem's point of view — otherwise copy then
/// delete, which is the fallback macOS needs whenever the Trash (or a
/// paste target) is on another disk than the source.
pub fn move_path(src: &Path, dst: &Path, cancel: &AtomicBool, on_bytes: &dyn Fn(u64)) -> io::Result<()> {
    if cancel.load(Ordering::SeqCst) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
    }
    // Measured before the attempt: after a successful rename `src` is gone,
    // and a failed one still wants an honest size for the fallback below.
    let size = total_bytes(src, cancel);
    if fs::rename(src, dst).is_ok() {
        on_bytes(size);
        return Ok(());
    }
    // `ErrorKind::CrossesDevices` is not stable across every target this
    // app builds for, so — per the module's contract — ANY rename error
    // takes the copy-then-delete path rather than trying to distinguish
    // "wrong device" from, say, "permission denied". A real permission
    // problem simply fails again inside `copy_tree`, with a clearer error.
    copy_tree(src, dst, cancel, on_bytes)?;
    remove_path(src)
}

/// Delete a file or a whole directory tree, whichever `path` is.
fn remove_path(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)?.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// The last path component, falling back to the whole path for something
/// path-shaped but nameless (like `/`). Used only for messages, so a
/// slightly odd fallback here is harmless.
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Refuses a copy/move whose destination is the source folder itself or
/// anything inside it — the one shape of this operation that would
/// otherwise recurse into its own output forever. Checked with
/// `canonicalize`, i.e. against what actually exists on disk right now,
/// per the module's contract; a destination that does not exist yet
/// cannot be inside a source that does, so it is allowed through.
fn refuse_into_self(sources: &[PathBuf], dest_dir: &Path) -> Option<String> {
    let dest_canon = fs::canonicalize(dest_dir).ok()?;
    for source in sources {
        let Ok(source_canon) = fs::canonicalize(source) else {
            continue;
        };
        if !source_canon.is_dir() {
            continue;
        }
        if dest_canon == source_canon || dest_canon.starts_with(&source_canon) {
            return Some(format!("Can't copy or move \"{}\" into itself", display_name(source)));
        }
    }
    None
}

/// True when `source` already lives directly inside `dest_dir` — the case
/// a cut-and-paste onto the folder it came from must treat as a no-op
/// rather than as a move that happens to land back where it started
/// (which would otherwise still burn a rename and a journal entry).
fn already_there(source: &Path, dest_dir: &Path, dest_canon: Option<&Path>) -> bool {
    if source.parent() == Some(dest_dir) {
        return true;
    }
    let (Some(parent), Some(dest_canon)) = (source.parent(), dest_canon) else {
        return false;
    };
    fs::canonicalize(parent).map(|p| p == dest_canon).unwrap_or(false)
}

// ---------------------------------------------------------------------
// Progress reporting
// ---------------------------------------------------------------------

/// Accumulates bytes for one job and turns them into throttled
/// [`OpUpdate::Progress`] pushes. `on_bytes` callbacks are plain `Fn`, not
/// `FnMut` (the free functions above are shared with contexts that only
/// hand out `&dyn Fn`), so the running counters live behind `Cell`/
/// `RefCell` instead of being captured by value.
struct Progress {
    id: u64,
    kind: OpKind,
    total: u64,
    done: Cell<u64>,
    bytes_since_emit: Cell<u64>,
    last_emit: Cell<Instant>,
    current: RefCell<String>,
    updates: Arc<Mutex<VecDeque<OpUpdate>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl Progress {
    fn new(id: u64, kind: OpKind, total: u64, updates: Arc<Mutex<VecDeque<OpUpdate>>>, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Progress {
            id,
            kind,
            total,
            done: Cell::new(0),
            bytes_since_emit: Cell::new(0),
            last_emit: Cell::new(Instant::now()),
            current: RefCell::new(String::new()),
            updates,
            notify,
        }
    }

    /// Call before starting a new top-level source so the progress row's
    /// "current" name updates between items even though byte reporting is
    /// only ever per-chunk, not per-file.
    fn set_current(&self, name: &str) {
        *self.current.borrow_mut() = name.to_string();
    }

    /// Emits at most every ~32ms or every 1MB of progress, never per file —
    /// per the module's contract, a paste of thousands of tiny files must
    /// not flood the update queue faster than the UI thread can drain it.
    fn on_bytes(&self, delta: u64) {
        let done = self.done.get() + delta;
        self.done.set(done);
        let since = self.bytes_since_emit.get() + delta;
        if since >= 1_000_000 || self.last_emit.get().elapsed() >= Duration::from_millis(32) {
            self.bytes_since_emit.set(0);
            self.last_emit.set(Instant::now());
            let update = OpUpdate::Progress {
                id: self.id,
                kind: self.kind,
                done,
                total: self.total,
                current: self.current.borrow().clone(),
            };
            self.updates.lock().unwrap().push_back(update);
            (self.notify)();
        } else {
            self.bytes_since_emit.set(since);
        }
    }
}

// ---------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------

enum Job {
    Run(OpRequest, Arc<AtomicBool>),
    Undo(u64, Undo, PathBuf, Arc<AtomicBool>),
}

/// The engine: one worker thread, a queue, a cancel flag per job.
///
/// Threading contract for whoever wires this to the UI:
/// - [`Ops::drain`] never blocks; it just swaps out a small buffer behind a
///   mutex, so it is safe to call every frame.
/// - `notify` runs on the worker thread, inside whatever pushed the
///   update, so it must not itself try to touch UI state directly — it
///   exists purely to raise a signal the UI thread will see.
/// - Dropping `Ops` drops the job sender, which makes the worker's next
///   `recv()` return `Err` and the thread exit — but the thread is never
///   joined and a job already in flight is not interrupted by the drop
///   (only [`Ops::cancel`], called before the drop, can stop it). A job
///   that outlives its `Ops` finishes writing to a queue nobody will ever
///   drain again; callers that care about a clean shutdown should cancel
///   every outstanding id first and wait for `busy()` to go false.
pub struct Ops {
    request_tx: mpsc::Sender<Job>,
    updates: Arc<Mutex<VecDeque<OpUpdate>>>,
    cancel_flags: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    busy_count: Arc<AtomicUsize>,
}

impl Ops {
    /// `notify` is called (from the worker thread) whenever an update is
    /// queued, so the UI can wake itself. Pass a closure that raises the
    /// framework's UI signal.
    pub fn new(notify: Box<dyn Fn() + Send + Sync>) -> Self {
        let (request_tx, request_rx) = mpsc::channel::<Job>();
        let updates: Arc<Mutex<VecDeque<OpUpdate>>> = Arc::new(Mutex::new(VecDeque::new()));
        let cancel_flags: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>> = Arc::new(Mutex::new(HashMap::new()));
        let busy_count = Arc::new(AtomicUsize::new(0));
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::from(notify);

        let worker_updates = updates.clone();
        let worker_cancel_flags = cancel_flags.clone();
        let worker_busy_count = busy_count.clone();
        let worker_notify = notify.clone();
        thread::spawn(move || {
            worker_loop(request_rx, worker_updates, worker_cancel_flags, worker_busy_count, worker_notify);
        });

        Ops { request_tx, updates, cancel_flags, busy_count }
    }

    /// Queue a job. `request.id` (chosen by the caller) is what later
    /// [`OpUpdate`]s and [`Ops::cancel`] calls refer back to.
    pub fn submit(&self, request: OpRequest) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_flags.lock().unwrap().insert(request.id, cancel.clone());
        self.busy_count.fetch_add(1, Ordering::SeqCst);
        let _ = self.request_tx.send(Job::Run(request, cancel));
    }

    /// Queue the reversal of a finished operation, under a fresh id so it
    /// gets its own progress row and its own `Done`/`Failed` update.
    pub fn submit_undo(&self, id: u64, undo: Undo, home: PathBuf) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel_flags.lock().unwrap().insert(id, cancel.clone());
        self.busy_count.fetch_add(1, Ordering::SeqCst);
        let _ = self.request_tx.send(Job::Undo(id, undo, home, cancel));
    }

    /// Everything the worker finished or progressed since the last call.
    /// Non-blocking: this only ever holds the mutex long enough to swap a
    /// `VecDeque` out, never while the worker itself is doing filesystem
    /// work.
    pub fn drain(&self) -> Vec<OpUpdate> {
        let mut guard = self.updates.lock().unwrap();
        guard.drain(..).collect()
    }

    /// Ask the running job to stop. A cancelled copy leaves what it
    /// already wrote — the resulting `Done` message says so rather than
    /// silently deleting it, so the user sees exactly what happened.
    pub fn cancel(&self, id: u64) {
        if let Some(flag) = self.cancel_flags.lock().unwrap().get(&id) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// True while a job is queued or running: the progress row's
    /// visibility.
    pub fn busy(&self) -> bool {
        self.busy_count.load(Ordering::SeqCst) > 0
    }
}

impl Default for Ops {
    fn default() -> Self {
        Ops::new(Box::new(|| {}))
    }
}

fn worker_loop(
    request_rx: mpsc::Receiver<Job>,
    updates: Arc<Mutex<VecDeque<OpUpdate>>>,
    cancel_flags: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    busy_count: Arc<AtomicUsize>,
    notify: Arc<dyn Fn() + Send + Sync>,
) {
    // `recv()` blocks until a job arrives and returns `Err` only once every
    // `Sender` (i.e. every `Ops`) has been dropped — that Err is this
    // thread's only exit path.
    while let Ok(job) = request_rx.recv() {
        let (id, update) = match job {
            Job::Run(request, cancel) => (request.id, run_request(&request, &cancel, &updates, &notify)),
            Job::Undo(id, undo, home, cancel) => (id, run_undo(id, &undo, &home, &cancel, &updates, &notify)),
        };
        push_update(&updates, &notify, update);
        cancel_flags.lock().unwrap().remove(&id);
        busy_count.fetch_sub(1, Ordering::SeqCst);
    }
}

fn push_update(updates: &Arc<Mutex<VecDeque<OpUpdate>>>, notify: &Arc<dyn Fn() + Send + Sync>, update: OpUpdate) {
    updates.lock().unwrap().push_back(update);
    notify();
}

fn run_request(
    request: &OpRequest,
    cancel: &AtomicBool,
    updates: &Arc<Mutex<VecDeque<OpUpdate>>>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> OpUpdate {
    match request.kind {
        OpKind::NewFolder => run_new_folder(request),
        OpKind::Rename => run_rename(request),
        OpKind::Copy => run_copy(request, cancel, updates, notify),
        OpKind::Move => run_move(request, cancel, updates, notify),
        OpKind::Trash => run_trash(request, cancel, updates, notify),
        OpKind::Delete => run_delete(request),
    }
}

/// Erase every source outright. No undo entry comes back — there is nothing
/// to put back.
fn run_delete(request: &OpRequest) -> OpUpdate {
    let mut gone = 0usize;
    for source in &request.sources {
        let result = if source.is_dir() && !source.is_symlink() {
            fs::remove_dir_all(source)
        } else {
            fs::remove_file(source)
        };
        if let Err(error) = result {
            return OpUpdate::Failed {
                id: request.id,
                kind: request.kind,
                message: format!("Could not delete {}: {error}", display_name(source)),
            };
        }
        gone += 1;
    }
    OpUpdate::Done {
        id: request.id,
        kind: request.kind,
        message: format!(
            "Deleted {gone} item{} permanently",
            if gone == 1 { "" } else { "s" }
        ),
        undo: None,
        touched: Vec::new(),
    }
}

fn run_new_folder(request: &OpRequest) -> OpUpdate {
    let name = request.new_name.as_deref().unwrap_or("New Folder");
    let path = unique_path(&request.dest_dir, name);
    match fs::create_dir(&path) {
        Ok(()) => OpUpdate::Done {
            id: request.id,
            kind: OpKind::NewFolder,
            message: format!("Created \"{}\"", display_name(&path)),
            undo: created_undo(vec![path.clone()]),
            touched: vec![path],
        },
        Err(error) => OpUpdate::Failed {
            id: request.id,
            kind: OpKind::NewFolder,
            message: format!("Could not create folder: {error}"),
        },
    }
}

fn run_rename(request: &OpRequest) -> OpUpdate {
    let Some(old_path) = request.sources.first() else {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Rename,
            message: "Rename needs a source".to_string(),
        };
    };
    let Some(new_name) = request.new_name.as_deref() else {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Rename,
            message: "Rename needs a new name".to_string(),
        };
    };
    let new_path = request.dest_dir.join(new_name);
    if &new_path != old_path && new_path.exists() {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Rename,
            message: format!("\"{new_name}\" already exists"),
        };
    }
    match fs::rename(old_path, &new_path) {
        Ok(()) => OpUpdate::Done {
            id: request.id,
            kind: OpKind::Rename,
            message: format!("Renamed to \"{new_name}\""),
            undo: moved_undo(vec![(old_path.clone(), new_path.clone())]),
            touched: vec![new_path],
        },
        Err(error) => OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Rename,
            message: format!("Could not rename: {error}"),
        },
    }
}

fn run_copy(
    request: &OpRequest,
    cancel: &AtomicBool,
    updates: &Arc<Mutex<VecDeque<OpUpdate>>>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> OpUpdate {
    if let Some(message) = refuse_into_self(&request.sources, &request.dest_dir) {
        return OpUpdate::Failed { id: request.id, kind: OpKind::Copy, message };
    }
    if let Err(error) = fs::create_dir_all(&request.dest_dir) {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Copy,
            message: format!("Could not use destination: {error}"),
        };
    }

    let total: u64 = request.sources.iter().map(|s| total_bytes(s, cancel)).sum();
    let progress = Progress::new(request.id, OpKind::Copy, total, updates.clone(), notify.clone());

    let mut touched = Vec::new();
    let mut cancelled = false;
    let mut failure = None;
    for source in &request.sources {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let target = unique_path(&request.dest_dir, &name);
        progress.set_current(&name);
        // Recorded before the copy runs: even a cancelled or failed copy
        // may have written a partial tree at `target` that a caller's undo
        // needs to know about to fully clean up.
        touched.push(target.clone());
        if let Err(error) = copy_tree(source, &target, cancel, &|delta| progress.on_bytes(delta)) {
            if cancel.load(Ordering::SeqCst) {
                cancelled = true;
            } else {
                failure = Some(format!("{name}: {error}"));
            }
            break;
        }
    }

    if cancelled {
        return OpUpdate::Done {
            id: request.id,
            kind: OpKind::Copy,
            message: format!("Cancelled: copied {} of {} item(s)", touched.len(), request.sources.len()),
            undo: created_undo(touched.clone()),
            touched,
        };
    }
    if let Some(message) = failure {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Copy,
            message: format!("Copy failed: {message}"),
        };
    }
    OpUpdate::Done {
        id: request.id,
        kind: OpKind::Copy,
        message: format!("Copied {} item(s)", touched.len()),
        undo: created_undo(touched.clone()),
        touched,
    }
}

fn run_move(
    request: &OpRequest,
    cancel: &AtomicBool,
    updates: &Arc<Mutex<VecDeque<OpUpdate>>>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> OpUpdate {
    if let Some(message) = refuse_into_self(&request.sources, &request.dest_dir) {
        return OpUpdate::Failed { id: request.id, kind: OpKind::Move, message };
    }
    if let Err(error) = fs::create_dir_all(&request.dest_dir) {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Move,
            message: format!("Could not use destination: {error}"),
        };
    }

    let dest_canon = fs::canonicalize(&request.dest_dir).ok();
    // A no-op paste (cut, then paste back onto the same folder) shouldn't
    // make the progress bar pretend there is work to measure, so those
    // sources are excluded from the total up front.
    let movers: Vec<&PathBuf> = request
        .sources
        .iter()
        .filter(|s| !already_there(s, &request.dest_dir, dest_canon.as_deref()))
        .collect();
    let total: u64 = movers.iter().map(|s| total_bytes(s, cancel)).sum();
    let mover_count = movers.len();
    let progress = Progress::new(request.id, OpKind::Move, total, updates.clone(), notify.clone());

    let mut moved_pairs = Vec::new();
    let mut touched = Vec::new();
    let mut skipped = 0usize;
    let mut cancelled = false;
    let mut failure = None;
    for source in &request.sources {
        if already_there(source, &request.dest_dir, dest_canon.as_deref()) {
            skipped += 1;
            touched.push(source.clone());
            continue;
        }
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let target = unique_path(&request.dest_dir, &name);
        progress.set_current(&name);
        match move_path(source, &target, cancel, &|delta| progress.on_bytes(delta)) {
            Ok(()) => {
                moved_pairs.push((source.clone(), target.clone()));
                touched.push(target);
            }
            Err(_) if cancel.load(Ordering::SeqCst) => {
                cancelled = true;
                break;
            }
            Err(error) => {
                failure = Some(format!("{name}: {error}"));
                break;
            }
        }
    }

    if cancelled {
        return OpUpdate::Done {
            id: request.id,
            kind: OpKind::Move,
            message: format!("Cancelled: moved {} of {} item(s)", moved_pairs.len(), mover_count),
            undo: moved_undo(moved_pairs),
            touched,
        };
    }
    if let Some(message) = failure {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Move,
            message: format!("Move failed: {message}"),
        };
    }
    if moved_pairs.is_empty() && skipped > 0 {
        return OpUpdate::Done {
            id: request.id,
            kind: OpKind::Move,
            message: "Nothing to move — already there".to_string(),
            undo: None,
            touched,
        };
    }
    let message = if skipped > 0 {
        format!("Moved {} item(s) ({} already there)", moved_pairs.len(), skipped)
    } else {
        format!("Moved {} item(s)", moved_pairs.len())
    };
    OpUpdate::Done {
        id: request.id,
        kind: OpKind::Move,
        message,
        undo: moved_undo(moved_pairs),
        touched,
    }
}

fn run_trash(
    request: &OpRequest,
    cancel: &AtomicBool,
    updates: &Arc<Mutex<VecDeque<OpUpdate>>>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> OpUpdate {
    let trash = trash_dir(&request.home);
    if let Err(error) = fs::create_dir_all(&trash) {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Trash,
            message: format!("Could not reach Trash: {error}"),
        };
    }

    let total: u64 = request.sources.iter().map(|s| total_bytes(s, cancel)).sum();
    let progress = Progress::new(request.id, OpKind::Trash, total, updates.clone(), notify.clone());

    let mut pairs = Vec::new();
    let mut touched = Vec::new();
    let mut cancelled = false;
    let mut failure = None;
    for source in &request.sources {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        // Collisions in the trash go through the same disambiguation as
        // everywhere else — two different "notes.txt" trashed on the same
        // day must not clobber one another.
        let target = unique_path(&trash, &name);
        progress.set_current(&name);
        match move_path(source, &target, cancel, &|delta| progress.on_bytes(delta)) {
            Ok(()) => {
                pairs.push((source.clone(), target.clone()));
                touched.push(target);
            }
            Err(_) if cancel.load(Ordering::SeqCst) => {
                cancelled = true;
                break;
            }
            Err(error) => {
                failure = Some(format!("{name}: {error}"));
                break;
            }
        }
    }

    if cancelled {
        return OpUpdate::Done {
            id: request.id,
            kind: OpKind::Trash,
            message: format!("Cancelled: moved {} of {} item(s) to Trash", pairs.len(), request.sources.len()),
            undo: moved_undo(pairs),
            touched,
        };
    }
    if let Some(message) = failure {
        return OpUpdate::Failed {
            id: request.id,
            kind: OpKind::Trash,
            message: format!("Could not move to Trash: {message}"),
        };
    }
    OpUpdate::Done {
        id: request.id,
        kind: OpKind::Trash,
        message: format!("Moved {} item(s) to Trash", pairs.len()),
        undo: moved_undo(pairs),
        touched,
    }
}

/// `home` is accepted for symmetry with [`OpRequest`] (and in case a
/// future `Undo` variant needs to relocate something relative to it), but
/// neither current variant needs it: both already carry fully-resolved
/// paths, which is exactly what makes them reversible without recomputing
/// anything about where they came from.
fn run_undo(
    id: u64,
    undo: &Undo,
    _home: &Path,
    cancel: &AtomicBool,
    updates: &Arc<Mutex<VecDeque<OpUpdate>>>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> OpUpdate {
    match undo {
        Undo::Moved { pairs } => run_undo_moved(id, pairs, cancel, updates, notify),
        Undo::Created { paths } => run_undo_created(id, paths, cancel, updates, notify),
    }
}

fn run_undo_moved(
    id: u64,
    pairs: &[(PathBuf, PathBuf)],
    cancel: &AtomicBool,
    updates: &Arc<Mutex<VecDeque<OpUpdate>>>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> OpUpdate {
    let total: u64 = pairs.iter().map(|(_, to)| total_bytes(to, cancel)).sum();
    // Reported as a Move: undoing a rename, a move, or a trash is always
    // itself a move, in the other direction.
    let progress = Progress::new(id, OpKind::Move, total, updates.clone(), notify.clone());

    let mut restored = Vec::new();
    let mut cancelled = false;
    let mut failure = None;
    for (from, to) in pairs {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        progress.set_current(&display_name(to));
        match move_path(to, from, cancel, &|delta| progress.on_bytes(delta)) {
            Ok(()) => restored.push(from.clone()),
            Err(_) if cancel.load(Ordering::SeqCst) => {
                cancelled = true;
                break;
            }
            Err(error) => {
                failure = Some(format!("{}: {error}", display_name(from)));
                break;
            }
        }
    }

    if cancelled {
        return OpUpdate::Done {
            id,
            kind: OpKind::Move,
            message: format!("Cancelled undo: restored {} of {} item(s)", restored.len(), pairs.len()),
            undo: None,
            touched: restored,
        };
    }
    if let Some(message) = failure {
        return OpUpdate::Failed { id, kind: OpKind::Move, message: format!("Undo failed: {message}") };
    }
    OpUpdate::Done {
        id,
        kind: OpKind::Move,
        message: format!("Undid move of {} item(s)", restored.len()),
        undo: None,
        touched: restored,
    }
}

fn run_undo_created(
    id: u64,
    paths: &[PathBuf],
    cancel: &AtomicBool,
    updates: &Arc<Mutex<VecDeque<OpUpdate>>>,
    notify: &Arc<dyn Fn() + Send + Sync>,
) -> OpUpdate {
    // Undoing a creation deletes rather than transfers bytes, so progress
    // here counts items, not bytes — see the note on `OpUpdate::Progress`.
    // Reported under `OpKind::Copy`: today the only source of a `Created`
    // undo the UI offers to reverse this way is a finished Copy (a
    // NewFolder's own undo is rarely surfaced as a re-doable action), and
    // there is no dedicated `OpKind` for "delete" to report instead.
    let total = paths.len() as u64;
    let mut done = 0u64;
    let mut last_emit = Instant::now();
    let mut removed = Vec::new();
    let mut cancelled = false;
    let mut failure = None;
    for path in paths {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        match remove_path(path) {
            Ok(()) => {
                removed.push(path.clone());
                done += 1;
                if done == total || last_emit.elapsed() >= Duration::from_millis(32) {
                    last_emit = Instant::now();
                    push_update(
                        updates,
                        notify,
                        OpUpdate::Progress { id, kind: OpKind::Copy, done, total, current: display_name(path) },
                    );
                }
            }
            Err(error) => {
                failure = Some(format!("{}: {error}", display_name(path)));
                break;
            }
        }
    }

    if cancelled {
        return OpUpdate::Done {
            id,
            kind: OpKind::Copy,
            message: format!("Cancelled undo: removed {} of {} item(s)", removed.len(), paths.len()),
            undo: None,
            touched: removed,
        };
    }
    if let Some(message) = failure {
        return OpUpdate::Failed { id, kind: OpKind::Copy, message: format!("Undo failed: {message}") };
    }
    OpUpdate::Done {
        id,
        kind: OpKind::Copy,
        message: format!("Undid creation of {} item(s)", removed.len()),
        undo: None,
        touched: removed,
    }
}

// ---------------------------------------------------------------------
// Undo journal
// ---------------------------------------------------------------------

/// The undo stack. Bounded, because a file browser that remembers forever
/// is a file browser that holds paths to files nobody has any more — an
/// undo from an hour and two hundred operations ago is more likely to
/// surprise than help.
pub struct Journal {
    stack: VecDeque<Undo>,
}

impl Journal {
    /// At least 10 steps are kept, per the app's contract.
    pub const DEPTH: usize = 32;

    pub fn new() -> Self {
        Journal { stack: VecDeque::new() }
    }

    pub fn push(&mut self, undo: Undo) {
        self.stack.push_back(undo);
        while self.stack.len() > Self::DEPTH {
            self.stack.pop_front();
        }
    }

    pub fn pop(&mut self) -> Option<Undo> {
        self.stack.pop_back()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn peek(&self) -> Option<&Undo> {
        self.stack.back()
    }
}

impl Default for Journal {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory under the system temp dir, unique to this
    /// test run so parallel `cargo test` threads never collide. Every test
    /// that touches disk creates its own with this and removes it with
    /// [`cleanup`] — nothing here ever reads or writes outside of it.
    fn fresh_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::AtomicU64;
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mpfiles-ops-test-{tag}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    /// Polls `drain()` until a `Done`/`Failed` for `id` shows up, bounded
    /// by `timeout` so a bug in the engine fails the test instead of
    /// hanging the suite.
    fn wait_for_done(ops: &Ops, id: u64, timeout: Duration) -> OpUpdate {
        let start = Instant::now();
        loop {
            for update in ops.drain() {
                let is_match = match &update {
                    OpUpdate::Done { id: uid, .. } | OpUpdate::Failed { id: uid, .. } => *uid == id,
                    OpUpdate::Progress { .. } => false,
                };
                if is_match {
                    return update;
                }
            }
            if start.elapsed() > timeout {
                panic!("timed out waiting for update {id}");
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn unique_path_avoids_collisions() {
        let dir = fresh_dir("unique");
        assert_eq!(unique_path(&dir, "report.txt"), dir.join("report.txt"));
        fs::write(dir.join("report.txt"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "report.txt"), dir.join("report (2).txt"));
        fs::write(dir.join("report (2).txt"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "report.txt"), dir.join("report (3).txt"));
        // A dotfile has no extension to protect: the suffix goes at the end.
        fs::write(dir.join(".zshrc"), b"x").unwrap();
        assert_eq!(unique_path(&dir, ".zshrc"), dir.join(".zshrc (2)"));
        // An extensionless name behaves the same way.
        fs::write(dir.join("README"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "README"), dir.join("README (2)"));
        cleanup(&dir);
    }

    #[test]
    fn copy_tree_preserves_bytes() {
        let root = fresh_dir("copytree");
        let src = root.join("src");
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("top.txt"), b"hello").unwrap();
        fs::write(src.join("a/mid.txt"), b"middle file").unwrap();
        fs::write(src.join("a/b/deep.bin"), vec![7u8; 5000]).unwrap();
        let expected_total = 5u64 + 11 + 5000;

        let dst = root.join("dst");
        let cancel = AtomicBool::new(false);
        let total = Cell::new(0u64);
        copy_tree(&src, &dst, &cancel, &|n| total.set(total.get() + n)).unwrap();

        assert_eq!(total.get(), expected_total);
        assert_eq!(fs::read(dst.join("top.txt")).unwrap(), b"hello");
        assert_eq!(fs::read(dst.join("a/mid.txt")).unwrap(), b"middle file");
        assert_eq!(fs::read(dst.join("a/b/deep.bin")).unwrap(), vec![7u8; 5000]);
        cleanup(&root);
    }

    #[test]
    fn copy_tree_cancellation_copies_nothing() {
        let root = fresh_dir("cancel");
        let src = root.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("f.bin"), vec![1u8; 1000]).unwrap();
        let dst = root.join("dst");

        let cancel = AtomicBool::new(true); // pre-cancelled
        let result = copy_tree(&src, &dst, &cancel, &|_| {});
        assert!(result.is_err());
        assert!(!dst.exists(), "a pre-cancelled copy must not create the destination at all");
        cleanup(&root);
    }

    #[test]
    fn move_path_same_volume_moves_bytes() {
        let root = fresh_dir("move");
        let src = root.join("f.bin");
        fs::write(&src, b"payload").unwrap();
        let dst = root.join("moved.bin");

        let cancel = AtomicBool::new(false);
        move_path(&src, &dst, &cancel, &|_| {}).unwrap();

        assert!(!src.exists());
        assert_eq!(fs::read(&dst).unwrap(), b"payload");
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn total_bytes_ignores_symlinked_content() {
        let root = fresh_dir("totalbytes");
        let real = root.join("big.bin");
        fs::write(&real, vec![9u8; 10_000]).unwrap();
        let tree = root.join("tree");
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join("small.txt"), b"hi").unwrap(); // 2 bytes
        std::os::unix::fs::symlink(&real, tree.join("link.bin")).unwrap();

        let cancel = AtomicBool::new(false);
        assert_eq!(total_bytes(&tree, &cancel), 2, "the symlink's target must never be counted");
        cleanup(&root);
    }

    #[test]
    fn refuses_copy_into_own_descendant() {
        let root = fresh_dir("selfcopy");
        let src = root.join("folder");
        fs::create_dir_all(src.join("child")).unwrap();

        assert!(refuse_into_self(&[src.clone()], &src.join("child")).is_some());
        assert!(refuse_into_self(&[src.clone()], &src).is_some());

        let elsewhere = root.join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        assert!(refuse_into_self(&[src.clone()], &elsewhere).is_none());
        cleanup(&root);
    }

    #[test]
    fn journal_is_bounded_lifo() {
        let mut journal = Journal::new();
        assert!(Journal::DEPTH >= 10);
        for i in 0..(Journal::DEPTH + 5) {
            journal.push(Undo::Created { paths: vec![PathBuf::from(format!("/x/{i}"))] });
        }
        assert_eq!(journal.len(), Journal::DEPTH);

        // Most recently pushed comes back first...
        match journal.pop().unwrap() {
            Undo::Created { paths } => assert_eq!(paths[0], PathBuf::from(format!("/x/{}", Journal::DEPTH + 4))),
            other => panic!("wrong variant: {other:?}"),
        }
        assert_eq!(journal.len(), Journal::DEPTH - 1);
        // ...and the oldest 5 were dropped, not the newest.
        let mut journal2 = Journal::new();
        for i in 0..(Journal::DEPTH + 5) {
            journal2.push(Undo::Created { paths: vec![PathBuf::from(format!("/y/{i}"))] });
        }
        for _ in 0..Journal::DEPTH {
            journal2.pop().unwrap();
        }
        assert!(journal2.peek().is_none());
    }

    #[test]
    fn ops_copy_end_to_end_then_undo() {
        let root = fresh_dir("e2e-copy");
        let src_dir = root.join("src");
        fs::create_dir_all(src_dir.join("nested")).unwrap();
        fs::write(src_dir.join("a.txt"), b"aaa").unwrap();
        fs::write(src_dir.join("nested/b.txt"), b"bbb").unwrap();
        let dest_dir = root.join("dest");
        fs::create_dir_all(&dest_dir).unwrap();

        let ops = Ops::default();
        ops.submit(OpRequest {
            id: 1,
            kind: OpKind::Copy,
            sources: vec![src_dir.clone()],
            dest_dir: dest_dir.clone(),
            new_name: None,
            home: std::env::temp_dir(),
        });

        let (undo, touched) = match wait_for_done(&ops, 1, Duration::from_secs(5)) {
            OpUpdate::Done { undo, touched, .. } => (undo, touched),
            other => panic!("expected Done, got {other:?}"),
        };
        assert_eq!(touched.len(), 1);
        let copied = touched[0].clone();
        assert!(copied.join("a.txt").exists());
        assert_eq!(fs::read(copied.join("nested/b.txt")).unwrap(), b"bbb");

        let undo = match undo {
            Some(Undo::Created { paths }) => paths,
            other => panic!("expected Created undo, got {other:?}"),
        };
        assert_eq!(undo, touched);

        ops.submit_undo(2, Undo::Created { paths: undo }, std::env::temp_dir());
        wait_for_done(&ops, 2, Duration::from_secs(5));
        assert!(!copied.exists(), "undoing the copy must remove what it created");

        cleanup(&root);
    }

    #[test]
    fn ops_trash_into_fake_home_then_undo() {
        let root = fresh_dir("e2e-trash");
        let fake_home = root.join("fakehome");
        fs::create_dir_all(&fake_home).unwrap();
        let victim_dir = root.join("victim_dir");
        fs::create_dir_all(&victim_dir).unwrap();
        let file = victim_dir.join("doomed.txt");
        fs::write(&file, b"bye").unwrap();

        let ops = Ops::default();
        ops.submit(OpRequest {
            id: 10,
            kind: OpKind::Trash,
            sources: vec![file.clone()],
            dest_dir: victim_dir.clone(),
            new_name: None,
            home: fake_home.clone(), // never the real home
        });

        let undo = match wait_for_done(&ops, 10, Duration::from_secs(5)) {
            OpUpdate::Done { undo: Some(undo), .. } => undo,
            other => panic!("expected Done with undo, got {other:?}"),
        };
        assert!(!file.exists());
        assert!(trash_dir(&fake_home).join("doomed.txt").exists());

        ops.submit_undo(11, undo, fake_home.clone());
        wait_for_done(&ops, 11, Duration::from_secs(5));
        assert!(file.exists());
        assert_eq!(fs::read(&file).unwrap(), b"bye");

        cleanup(&root);
    }

    #[test]
    fn ops_move_onto_same_folder_is_noop() {
        let root = fresh_dir("move-noop");
        let dir = root.join("here");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("stay.txt");
        fs::write(&file, b"still here").unwrap();

        let ops = Ops::default();
        ops.submit(OpRequest {
            id: 20,
            kind: OpKind::Move,
            sources: vec![file.clone()],
            dest_dir: dir.clone(),
            new_name: None,
            home: std::env::temp_dir(),
        });
        match wait_for_done(&ops, 20, Duration::from_secs(5)) {
            OpUpdate::Done { undo, .. } => assert!(undo.is_none(), "a no-op paste must not produce an undo entry"),
            other => panic!("expected Done, got {other:?}"),
        }
        assert!(file.exists());
        assert_eq!(fs::read(&file).unwrap(), b"still here");
        cleanup(&root);
    }

    #[test]
    fn ops_copy_into_same_folder_gets_suffix() {
        let root = fresh_dir("copy-suffix");
        let dir = root.join("here");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dup.txt");
        fs::write(&file, b"original").unwrap();

        let ops = Ops::default();
        ops.submit(OpRequest {
            id: 30,
            kind: OpKind::Copy,
            sources: vec![file.clone()],
            dest_dir: dir.clone(),
            new_name: None,
            home: std::env::temp_dir(),
        });
        match wait_for_done(&ops, 30, Duration::from_secs(5)) {
            OpUpdate::Done { touched, .. } => {
                assert_eq!(touched, vec![dir.join("dup (2).txt")]);
                assert!(file.exists(), "the original must be untouched by a copy of itself");
            }
            other => panic!("expected Done, got {other:?}"),
        }
        cleanup(&root);
    }
}
