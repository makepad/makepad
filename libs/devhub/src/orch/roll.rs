//! The self-update: when a landing changes a service's code, devhub updates it on every box by
//! itself (at most [`AT_ONCE`] boxes drained at a time, so capacity stays), with no rollout script.
//! A service is affected when the landing touches a crate in its dependency closure (read from the
//! Cargo.toml path dependencies).
//! A roll starts once the service's code has been still for [`SETTLE_MS`]: landings come in bursts.
//! - the node hubs (apps/devhub): its tree gets the landed code (the auto-sync, kicked; a lease's
//!   pin that has it counts) while the node works on (a box whose leases hold its tree back for
//!   [`TREE_DRAIN`] is drained for it: on a box in demand the next lease comes as one ends);
//!   then the node is drained (no new lease) for
//!   `hub-restart` at the first run boundary (no lease's run in flight: leases between runs stay
//!   and the new hub takes them back; no lease is ended): the hub exits and what keeps
//!   it (the WM's supervisor, the Mac's LaunchAgent) starts it again through `cargo run`. Its caps
//!   then name the commit it runs (`devhub nodes`: `hub <sha>`), and a hub that speaks an older
//!   protocol gets no leases until then. A hub from before the self-update needs one restart by
//!   hand (it is said once).
//! - the services the WM supervises (the AI hub): `service-restart` once the tree has the code
//!   (SIGTERM: it hands its jobs back; the WM starts it again through `cargo run`), never while
//!   a user is at the box (input within `user_idle_min`) or an app holds unsaved work: the roll
//!   waits until the box is idle. The hub itself restarts regardless (the user never sees it); a
//!   WM restart waits the same way on the node ([`crate::lease::exec::user_blockers`]).
//! - the master itself, never held up by the node rolls and ending nothing: what runs under a lease
//!   lives on the node, and the new master takes it back (jobs, live jobs, verify legs and CI
//!   shards go on from where their logs stand; queued ones stay queued). Only an allapps run is
//!   started over, so it is waited for up to [`MASTER_DEADLINE`]. It execs `cargo run --profile host
//!   -p makepad-run --bin makepad -- <its own args>` in the checkout (the node rolls start over
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

/// Boxes drained at once.
pub const AT_ONCE: usize = 2;
const TICK: Duration = Duration::from_secs(15);
/// How often a roll that waits for a box's user looks again.
const USER_WAIT: Duration = Duration::from_secs(30);
/// A drain waits this long for a run boundary before it says so (it keeps waiting: no lease is ended).
const DRAIN_SAY: Duration = Duration::from_secs(1800);
/// The node's tree may take this long to get the code once no lease holds it.
const TREE_FOR: Duration = Duration::from_secs(1200);
/// Leases hold a tree without the code this long: no new lease there, so it can move.
const TREE_DRAIN: Duration = Duration::from_secs(300);

