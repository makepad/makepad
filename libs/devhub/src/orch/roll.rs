//! The self-update: when a landing changes a service's code, devhub updates it on every box by
//! itself, with no rollout script. Hub updates never stop lease placement.
//! A service is affected when the landing touches a crate in its dependency closure (read from the
//! Cargo.toml path dependencies).
//! A roll starts once the service's code has been still for [`SETTLE_MS`]: landings come in bursts.
//! - the node hubs (apps/devhub): their target is the master's own build (never a tip the master
//!   did not build, never older than a hub runs, the same for every box), so they follow the
//!   master's update. The deployment gets the code (the auto-sync, kicked) while leases keep using
//!   the work tree. Once `caps.deploy` has the code, the master asks for `hub-restart`; the hub
//!   builds its new self first (`waits:` meanwhile) and refuses a deployment it does not build at
//!   (said once per commit, loudly; the box is not asked again until its deployment moves), then
//!   restarts at once: a restart ends nothing (runs and live apps go on,
//!   their wrappers record exits, the new hub takes leases, runs and answered starts back from its
//!   ledger; holders see a reconnect). Only a sync apply or an in-hub selftest makes it wait
//!   seconds (Windows: its runs, job objects that end with the hub). The hub exits and what keeps
//!   it (the WM's supervisor, the Mac's LaunchAgent) starts it again through `cargo run`. Its caps
//!   then name the commit it runs (`devhub nodes`: `hub <sha>`). A hub before protocol 6
//!   ([`crate::lease::HANDOVER_PROTO`]) restarts only between runs: its box takes no new lease
//!   until it has (once per box). Only requests needing a newer protocol wait for it. A hub from
//!   before the self-update needs one restart by hand (it is said once).
//! - the services the WM supervises (the AI hub): `service-restart` once `caps.deploy` has the code
//!   (SIGTERM: it hands its jobs back; the WM starts it again through `cargo run`), never while
//!   a user is at the box (input within `user_idle_min`) or an app holds unsaved work: the roll
//!   waits until the box is idle. The hub itself restarts regardless (the user never sees it); a
//!   WM restart waits the same way on the node ([`crate::lease::exec::user_blockers`]).
//! - the master itself, never held up by the node rolls and ending nothing: what runs under a lease
//!   lives on the node, and the new master takes it back (jobs, live jobs, verify legs and CI
//!   shards go on from where their logs stand; queued ones stay queued). Only an allapps run is
//!   started over, so it is waited for up to [`MASTER_DEADLINE`]. It builds first: a build that
//!   fails keeps it running and is an inbox error (`devhub master restart` waits for the build and
//!   answers with it; it never waits out the self-update's retry pause). It execs `cargo run --profile host
//!   -p makepad-devhub --bin makepad-devhub -- <its own args>` in the checkout (only devhub builds: its
//!   own small binary, never the JIT) (the node rolls start over
//!   from the new master).
//! - the WM (with its window handover, never over unsaved work) and the dock apps (patched live,
//!   or a restart offered) are the auto-sync's apply (`restart=auto`) and live deploy.
//!
//! Every step is an `update` event.

use super::{git, Ctx};
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

/// The services the self-update restarts on nodes: (name, its package dir).
pub const SERVICES: [(&str, &str); 2] = [("devhub", "apps/devhub"), ("ai-hub", "apps/aihub")];

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
    if git(repo, &["merge-base", "--is-ancestor", to, at]).is_ok() {
        return false;
    }
    touches(repo, at, to, dirs)
}

/// Whether the commits from `from` to `to` change any of `dirs`.
fn touches(repo: &Path, from: &str, to: &str, dirs: &BTreeSet<String>) -> bool {
    if from == to {
        return false;
    }
    let mut args = vec!["diff", "--quiet", from, to, "--"];
    args.extend(dirs.iter().map(String::as_str));
    // `diff --quiet` exits 1 when there are differences (and on errors: an unknown commit counts as a change)
    git(repo, &args).is_err()
}

/// The newest commit of `to` that touched `dirs`, as wall ms (its commit time).
fn last_change_ms(repo: &Path, to: &str, dirs: &BTreeSet<String>) -> u64 {
    let mut args = vec!["log", "-1", "--format=%ct", to, "--"];
    args.extend(dirs.iter().map(String::as_str));
    git(repo, &args).ok().and_then(|s| s.trim().parse::<u64>().ok()).map_or(0, |s| s * 1000)
}

