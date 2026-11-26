//! `devhub dockbuild`: a Mac node's Dock apps kept compiled in its synced tree, the Runner's way,
//! so a click on a Dock entry (the Runner's pins, sync/launch.rs) starts the app's compiled build
//! of the current code. A node on the list (`dockbuild on <node>`) is built again whenever its
//! tree moves to another pair: the Runner's builds stamp the whole checkout, so every sync leaves
//! them behind. `dockbuild run <node>` builds it now.
//!
//! A run is the Runner itself, hidden, from the tree (`launch ... hidden release runner`), asked
//! `compile all`: each app it may build gets the build its compiled run button makes (the same
//! folder, flags and current-code check), one at a time, below other work; a pinned app's Dock
//! entry becomes the new build in place (a hidden Runner keeps a shown entry shown). It runs under
//! one background lease pinned to the node that holds no tree (no rev): lane and timed work go
//! first, and a timed lease that waits for the node stops it (`compile cancel`, the Runner stopped,
//! the lease given back); the next tick goes on, cargo's work kept. A master restart ends a run;
//! the next master starts it again. Results are state (`dockbuild`) and events.

use super::leg::{Held, Spec, BACKGROUND_PRIO};
use super::{age, Ctx};
use crate::lease::sched::State;
use crate::lease::Kind;
use crate::proto::{wall_ms, Sev};
use makepad_strict_json::{obj, s, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(120);
/// How often a run reads the Runner's queue.
const LOOK: Duration = Duration::from_secs(30);
/// The Runner's own start (a cold JIT cache compiles it first).
const START_WAIT: Duration = Duration::from_secs(900);
const AGENT: &str = crate::lease::sched::DOCKBUILD_AGENT;

fn ctl() -> makepad_remote::link::Client {
    makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), AGENT)
}

/// One app's build as the Runner said it at the run's end.
#[derive(Clone, Debug, Default, PartialEq)]
struct AppBuild {
    app: String,
    /// none, current, stale; locked: a licensed app before the login
    build: String,
    error: String,
}

/// A node's dock build: whether it follows the node's syncs, whether it was asked, and its last run.
#[derive(Clone, Debug, Default, PartialEq)]
struct NodeBuild {
    node: String,
    on: bool,
    asked: bool,
    running: bool,
    /// the pair (`work,main`) its last whole run built
    pair: String,
    at_ms: u64,
    secs: u64,
    /// how the last run ended: done, yielded, or why not
    verdict: String,
    apps: Vec<AppBuild>,
}

impl NodeBuild {
    fn to_value(&self) -> Value {
        obj(vec![
            ("node", s(&self.node)),
            ("on", Value::Bool(self.on)),
            ("asked", Value::Bool(self.asked)),
            ("running", Value::Bool(self.running)),
            ("pair", s(&self.pair)),
            ("at_ms", Value::Int(self.at_ms as i64)),
            ("secs", Value::Int(self.secs as i64)),
            ("verdict", s(&self.verdict)),
            ("apps", Value::Arr(self.apps.iter().map(|a| obj(vec![("app", s(&a.app)), ("build", s(&a.build)), ("error", s(&a.error))])).collect())),
        ])
    }

    fn from_value(v: &Value) -> Option<NodeBuild> {
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let num = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Some(NodeBuild {
            node: v.get("node")?.as_str()?.to_string(),
            on: flag("on"),
            asked: flag("asked"),
            running: flag("running"),
            pair: text(v, "pair"),
            at_ms: num("at_ms"),
            secs: num("secs"),
            verdict: text(v, "verdict"),
            apps: v.get("apps").and_then(Value::as_arr).unwrap_or(&[]).iter().map(|a| AppBuild { app: text(a, "app"), build: text(a, "build"), error: text(a, "error") }).collect(),
        })
    }
}

fn nodes(ctx: &Ctx) -> Vec<NodeBuild> {
    ctx.store.read(|d| d.dockbuild.iter().filter_map(NodeBuild::from_value).collect())
}

