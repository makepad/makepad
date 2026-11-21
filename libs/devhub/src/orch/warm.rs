//! Warm native targets, dev JIT stores and shared overlay code caches, without copying between
//! boxes. A new landed JIT id plans every dev box per OS/arch that lacks it, the most lane-active
//! first, with one lane-priority leg per box using four cores, never held back by background
//! admission or preemption: a box left out made the first live session there build the JIT itself
//! (3-5 min, a fifth of all live starts, 11-03). After building it, each box lists (never runs)
//! tests for its four most-used package/target pairs in the same held lease, then, as background
//! work on a lease of its own, starts each app lanes ran on overlays there in the last day once
//! under the new JIT, hidden, until its first present ([`apps_leg`]). Every 20 s,
//! agent lanes with grants in the last hour keep one extra box warm at their newest pin, unless
//! they already used two up boxes in that group; a landed lane whose branch is deleted keeps
//! no spare. Live/show job lanes only count toward their box's plan. Overlay spares list the
//! lane's three newest package/target pairs; native spares build the lane's two newest pairs
//! without running tests.
//! Entries are checked against targets at the pin; missing ones are dropped with one note, and live defaults resolve by M30.
//! Both keep at most one leg per lane and one warm start per ten minutes.
//! Spare legs stay at background priority under Held's lane-work and N-2 admission rules.
//! Plans, spares and their running legs are state (the store's `warm`, [`persist`]): a master
//! restart, its self-update's exec or a crash, takes each leg back on its lease ([`take_back`]),
//! and its steps are asked again by name, which the box answers with the run the previous master
//! started: a box's prebuild goes on through the restart with its build. The restart waits for none
//! of them (it waited up to 16 min for prebuilds, 11-15).

