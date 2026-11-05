//! The self-update: when a landing changes a service's code, devhub updates it on every box by
//! itself, with no rollout script. Hub updates never stop lease placement.
//! A service is affected when the landing changes a file its package's binary is built from: the
//! crates of cargo's resolved graph ([`linked_dirs`]), minus what no build reads ([`builds`]).
//! A roll starts once the service's code has been still for [`SETTLE_MS`]: landings come in bursts.
//! Each roll leaves two lines in `rolls.jsonl` (`devhub rolls`): its target and why, then how it ended.
//! - the node hubs (they run apps/devhub, libs/devhub with its panel; only libs/devhub's graph rolls
//!   them, and what changed in the panel's alone comes along with the next roll): their target is
//!   the newest devhub build the master completed (its
//!   per-landing CLI build, [`prebuild_cli`]: never a tip that did not build, never older than a hub
//!   runs, the same for every box), so they never wait for the master's own restart (Mac hubs lagged
//!   the deployment for its 30 min uptime, 11-03). A hub newer than the master works with it; a
//!   landing that raises the hub protocol ([`crate::lease::HUB_PROTO`]) waits for the master to run
//!   it first, stated on each box as `stalled: master older than protocol N`. The deployment gets
//!   the code (the auto-sync, kicked; a push for the boxes outside it, which a timed run there makes
//!   wait: an update wait, said once) while leases keep using the work tree. Once `caps.deploy` has
//!   the code, the master asks for `hub-restart`; the hub
//!   builds its new self first (`waits:` meanwhile) and refuses a deployment it does not build at
//!   (said once per commit, loudly; the box is not asked again until its deployment moves), then
//!   restarts at once: a restart ends nothing (runs and live apps go on,
//!   their wrappers record exits, the new hub takes leases, runs and answered starts back from its
//!   ledger; a dev JIT store build's cargo goes on in its own scope and the new hub waits for it,
//!   lease/jitstore.rs; holders see a reconnect). Only a sync apply or an in-hub selftest makes it wait
//!   seconds (Windows: its runs, job objects that end with the hub). The hub exits and what keeps
//!   it (the WM's supervisor, the Mac's LaunchAgent) starts it again through `cargo run`. Its caps
//!   then name the commit it runs (`devhub nodes`: `hub <sha>`). A hub before protocol 6
//!   ([`crate::lease::HANDOVER_PROTO`]) restarts only between runs: its box takes no new lease
//!   until it has (once per box). Only requests needing a newer protocol wait for it. A hub from
//!   before the self-update needs one restart by hand (it is said once).
//! - the services the WM supervises (the AI hub): `service-restart` once `caps.deploy` has the code
//!   and the node's apply has built it, only when the hub does not run that build already (a
//!   restart reloads its models; `current` answers)
//!   (SIGTERM: it hands its jobs back; the WM starts it again through `cargo run`), never while
//!   a user is at the box (input within `user_idle_min`) or an app holds unsaved work: the roll
//!   waits until the box is idle. The hub itself restarts regardless (the user never sees it); a
//!   WM restart waits the same way on the node ([`crate::lease::exec::user_blockers`]).
//! - the master itself, never held up by the node rolls and ending nothing: what runs under a lease
//!   lives on the node, and the new master takes it back (jobs, live jobs, verify legs and CI
//!   shards go on from where their logs stand; queued ones stay queued). Only an allapps run is
//!   started over, so it is waited for up to [`MASTER_DEADLINE`]. It builds first: a build that
//!   fails keeps it running and is an inbox error (`devhub master restart` waits for the build and
//!   answers with it; it never waits out the self-update's retry pause). It builds devhub's own
//!   small binary (never the JIT) in a target dir of its own, swaps it in at
//!   `target/host/makepad-devhub` and execs it with its own args ([`build_devhub`]) (the node
//!   rolls start over from the new master).
//! - the WM (with its window handover, never over unsaved work) and the dock apps (patched live,
//!   or a restart offered) are the auto-sync's apply (`restart=auto`) and live deploy.
//!
//! Every step is an `update` event.

use super::{git, Ctx};
use crate::lease::sched::{deploy_to, DeployTo};
use crate::proto::{wall_ms, Sev};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// User-facing service rolls started at once. Hub requests need no slot.
pub const AT_ONCE: usize = 1;
const TICK: Duration = Duration::from_secs(15);
/// How often a roll that waits for a box's user looks again.
const USER_WAIT: Duration = Duration::from_secs(30);
/// An update waits this long before it says so (it keeps waiting: no lease is ended).
const DRAIN_SAY: Duration = Duration::from_secs(1800);

/// A service roll waiting this long gives its slot back: one busy box cannot hold the fleet.
const SLOT_HOLD: Duration = Duration::from_secs(300);

/// One of the [`AT_ONCE`] service rolls, held until it ends (or [`SLOT_HOLD`]).
struct Slot {
    drains: Arc<AtomicUsize>,
    held: bool,
}

impl Slot {
    fn try_take(&mut self) -> bool {
        let mut n = self.drains.load(Ordering::Acquire);
        while !self.held && n < AT_ONCE {
            match self.drains.compare_exchange(n, n + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => self.held = true,
                Err(now) => n = now,
            }
        }
        self.held
    }

