//! Clearing this machine's JIT cache: the run dirs `makepad run` keeps its compiled code in
//! (relax `dirs.rs`: `<target dir>/relax` per workspace, `<run home>/target/<8 hex>` per git
//! run). The next start of an app on a cleared cache is cold.
//!
//! Which run dirs: the checkout devhub was built from, the git-run dirs under the run home, the
//! run dir every running JIT announces (`run_dir` in its discovery file) and the ones announced
//! before (remembered in `~/.makepad/devhub/jit-caches`). A dir is cleared only when it is what a
//! run dir is: a real folder (not a link) named `relax` holding a `cache/` or `startup/`, or
//! an 8-hex folder directly under the run home's `target/`.
//!
//! What goes, by explicit name inside a run dir, and nothing else: `cache/` (the compiled stores:
//! code, front images, graph certificates, collected programs; the live locks of running processes
//! stay) and `startup/` (startup manifests). What stays: `registry/` (downloaded packages),
//! `git.lock` (dependency pins), `windows/` (the apps' window geometry), `build/` and
//! `build-scripts/` (build script outputs) and anything else; never `~/.cargo`, cargo's own
//! `target/` output, the run home's git source stores (`~/.relax/cache`) or any source. One app (`app=<package>`): its
//! startup manifest and its own crate's stores (`<crate>-<16 hex>.rcs` and their generations);
//! the stores shared with other apps stay, so only its own crate compiles again.
//!
//! In use: a running process of the cache holds its live lock (`cache/live/<build>`), so the
//! check is the lock itself (every `makepad run`, `check`, `test`, announced or not); the
//! announced JITs name who. A clear of a cache in use is refused unless forced; forced, the files
//! go and the folders stay. A running JIT keeps what it mapped (unlinked files live on until
//! unmapped; JIT code is in memory and only grows) and writes new stores into the kept folders.
//!
//! Removal walks the tree with explicit paths: no shell, no glob, links removed, never followed.
//!
//! The disk guard takes one Relax build's stores out of a run dir at a time ([`builds`],
//! [`retire`]): what a store file's header names (relax-session store_file), under the build's
//! live lock, as the JIT's own prune retires a build.

use makepad_remote::discovery;
use makepad_remote::json_str;
use relax_session::store_file as header;
use std::path::{Path, PathBuf};

/// What a run dir is, for the line that names it.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// `<target>/relax`
    Workspace,
    /// `<run home>/target/<8 hex>` (a git repository run's virtual workspace)
    GitRun,
}

/// A running process on a cache: (pid, app).
pub type User = (u32, String);

#[derive(Clone, Debug)]
pub struct CachePlan {
    pub dir: PathBuf,
    pub kind: Kind,
    /// files and bytes a clear removes (the scope's)
    pub files: u64,
    pub bytes: u64,
    /// bytes that stay (downloads, window geometry, ...)
    pub kept_bytes: u64,
    /// the announced JITs on it
    pub users: Vec<User>,
    /// live locks held (running processes of this cache, announced or not)
    pub held: usize,
}

impl CachePlan {
    pub fn in_use(&self) -> bool {
        self.held > 0 || !self.users.is_empty()
    }

    fn json(&self) -> String {
        let users: Vec<String> = self.users.iter().map(|(p, a)| format!("{{\"pid\":{p},\"app\":{}}}", json_str(a))).collect();
        format!(
            "{{\"dir\":{},\"kind\":\"{}\",\"files\":{},\"bytes\":{},\"kept_bytes\":{},\"held\":{},\"users\":[{}]}}",
            json_str(&self.dir.display().to_string()),
            if self.kind == Kind::Workspace { "workspace" } else { "git" },
            self.files,
            self.bytes,
            self.kept_bytes,
            self.held,
            users.join(",")
        )
    }
}

/// The entries of a run dir a clear removes; everything else stays.
pub const CLEARED: &[&str] = &["cache", "startup"];

fn run_home() -> PathBuf {
    std::env::var_os("RELAX_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
        home.join(".relax")
    })
}

/// Where the dev store keeps its run dirs: every overlay lease of this box runs on it
/// (lease/relaxgit.rs), under a run home of its own.
fn dev_runs() -> PathBuf {
    crate::lease::caps::LeaseConf::load().work.join("dev/run-home/target")
}

fn remembered_file() -> PathBuf {
    crate::proto::home().join("jit-caches")
}

