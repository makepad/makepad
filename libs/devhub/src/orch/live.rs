//! `devhub live`: a long-running app on a box that edits hot-patch into, so an agent's loop is
//! edit → patch → first result in tens of ms with no relaunch. Built on jobs: a live job is
//! placed like one (the worktree's snapshot shipped on top of its base commits), then holds its
//! box with the app running under the JIT (headless, or visible on a screen box). Every file the
//! master's monitor sees saved in the worktree is shipped to the box as it comes (debounced
//! 30 ms; `live sync` ships the whole dirty set), the box's JIT patches it live, and the job
//! records the outcome: patched, a restart needed or a refusal (with the reason), and the time from
//! the save to the patch and to the first frame after it. `live act` runs any inspect command in
//! the app (ui, key, click, grab, test hooks); `live wait patched|settled|frame` blocks on the
//! JIT. A parked panic pauses the live job (act and resume through it). It ends on release, when
//! its app exits, or after 10 min with no edit or act. One live job per lane per app.

use super::leg::{Held, Spec};
use super::Ctx;
use crate::cli::ask;
use crate::lease::Kind;
use crate::proto::{wall_ms, Record, Sev};
use crate::state::{JobPanic, Live, LivePatch};
use makepad_remote::json_str;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A live job with no edit or act this long ends.
pub const IDLE_MS: u64 = 10 * 60 * 1000;
/// Saves closer together than this ship together.
const DEBOUNCE: Duration = Duration::from_millis(30);

/// A running live job's box and the edits waiting to go.
struct Running {
    held: Held,
    node: String,
    lease: String,
    tree: PathBuf,
    /// (paths in the tree, when the first was saved, when the last was)
    pending: Mutex<(BTreeSet<String>, u64, Option<Instant>)>,
    /// an edit is on its way (shipped, its verdict not in yet)
    inflight: std::sync::atomic::AtomicBool,
}

static RUNNING: Mutex<Option<HashMap<String, Arc<Running>>>> = Mutex::new(None);

fn running(id: &str) -> Option<Arc<Running>> {
    RUNNING.lock().unwrap().as_ref().and_then(|m| m.get(id).cloned())
}

fn event(ctx: &Ctx, sev: Sev, line: String, x: Option<&Live>) {
    ctx.store.event("live", sev, line, x.map(|x| crate::state::live_value(x).to_json()));
}

fn set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Live)) -> Option<Live> {
    ctx.store.update(|d| {
        let x = d.lives.iter_mut().find(|x| x.id == id)?;
        f(x);
        Some(x.clone())
    })
}

fn get(ctx: &Ctx, id: &str) -> Option<Live> {
    ctx.store.read(|d| d.lives.iter().find(|x| x.id == id).cloned())
}

fn ctl() -> makepad_remote::link::Client {
    makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-live")
}

/// One inspect command in the live app: its answer lines.
fn inspect(node: &str, pid: u32, line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    makepad_remote::hub::ask(&ctl(), &format!("{node}:{pid}"), "cmd", "", line, &mut |l| {
        out.push(l.to_string());
        true
    })?;
    Ok(out)
}

fn num(line: &str, k: &str) -> Option<u64> {
    makepad_strict_json::parse(line.as_bytes()).ok()?.get(k).and_then(crate::lease::num).map(|n| n as u64)
}

