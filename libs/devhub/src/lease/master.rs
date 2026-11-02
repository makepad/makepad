//! The master's lease services (control socket): `lease`, `beat`, `release`, `leases`, `nodes`,
//! `node-dev`, `linspect`. It runs the [`Sched`], sets leases up and tears them down on the nodes
//! (`lease-start`, `lease-end`), beats the nodes' live leases (`lease-beat`), refreshes their
//! `caps`, pushes the dev switch (`policy`), keeps the ledger (`~/.makepad/devhub/leases.jsonl`)
//! and publishes every decision as a `lease` record.
//!
//! Nodes are reached only through [`Ask`] (production: `hub::ask` on the control socket's router).

use super::sched::{Action, Lease, NodeView, Req, Sched, State};
use super::{Caps, CostClass, Kind};
use crate::proto::{self, hub_record, Record, Sev};
use makepad_remote::json_str;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `(node, service, line)` -> the answer lines (without the end line).
pub type Ask = Arc<dyn Fn(&str, &str, &str) -> Result<Vec<String>, String> + Send + Sync>;
pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;
/// `(node, repo, repo dir, branch)` -> that commit, present in the node's store.
pub type EnsureRev = Arc<dyn Fn(&str, &str, &Path, &super::RevPin) -> Result<String, String> + Send + Sync>;
/// The nodes the master dials: (name, address).
pub type Hosts = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

pub const SERVICES: [&str; 17] = ["lease", "beat", "release", "cancel", "leases", "why-queued", "nodes", "node-dev", "linspect", "evict", "stop", "unblock", "override", "blocked", "jit", "stats", "prof"];
const NODE_BEAT_MS: u64 = 5_000;
/// How long a commit sent to a node's store counts as there without asking again.
#[cfg(feature = "checkout")]
const ENSURED_FOR: std::time::Duration = std::time::Duration::from_secs(600);
const CAPS_MS: u64 = 10_000;

/// What an inspect lease left on a JIT (undone on the node at `lease-end`).
#[derive(Clone, Debug, Default)]
struct Marks {
    jit: String,
    breaks: Vec<String>,
    probes: Vec<String>,
    paused: bool,
}

pub struct LeaseMaster {
    machine: String,
    sched: Mutex<Sched>,
    ask: Ask,
    publish: Publish,
    clock: Clock,
    ensure_rev: Option<EnsureRev>,
    home: PathBuf,
    costs_offset: Mutex<u64>,
    profile_writer: Mutex<u64>,
    /// tests: node calls run on the caller's thread
    inline: bool,
    marks: Mutex<HashMap<String, Marks>>,
    /// (node, jit) -> the lease that holds its breakpoints
    holders: Mutex<HashMap<(String, String), String>>,
    /// the dev switch each node was last told (a role change on the node moves its default)
    pushed: Mutex<HashMap<String, bool>>,
    /// per node: the lifetime JIT builds and build seconds its store reported last (the ledger gets the deltas)
    jit_seen: Mutex<HashMap<String, (u64, f64)>>,
    /// how each ended lease's cleanup went (`release` waits for it and says so)
    cleanups: Mutex<HashMap<String, String>>,
    cleaned: std::sync::Condvar,
    /// bumped on every change of the live leases (a grant, a start, an end): an in-process waiter
    /// for its grant wakes on it instead of polling
    changes: (Mutex<u64>, std::sync::Condvar),
    /// the last text written to `leases.live` ([`Self::persist`])
    saved: Mutex<String>,
}

/// A node call that failed because the node's hub could not be reached (not a refusal).
pub fn unreachable(e: &str) -> bool {
    ["link lost", "cannot reach", "Connection refused", "not connected", "no route to"].iter().any(|w| e.contains(w))
}

/// A lane's node dirs (`lane/<lane>/src`, its target) hold the tree of ONE Mac worktree: a lease
/// without a revision ships into them. `~/.makepad/devhub/lanes.conf` (`<lane> <worktree>` lines)
/// binds a lane to the worktree that first shipped under its name; a lease for the same lane from
/// another worktree is refused (two worktrees alternating in one node dir left it stale and gates
/// ran old files). A binding whose worktree is gone (removed after landing) passes to the next.
fn bind_lane(home: &Path, req: &Req) -> Result<(), String> {
    let (None, Some(dir), false) = (&req.rev, &req.repo_dir, req.kind == super::Kind::Inspect) else { return Ok(()) };
    // one at a time: leases asked at once renamed the one tmp file away from each other
    static BINDING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = BINDING.lock().unwrap_or_else(|e| e.into_inner());
    let path = home.join("lanes.conf");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<(String, String)> = text.lines().filter_map(|l| l.split_once(' ').map(|(a, b)| (a.to_string(), b.trim().to_string()))).collect();
    match lines.iter_mut().find(|(lane, _)| *lane == req.lane) {
        Some((_, bound)) if bound == dir => return Ok(()),
        Some((_, bound)) if Path::new(bound.as_str()).exists() => {
            return Err(format!(
                "lane {} already ships from {bound}; this worktree ({dir}) needs its own lane name (two worktrees in one node lane dir leave it stale), or remove the other worktree first",
                req.lane
            ));
        }
        Some((_, bound)) => *bound = dir.clone(),
        None => lines.push((req.lane.clone(), dir.clone())),
    }
    let out: String = lines.iter().map(|(l, d)| format!("{l} {d}\n")).collect();
    let tmp = home.join("lanes.conf.new");
    std::fs::create_dir_all(home).and_then(|_| std::fs::write(&tmp, out)).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))
}

fn err(e: &str) -> String {
    format!("{{\"err\":{}}}", json_str(e))
}

/// The dev switches in `nodes.conf` (`<node> ... dev=on|off`).
fn read_dev(path: &Path) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut w = line.split_whitespace();
        let Some(name) = w.next() else { continue };
        if let Some(d) = w.find_map(|kv| kv.strip_prefix("dev=")) {
            out.insert(name.to_string(), d == "on");
        }
    }
    out
}

/// Sets `dev=` on the node's line of `nodes.conf` (appends a line for a node it lacks).
fn write_dev(path: &Path, node: &str, on: bool) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| {
            let body = l.split('#').next().unwrap_or("");
            if body.split_whitespace().next() != Some(node) {
                return l.to_string();
            }
            found = true;
            let mut w: Vec<String> = body.split_whitespace().filter(|kv| !kv.starts_with("dev=")).map(str::to_string).collect();
            w.push(format!("dev={}", if on { "on" } else { "off" }));
            let comment = l.find('#').map(|i| format!(" {}", &l[i..])).unwrap_or_default();
            format!("{}{comment}", w.join(" "))
        })
        .collect();
    if !found {
        lines.push(format!("{node} dev={}", if on { "on" } else { "off" }));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("conf.new");
    std::fs::write(&tmp, format!("{}\n", lines.join("\n")))?;
    std::fs::rename(&tmp, path)
}

/// The user's block list (`blocked`: one lane or agent per line).
/// `~/.makepad/devhub/desk-links`: the Desk Link endpoints the nodes report in their caps (JIT
/// hosts that serve their own render), one `<node> <ip>:<port>` line each, for the viewer
/// (apps/desklink lists them as desktops). Rewritten when it changes; empty when none listens.
pub fn desk_links_path(home: &Path) -> PathBuf {
    home.join("desk-links")
}

pub fn desk_links(nodes: &[NodeView]) -> String {
    let mut text = String::new();
    for n in nodes.iter().filter(|n| n.up) {
        let Some(d) = n.caps.as_ref().and_then(|c| c.desk_link.as_ref()).filter(|d| d.on && d.port > 0) else { continue };
        let ip = n.addr.rsplit_once(':').map_or(n.addr.as_str(), |(h, _)| h);
        if !ip.is_empty() {
            text.push_str(&format!("{} {ip}:{}\n", n.name, d.port));
        }
    }
    text
}

fn write_desk_links(home: &Path, nodes: &[NodeView]) {
    let path = desk_links_path(home);
    let text = desk_links(nodes);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        let tmp = path.with_extension("new");
        let _ = std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, &path));
    }
}

fn read_blocked(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join("blocked")).unwrap_or_default().lines().map(str::trim).filter(|l| crate::trust::valid_name(l)).map(str::to_string).collect()
}

fn write_blocked(home: &Path, list: &[String]) {
    let _ = std::fs::create_dir_all(home);
    let tmp = home.join("blocked.new");
    if std::fs::write(&tmp, list.iter().map(|l| format!("{l}\n")).collect::<String>()).is_ok() {
        let _ = std::fs::rename(&tmp, home.join("blocked"));
    }
}

/// The next lease number after the ledger's highest.
fn next_id(ledger: &Path) -> u64 {
    let text = std::fs::read_to_string(ledger).unwrap_or_default();
    text.lines().filter_map(|l| l.split("\"id\":\"L").nth(1)?.split('"').next()?.parse::<u64>().ok()).max().map_or(1, |n| n + 1)
}

/// Standard base64 (the node's `lease-log` / `recover-get` data).
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let b: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(b.len() * 3 / 4);
    for ch in b.chunks(4) {
        let mut n = 0u32;
        for (i, c) in ch.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if ch.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if ch.len() > 3 {
            out.push(n as u8);
        }
        if ch.len() == 1 {
            return None;
        }
    }
    Some(out)
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn strs(v: Option<&makepad_strict_json::Value>) -> Vec<String> {
    v.and_then(|a| a.as_arr()).map(|a| a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| super::num(x).map(|n| (n as i64).to_string()).unwrap_or_default())).collect()).unwrap_or_default()
}