/// Records a run dir a JIT announced (once), so a later clear finds it after the JIT ended.
pub fn remember(dir: &str) {
    let dir = dir.trim();
    if dir.is_empty() || dir.contains('\n') {
        return;
    }
    let f = remembered_file();
    let text = std::fs::read_to_string(&f).unwrap_or_default();
    if text.lines().any(|l| l == dir) {
        return;
    }
    let _ = std::fs::create_dir_all(crate::proto::home());
    use std::io::Write;
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(&f) {
        let _ = writeln!(out, "{dir}");
    }
}

/// The announced JITs of this machine: (pid, app, run dir). The registry is read without
/// `discovery::apps` (which removes the files of ended processes: the hub's crash detection
/// needs them).
fn announced() -> Vec<(u32, String, Option<PathBuf>)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(discovery::registry_dir()) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("makepad-remote-").and_then(|r| r.strip_suffix(".json")).and_then(|p| p.parse::<u32>().ok()) else { continue };
        if !crate::procinfo::alive(pid) {
            continue;
        }
        let Some(app) = std::fs::read_to_string(e.path()).ok().and_then(|t| discovery::parse(&t)) else { continue };
        if app.text("run").is_none() {
            continue;
        }
        // a run that predates `run_dir`: the workspace it was started in (Linux), found upwards
        let dir = app.text("run_dir").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| below_cwd(pid));
        out.push((pid, app.app, dir));
    }
    out.sort_by_key(|j| j.0);
    out
}

/// `<folder>/target/relax` of the nearest folder at or above the process's working
/// directory that has one (Linux: `/proc/<pid>/cwd`).
fn below_cwd(pid: u32) -> Option<PathBuf> {
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    cwd.ancestors().map(|a| a.join("target").join("relax")).find(|d| d.is_dir())
}

/// `dir` as a run dir (its real path), or why it is not one.
pub fn checked(dir: &Path) -> Result<(PathBuf, Kind), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.file_type().is_dir() {
        return Err(format!("{} is not a folder (a link or a file)", dir.display()));
    }
    let real = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = real.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "relax" {
        if !(real.join("cache").is_dir() || real.join("startup").is_dir()) {
            return Err(format!("{} holds no cache/ or startup/: not a run dir", real.display()));
        }
        return Ok((real, Kind::Workspace));
    }
    let in_runs = |runs: PathBuf| std::fs::canonicalize(runs).is_ok_and(|r| real.parent() == Some(&*r));
    if name.len() == 8 && name.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) && (in_runs(run_home().join("target")) || in_runs(dev_runs())) {
        return Ok((real, Kind::GitRun));
    }
    Err(format!("{} is neither a `relax` folder nor a git run's dir under {} or {}", real.display(), run_home().join("target").display(), dev_runs().display()))
}

/// Every run dir of this machine (checked, each once).
pub fn caches() -> Vec<(PathBuf, Kind)> {
    let mut cand: Vec<PathBuf> = Vec::new();
    // the checkout devhub was built from (libs/devhub is two folders below its root), and the
    // one it runs in
    cand.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/relax"));
    if let Ok(cwd) = std::env::current_dir() {
        cand.push(cwd.join("target").join("relax"));
    }
    for runs in [run_home().join("target"), dev_runs()] {
        if let Ok(rd) = std::fs::read_dir(runs) {
            cand.extend(rd.flatten().map(|e| e.path()));
        }
    }
    for (_, _, dir) in announced() {
        cand.extend(dir);
    }
    cand.extend(std::fs::read_to_string(remembered_file()).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from));
    let mut out: Vec<(PathBuf, Kind)> = Vec::new();
    for c in cand {
        if let Ok((real, kind)) = checked(&c) {
            if !out.iter().any(|(d, _)| *d == real) {
                out.push((real, kind));
            }
        }
    }
    out
}

/// The live locks of `dir` that a running process holds.
pub(crate) fn held_locks(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir.join("cache").join("live")).into_iter().flatten().flatten() {
        let p = e.path();
        let Ok(f) = std::fs::OpenOptions::new().read(true).open(&p) else { continue };
        // a process of this cache holds it shared while it runs (relax store.rs hold_build)
        if f.try_lock().is_err() {
            out.push(p);
        }
    }
    out
}

