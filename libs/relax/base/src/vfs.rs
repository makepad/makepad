//! Every file the front end reads goes through here (local/plans/run-from-git.md). Two backends
//! behind one interface: the plain filesystem (today's std::fs calls; with nothing mounted it is
//! exactly that, one relaxed load in front) and git mounts: a virtual folder (never created on
//! disk) whose files come from a repository store (store.rs) at a commit. A path under a mount is
//! served from the mount (the longest mount root wins: a repository mounted inside another);
//! every other path is the disk. Paths stay plain strings everywhere else.
//!
//! EDITS (MAKEPAD_RUN_EDITS=<folder>): the files a program writes into its own virtual folders (Stage's
//! EDIT mode changing an app's sources) live in that disk folder, at the same path relative to
//! the outermost mount, on top of the store: a file there is read instead of the store's. Only
//! edited files exist there. `.makepad-run-edits` in it records, per edited file, the store blob it
//! was edited from (`new` for a file the store lacks), and `.makepad-run-base/<path>` keeps that blob.
//! At a run's start (`reconcile_edits`) edits whose store file is unchanged carry over to the
//! release being run; when the release changed the file too, the edit moves to
//! `.makepad-run-conflicts/<commit>/<path>` (beside it `<path>.base`) and the run reports it -- an edit
//! is never dropped silently. Without MAKEPAD_RUN_EDITS virtual files stay read-only.
//!
//! SEED (MAKEPAD_RUN_EDITS_SEED=<folder>[,<folder>...], relative to the outermost mount): at a run's
//! start the store's files under those folders are written into the edits folder where it has none
//! (`seed_edits`), so a tool outside the run (a coding agent) finds an app's whole source on disk and
//! edits it in place. `.makepad-run-seeded` records each written file's blob: a seeded file whose
//! bytes still are that blob is not an edit; `reconcile_edits` brings it to the release being run.
//! One whose bytes changed becomes an edit (recorded in `.makepad-run-edits` from that blob).
use crate::gitstore::{self as store, Entry, Oid, Store};
use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

struct Mount {
    /// the virtual folder, without a trailing separator
    root: String,
    store: Arc<Store>,
    commit: Oid,
    /// the commit's files, sorted by path
    files: Arc<[Entry]>,
    /// folder -> direct children (name, is folder), built on first listing
    folders: OnceLock<BTreeMap<String, Vec<(String, bool)>>>,
}

static ANY: AtomicBool = AtomicBool::new(false);

// ---- the read log (MAKEPAD_RUN_READ_LOG=<file>): what `pack-src` records. Every process of a run (the
// build-script children inherit the variable) appends `<kind>\t<path>` once per path and kind:
// `f` a file whose bytes were read, `p` a path a probe (metadata, exists, is_file, is_dir) found,
// `d` a folder that was listed. One relaxed load when off.
static RECORDING: AtomicBool = AtomicBool::new(false);
static RECORDED: relax_session::sync::Mutex<Option<(std::collections::HashSet<(u8, String)>, std::fs::File)>> = relax_session::sync::Mutex::new(None);

/// Starts the read log from MAKEPAD_RUN_READ_LOG (the loader calls this at start, next to MAKEPAD_RUN_VFS).
pub fn record_from_env() -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    let Some(p) = std::env::var_os("MAKEPAD_RUN_READ_LOG").filter(|p| !p.is_empty()) else { return Ok(()) };
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| format!("MAKEPAD_RUN_READ_LOG {}: {e}", p.to_string_lossy()))?;
    *RECORDED.lock().unwrap() = Some((Default::default(), f));
    RECORDING.store(true, Ordering::Release);
    Ok(())
}

fn note(kind: u8, path: &std::path::Path) {
    if !RECORDING.load(Ordering::Relaxed) {
        return;
    }
    let p = normalize_path(&path.to_string_lossy());
    let _k = crate::zone_alloc::kept();
    let mut g = RECORDED.lock().unwrap();
    if let Some((seen, file)) = g.as_mut() {
        if seen.insert((kind, p.clone())) {
            use std::io::Write;
            // one write per line: appends of different processes do not interleave
            let _ = file.write_all(format!("{}\t{p}\n", kind as char).as_bytes());
        }
    }
}
static MOUNTS: RwLock<Vec<Arc<Mount>>> = RwLock::new(Vec::new());
/// MAKEPAD_RUN_EDITS: the disk folder holding the edited files (None: virtual files are read-only)
static EDITS: RwLock<Option<std::path::PathBuf>> = RwLock::new(None);
// A verified binary std always reads the store bytes that were compared, even with app edits.
static FROZEN: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Verify a complete subtree directly from the store (no disk/edits reads), then protect it
/// from the app's edit overlay. Paths and bytes use the same digest as the compiler build.
pub fn verify_std(path: &str, expected: &str) -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    let (m, rel) = find(path).ok_or("no makepad run std mount")?;
    let prefix = if rel.is_empty() { String::new() } else { format!("{rel}/") };
    let mut files = Vec::new();
    for e in m.files.iter().filter(|e| e.path.starts_with(&prefix)) {
        if !matches!(e.mode, 0o100644 | 0o100755) {
            return Err(format!("std contains a non-regular file: {}", e.path));
        }
        let (_, bytes) = m.store.object(&e.oid).map_err(|e| e.to_string())?.ok_or("std object missing")?;
        files.push((e.path[prefix.len()..].to_string(), bytes));
    }
    if files.is_empty() {
        return Err("mounted commit has no makepad run std".into());
    }
    let actual = crate::std_digest::digest(files);
    if actual != expected {
        return Err(format!("std digest mismatch (binary {expected}, store {actual})"));
    }
    // A nested mount would change what readers see after verification.
    let path = normalize_path(path);
    if MOUNTS.read().unwrap().iter().any(|m| relative(&m.root, &path).is_some_and(|r| !r.is_empty())) {
        return Err("std contains a nested mount".into());
    }
    let mut frozen = FROZEN.write().unwrap();
    if !frozen.contains(&path) {
        frozen.push(path);
    }
    Ok(())
}

/// Serves `root` (a folder that does not exist on disk) from `store` at `commit`.
pub fn mount(root: &str, store: Arc<Store>, commit: &Oid) -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    let files: Arc<[Entry]> = store.index.table(commit).ok_or_else(|| format!("{}: commit {} is not kept", store.path.display(), store::hex(commit)))?.to_vec().into();
    let root = normalize_path(root);
    let mut m = MOUNTS.write().unwrap();
    m.retain(|x| x.root != root);
    m.push(Arc::new(Mount { root, store, commit: *commit, files, folders: OnceLock::new() }));
    // longest root first: a repository mounted inside another one wins for its folder
    m.sort_by(|a, b| b.root.len().cmp(&a.root.len()));
    ANY.store(true, Ordering::Release);
    Ok(())
}

/// Whether `path` lies under a git mount (callers that hand paths to other programs check this).
pub fn is_virtual(path: &str) -> bool {
    ANY.load(Ordering::Acquire) && find(path).is_some()
}

/// One lexical spelling for VFS paths on every host OS. For host-independent lookups,
/// separators, verbatim drive/UNC prefixes and drive-letter case are normalized;
/// authored component case is kept. Mounts have no symlinked folders, so `..` is
/// resolved here too. Disk operations still receive the caller's original path.
pub fn normalize_path(path: &str) -> String {
    normalize_components(path, true)
}