    fn give_back(&mut self) {
        if std::mem::take(&mut self.held) {
            self.drains.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn take(&mut self) {
        while !self.try_take() {
            std::thread::sleep(Duration::from_secs(5));
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        if self.held {
            self.drains.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// The services the self-update restarts on nodes: (name, the package whose graph rolls it).
pub const SERVICES: [(&str, &str); 2] = [("devhub", "libs/devhub"), ("ai-hub", "apps/aihub")];

/// The repo dirs of the crates the binary of the package at `pkg` is built from: cargo's resolved
/// graph, for every target. Reading `path` lines is not enough for this: an optional dependency
/// no feature turns on reads as one (libs/ai/hub's took platform, draw and 100 more crates into
/// libs/devhub's closure, 157 crates to the 48 it builds).
fn linked_dirs(repo: &Path, pkg: &str) -> Result<BTreeSet<String>, String> {
    // (no --locked: the lockfile is not tracked; each checkout's own goes stale with a landing's new
    // crate, and cargo refreshes it here as any build in this checkout would)
    let o = std::process::Command::new(cargo_bin()).current_dir(repo).args(["tree", "--offline", "-e", "normal,build", "--prefix", "none", "--target", "all", "--manifest-path"]).arg(format!("{pkg}/Cargo.toml")).output().map_err(|e| format!("cargo: {e}"))?;
    if !o.status.success() {
        return Err(crate::proto::clean(&String::from_utf8_lossy(&o.stderr), 300));
    }
    let root = repo.canonicalize().map_err(|e| format!("{}: {e}", repo.display()))?;
    Ok(tree_dirs(&String::from_utf8_lossy(&o.stdout), &root.to_string_lossy()))
}

/// The crates under `root` of a `cargo tree --prefix none` listing, relative to it.
fn tree_dirs(listing: &str, root: &str) -> BTreeSet<String> {
    let marker = format!(" ({}/", root.trim_end_matches('/'));
    listing.lines().filter_map(|l| {
        let rest = &l[l.find(&marker)? + marker.len()..];
        Some(rest[..rest.find(')')?].to_string())
    }).collect()
}

/// Whether a build reads this file of a crate (its path below the crate's dir): a CI proof's stamp,
/// the crate's tests and its notes are no code a service runs.
fn builds(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    name != "ci.toml" && !name.ends_with(".md") && !["tests/", "benches/", "examples/"].iter().any(|d| file.starts_with(d))
}

/// What the commits from `from` to `to` change of what `dirs` build, as repo paths ("" in `dirs`:
/// the whole repo, [`service_dirs`]). None: a commit is not known here.
fn built_changes(repo: &Path, from: &str, to: &str, dirs: &BTreeSet<String>) -> Option<Vec<String>> {
    let changed = crate::gitread::files_differing_under(repo, from, to, dirs.iter().map(String::as_str))?;
    Some(changed.into_iter().filter(|(_, file)| builds(file)).map(|(dir, file)| if dir.is_empty() { file } else { format!("{dir}/{file}") }).collect())
}

/// Each service's crates at this landing, and why they are the whole repo. A landing whose graph
/// cargo cannot read rolls every service: a missed update is the worse error.
fn service_dirs(repo: &Path) -> (HashMap<String, BTreeSet<String>>, Option<String>) {
    match SERVICES.iter().map(|(n, d)| Ok((n.to_string(), linked_dirs(repo, d)?))).collect::<Result<HashMap<_, _>, String>>() {
        Ok(dirs) => (dirs, None),
        Err(e) => (SERVICES.iter().map(|(n, _)| (n.to_string(), BTreeSet::from([String::new()]))).collect(), Some(format!("cargo does not read the services' crates ({e}): the whole repo counts"))),
    }
}

/// `rolls.jsonl` in devhub's home.
fn rolls_path() -> PathBuf {
    crate::proto::home().join("rolls.jsonl")
}

/// Past this `rolls.jsonl` is cut to half of it when a master starts: over a week of rolls at
/// 100 a day on 10 boxes.
const ROLLS_KEEP: usize = 8 << 20;

/// The tail of a record that grew past [`ROLLS_KEEP`], from a whole line.
fn rolls_kept(text: &str) -> Option<&str> {
    if text.len() <= ROLLS_KEEP {
        return None;
    }
    let mut at = text.len() - ROLLS_KEEP / 2;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    Some(text[at..].split_once('\n').map_or("", |(_, whole)| whole))
}

/// What `rolls.jsonl` says of a change: its count and the first few files.
fn why_of(changed: &[String]) -> String {
    const NAMED: usize = 3;
    let mut why = format!("{} file{}: {}", changed.len(), if changed.len() == 1 { "" } else { "s" }, changed.iter().take(NAMED).cloned().collect::<Vec<_>>().join(", "));
    if changed.len() > NAMED {
        why.push_str(&format!(" +{}", changed.len() - NAMED));
    }
    why
}

/// A line of the roll's record, kept when a master restarts in the middle of a roll or a box is
/// gone for good. `fields`: the line's own, by name.
fn note_roll(node: &str, svc: &str, fields: &[(&str, &str)]) {
    use std::io::Write;
    let rest: String = fields.iter().map(|(k, v)| format!(",{}:{}", makepad_remote::json_str(k), makepad_remote::json_str(v))).collect();
    let line = format!("{{\"ms\":{},\"node\":{},\"svc\":{}{rest}}}\n", wall_ms(), makepad_remote::json_str(node), makepad_remote::json_str(svc));
    let wrote = std::fs::OpenOptions::new().create(true).append(true).open(rolls_path()).and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = wrote {
        eprintln!("devhub: {}: {e}", rolls_path().display());
    }
}

/// `devhub rolls [--node N] [n]`.
pub fn rolls_cli(args: &[&str]) -> i32 {
    let at = args.iter().position(|a| *a == "--node");
    let node = at.and_then(|i| args.get(i + 1)).copied();
    let n = args.iter().enumerate().filter(|(i, _)| at.is_none_or(|a| *i != a + 1)).find_map(|(_, a)| a.parse().ok()).unwrap_or(30);
    match std::fs::read_to_string(rolls_path()) {
        Ok(text) => {
            println!("{}", rolls_text(&text, node, n, &super::stamp));
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("no rolls yet");
            0
        }
        Err(e) => {
            eprintln!("makepad devhub rolls: {}: {e}", rolls_path().display());
            2
        }
    }
}

/// The newest `n` lines of the record, oldest first.
fn rolls_text(text: &str, node: Option<&str>, n: usize, clock: &dyn Fn(u64) -> String) -> String {
    let lines: Vec<String> = text.lines().filter_map(|l| {
        let v = makepad_strict_json::parse(l.as_bytes()).ok()?;
        let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if node.is_some_and(|w| w != field("node")) {
            return None;
        }
        let short = |k: &str| { let f = field(k); f[..f.len().min(10)].to_string() };
        let at = clock(v.get("ms").and_then(|x| x.as_u64()).unwrap_or(0));
        Some(match field("ended").as_str() {
            "" => format!("{at} {} {}: {} -> {}: {}", field("node"), field("svc"), short("from"), short("to"), field("why")),
            ended => format!("{at} {} {}: {} ended: {ended}", field("node"), field("svc"), short("to")),
        })
    }).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The repo dirs of `pkg` and every crate it depends on by path (workspace deps resolved through
/// the root Cargo.toml), relative to `repo`.
pub fn crate_dirs(repo: &Path, pkg: &str) -> BTreeSet<String> {
    crate_dirs_from(pkg, &|p| std::fs::read_to_string(repo.join(p)).ok())
}

/// The same closure in a git tree, without moving a checkout.
pub fn crate_dirs_from(pkg: &str, read: &dyn Fn(&str) -> Option<String>) -> BTreeSet<String> {
    let root = read("Cargo.toml").unwrap_or_default();
    let mut ws: HashMap<String, String> = HashMap::new();
    let mut in_ws = false;
    for l in root.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            in_ws = t == "[workspace.dependencies]";
            continue;
        }
        if in_ws {
            if let (Some((name, _)), Some(p)) = (t.split_once('='), path_of(t)) {
                ws.insert(name.trim().to_string(), p);
            }
        }
    }
    let mut out = BTreeSet::new();
    let mut todo = vec![pkg.trim_end_matches('/').to_string()];
    while let Some(dir) = todo.pop() {
        if !out.insert(dir.clone()) {
            continue;
        }
        let text = read(&format!("{dir}/Cargo.toml")).unwrap_or_default();
        for l in text.lines() {
            let t = l.trim();
            let rel = if let Some(p) = path_of(t) {
                Some(normalize(&format!("{dir}/{p}")))
            } else if t.contains("workspace = true") || t.contains("workspace=true") {
                t.split_once('=').and_then(|(n, _)| ws.get(n.trim()).cloned())
            } else {
                None
            };
            if let Some(r) = rel.filter(|r| read(&format!("{r}/Cargo.toml")).is_some()) {
                todo.push(r);
            }
        }
    }
    out
}

/// `path = "x"` in a toml line.
fn path_of(line: &str) -> Option<String> {
    let i = line.find("path")?;
    let rest = line[i + 4..].trim_start().strip_prefix('=')?.trim_start().strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

/// `a/b/../c` -> `a/c`.
fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out.join("/")
}

/// Whether a tree at `at` lacks the code of `to` in `dirs`: false when `at` contains `to` (a later
/// landing moved it past) or differs from it nowhere in `dirs`.
fn lacks(repo: &Path, at: &str, to: &str, dirs: &BTreeSet<String>) -> bool {
    if at.is_empty() {
        return true;
    }
    if crate::gitread::is_ancestor(repo, to, at) {
        return false;
    }
    touches(repo, at, to, dirs)
}

/// Whether the commits from `from` to `to` change anything `dirs` build.
fn touches(repo: &Path, from: &str, to: &str, dirs: &BTreeSet<String>) -> bool {
    if from == to {
        return false;
    }
    // (an unknown commit counts as a change)
    built_changes(repo, from, to, dirs).is_none_or(|c| !c.is_empty())
}

/// The newest commit of `to` that touched `dirs`, as wall ms (its commit time). The answer for one
/// commit never changes: asked once per landing and dir set, not per tick.
fn last_change_ms(repo: &Path, to: &str, dirs: &BTreeSet<String>) -> u64 {
    static KNOWN: Mutex<Option<(String, HashMap<BTreeSet<String>, u64>)>> = Mutex::new(None);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if known.as_ref().is_none_or(|(at, _)| at != to) {
        *known = Some((to.to_string(), HashMap::new()));
    }
    let Some((_, by_dirs)) = known.as_mut() else { return 0 };
    if let Some(ms) = by_dirs.get(dirs) {
        return *ms;
    }
    let mut args = vec!["log", "-1", "--format=%ct", to, "--"];
    args.extend(dirs.iter().map(|d| if d.is_empty() { "." } else { d.as_str() }));
    let ms = git(repo, &args).ok().and_then(|s| s.trim().parse::<u64>().ok()).map_or(0, |s| s * 1000);
    by_dirs.insert(dirs.clone(), ms);
    ms
}

fn event(ctx: &Ctx, sev: Sev, line: String) {
    ctx.store.event("update", sev, line, None);
}

pub fn start(ctx: Arc<Ctx>) {
    let started = Instant::now();
    if let Some(kept) = std::fs::read_to_string(rolls_path()).ok().as_deref().and_then(rolls_kept) {
        if let Err(e) = std::fs::write(rolls_path(), kept) {
            event(&ctx, Sev::Warn, format!("update: {}: {e}", rolls_path().display()));
        }
    }
    // the hubs' target: the commit this master was built at, read now (lazily it would be
    // whatever the checkout moved to since)
    let _ = crate::lease::caps::hub_rev();
    let _ = std::thread::Builder::new().name("devhub-update".into()).spawn(move || {
        let busy: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(BTreeSet::new()));
        // node -> the deployment its new hub did not build at: no roll again until it moves
        let broken: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
        // (box, service) that said it runs the deployment's build already, at which last change and
        // deployment: asked again when one of them moves, not every tick
        let current: Arc<Mutex<HashMap<(String, String), (u64, String)>>> = Arc::new(Mutex::new(HashMap::new()));
        let drains = Arc::new(AtomicUsize::new(0));
        // a self-update whose build failed is said loudly and tried again later, not every tick
        let (mut master_next, mut master_wait) = (None::<Instant>, MASTER_RETRY);
        let mut update_master = |work: &str| {
            if master_next.is_some_and(|at| Instant::now() < at) {
                return;
            }
            if restart_master(&ctx, work, "its self-update").is_err() {
                // said loudly (inbox); it keeps running and tries again later, not every tick
                event(&ctx, Sev::Info, format!("update: the master tries {} again in {} min (or `devhub master restart`)", super::short(work), master_wait.as_secs() / 60));
                master_next = Some(Instant::now() + master_wait);
                master_wait = (master_wait * 2).min(MASTER_RETRY_MAX);
            }
        };
        let mut said: BTreeSet<String> = BTreeSet::new();
        let mut closures: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut closures_at = String::new();
        // why this landing's closures are the whole repo
        let mut closures_why: Option<String> = None;
        let mut wait_since: Option<Instant> = None;
        let mut fds_said = 1000;
        // the commit the CLI binary was last built at (this master's build at first), and the
        // newest one that built
        let mut cli_at = crate::lease::caps::hub_rev().to_string();
        *CLI_GOOD.lock().unwrap() = cli_at.clone();
        // the boxes whose hub waits for the master's protocol, and what they show
        let mut proto_held: HashMap<String, String> = HashMap::new();
        let mut tips_at = String::new();
        loop {
            std::thread::sleep(TICK);
            note_fds(&ctx, &mut fds_said);
            if RESTART_ASKED.swap(false, std::sync::atomic::Ordering::AcqRel) {
                if let Some(w) = crate::gitread::rev(&ctx.repo, "refs/heads/work") {
                    update_master(&w);
                }
            }
            let Some(work) = crate::gitread::rev(&ctx.repo, "refs/heads/work") else { continue };
            // where a box catches up to, and how far behind one is
            if let Some(main) = crate::gitread::rev(&ctx.repo.join("apps/commercial"), "refs/heads/main") {
                let tips = format!("{work},{main}");
                if tips != tips_at {
                    ctx.leases.set_tips(&tips, landed_since(&ctx, wall_ms().saturating_sub(LANDINGS_KNOWN.as_millis() as u64)));
                    tips_at = tips;
                }
            }
            if closures_at != work {
                (closures, closures_why) = service_dirs(&ctx.repo);
                closures_at = work.clone();
                if let Some(why) = &closures_why {
                    event(&ctx, Sev::Warn, format!("update: {} at {why}", super::short(&work)));
                }
            }
            prebuild_cli(&ctx, &work, &closures["devhub"], &mut cli_at);
            // the hubs' target: the newest devhub build the master completed, once its protocol is
            // one this master speaks (a hub newer than the master works with it otherwise)
            let good = cli_good();
            let target = hub_target(&good, hub_proto_at(&ctx.repo, &good), crate::lease::HUB_PROTO);
            if let (Err(why), false) = (&target, good.is_empty()) {
                if said.insert(format!("proto {good}")) {
                    event(&ctx, Sev::Info, format!("update: the hubs roll to {} after the master does: stalled: {why}", super::short(&good)));
                }
            }
            let settled = last_change_ms(&ctx.repo, &work, &closures["devhub"]) + SETTLE_MS <= wall_ms();
            // each service rolls once its own code settled: the hub's frequent landings never hold
            // back an AI hub whose code is still (11-01: the fleet's AI hubs ran 1 h old code)
            let changed: HashMap<&str, u64> = SERVICES.iter().skip(1).map(|(s, _)| (*s, last_change_ms(&ctx.repo, &work, &closures[*s]))).collect();
            // never a tip that did not build (11-02: hubs rolled onto a landing that broke
            // makepad-devhub, which the master's build had refused minutes before)
            let mine = crate::lease::caps::hub_rev();
            for n in ctx.leases.nodes().into_iter().filter(|n| n.up && !matches!(n.role(), crate::lease::Role::Device)) {
                let Some(c) = n.caps.clone() else { continue };
                if busy.lock().unwrap().contains(&n.name) {
                    continue;
                }
                // the hub
                let hub = &closures["devhub"];
                let lags = !c.hub_rev.is_empty()
                    && !good.is_empty()
                    && !crate::gitread::is_ancestor(&ctx.repo, &good, &c.hub_rev)
                    && touches(&ctx.repo, &c.hub_rev, &good, hub)
                    && broken.lock().unwrap().get(&n.name).map(String::as_str) != Some(c.deploy.split(',').next().unwrap_or(""));
                // a box whose hub waits for the master's protocol says so while it waits
                let stalled = match (&target, lags) {
                    (Err(why), true) => Some(format!("stalled: {why} (its hub updates to {} after the master)", super::short(&good))),
                    _ => None,
                };
                if proto_held.get(&n.name) != stalled.as_ref() {
                    ctx.leases.set_updating(&n.name, stalled.as_deref(), false);
                    match stalled {
                        Some(s) => proto_held.insert(n.name.clone(), s),
                        None => proto_held.remove(&n.name),
                    };
                }
                let job = if c.hub_rev.is_empty() {
                    if said.insert(format!("old {}", n.name)) {
                        event(&ctx, Sev::Warn, format!("update: {}'s hub is from before the self-update: restart it once by hand (then devhub updates it by itself)", n.name));
                    }
                    None
                } else if lags && target.is_ok() {
                    Some(("devhub", hub.clone()))
                } else {
                    // the WM-supervised services that run there and started before their code's last change
                    SERVICES.iter().skip(1).find_map(|(s, _)| {
                        let started = c.services.iter().find(|(x, _)| x == s)?.1;
                        let dirs = &closures[*s];
                        let at = changed[*s];
                        let asked = current.lock().unwrap().get(&(n.name.clone(), s.to_string())) == Some(&(at, c.deploy.clone()));
                        (started < at && !asked && at + SETTLE_MS <= wall_ms()).then(|| (*s, dirs.clone()))
                    })
                };
                let Some((svc, dirs)) = job else { continue };
                busy.lock().unwrap().insert(n.name.clone());
                let target = if svc == "devhub" { good.clone() } else { work.clone() };
                let from = if svc == "devhub" { c.hub_rev.clone() } else { String::new() };
                let why = if svc == "devhub" {
                    built_changes(&ctx.repo, &from, &target, &dirs).map_or("the commit it runs is not known here".to_string(), |c| why_of(&c))
                } else {
                    "its code changed after it started".to_string()
                };
                let why = closures_why.as_ref().map_or(why.clone(), |c| format!("{c}: {why}"));
                let (c2, b2, d2, node, br, cur) = (ctx.clone(), busy.clone(), drains.clone(), n.name.clone(), broken.clone(), current.clone());
                let (at, deploy) = (changed.get(svc).copied().unwrap_or(0), c.deploy.clone());
                let _ = std::thread::Builder::new().name("devhub-update-node".into()).spawn(move || {
                    let end = roll_node(&c2, &node, svc, &dirs, &target, d2, &|| note_roll(&node, svc, &[("from", &from), ("to", &target), ("why", &why)]));
                    let now = c2.leases.nodes().into_iter().find(|n| n.name == node).and_then(|n| n.caps).map_or(String::new(), |c| c.hub_rev);
                    let ended = match &end {
                        RollEnd::Broke(at) => Some(format!("its new hub does not build at {}; it runs on at {}", super::short(at), super::short(&now))),
                        RollEnd::Back if svc == "devhub" => Some(format!("runs {}", super::short(&now))),
                        RollEnd::Back => Some("restarted".to_string()),
                        RollEnd::Late => Some("did not come back in 25 min".to_string()),
                        RollEnd::Current | RollEnd::NoRestart => None,
                    };
                    if let Some(ended) = ended {
                        note_roll(&node, svc, &[("to", &target), ("ended", &ended)]);
                    }
                    if matches!(end, RollEnd::Current) {
                        cur.lock().unwrap().insert((node.clone(), svc.to_string()), (at, deploy));
                    }
                    match end {
                        RollEnd::Broke(at) => br.lock().unwrap().insert(node.clone(), at),
                        _ => br.lock().unwrap().remove(&node),
                    };
                    c2.leases.set_updating(&node, None, false);
                    b2.lock().unwrap().remove(&node);
                });
            }
            // the master itself, once the landings settled: the restart ends nothing, so nothing
            // holds it up but an allapps run (for a while). Never held up by node rolls either: one
            // waits for a long lease's run boundary, landings keep coming, and a master that waited
            // for them all never updated (18:18 → 19:05, 2026-10-30). A roll it cuts short starts
            // over from the new master.
            {
                // (the master is libs/devhub's own binary)
                let dirs = &closures["devhub"];
                if !mine.is_empty() && touches(&ctx.repo, mine, &work, dirs) {
                    let since = *wait_since.get_or_insert_with(Instant::now);
                    // A restart is cheap (nothing is lost, whatever the leases are doing) but not
                    // free: one per MASTER_MIN_UPTIME, and landings of a burst are taken together
                    // unless they keep coming
                    let why = restart_blocker_of(started.elapsed(), settled, since.elapsed(), super::lane::sweeping());
                    if let Some(why) = why {
                        if said.insert(format!("master {work} {}", why.split(' ').take(2).collect::<Vec<_>>().join(" "))) {
                            event(&ctx, Sev::Info, format!("update: the master's restart on {} waits (stalled: {why})", super::short(&work)));
                        }
                    } else if ctx.store.read(|d| d.allapps.running.is_none()) || since.elapsed() >= MASTER_DEADLINE {
                        update_master(&work);
                        wait_since = None;
                    } else if said.insert(format!("master {work}")) {
                        event(&ctx, Sev::Info, format!("update: the master restarts on {} once the allapps run ends (at most {} min)", super::short(&work), MASTER_DEADLINE.as_secs() / 60));
                    }
                } else {
                    wait_since = None;
                }
            }
        }
    });
}

/// A CLI build runs now ([`prebuild_cli`]).
static CLI_BUILDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The newest commit whose devhub the master built (or whose devhub code is that of one it
/// built): the hubs' target.
static CLI_GOOD: Mutex<String> = Mutex::new(String::new());

fn cli_good() -> String {
    CLI_GOOD.lock().unwrap().clone()
}

/// A commit that changes none of the master's code since `from` keeps `from`'s build: when `from`
/// built, `to` did.
fn carry_good(good: &mut String, from: &str, to: &str) {
    if good == from {
        *good = to.to_string();
    }
}

/// Lanes call the built binary (`target/host/makepad-devhub`, through `makepad devhub`): right
/// after a landing that changes its inputs the master builds it once, a background build on 4
/// jobs, and swaps it in when done ([`build_devhub`]); a landing's CLI goes live when its rebuild
/// finishes.
/// Landings during a build get one more after it. A build that succeeds is the hubs' target
/// ([`CLI_GOOD`]).
fn prebuild_cli(ctx: &Arc<Ctx>, work: &str, dirs: &BTreeSet<String>, at: &mut String) {
    if *at == work || CLI_BUILDING.load(Ordering::Acquire) {
        return;
    }
    if at.is_empty() || !touches(&ctx.repo, at, work, dirs) {
        carry_good(&mut CLI_GOOD.lock().unwrap(), at, work);
        *at = work.to_string();
        return;
    }
    *at = work.to_string();
    CLI_BUILDING.store(true, Ordering::Release);
    let (ctx, work) = (ctx.clone(), work.to_string());
    let spawned = std::thread::Builder::new().name("devhub-cli-build".into()).spawn(move || {
        let t = Instant::now();
        match build_devhub(&ctx.repo) {
            Ok(()) => {
                *CLI_GOOD.lock().unwrap() = work.clone();
                event(&ctx, Sev::Info, format!("update: devhub's CLI is built at {} in {:.0} s; the hubs roll to it", super::short(&work), t.elapsed().as_secs_f64()));
            }
            Err(e) => event(&ctx, Sev::Warn, format!("update: devhub's CLI at {}: {e}; the hubs stay at {}", super::short(&work), super::short(&cli_good()))),
        }
        CLI_BUILDING.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        CLI_BUILDING.store(false, Ordering::Release);
    }
}

/// The master restarts for a landing at most this often (`devhub master restart` asks at once).
pub const MASTER_MIN_UPTIME: Duration = Duration::from_secs(10 * 60);

/// A devhub landing the master lacks restarts it at most this long after it was first seen, even
/// while later landings keep its code from settling ([`SETTLE_MS`]): it takes the newest then.
const MASTER_PENDING_MAX: Duration = Duration::from_secs(5 * 60);

/// Why the master's update to a devhub landing it lacks waits, if it does: it is younger than
/// [`MASTER_MIN_UPTIME`], devhub landings still come (for at most [`MASTER_PENDING_MAX`]), or a
/// lane sweep runs (its state is this process's). Nothing the leases do holds it: a restart ends
/// no lease and no run, keeps the queue, starts leases in setup again and holds a box whose
/// cleanup is unconfirmed until its node confirms it (restart_tests, sched). Waiting for a moment
/// with no lease in setup or cleanup, it never came on a busy night: up 57 min with five landings
/// pending (11-04); behind back-to-back timed leases it waited for hours (11-03).
fn restart_blocker_of(uptime: Duration, settled: bool, pending: Duration, sweeping: bool) -> Option<String> {
    if uptime < MASTER_MIN_UPTIME {
        return Some(format!("up {} min of {}", uptime.as_secs() / 60, MASTER_MIN_UPTIME.as_secs() / 60));
    }
    if sweeping {
        return Some("a lane sweep runs in the master".into());
    }
    if !settled && pending < MASTER_PENDING_MAX {
        return Some(format!("devhub landings still come: it takes the newest {} min after the first at the latest", MASTER_PENDING_MAX.as_secs() / 60));
    }
    None
}

/// Where the hub protocol is set ([`crate::lease::HUB_PROTO`]).
const PROTO_FILE: &str = "libs/devhub/src/lease/mod.rs";

/// The hub protocol a source of [`PROTO_FILE`] sets.
fn proto_in(text: &str) -> Option<u32> {
    text.lines().find_map(|l| l.trim().strip_prefix("pub const HUB_PROTO: u32 = ")?.strip_suffix(';')?.trim().parse().ok())
}

/// The hub protocol of devhub at commit `at` (asked once per commit).
fn hub_proto_at(repo: &Path, at: &str) -> Option<u32> {
    static KNOWN: Mutex<Option<(String, Option<u32>)>> = Mutex::new(None);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, p)) = known.as_ref().filter(|(k, _)| k == at) {
        return *p;
    }
    let read = || {
        let oid = crate::gitread::entries_at(repo, at, &[PROTO_FILE])?.pop().filter(|o| !o.is_empty())?;
        proto_in(&String::from_utf8(crate::gitread::blob(repo, &oid).ok()?).ok()?)
    };
    let p = read();
    *known = Some((at.to_string(), p));
    p
}

/// The hubs roll to `good` (the newest devhub build) when its hub protocol (`proto`) is one this
/// master (`master`) speaks; Err: why they wait for the master's own restart.
fn hub_target(good: &str, proto: Option<u32>, master: u32) -> Result<(), String> {
    match proto {
        Some(p) if p > master => Err(format!("master older than protocol {p}")),
        Some(_) => Ok(()),
        None => Err(format!("the hub protocol at {} cannot be read", super::short(good))),
    }
}

/// What a deployment push only waits for (a timed run on the box: `waits: timed lease L.. runs
/// here: the deployment moves after it`), from its answer lines (`<node> {"err":..}`).
fn push_waits(lines: &[String]) -> Option<String> {
    lines.iter().rev().find_map(|l| {
        let v = makepad_strict_json::parse(l[l.find('{')?..].as_bytes()).ok()?;
        v.get("err")?.as_str()?.strip_prefix("waits: ").map(str::to_string)
    })
}

/// More open fds than this: the master restarts itself (see [`note_fds`]).
const FDS_RESTART: usize = 8000;
const FDS_WARN: usize = 2000;
/// Set by a fd leak ([`note_fds`]): the master restarts itself at the next tick.
static RESTART_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn open_fds() -> usize {
    let dir = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
    std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
}

/// A leak that fills the fd table stops every git and child the master spawns (14.8k unix sockets
/// on 10-30, git failing with EBADF, cause not found): it says so with the count early, and
/// restarts itself past the limit (leases and jobs go on or are placed again).
fn note_fds(ctx: &Ctx, said: &mut usize) {
    let n = open_fds();
    if n >= FDS_WARN && n >= *said * 2 {
        *said = n;
        event(ctx, Sev::Warn, format!("master: {n} open fds; {}", if n >= FDS_RESTART { "it restarts itself" } else { "a leak? `lsof -p` its pid and see who the peers are" }));
    }
    if n >= FDS_RESTART {
        RESTART_ASKED.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// A failed master update tries again after this, doubling up to [`MASTER_RETRY_MAX`].
const MASTER_RETRY: Duration = Duration::from_secs(10 * 60);
const MASTER_RETRY_MAX: Duration = Duration::from_secs(2 * 60 * 60);

/// How long the master's update waits for an allapps run.
const MASTER_DEADLINE: Duration = Duration::from_secs(3 * 60);

/// A service's code still this long: its roll starts (one per burst of landings).
const SETTLE_MS: u64 = 3 * 60 * 1000;

/// How far back the scheduler knows the landings, for how far behind a box is.
const LANDINGS_KNOWN: Duration = Duration::from_secs(4 * 3600);

/// The landings since `since` (ms), oldest first: the deployment each left and when.
fn landed_since(ctx: &Ctx, since: u64) -> Vec<(String, u64)> {
    use crate::land::{Entry, St};
    let history = ctx.store.history("land").unwrap_or_default();
    let mut landed: Vec<(String, u64)> = history.iter().filter_map(|v| Entry::parse(&v.to_json())).filter(|e| e.state == St::Landed && e.changed_ms >= since)
        .filter_map(|e| { let pair = crate::bucket::entry_pair(&e.landed)?; Some((format!("{},{}", pair.work, pair.main), e.changed_ms)) }).collect();
    landed.sort_by_key(|(_, ms)| *ms);
    landed
}

/// A node's hub that does not answer during its roll is restarting (its new build starts in
/// seconds): warned about only past this.
const HUB_BACK: Duration = Duration::from_secs(120);

/// An update (`what`: the deployment, the hub restart) that a timed run made wait
/// ([`crate::lease::master::Master::update_waits`]), said no more when its ask ends.
struct UpdateWaits<'a> {
    ctx: &'a Ctx,
    node: &'a str,
    what: &'static str,
    on: bool,
}

impl UpdateWaits<'_> {
    fn set(&mut self, why: Option<String>) {
        if why.is_some() || self.on {
            self.on = why.is_some();
            self.ctx.leases.update_waits(self.node, self.what, why);
        }
    }
}

impl Drop for UpdateWaits<'_> {
    fn drop(&mut self) {
        self.set(None);
    }
}

/// One node: wait for the deployment code, then ask for its hub's restart (which ends no run). Some: the
/// deployment its new hub does not build at (the hub runs on; said once per commit).
fn roll_node(ctx: &Ctx, node: &str, svc: &str, dirs: &BTreeSet<String>, work: &str, drains: Arc<AtomicUsize>, restarts: &dyn Fn()) -> RollEnd {
    let short = super::short(work);
    let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-update");
    let caps = || ctx.leases.nodes().into_iter().find(|n| n.name == node).and_then(|n| n.caps);
    // Its hub has the target's code already (its WM's restart or a newer roll took it there): this
    // roll is done. Never a restart to where its hub was or behind it (a roll begun for an older
    // target kept a box "updating" to it while its hub ran a newer commit, 11-03).
    let has_it = || svc == "devhub" && caps().is_some_and(|c| !c.hub_rev.is_empty() && !lacks(&ctx.repo, &c.hub_rev, work, dirs));
    let done = |c: &crate::lease::Caps| event(ctx, Sev::Info, format!("update: {node} runs devhub at {} already (the roll was for {short})", super::short(&c.hub_rev)));
    // Deployment moves beside leases. Nodes outside autosync get a push here, without a restart:
    // to the tips, or to the pin of the timed ask first in line there (None: not pushed yet)
    let mut pushed: Option<Option<String>> = None;
    let mut kicked = false;
    // the pin holds the deployment until that ask starts (said once while it does)
    let mut held = false;
    let mut said = false;
    let t = Instant::now();
    let mut slot = Slot { drains, held: false };
    let updating = |stall: Option<&str>| if svc == "devhub" {
        ctx.leases.set_updating(node, Some(&format!("its hub updates to {short}{}", stall.map(|s| format!("; stalled: {s}")).unwrap_or_default())), false);
    };
    updating(None);
    // a push a timed run makes wait: an update wait (no new timed lease there until it moves), said
    // once, asked again at USER_WAIT (it was a warning every 5 s, 26 times for one run, 11-03)
    let mut push_wait = UpdateWaits { ctx, node, what: "deployment", on: false };
    loop {
        let c = caps().unwrap_or_default();
        if has_it() {
            done(&c);
            return RollEnd::NoRestart;
        }
        // Older hubs share the sync checkout with leases: wait for apply without draining them.
        let leased_checkout = svc == "devhub" && c.deploy.is_empty();
        let at = if leased_checkout { &c.tree } else { &c.deploy };
        let at = at.split(',').next().unwrap_or("");
        if !lacks(&ctx.repo, at, work, dirs) {
            break;
        }
        let to = deploy_to(ctx.leases.update_pin(node), &c.deploy, |d, a| crate::sync::deploy_has(&ctx.repo, d, a));
        if let DeployTo::Held { lease, at } = &to {
            if !held {
                push_wait.set(None);
                updating(Some(&format!("held at {} until {lease} (timed, first in line) starts", super::short(at))));
                held = true;
            }
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        if held {
            updating(None);
            held = false;
        }
        let pin = match to {
            DeployTo::Pin(at) => Some(at),
            _ => None,
        };
        if crate::sync::autosync_covers(&ctx.home, node) {
            // (the auto-sync follows the pin itself)
            if !kicked {
                crate::sync::kick_autosync(wall_ms());
                kicked = true;
            }
        } else if pushed.as_ref() != Some(&pin) {
            let (code, lines) = crate::sync::push_tree(&ctl, &ctx.repo, node, pin.as_deref());
            if code == 0 {
                pushed = Some(pin.clone());
            }
            match push_waits(&lines).filter(|_| code != 0) {
                Some(why) => {
                    if !push_wait.on {
                        updating(Some(&why));
                    }
                    push_wait.set(Some(format!("its deployment moves to {}", super::short(pin.as_deref().unwrap_or(work)))));
                    std::thread::sleep(USER_WAIT);
                    continue;
                }
                None if push_wait.on => {
                    push_wait.set(None);
                    updating(None);
                }
                None => {}
            }
            if code != 0 {
                event(ctx, Sev::Warn, format!("update: pushing {node}'s deployment to {short} failed: {}", lines.iter().rev().find(|l| l.contains("err")).cloned().unwrap_or_default()));
            }
        }
        if !said && t.elapsed() > DRAIN_SAY {
            event(ctx, Sev::Warn, format!("update: {node}'s deployment waits for {short} (at {})", if at.is_empty() { "?" } else { super::short(at) }));
            said = true;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    if svc == "devhub" {
        // A hub before handover (protocol 6) restarts only between runs: it drains, once.
        let old = caps().is_some_and(|c| c.hub_proto < crate::lease::HANDOVER_PROTO);
        if old { ctx.leases.set_updating(node, Some(&format!("its hub updates to {short}; an older hub: no new leases until it restarts")), true); }
        event(ctx, Sev::Info, format!("update: {node} restarts its hub for {short}{}", if old { " once its runs end (an older hub: no new leases until then)" } else { " now (runs and leases go on)" }));
    } else {
        slot.take();
    }
    let word = if svc == "devhub" { "hub-restart" } else { "service-restart" };
    let before = caps();
    // the hub restarts at once (it says `waits:` only for seconds; an older one until its runs end);
    // a user-facing service (the AI hub) never under a present user or unsaved work: the roll
    // waits until the box is idle (the node refuses with `waits:` and says what for)
    let mut said = false;
    let t = Instant::now();
    // since when the node's hub has not answered: it restarts (for this roll, a parallel one or
    // its WM's), which is planned and said as info; a warning only past HUB_BACK
    let mut down: Option<Instant> = None;
    let mut said_down = false;
    // a timed run there makes the hub restart wait (its new hub builds after it): the next timed
    // lease waits for the restart, which gets the gap after the run (said until this ask ends)
    let mut waits = UpdateWaits { ctx, node, what: "hub restart", on: false };
    let mut stall = String::new();
    loop {
        let present = svc != "devhub" && caps().and_then(|c| c.user).is_some_and(|u| u.present);
        // A newer apply may have moved the deployment since the master read its caps.
        let deploy = caps().map(|c| c.deploy).unwrap_or_default().split(',').next().unwrap_or("").to_string();
        // a hub that came back from a restart at the deployment already: never restarted twice
        if svc == "devhub" && down.is_some() && caps().is_some_and(|c| !deploy.is_empty() && c.hub_rev == deploy) {
            event(ctx, Sev::Info, format!("update: {node} runs devhub at {}", super::short(&deploy)));
            return RollEnd::NoRestart;
        }
        if has_it() {
            done(&caps().unwrap_or_default());
            return RollEnd::NoRestart;
        }
        let line = if svc == "devhub" { format!("hub-restart {deploy}") } else { format!("service-restart {svc}") };
        let answer = if present { Ok(vec![format!("{{\"err\":\"waits: a user is at {node}\"}}")]) } else { crate::cli::ask(&ctl, Some(node), word, &line) };
        if answer.is_ok() { down = None; }
        match answer {
            Err(e) if crate::lease::master::unreachable(&e) && down.get_or_insert_with(Instant::now).elapsed() < HUB_BACK => {
                if !said_down {
                    event(ctx, Sev::Info, format!("update: {node}'s hub is restarting ({e}); {svc} is asked again when it answers"));
                    said_down = true;
                }
                std::thread::sleep(Duration::from_secs(5));
            }
            Ok(l) if l.first().is_some_and(|x| x.starts_with("{\"err\":\"waits: ")) => {
                if svc == "devhub" && !waits.on && l[0].contains("waits: a timed lease runs") {
                    waits.set(Some(format!("its hub restarts at {}", super::short(&deploy))));
                }
                // what it waits for, on the box's line while it waits
                if let Some(why) = push_waits(&l).filter(|w| *w != stall) {
                    updating(Some(&why));
                    stall = why;
                }
                if t.elapsed() > SLOT_HOLD {
                    slot.give_back();
                }
                if !said && (svc != "devhub" || t.elapsed() > DRAIN_SAY) {
                    event(ctx, Sev::Info, format!("update: {node} {svc} waits: {}", l[0]));
                    said = true;
                }
                std::thread::sleep(if svc == "devhub" { Duration::from_secs(5) } else { USER_WAIT });
            }
            // a hub from before run-boundary restarts refuses while any lease is there: wait it out
            Ok(l) if svc == "devhub" && l.first().is_some_and(|x| x.contains("drain first")) => std::thread::sleep(Duration::from_secs(5)),
            Ok(l) if svc == "devhub" && l.first().is_some_and(|x| x.starts_with("{\"err\":\"the new hub does not build")) => {
                let why = makepad_strict_json::parse_depth(l[0].as_bytes(), 8).ok().and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string)).unwrap_or_else(|| l[0].clone());
                if BROKEN_SAID.lock().unwrap().insert(deploy.clone()) {
                    event(ctx, Sev::Err, format!("update: the hub does not build at {} ({node}: {why}); the hubs keep running their code until a landing that builds", super::short(&deploy)));
                } else {
                    event(ctx, Sev::Info, format!("update: {node}: {why}"));
                }
                restarts();
                return RollEnd::Broke(deploy);
            }
            Ok(l) if l.first().is_some_and(|x| x.starts_with("{\"err\":")) => {
                event(ctx, Sev::Warn, format!("update: {node} {svc}: {}", l[0]));
                return RollEnd::NoRestart;
            }
            Err(e) => {
                let gone = down.map(|d| format!(" (no answer for {} s)", d.elapsed().as_secs())).unwrap_or_default();
                event(ctx, Sev::Warn, format!("update: {node} {svc}: {e}{gone}"));
                return RollEnd::NoRestart;
            }
            // its service already runs the deployment's build (the WM's restart took it there,
            // or the landing changed nothing it is built of): never restarted for nothing
            Ok(l) if svc != "devhub" && l.first().is_some_and(|x| x.starts_with("{\"current\":")) => {
                event(ctx, Sev::Info, format!("update: {node} runs {svc} at {short} already: no restart"));
                return RollEnd::Current;
            }
            Ok(l) => {
                // the hub build it restarts for, measured there: its class's measure in the ledger
                if let Some(b) = l.first().and_then(|x| makepad_strict_json::parse(x.as_bytes()).ok()).and_then(|v| v.get("built").and_then(crate::lease::HubMeasure::parse)) {
                    ctx.leases.ledger_hub_build(node, &b);
                }
                restarts();
                event(ctx, Sev::Info, format!("update: {node} restarts {svc} for {short}"));
                break;
            }
        }
    }
    drop(waits);
    // back: the hub reports the new commit; a service a newer start that answers again (its slot
    // is held till then: a new start takes seconds, its build and load minutes, and a roll that
    // went on at the new start had every box's AI hub down at once)
    let answers = || crate::cli::ask(&ctl, Some(node), "service-health", &format!("service-health {svc}")).is_ok_and(|l| l.first().is_some_and(|x| x.contains("\"answers\":true")));
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(1500) {
        std::thread::sleep(Duration::from_secs(5));
        let Some(c) = caps() else { continue };
        let back = if svc == "devhub" {
            !c.hub_rev.is_empty() && Some(&c.hub_rev) != before.as_ref().map(|b| &b.hub_rev)
        } else {
            let old = before.as_ref().and_then(|b| b.services.iter().find(|(x, _)| x == svc).map(|s| s.1)).unwrap_or(0);
            c.services.iter().any(|(x, t)| x == svc && *t > old) && answers()
        };
        if back {
            let at = if svc == "devhub" { super::short(&c.hub_rev).to_string() } else { short.to_string() };
            event(ctx, Sev::Info, format!("update: {node} runs {svc} at {at}{}", if svc == "devhub" { "" } else { ", and it answers" }));
            return RollEnd::Back;
        }
    }
    event(ctx, Sev::Err, format!("update: {node} {svc} did not come back in 25 min (its log on the node)"));
    RollEnd::Late
}

/// How a box's roll ended. Only a restart that was asked for goes on the record: a service that
/// runs that build already is not one (the AI hub said so to an ask every 15 s, read as 24 rolls a minute).
enum RollEnd {
    /// restarted, and back
    Back,
    /// restarted, and not back
    Late,
    /// its new hub does not build at this commit
    Broke(String),
    /// the service runs the deployment's build already
    Current,
    /// nothing to do any more, or it would not: said in the update's events
    NoRestart,
}

/// The deployments a hub did not build at, said loudly once (each box refuses it on its own).
static BROKEN_SAID: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Rustup counts its nested proxies here and refuses past 20 ("infinite recursion detected"): a
/// master started through `cargo run` carries it into every build it runs.
const RUSTUP_DEPTH: &str = "RUST_RECURSION_COUNT";

/// One master restart at a time: the self-update's and `devhub master restart`'s.
pub(super) static RESTARTING: Mutex<()> = Mutex::new(());

/// The master's own lease holders an exec cannot carry over (the show's prewarms, the warm and
/// prebuild legs: their work is this process's threads), and whether a restart drains them. A
/// cut prewarm lost its build's last step and its lease expired (11-02). Draining: no new one
/// starts (it waits in [`holding`]; the exec ends that wait and the new master plans its own), a
/// queued one gives its request up ([`draining`]), a running one finishes. Waiting, never a cap.
static DRAIN: (Mutex<(bool, usize)>, std::sync::Condvar) = (Mutex::new((false, 0)), std::sync::Condvar::new());

fn drain_state() -> std::sync::MutexGuard<'static, (bool, usize)> {
    DRAIN.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// One of the master's own lease holders, counted until dropped.
pub struct Holding(());

impl Drop for Holding {
    fn drop(&mut self) {
        drain_state().1 -= 1;
        DRAIN.1.notify_all();
    }
}

/// Before one of the master's own holders asks for its lease: waits out a drain, then counts it.
pub fn holding() -> Holding {
    let mut d = drain_state();
    while d.0 {
        d = DRAIN.1.wait(d).unwrap_or_else(|e| e.into_inner());
    }
    d.1 += 1;
    Holding(())
}

/// The master drains for its restart: a holder still queued gives its request up.
pub fn draining() -> bool {
    drain_state().0
}

fn set_draining(on: bool) {
    drain_state().0 = on;
    DRAIN.1.notify_all();
}

/// Waits until no holder of the master's own runs; `say` hears how many there were, once.
fn drained(say: impl FnOnce(usize)) {
    let mut d = drain_state();
    if d.1 > 0 { say(d.1); }
    while d.1 > 0 {
        d = DRAIN.1.wait(d).unwrap_or_else(|e| e.into_inner());
    }
}

/// The master execs itself into the landed code ([`build_devhub`]), with the same args.
/// It builds first, and a build that fails keeps this master running and says so: an Err event
/// (the inbox) and the answer to whoever asked (`by`: `devhub master restart` waits for it).
/// Never a silent no-op (11-02: a hand restart after a failed self-update was dropped by its
/// retry pause, and clients met the old master's "unsupported service" for 15 min). Ok: the
/// exec follows in a moment, after the answer went out.
pub fn restart_master(ctx: &Arc<Ctx>, work: &str, by: &str) -> Result<String, String> {
    let _one = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
    let mine = super::short(crate::lease::caps::hub_rev()).to_string();
    // the running build is kept before the new one is swapped in: `devhub master rollback` execs back into it
    keep_running_binary(&ctx.repo)?;
    event(ctx, Sev::Info, format!("update: the master restarts on {} ({by}; it builds in {} first)", super::short(work), ctx.repo.display()));
    let args: Vec<String> = std::env::args().skip(1).collect();
    // its own lease holders drain while it builds: none starts, the running ones finish
    set_draining(true);
    // built first: a build that fails keeps this master running; a prewarm of the JIT would take
    // the cores all that time
    let t = Instant::now();
    let built_at = crate::lease::caps::head_sha(&ctx.repo);
    if let Err(why) = build_devhub(&ctx.repo) {
        set_draining(false);
        let line = format!("update: the master's build of {} failed ({why}); it keeps running {mine} ({by})", super::short(work));
        event(ctx, Sev::Err, line.clone());
        return Err(line);
    }
    let t_drain = Instant::now();
    drained(|n| event(ctx, Sev::Info, format!("update: the master's restart waits for {n} of its own leases (prewarms, prebuilds) to finish; no new ones start")));
    // the build just swapped in, exec'd as it is: a `cargo run` here would wait on the build lock
    // of every other cargo in `target/host` while no master beats
    let mut cmd = std::process::Command::new(ctx.repo.join("target/host").join(format!("makepad-devhub{}", std::env::consts::EXE_SUFFIX)));
    cmd.args(&args).current_dir(&ctx.repo);
    match &built_at {
        Some(rev) => cmd.env(crate::lease::caps::BUILT_AT, rev),
        None => cmd.env_remove(crate::lease::caps::BUILT_AT),
    };
    #[cfg(unix)]
    {
        // the dials of the exec's gap queue for the new build instead of being refused
        crate::node::keep_control_over_exec(&mut cmd);
        let c2 = ctx.clone();
        let _ = std::thread::Builder::new().name("devhub-master-exec".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _held = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
            let e = std::os::unix::process::CommandExt::exec(&mut cmd);
            set_draining(false);
            event(&c2, Sev::Err, format!("update: the master could not restart itself: {e}; it keeps running {}", super::short(crate::lease::caps::hub_rev())));
        });
        let waited = t_drain.elapsed().as_secs();
        Ok(format!("built {} in {:.0} s{}; the master execs into it now (the same pid: leases, jobs and live sessions go on)", super::short(work), t.elapsed().as_secs_f64() - waited as f64, if waited > 0 { format!(", its own leases drained in {waited} s") } else { String::new() }))
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, t, t_drain);
        set_draining(false);
        let line = "update: restart the master by hand (no exec on this OS)".to_string();
        event(ctx, Sev::Err, line.clone());
        Err(line)
    }
}

fn cargo_bin() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).map(|h| h.join(".cargo/bin/cargo")).filter(|c| c.exists()).unwrap_or_else(|| PathBuf::from("cargo"))
}

/// devhub's binary, built in a target dir of its own (never the lock of `target/host`, where the
/// apps and the JIT build) and swapped in at `target/host/makepad-devhub` through a rename: a
/// caller runs the old build or the new one, never a missing or half-written file. A background
/// build ([`crate::os::background`]: background QoS on the person's Mac) on 4 jobs, the master's
/// own restart too: the running master beats on while it builds, and only the exec's gap counts.
pub fn build_devhub(repo: &Path) -> Result<(), String> {
    let mut cmd = crate::os::background(cargo_bin().as_os_str());
    cmd.args(["build", "-j4", "--profile", "host", "-p", "makepad-devhub", "--bin", "makepad-devhub", "--target-dir"]).arg(repo.join("target/devhub")).current_dir(repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH).stdin(std::process::Stdio::null());
    match cmd.output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => return Err(format!("does not build: {}", build_failure(&o))),
        Err(e) => return Err(format!("cargo: {e}")),
    }
    let name = format!("makepad-devhub{}", std::env::consts::EXE_SUFFIX);
    swap_in(&repo.join("target/devhub/host").join(&name), &repo.join("target/host").join(&name))
}

