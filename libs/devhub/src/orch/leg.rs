//! A command under a lease, run by the master itself (verify legs, allapps): the lease is asked on
//! the master's own control socket like any CLI's, this process holds it and beats it, the node
//! runs the command in the lease's tree at the pinned commits, and its output streams into a log
//! file here. The run lives on the node, which keeps its output and exit code: a restarted master
//! takes the lease back ([`lease_held`]) and follows the run from where its log stands
//! ([`Held::attach`], [`Held::follow`]).

use crate::cli::ask;
use crate::lease::sched::{parse_needs, Req};
use crate::lease::Kind;
use makepad_remote::link::{Client, Target};
use makepad_strict_json::Value;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BEAT: Duration = Duration::from_secs(2);
/// How long node calls may fail in a row (a link coming back) before the run counts as lost.
const NODE_GRACE: Duration = Duration::from_secs(90);
/// How long a run's start waits out its node's hub restarting for the self-update (a `cargo run`).
const HUB_RESTART_FOR: Duration = Duration::from_secs(900);
/// The exit a node reports for a run whose end it did not see (its hub restarted under it).
const EXIT_UNKNOWN: i32 = -1;

pub struct Spec {
    pub lane: String,
    pub agent: String,
    pub kind: Kind,
    /// `arch=x64,os=linux,role=pool`
    pub needs: String,
    pub prio: i32,
    /// the pair's `--rev` (pinned here)
    pub rev: String,
    pub repo_dir: String,
    /// a preferred node (a twin of the same arch and os may take it, unless `only`)
    pub node: Option<String>,
    pub only: bool,
    /// when the work first asked for a box (None: now): its place in the queue
    pub since_ms: Option<u64>,
}

/// Background work (verify legs, allapps) asks at this priority or lower; it never runs ahead of
/// lanes and landings: it asks no lease while one of theirs waits, withdraws a queued request
/// when one starts waiting, holds at most N-2 of the N dev boxes, and a running background lease
/// is preempted when lane work has waited ~30 s ([`super::preempt`]).
pub const BACKGROUND_PRIO: i32 = 9;

/// The fair share: a background run going this long is owed boxes. Its work stops stepping back for
/// waiting lane work and asks at [`OWED_PRIO`], so it takes the next free box ahead of new lane
/// leases (landing gates still go first); running lane leases are never ended for it, and as it is
/// no longer background it is not preempted. Verify first and past the N-2 cap too, allapps later.
/// CI shards always stay behind lane work. Counted from the run's start in the state: no
/// master restart, preemption or re-ask starts it over.
fn owed_after(agent: &str) -> Duration {
    match agent {
        "devhub-verify" => Duration::from_secs(20 * 60),
        _ => Duration::from_secs(40 * 60),
    }
}

/// When the run an agent's lease is for started.
fn run_started_ms(agent: &str) -> Option<u64> {
    super::ctx()?.store.read(|d| match agent {
        "devhub-verify" => d.verify.run.as_ref().map(|r| r.started_ms),
        "devhub-allapps" => d.allapps.running.as_ref().map(|r| r.at_ms),
        _ => None,
    })
}

/// The priority of owed background work: just ahead of a lane's default.
pub const OWED_PRIO: i32 = crate::lease::sched::DEFAULT_PRIO - 1;

/// The agents whose leases are background work (also once owed: they count to the N-2 cap).
const BACKGROUND_AGENTS: [&str; 3] = ["devhub-verify", "devhub-ci", "devhub-allapps"];

/// Background leases the master ended for waiting lane work: their legs ask again.
static PREEMPTED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn mark_preempted(id: &str) {
    let mut p = PREEMPTED.lock().unwrap();
    if p.len() > 64 {
        p.clear();
    }
    p.push(id.to_string());
}

/// Whether the master preempted lease `id` (and forgets it).
pub fn was_preempted(id: &str) -> bool {
    let mut p = PREEMPTED.lock().unwrap();
    match p.iter().position(|x| x == id) {
        Some(i) => {
            p.remove(i);
            true
        }
        None => false,
    }
}