pub fn start(ctx: Arc<Ctx>) {
    // a previous master's live jobs go on: one whose lease this master took back (its own exec,
    // the same holder) keeps its app and is followed from where its log stands; else it is placed
    // again and its app starts again
    let lives: Vec<Live> = ctx.store.read(|d| d.lives.iter().filter(|x| x.active()).cloned().collect());
    for x in lives {
        let c = ctx.clone();
        let held = !x.lease.is_empty() && x.run_n > 0 && ctx.leases.view().iter().any(|l| l.id == x.lease && !matches!(l.state, crate::lease::sched::State::Ended(_)));
        event(&ctx, Sev::Info, format!("live {} ({}): the master restarted; {}", x.id, x.app, if held { "its app runs on" } else { "placed again, its app starts again" }), Some(&x));
        if held {
            let _ = std::thread::Builder::new().name("devhub-live".into()).spawn(move || reattach(&c, &x));
        } else {
            set(&ctx, &x.id, |s| {
                s.state = "starting".into();
                s.pid = 0;
                s.lease.clear();
                s.run_n = 0;
                s.touched_ms = wall_ms();
            });
            let id = x.id.clone();
            let _ = std::thread::Builder::new().name("devhub-live".into()).spawn(move || run(&c, &id));
        }
    }
    // saves in a live job's worktree queue for its box
    ctx.monitor.listen(Arc::new(|root: &Path, rel: &str| {
        let g = RUNNING.lock().unwrap();
        for r in g.as_ref().map(|m| m.values().cloned().collect::<Vec<_>>()).unwrap_or_default() {
            let path = if root == r.tree { rel.to_string() } else if root == r.tree.join("apps/commercial") { format!("apps/commercial/{rel}") } else { continue };
            if root == r.tree && rel.starts_with("apps/commercial/") {
                continue;
            }
            let mut p = r.pending.lock().unwrap();
            if p.0.is_empty() {
                p.1 = wall_ms();
            }
            p.0.insert(path);
            p.2 = Some(Instant::now());
        }
    }));
    let c = ctx.clone();
    let _ = std::thread::Builder::new().name("devhub-live-ship".into()).spawn(move || loop {
        let list: Vec<(String, Arc<Running>)> = RUNNING.lock().unwrap().as_ref().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        for (id, r) in list {
            let due = {
                let mut p = r.pending.lock().unwrap();
                match p.2 {
                    Some(t) if t.elapsed() >= DEBOUNCE => {
                        p.2 = None;
                        Some((std::mem::take(&mut p.0), p.1))
                    }
                    _ => None,
                }
            };
            if let Some((files, edit_ms)) = due {
                r.inflight.store(true, std::sync::atomic::Ordering::SeqCst);
                let (c2, id2) = (c.clone(), id.clone());
                let _ = std::thread::Builder::new().name("devhub-live-patch".into()).spawn(move || ship_edit(&c2, &id2, &r, files.into_iter().collect(), edit_ms));
            }
        }
        // the idle timeout, from its placement on (a job still queued for a box is not idle)
        let now = wall_ms();
        let idle: Vec<Live> = c.store.read(|d| d.lives.iter().filter(|x| x.active() && !x.lease.is_empty() && now.saturating_sub(x.touched_ms) >= IDLE_MS).cloned().collect());
        for x in idle {
            set(&c, &x.id, |s| s.note = "ended after 10 min with no edit or act".into());
            end_lease(&x.id);
        }
        std::thread::sleep(Duration::from_millis(10));
    });
}

fn end_lease(id: &str) {
    if let Some(r) = running(id) {
        let _ = ask(&ctl(), None, "release", &r.lease);
    }
}

/// The JIT's frame count from its `live` answer (a header line comes first).
fn frames_of(lines: &[String]) -> Option<u64> {
    lines.iter().find_map(|l| num(l, "frames"))
}

/// How long the JIT's watch waits for the edit's verdict.
const WATCH_S: u64 = 60;

/// Why an edit has no verdict: the watch ran out (the edit changed nothing the JIT compiled?), or
/// it came back early without one: what it said (an error, a closed link, another answer).
fn no_verdict(answer: &Result<Vec<String>, String>, waited: Duration) -> String {
    if waited.as_secs() + 1 >= WATCH_S {
        return format!("no patch event in {WATCH_S} s (the edit changed nothing the JIT compiled?)");
    }
    let said = match answer {
        Err(e) => format!("error: {e}"),
        Ok(l) if l.is_empty() => "no answer (the link closed?)".to_string(),
        Ok(l) => format!("answer: {}", l.join(" | ")),
    };
    format!("the JIT's watch came back after {} ms with no patch event ({})", waited.as_millis(), crate::proto::clean(&said, 300))
}

