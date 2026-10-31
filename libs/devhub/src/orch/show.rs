//! `devhub show <app> [--on mac|<node>|quest] [--after landed <branch>]`: the latest landed build of
//! an app on one of the user's display devices, tracked like a job: one instance per app per
//! device, its pid, the landed commit it runs, a parked panic. Devices:
//! - `mac` (the default): the master's own launch, `cargo run --profile host -p makepad-run --bin
//!   makepad -- run <app>` in the checkout (work: the latest landed; the JIT patches landings live);
//! - a WM node: its dock app (the node's synced checkout is the latest landed);
//! - `quest`: the JIT host built for the Quest on the Mac (`cargo makepad android build-host
//!   --quest`), `adb install -r` and `am start` (never adb tcpip/usb/connect/disconnect/kill-server).
//!
//! A show on the Mac is gated: before it reaches the user, devhub boots the app at the exact
//! landed pair headless under the JIT on a box of the same OS (a Mac node, mini), keeps it up for the
//! settle window ([`SETTLE`]) and requires no panic, pause, hang, deadlock, patch or script error
//! on any thread and no early exit. Only then does it launch on the user's Mac. A failed pre-boot
//! never reaches the user: it is a `show_blocked` event (the inbox: the manager, and the lane that
//! asked, by name). A pair that passed is not booted again for later shows of that app.
//!
//! A show that is running is never replaced silently: it may hold unsaved work, so `show` refuses
//! and says so (ask the user, then `--replace`). When the user closes the app (its process ends
//! with exit 0) the show is done and keeps only how long it ran and which commit; a crash (a
//! nonzero exit, a signal) and a panic-pause are reported, not taken for a close.

use super::{git, Ctx};
use crate::proto::{wall_ms, Record, Sev};
use crate::state::{Cond, JobPanic, Show};
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

fn event(ctx: &Ctx, sev: Sev, line: String, x: Option<&Show>) {
    ctx.store.event("show", sev, line, x.map(|x| crate::state::show_value(x).to_json()));
}

/// A failure in a shown app (a panic, a hang, a patch or compile error, a crash): a `show_failed`
/// event, so the manager hears it without the user (`devhub watch show --until fail`).
fn failed(ctx: &Ctx, x: &Show, what: String) {
    ctx.store.event("show_failed", Sev::Err, format!("show_failed {} on {}: {what}", x.app, x.device), Some(crate::state::show_value(x).to_json()));
}

fn set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Show)) -> Option<Show> {
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

/// The JIT's app name for a package (`makepad-app-fab` and `makepad-fab` are `fab`).
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
    });
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
    });
    for x in again {
        event(&ctx, Sev::Info, format!("show {} on {}: the master restarted during its launch; starting again", x.app, x.device), Some(&x));
        let c = ctx.clone();
        let _ = std::thread::Builder::new().name("devhub-show".into()).spawn(move || launch(&c, &x));
    }
    let _ = std::thread::Builder::new().name("devhub-shows".into()).spawn(move || {
        let mut work = git(&ctx.repo, &["rev-parse", "refs/heads/work"]).unwrap_or_default();
        loop {
            tick(&ctx, &mut work);
            ctx.store.wait(TICK);
        }
    });
}

