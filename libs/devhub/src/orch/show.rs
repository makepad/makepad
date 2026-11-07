//! `devhub show <app> [--on mac|<node>|quest] [--after landed <branch>]`: the latest landed build of
//! an app on one of the user's display devices, tracked like a job: one instance per app per
//! device, its pid, the landed commit it runs, a parked panic. Devices:
//! - `mac` (the default): the master's own launch, `cargo run --profile host -p makepad-run --bin
//!   makepad -- run <app>` in the checkout (work: the latest landed; the JIT patches landings live);
//! - a WM node: its dock app (the node's synced checkout is the latest landed);
//! - `quest`: the JIT host built for the Quest on the Mac (`cargo makepad android build-host
//!   --quest`), `adb install -r` and `am start` (never adb tcpip/usb/connect/disconnect/kill-server).
//!
//! A show on the Mac opens at once: the user is waiting, and testing is deferred. Alongside it,
//! devhub boots the app at the exact landed pair headless under the JIT on a box of the same OS (a
//! Mac node), keeps it up for the settle window ([`SETTLE`]) and requires no panic, pause, hang,
//! deadlock, patch or script error on any thread and no early exit. A failed pre-boot is an error
//! on the running show: a `show_failed` event (the inbox: the manager, and the lane that asked, by
//! name). A pair that passed is not booted again for later shows of that app. A landing builds
//! nothing on the Mac: a show builds the JIT first, then opens, and a running app offers the update
//! (reload_host). The devboxes have theirs from the prebuild ([`super::warm`]).
//!
//! A show is what the person asked to see: its launch says so (`MAKEPAD_SHOW=1`), and the app's
//! platform brings it to the front once it started and opens a tray app's panel, as a launch from
//! this background process opened behind the person's windows (11-06: "i dont see it"). A show of
//! an app that runs already brings it to the front again (its `front` command). It is never
//! replaced silently: it may hold unsaved work, so a show with other arguments refuses and says so
//! (ask the user, then `--replace`). When the user closes the app (its process ends
//! with exit 0) the show is done and keeps only how long it ran and which commit; a crash (a
//! nonzero exit, a signal) and a panic-pause are reported, not taken for a close.
//!
//! A Mac show takes only proven tips (11-06). A landing that reaches its app (a file in one of the
//! app's packages, as its JIT says) is held back in its JIT before the checkout moves
//! ([`before_landing`]: `inspect hold`, each file with the blob id it lands at). The JIT keeps a
//! file's change waiting while the file says just that; the person's own saves, of any file, apply
//! at once. The show takes the tip once the fast bucket looked at it (or at a pair the same for the
//! app) and no check, cargo or present unit of the app's graph is red there that was not red when it
//! took its pair ([`proven`]): then `hold off` lets the landing in live, and a change that needs a
//! restart stays offered to the person. Nothing here restarts or relaunches a show. Meanwhile its
//! row says `held at <pair>: tip <pair> <why>` (`devhub shows`). A pinned show takes no landing,
//! proven or not, until the unpin: its hold stays (`hold off` would let the run read the landed
//! text at once, its JIT's pin or not), and the unpin hands it to the judge again. A new show of an
//! app whose own unit is red at the tip waits for one that is not.
//! A show on the user's WM node runs its deployment, which takes only proven tips by the same rule
//! for the whole tree (sync/autosync.rs): its row has the pair the deployment holds and why
//! ([`follow_deploy`]).

use super::{git, Ctx};
use crate::bucket::TipState;
use crate::proto::{wall_ms, Record, Sev};
use crate::state::{Cond, JobPanic, Pair, Show};
use makepad_remote::json_str;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const TICK: Duration = Duration::from_secs(2);
/// How long an ended show stays in `devhub shows` (its outcome; `--all`: every one).
const RECENT_MS: u64 = 30 * 60 * 1000;
/// A show's pre-boot: someone waits for it. Ahead of lanes' new leases and landing gates' priority;
/// background work on a box it could take is preempted at once ([`super::preempt`]).
pub const SHOW_PRIO: i32 = -2;
/// How long a pre-boot keeps the app up (its startup work) at most before it counts as passed.
pub const SETTLE: Duration = Duration::from_secs(20);
/// It counts earlier once its JIT looked idle twice, but not before this (late worker panics).
const SETTLE_MIN: Duration = Duration::from_secs(8);
/// The longest a pre-boot waits for its JIT to come up (a cold box builds first).
const PREBOOT_UP: Duration = Duration::from_secs(900);

/// Pre-boots that passed: (app, what it was built from): a later landing that touched neither the
/// app's crates nor the JIT's reuses the pass.
static PREBOOTED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// Pre-boots in flight: (box, JIT pid) -> the failures its records said.
static PREBOOTING: Mutex<Option<HashMap<(String, u32), Vec<String>>>> = Mutex::new(None);

/// The Mac launches this master started: their child handles (the exit status of a close).
static CHILDREN: Mutex<Option<HashMap<String, std::process::Child>>> = Mutex::new(None);

/// A landing's hold ([`before_landing`]) and a tick's release ([`judge_held`]) one at a time: a
/// release never lets a landing in between its hold and its fast-forward.
static HOLD: Mutex<()> = Mutex::new(());
/// A person's pin or unpin ([`pin_show`]) and a tick's pin step ([`keep_pin`]) one at a time, the
/// step on the show's pin as it is then: it acted on the tick's snapshot from before an unpin,
/// pinned the JIT again, and the next landing read that as a pin (Fab, 11-07).
static PIN: Mutex<()> = Mutex::new(());
/// What each held show was judged at last (its pid, its pin, the tip, the buckets' ledger and
/// logs): it is judged again once one of them changed.
static JUDGED: Mutex<Option<HashMap<String, (u32, bool, Pair, u64, u64)>>> = Mutex::new(None);
/// How long a hold or release waits for a show's JIT to answer (its graph, its hold): the landing
/// train does not wait on a hung app. One that did not answer is not held, and its row says so.
const HOLD_ASK: Duration = Duration::from_secs(5);

fn event(ctx: &Ctx, sev: Sev, line: String, x: Option<&Show>) {
    ctx.store.event("show", sev, line, x.map(|x| crate::state::show_value(x).to_json()));
}

/// A failure in a shown app (a panic, a hang, a patch or compile error, a crash): a `show_failed`
/// event, so the manager hears it without the user (`devhub watch show --until fail`).
fn failed(ctx: &Ctx, x: &Show, what: String) {
    ctx.store.event("show_failed", Sev::Err, format!("show_failed {} at {} on {}: {what}", x.app, super::short(&x.sha), x.device), Some(crate::state::show_value(x).to_json()));
}

fn set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Show)) -> Option<Show> {
    try_set(ctx, id, f).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); None })
}

fn try_set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Show)) -> Result<Option<Show>, String> {
    ctx.store.update(|d| {
        let x = d.shows.iter_mut().find(|x| x.id == id)?;
        f(x);
        Some(x.clone())
    })
}

fn ctl() -> makepad_remote::link::Client {
    makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-show")
}

/// `mac`, `quest`, or a node the master knows.
pub fn device(ctx: &Ctx, on: &str) -> Result<String, String> {
    match on {
        "" | "mac" | "local" => Ok("mac".into()),
        d if d == ctx.machine => Ok("mac".into()),
        "quest" => Ok("quest".into()),
        n if ctx.leases.nodes().iter().any(|x| x.name == n) => Ok(n.to_string()),
        n => Err(format!("no device {n} (mac, quest, or a node: devhub nodes)")),
    }
}

/// The JIT's app name for a package (`makepad-app-foo` and `makepad-foo` are `foo`).
fn short_app(pkg: &str) -> &str {
    pkg.strip_prefix("makepad-app-").or_else(|| pkg.strip_prefix("makepad-")).unwrap_or(pkg)
}

fn same_app(jit_app: &str, pkg: &str) -> bool {
    jit_app == pkg || jit_app == short_app(pkg) || short_app(jit_app) == short_app(pkg)
}