/// Why background work must not ask (or keep asking) for a box now; `mine`: its queued request.
pub fn background_block(lm: &crate::lease::master::LeaseMaster, mine: Option<&str>, needs: &[(String, String)]) -> Option<String> {
    use crate::lease::sched::State;
    let view = lm.view();
    let nodes = lm.nodes();
    let other = |id: &str| mine != Some(id);
    // a box both could use: lane work holds background work back only from the boxes it could
    // take itself (a waiting macOS request never holds back Linux shards)
    let usable = |want: &[(String, String)], pin: Option<&str>, n: &crate::lease::sched::NodeView| n.up && n.dev && !n.role().users() && pin.is_none_or(|p| p == n.name) && super::fits(want, n.caps.as_ref());
    let shared = |l: &crate::lease::sched::Lease| nodes.iter().any(|n| usable(needs, None, n) && usable(&l.req.needs, l.req.node.as_deref(), n));
    // lane work goes first, and a box beyond what it waits for is not taken from it
    let waiting: Vec<_> = view.iter().filter(|l| l.state == State::Queued && l.req.kind != Kind::Ci && !l.lane_capped() && l.req.prio < BACKGROUND_PRIO && !is_background(l) && other(&l.id) && shared(l)).collect();
    if let Some(l) = waiting.first() {
        let free = nodes.iter().filter(|n| usable(needs, None, n) && !n.draining && !view.iter().any(|l| matches!(l.state, State::Granted | State::Starting) && l.node.as_deref() == Some(n.name.as_str()) && !matches!(l.req.kind, Kind::Ci | Kind::Inspect))).count();
        if free <= waiting.len() {
            return Some(format!("lane work waits for a box this could take ({} of {}; {} waiting, {free} free)", l.id, l.req.lane, waiting.len()));
        }
    }
    background_cap(lm, mine)
}

pub fn is_background(l: &crate::lease::sched::Lease) -> bool {
    l.priority == crate::lease::Priority::Background && (l.req.prio >= BACKGROUND_PRIO || l.req.agent.as_deref().is_some_and(|a| BACKGROUND_AGENTS.contains(&a)))
}

/// Background work (owed included) holds at most N-2 of the N dev boxes.
fn background_cap(lm: &crate::lease::master::LeaseMaster, mine: Option<&str>) -> Option<String> {
    use crate::lease::sched::State;
    let view = lm.view();
    let n = lm.nodes().iter().filter(|n| n.up && n.dev && !n.role().users()).count();
    let running = view.iter().filter(|l| matches!(l.state, State::Granted | State::Starting) && l.req.kind != Kind::Ci && is_background(l) && mine != Some(l.id.as_str())).count();
    (running + 1 > n.saturating_sub(2)).then(|| format!("background holds {running} of {n} boxes (at most N-2)"))
}

/// Whether a granted lease is live here: one a previous master held comes back through
/// `leases.live`, or it ended with no holder.
pub fn lease_held(ctx: &super::Ctx, id: &str) -> bool {
    !id.is_empty() && ctx.leases.view().iter().any(|l| l.id == id && l.state == crate::lease::sched::State::Granted)
}

/// A granted lease this process holds; released on drop.
pub struct Held {
    pub id: String,
    pub node: String,
    ctl: Client,
    released: bool,
    /// its own thread beats the lease: a slow node call must not cost it
    _beats: Beats,
}

/// Beats a lease until dropped.
struct Beats(Arc<AtomicBool>, std::thread::Thread);

impl Beats {
    fn start(id: &str) -> Beats {
        let stop = Arc::new(AtomicBool::new(false));
        let (stopped, id) = (stop.clone(), id.to_string());
        let beater = std::thread::Builder::new().name("devhub-lease-beat".into()).spawn(move || {
            let ctl = Client::new(Target::Unix(crate::proto::control_socket()), "devhub-orch");
            while !stopped.load(Ordering::Acquire) {
                std::thread::park_timeout(BEAT);
                if !stopped.load(Ordering::Acquire) {
                    let _ = ask(&ctl, None, "beat", &id);
                }
            }
        });
        Beats(stop, beater.map_or_else(|_| std::thread::current(), |b| b.thread().clone()))
    }
}

