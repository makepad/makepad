//! The lease scheduler: who gets which node, and why the rest wait (DESIGN.md §10.2, §10.8).
//!
//! Pure: no IO and no clock of its own (every call takes `now` in ms). Mutators queue
//! [`Action`]s; [`Sched::tick`] places what can start and hands them all to the master, which
//! talks to the nodes and publishes the events.
//!
//! Order: priority (lower first; aged by one per 10 min waiting), then fair share (the lane with
//! the oldest last grant), then request time. A lease waits (never refused) for a global or lane
//! cap, a node's slots, budget, disk floor, pressure, dev switch or a timed hold; it is denied
//! only when no node could ever take it.

use super::{Caps, Cost, Kind, Role};
use crate::proto::{hub_record, Record, Sev};
use makepad_remote::json_str;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// No beat from the holder for this long: the lease expires (holders beat every 2 s; a holder
/// on the master's machine is also watched by its pid, which ends its lease within a second).
pub const BEAT_EXPIRY_MS: u64 = 10_000;
/// A waiting lease's priority improves by one per this much waiting (starvation guard).
pub const AGE_STEP_MS: u64 = 600_000;
/// A wait this long becomes a warn (once).
pub const WAIT_WARN_MS: u64 = 600_000;
pub const DEFAULT_PRIO: i32 = 5;
/// Running leases per lane and kind.
pub const LANE_CAP: u32 = 2;
/// A granted gate lease that runs nothing this long ends: an idle gate slot blocks landings.
pub const GATE_IDLE_MS: u64 = 600_000;
/// The lane whose gates are landings: first in line for the global gate cap.
pub const LANDING_LANE: &str = "landgate";
/// A node's hub is asked to reclaim disk at most this often.
const RECLAIM_EVERY_MS: u64 = 300_000;
/// Ended leases kept for late beats and the view.
const KEEP_ENDED: usize = 200;

/// Fleet-wide running leases per kind (`inspect` and `timed` have none).
pub fn global_cap(kind: Kind) -> Option<u32> {
    match kind {
        Kind::Test => Some(8),
        Kind::Build => Some(6),
        Kind::Long => Some(4),
        Kind::Screen => Some(8),
        Kind::Gpu => Some(4),
        Kind::Gate => Some(4),
        Kind::Timed | Kind::Inspect => None,
    }
}

/// A node's slots for `kind`: its lease.conf, else the defaults (build 2, test 2, screen 3,
/// long 2, gate 1, gpu = its GPUs, timed 1).
pub fn slots(c: &Caps, kind: Kind) -> u32 {
    if !c.slots.is_empty() {
        return c.slots(kind);
    }
    match kind {
        Kind::Build | Kind::Test | Kind::Long => 2,
        Kind::Screen => 3,
        Kind::Gate | Kind::Timed => 1,
        Kind::Gpu => c.gpus.len() as u32,
        Kind::Inspect => u32::MAX,
    }
}

/// Builds keep a target dir on the node: the lane's next build goes there warm.
fn buildish(kind: Kind) -> bool {
    matches!(kind, Kind::Build | Kind::Test | Kind::Gate)
}

/// A lease request (the `lease` service's JSON line).
#[derive(Clone, Debug, PartialEq)]
pub struct Req {
    pub lane: String,
    pub kind: Kind,
    /// exactly this node
    pub node: Option<String>,
    /// `arch os gpu ram disk cores display role` requirements
    pub needs: Vec<(String, String)>,
    pub prio: i32,
    /// `<makepad branch|sha>[,<commercial>]`: a worktree at that revision (else the lane's src dir)
    pub rev: Option<String>,
    /// the lane's repo on the master (rev lookup, recovered work)
    pub repo_dir: Option<String>,
    /// inspect leases: the JIT (pid or app)
    pub jit: Option<String>,
    /// the holder process on the master's machine (the CLI that asked): its end ends the lease
    pub holder: Option<u32>,
    /// the agent behind the lane (`DEVHUB_AGENT`): what the panel shows and `evict` can name
    pub agent: Option<String>,
}

impl Req {
    pub fn new(lane: &str, kind: Kind) -> Req {
        Req { lane: lane.to_string(), kind, node: None, needs: Vec::new(), prio: DEFAULT_PRIO, rev: None, repo_dir: None, jit: None, holder: None, agent: None }
    }

    pub fn to_json(&self) -> String {
        let opt = |o: &Option<String>| o.as_deref().map_or("null".to_string(), json_str);
        let needs: Vec<String> = self.needs.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        format!(
            "{{\"lane\":{},\"kind\":\"{}\",\"node\":{},\"needs\":{{{}}},\"prio\":{},\"rev\":{},\"repo_dir\":{},\"jit\":{},\"holder\":{},\"agent\":{}}}",
            json_str(&self.lane),
            self.kind.as_str(),
            opt(&self.node),
            needs.join(","),
            self.prio,
            opt(&self.rev),
            opt(&self.repo_dir),
            opt(&self.jit),
            self.holder.map_or("null".to_string(), |p| p.to_string()),
            opt(&self.agent)
        )
    }

    pub fn parse(text: &str) -> Result<Req, String> {
        let v = makepad_strict_json::parse(text.as_bytes()).map_err(|e| format!("bad lease request: {e:?}"))?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
        let lane = s("lane").filter(|l| crate::trust::valid_name(l)).ok_or("a lease needs a lane (a-z 0-9 - _)")?;
        let kind = s("kind").and_then(|k| Kind::parse(&k)).ok_or("unknown lease kind")?;
        let mut r = Req::new(&lane, kind);
        r.node = s("node");
        r.prio = v.get("prio").and_then(super::num).map_or(DEFAULT_PRIO, |p| p as i32);
        r.rev = s("rev");
        r.repo_dir = s("repo_dir");
        r.jit = s("jit");
        r.agent = s("agent").filter(|a| crate::trust::valid_name(a));
        r.holder = v.get("holder").and_then(super::num).map(|p| p as u32).filter(|p| *p > 1);
        if let Some(makepad_strict_json::Value::Obj(pairs)) = v.get("needs") {
            for (k, x) in pairs {
                r.needs.push((k.clone(), x.as_str().map(str::to_string).or_else(|| super::num(x).map(|n| n.to_string())).unwrap_or_default()));
            }
        }
        if kind == Kind::Inspect && (r.node.is_none() || r.jit.is_none()) {
            return Err("an inspect lease names its JIT: --node N and the pid or app".into());
        }
        Ok(r)
    }
}

/// `--needs k=v,k=v` as pairs.
pub fn parse_needs(s: &str) -> Vec<(String, String)> {
    s.split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Queued,
    /// granted; the node is setting it up (`lease-start`)
    Starting,
    Granted,
    Ended(String),
}

impl State {
    pub fn as_str(&self) -> &str {
        match self {
            State::Queued => "queued",
            State::Starting => "starting",
            State::Granted => "granted",
            State::Ended(_) => "ended",
        }
    }

    fn live(&self) -> bool {
        matches!(self, State::Starting | State::Granted)
    }
}

#[derive(Clone, Debug)]
pub struct Lease {
    pub id: String,
    pub req: Req,
    pub state: State,
    pub node: Option<String>,
    pub cost: Cost,
    pub asked_ms: u64,
    pub granted_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    pub beat_ms: u64,
    /// 1-based among the waiting leases of its kind
    pub pos: usize,
    pub reason: String,
    warned: bool,
    /// the node's `lease-start` answer (work, target, env)
    pub start: String,
    /// what the node says it runs, its work dir and disk use (`lease-beat` info)
    pub info: String,
    /// since when a granted gate lease runs nothing (`info`'s `running` empty)
    idle_since: Option<u64>,
    seq: u64,
}

impl Lease {
    /// `L12 build@217` (the node once placed).
    pub fn tag(&self) -> String {
        match &self.node {
            Some(n) => format!("{} {}@{n}", self.id, self.req.kind.as_str()),
            None => format!("{} {}", self.id, self.req.kind.as_str()),
        }
    }