fn normalize_components(path: &str, parents: bool) -> String {
    let spelling = path.replace('\\', "/");
    let mut rest = spelling.as_str();
    let drive = |s: &str| s.len() >= 2 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1] == b':';
    let mut prefix = String::new();
    if let Some(verbatim) = rest.strip_prefix("//?/") {
        if verbatim.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("UNC/")) {
            prefix.push_str("//");
            rest = &verbatim[4..];
        } else if drive(verbatim) {
            rest = verbatim;
        }
    }
    if drive(rest) {
        prefix.push((rest.as_bytes()[0] as char).to_ascii_lowercase());
        prefix.push(':');
        rest = &rest[2..];
        if rest.starts_with('/') { prefix.push('/'); }
    } else if rest.starts_with("//") {
        prefix.push_str("//");
    } else if rest.starts_with('/') {
        prefix.push('/');
    }
    // An absolute path cannot climb above its drive/root or UNC server/share.
    let rooted = prefix.ends_with('/');
    let floor = if prefix == "//" { 2 } else { 0 };
    let mut parts: Vec<&str> = Vec::new();
    for p in rest.split('/') {
        match p {
            "" | "." => {}
            ".." if parents => {
                if parts.len() > floor && parts.last() != Some(&"..") {
                    parts.pop();
                } else if !rooted {
                    parts.push(p);
                }
            }
            p => parts.push(p),
        }
    }
    prefix.push_str(&parts.join("/"));
    if prefix.is_empty() && !path.is_empty() { prefix.push('.'); }
    prefix
}

/// Strip a directory prefix by normalized components, never by a partial name.
pub fn relative_path(path: &str, root: &str) -> Option<String> {
    relative(&normalize_path(path), &normalize_path(root)).map(str::to_string)
}

// Inputs already have normalized spelling. Kept separate so canonicalize can
// check `..` before resolving it, using exactly the same mount boundaries.
fn relative<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    if path == root { return Some(""); }
    if root.is_empty() { return None; }
    let prefix = |s: &str| {
        if s.len() >= 2 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1] == b':' {
            if s.as_bytes().get(2) == Some(&b'/') { 3 } else { 2 }
        } else if s.starts_with("//") { 2 } else if s.starts_with('/') { 1 } else { 0 }
    };
    if path[..prefix(path)] != root[..prefix(root)] { return None; }
    let rest = path.strip_prefix(root)?;
    if root.ends_with('/') || (root.len() == 2 && prefix(root) == 2) { Some(rest) } else { rest.strip_prefix('/') }
}

/// The mount `path` is under and the path inside it ("" for the mount's root).
fn find(path: &str) -> Option<(Arc<Mount>, String)> {
    let p = normalize_path(path);
    let m = MOUNTS.read().unwrap();
    for mount in m.iter() {
        if let Some(rest) = relative(&p, &mount.root) {
            return Some((mount.clone(), rest.to_string()));
        }
    }
    None
}

/// The outermost mount root containing `p` (normalized), and `p` relative to it.
fn outer_rel(p: &str) -> Option<String> {
    let m = MOUNTS.read().unwrap();
    let mut best: Option<&str> = None;
    for mount in m.iter() {
        if relative(p, &mount.root).is_some() {
            if best.is_none_or(|b| mount.root.len() < b.len()) {
                best = Some(&mount.root);
            }
        }
    }
    let root = best?;
    relative(p, root).map(str::to_string)
}

/// The outermost mount root containing `path` (a git run's virtual workspace root), normalized.
pub fn outer_root(path: &str) -> Option<String> {
    let p = normalize_path(path);
    let m = MOUNTS.read().unwrap();
    m.iter().filter(|mount| relative(&p, &mount.root).is_some()).map(|mount| mount.root.clone()).min_by_key(|r| r.len())
}

/// Where `path` (virtual) lives in the edits folder, whether or not it exists there.
fn edit_path(path: &str) -> Option<(std::path::PathBuf, String)> {
    let dir = EDITS.read().unwrap().clone()?;
    let path = normalize_path(path);
    if FROZEN.read().unwrap().iter().any(|p| relative(&path, p).is_some()) {
        return None;
    }
    let rel = outer_rel(&path)?;
    Some((if rel.is_empty() { dir } else { dir.join(&rel) }, rel))
}

/// The edited file for `path`, when there is one.
fn edited_file(path: &str) -> Option<std::path::PathBuf> {
    let (p, rel) = edit_path(path)?;
    if rel.starts_with(".makepad-run-") {
        return None;
    }
    p.is_file().then_some(p)
}

/// Whether virtual files are writable (an edits folder is set).
pub fn edits_enabled() -> bool {
    EDITS.read().unwrap().is_some()
}

/// The edits folder from MAKEPAD_RUN_EDITS (the runner sets it per licensed app).
pub fn set_edits(dir: Option<std::path::PathBuf>) {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    *EDITS.write().unwrap() = dir;
}

impl Mount {
    fn file(&self, rel: &str) -> Option<&Entry> {
        self.files.binary_search_by(|e| e.path.as_str().cmp(rel)).ok().map(|i| &self.files[i])
    }
    fn folders(&self) -> &BTreeMap<String, Vec<(String, bool)>> {
        // the mount is process state: its folder index too (kept heap)
        self.folders.get_or_init(|| crate::zone_alloc::kept_with(|| store::folders(&self.files)))
    }
    fn is_dir(&self, rel: &str) -> bool {
        self.folders().contains_key(rel)
    }
}

fn not_found(path: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{path}: no such file"))
}

// Read bindings: a load binds its read snapshot to the thread it runs on for the extent of a
// `ReadScope` on that thread's stack. The binding lives in the thread's own table and owns its
// snapshot (a strong reference, dropped with the scope): there is no process registry, no weak
// pointer and no entry another thread can reach. A scope is !Send, so no shared state (a preload
// pool, a Program) can keep a thread's binding alive past its load.
thread_local! {
    static BOUND: std::cell::RefCell<Vec<(u64, Arc<ReadSnapshot>)>> = Default::default();
}
static BIND_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Binds a snapshot to this thread until dropped (on this thread: it is !Send).
pub struct ReadScope { id: u64, _thread: std::marker::PhantomData<*const ()> }

impl Drop for ReadScope {
    fn drop(&mut self) {
        // (scopes may end out of order: each removes its own entry; at thread exit the table
        // may already be gone, and with it every entry)
        let entry = BOUND.try_with(|b| {
            let mut b = b.borrow_mut();
            b.iter().rposition(|e| e.0 == self.id).map(|i| b.remove(i))
        });
        // the snapshot is released after the table borrow ends
        drop(entry);
    }
}