/// One Relax build's stores in a run dir's `cache/` (relax store.rs: a store file's header names
/// the build that wrote it, and a running process of the build holds `cache/live/<build>` shared).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BuildStores {
    pub build: u64,
    pub files: usize,
    /// bytes on disk
    pub bytes: u64,
    /// when a run of it last used them, unix seconds: its newest store stamp (a run stamps the
    /// stores of its own build it opens) or its live lock's (stamped when a run takes it)
    pub used: u64,
    /// a running process of the build holds its live lock
    pub running: bool,
}

/// A store file's name (relax store.rs name_ours, split_gen): `<crate>-<16 hex>.rcs`, or a
/// generation of one (`.rcs.<n>`).
fn store_name(n: &str) -> bool {
    let base = match n.rsplit_once('.') {
        Some((b, g)) if b.ends_with(".rcs") && !g.is_empty() && g.len() <= 9 && !g.starts_with('0') && g.bytes().all(|c| c.is_ascii_digit()) => b,
        _ => n,
    };
    let Some((c, h)) = base.strip_suffix(".rcs").and_then(|s| s.rsplit_once('-')) else { return false };
    !c.is_empty() && c.len() <= 128 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The build a store file's header names (relax-session store_file); None: not a store of the
/// store's format (an older format's header names none: the JIT's own prune retires those), or its
/// writer holds it right now.
fn store_build(p: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(p).ok()?;
    f.try_lock_shared().ok()?;
    let mut h = [0u8; header::HEADER];
    let ours = std::io::Read::read_exact(&mut f, &mut h).is_ok() && &h[..4] == header::MAGIC && h[4..8] == header::FORMAT.to_le_bytes();
    let _ = f.unlock();
    ours.then(|| u64::from_le_bytes(h[header::BUILD_AT..header::BUILD_AT + 8].try_into().unwrap()))
}

fn live_lock(dir: &Path, build: u64) -> PathBuf {
    dir.join("cache").join("live").join(format!("{build:016x}"))
}

/// The store files of run dir `dir`: (path, its build, its metadata).
fn store_files(dir: &Path) -> Vec<(PathBuf, u64, std::fs::Metadata)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir.join("cache")).into_iter().flatten().flatten() {
        let p = e.path();
        if !e.file_name().to_str().is_some_and(store_name) {
            continue;
        }
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if !m.file_type().is_file() {
            continue;
        }
        if let Some(b) = store_build(&p) {
            out.push((p, b, m));
        }
    }
    out
}

/// The builds whose stores the run dir `dir` holds, the most recently used first. One dir holds
/// the stores of every Relax build that ran there (another build's stay a day: relax cache.rs
/// OTHER_UNUSED_FOR), so on a box whose JIT is rebuilt at each landing one dir is most of its disk.
pub(crate) fn builds(dir: &Path) -> Vec<BuildStores> {
    let mut by: std::collections::BTreeMap<u64, BuildStores> = std::collections::BTreeMap::new();
    for (_, b, m) in store_files(dir) {
        let (bytes, mtime) = crate::os::on_disk(&m);
        let s = by.entry(b).or_insert(BuildStores { build: b, files: 0, bytes: 0, used: 0, running: false });
        s.files += 1;
        s.bytes += bytes;
        s.used = s.used.max(mtime);
    }
    for s in by.values_mut() {
        let lock = live_lock(dir, s.build);
        if let Ok(m) = std::fs::metadata(&lock) {
            s.used = s.used.max(crate::os::on_disk(&m).1);
        }
        s.running = std::fs::OpenOptions::new().read(true).open(&lock).is_ok_and(|f| f.try_lock().is_err());
    }
    let mut v: Vec<BuildStores> = by.into_values().collect();
    v.sort_by_key(|s| std::cmp::Reverse(s.used));
    v
}

/// The deployment moved on this box: the dev run dirs ([`dev_runs`]) and the tree runs' relax
/// dirs ([`tree_runs`], of the deployment checkout `checkout`) retire the stores of every Relax
/// build older than the previous one ([`superseded`]), and this move's time is kept for the next.
/// Relax's own prune keeps another build's stores a day, or until the volume is down to its free
/// floor (relax cache.rs OTHER_UNUSED_FOR); each landing that changes the JIT writes a cold set of
/// 2-22 GB, so one dir held 67 builds, 134 GB, and a box sat at that floor (11-19).
/// (builds retired, bytes freed)
pub fn deployment_moved(checkout: &Path) -> (usize, u64) {
    let keep = kept_builds();
    let stamp = crate::proto::home().join("deployed-at");
    let since = std::fs::read_to_string(&stamp).ok().and_then(|s| s.trim().parse::<u64>().ok());
    let _ = std::fs::write(&stamp, (crate::proto::wall_ms() / 1000).to_string());
    // (the first move a box records has no earlier one to measure use from)
    let Some(since) = since else { return (0, 0) };
    let conf = crate::lease::caps::LeaseConf::load();
    let dirs = std::fs::read_dir(dev_runs()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir());
    retire_superseded(dirs.chain(tree_runs(&conf.work, checkout, conf.role.users())), since, &keep)
}

