//! The disk guard: what occupies a node's work volume per category, and automatic reclaim when
//! free space drops below the floor (DESIGN.md §10.13).
//!
//! Categories and the paths they are (exact, computed paths; listing a folder is the only
//! wildcard): worktrees `<work>/wt/*`; deployment target `<checkout>/target`, work target
//! `<work>/ci-target`, retired tree `<work>/tree` (including its target); other targets
//! `<work>/target/*` + legacy `~/rc/pool/*/target`;
//! native trees `<work>/native/<lane>` and their `<work>/native-target/<lane>` are one
//! reclaimable item per lane, ordered by last lease use, never reclaimed while held.
//! Dev caches `<work>/dev/target`, `<work>/dev/run-home/*` and the lanes' own JIT builds
//! `<work>/dev/lane-jit/*` are lease-owned and reclaimable when idle. Dev git stores, overlays, committed tree and JIT binaries have their own owners.
//! jit-cache: the JIT's run dirs `target/*` and `code` of the run home and of the dev run home
//! (`<work>/dev/run-home`), a run dir no run uses whole, the others by Relax build, never a
//! running build's ([`jit_items`]), + `<run home>/cache` + `flightrec`; weights
//! `~/.makepad/weights`; desk-link `~/.makepad/wm/desk-link`; screencaps
//! `~/makepad/local/screencap`; logs `~/.makepad/{devhub,wm}/*.log` and their rotations
//! `*.log.<n>`; homes-tmp `<work>/tmp/*` of ended leases + `/tmp/landgate.*`; job inputs `<work>/inputs/*`
//! (lease/ship.rs [`super::ship::Input`]); retired run dirs: the JIT's run dirs under their name
//! before the 2026-11-12 relax rename (`<home>/<OLD_HOME>` and `<OLD>` in the targets above), listed
//! but never in a reclaim plan: [`Guard::check`] removes them when a work node is idle and no
//! process holds them, never on a user machine. Delete the category once no node has one.
//!
//! Reclaim order (stops at floor + 10 GB): 1 targets, native trees and job inputs no lease or process uses (LRU), 2 JIT
//! run dirs no run uses and the stores of builds that are not a run dir's newest (LRU), 3 ended
//! leases' tmp + orphaned gate dirs, 4 rotated logs, 5 (work nodes only) a run dir's newest build's
//! stores, 6 (work nodes only, no lease on the box) the target of
//! deployment and work trees. Never automatic: retired trees, worktrees (lease end owns them,
//! with recovery), weights, Desk Link history, screencaps and other user data; on a user machine
//! still short after reclaim, the user data is a `disk_ask` (inbox).

use super::caps::{self, GB};
use crate::local::Publish;
use crate::proto::{hub_record, Sev};
use makepad_remote::json_str;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The run dir's name and the run home's before the 2026-11-12 relax rename (written in two
/// parts so that the rename's script, which rewrites the old names, leaves them).
const OLD: &str = concat!("makepad", "-run");
const OLD_HOME: &str = concat!(".makepad", "-run");

/// How long a measurement stays good unless something changed (a lease started or ended).
pub const MEASURE_EVERY: Duration = Duration::from_secs(600);
/// Reclaim stops this far above the floor.
pub const MARGIN: u64 = 10 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Worktrees,
    Native,
    Targets,
    DeployTarget,
    WorkTarget,
    RetiredTree,
    RetiredRunDir,
    JitCache,
    Weights,
    DeskLink,
    Screencaps,
    Logs,
    HomesTmp,
    Inputs,
}

pub const CATS: [Cat; 14] = [Cat::Worktrees, Cat::Native, Cat::Targets, Cat::DeployTarget, Cat::WorkTarget, Cat::RetiredTree, Cat::RetiredRunDir, Cat::JitCache, Cat::Weights, Cat::DeskLink, Cat::Screencaps, Cat::Logs, Cat::HomesTmp, Cat::Inputs];

impl Cat {
    pub fn as_str(self) -> &'static str {
        match self {
            Cat::Worktrees => "worktrees",
            Cat::Native => "native trees",
            Cat::Targets => "targets",
            Cat::DeployTarget => "deployment target",
            Cat::WorkTarget => "ci-target",
            Cat::RetiredTree => "retired tree",
            Cat::RetiredRunDir => "retired run dir",
            Cat::JitCache => "jit-cache",
            Cat::Weights => "weights",
            Cat::DeskLink => "desk-link",
            Cat::Screencaps => "screencaps",
            Cat::Logs => "logs",
            Cat::HomesTmp => "homes-tmp",
            Cat::Inputs => "job inputs",
        }
    }
}

/// One measured path.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub cat: Cat,
    pub path: PathBuf,
    pub bytes: u64,
    /// newest mtime inside, unix seconds (LRU order)
    pub mtime: u64,
    /// the reclaim step that may take it automatically; None: kept (see `note`)
    pub step: Option<u8>,
    /// the user's own data: asked about on user machines, never deleted
    pub user_data: bool,
    pub note: String,
    /// a JIT run dir's stores of this Relax build alone (`path` is the run dir): retired under
    /// the build's live lock ([`crate::jitcache::retire`]), never the dir
    pub build: Option<u64>,
}

/// What is in use right now: live leases' paths and every process's cwd.
#[derive(Clone, Debug, Default)]
pub struct Live {
    /// (path, lease id)
    pub leases: Vec<(PathBuf, String)>,
    pub cwds: Vec<(u32, PathBuf)>,
}

impl Live {
    /// Why `p` must stay, if something uses it.
    pub fn user_of(&self, p: &Path) -> Option<String> {
        if let Some((_, id)) = self.leases.iter().find(|(l, _)| l.starts_with(p) || p.starts_with(l)) {
            return Some(format!("lease {id}"));
        }
        self.cwds.iter().find(|(_, c)| c.starts_with(p)).map(|(pid, _)| format!("cwd of pid {pid}"))
    }
}

/// The reclaim plan: what goes (in order), whether that reaches the floor, what to ask about,
/// and the reclaimable rest in the same order (`more`: what a reclaim goes on to when an item of
/// `take` is kept, [`walk`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub take: Vec<Item>,
    pub reaches: bool,
    pub ask: Vec<Item>,
    pub more: Vec<Item>,
}

/// Pure planner: below `floor`, take reclaimable items by step then LRU until `floor + MARGIN`
/// is free; step 5 (the current JIT cache) only on work nodes.
pub fn plan(items: &[Item], free: u64, floor: u64, user_machine: bool) -> Plan {
    if free >= floor {
        return Plan { reaches: true, ..Default::default() };
    }
    let mut c: Vec<&Item> = items.iter().filter(|i| i.step.is_some_and(|s| s < 5 || !user_machine)).collect();
    c.sort_by_key(|i| (i.step, i.mtime));
    let mut p = Plan::default();
    let mut got = free;
    for i in c {
        if got >= floor + MARGIN {
            p.more.push(i.clone());
            continue;
        }
        got += i.bytes;
        p.take.push(i.clone());
    }
    p.reaches = got >= floor;
    if !p.reaches && user_machine {
        p.ask = items.iter().filter(|i| i.user_data && i.bytes > 0).cloned().collect();
    }
    p
}