fn tick(ctx: &Arc<Ctx>, work: &mut String) {
    let shows: Vec<Show> = ctx.store.read(|d| d.shows.iter().filter(|x| x.active()).cloned().collect());
    // a landing while a show runs: the JIT patches it live; said once per landing
    let now_work = git(&ctx.repo, &["rev-parse", "refs/heads/work"]).unwrap_or_default();
    let landed = !now_work.is_empty() && now_work != *work;
    if landed {
        *work = now_work.clone();
    }
    let mut ps: HashMap<String, Option<crate::explorer::MachineRow>> = HashMap::new();
    for x in shows {
        match x.state.as_str() {
            "waiting" => {
                let (met, cond) = super::after::cond_met(ctx, &x.cond, x.added_ms);
                if cond != x.cond {
                    set(ctx, &x.id, |s| s.cond = cond.clone());
                }
                if let Some(why) = met {
                    let Some(x) = set(ctx, &x.id, |s| {
                        s.state = "starting".into();
                        s.note = why.clone();
                    }) else { continue };
                    event(ctx, Sev::Info, format!("show {} on {}: {why}; starting", x.app, x.device), Some(&x));
                    let c = ctx.clone();
                    let _ = std::thread::Builder::new().name("devhub-show".into()).spawn(move || launch(&c, &x));
                }
            }
            "running" | "paused" => {
                // pinned: an operation runs on it; the JIT holds still and is told so again after a restart
                if x.pinned && x.pid > 0 {
                    let _ = pin_jit(ctx, &x, true);
                }
                let x = if landed && x.device != "quest" && !x.pinned {
                    let x = set(ctx, &x.id, |s| s.sha = now_work.clone()).unwrap_or(x);
                    event(ctx, Sev::Info, format!("show {} on {}: work moved to {}; the JIT patches it live (`devhub show {} --on {} --replace` restarts it)", x.app, x.device, super::short(&now_work), x.app, x.device), Some(&x));
                    x
                } else {
                    x
                };
                if x.device == "mac" {
                    mac_watch(ctx, &x);
                } else {
                    let row = ps.entry(x.device.clone()).or_insert_with(|| crate::cli::ask(&ctl(), Some(&x.device), "ps", "ps").ok().map(|l| crate::explorer::parse_ps(&x.device, &l)));
                    remote_watch(ctx, &x, row.as_ref());
                }
            }
            _ => {}
        }
    }
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

/// The end of a show: exit 0 is the user's close (the show frees itself), anything else a crash.
fn ended(ctx: &Ctx, x: &Show, code: Option<i32>) {
    let closed = code == Some(0) && x.state != "paused";
    let ran = wall_ms().saturating_sub(x.started_ms) / 1000;
    let x = set(ctx, &x.id, |s| {
        if s.active() {
            s.state = if closed { "closed" } else { "crashed" }.into();
        }
        s.exit = code;
        s.ended_ms = wall_ms();
        if closed {
            s.note = format!("closed by the user after {ran} s at {}", super::short(&s.sha));
            s.pid = 0;
        }
    })
    .unwrap_or_else(|| x.clone());
    if closed {
        event(ctx, Sev::Info, format!("show {} on {} ended: closed by the user after {ran} s (ran {})", x.app, x.device, super::short(&x.sha)), Some(&x));
    } else if x.state == "crashed" {
        // its first error lines (a compile error stops the launch before any window)
        let errors: Vec<String> = std::fs::read_to_string(&x.log).map(|t| crate::land::failure_lines(&t)).unwrap_or_default().into_iter().take(6).collect();
        event(ctx, Sev::Err, format!("show {} on {} CRASHED after {ran} s (exit {}; log {})", x.app, x.device, code.map_or("signal".into(), |c| c.to_string()), x.log), Some(&x));
        failed(ctx, &x, format!("crashed (exit {}){}", code.map_or("signal".into(), |c| c.to_string()), if errors.is_empty() { String::new() } else { format!(": {}", errors.join(" | ")) }));
    }
}

/// A node or Quest show: its JIT in the device's `ps` (gone = ended; the device's hub says no more).
fn remote_watch(ctx: &Ctx, x: &Show, row: Option<&crate::explorer::MachineRow>) {
    let Some(row) = row.filter(|r| r.err.is_none()) else { return };
    let jit = row.jits.iter().find(|j| (x.pid != 0 && j.pid == x.pid) || (x.pid == 0 && same_app(&j.app, &x.app)));
    match jit {
        Some(j) if x.pid == 0 => {
            set(ctx, &x.id, |s| {
                s.pid = j.pid;
                s.state = "running".into();
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
    let path = dir.join(format!("{}-{}.log", x.app, x.device));
    let f = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((path, f))
}

/// Starts the show on its device.
fn launch(ctx: &Ctx, x: &Show) {
    set(ctx, &x.id, |s| s.launch_ms = wall_ms());
    let sha = git(&ctx.repo, &["rev-parse", "refs/heads/work"]).unwrap_or_default();
    let main = git(&ctx.commercial(), &["rev-parse", "refs/heads/main"]).unwrap_or_default();
    // the user's Mac sees only a build a devbox booted cleanly at these commits
    if x.device == "mac" {
        if let Err(e) = preboot(ctx, x, &sha, &main) {
            let x = set(ctx, &x.id, |s| {
                s.state = "blocked".into();
                s.ended_ms = wall_ms();
                s.note = crate::proto::clean(&e, 300);
            });
            let by = x.as_ref().map_or(String::new(), |x| x.by.clone());
            ctx.store.event(
                "show_blocked",
                Sev::Err,
                format!("show_blocked {} at {},{}: {e} (not shown on the Mac{})", x.as_ref().map_or("", |x| &x.app), super::short(&sha), super::short(&main), if by.is_empty() { String::new() } else { format!("; for {by}") }),
                x.as_ref().map(|x| crate::state::show_value(x).to_json()),
            );
            return;
        }
    }
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
            let x = set(ctx, &x.id, |s| {
                s.state = "running".into();
                s.pid = pid;
                s.sha = sha.clone();
                s.started_ms = wall_ms();
                s.log = log;
            });
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
    let tree = |dir: &std::path::Path, at: &str, p: &str| git(dir, &["rev-parse", &format!("{at}:{p}")]).unwrap_or_default();
    for p in ["Cargo.toml", "Cargo.lock", ".cargo/config.toml"] {
        text.push_str(&tree(&ctx.repo, work, p));
    }
    for d in dirs {
        text.push_str(d);
        text.push_str(&match d.strip_prefix("apps/commercial/") {
            Some(c) => tree(&ctx.commercial(), main, c),
            None => tree(&ctx.repo, work, d),
        });
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
/// and the JIT's unchanged since a passed boot: that pass counts. The JIT's sources unchanged since
/// a box built them: its stashed binary runs, no build. Each stage's time goes to `devhub shows`.
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
    set(ctx, &x.id, |s| s.note = format!("pre-boot at {},{} on a macOS devbox", super::short(work), super::short(main)));
    event(ctx, Sev::Info, format!("show {} on mac: pre-boot at {},{} on a macOS devbox first", x.app, super::short(work), super::short(main)), Some(x));
    let rev = if main.is_empty() { format!("work@{work}") } else { format!("work@{work},main@{main}") };
    let spec = super::leg::Spec { only: false, lane: "show-preboot".into(), agent: "devhub-show".into(), kind: crate::lease::Kind::Test, needs: "os=macos".into(), prio: SHOW_PRIO, rev, repo_dir: ctx.repo.to_string_lossy().into_owned(), node: None };
    // a box that refuses the start (its tree still held for another lease) is no verdict on the
    // app: ask again (another box, or the same once it is free)
    let mut tries = 0;
    let held = loop {
        match super::leg::Held::acquire(&spec, &mut |_| {}, &|| false) {
            Ok(h) => break h,
            Err(e) if e.contains("refused") && tries < 5 => {
                tries += 1;
                std::thread::sleep(Duration::from_secs(10));
            }
            Err(e) => return Err(format!("no macOS devbox for the pre-boot: {e}")),
        }
    };
    let node = held.node.clone();
    stage(format!("box {node} {} s", began.elapsed().as_secs()));
    let log = ctx.logs("show").join(format!("{}-preboot.log", x.id));
    let _ = std::fs::create_dir_all(ctx.logs("show"));
    let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    // the JIT of these sources: stashed on the box when it built them before (a tree that moved
    // away and back has new mtimes, and cargo builds 16 crates again)
    let stash = format!("target/host/makepad.jit-{jit_key}");
    let t = std::time::Instant::now();
    let have = held.output(&["test", "-x", &stash]).is_ok_and(|(c, _)| c == 0);
    if have {
        stage("jit reused".into());
    } else {
        let build: Vec<String> = ["cargo", "build", "--profile", "host", "-p", "makepad-run", "--bin", "makepad"].iter().map(|s| s.to_string()).collect();
        let mut out = file.try_clone().map_err(|e| e.to_string())?;
        match held.run(&build, &mut out) {
            Ok(0) => {}
            Ok(c) => {
                let _ = crate::cli::ask(&ctl(), None, "release", &held.id);
                return Err(format!("the JIT's build on {node} failed (exit {c}); {}", log.display()));
            }
            Err(e) => {
                let _ = crate::cli::ask(&ctl(), None, "release", &held.id);
                return Err(format!("the JIT's build on {node}: {e}"));
            }
        }
        // one stash per box: this one replaces the last
        let _ = held.output(&["find", "target/host", "-maxdepth", "1", "-name", "makepad.jit-*", "-delete"]);
        let _ = held.output(&["cp", "-p", "target/host/makepad", &stash]);
        stage(format!("jit build {} s", t.elapsed().as_secs()));
    }
    let boot = std::time::Instant::now();
    let argv: Vec<String> = ["env", "MAKEPAD_HIDE_WINDOWS=1", &stash, "run", &x.app].iter().map(|s| s.to_string()).collect();
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
    let bad: Vec<String> = t.lines().filter(|l| l.contains("panicked at") || l.contains("PANIC") || l.trim_start().starts_with("error") || l.contains("RC error") || l.contains("script error")).take(4).map(|l| crate::proto::clean(l, 200)).collect();
    bad.join(" | ")
}

/// The master's own launch on the Mac, detached in its own process group.
fn mac_launch(ctx: &Ctx, x: &Show) -> Result<(u32, String), String> {
    let (path, out) = log_file(ctx, x)?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = makepad_remote::group::command(cargo());
    cmd.args(["run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "run", &x.app]).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").stdin(std::process::Stdio::null()).stdout(out).stderr(err);
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
    set(ctx, &x.id, |s| s.note = "building the Quest JIT host on the Mac".into());
    let mut build = makepad_remote::group::command("nice");
    build.args(["-n", "10"]).arg(cargo()).args(["run", "--release", "-p", "cargo-makepad", "--", "android", "--abi=aarch64", "build-host", "--app", &x.app, "--devhub-name=quest", "--quest"]).current_dir(&ctx.repo).env("CARGO_INCREMENTAL", "0").env("CARGO_BUILD_JOBS", "4");
    run(&mut build, "the Quest build")?;
    let name = x.app.replace('-', "_");
    let apk = ctx.repo.join(format!("target/makepad-android-apk/{name}/apk/{name}.apk"));
    run(makepad_remote::group::command("adb").args(["-s", &serial, "install", "--no-incremental", "-r"]).arg(&apk), "adb install")?;
    run(makepad_remote::group::command("adb").args(["-s", &serial, "shell", "am", "start", "-n", "dev.makepad.run/dev.makepad.run.RunActivity"]), "adb am start")?;
    Ok((0, path.to_string_lossy().into_owned()))
}

/// Stops a show by its recorded pid (the Mac: its process group; a node or the Quest: the
/// device's own stop).
/// Tells the show's JIT its pin.
fn pin_jit(ctx: &Ctx, x: &Show, on: bool) -> Result<(), String> {
    let at = if x.device == "mac" { ctx.machine.as_str() } else { x.device.as_str() };
    let mut said = String::new();
    makepad_remote::hub::ask(&ctl(), &format!("{at}:{}", x.pid), "cmd", "", if on { "pin on" } else { "pin off" }, &mut |l| {
        said = l.to_string();
        true
    })?;
    if said.contains("\"pinned\"") {
        Ok(())
    } else {
        Err(format!("its JIT does not take a pin ({})", crate::proto::clean(&said, 160)))
    }
}

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
            let dev = device(ctx, &s("on"))?;
            let after: Vec<String> = v.get("after").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let cond = if after.is_empty() { Cond::None } else { super::after::parse_cond(ctx, &after.iter().map(String::as_str).collect::<Vec<_>>())? };
            let replace = v.get("replace").and_then(|x| x.as_bool()).unwrap_or(false);
            let pin = v.get("pin").and_then(|x| x.as_bool()).unwrap_or(false);
            // one instance per app per device; a running one may hold unsaved work
            // one that ended since the last look is not running
            if let Some(old) = ctx.store.read(|d| d.shows.iter().find(|x| x.active() && x.app == app && x.device == dev && x.device == "mac" && x.state == "running").cloned()) {
                mac_watch(ctx, &old);
            }
            if let Some(old) = ctx.store.read(|d| d.shows.iter().find(|x| x.active() && x.app == app && x.device == dev).cloned()) {
                if matches!(old.state.as_str(), "running" | "paused") && !replace {
                    return Err(format!("{app} already runs on {dev} ({}, pid {}, {} s): it may hold unsaved work; ask the user, then `devhub show {app} --on {dev} --replace`", old.state, old.pid, wall_ms().saturating_sub(old.started_ms) / 1000));
                }
                if matches!(old.state.as_str(), "running" | "paused") {
                    stop(&old)?;
                }
                set(ctx, &old.id, |s| {
                    s.state = "replaced".into();
                    s.ended_ms = wall_ms();
                });
            }
            let x = ctx.store.update(|d| {
                d.shows_next += 1;
                let x = Show { id: format!("V{}", d.shows_next), app: app.clone(), device: dev.clone(), state: if cond == Cond::None { "starting" } else { "waiting" }.into(), cond: cond.clone(), added_ms: wall_ms(), by: s("by"), pinned: pin, ..Default::default() };
                d.shows.push(x.clone());
                x
            });
            ctx.store.kick();
            if x.state == "starting" {
                event(ctx, Sev::Info, format!("show {app} on {dev}: starting"), Some(&x));
                // the launch runs on its own thread (a Quest build takes minutes)
                let (c, x2) = (ctx.clone(), x.clone());
                let _ = std::thread::Builder::new().name("devhub-show".into()).spawn(move || launch(&c, &x2));
            } else {
                event(ctx, Sev::Info, format!("show {app} on {dev}: waiting for {}", cond.describe()), Some(&x));
            }
            Ok(format!("{{\"id\":{},\"app\":{},\"device\":{},\"state\":{}}}", json_str(&x.id), json_str(&app), json_str(&dev), json_str(&x.state)))
        }
        "stop" => {
            let mut w = rest.split_whitespace();
            let (Some(app), dev) = (w.next(), w.next().unwrap_or("mac")) else { return Err("show stop <app> [<device>]".into()) };
            let dev = device(ctx, dev)?;
            let x = ctx.store.read(|d| d.shows.iter().find(|x| x.active() && x.app == app && x.device == dev).cloned()).ok_or_else(|| format!("{app} is not shown on {dev}"))?;
            if matches!(x.state.as_str(), "running" | "paused") {
                stop(&x)?;
            }
            let x = set(ctx, &x.id, |s| {
                s.state = if s.state == "waiting" { "cancelled" } else { "stopped" }.into();
                s.ended_ms = wall_ms();
            });
            event(ctx, Sev::Info, format!("show {app} on {dev} stopped"), x.as_ref());
            Ok(format!("{{\"app\":{},\"device\":{},\"state\":\"stopped\"}}", json_str(app), json_str(&dev)))
        }
        "pin" => {
            let mut w = rest.split_whitespace();
            let (Some(app), Some(dev), Some(on)) = (w.next(), w.next(), w.next()) else { return Err("show pin <app> <device> on|off".into()) };
            let on = on == "on";
            let dev = device(ctx, dev)?;
            let x = ctx.store.read(|d| d.shows.iter().find(|x| x.active() && x.app == app && x.device == dev).cloned()).ok_or_else(|| format!("{app} is not shown on {dev}"))?;
            if x.pid > 0 {
                pin_jit(ctx, &x, on)?;
            }
            let x = set(ctx, &x.id, |s| s.pinned = on);
            event(ctx, Sev::Info, format!("show {app} on {dev}: {}", if on { "pinned: landings go on, it stays as it is until the unpin" } else { "unpinned: it catches up" }), x.as_ref());
            Ok(format!("{{\"app\":{},\"device\":{},\"pinned\":{on}}}", json_str(app), json_str(&dev)))
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
        let leases = Arc::new(LeaseMaster::new("mac", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let ctx = Arc::new(Ctx { store: Store::open(&home, "mac", Arc::new(|_| {})), leases, machine: "mymac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: repo.clone(), home: home.clone() });
        let add = |on: &str| serve(&ctx, &format!("show add {{\"app\":\"makepad-fab\",\"on\":\"{on}\",\"after\":[\"landed\",\"rc-x\"],\"by\":\"t\"}}"));
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
        });
        assert!(add("mac").unwrap_err().contains("--replace"));
        ctx.store.update(|d| {
            d.shows[1].state = "waiting".into();
            d.shows[1].pid = 0;
        });
        assert!(serve(&ctx, "show stop makepad-fab mac").unwrap().contains("stopped"));
        assert_eq!(ctx.store.read(|d| d.shows[1].state.clone()), "cancelled");
        // what became of the recent ones stays listed (its outcome), not only the running ones
        let list = serve(&ctx, "show list").unwrap();
        assert!(list.contains("\"state\":\"cancelled\"") && list.contains("\"state\":\"replaced\""), "{list}");
        ctx.store.update(|d| d.shows.iter_mut().for_each(|x| {
            x.ended_ms = 1;
            x.added_ms = 1;
        }));
        assert_eq!(serve(&ctx, "show list").unwrap(), "[]");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn app_names_match_their_packages() {
        assert!(super::same_app("fab", "makepad-fab"));
        assert!(super::same_app("makepad-app-calculator", "makepad-app-calculator"));
        assert!(super::same_app("calculator", "makepad-app-calculator"));
        assert!(!super::same_app("stage", "makepad-fab"));
    }
}
