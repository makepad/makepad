//! Every file the front end reads goes through here (`makepad run <git url>`: gitstore.rs). Two backends
//! behind one interface: the plain filesystem (today's std::fs calls; with nothing mounted it is
//! exactly that, one relaxed load in front) and git mounts: a virtual folder (never created on
//! disk) whose files come from a repository store (store.rs) at a commit. A path under a mount is
//! served from the mount (the longest mount root wins: a repository mounted inside another);
//! every other path is the disk. Paths stay plain strings everywhere else.
//!
//! EDITS (RELAX_EDITS=<folder>): the files a program writes into its own virtual folders (an app's
//! EDIT mode changing an app's sources) live in that disk folder, at the same path relative to
//! the outermost mount, on top of the store: a file there is read instead of the store's. Only
//! edited files exist there. `.makepad-run-edits` in it records, per edited file, the store blob it
//! was edited from (`new` for a file the store lacks), and `.makepad-run-base/<path>` keeps that blob.
//! `.makepad-run-deleted` lists whiteouts, one relative path per line: those store files are absent
//! through every VFS read and probe. Their base blobs are recorded in `.makepad-run-edits` too.
//! A whiteout carries over only while the store still holds its base blob; otherwise reconciliation
//! removes it and reports the change.
//! At a run's start (`reconcile_edits`) edits whose store file is unchanged carry over to the
//! release being run; when the release changed the file too, the edit moves to
//! `.makepad-run-conflicts/<commit>/<path>` (beside it `<path>.base`) and the run reports it -- an edit
//! is never dropped silently. Without RELAX_EDITS virtual files stay read-only.
//!
//! SEED (RELAX_EDITS_SEED=<folder>[,<folder>...], relative to the outermost mount): at a run's
//! start the store's files under those folders are written into the edits folder where it has none
//! (`seed_edits`), so a tool outside the run (a coding agent) finds an app's whole source on disk and
//! edits it in place. `.makepad-run-seeded` records each written file's blob: a seeded file whose
//! bytes still are that blob is not an edit; `reconcile_edits` brings it to the release being run.
//! One whose bytes changed becomes an edit (recorded in `.makepad-run-edits` from that blob).
//!
//! The `.makepad-run-*` names stay as they are, though relax's other names say `relax`: these
//! files hold people's edits across runs (an installed app's edits folder), and a rename would
//! need a migration of every existing edits folder for a cosmetic gain.
//!
//! FOLDER MOUNTS (`mount_folder`): a virtual folder served from a disk folder, the plain map
//! `<root>/<rel>` -> `<folder>/<rel>`: no store, ledger, whiteouts, base copies or seeding; reads,
//! probes and writes go to the mapped disk path. The edits folder serves store mounts only.
//! `in_store` tells a store mount's path (immutable, edited through the ledger) from a folder
//! mount's; `is_virtual` keeps meaning "not where another program finds it" for both, and
//! `on_disk` / `virtual_of` map between a folder mount's two spellings.
//!
//! SHARE (RELAX_SHARE=<name>, `share_from_env`): the workspace of the current folder (cargo's root
//! for it) is folder-mounted at `<relax home>/vfs/share/<name>` and the run's workspace is its place
//! there. Every path a cache key or a stored record holds is then the virtual one and the run dir is
//! the relax home's (dirs.rs): checkouts of one repository at different places (git worktrees,
//! clones) that give the same name share their stores, and a crate compiles again only where its text
//! differs. What the run prints names the checkout's files (`shown`, the crates' print macros); the
//! JIT-run program's own output (panic locations, `file!()`) names the virtual paths, which keep its
//! code stores shared. A current folder in no cargo workspace has none to share and mounts nothing.
use crate::gitstore::{self as store, Entry, Oid, Store};
use std::collections::{BTreeMap, BTreeSet};
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

// ---- the read log (RELAX_READ_LOG=<file>): what `pack-src` records. Every process of a run (the
// build-script children inherit the variable) appends `<kind>\t<path>` once per path and kind:
// `f` a file whose bytes were read, `p` a path a probe (metadata, exists, is_file, is_dir) found,
// `d` a folder that was listed. One relaxed load when off.
static RECORDING: AtomicBool = AtomicBool::new(false);
static RECORDED: relax_session::sync::Mutex<Option<(std::collections::HashSet<(u8, String)>, std::fs::File)>> = relax_session::sync::Mutex::new(None);

/// Starts the read log from RELAX_READ_LOG (the loader calls this at start, next to RELAX_VFS).
pub fn record_from_env() -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    let Some(p) = std::env::var_os("RELAX_READ_LOG").filter(|p| !p.is_empty()) else { return Ok(()) };
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| format!("RELAX_READ_LOG {}: {e}", p.to_string_lossy()))?;
    *RECORDED.lock().unwrap() = Some((Default::default(), f));
    RECORDING.store(true, Ordering::Release);
    Ok(())
}

/// Whether this process records its reads (RELAX_READ_LOG): reads made only for the log
/// (provided::read_identity_files) are skipped when it does not.
pub fn recording() -> bool {
    RECORDING.load(Ordering::Acquire)
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
/// Folder mounts (`mount_folder`): (root, disk folder), longest root first.
static FOLDER_MOUNTS: RwLock<Vec<(String, std::path::PathBuf)>> = RwLock::new(Vec::new());
static ANY_FOLDER: AtomicBool = AtomicBool::new(false);
/// RELAX_EDITS: the disk folder holding the edited files (None: virtual files are read-only)
static EDITS: RwLock<Option<std::path::PathBuf>> = RwLock::new(None);
// A verified binary std always reads the store bytes that were compared, even with app edits.
static FROZEN: RwLock<Vec<String>> = RwLock::new(Vec::new());

/// Verify a complete subtree directly from the store (no disk/edits reads), then protect it
/// from the app's edit overlay. Paths and bytes use the same digest as the compiler build.
pub fn verify_std(path: &str, expected: &str) -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    let (m, rel) = find(path).ok_or("no Relax std mount")?;
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
        return Err("mounted commit has no Relax std".into());
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

/// Serves `root` (a folder that does not exist on disk) from the disk folder `folder`: `<root>/<rel>`
/// is `<folder>/<rel>`.
pub fn mount_folder(root: &str, folder: &std::path::Path) {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    let root = normalize_path(root);
    let mut m = FOLDER_MOUNTS.write().unwrap();
    m.retain(|x| x.0 != root);
    m.push((root, folder.to_path_buf()));
    m.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    ANY_FOLDER.store(true, Ordering::Release);
    ANY.store(true, Ordering::Release);
}

/// The folder mount whose root is the innermost mount containing `p` (normalized): (root, disk
/// folder, `p` relative to the root). A store mount inside it wins for its own folder.
fn folder_mount(p: &str) -> Option<(String, std::path::PathBuf, String)> {
    if !ANY_FOLDER.load(Ordering::Acquire) {
        return None;
    }
    let folders = FOLDER_MOUNTS.read().unwrap();
    let (root, dir, rel) = folders.iter().find_map(|(r, d)| relative(p, r).map(|rel| (r, d, rel)))?;
    if MOUNTS.read().unwrap().iter().any(|m| m.root.len() > root.len() && relative(p, &m.root).is_some()) {
        return None;
    }
    Some((root.clone(), dir.clone(), rel.to_string()))
}

/// The disk path of `path` when it lies under a folder mount.
fn folder_path(path: &str) -> Option<std::path::PathBuf> {
    let (_, dir, rel) = folder_mount(&normalize_path(path))?;
    Some(if rel.is_empty() { dir } else { dir.join(rel) })
}

/// Whether `path` lies under a mount, of a store or a folder: it is on no disk under this name
/// (callers that hand paths to other programs check this, and map a folder mount's with `on_disk`).
pub fn is_virtual(path: &str) -> bool {
    ANY.load(Ordering::Acquire) && (find(path).is_some() || folder_path(path).is_some())
}

/// Whether `path` lies under a folder mount (a shared workspace's checkout, SHARE).
pub fn in_folder(path: &str) -> bool {
    folder_path(path).is_some()
}

/// Whether `path` lies under a store mount: immutable, changed only by an edit through the edits
/// ledger (a folder mount's path is a disk file under another name).
pub fn in_store(path: &str) -> bool {
    ANY.load(Ordering::Acquire) && find(path).is_some()
}

/// `path` as another program finds it: a folder mount's file or folder is the disk one; any other
/// path is itself (a store mount's is on no disk).
pub fn on_disk(path: &str) -> std::borrow::Cow<'_, str> {
    match folder_path(path) {
        Some(p) => std::borrow::Cow::Owned(normalize_path(&p.to_string_lossy())),
        None => std::borrow::Cow::Borrowed(path),
    }
}