pub fn start(ctx: Arc<Ctx>) {
    // shows a previous master ran on the Mac: their processes outlived it, untracked
    let lost: Vec<Show> = ctx.store.update(|d| {
        let mut out = Vec::new();
        for x in d.shows.iter_mut().filter(|x| x.device == "mac" && matches!(x.state.as_str(), "running" | "paused")) {
            if x.pid > 0 && crate::procinfo::exited(x.pid).is_none() {
                x.note = "the master restarted: still running, its close is seen by its pid".into();
            } else {
                x.state = "closed".into();
                x.ended_ms = wall_ms();
                out.push(x.clone());
            }
        }
        out
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    for x in lost {
        event(&ctx, Sev::Info, format!("show {} on mac ended while the master was down", x.app), Some(&x));
    }
    // a launch a previous master was in (its pre-boot, its Quest build): someone asked for it, it
    // starts again
    let again: Vec<Show> = ctx.store.update(|d| {
        d.shows.iter_mut().filter(|x| x.state == "starting").map(|x| {
            x.note = "the master restarted during its launch: started again".into();
            x.stages.clear();
            x.clone()
        }).collect()
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    for x in again {
        event(&ctx, Sev::Info, format!("show {} on {}: the master restarted during its launch; starting again", x.app, x.device), Some(&x));
        let c = ctx.clone();
        let _ = std::thread::Builder::new().name("devhub-show".into()).spawn(move || launch(&c, &x));
    }
    let _ = std::thread::Builder::new().name("devhub-shows".into()).spawn(move || {
        let mut work = crate::gitread::rev(&ctx.repo, "refs/heads/work").unwrap_or_default();
        loop {
            tick(&ctx, &mut work);
            ctx.store.wait(TICK);
        }
    });
}

fn tick(ctx: &Arc<Ctx>, work: &mut String) {
    let mut pass = crate::gitread::Pass::default();
    let shows: Vec<Show> = ctx.store.read(|d| d.shows.iter().filter(|x| x.active()).cloned().collect());
    reap(&shows);
    // a landing while a show runs: the JIT patches it live; said once per landing
    let now_work = pass.rev(&ctx.repo, "refs/heads/work").unwrap_or_default();
    let landed = !now_work.is_empty() && now_work != *work;
    if landed {
        *work = now_work.clone();
    }
    let mut ps: HashMap<String, Option<crate::explorer::MachineRow>> = HashMap::new();
    for x in shows {
        match x.state.as_str() {
            "waiting" => {
                // (no condition: a launch that waits for a tip its app's own units are not red at)
                let (met, cond) = if x.cond == Cond::None { (Some("its tip is no longer red".to_string()), Cond::None) } else { super::after::cond_met_in(ctx, &x.cond, x.added_ms, &mut pass) };
                if cond != x.cond {
                    set(ctx, &x.id, |s| s.cond = cond.clone());
                }
                if let Some(why) = met {
                    if x.device == "mac" {
                        if let Some(red) = launch_red(ctx, &x.app) {
                            if red != x.held {
                                let x = set(ctx, &x.id, |s| s.held = red.clone());
                                event(ctx, Sev::Info, format!("show {} on mac waits: {red}", x.as_ref().map_or("", |x| &x.app)), x.as_ref());
                            }
                            continue;
                        }
                    }
                    let Some(x) = set(ctx, &x.id, |s| {
                        s.state = "starting".into();
                        s.note = why.clone();
                        s.held.clear();
                    }) else { continue };
                    event(ctx, Sev::Info, format!("show {} on {}: {why}; starting", x.app, x.device), Some(&x));
                    let c = ctx.clone();
                    let _ = std::thread::Builder::new().name("devhub-show".into()).spawn(move || launch(&c, &x));
                }
            }
            "running" | "paused" => {
                let x = keep_pin(ctx, x, landed, &ask_jit);
                // the user's WM node takes only proven tips (sync/autosync.rs): its show runs what
                // its deployment has, and its row says what that holds for
                let wm = x.device != "mac" && user_wm(ctx, &x.device);
                let x = if wm { follow_deploy(ctx, x) } else { x };
                let x = if landed && x.device != "quest" && !wm && !x.pinned && x.held.is_empty() {
                    let main = pass.rev(&ctx.commercial(), "refs/heads/main").unwrap_or_default();
                    let x = set(ctx, &x.id, |s| (s.sha, s.main) = (now_work.clone(), main.clone())).unwrap_or(x);
                    event(ctx, Sev::Info, format!("show {} on {}: work moved to {}; the JIT patches it live (`devhub show {} --on {} --replace` restarts it)", x.app, x.device, super::short(&now_work), x.app, x.device), Some(&x));
                    x
                } else {
                    x
                };
                if x.device == "mac" && !x.held.is_empty() {
                    judge_held(ctx, &x);
                }
                if x.device == "mac" {
                    if let Some((n, first)) = mac_pending(&x) {
                        note_restart(ctx, &x, n, &first);
                    }
                    watch_log(ctx, &x);
                    mac_watch(ctx, &x);
                } else {
                    let row = ps.entry(x.device.clone()).or_insert_with(|| crate::cli::ask(&ctl(), Some(&x.device), "ps", "ps").ok().map(|l| crate::explorer::parse_ps(&x.device, &l)));
                    // (its hub's `ps` says what waits in its JIT)
                    if let Some(j) = row.as_ref().filter(|r| r.err.is_none() && x.pid != 0).and_then(|r| r.jits.iter().find(|j| j.pid == x.pid)) {
                        note_restart(ctx, &x, j.restart_pending, &j.restart_first);
                    }
                    remote_watch(ctx, &x, row.as_ref());
                }
            }
            _ => {}
        }
    }
}

/// How often a Mac show's JIT is asked what waits for a restart ([`mac_pending`]).
const RESTART_LOOK: Duration = Duration::from_secs(10);
/// When each Mac show's JIT was last asked it.
static RESTART_LOOKED: Mutex<Option<HashMap<String, std::time::Instant>>> = Mutex::new(None);

/// A Mac show's edits that wait for a restart, as its JIT says them (`session`): how many and the
/// oldest; asked each [`RESTART_LOOK`], None between and when it does not answer.
fn mac_pending(x: &Show) -> Option<(u32, String)> {
    if x.pid == 0 {
        return None;
    }
    {
        let mut looked = RESTART_LOOKED.lock().unwrap_or_else(|e| e.into_inner());
        let looked = looked.get_or_insert_with(HashMap::new);
        if looked.get(&x.id).is_some_and(|at| at.elapsed() < RESTART_LOOK) {
            return None;
        }
        looked.insert(x.id.clone(), std::time::Instant::now());
    }
    let said = ask_mac_within(x, "session", HOLD_ASK).ok()?;
    let v = said.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok().filter(|v| v.get("restart_pending").is_some()))?;
    let n = v.get("restart_pending")?.as_u64()? as u32;
    let first = v.get("pending").and_then(|p| p.as_arr()?.first()?.as_str().map(str::to_string)).unwrap_or_default();
    Some((n, first))
}

/// A show's row says what of its landings it does not run: `n` edits its JIT could not apply live
/// wait for the person's restart (never devhub's), `first` the oldest. The sha it is listed at
/// moved with the landings, so the row says since which one ([`restart_said`]); none waiting
/// clears it (the person restarted it).
fn note_restart(ctx: &Ctx, x: &Show, n: u32, first: &str) {
    let (line, since) = restart_now(x, n, first);
    if line != x.restart || since != x.restart_since {
        set(ctx, &x.id, |s| (s.restart, s.restart_since) = (line.clone(), since.clone()));
    }
}

/// A show's restart line and since when (`restart`, `restart_since`) with `n` edits waiting: the
/// sha it is listed at when the first is seen, kept while any waits.
fn restart_now(x: &Show, n: u32, first: &str) -> (String, String) {
    let since = if n == 0 { String::new() } else if x.restart_since.is_empty() { x.sha.clone() } else { x.restart_since.clone() };
    (restart_said(&since, n, first), since)
}

/// `restart pending since <sha>: N landed edit(s) not running (first: <why>)`; empty for none.
fn restart_said(since: &str, n: u32, first: &str) -> String {
    if n == 0 {
        return String::new();
    }
    let first = if first.is_empty() { String::new() } else { format!(" (first: {})", crate::explorer::pending_said(first)) };
    format!("restart pending since {}: {n} landed edit{} not running{first}", super::short(since), if n == 1 { "" } else { "s" })
}

/// Whether `node` is a user's WM (role `user-wm`), whose deployment takes only proven tips.
fn user_wm(ctx: &Ctx, node: &str) -> bool {
    ctx.leases.nodes().iter().any(|n| n.name == node && n.caps.as_ref().is_some_and(|c| c.role == Some(crate::lease::Role::UserWm)))
}

/// A show on the user's WM node runs its deployment: the pair it has (a pinned show stays as it is)
/// and why it holds there ([`crate::sync::deploy_held`]), each said once.
fn follow_deploy(ctx: &Ctx, x: Show) -> Show {
    let hold = crate::sync::deploy_held(&x.device).unwrap_or_default();
    let deploy = ctx.leases.nodes().into_iter().find(|n| n.name == x.device).and_then(|n| n.caps).map(|c| c.deploy).unwrap_or_default();
    let at = Pair::of_pin_key(&deploy).filter(|p| p.valid() && !x.pinned);
    let moved = at.as_ref().filter(|p| p.work != x.sha || p.main != x.main);
    if hold == x.held && moved.is_none() {
        return x;
    }
    let shown = set(ctx, &x.id, |s| {
        s.held = hold.clone();
        if let Some(p) = moved {
            (s.sha, s.main) = (p.work.clone(), p.main.clone());
        }
    });
    if hold != x.held && !hold.is_empty() {
        event(ctx, Sev::Info, format!("show {} on {} {hold}; it takes a tip once the fast bucket proves it", x.app, x.device), shown.as_ref());
    } else if let Some(p) = moved {
        event(ctx, Sev::Info, format!("show {} on {}: its deployment moved to {}; the JIT patches it live (a change that needs a restart stays offered)", x.app, x.device, super::short(&p.work)), shown.as_ref());
    }
    shown.unwrap_or(x)
}

/// The processes of shows no longer followed (stopped, replaced) are waited for once they exit:
/// none stays a zombie of the master.
fn reap(active: &[Show]) {
    let mut g = CHILDREN.lock().unwrap();
    let Some(m) = g.as_mut() else { return };
    m.retain(|id, child| active.iter().any(|x| x.id == *id) || matches!(child.try_wait(), Ok(None)));
}

/// A Mac show's process: its exit decides closed (0) or crashed.
fn mac_watch(ctx: &Ctx, x: &Show) {
    let mut g = CHILDREN.lock().unwrap();
    let status = match g.get_or_insert_with(HashMap::new).get_mut(&x.id) {
        Some(child) => match child.try_wait() {
            Ok(Some(st)) => Some(st.code()),
            Ok(None) => None,
            Err(_) => Some(None),
        },
        // a launch of a previous master: only its pid
        None => crate::procinfo::exited(x.pid),
    };
    let Some(code) = status else { return };
    if let Some(m) = g.as_mut() {
        m.remove(&x.id);
    }
    drop(g);
    ended(ctx, x, code);
}

/// Why an app's run ended, from its log's last close or quit line (the platform and its backend
/// name each one's source): whether the person did it (a key, a click, the Dock's Quit, a click
/// on a window's caption the window system gave), and the line. Scope's window closed twice under
/// the person's hands and devhub said "closed by the user" (11-06): an exit 0 is not the person's
/// close by itself. The run's own end names a quit signal and its sender. A line that follows its
/// cause defers to it, back to the first of the chain: an app's session closing its window
/// (`window N closes: session S`) is no cause of its own; the app's close request after a
/// caption's close button is that click's; the app's quit after a close or quit request is that
/// request's; the run's end at the app's exit and the app's last window closing only lead back to
/// what came before; the host closing the window it lent (session 0) is the run's end's, or the
/// run's `the run's window closes: <why>`. (A click on a Mac window's close button, the app's quit,
/// its exit, then the host's close: Stage, Synth, Amp and the fractal zoomer said "session 0 (the
/// host) closed it", not by the person's hand, 11-06.)
fn close_cause(log: &str) -> (bool, String) {
    const SOURCES: [&str; 9] = ["makepad-hal: window ", "makepad-hal: asked to quit", "the app closes it at", "the app asks to close it at", "[makepad-remote] remote closed window", "hal: the app quit", "makepad-run: the run ends: SIG", "its caption's close button: ", RUN_WINDOW];
    // (the run's own close of its window is a cause only of the host's close after it: after the
    // person's close it says nothing of who closed the app's window)
    const RUN_WINDOW: &str = "makepad-run: the run's window closes: ";
    // the links of a chain, never a cause: the run's end at the app's exit, the app's last window
    // closing (its exit follows)
    const LINKS: [&str; 2] = ["makepad-run: the run ends: the app exited", "hal: the last window closed"];
    let follows = |l: &str| l.contains(" closes: session ") && !l.contains("(the host)");
    let link = |l: &str| LINKS.iter().any(|s| l.contains(s));
    let request = |l: &str| l.contains(" asked to close (") || l.contains("makepad-hal: asked to quit");
    let lines: Vec<&str> = log.lines().rev().map(str::trim).filter(|l| !follows(l) && (link(l) || SOURCES.iter().any(|s| l.contains(s)))).skip_while(|l| l.contains(RUN_WINDOW)).collect();
    // each line that came of the one before it gives way to it
    let mut at = 0;
    while let (Some(l), Some(before)) = (lines.get(at), lines.get(at + 1)) {
        let defers = if link(l) {
            true
        } else if l.contains("the app asks to close it at") || l.contains("the app closes it at") {
            before.contains("its caption's close button: ")
        } else if l.contains(" closes: session 0 ") {
            before.contains(RUN_WINDOW) || link(before)
        } else if l.contains("hal: the app quit") {
            request(before)
        } else {
            false
        };
        if !defers {
            break;
        }
        at += 1;
    }
    let Some(line) = lines.get(at).copied().filter(|l| !link(l)) else {
        return (false, "its log names no close or quit".into());
    };
    let asked = line.contains("(windowShouldClose:)") || line.contains("(applicationShouldTerminate:)");
    let by_hand = line.contains(": key down") || line.contains(": mouse event") || line.contains("(Dock)");
    let caption = line.contains("its caption's close button: the window system's input");
    ((asked && by_hand) || caption, crate::proto::clean(line, 240))
}

/// The end of a show's log, what tells why it ended: the Mac's own file, a node's through its hub
/// (the WM's log of the client, [`node_log`]); a Quest show has none here.
fn end_of_log(x: &Show) -> String {
    match x.device.as_str() {
        "mac" => log_tail(&x.log),
        "quest" => String::new(),
        node => node_log(node, x.pid, 2000).map(|l| l.1.join("\n")).unwrap_or_default(),
    }
}

/// Where a show's log is (a node's on that node).
fn log_name(x: &Show) -> String {
    if x.device == "mac" { x.log.clone() } else { format!("{}:{}", x.device, x.log) }
}

/// The log of JIT `pid` on node `node` (`jitlog`: the WM's log of the client when the WM started
/// it, there after the run too) and its last `n` lines.
fn node_log(node: &str, pid: u32, n: usize) -> Result<(String, Vec<String>), String> {
    let mut path = None;
    let mut lines = Vec::new();
    for l in crate::cli::ask(&ctl(), Some(node), "jitlog", &format!("jitlog {pid} {n}"))? {
        let Ok(v) = makepad_strict_json::parse(l.as_bytes()) else { continue };
        if let Some(e) = v.get("err").and_then(|x| x.as_str()) {
            return Err(e.to_string());
        }
        if let Some(t) = v.get("line").and_then(|x| x.as_str()) {
            lines.push(t.to_string());
        } else if let Some(p) = v.get("log").and_then(|x| x.as_str()) {
            path = Some(p.to_string());
        }
    }
    Ok((path.ok_or_else(|| format!("{node} names no log of pid {pid}"))?, lines))
}

/// The last 256 KB of a show's log (its end is what tells why it ended).
fn log_tail(path: &str) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return String::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(256 * 1024)));
    let mut bytes = Vec::new();
    let _ = f.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The end of a show: exit 0 is a close (the show frees itself; the person's only when its log
/// says so, [`close_cause`]), anything else a crash.
fn ended(ctx: &Ctx, x: &Show, code: Option<i32>) {
    let closed = code == Some(0) && x.state != "paused";
    let ran = wall_ms().saturating_sub(x.started_ms) / 1000;
    let (by_person, cause) = if closed { close_cause(&end_of_log(x)) } else { (false, String::new()) };
    let x = set(ctx, &x.id, |s| {
        if s.active() {
            s.state = if closed { "closed" } else { "crashed" }.into();
        }
        s.exit = code;
        s.ended_ms = wall_ms();
        if closed {
            s.note = if by_person {
                format!("closed by the user after {ran} s at {} ({cause})", super::short(&s.sha))
            } else {
                format!("ended at exit 0 after {ran} s at {}, not by the person's hand: {cause}", super::short(&s.sha))
            };
            s.pid = 0;
        }
    })
    .unwrap_or_else(|| x.clone());
    if closed && by_person {
        event(ctx, Sev::Info, format!("show {} on {} ended: closed by the user after {ran} s (ran {}): {cause}", x.app, x.device, super::short(&x.sha)), Some(&x));
    } else if closed {
        event(ctx, Sev::Warn, format!("show {} on {} ENDED at exit 0 after {ran} s (ran {}), not by the person's hand: {cause} (log {})", x.app, x.device, super::short(&x.sha), log_name(&x)), Some(&x));
    } else if x.state == "crashed" {
        // its first error lines (a compile error stops the launch before any window)
        let errors: Vec<String> = crate::land::failure_lines(&end_of_log(&x)).into_iter().take(6).collect();
        // (no exit status: a signal, or a process no master waited for, one that ended while the
        // master was down or with the Mac)
        let exit = code.map_or("unknown: a signal, or it ended unwatched".into(), |c| c.to_string());
        event(ctx, Sev::Err, format!("show {} on {} CRASHED after {ran} s (exit {exit}; log {})", x.app, x.device, log_name(&x)), Some(&x));
        failed(ctx, &x, format!("crashed (exit {exit}){}", if errors.is_empty() { String::new() } else { format!(": {}", errors.join(" | ")) }));
    }
}

/// A node or Quest show: its JIT in the device's `ps` (gone = ended; the device's hub says no more).
fn remote_watch(ctx: &Ctx, x: &Show, row: Option<&crate::explorer::MachineRow>) {
    let Some(row) = row.filter(|r| r.err.is_none()) else { return };
    let jit = row.jits.iter().find(|j| (x.pid != 0 && j.pid == x.pid) || (x.pid == 0 && same_app(&j.app, &x.app)));
    match jit {
        Some(j) if x.pid == 0 => {
            // a node's WM started it: its log is the WM's of the client, on the node
            let log = if x.device == "quest" { None } else { node_log(&x.device, j.pid, 0).ok().map(|l| l.0) };
            set(ctx, &x.id, |s| {
                s.pid = j.pid;
                s.state = "running".into();
                if let Some(log) = &log {
                    s.log = log.clone();
                }
            });
        }
        Some(_) => {}
        // a dock app the WM has not started yet keeps its show starting/running without a pid
        None if x.pid == 0 => {}
        // its JIT is gone: the device's hub saw no crash event for it, so the user closed it
        None => ended(ctx, x, Some(0)),
    }
}

fn cargo() -> PathBuf {
    let c = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".cargo/bin/cargo");
    if c.exists() {
        c
    } else {
        PathBuf::from("cargo")
    }
}

fn log_file(ctx: &Ctx, x: &Show) -> Result<(PathBuf, std::fs::File), String> {
    let dir = ctx.logs("shows");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = format!("{}-{}", x.app, x.device);
    let path = dir.join(format!("{name}.log"));
    keep_previous(&dir, &name, &path);
    let f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path, f))
}

/// The previous runs' logs a show keeps beside its current one.
const KEPT_LOGS: usize = 8;