#[derive(Default)]
struct ReadEntry {
    bytes: OnceLock<Result<Arc<[u8]>, (io::ErrorKind, String)>>,
    calls: std::sync::atomic::AtomicUsize,
    opens: std::sync::atomic::AtomicUsize,
}
#[derive(Default)]
struct ReadPaths {
    files: BTreeMap<String, Arc<ReadEntry>>,
    aliases: BTreeMap<String, String>,
    retired: Vec<(String, Arc<ReadEntry>)>,
}
/// Immutable bytes shared by prescan, lowering and loader retries. Each new
/// load/edited candidate owns a fresh table; it never serves a subsequent load. Exception: once a
/// file's image is mapped its entry is released (`release`), so an include!/include_str! of that
/// same module file from a later lazy expansion reads the disk.
#[derive(Default)]
pub struct ReadSnapshot {
    paths: relax_session::sync::Mutex<ReadPaths>,
    profile: bool,
}
impl ReadSnapshot {
    pub fn new(profile: bool) -> Arc<Self> { Arc::new(Self { profile, ..Self::default() }) }
    pub fn bind(self: &Arc<Self>) -> ReadScope {
        let id = BIND_SEQ.fetch_add(1, Ordering::Relaxed);
        // the thread's table outlives the compiler generation the load runs in
        let _k = crate::zone_alloc::kept();
        BOUND.with(|b| b.borrow_mut().push((id, self.clone())));
        ReadScope { id, _thread: std::marker::PhantomData }
    }
    pub fn alias(&self, path: &std::path::Path, canonical: &std::path::Path) {
        let path = normalize_components(&path.to_string_lossy(), false);
        let canonical = normalize_components(&canonical.to_string_lossy(), false);
        if path == canonical { return; }
        let mut paths = self.paths.lock().unwrap();
        if let Some(entry) = paths.files.remove(&path) {
            if paths.files.contains_key(&canonical) {
                // Keep every actual open in the audit, even when an alias was
                // first encountered before its canonical spelling was known.
                if self.profile { paths.retired.push((canonical.clone(), entry)); }
            } else { paths.files.insert(canonical.clone(), entry); }
        }
        paths.aliases.insert(path, canonical);
    }
    /// Drops the snapshot's copy of `path` once its bytes live elsewhere (a mapped image); the
    /// bytes stay alive only for holders that already have them.
    pub fn release(&self, path: &std::path::Path) {
        let key = normalize_components(&path.to_string_lossy(), false);
        let mut paths = self.paths.lock().unwrap();
        let key = paths.aliases.get(&key).cloned().unwrap_or(key);
        if let Some(entry) = paths.files.remove(&key) {
            if self.profile { paths.retired.push((key, entry)); }
        }
    }
    fn read_shared(&self, path: &std::path::Path) -> io::Result<Arc<[u8]>> {
        let key = normalize_components(&path.to_string_lossy(), false);
        let (key, entry) = {
            let mut paths = self.paths.lock().unwrap();
            let key = paths.aliases.get(&key).cloned().unwrap_or(key);
            let entry = paths.files.entry(key.clone()).or_default().clone();
            (key, entry)
        };
        if self.profile { entry.calls.fetch_add(1, Ordering::Relaxed); }
        entry.bytes.get_or_init(|| read_uncached(std::path::Path::new(&key), self.profile.then_some(&entry))
            .map(Arc::from).map_err(|e| (e.kind(), e.to_string())))
            .as_ref().map(Arc::clone).map_err(|(kind, message)| io::Error::new(*kind, message.clone()))
    }
    pub fn report(&self) {
        if !self.profile { return; }
        let paths = self.paths.lock().unwrap();
        let mut calls = 0;
        let mut opens = 0;
        let mut max = 0;
        let mut repeated = 0;
        let mut totals = BTreeMap::<_, (usize, usize)>::new();
        for (path, entry) in paths.files.iter().chain(paths.retired.iter().map(|(path, entry)| (path, entry))) {
            let path = paths.aliases.get(path).unwrap_or(path);
            let count = totals.entry(path.clone()).or_default();
            count.0 += entry.calls.load(Ordering::Relaxed);
            count.1 += entry.opens.load(Ordering::Relaxed);
        }
        drop(paths);
        for (path, (c, o)) in &totals {
            calls += c; opens += o; max = max.max(*o); repeated += usize::from(*o > 1);
            eprintln!("makepad-run source-read: calls={c} opens={o} path={path:?}");
        }
        eprintln!("makepad-run source-reads: calls={calls} paths={} opens={opens} repeated_paths={repeated} max_opens_per_path={max}", totals.len());
    }
}
/// The innermost snapshot bound on this thread.
fn read_snapshot() -> Option<Arc<ReadSnapshot>> {
    BOUND.try_with(|b| b.borrow().last().map(|e| e.1.clone())).ok().flatten()
}
fn read_disk(path: &std::path::Path, entry: Option<&ReadEntry>) -> io::Result<Vec<u8>> {
    if let Some(entry) = entry { entry.opens.fetch_add(1, Ordering::Relaxed); }
    std::fs::read(path)
}

/// Immutable file bytes, sharing a bound load snapshot's allocation. Callers
/// that keep authored sources need not copy the snapshot into another Arc.
pub fn read_shared(path: impl AsRef<std::path::Path>) -> io::Result<Arc<[u8]>> {
    let path = path.as_ref();
    let result = if let Some(snapshot) = read_snapshot() {
        snapshot.read_shared(path)
    } else {
        read_uncached(path, None).map(Arc::from)
    };
    if result.is_ok() { crate::why::read(path); }
    result
}

/// The file's bytes.
pub fn read(path: impl AsRef<std::path::Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    let result = if let Some(snapshot) = read_snapshot() { snapshot.read_shared(path).map(|bytes| bytes.to_vec()) } else { read_uncached(path, None) };
    if result.is_ok() { crate::why::read(path); }
    result
}

fn read_uncached(path: &std::path::Path, entry: Option<&ReadEntry>) -> io::Result<Vec<u8>> {
    let r = read_uncached_inner(path, entry);
    if r.is_ok() {
        note(b'f', path);
    }
    r
}

fn read_uncached_inner(path: &std::path::Path, entry: Option<&ReadEntry>) -> io::Result<Vec<u8>> {
    if !ANY.load(Ordering::Acquire) {
        return read_disk(path, entry);
    }
    let s = path.to_string_lossy();
    match find(&s) {
        None => read_disk(path, entry),
        Some(_) if edited_file(&s).is_some() => read_disk(&edited_file(&s).unwrap(), entry),
        Some((m, rel)) => {
            let e = m.file(&rel).ok_or_else(|| not_found(&s))?;
            if e.mode == 0o160000 {
                return Err(not_found(&s));
            }
            match m.store.object(&e.oid)? {
                Some((_, data)) => Ok(data),
                None => Err(io::Error::new(io::ErrorKind::InvalidData, format!("{s}: object missing from {}", m.store.path.display()))),
            }
        }
    }
}

pub fn read_to_string(path: impl AsRef<std::path::Path>) -> io::Result<String> {
    let bytes = read(path)?;
    String::from_utf8(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "stream did not contain valid UTF-8"))
}

#[derive(Clone, Debug)]
pub struct Metadata {
    pub len: u64,
    pub modified: std::time::SystemTime,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
}

/// Content metadata for either backend. Immutable store identities supply a stable
/// timestamp; edited virtual files use their overlay's filesystem metadata.
pub fn metadata(path: impl AsRef<std::path::Path>) -> io::Result<Metadata> {
    let r = metadata_inner(path.as_ref());
    if r.is_ok() {
        note(b'p', path.as_ref());
    }
    r
}

fn metadata_inner(path: &std::path::Path) -> io::Result<Metadata> {
    let s = path.to_string_lossy();
    if let Some((mount, rel)) = ANY.load(Ordering::Acquire).then(|| find(&s)).flatten() {
        if let Some(edited) = edited_file(&s) { return metadata(edited); }
        let is_dir = mount.is_dir(&rel);
        let entry = mount.file(&rel);
        if !is_dir && entry.is_none() { return Err(not_found(&s)); }
        use std::hash::{Hash, Hasher};
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        mount.commit.hash(&mut hash);
        if let Some(entry) = entry { entry.oid.hash(&mut hash); }
        return Ok(Metadata {
            len: if is_dir { 0 } else { read(path)?.len() as u64 },
            modified: std::time::UNIX_EPOCH + std::time::Duration::from_nanos(hash.finish()),
            is_dir, is_file: entry.is_some_and(|e| e.mode != 0o160000),
            is_symlink: entry.is_some_and(|e| e.mode == 0o120000),
        });
    }
    let link = std::fs::symlink_metadata(path)?;
    let meta = std::fs::metadata(path)?;
    Ok(Metadata { len: meta.len(), modified: meta.modified()?, is_dir: meta.is_dir(),
        is_file: meta.is_file(), is_symlink: link.file_type().is_symlink() })
}

/// Virtual paths normalize lexically and keep their mount identity. A traversal
/// that would leave its mount is rejected instead of falling through to disk.
pub fn canonicalize(path: impl AsRef<std::path::Path>) -> io::Result<std::path::PathBuf> {
    let path = path.as_ref();
    if ANY.load(Ordering::Acquire) {
        let raw = normalize_components(&path.to_string_lossy(), false);
        let mounts = MOUNTS.read().unwrap();
        if let Some(rel) = mounts.iter().find_map(|m| relative(&raw, &m.root)) {
            let mut depth = 0usize;
            for part in rel.split('/') {
                match part {
                    "" | "." => {},
                    ".." if depth == 0 => return Err(io::Error::new(io::ErrorKind::PermissionDenied, "path leaves virtual mount")),
                    ".." => depth -= 1,
                    _ => depth += 1,
                }
            }
            let normalized = normalize_path(&raw);
            drop(mounts);
            if !exists(&normalized) { return Err(not_found(&normalized)); }
            return Ok(normalized.into());
        }
        drop(mounts);
        let normalized = normalize_path(&raw);
        if find(&normalized).is_some() {
            if !exists(&normalized) { return Err(not_found(&normalized)); }
            return Ok(normalized.into());
        }
    }
    std::fs::canonicalize(path)
}