/// Walks `order` (a plan's take, then the rest of its candidates) until `goal + MARGIN` is free,
/// counting only what `take` frees: an item it keeps (in use, its cache lock held, a failed
/// removal) frees nothing and the walk goes on to the next one. Counted as freed, a kept item
/// ended the walk short with nothing freed and no word: a box's prebuild headed its queue 35 min at
/// 36 GB free against a 25 GB floor and a need of 20, with 151 GB of targets reclaimable
/// (L1377857, 11-18).
pub fn walk<'a>(order: impl IntoIterator<Item = &'a Item>, free: u64, goal: u64, take: &mut dyn FnMut(&'a Item) -> bool) -> u64 {
    let mut freed = 0;
    for i in order {
        if free + freed >= goal + MARGIN {
            break;
        }
        if take(i) {
            freed += i.bytes;
        }
    }
    freed
}

/// Bytes on disk (allocated blocks) under `p` and its newest mtime; symlinks are not followed.
pub fn dir_size(p: &Path) -> (u64, u64) {
    let Ok(m) = std::fs::symlink_metadata(p) else { return (0, 0) };
    let (mut bytes, mut newest) = crate::os::on_disk(&m);
    if m.is_dir() {
        let mut stack = vec![p.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let Ok(m) = e.metadata() else { continue };
                let (b, t) = crate::os::on_disk(&m);
                bytes += b;
                newest = newest.max(t);
                if m.is_dir() {
                    stack.push(e.path());
                }
            }
        }
    }
    (bytes, newest)
}

/// Where a node keeps things: its home, lease work dir, JIT home, temp dir.
#[derive(Clone, Debug)]
pub struct Roots {
    pub home: PathBuf,
    pub work: PathBuf,
    pub run_home: PathBuf,
    pub tmp: PathBuf,
    /// the deployment checkout, whose target holds the apps' build output and JIT cache
    pub deploy: Option<PathBuf>,
}

impl Roots {
    pub fn of(work: &Path) -> Roots {
        let home = caps::home_dir();
        let run_home = std::env::var_os("RELAX_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".relax"));
        #[cfg(feature = "checkout")]
        let deploy = Some(crate::sync::SyncConf::load().checkout).filter(|p| super::tree::ours(p));
        #[cfg(not(feature = "checkout"))]
        let deploy = None;
        Roots { home, work: work.to_path_buf(), run_home, tmp: std::env::temp_dir(), deploy }
    }
}

/// A running overlay protects dev caches across process and disk-guard races.
pub fn dev_cache_hold(work: &Path, active: &dyn Fn() -> Result<(), String>) -> Result<std::fs::File, String> {
    super::jitstore::lock_while(&work.join("dev/cache.lock"), true, active, &|| {})
}

/// Where lane `lane`'s own JIT tree (`dev/lane-jit/<lane>`) has its lock: beside the tree, not in
/// it, so a reclaim that deletes the tree holds it to the end.
pub fn lane_jit_lock(dev: &Path, lane: &str) -> PathBuf {
    dev.join("lane-jit.locks").join(lane)
}

/// A lease's build in its lane's own JIT tree holds the lane's lock shared, so a reclaim never
/// deletes the tree under it ([`item_lock`]); behind a reclaim deleting it (`waiting`) it starts
/// once the tree is gone, and builds it cold.
pub fn lane_jit_hold(dev: &Path, lane: &str, active: &dyn Fn() -> Result<(), String>, waiting: &dyn Fn()) -> Result<std::fs::File, String> {
    super::jitstore::lock_while(&lane_jit_lock(dev, lane), true, active, waiting)
}

/// The lock a reclaim holds while it deletes `i` under the dev dir `dev`, or why it keeps it. The
/// dev target and run home stay under the dev cache lock, which every running overlay lease holds
/// shared ([`dev_cache_hold`]); a build's stores in a run dir under that build's live lock alone
/// ([`crate::jitcache::retire`], not here). A lane's own JIT tree is under that lane's lock alone, which its
/// build holds shared ([`lane_jit_hold`]): another lane's session no longer keeps it. Under the one
/// dev lock, a box with a live session kept every lane's tree, and a box's prebuild waited 35 min
/// for disk beside 151 GB of targets (L1377857, 11-18).
pub fn item_lock(dev: &Path, i: &Item) -> Result<Option<std::fs::File>, String> {
    let lane_jit = dev.join("lane-jit");
    let (path, why) = if i.path.parent() == Some(lane_jit.as_path()) {
        let lane = name(&i.path);
        (lane_jit_lock(dev, &lane), format!("a lease of lane {lane} builds its JIT in it"))
    } else if i.path.starts_with(dev.join("target")) || i.path.starts_with(dev.join("run-home")) || i.path.starts_with(&lane_jit) {
        (dev.join("cache.lock"), "dev cache in use (a running overlay lease holds its lock)".to_string())
    } else {
        return Ok(None);
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(_) => Err(why),
    }
}

fn children(d: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).collect();
    v.sort();
    v
}

fn name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn rotated_log(n: &str) -> bool {
    n.rsplit_once(".log.").is_some_and(|(_, r)| !r.is_empty() && r.split('.').next().is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit())))
}

/// Lists and measures every category (a walk: run it niced, off the request path).
/// The retired run dirs that exist (see the module doc).
fn retired_run_dirs(r: &Roots) -> Vec<PathBuf> {
    let mut targets = vec![r.work.join("ci-target"), r.work.join("tree/target"), r.work.join("dev/target")];
    targets.extend(r.deploy.iter().map(|d| d.join("target")));
    targets.extend(children(&r.work.join("target")));
    targets.extend(children(&r.work.join("native-target")));
    targets.extend(children(&r.work.join("dev/lane-jit")).into_iter().map(|p| p.join("target")));
    let mut out: Vec<PathBuf> = targets.into_iter().map(|t| t.join(OLD)).collect();
    out.push(r.home.join(OLD_HOME));
    out.retain(|p| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()));
    out
}

/// Who still uses retired run dir `dir`: a lease, or a running JIT process of the old name
/// (it holds a live lock in the run dir, or for the run home in one of its `target/*`).
fn retired_user(r: &Roots, dir: &Path, live: &Live) -> Option<String> {
    let mut dirs = vec![dir.to_path_buf()];
    if dir == r.home.join(OLD_HOME) {
        dirs.extend(children(&dir.join("target")));
    }
    live.user_of(dir).or_else(|| dirs.iter().any(|d| !crate::jitcache::held_locks(d).is_empty()).then(|| "a running process holds its live lock".into()))
}