/// Each dir's [`superseded`] builds retired: (builds retired, bytes freed).
fn retire_superseded(dirs: impl Iterator<Item = PathBuf>, since: u64, keep: &std::collections::HashSet<u64>) -> (usize, u64) {
    let (mut n, mut freed) = (0, 0);
    for dir in dirs {
        for b in superseded(&builds(&dir), since, keep) {
            if let Ok(f) = retire(&dir, b) {
                n += 1;
                freed += f;
            }
        }
    }
    (n, freed)
}

/// The relax dirs a tree run keeps its stores in (`<cargo target>/relax`, relax dirs.rs): the
/// work tree's target, `<work>/ci-target` (every CI leg and gate a tree lease runs there), and
/// on a box that is no person's own (`users`: its role's) the deployment checkout's `target`. A
/// landing that changes the JIT writes a cold set there as in the dev run dirs; a CI box's work
/// target held 709 GB of stores, 698 GB of them from the last day, and its disk guard could take
/// none of it while a lease ran (11-22).
pub(crate) fn tree_runs(work: &Path, checkout: &Path, users: bool) -> Vec<PathBuf> {
    let mut dirs = vec![work.join("ci-target").join("relax")];
    if !users {
        dirs.push(checkout.join("target").join("relax"));
    }
    dirs.retain(|d| d.join("cache").is_dir());
    dirs
}

/// What a deployment's move retires of one run dir's `builds` (the most recently used first):
/// all but the two most recently used (the current build and the previous one), those a run used
/// since the move before this one (`since`, unix seconds: a lane's build in use), those a running
/// process holds and those a timed series keeps (`keep`, [`kept_builds`]).
fn superseded(builds: &[BuildStores], since: u64, keep: &std::collections::HashSet<u64>) -> Vec<u64> {
    builds.iter().skip(2).filter(|b| !b.running && b.used < since && !keep.contains(&b.build)).map(|b| b.build).collect()
}

/// A series' last builds by lane: `<home>/series-builds/<lane>`, one build per line (16 hex).
fn series_dir() -> PathBuf {
    crate::proto::home().join("series-builds")
}