    /// The holder's view: one JSON line (the `lease` and `beat` answers).
    pub fn state_line(&self, now: u64, addr: &str) -> String {
        let opt = |o: &Option<String>| o.as_deref().map_or("null".to_string(), json_str);
        let how = match &self.state {
            State::Ended(h) => json_str(h),
            _ => "null".into(),
        };
        format!(
            "{{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"state\":\"{}\",\"node\":{},\"addr\":{},\"pos\":{},\"reason\":{},\"prio\":{},\"wait_s\":{},\"how\":{},\"jit\":{},\"agent\":{},\"beat_s\":{},\"granted_s\":{},\"info\":{},\"start\":{}}}",
            json_str(&self.id),
            json_str(&self.req.lane),
            self.req.kind.as_str(),
            self.state.as_str(),
            opt(&self.node),
            json_str(addr),
            self.pos,
            json_str(&self.reason),
            self.req.prio,
            (self.granted_ms.unwrap_or(now).saturating_sub(self.asked_ms)) / 1000,
            how,
            opt(&self.req.jit),
            opt(&self.req.agent),
            now.saturating_sub(self.beat_ms) / 1000,
            self.granted_ms.map_or("null".to_string(), |g| (now.saturating_sub(g) / 1000).to_string()),
            if self.info.starts_with('{') { self.info.as_str() } else { "null" },
            if self.start.starts_with('{') { self.start.as_str() } else { "null" }
        )
    }

    /// The ledger line.
    pub fn ledger_line(&self) -> String {
        let how = match &self.state {
            State::Ended(h) => h.as_str(),
            _ => "",
        };
        let ms = |o: Option<u64>| o.map_or("null".to_string(), |v| v.to_string());
        format!(
            "{{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"node\":{},\"asked_ms\":{},\"granted_ms\":{},\"ended_ms\":{},\"how\":{},\"req\":{}}}",
            json_str(&self.id),
            json_str(&self.req.lane),
            self.req.kind.as_str(),
            self.node.as_deref().map_or("null".to_string(), json_str),
            self.asked_ms,
            ms(self.granted_ms),
            ms(self.ended_ms),
            json_str(how),
            self.req.to_json()
        )
    }
}

/// What the master does after a scheduler call.
#[derive(Clone, Debug)]
pub enum Action {
    /// set the lease up on the node (`lease-start`), then [`Sched::started`] or [`Sched::failed`]
    Grant { id: String, node: String },
    /// tear it down on the node (`lease-end`) when `node` is set; write the ledger
    End { id: String, node: Option<String>, how: String },
    /// the node is below its disk floor with a lease waiting: reclaim (`disk`)
    Reclaim { node: String },
    Event(Record),
}

#[derive(Clone, Debug, Default)]
struct NodeSt {
    addr: String,
    caps: Option<Caps>,
    caps_ms: u64,
    up: bool,
    /// the user's switch (nodes.conf); unset = by role
    dev: Option<bool>,
    reclaim_ms: Option<u64>,
}

/// One node as the explorer and `nodes` show it.
#[derive(Clone, Debug, Default)]
pub struct NodeView {
    pub name: String,
    pub addr: String,
    pub up: bool,
    pub dev: bool,
    /// the user switched it (else the role's default)
    pub dev_set: bool,
    pub caps: Option<Caps>,
    pub leases: usize,
    pub waiting: usize,
    /// dev off with leases still running
    pub draining: bool,
    /// a timed lease holds the node
    pub hold: Option<String>,
}

impl NodeView {
    pub fn role(&self) -> Role {
        self.caps.as_ref().map_or(Role::Pool, |c| c.role())
    }

    /// `x86_64 linux · 16 cores · load 2.1 · 40/60 GB · disk 105 GB free (floor 25) · RTX 5090`.
    pub fn caps_line(&self) -> String {
        let Some(c) = &self.caps else { return if self.up { "no caps yet".into() } else { "down".into() } };
        let cpu = c.cpu_pct.map_or(String::new(), |p| format!("cpu {p:.0}% \u{b7} "));
        let mut s = format!(
            "{} {} \u{b7} {} cores \u{b7} {cpu}load {:.1} \u{b7} {:.0}/{:.0} GB free \u{b7} disk {:.0} GB free (floor {:.0})",
            c.arch, c.os, c.cores, c.load1, c.ram_avail_gb, c.ram_gb, c.disk_free_gb, c.disk_floor_gb
        );
        for (i, (g, v)) in c.gpus.iter().enumerate() {
            let busy = c.gpu_busy.get(i).map_or(String::new(), |(u, m)| format!(" {u:.0}% {m:.0}/"));
            s.push_str(&format!(" \u{b7} {}{busy}{}{v:.0} GB", g.trim_start_matches("NVIDIA ").trim_start_matches("GeForce "), if busy.is_empty() { " " } else { "" }));
        }
        if let Some((p, ch)) = c.battery {
            s.push_str(&format!(" \u{b7} battery {p}%{}", if ch { " charging" } else { "" }));
        }
        if !c.spec.is_empty() {
            s.push_str(&format!(" \u{b7} {}", c.spec));
        }
        if let Some(d) = c.desk_link.as_ref().filter(|d| d.on) {
            s.push_str(&if d.port > 0 { format!(" \u{b7} desk link :{}", d.port) } else { " \u{b7} desk link waiting".to_string() });
        }
        s
    }

    /// `dev on`, `dev off (user WM)`, `draining (2 leases)`, `held for timed L9`.
    pub fn state_word(&self) -> String {
        let mut s = if !self.up {
            "down".to_string()
        } else if self.draining {
            format!("draining ({} leases)", self.leases)
        } else if self.dev {
            "dev on".to_string()
        } else if self.role().users() {
            format!("dev off ({})", if self.role() == Role::Device { "device" } else { "user WM" })
        } else {
            "dev off".to_string()
        };
        if let Some(h) = &self.hold {
            s.push_str(&format!(", held for timed {h}"));
        }
        s
    }

    pub fn to_json(&self) -> String {
        format!(
            "{{\"node\":{},\"addr\":{},\"up\":{},\"dev\":{},\"dev_set\":{},\"role\":\"{}\",\"leases\":{},\"waiting\":{},\"draining\":{},\"hold\":{},\"state\":{},\"caps_line\":{},\"caps\":{}}}",
            json_str(&self.name),
            json_str(&self.addr),
            self.up,
            self.dev,
            self.dev_set,
            self.role().as_str(),
            self.leases,
            self.waiting,
            self.draining,
            self.hold.as_deref().map_or("null".to_string(), json_str),
            json_str(&self.state_word()),
            json_str(&self.caps_line()),
            self.caps.as_ref().map_or("null".to_string(), |c| c.to_json())
        )
    }
}

enum Place {
    Grant(String, Cost),
    /// reason, the node a timed lease holds, the node to reclaim disk on
    Wait(String, Option<String>, Option<String>),
    Deny(String),
}

/// How long a lane stays off a node it cancelled a lease on.
pub const CANCEL_EXCLUDE_MS: u64 = 30 * 60 * 1000;

pub struct Sched {
    machine: String,
    nodes: BTreeMap<String, NodeSt>,
    /// queued and live
    leases: Vec<Lease>,
    ended: VecDeque<Lease>,
    next: u64,
    seq: u64,
    last_grant: HashMap<String, u64>,
    sticky: HashMap<String, String>,
    /// (lane, node) -> until (ms): a node the user cancelled that lane's lease on; not chosen
    /// for it again for [`CANCEL_EXCLUDE_MS`], unless the request names the node
    excluded: HashMap<(String, String), u64>,
    /// the last tick's time (placement has no clock of its own)
    clock: u64,
    warm: HashSet<(String, String)>,
    /// node -> the timed lease holding it (last tick)
    holds: HashMap<String, String>,
    out: Vec<Action>,
    pub lane_cap: u32,
    /// lanes and agents the user evicted: no new leases until unblocked
    pub blocked: Vec<String>,
}

/// `1st 2nd 3rd 4th … 11th 12th 13th … 21st`.
pub fn ordinal(n: usize) -> String {
    let suf = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suf}")
}

fn dur(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s / 60) % 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

/// `32`, `32G`, `32GB` as a number.
fn gb(v: &str) -> Option<f64> {
    v.trim().trim_end_matches(['B', 'b']).trim_end_matches(['G', 'g']).parse().ok()
}