use super::leg::{self, Held, Spec};
use super::Ctx;
use crate::lease::client::ALL_TESTS;
use crate::lease::sched::NodeView;
use crate::lease::{Caps, Kind, Role, Src, PREBUILD_LANE_PREFIX, WARM_LANE_PREFIX};
use crate::proto::{wall_ms, Sev};
use crate::state::Pair;
use makepad_strict_json::{obj, s, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(20);
const ACTIVE_MS: u64 = 60 * 60 * 1000;
const REWARM_MS: u64 = 10 * 60 * 1000;
const RETRY: Duration = Duration::from_secs(30);
const AGENT: &str = "devhub-warm";
/// The window of lane app runs a box's app pass warms ([`apps_by_box`]).
const APPS_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Package {
    pkg: String,
    harness: Harness,
    /// the features its target requires (the manifest's `required-features`, [`resolve`]):
    /// selected without them it does not build (`--test effect_modules` without `scene3d`: every
    /// warm run of it failed, 11-06)
    features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Harness {
    All,
    Default,
    Lib,
    Bin(String),
    Test(String),
    Tests,
}

impl Package {
    fn from_class(class: &str) -> Option<Self> {
        // `devhub test` classes; cargo-routed ones carry `native:`, live jobs `live:`
        let class = crate::lease::base_class(class);
        let class = class.strip_prefix("native:").unwrap_or(class);
        let live = class.starts_with("live:");
        let class = class.strip_prefix("live:").unwrap_or(class).strip_prefix("test:")?;
        let (pkg, target) = class.split_once(':').map_or((class, None), |(p, t)| (p, Some(t)));
        if pkg.is_empty() { return None; }
        let harness = match target {
            None => if live { Harness::Default } else { Harness::All },
            Some("lib") => Harness::Lib,
            Some(ALL_TESTS) => Harness::Tests,
            Some(t) if t.starts_with("bin:") => {
                let b = &t[4..];
                if b.is_empty() || b.contains(':') { return None; }
                Harness::Bin(b.to_string())
            }
            Some(t) if !t.is_empty() && !t.contains(':') => Harness::Test(t.to_string()),
            _ => return None,
        };
        Some(Self { pkg: pkg.to_string(), harness, features: Vec::new() })
    }

    fn label(&self) -> String {
        match &self.harness {
            Harness::All => self.pkg.clone(),
            Harness::Default => format!("{}:default", self.pkg),
            Harness::Lib => format!("{}:lib", self.pkg),
            Harness::Bin(b) => format!("{}:bin:{b}", self.pkg),
            Harness::Test(t) => format!("{}:{t}", self.pkg),
            Harness::Tests => format!("{}:{ALL_TESTS}", self.pkg),
        }
    }

    fn select(&self) -> Vec<String> {
        let mut select: Vec<String> = match &self.harness {
            Harness::Lib => vec!["--lib".into()],
            Harness::Bin(b) => vec!["--bin".into(), b.clone()],
            Harness::Test(t) => vec!["--test".into(), t.clone()],
            Harness::Tests => vec!["--tests".into()],
            Harness::All | Harness::Default => Vec::new(),
        };
        if !self.features.is_empty() {
            select.extend(["--features".into(), self.features.join(",")]);
        }
        select
    }

    fn argv(&self) -> Vec<String> {
        // the profile `devhub test` runs the package with ([`crate::lease::JIT_TEST_PROFILE`])
        let mut argv: Vec<_> = ["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "test", &self.pkg].into_iter().chain(crate::lease::JIT_TEST_PROFILE).map(String::from).collect();
        argv.extend(self.select());
        argv.extend(["--".into(), "--list".into()]);
        argv
    }

    /// Builds the lane's test target without running it; at nice 19 on Unix (runs start at 10): idle CPU only.
    fn native_argv(&self, os: &str) -> Vec<String> {
        let nice: &[&str] = if os == "windows" { &[] } else { &["nice", "-n", "9"] };
        let mut argv: Vec<_> = nice.iter().copied().chain(["cargo", "test", "--release", "-p", &self.pkg, "--no-run"]).map(String::from).collect();
        argv.extend(self.select());
        argv
    }
}

fn resolve(repo: &Path, rev: &str, packages: &[Package], overlay: bool) -> (Vec<Package>, String) {
    if packages.is_empty() { return (Vec::new(), String::new()); }
    let tree = crate::lease::Pin::of_rev(rev).ok_or_else(|| "missing commit pair".to_string()).and_then(|pin| {
        super::ci::Tree::at(repo, &pin.pair())
    });
    match tree {
        Ok(tree) => resolve_in(&tree, packages, overlay),
        Err(e) => (Vec::new(), format!("targets unknown at {rev}: {e}")),
    }
}

/// The packages' targets as `tree` has them, each with the features its target requires; the
/// note names those it dropped (no such package or target).
/// The packages a warm run builds at `tree`, with their targets' required features. `overlay`: a
/// JIT overlay run, which leaves out a package whose tests run in the lane's tree (its test-tree:
/// no run of them ever reads an overlay, and one fails there; relax-compiler's build script runs
/// cargo metadata on the checkout, 11-13).
fn resolve_in(tree: &super::ci::Tree, packages: &[Package], overlay: bool) -> (Vec<Package>, String) {
    let dirs = super::ci::package_dirs(tree);
    let (mut kept, mut dropped) = (Vec::new(), Vec::new());
    let mut seen = BTreeSet::new();
    for package in packages {
        let krate = dirs.get(&package.pkg).and_then(|dir| {
            super::ci::krate(tree, &if dir.is_empty() { "Cargo.toml".into() } else { format!("{dir}/Cargo.toml") })
        });
        let Some(k) = krate else {
            dropped.push(format!("{} (no such package)", package.label()));
            continue;
        };
        if let Some(why) = k.test_tree.as_deref().filter(|_| overlay) {
            dropped.push(format!("{} (runs in the lane's tree: {why})", package.label()));
            continue;
        }
        let harness = match &package.harness {
            Harness::All | Harness::Tests => Ok(package.harness.clone()),
            Harness::Lib if k.lib => Ok(Harness::Lib),
            Harness::Lib => Err("no lib".to_string()),
            Harness::Bin(b) if k.bins.contains(b) => Ok(Harness::Bin(b.clone())),
            Harness::Test(t) if k.tests.contains(t) => Ok(Harness::Test(t.clone())),
            Harness::Bin(_) | Harness::Test(_) => Err("no such target".to_string()),
            Harness::Default if k.lib => Ok(Harness::Lib),
            Harness::Default if k.bins.len() + k.tests.len() == 1 => {
                Ok(if let Some(b) = k.bins.first() { Harness::Bin(b.clone()) } else { Harness::Test(k.tests[0].clone()) })
            }
            Harness::Default => Err(format!("no lib and {} harness targets", k.bins.len() + k.tests.len())),
        };
        match harness {
            Ok(harness) => {
                let target = match &harness {
                    Harness::Bin(b) => Some(format!("bin:{b}")),
                    Harness::Test(t) => Some(format!("test:{t}")),
                    _ => None,
                };
                let features = target.and_then(|t| k.required.get(&t).cloned()).unwrap_or_default();
                let package = Package { pkg: package.pkg.clone(), harness, features };
                if seen.insert(package.clone()) { kept.push(package); }
            }
            Err(e) => dropped.push(format!("{} ({e})", package.label())),
        }
    }
    let note = if dropped.is_empty() { String::new() } else { format!("dropped {}", dropped.join(", ")) };
    (kept, note)
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    id: String,
    rev: String,
    packages: Vec<Package>,
}

#[derive(Clone)]
struct Progress {
    state: String,
    note: String,
    id: String,
    lease: Option<String>,
    started_ms: u64,
    ended_ms: u64,
    /// a prebuild's test listings so far (its plan's bucket verdict)
    listed: Vec<Listed>,
}

/// One test listing of a prebuild: its unit (`suite <pkg>`), whether it listed, its output.
#[derive(Clone)]
struct Listed {
    unit: String,
    ok: bool,
    text: String,
}

impl Progress {
    fn waiting(id: &str, now: u64) -> Self {
        Self { state: "waiting".into(), note: String::new(), id: id.into(), lease: None, started_ms: now, ended_ms: 0, listed: Vec::new() }
    }

    /// A failure its box outgrew: the JIT it failed on is in the box's store now, however it got
    /// there (a warm, a JIT run, a lane build). One box's failure stayed in the health report after a
    /// job built that JIT (11-03).
    fn clear_if_stored(&mut self, stored: &[String], now: u64) -> bool {
        if self.state != "failed" || self.id.is_empty() || !stored.contains(&self.id) { return false; }
        self.state = "stored".into();
        self.note = "the box's store holds it (another run built it)".into();
        self.ended_ms = now;
        true
    }

    fn fields(&self, now: u64) -> Vec<(&str, Value)> {
        vec![
            ("id", s(&self.id)), ("state", s(&self.state)), ("note", s(&self.note)),
            ("seconds", Value::Int((if self.ended_ms == 0 { now } else { self.ended_ms }).saturating_sub(self.started_ms) as i64 / 1000)),
        ]
    }
}

#[derive(Clone)]
struct Node {
    name: String,
    os: String,
    activity: usize,
    key: Key,
    progress: Progress,
    /// the apps lanes ran on overlays on this box in the last [`APPS_MS`], the most-run first: its
    /// app pass after the prebuild ([`apps_leg`])
    apps: Vec<String>,
    /// that pass's progress (None: no apps)
    apps_progress: Option<Progress>,
}

#[derive(Clone)]
struct Plan {
    generation: u64,
    pair: Pair,
    /// the previous plan's tip: the landings since are the ones its listings judge
    base: Option<Pair>,
    ids: Vec<String>,
    started_ms: u64,
    nodes: Vec<Node>,
    /// its listings went to the `prebuild` bucket
    judged: bool,
}

struct Spare {
    node: String,
    native: bool,
    generation: u64,
    warmed: Option<Key>,
    progress: Progress,
}

struct Spares {
    lanes: BTreeMap<String, Spare>,
    // Kept until the worker returns, even after its spare is forgotten: no overlapping legs.
    running: BTreeMap<String, u64>,
    last_started: BTreeMap<String, u64>,
    generation: u64,
}

#[derive(Clone)]
struct Run {
    generation: u64,
    lane: Option<String>,
    native: bool,
    /// its plan box's app pass ([`apps_leg`]), not its prebuild
    apps: bool,
    node: String,
    os: String,
    key: Key,
    started_ms: u64,
    since_ms: Option<u64>,
    /// the lease a previous master held this leg on, and whether it shipped there: taken back, not
    /// asked again ([`take_back`]); never persisted
    adopt: Option<(String, bool)>,
}

static PLAN: Mutex<Option<Plan>> = Mutex::new(None);
static SPARES: Mutex<Spares> = Mutex::new(Spares { lanes: BTreeMap::new(), running: BTreeMap::new(), last_started: BTreeMap::new(), generation: 0 });

pub fn start(ctx: Arc<Ctx>) {
    take_back(&ctx);
    let c = ctx.clone();
    if let Err(e) = std::thread::Builder::new().name(AGENT.into()).spawn(move || {
        let mut seen = None;
        loop {
            tick(&c, &mut seen);
            std::thread::sleep(TICK);
        }
    }) {
        ctx.store.event("warm", Sev::Warn, format!("warm: thread: {e}"), None);
    }
}

/// The legs running now, by [`leg_key`]: the state's `legs`.
static LEGS: Mutex<BTreeMap<(String, u64), Run>> = Mutex::new(BTreeMap::new());
/// The state [`persist`] wrote last.
static PERSISTED: Mutex<String> = Mutex::new(String::new());

/// A leg's name (its lane, else its box) and start: the same through a newer plan it follows.
fn leg_key(run: &Run) -> (String, u64) {
    let name = match (&run.lane, run.apps) {
        (Some(lane), _) => format!("lane {lane}"),
        (None, true) => format!("apps {}", run.node),
        (None, false) => format!("box {}", run.node),
    };
    (name, run.started_ms)
}

/// Writes the plan, the spares and the running legs to the store when they changed: what a
/// restarted master takes back ([`take_back`]). Called after a leg's lease or step changed, a leg
/// started or ended, and each tick; never for a wait line (none is kept).
fn persist() {
    let Some(ctx) = super::ctx() else { return };
    let mut last = PERSISTED.lock().unwrap();
    let legs: Vec<Run> = LEGS.lock().unwrap().values().cloned().collect();
    let plan = PLAN.lock().unwrap().clone();
    let value = state_value(plan.as_ref(), &SPARES.lock().unwrap(), &legs);
    let json = value.to_json();
    if *last == json { return; }
    match ctx.store.update(|d| d.warm = Some(value)) {
        Ok(()) => *last = json,
        Err(e) => ctx.store.event("warm", Sev::Warn, format!("warm: its state: {e}"), None),
    }
}

/// A plan of one box's prebuild at `rev`, its leg started: what a landed JIT plans, without the
/// fleet's caps (the master restart's end-to-end test, orch/restart_tests.rs).
#[cfg(all(test, target_os = "linux"))]
pub(super) fn prebuild_on(ctx: &Arc<Ctx>, node: &str, rev: &str) {
    let now = wall_ms();
    let key = Key { id: "test-jit".into(), rev: rev.into(), packages: Vec::new() };
    let progress = Progress::waiting(&key.id, now);
    *PLAN.lock().unwrap() = Some(Plan { generation: 1, pair: Pair::new("", ""), base: None, ids: vec![key.id.clone()], started_ms: now, nodes: vec![Node { name: node.into(), os: "linux".into(), activity: 0, key: key.clone(), progress, apps: Vec::new(), apps_progress: None }], judged: false });
    spawn(ctx, Run { generation: 1, lane: None, native: false, apps: false, node: node.into(), os: "linux".into(), key, started_ms: now, since_ms: Some(now), adopt: None });
}

/// A leg that follows a newer plan's target for its box: its state says the new one.
fn moved(run: &Run) {
    LEGS.lock().unwrap().insert(leg_key(run), run.clone());
    persist();
}

/// Takes back what the previous master left in the store: its plan, its spares and each leg it
/// ran ([`restore`]). Its other warm leases end. Called before the master's own leases that no
/// record names end: the state names the kept ones.
pub(super) fn take_back(ctx: &Arc<Ctx>) {
    let Some((mut plan, mut spares, runs)) = ctx.store.read(|d| d.warm.clone()).and_then(|v| state_parse(&v)) else { return };
    let legs = restore(&mut plan, &mut spares, runs, &|l| leg::lease_held(ctx, l));
    *PLAN.lock().unwrap() = plan;
    *SPARES.lock().unwrap() = spares;
    let kept: BTreeSet<String> = legs.iter().filter_map(|r| r.adopt.as_ref().map(|(l, _)| l.clone())).collect();
    for l in ctx.leases.view().iter().filter(|l| l.req.agent.as_deref() == Some(AGENT) && !kept.contains(&l.id)) {
        ctx.leases.release_live(&l.id);
    }
    for run in &legs {
        let who = run.lane.as_deref().unwrap_or(&run.key.rev);
        let how = run.adopt.as_ref().map_or_else(|| "it asks for a box again, keeping its age".to_string(), |(l, _)| format!("taken back on {l}: its runs go on"));
        ctx.store.event("warm", Sev::Info, format!("warm {who} on {}: the master restarted; {how}", run.node), None);
    }
    spawn_all(ctx, legs);
}

/// What [`take_back`] resumes of a previous master's state, on its values. A leg whose progress
/// has not ended goes on on its lease while `held` says that lease still is ([`Run::adopt`]), else
/// asks again in its place, keeping its age. A plan box the state names no leg for (a restart
/// between a plan and its legs) starts one; a lane marked running without a leg may start again.
fn restore(plan: &mut Option<Plan>, spares: &mut Spares, runs: Vec<Run>, held: &dyn Fn(&str) -> bool) -> Vec<Run> {
    let mut legs = Vec::new();
    for mut run in runs {
        // (a leg its plan or spare left behind, or one that ended, is not taken back)
        let Some(p) = progress_in(plan, spares, &run).filter(|p| p.ended_ms == 0) else { continue };
        run.adopt = p.lease.clone().filter(|l| held(l)).map(|l| (l, p.state != "held"));
        if run.adopt.is_none() { p.lease = None; p.state = "waiting".into(); }
        legs.push(run);
    }
    if let Some(p) = plan.as_mut() {
        let (generation, started_ms, len) = (p.generation, p.started_ms, p.nodes.len());
        for (rank, n) in p.nodes.iter_mut().enumerate() {
            let since_ms = Some(started_ms.saturating_sub((len - rank) as u64));
            // an app pass whose prebuild is done, its leg missing from the state, starts again
            if n.progress.state == "done" && !legs.iter().any(|r| r.apps && r.node == n.name) {
                if let Some(a) = n.apps_progress.as_mut().filter(|a| a.ended_ms == 0) {
                    a.lease = None;
                    a.state = "waiting".into();
                    legs.push(Run { generation, lane: None, native: false, apps: true, node: n.name.clone(), os: n.os.clone(), key: n.key.clone(), started_ms, since_ms, adopt: None });
                }
            }
            if n.progress.ended_ms != 0 || legs.iter().any(|r| r.lane.is_none() && !r.apps && r.node == n.name) { continue; }
            n.progress.lease = None;
            n.progress.state = "waiting".into();
            legs.push(Run { generation, lane: None, native: false, apps: false, node: n.name.clone(), os: n.os.clone(), key: n.key.clone(), started_ms, since_ms, adopt: None });
        }
    }
    let stale: Vec<String> = spares.running.iter().filter(|(lane, g)| !legs.iter().any(|r| r.lane.as_ref() == Some(*lane) && r.generation == **g)).map(|(lane, _)| lane.clone()).collect();
    for lane in stale {
        spares.running.remove(&lane);
        spares.last_started.remove(&lane);
        if let Some(s) = spares.lanes.get_mut(&lane).filter(|s| s.progress.ended_ms == 0) {
            s.progress.lease = None;
            s.progress.state = "waiting".into();
        }
    }
    legs
}

/// A leg's progress in a plan and spares: its plan box's, or its lane spare's, of its generation.
fn progress_in<'a>(plan: &'a mut Option<Plan>, spares: &'a mut Spares, run: &Run) -> Option<&'a mut Progress> {
    match &run.lane {
        Some(lane) => spares.lanes.get_mut(lane).filter(|s| s.generation == run.generation).map(|s| &mut s.progress),
        None => {
            let n = plan.as_mut().filter(|p| p.generation == run.generation)?.nodes.iter_mut().find(|n| n.name == run.node)?;
            if run.apps { n.apps_progress.as_mut() } else { Some(&mut n.progress) }
        }
    }
}

fn set(run: &Run, f: impl FnOnce(&mut Progress)) -> bool {
    if let Some(lane) = &run.lane {
        let mut spares = SPARES.lock().unwrap();
        let Some(s) = spares.lanes.get_mut(lane).filter(|s| s.generation == run.generation) else { return false };
        f(&mut s.progress);
    } else {
        let mut plan = PLAN.lock().unwrap();
        let Some(p) = plan.as_mut().filter(|p| p.generation == run.generation) else { return false };
        let Some(n) = p.nodes.iter_mut().find(|n| n.name == run.node) else { return false };
        if run.apps {
            let Some(a) = n.apps_progress.as_mut() else { return false };
            f(a);
        } else {
            f(&mut n.progress);
        }
    }
    true
}

#[derive(Default)]
struct LaneActivity {
    boxes: BTreeSet<String>,
    src: Src,
    newest_ms: u64,
    node: String,
    rev: String,
    hint: Vec<String>,
    packages: Vec<(Package, u64)>,
    native_packages: Vec<(Package, u64)>,
}

#[derive(Default)]
struct BoxActivity {
    lanes: BTreeSet<String>,
    packages: BTreeMap<Package, usize>,
}

impl BoxActivity {
    fn packages(&self) -> Vec<Package> {
        let mut pairs: Vec<_> = self.packages.iter().collect();
        pairs.sort_by(|(a, n), (b, m)| m.cmp(n).then_with(|| a.cmp(b)));
        pairs.into_iter().take(4).map(|(p, _)| p.clone()).collect()
    }
}

#[derive(Default)]
struct Activity {
    lanes: BTreeMap<String, LaneActivity>,
    boxes: BTreeMap<String, BoxActivity>,
    // Every granted lane, including work that does not itself ask for a spare.
    active: BTreeSet<String>,
}

impl Activity {
    fn see(&mut self, lane: &str, node: &str, granted_ms: u64, src: Src, rev: &str, hint: Vec<String>, class: &str) {
        let l = self.lanes.entry(lane.to_string()).or_default();
        l.boxes.insert(node.to_string());
        if l.node.is_empty() || granted_ms > l.newest_ms {
            l.newest_ms = granted_ms;
            l.src = src;
            l.node = node.to_string();
            l.rev = rev.to_string();
            l.hint = hint;
        }
        let package = Package::from_class(class);
        if src == Src::Overlay {
            let b = self.boxes.entry(node.to_string()).or_default();
            b.lanes.insert(lane.to_string());
            if let Some(p) = &package { *b.packages.entry(p.clone()).or_default() += 1; }
        }
        if let Some(p) = package {
            let packages = if src == Src::Tree { &mut l.native_packages } else { &mut l.packages };
            if let Some((_, t)) = packages.iter_mut().find(|(x, _)| *x == p) { *t = (*t).max(granted_ms); }
            else { packages.push((p, granted_ms)); }
            packages.sort_by(|(a, t), (b, u)| u.cmp(t).then_with(|| a.cmp(b)));
            packages.truncate(if src == Src::Tree { 2 } else { 3 });
        }
    }
}

fn activity(ctx: &Ctx, since_ms: u64) -> Activity {
    let mut activity = Activity::default();
    let mut seen = BTreeSet::new();
    for l in ctx.leases.view() {
        let Some(t) = l.granted_ms.filter(|t| *t >= since_ms) else { continue };
        let Some(node) = l.node else { continue };
        seen.insert(l.id);
        activity.active.insert(l.req.lane.clone());
        if l.req.agent.as_deref().is_some_and(|a| a.starts_with("devhub-")) || (l.req.src == Src::Tree && matches!(l.req.kind, Kind::Ci | Kind::Gate | Kind::Inspect)) { continue; }
        activity.see(&l.req.lane, &node, t, l.req.src, l.req.rev.as_deref().unwrap_or(""), l.req.hint, &l.req.class);
    }
    // Ended activity survives a master exec in the ledger; only read its recent tail.
    let Ok(mut file) = std::fs::File::open(crate::proto::home().join("leases.jsonl")) else { return activity };
    let Ok(len) = file.metadata().map(|m| m.len()) else { return activity };
    let start = len.saturating_sub(4 * 1024 * 1024);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut bytes).is_err() { return activity; }
    let mut lines = bytes.split(|b| *b == b'\n');
    if start != 0 { let _ = lines.next(); }
    for line in lines {
        let Ok(v) = makepad_strict_json::parse(line) else { continue };
        let Some(req) = v.get("req") else { continue };
        let Some(t) = v.get("granted_ms").and_then(Value::as_u64).filter(|t| *t >= since_ms) else { continue };
        let (Some(node), Some(lane)) = (v.get("node").and_then(Value::as_str), req.get("lane").and_then(Value::as_str)) else { continue };
        if let Some(id) = v.get("id").and_then(Value::as_str) { if !seen.insert(id.to_string()) { continue; } }
        activity.active.insert(lane.to_string());
        let Some(src) = req.get("src").and_then(Value::as_str).and_then(Src::parse) else { continue };
        if req.get("agent").and_then(Value::as_str).is_some_and(|a| a.starts_with("devhub-")) || (src == Src::Tree && matches!(req.get("kind").and_then(Value::as_str), Some("ci" | "gate" | "inspect"))) { continue; }
        let hint = req.get("hint").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect();
        activity.see(lane, node, t, src, req.get("rev").and_then(Value::as_str).unwrap_or(""), hint, req.get("class").and_then(Value::as_str).unwrap_or(""));
    }
    activity
}

fn tick(ctx: &Arc<Ctx>, seen: &mut Option<(String, Vec<String>)>) {
    // (until the boxes answer after the master's start: a plan or a spare judged on boxes not
    // heard yet would end what the previous master left running there)
    if ctx.leases.settling() { return; }
    let nodes = ctx.leases.nodes();
    let now = wall_ms();
    let activity = activity(ctx, now.saturating_sub(ACTIVE_MS));
    let compilers = ctx.leases.jit_compilers();
    clear_stored(&nodes, now);
    // Spares follow lane pins even while the landed tip is unchanged or unavailable.
    spare_tick(ctx, &nodes, &compilers, &activity, now);
    plan_tick(ctx, seen, &nodes, &compilers, &activity, now);
    persist();
}

/// Clears the plan's and the overlay spares' failures whose JIT their box's store holds now.
fn clear_stored(nodes: &[NodeView], now: u64) {
    let stored = |name: &str| nodes.iter().find(|n| n.name == name).and_then(|n| n.caps.as_ref()?.jit_store.as_ref().map(|s| s.ids.clone())).unwrap_or_default();
    if let Some(p) = PLAN.lock().unwrap().as_mut() {
        for n in &mut p.nodes { n.progress.clear_if_stored(&stored(&n.name), now); }
    }
    for s in SPARES.lock().unwrap().lanes.values_mut().filter(|s| !s.native) { s.progress.clear_if_stored(&stored(&s.node), now); }
}

fn plan_tick(ctx: &Arc<Ctx>, seen: &mut Option<(String, Vec<String>)>, nodes: &[NodeView], compilers: &[String], activity: &Activity, started_ms: u64) {
    let Some(pair) = super::snapshot(&ctx.repo) else { return };
    if compilers.is_empty() { return; }
    let key = (pair.work.clone(), compilers.to_vec());
    if seen.as_ref() == Some(&key) { return; }
    let ids = match crate::lease::loaders::commit_ids(&ctx.repo, &pair.work, compilers) {
        Ok(ids) => ids,
        Err(e) => {
            ctx.store.event("warm", Sev::Warn, format!("warm {}: {e}", super::short(&pair.work)), None);
            return;
        }
    };
    *seen = Some(key);
    let by_compiler: BTreeMap<_, _> = compilers.iter().cloned().zip(ids.iter().cloned()).collect();
    let ids: Vec<_> = ids.into_iter().collect::<BTreeSet<_>>().into_iter().collect();
    if PLAN.lock().unwrap().as_ref().is_some_and(|p| p.ids == ids) { return; }
    let mut apps = apps_by_box(&crate::proto::home().join("records.jsonl"), started_ms.saturating_sub(APPS_MS));
    let off = apps_off(&crate::master::nodes_conf());
    apps.retain(|node, _| !off.contains(node));
    let nodes = plan_nodes(nodes, &by_compiler, activity, &apps, &pair.rev(), started_ms);
    let (plan, previous, followed) = {
        let mut p = PLAN.lock().unwrap();
        let generation = p.as_ref().map_or(1, |p| p.generation + 1);
        let plan = Plan { generation, pair, base: p.as_ref().map(|p| p.pair.clone()), ids, started_ms, nodes, judged: false };
        let previous = p.replace(plan.clone());
        // a box whose leg still asks: that leg follows this plan in its place ([`ASKING`])
        let asking = ASKING.lock().unwrap();
        let followed: BTreeSet<String> = plan.nodes.iter().filter(|n| asking.contains_key(&n.name)).map(|n| n.name.clone()).collect();
        (plan, previous, followed)
    };
    // Registration and generation changes share the lock; no held old leg is missed.
    if let Some(p) = previous {
        // (an app pass of the older JIT ends with it: its lease goes)
        for id in p.nodes.iter().flat_map(|n| [n.progress.lease.as_deref(), n.apps_progress.as_ref().and_then(|a| a.lease.as_deref())]).flatten() { ctx.leases.release_live(id); }
        // the listings it got to before the next JIT came are its verdict (off this thread: the
        // landings' attribution reads their trees)
        if !p.judged {
            let c = ctx.clone();
            if let Err(e) = std::thread::Builder::new().name("warm-judge".into()).spawn(move || judge(&c, &p, true)) {
                ctx.store.event("warm", Sev::Warn, format!("warm: judging a superseded plan: {e}"), None);
            }
        }
    }
    ctx.store.event("warm", Sev::Info, format!("warm {}: {} on {}", super::short(&plan.pair.work), plan.ids.join(", "), plan.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", ")), None);
    let runs = plan.nodes.iter().enumerate().filter(|(_, n)| !followed.contains(&n.name)).map(|(rank, node)| {
        // Keep every timestamp in the past: the scheduler clamps future queue times to now.
        let since_ms = Some(plan.started_ms.saturating_sub((plan.nodes.len() - rank) as u64));
        Run { generation: plan.generation, lane: None, native: false, apps: false, node: node.name.clone(), os: node.os.clone(), key: node.key.clone(), started_ms, since_ms, adopt: None }
    }).collect();
    spawn_all(ctx, runs);
}

/// The boxes a new JIT id is built on: every up dev box that lacks it, per OS/arch group the most
/// lane-active first. A box no lane used lately builds it too (the next session may come to it),
/// listing no tests; so does a bench box, whose tests are its timed runs' own (the listing held
/// it a minute more per landing while timed work waited "held for interactive", 11-03). A box's
/// `apps` ([`apps_by_box`]) are its app pass after the prebuild; none on a bench box, whose runs
/// are timed, nor on Windows, where a run cannot quit after its present.
fn plan_nodes(nodes: &[NodeView], by_compiler: &BTreeMap<String, String>, activity: &Activity, apps: &BTreeMap<String, Vec<String>>, rev: &str, started_ms: u64) -> Vec<Node> {
    let mut groups: BTreeMap<(String, String), Vec<Node>> = BTreeMap::new();
    for n in nodes.iter().filter(|n| n.up && n.dev && !n.role().users()) {
        let Some(caps) = &n.caps else { continue };
        let Some(store) = &caps.jit_store else { continue };
        let Some(id) = by_compiler.get(&store.compiler) else { continue };
        if store.ids.contains(id) { continue; }
        let b = activity.boxes.get(&n.name);
        let packages = if n.role() == Role::Bench { Vec::new() } else { b.map(BoxActivity::packages).unwrap_or_default() };
        let apps = if n.role() == Role::Bench || caps.os == "windows" { Vec::new() } else { apps.get(&n.name).cloned().unwrap_or_default() };
        let apps_progress = (!apps.is_empty()).then(|| Progress { state: "after its prebuild".into(), ..Progress::waiting(id, started_ms) });
        groups.entry((caps.os.clone(), caps.arch.clone())).or_default().push(Node { name: n.name.clone(), os: caps.os.clone(), activity: b.map_or(0, |b| b.lanes.len()), key: Key { id: id.clone(), rev: rev.to_string(), packages }, progress: Progress::waiting(id, started_ms), apps, apps_progress });
    }
    let mut out = Vec::new();
    for (_, mut candidates) in groups {
        candidates.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| a.name.cmp(&b.name)));
        out.extend(candidates);
    }
    out
}