/// Changes `node`'s record (made when it has none).
fn update(ctx: &Ctx, node: &str, f: impl FnOnce(&mut NodeBuild)) -> Result<(), String> {
    ctx.store.update(|d| {
        let mut all: Vec<NodeBuild> = d.dockbuild.iter().filter_map(NodeBuild::from_value).collect();
        let at = all.iter().position(|n| n.node == node).unwrap_or_else(|| {
            all.push(NodeBuild { node: node.to_string(), ..Default::default() });
            all.len() - 1
        });
        f(&mut all[at]);
        d.dockbuild = all.iter().map(NodeBuild::to_value).collect();
    })
}

pub fn start(ctx: Arc<Ctx>) {
    // a run the previous master did not see end is asked again
    for n in nodes(&ctx).into_iter().filter(|n| n.running) {
        let _ = update(&ctx, &n.node, |r| {
            r.running = false;
            r.asked = true;
        });
    }
    let _ = std::thread::Builder::new().name("devhub-dockbuild".into()).spawn(move || loop {
        // a box the master counts off the network is asked nothing: each tick asked a lease
        // there that the master denied as off the network (11-21)
        let up: Vec<String> = ctx.leases.nodes().into_iter().filter(|n| n.up).map(|n| n.name).collect();
        for n in nodes(&ctx).into_iter().filter(|n| up.contains(&n.node)) {
            if let Some((pair, why)) = due(&n) {
                run(&ctx, &n.node, pair, &why);
            }
        }
        ctx.store.wait(TICK);
    });
}

/// The node's synced pair (`work,main`, short) from its sync service.
fn synced_pair(node: &str) -> Result<String, String> {
    let lines = crate::cli::ask(&ctl(), Some(node), "sync", "status")?;
    let v = lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).ok_or("sync status: no answer")?;
    let sha = |repo: &str| v.get("repos").and_then(|r| r.get(repo)).and_then(|r| r.get("checkout")).and_then(Value::as_str).map(|c| super::short(c).to_string());
    Ok(format!("{},{}", sha("makepad").ok_or("sync status: no makepad checkout")?, sha("commercial").unwrap_or_default()))
}

/// Whether to build `n` now, at which pair: asked, or on and its tree at a pair it has not built.
fn due(n: &NodeBuild) -> Option<(String, String)> {
    if !n.on && !n.asked {
        return None;
    }
    let pair = synced_pair(&n.node).ok()?;
    if n.asked {
        return Some((pair, "asked".into()));
    }
    (pair != n.pair).then(|| (pair.clone(), if n.pair.is_empty() { "first build".into() } else { format!("the tree moved from {}", n.pair) }))
}

/// A timed lease waits for the node: it goes first.
fn timed_waits(ctx: &Ctx, node: &str) -> bool {
    ctx.leases.view().iter().any(|l| l.state == State::Queued && l.req.kind == Kind::Timed && l.req.node.as_deref() == Some(node))
}

/// The node's launch service.
fn launch_ask(node: &str, line: &str) -> Result<Vec<String>, String> {
    let lines = crate::cli::ask(&ctl(), Some(node), "launch", line)?;
    match lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("err")?.as_str().map(str::to_string)) {
        Some(e) => Err(e),
        None => Ok(lines),
    }
}

/// An inspect command on a JIT (`node/pid`): its answer lines.
fn inspect(jit: &str, cmd: &str) -> Vec<String> {
    let Some((node, pid)) = jit.split_once('/') else { return Vec::new() };
    let mut out = Vec::new();
    let _ = makepad_remote::hub::ask(&ctl(), &format!("{node}:{pid}"), "cmd", "", cmd, &mut |l| {
        out.push(l.to_string());
        true
    });
    out
}

/// The Runner's queue (its `compile` answer's first line): whether it is done.
fn idle(status: &[String]) -> Option<bool> {
    let v = makepad_strict_json::parse(status.first()?.as_bytes()).ok()?;
    let queue = v.get("queue")?.as_arr()?;
    Some(queue.is_empty() && matches!(v.get("building"), Some(Value::Null)))
}