/// Waits completed by grants in the last hour; live grants supplement the ended ledger.
fn queue_stats(path: &Path, leases: &[Lease], now: u64) -> Result<String, String> {
    #[derive(Default)]
    struct Counts { granted: usize, queued: usize, waits: usize, sum: u128, max: u64 }
    impl Counts {
        fn line(&self, class: &str) -> String {
            let (avg, max) = if self.waits == 0 { ("-".into(), "-".into()) } else {
                (format!("{:.1}s", self.sum as f64 / self.waits as f64 / 1000.0), format!("{:.1}s", self.max as f64 / 1000.0))
            };
            format!("{class:<8} {:>11} {:>10} {avg:>10} {max:>10} {:>7}\n", self.granted, self.queued, self.waits)
        }
    }
    let since = now.saturating_sub(3_600_000);
    let mut waits = HashMap::new();
    let file = match std::fs::File::open(path) {
        Ok(f) => Some(f),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if let Some(file) = file {
        for line in std::io::BufReader::new(file).lines() {
            let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
            let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
            let n = |k| v.get(k).and_then(|v| v.as_u64());
            let (Some(id), Some(asked), Some(granted)) = (v.get("id").and_then(|v| v.as_str()), n("asked_ms"), n("granted_ms")) else { continue };
            if granted < since || granted > now || asked > granted { continue; }
            let class = v.get("cost").and_then(|v| v.as_str()).and_then(CostClass::parse)
                .or_else(|| v.get("req").and_then(|r| Req::parse(&r.to_json()).ok()).map(|r| r.asked_cost()))
                .or_else(|| v.get("kind").and_then(|v| v.as_str()).and_then(Kind::parse).map(CostClass::default_for));
            if let Some(class) = class { waits.insert(id.to_string(), (class, granted - asked)); }
        }
    }
    let mut counts: BTreeMap<_, Counts> = [CostClass::Small, CostClass::Medium, CostClass::Large].into_iter().map(|c| (c, Counts::default())).collect();
    for l in leases {
        let c = counts.get_mut(&l.cost_class).unwrap();
        match l.state {
            State::Starting | State::Granted => c.granted += 1,
            State::Queued => c.queued += 1,
            State::Ended(_) => {}
        }
        if let Some(granted) = l.granted_ms.filter(|g| *g >= since && *g <= now && l.asked_ms <= *g) {
            waits.insert(l.id.clone(), (l.cost_class, granted - l.asked_ms));
        }
    }
    for (class, wait) in waits.into_values() {
        let c = counts.get_mut(&class).unwrap();
        c.waits += 1;
        c.sum += wait as u128;
        c.max = c.max.max(wait);
    }
    let mut text = "queue now; waits for grants in the last hour (request to grant)\n".to_string();
    text += &format!("{:<8} {:>11} {:>10} {:>10} {:>10} {:>7}\n", "class", "granted now", "queued now", "avg wait", "max wait", "samples");
    let mut total = Counts::default();
    for (class, c) in counts {
        text += &c.line(class.as_str());
        total.granted += c.granted;
        total.queued += c.queued;
        total.waits += c.waits;
        total.sum += c.sum;
        total.max = total.max.max(c.max);
    }
    text += &total.line("total");
    Ok(text)
}

impl LeaseMaster {
    pub fn new(machine: &str, home: PathBuf, ask: Ask, publish: Publish, clock: Clock, ensure_rev: Option<EnsureRev>, inline: bool) -> LeaseMaster {
        // ids a previous master gave out and no file names yet (a grant in flight at its exec) stay
        // unused: a holder of one would take a new lease's place (a lane's run in fabmove's L9253)
        let next = next_id(&home.join("leases.jsonl"));
        let next = if next > 1 { next + 1000 } else { next };
        // the leases a previous master granted and did not end: their holders and nodes still
        // run them (a restart of the master must not cost them their work)
        let mut sched = Sched::new(machine, next);
        let now = clock();
        sched.nodes_settle_ms = now + super::sched::RESTORE_GRACE_MS;
        for line in std::fs::read_to_string(home.join("leases.live")).unwrap_or_default().lines() {
            sched.restore(line, now);
        }
        sched.blocked = read_blocked(&home);
        // A restart closes the last sampled interval at its last tick, not at startup.
        let profile_live = home.join("profile.live");
        if let Ok(live) = std::fs::read_to_string(&profile_live) {
            match std::fs::OpenOptions::new().create(true).append(true).open(home.join("profile.jsonl")).and_then(|mut f| f.write_all(live.as_bytes())) {
                Ok(()) => { let _ = std::fs::remove_file(&profile_live); }
                Err(e) => eprintln!("devhub: profile recovery: {e}"),
            }
        }
        LeaseMaster {
            machine: machine.to_string(),
            sched: Mutex::new(sched),
            ask,
            publish,
            clock,
            ensure_rev,
            home,
            costs_offset: Mutex::new(0),
            profile_writer: Mutex::new(0),
            inline,
            marks: Mutex::new(HashMap::new()),
            holders: Mutex::new(HashMap::new()),
            pushed: Mutex::new(HashMap::new()),
            jit_seen: Mutex::new(HashMap::new()),
            cleanups: Mutex::new(HashMap::new()),
            cleaned: std::sync::Condvar::new(),
            changes: (Mutex::new(0), std::sync::Condvar::new()),
            saved: Mutex::new(String::new()),
        }
    }

    fn cleaned_up(&self, id: &str, summary: String) {
        let mut c = self.cleanups.lock().unwrap();
        if c.len() > 256 {
            c.clear();
        }
        c.insert(id.to_string(), summary);
        self.cleaned.notify_all();
    }

    /// The cleanup summary of lease `id`, waiting up to `wait` for it.
    pub fn cleanup_of(&self, id: &str, wait: Duration) -> Option<String> {
        let end = std::time::Instant::now() + wait;
        let mut c = self.cleanups.lock().unwrap();
        loop {
            if let Some(s) = c.get(id) {
                return Some(s.clone());
            }
            let left = end.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            c = self.cleaned.wait_timeout(c, left).unwrap().0;
        }
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// `leases.live`: the granted leases, rewritten whole on every grant and end.
    /// The live leases' change count (see [`Self::wait_change`]).
    pub fn generation(&self) -> u64 {
        *self.changes.0.lock().unwrap()
    }

    /// Waits up to `t` for a change of the live leases after `seen` (a [`Self::generation`]).
    pub fn wait_change(&self, seen: u64, t: Duration) {
        let g = self.changes.0.lock().unwrap();
        let _ = self.changes.1.wait_timeout_while(g, t, |n| *n == seen);
    }

    fn save_live(&self) {
        *self.changes.0.lock().unwrap() += 1;
        self.changes.1.notify_all();
        self.persist();
    }

    /// Writes `leases.live` when its text changed: every state change reaches the file at once
    /// (not after a node's teardown), so a restart in between neither loses a lease nor brings an
    /// ended one back.
    fn persist(&self) {
        let lines = self.sched.lock().unwrap().live_lines();
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let mut saved = self.saved.lock().unwrap();
        if *saved == text { return; }
        let _ = std::fs::create_dir_all(&self.home);
        let (path, tmp) = (self.home.join("leases.live"), self.home.join("leases.live.new"));
        if std::fs::write(&tmp, &text).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            *saved = text;
        }
    }

    /// See [`Sched::timed_series`].
    pub fn timed_series(&self, dir: &str) -> Option<String> {
        let now = self.now();
        self.sched.lock().unwrap().timed_series(dir, now)
    }

    /// Why the master should not exec into a new build now: a lease between states.
    pub fn in_transition(&self) -> Option<String> {
        self.sched.lock().unwrap().in_transition()
    }

    /// Asks the nodes again to start the leases a previous master left `starting`.
    pub fn resume_starts(self: &Arc<Self>) {
        let list = std::mem::take(&mut self.sched.lock().unwrap().resume);
        for (id, node) in list {
            self.spawn(move |m| m.grant(&id, &node));
        }
    }

    fn event(&self, sev: Sev, line: String) {
        (self.publish)(hub_record(&self.machine, "", 0, "lease", sev, line, String::new()));
    }

    fn ask1(&self, node: &str, service: &str, line: &str) -> Result<makepad_strict_json::Value, String> {
        let lines = (self.ask)(node, service, line)?;
        let first = lines.first().ok_or_else(|| format!("{node} {service}: no answer"))?;
        let v = makepad_strict_json::parse(first.as_bytes()).map_err(|_| format!("{node} {service}: bad answer"))?;
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return Err(format!("{node} {service}: {e}"));
        }
        Ok(v)
    }

    /// Runs a node call off the caller's thread (inline in tests).
    fn spawn(self: &Arc<Self>, f: impl FnOnce(&Arc<Self>) + Send + 'static) {
        if self.inline {
            return f(self);
        }
        let me = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease".into()).spawn(move || f(&me));
    }

    /// Tail complete cost records, including per-id JIT builds appended between lease ends.
    fn refresh_costs(&self) {
        let mut offset = self.costs_offset.lock().unwrap();
        let Ok(mut file) = std::fs::File::open(self.home.join("costs.jsonl")) else { return };
        if file.metadata().is_ok_and(|m| m.len() < *offset) { *offset = 0; }
        if file.seek(std::io::SeekFrom::Start(*offset)).is_err() { return; }
        let mut reader = std::io::BufReader::new(file);
        let mut records = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).is_err() || !line.ends_with('\n') { break; }
            *offset += line.len() as u64;
            if let Ok(v) = makepad_strict_json::parse(line.as_bytes()) {
                if let (Some(class), Some(wall)) = (v.get("class").and_then(|v| v.as_str()), v.get("wall_ms").and_then(super::num)) {
                    if wall.is_finite() && wall > 0.0 {
                        let kind = v.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                        let peak = v.get("peak_ram_gb").and_then(super::num);
                        let cores = v.get("cpu_seconds").and_then(super::num).map(|cpu| cpu * 1000.0 / wall);
                        records.push((class.to_string(), kind.to_string(), v.get("node").and_then(|v| v.as_str()).unwrap_or("").to_string(), wall as u64, peak, cores));
                    }
                }
            }
        }
        let mut sched = self.sched.lock().unwrap();
        for (class, kind, node, wall, peak, cores) in records {
            sched.record_node_cost(&class, &kind, &node, wall);
            sched.record_resources(&class, &kind, peak, cores);
        }
    }

    /// Places what fits and carries out the scheduler's actions.
    pub fn pump(self: &Arc<Self>) {
        self.refresh_costs();
        let now = self.now();
        #[cfg(feature = "checkout")]
        let interactive_lanes = crate::orch::ctx().map(|ctx| ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live" && l.waited).map(|l| l.name.clone()).collect())).unwrap_or_default();
        #[cfg(feature = "checkout")]
        let live_jobs: std::collections::HashSet<String> = crate::orch::ctx().map(|ctx| ctx.store.read(|d| d.lives.iter().filter(|x| x.active() && !x.lease.is_empty()).map(|x| x.lease.clone()).collect())).unwrap_or_default();
        let acts = {
            let mut s = self.sched.lock().unwrap();
            #[cfg(feature = "checkout")]
            { s.interactive_lanes = interactive_lanes; s.live_jobs = live_jobs; }
            for id in s.holders_gone(&|pid| crate::procinfo::alive(pid)) {
                s.release(&id, "holder-dead", now);
            }
            s.tick(now)
        };
        self.act(acts);
        self.persist();
    }

    fn act(self: &Arc<Self>, acts: Vec<Action>) {
        for a in &acts {
            if let Action::Profile { closed, live, seq } = a {
                let mut writer = self.profile_writer.lock().unwrap();
                let written = (|| -> std::io::Result<()> {
                    std::fs::create_dir_all(&self.home)?;
                    if !closed.is_empty() { std::fs::OpenOptions::new().create(true).append(true).open(self.home.join("profile.jsonl"))?.write_all(closed.as_bytes())?; }
                    if *seq > *writer {
                        let tmp = self.home.join("profile.live.new");
                        std::fs::write(&tmp, live)?;
                        std::fs::rename(&tmp, self.home.join("profile.live"))?;
                        *writer = *seq;
                    }
                    Ok(())
                })();
                if let Err(e) = written { self.event(Sev::Warn, format!("workflow profile: {e}")); }
            }
        }
        for a in acts {
            match a {
                Action::Profile { .. } => {}
                Action::Event(r) => (self.publish)(r),
                Action::Grant { id, node } => {
                    let inspect = self.sched.lock().unwrap().get(&id).is_some_and(|l| !l.req.on_node());
                    if inspect {
                        // nothing to set up: an inspect lease is the right to talk to a JIT
                        self.sched.lock().unwrap().started(&id, "{}");
                        self.save_live();
                    } else {
                        self.spawn(move |m| m.grant(&id, &node));
                    }
                }
                Action::End { id, node, how } => self.spawn(move |m| m.end(&id, node.as_deref(), &how)),
                Action::ParkLive { leases, note } => {
                    #[cfg(feature = "checkout")]
                    if let Some(ctx) = crate::orch::ctx() { crate::orch::live::park(&ctx, &leases, &note); }
                    #[cfg(not(feature = "checkout"))]
                    let _ = (leases, note);
                }
                Action::Reclaim { node, need_gb } => self.spawn(move |m| {
                    if let Err(e) = m.ask1(&node, "disk", &format!("disk need {need_gb:.0}")) {
                        m.event(Sev::Warn, format!("node {node}: disk reclaim failed: {e}"));
                    }
                }),
            }
        }
    }

    /// A parked live job has copied its last answer. Teardown runs off the caller's thread.
    #[cfg(feature = "checkout")]
    pub fn release_live(self: &Arc<Self>, id: &str) {
        self.sched.lock().unwrap().release(id, "released", self.now());
        self.save_live();
        self.pump();
    }

    /// `lease-start` on the node (after making its revisions present there).
    fn grant(self: &Arc<Self>, id: &str, node: &str) {
        let Some(l) = self.sched.lock().unwrap().get(id).cloned() else { return };
        let t0 = std::time::Instant::now();
        let mut ship = 0u64;
        let r = self.start_on(&l, node, &mut ship);
        let setup = t0.elapsed().as_millis() as u64;
        let now = self.now();
        self.sched.lock().unwrap().phase(id, "sync", l.granted_ms.unwrap_or(now.saturating_sub(setup)), now);
        match r {
            Ok(answer) => {
                let mut s = self.sched.lock().unwrap();
                if !s.started(id, &answer) {
                    drop(s);
                    // (it would hold the box's tree against every lease after it)
                    if let Err(e) = self.ask1(node, "lease-end", &format!("lease-end {id} released")) {
                        self.event(Sev::Warn, format!("{} ended while it started on {node}, and its end there failed: {e}", l.tag()));
                    }
                    self.pump();
                    return;
                }
                s.set_up(id, setup, ship);
                let wait = s.get(id).and_then(|l| l.wait_ms);
                drop(s);
                let secs = |ms: u64| format!("{:.1} s", ms as f64 / 1000.0);
                self.event(Sev::Info, format!("lease_latency: {} of {} on {node}: {}setup {} (ship {})", l.tag(), l.req.lane, wait.map_or(String::new(), |w| format!("release->grant {}, ", secs(w))), secs(setup), secs(ship)));
            }
            // the box's hub is not there (restarting for its update): placed again, not failed
            Err(e) if unreachable(&e) => self.sched.lock().unwrap().unreachable(id, &e),
            // the box's hub is between its runs, restarting for its self-update: placed again (the
            // lease queues, never fails for a roll)
            Err(e) if e.contains("restarting: ") => self.sched.lock().unwrap().tree_busy(id, &e),
            // the box's previous lease is still leaving its tree: placed again, not failed
            Err(e) if e.contains("the tree is at ") || e.contains("overlay lane ") || e.contains("native trees busy:") => self.sched.lock().unwrap().tree_busy(id, &e),
            Err(e) => self.sched.lock().unwrap().failed(id, &e, now),
        }
        self.save_live();
        self.pump();
    }

    fn start_on(&self, l: &Lease, node: &str, ship_ms: &mut u64) -> Result<String, String> {
        let rev = match &l.req.rev {
            None => "null".to_string(),
            Some(spec) => {
                // the commits the CLI pinned at order time, never what the branches name now
                let (mk, com) = super::parse_pinned(spec)?;
                let dir = PathBuf::from(l.req.repo_dir.clone().unwrap_or_else(|| ".".into()));
                let f = self.ensure_rev.as_ref().ok_or("this master cannot ship revisions (built without checkout)")?;
                let t = std::time::Instant::now();
                let error = |repo: &str, sha: &str, e: String| if l.req.src == super::Src::Overlay { format!("overlay: {repo} {sha} bare transfer layer: {}", e.replace(['\n', '\r'], " | ")) } else { e };
                let mk = f(node, "makepad", &dir, &mk).map_err(|e| error("makepad", &mk.sha, e))?;
                let com = match com {
                    Some(c) => json_str(&f(node, "commercial", &dir.join("apps/commercial"), &c).map_err(|e| error("commercial", &c.sha, e))?),
                    None => "null".into(),
                };
                *ship_ms = t.elapsed().as_millis() as u64;
                format!("{{\"makepad\":{},\"commercial\":{com}}}", json_str(&mk))
            }
        };
        let needs: Vec<String> = l.req.needs.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        let line = format!("lease-start {{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"src\":\"{}\",\"priority\":\"{}\",\"loader\":{},\"cost\":{{\"cores\":{},\"ram_gb\":{}}},\"needs\":{{{}}},\"rev\":{rev},\"env\":{{}}}}", json_str(&l.id), json_str(&l.req.lane), l.req.kind.as_str(), l.req.src.as_str(), l.priority.as_str(), l.req.loader.as_deref().map_or("null".to_string(), json_str), l.cost.cores, l.cost.ram_gb, needs.join(","));
        let lines = (self.ask)(node, "lease-start", &line)?;
        let first = lines.into_iter().next().ok_or("lease-start: no answer")?;
        let v = makepad_strict_json::parse(first.as_bytes()).map_err(|_| "lease-start: bad answer".to_string())?;
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return Err(format!("{node} refused: {e}"));
        }
        Ok(first)
    }

    /// Record costs before freeing the node, so its next placement sees this wall.
    fn ended_cost(&self, l: &Lease, run_class: &str, usage: Option<&makepad_strict_json::Value>) {
        let class = if !l.req.class.is_empty() { l.req.class.as_str() } else if !run_class.is_empty() { run_class } else { l.req.kind.as_str() };
        let wall = l.ended_ms.unwrap_or_else(|| self.now()).saturating_sub(l.granted_ms.unwrap_or(l.ended_ms.unwrap_or(l.asked_ms)));
        let ended = l.ended_ms.unwrap_or_else(|| self.now());
        let mut phases = l.profile();
        phases.close("report", ended);
        let measured = |key| usage.and_then(|v| v.get(key)).and_then(super::num).filter(|n| n.is_finite() && *n >= 0.0).map_or("null".into(), |n| n.to_string());
        let line = format!("{{\"class\":{},\"kind\":\"{}\",\"cost\":\"{}\",\"wall_ms\":{wall},\"peak_ram_gb\":{},\"cpu_seconds\":{},\"node\":{},\"lane\":{},\"ended_ms\":{ended},\"lease\":{}, {}}}\n", json_str(class), l.req.kind.as_str(), l.cost_class.as_str(), measured("peak_ram_gb"), measured("cpu_seconds"), json_str(l.node.as_deref().unwrap_or("")), json_str(&l.req.lane), json_str(&l.id), phases.fields(ended));
        let _ = std::fs::create_dir_all(&self.home);
        // File appends and the rolling window have the same order, including concurrent ends.
        let offset = self.costs_offset.lock().unwrap();
        let written = std::fs::OpenOptions::new().create(true).append(true).open(self.home.join("costs.jsonl")).and_then(|mut f| f.write_all(line.as_bytes()));
        drop(offset);
        self.refresh_costs();
        if let Err(e) = written { self.event(Sev::Warn, format!("costs.jsonl: {e}")); }
    }

    /// `lease-end` on the node, the inspect marks undone, recovered work fetched, the ledger line.
    fn end(self: &Arc<Self>, id: &str, node: Option<&str>, how: &str) {
        let l = self.sched.lock().unwrap().get(id).cloned();
        let report_start = l.as_ref().and_then(|l| l.ended_ms).unwrap_or_else(|| self.now());
        let mut node_phases = l.as_ref().map(|l| l.node_phases.clone()).unwrap_or_default();
        for phase in ["sync", "jit", "run"] { node_phases.close(phase, report_start); }
        let mut run_class = String::new();
        let mut cost_recorded = false;
        let marks = self.marks.lock().unwrap().remove(id);
        self.holders.lock().unwrap().retain(|_, v| v != id);
        let mut summary = "{\"cleanup\":\"nothing to clean\"}".to_string();
        let (mut teardown, mut freed_early) = (0u64, false);
        if let (Some(node), Some(l)) = (node, l.as_ref()) {
            // an inspect lease that changed nothing has nothing to undo
            if l.req.on_node() || marks.is_some() {
                let wire_how = match how {
                    "released" | "expired" | "holder-dead" | "denied" => how,
                    "lost" => "expired",
                    _ => "released",
                };
                let mut parts = Vec::new();
                if let Some(m) = marks {
                    let list = |v: &[String]| v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",");
                    let pid = m.jit.parse::<u32>().map_or("null".to_string(), |p| p.to_string());
                    parts.push(format!("\"inspect\":{{\"pid\":{pid},\"jit\":{},\"breaks\":[{}],\"probes\":[{}],\"paused\":{}}}", json_str(&m.jit), list(&m.breaks), list(&m.probes), m.paused));
                }
                let payload = if parts.is_empty() { String::new() } else { format!(" {{{}}}", parts.join(",")) };
                let t0 = std::time::Instant::now();
                let answer = self.ask1(node, "lease-end", &format!("lease-end {id} {wire_how}{payload}"));
                teardown = t0.elapsed().as_millis() as u64;
                let class = answer.as_ref().ok().and_then(|v| v.get("class")).and_then(|v| v.as_str()).unwrap_or("");
                run_class = class.to_string();
                if let Some(v) = answer.as_ref().ok().filter(|v| v.get("phases").is_some()) {
                    node_phases = super::prof::Phases::parse(v.get("phases"));
                }
                let mut measured = l.clone();
                measured.node_phases = node_phases.clone();
                self.ended_cost(&measured, class, answer.as_ref().ok());
                cost_recorded = true;
                // the box is free the moment its lease-end answers: grant what waits for it now, the
                // bookkeeping (recovered work, ledger) after
                self.sched.lock().unwrap().cleaned(node, id, teardown);
                freed_early = true;
                self.pump();
                match answer {
                    Ok(v) => {
                        let leak = strs(v.get("leak"));
                        let killed = strs(v.get("killed"));
                        let freed = v.get("freed_mb").and_then(super::num).unwrap_or(0.0);
                        if !leak.is_empty() {
                            self.event(Sev::Err, format!("lease_leak: {} of {}: {} processes left after cleanup on {node} (pids {})", l.tag(), l.req.lane, leak.len(), leak.join(" ")));
                        }
                        self.event(Sev::Info, format!("{} of {}: cleaned up on {node}: {} stopped, {freed:.0} MB freed{}", l.tag(), l.req.lane, plural(killed.len(), "process", "processes"), if leak.is_empty() { String::new() } else { format!(", {} LEFT", leak.len()) }));
                        summary = format!("{{\"cleanup\":\"ok\",\"node\":{},\"stopped\":{},\"freed_mb\":{freed:.0},\"leak\":{}}}", json_str(node), killed.len(), leak.len());
                        if let Some(path) = v.get("recovered").and_then(|r| r.as_str()) {
                            self.fetch_recovered(node, path, l);
                        }
                    }
                    Err(e) => {
                        self.event(Sev::Warn, format!("{} of {}: cleanup on {node} failed: {e}", l.tag(), l.req.lane));
                        summary = format!("{{\"cleanup\":{}}}", json_str(&format!("failed on {node}: {e}")));
                    }
                }
            }
        }
        // a lease with nothing to tear down: its node is free now
        if let (Some(node), false) = (node, freed_early) {
            self.sched.lock().unwrap().cleaned(node, id, teardown);
        }
        self.save_live();
        let reported = self.now();
        self.sched.lock().unwrap().reported(id, node_phases.clone(), reported);
        // the ledger line carries the latencies the scheduler recorded
        let l = self.sched.lock().unwrap().get(id).cloned().or(l);
        if let Some(mut l) = l {
            l.node_phases = node_phases;
            l.phases.push("report", report_start, reported);
            l.reported_ms = Some(reported);
            if l.req.class.is_empty() { l.req.class = run_class; }
            if !cost_recorded { self.ended_cost(&l, "", None); }
            let path = self.home.join("leases.jsonl");
            let _ = std::fs::create_dir_all(&self.home);
            let line = format!("{}\n", l.ledger_line());
            if let Err(e) = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(line.as_bytes())) { self.event(Sev::Warn, format!("leases.jsonl: {e}")); }
        }
        self.cleaned_up(id, summary);
        self.pump();
    }

    /// Copies a recovery bundle off the node and fetches it into the lane's repo as
    /// `devhub/recovered/<lane>-<id>`.
    fn fetch_recovered(&self, node: &str, path: &str, l: &Lease) {
        let name = format!("{}-{}", l.req.lane, l.id);
        let dst = self.home.join("recovered").join(format!("{name}.bundle"));
        let r = (|| -> Result<(), String> {
            std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut f = std::fs::File::create(&dst).map_err(|e| e.to_string())?;
            let mut off = 0u64;
            loop {
                let v = self.ask1(node, "recover-get", &format!("recover-get {path} {off}"))?;
                let data = b64_decode(v.get("data").and_then(|d| d.as_str()).unwrap_or("")).ok_or("recover-get: bad data")?;
                f.write_all(&data).map_err(|e| e.to_string())?;
                let next = v.get("off").and_then(super::num).map_or(off + data.len() as u64, |n| n as u64);
                if v.get("eof").and_then(|e| e.as_bool()).unwrap_or(true) {
                    return Ok(());
                }
                if next <= off {
                    return Err("recover-get made no progress".into());
                }
                off = next;
            }
        })();
        if let Err(e) = r {
            return self.event(Sev::Err, format!("{} of {}: recovered work stays on {node} ({path}): {e}", l.tag(), l.req.lane));
        }
        let Some(dir) = &l.req.repo_dir else {
            return self.event(Sev::Warn, format!("{} of {}: recovered work saved as {}", l.tag(), l.req.lane, dst.display()));
        };
        let ok = makepad_remote::group::command("git")
            .arg("-C")
            .arg(dir)
            .arg("fetch")
            .arg("--quiet")
            .arg(&dst)
            .arg(format!("HEAD:refs/heads/devhub/recovered/{name}"))
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            if let Err(e) = self.ask1(node, "recover-del", &format!("recover-del {path}")) {
                self.event(Sev::Warn, format!("{} of {}: recovered, but the node copy {path} stays: {e}", l.tag(), l.req.lane));
            }
            self.event(Sev::Warn, format!("{} of {}: unlanded work recovered into branch devhub/recovered/{name} in {dir}", l.tag(), l.req.lane));
        } else {
            self.event(Sev::Err, format!("{} of {}: git fetch of the recovered work failed; bundle kept at {}", l.tag(), l.req.lane, dst.display()));
        }
    }

    /// The hosts into the scheduler (with their nodes.conf switches).
    fn sync_hosts(&self, hosts: &[(String, String)]) {
        let dev = read_dev(&self.home.join("nodes.conf"));
        let mut s = self.sched.lock().unwrap();
        for (name, addr) in hosts {
            let ip = addr.rsplit_once(':').map_or(addr.as_str(), |(h, _)| h);
            s.add_node(name, ip, dev.get(name).copied());
        }
    }

    fn push_policy(&self, node: &str, on: bool) {
        match self.ask1(node, "policy", &format!("policy dev={}", if on { "on" } else { "off" })) {
            Ok(_) => {
                self.pushed.lock().unwrap().insert(node.to_string(), on);
            }
            Err(e) => {
                self.pushed.lock().unwrap().remove(node);
                self.event(Sev::Info, format!("node {node}: policy not pushed: {e}"));
            }
        }
    }

    /// The node's JIT builds since its last report go to the master's ledger (the nodes hold no
    /// ledger of their own): one record per report that saw new builds, with the mean wall per
    /// build, so `jit stats` and the placement medians see them.
    fn ledger_jit_builds(&self, node: &str, caps: &Caps, now: u64) {
        let Some(store) = caps.jit_store.as_ref() else { return };
        let total = (store.per_id.iter().map(|s| s.builds).sum::<u64>(), store.per_id.iter().map(|s| s.build_seconds).sum::<f64>());
        let Some(prev) = self.jit_seen.lock().unwrap().insert(node.to_string(), total) else { return };
        if total.0 <= prev.0 { return; }
        let (n, secs) = (total.0 - prev.0, (total.1 - prev.1).max(0.0));
        let wall = ((secs * 1000.0) / n as f64) as u64;
        let line = format!("{{\"class\":\"jit-build\",\"kind\":\"\",\"node\":{},\"arch\":{},\"builds\":{n},\"wall_ms\":{wall},\"ended_ms\":{now}}}\n", json_str(node), json_str(&caps.arch));
        let offset = self.costs_offset.lock().unwrap();
        let written = std::fs::OpenOptions::new().create(true).append(true).open(self.home.join("costs.jsonl")).and_then(|mut f| f.write_all(line.as_bytes()));
        drop(offset);
        if let Err(e) = written { self.event(Sev::Warn, format!("costs.jsonl: {e}")); }
    }

    /// Asks every node for its caps (a node that answers again gets its policy).
    pub fn refresh(self: &Arc<Self>, hosts: &[(String, String)]) {
        self.sync_hosts(hosts);
        for (name, _) in hosts {
            let caps = (self.ask)(name, "caps", "caps").ok().and_then(|l| l.into_iter().next()).filter(|l| !l.starts_with("{\"err\":")).and_then(|l| Caps::parse(&l));
            let now = self.now();
            match caps {
                Some(c) => {
                    self.ledger_jit_builds(name, &c, now);
                    let asked = c.dev_req;
                    let up = self.sched.lock().unwrap().set_caps(name, c, now);
                    // a flip made on the node itself: the master takes it (saved, pushed; the push clears it there)
                    if let Some(want) = asked {
                        if self.sched.lock().unwrap().dev(name) != want {
                            let _ = self.set_dev(name, want);
                        } else {
                            self.pushed.lock().unwrap().remove(name);
                        }
                    }
                    let on = self.sched.lock().unwrap().dev(name);
                    if up || self.pushed.lock().unwrap().get(name) != Some(&on) {
                        self.push_policy(name, on);
                    }
                }
                None => self.sched.lock().unwrap().caps_missed(name, now),
            }
        }
        write_desk_links(&self.home, &self.nodes());
        self.pump();
    }

    /// `lease-beat` to every node with live leases; the ones it does not know are lost.
    pub fn beat_nodes(self: &Arc<Self>) {
        let nodes = self.sched.lock().unwrap().nodes_with_leases();
        for node in nodes {
            let ids = self.sched.lock().unwrap().live_on(&node);
            if ids.is_empty() {
                continue;
            }
            let Ok(v) = self.ask1(&node, "lease-beat", &format!("lease-beat {}", ids.join(" "))) else { continue };
            let now = self.now();
            if let Some(makepad_strict_json::Value::Obj(info)) = v.get("info") {
                let mut s = self.sched.lock().unwrap();
                for (id, i) in info {
                    s.set_info(id, &i.to_json());
                }
            }
            for id in strs(v.get("unknown")) {
                if ids.contains(&id) {
                    self.sched.lock().unwrap().release(&id, "lost", now);
                }
            }
        }
        self.pump();
    }

    /// The lease loop: places every second, beats the nodes every 5 s, their caps every 10 s. The
    /// beats have their own thread: a caps round that waits on slow or down boxes (a just-started
    /// master's links coming up) must not starve the beats, or the nodes end their leases after
    /// 60 s without one (a master restart ended L8158 on a node, 2026-10-30).
    pub fn start(self: &Arc<Self>, hosts: Hosts) {
        self.resume_starts();
        let m = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease-sched".into()).spawn(move || loop {
            m.pump();
            std::thread::sleep(Duration::from_secs(1));
        });
        let m = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease-beat".into()).spawn(move || loop {
            m.beat_nodes();
            std::thread::sleep(Duration::from_millis(NODE_BEAT_MS));
        });
        let m = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease-nodes".into()).spawn(move || loop {
            m.refresh(&hosts());
            std::thread::sleep(Duration::from_millis(CAPS_MS));
        });
    }

    /// Informational self-update progress in `nodes`; placement continues unchanged.
    pub fn set_updating(&self, node: &str, why: Option<&str>) {
        let mut s = self.sched.lock().unwrap();
        match why {
            Some(w) => s.updating.insert(node.to_string(), w.to_string()),
            None => s.updating.remove(node),
        };
    }

    pub fn view(&self) -> Vec<Lease> {
        self.sched.lock().unwrap().view()
    }

    pub fn nodes(&self) -> Vec<NodeView> {
        self.sched.lock().unwrap().node_view()
    }

    /// The user's switch: saved in nodes.conf, pushed to the node, the scheduler told.
    pub fn set_dev(self: &Arc<Self>, node: &str, on: bool) -> Result<(), String> {
        let now = self.now();
        self.sched.lock().unwrap().set_dev(node, on, now)?;
        write_dev(&self.home.join("nodes.conf"), node, on).map_err(|e| format!("nodes.conf: {e}"))?;
        let n = node.to_string();
        self.spawn(move |m| m.push_policy(&n, on));
        self.pump();
        Ok(())
    }

    fn state_line(&self, id: &str) -> Option<String> {
        let s = self.sched.lock().unwrap();
        let l = s.get(id)?;
        Some(l.state_line(self.now(), &l.node.as_deref().map(|n| s.addr(n)).unwrap_or_default()))
    }

    /// One control service request (the line may repeat the service word, like `ps`).
    pub fn serve(self: &Arc<Self>, service: &str, line: &str, out: &mut Vec<String>) {
        let now = self.now();
        let line = line.strip_prefix(service).filter(|r| r.is_empty() || r.starts_with(' ')).map_or(line, str::trim_start);
        match service {
            "lease" => match Req::parse(line).and_then(|req| bind_lane(&self.home, &req).map(|_| req)) {
                Ok(req) => {
                    let id = self.sched.lock().unwrap().request(req, now);
                    self.persist();
                    self.pump();
                    out.extend(self.state_line(&id));
                }
                Err(e) => out.push(err(&e)),
            },
            "beat" => match self.sched.lock().unwrap().beat(line.trim(), now) {
                Some(l) => out.push(l),
                None => out.push(err(&format!("no lease {}", line.trim()))),
            },
            "release" | "cancel" => {
                let mut w = line.split_whitespace();
                let id = w.next().unwrap_or("");
                let how = w.next().filter(|h| !h.starts_with("exit=")).unwrap_or("released");
                // `cancel`: the user's end of one lease; the lane is kept off that node for a while
                let by = w.clone().find_map(|x| x.strip_prefix("by=")).unwrap_or("user").to_string();
                let ended = if service == "cancel" { self.sched.lock().unwrap().cancel_by(id, &by, now) } else { self.sched.lock().unwrap().release(id, how, now) };
                if ended {
                    self.persist();
                    self.pump();
                    // the answer says how the node cleaned up (it waits for the node, not forever)
                    let cleanup = self.cleanup_of(id, Duration::from_secs(30)).unwrap_or_else(|| "{\"cleanup\":\"still running\"}".into());
                    out.extend(self.state_line(id).map(|l| format!("{},\"cleanup\":{cleanup}}}", l.strip_suffix('}').unwrap_or(&l))));
                } else {
                    let ended = self.sched.lock().unwrap().get(id).map(|l| l.state.as_str().to_string());
                    out.push(err(&match ended {
                        Some(_) => format!("{id} has already ended (`devhub leases --all`)"),
                        None => format!("no lease {id}"),
                    }));
                }
            }
            "why-queued" => match self.sched.lock().unwrap().why_queued(line.trim(), now) {
                Ok(lines) => out.extend(lines),
                Err(e) => out.push(err(&e)),
            },
            "leases" => {
                let s = self.sched.lock().unwrap();
                for l in s.view() {
                    out.push(l.state_line(now, &l.node.as_deref().map(|n| s.addr(n)).unwrap_or_default()));
                }
                if line.split_whitespace().any(|w| w == "all") {
                    for l in s.recent_ended(30) {
                        out.push(l.state_line(now, ""));
                    }
                }
            }
            "jit" => match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["misses", ..] => match crate::jit_misses::serve(&self.home, line, now, &|action, value| {
                    let id = value.get("id").and_then(makepad_strict_json::Value::as_str).unwrap_or("");
                    let line = if action == "closed" { format!("JIT miss {id} closed; the next devhub test tries the JIT again") } else { crate::jit_misses::notice(value) };
                    (self.publish)(hub_record(&self.machine, "", 0, "jit_miss", if action == "closed" { Sev::Info } else { Sev::Warn }, line, value.to_json()));
                }) {
                    Ok(v) => out.push(v),
                    Err(e) => out.push(err(&e)),
                },
                ["stats"] | ["stats", "--day"] => match super::jit::stats(&self.home.join("costs.jsonl"), &self.nodes(), line.contains("--day"), now) {
                    Ok(text) => out.push(format!("{{\"text\":{}}}", json_str(&text))),
                    Err(e) => out.push(err(&e)),
                },
                _ => out.push(err("jit stats [--day] | misses [--all] | misses close <id>")),
            },
            "stats" => match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["queue"] | ["queue", "--hour"] => {
                    let leases = {
                        let s = self.sched.lock().unwrap();
                        let mut leases = s.view();
                        leases.extend(s.recent_ended(usize::MAX));
                        leases
                    };
                    match queue_stats(&self.home.join("leases.jsonl"), &leases, now) {
                        Ok(text) => out.push(format!("{{\"text\":{}}}", json_str(&text))),
                        Err(e) => out.push(err(&e)),
                    }
                }
                _ => out.push(err("stats queue [--hour]")),
            },
            "prof" => {
                let summary = line == "--summary";
                let args: Vec<_> = if summary { Vec::new() } else { line.split_whitespace().collect() };
                let result = super::prof::Query::parse(&args).and_then(|q| {
                    self.pump();
                    let leases = {
                        let s = self.sched.lock().unwrap();
                        let mut leases = s.view();
                        leases.extend(s.recent_ended(usize::MAX));
                        leases
                    };
                    super::prof::report(&self.home, &leases, &q, self.now(), summary)
                });
                match result {
                    Ok(text) => out.push(format!("{{\"text\":{}}}", json_str(&text))),
                    Err(e) => out.push(err(&e)),
                }
            }
            "nodes" => out.extend(self.nodes().iter().map(|n| n.to_json())),
            "node-dev" => {
                let w: Vec<&str> = line.split_whitespace().collect();
                match w.as_slice() {
                    [node, v @ ("on" | "off")] => match self.set_dev(node, *v == "on") {
                        Ok(()) => out.extend(self.nodes().iter().filter(|n| n.name == *node).map(|n| n.to_json())),
                        Err(e) => out.push(err(&e)),
                    },
                    _ => out.push(err("node-dev <node> on|off")),
                }
            }
            "linspect" => self.linspect(line, out),
            "evict" => {
                let who = line.trim();
                if !crate::trust::valid_name(who) {
                    return out.push(err("evict <lane|agent>"));
                }
                let ids = self.sched.lock().unwrap().evict(who, now);
                write_blocked(&self.home, &self.sched.lock().unwrap().blocked);
                self.pump();
                let cleaned: Vec<String> = ids.iter().map(|id| format!("{}:{}", json_str(id), self.cleanup_of(id, Duration::from_secs(30)).unwrap_or_else(|| "{\"cleanup\":\"still running\"}".into()))).collect();
                out.push(format!("{{\"evicted\":{},\"ended\":{{{}}},\"blocked\":true}}", json_str(who), cleaned.join(",")));
            }
            "stop" => {
                let lane = line.trim();
                if !crate::trust::valid_name(lane) {
                    return out.push(err("stop <lane>"));
                }
                let ids = self.sched.lock().unwrap().stop(lane, now);
                self.pump();
                let cleaned: Vec<String> = ids.iter().map(|id| format!("{}:{}", json_str(id), self.cleanup_of(id, Duration::from_secs(30)).unwrap_or_else(|| "{\"cleanup\":\"still running\"}".into()))).collect();
                out.push(format!("{{\"stopped\":{},\"ended\":{{{}}}}}", json_str(lane), cleaned.join(",")));
            }
            "unblock" => {
                let who = line.trim();
                let had = self.sched.lock().unwrap().unblock(who);
                write_blocked(&self.home, &self.sched.lock().unwrap().blocked);
                self.pump();
                out.push(if had { format!("{{\"unblocked\":{}}}", json_str(who)) } else { err(&format!("{who} is not blocked")) });
            }
            "blocked" => {
                for b in self.sched.lock().unwrap().blocked.iter() {
                    out.push(format!("{{\"blocked\":{}}}", json_str(b)));
                }
            }
            "override" => {
                let w: Vec<&str> = line.split_whitespace().collect();
                let (node, to) = match w.as_slice() {
                    [n] => (*n, None),
                    [n, "--to", t] | [n, t] => (*n, Some(*t)),
                    _ => return out.push(err("override <node> [--to <lane>]")),
                };
                let r = self.sched.lock().unwrap().override_node(node, to, now);
                match r {
                    Ok(ids) => {
                        if to.is_none() {
                            // the node is the user's now: no new leases until the user switches it back
                            if let Err(e) = self.set_dev(node, false) {
                                return out.push(err(&e));
                            }
                        }
                        self.pump();
                        let cleaned: Vec<String> = ids.iter().map(|id| format!("{}:{}", json_str(id), self.cleanup_of(id, Duration::from_secs(30)).unwrap_or_else(|| "{\"cleanup\":\"still running\"}".into()))).collect();
                        out.push(format!("{{\"overridden\":{},\"to\":{},\"ended\":{{{}}}}}", json_str(node), to.map_or("\"user\"".into(), json_str), cleaned.join(",")));
                    }
                    Err(e) => out.push(err(&e)),
                }
            }
            _ => out.push(err("unknown lease service")),
        }
    }

    /// `linspect <lease> <node> <pid|app> <cmd...>`: checks the inspect lease, records the
    /// command, forwards it to the JIT, and remembers what it left (breaks, probes, pause).
    fn linspect(self: &Arc<Self>, line: &str, out: &mut Vec<String>) {
        let mut w = line.splitn(4, ' ');
        let (id, node, jit, cmd) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or("").trim());
        if cmd.is_empty() {
            return out.push(err("linspect <lease> <node> <pid|app> <cmd...>"));
        }
        let (lane, user) = {
            let s = self.sched.lock().unwrap();
            let Some(l) = s.get(id) else { return out.push(err(&format!("no lease {id}"))) };
            if l.state != State::Granted || l.req.on_node() || l.node.as_deref() != Some(node) || l.req.jit.as_deref() != Some(jit) {
                return out.push(err(&format!("{id} is not a live inspect lease on {node}/{jit}")));
            }
            let view = s.node_view();
            let n = view.iter().find(|n| n.name == node);
            let user = n.is_some_and(|n| n.role().users());
            // input and windows are writes: off while the node's dev switch is off (§10.11)
            let verb = cmd.split_whitespace().next().unwrap_or("");
            if INPUT_VERBS.contains(&verb) && !n.is_some_and(|n| n.dev) {
                return out.push(err(&format!("{node} is not available for dev: `{verb}` (input/windows) needs its dev switch on")));
            }
            (l.req.lane.clone(), user)
        };
        let verb = cmd.split_whitespace().next().unwrap_or("");
        let stops = matches!(verb, "break" | "pause") && !cmd.starts_with("break clear") && !cmd.starts_with("break list");
        let key = (node.to_string(), jit.to_string());
        if stops {
            let held = self.holders.lock().unwrap().get(&key).cloned().filter(|h| h != id);
            if let Some(h) = held {
                let live = self.sched.lock().unwrap().get(&h).filter(|l| l.state == State::Granted).map(|l| l.req.lane.clone());
                if let Some(other) = live {
                    return out.push(err(&format!("{node}/{jit}: breakpoints are held by {h} (lane {other})")));
                }
            }
        }
        if user || matches!(verb, "break" | "pause" | "continue" | "resume" | "probe") {
            let sev = if stops { Sev::Warn } else { Sev::Info };
            self.event(sev, format!("inspect {node}/{jit} by lane {lane}: {}", proto::clean(cmd, 200)));
        }
        let answer = match (self.ask)(&format!("{node}:{jit}"), "cmd", cmd) {
            Ok(l) => l,
            Err(e) => return out.push(err(&e)),
        };
        if matches!(verb, "break" | "pause" | "continue" | "resume" | "probe") {
            let mut marks = self.marks.lock().unwrap();
            let m = marks.entry(id.to_string()).or_insert_with(|| Marks { jit: jit.to_string(), ..Marks::default() });
            let ids: Vec<String> = answer.iter().filter_map(|a| makepad_strict_json::parse(a.as_bytes()).ok()).filter_map(|v| ["id", "probe", "break"].iter().find_map(|k| v.get(k)).and_then(|i| i.as_str().map(str::to_string).or_else(|| super::num(i).map(|n| (n as i64).to_string())))).collect();
            let arg = cmd.split_whitespace().nth(2).unwrap_or("").to_string();
            match (verb, cmd.split_whitespace().nth(1)) {
                ("break", Some("clear")) => m.breaks.retain(|b| *b != arg),
                ("probe", Some("off")) => m.probes.retain(|b| *b != arg),
                ("break", _) => m.breaks.extend(ids),
                ("probe", _) => m.probes.extend(ids),
                ("pause", _) => m.paused = true,
                _ => m.paused = false,
            }
            if stops {
                self.holders.lock().unwrap().insert(key, id.to_string());
            }
        }
        out.extend(answer);
    }
}