/// Does the node meet one `--needs` requirement?
fn meets(c: &Caps, k: &str, v: &str) -> bool {
    let lv = v.to_ascii_lowercase();
    match k {
        "arch" => {
            let a = c.arch.to_ascii_lowercase();
            a == lv || (lv == "x64" && a == "x86_64") || (lv == "arm64" && a == "aarch64") || (lv == "aarch64" && a == "arm64")
        }
        "os" => c.os.eq_ignore_ascii_case(v),
        "gpu" => c.gpus.iter().any(|(n, _)| n.to_ascii_lowercase().contains(&lv)),
        "ram" => gb(v).is_some_and(|g| c.ram_gb >= g),
        "disk" => gb(v).is_some_and(|g| c.disk_free_gb >= g),
        "cores" => gb(v).is_some_and(|g| c.cores as f64 >= g),
        "display" => match c.display.as_deref() {
            Some(d) => lv.is_empty() || matches!(lv.as_str(), "1" | "yes" | "on" | "true") || d.to_ascii_lowercase().contains(&lv),
            None => matches!(lv.as_str(), "0" | "no" | "off" | "false"),
        },
        "role" => c.role().as_str() == lv,
        // a test lease that opens windows (GUI tests, hidden presents): only nodes that present,
        // i.e. take screen leases (106 is at slots.screen=0 while its presents time out)
        "gui" => slots(c, Kind::Screen) > 0 || matches!(lv.as_str(), "0" | "no" | "off" | "false"),
        _ => false,
    }
}

impl Sched {
    /// `machine`: the master's name (the records' machine); `next`: the first lease number.
    pub fn new(machine: &str, next: u64) -> Sched {
        Sched {
            machine: machine.to_string(),
            nodes: BTreeMap::new(),
            leases: Vec::new(),
            ended: VecDeque::new(),
            next: next.max(1),
            seq: 0,
            last_grant: HashMap::new(),
            sticky: HashMap::new(),
            excluded: HashMap::new(),
            clock: 0,
            warm: HashSet::new(),
            holds: HashMap::new(),
            out: Vec::new(),
            lane_cap: LANE_CAP,
            blocked: Vec::new(),
        }
    }

    fn event(&mut self, sev: Sev, line: String, l: Option<&Lease>) {
        let r = hub_record(&self.machine, "", 0, "lease", sev, line, l.map(|l| l.ledger_line()).unwrap_or_default());
        self.out.push(Action::Event(r));
    }

    /// A node the master dials (seeds, gossip); `dev` = its nodes.conf switch when set.
    pub fn add_node(&mut self, name: &str, addr: &str, dev: Option<bool>) {
        let n = self.nodes.entry(name.to_string()).or_default();
        n.addr = addr.to_string();
        if dev.is_some() {
            n.dev = dev;
        }
    }

    /// Fresh caps from the node's hub; true when the node was down (push it its policy).
    pub fn set_caps(&mut self, name: &str, caps: Caps, now: u64) -> bool {
        let n = self.nodes.entry(name.to_string()).or_default();
        let was = n.up;
        n.caps = Some(caps);
        n.caps_ms = now;
        n.up = true;
        !was
    }

    pub fn set_down(&mut self, name: &str) {
        if let Some(n) = self.nodes.get_mut(name) {
            n.up = false;
        }
    }

    /// The node's dev switch (the user's, else on: every node, user WMs and devices included, defaults to dev on).
    pub fn dev(&self, name: &str) -> bool {
        self.nodes.get(name).is_some_and(|n| n.dev.unwrap_or(true))
    }

    /// The user switches a node; running leases are never ended (the node drains).
    pub fn set_dev(&mut self, name: &str, on: bool, now: u64) -> Result<(), String> {
        let n = self.nodes.get_mut(name).ok_or_else(|| format!("no node {name}"))?;
        n.dev = Some(on);
        let live = self.leases.iter().filter(|l| l.state.live() && l.node.as_deref() == Some(name)).count();
        let line = match (on, live) {
            (true, _) => format!("node {name} dev on: takes leases"),
            (false, 0) => format!("node {name} dev off: no new leases"),
            (false, n) => format!("node {name} dev off: draining ({n} leases)"),
        };
        self.event(Sev::Info, line, None);
        let _ = now;
        Ok(())
    }

    /// Queues a request; the id (`L12`). Placement happens at the next [`Sched::tick`].
    pub fn request(&mut self, req: Req, now: u64) -> String {
        let id = format!("L{}", self.next);
        self.next += 1;
        self.seq += 1;
        let running = self.leases.iter().filter(|l| l.state.live() && l.req.lane == req.lane && l.req.kind == req.kind).count() as u32;
        let l = Lease {
            id: id.clone(),
            cost: Cost::default_for(req.kind, false),
            req,
            state: State::Queued,
            node: None,
            asked_ms: now,
            granted_ms: None,
            ended_ms: None,
            beat_ms: now,
            pos: 0,
            reason: String::new(),
            warned: false,
            start: String::new(),
            info: String::new(),
            idle_since: None,
            seq: self.seq,
        };
        if let Some(who) = self.blocked_of(&l.req) {
            self.leases.push(l);
            let i = self.leases.len() - 1;
            self.end_at(i, "denied", Sev::Warn, &format!("evicted by the user ({who} is blocked until `devhub unblock {who}`)"), now);
            return id;
        }
        if l.req.kind != Kind::Inspect && running >= self.lane_cap {
            self.event(Sev::Warn, format!("{} for {}: over the lane cap ({running}/{} running), queued", l.tag(), l.req.lane, self.lane_cap), Some(&l));
        }
        self.leases.push(l);
        id
    }

    pub fn get(&self, id: &str) -> Option<&Lease> {
        self.leases.iter().find(|l| l.id == id).or_else(|| self.ended.iter().find(|l| l.id == id))
    }

    pub fn addr(&self, node: &str) -> String {
        self.nodes.get(node).map(|n| n.addr.clone()).unwrap_or_default()
    }

    /// The holder's heartbeat; its state line (None: no such lease).
    pub fn beat(&mut self, id: &str, now: u64) -> Option<String> {
        if let Some(l) = self.leases.iter_mut().find(|l| l.id == id) {
            l.beat_ms = now;
        }
        let l = self.get(id)?;
        Some(l.state_line(now, &l.node.as_deref().map(|n| self.addr(n)).unwrap_or_default()))
    }

    fn end_at(&mut self, i: usize, how: &str, sev: Sev, why: &str, now: u64) {
        let mut l = self.leases.remove(i);
        let node = if l.state.live() { l.node.clone() } else { None };
        let held = l.granted_ms.map(|g| now.saturating_sub(g));
        l.state = State::Ended(how.to_string());
        l.ended_ms = Some(now);
        l.reason = why.to_string();
        let line = match (how, held) {
            ("denied", _) => format!("{} for {} denied: {why}", l.tag(), l.req.lane),
            ("expired", _) => format!("{} of {} expired: no heartbeat for {} s{}", l.tag(), l.req.lane, BEAT_EXPIRY_MS / 1000, if node.is_some() { "; cleaning up" } else { " (was queued)" }),
            ("cancelled", _) => format!("{} of {} cancelled by user{}", l.tag(), l.req.lane, if node.is_some() { "; cleaning up" } else { " (was queued)" }),
            ("evicted", _) => format!("{} of {} ended: evicted by the user{}", l.tag(), l.req.lane, if node.is_some() { "; cleaning up" } else { "" }),
            ("overridden", _) => format!("{} of {} ended: {why}{}", l.tag(), l.req.lane, if node.is_some() { "; cleaning up" } else { "" }),
            ("holder-dead", _) => format!("{} of {} released: its holder (pid {}) ended{}", l.tag(), l.req.lane, l.req.holder.unwrap_or(0), if node.is_some() { "; cleaning up" } else { " (was queued)" }),
            (_, Some(h)) => format!("{} of {} ended ({how}{}) after {}", l.tag(), l.req.lane, if why.is_empty() { String::new() } else { format!(": {why}") }, dur(h)),
            (_, None) => format!("{} for {} left the queue ({how}{})", l.tag(), l.req.lane, if why.is_empty() { String::new() } else { format!(": {why}") }),
        };
        self.event(sev, line, Some(&l));
        self.out.push(Action::End { id: l.id.clone(), node, how: how.to_string() });
        self.ended.push_back(l);
        while self.ended.len() > KEEP_ENDED {
            self.ended.pop_front();
        }
    }