impl Drop for Beats {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
        self.1.unpark();
    }
}

fn json(l: &str) -> Option<Value> {
    makepad_strict_json::parse(l.as_bytes()).ok()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn first(lines: Result<Vec<String>, String>, what: &str) -> Result<Value, String> {
    let lines = lines?;
    let v = lines.first().and_then(|l| json(l)).ok_or_else(|| format!("{what}: no answer"))?;
    match v.get("err").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

impl Held {
    /// Asks for the lease and waits for its grant; `on_wait` hears each queue state line, and
    /// `stop` (asked every beat) gives the request up.
    pub fn acquire(spec: &Spec, on_wait: &mut dyn FnMut(&str), stop: &dyn Fn() -> bool) -> Result<Held, String> {
        Self::acquire_with(spec, &|_| {}, on_wait, stop)
    }

    /// Applies run-specific source and cost wishes before the request enters the queue.
    pub fn acquire_with(spec: &Spec, configure: &dyn Fn(&mut Req), on_wait: &mut dyn FnMut(&str), stop: &dyn Fn() -> bool) -> Result<Held, String> {
        let ctl = Client::new(Target::Unix(crate::proto::control_socket()), "devhub-orch");
        let mut req = Req::new(&spec.lane, spec.kind);
        req.needs = parse_needs(&spec.needs);
        req.prio = spec.prio;
        req.node = spec.node.clone();
        req.only = spec.only;
        req.repo_dir = Some(spec.repo_dir.clone());
        req.rev = Some(crate::lease::pin_revs(std::path::Path::new(&spec.repo_dir), &spec.rev)?);
        // (no holder pid: the master's restart keeps these, whatever its new pid)
        req.agent = Some(spec.agent.clone());
        req.since_ms = Some(spec.since_ms.unwrap_or_else(crate::proto::wall_ms));
        if spec.agent == "devhub-show" { req.priority = crate::lease::Priority::Interactive; }
        configure(&mut req);
        // CI's separate tree and slot are queued behind lane work by the scheduler.
        let background = req.priority == crate::lease::Priority::Background && spec.kind != Kind::Ci && spec.prio >= BACKGROUND_PRIO;
        let lm = super::leases();
        let mut last = String::new();
        let mut class_notified = false;
        let since = run_started_ms(&spec.agent);
        let waited = || Duration::from_millis(since.map_or(0, |t| crate::proto::wall_ms().saturating_sub(t)));
        // CI and sweeps stay at background priority even after a long wait.
        let owed = || background && spec.prio == BACKGROUND_PRIO && spec.lane != crate::lease::sched::CI_LANE && waited() > owed_after(&spec.agent);
        'ask: loop {
            // background work asks only while no lane work waits and it holds under N-2 boxes;
            // owed, only the cap holds it back, and it asks ahead of new lane leases
            if let (true, Some(lm)) = (background, &lm) {
                while let Some(why) = if owed() { (spec.agent != "devhub-verify").then(|| background_cap(lm, None)).flatten() } else { background_block(lm, None, &req.needs) } {
                    let line = format!("held back: {why}");
                    if line != last {
                        on_wait(&line);
                        last = line;
                    }
                    if stop() {
                        return Err("given up".into());
                    }
                    std::thread::sleep(BEAT);
                }
            }
            if owed() && req.prio != OWED_PRIO {
                req.prio = OWED_PRIO;
                // Re-asks keep their place from when the run became owed and age with lane work.
                if let Some(t) = since {
                    let owed_ms = t + owed_after(&spec.agent).as_millis() as u64;
                    req.since_ms = req.since_ms.map(|s| s.min(owed_ms));
                }
                on_wait(&format!("owed a box: its run is {} min old; asks ahead of new lane leases", waited().as_secs() / 60));
            }
            let mut gen = lm.as_ref().map_or(0, |m| m.generation());
            let mut v = first(ask(&ctl, None, "lease", &req.to_json()), "lease")?;
            let id = s(&v, "id");
            // fast at first (a free box grants at once), then at the beat; in the master the
            // scheduler's change wakes it at once (no poll between the grant and its start)
            let mut wait = Duration::from_millis(25);
            loop {
                let note = s(&v, "class_note");
                let note = if class_notified || note.is_empty() { String::new() } else { format!("; {note}") };
                match s(&v, "state").as_str() {
                    "granted" => {
                        if !note.is_empty() { on_wait(&format!("{id} granted{note}")); }
                        let beats = Beats::start(&id);
                        return Ok(Held { id, node: s(&v, "node"), ctl, released: false, _beats: beats });
                    }
                    "queued" | "starting" => {
                        let line = format!("{} {} {}", id, s(&v, "state"), s(&v, "reason"));
                        if line != last || !note.is_empty() {
                            on_wait(&format!("{line}{note}"));
                            last = line;
                            class_notified |= !note.is_empty();
                        }
                    }
                    _ => return Err(format!("{id} {}: {}", s(&v, "how"), s(&v, "reason"))),
                }
                match &lm {
                    Some(m) => {
                        m.wait_change(gen, wait);
                        gen = m.generation();
                    }
                    None => std::thread::sleep(wait),
                }
                wait = (wait * 2).min(BEAT);
                if stop() {
                    let _ = ask(&ctl, None, "release", &id);
                    return Err(format!("{id}: given up"));
                }
                // lane work started waiting: a queued background request steps back
                // owed now: asked again at the owed priority
                if background && owed() && req.prio != OWED_PRIO && s(&v, "state") == "queued" {
                    let _ = ask(&ctl, None, "release", &id);
                    continue 'ask;
                }
                if let (true, false, Some(lm), "queued") = (background, owed(), &lm, s(&v, "state").as_str()) {
                    if background_block(lm, Some(&id), &req.needs).is_some() {
                        let _ = ask(&ctl, None, "release", &id);
                        continue 'ask;
                    }
                }
                v = first(ask(&ctl, None, "beat", &id), "beat")?;
            }
        }
    }

    fn node_call(&self, service: &str, line: &str) -> Result<Value, String> {
        first(ask(&self.ctl, Some(&self.node), service, line), service).map_err(|e| format!("{}: {e}", self.node))
    }

    /// A lease this process took before its exec (the same pid holds it): held again.
    pub fn attach(id: &str, node: &str) -> Held {
        Held { id: id.to_string(), node: node.to_string(), ctl: Client::new(Target::Unix(crate::proto::control_socket()), "devhub-orch"), released: false, _beats: Beats::start(id) }
    }

    /// Runs `argv` in the lease's tree; its output goes to `log`; its exit code.
    pub fn run(&self, argv: &[String], log: &mut dyn Write) -> Result<i32, String> {
        self.run_noted(argv, log, &mut |_| {})
    }

    /// As [`Self::run`], telling `started` the run's number.
    pub fn run_noted(&self, argv: &[String], log: &mut dyn Write, started: &mut dyn FnMut(u64)) -> Result<i32, String> {
        self.run_at(".", argv, log, started)
    }

    /// A live job holds its run until the JIT ends; a hub restart may lose the exit status.
    pub fn run_live(&self, argv: &[String], log: &mut dyn Write, started: &mut dyn FnMut(u64)) -> Result<i32, String> {
        self.run_at_live(".", argv, true, log, started)
    }

    pub fn run_at(&self, cwd: &str, argv: &[String], log: &mut dyn Write, started: &mut dyn FnMut(u64)) -> Result<i32, String> {
        self.run_at_live(cwd, argv, false, log, started)
    }

    fn run_at_live(&self, cwd: &str, argv: &[String], live: bool, log: &mut dyn Write, started: &mut dyn FnMut(u64)) -> Result<i32, String> {
        let b64 = makepad_network::digest::base64_encode(argv.join("\0").as_bytes());
        let extra = if live { " {\"live\":true}" } else { "" };
        // the node's hub restarting for its self-update (at a run boundary: this one) takes the
        // lease back from its ledger: the run starts on the new hub
        let (mut failing, mut last, mut beat) = (None::<Instant>, String::new(), Instant::now());
        let v = loop {
            match self.node_call("lease-run", &format!("lease-run {} {cwd} {b64}{extra}", self.id)) {
                Ok(v) => {
                    failing = None;
                    let Some(pending) = v.get("pending").and_then(Value::as_str) else { break v };
                    if pending != last {
                        let _ = writeln!(log, "{} {}: {pending}", self.id, self.node);
                        let _ = log.flush();
                        last = pending.to_string();
                    }
                    if beat.elapsed() >= BEAT {
                        beat = Instant::now();
                        let st = first(ask(&self.ctl, None, "beat", &self.id), "beat")?;
                        if s(&st, "state") == "ended" { return Err(format!("{} ended ({})", self.id, s(&st, "how"))); }
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
                Err(e) if e.contains("restarting: ") || crate::lease::master::unreachable(&e) => {
                    if failing.get_or_insert_with(Instant::now).elapsed() >= HUB_RESTART_FOR { return Err(e); }
                    std::thread::sleep(Duration::from_secs(2));
                }
                Err(e) => return Err(e),
            }
        };
        let n = v.get("n").and_then(crate::lease::num).map_or(1, |x| x as u64);
        started(n);
        self.follow(n, 0, log)
    }

    /// Whether the node has run `n` of the lease: a run it started that the master that asked for
    /// it did not live to note (it then goes on, it is not started again).
    pub fn has_run(&self, n: u64) -> Result<bool, String> {
        match self.node_call("lease-log", &format!("lease-log {} {n} 0", self.id)) {
            Ok(_) => Ok(true),
            Err(e) if e.contains(&format!("has no run {n}")) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Follows run `n` of the lease from byte `off` of its output, to `log`; its exit code.
    pub fn follow(&self, n: u64, off: u64, log: &mut dyn Write) -> Result<i32, String> {
        let (mut off, mut beat, mut failing) = (off, Instant::now(), None::<Instant>);
        // the log is asked for fast while it is quiet at first, then every 0.5 s
        let mut quiet = Duration::from_millis(20);
        loop {
            match self.node_call("lease-log", &format!("lease-log {} {n} {off}", self.id)) {
                Ok(v) => {
                    failing = None;
                    let data = crate::lease::master::b64_decode(&s(&v, "data")).ok_or("lease-log: bad data")?;
                    let _ = log.write_all(&data);
                    if !data.is_empty() {
                        quiet = Duration::from_millis(20);
                    }
                    off = v.get("off").and_then(crate::lease::num).map_or(off + data.len() as u64, |x| x as u64);
                    if data.is_empty() {
                        if let Some(code) = v.get("exit").filter(|e| !e.is_null()).and_then(crate::lease::num) {
                            let _ = log.flush();
                            return self.verdict(code as i32);
                        }
                        std::thread::sleep(quiet);
                        quiet = (quiet * 2).min(Duration::from_millis(500));
                    }
                }
                Err(e) => {
                    let t = *failing.get_or_insert_with(Instant::now);
                    if t.elapsed() > NODE_GRACE {
                        return Err(e);
                    }
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
            if beat.elapsed() >= BEAT {
                beat = Instant::now();
                let st = first(ask(&self.ctl, None, "beat", &self.id), "beat")?;
                if s(&st, "state") == "ended" {
                    return Err(format!("{} ended ({})", self.id, s(&st, "how")));
                }
            }
        }
    }

    /// A run's exit as evidence: none when the node did not see its end, or when it was killed
    /// with its lease (a lease ended by the user, by preemption or for lost beats).
    fn verdict(&self, code: i32) -> Result<i32, String> {
        if code == EXIT_UNKNOWN {
            return Err(format!("{}: {}'s hub did not see the run end", self.id, self.node));
        }
        if code != 0 {
            let st = first(ask(&self.ctl, None, "beat", &self.id), "beat")?;
            if s(&st, "state") == "ended" {
                return Err(format!("{} ended ({})", self.id, s(&st, "how")));
            }
        }
        Ok(code)
    }

    /// Runs `argv` and returns what it printed (short commands: `git rev-parse`).
    pub fn output(&self, argv: &[&str]) -> Result<(i32, String), String> {
        let mut buf = Vec::new();
        let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
        let code = self.run(&argv, &mut buf)?;
        Ok((code, String::from_utf8_lossy(&buf).trim().to_string()))
    }

    /// Copies a file or directory while this lease is live, through the node's get services.
    /// Like `devhub get`, the last component of `rel` is kept under `dst`. Chunks stream to
    /// disk; a short or malformed transfer fails instead of leaving usable partial evidence.
    pub fn get(&self, lane: &str, rel: &str, dst: &Path) -> Result<BTreeSet<PathBuf>, String> {
        let inside = |p: &str| !p.is_empty() && !p.contains(['\r', '\n', '\0', '\\']) && Path::new(p).components().all(|c| matches!(c, Component::Normal(_)));
        if !crate::trust::valid_name(lane) || !inside(rel) {
            return Err(format!("get: invalid lane or path {rel}"));
        }
        let lines = ask(&self.ctl, Some(&self.node), "get-ls", &format!("get-ls {lane} {} {rel}", self.id)).map_err(|e| format!("{}: {e}", self.node))?;
        let mut copied = BTreeSet::new();
        for line in lines {
            let v = first(Ok(vec![line]), "get-ls")?;
            let (p, r) = (s(&v, "p"), s(&v, "r"));
            if !inside(&p) || !inside(&r) || (r != rel && !r.starts_with(&format!("{rel}/"))) {
                return Err(format!("{}: get-ls: bad path {p} ({r})", self.node));
            }
            let size = v.get("n").and_then(Value::as_u64).ok_or("get-ls: missing size")?;
            let out = dst.join(&p);
            if !copied.insert(out.clone()) {
                return Err(format!("get-ls: duplicate path {p}"));
            }
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
            let mut f = std::fs::File::create(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            let mut off = 0;
            loop {
                let v = self.node_call("get-read", &format!("get-read {lane} {} {off} {r}", self.id))?;
                let data = v.get("data").and_then(Value::as_str).and_then(crate::lease::master::b64_decode).ok_or("get-read: bad data")?;
                let next = v.get("off").and_then(Value::as_u64).ok_or("get-read: missing offset")?;
                let eof = v.get("eof").and_then(Value::as_bool).ok_or("get-read: missing eof")?;
                if next != off + data.len() as u64 || next > size || (data.is_empty() && !eof) || (eof && next != size) {
                    return Err(format!("{}: get-read: incomplete or inconsistent file {r}", self.node));
                }
                f.write_all(&data).map_err(|e| format!("{}: {e}", out.display()))?;
                off = next;
                if eof {
                    break;
                }
            }
        }
        if copied.is_empty() {
            return Err(format!("{}: nothing under {rel}", self.node));
        }
        Ok(copied)
    }

    /// The master's control client this lease is held through (node calls go through it).
    pub fn client(&self) -> &Client {
        &self.ctl
    }

    /// Lets go of the lease here without ending it (another holder follows its run).
    pub fn keep(mut self) {
        self.released = true;
    }

    pub fn release(mut self, exit: Option<i32>) {
        self.end(exit);
    }

    fn end(&mut self, exit: Option<i32>) {
        if self.released {
            return;
        }
        self.released = true;
        let line = match exit {
            Some(c) => format!("{} released exit={c}", self.id),
            None => self.id.clone(),
        };
        let _ = ask(&self.ctl, None, "release", &line);
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        self.end(None);
    }
}