/// Each app's build in the Runner's `compile` answer.
fn builds(status: &[String]) -> Vec<AppBuild> {
    status
        .iter()
        .skip(1)
        .filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .filter_map(|v| {
            let app = v.get("app")?.as_str()?.to_string();
            let locked = v.get("locked").and_then(Value::as_bool).unwrap_or(false);
            let build = if locked { "locked".to_string() } else { v.get("build").and_then(Value::as_str).unwrap_or("").to_string() };
            Some(AppBuild { app, build, error: v.get("error").and_then(Value::as_str).unwrap_or("").to_string() })
        })
        .collect()
}

/// What the Runner's `compile` answer says it does now, for its lease ([`crate::lease::master::LeaseMaster::note_progress`]):
/// the app it builds and how far, how many wait, how many are current. None when it builds
/// nothing and nothing waits (its queue is done), or the answer is no status yet.
fn progress(status: &[String]) -> Option<String> {
    let v = makepad_strict_json::parse(status.first()?.as_bytes()).ok()?;
    let queued = v.get("queue")?.as_arr()?.len();
    let building = v.get("building").and_then(Value::as_str);
    if building.is_none() && queued == 0 { return None; }
    let apps: Vec<Value> = status.iter().skip(1).filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).collect();
    let current = apps.iter().filter(|a| a.get("build").and_then(Value::as_str) == Some("current")).count();
    let doing = building.map_or("the Runner builds nothing yet".to_string(), |app| {
        let at = apps.iter().find(|a| a.get("app").and_then(Value::as_str) == Some(app));
        let state = at.and_then(|a| a.get("state")?.as_str()).filter(|s| !s.is_empty()).map_or(String::new(), |s| format!(": {s}"));
        let part = at.and_then(|a| crate::lease::num(a.get("fraction")?)).filter(|f| *f > 0.0).map_or(String::new(), |f| format!(" {:.0}%", f * 100.0));
        format!("the Runner builds {app}{state}{part}")
    });
    Some(format!("{doing}, {queued} more queued, {current} of {} apps current", apps.len()))
}

fn run(ctx: &Ctx, node: &str, pair: String, why: &str) {
    let started = wall_ms();
    let _ = update(ctx, node, |r| {
        r.running = true;
        r.asked = false;
    });
    ctx.store.event("dockbuild", Sev::Info, format!("dockbuild {node}: the Runner compiles the Dock apps at {pair} ({why})"), None);
    let spec = Spec { since_ms: Some(started), lane: format!("dockbuild-{node}"), agent: AGENT.into(), kind: Kind::Test, needs: String::new(), prio: BACKGROUND_PRIO, rev: String::new(), repo_dir: ctx.repo.to_string_lossy().into_owned(), node: Some(node.to_string()) };
    let (verdict, apps) = match Held::acquire(&spec, &mut |_| {}, &|| false) {
        Err(e) => (format!("no lease: {}", crate::proto::clean(&e, 160)), Vec::new()),
        Ok(h) => {
            let r = compile_all(ctx, node, &h.id);
            h.release(None);
            r
        }
    };
    let secs = wall_ms().saturating_sub(started) / 1000;
    let done = verdict == "done";
    let current = apps.iter().filter(|a| a.build == "current").count();
    let failed: Vec<&str> = apps.iter().filter(|a| !a.error.is_empty()).map(|a| a.app.as_str()).collect();
    let _ = update(ctx, node, |n| {
        n.running = false;
        n.verdict = verdict.clone();
        n.secs = secs;
        n.at_ms = started;
        if !apps.is_empty() {
            n.apps = apps.clone();
        }
        // a run that did not finish is no build of that pair: the next tick goes on with it
        if done {
            n.pair = pair.clone();
        }
    });
    let line = format!("dockbuild {node} at {pair}: {verdict} in {secs} s, {current} of {} apps current{}", apps.len(), if failed.is_empty() { String::new() } else { format!("; failed: {}", failed.join(", ")) });
    ctx.store.event("dockbuild", verdict_sev(&verdict, failed.is_empty()), line, None);
}