pub fn list(r: &Roots, live: &Live) -> Vec<Item> {
    let mut out = Vec::new();
    let mut add = |cat: Cat, path: PathBuf, step: Option<u8>, user_data: bool, note: &str| {
        if !path.exists() {
            return;
        }
        let (bytes, mtime) = dir_size(&path);
        let (step, note) = match (step, live.user_of(&path)) {
            (Some(_), Some(u)) => (None, format!("in use: {u}")),
            (s, _) => (s, note.to_string()),
        };
        out.push(Item { cat, path, bytes, mtime, step, user_data, note, build: None });
    };
    for p in children(&r.work.join("wt")) {
        add(Cat::Worktrees, p, None, false, "lease worktree: its lease end cleans it");
    }
    for p in children(&r.work.join("target")).into_iter().filter(|p| p.is_dir()) {
        add(Cat::Targets, p, Some(1), false, "");
    }
    let dev = r.work.join("dev");
    add(Cat::Targets, dev.join("target"), Some(1), false, "dev Cargo target");
    for p in children(&dev.join("lane-jit")) { add(Cat::Targets, p, Some(1), false, "a lane's own JIT build tree and target"); }
    // (its run dirs, `target/*` and `code`, by build: jit_items)
    for p in children(&dev.join("run-home")).into_iter().filter(|p| !matches!(name(p).as_str(), "target" | "code")) {
        add(Cat::JitCache, p, Some(2), false, "dev run home");
    }
    for p in [dev.join("git"), dev.join("jit"), dev.join("tree")] { add(Cat::Worktrees, p, None, false, "dev store: its owner reclaims it"); }
    for p in children(&dev.join("overlay")).into_iter().chain(children(&dev.join("lane"))) { add(Cat::Worktrees, p, None, false, "lane state: explicit lane cleanup owns it"); }
    for p in children(&dev.join("tmp")) { add(Cat::HomesTmp, p, Some(3), false, "tmp of an ended overlay lease"); }
    for p in children(&r.work.join("inputs")) { add(Cat::Inputs, p, Some(1), false, "a job input: the next lease that names it ships it again"); }
    for p in children(&r.home.join("rc/pool")) {
        add(Cat::Targets, p.join("target"), Some(1), false, "legacy pool target");
    }
    // warm targets go last, only on work nodes without a lease
    let step = live.leases.is_empty().then_some(6);
    if let Some(target) = r.deploy.as_ref().map(|t| t.join("target")) {
        add(Cat::DeployTarget, target, step, false, "deployment target: the next app build is cold");
    }
    add(Cat::WorkTarget, r.work.join("ci-target"), step, false, "work tree target: the next lease build is cold");
    add(Cat::RetiredTree, r.work.join("tree"), None, false, "reclaimable retired tree including its target; manual removal only");
    for p in retired_run_dirs(r) {
        let note = retired_user(r, &p, live).map_or_else(|| "the run dir under its name before the relax rename: removed when the node is idle".into(), |u| format!("in use: {u}"));
        add(Cat::RetiredRunDir, p, None, false, &note);
    }
    add(Cat::JitCache, r.run_home.join("cache"), None, false, "source stores: imports stay stored");
    add(Cat::JitCache, r.run_home.join("flightrec"), None, false, "bounded flight recordings");
    add(Cat::Weights, r.home.join(".makepad/weights"), None, false, "served weights: the AI hub owns them");
    add(Cat::DeskLink, r.home.join(".makepad/wm/desk-link"), None, true, "Desk Link history");
    add(Cat::Screencaps, r.home.join("makepad/local/screencap"), None, true, "the user's recordings");
    for d in [r.home.join(".makepad/devhub"), r.home.join(".makepad/wm")] {
        for p in children(&d) {
            let n = name(&p);
            if rotated_log(&n) {
                add(Cat::Logs, p, Some(4), false, "rotated log");
            } else if n.ends_with(".log") {
                add(Cat::Logs, p, None, false, "current log");
            }
        }
    }
    for p in children(&r.work.join("tmp")) {
        add(Cat::HomesTmp, p, Some(3), false, "tmp of an ended lease");
    }
    for p in children(&r.tmp).into_iter().filter(|p| name(p).starts_with("landgate.")) {
        add(Cat::HomesTmp, p, Some(3), false, "orphaned gate dir");
    }
    jit_items(&mut out, &dev.join("run-home"), live);
    jit_items(&mut out, &r.run_home, live);
    #[cfg(feature = "checkout")]
    {
        let mut lanes: std::collections::BTreeMap<_, _> = super::tree::natives(&r.work).into_iter().collect();
        for p in children(&r.work.join("native-target")) {
            if p.is_dir() && super::exec::valid_name(&name(&p)) { lanes.entry(name(&p)).or_insert(0); }
        }
        for (lane, mtime) in lanes {
            let path = super::tree::native_root(&r.work, &lane);
            let target = super::tree::native_target(&r.work, &lane);
            let bytes = dir_size(&path).0 + dir_size(&target).0;
            let used = live.user_of(&path).or_else(|| live.user_of(&target)).or_else(|| super::tree::held(&path).then(|| "tree hold".into()));
            out.push(Item { cat: Cat::Native, path, bytes, mtime: mtime / 1000, step: used.is_none().then_some(1), user_data: false, note: used.map_or_else(|| "native tree and target".into(), |u| format!("in use: {u}")), build: None });
        }
    }
    out
}

/// The JIT's run dirs in run home `home` (relax dirs.rs: `target/<8 hex>` per virtual workspace,
/// `code` the shared code folder). A run dir no run uses that is not the newest is one item
/// (step 2). In the others each Relax build's stores are an item of their own, the least
/// recently used first ([`crate::jitcache::builds`]): a running build's never (its live lock), the
/// dir's newest build last (step 5, work nodes only), every other build's at step 2. One run dir
/// serves every overlay lease of a box (the dev run home's workspace is the dev store's), and it
/// holds the stores of every JIT build that ran there for a day: kept whole while any overlay
/// lease ran, a box's prebuild waited 30 min for disk beside it, then taken whole the moment the
/// box had none for a lease 5 GB short, and every lease there started cold (L1389114, 11-18).
/// What else a dir holds (build script and native outputs, startup manifests) the JIT keeps by
/// use: listed, never taken.
fn jit_items(out: &mut Vec<Item>, home: &Path, live: &Live) {
    let dirs = children(&home.join("target"));
    let current = dirs.iter().max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).cloned();
    for p in dirs.into_iter().chain([home.join("code")]) {
        if !p.is_dir() {
            continue;
        }
        let (bytes, mtime) = dir_size(&p);
        let held = !crate::jitcache::held_locks(&p).is_empty();
        if !held && current.as_ref() != Some(&p) && p != home.join("code") {
            let (step, note) = match live.user_of(&p) {
                Some(u) => (None, format!("in use: {u}")),
                None => (Some(2), "a JIT run dir no run uses".to_string()),
            };
            out.push(Item { cat: Cat::JitCache, path: p, bytes, mtime, step, user_data: false, note, build: None });
            continue;
        }
        let builds = crate::jitcache::builds(&p);
        let newest = builds.first().map(|b| b.build);
        let stores: u64 = builds.iter().map(|b| b.bytes).sum();
        for b in builds {
            let (step, note) = if b.running {
                (None, format!("in use: a run of build {:016x} holds its live lock", b.build))
            } else if Some(b.build) == newest {
                (Some(5), format!("build {:016x}: the run dir's newest build, {} stores", b.build, b.files))
            } else {
                (Some(2), format!("build {:016x}: another build's {} stores", b.build, b.files))
            };
            out.push(Item { cat: Cat::JitCache, path: p.clone(), bytes: b.bytes, mtime: b.used, step, user_data: false, note, build: Some(b.build) });
        }
        out.push(Item { cat: Cat::JitCache, path: p, bytes: bytes.saturating_sub(stores), mtime, step: None, user_data: false, note: "the run dir's other files: the JIT keeps them by use".into(), build: None });
    }
}

