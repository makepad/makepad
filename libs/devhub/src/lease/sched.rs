//! The lease scheduler: who gets which node, and why the rest wait (DESIGN.md §10.2, §10.8).
//!
//! Pure: no IO and no clock of its own (every call takes `now` in ms). Mutators queue
//! [`Action`]s; [`Sched::tick`] places what can start and hands them all to the master, which
//! talks to the nodes and publishes the events.
//!
//! Pinned native lane leases on work boxes hold per-lane trees: only that lane's different pin
//! conflicts. CI, gates and user boxes keep the shared tree and its existing exclusions.
//! Native capacity waits for a free tree; the executor evicts the least recently used one.
//! Overlays never hold a tree; one lane
//! serializes its different overlay pins (CI overlays are keyed per pin). Timed and whole-box leases and their queued holds exclude
//! writers and overlays. Slots select kinds (0: never); cores, 85% of RAM and GPUs are
//! summed across running leases, with disk kept above its floor. Hot boxes take small work
//! first, then medium tests/screens; large work may use them after 10 min waiting. Long-cycle boxes (role `long`) take no CI.
//!
//! Timed work drains oldest first in quiet windows, yielding queued holds to interactive
//! demand. Otherwise: gates, interactive, then numeric priority (aged by one
//! per 10 min waiting), then fair share (the lane with
//! the oldest last grant), then request time. A lease past its lane's cap ([`LANE_CAP`]) goes
//! after every lease within its cap and only where none of them waits: the cap orders, it never
//! keeps a box idle. A lease waits (never refused) for a free node, the disk floor, pressure, the
//! dev switch or an exclusive hold; it is denied only when no node could ever take it.
//! CI runs only on boxes whose lease.conf opts in (`slots.ci = 1`) and queues behind gates and
//! lane work, regardless of aging.
//!
//! Placement: a box holding the lease's dev JIT beats one that would build it (the master
//! computes the ids from the pin); among boxes that would build it the lower finish ETA wins;
//! otherwise small jobs take hot boxes, then ready pool capacity. A ready box never waits for
//! an ETA. Exclusive and interactive work may reserve one box for ten minutes (a timed request:
//! for as long as it waits), only while running leases are its sole obstacle.
//! A reservation never blocks work that fits beside its owner or is expected to end before its
//! owner could start (backfill). A box at [`WARM_BUSY`] (load, or granted cores or RAM) takes
//! work only when every box is that busy (the least busy first); below it, bonuses favor a tree
//! already at a tree lease's commits, the lane's warm target and its last node, then the less
//! busy and longest-idle box, so warmth never makes a hotspot. Timed leases run on the bench
//! boxes (role `bench`, by name) unless their needs fit only elsewhere. `--node N` on a build, test, long, gate or
//! gpu lease is a preference: N when it can take it; when N cannot for [`PIN_GRACE_MS`] (at once
//! when N is held for a timed lease, busy, off or down) a twin (same arch and os) takes it.
//! `--only N`, timed, screen and inspect leases run on N or wait for it.
//! The Macs (os macos) leave the general pool: a lease lands there only with `--needs os=macos`
//! (inspect leases look at a JIT already there); a lease naming a Mac without it is refused.

use super::{Caps, Cost, CostClass, Kind, Pin, Priority, Role, Src};
use crate::proto::{hub_record, Record, Sev};
use makepad_remote::json_str;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// No beat from the holder for this long: the lease expires (holders beat every 2 s; a holder
/// on the master's machine is also watched by its pid, which ends its lease within a second).
pub const BEAT_EXPIRY_MS: u64 = 10_000;
/// How long a lease a restarted master took back waits for its holder's first beat.
pub const RESTORE_GRACE_MS: u64 = 60_000;
/// A waiting lease's priority improves by one per this much waiting (starvation guard).
pub const AGE_STEP_MS: u64 = 600_000;
/// A node whose caps go unanswered this long counts as down (polls are every 10 s).
pub const DOWN_AFTER_MS: u64 = 30_000;
/// Verify's correctness legs that are owed a box: one under a lane's default.
pub fn owed_verify(l: &Lease) -> bool {
    !l.req.ci() && l.req.prio == DEFAULT_PRIO - 1 && l.req.agent.as_deref() == Some("devhub-verify")
}

/// A wait this long becomes a warn (once).
pub const WAIT_WARN_MS: u64 = 600_000;
pub const DEFAULT_PRIO: i32 = 5;
/// A soft-pinned lease (`--node N`) waits this long for N before a twin node may take it.
pub const PIN_GRACE_MS: u64 = 120_000;
/// The priority of a bumped request (`devhub job bump`): the front of its queue class.
pub const BUMP_PRIO: i32 = i32::MIN / 2;
/// A queued request gets one reservation window; changing boxes never renews it.
pub const HOLD_MS: u64 = 600_000;
/// Warm live jobs give an exclusive request its box after its current calls finish.
pub const LIVE_PARK_MS: u64 = 60_000;
/// A timed request queued this long stops yielding to interactive demand: it reserves the next
/// box that frees and gets its quiet window. With many live sessions a bench box never went
/// quiet by itself (a timed lease waited past 10 min "held for interactive", 11-03).
pub const TIMED_DUE_MS: u64 = 600_000;
/// The cores an idle live session is charged ([`Sched::charged`]).
pub const LIVE_IDLE_CORES: f64 = 1.0;

/// A stall is also this far over its measure ([`Sched::say_stalled`]).
const STALL_SLACK_MS: u64 = 300_000;
/// A lane whose timed lease ended this recently may be between runs of one series: no merges.
pub const TIMED_SERIES_MS: u64 = 15 * 60_000;
/// Placement score bonus of the lane's warm node (its target there: no cold build) and of the
/// lane's last node.
const WARM_BONUS: f64 = 1.5;
const STICKY_BONUS: f64 = 0.5;
/// A box this busy (its load, or the cores or RAM granted on it, of what it has) loses the lane's
/// warm, sticky and tree preference: the lease goes to the least busy box that fits, which then
/// warms for the lane. Warmth only breaks ties between boxes with room; it never makes a hotspot.
const WARM_BUSY: f64 = 0.75;
/// a lease's wait for a box that holds its dev JIT ([`Sched::jit_box`])
pub(super) const JIT_WAIT: &str = "holds its dev JIT";
/// Placement score bonus of a node whose tree already holds the lease's commits.
const TREE_BONUS: f64 = 2.0;
/// Running leases per lane and kind within the lane's cap. Past it a lease is work-conserving
/// overflow: placed after every lease within its cap, on a box none of those waits for, with no
/// reservation, and it is the first to give way (no new grant while one within its cap waits
/// there; never ended for it). A hard cap queued 61 min of leases an hour while boxes idled
/// (five 12-min overlay tests at "lane cap 2/2" beside three near-idle boxes, 11-02).
pub const LANE_CAP: u32 = 2;
/// CI's lane has no such cap; its shards go only to boxes with CI slots ([`slots`]).
pub const CI_LANE: &str = "ci";
/// The hot bucket's lane: the highest lease class. It goes ahead of every queued lease, waits only
/// for running leases to fit, and is never held off by a reservation for another lease.
pub const TOP_LANE: &str = "landq-hot";
/// RAM (GB) of a hot box that only the top lane's leases may take: another lease never fills the
/// budget to its last [`TOP_RAM_RESERVE_GB`], so a post-land boot finds room at once instead of
/// waiting for a screen lease or a live test to end. Running leases are never preempted.
pub const TOP_RAM_RESERVE_GB: f64 = 5.0;
/// A granted gate lease that runs nothing this long ends: an idle gate slot blocks landings.
pub const GATE_IDLE_MS: u64 = 600_000;
/// The lane whose gates are landings: first in line for the global gate cap.
pub const LANDING_LANE: &str = "landgate";
/// A node's hub is asked to reclaim disk at most this often.
const RECLAIM_EVERY_MS: u64 = 300_000;
/// Ended leases kept for late beats and the view.
const KEEP_ENDED: usize = 200;

/// Whether a node takes `kind` at all (> 0): its lease.conf `slots.<kind>` (CI only where it opts
/// in, [`super::caps::LeaseConf::slots`]); a hub that reports none, one per kind (gpu: when it
/// has one). Resources, not slot counts, limit overlays.
pub fn slots(c: &Caps, kind: Kind) -> u32 {
    if kind == Kind::Ci && c.role().users() {
        return 0;
    }
    if !c.slots.is_empty() {
        return c.slots(kind);
    }
    match kind {
        Kind::Gpu => c.gpus.len().min(1) as u32,
        Kind::Inspect => u32::MAX,
        _ => 1,
    }
}

/// Builds keep a target dir on the node: the lane's next build goes there warm.
fn buildish(kind: Kind) -> bool {
    matches!(kind, Kind::Build | Kind::Test | Kind::Gate)
}

/// A lease request (the `lease` service's JSON line). `cores` and `ram_gb` override the
/// kind's default reservation; `whole` excludes other write leases and holds the node while
/// waiting, without timed CPU pinning or a quiet mark.
#[derive(Clone, Debug, PartialEq)]
pub struct Req {
    pub lane: String,
    /// A warm lease uses this lane's native tree and target, but keeps its own accounting lane.
    pub warms: Option<String>,
    pub kind: Kind,
    pub src: Src,
    /// Live jobs need a hub that accepts the detachable flag on lease-run.
    pub live: bool,
    pub cost: CostClass,
    pub cost_asked: bool,
    pub est_min: Option<f64>,
    pub cores: Option<f64>,
    pub ram_gb: Option<f64>,
    pub whole: bool,
    pub class: String,
    /// this node: a preference for build/test/long/gate/gpu leases (a twin may take it, see
    /// [`PIN_GRACE_MS`]), exactly this node for the other kinds or with `only`
    pub node: Option<String>,
    /// `--only N`: exactly `node`, never a twin
    pub only: bool,
    /// `arch os gpu ram disk cores display role` requirements
    pub needs: Vec<(String, String)>,
    pub prio: i32,
    pub priority: Priority,
    /// `<makepad branch|sha>[,<commercial>]`: pinned tree or store revisions (required for overlays)
    pub rev: Option<String>,
    /// the lane's repo on the master (rev lookup, recovered work)
    pub repo_dir: Option<String>,
    /// Tree inspect leases: the existing JIT pid or app.
    pub jit: Option<String>,
    /// Runnable leases: the explicit loader content id from --jit.
    pub loader: Option<String>,
    /// Overlay leases: the dev JIT ids their pin selects (one per compiler the nodes report),
    /// set by the master at intake; placement only, never sent to a node.
    pub hint: Vec<String>,
    /// the holder process on the master's machine (the CLI that asked): its end ends the lease
    pub holder: Option<u32>,
    /// asked by the master's own work (orch::leg::Held), which asks again after a restart
    pub master: bool,
    /// when the holder first asked: its place in the queue, also when it asks again (a restarted
    /// master knows only granted leases)
    pub since_ms: Option<u64>,
    /// the agent behind the lane (`DEVHUB_AGENT`): what the panel shows and `evict` can name
    pub agent: Option<String>,
}

impl Req {
    pub fn tree_lane(&self) -> &str { self.warms.as_deref().unwrap_or(&self.lane) }

    pub fn on_node(&self) -> bool { self.kind != Kind::Inspect || self.src == Src::Overlay }

    /// Sweeps use build/test kinds under the CI lane; explicit CI leases may name another lane.
    fn ci(&self) -> bool { self.kind == Kind::Ci || self.lane == CI_LANE }

    fn top(&self) -> bool { self.lane == TOP_LANE }

    /// The RAM budget (GB) of a node for this request: 85% of its RAM; on a hot box the top
    /// lane's leases get all of it and every other lease all but [`TOP_RAM_RESERVE_GB`].
    fn ram_budget(&self, c: &Caps) -> f64 {
        let all = c.ram_gb * 0.85;
        if self.top() || c.role() != Role::Hot { all } else { (all - TOP_RAM_RESERVE_GB).max(0.0) }
    }

    fn exclusive(&self) -> bool { self.kind == Kind::Timed || self.whole }

    /// Says it needs macOS (`--needs os=macos`): the only work the Macs take.
    pub fn requires_macos(&self) -> bool {
        self.needs.iter().any(|(k, v)| k == "os" && v.eq_ignore_ascii_case("macos"))
    }

    /// Shows windows on the node's WM: a screen lease (visible live apps among them) or one that
    /// needs a display or gui. A box whose WM speaks another hosted wire takes none of these.
    fn uses_wm(&self) -> bool {
        self.kind == Kind::Screen || self.needs.iter().any(|(k, v)| matches!(k.as_str(), "gui" | "display") && !matches!(v.to_ascii_lowercase().as_str(), "0" | "no" | "off" | "false"))
    }

    fn allocation(&self, warm: bool) -> Cost {
        let mut cost = Cost::default_for(self.kind, warm);
        if let Some(cores) = self.cores { cost.cores = cores; }
        if let Some(ram_gb) = self.ram_gb { cost.ram_gb = ram_gb; }
        cost
    }

    pub fn run_options(&mut self, src: Src, cost: Option<CostClass>, est_min: Option<f64>, class: &str, argv: &[String]) {
        self.src = if self.kind == Kind::Ci { Src::Tree } else { src };
        self.cost = cost.unwrap_or(self.cost);
        self.cost_asked |= cost.is_some();
        self.est_min = est_min;
        self.class = if class.is_empty() { super::run_class(self.kind, argv) } else { class.to_string() };
    }

    fn class(&self) -> &str { if self.class.is_empty() { self.kind.as_str() } else { &self.class } }

    pub fn asked_cost(&self) -> CostClass {
        match self.est_min {
            Some(m) => {
                let class = CostClass::from_wall((m * 60_000.0) as u64, self.allocation(false).cores);
                if self.cost_asked { self.cost.max(class) } else { class }
            }
            None => self.cost,
        }
    }

    pub fn conflicts(&self, other: &Req) -> bool {
        self.conflicts_on(other, Role::Pool)
    }

    fn native(&self, role: Role) -> bool {
        self.uses_tree() && super::native_lane(self.tree_lane(), self.kind, role)
    }

    /// The oldest hub protocol that can run this request: a hub that has not taken its update yet
    /// keeps taking the work it already knows (1: ship-done; 2: overlay sources; 3: per-lane
    /// native trees; 4: detachable live runs;
    /// 5: compiled-file dev JIT inputs, a dep-info mismatch never fails (every overlay)),
    /// so a protocol bump never idles the fleet.
    fn proto_needed(&self, role: Role) -> u32 {
        match self.src {
            _ if self.warms.is_some() => 7,
            Src::Overlay => 5,
            _ if self.live => 4,
            Src::Tree if self.native(role) => 3,
            Src::Tree => 1,
        }
    }

    fn conflicts_on(&self, other: &Req, role: Role) -> bool {
        let native = self.uses_tree() && other.uses_tree() && (self.native(role) || other.native(role));
        let kinds = if native { self.kind == Kind::Timed || other.kind == Kind::Timed } else { self.kind.conflicts(self.src, other.kind, other.src) };
        kinds || (self.whole && other.kind.writes()) || (other.whole && self.kind.writes()) || self.tree_conflicts(other, role) || (self.src == Src::Overlay && other.src == Src::Overlay && self.lane == other.lane && !(self.kind == Kind::Ci && other.kind == Kind::Ci) && self.rev.as_deref().and_then(Pin::of_rev) != other.rev.as_deref().and_then(Pin::of_rev))
    }

    fn uses_tree(&self) -> bool {
        self.src == Src::Tree && self.kind.writes() && (self.kind == Kind::Ci || self.rev.is_some())
    }

    fn same_tree(&self, other: &Req, role: Role) -> bool {
        if !self.uses_tree() || !other.uses_tree() {
            return false;
        }
        match (self.native(role), other.native(role)) {
            (true, true) => self.tree_lane() == other.tree_lane(),
            (false, false) => true,
            _ => false,
        }
    }

    /// Holders of the same tree must agree on a known pin. Different native lanes are independent.
    fn tree_conflicts(&self, other: &Req, role: Role) -> bool {
        if !self.same_tree(other, role) { return false; }
        match (self.rev.as_deref().and_then(Pin::of_rev), other.rev.as_deref().and_then(Pin::of_rev)) {
            (Some(a), Some(b)) if self.native(role) => a != b,
            (Some(a), Some(b)) => !a.fits(&b),
            _ => true,
        }
    }

    pub fn new(lane: &str, kind: Kind) -> Req {
        Req { lane: lane.to_string(), warms: None, kind, src: Src::Tree, live: false, cost: CostClass::default_for(kind), cost_asked: false, est_min: None, cores: None, ram_gb: None, whole: false, class: String::new(), node: None, only: false, needs: Vec::new(), since_ms: None, prio: DEFAULT_PRIO, priority: Priority::Background, rev: None, repo_dir: None, jit: None, loader: None, hint: Vec::new(), holder: None, master: false, agent: None }
    }

    pub fn to_json(&self) -> String {
        let opt = |o: &Option<String>| o.as_deref().map_or("null".to_string(), json_str);
        let needs: Vec<String> = self.needs.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        let warms = self.warms.as_deref().map_or(String::new(), |lane| format!(",\"warms\":{}", json_str(lane)));
        format!(
            "{{\"lane\":{}{warms},\"kind\":\"{}\",\"src\":\"{}\",\"live\":{},\"cost\":\"{}\",\"cost_asked\":{},\"est_min\":{},\"cores\":{},\"ram_gb\":{},\"whole\":{},\"class\":{},\"node\":{},\"only\":{},\"needs\":{{{}}},\"prio\":{},\"priority\":\"{}\",\"rev\":{},\"repo_dir\":{},\"jit\":{},\"loader\":{},\"hint\":[{}],\"holder\":{},\"master\":{},\"agent\":{},\"since_ms\":{}}}",
            json_str(&self.lane),
            self.kind.as_str(),
            self.src.as_str(),
            self.live,
            self.cost.as_str(),
            self.cost_asked,
            self.est_min.map_or("null".to_string(), |m| m.to_string()),
            self.cores.map_or("null".to_string(), |n| n.to_string()),
            self.ram_gb.map_or("null".to_string(), |n| n.to_string()),
            self.whole,
            json_str(&self.class),
            opt(&self.node),
            self.only as u8,
            needs.join(","),
            self.prio,
            self.priority.as_str(),
            opt(&self.rev),
            opt(&self.repo_dir),
            opt(&self.jit),
            opt(&self.loader),
            self.hint.iter().map(|id| json_str(id)).collect::<Vec<_>>().join(","),
            self.holder.map_or("null".to_string(), |p| p.to_string()),
            self.master,
            opt(&self.agent),
            self.since_ms.map_or("null".to_string(), |t| t.to_string())
        )
    }

