//! The master's lease services (control socket): `lease`, `beat`, `release`, `leases`, `nodes`,
//! `node-dev`, `linspect`. It runs the [`Sched`], sets leases up and tears them down on the nodes
//! (`lease-start`, `lease-end`), beats the nodes' live leases (`lease-beat`), refreshes their
//! `caps`, pushes the dev switch (`policy`), keeps the ledger (`~/.makepad/devhub/leases.jsonl`)
//! and publishes every decision as a `lease` record.
//!
//! Nodes are reached only through [`Ask`] (production: `hub::ask` on the control socket's router).

use super::sched::{Action, Lease, NodeView, Req, Sched, State};
use super::{Caps, Kind};
use crate::proto::{self, hub_record, Record, Sev};
use makepad_remote::json_str;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// `(node, service, line)` -> the answer lines (without the end line).
pub type Ask = Arc<dyn Fn(&str, &str, &str) -> Result<Vec<String>, String> + Send + Sync>;
pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;
/// `(node, repo, repo dir, branch)` -> that commit, present in the node's store.
pub type EnsureRev = Arc<dyn Fn(&str, &str, &Path, &str) -> Result<String, String> + Send + Sync>;
/// The nodes the master dials: (name, address).
pub type Hosts = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

pub const SERVICES: [&str; 7] = ["lease", "beat", "release", "leases", "nodes", "node-dev", "linspect"];
const NODE_BEAT_MS: u64 = 5_000;
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
    /// tests: node calls run on the caller's thread
    inline: bool,
    marks: Mutex<HashMap<String, Marks>>,
    /// (node, jit) -> the lease that holds its breakpoints
    holders: Mutex<HashMap<(String, String), String>>,
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

fn strs(v: Option<&makepad_strict_json::Value>) -> Vec<String> {
    v.and_then(|a| a.as_arr()).map(|a| a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| super::num(x).map(|n| (n as i64).to_string()).unwrap_or_default())).collect()).unwrap_or_default()
}