/// The Mac's guard (master side): lane worktrees under `wt.noindex` are counted, never removed;
/// a lane's `target` goes (step 1) only when the lane is merged into work and idle.
pub fn mac_items(repo: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    for i in super::wt::infos(repo).unwrap_or_default() {
        let t = i.entry.path.join("target");
        let (bytes, mtime) = dir_size(&i.entry.path);
        out.push(Item { cat: Cat::Worktrees, path: i.entry.path.clone(), bytes, mtime, step: None, user_data: false, note: "lane worktree: wt prune owns it".into(), build: None });
        if t.is_dir() {
            let (bytes, mtime) = dir_size(&t);
            let step = (i.merged && i.live.is_empty()).then_some(1);
            let note = if step.is_some() { "landed lane" } else { "lane not landed or busy" };
            out.push(Item { cat: Cat::Targets, path: t, bytes, mtime, step, user_data: false, note: note.into(), build: None });
        }
    }
    out
}

fn gb(b: u64) -> f64 {
    b as f64 / GB
}

/// Where what goes waits for its delete (`<work>/trash`): a rename takes no time whatever it holds,
/// so no lock is held over a delete, and a hub that stops halfway leaves whole trees or none at
/// their places.
fn trash(work: &Path) -> PathBuf {
    work.join("trash")
}

/// Moves `p` to the trash (a missing one is gone already).
pub fn to_trash(work: &Path, p: &Path) -> std::io::Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if std::fs::symlink_metadata(p).is_err() {
        return Ok(());
    }
    std::fs::create_dir_all(trash(work))?;
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::fs::rename(p, trash(work).join(format!("{}-{}-{}-{n}", name(p), crate::proto::wall_ms(), std::process::id())))
}

/// Deletes what is in the trash: what it freed (what could not go stays for the next time).
pub fn empty_trash(work: &Path) -> u64 {
    let mut freed = 0;
    for p in children(&trash(work)) {
        let bytes = dir_size(&p).0;
        match remove(&p) {
            Ok(()) => freed += bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("devhub disk: {}: {e}", p.display()),
        }
    }
    freed
}

/// [`empty_trash`] on a thread of its own, at low priority: nobody waits for it.
pub fn empty_trash_soon(work: &Path) {
    let work = work.to_path_buf();
    let _ = std::thread::Builder::new().name("devhub-trash".into()).spawn(move || {
        crate::os::nice_this_thread();
        empty_trash(&work);
    });
}

/// Deletes exactly `p` (a directory tree or one file); never follows a symlink out.
pub fn remove(p: &Path) -> std::io::Result<()> {
    let m = std::fs::symlink_metadata(p)?;
    if m.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
}

/// The share of its volume the JIT keeps free, at most JIT_FREE_MAX_GB (relax-session store_floor):
/// its cache fills a disk down to that floor by design, evicting the least recently used below it.
const JIT_FREE_SHARE: f64 = relax_session::store_floor::FREE_SHARE as f64;
const JIT_FREE_MAX_GB: f64 = (relax_session::store_floor::FREE_MAX >> 30) as f64;

/// How far under the JIT's floor free space goes before a box warns.
const WARN_MARGIN_GB: f64 = 5.0;

/// Where a box of `total_gb` warns (`disk_warn`): the JIT's floor less a margin, so the warning
/// says the JIT cannot hold its floor or something else fills the disk; never under devhub's own
/// floor `floor_gb`, which reclaims. A fixed 50 GB never cleared on a 494 GB box, whose JIT then kept
/// 49 GB free by design (main, 11-18).
pub fn warn_gb(total_gb: f64, floor_gb: f64) -> f64 {
    ((total_gb / JIT_FREE_SHARE).min(JIT_FREE_MAX_GB) - WARN_MARGIN_GB).max(floor_gb)
}

struct State {
    measured: Option<(Instant, Vec<Item>)>,
    dirty: bool,
    below_floor: bool,
    below_warn: bool,
}

/// A node's guard: cached measurement, floor and warn crossings, reclaim.
pub struct Guard {
    pub machine: String,
    pub roots: Roots,
    floor: std::sync::atomic::AtomicU64,
    user_machine: std::sync::atomic::AtomicBool,
    publish: Publish,
    state: Mutex<State>,
}

impl Guard {
    pub fn new(machine: &str, roots: Roots, floor_gb: f64, user_machine: bool, publish: Publish) -> Guard {
        Guard {
            machine: machine.to_string(),
            roots,
            floor: ((floor_gb * GB) as u64).into(),
            user_machine: user_machine.into(),
            publish,
            state: Mutex::new(State { measured: None, dirty: true, below_floor: false, below_warn: false }),
        }
    }

    /// The master changed the node's lease.conf: floors and the user-machine rule follow.
    pub fn set_policy(&self, floor_gb: f64, user_machine: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        self.floor.store((floor_gb * GB) as u64, Relaxed);
        self.user_machine.store(user_machine, Relaxed);
        self.touch();
    }