fn same_group(a: &Caps, b: &Caps) -> bool { a.os == b.os && a.arch == b.arch }

/// A box that may warm a lane's spare. A bench box only in a group with no pool box: the lane's
/// work goes to the pool boxes (the scheduler's pool_takes), and a spare there held a bench box
/// for nothing.
fn spare_ok(n: &NodeView, l: &LaneActivity, group: &Caps, pool_in_group: bool) -> bool {
    n.up && n.dev && !n.role().users() && !(pool_in_group && n.role() == Role::Bench) && !l.boxes.contains(&n.name) && n.caps.as_ref().is_some_and(|c| same_group(c, group) && c.has(if l.src == Src::Tree { "warm-trees" } else { "jit-inputs" }))
}

/// The group (OS, arch) has an up pool box.
fn pool_in_group(nodes: &[NodeView], group: &Caps) -> bool {
    nodes.iter().any(|n| n.up && matches!(n.role(), Role::Pool | Role::Long) && n.caps.as_ref().is_some_and(|c| same_group(c, group)))
}

fn native_room(c: &Caps, activity: &Activity) -> bool {
    c.native.len() < c.native_trees as usize || c.native.iter().any(|lane| !activity.active.contains(lane))
}

/// Of `free` (boxes the scheduler could start a spare's queued leg on now), those the lane's
/// spare may take ([`spare_ok`], RAM and a native slot as its first pick asks).
fn free_spares(nodes: &[NodeView], free: &[String], l: &LaneActivity, group: &Caps, native: bool, activity: &Activity) -> Vec<String> {
    let pooled = pool_in_group(nodes, group);
    nodes.iter().filter(|n| free.contains(&n.name) && spare_ok(n, l, group, pooled) && n.caps.as_ref().is_some_and(|c| c.ram_avail_gb >= 8.0 && (!native || native_room(c, activity)))).map(|n| n.name.clone()).collect()
}

/// A spare's leg that has waited to start at its box (`running`, holding no lease yet) past what
/// it saves there, `value_ms` (a JIT build on that box: the most the lane's next lease there
/// spends building it): its lane's next lease builds anyway. Three lanes' warm legs waited up to
/// 10 min at a box behind its tests shard while two others sat at 1-3% CPU (11-09).
fn waited_out(s: &Spare, running: bool, now: u64, value_ms: u64) -> bool {
    running && s.progress.lease.is_none() && s.progress.state == "waiting" && now.saturating_sub(s.progress.started_ms) > value_ms
}

/// The landed tips a lane's commits are looked for in: work, and the private checkout's main.
struct Tips {
    work: Option<String>,
    main: Option<String>,
}

impl Tips {
    fn read(repo: &std::path::Path) -> Tips {
        Tips { work: crate::gitread::rev(repo, "refs/heads/work"), main: crate::gitread::rev(&repo.join("apps/commercial"), "refs/heads/main") }
    }

    /// Every commit a lease's `rev` names (`branch@sha`, then `commercial=branch@sha`) is in its
    /// repository's tip; false for a rev that names none, or a tip not read.
    fn hold(&self, repo: &std::path::Path, rev: &str) -> bool {
        let mut any = false;
        for part in rev.split(',').filter(|p| !p.is_empty()) {
            let (dir, tip, part) = match part.strip_prefix("commercial=") {
                Some(p) => (repo.join("apps/commercial"), &self.main, p),
                None => (repo.to_path_buf(), &self.work, part),
            };
            let (Some(tip), Some((_, sha))) = (tip, part.rsplit_once('@')) else { return false };
            if sha.is_empty() || !crate::gitread::is_ancestor(&dir, sha, tip) {
                return false;
            }
            any = true;
        }
        any
    }
}

/// Only agent lanes receive spares, excluding devhub's own job lanes.
fn agent_lane(lane: &str) -> bool {
    !matches!(lane, crate::lease::sched::CI_LANE | crate::lease::sched::LANDING_LANE)
        && !["live-", "show-", "landq", "jitlegs", WARM_LANE_PREFIX].iter().any(|p| lane.starts_with(p))
}

fn spare_tick(ctx: &Arc<Ctx>, nodes: &[NodeView], compilers: &[String], activity: &Activity, now: u64) {
    // Check branches once per tick, before taking SPARES (refs read in-process).
    let mut done = BTreeMap::new();
    let tips = Tips::read(&ctx.repo);
    let mut landed = BTreeMap::new();
    let needing: BTreeMap<_, _> = activity.lanes.iter().filter_map(|(lane, l)| {
        if !agent_lane(lane) { return None; }
        let first = l.rev.split(',').next().unwrap_or("");
        if let Some((branch, _)) = first.split_once('@') {
            if !matches!(branch, "" | "work" | "main") && *done.entry(branch).or_insert_with(|| crate::gitread::rev(&ctx.repo, &format!("refs/heads/{branch}")).is_none()) { return None; }
        }
        // a lane whose newest lease ran commits the tips hold has landed: its build is the tip's,
        // which the boxes' prebuilds warm (11-17: two landed lanes' warm legs waited 10-12 min at
        // a box, at the head of the queue)
        if *landed.entry(l.rev.as_str()).or_insert_with(|| tips.hold(&ctx.repo, &l.rev)) { return None; }
        // (a lane in a nested repository names that repository's commit: the root cannot build it)
        if crate::gitread::lacks_commit(&ctx.repo, first.rsplit('@').next().unwrap_or("")) { return None; }
        let group = nodes.iter().find(|n| n.name == l.node)?.caps.as_ref()?;
        let used = nodes.iter().filter(|n| n.up && l.boxes.contains(&n.name) && n.caps.as_ref().is_some_and(|c| same_group(c, group))).count();
        (used < 2).then_some((lane, (l, group)))
    }).collect();
    let mut spares = SPARES.lock().unwrap();
    let mut release = Vec::new();
    let mut runs = Vec::new();
    // a spare whose leg waits in the queue at its box while boxes it may warm could start it now
    // moves to them: never a fixed full box (a lane's warm waited 4 min at a full box, its every
    // ask saying four others could start it, 11-07)
    let moves: BTreeMap<String, Vec<String>> = spares.lanes.iter().filter(|(lane, s)| spares.running.contains_key(*lane) && s.progress.lease.is_none() && s.progress.state == "waiting").filter_map(|(lane, s)| {
        let (l, group) = needing.get(lane)?;
        let free = ctx.leases.starts_elsewhere(&format!("{WARM_LANE_PREFIX}{lane}"));
        let free = free_spares(nodes, &free, l, group, s.native, activity);
        (!free.is_empty()).then(|| (lane.clone(), free))
    }).collect();
    // one with nowhere to move gives up once it waited past what it saves ([`waited_out`])
    let waited: BTreeMap<String, u64> = spares.lanes.iter().filter(|(lane, _)| !moves.contains_key(*lane)).filter_map(|(lane, s)| {
        let value = ctx.leases.jit_build_ms(&s.node);
        waited_out(s, spares.running.contains_key(lane), now, value).then(|| (lane.clone(), value))
    }).collect();
    let remove: Vec<_> = spares.lanes.iter().filter(|(lane, s)| {
        moves.contains_key(*lane) || waited.contains_key(*lane) || !needing.get(lane).is_some_and(|(l, group)| s.native == (l.src == Src::Tree) && nodes.iter().any(|n| n.name == s.node && spare_ok(n, l, group, pool_in_group(nodes, group))))
    }).map(|(lane, _)| lane.clone()).collect();
    for lane in remove {
        let s = spares.lanes.remove(&lane).unwrap();
        if let Some(free) = moves.get(&lane) {
            ctx.store.event("warm", Sev::Info, format!("warm {lane}: its leg waited at {}, which has no room, while {} could start it: it moves", s.node, free.join(" ")), None);
        }
        if let Some(value) = waited.get(&lane) {
            ctx.store.event("warm", Sev::Info, format!("warm {lane}: its leg waited {} s at {}, past what it saves there (a JIT build there, {} s), and no box it may warm could start it: dropped (the lane's next lease builds anyway)", now.saturating_sub(s.progress.started_ms) / 1000, s.node, value / 1000), None);
        }
        // Serialize this with finish's note_warm: an old completion cannot restore the mark.
        if !s.native { ctx.leases.forget_warm(&lane, &s.node); }
        if let Some(id) = s.progress.lease { release.push(id); }
    }
    spares.last_started.retain(|lane, _| activity.lanes.contains_key(lane));
    let mut assigned = BTreeMap::<String, usize>::new();
    for s in spares.lanes.values() { *assigned.entry(s.node.clone()).or_default() += 1; }
    for (lane, (l, group)) in needing {
        let native = l.src == Src::Tree;
        if !spares.lanes.contains_key(lane) {
            let pooled = pool_in_group(nodes, group);
            let moving = moves.get(lane.as_str());
            let candidate = nodes.iter().filter(|n| spare_ok(n, l, group, pooled) && n.caps.as_ref().is_some_and(|c| c.ram_avail_gb >= 8.0 && (!native || native_room(c, activity))) && moving.is_none_or(|free| free.contains(&n.name))).min_by(|a, b| {
                let (ac, bc) = (a.caps.as_ref().unwrap(), b.caps.as_ref().unwrap());
                assigned.get(&a.name).copied().unwrap_or(0).cmp(&assigned.get(&b.name).copied().unwrap_or(0))
                    .then_with(|| ac.cpu_pct.unwrap_or(100.0).total_cmp(&bc.cpu_pct.unwrap_or(100.0)))
                    .then_with(|| (ac.load1 / ac.cores.max(1) as f64).total_cmp(&(bc.load1 / bc.cores.max(1) as f64)))
                    .then_with(|| a.name.cmp(&b.name))
            });
            let Some(n) = candidate else { continue };
            *assigned.entry(n.name.clone()).or_default() += 1;
            spares.lanes.insert(lane.clone(), Spare { node: n.name.clone(), native, generation: 0, warmed: None, progress: Progress::waiting("", now) });
        }
        // An invalidated worker may still be returning from its released node call.
        if spares.running.contains_key(lane) { continue; }
        let last_started = spares.last_started.get(lane).copied();
        let s = spares.lanes.get_mut(lane).unwrap();
        let caps = nodes.iter().find(|n| n.name == s.node).and_then(|n| n.caps.as_ref()).unwrap();
        let id = if native { Some("") } else { caps.jit_store.as_ref().and_then(|store| compilers.iter().position(|c| *c == store.compiler)).and_then(|i| l.hint.get(i)).filter(|id| !id.is_empty()).map(String::as_str) };
        let Some(id) = id.filter(|_| !l.rev.is_empty()) else {
            s.progress.state = "waiting".into();
            s.progress.note = if native { "waiting for the lane's tree revision" } else { "waiting for the lane pin's compiler hint" }.into();
            continue;
        };
        if native && !caps.native.contains(lane) {
            s.warmed = None;
            if !native_room(caps, activity) {
                s.progress.state = "waiting".into();
                s.progress.note = "waiting for a native slot".into();
                continue;
            }
        }
        let packages = if native { &l.native_packages } else { &l.packages };
        let key = Key { id: id.to_string(), rev: l.rev.clone(), packages: packages.iter().map(|(p, _)| p.clone()).collect() };
        if s.warmed.as_ref() == Some(&key) {
            if s.progress.state != "warm" {
                s.progress.state = "warm".into();
                s.progress.id = id.to_string();
                s.progress.note.clear();
                s.progress.ended_ms = now;
            }
            continue;
        }
        if let Some(t) = last_started.filter(|t| now.saturating_sub(*t) < REWARM_MS) {
            if s.progress.state != "failed" {
                if s.progress.state != "waiting" { s.progress = Progress::waiting(id, now); }
                s.progress.id = id.to_string();
                s.progress.note = format!("next warm in {} s", (REWARM_MS - now.saturating_sub(t)).div_ceil(1000));
            }
            continue;
        }
        let node = s.node.clone();
        spares.generation += 1;
        let generation = spares.generation;
        let s = spares.lanes.get_mut(lane).unwrap();
        s.generation = generation;
        s.progress = Progress::waiting(id, now);
        spares.running.insert(lane.clone(), generation);
        spares.last_started.insert(lane.clone(), now);
        runs.push(Run { generation, lane: Some(lane.clone()), native, apps: false, node, os: caps.os.clone(), key, started_ms: now, since_ms: None, adopt: None });
    }
    drop(spares);
    for id in release { ctx.leases.release_live(&id); }
    spawn_all(ctx, runs);
}