fn lane_name(l: &str) -> bool {
    !l.is_empty() && l.len() <= 64 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A timed lease of `lane` ended here: the builds the dev run dirs used from its first run's
/// start (`since`, unix seconds) on are its series' last build, which a deployment's retire keeps
/// while the master names the lane ([`series_keep`]). A series that went cold between its runs
/// would time its next run's compile (its gaps can outlast a deployment).
pub fn note_series(lane: &str, since: u64) {
    if !lane_name(lane) {
        return;
    }
    let mut ids = std::collections::BTreeSet::new();
    for dir in std::fs::read_dir(dev_runs()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        ids.extend(builds(&dir).into_iter().filter(|b| b.used >= since).map(|b| b.build));
    }
    if ids.is_empty() {
        return;
    }
    let text: Vec<String> = ids.iter().map(|b| format!("{b:016x}")).collect();
    let _ = std::fs::create_dir_all(series_dir()).and_then(|_| std::fs::write(series_dir().join(lane), text.join("\n")));
}

/// The master names the lanes whose timed series keep their last build through the next
/// deployment's retire (`-`: none): its timed leases queued, live or just ended, its named
/// series and its held branches (lease/sched.rs `series_lanes`).
pub fn series_keep(lanes: &str) -> String {
    let lanes: Vec<&str> = lanes.split(',').map(str::trim).filter(|l| lane_name(l)).collect();
    match std::fs::write(crate::proto::home().join("series-keep"), lanes.join("\n")) {
        Ok(()) => format!("{{\"series_keep\":{}}}", lanes.len()),
        Err(e) => format!("{{\"err\":{}}}", makepad_remote::json_str(&format!("series-keep: {e}"))),
    }
}

/// The builds the lanes the master named last keep ([`series_keep`], [`note_series`]).
fn kept_builds() -> std::collections::HashSet<u64> {
    let lanes = std::fs::read_to_string(crate::proto::home().join("series-keep")).unwrap_or_default();
    lanes.lines().filter(|l| lane_name(l)).flat_map(|l| std::fs::read_to_string(series_dir().join(l)).unwrap_or_default().lines().filter_map(|b| u64::from_str_radix(b.trim(), 16).ok()).collect::<Vec<_>>()).collect()
}

/// Removes build `build`'s stores from run dir `dir` as the store retires a build (relax store.rs
/// retire_build): under the build's live lock held exclusively, so no run of it starts meanwhile
/// (one that starts waits for the lock, then finds the file gone and takes a new one: a cold
/// start), then the lock file goes. Only files with a store's name whose header names the build go;
/// what else the dir holds (build script outputs, startup manifests) the JIT keeps by use. Refused
/// while a run of the build holds the lock. The bytes freed.
pub(crate) fn retire(dir: &Path, build: u64) -> Result<u64, String> {
    let p = live_lock(dir, build);
    let live = dir.join("cache").join("live");
    std::fs::create_dir_all(&live).map_err(|e| format!("{}: {e}", live.display()))?;
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    if f.try_lock().is_err() {
        return Err(format!("a run of build {build:016x} holds its live lock"));
    }
    // a retire by the JIT's own prune may have removed the file between the open and the lock
    // (a run of the build then locks a new one there)
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let same = matches!((f.metadata(), std::fs::metadata(&p)), (Ok(a), Ok(b)) if (a.dev(), a.ino()) == (b.dev(), b.ino()));
        if !same {
            return Err(format!("build {build:016x}'s live lock was retired meanwhile"));
        }
    }
    let mut freed = 0;
    for (path, b, m) in store_files(dir) {
        if b == build && std::fs::remove_file(&path).is_ok() {
            freed += crate::os::on_disk(&m).0;
        }
    }
    // a run that locks the file checks it is still there (hold_build); on Windows an open file's
    // name stays taken until it closes, so the empty lock file stays
    #[cfg(unix)]
    let _ = std::fs::remove_file(&p);
    let _ = f.unlock();
    Ok(freed)
}

/// The package names `app` may mean (`foo` is `makepad-app-foo`).
fn app_names(app: &str) -> Vec<String> {
    let mut v = vec![app.to_string()];
    if !app.starts_with("makepad-") {
        v.push(format!("makepad-app-{app}"));
    }
    v
}

/// One app's files in run dir `dir`: its startup manifest and its crate's own stores.
fn app_files(dir: &Path, app: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in app_names(app) {
        let s = dir.join("startup").join(format!("{name}.startup"));
        if s.is_file() {
            out.push(s);
        }
        let prefix = format!("{}-", name.replace('-', "_"));
        for e in std::fs::read_dir(dir.join("cache")).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            // `<crate>-<16 hex>.rcs` or a generation of it (`.rcs.<n>`)
            let base = match n.rsplit_once('.') {
                Some((b, g)) if b.ends_with(".rcs") && g.bytes().all(|c| c.is_ascii_digit()) && !g.is_empty() => b.to_string(),
                _ => n.clone(),
            };
            let Some(hex) = base.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".rcs")) else { continue };
            if hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()) && e.file_type().is_ok_and(|t| t.is_file()) {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Files and bytes below `p` (links counted as files of no size, never followed).
fn tree_size(p: &Path) -> (u64, u64) {
    let Ok(m) = std::fs::symlink_metadata(p) else { return (0, 0) };
    if !m.file_type().is_dir() {
        return (1, if m.file_type().is_file() { m.len() } else { 0 });
    }
    let mut t = (0, 0);
    for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
        let (f, b) = tree_size(&e.path());
        t.0 += f;
        t.1 += b;
    }
    t
}