/// The inverse of [`on_disk`]: a disk path inside a folder mount's folder as the mount's virtual
/// path (a build script's declared input, given under its current folder on disk); any other path
/// is itself.
pub fn virtual_of(path: &str) -> std::borrow::Cow<'_, str> {
    if !ANY_FOLDER.load(Ordering::Acquire) {
        return std::borrow::Cow::Borrowed(path);
    }
    let p = normalize_path(path);
    for (root, dir) in FOLDER_MOUNTS.read().unwrap().iter() {
        if let Some(rel) = relative(&p, &normalize_path(&dir.to_string_lossy())) {
            return std::borrow::Cow::Owned(if rel.is_empty() { root.clone() } else { format!("{root}/{rel}") });
        }
    }
    std::borrow::Cow::Borrowed(path)
}

/// Whether a folder mount is in place (a shared workspace's run, SHARE).
pub fn folder_mounted() -> bool {
    ANY_FOLDER.load(Ordering::Acquire)
}

/// The disk folders of the folder mounts (a shared workspace's checkout, SHARE), sorted.
pub fn folder_disks() -> Vec<String> {
    if !ANY_FOLDER.load(Ordering::Acquire) {
        return Vec::new();
    }
    let mut disks: Vec<String> = FOLDER_MOUNTS.read().unwrap().iter().map(|(_, d)| normalize_path(&d.to_string_lossy())).collect();
    disks.sort();
    disks
}

/// `text` with each folder mount's virtual paths as the disk's (`roots`: (root, disk folder)
/// pairs): a root matches only at a path boundary, followed by `/` or by a byte that cannot continue
/// a name (`<root>2` is another folder).
pub fn shown_with<'a>(roots: &[(String, String)], text: &'a str) -> std::borrow::Cow<'a, str> {
    let mut out: Option<String> = None;
    for (root, disk) in roots {
        let cur = out.as_deref().unwrap_or(text);
        if !cur.contains(root.as_str()) {
            continue;
        }
        let mut s = String::with_capacity(cur.len());
        let mut rest = cur;
        while let Some(at) = rest.find(root.as_str()) {
            let after = &rest[at + root.len()..];
            let boundary = after.bytes().next().is_none_or(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.') || b >= 0x80));
            s.push_str(&rest[..at]);
            s.push_str(if boundary { disk.as_str() } else { root.as_str() });
            rest = after;
        }
        s.push_str(rest);
        out = Some(s);
    }
    match out {
        Some(s) => std::borrow::Cow::Owned(s),
        None => std::borrow::Cow::Borrowed(text),
    }
}

/// `text` as a shared workspace's user reads it ([`shown_with`] over this process's folder
/// mounts): the paths a run holds, its keys and stores, are the mount's; what it prints (a
/// diagnostic's location, a trace) names the files where an editor and a coding agent find them.
pub fn shown(text: &str) -> std::borrow::Cow<'_, str> {
    if !ANY_FOLDER.load(Ordering::Acquire) {
        return std::borrow::Cow::Borrowed(text);
    }
    let roots: Vec<(String, String)> = FOLDER_MOUNTS.read().unwrap().iter().map(|(r, d)| (r.clone(), normalize_path(&d.to_string_lossy()))).collect();
    match shown_with(&roots, text) {
        std::borrow::Cow::Owned(s) => std::borrow::Cow::Owned(s),
        std::borrow::Cow::Borrowed(_) => std::borrow::Cow::Borrowed(text),
    }
}

/// The crates' print macros (base, front, types and compiler lib.rs): a write whose error is
/// dropped; with a folder mount in place its paths are [`shown`].
pub fn write_err(text: std::fmt::Arguments<'_>) {
    use io::Write as _;
    if !ANY_FOLDER.load(Ordering::Relaxed) {
        let _ = io::stderr().write_fmt(text);
        return;
    }
    let _ = io::stderr().write_all(shown(&std::fmt::format(text)).as_bytes());
}