/// The measurement class of the lanes' JIT warm legs, whatever they run.
const WARM_CLASS: &str = "build jit warm";

/// The measurement class of a box's prebuild: its JIT's build, then the listing of each package
/// its lanes used, each compiled under that JIT. Measured with the lanes' warm legs, whose peaks
/// are a spare JIT's 0.06 GB, a 16 GB Mac's prebuild asked 1.58 GB, listed a firmware test whose
/// compile peaks at 3.2-3.8 GB beside 3.5 GB of other work, and the system killed it (11-19).
const PREBUILD_CLASS: &str = "build jit prebuild";

fn spawn(ctx: &Arc<Ctx>, run: Run) {
    LEGS.lock().unwrap().insert(leg_key(&run), run.clone());
    persist();
    let (c, r) = (ctx.clone(), run.clone());
    if let Err(e) = std::thread::Builder::new().name(format!("{AGENT}-{}", run.lane.as_deref().unwrap_or(&run.node))).spawn(move || run_leg(&c, &r)) {
        finish(ctx, &run, "", Err(format!("thread: {e}")));
    }
}

/// Starts legs, the whole batch in the state before the first starts: a restart in between takes
/// back every one ([`take_back`]).
fn spawn_all(ctx: &Arc<Ctx>, runs: Vec<Run>) {
    {
        let mut legs = LEGS.lock().unwrap();
        for run in &runs { legs.insert(leg_key(run), run.clone()); }
    }
    persist();
    for run in runs { spawn(ctx, run); }
}

fn log_name(run: &Run) -> String {
    match (&run.lane, run.apps) {
        (Some(lane), _) => format!("lane-{lane}.log"),
        (None, true) => format!("{}-apps.log", run.node),
        (None, false) => format!("{}.log", run.node),
    }
}

/// A box prebuild's leg asks while registered here (box → the plan generation it asks for), so
/// that a newer plan leaves the box to it: the queued request follows the new target in place
/// ([`Held::acquire_following`]) and keeps its place and age, where a new leg's ask started over
/// at the back of the queue (one box's prebuild was asked again ten times an evening and never
/// ran, 11-04). Touched only under [`PLAN`]'s lock, which orders it with plan changes.
static ASKING: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// Takes the newest plan's generation and target for the leg's box, when that plan has it;
/// false: it does not (the box got its JIT another way, or went down). Under [`PLAN`]'s lock.
fn adopt(plan: &Option<Plan>, run: &mut Run) -> bool {
    let Some(p) = plan.as_ref() else { return false };
    if p.generation == run.generation { return true; }
    let Some(n) = p.nodes.iter().find(|n| n.name == run.node) else { return false };
    (run.generation, run.key) = (p.generation, n.key.clone());
    true
}