    pub fn floor(&self) -> u64 {
        self.floor.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// [`warn_gb`] of the work dir's volume (unknown: devhub's floor).
    pub fn warn_at(&self) -> u64 {
        let total = caps::disk_space(&self.roots.work).map_or(0, |(_, total)| total);
        (warn_gb(gb(total), gb(self.floor())) * GB) as u64
    }

    pub fn user_machine(&self) -> bool {
        self.user_machine.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Something changed (a lease started or ended): the next look measures again.
    pub fn touch(&self) {
        self.state.lock().unwrap().dirty = true;
    }

    /// Free bytes on the work dir's filesystem. Unknown (no probe for this OS, or a failed
    /// probe) reads as unlimited: a guard that cannot measure must not reclaim. (Android had no
    /// probe; 0 read as "below the floor" and the running loader's own JIT cache was deleted
    /// every 60 s, so no launch ever started warm.)
    pub fn free(&self) -> u64 {
        caps::disk_space(&self.roots.work).map_or(u64::MAX, |(f, _)| f)
    }

    /// The cached listing, measured again when stale or changed (or `force`).
    pub fn items(&self, live: &Live, force: bool) -> Vec<Item> {
        {
            let st = self.state.lock().unwrap();
            if let (false, false, Some((at, items))) = (force, st.dirty, &st.measured) {
                if at.elapsed() < MEASURE_EVERY {
                    return items.clone();
                }
            }
        }
        let items = list(&self.roots, live);
        let mut st = self.state.lock().unwrap();
        st.measured = Some((Instant::now(), items.clone()));
        st.dirty = false;
        items
    }

    fn emit(&self, sev: Sev, line: String) {
        (self.publish)(hub_record(&self.machine, "", 0, "disk", sev, line, String::new()));
    }

    /// The periodic look: floor / warn crossings, and automatic reclaim below the floor; `quiet`
    /// (a timed lease runs) skips the walk that refreshes what the roots hold.
    pub fn check(&self, live: &dyn Fn() -> Live, quiet: bool) {
        let (free, warn) = (self.free(), self.warn_at());
        let (floor_cross, warn_cross) = {
            let mut st = self.state.lock().unwrap();
            let f = free < self.floor() && !st.below_floor;
            let w = free < warn && !st.below_warn;
            st.below_floor = free < self.floor();
            st.below_warn = free < warn;
            (f, w)
        };
        if warn_cross && !floor_cross {
            self.emit(Sev::Warn, format!("disk_warn: {} has {:.0} GB free, {:.0} GB under the {:.0} GB its JIT keeps free (1/{JIT_FREE_SHARE} of the disk, at most {JIT_FREE_MAX_GB} GB): the JIT cannot hold its floor, or something else fills the disk",
                self.machine, gb(free), gb(warn) + WARN_MARGIN_GB - gb(free), gb(warn) + WARN_MARGIN_GB));
        }
        if free < self.floor() {
            if floor_cross {
                self.emit(Sev::Warn, format!("disk_floor: {} has {:.0} GB free, floor {:.0} GB: reclaiming", self.machine, gb(free), gb(self.floor())));
            }
            self.reclaim(&live(), false, 0);
        } else if !quiet {
            let _ = self.items(&live(), false);
        }
        if !quiet && !self.user_machine() {
            self.remove_retired_run_dirs(live);
        }
    }

    /// Removes the retired run dirs while the node holds no lease and no process uses them.
    fn remove_retired_run_dirs(&self, live: &dyn Fn() -> Live) {
        for p in retired_run_dirs(&self.roots) {
            let now = live();
            if !now.leases.is_empty() {
                return;
            }
            if retired_user(&self.roots, &p, &now).is_some() {
                continue;
            }
            let bytes = dir_size(&p).0;
            match remove(&p) {
                Ok(()) => {
                    self.emit(Sev::Info, format!("disk: {} removed the retired run dir {} ({:.1} GB)", self.machine, p.display(), gb(bytes)));
                    self.touch();
                }
                Err(e) => eprintln!("devhub disk: {}: {e}", p.display()),
            }
        }
    }

    /// Reclaims (or with `dry`, plans) until `need` bytes are free over the floor (a waiting
    /// lease's cost; 0: the floor itself), oldest and most reclaimable first ([`plan`]), past
    /// what it must keep ([`walk`]); answer lines: the outcome first, then the categories, then
    /// each item taken or kept. A dry run keeps what a reclaim would keep now.
    pub fn reclaim(&self, live: &Live, dry: bool, need: u64) -> Vec<String> {
        let items = self.items(live, !dry);
        let free = self.free();
        let goal = self.floor() + need;
        let p = plan(&items, free, goal, self.user_machine());
        let dev = self.roots.work.join("dev");
        let (mut lines, mut kept) = (Vec::new(), Vec::<(String, u64)>::new());
        let freed = walk(p.take.iter().chain(&p.more), free, goal, &mut |i| {
            let why = format!("step {} {}", i.step.unwrap_or(0), if i.note.is_empty() { "LRU" } else { &i.note });
            let keep = |lines: &mut Vec<String>, kept: &mut Vec<(String, u64)>, u: String| {
                lines.push(format!("{{\"kept\":{},\"gb\":{:.2},\"why\":{}}}", json_str(&i.path.to_string_lossy()), gb(i.bytes), json_str(&u)));
                kept.push((u, i.bytes));
                false
            };
            // looked at again at the moment of deletion: a lease may have started using it (a
            // build's stores: a run of it, which holds its live lock; retire refuses then)
            if let Some(u) = live.user_of(&i.path).filter(|_| i.build.is_none()) {
                return keep(&mut lines, &mut kept, u);
            }
            let _lock = match i.build.map_or_else(|| item_lock(&dev, i), |_| Ok(None)) {
                Ok(lock) => lock,
                Err(why) => return keep(&mut lines, &mut kept, why),
            };
            let build = i.build.map_or(String::new(), |b| format!(",\"build\":\"{b:016x}\""));
            if dry {
                lines.push(format!("{{\"reclaim\":{},\"cat\":\"{}\"{build},\"gb\":{:.2},\"why\":{}}}", json_str(&i.path.to_string_lossy()), i.cat.as_str(), gb(i.bytes), json_str(&why)));
                return true;
            }
            #[cfg(feature = "checkout")]
            let removed = if let Some(b) = i.build {
                crate::jitcache::retire(&i.path, b).map(|_| ())
            } else if i.cat == Cat::Native {
                let target = super::tree::native_target(&self.roots.work, &name(&i.path));
                if let Some(u) = live.user_of(&target) {
                    return keep(&mut lines, &mut kept, u);
                }
                super::tree::remove_native(&self.roots.work, &name(&i.path)).map(|r| {
                    for b in r.bundles { self.emit(Sev::Warn, format!("ws_recovered: native tree {} bundled to {}", i.path.display(), b.display())); }
                    empty_trash(&self.roots.work);
                })
            } else { remove(&i.path).map_err(|e| e.to_string()) };
            #[cfg(not(feature = "checkout"))]
            let removed = match i.build {
                Some(b) => crate::jitcache::retire(&i.path, b).map(|_| ()),
                None => remove(&i.path).map_err(|e| e.to_string()),
            };
            match removed {
                Ok(()) => {
                    lines.push(format!("{{\"reclaimed\":{},\"cat\":\"{}\"{build},\"gb\":{:.2}}}", json_str(&i.path.to_string_lossy()), i.cat.as_str(), gb(i.bytes)));
                    let line = format!("disk_reclaim: {} freed {} MB of {} on {} ({why})", i.path.display(), i.bytes >> 20, i.cat.as_str(), self.machine);
                    // in the box's own log too: the master keeps no info records, and what a
                    // reclaim took was then known nowhere (a box's JIT stores, 11-18)
                    eprintln!("devhub {line}");
                    self.emit(Sev::Info, line);
                    true
                }
                Err(e) if i.build.is_some() => keep(&mut lines, &mut kept, e),
                Err(e) => {
                    lines.push(format!("{{\"failed\":{},\"err\":{}}}", json_str(&i.path.to_string_lossy()), json_str(&e.to_string())));
                    false
                }
            }
        });
        let reaches = free + freed >= goal;
        let kept_said = kept_said(&kept);
        if !dry && !reaches {
            self.emit(Sev::Warn, format!("disk_short: {} has {:.0} GB free and freed {:.0} GB of the {:.0} GB it needs over its {:.0} GB floor{kept_said}: what is left is not build output", self.machine, gb(free), gb(freed), gb(need), gb(self.floor())));
        }
        let mut out = vec![format!(
            "{{\"node\":{},\"free_gb\":{:.1},\"floor_gb\":{:.0},\"need_gb\":{:.0},\"warn_gb\":{:.0},\"dry\":{},\"reclaim_gb\":{:.1},\"kept_gb\":{:.1},\"reaches_floor\":{}}}",
            json_str(&self.machine),
            gb(free),
            gb(self.floor()),
            gb(need),
            gb(self.warn_at()),
            dry,
            gb(freed),
            gb(kept.iter().map(|k| k.1).sum()),
            reaches
        )];
        for c in CATS {
            let of: Vec<&Item> = items.iter().filter(|i| i.cat == c).collect();
            if of.is_empty() {
                continue;
            }
            let total: u64 = of.iter().map(|i| i.bytes).sum();
            let reclaimable: u64 = of.iter().filter(|i| i.step.is_some() || (i.cat == Cat::RetiredTree && live.user_of(&i.path).is_none())).map(|i| i.bytes).sum();
            out.push(format!("{{\"cat\":\"{}\",\"gb\":{:.2},\"n\":{},\"reclaimable_gb\":{:.2}}}", c.as_str(), gb(total), of.len(), gb(reclaimable)));
        }
        out.extend(lines);
        if !reaches && self.user_machine() {
            for i in items.iter().filter(|i| i.user_data && i.bytes > 0) {
                out.push(format!("{{\"ask\":{},\"cat\":\"{}\",\"gb\":{:.2}}}", json_str(&i.path.to_string_lossy()), i.cat.as_str(), gb(i.bytes)));
                if !dry {
                    self.emit(Sev::Warn, format!("disk_ask: {} is below its floor after reclaim; {} holds {:.1} GB of the user's {} (not deleted)", self.machine, i.path.display(), gb(i.bytes), i.cat.as_str()));
                }
            }
        }
        if !dry && freed > 0 {
            self.touch();
        }
        out
    }
}

/// `; kept N (G GB): why, ..` for what a reclaim kept, each reason once with its count.
fn kept_said(kept: &[(String, u64)]) -> String {
    if kept.is_empty() { return String::new(); }
    let mut by: Vec<(String, usize)> = Vec::new();
    for (why, _) in kept {
        // (a lease's own id differs per item: said as one reason)
        let key = if why.starts_with("lease ") { "in use by a lease".to_string() } else { why.clone() };
        match by.iter_mut().find(|(k, _)| *k == key) { Some(b) => b.1 += 1, None => by.push((key, 1)) }
    }
    let whys: Vec<String> = by.iter().map(|(k, n)| format!("{k} ({n})")).collect();
    format!("; kept {} ({:.0} GB): {}", kept.len(), gb(kept.iter().map(|k| k.1).sum()), whys.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(cat: Cat, path: &str, gbs: u64, mtime: u64, step: Option<u8>) -> Item {
        Item { cat, path: PathBuf::from(path), bytes: gbs << 30, mtime, step, user_data: false, note: String::new(), build: None }
    }

    /// A box warns under its JIT's floor less a margin, never under its own floor: the JIT filling
    /// a disk down to its floor is by design (11-18). Its floor is relax's own rule.
    #[test]
    fn a_box_warns_under_its_jits_floor() {
        assert_eq!((warn_gb(494.0, 10.0), warn_gb(2000.0, 10.0), warn_gb(120.0, 10.0)), (35.0, 35.0, 10.0));
        for gb in [120u64, 494, 2000] {
            assert_eq!(warn_gb(gb as f64, 0.0), (relax_session::store_floor::free_floor(gb << 30) >> 30) as f64 - WARN_MARGIN_GB, "{gb} GB: relax's floor less the margin");
        }
    }

    #[test]
    fn plans_by_step_then_lru_and_stops_at_floor_plus_margin() {
        let items = vec![
            item(Cat::Logs, "/l/wm.log.1", 1, 1, Some(4)),
            item(Cat::Targets, "/t/new", 20, 300, Some(1)),
            item(Cat::Targets, "/t/old", 20, 100, Some(1)),
            item(Cat::JitCache, "/j/old", 5, 50, Some(2)),
            item(Cat::JitCache, "/j/cur", 9, 900, Some(5)),
            item(Cat::Worktrees, "/w/x", 40, 1, None),
            Item { user_data: true, ..item(Cat::Screencaps, "/s", 30, 1, None) },
        ];
        let g = 1u64 << 30;
        assert_eq!(plan(&items, 30 * g, 25 * g, false), Plan { reaches: true, ..Default::default() });
        // 20 GB free, floor 25: needs 15 to reach floor + 10 -> the older target alone
        let p = plan(&items, 20 * g, 25 * g, false);
        assert_eq!(p.take.iter().map(|i| i.path.to_str().unwrap()).collect::<Vec<_>>(), vec!["/t/old"]);
        assert!(p.reaches && p.ask.is_empty());
        // 0 free: in order until floor + 10; the current JIT cache last, work nodes only
        let p = plan(&items, 0, 25 * g, false);
        assert_eq!(p.take.iter().map(|i| i.path.to_str().unwrap()).collect::<Vec<_>>(), vec!["/t/old", "/t/new"]);
        let p = plan(&items, 0, 60 * g, false);
        assert_eq!(p.take.last().unwrap().path, PathBuf::from("/j/cur"));
        assert!(!p.reaches && p.ask.is_empty());
        // a user machine keeps the current cache and asks about the user's data
        let p = plan(&items, 0, 60 * g, true);
        assert!(p.take.iter().all(|i| i.path != PathBuf::from("/j/cur")));
        assert_eq!(p.ask.len(), 1);
    }

    /// A reclaim goes past what it must keep: a kept item frees nothing, and the walk takes the
    /// next ones in order until the need over the floor, and the margin, is free. Counted as
    /// freed, a kept dev target ended a box's reclaims with nothing freed (L1377857, 11-18).
    #[test]
    fn a_reclaim_walks_past_kept_items_to_its_need_over_the_floor() {
        let g = 1u64 << 30;
        let items = vec![
            item(Cat::Targets, "/dev/target", 60, 1, Some(1)),
            item(Cat::Targets, "/t/a", 12, 2, Some(1)),
            item(Cat::Targets, "/t/b", 12, 3, Some(1)),
            item(Cat::Targets, "/t/c", 12, 4, Some(1)),
        ];
        let names = |v: &[Item]| v.iter().map(|i| i.path.to_str().unwrap().to_string()).collect::<Vec<_>>();
        // at the floor plus 11 (36 GB free, floor 25), a need of 20 aims for 45 GB free, 55 with the margin
        let (free, goal) = (36 * g, 45 * g);
        assert_eq!(plan(&items, free, 25 * g, false), Plan { reaches: true, ..Default::default() }, "the floor alone is reached");
        let p = plan(&items, free, goal, false);
        assert_eq!((names(&p.take), names(&p.more)), (vec!["/dev/target".to_string()], vec!["/t/a".to_string(), "/t/b".into(), "/t/c".into()]));
        let walked = |keep: &str| {
            let mut took = Vec::new();
            let freed = walk(p.take.iter().chain(&p.more), free, goal, &mut |i| {
                let t = i.path != PathBuf::from(keep);
                if t { took.push(i.path.to_str().unwrap().to_string()); }
                t
            });
            (took, freed)
        };
        assert_eq!(walked("/dev/target"), (vec!["/t/a".to_string(), "/t/b".into()], 24 * g), "kept: the dev target; then the oldest until 55 GB free");
        assert_eq!(walked("none"), (vec!["/dev/target".to_string()], 60 * g), "nothing kept: the plan's take alone");
    }

    /// A lane's own JIT tree is reclaimed under its lane's lock alone: a gone lane's tree goes
    /// while another lane's session holds the dev cache lock (which still keeps the dev target),
    /// a lane whose build holds its lock keeps its tree, and a build that starts while a reclaim
    /// deletes its tree waits for the delete, then finds no tree to build on, never half of one.
    #[test]
    fn a_lanes_jit_tree_is_reclaimed_under_its_lanes_lock_alone() {
        let base = std::env::temp_dir().join(format!("devhub-lane-jit-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dev = base.join("dev");
        for d in ["lane-jit/gone/tree/src", "lane-jit/building/tree", "lane-jit/starts/tree/src", "target/release"] {
            std::fs::create_dir_all(dev.join(d)).unwrap();
        }
        for f in ["lane-jit/gone/tree/src/lib.rs", "lane-jit/starts/tree/src/lib.rs"] {
            std::fs::write(dev.join(f), "x").unwrap();
        }
        let at = |p: &str| item(Cat::Targets, dev.join(p).to_str().unwrap(), 1, 1, Some(1));
        let ok = || Ok(());
        // another lane's session runs: it holds the dev cache lock shared
        let _session = dev_cache_hold(&base, &ok).unwrap();
        assert_eq!(item_lock(&dev, &at("target")).unwrap_err(), "dev cache in use (a running overlay lease holds its lock)", "the dev target stays under the shared lock");
        let gone = item_lock(&dev, &at("lane-jit/gone")).unwrap().expect("a gone lane's tree is the reclaim's beside another lane's session");
        remove(&dev.join("lane-jit/gone")).unwrap();
        drop(gone);
        assert!(!dev.join("lane-jit/gone").exists());
        // a lane whose build holds its lock keeps its tree
        let build = lane_jit_hold(&dev, "building", &ok, &|| {}).unwrap();
        assert_eq!(item_lock(&dev, &at("lane-jit/building")).unwrap_err(), "a lease of lane building builds its JIT in it");
        drop(build);
        // a build that starts mid-reclaim waits behind it and finds the tree gone: a cold start
        let reclaim = item_lock(&dev, &at("lane-jit/starts")).unwrap().unwrap();
        let (waits, waited) = std::sync::mpsc::channel();
        let (sees, seen) = std::sync::mpsc::channel();
        let starter = {
            let dev = dev.clone();
            std::thread::spawn(move || {
                let _held = lane_jit_hold(&dev, "starts", &|| Ok(()), &|| { let _ = waits.send(()); }).unwrap();
                sees.send(dev.join("lane-jit/starts").exists()).unwrap();
            })
        };
        waited.recv().expect("the build waits behind the reclaim's lock");
        remove(&dev.join("lane-jit/starts")).unwrap();
        drop(reclaim);
        assert!(!seen.recv().unwrap(), "the build sees no tree once it holds the lock: it builds one cold");
        starter.join().unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A store file of build `b` as relax writes one (relax-session store_file: magic, format, build),
    /// last used `ago` seconds back.
    fn store(path: &Path, b: u64, ago: u64) {
        let mut h = vec![0u8; 4096];
        h[..4].copy_from_slice(relax_session::store_file::MAGIC);
        h[4..8].copy_from_slice(&relax_session::store_file::FORMAT.to_le_bytes());
        let at = relax_session::store_file::BUILD_AT;
        h[at..at + 8].copy_from_slice(&b.to_le_bytes());
        std::fs::write(path, h).unwrap();
        let t = std::time::SystemTime::now() - Duration::from_secs(ago);
        std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
    }

    /// The JIT run dir every overlay lease of a box shares is reclaimed by Relax build, the least
    /// recently used first and only as far as the need goes, never a running build's and never
    /// kept whole for an overlay lease (its pin keeps the rest of the dev run home); a run dir no
    /// run uses, not the newest, is one item. A box's prebuild waited 30 min for disk beside the
    /// box's JIT stores while any session ran there, then the whole dir went for a lease 5 GB short
    /// and every lease of the box started cold (L1389114, 11-18).
    #[test]
    fn a_jit_run_dir_gives_its_least_recently_used_builds_stores_by_need_never_a_running_ones() {
        let base = std::env::temp_dir().join(format!("devhub-jit-builds-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let work = base.join("home/devhub");
        let r = Roots { home: base.join("home"), work: work.clone(), deploy: None, run_home: base.join("home/.relax"), tmp: base.join("tmp") };
        let (dir, idle) = (work.join("dev/run-home/target/bb676d1f"), work.join("dev/run-home/target/0002"));
        for d in [dir.join("cache/live"), idle.join("cache"), dir.join("build/out")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (old, mid, run) = (0xa1u64, 0xb2u64, 0xc3u64);
        store(&dir.join("cache/serde-00000000000000a1.rcs"), old, 3 * 3600);
        store(&dir.join("cache/serde-00000000000000a1.rcs.2"), old, 3 * 3600);
        store(&dir.join("cache/bitflags-00000000000000b2.rcs"), mid, 2 * 3600);
        store(&dir.join("cache/serde-00000000000000c3.rcs"), run, 60);
        store(&idle.join("cache/serde-00000000000000d4.rcs"), 0xd4, 4 * 3600);
        // a store of an older format names no build: the JIT's own prune retires it
        std::fs::write(dir.join("cache/serde-00000000000000e5.rcs"), [b"RCS1".as_slice(), &[2, 0, 0, 0], &[0xe5; 120]].concat()).unwrap();
        // what is no store stays: a file of another name, one being written
        std::fs::write(dir.join("cache/serde-00000000000000a1.rcs.tmp"), b"RCS1").unwrap();
        std::fs::write(dir.join("build/out/lib.a"), b"x").unwrap();
        // the old build's live lock, free, stamped when its last run took it
        store(&dir.join("cache/live/00000000000000a1"), 0, 3 * 3600);
        let running = std::fs::File::options().read(true).write(true).create(true).truncate(false).open(dir.join("cache/live/00000000000000c3")).unwrap();
        running.lock_shared().unwrap();
        let t = std::time::SystemTime::now() - Duration::from_secs(5 * 3600);
        std::fs::File::open(&idle).unwrap().set_modified(t).unwrap();
        // an overlay lease runs on the box: it pins the dev run home
        let live = Live { leases: vec![(work.join("dev/run-home"), "L1".into())], cwds: vec![] };
        let items = list(&r, &live);
        let of = |b: u64| items.iter().find(|i| i.build == Some(b)).unwrap_or_else(|| panic!("{b:x}: {items:?}"));
        assert_eq!((of(old).step, of(mid).step, of(run).step), (Some(2), Some(2), None), "the running build's stores stay; the others go at step 2: {items:?}");
        assert!(of(old).path == dir && of(old).bytes >= 8192 && of(old).mtime < of(mid).mtime, "a build's two store files, used before the other's: {:?}", of(old));
        assert!(of(run).note.contains("live lock") && !of(old).note.contains("L1"), "the overlay lease's pin keeps no build's stores");
        let whole = items.iter().find(|i| i.path == idle).unwrap();
        assert_eq!((whole.build, whole.step), (None, None), "a run dir no run uses is one item, and the dev run home's stays while an overlay lease runs: {whole:?}");
        assert!(items.iter().any(|i| i.path == dir && i.build.is_none() && i.step.is_none()), "the run dir's other files are listed, never taken");
        // a 1-byte need: the least recently used build alone goes
        let p = plan(&items, 0, 1, false);
        let mut took = Vec::new();
        walk(p.take.iter().chain(&p.more), MARGIN, 1, &mut |i| {
            let Some(b) = i.build else { return false };
            let freed = crate::jitcache::retire(&i.path, b).is_ok();
            if freed { took.push(b); }
            freed
        });
        assert_eq!(took, vec![old], "the oldest build's stores, no more");
        assert!(!dir.join("cache/serde-00000000000000a1.rcs").exists() && !dir.join("cache/serde-00000000000000a1.rcs.2").exists() && !dir.join("cache/live/00000000000000a1").exists(), "its store files and its live lock went");
        assert!(items.iter().all(|i| i.build.is_none_or(|b| [old, mid, run].contains(&b))), "an older format's store is no build's: {items:?}");
        for f in ["cache/bitflags-00000000000000b2.rcs", "cache/serde-00000000000000c3.rcs", "cache/serde-00000000000000a1.rcs.tmp", "cache/serde-00000000000000e5.rcs", "build/out/lib.a"] {
            assert!(dir.join(f).exists(), "{f} stays");
        }
        assert_eq!(crate::jitcache::retire(&dir, run).unwrap_err(), "a run of build 00000000000000c3 holds its live lock");
        drop(running);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lists_categories_and_keeps_what_is_in_use() {
        let base = std::env::temp_dir().join(format!("devhub-disk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let r = Roots { home: base.join("home"), work: base.join("home/devhub"), deploy: Some(base.join("home/makepad")), run_home: base.join("home/.relax"), tmp: base.join("tmp") };
        for d in ["home/makepad/target", "home/devhub/ci-target", "home/devhub/tree/target", "home/devhub/target/abc/release", "home/devhub/target/lane-x", "home/devhub/tmp/L1", "home/devhub/wt/x-L2", "home/.makepad/wm", "home/.relax/target/0001", "tmp/landgate.Q1", "home/devhub/inputs/aa/ui", "home/devhub/inputs/bb/ui"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("home/devhub/target/abc/release/bin"), vec![1u8; 100_000]).unwrap();
        std::fs::write(base.join("home/devhub/target/lane-x.last"), "abc").unwrap();
        std::fs::write(base.join("home/.makepad/wm/ai-hub.log"), "x").unwrap();
        std::fs::write(base.join("home/.makepad/wm/ai-hub.log.2"), "x").unwrap();
        let live = Live { leases: vec![(base.join("home/devhub/target/lane-x"), "L9".into()), (base.join("home/devhub/inputs/bb"), "L8".into())], cwds: vec![] };
        let items = list(&r, &live);
        let find = |p: &str| items.iter().find(|i| i.path == base.join(p)).unwrap_or_else(|| panic!("{p}: {items:?}"));
        assert_eq!(find("home/makepad/target").cat, Cat::DeployTarget);
        assert_eq!(find("home/devhub/ci-target").cat, Cat::WorkTarget);
        assert_eq!(find("home/devhub/tree").cat, Cat::RetiredTree);
        assert_eq!(find("home/devhub/tree").step, None);
        assert!(plan(&items, 0, 100 << 30, false).take.iter().all(|i| i.cat != Cat::RetiredTree));
        assert_eq!(find("home/devhub/target/abc").step, Some(1));
        assert!(find("home/devhub/target/abc").bytes >= 100_000);
        assert_eq!(find("home/devhub/target/lane-x").step, None);
        assert!(find("home/devhub/target/lane-x").note.contains("L9"));
        assert!(items.iter().all(|i| !i.path.ends_with("lane-x.last")));
        assert_eq!(find("home/.makepad/wm/ai-hub.log").step, None);
        assert_eq!(find("home/.makepad/wm/ai-hub.log.2").step, Some(4));
        assert_eq!(find("home/devhub/tmp/L1").cat, Cat::HomesTmp);
        assert_eq!(find("tmp/landgate.Q1").step, Some(3));
        assert_eq!(find("home/devhub/wt/x-L2").step, None);
        assert_eq!(find("home/.relax/target/0001").step, None, "the current run dir, with no stores: listed, never taken whole");
        // a run dir under its name before the relax rename is listed, never planned
        let old = format!("home/makepad/target/{OLD}");
        std::fs::create_dir_all(base.join(&old)).unwrap();
        let items = list(&r, &live);
        let find = |p: &str| items.iter().find(|i| i.path == base.join(p)).unwrap_or_else(|| panic!("{p}: {items:?}"));
        assert_eq!((find(&old).cat, find(&old).step), (Cat::RetiredRunDir, None));
        // a job input is a cache: reclaimed first, unless a lease reads it
        assert_eq!((find("home/devhub/inputs/aa").cat, find("home/devhub/inputs/aa").step), (Cat::Inputs, Some(1)));
        assert_eq!(find("home/devhub/inputs/bb").step, None);
        assert!(find("home/devhub/inputs/bb").note.contains("L8"));
        assert!(rotated_log("x.log.10") && rotated_log("x.log.1.gz") && !rotated_log("x.log") && !rotated_log("x.logs"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
