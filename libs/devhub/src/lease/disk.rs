//! The disk guard: what occupies a node's work volume per category, and automatic reclaim when
//! free space drops below the floor (DESIGN.md §10.13).
//!
//! Categories and the paths they are (exact, computed paths; listing a folder is the only
//! wildcard): worktrees `<work>/wt/*`; targets `<work>/target/*` + legacy `~/rc/pool/*/target`;
//! jit-cache `<run home>/target/*` (the newest is current) + `cache` + `flightrec`; weights
//! `~/.makepad/weights`; desk-link `~/.makepad/wm/desk-link`; screencaps
//! `~/makepad/local/screencap`; logs `~/.makepad/{devhub,wm}/*.log` and their rotations
//! `*.log.<n>`; homes-tmp `<work>/tmp/*` of ended leases + `/tmp/landgate.*`.
//!
//! Reclaim order (stops at floor + 10 GB): 1 targets no lease or process uses (LRU), 2 JIT
//! target caches that are not current, 3 ended leases' tmp + orphaned gate dirs, 4 rotated logs,
//! 5 (work nodes only) the current JIT cache. Never automatic: worktrees (lease end owns them,
//! with recovery), weights, Desk Link history, screencaps and other user data; on a user machine
//! still short after reclaim, the user data is a `disk_ask` (inbox).

use super::caps::{self, GB};
use crate::local::Publish;
use crate::proto::{hub_record, Sev};
use makepad_remote::json_str;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a measurement stays good unless something changed (a lease started or ended).
pub const MEASURE_EVERY: Duration = Duration::from_secs(600);
/// Reclaim stops this far above the floor.
pub const MARGIN: u64 = 10 << 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Worktrees,
    Targets,
    JitCache,
    Weights,
    DeskLink,
    Screencaps,
    Logs,
    HomesTmp,
}

pub const CATS: [Cat; 8] = [Cat::Worktrees, Cat::Targets, Cat::JitCache, Cat::Weights, Cat::DeskLink, Cat::Screencaps, Cat::Logs, Cat::HomesTmp];

impl Cat {
    pub fn as_str(self) -> &'static str {
        match self {
            Cat::Worktrees => "worktrees",
            Cat::Targets => "targets",
            Cat::JitCache => "jit-cache",
            Cat::Weights => "weights",
            Cat::DeskLink => "desk-link",
            Cat::Screencaps => "screencaps",
            Cat::Logs => "logs",
            Cat::HomesTmp => "homes-tmp",
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

/// The reclaim plan: what goes (in order), whether that reaches the floor, what to ask about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub take: Vec<Item>,
    pub reaches: bool,
    pub ask: Vec<Item>,
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
            break;
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
}

impl Roots {
    pub fn of(work: &Path) -> Roots {
        let home = caps::home_dir();
        let run_home = std::env::var_os("MAKEPAD_RUN_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".makepad-run"));
        Roots { home, work: work.to_path_buf(), run_home, tmp: std::env::temp_dir() }
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
        out.push(Item { cat, path, bytes, mtime, step, user_data, note });
    };
    for p in children(&r.work.join("wt")) {
        add(Cat::Worktrees, p, None, false, "lease worktree: its lease end cleans it");
    }
    for p in children(&r.work.join("target")).into_iter().filter(|p| p.is_dir()) {
        add(Cat::Targets, p, Some(1), false, "");
    }
    for p in children(&r.home.join("rc/pool")) {
        add(Cat::Targets, p.join("target"), Some(1), false, "legacy pool target");
    }
    let jit = children(&r.run_home.join("target"));
    let current = jit.iter().max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok()).cloned();
    for p in jit {
        let cur = Some(&p) == current.as_ref();
        // a running process of that cache holds its live lock (relax store.rs hold_build): never
        // taken, whatever its step (an embedded node's own loader runs from one of these)
        if !crate::jitcache::held_locks(&p).is_empty() {
            add(Cat::JitCache, p, None, false, "in use: a running process holds its live lock");
            continue;
        }
        add(Cat::JitCache, p, Some(if cur { 5 } else { 2 }), false, if cur { "current JIT cache" } else { "" });
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
    out
}

/// The Mac's guard (master side): lane worktrees under `wt.noindex` are counted, never removed;
/// a lane's `target` goes (step 1) only when the lane is merged into work and idle.
pub fn mac_items(repo: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    for i in super::wt::infos(repo).unwrap_or_default() {
        let t = i.entry.path.join("target");
        let (bytes, mtime) = dir_size(&i.entry.path);
        out.push(Item { cat: Cat::Worktrees, path: i.entry.path.clone(), bytes, mtime, step: None, user_data: false, note: "lane worktree: wt prune owns it".into() });
        if t.is_dir() {
            let (bytes, mtime) = dir_size(&t);
            let step = (i.merged && i.live.is_empty()).then_some(1);
            let note = if step.is_some() { "landed lane" } else { "lane not landed or busy" };
            out.push(Item { cat: Cat::Targets, path: t, bytes, mtime, step, user_data: false, note: note.into() });
        }
    }
    out
}

fn gb(b: u64) -> f64 {
    b as f64 / GB
}

/// Deletes exactly `p` (a directory tree or one file); never follows a symlink out.
pub fn remove(p: &Path) -> std::io::Result<()> {
    let m = std::fs::symlink_metadata(p)?;
    if m.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }
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
    warn: std::sync::atomic::AtomicU64,
    user_machine: std::sync::atomic::AtomicBool,
    publish: Publish,
    state: Mutex<State>,
}

impl Guard {
    pub fn new(machine: &str, roots: Roots, floor_gb: f64, warn_gb: f64, user_machine: bool, publish: Publish) -> Guard {
        Guard {
            machine: machine.to_string(),
            roots,
            floor: ((floor_gb * GB) as u64).into(),
            warn: ((warn_gb * GB) as u64).into(),
            user_machine: user_machine.into(),
            publish,
            state: Mutex::new(State { measured: None, dirty: true, below_floor: false, below_warn: false }),
        }
    }