fn run_leg(ctx: &Ctx, run: &Run) {
    if run.apps {
        let result = apps_leg(ctx, run);
        finish(ctx, run, "", result);
        return;
    }
    let run = std::cell::RefCell::new(run.clone());
    let prebuild = run.borrow().lane.is_none() && !run.borrow().native;
    let mut dropped = String::new();
    let result = (|| -> Result<String, String> {
        let dir = ctx.logs("warm");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Append: an old generation can still be returning when its successor starts.
        let mut log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(log_name(&run.borrow()))).map_err(|e| e.to_string())?;
        {
            let r = run.borrow();
            let _ = writeln!(log, "warm generation {}: {} {}", r.generation, r.key.rev, r.key.id);
        }
        let spec_of = |r: &Run| {
            let (lane, prio) = match &r.lane {
                Some(lane) => (format!("{WARM_LANE_PREFIX}{lane}"), leg::BACKGROUND_PRIO),
                None => (format!("{PREBUILD_LANE_PREFIX}{}", r.node), leg::OWED_PRIO),
            };
            Spec { lane, agent: AGENT.into(), kind: Kind::Build, needs: String::new(), prio, rev: r.key.rev.clone(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: Some(r.node.clone()), since_ms: r.since_ms }
        };
        if run.borrow().native {
            let r = run.borrow().clone();
            let (packages, note) = resolve(&ctx.repo, &r.key.rev, &r.key.packages, false);
            dropped = note;
            if !dropped.is_empty() { let _ = writeln!(log, "{dropped}"); }
            return native_leg(&r, &spec_of(&r), &packages, &mut log);
        }
        // A JIT appearing in caps is no longer completion: the code cache still needs warming.
        let stale = || !set(&run.borrow(), |_| {});
        // a box prebuild still asking is given up only when the newest plan has no work for its box
        let gone = || {
            let r = run.borrow();
            let plan = PLAN.lock().unwrap();
            plan.as_ref().is_none_or(|p| p.generation != r.generation && !p.nodes.iter().any(|n| n.name == r.node))
        };
        let give_up = || if prebuild { gone() } else { stale() };
        // the lease a previous master held it on: taken back once, its runs asked again by name
        let mut adopted = run.borrow_mut().adopt.take();
        loop {
            if stale() { return Ok(String::new()); }
            let (asked, ship) = if let Some((lease, shipped)) = adopted.take() {
                let r = run.borrow();
                let mut held = Held::attach(&lease, &r.node);
                held.rev = r.key.rev.clone();
                let _ = writeln!(log, "{lease} on {}: taken back after the master's restart; its runs are asked again by name", r.node);
                (Ok(held), !shipped)
            } else {
                if prebuild {
                    let r = run.borrow();
                    let plan = PLAN.lock().unwrap();
                    if !plan.as_ref().is_some_and(|p| p.generation == r.generation) { return Ok(String::new()); }
                    ASKING.lock().unwrap().insert(r.node.clone(), r.generation);
                }
                let spec = spec_of(&run.borrow());
                let mut on_wait = |line: &str| {
                    let _ = writeln!(log, "{line}");
                    set(&run.borrow(), |p| p.note = line.into());
                };
                // a newer plan's target for its box, taken in place while it asks
                let mut follow = || {
                    if !prebuild { return None; }
                    let to = {
                        let mut r = run.borrow_mut();
                        let plan = PLAN.lock().unwrap();
                        if plan.as_ref().is_none_or(|p| p.generation == r.generation) { return None; }
                        let before = r.key.rev.clone();
                        if !adopt(&plan, &mut r) { return None; }
                        ASKING.lock().unwrap().insert(r.node.clone(), r.generation);
                        (r.key.rev != before).then(|| r.key.rev.clone())
                    };
                    moved(&run.borrow());
                    to
                };
                let asked = Held::acquire_following(&spec, &|r| {
                    r.src = Src::Overlay;
                    r.cores = Some(if prebuild { 4.0 } else { 2.0 });
                    // RAM as measured (a dev JIT build and a test listing peak at 2.4 GB): an 8 GB ask
                    // kept prebuilds off the 16 GB boxes beside a live session for minutes (11-02).
                    // Measured under a class of their own: unnamed, they were measured as nothing and
                    // took a build's 12 GB default (three pool boxes' prebuilds waited 23 min an hour
                    // on RAM, holding their box meanwhile, 11-03)
                    r.class = if prebuild { PREBUILD_CLASS } else { WARM_CLASS }.into();
                    if run.borrow().os == "macos" { r.needs.push(("os".into(), "macos".into())); }
                }, &mut on_wait, &give_up, &mut follow);
                if prebuild {
                    // out of the asking legs; a plan that came meanwhile left its box to this leg
                    let kept = {
                        let mut r = run.borrow_mut();
                        let plan = PLAN.lock().unwrap();
                        let mut asking = ASKING.lock().unwrap();
                        if asking.get(&r.node) == Some(&r.generation) { asking.remove(&r.node); }
                        drop(asking);
                        adopt(&plan, &mut r)
                    };
                    if !kept {
                        if let Ok(held) = asked { held.release(None); }
                        return Ok(String::new());
                    }
                    moved(&run.borrow());
                }
                (asked, true)
            };
            let held = match asked {
                Ok(held) => held,
                Err(_) if stale() => return Ok(String::new()),
                Err(e) if e.split_whitespace().next().is_some_and(leg::was_preempted) => continue,
                Err(e) if missing_commit(&e) => {
                    let _ = writeln!(log, "{e}; retrying in {} s", RETRY.as_secs());
                    let r = run.borrow().clone();
                    set(&r, |p| p.note = format!("waiting for commit: {e}"));
                    ctx.store.event("warm", Sev::Warn, format!("warm {} on {}: {e}; retrying", r.key.rev, r.node), None);
                    let until = Instant::now() + RETRY;
                    while Instant::now() < until && !stale() { std::thread::sleep(Duration::from_secs(1)); }
                    continue;
                }
                Err(e) => return Err(e),
            };
            // granted at a target a newer plan replaced before it could follow: asked again at the
            // newest, keeping its age (`since_ms`)
            if held.rev != run.borrow().key.rev {
                let _ = writeln!(log, "{} granted at {}, superseded by {}: asking again in its place", held.id, held.rev, run.borrow().key.rev);
                held.release(None);
                continue;
            }
            let r = run.borrow().clone();
            // (taken back, its listings are asked again: the box answers each from its run)
            if !set(&r, |p| { p.state = "held".into(); p.note.clear(); p.lease = Some(held.id.clone()); p.listed.clear(); }) { return Ok(String::new()); }
            persist();
            let (packages, note) = resolve(&ctx.repo, &r.key.rev, &r.key.packages, true);
            dropped = note;
            if !dropped.is_empty() { let _ = writeln!(log, "{dropped}"); }
            let mut failures = Vec::new();
            // each step asked by its name in the lease: a master that takes the lease back asks
            // again, and the box answers with the run the first ask started (the build goes on)
            let result = (if ship { held.ship_nothing() } else { Ok(()) }).and_then(|_| {
                if stale() { return Ok(0); }
                set(&r, |p| p.state = "building".into());
                persist();
                let argv = ["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "--version"].map(String::from);
                let code = held.run_asked("warm-jit", ".", &argv, &mut log, &mut |_| {})?;
                if code != 0 { return Ok(code); }
                for package in &packages {
                    if stale() { break; }
                    let label = package.label();
                    set(&r, |p| p.state = format!("warming {label}"));
                    let _ = writeln!(log, "warming {label}");
                    let mut tee = Tee(&mut log, Vec::new());
                    let result = held.run_asked(&format!("warm-list-{label}"), ".", &package.argv(), &mut tee, &mut |_| {});
                    // a listing that ran is a verdict on the tip (a lease's error is none); a failed
                    // one keeps its output (its errors), a passing one only its line
                    if let Ok(code) = result {
                        let unit = format!("suite {}", package.pkg);
                        let head = format!("{label} on {}: exit {code}", r.node);
                        let text = if code == 0 { format!("{head}\n") } else {
                            let output = String::from_utf8_lossy(&tee.1).into_owned();
                            let kept = keep_listing(&ctx.logs(super::job::LEASE_LOGS), &held.id, &unit, &head, &output);
                            failed_listing(&unit, &head, &output, code, kept.as_ref().ok().map(|_| held.id.as_str()))
                        };
                        set(&r, |p| p.listed.push(Listed { unit, ok: code == 0, text }));
                    }
                    let failure = match result {
                        Ok(0) => None,
                        Ok(code) => Some(format!("{label}: exit {code}")),
                        Err(e) => Some(format!("{label}: {e}")),
                    };
                    if let Some(e) = failure {
                        let _ = writeln!(log, "warm skipped {e}");
                        failures.push(e);
                    }
                }
                // Cache-warm failures do not fail the JIT build or skip subsequent packages.
                Ok(0)
            });
            let preempted = leg::was_preempted(&held.id);
            held.release(result.as_ref().ok().copied());
            set(&r, |p| p.lease = None);
            if stale() { return Ok(String::new()); }
            if preempted {
                let _ = writeln!(log, "preempted for lane work; asking again");
                set(&r, |p| { p.state = "waiting".into(); p.note = "preempted for lane work".into(); });
                continue;
            }
            return match result { Ok(0) => Ok(failures.join("; ")), Ok(code) => Err(format!("exit {code}")), Err(e) => Err(e) };
        }
    })();
    let run = run.into_inner();
    finish(ctx, &run, &dropped, result);
}

/// The measurement class of a box's app pass ([`apps_leg`]).
const APPS_CLASS: &str = "warm jit apps";

/// A box's app pass, after its prebuild built the new JIT: each app lanes ran on overlays there in
/// the last [`APPS_MS`], the most-run first, started once under that JIT and ended by itself once
/// its startup went quiet after the first present (`makepad run --quit-after-present`: a graceful
/// quit, so the run's code cache is committed), so a lane's first run of it there finds that
/// JIT's code (11-19: 185 of 193 first app runs per box and dev JIT were cold; Fab's first frame
/// 25.5 s cold against 6.1 s warm, medians). It runs as background work (lane work goes first and
/// preempts it, which asks again for the apps not run yet); the next plan ends it and releases its
/// lease ([`plan_tick`]). Each app runs hidden, in an app home of its own: never the box's own
/// app state. An app that fails is noted; the rest go on.
fn apps_leg(ctx: &Ctx, run: &Run) -> Result<String, String> {
    let dir = ctx.logs("warm");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(log_name(run))).map_err(|e| e.to_string())?;
    let apps = {
        let plan = PLAN.lock().unwrap();
        let n = plan.as_ref().filter(|p| p.generation == run.generation).and_then(|p| p.nodes.iter().find(|n| n.name == run.node));
        match n { Some(n) => n.apps.clone(), None => return Ok(String::new()) }
    };
    let _ = writeln!(log, "warm generation {} apps: {} {}: {}", run.generation, run.key.rev, run.key.id, apps.join(", "));
    // the app home every app starts in: an empty archive, unpacked afresh for each lease
    let archive = crate::proto::home().join("warm-apps-home.tar");
    if !archive.exists() { std::fs::write(&archive, [0u8; 1024]).map_err(|e| format!("{}: {e}", archive.display()))?; }
    let lane = format!("{WARM_LANE_PREFIX}apps-{}", run.node);
    let stale = || !set(run, |_| {});
    let mut ran: Vec<String> = Vec::new();
    let mut failures = Vec::new();
    let mut adopted = run.adopt.clone();
    loop {
        if stale() { return Ok(String::new()); }
        let (asked, ship) = match adopted.take() {
            Some((lease, shipped)) => {
                let mut held = Held::attach(&lease, &run.node);
                held.rev = run.key.rev.clone();
                let _ = writeln!(log, "{lease} on {}: taken back after the master's restart; its runs are asked again by name", run.node);
                (Ok(held), !shipped)
            }
            None => {
                // (the kind of a hidden live session's lease, whose environment its app runs in)
                let spec = Spec { lane: lane.clone(), agent: AGENT.into(), kind: Kind::Test, needs: String::new(), prio: leg::BACKGROUND_PRIO, rev: run.key.rev.clone(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: Some(run.node.clone()), since_ms: run.since_ms };
                let mut on_wait = |line: &str| {
                    let _ = writeln!(log, "{line}");
                    set(run, |p| p.note = line.into());
                };
                let asked = Held::acquire_with(&spec, &|r| {
                    r.src = Src::Overlay;
                    r.cores = Some(4.0);
                    r.class = APPS_CLASS.into();
                    if run.os == "macos" { r.needs.push(("os".into(), "macos".into())); }
                }, &mut on_wait, &stale);
                (asked, true)
            }
        };
        let held = match asked {
            Ok(held) => held,
            Err(_) if stale() => return Ok(String::new()),
            Err(e) if e.split_whitespace().next().is_some_and(leg::was_preempted) => continue,
            Err(e) => return Err(e),
        };
        if !set(run, |p| { p.state = "held".into(); p.note.clear(); p.lease = Some(held.id.clone()); }) {
            held.release(None);
            return Ok(String::new());
        }
        persist();
        let mut preempted = false;
        let result = (if ship { held.ship_nothing() } else { Ok(()) }).and_then(|_| {
            let home = crate::lease::ship::send_home(held.client(), &held.node, &held.id, &archive)?;
            for app in &apps {
                if ran.contains(app) { continue; }
                if stale() { break; }
                set(run, |p| p.state = format!("warming {app}"));
                persist();
                let _ = writeln!(log, "warming {app}");
                let mut argv: Vec<String> = ["env", "MAKEPAD_HIDE_WINDOWS=1", "RELAX_STORE_LIVE=1"].map(String::from).to_vec();
                argv.extend([format!("MAKEPAD_TEST_HOME={home}"), format!("MAKEPAD_TEST_RUN={lane} {}", held.id)]);
                argv.extend(["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "run", "--quit-after-present", app.as_str()].map(String::from));
                let result = held.run_asked(&format!("warm-app-{app}"), ".", &argv, &mut log, &mut |_| {});
                if leg::was_preempted(&held.id) { preempted = true; break; }
                ran.push(app.clone());
                let failure = match result {
                    Ok(0) => None,
                    Ok(code) => Some(format!("{app}: exit {code}")),
                    Err(e) => Some(format!("{app}: {e}")),
                };
                if let Some(e) = failure {
                    let _ = writeln!(log, "warm skipped {e}");
                    failures.push(e);
                }
            }
            Ok(0)
        });
        let preempted = preempted || leg::was_preempted(&held.id);
        held.release(result.as_ref().ok().copied());
        set(run, |p| p.lease = None);
        if stale() { return Ok(String::new()); }
        if preempted {
            let _ = writeln!(log, "preempted for lane work; asking again for the apps not run yet");
            set(run, |p| { p.state = "waiting".into(); p.note = "preempted for lane work".into(); });
            continue;
        }
        return result.map(|_| failures.join("; "));
    }
}

/// The boxes whose `nodes.conf` line says `warm_apps=off`: no app pass there (a person's own
/// machine in the pool: no hidden app runs on it; a cold first start after an update is fine there).
fn apps_off(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter_map(|line| {
        let mut w = line.split('#').next().unwrap_or("").split_whitespace();
        let name = w.next()?;
        w.any(|kv| kv == "warm_apps=off").then(|| name.to_string())
    }).collect()
}

/// Per box, the apps lanes ran there on overlays since `since_ms` (live sessions, not tests), the
/// most-run first. The store appends each live record to `records.jsonl` as it changes; read back
/// from the end, the newest line of each session counts, until a chunk whose sessions all started
/// before `since_ms` (24 h of them: about 16 MB, 11-19).
fn apps_by_box(path: &Path, since_ms: u64) -> BTreeMap<String, Vec<String>> {
    const CHUNK: u64 = 4 << 20;
    // per session id: its (box, app) when it counts
    let mut sessions: BTreeMap<String, Option<(String, String)>> = BTreeMap::new();
    let Ok(mut file) = std::fs::File::open(path) else { return BTreeMap::new() };
    let mut end = file.metadata().map_or(0, |m| m.len());
    // the start of the line the last chunk began inside of
    let mut carry: Vec<u8> = Vec::new();
    while end > 0 {
        let start = end.saturating_sub(CHUNK);
        let mut bytes = vec![0; (end - start) as usize];
        if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut bytes).is_err() { break; }
        bytes.extend_from_slice(&carry);
        let whole = if start == 0 { 0 } else { bytes.iter().position(|b| *b == b'\n').map_or(bytes.len(), |i| i + 1) };
        carry = bytes[..whole].to_vec();
        // (a chunk of other records, a CI run's, says nothing of the window)
        let (mut any, mut recent) = (false, false);
        for line in bytes[whole..].split(|b| *b == b'\n').rev() {
            if !line.starts_with(b"{\"id\":\"W") { continue; }
            let Ok(v) = makepad_strict_json::parse(line) else { continue };
            let Some(id) = v.get("id").and_then(Value::as_str) else { continue };
            let started = v.get("started_ms").and_then(Value::as_u64).unwrap_or(0);
            any = true;
            recent |= started >= since_ms;
            if sessions.contains_key(id) { continue; }
            let counts = started >= since_ms && v.get("src").and_then(Value::as_str) == Some("overlay") && v.get("test").and_then(Value::as_bool) == Some(false);
            let run = counts.then(|| Some((v.get("node")?.as_str()?.to_string(), v.get("app")?.as_str()?.to_string()))).flatten().filter(|(n, a)| !n.is_empty() && !a.is_empty());
            sessions.insert(id.to_string(), run);
        }
        if any && !recent { break; }
        end = start;
    }
    let mut counts: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for (node, app) in sessions.into_values().flatten() { *counts.entry(node).or_default().entry(app).or_default() += 1; }
    counts.into_iter().map(|(node, apps)| {
        let mut apps: Vec<(String, usize)> = apps.into_iter().collect();
        apps.sort_by(|(a, n), (b, m)| m.cmp(n).then_with(|| a.cmp(b)));
        (node, apps.into_iter().map(|(a, _)| a).collect())
    }).collect()
}

fn native_leg(run: &Run, spec: &Spec, packages: &[Package], log: &mut dyn Write) -> Result<String, String> {
    let stale = || !set(run, |_| {});
    if stale() { return Ok(String::new()); }
    let held = if let Some((lease, _)) = &run.adopt {
        let mut held = Held::attach(lease, &run.node);
        held.rev = run.key.rev.clone();
        let _ = writeln!(log, "{lease} on {}: taken back after the master's restart; its runs are asked again by name", run.node);
        held
    } else {
        let mut on_wait = |line: &str| {
            let _ = writeln!(log, "{line}");
            set(run, |p| p.note = line.into());
        };
        match Held::acquire_with(spec, &|r| {
            r.warms = run.lane.clone();
            r.cores = Some(4.0);
            r.ram_gb = Some(8.0);
            if run.os == "macos" { r.needs.push(("os".into(), "macos".into())); }
        }, &mut on_wait, &stale) {
            Ok(held) => held,
            Err(e) => {
                if let Some(id) = e.split_whitespace().next() { let _ = leg::was_preempted(id); }
                return Err(e);
            }
        }
    };
    if !set(run, |p| { p.state = "held".into(); p.note.clear(); p.lease = Some(held.id.clone()); }) { return Ok(String::new()); }
    persist();
    // each step asked by its name in the lease, as a box prebuild's ([`run_leg`])
    let result = (|| -> Result<i32, String> {
        if stale() { return Ok(0); }
        if packages.is_empty() {
            set(run, |p| p.state = "syncing".into());
            return held.run_asked("warm-sync", ".", &["git", "rev-parse", "HEAD"].map(String::from), log, &mut |_| {});
        }
        for package in packages {
            if stale() { return Ok(0); }
            let label = package.label();
            set(run, |p| p.state = format!("warming {label}"));
            let _ = writeln!(log, "warming native {label}");
            let code = held.run_asked(&format!("warm-native-{label}"), ".", &package.native_argv(&run.os), log, &mut |_| {})?;
            if code != 0 { return Ok(code); }
        }
        Ok(0)
    })();
    let preempted = leg::was_preempted(&held.id);
    held.release(result.as_ref().ok().copied());
    set(run, |p| p.lease = None);
    if preempted { return Err("preempted for lane work".into()); }
    match result { Ok(0) => Ok(String::new()), Ok(code) => Err(format!("exit {code}")), Err(e) => Err(e) }
}

fn missing_commit(e: &str) -> bool {
    // lease-start reports the layer and Git's diagnostic when autosync has not arrived yet.
    e.contains("overlay:") && ["bad object", "bad revision", "not a valid object", "unknown revision", "no commit", "commit not kept", "missing store", "not our ref"].iter().any(|s| e.contains(s))
}

fn finish(ctx: &Ctx, run: &Run, dropped: &str, result: Result<String, String>) {
    // The leg leaves the state together with its end, under [`persist`]'s lock: a state written
    // between the two would have a restarted master run it again, or never end it.
    let persisted = PERSISTED.lock().unwrap();
    LEGS.lock().unwrap().remove(&leg_key(run));
    let ended_ms = wall_ms();
    let seconds = ended_ms.saturating_sub(run.started_ms) / 1000;
    let (sev, state, result_note) = match result {
        Ok(note) => (if note.is_empty() { Sev::Info } else { Sev::Warn }, if run.lane.is_some() { "warm" } else { "done" }, note),
        Err(e) => (Sev::Warn, "failed", e),
    };
    let note = [dropped, &result_note].into_iter().filter(|n| !n.is_empty()).collect::<Vec<_>>().join("; ");
    let update = |p: &mut Progress| { p.state = state.into(); p.note = note.clone(); p.ended_ms = ended_ms; p.lease = None; };
    let changed = if let Some(lane) = &run.lane {
        let mut spares = SPARES.lock().unwrap();
        if spares.running.get(lane) == Some(&run.generation) { spares.running.remove(lane); }
        if let Some(s) = spares.lanes.get_mut(lane).filter(|s| s.generation == run.generation) {
            update(&mut s.progress);
            if state == "warm" {
                s.warmed = Some(run.key.clone());
                if !run.native { ctx.leases.note_warm(lane, &run.node); }
            }
            true
        } else { false }
    } else { set(run, update) };
    drop(persisted);
    if changed && run.lane.is_none() && !run.apps {
        let done = listings_done(PLAN.lock().unwrap().as_mut(), run.generation);
        if let Some(plan) = done { judge(ctx, &plan, false); }
        // the box's app pass follows its prebuild, while its plan is the newest
        let pass = (state == "done").then(|| {
            let plan = PLAN.lock().unwrap();
            let n = plan.as_ref().filter(|p| p.generation == run.generation)?.nodes.iter().find(|n| n.name == run.node)?;
            n.apps_progress.as_ref().filter(|a| a.ended_ms == 0 && a.lease.is_none())?;
            Some(Run { generation: run.generation, lane: None, native: false, apps: true, node: n.name.clone(), os: n.os.clone(), key: n.key.clone(), started_ms: ended_ms, since_ms: Some(ended_ms), adopt: None })
        }).flatten();
        if let (Some(pass), Some(c)) = (pass, super::ctx()) { spawn(c, pass); }
    }
    if changed {
        let who = match (&run.lane, run.apps) { (Some(lane), _) => lane.clone(), (None, true) => format!("{} apps", run.key.rev), (None, false) => run.key.rev.clone() };
        let who = who.as_str();
        let line = |note: &str| format!("warm {who} on {}: {state} in {seconds} s{}", run.node, if note.is_empty() { String::new() } else { format!(": {note}") });
        if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(ctx.logs("warm").join(log_name(run))) {
            // Drops were logged before acquiring the lease.
            let _ = writeln!(log, "{}", line(&result_note));
        }
        ctx.store.event("warm", sev, line(&note), None);
    }
    persist();
}

/// The plan of `generation` once every box of it that lists tests has ended: judged once, by the
/// last of them to end. A box that lists none (idle, bench) is not waited for.
fn listings_done(plan: Option<&mut Plan>, generation: u64) -> Option<Plan> {
    let plan = plan.filter(|p| p.generation == generation && !p.judged && p.nodes.iter().all(|n| n.key.packages.is_empty() || n.progress.ended_ms != 0))?;
    plan.judged = true;
    Some(plan.clone())
}

/// A plan's test listings as the `prebuild` bucket's verdict on its tip: every box's that listed
/// (`superseded`: the next JIT came before the rest did). A plan that listed nothing is no verdict.
fn judge(ctx: &Ctx, plan: &Plan, superseded: bool) {
    let listed: Vec<&Listed> = plan.nodes.iter().flat_map(|n| &n.progress.listed).collect();
    if listed.is_empty() { return; }
    let mut failing: Vec<String> = listed.iter().filter(|l| !l.ok).map(|l| l.unit.clone()).collect();
    failing.sort();
    failing.dedup();
    let boxes = plan.nodes.iter().filter(|n| !n.progress.listed.is_empty()).count();
    let summary = format!("{} test listings on {boxes} boxes under JIT {}{}", listed.len(), plan.ids.join(", "), if superseded { "; the next JIT came before the rest" } else { "" });
    let mut text = format!("prebuild {}: {summary}\n", plan.pair.short());
    for l in &listed {
        text.push_str(&l.text);
        if !text.ends_with('\n') { text.push('\n'); }
        text.push_str(&format!("GATEUNIT {} {}\n", if l.ok { "ok" } else { "fail" }, l.unit));
    }
    let logs = ctx.home.join("bucket-logs");
    let log = logs.join(format!("prebuild-{}.log", plan.started_ms));
    let written = std::fs::create_dir_all(&logs).and_then(|_| std::fs::write(&log, &text));
    if let Err(e) = written {
        ctx.store.event("warm", Sev::Warn, format!("prebuild {}: its listings' log: {e}", plan.pair.short()), None);
        return;
    }
    let outcome = crate::bucket::Outcome { ok: failing.is_empty(), failing, log: log.display().to_string(), summary, no_verdict: None };
    if let Err(e) = crate::bucket::prebuild(&ctx.home, &ctx.repo, &ctx.store, plan.started_ms, plan.base.clone(), &plan.pair, outcome) {
        ctx.store.event("warm", Sev::Warn, format!("prebuild {}: no verdict: {e}", plan.pair.short()), None);
    }
}

/// What a failed listing keeps in its bucket's log: its line, its output's last [`LISTING_TAIL`]
/// bytes, what its end says when no error of its own does ([`exit_said`]: an `EXIT: ` line, which
/// the bucket names its red by) and the lease its whole stream is kept under (`GATEKEPT <unit> ::
/// <lease>`: `devhub job result <lease>` shows it, and the bucket's line points there).
fn failed_listing(unit: &str, head: &str, output: &str, code: i32, kept: Option<&str>) -> String {
    let mut text = format!("{head}\n{}", tail_of(output, LISTING_TAIL));
    if !text.ends_with('\n') { text.push('\n'); }
    if let Some(said) = exit_said(output, code) { text.push_str(&format!("EXIT: {said}\n")); }
    if let Some(lease) = kept { text.push_str(&format!("GATEKEPT {unit} :: {lease}\n")); }
    text
}

/// The bytes of a failed listing's output its bucket's log keeps: its end, where a run says why it
/// stopped; the whole stream is its lease's ([`keep_listing`]).
const LISTING_TAIL: usize = 64 << 10;

/// The last `max` bytes of `text` or fewer, from a line's start.
fn tail_of(text: &str, max: usize) -> &str {
    if text.len() <= max { return text; }
    let mut at = text.len() - max;
    while !text.is_char_boundary(at) { at += 1; }
    text[at..].find('\n').map_or(&text[at..], |n| &text[at + n + 1..])
}

/// Adds a failed listing's whole stream to its lease's kept log (`lease-logs/<lease>.log`), where
/// `devhub job result <lease>` reads it; once, as a master that took the lease back asks the box
/// again and hears the same run. A warm leg's lease is released as its JIT build ended, which
/// keeps nothing: `job result L1348447` said "kept no output" of a listing's exit 101 (11-18).
fn keep_listing(logs: &std::path::Path, lease: &str, unit: &str, head: &str, output: &str) -> std::io::Result<()> {
    let path = logs.join(format!("{lease}.log"));
    let said = format!("== {unit} failed: {head}; its whole stream");
    if std::fs::read_to_string(&path).is_ok_and(|t| t.lines().any(|l| l == said)) { return Ok(()); }
    std::fs::create_dir_all(logs)?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    write!(f, "{said}\n{output}{}", if output.is_empty() || output.ends_with('\n') { "" } else { "\n" })
}

/// What a failed listing's end says when its output holds no error of its own (no compile error
/// and no panic: the bucket names its red by those): `exit after listing` when its stream ends on
/// the whole listings of the targets it started (each `Running` line's `N tests, M benchmarks`, and
/// after the last only notes of warnings and doc tests), naming them; else where it stopped. None:
/// its errors say why. fab-elec-simjit's listing ended on its tests' count and exited 101 without a
/// word; the prebuild bucket's red named only its unit, and its lane chased its own tests (11-18).
fn exit_said(output: &str, code: i32) -> Option<String> {
    let lines: Vec<&str> = output.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.iter().any(|l| l.contains("panicked at") || crate::bucket::error_key(l).is_some()) { return None; }
    let note = |l: &str| l.starts_with("relax test: ") && (l.contains(" warning") || l.contains(" doc test"));
    let (mut listed, mut open, mut after) = (Vec::new(), None, false);
    for &l in &lines {
        if let Some(target) = l.strip_prefix("Running ") {
            open = Some(target);
            after = false;
        } else if listing_summary(l) {
            if let Some(target) = open.take() { listed.push(target); }
            after = false;
        } else if open.is_none() && !listed.is_empty() && !note(l) {
            after = true;
        }
    }
    let n = listed.len();
    let named = if n > 4 { format!("{} and {} more", listed[..3].join(", "), n - 3) } else { listed.join(", ") };
    Some(match open {
        None if n > 0 && !after => format!("{}: exit {code} with no error in its output once it had listed {n} target{} whole ({named}); --list runs no test, so the exit is the runner's own", crate::bucket::EXIT_AFTER_LISTING, if n == 1 { "" } else { "s" }),
        Some(target) => format!("exit {code} with no compile error or panic in its output, inside its listing of {target} (no count of its tests came)"),
        None => format!("exit {code} with no compile error or panic in its output; {}", lines.last().map_or("it printed nothing".to_string(), |l| format!("its last line: {}", crate::proto::clean(l, 200)))),
    })
}

/// libtest's count at the end of a target's `--list`: `N tests, M benchmarks` (either singular).
fn listing_summary(line: &str) -> bool {
    let Some((tests, benches)) = line.split_once(", ") else { return false };
    let count = |part: &str, word: &str| part.split_once(' ').is_some_and(|(n, w)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && (w == word || w.strip_suffix('s') == Some(word)));
    count(tests, "test") && count(benches, "benchmark")
}

/// Writes through to the leg's log and keeps a copy: a listing's own output.
struct Tee<'a>(&'a mut dyn Write, Vec<u8>);

impl Write for Tee<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.0.write(buf)?;
        self.1.extend_from_slice(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.0.flush() }
}

