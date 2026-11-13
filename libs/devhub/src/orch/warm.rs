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
        // the release profile, as `devhub test` runs the package (lease/client.rs)
        let mut argv: Vec<_> = ["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "test", &self.pkg, "--release"].into_iter().map(String::from).collect();
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
        for id in p.nodes.iter().filter_map(|n| n.progress.lease.as_deref()) { ctx.leases.release_live(id); }
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
    for (rank, node) in plan.nodes.iter().enumerate().filter(|(_, n)| !followed.contains(&n.name)) {
        // Keep every timestamp in the past: the scheduler clamps future queue times to now.
        let since_ms = Some(plan.started_ms.saturating_sub((plan.nodes.len() - rank) as u64));
        spawn(ctx, Run { generation: plan.generation, lane: None, native: false, node: node.name.clone(), os: node.os.clone(), key: node.key.clone(), started_ms, since_ms });
    }
}

/// The boxes a new JIT id is built on: every up dev box that lacks it, per OS/arch group the most
/// lane-active first. A box no lane used lately builds it too (the next session may come to it),
/// listing no tests; so does a bench box, whose tests are its timed runs' own (the listing held
/// it a minute more per landing while timed work waited "held for interactive", 11-03).
fn plan_nodes(nodes: &[NodeView], by_compiler: &BTreeMap<String, String>, activity: &Activity, rev: &str, started_ms: u64) -> Vec<Node> {
    let mut groups: BTreeMap<(String, String), Vec<Node>> = BTreeMap::new();
    for n in nodes.iter().filter(|n| n.up && n.dev && !n.role().users()) {
        let Some(caps) = &n.caps else { continue };
        let Some(store) = &caps.jit_store else { continue };
        let Some(id) = by_compiler.get(&store.compiler) else { continue };
        if store.ids.contains(id) { continue; }
        let b = activity.boxes.get(&n.name);
        let packages = if n.role() == Role::Bench { Vec::new() } else { b.map(BoxActivity::packages).unwrap_or_default() };
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
        let first = l.rev.split(',').next().unwrap_or("");
        if let Some((branch, _)) = first.split_once('@') {
            if !matches!(branch, "" | "work" | "main") && *done.entry(branch).or_insert_with(|| crate::gitread::rev(&ctx.repo, &format!("refs/heads/{branch}")).is_none()) { return None; }
        }
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
        runs.push(Run { generation, lane: Some(lane.clone()), native, node, os: caps.os.clone(), key, started_ms: now, since_ms: None });
    }
    drop(spares);
    for id in release { ctx.leases.release_live(&id); }
    for run in runs { spawn(ctx, run); }
}

/// The measurement class of the JIT warm legs (prebuilds and lane warms), whatever they run.
const WARM_CLASS: &str = "build jit warm";

fn spawn(ctx: &Arc<Ctx>, run: Run) {
    let (c, r) = (ctx.clone(), run.clone());
    if let Err(e) = std::thread::Builder::new().name(format!("{AGENT}-{}", run.lane.as_deref().unwrap_or(&run.node))).spawn(move || run_leg(&c, &r)) {
        finish(ctx, &run, "", Err(format!("thread: {e}")));
    }
}