/// One of the [`AT_ONCE`] drains, held until its roll ends.
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
    let root = std::fs::read_to_string(repo.join("Cargo.toml")).unwrap_or_default();
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
        let text = std::fs::read_to_string(repo.join(&dir).join("Cargo.toml")).unwrap_or_default();
        for l in text.lines() {
            let t = l.trim();
            let rel = if let Some(p) = path_of(t) {
                Some(normalize(&format!("{dir}/{p}")))
            } else if t.contains("workspace = true") || t.contains("workspace=true") {
                t.split_once('=').and_then(|(n, _)| ws.get(n.trim()).cloned())
            } else {
                None
            };
            if let Some(r) = rel.filter(|r| repo.join(r).join("Cargo.toml").exists()) {
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
    let _ = std::thread::Builder::new().name("devhub-update".into()).spawn(move || {
        let busy: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(BTreeSet::new()));
        let drains = Arc::new(AtomicUsize::new(0));
        // a self-update whose build failed is said loudly and tried again later, not every tick
        let (mut master_next, mut master_wait) = (None::<Instant>, MASTER_RETRY);
        let mut update_master = |work: &str| {
            if master_next.is_some_and(|at| Instant::now() < at) {
                return;
            }
            restart_master(&ctx, work);
            // still here: it keeps running
            event(&ctx, Sev::Err, format!("update: the master could not update itself to {}: it runs old code and tries again in {} min", super::short(work), master_wait.as_secs() / 60));
            master_next = Some(Instant::now() + master_wait);
            master_wait = (master_wait * 2).min(MASTER_RETRY_MAX);
        };
        let mut said: BTreeSet<String> = BTreeSet::new();
        let mut closures: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut closures_at = String::new();
        let mut wait_since: Option<Instant> = None;
        let mut fds_said = 1000;
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
                closures_at = work.clone();
            }
            let settled = last_change_ms(&ctx.repo, &work, &closures["devhub"]) + SETTLE_MS <= wall_ms();
            for n in ctx.leases.nodes().into_iter().filter(|n| settled && n.up && !matches!(n.role(), crate::lease::Role::Device)) {
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
                } else if touches(&ctx.repo, &c.hub_rev, &work, hub) {
                    Some(("devhub", hub.clone()))
                } else {
                    // the WM-supervised services that run there and started before their code's last change
                    SERVICES.iter().skip(1).find_map(|(s, _)| {
                        let started = c.services.iter().find(|(x, _)| x == s)?.1;
                        let dirs = &closures[*s];
                        (started < last_change_ms(&ctx.repo, &work, dirs)).then(|| (*s, dirs.clone()))
                    })
                };
                let Some((svc, dirs)) = job else { continue };
                busy.lock().unwrap().insert(n.name.clone());
                let (c2, b2, d2, node, work2) = (ctx.clone(), busy.clone(), drains.clone(), n.name.clone(), work.clone());
                let _ = std::thread::Builder::new().name("devhub-update-node".into()).spawn(move || {
                    roll_node(&c2, &node, svc, &dirs, &work2, d2);
                    c2.leases.set_updating(&node, None);
                    b2.lock().unwrap().remove(&node);
                });
            }
            // the master itself, once the landings settled: the restart ends nothing, so nothing
            // holds it up but an allapps run (for a while). Never held up by node rolls either: one
            // waits for a long lease's run boundary, landings keep coming, and a master that waited
            // for them all never updated (18:18 → 19:05, 2026-10-30). A roll it cuts short starts
            // over from the new master.
            {
                let dirs = &closures["devhub"];
                let mine = crate::lease::caps::hub_rev();
                // (none under a sweep: its state is this process's)
                if !mine.is_empty() && !super::lane::sweeping() && settled && touches(&ctx.repo, mine, &work, dirs) {
                    let since = *wait_since.get_or_insert_with(Instant::now);
                    if ctx.store.read(|d| d.allapps.running.is_none()) || since.elapsed() >= MASTER_DEADLINE {
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

/// More open fds than this: the master restarts itself (see [`note_fds`]).
const FDS_RESTART: usize = 8000;
const FDS_WARN: usize = 2000;
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

/// One node: drain, the tree at the code, restart the service at a run boundary, wait for it.
fn roll_node(ctx: &Ctx, node: &str, svc: &str, dirs: &BTreeSet<String>, work: &str, drains: Arc<AtomicUsize>) {
    let short = super::short(work);
    let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-update");
    let caps = || ctx.leases.nodes().into_iter().find(|n| n.name == node).and_then(|n| n.caps);
    let leased = || ctx.leases.view().iter().any(|l| l.node.as_deref() == Some(node) && !matches!(l.state, crate::lease::sched::State::Ended(_)));
    // the auto-sync leaves a node with a user alone: its tree moves after they are idle
    let user = || caps().and_then(|c| c.user).is_some_and(|u| u.present);
    // the tree has the code: the auto-sync moves it to the landed commits; a node it does not
    // follow (Windows, the Mac boxes) gets them pushed here (no build, no restart). A lease holds
    // the tree at its pin: one that has the code counts, else the tree moves once it ends.
    let mut moved: Option<Instant> = None;
    let t = Instant::now();
    let mut said = false;
    let mut slot = Slot { drains, held: false };
    let mut drained = false;
    let mut drain = |slot: &mut Slot| {
        if !std::mem::replace(&mut drained, true) {
            slot.take();
            ctx.leases.set_updating(node, Some(&format!("its hub updates to {short}")));
            event(ctx, Sev::Info, format!("update: {node} drains for its hub update to {short} (no new lease; it restarts at the first run boundary)"));
        }
    };
    // false: the tree did not get the code in time (said)
    let mut tree_has_code = |slot: &mut Slot, drain: &mut dyn FnMut(&mut Slot)| -> bool {
        loop {
            let tree = caps().map(|c| c.tree).unwrap_or_default();
            let at = tree.split(',').next().unwrap_or("");
            if !lacks(&ctx.repo, at, work, dirs) {
                moved = None;
                return true;
            }
            if svc == "devhub" && (leased() || user()) {
                moved = None;
                if leased() && t.elapsed() > TREE_DRAIN {
                    drain(slot);
                }
                if !said && t.elapsed() > DRAIN_SAY {
                    event(ctx, Sev::Warn, format!("update: {node}'s tree stays at {}, without {short}, for {} min (a lease holds it or a user is at it); its hub restarts after", if at.is_empty() { "?" } else { super::short(at) }, DRAIN_SAY.as_secs() / 60));
                    said = true;
                }
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
            let since = *moved.get_or_insert_with(|| {
                if crate::sync::autosync_covers(&ctx.home, node) {
                    crate::sync::kick_autosync(wall_ms());
                } else {
                    let (code, lines) = crate::sync::push_tree(&ctl, &ctx.repo, node);
                    if code != 0 {
                        event(ctx, Sev::Warn, format!("update: pushing {node}'s tree to {short} failed: {}", lines.iter().rev().find(|l| l.contains("err")).cloned().unwrap_or_default()));
                    }
                }
                Instant::now()
            });
            if since.elapsed() > TREE_FOR {
                event(ctx, Sev::Warn, format!("update: {node}'s tree did not get {short} in {} min ({}); {svc} not restarted", TREE_FOR.as_secs() / 60, if at.is_empty() { "unknown" } else { super::short(at) }));
                return false;
            }
            std::thread::sleep(Duration::from_secs(5));
        }
    };
    if !tree_has_code(&mut slot, &mut drain) {
        return;
    }
    if svc == "devhub" {
        // only now no new lease (the tree's move and build above may take minutes; the node works
        // on meanwhile); the ones running stay: the restart waits for a run boundary
        drain(&mut slot);
    } else {
        slot.take();
    }
    let (word, line) = if svc == "devhub" { ("hub-restart", "hub-restart".to_string()) } else { ("service-restart", format!("service-restart {svc}")) };
    let before = caps();
    // the hub restarts at a run boundary (the node says `waits:` while a lease's run is in flight);
    // a user-facing service (the AI hub) never under a present user or unsaved work: the roll
    // waits until the box is idle (the node refuses with `waits:` and says what for)
    let mut said = false;
    let t = Instant::now();
    loop {
        // a lease still here may have taken the tree back to a pin without the code: never restart on that
        if svc == "devhub" && !tree_has_code(&mut slot, &mut drain) {
            return;
        }
        let present = svc != "devhub" && caps().and_then(|c| c.user).is_some_and(|u| u.present);
        let answer = if present { Ok(vec![format!("{{\"err\":\"waits: a user is at {node}\"}}")]) } else { crate::cli::ask(&ctl, Some(node), word, &line) };
        match answer {
            Ok(l) if l.first().is_some_and(|x| x.starts_with("{\"err\":\"waits: ")) => {
                if !said && (svc != "devhub" || t.elapsed() > DRAIN_SAY) {
                    event(ctx, Sev::Info, format!("update: {node} {svc} waits: {}", l[0]));
                    said = true;
                }
                std::thread::sleep(if svc == "devhub" { Duration::from_secs(5) } else { USER_WAIT });
            }
            // a hub from before run-boundary restarts refuses while any lease is there: wait it out
            Ok(l) if svc == "devhub" && l.first().is_some_and(|x| x.contains("drain first")) => std::thread::sleep(Duration::from_secs(5)),
            Ok(l) if l.first().is_some_and(|x| x.starts_with("{\"err\":")) => return event(ctx, Sev::Warn, format!("update: {node} {svc}: {}", l[0])),
            Err(e) => return event(ctx, Sev::Warn, format!("update: {node} {svc}: {e}")),
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
            return event(ctx, Sev::Info, format!("update: {node} runs {svc} at {at}"));
        }
    }
    event(ctx, Sev::Err, format!("update: {node} {svc} did not come back in 25 min (its log on the node)"));
}

/// Rustup counts its nested proxies here and refuses past 20 ("infinite recursion detected"): each
/// exec through `cargo run` would count one more, and the master's 20th restart would be its last.
const RUSTUP_DEPTH: &str = "RUST_RECURSION_COUNT";

/// The master execs itself through cargo run in the checkout: the landed code, the same args.
fn restart_master(ctx: &Ctx, work: &str) {
    event(ctx, Sev::Info, format!("update: the master restarts on {} (cargo run in {})", super::short(work), ctx.repo.display()));
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cargo = std::env::var_os("HOME").map(PathBuf::from).map(|h| h.join(".cargo/bin/cargo")).filter(|c| c.exists()).unwrap_or_else(|| PathBuf::from("cargo"));
    // built first: a build that fails keeps this master running (an exec into a failing cargo run would end it)
    let built = std::process::Command::new(&cargo).args(["build", "--profile", "host", "-p", "makepad-run", "--bin", "makepad"]).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH).stdin(std::process::Stdio::null()).output();
    match built {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let first = err.lines().find(|l| l.starts_with("error")).unwrap_or("").to_string();
            return event(ctx, Sev::Err, format!("update: the master's build of {} failed ({}); it keeps running", super::short(work), crate::proto::clean(&first, 200)));
        }
        Err(e) => return event(ctx, Sev::Err, format!("update: cargo: {e}")),
    }
    let mut cmd = std::process::Command::new(cargo);
    cmd.args(["run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--"]).args(&args).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").env_remove(RUSTUP_DEPTH);
    #[cfg(unix)]
    {
        let e = std::os::unix::process::CommandExt::exec(&mut cmd);
        event(ctx, Sev::Err, format!("update: the master could not restart itself: {e}"));
    }
    #[cfg(not(unix))]
    {
        let _ = cmd;
        event(ctx, Sev::Warn, "update: restart the master by hand (no exec on this OS)".into());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn drains_stay_at_most_at_once_and_a_roll_gives_its_back() {
        let drains = std::sync::Arc::new(super::AtomicUsize::new(0));
        let slot = || super::Slot { drains: drains.clone(), held: false };
        let (mut a, mut b, mut c) = (slot(), slot(), slot());
        assert!(a.try_take() && a.try_take() && b.try_take());
        assert!(!c.try_take());
        drop(a);
        assert!(c.try_take());
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