/// A run's verdict line's severity: a warning when it did not finish or an app failed, but not
/// for a run its hub's restart cut ([`CUT`]): that is the roll's, and the next tick builds again.
fn verdict_sev(verdict: &str, none_failed: bool) -> Sev {
    if (verdict == "done" && none_failed) || verdict.starts_with(CUT) { Sev::Info } else { Sev::Warn }
}

/// The verdict of a run whose node's hub restarted under it and whose lease ended meanwhile.
const CUT: &str = "its hub restarted under the run";

/// Whether the node's hub answers again after a restart cut a call (its self-update: runs and
/// leases go on), asked each second while `lease` is held; false once the lease ended.
fn hub_back(ctx: &Ctx, node: &str, lease: &str) -> bool {
    loop {
        if !super::leg::lease_held(ctx, lease) { return false; }
        std::thread::sleep(Duration::from_secs(1));
        if crate::cli::ask(&ctl(), Some(node), "caps", "caps").is_ok() { return true; }
    }
}

/// The pid of the Runner a launch under `lease` started, of the node's `launch list` lines (one
/// from a hub that names no lease there is none).
fn runner_under(listed: &[String], lease: &str) -> Option<u64> {
    listed.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .find(|v| v.get("lease").and_then(Value::as_str) == Some(lease) && v.get("app").and_then(Value::as_str) == Some("runner"))
        .and_then(|v| v.get("pid")?.as_u64())
}

/// The hidden Runner from the node's tree under `lease`, asked `compile all`, until its queue is
/// done or a timed lease waits; it is stopped either way. Returns how it ended and the builds.
fn compile_all(ctx: &Ctx, node: &str, lease: &str) -> (String, Vec<AppBuild>) {
    let launch = format!("launch lease={lease} hidden release runner");
    let lines = match launch_ask(node, &launch) {
        Ok(l) => l,
        // its hub restarted under the launch, for its self-update: once it answers, the Runner the
        // launch started is the run's, or it launches again (a Mac's hub restarted 33 s into one, and
        // the run warned "launch failed: connection closed by the peer", 11-22)
        Err(e) if crate::lease::master::unreachable(&e) => {
            if !hub_back(ctx, node, lease) {
                return (format!("{CUT}, and its lease ended meanwhile: the next tick builds again"), Vec::new());
            }
            let found = runner_under(&launch_ask(node, "list").unwrap_or_default(), lease);
            ctx.store.event("dockbuild", Sev::Info, format!("dockbuild {node}: its hub restarted under the Runner's launch ({}); {}", crate::proto::clean(&e, 120), found.map_or("launched again once it answered".to_string(), |p| format!("its Runner {p} goes on"))), None);
            match found {
                Some(pid) => vec![format!("{{\"launched\":{pid}}}")],
                None => match launch_ask(node, &launch) {
                    Ok(l) => l,
                    Err(e) => return (format!("launch failed: {}", crate::proto::clean(&e, 160)), Vec::new()),
                },
            }
        }
        Err(e) => return (format!("launch failed: {}", crate::proto::clean(&e, 160)), Vec::new()),
    };
    let Some(pid) = lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()?.get("launched")?.as_u64()) else {
        return ("launch failed: no answer".into(), Vec::new());
    };
    let stop = || {
        let _ = launch_ask(node, &format!("stop {pid}"));
    };
    // the Runner's JIT, once it shows its first frame
    let t = Instant::now();
    let jit = loop {
        let listed = match launch_ask(node, "list") {
            Ok(l) => l,
            // (a hub restart cut the look: the Runner runs on under the lease)
            Err(e) if crate::lease::master::unreachable(&e) => {
                if !hub_back(ctx, node, lease) { return (format!("{CUT}, and its lease ended meanwhile: the next tick builds again"), Vec::new()); }
                continue;
            }
            Err(_) => Vec::new(),
        };
        let Some(e) = listed.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).find(|e| e.get("pid").and_then(Value::as_u64) == Some(pid)) else {
            return ("the Runner exited before its first frame".into(), Vec::new());
        };
        if let (Some(_), Some(jit)) = (e.get("present_ms").and_then(Value::as_u64), e.get("inspect").and_then(Value::as_str)) {
            break jit.to_string();
        }
        if timed_waits(ctx, node) || t.elapsed() > START_WAIT {
            stop();
            return (if t.elapsed() > START_WAIT { "the Runner showed no frame" } else { "yielded" }.into(), Vec::new());
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    inspect(&jit, "compile all");
    let mut last = Vec::new();
    let verdict = loop {
        std::thread::sleep(LOOK);
        let status = inspect(&jit, "compile");
        if status.is_empty() {
            // (its hub restarting cuts the call too: asked again once it answers)
            if crate::cli::ask(&ctl(), Some(node), "caps", "caps").is_err() {
                if hub_back(ctx, node, lease) { continue; }
                break format!("{CUT}, and its lease ended meanwhile: the next tick builds again");
            }
            break "the Runner stopped answering".to_string();
        }
        if !status[0].starts_with("{\"note\"") {
            last = status.clone();
        }
        // its builds write nothing to the lease's log: what the Runner says it does is the lease's
        // progress, or its stall check called a 34-app build silent at 30 min (L1797945, 11-26)
        if let Some(line) = progress(&status) {
            ctx.leases.note_progress(lease, &line);
        }
        if idle(&status) == Some(true) {
            break "done".into();
        }
        if timed_waits(ctx, node) {
            inspect(&jit, "compile cancel");
            // the build that runs stops at its next step: the Runner says so before it goes
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(60) && idle(&inspect(&jit, "compile")) != Some(true) {
                std::thread::sleep(Duration::from_secs(2));
            }
            break "yielded".into();
        }
    };
    stop();
    (verdict, builds(&last))
}