    pub fn parse(text: &str) -> Result<Req, String> {
        let v = makepad_strict_json::parse(text.as_bytes()).map_err(|e| format!("bad lease request: {e:?}"))?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
        let lane = s("lane").filter(|l| makepad_devhub_conf::valid_name(l)).ok_or("a lease needs a lane (a-z 0-9 - _)")?;
        let kind = s("kind").and_then(|k| Kind::parse(&k)).ok_or("unknown lease kind")?;
        let mut r = Req::new(&lane, kind);
        r.warms = v.get("warms").filter(|v| !v.is_null()).map(|v| v.as_str().filter(|l| makepad_devhub_conf::valid_name(l)).map(str::to_string).ok_or("warms needs a lane (a-z 0-9 - _)")).transpose()?;
        r.src = s("src").map(|s| Src::parse(&s).ok_or("src is tree or overlay")).transpose()?.unwrap_or(Src::Tree);
        r.live = v.get("live").and_then(|v| v.as_bool()).unwrap_or(false);
        r.cost = s("cost").map(|s| CostClass::parse(&s).ok_or("cost is small, medium or large")).transpose()?.unwrap_or(r.cost);
        r.cost_asked = v.get("cost_asked").and_then(|v| v.as_bool()).unwrap_or_else(|| s("cost").is_some());
        r.est_min = v.get("est_min").filter(|v| !v.is_null()).map(|v| super::num(v).filter(|m| m.is_finite() && *m >= 0.0).ok_or("est_min is a nonnegative number")).transpose()?;
        r.cores = v.get("cores").filter(|v| !v.is_null()).map(|v| super::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("cores is a positive number")).transpose()?;
        r.ram_gb = v.get("ram_gb").filter(|v| !v.is_null()).map(|v| super::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("ram_gb is a positive number")).transpose()?;
        r.whole = v.get("whole").and_then(|v| v.as_bool()).unwrap_or(false);
        r.class = s("class").unwrap_or_default();
        r.priority = s("priority").map(|s| Priority::parse(&s).ok_or("priority is interactive or background")).transpose()?.unwrap_or_default();
        r.node = s("node");
        r.only = r.node.is_some() && v.get("only").and_then(super::num).is_some_and(|x| x != 0.0);
        r.prio = v.get("prio").and_then(super::num).map_or(DEFAULT_PRIO, |p| p as i32);
        r.rev = s("rev");
        r.repo_dir = s("repo_dir");
        r.jit = s("jit");
        r.loader = s("loader");
        r.hint = v.get("hint").and_then(|v| v.as_arr()).unwrap_or(&[]).iter().filter_map(|v| v.as_str()).map(str::to_string).collect();
        r.agent = s("agent").filter(|a| makepad_devhub_conf::valid_name(a));
        r.holder = v.get("holder").and_then(super::num).map(|p| p as u32).filter(|p| *p > 1);
        r.master = v.get("master").and_then(|v| v.as_bool()).unwrap_or(false);
        r.since_ms = v.get("since_ms").and_then(super::num).map(|t| t as u64);
        if let Some(makepad_strict_json::Value::Obj(pairs)) = v.get("needs") {
            for (k, x) in pairs {
                r.needs.push((k.clone(), x.as_str().map(str::to_string).or_else(|| super::num(x).map(|n| n.to_string())).unwrap_or_default()));
            }
        }
        if r.src == Src::Overlay && r.rev.as_deref().and_then(Pin::of_rev).is_none() {
            return Err("overlay leases require pinned revisions".into());
        }
        if kind == Kind::Inspect && r.src == Src::Tree && (r.node.is_none() || r.jit.is_none()) {
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

/// The evidence used at placement; retained with the grant.
#[derive(Clone, Copy, Debug)]
pub enum CostSource { Measured(u64), Asked, Kind }

impl CostSource {
    fn fallback(req: &Req) -> Self {
        if req.cost_asked || req.est_min.is_some() { Self::Asked } else { Self::Kind }
    }

    fn label(self) -> String {
        match self { Self::Measured(ms) => format!("measured {}", dur(ms).replace(' ', "")), Self::Asked => "asked".into(), Self::Kind => "kind".into() }
    }

    fn ms(self) -> String {
        match self { Self::Measured(ms) => ms.to_string(), _ => "null".into() }
    }
}

#[derive(Clone, Debug)]
pub struct Lease {
    pub id: String,
    pub req: Req,
    pub state: State,
    pub node: Option<String>,
    pub cost: Cost,
    pub cost_class: CostClass,
    pub cost_source: CostSource,
    class_notified: bool,
    /// Effective priority, frozen when granted.
    pub priority: Priority,
    pub asked_ms: u64,
    pub granted_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    pub beat_ms: u64,
    /// 1-based among the waiting leases of its kind
    pub pos: usize,
    pub reason: String,
    /// an exclusive request's expected wait for its box (its running work, then the exclusive
    /// requests ahead of it there); 0: none known
    pub eta_ms: u64,
    warned: bool,
    /// the node's `lease-start` answer (work, target, env)
    pub start: String,
    /// what the node says it runs, its work dir and disk use (`lease-beat` info)
    pub info: String,
    /// since when a granted gate lease runs nothing (`info`'s `running` empty)
    idle_since: Option<u64>,
    hold_until: Option<u64>,
    /// since when it first waited for a busy box that holds its dev JIT while a box without it
    /// could take it: all such waiting together stays under one build ([`Sched::jit_box`])
    jit_wait_since: Option<u64>,
    seq: u64,
    /// latency (ms): from the previous lease's release on its node to this grant, when it waited for
    /// that box; its setup (`lease-start`, the revisions shipped inside it); its teardown (`lease-end`)
    pub wait_ms: Option<u64>,
    pub setup_ms: Option<u64>,
    pub ship_ms: Option<u64>,
    pub teardown_ms: Option<u64>,
    pub phases: super::prof::Phases,
    pub node_phases: super::prof::Phases,
    pub reported_ms: Option<u64>,
    queue_sample_ms: u64,
    queue_boxes: Vec<String>,
}

impl Lease {
    fn class_note(&self) -> String {
        if matches!(self.cost_source, CostSource::Measured(_)) && self.cost_class != self.req.asked_cost() {
            format!("class {} {}: {}", self.req.class(), self.cost_source.label(), self.cost_class.as_str())
        } else { String::new() }
    }

    fn take_class_note(&mut self) -> String {
        if self.class_notified { return String::new(); }
        let note = self.class_note();
        self.class_notified = !note.is_empty();
        if note.is_empty() { note } else { format!("; {note}") }
    }

    /// Waits on its own lane's cap: no free box would serve it now.
    pub fn lane_capped(&self) -> bool {
        self.state == State::Queued && self.reason.starts_with("lane cap")
    }

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
            "{{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"class_note\":{},\"src\":\"{}\",\"class\":{},\"state\":\"{}\",\"node\":{},\"addr\":{},\"pos\":{},\"reason\":{},\"eta_ms\":{},\"prio\":{},\"priority\":\"{}\",\"wait_s\":{},\"how\":{},\"jit\":{},\"loader\":{},\"agent\":{},\"beat_s\":{},\"granted_s\":{},\"info\":{},\"start\":{}}}",
            json_str(&self.id),
            json_str(&self.req.lane),
            self.req.kind.as_str(),
            self.cost_class.as_str(),
            json_str(&self.cost_source.label()),
            self.cost_source.ms(),
            json_str(&self.class_note()),
            self.req.src.as_str(),
            json_str(&self.req.class),
            self.state.as_str(),
            opt(&self.node),
            json_str(addr),
            self.pos,
            json_str(&self.reason),
            self.eta_ms,
            self.req.prio,
            self.priority.as_str(),
            (self.granted_ms.unwrap_or(now).saturating_sub(self.asked_ms)) / 1000,
            how,
            opt(&self.req.jit),
            opt(&self.req.loader),
            opt(&self.req.agent),
            now.saturating_sub(self.beat_ms) / 1000,
            self.granted_ms.map_or("null".to_string(), |g| (now.saturating_sub(g) / 1000).to_string()),
            if self.info.starts_with('{') { self.info.as_str() } else { "null" },
            if self.start.starts_with('{') { self.start.as_str() } else { "null" }
        )
    }

    fn sample_queue(&mut self, now: u64) {
        if self.state == State::Queued && now > self.queue_sample_ms {
            self.phases.sample("queued", &super::prof::blocker(&self.reason), &self.queue_boxes, self.queue_sample_ms, now);
            self.queue_sample_ms = now;
        }
    }

    pub fn profile(&self) -> super::prof::Phases {
        let mut phases = self.phases.clone();
        if self.state == State::Starting {
            if let Some(start) = self.granted_ms { phases.push("sync", start, 0); }
        }
        phases.0.extend(self.node_phases.0.iter().cloned());
        if let Some(end) = self.ended_ms {
            for phase in ["sync", "jit", "run"] { phases.close(phase, end); }
            if self.reported_ms.is_none() && self.node.is_some() { phases.push("report", end, 0); }
        }
        phases
    }

    /// The ledger line.
    pub fn ledger_line(&self) -> String {
        let mut req = self.req.clone();
        req.priority = self.priority;
        if req.class.is_empty() {
            if let Ok(v) = makepad_strict_json::parse(self.info.as_bytes()) {
                req.class = v.get("class").and_then(|v| v.as_str()).unwrap_or("").to_string();
            }
        }
        let how = match &self.state {
            State::Ended(h) => h.as_str(),
            _ => "",
        };
        let ms = |o: Option<u64>| o.map_or("null".to_string(), |v| v.to_string());
        format!(
            "{{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"node\":{},\"asked_ms\":{},\"granted_ms\":{},\"ended_ms\":{},\"how\":{},\"wait_ms\":{},\"setup_ms\":{},\"ship_ms\":{},\"teardown_ms\":{},\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"reported_ms\":{}, {},\"req\":{}}}",
            json_str(&self.id),
            json_str(&self.req.lane),
            self.req.kind.as_str(),
            self.node.as_deref().map_or("null".to_string(), json_str),
            self.asked_ms,
            ms(self.granted_ms),
            ms(self.ended_ms),
            json_str(how),
            ms(self.wait_ms),
            ms(self.setup_ms),
            ms(self.ship_ms),
            ms(self.teardown_ms),
            self.cost_class.as_str(),
            json_str(&self.cost_source.label()),
            self.cost_source.ms(),
            ms(self.reported_ms),
            self.profile().fields(self.reported_ms.or(self.ended_ms).unwrap_or(self.beat_ms)),
            req.to_json()
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
    /// a lease waits for `need_gb` over the node's disk floor: reclaim that (`disk`)
    Reclaim { node: String, need_gb: f64 },
    /// Stop admitting live calls, then release these leases when their calls have answered.
    ParkLive { leases: Vec<String>, note: String },
    /// a stalled lease's JITs capture their state on its node (`lease-jits`, then each one's
    /// `capture why=..`): the cause and the report's path go to the inbox
    Capture { id: String, node: String, why: String },
    Event(Record),
    /// Box state is sampled every tick; equal samples extend the durable live interval.
    Profile { closed: String, live: String, seq: u64 },
}

#[derive(Clone, Debug, Default)]
struct NodeSt {
    addr: String,
    caps: Option<Caps>,
    up: bool,
    /// since when its caps went unanswered while it counted as up ([`DOWN_AFTER_MS`])
    unheard_ms: Option<u64>,
    /// the user's switch (nodes.conf); unset = by role
    dev: Option<bool>,
    reclaim_ms: Option<u64>,
    /// when its last lease was released (a waiting lease's handoff starts there)
    released_ms: Option<u64>,
    lat: Lat,
}

/// A node's recent lease latencies (ms), newest last: release -> next grant of a waiting lease,
/// setup, the job, teardown.
#[derive(Clone, Debug, Default)]
pub struct Lat {
    pub handoff: VecDeque<u64>,
    pub setup: VecDeque<u64>,
    pub job: VecDeque<u64>,
    pub teardown: VecDeque<u64>,
}

impl Lat {
    const KEEP: usize = 20;
    fn push(q: &mut VecDeque<u64>, v: u64) {
        q.push_back(v);
        while q.len() > Self::KEEP {
            q.pop_front();
        }
    }
    fn median(q: &VecDeque<u64>) -> Option<u64> {
        let mut v: Vec<u64> = q.iter().copied().collect();
        v.sort_unstable();
        let upper = *v.get(v.len() / 2)?;
        Some(if v.len() % 2 == 0 { let lower = v[v.len() / 2 - 1]; lower + (upper - lower) / 2 } else { upper })
    }
    /// `handoff 0.4 s, setup 3.1 s, job 2m10s, teardown 1.2 s (medians of 12)`; empty with no samples.
    pub fn line(&self) -> String {
        let f = |name: &str, q: &VecDeque<u64>| Self::median(q).map(|m| format!("{name} {}", if m >= 60_000 { dur(m) } else { format!("{:.1} s", m as f64 / 1000.0) }));
        let parts: Vec<String> = [f("handoff", &self.handoff), f("setup", &self.setup), f("job", &self.job), f("teardown", &self.teardown)].into_iter().flatten().collect();
        if parts.is_empty() {
            return String::new();
        }
        format!("{} (medians of {})", parts.join(", "), self.setup.len().max(self.teardown.len()))
    }
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
    /// dev off, or a person uses it, with leases still running
    pub draining: bool,
    /// the queued lease reserving the node
    pub hold: Option<String>,
    /// Self-update progress, informational: leases keep being placed.
    pub updating: Option<String>,
    /// its lease latencies ([`Lat::line`])
    pub latency: String,
}

impl NodeView {
    pub fn role(&self) -> Role {
        self.caps.as_ref().map_or(Role::Pool, |c| c.role())
    }

    /// A person uses the node ([`super::user`]): no new leases while dev is on.
    pub fn user(&self) -> Option<&super::UserSense> {
        self.caps.as_ref().and_then(|c| c.user.as_ref()).filter(|u| u.present)
    }

    /// `x86_64 linux · 16 cores · load 2.1 · 40/60 GB · disk 105 GB free (floor 25) · GPU-B`.
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
        s.push_str(&format!(" \u{b7} hub {}", if c.hub_rev.is_empty() { "old" } else { &c.hub_rev[..c.hub_rev.len().min(10)] }));
        s.push_str(&format!(" deploy {}", if c.deploy.is_empty() { "-" } else { &c.deploy[..c.deploy.len().min(10)] }));
        if let Some(store) = &c.jit_store {
            s.push_str(&format!(" \u{b7} JIT ids [{}], {} hits, {} misses, {} builds today (UTC)", store.ids.join(", "), store.hits, store.misses, store.builds_today));
        }
        if let Some(d) = c.desk_link.as_ref().filter(|d| d.on) {
            s.push_str(&if d.port > 0 { format!(" \u{b7} desk link :{}", d.port) } else { " \u{b7} desk link waiting".to_string() });
        }
        s
    }

    /// `dev on`, `dev off (user WM)`, `draining (2 leases)`, `user (key from its device 20 s
    /// ago)`, `held for timed L9`.
    pub fn state_word(&self) -> String {
        let mut s = if !self.up {
            "down".to_string()
        } else if let Some(u) = self.user().filter(|_| self.dev) {
            if self.leases > 0 {
                format!("{}, draining ({} leases)", u.line(), self.leases)
            } else {
                u.line()
            }
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
            s.push_str(&format!(", reserved for {h}"));
        }
        if let Some(why) = &self.updating {
            s.push_str(&format!(", self-update: {why}"));
        }
        s
    }

    pub fn to_json(&self) -> String {
        format!(
            "{{\"node\":{},\"addr\":{},\"up\":{},\"dev\":{},\"dev_set\":{},\"role\":\"{}\",\"leases\":{},\"waiting\":{},\"draining\":{},\"user\":{},\"hold\":{},\"updating\":{},\"state\":{},\"latency\":{},\"caps_line\":{},\"caps\":{}}}",
            json_str(&self.name),
            json_str(&self.addr),
            self.up,
            self.dev,
            self.dev_set,
            self.role().as_str(),
            self.leases,
            self.waiting,
            self.draining,
            self.user().is_some(),
            self.hold.as_deref().map_or("null".to_string(), json_str),
            self.updating.as_deref().map_or("null".to_string(), json_str),
            json_str(&self.state_word()),
            json_str(&self.latency),
            json_str(&self.caps_line()),
            self.caps.as_ref().map_or("null".to_string(), |c| c.to_json())
        )
    }
}

enum Place {
    Grant(String, Cost),
    /// reason, the node held for exclusive or interactive work, the node to reclaim disk on,
    /// and the boxes responsible for this normalized reason
    Wait(String, Option<String>, Option<String>, Vec<String>),
    Deny(String),
}

#[derive(Clone, Debug)]
struct Hold {
    id: String,
    until: u64,
}

type Holds = BTreeMap<String, Hold>;

/// One failed start condition, shared by placement, reservations and why-queued.
#[derive(Debug)]
struct Block {
    reason: String,
    blockers: Vec<String>,
    disk: bool,
}

impl Block {
    fn new(reason: impl Into<String>) -> Self { Self { reason: reason.into(), blockers: Vec::new(), disk: false } }
    fn by(reason: impl Into<String>, id: &str) -> Self { Self { reason: reason.into(), blockers: vec![id.to_string()], disk: false } }
}

/// How long a lane stays off a node it cancelled a lease on.
pub const CANCEL_EXCLUDE_MS: u64 = 30 * 60 * 1000;

pub struct Sched {
    /// until then a node not known yet may still be heard (a restarted master's links coming up):
    /// a lease waits for it instead of being denied
    pub nodes_settle_ms: u64,
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
    /// A native start refused stale placement: wait for fresh caps or a completed cleanup.
    native_wait: HashMap<String, String>,
    /// the last tick's time (placement has no clock of its own)
    clock: u64,
    profile_at: Option<u64>,
    profile_boxes: Vec<String>,
    profile_seq: u64,
    warm: HashSet<(String, String)>,
    /// node -> one eligible queued request, revalidated on every placement pass
    holds: Holds,
    /// Ended leases retain their conflicts and reservations until cleanup answers.
    restoring: HashMap<String, (String, Req, Cost)>,
    cleanup_unknown: HashMap<String, String>,
    walls: HashMap<(String, String), VecDeque<u64>>,
    runs: HashMap<(String, String), VecDeque<u64>>,
    node_walls: HashMap<(String, String, String), VecDeque<u64>>,
    peaks: HashMap<(String, String), VecDeque<f64>>,
    cpu: HashMap<(String, String), VecDeque<f64>>,
    /// Interactive demand without queued holds: a timed hold cannot hide its own competition.
    interactive_boxes: BTreeMap<String, Vec<String>>,
    /// The manager marks lanes whose user is waiting.
    pub interactive_lanes: HashSet<String>,
    /// The leases held by live jobs, as the live subsystem knows them: a lease granted before
    /// its request carried `live` (or restored without it) still parks for timed work.
    pub live_jobs: HashSet<String>,
    /// Restored `starting` leases the master asks their node to start again (`lease-start` is
    /// idempotent there): (id, node).
    pub resume: Vec<(String, String)>,
    /// Self-update progress for the node view; never a placement blocker.
    pub updating: HashMap<String, String>,
    /// Nodes whose hub predates handover (protocol < 6) and has its update ready: it restarts
    /// only between runs, so it takes no new lease until it has (once per box).
    pub update_drain: HashSet<String>,
    /// Live sessions between calls ([`Sched::charged`]): their app and JIT sit idle, so their box
    /// is charged their resident share; a call takes the full charge back ([`Sched::live_room`]).
    pub live_idle: HashSet<String>,
    /// Running leases already said to be past twice their measured run ([`Sched::say_stalled`]).
    stalled: HashSet<String>,
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

pub(super) fn dur(ms: u64) -> String {
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


/// May a twin node take this `--node N` lease? (Not `--only`, timed, screen or inspect leases.)
/// A hot box takes small leases, and medium test/screen ones; the class can still move with measurements.
/// A hot Mac takes any work that requires macOS (devhub's own JIT prewarms on the hot Mac were refused).
fn hot_refuses(role: Role, l: &Lease) -> bool {
    role == Role::Hot && !l.req.requires_macos() && l.cost_class != CostClass::Small && !(l.cost_class == CostClass::Medium && matches!(l.req.kind, Kind::Test | Kind::Screen))
}

fn soft_pin(r: &Req) -> bool {
    r.node.is_some() && !r.only && matches!(r.kind, Kind::Build | Kind::Test | Kind::Long | Kind::Gate | Kind::Gpu)
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
        // this version or newer; a hub that does not say passes (it is told on its update)
        "rustc" => {
            let ver = |s: &str| s.split('.').map(|p| p.parse::<u32>().unwrap_or(0)).collect::<Vec<u32>>();
            c.rustc.is_empty() || ver(&c.rustc) >= ver(&lv)
        }
        // exactly this version (verify and CI: a box on another toolchain disagrees with the rest)
        "rustc_eq" => c.rustc == v,
        // a test lease that opens windows (GUI tests, hidden presents): only nodes that present,
        // i.e. take screen leases (a box at slots.screen=0 while its presents time out)
        "gui" => (c.display.is_some() && slots(c, Kind::Screen) > 0) || matches!(lv.as_str(), "0" | "no" | "off" | "false"),
        _ => false,
    }
}

impl Sched {
    /// `machine`: the master's name (the records' machine); `next`: the first lease number.
    pub fn new(machine: &str, next: u64) -> Sched {
        Sched {
            nodes_settle_ms: 0,
            machine: machine.to_string(),
            nodes: BTreeMap::new(),
            leases: Vec::new(),
            ended: VecDeque::new(),
            next: next.max(1),
            seq: 0,
            last_grant: HashMap::new(),
            sticky: HashMap::new(),
            excluded: HashMap::new(),
            native_wait: HashMap::new(),
            clock: 0,
            profile_at: None,
            profile_boxes: Vec::new(),
            profile_seq: 0,
            warm: HashSet::new(),
            holds: Holds::new(),
            restoring: HashMap::new(),
            cleanup_unknown: HashMap::new(),
            walls: HashMap::new(),
            runs: HashMap::new(),
            node_walls: HashMap::new(),
            peaks: HashMap::new(),
            cpu: HashMap::new(),
            interactive_boxes: BTreeMap::new(),
            interactive_lanes: HashSet::new(),
            live_jobs: HashSet::new(),
            resume: Vec::new(),
            updating: HashMap::new(),
            update_drain: HashSet::new(),
            live_idle: HashSet::new(),
            stalled: HashSet::new(),
            out: Vec::new(),
            lane_cap: LANE_CAP,
            blocked: Vec::new(),
        }
    }

    /// Last nine observed walls per (class, kind); persistence belongs to the master.
    fn record_cost(&mut self, class: &str, kind: &str, wall: u64) {
        let walls = self.walls.entry((class.to_string(), kind.to_string())).or_default();
        walls.push_back(wall);
        while walls.len() > 9 { walls.pop_front(); }
    }

    /// Last nine run phases per (class, kind): no queue, sync, dev JIT build or report; what routing compares.
    pub fn record_run(&mut self, class: &str, kind: &str, run_ms: u64) {
        let runs = self.runs.entry((class.to_string(), kind.to_string())).or_default();
        runs.push_back(run_ms);
        while runs.len() > 9 { runs.pop_front(); }
    }

    pub fn record_node_cost(&mut self, class: &str, kind: &str, node: &str, wall: u64) {
        self.record_cost(class, kind, wall);
        let walls = self.node_walls.entry((class.to_string(), kind.to_string(), node.to_string())).or_default();
        walls.push_back(wall);
        while walls.len() > 9 { walls.pop_front(); }
    }

    /// Missing measurements from older hubs do not turn into zero-use samples.
    pub fn record_resources(&mut self, class: &str, kind: &str, peak_ram_gb: Option<f64>, cores: Option<f64>) {
        for (samples, value) in [(&mut self.peaks, peak_ram_gb), (&mut self.cpu, cores)] {
            if let Some(value) = value.filter(|v| v.is_finite() && *v >= 0.0) {
                let q = samples.entry((class.to_string(), kind.to_string())).or_default();
                q.push_back(value);
                while q.len() > 9 { q.pop_front(); }
            }
        }
    }

    fn p90(samples: &HashMap<(String, String), VecDeque<f64>>, req: &Req) -> Option<f64> {
        let q = samples.get(&(req.class().to_string(), req.kind.as_str().to_string())).filter(|q| q.len() >= 3)?;
        let mut sorted: Vec<_> = q.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        Some(sorted[(sorted.len() * 9).div_ceil(10) - 1])
    }

    /// The RAM ask of a request that names none (`--ram-gb`), from its class's measured peaks: for
    /// scheduling only, never a cap (a lease's scope caps only an asked RAM). The highest of its
    /// class's last runs, so a run over its ask raises the next one; with fewer than three, not
    /// below the kind's `default`; a filtered test's class without any, the highest of its whole
    /// suite's (a filtered run's small peak once capped the whole suite's run: OOM-killed).
    fn measured_ram(&self, req: &Req, default: f64) -> Option<f64> {
        let peaks = |class: &str| self.peaks.get(&(class.to_string(), req.kind.as_str().to_string())).filter(|q| !q.is_empty());
        let highest = |q: &VecDeque<f64>| q.iter().copied().fold(0.0, f64::max) * 1.2;
        let base = super::base_class(req.class());
        match peaks(req.class()) {
            Some(q) if q.len() >= 3 => Some(highest(q)),
            Some(q) => Some(highest(q).max(default)),
            None if base != req.class() => peaks(base).map(highest),
            None => None,
        }.map(|gb| gb.max(0.5))
    }

    fn node_median(&self, class: &str, kind: &str, node: &str) -> Option<u64> {
        self.node_walls.get(&(class.to_string(), kind.to_string(), node.to_string())).and_then(Lat::median)
    }

    fn run_ms(&self, req: &Req, node: &str) -> u64 {
        self.node_median(req.class(), req.kind.as_str(), node).or_else(|| self.median(req.class(), req.kind.as_str())).unwrap_or_else(|| {
            req.est_min.map(|m| (m * 60_000.0) as u64).unwrap_or(match req.asked_cost() { CostClass::Small => 30_000, CostClass::Medium => 300_000, CostClass::Large => 1_200_000 })
        })
    }

    fn has_loader(&self, name: &str, l: &Lease) -> bool {
        l.req.on_node() && self.nodes[name].caps.as_ref().and_then(|c| c.jit_store.as_ref()).is_some_and(|s| s.ids.iter().any(|held| l.req.loader.as_ref() == Some(held) || l.req.hint.contains(held)))
    }

    /// Missing measurements are estimates only; they never limit a run.
    fn finish_ms(&self, name: &str, l: &Lease, wait: u64) -> u64 {
        let build = if l.req.on_node() && (l.req.loader.as_deref().is_some_and(|id| !id.is_empty()) || !l.req.hint.is_empty()) && !self.has_loader(name, l) {
            self.node_median("jit-build", "", name).or_else(|| self.median("jit-build", "")).unwrap_or(90_000)
        } else { 0 };
        wait.saturating_add(build).saturating_add(self.run_ms(&l.req, name))
    }

    /// Earliest release that clears conflicts and budgets; concurrent runs overlap in time.
    fn remaining_ms(&self, name: &str, l: &Lease, cost: &Cost) -> u64 {
        let c = self.nodes[name].caps.as_ref().unwrap();
        let mut running: Vec<_> = self.live_on_node(name, &l.id).map(|r| {
            let wall = self.run_ms(&r.req, name);
            let elapsed = self.clock.saturating_sub(r.granted_ms.unwrap_or(self.clock));
            let remaining = if elapsed < wall { wall - elapsed } else { wall };
            (remaining, self.charged(r), l.req.conflicts_on(&r.req, c.role()))
        }).chain(self.restoring.values().filter(|(n, _, _)| n == name).map(|(_, req, cost)| {
            (Lat::median(&self.nodes[name].lat.teardown).unwrap_or(1_000), *cost, l.req.conflicts_on(req, c.role()) || l.req.same_tree(req, c.role()))
        })).collect();
        let mut used = *cost;
        let mut conflicts = 0;
        for (_, held, conflict) in &running {
            conflicts += usize::from(*conflict);
            used.cores += held.cores;
            used.ram_gb += held.ram_gb;
            used.disk_gb += held.disk_gb;
            used.gpus += held.gpus;
        }
        let fits = |used: &Cost, conflicts| conflicts == 0 && used.cores <= c.cores as f64 && used.ram_gb <= l.req.ram_budget(c) && used.gpus as usize <= c.gpus.len() && used.disk_gb <= c.disk_free_gb - c.disk_floor_gb;
        if fits(&used, conflicts) { return 0; }
        running.sort_by_key(|(t, _, _)| *t);
        let last = running.last().map_or(0, |(t, _, _)| *t);
        for (t, held, conflict) in running {
            conflicts -= usize::from(conflict);
            used.cores -= held.cores;
            used.ram_gb -= held.ram_gb;
            used.disk_gb -= held.disk_gb;
            used.gpus -= held.gpus;
            if fits(&used, conflicts) { return t; }
        }
        last
    }

    fn median(&self, class: &str, kind: &str) -> Option<u64> {
        self.walls.get(&(class.to_string(), kind.to_string())).and_then(Lat::median)
    }

    pub fn class_median(&self, class: &str, kind: &str) -> Option<u64> {
        self.median(class, kind)
    }

    /// Run-phase median for routing only: no queue, sync, dev JIT build or report.
    pub fn class_run_median(&self, class: &str, kind: &str) -> Option<u64> {
        self.runs.get(&(class.to_string(), kind.to_string())).and_then(Lat::median)
    }

    fn classify(&self, req: &Req) -> (CostClass, CostSource) {
        if let Some(walls) = self.walls.get(&(req.class().to_string(), req.kind.as_str().to_string())).filter(|w| w.len() >= 3) {
            let mut sorted: Vec<_> = walls.iter().copied().collect();
            sorted.sort_unstable();
            let p90 = sorted[(sorted.len() * 9).div_ceil(10) - 1];
            let upper = sorted[sorted.len() / 2];
            let median = if sorted.len() % 2 == 0 { let lower = sorted[sorted.len() / 2 - 1]; lower + (upper - lower) / 2 } else { upper };
            let ms = median.max((p90 as u128 * 7 / 10) as u64);
            return (CostClass::from_wall(ms, req.allocation(false).cores), CostSource::Measured(ms));
        }
        (req.asked_cost(), CostSource::fallback(req))
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
    pub fn set_caps(&mut self, name: &str, caps: Caps, _now: u64) -> bool {
        self.native_wait.remove(name);
        let n = self.nodes.entry(name.to_string()).or_default();
        let was = n.up;
        n.caps = Some(caps);
        n.up = true;
        n.unheard_ms = None;
        !was
    }

    /// Its caps did not answer: it counts as down once they have not for [`DOWN_AFTER_MS`] (a
    /// missed poll, a link the master's exec just dropped, is no reason to reroute its work).
    pub fn caps_missed(&mut self, name: &str, now: u64) {
        if let Some(n) = self.nodes.get_mut(name) {
            let since = *n.unheard_ms.get_or_insert(now);
            if now.saturating_sub(since) >= DOWN_AFTER_MS {
                n.up = false;
            }
        }
    }

    /// Down now: a lease's start could not reach it.
    pub fn set_down(&mut self, name: &str) {
        if let Some(n) = self.nodes.get_mut(name) {
            n.up = false;
        }
    }

    /// The node's dev switch (the user's, else on: every node, user WMs and devices included, defaults to dev on).
    pub fn dev(&self, name: &str) -> bool {
        self.nodes.get(name).is_some_and(|n| n.dev.unwrap_or(true))
    }

    /// A person uses the node (its caps' [`super::UserSense`]): it takes no new lease.
    fn user_present(&self, name: &str) -> bool {
        self.nodes.get(name).and_then(|n| n.caps.as_ref()).and_then(|c| c.user.as_ref()).is_some_and(|u| u.present)
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
        self.request_as(id, req, now)
    }

    /// A request becomes visible only when its ownership record is durable. The caller
    /// holds the scheduler and writer locks throughout this operation.
    pub(super) fn request_durable(&mut self, req: Req, now: u64, persist: impl FnOnce(Vec<String>) -> Result<(), String>) -> Result<String, String> {
        let actions = self.out.len();
        let id = self.request(req, now);
        if let Err(e) = persist(self.live_lines()) {
            self.leases.retain(|l| l.id != id);
            self.ended.retain(|l| l.id != id);
            self.out.truncate(actions);
            return Err(e);
        }
        Ok(id)
    }

    pub(super) fn retry_actions(&mut self, actions: Vec<Action>) {
        self.out.extend(actions);
    }

    /// [`Self::request`] under a given id (a queued request a previous master persisted).
    fn request_as(&mut self, id: String, req: Req, now: u64) -> String {
        self.seq += 1;
        let running = self.leases.iter().filter(|l| l.state.live() && l.req.lane == req.lane && l.req.kind == req.kind).count() as u32;
        let (cost_class, cost_source) = self.classify(&req);
        let l = Lease {
            id: id.clone(),
            cost: req.allocation(false),
            cost_class,
            cost_source,
            class_notified: false,
            priority: req.priority,
            asked_ms: req.since_ms.map_or(now, |t| t.min(now)),
            req,
            state: State::Queued,
            node: None,
            granted_ms: None,
            ended_ms: None,
            beat_ms: now,
            pos: 0,
            reason: String::new(),
            eta_ms: 0,
            warned: false,
            start: String::new(),
            info: String::new(),
            idle_since: None,
            hold_until: None,
            jit_wait_since: None,
            wait_ms: None,
            setup_ms: None,
            ship_ms: None,
            teardown_ms: None,
            phases: Default::default(),
            node_phases: Default::default(),
            reported_ms: None,
            queue_sample_ms: now,
            queue_boxes: Vec::new(),
            seq: self.seq,
        };
        if let Some(who) = self.blocked_of(&l.req) {
            self.leases.push(l);
            let i = self.leases.len() - 1;
            self.end_at(i, "denied", Sev::Warn, &format!("evicted by the user ({who} is blocked until `devhub unblock {who}`)"), now);
            return id;
        }
        if l.req.kind != Kind::Inspect && l.req.lane != CI_LANE && running >= self.lane_cap {
            self.event(Sev::Info, format!("{} for {}: past the lane cap ({running}/{} running): it runs after other lanes' leases, where none waits", l.tag(), l.req.lane, self.lane_cap), Some(&l));
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
        self.holds.retain(|_, h| h.id != l.id);
        l.sample_queue(now);
        if l.state == State::Starting {
            if let Some(start) = l.granted_ms { l.phases.push("sync", start, now); }
        }
        let node = if l.state.live() { l.node.clone() } else { None };
        let held = l.granted_ms.map(|g| now.saturating_sub(g));
        l.state = State::Ended(how.to_string());
        l.ended_ms = Some(now);
        l.reason = why.to_string();
        let line = match (how, held) {
            ("denied", _) => format!("{} for {} denied: {why}", l.tag(), l.req.lane),
            ("expired", _) => format!("{} of {} expired: no heartbeat for {} s{}", l.tag(), l.req.lane, BEAT_EXPIRY_MS / 1000, if node.is_some() { "; cleaning up" } else { " (was queued)" }),
            ("cancelled", _) => format!("{} of {} {why}{}", l.tag(), l.req.lane, if node.is_some() { "; cleaning up" } else { " (was queued)" }),
            ("evicted", _) => format!("{} of {} ended: evicted by the user{}", l.tag(), l.req.lane, if node.is_some() { "; cleaning up" } else { "" }),
            ("overridden", _) => format!("{} of {} ended: {why}{}", l.tag(), l.req.lane, if node.is_some() { "; cleaning up" } else { "" }),
            ("holder-dead", _) => format!("{} of {} released: its holder (pid {}) ended{}", l.tag(), l.req.lane, l.req.holder.unwrap_or(0), if node.is_some() { "; cleaning up" } else { " (was queued)" }),
            (_, Some(h)) => format!("{} of {} ended ({how}{}) after {}", l.tag(), l.req.lane, if why.is_empty() { String::new() } else { format!(": {why}") }, dur(h)),
            (_, None) => format!("{} for {} left the queue ({how}{})", l.tag(), l.req.lane, if why.is_empty() { String::new() } else { format!(": {why}") }),
        };
        self.event(sev, line, Some(&l));
        if let Some(n) = node.as_ref().filter(|_| l.req.on_node()) {
            self.restoring.insert(l.id.clone(), (n.clone(), l.req.clone(), l.cost));
            if let Some(st) = self.nodes.get_mut(n) {
                st.released_ms = Some(now);
                if let Some(g) = l.granted_ms {
                    Lat::push(&mut st.lat.job, now.saturating_sub(g).saturating_sub(l.setup_ms.unwrap_or(0)));
                }
            }
        }
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
        self.cancel_by(id, "user", now)
    }

    /// As [`Self::cancel`], by whoever asked: only the user's keeps the lane off the node (a lane
    /// or agent ending its own lease has said nothing about the node).
    pub fn cancel_by(&mut self, id: &str, by: &str, now: u64) -> bool {
        let Some(i) = self.leases.iter().position(|l| l.id == id) else { return false };
        let (lane, node) = (self.leases[i].req.lane.clone(), if self.leases[i].state.live() { self.leases[i].node.clone() } else { None });
        if let (Some(n), "user") = (node, by) {
            self.excluded.insert((lane, n), now + CANCEL_EXCLUDE_MS);
        }
        self.end_at(i, "cancelled", Sev::Warn, &format!("cancelled by {by}"), now);
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
            if let Ok(v) = makepad_strict_json::parse(info.as_bytes()) {
                if v.get("phases").is_some() { l.node_phases = super::prof::Phases::parse(v.get("phases")); }
            }
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

    /// Stop a lane's live and queued leases; it may ask again immediately.
    pub fn stop(&mut self, lane: &str, now: u64) -> Vec<String> {
        let ids: Vec<String> = self.leases.iter().filter(|l| l.req.lane == lane).map(|l| l.id.clone()).collect();
        for id in &ids {
            if let Some(i) = self.leases.iter().position(|l| l.id == *id) {
                self.end_at(i, "stopped", Sev::Info, "", now);
            }
        }
        self.event(Sev::Info, format!("{lane} stopped: {} leases ended", ids.len()), None);
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
                l.req.prio = BUMP_PRIO;
                l.req.node = Some(node.to_string());
                moved += 1;
            }
        }
        self.event(Sev::Warn, format!("node {node} overridden by the user: given to {who}; {} ended{}", ids.len(), if to.is_some() { format!(", {moved} of its waiting leases to the front") } else { String::new() }), None);
        Ok(ids)
    }

    /// The manager's bump: a queued lease goes to the front of its queue class.
    pub fn bump(&mut self, id: &str) -> Result<(), String> {
        let l = self.leases.iter_mut().find(|l| l.id == id).ok_or_else(|| format!("no lease {id}"))?;
        if l.state != State::Queued { return Err(format!("{id} is {}, not queued", l.state.as_str())); }
        l.req.prio = BUMP_PRIO;
        let l = l.clone();
        self.event(Sev::Warn, format!("{} of {} bumped to the front of the queue", l.tag(), l.req.lane), Some(&l));
        Ok(())
    }

    /// Leases whose holder process (on the master's machine) is gone.
    pub fn holders_gone(&self, alive: &dyn Fn(u32) -> bool) -> Vec<String> {
        self.leases.iter().filter(|l| l.req.holder.is_some_and(|p| !alive(p))).map(|l| l.id.clone()).collect()
    }

    /// The lease's box could not be reached when it was set up (its hub restarting for an update,
    /// the box gone): the box counts as down until its caps answer again, and the lease goes back
    /// to the queue, to the next box that fits, instead of failing.
    pub fn unreachable(&mut self, id: &str, why: &str) {
        let Some(l) = self.leases.iter_mut().find(|l| l.id == id && l.state == State::Starting) else { return };
        let node = l.node.take().unwrap_or_default();
        l.state = State::Queued;
        l.queue_sample_ms = l.phases.0.last().map_or(self.clock, |s| s.end.max(self.clock));
        l.granted_ms = None;
        l.wait_ms = None;
        l.reason = format!("{node} was unreachable; placed again");
        let l = l.clone();
        self.set_down(&node);
        self.event(Sev::Warn, format!("{} of {}: {node} unreachable at its start ({why}); {node} counts as down until its hub reports in, the lease waits for the next box", l.tag(), l.req.lane), Some(&l));
    }

    /// The node's previous lease is still leaving its tree: the lease is placed again.
    pub fn tree_busy(&mut self, id: &str, why: &str) {
        let Some(l) = self.leases.iter_mut().find(|l| l.id == id && l.state == State::Starting) else { return };
        let node = l.node.take().unwrap_or_default();
        l.state = State::Queued;
        l.queue_sample_ms = l.phases.0.last().map_or(self.clock, |s| s.end.max(self.clock));
        l.granted_ms = None;
        l.wait_ms = None;
        l.reason = format!("{node}: {why}; placed again");
        if why.contains("native trees busy:") { self.native_wait.insert(node, why.to_string()); }
    }

    /// The node set the lease up: `answer` is its `lease-start` line.
    /// False: the lease ended while its `lease-start` was on its way, and that end reached the node
    /// first; what the start set up there is nobody's.
    pub fn started(&mut self, id: &str, answer: &str) -> bool {
        match self.leases.iter_mut().find(|l| l.id == id && l.state == State::Starting) {
            Some(l) => {
                l.state = State::Granted;
                l.start = answer.to_string();
                true
            }
            None => false,
        }
    }

    /// The leases a restart must not lose, one line each (`id`, `state`, `node`, `granted_ms`,
    /// `req`, `start`): granted ones, ones the node is setting up, and the queue. What a
    /// restarted master takes back ([`Sched::restore`]).
    pub fn live_lines(&self) -> Vec<String> {
        self.leases
            .iter()
            .filter(|l| matches!(l.state, State::Granted | State::Starting | State::Queued))
            .map(|l| {
                let mut req = l.req.clone();
                req.priority = l.priority;
                format!("{{\"id\":{},\"state\":\"{}\",\"node\":{},\"granted_ms\":{},\"asked_ms\":{},\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"req\":{},\"start\":{},\"phases\":{},\"node_phases\":{},\"allocation\":{{\"cores\":{},\"ram_gb\":{},\"disk_gb\":{},\"gpus\":{}}}}}", json_str(&l.id), l.state.as_str(), json_str(l.node.as_deref().unwrap_or("")), l.granted_ms.unwrap_or(0), l.asked_ms, l.cost_class.as_str(), json_str(&l.cost_source.label()), l.cost_source.ms(), req.to_json(), json_str(&l.start), l.phases.json(), l.node_phases.json(), l.cost.cores, l.cost.ram_gb, l.cost.disk_gb, l.cost.gpus)
            })
            .collect()
    }

    /// Takes back a lease a previous master granted or queued ([`Sched::live_lines`]). Its holder has
    /// [`RESTORE_GRACE_MS`] to beat again (a lane's holder re-attaches, the master's own work goes
    /// on while its node links come up).
    pub fn restore(&mut self, line: &str, now: u64) -> Option<String> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let id = v.get("id")?.as_str()?.to_string();
        let n: u64 = id.strip_prefix('L')?.parse().ok()?;
        let req = Req::parse(&v.get("req")?.to_json()).ok()?;
        self.next = self.next.max(n + 1);
        let state = v.get("state").and_then(|x| x.as_str()).unwrap_or("granted");
        if state == "queued" {
            // the master's own ask: its asker died with the previous master, which asks again (kept,
            // it would take its box first and hold it through the grace with nothing running)
            if req.master { return None; }
            // the queue survives: the request keeps its id, its place (`since_ms`) and its holder's
            // beat grace; the next tick places it
            let asked = v.get("asked_ms").and_then(super::num).map(|x| x as u64);
            self.request_as(id.clone(), req, now);
            if let Some(l) = self.leases.iter_mut().find(|l| l.id == id) {
                if let Some(a) = asked { l.asked_ms = l.asked_ms.min(a); }
                l.beat_ms = now + RESTORE_GRACE_MS;
            }
            return Some(id);
        }
        self.seq += 1;
        let mut cost = req.allocation(false);
        if let Some(v) = v.get("allocation") {
            let number = |key| v.get(key).and_then(super::num).filter(|n| n.is_finite() && *n >= 0.0);
            if let Some(n) = number("cores") { cost.cores = n; }
            if let Some(n) = number("ram_gb") { cost.ram_gb = n; }
            if let Some(n) = number("disk_gb") { cost.disk_gb = n; }
            if let Some(n) = number("gpus") { cost.gpus = n as u32; }
        }
        let l = Lease {
            id: id.clone(),
            cost,
            cost_class: v.get("cost").and_then(|v| v.as_str()).and_then(CostClass::parse).unwrap_or_else(|| req.asked_cost()),
            cost_source: v.get("measured_ms").and_then(|v| v.as_u64()).map(CostSource::Measured).unwrap_or_else(|| CostSource::fallback(&req)),
            class_notified: true,
            asked_ms: v.get("asked_ms").and_then(|v| v.as_u64()).or(req.since_ms).unwrap_or_else(|| v.get("granted_ms").and_then(|v| v.as_u64()).unwrap_or(now)),
            priority: req.priority,
            req,
            state: if state == "starting" { State::Starting } else { State::Granted },
            node: v.get("node").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string),
            granted_ms: v.get("granted_ms").and_then(super::num).map(|x| x as u64),
            ended_ms: None,
            beat_ms: now + RESTORE_GRACE_MS,
            pos: 0,
            reason: String::new(),
            eta_ms: 0,
            warned: false,
            start: v.get("start").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            info: String::new(),
            idle_since: None,
            hold_until: None,
            jit_wait_since: None,
            wait_ms: None,
            setup_ms: None,
            ship_ms: None,
            teardown_ms: None,
            phases: super::prof::Phases::parse(v.get("phases")),
            node_phases: super::prof::Phases::parse(v.get("node_phases")),
            reported_ms: None,
            queue_sample_ms: now,
            queue_boxes: Vec::new(),
            seq: self.seq,
        };
        if l.state == State::Starting {
            if let Some(n) = l.node.clone() { self.resume.push((id.clone(), n)); }
        }
        self.leases.push(l);
        Some(id)
    }

    /// A timed lease queued, live, or ended within [`TIMED_SERIES_MS`] for the worktree `dir`:
    /// its series pins a binary built from that tree, so nothing merges into the lane meanwhile.
    pub fn timed_series(&self, dir: &str, now: u64) -> Option<String> {
        let at = |l: &Lease| l.req.repo_dir.as_deref().is_some_and(|r| !r.is_empty() && (dir == r || dir.starts_with(&format!("{r}/")) || r.starts_with(&format!("{dir}/"))));
        let live = self.leases.iter().find(|l| l.req.kind == Kind::Timed && at(l));
        if let Some(l) = live { return Some(format!("{} (timed) holds a binary built from this worktree", l.id)); }
        self.ended.iter().rev().find(|l| l.req.kind == Kind::Timed && at(l) && l.ended_ms.is_some_and(|e| now.saturating_sub(e) < TIMED_SERIES_MS))
            .map(|l| format!("timed series: {} ended {} min ago", l.id, now.saturating_sub(l.ended_ms.unwrap_or(now)) / 60_000))
    }

    /// Why the master should not restart now: a lease between states (a node setting one up,
    /// or still cleaning up after one that ended), if any.
    pub fn in_transition(&self) -> Option<String> {
        if let Some(l) = self.leases.iter().find(|l| l.state == State::Starting) {
            return Some(format!("{} is being set up on {}", l.id, l.node.as_deref().unwrap_or("?")));
        }
        self.restoring.iter().next().map(|(id, (n, _, _))| format!("{id} is being cleaned up on {n}"))
    }

    /// A failed cleanup cannot establish available capacity, even after fresh caps.
    pub fn cleanup_failed(&mut self, node: &str, id: &str) {
        self.cleanup_unknown.insert(id.to_string(), node.to_string());
    }

    /// The node confirmed cleanup with no surviving processes after `teardown_ms`.
    pub fn cleaned(&mut self, node: &str, id: &str, teardown_ms: u64) {
        self.cleanup_unknown.remove(id);
        self.native_wait.remove(node);
        if self.restoring.get(id).is_some_and(|(n, _, _)| n == node) {
            self.restoring.remove(id);
        }
        if let Some(l) = self.ended.iter_mut().rev().find(|l| l.id == id) {
            l.teardown_ms = Some(teardown_ms);
        }
        if let Some(n) = self.nodes.get_mut(node) {
            Lat::push(&mut n.lat.teardown, teardown_ms);
        }
    }

    /// The node set lease `id` up in `setup_ms` (its revisions shipped in `ship_ms` of them).
    pub fn set_up(&mut self, id: &str, setup_ms: u64, ship_ms: u64) {
        let Some(l) = self.leases.iter_mut().find(|l| l.id == id) else { return };
        l.setup_ms = Some(setup_ms);
        l.ship_ms = Some(ship_ms);
        if let Some(n) = l.node.clone().and_then(|n| self.nodes.get_mut(&n)) {
            Lat::push(&mut n.lat.setup, setup_ms);
        }
    }

    /// Master-owned phases and the node's latest complete timing snapshot.
    pub fn phase(&mut self, id: &str, phase: &str, start: u64, end: u64) {
        if let Some(l) = self.leases.iter_mut().chain(self.ended.iter_mut()).find(|l| l.id == id) {
            l.phases.push(phase, start, l.ended_ms.map_or(end, |t| t.min(end)));
        }
    }

    pub fn reported(&mut self, id: &str, phases: super::prof::Phases, now: u64) {
        if let Some(l) = self.ended.iter_mut().find(|l| l.id == id) {
            l.node_phases = phases;
            l.phases.push("report", l.ended_ms.unwrap_or(now), now);
            l.reported_ms = Some(now);
        }
    }

    /// Sample after placement, using the same candidates and start conditions as the
    /// work-conserving loop. Idle-while-queued means a request could still start on that box.
    /// Offline boxes do not contribute observation time. Busy includes setup and teardown.
    fn box_samples(&self) -> Vec<String> {
        let tag = |lane: &str, kind: Kind| format!("{{\"lane\":{},\"kind\":{}}}", json_str(lane), json_str(kind.as_str()));
        let mut idle_by_node: BTreeMap<&String, Vec<String>> = BTreeMap::new();
        for l in self.leases.iter().filter(|l| l.state == State::Queued && l.req.on_node()) {
            let Ok(names) = self.candidates(l, &self.holds) else { continue };
            for name in names {
                if self.now_fits(name, l, &self.cost_on(name, l), &self.holds).is_ok() {
                    idle_by_node.entry(name).or_default().push(tag(&l.req.lane, l.req.kind));
                }
            }
        }
        self.nodes.iter().filter_map(|(name, n)| {
            n.caps.as_ref().filter(|_| n.up)?;
            let mut busy = Vec::new();
            for req in self.live_on_node(name, "").filter(|l| l.req.on_node()).map(|l| &l.req)
                .chain(self.restoring.values().filter(|(node, _, _)| node == name).map(|(_, r, _)| r)) {
                busy.push(tag(&req.lane, req.kind));
            }
            let mut idle = idle_by_node.remove(name).unwrap_or_default();
            busy.sort();
            busy.dedup();
            idle.sort();
            idle.dedup();
            Some(format!("\"node\":{},\"busy\":[{}],\"idle-while-queued\":[{}]", json_str(name), busy.join(","), idle.join(",")))
        }).collect()
    }

    /// The node could not set it up: the lease ends (`failed`, warn).
    pub fn failed(&mut self, id: &str, err: &str, now: u64) {
        if let Some(i) = self.leases.iter().position(|l| l.id == id) {
            self.end_at(i, "failed", Sev::Warn, err, now);
        }
    }

    /// Ids of the started leases on `node` (the master's `lease-beat` to it).
    /// A tree inspect lease only attaches to a JIT; an overlay inspect starts its own run.
    pub fn live_on(&self, node: &str) -> Vec<String> {
        self.leases.iter().filter(|l| l.state == State::Granted && l.req.on_node() && l.node.as_deref() == Some(node)).map(|l| l.id.clone()).collect()
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

    /// When a lease of the lane (by name, or run from under `dir`) was last asked or ended; a
    /// queued or live one counts as now (0: none kept).
    pub fn lane_seen_ms(&self, name: &str, dir: &std::path::Path) -> u64 {
        self.leases.iter().chain(self.ended.iter())
            .filter(|l| l.req.lane == name || l.req.repo_dir.as_deref().is_some_and(|d| std::path::Path::new(d).starts_with(dir)))
            .map(|l| l.ended_ms.unwrap_or(self.clock).max(l.asked_ms)).max().unwrap_or(0)
    }

    pub fn recent_ended(&self, n: usize) -> Vec<Lease> {
        self.ended.iter().rev().take(n).cloned().collect()
    }

    #[cfg(feature = "checkout")]
    pub(crate) fn jit_compilers(&self) -> Vec<String> {
        let mut compilers: Vec<_> = self.nodes.values().filter_map(|n| n.caps.as_ref()?.jit_store.as_ref())
            .map(|s| &s.compiler).filter(|c| !c.is_empty()).cloned().collect();
        compilers.sort();
        compilers.dedup();
        compilers
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
                    draining: (!dev || self.user_present(name)) && leases > 0,
                    hold: self.holds.get(name).map(|h| h.id.clone()),
                    updating: self.updating.get(name).cloned(),
                    latency: n.lat.line(),
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
        // The Macs are scarce and outside the general pool: they take only work that requires
        // macOS (Metal, the macOS platform layer, arm64-macOS JIT builds, macOS timings), never
        // overflow while the Linux boxes are busy. An inspect lease looks at a JIT already there.
        if c.os.eq_ignore_ascii_case("macos") && k != Kind::Inspect && !l.req.requires_macos() {
            return Err("a Mac: the Macs take only work that requires macOS; add --needs os=macos only if it does (Metal, the macOS platform layer, macOS timings), else leave the Mac out and a Linux node takes it".into());
        }
        // (a hot Mac takes any macOS work: Metal checks and arm64 builds go to whichever Mac is free)
        if c.role() == Role::Hot && !l.req.requires_macos() && (l.req.exclusive() || matches!(k, Kind::Build | Kind::Long | Kind::Gate | Kind::Timed | Kind::Ci)) {
            return Err(format!("hot box: no {} leases", k.as_str()));
        }
        if k == Kind::Ci && c.role().users() {
            return Err("a user's box takes no ci leases".into());
        }
        if k == Kind::Ci && c.role() == Role::Long {
            return Err("a long-cycle box takes no ci leases".into());
        }
        if l.req.ci() && slots(c, Kind::Ci) == 0 {
            return Err("takes no CI (its lease.conf has no `slots.ci`)".into());
        }

        if k == Kind::Timed && c.role() != Role::Bench && self.bench_meets(l) {
            return Err(format!("timed leases run on the bench boxes ({})", self.bench_nodes().join(", ")));
        }
        if k == Kind::Timed && !c.timed {
            return Err("takes no timed leases".into());
        }
        if k != Kind::Inspect && slots(c, k) == 0 {
            return Err(format!("takes no {} leases", k.as_str()));
        }
        // the user's own desktop: a lease that does not name it would run (and inject input) under
        // the person, sensed or not (J305, 2026-10-30); a soft pin's fallback does not name it
        if l.req.on_node() && c.role().users() && l.req.node.as_deref() != Some(name) {
            return Err("the user's WM: only a lease that names it".into());
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
        if cost.ram_gb > l.req.ram_budget(c) {
            return Err(format!("needs {:.0} GB RAM, may use {:.1} of its {:.0} GB", cost.ram_gb, l.req.ram_budget(c), c.ram_gb));
        }
        if cost.gpus as usize > c.gpus.len() {
            return Err("has no GPU".into());
        }
        if c.disk_total_gb > 0.0 && cost.disk_gb > c.disk_total_gb - c.disk_floor_gb {
            return Err(format!("needs {:.0} GB disk, the volume holds {:.0} over its floor", cost.disk_gb, c.disk_total_gb - c.disk_floor_gb));
        }
        Ok(())
    }

    fn cost_on(&self, name: &str, l: &Lease) -> Cost {
        let mut cost = l.req.allocation(self.warm_on(name, l));
        if l.req.ram_gb.is_none() {
            if let Some(gb) = self.measured_ram(&l.req, cost.ram_gb) { cost.ram_gb = gb; }
            // an unasked need (the kind's default, or a p90 pooled over every box's leases of the
            // class) takes all of a smaller box's budget, never a denial: a 16 GB Mac's show-prewarm
            // against a build's 12 GB default or Linux builds' peaks
            if let Some(c) = self.nodes[name].caps.as_ref() { cost.ram_gb = cost.ram_gb.min(l.req.ram_budget(c)); }
        }
        if l.req.cores.is_none() {
            if let Some(p90) = Self::p90(&self.cpu, &l.req) {
                cost.cores = p90.ceil().max(1.0).min(self.nodes[name].caps.as_ref().map_or(f64::INFINITY, |c| c.cores as f64));
            }
        }
        cost
    }

    fn allocation_note(&self, name: &str, l: &Lease) -> String {
        let cost = self.cost_on(name, l);
        let source = |asked: Option<f64>, p90: Option<f64>| {
            if asked.is_some() { "asked".into() } else { p90.map_or("kind".into(), |n| format!("measured p90 {n:.1}")) }
        };
        let default = l.req.allocation(self.warm_on(name, l)).ram_gb;
        let ram = match self.measured_ram(&l.req, default) {
            _ if l.req.ram_gb.is_some() => "asked".to_string(),
            Some(gb) if gb != default => format!("measured peak {:.1}", gb / 1.2),
            _ => "kind".to_string(),
        };
        format!("ram {:.1} GB ({ram}); cores {} ({})", cost.ram_gb, cost.cores, source(l.req.cores, Self::p90(&self.cpu, &l.req)))
    }

    fn refresh_interactive(&mut self) {
        self.interactive_boxes.clear();
        let mut boxes = BTreeMap::<String, Vec<String>>::new();
        for l in self.leases.iter().filter(|l| l.priority == Priority::Interactive && l.req.kind != Kind::Timed && l.req.on_node()) {
            if l.state.live() {
                // A running live job is no reason for timed work to wait: it parks for it
                // ([`Self::park_live`]). Counted here, it held a timed lease off its box for as
                // long as the job was touched (one held a box for 92 min on 11-01).
                if self.is_live_job(l) { continue; }
                if let Some(name) = &l.node { boxes.entry(name.clone()).or_default().push(l.id.clone()); }
            } else if l.state == State::Queued {
                // Ignore reservations when finding demand: an existing timed hold must yield
                // on the first tick, well within the interactive request's 60-second grace.
                // One that names no box claims a bench box only when no other is ready (or the lane
                // is warm there): placement takes those first.
                if let Ok(names) = self.candidates(l, &Holds::new()) {
                    let ready: Vec<_> = names.into_iter().filter(|name| self.ready(name, l, &self.cost_on(name, l)).is_ok()).collect();
                    let bench = |name: &str| l.req.node.is_none() && !self.warm_on(name, l) && self.nodes[name].caps.as_ref().is_some_and(|c| c.role() == Role::Bench);
                    let spare = ready.iter().any(|name| !bench(name));
                    for name in ready.into_iter().filter(|name| !spare || !bench(name)) { boxes.entry(name.clone()).or_default().push(l.id.clone()); }
                }
            }
        }
        self.interactive_boxes = boxes;
    }

    /// A timed request that waited [`TIMED_DUE_MS`]: interactive demand no longer keeps it off a
    /// box (running work still finishes first; the box it reserves takes no new work meanwhile).
    fn timed_due(&self, l: &Lease) -> bool {
        l.req.kind == Kind::Timed && l.state == State::Queued && self.clock.saturating_sub(l.asked_ms) >= TIMED_DUE_MS
    }

    fn timed_wait(&self, name: &str, l: &Lease) -> Option<Block> {
        if l.req.kind != Kind::Timed || self.timed_due(l) { return None; }
        let blockers = self.interactive_boxes.get(name)?;
        Some(Block { reason: format!("held for interactive {} (timed work waits for a quiet window)", blockers.join(", ")), blockers: blockers.clone(), disk: false })
    }

    /// The bench boxes (role `bench`), first choice first (by name): timed leases (benchmarks) run
    /// there, so timing series stay comparable and the other boxes keep building. Another box takes
    /// a timed lease only for needs no bench box meets (a GPU or an OS none of them has).
    fn bench_nodes(&self) -> Vec<&str> {
        self.nodes.iter().filter(|(_, n)| n.caps.as_ref().is_some_and(|c| c.role() == Role::Bench)).map(|(name, _)| name.as_str()).collect()
    }

    /// A bench box meets the lease's needs.
    fn bench_meets(&self, l: &Lease) -> bool {
        self.nodes.values().any(|n| n.caps.as_ref().is_some_and(|c| c.role() == Role::Bench && l.req.needs.iter().all(|(k, v)| meets(c, k, v))))
    }

    /// How busy the box is, 0..1+: its load per core, or the cores or RAM granted on it.
    fn busy(&self, name: &str) -> f64 {
        let Some(c) = self.nodes[name].caps.as_ref() else { return 1.0 };
        let (mut cores, mut ram) = (0.0, 0.0);
        for r in self.live_on_node(name, "") {
            let charged = self.charged(r);
            cores += charged.cores;
            ram += charged.ram_gb;
        }
        let n = c.cores.max(1) as f64;
        (c.load1 / n).max(cores / n).max(if c.ram_gb > 0.0 { ram / c.ram_gb } else { 0.0 })
    }

    /// A running lease past twice the measured run of its class on its box, and 5 min over it,
    /// says so, once: a hung leg holds its box unseen otherwise (a gate leg sat 40 min on an app's
    /// first frame against its 13 min, 11-02); a short one's JIT rebuild is no stall (a 1 min gate
    /// step said so at 2.5 min). Live sessions run open-ended and are not measured this way.
    fn say_stalled(&mut self, now: u64) {
        let mut said = Vec::new();
        for l in self.leases.iter().filter(|l| l.state == State::Granted && !self.stalled.contains(&l.id) && !self.is_live_job(l)) {
            let Some(node) = l.node.as_deref() else { continue };
            let Some(measured) = self.node_median(l.req.class(), l.req.kind.as_str(), node).or_else(|| self.median(l.req.class(), l.req.kind.as_str())).filter(|m| *m >= 60_000) else { continue };
            let ran = now.saturating_sub(l.granted_ms.unwrap_or(now));
            if ran > (2 * measured).max(measured + STALL_SLACK_MS) {
                let why = format!("{} of {} running {} against its measured {} ({})", l.tag(), l.req.lane, dur(ran), dur(measured), l.req.class());
                said.push((l.id.clone(), node.to_string(), format!("{} of {} on {node}: running {} against its measured {} ({}): stalled? `devhub cancel {}` frees the box", l.tag(), l.req.lane, dur(ran), dur(measured), l.req.class(), l.id), why));
            }
        }
        for (id, node, line, why) in said {
            let l = self.get(&id).cloned();
            self.event(Sev::Warn, line, l.as_ref());
            self.out.push(Action::Capture { id: id.clone(), node, why });
            self.stalled.insert(id);
        }
    }

    /// What a running lease takes of its box's budget: its cost, but a live session between
    /// calls only its resident share, [`LIVE_IDLE_CORES`] and its memory (24 idle sessions kept
    /// 16/16 cores admitted on boxes at 1-7% CPU while 23 leases waited, 11-02).
    fn charged(&self, r: &Lease) -> Cost {
        if self.live_idle.contains(&r.id) && self.is_live_job(r) {
            Cost { cores: r.cost.cores.min(LIVE_IDLE_CORES), ..r.cost }
        } else {
            r.cost
        }
    }

    /// Whether an idle live session's box has room for its full charge again (its next call is
    /// admitted like a lease): why not, if not.
    pub fn live_room(&self, id: &str) -> Result<(), String> {
        let Some(l) = self.get(id) else { return Ok(()) };
        let Some(name) = l.node.as_deref().filter(|n| self.nodes.contains_key(*n)) else { return Ok(()) };
        let Some(c) = self.nodes[name].caps.as_ref() else { return Ok(()) };
        let (mut cores, mut ram) = (l.cost.cores, l.cost.ram_gb);
        for r in self.live_on_node(name, id) {
            let charged = self.charged(r);
            cores += charged.cores;
            ram += charged.ram_gb;
        }
        for (_, _, cost) in self.restoring.values().filter(|(n, _, _)| n == name) {
            cores += cost.cores;
            ram += cost.ram_gb;
        }
        if cores > c.cores as f64 { return Err(format!("{name}: budget cores {:.0}/{}", cores - l.cost.cores, c.cores)); }
        let budget = l.req.ram_budget(c);
        if ram > budget { return Err(format!("{name}: budget ram {:.0}/{budget:.0} GB", ram - l.cost.ram_gb)); }
        Ok(())
    }

    fn warm_on(&self, name: &str, l: &Lease) -> bool {
        if let Some(c) = self.nodes[name].caps.as_ref().filter(|c| l.req.native(c.role())) {
            c.native.iter().any(|lane| lane == l.req.tree_lane())
        } else { self.warm.contains(&(l.req.lane.clone(), name.to_string())) }
    }

    #[cfg(feature = "checkout")]
    pub(crate) fn note_warm(&mut self, lane: &str, node: &str) {
        self.warm.insert((lane.to_string(), node.to_string()));
    }

    #[cfg(feature = "checkout")]
    pub(crate) fn forget_warm(&mut self, lane: &str, node: &str) {
        self.warm.remove(&(lane.to_string(), node.to_string()));
    }

    fn lane_block(&self, l: &Lease) -> Option<Block> {
        if l.req.kind == Kind::Inspect || l.req.lane == CI_LANE { return None; }
        let running: Vec<_> = self.leases.iter().filter(|r| r.state.live() && r.req.kind == l.req.kind && r.req.lane == l.req.lane).collect();
        (running.len() as u32 >= self.lane_cap).then(|| Block { reason: format!("lane cap {}/{}", running.len(), self.lane_cap), blockers: running.iter().map(|r| r.id.clone()).collect(), disk: false })
    }

    /// Everything other than running leases and queued reservations must already permit a start.
    /// A lease past its lane cap is no demand anywhere (reservations, interactive and small-work
    /// demand, idle sampling): only its own placement takes it past the cap ([`Self::place`]).
    fn ready(&self, name: &str, l: &Lease, cost: &Cost) -> Result<(), Block> {
        self.ready_as(name, l, cost, false)
    }

    fn ready_as(&self, name: &str, l: &Lease, cost: &Cost, past_cap: bool) -> Result<(), Block> {
        let n = &self.nodes[name];
        if !n.up { return Err(Block::new("node down")); }
        let c = n.caps.as_ref().ok_or_else(|| Block::new("no caps yet"))?;
        self.never(name, c, l, cost).map_err(Block::new)?;
        if !past_cap {
            if let Some(b) = self.lane_block(l) { return Err(b); }
        }
        if !l.req.on_node() { return Ok(()); }
        if self.cleanup_unknown.values().any(|node| node == name) { return Err(Block::new("capacity unknown: lease cleanup unconfirmed")); }
        if !self.dev(name) { return Err(Block::new("dev off")); }
        // a live session stays up for its idle half hour: a background one takes another box than
        // one a timed request is pinned to, or that box never drains for the benchmark. An
        // interactive one only prefers another box (placement); where none can take it, the timed
        // request yields to it and parks it once its call ends (W517 waited 7 min on the one box
        // with its GPU, 11-02)
        if l.state == State::Queued && self.is_live_job(l) && l.priority != Priority::Interactive && l.req.node.as_deref() != Some(name) {
            if let Some(t) = self.leases.iter().find(|r| r.state == State::Queued && r.req.kind == Kind::Timed && r.req.node.as_deref() == Some(name)) {
                return Err(Block::by(format!("{} (timed) waits for this box: live sessions go to another", t.id), &t.id));
            }
        }
        if let Some(u) = c.user.as_ref().filter(|u| u.present) {
            return Err(Block::new(format!("{} (back after {} min without input)", u.line(), u.idle_min)));
        }
        if let Some(why) = self.native_wait.get(name).filter(|_| l.req.native(c.role())) {
            return Err(Block { reason: why.clone(), blockers: Vec::new(), disk: why.contains("disk below") });
        }
        if self.update_drain.contains(name) {
            return Err(Block::new("its older hub restarts for its update between runs: no new lease until it has"));
        }
        // its windows would fail there (four visible live apps exited 1 on a node while new ones kept
        // its WM restart waiting, 11-02): the restart runs once the ones there end
        if !c.wm_wire.is_empty() && l.req.uses_wm() {
            return Err(Block::new(format!("its WM speaks another hosted wire ({}): no screen or gui lease until it restarts, once the ones there end", c.wm_wire)));
        }
        let needed = l.req.proto_needed(c.role());
        if c.hub_proto < needed {
            return Err(Block::new(format!("its hub speaks protocol {} (< {needed} for this request): waits for its update{}", c.hub_proto, if c.hub_proto == 0 { " (one restart by hand)" } else { "" })));
        }
        if !c.sick.is_empty() && l.req.kind != Kind::Inspect {
            return Err(Block { reason: format!("its toolchain cannot link: {}", c.sick), blockers: Vec::new(), disk: true });
        }
        // Class can change with measurements; large work also ages onto hot boxes.
        if hot_refuses(c.role(), l) && !(l.cost_class == CostClass::Large && self.clock.saturating_sub(l.asked_ms) >= AGE_STEP_MS) {
            return Err(Block::new(format!("class {}: hot box takes small or medium test/screen; large work waits until it has waited 10 min", l.cost_class.as_str())));
        }
        // Throttled runnable threads inflate load1; use actual CPU and available RAM.
        if l.cost_class != CostClass::Small && l.req.kind.writes() && (c.cpu_pct.is_some_and(|p| p > 95.0) || (c.ram_gb > 0.0 && c.ram_avail_gb < c.ram_gb * 0.1)) {
            return Err(Block::new("node busy"));
        }
        self.resources(name, l, cost, false)
    }

    /// With `running` false, even removing all running leases must leave enough usable budget.
    /// Cleanup and external disk pressure cannot be reserved away.
    fn resources(&self, name: &str, l: &Lease, cost: &Cost, running: bool) -> Result<(), Block> {
        self.resources_without(name, l, cost, running, &[])
    }

    fn resources_without(&self, name: &str, l: &Lease, cost: &Cost, running: bool, removed: &[String]) -> Result<(), Block> {
        if !l.req.on_node() { return Ok(()); }
        let c = self.nodes[name].caps.as_ref().unwrap();
        if let Some((id, _)) = self.restoring.iter().filter(|(_, (n, req, _))| n == name && (l.req.conflicts_on(req, c.role()) || l.req.same_tree(req, c.role()))).min_by_key(|(id, _)| *id) {
            return Err(Block::by(format!("scrubbing its work tree after {id}"), id));
        }
        if running {
            if let Some(r) = self.live_on_node(name, &l.id).filter(|r| !removed.contains(&r.id)).find(|r| l.req.conflicts_on(&r.req, c.role())) {
                return Err(Block::by(format!("held by {} ({} of {})", r.id, r.req.kind.as_str(), r.req.lane), &r.id));
            }
        }
        if l.req.native(c.role()) && !c.native.iter().any(|lane| lane == l.req.tree_lane()) {
            let holders: Vec<_> = self.live_on_node(name, &l.id).filter(|r| running && !removed.contains(&r.id)).map(|r| (&r.id, &r.req))
                .chain(self.restoring.iter().filter(|(_, (n, _, _))| n == name).map(|(id, (_, req, _))| (id, req)))
                .filter(|(_, req)| req.native(c.role())).collect();
            let lanes: HashSet<_> = holders.iter().map(|(_, req)| req.tree_lane()).collect();
            if !lanes.contains(l.req.tree_lane()) && lanes.len() >= c.native_trees as usize {
                return Err(Block { reason: format!("native trees busy: {}/{} held", lanes.len(), c.native_trees), blockers: holders.iter().map(|(id, _)| (*id).clone()).collect(), disk: false });
            }
        }
        let held: Vec<_> = self.live_on_node(name, &l.id).filter(|r| running && !removed.contains(&r.id)).map(|r| (&r.id, self.charged(r)))
            .chain(self.restoring.iter().filter(|(_, (n, _, _))| n == name).map(|(id, (_, _, cost))| (id, *cost))).collect();
        let mut used = Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 0.0, gpus: 0 };
        for (_, cost) in &held {
            used.cores += cost.cores;
            used.ram_gb += cost.ram_gb;
            used.disk_gb += cost.disk_gb;
            used.gpus += cost.gpus;
        }
        let blockers = || { let mut ids: Vec<_> = held.iter().map(|(id, _)| (*id).clone()).collect(); ids.sort(); ids };
        for (what, have, total, add) in [("cores", used.cores, c.cores as f64, cost.cores), ("ram", used.ram_gb, l.req.ram_budget(c), cost.ram_gb), ("gpu", used.gpus as f64, c.gpus.len() as f64, cost.gpus as f64)] {
            if have + add > total { return Err(Block { reason: format!("budget {what} {have:.0}/{total:.0} + {add:.0}"), blockers: blockers(), disk: false }); }
        }
        let free = c.disk_free_gb - used.disk_gb;
        if free - cost.disk_gb < c.disk_floor_gb {
            return Err(Block { reason: format!("disk floor: {free:.0} GB free, needs {:.0}, floor {:.0}", cost.disk_gb, c.disk_floor_gb), blockers: blockers(), disk: true });
        }
        Ok(())
    }

    /// Exclusive requests drain in this order: bumped first, then interactive before background,
    /// then arrival.
    fn exclusive_order(r: &Lease) -> (bool, bool, u64, u64) {
        (r.req.prio > BUMP_PRIO, r.priority != Priority::Interactive, r.asked_ms, r.seq)
    }

    /// An exclusive request's expected wait: its box's running work, then the exclusive requests
    /// ahead of it there.
    fn eta_ms(&self, l: &Lease) -> u64 {
        let name = match l.req.node.clone().filter(|n| self.nodes.contains_key(n)) {
            Some(n) => n,
            None => match self.waiting_box(l, &self.holds, false) { Some(n) => n, None => return 0 },
        };
        if self.nodes[&name].caps.is_none() { return 0; }
        let ahead: u64 = self.leases.iter().filter(|r| r.state == State::Queued && r.req.exclusive() && r.id != l.id && Self::exclusive_order(r) < Self::exclusive_order(l)
            && (r.req.node.as_deref() == Some(name.as_str()) || self.holds.get(&name).is_some_and(|h| h.id == r.id))).map(|r| self.run_ms(&r.req, &name)).sum();
        self.remaining_ms(&name, l, &self.cost_on(&name, l)).saturating_add(ahead)
    }

    /// Work that can coexist with the reservation's owner never waits for that reservation.
    fn held_by(&self, name: &str, l: &Lease, cost: &Cost, holds: &Holds) -> Option<Block> {
        if l.req.top() { return None; }
        let h = holds.get(name).filter(|h| h.id != l.id && h.until > self.clock)?;
        let owner = self.get(&h.id).filter(|r| r.state == State::Queued)?;
        if owner.req.kind == Kind::Timed && !self.timed_due(owner) && self.interactive_boxes.contains_key(name) { return None; }
        // A shared/whole reservation cannot split a timed batch after a long running
        // benchmark outlives the next timed request's reservation window.
        // Only an older timed request passes a reservation: exclusive work is served in arrival order
        // (a bumped request first), and a newer timed request's reservation, taken after the older
        // one's window ran out, never hands it the box first.
        if l.req.kind == Kind::Timed && (owner.req.kind == Kind::Timed || owner.priority != Priority::Interactive) && Self::exclusive_order(l) < Self::exclusive_order(owner) { return None; }
        if !l.req.on_node() { return None; }
        let c = self.nodes[name].caps.as_ref()?;
        let reserved = self.cost_on(name, owner);
        if !l.req.conflicts_on(&owner.req, c.role()) && cost.cores + reserved.cores <= c.cores as f64 && cost.ram_gb + reserved.ram_gb <= l.req.ram_budget(c) && cost.gpus as usize + reserved.gpus as usize <= c.gpus.len() && cost.disk_gb + reserved.disk_gb <= c.disk_free_gb - c.disk_floor_gb {
            return None;
        }
        // Backfill: shared work that could start now and is expected to end before the owner
        // could start anyway (its running blockers' estimates) never delays it. A live session
        // stays up until parked, whatever its estimate: it never backfills.
        if !l.req.exclusive() && !self.is_live_job(l) && self.resources(name, l, cost, true).is_ok() {
            let wait = self.remaining_ms(name, owner, &reserved);
            if wait > 0 && self.finish_ms(name, l, 0) <= wait { return None; }
        }
        if owner.req.kind == Kind::Timed {
            return Some(Block::by(format!("held for timed {} (the box drains for it: only work that ends first or fits beside it)", h.id), &h.id));
        }
        let what = if owner.req.whole { "whole" } else { "interactive" };
        Some(Block::by(format!("held for {what} {} (expires in {} min)", h.id, (h.until - self.clock).div_ceil(60_000)), &h.id))
    }

    fn hot_wait(&self, name: &str, l: &Lease, holds: &Holds) -> Option<Block> {
        if l.req.top() || !l.req.on_node() || l.cost_class != CostClass::Medium || self.nodes[name].caps.as_ref()?.role() != Role::Hot { return None; }
        // An existing medium reservation cannot justify itself by hiding new small demand.
        let mut demand = holds.clone();
        demand.retain(|_, h| h.id != l.id);
        // Only small work that can actually use this box counts. A capped, held or incompatible
        // request cannot keep medium work off an otherwise usable hot box.
        let small: Vec<_> = self.leases.iter().filter(|r| r.state == State::Queued && r.cost_class == CostClass::Small).filter(|r| {
            self.candidates(r, &demand).is_ok_and(|names| names.iter().any(|n| n.as_str() == name)) && self.now_fits(name, r, &self.cost_on(name, r), &demand).is_ok()
        }).map(|r| r.id.clone()).collect();
        if small.is_empty() { return None; }
        Some(Block { reason: format!("class medium: hot box takes small first ({} small waiting)", small.len()), blockers: small, disk: false })
    }

    /// Can the node take it now? The same condition supplies the CLI's single reason per box.
    fn now_fits(&self, name: &str, l: &Lease, cost: &Cost, holds: &Holds) -> Result<f64, Block> {
        self.now_fits_as(name, l, cost, holds, false)
    }

    fn now_fits_as(&self, name: &str, l: &Lease, cost: &Cost, holds: &Holds, past_cap: bool) -> Result<f64, Block> {
        self.ready_as(name, l, cost, past_cap)?;
        if let Some(b) = self.timed_wait(name, l) { return Err(b); }
        if let Some(b) = self.hot_wait(name, l, holds).or_else(|| self.held_by(name, l, cost, holds)) { return Err(b); }
        self.resources(name, l, cost, true)?;
        let idle = self.nodes[name].released_ms.map_or(3_600_000, |r| self.clock.saturating_sub(r).min(3_600_000));
        Ok(idle as f64 / 36_000_000.0)
    }

    /// Will a lease named to `name` wait long there: held for an exclusive lease, a conflicting lease
    /// running, off or down?
    fn long_wait(&self, name: &str, l: &Lease, holds: &Holds) -> bool {
        let n = &self.nodes[name];
        let cost = self.cost_on(name, l);
        !n.up || !self.dev(name) || self.user_present(name) || n.caps.as_ref().is_some_and(|c| self.never(name, c, l, &cost).is_err() || self.now_fits(name, l, &cost, holds).is_err())
    }

    fn candidates<'a>(&'a self, l: &Lease, holds: &Holds) -> Result<Vec<&'a String>, Place> {
        let names: Vec<&String> = match &l.req.node {
            Some(n) => match self.nodes.get_key_value(n) {
                Some((k, st)) => {
                    // `--only` on a hot box that refuses the class never places: say so now, not after a wait
                    if l.req.only && st.caps.as_ref().is_some_and(|c| hot_refuses(c.role(), l)) {
                        return Err(Place::Deny(format!("--only {k}: a hot box takes small leases (and medium test/screen), this one is class {} ({}); pin a pool node, or lower --est-min/--cost if the job is small", l.cost_class.as_str(), l.cost_source.label())));
                    }
                    let mut v = vec![k];
                    // a soft pin: a twin (same arch and os) may take it once N kept it waiting
                    // past the grace, or at once when N is held, busy, off or down
                    if soft_pin(&l.req) && (self.clock.saturating_sub(l.asked_ms) >= PIN_GRACE_MS || self.long_wait(k, l, holds)) {
                        if let Some(nc) = &st.caps {
                            let twin = |c: &Caps| c.arch == nc.arch && c.os.eq_ignore_ascii_case(&nc.os);
                            let free = |m: &String| self.excluded.get(&(l.req.lane.clone(), m.clone())).is_none_or(|until| *until <= self.clock);
                            v.extend(self.nodes.iter().filter(|(m, o)| *m != k && o.caps.as_ref().is_some_and(twin) && free(m)).map(|(m, _)| m));
                        }
                    }
                    v
                }
                None if self.clock < self.nodes_settle_ms => return Err(Place::Wait(format!("node {n} not heard yet"), None, None, vec![n.clone()])),
                None => return Err(Place::Deny(format!("no node {n}"))),
            },
            None => {
                let all: Vec<&String> = self.nodes.keys().collect();
                let ok: Vec<&String> = all.iter().copied().filter(|n| self.excluded.get(&(l.req.lane.clone(), (*n).clone())).is_none_or(|until| *until <= self.clock)).collect();
                if ok.is_empty() && !all.is_empty() {
                    return Err(Place::Wait("every node is one you cancelled this lane on (name a node to override)".into(), None, None, all.into_iter().cloned().collect()));
                }
                ok
            }
        };
        Ok(names)
    }

    /// No transitive reservations, including through another candidate box.
    fn can_hold(&self, name: &str, l: &Lease, holds: &Holds) -> bool {
        (l.req.kind == Kind::Timed || !l.hold_until.is_some_and(|until| until <= self.clock)) && self.can_wait(name, l, holds)
    }

    /// A reservation's end. A timed request's lasts while it waits (renewed each tick): its box
    /// drains for it, taking only work that fits beside it or ends first. One that expired after
    /// ten minutes let new leases in and the box never drained (workpool waited 23 min on a pool box
    /// while it granted six, 11-02). Interactive demand still comes first ([`Self::timed_wait`]).
    fn hold_end(&self, l: &Lease) -> u64 {
        if l.req.kind == Kind::Timed { self.clock + HOLD_MS } else { l.hold_until.unwrap_or(self.clock + HOLD_MS) }
    }

    fn can_wait(&self, name: &str, l: &Lease, holds: &Holds) -> bool {
        if l.state != State::Queued || !(l.req.exclusive() || l.priority == Priority::Interactive) { return false; }
        if self.timed_wait(name, l).is_some() { return false; }
        let Ok(names) = self.candidates(l, holds) else { return false };
        names.iter().any(|n| n.as_str() == name)
            && names.iter().all(|n| self.held_by(n, l, &self.cost_on(n, l), holds).is_none())
            && self.ready(name, l, &self.cost_on(name, l)).is_ok()
            && self.hot_wait(name, l, holds).is_none()
    }

    /// The same best waiting box as placement, even after its reservation window expired.
    fn waiting_box(&self, l: &Lease, holds: &Holds, reserve: bool) -> Option<String> {
        let names = self.candidates(l, holds).ok()?;
        names.into_iter().filter(|name| holds.get(name.as_str()).is_none_or(|h| h.id == l.id)
            && if reserve { self.can_hold(name, l, holds) } else { self.can_wait(name, l, holds) }).filter(|name| {
            let mut next = holds.clone();
            next.retain(|_, h| h.id != l.id);
            next.insert((*name).clone(), Hold { id: l.id.clone(), until: self.hold_end(l) });
            next.iter().all(|(n, h)| h.id == l.id || self.get(&h.id).is_some_and(|r| self.can_hold(n, r, &next)))
        }).min_by_key(|name| (l.req.kind == Kind::Timed && self.has_live(name), self.remaining_ms(name, l, &self.cost_on(name, l)), self.sticky.get(&l.req.lane) != Some(*name))).cloned()
    }

    /// A live session runs on the box (it stays up until parked: timed work waits elsewhere first).
    fn has_live(&self, name: &str) -> bool {
        self.leases.iter().any(|r| r.state.live() && r.node.as_deref() == Some(name) && self.is_live_job(r))
    }

    /// A lease a live job holds: flagged in its request, or known to the live subsystem.
    fn is_live_job(&self, l: &Lease) -> bool {
        l.req.live || self.live_jobs.contains(&l.id)
    }

    fn park_live(&self, l: &Lease) -> Option<Action> {
        if !l.req.exclusive() || self.clock.saturating_sub(l.asked_ms) < LIVE_PARK_MS { return None; }
        let name = self.waiting_box(l, &self.holds, false)?;
        let cost = self.cost_on(&name, l);
        let mut leases = Vec::new();
        // why-queued reports the first failed condition. Removing it may reveal another
        // conflict or budget blocker; all of them must be live jobs on this box.
        while let Err(b) = self.resources_without(&name, l, &cost, true, &leases) {
            if b.blockers.is_empty() || b.blockers.iter().any(|id| self.get(id).is_none_or(|r| !r.state.live() || !self.is_live_job(r) || r.node.as_deref() != Some(name.as_str())) || leases.contains(id)) { return None; }
            leases.extend(b.blockers);
        }
        (!leases.is_empty()).then(|| Action::ParkLive { leases, note: format!("parked for {} {}", if l.req.kind == Kind::Timed { "timed" } else { "whole" }, l.id) })
    }

    fn valid_holds(&self, mut holds: Holds) -> Holds {
        loop {
            let invalid: Vec<_> = holds.iter().filter(|(name, h)| h.until <= self.clock || self.get(&h.id).is_none_or(|l| !self.can_hold(name, l, &holds))).map(|(name, _)| name.clone()).collect();
            if invalid.is_empty() { return holds; }
            for name in invalid { holds.remove(&name); }
        }
    }

    /// `wanted`: the boxes leases within their lane cap wait for this round (consulted only for a
    /// lease past its cap, which gives way there).
    fn place(&self, l: &Lease, holds: &Holds, wanted: &HashSet<String>) -> Place {
        // a lane runs one timed lease at a time: its cases go in one run, not in parallel requests
        if l.req.kind == Kind::Timed {
            if let Some(o) = self.leases.iter().find(|r| r.id != l.id && r.req.kind == Kind::Timed && r.req.lane == l.req.lane && (r.state.live() || r.state == State::Queued && (r.asked_ms, r.seq) < (l.asked_ms, l.seq))) {
                return Place::Deny(format!("lane {} already has timed lease {} ({}): one timed lease per lane; put all its cases in that run, or ask again when it ends", l.req.lane, o.id, o.state.as_str()));
            }
        }
        let over = self.lane_block(l);
        let names = match self.candidates(l, holds) { Ok(names) => names, Err(p) => return p };
        // (reason, the boxes that give it)
        let mut never: Vec<(String, Vec<&str>)> = Vec::new();
        let mut fit = Vec::new();
        let mut unknown = Vec::new();
        for name in names {
            let cost = self.cost_on(name, l);
            match &self.nodes[name].caps {
                None => unknown.push(name.as_str()),
                Some(c) => match self.never(name, c, l, &cost) {
                    Ok(()) => fit.push((name, cost)),
                    Err(e) => match never.iter_mut().find(|(r, _)| *r == e) {
                        Some((_, boxes)) => boxes.push(name),
                        None => never.push((e, vec![name])),
                    },
                },
            }
        }
        if fit.is_empty() {
            let said = || never.iter().map(|(e, boxes)| if l.req.node.is_some() { e.clone() } else { format!("{}: {e}", boxes.join(" ")) }).collect::<Vec<_>>().join("; ");
            return match (unknown.is_empty(), &l.req.node) {
                (true, _) if never.is_empty() && self.clock < self.nodes_settle_ms => Place::Wait("no node heard yet".into(), None, None, Vec::new()),
                (true, _) if never.is_empty() => Place::Deny("no nodes".into()),
                (true, _) => Place::Deny(said()),
                (false, Some(n)) if self.clock < self.nodes_settle_ms => Place::Wait(format!("node {n} not heard yet"), None, None, vec![n.clone()]),
                (false, Some(n)) => Place::Wait(format!("node {n} down"), None, None, vec![n.clone()]),
                (false, None) if self.clock < self.nodes_settle_ms || never.is_empty() => Place::Wait("no fitting node up".into(), None, None, self.nodes.keys().cloned().collect()),
                // every box that reported in refuses it for good; one that never did (a device
                // that is off) is no reason to wait forever (an arm64 test waited on a headset
                // that was off, 11-02): say what to ask instead
                (false, None) => Place::Deny(format!("{}; {} never reported in", said(), unknown.join(" "))),
            };
        }
        let sticky = self.sticky.get(&l.req.lane);
        let benches = if l.req.kind == Kind::Timed { self.bench_nodes() } else { Vec::new() };
        let pin = l.req.rev.as_deref().and_then(Pin::of_rev);
        // Work other than timed takes a bench box last among those with room: one held by a live
        // session, a gate or a test is one more a timed lease waits for (a live session held one
        // for 8 min while pool boxes had room and a timed lease queued, 11-02)
        let spares_bench = l.req.kind != Kind::Timed && l.req.node.is_none();
        let mut best: Option<(&String, Cost, u64, f64, bool, bool, f64, bool)> = None;
        let loader = l.req.on_node() && (l.req.loader.as_deref().is_some_and(|id| !id.is_empty()) || !l.req.hint.is_empty());
        let mut why: Vec<(String, String, bool)> = Vec::new();
        for (name, cost) in &fit {
            if over.is_some() && wanted.contains(name.as_str()) {
                why.push(((*name).clone(), "a lease within its lane cap waits for it".into(), false));
                continue;
            }
            match self.now_fits_as(name, l, cost, holds, over.is_some()) {
                Ok(score) => {
                    // the named node when it can take it; else the lane's warm node while it has
                    // room (WARM_BUSY), else the least busy box; a timed lease the first bench box
                    let named = l.req.node.as_deref() == Some(name.as_str());
                    let busy = self.busy(name);
                    let room = busy < WARM_BUSY;
                    let warm = room && self.warm_on(name, l);
                    let at_pin = room && l.req.src == Src::Tree && pin.as_ref().is_some_and(|p| self.nodes[*name].caps.as_ref().filter(|c| !l.req.native(c.role())).and_then(|c| Pin::parse(&c.tree)).is_some_and(|t| t.fits(p)));
                    let hot = self.nodes[*name].caps.as_ref().is_some_and(|c| c.role() == Role::Hot) && l.cost_class == CostClass::Small;
                    // one warm for the lane (its native tree, its warmed JIT cache) is no spare:
                    // a cold box costs the agent its build again
                    let on_bench = spares_bench && !warm && self.nodes[*name].caps.as_ref().is_some_and(|c| c.role() == Role::Bench);
                    let bench = benches.iter().position(|b| *b == name.as_str()).map_or(0.0, |i| 10.0 * (benches.len() - i) as f64);
                    let score = score + (1.0 - busy.min(1.0)) + bench + if named { 100.0 } else { 0.0 } + if warm { WARM_BONUS } else { 0.0 } + if room && sticky == Some(*name) { STICKY_BONUS } else { 0.0 } + if at_pin { TREE_BONUS } else { 0.0 };
                    let eta = if loader { self.finish_ms(name, l, 0) } else { 0 };
                    let cold = loader && !self.has_loader(name, l);
                    // a live session leaves a box a timed request waits for to the last
                    let timed = self.is_live_job(l) && self.leases.iter().any(|r| r.state == State::Queued && r.req.kind == Kind::Timed && (r.req.node.as_deref() == Some(name.as_str()) || holds.get(name.as_str()).is_some_and(|h| h.id == r.id)));
                    // Boxes with room first (a full one only when every box is: the least busy),
                    // except that a full box holding the dev JIT still beats building it on one
                    // with room, unless its cores are oversubscribed; then ready JITs; cold boxes
                    // by ETA, then hot. Otherwise hot, then ETA. Score breaks remaining ties.
                    // A bench box comes after every other ready one with room.
                    if best.as_ref().is_none_or(|b| {
                        if timed != b.5 { return !timed; }
                        let (full, other_full) = (!room && !named, b.6 >= WARM_BUSY && l.req.node.as_deref() != Some(b.0.as_str()));
                        let jit_first = cold != b.4 && if cold { b.6 < 1.0 } else { busy < 1.0 };
                        if full != other_full && !jit_first { return !full; }
                        if full && other_full { return busy < b.6; }
                        let other_hot = self.nodes[b.0].caps.as_ref().is_some_and(|c| c.role() == Role::Hot) && l.cost_class == CostClass::Small;
                        if cold != b.4 { return !cold; }
                        if on_bench != b.7 { return !on_bench; }
                        if cold { eta < b.2 || eta == b.2 && (hot && !other_hot || hot == other_hot && score > b.3) }
                        else { hot && !other_hot || hot == other_hot && (eta < b.2 || eta == b.2 && score > b.3) }
                    }) {
                        best = Some((name, *cost, eta, score, cold, timed, busy, on_bench));
                    }
                }
                Err(b) => why.push(((*name).clone(), b.reason, b.disk)),
            }
        }
        if let Some((name, cost, _, _, cold, ..)) = best {
            if let Some(wait) = (cold && over.is_none()).then(|| self.jit_box(l, &fit, holds, name)).flatten() { return wait; }
            return Place::Grant(name.clone(), cost);
        }
        // past its cap it gives way: on the boxes others wait for, or wherever there is no room
        if let Some(b) = &over {
            let gave: Vec<String> = why.iter().filter(|w| w.1.starts_with("a lease within its lane cap")).map(|w| w.0.clone()).collect();
            if !gave.is_empty() {
                return Place::Wait(format!("{}: other lanes' leases wait for {}", b.reason, gave.join(" ")), None, None, gave);
            }
        }
        // Reserve at most one box, only if running work is the remaining obstacle. ETA chooses
        // the first release, never a faster finish on a box that cannot currently take the job.
        let hold = if over.is_some() { None } else { self.waiting_box(l, holds, true) };
        let first = why.iter().find(|w| Some(&w.0) == hold.as_ref()).or_else(|| why.iter().find(|w| Some(&w.0) == sticky)).unwrap_or(&why[0]);
        let reason = if fit.len() == 1 { first.1.clone() } else { format!("{}: {} (+{} nodes)", first.0, first.1, fit.len() - 1) };
        let reclaim = why.iter().find(|w| w.2).map(|w| w.0.clone());
        let key = super::prof::blocker(&first.1);
        let boxes = why.iter().filter(|w| super::prof::blocker(&w.1) == key).map(|w| w.0.clone()).collect();
        Place::Wait(reason, hold, reclaim, boxes)
    }

    /// A box that holds the lease's dev JIT, kept off only by running work that ends (by
    /// measure) before a JIT build on `cold`, the box without it that could take it now, would be
    /// done: the lease waits for it. All such waiting stays under one build, so an estimate that
    /// runs over costs at most what building would have (older landed inputs built on a box
    /// without the id 2-3 min while a box with it freed sooner, 11-02). Named and exclusive
    /// requests keep their own placement.
    fn jit_box(&self, l: &Lease, fit: &[(&String, Cost)], holds: &Holds, cold: &str) -> Option<Place> {
        if l.req.node.is_some() || l.req.exclusive() { return None; }
        let build = self.finish_ms(cold, l, 0).saturating_sub(self.run_ms(&l.req, cold));
        let waited = self.clock.saturating_sub(l.jit_wait_since.unwrap_or(self.clock));
        let (name, wait) = fit.iter().filter(|(n, _)| n.as_str() != cold && self.has_loader(n, l)).filter_map(|(n, cost)| {
            if self.ready(n, l, cost).is_err() || self.timed_wait(n, l).is_some() || self.hot_wait(n, l, holds).is_some() || self.held_by(n, l, cost, holds).is_some() { return None; }
            let b = self.resources(n, l, cost, true).err()?;
            if b.disk || b.blockers.is_empty() { return None; }
            let wait = self.remaining_ms(n, l, cost);
            (wait > 0 && waited + wait < build && self.finish_ms(n, l, wait) < self.finish_ms(cold, l, 0)).then_some((*n, wait))
        }).min_by_key(|(_, wait)| *wait)?;
        Some(Place::Wait(format!("{name}: {JIT_WAIT}: frees in {} by measure, a build on {cold} takes ~{}", dur(wait), dur(build)), None, None, vec![name.clone()]))
    }

    fn queue_order(&self) -> Vec<usize> {
        let mut order: Vec<_> = (0..self.leases.len()).filter(|&i| self.leases[i].state == State::Queued).collect();
        order.sort_by_key(|&i| {
            let l = &self.leases[i];
            let landing = l.req.kind == Kind::Gate && (l.req.lane == LANDING_LANE || l.req.lane.starts_with("landq"));
            let aged = l.req.prio as i64 - (self.clock.saturating_sub(l.asked_ms) / AGE_STEP_MS) as i64;
            // In a quiet window exclusive work (timed and whole-box) drains in arrival order,
            // before any shared work: the head gets the box when it frees, no newer request
            // overtakes it (a bump goes first). Interactive demand blocks timed placement and
            // holds, never a running lease.
            // past its lane cap: after every lease within one (it takes only what they leave)
            let over = self.lane_block(l).is_some();
            if l.req.exclusive() { let o = Self::exclusive_order(l); return (!l.req.top(), over, l.req.ci(), false, false, false, false, if o.0 { 0 } else { -1 }, u64::from(o.1), o.2, o.3); }
            (!l.req.top(), over, l.req.ci(), true, l.req.kind != Kind::Gate, !landing, l.priority != Priority::Interactive, aged, self.last_grant.get(&l.req.lane).map_or(0, |t| t + 1), l.asked_ms, l.seq)
        });
        order
    }

    /// Read-only scheduling explanation: one current failed condition and its blockers per box.
    pub fn why_queued(&mut self, id: &str, now: u64) -> Result<Vec<String>, String> {
        self.clock = now;
        self.refresh_interactive();
        let holds = self.valid_holds(self.holds.clone());
        let l = self.get(id).ok_or_else(|| format!("no lease {id}"))?;
        if l.state != State::Queued { return Err(format!("{id} is {}", l.state.as_str())); }
        let names = match self.candidates(l, &holds) {
            Ok(names) => names,
            Err(Place::Wait(why, _, _, _) | Place::Deny(why)) => return Err(why),
            Err(Place::Grant(_, _)) => unreachable!(),
        };
        Ok(names.into_iter().map(|name| {
            let b = self.now_fits(name, l, &self.cost_on(name, l), &holds).err().unwrap_or_else(|| Block::new("fits now"));
            let cost = self.cost_on(name, l);
            format!("{{\"id\":{},\"node\":{},\"reason\":{},\"blockers\":[{}],\"ram_gb\":{},\"cores\":{},\"allocation\":{}}}", json_str(id), json_str(name), json_str(&b.reason), b.blockers.iter().map(|id| json_str(id)).collect::<Vec<_>>().join(","), cost.ram_gb, cost.cores, json_str(&self.allocation_note(name, l)))
        }).collect())
    }

    /// Expires silent holders, places what fits, recomputes positions and reasons; the actions
    /// for the master (grants, ends, reclaims, one event per change).
    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        { let live: HashSet<&String> = self.leases.iter().filter(|l| l.state.live()).map(|l| &l.id).collect(); self.live_idle.retain(|id| live.contains(id)); self.stalled.retain(|id| live.contains(id)); }
        self.say_stalled(now);
        // Close the previous tick's wait interval once, before placement can grant or end it.
        for l in &mut self.leases { l.sample_queue(now); }
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
        for l in self.leases.iter_mut().filter(|l| l.state == State::Queued) {
            l.priority = if self.interactive_lanes.contains(&l.req.lane) { Priority::Interactive } else { l.req.priority };
        }
        for i in 0..self.leases.len() {
            if self.leases[i].state != State::Queued { continue; }
            let (class, source) = self.classify(&self.leases[i].req);
            self.leases[i].cost_class = class;
            self.leases[i].cost_source = source;
        }
        let mut holds = std::mem::take(&mut self.holds);
        let mut deny = Vec::new();
        let mut waits: Vec<(usize, String, Vec<String>)> = Vec::new();
        loop {
            self.refresh_interactive();
            holds = self.valid_holds(holds);
            deny.clear();
            waits.clear();
            let mut changed = false;
            for i in self.queue_order() {
                // past its lane cap a lease comes after every one within its cap (queue_order):
                // the boxes those wait for this round are theirs
                let wanted: HashSet<String> = if self.lane_block(&self.leases[i]).is_some() {
                    waits.iter().filter(|(j, _, _)| self.lane_block(&self.leases[*j]).is_none()).flat_map(|(_, _, b)| b.iter().cloned()).collect()
                } else { HashSet::new() };
                match self.place(&self.leases[i], &holds, &wanted) {
                    Place::Grant(node, cost) => {
                        let l = &mut self.leases[i];
                        l.state = State::Starting;
                        l.node = Some(node.clone());
                        l.cost = cost;
                        l.granted_ms = Some(now);
                        l.reason.clear();
                        l.pos = 0;
                        let released = self.nodes.get(&node).and_then(|n| n.released_ms);
                        if let Some(r) = released.filter(|r| l.asked_ms <= *r && l.req.kind != Kind::Inspect) {
                            l.wait_ms = Some(now.saturating_sub(r));
                            if let Some(n) = self.nodes.get_mut(&node) {
                                Lat::push(&mut n.lat.handoff, now.saturating_sub(r));
                            }
                        }
                        let (lane, kind, id) = (l.req.lane.clone(), l.req.kind, l.id.clone());
                        self.last_grant.insert(lane.clone(), now);
                        if buildish(kind) {
                            self.sticky.insert(lane.clone(), node.clone());
                            self.warm.insert((lane, node.clone()));
                        }
                        let note = self.leases[i].take_class_note();
                        let l = self.leases[i].clone();
                        let moved = l.req.node.as_deref().filter(|n| *n != node).map_or(String::new(), |n| format!(" (on twin {node}: {n} was busy)"));
                        self.event(Sev::Info, format!("{} granted to {} after {}{moved}{note}", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms))), Some(&l));
                        holds.retain(|_, h| h.id != id);
                        self.out.push(Action::Grant { id, node });
                        changed = true;
                        // A grant changes fair share, caps, hot-box demand and reservations. Revisit
                        // the queue immediately, including requests skipped earlier in this tick.
                        break;
                    }
                    Place::Wait(reason, hold, reclaim, boxes) => {
                        if reason.contains(JIT_WAIT) { self.leases[i].jit_wait_since.get_or_insert(now); }
                        if let Some(node) = hold {
                            let until = self.hold_end(&self.leases[i]);
                            let l = &mut self.leases[i];
                            l.hold_until = Some(until);
                            changed |= holds.get(&node).is_none_or(|h| h.id != l.id);
                            holds.retain(|_, h| h.id != l.id);
                            holds.insert(node, Hold { id: l.id.clone(), until });
                        }
                        if let Some(n) = reclaim {
                            let due = self.nodes.get(&n).is_some_and(|s| s.reclaim_ms.is_none_or(|t| now.saturating_sub(t) >= RECLAIM_EVERY_MS));
                            if due {
                                self.nodes.get_mut(&n).unwrap().reclaim_ms = Some(now);
                                let need_gb = self.leases[i].cost.disk_gb;
                                let line = format!("node {n} short of disk for {} ({need_gb:.0} GB over its floor): asked to reclaim", self.leases[i].id);
                                self.event(Sev::Info, line, None);
                                self.out.push(Action::Reclaim { node: n, need_gb });
                            }
                        }
                        waits.push((i, reason, boxes));
                    }
                    Place::Deny(reason) => deny.push((self.leases[i].id.clone(), reason)),
                }
            }
            if !changed { break; }
        }
        self.holds = holds;
        for i in self.queue_order() {
            if let Some(a) = self.park_live(&self.leases[i]) { self.out.push(a); }
        }
        // a box whose older hub restarts only between runs: its live sessions park once their
        // call answers (an idle one held a bench box 22 min after its verdict, 11-02)
        let mut drains: Vec<&String> = self.update_drain.iter().collect();
        drains.sort();
        let parks: Vec<Action> = drains.into_iter().filter_map(|n| {
            let leases: Vec<String> = self.leases.iter().filter(|r| r.state.live() && r.node.as_deref() == Some(n.as_str()) && self.is_live_job(r)).map(|r| r.id.clone()).collect();
            (!leases.is_empty()).then(|| Action::ParkLive { leases, note: format!("parked: {n}'s older hub restarts for its update") })
        }).collect();
        self.out.extend(parks);
        // positions among the waiting leases of a kind, in queue order
        let mut per_kind: HashMap<Kind, usize> = HashMap::new();
        let denied: HashSet<&String> = deny.iter().map(|(id, _)| id).collect();
        let mut changes = Vec::new();
        for (i, reason, boxes) in waits {
            if denied.contains(&self.leases[i].id) {
                continue;
            }
            let p = per_kind.entry(self.leases[i].req.kind).or_insert(0);
            *p += 1;
            let eta = if self.leases[i].req.exclusive() { self.eta_ms(&self.leases[i]) } else { 0 };
            let l = &mut self.leases[i];
            l.eta_ms = eta;
            // Keep attribution from the final pass, even when the displayed reason is unchanged.
            l.queue_boxes = boxes;
            let warn = !l.warned && now.saturating_sub(l.asked_ms) >= WAIT_WARN_MS;
            if l.pos != *p || l.reason != reason || warn || (!l.class_notified && !l.class_note().is_empty()) {
                l.pos = *p;
                l.reason = reason;
                l.warned |= warn;
                changes.push((i, warn));
            }
        }
        for (i, warn) in changes {
            let note = self.leases[i].take_class_note();
            let l = self.leases[i].clone();
            let line = if warn {
                format!("{} for {} waiting {}: {} in queue ({})", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms)), ordinal(l.pos), l.reason)
            } else {
                format!("{} for {}: {} in queue ({})", l.tag(), l.req.lane, ordinal(l.pos), l.reason)
            };
            self.event(if warn { Sev::Warn } else { Sev::Info }, format!("{line}{note}"), Some(&l));
        }
        for (id, reason) in deny {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "denied", Sev::Warn, &reason, now);
            }
        }
        let boxes = self.box_samples();
        let at = self.profile_at.unwrap_or(now);
        let sample = |boxes: &[String], at| -> String {
            if now <= at { return String::new(); }
            boxes.iter().map(|b| format!("{{\"start_ms\":{at},\"end_ms\":{now},{b}}}\n")).collect()
        };
        let closed = if boxes != self.profile_boxes { sample(&self.profile_boxes, at) } else { String::new() };
        let at = if boxes != self.profile_boxes { now } else { at };
        self.profile_at = Some(at);
        self.profile_seq += 1;
        self.out.push(Action::Profile { closed, live: sample(&boxes, at), seq: self.profile_seq });
        self.profile_boxes = boxes;
        std::mem::take(&mut self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_median_keeps_the_last_nine_samples() {
        let mut s = Sched::new("mac", 1);
        assert_eq!(s.class_run_median("test:pkg", "test"), None, "no run sample yet");
        s.record_run("test:pkg", "test", 90);
        s.record_run("test:pkg", "test", 10);
        assert_eq!(s.class_run_median("test:pkg", "test"), Some(50), "even median");
        for ms in 1..=10 { s.record_run("test:pkg", "test", ms * 10); }
        assert_eq!(s.class_run_median("test:pkg", "test"), Some(60), "only the last nine runs count");
        assert_eq!(s.runs.get(&("test:pkg".into(), "test".into())).unwrap().len(), 9, "bounded history");
        assert_eq!(s.class_median("test:pkg", "test"), None, "run samples do not change scheduling walls");
    }

    fn caps(cores: u32, ram: f64, role: Role) -> Caps {
        Caps {
            hub_proto: super::super::HUB_PROTO,
            native_trees: super::super::caps::NATIVE_TREES,
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
            gpus: vec![("GPU-A".into(), 24.0)],
            ..Default::default()
        }
    }

    fn sched(nodes: &[(&str, Caps)]) -> Sched {
        let mut s = Sched::new("mac", 1);
        for (n, c) in nodes {
            s.add_node(n, "192.0.2.1", None);
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

    /// A release and the node's cleanup answer (the master's `end` calls [`Sched::cleaned`]).
    fn release_clean(s: &mut Sched, id: &str, now: u64) {
        let node = s.get(id).and_then(|l| l.node.clone());
        s.release(id, "released", now);
        if let Some(n) = node {
            s.cleaned(&n, id, 0);
        }
    }

    fn beat_all(s: &mut Sched, now: u64) {
        let ids: Vec<String> = s.leases.iter().map(|l| l.id.clone()).collect();
        for id in ids {
            s.beat(&id, now);
        }
    }

    /// A live job's lease parks for waiting timed work. Live jobs run at interactive priority,
    /// and a running interactive lease used to keep its box in `interactive_boxes`, which made
    /// the timed lease wait for a quiet window instead of parking the job (a live job held a box
    /// for 92 min on 11-01). Also: the live subsystem knows its leases, so one granted before
    /// the request carried `live` (or restored without it) parks too.
    #[test]
    fn a_live_jobs_lease_parks_for_timed_work_even_without_the_flag() {
        let parks = |known: bool, flag: bool| {
            let mut s = sched(&[("a", caps(64, 256.0, Role::Pool))]);
            let mut live = req("live-W1", Kind::Test, Some("a"));
            live.priority = Priority::Interactive;
            live.live = flag;
            let live = s.request(live, 0);
            s.tick(0);
            s.started(&live, "{}");
            if known { s.live_jobs.insert(live.clone()); }
            let _timed = s.request(req("bench", Kind::Timed, Some("a")), 1);
            s.tick(1);
            beat_all(&mut s, LIVE_PARK_MS + 10);
            s.tick(LIVE_PARK_MS + 10).iter().any(|a| matches!(a, Action::ParkLive { leases, .. } if leases == &vec![live.clone()]))
        };
        assert!(parks(false, true), "the flagged lease parks (the scenario is a real park)");
        assert!(!parks(false, false), "a plain test lease is never parked");
        assert!(parks(true, false), "a lease the live subsystem holds parks for the waiting timed lease");
    }

    /// A restart takes back the queue and the leases a node was setting up, with their ids.
    #[test]
    fn a_restart_takes_back_queued_and_starting_leases() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool))]);
        let starting = s.request(req("one", Kind::Build, Some("a")), 0);
        s.tick(0);
        assert_eq!(state(&s, &starting).0, "starting");
        let granted = s.request(req("two", Kind::Test, Some("a")), 1);
        s.tick(1);
        s.started(&starting, "{}");
        let mut waits = req("three", Kind::Timed, Some("a"));
        waits.since_ms = Some(5);
        let queued = s.request(waits, 2);
        s.tick(2);
        s.started(&granted, "{}");
        // (whatever state each is in, the lines carry it)
        let before: Vec<(String, String)> = [&starting, &granted, &queued].iter().map(|id| ((*id).clone(), state(&s, id).0)).collect();
        assert!(before.iter().any(|(_, st)| st == "queued"), "{before:?}");
        let lines = s.live_lines();
        assert_eq!(lines.len(), 3, "granted, starting and queued all persist: {lines:?}");
        let mut s2 = sched(&[("a", caps(64, 256.0, Role::Pool))]);
        for l in &lines { s2.restore(l, 100_000); }
        for (id, st) in &before {
            assert_eq!(&state(&s2, id).0, st, "{id} comes back as it was");
        }
        assert_eq!(s2.get(&queued).unwrap().asked_ms, 2.min(s.get(&queued).unwrap().asked_ms));
        let fresh = s2.request(req("new", Kind::Test, Some("a")), 100_001);
        assert!(![&starting, &granted, &queued].contains(&&fresh), "ids continue past the restored ones");
    }

    #[test]
    fn a_restart_drops_the_masters_own_queued_requests() {
        let mut s = sched(&[("a", caps(8, 32.0, Role::Pool))]);
        let mut master = req("one", Kind::Build, Some("a"));
        master.master = true;
        assert!(Req::parse(&master.to_json()).unwrap().master, "master round-trips");
        assert!(!Req::parse(r#"{"lane":"old","kind":"build"}"#).unwrap().master, "absent master defaults to false");
        let granted = s.request(master.clone(), 0);
        s.tick(0);
        assert!(s.started(&granted, "{}"), "the master lease starts");
        master.lane = "two".into();
        let queued = s.request(master, 1);
        let mut cli = req("three", Kind::Build, Some("a"));
        cli.holder = Some(4242);
        assert!(!Req::parse(&cli.to_json()).unwrap().master, "CLI requests round-trip without master");
        let cli = s.request(cli, 2);
        s.tick(2);
        assert_eq!(state(&s, &queued).0, "queued", "the master's second request waits");
        assert_eq!(state(&s, &cli).0, "queued", "the CLI request waits");
        let lines = s.live_lines();
        assert_eq!(lines.len(), 3, "all three requests persist");
        let mut s2 = sched(&[("a", caps(8, 32.0, Role::Pool))]);
        for l in &lines { s2.restore(l, 100_000); }
        assert_eq!(state(&s2, &granted).0, "granted", "the granted master lease comes back");
        assert_eq!(state(&s2, &cli).0, "queued", "the CLI request keeps its id and stays queued");
        assert!(s2.get(&queued).is_none(), "the master's dead queued request is gone");
    }

    /// A starting lease comes back to be set up again by its node (lease-start is idempotent).
    #[test]
    fn a_restored_starting_lease_is_started_again() {
        let mut s = sched(&[("a", caps(8, 32.0, Role::Pool))]);
        let id = s.request(req("one", Kind::Build, Some("a")), 0);
        s.tick(0);
        let line = s.live_lines().remove(0);
        let mut s2 = sched(&[("a", caps(8, 32.0, Role::Pool))]);
        s2.restore(&line, 10);
        assert_eq!(s2.resume, vec![(id.clone(), "a".to_string())]);
        assert!(s2.in_transition().is_some_and(|w| w.contains(&id)), "a master with a lease in setup does not exec");
        s2.started(&id, "{}");
        assert!(s2.in_transition().is_none());
    }

    /// A timed series pins a binary built from its worktree: nothing merges into the lane while
    /// a timed lease is queued or live there, nor soon after one ended.
    #[test]
    fn a_timed_series_keeps_merges_off_its_worktree() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool))]);
        let mut r = req("bench", Kind::Timed, Some("a"));
        r.repo_dir = Some("/wt/bench".into());
        let id = s.request(r, 0);
        s.tick(0);
        assert!(s.timed_series("/wt/bench", 1).is_some());
        assert!(s.timed_series("/wt/bench/apps/commercial", 1).is_some(), "the nested commercial half too");
        assert!(s.timed_series("/wt/other", 1).is_none());
        release_clean(&mut s, &id, 1_000);
        assert!(s.timed_series("/wt/bench", 2_000).is_some_and(|w| w.contains("between") || w.contains("ended")));
        assert!(s.timed_series("/wt/bench", 1_000 + TIMED_SERIES_MS + 1).is_none());
    }

    /// A long-cycle box takes no ci leases; native tree work goes anywhere it fits.
    #[test]
    fn long_boxes_take_no_ci() {
        let mut s = sched(&[("long", caps(16, 64.0, Role::Long))]);
        let mut c = req("ci", Kind::Ci, Some("long"));
        c.only = true;
        c.rev = Some(format!("work@{}", "c".repeat(40)));
        let c = s.request(c, 0);
        let mut g = req("land", Kind::Gate, None);
        g.rev = Some(format!("work@{}", "a".repeat(40)));
        let g = s.request(g, 0);
        assert_eq!(grants(&s.tick(0)), vec![g]);
        assert!(state(&s, &c).1.contains("no ci"), "{:?}", state(&s, &c));
    }

    /// Unpinned tree writers stay exclusive. Overlays share their box within resource budgets.
    #[test]
    fn tree_writers_wait_and_overlays_share_budgets() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        let ids: Vec<String> = (0..5).map(|i| s.request(req(&format!("lane{i}"), [Kind::Test, Kind::Build, Kind::Gate, Kind::Long, Kind::Screen][i], None), 0)).collect();
        let a = s.tick(0);
        assert_eq!(grants(&a).len(), 3);
        let mut nodes: Vec<String> = ids[..3].iter().map(|i| node_of(&s, i).unwrap()).collect();
        nodes.sort();
        assert_eq!(nodes, vec!["a", "b", "c"], "one tree writer per node");
        assert_eq!(state(&s, &ids[3]).0, "queued");
        assert!(state(&s, &ids[3]).1.contains("held by"), "{:?}", state(&s, &ids[3]));
        assert_eq!(s.get(&ids[4]).unwrap().pos, 1, "the only waiting screen lease");
        let freed = node_of(&s, &ids[1]).unwrap();
        release_clean(&mut s, &ids[1], 1000);
        beat_all(&mut s, 1000);
        let a = s.tick(1000);
        assert_eq!(grants(&a), vec![ids[3].clone()]);
        assert_eq!(node_of(&s, &ids[3]), Some(freed));
        // inspect leases do not hold a node
        let mut r = req("look", Kind::Inspect, Some("a"));
        r.jit = Some("sandbox".into());
        let i = s.request(r, 1001);
        s.tick(1001);
        assert_eq!(state(&s, &i).0, "starting");
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let tree = s.request(req("tree", Kind::Build, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![tree.clone()]);
        let overlay = |lane: &str| {
            let mut r = req(lane, Kind::Test, Some("a"));
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r
        };
        let a = s.request(overlay("one"), 1);
        let b = s.request(overlay("two"), 1);
        assert_eq!(grants(&s.tick(1)), vec![a.clone(), b.clone()]);
        let c = s.request(overlay("three"), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert_eq!(state(&s, &c).1, "budget cores 16/16 + 4");
        release_clean(&mut s, &a, 3);
        assert_eq!(grants(&s.tick(3)), vec![c]);
        let timed = s.request(req("timed", Kind::Timed, Some("a")), 4);
        assert!(grants(&s.tick(4)).is_empty());
        let d = s.request(overlay("four"), 5);
        s.tick(5);
        assert_eq!(state(&s, &d).1, format!("held for timed {timed} (the box drains for it: only work that ends first or fits beside it)"));

        // Pinned native writers use independent trees. A free old tree can be evicted even
        // when caps report capacity full; running and scrubbing trees cannot be evicted.
        let mut c = caps(64, 256.0, Role::Pool);
        c.native_trees = 2;
        c.native = vec!["old-a".into(), "old-b".into()];
        let mut s = sched(&[("a", c)]);
        let native = |lane: &str, pin: char| {
            let mut r = req(lane, Kind::Test, Some("a"));
            r.rev = Some(format!("work@{}", pin.to_string().repeat(40)));
            r
        };
        let a = s.request(native("one", 'a'), 0);
        let b = s.request(native("two", 'b'), 0);
        assert_eq!(grants(&s.tick(0)), vec![a.clone(), b]);
        let c = s.request(native("three", 'c'), 1);
        let same = s.request(native("one", 'a'), 1);
        assert_eq!(grants(&s.tick(1)), vec![same.clone()], "equal pins share one lane's tree");
        assert!(state(&s, &c).1.contains("native trees busy"));
        release_clean(&mut s, &same, 2);
        let changed = s.request(native("one", 'd'), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert!(state(&s, &changed).1.contains("held by"));
        s.release(&changed, "released", 2);
        s.release(&a, "released", 3);
        assert!(grants(&s.tick(3)).is_empty(), "cleanup still holds the native slot");
        s.cleaned("a", &a, 0);
        assert_eq!(grants(&s.tick(4)), vec![c]);
        invariants(&s);
    }

    #[test]
    fn ci_overlays_share_a_box_at_different_pins() {
        let mut s = sched(&[("106", caps(16, 64.0, Role::Pool))]);
        let mut tree = req("tree", Kind::Build, Some("106"));
        tree.rev = Some(format!("work@{}", "c".repeat(40)));
        let tree = s.request(tree, 0);
        assert_eq!(grants(&s.tick(0)), vec![tree.clone()]);
        let overlay = |sha: &str| {
            let mut r = req("ci", Kind::Ci, Some("106"));
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", sha.repeat(40)));
            Req::parse(&r.to_json()).unwrap()
        };
        let a = s.request(overlay("a"), 1);
        let b = s.request(overlay("b"), 1);
        assert_eq!(grants(&s.tick(1)), vec![a.clone(), b.clone()]);
        assert_eq!(node_of(&s, &a), node_of(&s, &b));
        let timed = s.request(req("timed", Kind::Timed, Some("106")), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert_eq!(state(&s, &timed).0, "queued");
        for id in [tree, a, b] { release_clean(&mut s, &id, 3); }
        assert_eq!(grants(&s.tick(3)), vec![timed]);
        let c = s.request(overlay("c"), 4);
        assert!(grants(&s.tick(4)).is_empty());
        assert_eq!(state(&s, &c).0, "queued");
    }

    #[test]
    fn ci_shares_a_box_with_one_other_lease() {
        for kind in [Kind::Build, Kind::Test, Kind::Screen, Kind::Long, Kind::Gpu, Kind::Gate, Kind::Inspect] {
            for first_ci in [false, true] {
                let mut s = sched(&[("106", caps(16, 64.0, Role::Pool))]);
                let (first, second) = if first_ci { (Kind::Ci, kind) } else { (kind, Kind::Ci) };
                let at = |lane: &str, kind: Kind, sha: char| {
                    let mut r = req(lane, kind, Some("106"));
                    r.rev = Some(format!("work@{}", sha.to_string().repeat(40)));
                    r
                };
                let a = s.request(at("a", first, 'a'), 0);
                assert_eq!(grants(&s.tick(0)), vec![a.clone()]);
                if kind != Kind::Inspect {
                    let off = s.request(at("off", second, 'b'), 1);
                    if kind == Kind::Gate {
                        assert!(grants(&s.tick(1)).is_empty(), "another pin cannot share the work tree");
                        assert_eq!(state(&s, &off).0, "queued");
                        s.release(&off, "released", 1);
                    } else {
                        assert_eq!(grants(&s.tick(1)), vec![off.clone()], "native lane and CI trees are independent");
                        release_clean(&mut s, &off, 1);
                    }
                }
                let b = s.request(at("b", second, 'a'), 1);
                assert_eq!(grants(&s.tick(1)), vec![b.clone()]);
                assert!(s.get(&a).unwrap().state.live() && s.get(&b).unwrap().state.live());
                assert_eq!(node_of(&s, &a), node_of(&s, &b));
            }
        }
    }

    #[test]
    fn ci_and_timed_never_share_a_box() {
        for first in [Kind::Ci, Kind::Timed] {
            let mut s = sched(&[("106", caps(16, 64.0, Role::Bench))]);
            let a = s.request(req("a", first, Some("106")), 0);
            s.tick(0);
            let b = s.request(req("b", if first == Kind::Ci { Kind::Timed } else { Kind::Ci }, Some("106")), 1);
            assert!(grants(&s.tick(1)).is_empty());
            assert_eq!(state(&s, &b).0, "queued");
            release_clean(&mut s, &a, 2);
            assert_eq!(grants(&s.tick(2)), vec![b]);
        }
    }

    #[test]
    fn a_timed_hold_keeps_ci_off_the_box() {
        let mut s = sched(&[("106", caps(16, 64.0, Role::Pool))]);
        let test = s.request(req("test", Kind::Test, Some("106")), 0);
        s.tick(0);
        let timed = s.request(req("bench", Kind::Timed, Some("106")), 1);
        s.tick(1);
        assert_eq!(s.holds.get("106").map(|h| &h.id), Some(&timed));
        let mut r = req("ci", Kind::Ci, Some("106"));
        r.prio = 0;
        let ci = s.request(r, 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert!(state(&s, &ci).1.contains("held for timed"));
        release_clean(&mut s, &test, 3);
        assert_eq!(grants(&s.tick(3)), vec![timed.clone()]);
        assert_eq!(state(&s, &ci).0, "queued");
        release_clean(&mut s, &timed, 4);
        assert_eq!(grants(&s.tick(4)), vec![ci]);
    }

    #[test]
    fn ci_never_uses_a_user_box_even_when_named_and_enabled() {
        for role in [Role::UserWm, Role::Device] {
            let mut c = caps(16, 64.0, role);
            c.slots = vec![(Kind::Ci, 1)];
            assert_eq!(slots(&c, Kind::Ci), 0);
            let mut s = sched(&[("106", c)]);
            s.set_dev("106", true, 0).unwrap();
            let ci = s.request(req("ci", Kind::Ci, Some("106")), 0);
            assert!(grants(&s.tick(0)).is_empty());
            assert_eq!(state(&s, &ci).0, "ended denied");
        }
    }

    #[test]
    fn cleanup_holds_only_the_tree_being_scrubbed() {
        let mut c = caps(16, 64.0, Role::Pool);
        c.slots = vec![(Kind::Ci, 3), (Kind::Test, 1)];
        let mut s = sched(&[("106", c)]);
        let at = |lane: &str, kind: Kind| {
            let mut r = req(lane, kind, Some("106"));
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r
        };
        let a = s.request(at("ci", Kind::Ci), 0);
        s.tick(0);
        let b = s.request(at("ci", Kind::Ci), 1);
        assert!(grants(&s.tick(1)).is_empty());
        assert_eq!(state(&s, &b).0, "queued");
        s.release(&a, "released", 2);
        let test = s.request(at("test", Kind::Test), 2);
        assert_eq!(grants(&s.tick(2)), vec![test.clone()]);
        assert!(state(&s, &b).1.contains("scrubbing its work tree"));
        s.cleaned("106", &a, 0);
        assert_eq!(grants(&s.tick(3)), vec![b.clone()]);
        s.release(&test, "released", 4);
        let next = s.request(at("next", Kind::Test), 4);
        let same = s.request(at("test", Kind::Test), 4);
        assert_eq!(grants(&s.tick(4)), vec![next]);
        assert!(state(&s, &same).1.contains("scrubbing its work tree"));
        s.cleaned("106", &test, 0);
        assert_eq!(grants(&s.tick(5)), vec![same]);
    }

    #[test]
    fn ci_prefers_the_work_trees_pair() {
        let pin = format!("{},{}", "a".repeat(40), "c".repeat(40));
        let other = format!("{},{}", "b".repeat(40), "d".repeat(40));
        let mut a = caps(16, 64.0, Role::Pool);
        a.tree = other.clone();
        a.deploy = pin.clone();
        let mut b = a.clone();
        b.tree = pin;
        b.deploy = other;
        let mut s = sched(&[("106", a), ("105", b)]);
        let mut r = req("ci", Kind::Ci, None);
        r.rev = Some(format!("work@{},main@{}", "a".repeat(40), "c".repeat(40)));
        let ci = s.request(r, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &ci).as_deref(), Some("105"));
    }

    #[test]
    fn ci_goes_only_to_boxes_whose_lease_conf_opts_in() {
        let mut off = caps(16, 64.0, Role::Pool);
        off.slots = super::super::caps::LeaseConf::parse("", std::path::Path::new("/h")).slots(0);
        let mut on = off.clone();
        on.slots = super::super::caps::LeaseConf::parse("slots.ci = 1\n", std::path::Path::new("/h")).slots(0);
        let mut s = sched(&[("a", off), ("b", on)]);
        let ci = s.request(req("ci", Kind::Ci, None), 0);
        assert_eq!(grants(&s.tick(0)), vec![ci.clone()]);
        assert_eq!(node_of(&s, &ci).as_deref(), Some("b"));
        let mut only = req("ci", Kind::Ci, Some("a"));
        only.only = true;
        let only = s.request(only, 1);
        s.tick(1);
        assert!(state(&s, &only).1.contains("takes no"), "{:?}", state(&s, &only));
    }

    #[test]
    fn ci_slots_do_not_require_a_display_but_gui_needs_do() {
        for role in [Role::Pool, Role::Bench] {
            let c = caps(16, 64.0, role);
            assert_eq!(slots(&c, Kind::Ci), 1);
            let mut s = sched(&[("106", c)]);
            let mut r = req("ci", Kind::Ci, Some("106"));
            r.needs = parse_needs("gui=1");
            let gui = s.request(r, 0);
            assert!(grants(&s.tick(0)).is_empty());
            assert_eq!(state(&s, &gui).0, "ended denied");
            let plain = s.request(req("ci", Kind::Ci, Some("106")), 1);
            assert_eq!(grants(&s.tick(1)), vec![plain]);
        }
    }

    /// A restarted master waits for nodes to report before it denies queued work.
    #[test]
    fn a_restarted_master_waits_for_its_nodes_before_denying() {
        let mut s = Sched::new("mac", 1);
        s.nodes_settle_ms = 60_000;
        let mut pinned = Req::new("a", Kind::Test);
        pinned.node = Some("n1".into());
        let (any, pin) = (s.request(Req::new("a", Kind::Test), 1), s.request(pinned, 1));
        s.tick(1);
        assert!(s.get(&any).is_some_and(|l| l.state == State::Queued) && s.get(&pin).is_some_and(|l| l.state == State::Queued));
        s.tick(60_001);
        assert!(s.get(&any).is_some_and(|l| matches!(l.state, State::Ended(_))) && s.get(&pin).is_some_and(|l| matches!(l.state, State::Ended(_))));
    }

    /// A master that restarted knows only granted leases: a holder asks again with when it first
    /// asked, and is where it was, ahead of one that asked after it.
    #[test]
    fn a_request_asked_again_after_a_restart_keeps_its_place() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let at = 500_000;
        let holder = s.request(Req::new("x", Kind::Test), at);
        beat_all(&mut s, at);
        let _ = s.tick(at);
        assert_ne!(state(&s, &holder).0, "queued");
        let later = s.request(Req::new("later", Kind::Test), at + 100);
        let mut first = Req::new("first", Kind::Test);
        first.since_ms = Some(at - 400_000);
        let first = Req::parse(&first.to_json()).unwrap();
        assert_eq!(first.since_ms, Some(at - 400_000));
        let first = s.request(first, at + 200);
        beat_all(&mut s, at + 200);
        let _ = s.tick(at + 200);
        assert_eq!(s.get(&first).unwrap().asked_ms, at - 400_000);
        assert!(s.get(&first).unwrap().pos < s.get(&later).unwrap().pos, "first {} later {}", s.get(&first).unwrap().pos, s.get(&later).unwrap().pos);
    }

    #[test]
    fn a_box_is_free_only_after_its_tree_is_scrubbed() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let x = s.request(req("x", Kind::Test, None), 0);
        s.tick(0);
        let y = s.request(req("y", Kind::Test, None), 1);
        s.release(&x, "released", 2);
        beat_all(&mut s, 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert_eq!(state(&s, &y).1, format!("scrubbing its work tree after {x}"));
        s.cleaned("a", &x, 0);
        assert_eq!(grants(&s.tick(3)), vec![y]);
    }

    /// A native lease prefers its warm lane tree, independently of the shared work tree's pin.
    #[test]
    fn a_native_lease_prefers_its_warm_tree() {
        let (sha, other) = ("a".repeat(40), "b".repeat(40));
        let mut at = caps(16, 64.0, Role::Pool);
        at.load1 = 3.0;
        at.tree = format!("{other},{}", "c".repeat(40));
        at.native = vec!["x".into()];
        let mut off = caps(16, 64.0, Role::Pool);
        off.tree = format!("{sha},{}", "c".repeat(40));
        let mut s = sched(&[("at", at), ("off", off)]);
        let mut r = req("x", Kind::Test, None);
        r.rev = Some(format!("rc-x@{sha},main@{}", "c".repeat(40)));
        let x = s.request(r.clone(), 0);
        s.tick(0);
        assert_eq!(node_of(&s, &x).as_deref(), Some("at"));
        // the lane's tree is held at another pin: the other free node takes it
        let mut r2 = r.clone();
        r2.rev = Some(format!("rc-x@{other},main@{}", "c".repeat(40)));
        let y = s.request(r2, 1);
        s.tick(1);
        assert_eq!(node_of(&s, &y).as_deref(), Some("off"));
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
        assert_eq!(grants(&s.tick(1)), vec![named.clone()]);
        release_clean(&mut s, &named, 2);
        beat_all(&mut s, 2);
        assert_eq!(grants(&s.tick(2)), vec![by_os]);
    }
    #[test]
    fn ci_lane_has_no_lane_cap() {
        let mut s = sched(&[("106", caps(64, 256.0, Role::Pool))]);
        for _ in 0..3 {
            let mut r = overlay(CI_LANE, Some("106"), 4.0);
            r.kind = Kind::Build;
            s.request(r, 0);
        }
        assert_eq!(grants(&s.tick(0)).len(), 3);
    }

    #[test]
    fn past_the_lane_cap_a_lease_takes_only_what_leases_within_their_cap_leave() {
        let small = |lane: &str| overlay(lane, None, 8.0);
        let mut c = caps(64, 256.0, Role::Pool);
        c.slots = vec![(Kind::Test, 8)];
        // nothing else waits: the cap keeps no box idle
        let mut s = sched(&[("a", c.clone())]);
        for _ in 0..3 { s.request(small("gapx"), 0); }
        assert_eq!(grants(&s.tick(0)).len(), 3);
        s.request(small("gapx"), 1);
        let a = s.tick(1);
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.sev == Sev::Info && r.line.contains("past the lane cap (3/2 running)"))));
        assert_eq!(grants(&a).len(), 1);
        // another lane waits for the box: the lease past the cap gives way there and goes after it
        let mut s = sched(&[("a", c)]);
        let mine: Vec<String> = (0..2).map(|_| s.request(small("gapx"), 0)).collect();
        assert_eq!(grants(&s.tick(0)).len(), 2);
        let other = s.request(overlay("other", None, 56.0), 1);
        let over = s.request(small("gapx"), 1);
        assert!(grants(&s.tick(1)).is_empty(), "48 cores free: the 8-core lease past its cap leaves them to the 56-core one");
        assert!(state(&s, &over).1.starts_with("lane cap 2/2: other lanes' leases wait for a"), "{:?}", state(&s, &over));
        assert!(s.get(&over).unwrap().lane_capped());
        invariants(&s);
        release_clean(&mut s, &mine[0], 2);
        assert_eq!(grants(&s.tick(2)), vec![other.clone()]);
        assert_eq!(state(&s, &over).0, "queued");
        release_clean(&mut s, &mine[1], 3);
        assert_eq!(grants(&s.tick(3)), vec![over]);
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
        release_clean(&mut s, &a1, 30);
        let a = s.tick(30);
        assert_eq!(grants(&a), vec![b1]);
    }

    #[test]
    fn a_lease_past_twice_its_measure_says_stalled_once() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_node_cost("test", "test", "a", 120_000); }
        let leg = s.request(req("leg", Kind::Test, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![leg.clone()]);
        s.started(&leg, "{}");
        let stalled = |a: &[Action]| a.iter().filter(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains("stalled?") && r.line.contains(&leg))).count();
        beat_all(&mut s, 420_000);
        assert_eq!(stalled(&s.tick(420_000)), 0, "past twice its 2 min, but not 5 min over it");
        beat_all(&mut s, 421_000);
        let a = s.tick(421_000);
        assert_eq!(stalled(&a), 1);
        assert!(a.iter().any(|a| matches!(a, Action::Capture { id, node, why } if *id == leg && node == "a" && why.contains("against its measured"))), "its JITs capture their state");
        beat_all(&mut s, 422_000);
        assert_eq!(stalled(&s.tick(422_000)), 0, "said once");
    }

    #[test]
    fn aging_stays_within_priority() {
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
        hi.prio = 99;
        hi.priority = Priority::Interactive;
        let hi = s.request(hi, t);
        beat_all(&mut s, t);
        let a = s.tick(t);
        assert_eq!((s.get(&hi).unwrap().pos, s.get(&old).unwrap().pos, s.get(&urgent).unwrap().pos), (1, 2, 3));
        assert_eq!(state(&s, &old).1, format!("held for interactive {hi} (expires in 10 min)"));
        assert_eq!(state(&s, &first).0, "starting", "the background run finishes normally");
        // waited over 10 min: one warn
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains(&old))));
        beat_all(&mut s, t + 1);
        let a = s.tick(t + 1);
        assert!(!a.iter().any(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn)));
        release_clean(&mut s, &first, t + 2);
        assert_eq!(grants(&s.tick(t + 2)), vec![hi]);
    }

    #[test]
    fn timed_yields_to_interactive_then_drains_oldest_first() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool))]);
        let b = s.request(req("x", Kind::Build, Some("a")), 0);
        s.tick(0);
        let t = s.request(req("bench", Kind::Timed, Some("a")), 1);
        s.tick(1); // the reservation exists before the interactive request arrives
        let mut late = req("y", Kind::Test, Some("a"));
        late.priority = Priority::Interactive;
        let late = s.request(late, 2);
        s.tick(2);
        assert!(state(&s, &t).1.contains("held for interactive"));
        assert_eq!(s.node_view()[0].hold.as_deref(), Some(late.as_str()));
        invariants(&s);
        beat_all(&mut s, 60_002);
        release_clean(&mut s, &b, 60_002);
        assert_eq!(grants(&s.tick(60_002)), vec![late.clone()], "a timed reservation cannot cost interactive work a minute");
        let mut next = req("next-bench", Kind::Timed, Some("a"));
        next.prio = -100;
        let next = s.request(next, 60_003);
        let shared = s.request(req("shared", Kind::Gate, Some("a")), 60_003);
        s.tick(60_003);
        assert!(s.holds.values().all(|h| h.id != t && h.id != next), "running interactive work prevents timed holds too");
        release_clean(&mut s, &late, 60_004);
        assert_eq!(grants(&s.tick(60_004)), vec![t.clone()], "timed requests keep their original place, independent of numeric priority");
        invariants(&s);
        s.release(&t, "released", 60_005);
        assert!(grants(&s.tick(60_005)).is_empty(), "shared work cannot enter between timed runs during cleanup");
        s.cleaned("a", &t, 0);
        assert_eq!(grants(&s.tick(60_006)), vec![next.clone()]);
        release_clean(&mut s, &next, 60_007);
        assert_eq!(grants(&s.tick(60_007)), vec![shared]);
        invariants(&s);
    }

    /// Interactive work that never stops (live sessions, tests one after another) holds a timed
    /// request off only until it is due: then it reserves the box, the running lease finishes,
    /// and newer interactive requests wait for the timed run.
    #[test]
    fn a_due_timed_request_reserves_through_interactive_demand() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Bench))]);
        let interactive = |s: &mut Sched, name: &str, at: u64| {
            let mut r = req(name, Kind::Test, Some("a"));
            r.priority = Priority::Interactive;
            r.only = true;
            s.request(r, at)
        };
        let first = interactive(&mut s, "i0", 0);
        assert_eq!(grants(&s.tick(0)), vec![first.clone()]);
        let t = s.request(req("bench", Kind::Timed, Some("a")), 1);
        let mut running = first;
        let mut at = 2;
        // before it is due, each ending interactive lease hands the box to the next one
        while at + 60_000 < TIMED_DUE_MS {
            let next = interactive(&mut s, &format!("i{at}"), at);
            beat_all(&mut s, at);
            s.tick(at);
            assert!(state(&s, &t).1.contains("held for interactive"));
            release_clean(&mut s, &running, at + 1);
            assert_eq!(grants(&s.tick(at + 1)), vec![next.clone()]);
            running = next;
            at += 60_000;
        }
        let due = TIMED_DUE_MS + 1;
        let waiting = interactive(&mut s, "late", due);
        beat_all(&mut s, due);
        s.tick(due);
        assert_eq!(s.holds.get("a").map(|h| h.id.as_str()), Some(t.as_str()), "a due timed request reserves its box");
        invariants(&s);
        release_clean(&mut s, &running, due + 1);
        assert_eq!(grants(&s.tick(due + 1)), vec![t.clone()], "the timed run goes before newer interactive work");
        assert_eq!(state(&s, &waiting).0, "queued");
        invariants(&s);
    }

    #[test]
    fn running_timed_work_is_not_interrupted_and_demand_is_per_box() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        let timed = s.request(req("bench", Kind::Timed, Some("a")), 0);
        s.tick(0);
        let mut r = req("interactive", Kind::Test, Some("a"));
        r.priority = Priority::Interactive;
        r.only = true;
        let interactive = s.request(r, 1);
        let next = s.request(req("next", Kind::Timed, Some("a")), 2);
        let elsewhere = s.request(req("elsewhere", Kind::Timed, Some("b")), 2);
        assert_eq!(grants(&s.tick(2)), vec![elsewhere]);
        beat_all(&mut s, 60_001);
        let actions = s.tick(60_001);
        assert!(grants(&actions).is_empty());
        assert!(s.get(&timed).unwrap().state.live());
        assert_eq!(state(&s, &interactive).1, format!("held by {timed} (timed of bench)"));
        release_clean(&mut s, &timed, 60_002);
        assert_eq!(grants(&s.tick(60_002)), vec![interactive.clone()]);
        assert_eq!(state(&s, &next).0, "queued");
        release_clean(&mut s, &interactive, 60_003);
        assert_eq!(grants(&s.tick(60_003)), vec![next]);
        invariants(&s);
    }

    #[test]
    fn a_timed_batch_survives_expired_reservations() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let first = s.request(req("first", Kind::Timed, Some("a")), 0);
        s.tick(0);
        let next = s.request(req("next", Kind::Timed, Some("a")), 1);
        s.tick(1);
        let mut shared = req("whole", Kind::Test, Some("a"));
        shared.whole = true;
        let shared = s.request(shared, 2);
        s.tick(2);
        beat_all(&mut s, HOLD_MS + 2);
        s.tick(HOLD_MS + 2);
        assert_eq!(s.holds["a"].id, next, "a timed reservation lasts while it waits");
        invariants(&s);
        release_clean(&mut s, &first, HOLD_MS + 3);
        assert_eq!(grants(&s.tick(HOLD_MS + 3)), vec![next.clone()], "a whole-box hold cannot split the quiet window");
        invariants(&s);
        release_clean(&mut s, &next, HOLD_MS + 4);
        assert_eq!(grants(&s.tick(HOLD_MS + 4)), vec![shared]);
    }

    #[test]
    fn the_oldest_timed_request_gets_the_box_and_live_sessions_go_elsewhere() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool))]);
        let b = s.request(req("x", Kind::Build, Some("a")), 0);
        s.tick(0);
        let old = s.request(req("old", Kind::Timed, Some("a")), 1);
        s.tick(1);
        let twin = s.request(req("old", Kind::Timed, Some("a")), 1);
        s.tick(1);
        assert!(state(&s, &twin).0 == "ended denied" && state(&s, &twin).1.contains("one timed lease per lane"), "{:?}", state(&s, &twin));
        assert!(s.get(&old).unwrap().eta_ms > 0, "the asker sees its expected wait");
        let mut live = req("live-W1", Kind::Test, None);
        live.live = true;
        let live = s.request(live, 2);
        s.tick(2);
        assert!(state(&s, &live).1.contains("live sessions go to another"), "{:?}", state(&s, &live));
        beat_all(&mut s, HOLD_MS + 2);
        let new = s.request(req("new", Kind::Timed, Some("a")), HOLD_MS + 2);
        s.tick(HOLD_MS + 2);
        assert!(!state(&s, &old).1.contains("held for timed"), "a newer reservation never holds an older timed request: {:?}", state(&s, &old));
        release_clean(&mut s, &b, HOLD_MS + 3);
        assert_eq!(grants(&s.tick(HOLD_MS + 3)), vec![old.clone()]);
        invariants(&s);
        let mut urgent = req("urgent", Kind::Timed, Some("a"));
        urgent.priority = Priority::Interactive;
        let urgent = s.request(urgent, HOLD_MS + 3);
        s.tick(HOLD_MS + 3);
        release_clean(&mut s, &old, HOLD_MS + 4);
        assert_eq!(grants(&s.tick(HOLD_MS + 4)), vec![urgent.clone()], "interactive timed work goes before background timed work");
        release_clean(&mut s, &urgent, HOLD_MS + 5);
        assert_eq!(grants(&s.tick(HOLD_MS + 5)), vec![new]);

        // an interactive live session never waits on background timed work: it takes another box,
        // or the one box that can take it (the timed request parks it once its call ends)
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool)), ("c", caps(64, 256.0, Role::Pool))]);
        let mut r = req("x", Kind::Build, Some("a"));
        r.only = true;
        s.request(r, 0);
        s.tick(0);
        let timed = s.request(req("bench", Kind::Timed, Some("a")), 1);
        s.tick(1);
        let mut r = req("live-W1", Kind::Test, None);
        r.live = true;
        r.src = Src::Overlay;
        r.priority = Priority::Interactive;
        let anywhere = s.request(r, 2);
        let mut r = req("live-W2", Kind::Test, Some("a"));
        r.live = true;
        r.src = Src::Overlay;
        r.only = true;
        r.priority = Priority::Interactive;
        let only_a = s.request(r, 2);
        s.tick(2);
        assert_eq!(node_of(&s, &anywhere).as_deref(), Some("c"));
        assert_eq!(node_of(&s, &only_a).as_deref(), Some("a"), "{:?}", state(&s, &only_a));
        assert_eq!(state(&s, &timed).0, "queued");
        invariants(&s);
    }
    #[test]
    fn disk_floor_waits_and_asks_to_reclaim() {
        let mut c = caps(64, 256.0, Role::Pool);
        c.disk_free_gb = 31.0;
        let mut s = sched(&[("a", c.clone())]);
        let b = s.request(req("x", Kind::Build, None), 0);
        let a = s.tick(0);
        assert_eq!(state(&s, &b).1, "disk floor: 31 GB free, needs 20, floor 25");
        assert!(a.iter().any(|a| matches!(a, Action::Reclaim { node, need_gb } if node == "a" && *need_gb == 20.0)));
        // asked once, not every tick
        assert!(!s.tick(1).iter().any(|a| matches!(a, Action::Reclaim { .. })));
        c.disk_free_gb = 80.0;
        s.set_caps("a", c, 2);
        assert_eq!(grants(&s.tick(2)), vec![b]);
    }

    #[test]
    fn landing_gates_go_first_and_idle_gates_end() {
        let mut s = sched(&[("n0", caps(16, 64.0, Role::Pool)), ("n1", caps(16, 64.0, Role::Pool))]);
        // two lane gates take both boxes; a later lane gate and a landing wait
        let lanes: Vec<String> = (0..2).map(|i| s.request(req(&format!("lane{i}"), Kind::Gate, None), 0)).collect();
        s.tick(0);
        for id in &lanes {
            assert_eq!(state(&s, id).0, "starting");
        }
        let late = s.request(req("lane9", Kind::Gate, None), 1);
        let land = s.request(req(LANDING_LANE, Kind::Gate, None), 2);
        s.tick(2);
        assert_eq!(state(&s, &land).0, "queued");
        // the gates beat but one runs nothing: after the idle limit it ends, and the landing
        // takes its box first
        for id in lanes.iter().chain([&late, &land]) {
            s.beat(id, GATE_IDLE_MS + 5);
        }
        s.set_info(&lanes[0], r#"{"running":"bash land_gate.sh full"}"#);
        s.tick(GATE_IDLE_MS + 5);
        assert_eq!(state(&s, &lanes[0]).0, "starting", "a gate that runs something stays");
        assert_eq!(state(&s, &lanes[1]).0, "ended idle");
        let freed = s.get(&lanes[1]).unwrap().node.clone().unwrap();
        s.cleaned(&freed, &lanes[1], 0);
        beat_all(&mut s, GATE_IDLE_MS + 6);
        s.tick(GATE_IDLE_MS + 6);
        assert_eq!(state(&s, &land).0, "starting", "the landing takes the freed box first");
        assert_eq!(state(&s, &late).0, "queued");
    }
    /// `--needs gui`: a test lease that opens windows goes only to nodes that present (screen
    /// slots above 0); a box at slots.screen=0 takes none, plain test leases still go there.
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
        let mut lit = caps(16, 64.0, Role::Pool);
        lit.display = Some("wm".into());
        assert!(meets(&lit, "gui", "1") && meets(&lit, "gui", "") && !meets(&dark, "gui", "yes") && meets(&dark, "gui", "0"));
    }

    /// A user's WM takes only what names it: no unpinned lease, no soft pin's fallback.
    #[test]
    fn a_user_wm_takes_only_leases_that_name_it() {
        let mut s = sched(&[("wm", caps(64, 256.0, Role::UserWm)), ("p", caps(64, 256.0, Role::Pool))]);
        let mut held = req("h", Kind::Test, Some("p"));
        held.only = true;
        let held = s.request(held, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &held).as_deref(), Some("p"));
        // the pool is full and the WM idle: an unpinned lease and a soft pin to the pool wait
        let mut any = req("a", Kind::Test, None);
        any.needs = parse_needs("os=linux");
        let any = s.request(any, 1);
        let soft = s.request(req("b", Kind::Test, Some("p")), 1);
        s.tick(1 + PIN_GRACE_MS);
        assert_eq!(node_of(&s, &any), None);
        assert_eq!(node_of(&s, &soft), None);
        // one that names it goes there
        let named = s.request(req("c", Kind::Test, Some("wm")), 2 + PIN_GRACE_MS);
        s.tick(2 + PIN_GRACE_MS);
        assert_eq!(node_of(&s, &named).as_deref(), Some("wm"));
    }

    #[test]
    fn deny_what_never_fits() {
        let mut s = sched(&[("a", caps(8, 32.0, Role::Pool)), ("wm", caps(32, 64.0, Role::UserWm))]);
        // the user switched this WM's dev off (every node defaults on)
        s.set_dev("wm", false, 0).unwrap();
        let g = s.request(req("x", Kind::Gate, Some("a")), 0); // 8 cores fits exactly
        let mut big = req("x", Kind::Build, None);
        big.needs = parse_needs("gpu=GPU-B");
        let big = s.request(big, 0);
        let mut user = req("y", Kind::Build, Some("wm"));
        user.only = true;
        let user = s.request(user, 0);
        // a soft pin to the switched-off WM waits for a twin instead (a is full with the gate)
        let soft = s.request(req("z", Kind::Build, Some("wm")), 0);
        let insp = {
            let mut r = req("y", Kind::Inspect, Some("wm"));
            r.jit = Some("sandbox".into());
            s.request(r, 0)
        };
        s.tick(0);
        assert_eq!(state(&s, &g).0, "starting");
        assert_eq!(state(&s, &big).0, "ended denied");
        assert_eq!(state(&s, &user), ("queued".into(), "dev off".into()));
        s.release(&user, "released", 0);
        assert_eq!(state(&s, &soft).0, "queued");
        // inspect is always allowed on a user WM
        assert_eq!(state(&s, &insp).0, "starting");
        // the user switches it on: writes go, one at a time
        s.set_dev("wm", true, 1).unwrap();
        let w = s.request(req("y", Kind::Build, Some("wm")), 1);
        assert_eq!(grants(&s.tick(1)), vec![soft.clone()]);
        assert_eq!(node_of(&s, &soft).as_deref(), Some("wm"), "its own node, once it takes leases");
        assert_eq!(state(&s, &w).0, "queued");
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
        // the box frees once the node answered the expired lease's cleanup
        assert_eq!(state(&s, &q).0, "queued");
        s.cleaned("a", &b, 0);
        s.beat(&q, 30_001);
        s.tick(30_002);
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
        assert_eq!(state(&s, &w), ("queued".into(), "dev off".into()));
        s.set_dev("a", true, 3).unwrap();
        release_clean(&mut s, &b, 3);
        beat_all(&mut s, 3);
        assert_eq!(grants(&s.tick(3)), vec![w]);
    }

    #[test]
    fn a_person_at_the_box_drains_it_and_their_switch_wins() {
        let user = |present: bool| Some(crate::lease::UserSense { present, idle_ms: Some(if present { 5_000 } else { 200_000 }), what: "key from its device".into(), idle_min: 3 });
        let mut s = sched(&[("a", caps(64, 256.0, Role::UserWm))]);
        let b = s.request(req("x", Kind::Build, Some("a")), 0);
        s.tick(0);
        // the person arrives: the running lease is never ended, nothing new starts
        s.set_caps("a", Caps { user: user(true), ..caps(64, 256.0, Role::UserWm) }, 1);
        let a = s.tick(1);
        assert!(!a.iter().any(|a| matches!(a, Action::End { .. })));
        let v = &s.node_view()[0];
        assert!(v.draining && v.dev && v.state_word() == "user (key from its device 5 s ago), draining (1 leases)", "{}", v.state_word());
        assert!(v.to_json().contains("\"user\":true"));
        let w = s.request(req("y", Kind::Build, Some("a")), 2);
        let i = s.request(req("z", Kind::Inspect, Some("a")), 2);
        release_clean(&mut s, &b, 2);
        beat_all(&mut s, 2);
        // inspect stays (it writes nothing); a build waits for the person to leave
        assert_eq!(grants(&s.tick(2)), vec![i.clone()]);
        assert!(state(&s, &w).1.starts_with("user (key from its device 5 s ago)"), "{:?}", state(&s, &w));
        s.release(&i, "released", 2);
        assert_eq!(s.node_view()[0].state_word(), "user (key from its device 5 s ago)");
        // they leave: it takes work again
        s.set_caps("a", Caps { user: user(false), ..caps(64, 256.0, Role::UserWm) }, 3);
        beat_all(&mut s, 3);
        assert_eq!(grants(&s.tick(3)), vec![w.clone()]);
        // the user's own off wins: an idle person never turns a node on
        let mut s = sched(&[("a", caps(64, 256.0, Role::UserWm))]);
        s.set_dev("a", false, 0).unwrap();
        s.set_caps("a", Caps { user: user(false), ..caps(64, 256.0, Role::UserWm) }, 1);
        let w = s.request(req("y", Kind::Build, Some("a")), 1);
        s.tick(1);
        assert!(!s.dev("a") && s.node_view()[0].state_word() == "dev off (user WM)");
        assert_ne!(state(&s, &w).0, "starting");
        // nor does a person's presence show over it
        s.set_caps("a", Caps { user: user(true), ..caps(64, 256.0, Role::UserWm) }, 2);
        assert_eq!(s.node_view()[0].state_word(), "dev off (user WM)");
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
        release_clean(&mut s, &first, 1);
        let again = s.request(req("x", Kind::Build, None), 2);
        s.tick(2);
        let l = s.get(&again).unwrap();
        assert_eq!(l.node.as_deref(), Some(node.as_str()));
        assert_eq!(l.cost.disk_gb, 2.0);
    }

    #[test]
    fn cancel_ends_one_lease_and_keeps_the_lane_off_that_node() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool)), ("b", caps(64, 256.0, Role::Pool)), ("c", caps(64, 256.0, Role::Pool))]);
        let x = s.request(req("lane-x", Kind::Build, Some("a")), 0);
        let y = s.request(req("lane-y", Kind::Build, Some("b")), 0);
        s.tick(1);
        assert_eq!(s.get(&x).unwrap().node.as_deref(), Some("a"));
        assert_eq!(s.get(&y).unwrap().node.as_deref(), Some("b"));
        // cancel x: y keeps running
        assert!(s.cancel(&x, 10));
        let acts = s.tick(11);
        assert!(acts.iter().any(|a| matches!(a, Action::End { id, how, .. } if *id == x && how == "cancelled")), "{acts:?}");
        assert!(!acts.iter().any(|a| matches!(a, Action::End { id, .. } if *id == y)));
        assert!(s.get(&y).unwrap().state.live());
        assert!(!s.cancel(&x, 12), "already ended");
        s.cleaned("a", &x, 0);
        // lane-x asks again with no node: it is placed, but not on a (free) nor b (held)
        let again = s.request(req("lane-x", Kind::Build, None), 20);
        s.tick(21);
        assert_eq!(s.get(&again).unwrap().node.as_deref(), Some("c"));
        // naming the node overrides the exclusion
        let named = s.request(req("lane-x", Kind::Build, Some("a")), 22);
        s.tick(23);
        assert_eq!(s.get(&named).unwrap().node.as_deref(), Some("a"));
        // the exclusion lapses
        release_clean(&mut s, &again, 30);
        release_clean(&mut s, &named, 30);
        let later = s.request(req("lane-x", Kind::Build, None), CANCEL_EXCLUDE_MS + 100);
        s.tick(CANCEL_EXCLUDE_MS + 101);
        assert!(s.get(&later).unwrap().state.live());
    }
    #[test]
    fn requests_round_trip() {
        let mut r = req("gapx", Kind::Build, Some("217"));
        r.needs = parse_needs("arch=x64,ram=32G");
        r.prio = 0;
        r.priority = Priority::Interactive;
        r.loader = Some("loader-id".into());
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

    fn node_of(s: &Sched, id: &str) -> Option<String> {
        s.get(id).unwrap().node.clone()
    }

    #[test]
    fn only_on_a_hot_box_that_refuses_the_class_is_denied_at_once() {
        let mut s = sched(&[("h", caps(16, 64.0, Role::Hot)), ("p", caps(16, 64.0, Role::Pool))]);
        let mut r = req("x", Kind::Screen, Some("h"));
        r.only = true;
        r.est_min = Some(40.0);
        let x = s.request(r, 0);
        s.tick(0);
        assert_eq!(state(&s, &x).0, "ended denied", "{:?}", state(&s, &x));
        assert!(state(&s, &x).1.contains("hot box"), "{:?}", state(&s, &x));
        // a hot Mac takes the macOS work only it can do (devhub's JIT prewarm, class medium)
        let mut mac = caps(16, 64.0, Role::Hot);
        mac.os = "macos".into();
        let mut s = sched(&[("mac", mac)]);
        let mut r = req("show-prewarm", Kind::Build, Some("mac"));
        r.only = true;
        r.needs = vec![("os".into(), "macos".into())];
        r.est_min = Some(10.0);
        let x = s.request(r.clone(), 0);
        assert_eq!(grants(&s.tick(0)), vec![x]);
        // a build's 12 GB default and build peaks measured on the big Linux boxes each take a
        // 16 GB Mac's whole budget, no denial
        for peaks in [0, 3] {
            let mut mac = caps(10, 16.0, Role::Hot);
            mac.os = "macos".into();
            let mut s = sched(&[("mac", mac)]);
            for _ in 0..peaks { s.record_resources("build", "build", Some(10.0), None); }
            let x = s.request(r.clone(), 0);
            assert_eq!(grants(&s.tick(0)), vec![x.clone()], "{peaks} peaks: {:?}", state(&s, &x));
        }
    }

    #[test]
    fn large_tests_age_onto_a_hot_only_fleet() {
        let mut s = sched(&[("h", caps(16, 64.0, Role::Hot))]);
        let mut r = req("x", Kind::Test, None);
        r.cost = CostClass::Large;
        r.since_ms = Some(1000);
        let x = s.request(r, 2000);
        assert!(grants(&s.tick(2000)).is_empty());
        assert_eq!(state(&s, &x).0, "queued");
        assert!(state(&s, &x).1.contains("until it has waited 10 min"));
        let at = 1000 + AGE_STEP_MS;
        beat_all(&mut s, at - 1);
        assert!(grants(&s.tick(at - 1)).is_empty());
        assert_eq!(state(&s, &x).0, "queued");
        beat_all(&mut s, at);
        assert_eq!(grants(&s.tick(at)), vec![x.clone()]);
        assert_eq!(node_of(&s, &x).as_deref(), Some("h"));
    }

    #[test]
    fn soft_pin_waits_for_its_node_then_takes_a_twin() {
        let mut mac = caps(16, 64.0, Role::Pool);
        mac.arch = "aarch64".into();
        mac.os = "macos".into();
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("m", mac)]);
        let mut r = req("f1", Kind::Test, Some("a"));
        r.only = true;
        let fill = s.request(r, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &fill).as_deref(), Some("a"));
        // a soft pin to a held node goes to a twin at once; within the grace a node that is merely
        // slow to come up keeps it waiting
        let x = s.request(req("x", Kind::Test, Some("a")), 1);
        let mut y = req("y", Kind::Test, Some("a"));
        y.only = true;
        let y = s.request(y, 1);
        s.tick(2);
        assert_eq!(node_of(&s, &x).as_deref(), Some("b"), "a held node sends a soft pin to its twin");
        assert_eq!(state(&s, &y).0, "queued", "--only waits for its node");
        release_clean(&mut s, &fill, 3);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(node_of(&s, &y).as_deref(), Some("a"));
        // the Mac takes only what says it needs macOS: naming it is not enough
        let named = s.request(req("n", Kind::Test, Some("m")), 5);
        let mut mac = req("mac", Kind::Test, None);
        mac.needs.push(("os".into(), "macos".into()));
        let mac = s.request(mac, 5);
        s.tick(5);
        assert!(state(&s, &named).0 == "ended denied" && state(&s, &named).1.contains("--needs os=macos"), "{:?}", state(&s, &named));
        assert_eq!(node_of(&s, &mac).as_deref(), Some("m"));
        // nor does a live session (interactive, its app stays up) without macOS needs
        let mut live = req("live-W1", Kind::Test, None);
        live.live = true;
        live.priority = Priority::Interactive;
        let live = s.request(live, 6);
        s.tick(6);
        assert_ne!(node_of(&s, &live).as_deref(), Some("m"), "{:?}", state(&s, &live));
        // what every box that reported in refuses is refused now, not left waiting on a box that
        // never did; the answer says what to ask instead
        s.add_node("q", "192.0.2.9", None);
        let mut arm = req("arm", Kind::Test, None);
        arm.needs.push(("arch".into(), "arm64".into()));
        let arm = s.request(arm, 7);
        s.tick(7);
        let (st, why) = state(&s, &arm);
        assert!(st == "ended denied" && why.contains("--needs os=macos") && why.contains("q never reported in"), "{st}: {why}");
    }
    /// A timed lease waiting for a holds a: a build named to a goes to a twin at once, a test
    /// named `--only a` waits; the timed lease itself waits only for a's running lease.
    #[test]
    fn a_timed_hold_blocks_only_leases_that_need_that_node() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        let mut r = req("run", Kind::Test, Some("a"));
        r.only = true;
        let running = s.request(r, 0);
        s.tick(0);
        let timed = s.request(req("bench", Kind::Timed, Some("a")), 1);
        let build = s.request(req("lane", Kind::Build, Some("a")), 2);
        let mut o = req("strict", Kind::Test, Some("a"));
        o.only = true;
        let strict = s.request(o, 3);
        s.tick(3);
        assert!(state(&s, &timed).1.contains(&format!("held by {running}")), "{:?}", state(&s, &timed));
        assert_eq!(node_of(&s, &build).as_deref(), Some("b"), "a held node sends a soft pin to its twin at once");
        assert!(state(&s, &strict).1.contains("held for timed"), "{:?}", state(&s, &strict));
        release_clean(&mut s, &running, 4);
        beat_all(&mut s, 5);
        s.tick(5);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("a"));
    }

    /// A waiting timed lease holds an idle node (it starts now), not its lane's busy one.
    #[test]
    fn a_timed_lease_holds_an_idle_node_over_its_busy_sticky_one() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        let first = s.request(req("bench", Kind::Build, Some("a")), 0);
        s.tick(0);
        release_clean(&mut s, &first, 1);
        let mut r = req("other", Kind::Test, Some("a"));
        r.only = true;
        s.request(r, 2);
        s.tick(2);
        let timed = s.request(req("bench", Kind::Timed, None), 3);
        s.tick(3);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("b"));
    }

    /// Busy nodes wait; after pressure clears the warm and longest-idle preferences apply.
    #[test]
    fn busy_nodes_wait_then_warm_and_idle_preferences_apply() {
        let mut hot = caps(16, 64.0, Role::Pool);
        hot.load1 = 50.0;
        hot.cpu_pct = Some(100.0);
        hot.ram_avail_gb = 0.5;
        let mut s = sched(&[("a", hot.clone())]);
        let x = s.request(req("x", Kind::Build, None), 0);
        s.tick(0);
        assert_eq!(state(&s, &x).0, "queued");
        assert_eq!(state(&s, &x).1, "node busy");
        hot.ram_avail_gb = hot.ram_gb * 0.8;
        hot.cpu_pct = Some(0.0);
        hot.load1 = 0.0;
        s.set_caps("a", hot, 1);
        s.tick(1);
        assert_eq!(node_of(&s, &x).as_deref(), Some("a"), "{:?}", state(&s, &x));
        release_clean(&mut s, &x, 1);
        // the lane's warm box after its pressure cleared
        s.set_caps("b", caps(16, 64.0, Role::Pool), 2);
        let again = s.request(req("x", Kind::Build, None), 3);
        s.tick(3);
        assert_eq!(node_of(&s, &again).as_deref(), Some("a"));
        release_clean(&mut s, &again, 4);
        // a new lane: b never ran a lease (idle longest) wins over a, released just now
        let y = s.request(req("y", Kind::Build, None), 5);
        s.tick(5);
        assert_eq!(node_of(&s, &y).as_deref(), Some("b"));
        release_clean(&mut s, &y, 6);
        let mut warm = caps(16, 64.0, Role::Pool);
        warm.jit_store = Some(std::sync::Arc::new(super::super::jitstore::Snapshot { ids: vec!["jit-a".into()], ..Default::default() }));
        s.set_caps("a", warm, 6);
        s.record_node_cost("jit-build", "", "b", 90_000);
        s.record_node_cost("test:pkg", "test", "a", 20_000);
        s.record_node_cost("test:pkg", "test", "b", 20_000);
        let mut r = req("jit", Kind::Test, None);
        r.loader = Some("jit-a".into());
        r.class = "test:pkg".into();
        let hit = s.request(r.clone(), 7);
        s.tick(7);
        assert_eq!(node_of(&s, &hit).as_deref(), Some("a"), "the ready JIT saves a build");
        s.leases.iter_mut().find(|l| l.id == hit).unwrap().req.class = "long-background".into();
        s.record_node_cost("long-background", "test", "a", 600_000);
        r.lane = "cold".into();
        let cold = s.request(r, 8);
        s.tick(8);
        assert_eq!(node_of(&s, &cold).as_deref(), Some("b"), "a cold build finishes before the warm box frees");
        // a warm box at WARM_BUSY load or more loses its pull: the least busy box takes the lane
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        let first = s.request(req("w", Kind::Build, None), 0);
        s.tick(0);
        let warm = node_of(&s, &first).unwrap();
        release_clean(&mut s, &first, 1);
        let again = s.request(req("w", Kind::Build, None), 2);
        s.tick(2);
        assert_eq!(node_of(&s, &again).as_ref(), Some(&warm), "the warm box while it has room");
        release_clean(&mut s, &again, 3);
        let mut loaded = caps(16, 64.0, Role::Pool);
        loaded.load1 = 12.0;
        s.set_caps(&warm, loaded, 4);
        let moved = s.request(req("w", Kind::Build, None), 5);
        s.tick(5);
        assert!(node_of(&s, &moved).is_some_and(|n| n != warm), "a busy warm box: the least busy box ({:?})", state(&s, &moved));
    }

    fn overlay(lane: &str, node: Option<&str>, cores: f64) -> Req {
        let mut r = req(lane, Kind::Test, node);
        r.only = node.is_some();
        r.src = Src::Overlay;
        r.rev = Some(format!("work@{}", "a".repeat(40)));
        r.cores = Some(cores);
        r
    }

    /// Check every candidate, including partially occupied boxes, not just empty ones. Holds
    /// are checked independently of now_fits so an invalid reservation cannot hide idle capacity.
    fn invariants(s: &Sched) {
        for l in s.leases.iter().filter(|l| l.state == State::Queued) {
            if let Ok(names) = s.candidates(l, &s.holds) {
                for name in names {
                    assert!(s.now_fits(name, l, &s.cost_on(name, l), &s.holds).is_err(), "{} still fits on {name} at {}: {}", l.id, s.clock, l.reason);
                }
            }
        }
        let mut owners = HashSet::new();
        for (name, h) in &s.holds {
            let l = s.get(&h.id).unwrap();
            assert_eq!(l.state, State::Queued);
            assert!(owners.insert(&h.id), "{} reserves more than one box", h.id);
            assert!(h.until > s.clock && l.hold_until == Some(h.until));
            let cost = s.cost_on(name, l);
            assert!(s.ready(name, l, &cost).is_ok(), "{} cannot start on {name} even without running work: {:?}", h.id, s.ready(name, l, &cost));
            assert!(s.hot_wait(name, l, &s.holds).is_none());
            // (a timed request past its due time reserves despite interactive demand)
            if l.req.kind == Kind::Timed && !s.timed_due(l) {
                assert!(!s.interactive_boxes.contains_key(name), "{} reserves {name} while interactive work is waiting or running", h.id);
                for r in s.leases.iter().filter(|r| r.priority == Priority::Interactive && r.req.kind != Kind::Timed && r.req.on_node()) {
                    // (a running live job is no such demand: it parks for the timed request)
                    if r.state.live() && !s.is_live_job(r) { assert_ne!(r.node.as_deref(), Some(name.as_str())); }
                    if r.state == State::Queued && s.ready(name, r, &s.cost_on(name, r)).is_ok() {
                        assert!(!s.candidates(r, &Holds::new()).is_ok_and(|names| names.contains(&name)), "timed hold hides interactive demand {} on {name}", r.id);
                    }
                }
            }
            assert!(s.resources(name, l, &cost, true).is_err(), "{} holds an available box {name}", h.id);
            let names = s.candidates(l, &s.holds).ok().unwrap();
            assert!(names.contains(&name));
            assert!(names.iter().all(|n| s.held_by(n, l, &s.cost_on(n, l), &s.holds).is_none()), "transitive hold by {}", h.id);
        }
        for name in s.nodes.keys() {
            let running: Vec<_> = s.live_on_node(name, "").collect();
            for timed in running.iter().filter(|l| l.req.kind == Kind::Timed) {
                assert!(running.iter().all(|r| r.id == timed.id || !r.req.kind.writes()), "{} started beside a writer on {name}", timed.id);
            }
        }
    }

    #[test]
    fn live_deadlock_releases_unusable_holds_and_fills_the_fleet() {
        let nodes: Vec<_> = ["106", "105", "c", "d", "e", "f", "g", "h", "hot"].into_iter().map(|n| (n, caps(16, 64.0, if n == "hot" { Role::Hot } else { Role::Pool }))).collect();
        let mut s = sched(&nodes);
        for name in ["106", "105", "c"] {
            let mut r = req(name, Kind::Build, Some(name));
            r.only = true;
            s.request(r, 0);
        }
        s.tick(0);
        let timed = s.request(req("timed-a", Kind::Timed, Some("106")), 1);
        s.tick(1);
        assert_eq!(s.holds["106"].id, timed);
        // A required protocol update makes the node unready; its old hold must not persist.
        s.nodes.get_mut("106").unwrap().caps.as_mut().unwrap().hub_proto = 0;
        let cleanup = s.leases.iter().find(|l| l.node.as_deref() == Some("c")).unwrap().id.clone();
        s.release(&cleanup, "released", 2);
        s.request(req("timed-b", Kind::Timed, Some("105")), 2);
        for name in ["106", "105"] { s.request(req("ci", Kind::Ci, Some(name)), 2); }
        let mut interactive = req("interactive", Kind::Test, Some("c"));
        interactive.only = true;
        interactive.priority = Priority::Interactive;
        let interactive = s.request(interactive, 2);
        let beside_cleanup = s.request(overlay("beside-cleanup", Some("c"), 4.0), 2);
        // Measured demotion must not strand this request while the hot box is idle.
        for _ in 0..3 { s.record_node_cost("medium-screen", "screen", "hot", 180_000); }
        let mut medium = req("screen", Kind::Screen, Some("hot"));
        medium.class = "medium-screen".into();
        let medium = s.request(medium, 2);
        for i in 0..34 { s.request(overlay(&format!("lane-{i}"), None, 4.0), 2); }
        assert_eq!(s.leases.iter().filter(|l| l.state == State::Queued).count(), 41);
        s.tick(2);
        invariants(&s);
        assert!(!s.holds.contains_key("106"), "an incompatible hub cannot own a timed reservation");
        assert!(!s.holds.values().any(|h| h.id == interactive), "cleanup is not a running lease");
        assert_eq!(node_of(&s, &beside_cleanup).as_deref(), Some("c"));
        // Small requests fill hot first. Drain those without allowing successors, then the
        // measured medium request takes the now-free hot box in that same tick.
        let small: Vec<_> = s.leases.iter().filter(|l| l.req.lane.starts_with("lane-")).map(|l| l.id.clone()).collect();
        for id in small { release_clean(&mut s, &id, 3); }
        s.tick(3);
        invariants(&s);
        assert_eq!(node_of(&s, &medium).as_deref(), Some("hot"));
        assert!(s.why_queued(&timed, 3).unwrap().iter().any(|line| line.contains("protocol")));
    }

    #[test]
    fn reservations_allow_companions_expire_and_do_not_chain() {
        let mut s = sched(&[("106", caps(16, 64.0, Role::Pool))]);
        let running = s.request(overlay("running", Some("106"), 12.0), 0);
        s.tick(0);
        let mut r = overlay("interactive", Some("106"), 8.0);
        r.priority = Priority::Interactive;
        let held = s.request(r, 1);
        s.tick(1);
        let companion = s.request(overlay("companion", Some("106"), 4.0), 2);
        assert_eq!(grants(&s.tick(2)), vec![companion]);
        assert_eq!(s.holds["106"].id, held);
        invariants(&s);
        release_clean(&mut s, &running, 3);
        assert_eq!(grants(&s.tick(3)), vec![held]);
        assert!(s.holds.is_empty());

        let mut s = sched(&[("106", caps(16, 64.0, Role::Pool)), ("105", caps(16, 64.0, Role::Pool))]);
        for name in ["106", "105"] {
            let mut r = req(name, Kind::Test, Some(name));
            r.only = true;
            s.request(r, 0);
        }
        s.tick(0);
        let mut r = req("interactive", Kind::Test, Some("106"));
        r.only = true;
        r.priority = Priority::Interactive;
        let interactive = s.request(r, 1);
        s.tick(1);
        let timed = s.request(req("timed", Kind::Timed, None), 2);
        let ci = s.request(req("ci", Kind::Ci, Some("105")), 2);
        s.tick(2);
        invariants(&s);
        assert_eq!(s.holds.len(), 1);
        assert_eq!(s.holds["106"].id, interactive);
        assert_eq!(node_of(&s, &ci).as_deref(), Some("105"));
        assert_eq!(state(&s, &timed).0, "queued");

        let mut s = sched(&[("106", caps(16, 64.0, Role::Pool))]);
        let running = s.request(req("running", Kind::Test, Some("106")), 0);
        s.tick(0);
        let timed = s.request(req("timed", Kind::Timed, Some("106")), 1);
        s.tick(1);
        let mut r = req("ci", Kind::Ci, Some("106"));
        r.est_min = Some(120.0);
        let ci = s.request(r, 2);
        s.tick(2);
        let why = s.why_queued(&ci, 180_001).unwrap().join("\n");
        assert!(why.contains(&format!("held for timed {timed} (the box drains for it")) && why.contains(&format!("\"blockers\":[\"{timed}\"]")), "{why}");
        beat_all(&mut s, HOLD_MS + 1);
        assert!(grants(&s.tick(HOLD_MS + 1)).is_empty(), "a timed reservation lasts while it waits: work that outlives the drain stays off");
        assert_eq!(s.holds["106"].id, timed);
        invariants(&s);
        release_clean(&mut s, &running, HOLD_MS + 2);
        assert_eq!(grants(&s.tick(HOLD_MS + 2)), vec![timed]);
        assert!(s.holds.is_empty());
        invariants(&s);
    }

    #[test]
    fn timed_reserves_the_first_release_and_drops_unready_boxes() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        for (name, minutes) in [("a", 20.0), ("b", 1.0)] {
            let mut r = req(name, Kind::Test, Some(name));
            r.only = true;
            r.est_min = Some(minutes);
            s.request(r, 0);
        }
        s.tick(0);
        let timed = s.request(req("timed", Kind::Timed, None), 1);
        s.tick(1);
        assert_eq!(s.holds["b"].id, timed);
        s.set_dev("b", false, 2).unwrap();
        s.tick(2);
        assert_eq!(s.holds.len(), 1);
        assert_eq!(s.holds["a"].id, timed);
        s.set_dev("b", true, 3).unwrap();
        s.tick(3);
        assert_eq!(s.holds["b"].id, timed);
        assert_eq!(s.holds["b"].until, HOLD_MS + 3, "a timed reservation is renewed while it waits");
        s.updating.insert("a".into(), "replacement".into());
        s.nodes.get_mut("b").unwrap().caps.as_mut().unwrap().hub_proto = 0;
        s.tick(4);
        assert_eq!(s.holds.len(), 1);
        assert_eq!(s.holds["a"].id, timed, "an updating hub still takes leases");
        invariants(&s);
        // timed leases run on the bench boxes, the first one first; another box only for needs
        // no bench box meets (the GPU-B), never the throughput box
        let mut big_gpu = caps(16, 64.0, Role::Pool);
        big_gpu.gpus = vec![("GPU-B".into(), 32.0)];
        let mut s = sched(&[("bench1", caps(16, 64.0, Role::Bench)), ("bench2", caps(16, 64.0, Role::Bench)), ("pool", caps(32, 64.0, Role::Pool)), ("gpu", big_gpu)]);
        let t: Vec<String> = (0..3).map(|i| s.request(req(&format!("t{i}"), Kind::Timed, None), i)).collect();
        s.tick(3);
        assert_eq!(t.iter().map(|id| node_of(&s, id)).collect::<Vec<_>>(), vec![Some("bench1".into()), Some("bench2".into()), None], "{:?}", state(&s, &t[2]));
        let mut r = req("t-gpu", Kind::Timed, None);
        r.needs.push(("gpu".into(), "GPU-B".into()));
        let needs = s.request(r, 4);
        s.tick(4);
        assert_eq!(node_of(&s, &needs).as_deref(), Some("gpu"));
        // other work takes a bench box last among those with room; a queued interactive lease
        // claims one only when no other box is ready (or it names it)
        let mut s = sched(&[("bench1", caps(64, 256.0, Role::Bench)), ("pool", caps(64, 256.0, Role::Pool))]);
        let overlay = |lane: &str, kind: Kind| {
            let mut r = req(lane, kind, None);
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r
        };
        let mut session = overlay("live-W2", Kind::Test);
        session.live = true;
        let session = s.request(session, 0);
        let gate = s.request(overlay("gate", Kind::Gate), 0);
        let mut big = overlay("big", Kind::Build);
        big.cores = Some(40.0);
        let big = s.request(big, 0);
        s.tick(0);
        assert_eq!([&session, &gate, &big].map(|id| node_of(&s, id)), [Some("pool".into()), Some("pool".into()), Some("pool".into())], "{:?}", [&session, &gate, &big].map(|id| state(&s, id)));
        let next = s.request(overlay("next", Kind::Test), 1);
        s.tick(1);
        assert_eq!(node_of(&s, &next).as_deref(), Some("bench1"), "a full pool box loses to a bench box with room");
        let mut s2 = sched(&[("bench1", caps(64, 256.0, Role::Bench)), ("pool", caps(64, 256.0, Role::Pool))]);
        s2.warm.insert(("warm".into(), "bench1".into()));
        let warm = s2.request(overlay("warm", Kind::Test), 0);
        s2.tick(0);
        assert_eq!(node_of(&s2, &warm).as_deref(), Some("bench1"), "the lane's warm bench box beats a cold pool box");
        let mut asks = overlay("asks", Kind::Test);
        asks.priority = Priority::Interactive;
        let asks = s.request(asks, 2);
        let mut named = overlay("named", Kind::Test);
        named.node = Some("bench1".into());
        named.priority = Priority::Interactive;
        let named = s.request(named, 2);
        s.refresh_interactive();
        assert_eq!(s.interactive_boxes, BTreeMap::from([("bench1".to_string(), vec![named]), ("pool".to_string(), vec![asks])]));
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut session = req("live-W1", Kind::Test, None);
        session.live = true;
        let session = s.request(session, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &session).as_deref(), Some("a"));
        s.update_drain.insert("a".into());
        let drained = s.request(req("drained", Kind::Test, None), 0);
        let acts = s.tick(0);
        assert_eq!(node_of(&s, &drained), None, "an older hub with its update ready drains");
        assert!(acts.iter().any(|a| matches!(a, Action::ParkLive { leases, .. } if leases == &vec![session.clone()])), "its live sessions park");
        release_clean(&mut s, &session, 0);
        // idle live sessions are charged a core each: what waits on their box's budget gets the
        // rest; a session's next call has room for its full charge again
        let mut t = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let overlay = |lane: &str, live: bool| {
            let mut r = req(lane, Kind::Test, None);
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r.live = live;
            r
        };
        let sessions: Vec<String> = (0..4).map(|i| t.request(overlay(&format!("live-W{i}"), true), 0)).collect();
        t.tick(0);
        let waits = t.request(overlay("waits", false), 1);
        t.tick(1);
        assert!(state(&t, &waits).1.starts_with("budget cores 16/16"), "{:?}", state(&t, &waits));
        t.live_idle.extend(sessions.iter().cloned());
        t.tick(2);
        assert_eq!(node_of(&t, &waits).as_deref(), Some("a"), "{:?}", state(&t, &waits));
        assert_eq!(t.live_room(&sessions[0]), Ok(()));
        // a WM on another hosted wire: screen and gui leases wait for its restart, headless work goes on
        let mut w = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        w.nodes.get_mut("a").unwrap().caps.as_mut().unwrap().wm_wire = "makepad-wm-0 answered mismatch".into();
        let screen = w.request(req("screen", Kind::Screen, Some("a")), 0);
        let headless = w.request(req("headless", Kind::Test, Some("a")), 0);
        w.tick(0);
        assert_eq!((node_of(&w, &screen), node_of(&w, &headless).as_deref()), (None, Some("a")));
        assert!(state(&w, &screen).1.contains("another hosted wire"), "{:?}", state(&w, &screen));
        w.nodes.get_mut("a").unwrap().caps.as_mut().unwrap().wm_wire.clear();
        release_clean(&mut w, &headless, 1);
        w.tick(1);
        assert_eq!(node_of(&w, &screen).as_deref(), Some("a"), "{:?}", state(&w, &screen));
        s.update_drain.clear();
        s.tick(1);
        assert_eq!(node_of(&s, &drained).as_deref(), Some("a"), "{:?}", state(&s, &drained));
    }

    #[test]
    fn a_box_holding_the_jit_wins_while_it_frees_before_a_build_and_hot_demand_is_revisited() {
        // `a` holds the dev JIT and runs a conflicting test measured at `est` min; `b` is free
        // and would build the JIT first (90 s by default)
        let setup = |est: f64| {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
            let mut r = req("running", Kind::Test, Some("a"));
            r.only = true;
            r.est_min = Some(est);
            let running = s.request(r, 0);
            s.tick(0);
            s.nodes.get_mut("a").unwrap().caps.as_mut().unwrap().jit_store = Some(std::sync::Arc::new(super::super::jitstore::Snapshot { ids: vec!["cached".into()], ..Default::default() }));
            let mut r = req("next", Kind::Test, None);
            r.loader = Some("cached".into());
            r.cost = CostClass::Medium;
            r.cost_asked = true;
            let next = s.request(r, 1);
            (s, running, next)
        };
        let (mut s, running, next) = setup(0.1);
        s.tick(1);
        assert_eq!(node_of(&s, &next), None, "a box that frees before the build is done is waited for");
        assert!(s.get(&next).unwrap().reason.starts_with(&format!("a: {JIT_WAIT}: frees in")), "{}", s.get(&next).unwrap().reason);
        assert_eq!(super::super::prof::blocker(&s.get(&next).unwrap().reason), JIT_WAIT);
        release_clean(&mut s, &running, 5_000);
        s.tick(5_000);
        assert_eq!(node_of(&s, &next).as_deref(), Some("a"));
        invariants(&s);

        // its run outlives its measure: the waiting stays under one build, then the free box builds
        let (mut s, _, next) = setup(1.0);
        s.tick(1);
        beat_all(&mut s, 40_001);
        s.tick(40_001);
        assert_eq!(node_of(&s, &next), None);
        beat_all(&mut s, 61_001);
        s.tick(61_001);
        assert_eq!(node_of(&s, &next).as_deref(), Some("b"), "waited 61 s, a measured 60 s more there: past the 90 s build");
        invariants(&s);

        let (mut s, _, next) = setup(2.0);
        s.tick(1);
        assert_eq!(node_of(&s, &next).as_deref(), Some("b"), "a box busy longer than the build is not waited for");

        // a full box holding the JIT takes it at once over a free one that would build, unless
        // its cores are oversubscribed
        for (load, on) in [(13.0, "a"), (17.0, "b")] {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
            let a = s.nodes.get_mut("a").unwrap().caps.as_mut().unwrap();
            a.load1 = load;
            a.jit_store = Some(std::sync::Arc::new(super::super::jitstore::Snapshot { ids: vec!["cached".into()], ..Default::default() }));
            let mut r = req("next", Kind::Test, None);
            r.loader = Some("cached".into());
            let next = s.request(r, 0);
            s.tick(0);
            assert_eq!(node_of(&s, &next).as_deref(), Some(on), "load {load}");
        }

        let mut s = sched(&[("hot-a", caps(16, 64.0, Role::Hot)), ("hot-b", caps(16, 64.0, Role::Hot))]);
        let mut r = req("medium", Kind::Test, Some("hot-a"));
        r.only = true;
        r.priority = Priority::Interactive;
        r.cost = CostClass::Medium;
        let medium = s.request(r, PIN_GRACE_MS);
        // Small can use either hot box, and takes its named one. The earlier medium is
        // revisited immediately, without waiting another tick for small demand to disappear.
        let mut r = req("small", Kind::Test, Some("hot-b"));
        r.since_ms = Some(0);
        let small = s.request(r, PIN_GRACE_MS);
        assert_eq!(grants(&s.tick(PIN_GRACE_MS)), vec![small.clone(), medium.clone()]);
        assert_eq!(node_of(&s, &small).as_deref(), Some("hot-b"));
        assert_eq!(node_of(&s, &medium).as_deref(), Some("hot-a"));
        invariants(&s);

        let mut s = sched(&[("hot", caps(16, 64.0, Role::Hot))]);
        s.request(overlay("running", Some("hot"), 12.0), 0);
        s.tick(0);
        let mut r = overlay("medium", Some("hot"), 8.0);
        r.cost = CostClass::Medium;
        r.priority = Priority::Interactive;
        let medium = s.request(r, 1);
        s.tick(1);
        assert_eq!(s.holds["hot"].id, medium);
        let small = s.request(overlay("small", Some("hot"), 4.0), 2);
        assert_eq!(grants(&s.tick(2)), vec![small], "a medium reservation cannot hide new small demand");
        assert_eq!(s.holds["hot"].until, HOLD_MS + 1);
        invariants(&s);
    }

    #[test]
    fn a_filtered_runs_measurement_never_caps_the_whole_suite() {
        let mut s = sched(&[("pool", caps(16, 64.0, Role::Pool))]);
        let mut full = overlay("suite", None, 4.0);
        full.class = "test:fab:bin:fab".into();
        let mut filtered = full.clone();
        filtered.class = "test:fab:bin:fab?print::slice_bench".into();
        for _ in 0..3 { s.record_resources(&filtered.class, "test", Some(0.7), None); }
        let (f, w) = (s.request(filtered.clone(), 0), s.request(full.clone(), 0));
        let ram = |s: &Sched, id: &str| s.cost_on("pool", s.get(id).unwrap()).ram_gb;
        let default = full.allocation(false).ram_gb;
        assert!((ram(&s, &f) - 0.84).abs() < 1e-9, "the filter's own peak: {}", ram(&s, &f));
        assert_eq!(ram(&s, &w), default, "the suite has no runs: its kind's default, not the filter's 0.7 GB");
        // a suite run over its ask is no failure: its peak is the next ask
        s.record_resources(&full.class, "test", Some(4.7), None);
        assert!((ram(&s, &w) - 4.7 * 1.2_f64.max(default / 4.7)).abs() < 1e-9, "{}", ram(&s, &w));
        // a filter without runs of its own asks for the whole suite's highest
        let mut other = full.clone();
        other.class = "test:fab:bin:fab?other".into();
        let o = s.request(other, 0);
        assert!((ram(&s, &o) - 4.7 * 1.2).abs() < 1e-9, "{}", ram(&s, &o));
        // and a measured ask caps nothing: only an asked RAM is a scope's MemoryMax
        let cost = crate::lease::Cost { cores: 4.0, ram_gb: 0.84, disk_gb: 1.0, gpus: 0 };
        assert!(!crate::os::lease_argv(vec!["x".into()], Some("u"), "", Some(cost), None).iter().any(|a| a.starts_with("MemoryMax")));
        assert_eq!(crate::os::lease_argv(vec!["x".into()], Some("u"), "", Some(cost), Some(2.0)).iter().any(|a| a == "MemoryMax=3G"), cfg!(target_os = "linux"));
    }

    #[test]
    fn measured_resources_replace_defaults_but_not_explicit_asks() {
        let mut s = sched(&[("mini", caps(8, 16.0, Role::Pool))]);
        let mut r = overlay("small-build", Some("mini"), 8.0);
        r.kind = Kind::Build;
        r.cores = None;
        r.class = "build:small".into();
        let first = s.request(r.clone(), 0);
        for _ in 0..2 { s.record_resources(&r.class, "build", Some(1.75), Some(2.1)); }
        assert_eq!(s.cost_on("mini", s.get(&first).unwrap()).ram_gb, 12.0);
        assert!(s.allocation_note("mini", s.get(&first).unwrap()).contains("(kind)"));
        s.record_resources(&r.class, "build", Some(1.75), Some(2.1));
        let cost = s.cost_on("mini", s.get(&first).unwrap());
        assert_eq!(cost.ram_gb, 2.1);
        assert_eq!(cost.cores, 3.0);
        let why = s.why_queued(&first, 0).unwrap().join("\n");
        assert!(why.contains("ram 2.1 GB (measured peak 1.8)") && why.contains("cores 3 (measured p90 2.1)"), "{why}");
        r.lane = "other-build".into();
        let second = s.request(r.clone(), 0);
        assert_eq!(grants(&s.tick(0)), vec![first.clone(), second], "two measured builds fit on a 16 GB Mac");
        invariants(&s);
        s.started(&first, "{}");
        let saved = s.live_lines();
        let mut back = sched(&[("mini", caps(8, 16.0, Role::Pool))]);
        back.restore(&saved[0], 1).unwrap();
        assert_eq!(back.get(&first).unwrap().cost, s.get(&first).unwrap().cost, "restart retains the granted allocation, even without measurements");

        // Only the last nine valid measurements of this class and kind count.
        s.record_resources(&r.class, "build", Some(100.0), Some(100.0));
        for _ in 0..8 { s.record_resources(&r.class, "build", Some(0.1), Some(0.1)); }
        assert_eq!(s.cost_on("mini", s.get(&first).unwrap()).cores, 8.0, "measured cores are capped at the box");
        s.record_resources(&r.class, "build", Some(0.1), Some(0.1));
        s.record_resources(&r.class, "build", Some(f64::NAN), Some(f64::INFINITY));
        s.record_resources(&r.class, "test", Some(100.0), Some(100.0));
        let cost = s.cost_on("mini", s.get(&first).unwrap());
        assert_eq!((cost.ram_gb, cost.cores), (0.5, 1.0));
        assert_eq!((s.get(&first).unwrap().cost.ram_gb, s.get(&first).unwrap().cost.cores), (2.1, 3.0), "running allocations stay frozen");

        r.lane = "explicit".into();
        r.ram_gb = Some(5.0);
        r.cores = Some(10.0);
        let explicit = s.request(r, 1);
        let cost = s.cost_on("mini", s.get(&explicit).unwrap());
        assert_eq!((cost.ram_gb, cost.cores), (5.0, 10.0), "explicit cores are not silently capped");
        assert_eq!(s.allocation_note("mini", s.get(&explicit).unwrap()), "ram 5.0 GB (asked); cores 10 (asked)");
    }

    #[test]
    fn seeded_scheduler_invariants() {
        // Local PRNG: reproducible failures, no network or extra test dependency.
        fn next(seed: &mut u64, n: usize) -> usize {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            (*seed % n as u64) as usize
        }
        let roles = [Role::Pool, Role::Hot, Role::Long, Role::Suite, Role::Bench, Role::UserWm, Role::Device];
        for scenario in 1..=12u64 {
            let mut seed = scenario * 0x9e3779b9;
            let mut s = Sched::new("test", 1);
            let mut node_roles = roles;
            for i in 1..node_roles.len() { node_roles.swap(i, next(&mut seed, i + 1)); }
            for (i, role) in node_roles.into_iter().enumerate() {
                let name = (105 + i).to_string();
                let mut c = caps([8, 16, 32][next(&mut seed, 3)], [16.0, 32.0, 64.0][next(&mut seed, 3)], role);
                if next(&mut seed, 2) == 0 { c.gpus.clear(); }
                if next(&mut seed, 2) == 0 { c.display = Some("wm".into()); }
                if next(&mut seed, 3) == 0 { c.arch = "aarch64".into(); }
                s.add_node(&name, "test", None);
                s.set_caps(&name, c, 0);
            }
            for turn in 0..24u64 {
                let now = 1_000_000 + turn * 61_000;
                let running: Vec<_> = s.leases.iter().filter(|l| l.state.live()).map(|l| l.id.clone()).collect();
                for id in running {
                    if next(&mut seed, 3) == 0 { s.release(&id, "released", now); }
                    else { s.set_info(&id, "{\"running\":\"job\"}"); }
                }
                let mut cleanup: Vec<_> = s.restoring.iter().map(|(id, (n, _, _))| (id.clone(), n.clone())).collect();
                cleanup.sort(); // HashMap iteration must not change the seeded scenario.
                for (id, name) in cleanup { if next(&mut seed, 2) == 0 { s.cleaned(&name, &id, 1); } }
                for i in 0..roles.len() {
                    let name = (105 + i).to_string();
                    let n = s.nodes.get_mut(&name).unwrap();
                    n.up = next(&mut seed, 5) != 0;
                    n.dev = Some(next(&mut seed, 5) != 0);
                    let c = n.caps.as_mut().unwrap();
                    c.hub_proto = if next(&mut seed, 8) == 0 { 0 } else { super::super::HUB_PROTO };
                    c.disk_free_gb = if next(&mut seed, 8) == 0 { 26.0 } else { 500.0 };
                    c.cpu_pct = Some(if next(&mut seed, 8) == 0 { 100.0 } else { 0.0 });
                    c.user = (next(&mut seed, 8) == 0).then(|| super::super::UserSense { present: true, idle_ms: Some(0), what: "input".into(), idle_min: 3 });
                    if next(&mut seed, 5) == 0 { s.updating.insert(name.clone(), "replacement".into()); }
                    else { s.updating.remove(&name); }
                }
                for kind in super::super::KINDS {
                    let node = (next(&mut seed, 3) != 0).then(|| (105 + next(&mut seed, roles.len())).to_string());
                    let mut r = req(&format!("lane-{}", next(&mut seed, 6)), kind, node.as_deref());
                    if kind == Kind::Ci { r.lane = CI_LANE.into(); }
                    r.only = r.node.is_some() && next(&mut seed, 2) == 0;
                    r.cost = [CostClass::Small, CostClass::Medium, CostClass::Large][next(&mut seed, 3)];
                    r.cost_asked = true;
                    r.prio = next(&mut seed, 11) as i32 - 3;
                    r.since_ms = Some(now.saturating_sub(next(&mut seed, 30) as u64 * 60_000));
                    r.priority = if next(&mut seed, 3) == 0 { Priority::Interactive } else { Priority::Background };
                    r.cores = Some([1.0, 2.0, 4.0, 8.0, 12.0][next(&mut seed, 5)]);
                    r.ram_gb = Some([2.0, 6.0, 12.0][next(&mut seed, 3)]);
                    r.whole = kind.writes() && next(&mut seed, 12) == 0;
                    if kind != Kind::Ci && kind != Kind::Timed && next(&mut seed, 2) == 0 { r.src = Src::Overlay; }
                    if r.src == Src::Overlay || next(&mut seed, 3) == 0 { r.rev = Some(format!("work@{}", ["a", "b"][next(&mut seed, 2)].repeat(40))); }
                    if next(&mut seed, 4) == 0 { r.needs = parse_needs(["gui=1", "arch=x64", "ram=32", "cores=16"][next(&mut seed, 4)]); }
                    s.request(r, now);
                }
                beat_all(&mut s, now);
                s.tick(now);
                invariants(&s);
            }
        }
    }

    #[test]
    fn only_round_trips_and_needs_a_node() {
        let mut r = req("x", Kind::Test, Some("101"));
        r.only = true;
        assert_eq!(Req::parse(&r.to_json()).unwrap(), r);
        assert!(!Req::parse("{\"lane\":\"x\",\"kind\":\"test\",\"only\":1}").unwrap().only);
        assert!(soft_pin(&req("x", Kind::Build, Some("a"))) && !soft_pin(&r) && !soft_pin(&req("x", Kind::Timed, Some("a"))) && !soft_pin(&req("x", Kind::Screen, Some("a"))));
    }

    #[test]
    fn stop_ends_only_its_lane_without_blocking() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool)), ("b", caps(64, 256.0, Role::Pool))]);
        let x = s.request(req("x", Kind::Build, Some("a")), 0);
        let mut r = req("y", Kind::Build, Some("b"));
        r.agent = Some("x".into());
        let y = s.request(r, 0);
        s.tick(0);
        assert!(s.get(&x).unwrap().state.live() && s.get(&y).unwrap().state.live());
        let queued = s.request(req("x", Kind::Build, Some("a")), 1);
        s.tick(1);
        assert_eq!(state(&s, &queued).0, "queued");
        assert_eq!(s.stop("x", 2), vec![x.clone(), queued.clone()]);
        assert!(s.blocked.is_empty());
        assert_eq!(state(&s, &x).0, "ended stopped");
        assert_eq!(state(&s, &queued).0, "ended stopped");
        assert!(s.get(&y).unwrap().state.live(), "an agent named x in lane y stays");
        let again = s.request(req("x", Kind::Build, Some("a")), 2);
        assert_eq!(state(&s, &again).0, "queued", "x may ask again at once");
        s.cleaned("a", &x, 0);
        let acts = s.tick(2);
        assert_eq!(grants(&acts), vec![again]);
        assert!(acts.iter().any(|a| matches!(a, Action::End { id, node: Some(n), how } if *id == x && n == "a" && how == "stopped")));
        assert!(acts.iter().any(|a| matches!(a, Action::End { id, node: None, how } if *id == queued && how == "stopped")));
        assert!(!acts.iter().any(|a| matches!(a, Action::End { id, .. } if *id == y)));
        assert!(acts.iter().any(|a| matches!(a, Action::Event(r) if r.sev == Sev::Info && r.line == "x stopped: 2 leases ended")));
    }
}