pub fn exists(path: impl AsRef<std::path::Path>) -> bool {
    let found = exists_inner(path.as_ref());
    if found {
        note(b'p', path.as_ref());
    }
    found
}

fn exists_inner(path: &std::path::Path) -> bool {
    if !ANY.load(Ordering::Acquire) {
        return path.exists();
    }
    let s = path.to_string_lossy();
    match find(&s) {
        None => path.exists(),
        Some((m, rel)) => m.file(&rel).is_some() || m.is_dir(&rel) || edit_path(&s).is_some_and(|(p, _)| p.exists()),
    }
}

pub fn is_file(path: impl AsRef<std::path::Path>) -> bool {
    let found = is_file_inner(path.as_ref());
    if found {
        note(b'p', path.as_ref());
    }
    found
}

fn is_file_inner(path: &std::path::Path) -> bool {
    if !ANY.load(Ordering::Acquire) {
        return path.is_file();
    }
    let s = path.to_string_lossy();
    match find(&s) {
        None => path.is_file(),
        Some((m, rel)) => m.file(&rel).is_some_and(|e| e.mode != 0o160000) || edited_file(&s).is_some(),
    }
}

pub fn is_dir(path: impl AsRef<std::path::Path>) -> bool {
    let found = is_dir_inner(path.as_ref());
    if found {
        note(b'p', path.as_ref());
    }
    found
}

fn is_dir_inner(path: &std::path::Path) -> bool {
    if !ANY.load(Ordering::Acquire) {
        return path.is_dir();
    }
    let s = path.to_string_lossy();
    match find(&s) {
        None => path.is_dir(),
        Some((m, rel)) => m.is_dir(&rel) || edit_path(&s).is_some_and(|(p, _)| p.is_dir()),
    }
}

/// A folder's entries: (name, is folder), sorted by name.
pub fn read_dir(path: impl AsRef<std::path::Path>) -> io::Result<Vec<(String, bool)>> {
    let r = read_dir_inner(path.as_ref());
    if r.is_ok() {
        note(b'd', path.as_ref());
    }
    r
}

fn read_dir_inner(path: &std::path::Path) -> io::Result<Vec<(String, bool)>> {
    let disk = || -> io::Result<Vec<(String, bool)>> {
        let mut v: Vec<(String, bool)> = std::fs::read_dir(path)?
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
            .collect();
        v.sort();
        Ok(v)
    };
    if !ANY.load(Ordering::Acquire) {
        return disk();
    }
    match find(&path.to_string_lossy()) {
        None => disk(),
        Some((m, rel)) => {
            let edited: Vec<(String, bool)> = match edit_path(&path.to_string_lossy()) {
                Some((p, _)) => std::fs::read_dir(p)
                    .map(|d| {
                        d.flatten()
                            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
                            .filter(|(n, _)| !n.starts_with(".makepad-run-"))
                            .collect()
                    })
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let mut v = match m.folders().get(&rel) {
                Some(v) => v.clone(),
                None if !edited.is_empty() => Vec::new(),
                None => return Err(not_found(&path.to_string_lossy())),
            };
            for e in edited {
                if !v.iter().any(|x| x.0 == e.0) {
                    v.push(e);
                }
            }
            // repositories mounted directly inside this folder are folders of it too
            let here = normalize_path(&path.to_string_lossy());
            for x in MOUNTS.read().unwrap().iter() {
                if let Some(name) = relative(&x.root, &here) {
                    if !name.is_empty() && !name.contains('/') && !v.iter().any(|e| e.0 == name) {
                        v.push((name.to_string(), true));
                    }
                }
            }
            v.sort();
            Ok(v)
        }
    }
}

/// Directory entry kinds without opening every child for metadata. Keep
/// symlinks distinct so source discovery can retain its no-links policy.
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_file: bool,
    pub is_symlink: bool,
}
pub fn read_dir_entries(path: impl AsRef<std::path::Path>) -> io::Result<Vec<DirEntry>> {
    let path = path.as_ref();
    let disk = |path: &std::path::Path| -> io::Result<Vec<DirEntry>> {
        let mut entries = std::fs::read_dir(path)?.map(|entry| {
            let entry = entry?;
            let kind = entry.file_type()?;
            Ok(DirEntry { name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: kind.is_dir(), is_file: kind.is_file(), is_symlink: kind.is_symlink() })
        }).collect::<io::Result<Vec<_>>>()?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    };
    let name = path.to_string_lossy();
    let Some((mount, rel)) = ANY.load(Ordering::Acquire).then(|| find(&name)).flatten() else { return disk(path) };
    let edited = edit_path(&name).and_then(|(path, _)| disk(&path).ok()).unwrap_or_default();
    // Preserve read_dir's mount/overlay precedence and nested mount listing.
    read_dir(path)?.into_iter().map(|(name, is_dir)| {
        if let Some(entry) = edited.iter().find(|entry| entry.name == name) { return Ok(entry.clone()); }
        let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let entry = mount.file(&child);
        Ok(DirEntry { name, is_dir, is_file: !is_dir && entry.is_some_and(|e| e.mode != 0o160000),
            is_symlink: entry.is_some_and(|e| e.mode == 0o120000) })
    }).collect()
}

/// Resolve a link's authored target without canonicalizing a source file.
pub fn read_link(path: impl AsRef<std::path::Path>) -> io::Result<std::path::PathBuf> {
    let path = path.as_ref();
    let name = path.to_string_lossy();
    if ANY.load(Ordering::Acquire) && find(&name).is_some() {
        if let Some(edited) = edited_file(&name) { return std::fs::read_link(edited); }
        return read_to_string(path).map(std::path::PathBuf::from);
    }
    std::fs::read_link(path)
}

/// What a stat of an on-disk file or folder says (size, mtime and ctime in nanoseconds, inode):
/// a cache that recorded it can take the content as unchanged while a later stat is equal, and
/// reads the content only when it is not (the "racy" rule is the recorder's: it records no stat
/// for a file changed within `STAT_RACY_NS` of the recording).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileStat {
    pub size: u64,
    pub mtime_ns: u64,
    pub ctime_ns: u64,
    pub ino: u64,
}
pub const STAT_RACY_NS: u64 = 2_000_000_000;

/// The stat of an on-disk path (None: virtual (a mounted store or an edits overlay), missing).
pub fn file_stat(path: &str) -> Option<FileStat> {
    if ANY.load(Ordering::Acquire) && find(path).is_some() {
        return None;
    }
    let m = std::fs::metadata(path).ok()?;
    let ns = |t: std::io::Result<std::time::SystemTime>| t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64);
    #[cfg(unix)]
    let (ctime_ns, ino) = {
        use std::os::unix::fs::MetadataExt;
        (m.ctime().max(0) as u64 * 1_000_000_000 + m.ctime_nsec().max(0) as u64, m.ino())
    };
    #[cfg(not(unix))]
    let (ctime_ns, ino) = (0, 0);
    Some(FileStat { size: m.len(), mtime_ns: ns(m.modified())?, ctime_ns, ino })
}