/// A new show of the same app on the same device keeps the previous run's log (what a run that
/// went wrong said is wanted after the person started it again): `<name>.<unix time it ended>.log`,
/// the newest [`KEPT_LOGS`] of them.
fn keep_previous(dir: &std::path::Path, name: &str, path: &std::path::Path) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    let ended = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    if let Err(e) = std::fs::rename(path, dir.join(format!("{name}.{ended}.log"))) {
        eprintln!("devhub show: keeping {}: {e}", path.display());
        return;
    }
    let prefix = format!("{name}.");
    let mut kept: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().into_string().ok()?;
            let stamp = file.strip_prefix(&prefix)?.strip_suffix(".log")?.parse::<u64>().ok()?;
            Some((stamp, e.path()))
        })
        .collect();
    kept.sort();
    let extra = kept.len().saturating_sub(KEPT_LOGS);
    for (_, old) in kept.into_iter().take(extra) {
        let _ = std::fs::remove_file(old);
    }
}

/// Starts the show on its device.
fn launch(ctx: &Arc<Ctx>, x: &Show) {
    set(ctx, &x.id, |s| {
        s.launch_ms = wall_ms();
        // a new log, and nothing said of it yet
        (s.log_read, s.log_said, s.events_said) = (0, Vec::new(), Vec::new());
    });
    let sha = crate::gitread::rev(&ctx.repo, "refs/heads/work").unwrap_or_default();
    let main = crate::gitread::rev(&ctx.commercial(), "refs/heads/main").unwrap_or_default();
    let r = match x.device.as_str() {
        "mac" => mac_launch(ctx, x),
        "quest" => quest_launch(ctx, x),
        node => crate::cli::ask(&ctl(), Some(node), "launch", &format!("show {}", x.app)).and_then(|l| match l.first() {
            Some(e) if e.starts_with("{\"err\":") => Err(e.clone()),
            _ => Ok((0, String::new())),
        }),
    };
    match r {
        Ok((pid, log)) => {
            // what is red already: one of those red at a later tip is no new break of the app
            let took_red: Vec<String> = crate::bucket::tip_state(&ctx.home, &Pair::new(&sha, &main)).red.into_keys().collect();
            let x = set(ctx, &x.id, |s| {
                s.state = "running".into();
                s.pid = pid;
                s.sha = sha.clone();
                s.main = main.clone();
                s.took_red = took_red;
                s.started_ms = wall_ms();
                s.log = log;
            });
            if x.as_ref().is_some_and(|x| x.device == "mac") {
                preboot_alongside(ctx, x.clone().unwrap(), sha.clone(), main.clone());
            }
            if let Some(x) = x {
                event(ctx, Sev::Info, format!("show {} on {}: running{} at {}", x.app, x.device, if pid > 0 { format!(" as pid {pid}") } else { String::new() }, super::short(&sha)), Some(&x));
            }
        }
        Err(e) => {
            let x = set(ctx, &x.id, |s| {
                s.state = "failed".into();
                s.ended_ms = wall_ms();
                s.note = crate::proto::clean(&e, 300);
            });
            event(ctx, Sev::Err, format!("show {} on {} failed to launch: {}", x.as_ref().map_or("", |x| &x.app), x.as_ref().map_or("", |x| &x.device), crate::proto::clean(&e, 200)), x.as_ref());
            if let Some(x) = &x {
                failed(ctx, x, format!("launch failed: {}", crate::proto::clean(&e, 200)));
            }
        }
    }
}

/// A hash of what `dirs` hold at the two commits (repo-relative dirs; `apps/commercial/..` in the
/// commercial repo), and of the workspace's Cargo inputs.
fn inputs_key(ctx: &Ctx, dirs: &std::collections::BTreeSet<String>, work: &str, main: &str) -> String {
    let mut text = String::new();
    // `git rev-parse <at>:<p>` of each, read in-process
    let mut mk_paths: Vec<&str> = vec!["Cargo.toml", "Cargo.lock", ".cargo/config.toml"];
    let mut com_paths: Vec<&str> = Vec::new();
    for d in dirs {
        match d.strip_prefix("apps/commercial/") {
            Some(c) => com_paths.push(c),
            None => mk_paths.push(d),
        }
    }
    let mk = crate::gitread::entries_at(&ctx.repo, work, &mk_paths).unwrap_or_default();
    let com = crate::gitread::entries_at(&ctx.commercial(), main, &com_paths).unwrap_or_default();
    let (mut mk, mut com) = (mk.into_iter(), com.into_iter());
    for _ in 0..3 {
        text.push_str(&mk.next().unwrap_or_default());
    }
    for d in dirs {
        text.push_str(d);
        text.push_str(&if d.starts_with("apps/commercial/") { com.next() } else { mk.next() }.unwrap_or_default());
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// The package's directory in the checkout, from the Cargo.toml that names it.
fn package_dir(ctx: &Ctx, app: &str, work: &str, main: &str) -> Option<String> {
    let want = format!("name = \"{app}\"");
    let find = |dir: &std::path::Path, at: &str| -> Option<String> {
        let out = git(dir, &["grep", "-l", "-F", &want, at, "--", "*Cargo.toml"]).ok()?;
        out.lines().next().and_then(|l| l.split_once(':')).map(|(_, p)| p.trim_end_matches("/Cargo.toml").to_string())
    };
    find(&ctx.repo, work).or_else(|| find(&ctx.commercial(), main).map(|p| format!("apps/commercial/{p}")))
}

/// Boots `x.app` at `work`,`main` headless under the JIT on a macOS devbox and keeps it up until
/// it settled: Ok when nothing failed on any thread and it did not exit early. The app's crates
/// and the JIT's unchanged since a passed boot: that pass counts. The box's dev JIT of the pin runs
/// (its store builds one per JIT id, shared with the landing's prebuild). Each stage's time goes to
/// `devhub shows`.
/// The pre-boot of a show already open on the Mac: its failure is an error on the running show.
fn preboot_alongside(ctx: &Arc<Ctx>, x: Show, work: String, main: String) {
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new().name("devhub-show-check".into()).spawn(move || {
        let Err(e) = preboot(&ctx, &x, &work, &main) else {
            set(&ctx, &x.id, |s| s.note = "pre-boot passed".into());
            return;
        };
        let now = set(&ctx, &x.id, |s| s.note = format!("pre-boot failed: {}", crate::proto::clean(&e, 280)));
        let by = if x.by.is_empty() { String::new() } else { format!("; asked by {}", x.by) };
        ctx.store.event(
            "show_failed",
            Sev::Err,
            format!("show_failed {} at {},{}: {e} (it is running on the Mac{by})", x.app, super::short(&work), super::short(&main)),
            now.as_ref().map(|x| crate::state::show_value(x).to_json()),
        );
    });
}

/// The box's dev JIT of the lease's pin, from its store: built once per (box, JIT id), so the
/// prebuild of the same landing and this share one build (the second waits for the first's, two
/// a landing on a Mac devbox, 11-02).
fn ensure_jit(held: &super::leg::Held, out: &mut std::fs::File) -> Result<(), String> {
    held.ship_nothing()?;
    match held.run(&JIT_VERSION.map(String::from), out) {
        Ok(0) => Ok(()),
        Ok(c) => Err(format!("the dev JIT on {} failed (exit {c})", held.node)),
        Err(e) => Err(format!("the dev JIT on {}: {e}", held.node)),
    }
}

/// A run of the box's dev JIT (the node selects it for the lease's pin) that only says its version.
const JIT_VERSION: [&str; 10] = ["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "--version"];

fn preboot(ctx: &Ctx, x: &Show, work: &str, main: &str) -> Result<(), String> {
    let began = std::time::Instant::now();
    let stage = |line: String| {
        set(ctx, &x.id, |s| s.stages = if s.stages.is_empty() { line.clone() } else { format!("{}, {line}", s.stages) });
    };
    let jit_dirs = super::roll::crate_dirs(&ctx.repo, "tools/run");
    let jit_key = inputs_key(ctx, &jit_dirs, work, main);
    let app_dirs = package_dir(ctx, &x.app, work, main).map(|d| super::roll::crate_dirs(&ctx.repo, &d));
    let key = app_dirs.as_ref().map(|d| (x.app.clone(), format!("{jit_key} {}", inputs_key(ctx, d, work, main))));
    if key.as_ref().is_some_and(|k| PREBOOTED.lock().unwrap().contains(k)) {
        stage("pre-boot reused (the app's and the JIT's sources are unchanged since a pass)".into());
        return Ok(());
    }
    try_set(ctx, &x.id, |s| s.note = format!("pre-boot at {},{} on a macOS devbox", super::short(work), super::short(main)))?;
    event(ctx, Sev::Info, format!("show {} on mac: pre-boot at {},{} on a macOS devbox first", x.app, super::short(work), super::short(main)), Some(x));
    let rev = crate::state::Pair::new(work, main).rev();
    let spec = super::leg::Spec { since_ms: None, lane: "show-preboot".into(), agent: "devhub-show".into(), kind: crate::lease::Kind::Test, needs: "os=macos".into(), prio: SHOW_PRIO, rev, repo_dir: ctx.repo.to_string_lossy().into_owned(), node: None };
    // a box that refuses the start (its tree still held for another lease) is no verdict on the
    // app: ask again (another box, or the same once it is free)
    let mut tries = 0;
    let held = loop {
        match super::leg::Held::acquire_with(&spec, &|r| { r.src = crate::lease::Src::Overlay; r.class = format!("leg:{}", spec.lane); }, &mut |_| {}, &|| false) {
            Ok(h) => break h,
            Err(e) if e.contains("refused") && tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(10));
            }
            Err(e) => return Err(format!("no macOS devbox for the pre-boot: {e}")),
        }
    };
    let node = held.node.clone();
    // its queue wait and the tree's move to the pair (what a box costs is one or the other)
    let tree_ms = ctx.leases.view().iter().find(|l| l.id == held.id).and_then(|l| l.setup_ms).unwrap_or(0);
    stage(format!("box {node}: queue {} s, tree {} s", began.elapsed().as_secs().saturating_sub(tree_ms / 1000), tree_ms / 1000));
    let log = ctx.logs("show").join(format!("{}-preboot.log", x.id));
    let _ = std::fs::create_dir_all(ctx.logs("show"));
    let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    let t = std::time::Instant::now();
    let mut out = file.try_clone().map_err(|e| e.to_string())?;
    match ensure_jit(&held, &mut out) {
        Ok(()) => stage(format!("jit {} s", t.elapsed().as_secs())),
        Err(e) => {
            let _ = crate::cli::ask(&ctl(), None, "release", &held.id);
            return Err(format!("{e}; {}", log.display()));
        }
    }
    let boot = std::time::Instant::now();
    let argv: Vec<String> = ["env", "MAKEPAD_HIDE_WINDOWS=1"].into_iter().chain(JIT_VERSION[..9].iter().copied()).chain(["run", x.app.as_str()]).map(str::to_string).collect();
    // the run on its own thread; this one watches the JIT come up and settle
    struct Out {
        file: std::fs::File,
        text: Arc<Mutex<Vec<u8>>>,
    }
    impl std::io::Write for Out {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let mut t = self.text.lock().unwrap();
            if t.len() < 4 << 20 {
                t.extend_from_slice(b);
            }
            self.file.write(b)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.file.flush()
        }
    }
    let text = Arc::new(Mutex::new(Vec::new()));
    let done: Arc<Mutex<Option<Result<i32, String>>>> = Arc::new(Mutex::new(None));
    let lease = held.id.clone();
    // the lease's end stops the app (its processes) and frees the box
    let release = || {
        let _ = crate::cli::ask(&ctl(), None, "release", &lease);
    };
    let held = Arc::new(held);
    let (h2, t2, d2) = (held.clone(), text.clone(), done.clone());
    let _ = std::thread::Builder::new().name("devhub-show-preboot".into()).spawn(move || {
        let r = h2.run(&argv, &mut Out { file, text: t2 });
        *d2.lock().unwrap() = Some(r);
    });
    let pid_of = || String::from_utf8_lossy(&text.lock().unwrap()).split("makepad-remote: pid ").nth(1).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse::<u32>().ok());
    let ended = || done.lock().unwrap().clone();
    let t = std::time::Instant::now();
    let pid = loop {
        if let Some(p) = pid_of() {
            break p;
        }
        if let Some(r) = ended() {
            release();
            return Err(format!("the pre-boot on {node} ended before its JIT came up ({}); {}", r.map_or_else(|e| e, |c| format!("exit {c}")), log_errors(&text)));
        }
        if t.elapsed() > PREBOOT_UP {
            release();
            return Err(format!("the pre-boot on {node} did not come up in {} min", PREBOOT_UP.as_secs() / 60));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    PREBOOTING.lock().unwrap().get_or_insert_with(HashMap::new).insert((node.clone(), pid), Vec::new());
    stage(format!("up {} s", boot.elapsed().as_secs()));
    let t = std::time::Instant::now();
    let mut early = None;
    let mut idle = 0;
    while t.elapsed() < SETTLE && !(idle >= 2 && t.elapsed() >= SETTLE_MIN) {
        if let Some(r) = ended() {
            early = Some(r);
            break;
        }
        let mut quiet = false;
        let _ = makepad_remote::hub::ask(&ctl(), &format!("{node}:{pid}"), "cmd", "", "jit ms=300", &mut |l| {
            quiet |= l.starts_with("idle") || l.contains("\"idle\":true") || l.contains("\"state\":\"idle\"");
            true
        });
        idle = if quiet { idle + 1 } else { 0 };
        std::thread::sleep(Duration::from_millis(200));
    }
    stage(format!("settle {} s", t.elapsed().as_secs()));
    let said = PREBOOTING.lock().unwrap().as_mut().and_then(|m| m.remove(&(node.clone(), pid))).unwrap_or_default();
    release();
    if let Some(r) = early {
        return Err(format!("the app exited {} s into its pre-boot on {node} ({}); {}", t.elapsed().as_secs(), r.map_or_else(|e| e, |c| format!("exit {c}")), log_errors(&text)));
    }
    if !said.is_empty() {
        return Err(format!("its pre-boot on {node} (pid {pid}): {}", said.join(" | ")));
    }
    let errs = log_errors(&text);
    if !errs.is_empty() {
        return Err(format!("its pre-boot on {node} (pid {pid}) logged: {errs}"));
    }
    if let Some(k) = key {
        PREBOOTED.lock().unwrap().push(k);
    }
    event(ctx, Sev::Info, format!("show {} on mac: pre-boot passed on {node} in {} s ({} s settled, nothing failed)", x.app, began.elapsed().as_secs(), t.elapsed().as_secs()), Some(x));
    Ok(())
}

/// The failure lines of a pre-boot's output (a panic, a compile or script error).
fn log_errors(text: &Arc<Mutex<Vec<u8>>>) -> String {
    let t = String::from_utf8_lossy(&text.lock().unwrap()).into_owned();
    failure_lines(&t).into_iter().take(4).collect::<Vec<_>>().join(" | ")
}

/// What in a JIT's output says it failed, on any thread: a panic, an internal compiler error, a
/// compile or script error.
fn failure_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.contains("panicked at") || t.contains("PANIC") || t.contains("panic parked") || t.contains("stale access parked") || t.starts_with("error") || t.starts_with("[E]") || t.contains("RC error") || t.contains("compile error") || t.contains("internal compiler error") || t.contains("FATAL")
        })
        .map(|l| crate::proto::clean(l, 300))
        .collect()
}