/// What a clear of `dir` (one app's files, or the whole cache) would remove.
fn plan_one(dir: &Path, kind: Kind, app: Option<&str>, jits: &[(u32, String, Option<PathBuf>)]) -> CachePlan {
    let held = held_locks(dir).len();
    let users: Vec<User> = jits
        .iter()
        .filter(|(_, _, d)| d.as_ref().and_then(|d| std::fs::canonicalize(d).ok()).as_deref() == Some(dir))
        .filter(|(_, a, _)| app.is_none_or(|want| app_names(want).iter().any(|n| n == a)))
        .map(|(p, a, _)| (*p, a.clone()))
        .collect();
    let total = tree_size(dir).1;
    let (files, bytes) = match app {
        Some(a) => app_files(dir, a).iter().fold((0, 0), |t, f| (t.0 + 1, t.1 + std::fs::metadata(f).map_or(0, |m| m.len()))),
        None => CLEARED.iter().fold((0, 0), |t, n| {
            let (f, b) = tree_size(&dir.join(n));
            (t.0 + f, t.1 + b)
        }),
    };
    // for one app, a held lock is another app's run as much as its own: only its announced runs count
    let held = if app.is_some() { 0 } else { held };
    CachePlan { dir: dir.to_path_buf(), kind, files, bytes, kept_bytes: total.saturating_sub(bytes), users, held }
}

/// What a clear would remove on this machine: one plan per run dir (`app`: only its files).
pub fn plan(app: Option<&str>) -> Vec<CachePlan> {
    let jits = announced();
    let mut out: Vec<CachePlan> = caches().into_iter().map(|(d, k)| plan_one(&d, k, app, &jits)).collect();
    if app.is_some() {
        out.retain(|p| p.files > 0 || !p.users.is_empty());
    }
    out
}

#[derive(Default, Debug)]
pub struct Removal {
    pub files: u64,
    pub bytes: u64,
    pub skipped: Vec<(PathBuf, String)>,
}

impl Removal {
    fn file(&mut self, p: &Path) {
        let Ok(m) = std::fs::symlink_metadata(p) else { return };
        match std::fs::remove_file(p) {
            Ok(()) => {
                self.files += 1;
                self.bytes += if m.file_type().is_file() { m.len() } else { 0 };
            }
            Err(e) => self.skipped.push((p.to_path_buf(), e.to_string())),
        }
    }

    /// Removes everything below `d`, depth first, links removed and never followed; `keep_dirs`:
    /// the folders stay (a running process writes into them), as do the files in `keep`.
    fn tree(&mut self, d: &Path, keep_dirs: bool, keep: &[PathBuf]) {
        let entries: Vec<PathBuf> = match std::fs::read_dir(d) {
            Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
            Err(e) => {
                if std::fs::symlink_metadata(d).is_ok() {
                    self.skipped.push((d.to_path_buf(), e.to_string()));
                }
                return;
            }
        };
        for p in entries {
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_dir() => {
                    self.tree(&p, keep_dirs, keep);
                    if !keep_dirs {
                        if let Err(e) = std::fs::remove_dir(&p) {
                            if !self.skipped.iter().any(|(s, _)| s.starts_with(&p)) {
                                self.skipped.push((p, e.to_string()));
                            }
                        }
                    }
                }
                Ok(_) if keep.contains(&p) => {}
                Ok(_) => self.file(&p),
                Err(_) => {}
            }
        }
    }
}

/// The outcome of clearing one run dir.
#[derive(Debug)]
pub struct Cleared {
    pub dir: PathBuf,
    pub removal: Removal,
    /// it ran a process: only stores and startup manifests went
    pub in_use: bool,
}