/// `warm [status]`: the current box plan and spares, never a persisted run ledger.
pub fn serve(line: &str) -> Result<String, String> {
    match line.split_whitespace().skip(1).collect::<Vec<_>>().as_slice() {
        [] | ["status"] => {}
        _ => return Err("warm [status]".into()),
    }
    let plan = PLAN.lock().unwrap();
    let spares = SPARES.lock().unwrap();
    let now = wall_ms();
    let mut fields = if let Some(p) = plan.as_ref() {
        vec![
            ("generation", Value::Int(p.generation as i64)), ("work", s(&p.pair.work)), ("main", s(&p.pair.main)),
            ("ids", Value::Arr(p.ids.iter().map(s).collect())),
            ("nodes", Value::Arr(p.nodes.iter().map(|n| {
                let mut fields = n.progress.fields(now);
                fields.extend([("name", s(&n.name)), ("activity", Value::Int(n.activity as i64))]);
                if let Some(a) = &n.apps_progress {
                    let pass = a.fields(now).into_iter().filter(|(k, _)| *k != "id").collect();
                    fields.extend([("apps", Value::Arr(n.apps.iter().map(s).collect())), ("apps_pass", obj(pass))]);
                }
                obj(fields)
            }).collect())),
        ]
    } else { Vec::new() };
    fields.push(("spares", Value::Arr(spares.lanes.iter().map(|(lane, spare)| {
        let mut fields = spare.progress.fields(now);
        fields.extend([("lane", s(lane)), ("node", s(&spare.node)), ("src", s(if spare.native { "tree" } else { "overlay" }))]);
        obj(fields)
    }).collect())));
    Ok(obj(fields).to_json())
}

pub fn render(v: &Value) -> String {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let ids = v.get("ids").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ");
    let mut out = match v.get("generation").and_then(Value::as_u64) {
        Some(generation) => format!("warm {} generation {generation}: {ids}\n", super::short(&s(v, "work"))),
        None => "warm: no box plan yet\n".into(),
    };
    for n in v.get("nodes").and_then(Value::as_arr).unwrap_or(&[]) {
        out.push_str(&format!("  {} {:<8} {}s ({} lanes) {} {}\n", s(n, "name"), s(n, "state"), n.get("seconds").and_then(Value::as_u64).unwrap_or(0), n.get("activity").and_then(Value::as_u64).unwrap_or(0), s(n, "id"), s(n, "note")));
        if let Some(a) = n.get("apps_pass") {
            let apps = n.get("apps").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ");
            out.push_str(&format!("    apps {:<8} {}s: {apps} {}\n", s(a, "state"), a.get("seconds").and_then(Value::as_u64).unwrap_or(0), s(a, "note")));
        }
    }
    for n in v.get("spares").and_then(Value::as_arr).unwrap_or(&[]) {
        let native = if s(n, "src") == "tree" { "native " } else { "" };
        out.push_str(&format!("  {} spare {} {native}{:<8} {}s {} {}\n", s(n, "lane"), s(n, "node"), s(n, "state"), n.get("seconds").and_then(Value::as_u64).unwrap_or(0), s(n, "id"), s(n, "note")));
    }
    out
}

impl Package {
    fn value(&self) -> Value {
        obj(vec![("pkg", s(&self.pkg)), ("harness", self.harness.value()), ("features", Value::Arr(self.features.iter().map(s).collect()))])
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            pkg: v.get("pkg")?.as_str()?.to_string(),
            harness: Harness::parse(v.get("harness")?)?,
            features: v.get("features")?.as_arr()?.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        })
    }
}

impl Harness {
    fn value(&self) -> Value {
        match self {
            Self::All => s("all"), Self::Default => s("default"), Self::Lib => s("lib"),
            Self::Bin(name) => s(format!("bin:{name}")), Self::Test(name) => s(format!("test:{name}")),
            Self::Tests => s("tests"),
        }
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(match v.as_str()? {
            "all" => Self::All, "default" => Self::Default, "lib" => Self::Lib, "tests" => Self::Tests,
            text => match text.split_once(':')? {
                ("bin", name) if !name.is_empty() && !name.contains(':') => Self::Bin(name.to_string()),
                ("test", name) if !name.is_empty() => Self::Test(name.to_string()),
                _ => return None,
            },
        })
    }
}

impl Key {
    fn value(&self) -> Value {
        obj(vec![("id", s(&self.id)), ("rev", s(&self.rev)), ("packages", Value::Arr(self.packages.iter().map(Package::value).collect()))])
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            id: v.get("id")?.as_str()?.to_string(), rev: v.get("rev")?.as_str()?.to_string(),
            packages: v.get("packages")?.as_arr()?.iter().filter_map(Package::parse).collect(),
        })
    }
}

impl Listed {
    fn value(&self) -> Value {
        obj(vec![("unit", s(&self.unit)), ("ok", Value::Bool(self.ok)), ("text", s(&self.text))])
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(Self { unit: v.get("unit")?.as_str()?.to_string(), ok: v.get("ok")?.as_bool()?, text: v.get("text")?.as_str()?.to_string() })
    }
}

impl Progress {
    fn value(&self) -> Value {
        obj(vec![
            ("state", s(&self.state)), ("id", s(&self.id)), ("lease", self.lease.as_ref().map_or(Value::Null, s)),
            ("started_ms", Value::Int(self.started_ms as i64)), ("ended_ms", Value::Int(self.ended_ms as i64)),
            ("listed", Value::Arr(self.listed.iter().map(Listed::value).collect())),
        ])
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            state: v.get("state")?.as_str()?.to_string(), note: String::new(), id: v.get("id")?.as_str()?.to_string(),
            lease: match v.get("lease")? { Value::Null => None, v => Some(v.as_str()?.to_string()) },
            started_ms: v.get("started_ms")?.as_u64()?, ended_ms: v.get("ended_ms")?.as_u64()?,
            listed: v.get("listed")?.as_arr()?.iter().filter_map(Listed::parse).collect(),
        })
    }
}

impl Node {
    fn value(&self) -> Value {
        obj(vec![
            ("name", s(&self.name)), ("os", s(&self.os)), ("activity", Value::Int(self.activity as i64)),
            ("key", self.key.value()), ("progress", self.progress.value()),
            ("apps", Value::Arr(self.apps.iter().map(s).collect())), ("apps_progress", self.apps_progress.as_ref().map_or(Value::Null, Progress::value)),
        ])
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            name: v.get("name")?.as_str()?.to_string(), os: v.get("os")?.as_str()?.to_string(),
            activity: v.get("activity")?.as_u64()? as usize,
            key: Key::parse(v.get("key")?)?, progress: Progress::parse(v.get("progress")?)?,
            // (a state a master before app passes wrote has none)
            apps: v.get("apps").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect(),
            apps_progress: v.get("apps_progress").and_then(Progress::parse),
        })
    }
}

impl Plan {
    fn value(&self) -> Value {
        let pair = |p: &Pair| obj(vec![("work", s(&p.work)), ("main", s(&p.main))]);
        obj(vec![
            ("generation", Value::Int(self.generation as i64)), ("pair", pair(&self.pair)),
            ("base", self.base.as_ref().map_or(Value::Null, pair)), ("ids", Value::Arr(self.ids.iter().map(s).collect())),
            ("started_ms", Value::Int(self.started_ms as i64)), ("nodes", Value::Arr(self.nodes.iter().map(Node::value).collect())),
            ("judged", Value::Bool(self.judged)),
        ])
    }

    fn parse(v: &Value) -> Option<Self> {
        let pair = |v: &Value| Some(Pair { work: v.get("work")?.as_str()?.to_string(), main: v.get("main")?.as_str()?.to_string() });
        Some(Self {
            generation: v.get("generation")?.as_u64()?, pair: pair(v.get("pair")?)?,
            base: match v.get("base")? { Value::Null => None, v => Some(pair(v)?) },
            ids: v.get("ids")?.as_arr()?.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            started_ms: v.get("started_ms")?.as_u64()?,
            nodes: v.get("nodes")?.as_arr()?.iter().filter_map(Node::parse).collect(),
            judged: v.get("judged")?.as_bool()?,
        })
    }
}

impl Spare {
    fn value(&self) -> Value {
        obj(vec![
            ("node", s(&self.node)), ("native", Value::Bool(self.native)), ("generation", Value::Int(self.generation as i64)),
            ("warmed", self.warmed.as_ref().map_or(Value::Null, Key::value)), ("progress", self.progress.value()),
        ])
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            node: v.get("node")?.as_str()?.to_string(), native: v.get("native")?.as_bool()?,
            generation: v.get("generation")?.as_u64()?,
            warmed: match v.get("warmed")? { Value::Null => None, v => Some(Key::parse(v)?) },
            progress: Progress::parse(v.get("progress")?)?,
        })
    }
}