/// The kinds of failure line, and the app records, a show remembers it said (the oldest go).
const SAID_MAX: usize = 64;
/// How far a show's log read runs ahead of what its record keeps: a master started again (each
/// landing re-execs it) reads up to this much again, and the kinds its record says it said keep
/// that quiet; the record is written only when this much was read or a failure was said.
const LOG_KEPT_EVERY: u64 = 256 * 1024;

fn remember(said: &mut Vec<String>, key: String) {
    said.push(key);
    let over = said.len().saturating_sub(SAID_MAX);
    said.drain(..over);
}

/// A kind of failure line as the show's record keeps it: the line without its numbers, hashed
/// (FNV-1a: the same in every process and build).
fn line_kind(line: &str) -> String {
    let h = line.bytes().filter(|b| !b.is_ascii_digit()).fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    format!("{h:016x}")
}

/// The running app's own output, through and after its startup: what the JIT's events do not carry
/// (a compile error, an internal compiler error) reaches the manager too. Each kind of line once
/// per launch, by what the show's record keeps: a master started again reads on from there.
fn watch_log(ctx: &Ctx, x: &Show) {
    use std::io::{Read, Seek, SeekFrom};
    if x.log.is_empty() {
        return;
    }
    let Ok(mut f) = std::fs::File::open(&x.log) else { return };
    // (a log shorter than what was read is a new one: read from its start)
    let from = if f.metadata().map_or(0, |m| m.len()) < x.log_read { 0 } else { x.log_read };
    let mut bytes = Vec::new();
    if f.seek(SeekFrom::Start(from)).is_err() || f.read_to_end(&mut bytes).is_err() {
        return;
    }
    // whole lines only: the rest comes with the next look
    let Some(end) = bytes.iter().rposition(|b| *b == b'\n') else { return };
    let at = from + end as u64 + 1;
    let mut said = x.log_said.clone();
    let mut new = Vec::new();
    for line in failure_lines(&String::from_utf8_lossy(&bytes[..end])) {
        let kind = line_kind(&line);
        if !said.contains(&kind) {
            remember(&mut said, kind);
            new.push(line);
        }
    }
    if new.is_empty() && at.abs_diff(x.log_read) < LOG_KEPT_EVERY {
        return;
    }
    // (the same launch: a relaunch starts its log and its marks again)
    let Some(x) = set(ctx, &x.id, |s| if s.launch_ms == x.launch_ms { (s.log_read, s.log_said) = (at, said); }).filter(|s| s.launch_ms == x.launch_ms) else { return };
    for line in new {
        failed(ctx, &x, line);
    }
}

/// The master's own launch on the Mac, detached in its own process group.
fn mac_launch(ctx: &Ctx, x: &Show) -> Result<(u32, String), String> {
    let (path, out) = log_file(ctx, x)?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = makepad_remote::group::command(cargo());
    cmd.args(["run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "run", &x.app]).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").env("MAKEPAD_SHOW", "1").stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    if !x.args.is_empty() {
        cmd.arg("--").args(&x.args);
    }
    // the Mac's own settings for the app (`~/.makepad/apps/<id>.conf`, jit.env), as the dock's launch
    if let Some(app) = makepad_app_registry::apps().iter().find(|a| a.package == x.app) {
        cmd.envs(makepad_app_registry::machine::launch_env(app.id, true));
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let child = cmd.spawn().map_err(|e| format!("cargo: {e}"))?;
    let pid = child.id();
    CHILDREN.lock().unwrap().get_or_insert_with(HashMap::new).insert(x.id.clone(), child);
    Ok((pid, path.to_string_lossy().into_owned()))
}

/// The Quest: the adb device whose model names a Quest.
fn quest_serial() -> Result<String, String> {
    let o = makepad_remote::group::command("adb").args(["devices", "-l"]).stdin(std::process::Stdio::null()).output().map_err(|e| format!("adb: {e}"))?;
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    text.lines().filter(|l| l.contains(" device ") && l.to_ascii_lowercase().contains("model:quest")).find_map(|l| l.split_whitespace().next().map(str::to_string)).ok_or_else(|| "no Quest in `adb devices -l`".into())
}

/// Build the Quest JIT host for the app on the Mac (the Quest exception), install, start.
fn quest_launch(ctx: &Ctx, x: &Show) -> Result<(u32, String), String> {
    let (path, out) = log_file(ctx, x)?;
    let serial = quest_serial()?;
    let run = |cmd: &mut std::process::Command, what: &str| -> Result<(), String> {
        let o = out.try_clone().map_err(|e| e.to_string())?;
        let e = out.try_clone().map_err(|e| e.to_string())?;
        let st = cmd.stdin(std::process::Stdio::null()).stdout(o).stderr(e).status().map_err(|e| format!("{what}: {e}"))?;
        if st.success() {
            Ok(())
        } else {
            Err(format!("{what} failed (log {})", path.display()))
        }
    };
    try_set(ctx, &x.id, |s| s.note = "building the Quest JIT host on the Mac".into())?;
    let mut build = makepad_remote::group::command("nice");
    build.args(["-n", "10"]).arg(cargo()).args(["run", "--release", "-p", "cargo-makepad", "--", "android", "--abi=aarch64", "build-host", "--app", &x.app, "--devhub-name=quest", "--quest"]).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").env("CARGO_BUILD_JOBS", "4");
    run(&mut build, "the Quest build")?;
    let name = x.app.replace('-', "_");
    let apk = ctx.repo.join(format!("target/makepad-android-apk/{name}/apk/{name}.apk"));
    run(makepad_remote::group::command("adb").args(["-s", &serial, "install", "--no-incremental", "-r"]).arg(&apk), "adb install")?;
    run(makepad_remote::group::command("adb").args(["-s", &serial, "shell", "am", "start", "-n", "dev.makepad.run/dev.makepad.run.RunActivity"]), "adb am start")?;
    Ok((0, path.to_string_lossy().into_owned()))
}

/// One inspect command in the show's JIT, its answer lines. The Mac's own by its socket: the hub
/// routes to other machines' apps (`mac:<pid>` read "unknown host `mac`").
fn ask_jit(x: &Show, line: &str) -> Result<Vec<String>, String> {
    if x.device == "mac" {
        let jit = makepad_remote::link::Client::connect(&x.pid.to_string()).map_err(|e| e.to_string())?;
        return jit.cmd(line).map_err(|e| e.to_string())?.map(|l| l.map_err(|e| e.to_string())).collect();
    }
    let mut out = Vec::new();
    makepad_remote::hub::ask(&ctl(), &format!("{}:{}", x.device, x.pid), "cmd", "", line, &mut |l| {
        out.push(l.to_string());
        true
    })?;
    Ok(out)
}

fn pin_said(said: &[String]) -> Result<(), String> {
    if said.iter().any(|l| l.contains("\"pinned\"")) {
        Ok(())
    } else {
        Err(format!("its JIT does not take a pin ({})", crate::proto::clean(&said.join(" "), 160)))
    }
}

/// One inspect request to a Mac show's JIT, answered within `wait`.
fn ask_mac_within(x: &Show, line: &str, wait: Duration) -> Result<Vec<String>, String> {
    let jit = makepad_remote::link::Client::connect(&x.pid.to_string()).map_err(|e| e.to_string())?;
    jit.set_timeout(Some(wait));
    jit.cmd(line).map_err(|e| e.to_string())?.map(|l| l.map_err(|e| e.to_string())).collect()
}

/// The files that differ between two pairs, checkout-relative (the commercial repo's under
/// `apps/commercial/`).
fn changed(ctx: &Ctx, from: &Pair, to: &Pair) -> Result<Vec<String>, String> {
    let diff = |dir: &std::path::Path, a: &str, b: &str| crate::gitread::files_differing(dir, a, b).ok_or_else(|| format!("no diff of {} to {} in {}", super::short(a), super::short(b), dir.display()));
    let mut files = if from.work == to.work { Vec::new() } else { diff(&ctx.repo, &from.work, &to.work)? };
    if from.main != to.main {
        files.extend(diff(&ctx.commercial(), &from.main, &to.main)?.into_iter().map(|f| format!("apps/commercial/{f}")));
    }
    Ok(files)
}

/// Of `files` (checkout-relative), those in the show's app graph, as its JIT says: its inputs and
/// every other file in one of its packages (a module it loads later reads it then). A file in no
/// package of the graph (another app's, the lockfile) does not reach it.
fn in_graph(x: &Show, files: &[String]) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let req = makepad_strict_json::obj(vec![("cmd", makepad_strict_json::s("inputs")), ("of", makepad_strict_json::s(&files.join("\n")))]).to_json();
    let said = ask_mac_within(x, &req, HOLD_ASK)?;
    let lines: Vec<makepad_strict_json::Value> = said.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    if !lines.iter().any(|v| v.get("count").is_some()) {
        return Err(format!("its JIT did not say its inputs ({})", crate::proto::clean(&said.join(" "), 160)));
    }
    let outside: std::collections::HashSet<&str> = lines.iter().filter_map(|v| v.get("outside")?.as_str()).collect();
    Ok(files.iter().filter(|f| !outside.contains(f.as_str())).cloned().collect())
}

/// A landing is about to move the checkout from `from` to `to` (land.rs, under its fast-forward
/// lock): each running Mac show whose app it reaches has its JIT hold back the landing's files of
/// the app first, so it takes none of it until the tip is proven for the app ([`judge_held`]); the
/// person's own saves go on. A held show holds each later landing's files too, at its first pair;
/// a person's pin does not keep a show out (it lets the person's saves in once unpinned, not this).
pub fn before_landing(from: &Pair, to: &Pair) {
    let Some(ctx) = super::ctx() else { return };
    let _hold = HOLD.lock().unwrap_or_else(|e| e.into_inner());
    let shows: Vec<Show> = ctx.store.read(|d| d.shows.iter().filter(|x| x.device == "mac" && matches!(x.state.as_str(), "running" | "paused") && x.pid > 0).cloned().collect());
    if shows.is_empty() {
        return;
    }
    let files = changed(ctx, from, to);
    for x in shows {
        let not_held = |e: String| {
            let note = unheld_note(&e, x.pinned, to);
            if note == x.note {
                return;
            }
            let shown = set(ctx, &x.id, |s| s.note = note.clone());
            // (only an unpinned show that a JIT able to hold did not hold takes the tip unproven now)
            let sev = if x.pinned || note.starts_with(OLD_JIT) { Sev::Info } else { Sev::Warn };
            event(ctx, sev, format!("show {} on mac: {note}", x.app), shown.as_ref());
        };
        let reach = match files.clone().and_then(|f| in_graph(&x, &f)) {
            Ok(r) if r.is_empty() => continue,
            Ok(r) => r,
            Err(e) => {
                not_held(e);
                continue;
            }
        };
        if let Err(e) = landed_ids(ctx, to, &reach).and_then(|ids| hold_jit(&x, &ids)) {
            not_held(e);
            continue;
        }
        if x.held.is_empty() {
            let why = format!("changes {}{}", reach[0], if reach.len() > 1 { format!(" and {} more files of its app", reach.len() - 1) } else { String::new() });
            let held = format!("held at {}: tip {} {why}; it takes the tip once the fast bucket proves it", from.short(), to.short());
            let x = set(ctx, &x.id, |s| {
                s.held = held.clone();
                (s.sha, s.main) = (from.work.clone(), from.main.clone());
            });
            event(ctx, Sev::Info, format!("show {} on mac {held}", x.as_ref().map_or("", |x| &x.app)), x.as_ref());
        }
    }
}

/// How a show's note begins when its JIT predates held landings.
const OLD_JIT: &str = "its JIT predates held landings";

/// What happens to a landing a show's JIT did not hold (`e`: why): a pinned show takes none until
/// the unpin, then what landed meanwhile; a JIT from before `hold` patches each landing in live as
/// it lands, as it did before; any other has the tip reach it unproven. Said once per show (one
/// note for every landing on a JIT that predates holds).
fn unheld_note(e: &str, pinned: bool, to: &Pair) -> String {
    let old = e.contains("unknown command");
    match (pinned, old) {
        (true, true) => format!("{OLD_JIT}; pinned, it takes no landing until the unpin, which then lets in what landed meanwhile, unproven"),
        (false, true) => format!("{OLD_JIT}: it patches each landing in live as it lands, unproven (a show started on a newer JIT holds them)"),
        (true, false) => format!("pinned, it takes no landing until the unpin; tip {} is not held for then: {}", to.short(), crate::proto::clean(e, 200)),
        (false, false) => format!("not held for tip {}: {}: the landing reaches it unproven", to.short(), crate::proto::clean(e, 200)),
    }
}

/// Each of `files` (checkout-relative, the commercial repo's under `apps/commercial/`) with the
/// blob id it has at `to` (`-`: none, the landing removes it).
fn landed_ids(ctx: &Ctx, to: &Pair, files: &[String]) -> Result<Vec<(String, String)>, String> {
    let (theirs, ours): (Vec<&String>, Vec<&String>) = files.iter().partition(|f| f.starts_with("apps/commercial/"));
    let ids = |dir: &std::path::Path, at: &str, paths: Vec<&str>| crate::gitread::entries_at(dir, at, &paths).ok_or_else(|| format!("no tree at {} in {}", super::short(at), dir.display()));
    let mut out = Vec::new();
    for (list, dir, at, strip) in [(ours, ctx.repo.clone(), &to.work, ""), (theirs, ctx.commercial(), &to.main, "apps/commercial/")] {
        if list.is_empty() {
            continue;
        }
        let found = ids(&dir, at, list.iter().map(|f| &f[strip.len()..]).collect())?;
        out.extend(list.into_iter().zip(found).map(|(f, id)| (f.clone(), if id.is_empty() { "-".to_string() } else { id })));
    }
    Ok(out)
}

/// Tells a Mac show's JIT to hold back these files of a landing ([`landed_ids`]).
fn hold_jit(x: &Show, files: &[(String, String)]) -> Result<usize, String> {
    let list: Vec<String> = files.iter().map(|(f, id)| format!("{f}={id}")).collect();
    let req = makepad_strict_json::obj(vec![("cmd", makepad_strict_json::s("hold")), ("files", makepad_strict_json::s(&list.join("\n")))]).to_json();
    hold_said(&ask_mac_within(x, &req, HOLD_ASK)?)
}

/// How many files a JIT says it holds back (`{"held":N}`).
fn hold_said(said: &[String]) -> Result<usize, String> {
    said.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("held")?.as_i64()).map(|n| n.max(0) as usize)
        .ok_or_else(|| format!("its JIT does not take a held landing ({})", crate::proto::clean(&said.join(" "), 160)))
}