/// Clears one checked run dir (`app`: only its files). The caller checked `in_use` / `force`.
fn clear_one(p: &CachePlan, app: Option<&str>) -> Cleared {
    let mut r = Removal::default();
    let in_use = p.in_use() || !held_locks(&p.dir).is_empty();
    match app {
        Some(a) => {
            for f in app_files(&p.dir, a) {
                r.file(&f);
            }
        }
        None => {
            let held = held_locks(&p.dir);
            for n in CLEARED {
                let d = p.dir.join(n);
                if std::fs::symlink_metadata(&d).is_ok_and(|m| m.file_type().is_dir()) {
                    r.tree(&d, in_use, &held);
                    if !in_use {
                        if let Err(e) = std::fs::remove_dir(&d) {
                            if !r.skipped.iter().any(|(s, _)| s.starts_with(&d)) {
                                r.skipped.push((d, e.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }
    Cleared { dir: p.dir.clone(), removal: r, in_use }
}

pub fn human(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    }
}

fn log_file() -> PathBuf {
    crate::proto::home().join("jitcache.log")
}

fn log(line: &str) {
    eprintln!("{line}");
    use std::io::Write;
    let _ = std::fs::create_dir_all(crate::proto::home());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_file()) {
        let _ = writeln!(f, "{} {line}", crate::proto::wall_ms() / 1000);
    }
}

/// `jitcache plan|clear [app=<package>] [dir=<run dir>] [force]`: the `jitcache` service of a hub (its own
/// machine's caches). Plan lines are [`CachePlan`] JSON; a clear answers one line per cache and a
/// summary `{"freed":…}`, and publishes the summary as a hub record.
pub fn service(machine: &str, line: &str, out: &mut Vec<String>, publish: &dyn Fn(crate::proto::Record)) {
    let mut words = line.split_whitespace().skip_while(|w| *w == "jitcache");
    let verb = words.next().unwrap_or("plan");
    let (mut app, mut force, mut only) = (None, false, None);
    for w in words {
        match w {
            "force" => force = true,
            w if w.starts_with("dir=") && w.len() > 4 => match checked(Path::new(&w[4..])) {
                Ok(d) => only = Some(d),
                Err(e) => return out.push(format!("{{\"err\":{}}}", json_str(&format!("jitcache: {e}")))),
            },
            w if w.starts_with("app=") && w.len() > 4 => app = Some(w[4..].to_string()),
            _ => return out.push(format!("{{\"err\":{}}}", json_str(&format!("jitcache: unknown word `{}`", crate::proto::clean(w, 64))))),
        }
    }
    if let Some(a) = &app {
        if a.len() > 128 || !a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return out.push(format!("{{\"err\":{}}}", json_str("jitcache: app= is a package name")));
        }
    }
    let mut plans = plan(app.as_deref());
    if let Some((d, kind)) = &only {
        plans.retain(|p| p.dir == *d);
        // a run dir named outright counts even when nothing announced it (no JIT runs on it now)
        if plans.is_empty() {
            plans.push(plan_one(d, kind.clone(), app.as_deref(), &announced()));
        }
    }
    match verb {
        "plan" => {
            out.extend(plans.iter().map(CachePlan::json));
            let bytes: u64 = plans.iter().map(|p| p.bytes).sum();
            let dev = std::fs::canonicalize(dev_runs()).ok();
            let dev_bytes: u64 = plans.iter().filter(|p| dev.is_some() && p.dir.parent() == dev.as_deref()).map(|p| p.bytes + p.kept_bytes).sum();
            out.push(format!("{{\"total_bytes\":{bytes},\"caches\":{},\"in_use\":{},\"dev_bytes\":{dev_bytes}}}", plans.len(), plans.iter().any(CachePlan::in_use)));
        }
        "clear" => {
            let busy: Vec<&CachePlan> = plans.iter().filter(|p| p.in_use()).collect();
            if !busy.is_empty() && !force {
                out.extend(plans.iter().map(CachePlan::json));
                return out.push(format!("{{\"err\":{}}}", json_str(&format!("in use by {}: pass force to clear it under them (they keep running)", who(&busy)))));
            }
            let scope = app.as_deref().map_or("all".to_string(), |a| format!("app {a}"));
            let (mut freed, mut files, mut skipped) = (0u64, 0u64, 0usize);
            for p in &plans {
                let c = clear_one(p, app.as_deref());
                freed += c.removal.bytes;
                files += c.removal.files;
                skipped += c.removal.skipped.len();
                log(&format!("devhub jitcache: {}: removed {} files, {} from cache/ and startup/ ({scope}{})", c.dir.display(), c.removal.files, human(c.removal.bytes), if c.in_use { "; in use: folders and live locks kept" } else { "" }));
                for (s, e) in c.removal.skipped.iter().take(20) {
                    log(&format!("devhub jitcache:   skipped {} ({e})", s.display()));
                }
                out.push(format!(
                    "{{\"dir\":{},\"removed_files\":{},\"freed\":{},\"skipped\":{},\"in_use\":{}}}",
                    json_str(&c.dir.display().to_string()),
                    c.removal.files,
                    c.removal.bytes,
                    c.removal.skipped.len(),
                    c.in_use
                ));
            }
            let summary = format!(
                "jitcache: cleared {} cache{} ({scope}), freed {} in {files} files{}; the next start{} cold{}",
                plans.len(),
                if plans.len() == 1 { "" } else { "s" },
                human(freed),
                if skipped > 0 { format!(", {skipped} skipped (in use)") } else { String::new() },
                if app.is_some() { " of that app is" } else { " of every app here is" },
                if busy.is_empty() { String::new() } else { format!("; {} kept running", who(&busy)) }
            );
            log(&format!("devhub {summary}"));
            publish(crate::proto::hub_record(machine, app.as_deref().unwrap_or(""), 0, "jitcache", crate::proto::Sev::Info, summary.clone(), String::new()));
            out.push(format!("{{\"freed\":{freed},\"files\":{files},\"skipped\":{skipped},\"caches\":{},\"line\":{}}}", plans.len(), json_str(&summary)));
        }
        v => out.push(format!("{{\"err\":{}}}", json_str(&format!("jitcache {}: plan or clear", crate::proto::clean(v, 32))))),
    }
}

/// The processes on the busy caches, for a line: `foo#4412, makepad-app-x#981`.
fn who(busy: &[&CachePlan]) -> String {
    let mut names: Vec<String> = busy.iter().flat_map(|p| p.users.iter().map(|(pid, a)| format!("{a}#{pid}"))).collect();
    names.dedup();
    // a held lock with no announced JIT: a `makepad check` / `test`, or a run that does not
    // announce its run dir
    if busy.iter().any(|p| p.held > 0 && p.users.is_empty()) {
        names.push("a makepad run/check/test process".into());
    }
    if names.is_empty() { "a running process".into() } else { names.join(", ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deployment's move keeps the current and the previous build, what a run used since the
    /// move before it, what a run holds and a named timed series' last build; every older build goes.
    #[test]
    fn a_deployment_retires_the_builds_older_than_the_previous_one() {
        let b = |build: u64, used: u64, running: bool| BuildStores { build, files: 1, bytes: 1, used, running };
        let since = 1_000;
        let none = std::collections::HashSet::new();
        let builds = [b(1, 1_500, false), b(2, 1_200, false), b(3, 1_100, false), b(4, 900, true), b(5, 800, false), b(6, 100, false)];
        assert_eq!(superseded(&builds, since, &none), vec![5, 6], "kept: the two newest, one used since the last move, a running one");
        assert_eq!(superseded(&builds, since, &[6].into()), vec![5], "a timed series' last build stays between its runs");
        assert_eq!(superseded(&builds[..2], u64::MAX, &none), Vec::<u64>::new(), "the current and the previous build stay however long unused");
        assert_eq!(superseded(&[b(1, 10, false), b(2, 5, false), b(3, 1, false)], since, &none), vec![3], "an idle box keeps its two newest builds");
        // the tree runs' relax dirs too: the work tree's target, and the deployment's on a box
        // that is no person's own
        let d = std::env::temp_dir().join(format!("devhub-tree-runs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (work, checkout) = (d.join("devhub"), d.join("makepad"));
        let (ci, deploy) = (work.join("ci-target/relax"), checkout.join("target/relax"));
        for c in [&ci, &deploy] {
            std::fs::create_dir_all(c.join("cache")).unwrap();
        }
        assert_eq!(tree_runs(&work, &checkout, false), vec![ci.clone(), deploy.clone()]);
        assert_eq!(tree_runs(&work, &checkout, true), vec![ci.clone()], "a person's own checkout keeps its JIT's stores");
        assert_eq!(tree_runs(&d.join("none"), &d.join("none"), false), Vec::<PathBuf>::new());
        let now = crate::proto::wall_ms() / 1000;
        let store = |name: &str, build: u64, ago: u64| {
            let mut h = vec![0u8; header::HEADER];
            h[..4].copy_from_slice(header::MAGIC);
            h[4..8].copy_from_slice(&header::FORMAT.to_le_bytes());
            h[header::BUILD_AT..header::BUILD_AT + 8].copy_from_slice(&build.to_le_bytes());
            let p = ci.join("cache").join(name);
            std::fs::write(&p, h).unwrap();
            std::fs::File::options().write(true).open(&p).unwrap().set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(now - ago)).unwrap();
            p
        };
        let (old, prev, cur) = (store("serde-00000000000000a1.rcs", 0xa1, 3 * 3600), store("serde-00000000000000b2.rcs", 0xb2, 2 * 3600), store("serde-00000000000000c3.rcs", 0xc3, 60));
        assert_eq!(retire_superseded(tree_runs(&work, &checkout, false).into_iter(), now - 600, &none).0, 1, "the work target's oldest build goes");
        assert!(!old.exists() && prev.exists() && cur.exists(), "the current and the previous build stay");
        let _ = std::fs::remove_dir_all(&d);
    }
}