fn event(ctx: &Ctx, sev: Sev, line: String) {
    ctx.store.event("update", sev, line, None);
}

pub fn start(ctx: Arc<Ctx>) {
    let started = Instant::now();
    // the hubs' target: the commit this master was built at, read now (lazily it would be
    // whatever the checkout moved to since)
    let _ = crate::lease::caps::hub_rev();
    let _ = std::thread::Builder::new().name("devhub-update".into()).spawn(move || {
        let busy: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(BTreeSet::new()));
        // node -> the deployment its new hub did not build at: no roll again until it moves
        let broken: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
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
        let mut master_dirs: BTreeSet<String> = BTreeSet::new();
        let mut wait_since: Option<Instant> = None;
        let mut fds_said = 1000;
        // the commit the CLI binary was last built at (this master's build at first)
        let mut cli_at = crate::lease::caps::hub_rev().to_string();
        loop {
            std::thread::sleep(TICK);
            note_fds(&ctx, &mut fds_said);
            if RESTART_ASKED.swap(false, std::sync::atomic::Ordering::AcqRel) {
                if let Ok(w) = git(&ctx.repo, &["rev-parse", "refs/heads/work"]) {
                    update_master(&w);
                }
            }
            let Ok(work) = git(&ctx.repo, &["rev-parse", "refs/heads/work"]) else { continue };
            if closures_at != work {
                closures = SERVICES.iter().map(|(n, d)| (n.to_string(), crate_dirs(&ctx.repo, d))).collect();
                master_dirs = crate_dirs(&ctx.repo, "libs/devhub");
                closures_at = work.clone();
            }
            prebuild_cli(&ctx, &work, &master_dirs, &mut cli_at);
            let settled = last_change_ms(&ctx.repo, &work, &closures["devhub"]) + SETTLE_MS <= wall_ms();
            // each service rolls once its own code settled: the hub's frequent landings never hold
            // back an AI hub whose code is still (11-01: the fleet's AI hubs ran 1 h old code)
            let changed: HashMap<&str, u64> = SERVICES.iter().skip(1).map(|(s, _)| (*s, last_change_ms(&ctx.repo, &work, &closures[*s]))).collect();
            // the hubs' target is the master's own build: never a tip the master did not build
            // (11-02: hubs rolled onto a landing that broke makepad-devhub, which the master's
            // build had refused minutes before), never older than a hub runs, one for every box
            let mine = crate::lease::caps::hub_rev();
            for n in ctx.leases.nodes().into_iter().filter(|n| n.up && !matches!(n.role(), crate::lease::Role::Device)) {
                let Some(c) = n.caps.clone() else { continue };
                if busy.lock().unwrap().contains(&n.name) {
                    continue;
                }
                // the hub
                let hub = &closures["devhub"];
                let job = if c.hub_rev.is_empty() {
                    if said.insert(format!("old {}", n.name)) {
                        event(&ctx, Sev::Warn, format!("update: {}'s hub is from before the self-update: restart it once by hand (then devhub updates it by itself)", n.name));
                    }
                    None
                } else if !mine.is_empty()
                    && git(&ctx.repo, &["merge-base", "--is-ancestor", mine, &c.hub_rev]).is_err()
                    && touches(&ctx.repo, &c.hub_rev, mine, hub)
                    && broken.lock().unwrap().get(&n.name).map(String::as_str) != Some(c.deploy.split(',').next().unwrap_or(""))
                {
                    Some(("devhub", hub.clone()))
                } else {
                    // the WM-supervised services that run there and started before their code's last change
                    SERVICES.iter().skip(1).find_map(|(s, _)| {
                        let started = c.services.iter().find(|(x, _)| x == s)?.1;
                        let dirs = &closures[*s];
                        let at = changed[*s];
                        (started < at && at + SETTLE_MS <= wall_ms()).then(|| (*s, dirs.clone()))
                    })
                };
                let Some((svc, dirs)) = job else { continue };
                busy.lock().unwrap().insert(n.name.clone());
                let target = if svc == "devhub" { mine.to_string() } else { work.clone() };
                let (c2, b2, d2, node, br) = (ctx.clone(), busy.clone(), drains.clone(), n.name.clone(), broken.clone());
                let _ = std::thread::Builder::new().name("devhub-update-node".into()).spawn(move || {
                    match roll_node(&c2, &node, svc, &dirs, &target, d2) {
                        Some(at) => br.lock().unwrap().insert(node.clone(), at),
                        None => br.lock().unwrap().remove(&node),
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
                // (the master is libs/devhub's own binary: the hub app's UI crates are not its code)
                let dirs = &master_dirs;
                // (none under a sweep: its state is this process's)
                if !mine.is_empty() && !super::lane::sweeping() && settled && touches(&ctx.repo, mine, &work, dirs) {
                    let since = *wait_since.get_or_insert_with(Instant::now);
                    // A restart is cheap now (nothing is lost) but not free: one per
                    // MASTER_MIN_UPTIME, at a quiet boundary, never mid-grant or in a gate's last step.
                    let why = restart_blocker(&ctx, started.elapsed());
                    if let Some(why) = why {
                        if said.insert(format!("master {work} {}", if why.starts_with("up ") { "young" } else { why.as_str() })) {
                            event(&ctx, Sev::Info, format!("update: the master restarts on {} at a quiet moment (waits: {why})", super::short(&work)));
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

/// Lanes call devhub through `cargo run -p makepad-devhub`: right after a landing that changes
/// its inputs the master builds that binary once, niced on 4 jobs, so their runs find it fresh
/// (the first one rebuilt it at full -j and default nice while the rest waited on the lock: 5 at
/// once, the Mac at load 20, 11-02). Landings during a build get one more after it.
fn prebuild_cli(ctx: &Arc<Ctx>, work: &str, dirs: &BTreeSet<String>, at: &mut String) {
    if *at == work || CLI_BUILDING.load(Ordering::Acquire) {
        return;
    }
    if at.is_empty() || !touches(&ctx.repo, at, work, dirs) {
        *at = work.to_string();
        return;
    }
    *at = work.to_string();
    CLI_BUILDING.store(true, Ordering::Release);
    let (ctx, work) = (ctx.clone(), work.to_string());
    let spawned = std::thread::Builder::new().name("devhub-cli-build".into()).spawn(move || {
        let t = Instant::now();
        let cargo = std::env::var_os("HOME").map(PathBuf::from).map(|h| h.join(".cargo/bin/cargo")).filter(|c| c.exists()).unwrap_or_else(|| PathBuf::from("cargo"));
        let built = std::process::Command::new("nice").args(["-n", "10"]).arg(cargo).args(["build", "--profile", "host", "-j4", "-p", "makepad-devhub", "--bin", "makepad-devhub"]).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH).stdin(std::process::Stdio::null()).output();
        match built {
            Ok(o) if o.status.success() => event(&ctx, Sev::Info, format!("update: devhub's CLI is built at {} in {:.0} s", super::short(&work), t.elapsed().as_secs_f64())),
            Ok(o) => event(&ctx, Sev::Warn, format!("update: devhub's CLI does not build at {}: {}", super::short(&work), first_error(&String::from_utf8_lossy(&o.stderr)))),
            Err(e) => event(&ctx, Sev::Warn, format!("update: devhub's CLI build: {e}")),
        }
        CLI_BUILDING.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        CLI_BUILDING.store(false, Ordering::Release);
    }
}

/// The master restarts for a landing at most this often (`devhub master restart` asks at once).
pub const MASTER_MIN_UPTIME: Duration = Duration::from_secs(30 * 60);

/// Why the master must not exec into a new build now, if so: it is younger than
/// [`MASTER_MIN_UPTIME`], a lease is between states (a node sets it up or cleans up after it), or the
/// landing train has a gate running or a passed one about to land.
pub fn restart_blocker(ctx: &Ctx, uptime: Duration) -> Option<String> {
    let land = ctx.store.read(|d| d.land.iter().any(|e| matches!(e.get("state").and_then(|s| s.as_str()), Some("gating" | "passed"))));
    restart_blocker_of(uptime, ctx.leases.in_transition(), land)
}

fn restart_blocker_of(uptime: Duration, transition: Option<String>, land: bool) -> Option<String> {
    if uptime < MASTER_MIN_UPTIME {
        return Some(format!("up {} min of {}", uptime.as_secs() / 60, MASTER_MIN_UPTIME.as_secs() / 60));
    }
    if let Some(t) = transition {
        return Some(t);
    }
    land.then(|| "the landing train has a gate running".to_string())
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

/// One node: wait for the deployment code, then ask for a restart at a run boundary. Some: the
/// deployment its new hub does not build at (the hub runs on; said once per commit).
fn roll_node(ctx: &Ctx, node: &str, svc: &str, dirs: &BTreeSet<String>, work: &str, drains: Arc<AtomicUsize>) -> Option<String> {
    let short = super::short(work);
    let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-update");
    let caps = || ctx.leases.nodes().into_iter().find(|n| n.name == node).and_then(|n| n.caps);
    // Deployment moves beside leases. Nodes outside autosync get a push here, without a restart.
    let mut pushed = false;
    let mut said = false;
    let t = Instant::now();
    let mut slot = Slot { drains, held: false };
    if svc == "devhub" {
        ctx.leases.set_updating(node, Some(&format!("its hub updates to {short}")), false);
    }
    loop {
        let c = caps().unwrap_or_default();
        // Older hubs share the sync checkout with leases: wait for apply without draining them.
        let leased_checkout = svc == "devhub" && c.deploy.is_empty();
        let at = if leased_checkout { c.tree } else { c.deploy };
        let at = at.split(',').next().unwrap_or("");
        if !lacks(&ctx.repo, at, work, dirs) {
            break;
        }
        if crate::sync::autosync_covers(&ctx.home, node) {
            if !pushed {
                crate::sync::kick_autosync(wall_ms());
                pushed = true;
            }
        } else if !pushed {
            let (code, lines) = crate::sync::push_tree(&ctl, &ctx.repo, node);
            pushed = code == 0;
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
    loop {
        let present = svc != "devhub" && caps().and_then(|c| c.user).is_some_and(|u| u.present);
        // A newer apply may have moved the deployment since the master read its caps.
        let deploy = caps().map(|c| c.deploy).unwrap_or_default().split(',').next().unwrap_or("").to_string();
        let line = if svc == "devhub" { format!("hub-restart {deploy}") } else { format!("service-restart {svc}") };
        let answer = if present { Ok(vec![format!("{{\"err\":\"waits: a user is at {node}\"}}")]) } else { crate::cli::ask(&ctl, Some(node), word, &line) };
        match answer {
            Ok(l) if l.first().is_some_and(|x| x.starts_with("{\"err\":\"waits: ")) => {
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
                return Some(deploy);
            }
            Ok(l) if l.first().is_some_and(|x| x.starts_with("{\"err\":")) => {
                event(ctx, Sev::Warn, format!("update: {node} {svc}: {}", l[0]));
                return None;
            }
            Err(e) => {
                event(ctx, Sev::Warn, format!("update: {node} {svc}: {e}"));
                return None;
            }
            Ok(_) => {
                event(ctx, Sev::Info, format!("update: {node} restarts {svc} for {short}"));
                break;
            }
        }
    }
    // back: the hub reports the new commit; a service a newer start
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(1500) {
        std::thread::sleep(Duration::from_secs(5));
        let Some(c) = caps() else { continue };
        let back = if svc == "devhub" {
            !c.hub_rev.is_empty() && Some(&c.hub_rev) != before.as_ref().map(|b| &b.hub_rev)
        } else {
            let old = before.as_ref().and_then(|b| b.services.iter().find(|(x, _)| x == svc).map(|s| s.1)).unwrap_or(0);
            c.services.iter().any(|(x, t)| x == svc && *t > old)
        };
        if back {
            let at = if svc == "devhub" { super::short(&c.hub_rev).to_string() } else { short.to_string() };
            event(ctx, Sev::Info, format!("update: {node} runs {svc} at {at}"));
            return None;
        }
    }
    event(ctx, Sev::Err, format!("update: {node} {svc} did not come back in 25 min (its log on the node)"));
    None
}

/// The deployments a hub did not build at, said loudly once (each box refuses it on its own).
static BROKEN_SAID: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Rustup counts its nested proxies here and refuses past 20 ("infinite recursion detected"): each
/// exec through `cargo run` would count one more, and the master's 20th restart would be its last.
const RUSTUP_DEPTH: &str = "RUST_RECURSION_COUNT";

/// One master restart at a time: the self-update's and `devhub master restart`'s.
pub(super) static RESTARTING: Mutex<()> = Mutex::new(());

/// The master execs itself through cargo run in the checkout: the landed code, the same args.
/// It builds first, and a build that fails keeps this master running and says so: an Err event
/// (the inbox) and the answer to whoever asked (`by`: `devhub master restart` waits for it).
/// Never a silent no-op (11-02: a hand restart after a failed self-update was dropped by its
/// retry pause, and clients met the old master's "unsupported service" for 15 min). Ok: the
/// exec follows in a moment, after the answer went out.
pub fn restart_master(ctx: &Arc<Ctx>, work: &str, by: &str) -> Result<String, String> {
    let _one = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
    let mine = super::short(crate::lease::caps::hub_rev()).to_string();
    event(ctx, Sev::Info, format!("update: the master restarts on {} ({by}; it builds in {} first)", super::short(work), ctx.repo.display()));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cargo = std::env::var_os("HOME").map(PathBuf::from).map(|h| h.join(".cargo/bin/cargo")).filter(|c| c.exists()).unwrap_or_else(|| PathBuf::from("cargo"));
    // built first: a build that fails keeps this master running (an exec into a failing cargo run would end it);
    // a prewarm of the JIT would hold the cargo lock all that time
    super::show::stop_mac_prewarm();
    let t = Instant::now();
    let built = std::process::Command::new(&cargo).args(["build", "--profile", "host", "-p", "makepad-devhub", "--bin", "makepad-devhub"]).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH).stdin(std::process::Stdio::null()).output();
    let failed = match built {
        Ok(o) if o.status.success() => None,
        Ok(o) => Some(first_error(&String::from_utf8_lossy(&o.stderr))),
        Err(e) => Some(format!("cargo: {e}")),
    };
    if let Some(why) = failed {
        let line = format!("update: the master's build of {} failed ({why}); it keeps running {mine} ({by})", super::short(work));
        event(ctx, Sev::Err, line.clone());
        return Err(line);
    }
    let mut cmd = std::process::Command::new(cargo);
    cmd.args(["run", "--profile", "host", "-p", "makepad-devhub", "--bin", "makepad-devhub", "--"]).args(&args).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH);
    #[cfg(unix)]
    {
        let c2 = ctx.clone();
        let _ = std::thread::Builder::new().name("devhub-master-exec".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _held = RESTARTING.lock().unwrap_or_else(|e| e.into_inner());
            let e = std::os::unix::process::CommandExt::exec(&mut cmd);
            event(&c2, Sev::Err, format!("update: the master could not restart itself: {e}; it keeps running {}", super::short(crate::lease::caps::hub_rev())));
        });
        Ok(format!("built {} in {:.0} s; the master execs into it now (the same pid: leases, jobs and live sessions go on)", super::short(work), t.elapsed().as_secs_f64()))
    }
    #[cfg(not(unix))]
    {
        let _ = (cmd, t);
        let line = "update: restart the master by hand (no exec on this OS)".to_string();
        event(ctx, Sev::Err, line.clone());
        Err(line)
    }
}

/// A failed cargo build's first error and where it is (`error[E0425]: ... at libs/x.rs:12:5`).
pub fn first_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().collect();
    let Some(i) = lines.iter().position(|l| l.starts_with("error")) else {
        return crate::proto::clean(lines.iter().rev().find(|l| !l.trim().is_empty()).copied().unwrap_or("no output"), 300);
    };
    let at = lines[i + 1..].iter().take(3).find_map(|l| l.trim().strip_prefix("--> ")).map(|a| format!(" at {a}")).unwrap_or_default();
    crate::proto::clean(&format!("{}{at}", lines[i]), 300)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_build_says_its_first_error_and_where() {
        let err = "   Compiling makepad-devhub v0.1.0\nerror[E0425]: cannot find value `X` in module `super`\n   --> libs/devhub/src/lease/native.rs:122:40\n    |\nerror: could not compile `makepad-devhub`";
        assert_eq!(super::first_error(err), "error[E0425]: cannot find value `X` in module `super` at libs/devhub/src/lease/native.rs:122:40");
        assert_eq!(super::first_error("linker went away\n"), "linker went away");
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
    fn the_master_restarts_only_at_a_quiet_moment() {
        use std::time::Duration;
        let old = super::MASTER_MIN_UPTIME + Duration::from_secs(1);
        assert!(super::restart_blocker_of(Duration::from_secs(600), None, false).is_some_and(|w| w.contains("10 min")));
        assert!(super::restart_blocker_of(old, Some("L1 is being set up on a node".into()), false).is_some_and(|w| w.contains("set up")));
        assert!(super::restart_blocker_of(old, None, true).is_some_and(|w| w.contains("gate")));
        assert_eq!(super::restart_blocker_of(old, None, false), None);
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