/// Copies `built` beside `live` and renames it over: `live` is always a whole binary. One swap
/// at a time (the CLI's rebuild and the master's restart share the file beside it).
fn swap_in(built: &Path, live: &Path) -> Result<(), String> {
    static SWAP: Mutex<()> = Mutex::new(());
    let _one = SWAP.lock().unwrap_or_else(|e| e.into_inner());
    let next = live.with_extension("next");
    std::fs::create_dir_all(live.parent().unwrap_or(Path::new("."))).map_err(|e| format!("{}: {e}", live.display()))?;
    std::fs::copy(built, &next).and_then(|_| std::fs::rename(&next, live)).map(|_| ()).map_err(|e| format!("swapping {} in at {}: {e}", built.display(), live.display()))
}

/// The master build a restart replaced, kept for `devhub master rollback`.
fn prev_binary(repo: &Path) -> PathBuf {
    repo.join("target/host").join(format!("makepad-devhub{}.prev", std::env::consts::EXE_SUFFIX))
}

/// The build this master started from, copied as it starts: a CLI rebuild swaps a new file in at
/// the path it ran from, and a rollback needs the build that ran.
fn started_binary(repo: &Path) -> PathBuf {
    prev_binary(repo).with_extension("running")
}

/// At the master's start: its own binary, kept as [`started_binary`] (a master that runs the
/// rollback copy keeps none: [`prev_binary`] is that build already).
pub fn keep_started_binary(repo: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("update: the master's binary: {e}"))?;
    let started = started_binary(repo);
    if exe.canonicalize().ok() == prev_binary(repo).canonicalize().ok() {
        let _ = std::fs::remove_file(&started);
        return Ok(());
    }
    std::fs::copy(&exe, &started).map(|_| ()).map_err(|e| format!("update: keeping {} as {}: {e}", exe.display(), started.display()))
}