impl Spares {
    fn value(&self) -> Value {
        obj(vec![
            ("lanes", Value::Obj(self.lanes.iter().map(|(lane, spare)| (lane.clone(), spare.value())).collect())),
            ("running", Value::Obj(self.running.iter().map(|(lane, generation)| (lane.clone(), Value::Int(*generation as i64))).collect())),
            ("last_started", Value::Obj(self.last_started.iter().map(|(lane, ms)| (lane.clone(), Value::Int(*ms as i64))).collect())),
            ("generation", Value::Int(self.generation as i64)),
        ])
    }

    fn parse(v: &Value) -> Option<Self> {
        let Value::Obj(lanes) = v.get("lanes")? else { return None };
        let Value::Obj(running) = v.get("running")? else { return None };
        let Value::Obj(last_started) = v.get("last_started")? else { return None };
        Some(Self {
            lanes: lanes.iter().filter_map(|(lane, v)| Some((lane.clone(), Spare::parse(v)?))).collect(),
            running: running.iter().filter_map(|(lane, v)| Some((lane.clone(), v.as_u64()?))).collect(),
            last_started: last_started.iter().filter_map(|(lane, v)| Some((lane.clone(), v.as_u64()?))).collect(),
            generation: v.get("generation")?.as_u64()?,
        })
    }
}

impl Run {
    fn value(&self) -> Value {
        obj(vec![
            ("generation", Value::Int(self.generation as i64)), ("lane", self.lane.as_ref().map_or(Value::Null, s)),
            ("native", Value::Bool(self.native)), ("apps", Value::Bool(self.apps)), ("node", s(&self.node)), ("os", s(&self.os)), ("key", self.key.value()),
            ("started_ms", Value::Int(self.started_ms as i64)), ("since_ms", self.since_ms.map_or(Value::Null, |ms| Value::Int(ms as i64))),
        ])
    }

    fn parse(v: &Value) -> Option<Self> {
        Some(Self {
            generation: v.get("generation")?.as_u64()?,
            lane: match v.get("lane")? { Value::Null => None, v => Some(v.as_str()?.to_string()) },
            native: v.get("native")?.as_bool()?, apps: v.get("apps").and_then(Value::as_bool).unwrap_or(false),
            node: v.get("node")?.as_str()?.to_string(), os: v.get("os")?.as_str()?.to_string(),
            key: Key::parse(v.get("key")?)?, started_ms: v.get("started_ms")?.as_u64()?,
            since_ms: match v.get("since_ms")? { Value::Null => None, v => Some(v.as_u64()?) },
            adopt: None,
        })
    }
}

/// The warm state a master restart takes back.
/// Progress preserves durable fields, never a wait line.
fn state_value(plan: Option<&Plan>, spares: &Spares, legs: &[Run]) -> Value {
    obj(vec![
        ("plan", plan.map_or(Value::Null, Plan::value)), ("spares", spares.value()),
        ("legs", Value::Arr(legs.iter().map(Run::value).collect())),
    ])
}

fn state_parse(v: &Value) -> Option<(Option<Plan>, Spares, Vec<Run>)> {
    let plan = match v.get("plan")? { Value::Null => None, v => Some(Plan::parse(v)?) };
    let spares = match v.get("spares") {
        Some(v) => Spares::parse(v)?,
        None => Spares { lanes: BTreeMap::new(), running: BTreeMap::new(), last_started: BTreeMap::new(), generation: 0 },
    };
    let legs = v.get("legs")?.as_arr()?.iter().filter_map(Run::parse).collect();
    Some((plan, spares, legs))
}

#[cfg(test)]
mod tests {
    use super::{Key, Listed, Node, Package, Plan, Progress, Role, Src};