/// Inspect words that inject input or move windows: writes, refused on a node with dev off.
pub const INPUT_VERBS: &[&str] = &["key", "keys", "click", "type", "text", "scroll", "drag", "mouse", "touch", "tap", "resize", "move", "window", "focus", "input", "gesture"];

/// The production lease master: nodes through this master's router, revisions through sync.
pub fn register(ep: &makepad_remote::Endpoint, machine: &str, publish: Publish) -> Arc<LeaseMaster> {
    // one link to this master's own router for every call (a link per call left its connection
    // and service threads behind on both ends: 680 threads and the fd limit within minutes)
    let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(proto::control_socket()), "devhub-lease");
    let client = move || ctl.clone();
    let c1 = client.clone();
    let ask: Ask = Arc::new(move |node: &str, service: &str, line: &str| {
        let mut out = Vec::new();
        makepad_remote::hub::ask(&c1(), node, service, "", line, &mut |l| {
            out.push(l.to_string());
            true
        })?;
        Ok(out)
    });
    #[cfg(feature = "checkout")]
    // a commit a node's store got from this master is not sent again for a while (a lease's
    // bundle round trip is most of its setup on a warm box)
    #[cfg(feature = "checkout")]
    let ensured: Arc<Mutex<HashMap<(String, String), std::time::Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    #[cfg(feature = "checkout")]
    let rev: Option<EnsureRev> = Some(Arc::new(move |node: &str, repo: &str, dir: &Path, pin: &super::RevPin| {
        let key = (node.to_string(), pin.sha.clone());
        if ensured.lock().unwrap().get(&key).is_some_and(|t| t.elapsed() < ENSURED_FOR) {
            return Ok(pin.sha.clone());
        }
        let r = crate::sync::ensure_rev(&client(), node, repo, dir, &pin.branch, &pin.sha);
        if r.is_ok() {
            let mut e = ensured.lock().unwrap();
            e.retain(|_, t| t.elapsed() < ENSURED_FOR);
            e.insert(key, std::time::Instant::now());
        }
        r
    }));
    #[cfg(not(feature = "checkout"))]
    let rev: Option<EnsureRev> = None;
    let lm = Arc::new(LeaseMaster::new(machine, proto::home(), ask, publish, Arc::new(proto::wall_ms), rev, false));
    for svc in SERVICES {
        let l = lm.clone();
        ep.register(svc, 1, makepad_remote::lines::service(svc, Arc::new(move |line, out| l.serve(svc, line, out))));
    }
    lm
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::Kind;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn desk_links_list_the_listening_endpoints() {
        let view = |name: &str, addr: &str, up: bool, desk: Option<(bool, u16)>| NodeView {
            name: name.into(),
            addr: addr.into(),
            up,
            dev: false,
            dev_set: false,
            caps: Some(Caps { desk_link: desk.map(|(on, port)| crate::embed::DeskLinkState { on, port, note: String::new() }), ..Default::default() }),
            leases: 0,
            waiting: 0,
            draining: false,
            hold: None,
            updating: None,
            latency: String::new(),
        };
        let nodes = [view("quest", "192.0.2.85:8788", true, Some((true, 8790))), view("phone", "192.0.2.50", true, Some((true, 0))), view("off", "192.0.2.51", true, Some((false, 8790))), view("gone", "192.0.2.52", false, Some((true, 8790))), view("217", "192.0.2.89", true, None)];
        assert_eq!(desk_links(&nodes), "quest 192.0.2.85:8790\n");
    }

    /// A node that answers the node services and remembers what it was asked.
    struct FakeNode {
        calls: Mutex<Vec<(String, String, String)>>,
        known: Mutex<Vec<String>>,
            /// its hub is gone (restarting): every call fails as a lost link
        gone: std::sync::atomic::AtomicBool,
}

    impl FakeNode {
        fn answer(&self, node: &str, service: &str, line: &str) -> Result<Vec<String>, String> {
            self.calls.lock().unwrap().push((node.to_string(), service.to_string(), line.to_string()));
            if self.gone.load(Ordering::SeqCst) {
                return Err(format!("link lost: cannot reach fleet hub `{node}` at 192.0.2.89:8797: Connection refused (os error 111)"));
            }
            let caps = super::super::Caps { hub_proto: super::super::HUB_PROTO, node: node.into(), arch: "x86_64".into(), os: "linux".into(), cores: 16, ram_gb: 64.0, ram_avail_gb: 50.0, disk_free_gb: 300.0, disk_total_gb: 1000.0, disk_floor_gb: 25.0, timed: true, ..Default::default() };
            Ok(vec![match service {
                "caps" => caps.to_json(),
                "policy" => "{\"dev\":\"on\"}".into(),
                "lease-start" => {
                    let id = line.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                    self.known.lock().unwrap().push(id.clone());
                    format!("{{\"ok\":1,\"work\":\"/home/me/devhub/lane/x/src\",\"target\":\"/t\",\"tmp\":\"/tmp/{id}\",\"env\":{{\"CARGO_INCREMENTAL\":\"0\"}}}}")
                }
                "lease-beat" => {
                    let known = self.known.lock().unwrap();
                    let (k, u): (Vec<&str>, Vec<&str>) = line.split_whitespace().skip(1).partition(|i| known.iter().any(|x| x == i));
                    format!("{{\"live\":[{}],\"unknown\":[{}]}}", k.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","), u.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","))
                }
                "lease-end" => {
                    let id = line.split_whitespace().nth(1).unwrap();
                    self.known.lock().unwrap().retain(|k| k != id);
                    format!("{{\"ended\":{},\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[]}}", json_str(id))
                }
                "cmd" => "{\"id\":\"B1\",\"ok\":true}".into(),
                _ => return Err(format!("no service {service}")),
            }])
        }
    }

    fn master(dir: &Path) -> (Arc<LeaseMaster>, Arc<FakeNode>, Arc<AtomicU64>, Arc<Mutex<Vec<Record>>>) {
        let node = Arc::new(FakeNode { calls: Mutex::new(Vec::new()), known: Mutex::new(Vec::new()), gone: Default::default() });
        let clock = Arc::new(AtomicU64::new(1_000));
        let recs = Arc::new(Mutex::new(Vec::new()));
        let (n, c, r) = (node.clone(), clock.clone(), recs.clone());
        let m = Arc::new(LeaseMaster::new(
            "mac",
            dir.to_path_buf(),
            Arc::new(move |a: &str, b: &str, l: &str| n.answer(a, b, l)),
            Arc::new(move |rec| r.lock().unwrap().push(rec)),
            Arc::new(move || c.load(Ordering::SeqCst)),
            None,
            true,
        ));
        (m, node, clock, recs)
    }

    fn serve(m: &Arc<LeaseMaster>, svc: &str, line: &str) -> String {
        let mut out = Vec::new();
        m.serve(svc, line, &mut out);
        out.join("\n")
    }

    #[test]
    fn grant_start_beat_release_end() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, clock, recs) = master(&dir);
        let hosts = vec![("217".to_string(), "192.0.2.89:8797".to_string())];
        m.refresh(&hosts);
        assert!(node.calls.lock().unwrap().iter().any(|c| c.1 == "policy" && c.2 == "policy dev=on"));
        let a = serve(&m, "lease", &Req::new("gapx", Kind::Build).to_json());
        assert!(a.contains("\"id\":\"L1\"") && a.contains("\"state\":\"granted\"") && a.contains("\"addr\":\"192.0.2.89\"") && a.contains("CARGO_INCREMENTAL"), "{a}");
        clock.store(20_000, Ordering::SeqCst);
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""));
        m.beat_nodes();
        assert!(node.calls.lock().unwrap().iter().any(|c| c.1 == "lease-beat" && c.2 == "lease-beat L1"));
        assert!(serve(&m, "leases", "").contains("\"id\":\"L1\""));
        assert!(serve(&m, "nodes", "").contains("\"leases\":1"));
        let r = serve(&m, "release", "L1 released exit=0");
        assert!(r.contains("\"state\":\"ended\"") && r.contains("\"how\":\"released\""), "{r}");
        assert!(node.calls.lock().unwrap().iter().any(|c| c.1 == "lease-end" && c.2 == "lease-end L1 released"));
        let ledger = std::fs::read_to_string(dir.join("leases.jsonl")).unwrap();
        assert!(ledger.contains("\"id\":\"L1\"") && ledger.contains("\"how\":\"released\""));
        assert!(recs.lock().unwrap().iter().any(|r| r.kind == "lease" && r.line.starts_with("L1 build@217 granted to gapx")));
        // a master restart mid-lease: the new master takes the granted lease back from
        // leases.live, its holder re-attaches by id, the node keeps running it, and the gate's
        // verdict is released as usual
        let mid = serve(&m, "lease", &Req::new("gate", Kind::Gate).to_json());
        assert!(mid.contains("\"state\":\"granted\""), "{mid}");
        let gid = mid.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
        drop(m);
        let (m1, node1, ..) = master(&dir);
        node1.known.lock().unwrap().push(gid.clone());
        m1.refresh(&hosts);
        assert!(serve(&m1, "beat", &gid).contains("\"state\":\"granted\""), "the lease outlived the restart");
        m1.beat_nodes();
        assert!(serve(&m1, "beat", &gid).contains("\"state\":\"granted\""), "the node still runs it");
        let r = serve(&m1, "release", &format!("{gid} released exit=0"));
        assert!(r.contains("\"how\":\"released\""), "{r}");
        drop(m1);
        // a new master continues the numbering, past what the last one may have given out unrecorded
        let (m2, node2, ..) = master(&dir);
        m2.refresh(&hosts);
        assert!(serve(&m2, "lease", &Req::new("gapx", Kind::Test).to_json()).contains("\"id\":\"L1003\""));
        // the node forgot a lease (restarted): lost, warn
        node2.known.lock().unwrap().clear();
        m2.beat_nodes();
        assert!(serve(&m2, "beat", "L1003").contains("\"how\":\"lost\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A master restart loses nothing: the queue keeps its ids and places, a lease the node was
    /// still setting up is started again, a lease released an instant before the exec does not
    /// come back, and the file is written at the change, not after the node's teardown.
    #[test]
    fn a_restart_keeps_the_queue_and_the_leases_in_setup() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, _node, _clock, _recs) = master(&dir);
        let hosts = vec![("217".to_string(), "192.0.2.89:8797".to_string())];
        m.refresh(&hosts);
        let a = serve(&m, "lease", &Req::new("a", Kind::Build).to_json());
        assert!(a.contains("\"state\":\"granted\""), "{a}");
        let b = serve(&m, "lease", &Req::new("b", Kind::Test).to_json());
        assert!(b.contains("\"state\":\"queued\""), "one lease per box: {b}");
        let c = serve(&m, "lease", &Req::new("c", Kind::Test).to_json());
        assert!(c.contains("\"state\":\"queued\""), "{c}");
        // a lease released right before the exec: the file already forgot it
        let id = |l: &str| l.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
        let (ia, ib, ic) = (id(&a), id(&b), id(&c));
        let text = std::fs::read_to_string(dir.join("leases.live")).unwrap();
        assert!(text.contains(&format!("\"id\":\"{ib}\"")) && text.contains(&format!("\"id\":\"{ic}\"")), "the queue is persisted: {text}");
        serve(&m, "cancel", &format!("{ic} by=test"));
        let text = std::fs::read_to_string(dir.join("leases.live")).unwrap();
        assert!(!text.contains(&format!("\"id\":\"{ic}\"")), "an ended lease leaves the file at once: {text}");
        drop(m);
        let (m1, node1, ..) = master(&dir);
        node1.known.lock().unwrap().push(ia.clone());
        m1.refresh(&hosts);
        assert!(serve(&m1, "beat", &ia).contains("\"state\":\"granted\""));
        assert!(serve(&m1, "beat", &ib).contains("\"state\":\"queued\""), "the queued request kept its id and place");
        assert!(serve(&m1, "beat", &ic).contains("no lease"), "the cancelled one stays gone");
        let r = serve(&m1, "release", &format!("{ia} released exit=0"));
        assert!(r.contains("\"how\":\"released\""), "{r}");
        assert!(serve(&m1, "beat", &ib).contains("\"state\":\"granted\""), "the restored queue is served after the release");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lease waiting for the box gets it the moment the running one's lease-end answers: the
    /// tree stays at its pin; the box's latency line and ledger carry release -> grant, setup
    /// and teardown.
    #[test]
    fn a_waiting_lease_gets_the_box_straight_from_the_release() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-handoff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, clock, recs) = master(&dir);
        m.refresh(&[("217".to_string(), "192.0.2.89".to_string())]);
        assert!(serve(&m, "lease", &Req::new("a", Kind::Build).to_json()).contains("\"state\":\"granted\""));
        let b = serve(&m, "lease", &Req::new("b", Kind::Test).to_json());
        assert!(b.contains("\"state\":\"queued\""), "one lease per box: {b}");
        clock.store(5_000, Ordering::SeqCst);
        serve(&m, "release", "L1 released exit=0");
        assert!(node.calls.lock().unwrap().iter().any(|c| c.1 == "lease-end" && c.2 == "lease-end L1 released"), "{:?}", node.calls.lock().unwrap());
        assert!(serve(&m, "beat", "L2").contains("\"state\":\"granted\""), "granted in the same step as the release");
        // the last lease uses the same release; its work tree stays at the pin
        serve(&m, "release", "L2 released exit=0");
        assert!(node.calls.lock().unwrap().iter().any(|c| c.2 == "lease-end L2 released"));
        let nodes = serve(&m, "nodes", "");
        assert!(nodes.contains("handoff 0.0 s") && nodes.contains("setup ") && nodes.contains("teardown "), "{nodes}");
        let ledger = std::fs::read_to_string(dir.join("leases.jsonl")).unwrap();
        assert!(ledger.contains("\"id\":\"L2\"") && ledger.contains("\"wait_ms\":0") && ledger.contains("\"setup_ms\":"), "{ledger}");
        assert!(recs.lock().unwrap().iter().any(|r| r.line.contains("lease_latency: L2 test@217 of b on a node: release->grant 0.0 s")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A box whose hub is gone at the lease's start (restarting for its update) is down until its
    /// hub reports in; the lease waits in the queue and gets the box then, it does not fail.
    #[test]
    fn a_lease_whose_box_is_unreachable_waits_instead_of_failing() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _clock, recs) = master(&dir);
        let hosts = vec![("217".to_string(), "192.0.2.89".to_string())];
        m.refresh(&hosts);
        node.gone.store(true, Ordering::SeqCst);
        let a = serve(&m, "lease", &Req::new("a", Kind::Test).to_json());
        assert!(a.contains("\"state\":\"queued\""), "{a}");
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"queued\""), "still waiting while the box is down");
        assert!(recs.lock().unwrap().iter().any(|r| r.line.contains("217 unreachable at its start")));
        // the hub is back: its caps answer, the box is up, the lease gets it
        node.gone.store(false, Ordering::SeqCst);
        m.refresh(&hosts);
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dev_switch_and_inspect_holders() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-test2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nodes.conf"), "217 addr=192.0.2.89 # pool\n").unwrap();
        let (m, node, _clock, recs) = master(&dir);
        m.refresh(&[("217".to_string(), "192.0.2.89".to_string())]);
        assert!(serve(&m, "node-dev", "217 off").contains("\"dev\":false"));
        assert_eq!(std::fs::read_to_string(dir.join("nodes.conf")).unwrap(), "217 addr=192.0.2.89 dev=off # pool\n");
        assert!(node.calls.lock().unwrap().iter().any(|c| c.2 == "policy dev=off"));
        assert!(serve(&m, "lease", &Req::new("x", Kind::Build).to_json()).contains("\"reason\":\"dev off\""));
        let ins = |lane: &str| {
            let mut r = Req::new(lane, Kind::Inspect);
            r.node = Some("217".into());
            r.jit = Some("sandbox".into());
            r.to_json()
        };
        assert!(serve(&m, "lease", &ins("a")).contains("\"state\":\"granted\""));
        assert!(serve(&m, "lease", &ins("b")).contains("\"state\":\"granted\""));
        let id_a = "L2";
        let id_b = "L3";
        assert!(serve(&m, "linspect", &format!("{id_a} 217 sandbox break set game/ai.rs:88")).contains("B1"));
        assert!(recs.lock().unwrap().iter().any(|r| r.sev == Sev::Warn && r.line == "inspect 217/sandbox by lane a: break set game/ai.rs:88"));
        assert!(serve(&m, "linspect", &format!("{id_b} 217 sandbox pause")).contains("held by L2"));
        assert!(serve(&m, "linspect", &format!("{id_b} 217 other eval 1")).contains("not a live inspect lease"));
        serve(&m, "release", id_a);
        let end = node.calls.lock().unwrap().iter().find(|c| c.1 == "lease-end").cloned().unwrap();
        assert_eq!(end.2, "lease-end L2 released {\"inspect\":{\"pid\":null,\"jit\":\"sandbox\",\"breaks\":[\"B1\"],\"probes\":[],\"paused\":false}}");
        assert!(!serve(&m, "linspect", &format!("{id_b} 217 sandbox pause")).contains("err"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_decodes() {
        let s = makepad_network::digest::base64_encode(b"devhub lease\0x");
        assert_eq!(b64_decode(&s).unwrap(), b"devhub lease\0x");
    }
}

#[cfg(test)]
mod lane_binding_tests {
    use super::*;

    /// A lane ships from one worktree: a second worktree under the same name is refused with a
    /// clear error, the first may go on, and a removed worktree's name passes to the next.
    #[test]
    fn a_second_worktree_under_one_lane_name_is_refused() {
        let base = std::env::temp_dir().join(format!("devhub-lanes-{}", std::process::id()));
        let (home, a, b) = (base.join("home"), base.join("wt-a"), base.join("wt-b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let req = |dir: &Path| {
            let mut r = Req::new("fablang", super::super::Kind::Gate);
            r.repo_dir = Some(dir.to_string_lossy().into_owned());
            r
        };
        assert!(bind_lane(&home, &req(&a)).is_ok());
        assert!(bind_lane(&home, &req(&a)).is_ok(), "the bound worktree goes on");
        let e = bind_lane(&home, &req(&b)).unwrap_err();
        assert!(e.contains("already ships from") && e.contains("own lane name"), "{e}");
        // a lease with a revision ships nothing into the lane dir: never refused
        let mut rev = req(&b);
        rev.rev = Some("rc-x,main".into());
        assert!(bind_lane(&home, &rev).is_ok());
        // the first worktree is removed (landed): the name passes on
        std::fs::remove_dir_all(&a).unwrap();
        assert!(bind_lane(&home, &req(&b)).is_ok());
        assert!(std::fs::read_to_string(home.join("lanes.conf")).unwrap().contains(&b.to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