/// Before a restart: the build that runs becomes [`prev_binary`] (what it started from, or its
/// file now when it kept none; a master that runs that copy already, after a rollback, keeps it).
fn keep_running_binary(repo: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("update: the running master's binary: {e}"))?;
    let prev = prev_binary(repo);
    if exe.canonicalize().ok() == prev.canonicalize().ok() {
        return Ok(());
    }
    let started = started_binary(repo);
    let kept = std::fs::copy(if started.is_file() { &started } else { &exe }, &prev).map(|_| ());
    kept.map_err(|e| format!("update: keeping {} as {}: {e} (no restart without a build to roll back to)", exe.display(), prev.display()))
}

/// Execs the master back into the build its last restart replaced ([`prev_binary`]), the same
/// args and pid: a deploy rollback when a new build misbehaves (memory, CPU), not a code revert.
/// The next `devhub master restart` builds work's tip again.
pub fn rollback_master(ctx: &Arc<Ctx>, by: &str) -> Result<String, String> {
    let _one = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
    let prev = prev_binary(&ctx.repo);
    if !prev.is_file() {
        return Err(format!("update: no previous master build at {} to roll back to", prev.display()));
    }
    event(ctx, Sev::Warn, format!("update: the master rolls back from {} to its previous build ({by})", super::short(crate::lease::caps::hub_rev())));
    let args: Vec<String> = std::env::args().skip(1).collect();
    set_draining(true);
    drained(|n| event(ctx, Sev::Info, format!("update: the master's rollback waits for {n} of its own leases to finish")));
    let mut cmd = std::process::Command::new(&prev);
    cmd.args(&args).current_dir(&ctx.repo).env_remove(crate::lease::caps::BUILT_AT);
    #[cfg(unix)]
    {
        crate::node::keep_control_over_exec(&mut cmd);
        let c2 = ctx.clone();
        let _ = std::thread::Builder::new().name("devhub-master-exec".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _held = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
            let e = std::os::unix::process::CommandExt::exec(&mut cmd);
            set_draining(false);
            event(&c2, Sev::Err, format!("update: the master could not roll back: {e}; it keeps running {}", super::short(crate::lease::caps::hub_rev())));
        });
        Ok(format!("the master execs into its previous build {} now (the same pid)", prev.display()))
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
        set_draining(false);
        Err("update: roll the master back by hand (no exec on this OS)".to_string())
    }
}