    /// A lane whose newest lease ran commits work (and the private main) hold has landed, and
    /// gets no warm leg: its build is the tip's (11-17: two landed lanes' legs waited 10-12 min).
    /// A lane commit work lacks, either half's, keeps it warm.
    #[test]
    fn a_landed_lanes_commits_are_in_the_tips_and_a_live_lanes_are_not() {
        let d = std::env::temp_dir().join(format!("devhub-warm-landed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let com = d.join("apps/commercial");
        std::fs::create_dir_all(&com).unwrap();
        let g = |dir: &std::path::Path, args: &[&str]| {
            let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}");
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let commit = |dir: &std::path::Path, m: &str| {
            g(dir, &["commit", "-q", "--allow-empty", "-m", m]);
            g(dir, &["rev-parse", "HEAD"])
        };
        g(&d, &["init", "-q", "-b", "work"]);
        g(&com, &["init", "-q", "-b", "main"]);
        let (landed, private) = (commit(&d, "lane 1"), commit(&com, "lane 1 private"));
        commit(&d, "after");
        g(&d, &["checkout", "-q", "-b", "rc-x"]);
        let ahead = commit(&d, "lane 2, not landed");
        g(&com, &["checkout", "-q", "-b", "rc-x"]);
        let private_ahead = commit(&com, "lane 2 private");
        let tips = super::Tips::read(&d);
        assert!(tips.hold(&d, &format!("rc-x@{landed}")), "landed");
        assert!(tips.hold(&d, &format!("rc-x@{landed},commercial=rc-x@{private}")), "landed in both halves");
        assert!(!tips.hold(&d, &format!("rc-x@{ahead}")), "a commit work lacks");
        assert!(!tips.hold(&d, &format!("rc-x@{landed},commercial=rc-x@{private_ahead}")), "the private half not landed");
        let unknown = format!("rc-x@{}", "0".repeat(40));
        for rev in ["", "rc-x", "rc-x@", unknown.as_str()] {
            assert!(!tips.hold(&d, rev), "{rev:?} names no commit the tips hold");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_warm_state_survives_its_round_trip() {
        use super::{state_parse, state_value, Harness, Pair, Run, Spare, Spares};
        let key = Key {
            id: "jit".into(), rev: "work@x".into(),
            packages: vec![
                Package { pkg: "package".into(), harness: Harness::Test("t".into()), features: vec!["f".into()] },
                Package { pkg: "package".into(), harness: Harness::Bin("b".into()), features: Vec::new() },
            ],
        };
        let progress = Progress {
            state: "waiting".into(), note: "waiting for a box".into(), id: "jit".into(), lease: Some("L9".into()),
            started_ms: 10, ended_ms: 0, listed: vec![Listed { unit: "suite package".into(), ok: true, text: "t: test\n".into() }],
        };
        let plan = Plan {
            generation: 3, pair: Pair::new("work", "main"), base: Some(Pair::new("base-work", "base-main")),
            ids: vec!["jit".into(), "other".into()], started_ms: 10,
            nodes: vec![Node { name: "node".into(), os: "linux".into(), activity: 2, key: key.clone(), progress: progress.clone(), apps: vec!["makepad-fab".into(), "app".into()], apps_progress: Some(progress.clone()) }],
            judged: true,
        };
        let spares = Spares {
            lanes: [("lane".into(), Spare { node: "node".into(), native: true, generation: 4, warmed: Some(key.clone()), progress })].into(),
            running: [("lane".into(), 4)].into(), last_started: [("lane".into(), 20)].into(), generation: 4,
        };
        let legs = [
            Run { generation: 3, lane: None, native: false, apps: false, node: "node".into(), os: "linux".into(), key: key.clone(), started_ms: 10, since_ms: Some(5), adopt: None },
            Run { generation: 4, lane: Some("lane".into()), native: true, apps: false, node: "node".into(), os: "linux".into(), key: key.clone(), started_ms: 20, since_ms: None, adopt: None },
            Run { generation: 3, lane: None, native: false, apps: true, node: "node".into(), os: "linux".into(), key, started_ms: 30, since_ms: Some(30), adopt: None },
        ];
        let value = state_value(Some(&plan), &spares, &legs);
        let (plan, spares, legs) = state_parse(&value).expect("serialized warm state parses");
        assert_eq!(state_value(plan.as_ref(), &spares, &legs).to_json(), value.to_json(), "warm state JSON survives its round trip");
        let node = &plan.expect("the plan survives").nodes[0];
        assert_eq!(node.progress.lease.as_deref(), Some("L9"), "progress keeps its lease");
        assert!(node.progress.note.is_empty(), "the wait note is not persisted");
        assert_eq!(node.key.packages.len(), 2, "both packages survive");
        assert_eq!(node.key.packages[0].harness, Harness::Test("t".into()), "the test harness survives");
        assert_eq!(node.key.packages[0].features, ["f"], "the test features survive");
        assert_eq!(node.key.packages[1].harness, Harness::Bin("b".into()), "the binary harness survives");
        assert!(node.key.packages[1].features.is_empty(), "the binary keeps its empty features");
        assert_eq!((node.apps.as_slice(), node.apps_progress.as_ref().and_then(|a| a.lease.as_deref())), (["makepad-fab".to_string(), "app".to_string()].as_slice(), Some("L9")), "the app pass keeps its apps, in order, and its lease");
        assert_eq!(legs.len(), 3, "the running legs survive");
        assert!(legs[2].apps && !legs[0].apps && !legs[1].apps, "an app pass stays one, and the others stay theirs");
        assert_eq!((legs[0].lane.as_deref(), legs[0].native, legs[0].since_ms), (None, false, Some(5)), "the prebuild keeps its lane, native flag and since time");
        assert_eq!((legs[1].lane.as_deref(), legs[1].native, legs[1].since_ms), (Some("lane"), true, None), "the native lane run keeps its lane, native flag and since time");
    }

    /// A restarted master takes back its warm legs on the leases still held and asks again for
    /// the others, keeping their age; an ended leg stays ended; a plan box or a running lane the
    /// state names no leg for (a restart between a plan and its legs) starts again.
    #[test]
    fn a_restart_takes_back_each_warm_leg_once() {
        use super::{restore, Pair, Run, Spare, Spares};
        let key = Key { id: "jit".into(), rev: "work@x".into(), packages: Vec::new() };
        let progress = |state: &str, lease: Option<&str>, ended_ms: u64| Progress { state: state.into(), note: String::new(), id: "jit".into(), lease: lease.map(Into::into), started_ms: 10, ended_ms, listed: Vec::new() };
        let node = |name: &str, p: Progress| Node { name: name.into(), os: "linux".into(), activity: 0, key: key.clone(), progress: p, apps: Vec::new(), apps_progress: None };
        let mut plan = Some(Plan {
            generation: 3, pair: Pair::new("work", "main"), base: None, ids: vec!["jit".into()], started_ms: 100,
            nodes: vec![
                node("held", progress("building", Some("L1"), 0)), node("gone", progress("building", Some("L2"), 0)), node("unled", progress("waiting", None, 0)), node("ended", progress("done", None, 150)),
                Node { apps: vec!["app".into()], apps_progress: Some(progress("warming app", Some("L5"), 0)), ..node("passing", progress("done", None, 150)) },
                Node { apps: vec!["app".into()], apps_progress: Some(progress("after its prebuild", None, 0)), ..node("pass-unled", progress("done", None, 150)) },
            ],
            judged: false,
        });
        let spare = |generation: u64, p: Progress| Spare { node: "node".into(), native: false, generation, warmed: None, progress: p };
        let mut spares = Spares {
            lanes: [("shipping".into(), spare(5, progress("held", Some("L3"), 0))), ("unled".into(), spare(4, progress("building", Some("L4"), 0)))].into(),
            running: [("shipping".into(), 5), ("unled".into(), 4)].into(),
            last_started: [("shipping".into(), 90), ("unled".into(), 80)].into(),
            generation: 5,
        };
        let run = |generation: u64, lane: Option<&str>, node: &str| Run { generation, lane: lane.map(Into::into), native: false, apps: false, node: node.into(), os: "linux".into(), key: key.clone(), started_ms: 100, since_ms: Some(96), adopt: None };
        let runs = vec![run(3, None, "held"), run(3, None, "gone"), run(3, None, "ended"), run(2, None, "unled"), run(5, Some("shipping"), "node"), Run { apps: true, ..run(3, None, "passing") }];
        let legs = restore(&mut plan, &mut spares, runs, &|l| l != "L2");
        let pass = |node: &str| legs.iter().find(|r| r.apps && r.node == node);
        assert_eq!(pass("passing").and_then(|r| r.adopt.clone()), Some(("L5".into(), true)), "an app pass whose lease is held goes on on it");
        assert!(pass("pass-unled").is_some_and(|r| r.generation == 3 && r.adopt.is_none()), "a done prebuild's app pass the state names no leg for starts one");
        assert!(!legs.iter().any(|r| !r.apps && (r.node == "passing" || r.node == "pass-unled")), "their done prebuilds are not run again");
        let leg = |lane: Option<&str>, node: &str| legs.iter().find(|r| r.lane.as_deref() == lane && (lane.is_some() || r.node == node));
        assert_eq!(legs.len(), 6, "the held, the gone, the unled box, the shipping lane and the two app passes go on; nothing else: {:?}", legs.iter().map(|r| (&r.lane, &r.node, r.apps)).collect::<Vec<_>>());
        assert_eq!(leg(None, "held").and_then(|r| r.adopt.clone()), Some(("L1".into(), true)), "a leg whose lease is held goes on on it, shipped");
        assert_eq!(leg(Some("shipping"), "").and_then(|r| r.adopt.clone()), Some(("L3".into(), false)), "a lane leg held but not shipped ships on its lease");
        let plan = plan.unwrap();
        assert!(leg(None, "gone").is_some_and(|r| r.adopt.is_none() && r.since_ms == Some(96)), "a leg whose lease went asks again, keeping its age");
        assert_eq!((plan.nodes[1].progress.lease.as_deref(), plan.nodes[1].progress.state.as_str()), (None, "waiting"), "its box waits for a lease again");
        assert!(leg(None, "ended").is_none(), "an ended leg is not run again");
        assert!(leg(None, "unled").is_some_and(|r| r.generation == 3 && r.adopt.is_none() && r.since_ms == Some(96)), "a plan box whose leg the state missed starts one of the plan's generation, at its rank's age");
        assert!(leg(Some("unled"), "").is_none() && !spares.running.contains_key("unled") && !spares.last_started.contains_key("unled"), "a lane marked running without a leg may start again at once");
        assert_eq!((spares.lanes["unled"].progress.lease.as_deref(), spares.lanes["unled"].progress.state.as_str()), (None, "waiting"), "its spare waits again");
    }

    /// A warm replay builds a target with the features it requires (`required-features`): a
    /// test target selected without them is an error (makepad-motion's `effect_modules` needs
    /// `scene3d`: every warm run of it failed, 11-06). A target that needs none, and the
    /// all-targets selections cargo skips such a target in, get no `--features`. A package whose
    /// tests run in the lane's tree (test-tree) is built by a native warm run alone: in an overlay
    /// relax-compiler's build script ran cargo metadata on a root only the JIT reads (11-13).
    #[test]
    fn a_warm_target_builds_with_the_features_it_requires() {
        let manifest = "[package]\nname = \"motion\"\n\n[features]\nscene3d = []\n\n[[test]]\nname = \"effect_modules\"\nrequired-features = [\"scene3d\"]\n\n[[bin]]\nname = \"viewer\"\nrequired-features = [\"scene3d\", \"gpu\"]\n";
        let tree = super::super::ci::Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"motion\", \"stamped\"]\n"),
            ("motion/Cargo.toml", manifest),
            ("motion/src/lib.rs", ""),
            ("motion/tests/effect_modules.rs", ""),
            ("motion/tests/plain.rs", ""),
            ("stamped/Cargo.toml", "[package]\nname = \"stamped\"\n[package.metadata.makepad]\ntest-tree = \"its build script runs cargo on the checkout\"\n"),
            ("stamped/src/lib.rs", ""),
        ]);
        let mut asked: Vec<Package> = ["test:motion:effect_modules", "test:motion:plain", "test:motion:bin:viewer", "test:motion:lib"].iter().map(|c| Package::from_class(c).unwrap()).collect();
        let stamped = Package::from_class("test:stamped:lib").unwrap();
        asked.push(stamped.clone());
        let (natively, note) = super::resolve_in(&tree, &asked, false);
        assert!(natively.contains(&stamped) && note.is_empty(), "a native warm run builds it: {note}");
        let (packages, note) = super::resolve_in(&tree, &asked, true);
        assert_eq!(note, "dropped stamped:lib (runs in the lane's tree: its build script runs cargo on the checkout)", "an overlay one leaves it out");
        let argv: Vec<String> = packages.iter().map(|p| p.argv()[11..].join(" ")).collect();
        assert_eq!(argv, ["motion --profile release-checks --test effect_modules --features scene3d -- --list", "motion --profile release-checks --test plain -- --list", "motion --profile release-checks --bin viewer --features scene3d,gpu -- --list", "motion --profile release-checks --lib -- --list"]);
        let native = packages.iter().find(|p| p.label() == "motion:effect_modules").unwrap().native_argv("linux");
        assert!(native.ends_with(&["--test".to_string(), "effect_modules".into(), "--features".into(), "scene3d".into()]), "{native:?}");
    }

    /// Every dev box that lacks the new JIT builds it, the lane-active ones first; one that has it,
    /// one with another compiler, a box down or with dev off is left alone.
    #[test]
    fn a_new_jit_is_planned_on_every_box_that_lacks_it() {
        use crate::lease::sched::NodeView;
        let node = |name: &str, up: bool, dev: bool, compiler: &str, ids: &[&str]| NodeView {
            name: name.into(), up, dev,
            caps: Some(crate::lease::Caps { os: "linux".into(), arch: "x86_64".into(), jit_store: Some(std::sync::Arc::new(crate::lease::jitstore::Snapshot { compiler: compiler.into(), ids: ids.iter().map(|s| s.to_string()).collect(), ..Default::default() })), ..Default::default() }),
            ..Default::default()
        };
        let nodes = [node("idle", true, true, "c", &[]), node("busy", true, true, "c", &[]), node("has", true, true, "c", &["new"]), node("other", true, true, "d", &[]), node("down", false, true, "c", &[]), node("off", true, false, "c", &[])];
        let mut activity = super::Activity::default();
        activity.boxes.insert("busy".into(), super::BoxActivity { lanes: ["rc-a".to_string()].into(), packages: Default::default() });
        let by_compiler = std::collections::BTreeMap::from([("c".to_string(), "new".to_string())]);
        let plan = super::plan_nodes(&nodes, &by_compiler, &activity, &Default::default(), "work@x", 1);
        assert_eq!(plan.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["busy", "idle"]);
        assert!(plan.iter().all(|n| n.key.id == "new"));
    }

    #[test]
    fn a_failure_clears_once_its_boxs_store_holds_the_jit() {
        let id = "66f764de1aae36f89242ec4f";
        let mut p = Progress { state: "failed".into(), note: "refused: node-a: x".into(), ..Progress::waiting(id, 1) };
        assert!(!p.clear_if_stored(&["0".repeat(24)], 5), "another JIT in the store is no fix");
        assert_eq!(p.state, "failed");
        assert!(p.clear_if_stored(&["0".repeat(24), id.into()], 5));
        assert_eq!((p.state.as_str(), p.ended_ms), ("stored", 5));
        assert!(!p.note.contains("refused"));
        // only a failure clears: a warm in flight or done stays what it is
        for state in ["waiting", "building", "done", "warm"] {
            let mut p = Progress { state: state.into(), ..Progress::waiting(id, 1) };
            assert!(!p.clear_if_stored(&[id.into()], 5) && p.state == state);
        }
        // a failure before any JIT was named never clears
        let mut p = Progress { state: "failed".into(), ..Progress::waiting("", 1) };
        assert!(!p.clear_if_stored(&[String::new()], 5));
    }

    /// A spare's leg queued at a full box moves only to a box the scheduler could start it on now
    /// that its spare may take: never one of the lane's own boxes, a bench box beside pool ones, a
    /// box short of RAM or another OS.
    #[test]
    fn a_spare_moves_only_to_a_free_box_it_may_warm() {
        use crate::lease::sched::NodeView;
        let node = |name: &str, os: &str, role: Role, ram: f64| NodeView {
            name: name.into(), up: true, dev: true,
            caps: Some(crate::lease::Caps { os: os.into(), arch: "x86_64".into(), features: crate::lease::FEATURES.iter().map(|f| f.to_string()).collect(), ram_avail_gb: ram, role: Some(role), ..Default::default() }),
            ..Default::default()
        };
        let nodes = [node("full", "linux", Role::Pool, 16.0), node("own", "linux", Role::Pool, 16.0), node("free", "linux", Role::Pool, 16.0), node("bench", "linux", Role::Bench, 16.0), node("short", "linux", Role::Pool, 4.0), node("mac", "macos", Role::Pool, 16.0)];
        let l = super::LaneActivity { boxes: ["own".to_string()].into(), src: Src::Overlay, newest_ms: 1, node: "own".into(), rev: "work@x".into(), hint: Vec::new(), packages: Vec::new(), native_packages: Vec::new() };
        let group = nodes[0].caps.clone().unwrap();
        let free: Vec<String> = ["own", "free", "bench", "short", "mac"].iter().map(|n| n.to_string()).collect();
        assert_eq!(super::free_spares(&nodes, &free, &l, &group, false, &super::Activity::default()), vec!["free".to_string()]);
        assert!(super::free_spares(&nodes, &[], &l, &group, false, &super::Activity::default()).is_empty(), "no box could start it: it stays");
    }

    /// A spare's leg that waited to start past what it saves at its box gives up: not before, not
    /// once it holds its lease, and not while no leg runs (its next warm's wait).
    #[test]
    fn a_spares_leg_gives_up_past_what_it_saves() {
        let spare = |state: &str, lease: Option<&str>| super::Spare { node: "a".into(), native: true, generation: 1, warmed: None, progress: Progress { state: state.into(), lease: lease.map(str::to_string), ..Progress::waiting("", 1_000) } };
        let waiting = spare("waiting", None);
        assert!(!super::waited_out(&waiting, true, 91_000, 90_000), "90 s: what a build there takes");
        assert!(super::waited_out(&waiting, true, 91_001, 90_000));
        assert!(!super::waited_out(&waiting, false, 900_000, 90_000), "no leg runs: its next warm waits its turn");
        assert!(!super::waited_out(&spare("held", Some("L1")), true, 900_000, 90_000), "started: it builds");
        assert!(!super::waited_out(&spare("syncing", Some("L1")), true, 900_000, 90_000));
    }

    fn plan(nodes: Vec<Node>) -> Plan {
        Plan { generation: 7, pair: crate::state::Pair::new(&"a".repeat(40), &"b".repeat(40)), base: None, ids: vec!["new".into()], started_ms: 1, nodes, judged: false }
    }

    fn node(name: &str, packages: &[&str], ended_ms: u64, listed: Vec<Listed>) -> Node {
        let packages = packages.iter().map(|p| Package::from_class(&format!("test:{p}")).unwrap()).collect();
        Node { name: name.into(), os: "linux".into(), activity: 0, key: Key { id: "new".into(), rev: "work@x".into(), packages }, progress: Progress { ended_ms, listed, ..Progress::waiting("new", 1) }, apps: Vec::new(), apps_progress: None }
    }

    /// The last box that lists tests to end judges the plan, once; a box that lists none is not
    /// waited for, nor is a plan of another generation judged.
    #[test]
    fn a_plan_is_judged_once_its_listing_boxes_ended() {
        let listed = |ok| vec![Listed { unit: "suite x".into(), ok, text: "x on a: exit 0\n".into() }];
        let mut p = plan(vec![node("a", &["x"], 5, listed(true)), node("b", &["y"], 0, Vec::new()), node("idle", &[], 0, Vec::new())]);
        assert!(super::listings_done(Some(&mut p), 7).is_none(), "b still lists");
        p.nodes[1].progress.ended_ms = 6;
        assert!(super::listings_done(Some(&mut p), 6).is_none(), "another generation's run");
        let judged = super::listings_done(Some(&mut p), 7).expect("both listing boxes ended; the idle one is not waited for");
        assert!(judged.judged && p.judged);
        assert!(super::listings_done(Some(&mut p), 7).is_none(), "judged once");
    }

    /// A listing that fails on a landed tip is a red row of the `prebuild` bucket with its errors
    /// and a failed feed line; one that lists is an ok row.
    #[test]
    fn a_failed_listing_is_a_red_prebuild_bucket_row() {
        use crate::proto::{Record, Sev};
        use std::sync::{Arc, Mutex};
        let home = std::env::temp_dir().join(format!("devhub-warm-test-prebuild-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let fed: Arc<Mutex<Vec<Record>>> = Arc::default();
        let feed = fed.clone();
        let leases = Arc::new(crate::lease::master::LeaseMaster::new("mac", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let store = crate::state::Store::open(&home, "mac", Arc::new(move |r: Record| { feed.lock().unwrap().push(r); 0 })).unwrap();
        let ctx = super::Ctx { store, leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: home.clone(), home: home.clone() };
        let broken = Listed { unit: "suite x".into(), ok: false, text: "x:lib on a: exit 1\n/n/tree/libs/x/src/lib.rs:3:9: E0425: cannot find value `z`\n".into() };
        let fine = Listed { unit: "suite y".into(), ok: true, text: "y on b: exit 0\n".into() };
        // a listing that ended on its tests' count and exited 101 without a word (L1348447, 11-18):
        // labelled, its whole stream kept once under its lease, and its red points there
        let stream = "L7 a: selecting\nrelax test: 0 warnings\n     Running tests/proofs.rs\nsolves: test\n\n1 tests, 0 benchmarks\n";
        let logs = home.join("lease-logs");
        for _ in 0..2 { super::keep_listing(&logs, "L7", "suite z", "z on a: exit 101", stream).unwrap(); }
        let kept = std::fs::read_to_string(logs.join("L7.log")).unwrap();
        assert_eq!(kept.matches("== suite z failed: z on a: exit 101; its whole stream\n").count(), 1, "kept once, a master's second ask hearing the same run: {kept}");
        assert!(kept.ends_with("1 tests, 0 benchmarks\n"), "{kept}");
        let silent = Listed { unit: "suite z".into(), ok: false, text: super::failed_listing("suite z", "z on a: exit 101", stream, 101, Some("L7")) };
        assert!(silent.text.contains("EXIT: exit after listing: exit 101 with no error in its output once it had listed 1 target whole (tests/proofs.rs)") && silent.text.ends_with("GATEKEPT suite z :: L7\n"), "{}", silent.text);
        let said = |out: &str| super::exit_said(out, 101);
        assert_eq!(said("/n/tree/libs/x/src/lib.rs:3:9: E0425: cannot find value `z`\n"), None, "its errors say why");
        assert_eq!(said("     Running x\nthread 'main' panicked at a.rs:1:1:\n"), None, "so does its panic");
        assert!(said("     Running src/lib.rs\na: test\n").is_some_and(|s| s.contains("inside its listing of src/lib.rs")), "a listing cut short is no whole one");
        assert!(said("     Running a\n1 test, 0 benchmarks\nerror: test failed\n").is_some_and(|s| s.contains("its last line: error: test failed")), "a line after the count is where it stopped");
        assert!(said("").is_some_and(|s| s.ends_with("it printed nothing")));
        let long = format!("{}\nend\n", "x".repeat(super::LISTING_TAIL));
        assert_eq!(super::tail_of(&long, super::LISTING_TAIL), "end\n", "a tail starts at a line");
        super::judge(&ctx, &plan(vec![node("a", &["x", "z"], 5, vec![broken, silent]), node("b", &["y"], 5, vec![fine.clone()])]), false);
        let rows: Vec<_> = crate::bucket::ledger(&home, 10).iter().map(|l| makepad_strict_json::parse(l.as_bytes()).unwrap()).collect();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.get("bucket").and_then(|v| v.as_str()), Some("prebuild"));
        assert_eq!(row.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(row.get("failing").and_then(|v| v.as_arr()).map(|f| f.iter().filter_map(|u| u.as_str()).collect::<Vec<_>>()), Some(vec!["suite x", "suite z"]));
        assert_eq!(row.get("errors").and_then(|e| e.get("suite x")).and_then(|v| v.as_arr()).map(|e| e.len()), Some(1), "{}", row.to_json());
        assert!(row.get("errors").and_then(|e| e.get("suite z")).and_then(|v| v.as_arr()).and_then(|e| e.first()?.as_str().map(str::to_string)).is_some_and(|e| e.starts_with(crate::bucket::EXIT_AFTER_LISTING)), "{}", row.to_json());
        assert!(row.get("passed").and_then(|v| v.as_arr()).is_some_and(|p| p.iter().any(|u| u.as_str() == Some("suite y"))));
        {
            let fed = fed.lock().unwrap();
            let red = fed.iter().find(|r| r.kind == "bucket").expect("a bucket line on the feed");
            assert!(red.sev == Sev::Err && red.line.contains("bucket prebuild") && red.line.contains("FAILED suite x, suite z (exit after listing; its output: devhub job result L7)"), "{}", red.line);
        }
        // the next JIT's listings pass: an ok row; a plan that listed nothing is none
        super::judge(&ctx, &plan(vec![node("b", &["y"], 5, vec![fine])]), false);
        super::judge(&ctx, &plan(vec![node("idle", &[], 5, Vec::new())]), true);
        let rows = crate::bucket::ledger(&home, 10);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].contains("\"ok\":true"), "{}", rows[0]);
        let _ = std::fs::remove_dir_all(&home);
    }
}