fn log_name(run: &Run) -> String {
    match &run.lane { Some(lane) => format!("lane-{lane}.log"), None => format!("{}.log", run.node) }
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
        // a request still queued gives way to a master restart's drain and asks again after it
        let give_up = || (if prebuild { gone() } else { stale() }) || super::roll::draining();
        loop {
            if stale() { return Ok(String::new()); }
            if prebuild {
                let r = run.borrow();
                let plan = PLAN.lock().unwrap();
                if !plan.as_ref().is_some_and(|p| p.generation == r.generation) { return Ok(String::new()); }
                ASKING.lock().unwrap().insert(r.node.clone(), r.generation);
            }
            let spec = spec_of(&run.borrow());
            // counted while it asks and runs; waits out a drain (the exec ends this thread)
            let _drain = super::roll::holding();
            let mut on_wait = |line: &str| {
                let _ = writeln!(log, "{line}");
                set(&run.borrow(), |p| p.note = line.into());
            };
            // a newer plan's target for its box, taken in place while it asks
            let mut follow = || {
                if !prebuild { return None; }
                let mut r = run.borrow_mut();
                let plan = PLAN.lock().unwrap();
                if plan.as_ref().is_none_or(|p| p.generation == r.generation) { return None; }
                let before = r.key.rev.clone();
                if !adopt(&plan, &mut r) { return None; }
                ASKING.lock().unwrap().insert(r.node.clone(), r.generation);
                (r.key.rev != before).then(|| r.key.rev.clone())
            };
            let asked = Held::acquire_following(&spec, &|r| {
                r.src = Src::Overlay;
                r.cores = Some(if prebuild { 4.0 } else { 2.0 });
                // RAM as measured (a dev JIT build and a test listing peak at 2.4 GB): an 8 GB ask
                // kept prebuilds off the 16 GB boxes beside a live session for minutes (11-02).
                // Measured under a class of their own: unnamed, they were measured as nothing and
                // took a build's 12 GB default (three pool boxes' prebuilds waited 23 min an hour
                // on RAM, holding their box meanwhile, 11-03)
                r.class = WARM_CLASS.into();
                if run.borrow().os == "macos" { r.needs.push(("os".into(), "macos".into())); }
            }, &mut on_wait, &give_up, &mut follow);
            if prebuild {
                // out of the asking legs; a plan that came meanwhile left its box to this leg
                let mut r = run.borrow_mut();
                let plan = PLAN.lock().unwrap();
                let mut asking = ASKING.lock().unwrap();
                if asking.get(&r.node) == Some(&r.generation) { asking.remove(&r.node); }
                drop(asking);
                if !adopt(&plan, &mut r) {
                    drop((plan, r));
                    if let Ok(held) = asked { held.release(None); }
                    return Ok(String::new());
                }
            }
            let held = match asked {
                Ok(held) => held,
                Err(_) if stale() => return Ok(String::new()),
                Err(_) if super::roll::draining() => continue,
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
            if !set(&r, |p| { p.state = "held".into(); p.note.clear(); p.lease = Some(held.id.clone()); p.listed.clear(); }) { return Ok(String::new()); }
            let (packages, note) = resolve(&ctx.repo, &r.key.rev, &r.key.packages, true);
            dropped = note;
            if !dropped.is_empty() { let _ = writeln!(log, "{dropped}"); }
            let mut failures = Vec::new();
            let result = held.ship_nothing().and_then(|_| {
                if stale() { return Ok(0); }
                set(&r, |p| p.state = "building".into());
                let argv = ["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "--version"].map(String::from);
                let code = held.run(&argv, &mut log)?;
                if code != 0 { return Ok(code); }
                for package in &packages {
                    if stale() { break; }
                    let label = package.label();
                    set(&r, |p| p.state = format!("warming {label}"));
                    let _ = writeln!(log, "warming {label}");
                    let mut tee = Tee(&mut log, Vec::new());
                    let result = held.run(&package.argv(), &mut tee);
                    // a listing that ran is a verdict on the tip (a lease's error is none); a failed
                    // one keeps its output (its errors), a passing one only its line
                    if let Ok(code) = result {
                        let output = if code == 0 { String::new() } else { String::from_utf8_lossy(&tee.1).into_owned() };
                        set(&r, |p| p.listed.push(Listed { unit: format!("suite {}", package.pkg), ok: code == 0, text: format!("{label} on {}: exit {code}\n{output}", r.node) }));
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
    if changed && run.lane.is_none() {
        let done = listings_done(PLAN.lock().unwrap().as_mut(), run.generation);
        if let Some(plan) = done { judge(ctx, &plan, false); }
    }
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
    use super::{Key, Listed, Node, Package, Plan, Progress, Role, Src};

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
        assert_eq!(argv, ["motion --release --test effect_modules --features scene3d -- --list", "motion --release --test plain -- --list", "motion --release --bin viewer --features scene3d,gpu -- --list", "motion --release --lib -- --list"]);
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
        let plan = super::plan_nodes(&nodes, &by_compiler, &activity, "work@x", 1);
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
        Node { name: name.into(), os: "linux".into(), activity: 0, key: Key { id: "new".into(), rev: "work@x".into(), packages }, progress: Progress { ended_ms, listed, ..Progress::waiting("new", 1) } }
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
        super::judge(&ctx, &plan(vec![node("a", &["x"], 5, vec![broken]), node("b", &["y"], 5, vec![fine.clone()])]), false);
        let rows: Vec<_> = crate::bucket::ledger(&home, 10).iter().map(|l| makepad_strict_json::parse(l.as_bytes()).unwrap()).collect();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.get("bucket").and_then(|v| v.as_str()), Some("prebuild"));
        assert_eq!(row.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(row.get("failing").and_then(|v| v.as_arr()).map(|f| f.iter().filter_map(|u| u.as_str()).collect::<Vec<_>>()), Some(vec!["suite x"]));
        assert_eq!(row.get("errors").and_then(|e| e.get("suite x")).and_then(|v| v.as_arr()).map(|e| e.len()), Some(1), "{}", row.to_json());
        assert!(row.get("passed").and_then(|v| v.as_arr()).is_some_and(|p| p.iter().any(|u| u.as_str() == Some("suite y"))));
        {
            let fed = fed.lock().unwrap();
            let red = fed.iter().find(|r| r.kind == "bucket").expect("a bucket line on the feed");
            assert!(red.sev == Sev::Err && red.line.contains("bucket prebuild") && red.line.contains("FAILED suite x"), "{}", red.line);
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