/// Why a cargo build failed: its first error and where it is (`error[E0425]: ... at
/// libs/x.rs:12:5`), else its last stderr lines, and always its exit status (a build killed by a
/// signal may print nothing: 11-03 a node said only "no output").
pub fn build_failure(o: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&o.stderr);
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let why = match lines.iter().position(|l| l.starts_with("error")) {
        Some(i) => {
            let at = lines[i + 1..].iter().take(3).find_map(|l| l.strip_prefix("--> ")).map(|a| format!(" at {a}")).unwrap_or_default();
            format!("{}{at}", lines[i])
        }
        None if lines.is_empty() => "no output".to_string(),
        None => lines[lines.len().saturating_sub(3)..].join(" | "),
    };
    crate::proto::clean(&format!("{why} ({})", o.status), 400)
}

#[cfg(test)]
mod tests {
    /// A restart keeps the running build beside the new one; a master that runs that copy keeps it.
    #[test]
    fn the_previous_master_build_is_kept_for_a_rollback() {
        let d = std::env::temp_dir().join(format!("devhub-prev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("target/host")).unwrap();
        super::keep_running_binary(&d).unwrap();
        let prev = super::prev_binary(&d);
        assert_eq!(std::fs::read(&prev).unwrap(), std::fs::read(std::env::current_exe().unwrap()).unwrap());
        // the build it started from wins over whatever replaced its file since
        std::fs::write(super::started_binary(&d), b"the build that ran").unwrap();
        super::keep_running_binary(&d).unwrap();
        assert_eq!(std::fs::read(&prev).unwrap(), b"the build that ran");
        super::keep_started_binary(&d).unwrap();
        assert_eq!(std::fs::read(super::started_binary(&d)).unwrap(), std::fs::read(std::env::current_exe().unwrap()).unwrap());
        assert!(super::keep_running_binary(std::path::Path::new("/nonexistent-devhub-repo")).is_err(), "no restart without a build to roll back to");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The live binary is replaced whole: the old one or the new one, never a partial file.
    #[test]
    fn a_built_binary_is_swapped_in_whole() {
        let d = std::env::temp_dir().join(format!("devhub-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("own/host")).unwrap();
        let (built, live) = (d.join("own/host/bin"), d.join("host/bin"));
        std::fs::write(&built, b"new build").unwrap();
        std::fs::create_dir_all(d.join("host")).unwrap();
        std::fs::write(&live, b"old build").unwrap();
        let held = std::fs::File::open(&live).unwrap();
        super::swap_in(&built, &live).unwrap();
        assert_eq!(std::fs::read(&live).unwrap(), b"new build");
        // a caller that had the old file open keeps reading the old build
        let mut old = String::new();
        std::io::Read::read_to_string(&mut &held, &mut old).unwrap();
        assert_eq!(old, "old build");
        assert!(!live.with_extension("next").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A failed build says its first error and where, else its last lines, with its exit status.
    #[cfg(unix)]
    #[test]
    fn a_failed_build_says_its_first_error_and_how_it_ended() {
        use std::os::unix::process::ExitStatusExt;
        let out = |stderr: &str, raw: i32| std::process::Output { status: std::process::ExitStatus::from_raw(raw), stdout: Vec::new(), stderr: stderr.as_bytes().to_vec() };
        let err = "   Compiling makepad-devhub v0.1.0\nerror[E0425]: cannot find value `X` in module `super`\n   --> libs/devhub/src/lease/native.rs:122:40\n    |\nerror: could not compile `makepad-devhub`";
        assert_eq!(super::build_failure(&out(err, 101 << 8)), "error[E0425]: cannot find value `X` in module `super` at libs/devhub/src/lease/native.rs:122:40 (exit status: 101)");
        assert_eq!(super::build_failure(&out("   Compiling a\n   Compiling b\n   Compiling c\nlinker went away\n\n", 1 << 8)), "Compiling b | Compiling c | linker went away (exit status: 1)");
        let killed = super::build_failure(&out("", 9));
        assert!(killed.starts_with("no output (signal: 9"), "{killed}");
    }

    #[test]
    fn drains_stay_at_most_at_once_and_a_roll_gives_its_back() {
        let drains = std::sync::Arc::new(super::AtomicUsize::new(0));
        let slot = || super::Slot { drains: drains.clone(), held: false };
        // (one box at a time)
        let (mut a, mut b) = (slot(), slot());
        assert!(a.try_take() && a.try_take());
        assert!(!b.try_take());
        drop(a);
        assert!(b.try_take());
    }

    #[test]
    fn a_restart_drains_the_masters_own_holders() {
        use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
        let running = super::holding();
        super::set_draining(true);
        let started = std::sync::Arc::new(AtomicBool::new(false));
        let s2 = started.clone();
        // a new one waits out the drain
        let new = std::thread::spawn(move || {
            let _h = super::holding();
            s2.store(true, SeqCst);
        });
        // the running one finishes: drained, the new one not started
        let drain = std::thread::spawn(|| super::drained(|n| assert_eq!(n, 1)));
        drop(running);
        drain.join().unwrap();
        assert!(!started.load(SeqCst) && super::draining());
        // no exec (its build failed): it goes on
        super::set_draining(false);
        new.join().unwrap();
        assert!(started.load(SeqCst));
    }

    #[test]
    fn the_master_updates_within_minutes_of_a_landing_whatever_comes_after_it() {
        use std::time::Duration;
        let old = super::MASTER_MIN_UPTIME + Duration::from_secs(1);
        let min = Duration::from_secs(60);
        assert!(super::restart_blocker_of(4 * min, true, min, false).is_some_and(|w| w.starts_with("up 4 min of 10")));
        assert_eq!(super::restart_blocker_of(old, true, Duration::ZERO, false), None, "settled: at once");
        // landings every minute: the code never settles, the update still comes
        let waits: Vec<bool> = (0..8).map(|m| super::restart_blocker_of(old, false, m * min, false).is_some()).collect();
        assert_eq!(waits, [true, true, true, true, true, false, false, false]);
        assert!(super::restart_blocker_of(old, true, Duration::ZERO, true).is_some_and(|w| w.contains("sweep")));
    }

    /// The hubs follow the newest devhub build, not the master's restart (Mac hubs lagged the
    /// deployment for the master's 30 min uptime, 11-03); one that speaks a newer protocol than the
    /// master waits for it, stated.
    #[test]
    fn hubs_roll_to_the_newest_build_unless_the_master_is_older_than_its_protocol() {
        let mine = crate::lease::HUB_PROTO;
        assert_eq!(super::proto_in(include_str!("../lease/mod.rs")), Some(mine), "the protocol is read from its source");
        assert_eq!(super::hub_target("abc", Some(mine), mine), Ok(()));
        assert_eq!(super::hub_target("abc", Some(mine - 1), mine), Ok(()), "a hub of an older protocol than the master's");
        assert_eq!(super::hub_target("abc", Some(mine + 1), mine), Err(format!("master older than protocol {}", mine + 1)));
        assert!(super::hub_target("abcdef1234567", None, mine).is_err_and(|e| e.contains("cannot be read")));
        // a landing that leaves the master's code alone keeps the build's verdict; one after a
        // failed build does not
        let mut good = "a".to_string();
        super::carry_good(&mut good, "a", "b");
        assert_eq!(good, "b");
        super::carry_good(&mut good, "c", "d");
        assert_eq!(good, "b", "c never built: d is not known good");
    }

    /// A push a timed run makes wait is an update wait, not a failure.
    #[test]
    fn a_push_refused_for_a_timed_run_only_waits() {
        let refused = vec!["node-a {\"err\":\"waits: timed lease L287466 runs here: the deployment moves after it\"}".to_string()];
        assert_eq!(super::push_waits(&refused).as_deref(), Some("timed lease L287466 runs here: the deployment moves after it"));
        assert_eq!(super::push_waits(&["node-a {\"err\":\"push: connection reset\"}".to_string()]), None);
        assert_eq!(super::push_waits(&[r#"{"err":"waits: a timed lease runs on node-a: its new hub builds after it"}"#.to_string()]).as_deref(), Some("a timed lease runs on node-a: its new hub builds after it"));
    }

    #[test]
    fn a_service_rolls_for_what_cargo_builds_of_it() {
        let listing = "makepad-devhub v0.1.0 (/repo/libs/devhub)\nmakepad-remote v0.1.0 (/repo/libs/remote) (*)\nmacro-thing v0.1.0 (proc-macro) (/repo/libs/macro/impl)\nlibc v0.2.1\nother v0.1.0 (/elsewhere/other)\n";
        let dirs: Vec<String> = super::tree_dirs(listing, "/repo").into_iter().collect();
        assert_eq!(dirs, ["libs/devhub", "libs/macro/impl", "libs/remote"]);
        for file in ["src/lib.rs", "src/tests/a.rs", "Cargo.toml", "build.rs", ""] {
            assert!(super::builds(file), "{file}");
        }
        for file in ["ci.toml", "README.md", "src/notes.md", "tests/a.rs", "benches/b.rs", "examples/c/main.rs"] {
            assert!(!super::builds(file), "{file}");
        }
    }

    /// A landing cargo cannot read rolls, for whatever changed, and says why.
    #[test]
    fn a_landing_cargo_cannot_read_rolls_for_any_file() {
        let d = std::env::temp_dir().join(format!("devhub-roll-unread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("libs/a/tests")).unwrap();
        let g = |args: &[&str]| {
            let o = std::process::Command::new("git").arg("-C").arg(&d).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}");
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let save = |from: &str| {
            for f in ["top.rs", "libs/a/x.rs", "libs/a/ci.toml", "libs/a/tests/t.rs"] {
                std::fs::write(d.join(f), from).unwrap();
            }
            g(&["add", "."]);
            g(&["commit", "-q", "-m", from]);
            g(&["rev-parse", "HEAD"])
        };
        g(&["init", "-q", "-b", "work"]);
        let (one, two) = (save("1"), save("2"));
        let (dirs, why) = super::service_dirs(&d);
        assert!(why.as_deref().is_some_and(|w| w.contains("the whole repo counts")), "{why:?}");
        for (svc, _) in super::SERVICES {
            let mut changed = super::built_changes(&d, &one, &two, &dirs[svc]).unwrap();
            changed.sort();
            assert_eq!(changed, ["libs/a/tests/t.rs", "libs/a/x.rs", "top.rs"], "{svc}: the whole repo, less what no crate reads");
            assert!(super::touches(&d, &one, &two, &dirs[svc]) && !super::touches(&d, &two, &two, &dirs[svc]));
            assert!(super::last_change_ms(&d, &two, &dirs[svc]) > 0, "{svc}");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The rule's point: what a hub builds of devhub is far from its panel's widgets.
    #[test]
    fn devhub_rolls_for_its_own_graph_and_not_its_panels() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let d = super::linked_dirs(&repo, "libs/devhub").unwrap();
        assert!(d.contains("libs/devhub") && d.contains("libs/remote") && d.contains("libs/strict_json"), "{d:?}");
        for not_built in ["apps/devhub", "widgets", "draw", "platform"] {
            assert!(!d.contains(not_built), "{not_built}: {d:?}");
        }
    }

    #[test]
    fn a_roll_can_be_read_back_with_its_target_and_why() {
        assert_eq!(super::rolls_kept("short\n"), None);
        let long = format!("{}\nlast\n", "x".repeat(super::ROLLS_KEEP));
        assert_eq!(super::rolls_kept(&long), Some("last\n"));
        assert_eq!(super::why_of(&["a".into()]), "1 file: a");
        assert_eq!(super::why_of(&["a".into(), "b".into(), "c".into(), "d".into(), "e".into()]), "5 files: a, b, c +2");
        let text = "{\"ms\":1,\"node\":\"n1\",\"svc\":\"devhub\",\"from\":\"aaaaaaaaaaaaaa\",\"to\":\"bbbbbbbbbbbbbb\",\"why\":\"1 file: a\"}\n{\"ms\":2,\"node\":\"n2\",\"svc\":\"devhub\",\"from\":\"a\",\"to\":\"b\",\"why\":\"w\"}\n{\"ms\":3,\"node\":\"n1\",\"svc\":\"devhub\",\"to\":\"bbbbbbbbbbbbbb\",\"ended\":\"runs bbbbbbbbbb\"}\nnot a whole li";
        let clock = |ms: u64| format!("t{ms}");
        assert_eq!(super::rolls_text(text, Some("n1"), 10, &clock), "t1 n1 devhub: aaaaaaaaaa -> bbbbbbbbbb: 1 file: a\nt3 n1 devhub: bbbbbbbbbb ended: runs bbbbbbbbbb");
        assert_eq!(super::rolls_text(text, None, 1, &clock), "t3 n1 devhub: bbbbbbbbbb ended: runs bbbbbbbbbb");
    }

    #[test]
    fn closures_follow_path_and_workspace_deps() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let d = super::crate_dirs(&repo, "apps/devhub");
        assert!(d.contains("apps/devhub") && d.contains("libs/devhub") && d.contains("libs/remote") && d.contains("libs/strict_json"), "{d:?}");
        assert!(!d.contains("apps/wm"));
        assert_eq!(super::normalize("apps/devhub/../../libs/devhub"), "libs/devhub");
        assert_eq!(super::path_of(r#"makepad-remote = { path = "../remote", optional = true }"#).as_deref(), Some("../remote"));
    }
}