    /// The user cancels one lease: its processes stop and its slot frees (the node cleans up like
    /// a release), it ends as `cancelled`, and the lane is kept off that node for a while. A
    /// queued lease just leaves the queue. False: not live or queued.
    pub fn cancel(&mut self, id: &str, now: u64) -> bool {
        let Some(i) = self.leases.iter().position(|l| l.id == id) else { return false };
        let (lane, node) = (self.leases[i].req.lane.clone(), if self.leases[i].state.live() { self.leases[i].node.clone() } else { None });
        if let Some(n) = node {
            self.excluded.insert((lane, n), now + CANCEL_EXCLUDE_MS);
        }
        self.end_at(i, "cancelled", Sev::Warn, "cancelled by user", now);
        true
    }

    /// Release (`how` = released | holder-dead | lost | failed …); false: not live or queued.
    pub fn release(&mut self, id: &str, how: &str, now: u64) -> bool {
        match self.leases.iter().position(|l| l.id == id) {
            Some(i) => {
                // anything but the holder's own release is worth a look
                self.end_at(i, how, if how == "released" { Sev::Info } else { Sev::Warn }, "", now);
                true
            }
            None => false,
        }
    }

    fn blocked_of(&self, r: &Req) -> Option<String> {
        self.blocked.iter().find(|b| **b == r.lane || r.agent.as_deref() == Some(b.as_str())).cloned()
    }

    /// The node's word on a live lease (running, work, disk).
    pub fn set_info(&mut self, id: &str, info: &str) {
        if let Some(l) = self.leases.iter_mut().find(|l| l.id == id) {
            l.info = info.to_string();
        }
    }

    /// The user evicts a lane or agent: every lease of it ends (`evicted`) and it is blocked.
    /// Returns the leases ended.
    pub fn evict(&mut self, who: &str, now: u64) -> Vec<String> {
        if !self.blocked.iter().any(|b| b == who) {
            self.blocked.push(who.to_string());
        }
        let ids: Vec<String> = self.leases.iter().filter(|l| l.req.lane == who || l.req.agent.as_deref() == Some(who)).map(|l| l.id.clone()).collect();
        for id in &ids {
            if let Some(i) = self.leases.iter().position(|l| l.id == *id) {
                self.end_at(i, "evicted", Sev::Warn, "evicted by the user", now);
            }
        }
        self.event(Sev::Warn, format!("{who} evicted by the user: {} ended, no new leases until `devhub unblock {who}`", ids.len()), None);
        ids
    }

    pub fn unblock(&mut self, who: &str) -> bool {
        let had = self.blocked.iter().any(|b| b == who);
        self.blocked.retain(|b| b != who);
        if had {
            self.event(Sev::Info, format!("{who} unblocked by the user"), None);
        }
        had
    }

    /// The user takes a node now: its live leases end (`overridden`), except `to`'s; with `to`,
    /// that lane's waiting leases go to the front of the queue, pinned to the node.
    pub fn override_node(&mut self, node: &str, to: Option<&str>, now: u64) -> Result<Vec<String>, String> {
        if !self.nodes.contains_key(node) {
            return Err(format!("no node {node}"));
        }
        let ids: Vec<String> = self.leases.iter().filter(|l| l.state.live() && l.node.as_deref() == Some(node) && Some(l.req.lane.as_str()) != to).map(|l| l.id.clone()).collect();
        let who = to.map_or("the user".to_string(), |t| format!("lane {t}"));
        for id in &ids {
            if let Some(i) = self.leases.iter().position(|l| l.id == *id) {
                self.end_at(i, "overridden", Sev::Warn, &format!("{node} given to {who}"), now);
            }
        }
        let mut moved = 0;
        if let Some(t) = to {
            for l in self.leases.iter_mut().filter(|l| l.state == State::Queued && l.req.lane == t) {
                l.req.prio = i32::MIN / 2;
                l.req.node = Some(node.to_string());
                moved += 1;
            }
        }
        self.event(Sev::Warn, format!("node {node} overridden by the user: given to {who}; {} ended{}", ids.len(), if to.is_some() { format!(", {moved} of its waiting leases to the front") } else { String::new() }), None);
        Ok(ids)
    }

    /// Leases whose holder process (on the master's machine) is gone.
    pub fn holders_gone(&self, alive: &dyn Fn(u32) -> bool) -> Vec<String> {
        self.leases.iter().filter(|l| l.req.holder.is_some_and(|p| !alive(p))).map(|l| l.id.clone()).collect()
    }

    /// The node set the lease up: `answer` is its `lease-start` line.
    pub fn started(&mut self, id: &str, answer: &str) {
        if let Some(l) = self.leases.iter_mut().find(|l| l.id == id && l.state == State::Starting) {
            l.state = State::Granted;
            l.start = answer.to_string();
        }
    }

    /// The granted leases, one line each (`id`, `node`, `granted_ms`, `req`, `start`): what a
    /// restarted master takes back ([`Sched::restore`]).
    pub fn live_lines(&self) -> Vec<String> {
        self.leases
            .iter()
            .filter(|l| l.state == State::Granted)
            .map(|l| format!("{{\"id\":{},\"node\":{},\"granted_ms\":{},\"req\":{},\"start\":{}}}", json_str(&l.id), json_str(l.node.as_deref().unwrap_or("")), l.granted_ms.unwrap_or(0), l.req.to_json(), json_str(&l.start)))
            .collect()
    }