/// The buckets' ledger and logs as they are now (their length, the logs folder's last change).
fn buckets_stamp(home: &std::path::Path) -> (u64, u64) {
    let len = std::fs::metadata(home.join("buckets.jsonl")).map_or(0, |m| m.len());
    let logs = std::fs::metadata(home.join("bucket-logs")).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
    (len, logs)
}

/// A held show takes the checkout's tip once it is proven for its app ([`proven`]): its JIT lets
/// the held landing in live (a change that needs a restart stays the JIT's offer to the person).
/// Until then its row says why it stays; a new break of its app is said once as an event.
fn judge_held(ctx: &Ctx, x: &Show) {
    let _hold = HOLD.lock().unwrap_or_else(|e| e.into_inner());
    // (none while a landing moves the checkout)
    let Some(tip) = super::snapshot(&ctx.repo) else { return };
    let Some(x) = ctx.store.read(|d| d.shows.iter().find(|s| s.id == x.id).cloned()) else { return };
    if x.held.is_empty() {
        return;
    }
    // a run that restarted in place (the person's edit needed it) read every file as it is: it
    // runs the tip, and its JIT holds nothing
    if ask_mac_within(&x, "hold", HOLD_ASK).and_then(|said| hold_said(&said)) == Ok(0) {
        let x = set(ctx, &x.id, |s| {
            s.held.clear();
            (s.sha, s.main) = (tip.work.clone(), tip.main.clone());
        });
        event(ctx, Sev::Info, format!("show {} on mac runs tip {}: its run restarted and read every file as it is", x.as_ref().map_or("", |x| &x.app), tip.short()), x.as_ref());
        return;
    }
    let (len, logs) = buckets_stamp(&ctx.home);
    // (an unpin judges it again at once)
    let stamp = (x.pid, x.pinned, tip.clone(), len, logs);
    if JUDGED.lock().unwrap().get_or_insert_with(HashMap::new).insert(x.id.clone(), stamp.clone()).as_ref() == Some(&stamp) {
        return;
    }
    let at = crate::bucket::tip_state(&ctx.home, &tip);
    let pair = Pair::new(&x.sha, &x.main);
    let same = |p: &Pair| changed(ctx, p, &tip).and_then(|f| in_graph(&x, &f)).map(|r| r.is_empty());
    let ahead = |p: &Pair| p != &pair && crate::gitread::is_ancestor(&ctx.repo, &pair.work, &p.work) && crate::gitread::is_ancestor(&ctx.commercial(), &pair.main, &p.main);
    let reaches = |units: &[String]| reaching_units(ctx, &x, &tip, units);
    let id = x.id.clone();
    match proven(&x.app, x.pinned, &x.took_red, &pair, &tip, &at, &same, &ahead, &reaches) {
        Ok((to, took)) => {
            // the tip: the whole held landing goes in; a pair newer landings overtook: its own text
            let released = if to == tip { ask_mac_within(&x, "hold off", HOLD_ASK).and_then(|said| hold_said(&said)) } else { take_pair(ctx, &x, &pair, &to) };
            if let Err(e) = released {
                if to == tip {
                    // judged again at the next tick
                    JUDGED.lock().unwrap().get_or_insert_with(HashMap::new).remove(&x.id);
                }
                set(ctx, &x.id, |s| s.note = format!("{} is proven, its JIT did not take it: {}", to.short(), crate::proto::clean(&e, 200)));
                return;
            }
            let why = if took.is_some() { "the fast bucket proved it" } else { "nothing of its app changed" };
            let x = set(ctx, &x.id, |s| {
                // (an older pair's take leaves the newer landings held: judged again next tick)
                s.held = if to == tip { String::new() } else { format!("held at {}: tip {} is judged next", to.short(), tip.short()) };
                (s.sha, s.main) = (to.work.clone(), to.main.clone());
                if let Some(red) = &took {
                    s.took_red = red.clone();
                }
            });
            let what = if to == tip { "tip" } else { "pair" };
            event(ctx, Sev::Info, format!("show {} on mac takes {what} {}: {why}; its JIT catches up live (a change that needs a restart stays offered)", x.as_ref().map_or("", |x| &x.app), to.short()), x.as_ref());
            if to != tip {
                JUDGED.lock().unwrap().get_or_insert_with(HashMap::new).remove(&id);
            }
        }
        Err((why, loud)) => {
            let held = format!("held at {}: tip {} {why}", pair.short(), tip.short());
            if held != x.held {
                let shown = set(ctx, &x.id, |s| s.held = held.clone());
                if loud {
                    event(ctx, Sev::Info, format!("show {} on mac {held}", x.app), shown.as_ref());
                }
            }
        }
    }
}

/// Which pair a held show at `pair` takes: never one while `pinned` (no landing reaches a pinned
/// show until the unpin); else Ok(the pair, the red units it takes with it: None when nothing of
/// its app changed since its pair, the tip) once the fast bucket's newest run looked at it and no
/// check, cargo or present unit that reaches the app is red there that was not red when it took
/// its pair. That pair is the tip when nothing of the app differs between them (`same`), else the
/// run's own pair when it is `ahead` of the show's: a landing stream overtook each fast run just
/// before it ended, and the user's Fab sat 25 min at one pair (11-07). Err(why it stays, and
/// whether that is a new break of the app). `same(p)`: nothing that differs from `p` to the tip is
/// in the app's graph; `ahead(p)`: `p` follows `pair` in both repositories; `reaches(units)`: those
/// of the units whose package is in it.
fn proven(app: &str, pinned: bool, took_red: &[String], pair: &Pair, tip: &Pair, at: &TipState, same: &dyn Fn(&Pair) -> Result<bool, String>, ahead: &dyn Fn(&Pair) -> bool, reaches: &dyn Fn(&[String]) -> Result<Vec<String>, String>) -> Result<(Pair, Option<Vec<String>>), (String, bool)> {
    if pinned {
        return Err(("is not let in: the show is pinned, it takes no landing until the unpin".to_string(), false));
    }
    let quiet = |e: String| (format!("is not judged: {}", crate::proto::clean(&e, 200)), false);
    if same(pair).map_err(quiet)? {
        return Ok((tip.clone(), None));
    }
    let present = format!("present {app}");
    let key = |u: &str| u.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    let waits = || {
        let hot = if at.hot_failing.iter().any(|u| key(u) == present) { "failed its present in the hot bucket; it " } else { "" };
        let hm = |ms: u64| makepad_civil_time::local_or_utc((ms / 1000) as i64).hm();
        let when = match &at.running {
            Some((ms, looks)) if looks == tip => format!("its run of it since {}", hm(*ms)),
            Some((ms, looks)) => format!("its run of {} since {}; the next one, of the newest tip, starts as it ends", looks.short(), hm(*ms)),
            None => "its run of the newest tip starts at once".to_string(),
        };
        (format!("{hot}waits for the fast bucket ({when})"), false)
    };
    let Some((looked, log)) = &at.fast else { return Err(waits()) };
    let to = if looked == tip || same(looked).map_err(quiet)? {
        tip.clone()
    } else if ahead(looked) {
        looked.clone()
    } else {
        return Err(waits());
    };
    if let Some(step) = &at.stopped {
        return Err((format!("is not proven: the fast bucket's run of {} stopped at its {step} step (log {log})", looked.short()), true));
    }
    let new: Vec<String> = at.red.keys().filter(|u| !took_red.contains(u)).filter(|u| match u.split_once(' ') {
        Some(("check" | "cargo", _)) => true,
        Some(("present", _)) => **u == present,
        _ => false,
    }).cloned().collect();
    if !new.is_empty() {
        if let Some(unit) = reaches(&new).map_err(quiet)?.first() {
            let who = at.red.get(unit).map(String::as_str).unwrap_or("");
            return Err((format!("has {unit} failing {} (log {log})", crate::bucket::still_since(who)), true));
        }
    }
    Ok((to, Some(at.red.keys().cloned().collect())))
}

/// Has a held show's JIT run `to`, a pair the fast bucket proved that newer landings overtook:
/// each file of its app that differs between the show's pair and `to` takes its text at `to`
/// (written to a folder the JIT reads it from), and every other held file stays held (`inspect
/// hold take`). Answers how many files stay held.
fn take_pair(ctx: &Ctx, x: &Show, pair: &Pair, to: &Pair) -> Result<usize, String> {
    let ids = landed_ids(ctx, to, &in_graph(x, &changed(ctx, pair, to)?)?)?;
    let dir = ctx.home.join("shows").join(format!("take-{}", x.id));
    let _ = std::fs::remove_dir_all(&dir);
    for (f, id) in ids.iter().filter(|(_, id)| id != "-") {
        let (repo, rel) = match f.strip_prefix("apps/commercial/") { Some(rel) => (ctx.commercial(), rel), None => (ctx.repo.clone(), f.as_str()) };
        let path = dir.join(f);
        crate::gitread::blob(&repo, id).and_then(|text| {
            std::fs::create_dir_all(path.parent().unwrap_or(&dir)).and_then(|_| std::fs::write(&path, text)).map_err(|e| format!("{}: {e}", path.display()))
        }).map_err(|e| format!("{rel} at {}: {e}", to.short()))?;
    }
    let list: Vec<String> = ids.iter().map(|(f, id)| format!("{f}={id}")).collect();
    let req = makepad_strict_json::obj(vec![("cmd", makepad_strict_json::s("hold take")), ("dir", makepad_strict_json::s(&dir.display().to_string())), ("files", makepad_strict_json::s(&list.join("\n")))]).to_json();
    let said = ask_mac_within(x, &req, HOLD_ASK).and_then(|said| hold_said(&said));
    let _ = std::fs::remove_dir_all(&dir);
    said
}

/// Of the units (`check <pkg>`, `present <app>`), those of the show's app: its own present, and a
/// check or cargo unit whose package is in the app's graph (its manifest at the tip, as its JIT
/// says).
fn reaching_units(ctx: &Ctx, x: &Show, tip: &Pair, units: &[String]) -> Result<Vec<String>, String> {
    let present = format!("present {}", x.app);
    let mut out: Vec<String> = units.iter().filter(|u| **u == present).cloned().collect();
    let pkgs: Vec<(&String, &str)> = units.iter().filter(|u| **u != present).filter_map(|u| Some((u, u.split_whitespace().nth(1)?))).collect();
    if pkgs.is_empty() {
        return Ok(out);
    }
    let dirs = super::ci::package_dirs(&super::ci::Tree::at(&ctx.repo, tip)?);
    let manifests: Vec<(&String, String)> = pkgs.into_iter().filter_map(|(u, p)| dirs.get(p).map(|d| (u, if d.is_empty() { "Cargo.toml".to_string() } else { format!("{d}/Cargo.toml") }))).collect();
    let hit = in_graph(x, &manifests.iter().map(|(_, m)| m.clone()).collect::<Vec<_>>())?;
    out.extend(manifests.into_iter().filter(|(_, m)| hit.contains(m)).map(|(u, _)| u.clone()));
    Ok(out)
}

/// Why a new Mac show of `app` waits: its own present, check or cargo unit red at the checkout's
/// tip (the hot bucket's run of it, or the fast bucket's). None: it opens.
fn launch_red(ctx: &Ctx, app: &str) -> Option<String> {
    let tip = super::snapshot(&ctx.repo)?;
    let at = crate::bucket::tip_state(&ctx.home, &tip);
    let present = format!("present {app}");
    let key = |u: &str| u.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    if at.hot_failing.iter().any(|u| key(u) == present) {
        return Some(format!("tip {} failed its present in the hot bucket", tip.short()));
    }
    let own = [present, format!("check {app}"), format!("cargo {app}")];
    let (looked, log) = at.fast.as_ref().filter(|(looked, _)| *looked == tip)?;
    let unit = own.iter().find(|u| at.red.contains_key(*u))?;
    Some(format!("tip {} has {unit} failing {} (log {log})", looked.short(), crate::bucket::still_since(&at.red[unit])))
}

/// What one inspect request to a show's JIT answered ([`ask_jit`]; a test's stand-in).
type AskJit = dyn Fn(&Show, &str) -> Result<Vec<String>, String>;