/// When the file last changed (a git file: never, None; the caller treats it as unchanged).
pub fn modified(path: impl AsRef<std::path::Path>) -> Option<std::time::SystemTime> {
    let path = path.as_ref();
    if ANY.load(Ordering::Acquire) && find(&path.to_string_lossy()).is_some() {
        // an edited file changes on disk; the store's never does
        return edited_file(&path.to_string_lossy()).and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    }
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What a virtual path's content is fixed by: its mount's store and commit (None on disk). Caches
/// that would compare file times compare this instead.
pub fn identity(path: &str) -> Option<String> {
    if !ANY.load(Ordering::Acquire) {
        return None;
    }
    let (m, _) = find(path)?;
    if edited_file(path).is_some() {
        return None;
    }
    Some(match m.store.range {
        Some((offset, len)) => format!("{}@{offset}+{len}@{}", m.store.path.display(), store::hex(&m.commit)),
        None => format!("{}@{}", m.store.path.display(), store::hex(&m.commit)),
    })
}

// ---- the mount table as text (a child process started in a virtual folder mounts the same)

/// `root\tstore path\tcommit hex[\toffset+len]` per mount, newline-separated (the range of a store
/// that sits inside a bigger file, gitstore.rs `open_range`).
pub fn mounts_text() -> String {
    let m = MOUNTS.read().unwrap();
    let mut out = String::new();
    for x in m.iter() {
        out.push_str(&format!("{}\t{}\t{}", x.root, x.store.path.display(), store::hex(&x.commit)));
        if let Some((offset, len)) = x.store.range {
            out.push_str(&format!("\t{offset}+{len}"));
        }
        out.push('\n');
    }
    out
}

/// Mounts what `mounts_text` wrote (MAKEPAD_RUN_VFS).
pub fn mount_text(text: &str) -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let mut it = line.split('\t');
        let (Some(root), Some(path), Some(commit)) = (it.next(), it.next(), it.next()) else { return Err(format!("MAKEPAD_RUN_VFS: bad line {line}")) };
        let commit = store::from_hex(commit).ok_or_else(|| format!("MAKEPAD_RUN_VFS: bad commit in {line}"))?;
        let s = match it.next() {
            None => Store::open(std::path::Path::new(path)).map_err(|e| format!("{path}: {e}"))?,
            Some(range) => {
                let (offset, len) = parse_range(range).ok_or_else(|| format!("MAKEPAD_RUN_VFS: bad range in {line}"))?;
                Store::open_range(std::path::Path::new(path), offset, len).map_err(|e| format!("{path}: {e}"))?
            }
        };
        mount(root, Arc::new(s.ok_or_else(|| format!("{path}: no store"))?), &commit)?;
    }
    Ok(())
}

/// `offset+len` (decimal).
pub fn parse_range(s: &str) -> Option<(u64, u64)> {
    let (o, l) = s.split_once('+')?;
    Some((o.parse().ok()?, l.parse().ok()?))
}

/// MAKEPAD_RUN_VFS from the environment, mounted once (the loader calls this at start).
pub fn mount_from_env() -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    record_from_env()?;
    if let Some(d) = std::env::var_os("MAKEPAD_RUN_EDITS").filter(|d| !d.is_empty()) {
        set_edits(Some(d.into()));
    }
    match std::env::var("MAKEPAD_RUN_VFS") {
        Ok(t) if !t.is_empty() => mount_text(&t),
        _ => Ok(()),
    }
}

// ---- what a program running on Relax's std calls (std/fs.rs, std/process.rs; C ABI, found by
// their names like the loader's other symbols, jit.rs loader_symbol)

static HELD: relax_session::sync::Mutex<Vec<(u64, Vec<u8>)>> = relax_session::sync::Mutex::new(Vec::new());
static HELD_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

unsafe fn path_arg<'a>(p: *const u8, n: usize) -> &'a str {
    std::str::from_utf8(std::slice::from_raw_parts(p, n)).unwrap_or("")
}

fn hold(data: Vec<u8>, out: *mut *const u8, out_len: *mut usize) -> u64 {
    let id = HELD_SEQ.fetch_add(1, Ordering::Relaxed);
    let _k = crate::zone_alloc::kept();
    let mut h = HELD.lock().unwrap();
    h.push((id, data));
    let d = &h.last().unwrap().1;
    unsafe {
        *out = d.as_ptr();
        *out_len = d.len();
    }
    id
}

/// 0: not a virtual path (the disk), 1: a file (`*size` its length), 2: a folder, -1: virtual but
/// missing.
pub extern "C" fn relax_vfs_stat(p: *const u8, n: usize, size: *mut u64) -> i32 {
    if !ANY.load(Ordering::Acquire) {
        return 0;
    }
    let path = unsafe { path_arg(p, n) };
    let Some((m, rel)) = find(path) else { return 0 };
    if let Some(f) = edited_file(path) {
        unsafe { *size = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0) };
        return 1;
    }
    if let Some(e) = m.file(&rel) {
        if e.mode == 0o160000 {
            return -1;
        }
        let len = m.store.object_size(&e.oid).ok().flatten().unwrap_or(0);
        unsafe { *size = len };
        return 1;
    }
    if m.is_dir(&rel) || edit_path(path).is_some_and(|(p, _)| p.is_dir()) { 2 } else { -1 }
}

/// A virtual file's bytes, held until `relax_vfs_release`; 0 when it cannot be read.
pub extern "C" fn relax_vfs_read(p: *const u8, n: usize, out: *mut *const u8, out_len: *mut usize) -> u64 {
    let path = unsafe { path_arg(p, n) };
    match read(path) {
        Ok(d) => hold(d, out, out_len),
        Err(_) => 0,
    }
}

/// A virtual folder's entries as lines `d <name>` / `f <name>`, held until `relax_vfs_release`.
pub extern "C" fn relax_vfs_list(p: *const u8, n: usize, out: *mut *const u8, out_len: *mut usize) -> u64 {
    let path = unsafe { path_arg(p, n) };
    match read_dir(path) {
        Ok(v) => hold(v.iter().map(|(name, d)| format!("{} {name}\n", if *d { 'd' } else { 'f' })).collect::<String>().into_bytes(), out, out_len),
        Err(_) => 0,
    }
}

/// The mount table (`mounts_text`), held until `relax_vfs_release`.
pub extern "C" fn relax_vfs_mounts(out: *mut *const u8, out_len: *mut usize) -> u64 {
    hold(mounts_text().into_bytes(), out, out_len)
}

/// std::relax::store_file: one file of a repository's store at a commit (`url`, 40-hex `commit`,
/// `path` in the repository), held until `relax_vfs_release`; 0 when it is not there.
pub extern "C" fn relax_store_file(u: *const u8, un: usize, c: *const u8, cn: usize, p: *const u8, pn: usize, out: *mut *const u8, out_len: *mut usize) -> u64 {
    let (url, commit, path) = unsafe { (path_arg(u, un), path_arg(c, cn), path_arg(p, pn)) };
    let Some(oid) = store::from_hex(commit) else { return 0 };
    match store::store_file(url, &oid, path) {
        Some(d) => hold(d, out, out_len),
        None => 0,
    }
}

pub extern "C" fn relax_vfs_release(id: u64) {
    HELD.lock().unwrap().retain(|(i, _)| *i != id);
}


// ---- edits (MAKEPAD_RUN_EDITS): writes into virtual folders

/// The edits folder's records: edited path -> the store blob it was edited from (None: a file
/// the store did not have).
fn load_records(dir: &std::path::Path) -> BTreeMap<String, Option<Oid>> {
    let mut out = BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(dir.join(".makepad-run-edits")) {
        for line in t.lines() {
            if let Some((rel, oid)) = line.split_once('\t') {
                out.insert(rel.to_string(), if oid == "new" { None } else { store::from_hex(oid) });
            }
        }
    }
    out
}