    /// Takes back a lease a previous master granted ([`Sched::live_lines`]); its holder has the
    /// usual 30 s to beat again from `now`.
    pub fn restore(&mut self, line: &str, now: u64) -> Option<String> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let id = v.get("id")?.as_str()?.to_string();
        let n: u64 = id.strip_prefix('L')?.parse().ok()?;
        let req = Req::parse(&v.get("req")?.to_json()).ok()?;
        self.next = self.next.max(n + 1);
        self.seq += 1;
        let l = Lease {
            id: id.clone(),
            cost: Cost::default_for(req.kind, false),
            req,
            state: State::Granted,
            node: v.get("node").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string),
            asked_ms: now,
            granted_ms: v.get("granted_ms").and_then(super::num).map(|x| x as u64),
            ended_ms: None,
            beat_ms: now,
            pos: 0,
            reason: String::new(),
            warned: false,
            start: v.get("start").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            info: String::new(),
            idle_since: None,
            seq: self.seq,
        };
        self.leases.push(l);
        Some(id)
    }

    /// The node could not set it up: the lease ends (`failed`, warn).
    pub fn failed(&mut self, id: &str, err: &str, now: u64) {
        if let Some(i) = self.leases.iter().position(|l| l.id == id) {
            self.end_at(i, "failed", Sev::Warn, err, now);
        }
    }

    /// Ids of the started leases on `node` (the master's `lease-beat` to it).
    /// Inspect leases are not set up on the node (no `lease-start`), so they are not beaten there.
    pub fn live_on(&self, node: &str) -> Vec<String> {
        self.leases.iter().filter(|l| l.state == State::Granted && l.req.kind != Kind::Inspect && l.node.as_deref() == Some(node)).map(|l| l.id.clone()).collect()
    }

    /// Nodes with started leases.
    pub fn nodes_with_leases(&self) -> Vec<String> {
        let mut v: Vec<String> = self.leases.iter().filter(|l| l.state == State::Granted).filter_map(|l| l.node.clone()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Queued and live leases, in queue order for the waiting ones.
    pub fn view(&self) -> Vec<Lease> {
        let mut v = self.leases.clone();
        v.sort_by_key(|l| (!l.state.live(), l.pos, l.seq));
        v
    }

    pub fn recent_ended(&self, n: usize) -> Vec<Lease> {
        self.ended.iter().rev().take(n).cloned().collect()
    }

    pub fn node_view(&self) -> Vec<NodeView> {
        self.nodes
            .iter()
            .map(|(name, n)| {
                let leases = self.leases.iter().filter(|l| l.state.live() && l.node.as_deref() == Some(name.as_str())).count();
                let waiting = self.leases.iter().filter(|l| l.state == State::Queued && l.req.node.as_deref() == Some(name.as_str())).count();
                let dev = self.dev(name);
                NodeView {
                    name: name.clone(),
                    addr: n.addr.clone(),
                    up: n.up,
                    dev,
                    dev_set: n.dev.is_some(),
                    caps: n.caps.clone(),
                    leases,
                    waiting,
                    draining: !dev && leases > 0,
                    hold: self.holds.get(name).cloned(),
                }
            })
            .collect()
    }

    fn live_on_node<'a>(&'a self, node: &'a str, except: &'a str) -> impl Iterator<Item = &'a Lease> + 'a {
        self.leases.iter().filter(move |l| l.state.live() && l.node.as_deref() == Some(node) && l.id != except)
    }

    /// Why the node can never take the lease (`Err`), however long it waits.
    fn never(&self, name: &str, c: &Caps, l: &Lease, cost: &Cost) -> Result<(), String> {
        let k = l.req.kind;
        if k == Kind::Timed && !c.timed {
            return Err("takes no timed leases".into());
        }
        if k != Kind::Inspect && slots(c, k) == 0 {
            return Err(format!("takes no {} leases", k.as_str()));
        }
        if k.writes() && c.role().users() && !self.dev(name) {
            return Err(format!("{}: dev off", if c.role() == Role::Device { "device" } else { "user WM" }));
        }
        for (key, v) in &l.req.needs {
            if !meets(c, key, v) {
                return Err(format!("needs {key}={v}"));
            }
        }
        // a Windows node runs no shell scripts and no Unix tools: only what asks for it
        if c.os.eq_ignore_ascii_case("windows") && l.req.node.is_none() && !l.req.needs.iter().any(|(k, v)| k == "os" && v.eq_ignore_ascii_case("windows")) {
            return Err("windows: only leases that name it (--node or --needs os=windows)".into());
        }
        if cost.cores > c.cores as f64 {
            return Err(format!("needs {:.0} cores, has {}", cost.cores, c.cores));
        }
        if cost.ram_gb > c.ram_gb * 0.9 {
            return Err(format!("needs {:.0} GB RAM, has {:.0}", cost.ram_gb, c.ram_gb));
        }
        if cost.gpus as usize > c.gpus.len() {
            return Err("has no GPU".into());
        }
        if c.disk_total_gb > 0.0 && cost.disk_gb > c.disk_total_gb - c.disk_floor_gb {
            return Err(format!("needs {:.0} GB disk, the volume holds {:.0} over its floor", cost.disk_gb, c.disk_total_gb - c.disk_floor_gb));
        }
        Ok(())
    }

    /// Can the node take it now? Ok(headroom score) or Err((reason, disk floor hit)).
    fn now_fits(&self, name: &str, l: &Lease, cost: &Cost, holds: &HashMap<String, String>) -> Result<f64, (String, bool)> {
        let n = &self.nodes[name];
        let c = n.caps.as_ref().unwrap();
        let k = l.req.kind;
        if !n.up {
            return Err(("node down".into(), false));
        }
        if k == Kind::Inspect {
            return Ok(0.0);
        }
        if !self.dev(name) {
            return Err(("node off".into(), false));
        }
        let running: Vec<&Lease> = self.live_on_node(name, &l.id).filter(|r| r.req.kind != Kind::Inspect).collect();
        if let Some(t) = running.iter().find(|r| r.cost.whole) {
            return Err((format!("timed {} running", t.id), false));
        }
        if let Some(h) = holds.get(name).filter(|h| **h != l.id) {
            return Err((if k == Kind::Timed { format!("behind timed {h}") } else { format!("hold for timed {h}") }, false));
        }
        if k == Kind::Timed {
            if !running.is_empty() {
                return Err((format!("timed: waiting for {} leases to end", running.len()), false));
            }
        } else {
            let busy = c.load1 > c.cores as f64 * 1.5 || (c.ram_gb > 0.0 && c.ram_avail_gb < c.ram_gb * 0.1);
            if busy {
                return Err(("node busy".into(), false));
            }
            let used = running.iter().filter(|r| r.req.kind == k).count() as u32;
            let s = slots(c, k);
            if used >= s {
                return Err((format!("node slots {} {used}/{s}", k.as_str()), false));
            }
            let cores: f64 = running.iter().map(|r| r.cost.cores).sum();
            if cores + cost.cores > c.cores as f64 {
                return Err((format!("node budget: cores {:.0}+{:.0} > {}", cores, cost.cores, c.cores), false));
            }
            let ram: f64 = running.iter().map(|r| r.cost.ram_gb).sum();
            if ram + cost.ram_gb > c.ram_gb * 0.9 {
                return Err((format!("node budget: RAM {:.0}+{:.0} > {:.0} GB", ram, cost.ram_gb, c.ram_gb * 0.9), false));
            }
            let gpus: u32 = running.iter().map(|r| r.cost.gpus).sum();
            if gpus + cost.gpus > c.gpus.len() as u32 {
                return Err((format!("GPUs {gpus}/{} in use", c.gpus.len()), false));
            }
        }
        // what leases granted since the last measurement will still write
        let pending: f64 = running.iter().filter(|r| r.granted_ms.unwrap_or(0) >= n.caps_ms).map(|r| r.cost.disk_gb).sum();
        let free = c.disk_free_gb - pending;
        if free - cost.disk_gb < c.disk_floor_gb {
            return Err((format!("disk floor: {free:.0} GB free, needs {:.0}, floor {:.0}", cost.disk_gb, c.disk_floor_gb), true));
        }
        let cores: f64 = running.iter().map(|r| r.cost.cores).sum::<f64>() + cost.cores;
        let ram: f64 = running.iter().map(|r| r.cost.ram_gb).sum::<f64>() + cost.ram_gb;
        Ok((1.0 - cores / c.cores.max(1) as f64) + (c.ram_avail_gb.min(c.ram_gb - ram).max(0.0) / c.ram_gb.max(1.0)))
    }

    fn place(&self, l: &Lease, holds: &HashMap<String, String>) -> Place {
        let k = l.req.kind;
        let live = |f: &dyn Fn(&Lease) -> bool| self.leases.iter().filter(|x| x.state.live() && f(x)).count() as u32;
        if let Some(cap) = global_cap(k) {
            let n = live(&|x| x.req.kind == k);
            if n >= cap {
                return Place::Wait(format!("global cap {n}/{cap}"), None, None);
            }
        }
        if k != Kind::Inspect {
            let n = live(&|x| x.req.kind == k && x.req.lane == l.req.lane);
            if n >= self.lane_cap {
                return Place::Wait(format!("lane cap {n}/{}", self.lane_cap), None, None);
            }
        }
        let names: Vec<&String> = match &l.req.node {
            Some(n) => match self.nodes.get_key_value(n) {
                Some((k, _)) => vec![k],
                None => return Place::Deny(format!("no node {n}")),
            },
            None => {
                let all: Vec<&String> = self.nodes.keys().collect();
                let ok: Vec<&String> = all.iter().copied().filter(|n| self.excluded.get(&(l.req.lane.clone(), (*n).clone())).is_none_or(|until| *until <= self.clock)).collect();
                if ok.is_empty() && !all.is_empty() {
                    return Place::Wait("every node is one you cancelled this lane on (name a node to override)".into(), None, None);
                }
                ok
            }
        };
        let mut never = Vec::new();
        let mut fit = Vec::new();
        let mut unknown = 0;
        for name in names {
            let warm = self.warm.contains(&(l.req.lane.clone(), name.clone()));
            let cost = Cost::default_for(k, warm);
            match &self.nodes[name].caps {
                None => unknown += 1,
                Some(c) => match self.never(name, c, l, &cost) {
                    Ok(()) => fit.push((name, cost)),
                    Err(e) => never.push(if l.req.node.is_some() { e } else { format!("{name}: {e}") }),
                },
            }
        }
        if fit.is_empty() {
            return match (unknown, &l.req.node) {
                (0, _) if never.is_empty() => Place::Deny("no nodes".into()),
                (0, _) => Place::Deny(never.join("; ")),
                (_, Some(n)) => Place::Wait(format!("node {n} down"), None, None),
                (_, None) => Place::Wait("no fitting node up".into(), None, None),
            };
        }
        let sticky = self.sticky.get(&l.req.lane);
        let mut best: Option<(&String, Cost, f64)> = None;
        let mut why: Vec<(String, String, bool)> = Vec::new();
        for (name, cost) in &fit {
            match self.now_fits(name, l, cost, holds) {
                Ok(score) => {
                    let score = if sticky == Some(*name) { f64::MAX } else { score };
                    if best.as_ref().is_none_or(|b| score > b.2) {
                        best = Some((name, *cost, score));
                    }
                }
                Err((r, disk)) => why.push(((*name).clone(), r, disk)),
            }
        }
        if let Some((name, cost, _)) = best {
            return Place::Grant(name.clone(), cost);
        }
        // a timed lease holds one node: the named one, the sticky one, else the least busy
        let hold = (k == Kind::Timed).then(|| {
            let mut c: Vec<&String> = fit.iter().map(|(n, _)| *n).collect();
            c.sort_by_key(|n| (sticky != Some(*n), self.live_on_node(n, &l.id).count()));
            c[0].clone()
        });
        let first = why.iter().find(|w| Some(&w.0) == hold.as_ref()).or_else(|| why.iter().find(|w| Some(&w.0) == sticky)).unwrap_or(&why[0]);
        let reason = if fit.len() == 1 { first.1.clone() } else { format!("{}: {} (+{} nodes)", first.0, first.1, fit.len() - 1) };
        let reclaim = why.iter().find(|w| w.2).map(|w| w.0.clone());
        Place::Wait(reason, hold, reclaim)
    }

    /// Expires silent holders, places what fits, recomputes positions and reasons; the actions
    /// for the master (grants, ends, reclaims, one event per change).
    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        self.clock = now;
        self.excluded.retain(|_, until| *until > now);
        let expired: Vec<String> = self.leases.iter().filter(|l| now.saturating_sub(l.beat_ms) > BEAT_EXPIRY_MS).map(|l| l.id.clone()).collect();
        for id in expired {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "expired", Sev::Warn, "", now);
            }
        }
        // a gate lease holds one of the few gate slots: one that runs nothing gives it back
        let mut idle = Vec::new();
        for l in self.leases.iter_mut().filter(|l| l.req.kind == Kind::Gate && l.state.live()) {
            let running = makepad_strict_json::parse(l.info.as_bytes()).ok().and_then(|v| v.get("running").and_then(|r| r.as_str()).map(|r| !r.is_empty())).unwrap_or(false);
            if running {
                l.idle_since = None;
                continue;
            }
            let since = *l.idle_since.get_or_insert(l.granted_ms.unwrap_or(now));
            if now.saturating_sub(since) > GATE_IDLE_MS {
                idle.push(l.id.clone());
            }
        }
        for id in idle {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "idle", Sev::Warn, &format!("nothing ran on this gate lease for {} min, so it gave its gate slot back (landings wait for gate slots); take a new gate lease when the gate is ready to run", GATE_IDLE_MS / 60_000), now);
            }
        }
        let mut order: Vec<usize> = (0..self.leases.len()).filter(|&i| self.leases[i].state == State::Queued).collect();
        order.sort_by_key(|&i| {
            let l = &self.leases[i];
            let aged = l.req.prio as i64 - (now.saturating_sub(l.asked_ms) / AGE_STEP_MS) as i64;
            // landings' gates first, then priority; never granted sorts before a grant at time 0
            let landing = l.req.kind == Kind::Gate && l.req.lane == LANDING_LANE;
            (!landing, aged, self.last_grant.get(&l.req.lane).map_or(0, |t| t + 1), l.asked_ms, l.seq)
        });
        let mut holds: HashMap<String, String> = HashMap::new();
        let mut deny = Vec::new();
        let mut waits: Vec<(usize, String)> = Vec::new();
        for i in order {
            match self.place(&self.leases[i], &holds) {
                Place::Grant(node, cost) => {
                    let l = &mut self.leases[i];
                    l.state = State::Starting;
                    l.node = Some(node.clone());
                    l.cost = cost;
                    l.granted_ms = Some(now);
                    l.reason.clear();
                    l.pos = 0;
                    let (lane, kind, id) = (l.req.lane.clone(), l.req.kind, l.id.clone());
                    self.last_grant.insert(lane.clone(), now);
                    if buildish(kind) {
                        self.sticky.insert(lane.clone(), node.clone());
                        self.warm.insert((lane, node.clone()));
                    }
                    let l = self.leases[i].clone();
                    self.event(Sev::Info, format!("{} granted to {} after {}", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms))), Some(&l));
                    self.out.push(Action::Grant { id, node });
                }
                Place::Wait(reason, hold, reclaim) => {
                    if let Some(h) = hold {
                        holds.entry(h).or_insert_with(|| self.leases[i].id.clone());
                    }
                    if let Some(n) = reclaim {
                        let due = self.nodes.get(&n).is_some_and(|s| s.reclaim_ms.is_none_or(|t| now.saturating_sub(t) >= RECLAIM_EVERY_MS));
                        if due {
                            self.nodes.get_mut(&n).unwrap().reclaim_ms = Some(now);
                            let line = format!("node {n} below its disk floor with {} waiting: asked to reclaim", self.leases[i].id);
                            self.event(Sev::Info, line, None);
                            self.out.push(Action::Reclaim { node: n });
                        }
                    }
                    waits.push((i, reason));
                }
                Place::Deny(reason) => deny.push((self.leases[i].id.clone(), reason)),
            }
        }
        self.holds = holds;
        // positions among the waiting leases of a kind, in queue order
        let mut per_kind: HashMap<Kind, usize> = HashMap::new();
        let denied: HashSet<&String> = deny.iter().map(|(id, _)| id).collect();
        let mut changes = Vec::new();
        for (i, reason) in waits {
            if denied.contains(&self.leases[i].id) {
                continue;
            }
            let p = per_kind.entry(self.leases[i].req.kind).or_insert(0);
            *p += 1;
            let l = &mut self.leases[i];
            let warn = !l.warned && now.saturating_sub(l.asked_ms) >= WAIT_WARN_MS;
            if l.pos != *p || l.reason != reason || warn {
                l.pos = *p;
                l.reason = reason;
                l.warned |= warn;
                changes.push((i, warn));
            }
        }
        for (i, warn) in changes {
            let l = self.leases[i].clone();
            let line = if warn {
                format!("{} for {} waiting {}: {} in queue ({})", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms)), ordinal(l.pos), l.reason)
            } else {
                format!("{} for {}: {} in queue ({})", l.tag(), l.req.lane, ordinal(l.pos), l.reason)
            };
            self.event(if warn { Sev::Warn } else { Sev::Info }, line, Some(&l));
        }
        for (id, reason) in deny {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "denied", Sev::Warn, &reason, now);
            }
        }
        std::mem::take(&mut self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(cores: u32, ram: f64, role: Role) -> Caps {
        Caps {
            node: String::new(),
            arch: "x86_64".into(),
            os: "linux".into(),
            cores,
            ram_gb: ram,
            ram_avail_gb: ram * 0.8,
            disk_free_gb: 500.0,
            disk_total_gb: 1000.0,
            disk_floor_gb: 25.0,
            timed: true,
            role: Some(role),
            gpus: vec![("RTX 4090".into(), 24.0)],
            ..Default::default()
        }
    }

    fn sched(nodes: &[(&str, Caps)]) -> Sched {
        let mut s = Sched::new("mac", 1);
        for (n, c) in nodes {
            s.add_node(n, "10.0.0.1", None);
            s.set_caps(n, c.clone(), 0);
        }
        s
    }

    fn req(lane: &str, kind: Kind, node: Option<&str>) -> Req {
        let mut r = Req::new(lane, kind);
        r.node = node.map(str::to_string);
        r
    }

    fn state(s: &Sched, id: &str) -> (String, String) {
        let l = s.get(id).unwrap();
        let st = match &l.state {
            State::Ended(h) => format!("ended {h}"),
            x => x.as_str().to_string(),
        };
        (st, l.reason.clone())
    }

    fn grants(a: &[Action]) -> Vec<String> {
        a.iter().filter_map(|a| if let Action::Grant { id, .. } = a { Some(id.clone()) } else { None }).collect()
    }

    fn beat_all(s: &mut Sched, now: u64) {
        let ids: Vec<String> = s.leases.iter().map(|l| l.id.clone()).collect();
        for id in ids {
            s.beat(&id, now);
        }
    }

    #[test]
    fn global_cap_waits_never_refuses() {
        let big = caps(256, 1024.0, Role::Pool);
        let mut c = big.clone();
        c.slots = vec![(Kind::Test, 20)];
        let mut s = sched(&[("a", c)]);
        let ids: Vec<String> = (0..10).map(|i| s.request(req(&format!("lane{i}"), Kind::Test, None), 0)).collect();
        let a = s.tick(0);
        assert_eq!(grants(&a).len(), 8);
        assert_eq!(state(&s, &ids[8]), ("queued".into(), "global cap 8/8".into()));
        assert_eq!(s.get(&ids[9]).unwrap().pos, 2);
        s.release(&ids[0], "released", 1000);
        let a = s.tick(1000);
        assert_eq!(grants(&a), vec![ids[8].clone()]);
    }

    #[test]
    fn a_windows_node_takes_only_leases_that_ask_for_it() {
        let mut w = caps(64, 256.0, Role::Pool);
        w.os = "windows".into();
        let mut s = sched(&[("win", w)]);
        let mut x64 = req("lane", Kind::Build, None);
        x64.needs = vec![("arch".into(), "x64".into())];
        let any = s.request(x64, 0);
        s.tick(0);
        assert_eq!(state(&s, &any).0, "ended denied", "{:?}", state(&s, &any));
        assert!(state(&s, &any).1.contains("only leases that name it"));
        let named = s.request(req("lane", Kind::Build, Some("win")), 1);
        let mut by_os = req("lane2", Kind::Build, None);
        by_os.needs = vec![("os".into(), "windows".into())];
        let by_os = s.request(by_os, 1);
        let a = s.tick(1);
        assert!(grants(&a).contains(&named) && grants(&a).contains(&by_os), "{a:?}");
    }

    #[test]
    fn lane_cap_queues_with_a_warning() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool)), ("b", caps(64, 256.0, Role::Pool))]);
        let ids: Vec<String> = (0..3).map(|_| s.request(req("gapx", Kind::Build, None), 0)).collect();
        let a = s.tick(0);
        assert_eq!(grants(&a).len(), 2);
        assert_eq!(state(&s, &ids[2]).1, "lane cap 2/2");
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.line.contains("1st in queue (lane cap 2/2)"))));
        // asking past the cap while at it is a warn
        s.request(req("gapx", Kind::Build, None), 1);
        let a = s.tick(1);
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains("over the lane cap"))));
    }

    #[test]
    fn fair_share_goes_to_the_least_recent_lane() {
        let mut c = caps(16, 64.0, Role::Pool);
        c.slots = vec![(Kind::Test, 1)];
        let mut s = sched(&[("a", c)]);
        let a1 = s.request(req("alpha", Kind::Test, None), 0);
        s.tick(0);
        // alpha asks again before beta: beta has never had one, so it goes first
        let a2 = s.request(req("alpha", Kind::Test, None), 10);
        let b1 = s.request(req("beta", Kind::Test, None), 20);
        beat_all(&mut s, 20);
        s.tick(20);
        assert_eq!(s.get(&b1).unwrap().pos, 1);
        assert_eq!(s.get(&a2).unwrap().pos, 2);
        s.release(&a1, "released", 30);
        let a = s.tick(30);
        assert_eq!(grants(&a), vec![b1]);
    }

    #[test]
    fn aging_beats_priority() {
        let mut c = caps(16, 64.0, Role::Pool);
        c.slots = vec![(Kind::Test, 1)];
        let mut s = sched(&[("a", c)]);
        let first = s.request(req("x", Kind::Test, None), 0);
        s.tick(0);
        let old = s.request(req("old", Kind::Test, None), 0);
        let t = 25 * 60_000; // 2 steps of aging: prio 5 -> 3
        let mut urgent = req("urgent", Kind::Test, None);
        urgent.prio = 4;
        let urgent = s.request(urgent, t);
        let mut hi = req("main", Kind::Test, None);
        hi.prio = 0;
        let hi = s.request(hi, t);
        beat_all(&mut s, t);
        let a = s.tick(t);
        assert_eq!((s.get(&hi).unwrap().pos, s.get(&old).unwrap().pos, s.get(&urgent).unwrap().pos), (1, 2, 3));
        // waited over 10 min: one warn
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains(&old))));
        beat_all(&mut s, t + 1);
        let a = s.tick(t + 1);
        assert!(!a.iter().any(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn)));
        s.release(&first, "released", t + 2);
        assert_eq!(grants(&s.tick(t + 2)), vec![hi]);
    }

    #[test]
    fn timed_holds_the_node() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool))]);
        let b = s.request(req("x", Kind::Build, Some("a")), 0);
        s.tick(0);
        let t = s.request(req("bench", Kind::Timed, Some("a")), 1);
        let late = s.request(req("y", Kind::Test, Some("a")), 2);
        s.tick(2);
        assert_eq!(state(&s, &t), ("queued".into(), "timed: waiting for 1 leases to end".into()));
        assert_eq!(state(&s, &late).1, format!("hold for timed {t}"));
        assert_eq!(s.node_view()[0].hold.as_deref(), Some(t.as_str()));
        s.release(&b, "released", 3);
        assert_eq!(grants(&s.tick(3)), vec![t.clone()]);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(state(&s, &late).1, format!("timed {t} running"));
        s.release(&t, "released", 5);
        assert_eq!(grants(&s.tick(5)), vec![late]);
    }

    #[test]
    fn disk_floor_waits_and_asks_to_reclaim() {
        let mut c = caps(64, 256.0, Role::Pool);
        c.disk_free_gb = 31.0;
        let mut s = sched(&[("a", c.clone())]);
        let b = s.request(req("x", Kind::Build, None), 0);
        let a = s.tick(0);
        assert_eq!(state(&s, &b).1, "disk floor: 31 GB free, needs 20, floor 25");
        assert!(a.iter().any(|a| matches!(a, Action::Reclaim { node } if node == "a")));
        // asked once, not every tick
        assert!(!s.tick(1).iter().any(|a| matches!(a, Action::Reclaim { .. })));
        c.disk_free_gb = 80.0;
        s.set_caps("a", c, 2);
        assert_eq!(grants(&s.tick(2)), vec![b]);
    }

    #[test]
    fn landing_gates_go_first_and_idle_gates_end() {
        let big = caps(256, 1024.0, Role::Pool);
        let nodes: Vec<(String, Caps)> = (0..6).map(|i| (format!("n{i}"), big.clone())).collect();
        let refs: Vec<(&str, Caps)> = nodes.iter().map(|(n, c)| (n.as_str(), c.clone())).collect();
        let mut s = sched(&refs);
        // four lane gates take the global gate cap; a landing and a fifth lane gate wait
        let lanes: Vec<String> = (0..4).map(|i| s.request(req(&format!("lane{i}"), Kind::Gate, None), 0)).collect();
        s.tick(0);
        for id in &lanes {
            assert_eq!(state(&s, id).0, "starting");
        }
        let late = s.request(req("lane9", Kind::Gate, None), 1);
        let land = s.request(req(LANDING_LANE, Kind::Gate, None), 2);
        s.tick(2);
        assert_eq!(state(&s, &land).0, "queued");
        // the lane gates beat but run nothing: after the idle limit they end, and the landing
        // goes first
        for id in lanes.iter().chain([&late, &land]) {
            s.beat(id, GATE_IDLE_MS + 5);
        }
        s.set_info(&lanes[0], r#"{"running":"bash land_gate.sh full"}"#);
        s.tick(GATE_IDLE_MS + 5);
        assert_eq!(state(&s, &lanes[0]).0, "starting", "a gate that runs something stays");
        for id in &lanes[1..] {
            assert_eq!(state(&s, id).0, "ended idle");
        }
        assert_eq!(state(&s, &land).0, "starting", "the landing takes a freed gate slot first");
    }

    /// `--needs gui`: a test lease that opens windows goes only to nodes that present (screen
    /// slots above 0); 106 at slots.screen=0 takes none, plain test leases still go there.
    #[test]
    fn gui_tests_avoid_nodes_without_screen_slots() {
        let mut dark = caps(16, 64.0, Role::Pool);
        dark.slots = vec![(Kind::Build, 2), (Kind::Test, 2), (Kind::Screen, 0), (Kind::Long, 2), (Kind::Gate, 1)];
        let mut s = sched(&[("106", dark.clone())]);
        let mut gui = req("x", Kind::Test, None);
        gui.needs = parse_needs("gui=1");
        let gui = s.request(gui, 0);
        let plain = s.request(req("y", Kind::Test, None), 0);
        s.tick(0);
        assert_eq!(state(&s, &gui).0, "ended denied");
        assert_eq!(state(&s, &plain).0, "starting");
        let lit = caps(16, 64.0, Role::Pool);
        assert!(meets(&lit, "gui", "1") && meets(&lit, "gui", "") && !meets(&dark, "gui", "yes") && meets(&dark, "gui", "0"));
    }

    #[test]
    fn deny_what_never_fits() {
        let mut s = sched(&[("a", caps(8, 32.0, Role::Pool)), ("wm", caps(32, 64.0, Role::UserWm))]);
        // the user switched this WM's dev off (every node defaults on)
        s.set_dev("wm", false, 0).unwrap();
        let g = s.request(req("x", Kind::Gate, Some("a")), 0); // 8 cores fits exactly
        let mut big = req("x", Kind::Build, None);
        big.needs = parse_needs("gpu=5090");
        let big = s.request(big, 0);
        let user = s.request(req("y", Kind::Build, Some("wm")), 0);
        let insp = {
            let mut r = req("y", Kind::Inspect, Some("wm"));
            r.jit = Some("sandbox".into());
            s.request(r, 0)
        };
        s.tick(0);
        assert_eq!(state(&s, &g).0, "starting");
        assert_eq!(state(&s, &big).0, "ended denied");
        assert_eq!(state(&s, &user).0, "ended denied");
        assert!(s.recent_ended(5).iter().any(|l| l.id == user));
        // inspect is always allowed on a user WM
        assert_eq!(state(&s, &insp).0, "starting");
        // the user switches it on: writes go
        s.set_dev("wm", true, 1).unwrap();
        let w = s.request(req("y", Kind::Build, Some("wm")), 1);
        assert_eq!(grants(&s.tick(1)), vec![w]);
    }

    #[test]
    fn heartbeat_expiry_ends_silent_holders() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool))]);
        let b = s.request(req("x", Kind::Build, None), 0);
        let q = s.request(req("x", Kind::Build, None), 0);
        s.tick(0);
        s.started(&b, "{\"ok\":1}");
        s.beat(&b, 20_000);
        s.beat(&q, 20_000);
        assert!(s.tick(30_000).iter().all(|a| !matches!(a, Action::End { .. })));
        s.beat(&q, 25_000);
        let a = s.tick(30_001);
        assert!(a.iter().any(|a| matches!(a, Action::End { id, node: Some(n), how } if *id == b && n == "a" && how == "expired")));
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains("expired"))));
        assert_eq!(state(&s, &q).0, "starting");
        // a holder pid that is gone ends its lease at once
        s.leases.iter_mut().find(|l| l.id == q).unwrap().req.holder = Some(4242);
        assert_eq!(s.holders_gone(&|p| p != 4242), vec![q.clone()]);
        assert!(s.release(&q, "holder-dead", 30_500));
        assert_eq!(s.get(&q).map(|l| l.state.clone()), Some(State::Ended("holder-dead".into())));
    }

    #[test]
    fn dev_off_drains() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool))]);
        let b = s.request(req("x", Kind::Build, Some("a")), 0);
        s.tick(0);
        s.set_dev("a", false, 1).unwrap();
        let a = s.tick(1);
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.line == "node a dev off: draining (1 leases)")));
        assert!(!a.iter().any(|a| matches!(a, Action::End { .. })));
        assert!(s.node_view()[0].draining && s.node_view()[0].state_word() == "draining (1 leases)");
        let w = s.request(req("y", Kind::Build, Some("a")), 2);
        beat_all(&mut s, 2);
        s.tick(2);
        // a pool node switched off: waits, not denied
        assert_eq!(state(&s, &w), ("queued".into(), "node off".into()));
        s.set_dev("a", true, 3).unwrap();
        assert_eq!(grants(&s.tick(3)), vec![w]);
        let _ = b;
    }

    #[test]
    fn dev_defaults_on_and_sticky_node_is_preferred() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool)), ("b", caps(64, 256.0, Role::Pool)), ("wm", caps(64, 256.0, Role::UserWm))]);
        assert!(s.dev("wm") && s.dev("a"));
        s.set_dev("wm", false, 0).unwrap();
        assert!(!s.dev("wm"));
        let first = s.request(req("x", Kind::Build, None), 0);
        s.tick(0);
        let node = s.get(&first).unwrap().node.clone().unwrap();
        assert_ne!(node, "wm");
        s.release(&first, "released", 1);
        let again = s.request(req("x", Kind::Build, None), 2);
        s.tick(2);
        let l = s.get(&again).unwrap();
        assert_eq!(l.node.as_deref(), Some(node.as_str()));
        assert_eq!(l.cost.disk_gb, 2.0);
    }

    #[test]
    fn cancel_ends_one_lease_and_keeps_the_lane_off_that_node() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool)), ("b", caps(64, 256.0, Role::Pool))]);
        let x = s.request(req("lane-x", Kind::Build, Some("a")), 0);
        let y = s.request(req("lane-y", Kind::Build, Some("a")), 0);
        s.tick(1);
        assert_eq!(s.get(&x).unwrap().node.as_deref(), Some("a"));
        assert_eq!(s.get(&y).unwrap().node.as_deref(), Some("a"));
        // cancel x: y keeps running on the same node
        assert!(s.cancel(&x, 10));
        let acts = s.tick(11);
        assert!(acts.iter().any(|a| matches!(a, Action::End { id, how, .. } if *id == x && how == "cancelled")), "{acts:?}");
        assert!(!acts.iter().any(|a| matches!(a, Action::End { id, .. } if *id == y)));
        assert!(s.get(&y).unwrap().state.live());
        assert!(!s.cancel(&x, 12), "already ended");
        // lane-x asks again with no node: it is placed, but not on a
        let again = s.request(req("lane-x", Kind::Build, None), 20);
        s.tick(21);
        assert_eq!(s.get(&again).unwrap().node.as_deref(), Some("b"));
        // naming the node overrides the exclusion
        let named = s.request(req("lane-x", Kind::Build, Some("a")), 22);
        s.tick(23);
        assert_eq!(s.get(&named).unwrap().node.as_deref(), Some("a"));
        // the exclusion lapses
        s.release(&again, "released", 30);
        s.release(&named, "released", 30);
        let later = s.request(req("lane-x", Kind::Build, None), CANCEL_EXCLUDE_MS + 100);
        s.tick(CANCEL_EXCLUDE_MS + 101);
        assert!(s.get(&later).unwrap().state.live());
    }

    #[test]
    fn requests_round_trip() {
        let mut r = req("gapx", Kind::Build, Some("217"));
        r.needs = parse_needs("arch=x64,ram=32G");
        r.prio = 0;
        r.rev = Some("work,main".into());
        assert_eq!(Req::parse(&r.to_json()).unwrap(), r);
        assert!(Req::parse("{\"lane\":\"x\",\"kind\":\"inspect\"}").is_err());
        assert_eq!(ordinal(3), "3rd");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(22), "22nd");
    }

    #[test]
    fn a_restarted_master_takes_back_its_granted_leases() {
        let mut s = sched(&[("217", caps(16, 60.0, Role::Pool))]);
        let id = s.request(req("lane-a", Kind::Build, Some("217")), 0);
        assert_eq!(grants(&s.tick(1)), vec![id.clone()]);
        s.started(&id, "{\"work\":\"/w\"}");
        let live = s.live_lines();
        assert_eq!(live.len(), 1);
        let mut t = sched(&[("217", caps(16, 60.0, Role::Pool))]);
        assert_eq!(t.restore(&live[0], 100), Some(id.clone()));
        assert_eq!(state(&t, &id).0, "granted");
        assert_eq!(t.get(&id).unwrap().start, "{\"work\":\"/w\"}");
        let next = t.request(req("lane-a", Kind::Test, Some("217")), 100);
        assert_ne!(next, id, "ids continue after the restored ones");
        assert_eq!(t.live_on("217"), vec![id]);
    }
}