/// `dockbuild [status] | on <node> | off <node> | run <node>`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let words: Vec<&str> = line.split_whitespace().skip(1).collect();
    match words.as_slice() {
        [] | ["status"] => Ok(ctx.store.read(|d| d.section(Some("dockbuild"))).unwrap_or_default()),
        [on @ ("on" | "off"), node] => {
            update(ctx, node, |n| n.on = *on == "on")?;
            ctx.store.event("dockbuild", Sev::Info, format!("dockbuild {node}: follows its syncs {on}"), None);
            Ok(format!("{{\"node\":{},\"on\":{}}}", makepad_remote::json_str(node), *on == "on"))
        }
        ["run", node] => {
            update(ctx, node, |n| n.asked = true)?;
            ctx.store.event("dockbuild", Sev::Info, format!("dockbuild {node}: asked"), None);
            Ok("{\"asked\":true}".into())
        }
        _ => Err("dockbuild [status] | on <node> | off <node> | run <node>".into()),
    }
}

/// The terse view: per node its last run, then each app's build.
pub fn render(v: &Value) -> String {
    let mut out = String::new();
    for n in v.as_arr().unwrap_or(&[]).iter().filter_map(NodeBuild::from_value) {
        let state = if n.running { " running".to_string() } else if n.asked { " asked".to_string() } else { String::new() };
        out.push_str(&format!("{} on={} built {} ({} ago, {} s): {}{state}\n", n.node, n.on as u8, if n.pair.is_empty() { "-" } else { &n.pair }, age(n.at_ms), n.secs, if n.verdict.is_empty() { "-" } else { &n.verdict }));
        for a in &n.apps {
            out.push_str(&format!("  {:<16} {:<8}{}\n", a.app, a.build, if a.error.is_empty() { String::new() } else { format!("  {}", a.error) }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node's record survives the state round trip, and the Runner's `compile` answer reads as
    /// its queue's state and each app's build.
    #[test]
    fn a_node_build_round_trips_and_reads_the_runners_answer() {
        let status: Vec<String> = [
            r#"{"queue":[],"building":null}"#,
            r#"{"app":"fab","locked":true,"pinned":false,"build":"none","state":"","fraction":0.00,"error":""}"#,
            r#"{"app":"clock","locked":false,"pinned":true,"build":"current","state":"Compiled","fraction":1.00,"error":""}"#,
            r#"{"app":"amp","locked":false,"pinned":true,"build":"stale","state":"The native build failed","fraction":0.00,"error":"The native build failed (exit 101)"}"#,
        ]
        .iter()
        .map(|l| l.to_string())
        .collect();
        assert_eq!(idle(&status), Some(true), "an empty queue with nothing building is done");
        assert_eq!(idle(&[r#"{"queue":["a"],"building":"b"}"#.to_string()]), Some(false));
        assert_eq!(idle(&[r#"{"note":"ask again"}"#.to_string()]), None, "the Runner's first answer is no status");
        // what it does now, for its lease: nothing once done, else the app it builds and how far
        assert_eq!(progress(&status), None, "a done queue is no progress");
        assert_eq!(progress(&[r#"{"note":"ask again"}"#.to_string()]), None);
        let mut busy = status.clone();
        busy[0] = r#"{"queue":["fab","amp"],"building":"clock"}"#.into();
        busy[2] = r#"{"app":"clock","locked":false,"pinned":true,"build":"stale","state":"Compiling","fraction":0.40,"error":""}"#.into();
        assert_eq!(progress(&busy).as_deref(), Some("the Runner builds clock: Compiling 40%, 2 more queued, 0 of 3 apps current"));
        let apps = builds(&status);
        assert_eq!(apps.iter().map(|a| (a.app.as_str(), a.build.as_str())).collect::<Vec<_>>(), vec![("fab", "locked"), ("clock", "current"), ("amp", "stale")]);
        let n = NodeBuild { node: "m9".into(), on: true, asked: false, running: false, pair: "aaaa,bbbb".into(), at_ms: 5, secs: 61, verdict: "done".into(), apps };
        let back = NodeBuild::from_value(&makepad_strict_json::parse(n.to_value().to_json().as_bytes()).unwrap()).unwrap();
        assert_eq!(back, n);
        let text = render(&Value::Arr(vec![n.to_value()]));
        assert!(text.contains("aaaa,bbbb") && text.contains("clock") && text.contains("exit 101"), "{text}");
        // a hub restart under the launch: the Runner its lease started goes on (a hub from before
        // the list named leases has none), and a run it cut is told as info, not a failure (11-22)
        let listed: Vec<String> = [
            r#"{"pid":7,"id":"m9/runner#7","app":"runner","visible":false,"jit":true,"inspect":null,"present_ms":null,"log":"/l","lease":"L2"}"#,
            r#"{"pid":8,"id":"m9/fab#8","app":"fab","visible":false,"jit":true,"inspect":null,"present_ms":null,"log":"/l","lease":"L1"}"#,
            r#"{"pid":9,"id":"m9/runner#9","app":"runner","visible":false,"jit":true,"inspect":null,"present_ms":null,"log":"/l","lease":"L1"}"#,
            r#"{"pid":10,"id":"m9/runner#10","app":"runner","visible":false,"jit":true,"inspect":null,"present_ms":null,"log":"/l"}"#,
        ].iter().map(|l| l.to_string()).collect();
        assert_eq!((runner_under(&listed, "L1"), runner_under(&listed, "L3")), (Some(9), None), "the lease's own Runner");
        assert_eq!(verdict_sev(&format!("{CUT}, and its lease ended meanwhile"), true), Sev::Info, "a cut run is the roll's");
        assert_eq!((verdict_sev("done", true), verdict_sev("done", false), verdict_sev("launch failed: x", true)), (Sev::Info, Sev::Warn, Sev::Warn));
    }
}