/// A tick's pin step for a running show `x` (the tick's snapshot of it), on its pin as it is now
/// ([`PIN`]): a pinned show's JIT holds still and is told so again (after a restart of the JIT); a
/// pin set in the JIT itself (`devhub inspect .. pin on`) is a pin, read at a landing (it read as
/// updated while it held still). Returns the show as it is now.
fn keep_pin(ctx: &Ctx, x: Show, landed: bool, ask: &AskJit) -> Show {
    let _one = PIN.lock().unwrap_or_else(|e| e.into_inner());
    let Some(x) = ctx.store.read(|d| d.shows.iter().find(|s| s.id == x.id).cloned()) else { return x };
    if x.pid == 0 {
        return x;
    }
    if x.pinned {
        let _ = ask(&x, "pin on").and_then(|said| pin_said(&said));
        return x;
    }
    if landed && x.device != "quest" && ask(&x, "pin").is_ok_and(|l| l.iter().any(|l| l.contains("\"pinned\":true"))) {
        return set(ctx, &x.id, |s| s.pinned = true).unwrap_or(x);
    }
    x
}

/// Pins or unpins a show that is there. A held one's landing stays held until its tip is proven.
fn pin_show(ctx: &Ctx, x: &Show, on: bool, ask: &AskJit) -> Result<String, String> {
    let _one = PIN.lock().unwrap_or_else(|e| e.into_inner());
    if x.pid > 0 {
        pin_said(&ask(x, if on { "pin on" } else { "pin off" })?)?;
    }
    let shown = try_set(ctx, &x.id, |s| s.pinned = on)?;
    let said = if on { "pinned: landings go on, it stays as it is until the unpin".to_string() } else if !x.held.is_empty() { format!("unpinned: it catches up, but for the landing it holds ({})", x.held) } else { "unpinned: it catches up".to_string() };
    event(ctx, Sev::Info, format!("show {} on {}: {said}", x.app, x.device), shown.as_ref());
    Ok(format!("{{\"app\":{},\"device\":{},\"pinned\":{on}}}", json_str(&x.app), json_str(&x.device)))
}

/// A registry name (`calculator`, what the dock calls it) as its package
/// (`makepad-app-calculator`): `makepad run` takes packages.
fn package_of(app: &str) -> String {
    makepad_app_registry::find(app).map_or(app.to_string(), |a| a.package.to_string())
}

/// Stops a show by its recorded pid (the Mac: its process group; a node or the Quest: the
/// device's own stop).
fn stop(x: &Show) -> Result<(), String> {
    match x.device.as_str() {
        "mac" if x.pid > 0 => {
            makepad_remote::group::signal(x.pid, false);
            Ok(())
        }
        "mac" => Ok(()),
        "quest" => {
            let serial = quest_serial()?;
            let st = makepad_remote::group::command("adb").args(["-s", &serial, "shell", "am", "force-stop", "dev.makepad.run"]).stdin(std::process::Stdio::null()).status().map_err(|e| e.to_string())?;
            st.success().then_some(()).ok_or_else(|| "adb am force-stop failed".into())
        }
        node if x.pid > 0 => crate::cli::ask(&ctl(), Some(node), "launch", &format!("stop {}", x.pid)).and_then(|l| match l.first() {
            Some(e) if e.starts_with("{\"err\":") => Err(e.clone()),
            _ => Ok(()),
        }),
        _ => Err("its pid is not known yet".into()),
    }
}

/// A record on the master's stream: a panic of a show's app on its device pauses the show.
pub fn on_record(ctx: &Ctx, r: &Record) {
    if !matches!(r.kind.as_str(), "panic" | "hang" | "deadlock" | "patch_failed" | "scripterr") {
        return;
    }
    // a pre-boot's JIT: its failure blocks the show
    if let Some(v) = PREBOOTING.lock().unwrap().as_mut().and_then(|m| m.get_mut(&(r.machine.clone(), r.pid))) {
        v.push(format!("{}: {}", r.kind, crate::proto::clean(&r.line, 200)));
        return;
    }
    let dev = if r.machine == ctx.machine { "mac".to_string() } else { r.machine.clone() };
    let hit = ctx.store.read(|d| d.shows.iter().find(|x| matches!(x.state.as_str(), "running" | "paused") && x.device == dev && (x.pid == r.pid || same_app(&r.app, &x.app))).cloned());
    let Some(x) = hit else { return };
    // each record once per show, across re-execs: by its id, which names a JIT event's run, so the
    // same event heard again (a hub or master that follows the JIT again) says nothing while a run
    // started again in place (same pid, its seq from 1 again) does
    let key = r.id.clone();
    let mut fresh = false;
    let Some(x) = set(ctx, &x.id, |s| if !s.events_said.contains(&key) {
        fresh = true;
        remember(&mut s.events_said, key.clone());
    }) else { return };
    if !fresh {
        return;
    }
    if r.kind != "panic" {
        // a hang, a deadlock, a patch or script error: the app runs on; said, not a state
        return failed(ctx, &x, r.line.clone());
    }
    let d = makepad_strict_json::parse(r.detail.as_bytes()).ok();
    let f = |k: &str| d.as_ref().and_then(|d| d.get(k)).and_then(|v| v.as_str()).map(|s| crate::proto::clean(s, 300)).unwrap_or_default();
    let p = JobPanic { id: f("id"), pid: r.pid, message: f("message"), thread: f("thread"), location: f("location"), at_ms: wall_ms() };
    let Some(x) = set(ctx, &x.id, |s| {
        s.state = "paused".into();
        s.panic = Some(p.clone());
    }) else { return };
    event(ctx, Sev::Err, format!("show {} on {} PAUSED on a panic: {} at {} [{}] ({}): devhub inspect {}/{} panics", x.app, x.device, p.message, p.location, p.thread, p.id, r.machine, r.pid), Some(&x));
    failed(ctx, &x, format!("panic {}: {} at {} [{}]", p.id, p.message, p.location, p.thread));
}