fn save_records(dir: &std::path::Path, r: &BTreeMap<String, Option<Oid>>) -> io::Result<()> {
    let text: String = r.iter().map(|(rel, oid)| format!("{}\t{}\n", rel, oid.as_ref().map(store::hex).unwrap_or_else(|| "new".into()))).collect();
    let tmp = dir.join(".makepad-run-edits.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, dir.join(".makepad-run-edits"))
}

static EDIT_LOCK: relax_session::sync::Mutex<()> = relax_session::sync::Mutex::new(());

/// The files `seed_edits` wrote: path -> the store blob written.
fn load_seeded(dir: &std::path::Path) -> BTreeMap<String, Oid> {
    let mut out = BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(dir.join(".makepad-run-seeded")) {
        for line in t.lines() {
            if let Some((rel, oid)) = line.split_once('\t').and_then(|(r, o)| Some((r, store::from_hex(o)?))) {
                out.insert(rel.to_string(), oid);
            }
        }
    }
    out
}

fn save_seeded(dir: &std::path::Path, s: &BTreeMap<String, Oid>) -> io::Result<()> {
    let text: String = s.iter().map(|(rel, oid)| format!("{}\t{}\n", rel, store::hex(oid))).collect();
    let tmp = dir.join(".makepad-run-seeded.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, dir.join(".makepad-run-seeded"))
}

/// git's blob id of `data`.
fn blob_oid(data: &[u8]) -> Oid {
    let mut bytes = format!("blob {}\0", data.len()).into_bytes();
    bytes.extend_from_slice(data);
    store::sha1(&bytes)
}

/// Seeding writes the store's source files under these folders (relative to the outermost mount)
/// into the edits folder where it has none; returns how many it wrote. Left out: `resources/`
/// folders and files over 1 MB (assets: read from the store as before), submodules, symlinks.
pub fn seed_edits(folders: &[String]) -> io::Result<usize> {
    let Some(dir) = EDITS.read().unwrap().clone() else { return Ok(0) };
    let Some(outer) = MOUNTS.read().unwrap().iter().map(|x| x.root.clone()).min_by_key(|r| r.len()) else { return Ok(0) };
    let _g = EDIT_LOCK.lock().unwrap();
    let mut seeded = load_seeded(&dir);
    let records = load_records(&dir);
    let mut wrote = 0;
    for folder in folders {
        let folder = folder.trim_matches('/');
        if folder.is_empty() || folder.split('/').any(|c| c == ".." || c == ".") || folder.starts_with(".makepad-run-") {
            continue;
        }
        let vdir = format!("{outer}/{folder}");
        let Some((m, mrel)) = find(&vdir) else { continue };
        let prefix = if mrel.is_empty() { String::new() } else { format!("{mrel}/") };
        // the mount's own folder relative to the outermost mount
        let mount_rel = relative(&m.root, &outer).unwrap_or("").to_string();
        for e in m.files.iter().filter(|e| e.path.starts_with(&prefix) && (e.mode == 0o100644 || e.mode == 0o100755)) {
            let rel = if mount_rel.is_empty() { e.path.clone() } else { format!("{mount_rel}/{}", e.path) };
            if e.path.split('/').any(|c| c == "resources") || m.store.object_size(&e.oid)?.is_none_or(|n| n > 1 << 20) {
                continue;
            }
            let disk = dir.join(&rel);
            if disk.exists() || records.contains_key(&rel) {
                continue;
            }
            // a file of a nested mount inside this folder is that mount's, not this one's
            if find(&format!("{outer}/{rel}")).is_none_or(|(n, _)| !Arc::ptr_eq(&n, &m)) {
                continue;
            }
            let Some((_, data)) = m.store.object(&e.oid)? else { continue };
            if let Some(p) = disk.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&disk, &data)?;
            seeded.insert(rel, e.oid);
            wrote += 1;
        }
    }
    if wrote > 0 {
        save_seeded(&dir, &seeded)?;
    }
    Ok(wrote)
}

/// What a write into a virtual path needs: the disk path to write (in the edits folder), its
/// folder made, and on the first edit of a store file its record and base copy. `keep`: the
/// write keeps the content (append, read-write): the store's bytes are copied first. Err when
/// the path is not virtual or there is no edits folder (the file stays read-only).
pub fn prepare_edit(path: &str, keep: bool) -> io::Result<std::path::PathBuf> {
    let (disk, rel) = edit_path(path).ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "read-only (no MAKEPAD_RUN_EDITS)"))?;
    if rel.is_empty() || rel.starts_with(".makepad-run-") {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not an editable path"));
    }
    let _g = EDIT_LOCK.lock().unwrap();
    if disk.is_file() {
        return Ok(disk);
    }
    let dir = EDITS.read().unwrap().clone().unwrap();
    if let Some(parent) = disk.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let (m, mrel) = find(path).ok_or_else(|| not_found(path))?;
    let entry = m.file(&mrel).filter(|e| e.mode != 0o160000).cloned();
    let mut records = load_records(&dir);
    match &entry {
        Some(e) => {
            let data = match m.store.object(&e.oid)? {
                Some((_, d)) => d,
                None => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{path}: object missing"))),
            };
            let base = dir.join(".makepad-run-base").join(&rel);
            if let Some(bp) = base.parent() {
                std::fs::create_dir_all(bp)?;
            }
            std::fs::write(&base, &data)?;
            if keep {
                std::fs::write(&disk, &data)?;
            }
            records.insert(rel.clone(), Some(e.oid));
        }
        None => {
            records.entry(rel.clone()).or_insert(None);
        }
    }
    save_records(&dir, &records)?;
    Ok(disk)
}

/// The edits ledger's stamp (modified time, length); None without an edits folder or ledger.
/// Every edit of a mounted store's file rewrites the ledger (`save_records`): a watcher whose
/// last look saw this stamp knows no virtual file changed since, without a stat per file.
pub fn edits_ledger_stamp() -> Option<(std::time::SystemTime, u64)> {
    let dir = EDITS.read().unwrap().clone()?;
    let m = std::fs::metadata(dir.join(".makepad-run-edits")).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The disk path of `path` in the edits folder when it is an edited file or folder there
/// (removing or renaming an edited file).
pub fn edited_disk_path(path: &str) -> Option<std::path::PathBuf> {
    let (p, rel) = edit_path(path)?;
    (!rel.starts_with(".makepad-run-") && p.exists()).then_some(p)
}

/// A folder made in a virtual folder: made in the edits folder.
pub fn prepare_edit_dir(path: &str) -> io::Result<std::path::PathBuf> {
    let (disk, rel) = edit_path(path).ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "read-only (no MAKEPAD_RUN_EDITS)"))?;
    if rel.starts_with(".makepad-run-") {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not an editable path"));
    }
    Ok(disk)
}

/// At a run's start: the edits against the release being run. An edit whose store file is
/// unchanged carries over; a file the release changed too (or now has, for a new file) moves
/// to `.makepad-run-conflicts/<commit>/` unless the edit equals the release's file (then it is done).
/// Returns one line per conflict (for the run's report); nothing when there are no edits.
pub fn reconcile_edits() -> Vec<String> {
    let Some(dir) = EDITS.read().unwrap().clone() else { return Vec::new() };
    reconcile_seeded(&dir);
    if !dir.join(".makepad-run-edits").is_file() {
        return Vec::new();
    }
    let _g = EDIT_LOCK.lock().unwrap();
    let mut records = load_records(&dir);
    let mut out = Vec::new();
    let mut changed = false;
    let roots: Vec<String> = {
        let m = MOUNTS.read().unwrap();
        let outer = m.iter().map(|x| x.root.clone()).min_by_key(|r| r.len());
        outer.into_iter().collect()
    };
    let Some(outer) = roots.first().cloned() else { return Vec::new() };
    for (rel, base) in records.clone() {
        let disk = dir.join(&rel);
        if !disk.is_file() {
            records.remove(&rel);
            changed = true;
            continue;
        }
        let vpath = format!("{outer}/{rel}");
        let Some((m, mrel)) = find(&vpath) else { continue };
        let now = m.file(&mrel).filter(|e| e.mode != 0o160000).map(|e| e.oid);
        if now == base {
            continue;
        }
        // the release changed this file (or added it)
        let theirs = match now {
            Some(oid) => m.store.object(&oid).ok().flatten().map(|(_, d)| d),
            None => None,
        };
        let mine = std::fs::read(&disk).unwrap_or_default();
        if theirs.as_deref() == Some(mine.as_slice()) {
            let _ = std::fs::remove_file(&disk);
            records.remove(&rel);
            changed = true;
            continue;
        }
        if now.is_none() && base.is_none() {
            continue;
        }
        let tag = &store::hex(&m.commit)[..12];
        let keep = dir.join(".makepad-run-conflicts").join(tag).join(&rel);
        if let Some(p) = keep.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        if std::fs::rename(&disk, &keep).is_ok() {
            let base_copy = dir.join(".makepad-run-base").join(&rel);
            if base_copy.is_file() {
                let mut b = keep.as_os_str().to_os_string();
                b.push(".base");
                let _ = std::fs::copy(&base_copy, std::path::PathBuf::from(b));
            }
            records.remove(&rel);
            changed = true;
            out.push(format!("{rel}: changed in this release too; your edit is kept in {}", keep.display()));
        }
    }
    if changed {
        let _ = save_records(&dir, &records);
    }
    out
}