    /// The master changed the node's lease.conf: floors and the user-machine rule follow.
    pub fn set_policy(&self, floor_gb: f64, warn_gb: f64, user_machine: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        self.floor.store((floor_gb * GB) as u64, Relaxed);
        self.warn.store((warn_gb * GB) as u64, Relaxed);
        self.user_machine.store(user_machine, Relaxed);
        self.touch();
    }

    pub fn floor(&self) -> u64 {
        self.floor.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn warn_at(&self) -> u64 {
        self.warn.load(std::sync::atomic::Ordering::Relaxed)
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

    /// The periodic look: floor / warn crossings, and automatic reclaim below the floor.
    pub fn check(&self, live: &dyn Fn() -> Live) {
        let free = self.free();
        let (floor_cross, warn_cross) = {
            let mut st = self.state.lock().unwrap();
            let f = free < self.floor() && !st.below_floor;
            let w = free < self.warn_at() && !st.below_warn;
            st.below_floor = free < self.floor();
            st.below_warn = free < self.warn_at();
            (f, w)
        };
        if warn_cross && !floor_cross {
            self.emit(Sev::Warn, format!("disk_warn: {} has {:.0} GB free, warn at {:.0} GB", self.machine, gb(free), gb(self.warn_at())));
        }
        if free < self.floor() {
            if floor_cross {
                self.emit(Sev::Warn, format!("disk_floor: {} has {:.0} GB free, floor {:.0} GB: reclaiming", self.machine, gb(free), gb(self.floor())));
            }
            self.reclaim(&live(), false);
        } else {
            let _ = self.items(&live(), false);
        }
    }

    /// Reclaims (or with `dry`, plans) down to the floor; answer lines.
    pub fn reclaim(&self, live: &Live, dry: bool) -> Vec<String> {
        let items = self.items(live, !dry);
        let free = self.free();
        let p = plan(&items, free, self.floor(), self.user_machine());
        let mut out = vec![format!(
            "{{\"node\":{},\"free_gb\":{:.1},\"floor_gb\":{:.0},\"warn_gb\":{:.0},\"dry\":{},\"reclaim_gb\":{:.1},\"reaches_floor\":{}}}",
            json_str(&self.machine),
            gb(free),
            gb(self.floor()),
            gb(self.warn_at()),
            dry,
            gb(p.take.iter().map(|i| i.bytes).sum()),
            p.reaches
        )];
        for c in CATS {
            let of: Vec<&Item> = items.iter().filter(|i| i.cat == c).collect();
            if of.is_empty() {
                continue;
            }
            let total: u64 = of.iter().map(|i| i.bytes).sum();
            let auto: u64 = of.iter().filter(|i| i.step.is_some()).map(|i| i.bytes).sum();
            out.push(format!("{{\"cat\":\"{}\",\"gb\":{:.2},\"n\":{},\"reclaimable_gb\":{:.2}}}", c.as_str(), gb(total), of.len(), gb(auto)));
        }
        let mut freed = 0;
        for i in &p.take {
            let why = format!("step {} {}", i.step.unwrap_or(0), if i.note.is_empty() { "LRU" } else { &i.note });
            if dry {
                out.push(format!("{{\"reclaim\":{},\"cat\":\"{}\",\"gb\":{:.2},\"why\":{}}}", json_str(&i.path.to_string_lossy()), i.cat.as_str(), gb(i.bytes), json_str(&why)));
                continue;
            }
            // looked at again at the moment of deletion: a lease may have started using it
            if let Some(u) = live.user_of(&i.path) {
                out.push(format!("{{\"kept\":{},\"why\":{}}}", json_str(&i.path.to_string_lossy()), json_str(&u)));
                continue;
            }
            match remove(&i.path) {
                Ok(()) => {
                    freed += i.bytes;
                    out.push(format!("{{\"reclaimed\":{},\"cat\":\"{}\",\"gb\":{:.2}}}", json_str(&i.path.to_string_lossy()), i.cat.as_str(), gb(i.bytes)));
                    self.emit(Sev::Info, format!("disk_reclaim: {} freed {} MB of {} on {} ({why})", i.path.display(), i.bytes >> 20, i.cat.as_str(), self.machine));
                }
                Err(e) => out.push(format!("{{\"failed\":{},\"err\":{}}}", json_str(&i.path.to_string_lossy()), json_str(&e.to_string()))),
            }
        }
        for i in &p.ask {
            out.push(format!("{{\"ask\":{},\"cat\":\"{}\",\"gb\":{:.2}}}", json_str(&i.path.to_string_lossy()), i.cat.as_str(), gb(i.bytes)));
            if !dry {
                self.emit(Sev::Warn, format!("disk_ask: {} is below its floor after reclaim; {} holds {:.1} GB of the user's {} (not deleted)", self.machine, i.path.display(), gb(i.bytes), i.cat.as_str()));
            }
        }
        if !dry && freed > 0 {
            self.touch();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(cat: Cat, path: &str, gbs: u64, mtime: u64, step: Option<u8>) -> Item {
        Item { cat, path: PathBuf::from(path), bytes: gbs << 30, mtime, step, user_data: false, note: String::new() }
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

    #[test]
    fn lists_categories_and_keeps_what_is_in_use() {
        let base = std::env::temp_dir().join(format!("devhub-disk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let r = Roots { home: base.join("home"), work: base.join("home/devhub"), run_home: base.join("home/.makepad-run"), tmp: base.join("tmp") };
        for d in ["home/devhub/target/abc/release", "home/devhub/target/lane-x", "home/devhub/tmp/L1", "home/devhub/wt/x-L2", "home/.makepad/wm", "home/.makepad-run/target/0001", "tmp/landgate.Q1"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("home/devhub/target/abc/release/bin"), vec![1u8; 100_000]).unwrap();
        std::fs::write(base.join("home/devhub/target/lane-x.last"), "abc").unwrap();
        std::fs::write(base.join("home/.makepad/wm/ai-hub.log"), "x").unwrap();
        std::fs::write(base.join("home/.makepad/wm/ai-hub.log.2"), "x").unwrap();
        let live = Live { leases: vec![(base.join("home/devhub/target/lane-x"), "L9".into())], cwds: vec![] };
        let items = list(&r, &live);
        let find = |p: &str| items.iter().find(|i| i.path == base.join(p)).unwrap_or_else(|| panic!("{p}: {items:?}"));
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
        assert_eq!(find("home/.makepad-run/target/0001").step, Some(5));
        assert!(rotated_log("x.log.10") && rotated_log("x.log.1.gz") && !rotated_log("x.log") && !rotated_log("x.logs"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