/// Ships saved files to the box; the JIT's verdict and the first frame after it are the result.
fn ship_edit(ctx: &Ctx, id: &str, r: &Running, files: Vec<String>, edit_ms: u64) {
    r.inflight.store(true, std::sync::atomic::Ordering::SeqCst);
    patch(ctx, id, r, files, edit_ms);
    r.inflight.store(false, std::sync::atomic::Ordering::SeqCst);
}

fn patch(ctx: &Ctx, id: &str, r: &Running, files: Vec<String>, edit_ms: u64) {
    let Some(x) = get(ctx, id) else { return };
    // the JIT's event count before: its patch event comes after
    let since = if x.pid > 0 { inspect(&r.node, x.pid, "events").ok().and_then(|l| l.iter().filter_map(|l| num(l, "seq")).max()).unwrap_or(0) } else { 0 };
    let frames0 = if x.pid > 0 { inspect(&r.node, x.pid, "live").ok().and_then(|l| frames_of(&l)).unwrap_or(0) } else { 0 };
    let (mut changed, mut deleted) = (Vec::new(), Vec::new());
    for f in files.iter().filter(|f| !crate::lease::ship::never_ships(f)) {
        match std::fs::symlink_metadata(r.tree.join(f)) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => changed.push(f.clone()),
            Err(_) => deleted.push(f.clone()),
        }
    }
    let shipped = (|| -> Result<(), String> {
        let mut entries = Vec::new();
        for p in &changed {
            crate::lease::ship::entries(&r.tree, p, &mut entries)?;
        }
        if !entries.is_empty() {
            crate::lease::ship::send(r.held.client(), &r.node, &r.lease, &r.tree, &entries)?;
        }
        let list = |v: &[String]| v.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(",");
        let body = format!("{{\"shipped\":[{}],\"deleted\":[{}]}}", list(&changed), list(&deleted));
        let a = ask(r.held.client(), Some(&r.node), "ship-done", &format!("ship-done {} {}", r.lease, makepad_network::digest::base64_encode(body.as_bytes())))?;
        match a.first() {
            Some(e) if e.starts_with("{\"err\":") => Err(e.clone()),
            _ => Ok(()),
        }
    })();
    let mut p = LivePatch { files: files.clone(), edit_ms, shipped_ms: wall_ms(), ..Default::default() };
    if let Err(e) = shipped {
        p.outcome = "failed".into();
        p.reason = crate::proto::clean(&e, 300);
        let x = set(ctx, id, |s| s.patch = Some(p));
        return event(ctx, Sev::Warn, format!("live {id}: shipping {} file(s) failed: {e}", files.len()), x.as_ref());
    }
    set(ctx, id, |s| {
        s.patch = Some(p.clone());
        s.touched_ms = wall_ms();
    });
    if x.pid == 0 {
        return;
    }
    // the JIT's verdict on the edit
    let asked = Instant::now();
    let answer = inspect(&r.node, x.pid, &format!("watch on=patchverified,patchrefused,patchreverted since={since} timeout_s={WATCH_S}"));
    let waited = asked.elapsed();
    // the line that answers (any header line before it is skipped)
    let ev = answer.as_ref().ok().and_then(|l| l.iter().find(|l| l.contains("\"event\"")).cloned()).unwrap_or_default();
    let v = makepad_strict_json::parse(ev.as_bytes()).ok();
    let s = |k: &str| v.as_ref().and_then(|v| v.get(k)).map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_json())).unwrap_or_default();
    p.outcome_ms = wall_ms();
    match s("event").as_str() {
        "patchverified" => p.outcome = "patched".into(),
        "patchrefused" => {
            p.outcome = if s("outcome") == "restart" { "restart" } else { "refused" }.into();
            p.reason = crate::proto::clean(&s("reasons"), 400);
        }
        "patchreverted" => {
            p.outcome = "reverted".into();
            p.reason = crate::proto::clean(&s("error"), 400);
        }
        _ => {
            p.outcome = "none".into();
            p.reason = no_verdict(&answer, waited);
        }
    }
    // the first frame after it
    if p.outcome == "patched" {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if inspect(&r.node, x.pid, "live").ok().and_then(|l| frames_of(&l)).is_some_and(|f| f > frames0) {
                p.frame_ms = wall_ms();
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let x = set(ctx, id, |s| s.patch = Some(p.clone()));
    let after = |t: u64| if t == 0 { "-".to_string() } else { format!("{} ms", t.saturating_sub(p.edit_ms)) };
    event(
        ctx,
        if matches!(p.outcome.as_str(), "patched") { Sev::Info } else { Sev::Warn },
        format!("live {id}: {} file(s) {} (save → shipped {}, → verdict {}, → frame {}){}", files.len(), p.outcome, after(p.shipped_ms), after(p.outcome_ms), after(p.frame_ms), if p.reason.is_empty() { String::new() } else { format!(": {}", p.reason) }),
        x.as_ref(),
    );
}

/// The output of the live app's run: the log, and the JIT's pid from its announce line.
struct RunOut {
    file: std::fs::File,
    buf: Vec<u8>,
    pid: Option<u32>,
}

impl std::io::Write for RunOut {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.pid.is_none() && self.buf.len() < 1 << 20 {
            self.buf.extend_from_slice(b);
            // `makepad-remote: pid <n> app <name> socket ...`
            let text = String::from_utf8_lossy(&self.buf);
            if let Some(n) = text.split("makepad-remote: pid ").nth(1).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse().ok()) {
                self.pid = Some(n);
            }
        }
        self.file.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// Holds a live job whose lease and app outlived the previous master, until it ends.
fn reattach(ctx: &Arc<Ctx>, x: &Live) {
    let held = Held::attach(&x.lease, &x.node);
    let tree = PathBuf::from(&x.tree);
    let r = Arc::new(Running { node: x.node.clone(), lease: x.lease.clone(), held, tree, pending: Mutex::new((BTreeSet::new(), 0, None)), inflight: std::sync::atomic::AtomicBool::new(false) });
    RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).insert(x.id.clone(), r.clone());
    set(ctx, &x.id, |s| s.touched_ms = wall_ms());
    let path = PathBuf::from(&x.log);
    let off = std::fs::metadata(&path).map_or(0, |m| m.len());
    let res = match std::fs::OpenOptions::new().append(true).create(true).open(&path) {
        Ok(mut f) => r.held.follow(x.run_n, off, &mut f),
        Err(e) => Err(e.to_string()),
    };
    finished(ctx, &x.id, &r, res);
}