/// The seeded files against the release being run: one still holding its seeded blob is brought
/// to the release's file (removed when the release has none); one whose bytes changed becomes an
/// edit from its seeded blob (then reconciled as every edit); one removed is forgotten.
fn reconcile_seeded(dir: &std::path::Path) {
    let mut seeded = load_seeded(dir);
    if seeded.is_empty() {
        return;
    }
    let Some(outer) = MOUNTS.read().unwrap().iter().map(|x| x.root.clone()).min_by_key(|r| r.len()) else { return };
    let _g = EDIT_LOCK.lock().unwrap();
    let mut records = load_records(dir);
    let mut records_changed = false;
    for (rel, oid) in seeded.clone() {
        let disk = dir.join(&rel);
        let Ok(mine) = std::fs::read(&disk) else {
            seeded.remove(&rel);
            continue;
        };
        if blob_oid(&mine) != oid {
            seeded.remove(&rel);
            records.entry(rel).or_insert(Some(oid));
            records_changed = true;
            continue;
        }
        let now = find(&format!("{outer}/{rel}")).and_then(|(m, mrel)| m.file(&mrel).filter(|e| e.mode != 0o160000).map(|e| (m.clone(), e.oid)));
        match now {
            Some((_, n)) if n == oid => {}
            Some((m, n)) => match m.store.object(&n) {
                Ok(Some((_, data))) if std::fs::write(&disk, &data).is_ok() => {
                    seeded.insert(rel, n);
                }
                _ => {}
            },
            None => {
                let _ = std::fs::remove_file(&disk);
                seeded.remove(&rel);
            }
        }
    }
    let _ = save_seeded(dir, &seeded);
    if records_changed {
        let _ = save_records(dir, &records);
    }
}

/// std's write into a virtual path (fs_vfs.rs): mode 0 the edited file or folder if any, 1 a
/// write keeping the content, 2 a write replacing it, 3 a folder. The disk path, held until
/// `relax_vfs_release`; 0 when the path cannot be written.
pub extern "C" fn relax_vfs_edit(p: *const u8, n: usize, mode: u32, out: *mut *const u8, out_len: *mut usize) -> u64 {
    let path = unsafe { path_arg(p, n) };
    let r = match mode {
        0 => edited_disk_path(path).ok_or_else(|| not_found(path)),
        1 => prepare_edit(path, true),
        2 => prepare_edit(path, false),
        _ => prepare_edit_dir(path),
    };
    match r {
        Ok(d) => hold(d.to_string_lossy().into_owned().into_bytes(), out, out_len),
        Err(_) => 0,
    }
}

#[cfg(test)]
#[path = "../tests/support/vfs_store.rs"]
mod test_store;

#[cfg(test)]
mod tests {
    use super::*;

    /// The mounts' outermost root and the edits folder are process state: the tests that set
    /// them run one at a time.
    static EDITS_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn seeded_files_follow_the_release_until_edited() {
        let _one = EDITS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let v1 = test_store::Fixture::new(&[("app/a.rs", b"a1"), ("app/b.rs", b"b1"), ("app/gone.rs", b"g1"), ("app/src/c.rs", b"c1"), ("other/x.rs", b"x1")]);
        let v2 = test_store::Fixture::new(&[("app/a.rs", b"a2"), ("app/b.rs", b"b2"), ("app/src/c.rs", b"c1"), ("app/new.rs", b"n2"), ("other/x.rs", b"x2")]);
        let root = "/seed-test/vfs/m";
        let edits = v1.store.path.with_extension("seed-edits");
        mount(root, v1.store.clone(), &v1.commit).unwrap();
        set_edits(Some(edits.clone()));
        assert_eq!(reconcile_edits(), Vec::<String>::new());
        assert_eq!(seed_edits(&["app".to_string()]).unwrap(), 4);
        // again: nothing to write
        assert_eq!(seed_edits(&["app".to_string()]).unwrap(), 0);
        assert_eq!(std::fs::read(edits.join("app/src/c.rs")).unwrap(), b"c1");
        assert!(!edits.join("other/x.rs").exists());
        assert!(load_records(&edits).is_empty(), "a seeded file is not an edit");
        // a tool edits b.rs in place
        std::fs::write(edits.join("app/b.rs"), b"b-mine").unwrap();
        // the next release
        mount(root, v2.store.clone(), &v2.commit).unwrap();
        let conflicts = reconcile_edits();
        // new.rs, and b.rs again (its edit is kept aside below)
        assert_eq!(seed_edits(&["app".to_string()]).unwrap(), 2);
        assert_eq!(std::fs::read(edits.join("app/b.rs")).unwrap(), b"b2");
        // untouched seeded files follow the release; one the release dropped goes
        assert_eq!(std::fs::read(edits.join("app/a.rs")).unwrap(), b"a2");
        assert!(!edits.join("app/gone.rs").exists());
        assert_eq!(std::fs::read(edits.join("app/new.rs")).unwrap(), b"n2");
        assert_eq!(read(format!("{root}/app/a.rs")).unwrap(), b"a2");
        // the edited one became an edit from its seeded blob: the release changed it too, so it
        // is kept aside and reported
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert!(conflicts[0].starts_with("app/b.rs"));
        assert_eq!(load_seeded(&edits).len(), 4);
        set_edits(None);
        MOUNTS.write().unwrap().retain(|m| m.root != normalize_path(root));
        let _ = std::fs::remove_dir_all(&edits);
    }

    #[test]
    fn disk_paths_are_untouched_and_mounts_resolve() {
        assert_eq!(normalize_path("/a/./b/../c/"), "/a/c");
        assert_eq!(normalize_path("a\\b"), "a/b");
        // nothing mounted: the disk, as std::fs
        let me = file!();
        let _ = me;
        assert!(exists(std::env::temp_dir()));
        assert!(!is_virtual("/nowhere/x"));
    }

