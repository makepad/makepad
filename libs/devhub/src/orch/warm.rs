//! Warm native targets, dev JIT stores and shared overlay code caches, without copying between
//! boxes. A new landed JIT id plans every dev box per OS/arch that lacks it, the most lane-active
//! first, with one lane-priority leg per box using four cores, never held back by background
//! admission or preemption: a box left out made the first live session there build the JIT itself
//! (3-5 min, a fifth of all live starts, 11-03). After building it, each box lists (never runs)
//! tests for its four most-used package/target pairs in the same held lease. Every 20 s,
//! agent lanes with grants in the last hour keep one extra box warm at their newest pin, unless
//! they already used two up boxes in that group; a landed lane whose branch is deleted keeps
//! no spare. Live/show job lanes only count toward their box's plan. Overlay spares list the
//! lane's three newest package/target pairs; native spares build the lane's two newest pairs
//! without running tests.
//! Entries are checked against targets at the pin; missing ones are dropped with one note, and live defaults resolve by M30.
//! Both keep at most one leg per lane and one warm start per ten minutes.
//! Spare legs stay at background priority under Held's lane-work and N-2 admission rules.
//! Plans and spare assignments live only in this master; a restart rebuilds them cheaply
//! from activity and the caches already on each box.

use super::leg::{self, Held, Spec};
use super::Ctx;
use crate::lease::client::ALL_TESTS;
use crate::lease::sched::NodeView;
use crate::lease::{Caps, Kind, Src, PREBUILD_LANE_PREFIX, WARM_LANE_PREFIX};
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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Package {
    pkg: String,
    harness: Harness,
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
        Some(Self { pkg: pkg.to_string(), harness })
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
        match &self.harness {
            Harness::Lib => vec!["--lib".into()],
            Harness::Bin(b) => vec!["--bin".into(), b.clone()],
            Harness::Test(t) => vec!["--test".into(), t.clone()],
            Harness::Tests => vec!["--tests".into()],
            Harness::All | Harness::Default => Vec::new(),
        }
    }

    fn argv(&self) -> Vec<String> {
        let mut argv: Vec<_> = ["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "test", &self.pkg].into_iter().map(String::from).collect();
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

fn resolve(repo: &Path, rev: &str, packages: &[Package]) -> (Vec<Package>, String) {
    if packages.is_empty() { return (Vec::new(), String::new()); }
    let tree = crate::lease::Pin::of_rev(rev).ok_or_else(|| "missing commit pair".to_string()).and_then(|pin| {
        super::ci::Tree::at(repo, &Pair::new(&pin.makepad, pin.commercial.as_deref().unwrap_or("")))
    });
    let tree = match tree {
        Ok(tree) => tree,
        Err(e) => return (Vec::new(), format!("targets unknown at {rev}: {e}")),
    };
    let dirs = super::ci::package_dirs(&tree);
    let (mut kept, mut dropped) = (Vec::new(), Vec::new());
    let mut seen = BTreeSet::new();
    for package in packages {
        let krate = dirs.get(&package.pkg).and_then(|dir| {
            super::ci::krate(&tree, &if dir.is_empty() { "Cargo.toml".into() } else { format!("{dir}/Cargo.toml") })
        });
        let Some(k) = krate else {
            dropped.push(format!("{} (no such package)", package.label()));
            continue;
        };
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
                let package = Package { pkg: package.pkg.clone(), harness };
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
}

impl Progress {
    fn waiting(id: &str, now: u64) -> Self {
        Self { state: "waiting".into(), note: String::new(), id: id.into(), lease: None, started_ms: now, ended_ms: 0 }
    }

    /// A failure its box outgrew: the JIT it failed on is in the box's store now, however it got
    /// there (a warm, a JIT run, a lane build). 105's failure stayed in the health report after a
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
}

#[derive(Clone)]
struct Plan {
    generation: u64,
    pair: Pair,
    ids: Vec<String>,
    started_ms: u64,
    nodes: Vec<Node>,
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
    node: String,
    os: String,
    key: Key,
    started_ms: u64,
    since_ms: Option<u64>,
}

static PLAN: Mutex<Option<Plan>> = Mutex::new(None);
static SPARES: Mutex<Spares> = Mutex::new(Spares { lanes: BTreeMap::new(), running: BTreeMap::new(), last_started: BTreeMap::new(), generation: 0 });

pub fn start(ctx: Arc<Ctx>) {
    let c = ctx.clone();
    if let Err(e) = std::thread::Builder::new().name(AGENT.into()).spawn(move || {
        // No plan survives an exec. Give up the previous master's legs before planning anew.
        for l in c.leases.view().iter().filter(|l| l.req.agent.as_deref() == Some(AGENT)) {
            c.leases.release_live(&l.id);
        }
        let mut seen = None;
        loop {
            tick(&c, &mut seen);
            std::thread::sleep(TICK);
        }
    }) {
        ctx.store.event("warm", Sev::Warn, format!("warm: thread: {e}"), None);
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
        f(&mut n.progress);
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
    let nodes = ctx.leases.nodes();
    let now = wall_ms();
    let activity = activity(ctx, now.saturating_sub(ACTIVE_MS));
    let compilers = ctx.leases.jit_compilers();
    clear_stored(&nodes, now);
    // Spares follow lane pins even while the landed tip is unchanged or unavailable.
    spare_tick(ctx, &nodes, &compilers, &activity, now);
    plan_tick(ctx, seen, &nodes, &compilers, &activity, now);
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
    let nodes = plan_nodes(nodes, &by_compiler, activity, &pair.rev(), started_ms);
    let (plan, previous) = {
        let mut p = PLAN.lock().unwrap();
        let generation = p.as_ref().map_or(1, |p| p.generation + 1);
        let plan = Plan { generation, pair, ids, started_ms, nodes };
        let previous = p.replace(plan.clone());
        (plan, previous)
    };
    // Registration and generation changes share the lock; no held old leg is missed.
    if let Some(p) = previous {
        for id in p.nodes.iter().filter_map(|n| n.progress.lease.as_deref()) { ctx.leases.release_live(id); }
    }
    ctx.store.event("warm", Sev::Info, format!("warm {}: {} on {}", super::short(&plan.pair.work), plan.ids.join(", "), plan.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>().join(", ")), None);
    for (rank, node) in plan.nodes.iter().enumerate() {
        // Keep every timestamp in the past: the scheduler clamps future queue times to now.
        let since_ms = Some(plan.started_ms.saturating_sub((plan.nodes.len() - rank) as u64));
        spawn(ctx, Run { generation: plan.generation, lane: None, native: false, node: node.name.clone(), os: node.os.clone(), key: node.key.clone(), started_ms, since_ms });
    }
}

/// The boxes a new JIT id is built on: every up dev box that lacks it, per OS/arch group the most
/// lane-active first. A box no lane used lately builds it too (the next session may come to it),
/// listing no tests.
fn plan_nodes(nodes: &[NodeView], by_compiler: &BTreeMap<String, String>, activity: &Activity, rev: &str, started_ms: u64) -> Vec<Node> {
    let mut groups: BTreeMap<(String, String), Vec<Node>> = BTreeMap::new();
    for n in nodes.iter().filter(|n| n.up && n.dev && !n.role().users()) {
        let Some(caps) = &n.caps else { continue };
        let Some(store) = &caps.jit_store else { continue };
        let Some(id) = by_compiler.get(&store.compiler) else { continue };
        if store.ids.contains(id) { continue; }
        let b = activity.boxes.get(&n.name);
        let packages = b.map(BoxActivity::packages).unwrap_or_default();
        groups.entry((caps.os.clone(), caps.arch.clone())).or_default().push(Node { name: n.name.clone(), os: caps.os.clone(), activity: b.map_or(0, |b| b.lanes.len()), key: Key { id: id.clone(), rev: rev.to_string(), packages }, progress: Progress::waiting(id, started_ms) });
    }
    let mut out = Vec::new();
    for (_, mut candidates) in groups {
        candidates.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| a.name.cmp(&b.name)));
        out.extend(candidates);
    }
    out
}

fn same_group(a: &Caps, b: &Caps) -> bool { a.os == b.os && a.arch == b.arch }

fn spare_ok(n: &NodeView, l: &LaneActivity, group: &Caps) -> bool {
    n.up && n.dev && !n.role().users() && !l.boxes.contains(&n.name) && n.caps.as_ref().is_some_and(|c| same_group(c, group) && c.hub_proto >= if l.src == Src::Tree { 7 } else { 5 })
}

fn native_room(c: &Caps, activity: &Activity) -> bool {
    c.native.len() < c.native_trees as usize || c.native.iter().any(|lane| !activity.active.contains(lane))
}

/// Only agent lanes receive spares, excluding devhub's own job lanes.
fn agent_lane(lane: &str) -> bool {
    !matches!(lane, crate::lease::sched::CI_LANE | crate::lease::sched::LANDING_LANE)
        && !["live-", "show-", "landq", "jitlegs", WARM_LANE_PREFIX].iter().any(|p| lane.starts_with(p))
}

fn spare_tick(ctx: &Arc<Ctx>, nodes: &[NodeView], compilers: &[String], activity: &Activity, now: u64) {
    // Check branches once per tick, before taking SPARES (refs read in-process).
    let mut done = BTreeMap::new();
    let needing: BTreeMap<_, _> = activity.lanes.iter().filter_map(|(lane, l)| {
        if !agent_lane(lane) { return None; }
        if let Some((branch, _)) = l.rev.split(',').next().unwrap_or("").split_once('@') {
            if !matches!(branch, "" | "work" | "main") && *done.entry(branch).or_insert_with(|| crate::gitread::rev(&ctx.repo, &format!("refs/heads/{branch}")).is_none()) { return None; }
        }
        let group = nodes.iter().find(|n| n.name == l.node)?.caps.as_ref()?;
        let used = nodes.iter().filter(|n| n.up && l.boxes.contains(&n.name) && n.caps.as_ref().is_some_and(|c| same_group(c, group))).count();
        (used < 2).then_some((lane, (l, group)))
    }).collect();
    let mut spares = SPARES.lock().unwrap();
    let mut release = Vec::new();
    let mut runs = Vec::new();
    let remove: Vec<_> = spares.lanes.iter().filter(|(lane, s)| {
        !needing.get(lane).is_some_and(|(l, group)| s.native == (l.src == Src::Tree) && nodes.iter().any(|n| n.name == s.node && spare_ok(n, l, group)))
    }).map(|(lane, _)| lane.clone()).collect();
    for lane in remove {
        let s = spares.lanes.remove(&lane).unwrap();
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
            let candidate = nodes.iter().filter(|n| spare_ok(n, l, group) && n.caps.as_ref().is_some_and(|c| c.ram_avail_gb >= 8.0 && (!native || native_room(c, activity)))).min_by(|a, b| {
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
        runs.push(Run { generation, lane: Some(lane.clone()), native, node, os: caps.os.clone(), key, started_ms: now, since_ms: None });
    }
    drop(spares);
    for id in release { ctx.leases.release_live(&id); }
    for run in runs { spawn(ctx, run); }
}

fn spawn(ctx: &Arc<Ctx>, run: Run) {
    let (c, r) = (ctx.clone(), run.clone());
    if let Err(e) = std::thread::Builder::new().name(format!("{AGENT}-{}", run.lane.as_deref().unwrap_or(&run.node))).spawn(move || run_leg(&c, &r)) {
        finish(ctx, &run, "", Err(format!("thread: {e}")));
    }
}

fn log_name(run: &Run) -> String {
    match &run.lane { Some(lane) => format!("lane-{lane}.log"), None => format!("{}.log", run.node) }
}

fn run_leg(ctx: &Ctx, run: &Run) {
    let mut dropped = String::new();
    let result = (|| -> Result<String, String> {
        let dir = ctx.logs("warm");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // Append: an old generation can still be returning when its successor starts.
        let mut log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(log_name(run))).map_err(|e| e.to_string())?;
        let _ = writeln!(log, "warm generation {}: {} {}", run.generation, run.key.rev, run.key.id);
        let (packages, note) = resolve(&ctx.repo, &run.key.rev, &run.key.packages);
        dropped = note;
        if !dropped.is_empty() { let _ = writeln!(log, "{dropped}"); }
        let (lane, prio) = match &run.lane {
            Some(lane) => (format!("{WARM_LANE_PREFIX}{lane}"), leg::BACKGROUND_PRIO),
            None => (format!("{PREBUILD_LANE_PREFIX}{}", run.node), leg::OWED_PRIO),
        };
        let spec = Spec { lane, agent: AGENT.into(), kind: Kind::Build, needs: String::new(), prio, rev: run.key.rev.clone(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: Some(run.node.clone()), only: true, since_ms: run.since_ms };
        if run.native { return native_leg(run, &spec, &packages, &mut log); }
        // A JIT appearing in caps is no longer completion: the code cache still needs warming.
        let stale = || !set(run, |_| {});
        // a request still queued gives way to a master restart's drain and asks again after it
        let give_up = || stale() || super::roll::draining();
        loop {
            if stale() { return Ok(String::new()); }
            // counted while it asks and runs; waits out a drain (the exec ends this thread)
            let _drain = super::roll::holding();
            let mut on_wait = |line: &str| {
                let _ = writeln!(log, "{line}");
                set(run, |p| p.note = line.into());
            };
            let held = match Held::acquire_with(&spec, &|r| {
                r.src = Src::Overlay;
                r.cores = Some(if run.lane.is_none() { 4.0 } else { 2.0 });
                // RAM as measured (a dev JIT build and a test listing peak at 2.4 GB): an 8 GB ask
                // kept prebuilds off the 16 GB boxes beside a live session for minutes (11-02)
                if run.lane.is_none() { r.priority = crate::lease::Priority::Interactive; }
                if run.os == "macos" { r.needs.push(("os".into(), "macos".into())); }
            }, &mut on_wait, &give_up) {
                Ok(held) => held,
                Err(_) if stale() => return Ok(String::new()),
                Err(_) if super::roll::draining() => continue,
                Err(e) if e.split_whitespace().next().is_some_and(leg::was_preempted) => continue,
                Err(e) if missing_commit(&e) => {
                    let _ = writeln!(log, "{e}; retrying in {} s", RETRY.as_secs());
                    set(run, |p| p.note = format!("waiting for commit: {e}"));
                    ctx.store.event("warm", Sev::Warn, format!("warm {} on {}: {e}; retrying", run.key.rev, run.node), None);
                    let until = Instant::now() + RETRY;
                    while Instant::now() < until && !stale() { std::thread::sleep(Duration::from_secs(1)); }
                    continue;
                }
                Err(e) => return Err(e),
            };
            if !set(run, |p| { p.state = "held".into(); p.note.clear(); p.lease = Some(held.id.clone()); }) { return Ok(String::new()); }
            let mut failures = Vec::new();
            let result = held.ship_nothing().and_then(|_| {
                if stale() { return Ok(0); }
                set(run, |p| p.state = "building".into());
                let argv = ["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "--version"].map(String::from);
                let code = held.run(&argv, &mut log)?;
                if code != 0 { return Ok(code); }
                for package in &packages {
                    if stale() { break; }
                    let label = package.label();
                    set(run, |p| p.state = format!("warming {label}"));
                    let _ = writeln!(log, "warming {label}");
                    let failure = match held.run(&package.argv(), &mut log) {
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
            set(run, |p| p.lease = None);
            if stale() { return Ok(String::new()); }
            if preempted {
                let _ = writeln!(log, "preempted for lane work; asking again");
                set(run, |p| { p.state = "waiting".into(); p.note = "preempted for lane work".into(); });
                continue;
            }
            return match result { Ok(0) => Ok(failures.join("; ")), Ok(code) => Err(format!("exit {code}")), Err(e) => Err(e) };
        }
    })();
    finish(ctx, run, &dropped, result);
}

fn native_leg(run: &Run, spec: &Spec, packages: &[Package], log: &mut dyn Write) -> Result<String, String> {
    let stale = || !set(run, |_| {});
    // a request still queued gives way to a master restart's drain and asks again after it
    let give_up = || stale() || super::roll::draining();
    let mut on_wait = |line: &str| {
        let _ = writeln!(log, "{line}");
        set(run, |p| p.note = line.into());
    };
    // counted while it asks and runs; waits out a drain (the exec ends this thread)
    let _drain;
    let held = loop {
        if stale() { return Ok(String::new()); }
        let drain = super::roll::holding();
        match Held::acquire_with(spec, &|r| {
            r.warms = run.lane.clone();
            r.cores = Some(4.0);
            r.ram_gb = Some(8.0);
            if run.os == "macos" { r.needs.push(("os".into(), "macos".into())); }
        }, &mut on_wait, &give_up) {
            Ok(held) => {
                _drain = drain;
                break held;
            }
            Err(_) if !stale() && super::roll::draining() => continue,
            Err(e) => {
                if let Some(id) = e.split_whitespace().next() { let _ = leg::was_preempted(id); }
                return Err(e);
            }
        }
    };
    if !set(run, |p| { p.state = "held".into(); p.note.clear(); p.lease = Some(held.id.clone()); }) { return Ok(String::new()); }
    let result = (|| -> Result<i32, String> {
        if stale() { return Ok(0); }
        if packages.is_empty() {
            set(run, |p| p.state = "syncing".into());
            return held.run(&["git", "rev-parse", "HEAD"].map(String::from), log);
        }
        for package in packages {
            if stale() { return Ok(0); }
            let label = package.label();
            set(run, |p| p.state = format!("warming {label}"));
            let _ = writeln!(log, "warming native {label}");
            let code = held.run(&package.native_argv(&run.os), log)?;
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
    if changed {
        let who = run.lane.as_deref().unwrap_or(&run.key.rev);
        let line = |note: &str| format!("warm {who} on {}: {state} in {seconds} s{}", run.node, if note.is_empty() { String::new() } else { format!(": {note}") });
        if let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(ctx.logs("warm").join(log_name(run))) {
            // Drops were logged before acquiring the lease.
            let _ = writeln!(log, "{}", line(&result_note));
        }
        ctx.store.event("warm", sev, line(&note), None);
    }
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
    }
    for n in v.get("spares").and_then(Value::as_arr).unwrap_or(&[]) {
        let native = if s(n, "src") == "tree" { "native " } else { "" };
        out.push_str(&format!("  {} spare {} {native}{:<8} {}s {} {}\n", s(n, "lane"), s(n, "node"), s(n, "state"), n.get("seconds").and_then(Value::as_u64).unwrap_or(0), s(n, "id"), s(n, "note")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::Progress;

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
        let plan = super::plan_nodes(&nodes, &by_compiler, &activity, "work@x", 1);
        assert_eq!(plan.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["busy", "idle"]);
        assert!(plan.iter().all(|n| n.key.id == "new"));
    }

    #[test]
    fn a_failure_clears_once_its_boxs_store_holds_the_jit() {
        let id = "66f764de1aae36f89242ec4f";
        let mut p = Progress { state: "failed".into(), note: "refused: 105: x".into(), ..Progress::waiting(id, 1) };
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
}