impl LeaseMaster {
    pub fn new(machine: &str, home: PathBuf, ask: Ask, publish: Publish, clock: Clock, ensure_rev: Option<EnsureRev>, inline: bool) -> LeaseMaster {
        let next = next_id(&home.join("leases.jsonl"));
        // the leases a previous master granted and did not end: their holders and nodes still
        // run them (a restart of the master must not cost them their work)
        let mut sched = Sched::new(machine, next);
        let now = clock();
        for line in std::fs::read_to_string(home.join("leases.live")).unwrap_or_default().lines() {
            sched.restore(line, now);
        }
        LeaseMaster {
            machine: machine.to_string(),
            sched: Mutex::new(sched),
            ask,
            publish,
            clock,
            ensure_rev,
            home,
            inline,
            marks: Mutex::new(HashMap::new()),
            holders: Mutex::new(HashMap::new()),
        }
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    /// `leases.live`: the granted leases, rewritten whole on every grant and end.
    fn save_live(&self) {
        let lines = self.sched.lock().unwrap().live_lines();
        let _ = std::fs::create_dir_all(&self.home);
        let (path, tmp) = (self.home.join("leases.live"), self.home.join("leases.live.new"));
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
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

    /// Places what fits and carries out the scheduler's actions.
    pub fn pump(self: &Arc<Self>) {
        let now = self.now();
        let acts = self.sched.lock().unwrap().tick(now);
        self.act(acts);
    }

    fn act(self: &Arc<Self>, acts: Vec<Action>) {
        for a in acts {
            match a {
                Action::Event(r) => (self.publish)(r),
                Action::Grant { id, node } => {
                    let inspect = self.sched.lock().unwrap().get(&id).is_some_and(|l| l.req.kind == Kind::Inspect);
                    if inspect {
                        // nothing to set up: an inspect lease is the right to talk to a JIT
                        self.sched.lock().unwrap().started(&id, "{}");
                        self.save_live();
                    } else {
                        self.spawn(move |m| m.grant(&id, &node));
                    }
                }
                Action::End { id, node, how } => self.spawn(move |m| m.end(&id, node.as_deref(), &how)),
                Action::Reclaim { node } => self.spawn(move |m| {
                    if let Err(e) = m.ask1(&node, "disk", "disk") {
                        m.event(Sev::Warn, format!("node {node}: disk reclaim failed: {e}"));
                    }
                }),
            }
        }
    }

    /// `lease-start` on the node (after making its revisions present there).
    fn grant(self: &Arc<Self>, id: &str, node: &str) {
        let Some(l) = self.sched.lock().unwrap().get(id).cloned() else { return };
        let r = self.start_on(&l, node);
        let now = self.now();
        match r {
            Ok(answer) => self.sched.lock().unwrap().started(id, &answer),
            Err(e) => self.sched.lock().unwrap().failed(id, &e, now),
        }
        self.save_live();
        self.pump();
    }

    fn start_on(&self, l: &Lease, node: &str) -> Result<String, String> {
        let rev = match &l.req.rev {
            None => "null".to_string(),
            Some(spec) => {
                let (mk, com) = spec.split_once(',').map_or((spec.as_str(), None), |(a, b)| (a, Some(b)));
                let dir = PathBuf::from(l.req.repo_dir.clone().unwrap_or_else(|| ".".into()));
                let f = self.ensure_rev.as_ref().ok_or("this master cannot ship revisions (built without checkout)")?;
                let mk = f(node, "makepad", &dir, mk)?;
                let com = match com {
                    Some(b) => json_str(&f(node, "commercial", &dir.join("apps/commercial"), b)?),
                    None => "null".into(),
                };
                format!("{{\"makepad\":{},\"commercial\":{com}}}", json_str(&mk))
            }
        };
        let line = format!("lease-start {{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"rev\":{rev},\"env\":{{}}}}", json_str(&l.id), json_str(&l.req.lane), l.req.kind.as_str());
        let lines = (self.ask)(node, "lease-start", &line)?;
        let first = lines.into_iter().next().ok_or("lease-start: no answer")?;
        let v = makepad_strict_json::parse(first.as_bytes()).map_err(|_| "lease-start: bad answer".to_string())?;
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return Err(format!("{node} refused: {e}"));
        }
        Ok(first)
    }

    /// `lease-end` on the node, the inspect marks undone, recovered work fetched, the ledger line.
    fn end(self: &Arc<Self>, id: &str, node: Option<&str>, how: &str) {
        let l = self.sched.lock().unwrap().get(id).cloned();
        let marks = self.marks.lock().unwrap().remove(id);
        self.holders.lock().unwrap().retain(|_, v| v != id);
        if let (Some(node), Some(l)) = (node, l.as_ref()) {
            // an inspect lease that changed nothing has nothing to undo
            if l.req.kind != Kind::Inspect || marks.is_some() {
                let wire_how = match how {
                    "released" | "expired" | "holder-dead" | "denied" => how,
                    "lost" => "expired",
                    _ => "released",
                };
                let payload = marks.map_or(String::new(), |m| {
                    let list = |v: &[String]| v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",");
                    let pid = m.jit.parse::<u32>().map_or("null".to_string(), |p| p.to_string());
                    format!(" {{\"inspect\":{{\"pid\":{pid},\"jit\":{},\"breaks\":[{}],\"probes\":[{}],\"paused\":{}}}}}", json_str(&m.jit), list(&m.breaks), list(&m.probes), m.paused)
                });
                match self.ask1(node, "lease-end", &format!("lease-end {id} {wire_how}{payload}")) {
                    Ok(v) => {
                        let leak = strs(v.get("leak"));
                        if !leak.is_empty() {
                            self.event(Sev::Err, format!("lease_leak: {} of {}: {} processes left after cleanup on {node} (pids {})", l.tag(), l.req.lane, leak.len(), leak.join(" ")));
                        }
                        if let Some(path) = v.get("recovered").and_then(|r| r.as_str()) {
                            self.fetch_recovered(node, path, l);
                        }
                    }
                    Err(e) => self.event(Sev::Warn, format!("{} of {}: cleanup on {node} failed: {e}", l.tag(), l.req.lane)),
                }
            }
        }
        self.save_live();
        if let Some(l) = l {
            let path = self.home.join("leases.jsonl");
            let _ = std::fs::create_dir_all(&self.home);
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "{}", l.ledger_line());
            }
        }
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
        let ok = std::process::Command::new("git")
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
        if let Err(e) = self.ask1(node, "policy", &format!("policy dev={}", if on { "on" } else { "off" })) {
            self.event(Sev::Info, format!("node {node}: policy not pushed: {e}"));
        }
    }

    /// Asks every node for its caps (a node that answers again gets its policy).
    pub fn refresh(self: &Arc<Self>, hosts: &[(String, String)]) {
        self.sync_hosts(hosts);
        for (name, _) in hosts {
            let caps = (self.ask)(name, "caps", "caps").ok().and_then(|l| l.into_iter().next()).filter(|l| !l.starts_with("{\"err\":")).and_then(|l| Caps::parse(&l));
            let now = self.now();
            match caps {
                Some(c) => {
                    let up = self.sched.lock().unwrap().set_caps(name, c, now);
                    if up {
                        let on = self.sched.lock().unwrap().dev(name);
                        self.push_policy(name, on);
                    }
                }
                None => self.sched.lock().unwrap().set_down(name),
            }
        }
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
            for id in strs(v.get("unknown")) {
                if ids.contains(&id) {
                    self.sched.lock().unwrap().release(&id, "lost", now);
                }
            }
        }
        self.pump();
    }

    /// The lease loop: places every second, beats the nodes every 5 s, their caps every 10 s.
    pub fn start(self: &Arc<Self>, hosts: Hosts) {
        let m = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease-sched".into()).spawn(move || loop {
            m.pump();
            std::thread::sleep(Duration::from_secs(1));
        });
        let m = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease-nodes".into()).spawn(move || {
            let mut last_caps = 0u64;
            loop {
                let now = m.now();
                if now.saturating_sub(last_caps) >= CAPS_MS {
                    last_caps = now;
                    m.refresh(&hosts());
                }
                m.beat_nodes();
                std::thread::sleep(Duration::from_millis(NODE_BEAT_MS));
            }
        });
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
            "lease" => match Req::parse(line) {
                Ok(req) => {
                    let id = self.sched.lock().unwrap().request(req, now);
                    self.pump();
                    out.extend(self.state_line(&id));
                }
                Err(e) => out.push(err(&e)),
            },
            "beat" => match self.sched.lock().unwrap().beat(line.trim(), now) {
                Some(l) => out.push(l),
                None => out.push(err(&format!("no lease {}", line.trim()))),
            },
            "release" => {
                let mut w = line.split_whitespace();
                let id = w.next().unwrap_or("");
                let how = w.next().filter(|h| !h.starts_with("exit=")).unwrap_or("released");
                if self.sched.lock().unwrap().release(id, how, now) {
                    self.pump();
                    out.extend(self.state_line(id));
                } else {
                    out.push(err(&format!("no live lease {id}")));
                }
            }
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
            if l.state != State::Granted || l.req.kind != Kind::Inspect || l.node.as_deref() != Some(node) || l.req.jit.as_deref() != Some(jit) {
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
    let rev: Option<EnsureRev> = Some(Arc::new(move |node: &str, repo: &str, dir: &Path, branch: &str| crate::sync::ensure_rev(&client(), node, repo, dir, branch)));
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
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A node that answers the node services and remembers what it was asked.
    struct FakeNode {
        calls: Mutex<Vec<(String, String, String)>>,
        known: Mutex<Vec<String>>,
    }

    impl FakeNode {
        fn answer(&self, node: &str, service: &str, line: &str) -> Result<Vec<String>, String> {
            self.calls.lock().unwrap().push((node.to_string(), service.to_string(), line.to_string()));
            let caps = super::super::Caps { node: node.into(), arch: "x86_64".into(), os: "linux".into(), cores: 16, ram_gb: 64.0, ram_avail_gb: 50.0, disk_free_gb: 300.0, disk_total_gb: 1000.0, disk_floor_gb: 25.0, timed: true, ..Default::default() };
            Ok(vec![match service {
                "caps" => caps.to_json(),
                "policy" => "{\"dev\":\"on\"}".into(),
                "lease-start" => {
                    let id = line.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                    self.known.lock().unwrap().push(id.clone());
                    format!("{{\"ok\":1,\"work\":\"/home/arch/devhub/lane/x/src\",\"target\":\"/t\",\"tmp\":\"/tmp/{id}\",\"env\":{{\"CARGO_INCREMENTAL\":\"0\"}}}}")
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
        let node = Arc::new(FakeNode { calls: Mutex::new(Vec::new()), known: Mutex::new(Vec::new()) });
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
        let hosts = vec![("217".to_string(), "10.0.0.217:8797".to_string())];
        m.refresh(&hosts);
        assert!(node.calls.lock().unwrap().iter().any(|c| c.1 == "policy" && c.2 == "policy dev=on"));
        let a = serve(&m, "lease", &Req::new("gapx", Kind::Build).to_json());
        assert!(a.contains("\"id\":\"L1\"") && a.contains("\"state\":\"granted\"") && a.contains("\"addr\":\"10.0.0.217\"") && a.contains("CARGO_INCREMENTAL"), "{a}");
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
        // a new master continues the numbering
        let (m2, node2, ..) = master(&dir);
        m2.refresh(&hosts);
        assert!(serve(&m2, "lease", &Req::new("gapx", Kind::Test).to_json()).contains("\"id\":\"L2\""));
        // the node forgot a lease (restarted): lost, warn
        node2.known.lock().unwrap().clear();
        m2.beat_nodes();
        assert!(serve(&m2, "beat", "L2").contains("\"how\":\"lost\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dev_switch_and_inspect_holders() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-test2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nodes.conf"), "217 addr=10.0.0.217 # pool\n").unwrap();
        let (m, node, _clock, recs) = master(&dir);
        m.refresh(&[("217".to_string(), "10.0.0.217".to_string())]);
        assert!(serve(&m, "node-dev", "217 off").contains("\"dev\":false"));
        assert_eq!(std::fs::read_to_string(dir.join("nodes.conf")).unwrap(), "217 addr=10.0.0.217 dev=off # pool\n");
        assert!(node.calls.lock().unwrap().iter().any(|c| c.2 == "policy dev=off"));
        assert!(serve(&m, "lease", &Req::new("x", Kind::Build).to_json()).contains("\"reason\":\"node off\""));
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
