//! The lease scheduler: who gets which node, and why the rest wait (DESIGN.md §10.2, §10.8).
//!
//! Pure: no IO and no clock of its own (every call takes `now` in ms). Mutators queue
//! [`Action`]s; [`Sched::tick`] places what can start and hands them all to the master, which
//! talks to the nodes and publishes the events.
//!
//! Pinned native lane leases on work boxes hold per-lane trees; CI, gates and user boxes keep the
//! shared tree and its existing exclusions. One lease at a time holds a tree, at its pin too.
//! Native capacity waits for a free tree; the executor evicts the least recently used one.
//! Overlays never hold a tree; one lane
//! serializes its different overlay pins (CI overlays are keyed per pin). Timed and whole-box leases and their queued holds exclude
//! writers and overlays. Slots select kinds (0: never); cores, the RAM budget (85% of RAM, at
//! most what the box's residents leave less [`RAM_MARGIN_SHARE`], a Mac's
//! [`MAC_RAM_MARGIN_SHARE`]) and GPUs are summed across
//! running leases, with disk kept above its floor. Hot boxes take small work
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
//! an ETA. Exclusive and interactive work may reserve one box for ten minutes (a timed request and
//! a background leg owed a box: for as long as it waits), only while running leases are its sole
//! obstacle.
//! A reservation never blocks work that fits beside its owner or is expected to end before its
//! owner could start (backfill). A box at [`WARM_BUSY`] (load, or granted cores or RAM) takes
//! work only when every box is that busy (the least busy first); below it, bonuses favor a tree
//! already at a tree lease's commits, the lane's warm target and its last node, then the less
//! busy and longest-idle box, so warmth never makes a hotspot. Timed leases run on the bench
//! boxes (role `bench`, by name) unless their needs fit only elsewhere. `--node N` runs on N or
//! waits for it, and is refused with the reason when N can never take it; it never runs elsewhere
//! (an A/B pinned to one box ran its before and afters on three, 11-03).
//! The Macs (os macos) leave the general pool: a lease lands there only with `--needs os=macos`
//! (inspect leases look at a JIT already there); a lease naming a Mac without it is refused.
//! A gpu lease that needs macOS is Metal work on the box to itself: it is a timed lease.

use super::{Caps, Cost, CostClass, Kind, Pin, Priority, Role, Src};
use crate::proto::{hub_record, Record, Sev};
use makepad_remote::json_str;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// No beat from the holder for this long: the lease expires (holders beat every 2 s). A lease
/// whose holder is a process on the master's machine (its pid in the request) is watched by that
/// pid instead, which ends it within a second of the holder's end: its beats only carry news to
/// the holder, and arrive late from a holder the OS throttles (an agent's job run at background
/// priority under load went 10 s without a beat while alive, three leases ending at once, 11-03).
pub const BEAT_EXPIRY_MS: u64 = 10_000;
/// A node hub holding more than this share of its box's RAM is a warning (once per crossing):
/// a hub needs tens of MB; node hubs reached 2.5-10 GB within an hour, unseen (11-03).
pub const HUB_HEAVY_SHARE: f64 = 0.25;
/// How long a lease a restarted master took back waits for its holder's first beat.
pub const RESTORE_GRACE_MS: u64 = 60_000;

/// How long after a master restart its own dropped queued ask keeps its place for the same work
/// asked again ([`Sched::request`]): the new master's prebuild plan asks within its first ticks.
const DROPPED_ASK_KEEP_MS: u64 = 10 * 60_000;
/// A waiting lease's priority improves by one per this much waiting (starvation guard).
pub const AGE_STEP_MS: u64 = 600_000;
/// A node whose caps go unanswered this long counts as down (polls are every 10 s).
pub const DOWN_AFTER_MS: u64 = 30_000;
/// Background legs owed a box (their run outlasted its fair share, `orch::leg`): verify's
/// correctness legs and jitlegs, asking at one under a lane's default. They reserve their box for
/// as long as they wait, so new lane work does not overtake them there (a jitlegs leg headed the
/// queue 24 min while later lane tests took its box, 11-03).
pub fn owed_leg(l: &Lease) -> bool {
    !l.req.ci() && l.req.prio == DEFAULT_PRIO - 1 && l.req.agent.as_deref().is_some_and(|a| a == "devhub-verify" || a == "devhub-jitlegs")
}

/// A wait this long becomes a warn (once).
pub const WAIT_WARN_MS: u64 = 600_000;
pub const DEFAULT_PRIO: i32 = 5;
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
/// A lease past its measure whose node found its output grown this recently is no stall
/// ([`Sched::say_stalled`]): its beats carry what its runs wrote.
const OUTPUT_FRESH_MS: u64 = 300_000;
/// The least a lease of a class with no measure yet is held to before it is a stall: its asked
/// or kind's length may be a warm run's, and a new class often builds cold first.
const UNMEASURED_RUN_MS: u64 = 300_000;
/// How long every process of a lease's runs may do nothing (under [`super::exec::IDLE_CPU_PCT`] of
/// a core, no I/O) before it says so, whatever its class's length.
pub const QUIET_STALL_MS: u64 = 600_000;

/// Under this load a box does nothing: one thread at work holds it near 1.
const BOX_IDLE_LOAD: f64 = 0.5;

/// How long a box stays near idle under a lease, with someone queued on it, before that is said.
const HOLDER_IDLE_MS: u64 = 600_000;
/// A timed run is told to yield ([`Sched::say_overruns`]) a minute in at the earliest.
const OVERRUN_MIN_MS: u64 = 60_000;
/// The idle verdict as its reader sees it: the inbox and the lease's owner read the same words.
fn idle_line(l: &Lease, node: &str, quiet_ms: u64) -> String {
    format!("{} of {} on {node}: every process idle (under {}% of a core, no I/O) for {}: hung? `devhub cancel {}` frees the box", l.tag(), l.req.lane, super::exec::IDLE_CPU_PCT, dur(quiet_ms), l.id)
}