    #[test]
    fn path_components_preserve_roots_case_and_boundaries() {
        for (path, expected) in [
            (r"C:\Users\x\vfs\.\m\..\n\", "c:/Users/x/vfs/n"),
            (r"\\?\C:\Users\x\vfs", "c:/Users/x/vfs"),
            (r"\\server\share\vfs", "//server/share/vfs"),
            (r"\\?\UNC\server\share\vfs", "//server/share/vfs"),
            ("//?/unc/server/share/vfs", "//server/share/vfs"),
            (r"C:\..\", "c:/"),
            (r"\\server\share\..\..\file", "//server/share/file"),
            ("C:relative", "c:relative"),
            ("../a/../../b", "../../b"),
            ("/../../a", "/a"),
            ("/", "/"),
        ] {
            assert_eq!(normalize_path(path), expected, "{path}");
            assert_eq!(normalize_path(expected), expected, "idempotent: {path}");
        }
        assert_eq!(relative_path(r"\\?\C:\Users\x\vfs\m\file", "c:/Users/x/vfs/m"), Some("file".into()));
        assert_eq!(relative_path("c:/Users/x/vfs/more/file", r"C:\Users\x\vfs\m"), None);
        assert_eq!(relative_path("c:/users/x/file", "C:/Users/x"), None);
        assert_eq!(relative_path("c:/file", "C:/"), Some("file".into()));
        assert_eq!(relative_path("C:relative", "c:/"), None);
        assert_eq!(relative_path("c:/file", "C:"), None);
        assert_eq!(relative_path("//server/share/file", "/"), None);
        assert_eq!(relative_path("/file", ""), None);
        assert_eq!(relative_path("/a:other", "/a:"), None);
        assert_eq!(relative_path("//server/share/file", "/server/share"), None);
    }

    #[test]
    fn windows_spellings_share_mounts_and_edits() {
        let _one = EDITS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let file = "examples/counter/src/counter.rs";
        let bytes = b"pub fn value() -> u32 { 7 }\n";
        let fixture = test_store::Fixture::new(&[(file, bytes)]);
        let roots = [
            r"C:\Users\x\vfs\m+c@apps_commercial",
            "c:/Users/x/vfs/m+c@apps_commercial",
            r"\\?\C:\Users\x\vfs\m+c@apps_commercial",
            "//?/c:/Users//x/./vfs/m+c@apps_commercial/",
        ];
        let unc = [
            r"\\server\share\vfs\m+c@apps_commercial",
            "//server/share/vfs/m+c@apps_commercial",
            r"\\?\UNC\server\share\vfs\m+c@apps_commercial",
        ];
        // No source tree exists on disk; the only real file is the store.
        for root in roots.iter().chain(&unc) { assert!(!std::path::Path::new(root).exists()); }
        mount(roots[0], fixture.store.clone(), &fixture.commit).unwrap();
        mount(unc[2], fixture.store.clone(), &fixture.commit).unwrap();
        let identity = identity(&format!("{}/{file}", roots[0])).unwrap();
        for root in roots.iter().chain(&unc) {
            let path = format!("{root}\\examples\\counter\\src\\counter.rs");
            let dir = format!("{root}/examples/counter/src");
            assert!(is_virtual(&path), "{path}");
            assert_eq!(read(&path).unwrap(), bytes, "{path}");
            assert_eq!(read_to_string(&path).unwrap().as_bytes(), bytes);
            let meta = metadata(&path).unwrap();
            assert!(meta.is_file && !meta.is_dir && !meta.is_symlink);
            assert_eq!(meta.len, bytes.len() as u64);
            assert!(metadata(&dir).unwrap().is_dir);
            assert!(exists(&path) && is_file(&path) && !is_dir(&path));
            assert!(exists(root) && is_dir(root) && !is_file(root));
            assert_eq!(read_dir(&dir).unwrap(), vec![("counter.rs".into(), false)]);
            let canonical = canonicalize(format!("{dir}/../src/./counter.rs")).unwrap();
            assert_eq!(canonical.to_string_lossy(), normalize_path(&path));
            assert_eq!(read(&canonical).unwrap(), bytes);
            assert_eq!(super::identity(&path).as_ref(), Some(&identity));
            assert_eq!(modified(&path), None);
            let missing = format!("{dir}/missing.rs");
            assert!(!exists(&missing) && !is_file(&missing) && !is_dir(&missing));
            assert_eq!(read(&missing).unwrap_err().kind(), io::ErrorKind::NotFound);
            assert_eq!(metadata(&missing).unwrap_err().kind(), io::ErrorKind::NotFound);
            assert_eq!(canonicalize(&missing).unwrap_err().kind(), io::ErrorKind::NotFound);
            assert_eq!(canonicalize(format!("{root}/../outside")).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
            let mut len = 0;
            assert_eq!(relax_vfs_stat(path.as_ptr(), path.len(), &mut len), 1);
            assert_eq!(len, bytes.len() as u64);
            assert_eq!(relax_vfs_stat(missing.as_ptr(), missing.len(), &mut len), -1);
            for (path, list) in [(&path, false), (&dir, true)] {
                let (mut data, mut len) = (std::ptr::null(), 0);
                let id = if list { relax_vfs_list(path.as_ptr(), path.len(), &mut data, &mut len) }
                    else { relax_vfs_read(path.as_ptr(), path.len(), &mut data, &mut len) };
                assert_ne!(id, 0);
                assert_eq!(unsafe { std::slice::from_raw_parts(data, len) }, if list { &b"f counter.rs\n"[..] } else { bytes });
                relax_vfs_release(id);
            }
        }
        assert!(!is_virtual("c:/Users/x/vfs/m+c@apps_commercial-extra/file"));
        assert!(!is_virtual("c:/users/x/vfs/m+c@apps_commercial/file"));
        assert!(!is_virtual("/server/share/vfs/m+c@apps_commercial/file"));
        // Remounting an alternate spelling replaces the same root, and the
        // serialized table round-trips through the child-process entry point.
        mount(roots[2], fixture.store.clone(), &fixture.commit).unwrap();
        assert_eq!(MOUNTS.read().unwrap().iter().filter(|m| m.root == normalize_path(roots[0])).count(), 1);
        let text: String = mounts_text().lines().filter(|l| l.starts_with(&normalize_path(roots[0])) || l.starts_with(&normalize_path(unc[0])))
            .map(|l| format!("{l}\n")).collect();
        mount_text(&text).unwrap();
        assert_eq!(read(format!("{}/{file}", roots[1])).unwrap(), bytes);

        // A nested mount wins even when its spelling differs from the outer one.
        let nested = test_store::Fixture::new(&[("nested.rs", b"nested")]);
        mount(&format!("{}/examples/counter/src/child", roots[2]), nested.store.clone(), &nested.commit).unwrap();
        assert_eq!(read(format!("{}/examples/counter/src/child/nested.rs", roots[1])).unwrap(), b"nested");
        assert!(read_dir(format!("{}/examples/counter/src", roots[0])).unwrap().contains(&("child".into(), true)));
        assert_eq!(canonicalize(format!("{}/examples/counter/src/child/../counter.rs", roots[1])).unwrap_err().kind(), io::ErrorKind::PermissionDenied);

        let edits = fixture.store.path.with_extension("edits");
        set_edits(Some(edits.clone()));
        let path = format!("{}/{file}", roots[2]);
        let disk = prepare_edit(&path, true).unwrap();
        assert_eq!(std::fs::read(&disk).unwrap(), bytes);
        std::fs::write(&disk, b"edited").unwrap();
        for root in roots {
            let path = format!("{root}/{file}");
            assert_eq!(read(&path).unwrap(), b"edited");
            assert_eq!(metadata(&path).unwrap().len, 6);
            assert_eq!(edited_disk_path(&path), Some(disk.clone()));
            assert!(super::identity(&path).is_none());
            assert!(modified(&path).is_some());
            assert_eq!(prepare_edit_dir(&format!("{root}/new-dir")).unwrap(), edits.join("new-dir"));
        }
        // Frozen std subtrees and their nested-mount check use the same keys.
        let digest = crate::std_digest::digest(vec![("counter.rs".into(), bytes.to_vec())]);
        assert_eq!(verify_std(&format!("{}/examples/counter/src", roots[0]), &digest).unwrap_err(), "std contains a nested mount");
        MOUNTS.write().unwrap().retain(|m| m.commit != nested.commit);
        verify_std(&format!("{}/examples/counter/src", roots[0]), &digest).unwrap();
        for root in roots {
            assert_eq!(read(format!("{root}/{file}")).unwrap(), bytes);
            assert_eq!(prepare_edit(&format!("{root}/{file}"), true).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        }
        set_edits(None);
        FROZEN.write().unwrap().retain(|p| relative(p, &normalize_path(roots[0])).is_none());
        MOUNTS.write().unwrap().retain(|m| m.commit != fixture.commit);
    }
}