/// `show add {"app","on","after":[words],"replace","by"}`, `show stop <app> <device>`, `show list [all]`.
pub fn serve(ctx: &Arc<Ctx>, line: &str) -> Result<String, String> {
    let mut w = line.splitn(3, ' ');
    let _ = w.next();
    let verb = w.next().unwrap_or("list");
    let rest = w.next().unwrap_or("").trim();
    match verb {
        "add" => {
            let v = makepad_strict_json::parse(rest.as_bytes()).map_err(|_| "show: bad request")?;
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let app = s("app");
            if app.is_empty() || !app.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return Err(format!("show: `{app}` is not a package name"));
            }
            let app = package_of(&app);
            let after: Vec<String> = v.get("after").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            // nothing starts on a device for a name no Cargo.toml has, landed or in a branch the
            // show waits for (`show 518` started a pre-boot on the Mac)
            if package_dir(ctx, &app, "work", "main").is_none() && !after.iter().any(|b| package_dir(ctx, &app, b, b).is_some()) {
                let branches = if after.is_empty() { String::new() } else { format!(" or {}", after.join(", ")) };
                return Err(format!("show: no package `{app}` at work or commercial main{branches} (an inbox event: `devhub show <its id=>`)"));
            }
            let dev = device(ctx, &s("on"))?;
            let cond = if after.is_empty() { Cond::None } else { super::after::parse_cond(ctx, &after.iter().map(String::as_str).collect::<Vec<_>>())? };
            let replace = v.get("replace").and_then(|x| x.as_bool()).unwrap_or(false);
            let pin = v.get("pin").and_then(|x| x.as_bool()).unwrap_or(false);
            let args: Vec<String> = v.get("args").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            // (a node's show asks its WM's dock to launch or focus, the Quest's starts its activity:
            // neither takes arguments)
            if !args.is_empty() && dev != "mac" {
                return Err(format!("show: `-- <args>` reaches an app on the Mac only, not on {dev}"));
            }
            // one instance per app per device; a running one may hold unsaved work
            // one that ended since the last look is not running
            if let Some(old) = ctx.store.read(|d| d.shows.iter().find(|x| x.active() && x.app == app && x.device == dev && x.device == "mac" && x.state == "running").cloned()) {
                mac_watch(ctx, &old);
            }
            if let Some(old) = ctx.store.read(|d| d.shows.iter().find(|x| x.active() && x.app == app && x.device == dev).cloned()) {
                // (a pin leaves it running: nothing of it is at risk)
                if matches!(old.state.as_str(), "running" | "paused") && !replace && pin && args.is_empty() {
                    return pin_show(ctx, &old, true, &ask_jit);
                }
                // the person asks to see it again: it comes to the front, its work as it is
                if old.state == "running" && old.pid > 0 && !replace && args.is_empty() && dev == "mac" {
                    return match ask_jit(&old, "front") {
                        Ok(said) if said.iter().any(|l| l.contains("\"front\"")) => {
                            event(ctx, Sev::Info, format!("show {app} on {dev}: running (pid {}): brought to the front", old.pid), Some(&old));
                            Ok(format!("{{\"id\":{},\"app\":{},\"device\":{},\"state\":\"running\",\"front\":true}}", json_str(&old.id), json_str(&app), json_str(&dev)))
                        }
                        Ok(said) => Err(format!("{app} runs on {dev} (pid {}) and could not come to the front: {}", old.pid, crate::proto::clean(&said.join(" "), 160))),
                        Err(e) => Err(format!("{app} runs on {dev} (pid {}) and could not come to the front: {e}", old.pid)),
                    };
                }
                if matches!(old.state.as_str(), "running" | "paused") && !replace {
                    return Err(format!("{app} already runs on {dev} ({}, pid {}, {} s): it may hold unsaved work; ask the user, then `devhub show {app} --on {dev} --replace`", old.state, old.pid, wall_ms().saturating_sub(old.started_ms) / 1000));
                }
                if matches!(old.state.as_str(), "running" | "paused") {
                    stop(&old)?;
                }
                try_set(ctx, &old.id, |s| {
                    s.state = "replaced".into();
                    s.ended_ms = wall_ms();
                })?;
            }
            // its own unit red at the tip: it waits for a tip that is not
            let red = if cond == Cond::None && dev == "mac" { launch_red(ctx, &app).unwrap_or_default() } else { String::new() };
            let x = ctx.store.update(|d| {
                d.shows_next += 1;
                let x = Show { id: format!("V{}", d.shows_next), app: app.clone(), device: dev.clone(), state: if cond == Cond::None && red.is_empty() { "starting" } else { "waiting" }.into(), cond: cond.clone(), added_ms: wall_ms(), by: s("by"), pinned: pin, args: args.clone(), held: red.clone(), ..Default::default() };
                d.shows.push(x.clone());
                x
            })?;
            ctx.store.kick();
            if x.state == "starting" {
                event(ctx, Sev::Info, format!("show {app} on {dev}: starting"), Some(&x));
                // the launch runs on its own thread (a Quest build takes minutes)
                let (c, x2) = (ctx.clone(), x.clone());
                let _ = std::thread::Builder::new().name("devhub-show".into()).spawn(move || launch(&c, &x2));
            } else if !red.is_empty() {
                event(ctx, Sev::Info, format!("show {app} on {dev} waits: {red}"), Some(&x));
            } else {
                event(ctx, Sev::Info, format!("show {app} on {dev}: waiting for {}", cond.describe()), Some(&x));
            }
            Ok(format!("{{\"id\":{},\"app\":{},\"device\":{},\"state\":{}}}", json_str(&x.id), json_str(&app), json_str(&dev), json_str(&x.state)))
        }
        "stop" => {
            let mut w = rest.split_whitespace();
            let (Some(app), dev) = (w.next(), w.next().unwrap_or("mac")) else { return Err("show stop <app> [<device>]".into()) };
            let dev = device(ctx, dev)?;
            let app = &package_of(app);
            let x = ctx.store.read(|d| d.shows.iter().find(|x| x.active() && x.app == *app && x.device == dev).cloned()).ok_or_else(|| format!("{app} is not shown on {dev}"))?;
            if matches!(x.state.as_str(), "running" | "paused") {
                stop(&x)?;
            }
            let x = try_set(ctx, &x.id, |s| {
                s.state = if s.state == "waiting" { "cancelled" } else { "stopped" }.into();
                s.ended_ms = wall_ms();
            })?;
            event(ctx, Sev::Info, format!("show {app} on {dev} stopped"), x.as_ref());
            Ok(format!("{{\"app\":{},\"device\":{},\"state\":\"stopped\"}}", json_str(app), json_str(&dev)))
        }
        "pin" => {
            let mut w = rest.split_whitespace();
            let (Some(app), Some(dev), Some(on)) = (w.next(), w.next(), w.next()) else { return Err("show pin <app> <device> on|off".into()) };
            let dev = device(ctx, dev)?;
            let app = &package_of(app);
            let x = ctx.store.read(|d| d.shows.iter().find(|x| x.active() && x.app == *app && x.device == dev).cloned()).ok_or_else(|| format!("{app} is not shown on {dev}"))?;
            pin_show(ctx, &x, on == "on", &ask_jit)
        }
        "list" => {
            let all = rest == "all";
            // the running ones and what became of the recent ones (closed, crashed, blocked, failed)
            let recent = wall_ms().saturating_sub(RECENT_MS);
            let list: Vec<String> = ctx.store.read(|d| d.shows.iter().filter(|x| all || x.active() || x.ended_ms >= recent || x.added_ms >= recent).map(|x| crate::state::show_value(x).to_json()).collect());
            Ok(format!("[{}]", list.join(",")))
        }
        _ => Err(format!("show: unknown word `{verb}` (add stop pin list)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::master::LeaseMaster;
    use crate::state::Store;
    use std::collections::BTreeMap;

    /// A JIT's answer to `hold`: how many files it holds back; one that does not know the command
    /// (an older build) holds nothing, and says so.
    #[test]
    fn a_jit_says_how_many_files_of_a_landing_it_holds_back() {
        assert_eq!(hold_said(&["{\"held\":3}".to_string()]), Ok(3));
        assert_eq!(hold_said(&["{\"held\":0}".to_string()]), Ok(0));
        assert!(hold_said(&["{\"error\":\"unknown command `hold`\"}".to_string()]).unwrap_err().contains("does not take a held landing"));
    }

    /// A landing a show's JIT did not hold is said as what happens: a pinned show takes none
    /// until the unpin; a JIT from before `hold` patches it in live as ever (one note for every
    /// landing); only another miss has the tip reach the show unproven.
    #[test]
    fn a_landing_a_shows_jit_did_not_hold_says_what_happens_to_it() {
        let (to, later) = (Pair::new(&"a".repeat(40), &"f".repeat(40)), Pair::new(&"b".repeat(40), &"f".repeat(40)));
        let old = hold_said(&["{\"error\":\"unknown command `hold`\"}".to_string()]).unwrap_err();
        let pinned = unheld_note(&old, true, &to);
        assert!(pinned.starts_with(OLD_JIT) && pinned.contains("no landing until the unpin") && !pinned.contains("reaches it"), "{pinned}");
        let live = unheld_note(&old, false, &to);
        assert!(live.starts_with(OLD_JIT) && live.contains("patches each landing in live"), "{live}");
        assert_eq!(live, unheld_note(&old, false, &later), "one note for every landing");
        let gone = unheld_note("no answer", false, &to);
        assert!(gone.starts_with(&format!("not held for tip {}", to.short())) && gone.ends_with("the landing reaches it unproven"), "{gone}");
        assert!(unheld_note("no answer", true, &to).starts_with("pinned, it takes no landing until the unpin"));
    }

    #[test]
    fn a_held_show_takes_a_tip_once_nothing_new_that_reaches_its_app_is_red_there() {
        let pair = |c: char| Pair::new(&c.to_string().repeat(40), &"f".repeat(40));
        let (at_pair, tip, other) = (pair('a'), pair('b'), pair('c'));
        let app = "makepad-foo";
        let differs = |_: &Pair| Ok(false);
        let none = |_: &[String]| Ok(Vec::new());
        let behind = |_: &Pair| false;
        // nothing of its app changed since its pair (a landing that did not come): it takes the tip
        assert_eq!(proven(app, false, &[], &at_pair, &tip, &TipState::default(), &|_: &Pair| Ok(true), &behind, &none), Ok((tip.clone(), None)));
        // pinned: it takes nothing, proven or not, until the unpin (the pin is the person's)
        let green = TipState { fast: Some((tip.clone(), "l".into())), ..Default::default() };
        for same in [&differs as &dyn Fn(&Pair) -> Result<bool, String>, &|_: &Pair| Ok(true)] {
            let e = proven(app, true, &[], &at_pair, &tip, &green, same, &behind, &none).unwrap_err();
            assert_eq!(e, ("is not let in: the show is pinned, it takes no landing until the unpin".to_string(), false));
        }
        // no fast run yet, or one of a tip whose difference reaches the app: it waits, quietly
        let e = proven(app, false, &[], &at_pair, &tip, &TipState::default(), &differs, &behind, &none).unwrap_err();
        assert!(e.0.contains("waits for the fast bucket (its run of the newest tip starts at once)") && !e.1, "{e:?}");
        let going = TipState { fast: Some((other.clone(), "l".into())), running: Some((60_000, tip.clone())), hot_failing: vec![format!("present {app}")], ..Default::default() };
        let e = proven(app, false, &[], &at_pair, &tip, &going, &differs, &behind, &none).unwrap_err();
        assert!(e.0.starts_with("failed its present in the hot bucket; it waits for the fast bucket (its run of it since ") && !e.1, "{e:?}");
        let before = TipState { running: Some((60_000, other.clone())), ..Default::default() };
        let e = proven(app, false, &[], &at_pair, &tip, &before, &differs, &behind, &none).unwrap_err();
        assert!(e.0.starts_with(&format!("waits for the fast bucket (its run of {} since ", other.short())) && e.0.ends_with("the next one, of the newest tip, starts as it ends)"), "{e:?}");
        // a run of the tip, or of a pair the same for the app, with nothing red: it takes the tip
        assert_eq!(proven(app, false, &[], &at_pair, &tip, &green, &differs, &behind, &none), Ok((tip.clone(), Some(Vec::new()))));
        let same_as_other = |p: &Pair| Ok(*p == other);
        assert_eq!(proven(app, false, &[], &at_pair, &tip, &going, &same_as_other, &behind, &none), Ok((tip.clone(), Some(Vec::new()))));
        // a run that stopped at a step said nothing of the units
        let stopped = TipState { stopped: Some("jit".into()), ..green.clone() };
        let e = proven(app, false, &[], &at_pair, &tip, &stopped, &differs, &behind, &none).unwrap_err();
        assert!(e.0.contains("stopped at its jit step") && e.1, "{e:?}");
        // red: a check of its graph is a new break; a check outside it, another app's present and a
        // test target are not; one red when it took its pair is no new break
        let red = TipState { red: BTreeMap::from([
            ("check makepad-bar".to_string(), "rc-x (LANE-X) #7".to_string()), ("check makepad-other".to_string(), "rc-y (LANE-Y) #8".to_string()),
            ("present makepad-other".to_string(), String::new()), ("tests makepad-foo".to_string(), String::new()),
        ]), ..green.clone() };
        let asked = std::cell::RefCell::new(Vec::new());
        let graph = |units: &[String]| {
            asked.borrow_mut().extend(units.iter().cloned());
            Ok(units.iter().filter(|u| *u == "check makepad-bar").cloned().collect())
        };
        let e = proven(app, false, &[], &at_pair, &tip, &red, &differs, &behind, &graph).unwrap_err();
        assert_eq!(e, ("has check makepad-bar failing since rc-x (LANE-X) #7 (log l)".to_string(), true));
        assert_eq!(*asked.borrow(), vec!["check makepad-bar".to_string(), "check makepad-other".to_string()], "only checks, cargo units and its own present");
        let all: Vec<String> = red.red.keys().cloned().collect();
        assert_eq!(proven(app, false, &["check makepad-bar".to_string()], &at_pair, &tip, &red, &differs, &behind, &graph), Ok((tip.clone(), Some(all.clone()))));
        assert_eq!(proven(app, false, &[], &at_pair, &tip, &red, &differs, &behind, &none), Ok((tip.clone(), Some(all))));
        // its JIT did not say: it stays, quietly
        let e = proven(app, false, &[], &at_pair, &tip, &red, &differs, &behind, &|_: &[String]| Err("no answer".to_string())).unwrap_err();
        assert!(e.0.contains("is not judged: no answer") && !e.1, "{e:?}");
    }

    /// A landing stream with gaps shorter than one fast run moves a held show to each pair a run
    /// proved in turn, never to one no run looked at; a run of a pair not ahead of the show's, or
    /// one that broke the app, moves it nowhere; a run of the tip takes the tip.
    #[test]
    fn a_landing_stream_moves_a_held_show_to_each_proven_pair_in_turn() {
        let pair = |c: char| Pair::new(&c.to_string().repeat(40), &c.to_string().repeat(40));
        let app = "makepad-foo";
        let differs = |_: &Pair| Ok(false);
        let none = |_: &[String]| Ok(Vec::new());
        let mut show = pair('a');
        // each run ends just after the next landing: it proves the pair before the tip
        for (looked, tip) in [('b', 'c'), ('c', 'd'), ('d', 'e')] {
            let (looked, tip) = (pair(looked), pair(tip));
            let at = TipState { fast: Some((looked.clone(), "l".into())), running: Some((60_000, tip.clone())), ..Default::default() };
            let from = show.clone();
            let ahead = |p: &Pair| p.work > from.work && p.main > from.main;
            let (to, took) = proven(app, false, &[], &show, &tip, &at, &differs, &ahead, &none).unwrap();
            assert_eq!((&to, took), (&looked, Some(Vec::new())), "the pair the run proved, not the tip");
            show = to;
        }
        let ahead = |p: &Pair| p.work > show.work && p.main > show.main;
        for looked in ['c', 'd'] {
            let at = TipState { fast: Some((pair(looked), "l".into())), ..Default::default() };
            let e = proven(app, false, &[], &show, &pair('e'), &at, &differs, &ahead, &none).unwrap_err();
            assert!(e.0.starts_with("waits for the fast bucket") && !e.1, "{e:?}");
        }
        let red = TipState { fast: Some((pair('e'), "l".into())), red: BTreeMap::from([("check makepad-foo".to_string(), "rc-x (LANE-X) #7".to_string())]), ..Default::default() };
        let e = proven(app, false, &[], &show, &pair('f'), &red, &differs, &ahead, &|u: &[String]| Ok(u.to_vec())).unwrap_err();
        assert!(e.0.starts_with("has check makepad-foo failing") && e.1, "{e:?}");
        let green = TipState { fast: Some((pair('e'), "l".into())), ..Default::default() };
        assert_eq!(proven(app, false, &[], &show, &pair('e'), &green, &differs, &ahead, &none), Ok((pair('e'), Some(Vec::new()))));
    }

    #[cfg(unix)]
    #[test]
    fn a_stopped_shows_process_is_reaped() {
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        CHILDREN.lock().unwrap().get_or_insert_with(HashMap::new).insert("reap-test".into(), child);
        // no show follows it any more (stopped): once it exited, a tick waits for it
        let t = std::time::Instant::now();
        while CHILDREN.lock().unwrap().as_ref().is_some_and(|m| m.contains_key("reap-test")) {
            assert!(t.elapsed() < Duration::from_secs(10), "pid {pid} was not reaped");
            reap(&[]);
            std::thread::sleep(Duration::from_millis(20));
        }
        // gone, not a zombie (a zombie still answers signal 0)
        assert!(!crate::os::pid_alive(pid), "pid {pid} is still there");
    }

    /// A show whose JIT holds landed edits for a restart says so, since the sha it was listed at
    /// when the first came, with what the oldest is; the person's restart clears it.
    #[test]
    fn a_show_says_which_landed_edits_wait_for_its_restart() {
        let first = "1 files changed: band.rs: the type alias `Found` changed; restart `makepad run makepad-fab` (or `makepad inspect <app> session relaunch`) to pick them up; keeping the running code";
        use crate::explorer::pending_said;
        assert_eq!(pending_said(first), "1 files changed: band.rs: the type alias `Found` changed");
        assert_eq!(pending_said("2 files changed (a.rs, b.rs): a static's type changed; needs a JIT rebuild: restart it through cargo"), "2 files changed (a.rs, b.rs): a static's type changed");
        let mut x = Show { sha: "b518cd2ffb0123".into(), ..Default::default() };
        let (line, since) = restart_now(&x, 1, first);
        assert_eq!(line, "restart pending since b518cd2ffb: 1 landed edit not running (first: 1 files changed: band.rs: the type alias `Found` changed)");
        (x.restart, x.restart_since) = (line, since);
        // a later landing moves its sha: the since stays, the count grows
        x.sha = "c0ffee0000aaaa".into();
        let (line, since) = restart_now(&x, 2, first);
        assert!(line.starts_with("restart pending since b518cd2ffb: 2 landed edits not running (first: "), "{line}");
        assert_eq!(since, "b518cd2ffb0123");
        assert_eq!(restart_now(&x, 0, ""), (String::new(), String::new()), "restarted: nothing waits");
        assert_eq!(restart_now(&x, 1, "").0, "restart pending since b518cd2ffb: 1 landed edit not running", "a JIT that names none");
    }

    #[test]
    fn a_close_is_the_persons_only_when_its_log_says_a_key_or_click_did_it() {
        let end = "\n[I] platform/src/event/hal_intake.rs:620:21 - hal: the last window closed (\"Scope\")\n";
        let key = format!("makepad-hal: window 0 asked to close (windowShouldClose:): key down \"w\" (code 13) +cmd{end}");
        assert!(close_cause(&key).0, "Cmd+W");
        let button = format!("makepad-hal: window 0 asked to close (windowShouldClose:): mouse event 2 in window 77 at (12, 900), on its close button{end}");
        assert!(close_cause(&button).0);
        let dock = format!("makepad-hal: asked to quit (applicationShouldTerminate:): sender NSApplication, no event (a call from code), Apple Event 'aevtquit' from pid 600 (Dock){end}");
        assert!(close_cause(&dock).0, "the Dock's Quit");
        let code = format!("makepad-hal: window 0 asked to close (windowShouldClose:): no event (a call from code){end}");
        assert!(!close_cause(&code).0);
        let app = format!("[I] platform/src/window.rs:868:13 - window 0: the app closes it at apps/x/src/app.rs:40:9{end}");
        assert_eq!(close_cause(&app), (false, "[I] platform/src/window.rs:868:13 - window 0: the app closes it at apps/x/src/app.rs:40:9".to_string()));
        let appkit = format!("makepad-hal: window 0 closes without the app's close or a close request (windowWillClose:): event type 13{end}");
        assert!(!close_cause(&appkit).0);
        assert_eq!(close_cause(end), (false, "its log names no close or quit".to_string()), "an old build's log");
        // the run's end lines (11-06): the app's session closing its window and the app's exit
        // follow the cause; a quit signal, and the host closing the window it lent, are causes
        let ran = "makepad-hal: window 0 closes: session 1 closed it\nmakepad-run: the run ends: the app exited (0)\n";
        assert!(close_cause(&format!("makepad-hal: window 0 asked to close (windowShouldClose:): key down \"w\" (code 13) +cmd\n{ran}")).0, "Cmd+W, then the app's close");
        let signal = "makepad-hal: window 0 closes: session 0 (the host) closed it\nmakepad-run: the run ends: SIGTERM from pid 812 (cmd x, its parent 640: cmd y)\n";
        assert_eq!(close_cause(signal), (false, "makepad-run: the run ends: SIGTERM from pid 812 (cmd x, its parent 640: cmd y)".to_string()));
        assert_eq!(close_cause("makepad-hal: window 0 closes: session 0 (the host) closed it\n").1, "makepad-hal: window 0 closes: session 0 (the host) closed it");
        assert_eq!(close_cause(ran).1, "its log names no close or quit");
        // a caption's close button (a node's WM, app-drawn captions): the window system's input
        // is the person's click, an injected one an agent's; the app's close request after it is
        // that click's
        let asks = "[I] widgets/core/src/window.rs:1184:32 - window 0: the app asks to close it at widgets/core/src/window.rs:1184:32\n";
        let person = format!("[I] platform/src/window.rs:900:9 - window 0: its caption's close button: the window system's input\n{asks}{ran}");
        assert_eq!(close_cause(&person), (true, "[I] platform/src/window.rs:900:9 - window 0: its caption's close button: the window system's input".to_string()));
        let agent = format!("[I] platform/src/window.rs:900:9 - window 0: its caption's close button: a click injected by agent A1, pid 77\n{asks}");
        assert!(!close_cause(&agent).0 && close_cause(&agent).1.contains("injected by agent A1"));
        assert_eq!(close_cause(asks).0, false, "the app's own request, no click before it");
        // the run closing the window it lent names why
        let run = "makepad-run: the run's window closes: the app exited (0)\nmakepad-hal: window 0 closes: session 0 (the host) closed it\n";
        assert_eq!(close_cause(run), (false, "makepad-run: the run's window closes: the app exited (0)".to_string()));
        let after = format!("makepad-hal: window 0 asked to close (windowShouldClose:): key down \"w\" (code 13) +cmd\n{ran}makepad-run: the run's window closes: the app exited (0)\n");
        assert!(close_cause(&after).0, "the run closing its window after the person's close says nothing of who closed it");
        // a Mac window's close button, then the app's quit (or its last window closing), the
        // run's end and the host's close (Stage and Synth, 11-06): the click's
        let click = "makepad-hal: window 0 asked to close (windowShouldClose:): mouse event 2 in window 807 at (16, 779), on its close button";
        let stage = format!("{click}\n[I] platform/src/cx_api.rs:1480:9 - the app quits: Cx::quit at apps/x/src/product/mod.rs:1088:16\n[I] platform/src/os/hal/event_loop.rs:396:17 - hal: the app quit\nmakepad-run: the UI waits on the JIT (504 ms): the load screen fades in\nmakepad-run: after 793 ms the load screen gives way to the run's view\nmakepad-run: the run ends: the app exited (0)\nmakepad-hal: window 0 closes: session 0 (the host) closed it\nmakepad-run domain: 1: retaining mapping until thread exit: domain-1-x\n");
        assert_eq!(close_cause(&stage), (true, click.to_string()), "Stage: the click, not the host's close");
        let synth = format!("{click}\n[I] platform/src/event/hal_intake.rs:621:21 - hal: the last window closed (\"Synth\")\nmakepad-run: the run ends: the app exited (0)\nmakepad-hal: window 0 closes: session 0 (the host) closed it\n");
        assert_eq!(close_cause(&synth), (true, click.to_string()), "Synth: the click, not the host's close");
        // the app's own quit, no request before it: the app's; the host's close before the run ends: the host's
        let quit = "[I] platform/src/os/hal/event_loop.rs:396:17 - hal: the app quit\nmakepad-run: the run ends: the app exited (0)\nmakepad-hal: window 0 closes: session 0 (the host) closed it\n";
        assert_eq!(close_cause(quit), (false, "[I] platform/src/os/hal/event_loop.rs:396:17 - hal: the app quit".to_string()));
        let host = format!("{click}\nmakepad-hal: window 0 closes: session 0 (the host) closed it\n");
        assert_eq!(close_cause(&host).1, "makepad-hal: window 0 closes: session 0 (the host) closed it", "no run's end between: the host's own close");
    }

    #[test]
    fn a_show_waits_for_its_landing_is_one_per_device_and_stops() {
        let home = std::env::temp_dir().join(format!("devhub-show-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let repo = home.join("makepad");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap().status.success(), "git {args:?}");
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "user.name", "t"]);
        g(&["commit", "-q", "--allow-empty", "-m", "base"]);
        g(&["branch", "rc-x"]);
        std::fs::create_dir_all(repo.join("apps/foo")).unwrap();
        std::fs::write(repo.join("apps/foo/Cargo.toml"), "[package]\nname = \"makepad-foo\"\n").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-q", "-m", "foo"]);
        let leases = Arc::new(LeaseMaster::new("mac", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let ctx = Arc::new(Ctx { store: Store::open(&home, "mac", Arc::new(|_| 0)).unwrap(), leases, machine: "mymac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: repo.clone(), home: home.clone() });
        let add = |on: &str| serve(&ctx, &format!("show add {{\"app\":\"makepad-foo\",\"on\":\"{on}\",\"after\":[\"landed\",\"rc-x\"],\"by\":\"t\"}}"));
        // a name no Cargo.toml has starts nothing (`devhub show 518`)
        assert!(serve(&ctx, "show add {\"app\":\"518\",\"on\":\"mac\",\"by\":\"t\"}").unwrap_err().contains("no package `518`"));
        let r = add("mymac").unwrap();
        assert!(r.contains("\"device\":\"mac\"") && r.contains("\"state\":\"waiting\""), "{r}");
        assert!(add("nowhere").unwrap_err().contains("no device nowhere"));
        // a second show of the same app on the same device replaces a waiting one
        add("mac").unwrap();
        assert_eq!(ctx.store.read(|d| d.shows.iter().map(|x| x.state.clone()).collect::<Vec<_>>()), vec!["replaced", "waiting"]);
        // a running one is never replaced silently (unsaved work)
        // (a live pid: show looks first, and one that ended is not running)
        ctx.store.update(|d| {
            d.shows[1].state = "running".into();
            d.shows[1].pid = std::process::id();
        }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
        // a repeat show asks it to come to the front (this test's process has no JIT link to answer);
        // one with other arguments is a different run, which only `--replace` starts
        assert!(add("mac").unwrap_err().contains("could not come to the front"));
        assert!(serve(&ctx, "show add {\"app\":\"makepad-foo\",\"on\":\"mac\",\"args\":[\"x\"],\"by\":\"t\"}").unwrap_err().contains("--replace"));
        ctx.store.update(|d| {
            d.shows[1].state = "waiting".into();
            d.shows[1].pid = 0;
        }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
        assert!(serve(&ctx, "show stop makepad-foo mac").unwrap().contains("stopped"));
        assert_eq!(ctx.store.read(|d| d.shows[1].state.clone()), "cancelled");
        // what became of the recent ones stays listed (its outcome), not only the running ones
        let list = serve(&ctx, "show list").unwrap();
        assert!(list.contains("\"state\":\"cancelled\"") && list.contains("\"state\":\"replaced\""), "{list}");
        ctx.store.update(|d| d.shows.iter_mut().for_each(|x| {
            x.ended_ms = 1;
            x.added_ms = 1;
        })).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
        assert_eq!(serve(&ctx, "show list").unwrap(), "[]");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A shown app's failure is said once, across a re-exec of the master between two looks (each
    /// landing re-execs it): what its log was read to and what was said, of the log and of the
    /// app's records, is in the show's record, not in the process. A panic of the morning was said
    /// again at about 20 shows after (11-06).
    #[test]
    fn a_failure_is_said_once_across_a_master_re_exec() {
        use std::io::Write as _;
        let base = std::env::temp_dir().join(format!("devhub-show-reexec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let log = base.join("app.log");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&log, "starting\nthread 'main' panicked at src/a.rs:10:5:\nboom\n").unwrap();
        let said: Arc<Mutex<Vec<String>>> = Default::default();
        // a master on `home`: its state as the disk holds it
        let master = |home: &std::path::Path| {
            let said = said.clone();
            let store = Store::open(home, "mac", Arc::new(move |r: Record| { if r.kind == "show_failed" { said.lock().unwrap().push(r.line.clone()); } 0 })).unwrap();
            let leases = Arc::new(LeaseMaster::new("mac", home.to_path_buf(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
            Arc::new(Ctx { store, leases, machine: "mymac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: home.to_path_buf(), home: home.to_path_buf() })
        };
        let count = || said.lock().unwrap().len();
        let first = base.join("first");
        let ctx = master(&first);
        ctx.store.update(|d| d.shows.push(Show { id: "V1".into(), app: "makepad-foo".into(), device: "mac".into(), state: "running".into(), pid: 4242, log: log.to_string_lossy().into(), launch_ms: 1, ..Default::default() })).unwrap();
        let show = |ctx: &Ctx| ctx.store.read(|d| d.shows[0].clone());
        let record = |seq: u64, clock: u64| Record { id: format!("mymac.4242.{seq}.{clock}"), at_ms: 1, machine: "mymac".into(), app: "makepad-foo".into(), pid: 4242, kind: "scripterr".into(), sev: Sev::Warn, line: "scripterr: x not found".into(), detail: format!("{{\"event\":\"scripterr\",\"seq\":{seq},\"pid\":4242,\"at_ms\":{clock},\"error\":\"x not found\"}}"), untrusted: false };
        watch_log(&ctx, &show(&ctx));
        on_record(&ctx, &record(1, 5120));
        assert_eq!(count(), 2, "{:?}", said.lock().unwrap());
        // (the record holds it: no process keeps it)
        let x = show(&ctx);
        assert_eq!((x.log_read, x.log_said.len(), x.events_said.as_slice()), (std::fs::metadata(&log).unwrap().len(), 1, &["mymac.4242.1.5120".to_string()][..]));
        // the re-exec: a new process, with nothing but what the disk holds
        drop(ctx);
        let second = base.join("second");
        std::fs::create_dir_all(&second).unwrap();
        for f in ["state.json", "state.journal"] {
            if first.join(f).exists() { std::fs::copy(first.join(f), second.join(f)).unwrap(); }
        }
        let ctx = master(&second);
        watch_log(&ctx, &show(&ctx));
        on_record(&ctx, &record(1, 5120));
        assert_eq!(count(), 2, "nothing is said again: {:?}", said.lock().unwrap());
        // the same kind of line again (numbers apart) stays quiet; a new kind and a run started
        // again in place (its seq from 1 again, on a new clock) are said
        std::fs::OpenOptions::new().append(true).open(&log).unwrap().write_all(b"thread 'main' panicked at src/a.rs:12:9:\nerror: a new kind\n").unwrap();
        watch_log(&ctx, &show(&ctx));
        on_record(&ctx, &record(1, 830));
        assert_eq!(count(), 4, "{:?}", said.lock().unwrap());
        assert!(said.lock().unwrap()[2].contains("error: a new kind"), "{:?}", said.lock().unwrap());
        drop(ctx);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A person's unpin is the show's pin from then on: a tick that looked at the show before it
    /// (pinned then) does not pin its JIT again, so no landing reads the JIT's pin as a pin; and a
    /// master restart reads it as unpinned. A pinned show's JIT that lost its pin (it restarted) is
    /// told again, and a pin set in the JIT itself is a pin at the next landing. (The user's Fab
    /// was unpinned and pinned again by the next landing, 11-07.)
    #[test]
    fn an_unpin_holds_against_an_earlier_look_and_a_master_restart() {
        let base = std::env::temp_dir().join(format!("devhub-show-unpin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let master = |home: &std::path::Path| {
            std::fs::create_dir_all(home).unwrap();
            let store = Store::open(home, "mac", Arc::new(|_| 0)).unwrap();
            let leases = Arc::new(LeaseMaster::new("mac", home.to_path_buf(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
            Arc::new(Ctx { store, leases, machine: "mymac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: home.to_path_buf(), home: home.to_path_buf() })
        };
        // the show's JIT: its pin, as `inspect pin [on|off]` sets and says it
        let jit = Arc::new(Mutex::new(true));
        let j = jit.clone();
        let ask = move |_: &Show, line: &str| -> Result<Vec<String>, String> {
            let mut pinned = j.lock().unwrap();
            match line {
                "pin on" => *pinned = true,
                "pin off" => *pinned = false,
                "pin" => {}
                other => return Err(format!("no `{other}` here")),
            }
            Ok(vec![format!("{{\"pinned\":{}}}", *pinned)])
        };
        let first = base.join("first");
        let ctx = master(&first);
        ctx.store.update(|d| d.shows.push(Show { id: "V1".into(), app: "makepad-foo".into(), device: "mac".into(), state: "running".into(), pid: 4242, pinned: true, launch_ms: 1, ..Default::default() })).unwrap();
        let show = |ctx: &Ctx| ctx.store.read(|d| d.shows[0].clone());
        // the tick's look, then the person's unpin, then the tick's pin step with a landing
        let looked = show(&ctx);
        assert!(pin_show(&ctx, &looked, false, &ask).unwrap().contains("\"pinned\":false"));
        let x = keep_pin(&ctx, looked, true, &ask);
        assert!(!x.pinned && !*jit.lock().unwrap(), "the unpin holds: the show and its JIT");
        // the re-exec: a new process, with nothing but what the disk holds
        drop(ctx);
        let second = base.join("second");
        std::fs::create_dir_all(&second).unwrap();
        for f in ["state.json", "state.journal"] {
            if first.join(f).exists() { std::fs::copy(first.join(f), second.join(f)).unwrap(); }
        }
        let ctx = master(&second);
        assert!(!show(&ctx).pinned, "a master restart reads the unpin");
        let x = keep_pin(&ctx, show(&ctx), true, &ask);
        assert!(!x.pinned && !*jit.lock().unwrap(), "and its next landing leaves it unpinned");
        // a pin set in the JIT itself: a pin at the next landing (not before one)
        *jit.lock().unwrap() = true;
        assert!(!keep_pin(&ctx, show(&ctx), false, &ask).pinned);
        assert!(keep_pin(&ctx, show(&ctx), true, &ask).pinned);
        // a pinned show's JIT that restarted (its pin gone) is told again
        *jit.lock().unwrap() = false;
        assert!(keep_pin(&ctx, show(&ctx), false, &ask).pinned && *jit.lock().unwrap());
        drop(ctx);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn app_names_match_their_packages() {
        assert!(super::same_app("foo", "makepad-foo"));
        assert!(super::same_app("makepad-app-calculator", "makepad-app-calculator"));
        assert!(super::same_app("calculator", "makepad-app-calculator"));
        assert!(!super::same_app("bar", "makepad-foo"));
    }
}