/// What a stalled CI leg's capture gets before [`Sched::end_hung_legs`] ends it: it reads the JIT alive.
const HUNG_CAPTURE_MS: u64 = 60_000;
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
/// waiting for a screen lease or a live test to end. Running leases are never preempted. A hot Mac
/// keeps none ([`keeps_top_reserve`]).
pub const TOP_RAM_RESERVE_GB: f64 = 5.0;
/// The share of a box's RAM its leases leave free beyond what its residents (WM, services, hub,
/// kernel, tmpfs: [`Caps::ram_resident_gb`]) hold: page cache for the work's files and the
/// residents' growth between polls. A gate granted 24 GB of a 30 GB box's 85% beside 4 GB of
/// residents ran it out of memory, and the kernel's OOM kill took the box's services (11-03).
pub const RAM_MARGIN_SHARE: f64 = 0.10;
/// A Mac's share: past it macOS compresses and swaps rather than give back cache. A 16 GB Mac
/// held 13.45 GB in use (10 GB in a lease beside 3.45 GB of residents) at normal pressure with
/// no swap; 0.25 GB more went to warn and 558 MB into swap (a bench Mac, 11-04).
pub const MAC_RAM_MARGIN_SHARE: f64 = 0.16;
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
    /// exactly this node (it waits for it; never another)
    pub node: Option<String>,
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
    pub(crate) fn ci(&self) -> bool { self.kind == Kind::Ci || self.lane == CI_LANE }

    fn top(&self) -> bool { self.lane == TOP_LANE }

    /// The RAM budget (GB) of a node for this request: 85% of its RAM, and no more than its
    /// residents leave less [`RAM_MARGIN_SHARE`]; on a hot box the top lane's leases get all of it
    /// and every other lease all but [`TOP_RAM_RESERVE_GB`] ([`keeps_top_reserve`]).
    fn ram_budget(&self, c: &Caps) -> f64 {
        self.ram_share(c, c.ram_resident_gb)
    }

    /// What the box could ever give this request: its budget with nothing resident. Residents
    /// come and go (a deployment's build, an app on its WM): only this refuses a request.
    fn ram_cap(&self, c: &Caps) -> f64 {
        self.ram_share(c, 0.0)
    }

    fn ram_share(&self, c: &Caps, resident_gb: f64) -> f64 {
        let all = (c.ram_gb * 0.85).min(c.ram_gb * (1.0 - margin_share(c)) - resident_gb).max(0.0);
        if self.top() || !keeps_top_reserve(c) { all } else { (all - TOP_RAM_RESERVE_GB).max(0.0) }
    }

    /// How the box's RAM budget for this request comes about, for a wait's reason.
    fn ram_budget_why(&self, c: &Caps) -> String {
        let margin = c.ram_gb * (1.0 - margin_share(c)) - c.ram_resident_gb;
        let mut why = if margin < c.ram_gb * 0.85 {
            format!("{:.0} GB less {:.0}% and the {:.1} GB its residents hold", c.ram_gb, margin_share(c) * 100.0, c.ram_resident_gb)
        } else {
            format!("85% of {:.0} GB", c.ram_gb)
        };
        if !self.top() && keeps_top_reserve(c) {
            why += &format!(", less {TOP_RAM_RESERVE_GB:.0} GB kept for {TOP_LANE}");
        }
        why
    }

    fn exclusive(&self) -> bool { self.kind == Kind::Timed || self.whole }

    /// A box's own prebuild (its dev JIT for the current pin): upkeep of the box, which a timed
    /// lease waiting for the box would otherwise start on a stale JIT without.
    fn upkeep(&self) -> bool {
        self.kind == Kind::Build && self.node.as_deref().is_some_and(|n| self.lane.strip_prefix(super::PREBUILD_LANE_PREFIX) == Some(n))
    }

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
        self.class = if class.is_empty() { super::run_class(&self.lane, self.kind, argv) } else { class.to_string() };
    }

    fn class(&self) -> &str { if self.class.is_empty() { self.kind.as_str() } else { &self.class } }

    /// What the run declared it takes (`--est-min`).
    fn declared_ms(&self) -> Option<u64> {
        self.est_min.map(|m| (m * 60_000.0) as u64)
    }

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
    /// 5: compiled-file dev JIT inputs, a dep-info mismatch never fails; 8: each overlay lease
    /// has its own edits, an older hub lets a same-pin lease replace them (every overlay)),
    /// so a protocol bump never idles the fleet.
    fn proto_needed(&self, role: Role) -> u32 {
        match self.src {
            _ if self.warms.is_some() => 7,
            Src::Overlay => 8,
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

    /// One lease at a time in a tree, at its pin too: two of one lane at one pin shared its native
    /// tree, and one's script left the other's build half-patched (11-03). Different native lanes
    /// are independent.
    fn tree_conflicts(&self, other: &Req, role: Role) -> bool {
        self.same_tree(other, role)
    }

    pub fn new(lane: &str, kind: Kind) -> Req {
        Req { lane: makepad_devhub_conf::lane_name(lane), warms: None, kind, src: Src::Tree, live: false, cost: CostClass::default_for(kind), cost_asked: false, est_min: None, cores: None, ram_gb: None, whole: false, class: String::new(), node: None, needs: Vec::new(), since_ms: None, prio: DEFAULT_PRIO, priority: Priority::Background, rev: None, repo_dir: None, jit: None, loader: None, hint: Vec::new(), holder: None, master: false, agent: None }
    }

    pub fn to_json(&self) -> String {
        let opt = |o: &Option<String>| o.as_deref().map_or("null".to_string(), json_str);
        let needs: Vec<String> = self.needs.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        let warms = self.warms.as_deref().map_or(String::new(), |lane| format!(",\"warms\":{}", json_str(lane)));
        format!(
            "{{\"lane\":{}{warms},\"kind\":\"{}\",\"src\":\"{}\",\"live\":{},\"cost\":\"{}\",\"cost_asked\":{},\"est_min\":{},\"cores\":{},\"ram_gb\":{},\"whole\":{},\"class\":{},\"node\":{},\"needs\":{{{}}},\"prio\":{},\"priority\":\"{}\",\"rev\":{},\"repo_dir\":{},\"jit\":{},\"loader\":{},\"hint\":[{}],\"holder\":{},\"master\":{},\"agent\":{},\"since_ms\":{}}}",
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
        let lane = s("lane").map(|l| makepad_devhub_conf::lane_name(&l)).filter(|l| makepad_devhub_conf::valid_name(l)).ok_or("a lease needs a lane (a-z 0-9 - _)")?;
        let kind = s("kind").and_then(|k| Kind::parse(&k)).ok_or("unknown lease kind")?;
        let mut r = Req::new(&lane, kind);
        r.warms = v.get("warms").filter(|v| !v.is_null()).map(|v| v.as_str().map(makepad_devhub_conf::lane_name).filter(|l| makepad_devhub_conf::valid_name(l)).ok_or("warms needs a lane (a-z 0-9 - _)")).transpose()?;
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
        // GPU work on a Mac is Metal on the box to itself (its timings, its smoke runs): a timed
        // lease, the one form the Macs take it in (they offer no gpu slot)
        if r.kind == Kind::Gpu && r.requires_macos() {
            r.kind = Kind::Timed;
            if !r.cost_asked {
                r.cost = CostClass::default_for(Kind::Timed);
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

/// Whether a queue reason names lease `id` (L12 is not L123).
fn names_lease(reason: &str, id: &str) -> bool {
    reason.match_indices(id).any(|(at, id)| !reason[at + id.len()..].starts_with(|c: char| c.is_ascii_digit()))
}

const OWN_SESSION: &str = "it comes down to your own";

/// What a wait says when only its own lane can end it, naming the lease to release.
fn own_session_line(h: &Lease) -> String {
    format!("{OWN_SESSION} {} ({} on {}): `devhub release {}` frees it", h.id, h.req.kind.as_str(), h.node.as_deref().unwrap_or("its box"), h.id)
}

/// A class's walls as one figure, what a lease row shows: the median, or 7/10 of the p90 when that
/// is more, so many short runs do not hide what the class can take.
fn wall_measure(walls: &VecDeque<u64>) -> Option<u64> {
    if walls.is_empty() { return None; }
    let mut sorted: Vec<_> = walls.iter().copied().collect();
    sorted.sort_unstable();
    let p90 = sorted[(sorted.len() * 9).div_ceil(10) - 1];
    let upper = sorted[sorted.len() / 2];
    let median = if sorted.len() % 2 == 0 { let lower = sorted[sorted.len() / 2 - 1]; lower + (upper - lower) / 2 } else { upper };
    Some(median.max((p90 as u128 * 7 / 10) as u64))
}

/// How long a run of `cost` is taken to run when nothing else says ([`Sched::run_ms`]).
fn guess_ms(cost: CostClass) -> u64 {
    match cost { CostClass::Small => 30_000, CostClass::Medium => 300_000, CostClass::Large => 1_200_000 }
}

/// The evidence used at placement; retained with the grant.
#[derive(Clone, Copy, Debug)]
pub enum CostSource { Measured(u64), Asked, Kind }

impl CostSource {
    fn fallback(req: &Req) -> Self {
        if req.cost_asked || req.est_min.is_some() { Self::Asked } else { Self::Kind }
    }

    pub fn label(self) -> String {
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
    /// What a timed ask is told when nothing says how long it runs ([`Sched::guess_note`])
    guess_note: String,
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
    /// It waits behind a run past its estimate: no start can be told ([`Sched::behind_overrun`]).
    pub eta_late: bool,
    /// that run, said: who, how far into what estimate (empty: not late)
    pub eta_late_by: String,
    /// the box that wait is for
    pub eta_box: Option<String>,
    warned: bool,
    /// the node's `lease-start` answer (work, target, env)
    pub start: String,
    /// what the node says it runs, its work dir and disk use (`lease-beat` info)
    pub info: String,
    /// the most RAM its live runs held at one look so far, as its node measures it (`lease-beat`
    /// info `held_mb`): a scoped run's anonymous and shared memory, on macOS the footprint
    ram_held_gb: Option<f64>,
    /// how long its node saw every process of its runs do nothing (`info`'s `idle_s`)
    quiet_s: Option<u64>,
    /// what its runs wrote so far (`info`'s `out_bytes`), and when a beat last found it grown
    out_bytes: u64,
    out_grew_ms: Option<u64>,
    /// since when a granted gate lease runs nothing (`info`'s `running` empty): the last tick it
    /// ran something, else its grant
    idle_since: Option<u64>,
    hold_until: Option<u64>,
    /// since when it first waited for a busy box that holds its dev JIT while a box without it
    /// could take it: all such waiting together stays under one build ([`Sched::jit_box`])
    jit_wait_since: Option<u64>,
    /// A queued timed ask's catch-up ([`Sched::pin_updates`]): the deployment
    /// (`makepad,commercial`) its box moves to before it starts, the box having fallen more than
    /// [`QUEUE_BEHIND_MS`] behind; the tips of that moment
    pub deploy_pin: Option<String>,
    /// The deployment (`makepad,commercial`) of its box at its grant: what a timed run ran on,
    /// so two timings can be told apart or compared
    pub deployment: String,
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
        if !self.guess_note.is_empty() {
            self.guess_note.clone()
        } else if matches!(self.cost_source, CostSource::Measured(_)) && self.cost_class != self.req.asked_cost() {
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

    /// `L12 build@node-a` (the node once placed).
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
            "{{\"id\":{},\"deployment\":{},\"lane\":{},\"kind\":\"{}\",\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"class_note\":{},\"src\":\"{}\",\"class\":{},\"state\":\"{}\",\"node\":{},\"addr\":{},\"pos\":{},\"reason\":{},\"eta_ms\":{},\"eta_late\":{},\"eta_late_by\":{},\"eta_box\":{},\"prio\":{},\"priority\":\"{}\",\"wait_s\":{},\"how\":{},\"jit\":{},\"loader\":{},\"agent\":{},\"beat_s\":{},\"granted_s\":{},\"info\":{},\"start\":{}}}",
            json_str(&self.id),
            json_str(&self.deployment),
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
            self.eta_late,
            json_str(&self.eta_late_by),
            opt(&self.eta_box),
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
        // (not ranked yet: the moments before its first tick go to the reason that tick gives it)
        if self.state == State::Queued && now > self.queue_sample_ms && !self.reason.is_empty() {
            self.phases.sample("queued", &super::prof::blocker(&self.reason), &self.queue_boxes, self.queue_sample_ms, now);
            self.queue_sample_ms = now;
        }
    }

    /// Its wait goes on from an earlier ask of it: what that ask waited on stands, and the time
    /// since is the master's restart. Without this every restart put a queued lease's whole wait
    /// under no reason at all (29 min of one job's, 11-04), and a restart comes with every landing.
    fn wait_goes_on(&mut self, waited: super::prof::Phases, now: u64) {
        self.queue_sample_ms = waited.0.last().map_or(self.asked_ms, |s| s.end.max(self.asked_ms)).min(now);
        self.phases = waited;
        self.reason = "master restart".into();
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
            "{{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"node\":{},\"asked_ms\":{},\"granted_ms\":{},\"ended_ms\":{},\"how\":{},\"wait_ms\":{},\"setup_ms\":{},\"ship_ms\":{},\"teardown_ms\":{},\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"reported_ms\":{},\"alloc\":{{\"cores\":{},\"ram_gb\":{},\"gpus\":{}}}, {},\"req\":{}}}",
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
            self.cost.cores,
            self.cost.ram_gb,
            self.cost.gpus,
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
    /// its node makes the lease's `$DEVHUB_YIELD` file, `why` in it (`lease-yield`)
    Yield { id: String, node: String, why: String },
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
    /// its hub holds more than [`HUB_HEAVY_SHARE`] of its RAM (said once per crossing)
    hub_heavy: bool,
    /// the user's switch (nodes.conf); unset = by role
    dev: Option<bool>,
    reclaim_ms: Option<u64>,
    /// when its last lease was released (a waiting lease's handoff starts there)
    released_ms: Option<u64>,
    lat: Lat,
    /// its load at each caps round, oldest first, for [`super::panel::PRESSURE_KEEP_MS`]
    pressure: VecDeque<super::panel::Sample>,
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
    /// `handoff 0.4 s (median of 3), setup 3.1 s (of 12), job 2m10s (of 12), teardown 1.2 s (of
    /// 12)`: each median with its own samples (one count for all called a single handoff a
    /// median of 9, 11-04); empty with no samples.
    pub fn line(&self) -> String {
        let f = |name: &str, q: &VecDeque<u64>| Self::median(q).map(|m| (format!("{name} {}", if m >= 60_000 { dur(m) } else { format!("{:.1} s", m as f64 / 1000.0) }), q.len()));
        let parts: Vec<String> = [f("handoff", &self.handoff), f("setup", &self.setup), f("job", &self.job), f("teardown", &self.teardown)]
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, (m, n))| format!("{m} ({}of {n})", if i == 0 { "median " } else { "" }))
            .collect();
        parts.join(", ")
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
    /// Updates a timed run made wait there ([`Sched::update_waits`]): no new timed lease until they move.
    pub update_waits: Option<String>,
    /// The timed ask first in line there and the deployment the box stays at, or catches up to,
    /// until that ask starts ([`Sched::update_pin`]), while the tips are past it
    pub update_pin: Option<(String, String)>,
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
            "{} {} \u{b7} {} cores \u{b7} {cpu}load {:.1} \u{b7} {:.0}/{:.0} GB free ({:.1} resident) \u{b7} disk {:.0} GB free (floor {:.0})",
            c.arch, c.os, c.cores, c.load1, c.ram_avail_gb, c.ram_gb, c.ram_resident_gb, c.disk_free_gb, c.disk_floor_gb
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
        if c.hub_rss_mb > 0 {
            s.push_str(&format!(" {:.1} GB (heap {:.1})", c.hub_rss_mb as f64 / 1024.0, c.hub_heap_mb as f64 / 1024.0));
        }
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
        if let Some(why) = &self.update_waits {
            s.push_str(&format!(", update waits for its timed run: {why}"));
        }
        if let Some((lease, at)) = &self.update_pin {
            s.push_str(&format!(", updates stay at {} until {lease} (timed, first in line) starts", &at[..at.len().min(10)]));
        }
        // what its hub waits on, said while it waits (its next answer drops it)
        for stall in self.caps.iter().flat_map(|c| &c.stalls) {
            s.push_str(&format!(", stalled: {stall}"));
        }
        s
    }

    pub fn to_json(&self) -> String {
        format!(
            "{{\"node\":{},\"addr\":{},\"up\":{},\"dev\":{},\"dev_set\":{},\"role\":\"{}\",\"leases\":{},\"waiting\":{},\"draining\":{},\"user\":{},\"hold\":{},\"updating\":{},\"update_waits\":{},\"update_pin\":{},\"state\":{},\"latency\":{},\"caps_line\":{},\"caps\":{}}}",
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
            self.update_waits.as_deref().map_or("null".to_string(), json_str),
            self.update_pin.as_ref().map_or("null".to_string(), |(lease, at)| format!("{{\"lease\":{},\"at\":{}}}", json_str(lease), json_str(at))),
            json_str(&self.state_word()),
            json_str(&self.latency),
            json_str(&self.caps_line()),
            self.caps.as_ref().map_or("null".to_string(), |c| c.to_json())
        )
    }

    /// The node as [`Self::to_json`] wrote it (its words are derived again from the fields).
    pub fn parse(v: &makepad_strict_json::Value) -> Option<NodeView> {
        use makepad_strict_json::Value;
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
        let n = |k: &str| v.get(k).and_then(super::num).unwrap_or(0.0) as usize;
        Some(NodeView {
            name: s("node")?,
            addr: s("addr").unwrap_or_default(),
            up: b("up"),
            dev: b("dev"),
            dev_set: b("dev_set"),
            caps: v.get("caps").filter(|c| !c.is_null()).and_then(|c| Caps::parse(&c.to_json())),
            leases: n("leases"),
            waiting: n("waiting"),
            draining: b("draining"),
            hold: s("hold"),
            updating: s("updating"),
            update_waits: s("update_waits"),
            update_pin: v.get("update_pin").and_then(|p| Some((p.get("lease")?.as_str()?.to_string(), p.get("at")?.as_str()?.to_string()))),
            latency: s("latency").unwrap_or_default(),
        })
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

/// The measurement class of a box's own hub build ([`super::exec::build_hub`]): measured like a
/// build lease's, from its hub's restart answer ([`super::master::LeaseMaster::ledger_hub_build`]).
pub const HUB_BUILD: &str = "hub-build";

/// How far behind the landings a box stays for its timed queue before it catches up.
pub const QUEUE_BEHIND_MS: u64 = 30 * 60_000;

/// Where a box's deployment moves now, for its updater (the master's autosync, its roll).
#[derive(Debug, PartialEq)]
pub enum DeployTo {
    /// the tips: nothing pins it
    Tips,
    /// the catch-up of the timed ask first in line there ([`Sched::update_pin`]): no further
    Pin(String),
    /// nowhere: it holds what the timed ask first in line starts on, and that ask starts first
    Held { lease: String, at: String },
}

/// Where a box at deployment `deploy` moves, given its pin; `has(deploy, at)`: `deploy` holds
/// `at` (both `makepad,commercial`), so a move there would be none or one back.
pub fn deploy_to(pin: Option<(String, String)>, deploy: &str, has: impl Fn(&str, &str) -> bool) -> DeployTo {
    match pin {
        None => DeployTo::Tips,
        Some((lease, at)) if deploy == at || has(deploy, &at) => DeployTo::Held { lease, at },
        Some((_, at)) => DeployTo::Pin(at),
    }
}

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
    /// every lease that ended at or after this is in `ended`: the master's start, then just past
    /// the newest one dropped from it (older ones are in the ledger only)
    pub ended_from_ms: u64,
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
    /// the master's own queued asks its restart dropped, by (lane, box): their place (`asked_ms`)
    /// and when; asked again by the new master, the same work keeps that place ([`Self::request`])
    dropped_asks: HashMap<(String, Option<String>), (u64, u64)>,
    /// what those asks had waited on, by (lane, `asked_ms`): the ask again goes on from it
    dropped_waits: HashMap<(String, u64), super::prof::Phases>,
    cleanup_unknown: HashMap<String, String>,
    walls: HashMap<(String, String), VecDeque<u64>>,
    runs: HashMap<(String, String), VecDeque<u64>>,
    node_walls: HashMap<(String, String, String), VecDeque<u64>>,
    peaks: HashMap<(String, String), VecDeque<f64>>,
    cpu: HashMap<(String, String), VecDeque<f64>>,
    /// Interactive demand without queued holds: a timed hold cannot hide its own competition.
    interactive_boxes: BTreeMap<String, Vec<String>>,
    /// Requests this tick denies: they keep no box from other work ([`Self::timed_queued_for`]).
    denying: HashSet<String>,
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
    /// Updates that a running timed lease made wait on a node, by what waits (its deployment's
    /// apply, its hub's restart). The next timed lease starts on what the box has all the same,
    /// unless the box catches up ([`Sched::pin_updates`]): back to back, a bench box's timed queue
    /// kept a broken hub (11-03). Their askers set and clear them
    /// ([`super::master::Master::update_waits`]).
    pub update_waits: HashMap<String, BTreeMap<&'static str, String>>,
    /// The deployment the updates move to (work's and commercial main's tips,
    /// `makepad,commercial`), as the master's roll last read it: where a box that fell behind
    /// catches up to ([`Self::pin_updates`])
    pub tips: String,
    /// The recent landings, oldest first: the deployment each left and when, for how far behind
    /// a box is ([`Self::behind_ms`])
    pub landed: Vec<(String, u64)>,
    /// Live sessions between calls ([`Sched::charged`]): their app and JIT sit idle, so their box
    /// is charged their resident share; a call takes the full charge back ([`Sched::live_room`]).
    pub live_idle: HashSet<String>,
    /// Idle live sessions asked to park for the RAM a queued ask waits for
    /// ([`Sched::park_idle_live_for_ram`]): until they end, no other session parks for that RAM.
    parking_ram: HashSet<String>,
    /// Running leases already said to be past twice their measured run ([`Sched::say_stalled`]), and when.
    stalled: HashMap<String, u64>,
    /// Since when each box has been near idle ([`BOX_IDLE_LOAD`]).
    box_idle_since: HashMap<String, u64>,
    /// Running leases already said to hold a near idle box someone waits on ([`Sched::say_idle_holders`]).
    idle_said: HashSet<String>,
    /// Running timed leases told to yield their box ([`Sched::say_overruns`]): what they were told
    yielded: HashMap<String, String>,
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


/// Whether the box keeps [`TOP_RAM_RESERVE_GB`] for the top lane: a hot box, but not a Mac. The top
/// lane's boots run on Linux (`bucket::hot`) and a Mac takes only macOS work: kept there, it held
/// 5 GB of a 16 GB Mac for leases that never come, and a live session left no room (11-04).
/// The share of a box's RAM its leases leave free beyond its residents.
fn margin_share(c: &Caps) -> f64 {
    if c.os.eq_ignore_ascii_case("macos") { MAC_RAM_MARGIN_SHARE } else { RAM_MARGIN_SHARE }
}

fn keeps_top_reserve(c: &Caps) -> bool {
    c.role() == Role::Hot && !c.os.eq_ignore_ascii_case("macos")
}

/// A hot box takes small leases, and medium test/screen ones; the class can still move with measurements.
/// A hot Mac takes any work that requires macOS (devhub's own JIT prewarms on the hot Mac were refused).
/// A Mac other than a bench box, not a user's, may take a macOS timed lease when the bench Macs are
/// taken ([`Sched::spare_wait`]).
fn spare_mac(c: &Caps, l: &Lease) -> bool {
    l.req.requires_macos() && c.os.eq_ignore_ascii_case("macos") && !c.role().users()
}

fn hot_refuses(role: Role, l: &Lease) -> bool {
    role == Role::Hot && !l.req.requires_macos() && l.cost_class != CostClass::Small && !(l.cost_class == CostClass::Medium && matches!(l.req.kind, Kind::Test | Kind::Screen))
}

/// Does the node meet one `--needs` requirement?
/// What a box has for a need's key, said beside a need it fails.
fn has(c: &Caps, k: &str) -> Option<String> {
    match k {
        "arch" => Some(c.arch.clone()),
        "os" => Some(c.os.clone()),
        "role" => Some(c.role().as_str().to_string()),
        "cores" => Some(c.cores.to_string()),
        "ram" => Some(format!("{:.0}", c.ram_gb)),
        "gpu" => Some(if c.gpus.is_empty() { "none".into() } else { c.gpus.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join("+") }),
        _ => None,
    }
}

fn meets(c: &Caps, k: &str, v: &str) -> bool {
    let lv = v.to_ascii_lowercase();
    match k {
        "arch" => {
            let a = c.arch.to_ascii_lowercase();
            a == lv || (lv == "x64" && a == "x86_64") || (lv == "arm64" && a == "aarch64") || (lv == "aarch64" && a == "arm64")
        }
        "os" => c.os.eq_ignore_ascii_case(v),
        "gpu" => match lv.as_str() {
            "yes" | "on" | "true" => !c.gpus.is_empty(),
            "0" | "no" | "off" | "false" => c.gpus.is_empty(),
            // a count; a longer number is part of a name
            n => match n.parse::<usize>() {
                Ok(count) if n.len() <= 2 => c.gpus.len() >= count,
                _ => c.gpus.iter().any(|(name, _)| name.to_ascii_lowercase().contains(n)),
            },
        },
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
            ended_from_ms: 0,
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
            dropped_asks: HashMap::new(),
            dropped_waits: HashMap::new(),
            cleanup_unknown: HashMap::new(),
            walls: HashMap::new(),
            runs: HashMap::new(),
            node_walls: HashMap::new(),
            peaks: HashMap::new(),
            cpu: HashMap::new(),
            interactive_boxes: BTreeMap::new(),
            denying: HashSet::new(),
            interactive_lanes: HashSet::new(),
            live_jobs: HashSet::new(),
            resume: Vec::new(),
            updating: HashMap::new(),
            update_drain: HashSet::new(),
            update_waits: HashMap::new(),
            tips: String::new(),
            landed: Vec::new(),
            live_idle: HashSet::new(),
            parking_ram: HashSet::new(),
            stalled: HashMap::new(),
            box_idle_since: HashMap::new(),
            idle_said: HashSet::new(),
            yielded: HashMap::new(),
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
    /// class's last runs with a fifth more, from its first measured run on, so a run over its ask
    /// raises the next one: a test that peaked at 1.3 GB twice still asked its kind's 6 GB and
    /// waited 7 min for a 16 GB Mac's budget (11-04). A run that holds more than it asked is
    /// charged what it holds from its node's first look ([`Self::charged`]), and a Mac under
    /// pressure takes nothing new. A filtered test's class without any: the highest of its whole
    /// suite's (a filtered run's small peak once capped the whole suite's run: OOM-killed).
    pub(super) fn measured_ram(&self, req: &Req) -> Option<f64> {
        let peaks = |class: &str| self.peaks.get(&(class.to_string(), req.kind.as_str().to_string())).filter(|q| !q.is_empty());
        let highest = |q: &VecDeque<f64>| q.iter().copied().fold(0.0, f64::max) * 1.2;
        let base = super::base_class(req.class());
        match peaks(req.class()) {
            Some(q) => Some(highest(q)),
            None if base != req.class() => peaks(base).map(highest),
            None => None,
        }.map(|gb| gb.max(0.5))
    }

    fn node_median(&self, class: &str, kind: &str, node: &str) -> Option<u64> {
        self.node_walls.get(&(class.to_string(), kind.to_string(), node.to_string())).and_then(Lat::median)
    }

    /// What a lease's class measures: the figure its row shows ([`CostSource::Measured`]), and
    /// from its class's first run on, before that row trusts it enough to place by.
    fn measured_ms(&self, l: &Lease) -> Option<u64> {
        match l.cost_source {
            CostSource::Measured(ms) => Some(ms),
            _ => self.walls.get(&(l.req.class().to_string(), l.req.kind.as_str().to_string())).and_then(|w| wall_measure(w)),
        }
    }

    /// How long a lease's run takes, the one figure its stall check and the ETAs behind it read:
    /// what it declared (`--est-min`: only the run knows the size of its work), else what its row
    /// shows, else its asked cost's length. One box's own walls said otherwise: a `makepad test
    /// makepad-foo` that declared 25 min, its row at 11m50s, was called stalled against 1m23s of
    /// smaller runs on its box, and the lease behind it read "~2 min" for 11 min (11-04).
    fn run_ms(&self, l: &Lease) -> u64 {
        l.req.declared_ms().or_else(|| self.measured_ms(l)).unwrap_or_else(|| guess_ms(l.req.asked_cost()))
    }

    /// What a timed ask is told at its ask when nothing says how long it runs (no --est-min, its
    /// class not measured yet): the asks behind it count on its cost's guess, and the queue calls
    /// it late only past that. Said, never refused: 184 of 505 timed runs (11-03..05) ran on that
    /// guess, and 7 of the 11 that ran past twice their estimate.
    fn guess_note(&self, req: &Req) -> String {
        let measured = self.walls.get(&(req.class().to_string(), req.kind.as_str().to_string())).and_then(|w| wall_measure(w)).is_some();
        if req.kind != Kind::Timed || req.live || req.declared_ms().is_some() || measured { return String::new(); }
        let cost = req.asked_cost();
        format!("no --est-min and class {} not measured yet: the asks behind it count on {} (its cost {}'s guess); --est-min N tells them", req.class(), dur(guess_ms(cost)), cost.as_str())
    }

    /// The same run when choosing between boxes: a box's own walls say which is faster for it.
    fn placed_run_ms(&self, name: &str, l: &Lease) -> u64 {
        if l.req.declared_ms().is_some() { return self.run_ms(l); }
        self.node_median(l.req.class(), l.req.kind.as_str(), name).unwrap_or_else(|| self.run_ms(l))
    }

    fn has_loader(&self, name: &str, l: &Lease) -> bool {
        l.req.on_node() && self.nodes[name].caps.as_ref().and_then(|c| c.jit_store.as_ref()).is_some_and(|s| s.ids.iter().any(|held| l.req.loader.as_ref() == Some(held) || l.req.hint.contains(held)))
    }

    /// The JIT build a lease waits for on this box first.
    fn build_ms(&self, name: &str, l: &Lease) -> u64 {
        if l.req.on_node() && (l.req.loader.as_deref().is_some_and(|id| !id.is_empty()) || !l.req.hint.is_empty()) && !self.has_loader(name, l) {
            self.node_median("jit-build", "", name).or_else(|| self.median("jit-build", "")).unwrap_or(90_000)
        } else { 0 }
    }

    /// Missing measurements are estimates only; they never limit a run.
    fn finish_ms(&self, name: &str, l: &Lease, wait: u64) -> u64 {
        wait.saturating_add(self.build_ms(name, l)).saturating_add(self.placed_run_ms(name, l))
    }

    /// Earliest release that clears conflicts and budgets; concurrent runs overlap in time.
    fn remaining_ms(&self, name: &str, l: &Lease, cost: &Cost) -> u64 {
        let c = self.nodes[name].caps.as_ref().unwrap();
        let mut running: Vec<_> = self.live_on_node(name, &l.id).map(|r| {
            let wall = self.run_ms(r);
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
        if let Some(ms) = self.walls.get(&(req.class().to_string(), req.kind.as_str().to_string())).filter(|w| w.len() >= 3).and_then(|w| wall_measure(w)) {
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
    pub fn set_caps(&mut self, name: &str, caps: Caps, now: u64) -> bool {
        self.native_wait.remove(name);
        let sample = super::panel::Sample::of(now, &caps, self.charged_on(name));
        let n = self.nodes.entry(name.to_string()).or_default();
        n.pressure.push_back(sample);
        while n.pressure.front().is_some_and(|s| s.t_ms + super::panel::PRESSURE_KEEP_MS < now) {
            n.pressure.pop_front();
        }
        let was = n.up;
        // a box off the TSC: said once per change of its clock source (a reboot restores it)
        let clock = (n.caps.as_ref().map(|c| c.clocksource.as_str()) != Some(caps.clocksource.as_str())).then(|| super::caps::slow_clock(&caps.clocksource)).flatten();
        let heavy = caps.hub_rss_mb > 0 && caps.hub_rss_mb as f64 > caps.ram_gb * 1024.0 * HUB_HEAVY_SHARE;
        let crossed = heavy && !n.hub_heavy;
        let line = format!("node {name}: its hub holds {:.1} GB of {:.0} GB RAM (heap {:.1} GB): `devhub heap {name}` says where", caps.hub_rss_mb as f64 / 1024.0, caps.ram_gb, caps.hub_heap_mb as f64 / 1024.0);
        n.hub_heavy = heavy;
        n.caps = Some(caps);
        n.up = true;
        n.unheard_ms = None;
        if crossed {
            self.event(Sev::Warn, line, None);
        }
        if let Some(why) = clock {
            self.event(Sev::Warn, format!("{name}: {why}"), None);
        }
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

    /// Queues a request; the id (`L12`). Placement happens at the next [`Sched::tick`]. The
    /// master's own ask for work its restart's drop left queued (the same lane and box, asked again
    /// within [`DROPPED_ASK_KEEP_MS`]) keeps that ask's place and age: a box's prebuild started
    /// over at the back of the queue at each master restart (11-04).
    pub fn request(&mut self, mut req: Req, now: u64) -> String {
        if req.master {
            if let Some((asked, _)) = self.dropped_asks.remove(&(req.lane.clone(), req.node.clone())).filter(|(_, at)| now.saturating_sub(*at) <= DROPPED_ASK_KEEP_MS) {
                req.since_ms = Some(req.since_ms.map_or(asked, |t| t.min(asked)));
            }
        }
        let waited = req.since_ms.filter(|_| req.master).and_then(|t| self.dropped_waits.remove(&(req.lane.clone(), t)));
        let id = format!("L{}", self.next);
        self.next += 1;
        let id = self.request_as(id, req, now);
        if let (Some(waited), Some(l)) = (waited, self.leases.iter_mut().find(|l| l.id == id)) {
            l.wait_goes_on(waited, now);
        }
        id
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
        let guess_note = self.guess_note(&req);
        let asked_ms = req.since_ms.map_or(now, |t| t.min(now));
        let l = Lease {
            id: id.clone(),
            cost: req.allocation(false),
            cost_class,
            cost_source,
            class_notified: false,
            guess_note,
            priority: req.priority,
            asked_ms,
            req,
            state: State::Queued,
            node: None,
            granted_ms: None,
            ended_ms: None,
            beat_ms: now,
            pos: 0,
            reason: String::new(),
            eta_ms: 0,
            eta_late: false,
            eta_late_by: String::new(),
            eta_box: None,
            warned: false,
            start: String::new(),
            info: String::new(),
            ram_held_gb: None,
            quiet_s: None,
            out_bytes: 0,
            out_grew_ms: None,
            idle_since: None,
            hold_until: None,
            jit_wait_since: None,
            deploy_pin: None,
            deployment: String::new(),
            wait_ms: None,
            setup_ms: None,
            ship_ms: None,
            teardown_ms: None,
            phases: Default::default(),
            node_phases: Default::default(),
            reported_ms: None,
            // (asked again by a new master: the ledger's queue time counts from the first ask)
            // (`asked_ms` is its place: what it waited on before this ask is another lease's record)
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
            ("holder-dead", _) if l.req.master => format!("{} of {} released: its run ended with the previous master{}", l.tag(), l.req.lane, if node.is_some() { "; cleaning up" } else { "" }),
            ("holder-dead", _) => format!("{} of {} released: its holder (pid {}) ended{}", l.tag(), l.req.lane, l.req.holder.unwrap_or(0), if node.is_some() { "; cleaning up" } else { " (was queued)" }),
            (_, Some(h)) => format!("{} of {} ended ({how}{}) after {}", l.tag(), l.req.lane, if why.is_empty() { String::new() } else { format!(": {why}") }, dur(h)),
            (_, None) => format!("{} for {} left the queue ({how}{})", l.tag(), l.req.lane, if why.is_empty() { String::new() } else { format!(": {why}") }),
        };
        self.event(sev, line, Some(&l));
        if let Some(n) = node.as_ref().filter(|_| l.req.on_node()) {
            // until its cleanup is confirmed it holds what it held running
            let held = self.charged(&l);
            self.restoring.insert(l.id.clone(), (n.clone(), l.req.clone(), held));
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
            if let Some(e) = self.ended.pop_front().and_then(|l| l.ended_ms) {
                self.ended_from_ms = self.ended_from_ms.max(e + 1);
            }
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

    /// A node's look found this lease took its disk: the lease ends, and that is the verdict on
    /// what it ran, as a hang is. Placed again, it would do the same on the next box.
    pub fn end_for_disk(&mut self, id: &str, why: &str, now: u64) {
        if let Some(i) = self.leases.iter().position(|l| l.id == id && l.state.live()) {
            self.end_at(i, "disk", Sev::Err, why, now);
        }
    }

    /// A queued request whose target moved (a newer landing supersedes the commit a prebuild or
    /// prewarm asked for) takes its new pin and JIT hint in place: it keeps its id, its place in
    /// the queue and its age, so its wait is the job's. Released and asked again, a box's prebuild
    /// started over at the back of the queue each landing, ten times an evening, and never ran
    /// (11-04). The rest of the request stays the queued one's (a bump keeps). A granted one
    /// keeps its grant (its node started it at its pin): Err.
    pub fn retarget(&mut self, id: &str, req: Req) -> Result<(), String> {
        let l = self.leases.iter_mut().find(|l| l.id == id).ok_or_else(|| format!("no lease {id}"))?;
        if l.state != State::Queued {
            return Err(format!("{id} is {}: only a queued request takes a new target", l.state.as_str()));
        }
        if (&req.lane, req.kind, &req.node) != (&l.req.lane, l.req.kind, &l.req.node) {
            return Err(format!("{id}: a new target is for the same lane, kind and box"));
        }
        (l.req.rev, l.req.hint) = (req.rev, req.hint);
        l.jit_wait_since = None;
        Ok(())
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

    /// A live lease of the master's own work that nothing follows after a master restart
    /// ([`super::master::LeaseMaster::end_unfollowed`]): it ends now, as its holder's death (a
    /// planned restart's expected end: info). False: not such a lease.
    pub fn end_unfollowed(&mut self, id: &str, now: u64) -> bool {
        let Some(i) = self.leases.iter().position(|l| l.id == id && l.req.master && l.state.live()) else { return false };
        // its work, asking again (granted as the previous master went, before its record named
        // the lease), keeps the ask's place and age (a jitlegs leg granted at a restart went to
        // the back of the queue, 11-04)
        let l = &self.leases[i];
        let asked = l.req.since_ms.map_or(l.asked_ms, |s| s.min(l.asked_ms));
        self.dropped_asks.insert((l.req.lane.clone(), l.req.node.clone()), (asked, now));
        self.end_at(i, "holder-dead", Sev::Info, "", now);
        true
    }

    fn blocked_of(&self, r: &Req) -> Option<String> {
        self.blocked.iter().find(|b| **b == r.lane || r.agent.as_deref() == Some(b.as_str())).cloned()
    }

    /// The node's word on a live lease (running, work, disk, and the class its first run makes:
    /// a held lease asks with none, and its stall check measured it against every class-less
    /// lease of its kind on the box, "a 15 min gate running 9 min against its measured 4 min").
    pub fn set_info(&mut self, id: &str, info: &str, now: u64) {
        if let Some(l) = self.leases.iter_mut().find(|l| l.id == id) {
            l.info = info.to_string();
            if let Ok(v) = makepad_strict_json::parse(info.as_bytes()) {
                if v.get("phases").is_some() { l.node_phases = super::prof::Phases::parse(v.get("phases")); }
                if let Some(mb) = v.get("held_mb").and_then(super::num) { l.ram_held_gb = Some(mb / 1024.0); }
                l.quiet_s = v.get("idle_s").and_then(super::num).map(|s| s as u64);
                if let Some(b) = v.get("out_bytes").and_then(super::num).map(|b| b as u64).filter(|b| *b > l.out_bytes) {
                    (l.out_bytes, l.out_grew_ms) = (b, Some(now));
                }
                if l.req.class.is_empty() {
                    if let Some(class) = v.get("class").and_then(|c| c.as_str()).filter(|c| !c.is_empty()) {
                        l.req.class = class.to_string();
                        // its row and stall check read this class now, not every class-less lease of its kind
                        let req = l.req.clone();
                        let (cost_class, cost_source) = self.classify(&req);
                        if let Some(l) = self.leases.iter_mut().find(|l| l.id == id) { (l.cost_class, l.cost_source) = (cost_class, cost_source); }
                    }
                }
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
    /// The box refused a gui lease, its display gone since its caps said: no display there until
    /// its next caps say so again.
    pub fn no_display(&mut self, node: &str) {
        if let Some(c) = self.nodes.get_mut(node).and_then(|n| n.caps.as_mut()) {
            c.display = None;
        }
    }

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
                format!("{{\"id\":{},\"state\":\"{}\",\"node\":{},\"granted_ms\":{},\"asked_ms\":{},\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"req\":{},\"start\":{},\"phases\":{},\"node_phases\":{},\"allocation\":{{\"cores\":{},\"ram_gb\":{},\"disk_gb\":{},\"gpus\":{}}},\"deploy_pin\":{},\"deployment\":{}}}", json_str(&l.id), l.state.as_str(), json_str(l.node.as_deref().unwrap_or("")), l.granted_ms.unwrap_or(0), l.asked_ms, l.cost_class.as_str(), json_str(&l.cost_source.label()), l.cost_source.ms(), req.to_json(), json_str(&l.start), l.phases.json(), l.node_phases.json(), l.cost.cores, l.cost.ram_gb, l.cost.disk_gb, l.cost.gpus, json_str(l.deploy_pin.as_deref().unwrap_or("")), json_str(&l.deployment))
            })
            // an ended lease whose node has not confirmed its cleanup holds what it held: a
            // restart keeps that, and asks the node again ([`Self::cleaning`])
            .chain(self.restoring.iter().map(|(id, (node, req, c))| {
                format!("{{\"id\":{},\"state\":\"cleaning\",\"node\":{},\"req\":{},\"allocation\":{{\"cores\":{},\"ram_gb\":{},\"disk_gb\":{},\"gpus\":{}}}}}", json_str(id), json_str(node), req.to_json(), c.cores, c.ram_gb, c.disk_gb, c.gpus)
            }))
            .collect()
    }

    /// The node of an ended lease whose cleanup is not confirmed yet, if `id` is one.
    pub fn cleaning(&self, id: &str) -> Option<&str> {
        self.restoring.get(id).map(|(n, _, _)| n.as_str())
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
            let asked = v.get("asked_ms").and_then(super::num).map(|x| x as u64);
            // the master's own ask: its asker died with the previous master, which asks again (kept,
            // it would take its box first and hold it through the grace with nothing running); the
            // ask again keeps its place
            if req.master {
                if let Some(a) = asked.or(req.since_ms) {
                    self.dropped_asks.insert((req.lane.clone(), req.node.clone()), (a, now));
                    self.dropped_waits.insert((req.lane.clone(), a), super::prof::Phases::parse(v.get("phases")));
                }
                return None;
            }
            // the queue survives: the request keeps its id, its place (`since_ms`) and its holder's
            // beat grace; the next tick places it
            self.request_as(id.clone(), req, now);
            if let Some(l) = self.leases.iter_mut().find(|l| l.id == id) {
                if let Some(a) = asked { l.asked_ms = l.asked_ms.min(a); }
                l.beat_ms = now + RESTORE_GRACE_MS;
                l.deploy_pin = v.get("deploy_pin").and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
                l.wait_goes_on(super::prof::Phases::parse(v.get("phases")), now);
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
        // a cleanup the previous master did not see confirmed: its box stays charged until the
        // node confirms it (the master asks again)
        if state == "cleaning" {
            let node = v.get("node").and_then(|x| x.as_str()).filter(|x| !x.is_empty())?.to_string();
            self.restoring.insert(id.clone(), (node, req, cost));
            return Some(id);
        }
        let l = Lease {
            id: id.clone(),
            cost,
            cost_class: v.get("cost").and_then(|v| v.as_str()).and_then(CostClass::parse).unwrap_or_else(|| req.asked_cost()),
            cost_source: v.get("measured_ms").and_then(|v| v.as_u64()).map(CostSource::Measured).unwrap_or_else(|| CostSource::fallback(&req)),
            class_notified: true,
            guess_note: String::new(),
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
            eta_late: false,
            eta_late_by: String::new(),
            eta_box: None,
            warned: false,
            start: v.get("start").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            info: String::new(),
            ram_held_gb: None,
            quiet_s: None,
            out_bytes: 0,
            out_grew_ms: None,
            idle_since: None,
            hold_until: None,
            jit_wait_since: None,
            deploy_pin: None,
            deployment: v.get("deployment").and_then(|x| x.as_str()).unwrap_or("").to_string(),
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
            let Ok(names) = self.candidates(l) else { continue };
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

    /// Its leases being set up on the node (`lease-start` sent, or sent again after a restart):
    /// the master holds them there too, though the node may not have them yet.
    pub fn starting_on(&self, node: &str) -> Vec<String> {
        self.leases.iter().filter(|l| l.state == State::Starting && l.req.on_node() && l.node.as_deref() == Some(node)).map(|l| l.id.clone()).collect()
    }

    /// The nodes the master beats: every one up, and every one it holds a lease on. A node it
    /// holds nothing on gets an empty beat: that is its claim there, so a lease left from before
    /// a restart ends ([`crate::lease::exec`]'s `master_contact`).
    pub fn beat_targets(&self) -> Vec<String> {
        let mut v: Vec<String> = self.leases.iter().filter(|l| l.state == State::Granted).filter_map(|l| l.node.clone()).chain(self.nodes.iter().filter(|(_, n)| n.up).map(|(name, _)| name.clone())).collect();
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
                    update_waits: self.update_waits.get(name).map(|w| w.values().cloned().collect::<Vec<_>>().join("; ")),
                    update_pin: self.update_pin(name).filter(|(_, at)| *at != self.tips),
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

        if k == Kind::Timed && c.role() != Role::Bench && self.bench_meets(l) && !spare_mac(c, l) {
            return Err(format!("timed leases run on the bench boxes ({})", self.bench_nodes().join(", ")));
        }
        if k == Kind::Timed && !c.timed {
            return Err("takes no timed leases".into());
        }
        if let Some(why) = super::caps::slow_clock(&c.clocksource).filter(|_| k == Kind::Timed) {
            return Err(why);
        }
        // The bench boxes keep their quiet windows for timed work: other work runs there only by
        // name, or when no pool box could ever take it (needs only a bench box meets, every pool
        // box down). A gate there left a timed series no box (11-03); tests, builds and hosted
        // apps there held timed work 20-50 lease-min an hour and made its runs not quiet.
        if k != Kind::Timed && k != Kind::Inspect && c.role() == Role::Bench && l.req.node.as_deref() != Some(name) && self.pool_takes(l) {
            return Err("a bench box: work runs on the pool boxes, the bench boxes keep their quiet windows for timed work".into());
        }
        if k != Kind::Inspect && slots(c, k) == 0 {
            return Err(format!("takes no {} leases", k.as_str()));
        }
        // the user's own desktop: a lease that does not name it would run (and inject input) under
        // the person, sensed or not (2026-10-30)
        if l.req.on_node() && c.role().users() && l.req.node.as_deref() != Some(name) {
            return Err("the user's WM: only a lease that names it".into());
        }
        for (key, v) in &l.req.needs {
            if !meets(c, key, v) {
                return Err(match has(c, key) { Some(h) => format!("needs {key}={v}, it has {key}={h}"), None => format!("needs {key}={v}") });
            }
        }
        // a Windows node runs no shell scripts and no Unix tools: only what asks for it
        if c.os.eq_ignore_ascii_case("windows") && l.req.node.is_none() && !l.req.needs.iter().any(|(k, v)| k == "os" && v.eq_ignore_ascii_case("windows")) {
            return Err("windows: only leases that name it (--node or --needs os=windows)".into());
        }
        if cost.cores > c.cores as f64 {
            return Err(format!("needs {:.0} cores, has {}", cost.cores, c.cores));
        }
        if cost.ram_gb > l.req.ram_cap(c) {
            return Err(format!("needs {:.0} GB RAM, may use {:.1} of its {:.0} GB", cost.ram_gb, l.req.ram_cap(c), c.ram_gb));
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
        if let Some(gb) = self.unasked_ram(name, l) {
            // an unasked need (the kind's default, or the highest peak pooled over every box's
            // leases of the class) takes all of a smaller box's budget, never a denial: a 16 GB
            // Mac's prebuild against a build's 12 GB default or Linux builds' peaks.
            // Placement takes a box that holds it whole first ([`Self::ram_cut`]).
            cost.ram_gb = self.nodes[name].caps.as_ref().map_or(gb, |c| gb.min(l.req.ram_budget(c)));
        }
        if l.req.cores.is_none() {
            if let Some(p90) = Self::p90(&self.cpu, &l.req) {
                cost.cores = p90.ceil().max(1.0).min(self.nodes[name].caps.as_ref().map_or(f64::INFINITY, |c| c.cores as f64));
            }
        }
        cost
    }

    /// The RAM need of a request that asks none (`--ram-gb`) before a box's budget cuts it: its
    /// class's measured peaks, else its kind's default. None: it asked.
    fn unasked_ram(&self, name: &str, l: &Lease) -> Option<f64> {
        if l.req.ram_gb.is_some() { return None; }
        let default = l.req.allocation(self.warm_on(name, l)).ram_gb;
        Some(self.measured_ram(&l.req).unwrap_or(default))
    }

    /// The box's budget cuts the request's unasked RAM need: it runs there on less than its class
    /// measured (a gate's 34 GB peaks on a 30 GB box).
    fn ram_cut(&self, name: &str, l: &Lease) -> bool {
        let Some(c) = self.nodes[name].caps.as_ref() else { return false };
        self.unasked_ram(name, l).is_some_and(|gb| gb > l.req.ram_budget(c))
    }

    fn allocation_note(&self, name: &str, l: &Lease) -> String {
        let cost = self.cost_on(name, l);
        let source = |asked: Option<f64>, p90: Option<f64>| {
            if asked.is_some() { "asked".into() } else { p90.map_or("kind".into(), |n| format!("measured p90 {n:.1}")) }
        };
        let default = l.req.allocation(self.warm_on(name, l)).ram_gb;
        let ram = match self.measured_ram(&l.req) {
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
                // One that names no box claims a bench box only when no other is ready: placement
                // takes those first ([`Self::foreground`]).
                if let Ok(names) = self.candidates(l) {
                    let ready: Vec<_> = names.into_iter().filter(|name| self.ready(name, l, &self.cost_on(name, l)).is_ok()).collect();
                    let bench = |name: &str| l.req.node.is_none() && self.is_bench(name);
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

    /// A spare Mac takes macOS timed work only while every bench box that meets it is taken by
    /// timed work (a timed lease runs there, or another timed request reserves it) or down: the
    /// one bench Mac queued four timed leases while another Mac could run one (11-03). Between
    /// timed leases it keeps its own role's work; a request that names it takes it.
    fn spare_wait(&self, name: &str, l: &Lease, holds: &Holds) -> Option<Block> {
        if l.req.kind != Kind::Timed || l.req.node.as_deref() == Some(name) { return None; }
        if self.nodes[name].caps.as_ref().is_none_or(|c| c.role() == Role::Bench || !self.bench_meets(l)) { return None; }
        // a bench box out of rotation (its toolchain or git fails there) takes nothing: the spare runs it
        let free: Vec<&str> = self.nodes.iter().filter(|(b, n)| n.up && n.caps.as_ref().is_some_and(|c| c.role() == Role::Bench && c.timed && c.sick.is_empty() && super::caps::slow_clock(&c.clocksource).is_none() && l.req.needs.iter().all(|(k, v)| meets(c, k, v)))
            && !self.leases.iter().any(|r| r.req.kind == Kind::Timed && r.state.live() && r.node.as_ref() == Some(*b))
            && holds.get(b.as_str()).is_none_or(|h| h.id == l.id || self.get(&h.id).is_none_or(|r| r.req.kind != Kind::Timed))).map(|(b, _)| b.as_str()).collect();
        (!free.is_empty()).then(|| Block::new(format!("a spare Mac for timed work: it waits while {} can take it", free.join(", "))))
    }

    /// Interactive, live and screen work that names no box: it takes a pool box first, a bench box
    /// only when none fits ([`Self::place`]), and never one a timed request waits for: timed work
    /// waited 50 lease-min an hour on the bench boxes behind interactive leases a pool box could
    /// have taken (11-03).
    fn foreground(&self, l: &Lease) -> bool {
        l.req.node.is_none() && l.req.kind != Kind::Timed && (l.priority == Priority::Interactive || self.is_live_job(l) || l.req.kind == Kind::Screen)
    }

    fn is_bench(&self, name: &str) -> bool {
        self.nodes[name].caps.as_ref().is_some_and(|c| c.role() == Role::Bench)
    }

    /// A queued timed request that waits for the box and drains it for `l`: it names the box, or
    /// names none and could run there, and `l` is no older ([`Self::drains_for`]).
    fn timed_queued_for(&self, name: &str, l: &Lease) -> Option<&Lease> {
        let c = self.nodes[name].caps.as_ref()?;
        self.leases.iter().find(|r| r.state == State::Queued && r.req.kind == Kind::Timed && !self.denying.contains(&r.id) && Self::drains_for(r, l) && r.req.node.as_deref().map_or_else(|| self.never(name, c, r, &self.cost_on(name, r)).is_ok(), |n| n == name))
    }

    /// The timed ask first in line for box `name`: the first queued timed request it can take,
    /// in timed order ([`Self::exclusive_order`]).
    fn first_timed(&self, name: &str) -> Option<&Lease> {
        let c = self.nodes.get(name)?.caps.as_ref()?;
        self.leases.iter().filter(|r| r.state == State::Queued && r.req.kind == Kind::Timed && !self.denying.contains(&r.id) && r.req.node.as_deref().map_or_else(|| self.never(name, c, r, &self.cost_on(name, r)).is_ok(), |n| n == name)).min_by_key(|r| Self::exclusive_order(r))
    }

    /// The timed ask first in line on `node` and the deployment (`makepad,commercial`) the box holds
    /// until that ask starts: what it has, so timed runs start at once and a queue's timings are
    /// of one deployment. Every landing moved the target of an update that went before the next
    /// timed run, and each new one restarted it: a timed ask waited 37 min at the head of its
    /// queue behind a busy landing hour (11-04). The box catches up when its queue empties (no
    /// ask, no pin) or when it fell behind ([`Self::pin_updates`]).
    pub fn update_pin(&self, node: &str) -> Option<(String, String)> {
        let q = self.first_timed(node)?;
        let at = q.deploy_pin.clone().or_else(|| self.nodes.get(node)?.caps.as_ref().map(|c| c.deploy.clone()).filter(|d| !d.is_empty()))?;
        Some((q.id.clone(), at))
    }

    /// How long `deploy` has lacked the first landing after it: its next one in [`Self::landed`],
    /// the list's oldest for a deployment older than the list.
    fn behind_ms(&self, deploy: &str, now: u64) -> u64 {
        if deploy == self.tips { return 0; }
        let lacks = match self.landed.iter().position(|(at, _)| at == deploy) {
            Some(i) => self.landed.get(i + 1),
            None => self.landed.first(),
        };
        lacks.map_or(0, |(_, ms)| now.saturating_sub(*ms))
    }

    /// A box that fell over [`QUEUE_BEHIND_MS`] behind catches up before its next timed ask: that
    /// ask pins the tips of the moment, one update, and the queue goes on from there.
    fn pin_updates(&mut self, now: u64) {
        if self.tips.is_empty() { return; }
        let behind = |n: &String| self.nodes.get(n).and_then(|node| node.caps.as_ref()).is_some_and(|c| !c.deploy.is_empty() && self.behind_ms(&c.deploy, now) > QUEUE_BEHIND_MS);
        let firsts: HashSet<String> = self.nodes.keys().filter(|n| behind(n)).filter_map(|n| self.first_timed(n)).filter(|q| q.deploy_pin.is_none()).map(|q| q.id.clone()).collect();
        for l in self.leases.iter_mut().filter(|l| firsts.contains(&l.id)) {
            l.deploy_pin = Some(self.tips.clone());
        }
    }

    /// Whether timed request `t` keeps `l` off the box it waits for. When a box frees, the oldest
    /// waiting ask that fits starts first: a timed request drains its box only against asks
    /// younger than itself (its queue place), and one already waiting keeps its place ahead of it,
    /// the timed run starting right after. A small test waited 46 min on the bench Mac behind a
    /// chain of timed leases, each new one "~1 min" from its start, so it never ended first (11-04).
    /// Timed requests keep their own order ([`Self::exclusive_order`]); a bumped one goes first.
    fn drains_for(t: &Lease, l: &Lease) -> bool {
        l.req.kind == Kind::Timed || t.req.prio <= BUMP_PRIO || (t.asked_ms, t.seq) < (l.asked_ms, l.seq)
    }

    /// The bench boxes (role `bench`), first choice first (by name): timed leases (benchmarks) run
    /// there, so timing series stay comparable and the other boxes keep building. Another box takes
    /// a timed lease only for needs no bench box meets (a GPU or an OS none of them has), or as a
    /// spare Mac ([`spare_mac`]).
    fn bench_nodes(&self) -> Vec<&str> {
        self.nodes.iter().filter(|(_, n)| n.caps.as_ref().is_some_and(|c| c.role() == Role::Bench)).map(|(name, _)| name.as_str()).collect()
    }

    /// A pool box that is up and in rotation could take the lease, however long it waits.
    fn pool_takes(&self, l: &Lease) -> bool {
        self.nodes.iter().any(|(name, n)| n.up && self.dev(name) && n.caps.as_ref().is_some_and(|c| matches!(c.role(), Role::Pool | Role::Long) && c.sick.is_empty() && self.never(name, c, l, &self.cost_on(name, l)).is_ok()))
    }

    fn bench_meets(&self, l: &Lease) -> bool {
        self.nodes.values().any(|n| n.caps.as_ref().is_some_and(|c| c.role() == Role::Bench && l.req.needs.iter().all(|(k, v)| meets(c, k, v))))
    }

    /// The box's load a lease-end measured after its teardown ([`super::caps::load_now`]): its CPU
    /// use and available RAM until its next caps round.
    pub fn fresh_load(&mut self, name: &str, cpu_pct: f64, ram_avail_gb: f64) {
        if let Some(c) = self.nodes.get_mut(name).and_then(|n| n.caps.as_mut()) {
            c.cpu_pct = Some(cpu_pct);
            c.ram_avail_gb = ram_avail_gb;
        }
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

    /// A running lease past twice the run it declared or its row shows (at least 1 min), and 5 min
    /// over it, says so, once: a hung leg holds its box unseen otherwise (a gate leg sat 40
    /// min on an app's first frame against its 13 min, 11-02); a short one's JIT rebuild is no
    /// stall (a 1 min gate step said so at 2.5 min). A class with no measure yet is held to what
    /// it asked or its kind's length, at least [`UNMEASURED_RUN_MS`]: a new class never runs
    /// unwatched (a CI test of one sat 50 min at 0% CPU, 11-04). Whatever its class, a lease whose
    /// every process did nothing for [`QUIET_STALL_MS`] says so too. Both capture why it hangs.
    /// Open-ended sessions (live, screen, inspect) are held only to a measure of their class.
    fn say_stalled(&mut self, now: u64) {
        let mut said = Vec::new();
        for l in self.leases.iter().filter(|l| l.state == State::Granted && !self.stalled.contains_key(&l.id) && !self.is_live_job(l)) {
            let Some(node) = l.node.as_deref() else { continue };
            let open = self.session(&l.req) || l.req.kind == Kind::Inspect;
            if let Some(quiet) = self.quiet_ms(l) {
                let why = format!("{} of {}: every process idle for {}", l.tag(), l.req.lane, dur(quiet));
                said.push((l.id.clone(), node.to_string(), idle_line(l, node, quiet), why));
                continue;
            }
            let script = super::lane_script(l.req.class());
            let (expected, against) = match (l.req.declared_ms(), self.measured_ms(l).filter(|_| !script)) {
                (Some(d), _) => (d.max(60_000), format!("its declared {}", dur(d))),
                (None, Some(m)) => (m.max(60_000), format!("its measured {}", dur(m))),
                (None, None) if open || script => continue,
                (None, None) => {
                    let d = self.run_ms(l).max(UNMEASURED_RUN_MS);
                    (d, format!("{} {} (its class has no measure yet)", if l.req.cost_asked { "the asked" } else { "its kind's" }, dur(d)))
                }
            };
            let ran = now.saturating_sub(l.granted_ms.unwrap_or(now));
            // its measure is its class's (the command, not the size of its work: a 426-crate
            // sweep was called stalled against a smaller sweep's 4m20s while it beat and wrote on,
            // 11-04): a lease whose output grows works
            if l.out_grew_ms.is_some_and(|t| now.saturating_sub(t) < OUTPUT_FRESH_MS) {
                continue;
            }
            if ran > (2 * expected).max(expected + STALL_SLACK_MS) {
                let why = format!("{} of {} running {} against {against} ({})", l.tag(), l.req.lane, dur(ran), l.req.class());
                said.push((l.id.clone(), node.to_string(), format!("{} of {} on {node}: running {} against {against} ({}): stalled? `devhub cancel {}` frees the box", l.tag(), l.req.lane, dur(ran), l.req.class(), l.id), why));
            }
        }
        for (id, node, line, why) in said {
            let l = self.get(&id).cloned();
            self.event(Sev::Warn, line, l.as_ref());
            self.out.push(Action::Capture { id: id.clone(), node, why });
            self.stalled.insert(id, now);
        }
    }

    /// A running lease of any kind whose box stayed near idle for [`HOLDER_IDLE_MS`] while someone
    /// is queued on it says so, once. Only said: what it waits for may be real, but with someone
    /// behind it nobody may be watching (a timed script waited on a hidden app at load 0.3 for 93
    /// min with a queue head behind it, and no measure of its class to run past, 11-05). Nobody
    /// behind it: a quiet wait is its own business.
    fn say_idle_holders(&mut self, now: u64) {
        for (name, n) in &self.nodes {
            if n.up && n.caps.as_ref().is_some_and(|c| c.load1 < BOX_IDLE_LOAD) {
                self.box_idle_since.entry(name.clone()).or_insert(now);
            } else {
                self.box_idle_since.remove(name);
            }
        }
        let said: Vec<(String, String)> = self.leases.iter().filter(|h| h.state == State::Granted && !self.idle_said.contains(&h.id) && !self.is_live_job(h)).filter_map(|h| {
            let node = h.node.as_deref()?;
            let idle = now.saturating_sub((*self.box_idle_since.get(node)?).max(h.granted_ms.unwrap_or(now)));
            if idle < HOLDER_IDLE_MS {
                return None;
            }
            let on_it = |q: &&Lease| q.state == State::Queued && (names_lease(&q.reason, &h.id) || q.eta_box.as_deref() == Some(node) || q.req.node.as_deref() == Some(node));
            let head = self.leases.iter().filter(on_it).min_by_key(|q| Self::exclusive_order(q))?;
            let load = self.nodes[node].caps.as_ref().map_or(0.0, |c| c.load1);
            Some((h.id.clone(), format!("{} of {} on {node}: the box near idle (load {load:.1}) for {} while {} of {} waits on it; if it hung, `devhub cancel {}` frees the box", h.tag(), h.req.lane, dur(idle), head.id, head.req.lane, h.id)))
        }).collect();
        for (id, line) in said {
            let l = self.get(&id).cloned();
            self.event(Sev::Warn, line, l.as_ref());
            self.idle_said.insert(id);
        }
    }

    /// A running timed lease past twice its estimate (a minute at least) while a timed ask waits on
    /// its box is told to yield, once: an inbox warning, its owner's `job wait` woken and its beat
    /// told ([`Self::owner_word`]), and its node makes its `$DEVHUB_YIELD` file
    /// ([`super::YIELD_FILE`]), so a series or bench that checks it between steps stops after the
    /// current one with what it has and releases. Its output may grow: a bench at twice its
    /// guessed 20 min wrote on while two timed asks waited 21 min behind it and its owner sat in
    /// `job wait` (11-05). Never a kill: only the run knows where its steps end.
    fn say_overruns(&mut self, now: u64) {
        let said: Vec<(String, String, String)> = self.leases.iter().filter(|h| h.state == State::Granted && h.req.kind == Kind::Timed && !self.yielded.contains_key(&h.id) && !self.is_live_job(h)).filter_map(|h| {
            let node = h.node.as_deref()?;
            let ran = now.saturating_sub(h.granted_ms?);
            if ran < (2 * self.run_ms(h)).max(OVERRUN_MIN_MS) { return None; }
            let on_it = |q: &&Lease| q.state == State::Queued && q.req.kind == Kind::Timed && (names_lease(&q.reason, &h.id) || q.eta_box.as_deref() == Some(node) || q.req.node.as_deref() == Some(node));
            let waiting: Vec<String> = self.leases.iter().filter(on_it).map(|q| format!("{} of {}", q.id, q.req.lane)).collect();
            if waiting.is_empty() { return None; }
            Some((h.id.clone(), node.to_string(), format!("{} of {} on {node}: {} into its {}, while timed {} wait{} on its box: its $DEVHUB_YIELD file now exists; stop after the current step with what you have and release, or `devhub cancel {}`", h.tag(), h.req.lane, dur(ran), self.estimate_said(h), waiting.join(", "), if waiting.len() == 1 { "s" } else { "" }, h.id)))
        }).collect();
        for (id, node, word) in said {
            let l = self.get(&id).cloned();
            self.event(Sev::Warn, word.clone(), l.as_ref());
            self.out.push(Action::Yield { id: id.clone(), node, why: word.clone() });
            self.yielded.insert(id, word);
        }
    }

    /// A CI leg whose every process did nothing for [`QUIET_STALL_MS`] ends as a failure once its
    /// stall's capture had its time: nobody drives a leg, nothing wakes a parked JIT, and it holds
    /// its tree's one lease (one hung test held all of Windows CI for 30 min, 11-04). The verdict
    /// is its idleness, never how long it ran. Other leases have someone to read the stall.
    fn end_hung_legs(&mut self, now: u64) {
        let hung: Vec<(String, u64)> = self.leases.iter()
            .filter(|l| l.state == State::Granted && l.req.ci())
            .filter(|l| self.stalled.get(&l.id).is_some_and(|said| now.saturating_sub(*said) >= HUNG_CAPTURE_MS))
            .filter_map(|l| Some((l.id.clone(), self.quiet_ms(l)?)))
            .collect();
        for (id, quiet) in hung {
            let why = format!("every process idle (under {}% of a core, no I/O) for {}", super::exec::IDLE_CPU_PCT, dur(quiet));
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "hung", Sev::Err, &why, now);
            }
        }
    }

    /// What a running lease takes of its box's budget: its cost, but a live session between
    /// calls only its resident share, [`LIVE_IDLE_CORES`] (24 idle sessions kept 16/16 cores
    /// admitted on boxes at 1-7% CPU while 23 leases waited, 11-02), and the RAM it holds.
    fn charged(&self, r: &Lease) -> Cost {
        let mut c = r.cost;
        if self.live_idle.contains(&r.id) && self.is_live_job(r) {
            c.cores = c.cores.min(LIVE_IDLE_CORES);
        }
        // the RAM it holds (the most at one look so far), not what its class or kind needs: a
        // screen session's default took 8.6 GB of a 16 GB Mac's 9 GB budget (11-03), a live Fab
        // repro holding 4.5 GB was charged its class's 11.5 GB of a 9.7 GB budget and every other
        // lease waited on that Mac, one 40 min (11-04). Its cost stands until its node's first
        // look, and when it asked its RAM (`--ram-gb`).
        if r.req.ram_gb.is_none() {
            if let Some(gb) = r.ram_held_gb { c.ram_gb = gb; }
        }
        c
    }

    /// An open-ended session (a screen lease or a live job's): it is charged and asks the RAM
    /// measured for it, not its kind's default.
    fn session(&self, req: &Req) -> bool {
        req.kind == Kind::Screen || req.live
    }

    /// What the box's running leases and the ones still being cleaned up are charged now.
    fn charged_on(&self, name: &str) -> Cost {
        let mut used = Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 0.0, gpus: 0 };
        let costs = self.live_on_node(name, "").map(|r| self.charged(r)).chain(self.restoring.values().filter(|(n, _, _)| n == name).map(|(_, _, c)| *c)).chain(self.hub_build_charge(name));
        for c in costs {
            used.cores += c.cores;
            used.ram_gb += c.ram_gb;
            used.disk_gb += c.disk_gb;
            used.gpus += c.gpus;
        }
        used
    }

    /// What a box's hub build is charged while it runs (its caps say so): a build lease of its
    /// measured size, its class's highest peak and CPU, else a build's default within the box's
    /// budget. Placement saw none of it, and leases started beside its rustc on every core (11-04).
    fn hub_build_charge(&self, name: &str) -> Option<Cost> {
        let c = self.nodes.get(name)?.caps.as_ref().filter(|c| c.hub_building)?;
        let mut req = Req::new("devhub", Kind::Build);
        req.class = HUB_BUILD.to_string();
        let mut cost = req.allocation(false);
        // (it builds in the deployment's target, not a lease tree)
        cost.disk_gb = 0.0;
        cost.ram_gb = self.measured_ram(&req).unwrap_or(cost.ram_gb).min(req.ram_budget(c));
        if let Some(p90) = Self::p90(&self.cpu, &req) { cost.cores = p90.ceil().max(1.0); }
        cost.cores = cost.cores.min(c.cores as f64);
        Some(cost)
    }

    /// Each node's pressure samples since `since`, oldest first (the panel's view).
    pub fn pressure_since(&self, since: u64) -> Vec<(String, Vec<super::panel::Sample>)> {
        self.nodes.iter().map(|(name, n)| (name.clone(), n.pressure.iter().filter(|s| s.t_ms >= since).copied().collect())).collect()
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
        if let Some(hub) = self.hub_build_charge(name) {
            cores += hub.cores;
            ram += hub.ram_gb;
        }
        // its own idle use is in the box's measure: what it is charged comes off it
        let others = self.live_on_node(name, id).next().is_some();
        let (in_use, measured) = self.cores_in_use(name, cores - l.cost.cores + self.charged(l).cores, others);
        let in_use = in_use - self.charged(l).cores;
        if in_use + l.cost.cores > c.cores as f64 {
            let how = if measured { " (the box measured busy)" } else { "" };
            return Err(format!("{name}: budget cores {}, in use {in_use:.0}{how}, asked {:.0}", c.cores, l.cost.cores));
        }
        let budget = l.req.ram_budget(c);
        if ram > budget { return Err(format!("{name}: budget ram {budget:.1} GB, in use {:.1} GB, asked {:.1} GB ({})", ram - l.cost.ram_gb, l.cost.ram_gb, l.req.ram_budget_why(c))); }
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
        // request yields to it and parks it once its call ends (a live run waited 7 min on the one box
        // with its GPU, 11-02)
        if l.state == State::Queued && self.is_live_job(l) && l.priority != Priority::Interactive && l.req.node.as_deref() != Some(name) {
            if let Some(t) = self.leases.iter().find(|r| r.state == State::Queued && r.req.kind == Kind::Timed && r.req.node.as_deref() == Some(name)) {
                return Err(Block::by(format!("{} (timed) waits for this box: live sessions go to another", t.id), &t.id));
            }
        }
        if l.state == State::Queued && self.foreground(l) && c.role() == Role::Bench {
            if let Some(t) = self.timed_queued_for(name, l) {
                return Err(Block::by(format!("a bench box {} (timed) waits for: interactive, live and screen work goes to another", t.id), &t.id));
            }
        }
        if let Some(u) = c.user.as_ref().filter(|u| u.present) {
            return Err(Block::new(format!("{} (back after {} min without input)", u.line(), u.idle_min)));
        }
        if let Some(why) = self.native_wait.get(name).filter(|_| l.req.native(c.role())) {
            return Err(Block { reason: why.clone(), blockers: Vec::new(), disk: why.contains("disk below") });
        }
        // a box whose deployment moves and builds now: timed work starts after it, not beside
        // its rustc (a node's apply waits for a running timed lease the same way, sync/serve.rs)
        if l.req.kind == Kind::Timed && c.applying {
            return Err(Block::new("its deployment applies (a sync's build): timed work starts after it"));
        }
        if l.req.kind == Kind::Timed && c.hub_building {
            return Err(Block::new("its hub builds itself (rustc on its cores): timed work starts after it"));
        }
        if self.update_drain.contains(name) {
            return Err(Block::new("its older hub restarts for its update between runs: no new lease until it has"));
        }
        // a box catching up ([`Self::pin_updates`]) takes its update first; else a timed lease
        // starts on what the box has, whatever waits for it
        if let Some(w) = self.update_waits.get(name).filter(|_| l.req.kind == Kind::Timed && l.deploy_pin.is_some()) {
            let what = w.values().cloned().collect::<Vec<_>>().join("; ");
            // the update itself waits for what runs there: that is what this ask waits on (a timed
            // ask read "catches up first" for 2 h behind a hung timed lease nothing named, 11-05)
            if let Some(r) = self.live_on_node(name, &l.id).find(|r| l.req.conflicts_on(&r.req, c.role())) {
                return Err(Block::by(format!("held by {} ({} of {}); then the box catches up first ({what}): timed work starts after it", r.id, r.req.kind.as_str(), r.req.lane), &r.id));
            }
            return Err(Block::new(format!("the box fell over 30 min behind and catches up first ({what}): timed work starts after it")));
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
            return Err(Block { reason: format!("out of rotation: {}", c.sick), blockers: Vec::new(), disk: true });
        }
        // Class can change with measurements; large work also ages onto hot boxes.
        if hot_refuses(c.role(), l) && !(l.cost_class == CostClass::Large && self.clock.saturating_sub(l.asked_ms) >= AGE_STEP_MS) {
            return Err(Block::new(format!("class {}: hot box takes small or medium test/screen; large work waits until it has waited 10 min", l.cost_class.as_str())));
        }
        if let Some(why) = super::caps::mac_pressure(c.mem_pressure).filter(|_| l.req.on_node()) {
            return Err(Block::new(why));
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
            // the cleanup's answer is what the scrub waits for: what its hub waits on, if anything
            let stalled = c.stalls.first().map_or(String::new(), |s| format!("; its hub waits: {s}"));
            return Err(Block::by(format!("scrubbing its work tree after {id}{stalled}"), id));
        }
        if running {
            if let Some(r) = self.live_on_node(name, &l.id).filter(|r| !removed.contains(&r.id)).find(|r| l.req.conflicts_on(&r.req, c.role())) {
                let turn = if l.req.same_tree(&r.req, c.role()) { ": one lease at a time in its tree" } else { "" };
                return Err(Block::by(format!("held by {} ({} of {}){turn}", r.id, r.req.kind.as_str(), r.req.lane), &r.id));
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
        let hub = self.hub_build_charge(name);
        for cost in held.iter().map(|(_, c)| c).chain(hub.as_ref()) {
            used.cores += cost.cores;
            used.ram_gb += cost.ram_gb;
            used.disk_gb += cost.disk_gb;
            used.gpus += cost.gpus;
        }
        let hub = hub.map_or(String::new(), |h| format!("; its hub builds: {:.0} cores, {:.1} GB", h.cores, h.ram_gb));
        let blockers = || { let mut ids: Vec<_> = held.iter().map(|(id, _)| (*id).clone()).collect(); ids.sort(); ids };
        // (parking `removed` frees what they were charged: the box's measure still holds their use)
        let (in_use, measured) = self.cores_in_use(name, used.cores, running && removed.is_empty() && !held.is_empty());
        let measured = if measured { format!(" (the box measured busy; charged {:.0})", used.cores) } else { String::new() };
        for (what, have, total, add) in [("cores", in_use, c.cores as f64, cost.cores), ("ram", used.ram_gb, l.req.ram_budget(c), cost.ram_gb), ("gpu", used.gpus as f64, c.gpus.len() as f64, cost.gpus as f64)] {
            if have + add > total {
                let reason = if what == "ram" {
                    format!("budget ram {total:.1} GB, in use {have:.1} GB, asked {add:.1} GB ({}){hub}", l.req.ram_budget_why(c))
                } else if what == "cores" {
                    format!("budget cores {total:.0}, in use {have:.0}{measured}, asked {add:.0}{hub}")
                } else {
                    format!("budget {what} {total:.0}, in use {have:.0}, asked {add:.0}{hub}")
                };
                return Err(Block { reason, blockers: blockers(), disk: false });
            }
        }
        let free = c.disk_free_gb - used.disk_gb;
        if free - cost.disk_gb < c.disk_floor_gb {
            return Err(Block { reason: format!("disk floor: {free:.0} GB free, needs {:.0}, floor {:.0}", cost.disk_gb, c.disk_floor_gb), blockers: blockers(), disk: true });
        }
        Ok(())
    }

    /// The cores a box's running work takes: what it was charged, or what the box measured busy
    /// (its CPU use at its last look) when that is more: a live session charged its idle share and
    /// busy all the same, a build past its -j, the box's own services. No box is granted past its
    /// cores: oversubscribing adds no throughput, slows every lease there and makes a timing
    /// verdict flaky (a jitlegs leg failed beside 44 granted cores of 32, charged 31 with three
    /// live sessions at their idle share, 11-05). `others`: work runs there; an empty box takes
    /// what fits its cores whatever its own services use, or a box whose resident runs hot never
    /// takes a lease. Whether the measure decided it.
    fn cores_in_use(&self, name: &str, charged: f64, others: bool) -> (f64, bool) {
        let measured = self.nodes[name].caps.as_ref().filter(|_| others).and_then(|c| c.cpu_pct.map(|p| p / 100.0 * c.cores as f64));
        match measured {
            Some(m) if m > charged => (m, true),
            _ => (charged, false),
        }
    }

    /// Exclusive requests drain in this order: bumped first, then interactive before background,
    /// then arrival.
    fn exclusive_order(r: &Lease) -> (bool, bool, u64, u64) {
        (r.req.prio > BUMP_PRIO, r.priority != Priority::Interactive, r.asked_ms, r.seq)
    }

    /// An exclusive request's box and expected wait there: the box's running work, then the
    /// exclusive requests ahead of it there.
    fn eta(&self, l: &Lease) -> Option<(String, u64)> {
        let name = match l.req.node.clone().filter(|n| self.nodes.contains_key(n)) {
            Some(n) => n,
            None => self.waiting_box(l, &self.holds, false)?,
        };
        self.nodes[&name].caps.as_ref()?;
        let ahead: u64 = self.leases.iter().filter(|r| r.state == State::Queued && r.req.exclusive() && r.id != l.id && Self::exclusive_order(r) < Self::exclusive_order(l)
            && (r.req.node.as_deref() == Some(name.as_str()) || self.holds.get(&name).is_some_and(|h| h.id == r.id))).map(|r| self.run_ms(r)).sum();
        let wait = self.remaining_ms(&name, l, &self.cost_on(&name, l)).saturating_add(ahead);
        Some((name, wait))
    }

    /// What `l` waits behind on `name` past its estimate, said: the run furthest past it, who
    /// holds it, how far into which estimate (`behind L1 timed@node-c of lane (AGENT), 41m30s into
    /// its 20m00s (a guess from its cost large: ...)`). [`Self::remaining_ms`] counts such a run as
    /// starting over, which serves placing work; told to the asker, the same "~20 min" stood for
    /// 2 h behind a hung lease (11-05), and "past its estimate" alone named no one while two timed
    /// leases waited 21 min behind a benchmark that ran twice its guessed 20 (11-05).
    fn behind_overrun(&self, name: &str, l: &Lease) -> Option<String> {
        let c = self.nodes[name].caps.as_ref()?;
        let ran = |r: &Lease| self.clock.saturating_sub(r.granted_ms.unwrap_or(self.clock));
        let r = self.live_on_node(name, &l.id).filter(|r| !self.is_live_job(r) && l.req.conflicts_on(&r.req, c.role()) && ran(r) >= self.run_ms(r)).max_by_key(|r| ran(r) - self.run_ms(r))?;
        let agent = r.req.agent.as_deref().map_or(String::new(), |a| format!(" ({a})"));
        Some(format!("behind {} of {}{agent}, {} into its {}", r.tag(), r.req.lane, dur(ran(r)), self.estimate_said(r)))
    }

    /// [`Self::run_ms`] and where it comes from, said: `20m00s (a guess from its cost large: no
    /// --est-min, its class not measured yet)`.
    fn estimate_said(&self, l: &Lease) -> String {
        let from = if l.req.declared_ms().is_some() {
            "its --est-min".to_string()
        } else if self.measured_ms(l).is_some() {
            "its class's measure".to_string()
        } else {
            format!("a guess from its cost {}: no --est-min, its class not measured yet", l.req.asked_cost().as_str())
        };
        format!("{} ({from})", dur(self.run_ms(l)))
    }

    /// Work that can coexist with the reservation's owner never waits for that reservation.
    fn held_by(&self, name: &str, l: &Lease, cost: &Cost, holds: &Holds) -> Option<Block> {
        if l.req.top() { return None; }
        let h = holds.get(name).filter(|h| h.id != l.id && h.until > self.clock)?;
        let owner = self.get(&h.id).filter(|r| r.state == State::Queued)?;
        if owner.req.kind == Kind::Timed && !Self::drains_for(owner, l) { return None; }
        // the box's own prebuild goes first: the timed run would start on a stale JIT without it
        // (one waited 19 min behind two timed leases that were not running yet, 11-03)
        if owner.req.kind == Kind::Timed && l.req.upkeep() && l.req.node.as_deref() == Some(name) { return None; }
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
        if owed_leg(owner) {
            return Some(Block::by(format!("held for owed {} (its run is owed a box: only work that ends first or fits beside it)", h.id), &h.id));
        }
        // A background whole-box ask holds interactive work only once it could take the box: a live
        // session waited behind a CI reservation that a 20 min run kept off the box anyway (11-04).
        // A session let in parks for it later ([`Self::park_live`]). A CI ask holds no lane work
        // either: CI gates nothing, and a shard that gets no box gives up and goes behind the rest
        // (a lane's build waited behind a CI ask that was waiting for the lane's own screen).
        if owner.req.whole && owner.priority != Priority::Interactive && (l.priority == Priority::Interactive || (owner.req.kind == Kind::Ci && l.req.kind != Kind::Ci)) && self.remaining_ms(name, owner, &reserved) > 0 { return None; }
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
            self.candidates(r).is_ok_and(|names| names.iter().any(|n| n.as_str() == name)) && self.now_fits(name, r, &self.cost_on(name, r), &demand).is_ok()
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
        if let Some(b) = self.timed_wait(name, l).or_else(|| self.spare_wait(name, l, holds)) { return Err(b); }
        if let Some(b) = self.hot_wait(name, l, holds).or_else(|| self.held_by(name, l, cost, holds)) { return Err(b); }
        self.resources(name, l, cost, true)?;
        let idle = self.nodes[name].released_ms.map_or(3_600_000, |r| self.clock.saturating_sub(r).min(3_600_000));
        Ok(idle as f64 / 36_000_000.0)
    }

    fn candidates<'a>(&'a self, l: &Lease) -> Result<Vec<&'a String>, Place> {
        let names: Vec<&String> = match &l.req.node {
            // (`--node "<a Mac> os=macos"` waited for a node of that name, then was denied as "no
            // node <a Mac> os=macos", which read as that Mac missing, 11-04)
            Some(n) if n.is_empty() || n.contains(|c: char| c.is_whitespace() || c == '=' || c == ',') => {
                return Err(Place::Deny(format!("--node `{n}` is no node name: --node takes one name (devhub nodes); needs such as os=macos go in --needs")));
            }
            Some(n) => match self.nodes.get_key_value(n) {
                Some((k, st)) => {
                    // a hot box that refuses the class never places: say so now, not after a wait
                    if st.caps.as_ref().is_some_and(|c| hot_refuses(c.role(), l)) {
                        return Err(Place::Deny(format!("--node {k}: a hot box takes small leases (and medium test/screen), this one is class {} ({}); pin a pool node, or lower --est-min/--cost if the job is small", l.cost_class.as_str(), l.cost_source.label())));
                    }
                    vec![k]
                }
                None if self.clock < self.nodes_settle_ms => return Err(Place::Wait(format!("node {n} not heard yet"), None, None, vec![n.clone()])),
                None => return Err(Place::Deny(format!("no node `{n}` (devhub nodes lists them)"))),
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
        (Self::holds_while_waiting(l) || !l.hold_until.is_some_and(|until| until <= self.clock)) && self.can_wait(name, l, holds)
    }

    /// A timed request and an owed leg keep their reservation for as long as they wait.
    fn holds_while_waiting(l: &Lease) -> bool {
        l.req.kind == Kind::Timed || owed_leg(l)
    }

    /// A reservation's end. A timed request's lasts while it waits (renewed each tick): its box
    /// drains for it, taking only work that fits beside it or ends first. One that expired after
    /// ten minutes let new leases in and the box never drained (workpool waited 23 min on a pool box
    /// while it granted six, 11-02). Interactive demand still comes first ([`Self::timed_wait`]).
    fn hold_end(&self, l: &Lease) -> u64 {
        if Self::holds_while_waiting(l) { self.clock + HOLD_MS } else { l.hold_until.unwrap_or(self.clock + HOLD_MS) }
    }

    fn can_wait(&self, name: &str, l: &Lease, holds: &Holds) -> bool {
        if l.state != State::Queued || !(l.req.exclusive() || l.priority == Priority::Interactive || owed_leg(l)) { return false; }
        if self.timed_wait(name, l).is_some() || self.spare_wait(name, l, holds).is_some() { return false; }
        let Ok(names) = self.candidates(l) else { return false };
        names.iter().any(|n| n.as_str() == name)
            && names.iter().all(|n| self.held_by(n, l, &self.cost_on(n, l), holds).is_none())
            && self.ready(name, l, &self.cost_on(name, l)).is_ok()
            && self.hot_wait(name, l, holds).is_none()
            && (l.req.exclusive() || Self::holds_while_waiting(l) || self.starts_after_ends(name, l))
    }

    /// Whether `l` could start on `name` once the running leases there with an expected end
    /// ([`Self::expected_end`]) have ended; those without one stay. Shared work that could not
    /// holds no box: other work that fits goes on meanwhile. Prebuilds blocked by a Mac's RAM
    /// budget, beside a lease past its measure, held the Mac 10 min at a time and every lease
    /// waited behind them (64 lease-min in an hour, 11-04). An exclusive request still holds (the
    /// box drains for it and parks its live jobs), and so does an owed leg (it is owed the box).
    fn starts_after_ends(&self, name: &str, l: &Lease) -> bool {
        let Some(c) = self.nodes[name].caps.as_ref() else { return true };
        let cost = self.cost_on(name, l);
        let mut stays = Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 0.0, gpus: 0 };
        for r in self.live_on_node(name, &l.id).filter(|r| self.expected_end(r).is_none()) {
            let held = self.charged(r);
            stays.cores += held.cores;
            stays.ram_gb += held.ram_gb;
            stays.gpus += held.gpus;
        }
        stays.cores + cost.cores <= c.cores as f64 && stays.ram_gb + cost.ram_gb <= l.req.ram_budget(c) && (stays.gpus + cost.gpus) as usize <= c.gpus.len()
    }

    /// In how many ms a running lease ends by its run's estimate ([`Self::run_ms`]: its class's
    /// measure, else what it asked). None: past it (a stall, or a session open longer than its
    /// class ever ran), or a live job (it runs until it is parked).
    fn expected_end(&self, r: &Lease) -> Option<u64> {
        if self.is_live_job(r) { return None; }
        let wall = self.run_ms(r);
        let elapsed = self.clock.saturating_sub(r.granted_ms?);
        (elapsed < wall).then(|| wall - elapsed)
    }

    /// The same best waiting box as placement, even after its reservation window expired.
    fn waiting_box(&self, l: &Lease, holds: &Holds, reserve: bool) -> Option<String> {
        let names = self.candidates(l).ok()?;
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
    pub fn is_live_job(&self, l: &Lease) -> bool {
        l.req.live || self.live_jobs.contains(&l.id)
    }

    /// The lease was said to run past twice its measured run ([`Self::say_stalled`]).
    pub fn is_stalled(&self, id: &str) -> bool {
        self.stalled.contains_key(id)
    }

    /// How long every process of this lease did nothing, once that says something: not of a live
    /// job or an open session, which wait for their user.
    fn quiet_ms(&self, l: &Lease) -> Option<u64> {
        if self.is_live_job(l) || self.session(&l.req) || l.req.kind == Kind::Inspect { return None; }
        l.quiet_s.map(|s| s * 1000).filter(|ms| *ms >= QUIET_STALL_MS)
    }

    /// The running lease a queue reason names, every process of it idle.
    fn hung_named(&self, reason: &str) -> Option<&Lease> {
        self.leases.iter().filter(|h| h.state == State::Granted && self.quiet_ms(h).is_some()).find(|h| names_lease(reason, &h.id))
    }

    /// The session of `l`'s own lane its wait comes down to: a running lease `reason` names, or one
    /// a queued lease it names waits for in turn. A session stays until its owner ends it, and that
    /// owner may be waiting on `l` (a lane's build waited 23 min behind a whole CI ask that waited
    /// for the lane's own screen lease, while the lane sat in `job wait`, 11-05).
    fn own_session_behind<'a>(&'a self, l: &'a Lease, reason: &'a str) -> Option<&'a Lease> {
        let mut seen = vec![l.id.as_str()];
        let mut named = vec![reason];
        while let Some(r) = named.pop() {
            for h in &self.leases {
                if seen.contains(&h.id.as_str()) || !names_lease(r, &h.id) { continue; }
                seen.push(&h.id);
                if h.state == State::Queued {
                    named.push(&h.reason);
                } else if h.state.live() && h.req.lane == l.req.lane && (self.session(&h.req) || self.is_live_job(h)) {
                    return Some(h);
                }
            }
        }
        None
    }

    /// A queued lease of `lane` waits behind a running lease whose every process idles: that
    /// lease, and the words for whoever waits (a job queued behind its lane's hung job waited for
    /// ever, its background wait with it, 11-04).
    pub fn behind_hung(&self, lane: &str) -> Option<(String, String)> {
        self.leases.iter().filter(|l| l.state == State::Queued && l.req.lane == lane).find_map(|l| {
            let h = self.hung_named(&l.reason)?;
            Some((h.id.clone(), format!("{} waits behind {}: {}", l.id, h.id, idle_line(h, h.node.as_deref()?, self.quiet_ms(h)?))))
        })
    }

    /// A queued lease of `lane` waits on the lane's own session ([`Self::own_session_behind`]): that
    /// session, and the words for whoever waits.
    pub fn behind_own(&self, lane: &str) -> Option<(String, String)> {
        self.leases.iter().filter(|l| l.state == State::Queued && l.req.lane == lane).find_map(|l| {
            let h = self.own_session_behind(l, &l.reason)?;
            Some((h.id.clone(), format!("{} waits: {}", l.id, own_session_line(h))))
        })
    }

    /// What a running lease's owner is to hear as it stands now: its idle verdict, else its yield
    /// ([`Self::say_overruns`]).
    pub fn owner_word(&self, id: &str) -> Option<String> {
        let l = self.get(id).filter(|l| l.state == State::Granted)?;
        self.quiet_ms(l).and_then(|q| Some(idle_line(l, l.node.as_deref()?, q))).or_else(|| self.yielded.get(id).cloned())
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

    /// A queued ask that waits for RAM idle live sessions hold on its box parks them: the longest
    /// quiet first, only as many as make it fit, none when all of them would not. Idle is a
    /// state, never a timer: no call or edit in flight ([`Self::live_idle`]) and its node saw its
    /// runs do nothing across a look (`quiet_s`). A later ask counts the RAM of the sessions
    /// already parking as free, so the room is made once. One lane's sessions sat 57-73 min
    /// between calls on a 16 GB Mac while other work waited 134 min for their RAM (11-04).
    fn park_idle_live_for_ram(&mut self) {
        let mut parking = self.parking_ram.clone();
        let mut parks = Vec::new();
        for i in self.queue_order() {
            let l = &self.leases[i];
            if l.state != State::Queued || !l.req.on_node() { continue; }
            let Ok(names) = self.candidates(l) else { continue };
            // the box where the fewest idle sessions make room (any box that has room already:
            // none parks)
            let mut best: Option<(String, Vec<String>, f64)> = None;
            for name in names {
                let cost = self.cost_on(name, l);
                if self.ready(name, l, &cost).is_err() { continue; }
                let mut removed: Vec<String> = parking.iter().cloned().collect();
                removed.sort();
                // it fits once the sessions already parking end, or waits on more than RAM
                match self.resources_without(name, l, &cost, true, &removed) {
                    Ok(()) => { best = None; break; }
                    Err(b) if b.reason.starts_with("budget ram") => {}
                    Err(_) => continue,
                }
                let mut idle: Vec<&Lease> = self.live_on_node(name, &l.id)
                    .filter(|r| self.is_live_job(r) && self.live_idle.contains(&r.id) && !parking.contains(&r.id) && r.quiet_s.is_some_and(|q| q > 0))
                    .collect();
                idle.sort_by(|a, b| b.quiet_s.cmp(&a.quiet_s).then_with(|| a.id.cmp(&b.id)));
                let mut take = Vec::new();
                for r in idle {
                    take.push(r.id.clone());
                    removed.push(r.id.clone());
                    if self.resources_without(name, l, &cost, true, &removed).is_ok() {
                        if best.as_ref().is_none_or(|b| take.len() < b.1.len()) { best = Some((name.clone(), take, cost.ram_gb)); }
                        break;
                    }
                }
            }
            let Some((name, take, asked)) = best else { continue };
            let held: f64 = take.iter().filter_map(|id| self.get(id)).map(|r| self.charged(r).ram_gb).sum();
            let note = format!("parked for {} ({} of {}): it waited for {asked:.1} GB of RAM on {name}, and idle sessions held {held:.1} GB", l.id, l.req.kind.as_str(), l.req.lane);
            parking.extend(take.iter().cloned());
            parks.push((take, note));
        }
        for (leases, note) in parks {
            self.parking_ram.extend(leases.iter().cloned());
            self.out.push(Action::ParkLive { leases, note });
        }
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
        let names = match self.candidates(l) { Ok(names) => names, Err(p) => return p };
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
        let foreground = self.foreground(l);
        let mut best: Option<(&String, Cost, u64, f64, bool, bool, f64, bool, bool, bool)> = None;
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
                    let on_bench = spares_bench && !warm && self.is_bench(name);
                    // interactive, live and screen work: any other box that fits first
                    let fg_bench = foreground && self.is_bench(name);
                    let bench = benches.iter().position(|b| *b == name.as_str()).map_or(0.0, |i| 10.0 * (benches.len() - i) as f64);
                    let score = score + (1.0 - busy.min(1.0)) + bench + if named { 100.0 } else { 0.0 } + if warm { WARM_BONUS } else { 0.0 } + if room && sticky == Some(*name) { STICKY_BONUS } else { 0.0 } + if at_pin { TREE_BONUS } else { 0.0 };
                    let eta = if loader { self.finish_ms(name, l, 0) } else { 0 };
                    let cold = loader && !self.has_loader(name, l);
                    // a live session leaves a box a timed request waits for to the last
                    let timed = self.is_live_job(l) && self.leases.iter().any(|r| r.state == State::Queued && r.req.kind == Kind::Timed && (r.req.node.as_deref() == Some(name.as_str()) || holds.get(name.as_str()).is_some_and(|h| h.id == r.id)));
                    // its budget would cut what the class measured: a box that holds it whole first
                    let cut = self.ram_cut(name, l);
                    // Boxes with room first (a full one only when every box is: the least busy),
                    // except that a full box holding the dev JIT still beats building it on one
                    // with room, unless its cores are oversubscribed; then ready JITs; cold boxes
                    // by ETA, then hot. Otherwise hot, then ETA. Score breaks remaining ties.
                    // A bench box comes after every other ready one with room, and one whose
                    // budget cuts the lease's measured RAM after every one that holds it.
                    if best.as_ref().is_none_or(|b| {
                        if fg_bench != b.8 { return !fg_bench; }
                        if timed != b.5 { return !timed; }
                        let (full, other_full) = (!room && !named, b.6 >= WARM_BUSY && l.req.node.as_deref() != Some(b.0.as_str()));
                        let jit_first = cold != b.4 && if cold { b.6 < 1.0 } else { busy < 1.0 };
                        if full != other_full && !jit_first { return !full; }
                        if full && other_full { return busy < b.6; }
                        let other_hot = self.nodes[b.0].caps.as_ref().is_some_and(|c| c.role() == Role::Hot) && l.cost_class == CostClass::Small;
                        if cut != b.9 { return !cut; }
                        if cold != b.4 { return !cold; }
                        if on_bench != b.7 { return !on_bench; }
                        if cold { eta < b.2 || eta == b.2 && (hot && !other_hot || hot == other_hot && score > b.3) }
                        else { hot && !other_hot || hot == other_hot && (eta < b.2 || eta == b.2 && score > b.3) }
                    }) {
                        best = Some((name, *cost, eta, score, cold, timed, busy, on_bench, fg_bench, cut));
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
        // a named box waits; one its needs meet that could start it now is the agent's choice to
        // see (a Mac test named the bench Mac behind a timed lease 13 min while the other was idle,
        // 11-04)
        let reason = match l.req.node.as_deref() {
            Some(named) => {
                let free: Vec<&str> = self.nodes.iter().filter(|(o, n)| o.as_str() != named && n.caps.as_ref().is_some_and(|c| !hot_refuses(c.role(), l)))
                    .filter(|(o, _)| self.now_fits(o, l, &self.cost_on(o, l), holds).is_ok()).map(|(o, _)| o.as_str()).collect();
                if free.is_empty() { reason } else { format!("{reason}; it named {named} (--node): {} could start it now", free.join(" ")) }
            }
            None => reason,
        };
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
        let build = self.build_ms(cold, l);
        let waited = self.clock.saturating_sub(l.jit_wait_since.unwrap_or(self.clock));
        let (name, wait) = fit.iter().filter(|(n, _)| n.as_str() != cold && self.has_loader(n, l) && !(self.foreground(l) && self.is_bench(n))).filter_map(|(n, cost)| {
            if self.ready(n, l, cost).is_err() || self.timed_wait(n, l).is_some() || self.hot_wait(n, l, holds).is_some() || self.held_by(n, l, cost, holds).is_some() { return None; }
            let b = self.resources(n, l, cost, true).err()?;
            if b.disk || b.blockers.is_empty() { return None; }
            let wait = self.remaining_ms(n, l, cost);
            (wait > 0 && waited + wait < build && self.finish_ms(n, l, wait) < self.finish_ms(cold, l, 0)).then_some((*n, wait))
        }).min_by_key(|(_, wait)| *wait)?;
        Some(Place::Wait(format!("{name}: {JIT_WAIT}: frees in {} by measure, a build on {cold} takes ~{}", dur(wait), dur(build)), None, None, vec![name.clone()]))
    }

    /// An ask older than timed request `t` that starts on `node` now, ahead of it: when a box
    /// frees, the oldest waiting ask that fits starts first ([`Self::drains_for`]).
    fn asked_first(&self, t: &Lease, node: &str, holds: &Holds) -> Option<String> {
        if t.req.kind != Kind::Timed { return None; }
        let none = HashSet::new();
        self.leases.iter().filter(|r| r.state == State::Queued && r.id != t.id && !Self::drains_for(t, r) && self.lane_block(r).is_none())
            .find(|r| matches!(self.place(r, holds, &none), Place::Grant(n, _) if n == node)).map(|r| r.id.clone())
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
            // a leg owed a box ranks with interactive work by age: newer interactive work never
            // takes the box it waits for first (a live session got a Mac the moment its hub came
            // back, ahead of a leg owed it for 31 min, 11-03)
            (!l.req.top(), over, l.req.ci(), true, l.req.kind != Kind::Gate, !landing, l.priority != Priority::Interactive && !owed_leg(l), aged, self.last_grant.get(&l.req.lane).map_or(0, |t| t + 1), l.asked_ms, l.seq)
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
        let names = match self.candidates(l) {
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

    /// Whether queued `id` is granted `node` once `gone` have ended, and is not now: what ending
    /// background work for it buys. Asked of the placement itself, so another lease in its way, a
    /// need the box lacks, a reservation or a better box all say no.
    pub fn starts_without(&mut self, id: &str, node: &str, gone: &[String], now: u64) -> bool {
        self.clock = now;
        self.refresh_interactive();
        let holds = self.valid_holds(self.holds.clone());
        let Some(l) = self.get(id).filter(|l| l.state == State::Queued).cloned() else { return false };
        let wanted = HashSet::new();
        if matches!(self.place(&l, &holds, &wanted), Place::Grant(..)) { return false; }
        let mut were = Vec::new();
        for r in self.leases.iter_mut().filter(|r| gone.contains(&r.id)) {
            were.push((r.id.clone(), std::mem::replace(&mut r.state, State::Ended(String::new()))));
        }
        let starts = matches!(self.place(&l, &holds, &wanted), Place::Grant(name, _) if name == node);
        for (id, state) in were {
            if let Some(r) = self.leases.iter_mut().find(|r| r.id == id) { r.state = state; }
        }
        starts
    }

    /// Expires silent holders, places what fits, recomputes positions and reasons; the actions
    /// for the master (grants, ends, reclaims, one event per change).
    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        { let live: HashSet<&String> = self.leases.iter().filter(|l| l.state.live()).map(|l| &l.id).collect(); self.live_idle.retain(|id| live.contains(id)); self.idle_said.retain(|id| live.contains(id)); self.yielded.retain(|id, _| live.contains(id)); self.parking_ram.retain(|id| live.contains(id)); self.stalled.retain(|id, _| live.contains(id)); }
        self.end_hung_legs(now);
        self.say_stalled(now);
        self.say_idle_holders(now);
        self.say_overruns(now);
        // Close the previous tick's wait interval once, before placement can grant or end it.
        for l in &mut self.leases { l.sample_queue(now); }
        self.clock = now;
        self.excluded.retain(|_, until| *until > now);
        // a holder watched by its pid ([`Sched::holders_gone`]) is not timed by its beats
        let expired: Vec<String> = self.leases.iter().filter(|l| l.req.holder.is_none() && now.saturating_sub(l.beat_ms) > BEAT_EXPIRY_MS).map(|l| l.id.clone()).collect();
        for id in expired {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "expired", Sev::Warn, "", now);
            }
        }
        // a gate lease holds one of the few gate slots: one that runs nothing gives it back
        let mut idle = Vec::new();
        for l in self.leases.iter_mut().filter(|l| l.req.kind == Kind::Gate && l.state.live()) {
            let running = makepad_strict_json::parse(l.info.as_bytes()).ok().and_then(|v| v.get("running").and_then(|r| r.as_str()).map(|r| !r.is_empty())).unwrap_or(false);
            // from the last tick it ran something: a 21 min run counted from its grant was "10 min
            // idle" the moment it ended, and its lease went before its holder could release it (11-05)
            if running {
                l.idle_since = Some(now);
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
        self.denying.clear();
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
                let placed = match self.place(&self.leases[i], &holds, &wanted) {
                    // exclusive work places first: an older ask that starts on its box now goes
                    // ahead of it, granted further down this round ([`Self::drains_for`])
                    Place::Grant(node, cost) => match self.asked_first(&self.leases[i], &node, &holds) {
                        Some(r) => Place::Wait(format!("waits for {r}, asked before it: it starts on {node} first"), Some(node.clone()), None, vec![node]),
                        None => Place::Grant(node, cost),
                    },
                    p => p,
                };
                match placed {
                    Place::Grant(node, cost) => {
                        let l = &mut self.leases[i];
                        l.state = State::Starting;
                        l.node = Some(node.clone());
                        l.deployment = self.nodes.get(&node).and_then(|n| n.caps.as_ref()).map_or(String::new(), |c| c.deploy.clone());
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
                        self.event(Sev::Info, format!("{} granted to {} after {}{note}", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms))), Some(&l));
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
                        // whoever queues behind a holder gone idle hears it where it reads its wait
                        let reason = match self.hung_named(&reason) {
                            Some(h) => format!("{reason}; {} looks hung (every process idle): `devhub cancel {}` frees it", h.id, h.id),
                            None => reason,
                        };
                        let own = self.own_session_behind(&self.leases[i], &reason).map(own_session_line);
                        let reason = match own {
                            Some(line) => format!("{reason}; {line}"),
                            None => reason,
                        };
                        waits.push((i, reason, boxes));
                    }
                    Place::Deny(reason) => {
                        // a denied request reserves nothing: what it kept away places again
                        changed |= self.denying.insert(self.leases[i].id.clone());
                        deny.push((self.leases[i].id.clone(), reason));
                    }
                }
            }
            if !changed { break; }
        }
        self.holds = holds;
        self.pin_updates(now);
        for i in self.queue_order() {
            if let Some(a) = self.park_live(&self.leases[i]) { self.out.push(a); }
        }
        self.park_idle_live_for_ram();
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
            let (eta_box, eta) = if self.leases[i].req.exclusive() { self.eta(&self.leases[i]).map_or((None, 0), |(n, ms)| (Some(n), ms)) } else { (None, 0) };
            let late = eta_box.as_deref().and_then(|n| self.behind_overrun(n, &self.leases[i]));
            let l = &mut self.leases[i];
            l.eta_ms = if late.is_some() { 0 } else { eta };
            l.eta_late = late.is_some();
            l.eta_late_by = late.unwrap_or_default();
            l.eta_box = eta_box;
            // Keep attribution from the final pass, even when the displayed reason is unchanged.
            l.queue_boxes = boxes;
            // not in a new master's first minute: every box is "not heard yet" for its first
            // seconds, and an ask 21 min behind a test warned that its node was missing (11-04)
            let warn = !l.warned && now >= self.nodes_settle_ms && now.saturating_sub(l.asked_ms) >= WAIT_WARN_MS;
            // only its lane can end this wait: said where the lane's manager reads too, once
            let own = reason.contains(OWN_SESSION) && !l.reason.contains(OWN_SESSION);
            if l.pos != *p || l.reason != reason || warn || (!l.class_notified && !l.class_note().is_empty()) {
                l.pos = *p;
                l.reason = reason;
                l.warned |= warn;
                changes.push((i, warn || own));
            }
        }
        for (i, warn) in changes {
            let note = self.leases[i].take_class_note();
            let l = self.leases[i].clone();
            // a box's build of its own next JIT is background upkeep: its wait starves nobody
            let upkeep = l.req.upkeep();
            let line = if warn && upkeep {
                format!("{} for {} waiting {}, the box's next JIT, a background build; a lease that needs it sooner builds it: {} in queue ({})", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms)), ordinal(l.pos), l.reason)
            } else if warn {
                let late = if l.eta_late { format!("; no expected start: {}", l.eta_late_by) } else { String::new() };
                format!("{} for {} waiting {}: {} in queue ({}){late}", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms)), ordinal(l.pos), l.reason)
            } else {
                format!("{} for {}: {} in queue ({})", l.tag(), l.req.lane, ordinal(l.pos), l.reason)
            };
            self.event(if warn && !upkeep { Sev::Warn } else { Sev::Info }, format!("{line}{note}"), Some(&l));
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

    /// An ask that waits for RAM idle live sessions hold on its box parks them by state: the
    /// longest quiet first, only as many as make it fit; never one in a call, one whose runs work,
    /// or any when all of them would not make room; the next tick parks nothing more for that RAM.
    #[test]
    fn an_ask_waiting_for_ram_parks_the_idle_live_sessions_that_hold_it() {
        // overlays: tree writers would take turns, not share the box
        let overlay = |lane: &str| {
            let mut r = req(lane, Kind::Test, None);
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r
        };
        let setup = || {
            let mut s = sched(&[("a", caps(16, 32.0, Role::Pool))]);
            let sessions: Vec<String> = (0..2).map(|i| {
                let mut r = overlay(&format!("live-W{i}"));
                (r.live, r.priority) = (true, Priority::Interactive);
                s.request(r, 0)
            }).collect();
            s.tick(0);
            s.tick(0);
            for id in &sessions { assert_eq!(node_of(&s, id).as_deref(), Some("a"), "{:?}", state(&s, id)); }
            for id in &sessions {
                s.started(id, "{}");
                s.set_info(id, r#"{"running":"makepad","held_mb":12288,"idle_s":30}"#, 0);
            }
            // W1 quiet longer: it goes first
            s.set_info(&sessions[1], r#"{"running":"makepad","held_mb":12288,"idle_s":90}"#, 0);
            s.live_idle.extend(sessions.iter().cloned());
            (s, sessions)
        };
        let ask = |s: &mut Sched, gb: f64, now: u64| {
            let mut r = overlay("waits");
            r.ram_gb = Some(gb);
            let id = s.request(r, now);
            (id.clone(), s.tick(now))
        };
        let parked = |acts: &[Action]| acts.iter().filter_map(|a| if let Action::ParkLive { leases, note } = a { Some((leases.clone(), note.clone())) } else { None }).collect::<Vec<_>>();
        // 24 GB held of a 27.2 GB budget, 8 asked: one session's 12 GB makes room
        let (mut s, sessions) = setup();
        let (waits, acts) = ask(&mut s, 8.0, 1);
        assert!(state(&s, &waits).1.contains("budget ram"), "{:?}", state(&s, &waits));
        let p = parked(&acts);
        assert_eq!(p.iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(), vec![vec![sessions[1].clone()]], "{p:?}");
        assert!(p[0].1.starts_with(&format!("parked for {waits} (test of waits): it waited for 8.0 GB of RAM on a")), "{}", p[0].1);
        assert!(parked(&s.tick(2)).is_empty(), "the session already parking makes the room: nothing more parks");
        release_clean(&mut s, &sessions[1], 3);
        s.tick(3);
        assert_eq!(node_of(&s, &waits).as_deref(), Some("a"), "{:?}", state(&s, &waits));
        // 20 asked: both sessions go
        let (mut s, sessions) = setup();
        let (_, acts) = ask(&mut s, 20.0, 1);
        assert_eq!(parked(&acts).iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(), vec![vec![sessions[1].clone(), sessions[0].clone()]]);
        // a session in a call, or whose runs work, stays; with the other alone not enough, none parks
        let (mut s, sessions) = setup();
        s.live_idle.remove(&sessions[1]);
        s.set_info(&sessions[0], r#"{"running":"makepad","held_mb":12288,"idle_s":0}"#, 0);
        let (_, acts) = ask(&mut s, 8.0, 1);
        assert!(parked(&acts).is_empty(), "{:?}", parked(&acts));
        let (mut s, sessions) = setup();
        s.live_idle.remove(&sessions[1]);
        let (_, acts) = ask(&mut s, 20.0, 1);
        assert!(parked(&acts).is_empty(), "W0 alone frees 12 GB of the 20 asked: {:?}", parked(&acts));
        let (_, acts) = ask(&mut s, 8.0, 2);
        assert_eq!(parked(&acts).iter().map(|(l, _)| l.clone()).collect::<Vec<_>>(), vec![vec![sessions[0].clone()]], "W0 alone makes room for 8 GB");
        invariants(&s);
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
        let folded = Req::parse(r#"{"lane":"SliceCPU","warms":"Warm-X","kind":"build"}"#).unwrap();
        assert_eq!((folded.lane.as_str(), folded.warms.as_deref()), ("slicecpu", Some("warm-x")), "a lane has one spelling, lower case");
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
        // the new master asks for the same work again: it keeps the dropped ask's place
        let mut again = req("two", Kind::Build, Some("a"));
        again.master = true;
        let again = s2.request(again, 100_500);
        assert_eq!(s2.get(&again).unwrap().asked_ms, 1, "{:?}", s2.get(&again).map(|l| l.asked_ms));
        let mut later = req("two", Kind::Build, Some("a"));
        later.master = true;
        let later = s2.request(later, 100_600);
        assert_eq!(s2.get(&later).unwrap().asked_ms, 100_600, "kept once, for the ask again");
    }

    /// What a queued ask waited on survives a restart, whichever way it comes back: a holder's
    /// by its id, the master's own by its ask again. The restart itself has its own name.
    #[test]
    fn a_restart_keeps_what_a_queued_ask_waited_on() {
        let waits = |s: &Sched, id: &str| s.get(id).unwrap().phases.0.iter().filter(|p| p.phase == "queued").map(|p| (p.blocker.clone(), p.start, p.end)).collect::<Vec<_>>();
        let mut s = sched(&[("a", caps(8, 32.0, Role::Pool))]);
        let fill = s.request(req("fill", Kind::Build, Some("a")), 0);
        s.tick(0);
        assert!(s.started(&fill, "{}"));
        assert_eq!(waits(&s, &fill), Vec::new(), "granted on its first tick: it waited on nothing");
        let mut own = req("own", Kind::Build, Some("a"));
        own.master = true;
        own.since_ms = Some(1);
        let first = s.request(own.clone(), 1);
        let holder = s.request(req("holder", Kind::Build, Some("a")), 2);
        beat_all(&mut s, 3);
        s.tick(3);
        beat_all(&mut s, 60_000);
        s.tick(60_000);
        let (own_before, holder_before) = (waits(&s, &first), waits(&s, &holder));
        assert!(!own_before.is_empty() && !holder_before.is_empty() && own_before[0].1 == 1, "{own_before:?} {holder_before:?}");
        let lines = s.live_lines();
        let mut s2 = sched(&[("a", caps(8, 32.0, Role::Pool))]);
        for l in &lines { s2.restore(l, 100_000); }
        let again = s2.request(own, 100_500);
        assert_ne!(again, first, "the master's own ask comes back under a new id");
        beat_all(&mut s2, 101_000);
        s2.tick(101_000);
        beat_all(&mut s2, 160_000);
        s2.tick(160_000);
        for (id, before) in [(&again, &own_before), (&holder, &holder_before)] {
            let after = waits(&s2, id);
            assert_eq!(after[..before.len()], before[..], "{id}: {after:?}");
            let gap = &after[before.len()];
            assert_eq!((gap.0.as_str(), gap.1, gap.2), ("master restart", 60_000, 101_000), "{id}: {after:?}");
            assert!(after.iter().all(|w| w.0 != "unobserved"), "{id}: {after:?}");
            assert!(after[before.len() + 1..].iter().any(|w| w.2 == 160_000), "{id}: the new master's reasons follow: {after:?}");
        }
    }

    /// A queued request whose target moves (a newer landing's prebuild) takes it in place: its
    /// id, place and age stay, its pin and JIT hint change; a granted one keeps its grant, and a
    /// new target is for the same work.
    #[test]
    fn a_queued_request_takes_a_new_target_in_place() {
        let mut s = sched(&[("a", caps(8, 32.0, Role::Pool))]);
        let fill = s.request(req("fill", Kind::Build, Some("a")), 0);
        let mut ask = overlay("prebuild-a", Some("a"), 8.0);
        ask.kind = Kind::Build;
        ask.since_ms = Some(5);
        ask.hint = vec!["old".into()];
        let queued = s.request(ask.clone(), 10);
        s.tick(10);
        assert_eq!(state(&s, &fill).0, "starting");
        assert_eq!(state(&s, &queued).0, "queued");
        let mut newer = ask.clone();
        (newer.rev, newer.hint, newer.since_ms) = (Some(format!("work@{}", "b".repeat(40))), vec!["new".into()], Some(20));
        s.retarget(&queued, newer.clone()).unwrap();
        let l = s.get(&queued).unwrap();
        assert_eq!((l.asked_ms, l.req.since_ms, l.req.hint.clone(), l.req.rev.clone()), (5, Some(5), vec!["new".to_string()], newer.rev.clone()));
        let mut elsewhere = newer.clone();
        elsewhere.node = Some("b".into());
        assert!(s.retarget(&queued, elsewhere).unwrap_err().contains("same lane, kind and box"));
        assert!(s.retarget(&fill, req("fill", Kind::Build, Some("a"))).unwrap_err().contains("only a queued request"));
        invariants(&s);
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
        assert!(s2.started(&id, "{}"));
    }

    /// The master restarts under continuous lease traffic: whatever moment it execs at, every
    /// lease comes back as it was. Queued requests keep their place, leases in setup are started
    /// again, granted ones go on, and an ended one whose cleanup its node has not confirmed still
    /// holds its box until it is, so the restart needs no quiet moment (the master waited 57 min
    /// with five landings pending for one, 11-04).
    #[test]
    fn a_restart_at_any_moment_of_lease_traffic_keeps_every_lease() {
        let boxes = [("a", caps(8, 32.0, Role::Pool)), ("b", caps(8, 32.0, Role::Pool))];
        let mut s = sched(&boxes);
        let mut mid = 0;
        for t in 0..60u64 {
            // two asks a tick; the oldest in setup starts, the oldest running ends, and every other
            // tick a node confirms a cleanup
            for n in 0..2 {
                let mut r = req(&format!("lane{}", (t * 2 + n) % 7), Kind::Test, None);
                r.cores = Some(3.0);
                s.request(r, t);
            }
            s.tick(t);
            let first = |s: &Sched, st: State| s.leases.iter().filter(|l| l.state == st).map(|l| l.id.clone()).next();
            if let Some(id) = first(&s, State::Granted) { s.release(&id, "released", t); }
            if let Some(id) = first(&s, State::Starting) { assert!(s.started(&id, "{}")); }
            if t % 2 == 0 {
                if let Some((id, node)) = s.restoring.iter().map(|(id, (n, _, _))| (id.clone(), n.clone())).next() { s.cleaned(&node, &id, 0); }
            }
            if t % 9 != 4 { continue; }
            // the master execs now: the new one takes back exactly what this one had
            let before = |s: &Sched| {
                let mut v: Vec<(String, String, Option<String>)> = s.leases.iter().map(|l| (l.id.clone(), l.state.as_str().to_string(), l.node.clone())).collect();
                v.extend(s.restoring.iter().map(|(id, (n, _, _))| (id.clone(), "cleaning".to_string(), Some(n.clone()))));
                v.sort();
                v
            };
            mid += usize::from(!s.restoring.is_empty() || s.leases.iter().any(|l| l.state == State::Starting));
            let mut s2 = sched(&boxes);
            for line in s.live_lines() { s2.restore(&line, t); }
            assert_eq!(before(&s2), before(&s), "tick {t}");
            for (id, (n, _, c)) in &s.restoring {
                assert_eq!(s2.cleaning(id), Some(n.as_str()));
                assert_eq!(s2.restoring[id].2, *c, "{id} holds what it held");
            }
            let starting: Vec<_> = s.leases.iter().filter(|l| l.state == State::Starting).map(|l| (l.id.clone(), l.node.clone().unwrap())).collect();
            assert_eq!(s2.resume, starting, "leases in setup are started again");
        }
        assert!(mid >= 5, "restarts mid-transition: {mid}");
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
        r.jit = Some("foo".into());
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
        assert_eq!(state(&s, &c).1, "budget cores 16, in use 16, asked 4");
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
        // a lane asked in upper case is the same lane: its name is folded where it enters
        let upper = s.request(native("ONE", 'a'), 1);
        assert!(grants(&s.tick(1)).is_empty(), "one lease at a time in a lane's tree, at its pin too");
        assert!(state(&s, &same).1.contains(&format!("held by {a} (test of one): one lease at a time in its tree")), "{:?}", state(&s, &same));
        assert!(state(&s, &upper).1.contains(&format!("held by {a} (test of one): one lease at a time in its tree")), "{:?}", state(&s, &upper));
        s.release(&upper, "released", 2);
        assert!(state(&s, &c).1.contains("native trees busy"));
        s.release(&same, "released", 2);
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
        let mut s = sched(&[("node-b", caps(16, 64.0, Role::Pool))]);
        let mut tree = req("tree", Kind::Build, Some("node-b"));
        tree.rev = Some(format!("work@{}", "c".repeat(40)));
        let tree = s.request(tree, 0);
        assert_eq!(grants(&s.tick(0)), vec![tree.clone()]);
        let overlay = |sha: &str| {
            let mut r = req("ci", Kind::Ci, Some("node-b"));
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", sha.repeat(40)));
            Req::parse(&r.to_json()).unwrap()
        };
        let a = s.request(overlay("a"), 1);
        let b = s.request(overlay("b"), 1);
        assert_eq!(grants(&s.tick(1)), vec![a.clone(), b.clone()]);
        assert_eq!(node_of(&s, &a), node_of(&s, &b));
        let timed = s.request(req("timed", Kind::Timed, Some("node-b")), 2);
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
                let mut s = sched(&[("node-b", caps(16, 64.0, Role::Pool))]);
                let (first, second) = if first_ci { (Kind::Ci, kind) } else { (kind, Kind::Ci) };
                let at = |lane: &str, kind: Kind, sha: char| {
                    let mut r = req(lane, kind, Some("node-b"));
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
                if kind == Kind::Gate {
                    assert!(grants(&s.tick(1)).is_empty(), "the same pin waits too: one lease at a time in the work tree");
                    assert!(state(&s, &b).1.contains("one lease at a time in its tree"), "{:?}", state(&s, &b));
                    continue;
                }
                assert_eq!(grants(&s.tick(1)), vec![b.clone()]);
                assert!(s.get(&a).unwrap().state.live() && s.get(&b).unwrap().state.live());
                assert_eq!(node_of(&s, &a), node_of(&s, &b));
            }
        }
    }

    #[test]
    fn ci_and_timed_never_share_a_box() {
        for first in [Kind::Ci, Kind::Timed] {
            let mut s = sched(&[("node-b", caps(16, 64.0, Role::Bench))]);
            let a = s.request(req("a", first, Some("node-b")), 0);
            s.tick(0);
            let b = s.request(req("b", if first == Kind::Ci { Kind::Timed } else { Kind::Ci }, Some("node-b")), 1);
            assert!(grants(&s.tick(1)).is_empty());
            assert_eq!(state(&s, &b).0, "queued");
            release_clean(&mut s, &a, 2);
            assert_eq!(grants(&s.tick(2)), vec![b]);
        }
    }

    #[test]
    fn a_timed_hold_keeps_ci_off_the_box() {
        let mut s = sched(&[("node-b", caps(16, 64.0, Role::Pool))]);
        let test = s.request(req("test", Kind::Test, Some("node-b")), 0);
        s.tick(0);
        let timed = s.request(req("bench", Kind::Timed, Some("node-b")), 1);
        s.tick(1);
        assert_eq!(s.holds.get("node-b").map(|h| &h.id), Some(&timed));
        let mut r = req("ci", Kind::Ci, Some("node-b"));
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
    fn a_holder_on_an_idle_box_is_said_once_when_someone_waits_on_it() {
        let idle_lines = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Event(r) if r.line.contains("near idle") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        s.nodes.get_mut("node-c").unwrap().caps.as_mut().unwrap().load1 = 0.3;
        let holder = s.request(req("quiet", Kind::Timed, Some("node-c")), 0);
        assert_eq!(grants(&s.tick(0)), vec![holder.clone()]);
        s.started(&holder, "{}");
        const MIN: u64 = 60_000;
        beat_all(&mut s, 30 * MIN);
        assert!(idle_lines(&s.tick(30 * MIN)).is_empty(), "a quiet wait with nobody behind it is its own business");
        let head = s.request(req("behind", Kind::Timed, Some("node-c")), 30 * MIN);
        beat_all(&mut s, 30 * MIN + 1);
        let said = idle_lines(&s.tick(30 * MIN + 1));
        assert!(said.len() == 1 && said[0].contains(&holder) && said[0].contains(&head) && said[0].contains("load 0.3"), "{said:?}");
        beat_all(&mut s, 31 * MIN);
        let later = s.tick(31 * MIN);
        assert!(idle_lines(&later).is_empty(), "once");
        assert_eq!(state(&s, &holder).0, "granted", "only said");

        // a working box, or a holder that has only just got it, is not said
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        s.nodes.get_mut("node-c").unwrap().caps.as_mut().unwrap().load1 = 1.2;
        let holder = s.request(req("works", Kind::Timed, Some("node-c")), 0);
        grants(&s.tick(0));
        s.started(&holder, "{}");
        s.request(req("behind", Kind::Timed, Some("node-c")), 0);
        beat_all(&mut s, 30 * MIN);
        assert!(idle_lines(&s.tick(30 * MIN)).is_empty(), "{holder} works");
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        s.nodes.get_mut("node-c").unwrap().caps.as_mut().unwrap().load1 = 0.3;
        s.tick(0);
        let holder = s.request(req("new", Kind::Timed, Some("node-c")), 30 * MIN);
        grants(&s.tick(30 * MIN));
        s.started(&holder, "{}");
        s.request(req("behind", Kind::Timed, Some("node-c")), 30 * MIN);
        beat_all(&mut s, 31 * MIN);
        assert!(idle_lines(&s.tick(31 * MIN)).is_empty(), "{holder} just got it");
    }

    #[test]
    fn a_timed_ask_behind_a_run_names_it_and_tells_no_start_past_its_estimate() {
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        let running = s.request(req("holder", Kind::Timed, Some("node-c")), 0);
        grants(&s.tick(0));
        s.update_waits.entry("node-c".into()).or_default().insert("deployment", "its deployment moves to abc".into());
        let timed = s.request(req("bench", Kind::Timed, Some("node-c")), 0);
        s.leases.iter_mut().find(|l| l.id == timed).unwrap().deploy_pin = Some("abc,def".into());
        s.tick(1);
        let reason = state(&s, &timed).1;
        assert!(reason.starts_with(&format!("held by {running} (timed of holder)")) && reason.contains("catches up first (its deployment moves to abc)"), "{reason}");
        assert_eq!(super::super::prof::blocker(&reason), "held by timed");
        assert!(s.get(&timed).unwrap().eta_ms > 0 && !s.get(&timed).unwrap().eta_late, "within its estimate: a start is told");
        let past = s.run_ms(s.get(&running).unwrap()) + 1;
        beat_all(&mut s, past);
        s.tick(past);
        let waits = s.get(&timed).unwrap();
        assert!(waits.eta_late && waits.eta_ms == 0, "past it: none is ({}, {})", waits.eta_late, waits.eta_ms);
        assert_eq!(waits.eta_late_by, format!("behind {running} timed@node-c of holder, {} into its {} (a guess from its cost large: no --est-min, its class not measured yet)", dur(past), dur(past - 1)), "the run past its estimate is named, with how far and what estimate");
        assert!(waits.state_line(past, "").contains(&format!("\"eta_late_by\":{}", json_str(&waits.eta_late_by))), "its row carries it to the asker");
        assert_eq!(super::super::prof::blocker("the box fell over 30 min behind and catches up first (its deployment moves to abc): timed work starts after it"), "box catches up");
    }

    #[test]
    fn a_timed_ask_nothing_estimates_is_told_its_costs_guess_at_its_ask() {
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        let guessed = s.request(req("bench", Kind::Timed, Some("node-c")), 0);
        let l = s.get(&guessed).unwrap();
        assert_eq!(l.class_note(), format!("no --est-min and class timed not measured yet: the asks behind it count on {} (its cost large's guess); --est-min N tells them", dur(s.run_ms(l))));
        assert!(l.state_line(0, "").contains("--est-min N tells them"), "its row carries it to the asker");
        let mut declared = req("bench", Kind::Timed, Some("node-c"));
        declared.est_min = Some(5.0);
        let declared = s.request(declared, 0);
        s.walls.entry(("timed".into(), "timed".into())).or_default().push_back(60_000);
        let measured = s.request(req("bench", Kind::Timed, Some("node-c")), 0);
        let test = s.request(req("t", Kind::Test, Some("node-c")), 0);
        for id in [declared, measured, test] {
            assert_eq!(s.get(&id).unwrap().class_note(), "", "{id}: a declared or measured run, or no timed one, is told nothing");
        }
    }

    #[test]
    fn a_timed_run_past_twice_its_estimate_with_a_timed_ask_on_its_box_is_told_to_yield_once() {
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        let mut r = req("bench", Kind::Timed, Some("node-c"));
        r.est_min = Some(5.0);
        let running = s.request(r, 0);
        grants(&s.tick(0));
        s.started(&running, "{}");
        let yields = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Yield { id, node, why } => Some((id.clone(), node.clone(), why.clone())), _ => None }).collect::<Vec<_>>();
        let twice = 10 * 60_000;
        beat_all(&mut s, twice);
        assert!(yields(&s.tick(twice)).is_empty(), "nobody waits on its box: its run is its own business");
        let waiting = s.request(req("other", Kind::Timed, Some("node-c")), twice);
        beat_all(&mut s, twice + 1);
        let a = s.tick(twice + 1);
        let y = yields(&a);
        assert_eq!(y.iter().map(|(id, node, _)| (id.as_str(), node.as_str())).collect::<Vec<_>>(), vec![(running.as_str(), "node-c")], "{a:?}");
        assert!(y[0].2.contains(&format!("10m00s into its 5m00s (its --est-min), while timed {waiting} of other waits on its box: its $DEVHUB_YIELD file now exists")), "{}", y[0].2);
        assert_eq!(s.owner_word(&running).as_deref(), Some(y[0].2.as_str()), "its job wait and its beat hear it");
        beat_all(&mut s, twice + 2);
        assert!(yields(&s.tick(twice + 2)).is_empty(), "said once");
    }

    #[test]
    fn a_waiting_update_holds_a_timed_lease_only_when_the_box_catches_up() {
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        s.update_waits.entry("node-c".into()).or_default().insert("deployment", "its deployment moves to abc".into());
        let timed = s.request(req("bench", Kind::Timed, Some("node-c")), 0);
        s.leases.iter_mut().find(|l| l.id == timed).unwrap().deploy_pin = Some("abc,def".into());
        let test = s.request(req("test", Kind::Test, Some("node-c")), 0);
        assert_eq!(grants(&s.tick(0)), vec![test.clone()], "other work goes on");
        assert!(state(&s, &timed).1.contains("catches up first (its deployment moves to abc)"), "{:?}", state(&s, &timed));
        let word = s.node_view().into_iter().find(|n| n.name == "node-c").unwrap().state_word();
        assert!(word.contains("update waits for its timed run: its deployment moves to abc"), "{word}");
        s.update_waits.clear();
        release_clean(&mut s, &test, 1);
        assert_eq!(grants(&s.tick(1)), vec![timed.clone()]);
        release_clean(&mut s, &timed, 2);
        // no catch-up: it starts on what the box has, whatever waits
        s.update_waits.entry("node-c".into()).or_default().insert("deployment", "its deployment moves to abc".into());
        let next = s.request(req("bench", Kind::Timed, Some("node-c")), 3);
        assert_eq!(grants(&s.tick(3)), vec![next]);
    }

    /// A box's own hub build is charged like a build lease while it runs: a build's default
    /// before it is measured, then its measured size; timed work waits for it.
    #[test]
    fn a_hub_build_is_charged_like_a_build_lease_of_its_measured_size() {
        let mut c = caps(16, 64.0, Role::Pool);
        c.hub_building = true;
        let mut s = sched(&[("n1", c.clone())]);
        let mut wide = req("wide", Kind::Test, Some("n1"));
        wide.cores = Some(10.0);
        let wide = s.request(wide, 0);
        assert!(grants(&s.tick(0)).is_empty());
        assert!(state(&s, &wide).1.contains("budget cores 16, in use 8, asked 10; its hub builds: 8 cores, 12.0 GB"), "{:?}", state(&s, &wide));
        for _ in 0..3 { s.record_resources(HUB_BUILD, "build", Some(2.0), Some(3.0)); }
        assert_eq!(grants(&s.tick(1)), vec![wide.clone()], "measured: 3 cores, 2.4 GB");
        assert_eq!(s.charged_on("n1").cores, 13.0);
        release_clean(&mut s, &wide, 2);
        let timed = s.request(req("bench", Kind::Timed, Some("n1")), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert!(state(&s, &timed).1.contains("its hub builds itself"), "{:?}", state(&s, &timed));
        c.hub_building = false;
        s.set_caps("n1", c, 3);
        assert_eq!(grants(&s.tick(3)), vec![timed]);
    }

    /// Landings every 2 min, a deployment apply of 3 min, timed runs of 12 min with the next one
    /// always asked: an update before each run kept a timed ask waiting behind the stream (37 min
    /// at the head of its queue, 11-04). Each starts as the last ends, on what the box has; the
    /// box catches up once when it fell 30 min behind, and when its queue empties.
    #[test]
    fn a_timed_queue_stays_on_what_the_box_has_until_it_falls_30_min_behind() {
        const MIN: u64 = 60_000;
        let at = |n: u64| format!("{n:040},{n:040}");
        let mut s = sched(&[("b1", caps(16, 64.0, Role::Bench))]);
        let (mut deploy, mut tips) = (at(0), 0u64);
        let mut applying: Option<(u64, String)> = None;
        (s.tips, s.landed) = (at(0), vec![(at(0), 0)]);
        let mut running: Option<(String, u64)> = None;
        let mut queued = Some(s.request(req("q", Kind::Timed, Some("b1")), 0));
        let mut starts: Vec<(u64, String)> = Vec::new();
        for step in 0..=(2 * 90u64) {
            let now = step * MIN / 2;
            if now > 0 && now % (2 * MIN) == 0 {
                tips += 1;
                s.tips = at(tips);
                s.landed.push((at(tips), now));
            }
            if let Some((l, _)) = running.clone().filter(|(_, end)| *end <= now) {
                release_clean(&mut s, &l, now);
                running = None;
            }
            if let Some((_, to)) = applying.clone().filter(|(end, _)| *end <= now) {
                deploy = to;
                applying = None;
            }
            // the box's updater (the auto-sync, the roll)
            let target = match deploy_to(s.update_pin("b1"), &deploy, |d, a| d >= a) {
                DeployTo::Tips => Some(at(tips)),
                DeployTo::Pin(p) => Some(p),
                DeployTo::Held { .. } => None,
            };
            s.update_waits.clear();
            match target.filter(|t| *t != deploy && applying.is_none()) {
                Some(t) if running.is_some() => { s.update_waits.entry("b1".into()).or_default().insert("deployment", format!("its deployment moves to {}", &t[38..40])); }
                Some(t) => applying = Some((now + 3 * MIN, t)),
                None => {}
            }
            let mut c = caps(16, 64.0, Role::Bench);
            (c.deploy, c.applying) = (deploy.clone(), applying.is_some());
            s.set_caps("b1", c, now);
            beat_all(&mut s, now);
            s.tick(now);
            // (a release's own pass may have granted it)
            if let Some(q) = queued.clone().filter(|q| s.get(q).is_some_and(|l| l.state == State::Starting)) {
                s.started(&q, "{}");
                starts.push((now / MIN, s.get(&q).unwrap().deployment.clone()));
                running = Some((q, now + 12 * MIN));
                queued = (now < 60 * MIN).then(|| s.request(req(&format!("q{}", starts.len()), Kind::Timed, Some("b1")), now));
            }
            if now == 20 * MIN {
                // the box says what it holds, and whose run that is
                let view = s.node_view().into_iter().find(|n| n.name == "b1").unwrap();
                let q = queued.clone().unwrap();
                assert_eq!(view.update_pin, Some((q.clone(), at(0))));
                assert!(view.state_word().contains(&format!("until {q} (timed, first in line) starts")), "{}", view.state_word());
            }
            if now == 33 * MIN {
                // a restarted master keeps a catch-up with the queue, and what a run ran on
                let mut back = Sched::new("mac", 1);
                for line in s.live_lines() { back.restore(&line, now); }
                let (q, run) = (queued.clone().unwrap(), running.clone().unwrap().0);
                assert_eq!(back.get(&q).and_then(|l| l.deploy_pin.clone()), Some(at(16)));
                assert_eq!(back.get(&run).map(|l| l.deployment.clone()), Some(at(0)));
            }
        }
        // on the deployment it had for 30 min, and the run after the catch-up waits for its apply
        // alone (the run before ends at 36, the box applies for 3 min); then on from there
        assert_eq!(starts[..5], [(0, at(0)), (12, at(0)), (24, at(0)), (39, at(16)), (51, at(16))], "{starts:?}");
        // the queue empty: the box goes on to the tips
        assert_eq!(s.update_pin("b1"), None);
        assert_eq!(deploy_to(s.update_pin("b1"), &deploy, |d, a| d >= a), DeployTo::Tips);
        assert!(deploy > at(16));
    }

    #[test]
    fn how_far_behind_a_box_is_counts_from_the_first_landing_it_lacks() {
        const MIN: u64 = 60_000;
        let at = |n: u64| format!("{n:040},{n:040}");
        let mut s = sched(&[("b1", caps(16, 64.0, Role::Bench))]);
        (s.tips, s.landed) = (at(3), vec![(at(1), 10 * MIN), (at(2), 20 * MIN), (at(3), 50 * MIN)]);
        assert_eq!(s.behind_ms(&at(3), 60 * MIN), 0, "at the tips");
        assert_eq!(s.behind_ms(&at(2), 60 * MIN), 10 * MIN);
        assert_eq!(s.behind_ms(&at(1), 60 * MIN), 40 * MIN);
        assert_eq!(s.behind_ms(&at(0), 60 * MIN), 50 * MIN, "older than the list: at least since its oldest");
        s.landed.clear();
        assert_eq!(s.behind_ms(&at(0), 60 * MIN), 0, "no landing known: nothing says it is behind");
    }

    #[test]
    fn ci_never_uses_a_user_box_even_when_named_and_enabled() {
        for role in [Role::UserWm, Role::Device] {
            let mut c = caps(16, 64.0, role);
            c.slots = vec![(Kind::Ci, 1)];
            assert_eq!(slots(&c, Kind::Ci), 0);
            let mut s = sched(&[("node-b", c)]);
            s.set_dev("node-b", true, 0).unwrap();
            let ci = s.request(req("ci", Kind::Ci, Some("node-b")), 0);
            assert!(grants(&s.tick(0)).is_empty());
            assert_eq!(state(&s, &ci).0, "ended denied");
        }
    }

    #[test]
    fn cleanup_holds_only_the_tree_being_scrubbed() {
        let mut c = caps(16, 64.0, Role::Pool);
        c.slots = vec![(Kind::Ci, 3), (Kind::Test, 1)];
        let mut s = sched(&[("node-b", c)]);
        let at = |lane: &str, kind: Kind| {
            let mut r = req(lane, kind, Some("node-b"));
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
        s.cleaned("node-b", &a, 0);
        assert_eq!(grants(&s.tick(3)), vec![b.clone()]);
        s.release(&test, "released", 4);
        let next = s.request(at("next", Kind::Test), 4);
        let same = s.request(at("test", Kind::Test), 4);
        assert_eq!(grants(&s.tick(4)), vec![next]);
        assert!(state(&s, &same).1.contains("scrubbing its work tree"));
        s.cleaned("node-b", &test, 0);
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
        let mut s = sched(&[("node-b", a), ("node-a", b)]);
        let mut r = req("ci", Kind::Ci, None);
        r.rev = Some(format!("work@{},main@{}", "a".repeat(40), "c".repeat(40)));
        let ci = s.request(r, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &ci).as_deref(), Some("node-a"));
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
        let only = s.request(req("ci", Kind::Ci, Some("a")), 1);
        s.tick(1);
        assert!(state(&s, &only).1.contains("takes no"), "{:?}", state(&s, &only));
    }

    #[test]
    fn ci_slots_do_not_require_a_display_but_gui_needs_do() {
        for role in [Role::Pool, Role::Bench] {
            let c = caps(16, 64.0, role);
            assert_eq!(slots(&c, Kind::Ci), 1);
            let mut s = sched(&[("node-b", c)]);
            let mut r = req("ci", Kind::Ci, Some("node-b"));
            r.needs = parse_needs("gui=1");
            let gui = s.request(r, 0);
            assert!(grants(&s.tick(0)).is_empty());
            assert_eq!(state(&s, &gui).0, "ended denied");
            let plain = s.request(req("ci", Kind::Ci, Some("node-b")), 1);
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
        let mut s = sched(&[("node-b", caps(64, 256.0, Role::Pool))]);
        for _ in 0..3 {
            let mut r = overlay(CI_LANE, Some("node-b"), 4.0);
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
    fn a_new_masters_first_seconds_warn_of_no_wait() {
        let warned = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Event(r) if r.sev == Sev::Warn && r.line.contains(" waiting ") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = req("busy", Kind::Test, Some("a"));
        r.whole = true;
        let busy = s.request(r, 0);
        assert_eq!(grants(&s.tick(0)), vec![busy.clone()]);
        s.started(&busy, "{}");
        // the master restarts with an ask that has waited over the warning's time: asked again,
        // its age counts from where it first asked
        let settled = WAIT_WARN_MS + 60_000;
        s.nodes_settle_ms = settled;
        let mut r = req("waits", Kind::Test, Some("a"));
        r.since_ms = Some(5);
        let waits = s.request(r, WAIT_WARN_MS);
        assert_eq!(s.get(&waits).unwrap().asked_ms, 5);
        assert_eq!(s.get(&waits).unwrap().queue_sample_ms, WAIT_WARN_MS, "what it waited on before this ask is no wait of this lease");
        s.nodes.get_mut("a").unwrap().caps = None;
        beat_all(&mut s, WAIT_WARN_MS);
        assert_eq!(warned(&s.tick(WAIT_WARN_MS)), Vec::<String>::new());
        assert_eq!(state(&s, &waits).1, "node a not heard yet");
        // the box is heard: once the minute is over the warning says what the ask waits for
        s.set_caps("a", caps(16, 64.0, Role::Pool), WAIT_WARN_MS + 3_000);
        beat_all(&mut s, WAIT_WARN_MS + 3_000);
        assert_eq!(warned(&s.tick(WAIT_WARN_MS + 3_000)), Vec::<String>::new());
        beat_all(&mut s, settled);
        let said = warned(&s.tick(settled));
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains(&format!("held by {busy}")), "{said:?}");
    }

    /// A lease of a class with no measure yet is held to its kind's length, at least 5 min (a test
    /// lease's 30 s says stalled at 10 min); a lease whose every process did nothing for 10 min
    /// says so whatever its class (here a measured 1 h); both capture its state. A live session
    /// may sit idle.
    #[test]
    fn a_new_class_and_an_idle_lease_are_watched_too() {
        let warned = |a: &[Action], id: &str, what: &str| a.iter().filter(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains(what) && r.line.contains(id))).count();
        let captured = |a: &[Action], lease: &str, what: &str| a.iter().any(|a| matches!(a, Action::Capture { id, why, .. } if id == lease && why.contains(what)));
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let fresh = s.request(req("fresh", Kind::Test, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![fresh.clone()]);
        s.started(&fresh, "{}");
        beat_all(&mut s, 600_000);
        assert_eq!(warned(&s.tick(600_000), &fresh, "stalled?"), 0, "10 min of a new class is no stall yet");
        beat_all(&mut s, 601_000);
        let a = s.tick(601_000);
        assert_eq!(warned(&a, &fresh, "stalled?"), 1);
        assert!(captured(&a, &fresh, "against its kind's 5m00s (its class has no measure yet)"), "{a:?}");

        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_node_cost("test", "test", "a", 3_600_000); }
        let hung = s.request(req("hung", Kind::Test, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![hung.clone()]);
        let mut live = req("live", Kind::Test, Some("b"));
        live.live = true;
        let live = s.request(live, 0);
        assert_eq!(grants(&s.tick(0)), vec![live.clone()], "{:?}", state(&s, &live));
        for id in [&hung, &live] {
            s.started(id, "{}");
            s.set_info(id, r#"{"idle_s":599}"#, 0);
        }
        beat_all(&mut s, 600_000);
        assert_eq!(warned(&s.tick(600_000), &hung, "idle"), 0, "just under 10 min");
        for id in [&hung, &live] { s.set_info(id, r#"{"idle_s":600}"#, 0); }
        beat_all(&mut s, 601_000);
        let a = s.tick(601_000);
        assert_eq!(warned(&a, &hung, "every process idle (under 5% of a core, no I/O) for 10m00s"), 1, "{a:?}");
        assert!(captured(&a, &hung, "every process idle"));
        assert_eq!(warned(&a, &live, "idle"), 0, "a live session waits for its caller");
        beat_all(&mut s, 602_000);
        assert_eq!(warned(&s.tick(602_000), &hung, "idle"), 0, "said once");
    }

    /// A CI leg nobody drives ends as a failure when its processes stay idle; its stall's capture
    /// has its time first. A lane's own lease stays for whoever reads the stall, and a leg that
    /// works again is not ended.
    #[test]
    fn whoever_queues_behind_a_hung_lease_hears_it() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut held = req("lane", Kind::Test, Some("a"));
        held.rev = Some(format!("work@{}", "a".repeat(40)));
        let next = held.clone();
        let hung = s.request(held, 0);
        s.tick(0);
        s.started(&hung, "{}");
        let queued = s.request(next, 1);
        s.tick(1);
        assert!(state(&s, &queued).1.contains(&format!("held by {hung}")), "{:?}", state(&s, &queued));
        assert_eq!(s.behind_hung("lane"), None, "it works");
        s.set_info(&hung, r#"{"idle_s":600}"#, 0);
        beat_all(&mut s, 600_000);
        s.tick(600_000);
        assert!(state(&s, &queued).1.contains(&format!("`devhub cancel {hung}` frees it")), "{:?}", state(&s, &queued));
        let (id, word) = s.behind_hung("lane").unwrap();
        assert_eq!(id, hung);
        assert!(word.contains(&format!("{queued} waits behind {hung}")) && word.contains("every process idle"), "{word}");
        assert_eq!(s.behind_hung("another"), None);
    }

    /// A lane's build behind a whole ask of another lane, itself behind the lane's own screen lease:
    /// the wait says so and names the lease, for this lane only, and goes with the lease.
    #[test]
    fn a_wait_that_comes_down_to_the_lanes_own_session_says_so() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let own = s.request(req("lane", Kind::Screen, Some("a")), 0);
        s.tick(0);
        s.started(&own, "{}");
        let mut ask = req("ci-like", Kind::Test, Some("a"));
        ask.whole = true;
        let whole = s.request(ask, 1);
        s.tick(1);
        assert!(state(&s, &whole).1.contains(&format!("held by {own}")), "{:?}", state(&s, &whole));
        assert!(!state(&s, &whole).1.contains(OWN_SESSION), "another lane's session is no wait of its own: {:?}", state(&s, &whole));
        let build = s.request(req("lane", Kind::Build, Some("a")), 2);
        let other = s.request(req("other", Kind::Build, Some("a")), 2);
        s.tick(2);
        s.tick(3);
        let said = state(&s, &build).1;
        assert!(said.contains(&format!("held for whole {whole}")) && said.contains(&format!("{OWN_SESSION} {own} (screen on a): `devhub release {own}` frees it")), "{said}");
        assert_eq!(said.matches(OWN_SESSION).count(), 1, "{said}");
        assert!(!state(&s, &other).1.contains(OWN_SESSION), "{:?}", state(&s, &other));
        let (id, word) = s.behind_own("lane").unwrap();
        assert_eq!(id, own);
        assert!(word.starts_with(&format!("{build} waits: {OWN_SESSION} {own}")), "{word}");
        assert_eq!(s.behind_own("other"), None);
        release_clean(&mut s, &own, 4);
        s.tick(4);
        assert_eq!(s.behind_own("lane"), None, "it went with the session");
    }

    /// Work of the lane's own that ends by itself is no such wait.
    #[test]
    fn a_wait_on_the_lanes_own_running_work_says_nothing_more() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut first = req("lane", Kind::Test, Some("a"));
        first.rev = Some(format!("work@{}", "a".repeat(40)));
        let next = first.clone();
        let running = s.request(first, 0);
        s.tick(0);
        s.started(&running, "{}");
        let queued = s.request(next, 1);
        s.tick(1);
        assert!(state(&s, &queued).1.contains(&format!("held by {running}")), "{:?}", state(&s, &queued));
        assert!(!state(&s, &queued).1.contains(OWN_SESSION), "{:?}", state(&s, &queued));
        assert_eq!(s.behind_own("lane"), None);
    }

    #[test]
    fn a_boxs_own_prebuild_waits_without_a_warning() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut busy = req("lane", Kind::Test, Some("a"));
        busy.whole = true;
        let held = s.request(busy, 0);
        s.tick(0);
        s.started(&held, "{}");
        let prebuild = req(&format!("{}a", super::super::PREBUILD_LANE_PREFIX), Kind::Build, Some("a"));
        let prebuild = s.request(prebuild, 1);
        beat_all(&mut s, WAIT_WARN_MS + 1);
        let acts = s.tick(WAIT_WARN_MS + 1);
        assert_eq!(state(&s, &prebuild).0, "queued");
        let said = |sev: Sev| acts.iter().filter(|a| matches!(a, Action::Event(r) if r.sev == sev && r.line.contains(&prebuild) && r.line.contains(" waiting "))).count();
        assert_eq!((said(Sev::Info), said(Sev::Warn)), (1, 0));
    }

    #[test]
    fn a_hung_ci_leg_ends_after_its_capture_and_a_lanes_lease_stays() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        let (leg, woke, lane) = (s.request(req(CI_LANE, Kind::Test, Some("a")), 0), s.request(req(CI_LANE, Kind::Test, Some("b")), 0), s.request(req("lane", Kind::Test, Some("c")), 0));
        s.tick(0);
        for id in [&leg, &woke, &lane] {
            s.started(id, "{}");
            s.set_info(id, r#"{"idle_s":600}"#, 0);
        }
        beat_all(&mut s, 600_000);
        let a = s.tick(600_000);
        assert!(a.iter().any(|x| matches!(x, Action::Capture { id, why, .. } if *id == leg && why.contains("every process idle"))), "{a:?}");
        beat_all(&mut s, 659_000);
        s.tick(659_000);
        assert_eq!(state(&s, &leg).0, "granted", "the capture has its minute");
        assert!(s.owner_word(&lane).is_some_and(|w| w.contains("every process idle") && w.contains("10m00s")), "its owner hears it");
        s.set_info(&woke, r#"{"idle_s":0}"#, 0);
        assert_eq!(s.owner_word(&woke), None, "it works again");
        beat_all(&mut s, 660_000);
        s.tick(660_000);
        assert_eq!(state(&s, &leg), ("ended hung".to_string(), "every process idle (under 5% of a core, no I/O) for 10m00s".to_string()));
        assert_eq!(state(&s, &woke).0, "granted", "it works again");
        assert_eq!(state(&s, &lane).0, "granted", "a lane's lease has someone to read the stall");
    }

    #[test]
    fn a_held_lease_is_measured_by_the_class_its_run_names() {
        // class-less leases of the kind ran 2 min on the box; this one's runs take 15
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 {
            s.record_node_cost("test", "test", "a", 120_000);
            s.record_node_cost("test bash legs.sh", "test", "a", 900_000);
        }
        let leg = s.request(req("legs", Kind::Test, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![leg.clone()]);
        s.started(&leg, "{}");
        s.set_info(&leg, r#"{"running":"bash legs.sh","class":"test bash legs.sh"}"#, 0);
        assert_eq!(s.get(&leg).unwrap().req.class, "test bash legs.sh");
        let stalled = |a: &[Action]| a.iter().filter(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains("stalled?"))).count();
        beat_all(&mut s, 600_000);
        assert_eq!(stalled(&s.tick(600_000)), 0, "10 min of a 15 min run is no stall");
        beat_all(&mut s, 1_801_000);
        assert_eq!(stalled(&s.tick(1_801_000)), 1, "past twice its own 15 min it is");
        // a class the request named stays
        let mut named = req("named", Kind::Test, Some("a"));
        named.class = "test cargo test".into();
        let named = s.request(named, 1_802_000);
        s.set_info(&named, r#"{"class":"test git rev-parse HEAD"}"#, 0);
        assert_eq!(s.get(&named).unwrap().req.class, "test cargo test");
    }

    /// A class's whole runs and its filtered ones in one class, this box's walls
    /// all short ones. A run that declares its length is held to that; one that does not, to the
    /// figure its row shows. So are the ETAs of what waits behind it.
    #[test]
    fn a_stall_check_and_an_eta_read_the_declared_run_else_the_row() {
        const CLASS: &str = "timed makepad test makepad-foo";
        let stalled = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Event(r) if r.sev == Sev::Warn && r.line.contains("stalled?") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        let start = |est_min: Option<f64>| {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
            for _ in 0..3 { s.record_node_cost(CLASS, "timed", "a", 83_000); }
            s.record_node_cost(CLASS, "timed", "b", 1_015_000);
            let mut r = req("tests", Kind::Timed, Some("a"));
            r.class = CLASS.into();
            r.est_min = est_min;
            let run = s.request(r, 0);
            assert_eq!(grants(&s.tick(0)), vec![run.clone()]);
            s.started(&run, "{}");
            let behind = s.request(req("next", Kind::Timed, Some("a")), 0);
            (s, run, behind)
        };
        let wait = |s: &Sched, id: &str| s.eta(s.get(id).unwrap()).map(|(_, ms)| ms);

        let (mut s, run, behind) = start(Some(25.0));
        assert!(matches!(s.get(&run).unwrap().cost_source, CostSource::Measured(710_500)), "its row: 7/10 of the long run");
        beat_all(&mut s, 384_000);
        assert_eq!(stalled(&s.tick(384_000)), Vec::<String>::new(), "6m24s of a declared 25 min, against this box's 1m23s");
        assert_eq!(wait(&s, &behind), Some(1_500_000 - 384_000), "what waits behind it counts what is left of the 25 min");
        beat_all(&mut s, 3_000_000);
        assert!(stalled(&s.tick(3_000_000)).is_empty(), "at twice what it declared: not past it yet");
        beat_all(&mut s, 3_001_000);
        let said = stalled(&s.tick(3_001_000));
        assert!(said.len() == 1 && said[0].contains("against its declared 25m"), "{said:?}");

        let (mut s, run, behind) = start(None);
        beat_all(&mut s, 384_000);
        assert!(stalled(&s.tick(384_000)).is_empty(), "under the figure its row shows");
        assert_eq!(wait(&s, &behind), Some(710_500 - 384_000), "the same figure");
        beat_all(&mut s, 1_421_001);
        let said = stalled(&s.tick(1_421_001));
        assert!(said.len() == 1 && said[0].contains(&run) && said[0].contains(&format!("against its measured {}", dur(710_500))), "{said:?}");
    }

    #[test]
    fn a_lease_past_its_measure_whose_output_grows_is_no_stall() {
        // a sweep's class measured 4 min on smaller sweeps; this one is 426 crates and writes on
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 {
            s.record_node_cost("test bash sweep.sh", "test", "a", 240_000);
        }
        let mut r = req("gaps", Kind::Test, Some("a"));
        r.class = "test bash sweep.sh".into();
        let sweep = s.request(r, 0);
        assert_eq!(grants(&s.tick(0)), vec![sweep.clone()]);
        s.started(&sweep, "{}");
        let stalled = |a: &[Action]| a.iter().filter(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains("stalled?"))).count();
        let mut out = 0;
        for now in (60_000..=1_200_000).step_by(60_000) {
            out += 4096;
            s.set_info(&sweep, &format!(r#"{{"running":"bash sweep.sh","out_bytes":{out}}}"#), now);
            beat_all(&mut s, now);
            assert_eq!(stalled(&s.tick(now)), 0, "20 min against a 4 min measure, writing on, at {now}");
        }
        // its output stops: past the window it is a stall
        for now in (1_260_000..1_500_000).step_by(60_000) {
            s.set_info(&sweep, &format!(r#"{{"running":"bash sweep.sh","out_bytes":{out}}}"#), now);
            beat_all(&mut s, now);
            assert_eq!(stalled(&s.tick(now)), 0, "its output grew {} s ago", (now - 1_200_000) / 1000);
        }
        s.set_info(&sweep, &format!(r#"{{"running":"bash sweep.sh","out_bytes":{out}}}"#), 1_500_000);
        beat_all(&mut s, 1_500_000);
        assert_eq!(stalled(&s.tick(1_500_000)), 1, "5 min with no output, past twice its measure");
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
        s.request(req("x", Kind::Build, Some("a")), 0);
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
        s.set_info(&lanes[0], r#"{"running":"bash land_gate.sh full"}"#, 0);
        s.tick(GATE_IDLE_MS + 5);
        assert_eq!(state(&s, &lanes[0]).0, "starting", "a gate that runs something stays");
        assert_eq!(state(&s, &lanes[1]).0, "ended idle");
        let freed = s.get(&lanes[1]).unwrap().node.clone().unwrap();
        s.cleaned(&freed, &lanes[1], 0);
        beat_all(&mut s, GATE_IDLE_MS + 6);
        s.tick(GATE_IDLE_MS + 6);
        assert_eq!(state(&s, &land).0, "starting", "the landing takes the freed box first");
        assert_eq!(state(&s, &late).0, "queued");
        // a run longer than the idle limit: idle from its end, not from the grant
        s.set_info(&lanes[0], r#"{"running":""}"#, 0);
        beat_all(&mut s, GATE_IDLE_MS + 7);
        s.tick(GATE_IDLE_MS + 7);
        assert_eq!(state(&s, &lanes[0]).0, "starting", "its run just ended");
        beat_all(&mut s, 2 * GATE_IDLE_MS + 8);
        s.tick(2 * GATE_IDLE_MS + 8);
        assert_eq!(state(&s, &lanes[0]).0, "ended idle", "idle for the limit since its run");
    }
    /// `--needs gui`: a test lease that opens windows goes only to nodes that present (screen
    /// slots above 0); a box at slots.screen=0 takes none, plain test leases still go there.
    #[test]
    fn gui_tests_avoid_nodes_without_screen_slots() {
        let mut dark = caps(16, 64.0, Role::Pool);
        dark.slots = vec![(Kind::Build, 2), (Kind::Test, 2), (Kind::Screen, 0), (Kind::Long, 2), (Kind::Gate, 1)];
        let mut s = sched(&[("node-b", dark.clone())]);
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

    /// A user's WM takes only what names it: no unpinned lease, no lease named to another box.
    #[test]
    fn a_user_wm_takes_only_leases_that_name_it() {
        let mut s = sched(&[("wm", caps(64, 256.0, Role::UserWm)), ("p", caps(64, 256.0, Role::Pool))]);
        let held = s.request(req("h", Kind::Test, Some("p")), 0);
        s.tick(0);
        assert_eq!(node_of(&s, &held).as_deref(), Some("p"));
        // the pool is full and the WM idle: an unpinned lease and one named to the pool wait
        let mut any = req("a", Kind::Test, None);
        any.needs = parse_needs("os=linux");
        let any = s.request(any, 1);
        let pinned = s.request(req("b", Kind::Test, Some("p")), 1);
        beat_all(&mut s, 600_000);
        s.tick(600_000);
        assert_eq!(node_of(&s, &any), None);
        assert_eq!(node_of(&s, &pinned), None);
        // one that names it goes there
        let named = s.request(req("c", Kind::Test, Some("wm")), 600_001);
        s.tick(600_001);
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
        let user = s.request(req("y", Kind::Build, Some("wm")), 0);
        // another lane named to the switched-off WM waits for it too (a has room only for the gate)
        let soft = s.request(req("z", Kind::Build, Some("wm")), 0);
        let insp = {
            let mut r = req("y", Kind::Inspect, Some("wm"));
            r.jit = Some("foo".into());
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
        // while it lives, beats that come late (a throttled holder) end nothing
        assert!(s.tick(90_000).iter().all(|a| !matches!(a, Action::End { .. })), "a live holder's lease does not expire");
        assert_eq!(state(&s, &q).0, "starting");
        assert!(s.release(&q, "holder-dead", 90_001));
        assert_eq!(s.get(&q).map(|l| l.state.clone()), Some(State::Ended("holder-dead".into())));
    }

    /// A node hub past a quarter of its box's RAM is a warning, once per crossing, naming the
    /// command that says where its memory went; the nodes line shows every hub's size.
    #[test]
    fn a_heavy_node_hub_is_said_once() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let heavy = |s: &mut Sched, rss_mb: u64| {
            let mut c = caps(16, 64.0, Role::Pool);
            c.hub_rss_mb = rss_mb;
            c.hub_heap_mb = rss_mb / 4;
            s.set_caps("a", c, 1);
            s.tick(1).into_iter().filter(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains("its hub holds"))).count()
        };
        assert_eq!(heavy(&mut s, 3 << 10), 0, "3 GB of 64 is not heavy");
        assert_eq!(heavy(&mut s, 17 << 10), 1, "17 GB of 64 is");
        assert_eq!(heavy(&mut s, 18 << 10), 0, "said once");
        assert_eq!(heavy(&mut s, 1 << 10), 0);
        assert_eq!(heavy(&mut s, 20 << 10), 1, "again after it came back down");
        let line = s.node_view().into_iter().find(|n| n.name == "a").unwrap().caps_line();
        assert!(line.contains("hub old 20.0 GB (heap 5.0)"), "{line}");
        let c = Caps::parse(&s.nodes["a"].caps.as_ref().unwrap().to_json()).unwrap();
        assert_eq!((c.hub_rss_mb, c.hub_heap_mb), (20 << 10, 5 << 10), "round-trips");
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
        let mut r = req("gapx", Kind::Build, Some("node-d"));
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
        let mut s = sched(&[("node-d", caps(16, 60.0, Role::Pool))]);
        let id = s.request(req("lane-a", Kind::Build, Some("node-d")), 0);
        assert_eq!(grants(&s.tick(1)), vec![id.clone()]);
        s.started(&id, "{\"work\":\"/w\"}");
        let live = s.live_lines();
        assert_eq!(live.len(), 1);
        let mut t = sched(&[("node-d", caps(16, 60.0, Role::Pool))]);
        assert_eq!(t.restore(&live[0], 100), Some(id.clone()));
        assert_eq!(state(&t, &id).0, "granted");
        assert_eq!(t.get(&id).unwrap().start, "{\"work\":\"/w\"}");
        let next = t.request(req("lane-a", Kind::Test, Some("node-d")), 100);
        assert_ne!(next, id, "ids continue after the restored ones");
        assert_eq!(t.live_on("node-d"), vec![id]);
    }

    fn node_of(s: &Sched, id: &str) -> Option<String> {
        s.get(id).unwrap().node.clone()
    }

    #[test]
    fn only_on_a_hot_box_that_refuses_the_class_is_denied_at_once() {
        let mut s = sched(&[("h", caps(16, 64.0, Role::Hot)), ("p", caps(16, 64.0, Role::Pool))]);
        let mut r = req("x", Kind::Screen, Some("h"));
        r.est_min = Some(40.0);
        let x = s.request(r, 0);
        s.tick(0);
        assert_eq!(state(&s, &x).0, "ended denied", "{:?}", state(&s, &x));
        assert!(state(&s, &x).1.contains("hot box"), "{:?}", state(&s, &x));
        // a hot Mac takes the macOS work only it can do (devhub's JIT prebuild, class medium)
        let mut mac = caps(16, 64.0, Role::Hot);
        mac.os = "macos".into();
        let mut s = sched(&[("mac", mac)]);
        let mut r = req("mac-prebuild", Kind::Build, Some("mac"));
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
        // measured as its own class (`leg:mac-prebuild`): its peak, not the whole budget, so it
        // fits beside a live session there
        let mut mac = caps(10, 16.0, Role::Hot);
        mac.os = "macos".into();
        let mut s = sched(&[("mac", mac)]);
        let mut live = overlay("live", Some("mac"), 2.0);
        live.needs = r.needs.clone();
        live.ram_gb = Some(2.0);
        let live = s.request(live, 0);
        assert_eq!(grants(&s.tick(0)), vec![live]);
        let mut own = r.clone();
        (own.src, own.rev) = (Src::Overlay, Some(format!("work@{}", "a".repeat(40))));
        own.class = "leg:mac-prebuild".into();
        for _ in 0..3 { s.record_resources("build", "build", Some(10.0), None); }
        for _ in 0..3 { s.record_resources(&own.class, "build", Some(2.5), None); }
        let x = s.request(own, 1);
        assert_eq!(grants(&s.tick(1)), vec![x.clone()], "{:?}", state(&s, &x));
    }

    #[test]
    fn a_macos_gpu_lease_is_a_timed_one() {
        // the Macs list no GPU and offer no gpu slot: their Metal work ran into "takes no gpu leases"
        let r = Req::parse(r#"{"lane":"g","kind":"gpu","needs":{"arch":"arm64","os":"macos"}}"#).unwrap();
        assert_eq!((r.kind, r.cost), (Kind::Timed, CostClass::default_for(Kind::Timed)));
        assert_eq!(Req::parse(r#"{"lane":"g","kind":"gpu"}"#).unwrap().kind, Kind::Gpu, "a Linux gpu lease stays one");
        let mut mac = caps(10, 16.0, Role::Bench);
        (mac.arch, mac.os, mac.gpus) = ("aarch64".into(), "macos".into(), Vec::new());
        let mut s = sched(&[("linux", caps(16, 64.0, Role::Pool)), ("mac", mac)]);
        let x = s.request(r, 0);
        assert_eq!(grants(&s.tick(0)), vec![x.clone()], "{:?}", state(&s, &x));
        assert_eq!(node_of(&s, &x).as_deref(), Some("mac"));
    }

    /// A session is charged the RAM its node measured, not its kind's default, and a new one asks
    /// its class's measured peak even from one run.
    #[test]
    fn a_session_is_charged_the_ram_it_uses() {
        let mut s = sched(&[("mac", caps(10, 16.0, Role::Hot))]);
        let ask = |lane: &str, kind: Kind, ram: Option<f64>| {
            let mut r = overlay(lane, None, 1.0);
            (r.kind, r.ram_gb, r.cost, r.cost_asked, r.class) = (kind, ram, CostClass::Small, true, lane.to_string());
            r
        };
        let show = s.request(ask("show", Kind::Screen, None), 0);
        s.tick(0);
        assert_eq!(node_of(&s, &show).as_deref(), Some("mac"));
        let default = s.charged(s.get(&show).unwrap()).ram_gb;
        assert_eq!(default, 4.0, "the kind's default");
        let test = s.request(ask("t", Kind::Test, Some(5.0)), 1);
        s.tick(1);
        assert_eq!(node_of(&s, &test), None, "{:?}", state(&s, &test));
        s.set_info(&show, r#"{"running":"makepad","held_mb":1024}"#, 0);
        assert_eq!(s.charged(s.get(&show).unwrap()).ram_gb, 1.0);
        beat_all(&mut s, 2);
        s.tick(2);
        assert_eq!(node_of(&s, &test).as_deref(), Some("mac"), "{:?}", state(&s, &test));
        // its class's one measured run is the next session's ask
        s.record_resources("show", "screen", Some(1.5), None);
        let next = s.request(ask("show", Kind::Screen, None), 3);
        assert!((s.cost_on("mac", s.get(&next).unwrap()).ram_gb - 1.8).abs() < 1e-9);
    }

    /// A running lease is charged the RAM its node measured it holding (the
    /// most so far), not its class's peak, which it is charged only before that first look. A
    /// live repro whose class measured 11.5 GB, charged that on a Mac's 9.4 GB budget while it
    /// held 4.5 GB, kept a 3.6 GB prebuild waiting; charged its 4.5 GB the prebuild starts and an
    /// 8 GB ask still waits. A lease that asked its RAM stays charged its ask, and one being
    /// cleaned up what it held.
    #[test]
    fn a_running_lease_is_charged_the_ram_it_holds() {
        let mut mac = caps(10, 16.0, Role::Hot);
        (mac.os, mac.arch, mac.ram_resident_gb) = ("macos".into(), "aarch64".into(), 5.0);
        let mut s = sched(&[("mac", mac)]);
        let on_mac = |lane: &str, kind: Kind, ram: Option<f64>| {
            let mut r = overlay(lane, None, 1.0);
            r.needs = vec![("os".into(), "macos".into())];
            (r.kind, r.ram_gb, r.class) = (kind, ram, lane.to_string());
            r
        };
        for _ in 0..3 { s.record_resources("repro", "test", Some(9.6), None); }
        let repro = s.request(on_mac("repro", Kind::Test, None), 0);
        assert_eq!(grants(&s.tick(0)), vec![repro.clone()]);
        assert!((s.charged(s.get(&repro).unwrap()).ram_gb - 8.44).abs() < 1e-9, "before a look: its class's peak, cut to the budget");
        let prebuild = s.request(on_mac("prebuild", Kind::Build, Some(3.6)), 1);
        assert!(grants(&s.tick(1)).is_empty());
        assert!(state(&s, &prebuild).1.contains("budget ram 8.4 GB, in use 8.4 GB, asked 3.6 GB"), "{:?}", state(&s, &prebuild));
        s.set_info(&repro, r#"{"running":"makepad","held_mb":4608}"#, 0);
        assert_eq!(s.charged(s.get(&repro).unwrap()).ram_gb, 4.5);
        beat_all(&mut s, 2);
        assert_eq!(grants(&s.tick(2)), vec![prebuild.clone()], "{:?}", state(&s, &prebuild));
        let screen = s.request(on_mac("jitcost", Kind::Test, Some(8.0)), 3);
        beat_all(&mut s, 3);
        assert!(grants(&s.tick(3)).is_empty());
        assert!(state(&s, &screen).1.contains("budget ram 8.4 GB, in use 8.1 GB, asked 8.0 GB"), "{:?}", state(&s, &screen));
        // an asked RAM stays its charge whatever it holds
        s.set_info(&prebuild, r#"{"running":"cargo","held_mb":512}"#, 0);
        assert_eq!(s.charged(s.get(&prebuild).unwrap()).ram_gb, 3.6);
        // ended, it is charged what it held until its cleanup is confirmed
        s.release(&repro, "done", 4);
        assert_eq!(s.charged_on("mac").ram_gb, 3.6 + 4.5);
        invariants(&s);
    }

    /// Work that names no box takes a pool box that fits, even a busy one, over an idle bench box
    /// the lane is warm on; when no pool box fits it waits for one, the bench box idle meanwhile
    /// (its quiet window is timed work's). A bench box takes it by name, or while no pool box is
    /// up, and never while a timed request waits for it, which then is not held for that work.
    #[test]
    fn work_keeps_off_the_bench_boxes() {
        let mut s = sched(&[("bench", caps(16, 64.0, Role::Bench)), ("pool", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, kind: Kind, cores: f64, interactive: bool| {
            let mut r = overlay(lane, None, cores);
            r.kind = kind;
            if interactive { r.priority = Priority::Interactive; }
            r
        };
        let mut busy = ask("busy", Kind::Build, 13.0, false);
        busy.node = Some("pool".into());
        let busy = s.request(busy, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &busy).as_deref(), Some("pool"));
        s.warm.insert(("ui".into(), "bench".into()));
        let ui = s.request(ask("ui", Kind::Test, 2.0, true), 1);
        let screen = s.request(ask("screen", Kind::Screen, 1.0, false), 1);
        s.tick(1);
        assert_eq!(node_of(&s, &ui).as_deref(), Some("pool"), "{:?}", state(&s, &ui));
        assert_eq!(node_of(&s, &screen).as_deref(), Some("pool"), "{:?}", state(&s, &screen));
        // no pool box fits: interactive or background, it waits for the pool box
        let wide = s.request(ask("wide", Kind::Test, 8.0, true), 2);
        let build = s.request(ask("build", Kind::Build, 8.0, false), 2);
        s.tick(2);
        for id in [&wide, &build] {
            assert_eq!(node_of(&s, id), None, "{:?}", state(&s, id));
            assert!(s.why_queued(id, 2).unwrap().iter().any(|w| w.contains("a bench box: work runs on the pool boxes")), "{:?}", s.why_queued(id, 2));
        }
        // by name it runs there
        let mut named = ask("named", Kind::Test, 8.0, true);
        named.node = Some("bench".into());
        let named = s.request(named, 3);
        s.tick(3);
        assert_eq!(node_of(&s, &named).as_deref(), Some("bench"), "{:?}", state(&s, &named));
        release_clean(&mut s, &named, 4);
        // a timed request waits for the bench box: it is not held for the work waiting on the pool
        let timed = s.request(req("timed", Kind::Timed, None), 5);
        beat_all(&mut s, 5);
        s.tick(5);
        assert!(!s.interactive_boxes.get("bench").is_some_and(|ids| ids.contains(&wide)), "{:?}", s.interactive_boxes);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("bench"), "{:?}", state(&s, &timed));
        release_clean(&mut s, &timed, 6);
        // the pool box down: the bench box takes what waits
        s.nodes.get_mut("pool").unwrap().up = false;
        beat_all(&mut s, 7);
        s.tick(7);
        assert_eq!(node_of(&s, &wide).as_deref(), Some("bench"), "{:?}", state(&s, &wide));
        invariants(&s);
    }

    /// A spare Mac takes macOS timed work only while the bench Mac is taken by timed work; while
    /// the bench Mac is busy with other work a timed request drains it, and one that names the
    /// spare takes the spare.
    #[test]
    fn a_spare_mac_takes_timed_work_only_while_the_bench_mac_is_taken() {
        let mac = |role| {
            let mut c = caps(10, 16.0, role);
            (c.arch, c.os, c.gpus) = ("aarch64".into(), "macos".into(), Vec::new());
            c
        };
        let mut s = sched(&[("linux", caps(16, 64.0, Role::Bench)), ("m1", mac(Role::Bench)), ("m2", mac(Role::Hot))]);
        let timed = |lane: &str, node: Option<&str>| {
            let mut r = req(lane, Kind::Timed, node);
            r.needs = vec![("arch".into(), "arm64".into()), ("os".into(), "macos".into())];
            r
        };
        let a = s.request(timed("a", None), 0);
        s.tick(0);
        assert_eq!(node_of(&s, &a).as_deref(), Some("m1"), "the bench Mac first: {:?}", state(&s, &a));
        let b = s.request(timed("b", None), 1);
        s.tick(1);
        assert_eq!(node_of(&s, &b).as_deref(), Some("m2"), "the bench Mac runs timed work: {:?}", state(&s, &b));
        release_clean(&mut s, &a, 2);
        release_clean(&mut s, &b, 2);
        let mut build = req("build", Kind::Build, Some("m1"));
        build.needs = vec![("os".into(), "macos".into())];
        build.est_min = Some(5.0);
        let build = s.request(build, 3);
        s.tick(3);
        assert_eq!(node_of(&s, &build).as_deref(), Some("m1"));
        let c = s.request(timed("c", None), 4);
        s.tick(4);
        assert_eq!(node_of(&s, &c), None, "{:?}", state(&s, &c));
        assert_eq!(s.holds.get("m1").map(|h| h.id.clone()), Some(c.clone()), "it drains the bench Mac");
        assert!(s.why_queued(&c, 4).unwrap().iter().any(|w| w.contains("a spare Mac for timed work: it waits while m1 can take it")));
        let d = s.request(timed("d", Some("m2")), 5);
        s.tick(5);
        assert_eq!(node_of(&s, &d).as_deref(), Some("m2"), "named: {:?}", state(&s, &d));
        invariants(&s);
    }

    /// `--node` takes one node name: a value with needs pasted in is denied at once, and says so;
    /// a name the master does not know is quoted; a need a box fails says what the box has.
    #[test]
    fn a_node_name_with_needs_in_it_is_denied_at_once_and_a_failed_need_says_what_the_box_has() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        s.nodes_settle_ms = 60_000;
        let pasted = s.request(req("x", Kind::Test, Some("a os=macos")), 0);
        s.tick(0);
        assert_eq!(state(&s, &pasted), ("ended denied".to_string(), "--node `a os=macos` is no node name: --node takes one name (devhub nodes); needs such as os=macos go in --needs".to_string()));
        let unknown = s.request(req("x", Kind::Test, Some("zz")), 61_000);
        s.tick(61_000);
        assert_eq!(state(&s, &unknown).1, "no node `zz` (devhub nodes lists them)");
        let mut mac = req("x", Kind::Test, Some("a"));
        mac.needs = vec![("os".into(), "macos".into())];
        let mac = s.request(mac, 62_000);
        s.tick(62_000);
        assert!(state(&s, &mac).1.contains("needs os=macos, it has os=linux"), "{:?}", state(&s, &mac));
    }

    /// `gpu=1` asks for a GPU: read as part of a name, it denied most and passed any with a 1 in it.
    #[test]
    fn a_gpu_need_is_a_count_a_yes_or_no_or_part_of_a_name() {
        let mut c = caps(16, 64.0, Role::Pool);
        c.gpus = vec![("GPU-B 3000".into(), 8.0)];
        for v in ["1", "yes", "3000", "gpu-b", ""] { assert!(meets(&c, "gpu", v), "`{v}`"); }
        for v in ["2", "0", "no", "5000"] { assert!(!meets(&c, "gpu", v), "`{v}`"); }
        c.gpus.clear();
        for v in ["0", "no"] { assert!(meets(&c, "gpu", v), "`{v}`"); }
        for v in ["1", "yes", "3000", ""] { assert!(!meets(&c, "gpu", v), "`{v}`"); }
    }

    /// Two Macs fit an interactive test ask and a timed lease holds one: the ask starts on the
    /// other at once. One that names the held Mac waits for it, and its queue line says it named
    /// that box and which other one could start it now.
    #[test]
    fn a_mac_ask_beside_a_timed_mac_takes_the_other_mac_and_a_named_one_says_so() {
        let mac = |role| {
            let mut c = caps(10, 16.0, role);
            (c.arch, c.os, c.gpus) = ("aarch64".into(), "macos".into(), Vec::new());
            c
        };
        let mut s = sched(&[("bench", mac(Role::Bench)), ("spare", mac(Role::Hot))]);
        let on_mac = |lane: &str, kind: Kind, node: Option<&str>| {
            let mut r = overlay(lane, node, 4.0);
            r.needs = vec![("arch".into(), "arm64".into()), ("os".into(), "macos".into())];
            (r.kind, r.ram_gb, r.live, r.priority) = (kind, Some(6.0), kind == Kind::Test, Priority::Interactive);
            r
        };
        let mut timed = req("timing", Kind::Timed, Some("bench"));
        timed.needs = vec![("os".into(), "macos".into())];
        let timed = s.request(timed, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("bench"), "{:?}", state(&s, &timed));
        for _ in 0..3 { s.record_node_cost("test makepad run makepad-foo", "test", "bench", 714_004); }
        let mut free = on_mac("free", Kind::Test, None);
        free.class = "test makepad run makepad-foo".into();
        let free = s.request(free, 1);
        beat_all(&mut s, 1);
        s.tick(1);
        assert_eq!(node_of(&s, &free).as_deref(), Some("spare"), "{:?}", state(&s, &free));
        release_clean(&mut s, &free, 2);
        let named = s.request(on_mac("named", Kind::Test, Some("bench")), 3);
        beat_all(&mut s, 3);
        s.tick(3);
        assert_eq!(node_of(&s, &named), None, "{:?}", state(&s, &named));
        let line = state(&s, &named).1;
        assert!(line.contains(&format!("held by {timed}")) && line.ends_with("; it named bench (--node): spare could start it now"), "{line}");
        invariants(&s);
    }

    /// Mac work that names no box and waits on one Mac only because a live session's peak holds
    /// its RAM budget takes the other Mac in a gap of its timed queue, background and interactive
    /// work alike; while a timed request waits for the bench Mac, interactive work stays off it
    /// (its quiet window) and background work fits in only when it ends first.
    #[test]
    fn mac_work_a_live_peak_keeps_off_one_mac_takes_the_other_in_a_timed_gap() {
        let mac = |role, resident| {
            let mut c = caps(10, 16.0, role);
            (c.arch, c.os, c.gpus, c.ram_resident_gb) = ("aarch64".into(), "macos".into(), Vec::new(), resident);
            c
        };
        let mut s = sched(&[("bench", mac(Role::Bench, 4.0)), ("hot", mac(Role::Hot, 5.0))]);
        let on_mac = |lane: &str, kind: Kind, ram: Option<f64>, node: Option<&str>| {
            let mut r = overlay(lane, node, 1.0);
            r.needs = vec![("os".into(), "macos".into())];
            (r.kind, r.ram_gb) = (kind, ram);
            r
        };
        let mut live = on_mac("repro", Kind::Test, None, Some("hot"));
        (live.live, live.priority) = (true, Priority::Interactive);
        let live = s.request(live, 0);
        assert_eq!(grants(&s.tick(0)), vec![live.clone()], "{:?}", state(&s, &live));
        s.set_info(&live, r#"{"running":"makepad","held_mb":9830}"#, 0);
        let bg = s.request(on_mac("bg", Kind::Test, Some(3.6), None), 1);
        let mut fg = on_mac("fg", Kind::Test, Some(3.6), None);
        fg.priority = Priority::Interactive;
        let fg = s.request(fg, 1);
        beat_all(&mut s, 1);
        s.tick(1);
        assert_eq!(node_of(&s, &bg).as_deref(), Some("bench"), "{:?}", state(&s, &bg));
        assert_eq!(node_of(&s, &fg).as_deref(), Some("bench"), "{:?}", state(&s, &fg));
        release_clean(&mut s, &bg, 2);
        release_clean(&mut s, &fg, 2);
        // a timed request waits for the bench Mac: interactive work keeps off it
        let mut timed = on_mac("bench-series", Kind::Timed, None, None);
        timed.cost = CostClass::Medium;
        let timed = s.request(timed, 3);
        let mut fg = on_mac("fg2", Kind::Test, Some(3.6), None);
        fg.priority = Priority::Interactive;
        let fg = s.request(fg, 3);
        beat_all(&mut s, 3);
        s.tick(3);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("bench"), "{:?}", state(&s, &timed));
        assert_eq!(node_of(&s, &fg), None, "{:?}", state(&s, &fg));
        invariants(&s);
    }

    #[test]
    fn a_bench_mac_out_of_rotation_sends_timed_work_to_the_spare_mac() {
        let mac = |role| {
            let mut c = caps(10, 16.0, role);
            (c.arch, c.os, c.gpus) = ("aarch64".into(), "macos".into(), Vec::new());
            c
        };
        let mut bench = mac(Role::Bench);
        bench.sick = "git fails on its stores: git bundle: fatal: Unable to read current working directory".into();
        let mut s = sched(&[("m1", bench), ("m2", mac(Role::Hot))]);
        let mut r = req("a", Kind::Timed, None);
        r.needs = vec![("arch".into(), "arm64".into()), ("os".into(), "macos".into())];
        let a = s.request(r, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &a).as_deref(), Some("m2"), "{:?}", state(&s, &a));
        invariants(&s);
    }

    #[test]
    fn a_mac_past_normal_memory_pressure_starts_nothing_new() {
        let mac = |level| {
            let mut c = caps(10, 16.0, Role::Hot);
            (c.arch, c.os, c.gpus, c.mem_pressure) = ("aarch64".into(), "macos".into(), Vec::new(), level);
            c
        };
        let ask = |lane: &str| {
            let mut r = overlay(lane, Some("mac"), 1.0);
            (r.needs, r.ram_gb) = (vec![("os".into(), "macos".into())], Some(1.0));
            r
        };
        let mut s = sched(&[("mac", mac(1))]);
        let a = s.request(ask("a"), 0);
        assert_eq!(grants(&s.tick(0)), vec![a.clone()]);
        s.set_caps("mac", mac(2), 10);
        let b = s.request(ask("b"), 10);
        assert!(grants(&s.tick(10)).is_empty());
        let (st, why) = state(&s, &b);
        assert!(st == "queued" && why.contains("mac pressure warn"), "{st}: {why}");
        assert!(matches!(state(&s, &a).0.as_str(), "starting" | "granted"), "what runs there goes on: {:?}", state(&s, &a));
        s.set_caps("mac", mac(4), 20);
        s.tick(20);
        assert!(state(&s, &b).1.contains("mac pressure critical"), "{:?}", state(&s, &b));
        s.set_caps("mac", mac(1), 30);
        assert_eq!(grants(&s.tick(30)), vec![b], "back at normal, it starts");
        invariants(&s);
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

    /// `test --node N` pins the box: held or busy, N keeps it waiting and a twin with room never
    /// takes it (an A/B's before ran on one box and its afters on two others, 11-03).
    #[test]
    fn test_node_pins_the_box() {
        let mut mac = caps(16, 64.0, Role::Pool);
        mac.arch = "aarch64".into();
        mac.os = "macos".into();
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("m", mac)]);
        let fill = s.request(req("f1", Kind::Test, Some("a")), 0);
        s.tick(0);
        assert_eq!(node_of(&s, &fill).as_deref(), Some("a"));
        let x = s.request(req("x", Kind::Test, Some("a")), 1);
        for now in [2, 600_000] {
            beat_all(&mut s, now);
            s.tick(now);
            assert_eq!(node_of(&s, &x), None, "b has room, but x named a");
        }
        assert!(state(&s, &x).1.contains(&format!("held by {fill}")), "{:?}", state(&s, &x));
        release_clean(&mut s, &fill, 600_001);
        beat_all(&mut s, 600_002);
        s.tick(600_002);
        assert_eq!(node_of(&s, &x).as_deref(), Some("a"));
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
    /// A timed lease waiting for a holds a: what names a waits (held for timed); the timed lease
    /// itself waits only for a's running lease.
    #[test]
    fn a_timed_hold_blocks_only_leases_that_need_that_node() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        let running = s.request(req("run", Kind::Test, Some("a")), 0);
        s.tick(0);
        let timed = s.request(req("bench", Kind::Timed, Some("a")), 1);
        let build = s.request(req("lane", Kind::Build, Some("a")), 2);
        let strict = s.request(req("strict", Kind::Test, Some("a")), 3);
        s.tick(3);
        assert!(state(&s, &timed).1.contains(&format!("held by {running}")), "{:?}", state(&s, &timed));
        for l in [&build, &strict] {
            assert!(node_of(&s, l).is_none() && state(&s, l).1.contains("held for timed"), "{:?}", state(&s, l));
        }
        release_clean(&mut s, &running, 4);
        beat_all(&mut s, 5);
        s.tick(5);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("a"));
    }

    /// A bench box off the TSC (the kernel marked it unstable at boot) takes no timed lease: its
    /// clock reads are slow syscalls that inflate what a timed run measures. Timed work goes to
    /// another bench box; one that names it is refused, saying why; the box is said once.
    #[test]
    fn a_bench_box_off_the_tsc_takes_no_timed_lease() {
        let bench = |clock: &str| { let mut c = caps(16, 64.0, Role::Bench); c.timed = true; c.clocksource = clock.into(); c };
        let mut s = sched(&[("slow", bench("hpet")), ("fast", bench("tsc"))]);
        let said = |s: &Sched| s.out.iter().filter(|a| matches!(a, Action::Event(r) if r.line.contains("slow: no timed leases: clocksource hpet (TSC unstable since boot; a reboot restores it)"))).count();
        assert_eq!(said(&s), 1, "said when its first caps come");
        s.set_caps("slow", bench("hpet"), 0);
        assert_eq!(said(&s), 1, "said when it changes, not on every caps");
        s.set_caps("slow", bench("tsc"), 0);
        s.set_caps("slow", bench("hpet"), 0);
        assert_eq!(said(&s), 2);
        let any = s.request(req("bench", Kind::Timed, None), 1);
        let named = s.request(req("series", Kind::Timed, Some("slow")), 1);
        s.tick(1);
        assert_eq!(node_of(&s, &any).as_deref(), Some("fast"), "{:?}", state(&s, &any));
        let (st, why) = state(&s, &named);
        assert!(st == "ended denied" && why.contains("no timed leases: clocksource hpet"), "{st}: {why}");
        // other work still runs there
        let build = s.request(req("lane", Kind::Build, Some("slow")), 2);
        s.tick(2);
        assert_eq!(node_of(&s, &build).as_deref(), Some("slow"), "{:?}", state(&s, &build));
    }

    /// When a box frees, the oldest waiting ask that fits starts first. A chain of timed requests
    /// that keeps refilling drains the box only against asks younger than each: a small test asked
    /// before the next timed request starts when the running one ends, that timed request right
    /// after it; a test asked after a timed request waits for it.
    #[test]
    fn an_older_ask_starts_ahead_of_a_refilling_timed_chain() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Bench))]);
        // one timed lease per lane: the chain is several lanes' timed work
        let timed = |s: &mut Sched, now: u64| s.request(req(&format!("bench{now}"), Kind::Timed, Some("a")), now);
        // each outlasts what runs before the next timed lease could start: none ends first
        let small = |lane: &str| {
            let mut r = req(lane, Kind::Test, Some("a"));
            r.est_min = Some(30.0);
            r
        };
        let t1 = timed(&mut s, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &t1).as_deref(), Some("a"));
        let old = s.request(small("old"), 1);
        let t2 = timed(&mut s, 2);
        s.tick(2);
        assert!(state(&s, &old).1.contains(&format!("held by {t1}")), "{:?}", state(&s, &old));
        release_clean(&mut s, &t1, 3);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(node_of(&s, &old).as_deref(), Some("a"), "the older ask starts first: {:?}", state(&s, &old));
        assert!(node_of(&s, &t2).is_none() && state(&s, &t2).1.contains(&format!("held by {old}")), "{:?}", state(&s, &t2));
        // the chain refills; a test asked after its next request waits for that one
        let t3 = timed(&mut s, 5);
        let young = s.request(small("young"), 6);
        s.tick(6);
        release_clean(&mut s, &old, 7);
        beat_all(&mut s, 8);
        s.tick(8);
        assert_eq!(node_of(&s, &t2).as_deref(), Some("a"), "the timed lease right after it: {:?}", state(&s, &t2));
        release_clean(&mut s, &t2, 9);
        beat_all(&mut s, 10);
        s.tick(10);
        assert_eq!(node_of(&s, &t3).as_deref(), Some("a"), "{:?}", state(&s, &t3));
        assert!(node_of(&s, &young).is_none(), "{:?}", state(&s, &young));
        invariants(&s);
    }

    /// A box's own prebuild passes the drain for a timed lease that is not running yet; other work
    /// waits for the timed lease, and the timed lease waits for the prebuild.
    #[test]
    fn a_box_prebuild_goes_before_timed_work_that_waits_for_the_box() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let running = s.request(req("run", Kind::Test, Some("a")), 0);
        s.tick(0);
        let timed = s.request(req("bench", Kind::Timed, Some("a")), 1);
        s.tick(1);
        let build = |lane: &str, min: f64| {
            let mut r = overlay(lane, Some("a"), 4.0);
            r.kind = Kind::Build;
            r.est_min = Some(min);
            r
        };
        // longer than what runs there: no backfill
        let other = s.request(build("lane", 600.0), 2);
        let prebuild = s.request(build("prebuild-a", 5.0), 2);
        s.tick(2);
        assert!(state(&s, &other).1.contains(&format!("held for timed {timed}")), "{:?}", state(&s, &other));
        assert_eq!(node_of(&s, &prebuild).as_deref(), Some("a"), "{:?}", state(&s, &prebuild));
        let line = s.get(&timed).unwrap().state_line(2, "");
        assert!(line.contains("\"eta_box\":\"a\""), "{line}");
        release_clean(&mut s, &running, 3);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(node_of(&s, &timed), None, "the timed run waits for the box's prebuild");
        release_clean(&mut s, &prebuild, 5);
        beat_all(&mut s, 6);
        s.tick(6);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("a"));
        // the timed run is alone on its box: the box's next prebuild waits for its end
        let next = s.request(build("prebuild-a", 5.0), 7);
        s.tick(7);
        assert_eq!(node_of(&s, &next), None, "{:?}", state(&s, &next));
    }

    /// A background leg owed a box reserves it for as long as it waits: newer lane work that
    /// would not fit beside it and outlasts what runs there waits; work that ends first still goes.
    #[test]
    fn an_owed_leg_keeps_its_box_from_newer_lane_work() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.est_min = Some(min);
            r
        };
        let running = s.request(ask("run", 6.0, 10.0), 0);
        s.tick(0);
        let mut leg = ask("jitlegs-x", 12.0, 5.0);
        leg.agent = Some("devhub-jitlegs".into());
        leg.prio = DEFAULT_PRIO - 1;
        let leg = s.request(leg, 1);
        s.tick(1);
        let lane = s.request(ask("lane", 6.0, 60.0), 2);
        let quick = s.request(ask("quick", 6.0, 1.0), 2);
        s.tick(2);
        assert!(state(&s, &lane).1.contains(&format!("held for owed {leg}")), "{:?}", state(&s, &lane));
        assert_eq!(node_of(&s, &quick).as_deref(), Some("a"), "it ends before the leg could start: {:?}", state(&s, &quick));
        // past its first ten minutes the reservation still holds
        beat_all(&mut s, HOLD_MS + 3);
        s.tick(HOLD_MS + 3);
        assert!(state(&s, &lane).1.contains(&format!("held for owed {leg}")), "{:?}", state(&s, &lane));
        release_clean(&mut s, &running, HOLD_MS + 4);
        release_clean(&mut s, &quick, HOLD_MS + 4);
        beat_all(&mut s, HOLD_MS + 5);
        s.tick(HOLD_MS + 5);
        assert_eq!(node_of(&s, &leg).as_deref(), Some("a"));
    }

    /// A box back from an outage (its reservations went with it) goes to the leg owed it before
    /// newer interactive work, and work it held off before still waits.
    #[test]
    fn a_box_back_from_an_outage_goes_to_the_leg_owed_it_first() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.est_min = Some(min);
            r
        };
        let running = s.request(ask("run", 10.0, 10.0), 0);
        s.tick(0);
        let mut leg = ask("jitlegs-x", 12.0, 5.0);
        leg.agent = Some("devhub-jitlegs".into());
        leg.prio = DEFAULT_PRIO - 1;
        let leg = s.request(leg, 1);
        s.tick(1);
        let short = s.request(ask("short", 8.0, 0.2), 1);
        s.tick(1);
        assert!(state(&s, &short).1.contains(&format!("held for owed {leg}")), "{:?}", state(&s, &short));
        s.nodes.get_mut("a").unwrap().up = false;
        let mut live = ask("live", 8.0, 35.0);
        live.priority = Priority::Interactive;
        let live = s.request(live, 2);
        s.tick(2);
        release_clean(&mut s, &running, 3);
        s.nodes.get_mut("a").unwrap().up = true;
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!((node_of(&s, &leg).as_deref(), node_of(&s, &live)), (Some("a"), None), "{:?}", state(&s, &live));
    }

    /// A waiting timed lease holds an idle node (it starts now), not its lane's busy one.
    #[test]
    fn a_timed_lease_holds_an_idle_node_over_its_busy_sticky_one() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        let first = s.request(req("bench", Kind::Build, Some("a")), 0);
        s.tick(0);
        release_clean(&mut s, &first, 1);
        s.request(req("other", Kind::Test, Some("a")), 2);
        s.tick(2);
        let timed = s.request(req("bench", Kind::Timed, None), 3);
        s.tick(3);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("b"));
    }

    /// No box is granted past its cores: what its running work was charged, or what the box
    /// measured busy when that is more (live sessions at their idle share, busy all the same). A
    /// lease that finds every box full queues and says why; an empty box takes a lease whatever
    /// its own services use.
    #[test]
    fn a_box_is_never_granted_past_its_cores_as_measured() {
        let mut busy = caps(16, 64.0, Role::Pool);
        busy.cpu_pct = Some(90.0);
        let mut s = sched(&[("a", busy.clone())]);
        let overlay = |lane: &str| {
            let mut r = req(lane, Kind::Test, None);
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r
        };
        let first = s.request(overlay("one"), 0);
        assert_eq!(grants(&s.tick(0)), vec![first.clone()], "an empty box: its own use is no lease's");
        // charged 4 of 16, measured 14.4: one more test's 4 would pass its 16
        let second = s.request(overlay("two"), 1);
        assert!(grants(&s.tick(1)).is_empty());
        assert_eq!(state(&s, &second), ("queued".to_string(), "budget cores 16, in use 14 (the box measured busy; charged 4), asked 4".to_string()));
        busy.cpu_pct = Some(50.0);
        s.set_caps("a", busy, 2);
        assert_eq!(grants(&s.tick(2)), vec![second], "8 measured and 4 asked fit");
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
        r.src = Src::Overlay;
        r.rev = Some(format!("work@{}", "a".repeat(40)));
        r.cores = Some(cores);
        r
    }

    /// Check every candidate, including partially occupied boxes, not just empty ones. Holds
    /// are checked independently of now_fits so an invalid reservation cannot hide idle capacity.
    fn invariants(s: &Sched) {
        for l in s.leases.iter().filter(|l| l.state == State::Queued) {
            if let Ok(names) = s.candidates(l) {
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
                        assert!(!s.candidates(r).is_ok_and(|names| names.contains(&name)), "timed hold hides interactive demand {} on {name}", r.id);
                    }
                }
            }
            assert!(s.resources(name, l, &cost, true).is_err(), "{} holds an available box {name}", h.id);
            let names = s.candidates(l).ok().unwrap();
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
        let nodes: Vec<_> = ["b", "a", "c", "d", "e", "f", "g", "h", "hot"].into_iter().map(|n| (n, caps(16, 64.0, if n == "hot" { Role::Hot } else { Role::Pool }))).collect();
        let mut s = sched(&nodes);
        for name in ["b", "a", "c"] {
            s.request(req(name, Kind::Build, Some(name)), 0);
        }
        s.tick(0);
        let timed = s.request(req("timed-a", Kind::Timed, Some("b")), 1);
        s.tick(1);
        assert_eq!(s.holds["b"].id, timed);
        // A required protocol update makes the node unready; its old hold must not persist.
        s.nodes.get_mut("b").unwrap().caps.as_mut().unwrap().hub_proto = 0;
        let cleanup = s.leases.iter().find(|l| l.node.as_deref() == Some("c")).unwrap().id.clone();
        s.release(&cleanup, "released", 2);
        s.request(req("timed-b", Kind::Timed, Some("a")), 2);
        for name in ["b", "a"] { s.request(req("ci", Kind::Ci, Some(name)), 2); }
        let mut interactive = req("interactive", Kind::Test, Some("c"));
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
        assert!(!s.holds.contains_key("b"), "an incompatible hub cannot own a timed reservation");
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

    /// (`running 13m59s against its measured 9 s`: another `bash sweep.sh` of the lane, 11-04)
    #[test]
    fn a_lanes_script_is_not_stalled_against_its_names_walls() {
        let class = "long bash sweep.sh @sweep";
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        s.walls.entry((class.to_string(), "long".to_string())).or_default().extend([9_000, 9_000, 9_000]);
        let mut r = req("sweep", Kind::Long, Some("a"));
        r.class = class.to_string();
        let run = s.request(r, 0);
        assert_eq!(grants(&s.tick(0)), vec![run.clone()]);
        s.started(&run, "{}");
        beat_all(&mut s, 839_000);
        let said: Vec<_> = s.tick(839_000).into_iter().filter(|a| matches!(a, Action::Event(r) if r.line.contains("stalled?"))).collect();
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn a_ci_whole_ask_that_cannot_start_yet_holds_no_lane_work() {
        let mut s = sched(&[("node-b", caps(16, 64.0, Role::Pool))]);
        let long = s.request(overlay("long", Some("node-b"), 4.0), 0);
        s.tick(0);
        let mut r = req("ci", Kind::Ci, Some("node-b"));
        r.whole = true;
        let whole = s.request(r, 1);
        s.tick(1);
        assert_eq!(s.holds["node-b"].id, whole);
        // background lane work that does not fit before the ask could start goes all the same
        let mut r = overlay("build", Some("node-b"), 4.0);
        r.live = true;
        let build = s.request(r, 2);
        assert_eq!(grants(&s.tick(2)), vec![build.clone()], "lane work goes first while the CI ask cannot start");
        assert_eq!(s.holds["node-b"].id, whole, "the ask keeps its place");
        invariants(&s);
        // another CI shard stays behind it
        let mut r = req("ci", Kind::Ci, Some("node-b"));
        r.cores = Some(2.0);
        r.live = true;
        let shard = s.request(r, 3);
        assert!(grants(&s.tick(3)).is_empty());
        assert!(state(&s, &shard).1.contains(&format!("held for whole {whole}")), "{}", state(&s, &shard).1);
        // and the ask still gets the box once what ran is gone
        release_clean(&mut s, &long, 4);
        release_clean(&mut s, &build, 4);
        assert_eq!(grants(&s.tick(4)), vec![whole]);
    }

    #[test]
    fn a_background_whole_ask_that_cannot_start_yet_holds_no_interactive_work() {
        let mut s = sched(&[("node-b", caps(16, 64.0, Role::Pool))]);
        let long = s.request(overlay("long", Some("node-b"), 4.0), 0);
        s.tick(0);
        let mut r = overlay("box", Some("node-b"), 4.0);
        r.whole = true;
        let whole = s.request(r, 1);
        s.tick(1);
        assert_eq!(s.holds["node-b"].id, whole);
        // a live session never backfills: nothing but this rule lets it in
        let mut r = overlay("session", Some("node-b"), 4.0);
        r.priority = Priority::Interactive;
        r.live = true;
        let session = s.request(r, 2);
        assert_eq!(grants(&s.tick(2)), vec![session.clone()], "interactive work goes first while the whole ask cannot start");
        assert_eq!(s.holds["node-b"].id, whole, "the ask keeps its place");
        invariants(&s);
        // background work stays behind it
        let mut r = overlay("background", Some("node-b"), 4.0);
        r.live = true;
        let background = s.request(r, 3);
        assert!(grants(&s.tick(3)).is_empty());
        assert!(state(&s, &background).1.contains(&format!("held for whole {whole}")), "{}", state(&s, &background).1);
        // and the whole ask still gets the box once what ran is gone
        release_clean(&mut s, &long, 4);
        release_clean(&mut s, &session, 4);
        assert_eq!(grants(&s.tick(4)), vec![whole]);
    }

    #[test]
    fn reservations_allow_companions_expire_and_do_not_chain() {
        let mut s = sched(&[("node-b", caps(16, 64.0, Role::Pool))]);
        let running = s.request(overlay("running", Some("node-b"), 12.0), 0);
        s.tick(0);
        let mut r = overlay("interactive", Some("node-b"), 8.0);
        r.priority = Priority::Interactive;
        let held = s.request(r, 1);
        s.tick(1);
        let companion = s.request(overlay("companion", Some("node-b"), 4.0), 2);
        assert_eq!(grants(&s.tick(2)), vec![companion]);
        assert_eq!(s.holds["node-b"].id, held);
        invariants(&s);
        release_clean(&mut s, &running, 3);
        assert_eq!(grants(&s.tick(3)), vec![held]);
        assert!(s.holds.is_empty());

        let mut s = sched(&[("node-b", caps(16, 64.0, Role::Pool)), ("node-a", caps(16, 64.0, Role::Pool))]);
        for name in ["node-b", "node-a"] {
            s.request(req(name, Kind::Test, Some(name)), 0);
        }
        s.tick(0);
        let mut r = req("interactive", Kind::Test, Some("node-b"));
        r.priority = Priority::Interactive;
        let interactive = s.request(r, 1);
        s.tick(1);
        let timed = s.request(req("timed", Kind::Timed, None), 2);
        let ci = s.request(req("ci", Kind::Ci, Some("node-a")), 2);
        s.tick(2);
        invariants(&s);
        assert_eq!(s.holds.len(), 1);
        assert_eq!(s.holds["node-b"].id, interactive);
        assert_eq!(node_of(&s, &ci).as_deref(), Some("node-a"));
        assert_eq!(state(&s, &timed).0, "queued");

        let mut s = sched(&[("node-b", caps(16, 64.0, Role::Pool))]);
        let running = s.request(req("running", Kind::Test, Some("node-b")), 0);
        s.tick(0);
        let timed = s.request(req("timed", Kind::Timed, Some("node-b")), 1);
        s.tick(1);
        let mut r = req("ci", Kind::Ci, Some("node-b"));
        r.est_min = Some(120.0);
        let ci = s.request(r, 2);
        s.tick(2);
        let why = s.why_queued(&ci, 180_001).unwrap().join("\n");
        assert!(why.contains(&format!("held for timed {timed} (the box drains for it")) && why.contains(&format!("\"blockers\":[\"{timed}\"]")), "{why}");
        beat_all(&mut s, HOLD_MS + 1);
        assert!(grants(&s.tick(HOLD_MS + 1)).is_empty(), "a timed reservation lasts while it waits: work that outlives the drain stays off");
        assert_eq!(s.holds["node-b"].id, timed);
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
        // other work waits for a pool box rather than take a bench box (pool_takes), warm or
        // not; a queued interactive lease claims a bench box only by name
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
        assert_eq!(node_of(&s, &next).as_deref(), Some("pool"), "a busy pool box over a bench box with room: {:?}", state(&s, &next));
        let mut s2 = sched(&[("bench1", caps(64, 256.0, Role::Bench)), ("pool", caps(64, 256.0, Role::Pool))]);
        s2.warm.insert(("warm".into(), "bench1".into()));
        let warm = s2.request(overlay("warm", Kind::Test), 0);
        s2.tick(0);
        assert_eq!(node_of(&s2, &warm).as_deref(), Some("pool"), "a cold pool box over the lane's warm bench box");
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
        assert!(state(&t, &waits).1.starts_with("budget cores 16, in use 16"), "{:?}", state(&t, &waits));
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
        s.nodes.get_mut("hot-a").unwrap().caps.as_mut().unwrap().load1 = 8.0;
        let mut r = req("medium", Kind::Test, Some("hot-a"));
        r.priority = Priority::Interactive;
        r.cost = CostClass::Medium;
        let medium = s.request(r, 120_000);
        // Small can use either hot box, and takes the idler one. The earlier medium is
        // revisited immediately, without waiting another tick for small demand to disappear.
        let mut r = req("small", Kind::Test, None);
        r.since_ms = Some(0);
        let small = s.request(r, 120_000);
        assert_eq!(grants(&s.tick(120_000)), vec![small.clone(), medium.clone()]);
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
        full.class = "test:foo:bin:foo".into();
        let mut filtered = full.clone();
        filtered.class = "test:foo:bin:foo?bench::one_case".into();
        for _ in 0..3 { s.record_resources(&filtered.class, "test", Some(0.7), None); }
        let (f, w) = (s.request(filtered.clone(), 0), s.request(full.clone(), 0));
        let ram = |s: &Sched, id: &str| s.cost_on("pool", s.get(id).unwrap()).ram_gb;
        let default = full.allocation(false).ram_gb;
        assert!((ram(&s, &f) - 0.84).abs() < 1e-9, "the filter's own peak: {}", ram(&s, &f));
        assert_eq!(ram(&s, &w), default, "the suite has no runs: its kind's default, not the filter's 0.7 GB");
        // a suite run over its ask is no failure: its peak and a fifth is the next ask
        s.record_resources(&full.class, "test", Some(4.7), None);
        assert!((ram(&s, &w) - 4.7 * 1.2).abs() < 1e-9, "{}", ram(&s, &w));
        // a filter without runs of its own asks for the whole suite's highest
        let mut other = full.clone();
        other.class = "test:foo:bin:foo?other".into();
        let o = s.request(other, 0);
        assert!((ram(&s, &o) - 4.7 * 1.2).abs() < 1e-9, "{}", ram(&s, &o));
        // and a measured ask caps nothing: only an asked RAM is a scope's MemoryMax
        let cost = crate::lease::Cost { cores: 4.0, ram_gb: 0.84, disk_gb: 1.0, gpus: 0 };
        assert!(!crate::os::lease_argv(vec!["x".into()], Some("u"), 10, "", Some(cost), None).iter().any(|a| a.starts_with("MemoryMax")));
        assert_eq!(crate::os::lease_argv(vec!["x".into()], Some("u"), 10, "", Some(cost), Some(2.0)).iter().any(|a| a == "MemoryMax=3G"), cfg!(target_os = "linux"));
    }

    /// A box's residents (its WM, services, hub) come off its RAM budget: a 30 GB box holding 4 GB
    /// gives its leases 23 GB (30 less 10% less 4), not the 25.5 GB of 85% alone (a gate granted
    /// 24 GB there ran the box out of memory, 11-03). An asked 24 GB waits there, and starts once the
    /// residents shrink: nothing is refused for good. A measured 24 GB goes to a box that holds it
    /// whole; where none can, it runs alone on the cut budget.
    #[test]
    fn residents_come_off_the_ram_budget() {
        let mut small = caps(16, 30.0, Role::Pool);
        small.ram_resident_gb = 4.0;
        let mut s = sched(&[("small", small.clone())]);
        let mut asked = overlay("asked", None, 4.0);
        asked.ram_gb = Some(24.0);
        let a = s.request(asked, 0);
        assert!(grants(&s.tick(0)).is_empty(), "a 30 GB box with 4 GB resident takes no 24 GB ask");
        let (st, why) = state(&s, &a);
        assert!(st == "queued" && why.contains("budget ram 23.0 GB, in use 0.0 GB, asked 24.0 GB (30 GB less 10% and the 4.0 GB its residents hold)"), "{st}: {why}");
        invariants(&s);
        small.ram_resident_gb = 1.0;
        s.set_caps("small", small.clone(), 10);
        assert_eq!(grants(&s.tick(10)), vec![a.clone()], "its residents shrank: 25.5 GB (85%) holds it");
        release_clean(&mut s, &a, 20);

        let mut gate = overlay("gate", None, 4.0);
        gate.class = "gate fast".into();
        small.ram_resident_gb = 4.0;
        let mut big = caps(16, 62.0, Role::Pool);
        big.ram_resident_gb = 4.0;
        // busier, so only the whole budget makes it the better box
        big.load1 = 4.0;
        let mut s = sched(&[("small", small.clone()), ("big", big)]);
        for _ in 0..3 { s.record_resources(&gate.class, "test", Some(20.0), None); }
        let g = s.request(gate.clone(), 0);
        assert_eq!(grants(&s.tick(0)), vec![g.clone()]);
        assert_eq!(s.get(&g).unwrap().node.as_deref(), Some("big"), "the box whose budget holds its measured 24 GB whole");

        let mut s = sched(&[("small", small)]);
        for _ in 0..3 { s.record_resources(&gate.class, "test", Some(20.0), None); }
        let g = s.request(gate, 0);
        assert_eq!(grants(&s.tick(0)), vec![g.clone()], "no box holds it whole: it runs on the cut budget, never denied");
        assert_eq!(s.get(&g).unwrap().cost.ram_gb, 23.0);
        let mut beside = overlay("beside", None, 1.0);
        beside.ram_gb = Some(1.0);
        let b = s.request(beside, 1);
        assert!(grants(&s.tick(1)).is_empty(), "nothing shares the budget it fills");
        assert!(state(&s, &b).1.contains("budget ram 23.0 GB, in use 23.0 GB, asked 1.0 GB"), "{:?}", state(&s, &b));
    }

    /// A hot Mac keeps no RAM for the top lane, whose boots run on Linux: on a 16 GB Mac whose OS
    /// and apps hold 5.4 GB, a 2 GB live session leaves 6 GB of its 8 GB budget, room for two 3 GB
    /// macOS leases (with the reserve its budget was 3 GB and nothing fit beside a session,
    /// 11-04). A hot Linux box keeps the reserve, and a wait says budget, in use and asked.
    #[test]
    fn a_hot_mac_keeps_no_top_reserve() {
        let mut mac = caps(10, 16.0, Role::Hot);
        (mac.os, mac.arch, mac.ram_resident_gb) = ("macos".into(), "aarch64".into(), 5.4);
        let mut s = sched(&[("mac", mac)]);
        let on_mac = |lane: &str, ram: f64| {
            let mut r = overlay(lane, None, 2.0);
            r.needs = vec![("os".into(), "macos".into())];
            r.ram_gb = Some(ram);
            r
        };
        let session = s.request(on_mac("session", 2.0), 0);
        assert_eq!(grants(&s.tick(0)), vec![session]);
        let a = s.request(on_mac("a", 3.0), 1);
        let b = s.request(on_mac("b", 3.0), 1);
        let mut granted = grants(&s.tick(1));
        granted.sort();
        assert_eq!(granted, vec![a.clone(), b.clone()], "{:?} {:?}", state(&s, &a), state(&s, &b));
        let c = s.request(on_mac("c", 1.0), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert!(state(&s, &c).1.contains("budget ram 8.0 GB, in use 8.0 GB, asked 1.0 GB (16 GB less 16% and the 5.4 GB its residents hold)"), "{:?}", state(&s, &c));
        invariants(&s);

        let mut hot = caps(16, 30.0, Role::Hot);
        hot.ram_resident_gb = 4.0;
        let mut s = sched(&[("hot", hot)]);
        let mut big = overlay("big", None, 2.0);
        big.ram_gb = Some(20.0);
        let g = s.request(big, 0);
        assert!(grants(&s.tick(0)).is_empty());
        let why = state(&s, &g).1;
        assert!(why.contains("budget ram 18.0 GB, in use 0.0 GB, asked 20.0 GB (30 GB less 10% and the 4.0 GB its residents hold, less 5 GB kept for landq-hot)"), "{why}");
    }

    /// An interactive prebuild that cannot start beside a live job (which
    /// runs until parked) holds no box, and a 30-min leg that fits beside the job starts. Before,
    /// its reservation held the box and the leg, too long to backfill, waited behind it.
    #[test]
    fn background_work_ends_only_for_a_lease_its_end_starts() {
        let mut s = sched(&[("a", caps(8, 64.0, Role::Pool)), ("b", caps(8, 64.0, Role::Pool))]);
        let shard = |s: &mut Sched, node: &str| {
            let mut r = overlay("ci", Some(node), 2.0);
            (r.prio, r.ram_gb) = (9, Some(2.0));
            s.request(r, 0)
        };
        let (on_a, on_b) = (shard(&mut s, "a"), shard(&mut s, "b"));
        let lane = |lane: &str, node: &str, cores: f64| {
            let mut r = overlay(lane, Some(node), cores);
            r.ram_gb = Some(2.0);
            r
        };
        let running = s.request(lane("busy", "a", 4.0), 0);
        assert_eq!(grants(&s.tick(0)).len(), 3, "{:?} {running}", state(&s, &running));
        // 8 cores: 4 of a lane, 2 of CI
        let fits_once_gone = s.request(lane("waits", "a", 4.0), 1);
        let never_fits_beside = s.request(lane("big", "a", 6.0), 2);
        s.tick(2);
        assert!(s.starts_without(&fits_once_gone, "a", std::slice::from_ref(&on_a), 3));
        assert!(!s.starts_without(&never_fits_beside, "a", std::slice::from_ref(&on_a), 3), "the lane's lease is in its way too");
        assert!(!s.starts_without(&fits_once_gone, "b", std::slice::from_ref(&on_b), 3), "pinned to another box");
        assert!(s.get(&on_a).is_some_and(|l| l.state.live()), "the question ends nothing");
        // it starts on neither box, whatever ends: it needs an OS the boxes do not have
        let mut other = overlay("elsewhere", None, 2.0);
        other.needs = vec![("os".to_string(), "windows".to_string())];
        let other = s.request(other, 3);
        assert!(!s.starts_without(&other, "b", std::slice::from_ref(&on_b), 4));
    }

    #[test]
    fn a_lease_that_cannot_start_after_its_blockers_end_holds_no_box() {
        let mut mac = caps(10, 16.0, Role::Hot);
        (mac.os, mac.arch, mac.ram_resident_gb) = ("macos".into(), "aarch64".into(), 6.4);
        let mut s = sched(&[("mac", mac)]);
        let macos = vec![("os".to_string(), "macos".to_string())];
        let mut live = overlay("session", None, 2.0);
        (live.needs, live.ram_gb, live.live) = (macos.clone(), Some(4.0), true);
        let session = s.request(live, 0);
        assert_eq!(grants(&s.tick(0)), vec![session.clone()]);
        let mut build = req("prebuild-mac", Kind::Build, Some("mac"));
        (build.needs, build.priority, build.cores) = (macos.clone(), Priority::Interactive, Some(4.0));
        let prebuild = s.request(build, 1);
        let mut leg = overlay("jitlegs-arm64", None, 2.0);
        (leg.needs, leg.ram_gb, leg.est_min, leg.priority) = (macos, Some(3.0), Some(30.0), Priority::Background);
        let leg = s.request(leg, 2);
        assert_eq!(grants(&s.tick(2)), vec![leg.clone()], "{:?} {:?}", state(&s, &prebuild), state(&s, &leg));
        let (st, why) = state(&s, &prebuild);
        assert!(st == "queued" && why.contains("budget ram") && !s.holds.values().any(|h| h.id == prebuild), "{st}: {why} {:?}", s.holds);
        invariants(&s);
    }

    #[test]
    fn measured_resources_replace_defaults_but_not_explicit_asks() {
        let mut s = sched(&[("node-e", caps(8, 16.0, Role::Pool))]);
        let mut r = overlay("small-build", Some("node-e"), 8.0);
        r.kind = Kind::Build;
        r.cores = None;
        r.class = "build:small".into();
        let first = s.request(r.clone(), 0);
        assert_eq!(s.cost_on("node-e", s.get(&first).unwrap()).ram_gb, 12.0, "no run measured: its kind's");
        assert!(s.allocation_note("node-e", s.get(&first).unwrap()).contains("ram 12.0 GB (kind)"));
        s.record_resources(&r.class, "build", Some(1.75), Some(2.1));
        assert_eq!(s.cost_on("node-e", s.get(&first).unwrap()).ram_gb, 2.1, "one run measured: its peak and a fifth");
        for _ in 0..2 { s.record_resources(&r.class, "build", Some(1.75), Some(2.1)); }
        let cost = s.cost_on("node-e", s.get(&first).unwrap());
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
        let mut back = sched(&[("node-e", caps(8, 16.0, Role::Pool))]);
        back.restore(&saved[0], 1).unwrap();
        assert_eq!(back.get(&first).unwrap().cost, s.get(&first).unwrap().cost, "restart retains the granted allocation, even without measurements");

        // Only the last nine valid measurements of this class and kind count.
        s.record_resources(&r.class, "build", Some(100.0), Some(100.0));
        for _ in 0..8 { s.record_resources(&r.class, "build", Some(0.1), Some(0.1)); }
        assert_eq!(s.cost_on("node-e", s.get(&first).unwrap()).cores, 8.0, "measured cores are capped at the box");
        s.record_resources(&r.class, "build", Some(0.1), Some(0.1));
        s.record_resources(&r.class, "build", Some(f64::NAN), Some(f64::INFINITY));
        s.record_resources(&r.class, "test", Some(100.0), Some(100.0));
        let cost = s.cost_on("node-e", s.get(&first).unwrap());
        assert_eq!((cost.ram_gb, cost.cores), (0.5, 1.0));
        assert_eq!((s.get(&first).unwrap().cost.ram_gb, s.get(&first).unwrap().cost.cores), (2.1, 3.0), "running allocations stay frozen");

        r.lane = "explicit".into();
        r.ram_gb = Some(5.0);
        r.cores = Some(10.0);
        let explicit = s.request(r, 1);
        let cost = s.cost_on("node-e", s.get(&explicit).unwrap());
        assert_eq!((cost.ram_gb, cost.cores), (5.0, 10.0), "explicit cores are not silently capped");
        assert_eq!(s.allocation_note("node-e", s.get(&explicit).unwrap()), "ram 5.0 GB (asked); cores 10 (asked)");
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
                    else { s.set_info(&id, "{\"running\":\"job\"}", now); }
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
    fn a_pinned_request_round_trips() {
        let r = req("x", Kind::Test, Some("n1"));
        assert_eq!(Req::parse(&r.to_json()).unwrap(), r);
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