/// [`write_err`] for stdout.
pub fn write_out(text: std::fmt::Arguments<'_>) {
    use io::Write as _;
    if !ANY_FOLDER.load(Ordering::Relaxed) {
        let _ = io::stdout().write_fmt(text);
        return;
    }
    let _ = io::stdout().write_all(shown(&std::fmt::format(text)).as_bytes());
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

/// relative_path's containment for inputs already normalized (normalize_path): many paths
/// checked against folders normalized once.
pub fn is_under_normalized(path: &str, root: &str) -> bool {
    relative(path, root).is_some()
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
/// A path whose innermost mount is a folder mount has none ([`folder_path`]).
fn find(path: &str) -> Option<(Arc<Mount>, String)> {
    let p = normalize_path(path);
    let m = MOUNTS.read().unwrap();
    for mount in m.iter() {
        if let Some(rest) = relative(&p, &mount.root) {
            if ANY_FOLDER.load(Ordering::Acquire) && FOLDER_MOUNTS.read().unwrap().iter().any(|(r, _)| r.len() > mount.root.len() && relative(&p, r).is_some()) {
                return None;
            }
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

/// The outermost mount root containing `path` (a git run's virtual workspace root, a shared
/// workspace's mount), normalized.
pub fn outer_root(path: &str) -> Option<String> {
    let p = normalize_path(path);
    let m = MOUNTS.read().unwrap();
    let stores = m.iter().filter(|mount| relative(&p, &mount.root).is_some()).map(|mount| mount.root.clone());
    let folders = FOLDER_MOUNTS.read().unwrap();
    let folders = folders.iter().filter(|(r, _)| relative(&p, r).is_some()).map(|(r, _)| r.clone());
    stores.chain(folders).min_by_key(|r| r.len())
}

/// Where `path` (virtual) lives in the edits folder, whether or not it exists there.
/// A folder mount's path has none: the edits folder serves store mounts only.
fn edit_path(path: &str) -> Option<(std::path::PathBuf, String)> {
    let dir = EDITS.read().unwrap().clone()?;
    let path = normalize_path(path);
    if folder_mount(&path).is_some() {
        return None;
    }
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

/// The whiteout list changes on disk during a live run; stat it, parse only when it changes.
static WHITEOUTS: RwLock<Option<(std::path::PathBuf, Option<FileStat>, Arc<BTreeSet<String>>)>> = RwLock::new(None);

fn load_deleted(dir: &std::path::Path) -> BTreeSet<String> {
    std::fs::read_to_string(dir.join(".makepad-run-deleted")).unwrap_or_default().lines()
        .filter(|p| !p.is_empty()).map(str::to_string).collect()
}

fn save_deleted(dir: &std::path::Path, deleted: &BTreeSet<String>) -> io::Result<()> {
    let tmp = dir.join(".makepad-run-deleted.tmp");
    std::fs::write(&tmp, deleted.iter().map(|p| format!("{p}\n")).collect::<String>())?;
    std::fs::rename(tmp, dir.join(".makepad-run-deleted"))
}

fn deleted_paths(dir: &std::path::Path) -> Arc<BTreeSet<String>> {
    let stat = disk_stat(&dir.join(".makepad-run-deleted").to_string_lossy());
    let cache = WHITEOUTS.read().unwrap();
    if let Some((_, _, paths)) = cache.as_ref().filter(|(at, held, _)| at == dir && *held == stat) {
        return paths.clone();
    }
    drop(cache);
    let _k = crate::zone_alloc::kept();
    let paths = Arc::new(load_deleted(dir));
    *WHITEOUTS.write().unwrap() = Some((dir.to_path_buf(), stat, paths.clone()));
    paths
}

fn whiteout(path: &str) -> bool {
    if !ANY.load(Ordering::Acquire) { return false; }
    let Some((_, rel)) = edit_path(path) else { return false };
    let Some(dir) = EDITS.read().unwrap().clone() else { return false };
    deleted_paths(&dir).contains(&rel)
}

/// The files of a landing the JIT holds back (`inspect hold`, the compiler's reload.rs): path
/// (normalized) -> what a read of it gives meanwhile, the file as it was before the landing (None:
/// it was not there). A module the app loads while the landing is held, and an include a save
/// reads again, read what the running app was built from, never the landed text.
static LANDING_READS: relax_session::sync::Mutex<BTreeMap<String, Option<Arc<[u8]>>>> = relax_session::sync::Mutex::new(BTreeMap::new());
static ANY_LANDING_READ: AtomicBool = AtomicBool::new(false);

/// While a landing holds `path` back, its reads give `before` (None: not there).
pub fn hold_read(path: &str, before: Option<Arc<[u8]>>) {
    let _k = crate::zone_alloc::kept();
    let mut reads = LANDING_READS.lock().unwrap();
    reads.insert(normalize_path(path), before);
    ANY_LANDING_READ.store(true, Ordering::Release);
}

/// `path`'s reads (every held file's, None) give the disk's again.
pub fn release_read(path: Option<&str>) {
    let mut reads = LANDING_READS.lock().unwrap();
    match path {
        Some(p) => { reads.remove(&normalize_path(p)); }
        None => reads.clear(),
    }
    ANY_LANDING_READ.store(!reads.is_empty(), Ordering::Release);
}

/// What a read of `path` gives while a landing holds it back ([`hold_read`]).
fn landing_read(path: &std::path::Path) -> Option<Option<Arc<[u8]>>> {
    if !ANY_LANDING_READ.load(Ordering::Acquire) { return None; }
    LANDING_READS.lock().unwrap().get(&normalize_path(&path.to_string_lossy())).cloned()
}

/// Whether virtual files are writable (an edits folder is set).
pub fn edits_enabled() -> bool {
    EDITS.read().unwrap().is_some()
}

/// The edits folder from RELAX_EDITS (the runner sets it per licensed app).
pub fn set_edits(dir: Option<std::path::PathBuf>) {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    *EDITS.write().unwrap() = dir;
    *WHITEOUTS.write().unwrap() = None;
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

// ---- the store alone: for code that must reach nothing on disk (a confined build script,
// compiler sandbox/script.rs). A store mount's committed files only: never a folder mount's disk,
// the edits folder or a link's target (a link entry reads as its text). None: `path` is under no
// store mount (a folder mount's path is the disk's).

/// What a store mount holds at `path`.
pub enum StoreKind {
    /// a file: its length and stat time ([`store_time_ns`])
    File(u64, u64),
    Dir,
    Missing,
}

pub fn store_kind(path: &str) -> Option<StoreKind> {
    let (m, rel) = find(path)?;
    Some(match m.file(&rel) {
        Some(e) if e.mode == 0o160000 => StoreKind::Missing,
        Some(e) => StoreKind::File(m.store.object_size(&e.oid).ok().flatten().unwrap_or(0), store_time_ns(&m, Some(e))),
        None if m.is_dir(&rel) => StoreKind::Dir,
        None => StoreKind::Missing,
    })
}

pub fn store_read(path: &str) -> Option<io::Result<Vec<u8>>> {
    let (m, rel) = find(path)?;
    let Some(e) = m.file(&rel).filter(|e| e.mode != 0o160000) else { return Some(Err(not_found(path))) };
    Some(match m.store.object(&e.oid) {
        Ok(Some((_, data))) => Ok(data),
        Ok(None) => Err(io::Error::new(io::ErrorKind::InvalidData, format!("{path}: object missing from {}", m.store.path.display()))),
        Err(e) => Err(e),
    })
}

/// The root of the store mount `path` lies under (a git package's repository).
pub fn store_root(path: &str) -> Option<String> {
    find(path).map(|(m, _)| m.root.clone())
}

/// A store folder's entries (name, is folder).
pub fn store_list(path: &str) -> Option<io::Result<Vec<(String, bool)>>> {
    let (m, rel) = find(path)?;
    Some(m.folders().get(&rel).cloned().ok_or_else(|| not_found(path)))
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
    /// Snapshot keys preserve parent components until canonicalization records an alias.
    pub fn key(path: &str) -> String { normalize_components(path, false) }
    pub fn bind(self: &Arc<Self>) -> ReadScope {
        let id = BIND_SEQ.fetch_add(1, Ordering::Relaxed);
        // the thread's table outlives the compiler generation the load runs in
        let _k = crate::zone_alloc::kept();
        BOUND.with(|b| b.borrow_mut().push((id, self.clone())));
        ReadScope { id, _thread: std::marker::PhantomData }
    }
    pub fn alias(&self, path: &std::path::Path, canonical: &std::path::Path) {
        let path = Self::key(&path.to_string_lossy());
        let canonical = Self::key(&canonical.to_string_lossy());
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
    /// Drops the snapshot's copy for `ReadSnapshot::key` once its bytes live elsewhere; the
    /// bytes stay alive only for holders that already have them.
    pub fn release(&self, key: &str) {
        let mut paths = self.paths.lock().unwrap();
        let key = paths.aliases.get(key).cloned().unwrap_or_else(|| key.to_string());
        if let Some(entry) = paths.files.remove(&key) {
            if self.profile { paths.retired.push((key, entry)); }
        }
    }
    fn read_shared(&self, path: &std::path::Path) -> io::Result<Arc<[u8]>> {
        let key = Self::key(&path.to_string_lossy());
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
            eprintln!("relax source-read: calls={c} opens={o} path={path:?}");
        }
        eprintln!("relax source-reads: calls={calls} paths={} opens={opens} repeated_paths={repeated} max_opens_per_path={max}", totals.len());
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
    if let Some(held) = landing_read(path) { return held.ok_or_else(|| not_found(&path.to_string_lossy())); }
    if whiteout(&path.to_string_lossy()) { return Err(not_found(&path.to_string_lossy())); }
    let result = if let Some(snapshot) = read_snapshot() {
        snapshot.read_shared(path)
    } else {
        read_uncached(path, None).map(Arc::from)
    };
    if result.is_ok() { crate::why::read(path); }
    result
}

/// The file's bytes (a held landing's file: as it was before the landing, [`hold_read`]).
pub fn read(path: impl AsRef<std::path::Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    if let Some(held) = landing_read(path) { return held.map(|b| b.to_vec()).ok_or_else(|| not_found(&path.to_string_lossy())); }
    read_unheld(path)
}

/// The file's bytes as they are, a held landing's too (the live watch compares them with it).
pub fn read_unheld(path: impl AsRef<std::path::Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    if whiteout(&path.to_string_lossy()) { return Err(not_found(&path.to_string_lossy())); }
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
    if let Some(p) = folder_path(&s) { return read_disk(&p, entry); }
    if whiteout(&s) { return Err(not_found(&s)); }
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

/// When a store entry last changed, as its stat says (ns since the Unix epoch): derived from its
/// mount's commit and its blob, so it holds while the file does and moves whenever its blob does.
fn store_time_ns(mount: &Mount, entry: Option<&Entry>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    mount.commit.hash(&mut hash);
    if let Some(entry) = entry { entry.oid.hash(&mut hash); }
    hash.finish()
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
    if let Some(p) = folder_path(&s) { return disk_metadata(&p); }
    if whiteout(&s) { return Err(not_found(&s)); }
    if let Some((mount, rel)) = ANY.load(Ordering::Acquire).then(|| find(&s)).flatten() {
        if let Some(edited) = edited_file(&s) { return metadata(edited); }
        let is_dir = mount.is_dir(&rel);
        let entry = mount.file(&rel);
        if !is_dir && entry.is_none() { return Err(not_found(&s)); }
        return Ok(Metadata {
            len: if is_dir { 0 } else { read(path)?.len() as u64 },
            modified: std::time::UNIX_EPOCH + std::time::Duration::from_nanos(store_time_ns(&mount, entry)),
            is_dir, is_file: entry.is_some_and(|e| e.mode != 0o160000),
            is_symlink: entry.is_some_and(|e| e.mode == 0o120000),
        });
    }
    disk_metadata(path)
}

fn disk_metadata(path: &std::path::Path) -> io::Result<Metadata> {
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
        let folders = FOLDER_MOUNTS.read().unwrap();
        if let Some(rel) = mounts.iter().map(|m| m.root.as_str()).chain(folders.iter().map(|f| f.0.as_str())).find_map(|root| relative(&raw, root)) {
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
            drop(folders);
            if !exists(&normalized) { return Err(not_found(&normalized)); }
            return Ok(normalized.into());
        }
        drop(mounts);
        drop(folders);
        let normalized = normalize_path(&raw);
        if is_virtual(&normalized) {
            if !exists(&normalized) { return Err(not_found(&normalized)); }
            return Ok(normalized.into());
        }
    }
    std::fs::canonicalize(path)
}

/// A path's one spelling, for comparing two paths: [`canonicalize`]d where a mount or the disk
/// has it, else [`normalize_path`]d. A path under an overlay's virtual root is on no disk, and
/// Windows spells one path several ways (`C:\a` and `c:/a`).
pub fn canonical(path: &str) -> String {
    canonicalize(path).map(|p| normalize_path(&p.to_string_lossy())).unwrap_or_else(|_| normalize_path(path))
}

pub fn exists(path: impl AsRef<std::path::Path>) -> bool {
    let found = exists_inner(path.as_ref());
    if found {
        note(b'p', path.as_ref());
    }
    found
}

fn exists_inner(path: &std::path::Path) -> bool {
    if let Some(held) = landing_read(path) { return held.is_some(); }
    if !ANY.load(Ordering::Acquire) {
        return path.exists();
    }
    if let Some(p) = folder_path(&path.to_string_lossy()) { return p.exists(); }
    if whiteout(&path.to_string_lossy()) { return false; }
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
    if let Some(held) = landing_read(path) { return held.is_some(); }
    if !ANY.load(Ordering::Acquire) {
        return path.is_file();
    }
    if let Some(p) = folder_path(&path.to_string_lossy()) { return p.is_file(); }
    if whiteout(&path.to_string_lossy()) { return false; }
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
    if let Some(p) = folder_path(&path.to_string_lossy()) { return p.is_dir(); }
    if whiteout(&path.to_string_lossy()) { return false; }
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
    let folder = folder_path(&path.to_string_lossy());
    let path = folder.as_deref().unwrap_or(path);
    if folder.is_none() && whiteout(&path.to_string_lossy()) { return Err(not_found(&path.to_string_lossy())); }
    let disk = || -> io::Result<Vec<(String, bool)>> {
        let mut v: Vec<(String, bool)> = std::fs::read_dir(path)?
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
            .collect();
        v.sort();
        Ok(v)
    };
    if !ANY.load(Ordering::Acquire) || folder.is_some() {
        return disk();
    }
    match find(&path.to_string_lossy()) {
        None => disk(),
        Some((m, rel)) => {
            // the run's edits copy of the folder, when there is one (empty: a folder the run made)
            let edited: Option<Vec<(String, bool)>> = edit_path(&path.to_string_lossy()).and_then(|(p, _)| {
                std::fs::read_dir(p).ok().map(|d| {
                    d.flatten()
                        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
                        .filter(|(n, _)| !n.starts_with(".makepad-run-"))
                        .collect()
                })
            });
            let mut v = match m.folders().get(&rel) {
                Some(v) => v.clone(),
                None if edited.is_some() => Vec::new(),
                None => return Err(not_found(&path.to_string_lossy())),
            };
            for e in edited.unwrap_or_default() {
                if !v.iter().any(|x| x.0 == e.0) {
                    v.push(e);
                }
            }
            // repositories and folders mounted directly inside this folder are folders of it too
            let here = normalize_path(&path.to_string_lossy());
            let folders: Vec<String> = FOLDER_MOUNTS.read().unwrap().iter().map(|f| f.0.clone()).collect();
            for root in MOUNTS.read().unwrap().iter().map(|x| x.root.clone()).chain(folders) {
                if let Some(name) = relative(&root, &here) {
                    if !name.is_empty() && !name.contains('/') && !v.iter().any(|e| e.0 == name) {
                        v.push((name.to_string(), true));
                    }
                }
            }
            if let Some((_, rel)) = edit_path(&here) {
                if let Some(dir) = EDITS.read().unwrap().clone() {
                    let deleted = deleted_paths(&dir);
                    v.retain(|(name, _)| !deleted.contains(&if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") }));
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
    if let Some(p) = folder_path(&name) { return disk(&p); }
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
    if let Some(p) = folder_path(&name) { return std::fs::read_link(p); }
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

/// The stat of an on-disk path (None: virtual (a mounted store or an edits overlay, a folder
/// mount: its content names it), missing).
pub fn file_stat(path: &str) -> Option<FileStat> {
    if is_virtual(path) {
        return None;
    }
    disk_stat(path)
}

#[cfg(unix)]
fn disk_stat(path: &str) -> Option<FileStat> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    let mtime_ns = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() as u64;
    Some(FileStat { size: m.len(), mtime_ns, ctime_ns: m.ctime().max(0) as u64 * 1_000_000_000 + m.ctime_nsec().max(0) as u64, ino: m.ino() })
}

/// Windows' metadata has no change time or file index on stable Rust, so one attributes-only
/// handle (nothing reads the content; a folder opens too) answers all of it. A file system without
/// a change time answers 0: size and mtime are all its stat can say.
#[cfg(windows)]
fn disk_stat(path: &str) -> Option<FileStat> {
    use makepad_windows_sys::Win32::Foundation::HANDLE;
    use makepad_windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandle, GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    };
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    let f = std::fs::OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0).open(path).ok()?;
    let h = HANDLE(f.as_raw_handle() as _);
    let mut file = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(h, &mut file) }.ok()?;
    let mut times = FILE_BASIC_INFO::default();
    unsafe { GetFileInformationByHandleEx(h, FileBasicInfo, &mut times as *mut FILE_BASIC_INFO as *mut std::ffi::c_void, std::mem::size_of::<FILE_BASIC_INFO>() as u32) }.ok()?;
    // 100 ns ticks since 1601
    let ns = |t: i64| (t as u64).checked_sub(116_444_736_000_000_000).map(|t| t * 100);
    let wide = |high: u32, low: u32| (high as u64) << 32 | low as u64;
    Some(FileStat {
        size: wide(file.nFileSizeHigh, file.nFileSizeLow),
        mtime_ns: ns(times.LastWriteTime)?,
        ctime_ns: ns(times.ChangeTime).unwrap_or(0),
        ino: wide(file.nFileIndexHigh, file.nFileIndexLow),
    })
}

/// When the file last changed (a git file: never, None; the caller treats it as unchanged).
pub fn modified(path: impl AsRef<std::path::Path>) -> Option<std::time::SystemTime> {
    let path = path.as_ref();
    if let Some(p) = folder_path(&path.to_string_lossy()) {
        return std::fs::metadata(p).and_then(|m| m.modified()).ok();
    }
    if ANY.load(Ordering::Acquire) && find(&path.to_string_lossy()).is_some() {
        // an edited file changes on disk; the store's never does
        return edited_file(&path.to_string_lossy()).and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    }
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What a virtual path's content is fixed by: its mount's store and commit (None on disk, and under
/// a folder mount: its content names it). Caches that would compare file times compare this
/// instead.
pub fn identity(path: &str) -> Option<String> {
    if !ANY.load(Ordering::Acquire) {
        return None;
    }
    let (m, _) = find(path)?;
    if whiteout(path) || edited_file(path).is_some() {
        return None;
    }
    Some(match m.store.range {
        Some((offset, len)) => format!("{}@{offset}+{len}@{}", m.store.path.display(), store::hex(&m.commit)),
        None => format!("{}@{}", m.store.path.display(), store::hex(&m.commit)),
    })
}

// ---- the mount table as text (a child process started in a virtual folder mounts the same)

/// `root\tstore path\tcommit hex[\toffset+len]` per store mount, newline-separated (the range of a
/// store that sits inside a bigger file, gitstore.rs `open_range`), and `root\tfolder\tdir` per
/// folder mount.
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
    for (root, dir) in FOLDER_MOUNTS.read().unwrap().iter() {
        out.push_str(&format!("{root}\t{}\tdir\n", dir.display()));
    }
    out
}

/// The mount table as a key holds it: what the files are, not where a folder mount reads them
/// (`root\tstore@commit[@range]` per store mount, `root\tdir` per folder mount). Two checkouts of
/// one shared workspace give the same.
pub fn mounts_key_text() -> String {
    let mut out = String::new();
    for x in MOUNTS.read().unwrap().iter() {
        out.push_str(&format!("{}\t{}@{}", x.root, x.store.path.display(), store::hex(&x.commit)));
        if let Some((offset, len)) = x.store.range {
            out.push_str(&format!("@{offset}+{len}"));
        }
        out.push('\n');
    }
    for (root, _) in FOLDER_MOUNTS.read().unwrap().iter() {
        out.push_str(&format!("{root}\tdir\n"));
    }
    out
}

/// Mounts what `mounts_text` wrote (RELAX_VFS).
pub fn mount_text(text: &str) -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let mut it = line.split('\t');
        let (Some(root), Some(path), Some(commit)) = (it.next(), it.next(), it.next()) else { return Err(format!("RELAX_VFS: bad line {line}")) };
        if commit == "dir" {
            mount_folder(root, std::path::Path::new(path));
            continue;
        }
        let commit = store::from_hex(commit).ok_or_else(|| format!("RELAX_VFS: bad commit in {line}"))?;
        let s = match it.next() {
            None => Store::open(std::path::Path::new(path)).map_err(|e| format!("{path}: {e}"))?,
            Some(range) => {
                let (offset, len) = parse_range(range).ok_or_else(|| format!("RELAX_VFS: bad range in {line}"))?;
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

/// RELAX_VFS from the environment, mounted once (the loader calls this at start); without one,
/// a shared workspace's mount (RELAX_SHARE, `share_from_env`).
pub fn mount_from_env() -> Result<(), String> {
    // process state: built in the kept heap
    let _k = crate::zone_alloc::kept();
    record_from_env()?;
    if let Some(d) = std::env::var_os("RELAX_EDITS").filter(|d| !d.is_empty()) {
        set_edits(Some(d.into()));
    }
    match std::env::var("RELAX_VFS") {
        Ok(t) if !t.is_empty() => mount_text(&t),
        _ => share_from_env(),
    }
}

/// Where a shared workspace named `name` is mounted: `<relax home>/vfs/share/<name>` (beside
/// the git runs' labels and `vfs/git`, never among them).
pub fn share_root(name: &str) -> String {
    normalize_path(&store::relax_home().join("vfs").join("share").join(name).to_string_lossy())
}

/// RELAX_SHARE=<name>: the workspace of the current folder (cargo's root for it) is folder-mounted
/// at [`share_root`] and the run's workspace is the current folder's place there, said in
/// RELAX_VFS and RELAX_WORKSPACE as run facts (progenv.rs; the name is one too): this run again (a
/// self-exec child) and a build script of the mount (unitgraph.rs) are handed them; no other child
/// inherits them. A run handed a mount (RELAX_VFS) mounts that and never reaches here. A current
/// folder in no cargo workspace (or one whose manifest does not read) mounts nothing: the loader
/// mounts before it knows the command, `--version` and `inspect` need no workspace, and a command
/// that needs one reports the manifest as it does without RELAX_SHARE.
fn share_from_env() -> Result<(), String> {
    let Some(name) = std::env::var("RELAX_SHARE").ok().filter(|n| !n.is_empty()) else { return Ok(()) };
    if name.starts_with('.') || !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) {
        return Err(format!("RELAX_SHARE: `{name}` is not a name (letters, digits, `-`, `_`, `.`)"));
    }
    let root = share_root(&name);
    let Ok(cwd) = std::env::current_dir() else { return Ok(()) };
    let cwd = cwd.to_string_lossy().into_owned();
    let Ok(disk) = crate::resolve::find_root(&cwd) else { return Ok(()) };
    let rel = relative_path(&cwd, &disk).unwrap_or_default();
    mount_folder(&root, std::path::Path::new(&disk));
    crate::progenv::set_run_fact("RELAX_VFS", mounts_text());
    crate::progenv::set_run_fact("RELAX_WORKSPACE", if rel.is_empty() { root } else { format!("{root}/{rel}") });
    Ok(())
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

/// 0: not a virtual path (the disk), 1: a file (`*size` its length, `*mtime_ns` when it last
/// changed: an edited file's disk time, a store file's [`store_time_ns`]), 2: a folder, -1: virtual
/// but missing.
pub extern "C" fn relax_vfs_stat(p: *const u8, n: usize, size: *mut u64, mtime_ns: *mut u64) -> i32 {
    if !ANY.load(Ordering::Acquire) {
        return 0;
    }
    let path = unsafe { path_arg(p, n) };
    let since = |t: std::time::SystemTime| t.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    if let Some(disk) = folder_path(path) {
        return match std::fs::metadata(disk) {
            Ok(meta) if meta.is_dir() => 2,
            Ok(meta) => {
                unsafe {
                    *size = meta.len();
                    *mtime_ns = meta.modified().map_or(0, since);
                }
                1
            }
            Err(_) => -1,
        };
    }
    let Some((m, rel)) = find(path) else { return 0 };
    if let Some(f) = edited_file(path) {
        let meta = std::fs::metadata(f).ok();
        unsafe {
            *size = meta.as_ref().map_or(0, |m| m.len());
            *mtime_ns = meta.and_then(|m| m.modified().ok()).map_or(0, since);
        }
        return 1;
    }
    if let Some(e) = m.file(&rel) {
        if e.mode == 0o160000 {
            return -1;
        }
        let len = m.store.object_size(&e.oid).ok().flatten().unwrap_or(0);
        unsafe {
            *size = len;
            *mtime_ns = store_time_ns(&m, Some(e));
        }
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


// ---- edits (RELAX_EDITS): writes into virtual folders

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
pub fn blob_oid(data: &[u8]) -> Oid {
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
/// the path is not virtual or there is no edits folder (the file stays read-only). A folder
/// mount's path is written where it maps, as it is (no ledger).
pub fn prepare_edit(path: &str, keep: bool) -> io::Result<std::path::PathBuf> {
    if let Some(disk) = folder_path(path) {
        if let Some(parent) = disk.parent() {
            std::fs::create_dir_all(parent)?;
        }
        return Ok(disk);
    }
    let (disk, rel) = edit_path(path).ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "read-only (no RELAX_EDITS)"))?;
    if rel.is_empty() || rel.starts_with(".makepad-run-") {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "not an editable path"));
    }
    let _g = EDIT_LOCK.lock().unwrap();
    let dir = EDITS.read().unwrap().clone().unwrap();
    let mut deleted = load_deleted(&dir);
    if deleted.remove(&rel) { save_deleted(&dir, &deleted)?; }
    if disk.is_file() {
        return Ok(disk);
    }
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
    if let Some(disk) = folder_path(path) {
        return disk.exists().then_some(disk);
    }
    let (p, rel) = edit_path(path)?;
    (!rel.starts_with(".makepad-run-") && p.exists()).then_some(p)
}

/// The edits folder as one look found it: each file and folder in it (the run's own
/// `.relax-*` files left out) with its (modified time, length). A watcher's round that
/// looks at every virtual file takes their stamps from it: one walk of the edits instead of a
/// failing stat per unedited file (5,500 of them on a big app's every edit).
pub struct EditsLook(BTreeMap<String, (std::time::SystemTime, u64)>);

impl EditsLook {
    /// The edits folder now; None without one.
    pub fn now() -> Option<EditsLook> {
        let dir = EDITS.read().unwrap().clone()?;
        let mut found = BTreeMap::new();
        let mut todo = vec![(dir, String::new())];
        while let Some((at, rel)) = todo.pop() {
            let Ok(entries) = std::fs::read_dir(&at) else { continue };
            for e in entries.flatten() {
                let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
                if rel.is_empty() && name.starts_with(".makepad-run-") {
                    continue;
                }
                let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
                let Ok(m) = std::fs::metadata(e.path()) else { continue };
                let Ok(modified) = m.modified() else { continue };
                if m.is_dir() {
                    todo.push((e.path(), rel.clone()));
                }
                found.insert(rel, (modified, m.len()));
            }
        }
        Some(EditsLook(found))
    }

    /// Each edit in the folder: its place there (`edit_rel`) and its (modified time, length).
    pub fn entries(&self) -> impl Iterator<Item = (&str, (std::time::SystemTime, u64))> {
        self.0.iter().map(|(rel, stamp)| (rel.as_str(), *stamp))
    }
}

/// Where an edit of virtual `path` sits in the edits folder (relative to it), as `EditsLook`
/// lists it; None when it can have none there (a disk or frozen path, no edits folder, the run's
/// own files).
pub fn edit_rel(path: &str) -> Option<String> {
    let (_, rel) = edit_path(path)?;
    (!rel.is_empty() && !rel.starts_with(".makepad-run-")).then_some(rel)
}

/// Where an edit of `path` lives on disk (whether or not there is one yet): what a file watcher
/// watches for a virtual file. None for a disk path or without an edits folder.
pub fn edit_location(path: &str) -> Option<std::path::PathBuf> {
    if !is_virtual(path) { return None; }
    let (p, rel) = edit_path(path)?;
    (!rel.is_empty() && !rel.starts_with(".makepad-run-")).then_some(p)
}

/// The edits folder's record file (`.makepad-run-edits`): replaced whenever an edit is added.
pub fn edits_record() -> Option<std::path::PathBuf> {
    EDITS.read().unwrap().as_ref().map(|d| d.join(".makepad-run-edits"))
}

/// A folder made in a virtual folder: made in the edits folder (a folder mount's: where it maps).
pub fn prepare_edit_dir(path: &str) -> io::Result<std::path::PathBuf> {
    if let Some(disk) = folder_path(path) {
        return Ok(disk);
    }
    let (disk, rel) = edit_path(path).ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "read-only (no RELAX_EDITS)"))?;
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
    let mut deleted = load_deleted(&dir);
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
        if !disk.is_file() && !deleted.contains(&rel) {
            records.remove(&rel);
            changed = true;
            continue;
        }
        let vpath = format!("{outer}/{rel}");
        let Some((m, mrel)) = find(&vpath) else { continue };
        let now = m.file(&mrel).filter(|e| e.mode != 0o160000).map(|e| e.oid);
        if deleted.contains(&rel) {
            if now.is_some() && now == base { continue; }
            deleted.remove(&rel);
            records.remove(&rel);
            changed = true;
            out.push(format!("{rel}: whiteout removed; {}", if now.is_none() { "absent in this release" } else { "changed in this release too" }));
            continue;
        }
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
        if let Err(e) = save_deleted(&dir, &deleted) { out.push(format!("whiteouts: {e}")); }
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

    /// A held landing's files read as they were before it: changed, removed (still there) and
    /// added (not there yet) alike, through every read; the live watch reads them as they are; a
    /// release gives the disk back.
    #[test]
    fn a_held_landings_files_read_as_they_were_before_it() {
        let dir = std::env::temp_dir().join(format!("relax-vfs-landing-hold-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (changed, removed, added) = (dir.join("changed.rs"), dir.join("removed.rs"), dir.join("added.rs"));
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        std::fs::write(&changed, "landed").unwrap();
        std::fs::write(&added, "new").unwrap();
        hold_read(&s(&changed), Some(Arc::from(&b"before"[..])));
        hold_read(&s(&removed), Some(Arc::from(&b"was here"[..])));
        hold_read(&format!("{}/./added.rs", s(&dir)), None);
        assert_eq!(read(&changed).unwrap(), b"before");
        assert_eq!(&*read_shared(&changed).unwrap(), b"before");
        assert_eq!(read_to_string(&removed).unwrap(), "was here");
        assert!(exists(&removed) && is_file(&removed), "a file the landing removed is still there");
        assert!(!exists(&added) && !is_file(&added) && read(&added).is_err(), "one it added is not there yet (a normalized path)");
        assert_eq!(read_unheld(&changed).unwrap(), b"landed", "the watch reads the disk");
        release_read(Some(&s(&changed)));
        assert_eq!(read(&changed).unwrap(), b"landed");
        assert_eq!(read(&removed).unwrap(), b"was here", "one release, one file");
        release_read(None);
        assert!(!exists(&removed) && read(&added).unwrap() == b"new");
        std::fs::remove_dir_all(&dir).unwrap();
    }

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

    /// A watcher's look at the edits folder gives each virtual file the stamp a stat of its edit
    /// gives (an edited file, an edited file in a new folder), and none to an unedited file or
    /// to the run's own ledger.
    #[test]
    fn an_edits_look_stamps_each_virtual_file_as_its_stat_does() {
        let _one = EDITS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = test_store::Fixture::new(&[("app/a.rs", b"a"), ("app/b.rs", b"b")]);
        let root = "/edits-look-test/vfs/m";
        let edits = fixture.store.path.with_extension("look-edits");
        mount(root, fixture.store.clone(), &fixture.commit).unwrap();
        set_edits(Some(edits.clone()));
        std::fs::write(prepare_edit(&format!("{root}/app/a.rs"), false).unwrap(), b"a2").unwrap();
        std::fs::write(prepare_edit(&format!("{root}/app/new/c.rs"), false).unwrap(), b"c").unwrap();
        let look = EditsLook::now().unwrap();
        let found: BTreeMap<&str, _> = look.entries().collect();
        let stat = |p: &str| edited_disk_path(p).and_then(|d| std::fs::metadata(d).ok()).map(|m| (m.modified().unwrap(), m.len()));
        for p in ["app/a.rs", "app/new/c.rs", "app/b.rs", "app/none.rs", ".makepad-run-edits"] {
            let p = format!("{root}/{p}");
            assert_eq!(edit_rel(&p).and_then(|rel| found.get(rel.as_str()).copied()), stat(&p), "{p}");
        }
        assert!(stat(&format!("{root}/app/a.rs")).is_some() && stat(&format!("{root}/app/new/c.rs")).is_some());
        assert!(stat(&format!("{root}/app/b.rs")).is_none());
        set_edits(None);
        MOUNTS.write().unwrap().retain(|m| m.root != normalize_path(root));
        let _ = std::fs::remove_dir_all(&edits);
    }

    /// A virtual file's stat time (`relax_vfs_stat`: what the JIT std's `metadata().modified()`
    /// reports) holds while the file does and moves when it changes: a store file's with its blob,
    /// an edited file's with its disk time, a same-size edit included. Before, every virtual file
    /// stated time 0, and no mtime watcher of a program run from a store saw an edit.
    #[test]
    fn a_virtual_files_stat_time_moves_only_when_the_file_changes() {
        let _one = EDITS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let v1 = test_store::Fixture::new(&[("app/a.rs", b"a1"), ("app/b.rs", b"b1")]);
        let v2 = test_store::Fixture::new(&[("app/a.rs", b"a2"), ("app/b.rs", b"b1")]);
        let root = "/mtime-test/vfs/m";
        let edits = v1.store.path.with_extension("mtime-edits");
        let stat = |rel: &str| {
            let p = format!("{root}/{rel}");
            let (mut len, mut mtime) = (0, 0);
            assert_eq!(relax_vfs_stat(p.as_ptr(), p.len(), &mut len, &mut mtime), 1, "{p}");
            (len, mtime)
        };
        mount(root, v1.store.clone(), &v1.commit).unwrap();
        let a1 = stat("app/a.rs");
        assert_ne!(a1.1, 0, "a store file has a time");
        assert_eq!(stat("app/a.rs"), a1, "two stats with no change between them agree");
        mount(root, v2.store.clone(), &v2.commit).unwrap();
        assert_ne!(stat("app/a.rs").1, a1.1, "a new blob is a new time");
        set_edits(Some(edits.clone()));
        let disk = prepare_edit(&format!("{root}/app/a.rs"), false).unwrap();
        let at = |secs: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let edit = |text: &[u8], secs: u64| {
            std::fs::write(&disk, text).unwrap();
            std::fs::File::options().write(true).open(&disk).unwrap().set_modified(at(secs)).unwrap();
        };
        edit(b"edit-1", 1_700_000_000);
        assert_eq!(stat("app/a.rs"), (6, 1_700_000_000_000_000_000), "an edited file states its disk time");
        assert_eq!(stat("app/a.rs"), (6, 1_700_000_000_000_000_000), "and holds it while unchanged");
        edit(b"edit-2", 1_700_000_002);
        assert_eq!(stat("app/a.rs"), (6, 1_700_000_002_000_000_000), "a same-size edit moves it");
        set_edits(None);
        MOUNTS.write().unwrap().retain(|m| m.root != normalize_path(root));
        let _ = std::fs::remove_dir_all(&edits);
    }

    /// A game's store (asset client cache.rs open: create_dir_all(tmp), then read_dir(tmp)): a
    /// folder the run made in a virtual folder lists, empty, as the disk's new folder does; it
    /// was NotFound while it had no entries.
    #[test]
    fn a_folder_made_in_a_virtual_folder_lists_while_empty() {
        let _one = EDITS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = test_store::Fixture::new(&[("app/a.rs", b"a")]);
        let root = "/empty-dir-test/vfs/m";
        let edits = fixture.store.path.with_extension("empty-dir-edits");
        mount(root, fixture.store.clone(), &fixture.commit).unwrap();
        set_edits(Some(edits.clone()));
        let made = format!("{root}/app/cache/tmp");
        std::fs::create_dir_all(prepare_edit_dir(&made).unwrap()).unwrap();
        assert!(is_dir(&made));
        assert_eq!(read_dir(&made).unwrap(), Vec::<(String, bool)>::new());
        assert_eq!(read_dir(format!("{root}/app/cache")).unwrap(), vec![("tmp".to_string(), true)]);
        assert_eq!(read_dir(format!("{root}/app")).unwrap(), vec![("a.rs".to_string(), false), ("cache".to_string(), true)]);
        assert_eq!(read_dir(format!("{root}/app/none")).unwrap_err().kind(), io::ErrorKind::NotFound);
        set_edits(None);
        MOUNTS.write().unwrap().retain(|m| m.root != normalize_path(root));
        let _ = std::fs::remove_dir_all(&edits);
    }

    /// A test target's source under an overlay is on no disk: its spellings (the Windows host's
    /// separators among them) are one path.
    #[test]
    fn a_virtual_source_has_one_canonical_spelling() {
        let _one = EDITS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = test_store::Fixture::new(&[("app/tests/chain.rs", b"t")]);
        let root = "/canonical-test/vfs/m";
        mount(root, fixture.store.clone(), &fixture.commit).unwrap();
        let path = format!("{root}/app/tests/chain.rs");
        assert!(std::fs::canonicalize(&path).is_err());
        for spelling in [r"\canonical-test\vfs\m\app\tests\chain.rs", "/canonical-test/vfs/m/app/./src/../tests/chain.rs"] {
            assert_eq!(canonical(spelling), canonical(&path), "{spelling}");
        }
        assert_eq!(canonical(&path), path);
        MOUNTS.write().unwrap().retain(|m| m.root != normalize_path(root));
        // off every mount and disk: by its components, the drive's letter in one case
        assert_eq!(canonical(r"C:\nowhere\tests\chain.rs"), canonical("c:/nowhere/tests/chain.rs"));
    }

    /// A Windows host's overlay (a lease's tree on a drive): the test runner names a target's
    /// source `c:\..`, the package graph `C:\..` or `c:/..`, and a verbatim `\\?\C:\..` comes from
    /// a canonicalize. Each is the one path, under the mount and inside it: `makepad run --subject`
    /// found no test target for makepad-git's worktree_conformance.rs on such a node (11-06).
    #[test]
    fn a_windows_overlay_source_has_one_spelling_per_drive_case_and_separator() {
        let _one = EDITS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = test_store::Fixture::new(&[("libs/git/tests/worktree_conformance.rs", b"t")]);
        let root = r"C:\Users\u\devhub\native\lane";
        mount(root, fixture.store.clone(), &fixture.commit).unwrap();
        let graph = r"C:\Users\u\devhub\native\lane\libs\git\tests\worktree_conformance.rs";
        assert!(is_virtual(graph));
        for spelling in [
            r"c:\Users\u\devhub\native\lane\libs\git\tests\worktree_conformance.rs",
            "c:/Users/u/devhub/native/lane/libs/git/tests/worktree_conformance.rs",
            "C:/Users/u/devhub/native/lane/libs/git/tests/worktree_conformance.rs",
            r"\\?\C:\Users\u\devhub\native\lane\libs\git\tests\worktree_conformance.rs",
            r"c:\Users\u\devhub\native\lane\libs\git\src\..\tests\worktree_conformance.rs",
        ] {
            assert_eq!(canonical(spelling), canonical(graph), "{spelling}");
        }
        assert_eq!(canonical(graph), "c:/Users/u/devhub/native/lane/libs/git/tests/worktree_conformance.rs");
        MOUNTS.write().unwrap().retain(|m| m.root != normalize_path(root));
    }

    /// A shared workspace's text as its user reads it (`shown_with`, pure over the mount table): a
    /// root names the disk folder only at a path boundary, so share `ws` leaves share `ws2` alone.
    #[test]
    fn shown_paths_match_a_root_only_at_a_path_boundary() {
        let roots = [("/h/vfs/share/ws".to_string(), "/src/a".to_string()), ("/h/vfs/share/ws2".to_string(), "/src/b".to_string())];
        assert_eq!(shown_with(&roots, "/h/vfs/share/ws/src/x.rs:3:1"), "/src/a/src/x.rs:3:1");
        assert_eq!(shown_with(&roots, "in /h/vfs/share/ws2/y.rs and /h/vfs/share/ws"), "in /src/b/y.rs and /src/a", "each root, its own folder");
        assert_eq!(shown_with(&roots, "/h/vfs/share/ws-old/z /h/vfs/share/ws_1 /h/vfs/share/ws.x"), "/h/vfs/share/ws-old/z /h/vfs/share/ws_1 /h/vfs/share/ws.x", "a longer name is another folder");
        assert_eq!(shown_with(&roots, "'/h/vfs/share/ws': nothing"), "'/src/a': nothing", "a quote ends a name");
        assert!(matches!(shown_with(&roots, "no path here"), std::borrow::Cow::Borrowed(_)), "text without a root is not copied");
    }

    /// A folder mount (a shared workspace, SHARE) is its disk folder under another name: every read
    /// and probe answers the disk's, edits write the disk file itself, `on_disk` and `virtual_of`
    /// map both ways, keys name the mount without the folder, and a store mount beside it or inside
    /// it serves its own files (the edits folder of the port's first shape served a git
    /// dependency's `src/lib.rs` from the checkout's).
    #[test]
    fn a_folder_mount_is_its_folder_under_another_name() {
        let _one = EDITS_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("relax-vfs-folder-{}", std::process::id()));
        let checkout = base.join("checkout");
        std::fs::create_dir_all(checkout.join("src")).unwrap();
        std::fs::create_dir_all(checkout.join("empty")).unwrap();
        std::fs::write(checkout.join("src/lib.rs"), "checkout").unwrap();
        std::fs::write(checkout.join("Cargo.toml"), "[package]").unwrap();
        let disk = normalize_path(&checkout.to_string_lossy());
        let root = normalize_path(&base.join("vfs/share/ws").to_string_lossy());
        let dep_root = normalize_path(&base.join("vfs/git/dep/abc").to_string_lossy());
        let inner_root = format!("{root}/vendor/dep");
        mount_folder(&root, &checkout);
        let dep = test_store::Fixture::new(&[("src/lib.rs", b"dep"), ("Cargo.toml", b"[dep]")]);
        mount(&dep_root, dep.store.clone(), &dep.commit).unwrap();
        mount(&inner_root, dep.store.clone(), &dep.commit).unwrap();
        let lib = format!("{root}/src/lib.rs");

        assert_eq!(read_to_string(&lib).unwrap(), "checkout");
        assert!(exists(&lib) && is_file(&lib) && !is_dir(&lib) && is_dir(format!("{root}/src")) && is_dir(&root));
        assert!(!exists(format!("{root}/none.rs")) && read(format!("{root}/none.rs")).is_err());
        assert_eq!(metadata(&lib).unwrap().len, 8);
        let mut names: Vec<String> = read_dir(&root).unwrap().into_iter().map(|(n, _)| n).collect();
        names.sort();
        assert_eq!(names, ["Cargo.toml", "empty", "src"], "the folder's own entries");
        assert!(read_dir(format!("{root}/empty")).unwrap().is_empty());
        assert_eq!(canonicalize(format!("{root}/src/../src/lib.rs")).unwrap(), std::path::PathBuf::from(&lib), "the virtual spelling, never the disk's");
        assert!(is_virtual(&lib) && in_folder(&lib) && !in_store(&lib));
        assert!(identity(&lib).is_none() && file_stat(&lib).is_none(), "its content names it");

        assert_eq!(on_disk(&lib), format!("{disk}/src/lib.rs"));
        assert_eq!(on_disk(&root), disk);
        assert_eq!(virtual_of(&format!("{disk}/src/lib.rs")), lib, "a path a script gave under its folder on disk");
        assert_eq!(virtual_of(&disk), root);
        assert_eq!(virtual_of(&format!("{disk}2/x")), format!("{disk}2/x"), "a longer folder name is not the checkout");
        assert_eq!(on_disk("/elsewhere/x"), "/elsewhere/x");
        assert_eq!(shown(&format!("{lib}:1:2")), format!("{disk}/src/lib.rs:1:2"));

        std::fs::write(prepare_edit(&format!("{root}/src/new/m.rs"), false).unwrap(), "new").unwrap();
        assert_eq!(std::fs::read_to_string(checkout.join("src/new/m.rs")).unwrap(), "new", "an edit writes the checkout");
        assert_eq!(edited_disk_path(&lib), Some(checkout.join("src/lib.rs")));
        std::fs::write(checkout.join("src/lib.rs"), "changed").unwrap();
        assert_eq!(read_to_string(&lib).unwrap(), "changed", "no copy: the disk as it is now");

        for r in [&dep_root, &inner_root] {
            assert_eq!(read_to_string(format!("{r}/src/lib.rs")).unwrap(), "dep", "{r}: the store's own text");
            assert!(in_store(&format!("{r}/src/lib.rs")) && !in_folder(&format!("{r}/src/lib.rs")));
        }
        assert_eq!(on_disk(&format!("{inner_root}/src/lib.rs")), format!("{inner_root}/src/lib.rs"), "a store file is on no disk");

        let text = mounts_text();
        assert!(text.contains(&format!("{root}\t{}\tdir\n", checkout.display())), "{text}");
        let key = mounts_key_text();
        assert!(key.contains(&format!("{root}\tdir\n")) && !key.contains(&disk), "a key names no checkout: {key}");
        let other = normalize_path(&base.join("vfs/share/other").to_string_lossy());
        mount_text(&format!("{other}\t{}\tdir\n", checkout.display())).unwrap();
        assert_eq!(read_to_string(format!("{other}/Cargo.toml")).unwrap(), "[package]", "the table's text mounts it again");

        MOUNTS.write().unwrap().retain(|m| m.root != dep_root && m.root != inner_root);
        let mut folders = FOLDER_MOUNTS.write().unwrap();
        folders.retain(|(r, _)| *r != root && *r != other);
        if folders.is_empty() {
            ANY_FOLDER.store(false, Ordering::Release);
        }
        drop(folders);
        std::fs::remove_dir_all(&base).unwrap();
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
            let (mut len, mut mtime) = (0, 0);
            assert_eq!(relax_vfs_stat(path.as_ptr(), path.len(), &mut len, &mut mtime), 1);
            assert_eq!(len, bytes.len() as u64);
            assert_eq!(relax_vfs_stat(missing.as_ptr(), missing.len(), &mut len, &mut mtime), -1);
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

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&HELD, relax_session::sync::Mutex::new(Vec::new()));
    reset(&LANDING_READS, relax_session::sync::Mutex::new(BTreeMap::new()));
    reset(&ANY_LANDING_READ, AtomicBool::new(false));
}