/// The live job's app ended: its lease goes and the job is over.
fn finished(ctx: &Ctx, id: &str, r: &Running, res: Result<i32, String>) {
    RUNNING.lock().unwrap().as_mut().map(|m| m.remove(id));
    let _ = ask(&ctl(), None, "release", &r.lease);
    let x = set(ctx, id, |s| {
        if s.active() {
            s.state = "ended".into();
        }
        s.ended_ms = wall_ms();
        if s.note.is_empty() {
            s.note = match &res {
                Ok(c) => format!("the app exited ({c})"),
                Err(e) => crate::proto::clean(e, 200),
            };
        }
    });
    event(ctx, Sev::Info, format!("live {id} ended: {}", x.as_ref().map_or(String::new(), |x| x.note.clone())), x.as_ref());
}

/// Places the live job, starts its app, runs until it ends.
fn run(ctx: &Arc<Ctx>, id: &str) {
    let Some(x) = get(ctx, id) else { return };
    let fail = |e: String| {
        let x = set(ctx, id, |s| {
            s.state = "failed".into();
            s.ended_ms = wall_ms();
            s.note = crate::proto::clean(&e, 300);
        });
        event(ctx, Sev::Warn, format!("live {id} ({}) failed: {e}", x.as_ref().map_or("", |x| &x.app)), x.as_ref());
    };
    let tree = PathBuf::from(&x.tree);
    // every file saved in the tree ships to the box: a person's checkout never
    if crate::lease::ship::is_main_checkout(&tree) {
        return fail(format!("{} is a main checkout, not a lane's worktree: live ships every saved file", tree.display()));
    }
    let (rev, snap, _) = match super::job::snapshot(ctx, &tree, &x.lane) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let spec = Spec { only: false, lane: x.lane.clone(), agent: x.agent.clone(), kind: if x.visible { Kind::Screen } else { Kind::Test }, needs: x.needs.clone(), prio: crate::lease::sched::DEFAULT_PRIO, rev, repo_dir: x.tree.clone(), node: None };
    let held = match Held::acquire(&spec, &mut |_| {}, &|| get(ctx, id).is_none_or(|x| !x.active())) {
        Ok(h) => h,
        Err(e) => return fail(e),
    };
    if !snap.is_empty() {
        if let Err(e) = super::job::ship_snap(ctx, &held, &snap) {
            return fail(format!("shipping {snap}: {e}"));
        }
    }
    let log = ctx.logs("live").join(format!("{id}.log"));
    let _ = std::fs::create_dir_all(ctx.logs("live"));
    let file = match std::fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => return fail(format!("{}: {e}", log.display())),
    };
    let r = Arc::new(Running { node: held.node.clone(), lease: held.id.clone(), held, tree: tree.clone(), pending: Mutex::new((BTreeSet::new(), 0, None)), inflight: std::sync::atomic::AtomicBool::new(false) });
    RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).insert(id.to_string(), r.clone());
    let x = set(ctx, id, |s| {
        s.state = "running".into();
        s.lease = r.lease.clone();
        s.node = r.node.clone();
        s.started_ms = wall_ms();
        s.touched_ms = wall_ms();
        s.log = log.to_string_lossy().into_owned();
    });
    event(ctx, Sev::Info, format!("live {id}: {} starting on {} ({}, {})", x.as_ref().map_or("", |x| &x.app), r.node, r.lease, if x.as_ref().is_some_and(|x| x.visible) { "visible" } else { "headless" }), x.as_ref());
    // (any value of the variable hides windows: a visible run has it unset)
    let hide: &[&str] = if x.as_ref().is_some_and(|x| x.visible) { &["-u", "MAKEPAD_HIDE_WINDOWS"] } else { &["MAKEPAD_HIDE_WINDOWS=1"] };
    let argv: Vec<String> = ["env"]
        .iter()
        .chain(hide)
        .chain(&["cargo", "run", "--profile", "host", "-p", "makepad-run", "--bin", "makepad", "--", "run"])
        .map(|s| s.to_string())
        .chain(std::iter::once(x.as_ref().map_or(String::new(), |x| x.app.clone())))
        .collect();
    // the pid as soon as the JIT announces itself
    let (c2, id2) = (ctx.clone(), id.to_string());
    let out = RunOut { file, buf: Vec::new(), pid: None };
    let shared = Arc::new(Mutex::new(out));
    let s2 = shared.clone();
    let _ = std::thread::Builder::new().name("devhub-live-pid".into()).spawn(move || {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(600) && running(&id2).is_some() {
            if let Some(pid) = s2.lock().unwrap().pid {
                let x = set(&c2, &id2, |s| s.pid = pid);
                event(&c2, Sev::Info, format!("live {id2}: its JIT is up as pid {pid}"), x.as_ref());
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    struct Shared(Arc<Mutex<RunOut>>);
    impl std::io::Write for Shared {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(b)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.lock().unwrap().flush()
        }
    }
    let res = r.held.run_noted(&argv, &mut Shared(shared), &mut |n| {
        set(ctx, id, |s| s.run_n = n);
    });
    finished(ctx, id, &r, res);
}

/// A parked panic of a live job's app pauses it (act and resume through it).
pub fn on_record(ctx: &Ctx, r: &Record) {
    if r.kind != "panic" {
        return;
    }
    let Some(x) = ctx.store.read(|d| d.lives.iter().find(|x| x.state == "running" && x.node == r.machine && x.pid == r.pid).cloned()) else { return };
    let d = makepad_strict_json::parse(r.detail.as_bytes()).ok();
    let f = |k: &str| d.as_ref().and_then(|d| d.get(k)).and_then(|v| v.as_str()).map(|s| crate::proto::clean(s, 300)).unwrap_or_default();
    let p = JobPanic { id: f("id"), pid: r.pid, message: f("message"), thread: f("thread"), location: f("location"), at_ms: wall_ms() };
    let x = set(ctx, &x.id, |s| {
        s.state = "paused".into();
        s.panic = Some(p.clone());
        s.touched_ms = wall_ms();
    });
    if let Some(x) = x {
        event(ctx, Sev::Err, format!("live {} PAUSED on a panic: {} at {} [{}] ({}): live act {} stacks | live act {} resume {} retry", x.id, p.message, p.location, p.thread, p.id, x.id, x.id, p.id), Some(&x));
    }
}

/// `live start {"app","tree","lane","agent","needs","visible"}`, `live act <id> <cmd>`,
/// `live wait <id> patched|settled|frame [timeout_s]`, `live sync|release|get <id>`, `live list [all]`.
pub fn serve(ctx: &Arc<Ctx>, line: &str) -> Result<String, String> {
    let mut w = line.splitn(3, ' ');
    let _ = w.next();
    let verb = w.next().unwrap_or("list");
    let rest = w.next().unwrap_or("").trim();
    let (id, arg) = rest.split_once(' ').map_or((rest, ""), |(a, b)| (a, b.trim()));
    match verb {
        "start" => {
            let v = makepad_strict_json::parse(rest.as_bytes()).map_err(|_| "live start: bad request")?;
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let (app, tree, lane) = (s("app"), s("tree"), s("lane"));
            // a registry name (`calculator`) runs as its package (`makepad-app-calculator`)
            let app = makepad_app_registry::find(&app).map_or(app, |e| e.package.to_string());
            if app.is_empty() || !app.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return Err(format!("live: `{app}` is not a package name"));
            }
            if !Path::new(&tree).join(".git").exists() {
                return Err(format!("live: `{tree}` is not a worktree"));
            }
            if !crate::trust::valid_name(&lane) {
                return Err(format!("live: lane `{lane}` is not a name"));
            }
            if let Some(old) = ctx.store.read(|d| d.lives.iter().find(|x| x.active() && x.lane == lane && x.app == app).cloned()) {
                return Err(format!("{} already runs {app} for lane {lane} on {} (one per lane per app: live release {} first)", old.id, old.node, old.id));
            }
            // the monitor follows the tree from now (its saves ship to the box)
            let _ = ctx.monitor.dirty(Path::new(&tree));
            if Path::new(&tree).join("apps/commercial/.git").exists() {
                let _ = ctx.monitor.dirty(&Path::new(&tree).join("apps/commercial"));
            }
            let x = ctx.store.update(|d| {
                d.lives_next += 1;
                let x = Live { id: format!("W{}", d.lives_next), app: app.clone(), lane: lane.clone(), agent: s("agent"), tree: tree.clone(), needs: s("needs"), visible: v.get("visible").and_then(|b| b.as_bool()).unwrap_or(false), state: "starting".into(), touched_ms: wall_ms(), ..Default::default() };
                d.lives.push(x.clone());
                x
            });
            event(ctx, Sev::Info, format!("live {}: {app} for lane {lane} asks for a box", x.id), Some(&x));
            let (c, id) = (ctx.clone(), x.id.clone());
            let _ = std::thread::Builder::new().name("devhub-live".into()).spawn(move || run(&c, &id));
            Ok(format!("{{\"id\":{},\"app\":{},\"state\":\"starting\"}}", json_str(&x.id), json_str(&app)))
        }
        "act" => {
            let x = get(ctx, id).filter(|x| x.active()).ok_or_else(|| format!("no running live job {id}"))?;
            if x.pid == 0 {
                return Err(format!("{id}: its JIT is not up yet (live wait {id} frame)"));
            }
            set(ctx, id, |s| s.touched_ms = wall_ms());
            let t = Instant::now();
            let lines = match arg.split_whitespace().next() {
                // images come from the app's frames, answered by a client on its box (the `cmd`
                // service has nothing for them): the file stays there, in the lease's tmp
                Some("grab" | "gseq" | "gq" | "rec") if !arg.contains(" save") && !arg.contains(" status") => {
                    let r = running(id).ok_or_else(|| format!("{id}: not running here"))?;
                    let pid = x.pid.to_string();
                    let mut argv = vec!["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "inspect", &pid];
                    argv.extend(arg.split_whitespace());
                    let (code, text) = r.held.output(&argv)?;
                    if code != 0 {
                        return Err(format!("{id}: {arg} on {}: exit {code}: {}", x.node, crate::proto::clean(&text, 300)));
                    }
                    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
                    let tmp = format!("/tmp/{}/", r.lease);
                    if let Some(rel) = lines.iter().find_map(|l| l.split('"').find(|w| w.contains(&tmp) && w.ends_with(".png")).and_then(|w| w.split(&tmp).nth(1)).map(str::to_string)) {
                        lines.push(format!("copy it here: makepad devhub get {} tmp/{rel} <dir> --node {} --lease {}", x.lane, x.node, r.lease));
                    }
                    lines
                }
                _ => inspect(&x.node, x.pid, arg)?,
            };
            if arg.starts_with("resume ") {
                set(ctx, id, |s| {
                    if s.state == "paused" {
                        s.state = "running".into();
                    }
                });
            }
            Ok(format!("{{\"ms\":{},\"lines\":[{}]}}", t.elapsed().as_millis(), lines.iter().map(|l| json_str(l)).collect::<Vec<_>>().join(",")))
        }
        "wait" => {
            let mut a = arg.split_whitespace();
            let what = a.next().unwrap_or("patched");
            let timeout = Duration::from_secs(a.next().and_then(|t| t.parse().ok()).unwrap_or(120));
            let t = Instant::now();
            get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
            let asked = wall_ms();
            loop {
                let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
                if !x.active() {
                    return Ok(crate::state::live_value(&x).to_json());
                }
                let done = match what {
                    // the newest edit has its verdict: nothing waits to ship or for a verdict, and an
                    // edit saved before this call (the OS monitor hears it within ~0.1 s) is in
                    "patched" => {
                        let quiet = running(id).is_none_or(|r| r.pending.lock().unwrap().0.is_empty() && !r.inflight.load(std::sync::atomic::Ordering::SeqCst));
                        let newest = x.patch.as_ref().map_or(0, |p| p.edit_ms);
                        quiet && x.patch.as_ref().is_some_and(|p| !p.outcome.is_empty()) && (newest + 300 >= asked || t.elapsed() > Duration::from_millis(300))
                    }
                    // the app is up and its JIT idle
                    "settled" => x.pid > 0 && inspect(&x.node, x.pid, "jit ms=100").ok().and_then(|l| l.first().cloned()).is_some_and(|l| l.contains("idle")),
                    // a frame presented: the app is up and drawing
                    "frame" => x.pid > 0 && inspect(&x.node, x.pid, "live").ok().and_then(|l| frames_of(&l)).is_some_and(|f| f > 0),
                    w => return Err(format!("live wait: `{w}` (patched settled frame)")),
                };
                if done || x.state == "paused" {
                    return Ok(crate::state::live_value(&x).to_json());
                }
                if t.elapsed() > timeout {
                    return Err(format!("live wait {id} {what}: not within {} s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        "sync" => {
            let x = get(ctx, id).filter(|x| x.active()).ok_or_else(|| format!("no running live job {id}"))?;
            let r = running(id).ok_or_else(|| format!("{id} has no box yet"))?;
            let tree = PathBuf::from(&x.tree);
            let mut files = Vec::new();
            let d = ctx.monitor.dirty(&tree)?;
            files.extend(d.changed.into_iter().chain(d.deleted));
            if tree.join("apps/commercial/.git").exists() {
                let c = ctx.monitor.dirty(&tree.join("apps/commercial"))?;
                files.extend(c.changed.into_iter().chain(c.deleted).map(|p| format!("apps/commercial/{p}")));
            }
            let n = files.len();
            let mut p = r.pending.lock().unwrap();
            if p.0.is_empty() {
                p.1 = wall_ms();
            }
            p.0.extend(files);
            p.2 = Some(Instant::now() - DEBOUNCE);
            Ok(format!("{{\"id\":{},\"queued\":{n}}}", json_str(id)))
        }
        "release" => {
            let x = get(ctx, id).ok_or_else(|| format!("no live job {id}"))?;
            set(ctx, id, |s| s.note = "released".into());
            if running(id).is_some() {
                end_lease(id);
            } else if x.active() {
                set(ctx, id, |s| {
                    s.state = "ended".into();
                    s.ended_ms = wall_ms();
                });
            }
            Ok(format!("{{\"id\":{},\"state\":\"released\"}}", json_str(id)))
        }
        "get" => get(ctx, id).map(|x| crate::state::live_value(&x).to_json()).ok_or_else(|| format!("no live job {id}")),
        "list" => {
            let all = rest == "all";
            let list: Vec<String> = ctx.store.read(|d| d.lives.iter().filter(|x| all || x.active()).map(|x| crate::state::live_value(x).to_json()).collect());
            Ok(format!("[{}]", list.join(",")))
        }
        _ => Err(format!("live: unknown word `{verb}` (start act wait sync release get list)")),
    }
}

/// One live job, answer first: `W3 running makepad-app-calculator on 104 pid 4412; last edit: 2
/// file(s) patched, save → frame 38 ms`.
pub fn render(x: &Live) -> String {
    let mut out = format!("{} {} {} on {} pid {}{}\n", x.id, x.state, x.app, if x.node.is_empty() { "-" } else { &x.node }, x.pid, if x.note.is_empty() { String::new() } else { format!(" ({})", x.note) });
    if let Some(p) = &x.patch {
        let after = |t: u64| if t == 0 { "-".to_string() } else { format!("{} ms", t.saturating_sub(p.edit_ms)) };
        out.push_str(&format!("  last edit: {} file(s) {}; save → shipped {}, → verdict {}, → frame {}{}\n", p.files.len(), if p.outcome.is_empty() { "shipping" } else { &p.outcome }, after(p.shipped_ms), after(p.outcome_ms), after(p.frame_ms), if p.reason.is_empty() { String::new() } else { format!(": {}", p.reason) }));
    }
    if let Some(p) = x.panic.as_ref().filter(|_| x.state == "paused") {
        out.push_str(&format!("  panic {}: {} at {} [{}] -> live act {} stacks | live act {} resume {} retry|restart|panic\n", p.id, p.message, p.location, p.thread, x.id, x.id, p.id));
    }
    out
}
