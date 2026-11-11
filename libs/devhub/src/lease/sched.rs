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
/// The refusal of a lane's non-timed ask pinned to a bench box: the bench's quiet window is the only
/// clean number a timed series has.
pub const BENCH_PIN: &str = "a bench box takes timed leases only; ask timed, or pin a pool box";
/// How much sooner another box must start an ask that holds a box for its hold to move there
/// ([`Sched::hold_kept`]).
const HOLD_MOVE_MS: u64 = 120_000;

/// How long a dropped queued ask keeps its place for the same lane's next ask of its kind on its
/// box ([`Sched::request`]): a master restart's drop of its own asks (the new master's prebuild plan
/// asks within its first ticks), and a lane's drop of its own (its holder released it, died or
/// cancelled it: a resubmit).
const DROPPED_ASK_KEEP_MS: u64 = 10 * 60_000;
/// A waiting lease's priority improves by one per this much waiting (starvation guard).
pub const AGE_STEP_MS: u64 = 600_000;
/// A node whose caps go unanswered this long counts as down (polls are every 10 s).
pub const DOWN_AFTER_MS: u64 = 30_000;
/// A box off the network this long (its connects fail as host down, no route or timed out; a
/// restarting hub's are refused) ends its live leases as lost: its hub ends them itself after a
/// minute without the master's beat, and their holders, reading the silence as a hub restart,
/// waited 15 min to re-attach (a pool box left the network holding five leases, 11-07).
pub const HOST_GONE_MS: u64 = 60_000;
/// Background legs owed a box (their run outlasted its fair share, `orch::leg`): verify's
/// correctness legs and jitlegs, asking at one under a lane's default. They reserve their box for
/// as long as they wait, so new lane work does not overtake them there (a jitlegs leg headed the
/// queue 24 min while later lane tests took its box, 11-03).
pub fn owed_leg(l: &Lease) -> bool {
    !l.req.ci() && l.req.prio == DEFAULT_PRIO - 1 && l.req.agent.as_deref().is_some_and(|a| a == "devhub-verify" || a == "devhub-jitlegs")
}

/// The agents of the legs devhub runs on its own (verify, CI sweeps, allapps, jitlegs): their
/// leases are background work, and nobody but devhub watches them.
pub const OWN_LEG_AGENTS: [&str; 4] = ["devhub-verify", "devhub-ci", "devhub-allapps", "devhub-jitlegs"];

/// A leg devhub runs itself, CI aside, asks at its run's place in the queue, not its own ask's
/// (`orch::leg`): what it waited is its own time in the queue ([`Lease::waited_ms`]).
pub(crate) fn runs_place(req: &Req) -> bool {
    !req.ci() && req.agent.as_deref().is_some_and(|a| OWN_LEG_AGENTS.contains(&a))
}

/// The agent of a bucket's gate (bucket.rs): not background work, but nobody watches it either
/// ([`Req::unattended`]).
pub const BUCKET_AGENT: &str = "devhub-bucket";

/// A wait this long becomes a warn (once).
pub const WAIT_WARN_MS: u64 = 600_000;
pub const DEFAULT_PRIO: i32 = 5;
/// The priority of a bumped request (`devhub job bump`): the front of its queue class.
pub const BUMP_PRIO: i32 = i32::MIN / 2;
/// A hold lasts this long past the tick that last renewed it: an ask renews its hold each tick
/// it waits for that box ([`Sched::hold_end`]), so a hold ends when its ask starts, leaves, or no
/// longer waits for the box.
pub const HOLD_MS: u64 = 600_000;
/// Warm live jobs give an exclusive request its box after its current calls finish.
pub const LIVE_PARK_MS: u64 = 60_000;
/// A timed request queued this long stops yielding to interactive demand: it reserves the next
/// box that frees and gets its quiet window. With many live sessions a bench box never went
/// quiet by itself (a timed lease waited past 10 min "held for interactive", 11-03).
pub const TIMED_DUE_MS: u64 = 600_000;

/// A stall is also this far over its measure ([`Sched::say_stalled`]).
const STALL_SLACK_MS: u64 = 300_000;
/// A lease past its measure whose node found its output grown this recently is no stall
/// ([`Sched::say_stalled`]): its beats carry what its runs wrote.
const OUTPUT_FRESH_MS: u64 = 300_000;
/// The least a lease of a class with no measure yet is held to before it is a stall: its asked
/// or kind's length may be a warm run's, and a new class often builds cold first.
const UNMEASURED_RUN_MS: u64 = 300_000;
/// A live session's call (a test session's tests, the run it starts with) this many times its
/// class's measured run, and [`STALL_SLACK_MS`] over it, is said once ([`Sched::say_live_overruns`]).
const LIVE_OVERRUN_TIMES: u64 = 3;
/// How long every process of a lease's runs may do nothing (under [`super::exec::IDLE_CPU_PCT`] of
/// a core, no I/O) before it says so, whatever its class's length.
pub const QUIET_STALL_MS: u64 = 600_000;

/// Under this load a box does nothing: one thread at work holds it near 1.
const BOX_IDLE_LOAD: f64 = 0.5;

/// How long a box stays near idle under a lease, with someone queued on it, before that is said.
const HOLDER_IDLE_MS: u64 = 600_000;
/// A lease that names its box (`--node`) and waits this long while another box could start it
/// now asks its holder to unpin it or say why ([`Sched::say_pinned`]).
const PIN_WAIT_MS: u64 = 5 * 60_000;
/// A timed ask whose every box was not quiet at each look this long, no lease of ours there, is
/// told what keeps it and what would let it start ([`Sched::say_never_quiet`]).
const NEVER_QUIET_MS: u64 = 10 * 60_000;

/// A tests shard waiting this long starts on the RAM and cores its box has free ([`Sched::cost_on`]).
const SHARD_RESIZE_MS: u64 = 10 * 60_000;
/// Lane cargo work charged its class's measured cores waits for them as long as its runs take, at
/// least this, at most [`SHARD_RESIZE_MS`], then starts on the cores its box has free ([`Sched::cores_wait_ms`]).
const CORES_WAIT_MIN_MS: u64 = 2 * 60_000;
/// A landing gate waiting this long starts on the cores its box has free, down to
/// [`GATE_MIN_CORES`] ([`Sched::cost_on`]): every hand-in waits on its verdict.
const GATE_RESIZE_MS: u64 = 60_000;
/// The fewest cores a landing gate starts on: one cold present's (land_gate.sh PRESENT_CORES), its
/// checks and presents split what it got (MAKEPAD_RUN_JOBS).
const GATE_MIN_CORES: f64 = 4.0;

/// An exclusive ask expected to run longer than this is long: it waits for queued exclusive asks
/// that are not ([`Sched::exclusive_order`]), and a running timed one yields to timed asks that
/// are not ([`Sched::say_overruns`]). A 26-min multi-run script held a bench box's timed slot while a
/// 1-min case (declared 6) waited 62 min and a 5m22s one 90 min, and a 5-min line counted neither
/// as short (11-07).
const LONG_TIMED_MS: u64 = 10 * 60_000;
/// A timed run is told to yield ([`Sched::say_overruns`]) a minute in at the earliest.
const OVERRUN_MIN_MS: u64 = 60_000;
/// A queued ask this long in the queue holds the box whose work ends first and drains it, as a
/// whole-box ask does ([`Sched::starved`]).
const STARVED_MS: u64 = 10 * 60_000;
/// A queued ask this long holds its box against interactive work too, and ranks with that work by
/// age ([`Sched::long_overdue`]).
const OVERDUE_MS: u64 = 30 * 60_000;
/// How long past the start a box's head was told it still waits before that start counts as
/// missed ([`Sched::age_heads`]): the box's cleanup and the next look come between.
const ETA_GRACE_MS: u64 = 30_000;
/// Missed starts after which a head drains its box, as one that waited [`STARVED_MS`] does
/// ([`Sched::aged`]).
const ETA_MISSES: u32 = 2;
/// ... and an interactive live session's launch this long: a person or a lane waits on it.
const LIVE_STARVED_MS: u64 = 60_000;
/// The idle verdict as its reader sees it: the inbox and the lease's owner read the same words.
/// A leg or a test run ends once its stacks are in ([`Sched::end_hung_legs`]); other leases stay.
fn idle_line(l: &Lease, node: &str, quiet_ms: u64) -> String {
    let end = if l.req.unattended() || l.req.test_run() { "devhub ends it once they are in".to_string() } else { format!("devhub does not end it (`devhub cancel {}` frees the box)", l.id) };
    format!("{} of {} on {node}: every process idle (under {}% of a core, no I/O) for {}: hung? its stacks: `devhub job result {}`; {end}", l.tag(), l.req.lane, super::exec::IDLE_CPU_PCT, dur(quiet_ms), l.id)
}

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
/// A lane's screen run of an app ([`app_run`]) that nobody looked at or drove this long, past
/// what it declared, ends ([`Sched::app_touched`]): a forgotten one held its box's screen slot
/// and its WM's restart until someone saw it (11-07).
pub const SCREEN_IDLE_MS: u64 = 15 * 60_000;

/// The package a screen class runs as an app (`screen makepad run <package> ..`,
/// [`super::run_class`]): its command is the app itself, open until someone ends it. A script's
/// class (its own driver: a rig, a sheet, a fixture) is none.
fn app_run(class: &str) -> Option<&str> {
    let mut w = class.split_whitespace();
    match (w.next(), w.next(), w.next(), w.next()) {
        (Some("screen"), Some("makepad"), Some("run"), package) => package,
        _ => None,
    }
}

/// The lane whose gates are landings: first in line for the global gate cap.
pub const LANDING_LANE: &str = "landgate";
/// The post-land tests bucket's lane: its shards' test leases (`bucket::tests`).
pub const TESTS_LANE: &str = "landq-tests";

/// A lease of `lane` and `kind` is a shard of the tests bucket ([`TESTS_LANE`]): the master and
/// its box (which lets the shards of one run share their native tree) read it alike.
pub fn tests_shard(lane: &str, kind: Kind) -> bool {
    kind == Kind::Test && lane == TESTS_LANE
}
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
    pub exec_profile: Option<super::argv::ExecProfile>,
    pub lane: String,
    /// A warm lease uses this lane's native tree and target, but keeps its own accounting lane.
    pub warms: Option<String>,
    pub kind: Kind,
    /// GPU work a Mac takes as a timed lease ([`Self::normalize_kind`]): what it times is its
    /// GPU, so its box is quiet when its GPU is ([`gpu_noise`]), whatever CPU the Mac's own
    /// services take beside it
    pub gpu: bool,
    pub src: Src,
    /// Live jobs need a hub that accepts the detachable flag on lease-run.
    pub live: bool,
    pub cost: CostClass,
    pub cost_asked: bool,
    pub est_min: Option<f64>,
    pub cores: Option<f64>,
    pub ram_gb: Option<f64>,
    /// The least RAM (GB) an unasked need may be cut to on a box (`--ram-floor-gb`): a tests
    /// shard's largest unit's last peak, which it runs alone. No budget cut or resize takes its
    /// charge below it, and a box that could never give it that much never takes it.
    pub ram_floor_gb: Option<f64>,
    pub whole: bool,
    pub class: String,
    /// Full argv classification, when known; classes may omit timing-output flags.
    pub plain_test: Option<bool>,
    /// exactly this node (it waits for it; never another)
    pub node: Option<String>,
    /// `--series <name>`: why it names its box ([`Self::node`]): a series compared on that one box
    /// (AGENTS.md: a series compared across leases names its box). Its pinned wait is never asked
    /// to unpin ([`Sched::say_pinned`]); its queue line names the series.
    pub series: Option<String>,
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
    /// `--for <node>`: the `-C target-cpu` that box reports ([`super::Caps::cpu_target`]); its
    /// builds take it in place of `native`, in a target dir of their own.
    pub cpu_target: Option<String>,
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

// A key is never removed while a persisted lease may carry it.
pub(crate) const REQ_KEYS: &[&str] = &[
    "exec_profile", "lane", "warms", "series", "plain_test", "kind", "gpu",
    "src", "live", "cost", "cost_asked", "est_min", "cores",
    "ram_gb", "ram_floor_gb", "whole", "class", "node", "needs",
    "prio", "priority", "rev", "repo_dir", "jit", "loader",
    "cpu_target", "hint", "holder", "master", "agent", "since_ms",
];

impl Req {
    pub fn tree_lane(&self) -> &str { self.warms.as_deref().unwrap_or(&self.lane) }

    pub fn on_node(&self) -> bool { self.kind != Kind::Inspect || self.src == Src::Overlay }

    /// A box's own hub build ([`HUB_BUILD`]): the room its rustc takes there, the master's own
    /// lease that the self-update holds from its turn until the hub answers that it restarts
    /// (`orch::roll`). Only the box's room holds it, not what keeps lane work off a box (dev off,
    /// a user there, a hot or bench box): the hub updates on every box. Nothing of it is on the
    /// node: no setup, no beat, no cleanup ([`Self::set_up_on_node`]).
    /// A deployment's prebuild takes one too ([`Self::deploy_build`]).
    pub fn hub_build(&self) -> bool { self.master && self.class == HUB_BUILD }

    /// A box's deployment build (`sync` apply's prebuild: the JIT, the WM and the apps it starts):
    /// a room like its hub build's ([`Self::hub_build`], that class's size and queueing), held by
    /// the apply that asks for it until the node answers ([`DEPLOY_BUILD`]).
    pub fn deploy_build(&self) -> bool { self.hub_build() && self.lane.starts_with(DEPLOY_BUILD) }

    /// The name a box's CPU split gives a hub build's processes ([`crate::top::cpu_split`]): its
    /// class's, the self-update's or the deployment's. None: a lease's own, by its id.
    pub(crate) fn hub_work(&self) -> Option<&'static str> {
        self.hub_build().then(|| if self.deploy_build() { DEPLOY_BUILD } else { HUB_BUILD })
    }

    /// Set up on its node (`lease-start`, the master's beats, `lease-end`): what runs there under it.
    pub fn set_up_on_node(&self) -> bool { self.on_node() && !self.hub_build() }

    /// Sweeps use build/test kinds under the CI lane; explicit CI leases may name another lane.
    pub(crate) fn ci(&self) -> bool { self.kind == Kind::Ci || self.lane == CI_LANE }

    /// A leg devhub runs itself ([`OWN_LEG_AGENTS`], a bucket's gate, CI): nobody reads its stall,
    /// edits its code or resumes its parked JIT.
    pub(crate) fn unattended(&self) -> bool { self.ci() || self.agent.as_deref().is_some_and(|a| OWN_LEG_AGENTS.contains(&a) || a == BUCKET_AGENT) }

    /// A test run (`devhub test`, class `test:<package>[:<target>]`): nobody drives it either, and
    /// what hangs in it hangs until it ends ([`Sched::end_hung_legs`]).
    fn test_run(&self) -> bool { self.class.starts_with("test:") }

    fn top(&self) -> bool { self.lane == TOP_LANE }

    /// A land queue's gate: every lane's landing waits behind it.
    fn landing_gate(&self) -> bool {
        self.kind == Kind::Gate && (self.lane == LANDING_LANE || self.lane.starts_with("landq"))
    }

    /// A shard of the post-land tests bucket ([`TESTS_LANE`]): background work that gates nothing,
    /// but owed its box as a landing gate is. Its four shards headed the queue 18 min while newer
    /// lane work took the cores every box freed (11-07).
    fn tests_shard(&self) -> bool {
        tests_shard(&self.lane, self.kind)
    }

    /// Two shards of one tests bucket run (its tip): one unit of work, dealt over the pool boxes,
    /// that reads one tree at one pin.
    fn same_run(&self, other: &Req) -> bool {
        self.tests_shard() && other.tests_shard() && self.rev == other.rev
    }

    /// The RAM budget (GB) of a node for this request: 85% of its RAM, and no more than its
    /// residents leave less [`RAM_MARGIN_SHARE`]; on a hot box the top lane's leases get all of it
    /// and every other lease all but [`TOP_RAM_RESERVE_GB`] ([`keeps_top_reserve`]).
    pub(super) fn ram_budget(&self, c: &Caps) -> f64 {
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

    /// An ask devhub itself made (its master's, or an agent `devhub-...`: warm legs, prebuilds,
    /// buckets, CI): a box it names is devhub's choice, and devhub moves it itself where it can
    /// (orch/warm.rs: a spare's leg goes to a free box it may warm). Nobody reads an "unpin it".
    fn devhubs_own(&self) -> bool {
        self.master || self.agent.as_deref().is_some_and(|a| a.starts_with("devhub-"))
    }

    /// A box's own prebuild (its dev JIT for the newest landed tip): upkeep of the box, run in a
    /// gap of its timed queue ([`Sched::gap_wait`]). Background work: nobody waits on it, a lease
    /// that needs that JIT sooner builds it. 354 asks a day waited 630 min, 18 min at worst behind
    /// a tests shard's charged RAM, and lanes built 10 JIT-min themselves meanwhile (11-09). Asked
    /// interactive it read as a stuck head, and contended with live calls at their CPU weight.
    pub(crate) fn upkeep(&self) -> bool {
        self.kind == Kind::Build && self.node.as_deref().is_some_and(|n| self.lane.strip_prefix(super::PREBUILD_LANE_PREFIX) == Some(n))
    }

    /// devhub's own timed check (verify's and jitlegs' legs, jitlegs' warm leg among them): it
    /// measures the fleet, not a lane's change, so a lane's timed work goes first on a box
    /// ([`Sched::gap_wait`]).
    fn own_timed(&self) -> bool {
        self.kind == Kind::Timed && self.agent.as_deref().is_some_and(|a| a == "devhub-verify" || a == "devhub-jitlegs")
    }

    /// A JIT warm-up: a box's prebuild ([`Self::upkeep`]) or a lane's warm leg.
    fn warm_up(&self) -> bool {
        self.upkeep() || self.lane_warm()
    }

    /// A lane's warm leg: a spare box's warmth for the lane's next lease (`orch::warm`), work
    /// nobody waits for.
    fn lane_warm(&self) -> bool {
        self.lane.starts_with(super::WARM_LANE_PREFIX)
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

    /// Work a WM restart on its box waits for, as the box's hub says it (`exec::on_the_wm`): a
    /// screen or CI lease, one given the WM session (gui, display). A headless lease, a landing's
    /// gate or tests among them, runs on through the restart.
    fn cut_by_wm_restart(&self) -> bool {
        matches!(self.kind, Kind::Screen | Kind::Ci) || self.uses_wm()
    }

    fn allocation(&self, warm: bool) -> Cost {
        let mut cost = Cost::default_for(self.kind, warm);
        if let Some(cores) = self.cores { cost.cores = cores; }
        if let Some(ram_gb) = self.ram_gb { cost.ram_gb = ram_gb; }
        // (it builds in the deployment's target, not a lease tree)
        if self.hub_build() { cost.disk_gb = 0.0; }
        cost
    }

    pub fn run_options(&mut self, src: Src, cost: Option<CostClass>, est_min: Option<f64>, class: &str, argv: &[String]) {
        self.exec_profile = super::argv::ExecProfile::of(argv);
        self.src = if self.kind == Kind::Ci { Src::Tree } else { src };
        self.cost = cost.unwrap_or(self.cost);
        self.cost_asked |= cost.is_some();
        self.est_min = est_min;
        self.plain_test = (!argv.is_empty()).then(|| match super::loaders::cargo_run_args(argv) {
            Some(args) => plain_test(std::iter::once("makepad").chain(args.iter().map(String::as_str))),
            None => plain_test(super::loaders::command_of(argv).iter().map(String::as_str)),
        });
        self.class = if class.is_empty() { super::run_class(&self.lane, self.kind, argv) } else { class.to_string() };
    }

    pub(super) fn class(&self) -> &str { if self.class.is_empty() { self.kind.as_str() } else { &self.class } }

    /// What the run declared it takes (`--est-min`).
    fn declared_ms(&self) -> Option<u64> {
        self.est_min.map(|m| (m * 60_000.0) as u64)
    }

    pub fn normalize_kind(&mut self) {
        // GPU work on a Mac is Metal on the box to itself (its timings, its smoke runs): a timed
        // lease, the one form the Macs take it in (they offer no gpu slot)
        if self.kind == Kind::Gpu && self.requires_macos() {
            self.kind = Kind::Timed;
            self.gpu = true;
            if !self.cost_asked {
                self.cost = CostClass::default_for(Kind::Timed);
            }
        }
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

    fn native(&self, role: Role) -> bool {
        self.uses_tree() && super::native_lane(self.tree_lane(), self.kind, role)
    }

    /// The hub features this request's fields need: work waits only for facilities it uses.
    pub(crate) fn required(&self, role: Role) -> Vec<&'static str> {
        let mut features = vec![match self.src {
            _ if self.tests_shard() => "tell",
            _ if self.warms.is_some() => "warm-trees",
            Src::Overlay => "overlay-edits",
            _ if self.live => "detach",
            Src::Tree if self.native(role) => "native-trees",
            Src::Tree => "ship-done",
        }];
        if self.rev_names_unpaired() { features.push("nested"); }
        if self.cpu_target.is_some() { features.push("for-cpu"); }
        if self.gpu { features.push("gpu-quiet"); }
        if self.loader.as_deref().is_some_and(|id| !id.is_empty()) { features.push("jit-ask"); }
        features
    }

    /// Whether the rev names a nested repository other than the paired one: hubs without
    /// `nested` would leave it out of the tree.
    fn rev_names_unpaired(&self) -> bool {
        self.rev.as_deref().and_then(Pin::of_rev).is_some_and(|p| p.nested.keys().any(|n| n != crate::state::PAIRED))
    }

    fn conflicts_on(&self, other: &Req, c: &Caps) -> bool {
        let role = c.role();
        // a hub build writes no lease tree: beside it only timed work does not run (its rustc
        // would be in the timings)
        if self.hub_build() || other.hub_build() { return self.kind == Kind::Timed || other.kind == Kind::Timed; }
        let native = self.uses_tree() && other.uses_tree() && (self.native(role) || other.native(role));
        let kinds = if native { self.kind == Kind::Timed || other.kind == Kind::Timed } else { self.kind.conflicts(self.src, other.kind, other.src) };
        kinds || (self.whole && other.kind.writes()) || (other.whole && self.kind.writes()) || self.tree_conflicts(other, c) || (self.src == Src::Overlay && other.src == Src::Overlay && self.lane == other.lane && !(self.kind == Kind::Ci && other.kind == Kind::Ci) && self.rev.as_deref().and_then(Pin::of_rev) != other.rev.as_deref().and_then(Pin::of_rev))
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
    /// The shards of one tests bucket run share it: they read it at one pin, and its last holder's
    /// release scrubs it (lease/tree.rs). A run's shards waited on each other's box at "one lease at
    /// a time in its tree" (11-07).
    fn tree_conflicts(&self, other: &Req, c: &Caps) -> bool {
        self.same_tree(other, c.role()) && !self.same_run(other)
    }

    pub fn new(lane: &str, kind: Kind) -> Req {
        Req { exec_profile: None, lane: makepad_devhub_conf::lane_name(lane), warms: None, kind, gpu: false, src: Src::Tree, live: false, cost: CostClass::default_for(kind), cost_asked: false, est_min: None, cores: None, ram_gb: None, ram_floor_gb: None, whole: false, class: String::new(), plain_test: None, node: None, series: None, needs: Vec::new(), since_ms: None, prio: DEFAULT_PRIO, priority: Priority::Background, rev: None, repo_dir: None, jit: None, loader: None, cpu_target: None, hint: Vec::new(), holder: None, master: false, agent: None }
    }

    /// A series names why the lease names its box: it takes `--node`, and a name (a-z 0-9 - _).
    pub fn check_series(&self) -> Result<(), String> {
        match (&self.series, &self.node) {
            (Some(name), _) if !makepad_devhub_conf::valid_name(name) => Err(format!("--series `{name}`: a name (a-z 0-9 - _)")),
            (Some(name), None) => Err(format!("--series {name} says why the lease names its box: it takes --node")),
            _ => Ok(()),
        }
    }

    pub fn to_json(&self) -> String {
        let opt = |o: &Option<String>| o.as_deref().map_or("null".to_string(), json_str);
        let needs: Vec<String> = self.needs.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        let exec_profile = self.exec_profile.as_ref().map_or("null".into(), |p| p.to_json());
        let plain_test = self.plain_test.map_or(String::new(), |v| format!(",\"plain_test\":{v}"));
        let warms = self.warms.as_deref().map_or(String::new(), |lane| format!(",\"warms\":{}", json_str(lane)));
        let series = self.series.as_deref().map_or(String::new(), |name| format!(",\"series\":{}", json_str(name)));
        let gpu = if self.gpu { ",\"gpu\":true" } else { "" };
        format!(
            "{{\"exec_profile\":{exec_profile},\"lane\":{}{warms}{series}{plain_test},\"kind\":\"{}\"{gpu},\"src\":\"{}\",\"live\":{},\"cost\":\"{}\",\"cost_asked\":{},\"est_min\":{},\"cores\":{},\"ram_gb\":{},\"ram_floor_gb\":{},\"whole\":{},\"class\":{},\"node\":{},\"needs\":{{{}}},\"prio\":{},\"priority\":\"{}\",\"rev\":{},\"repo_dir\":{},\"jit\":{},\"loader\":{},\"cpu_target\":{},\"hint\":[{}],\"holder\":{},\"master\":{},\"agent\":{},\"since_ms\":{}}}",
            json_str(&self.lane),
            self.kind.as_str(),
            self.src.as_str(),
            self.live,
            self.cost.as_str(),
            self.cost_asked,
            self.est_min.map_or("null".to_string(), |m| m.to_string()),
            self.cores.map_or("null".to_string(), |n| n.to_string()),
            self.ram_gb.map_or("null".to_string(), |n| n.to_string()),
            self.ram_floor_gb.map_or("null".to_string(), |n| n.to_string()),
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
            opt(&self.cpu_target),
            self.hint.iter().map(|id| json_str(id)).collect::<Vec<_>>().join(","),
            self.holder.map_or("null".to_string(), |p| p.to_string()),
            self.master,
            opt(&self.agent),
            self.since_ms.map_or("null".to_string(), |t| t.to_string())
        )
    }

    pub fn parse(text: &str) -> Result<Req, String> {
        let v = makepad_strict_json::parse(text.as_bytes()).map_err(|e| format!("bad lease request: {e:?}"))?;
        let makepad_strict_json::Value::Obj(fields) = &v else { return Err("a lease needs a lane (a-z 0-9 - _)".into()) };
        for (k, _) in fields {
            if !REQ_KEYS.contains(&k.as_str()) {
                return Err(format!("this master predates the lease field `{k}`: it reads only the fields it knows (it restarts for its update within minutes; ask again then)"));
            }
        }
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_string);
        let lane = s("lane").map(|l| makepad_devhub_conf::lane_name(&l)).filter(|l| makepad_devhub_conf::valid_name(l)).ok_or("a lease needs a lane (a-z 0-9 - _)")?;
        let kind = s("kind").and_then(|k| Kind::parse(&k)).ok_or("unknown lease kind")?;
        let mut r = Req::new(&lane, kind);
        r.exec_profile = v.get("exec_profile").filter(|v| !v.is_null()).map(|v| super::argv::ExecProfile::parse(v).ok_or("invalid exec profile")).transpose()?;
        r.warms = v.get("warms").filter(|v| !v.is_null()).map(|v| v.as_str().map(makepad_devhub_conf::lane_name).filter(|l| makepad_devhub_conf::valid_name(l)).ok_or("warms needs a lane (a-z 0-9 - _)")).transpose()?;
        r.src = s("src").map(|s| Src::parse(&s).ok_or("src is tree or overlay")).transpose()?.unwrap_or(Src::Tree);
        r.live = v.get("live").and_then(|v| v.as_bool()).unwrap_or(false);
        r.cost = s("cost").map(|s| CostClass::parse(&s).ok_or("cost is small, medium or large")).transpose()?.unwrap_or(r.cost);
        r.cost_asked = v.get("cost_asked").and_then(|v| v.as_bool()).unwrap_or_else(|| s("cost").is_some());
        r.est_min = v.get("est_min").filter(|v| !v.is_null()).map(|v| super::num(v).filter(|m| m.is_finite() && *m >= 0.0).ok_or("est_min is a nonnegative number")).transpose()?;
        r.cores = v.get("cores").filter(|v| !v.is_null()).map(|v| super::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("cores is a positive number")).transpose()?;
        r.ram_gb = v.get("ram_gb").filter(|v| !v.is_null()).map(|v| super::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("ram_gb is a positive number")).transpose()?;
        r.ram_floor_gb = v.get("ram_floor_gb").filter(|v| !v.is_null()).map(|v| super::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("ram_floor_gb is a positive number")).transpose()?;
        r.whole = v.get("whole").and_then(|v| v.as_bool()).unwrap_or(false);
        r.class = s("class").unwrap_or_default();
        r.plain_test = v.get("plain_test").and_then(|v| v.as_bool());
        r.priority = s("priority").map(|s| Priority::parse(&s).ok_or("priority is interactive or background")).transpose()?.unwrap_or_default();
        r.node = s("node");
        r.series = s("series");
        r.check_series()?;
        r.prio = v.get("prio").and_then(super::num).map_or(DEFAULT_PRIO, |p| p as i32);
        r.rev = s("rev");
        r.repo_dir = s("repo_dir");
        r.jit = s("jit");
        r.loader = s("loader");
        r.cpu_target = s("cpu_target").map(|t| if super::valid_cpu_target(&t) { Ok(t) } else { Err(format!("cpu_target `{t}` is not a CPU name")) }).transpose()?;
        r.hint = v.get("hint").and_then(|v| v.as_arr()).unwrap_or(&[]).iter().filter_map(|v| v.as_str()).map(str::to_string).collect();
        r.agent = s("agent").filter(|a| makepad_devhub_conf::valid_name(a));
        r.holder = v.get("holder").and_then(super::num).map(|p| p as u32).filter(|p| *p > 1);
        r.master = v.get("master").and_then(|v| v.as_bool()).unwrap_or(false);
        r.gpu = v.get("gpu").and_then(|v| v.as_bool()).unwrap_or(false);
        r.since_ms = v.get("since_ms").and_then(super::num).map(|t| t as u64);
        if let Some(makepad_strict_json::Value::Obj(pairs)) = v.get("needs") {
            for (k, x) in pairs {
                r.needs.push((k.clone(), x.as_str().map(str::to_string).or_else(|| super::num(x).map(|n| n.to_string())).unwrap_or_default()));
            }
        }
        r.normalize_kind();
        if r.src == Src::Overlay && r.rev.as_deref().and_then(Pin::of_rev).is_none() {
            return Err("overlay leases require pinned revisions".into());
        }
        if kind == Kind::Inspect && r.src == Src::Tree && (r.node.is_none() || r.jit.is_none()) {
            // `--jit` is a run's JIT loader key, not the app: lanes took it for one (11-07)
            return Err("an inspect lease is taken by `makepad devhub inspect <node>/<pid|app> <cmd...>` (a grab too: `grab out=<file>` saves it here); `--jit` is a run's JIT loader key, not the app".into());
        }
        Ok(r)
    }
}

/// A pass/fail test invocation with no requested timing output or benchmark mode.
fn plain_test<'a>(mut words: impl Iterator<Item = &'a str>) -> bool {
    let command = words.next().unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("");
    matches!(command, "makepad" | "cargo") && words.next() == Some("test")
        && !words.any(|w| matches!(w, "--nocapture" | "--show-output" | "--bench"))
}

const TIMED_TEST_NOTE: &str = "a timed box is for a time the run measures: a test's pass or fail needs none (devhub test <lane> <pkg> runs it on any box now); keep it timed only if it prints a timing";

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
    /// What its run gets on its box, decided at its grant ([`Self::run_cores`]).
    run: f64,
    /// Whether the class had a measured RAM need when this lease was granted.
    ram_measured: bool,
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
    /// The absolute start its reservation last told it (none while it waits behind an overrun);
    /// kept after the grant, so the ledger says what it was told beside when it started
    pub told_ms: Option<u64>,
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
    /// how long its node saw its runs work under [`super::exec::SLOW_CPU_PCT`] of a core
    /// (`info`'s `slow_s`)
    slow_s: Option<u64>,
    /// what its runs wrote so far (`info`'s `out_bytes`), and when a beat last found it grown
    out_bytes: u64,
    out_grew_ms: Option<u64>,
    /// since when a granted gate lease runs nothing (`info`'s `running` empty): the last tick it
    /// ran something, else its grant; a screen run of an app: when someone last looked at it or
    /// drove it ([`Sched::app_touched`])
    idle_since: Option<u64>,
    /// How far past its box's cores the charges there stand with this live launch or call running
    /// ([`Sched::past_budget`]), as of the last tick: its lease line says so.
    past_budget: f64,
    /// since when it first waited for a busy box that holds its dev JIT while a box without it
    /// could take it: all such waiting together stays under one build ([`Sched::jit_box`])
    jit_wait_since: Option<u64>,
    /// A queued timed ask's catch-up ([`Sched::pin_updates`]): the deployment
    /// (a pin key, [`super::Pin::key`]) its box moves to before it starts, the box having fallen more than
    /// [`QUEUE_BEHIND_MS`] behind; the tips of that moment
    pub deploy_pin: Option<String>,
    /// The deployment (a pin key, [`super::Pin::key`]) of its box at its grant: what a timed run ran on,
    /// so two timings can be told apart or compared
    pub deployment: String,
    seq: u64,
    /// latency (ms): from the previous lease's release on its node to this grant, when it stood in
    /// the queue at that release; its setup (`lease-start`, the revisions shipped inside it); its teardown (`lease-end`)
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
    /// Its box builds its work tree's JIT as this lease's step now: its `jit` phase, as the box's
    /// last beat said it. (An overlay's phase is the box's store build, whose CPU the box measures
    /// as the lease's that started it: charged as measured, [`Sched::measured_cores_now`].)
    fn building_jit(&self) -> bool {
        self.req.src != Src::Overlay && self.node_phases.0.iter().rev().find(|s| s.phase == "jit").is_some_and(|s| s.end == 0)
    }

    fn class_note(&self) -> String {
        let mut note = if !self.guess_note.is_empty() {
            self.guess_note.clone()
        } else if matches!(self.cost_source, CostSource::Measured(_)) && self.cost_class != self.req.asked_cost() {
            format!("class {} {}: {}", self.req.class(), self.cost_source.label(), self.cost_class.as_str())
        } else { String::new() };
        if self.req.kind == Kind::Timed && self.req.plain_test.unwrap_or_else(|| {
            let mut words = self.req.class.split_whitespace();
            words.next() == Some("timed") && plain_test(words)
        }) {
            if !note.is_empty() { note.push_str("; "); }
            note.push_str(TIMED_TEST_NOTE);
        }
        note
    }

    fn take_class_note(&mut self) -> String {
        if self.class_notified { return String::new(); }
        let note = self.class_note();
        self.class_notified = !note.is_empty();
        if note.is_empty() { note } else { format!("; {note}") }
    }

    /// The cores its run gets on its box (its threads, its CPU weight), as its grant decided them
    /// ([`Sched::run_on`]): its ask, charged its class's measure.
    pub fn run_cores(&self) -> f64 {
        self.run
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
            "{{\"id\":{},\"deployment\":{},\"lane\":{},\"kind\":\"{}\",\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"class_note\":{},\"src\":\"{}\",\"class\":{},\"state\":\"{}\",\"node\":{},\"addr\":{},\"pos\":{},\"reason\":{},\"past_budget\":{},\"eta_ms\":{},\"eta_late\":{},\"eta_late_by\":{},\"eta_box\":{},\"prio\":{},\"priority\":\"{}\",\"wait_s\":{},\"how\":{},\"jit\":{},\"loader\":{},\"agent\":{},\"beat_s\":{},\"granted_s\":{},\"info\":{},\"start\":{}}}",
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
            self.past_budget.round(),
            self.eta_ms,
            self.eta_late,
            json_str(&self.eta_late_by),
            opt(&self.eta_box),
            self.req.prio,
            self.priority.as_str(),
            self.waited_ms(now) / 1000,
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

    /// How long it waited (its warn, its grant line, `wait_s`): from its place (`asked_ms`) to its
    /// grant or now. A leg devhub runs itself ([`runs_place`]) has its run's place: an owed leg the
    /// age its run became owed ([`owed_leg`]), after the run's earlier legs ran; allapps its run's
    /// start, before which it asked nothing while lane work waited (`orch::leg`). Its own wait is
    /// its time in this queue, carried over a master restart: two owed legs asked a minute earlier
    /// read "waiting 55m07s" and "1h03m" as starved behind their boxes' own work (11-05), and
    /// allapps asked 26 s before its grant read "waiting 17m51s: 1st in queue" (11-08).
    pub(crate) fn waited_ms(&self, now: u64) -> u64 {
        if !runs_place(&self.req) { return self.granted_ms.unwrap_or(now).saturating_sub(self.asked_ms); }
        let done: u64 = self.phases.0.iter().filter(|s| s.phase == "queued" && s.end != 0).map(|s| s.end.saturating_sub(s.start)).sum();
        done + if self.state == State::Queued { now.saturating_sub(self.queue_sample_ms) } else { 0 }
    }

    /// The warn of a leg with its run's place ([`runs_place`]) says its run's wait too.
    fn owed_age(&self, now: u64) -> String {
        let age = dur(now.saturating_sub(self.asked_ms));
        if owed_leg(self) { format!(" (its run owed a box for {age})") } else if runs_place(&self.req) { format!(" (its run waited {age} in all, standing back while lane work waited)") } else { String::new() }
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
            "{{\"id\":{},\"lane\":{},\"kind\":\"{}\",\"node\":{},\"asked_ms\":{},\"granted_ms\":{},\"told_ms\":{},\"ended_ms\":{},\"how\":{},\"wait_ms\":{},\"setup_ms\":{},\"ship_ms\":{},\"teardown_ms\":{},\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"reported_ms\":{},\"alloc\":{{\"cores\":{},\"ram_gb\":{},\"gpus\":{}}}, {},\"req\":{}}}",
            json_str(&self.id),
            json_str(&self.req.lane),
            self.req.kind.as_str(),
            self.node.as_deref().map_or("null".to_string(), json_str),
            self.asked_ms,
            ms(self.granted_ms),
            ms(self.told_ms),
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
    /// A line for the master's log: a decision no record carries, to read back after a wait
    Log(String),
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
    /// when its caps last answered
    heard_ms: Option<u64>,
    /// why its caps did not answer, at the last miss
    miss: String,
    /// since when its host has been off the network, every miss since ([`HOST_GONE_MS`])
    gone_ms: Option<u64>,
    /// its hub holds more than [`HUB_HEAVY_SHARE`] of its RAM (said once per crossing)
    hub_heavy: bool,
    /// since when its tasks stall on memory past [`super::caps::MEM_STALL_PCT`], and the deepest
    /// stall since (said when it starts and when it eases)
    mem_stall: Option<(u64, f64)>,
    /// since when every look found it no place for a timed run for what runs outside its leases
    /// ([`timed_noise`]), and for a GPU timing ([`gpu_noise`]): a timed ask these keep from every
    /// box it may take is told what would let it start ([`Sched::say_never_quiet`])
    noisy: (Option<u64>, Option<u64>),
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
    /// How long it has gone unheard and why, while [`Self::caps`] is what it said last and not
    /// what it says now
    pub unheard: Option<(u64, String)>,
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
    /// Timed work that keeps the background pre-warm off it ([`Sched::timed_near`])
    pub timed: Option<String>,
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

    /// `x86_64 linux (x86-64-v3) · 16 cores · load 2.1 · 40/60 GB · disk 105 GB free (floor 25) · GPU-B`.
    /// A box nobody hears has no numbers: what it said last is not what it says now, and every box
    /// read as healthy for 15 min of an outage with the far end of the line saying `down`, 11-09.
    pub fn caps_line(&self) -> String {
        let unheard = self.unheard.as_ref().map(|(ms, why)| format!("not heard for {} ({why})", dur(*ms)));
        if !self.up {
            return unheard.map_or("down".into(), |u| format!("down: {u}"));
        }
        let Some(c) = &self.caps else { return "no caps yet".into() };
        let cpu = c.cpu_pct.map_or(String::new(), |p| format!("cpu {p:.0}% \u{b7} "));
        // the CPU a build `--for` this box targets
        let target = if c.cpu_target.is_empty() { String::new() } else { format!(" ({})", c.cpu_target) };
        let mut s = format!(
            "{} {}{target} \u{b7} {} cores \u{b7} {cpu}load {:.1} \u{b7} {:.0}/{:.0} GB free ({:.1} resident) \u{b7} disk {:.0} GB free (floor {:.0})",
            c.arch, c.os, c.cores, c.load1, c.ram_avail_gb, c.ram_gb, c.ram_resident_gb, c.disk_free_gb, c.disk_floor_gb
        );
        // what used the box's CPU outside its leases
        if !c.busy_outside.is_empty() {
            s.push_str(&format!(" \u{b7} busy outside its leases: {}", crate::top::outside_said(&c.busy_outside)));
        }
        // the clock a timed number rests on (x86 Linux): a box off the TSC takes no timed lease,
        // and its role and slots alone did not say so (a node sat as a bench box on hpet, 11-06)
        if !c.clocksource.is_empty() {
            let slow = super::caps::slow_clock(&c.clocksource).is_some();
            s.push_str(&format!(" \u{b7} clock {}{}", c.clocksource, if slow { ": no timed leases (TSC unstable since boot; a reboot restores it)" } else { "" }));
        }
        // its memory stall once it shows (PSI, Linux): past [`super::caps::MEM_STALL_PCT`] it starts no new lease
        if c.mem_stall >= 1.0 {
            s.push_str(&format!(" \u{b7} {}", super::caps::memory_stall(c.mem_stall).unwrap_or_else(|| format!("memory stall {:.0}% (PSI full avg10)", c.mem_stall))));
        }
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
        if c.features.is_empty() {
            s.push_str(" (no features: an older hub)");
        }
        if c.hub_rss_mb > 0 {
            s.push_str(&format!(" {:.1} GB (heap {:.1})", c.hub_rss_mb as f64 / 1024.0, c.hub_heap_mb as f64 / 1024.0));
        }
        s.push_str(&format!(" deploy {}", if c.deploy.is_empty() { "-" } else { &c.deploy[..c.deploy.len().min(10)] }));
        if !c.wm_dark.is_empty() {
            s.push_str(&format!(" \u{b7} no display ({}): no screen or gui lease", c.wm_dark));
        }
        if let Some(store) = &c.jit_store {
            s.push_str(&format!(" \u{b7} JIT ids [{}], {} hits, {} misses, {} builds today (UTC)", store.ids.join(", "), store.hits, store.misses, store.builds_today));
        }
        if let Some(d) = c.desk_link.as_ref().filter(|d| d.on) {
            s.push_str(&if d.port > 0 { format!(" \u{b7} desk link :{}", d.port) } else { " \u{b7} desk link waiting".to_string() });
        }
        if !c.wm_drain.is_empty() {
            s.push_str(&format!(" \u{b7} WM restart drains it (waits on {})", c.wm_drain));
        }
        // (up, its last answer not long past: counting as down takes [`DOWN_AFTER_MS`])
        if let Some(u) = unheard {
            s.push_str(&format!(" \u{b7} {u}"));
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
            "{{\"node\":{},\"addr\":{},\"up\":{},\"dev\":{},\"dev_set\":{},\"role\":\"{}\",\"leases\":{},\"waiting\":{},\"draining\":{},\"user\":{},\"hold\":{},\"updating\":{},\"update_waits\":{},\"update_pin\":{},\"timed\":{},\"state\":{},\"latency\":{},\"caps_line\":{},\"caps\":{}}}",
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
            self.timed.as_deref().map_or("null".to_string(), json_str),
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
            // (the master's `caps_line` says it)
            unheard: None,
            leases: n("leases"),
            waiting: n("waiting"),
            draining: b("draining"),
            hold: s("hold"),
            updating: s("updating"),
            update_waits: s("update_waits"),
            update_pin: v.get("update_pin").and_then(|p| Some((p.get("lease")?.as_str()?.to_string(), p.get("at")?.as_str()?.to_string()))),
            timed: s("timed"),
            latency: s("latency").unwrap_or_default(),
        })
    }
}

pub(super) enum Place {
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

/// When a box's head ([`Sched::age_heads`]) was told it starts, and how often that start passed
/// while it still waited.
#[derive(Clone, Copy, Debug)]
struct Promise {
    at: u64,
    misses: u32,
}

/// The class of a box's own hub build ([`super::exec::build_hub`], [`Req::hub_build`]): measured
/// like a build lease's, from its hub's restart answer ([`super::master::LeaseMaster::ledger_hub_build`]).
pub const HUB_BUILD: &str = "hub-build";
/// The lane prefix of a box's deployment-build room (`deploy-build-<box>`, [`Req::deploy_build`]),
/// and the name its box's CPU split gives that build's processes.
pub const DEPLOY_BUILD: &str = "deploy-build";
/// A lease's `--jit` copied to its box at its grant: its wall per box, in the ledger.
pub const JIT_COPY: &str = "jit-copy";
/// A copy's wall before one is measured.
const JIT_COPY_GUESS_MS: u64 = 10_000;
/// The fewest jobs a hub build starts on when its box has fewer cores free than it measured.
const HUB_BUILD_MIN_CORES: f64 = 4.0;

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
/// `at` (both pin keys, [`super::Pin::key`]), so a move there would be none or one back.
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

/// Lease charges and the work outside them, for one admission check.
struct InUse {
    cost: Cost,
    ids: Vec<String>,
    outside: f64,
}

/// Room kept for calls and the existing build exemptions, supplied by the caller.
struct Room {
    cores: f64,
    ram_gb: f64,
    cores_bump: bool,
    hub_ram: bool,
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
    /// lane -> when its last timed lease that ran ended: lanes take turns at timed work
    /// ([`Self::exclusive_place`])
    timed_turn: HashMap<String, u64>,
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
    /// the holds as this round began: who held each box at the previous look
    held_before: Holds,
    /// each box's first unserved prebuild ask, by when it was asked: a re-plan's ask keeps that
    /// age and place until a prebuild runs there ([`Sched::prebuild_owed`])
    prebuild_since: HashMap<String, u64>,
    /// each queued head's start as it was told, by lease ([`Self::age_heads`])
    promised: HashMap<String, Promise>,
    /// Ended leases retain their conflicts and reservations until cleanup answers: box, ask,
    /// charge, and when it ended ([`Sched::cleanup_end`]).
    restoring: HashMap<String, (String, Req, Cost, u64)>,
    /// dropped queued asks by (lane, kind, box): the master's own its restart dropped, and a lane's
    /// own its holder dropped ([`Self::keep_dropped_ask`]); their place (`asked_ms`) and when.
    /// Asked again, the same work keeps that place ([`Self::request`])
    dropped_asks: HashMap<(String, Kind, Option<String>), (u64, u64)>,
    /// what those asks had waited on, by (lane, `asked_ms`): the ask again goes on from it
    dropped_waits: HashMap<(String, u64), super::prof::Phases>,
    cleanup_unknown: HashMap<String, String>,
    walls: HashMap<(String, String), VecDeque<u64>>,
    runs: HashMap<(String, String), VecDeque<u64>>,
    node_walls: HashMap<(String, String, String), VecDeque<u64>>,
    peaks: HashMap<(String, String), VecDeque<f64>>,
    /// The peaks of each OS family's boxes (`Caps::os`, class, kind): what a box of that family
    /// weighs ([`Self::raw_peak`]).
    family_peaks: HashMap<(String, String, String), VecDeque<f64>>,
    /// The peaks of each lane's runs on each OS family (family, lane, class, kind): what a live
    /// session of that lane weighs ([`Self::raw_peak`]).
    lane_peaks: HashMap<(String, String, String, String), VecDeque<f64>>,
    cpu: HashMap<(String, String), VecDeque<f64>>,
    /// Last nine live calls' peak cores per (class, kind, launch) ([`Self::record_call`]).
    calls: HashMap<(String, String, bool), VecDeque<f64>>,
    /// Interactive demand without queued holds: a timed hold cannot hide its own competition.
    interactive_boxes: BTreeMap<String, Vec<String>>,
    /// Requests this tick denies: they keep no box from other work ([`Self::timed_queued_for`]).
    denying: HashSet<String>,
    /// The manager marks lanes whose user is waiting.
    pub interactive_lanes: HashSet<String>,
    /// The lanes the user put first (`devhub lane first`): their queued leases are interactive,
    /// head the queue after the boxes' hub builds, and take a box another lane's lease holds.
    pub first_lanes: HashSet<String>,
    /// The leases held by live jobs, as the live subsystem knows them: a lease granted before
    /// its request carried `live` (or restored without it) still parks for timed work.
    pub live_jobs: HashSet<String>,
    /// Restored `starting` leases the master asks their node to start again (`lease-start` is
    /// idempotent there): (id, node).
    pub resume: Vec<(String, String)>,
    /// Self-update progress for the node view; never a placement blocker.
    pub updating: HashMap<String, String>,
    /// Nodes whose hub lacks `handover` and has its update ready: it restarts
    /// only between runs, so it takes no new lease until it has (once per box).
    pub update_drain: HashSet<String>,
    /// Updates that a running timed lease made wait on a node, by what waits (its deployment's
    /// apply, its hub's restart). The next timed lease starts on what the box has all the same,
    /// unless the box catches up ([`Sched::pin_updates`]): back to back, a bench box's timed queue
    /// kept a broken hub (11-03). Their askers set and clear them
    /// ([`super::master::Master::update_waits`]).
    pub update_waits: HashMap<String, BTreeMap<&'static str, String>>,
    /// The deployment the updates move to (work's and commercial main's tips,
    /// a pin key), as the master's roll last read it: where a box that fell behind
    /// catches up to ([`Self::pin_updates`])
    pub tips: String,
    /// The recent landings, oldest first: the deployment each left and when, for how far behind
    /// a box is ([`Self::behind_ms`])
    pub landed: Vec<(String, u64)>,
    /// Live sessions between calls ([`Sched::charged`]): their app and JIT sit idle, so their box
    /// is charged their resident share; a call takes the full charge back ([`Sched::live_room`]).
    pub live_idle: HashSet<String>,
    /// Live sessions one of whose calls (or edits) ended: what they held at most has a call's
    /// peak in it, and their next call weighs that ([`Sched::call_ram`]).
    pub live_called: HashSet<String>,
    /// Idle live sessions whose call waits for room on their box ([`Sched::live_room`]): the box
    /// keeps that room from every other ask until the call runs ([`Sched::call_room`]).
    pub live_waiting: HashSet<String>,
    /// When each live session's current call began, admitted after an idle spell
    /// ([`Sched::live_call_began`]); one never idle yet is in the run it started with.
    live_call_ms: HashMap<String, u64>,
    /// Live sessions said to run a call past [`LIVE_OVERRUN_TIMES`] their measure
    /// ([`Sched::say_live_overruns`]): that call's start, and what their owner is told.
    live_overran: HashMap<String, (u64, String)>,
    /// Interactive live calls that started with the cores free on their box, fewer than their
    /// full charge ([`Sched::call_start_cores`]): what each is charged while its call runs.
    live_call_charge: HashMap<String, f64>,
    /// Idle live sessions asked to park for the RAM a queued ask waits for
    /// ([`Sched::park_idle_live_for_ram`]): until they end, no other session parks for that RAM.
    parking_ram: HashSet<String>,
    /// Running leases already said to be past twice their measured run ([`Sched::say_stalled`]), and when.
    stalled: HashMap<String, u64>,
    /// Of those, the legs devhub runs said past their measure (not idle, not held to a guess), and
    /// what they ran against: [`Sched::end_hung_legs`] ends them once captured.
    overran: HashMap<String, String>,
    /// What a stalled lease's capture found ([`Sched::captured`]), for the line it may end with.
    captures: HashMap<String, String>,
    /// Since when each box has been near idle ([`BOX_IDLE_LOAD`]).
    box_idle_since: HashMap<String, u64>,
    /// What the master's own network check says keeps it from its hubs ([`Self::own_network`])
    own_fault: Option<String>,
    /// The last time the master heard a box, while it hears none and the fault is its own
    /// ([`Self::deaf`]): no box's leases end as lost meanwhile, and no run is judged on what it
    /// could not hear
    deaf_since: Option<u64>,
    /// Running leases already said to hold a near idle box someone waits on ([`Sched::say_idle_holders`]).
    idle_said: HashSet<String>,
    /// queued leases whose pinned wait was said ([`Self::say_pinned`])
    pin_said: HashSet<String>,
    /// queued timed leases told their boxes are never quiet ([`Self::say_never_quiet`])
    quiet_said: HashSet<String>,
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
/// A box other than a bench box, not a user's, that may take a timed lease while the bench boxes
/// that meet it are taken by timed work ([`Sched::spare_wait`]): a Mac for work that requires
/// macOS, and a pool box whose lease.conf says `timed = spare`. A pool box made a bench box took
/// 16 cores from a pool whose top blocker was cores, for timed demand of some 14 h a day (11-10):
/// a spare is a pool box between timed runs. devhub's own timed checks ([`Req::own_timed`]) are no
/// lane's wait and a series on its box: they keep to the bench boxes. jitlegs' warm leg was the
/// first a spare took, and drained it 10 min for its run while lane work waited (L777258, 11-10).
fn spare_timed(c: &Caps, l: &Lease) -> bool {
    !c.role().users() && (l.req.requires_macos() && c.os.eq_ignore_ascii_case("macos") || c.timed_spare && c.role() == Role::Pool && !l.req.own_timed())
}

/// Why a box no lease of ours runs on is no place for a timed run now, by its last look ([`Caps`]):
/// work outside its leases at [`super::master::NOISY_CPU_PCT`] or more (a desktop left running:
/// WindowServer, audio, a music and a chat app at 1.4 cores under a timed gate whose single runs
/// took 56-280 s against 7.6 s, 11-10). Its load is no measure: it trails the leases that just
/// ended by a minute, and a Mac's background-QoS threads queued on its efficiency cores count in
/// it (load 54 at 23% of the CPU under its hub's build, which `hub_building` holds timed work for).
fn timed_noise(c: &Caps) -> Option<String> {
    let leases: f64 = c.busy_leases.iter().map(|(_, cores)| cores).sum();
    let outside = (c.cpu_pct? / 100.0 * c.cores as f64 - leases) * 100.0;
    let busiest = if c.busy_outside.is_empty() { String::new() } else { format!(": {}", crate::top::outside_said(&c.busy_outside)) };
    (outside >= super::master::NOISY_CPU_PCT).then(|| format!("{outside:.0}% of a core runs outside its leases{busiest}"))
}

/// Why a box no lease of ours runs on is no place for a GPU timing ([`Req::gpu`]) now, by its
/// last look: its GPU busy [`super::master::NOISY_GPU_PCT`] or more outside every lease
/// ([`Caps::mac_gpu`]), and its busiest outside users. The CPU they take is no measure of it: a Mac's
/// own services keep half a core to a core busy on it at all times (11-10).
fn gpu_noise(c: &Caps) -> Option<String> {
    let (pct, on) = c.mac_gpu.as_ref()?;
    let on = if on.is_empty() { String::new() } else { format!(": what uses it, {}", crate::top::gpu_said(on)) };
    (*pct >= super::master::NOISY_GPU_PCT).then(|| format!("its GPU is {pct:.0}% busy outside its leases{on}"))
}

fn hot_refuses(role: Role, l: &Lease) -> bool {
    role == Role::Hot && !l.req.hub_build() && !l.req.requires_macos() && l.cost_class != CostClass::Small && !(l.cost_class == CostClass::Medium && matches!(l.req.kind, Kind::Test | Kind::Screen))
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

/// The box's JIT store holds `id`.
fn holds_jit(c: &Caps, id: &str) -> bool {
    c.jit_store.as_ref().is_some_and(|s| s.ids.iter().any(|held| held == id))
}

/// Box `b`'s JITs run on box `a`: the same OS, arch and compiler (a JIT's key names its inputs
/// and compiler, not its target).
fn same_platform(a: &Caps, b: &Caps) -> bool {
    fn compiler(c: &Caps) -> Option<&str> {
        c.jit_store.as_ref().map(|s| s.compiler.as_str()).filter(|s| !s.is_empty())
    }
    a.os == b.os && a.arch == b.arch && compiler(a).is_some() && compiler(a) == compiler(b)
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
            timed_turn: HashMap::new(),
            sticky: HashMap::new(),
            excluded: HashMap::new(),
            native_wait: HashMap::new(),
            clock: 0,
            profile_at: None,
            profile_boxes: Vec::new(),
            profile_seq: 0,
            warm: HashSet::new(),
            holds: Holds::new(),
            held_before: Holds::new(),
            prebuild_since: HashMap::new(),
            promised: HashMap::new(),
            restoring: HashMap::new(),
            dropped_asks: HashMap::new(),
            dropped_waits: HashMap::new(),
            cleanup_unknown: HashMap::new(),
            walls: HashMap::new(),
            runs: HashMap::new(),
            node_walls: HashMap::new(),
            peaks: HashMap::new(),
            family_peaks: HashMap::new(),
            lane_peaks: HashMap::new(),
            cpu: HashMap::new(),
            calls: HashMap::new(),
            interactive_boxes: BTreeMap::new(),
            denying: HashSet::new(),
            interactive_lanes: HashSet::new(),
            first_lanes: HashSet::new(),
            live_jobs: HashSet::new(),
            resume: Vec::new(),
            updating: HashMap::new(),
            update_drain: HashSet::new(),
            update_waits: HashMap::new(),
            tips: String::new(),
            landed: Vec::new(),
            live_idle: HashSet::new(),
            live_called: HashSet::new(),
            live_waiting: HashSet::new(),
            live_call_ms: HashMap::new(),
            live_overran: HashMap::new(),
            live_call_charge: HashMap::new(),
            parking_ram: HashSet::new(),
            stalled: HashMap::new(),
            overran: HashMap::new(),
            captures: HashMap::new(),
            box_idle_since: HashMap::new(),
            own_fault: None,
            deaf_since: None,
            idle_said: HashSet::new(),
            pin_said: HashSet::new(),
            quiet_said: HashSet::new(),
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

    /// A live call's peak cores as its box metered it (its launch, the session's first call, apart):
    /// what its class's calls are charged ([`Self::call_cores`]).
    pub fn record_call(&mut self, class: &str, kind: &str, launch: bool, peak_cores: f64) {
        if !peak_cores.is_finite() || peak_cores < 0.0 { return; }
        let q = self.calls.entry((class.to_string(), kind.to_string(), launch)).or_default();
        q.push_back(peak_cores);
        while q.len() > 9 { q.pop_front(); }
    }

    /// The p90 of a live request's class's last metered calls' peak cores (`launch`: its
    /// launches'), and how many it is of ([`Self::record_call`]); its class without its test
    /// filter's while the filtered one has none.
    pub fn call_p90(&self, req: &Req, launch: bool) -> Option<(f64, usize)> {
        let kind = req.kind.as_str().to_string();
        let of = |class: &str| self.calls.get(&(class.to_string(), kind.clone(), launch)).filter(|q| !q.is_empty());
        let base = super::base_class(req.class());
        let q = of(req.class()).or_else(|| if base != req.class() { of(base) } else { None })?;
        let mut sorted: Vec<f64> = q.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        Some((sorted[(sorted.len() * 9).div_ceil(10) - 1], sorted.len()))
    }

    /// The cores a live session's call is charged: its class's metered peak p90 ([`Self::call_p90`]),
    /// its launch's until one of its calls ended, rounded up, at most its box's; what it asked
    /// (`--cores`), or its cost while its class has none metered. Not its class's mean over a
    /// session's life, mostly idle time: live sessions charged a few cores in a call compiled on
    /// most of their box, their boxes ran well over their cores beside a tests shard, and a save
    /// waited a minute for its call (11-08).
    fn call_cores(&self, l: &Lease) -> f64 {
        if l.req.cores.is_some() || !self.is_live_job(l) { return l.cost.cores; }
        let Some((p90, _)) = self.call_p90(&l.req, !self.live_called.contains(&l.id)) else { return l.cost.cores };
        let most = l.node.as_deref().and_then(|n| self.nodes.get(n)).and_then(|n| n.caps.as_ref()).map_or(f64::INFINITY, |c| c.cores_for(l.req.kind) as f64);
        p90.ceil().max(1.0).min(most)
    }

    /// A peak measured on a box of the OS family `os` (its ledger record says which), beside
    /// every box's ([`Self::record_resources`]).
    pub fn record_family_peak(&mut self, os: &str, class: &str, kind: &str, peak_ram_gb: Option<f64>) {
        if let Some(value) = peak_ram_gb.filter(|v| v.is_finite() && *v >= 0.0) {
            let q = self.family_peaks.entry((os.to_string(), class.to_string(), kind.to_string())).or_default();
            q.push_back(value);
            while q.len() > 9 { q.pop_front(); }
        }
    }

    /// A peak of a run of `lane` on a box of the OS family `os`, beside its family's
    /// ([`Self::record_family_peak`]).
    pub fn record_lane_peak(&mut self, os: &str, lane: &str, class: &str, kind: &str, peak_ram_gb: Option<f64>) {
        if lane.is_empty() { return; }
        if let Some(value) = peak_ram_gb.filter(|v| v.is_finite() && *v >= 0.0) {
            let q = self.lane_peaks.entry((os.to_string(), lane.to_string(), class.to_string(), kind.to_string())).or_default();
            q.push_back(value);
            while q.len() > 9 { q.pop_front(); }
        }
    }

    /// The OS family of a box that reported its caps.
    pub fn os_of(&self, name: &str) -> Option<&str> {
        self.nodes.get(name)?.caps.as_ref().map(|c| c.os.as_str()).filter(|os| !os.is_empty())
    }

    /// The cores of a request that names none (`--cores`), from its class's measured runs (each
    /// one's CPU time over its wall): their p90, from its first run on (the most of its first two),
    /// as its RAM is weighed ([`Self::raw_peak`]), and its class without its test filter's while the
    /// filtered one has none. Cores waited for three runs, and a test that asked none was charged
    /// its kind's 4: a filtered fuzz test measured once at 6.4 cores went in at 4 beside a tests
    /// shard's hold, ran at 11 and kept the shard off its box 9 min (11-07).
    ///
    /// An open-ended session's ([`Self::session`]) are their median instead: a session's CPU over
    /// its open time is mostly its idle share between calls, and the p90 of nine is their highest.
    /// One fab session that ran tree generation on every core of a 32-core box for 19 min (24.3
    /// cores) made each later session of its class ask all 16 of a pool box, where its others
    /// used 0.3-5: L615639 headed the queue 46 min at "budget cores 16, in use 16, asked 16"
    /// (11-07). A session that needs more asks it (`live start --cores`).
    pub(super) fn measured_cores(&self, req: &Req) -> Option<f64> {
        let kind = req.kind.as_str();
        let session = self.session(req);
        let measure = |class: &str| self.cpu.get(&(class.to_string(), kind.to_string())).filter(|q| !q.is_empty()).map(|q| {
            let mut sorted: Vec<f64> = q.iter().copied().collect();
            sorted.sort_by(f64::total_cmp);
            if session { sorted[(sorted.len() - 1) / 2] } else { sorted[(sorted.len() * 9).div_ceil(10) - 1] }
        });
        let base = super::base_class(req.class());
        measure(req.class()).or_else(|| if base != req.class() { measure(base) } else { None })
    }

    /// The RAM ask of a request that names none (`--ram-gb`), from its class's measured peaks: for
    /// scheduling only, never a cap (a lease's scope caps only an asked RAM). It is the highest of
    /// the class's last runs with a fifth more, from its first measured run on, so a run over its
    /// ask raises the next one: a test that peaked at 1.3 GB twice still asked its kind's 6 GB and
    /// waited 7 min for a 16 GB Mac's budget (11-04). A running lease of a measured class is
    /// charged at least this weight, whatever it holds so far ([`Self::charged`]), and a Mac under
    /// pressure takes nothing new. A filtered test's class without any: the highest of its whole
    /// suite's (a filtered run's small peak once capped the whole suite's run: OOM-killed).
    /// `on`: the box it weighs for, whose OS family's own peaks it reads ([`Self::raw_peak`]).
    pub(super) fn measured_ram(&self, req: &Req, on: Option<&str>) -> Option<f64> {
        self.raw_peak(req, on).map(|gb| (gb * 1.2).max(0.5))
    }

    /// The class's highest recent peak on the box `on`, before the scheduling margin is added:
    /// its OS family's peaks, every box's being their starting estimate until the family has
    /// its own. A Fab session weighed 8.9 GB on a 16 GB Mac, all of its budget, from Linux
    /// sessions' 7.4 GB slices while the Macs' own peaked at 2-5.4 GB: each call there needed
    /// the box empty, and one waited 265 s (11-06). A live session weighs its own lane's peaks
    /// on that family first: one class runs every lane's sessions of an app, whatever each does
    /// in them. A Fab session that peaked at 6.5 GB in an hour asked 19.4 GB for each call, from
    /// another lane's 16.2 GB session, and a call waited 220 s for that on its box (11-06).
    fn raw_peak(&self, req: &Req, on: Option<&str>) -> Option<f64> {
        let (kind, os) = (req.kind.as_str(), on.and_then(|n| self.os_of(n)));
        let lane = |class: &str| os.filter(|_| req.live).and_then(|os| self.lane_peaks.get(&(os.to_string(), req.lane.clone(), class.to_string(), kind.to_string()))).filter(|q| !q.is_empty());
        let family = |class: &str| os.and_then(|os| self.family_peaks.get(&(os.to_string(), class.to_string(), kind.to_string()))).filter(|q| !q.is_empty());
        let peaks = |class: &str| lane(class).or_else(|| family(class)).or_else(|| self.peaks.get(&(class.to_string(), kind.to_string())).filter(|q| !q.is_empty()));
        let highest = |q: &VecDeque<f64>| q.iter().copied().fold(0.0, f64::max);
        let base = super::base_class(req.class());
        match peaks(req.class()) {
            Some(q) => Some(highest(q)),
            None if base != req.class() => peaks(base).map(highest),
            None => None,
        }
    }

    /// The RAM weight its node's memory guard holds the lease to, and the raw peak behind it: its
    /// ask, else its class's measured need, else its kind's default, and no more than it was
    /// charged on its box. A box's budget or a shard's resize that cut its charge let other work in
    /// beside it on the rest: weighed at its uncut 46 GB, two shards charged 10.6 and 10.8 GB of a
    /// 30 GB box grew past them, the guard said no lease held more than it was weighed at, and the
    /// kernel's OOM killer picked a process (11-08).
    pub(super) fn ram_weighed(&self, l: &Lease) -> (f64, Option<f64>) {
        let on = l.node.as_deref();
        let need = l.req.ram_gb.or_else(|| self.measured_ram(&l.req, on)).unwrap_or_else(|| l.req.allocation(false).ram_gb);
        (need.min(l.cost.ram_gb), self.raw_peak(&l.req, on))
    }

    /// A JIT build on box `node`: its measured JIT build, else the fleet's, else 90 s.
    pub fn jit_build_ms(&self, node: &str) -> u64 {
        self.node_median("jit-build", "", node).or_else(|| self.median("jit-build", "")).unwrap_or(90_000)
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
    /// A run whose box builds its JIT first (`+jit-build`, learned from its first run) adds that
    /// build to a figure that is not its class's own measure: what it declared, or its warm run's
    /// measure, else its asked cost's length, plus its box's measured JIT build. Learning the class
    /// dropped a lane's lib test from its warm 11m27s to its kind's 5 min, and its rebuild of the
    /// runner was called stalled at 10 min with a lease queued behind it (L669068, 11-08).
    fn run_ms(&self, l: &Lease) -> u64 {
        if let Some(d) = l.req.declared_ms() { return d.saturating_add(self.runner_build_ms(l)); }
        self.measured_ms(l).unwrap_or_else(|| self.warm_ms(l).unwrap_or_else(|| guess_ms(l.req.asked_cost())).saturating_add(self.runner_build_ms(l)))
    }

    /// The JIT build a running lease's box does first ([`super::JIT_BUILD_RUN`] in its class): that
    /// box's measured JIT build, else the fleet's, else 90 s; none for any other run.
    fn runner_build_ms(&self, l: &Lease) -> u64 {
        if !l.req.class().ends_with(super::JIT_BUILD_RUN) { return 0; }
        self.box_build_ms(l)
    }

    /// A JIT build on a lease's box ([`Self::jit_build_ms`]), else the fleet's, else 90 s.
    fn box_build_ms(&self, l: &Lease) -> u64 {
        l.node.as_deref().map_or_else(|| self.median("jit-build", "").unwrap_or(90_000), |n| self.jit_build_ms(n))
    }

    /// The measure of a `+jit-build` run's warm class: the same command where the box keeps its JIT.
    fn warm_ms(&self, l: &Lease) -> Option<u64> {
        let warm = l.req.class().strip_suffix(super::JIT_BUILD_RUN)?;
        self.walls.get(&(warm.to_string(), l.req.kind.as_str().to_string())).and_then(|w| wall_measure(w))
    }

    /// What [`Self::run_ms`] holds a run to that is not its class's own measure, said: "its
    /// declared 5m00s", "its kind's 5m00s", "its warm run's 11m27s and its box's JIT build 50 s".
    fn held_to_said(&self, l: &Lease, base: &str) -> String {
        match self.runner_build_ms(l) {
            0 => base.to_string(),
            built => format!("{base} and its box's JIT build {}", dur(built)),
        }
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

    /// The run of `l` by its class's measure, one no run of the class now on a box has passed (past
    /// it, the class is "measured, exceeded": a guess again), and at least what it declared. None
    /// without one: a run only declared or guessed is no measure ([`Self::held_by`]'s backfill of
    /// a box that drains).
    fn measured_run(&self, l: &Lease) -> Option<u64> {
        let m = self.measured_ms(l)?;
        let exceeded = self.leases.iter().any(|r| r.state.live() && r.req.kind == l.req.kind && r.req.class() == l.req.class() && r.granted_ms.is_some_and(|g| self.clock.saturating_sub(g) > m));
        (!exceeded).then(|| m.max(l.req.declared_ms().unwrap_or(0)))
    }

    /// The same run when choosing between boxes: a box's own walls say which is faster for it.
    fn placed_run_ms(&self, name: &str, l: &Lease) -> u64 {
        if l.req.declared_ms().is_some() { return self.run_ms(l); }
        self.node_median(l.req.class(), l.req.kind.as_str(), name).unwrap_or_else(|| self.run_ms(l))
    }

    fn has_loader(&self, name: &str, l: &Lease) -> bool {
        l.req.on_node() && self.nodes[name].caps.as_ref().and_then(|c| c.jit_store.as_ref()).is_some_and(|s| s.ids.iter().any(|held| l.req.loader.as_ref() == Some(held) || l.req.hint.contains(held)))
    }

    /// The JIT build a lease waits for on this box first (its `--jit`'s copy there, [`Self::jit_source`]).
    fn build_ms(&self, name: &str, l: &Lease) -> u64 {
        if self.jit_source(name, l).is_some() { return self.copy_ms(name); }
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
        self.remaining_after(name, l, cost, true)
    }

    /// [`Self::remaining_ms`], counting the box's live sessions or not (`live`): an exclusive ask
    /// parks them once the rest there has ended ([`Self::park_live`]), so they are no wait of its.
    fn remaining_after(&self, name: &str, l: &Lease, cost: &Cost, live: bool) -> u64 {
        let c = self.nodes[name].caps.as_ref().unwrap();
        // (a live launch parks no session, [`Self::park_live`]: the sessions there are no end of its)
        let left = |r: &Lease| {
            if self.live_launch(l) && self.is_live_job(r) { return u64::MAX; }
            let wall = self.run_ms(r);
            let elapsed = self.clock.saturating_sub(r.granted_ms.unwrap_or(self.clock));
            if elapsed < wall { wall - elapsed } else { wall }
        };
        let ahead = self.ahead_of_background(name, l);
        let stays = |r: &Lease| (live || !self.is_live_job(r) || !Self::parks_for(r, l)) && !(ahead && self.yields_to_live(r) && !l.req.conflicts_on(&r.req, c));
        let mut removed: Vec<String> = self.live_on_node(name, &l.id).filter(|r| !stays(r)).map(|r| r.id.clone()).collect();
        let mut running: Vec<_> = self.live_on_node(name, &l.id).filter(|r| stays(r)).map(|r| {
            (left(r), r.id.clone(), l.req.conflicts_on(&r.req, c))
        }).chain(self.restoring.iter().filter(|(_, (n, ..))| n == name).map(|(id, (_, req, _, ended))| {
            let teardown = Lat::median(&self.nodes[name].lat.teardown).unwrap_or(1_000);
            (self.cleanup_end(name, *ended).unwrap_or(teardown), id.clone(), l.req.conflicts_on(req, c) || l.req.same_tree(req, c.role()))
        })).collect();
        let mut conflicts = running.iter().filter(|(_, _, conflict)| *conflict).count();
        let fits = |removed: &[String], conflicts| {
            let used = self.in_use(name, l, true, removed);
            let mut room = self.room(name, l, cost, true, removed, live);
            if !live { room.cores = 0.0; }
            conflicts == 0 && self.fits(name, l, &used, cost, room).is_ok()
        };
        running.sort_by_key(|(t, _, _)| *t);
        let last = running.last().map_or(0, |(t, _, _)| *t);
        if fits(&removed, conflicts) { return 0; }
        for (t, id, conflict) in running {
            conflicts -= usize::from(conflict);
            removed.push(id);
            if fits(&removed, conflicts) { return t; }
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
            // An open-ended session's wall is how long its lane kept it open, not its work: it is
            // the class it asked (its kind's small), its wall still its ETA. Classed medium by its
            // 13 min, a fab session that used a core waited 10 of its 46 min at the head of the
            // queue behind "node busy", boxes past 95% CPU (L615639, 11-07).
            let class = if self.session(req) { req.asked_cost() } else { CostClass::from_wall(ms, req.allocation(false).cores) };
            return (class, CostSource::Measured(ms));
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
        if let Some(last) = self.deaf_since.take() {
            let deaf = now.saturating_sub(last);
            self.move_heard_clocks(deaf, now);
            self.event(Sev::Info, format!("the master hears its boxes again after {}: its leases' output and idle times do not count that time", dur(deaf)), None);
        }
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
        // a memory stall: said as it starts, and as it eases with how long and how deep, so a red
        // run on the box can be read against it
        let stall = match (super::caps::memory_stall(caps.mem_stall), n.mem_stall) {
            (Some(why), None) => {
                n.mem_stall = Some((now, caps.mem_stall));
                Some((Sev::Warn, format!("{name}: {why}")))
            }
            (Some(_), Some((since, peak))) => {
                n.mem_stall = Some((since, peak.max(caps.mem_stall)));
                None
            }
            (None, Some((since, peak))) => {
                n.mem_stall = None;
                Some((Sev::Info, format!("{name}: memory stall eased after {} (at most {peak:.0}% PSI full avg10)", dur(now.saturating_sub(since)))))
            }
            (None, None) => None,
        };
        n.noisy = (timed_noise(&caps).map(|_| n.noisy.0.unwrap_or(now)), gpu_noise(&caps).map(|_| n.noisy.1.unwrap_or(now)));
        n.caps = Some(caps);
        n.up = true;
        n.unheard_ms = None;
        n.heard_ms = Some(now);
        n.miss.clear();
        n.gone_ms = None;
        if crossed {
            self.event(Sev::Warn, line, None);
        }
        if let Some(why) = clock {
            self.event(Sev::Warn, format!("{name}: {why}"), None);
        }
        if let Some((sev, line)) = stall {
            self.event(sev, line, None);
        }
        !was
    }

    /// Its caps did not answer: it counts as down once they have not for [`DOWN_AFTER_MS`] (a
    /// missed poll, a link the master's exec just dropped, is no reason to reroute its work).
    /// `miss`: why, said on its node line. One that says the host is off the network
    /// ([`super::master::host_gone`]) for [`HOST_GONE_MS`], every miss since, ends the box's live
    /// leases as lost (their ids): their holders get no verdict and ask again. A miss of a host that
    /// answers (a refused connect: its hub restarts) starts that count again.
    pub fn caps_missed(&mut self, name: &str, miss: &str, now: u64) -> Vec<String> {
        let Some(n) = self.nodes.get_mut(name) else { return Vec::new() };
        let since = *n.unheard_ms.get_or_insert(now);
        if now.saturating_sub(since) >= DOWN_AFTER_MS {
            n.up = false;
        }
        let gone = super::master::host_gone(miss);
        n.miss = crate::proto::clean(miss, 160);
        let deaf = self.deaf(name, since, now);
        if deaf {
            self.went_deaf(now);
        } else {
            self.deaf_since = None;
        }
        let Some(n) = self.nodes.get_mut(name) else { return Vec::new() };
        if deaf || !gone {
            n.gone_ms = None;
            return Vec::new();
        }
        let off = now.saturating_sub(*n.gone_ms.get_or_insert(now));
        if off < HOST_GONE_MS { return Vec::new(); }
        let why = format!("its box {name} is off the network for {} ({}): no verdict, ask again", dur(off), n.miss);
        let mut ended = Vec::new();
        while let Some(i) = self.leases.iter().position(|l| l.state.live() && l.node.as_deref() == Some(name)) {
            ended.push(self.leases[i].id.clone());
            self.end_at(i, "lost", Sev::Warn, &why, now);
        }
        ended
    }

    /// What the master's own network check says keeps it from its hubs, None when nothing does
    /// ([`crate::trust::network_problem`]): with it, a silence of all its boxes is its own fault.
    pub fn own_network(&mut self, fault: Option<String>) {
        self.own_fault = fault;
    }

    /// Whether the master hears none of its boxes and the fault is its own, so `name`'s silence
    /// since `since` says nothing of its box: its own network check says so, or every other box
    /// that answered when this one went quiet is quiet too. Boxes do not go off the network
    /// together; the master's link does (an address change silenced every box for 20 min, 11-09).
    /// One box heard lately and the master hears: a box that died then counts as ever. A box that
    /// is alone in the fleet counts as ever too, unless the master's own check says otherwise.
    fn deaf(&self, name: &str, since: u64, now: u64) -> bool {
        if self.nodes.values().any(|n| n.heard_ms.is_some_and(|h| now.saturating_sub(h) < DOWN_AFTER_MS)) {
            return false;
        }
        self.own_fault.is_some() || self.nodes.iter().any(|(other, n)| other != name && n.heard_ms.is_some_and(|h| h.saturating_add(DOWN_AFTER_MS) >= since))
    }

    /// Said once per outage of the master's own network.
    fn went_deaf(&mut self, now: u64) {
        if self.deaf_since.is_some() { return; }
        let last = self.nodes.values().filter_map(|n| n.heard_ms).max().unwrap_or(now);
        self.deaf_since = Some(last);
        let why = self.own_fault.clone().unwrap_or_else(|| "every box went quiet at once".to_string());
        self.event(Sev::Warn, format!("the master has heard none of its boxes for {} ({why}): the fault is its own network, so no box's leases end as lost and no run is judged on what it cannot hear until it hears a box again", dur(now.saturating_sub(last))), None);
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

    /// When a person last gave input at the node: its last caps' answer less the idle time it
    /// said ([`super::UserSense`]). None: no input since its hub started, or no WM of ours.
    pub fn user_input_ms(&self, name: &str) -> Option<u64> {
        let n = self.nodes.get(name)?;
        let idle = n.caps.as_ref()?.user.as_ref()?.idle_ms?;
        Some(n.heard_ms?.saturating_sub(idle))
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

    /// Queues a request; the id (`L12`). Placement happens at the next [`Sched::tick`]. An ask for
    /// work a dropped queued ask was for (the same lane, kind and box, asked again within
    /// [`DROPPED_ASK_KEEP_MS`]) keeps that ask's place and age: a box's prebuild started over at
    /// the back of the queue at each master restart (11-04), and a lane's resubmit of a timed run
    /// lost the 9.5 min it had waited, then waited 31 min more (TREESPEED, 11-07).
    pub fn request(&mut self, mut req: Req, now: u64) -> String {
        if let Some((asked, _)) = self.dropped_asks.remove(&(req.lane.clone(), req.kind, req.node.clone())).filter(|(_, at)| now.saturating_sub(*at) <= DROPPED_ASK_KEEP_MS) {
            req.since_ms = Some(req.since_ms.map_or(asked, |t| t.min(asked)));
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
        let over = self.lane_over(&req).map(|(units, _)| units);
        let mut l = self.proposed(id.clone(), req, now);
        // a box's prebuild asked again at a re-plan (a newer tip) keeps the age of the box's first
        // unserved one: re-plans faster than its run started it over each time, and it never
        // came to be owed its gap ([`Self::prebuild_owed`])
        if let Some(node) = l.req.node.clone().filter(|_| l.req.upkeep()) {
            l.asked_ms = l.asked_ms.min(*self.prebuild_since.entry(node).or_insert(l.asked_ms));
        }
        if let Some(who) = self.blocked_of(&l.req) {
            self.leases.push(l);
            let i = self.leases.len() - 1;
            self.end_at(i, "denied", Sev::Warn, &format!("evicted by the user ({who} is blocked until `devhub unblock {who}`)"), now);
            return id;
        }
        if let Some(running) = over {
            self.event(Sev::Info, format!("{} for {}: past the lane cap ({running}/{} running): it runs after other lanes' leases, where none waits", l.tag(), l.req.lane, self.lane_cap), Some(&l));
        }
        self.leases.push(l);
        id
    }

    /// Construct a lease for capability checks without entering any queue or consuming an id.
    fn proposed(&self, id: String, req: Req, now: u64) -> Lease {
        let (cost_class, cost_source) = self.classify(&req);
        let guess_note = self.guess_note(&req);
        let asked_ms = req.since_ms.map_or(now, |t| t.min(now));
        Lease {
            id,
            cost: req.allocation(false),
            run: req.allocation(false).cores,
            ram_measured: false,
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
            told_ms: None,
            eta_late: false,
            eta_late_by: String::new(),
            eta_box: None,
            warned: false,
            start: String::new(),
            info: String::new(),
            ram_held_gb: None,
            quiet_s: None,
            slow_s: None,
            out_bytes: 0,
            out_grew_ms: None,
            idle_since: None,
            past_budget: 0.0,
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
        }
    }

    /// Why no box could ever take the request, from the request and the boxes' caps alone: what
    /// its lease would be denied for at once when asked ([`Self::place`]), said at a job's
    /// submission instead (a timed job pinned to a Mac without os=macos was denied after 349 s
    /// in its lane's queue, 11-06). A box with unknown caps (or limits) proves nothing: None.
    pub fn never_places(&self, req: &Req) -> Option<String> {
        let l = self.proposed(String::new(), req.clone(), self.clock);
        let names = match self.candidates(&l) {
            Ok(names) => names,
            Err(Place::Deny(why)) => return Some(why),
            Err(_) => return None,
        };
        // (reason, the boxes that give it), as the lease's denial says it
        let mut never: Vec<(String, Vec<&str>)> = Vec::new();
        for name in names {
            let Some(c) = self.nodes[name].caps.as_ref() else { return None };
            match self.never(name, c, &l, &self.cost_on(name, &l)) {
                Ok(()) => return None,
                Err(e) => match never.iter_mut().find(|(r, _)| *r == e) {
                    Some((_, boxes)) => boxes.push(name),
                    None => never.push((e, vec![name])),
                },
            }
        }
        (!never.is_empty()).then(|| never.iter().map(|(e, boxes)| if l.req.node.is_some() { e.clone() } else { format!("{}: {e}", boxes.join(" ")) }).collect::<Vec<_>>().join("; "))
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
        if l.req.kind == Kind::Timed && l.granted_ms.is_some() {
            self.timed_turn.insert(l.req.lane.clone(), now);
        }
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
            // until its cleanup is confirmed it holds what it held running: an ended run grows no
            // more, so a measured class's weight no longer counts (an asked RAM stays its ask)
            let mut held = self.charged(&l);
            if let Some(gb) = l.ram_held_gb.filter(|_| l.req.ram_gb.is_none()) { held.ram_gb = gb; }
            self.restoring.insert(l.id.clone(), (n.clone(), l.req.clone(), held, now));
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
    /// or agent ending its own lease has said nothing about the node), and never a land queue's
    /// gate or a tests shard's lane: the cancel ends that run (a hung unit, a wedged gate), and
    /// the next landing's work is not it. A cancelled hung shard kept the tests bucket off its box
    /// for 30 min, and the next shard headed the queue 16 min for RAM on the other boxes with that
    /// one near empty (L634019, 11-08).
    pub fn cancel_by(&mut self, id: &str, by: &str, now: u64) -> bool {
        let Some(i) = self.leases.iter().position(|l| l.id == id) else { return false };
        let req = &self.leases[i].req;
        let (lane, node) = (req.lane.clone(), if self.leases[i].state.live() && !req.landing_gate() && !req.tests_shard() { self.leases[i].node.clone() } else { None });
        if let (Some(n), "user") = (node, by) {
            self.excluded.insert((lane, n), now + CANCEL_EXCLUDE_MS);
        }
        // the user's cancel says the work is unwanted; a lane's own may be its resubmit
        if by != "user" { self.keep_dropped_ask(i, now); }
        self.end_at(i, "cancelled", Sev::Warn, &format!("cancelled by {by}"), now);
        true
    }

    /// A stalled leg devhub runs ([`Req::unattended`]) whose JIT's capture found it parked (at
    /// code that did not compile, or a panic) ends as hung with that cause: a parked JIT waits for
    /// an edit or a resume, and no one makes one for such a leg. A jitlegs leg's app parked at a
    /// compile error until its script's alarm, 15 min a boot, and the leg held a Mac 22 min
    /// against its 3m46s with its hub's self-update waiting behind it (11-07). Whether it ended.
    pub fn end_parked(&mut self, id: &str, cause: &str, now: u64) -> bool {
        let Some(i) = self.leases.iter().position(|l| l.id == id && l.state == State::Granted && l.req.unattended()) else { return false };
        self.end_at(i, "hung", Sev::Err, &format!("its JIT parked ({cause}): no one edits or resumes a leg devhub runs"), now);
        true
    }

    /// A node's look found this lease took its disk: the lease ends, and that is the verdict on
    /// what it ran, as a hang is. Placed again, it would do the same on the next box.
    pub fn end_for_disk(&mut self, id: &str, why: &str, now: u64) {
        if let Some(i) = self.leases.iter().position(|l| l.id == id && l.state.live()) {
            self.end_at(i, "disk", Sev::Err, why, now);
        }
    }

    /// A node ended this lease's run for memory, before the kernel's OOM killer could pick a
    /// process of any lease ([`super::exec`]'s memory watch). A lease its holder released first
    /// (it heard the verdict from the run's log before the node's beat reached here) ends as
    /// `memory` too: its run was cut, it is no measure of its class's time.
    pub fn end_for_memory(&mut self, id: &str, why: &str, now: u64) {
        if let Some(i) = self.leases.iter().position(|l| l.id == id && l.state.live()) {
            self.end_at(i, "memory", Sev::Err, why, now);
        } else if let Some(l) = self.ended.iter_mut().find(|l| l.id == id && l.state == State::Ended("released".into())) {
            (l.state, l.reason) = (State::Ended("memory".into()), why.to_string());
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
                self.keep_dropped_ask(i, now);
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
        self.dropped_asks.insert((l.req.lane.clone(), l.req.kind, l.req.node.clone()), (asked, now));
        self.end_at(i, "holder-dead", Sev::Info, "", now);
        true
    }

    /// A queued lease its holder drops (released, died, or cancelled by its lane: not the user)
    /// keeps its place for its lane's next ask of the same work ([`Self::request`]).
    fn keep_dropped_ask(&mut self, i: usize, now: u64) {
        let l = &self.leases[i];
        if l.state != State::Queued { return; }
        let asked = l.req.since_ms.map_or(l.asked_ms, |s| s.min(l.asked_ms));
        self.dropped_asks.insert((l.req.lane.clone(), l.req.kind, l.req.node.clone()), (asked, now));
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
                l.slow_s = v.get("slow_s").and_then(super::num).map(|s| s as u64);
                if let Some(b) = v.get("out_bytes").and_then(super::num).map(|b| b as u64).filter(|b| *b > l.out_bytes) {
                    (l.out_bytes, l.out_grew_ms) = (b, Some(now));
                }
                // (a lease that asked its command's class learns here that cargo builds its JIT first)
                let class = v.get("class").and_then(|c| c.as_str()).map(|run| super::lease_class(&l.req.class, run)).filter(|c| !c.is_empty() && *c != l.req.class);
                if let Some(class) = class {
                    l.req.class = class;
                    // its row and stall check read this class now, not every class-less lease of its kind
                    let req = l.req.clone();
                    let (cost_class, cost_source) = self.classify(&req);
                    if let Some(l) = self.leases.iter_mut().find(|l| l.id == id) { (l.cost_class, l.cost_source) = (cost_class, cost_source); }
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

    /// The user takes a node now: its live leases end (`overridden`), except `to`'s and the box's
    /// own hub build (an override ended L515981, a node's, and left its hub on old code, 11-06); with
    /// `to`, that lane's waiting leases go to the front of the queue, pinned to the node. A lane is
    /// given only a box that takes leases: on one with dev off its leases waited there unseen
    /// ("1st in queue (dev off)", 11-06), so that override is refused and ends nothing.
    pub fn override_node(&mut self, node: &str, to: Option<&str>, now: u64) -> Result<Vec<String>, String> {
        if !self.nodes.contains_key(node) {
            return Err(format!("no node {node}"));
        }
        if let Some(lane) = to.filter(|_| !self.dev(node)) {
            return Err(format!("dev off: {node} takes no leases, so its override to lane {lane} would only queue them; switch it on first (`devhub node {node} dev on`), then override"));
        }
        let ids: Vec<String> = self.leases.iter().filter(|l| l.state.live() && l.node.as_deref() == Some(node) && Some(l.req.lane.as_str()) != to && !l.req.hub_build()).map(|l| l.id.clone()).collect();
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

    /// Someone looks at or drives an app on `node` (`spec`: its pid or package, as a route to it
    /// names it; `service` what it opened): a lane's screen run of it is in use. The passive feeds
    /// (`watch`, `stream`) are no look: a panel follows every app.
    pub fn app_touched(&mut self, node: &str, spec: &str, service: &str, now: u64) {
        if matches!(service, "watch" | "stream") {
            return;
        }
        for l in self.leases.iter_mut().filter(|l| l.req.kind == Kind::Screen && l.state.live() && l.node.as_deref() == Some(node)) {
            let pid = makepad_strict_json::parse(l.info.as_bytes()).ok().and_then(|v| v.get("pid").and_then(super::num)).map(|p| p as u64);
            if app_run(&l.req.class) == Some(spec) || pid.is_some_and(|p| p.to_string() == spec) {
                l.idle_since = Some(now);
            }
        }
    }

    /// A lane's screen run of an app that nobody looked at or drove for [`SCREEN_IDLE_MS`] past
    /// what it declared ends: no inspect, grab or act reached its app, no inspect lease named it
    /// (at its grant or while it lives), and no person was at its box (who may watch it through a viewer). A script's run (its own
    /// driver, a rig or sheet), devhub's own work and live jobs keep their own ends.
    fn end_forgotten_screens(&mut self, now: u64) {
        let inspected: Vec<(String, String)> = self.leases.iter().filter(|l| l.req.kind == Kind::Inspect && l.state.live()).filter_map(|l| Some((l.node.clone()?, l.req.jit.clone()?))).collect();
        for (node, jit) in inspected {
            self.app_touched(&node, &jit, "cmd", now);
        }
        let watched: HashSet<String> = self.nodes.keys().filter(|n| self.user_present(n)).cloned().collect();
        let mut forgotten = Vec::new();
        for l in self.leases.iter_mut().filter(|l| l.req.kind == Kind::Screen && l.state == State::Granted && !l.req.master && !l.req.live && !self.live_jobs.contains(&l.id)) {
            let (Some(node), Some(app)) = (l.node.as_deref(), app_run(&l.req.class)) else { continue };
            if watched.contains(node) {
                l.idle_since = Some(now);
                continue;
            }
            let since = l.idle_since.unwrap_or(0).max(l.granted_ms.unwrap_or(now).saturating_add(l.req.declared_ms().unwrap_or(0)));
            if now.saturating_sub(since) > SCREEN_IDLE_MS {
                forgotten.push((l.id.clone(), app.to_string()));
            }
        }
        for (id, app) in forgotten {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "idle", Sev::Warn, &format!("nobody inspected, grabbed or drove its {app} for {} min, so its screen run ended and gave the box back; `live start` keeps an app open for its calls, `--est-min` declares a longer look", SCREEN_IDLE_MS / 60_000), now);
            }
        }
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
                format!("{{\"id\":{},\"state\":\"{}\",\"node\":{},\"granted_ms\":{},\"asked_ms\":{},\"cost\":\"{}\",\"cost_source\":{},\"measured_ms\":{},\"req\":{},\"start\":{},\"phases\":{},\"node_phases\":{},\"allocation\":{{\"cores\":{},\"ram_gb\":{},\"disk_gb\":{},\"gpus\":{}}},\"ram_measured\":{},\"called\":{},\"deploy_pin\":{},\"deployment\":{},\"told_ms\":{}}}", json_str(&l.id), l.state.as_str(), json_str(l.node.as_deref().unwrap_or("")), l.granted_ms.unwrap_or(0), l.asked_ms, l.cost_class.as_str(), json_str(&l.cost_source.label()), l.cost_source.ms(), req.to_json(), json_str(&l.start), l.phases.json(), l.node_phases.json(), l.cost.cores, l.cost.ram_gb, l.cost.disk_gb, l.cost.gpus, l.ram_measured, self.live_called.contains(&l.id), json_str(l.deploy_pin.as_deref().unwrap_or("")), json_str(&l.deployment), l.told_ms.map_or("null".to_string(), |v| v.to_string()))
            })
            // an ended lease whose node has not confirmed its cleanup holds what it held: a
            // restart keeps that, and asks the node again ([`Self::cleaning`])
            .chain(self.restoring.iter().map(|(id, (node, req, c, ended))| {
                format!("{{\"id\":{},\"state\":\"cleaning\",\"node\":{},\"ended_ms\":{ended},\"req\":{},\"allocation\":{{\"cores\":{},\"ram_gb\":{},\"disk_gb\":{},\"gpus\":{}}}}}", json_str(id), json_str(node), req.to_json(), c.cores, c.ram_gb, c.disk_gb, c.gpus)
            }))
            // a timed lease that ended within its series' window keeps the pre-warm off its box and
            // the merge bot off its worktree for the rest of it ([`Self::timed_near`],
            // [`Self::timed_series`]): a restart that forgot the bench box's last run let the box
            // pre-warm at 3.7 cores 13 min into its series' gap (11-10)
            .chain(self.ended.iter().filter_map(|l| {
                let (node, end) = (l.node.as_deref()?, l.ended_ms?);
                let how = match &l.state { State::Ended(how) => how.as_str(), _ => "released" };
                (l.req.kind == Kind::Timed && self.clock.saturating_sub(end) < TIMED_SERIES_MS).then(|| format!("{{\"id\":{},\"state\":\"ended\",\"how\":{},\"node\":{},\"granted_ms\":{},\"ended_ms\":{end},\"req\":{}}}", json_str(&l.id), json_str(how), json_str(node), l.granted_ms.unwrap_or(0), l.req.to_json()))
            }))
            .collect()
    }

    /// The node of an ended lease whose cleanup is not confirmed yet, if `id` is one.
    pub fn cleaning(&self, id: &str) -> Option<&str> {
        self.restoring.get(id).map(|(n, ..)| n.as_str())
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
        // a timed lease that ended within its series' window: back among the ended, nothing live
        if state == "ended" {
            let node = v.get("node").and_then(|x| x.as_str()).filter(|x| !x.is_empty())?.to_string();
            let end = v.get("ended_ms").and_then(super::num).map(|x| x as u64)?;
            if req.kind != Kind::Timed || now.saturating_sub(end) >= TIMED_SERIES_MS { return None; }
            let mut l = self.proposed(id, req, now);
            l.state = State::Ended(v.get("how").and_then(|x| x.as_str()).unwrap_or("released").to_string());
            (l.node, l.ended_ms) = (Some(node), Some(end));
            l.granted_ms = v.get("granted_ms").and_then(super::num).map(|x| x as u64).filter(|&g| g > 0);
            self.ended.push_back(l);
            return None;
        }
        if state == "queued" {
            let asked = v.get("asked_ms").and_then(super::num).map(|x| x as u64);
            // the master's own ask: its asker died with the previous master, which asks again (kept,
            // it would take its box first and hold it through the grace with nothing running); the
            // ask again keeps its place
            if req.master {
                if let Some(a) = asked.or(req.since_ms) {
                    self.dropped_asks.insert((req.lane.clone(), req.kind, req.node.clone()), (a, now));
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
                l.told_ms = v.get("told_ms").and_then(|x| x.as_u64());
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
            let ended = v.get("ended_ms").and_then(super::num).map_or(now, |x| x as u64);
            self.restoring.insert(id.clone(), (node, req, cost, ended));
            return Some(id);
        }
        let l = Lease {
            id: id.clone(),
            cost,
            // (granted before: its run's cores went out with its start)
            run: cost.cores,
            // as its grant weighed it ([`Self::charged`]): the master restores before it reads its
            // measures, and a restart charged every running lease only what it held then: a tests
            // shard granted 45.9 GB charged 18.2, and its sibling got 27.1 GB beside it on a 62 GB
            // box (L634019, 11-08)
            ram_measured: v.get("ram_measured").and_then(|x| x.as_bool()) == Some(true),
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
            told_ms: v.get("told_ms").and_then(|v| v.as_u64()),
            eta_late: false,
            eta_late_by: String::new(),
            eta_box: None,
            warned: false,
            start: v.get("start").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            info: String::new(),
            ram_held_gb: None,
            quiet_s: None,
            slow_s: None,
            out_bytes: 0,
            // What the previous master knew of its activity (what runs, its output, its last use)
            // went with it: those clocks start at the restore, until its node's next word. Counted
            // from its grant, a landing gate 12 min into its steps was ended as "idle for 10 min"
            // and said stalled in the first tick after a master's self-update (L693168, 11-09).
            out_grew_ms: Some(now),
            idle_since: Some(now),
            past_budget: 0.0,
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
        // a live session one of whose calls ended is idle between calls: no call outlives the
        // master that admitted it. Restored as busy, every session was charged its launch's p90
        // after each master update until its next call ended, and its box took little beside it
        if v.get("called").and_then(|x| x.as_bool()) == Some(true) {
            self.live_called.insert(id.clone());
            self.live_idle.insert(id.clone());
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

    /// Timed work on box `name` that keeps the background pre-warm off it (`sync::prewarm`): a
    /// timed lease running there, the timed ask first in its line, or one that ended there within
    /// [`TIMED_SERIES_MS`] (its series' next run comes). A pre-warm in a series' gap ran 4.8 cores
    /// right up to the next run's start, and that start waited for its cancel (11-10).
    pub fn timed_near(&self, name: &str, now: u64) -> Option<String> {
        let here = |l: &Lease| l.req.kind == Kind::Timed && l.node.as_deref() == Some(name);
        if let Some(l) = self.leases.iter().find(|l| l.state.live() && here(l)) {
            return Some(format!("{} (timed, {}) runs there", l.id, l.req.lane));
        }
        if let Some(l) = self.first_timed(name) {
            return Some(format!("{} (timed, {}) waits for it", l.id, l.req.lane));
        }
        self.ended.iter().rev().find(|l| here(l) && l.ended_ms.is_some_and(|e| now.saturating_sub(e) < TIMED_SERIES_MS))
            .map(|l| format!("{} (timed, {}) ended there {} min ago: its series' next run comes", l.id, l.req.lane, now.saturating_sub(l.ended_ms.unwrap_or(now)) / 60_000))
    }

    /// A failed cleanup cannot establish available capacity, even after fresh caps.
    pub fn cleanup_failed(&mut self, node: &str, id: &str) {
        self.cleanup_unknown.insert(id.to_string(), node.to_string());
    }

    /// The node confirmed cleanup with no surviving processes after `teardown_ms`.
    pub fn cleaned(&mut self, node: &str, id: &str, teardown_ms: u64) {
        self.cleanup_unknown.remove(id);
        self.native_wait.remove(node);
        if self.restoring.get(id).is_some_and(|(n, ..)| n == node) {
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
                .chain(self.restoring.values().filter(|(node, ..)| node == name).map(|(_, r, ..)| r)) {
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
        self.leases.iter().filter(|l| l.state == State::Granted && l.req.set_up_on_node() && l.node.as_deref() == Some(node)).map(|l| l.id.clone()).collect()
    }

    /// Its leases being set up on the node (`lease-start` sent, or sent again after a restart):
    /// the master holds them there too, though the node may not have them yet.
    pub fn starting_on(&self, node: &str) -> Vec<String> {
        self.leases.iter().filter(|l| l.state == State::Starting && l.req.set_up_on_node() && l.node.as_deref() == Some(node)).map(|l| l.id.clone()).collect()
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

    pub(crate) fn jit_compilers(&self) -> Vec<String> {
        let mut compilers: Vec<_> = self.nodes.values().filter_map(|n| n.caps.as_ref()?.jit_store.as_ref())
            .map(|s| &s.compiler).filter(|c| !c.is_empty()).cloned().collect();
        compilers.sort();
        compilers.dedup();
        compilers
    }

    pub fn node_view(&self, now: u64) -> Vec<NodeView> {
        self.nodes
            .iter()
            .map(|(name, n)| {
                // from its last answer: a caps call that never returns notes no miss at all
                let quiet = now.saturating_sub(n.heard_ms.or(n.unheard_ms).unwrap_or(now));
                let unheard = (n.unheard_ms.is_some() || quiet >= DOWN_AFTER_MS)
                    .then(|| (quiet, if n.miss.is_empty() { "its caps call has not answered".to_string() } else { n.miss.clone() }));
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
                    unheard,
                    leases,
                    waiting,
                    draining: (!dev || self.user_present(name)) && leases > 0,
                    hold: self.holds.get(name).map(|h| h.id.clone()),
                    updating: self.updating.get(name).cloned(),
                    update_waits: self.update_waits.get(name).map(|w| w.values().cloned().collect::<Vec<_>>().join("; ")),
                    update_pin: self.update_pin(name).filter(|(_, at)| *at != self.tips),
                    timed: self.timed_near(name, now),
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
        if let Some(over) = l.req.exec_profile.as_ref().and_then(|p| c.exec_limits.and_then(|limits| p.over(limits))) { return Err(over); }
        if l.req.hub_build() { return Self::holds(c, l, cost); }
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

        if k == Kind::Timed && c.role() != Role::Bench && self.bench_meets(l) && !spare_timed(c, l) {
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
        // nor by a lane's name: a lane's test pinned there ran in the window a timed lease asked for
        // 25 s later, which waited for it (L696386, 11-09). devhub's own (a box's prebuild, a warm
        // leg, CI, a show) is its choice, made around the box's timed queue ([`Self::gap_wait`])
        if k != Kind::Timed && k != Kind::Inspect && c.role() == Role::Bench && l.req.node.as_deref() == Some(name) && !l.req.devhubs_own() {
            return Err(BENCH_PIN.into());
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
        self.gets_jit(c, l)?;
        Self::holds(c, l, cost)
    }

    /// The JIT a lease asks for by id (`--jit`) is its run's by construction: a box takes it only
    /// when it holds that binary or the lease builds it there first (an overlay's pin's own JIT
    /// for the box's compiler). A job named a box without the JIT its run needed and failed in
    /// 0 s, its next skipped, while five other boxes held it (11-07).
    /// A box that lacks it gets it copied there at the grant from one of its platform that holds
    /// it ([`Self::jit_source`]): a lane's JIT ran only on the box that built it, and drove the
    /// lane's tests onto a bench box (11-07).
    fn gets_jit(&self, c: &Caps, l: &Lease) -> Result<(), String> {
        let Some(id) = Self::asked_jit(l) else { return Ok(()) };
        if holds_jit(c, id) || self.builds_own(c, l, id) { return Ok(()); }
        if self.nodes.values().any(|n| n.caps.as_ref().is_some_and(|h| holds_jit(h, id) && same_platform(c, h))) { return Ok(()); }
        let held: Vec<&str> = self.nodes.iter().filter(|(_, n)| n.caps.as_ref().is_some_and(|h| holds_jit(h, id))).map(|(name, _)| name.as_str()).collect();
        let by = if held.is_empty() { "no box holds it".to_string() } else { format!("held by {}, none of its platform", held.join(" ")) };
        Err(format!("holds no JIT {id} ({by}; a lease builds only its pin's own JIT, as an overlay; without --jit it runs that one)"))
    }

    /// The JIT a lease asks for by id (`--jit`) on its box.
    fn asked_jit(l: &Lease) -> Option<&str> {
        l.req.loader.as_deref().filter(|id| !id.is_empty() && l.req.on_node())
    }

    /// The lease is an overlay whose pin's own JIT for the box's compiler is `id`: it builds it there.
    fn builds_own(&self, c: &Caps, l: &Lease, id: &str) -> bool {
        let own = c.jit_store.as_ref().and_then(|s| self.jit_compilers().iter().position(|x| *x == s.compiler)).and_then(|i| l.req.hint.get(i));
        l.req.src == Src::Overlay && own.is_some_and(|own| own == id)
    }

    /// The box the lease's `--jit` is copied to box `name` from, which lacks it: an up box of the
    /// same platform that holds it, both hubs reporting `jit-copy`; a
    /// pool box before a bench box, whose quiet window its reads would take from. None: the box
    /// holds it, the lease asks for none, or no such box is up.
    pub(crate) fn jit_source(&self, name: &str, l: &Lease) -> Option<&String> {
        let id = Self::asked_jit(l)?;
        let c = self.nodes.get(name)?.caps.as_ref()?;
        if holds_jit(c, id) || !c.has("jit-copy") { return None; }
        self.nodes.iter()
            .filter(|(n, s)| n.as_str() != name && s.up && s.caps.as_ref().is_some_and(|h| h.has("jit-copy") && holds_jit(h, id) && same_platform(c, h)))
            .min_by_key(|&(n, s)| (s.caps.as_ref().is_some_and(|h| h.role() == Role::Bench), n))
            .map(|(n, _)| n)
    }

    /// Why box `name` cannot get the lease's `--jit` now: it lacks it, does not build it, and no
    /// box can copy it there yet ([`Self::jit_source`]).
    fn jit_wait(&self, name: &str, c: &Caps, l: &Lease) -> Option<String> {
        let id = Self::asked_jit(l)?;
        if holds_jit(c, id) || self.builds_own(c, l, id) || self.jit_source(name, l).is_some() { return None; }
        Some(if !c.has("jit-copy") {
            format!("its hub lacks jit-copy (to copy JIT {id} in): waits for its update")
        } else {
            format!("JIT {id}: no box that holds it is up (and updated) to copy it here")
        })
    }

    /// A copy's wall to box `name`: its measure there, else the fleet's, else a guess.
    fn copy_ms(&self, name: &str) -> u64 {
        self.node_median(JIT_COPY, "", name).or_else(|| self.median(JIT_COPY, "")).unwrap_or(JIT_COPY_GUESS_MS)
    }

    /// Whether the box could ever hold the cost.
    fn holds(c: &Caps, l: &Lease, cost: &Cost) -> Result<(), String> {
        if cost.cores > c.cores_for(l.req.kind) as f64 {
            return Err(format!("needs {:.0} cores, has {}", cost.cores, Self::cores_said(c, l.req.kind)));
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

    /// The request's allocation on this box, with an unasked RAM need cut to its budget.
    /// How long a queued lease waits for all the cores it is charged before it starts on what its
    /// box has free ([`Self::cost_on`]): a tests shard [`SHARD_RESIZE_MS`]; a landing gate
    /// [`GATE_RESIZE_MS`]; lane test or build
    /// work charged its class's measured cores (no `--cores`) its runs' median wall, from
    /// [`CORES_WAIT_MIN_MS`] to [`SHARD_RESIZE_MS`]. None: it waits for all of them (an asked
    /// count, a whole box, a hub build, other kinds).
    fn cores_wait_ms(&self, l: &Lease) -> Option<u64> {
        if l.state != State::Queued { return None; }
        if l.req.tests_shard() { return Some(SHARD_RESIZE_MS); }
        if l.req.landing_gate() { return Some(GATE_RESIZE_MS); }
        if l.req.cores.is_some() || l.req.whole || l.req.hub_build() || !matches!(l.req.kind, Kind::Test | Kind::Build) { return None; }
        self.measured_cores(&l.req)?;
        let run = self.median(l.req.class(), l.req.kind.as_str()).unwrap_or(SHARD_RESIZE_MS);
        Some(run.clamp(CORES_WAIT_MIN_MS, SHARD_RESIZE_MS))
    }

    pub(super) fn cost_on(&self, name: &str, l: &Lease) -> Cost {
        let mut cost = l.req.allocation(self.warm_on(name, l));
        if let Some(gb) = self.unasked_ram(name, l) {
            // an unasked need (the kind's default, or the highest peak pooled over every box's
            // leases of the class) takes all of a smaller box's budget, never a denial: a 16 GB
            // Mac's prebuild against a build's 12 GB default or Linux builds' peaks. A measured
            // need goes to such a box only when no box it may use could ever hold it ([`Self::place`]).
            cost.ram_gb = self.nodes[name].caps.as_ref().map_or(gb, |c| gb.min(l.req.ram_budget(c)));
        }
        // A tests shard that waited [`SHARD_RESIZE_MS`] starts on the RAM its box has free, down to
        // a third of its need (one of its three units at once), rather than wait for the box to
        // empty: its run starts its units within what it got (land_gate.sh ram_room,
        // `DEVHUB_RAM_MB`). One asking 42.7 GB headed the queue 14 min for a near-empty box while
        // its siblings dealt its units out (L605885, 11-07).
        if l.req.tests_shard() && l.req.ram_gb.is_none() && l.state == State::Queued && self.clock.saturating_sub(l.asked_ms) >= SHARD_RESIZE_MS {
            if let Some(c) = self.nodes[name].caps.as_ref() {
                let free = (l.req.ram_budget(c) - self.charged_on(name).ram_gb).max(0.0);
                cost.ram_gb = cost.ram_gb.min(free.max(cost.ram_gb / 3.0));
            }
        }
        // ... and never below its floor: a unit it runs alone peaks there. Resized to 10.6 GB of a
        // 30 GB box (a third of its need cut to the box's budget), a shard ran an app's
        // 18-21 GB unit beside another shard, and the kernel's OOM killer took it (11-08).
        if let Some(floor) = l.req.ram_floor_gb.filter(|_| l.req.ram_gb.is_none()) {
            cost.ram_gb = cost.ram_gb.max(floor);
        }
        cost.cores = self.charged_cores(name, l, cost.cores);
        // Such a shard starts on the cores its box has free too, down to a third of its charge, and so
        // does lane cargo work charged its class's measured cores once it waited as long as its
        // runs take ([`Self::cores_wait_ms`]): its run starts its jobs on what it got (land_gate.sh:
        // `MAKEPAD_RUN_JOBS`; cargo's jobs share the box under the lease's CPU weight). One shard
        // asking 12 of a box's 16 headed the queue 60 min with 12-13 in use there by landing gates
        // its hold let pass, and held back lane tests that would have fit beside it all that while
        // (L620174, 11-07). A cargo test compiles on every core, so its measure is near a whole
        // box: 1m45s lane tests asking 15 of 16 headed the queue 24-28 min (11-07). A landing gate
        // starts so after a minute, down to [`GATE_MIN_CORES`]: asking 8 of a box's 16 with 13 in
        // use by a tests shard and live sessions, every hand-in waited on one 15 min (L657290, 11-08).
        if self.cores_wait_ms(l).is_some_and(|wait| self.clock.saturating_sub(l.asked_ms) >= wait) {
            if let Some(c) = self.nodes[name].caps.as_ref() {
                let used = self.in_use(name, l, true, &[]);
                let in_use = used.cost.cores + used.outside;
                let free = (c.cores_for(l.req.kind) as f64 - in_use).floor();
                let least = if l.req.landing_gate() { GATE_MIN_CORES.min(cost.cores) } else { (cost.cores / 3.0).ceil() };
                cost.cores = cost.cores.min(free.max(least));
            }
        }
        // a box's hub build starts now on the cores its box has free, down to
        // HUB_BUILD_MIN_CORES jobs, rather than wait for all it measured: a hub on old code keeps
        // the fleet behind on fixes (L484101 waited 10 min on a Linux node with 8 of 16 cores in use,
        // 11-06). A hub without `hub-jobs` builds with cargo's own jobs: its measured cores.
        if l.req.hub_build() {
            if let Some(c) = self.nodes[name].caps.as_ref().filter(|c| c.has("hub-jobs")) {
                let used = self.in_use(name, l, true, &[]);
                let in_use = used.cost.cores + used.outside;
                cost.cores = cost.cores.min((c.cores_for(l.req.kind) as f64 - in_use).floor().max(HUB_BUILD_MIN_CORES));
            }
        }
        cost
    }

    /// The cores `l` is charged on `name` before its box's free cores cut them ([`Self::cost_on`]):
    /// its class's measure, its ask's or its kind's (`asked`) while its class has none. An ask is
    /// charged its class's measure on either side of it, and runs on its ask ([`Lease::run_cores`]):
    /// a box takes in what its runs use. Hot buckets asked 3 cores, and most of their runs used
    /// more (11-08). Tests shards asked 12 and used 6.9 at their median, 8.8 at p90: about 3 of
    /// each shard's 12 were charged and idle, 3100 core-min a day, while the land queue's own
    /// shards and gates waited at "budget cores" on boxes whose runs used under their cores
    /// (11-10). An ask over its box's cores stays its own: never cut to what the box has. So does
    /// a session's over its measure, the median of lives mostly idle: its calls take its ask
    /// ([`Self::call_cores`]).
    fn charged_cores(&self, name: &str, l: &Lease, asked: f64) -> f64 {
        let most = self.nodes[name].caps.as_ref().map_or(f64::INFINITY, |c| c.cores_for(l.req.kind) as f64);
        if l.req.cores.is_some_and(|a| a > most) { return asked; }
        // a live session starts with its launch: its class's launches' peak ([`Self::call_cores`])
        let launch = if self.is_live_job(l) && l.req.cores.is_none() { self.call_p90(&l.req, true).map(|(p90, _)| p90) } else { None };
        let Some(measured) = launch.or_else(|| self.measured_cores(&l.req)).map(|cores| cores.ceil().max(1.0).min(most)) else { return asked };
        if l.req.cores.is_some() && self.session(&l.req) { measured.max(asked) } else { measured }
    }

    /// The cores `l` runs on, granted `cost` on `name` ([`Lease::run_cores`]): its ask, or its
    /// grant when it asked none. Charged its class's measure either way ([`Self::charged_cores`]):
    /// run on that charge, an ask under it would use more and raise its measure each run, and one
    /// over it would use less and lower it (a tests shard runs a unit per 4 of its cores). Started
    /// on fewer than its charge (its box's free cores, [`Self::cost_on`]), it runs on those.
    fn run_on(&self, name: &str, l: &Lease, cost: &Cost) -> f64 {
        let Some(asked) = l.req.cores else { return cost.cores };
        if cost.cores < self.charged_cores(name, l, asked) { asked.min(cost.cores) } else { asked }
    }

    /// The jobs a granted hub build builds with: its granted cores when its box had fewer free
    /// than it measured ([`Self::cost_on`]); none (cargo's own) when it got all it measured.
    pub fn hub_build_jobs(&self, id: &str) -> Option<u32> {
        let l = self.get(id).filter(|l| l.req.hub_build() && l.state.live())?;
        (l.cost.cores < self.measured_cores(&l.req)?.ceil()).then_some(l.cost.cores as u32)
    }

    /// The RAM need of a request that asks none (`--ram-gb`) before a box's budget cuts it: its
    /// class's measured peaks, else its kind's default. None: it asked.
    fn unasked_ram(&self, name: &str, l: &Lease) -> Option<f64> {
        if l.req.ram_gb.is_some() { return None; }
        let default = l.req.allocation(self.warm_on(name, l)).ram_gb;
        Some(self.measured_ram(&l.req, Some(name)).unwrap_or(default))
    }

    /// The box's budget cuts the request's unasked RAM need: it runs there on less than its class
    /// measured (a gate's 34 GB peaks on a 30 GB box).
    fn ram_cut(&self, name: &str, l: &Lease) -> bool {
        let Some(c) = self.nodes[name].caps.as_ref() else { return false };
        self.unasked_ram(name, l).is_some_and(|gb| gb > l.req.ram_budget(c))
    }

    fn allocation_note(&self, name: &str, l: &Lease) -> String {
        let cost = self.cost_on(name, l);
        let stat = if self.session(&l.req) { "median" } else { "p90" };
        let source = |asked: Option<f64>, measured: Option<f64>| match (asked, measured) {
            (Some(a), Some(m)) if self.charged_cores(name, l, a) != a => format!("asked {a}, charged its class's measured {stat} {m:.1}; it runs on {}", self.run_on(name, l, &cost)),
            (Some(_), _) => "asked".into(),
            (None, m) => m.map_or("kind".into(), |n| format!("measured {stat} {n:.1}")),
        };
        let default = l.req.allocation(self.warm_on(name, l)).ram_gb;
        let ram = match self.measured_ram(&l.req, Some(name)) {
            _ if l.req.ram_gb.is_some() => "asked".to_string(),
            Some(gb) if gb != default => format!("measured peak {:.1}", gb / 1.2),
            _ => "kind".to_string(),
        };
        let calls = if !l.req.live { String::new() } else {
            let said = |launch: bool| self.call_p90(&l.req, launch).map_or("none metered".to_string(), |(p90, n)| format!("p90 {p90:.1} of {n}"));
            format!("; its calls are charged their class's peak cores: launch {}, call {}", said(true), said(false))
        };
        let launch = if l.req.live && l.req.cores.is_none() { self.call_p90(&l.req, true) } else { None };
        let source = match launch { Some((p90, _)) => format!("its class's launch peak p90 {p90:.1}"), None => source(l.req.cores, self.measured_cores(&l.req)) };
        format!("ram {:.1} GB ({ram}); cores {} ({source}){calls}", cost.ram_gb, cost.cores)
    }

    fn refresh_interactive(&mut self, holds: &Holds) {
        self.interactive_boxes.clear();
        let mut boxes = BTreeMap::<String, Vec<String>>::new();
        // (a box's own prebuild asks interactive to head its box's lane work, and is no demand
        // timed work waits for: it runs in the timed queue's gaps, [`Self::gap_wait`])
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
                // One that names no box claims a bench box only when no other is ready and free of
                // another ask's hold: placement takes those first ([`Self::foreground`]). A box held
                // for another is none it can take: counted, an interactive ask ready on a box a long
                // whole ask held waited behind a timed hold on the bench box it was ready on.
                if let Ok(names) = self.candidates(l) {
                    let ready: Vec<_> = names.into_iter().filter(|name| self.ready(name, l, &self.cost_on(name, l)).is_ok()).collect();
                    let bench = |name: &str| l.req.node.is_none() && self.is_bench(name);
                    let spare = ready.iter().any(|name| !bench(name) && holds.get(*name).map_or(true, |h| h.id == l.id));
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

    /// A spare ([`spare_timed`]) takes timed work only while every bench box that meets it is
    /// taken by timed work (a timed lease runs there, or another timed request reserves it) or
    /// down: the one bench Mac queued four timed leases while another Mac could run one (11-03).
    /// Then the ask holds it and it drains as any box does for a timed ask ([`Self::held_by`]);
    /// between timed leases it keeps its own role's work. A request that names it takes it.
    fn spare_wait(&self, name: &str, l: &Lease, holds: &Holds) -> Option<Block> {
        if l.req.kind != Kind::Timed || l.req.node.as_deref() == Some(name) { return None; }
        if self.nodes[name].caps.as_ref().is_none_or(|c| c.role() == Role::Bench || !self.bench_meets(l)) { return None; }
        // a bench box out of rotation (its toolchain or git fails there) takes nothing: the spare runs it
        let free: Vec<&str> = self.nodes.iter().filter(|(b, n)| n.up && n.caps.as_ref().is_some_and(|c| c.role() == Role::Bench && c.timed && c.sick.is_empty() && super::caps::slow_clock(&c.clocksource).is_none() && l.req.needs.iter().all(|(k, v)| meets(c, k, v)))
            && !self.leases.iter().any(|r| r.req.kind == Kind::Timed && r.state.live() && r.node.as_ref() == Some(*b))
            && holds.get(b.as_str()).is_none_or(|h| h.id == l.id || self.get(&h.id).is_none_or(|r| r.req.kind != Kind::Timed))).map(|(b, _)| b.as_str()).collect();
        (!free.is_empty()).then(|| Block::new(format!("a spare box for timed work: it waits while {} can take it", free.join(", "))))
    }

    /// Interactive, live and screen work that names no box: it takes a pool box first, a bench box
    /// only when none fits ([`Self::place`]), and never one a timed request waits for: timed work
    /// waited 50 lease-min an hour on the bench boxes behind interactive leases a pool box could
    /// have taken (11-03).
    fn foreground(&self, l: &Lease) -> bool {
        l.req.node.is_none() && l.req.kind != Kind::Timed && (l.priority == Priority::Interactive || self.is_live_job(l) || l.req.kind == Kind::Screen)
    }

    /// Why a lease's named box is its reason, said in its queue line in place of an unpin: a
    /// series compares its runs on one box, and timed work belongs on a bench box (a timed lease
    /// pinned to the bench box was told to unpin to a spare, 11-10).
    fn pinned_for(&self, l: &Lease, named: &str) -> Option<String> {
        match &l.req.series {
            Some(series) => Some(format!("the box of its series {series} (--series)")),
            None => (l.req.kind == Kind::Timed && self.nodes.get(named).and_then(|n| n.caps.as_ref()).is_some_and(|c| c.role() == Role::Bench)).then(|| "a bench box, where timed runs belong".to_string()),
        }
    }

    fn is_bench(&self, name: &str) -> bool {
        self.nodes[name].caps.as_ref().is_some_and(|c| c.role() == Role::Bench)
    }

    /// A queued timed request that waits for the box and drains it for `l`: it names the box, or
    /// names none and could run there, and `l` is no older ([`Self::drains_for`]).
    fn timed_queued_for(&self, name: &str, l: &Lease) -> Option<&Lease> {
        let c = self.nodes[name].caps.as_ref()?;
        self.leases.iter().find(|r| r.state == State::Queued && r.req.kind == Kind::Timed && !self.denying.contains(&r.id) && Self::drains_for(r, l) && self.timed_could_take(name, c, r))
    }

    /// Whether box `name` could take timed request `r`: it names the box, or names none and could
    /// run there. A spare pool box ([`spare_timed`]) only once the request holds it (the bench
    /// boxes taken by timed work, as the last placement left them): an ask a bench box runs
    /// moves no spare's deployment and keeps none of its prebuilds waiting.
    fn timed_could_take(&self, name: &str, c: &Caps, r: &Lease) -> bool {
        match r.req.node.as_deref() {
            Some(n) => n == name,
            None => self.never(name, c, r, &self.cost_on(name, r)).is_ok() && (!c.timed_spare || c.role() == Role::Bench || self.held_before.get(name).is_some_and(|h| h.id == r.id)),
        }
    }

    /// The timed ask first in line for box `name`: the first queued timed request it can take,
    /// in timed order ([`Self::exclusive_order`]). One that waits for the box's hub to update (a
    /// missing feature) is in no line there yet: it held the box's deployment, so the update it
    /// waited for never came.
    pub(super) fn first_timed(&self, name: &str) -> Option<&Lease> {
        let c = self.nodes.get(name)?.caps.as_ref()?;
        // none asked after the ask that waits for the box's deployment: it would hold the box
        // where it is, and that ask's catch-up would never come ([`Self::deploy_head`])
        let head = self.deploy_head(name).map(|h| (h.asked_ms, h.seq));
        self.leases.iter().filter(|r| self.in_timed_line(name, c, r) && head.is_none_or(|h| (r.asked_ms, r.seq) <= h)).min_by_key(|r| self.exclusive_order(r))
    }

    /// Whether `r` is in box `name`'s timed line: a queued timed request the box could take (it
    /// names it, or names none and could run there) once its hub has the features it needs.
    fn in_timed_line(&self, name: &str, c: &Caps, r: &Lease) -> bool {
        r.state == State::Queued && r.req.kind == Kind::Timed && !self.denying.contains(&r.id) && r.req.required(c.role()).iter().all(|f| c.has(f)) && self.timed_could_take(name, c, r)
    }

    /// The earliest asked of box `name`'s timed line that waits for a deployment the box does not
    /// have yet (its catch-up, [`Lease::deploy_pin`]). That deployment takes the box's next gap and
    /// the ask the box after it; every timed ask after it waits behind both, by ask time
    /// ([`Self::deploy_wait`]). Each timed ask asked after it took the box as the last run ended,
    /// before the update could, and the update waited for that run in turn: jitlegs' warm leg
    /// waited 24 min at the head of a bench box's queue while two later asks ran (11-08).
    fn deploy_head(&self, name: &str) -> Option<&Lease> {
        let c = self.nodes.get(name)?.caps.as_ref()?;
        if c.deploy.is_empty() { return None; }
        self.leases.iter().filter(|r| self.lacks_deployment(c, r) && self.in_timed_line(name, c, r)).min_by_key(|r| (r.asked_ms, r.seq))
    }

    /// Whether `l` waits for a deployment the box (its caps `c`) does not have yet: its catch-up
    /// ([`Lease::deploy_pin`]).
    fn lacks_deployment(&self, c: &Caps, l: &Lease) -> bool {
        !c.deploy.is_empty() && l.deploy_pin.as_deref().is_some_and(|pin| match (Pin::parse(pin), Pin::parse(&c.deploy)) {
            (Some(pin), Some(at)) => !at.fits(&pin),
            _ => pin != c.deploy,
        })
    }

    /// A timed ask asked after the one that waits for box `name`'s deployment waits behind it and
    /// that deployment ([`Self::deploy_head`]). When that ask is also first in timed order there
    /// ([`Self::first_timed`]), every other timed ask waits behind its catch-up, one asked before it
    /// too: a 45-min timed ask took the gap as the run before it ended, ahead of a 6-min one first
    /// in timed order whose catch-up then waited for its whole run, and the 6-min one behind that
    /// waited with it; both had queued 11-14 min (a bench box, 11-11 01:15Z).
    fn deploy_wait(&self, name: &str, l: &Lease) -> Option<Block> {
        if l.req.kind != Kind::Timed { return None; }
        let c = self.nodes.get(name)?.caps.as_ref()?;
        let first = self.first_timed(name).filter(|h| self.lacks_deployment(c, h));
        let h = first.or_else(|| self.deploy_head(name)).filter(|h| h.id != l.id)?;
        let asked_after = (h.asked_ms, h.seq) < (l.asked_ms, l.seq);
        if !asked_after && first.is_none() { return None; }
        let pin = h.deploy_pin.as_deref().unwrap_or("");
        let pin = Pin::parse(pin).map_or_else(|| pin.to_string(), |p| p.short());
        if !asked_after {
            return Some(Block::by(format!("behind a catch-up: {} of {} (first in timed order) waits for {name}'s deployment at {pin}, which takes the next gap; timed work after it starts after both", h.id, h.req.lane), &h.id));
        }
        Some(Block::by(format!("behind a catch-up: {} of {} (asked first) waits for {name}'s deployment at {pin}, which takes the next gap; timed work asked after it starts after both", h.id, h.req.lane), &h.id))
    }

    /// The timed ask first in line on `node` and the deployment (a pin key, [`super::Pin::key`]) the box holds
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
    /// spare ([`spare_timed`]).
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
        (c.load1 / n).max(cores / c.cores_for(Kind::Build).max(1) as f64).max(if c.ram_gb > 0.0 { ram / c.ram_gb } else { 0.0 })
    }

    /// A running lease past twice the run it declared or its row shows (at least 1 min), and 5 min
    /// over it, says so, once: a hung leg holds its box unseen otherwise (a gate leg sat 40
    /// min on an app's first frame against its 13 min, 11-02); a short one's JIT rebuild is no
    /// stall (a 1 min gate step said so at 2.5 min). A class with no measure yet is held to what
    /// it asked or its kind's length, at least [`UNMEASURED_RUN_MS`]: a new class never runs
    /// unwatched (a CI test of one sat 50 min at 0% CPU, 11-04). Whatever its class, a lease whose
    /// every process did nothing for [`QUIET_STALL_MS`] says so too. Both capture why it hangs.
    /// Open-ended sessions (live, screen, inspect) are held only to a measure of their class. A leg
    /// devhub runs past what it declared or measured is marked to end once captured
    /// ([`Self::end_hung_legs`]); held to a guess, it is only said.
    fn say_stalled(&mut self, now: u64) {
        let mut said = Vec::new();
        let mut overran = Vec::new();
        // (a hub build runs no process of its lease: its roll says what its build waits on)
        for l in self.leases.iter().filter(|l| l.state == State::Granted && !self.stalled.contains_key(&l.id) && !self.is_live_job(l) && !l.req.hub_build()) {
            let Some(node) = l.node.as_deref() else { continue };
            let open = self.session(&l.req) || l.req.kind == Kind::Inspect;
            if let Some(quiet) = self.quiet_ms(l) {
                let why = format!("{} of {}: every process idle for {}", l.tag(), l.req.lane, dur(quiet));
                said.push((l.id.clone(), node.to_string(), idle_line(l, node, quiet), why));
                continue;
            }
            // its measure is its class's (the command, not the size of its work: a 426-crate
            // sweep was called stalled against a smaller sweep's 4m20s while it beat and wrote on,
            // 11-04): a lease whose output grows works
            if l.out_grew_ms.is_some_and(|t| now.saturating_sub(t) < OUTPUT_FRESH_MS) {
                continue;
            }
            // what runs now: its JIT's build (its box's word: the lease's `jit` phase, a work tree's
            // cargo step or an overlay's dev JIT) is judged by what it does, above: its processes
            // idle, or its output grown. How long one takes is how much it has to do, from seconds
            // to many minutes: against its box's 32 s measure a healthy cold build read as stalled
            // at 5m33s and its lane was told to cancel it (L702786, 11-09). Once built, its command
            // is held to its own run from the build's end; held to its test, a small test lease's
            // 12 min runner rebuild was its test's run (L693116, 11-08).
            let jit = l.node_phases.0.iter().rev().find(|s| s.phase == "jit");
            if jit.is_some_and(|s| s.end == 0) { continue; }
            // (built as its step: its command's run is what runs, without the build)
            let build = if jit.is_some() { 0 } else { self.runner_build_ms(l) };
            let with_build = |base: String| if build == 0 { base } else { format!("{base} and its box's JIT build {}", dur(build)) };
            let warm = jit.is_some() && l.req.class().ends_with(super::JIT_BUILD_RUN);
            let script = super::lane_script(l.req.class());
            let measure = if warm { self.warm_ms(l) } else { self.measured_ms(l) };
            let (expected, against, measured) = match (l.req.declared_ms(), measure.filter(|_| !script)) {
                (Some(d), _) => (d.max(60_000).saturating_add(build), with_build(format!("its declared {}", dur(d))), true),
                (None, Some(m)) => (m.max(60_000), format!("its {} {}", if warm { "warm run's" } else { "measured" }, dur(m)), true),
                (None, None) if open || script => continue,
                (None, None) => {
                    let base = self.run_ms(l).saturating_sub(self.runner_build_ms(l)).max(UNMEASURED_RUN_MS);
                    let what = match self.warm_ms(l) { Some(w) if w >= UNMEASURED_RUN_MS => "its warm run's", _ if l.req.cost_asked => "the asked", _ => "its kind's" };
                    (base.saturating_add(build), format!("{} (its class has no measure yet)", with_build(format!("{what} {}", dur(base)))), false)
                }
            };
            let ran = now.saturating_sub(jit.map_or(l.granted_ms.unwrap_or(now), |s| s.end));
            if ran > expected.saturating_mul(2).max(expected.saturating_add(STALL_SLACK_MS)) {
                let why = format!("{} of {} running {} against {against} ({})", l.tag(), l.req.lane, dur(ran), l.req.class());
                said.push((l.id.clone(), node.to_string(), format!("{} of {} on {node}: running {} against {against} ({}): stalled? `devhub cancel {}` frees the box", l.tag(), l.req.lane, dur(ran), l.req.class(), l.id), why));
                if measured && (l.req.unattended() || l.req.test_run()) { overran.push((l.id.clone(), against)); }
            }
        }
        self.overran.extend(overran);
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
        let said: Vec<(String, String)> = self.leases.iter().filter(|h| h.state == State::Granted && !self.idle_said.contains(&h.id) && !self.is_live_job(h) && !h.req.hub_build()).filter_map(|h| {
            let node = h.node.as_deref()?;
            let idle = now.saturating_sub((*self.box_idle_since.get(node)?).max(h.granted_ms.unwrap_or(now)));
            if idle < HOLDER_IDLE_MS {
                return None;
            }
            let on_it = |q: &&Lease| q.state == State::Queued && (names_lease(&q.reason, &h.id) || q.eta_box.as_deref() == Some(node) || q.req.node.as_deref() == Some(node));
            let head = self.leases.iter().filter(on_it).min_by_key(|q| self.exclusive_order(q))?;
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
    /// `job wait` (11-05). So is a long run ([`LONG_TIMED_MS`]) while a timed ask that is not
    /// long waits on its box, from its first minute: a 1-min case waited 31 min behind a 64-min
    /// run (11-07). Never a kill: only the run knows where its steps end.
    fn say_overruns(&mut self, now: u64) {
        let said: Vec<(String, String, String)> = self.leases.iter().filter(|h| h.state == State::Granted && h.req.kind == Kind::Timed && !self.yielded.contains_key(&h.id) && !self.is_live_job(h)).filter_map(|h| {
            let node = h.node.as_deref()?;
            let ran = now.saturating_sub(h.granted_ms?);
            if ran < OVERRUN_MIN_MS { return None; }
            let overrun = ran >= self.yield_at_ms(h);
            if !overrun && self.run_ms(h) <= LONG_TIMED_MS { return None; }
            let on_it = |q: &&Lease| q.state == State::Queued && q.req.kind == Kind::Timed && (names_lease(&q.reason, &h.id) || q.eta_box.as_deref() == Some(node) || q.req.node.as_deref() == Some(node));
            let listed: Vec<&Lease> = self.leases.iter().filter(on_it).filter(|q| overrun || self.run_ms(q) <= LONG_TIMED_MS).collect();
            // the box's catch-up a queued timed ask waits for (its deployment, in the box's next
            // gap: [`Self::deploy_head`]) is short work waiting on the run, however long that ask:
            // a bench box's catch-up waited 20 min behind back-to-back 16-18 min timed runs, and
            // the asks behind it with it (L691645, 11-08)
            let catch_up = self.deploy_head(node).filter(|q| q.id != h.id && !listed.iter().any(|l| l.id == q.id));
            let mut whom = Vec::new();
            let (why, short) = if overrun { (format!("{} into its {}", dur(ran), self.estimate_said(h)), "") } else { (format!("a long run, {} into its {}", dur(ran), self.estimate_said(h)), " short") };
            if !listed.is_empty() {
                let waiting: Vec<String> = listed.iter().map(|q| format!("{} of {}", q.id, q.req.lane)).collect();
                whom.push(format!("{short} timed {} wait{} on its box", waiting.join(", "), if waiting.len() == 1 { "s" } else { "" }));
            }
            if let Some(q) = catch_up {
                whom.push(format!(" its box's catch-up, which {} of {} waits for, waits on it", q.id, q.req.lane));
            }
            if whom.is_empty() { return None; }
            Some((h.id.clone(), node.to_string(), format!("{} of {} on {node}: {why}, while{}: its $DEVHUB_YIELD file now exists; stop after the current step with what you have and release, or `devhub cancel {}`", h.tag(), h.req.lane, whom.join(", and"), h.id)))
        }).collect();
        for (id, node, word) in said {
            let l = self.get(&id).cloned();
            self.event(Sev::Warn, word.clone(), l.as_ref());
            self.out.push(Action::Yield { id: id.clone(), node, why: word.clone() });
            self.yielded.insert(id, word);
        }
    }

    /// How far into a timed run it is told to yield to timed work waiting on its box
    /// ([`Self::say_overruns`]): a quarter past its measure or its --est-min, a minute at least;
    /// twice a guess. At twice a 7m45s measure, a timed ask waited 16 min behind the run (L661643,
    /// 11-08). The waiter's queue line says when ([`Self::behind_overrun`]); nothing ends the run.
    fn yield_at_ms(&self, h: &Lease) -> u64 {
        let est = self.run_ms(h);
        if h.req.declared_ms().is_some() || self.measured_ms(h).is_some() { est.saturating_add((est / 4).max(OVERRUN_MIN_MS)) } else { est.saturating_mul(2) }
    }

    /// A queued lease that names its box (`--node`) and has waited [`PIN_WAIT_MS`] while another
    /// box could start it now tells its holder, once: an inbox warning naming its lane and agent,
    /// and its queue line ([`Self::place`]) asks to unpin it or say why. A build waited 17 min
    /// pinned to a busy box with two others free, said only inside its queue line, and two lanes
    /// hit the same in one check-in (11-07). Not devhub's own asks ([`Req::devhubs_own`]): a
    /// lane's spare warm leg waited pinned to the one box it may warm, and the health section
    /// said "unpin it" of it three times in an hour (11-08). Nor one whose box is its reason
    /// ([`Self::pinned_for`]).
    fn say_pinned(&mut self, now: u64) {
        let said: Vec<(String, String)> = self.leases.iter().filter(|l| l.state == State::Queued && !l.req.devhubs_own() && !self.pin_said.contains(&l.id) && now.saturating_sub(l.asked_ms) >= PIN_WAIT_MS).filter_map(|l| {
            let named = l.req.node.as_deref().filter(|n| self.pinned_for(l, n).is_none())?;
            let free = self.free_elsewhere(l, &self.holds);
            if free.is_empty() { return None; }
            let agent = l.req.agent.as_deref().map_or(String::new(), |a| format!(" ({a})"));
            Some((l.id.clone(), format!("{} of {}{agent} waited {} pinned to {named} (--node) while {} could start it now: unpin it (ask again with {} in place of --node), or say why it needs {named}", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms)), free.join(" "), self.needs_of(named))))
        }).collect();
        for (id, line) in said {
            let l = self.get(&id).cloned();
            self.event(Sev::Warn, line, l.as_ref());
            self.pin_said.insert(id);
        }
    }

    /// A timed ask that what runs outside our leases keeps off every box it may take, each not
    /// quiet at every look for [`NEVER_QUIET_MS`] with no lease of ours there to wait for: a wait
    /// that will not end by itself (a Mac's own services at 0.5-1.2 cores held a gpu lease first
    /// in line on an empty Mac, said nowhere, 11-10). Its holder is told once what keeps it, by
    /// process, and what would let it start. Nothing is stopped: those are no lease's.
    fn say_never_quiet(&mut self, now: u64) {
        let said: Vec<(String, String)> = self.leases.iter().filter(|l| l.state == State::Queued && l.req.kind == Kind::Timed && !self.quiet_said.contains(&l.id) && now.saturating_sub(l.asked_ms) >= NEVER_QUIET_MS).filter_map(|l| {
            let mut boxes = Vec::new();
            for name in self.candidates(l).ok()? {
                let c = self.nodes[name].caps.as_ref()?;
                if self.never(name, c, l, &self.cost_on(name, l)).is_err() { continue; }
                // a box a lease of ours holds frees when it ends
                if self.live_on_node(name, &l.id).next().is_some() { return None; }
                let (cpu, gpu) = self.nodes[name].noisy;
                let since = (if l.req.gpu { gpu } else { cpu }).filter(|t| now.saturating_sub(*t) >= NEVER_QUIET_MS)?;
                let why = if l.req.gpu { gpu_noise(c) } else { timed_noise(c) }?;
                boxes.push((name.as_str(), format!("{name} not quiet at any look for {} ({why})", dur(now.saturating_sub(since)))));
            }
            if boxes.is_empty() { return None; }
            let agent = l.req.agent.as_deref().map_or(String::new(), |a| format!(" ({a})"));
            let (what, wants) = if l.req.gpu { (", its GPU's timing,", format!("its GPU under {}% busy outside its leases", super::master::NOISY_GPU_PCT)) } else { ("", format!("under {}% of a core running outside its leases", super::master::NOISY_CPU_PCT)) };
            let said: Vec<&str> = boxes.iter().map(|(_, b)| b.as_str()).collect();
            let names: Vec<&str> = boxes.iter().map(|(n, _)| *n).collect();
            Some((l.id.clone(), format!("{}{what} of {}{agent} has waited {} for a quiet box, and what keeps it is no lease's work: {}. It starts once {} has {wants}: stop or pause those processes there, or ask it of another box", l.tag(), l.req.lane, dur(now.saturating_sub(l.asked_ms)), said.join("; "), names.join(" or "))))
        }).collect();
        for (id, line) in said {
            let l = self.get(&id).cloned();
            self.event(Sev::Warn, line, l.as_ref());
            self.quiet_said.insert(id);
        }
    }

    /// A leg devhub runs ([`Req::unattended`]) ends as a failure once its stall's capture is in
    /// ([`Self::captured`]), when every process of it did nothing for [`QUIET_STALL_MS`], or when
    /// it runs past twice what it declared or measured (and 5 min over it, [`Self::say_stalled`])
    /// and its output grew none for [`OUTPUT_FRESH_MS`]: nobody drives a leg, nothing wakes a
    /// parked JIT, and it holds its tree's one lease (one hung test held all of Windows CI for 30
    /// min, 11-04). A jitlegs leg's fixture stub slept at 37 min against its measured 12 min,
    /// said stalled, while its shells' poll loop kept its box from ever reading idle, until it was
    /// cancelled by hand (L634083, 11-08). A leg with no measure ends on its idleness alone, never
    /// on how long it ran. Its line names where it waited and its capture (`devhub job result`),
    /// for whoever reads its bucket. A lane's test run ([`Req::test_run`]) ends so too, on its
    /// idleness, or past its measure as a leg is while its processes also ran under
    /// [`super::exec::SLOW_CPU_PCT`] of a core for [`QUIET_STALL_MS`]: a Fab test waiting on a
    /// button that never came, its app ticking a sim at 4-12% of a core, was said stalled at 6 min
    /// and held its box until a person cancelled it at 19 min (L798418, 11-10). A test that works
    /// at a core stays, past its measure or not. Other leases have someone to read the stall.
    fn end_hung_legs(&mut self, now: u64) {
        let hung: Vec<(String, String)> = self.leases.iter()
            .filter(|l| l.state == State::Granted && (l.req.unattended() || l.req.test_run()) && self.stalled.contains_key(&l.id))
            .filter_map(|l| {
                let capture = self.captures.get(&l.id)?;
                let slow = l.slow_s.map(|s| s * 1000).filter(|ms| *ms >= QUIET_STALL_MS);
                let why = match (self.quiet_ms(l), self.overran.get(&l.id)) {
                    (Some(quiet), _) => format!("every process idle (under {}% of a core, no I/O) for {}", super::exec::IDLE_CPU_PCT, dur(quiet)),
                    (None, Some(against)) if !l.out_grew_ms.is_some_and(|t| now.saturating_sub(t) < OUTPUT_FRESH_MS) && (l.req.unattended() || slow.is_some()) => {
                        let slow = slow.map_or(String::new(), |ms| format!(", its processes under {}% of a core for {}", super::exec::SLOW_CPU_PCT, dur(ms)));
                        format!("running {} against {against}, its output grown none for {}{slow}", dur(now.saturating_sub(l.granted_ms.unwrap_or(now))), dur(now.saturating_sub(l.out_grew_ms.unwrap_or(l.granted_ms.unwrap_or(now)))))
                    }
                    _ => return None,
                };
                Some((l.id.clone(), if capture.is_empty() { why } else { format!("{why}; {capture}") }))
            })
            .collect();
        for (id, why) in hung {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "hung", Sev::Err, &why, now);
            }
        }
    }

    /// A stalled lease's capture is in (lease/master.rs `capture`): `note` is what it found
    /// (where it waits, its last step line, where its capture is), for the line a leg devhub runs
    /// ends with ([`Self::end_hung_legs`]).
    pub fn captured(&mut self, id: &str, note: &str) {
        if self.leases.iter().any(|l| l.id == id && l.state.live()) {
            self.captures.insert(id.to_string(), note.to_string());
        }
    }

    /// Cores used in the latest caps round. A non-live lease must be charged at least its
    /// grant or this measure: a run charged 8 used 12.5 on a 16-thread script (11-10).
    fn measured_cores_now(&self, r: &Lease) -> Option<f64> {
        let caps = self.nodes.get(r.node.as_deref()?)?.caps.as_ref()?;
        let work = r.req.hub_work().unwrap_or(&r.id);
        caps.busy_leases.iter().find(|(id, _)| id == work).map(|(_, cores)| *cores)
    }

    /// The lease-list row includes its current charge when measured use exceeds its grant.
    pub(super) fn lease_view_line(&self, r: &Lease, now: u64) -> String {
        let line = r.state_line(now, &r.node.as_deref().map(|n| self.addr(n)).unwrap_or_default());
        let cores = if r.state == State::Granted && !self.is_live_job(r) {
            self.measured_cores_now(r).filter(|m| *m >= r.cost.cores + 0.5)
                .map(|m| format!(" cores {} (granted {}, measured {m:.1})", self.charged(r).cores, r.cost.cores))
                .unwrap_or_default()
        } else { String::new() };
        format!("{},\"cores_now\":{}}}", line.strip_suffix('}').unwrap_or(&line), json_str(&cores))
    }

    /// What a running lease takes of its box's budget: at least its grant or measured cores,
    /// but a live session between calls only what its box measures for it now (its JIT's
    /// background work, if any; none where the box cannot tell a lease's CPU), its next call
    /// charged again in full ([`Self::live_room`]), and the RAM it holds. 24 idle sessions at
    /// their full charge kept 16/16 cores admitted on boxes at 1-7% CPU while 23 leases waited
    /// (11-02); at a flat core each, six sessions with their verdicts in held 3 of a box's 16 and
    /// some 170 core-min an hour on four pool boxes while asks waited 103 min for cores (11-11).
    ///
    /// RAM, once its node has looked (until then, and when it asked its RAM, its cost stands): a
    /// lease whose class was measured at its grant is charged that weight or what it holds when
    /// more, since it grows towards its peak: charged the 1 GB its first look found, a run whose
    /// class peaked at 8 GB lets two more start beside it on a 30 GB box, and the three reach
    /// 24 GB together (a 30 GB box ran out of memory under 3 x 8 GB, 11-06). A first run is
    /// charged what it holds, not its kind's default (a screen session's default took
    /// 8.6 GB of a 16 GB Mac's 9 GB budget, 11-03), and so is a live session between calls, whose
    /// next call is admitted on its own ([`Self::live_room`]): a live Fab repro holding 4.5 GB was
    /// charged its class's 11.5 GB of a 9.7 GB budget and every other lease waited on that Mac,
    /// one 40 min (11-04).
    pub(super) fn charged(&self, r: &Lease) -> Cost {
        let mut c = r.cost;
        let idle = self.live_idle.contains(&r.id) && self.is_live_job(r);
        if idle {
            c.cores = self.measured_cores_now(r).unwrap_or(0.0);
        } else if self.is_live_job(r) {
            c.cores = self.live_call_charge.get(&r.id).copied().unwrap_or_else(|| self.call_cores(r));
        } else if let Some(measured) = self.measured_cores_now(r) {
            c.cores = c.cores.max(measured);
        }
        if r.req.ram_gb.is_none() {
            if let Some(gb) = r.ram_held_gb {
                c.ram_gb = if idle { gb } else if self.own_call_peak(r).is_some() { self.call_ram(r) } else if r.ram_measured { gb.max(c.ram_gb) } else { gb };
            }
        }
        // its JIT's build takes all of cargo's jobs ([`super::exec`]'s `jit_step`): every core, and
        // the RAM such a build measured, before it holds it
        if let (true, Some(node)) = (r.building_jit(), r.node.as_deref()) {
            if let Some(caps) = self.nodes.get(node).and_then(|n| n.caps.as_ref()) { c.cores = c.cores.max(caps.cores_for(r.req.kind) as f64); }
            if let Some(gb) = self.jit_build_ram(node) { c.ram_gb = c.ram_gb.max(gb); }
        }
        c
    }

    /// What a lease's JIT build weighs in RAM on `node`: the highest of the last such builds on its
    /// OS family, else on any, with the margin of any measured RAM ([`Self::measured_ram`]). None
    /// before a first one measured: its charge then follows what it holds, as any lease's does.
    pub(super) fn jit_build_ram(&self, node: &str) -> Option<f64> {
        let key = |os: &str| (os.to_string(), "jit-build".to_string(), String::new());
        let family = self.os_of(node).and_then(|os| self.family_peaks.get(&key(os))).filter(|q| !q.is_empty());
        let peaks = family.or_else(|| self.peaks.get(&("jit-build".to_string(), String::new())).filter(|q| !q.is_empty()))?;
        Some((peaks.iter().copied().fold(0.0, f64::max) * 1.2).max(0.5))
    }

    /// The most a live session held so far, once one of its calls ended (its peak then has a
    /// call's in it), as its node measures it; none for a session that asked its RAM.
    fn own_call_peak(&self, l: &Lease) -> Option<f64> {
        l.ram_held_gb.filter(|_| l.req.ram_gb.is_none() && self.live_called.contains(&l.id) && self.is_live_job(l))
    }

    /// What a live session's call weighs in RAM: what the session held at most with a fifth more
    /// once one of its calls ended ([`Self::own_call_peak`]); its first, its cost (its lane's peaks
    /// of its class); never more than its box's budget now. One lane's session that leaked across
    /// ten plates peaked at 16.2 GB, and every later call of that lane's sessions asked 18.3 GB
    /// with the app idle at 5.1 GB; their box's budget had become 17.6 GB, and the calls never got
    /// in, one waiting over 5 min (11-06).
    fn call_ram(&self, l: &Lease) -> f64 {
        let gb = self.own_call_peak(l).map_or(l.cost.ram_gb, |gb| (gb * 1.2).max(0.5));
        match l.node.as_deref().and_then(|n| self.nodes.get(n)).and_then(|n| n.caps.as_ref()) {
            Some(c) => gb.min(l.req.ram_budget(c)),
            None => gb,
        }
    }

    /// An open-ended session (a screen lease or a live job's): it is charged and asks the RAM
    /// measured for it, not its kind's default.
    fn session(&self, req: &Req) -> bool {
        req.kind == Kind::Screen || req.live
    }

    /// What the box's running leases and the ones still being cleaned up are charged now.
    fn charged_on(&self, name: &str) -> Cost {
        let mut used = Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 0.0, gpus: 0 };
        let costs = self.live_on_node(name, "").map(|r| self.charged(r)).chain(self.restoring.values().filter(|(n, ..)| n == name).map(|(_, _, c, _)| *c));
        for c in costs {
            used.cores += c.cores;
            used.ram_gb += c.ram_gb;
            used.disk_gb += c.disk_gb;
            used.gpus += c.gpus;
        }
        used
    }

    /// `devhub stats cores`: each box's cores as charged past its cores in the last hour, from its
    /// pressure samples (one a caps report): the share of samples over and the peak. Charged as the
    /// grants count it (a live session between calls at its idle share, a hub build its lease),
    /// not leases' sizes summed: that counted idle sessions whole (11-05).
    pub fn cores_over(&self, now: u64) -> String {
        let since = now.saturating_sub(3_600_000);
        let oldest = self.nodes.values().filter_map(|n| n.pressure.iter().find(|s| s.t_ms >= since)).map(|s| s.t_ms).min();
        let Some(oldest) = oldest else { return "cores charged past a box's cores: no samples in the last hour\n".to_string() };
        let mut out = format!("cores charged past each box's cores, the last {} min:\n", now.saturating_sub(oldest).div_ceil(60_000));
        for (name, n) in &self.nodes {
            let Some(c) = n.caps.as_ref() else { continue };
            let xs: Vec<f32> = n.pressure.iter().filter(|s| s.t_ms >= since).map(|s| s.res_cores).collect();
            if xs.is_empty() { continue; }
            let over = xs.iter().filter(|r| **r as f64 > c.cores_for(Kind::Build) as f64).count();
            let peak = xs.iter().copied().fold(0.0, f32::max);
            out.push_str(&format!("  {name}: {}% of {} samples over, peak {peak:.0} of {}\n", over * 100 / xs.len(), xs.len(), Self::cores_said(c, Kind::Build)));
        }
        out
    }

    /// Each node's pressure samples since `since`, oldest first (the panel's view).
    pub fn pressure_since(&self, since: u64) -> Vec<(String, Vec<super::panel::Sample>)> {
        self.nodes.iter().map(|(name, n)| (name.clone(), n.pressure.iter().filter(|s| s.t_ms >= since).copied().collect())).collect()
    }

    /// Whether an idle live session's box has room for its full charge again (its next call is
    /// admitted like a lease): why not, if not. Its cores are what the box's work was charged, not
    /// what the box measured busy: work past its charge is background work, whose runs yield to
    /// the call ([`super::Priority::cpu_weight`]), or another session's (calls waited 96-125 s on
    /// a box measured busy, charged 7-9 of 16, 11-06). On a box that weighs CPU, background shared
    /// work's charges don't count at all ([`Self::yields_to_live`]): the call goes ahead of it. Nor
    /// do idle sessions' cores: nothing runs on them, and a 15-core call waited 8 min on a box with 3
    /// cores charged, 2 of them idle sessions', for one of those to time out (W4484, 11-09). Their
    /// RAM counts: they hold it.
    ///
    /// An interactive call does not wait for cores: it starts at once with the cores free beside
    /// that work ([`Self::call_start_cores`]) and runs slower beside it; the work granted after it
    /// is weighed by its lease charges and outside work ([`Self::in_use`]). A test session's
    /// call is charged its class's metered peak (the busiest second of its JIT's compile, 15 of
    /// 16 cores), and with 6 charged beside it a save waited 166 s and 672 s for its call (W4666,
    /// 11-10). A background call still waits for its full charge. RAM is waited for either way.
    pub fn live_room(&self, id: &str) -> Result<(), String> {
        let Some((l, name, c, call, in_use, ram)) = self.call_sums(id) else { return Ok(()) };
        let (budget, ask) = (l.req.ram_budget(c), self.call_ram(l));
        let cores = call + in_use;
        let weighs = Self::weighs_cpu(c);
        let counts = |r: &&Lease| !(weighs && self.yields_to_live(r)) && !(self.is_live_job(r) && self.live_idle.contains(&r.id));
        if cores > c.cores_for(l.req.kind) as f64 && l.priority != Priority::Interactive {
            // why its box's room for a call was not enough ([`Self::call_room`]): another session's
            // call took it, or the room is too small
            let calling: Vec<&str> = self.live_on_node(name, id).filter(|r| self.is_live_job(r) && !self.live_idle.contains(&r.id)).map(|r| r.id.as_str()).collect();
            let why = if !calling.is_empty() { format!("beside the call of {}", calling.join(", ")) } else { "beside the work granted there".to_string() };
            // and who holds the cores, most first: what it waits on, said while it waits
            let mut held: Vec<(f64, String)> = self.live_on_node(name, id).filter(counts).map(|r| {
                let charged = self.charged(r).cores;
                let what = if self.is_live_job(r) { "live, in a call" } else { r.req.kind.as_str() };
                (charged, format!("{} ({what}, {charged:.0})", r.id))
            }).collect();
            held.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let held = if held.is_empty() { "nothing granted".to_string() } else { held.into_iter().map(|(_, h)| h).collect::<Vec<_>>().join(", ") };
            return Err(format!("{name}: waiting for {call:.0} cores ({in_use:.0} of {} in use), held by {held}: {why}", Self::cores_said(c, l.req.kind)));
        }
        if ram > budget { return Err(format!("{name}: budget ram {budget:.1} GB, in use {:.1} GB, asked {ask:.1} GB ({})", ram - ask, l.req.ram_budget_why(c))); }
        Ok(())
    }

    /// A live call's charge beside its box's work ([`Self::live_room`]): the lease, its box and
    /// caps, its full charge ([`Self::call_cores`]), the cores of the work there that counts
    /// against a call (not background shared work on a box that weighs CPU, not idle sessions;
    /// sessions restoring after a master restart do), and the RAM of all of it with the call's.
    fn call_sums(&self, id: &str) -> Option<(&Lease, &str, &Caps, f64, f64, f64)> {
        let l = self.get(id)?;
        let name = l.node.as_deref().filter(|n| self.nodes.contains_key(*n))?;
        let c = self.nodes[name].caps.as_ref()?;
        let weighs = Self::weighs_cpu(c);
        let (mut cores, mut ram) = (0.0, self.call_ram(l));
        for r in self.live_on_node(name, id) {
            let charged = self.charged(r);
            if !(weighs && self.yields_to_live(r)) && !(self.is_live_job(r) && self.live_idle.contains(&r.id)) { cores += charged.cores; }
            ram += charged.ram_gb;
        }
        for (_, _, cost, _) in self.restoring.values().filter(|(n, ..)| n == name) {
            cores += cost.cores;
            ram += cost.ram_gb;
        }
        Some((l, name, c, self.call_cores(l), cores, ram))
    }

    /// The cores an interactive live call starts with when its full charge does not fit beside
    /// its box's work ([`Self::live_room`]): (what is free there, at least a core; its full
    /// charge; the cores in use beside it). None when it fits, or for a background call.
    pub fn call_start_cores(&self, id: &str) -> Option<(f64, f64, f64)> {
        let (l, _, c, call, in_use, _) = self.call_sums(id)?;
        let room = c.cores_for(l.req.kind) as f64;
        (l.priority == Priority::Interactive && call + in_use > room).then(|| ((room - in_use).floor().max(1.0), call, in_use))
    }

    /// The cores and RAM a box keeps for a call of its idle live sessions: the largest one's full
    /// charge back, its cost past its idle share. Work granted beside idle sessions filled their
    /// box, and a call then ran past the box's cores (a box over them 30% of an hour, 11-05) or
    /// waited for that work to end (a median 40 s on that box; a Fab call 220 s for 8 GB of RAM a
    /// test took beside its session, 11-06). Two calls at once: the second waits for the first.
    /// The RAM is kept from an ask only when the call and the ask fit the box's budget together
    /// (`ask_ram` of `budget`): a session whose call needs the box to itself would keep every
    /// other ask off it while it lives, and a call waits for the work beside it instead (an idle
    /// repro on a Mac and its 3.6 GB prebuild, 11-04). A call that waits keeps its room from
    /// every ask until it runs ([`Self::live_waiting`]): one waited over 5 min while work was let
    /// in beside it (11-06). `except`: the lease asking; `removed`: sessions parking to make room.
    /// A box that weighs CPU keeps no cores for a call: the call goes ahead of its background work
    /// ([`Self::yields_to_live`]), and 508 core-min kept in 45 min served 38 core-min of launches
    /// and calls (11-09).
    fn call_room(&self, name: &str, except: &str, removed: &[String], ask_ram: f64, budget: f64) -> (f64, f64) {
        let keeps_cores = !self.nodes.get(name).and_then(|n| n.caps.as_ref()).is_some_and(Self::weighs_cpu);
        self.live_on_node(name, except).filter(|r| !removed.contains(&r.id) && self.is_live_job(r) && self.live_idle.contains(&r.id))
            .map(|r| {
                let (idle, call) = (self.charged(r), self.call_ram(r));
                let ram = if call + ask_ram <= budget || self.live_waiting.contains(&r.id) { (call - idle.ram_gb).max(0.0) } else { 0.0 };
                (if keeps_cores { (self.call_cores(r) - idle.cores).max(0.0) } else { 0.0 }, ram)
            })
            .fold((0.0, 0.0), |(cores, ram), (c, r)| (cores.max(c), ram.max(r)))
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
        let (units, running) = self.lane_over(&l.req)?;
        Some(Block { reason: format!("lane cap {units}/{}", self.lane_cap), blockers: running, disk: false })
    }

    /// A request past its lane's cap: the running units of its lane and kind ([`LANE_CAP`]), and
    /// their leases. The shards of one tests bucket run are one unit, and one of them running takes
    /// the rest in: the cap ran a run's four shards two at a time, the other two 18 min in the
    /// queue at "lane cap 2/2" (11-07).
    fn lane_over(&self, req: &Req) -> Option<(u32, Vec<String>)> {
        if req.kind == Kind::Inspect || req.lane == CI_LANE { return None; }
        let running: Vec<&Lease> = self.leases.iter().filter(|r| r.state.live() && r.req.kind == req.kind && r.req.lane == req.lane).collect();
        if running.iter().any(|r| r.req.same_run(req)) { return None; }
        let units = running.iter().enumerate().filter(|(i, r)| !running[..*i].iter().any(|u| u.req.same_run(&r.req))).count() as u32;
        (units >= self.lane_cap).then(|| (units, running.iter().map(|r| r.id.clone()).collect()))
    }

    /// Everything other than running leases and queued reservations must already permit a start.
    /// A lease past its lane cap is no demand anywhere (reservations, interactive and small-work
    /// demand, idle sampling): only its own placement takes it past the cap ([`Self::place`]).
    fn ready(&self, name: &str, l: &Lease, cost: &Cost) -> Result<(), Block> {
        self.ready_as(name, l, cost, false, false)
    }

    /// Whether the box could hold `l` until its running work ends: the same resource predicate
    /// without those running charges. Outside CPU use counts only while a lease or cleanup remains.
    /// No ask takes a hold on a box whose cleanup is unconfirmed, but one that held the box at the
    /// last look waits out the cleanups of the work that came in beside it, as its estimate does
    /// ([`Self::remaining_ms`]: they end in the box's teardown time). Counted as staying, each end
    /// on its box moved a starved launch's hold to a box 53 min off and back once the cleanup
    /// confirmed, four times in a minute, while short work refilled the box it left (W4322, 11-08).
    fn ready_to_hold(&self, name: &str, l: &Lease, cost: &Cost) -> Result<(), Block> {
        let held = self.held_before.get(name).is_some_and(|h| h.id == l.id);
        self.ready_as(name, l, cost, false, held)
    }

    /// `held`: it holds the box, whose unconfirmed cleanups end before it starts
    /// ([`Self::ready_to_hold`]).
    fn ready_as(&self, name: &str, l: &Lease, cost: &Cost, past_cap: bool, held: bool) -> Result<(), Block> {
        let n = &self.nodes[name];
        if !n.up { return Err(Block::new("node down")); }
        let c = n.caps.as_ref().ok_or_else(|| Block::new("no caps yet"))?;
        self.never(name, c, l, cost).map_err(Block::new)?;
        if !past_cap {
            if let Some(b) = self.lane_block(l) { return Err(b); }
        }
        if !l.req.on_node() { return Ok(()); }
        if self.cleanup_unknown.values().any(|node| node == name) { return Err(Block::new("capacity unknown: lease cleanup unconfirmed")); }
        if l.req.hub_build() { return self.resources_without(name, l, cost, false, &self.cleanups_waited(name, held), true); }
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
        // nor beside what the box runs outside its leases, once none of ours runs there (those it
        // holds the box against): its timings would not be clean (its node looks again at the
        // start, `Exec::timed_waits`, and its release says what shared it). A GPU timing's box is
        // quiet when its GPU is.
        let ours = || self.live_on_node(name, &l.id).next().is_some() || self.restoring.values().any(|(n, ..)| n == name);
        let noise = || if l.req.gpu { gpu_noise(c) } else { timed_noise(c) };
        if let Some(why) = (l.req.kind == Kind::Timed && !ours()).then(noise).flatten() {
            return Err(Block::new(format!("the box is not quiet ({why}): timed work starts after it")));
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
            if let Some(r) = self.live_on_node(name, &l.id).find(|r| l.req.conflicts_on(&r.req, c)) {
                return Err(Block::by(format!("held by {} ({} of {}); then the box catches up first ({what}): timed work starts after it", r.id, r.req.kind.as_str(), r.req.lane), &r.id));
            }
            return Err(Block::new(format!("the box fell over 30 min behind and catches up first ({what}): timed work starts after it")));
        }
        // its windows would fail there (four visible live apps exited 1 on a node while new ones kept
        // its WM restart waiting, 11-02): the restart runs once the ones there end
        if !c.wm_wire.is_empty() && l.req.uses_wm() {
            return Err(Block::new(format!("its WM speaks another hosted wire ({}): no screen or gui lease until it restarts, once the ones there end", c.wm_wire)));
        }
        // a WM that shows nothing: each window there timed out on its first frame (fourteen gui
        // and screen leases in 3 h on a node whose WM window was 0x0, 11-07)
        if !c.wm_dark.is_empty() && l.req.uses_wm() {
            return Err(Block::new(format!("no display ({}): no screen or gui lease until it shows one", c.wm_dark)));
        }
        // its WM restart waits only on leases: none it would wait for starts there until it has
        // restarted, so it does in the gap (back-to-back landings kept four boxes' WMs waiting for
        // hours, each sync apply holding the restart while the next tests run took the box, 11-07)
        if !c.wm_drain.is_empty() && l.req.cut_by_wm_restart() {
            return Err(Block::new(format!("its WM restarts first, once what it waits on ends ({}): no screen or gui lease starts there until it has", c.wm_drain)));
        }
        let missing: Vec<&str> = l.req.required(c.role()).into_iter().filter(|f| !c.has(f)).collect();
        if !missing.is_empty() {
            let have = if c.features.is_empty() {
                " (it reports no features: an older hub)".to_string()
            } else {
                let boxes: Vec<&str> = self.nodes.iter()
                    .filter(|(_, n)| n.up && n.caps.as_ref().is_some_and(|c| missing.iter().all(|f| c.has(f))))
                    .map(|(name, _)| name.as_str()).collect();
                if boxes.is_empty() { String::new() } else { format!(" (boxes that have it: {})", boxes.join(", ")) }
            };
            return Err(Block::new(format!("its hub lacks {} for this request{}: waits for its update", missing.join(", "), have)));
        }
        if let Some(why) = self.jit_wait(name, c, l) { return Err(Block::new(why)); }
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
        if let Some(why) = super::caps::memory_stall(c.mem_stall).filter(|_| l.req.on_node()) {
            return Err(Block::new(why));
        }
        self.resources_without(name, l, cost, false, &self.cleanups_waited(name, held), true)
    }

    /// The leases whose cleanup on `name` an ask that holds it waits out ([`Self::ready_to_hold`]):
    /// every one the box has yet to confirm. None for a start or a new hold.
    fn cleanups_waited(&self, name: &str, held: bool) -> Vec<String> {
        if !held { return Vec::new(); }
        self.restoring.iter().filter(|(_, (n, ..))| n == name).map(|(id, _)| id.clone()).collect()
    }

    /// With `running` false, even removing all running leases must leave enough usable budget.
    /// Cleanup and external disk pressure cannot be reserved away (a hold already taken waits out
    /// its box's cleanups: [`Self::cleanups_waited`]).
    fn resources(&self, name: &str, l: &Lease, cost: &Cost, running: bool) -> Result<(), Block> {
        self.resources_without(name, l, cost, running, &[], true)
    }

    /// `ram_rooms`: the RAM idle live sessions keep for their next call counts ([`Self::call_room`]).
    /// `removed`: running leases parked for it, or cleanups a hold waits out: their charges are free.
    fn resources_without(&self, name: &str, l: &Lease, cost: &Cost, running: bool, removed: &[String], ram_rooms: bool) -> Result<(), Block> {
        if !l.req.on_node() { return Ok(()); }
        let c = self.nodes[name].caps.as_ref().unwrap();
        if let Some((id, _)) = self.restoring.iter().filter(|(id, (n, req, ..))| n == name && !removed.contains(id) && (l.req.conflicts_on(req, c) || l.req.same_tree(req, c.role()))).min_by_key(|(id, _)| *id) {
            // the cleanup's answer is what the scrub waits for: what its hub waits on, if anything
            let stalled = c.stalls.first().map_or(String::new(), |s| format!("; its hub waits: {s}"));
            return Err(Block::by(format!("scrubbing its work tree after {id}{stalled}"), id));
        }
        if running {
            if let Some(r) = self.live_on_node(name, &l.id).filter(|r| !removed.contains(&r.id)).find(|r| l.req.conflicts_on(&r.req, c)) {
                let turn = if l.req.same_tree(&r.req, c.role()) { ": one lease at a time in its tree" } else { "" };
                return Err(Block::by(format!("held by {} ({} of {}){turn}", r.id, r.req.kind.as_str(), r.req.lane), &r.id));
            }
        }
        if l.req.native(c.role()) && !c.native.iter().any(|lane| lane == l.req.tree_lane()) {
            let holders: Vec<_> = self.live_on_node(name, &l.id).filter(|r| running && !removed.contains(&r.id)).map(|r| (&r.id, &r.req))
                .chain(self.restoring.iter().filter(|(id, (n, ..))| n == name && !removed.contains(id)).map(|(id, (_, req, ..))| (id, req)))
                .filter(|(_, req)| req.native(c.role())).collect();
            let lanes: HashSet<_> = holders.iter().map(|(_, req)| req.tree_lane()).collect();
            if !lanes.contains(l.req.tree_lane()) && lanes.len() >= c.native_trees as usize {
                return Err(Block { reason: format!("native trees busy: {}/{} held", lanes.len(), c.native_trees), blockers: holders.iter().map(|(id, _)| (*id).clone()).collect(), disk: false });
            }
        }
        let used = self.in_use(name, l, running, removed);
        self.fits(name, l, &used, cost, self.room(name, l, cost, running, removed, ram_rooms))
    }

    /// What the box measured busy past everything charged there (each lease at [`Self::charged`],
    /// each unconfirmed cleanup at its cost), in cores: its own services and a person's apps, which
    /// no lease is charged for. A lease is charged at least its measure, so a box whose hub names
    /// no lease's use (its first look after a restart, a Windows box) charges its grants and
    /// counts the rest as outside: with nothing removed, charges plus this are the box's
    /// measure whenever that is more than its charges.
    fn outside_cores(&self, name: &str) -> f64 {
        let Some(busy) = self.nodes[name].caps.as_ref().and_then(|c| c.cpu_pct.map(|pct| pct / 100.0 * c.cores as f64)) else { return 0.0 };
        let charged = self.live_on_node(name, "").map(|r| self.charged(r).cores).sum::<f64>()
            + self.restoring.values().filter(|(n, ..)| n == name).map(|(_, _, cost, _)| cost.cores).sum::<f64>();
        (busy - charged).max(0.0)
    }

    /// Charges that remain on the box, including unconfirmed cleanups. An empty box takes
    /// what fits its cores whatever its own services use.
    fn in_use(&self, name: &str, l: &Lease, running: bool, removed: &[String]) -> InUse {
        let mut used = InUse { cost: Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 0.0, gpus: 0 }, ids: Vec::new(), outside: 0.0 };
        let ahead = self.ahead_of_background(name, l);
        for (id, cost) in self.live_on_node(name, &l.id).filter(|r| running && !removed.contains(&r.id)).map(|r| {
            let mut cost = self.charged(r);
            // A live launch passes yielding background cores; their other resources still count.
            if ahead && self.yields_to_live(r) { cost.cores = 0.0; }
            (&r.id, cost)
        }).chain(self.restoring.iter().filter(|(id, (n, ..))| n == name && !removed.contains(id)).map(|(id, (_, _, cost, _))| (id, *cost))) {
            used.cost.cores += cost.cores;
            used.cost.ram_gb += cost.ram_gb;
            used.cost.disk_gb += cost.disk_gb;
            used.cost.gpus += cost.gpus;
            used.ids.push(id.clone());
        }
        if !used.ids.is_empty() { used.outside = self.outside_cores(name); }
        used.ids.sort();
        used
    }

    fn room(&self, name: &str, l: &Lease, cost: &Cost, running: bool, removed: &[String], ram_rooms: bool) -> Room {
        let c = self.nodes[name].caps.as_ref().unwrap();
        // A hub build keeps no room for a live session's next call.
        let (cores, ram_gb) = if running && !l.req.hub_build() { self.call_room(name, &l.id, removed, cost.ram_gb, l.req.ram_budget(c)) } else { (0.0, 0.0) };
        // a box's hub build waits for no cores: it builds beside what runs there, on the cores the
        // box has free and at least HUB_BUILD_MIN_CORES jobs, past its cores for the build's
        // minutes. It is devhub keeping itself current: heading its box's queue, a Linux node's
        // still waited 10m48s on a box charged 16 of 16 (11-07), and an older hub's work
        // waited on the box for it (a tests shard behind a 45-min run, 11-07). A hub without
        // `hub-jobs` builds on cargo's own jobs, every core: it waits for its room. So does the
        // box's own prebuild ([`Req::upkeep`]) for cores, on the jobs of its charge, whose CPU weight
        // gets it those beside background work: every lane's next lease there builds its JIT itself
        // without it, and a 49 s one headed a Linux node's queue 20 min behind two background tests
        // shards charged its 16 cores (11-08). Its RAM stays charged: it waits for idle live sessions to end, not past them.
        // One of these builds goes past the box's cores at a time; the next waits for its room or
        // for that one to end: a prebuild, a deployment's build and a hub build granted together
        // beside a 12-core tests shard had a 16-core box at 24 (L780395, L780405, L780406, 11-10).
        // The box's hub build goes past them beside another: the box's roll waits for it, and the
        // fleet's update for the box (L791258 waited 2 min at "budget cores" for its box's
        // deployment build, 11-10). At most two go past the cores at once.
        let bump = l.req.hub_build() && c.has("hub-jobs");
        let own_update = l.req.hub_build() && !l.req.deploy_build();
        let stacked = running && !own_update && self.live_on_node(name, &l.id).any(|r| !removed.contains(&r.id) && (r.req.hub_build() || r.req.upkeep()));
        Room { cores, ram_gb: if ram_rooms { ram_gb } else { 0.0 }, cores_bump: (bump || l.req.upkeep()) && !stacked, hub_ram: bump }
    }

    /// One resource predicate for starts, holds and estimates after leases end.
    fn fits(&self, name: &str, l: &Lease, u: &InUse, cost: &Cost, extra: Room) -> Result<(), Block> {
        let c = self.nodes[name].caps.as_ref().unwrap();
        let blockers = || u.ids.clone();
        let measured = if u.outside >= 0.5 {
            // and what took the cores past the grants: a lease past its grant by a core or
            // more, work of no lease's by its pid (a box charged a few cores measured 7-8 more for
            // 9 min, and nothing said what, W4322, 11-08)
            // (a hub-build lease's is its hub's build, which the box names by its class)
            let mut busy: Vec<String> = u.ids.iter().filter_map(|id| {
                let r = self.get(id)?;
                let g = r.cost.cores;
                let work = r.req.hub_work();
                let (_, m) = c.busy_leases.iter().find(|(l, m)| l == work.unwrap_or(id.as_str()) && *m >= g + 1.0)?;
                let what = match work { Some(DEPLOY_BUILD) => " (its deployment's build)", Some(_) => " (its hub's build)", None => "" };
                Some(format!("{id}{what} {m:.1} cores (granted {g:.0})"))
            }).collect();
            if !c.busy_outside.is_empty() { busy.push(crate::top::outside_said(&c.busy_outside)); }
            let busy = if busy.is_empty() { String::new() } else { format!("; busy: {}", busy.join(", ")) };
            format!(" (outside its leases {:.0}{busy})", u.outside)
        } else { String::new() };
        let budget = l.req.ram_budget(c);
        let (room, ram_room) = (extra.cores, extra.ram_gb);
        let kept = if room > 0.0 { format!(", kept for a live call {room:.0}") } else { String::new() };
        let kept_ram = if ram_room > 0.0 { format!(", kept for a live call {ram_room:.1} GB") } else { String::new() };
        // Its RAM goes by the box's measure: what the box has available less its margin
        // ([`margin_share`]), not by charges. Running work is charged its class's peak
        // ([`Self::charged`]) for the whole run, and a tests shard granted its box's budget kept
        // that box's hub build waiting its 40 min (a 62 GB box: 48.1 GB charged of 45.2, 40 GB free,
        // 11-08); a build of a few GB for minutes fits the margin those peaks leave.
        if extra.hub_ram && c.ram_gb > 0.0 {
            let room = c.ram_avail_gb - c.ram_gb * margin_share(c);
            if room < cost.ram_gb {
                return Err(Block { reason: format!("ram: {:.1} GB available less its {:.0}% margin, asked {:.1} GB (a hub build goes by the box's measure)", c.ram_avail_gb, margin_share(c) * 100.0, cost.ram_gb), blockers: blockers(), disk: false });
            }
        }
        for (what, have, total, add) in [("cores", u.cost.cores + u.outside + room, c.cores_for(l.req.kind) as f64, cost.cores), ("ram", u.cost.ram_gb + ram_room, budget, cost.ram_gb), ("gpu", u.cost.gpus as f64, c.gpus.len() as f64, cost.gpus as f64)] {
            if extra.cores_bump && what == "cores" || extra.hub_ram && what == "ram" && c.ram_gb > 0.0 { continue; }
            if have + add > total {
                let reason = if what == "ram" {
                    format!("budget ram {total:.1} GB, in use {:.1} GB{kept_ram}, asked {add:.1} GB ({})", have - ram_room, l.req.ram_budget_why(c))
                } else if what == "cores" {
                    let beside = if self.ahead_of_background(name, l) { " (background work not counted)" } else { "" };
                    format!("budget cores {total:.0}{}, in use {:.0}{beside}{measured}{kept}, asked {add:.0}", Self::pinned_said(c, l.req.kind), u.cost.cores)
                } else {
                    format!("budget {what} {total:.0}, in use {have:.0}, asked {add:.0}")
                };
                return Err(Block { reason, blockers: blockers(), disk: false });
            }
        }
        if l.cost_class != CostClass::Small && l.req.kind.writes() && c.ram_gb > 0.0 && c.ram_avail_gb < c.ram_gb * 0.1 {
            return Err(Block { reason: format!("ram: {:.1} GB available of {:.1} (under 10%)", c.ram_avail_gb, c.ram_gb), blockers: blockers(), disk: false });
        }
        let free = c.disk_free_gb - u.cost.disk_gb;
        if free - cost.disk_gb < c.disk_floor_gb {
            return Err(Block { reason: format!("disk floor: {free:.0} GB free, needs {:.0}, floor {:.0}", cost.disk_gb, c.disk_floor_gb), blockers: blockers(), disk: true });
        }
        Ok(())
    }

    /// ` (of 32, pinned to its cpus.shared)` where `kind` runs on fewer than the box's cores.
    fn pinned_said(c: &Caps, kind: Kind) -> String {
        let n = c.cores_for(kind);
        if n < c.cores { format!(" (of {}, pinned to its cpus.{})", c.cores, if kind == Kind::Timed { "timed" } else { "shared" }) } else { String::new() }
    }

    /// A box's cores for `kind`, said: `16 (of 32, pinned to its cpus.shared)`.
    fn cores_said(c: &Caps, kind: Kind) -> String {
        format!("{}{}", c.cores_for(kind), Self::pinned_said(c, kind))
    }

    /// Exclusive requests drain in this order: bumped first, then interactive before background,
    /// then by epoch ([`Self::exclusive_epoch`]), then those within [`LONG_TIMED_MS`] before long
    /// ones, then by expected end ([`Self::exclusive_place`]). Short before long alone let short
    /// asks pass a long one without bound; the epoch fixes a long ask's place once it has queued
    /// [`STARVED_MS`].
    fn exclusive_order(&self, r: &Lease) -> (bool, bool, usize, bool, u64, u64) {
        (r.req.prio > BUMP_PRIO, r.priority != Priority::Interactive, self.exclusive_epoch(r), self.run_ms(r) > LONG_TIMED_MS, self.exclusive_place(r), r.seq)
    }

    /// How many long exclusive asks (over [`LONG_TIMED_MS`]) fixed their place ahead of `r`: one
    /// that has queued [`STARVED_MS`] fixes it at its ask + [`STARVED_MS`] (its fix). Asks whose turn
    /// ([`Self::exclusive_turn`]) came before that moment stay ahead of it in their order (short
    /// before long, then the earliest end), and none whose turn came later gets ahead of it: they
    /// count its fix in their epoch, it counts only the fixes before its own. A long ask waited
    /// behind every short one asked while it waited, without bound; and one that had waited 19 min
    /// must not go ahead of two 6-min ones asked 5 and 8 min after it either (a bench box, 11-11).
    /// (Not [`Self::starved`]: it is false for every exclusive ask.)
    fn exclusive_epoch(&self, r: &Lease) -> usize {
        // (the run's measure is read only for asks that queued that long: few)
        let fix = |l: &Lease| Some(l.asked_ms.saturating_add(STARVED_MS)).filter(|f| self.clock >= *f && self.run_ms(l) > LONG_TIMED_MS);
        let own = (r.state == State::Queued).then(|| fix(r)).flatten();
        let turn = self.exclusive_turn(r);
        self.leases.iter().filter(|l| l.state == State::Queued && l.req.exclusive()).filter_map(fix)
            .filter(|f| match own { Some(own) => *f < own, None => *f <= turn }).count()
    }

    /// An exclusive ask's place in its queue: when it would end, run from its turn. A whole-box
    /// ask's turn is its ask. A timed one's is its ask, but never before its lane's last timed run
    /// ended (last while one runs). Lanes take turns: a chain's next case keeps the chain's first
    /// ask (`since_ms`), and it went ahead of every lane that asked while its previous case ran:
    /// one lane's chain held a bench box three cases back to back while another lane's timed asks
    /// waited 14-20 min (11-07). The earliest end goes first: a short run before a long one whose
    /// turn came up to their lengths' difference before its own, so a long run waits for no short
    /// one whose turn came past its own length. A 1-min case waited 9.4 min behind a run
    /// estimated at 26 min whose turn came 12 s before its own (TREESPEED behind JITTIER, 11-07).
    fn exclusive_place(&self, r: &Lease) -> u64 {
        self.exclusive_turn(r).saturating_add(self.run_ms(r))
    }

    /// An exclusive ask's turn ([`Self::exclusive_place`]): its ask, a timed one's never before its
    /// lane's last timed run ended; `u64::MAX` while its lane has a timed lease live.
    fn exclusive_turn(&self, r: &Lease) -> u64 {
        if r.req.kind != Kind::Timed {
            r.asked_ms
        } else if self.leases.iter().any(|o| o.id != r.id && o.req.kind == Kind::Timed && o.req.lane == r.req.lane && o.state.live()) {
            u64::MAX
        } else {
            self.timed_turn.get(&r.req.lane).map_or(r.asked_ms, |ended| r.asked_ms.max(*ended))
        }
    }

    /// A draining request's box and expected wait there ([`Self::drains_box`]): the box's running
    /// work, then the draining requests ahead of it there.
    fn eta(&self, l: &Lease) -> Option<(String, u64)> {
        let name = match l.req.node.clone().filter(|n| self.nodes.contains_key(n)) {
            Some(n) => n,
            None => self.waiting_box(l, &self.holds, false)?,
        };
        self.nodes[&name].caps.as_ref()?;
        let ahead: u64 = self.leases.iter().filter(|r| r.state == State::Queued && self.drains_box(r) && r.id != l.id && self.exclusive_order(r) < self.exclusive_order(l)
            && (r.req.node.as_deref() == Some(name.as_str()) || self.holds.get(&name).is_some_and(|h| h.id == r.id))).map(|r| self.run_ms(r)).fold(0, u64::saturating_add);
        let wait = self.remaining_ms(&name, l, &self.cost_on(&name, l)).saturating_add(ahead);
        // (no end known there, as behind a live session: no start to tell. `u64::MAX` in its state
        // line fits no JSON integer, so its holder read "beat: no answer" and gave up its ask.)
        Some((name, if wait == u64::MAX { 0 } else { wait }))
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
        let r = self.live_on_node(name, &l.id).filter(|r| !self.is_live_job(r) && l.req.conflicts_on(&r.req, c) && ran(r) >= self.run_ms(r)).max_by_key(|r| ran(r) - self.run_ms(r))?;
        let agent = r.req.agent.as_deref().map_or(String::new(), |a| format!(" ({a})"));
        // a timed run hears when to yield to timed work: its waiters read the same deadline
        let yields = if r.req.kind != Kind::Timed || l.req.kind != Kind::Timed {
            String::new()
        } else if self.yielded.contains_key(&r.id) {
            "; told to yield".to_string()
        } else {
            let at = self.yield_at_ms(r);
            format!("; told to yield at {}, in {}", dur(at), dur(at.saturating_sub(ran(r))))
        };
        Some(format!("behind {} of {}{agent}, {} into its {}{yields}", r.tag(), r.req.lane, dur(ran(r)), self.estimate_said(r)))
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
        // what yields to a live launch once it runs keeps nothing from it while it waits
        if self.ahead_of_background(name, l) && self.yields_to_live(owner) && !owner.req.exclusive() && !l.req.conflicts_on(&owner.req, self.nodes[name].caps.as_ref()?) { return None; }
        // a lane the user put first takes a box another lane's lease holds (a box's hub build
        // still goes first on its box, and a landing's gate, which the first lanes' landings
        // wait on too)
        if self.first_lanes.contains(&l.req.lane) && !self.first_lanes.contains(&owner.req.lane) && !owner.req.hub_build() && !owner.req.landing_gate() { return None; }
        // A tests run's shards are one unit: a sibling's hold keeps none of them from holding a box
        // of its own (no reservation goes through another candidate box, [`Self::can_wait`]). With
        // one shard holding, its siblings held nothing, and lane leases granted on every other box
        // kept three of a run's four shards queued 21 min (11-07).
        if owner.req.tests_shard() && l.req.tests_shard() && owner.req.rev == l.req.rev { return None; }
        if owner.req.kind == Kind::Timed && !Self::drains_for(owner, l) { return None; }
        // the box's own hub build goes first; its prebuild runs in the timed queue's gaps
        // ([`Self::gap_wait`])
        if l.req.hub_build() && l.req.node.as_deref() == Some(name) { return None; }
        if owner.req.kind == Kind::Timed && !self.timed_due(owner) && self.interactive_boxes.contains_key(name) { return None; }
        // A shared/whole reservation cannot split a timed batch after a long running
        // benchmark outlives the next timed request's reservation window.
        // Only a timed request ahead passes a reservation: exclusive work is served in its order
        // (a bumped request first, then by expected end, [`Self::exclusive_place`]), and a
        // reservation of a timed request behind it, taken after its window ran out, never hands
        // that one the box first.
        if l.req.kind == Kind::Timed && (owner.req.kind == Kind::Timed || owner.priority != Priority::Interactive) && self.exclusive_order(l) < self.exclusive_order(owner) { return None; }
        if !l.req.on_node() { return None; }
        // A landing's gate outranks a tests shard: every landing's verdict waits on the gate, and
        // the shard gates nothing. The shard's hold of the gate's best box kept the gate from
        // holding any box (no transitive holds), and the gate waited 14 min at the head of the
        // queue while smaller lane asks took each core the boxes freed (11-07). The gate takes the
        // held box's cores first and may hold it in the shard's place.
        if owner.req.tests_shard() && l.req.landing_gate() { return None; }
        let c = self.nodes[name].caps.as_ref()?;
        let reserved = self.cost_on(name, owner);
        // beside the owner is in the room its hold keeps: what came in beside it since it asked
        // (what ran before is what it waits for), this lease and its reservation all fit. Counted
        // against the owner alone, each lease that fit beside it came in on top of the last, and a
        // landing gate held a box at "node busy" behind four leases its hold had let in, more
        // cores charged beside its own than the box had (11-07).
        // Once a landing gate starts on what its box has free ([`GATE_RESIZE_MS`]), the box drains
        // for it: what runs there counts, whenever it came in, so newer work never takes the cores
        // it waits for. Live sessions and lane tests let in beside an 8-core gate's room kept 13 of
        // a box's 16 in use behind a tests shard, every hand-in behind the gate 15 min (L657290, 11-08).
        // A tests shard's box, and a box's prebuild of the landed tip, drain of background work from
        // the start, by the RAM and cores they are charged: the regression bucket and the JIT every
        // lane's next lease there needs outrank a lane's background work. Two shards waited 16 min
        // for 4 cores each while lane rigs came in beside their room and the box's older work ran on
        // (L662711/12), and a pool box's prebuild 11.5 min for its RAM while background asks took
        // what the box freed (11-08). A tests shard's box drains of interactive work too, and takes
        // in beside it only what fits beside all of the box's work or its class measured to end
        // before the shard starts: a day's shards waited 5 min at their median and 16 at p90, 64 of
        // 158 starting over 2 min after the work on their box from before their ask had left their
        // room, with interactive lane tests and sessions let in since, and the bucket's reds came 20 min after
        // their landing (L794661, 11-10). What goes ahead of it still does: a landing's gate, the hot
        // bucket, the box's hub build, a live launch, the user's first lanes, an interactive ask's
        // own hold. A prebuild holds as any head does, once the box's work ends by
        // its measure ([`Self::starts_after_ends`]): one that waits on a live session holds nothing.
        // A live launch: what runs there counts against a box's prebuild at once (it runs in the
        // box's next gap), and against all work once it waited a minute ([`Self::starved`]).
        // So does any ask that drains its box ([`Self::drains_box`]: one that waited long), as its
        // queue line says.
        // What ran before it asked and has no end known (past its measure, a live job) stays, when
        // the owner fits beside it: it is no end the owner waits for (one that fits only once it
        // ended waits for it). And an ended lease's charge stays until its cleanup: the cores that
        // cleanup frees go to the owner first, its waiters' turn comes after. Counted as ending,
        // a 4-core run past its measure and each ended lease's charge let a 4-core ask in beside an
        // interactive 9-core one on each end on its 16-core box, and the 9-core ask waited 28 min
        // at the head of its queue (L755962, 11-10).
        let drains = owner.req.landing_gate() && self.clock.saturating_sub(owner.asked_ms) >= GATE_RESIZE_MS
            || owner.req.tests_shard()
            || owner.req.upkeep() && l.priority == Priority::Background
            || self.live_launch(owner) && l.req.upkeep()
            || self.drains_box(owner);
        let since_ask = |r: &Lease| r.granted_ms.is_some_and(|g| g >= owner.asked_ms);
        let add = |sum: &mut Cost, x: Cost| (sum.cores, sum.ram_gb, sum.gpus) = (sum.cores + x.cores, sum.ram_gb + x.ram_gb, sum.gpus + x.gpus);
        let mut stays = Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 0.0, gpus: 0 };
        for r in self.live_on_node(name, &l.id).filter(|r| !since_ask(r) && self.expected_end(r).is_none()) { add(&mut stays, self.charged(r)); }
        let stays_beside = stays.cores + reserved.cores <= c.cores_for(owner.req.kind) as f64 && stays.ram_gb + reserved.ram_gb <= owner.req.ram_budget(c) && (stays.gpus + reserved.gpus) as usize <= c.gpus.len();
        let mut beside = Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 0.0, gpus: 0 };
        for r in self.live_on_node(name, &l.id).filter(|r| drains || since_ask(r) || stays_beside && self.expected_end(r).is_none()) { add(&mut beside, self.charged(r)); }
        for (_, _, charged, _) in self.restoring.values().filter(|(n, ..)| n == name) { add(&mut beside, *charged); }
        if !l.req.conflicts_on(&owner.req, c) && beside.cores + cost.cores + reserved.cores <= c.cores_for(l.req.kind) as f64 && beside.ram_gb + cost.ram_gb + reserved.ram_gb <= l.req.ram_budget(c) && (beside.gpus + cost.gpus + reserved.gpus) as usize <= c.gpus.len() && cost.disk_gb + reserved.disk_gb <= c.disk_free_gb - c.disk_floor_gb {
            return None;
        }
        // Backfill: shared work that could start now and is expected to end before the owner
        // could start anyway (its running blockers' estimates) never delays it. A live session
        // stays up until parked, whatever its estimate: it never backfills, and an exclusive
        // owner's wait leaves out the sessions it parks. Counted at a session's 30 min, small asks
        // came in one after another, each a lease that is not a session, and the session never
        // parked for the whole ask that waited on it (11-08).
        // Only against ends someone knows: a run past its estimate (or a live session its owner does
        // not park) is counted as starting over ([`Self::remaining_after`]), and each time a run
        // passed its guess, work came in on a fresh one. So is a cleanup past its lease's end and
        // the box's teardown measure ([`Self::cleanup_end`]): counted from now, its window opened
        // again at each look while the node did not confirm it.
        // A box that drains for its owner takes backfill by a measure only ([`Self::measured_run`]):
        // short asks that said 1 min and ran 3 came in against a launch's last minute, and it
        // started 2 min after its box's work had ended.
        if !l.req.exclusive() && !self.is_live_job(l) && self.resources(name, l, cost, true).is_ok() {
            let parks = self.drains_box(owner) && !self.live_launch(owner);
            let known = self.live_on_node(name, &owner.id).filter(|r| !(parks && self.is_live_job(r) && Self::parks_for(r, owner))).all(|r| self.expected_end(r).is_some())
                && self.restoring.values().filter(|(n, ..)| n == name).all(|(.., ended)| self.cleanup_end(name, *ended).is_some());
            let wait = self.remaining_after(name, owner, &reserved, !parks);
            let finish = if self.drains_box(owner) || owner.req.tests_shard() { self.measured_run(l).map(|run| self.build_ms(name, l).saturating_add(run)) } else { Some(self.finish_ms(name, l, 0)) };
            if known && wait > 0 && finish.is_some_and(|f| f <= wait) { return None; }
        }
        if owner.req.kind == Kind::Timed {
            return Some(Block::by(format!("held for timed {} (the box drains for it: only work that ends first or fits beside it)", h.id), &h.id));
        }
        if owed_leg(owner) {
            return Some(Block::by(format!("held for owed {} (its run is owed a box: only work that ends first or fits beside it)", h.id), &h.id));
        }
        // newer work filling the box beside what runs kept its hub build waiting on and on
        if owner.req.hub_build() {
            let what = if owner.req.deploy_build() { "its deployment's build" } else { "its hub's build" };
            return Some(Block::by(format!("held for {} ({what}: only work that ends first or fits beside it)", h.id), &h.id));
        }
        if owner.req.landing_gate() {
            return Some(Block::by(format!("held for landing gate {} (every landing waits behind it: only work that ends first or fits beside it)", h.id), &h.id));
        }
        if owner.req.tests_shard() {
            return Some(Block::by(format!("held for tests shard {} (the post-land tests bucket: only work that ends first or fits beside it)", h.id), &h.id));
        }
        if owner.req.upkeep() {
            if l.priority == Priority::Interactive { return None; }
            return Some(Block::by(format!("held for {} (the box's prebuild of the landed tip: only work that ends first or fits beside it)", h.id), &h.id));
        }
        // A background whole-box ask holds interactive work only once it could take the box: a live
        // session waited behind a CI reservation that a 20 min run kept off the box anyway (11-04).
        // A session let in parks for it later ([`Self::park_live`]). A CI ask holds no lane work
        // either: CI gates nothing, and a shard that gets no box gives up and goes behind the rest
        // (a lane's build waited behind a CI ask that was waiting for the lane's own screen).
        // ([`Self::long_overdue`]: past that, interactive work waits for it as well, but a landing's gate)
        let whole = owner.req.whole || self.starved(owner);
        let holds_interactive = self.long_overdue(owner) && !l.req.landing_gate();
        if whole && !holds_interactive && owner.priority != Priority::Interactive && (l.priority == Priority::Interactive || (owner.req.kind == Kind::Ci && l.req.kind != Kind::Ci)) && self.remaining_ms(name, owner, &reserved) > 0 { return None; }
        // a background ask's hold keeps background work off, never interactive work
        if owner.priority != Priority::Interactive && !whole && l.priority == Priority::Interactive { return None; }
        let what = if owner.req.whole { "whole ".to_string() } else if owner.priority == Priority::Interactive { "interactive ".to_string() } else { String::new() };
        let queued = self.clock.saturating_sub(owner.asked_ms);
        let long = if !self.starved(owner) {
            String::new()
        } else if queued < STARVED_MS && self.aged(owner) {
            format!("; its start passed {ETA_MISSES} times: the box drains for it")
        } else {
            format!("; it has queued {}: the box drains for it", dur(queued))
        };
        Some(Block::by(format!("held for {what}{} (it starts as the box's work ends by its measure: only work that ends first or fits beside it){long}", h.id), &h.id))
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

    /// Work that runs only in a gap of a box's timed queue: the box's own prebuild (its JIT for
    /// the newest landed tip, which a box pinned for its queued timed asks does not run yet) and
    /// devhub's own timed checks ([`Req::own_timed`]) wait, holding nothing, while a lane's timed
    /// ask waits for the box (it names it or could take it). A prebuild at each landing headed
    /// the bench box's timed queue, and as an interactive ask held its timed asks "for
    /// interactive" 3 min each; jitlegs' warm leg, asked first, ran its 6 min ahead of lanes'
    /// timed asks: one waited 14 min behind both (11-07).
    fn gap_wait(&self, name: &str, l: &Lease) -> Option<Block> {
        if !(l.req.upkeep() || l.req.own_timed()) { return None; }
        if self.prebuild_owed(name).is_some_and(|p| p.id == l.id) { return None; }
        let c = self.nodes.get(name)?.caps.as_ref()?;
        // a lane's ask that waits behind this own ask's catch-up ([`Self::deploy_wait`]) leaves it
        // no gap to wait for: each would wait for the other
        let head = self.deploy_head(name).filter(|h| h.id == l.id).map(|h| (h.asked_ms, h.seq));
        let r = self.leases.iter().find(|r| r.id != l.id && r.state == State::Queued && r.req.kind == Kind::Timed && !r.req.own_timed() && !self.denying.contains(&r.id)
            && head.is_none_or(|h| (r.asked_ms, r.seq) < h)
            && self.timed_could_take(name, c, r))?;
        Some(Block::by(format!("in a gap of {name}'s timed queue: {} of {} waits for the box", r.id, r.req.lane), &r.id))
    }

    /// Box `name`'s prebuild once it has waited past its own run in gaps of the box's timed queue
    /// ([`Self::gap_wait`]): the gap is made. It yields no more, and the box's next timed grant
    /// waits for it, as one waits behind a catch-up ([`Self::deploy_wait`]); a running timed lease
    /// runs on. Only while it could start once the box's running work ends: one that cannot run
    /// there for another reason holds no timed work. A bench box's queue ran back to back, its
    /// prebuild (measured 2m12s) waited 1st in queue 10 min and more, and each timed run there
    /// built its own JIT (L777262, 11-10).
    fn prebuild_owed(&self, name: &str) -> Option<&Lease> {
        self.leases.iter().find(|r| r.state == State::Queued && r.req.upkeep() && r.req.node.as_deref() == Some(name) && !self.denying.contains(&r.id)
            && self.clock.saturating_sub(r.asked_ms) > self.run_ms(r)
            && self.ready_as(name, r, &self.cost_on(name, r), false, false).is_ok())
    }

    /// A timed ask on a box whose prebuild is owed its gap ([`Self::prebuild_owed`]) starts after it.
    fn prebuild_wait(&self, name: &str, l: &Lease) -> Option<Block> {
        if l.req.kind != Kind::Timed { return None; }
        let p = self.prebuild_owed(name)?;
        Some(Block::by(format!("behind {name}'s prebuild: {} waited {}, past its run's {}: it takes the next gap, timed work starts after it", p.id, dur(self.clock.saturating_sub(p.asked_ms)), dur(self.run_ms(p))), &p.id))
    }

    fn now_fits_as(&self, name: &str, l: &Lease, cost: &Cost, holds: &Holds, past_cap: bool) -> Result<f64, Block> {
        self.ready_as(name, l, cost, past_cap, false)?;
        if let Some(b) = self.timed_wait(name, l).or_else(|| self.spare_wait(name, l, holds)) { return Err(b); }
        if let Some(b) = self.deploy_wait(name, l).or_else(|| self.prebuild_wait(name, l)).or_else(|| self.gap_wait(name, l)).or_else(|| self.hot_wait(name, l, holds)).or_else(|| self.held_by(name, l, cost, holds)) { return Err(b); }
        self.resources(name, l, cost, true)?;
        let idle =self.nodes[name].released_ms.map_or(3_600_000, |r| self.clock.saturating_sub(r).min(3_600_000));
        Ok(idle as f64 / 36_000_000.0)
    }

    pub(super) fn candidates<'a>(&'a self, l: &Lease) -> Result<Vec<&'a String>, Place> {
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
        self.can_wait(name, l, holds)
    }

    /// A timed request, an owed leg, a box's hub build, a land queue's gate and a tests bucket's
    /// shard hold the box they wait for even when its running work, ended by its measure, would
    /// not leave them room ([`Self::can_wait`]). A gate heading the queue waited 725 s while lane
    /// work took the cores its box freed, and every landing waited behind it (11-06).
    fn holds_while_waiting(l: &Lease) -> bool {
        l.req.kind == Kind::Timed || owed_leg(l) || l.req.hub_build() || l.req.landing_gate() || l.req.tests_shard()
    }

    /// A reservation's end: renewed each tick its ask waits, so it lasts while the ask waits. The
    /// box drains for it, taking only work that fits beside it or ends first. One that expired
    /// after ten minutes let new leases in and the box never drained (workpool waited 23 min on a
    /// pool box while it granted six, 11-02); an interactive ask's expired and never came back,
    /// and smaller asks took every core its box freed. Interactive demand still comes first
    /// ([`Self::timed_wait`]).
    fn hold_end(&self) -> u64 {
        self.clock + HOLD_MS
    }

    /// A tests shard, a landing gate or a whole-box ask holds the box it waits for whatever holds
    /// another box against it: it may take any pool box, so one hold elsewhere (a hub build, a
    /// gate, after every landing) took its reservation everywhere, and lane work took the box it
    /// waited for. A shard held nothing while a box's hub build waited, and the box it waited for
    /// went to three lane leases (11-07). A lane's whole-box ask, first in its queue, held no box
    /// whenever a shard or a gate held another, and lane asks took each box as it freed: 37 min
    /// (11-08). (Timed work holds by its own rules, a bench box's quiet window behind interactive
    /// demand; a CI ask holds no lane work.)
    /// What its hold keeps off holds nothing in turn ([`Self::can_wait`]): no chain grows from it.
    fn holds_past_other_holds(l: &Lease) -> bool {
        l.req.tests_shard() || l.req.landing_gate() || l.req.whole && !matches!(l.req.kind, Kind::Timed | Kind::Ci)
    }

    /// Whether `l` holds `name` while it waits: any ask at the head of the box's queue that starts
    /// once the box's running work ends by its measure ([`Self::starts_after_ends`]), whatever its
    /// kind and priority, so the wait is at most one lease's length: only work that ends first or
    /// fits beside it gets in ([`Self::held_by`]). Holding only timed, interactive and the land
    /// queue's asks, a background ask that needed more than a box ever freed at once waited
    /// while smaller asks took each core as it freed.
    fn can_wait(&self, name: &str, l: &Lease, holds: &Holds) -> bool {
        if l.state != State::Queued { return false; }
        if self.timed_wait(name, l).is_some() || self.spare_wait(name, l, holds).is_some() || self.deploy_wait(name, l).is_some() || self.prebuild_wait(name, l).is_some() || self.gap_wait(name, l).is_some() { return false; }
        let Ok(names) = self.candidates(l) else { return false };
        names.iter().any(|n| n.as_str() == name)
            && ((Self::holds_past_other_holds(l) || self.starved(l)) && self.held_by(name, l, &self.cost_on(name, l), holds).is_none() || names.iter().all(|n| self.held_by(n, l, &self.cost_on(n, l), holds).is_none()))
            && self.ready_to_hold(name, l, &self.cost_on(name, l)).is_ok()
            && self.hot_wait(name, l, holds).is_none()
            && (self.drains_box(l) || Self::holds_while_waiting(l) || self.starts_after_ends(name, l))
    }

    /// An ask that has waited [`STARVED_MS`] (a timed or whole one has its own rules; CI and a
    /// box's background upkeep gate nobody): it holds the box whose work ends first by its
    /// measure, whatever holds another box against it, the box's live sessions park for it, and
    /// its queue line says when it starts ([`Self::drains_box`]). A 16-core lane test headed its
    /// queue 46 min: a live session on its box had no end, so it held nothing, and lane asks took
    /// each core the box freed (L661568, 11-08).
    /// An interactive live launch drains its box a minute in ([`LIVE_STARVED_MS`]): one asking 13
    /// cores waited 11 min while background lane work and every box's prebuild took the cores the
    /// boxes freed (11-08). So does a head whose start passed twice ([`Self::aged`]).
    fn starved(&self, l: &Lease) -> bool {
        let after = if self.live_launch(l) { LIVE_STARVED_MS } else { STARVED_MS };
        Self::ages(l) && (self.clock.saturating_sub(l.asked_ms) >= after || self.aged(l))
    }

    /// An ask that waited [`OVERDUE_MS`]: age now counts past the line between background and
    /// interactive work. It ranks with interactive asks by age, and its hold keeps newer ones off
    /// its box unless they end first or fit beside it. A starved background ask still gives way
    /// to each of them, so with interactive work always arriving it never had the cores: a 12-core
    /// test waited 83 min at the head of its queue and an 8-core build 89 min, with six leases held
    /// behind it, while its box granted some 30 interactive leases (L716027, L715925, 11-09).
    /// A landing's gate, the hot bucket, a live launch and the user's first lanes still go ahead.
    fn long_overdue(&self, l: &Lease) -> bool {
        Self::ages(l) && self.clock.saturating_sub(l.asked_ms) >= OVERDUE_MS
    }

    /// An ask that drains its box once it waited long ([`Self::starved`]): a timed or whole one
    /// has its own rules, CI and a box's background upkeep gate nobody.
    fn ages(l: &Lease) -> bool {
        l.state == State::Queued && !l.req.exclusive() && l.req.on_node() && l.req.kind != Kind::Ci && !l.req.upkeep()
    }

    /// A head whose start, as its box's work said it at its hold, passed [`ETA_MISSES`] times while
    /// it waited: the box drains for it ([`Self::starved`]). The estimates it was told were wrong
    /// (work let in "ending first" ran past them, or the box's measure kept it off), and the
    /// next one need not be any better.
    fn aged(&self, l: &Lease) -> bool {
        self.promised.get(&l.id).is_some_and(|p| p.misses >= ETA_MISSES)
    }

    /// Each round, for each box's head as the round began ([`Self::held_before`]): its start by
    /// the box's work ([`Self::remaining_ms`]). A start that passed [`ETA_GRACE_MS`] ago while it
    /// still waits is missed, and its start is told again from now; an earlier one replaces it.
    /// A launch asking 15 of a box's 16 cores was told 4m46s, then 3m56s, 8m00s, 16m22s and
    /// 14m09s, and never started (W4322, 11-08).
    fn age_heads(&mut self, now: u64) {
        let heads: Vec<(String, u64)> = self.held_before.iter().filter_map(|(name, h)| {
            let l = self.leases.iter().find(|l| l.id == h.id && Self::ages(l))?;
            self.nodes.get(name)?.caps.as_ref()?;
            Some((h.id.clone(), self.remaining_ms(name, l, &self.cost_on(name, l))))
        }).collect();
        let queued: HashSet<&String> = self.leases.iter().filter(|l| l.state == State::Queued).map(|l| &l.id).collect();
        self.promised.retain(|id, _| queued.contains(id));
        for (id, eta) in heads {
            let p = self.promised.entry(id).or_insert(Promise { at: now.saturating_add(eta), misses: 0 });
            if now > p.at.saturating_add(ETA_GRACE_MS) {
                p.misses += 1;
                p.at = now.saturating_add(eta);
            } else {
                p.at = p.at.min(now.saturating_add(eta));
            }
        }
    }

    /// A live session's launch someone waits on: queued, interactive.
    fn live_launch(&self, l: &Lease) -> bool {
        l.state == State::Queued && l.req.live && l.priority == Priority::Interactive
    }

    /// A box whose run scopes weigh CPU ([`super::Priority::cpu_weight`]).
    fn weighs_cpu(c: &Caps) -> bool {
        c.os == "linux"
    }

    /// Work a live launch or call goes ahead of on a box that weighs CPU, whatever it is charged:
    /// a background shared run (1 a core against the call's 100) whose verdict no landing and no
    /// timing waits on. Timed and GPU runs, interactive work, gates and owed legs still count,
    /// and so does everyone's RAM. A box charged full by work using half of it kept launches
    /// queued 7 min at the median, and those launches held 75% of what queued behind holds
    /// (one reading, 11-09); they use their peak for 15-141 s.
    fn yields_to_live(&self, r: &Lease) -> bool {
        r.priority == Priority::Background && r.req.kind.shared() && r.req.kind != Kind::Gate && !owed_leg(r) && !self.is_live_job(r)
    }

    /// Whether `l` is a live launch that goes ahead of `name`'s background work
    /// ([`Self::yields_to_live`]).
    fn ahead_of_background(&self, name: &str, l: &Lease) -> bool {
        self.live_launch(l) && self.nodes.get(name).and_then(|n| n.caps.as_ref()).is_some_and(Self::weighs_cpu)
    }

    /// How far past its box's cores the charges there stand with this live launch or call running:
    /// the work beside it past them yields ([`Self::yields_to_live`]). 0 for an idle session.
    fn past_budget(&self, l: &Lease) -> f64 {
        if !self.is_live_job(l) || self.live_idle.contains(&l.id) { return 0.0; }
        let Some(name) = l.node.as_deref() else { return 0.0 };
        let Some(c) = self.nodes.get(name).and_then(|n| n.caps.as_ref()).filter(|c| Self::weighs_cpu(c)) else { return 0.0 };
        (self.charged_on(name).cores - c.cores_for(l.req.kind) as f64).max(0.0)
    }

    /// [`Self::past_budget`] for a grant's or a call's line, empty when it is none.
    pub(super) fn past_budget_said(&self, id: &str) -> String {
        let Some(l) = self.get(id) else { return String::new() };
        let past = self.past_budget(l);
        if past <= 0.0 { return String::new(); }
        format!("; past the box's budget by {past:.0} cores: background work there yields by CPU weight")
    }

    /// Whether `l` drains the box it waits for: only work that ends first or fits beside it gets
    /// in, and its live sessions park for it (not for a live launch, [`Self::park_live`]; a visible
    /// one by [`Self::parks_for`]). A timed or whole-box ask, or one that waited long
    /// ([`Self::starved`]).
    fn drains_box(&self, l: &Lease) -> bool {
        l.req.exclusive() || self.starved(l)
    }

    /// Whether the live session `r` parks for `l`. A visible session has someone looking at it, and
    /// `live again` starts it over, without what they were looking at: only work that cannot run
    /// beside it parks it (timed, whole-box). It ends by itself 10 min after its last use, so what
    /// waits for its RAM has it then at the latest. A land gate parked a lane's Fab between looks
    /// for 18.8 GB and a 4 min run, a lane's test another for 6 GB, and asks queued 10 min four more
    /// in two hours (8 of 94 visible sessions in 48 h, 11-09).
    fn parks_for(r: &Lease, l: &Lease) -> bool {
        l.req.exclusive() || r.req.kind != Kind::Screen
    }

    /// Whether `l` could start on `name` once the running leases there with an expected end
    /// ([`Self::expected_end`]) have ended; those without one stay. Shared work that could not
    /// holds no box: other work that fits goes on meanwhile. Prebuilds blocked by a Mac's RAM
    /// budget, beside a lease past its measure, held the Mac 10 min at a time and every lease
    /// waited behind them (64 lease-min in an hour, 11-04). An exclusive request still holds (the
    /// box drains for it and parks its live jobs), and so does an owed leg (it is owed the box).
    /// A head that held the box at the last look (lane work, not a box's upkeep) keeps it through
    /// a run there past its estimate: that run ends, a live session only when parked. Dropped
    /// when short work there ran past its estimate, a 15-core ask held nothing, and short asks
    /// took each core the box freed until it had queued [`STARVED_MS`] (W4322, 11-08). It still
    /// moves to a box that starts it sooner ([`Self::hold_kept`]).
    fn starts_after_ends(&self, name: &str, l: &Lease) -> bool {
        if self.nodes[name].caps.is_none() { return true; }
        let cost = self.cost_on(name, l);
        let held = !l.req.upkeep() && self.held_before.get(name).is_some_and(|h| h.id == l.id);
        let ahead = self.ahead_of_background(name, l);
        let removed: Vec<String> = self.live_on_node(name, &l.id).filter(|r| self.expected_end(r).is_some() || held && !self.is_live_job(r) || ahead && self.yields_to_live(r)).map(|r| r.id.clone()).collect();
        let used = self.in_use(name, l, true, &removed);
        self.fits(name, l, &used, &cost, self.room(name, l, &cost, true, &removed, true)).is_ok()
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

    /// In how many ms the cleanup of a lease that ended on `name` at `ended` ends: its end and the
    /// box's teardown measure. None: past it (the node has not confirmed it).
    fn cleanup_end(&self, name: &str, ended: u64) -> Option<u64> {
        let teardown = Lat::median(&self.nodes[name].lat.teardown).unwrap_or(1_000);
        let gone = self.clock.saturating_sub(ended);
        (gone < teardown).then(|| teardown - gone)
    }

    /// The same best waiting box as placement, even after its reservation window expired.
    fn waiting_box(&self, l: &Lease, holds: &Holds, reserve: bool) -> Option<String> {
        self.waiting_box_among(l, holds, reserve, self.candidates(l).ok()?)
    }

    /// Select a reservation only among the boxes placement can use.
    /// The box it held at the last look stays its box while it can hold it, unless another starts
    /// it sooner by [`HOLD_MOVE_MS`] ([`Self::hold_kept`]).
    fn waiting_box_among(&self, l: &Lease, holds: &Holds, reserve: bool, names: Vec<&String>) -> Option<String> {
        // (a landing gate takes a box a tests shard holds: [`Self::held_by`])
        let boxes: Vec<&String> = names.into_iter().filter(|name| holds.get(name.as_str()).is_none_or(|h| h.id == l.id || (l.req.landing_gate() && self.get(&h.id).is_some_and(|r| r.req.tests_shard())))
            && if reserve { self.can_hold(name, l, holds) } else { self.can_wait(name, l, holds) }).filter(|name| {
            let mut next = holds.clone();
            next.retain(|_, h| h.id != l.id);
            next.insert((*name).clone(), Hold { id: l.id.clone(), until: self.hold_end() });
            next.iter().all(|(n, h)| h.id == l.id || self.get(&h.id).is_some_and(|r| self.can_hold(n, r, &next)))
        }).collect();
        let key = |name: &String| (l.req.kind == Kind::Timed && self.has_live(name), self.remaining_ms(name, l, &self.cost_on(name, l)), self.sticky.get(&l.req.lane) != Some(name));
        let best = boxes.iter().copied().min_by_key(|name| key(name))?;
        let held = boxes.iter().copied().find(|name| self.held_before.get(name.as_str()).is_some_and(|h| h.id == l.id));
        Some(match held {
            Some(held) if Self::hold_kept(key(held), key(best)) => held.clone(),
            _ => best.clone(),
        })
    }

    /// A live session whose call runs past [`LIVE_OVERRUN_TIMES`] its class's measured run, and
    /// [`STALL_SLACK_MS`] over it, says so once per call: an inbox warning naming its lane and
    /// agent, its why-hung sample in its kept log ([`Action::Capture`]: `devhub job result <id>`),
    /// and its owner told ([`Self::owner_word`]: the live test's still-running line). Only said:
    /// the session is its lane's, and a capture ends no lane's lease. [`Self::say_stalled`] leaves
    /// live jobs out, and the idle end waits for the call: a test session sat 2.3 h at its first
    /// verdict, holding 4.5 GB, against its measured 16 min, and nothing said so (W4652, 11-10).
    /// A class with no measure is held to nothing: an open session has no length of its own.
    fn say_live_overruns(&mut self, now: u64) {
        let said: Vec<(String, String, u64, String, String)> = self.leases.iter().filter(|l| l.state == State::Granted && self.is_live_job(l) && !self.live_idle.contains(&l.id)).filter_map(|l| {
            let node = l.node.as_deref()?;
            let measured = self.measured_ms(l).filter(|_| !super::lane_script(l.req.class()))?;
            // (its box's JIT build is judged by what it does: [`Self::say_stalled`])
            let jit = l.node_phases.0.iter().rev().find(|s| s.phase == "jit");
            if jit.is_some_and(|s| s.end == 0) { return None; }
            let since = match self.live_call_ms.get(&l.id) { Some(t) => *t, None => jit.map_or(l.granted_ms?, |s| s.end) };
            if self.live_overran.get(&l.id).is_some_and(|(at, _)| *at == since) { return None; }
            let (ran, m) = (now.saturating_sub(since), measured.max(60_000));
            if ran <= m.saturating_mul(LIVE_OVERRUN_TIMES).max(m.saturating_add(STALL_SLACK_MS)) { return None; }
            let agent = l.req.agent.as_deref().map_or(String::new(), |a| format!(" ({a})"));
            let word = format!("{} of {}{agent} on {node}: its live call has run {} against its class's measured {} ({}), over {LIVE_OVERRUN_TIMES}x: hung? its why-hung sample: `devhub job result {}`; devhub does not end it (`devhub cancel {}` frees the box)", l.tag(), l.req.lane, dur(ran), dur(measured), l.req.class(), l.id, l.id);
            let why = format!("{} of {} running a live call {} against its measured {}", l.tag(), l.req.lane, dur(ran), dur(measured));
            Some((l.id.clone(), node.to_string(), since, word, why))
        }).collect();
        for (id, node, since, word, why) in said {
            let l = self.get(&id).cloned();
            self.event(Sev::Warn, word.clone(), l.as_ref());
            self.out.push(Action::Capture { id: id.clone(), node, why });
            self.live_overran.insert(id, (since, word));
        }
    }

    /// A live session's call began after an idle spell ([`Self::say_live_overruns`] times it).
    pub fn live_call_began(&mut self, id: &str, now: u64) {
        self.live_call_ms.insert(id.to_string(), now);
    }

    /// A live session's call ended: what was said of it is over, and its next call is charged
    /// afresh.
    pub fn live_call_ended(&mut self, id: &str) {
        self.live_overran.remove(id);
        self.live_call_charge.remove(id);
    }

    /// An interactive live call starts with `cores` ([`Self::call_start_cores`]): its charge
    /// while it runs.
    pub fn live_call_charged(&mut self, id: &str, cores: f64) {
        self.live_call_charge.insert(id.to_string(), cores);
    }

    /// A held box (`held`: its key in [`Self::waiting_box_among`]) stays held against the best
    /// other (`best`) unless that one is preferred for another reason than its start, or starts it
    /// sooner by [`HOLD_MOVE_MS`]: estimates move as work starts and ends, and a hold that follows
    /// each move drains no box.
    fn hold_kept(held: (bool, u64, bool), best: (bool, u64, bool)) -> bool {
        held.0 == best.0 && held.1 <= best.1.saturating_add(HOLD_MOVE_MS)
    }

    /// A live session runs on the box (it stays up until parked: timed work waits elsewhere first).
    fn has_live(&self, name: &str) -> bool {
        self.leases.iter().any(|r| r.state.live() && r.node.as_deref() == Some(name) && self.is_live_job(r))
    }

    /// A lease a live job holds: flagged in its request, or known to the live subsystem.
    /// The running leases past `times` their measure (what each declared, else its class's measured
    /// run, at least 1 min, as [`Self::say_stalled`] holds them to): each with how long it ran, its
    /// measure and what that is. Open-ended sessions, hub builds and unmeasured classes have no
    /// measure to be over.
    pub fn overdue(&self, now: u64, times: u64) -> Vec<(Lease, u64, u64, String)> {
        self.leases.iter().filter(|l| l.state == State::Granted && !self.is_live_job(l) && !l.req.hub_build() && !self.session(&l.req) && l.req.kind != Kind::Inspect).filter_map(|l| {
            let (measure, what) = match (l.req.declared_ms(), self.measured_ms(l).filter(|_| !super::lane_script(l.req.class()))) {
                (Some(d), _) => (d.saturating_add(self.runner_build_ms(l)), self.held_to_said(l, &format!("its declared {}", dur(d)))),
                (None, Some(m)) => (m, format!("its measured {}", dur(m))),
                (None, None) => return None,
            };
            let ran = now.saturating_sub(l.granted_ms?);
            (ran > times.saturating_mul(measure.max(60_000))).then(|| (l.clone(), ran, measure, what))
        }).collect()
    }

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
        self.get(id).filter(|l| l.state == State::Granted)?;
        self.hung_word(id).or_else(|| self.yielded.get(id).cloned()).or_else(|| self.live_overran.get(id).map(|(_, word)| word.clone()))
    }

    /// The idle verdict on a running lease: every process of it idle ([`Self::quiet_ms`]).
    pub fn hung_word(&self, id: &str) -> Option<String> {
        let l = self.get(id).filter(|l| l.state == State::Granted)?;
        self.quiet_ms(l).and_then(|q| Some(idle_line(l, l.node.as_deref()?, q)))
    }

    /// A holder hands its running lease to the master's own work (a job that follows its run,
    /// orch/job.rs `adopt`): its end no longer ends the lease, its follower's beats keep it, and a
    /// restarted master's job takes it back. Only its holder hands it over; its box and grant.
    pub fn adopt(&mut self, id: &str, pid: u32) -> Result<(String, u64), String> {
        let l = self.leases.iter_mut().find(|l| l.id == id && l.state == State::Granted).ok_or_else(|| format!("{id} is not running"))?;
        if l.req.holder != Some(pid) { return Err(format!("{id} is not held by pid {pid}")); }
        let node = l.node.clone().ok_or_else(|| format!("{id} has no box"))?;
        l.req.holder = None;
        l.req.master = true;
        Ok((node, l.granted_ms.unwrap_or(self.clock)))
    }

    /// A live launch parks no other session: one waiting a minute would park another lane's.
    fn park_live(&self, l: &Lease) -> Option<Action> {
        if !self.drains_box(l) || self.live_launch(l) || self.clock.saturating_sub(l.asked_ms) < LIVE_PARK_MS { return None; }
        let name = self.waiting_box(l, &self.holds, false)?;
        let cost = self.cost_on(&name, l);
        let mut leases = Vec::new();
        // why-queued reports the first failed condition. Removing it may reveal another
        // conflict or budget blocker; all of them must be live jobs on this box.
        while let Err(b) = self.resources_without(&name, l, &cost, true, &leases, true) {
            if b.blockers.is_empty() || b.blockers.iter().any(|id| self.get(id).is_none_or(|r| !r.state.live() || !self.is_live_job(r) || r.node.as_deref() != Some(name.as_str()) || !Self::parks_for(r, l)) || leases.contains(id)) { return None; }
            leases.extend(b.blockers);
        }
        let what = if l.req.kind == Kind::Timed { "timed".to_string() } else if l.req.whole { "whole".to_string() } else { format!("{} queued", dur(self.clock.saturating_sub(l.asked_ms))) };
        (!leases.is_empty()).then(|| Action::ParkLive { leases, note: format!("parked for {what} {}", l.id) })
    }

    /// A queued ask that waits for RAM idle live sessions hold on its box parks them: the longest
    /// quiet first, only as many as make it fit, none when all of them would not. Idle is a
    /// state, never a timer: no call or edit in flight ([`Self::live_idle`]) and its node saw its
    /// runs do nothing across a look (`quiet_s`). A later ask counts the RAM of the sessions
    /// already parking as free, so the room is made once. One lane's sessions sat 57-73 min
    /// between calls on a 16 GB Mac while other work waited 134 min for their RAM (11-04).
    /// A JIT warm-up (a box's prebuild, a lane's warm leg) parks none: a session in use is the
    /// work it warms boxes for, and it ends by itself (its lane's release, its idle timeout). A
    /// box's prebuild parked a lane's Fab session on a 16 GB Mac between its calls, twice in one
    /// experiment, and its start again needed that RAM back (11-06).
    fn park_idle_live_for_ram(&mut self) {
        let mut parking = self.parking_ram.clone();
        let mut parks = Vec::new();
        for i in self.queue_order() {
            let l = &self.leases[i];
            if l.state != State::Queued || !l.req.on_node() || l.req.warm_up() { continue; }
            let Ok(names) = self.candidates(l) else { continue };
            // the box where the fewest idle sessions make room (any box that has room already:
            // none parks)
            let mut best: Option<(String, Vec<String>, f64)> = None;
            for name in names {
                let cost = self.cost_on(name, l);
                if self.ready(name, l, &cost).is_err() { continue; }
                let mut removed: Vec<String> = parking.iter().cloned().collect();
                removed.sort();
                // it fits once the sessions already parking end, or waits on more than RAM; the
                // RAM sessions keep for their calls parks none (that ask waits for the work beside
                // them: it fits beside the call, [`Self::call_room`])
                match self.resources_without(name, l, &cost, true, &removed, false) {
                    Ok(()) => { best = None; break; }
                    Err(b) if b.reason.starts_with("budget ram") => {}
                    Err(_) => continue,
                }
                let mut idle: Vec<&Lease> = self.live_on_node(name, &l.id)
                    .filter(|r| self.is_live_job(r) && Self::parks_for(r, l) && self.live_idle.contains(&r.id) && !self.live_waiting.contains(&r.id) && !parking.contains(&r.id) && r.quiet_s.is_some_and(|q| q > 0))
                    .collect();
                idle.sort_by(|a, b| b.quiet_s.cmp(&a.quiet_s).then_with(|| a.id.cmp(&b.id)));
                let mut take = Vec::new();
                for r in idle {
                    take.push(r.id.clone());
                    removed.push(r.id.clone());
                    if self.resources_without(name, l, &cost, true, &removed, false).is_ok() {
                        if best.as_ref().is_none_or(|b| take.len() < b.1.len()) { best = Some((name.clone(), take, cost.ram_gb)); }
                        break;
                    }
                }
            }
            let Some((name, take, asked)) = best else { continue };
            let held: f64 = take.iter().filter_map(|id| self.get(id)).map(|r| self.charged(r).ram_gb).sum();
            // and when it would fit there again: the ask's own run (an ask behind it may come first)
            let note = format!("parked for {} ({} of {}): it waited for {asked:.1} GB of RAM on {name}, and idle sessions held {held:.1} GB; that RAM is free there again when {} ends (it runs ~{})", l.id, l.req.kind.as_str(), l.req.lane, l.id, dur(self.run_ms(l)));
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
                // A job chain asks its next case before releasing the previous one. Only the
                // master's asks carrying the same original place may queue beside that case.
                if l.req.master && o.req.master && l.req.since_ms.is_some() && l.req.since_ms == o.req.since_ms {
                    return Place::Wait(format!("held by {} (timed of {}; the chain's previous case)", o.id, o.req.lane), None, None, o.node.iter().cloned().collect());
                }
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
        // A box whose budget could never hold the class's measured need is no candidate while one
        // that can is: cut to a 30 GB box's budget, a class that peaked at 24 GB would take that
        // box while a 64 GB one is busy, and run it out of memory. A first run (no measure yet)
        // places as before.
        let measured = |name: &str| self.measured_ram(&l.req, Some(name)).filter(|_| l.req.ram_gb.is_none());
        let holds_ram = |name: &str| measured(name).is_none_or(|gb| self.nodes[name].caps.as_ref().is_some_and(|c| gb <= l.req.ram_cap(c)));
        if fit.iter().any(|(name, _)| holds_ram(name)) {
            fit.retain(|(name, _)| holds_ram(name));
        }
        if let Some(why) = self.pinned_past_ram(l, &fit) {
            return Place::Deny(why);
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
        let mut best: Option<(&String, Cost, u64, f64, bool, bool, f64, bool, bool, bool, bool)> = None;
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
                    // (a box its `--jit` is copied to is ready as one that holds it, less the copy)
                    let cold = loader && !self.has_loader(name, l) && self.jit_source(name, l).is_none();
                    // a live session leaves a box a timed request waits for to the last
                    let timed = self.is_live_job(l) && self.leases.iter().any(|r| r.state == State::Queued && r.req.kind == Kind::Timed && (r.req.node.as_deref() == Some(name.as_str()) || holds.get(name.as_str()).is_some_and(|h| h.id == r.id)));
                    // its budget would cut what the class measured: a box that holds it whole first
                    let cut = self.ram_cut(name, l);
                    // a tests bucket run's shards spread: a box beside a sibling (their shared tree)
                    // after every box with room that has none (four took two boxes, 11-07)
                    let sibling = l.req.tests_shard() && self.live_on_node(name, &l.id).any(|r| r.req.same_run(&l.req));
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
                        if sibling != b.10 { return !sibling; }
                        let other_hot = self.nodes[b.0].caps.as_ref().is_some_and(|c| c.role() == Role::Hot) && l.cost_class == CostClass::Small;
                        if cut != b.9 { return !cut; }
                        if cold != b.4 { return !cold; }
                        if on_bench != b.7 { return !on_bench; }
                        if cold { eta < b.2 || eta == b.2 && (hot && !other_hot || hot == other_hot && score > b.3) }
                        else { hot && !other_hot || hot == other_hot && (eta < b.2 || eta == b.2 && score > b.3) }
                    }) {
                        best = Some((name, *cost, eta, score, cold, timed, busy, on_bench, fg_bench, cut, sibling));
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
        // A lane's warm leg reserves none: it starts where its box can start it now, and drained
        // for it, a box took no lane work (three lanes' warm legs held a box behind its tests
        // shard in turn, 11-09)
        let hold = if over.is_some() || l.req.lane_warm() { None } else { self.waiting_box_among(l, holds, true, fit.iter().map(|(name, _)| *name).collect()) };
        let first = why.iter().find(|w| Some(&w.0) == hold.as_ref()).or_else(|| why.iter().find(|w| Some(&w.0) == sticky)).unwrap_or(&why[0]);
        let reason = if fit.len() == 1 { first.1.clone() } else { format!("{}: {} (+{} nodes)", first.0, first.1, fit.len() - 1) };
        // a named box waits; one its needs meet that could start it now is the agent's choice to
        // see (a Mac test named the bench Mac behind a timed lease 13 min while the other was idle,
        // 11-04)
        let reason = match l.req.node.as_deref() {
            Some(named) => {
                let free = self.free_elsewhere(l, holds);
                // past PIN_WAIT_MS its holder is asked to unpin it ([`Self::say_pinned`]); one
                // whose box is its reason says why it waits for it
                let unpin = if self.clock.saturating_sub(l.asked_ms) >= PIN_WAIT_MS && !l.req.devhubs_own() { format!(": unpin it ({} in place of --node), or say why it needs {named}", self.needs_of(named)) } else { String::new() };
                match self.pinned_for(l, named) {
                    Some(why) => format!("{reason}; it waits for {named}, {why}"),
                    None if free.is_empty() => reason,
                    None => format!("{reason}; it named {named} (--node): {} could start it now{unpin}", free.join(" ")),
                }
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
            let landing = l.req.landing_gate();
            let aged = l.req.prio as i64 - (self.clock.saturating_sub(l.asked_ms) / AGE_STEP_MS) as i64;
            // In a quiet window exclusive work (timed and whole-box) drains in its order
            // ([`Self::exclusive_order`]), before any shared work: the head gets the box when it
            // frees, no request behind it overtakes it (a bump goes first). Interactive demand blocks timed placement and
            // holds, never a running lease.
            // past its lane cap: after every lease within one (it takes only what they leave)
            let over = self.lane_block(l).is_some();
            // the lanes the user put first come before every other lane's work and the land queue's
            let later = !self.first_lanes.contains(&l.req.lane);
            if l.req.exclusive() { let o = self.exclusive_order(l); return (!l.req.top(), over, later, l.req.ci(), false, false, false, false, if o.0 { 0 } else { -1 }, u64::from(o.1) << 32 | (o.2 as u64) << 1 | u64::from(o.3), o.4, o.5); }
            // a leg owed a box ranks with interactive work by age: newer interactive work never
            // takes the box it waits for first (a live session got a Mac the moment its hub came
            // back, ahead of a leg owed it for 31 min, 11-03); so does a tests shard: a lane's
            // newer ask went ahead of it by its lane's last grant (its sibling's), and took the box
            // that freed before the one the shard held (11-07)
            // a box's hub build heads its box's queue with the hot bucket's work (it asks for its box only)
            (!(l.req.top() || l.req.hub_build()), over, later, l.req.ci(), true, l.req.kind != Kind::Gate, !landing, l.priority != Priority::Interactive && !owed_leg(l) && !l.req.tests_shard() && !self.long_overdue(l), aged, self.last_grant.get(&l.req.lane).map_or(0, |t| t + 1), l.asked_ms, l.seq)
        });
        order
    }

    /// The boxes other than the one `l` names that could start it now (and could ever take it), of
    /// the named box's platform: what a lane pins a box for is most often its OS (a Linux build, a
    /// CUDA run), and no box's own prebuild or hub build runs elsewhere. The hint named the Windows
    /// box for a CUDA gpu run, a Linux build and two Linux boxes' prebuilds (11-07), and another
    /// box for a Linux node's hub build (11-08).
    fn free_elsewhere(&self, l: &Lease, holds: &Holds) -> Vec<String> {
        let Some(named) = l.req.node.as_deref() else { return Vec::new() };
        if l.req.upkeep() || l.req.hub_build() { return Vec::new(); }
        let platform = self.nodes.get(named).and_then(|n| n.caps.as_ref()).map(|c| (c.os.clone(), c.arch.clone()));
        self.nodes.iter().filter(|(o, n)| o.as_str() != named && n.caps.as_ref().is_some_and(|c| !hot_refuses(c.role(), l) && platform.as_ref().is_none_or(|(os, arch)| c.os == *os && c.arch == *arch)))
            .filter(|(o, _)| self.now_fits(o, l, &self.cost_on(o, l), holds).is_ok()).map(|(o, _)| o.clone()).collect()
    }

    /// A lane's ask that names its box (`--node`) with more RAM (`--ram-gb`) than that box may give
    /// it now beside its residents (a person's apps, its hub; [`Req::ram_budget`]): it would head
    /// the box's queue until they shrink, which may be never. One asked 48 GB of a box whose budget
    /// was 43.8 GB beside 11.9 GB of a person's apps and headed its queue 12 min (L798323, 11-10).
    /// Denied when asked, and later should the residents grow while it waits, saying the box's
    /// budget and the boxes of its platform that hold it now. Only by what lasts there: while
    /// the box's hub builds itself, its deployment applies or its pre-warm runs, their RAM among
    /// the residents passes, and the ask waits as for a busy box. Not devhub's own asks: devhub
    /// moves those itself.
    fn pinned_past_ram(&self, l: &Lease, fit: &[(&String, Cost)]) -> Option<String> {
        let named = l.req.node.as_deref().filter(|_| l.req.ram_gb.is_some())?;
        if l.req.lane_warm() || l.req.hub_build() || l.req.agent.as_deref().is_some_and(|a| a.starts_with("devhub-")) { return None; }
        let (_, cost) = fit.iter().find(|(n, _)| n.as_str() == named)?;
        let c = self.nodes[named].caps.as_ref().filter(|c| c.ram_gb > 0.0 && !(c.hub_building || c.applying || c.prewarming))?;
        let budget = l.req.ram_budget(c);
        if cost.ram_gb <= budget { return None; }
        let holds: Vec<&str> = self.nodes.iter().filter(|(o, n)| o.as_str() != named && n.up).filter_map(|(o, n)| {
            let oc = n.caps.as_ref().filter(|oc| oc.os == c.os && oc.arch == c.arch && !hot_refuses(oc.role(), l))?;
            let cost = self.cost_on(o, l);
            (self.never(o, oc, l, &cost).is_ok() && cost.ram_gb <= l.req.ram_budget(oc)).then_some(o.as_str())
        }).collect();
        let then = match &l.req.series {
            Some(series) => format!("its series {series} runs there: ask for less (--ram-gb)"),
            None if holds.is_empty() => "no other box of its platform holds it now either: ask for less (--ram-gb)".to_string(),
            None => format!("{} {} it now: unpin it ({} in place of --node)", holds.join(" "), if holds.len() == 1 { "holds" } else { "hold" }, self.needs_of(named)),
        };
        Some(format!("needs {:.0} GB RAM (--ram-gb) and {named} (--node) may give {budget:.1} GB now ({}): {then}", cost.ram_gb, l.req.ram_budget_why(c)))
    }

    /// The `--needs` that asks for the named box's platform instead of the box: what an unpinned
    /// ask says when a lane pinned the box only to get its OS (lanes pinned `--node` for Linux and
    /// CUDA, not knowing the form, 11-07).
    fn needs_of(&self, named: &str) -> String {
        self.nodes.get(named).and_then(|n| n.caps.as_ref()).map_or_else(|| "`--needs os=..,arch=..`".to_string(), |c| format!("`--needs os={},arch={}`", c.os, c.arch))
    }

    /// The boxes other than the one it names that could start lane `lane`'s queued lease now (its
    /// reason's "could start it now"); empty: none, or the lane has no such lease.
    pub fn starts_elsewhere(&mut self, lane: &str, now: u64) -> Vec<String> {
        self.clock = now;
        let held = self.holds.clone();
        self.refresh_interactive(&held);
        let holds = self.valid_holds(held);
        self.leases.iter().find(|l| l.state == State::Queued && l.req.lane == lane && l.req.node.is_some()).map_or_else(Vec::new, |l| self.free_elsewhere(l, &holds))
    }

    /// Read-only scheduling explanation: one current failed condition and its blockers per box.
    pub fn why_queued(&mut self, id: &str, now: u64) -> Result<Vec<String>, String> {
        self.clock = now;
        let held = self.holds.clone();
        self.refresh_interactive(&held);
        let holds = self.valid_holds(held);
        let l = self.get(id).ok_or_else(|| format!("no lease {id}"))?;
        if l.state != State::Queued { return Err(format!("{id} is {}", l.state.as_str())); }
        let names = match self.candidates(l) {
            Ok(names) => names,
            Err(Place::Wait(why, _, _, _) | Place::Deny(why)) => return Err(why),
            Err(Place::Grant(_, _)) => unreachable!(),
        };
        // a box the user's cancel keeps the lane off is no candidate, said here: left out, a near
        // empty box read as one the queue head could not fit (L634019, 11-08)
        let kept_off: Vec<(&String, u64)> = if l.req.node.is_some() { Vec::new() } else {
            self.nodes.keys().filter_map(|n| self.excluded.get(&(l.req.lane.clone(), n.clone())).filter(|until| **until > self.clock).map(|until| (n, *until))).collect()
        };
        let line = |name: &str, reason: &str, blockers: &[String]| {
            let cost = self.cost_on(name, l);
            format!("{{\"id\":{},\"node\":{},\"reason\":{},\"blockers\":[{}],\"ram_gb\":{},\"cores\":{},\"allocation\":{}}}", json_str(id), json_str(name), json_str(reason), blockers.iter().map(|id| json_str(id)).collect::<Vec<_>>().join(","), cost.ram_gb, cost.cores, json_str(&self.allocation_note(name, l)))
        };
        let mut out: Vec<String> = names.into_iter().map(|name| {
            let b = self.now_fits(name, l, &self.cost_on(name, l), &holds).err().unwrap_or_else(|| Block::new("fits now"));
            line(name, &b.reason, &b.blockers)
        }).collect();
        for (name, until) in kept_off {
            let why = format!("the user cancelled a {} lease here: the lane stays off it {} min more (--node {name} overrides)", l.req.lane, (until - self.clock).div_ceil(60_000));
            out.push(line(name, &why, &[]));
        }
        Ok(out)
    }

    /// Whether queued `id` is granted `node` once `gone` have ended, and is not now: what ending
    /// background work for it buys. Asked of the placement itself, so another lease in its way, a
    /// need the box lacks, a reservation or a better box all say no.
    pub fn starts_without(&mut self, id: &str, node: &str, gone: &[String], now: u64) -> bool {
        self.clock = now;
        let held = self.holds.clone();
        self.refresh_interactive(&held);
        let holds = self.valid_holds(held);
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

    /// The master's machine slept `slept` ms (its wall clock ran on and its monotonic clock did
    /// not, [`super::master::LeaseMaster`]'s look): no lease is judged on that time. Every silence
    /// the scheduler ends work on moves on by it: a holder's beats ([`BEAT_EXPIRY_MS`]), a run's
    /// output ([`OUTPUT_FRESH_MS`]), a gate's or a screen's idle time, a box's unanswered caps
    /// ([`HOST_GONE_MS`]). The fleet ran on meanwhile, but nobody here could hear it, nor could the
    /// holders on this machine beat. Woken, the master's first tick ended every lease on its
    /// sleep: a lid-closed sleep expired three live sessions on a pool box (no heartbeat for
    /// 10 s), ended a fast bucket's gate as hung (its output grown none for 16m47s) and its bucket
    /// ended ambiguous, then the hubs ended what the master no longer held (11-08).
    /// Real run time is kept: a run's wall, waits and overruns still count the sleep. `now` is the
    /// look that saw the sleep: nothing moves past it, since a beat that came in after the wake and
    /// before that look (a holder's `beat` on the control socket, a hub's caps on its own thread)
    /// has no sleep in it, and moved on by the sleep it would hold a dead holder's lease that much
    /// longer.
    pub fn master_slept(&mut self, slept: u64, now: u64) {
        if slept == 0 { return; }
        let on = |t: u64| t.saturating_add(slept).min(now);
        for l in &mut self.leases {
            l.beat_ms = on(l.beat_ms);
        }
        for n in self.nodes.values_mut() {
            for since in [&mut n.unheard_ms, &mut n.heard_ms, &mut n.gone_ms].into_iter().flatten() {
                *since = on(*since);
            }
        }
        self.move_heard_clocks(slept, now);
        let n = self.leases.len();
        self.event(Sev::Info, format!("the master's machine slept {}: no lease's beats, output or idle time count it ({n} lease(s) kept)", dur(slept)), None);
    }

    /// Moves on by `by` the clocks of what the master hears from its boxes, nothing past `now`: a
    /// run's output and idle time, a box near idle. Time the master could not hear them (asleep,
    /// its network down) is no time they were quiet.
    fn move_heard_clocks(&mut self, by: u64, now: u64) {
        let on = |t: u64| t.saturating_add(by).min(now);
        for l in &mut self.leases {
            if let Some(granted) = l.granted_ms {
                l.out_grew_ms = Some(on(l.out_grew_ms.unwrap_or(granted)));
                // (a screen's look runs from its declared time on, [`Self::end_forgotten_screens`])
                let idle = match l.req.kind {
                    Kind::Screen => l.idle_since.unwrap_or(0).max(granted.saturating_add(l.req.declared_ms().unwrap_or(0))),
                    _ => l.idle_since.unwrap_or(granted),
                };
                l.idle_since = Some(on(idle));
            }
        }
        // (a box near idle since before was not watched near idle through it)
        for since in self.box_idle_since.values_mut() {
            *since = on(*since);
        }
    }

    /// Expires silent holders, places what fits, recomputes positions and reasons; the actions
    /// for the master (grants, ends, reclaims, one event per change).
    pub fn tick(&mut self, now: u64) -> Vec<Action> {
        { let live: HashSet<&String> = self.leases.iter().filter(|l| l.state.live()).map(|l| &l.id).collect(); self.live_idle.retain(|id| live.contains(id)); self.live_called.retain(|id| live.contains(id)); self.live_waiting.retain(|id| live.contains(id)); self.live_call_ms.retain(|id, _| live.contains(id)); self.live_overran.retain(|id, _| live.contains(id)); self.live_call_charge.retain(|id, _| live.contains(id)); self.idle_said.retain(|id| live.contains(id)); self.yielded.retain(|id, _| live.contains(id)); self.pin_said.retain(|id| self.leases.iter().any(|l| l.id == *id && l.state == State::Queued)); self.quiet_said.retain(|id| self.leases.iter().any(|l| l.id == *id && l.state == State::Queued)); self.parking_ram.retain(|id| live.contains(id)); self.stalled.retain(|id, _| live.contains(id)); self.overran.retain(|id, _| live.contains(id)); self.captures.retain(|id, _| live.contains(id)); }
        // (while the master hears no box, what it hears of their runs is old: [`Self::deaf`])
        let deaf = self.deaf_since.is_some();
        if !deaf {
            self.end_hung_legs(now);
            self.say_stalled(now);
            self.say_live_overruns(now);
        }
        self.say_idle_holders(now);
        self.say_overruns(now);
        // Close the previous tick's wait interval once, before placement can grant or end it.
        for l in &mut self.leases { l.sample_queue(now); }
        self.clock = now;
        self.held_before = self.holds.clone();
        self.age_heads(now);
        self.excluded.retain(|_, until| *until > now);
        self.say_pinned(now);
        self.say_never_quiet(now);
        // a holder watched by its pid ([`Sched::holders_gone`]) is not timed by its beats
        let expired: Vec<String> = self.leases.iter().filter(|l| l.req.holder.is_none() && now.saturating_sub(l.beat_ms) > BEAT_EXPIRY_MS).map(|l| l.id.clone()).collect();
        for id in expired {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.keep_dropped_ask(i, now);
                self.end_at(i, "expired", Sev::Warn, "", now);
            }
        }
        // a gate lease holds one of the few gate slots: one that runs nothing gives it back
        let mut idle = Vec::new();
        for l in self.leases.iter_mut().filter(|l| !deaf && l.req.kind == Kind::Gate && l.state.live()) {
            let running = makepad_strict_json::parse(l.info.as_bytes()).ok().and_then(|v| v.get("running").and_then(|r| r.as_str()).map(|r| !r.is_empty())).unwrap_or(false);
            // from the last tick it ran something: a 21 min run counted from its grant was "10 min
            // idle" the moment it ended, and its lease went before its holder could release it (11-05)
            if running {
                l.idle_since = Some(now);
                continue;
            }
            // or from its output's last growth: its steps' lines (STEP, GATEUNIT), between runs
            let since = (*l.idle_since.get_or_insert(l.granted_ms.unwrap_or(now))).max(l.out_grew_ms.unwrap_or(0));
            if now.saturating_sub(since) > GATE_IDLE_MS {
                idle.push(l.id.clone());
            }
        }
        for id in idle {
            if let Some(i) = self.leases.iter().position(|l| l.id == id) {
                self.end_at(i, "idle", Sev::Warn, &format!("nothing ran on this gate lease for {} min, so it gave its gate slot back (landings wait for gate slots); take a new gate lease when the gate is ready to run", GATE_IDLE_MS / 60_000), now);
            }
        }
        if !deaf {
            self.end_forgotten_screens(now);
        }
        for l in self.leases.iter_mut().filter(|l| l.state == State::Queued) {
            l.priority = if self.interactive_lanes.contains(&l.req.lane) || self.first_lanes.contains(&l.req.lane) { Priority::Interactive } else { l.req.priority };
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
            self.refresh_interactive(&holds);
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
                        let ram_measured = self.measured_ram(&self.leases[i].req, None).is_some();
                        let run = self.run_on(&node, &self.leases[i], &cost);
                        if self.leases[i].req.upkeep() { self.prebuild_since.remove(&node); }
                        // since when it stood in this queue: a leg with its run's place ([`runs_place`])
                        // entered it only when its run's previous leg ended, maybe long after its box's release
                        let queued_since = now.saturating_sub(self.leases[i].waited_ms(now));
                        let l = &mut self.leases[i];
                        l.state = State::Starting;
                        l.node = Some(node.clone());
                        l.deployment = self.nodes.get(&node).and_then(|n| n.caps.as_ref()).map_or(String::new(), |c| c.deploy.clone());
                        l.cost = cost;
                        l.run = run;
                        l.ram_measured = ram_measured;
                        l.granted_ms = Some(now);
                        l.reason.clear();
                        l.pos = 0;
                        let released = self.nodes.get(&node).and_then(|n| n.released_ms);
                        if let Some(r) = released.filter(|r| queued_since <= *r && l.req.kind != Kind::Inspect) {
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
                        let mut note = self.leases[i].take_class_note();
                        note.push_str(&self.past_budget_said(&self.leases[i].id));
                        let l = self.leases[i].clone();
                        self.event(Sev::Info, format!("{} granted to {} after {}{note}", l.tag(), l.req.lane, dur(l.waited_ms(now))), Some(&l));
                        // (an inspect of one app, often granted and done between two ticks: a look at it)
                        if let (Kind::Inspect, Some(jit)) = (l.req.kind, l.req.jit.as_deref()) {
                            self.app_touched(&node, jit, "cmd", now);
                        }
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
                            let until = self.hold_end();
                            let l = &mut self.leases[i];
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
        // a starved ask's hold is said in the master's log when it takes a box or moves to
        // another: which ask, which box, when it starts there by that box's measure and what it
        // waits on there. Which box a live launch held in its 210 s wait could only be guessed
        // afterwards (L678678, 11-08)
        let mut said = Vec::new();
        for (node, h) in &holds {
            if self.held_before.get(node).is_some_and(|o| o.id == h.id) { continue; }
            let Some(l) = self.get(&h.id).filter(|l| self.starved(l)) else { continue };
            let cost = self.cost_on(node, l);
            let was = self.held_before.iter().find(|(_, o)| o.id == h.id).map_or(String::new(), |(n, _)| format!(" (was {n})"));
            let starts = match self.remaining_ms(node, l, &cost) { u64::MAX => "at no end known there".to_string(), 0 => "now".to_string(), ms => format!("in {}", dur(ms)) };
            let on = self.now_fits(node, l, &cost, &holds).err().map_or_else(|| "nothing".to_string(), |b| b.reason);
            let what = if self.live_launch(l) { "live launch".to_string() } else { l.req.kind.as_str().to_string() };
            said.push(format!("{} ({} {what}, {:.0} cores, queued {}) holds {node}{was}: starts {starts} by its measure; waits there on {on}", l.id, l.req.lane, cost.cores, dur(self.clock.saturating_sub(l.asked_ms))));
        }
        self.out.extend(said.into_iter().map(Action::Log));
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
            let (eta_box, eta) = if self.drains_box(&self.leases[i]) { self.eta(&self.leases[i]).map_or((None, 0), |(n, ms)| (Some(n), ms)) } else { (None, 0) };
            let late = eta_box.as_deref().and_then(|n| self.behind_overrun(n, &self.leases[i]));
            let l = &mut self.leases[i];
            l.eta_ms = if late.is_some() { 0 } else { eta };
            if eta_box.is_some() && late.is_none() { l.told_ms = Some(now.saturating_add(eta)); }
            l.eta_late = late.is_some();
            l.eta_late_by = late.unwrap_or_default();
            l.eta_box = eta_box;
            // Keep attribution from the final pass, even when the displayed reason is unchanged.
            l.queue_boxes = boxes;
            // not in a new master's first minute: every box is "not heard yet" for its first
            // seconds, and an ask 21 min behind a test warned that its node was missing (11-04)
            let warn = !l.warned && now >= self.nodes_settle_ms && l.waited_ms(now) >= WAIT_WARN_MS;
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
                format!("{} for {} waiting {}, the box's next JIT, a background build; a lease that needs it sooner builds it: {} in queue ({})", l.tag(), l.req.lane, dur(l.waited_ms(now)), ordinal(l.pos), l.reason)
            } else if warn {
                let late = if l.eta_late { format!("; no expected start: {}", l.eta_late_by) } else { String::new() };
                format!("{} for {} waiting {}{}: {} in queue ({}){late}", l.tag(), l.req.lane, dur(l.waited_ms(now)), l.owed_age(now), ordinal(l.pos), l.reason)
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
        let past: Vec<f64> = self.leases.iter().map(|l| self.past_budget(l)).collect();
        for (l, past) in self.leases.iter_mut().zip(past) { l.past_budget = past; }
        std::mem::take(&mut self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_req_field_is_read() {
        // Test fixtures must not count as readers of a field used by no running code.
        let production = |src: &'static str| src.split("\n#[cfg(test)]\nmod ").next().unwrap();
        let src = production(include_str!("sched.rs"));
        let start = src.find("pub struct Req {").unwrap();
        let end = start + src[start..].find("\n}").unwrap() + 2;
        let fields: Vec<&str> = src[start..end].lines()
            .filter_map(|line| line.trim().strip_prefix("pub "))
            .filter_map(|line| line.split_once(':').map(|(field, _)| field)).collect();
        assert!(!fields.is_empty(), "Req must contain fields");
        let mut readers = src.to_string();
        readers.replace_range(start..end, "");
        for name in ["to_json", "parse"] {
            let implementation = readers.find("impl Req {").unwrap();
            let start = implementation + readers[implementation..].find(&format!("    pub fn {name}(")).unwrap();
            let end = start + readers[start..].find("\n    }").unwrap() + "\n    }".len();
            readers.replace_range(start..end, "");
        }
        let sources = [readers.as_str(), production(include_str!("exec.rs")), production(include_str!("master.rs"))];
        for field in fields {
            let access = format!(".{field}");
            let read = sources.iter().any(|src| src.match_indices(&access).any(|(at, _)| {
                src[at + access.len()..].starts_with(|c: char| !c.is_alphanumeric() && c != '_')
            }));
            assert!(read, "`Req::{field}` is parsed but nothing reads it: a flag that sets it does nothing");
        }
    }

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
            features: super::super::FEATURES.iter().map(|f| f.to_string()).collect(),
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

    #[test]
    fn exec_size_routes_by_node_limits_and_spills_only_the_script() {
        use crate::os::ExecLimits;
        use super::super::argv::ExecProfile;
        let linux = Caps { exec_limits: Some(ExecLimits { total: 2 * 1024 * 1024, per_string: Some(128 * 1024) }), ..caps(16, 64.0, Role::Pool) };
        let mac = Caps { os: "macos".into(), exec_limits: Some(ExecLimits { total: 1024 * 1024, per_string: None }), ..linux.clone() };
        assert_eq!(Caps::parse(&linux.to_json()).unwrap().exec_limits, linux.exec_limits);
        assert_eq!(Caps::parse(&mac.to_json()).unwrap().exec_limits, mac.exec_limits);
        let mut s = sched(&[("node-a", linux.clone()), ("node-b", mac.clone())]);
        // Inspect allows both operating systems; ordinary macOS work still needs os=macos.
        let mut r = req("exec-size", Kind::Inspect, Some("node-a"));
        r.jit = Some("app".into());
        let word = format!("X={}", "é".repeat(100 * 1024));
        r.exec_profile = ExecProfile::of(&["env".into(), word.clone(), "true".into()]);
        assert_eq!(Req::parse(&r.to_json()).unwrap().exec_profile, r.exec_profile);
        let l = s.proposed(String::new(), r.clone(), 0);
        let over = s.never("node-a", &linux, &l, &l.cost).unwrap_err();
        assert!(over.contains("Linux execs at most 128 KiB"), "{over}");
        assert!(over.contains(&word.chars().take(40).collect::<String>()), "the word's first forty characters: {over}");
        assert!(s.never_places(&r).is_some(), "an explicit Linux pin is refused");
        assert!(s.never("node-b", &mac, &l, &l.cost).is_ok(), "no per-string limit on macOS");
        r.node = None;
        assert!(s.never_places(&r).is_none(), "one eligible node fits");
        let old = Caps { exec_limits: None, ..linux.clone() };
        assert!(s.never("node-a", &old, &l, &l.cost).is_ok(), "old caps defer to the node");
        s.set_caps("node-a", old, 0);
        r.node = Some("node-a".into());
        assert!(s.never_places(&r).is_none(), "unknown limits cannot refuse submission");
        let script = "x".repeat(3 * 1024 * 1024);
        for argv in [vec!["sh".into(), "-c".into(), script.clone()], vec!["env".into(), "X=ok".into(), "sh".into(), "-c".into(), script.clone()]] {
            r.exec_profile = ExecProfile::of(&argv);
            let l = s.proposed(String::new(), r.clone(), 0);
            assert!(s.never("node-a", &linux, &l, &l.cost).is_ok(), "the script can spill");
        }
        r.exec_profile = ExecProfile::of(&["sh".into(), "-c".into(), script, word]);
        let l = s.proposed(String::new(), r.clone(), 0);
        assert!(s.never("node-a", &linux, &l, &l.cost).is_err(), "an additional large word cannot spill");
        r.exec_profile = ExecProfile::of(&vec!["x".repeat(64 * 1024); 33]);
        let l = s.proposed(String::new(), r, 0);
        assert!(s.never("node-a", &linux, &l, &l.cost).unwrap_err().contains("command and env are"), "total size is checked even when each word fits");
    }

    fn req(lane: &str, kind: Kind, node: Option<&str>) -> Req {
        let mut r = Req::new(lane, kind);
        r.node = node.map(str::to_string);
        r
    }

    /// `r` as devhub's own ask (an agent `devhub-...`: a warm leg, CI): the only work besides timed
    /// and inspect leases a bench box takes by name ([`BENCH_PIN`])
    fn devhubs(mut r: Req) -> Req {
        r.agent = Some("devhub-warm".into());
        r
    }

    /// A lane's ask that names its box with more RAM than the box may give now beside its
    /// residents is denied when asked, saying the box's budget and the boxes of its platform that
    /// hold it; one that fit is denied so once the residents grow while it waits, not left heading
    /// the box's queue (L798323, 11-10). The RAM its hub's own build holds passes: under it the ask
    /// waits, and is judged once the build ends.
    #[test]
    fn a_pinned_ask_past_its_boxs_ram_now_is_denied_with_the_boxes_that_hold_it() {
        let user = |resident: f64| Caps { ram_resident_gb: resident, ..caps(16, 62.0, Role::Pool) };
        let mut s = sched(&[("box-a", user(11.9)), ("box-b", caps(16, 64.0, Role::Pool)), ("box-c", caps(16, 32.0, Role::Pool))]);
        let ask = |lane: &str, gb: f64| Req { ram_gb: Some(gb), ..req(lane, Kind::Test, Some("box-a")) };
        let big = s.request(ask("look", 48.0), 0);
        s.tick(0);
        assert_eq!(state(&s, &big).0, "ended denied", "{:?}", state(&s, &big));
        assert!(state(&s, &big).1.contains("needs 48 GB RAM (--ram-gb) and box-a (--node) may give 43.9 GB now (62 GB less 10% and the 11.9 GB its residents hold): box-b holds it now: unpin it (`--needs os=linux,arch=x86_64` in place of --node)"), "box-c's 32 GB never holds it: {:?}", state(&s, &big));
        let first = s.request(ask("first", 10.0), 1);
        assert_eq!(grants(&s.tick(1)), vec![first.clone()]);
        let waits = s.request(ask("waits", 40.0), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert_eq!(state(&s, &waits).0, "queued", "it fits once the first ends: {:?}", state(&s, &waits));
        s.set_caps("box-a", Caps { hub_building: true, ..user(20.0) }, 3);
        s.tick(3);
        assert_eq!(state(&s, &waits).0, "queued", "its hub's build is no lasting resident: {:?}", state(&s, &waits));
        s.set_caps("box-a", user(20.0), 4);
        s.tick(4);
        assert_eq!(state(&s, &waits).0, "ended denied", "{:?}", state(&s, &waits));
        assert!(state(&s, &waits).1.contains("needs 40 GB RAM (--ram-gb) and box-a (--node) may give 35.8 GB now (62 GB less 10% and the 20.0 GB its residents hold): box-b holds it now"), "{:?}", state(&s, &waits));
    }

    /// A timed lease starts only on a quiet box: not while work outside its leases takes half a
    /// core or more (a desktop left running), its queue line saying why. The box's own leases are
    /// no noise: it holds the box against them as before, however busy they make it; a noisy box
    /// with none takes other work meanwhile. Its load alone holds nothing (it trails the leases
    /// that just ended).
    #[test]
    fn a_timed_lease_waits_for_a_quiet_box() {
        let noisy = |load1: f64, cpu: f64| Caps { load1, cpu_pct: Some(cpu), ..caps(16, 64.0, Role::Pool) };
        let mut s = sched(&[("a", noisy(1.0, 2.0))]);
        let x = s.request(req("lane", Kind::Test, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![x.clone()]);
        s.started(&x, "{}");
        s.set_caps("a", Caps { busy_leases: vec![(x.clone(), 15.0)], ..noisy(40.0, 99.0) }, 1_000);
        let t = s.request(req("bench", Kind::Timed, Some("a")), 1_000);
        assert!(grants(&s.tick(1_000)).is_empty());
        let d = s.request(req("other", Kind::Test, Some("a")), 1_001);
        s.tick(1_001);
        assert_eq!(state(&s, &d).1, format!("held for timed {t} (the box drains for it: only work that ends first or fits beside it)"), "the lane's work is no noise: the timed lease holds the box");
        let desktop = || Caps { busy_outside: vec![("WindowServer".into(), 407, 0.6)], ..noisy(3.0, 9.0) };
        release_clean(&mut s, &x, 2_000);
        s.set_caps("a", desktop(), 2_000);
        assert_eq!(grants(&s.tick(2_000)), vec![d.clone()], "a noisy box takes other work meanwhile");
        s.started(&d, "{}");
        release_clean(&mut s, &d, 2_500);
        s.set_caps("a", desktop(), 2_500);
        assert!(grants(&s.tick(2_500)).is_empty());
        assert!(state(&s, &t).1.contains("the box is not quiet (144% of a core runs outside its leases: WindowServer pid 407 (0.6 cores))"), "{:?}", state(&s, &t));
        s.set_caps("a", noisy(54.0, 2.0), 3_000);
        assert!(grants(&s.tick(3_000)).contains(&t), "a quiet box, its load trailing: {:?}", state(&s, &t));
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

    /// A visible session stays up for a land gate or a lane's test that waits for its RAM, however
    /// quiet it is, and for an ask that has queued long. A whole-box ask, which cannot run beside
    /// it, parks it.
    #[test]
    fn a_visible_session_parks_only_for_work_that_cannot_run_beside_it() {
        const MIN: u64 = 60_000;
        let overlay = |lane: &str, kind: Kind| {
            let mut r = req(lane, kind, Some("a"));
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r
        };
        let setup = |kinds: &[Kind]| {
            let mut s = sched(&[("a", caps(16, 32.0, Role::Pool))]);
            let sessions: Vec<String> = kinds.iter().enumerate().map(|(i, kind)| {
                let mut r = overlay(&format!("live-W{i}"), *kind);
                (r.live, r.priority) = (true, Priority::Interactive);
                s.request(r, 0)
            }).collect();
            s.tick(0);
            s.tick(0);
            for (i, id) in sessions.iter().enumerate() {
                assert_eq!(node_of(&s, id).as_deref(), Some("a"), "{:?}", state(&s, id));
                s.started(id, "{}");
                // the first is the quietest: it would go first
                s.set_info(id, &format!(r#"{{"running":"makepad","held_mb":12288,"idle_s":{}}}"#, 90 - i * 30), 0);
            }
            s.live_idle.extend(sessions.iter().cloned());
            (s, sessions)
        };
        let parked = |acts: &[Action]| acts.iter().filter_map(|a| if let Action::ParkLive { leases, .. } = a { Some(leases.clone()) } else { None }).collect::<Vec<_>>();
        let gate = |gb: f64| {
            let mut r = overlay("landq-fast", Kind::Gate);
            (r.priority, r.ram_gb) = (Priority::Interactive, Some(gb));
            r
        };
        // the tests session goes, though the visible one is quieter: 12 + 8 of a 27.2 GB budget
        let (mut s, sessions) = setup(&[Kind::Screen, Kind::Test]);
        let g = s.request(gate(8.0), 1);
        let acts = s.tick(1);
        assert!(state(&s, &g).1.contains("budget ram"), "{:?}", state(&s, &g));
        assert_eq!(parked(&acts), vec![vec![sessions[1].clone()]]);
        // the visible session alone: a land gate and a lane's test wait for its RAM
        let (mut s, _) = setup(&[Kind::Screen]);
        let g = s.request(gate(20.0), 1);
        let acts = s.tick(1);
        assert!(state(&s, &g).1.contains("budget ram") && parked(&acts).is_empty(), "{:?} {acts:?}", state(&s, &g));
        // past the time it drains its box too: it starts when the session has ended
        let (mut s, sessions) = setup(&[Kind::Screen]);
        let g = s.request(gate(20.0), 1);
        beat_all(&mut s, 11 * MIN);
        let acts = s.tick(11 * MIN);
        assert!(parked(&acts).is_empty(), "{:?} {acts:?}", state(&s, &g));
        release_clean(&mut s, &sessions[0], 11 * MIN + 1);
        s.tick(11 * MIN + 1);
        assert_eq!(node_of(&s, &g).as_deref(), Some("a"), "{:?}", state(&s, &g));
        // a whole-box ask parks it, and the tests session alike
        let (mut s, sessions) = setup(&[Kind::Screen]);
        let mut whole = overlay("whole", Kind::Test);
        whole.whole = true;
        let w = s.request(whole, 1);
        s.tick(1);
        beat_all(&mut s, LIVE_PARK_MS + 10);
        let acts = s.tick(LIVE_PARK_MS + 10);
        assert_eq!(parked(&acts), vec![sessions.clone()], "{:?}", state(&s, &w));
        invariants(&s);
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
        assert!(p[0].1.contains(&format!("that RAM is free there again when {waits} ends (it runs ~")), "and when it would fit: {}", p[0].1);
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
        // a JIT warm-up (the box's prebuild, a lane's warm leg) waits for the sessions to end
        for lane in [format!("{}a", super::super::PREBUILD_LANE_PREFIX), format!("{}lane-x", super::super::WARM_LANE_PREFIX)] {
            let (mut s, _) = setup();
            let mut r = req(&lane, Kind::Build, Some("a"));
            (r.src, r.rev, r.ram_gb) = (Src::Overlay, Some(format!("work@{}", "a".repeat(40))), Some(8.0));
            let warm = s.request(r, 1);
            let acts = s.tick(1);
            assert!(state(&s, &warm).1.contains("budget ram"), "{:?}", state(&s, &warm));
            assert!(parked(&acts).is_empty(), "{lane} parks no session: {:?}", parked(&acts));
        }
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

    /// A queued request whose target moves (a newer landing's prewarm) takes it in place: its
    /// id, place and age stay, its pin and JIT hint change; a granted one keeps its grant, and a
    /// new target is for the same work.
    #[test]
    fn a_queued_request_takes_a_new_target_in_place() {
        let mut s = sched(&[("a", caps(8, 32.0, Role::Pool))]);
        let fill = s.request(req("fill", Kind::Build, Some("a")), 0);
        let mut ask = overlay(&format!("{}lane", super::super::WARM_LANE_PREFIX), Some("a"), 8.0);
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
                if let Some((id, node)) = s.restoring.iter().map(|(id, (n, ..))| (id.clone(), n.clone())).next() { s.cleaned(&node, &id, 0); }
            }
            if t % 9 != 4 { continue; }
            // the master execs now: the new one takes back exactly what this one had
            let before = |s: &Sched| {
                let mut v: Vec<(String, String, Option<String>)> = s.leases.iter().map(|l| (l.id.clone(), l.state.as_str().to_string(), l.node.clone())).collect();
                v.extend(s.restoring.iter().map(|(id, (n, ..))| (id.clone(), "cleaning".to_string(), Some(n.clone()))));
                v.sort();
                v
            };
            mid += usize::from(!s.restoring.is_empty() || s.leases.iter().any(|l| l.state == State::Starting));
            let mut s2 = sched(&boxes);
            for line in s.live_lines() { s2.restore(&line, t); }
            assert_eq!(before(&s2), before(&s), "tick {t}");
            for (id, (n, _, c, ended)) in &s.restoring {
                assert_eq!(s2.cleaning(id), Some(n.as_str()));
                assert_eq!(s2.restoring[id].2, *c, "{id} holds what it held");
                assert_eq!(s2.restoring[id].3, *ended, "{id} keeps when it ended");
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

    /// A box's timed work keeps the pre-warm off it: a timed ask waiting for it, the lease
    /// running there, and its series' window after it ended, a master's restart in it too (one
    /// forgot the bench box's last run and the box pre-warmed in its series' gap, 11-10); another
    /// box's timed work does not.
    #[test]
    fn timed_work_on_a_box_keeps_its_prewarm_off() {
        let mut s = sched(&[("a", caps(64, 256.0, Role::Pool)), ("b", caps(64, 256.0, Role::Pool))]);
        let blocker = s.request(req("long", Kind::Build, Some("a")), 0);
        s.tick(0);
        let id = s.request(req("bench", Kind::Timed, Some("a")), 1);
        s.tick(1);
        assert!(s.timed_near("a", 2).is_some_and(|w| w.contains(&id) && w.contains("waits for it")), "{:?}", s.timed_near("a", 2));
        assert!(s.timed_near("b", 2).is_none(), "another box's timed work");
        release_clean(&mut s, &blocker, 10);
        s.tick(11);
        assert!(s.timed_near("a", 12).is_some_and(|w| w.contains("runs there")), "{:?}", s.timed_near("a", 12));
        let view = |s: &Sched, now: u64| s.node_view(now).into_iter().find(|v| v.name == "a").unwrap();
        let line = makepad_strict_json::parse(view(&s, 12).to_json().as_bytes()).unwrap();
        assert!(NodeView::parse(&line).is_some_and(|v| v.timed.as_deref().is_some_and(|w| w.contains(&id))), "the nodes line carries it");
        release_clean(&mut s, &id, 1_000);
        assert!(s.timed_near("a", 2_000).is_some_and(|w| w.contains("ended there")), "{:?}", s.timed_near("a", 2_000));
        assert!(s.timed_near("a", 1_000 + TIMED_SERIES_MS + 1).is_none(), "past its series' window");
        assert!(view(&s, 1_000 + TIMED_SERIES_MS + 1).timed.is_none());
        s.tick(2_000);
        let lines = s.live_lines();
        let restarted = |at: u64| {
            let mut s2 = sched(&[("a", caps(64, 256.0, Role::Pool)), ("b", caps(64, 256.0, Role::Pool))]);
            assert!(lines.iter().all(|l| s2.restore(l, at).is_none()), "nothing live comes back: {lines:?}");
            s2
        };
        let s2 = restarted(2_000);
        assert!(s2.timed_near("a", 2_000).is_some_and(|w| w.contains(&id) && w.contains("ended there")), "a restart keeps its window: {:?}", s2.timed_near("a", 2_000));
        assert!(s2.get(&id).is_some_and(|l| matches!(l.state, State::Ended(_))) && s2.view().iter().all(|l| l.id != id), "back as ended, not live");
        assert!(restarted(1_000 + TIMED_SERIES_MS + 1).timed_near("a", 1_000 + TIMED_SERIES_MS + 1).is_none(), "one restored past its window is gone");
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
            let a = s.request(devhubs(req("a", first, Some("node-b"))), 0);
            s.tick(0);
            let b = s.request(devhubs(req("b", if first == Kind::Ci { Kind::Timed } else { Kind::Ci }, Some("node-b"))), 1);
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
        assert_eq!(waits.eta_late_by, format!("behind {running} timed@node-c of holder, {} into its {} (a guess from its cost large: no --est-min, its class not measured yet); told to yield at {}, in {}", dur(past), dur(past - 1), dur(2 * (past - 1)), dur(past - 2)), "the run past its estimate is named, with how far, what estimate and when it is told to yield");
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
    fn timed_plain_tests_get_one_note_from_class_or_full_argv() {
        for (command, told) in [
            ("makepad test p --bin p", true),
            ("cargo test --release", true),
            ("makepad test p -- --nocapture", false),
            ("cargo test --release -- --show-output", false),
            ("cargo test --bench timing", false),
            ("bash ab.sh", false),
        ] {
            for argv in [false, true] {
                let mut s = sched(&[("box-a", caps(16, 64.0, Role::Bench))]);
                let mut r = req("bench", Kind::Timed, Some("box-a"));
                r.est_min = Some(1.0); // isolate this note from the estimate note
                r.class = format!("timed {command}");
                if argv {
                    let args: Vec<_> = command.split_whitespace().map(str::to_string).collect();
                    r.run_options(Src::Tree, None, Some(1.0), "custom", &args);
                }
                let r = Req::parse(&r.to_json()).unwrap();
                let id = s.request(r, 1);
                let l = s.leases.iter_mut().find(|l| l.id == id).unwrap();
                assert_eq!(l.class_note(), if told { TIMED_TEST_NOTE } else { "" }, "{command}, argv={argv}");
                assert_eq!(!l.take_class_note().is_empty(), told);
                assert!(l.take_class_note().is_empty(), "once per request");
            }
        }
        // A generated class drops output flags: the full argv must take precedence.
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Bench))]);
        let mut r = req("bench", Kind::Timed, Some("box-a"));
        let args = ["makepad", "test", "p", "--", "--nocapture"].map(str::to_string);
        r.run_options(Src::Tree, None, Some(1.0), "", &args);
        let id = s.request(Req::parse(&r.to_json()).unwrap(), 1);
        assert!(s.get(&id).unwrap().class_note().is_empty());
    }

    /// A timed run a quarter past its estimate (a minute at least) while a timed ask waits on its
    /// box is told to yield, once, and the waiter's queue line says when. At twice a 7m45s
    /// measure, a timed ask waited 16 min behind the run (L661643, 11-08).
    #[test]
    fn a_timed_run_past_its_estimate_with_a_timed_ask_on_its_box_is_told_to_yield_once() {
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        let mut r = req("bench", Kind::Timed, Some("node-c"));
        r.est_min = Some(5.0);
        let running = s.request(r, 0);
        grants(&s.tick(0));
        s.started(&running, "{}");
        let yields = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Yield { id, node, why } => Some((id.clone(), node.clone(), why.clone())), _ => None }).collect::<Vec<_>>();
        let past = 6 * 60_000;
        let due = 6 * 60_000 + 15_000;
        beat_all(&mut s, due);
        assert!(yields(&s.tick(due)).is_empty(), "nobody waits on its box: its run is its own business");
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        let mut r = req("bench", Kind::Timed, Some("node-c"));
        r.est_min = Some(5.0);
        let running = s.request(r, 0);
        grants(&s.tick(0));
        s.started(&running, "{}");
        let waiting = s.request(req("other", Kind::Timed, Some("node-c")), past);
        beat_all(&mut s, past + 1);
        assert!(yields(&s.tick(past + 1)).is_empty(), "past its estimate, not yet a quarter past");
        let w = s.get(&waiting).unwrap();
        assert!(w.eta_late && w.eta_late_by.ends_with("6m00s into its 5m00s (its --est-min); told to yield at 6m15s, in 14 s"), "{}", w.eta_late_by);
        beat_all(&mut s, due);
        let a = s.tick(due);
        let y = yields(&a);
        assert_eq!(y.iter().map(|(id, node, _)| (id.as_str(), node.as_str())).collect::<Vec<_>>(), vec![(running.as_str(), "node-c")], "{a:?}");
        assert!(y[0].2.contains(&format!("6m15s into its 5m00s (its --est-min), while timed {waiting} of other waits on its box: its $DEVHUB_YIELD file now exists")), "{}", y[0].2);
        assert_eq!(s.owner_word(&running).as_deref(), Some(y[0].2.as_str()), "its job wait and its beat hear it");
        assert!(s.get(&waiting).unwrap().eta_late_by.ends_with("; told to yield"), "{}", s.get(&waiting).unwrap().eta_late_by);
        beat_all(&mut s, due + 1);
        assert!(yields(&s.tick(due + 1)).is_empty(), "said once");
    }

    /// A long timed run is told to yield from its first minute while its box's catch-up waits on
    /// it: a timed ask queued for the box needs its deployment ([`Sched::deploy_head`]), and the
    /// update takes the box's next gap. That ask's length does not matter: the catch-up is short.
    /// A bench box's catch-up waited 20 min behind back-to-back 16-18 min timed runs (L691645, 11-08).
    #[test]
    fn a_long_timed_run_yields_to_its_boxs_catch_up() {
        let at = |n: u64| format!("{n}{:039},commercial={n}{:039}", 0, 0);
        let mut c = caps(16, 64.0, Role::Bench);
        c.deploy = at(0);
        let mut s = sched(&[("node-c", c)]);
        (s.tips, s.landed) = (at(1), vec![(at(0), 0), (at(1), 0)]);
        let timed = |lane: &str, min: f64| {
            let mut r = req(lane, Kind::Timed, Some("node-c"));
            r.est_min = Some(min);
            r
        };
        let long = s.request(timed("bench", 30.0), 0);
        grants(&s.tick(0));
        s.started(&long, "{}");
        let yields = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Yield { id, why, .. } => Some((id.clone(), why.clone())), _ => None }).collect::<Vec<_>>();
        // a long ask of its own: alone it leaves the run be until the run overruns
        let next = s.request(timed("long-too", 15.0), 1_000);
        beat_all(&mut s, 61_000);
        assert!(yields(&s.tick(61_000)).is_empty(), "long behind long");
        // it needs the box's deployment: the catch-up waits on the run
        s.leases.iter_mut().find(|l| l.id == next).unwrap().deploy_pin = Some(at(1));
        beat_all(&mut s, 62_000);
        let y = yields(&s.tick(62_000));
        assert_eq!(y.len(), 1, "{y:?}");
        assert_eq!(y[0].0, long);
        assert!(y[0].1.contains(&format!("a long run, 1m02s into its 30m00s (its --est-min), while its box's catch-up, which {next} of long-too waits for, waits on it: its $DEVHUB_YIELD file now exists")), "{}", y[0].1);
        beat_all(&mut s, 63_000);
        assert!(yields(&s.tick(63_000)).is_empty(), "said once");
    }

    /// A long timed run is told to yield from its first minute while a timed ask that is not long
    /// waits on its box; a long one leaves it be until it overruns (main, 11-07: a 1-min case
    /// waited 31 min behind a 64-min run).
    #[test]
    fn a_long_timed_run_yields_to_short_timed_work_waiting_on_its_box() {
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        let timed = |lane: &str, min: f64| {
            let mut r = req(lane, Kind::Timed, Some("node-c"));
            r.est_min = Some(min);
            r
        };
        let long = s.request(timed("bench", 30.0), 0);
        grants(&s.tick(0));
        s.started(&long, "{}");
        let yields = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Yield { id, why, .. } => Some((id.clone(), why.clone())), _ => None }).collect::<Vec<_>>();
        s.request(timed("long-too", 15.0), 1_000);
        let short = s.request(timed("quick", 2.0), 2_000);
        beat_all(&mut s, 30_000);
        assert!(yields(&s.tick(30_000)).is_empty(), "a minute in at the earliest");
        beat_all(&mut s, 61_000);
        let y = yields(&s.tick(61_000));
        assert_eq!(y.len(), 1, "{y:?}");
        assert_eq!(y[0].0, long);
        assert!(y[0].1.contains(&format!("a long run, 1m01s into its 30m00s (its --est-min), while short timed {short} of quick waits on its box: its $DEVHUB_YIELD file now exists")), "{}", y[0].1);
        beat_all(&mut s, 62_000);
        assert!(yields(&s.tick(62_000)).is_empty(), "said once");
        // a run not long enough to yield to short work
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        let mid = s.request(timed("bench", 8.0), 0);
        grants(&s.tick(0));
        s.started(&mid, "{}");
        s.request(timed("quick", 2.0), 1_000);
        beat_all(&mut s, 120_000);
        assert!(yields(&s.tick(120_000)).is_empty(), "8 min is no long run");
    }

    /// A long timed ask waits for queued ones that are not long and asked before it had queued
    /// [`STARVED_MS`] (a 1-min case waited 62 min behind a 26-min script, 11-07); among those, the
    /// earliest expected end goes first. A short ask whose turn came after that (21 min) waits
    /// for it ([`Sched::exclusive_epoch`]).
    #[test]
    fn a_short_timed_run_goes_before_a_long_one_asked_just_before_it() {
        let min = 60_000;
        let timed = |lane: &str, est: f64| {
            let mut r = req(lane, Kind::Timed, Some("bench-a"));
            r.est_min = Some(est);
            r
        };
        for (short_at, short_first) in [(2, true), (21 * min, false)] {
            let mut s = sched(&[("bench-a", caps(16, 64.0, Role::Bench))]);
            let running = s.request(timed("first", 60.0), 0);
            assert_eq!(grants(&s.tick(0)), vec![running.clone()]);
            let long = s.request(timed("long", 26.0), 1);
            s.tick(1);
            beat_all(&mut s, short_at);
            let short = s.request(timed("short", 6.0), short_at);
            s.tick(short_at);
            beat_all(&mut s, short_at + 1);
            release_clean(&mut s, &running, short_at + 1);
            let first = if short_first { &short } else { &long };
            assert_eq!(grants(&s.tick(short_at + 1)), vec![first.clone()], "the short one asked at {short_at}");
            invariants(&s);
        }
        // within 10 min both: the earliest end first
        let mut s = sched(&[("bench-a", caps(16, 64.0, Role::Bench))]);
        let running = s.request(timed("first", 60.0), 0);
        assert_eq!(grants(&s.tick(0)), vec![running.clone()]);
        s.request(timed("eight", 8.0), 1);
        let two = s.request(timed("two", 2.0), 3 * min);
        s.tick(3 * min);
        beat_all(&mut s, 3 * min + 1);
        release_clean(&mut s, &running, 3 * min + 1);
        assert_eq!(grants(&s.tick(3 * min + 1)), vec![two], "it ends at 5 min, the 8-min one at 8");
    }

    /// A long timed ask fixes its place once it has queued STARVED_MS: short asks whose turn came
    /// before that stay ahead of it, none whose turn came later passes it (a stream of 2-min asks
    /// every 3 min). And the bench box's case of 11-11 01:15Z: a 45-min ask asked first, a 6-min
    /// one first in timed order that waits for the box's catch-up, a 6-min one asked before the
    /// long ask's fix and a 1-min one after it: the catch-up takes the gap, then the 6-min ones,
    /// the long one, the 1-min one.
    #[test]
    fn a_long_timed_ask_keeps_its_place_once_it_has_waited_starved_ms() {
        const MIN: u64 = 60_000;
        let timed = |lane: &str, est: f64| {
            let mut r = req(lane, Kind::Timed, Some("box-a"));
            r.est_min = Some(est);
            r
        };
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Bench))]);
        let first = s.request(timed("first", 5.0), 0);
        assert_eq!(grants(&s.tick(0)), vec![first.clone()]);
        s.started(&first, "{}");
        let long = s.request(timed("long", 26.0), 1);
        s.tick(1);
        let mut shorts = Vec::new();
        let mut starts = Vec::new();
        for minute in 1..=70 {
            let now = minute * MIN;
            if (2..=30).contains(&minute) && (minute - 2) % 3 == 0 {
                shorts.push(s.request(timed(&format!("short-{minute}"), 2.0), now));
            }
            let ended: Vec<_> = s.leases.iter().filter(|l| l.state.live() && l.granted_ms.is_some_and(|at| now >= at + s.run_ms(l))).map(|l| l.id.clone()).collect();
            for id in ended { release_clean(&mut s, &id, now); }
            beat_all(&mut s, now);
            for id in grants(&s.tick(now)) {
                s.started(&id, "{}");
                starts.push(id);
            }
        }
        let long_at = s.get(&long).unwrap().granted_ms.expect("the stream must let the long ask start");
        assert!(long_at <= 1 + STARVED_MS + 2 * MIN + MIN, "long started at {long_at}");
        let long_pos = starts.iter().position(|id| id == &long).unwrap();
        for id in shorts {
            let short = s.get(&id).unwrap();
            let pos = starts.iter().position(|started| started == &id).expect("each short starts");
            assert_eq!(pos < long_pos, short.asked_ms < 1 + STARVED_MS, "{} asked at {} versus long fix {}", short.req.lane, short.asked_ms, 1 + STARVED_MS);
        }
        invariants(&s);

        // The updater waits for timed work, then takes three minutes in the next gap.
        let at = |n: u64| format!("{n}{:039},commercial={n}{:039}", 0, 0);
        let mut deploy = at(0);
        let mut c = caps(16, 64.0, Role::Bench);
        c.deploy = deploy.clone();
        let mut s = sched(&[("box-a", c)]);
        (s.tips, s.landed) = (at(1), vec![(at(0), 0), (at(1), 0)]);
        let cmpops = s.request(timed("cmpops", 20.0), 0);
        assert_eq!(grants(&s.tick(0)), vec![cmpops.clone()]);
        s.started(&cmpops, "{}");
        let fabxrgpu = s.request(timed("fabxrgpu", 45.0), 1);
        s.tick(1);
        let mut ids = vec![cmpops.clone(), fabxrgpu.clone()];
        let mut running = Some((cmpops, 39 * MIN / 2));
        let mut applying: Option<(u64, String)> = None;
        let mut starts = Vec::new();
        for step in 1..=180 {
            let now = step * MIN / 2;
            for (minute, lane, est) in [(5, "jito3o3c", 6.0), (8, "jito3bfold", 6.0), (15, "fabtreefit", 1.0)] {
                if now == minute * MIN {
                    let id = s.request(timed(lane, est), now);
                    if lane == "jito3o3c" { s.leases.iter_mut().find(|l| l.id == id).unwrap().deploy_pin = Some(at(1)); }
                    ids.push(id);
                }
            }
            if let Some((id, _)) = running.clone().filter(|(_, end)| *end <= now) {
                release_clean(&mut s, &id, now);
                running = None;
            }
            if now == 39 * MIN / 2 {
                beat_all(&mut s, now);
                assert!(grants(&s.tick(now)).is_empty(), "the catch-up takes the gap");
                let why = state(&s, &fabxrgpu).1;
                assert!(why.contains(&format!("behind a catch-up: {} of jito3o3c (first in timed order) waits for box-a's deployment at 1000000000,commercial=1000000000, which takes the next gap; timed work after it starts after both", ids[2])), "{why}");
                assert_eq!(super::super::prof::blocker(&why), "behind a catch-up");
            }
            if let Some((_, to)) = applying.clone().filter(|(end, _)| *end <= now) {
                deploy = to;
                applying = None;
            }
            let target = match deploy_to(s.update_pin("box-a"), &deploy, |d, a| d >= a) {
                DeployTo::Tips => Some(at(1)),
                DeployTo::Pin(p) => Some(p),
                DeployTo::Held { .. } => None,
            };
            s.update_waits.clear();
            match target.filter(|t| *t != deploy && applying.is_none()) {
                Some(_) if running.is_some() => { s.update_waits.entry("box-a".into()).or_default().insert("deployment", "its deployment moves to the next pin".into()); }
                Some(t) => applying = Some((now + 3 * MIN, t)),
                None => {}
            }
            let mut c = caps(16, 64.0, Role::Bench);
            (c.deploy, c.applying) = (deploy.clone(), applying.is_some());
            s.set_caps("box-a", c, now);
            beat_all(&mut s, now);
            let granted = grants(&s.tick(now));
            assert!(granted.len() <= 1, "one timed run at a time: {granted:?}");
            for id in granted {
                assert!(running.is_none());
                s.started(&id, "{}");
                let l = s.get(&id).unwrap();
                starts.push((l.req.lane.clone(), now, l.deployment.clone()));
                running = Some((id, now + s.run_ms(l)));
            }
        }
        assert_eq!(starts, vec![
            ("jito3o3c".into(), 45 * MIN / 2, at(1)),
            ("jito3bfold".into(), 57 * MIN / 2, at(1)),
            ("fabxrgpu".into(), 69 * MIN / 2, at(1)),
            ("fabtreefit".into(), 159 * MIN / 2, at(1)),
        ]);
        invariants(&s);
    }

    /// A lane's resubmit keeps the place of the queued ask it dropped (its holder released it, or
    /// it cancelled it itself): TREESPEED's resubmit lost the 9.5 min its ask had waited (11-07).
    /// The user's cancel keeps none, nor does an ask of another kind.
    #[test]
    fn a_lanes_resubmit_keeps_the_place_of_the_ask_it_dropped() {
        let timed = |lane: &str| req(lane, Kind::Timed, Some("bench-a"));
        for how in ["released", "lane cancel", "user cancel", "other kind"] {
            let mut s = sched(&[("bench-a", caps(16, 64.0, Role::Bench))]);
            let running = s.request(timed("first"), 0);
            assert_eq!(grants(&s.tick(0)), vec![running.clone()]);
            let dropped = s.request(timed("lane"), 10);
            let other = s.request(timed("other"), 20);
            s.tick(20);
            match how {
                "released" | "other kind" => { s.release(&dropped, "released", 30); }
                "lane cancel" => { s.cancel_by(&dropped, "LANE", 30); }
                _ => { s.cancel(&dropped, 30); }
            }
            let again = if how == "other kind" { s.request(req("lane", Kind::Test, Some("bench-a")), 31) } else { s.request(timed("lane"), 31) };
            let kept = matches!(how, "released" | "lane cancel");
            assert_eq!(s.get(&again).unwrap().asked_ms, if kept { 10 } else { 31 }, "{how}");
            if how == "other kind" { continue; }
            beat_all(&mut s, 32);
            release_clean(&mut s, &running, 32);
            assert_eq!(grants(&s.tick(32)), vec![if kept { again.clone() } else { other.clone() }], "{how}");
            invariants(&s);
        }
    }

    #[test]
    fn a_waiting_update_holds_a_timed_lease_only_when_the_box_catches_up() {
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        s.update_waits.entry("node-c".into()).or_default().insert("deployment", "its deployment moves to abc".into());
        let timed = s.request(req("bench", Kind::Timed, Some("node-c")), 0);
        s.leases.iter_mut().find(|l| l.id == timed).unwrap().deploy_pin = Some("abc,def".into());
        let test = s.request(devhubs(req("test", Kind::Test, Some("node-c"))), 0);
        assert_eq!(grants(&s.tick(0)), vec![test.clone()], "other work goes on");
        assert!(state(&s, &timed).1.contains("catches up first (its deployment moves to abc)"), "{:?}", state(&s, &timed));
        let word = s.node_view(0).into_iter().find(|n| n.name == "node-c").unwrap().state_word();
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

    /// A box's own hub build is a lease of its measured size: it waits behind the work granted
    /// there and reserves its room meanwhile (newer work goes only beside it), nothing is granted
    /// past the box's cores, and timed work waits for it. Only the box's room holds it (a Mac,
    /// hot, dev off: no lane work, still its hub's build), and nothing of it goes to the node (two
    /// Macs' hubs built beside their work at 12 and 14 of their 10 cores, 11-05).
    #[test]
    fn a_hub_build_waits_behind_granted_work_and_the_box_never_runs_past_its_cores() {
        let mut c = caps(10, 32.0, Role::Hot);
        c.os = "macos".into();
        // a hub without `hub-jobs` builds with cargo's own jobs: its room is all it measured
        c.features.retain(|f| f != "hub-jobs");
        let mut s = sched(&[("mac1", c.clone())]);
        for _ in 0..3 { s.record_resources(HUB_BUILD, "build", Some(3.0), Some(6.0)); }
        let ask = |lane: &str, kind: Kind, cores: f64| {
            let mut r = req(lane, kind, Some("mac1"));
            r.needs = vec![("os".into(), "macos".into())];
            r.cores = Some(cores);
            r.src = Src::Overlay;
            r
        };
        let hub_ask = || {
            let mut r = req(&format!("{HUB_BUILD}-mac1"), Kind::Build, Some("mac1"));
            r.class = HUB_BUILD.into();
            r.master = true;
            r
        };
        assert!(hub_ask().hub_build() && !hub_ask().set_up_on_node());
        let cap = |s: &Sched| assert!(s.charged_on("mac1").cores <= 10.0, "{} of 10 cores charged", s.charged_on("mac1").cores);
        let work = s.request(ask("work", Kind::Test, 5.0), 0);
        assert_eq!(grants(&s.tick(0)), vec![work.clone()]);
        let hub = s.request(hub_ask(), 1);
        beat_all(&mut s, 1);
        assert!(grants(&s.tick(1)).is_empty());
        cap(&s);
        assert_eq!(state(&s, &hub), ("queued".to_string(), "budget cores 10, in use 5, asked 6".to_string()));
        // newer work goes only beside it: what would keep it waiting once the work ends waits
        let big = s.request(ask("big", Kind::Test, 5.0), 2);
        let small = s.request(ask("small", Kind::Test, 4.0), 2);
        beat_all(&mut s, 2);
        assert_eq!(grants(&s.tick(2)), vec![small.clone()]);
        cap(&s);
        assert!(state(&s, &big).1.contains(&format!("held for {hub} (its hub's build")), "{:?}", state(&s, &big));
        release_clean(&mut s, &work, 3);
        beat_all(&mut s, 3);
        assert_eq!(grants(&s.tick(3)), vec![hub.clone()], "{:?}", state(&s, &hub));
        cap(&s);
        assert!(!s.live_on("mac1").contains(&hub) && !s.starting_on("mac1").contains(&hub), "the node holds nothing of it");
        // beside it: nothing past the cores, and no timed work
        release_clean(&mut s, &small, 4);
        let timed = s.request(ask("bench", Kind::Timed, 2.0), 4);
        beat_all(&mut s, 4);
        assert!(grants(&s.tick(4)).is_empty());
        cap(&s);
        assert!(state(&s, &big).1.contains("budget cores 10, in use 6, asked 5"), "{:?}", state(&s, &big));
        assert!(state(&s, &timed).1.contains(&format!("held for {big} (it starts as the box's work ends")), "the older ask at the head first: {:?}", state(&s, &timed));
        // its box's dev switch off keeps lane work off, not its hub's build
        release_clean(&mut s, &hub, 5);
        s.set_dev("mac1", false, 5).unwrap();
        let again = s.request(hub_ask(), 6);
        beat_all(&mut s, 6);
        assert_eq!(grants(&s.tick(6)), vec![again], "{:?}", state(&s, &big));
        cap(&s);
    }

    #[test]
    fn own_prebuild_and_hub_build_go_between_exclusive_leases() {
        for hub in [false, true] {
            for whole in [false, true] {
                if hub && whole { continue; } // hub builds exclude timed work only
                let mut s = sched(&[("box-a", caps(16, 64.0, Role::Bench))]);
                let running = s.request(req("running", Kind::Timed, Some("box-a")), 1);
                assert_eq!(grants(&s.tick(1)), vec![running.clone()]);
                let mut r = devhubs(req("next", if whole { Kind::Test } else { Kind::Timed }, Some("box-a")));
                r.whole = whole;
                let next = s.request(r, 2);
                s.tick(2); // its reservation predates the box's maintenance
                let mut r = devhubs(req("prebuild-box-a", Kind::Build, Some("box-a")));
                if hub { r.lane = "hub-build-box-a".into(); r.class = HUB_BUILD.into(); r.master = true; }
                let upkeep = s.request(r, 3);
                assert!(grants(&s.tick(3)).is_empty(), "no preemption");
                if hub {
                    assert!(state(&s, &upkeep).1.contains(&format!("held by {running}")), "{:?}", state(&s, &upkeep));
                } else if whole {
                    assert!(state(&s, &upkeep).1.contains(&format!("held for whole {next}")), "{:?}", state(&s, &upkeep));
                } else {
                    // a box's prebuild runs only in a gap of its timed queue, and holds nothing
                    let l = s.get(&upkeep).unwrap();
                    let b = s.now_fits("box-a", l, &s.cost_on("box-a", l), &s.holds).unwrap_err();
                    assert_eq!(b.blockers, vec![next.clone()]);
                    assert_eq!(b.reason, format!("in a gap of box-a's timed queue: {next} of next waits for the box"));
                    assert_eq!(super::super::prof::blocker(&b.reason), "timed gap");
                    assert!(!s.holds.values().any(|h| h.id == upkeep), "{:?}", s.holds);
                }
                release_clean(&mut s, &running, 4);
                // the hub build goes first; the prebuild after the exclusive work
                let (first, then) = if hub { (upkeep.clone(), next.clone()) } else { (next.clone(), upkeep.clone()) };
                assert_eq!(grants(&s.tick(4)), vec![first.clone()], "hub={hub}, whole={whole}: {:?} {:?}", state(&s, &upkeep), state(&s, &next));
                assert_eq!(state(&s, &then).0, "queued");
                release_clean(&mut s, &first, 5);
                assert_eq!(grants(&s.tick(5)), vec![then]);
                invariants(&s);
            }
        }
        // An upkeep that cannot start must not stop a timed request that can.
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Bench))]);
        let mut r = req("prebuild-box-a", Kind::Build, Some("box-a"));
        r.needs = vec![("os".into(), "unavailable".into())];
        s.request(r, 1);
        let timed = s.request(req("bench", Kind::Timed, Some("box-a")), 2);
        assert_eq!(grants(&s.tick(2)), vec![timed]);
        // Upkeep retains its normal shared queue position on a pool box.
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        let lane = s.request(req("lane", Kind::Test, Some("box-a")), 1);
        let upkeep = s.request(req("prebuild-box-a", Kind::Build, Some("box-a")), 2);
        let order: Vec<_> = s.queue_order().iter().map(|&i| s.leases[i].id.clone()).collect();
        assert_eq!(order, vec![lane, upkeep]);
    }

    /// A box's hub build heads its box's queue, keeps no room for a live call, and starts now on
    /// the cores its box has free, down to four jobs: a hub on old code keeps the fleet behind on
    /// An override ends the box's lane work, never its own hub build (L515981, a node's, 11-06), and
    /// gives a lane no box with dev off.
    #[test]
    fn an_override_never_ends_its_boxs_hub_build() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        let mut hub = req(&format!("{HUB_BUILD}-box-a"), Kind::Build, Some("box-a"));
        hub.class = HUB_BUILD.into();
        hub.master = true;
        let hub = s.request(hub, 0);
        let small = |lane: &str| {
            let mut r = req(lane, Kind::Test, Some("box-a"));
            r.cores = Some(2.0);
            r.src = Src::Overlay;
            r
        };
        let (lane, given) = (s.request(small("lane"), 0), s.request(small("given"), 0));
        s.tick(0);
        let live = |s: &Sched, id: &str| s.get(id).unwrap().state.live();
        assert!([&hub, &lane, &given].iter().all(|id| live(&s, id)), "{:?}", [&hub, &lane, &given].map(|id| state(&s, id)));
        assert_eq!(s.override_node("box-a", Some("given"), 1), Ok(vec![lane.clone()]));
        assert_eq!(state(&s, &lane).0, "ended overridden");
        assert!(live(&s, &hub) && live(&s, &given));
        assert_eq!(s.override_node("box-a", None, 2), Ok(vec![given.clone()]), "to the user: the lanes' leases end, the hub build stays");
        assert!(live(&s, &hub));
        // a dev-off box is given to no lane: refused, saying what to do, and nothing ends
        s.set_dev("box-a", false, 3).unwrap();
        let e = s.override_node("box-a", Some("given"), 3).unwrap_err();
        assert!(e.starts_with("dev off: box-a") && e.contains("switch it on first"), "{e}");
        assert!(live(&s, &hub));
    }

    /// fixes (L484101 waited 10 min on a Linux node with 8 of 16 cores in use and 5 kept for a live call,
    /// 11-06). With fewer than four free it goes beside the box's work on four, whatever other features
    /// its hub reports: a Linux node's waited 10m48s on a box charged 16 of 16 (11-07).
    /// A box's own prebuild goes beside what runs there as its hub build does, on its charge's
    /// jobs: two background tests shards charged a Linux node's 16 cores while its 49 s prebuild
    /// headed the queue 20 min (11-08). Lane work of its size still waits for its cores, and a
    /// prebuild for another box is lane work here.
    #[test]
    fn a_boxs_prebuild_goes_beside_the_work_charged_its_cores() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool)), ("box-b", caps(16, 64.0, Role::Pool))]);
        for node in ["box-a", "box-b"] {
            for (lane, cores) in [("bg1", 12.0), ("bg2", 4.0)] {
                let mut bg = overlay(&format!("{lane}{node}"), Some(node), cores);
                bg.priority = Priority::Background;
                s.request(bg, 0);
                s.tick(0);
            }
            assert_eq!(s.charged_on(node).cores, 16.0);
        }
        let build = |lane: String, node: &str| {
            let mut r = req(&lane, Kind::Build, Some(node));
            r.cores = Some(4.0);
            r
        };
        let own = s.request(build(format!("{}box-a", super::super::PREBUILD_LANE_PREFIX), "box-a"), 1);
        let other = s.request(build(format!("{}box-c", super::super::PREBUILD_LANE_PREFIX), "box-b"), 1);
        // (an overlay: a build in the box's tree would wait its turn there)
        let mut lane = overlay("lane", Some("box-a"), 4.0);
        (lane.kind, lane.priority) = (Kind::Build, Priority::Interactive);
        let lane = s.request(lane, 1);
        beat_all(&mut s, 2);
        assert_eq!(grants(&s.tick(2)), vec![own.clone()], "{:?} {:?}", state(&s, &own), state(&s, &other));
        assert_eq!(s.get(&own).unwrap().cost.cores, 4.0, "on its charge's jobs");
        assert!(state(&s, &lane).0 == "queued" && state(&s, &lane).1.starts_with("budget cores 16, in use "), "{:?}", state(&s, &lane));
        assert!(state(&s, &other).0 == "queued" && state(&s, &other).1.starts_with("budget cores 16, in use 16"), "{:?}", state(&s, &other));
    }

    /// A box's prebuild of the landed tip holds its box against background work by the RAM it is
    /// charged: background asks stay off the RAM the box frees, interactive work still goes first
    /// (a pool box's prebuild waited 11.5 min for its RAM, 11-08).
    #[test]
    fn a_boxs_tip_prebuild_holds_its_box_against_background_ram() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        let budget = s.nodes["box-a"].caps.as_ref().map(|c| Req::new("x", Kind::Test).ram_budget(c)).unwrap();
        let ask = |s: &mut Sched, lane: &str, gb: f64, min: f64, at: u64| {
            let mut r = overlay(lane, Some("box-a"), 2.0);
            (r.ram_gb, r.est_min) = (Some(gb), Some(min));
            s.request(r, at)
        };
        let small = ask(&mut s, "small", 4.0, 30.0, 0);
        let big = ask(&mut s, "big", budget - 6.0, 30.0, 0);
        s.tick(0);
        let mut r = req(&format!("{}box-a", super::super::PREBUILD_LANE_PREFIX), Kind::Build, Some("box-a"));
        (r.cores, r.ram_gb) = (Some(4.0), Some(6.5));
        let pre = s.request(r, 1);
        beat_all(&mut s, 2);
        s.tick(2);
        assert_eq!(s.holds.get("box-a").map(|h| h.id.clone()), Some(pre.clone()), "{:?}", state(&s, &pre));
        // the RAM the box frees stays for it, not for background work asked after it; interactive
        // work goes first
        release_clean(&mut s, &small, 5 * MIN);
        let later = ask(&mut s, "later", 4.0, 30.0, 5 * MIN);
        let mut r = overlay("person", Some("box-a"), 2.0);
        (r.ram_gb, r.est_min, r.priority) = (Some(2.0), Some(2.0), Priority::Interactive);
        let person = s.request(r, 5 * MIN);
        beat_all(&mut s, 5 * MIN + 1);
        assert_eq!(grants(&s.tick(5 * MIN + 1)), vec![person.clone()], "{:?}", state(&s, &later));
        assert!(state(&s, &later).1.contains(&format!("held for {pre} (the box's prebuild of the landed tip")), "{:?}", state(&s, &later));
        invariants(&s);
        release_clean(&mut s, &big, 30 * MIN);
        beat_all(&mut s, 30 * MIN + 1);
        assert_eq!(grants(&s.tick(30 * MIN + 1)).first(), Some(&pre));
    }

    /// A box's prebuild is background work: nobody waits on it, so a live launch goes ahead of it
    /// as of any background work, and it shows as background in the queue.
    #[test]
    fn a_live_launch_goes_ahead_of_a_boxs_prebuild() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        let mut lane = overlay("lane", Some("box-a"), 12.0);
        lane.priority = Priority::Background;
        let lane = s.request(lane, 0);
        assert_eq!(grants(&s.tick(0)), vec![lane]);
        let mut r = overlay(&format!("{}box-a", super::super::PREBUILD_LANE_PREFIX), Some("box-a"), 4.0);
        (r.kind, r.priority) = (Kind::Build, Priority::Background);
        let pre = s.request(r, 1);
        beat_all(&mut s, 1);
        assert_eq!(grants(&s.tick(1)), vec![pre.clone()]);
        assert!(s.get(&pre).unwrap().req.upkeep());
        let mut r = overlay("live-W1", Some("box-a"), 13.0);
        (r.live, r.priority) = (true, Priority::Interactive);
        let session = s.request(r, 2);
        beat_all(&mut s, 2);
        assert_eq!(grants(&s.tick(2)), vec![session.clone()]);
        assert_eq!(s.past_budget(s.get(&session).unwrap()), 13.0, "12 + 4 + 13 of 16");
    }

    /// A live launch someone waits on goes first on the box it holds: the box's prebuild yields to
    /// it at once (it would hold the cores past the launch's start), and once it waited a minute
    /// newer work that would hold them past it waits too, even beside its room. It parks no other
    /// session. One asking 13 cores waited 11 min while background lane work and every box's
    /// prebuild took the cores the boxes freed (11-08).
    #[test]
    fn a_live_launch_goes_first_on_its_box() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        let lane = |s: &mut Sched, name: &str, cores: f64, min: f64, at: u64| {
            let mut r = overlay(name, Some("box-a"), cores);
            r.est_min = Some(min);
            // (what it waits for is work it doesn't go ahead of; `late` is work that comes after it)
            if name != "late" { r.priority = Priority::Interactive; }
            s.request(r, at)
        };
        let shard = lane(&mut s, "shard", 8.0, 20.0, 0);
        let quick = lane(&mut s, "quick", 4.0, 1.0, 0);
        assert_eq!(grants(&s.tick(0)).len(), 2);
        let mut r = overlay("live-w1", Some("box-a"), 5.0);
        (r.live, r.priority) = (true, Priority::Interactive);
        let launch = s.request(r, 1_000);
        beat_all(&mut s, 1_000);
        assert!(grants(&s.tick(1_000)).is_empty(), "12 of 16 in use");
        // the prebuild fits now, and would hold its 4 cores past the launch's start
        let mut r = req(&format!("{}box-a", super::super::PREBUILD_LANE_PREFIX), Kind::Build, Some("box-a"));
        (r.cores, r.est_min) = (Some(4.0), Some(2.0));
        let pre = s.request(r, 2_000);
        beat_all(&mut s, 3_000);
        assert!(grants(&s.tick(3_000)).is_empty(), "{:?}", state(&s, &pre));
        assert!(state(&s, &pre).1.contains(&format!("held for interactive {launch}")), "{:?}", state(&s, &pre));
        // a minute in, background work that fits beside its room waits too
        let late = lane(&mut s, "late", 3.0, 30.0, MIN + 1_000);
        beat_all(&mut s, MIN + 2_000);
        let a = s.tick(MIN + 2_000);
        assert!(grants(&a).is_empty(), "{:?}", state(&s, &late));
        assert!(state(&s, &late).1.contains("it has queued 1m01s: the box drains for it"), "{:?}", state(&s, &late));
        assert!(a.iter().all(|x| !matches!(x, Action::ParkLive { .. })), "{a:?}");
        release_clean(&mut s, &quick, MIN + 3_000);
        beat_all(&mut s, MIN + 4_000);
        assert_eq!(grants(&s.tick(MIN + 4_000)).first(), Some(&launch), "8 + 5 of 16");
        assert_eq!(state(&s, &shard).0, "starting", "the shard runs on");
    }

    /// A starved live launch holds the box where it can start once that box's work ends: the room
    /// a box keeps for its idle session's next call is no end it waits for, as the launch parks no
    /// session. It held the box whose tests shard ended first, beside an idle session whose room
    /// left it no start, and waited on while other boxes freed their cores (11-08).
    #[test]
    fn a_starved_live_launch_holds_a_box_it_can_start_on() {
        const MIN: u64 = 60_000;
        let mut mac = caps(16, 64.0, Role::Pool);
        mac.os = "macos".into();
        let mut s = sched(&[("a", mac.clone()), ("c", mac)]);
        let overlay = |lane: &str, node: Option<&str>, cores: f64| {
            let mut r = overlay(lane, node, cores);
            r.needs = vec![("os".into(), "macos".into())];
            r
        };
        // c: an idle live session (5 cores, 1 between calls) and a tests shard that ends first
        let mut r = overlay("live-w0", Some("c"), 5.0);
        r.live = true;
        let session = s.request(r, 0);
        assert_eq!(grants(&s.tick(0)), vec![session.clone()]);
        s.live_idle.insert(session.clone());
        let shard = |s: &mut Sched, node: &str, cores: f64, min: f64| {
            let mut r = overlay(TESTS_LANE, Some(node), cores);
            (r.est_min, r.priority) = (Some(min), Priority::Background);
            s.request(r, 0)
        };
        let shard_c = shard(&mut s, "c", 10.0, 3.0);
        let shard_a = shard(&mut s, "a", 12.0, 4.0);
        let mut r = overlay("small", Some("a"), 1.0);
        r.est_min = Some(10.0);
        s.request(r, 0);
        beat_all(&mut s, 0);
        s.tick(0);
        beat_all(&mut s, 500);
        s.tick(500);
        assert!([&shard_a, &shard_c].iter().all(|x| state(&s, x).0 == "starting"));
        let mut r = overlay("live-w1", None, 15.0);
        (r.live, r.priority) = (true, Priority::Interactive);
        let launch = s.request(r, 1_000);
        beat_all(&mut s, MIN + 2_000);
        let a = s.tick(MIN + 2_000);
        assert!(grants(&a).is_empty());
        assert_eq!(s.holds.get("a").map(|h| h.id.clone()), Some(launch.clone()), "a: 1 + 15 of 16 once its shard ends; c: 1 + 4 kept for a call + 15");
        // the master's log says it: which box, when it starts there and what it waits on
        let logged: Vec<&String> = a.iter().filter_map(|x| match x { Action::Log(line) => Some(line), _ => None }).collect();
        assert_eq!(logged, vec![&format!("{launch} (live-w1 live launch, 15 cores, queued 1m01s) holds a: starts in 2m58s by its measure; waits there on budget cores 16, in use 13, asked 15")]);
        beat_all(&mut s, MIN + 3_000);
        assert!(s.tick(MIN + 3_000).iter().all(|x| !matches!(x, Action::Log(_))), "said once");
        assert!(!s.holds.get("c").is_some_and(|h| h.id == launch));
        release_clean(&mut s, &shard_c, 3 * MIN);
        beat_all(&mut s, 3 * MIN + 500);
        assert!(grants(&s.tick(3 * MIN + 500)).is_empty(), "{:?}", state(&s, &launch));
        release_clean(&mut s, &shard_a, 4 * MIN);
        beat_all(&mut s, 4 * MIN + 500);
        assert_eq!(grants(&s.tick(4 * MIN + 500)), vec![launch.clone()]);
        assert_eq!(node_of(&s, &launch).as_deref(), Some("a"));
        invariants(&s);
    }

    /// A starved ask behind a live session has no end to wait for, and still tells its box. Told
    /// as `u64::MAX`, its state line was no JSON its holder could read: from its first minute each
    /// beat read "beat: no answer", and the live call failed after three asks (W4355, 11-09).
    #[test]
    fn a_starved_ask_with_no_end_known_still_answers_in_json() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = overlay("live-w0", Some("a"), 10.0);
        r.live = true;
        let session = s.request(r, 0);
        assert_eq!(grants(&s.tick(0)), vec![session]);
        let mut r = overlay("live-w1", None, 15.0);
        (r.live, r.priority) = (true, Priority::Interactive);
        let launch = s.request(r, 1_000);
        beat_all(&mut s, MIN + 2_000);
        assert!(grants(&s.tick(MIN + 2_000)).is_empty());
        let line = s.get(&launch).unwrap().state_line(MIN + 2_000, "");
        let v = makepad_strict_json::parse(line.as_bytes()).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(v.get("eta_box").and_then(|x| x.as_str()), Some("a"), "{line}");
        assert_eq!(v.get("eta_ms").and_then(|x| x.as_u64()), Some(0), "{line}");
        invariants(&s);
    }

    /// A held head keeps its box through each lease end there: the cleanup the box has yet to
    /// confirm ends in its teardown time, as the head's estimate counts it. Counted as staying,
    /// a 15-core launch's hold moved at each end to a box 53 min off and back once the cleanup
    /// confirmed, four times in a minute, while short builds refilled its box (W4322, 11-08).
    #[test]
    fn a_held_head_keeps_its_box_through_a_lease_cleanup_there() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("b", caps(16, 64.0, Role::Pool)), ("g", caps(16, 64.0, Role::Pool))]);
        let mut r = overlay(TESTS_LANE, Some("g"), 12.0);
        (r.est_min, r.priority) = (Some(60.0), Priority::Interactive);
        let shard = s.request(r, 0);
        let mut r = overlay("bench", Some("b"), 4.0);
        (r.est_min, r.priority) = (Some(15.0), Priority::Interactive);
        let bench = s.request(r, 0);
        s.tick(0);
        assert!(s.started(&shard, "{}") && s.started(&bench, "{}"));
        let mut r = overlay("live-w1", None, 15.0);
        (r.live, r.priority) = (true, Priority::Interactive);
        let launch = s.request(r, 1_000);
        let held = |s: &Sched| s.holds.iter().filter(|(_, h)| h.id == launch).map(|(n, _)| n.clone()).collect::<Vec<_>>();
        let mut t = MIN + 2_000;
        beat_all(&mut s, t);
        s.tick(t);
        assert_eq!(held(&s), vec!["b".to_string()], "b starts it once its bench ends");
        // (measured: the box drains for the launch, and takes work that ends first by its measure)
        for _ in 0..3 { s.record_node_cost("test builds", "test", "b", 9_000); }
        for k in 0..4 {
            // a short build that ends first comes in beside it, ends, and its cleanup confirms
            let mut r = overlay("builds", Some("b"), 5.0);
            (r.est_min, r.class) = (Some(0.15), "test builds".into());
            let build = s.request(r, t);
            t += 500;
            beat_all(&mut s, t);
            assert_eq!(grants(&s.tick(t)), vec![build.clone()], "{k}: it ends first");
            s.started(&build, "{}");
            t += 8_000;
            s.release(&build, "released", t);
            for confirmed in [false, true] {
                if confirmed { s.cleaned("b", &build, 2_000); }
                t += 300;
                beat_all(&mut s, t);
                let a = s.tick(t);
                assert_eq!(held(&s), vec!["b".to_string()], "{k}: cleanup confirmed {confirmed}");
                assert!(a.iter().all(|x| !matches!(x, Action::Log(_))), "{k}: no move said: {a:?}");
            }
        }
        // a cleanup past its end and the box's 2 s teardown is an open end: nothing goes in on
        // the window it left (counted from now, it opened again at each look) until it confirms
        let build = |s: &mut Sched, t| {
            let mut r = overlay("builds", Some("b"), 5.0);
            (r.est_min, r.class) = (Some(0.15), "test builds".into());
            s.request(r, t)
        };
        let ended = build(&mut s, t);
        t += 500;
        beat_all(&mut s, t);
        assert_eq!(grants(&s.tick(t)), vec![ended.clone()]);
        s.started(&ended, "{}");
        t += 8_000;
        s.release(&ended, "released", t);
        t += 3_000;
        let late = build(&mut s, t);
        beat_all(&mut s, t + 500);
        assert!(grants(&s.tick(t + 500)).is_empty(), "behind an unconfirmed cleanup: {:?}", state(&s, &late));
        s.cleaned("b", &ended, 2_000);
        t += 1_000;
        beat_all(&mut s, t);
        assert_eq!(grants(&s.tick(t)), vec![late.clone()], "its cleanup confirmed: it ends first");
        s.started(&late, "{}");
        release_clean(&mut s, &late, t + 8_000);
        release_clean(&mut s, &bench, 15 * MIN);
        beat_all(&mut s, 15 * MIN + 500);
        assert_eq!(grants(&s.tick(15 * MIN + 500)), vec![launch.clone()]);
        assert_eq!(node_of(&s, &launch).as_deref(), Some("b"));
        invariants(&s);
    }

    /// A held head moves to another box only when that one starts it sooner by [`HOLD_MOVE_MS`]:
    /// estimates move as work runs past its measure, starts and ends, and a hold that follows each
    /// move drains neither box.
    #[test]
    fn a_held_head_moves_only_for_a_start_sooner_by_a_margin() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        let run = |s: &mut Sched, node: &str, min: f64| {
            let mut r = overlay(&format!("lane-{node}"), Some(node), 12.0);
            (r.est_min, r.priority) = (Some(min), Priority::Interactive);
            s.request(r, 0)
        };
        let on_a = run(&mut s, "a", 3.0);
        let on_c = run(&mut s, "c", 6.0);
        s.tick(0);
        assert!(s.started(&on_a, "{}") && s.started(&on_c, "{}"));
        let mut r = overlay("live-w1", None, 15.0);
        (r.live, r.priority) = (true, Priority::Interactive);
        let launch = s.request(r, 0);
        let held = |s: &mut Sched, t: u64| {
            beat_all(s, t);
            s.tick(t);
            s.holds.iter().filter(|(_, h)| h.id == launch).map(|(n, _)| n.clone()).collect::<Vec<_>>()
        };
        assert_eq!(held(&mut s, MIN + 1_000), vec!["a".to_string()], "a in 2 min, c in 5");
        // past its measure, a's run is said to take its whole measure again: c starts it sooner,
        // first by less than the margin, then by more
        assert_eq!(held(&mut s, 4 * MIN + 30_000), vec!["a".to_string()], "a in 3 min, c in 1m30s");
        assert_eq!(held(&mut s, 5 * MIN + 30_000), vec!["c".to_string()], "a in 3 min, c in 30 s");
        invariants(&s);
    }

    /// A tests shard's box drains for it from its ask: what ran there before counts, interactive
    /// work's too, and only work its class measured to end before the shard starts comes in beside
    /// it. Two shards waited 16 min for 4 cores each while lane rigs came in beside their room
    /// (L662711/12, 11-08); a day's shards started up to 34 min after the work on their box from
    /// before their ask had left their room, with interactive work let in since (L794661, 11-10).
    #[test]
    fn a_tests_shards_box_drains_for_it_from_its_ask() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = overlay("rig", Some("a"), 10.0);
        r.est_min = Some(20.0);
        let rig = s.request(r, 0);
        s.tick(0);
        let mut shard = overlay(TESTS_LANE, None, 8.0);
        shard.est_min = Some(20.0);
        let shard = s.request(shard, 6 * MIN);
        beat_all(&mut s, 6 * MIN + 1);
        s.tick(6 * MIN + 1);
        assert_eq!(s.holds.get("a").map(|h| h.id.clone()), Some(shard.clone()), "{:?}", state(&s, &shard));
        for _ in 0..3 { s.record_node_cost("quick", "test", "a", MIN); }
        let ask = |lane: &str, class: &str, priority: Priority| {
            let mut r = overlay(lane, None, 4.0);
            (r.est_min, r.priority, r.class) = (Some(2.0), priority, class.into());
            r
        };
        let lane = s.request(ask("lane", "", Priority::Background), 6 * MIN + 2);
        let person = s.request(ask("person", "", Priority::Interactive), 6 * MIN + 2);
        let quick = s.request(ask("quick", "quick", Priority::Interactive), 6 * MIN + 2);
        beat_all(&mut s, 6 * MIN + 3);
        assert_eq!(grants(&s.tick(6 * MIN + 3)), vec![quick.clone()], "measured to end before the rig: {:?}", state(&s, &quick));
        for id in [&lane, &person] {
            assert!(state(&s, id).1.contains(&format!("held for tests shard {shard}")), "interactive too: {:?}", state(&s, id));
        }
        release_clean(&mut s, &quick, 7 * MIN);
        release_clean(&mut s, &rig, 9 * MIN);
        beat_all(&mut s, 9 * MIN + 1);
        assert!(grants(&s.tick(9 * MIN + 1)).contains(&shard), "{:?}", state(&s, &shard));
        invariants(&s);
    }

    /// devhub's own builds that go past a box's cores (a hub build, a deployment's build, the
    /// box's prebuild) go one at a time: the next waits for its room or for that one to end. The
    /// box's hub build goes past beside another: the box's roll waits for it (L791258, 11-10).
    #[test]
    fn devhubs_own_builds_go_past_a_boxs_cores_one_at_a_time_but_its_hub_build() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_resources(HUB_BUILD, "build", Some(3.0), Some(7.0)); }
        let mut work = req("work", Kind::Test, Some("box-a"));
        (work.cores, work.src) = (Some(12.0), Src::Overlay);
        s.request(work, 0);
        assert_eq!(grants(&s.tick(0)).len(), 1);
        let own = |lane: String, hub: bool| {
            let mut r = devhubs(req(&lane, Kind::Build, Some("box-a")));
            r.cores = Some(4.0);
            if hub { (r.class, r.master) = (HUB_BUILD.into(), true); }
            r
        };
        let mut asked = vec![
            s.request(own(format!("{DEPLOY_BUILD}-box-a"), true), 1),
            s.request(own(format!("{}box-a", super::super::PREBUILD_LANE_PREFIX), false), 1),
        ];
        beat_all(&mut s, 1);
        let first = grants(&s.tick(1));
        assert_eq!(first.len(), 1, "one at a time past the box's cores: {first:?}");
        asked.retain(|id| *id != first[0]);
        assert!(state(&s, &asked[0]).1.contains("budget cores 16"), "{:?}", state(&s, &asked[0]));
        let hub = s.request(own(format!("{HUB_BUILD}-box-a"), true), 2);
        beat_all(&mut s, 2);
        assert_eq!(grants(&s.tick(2)), vec![hub.clone()], "the box's hub build goes past beside it: {:?}", state(&s, &hub));
        assert!(s.charged_on("box-a").cores <= 16.0 + 8.0, "{} cores charged", s.charged_on("box-a").cores);
        assert!(state(&s, &asked[0]).1.contains("budget cores 16"), "the third waits: {:?}", state(&s, &asked[0]));
        release_clean(&mut s, &first[0], 3);
        release_clean(&mut s, &hub, 3);
        beat_all(&mut s, 4);
        assert_eq!(grants(&s.tick(4)), asked);
        invariants(&s);
    }

    #[test]
    fn a_hub_build_heads_its_queue_and_starts_on_the_cores_its_box_has_free() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_resources(HUB_BUILD, "build", Some(3.0), Some(7.0)); }
        let hub_ask = || {
            let mut r = req(&format!("{HUB_BUILD}-box-a"), Kind::Build, Some("box-a"));
            r.class = HUB_BUILD.into();
            r.master = true;
            r
        };
        let ask = |lane: &str, cores: f64| {
            let mut r = req(lane, Kind::Test, Some("box-a"));
            r.cores = Some(cores);
            r.src = Src::Overlay;
            r
        };
        let cores = |s: &Sched, id: &str| s.get(id).unwrap().cost.cores;
        let cap = |s: &Sched| assert!(s.charged_on("box-a").cores <= 16.0, "{} of 16 cores charged", s.charged_on("box-a").cores);
        // 10 of 16 cores granted, then an idle live session that keeps room for its next call
        s.request(ask("work", 9.0), 0);
        let mut live = ask("live", 4.0);
        live.live = true;
        let live = s.request(live, 0);
        assert_eq!(grants(&s.tick(0)).len(), 2);
        s.live_idle.insert(live.clone());
        let in_use = s.charged_on("box-a").cores;
        assert!(in_use < 13.0, "the idle session is charged less than its grant: {in_use}");
        // an older lane ask for the box, then the hub build: the build goes first, on what is free
        let lane = s.request(ask("lane", 6.0), 1);
        let hub = s.request(hub_ask(), 2);
        beat_all(&mut s, 2);
        assert_eq!(grants(&s.tick(2)), vec![hub.clone()], "the hub build heads its box's queue and keeps no room for a live call: {:?}", state(&s, &hub));
        assert_eq!(cores(&s, &hub), (16.0 - in_use).floor().min(7.0), "it takes the free cores when fewer than it measured");
        cap(&s);
        assert!(state(&s, &lane).0 == "queued", "the lane's ask waits: {:?}", state(&s, &lane));
        // five free: on five jobs
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_resources(HUB_BUILD, "build", Some(3.0), Some(7.0)); }
        s.request(ask("a", 11.0), 0);
        assert_eq!(grants(&s.tick(0)).len(), 1);
        let hub = s.request(hub_ask(), 1);
        beat_all(&mut s, 1);
        assert!(grants(&s.tick(1)).contains(&hub), "{:?}", state(&s, &hub));
        assert_eq!((cores(&s, &hub), s.hub_build_jobs(&hub)), (5.0, Some(5)), "on five jobs");
        cap(&s);
        // on a box with all it measured free it takes that, on cargo's own jobs
        let mut idle = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { idle.record_resources(HUB_BUILD, "build", Some(3.0), Some(7.0)); }
        let hub = idle.request(hub_ask(), 0);
        beat_all(&mut idle, 0);
        assert!(grants(&idle.tick(0)).contains(&hub), "{:?}", state(&idle, &hub));
        assert_eq!((cores(&idle, &hub), idle.hub_build_jobs(&hub)), (7.0, None));
        // three free, or none: it builds beside the box's work at once, on four jobs, past its
        // box's cores for the build, with or without `deploy-jobs`; the box's
        // freed cores are its while it waits for nothing
        for (deploy_jobs, held) in [(true, 13.0), (false, 13.0), (true, 16.0)] {
            let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
            if !deploy_jobs { s.nodes.get_mut("box-a").unwrap().caps.as_mut().unwrap().features.retain(|f| f != "deploy-jobs"); }
            for _ in 0..3 { s.record_resources(HUB_BUILD, "build", Some(3.0), Some(7.0)); }
            s.request(ask("a", held - 2.0), 0);
            s.request(ask("b", 2.0), 0);
            assert_eq!(grants(&s.tick(0)).len(), 2);
            let later = s.request(ask("later", 1.0), 1);
            let hub = s.request(hub_ask(), 1);
            beat_all(&mut s, 1);
            assert!(grants(&s.tick(1)).contains(&hub), "a box's hub build waits for no room ({held} of 16 charged, deploy-jobs {deploy_jobs}): {:?}", state(&s, &hub));
            assert_eq!((cores(&s, &hub), s.hub_build_jobs(&hub)), (4.0, Some(4)), "on four jobs");
            assert_eq!(s.charged_on("box-a").cores, held + 4.0);
            assert_eq!(state(&s, &later).0, "queued", "lane work goes after it: {:?}", state(&s, &later));
        }
        // its RAM by the box's measure: a shard charged past the budget at its class's peak, the
        // box measured mostly free, builds at once; with too little available it waits, saying so
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_resources(HUB_BUILD, "build", Some(3.0), Some(7.0)); }
        let mut shard = ask("shard", 8.0);
        shard.ram_gb = Some(54.0);
        let shard = s.request(shard, 0);
        assert_eq!(grants(&s.tick(0)), vec![shard]);
        let budget = s.get(&s.leases[0].id).unwrap().req.ram_budget(s.nodes["box-a"].caps.as_ref().unwrap());
        assert!(s.charged_on("box-a").ram_gb + 3.6 > budget, "charged past what the build would fit");
        let hub = s.request(hub_ask(), 1);
        beat_all(&mut s, 1);
        assert!(grants(&s.tick(1)).contains(&hub), "the box measured 51 GB available: {:?}", state(&s, &hub));
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_resources(HUB_BUILD, "build", Some(3.0), Some(7.0)); }
        s.nodes.get_mut("box-a").unwrap().caps.as_mut().unwrap().ram_avail_gb = 9.0;
        let hub = s.request(hub_ask(), 1);
        beat_all(&mut s, 1);
        assert!(grants(&s.tick(1)).is_empty());
        assert!(state(&s, &hub).1.contains("ram: 9.0 GB available less its 10% margin, asked 3.6 GB (a hub build goes by the box's measure)"), "{:?}", state(&s, &hub));
    }

    /// A box weighs a class on its OS family's own peaks once it has any, every box's being their
    /// starting estimate: a Fab session weighed 8.9 GB on a 16 GB Mac from Linux sessions' peaks,
    /// all of its budget, while Mac sessions peaked at 2-5.4 GB (11-06).
    #[test]
    fn a_box_weighs_a_class_on_its_os_familys_own_peaks() {
        let mut mac = caps(10, 16.0, Role::Pool);
        mac.os = "macos".into();
        let mut s = sched(&[("lin", caps(16, 64.0, Role::Pool)), ("mac", mac)]);
        let mut r = req("fab", Kind::Test, None);
        r.class = "live fab".into();
        let id = s.request(r, 0);
        let ram = |s: &Sched, name: &str| s.cost_on(name, s.get(&id).unwrap()).ram_gb;
        for gb in [7.4, 5.0] { s.record_resources("live fab", "test", Some(gb), None); }
        let budget = s.get(&id).unwrap().req.ram_budget(s.nodes["mac"].caps.as_ref().unwrap());
        assert!((ram(&s, "mac") - (7.4f64 * 1.2).min(budget)).abs() < 1e-9 && (ram(&s, "lin") - 7.4 * 1.2).abs() < 1e-9, "no family peaks: every box's ({}, {})", ram(&s, "mac"), ram(&s, "lin"));
        s.record_family_peak("macos", "live fab", "test", Some(4.5));
        assert!((ram(&s, "mac") - 4.5 * 1.2).abs() < 1e-9, "the Macs' own: {}", ram(&s, "mac"));
        assert!((ram(&s, "lin") - 7.4 * 1.2).abs() < 1e-9, "Linux has none of its own yet: {}", ram(&s, "lin"));
        s.record_family_peak("linux", "live fab", "test", Some(3.0));
        assert!((ram(&s, "lin") - 3.0 * 1.2).abs() < 1e-9, "{}", ram(&s, "lin"));
    }

    /// Landings every 2 min, a deployment apply of 3 min, timed runs of 12 min with the next one
    /// always asked: an update before each run kept a timed ask waiting behind the stream (37 min
    /// at the head of its queue, 11-04). Each starts as the last ends, on what the box has; the
    /// box catches up once when it fell 30 min behind, and when its queue empties.
    #[test]
    fn a_timed_queue_stays_on_what_the_box_has_until_it_falls_30_min_behind() {
        const MIN: u64 = 60_000;
        let at = |n: u64| format!("{n:040},commercial={n:040}");
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
                let view = s.node_view(0).into_iter().find(|n| n.name == "b1").unwrap();
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

    /// Three timed asks for a bench box whose update is pending: B waits for the box's catch-up
    /// (its deploy pin), A (asked first) and C (asked last) take it as it is. A runs on what the
    /// box has, the deployment takes the gap after it, then B, then C: by ask time. C took the box
    /// as A's run ended, before the update could (it waits for each timed run, then for the next
    /// look), and the update waited for C's run in turn: jitlegs' warm leg waited 24 min at the
    /// head of its queue so (11-08).
    #[test]
    fn a_timed_ask_waiting_for_its_boxs_catch_up_keeps_its_place_and_the_update_takes_the_gap() {
        const MIN: u64 = 60_000;
        // the digit first: a reason names a deployment by its first 10 hex digits
        let at = |n: u64| format!("{n}{:039},commercial={n}{:039}", 0, 0);
        // the box says what it has from the first look: with no deployment heard, A was in line
        // with nothing to hold, and the box went on to the tips before any run
        let mut deploy = at(0);
        let mut c0 = caps(16, 64.0, Role::Bench);
        c0.deploy = deploy.clone();
        let mut s = sched(&[("b1", c0)]);
        (s.tips, s.landed) = (at(1), vec![(at(0), 0), (at(1), 0)]);
        let mut applying: Option<(u64, String)> = None;
        let ids: Vec<String> = ["a", "b", "c"].iter().map(|lane| s.request(req(lane, Kind::Timed, Some("b1")), 0)).collect();
        s.leases.iter_mut().find(|l| l.id == ids[1]).unwrap().deploy_pin = Some(at(1));
        let mut running: Option<(String, u64)> = None;
        let mut starts: Vec<(&str, u64, String)> = Vec::new();
        for step in 0..=60u64 {
            let now = step * MIN / 2;
            // the run ends: its release's own pass places whoever the box can take now, before
            // the box's updater looks again
            if let Some((l, _)) = running.clone().filter(|(_, end)| *end <= now) {
                release_clean(&mut s, &l, now);
                running = None;
            }
            if now == 5 * MIN {
                // A ended; B's update still waits for the next look: C says what it waits behind
                let (st, why) = state(&s, &ids[2]);
                assert_eq!(st, "queued", "{why}");
                assert!(why.contains(&format!("behind a catch-up: {} of b (asked first) waits for b1's deployment at 1000000000,commercial=1000000000", ids[1])), "{why}");
                assert_eq!(super::super::prof::blocker(&why), "behind a catch-up");
            }
            if let Some((_, to)) = applying.clone().filter(|(end, _)| *end <= now) {
                deploy = to;
                applying = None;
            }
            for (i, id) in ids.iter().enumerate() {
                if running.is_none() && s.get(id).is_some_and(|l| l.state == State::Starting) {
                    s.started(id, "{}");
                    starts.push((["A", "B", "C"][i], now / MIN, s.get(id).unwrap().deployment.clone()));
                    running = Some((id.clone(), now + 5 * MIN));
                }
            }
            // the box's updater (the auto-sync, the roll): it waits for a running timed lease
            let target = match deploy_to(s.update_pin("b1"), &deploy, |d, a| d >= a) {
                DeployTo::Tips => Some(at(1)),
                DeployTo::Pin(p) => Some(p),
                DeployTo::Held { .. } => None,
            };
            s.update_waits.clear();
            match target.filter(|t| *t != deploy && applying.is_none()) {
                Some(_) if running.is_some() => { s.update_waits.entry("b1".into()).or_default().insert("deployment", "its deployment moves to 0000000001".into()); }
                Some(t) => applying = Some((now + 3 * MIN, t)),
                None => {}
            }
            let mut c = caps(16, 64.0, Role::Bench);
            (c.deploy, c.applying) = (deploy.clone(), applying.is_some());
            s.set_caps("b1", c, now);
            beat_all(&mut s, now);
            s.tick(now);
            for (i, id) in ids.iter().enumerate() {
                if running.is_none() && s.get(id).is_some_and(|l| l.state == State::Starting) {
                    s.started(id, "{}");
                    starts.push((["A", "B", "C"][i], now / MIN, s.get(id).unwrap().deployment.clone()));
                    running = Some((id.clone(), now + 5 * MIN));
                }
            }
        }
        // A on what the box had; the update in the gap after it (5-8 min); B, then C, on it
        assert_eq!(starts, [("A", 0, at(0)), ("B", 8, at(1)), ("C", 13, at(1))], "{starts:?}");
    }

    #[test]
    fn how_far_behind_a_box_is_counts_from_the_first_landing_it_lacks() {
        const MIN: u64 = 60_000;
        let at = |n: u64| format!("{n:040},commercial={n:040}");
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
        let pin = format!("{},commercial={}", "a".repeat(40), "c".repeat(40));
        let other = format!("{},commercial={}", "b".repeat(40), "d".repeat(40));
        let mut a = caps(16, 64.0, Role::Pool);
        a.tree = other.clone();
        a.deploy = pin.clone();
        let mut b = a.clone();
        b.tree = pin;
        b.deploy = other;
        let mut s = sched(&[("node-b", a), ("node-a", b)]);
        let mut r = req("ci", Kind::Ci, None);
        r.rev = Some(format!("work@{},commercial=main@{}", "a".repeat(40), "c".repeat(40)));
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
            let mut r = devhubs(req("ci", Kind::Ci, Some("node-b")));
            r.needs = parse_needs("gui=1");
            let gui = s.request(r, 0);
            assert!(grants(&s.tick(0)).is_empty());
            assert_eq!(state(&s, &gui).0, "ended denied");
            let plain = s.request(devhubs(req("ci", Kind::Ci, Some("node-b"))), 1);
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
        at.tree = format!("{other},commercial={}", "c".repeat(40));
        at.native = vec!["x".into()];
        let mut off = caps(16, 64.0, Role::Pool);
        off.tree = format!("{sha},commercial={}", "c".repeat(40));
        let mut s = sched(&[("at", at), ("off", off)]);
        let mut r = req("x", Kind::Test, None);
        r.rev = Some(format!("rc-x@{sha},commercial=main@{}", "c".repeat(40)));
        let x = s.request(r.clone(), 0);
        s.tick(0);
        assert_eq!(node_of(&s, &x).as_deref(), Some("at"));
        // the lane's tree is held at another pin: the other free node takes it
        let mut r2 = r.clone();
        r2.rev = Some(format!("rc-x@{other},commercial=main@{}", "c".repeat(40)));
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

    /// [`Sched::overdue`]: a running lease past 5x its measure, its class's measured run or what it
    /// declared; a live session and a class with no measure are over nothing.
    #[test]
    fn a_lease_past_five_times_its_measure_is_overdue() {
        let ids = |s: &Sched, now: u64| s.overdue(now, 5).into_iter().map(|(l, ran, measure, what)| (l.req.lane, ran, measure, what)).collect::<Vec<_>>();
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_node_cost("test", "test", "a", 120_000); }
        let slow = s.request(req("slow", Kind::Test, Some("a")), 0);
        let mut live = req("live", Kind::Test, Some("b"));
        live.live = true;
        let live = s.request(live, 0);
        let mut declared = req("declared", Kind::Build, Some("c"));
        declared.est_min = Some(2.0);
        let declared = s.request(declared, 0);
        assert_eq!(grants(&s.tick(0)).len(), 3);
        for id in [&slow, &live, &declared] {
            s.started(id, "{}");
            assert_eq!(state(&s, id).0, "granted", "{id}");
        }
        assert_eq!(ids(&s, 600_000), vec![], "5x its 2 min, not past it");
        assert_eq!(ids(&s, 600_001), vec![("slow".to_string(), 600_001, 120_000, "its measured 2m00s".to_string()), ("declared".to_string(), 600_001, 120_000, "its declared 2m00s".to_string())]);
    }

    /// A run whose box builds its JIT first (its first run says `+jit-build`) is held to its warm
    /// class's measure plus that box's measured JIT build, not to its kind's length, and so is
    /// what it declared: a lane's lib test measured 11m27s warm learned the class at its run and
    /// was called stalled at 10 min while cargo rebuilt the runner (L669068, 11-08). The asks behind
    /// it count on the same figure. A warm run under the 5 min floor is held to the floor, said
    /// as its kind's.
    #[test]
    fn a_run_that_builds_its_jit_first_is_held_to_its_warm_run_and_its_boxs_build() {
        let warned = |a: &[Action], id: &str| a.iter().filter(|a| matches!(a, Action::Event(r) if r.sev == Sev::Warn && r.line.contains("stalled?") && r.line.contains(id))).count();
        let captured = |a: &[Action], lease: &str, what: &str| a.iter().any(|a| matches!(a, Action::Capture { id, why, .. } if id == lease && why.contains(what)));
        let built = format!("test makepad-foo{}", super::super::JIT_BUILD_RUN);
        let quick_built = format!("test makepad-quick{}", super::super::JIT_BUILD_RUN);
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 {
            s.record_node_cost("test makepad-foo", "test", "a", 600_000);
            s.record_node_cost("test makepad-quick", "test", "c", 120_000);
            s.record_node_cost("jit-build", "", "a", 240_000);
            s.record_node_cost("jit-build", "", "b", 60_000);
            s.record_node_cost("jit-build", "", "c", 60_000);
        }
        let mut r = req("tree", Kind::Test, Some("a"));
        r.class = "test makepad-foo".into();
        let tree = s.request(r, 0);
        let mut r = req("declared", Kind::Test, Some("b"));
        r.est_min = Some(5.0);
        let declared = s.request(r, 0);
        let mut r = req("quick", Kind::Test, Some("c"));
        r.class = "test makepad-quick".into();
        let quick = s.request(r, 0);
        assert_eq!(grants(&s.tick(0)).len(), 3);
        for id in [&tree, &declared] {
            s.started(id, "{}");
            s.set_info(id, &format!(r#"{{"class":"{built}"}}"#), 0);
        }
        s.started(&quick, "{}");
        s.set_info(&quick, &format!(r#"{{"class":"{quick_built}"}}"#), 0);
        assert_eq!(s.get(&tree).unwrap().req.class(), built, "its first run's class");
        let held = |s: &Sched, id: &str| s.run_ms(s.get(id).unwrap());
        assert_eq!((held(&s, &tree), held(&s, &declared)), (840_000, 360_000), "10 min warm + a's 4 min build; 5 min declared + b's 1 min");
        beat_all(&mut s, 28 * 60_000);
        let a = s.tick(28 * 60_000);
        assert_eq!((warned(&a, &tree), warned(&a, &declared)), (0, 1), "twice its 14 min is no stall yet; past twice its 6 min is: {a:?}");
        assert!(captured(&a, &declared, "against its declared 5m00s and its box's JIT build 1m00s"), "{a:?}");
        assert!(captured(&a, &quick, "against its kind's 5m00s and its box's JIT build 1m00s (its class has no measure yet)"), "its warm 2 min is under the floor: {a:?}");
        beat_all(&mut s, 28 * 60_000 + 1_000);
        let a = s.tick(28 * 60_000 + 1_000);
        assert_eq!(warned(&a, &tree), 1);
        assert!(captured(&a, &tree, "against its warm run's 10m00s and its box's JIT build 4m00s (its class has no measure yet)"), "{a:?}");
    }

    /// What a lease runs now is what its stall check judges. While its box builds its JIT (its
    /// `jit` phase open), it goes by what that build does, not by how long others took: against
    /// its box's 32 s measure a healthy cold build read as stalled at 5m33s (L702786, 11-09). Every
    /// process idle says so. Once built, its command is held to its warm run from the build's end:
    /// held from its grant to warm run and build together, a small test lease's 12 min runner
    /// rebuild was its test's run (L693116, 11-08).
    #[test]
    fn a_lease_building_its_jit_is_judged_by_what_it_does_then_its_command_by_its_run() {
        let warned = |a: &[Action], id: &str, what: &str| a.iter().filter_map(|a| match a { Action::Event(r) if r.sev == Sev::Warn && r.line.contains(what) && r.line.contains(id) => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        let built = format!("test makepad-foo{}", super::super::JIT_BUILD_RUN);
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 {
            s.record_node_cost("test makepad-foo", "test", "a", 600_000);
            s.record_node_cost("jit-build", "", "a", 32_000);
        }
        // (overlays: a box's shared work tree takes one tree lease at a time)
        let ask = |s: &mut Sched, lane: &str| {
            let mut r = req(lane, Kind::Test, Some("a"));
            (r.src, r.rev, r.class) = (Src::Overlay, Some(format!("work@{}", "a".repeat(40))), "test makepad-foo".into());
            s.request(r, 0)
        };
        let (long, done) = (ask(&mut s, "long"), ask(&mut s, "done"));
        assert_eq!(grants(&s.tick(0)).len(), 2);
        let info = |end: u64, idle: u64| format!(r#"{{"class":{},"idle_s":{idle},"phases":[{{"phase":"jit","blocker":"","boxes":[],"start_ms":1000,"end_ms":{end}}}]}}"#, json_str(&built));
        for id in [&long, &done] {
            s.started(id, "{}");
            s.set_info(id, &info(0, 0), 1_000);
        }
        // far past its box's measure and working: nothing said
        beat_all(&mut s, 541_000);
        assert_eq!(warned(&s.tick(541_000), &long, "stalled?"), Vec::<String>::new());
        // built at 8m21s: its command held to its warm 10 min from there (from its grant, warm and
        // build together were past at 28 min)
        s.set_info(&done, &info(501_000, 0), 541_500);
        beat_all(&mut s, 1_690_000);
        let a = s.tick(1_690_000);
        assert_eq!(warned(&a, &done, "stalled?"), Vec::<String>::new(), "19m49s of its command");
        assert_eq!(warned(&a, &long, "stalled?"), Vec::<String>::new(), "28 min of a build that works");
        beat_all(&mut s, 1_702_000);
        let said = warned(&s.tick(1_702_000), &done, "stalled?");
        assert!(said.len() == 1 && said[0].contains("running 20m01s against its warm run's 10m00s"), "{said:?}");
        // every process of the build idle says so, whatever its length, and captures why
        s.set_info(&long, &info(0, 600), 1_703_000);
        beat_all(&mut s, 1_703_000);
        let a = s.tick(1_703_000);
        let said = warned(&a, &long, "idle");
        assert!(said.len() == 1 && said[0].contains("10m00s"), "{said:?}");
        assert!(a.iter().any(|a| matches!(a, Action::Capture { id, .. } if *id == long)), "captured: {a:?}");
    }

    /// A work tree's lease builds its JIT on all of cargo's jobs, not on the jobs of its charge
    /// (one core's `-j 1` built for 18 min, L693116, 11-08): its `jit` phase open, it is charged its
    /// box's cores from the beat that says so, so no second lease goes onto them, and built, its
    /// own charge again. An overlay's dev JIT is the box's store build, not the lease's: no charge.
    #[test]
    fn a_lease_building_its_jit_is_charged_its_boxs_cores() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        // (overlays go beside a box's shared work tree, which takes one tree lease at a time)
        let overlay = |lane: &str| {
            let mut r = req(lane, Kind::Test, Some("a"));
            (r.src, r.rev) = (Src::Overlay, Some(format!("work@{}", "a".repeat(40))));
            r
        };
        let small = s.request(req("small", Kind::Test, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![small.clone()]);
        s.started(&small, "{}");
        assert_eq!(s.charged_on("a").cores, 4.0);
        let info = |end: u64| format!(r#"{{"class":"test makepad-foo{}","phases":[{{"phase":"jit","blocker":"","boxes":[],"start_ms":1000,"end_ms":{end}}}]}}"#, super::super::JIT_BUILD_RUN);
        s.set_info(&small, &info(0), 1_000);
        assert_eq!(s.charged_on("a").cores, 16.0, "from the beat, before any caps poll measures the build");
        let second = s.request(overlay("second"), 2_000);
        assert!(grants(&s.tick(2_000)).is_empty());
        assert_eq!(state(&s, &second), ("queued".to_string(), "budget cores 16, in use 16, asked 4".to_string()));
        // built
        s.set_info(&small, &info(60_000), 60_000);
        assert_eq!(s.charged_on("a").cores, 4.0);
        beat_all(&mut s, 60_000);
        assert_eq!(grants(&s.tick(60_000)), vec![second.clone()]);
        s.started(&second, "{}");
        s.set_info(&second, &info(0), 61_000);
        assert_eq!(s.charged_on("a").cores, 8.0, "an overlay's store build charges its lease nothing more");
        // its RAM too, once a build measured it: before it holds it, and not past the build
        let held = s.charged_on("a").ram_gb;
        s.record_resources("jit-build", "", Some(10.0), None);
        assert_eq!(s.charged_on("a").ram_gb, held, "no build open");
        s.set_info(&small, &info(0), 62_000);
        assert_eq!(s.charged_on("a").ram_gb, held - 6.0 + 12.0, "its 6 GB ask becomes the build's 10 GB and a fifth more");
        // its OS family's own builds go first
        s.record_family_peak("linux", "jit-build", "", Some(7.5));
        assert_eq!(s.charged_on("a").ram_gb, held - 6.0 + 9.0);
        s.set_info(&small, &info(63_000), 63_000);
        assert_eq!(s.charged_on("a").ram_gb, held);
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

    /// A CI leg nobody drives ends as a failure when its processes stay idle, once its stall's
    /// capture is in, naming what it found. A lane's own lease stays for whoever reads the stall,
    /// and a leg that works again is not ended.
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
        assert_eq!(state(&s, &leg).0, "granted", "it ends once its capture is in");
        assert!(s.owner_word(&lane).is_some_and(|w| w.contains("every process idle") && w.contains("10m00s")), "its owner hears it");
        s.set_info(&woke, r#"{"idle_s":0}"#, 0);
        assert_eq!(s.owner_word(&woke), None, "it works again");
        for id in [&leg, &woke, &lane] { s.captured(id, "it waits in `sh > sleep 3600` (pid 7, for 9m50s)"); }
        beat_all(&mut s, 660_000);
        s.tick(660_000);
        assert_eq!(state(&s, &leg), ("ended hung".to_string(), "every process idle (under 5% of a core, no I/O) for 10m00s; it waits in `sh > sleep 3600` (pid 7, for 9m50s)".to_string()));
        assert_eq!(state(&s, &woke).0, "granted", "it works again");
        assert_eq!(state(&s, &lane).0, "granted", "a lane's lease has someone to read the stall");
    }

    /// The master's machine asleep (the Mac's lid, 11-08) costs no lease. Woken, it expires no
    /// holder that beat just before the sleep (a live session's), ends no bucket's gate as hung on
    /// output it could not hear, and ends no box's leases as lost on caps it could not ask. A
    /// holder already silent before the sleep expires by its silence awake.
    #[test]
    fn the_masters_sleep_costs_no_lease() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool)), ("box-b", caps(16, 64.0, Role::Pool)), ("box-c", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_node_cost("test", "test", "box-a", 382_000); }
        let mut live = req("live-W1", Kind::Test, Some("box-b"));
        live.live = true;
        let live = s.request(live, 0);
        let mut gate = req("landq-fast", Kind::Test, Some("box-a"));
        gate.agent = Some(BUCKET_AGENT.into());
        let gate = s.request(gate, 0);
        let silent = s.request(req("lane", Kind::Test, Some("box-c")), 0);
        assert_eq!(grants(&s.tick(0)).len(), 3);
        for id in [&live, &gate, &silent] {
            s.started(id, "{}");
            s.set_info(id, r#"{"idle_s":0,"out_bytes":100}"#, 1_000);
        }
        // the gate is 5 min into its measured 6m22s and writing; the lane's holder last beat 6 s
        // before the lid closed, and box-b stopped answering its caps a second before it
        s.set_info(&gate, r#"{"idle_s":0,"out_bytes":200}"#, 240_000);
        for id in [&live, &gate] { s.beat(id, 300_000); }
        s.beat(&silent, 294_000);
        assert!(s.caps_missed("box-b", "Host is down", 299_000).is_empty());
        s.tick(300_000);
        // asleep 15m29s, the first dark wake's look says so
        let woke = 300_000 + 929_000;
        s.master_slept(929_000, woke);
        let a = s.tick(woke);
        assert!(a.iter().any(|x| matches!(x, Action::Event(r) if r.line == "the master's machine slept 15m29s: no lease's beats, output or idle time count it (3 lease(s) kept)")), "{a:?}");
        assert!(a.iter().all(|x| !matches!(x, Action::End { .. })), "{a:?}");
        assert!(s.caps_missed("box-b", "Host is down", woke + 1_000).is_empty(), "box-b's leases are not lost for the sleep");
        // its overrun is real wall: said, captured; its output stopped 1 min before the sleep
        for id in [&live, &gate] { s.beat(id, woke + 2_000); }
        s.captured(&gate, "its processes were not read: link lost");
        s.tick(woke + 2_000);
        assert_eq!(state(&s, &gate).0, "granted", "no gate hangs on output the master could not hear");
        assert_eq!(state(&s, &live).0, "granted", "the live session is kept");
        assert_eq!(state(&s, &silent).0, "granted", "6 s of silence before the sleep, 2 s since");
        for id in [&live, &gate] { s.beat(id, woke + 4_001); }
        s.tick(woke + 4_001);
        assert_eq!(state(&s, &silent).0, "ended expired", "silent 10 s awake");
        assert_eq!(state(&s, &live).0, "granted");
    }

    /// The sleep moves nothing past the look that saw it: a holder that beat once at the wake,
    /// before that look (a `beat` on the control socket, a hub's caps on its own thread), and then
    /// died expires 10 s after it, not a sleep's length later. A box near idle since before the
    /// sleep counts only its awake time near idle.
    #[test]
    fn the_masters_sleep_moves_nothing_past_its_look() {
        let idle_lines = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Event(r) if r.line.contains("near idle") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        const MIN: u64 = 60_000;
        let mut s = sched(&[("node-c", caps(16, 64.0, Role::Bench))]);
        s.nodes.get_mut("node-c").unwrap().caps.as_mut().unwrap().load1 = 0.3;
        let holder = s.request(req("quiet", Kind::Timed, Some("node-c")), 0);
        assert_eq!(grants(&s.tick(0)), vec![holder.clone()]);
        s.started(&holder, "{}");
        let head = s.request(req("behind", Kind::Timed, Some("node-c")), 0);
        beat_all(&mut s, 5 * MIN);
        assert!(idle_lines(&s.tick(5 * MIN)).is_empty());
        // asleep 15 min; the holder beats once at the wake, before the look, and dies
        let woke = 20 * MIN;
        s.beat(&holder, woke - 500);
        s.master_slept(15 * MIN, woke);
        let a = s.tick(woke);
        assert!(idle_lines(&a).is_empty(), "near idle 5 min awake, not 20: {a:?}");
        s.beat(&head, woke + 9_000);
        s.tick(woke + 9_000);
        assert_eq!(state(&s, &holder).0, "granted");
        s.beat(&head, woke + 10_001);
        s.tick(woke + 10_001);
        assert_eq!(state(&s, &holder).0, "ended expired", "silent 10 s after its beat at the wake");
    }

    /// A live session's call past 3x its class's measure (and 5 min over it) is said once: an
    /// inbox warning naming its lane and agent, its why-hung capture, its owner told; nothing ends
    /// it. Its next call, after an idle spell, is timed from its own start (W4652, 11-10).
    #[test]
    fn a_live_call_far_past_its_measure_is_said_and_captured_never_ended() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_node_cost("test", "test", "box-a", 600_000); }
        let mut r = req("live-w1", Kind::Test, Some("box-a"));
        (r.live, r.agent) = (true, Some("AGENTX".into()));
        let id = s.request(r, 0);
        assert_eq!(grants(&s.tick(0)), vec![id.clone()]);
        s.started(&id, "{}");
        let at = |s: &mut Sched, t: u64| {
            s.set_caps("box-a", caps(16, 64.0, Role::Pool), t);
            beat_all(s, t);
            s.tick(t)
        };
        let said = |a: &[Action]| a.iter().filter(|x| matches!(x, Action::Event(r) if r.sev == Sev::Warn && r.line.contains("its live call has run"))).count();
        let captured = |a: &[Action]| a.iter().filter(|x| matches!(x, Action::Capture { id: c, .. } if *c == id)).count();
        let a = at(&mut s, 30 * MIN);
        assert_eq!((said(&a), captured(&a)), (0, 0), "at 3x its 10 min: not yet");
        let a = at(&mut s, 30 * MIN + 1_000);
        assert_eq!((said(&a), captured(&a)), (1, 1), "past 3x: said and captured: {a:?}");
        assert!(a.iter().any(|x| matches!(x, Action::Event(r) if r.line.contains("live-w1 (AGENTX) on box-a") && r.line.contains("against its class's measured 10m00s"))), "it names its lane, agent and measure: {a:?}");
        assert!(s.owner_word(&id).is_some_and(|w| w.contains("why-hung")), "its owner hears it");
        let a = at(&mut s, 120 * MIN);
        assert_eq!((said(&a), captured(&a)), (0, 0), "once per call");
        assert_eq!(state(&s, &id).0, "granted", "nothing ends it");
        // its call ends; the next one is timed from its own start
        s.live_idle.insert(id.clone());
        s.live_call_ended(&id);
        assert_eq!(s.owner_word(&id), None, "that call is over");
        s.live_idle.remove(&id);
        s.live_call_began(&id, 121 * MIN);
        assert_eq!(said(&at(&mut s, 151 * MIN)), 0, "30 min into the new call");
        assert_eq!(said(&at(&mut s, 152 * MIN)), 1, "31 min into it");
        // a session between calls is held to nothing
        s.live_idle.insert(id.clone());
        s.live_call_ended(&id);
        assert_eq!(said(&at(&mut s, 400 * MIN)), 0, "idle");
    }

    /// A leg devhub runs (here a jitlegs leg and a bucket's gate) past twice its measure, 5 min
    /// over it, ends as a failure once its capture is in, even while its box never reads idle (a
    /// poll loop beside a hung fixture stub, L634083, 11-08), naming where it waited and its
    /// capture; not while its output grows. A leg held to a guess (no measure) ends only on its
    /// idleness, and a lane's lease past its measure is only said.
    #[test]
    fn a_leg_devhub_runs_past_its_measure_ends_hung_once_captured() {
        let nodes: Vec<(String, Caps)> = (0..5).map(|i| (format!("n{i}"), caps(16, 64.0, Role::Pool))).collect();
        let nodes: Vec<(&str, Caps)> = nodes.iter().map(|(n, c)| (n.as_str(), c.clone())).collect();
        let mut s = sched(&nodes);
        for _ in 0..3 { s.record_node_cost("test", "test", "n0", 600_000); }
        let leg = |s: &mut Sched, lane: &str, agent: Option<&str>, node: &str| {
            let mut r = req(lane, Kind::Test, Some(node));
            r.agent = agent.map(str::to_string);
            s.request(r, 0)
        };
        let jitlegs = leg(&mut s, "jitlegs-full-a", Some("devhub-jitlegs"), "n0");
        let bucket = leg(&mut s, "bucket-full", Some(BUCKET_AGENT), "n1");
        let writing = leg(&mut s, "jitlegs-full-b", Some("devhub-jitlegs"), "n2");
        let lane = leg(&mut s, "lane", Some("AGENTX"), "n3");
        let mut guess = req("jitlegs-new", Kind::Test, Some("n4"));
        (guess.agent, guess.class) = (Some("devhub-jitlegs".into()), "leg:new".into());
        let guess = s.request(guess, 0);
        assert_eq!(grants(&s.tick(0)).len(), 5);
        let all = [&jitlegs, &bucket, &writing, &lane, &guess];
        for id in all {
            s.started(id, "{}");
            s.set_info(id, r#"{"idle_s":30,"out_bytes":100}"#, 60_000);
        }
        // past twice its 10 min and 5 min over it: said, captured
        beat_all(&mut s, 1_201_000);
        let a = s.tick(1_201_000);
        for id in [&jitlegs, &bucket, &writing, &lane] {
            assert!(a.iter().any(|x| matches!(x, Action::Capture { id: c, why, .. } if c == id && why.contains("against its measured 10m00s"))), "{id}: {a:?}");
        }
        assert_eq!(state(&s, &jitlegs).0, "granted", "it ends once its capture is in");
        s.set_info(&writing, r#"{"idle_s":0,"out_bytes":200}"#, 1_202_000);
        for id in all { s.captured(id, "it waits in `bash land_gate.sh full-a > bash test_run.sh > makepad test test-run-plan` (pid 9, for 22m00s); its last step line `STEP fuzz 47 s [10:34:51-10:35:38]`; its capture: devhub job result L1 (/x/L1.log)"); }
        beat_all(&mut s, 1_203_000);
        s.tick(1_203_000);
        let (how, why) = state(&s, &jitlegs);
        assert_eq!(how, "ended hung");
        assert_eq!(why, "running 20m03s against its measured 10m00s, its output grown none for 19m03s; it waits in `bash land_gate.sh full-a > bash test_run.sh > makepad test test-run-plan` (pid 9, for 22m00s); its last step line `STEP fuzz 47 s [10:34:51-10:35:38]`; its capture: devhub job result L1 (/x/L1.log)");
        assert_eq!(state(&s, &bucket).0, "ended hung", "a bucket's gate too");
        assert_eq!(state(&s, &writing).0, "granted", "its output grows");
        assert_eq!(state(&s, &lane).0, "granted", "a lane's lease has someone to read the stall");
        assert_eq!(state(&s, &guess).0, "granted", "held to a guess, it is only said");
        // its output stops: it ends; the guess ends once idle
        beat_all(&mut s, 1_502_000);
        s.tick(1_502_000);
        assert_eq!(state(&s, &writing).0, "ended hung");
        assert_eq!(state(&s, &guess).0, "granted");
        s.set_info(&guess, r#"{"idle_s":600}"#, 1_503_000);
        beat_all(&mut s, 1_503_000);
        s.tick(1_503_000);
        assert!(state(&s, &guess).1.starts_with("every process idle"), "{:?}", state(&s, &guess));
        assert_eq!(state(&s, &lane).0, "granted");
    }

    /// A lane's test run (`devhub test`) is driven by nobody either: past twice its measure, its
    /// output grown none and its processes under half a core for 10 min, it ends hung once its
    /// capture is in, while they never read idle (an app ticking its sim, L798418, 11-10). One
    /// whose processes work, and a lane's lease that is no test run, are only said.
    #[test]
    fn a_lanes_test_run_that_hangs_slow_ends_once_captured() {
        let mut s = sched(&[("n0", caps(16, 64.0, Role::Pool)), ("n1", caps(16, 64.0, Role::Pool)), ("n2", caps(16, 64.0, Role::Pool))]);
        let (test, script) = ("test:makepad-fab:mechanics", "makepad check");
        for class in [test, script] { for _ in 0..3 { s.record_node_cost(class, "test", "n0", 77_000); } }
        let run = |s: &mut Sched, node: &str, class: &str| {
            let mut r = req("mechport", Kind::Test, Some(node));
            (r.agent, r.class) = (Some("AGENTX".into()), class.into());
            s.request(r, 0)
        };
        let (hung, working, other) = (run(&mut s, "n0", test), run(&mut s, "n1", test), run(&mut s, "n2", script));
        assert_eq!(grants(&s.tick(0)).len(), 3);
        let all = [&hung, &working, &other];
        for id in all {
            s.started(id, "{}");
            s.set_info(id, r#"{"idle_s":0,"slow_s":0,"out_bytes":100}"#, 10_000);
        }
        // past twice its 1m17s and 5 min over it: said, captured
        beat_all(&mut s, 378_000);
        let a = s.tick(378_000);
        for id in all {
            assert!(a.iter().any(|x| matches!(x, Action::Capture { id: c, why, .. } if c == id && why.contains("against its measured 1m17s"))), "{id}: {a:?}");
            s.captured(id, "it waits in `makepad test makepad-fab --test mechanics` (pid 9, for 6m00s)");
        }
        s.set_info(&hung, r#"{"idle_s":0,"slow_s":360,"out_bytes":100}"#, 379_000);
        s.set_info(&other, r#"{"idle_s":0,"slow_s":600,"out_bytes":100}"#, 379_000);
        beat_all(&mut s, 380_000);
        s.tick(380_000);
        assert_eq!(state(&s, &hung).0, "granted", "6 min slow is no hang yet");
        s.set_info(&hung, r#"{"idle_s":0,"slow_s":600,"out_bytes":100}"#, 620_000);
        beat_all(&mut s, 620_000);
        s.tick(620_000);
        assert_eq!(state(&s, &hung), ("ended hung".to_string(), "running 10m20s against its measured 1m17s, its output grown none for 10m10s, its processes under 50% of a core for 10m00s; it waits in `makepad test makepad-fab --test mechanics` (pid 9, for 6m00s)".to_string()));
        assert_eq!(state(&s, &working).0, "granted", "its processes work");
        assert_eq!(state(&s, &other).0, "granted", "a lane's lease that is no test run has someone to read the stall");
        // idle for 10 min, a test run ends whatever its measure
        s.set_info(&working, r#"{"idle_s":600,"slow_s":600,"out_bytes":100}"#, 621_000);
        beat_all(&mut s, 621_000);
        s.tick(621_000);
        assert!(state(&s, &working).1.starts_with("every process idle"), "{:?}", state(&s, &working));
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

    /// CI's JIT pre-build runs `makepad --version` through cargo: a fraction of a second where the
    /// box keeps the JIT's binary, minutes where cargo builds it. Its box says which, and a build
    /// is held to builds: 6 min of one on a box was "stalled" against the warm runs' 0 s (11-07).
    #[test]
    fn a_run_whose_jit_cargo_builds_is_measured_as_that_build() {
        let warm = "ci makepad --version";
        let built = format!("{warm}{}", super::super::JIT_BUILD_RUN);
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        for _ in 0..12 { s.record_node_cost(warm, "ci", "b", 200); }
        s.record_node_cost(&built, "ci", "a", 236_000);
        let stalled = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Event(r) if r.sev == Sev::Warn && r.line.contains("stalled?") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        let first = s.request(req("ci", Kind::Ci, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![first.clone()]);
        s.started(&first, "{}");
        s.set_info(&first, &format!(r#"{{"running":"cargo","class":{}}}"#, json_str(&built)), 0);
        assert_eq!(s.get(&first).unwrap().req.class, built);
        beat_all(&mut s, 362_000);
        assert_eq!(stalled(&s.tick(362_000)), Vec::<String>::new(), "6 min of a build that took 3m56s");
        beat_all(&mut s, 537_000);
        let said = stalled(&s.tick(537_000));
        assert!(said.len() == 1 && said[0].contains(&format!("against its measured 3m56s ({built})")), "past twice its builds' measure it is: {said:?}");
        // a lease that asked its command's class: its box's word that cargo builds names the same job
        let mut asked = req("lane", Kind::Ci, Some("b"));
        asked.class = warm.into();
        let asked = s.request(asked, 538_000);
        assert_eq!(grants(&s.tick(538_000)), vec![asked.clone()]);
        s.started(&asked, "{}");
        s.set_info(&asked, &format!(r#"{{"class":{}}}"#, json_str(&built)), 538_000);
        assert_eq!(s.get(&asked).unwrap().req.class, built);
        beat_all(&mut s, 900_000);
        assert_eq!(stalled(&s.tick(900_000)), Vec::<String>::new(), "its 6 min held to the build too");
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
        assert_eq!(state(&s, &old).1, format!("held for interactive {hi} (it starts as the box's work ends by its measure: only work that ends first or fits beside it)"));
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
        assert_eq!(s.node_view(0)[0].hold.as_deref(), Some(late.as_str()));
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
            let mut r = devhubs(req(name, Kind::Test, Some("a")));
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

    /// A chain's next case asks before its previous one is released and keeps the chain's first
    /// ask against other work, but not against a lane that asked for timed work while its
    /// previous case ran: that lane's turn comes first ([`Sched::exclusive_place`]).
    #[test]
    fn a_timed_chain_queues_before_release_and_keeps_its_original_place() {
        for older in [false, true] {
            let mut s = sched(&[("box-a", caps(16, 64.0, Role::Bench))]);
            let mut r = req("chain", Kind::Timed, Some("box-a"));
            r.master = true;
            r.since_ms = Some(2);
            let first = s.request(r.clone(), 2);
            assert_eq!(grants(&s.tick(2)), vec![first.clone()]);
            let mut other = req("other", Kind::Timed, Some("box-a"));
            other.since_ms = Some(if older { 1 } else { 3 });
            let other = s.request(other, 3);
            s.tick(3);
            let shared = s.request(devhubs(req("shared", Kind::Test, Some("box-a"))), 4);
            let next = s.request(r, 5);
            assert!(grants(&s.tick(5)).is_empty());
            assert_eq!(state(&s, &next).0, "queued", "a master chain may ask before release");
            release_clean(&mut s, &first, 6);
            assert_eq!(grants(&s.tick(6)), vec![other.clone()], "its turn: it asked while the chain's case ran");
            release_clean(&mut s, &other, 7);
            assert_eq!(grants(&s.tick(7)), vec![next.clone()], "the chain's place is ahead of the shared ask");
            release_clean(&mut s, &next, 8);
            assert_eq!(grants(&s.tick(8)), vec![shared]);
            invariants(&s);
        }
    }

    /// Two lanes' timed chains on one bench box take turns, case by case: the older chain's next
    /// case kept its first ask and ran three cases back to back while the other lane's waited
    /// 14-20 min (11-07).
    #[test]
    fn two_lanes_timed_chains_alternate_on_a_box() {
        let mut s = sched(&[("bench-a", caps(16, 64.0, Role::Bench))]);
        let case = |s: &mut Sched, lane: &str, since: u64, now: u64| {
            let mut r = req(lane, Kind::Timed, Some("bench-a"));
            r.master = true;
            r.since_ms = Some(since);
            s.request(r, now)
        };
        let mut running = case(&mut s, "fab", 1, 1);
        assert_eq!(grants(&s.tick(1)), vec![running.clone()]);
        let mut waiting = case(&mut s, "jit", 2, 2);
        s.tick(2);
        let mut ran = vec![("fab", 1)];
        let mut cases = HashMap::from([("fab", 1), ("jit", 1)]);
        let lane_of = |s: &Sched, id: &str| s.get(id).map(|l| l.req.lane.clone()).unwrap();
        for now in (3..30).step_by(3) {
            // the running case asks its chain's next case before its release
            let lane = lane_of(&s, &running);
            let since = if lane == "fab" { 1 } else { 2 };
            let next = case(&mut s, &lane, since, now);
            s.tick(now);
            assert!(state(&s, &next).1.contains("the chain's previous case"), "{:?}", state(&s, &next));
            release_clean(&mut s, &running, now + 1);
            let granted = grants(&s.tick(now + 1));
            assert_eq!(granted, vec![waiting.clone()], "the other lane's case first, at {now}: {:?}", state(&s, &waiting));
            let l = lane_of(&s, &waiting);
            *cases.get_mut(l.as_str()).unwrap() += 1;
            ran.push((if l == "fab" { "fab" } else { "jit" }, cases[l.as_str()]));
            (running, waiting) = (waiting, next);
        }
        let lanes: Vec<&str> = ran.iter().map(|(l, _)| *l).collect();
        assert!(lanes.windows(2).all(|w| w[0] != w[1]), "turn by turn: {ran:?}");
        invariants(&s);
    }

    /// The head of a box's queue holds the box while it waits, whatever its priority, past
    /// [`HOLD_MS`]: smaller, newer asks that would outlive its wait take none of the cores the
    /// box frees (only work that ends first or fits beside it), and it starts as the box's
    /// running work ends by its measure. A background ask's hold keeps no interactive work off.
    #[test]
    fn the_head_of_a_boxs_queue_holds_it_until_the_running_work_ends() {
        const MIN: u64 = 60_000;
        let ask = |lane: &str, cores: f64, est_min: f64, priority: Priority| {
            let mut r = overlay(lane, None, cores);
            r.est_min = Some(est_min);
            r.priority = priority;
            r
        };
        for priority in [Priority::Background, Priority::Interactive] {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
            let first = s.request(ask("first", 8.0, 15.0, Priority::Background), 0);
            let second = s.request(ask("second", 8.0, 30.0, Priority::Background), 0);
            assert_eq!(grants(&s.tick(0)).len(), 2);
            let head = s.request(ask("head", 16.0, 10.0, priority), 1);
            let long = s.request(ask("long", 4.0, 60.0, Priority::Background), 2);
            beat_all(&mut s, 2);
            assert!(grants(&s.tick(2)).is_empty());
            assert_eq!(s.holds["a"].id, head, "{priority:?}: the head holds the box it waits for");
            // past HOLD_MS the first ends: the long ask would outlive the head's wait and stays off
            release_clean(&mut s, &first, 15 * MIN);
            beat_all(&mut s, 15 * MIN);
            assert!(grants(&s.tick(15 * MIN)).is_empty(), "{priority:?}: {:?}", state(&s, &long));
            assert_eq!(s.holds["a"].id, head, "{priority:?}: a hold lasts while its ask waits");
            assert!(state(&s, &long).1.contains(&format!("{head} (it starts as the box's work ends by its measure: only work that ends first or fits beside it)")), "{:?}", state(&s, &long));
            invariants(&s);
            // work that ends first by its measure gets in (the head has queued past STARVED_MS: a
            // declared run is no measure), and interactive work beside a background ask's hold
            for _ in 0..3 { s.record_node_cost("test quick", "test", "a", 5 * MIN); }
            let mut r = ask("quick", 4.0, 5.0, Priority::Background);
            r.class = "test quick".into();
            let quick = s.request(r, 15 * MIN + 1);
            beat_all(&mut s, 15 * MIN + 1);
            assert_eq!(grants(&s.tick(15 * MIN + 1)), vec![quick.clone()], "{priority:?}: {:?}", state(&s, &quick));
            let call = s.request(ask("call", 4.0, 60.0, Priority::Interactive), 15 * MIN + 2);
            beat_all(&mut s, 15 * MIN + 2);
            let got = grants(&s.tick(15 * MIN + 2));
            assert_eq!(got.contains(&call), priority == Priority::Background, "{priority:?}: {:?}", state(&s, &call));
            if got.contains(&call) { release_clean(&mut s, &call, 15 * MIN + 3); }
            release_clean(&mut s, &quick, 20 * MIN);
            release_clean(&mut s, &second, 30 * MIN);
            beat_all(&mut s, 30 * MIN);
            let started = grants(&s.tick(30 * MIN));
            assert!(started.contains(&head) && !started.contains(&long), "{priority:?}: the head starts as the box's work ends: {started:?} {:?}", state(&s, &head));
        }
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
        // longer than the timed run's guess: exclusive work goes by expected end
        shared.est_min = Some(30.0);
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

    /// A gate is idle only when nothing of it ran and its output stayed for the idle limit: one
    /// whose steps run 15 min in all, a few minutes each with gaps between, keeps its gate slot.
    #[test]
    fn a_gate_whose_steps_run_past_the_idle_limit_keeps_its_lease() {
        let mut s = sched(&[("n0", caps(16, 64.0, Role::Pool))]);
        let gate = s.request(req(LANDING_LANE, Kind::Gate, None), 0);
        s.tick(0);
        s.started(&gate, "{}");
        let min = 60_000;
        // a step each 3 min (a run, then its lines), none running at the look
        for k in 1..=5u64 {
            let t = k * 3 * min;
            s.beat(&gate, t);
            s.set_info(&gate, &format!(r#"{{"running":"","out_bytes":{}}}"#, k * 1000), t);
            s.tick(t);
            assert_eq!(state(&s, &gate).0, "granted", "step {k}: its output grew {t} ms in");
        }
        // then nothing runs and nothing is written: idle from its last line
        let last = 15 * min;
        s.beat(&gate, last + GATE_IDLE_MS - 1);
        s.tick(last + GATE_IDLE_MS - 1);
        assert_eq!(state(&s, &gate).0, "granted");
        s.beat(&gate, last + GATE_IDLE_MS + 1);
        s.tick(last + GATE_IDLE_MS + 1);
        assert_eq!(state(&s, &gate).0, "ended idle", "idle for the limit since its last line");
    }

    /// A master's restart (each self-update) loses what it knew of a lease's activity: its idle
    /// and output clocks start at the restore. A gate 12 min into its steps keeps its lease and is
    /// not said stalled, and a screen run 20 min in is not ended as forgotten, in the first tick
    /// after the restore; each ends as it would have from the restore on (L693168, 11-09).
    #[test]
    fn a_master_restart_starts_each_leases_idle_clocks_at_the_restore() {
        let mut s = sched(&[("n0", caps(16, 64.0, Role::Pool)), ("n1", caps(16, 64.0, Role::Pool))]);
        let gate = s.request(req(LANDING_LANE, Kind::Gate, Some("n0")), 0);
        let mut look = req("lane-s", Kind::Screen, Some("n1"));
        look.class = "screen makepad run makepad-app-calculator".into();
        let screen = s.request(look, 0);
        s.tick(0);
        assert!(s.started(&gate, "{}") && s.started(&screen, "{}"), "both granted");
        assert!(app_run(&s.get(&screen).unwrap().req.class).is_some(), "a screen run of an app");
        let min = 60_000;
        let at = 12 * min;
        s.set_info(&gate, r#"{"running":"bash land_gate.sh fast","out_bytes":4000}"#, at - 1000);
        let lines = s.live_lines();
        let mut s2 = sched(&[("n0", caps(16, 64.0, Role::Pool)), ("n1", caps(16, 64.0, Role::Pool))]);
        for l in &lines { s2.restore(l, at); }
        let events = |a: &[Action]| a.iter().filter_map(|a| if let Action::Event(r) = a { Some(r.line.clone()) } else { None }).collect::<Vec<_>>();
        let first = s2.tick(at + 1000);
        assert_eq!(state(&s2, &gate).0, "granted", "the gate's steps go on");
        assert!(!events(&first).iter().any(|l| l.contains("stalled?")), "{:?}", events(&first));
        let later = 20 * min;
        let mut s3 = sched(&[("n0", caps(16, 64.0, Role::Pool)), ("n1", caps(16, 64.0, Role::Pool))]);
        for l in &lines { s3.restore(l, later); }
        s3.tick(later + 1000);
        assert_eq!(state(&s3, &screen).0, "granted", "a screen run is judged from the restore");
        assert_eq!(state(&s3, &gate).0, "granted");
        // from the restore on, as before: no word of a run and no output for the idle limit
        beat_all(&mut s2, at + GATE_IDLE_MS + 1);
        s2.tick(at + GATE_IDLE_MS + 1);
        assert_eq!(state(&s2, &gate).0, "ended idle", "idle for the limit since the restore");
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
        let mut node = s.node_view(0).into_iter().find(|n| n.name == "a").unwrap();
        let line = node.caps_line();
        assert!(line.contains("hub old 20.0 GB (heap 5.0)"), "{line}");
        node.caps.as_mut().unwrap().features.clear();
        assert_eq!((node.caps_line().contains("(no features: an older hub)"), line.contains("(no features: an older hub)")), (true, false), "only a hub with no features gets the older-hub note");
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
        assert!(s.node_view(0)[0].draining && s.node_view(0)[0].state_word() == "draining (1 leases)");
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

    /// A lane's screen run of an app ends once nobody looked at or drove it for
    /// [`SCREEN_IDLE_MS`] past what it declared: a route to its app (by package or pid) or an
    /// inspect lease naming it keeps it, so does a person at its box; a passive feed does not,
    /// and a script's screen run is never idle-ended (L615704, 11-07).
    #[test]
    fn a_forgotten_screen_run_of_an_app_ends_idle() {
        let user = |present: bool| Some(crate::lease::UserSense { present, idle_ms: Some(if present { 5_000 } else { 200_000 }), what: "key from its device".into(), idle_min: 3 });
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        let screen = |s: &mut Sched, lane: &str, node: &str, class: &str, est_min: Option<f64>| {
            let mut r = req(lane, Kind::Screen, Some(node));
            (r.class, r.est_min) = (class.to_string(), est_min);
            let id = s.request(r, 0);
            s.tick(0);
            assert!(s.started(&id, "{}"), "{id} granted");
            id
        };
        let app = screen(&mut s, "lane-a", "a", "screen makepad run makepad-source-library --bin makepad-source-library", None);
        let rig = screen(&mut s, "lane-b", "b", "screen bash rig.sh @lane-b", None);
        let long = screen(&mut s, "lane-c", "c", "screen makepad run makepad-fab", Some(30.0));
        s.set_info(&app, r#"{"running":"makepad.jit-ab run","pid":4242}"#, 0);
        let at = |s: &mut Sched, ms: u64| {
            beat_all(s, ms);
            s.tick(ms);
        };
        // looked at by package, then by pid: it stays; a feed is no look
        s.app_touched("a", "makepad-source-library", "cmd", 10 * 60_000);
        s.app_touched("a", "4242", "frames", 20 * 60_000);
        s.app_touched("a", "4242", "watch", 30 * 60_000);
        at(&mut s, 20 * 60_000 + SCREEN_IDLE_MS);
        assert_eq!(state(&s, &app).0, "granted", "touched {} min ago", SCREEN_IDLE_MS / 60_000);
        at(&mut s, 20 * 60_000 + SCREEN_IDLE_MS + 1);
        assert_eq!(state(&s, &app).0, "ended idle", "a feed since its last look is no look");
        assert_eq!(state(&s, &rig).0, "granted", "a script's screen run keeps its own end");
        assert_eq!(state(&s, &long).0, "granted", "within its declared 30 min and the idle limit after");
        // a person at its box, or an inspect lease naming its app, keeps it
        s.set_caps("c", Caps { user: user(true), ..caps(16, 64.0, Role::Pool) }, 50 * 60_000);
        at(&mut s, 50 * 60_000);
        s.set_caps("c", Caps { user: user(false), ..caps(16, 64.0, Role::Pool) }, 50 * 60_000 + 1);
        let mut look = req("lane-c", Kind::Inspect, Some("c"));
        look.jit = Some("makepad-fab".into());
        let look = s.request(look, 60 * 60_000);
        at(&mut s, 60 * 60_000);
        assert_eq!(state(&s, &look).0, "starting", "{:?}", state(&s, &look));
        s.release(&look, "released", 60 * 60_000 + 1);
        at(&mut s, 60 * 60_000 + SCREEN_IDLE_MS);
        assert_eq!(state(&s, &long).0, "granted", "inspected at 60 min");
        at(&mut s, 60 * 60_000 + SCREEN_IDLE_MS + 1);
        assert_eq!(state(&s, &long).0, "ended idle");
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
        let v = &s.node_view(0)[0];
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
        assert_eq!(s.node_view(0)[0].state_word(), "user (key from its device 5 s ago)");
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
        assert!(!s.dev("a") && s.node_view(0)[0].state_word() == "dev off (user WM)");
        assert_ne!(state(&s, &w).0, "starting");
        // nor does a person's presence show over it
        s.set_caps("a", Caps { user: user(true), ..caps(64, 256.0, Role::UserWm) }, 2);
        assert_eq!(s.node_view(0)[0].state_word(), "dev off (user WM)");
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
    fn a_cancel_keeps_no_land_queue_work_off_its_box_and_why_queued_names_a_kept_off_box() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        let x = s.request(req("lane-x", Kind::Test, Some("a")), 0);
        let gate = s.request(req("landq-fast", Kind::Gate, Some("b")), 0);
        let hung = s.request(req(TESTS_LANE, Kind::Test, Some("c")), 0);
        s.tick(1);
        for id in [&x, &gate, &hung] {
            assert!(s.get(id).unwrap().state.live(), "{id}: {:?}", state(&s, id));
            assert!(s.cancel(id, 10));
        }
        s.tick(11);
        for (id, node) in [(&x, "a"), (&gate, "b"), (&hung, "c")] {
            s.cleaned(node, id, 0);
        }
        // the user's cancel of a hung shard and of a gate ends that run: the next landing's go anywhere
        assert_eq!(s.excluded.keys().cloned().collect::<Vec<_>>(), vec![("lane-x".to_string(), "a".to_string())]);
        let busy: Vec<String> = ["a", "b"].into_iter().map(|n| s.request(overlay(&format!("busy-{n}"), Some(n), 16.0), 20)).collect();
        s.tick(20);
        let mut r = req(TESTS_LANE, Kind::Test, None);
        r.cores = Some(16.0);
        let next = s.request(r, 21);
        s.tick(21);
        assert_eq!(node_of(&s, &next).as_deref(), Some("c"), "the next shard takes the box its sibling was cancelled on");
        // every box busy: lane-x waits, and why-queued says a keeps it off
        let again = s.request(req("lane-x", Kind::Test, None), 22);
        s.tick(22);
        assert_eq!(state(&s, &again).0, "queued");
        let why = s.why_queued(&again, 23).unwrap().join("\n");
        assert!(why.contains("\"node\":\"a\",\"reason\":\"the user cancelled a lane-x lease here: the lane stays off it 30 min more (--node a overrides)\""), "{why}");
        assert!(why.contains(&format!("\"blockers\":[\"{}\"]", busy[1])) && why.contains(&format!("\"blockers\":[\"{next}\"]")), "{why}");
    }
    #[test]
    fn requests_round_trip() {
        let mut r = req("gapx", Kind::Build, Some("box-a"));
        r.needs = parse_needs("arch=x64,ram=32G");
        r.prio = 0;
        r.priority = Priority::Interactive;
        r.loader = Some("loader-id".into());
        r.rev = Some("work,commercial=main".into());
        r.exec_profile = super::super::argv::ExecProfile::of(&["makepad".into()]);
        r.warms = Some("warm-lane".into());
        r.series = Some("series-a".into());
        r.plain_test = Some(false);
        r.live = true;
        r.cost_asked = true;
        r.est_min = Some(2.0);
        r.cores = Some(2.0);
        r.ram_gb = Some(4.0);
        r.ram_floor_gb = Some(1.0);
        r.whole = true;
        r.class = "build-example".into();
        r.repo_dir = Some(".".into());
        r.jit = Some("app-a".into());
        r.cpu_target = Some("generic".into());
        r.hint = vec!["loader-id".into()];
        r.holder = Some(42);
        r.master = true;
        r.gpu = true;
        r.agent = Some("agent-a".into());
        r.since_ms = Some(123);
        let json = r.to_json();
        assert_eq!(Req::parse(&json).unwrap(), r);
        let makepad_strict_json::Value::Obj(fields) = makepad_strict_json::parse(json.as_bytes()).unwrap() else { panic!("request is an object") };
        for (k, _) in &fields { assert!(REQ_KEYS.contains(&k.as_str()), "written lease field {k} is accepted"); }
        for k in REQ_KEYS { assert!(fields.iter().any(|(key, _)| key == k), "accepted lease field {k} is written"); }
        let extra = format!("{},\"keep\":\"x\"}}", json.strip_suffix('}').unwrap());
        assert!(Req::parse(&extra).unwrap_err().contains("lease field `keep`"));
        assert!(Req::parse("[]").is_err());
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

    /// A Mac's gpu lease (a timed one, [`Req::gpu`]) waits for its GPU to be quiet, not for the
    /// box's CPU: a Mac's own services at a core outside its leases held a gpu lease first in line
    /// on an empty Mac while its GPU idled (11-10). A timed ask whose every box stays not quiet at
    /// each look for 10 min, no lease of ours there, is told once what keeps it, by process, and
    /// what would let it start; a box quiet at one look starts that count again. Only a hub that
    /// judges the GPU takes one (`gpu-quiet`), and its node's start hears it is one.
    #[test]
    fn a_gpu_timing_waits_for_its_gpu_and_a_wait_that_will_not_end_is_said() {
        let min = 60_000;
        let mut mac = caps(16, 64.0, Role::Bench);
        (mac.arch, mac.os, mac.gpus) = ("aarch64".into(), "macos".into(), Vec::new());
        // the Mac's own services: 1.1 cores outside its leases, its helper using the GPU
        let services = vec![("mds_stores".to_string(), 401, 0.6), ("XprotectService".to_string(), 402, 0.3), ("helper".to_string(), 403, 0.2)];
        let mac_look = |gpu: f64| Caps { cpu_pct: Some(110.0 / 16.0), busy_outside: services.clone(), mac_gpu: Some((gpu, vec![("helper".into(), 403, gpu)])), ..mac.clone() };
        let desktop = |cpu: f64, applying: bool| Caps { cpu_pct: Some(cpu), busy_outside: vec![("WindowServer".into(), 407, 0.6)], applying, ..caps(16, 64.0, Role::Bench) };
        let mut s = sched(&[("box-a", mac_look(30.0)), ("box-b", desktop(9.0, false))]);
        let g = Req::parse(r#"{"lane":"fabxr","kind":"gpu","needs":{"os":"macos"},"agent":"GPUAGENT"}"#).unwrap();
        assert!(g.gpu && g.kind == Kind::Timed && Req::parse(&g.to_json()).unwrap().gpu, "a Mac's gpu lease is a GPU timing, and says so where it is kept");
        assert!(g.required(Role::Bench).contains(&"gpu-quiet") && !req("bench", Kind::Timed, None).required(Role::Bench).contains(&"gpu-quiet"));
        let g = s.request(g, 0);
        let t = s.request(req("bench", Kind::Timed, Some("box-b")), 0);
        let said = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Event(r) if r.line.contains("for a quiet box") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        let at = |s: &mut Sched, now: u64, gpu: f64, desk: Caps| {
            s.set_caps("box-a", mac_look(gpu), now);
            s.set_caps("box-b", desk, now);
            beat_all(s, now);
            s.tick(now)
        };
        assert!(said(&at(&mut s, 0, 30.0, desktop(9.0, false))).is_empty());
        assert!(state(&s, &g).1.contains("its GPU is 30% busy outside its leases: what uses it, helper pid 403 at 30% of the GPU"), "{:?}", state(&s, &g));
        assert!(state(&s, &t).1.contains("144% of a core runs outside its leases"), "{:?}", state(&s, &t));
        for m in 1..10 {
            // box-b quiet at one look, while its deployment applies: its count starts again
            let a = at(&mut s, m * min, 30.0, desktop(if m == 3 { 2.0 } else { 9.0 }, m == 3));
            assert!(grants(&a).is_empty() && said(&a).is_empty(), "minute {m}: {:?}", said(&a));
        }
        let a = at(&mut s, 10 * min, 30.0, desktop(9.0, false));
        assert_eq!(said(&a), vec![format!("{g} timed, its GPU's timing, of fabxr (GPUAGENT) has waited 10m00s for a quiet box, and what keeps it is no lease's work: box-a not quiet at any look for 10m00s (its GPU is 30% busy outside its leases: what uses it, helper pid 403 at 30% of the GPU). It starts once box-a has its GPU under 10% busy outside its leases: stop or pause those processes there, or ask it of another box")]);
        for m in 11..14 { assert!(said(&at(&mut s, m * min, 30.0, desktop(9.0, false))).is_empty(), "minute {m}: said once, and box-b's count started again at minute 4"); }
        let a = at(&mut s, 14 * min, 30.0, desktop(9.0, false));
        assert_eq!(said(&a).len(), 1, "{:?}", said(&a));
        assert!(said(&a)[0].starts_with(&format!("{t} timed of bench has waited 14m00s for a quiet box, and what keeps it is no lease's work: box-b not quiet at any look for 10m00s (144% of a core runs outside its leases: WindowServer pid 407 (0.6 cores)). It starts once box-b has under 50% of a core running outside its leases")), "{}", said(&a)[0]);
        // the GPU quiet, the Mac's services still at 1.1 cores: the GPU timing starts
        assert_eq!(grants(&at(&mut s, 15 * min, 2.0, desktop(9.0, false))), vec![g.clone()], "{:?}", state(&s, &g));
        assert_eq!(node_of(&s, &g).as_deref(), Some("box-a"));
        assert!(super::super::master::lease_start_line(s.get(&g).unwrap(), "null", 1.0, None).contains(",\"gpu\":true,"), "its node's start hears it is a GPU timing");
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

    /// A live session between calls is charged the RAM its node measured it holding (the most so
    /// far), not its class's peak, which it is charged only before that first look (a batch lease
    /// of a measured class keeps that weight: `a_running_measured_lease_keeps_its_ram_weight`).
    /// A live repro whose class measured 11.5 GB, charged that on a Mac's 9.4 GB budget while it
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
        let mut repro = on_mac("repro", Kind::Test, None);
        repro.live = true;
        let repro = s.request(repro, 0);
        assert_eq!(grants(&s.tick(0)), vec![repro.clone()]);
        assert!((s.charged(s.get(&repro).unwrap()).ram_gb - 8.44).abs() < 1e-9, "before a look: its class's peak, cut to the budget");
        let prebuild = s.request(on_mac("prebuild", Kind::Build, Some(3.6)), 1);
        assert!(grants(&s.tick(1)).is_empty());
        assert!(state(&s, &prebuild).1.contains("budget ram 8.4 GB, in use 8.4 GB, asked 3.6 GB"), "{:?}", state(&s, &prebuild));
        s.live_idle.insert(repro.clone());
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

    /// Three JIT test copies peaked at 8 GB each beside another lease and exhausted a 30 GB box.
    /// A measured batch keeps its granted weight while growing; first runs and idle live
    /// sessions use their held RAM, and cleanup retains only what the ended run held.
    /// A tests shard that waited 10 min starts on the RAM its box has free, down to a third of its
    /// need: one asking 42.7 GB headed the queue 14 min for a near-empty box (L605885, 11-07). Its
    /// run starts its units within what it got (`DEVHUB_RAM_MB`).
    #[test]
    fn a_tests_shard_that_waited_takes_the_ram_its_box_has_free() {
        let min = 60_000;
        for (lane_gb, fits) in [(20.0, true), (45.0, false)] {
            let mut s = sched(&[("pool-a", caps(16, 60.0, Role::Pool))]);
            s.record_resources("shard", "test", Some(35.6), None);
            let mut lane = overlay("lane", Some("pool-a"), 2.0);
            lane.ram_gb = Some(lane_gb);
            let lane = s.request(lane, 0);
            assert_eq!(grants(&s.tick(0)), vec![lane.clone()]);
            let mut shard = req(TESTS_LANE, Kind::Test, None);
            (shard.class, shard.cores) = ("shard".into(), Some(4.0));
            let shard = s.request(shard, 1);
            assert!(grants(&s.tick(1)).is_empty(), "42.7 GB beside {lane_gb}: no room");
            beat_all(&mut s, 10 * min);
            assert!(grants(&s.tick(10 * min)).is_empty(), "not 10 min since its ask yet");
            beat_all(&mut s, 10 * min + 1);
            let granted = grants(&s.tick(10 * min + 1));
            if !fits {
                assert!(granted.is_empty(), "under a third of its need free: it waits");
                continue;
            }
            assert_eq!(granted, vec![shard.clone()], "{:?}", state(&s, &shard));
            let got = s.get(&shard).unwrap().cost.ram_gb;
            let budget = s.get(&shard).unwrap().req.ram_budget(s.nodes["pool-a"].caps.as_ref().unwrap());
            let free = budget - s.charged(s.get(&lane).unwrap()).ram_gb;
            assert!(got < 42.0 && (got - free).abs() < 0.01, "its box's free RAM {free}: {got}");
            assert!((s.charged(s.get(&shard).unwrap()).ram_gb - got).abs() < 0.01, "charged what it got");
            assert!((s.ram_weighed(s.get(&shard).unwrap()).0 - got).abs() < 0.01, "its node's guard weighs it at what it got, not its uncut 42.7 GB");
            invariants(&s);
        }
    }

    /// A shard's floor (`--ram-floor-gb`: its largest unit's last peak) bounds its resize: two
    /// shards resized to 10.6 and 10.8 GB of one 30 GB box ran units of 18-28 GB, and the kernel's
    /// OOM killer took one (11-08). Charged at least its floor, it starts only where that much is
    /// free, and a box that could never give it that takes none.
    #[test]
    fn a_tests_shard_never_resizes_below_its_largest_units_peak() {
        let min = 60_000;
        let mut s = sched(&[("small", caps(16, 30.0, Role::Pool)), ("big", caps(16, 60.0, Role::Pool))]);
        s.record_resources("shard", "test", Some(35.6), None);
        let mut lane = overlay("lane", Some("big"), 2.0);
        lane.ram_gb = Some(30.0);
        let lane = s.request(lane, 0);
        assert_eq!(grants(&s.tick(0)), vec![lane.clone()]);
        let mut shard = req(TESTS_LANE, Kind::Test, None);
        (shard.class, shard.cores, shard.ram_floor_gb) = ("shard".into(), Some(4.0), Some(27.0));
        let shard = s.request(shard, 1);
        beat_all(&mut s, 10 * min + 1);
        assert!(grants(&s.tick(10 * min + 1)).is_empty(), "an empty 30 GB box never gives it 27 GB, nor 21 GB free beside the lane: {:?}", state(&s, &shard));
        invariants(&s);
        release_clean(&mut s, &lane, 11 * min);
        beat_all(&mut s, 11 * min);
        assert_eq!(grants(&s.tick(11 * min)), vec![shard.clone()], "{:?}", state(&s, &shard));
        assert_eq!(node_of(&s, &shard).as_deref(), Some("big"));
        assert!(s.get(&shard).unwrap().cost.ram_gb >= 27.0, "{}", s.get(&shard).unwrap().cost.ram_gb);
        invariants(&s);
    }

    /// A tests shard that waited 10 min starts on the cores its box has free too, down to a third
    /// of its ask, and the lane test its hold kept off goes in beside it: one asking 12 of 16 waited
    /// 60 min with 12-13 in use there (L620174, 11-07).
    #[test]
    fn a_tests_shard_that_waited_takes_the_cores_its_box_has_free() {
        let min = 60_000;
        for (busy, fits) in [(8.0, true), (13.0, false)] {
            let mut s = sched(&[("pool-a", caps(16, 64.0, Role::Pool))]);
            let lane = s.request(overlay("lane", Some("pool-a"), busy), 0);
            assert_eq!(grants(&s.tick(0)), vec![lane.clone()]);
            let mut shard = req(TESTS_LANE, Kind::Test, None);
            shard.cores = Some(12.0);
            let shard = s.request(shard, 1);
            assert!(grants(&s.tick(1)).is_empty(), "12 beside {busy}: no room");
            beat_all(&mut s, 10 * min);
            assert!(grants(&s.tick(10 * min)).is_empty(), "not 10 min since its ask yet");
            beat_all(&mut s, 10 * min + 1);
            let granted = grants(&s.tick(10 * min + 1));
            if !fits {
                assert!(granted.is_empty(), "under a third of its ask free: it waits");
                continue;
            }
            assert_eq!(granted, vec![shard.clone()], "{:?}", state(&s, &shard));
            assert_eq!(s.get(&shard).unwrap().cost.cores, 8.0, "the 8 its box had free");
            invariants(&s);
        }
    }

    /// A landing gate that waited a minute starts on the cores its box has free, down to 4 of its
    /// 8: one asking 8 of 16 with 13 in use by a tests shard and live sessions kept every hand-in
    /// waiting 15 min (L657290, 11-08).
    #[test]
    fn a_landing_gate_that_waited_a_minute_takes_the_cores_its_box_has_free() {
        for (busy, got) in [(11.0, Some(5.0)), (13.0, None)] {
            let mut s = sched(&[("pool-a", caps(16, 64.0, Role::Pool))]);
            let mut long = overlay("lane", Some("pool-a"), busy);
            long.est_min = Some(60.0);
            let long = s.request(long, 0);
            assert_eq!(grants(&s.tick(0)), vec![long.clone()]);
            let mut gate = req("landq-fast", Kind::Gate, None);
            (gate.cores, gate.priority) = (Some(8.0), Priority::Interactive);
            let gate = s.request(gate, 1);
            assert!(grants(&s.tick(1)).is_empty(), "8 beside {busy}: no room");
            beat_all(&mut s, 60_000);
            assert!(grants(&s.tick(60_000)).is_empty(), "not a minute since its ask yet");
            beat_all(&mut s, 60_001);
            let granted = grants(&s.tick(60_001));
            let Some(got) = got else {
                assert!(granted.is_empty(), "under 4 cores free: it waits, {:?}", state(&s, &gate));
                continue;
            };
            assert_eq!(granted, vec![gate.clone()], "{:?}", state(&s, &gate));
            assert_eq!(s.get(&gate).unwrap().cost.cores, got, "the {got} its box had free");
            invariants(&s);
        }
    }

    /// Past its first minute a landing gate's box drains for it: newer work, live sessions too,
    /// goes beside it only where it would still start now, and it takes the first cores its box
    /// frees. Sessions let in beside its room kept 13 of 16 in use behind a tests shard (L657290, 11-08).
    #[test]
    fn a_landing_gate_past_its_first_minute_takes_the_first_cores_its_box_frees() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.est_min = Some(min);
            r
        };
        let shard = s.request(ask("shard", 9.0, 60.0), 0);
        let short = s.request(ask("short", 4.0, 5.0), 0);
        s.tick(0);
        let mut gate = ask("landq-fast", 8.0, 5.0);
        (gate.kind, gate.priority) = (Kind::Gate, Priority::Interactive);
        let gate = s.request(gate, 1);
        beat_all(&mut s, MIN + 1);
        s.tick(MIN + 1);
        assert_eq!(s.holds["a"].id, gate, "{:?}", state(&s, &gate));
        let mut session = ask("session", 2.0, 30.0);
        (session.live, session.priority) = (true, Priority::Interactive);
        let session = s.request(session, MIN + 2);
        beat_all(&mut s, MIN + 2);
        s.tick(MIN + 2);
        assert!(state(&s, &session).1.contains(&format!("held for landing gate {gate}")), "3 free of the 4 it starts on: {:?}", state(&s, &session));
        release_clean(&mut s, &short, 5 * MIN);
        beat_all(&mut s, 5 * MIN);
        s.tick(5 * MIN);
        assert_eq!(node_of(&s, &gate).as_deref(), Some("a"), "{:?}", state(&s, &gate));
        assert_eq!(s.get(&gate).unwrap().cost.cores, 7.0, "the 7 its box had free");
        // the gate's hold gone, the session goes ahead of the background shard: 9 + 7 + 2 of 16
        assert_eq!(node_of(&s, &session).as_deref(), Some("a"), "{:?}", state(&s, &session));
        assert_eq!(s.past_budget(s.get(&session).unwrap()), 2.0);
        assert!(s.get(&shard).is_some());
        invariants(&s);
    }

    /// The fast gate asks interactive: a lane's background whole ask holds a box against it only
    /// once it could take the box. Background, the gate waited 15 min on a box a tests shard kept
    /// busy, the two others held by a lane's whole asks waiting on live sessions (L657290, 11-08).
    #[test]
    fn an_interactive_landing_gate_starts_on_a_box_a_lane_whole_ask_still_waits_for() {
        const MIN: u64 = 60_000;
        for interactive in [false, true] {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
            let ask = |lane: &str, node: Option<&str>, cores: f64, min: f64| {
                let mut r = overlay(lane, node, cores);
                r.est_min = Some(min);
                r
            };
            let shard = s.request(ask(TESTS_LANE, Some("a"), 12.0, 60.0), 0);
            let mut session = ask("session", Some("b"), 2.0, 30.0);
            (session.live, session.priority) = (true, Priority::Interactive);
            let lane = s.request(session, 0);
            s.tick(0);
            let mut whole = ask("flakes", None, 4.0, 15.0);
            whole.whole = true;
            let whole = s.request(whole, 1);
            beat_all(&mut s, 1);
            s.tick(1);
            assert_eq!(s.holds.get("b").map(|h| h.id.as_str()), Some(whole.as_str()), "{:?}", state(&s, &whole));
            let mut gate = req("landq-fast", Kind::Gate, None);
            gate.cores = Some(8.0);
            if interactive { gate.priority = Priority::Interactive; }
            let gate = s.request(gate, 2);
            beat_all(&mut s, 2);
            let granted = grants(&s.tick(2));
            if !interactive {
                assert!(granted.is_empty(), "background: held off b by the whole ask, {:?}", state(&s, &gate));
                continue;
            }
            assert_eq!(granted, vec![gate.clone()], "{:?}", state(&s, &gate));
            assert_eq!(node_of(&s, &gate).as_deref(), Some("b"));
            invariants(&s);
            // the whole ask starts once the session and the gate end, ahead of later lane work
            release_clean(&mut s, &lane, 5 * MIN);
            release_clean(&mut s, &gate, 6 * MIN);
            let later = s.request(ask("later", None, 4.0, 30.0), 6 * MIN + 1);
            beat_all(&mut s, 6 * MIN + 2);
            s.tick(6 * MIN + 2);
            assert_eq!(node_of(&s, &whole).as_deref(), Some("b"), "{:?}", state(&s, &whole));
            assert_ne!(node_of(&s, &later).as_deref(), Some("b"));
            assert!(s.get(&shard).is_some());
            invariants(&s);
        }
    }

    /// Lane test work charged its class's measured cores (a cargo test's, near a whole box)
    /// starts on the cores its box has free, down to a third of them, once it waited as long as
    /// its runs take (2 min at least); an asked count waits for all of it. 1m45s lane tests asking
    /// 15 of 16 cores headed the queue 24-28 min (11-07).
    #[test]
    fn a_measured_test_that_waited_its_run_takes_the_cores_its_box_has_free() {
        let min = 60_000;
        let mut s = sched(&[("pool-a", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 {
            s.record_node_cost("test:x", "test", "pool-a", 105_000);
            s.record_resources("test:x", "test", Some(2.0), Some(15.0));
        }
        let busy = s.request(overlay("lane", Some("pool-a"), 8.0), 0);
        assert_eq!(grants(&s.tick(0)), vec![busy]);
        let mut measured = req("lane-m", Kind::Test, None);
        measured.class = "test:x".into();
        let mut asked = req("lane-a", Kind::Test, None);
        asked.cores = Some(15.0);
        let (measured, asked) = (s.request(measured, 1), s.request(asked, 1));
        assert!(grants(&s.tick(1)).is_empty(), "15 beside 8: no room");
        beat_all(&mut s, 2 * min);
        assert!(grants(&s.tick(2 * min)).is_empty(), "not 2 min since its ask yet");
        beat_all(&mut s, 2 * min + 1);
        assert_eq!(grants(&s.tick(2 * min + 1)), vec![measured.clone()], "{:?}", state(&s, &measured));
        assert_eq!(s.get(&measured).unwrap().cost.cores, 8.0, "the 8 its box had free");
        assert_eq!(state(&s, &asked).0, "queued", "an asked count waits for all of it");
        invariants(&s);
    }

    /// A lease pinned to a busy box while another could start it asks its holder to unpin it or
    /// say why after 5 min, once: an inbox warning naming its lane and agent, and its queue line (a
    /// build waited 17 min pinned to a busy box with two others free, 11-07). devhub's own ask
    /// (a lane's spare warm leg) is not asked: devhub chose its box (11-08). Nor is a series
    /// (`--series`): its box is its reason, and its queue line says so.
    #[test]
    fn a_pinned_wait_with_another_box_free_asks_to_unpin() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool)), ("box-b", caps(16, 64.0, Role::Pool))]);
        let mut big = req("big", Kind::Build, Some("box-a"));
        big.cores = Some(16.0);
        let big = s.request(big, 0);
        assert_eq!(grants(&s.tick(0)), vec![big]);
        let mut warm = req(&format!("{}lane", super::super::WARM_LANE_PREFIX), Kind::Build, Some("box-a"));
        (warm.cores, warm.agent) = (Some(4.0), Some("devhub-warm".into()));
        let warm = s.request(warm, 1);
        let mut pinned = req("lane", Kind::Build, Some("box-a"));
        (pinned.cores, pinned.agent) = (Some(4.0), Some("AGENT".into()));
        let pinned = s.request(pinned, 1);
        let mut series = req("bench", Kind::Build, Some("box-a"));
        (series.cores, series.agent, series.series) = (Some(4.0), Some("AGENT".into()), Some("cold-start".into()));
        assert_eq!(Req::parse(&series.to_json()).unwrap().series.as_deref(), Some("cold-start"), "a series round-trips");
        assert!(Req::parse(&req("bench", Kind::Build, None).to_json().replacen('{', "{\"series\":\"cold-start\",", 1)).unwrap_err().contains("it takes --node"));
        let series = s.request(series, 1);
        let warned = |a: &[Action]| a.iter().filter_map(|a| match a { Action::Event(r) if r.line.contains(" pinned to ") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        assert!(warned(&s.tick(1)).is_empty());
        assert!(state(&s, &pinned).1.contains("it named box-a (--node): box-b could start it now"), "{:?}", state(&s, &pinned));
        assert!(!state(&s, &pinned).1.contains("unpin"), "not before 5 min");
        let min = 60_000;
        beat_all(&mut s, 5 * min + 1);
        let said = warned(&s.tick(5 * min + 1));
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains(&format!("{pinned} build of lane (AGENT) waited 5m00s pinned to box-a (--node) while box-b could start it now: unpin it (ask again with `--needs os=linux,arch=x86_64` in place of --node), or say why it needs box-a")), "{}", said[0]);
        assert!(state(&s, &pinned).1.ends_with("could start it now: unpin it (`--needs os=linux,arch=x86_64` in place of --node), or say why it needs box-a"), "{:?}", state(&s, &pinned));
        assert!(state(&s, &warm).1.ends_with("it named box-a (--node): box-b could start it now"), "devhub moves its own: {:?}", state(&s, &warm));
        assert!(state(&s, &series).1.ends_with("it waits for box-a, the box of its series cold-start (--series)"), "a series is never asked to unpin: {:?}", state(&s, &series));
        assert_eq!(s.starts_elsewhere(&format!("{}lane", super::super::WARM_LANE_PREFIX), 5 * min + 1), vec!["box-b".to_string()], "where warm may move it");
        beat_all(&mut s, 5 * min + 2);
        assert!(warned(&s.tick(5 * min + 2)).is_empty(), "said once");
    }

    /// A timed lease pinned to a bench box waits for it, said in its queue line, and is never asked
    /// to unpin to another box that could start it: timed work belongs there (11-10).
    #[test]
    fn a_timed_lease_pinned_to_its_bench_box_is_never_asked_to_unpin() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Bench)), ("box-b", caps(16, 64.0, Role::Bench))]);
        let running = s.request(req("running", Kind::Timed, Some("box-a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![running]);
        let mut pinned = req("lane", Kind::Timed, Some("box-a"));
        pinned.agent = Some("AGENT".into());
        let pinned = s.request(pinned, 1);
        let min = 60_000;
        beat_all(&mut s, 5 * min + 1);
        let said: Vec<String> = s.tick(5 * min + 1).iter().filter_map(|a| match a { Action::Event(r) if r.line.contains(" pinned to ") => Some(r.line.clone()), _ => None }).collect();
        assert_eq!(s.free_elsewhere(s.get(&pinned).unwrap(), &s.holds), vec!["box-b".to_string()], "another box could start it");
        assert!(said.is_empty(), "{said:?}");
        assert!(state(&s, &pinned).1.ends_with("it waits for box-a, a bench box, where timed runs belong"), "{:?}", state(&s, &pinned));
    }

    /// The hint offers only boxes of the named box's platform, and none for a box's own prebuild:
    /// it named the Windows box for Linux-only work (11-07).
    #[test]
    fn a_pinned_wait_is_offered_only_boxes_of_its_platform() {
        let mut win = caps(16, 64.0, Role::Pool);
        win.os = "windows".into();
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool)), ("box-w", win)]);
        let mut big = req("big", Kind::Build, Some("box-a"));
        big.cores = Some(16.0);
        let big = s.request(big, 0);
        assert_eq!(grants(&s.tick(0)), vec![big]);
        let mut pinned = req("lane", Kind::Build, Some("box-a"));
        pinned.cores = Some(4.0);
        let pinned = s.request(pinned, 1);
        s.tick(1);
        assert!(!state(&s, &pinned).1.contains("could start it now"), "{:?}", state(&s, &pinned));
        assert!(s.starts_elsewhere("lane", 2).is_empty());
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool)), ("box-b", caps(16, 64.0, Role::Pool))]);
        let mut big = req("big", Kind::Build, Some("box-a"));
        big.cores = Some(16.0);
        let big = s.request(big, 0);
        assert_eq!(grants(&s.tick(0)), vec![big]);
        let prebuild = format!("{}box-a", super::super::PREBUILD_LANE_PREFIX);
        let hub = format!("{HUB_BUILD}-box-a");
        for lane in ["lane", prebuild.as_str(), hub.as_str()] {
            let mut r = req(lane, Kind::Build, Some("box-a"));
            r.cores = Some(4.0);
            if lane == hub { (r.class, r.master) = (HUB_BUILD.into(), true); }
            s.request(r, 1);
        }
        assert_eq!(s.starts_elsewhere("lane", 2), vec!["box-b".to_string()]);
        assert!(s.starts_elsewhere(&prebuild, 2).is_empty(), "a box's own prebuild runs on it");
        assert!(s.starts_elsewhere(&hub, 2).is_empty(), "and its hub build");
    }

    #[test]
    fn a_running_measured_lease_keeps_its_ram_weight() {
        let mut small = caps(16, 30.0, Role::Pool);
        small.ram_resident_gb = 4.0;
        let mut s = sched(&[("small", small)]);
        let ask = |lane: &str| {
            let mut r = overlay(lane, None, 1.0);
            r.class = "rt".into();
            r
        };
        let first = s.request(ask("first"), 0);
        assert_eq!(grants(&s.tick(0)), vec![first.clone()], "the first run starts without a measurement");
        assert_eq!(s.charged(s.get(&first).unwrap()).ram_gb, 6.0, "the first run starts at its kind's default");
        s.set_info(&first, r#"{"held_mb":1024}"#, 1);
        assert_eq!(s.charged(s.get(&first).unwrap()).ram_gb, 1.0, "the first look replaces an unmeasured default");
        release_clean(&mut s, &first, 2);
        s.record_resources("rt", "test", Some(8.0), None);

        let second = s.request(ask("second"), 3);
        let third = s.request(ask("third"), 3);
        assert_eq!(grants(&s.tick(3)).len(), 2, "two measured runs fit the 23 GB budget");
        for id in [&second, &third] {
            assert_eq!(s.get(id).unwrap().cost.ram_gb, 9.6, "each measured grant weighs 9.6 GB");
            assert_eq!(s.charged(s.get(id).unwrap()).ram_gb, 9.6, "the measured weight stands before a look");
            s.set_info(id, r#"{"held_mb":1024}"#, 4);
            assert_eq!(s.charged(s.get(id).unwrap()).ram_gb, 9.6, "a small first look must not lower a measured weight");
        }
        let fourth = s.request(ask("fourth"), 5);
        assert!(grants(&s.tick(5)).is_empty(), "a fourth run must wait for RAM");
        assert!(state(&s, &fourth).1.contains("budget ram 23.0 GB, in use 19.2 GB, asked 9.6 GB"), "the wait must count measured weights: {:?}", state(&s, &fourth));
        // a master restart keeps each grant's weight: the new master restores before it reads its measures
        let mut back_caps = caps(16, 30.0, Role::Pool);
        back_caps.ram_resident_gb = 4.0;
        let mut back = sched(&[("small", back_caps)]);
        for line in s.live_lines() { back.restore(&line, 5); }
        for id in [&second, &third] {
            back.set_info(id, r#"{"held_mb":1024}"#, 6);
            assert_eq!(back.charged(back.get(id).unwrap()).ram_gb, 9.6, "a restart must not lower a measured weight to what the run holds");
        }
        s.set_info(&second, r#"{"held_mb":12288}"#, 6);
        assert_eq!(s.charged(s.get(&second).unwrap()).ram_gb, 12.0, "held RAM above the grant raises its charge");

        s.add_node("big", "192.0.2.2", None);
        s.set_caps("big", caps(16, 64.0, Role::Pool), 7);
        assert_eq!(grants(&s.tick(7)), vec![fourth.clone()], "the waiting run starts when a second box is available");
        assert_eq!(node_of(&s, &fourth).as_deref(), Some("big"), "the waiting run uses the second box");
        let mut live = ask("live");
        live.live = true;
        live.node = Some("big".into());
        let live = s.request(live, 8);
        assert_eq!(grants(&s.tick(8)), vec![live.clone()], "the measured live session starts");
        assert_eq!(s.get(&live).unwrap().cost.ram_gb, 9.6, "the live session has the measured grant weight");
        s.live_idle.insert(live.clone());
        s.set_info(&live, r#"{"held_mb":2048}"#, 9);
        assert_eq!(s.charged(s.get(&live).unwrap()).ram_gb, 2.0, "an idle live session uses its held RAM");
        s.release(&third, "done", 10);
        assert_eq!(s.restoring.get(&third).unwrap().2.ram_gb, 1.0, "cleanup keeps held RAM without the measured weight");
    }

    /// Work waits for an eligible box that can hold its whole need, even while a smaller box
    /// is idle. It still takes a cut budget when every eligible box is too small or named.
    #[test]
    fn an_unasked_ram_need_waits_for_a_box_that_can_hold_it() {
        let mut small = caps(16, 30.0, Role::Pool);
        small.ram_resident_gb = 4.0;
        let big = caps(16, 64.0, Role::Pool);
        let mut s = sched(&[("small", small.clone()), ("big", big.clone())]);
        s.record_resources("huge", "test", Some(24.0), None);
        let mut busy = overlay("busy", Some("big"), 1.0);
        busy.ram_gb = Some(busy.ram_budget(&big));
        let busy = s.request(busy, 0);
        assert_eq!(grants(&s.tick(0)), vec![busy.clone()], "the other lease fills the big box's RAM budget");
        let mut huge = overlay("huge", None, 1.0);
        huge.class = "huge".into();
        huge.priority = Priority::Interactive;
        let waiting = s.request(huge.clone(), 1);
        assert!(grants(&s.tick(1)).is_empty(), "the oversized need must not start on idle small");
        assert_eq!(state(&s, &waiting).0, "queued", "the oversized need waits rather than being denied");
        assert!(state(&s, &waiting).1.contains("budget ram 54.4 GB, in use 54.4 GB, asked 28.8 GB"), "the wait describes the big box's RAM: {:?}", state(&s, &waiting));
        assert_eq!(s.holds.get("big").map(|h| h.id.as_str()), Some(waiting.as_str()), "the wait reserves the box that can hold the need");
        assert!(!s.holds.contains_key("small"), "the undersized box is not reserved");
        release_clean(&mut s, &busy, 2);
        assert_eq!(grants(&s.tick(2)), vec![waiting.clone()], "the waiting run starts after cleanup");
        assert_eq!(node_of(&s, &waiting).as_deref(), Some("big"), "the measured need runs on the big box");

        let mut only_small = sched(&[("small", small)]);
        only_small.record_resources("huge", "test", Some(24.0), None);
        let only = only_small.request(huge.clone(), 0);
        assert_eq!(grants(&only_small.tick(0)), vec![only.clone()], "a sole undersized box still grants the request");
        assert_eq!(node_of(&only_small, &only).as_deref(), Some("small"), "the sole box takes the cut");
        assert_eq!(only_small.get(&only).unwrap().cost.ram_gb, 23.0, "the cut takes the sole box's whole budget");
        huge.node = Some("small".into());
        let named = s.request(huge, 3);
        assert_eq!(grants(&s.tick(3)), vec![named.clone()], "naming the small box still grants a cut");
        assert_eq!(node_of(&s, &named).as_deref(), Some("small"), "the explicitly named box is used");
        assert_eq!(s.get(&named).unwrap().cost.ram_gb, 23.0, "the named box contributes its whole budget");
    }

    /// Work that names no box takes a pool box that fits, even a busy one, over an idle bench box
    /// the lane is warm on; when no pool box fits it waits for one, the bench box idle meanwhile
    /// (its quiet window is timed work's). A bench box takes it while no pool box is up, and never
    /// while a timed request waits for it, which then is not held for that work. A lane's ask that
    /// names it is refused ([`BENCH_PIN`]); devhub's own runs there by name.
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
        // a lane's ask by name is refused, saying why; a timed one and an inspect are not
        let mut named = ask("named", Kind::Test, 8.0, true);
        named.node = Some("bench".into());
        assert_eq!(s.never_places(&named).as_deref(), Some(BENCH_PIN));
        let refused = s.request(named.clone(), 3);
        s.tick(3);
        assert_eq!(state(&s, &refused).0, "ended denied", "{:?}", state(&s, &refused));
        assert!(state(&s, &refused).1.contains(BENCH_PIN), "{:?}", state(&s, &refused));
        for kind in [Kind::Timed, Kind::Inspect] {
            assert_eq!(s.never_places(&Req { kind, ..named.clone() }), None, "{kind:?}");
        }
        // devhub's own by name runs there
        let named = s.request(devhubs(named), 3);
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
        let mut build = devhubs(req("build", Kind::Build, Some("m1")));
        build.needs = vec![("os".into(), "macos".into())];
        build.est_min = Some(5.0);
        let build = s.request(build, 3);
        s.tick(3);
        assert_eq!(node_of(&s, &build).as_deref(), Some("m1"));
        let c = s.request(timed("c", None), 4);
        s.tick(4);
        assert_eq!(node_of(&s, &c), None, "{:?}", state(&s, &c));
        assert_eq!(s.holds.get("m1").map(|h| h.id.clone()), Some(c.clone()), "it drains the bench Mac");
        assert!(s.why_queued(&c, 4).unwrap().iter().any(|w| w.contains("a spare box for timed work: it waits while m1 can take it")));
        let d = s.request(timed("d", Some("m2")), 5);
        s.tick(5);
        assert_eq!(node_of(&s, &d).as_deref(), Some("m2"), "named: {:?}", state(&s, &d));
        invariants(&s);
    }

    /// A pool box whose lease.conf says `timed = spare` is a pool box between timed runs. While the
    /// bench box can take a timed ask, the spare is in no timed line (its deployment and prebuilds
    /// keep their course); once the bench box runs timed work, the next timed ask holds the spare,
    /// which drains for it, runs it whole and takes pool work again after it. A plain pool box
    /// takes no timed work, and devhub's own timed checks keep to the bench boxes.
    #[test]
    fn a_spare_pool_box_drains_for_timed_work_only_while_the_bench_box_is_taken() {
        let mut spare = caps(16, 64.0, Role::Pool);
        spare.timed_spare = true;
        let mut s = sched(&[("bench", caps(16, 64.0, Role::Bench)), ("spare", spare), ("pool", caps(16, 64.0, Role::Pool))]);
        let on_spare = |lane: &str, est: f64| {
            let mut r = req(lane, Kind::Test, Some("spare"));
            r.est_min = Some(est);
            r
        };
        let work = s.request(on_spare("work", 5.0), 0);
        s.tick(0);
        assert_eq!(node_of(&s, &work).as_deref(), Some("spare"), "{:?}", state(&s, &work));
        let mut a = req("a", Kind::Timed, None);
        a.est_min = Some(30.0);
        let a = s.request(a, 1);
        assert_eq!(s.first_timed("spare").map(|r| r.id.clone()), None, "the bench box can take it: not in the spare's line");
        assert_eq!(s.first_timed("bench").map(|r| r.id.clone()), Some(a.clone()));
        s.tick(1);
        assert_eq!(node_of(&s, &a).as_deref(), Some("bench"), "the bench box first: {:?}", state(&s, &a));
        let b = s.request(req("b", Kind::Timed, None), 2);
        s.tick(2);
        assert_eq!(node_of(&s, &b), None, "{:?}", state(&s, &b));
        assert_eq!(s.holds.get("spare").map(|h| h.id.clone()), Some(b.clone()), "the spare drains for it: {:?}", state(&s, &b));
        let later = s.request(on_spare("later", 30.0), 3);
        s.tick(3);
        let line = state(&s, &later).1;
        assert!(line.starts_with(&format!("held for timed {b} (the box drains for it: only work that ends first or fits beside it)")), "{line}");
        assert_eq!(s.first_timed("spare").map(|r| r.id.clone()), Some(b.clone()), "it holds the spare: in its line");
        release_clean(&mut s, &work, 4);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(node_of(&s, &b).as_deref(), Some("spare"), "{:?}", state(&s, &b));
        assert_eq!(node_of(&s, &later), None, "it runs whole: {:?}", state(&s, &later));
        let mut leg = req("jitlegs-warm", Kind::Timed, None);
        leg.agent = Some("devhub-jitlegs".into());
        let leg = s.request(leg, 4);
        release_clean(&mut s, &b, 5);
        beat_all(&mut s, 5);
        s.tick(5);
        assert_eq!(node_of(&s, &later).as_deref(), Some("spare"), "pool work again: {:?}", state(&s, &later));
        assert_eq!(node_of(&s, &leg), None, "devhub's own timed check waits for the bench box: {:?}", state(&s, &leg));
        assert!(s.holds.get("spare").is_none());
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
    /// other at once. A lane's that names the held bench Mac is refused ([`BENCH_PIN`]); devhub's
    /// own waits for it, and its queue line says it named that box and which other one could start
    /// it now.
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
        // a lane's ask that names the bench Mac is refused at once ([`BENCH_PIN`])
        let refused = s.request(on_mac("named", Kind::Test, Some("bench")), 3);
        let named = s.request(devhubs(on_mac("named", Kind::Test, Some("bench"))), 3);
        beat_all(&mut s, 3);
        s.tick(3);
        let (st, why) = state(&s, &refused);
        assert!(st == "ended denied" && why.contains(BENCH_PIN), "{st}: {why}");
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

    /// A Linux box whose tasks all stall on memory (PSI full avg10 past 10%) starts nothing new
    /// until it eases; what runs there goes on. The stall is said as it starts and as it eases,
    /// with how long and how deep, and the nodes line shows it.
    #[test]
    fn a_linux_box_stalled_on_memory_starts_nothing_new() {
        let stalled = |pct| {
            let mut c = caps(10, 16.0, Role::Hot);
            c.mem_stall = pct;
            c
        };
        let ask = |lane: &str| {
            let mut r = overlay(lane, Some("box-a"), 1.0);
            r.ram_gb = Some(1.0);
            r
        };
        let said = |a: Vec<Action>| a.into_iter().filter_map(|x| match x { Action::Event(r) if r.line.starts_with("box-a: memory stall") => Some((r.sev, r.line)), _ => None }).collect::<Vec<_>>();
        let mut s = sched(&[("box-a", stalled(0.0))]);
        let a = s.request(ask("a"), 0);
        assert_eq!(grants(&s.tick(0)), vec![a.clone()]);
        s.set_caps("box-a", stalled(4.0), 5);
        assert!(said(s.tick(5)).is_empty(), "4% is no stall");
        s.set_caps("box-a", stalled(14.2), 10);
        let b = s.request(ask("b"), 10);
        let out = s.tick(10);
        assert_eq!(said(out.clone()), vec![(Sev::Warn, "box-a: memory stall 14% (PSI full avg10): no new lease until it eases".to_string())]);
        assert!(grants(&out).is_empty());
        let (st, why) = state(&s, &b);
        assert!(st == "queued" && why.contains("memory stall 14% (PSI full avg10)"), "{st}: {why}");
        assert!(matches!(state(&s, &a).0.as_str(), "starting" | "granted"), "what runs there goes on: {:?}", state(&s, &a));
        let line = s.node_view(10).into_iter().find(|n| n.name == "box-a").unwrap().caps_line();
        assert!(line.contains("memory stall 14% (PSI full avg10): no new lease until it eases"), "{line}");
        s.set_caps("box-a", stalled(31.0), 20);
        assert!(said(s.tick(20)).is_empty(), "said once");
        s.beat(&b, 20_000);
        s.set_caps("box-a", stalled(2.0), 20_010);
        let out = s.tick(20_010);
        assert_eq!(said(out.clone()), vec![(Sev::Info, "box-a: memory stall eased after 20 s (at most 31% PSI full avg10)".to_string())]);
        assert_eq!(grants(&out), vec![b.clone()], "eased, it starts: {:?}", state(&s, &b));
        let c = Caps::parse(&s.nodes["box-a"].caps.as_ref().unwrap().to_json()).unwrap();
        assert_eq!(c.mem_stall, 2.0, "the hub's answer carries it");
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
        let line = |s: &Sched, name: &str| s.node_view(0).into_iter().find(|n| n.name == name).unwrap().caps_line();
        assert!(line(&s, "slow").contains(" \u{b7} clock hpet: no timed leases (TSC unstable since boot"), "the node line says its clock and why it takes no timed lease: {}", line(&s, "slow"));
        assert!(line(&s, "fast").contains(" \u{b7} clock tsc") && !line(&s, "fast").contains("no timed"), "a box on the TSC says its clock only: {}", line(&s, "fast"));
        let any = s.request(req("bench", Kind::Timed, None), 1);
        let named = s.request(req("series", Kind::Timed, Some("slow")), 1);
        s.tick(1);
        assert_eq!(node_of(&s, &any).as_deref(), Some("fast"), "{:?}", state(&s, &any));
        let (st, why) = state(&s, &named);
        assert!(st == "ended denied" && why.contains("no timed leases: clocksource hpet"), "{st}: {why}");
        // other work still runs there (devhub's own by name)
        let build = s.request(devhubs(req("lane", Kind::Build, Some("slow"))), 2);
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
            let mut r = devhubs(req(lane, Kind::Test, Some("a")));
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

    /// A box's own prebuild runs only in a gap of its box's timed queue: while a lane's timed ask
    /// waits for the box it waits too, holding nothing; with the queue empty it starts, and a timed
    /// ask then waits for it.
    #[test]
    fn a_box_prebuild_runs_only_in_a_gap_of_its_timed_queue() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let running = s.request(req("run", Kind::Test, Some("a")), 0);
        s.tick(0);
        let timed = s.request(req("bench", Kind::Timed, Some("a")), 1);
        s.tick(1);
        let mut r = overlay("prebuild-a", Some("a"), 4.0);
        (r.kind, r.est_min) = (Kind::Build, Some(5.0));
        let prebuild = s.request(r, 2);
        s.tick(2);
        assert_eq!(node_of(&s, &prebuild), None, "{:?}", state(&s, &prebuild));
        assert!(state(&s, &prebuild).1.contains(&format!("in a gap of a's timed queue: {timed} of bench waits for the box")), "{:?}", state(&s, &prebuild));
        assert!(!s.holds.values().any(|h| h.id == prebuild), "{:?}", s.holds);
        release_clean(&mut s, &running, 3);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("a"), "not held for the prebuild: {:?}", state(&s, &timed));
        assert_eq!(node_of(&s, &prebuild), None, "{:?}", state(&s, &prebuild));
        release_clean(&mut s, &timed, 5);
        beat_all(&mut s, 6);
        s.tick(6);
        assert_eq!(node_of(&s, &prebuild).as_deref(), Some("a"), "the gap: {:?}", state(&s, &prebuild));
        let next = s.request(req("bench", Kind::Timed, Some("a")), 7);
        s.tick(7);
        assert_eq!(node_of(&s, &next), None, "{:?}", state(&s, &next));
        let l = s.get(&next).unwrap();
        let b = s.now_fits("a", l, &s.cost_on("a", l), &s.holds).unwrap_err();
        assert_eq!(b.reason, format!("held by {prebuild} (build of prebuild-a)"));
        release_clean(&mut s, &prebuild, 8);
        beat_all(&mut s, 9);
        s.tick(9);
        assert_eq!(node_of(&s, &next).as_deref(), Some("a"), "{:?}", state(&s, &next));
        invariants(&s);
    }

    /// A box's prebuild that waited past its run in gaps of a timed queue that never empties gets
    /// its gap made: it yields no more, the running timed lease runs on, the box's next timed
    /// grant waits for it, and timed work goes on after it. Its age is the box's first unserved
    /// ask's, whatever re-plans asked it again, until a prebuild runs there.
    #[test]
    fn a_box_prebuild_past_its_run_makes_its_gap_in_a_back_to_back_timed_queue() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Bench))]);
        let timed = |s: &mut Sched, lane: &str, at: u64| {
            let mut r = req(lane, Kind::Timed, Some("a"));
            r.est_min = Some(10.0);
            s.request(r, at)
        };
        let first = timed(&mut s, "one", 0);
        s.tick(0);
        assert_eq!(node_of(&s, &first).as_deref(), Some("a"));
        let ask = |s: &mut Sched, at: u64| {
            let mut r = devhubs(overlay("prebuild-a", Some("a"), 4.0));
            (r.kind, r.est_min) = (Kind::Build, Some(2.0));
            s.request(r, at)
        };
        let replaced = ask(&mut s, 1);
        let second = timed(&mut s, "two", 1);
        s.tick(1);
        assert!(state(&s, &replaced).1.contains(&format!("in a gap of a's timed queue: {second} of two waits for the box")), "{:?}", state(&s, &replaced));
        // a re-plan a minute in asks it again at the newer tip: the ask keeps the first's age
        s.release(&replaced, "released", 60_001);
        let prebuild = ask(&mut s, 60_001);
        assert_eq!(s.get(&prebuild).unwrap().asked_ms, 1, "the box's first unserved ask's age");
        let past = 1 + 120_000 + 1;
        beat_all(&mut s, past);
        s.tick(past);
        assert_eq!(node_of(&s, &first).as_deref(), Some("a"), "the running timed lease runs on");
        assert!(s.why_queued(&second, past).unwrap().iter().any(|w| w.contains(&format!("behind a's prebuild: {prebuild} waited 2m00s, past its run's 2m00s: it takes the next gap, timed work starts after it"))), "{:?}", s.why_queued(&second, past));
        assert!(!state(&s, &prebuild).1.contains("in a gap"), "{:?}", state(&s, &prebuild));
        release_clean(&mut s, &first, past + 1);
        beat_all(&mut s, past + 2);
        s.tick(past + 2);
        assert_eq!(node_of(&s, &prebuild).as_deref(), Some("a"), "the gap is made: {:?}", state(&s, &prebuild));
        assert_eq!(node_of(&s, &second), None, "{:?}", state(&s, &second));
        release_clean(&mut s, &prebuild, past + 3);
        beat_all(&mut s, past + 4);
        s.tick(past + 4);
        assert_eq!(node_of(&s, &second).as_deref(), Some("a"), "{:?}", state(&s, &second));
        let next = ask(&mut s, past + 5);
        assert_eq!(s.get(&next).unwrap().asked_ms, past + 5, "once one ran, a new ask's age is its own");
        invariants(&s);
    }

    /// A box's handoff runs from its release to the grant of a lease that stood in the queue then.
    /// A leg with its run's place ([`runs_place`]) asked before that release but entered the queue
    /// only when its run's previous leg ended, after it: its grant is no handoff (one such leg
    /// read 25 min, half of a Mac's two-sample median, 11-11). A lane's ask that waited is one.
    #[test]
    fn a_leg_queued_after_its_box_was_released_is_no_handoff() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Bench))]);
        let run = s.request(req("run", Kind::Timed, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![run.clone()]);
        release_clean(&mut s, &run, 10_000);
        let mut leg = req("jitlegs-warm", Kind::Timed, Some("a"));
        (leg.agent, leg.prio, leg.since_ms) = (Some("devhub-jitlegs".into()), DEFAULT_PRIO - 1, Some(1));
        let leg = s.request(leg, 600_000);
        assert_eq!(grants(&s.tick(600_000)), vec![leg.clone()]);
        let l = s.get(&leg).unwrap();
        assert_eq!((l.asked_ms, l.wait_ms), (1, None), "its place predates the release, its time in the queue does not");
        assert!(s.nodes["a"].lat.handoff.is_empty(), "{:?}", s.nodes["a"].lat.handoff);
        let lane = s.request(req("lane", Kind::Timed, Some("a")), 600_001);
        s.tick(600_001);
        release_clean(&mut s, &leg, 700_000);
        beat_all(&mut s, 700_500);
        assert_eq!(grants(&s.tick(700_500)), vec![lane.clone()]);
        assert_eq!(s.get(&lane).unwrap().wait_ms, Some(500), "a lane's ask that waited for the box");
        assert_eq!(s.nodes["a"].lat.handoff, [500]);
        invariants(&s);
    }

    /// devhub's own timed checks (jitlegs' warm leg) go after lanes' timed work on a box, asked
    /// first or not: they measure the fleet, a lane's ask its change. The leg's reservation goes
    /// once a lane's ask waits; with none waiting, the leg takes the box.
    #[test]
    fn devhubs_own_timed_leg_goes_after_lanes_timed_work() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Bench))]);
        let running = s.request(req("run", Kind::Timed, Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![running.clone()]);
        let mut leg = req("jitlegs-warm", Kind::Timed, Some("a"));
        (leg.agent, leg.prio) = (Some("devhub-jitlegs".into()), DEFAULT_PRIO - 1);
        let leg = s.request(leg, 1);
        s.tick(1);
        let lane = s.request(req("lane", Kind::Timed, Some("a")), 2);
        s.tick(2);
        let l = s.get(&leg).unwrap();
        let b = s.now_fits("a", l, &s.cost_on("a", l), &s.holds).unwrap_err();
        assert_eq!(b.reason, format!("in a gap of a's timed queue: {lane} of lane waits for the box"));
        assert!(!s.holds.values().any(|h| h.id == leg), "{:?}", s.holds);
        release_clean(&mut s, &running, 3);
        beat_all(&mut s, 4);
        assert_eq!(grants(&s.tick(4)), vec![lane.clone()], "{:?} {:?}", state(&s, &lane), state(&s, &leg));
        release_clean(&mut s, &lane, 5);
        beat_all(&mut s, 6);
        assert_eq!(grants(&s.tick(6)), vec![leg], "{:?}", s.holds);
        invariants(&s);
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

    /// A land queue's gate holds its box for as long as it waits, as an owed leg does: lane work
    /// that would not fit beside it and outlasts what runs there waits; work that ends first goes.
    #[test]
    fn a_landing_gate_keeps_its_box_from_newer_lane_work() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.est_min = Some(min);
            r
        };
        let running = s.request(ask("run", 8.0, 10.0), 0);
        s.tick(0);
        let mut gate = ask("landq-fast", 12.0, 10.0);
        gate.kind = Kind::Gate;
        let gate = s.request(gate, 1);
        s.tick(1);
        let lane = s.request(ask("lane", 6.0, 60.0), 2);
        let quick = s.request(ask("quick", 6.0, 1.0), 2);
        s.tick(2);
        assert!(state(&s, &lane).1.contains(&format!("held for landing gate {gate}")), "{:?}", state(&s, &lane));
        assert_eq!(node_of(&s, &quick).as_deref(), Some("a"), "it ends before the gate could start: {:?}", state(&s, &quick));
        beat_all(&mut s, HOLD_MS + 3);
        s.tick(HOLD_MS + 3);
        assert!(state(&s, &lane).1.contains(&format!("held for landing gate {gate}")), "past ten minutes it still holds: {:?}", state(&s, &lane));
        release_clean(&mut s, &running, HOLD_MS + 4);
        release_clean(&mut s, &quick, HOLD_MS + 4);
        beat_all(&mut s, HOLD_MS + 5);
        s.tick(HOLD_MS + 5);
        assert_eq!(node_of(&s, &gate).as_deref(), Some("a"), "{:?}", state(&s, &gate));
    }

    /// Beside a landing gate's hold is in the room the hold keeps: what came in beside it since it
    /// asked counts, what ran before it asked (what it waits for) not. Each interactive lease that
    /// fit beside the gate alone took the cores its blockers freed, one after the other, and the
    /// gate held a box behind four leases its hold had let in (11-07).
    #[test]
    fn beside_a_landing_gate_counts_what_came_in_beside_it() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.est_min = Some(min);
            r
        };
        let blockers: Vec<String> = (1..=3).map(|i| s.request(ask(&format!("run{i}"), 4.0, 10.0), 0)).collect();
        s.tick(0);
        let mut gate = ask("landq-fast", 8.0, 5.0);
        gate.kind = Kind::Gate;
        let gate = s.request(gate, 1);
        s.tick(1);
        let mut beside = Vec::new();
        for (i, blocker) in blockers.iter().enumerate() {
            let t = 2 + i as u64;
            release_clean(&mut s, blocker, t);
            let mut r = ask(&format!("ui{i}"), 4.0, 60.0);
            r.priority = Priority::Interactive;
            beside.push(s.request(r, t));
            beat_all(&mut s, t);
            s.tick(t);
        }
        assert_eq!(node_of(&s, &beside[0]).as_deref(), Some("a"), "4 beside the gate's 8: {:?}", state(&s, &beside[0]));
        assert_eq!(node_of(&s, &beside[1]).as_deref(), Some("a"), "4 + 4 beside the gate's 8: {:?}", state(&s, &beside[1]));
        assert_eq!(node_of(&s, &gate).as_deref(), Some("a"), "the gate takes the cores its last blocker freed, not a third 4 past its room: {:?}", state(&s, &gate));
        assert_eq!(state(&s, &beside[2]).0, "queued", "{:?}", state(&s, &beside[2]));
    }

    /// Beside an ask's room counts what stays on its box: a run from before it asked past its
    /// measure that it fits beside, and an ended lease's charge until its cleanup (the cores a
    /// cleanup frees go to the ask first). Once it waited long the box drains for it, as its queue line says: what runs
    /// there counts, whenever it came in. A 9-core interactive ask on a 16-core box headed its
    /// queue 28 min beside a 4-core run past its measure, while 4-core asks took the cores each
    /// end there freed (L755962, 11-10).
    #[test]
    fn an_asks_room_counts_what_stays_on_its_box_and_a_long_wait_drains_it() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("pool-a", caps(16, 64.0, Role::Pool))]);
        let ask = |s: &mut Sched, lane: &str, cores: f64, min: f64, at: u64| {
            let mut r = overlay(lane, Some("pool-a"), cores);
            (r.est_min, r.priority) = (Some(min), Priority::Interactive);
            s.request(r, at)
        };
        let over = ask(&mut s, "over", 4.0, 2.0, 0);
        let soon = ask(&mut s, "soon", 4.0, 12.0, 0);
        ask(&mut s, "late", 4.0, 40.0, 0);
        assert_eq!(grants(&s.tick(0)).len(), 3);
        let owner = ask(&mut s, "fablook", 9.0, 30.0, MIN);
        beat_all(&mut s, MIN);
        assert!(grants(&s.tick(MIN)).is_empty(), "12 of 16 in use");
        // past its measure, `over` stays: 4 + 4 + the owner's 9 of 16
        let first = ask(&mut s, "first", 4.0, 30.0, 3 * MIN);
        beat_all(&mut s, 3 * MIN);
        assert!(grants(&s.tick(3 * MIN)).is_empty(), "a run past its measure counts beside the owner: {:?}", state(&s, &first));
        assert!(state(&s, &first).1.contains(&format!("held for interactive {owner} (")), "{:?}", state(&s, &first));
        // `over` ends; the owner has waited past STARVED_MS: the box drains for it, and the run
        // that ends at 12 min counts too (let in, `first` would keep the owner off past it)
        let t = 11 * MIN + 30_000;
        release_clean(&mut s, &over, t);
        beat_all(&mut s, t);
        assert!(grants(&s.tick(t)).is_empty(), "the box drains for the owner: {:?}", state(&s, &first));
        assert!(state(&s, &first).1.ends_with("; it has queued 10m30s: the box drains for it"), "{:?}", state(&s, &first));
        // `soon` ends; its charge stays until its cleanup, and the cores that frees go to the owner
        s.release(&soon, "released", 12 * MIN);
        let small = ask(&mut s, "small", 3.0, 30.0, 12 * MIN);
        beat_all(&mut s, 12 * MIN);
        assert!(grants(&s.tick(12 * MIN)).is_empty(), "an ended lease's charge counts until its cleanup: {:?}", state(&s, &small));
        s.cleaned("pool-a", &soon, 0);
        beat_all(&mut s, 12 * MIN + 1_000);
        assert_eq!(grants(&s.tick(12 * MIN + 1_000)).first(), Some(&owner), "{:?}", state(&s, &owner));
        assert_eq!(state(&s, &first).0, "queued", "4 + 9 + 4 of 16");
        invariants(&s);
    }

    /// A box holder waits for its box as measured, as every other ask does: one definition of what
    /// a box's work takes holds it and starts it. Before, the holder started by charges alone: it
    /// held a box by one definition and started by another (a landing gate held its box at "node
    /// busy" behind a prebuild charged a few cores that ran on 7.5, 11-07: that prebuild is now
    /// charged its 7.5). It cannot starve: a box with no lease left counts no outside work.
    #[test]
    fn a_box_holder_waits_for_its_box_as_measured_and_starts_once_it_empties() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.est_min = Some(min);
            r
        };
        let blocker = s.request(ask("run", 8.0, 10.0), 0);
        s.tick(0);
        let mut gate = ask("landq-fast", 12.0, 5.0);
        gate.kind = Kind::Gate;
        let gate = s.request(gate, 1);
        s.tick(1);
        assert_eq!(s.holds["a"].id, gate);
        let beside = s.request(ask("beside", 4.0, 60.0), 2);
        s.tick(2);
        assert_eq!(node_of(&s, &beside).as_deref(), Some("a"), "{:?}", state(&s, &beside));
        release_clean(&mut s, &blocker, 3);
        let mut busy = caps(16, 64.0, Role::Pool);
        busy.cpu_pct = Some(99.0);
        s.set_caps("a", busy, 4);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(state(&s, &gate).0, "queued", "its hold exempts no outside work: {:?}", state(&s, &gate));
        let medium = s.request(ask("other", 1.0, 30.0), 5);
        beat_all(&mut s, 5);
        s.tick(5);
        assert_eq!(state(&s, &medium).0, "queued", "a lease that holds nothing still waits: {:?}", state(&s, &medium));
        release_clean(&mut s, &beside, 6);
        beat_all(&mut s, 6);
        s.tick(6);
        assert_eq!(node_of(&s, &gate).as_deref(), Some("a"), "its box empties and it starts there: {:?}", state(&s, &gate));
        assert_eq!(state(&s, &medium).0, "queued", "{:?}", state(&s, &medium));
        invariants(&s);
    }

    /// A landing gate outranks a tests shard's hold: it waits behind none, holds the shard's box in
    /// the shard's place, and takes the cores the box frees first. A gate waited 14 min at the
    /// queue's head while a shard held its box and smaller lane asks took every freed core (11-07).
    #[test]
    fn a_landing_gate_outranks_a_tests_shards_hold() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.est_min = Some(min);
            r
        };
        let running = s.request(ask("run", 8.0, 10.0), 0);
        s.tick(0);
        let mut shard = ask(TESTS_LANE, 12.0, 60.0);
        shard.priority = Priority::Background;
        let shard = s.request(shard, 1);
        s.tick(1);
        let lane = s.request(ask("lane", 7.0, 60.0), 2);
        s.tick(2);
        assert!(state(&s, &lane).1.contains(&format!("held for tests shard {shard}")), "{:?}", state(&s, &lane));
        let mut gate = ask("landq-fast", 10.0, 4.0);
        gate.kind = Kind::Gate;
        gate.priority = Priority::Background;
        let gate = s.request(gate, 3);
        s.tick(3);
        assert!(state(&s, &lane).1.contains(&format!("held for landing gate {gate}")), "the gate holds the box in the shard's place: {:?}", state(&s, &lane));
        assert!(!state(&s, &gate).1.contains("held for tests shard"), "{:?}", state(&s, &gate));
        release_clean(&mut s, &running, 4);
        beat_all(&mut s, 5);
        s.tick(5);
        assert_eq!(node_of(&s, &gate).as_deref(), Some("a"), "the gate takes the freed cores first: {:?}", state(&s, &gate));
        assert_eq!(state(&s, &shard).0, "queued", "{:?}", state(&s, &shard));
        release_clean(&mut s, &gate, 6);
        beat_all(&mut s, 7);
        s.tick(7);
        assert_eq!(node_of(&s, &shard).as_deref(), Some("a"), "then the shard: {:?}", state(&s, &shard));
    }

    /// A tests bucket's shard holds its box for as long as it waits, as a landing gate does,
    /// though it asks at background priority: lane work that would not fit beside it and outlasts
    /// what runs there waits; work its class measured to end first goes. Its four shards headed
    /// the queue 18 min while newer lane work took the cores each box freed (11-07).
    #[test]
    fn a_tests_shard_keeps_its_box_from_newer_lane_work() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.est_min = Some(min);
            r
        };
        let running = s.request(ask("run", 8.0, 10.0), 0);
        s.tick(0);
        let mut shard = ask(TESTS_LANE, 12.0, 10.0);
        shard.priority = Priority::Background;
        let shard = s.request(shard, 1);
        s.tick(1);
        for _ in 0..3 { s.record_node_cost("quick", "test", "a", 60_000); }
        let lane = s.request(ask("lane", 6.0, 60.0), 2);
        let mut quick = ask("quick", 6.0, 1.0);
        quick.class = "quick".into();
        let quick = s.request(quick, 2);
        s.tick(2);
        assert!(state(&s, &lane).1.contains(&format!("held for tests shard {shard}")), "{:?}", state(&s, &lane));
        assert_eq!(node_of(&s, &quick).as_deref(), Some("a"), "it ends before the shard could start: {:?}", state(&s, &quick));
        beat_all(&mut s, HOLD_MS + 3);
        s.tick(HOLD_MS + 3);
        // (past ten minutes the shard asks only what its box has free, a third of its ask at
        // least: the lane, beside what came in since, waits on the box's cores)
        assert_eq!(state(&s, &lane).0, "queued", "past ten minutes it still waits: {:?}", state(&s, &lane));
        release_clean(&mut s, &running, HOLD_MS + 4);
        release_clean(&mut s, &quick, HOLD_MS + 4);
        beat_all(&mut s, HOLD_MS + 5);
        s.tick(HOLD_MS + 5);
        assert_eq!(node_of(&s, &shard).as_deref(), Some("a"), "{:?}", state(&s, &shard));
    }

    /// Each queued shard of a tests run holds a box of its own while it waits: newer lane work
    /// that neither fits beside it nor ends first waits on every box, not only on the one the
    /// first shard holds. Only one shard held (its hold kept its siblings from holding any box),
    /// and lane leases granted on the other boxes kept three shards queued 21 min (11-07).
    #[test]
    fn every_queued_shard_of_a_tests_run_holds_a_box_of_its_own() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, node: Option<&str>, cores: f64, min: f64| {
            let mut r = overlay(lane, node, cores);
            r.est_min = Some(min);
            r
        };
        let running: Vec<String> = ["a", "b", "c"].iter().map(|n| s.request(ask(&format!("run{n}"), Some(n), 8.0, 10.0), 0)).collect();
        s.tick(0);
        let shards: Vec<String> = (0..3).map(|_| {
            let mut r = ask(TESTS_LANE, None, 12.0, 20.0);
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r.priority = Priority::Background;
            s.request(r, 1)
        }).collect();
        s.tick(1);
        let held: std::collections::BTreeSet<&str> = s.holds.iter().filter(|(_, h)| shards.contains(&h.id)).map(|(n, _)| n.as_str()).collect();
        assert_eq!(held.len(), 3, "a box each: {:?}", s.holds);
        let lane = s.request(ask("lane", None, 6.0, 60.0), 2);
        s.tick(2);
        assert_eq!(state(&s, &lane).0, "queued", "held on every box: {:?}", state(&s, &lane));
        assert!(state(&s, &lane).1.contains("held for tests shard"), "{:?}", state(&s, &lane));
        // what its class measured to end before a shard could start still goes
        for _ in 0..3 { s.record_node_cost("quick", "test", "a", 60_000); }
        let mut quick = ask("quick", None, 6.0, 1.0);
        quick.class = "quick".into();
        let quick = s.request(quick, 3);
        s.tick(3);
        assert!(node_of(&s, &quick).is_some(), "{:?}", state(&s, &quick));
        for r in &running { release_clean(&mut s, r, 4); }
        release_clean(&mut s, &quick, 4);
        beat_all(&mut s, 5);
        s.tick(5);
        let boxes: std::collections::BTreeSet<Option<String>> = shards.iter().map(|id| node_of(&s, id)).collect();
        assert_eq!(boxes.len(), 3, "each on its box: {boxes:?}");
        assert!(boxes.iter().all(Option::is_some), "{:?}", shards.iter().map(|id| state(&s, id)).collect::<Vec<_>>());
        assert_eq!(state(&s, &lane).0, "queued", "{:?}", state(&s, &lane));
    }

    /// A landing gate holds a box its leases measure busy, and lane work does not take the cores it
    /// frees: every box the gate could take measured over 95% busy (three tests shards, lane work),
    /// it held none, and lane asks took each core freed while it headed its queue 18 min (11-07).
    /// Holding it, it starts once charges and outside work fit. A
    /// box's hub build starts beside a shard charged less than the box's cores, busy as the box
    /// measures: one waited 20 min there, its hub a shard behind the master (11-07).
    #[test]
    fn a_gate_holds_a_busy_box_and_a_hub_build_goes_beside_a_shard() {
        let nodes = [("a", 64.0), ("b", 30.0), ("c", 64.0)];
        let mut s = sched(&nodes.map(|(n, ram)| (n, caps(16, ram, Role::Pool))));
        let ask = |lane: &str, node: Option<&str>, cores: f64, min: f64| {
            let mut r = overlay(lane, node, cores);
            r.est_min = Some(min);
            r
        };
        for n in ["a", "c"] {
            let mut r = ask(TESTS_LANE, Some(n), 12.0, 41.0);
            r.priority = Priority::Background;
            s.request(r, 0);
        }
        let short = s.request(ask("lanex", Some("b"), 6.0, 3.0), 0);
        s.request(ask("laney", Some("b"), 6.0, 5.0), 0);
        assert_eq!(grants(&s.tick(0)).len(), 4);
        let busy = |s: &mut Sched, pct: f64, at: u64| for (n, ram) in nodes {
            let mut c = caps(16, ram, Role::Pool);
            c.cpu_pct = Some(if n == "b" { pct } else { 100.0 });
            s.set_caps(n, c, at);
        };
        busy(&mut s, 100.0, 1);
        let mut hub = req("prebuild-a", Kind::Build, Some("a"));
        hub.lane = "hub-build-a".into();
        hub.class = HUB_BUILD.into();
        hub.master = true;
        let hub = s.request(hub, 1);
        let mut gate = ask("landq-fast", None, 8.0, 4.0);
        gate.kind = Kind::Gate;
        gate.priority = Priority::Background;
        let gate = s.request(gate, 1);
        s.tick(1);
        assert_eq!(node_of(&s, &hub).as_deref(), Some("a"), "the hub build starts beside the shard: {:?}", state(&s, &hub));
        assert_eq!(s.holds.get("b").map(|h| h.id.clone()), Some(gate.clone()), "the gate holds the box that frees first: {:?} {:?}", s.holds, state(&s, &gate));
        invariants(&s);
        let lane = s.request(ask("lanez", None, 10.0, 10.0), 2);
        s.tick(2);
        release_clean(&mut s, &short, 3);
        beat_all(&mut s, 3);
        s.tick(3);
        // the box's last look still counts the ended lease: its holder waits for the next one
        assert_eq!(state(&s, &gate).0, "queued", "{:?}", state(&s, &gate));
        assert_eq!(state(&s, &lane).0, "queued", "the freed cores were the gate's: {:?}", state(&s, &lane));
        invariants(&s);
        // which measures the lane work left there: the gate starts beside it, and the freed
        // cores were the gate's, not the newer lane ask's
        busy(&mut s, 37.5, 4);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(node_of(&s, &gate).as_deref(), Some("b"), "{:?}", state(&s, &gate));
        assert_eq!(state(&s, &lane).0, "queued", "{:?}", state(&s, &lane));
        invariants(&s);
    }

    /// A tests shard goes before newer lane work when a box frees, whichever box it held: a lane's
    /// newer ask went first by its lane's last grant (the shard's sibling's) and took the box that
    /// freed before the one the shard held (11-07).
    #[test]
    fn a_tests_shard_takes_a_freed_box_before_newer_lane_work() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("d", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, node: Option<&str>, cores: f64, min: f64| {
            let mut r = overlay(lane, node, cores);
            r.est_min = Some(min);
            r
        };
        let early = s.request(ask("runa", Some("a"), 12.0, 20.0), 0);
        s.request(ask("runb", Some("b"), 12.0, 10.0), 0);
        s.tick(0);
        let shard = |s: &mut Sched, t: u64| {
            let mut r = ask(TESTS_LANE, None, 12.0, 20.0);
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r.priority = Priority::Background;
            s.request(r, t)
        };
        let first = shard(&mut s, 1);
        let second = shard(&mut s, 1);
        s.tick(1);
        assert!(node_of(&s, &first).is_some() || node_of(&s, &second).is_some(), "one shard takes the empty box");
        let waiting = if node_of(&s, &first).is_some() { second } else { first };
        assert_eq!(s.holds.get("b").map(|h| h.id.clone()), Some(waiting.clone()), "it holds the box that ends first: {:?}", s.holds);
        let lane = s.request(ask("lane", None, 6.0, 60.0), 2);
        s.tick(2);
        assert_eq!(state(&s, &lane).0, "queued", "{:?}", state(&s, &lane));
        // the box it did not hold frees first
        release_clean(&mut s, &early, 3);
        beat_all(&mut s, 4);
        s.tick(4);
        assert_eq!(node_of(&s, &waiting).as_deref(), Some("a"), "the shard takes it: {:?} / lane {:?}", state(&s, &waiting), state(&s, &lane));
        assert_eq!(state(&s, &lane).0, "queued", "{:?}", state(&s, &lane));
    }

    /// A shard keeps a box while a landing gate holds another box against it: a hold elsewhere took
    /// its reservation everywhere, and lane work took the box it waited for (11-07). Lane work held
    /// off both boxes holds nothing.
    #[test]
    fn a_tests_shard_holds_its_box_while_a_gate_holds_another() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, cores: f64, min: f64| {
            let mut r = overlay(lane, None, cores);
            r.est_min = Some(min);
            r
        };
        let running: Vec<String> = ["a", "b"].iter().map(|n| {
            let mut r = ask(&format!("run{n}"), 8.0, 10.0);
            r.node = Some(n.to_string());
            s.request(r, 0)
        }).collect();
        s.tick(0);
        let mut gate = ask("landq-fast", 12.0, 10.0);
        gate.kind = Kind::Gate;
        let gate = s.request(gate, 1);
        s.tick(1);
        let gate_box = s.holds.iter().find(|(_, h)| h.id == gate).map(|(n, _)| n.clone()).expect("the gate holds a box");
        let mut shard = ask(TESTS_LANE, 12.0, 20.0);
        shard.priority = Priority::Background;
        let shard = s.request(shard, 2);
        s.tick(2);
        let shard_box = s.holds.iter().find(|(_, h)| h.id == shard).map(|(n, _)| n.clone());
        assert!(shard_box.is_some() && shard_box != Some(gate_box.clone()), "the shard holds the other box: {:?}", s.holds);
        let lane = s.request(ask("lane", 6.0, 60.0), 3);
        s.tick(3);
        assert_eq!(state(&s, &lane).0, "queued", "{:?}", state(&s, &lane));
        assert!(s.holds.values().all(|h| h.id != lane), "it holds nothing: {:?}", s.holds);
        for r in &running { release_clean(&mut s, r, 4); }
        beat_all(&mut s, 5);
        s.tick(5);
        assert_eq!(node_of(&s, &gate), Some(gate_box), "{:?}", state(&s, &gate));
        assert_eq!(node_of(&s, &shard), shard_box, "{:?}", state(&s, &shard));
        assert_eq!(state(&s, &lane).0, "queued", "{:?}", state(&s, &lane));
    }

    /// The shards of one tests bucket run are one unit against their lane's cap, and share their
    /// tree at its pin on a hub that takes them so: a run's four shards ran two at a time, the
    /// others 18 min at "lane cap 2/2", and before that one waited for its sibling's box at "one
    /// lease at a time in its tree" (11-07).
    #[test]
    fn a_tests_runs_shards_neither_cap_nor_block_each_other() {
        let shard = |pin: char, node: Option<&str>| {
            let mut r = req(TESTS_LANE, Kind::Test, node);
            r.rev = Some(format!("work@{}", pin.to_string().repeat(40)));
            r.cores = Some(8.0);
            r.priority = Priority::Background;
            r
        };
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool)), ("d", caps(16, 64.0, Role::Pool))]);
        let four: Vec<String> = (0..4).map(|_| s.request(shard('a', None), 0)).collect();
        let mut granted = grants(&s.tick(0));
        granted.sort();
        assert_eq!(granted, four, "{:?}", four.iter().map(|id| state(&s, id)).collect::<Vec<_>>());
        let boxes: std::collections::BTreeSet<Option<String>> = four.iter().map(|id| node_of(&s, id)).collect();
        assert_eq!(boxes.len(), 4, "a box each while each has room: {boxes:?}");
        // the run's four running shards are one unit: a fifth (a box come up) is within the cap;
        // another lane's third lease beside its two running is past it, as before
        let fifth = s.request(shard('a', None), 1);
        assert!(s.lane_block(s.get(&fifth).unwrap()).is_none());
        for _ in 0..2 { s.request(overlay("lanex", None, 2.0), 1); }
        s.tick(1);
        let third = s.request(overlay("lanex", None, 2.0), 2);
        assert_eq!(s.lane_block(s.get(&third).unwrap()).map(|b| b.reason).as_deref(), Some("lane cap 2/2"));

        // one box: the run's second shard joins its sibling's tree; another run's waits for it
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let one = s.request(shard('a', Some("a")), 0);
        let two = s.request(shard('a', Some("a")), 0);
        assert_eq!(grants(&s.tick(0)), vec![one.clone(), two.clone()]);
        release_clean(&mut s, &two, 1);
        let other = s.request(shard('b', Some("a")), 2);
        s.tick(2);
        assert!(state(&s, &other).1.contains(&format!("held by {one} (test of {TESTS_LANE}): one lease at a time in its tree")), "{:?}", state(&s, &other));
        // a hub before the run's queue (`lease-tell`) takes no shard: it waits for its update
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("box-b", caps(16, 64.0, Role::Pool))]);
        s.nodes.get_mut("a").unwrap().caps.as_mut().unwrap().features.retain(|f| f != "tell");
        let one = s.request(shard('a', Some("a")), 0);
        assert!(grants(&s.tick(0)).is_empty());
        assert!(state(&s, &one).1.contains("its hub lacks tell for this request (boxes that have it: box-b): waits for its update"), "{:?}", state(&s, &one));
        assert!(state(&s, &one).1.contains("boxes that have it: box-b"), "{:?}", state(&s, &one));
    }

    /// Allapps asks at its run's start, having asked nothing while lane work waited: what it
    /// waited is its time in the queue, not its run's age. Its ask 16 min into its run read
    /// "waiting 15m53s: 1st in queue" the moment it asked, and the next "waiting 17m51s" 29 s
    /// before its grant (L644512, L644515, 11-08). Ten minutes in the queue it warns, its run's
    /// whole wait beside it.
    #[test]
    fn a_background_legs_wait_is_its_own_time_in_the_queue() {
        let warned = |a: &[Action], id: &str| a.iter().filter_map(|a| match a { Action::Event(r) if r.sev == Sev::Warn && r.line.starts_with(&format!("{id} ")) && r.line.contains(" waiting ") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = req("busy", Kind::Test, Some("a"));
        r.whole = true;
        let busy = s.request(r, 0);
        s.tick(0);
        let asked = 16 * 60_000;
        let mut leg = req("allapps", Kind::Long, Some("a"));
        leg.agent = Some("devhub-allapps".into());
        leg.prio = 9;
        leg.priority = Priority::Background;
        leg.since_ms = Some(0);
        let leg = s.request(leg, asked);
        beat_all(&mut s, asked);
        assert_eq!(warned(&s.tick(asked), &leg), Vec::<String>::new(), "asked just now");
        assert!(s.beat(&leg, asked + 26_000).unwrap().contains("\"wait_s\":26,"));
        let late = asked + WAIT_WARN_MS;
        beat_all(&mut s, late);
        let said = warned(&s.tick(late), &leg);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains(&format!("waiting 10m00s (its run waited 26m00s in all, standing back while lane work waited): 1st in queue (held by {busy}")), "{said:?}");
        release_clean(&mut s, &busy, late + 1);
        beat_all(&mut s, late + 1);
        let a = s.tick(late + 1);
        assert_eq!(grants(&a), vec![leg.clone()]);
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.line.starts_with(&format!("{leg} ")) && r.line.contains("granted to allapps after 10m00s"))), "{a:?}");
    }

    /// An owed leg asks at its run's age (its place), after the run's earlier legs ran: what it
    /// waited is its own time in the queue. Asked just now it warns of no hour; ten minutes in it
    /// warns, its run's age beside it; its grant says the same. A lane's ask counts from its place.
    #[test]
    fn an_owed_legs_wait_is_its_own_time_in_the_queue() {
        let warned = |a: &[Action], id: &str| a.iter().filter_map(|a| match a { Action::Event(r) if r.sev == Sev::Warn && r.line.starts_with(&format!("{id} ")) && r.line.contains(" waiting ") => Some(r.line.clone()), _ => None }).collect::<Vec<_>>();
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = req("busy", Kind::Test, Some("a"));
        r.whole = true;
        let busy = s.request(r, 0);
        s.tick(0);
        // the run became owed at 0; its earlier legs ran until this one asks, 55 min later
        let asked = 55 * 60_000;
        let mut leg = req("jitlegs-arm64", Kind::Test, Some("a"));
        leg.agent = Some("devhub-jitlegs".into());
        leg.prio = DEFAULT_PRIO - 1;
        leg.since_ms = Some(0);
        let leg = s.request(leg, asked);
        let mut lane = req("lane", Kind::Test, Some("a"));
        lane.since_ms = Some(asked - WAIT_WARN_MS);
        let lane = s.request(lane, asked);
        beat_all(&mut s, asked);
        let a = s.tick(asked);
        assert_eq!(warned(&a, &leg), Vec::<String>::new(), "asked just now");
        assert_eq!(warned(&a, &lane).len(), 1, "a lane's re-ask waited from its first ask: {a:?}");
        assert!(s.beat(&leg, asked + 1_000).unwrap().contains("\"wait_s\":1,"));
        let late = asked + WAIT_WARN_MS;
        beat_all(&mut s, late);
        let said = warned(&s.tick(late), &leg);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains(&format!("waiting 10m00s (its run owed a box for 1h05m): 1st in queue (held by {busy}")), "{said:?}");
        release_clean(&mut s, &busy, late + 1);
        beat_all(&mut s, late + 1);
        let a = s.tick(late + 1);
        assert_eq!(grants(&a), vec![leg.clone()]);
        assert!(a.iter().any(|a| matches!(a, Action::Event(r) if r.line.starts_with(&format!("{leg} ")) && r.line.contains("granted to jitlegs-arm64 after 10m00s"))), "{a:?}");
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

    /// A lane's warm leg waiting at its named box reserves none: work that fits there now still
    /// starts there, and the leg starts once the box can start it. Another lane's ask of the same
    /// size waiting there holds the box (it drains for it). Three lanes' warm legs held a box
    /// behind its tests shard in turn while two others idled (11-09).
    #[test]
    fn a_lanes_warm_leg_reserves_no_box() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        // (overlays: a box's shared work tree takes one tree lease at a time)
        let ask = |s: &mut Sched, lane: &str, kind: Kind, cores: f64, now: u64| {
            let mut r = req(lane, kind, Some("a"));
            (r.src, r.rev, r.cores) = (Src::Overlay, Some(format!("work@{}", "a".repeat(40))), Some(cores));
            s.request(r, now)
        };
        let shard = ask(&mut s, "busy", Kind::Test, 13.0, 0);
        assert_eq!(grants(&s.tick(0)), vec![shard.clone()]);
        s.started(&shard, "{}");
        let warm = ask(&mut s, &format!("{}lane", super::super::WARM_LANE_PREFIX), Kind::Build, 4.0, 1_000);
        assert!(grants(&s.tick(1_000)).is_empty());
        assert!(!s.holds.contains_key("a"), "a warm leg holds no box: {:?}", s.holds.keys().collect::<Vec<_>>());
        // lane work that fits now starts beside it
        let small = ask(&mut s, "small", Kind::Test, 2.0, 2_000);
        assert_eq!(grants(&s.tick(2_000)), vec![small.clone()], "{:?}", state(&s, &small));
        // the same ask of a lane holds the box
        let lane = ask(&mut s, "other", Kind::Build, 4.0, 3_000);
        assert!(grants(&s.tick(3_000)).is_empty());
        assert_eq!(s.holds.get("a").map(|h| h.id.clone()), Some(lane.clone()), "{:?}", state(&s, &lane));
        // the box frees: the held ask first, then the warm leg where it fits
        release_clean(&mut s, &shard, 4_000);
        let granted = grants(&s.tick(4_000));
        assert!(granted.contains(&lane) && granted.contains(&warm), "{granted:?}");
    }

    #[test]
    fn a_running_lease_is_charged_at_least_its_measured_cores() {
        let mut measured = caps(16, 64.0, Role::Pool);
        let mut s = sched(&[("box-a", measured.clone())]);
        let first = s.request(overlay("one", Some("box-a"), 4.0), 0);
        assert_eq!(grants(&s.tick(0)), vec![first.clone()]);
        s.started(&first, "{}");
        assert_eq!(s.get(&first).unwrap().cost.cores, 4.0);
        measured.cpu_pct = Some(78.1);
        measured.busy_leases = vec![(first.clone(), 12.5)];
        s.set_caps("box-a", measured.clone(), 1);
        assert_eq!(s.charged_on("box-a").cores, 12.5, "the latest use raises the charge above its grant");
        let row = s.lease_view_line(s.get(&first).unwrap(), 1);
        assert!(row.contains("cores 12.5 (granted 4, measured 12.5)"), "{row}");
        let second = s.request(overlay("two", Some("box-a"), 8.0), 1);
        assert!(grants(&s.tick(1)).is_empty());
        assert_eq!(state(&s, &second), ("queued".to_string(), "budget cores 16, in use 12, asked 8".to_string()), "12.5 rounds to 12 in the existing budget wording");
        measured.cpu_pct = Some(18.75);
        measured.busy_leases = vec![(first.clone(), 3.0)];
        s.set_caps("box-a", measured.clone(), 2);
        assert_eq!(s.charged_on("box-a").cores, 4.0, "use below the grant restores the grant's charge");
        assert_eq!(grants(&s.tick(2)), vec![second]);
        assert_eq!(s.charged_on("box-a").cores, 12.0);
        assert!(!s.lease_view_line(s.get(&first).unwrap(), 2).contains("(granted"));

        let mut live = overlay("live", Some("box-a"), 4.0);
        live.live = true;
        let live = s.request(live, 3);
        assert_eq!(grants(&s.tick(3)), vec![live.clone()]);
        s.started(&live, "{}");
        measured.busy_leases.push((live.clone(), 12.5));
        s.set_caps("box-a", measured.clone(), 4);
        assert_eq!(s.charged(s.get(&live).unwrap()).cores, 4.0, "a live launch keeps its own charge");
        s.live_call_charged(&live, 2.0);
        assert_eq!(s.charged(s.get(&live).unwrap()).cores, 2.0, "a live call keeps its own charge");
        s.live_idle.insert(live.clone());
        assert_eq!(s.charged(s.get(&live).unwrap()).cores, 12.5, "an idle live session is charged what it measures (its JIT still compiling)");
        measured.busy_leases.retain(|(id, _)| *id != live);
        measured.busy_leases.push((live.clone(), 0.1));
        s.set_caps("box-a", measured, 5);
        assert_eq!(s.charged(s.get(&live).unwrap()).cores, 0.1, "settled, it is charged next to nothing");
        assert!(!s.lease_view_line(s.get(&live).unwrap(), 4).contains("(granted"));
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
        assert_eq!(state(&s, &second), ("queued".to_string(), "budget cores 16, in use 4 (outside its leases 10), asked 4".to_string()));
        // what took the rest is named: a lease past its grant, work of no lease's by its pid
        busy.busy_leases = vec![(first.clone(), 5.5)];
        busy.busy_outside = vec![("indexer --all".into(), 4_000_021, 7.5), ("dumper".into(), 4_000_020, 1.0)];
        s.set_caps("a", busy.clone(), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert_eq!(state(&s, &second).1, format!("budget cores 16, in use 6 (outside its leases 9; busy: {first} 5.5 cores (granted 4), indexer --all pid 4000021 (7.5 cores), dumper pid 4000020 (1.0 cores)), asked 4"));
        // a lease within a core of its grant is not named
        busy.busy_leases = vec![(first.clone(), 4.5)];
        busy.busy_outside.clear();
        s.set_caps("a", busy.clone(), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert_eq!(state(&s, &second).1, "budget cores 16, in use 4 (outside its leases 10), asked 4");
        // the box's own hub build under its hub-build lease is that lease's (the box names it by
        // its class), not work of no lease's
        let mut hub = req(&format!("{HUB_BUILD}-a"), Kind::Build, Some("a"));
        (hub.class, hub.master) = (HUB_BUILD.into(), true);
        let hub = s.request(hub, 2);
        assert_eq!(grants(&s.tick(2)), vec![hub.clone()], "{:?}", state(&s, &hub));
        busy.busy_leases = vec![(first.clone(), 4.5), (HUB_BUILD.to_string(), 6.0)];
        s.set_caps("a", busy.clone(), 2);
        assert!(grants(&s.tick(2)).is_empty());
        let why = state(&s, &second).1;
        assert!(why.contains(&format!("busy: {hub} (its hub's build) 6.0 cores (granted ")) && !why.contains(&first), "{why}");
        release_clean(&mut s, &hub, 2);
        // so is its deployment's prebuild under that deployment's room: a lease of the hub
        // build's class (room only, nothing set up on the box), named by its own
        let mut deploy = req(&format!("{DEPLOY_BUILD}-a"), Kind::Build, Some("a"));
        (deploy.class, deploy.master) = (HUB_BUILD.into(), true);
        assert!(deploy.hub_build() && deploy.deploy_build() && !deploy.set_up_on_node() && deploy.hub_work() == Some(DEPLOY_BUILD));
        let deploy = s.request(deploy, 2);
        assert_eq!(grants(&s.tick(2)), vec![deploy.clone()], "{:?}", state(&s, &deploy));
        busy.busy_leases = vec![(first.clone(), 4.5), (DEPLOY_BUILD.to_string(), 6.0)];
        s.set_caps("a", busy.clone(), 2);
        assert!(grants(&s.tick(2)).is_empty());
        let why = state(&s, &second).1;
        assert!(why.contains(&format!("busy: {deploy} (its deployment's build) 6.0 cores (granted ")) && !why.contains(&first), "{why}");
        release_clean(&mut s, &deploy, 2);
        busy.cpu_pct = Some(50.0);
        s.set_caps("a", busy, 3);
        assert_eq!(grants(&s.tick(3)), vec![second], "8 measured and 4 asked fit");
    }

    /// Outside work is counted once and a hold gives no CPU exemption. Here 8 outside plus
    /// the ask's 8 fits 16 once the 4-core lease ends; that end is also the last end on the box.
    #[test]
    fn outside_work_counts_the_same_for_a_holder_and_its_remaining_time() {
        let mut c = caps(16, 64.0, Role::Pool);
        let mut s = sched(&[("box-a", c.clone())]);
        let mut running = overlay("running", Some("box-a"), 4.0);
        running.est_min = Some(10.0);
        let running = s.request(running, 0);
        assert_eq!(grants(&s.tick(0)), vec![running.clone()]);
        c.cpu_pct = Some(75.0);
        c.busy_leases = vec![(running.clone(), 4.0)];
        s.set_caps("box-a", c, 1);
        let mut ask = overlay("landq-fast", Some("box-a"), 8.0);
        ask.kind = Kind::Gate;
        let ask = s.request(ask, 1);
        let l = s.get(&ask).unwrap();
        let cost = s.cost_on("box-a", l);
        let reason = "budget cores 16, in use 4 (outside its leases 8), asked 8";
        assert_eq!(s.outside_cores("box-a"), 8.0);
        assert_eq!(s.resources("box-a", l, &cost, true).unwrap_err().reason, reason);
        s.held_before.insert("box-a".into(), Hold { id: ask.clone(), until: s.hold_end() });
        let l = s.get(&ask).unwrap();
        assert_eq!(s.resources("box-a", l, &cost, true).unwrap_err().reason, reason, "the holder waits the same way");
        let mut after = s.in_use("box-a", l, true, &[running.clone()]);
        assert_eq!(after.outside, 0.0, "an empty box counts no outside work");
        after.outside = 8.0;
        assert!(s.fits("box-a", l, &after, &cost, s.room("box-a", l, &cost, true, &[running.clone()], true)).is_ok(), "8 outside + 8 asked fits 16 even before the empty-box exemption");
        let end = s.expected_end(s.get(&running).unwrap()).unwrap();
        assert_eq!(s.remaining_after("box-a", l, &cost, true), end, "the lease's end fits and is also last");
        assert!(s.starts_after_ends("box-a", l));
        assert!(grants(&s.tick(1)).is_empty());
        assert_eq!(state(&s, &ask), ("queued".into(), reason.into()));
    }

    /// A box whose lease.conf pins shared kinds to some of its CPUs budgets them on those: a box
    /// of 32 pinned 16 to shared work was charged 32 cores of it, load 66 (11-06). One asking
    /// more than them can never run there; timed work has its own CPUs.
    #[test]
    fn shared_work_is_budgeted_on_the_cpus_it_is_pinned_to() {
        let mut pinned = caps(32, 64.0, Role::Pool);
        (pinned.cpus_shared, pinned.cpus_timed) = (16, 8);
        let mut s = sched(&[("a", pinned)]);
        let ask = |lane: &str, cores: f64| {
            let mut r = req(lane, Kind::Test, None);
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            r.cores = Some(cores);
            r
        };
        let (one, two) = (s.request(ask("one", 8.0), 0), s.request(ask("two", 8.0), 0));
        let three = s.request(ask("three", 4.0), 0);
        let granted = grants(&s.tick(0));
        assert!(granted.contains(&one) && granted.contains(&two) && !granted.contains(&three), "{granted:?}");
        assert_eq!(state(&s, &three), ("queued".to_string(), "budget cores 16 (of 32, pinned to its cpus.shared), in use 16, asked 4".to_string()));
        let big = s.request(ask("big", 20.0), 1);
        s.tick(1);
        assert!(state(&s, &big).1.contains("needs 20 cores, has 16 (of 32, pinned to its cpus.shared)"), "{:?}", state(&s, &big));
        let c = s.nodes["a"].caps.clone().unwrap();
        assert_eq!((c.cores_for(Kind::Build), c.cores_for(Kind::Timed)), (16, 8));
    }

    /// On a box that weighs no CPU a live session's call is admitted like a lease, never past its
    /// box's cores, and the box keeps room for one call of its largest idle session. Work granted
    /// beside idle sessions filled their box and their calls were taken anyway: a box was over its
    /// cores 30% of an hour (11-05). A second call at once waits for the first. One on a box
    /// measured busy past its charges runs (calls waited 96-125 s so, 11-06).
    /// A call fits beside idle sessions however many there are: their cores are a share of the box,
    /// not work it waits for. Work that runs still counts, and so does another session's call:
    /// an interactive call whose full charge does not fit starts at once with the cores free
    /// there, charged those until its call ends (a save waited 166 s and 672 s for its call's
    /// 15 cores beside 6 charged, W4666, 11-10); a background one waits.
    #[test]
    fn a_live_call_does_not_wait_for_idle_sessions_cores() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let overlay = |lane: &str, cores: f64| {
            let mut r = req(lane, Kind::Test, None);
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            (r.live, r.cores, r.priority) = (true, Some(cores), Priority::Interactive);
            r
        };
        let large = s.request(overlay("live-large", 15.0), 0);
        assert_eq!(grants(&s.tick(0)), vec![large.clone()]);
        s.live_idle.insert(large.clone());
        let others: Vec<String> = (0..3).map(|i| s.request(overlay(&format!("live-W{i}"), 2.0), 1)).collect();
        assert_eq!(grants(&s.tick(1)), others);
        s.live_idle.extend(others.iter().cloned());
        assert_eq!(s.live_room(&large), Ok(()), "three idle sessions' cores are no reason to wait");
        assert_eq!(s.call_start_cores(&large), None, "its full charge fits");
        s.live_idle.remove(&others[0]);
        assert_eq!(s.live_room(&large), Ok(()), "an interactive call does not wait beside another's");
        assert_eq!(s.call_start_cores(&large), Some((14.0, 15.0, 2.0)), "it starts with the cores free");
        s.live_call_charged(&large, 14.0);
        s.live_idle.remove(&large);
        assert_eq!(s.charged(s.get(&large).unwrap()).cores, 14.0, "charged what was free while its call runs");
        s.live_idle.insert(large.clone());
        s.live_call_ended(&large);
        s.live_idle.remove(&large);
        assert_eq!(s.charged(s.get(&large).unwrap()).cores, 15.0, "its next call is charged afresh");
        s.live_idle.insert(large.clone());
        // a background session's call waits for its full charge
        s.leases.iter_mut().find(|l| l.id == large).unwrap().priority = Priority::Background;
        let why = s.live_room(&large).unwrap_err();
        assert!(why.contains("(2 of 16 in use)") && why.contains("(live, in a call, 2)"), "{why}");
        assert_eq!(s.call_start_cores(&large), None);
        invariants(&s);
    }

    #[test]
    fn a_live_call_on_a_box_that_weighs_no_cpu_never_runs_past_it_and_the_box_keeps_it_room() {
        let mut busy = caps(16, 64.0, Role::Pool);
        busy.os = "macos".into();
        let mut s = sched(&[("a", busy.clone())]);
        let overlay = |lane: &str, live: bool, cores: f64| {
            let mut r = req(lane, Kind::Test, None);
            r.src = Src::Overlay;
            r.rev = Some(format!("work@{}", "a".repeat(40)));
            (r.live, r.cores, r.needs) = (live, Some(cores), vec![("os".into(), "macos".into())]);
            r
        };
        let sessions: Vec<String> = (0..2).map(|i| s.request(overlay(&format!("live-W{i}"), true, 6.0), 0)).collect();
        assert_eq!(grants(&s.tick(0)), sessions);
        s.live_idle.extend(sessions.iter().cloned());
        // two idle sessions charged what they measure (nothing), and 6 kept for a call: 8 fit,
        // then 4 more do not
        let work = s.request(overlay("work", false, 8.0), 1);
        assert_eq!(grants(&s.tick(1)), vec![work.clone()]);
        let more = s.request(overlay("more", false, 4.0), 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert_eq!(state(&s, &more), ("queued".to_string(), "budget cores 16, in use 8, kept for a live call 6, asked 4".to_string()));
        // a call takes the room kept for it; a second at once waits for the first
        assert_eq!(s.live_room(&sessions[0]), Ok(()));
        s.live_idle.remove(&sessions[0]);
        assert_eq!(s.live_room(&sessions[1]), Err(format!("a: waiting for 6 cores (14 of 16 in use), held by {} (test, 8), {} (live, in a call, 6): beside the call of {}", work, sessions[0], sessions[0])));
        assert!(grants(&s.tick(3)).is_empty(), "nothing more fits while it calls");
        s.live_idle.insert(sessions[0].clone());
        assert_eq!(s.live_room(&sessions[1]), Ok(()));
        // a box measured busy past its charges: the call runs, and new work waits for the measure
        busy.cpu_pct = Some(95.0);
        s.set_caps("a", busy, 4);
        assert_eq!(s.live_room(&sessions[1]), Ok(()));
        assert!(grants(&s.tick(5)).is_empty());
        assert_eq!(state(&s, &more), ("queued".to_string(), "budget cores 16, in use 8 (outside its leases 7), kept for a live call 6, asked 4".to_string()));
        invariants(&s);
    }

    /// On a box that weighs CPU a live launch goes ahead of background shared work: its charges and
    /// the box's measure don't count, and a launch past the box's cores says by how much. Launches
    /// waited 7 min at the median on boxes charged full and half used, and held 75% of what queued
    /// behind holds (11-09). What comes after it waits as ever.
    #[test]
    fn a_live_launch_goes_ahead_of_background_shared_work() {
        let mut busy = caps(16, 64.0, Role::Pool);
        let mut s = sched(&[("a", busy.clone())]);
        let background = |lane: &str, cores: f64| {
            let mut r = overlay(lane, Some("a"), cores);
            r.priority = Priority::Background;
            r
        };
        let (shard, lane) = (s.request(background(TESTS_LANE, 12.0), 0), s.request(background("lane", 4.0), 0));
        assert_eq!(grants(&s.tick(0)).len(), 2);
        // full by charge, and past 95% by the box's measure
        busy.cpu_pct = Some(100.0);
        s.set_caps("a", busy, 1);
        let mut live = overlay("live-W1", Some("a"), 13.0);
        (live.live, live.priority) = (true, Priority::Interactive);
        let session = s.request(live, 2);
        assert_eq!(grants(&s.tick(2)), vec![session.clone()]);
        let l = s.get(&session).unwrap();
        assert_eq!(s.past_budget(l), 13.0);
        assert!(s.past_budget_said(&session).contains("past the box's budget by 13 cores"), "{}", s.past_budget_said(&session));
        // the work it went ahead of says nothing, and its line says it from the next tick
        assert_eq!(s.past_budget(s.get(&lane).unwrap()), 0.0);
        s.tick(3);
        assert!(s.get(&session).unwrap().state_line(3, "").contains("\"past_budget\":13"), "{}", s.get(&session).unwrap().state_line(3, ""));
        assert!(s.get(&shard).unwrap().state_line(3, "").contains("\"past_budget\":0"));
        // background work asked after it waits for the box's cores
        let more = s.request(background("more", 1.0), 4);
        assert!(grants(&s.tick(4)).is_empty());
        assert!(state(&s, &more).1.starts_with("budget cores 16, in use"), "{:?}", state(&s, &more));
        // an idle session is past nothing
        s.live_idle.insert(session.clone());
        s.tick(5);
        assert!(s.get(&session).unwrap().state_line(5, "").contains("\"past_budget\":0"));
        invariants(&s);
    }

    /// What a live launch does not go ahead of still counts: interactive work, gates, GPU runs,
    /// owed legs, and everyone's RAM. A timed run keeps the box to itself.
    #[test]
    fn a_live_launch_still_waits_for_what_does_not_yield_to_it() {
        let live = |at: &str| {
            let mut r = overlay(&format!("live-{at}"), Some("a"), 13.0);
            (r.live, r.priority) = (true, Priority::Interactive);
            r
        };
        let waits = |other: Req, line: &str| {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
            let id = s.request(other, 0);
            assert_eq!(grants(&s.tick(0)), vec![id.clone()]);
            let session = s.request(live("W1"), 1);
            assert!(grants(&s.tick(1)).is_empty(), "{line}: {:?}", state(&s, &session));
            assert!(state(&s, &session).1.starts_with(line), "{:?}", state(&s, &session));
        };
        let in_use_8 = "budget cores 16, in use 8 (background work not counted), asked 13";
        let mut interactive = overlay("interactive", Some("a"), 8.0);
        interactive.priority = Priority::Interactive;
        waits(interactive, in_use_8);
        let mut gate = req("landq-fast", Kind::Gate, Some("a"));
        (gate.priority, gate.cores) = (Priority::Background, Some(8.0));
        waits(gate, in_use_8);
        let mut gpu = overlay("gpu", Some("a"), 8.0);
        (gpu.kind, gpu.priority) = (Kind::Gpu, Priority::Background);
        waits(gpu, in_use_8);
        let mut leg = overlay("jitlegs-warm", Some("a"), 8.0);
        (leg.priority, leg.prio, leg.agent) = (Priority::Background, DEFAULT_PRIO - 1, Some("devhub-jitlegs".to_string()));
        waits(leg, in_use_8);
        let mut timed = req("timed", Kind::Timed, Some("a"));
        timed.priority = Priority::Background;
        waits(timed, "held by");
        let mut ram = overlay("ram", Some("a"), 1.0);
        (ram.priority, ram.ram_gb) = (Priority::Background, Some(40.0));
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let id = s.request(ram, 0);
        assert_eq!(grants(&s.tick(0)), vec![id]);
        let mut hungry = live("W2");
        hungry.ram_gb = Some(30.0);
        let session = s.request(hungry, 1);
        assert!(grants(&s.tick(1)).is_empty());
        assert!(state(&s, &session).1.starts_with("budget ram"), "{:?}", state(&s, &session));
    }

    /// A queued background ask's hold keeps nothing from a live launch, which would go ahead of it
    /// once it ran. A landing gate's does, by the gate's own cores.
    #[test]
    fn a_live_launch_passes_a_background_hold_but_not_a_gate_one() {
        let hold = |owner: Req, asked: f64, granted: bool| {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
            let mut first = overlay("first", Some("a"), 4.0);
            first.priority = Priority::Background;
            let first = s.request(first, 0);
            assert_eq!(grants(&s.tick(0)), vec![first]);
            let owner = s.request(owner, 1);
            s.tick(1);
            assert_eq!(s.holds.get("a").map(|h| h.id.clone()), Some(owner.clone()));
            let mut live = overlay("live-W1", Some("a"), asked);
            (live.live, live.priority) = (true, Priority::Interactive);
            let session = s.request(live, 2);
            let got = grants(&s.tick(2));
            assert_eq!(got.contains(&session), granted, "{:?} {:?}", state(&s, &session), state(&s, &owner));
        };
        let mut shard = overlay(TESTS_LANE, Some("a"), 16.0);
        shard.priority = Priority::Background;
        hold(shard, 13.0, true);
        let mut gate = req(LANDING_LANE, Kind::Gate, Some("a"));
        (gate.priority, gate.cores, gate.src) = (Priority::Interactive, Some(16.0), Src::Tree);
        hold(gate, 13.0, false);
    }

    /// A box that weighs CPU keeps no cores for a live call: an ask fits beside an idle session by
    /// charges, and the call that comes runs ahead of it. 508 core-min kept in 45 min served 38 of
    /// launches and calls (11-09). A call still waits for what does not yield to it, and says who.
    #[test]
    fn a_box_that_weighs_cpu_keeps_no_cores_for_a_call_and_the_call_runs_ahead_of_background() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut live = overlay("live-W1", Some("a"), 6.0);
        live.live = true;
        let session = s.request(live, 0);
        assert_eq!(grants(&s.tick(0)), vec![session.clone()]);
        s.live_idle.insert(session.clone());
        let mut lane = overlay("lane", Some("a"), 15.0);
        lane.priority = Priority::Background;
        let lane = s.request(lane, 1);
        assert_eq!(grants(&s.tick(1)), vec![lane.clone()], "in use 1, asked 15");
        assert_eq!(s.live_room(&session), Ok(()), "6 cores beside background work charged 15");
        s.live_idle.remove(&session);
        assert_eq!(s.past_budget(s.get(&session).unwrap()), 5.0);
        s.live_idle.insert(session.clone());
        // interactive work stays in its way
        let mut t = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut live = overlay("live-W1", Some("a"), 6.0);
        live.live = true;
        let session = t.request(live, 0);
        assert_eq!(grants(&t.tick(0)), vec![session.clone()]);
        t.live_idle.insert(session.clone());
        let mut work = overlay("interactive", Some("a"), 15.0);
        work.priority = Priority::Interactive;
        let work = t.request(work, 1);
        assert_eq!(grants(&t.tick(1)), vec![work.clone()]);
        assert_eq!(t.live_room(&session), Err(format!("a: waiting for 6 cores (15 of 16 in use), held by {work} (test, 15): beside the work granted there")));
        invariants(&s);
    }


    /// A restarted master keeps which live sessions are between calls: each one whose call ended
    /// is idle again, charged its idle share with its call's room kept; one still in its launch is
    /// charged its launch.
    #[test]
    fn a_restarted_master_keeps_its_sessions_between_calls_idle() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let session = |s: &mut Sched, lane: &str| {
            let mut r = overlay(lane, Some("a"), 6.0);
            r.live = true;
            s.request(r, 0)
        };
        let (called, launching) = (session(&mut s, "live-W1"), session(&mut s, "live-W2"));
        assert_eq!(grants(&s.tick(0)).len(), 2);
        s.live_called.insert(called.clone());
        s.live_idle.insert(called.clone());
        let mut s2 = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        for line in s.live_lines() { s2.restore(&line, 5); }
        assert!(s2.live_idle.contains(&called) && s2.live_called.contains(&called));
        assert_eq!(s2.charged(s2.get(&called).unwrap()).cores, 0.0, "no measure yet: an idle session runs nothing");
        assert!(!s2.live_idle.contains(&launching) && !s2.live_called.contains(&launching));
        assert_eq!(s2.charged(s2.get(&launching).unwrap()).cores, 6.0);
        invariants(&s2);
    }

    /// RAM alike, where the call fits beside the ask: a test granted beside an idle Fab session
    /// took the RAM of its next call, which waited 220 s for that test to end (11-06). An ask the
    /// call could never run beside is not kept off by it (a Mac's prebuild beside an idle repro,
    /// 11-04): the call waits for that one. The room parks no session: an ask it keeps off fits
    /// beside the call once the work beside them ends.
    #[test]
    fn an_idle_session_keeps_its_calls_ram_where_the_call_fits_beside_the_ask() {
        let mut s = sched(&[("a", caps(16, 30.0, Role::Pool))]);
        let budget = |s: &Sched| Req::new("x", Kind::Test).ram_budget(s.nodes["a"].caps.as_ref().unwrap());
        assert!((budget(&s) - 25.5).abs() < 1e-9);
        let ask = |lane: &str, ram: Option<f64>| {
            let mut r = overlay(lane, None, 2.0);
            r.ram_gb = ram;
            r
        };
        // a session whose call weighs 10 GB, idle, holding 1 GB
        s.record_resources("live fab", "test", Some(10.0 / 1.2), None);
        let mut live = ask("fab", None);
        (live.live, live.class) = (true, "live fab".into());
        let session = s.request(live, 0);
        assert_eq!(grants(&s.tick(0)), vec![session.clone()]);
        assert!((s.get(&session).unwrap().cost.ram_gb - 10.0).abs() < 1e-9);
        s.live_idle.insert(session.clone());
        s.set_info(&session, r#"{"running":"makepad","held_mb":1024,"idle_s":90}"#, 0);
        // 16 GB never fits beside the call: it is let in beside what the session holds, and the
        // call waits for it
        let big = s.request(ask("big", Some(16.0)), 1);
        assert_eq!(grants(&s.tick(1)), vec![big.clone()]);
        assert!(s.live_room(&session).is_err());
        s.release(&big, "done", 2);
        s.cleaned("a", &big, 0);
        // 12 GB fits beside it; 4 GB more would leave the call 8.5 of its 10 GB
        let work = s.request(ask("work", Some(12.0)), 3);
        assert_eq!(grants(&s.tick(3)), vec![work.clone()]);
        let more = s.request(ask("more", Some(4.0)), 4);
        let acts = s.tick(4);
        assert!(grants(&acts).is_empty());
        assert!(!acts.iter().any(|a| matches!(a, Action::ParkLive { .. })), "the room parks no session: {acts:?}");
        assert!(state(&s, &more).1.starts_with("budget ram 25.5 GB, in use 13.0 GB, kept for a live call 9.0 GB, asked 4.0 GB"), "{:?}", state(&s, &more));
        assert_eq!(s.live_room(&session), Ok(()), "the call has its room");
        // the work ends: the ask fits beside the call
        s.release(&work, "done", 5);
        s.cleaned("a", &work, 0);
        assert_eq!(grants(&s.tick(5)), vec![more.clone()]);
        invariants(&s);
    }

    /// A live call weighs what its session held at most, a fifth more, once one of its calls
    /// ended. Its lane's peak of its class, one session's leak across ten plates (16.2 GB), sized
    /// every call of a Fab session idle at 5.1 GB at 18.3 GB on a box whose budget had become
    /// 17.6 GB, and those calls never got in, one waiting over 5 min (11-06). A first call weighs
    /// its cost, never more than its box's budget now, and a call that waits keeps its room from
    /// every other ask: the work beside it ends, and it runs.
    #[test]
    fn a_live_call_weighs_its_own_peak_and_never_waits_forever() {
        let mut s = sched(&[("a", caps(16, 30.0, Role::Pool))]);
        let ask = |lane: &str, ram: Option<f64>| {
            let mut r = overlay(lane, None, 2.0);
            r.ram_gb = ram;
            r
        };
        for gb in [6.0, 16.2, 7.0] {
            s.record_resources("live fab", "test", Some(gb), None);
            s.record_lane_peak("linux", "blend", "live fab", "test", Some(gb));
        }
        let mut live = ask("blend", None);
        (live.live, live.class) = (true, "live fab".into());
        let session = s.request(live, 0);
        assert_eq!(grants(&s.tick(0)), vec![session.clone()]);
        assert!((s.get(&session).unwrap().cost.ram_gb - 16.2 * 1.2).abs() < 1e-9, "its lane's outlier");
        s.live_idle.insert(session.clone());
        s.set_info(&session, r#"{"running":"makepad","held_mb":5120,"idle_s":90}"#, 0);
        // residents hold 9.4 GB: the box's budget is 17.6 GB, less than the call's cost
        let mut held = caps(16, 30.0, Role::Pool);
        held.ram_resident_gb = 9.4;
        s.set_caps("a", held, 1);
        assert_eq!(s.live_room(&session), Ok(()), "an empty box lets it in: it asks the budget");
        // 4 GB never fits beside that call: let in, and the call waits for it
        let work = s.request(ask("work", Some(4.0)), 2);
        assert_eq!(grants(&s.tick(2)), vec![work.clone()]);
        let why = s.live_room(&session).unwrap_err();
        assert!(why.starts_with("a: budget ram 17.6 GB, in use 4.0 GB, asked 17.6 GB"), "{why}");
        // while it waits it keeps its room: nothing more is let in, and no session parks
        s.live_waiting.insert(session.clone());
        let more = s.request(ask("more", Some(2.0)), 3);
        let acts = s.tick(3);
        assert!(grants(&acts).is_empty());
        assert!(!acts.iter().any(|a| matches!(a, Action::ParkLive { .. })), "{acts:?}");
        assert!(state(&s, &more).1.starts_with("budget ram 17.6 GB, in use 9.0 GB, kept for a live call 12.6 GB, asked 2.0 GB"), "{:?}", state(&s, &more));
        s.release(&work, "done", 4);
        s.cleaned("a", &work, 0);
        assert!(grants(&s.tick(4)).is_empty(), "the room stays its call's");
        assert_eq!(s.live_room(&session), Ok(()), "the work beside it ended: it runs");
        s.live_idle.remove(&session);
        s.live_waiting.remove(&session);
        assert!(grants(&s.tick(5)).is_empty(), "nothing beside its first call");
        // its call ended: the next weighs the 5 GB it held at most and a fifth more
        s.live_called.insert(session.clone());
        s.live_idle.insert(session.clone());
        assert_eq!(grants(&s.tick(6)), vec![more.clone()]);
        assert_eq!(s.live_room(&session), Ok(()));
        s.live_idle.remove(&session);
        assert!((s.charged(s.get(&session).unwrap()).ram_gb - 6.0).abs() < 1e-9, "charged its own weight in its call");
        invariants(&s);
    }

    /// A live session weighs its own lane's peaks of its class: every lane's sessions of an app are
    /// one class, whatever each does in them. A Fab session that peaked at 6.5 GB in an hour asked
    /// 19.4 GB for each call, from another lane's 16.2 GB session (11-06). A lane with none of its
    /// own weighs its family's, and a batch run every lane's.
    #[test]
    fn a_live_session_weighs_its_own_lanes_peaks() {
        let mut s = sched(&[("lin", caps(16, 64.0, Role::Pool))]);
        for (lane, gb) in [("blend", 16.2), ("wow", 6.5)] {
            s.record_resources("live fab", "test", Some(gb), None);
            s.record_family_peak("linux", "live fab", "test", Some(gb));
            s.record_lane_peak("linux", lane, "live fab", "test", Some(gb));
        }
        let mut weigh = |lane: &str, live: bool| {
            let mut r = req(lane, Kind::Test, None);
            (r.live, r.class) = (live, "live fab".into());
            let id = s.request(r, 0);
            s.cost_on("lin", s.get(&id).unwrap()).ram_gb
        };
        assert!((weigh("wow", true) - 6.5 * 1.2).abs() < 1e-9, "its lane's own");
        assert!((weigh("blend", true) - 16.2 * 1.2).abs() < 1e-9);
        assert!((weigh("new", true) - 16.2 * 1.2).abs() < 1e-9, "none of its own: its family's");
        assert!((weigh("wow", false) - 16.2 * 1.2).abs() < 1e-9, "a batch run: every lane's");
    }

    /// `devhub stats cores` says each box's charged cores past its cores in the last hour.
    #[test]
    fn stats_cores_says_each_box_over_its_cores() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        s.nodes.get_mut("a").unwrap().pressure.clear();
        assert_eq!(s.cores_over(0), "cores charged past a box's cores: no samples in the last hour\n");
        let sample = |t_ms: u64, res_cores: f32| super::super::panel::Sample { t_ms, res_cores, ..super::super::panel::Sample::of(t_ms, &caps(16, 64.0, Role::Pool), Cost { cores: 0.0, ram_gb: 0.0, disk_gb: 0.0, gpus: 0 }) };
        s.nodes.get_mut("a").unwrap().pressure.extend([sample(0, 20.0), sample(1_000_000, 18.0), sample(2_000_000, 12.0), sample(3_000_000, 16.0)]);
        // the first sample is older than the hour
        assert_eq!(s.cores_over(3_700_000), "cores charged past each box's cores, the last 45 min:\n  a: 33% of 3 samples over, peak 18 of 16\n");
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
        assert_eq!(state(&s, &x).1, "ram: 0.5 GB available of 64.0 (under 10%)");
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
        warm.jit_store = jit_store(&["jit-a"]);
        s.set_caps("a", warm, 6);
        let mut cold = caps(16, 64.0, Role::Pool);
        cold.jit_store = jit_store(&[]);
        s.set_caps("b", cold, 6);
        s.record_node_cost("jit-build", "", "b", 90_000);
        s.record_node_cost("test:pkg", "test", "a", 20_000);
        s.record_node_cost("test:pkg", "test", "b", 20_000);
        // its pin's own JIT, which an overlay builds where it is missing
        let mut r = req("jit", Kind::Test, None);
        (r.src, r.hint) = (Src::Overlay, vec!["jit-a".into()]);
        r.loader = Some("jit-a".into());
        r.class = "test:pkg".into();
        // (the whole box: an overlay shares no tree to wait on)
        r.cores = Some(16.0);
        let hit = s.request(r.clone(), 7);
        s.tick(7);
        assert_eq!(node_of(&s, &hit).as_deref(), Some("a"), "the ready JIT saves a build");
        s.leases.iter_mut().find(|l| l.id == hit).unwrap().req.class = "long-background".into();
        s.record_node_cost("long-background", "test", "a", 600_000);
        r.lane = "cold".into();
        let cold = s.request(r, 8);
        s.tick(8);
        assert_eq!(node_of(&s, &cold).as_deref(), Some("b"), "its copy to b finishes before the warm box frees");
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

    /// A box whose WM restart waits only on leases drains: a stream of screen work never starves
    /// the restart. A new screen or gui lease goes to another box or waits there, saying why; other
    /// work still takes it, a landing's headless gate and tests among them (they run on through a
    /// WM restart, and one shard after another held a busy box's WM on a replaced binary, 11-08).
    /// Once what the restart waits on ends, nothing it would wait for is left there (the gap it
    /// restarts in), and the screen work takes the box again.
    #[test]
    fn a_screen_stream_never_starves_a_boxs_wm_restart_and_landings_never_hold_it() {
        let shown = |mut c: Caps| { c.display = Some("wm:wayland-wm-0".into()); c };
        let mut s = sched(&[("a", shown(caps(16, 64.0, Role::Pool))), ("b", shown(caps(16, 64.0, Role::Pool)))]);
        // (lanes' screen and gui work runs on overlays)
        let gui = |lane: &str, kind: Kind, node: Option<&str>| { let mut r = req(lane, kind, node); r.needs = parse_needs("gui=1"); r.src = Src::Overlay; r };
        let screen = |lane: &str, node: Option<&str>| { let mut r = req(lane, Kind::Screen, node); r.src = Src::Overlay; r };
        let mut r = gui("lane-gui", Kind::Test, Some("a"));
        r.cores = Some(4.0);
        let shard = s.request(r, 0);
        s.tick(0);
        assert_eq!(node_of(&s, &shard).as_deref(), Some("a"));
        // its landing's sync gave the WM another wire: the restart waits on the gui test alone
        s.nodes.get_mut("a").unwrap().caps.as_mut().unwrap().wm_drain = format!("test lease {shard} of lane-gui (its windows)");
        let mut asked = Vec::new();
        for t in 1..4u64 {
            for r in [screen("lane-s", None), gui("lane-ui", Kind::Build, None)] {
                asked.push(s.request(r, t));
            }
            s.tick(t);
            assert!(asked.iter().all(|id| node_of(&s, id).as_deref() != Some("a")), "{:?}", asked.iter().map(|id| state(&s, id)).collect::<Vec<_>>());
        }
        // other work still takes it, a landing's headless gate among them; nor would a landing's
        // headless tests shard hold the restart
        assert!(!req(TESTS_LANE, Kind::Test, None).cut_by_wm_restart() && !req("landq-hot", Kind::Test, None).cut_by_wm_restart());
        let mut r = overlay("lane-x", Some("a"), 2.0);
        r.kind = Kind::Build;
        let plain = s.request(r, 4);
        let gate = s.request(req("landq-fast", Kind::Gate, Some("a")), 4);
        s.tick(4);
        for id in [&plain, &gate] {
            assert_eq!(node_of(&s, id).as_deref(), Some("a"), "{:?}", state(&s, id));
        }
        let named = s.request(screen("lane-s2", Some("a")), 5);
        s.tick(5);
        assert!(state(&s, &named).1.starts_with(&format!("its WM restarts first, once what it waits on ends (test lease {shard} of lane-gui (its windows))")), "{:?}", state(&s, &named));
        // the shard ends: the gap, nothing there the restart waits for; it restarts, the drain lifts
        let on_a = |s: &Sched| s.leases.iter().filter(|l| l.state.live() && l.node.as_deref() == Some("a")).map(|l| (l.id.clone(), l.req.cut_by_wm_restart())).collect::<Vec<_>>();
        assert!(on_a(&s).contains(&(plain.clone(), false)) && on_a(&s).contains(&(gate.clone(), false)), "{:?}", on_a(&s));
        release_clean(&mut s, &shard, 6);
        s.tick(6);
        assert!(on_a(&s).iter().all(|(_, cut)| !cut), "the gap: {:?}", on_a(&s));
        s.nodes.get_mut("a").unwrap().caps.as_mut().unwrap().wm_drain.clear();
        s.tick(7);
        assert!(on_a(&s).iter().any(|(_, cut)| *cut), "the screen work takes it again: {:?}", on_a(&s));
        invariants(&s);
    }

    /// A box's dev JIT store holding `ids`, built by one compiler.
    fn jit_store(ids: &[&str]) -> Option<std::sync::Arc<super::super::jitstore::Snapshot>> {
        Some(std::sync::Arc::new(super::super::jitstore::Snapshot { ids: ids.iter().map(|id| id.to_string()).collect(), compiler: "rustc-1".into(), ..Default::default() }))
    }

    /// A run that asks for a JIT by id (`--jit`) gets it by construction: only a box that holds
    /// it, one of its platform it is copied to at the grant ([`Sched::jit_source`]), or one its
    /// lease builds it on (an overlay's pin's own JIT), ever takes it; a named box of another
    /// platform, or none holding it, refuses it at the ask, never a run that starts and fails.
    #[test]
    fn a_run_asking_for_a_jit_by_id_goes_only_where_it_is() {
        let mut mac = caps(16, 64.0, Role::Pool);
        (mac.os, mac.arch) = ("macos".into(), "aarch64".into());
        let boxes = [("holder", caps(16, 64.0, Role::Pool)), ("idle", caps(32, 128.0, Role::Pool)), ("mac", mac), ("other", caps(16, 64.0, Role::Pool))];
        let mut s = sched(&boxes);
        s.nodes.get_mut("holder").unwrap().caps.as_mut().unwrap().jit_store = jit_store(&["tipjit"]);
        s.nodes.get_mut("idle").unwrap().caps.as_mut().unwrap().jit_store = jit_store(&["lanejit"]);
        s.nodes.get_mut("mac").unwrap().caps.as_mut().unwrap().jit_store = jit_store(&[]);
        s.nodes.get_mut("other").unwrap().caps.as_mut().unwrap().jit_store = jit_store(&["tipjit"]);
        s.nodes.get_mut("other").unwrap().caps.as_mut().unwrap().load1 = 15.0;
        // a job's script that runs the tip's JIT, its box named: one of another platform refuses
        // at the ask
        let mut r = req("jittier", Kind::Build, Some("mac"));
        (r.loader, r.needs) = (Some("tipjit".into()), parse_needs("os=macos"));
        let said = s.never_places(&r).expect("a named box of another platform never takes it");
        assert_eq!(said, "holds no JIT tipjit (held by holder other, none of its platform; a lease builds only its pin's own JIT, as an overlay; without --jit it runs that one)");
        let id = s.request(r.clone(), 0);
        s.tick(0);
        assert_eq!(state(&s, &id).0, "ended denied", "{:?}", state(&s, &id));
        // one of its platform without it takes it, copied there at the grant from a box that holds it
        (r.node, r.needs) = (Some("idle".into()), Vec::new());
        assert_eq!(s.never_places(&r), None);
        let id = s.request(r.clone(), 0);
        s.tick(0);
        assert_eq!(node_of(&s, &id).as_deref(), Some("idle"), "{:?}", state(&s, &id));
        assert_eq!(s.jit_source("idle", s.get(&id).unwrap()).map(String::as_str), Some("holder"));
        assert_eq!(s.jit_source("holder", s.get(&id).unwrap()), None, "it holds it");
        // unnamed: the box holding it, over a bigger idle one it would be copied to
        r.node = None;
        assert_eq!(s.never_places(&r), None);
        let id = s.request(r.clone(), 1);
        s.tick(1);
        assert_eq!(node_of(&s, &id).as_deref(), Some("holder"));
        // none holds it: refused at the ask
        r.loader = Some("gonejit".into());
        assert!(s.never_places(&r).is_some_and(|w| w.contains("holds no JIT gonejit (no box holds it;")), "{:?}", s.never_places(&r));
        // an overlay whose pin's own JIT it is builds it on a box that lacks it
        let mut o = overlay("lane", Some("idle"), 4.0);
        (o.loader, o.hint) = (Some("pinjit".into()), vec!["pinjit".into()]);
        assert_eq!(s.never_places(&o), None);
        // but not one its pin does not select (a tree lease never builds an asked JIT)
        o.hint = vec!["otherjit".into()];
        assert!(s.never_places(&o).is_some());
        let mut t = req("lane", Kind::Build, Some("idle"));
        (t.loader, t.hint) = (Some("pinjit".into()), vec!["pinjit".into()]);
        assert!(s.never_places(&t).is_some(), "a tree lease runs a held JIT only");
        invariants(&s);
        // a hub that predates `--jit` for any command waits for its update: an older one ran a
        // script without its $MAKEPAD_LOADER (11-07)
        let mut s = sched(&[("holder", caps(16, 64.0, Role::Pool))]);
        s.nodes.get_mut("holder").unwrap().caps.as_mut().unwrap().jit_store = jit_store(&["tipjit"]);
        s.nodes.get_mut("holder").unwrap().caps.as_mut().unwrap().features.retain(|f| f != "jit-ask");
        s.nodes.get_mut("holder").unwrap().caps.as_mut().unwrap().deploy = "abc,def".into();
        let mut r = req("jittier", Kind::Build, Some("holder"));
        r.loader = Some("tipjit".into());
        let id = s.request(r, 0);
        assert!(grants(&s.tick(0)).is_empty());
        assert!(state(&s, &id).1.contains("its hub lacks jit-ask for this request: waits for its update"), "{:?}", state(&s, &id));
        let plain = s.request(req("other", Kind::Build, Some("holder")), 1);
        assert_eq!(grants(&s.tick(1)), vec![plain.clone()], "a lease without --jit keeps using the hub");
        release_clean(&mut s, &plain, 2);
        assert!(s.cancel(&id, 2));
        // a timed one holds no deployment while it waits for the hub's update: that update runs
        // on the deployment it would hold
        let mut timed = req("jittier", Kind::Timed, Some("holder"));
        timed.loader = Some("tipjit".into());
        let timed = s.request(timed, 2);
        s.tick(2);
        assert!(state(&s, &timed).1.contains("waits for its update"), "{:?}", state(&s, &timed));
        assert_eq!(s.update_pin("holder"), None, "it pins nothing");
        s.nodes.get_mut("holder").unwrap().caps.as_mut().unwrap().features.push("jit-ask".into());
        assert_eq!(s.update_pin("holder").map(|(id, _)| id), Some(timed), "updated, it is first in line");
    }

    /// A rev naming a nested repository the hub without `nested` does not
    /// know waits for its update; one naming makepad and the paired one keeps using it.
    #[test]
    fn a_rev_naming_another_nested_repository_waits_for_the_hubs_update() {
        let mut s = sched(&[("old", caps(16, 64.0, Role::Pool))]);
        s.nodes.get_mut("old").unwrap().caps.as_mut().unwrap().features.retain(|f| f != "nested");
        let (a, c, e) = ("a".repeat(40), "c".repeat(40), "e".repeat(40));
        let mut r = req("sl", Kind::Build, Some("old"));
        r.rev = Some(format!("rc-sl@{a},commercial=main@{c},source-library=rc-sl@{e}"));
        let id = s.request(r, 0);
        assert!(grants(&s.tick(0)).is_empty());
        assert!(state(&s, &id).1.contains("its hub lacks nested for this request: waits for its update"), "{:?}", state(&s, &id));
        let mut paired = req("other", Kind::Build, Some("old"));
        paired.rev = Some(format!("rc-x@{a},commercial=main@{c}"));
        let paired = s.request(paired, 1);
        assert_eq!(grants(&s.tick(1)), vec![paired.clone()], "makepad and the paired repository: the hub runs it as before");
        release_clean(&mut s, &paired, 2);
        s.nodes.get_mut("old").unwrap().caps.as_mut().unwrap().features.push("nested".into());
        assert_eq!(grants(&s.tick(3)), vec![id], "updated, it starts");
    }

    /// A build `--for` another box's CPU waits for a hub that builds for it: one without
    /// `for-cpu` would build for its own CPU and say nothing. The CPU rides
    /// the request's JSON, and one that is no CPU name is refused there.
    #[test]
    fn a_build_for_another_boxs_cpu_waits_for_a_hub_that_builds_for_it() {
        let mut s = sched(&[("old", caps(16, 64.0, Role::Pool))]);
        s.nodes.get_mut("old").unwrap().caps.as_mut().unwrap().features.retain(|f| f != "for-cpu");
        let mut r = req("cross", Kind::Build, None);
        r.cpu_target = Some("x86-64-v3".into());
        assert_eq!(Req::parse(&r.to_json()).unwrap().cpu_target.as_deref(), Some("x86-64-v3"), "round-trips");
        let mut bad = r.clone();
        bad.cpu_target = Some("native -C opt-level=0".into());
        assert!(Req::parse(&bad.to_json()).is_err(), "no flags ride in as a CPU");
        let id = s.request(r, 0);
        assert!(grants(&s.tick(0)).is_empty());
        assert!(state(&s, &id).1.contains("its hub lacks for-cpu for this request: waits for its update"), "{:?}", state(&s, &id));
        let own = s.request(req("plain", Kind::Build, None), 1);
        assert_eq!(grants(&s.tick(1)), vec![own.clone()], "a build for the box's own CPU runs there as before");
        release_clean(&mut s, &own, 2);
        s.nodes.get_mut("old").unwrap().caps.as_mut().unwrap().features.push("for-cpu".into());
        assert_eq!(grants(&s.tick(3)), vec![id], "updated, it starts");
    }

    /// A lane's JIT built on a bench box: its ordinary tests go to a pool box of its platform, which
    /// gets it copied there at the grant ([`Sched::jit_source`]), and the bench box keeps its quiet
    /// window for the lane's timed runs (a lane's JIT only on the box that built it put its tests
    /// there, and another lane's timed run waited 22 min behind them, 11-07). The pool box waits to
    /// copy it while its hub predates the copy, or while no box that holds it is up.
    #[test]
    fn a_lane_jit_built_on_a_bench_box_runs_its_tests_on_a_pool_box() {
        let held = |c: Caps, ids: &[&str]| Caps { jit_store: jit_store(ids), ..c };
        let mut s = sched(&[("bench", held(caps(16, 64.0, Role::Bench), &["lanejit"])), ("pool", held(caps(16, 64.0, Role::Pool), &[]))]);
        let ask = |kind| { let mut r = req("jittier", kind, None); r.loader = Some("lanejit".into()); r };
        s.nodes.get_mut("pool").unwrap().caps.as_mut().unwrap().features.retain(|f| f != "jit-copy");
        let test = s.request(ask(Kind::Test), 0);
        assert!(grants(&s.tick(0)).is_empty());
        assert!(state(&s, &test).1.contains("its hub lacks jit-copy (to copy JIT lanejit in): waits for its update"), "{:?}", state(&s, &test));
        s.nodes.get_mut("pool").unwrap().caps.as_mut().unwrap().features.push("jit-copy".into());
        s.set_down("bench");
        assert!(grants(&s.tick(1)).is_empty());
        assert!(state(&s, &test).1.contains("JIT lanejit: no box that holds it is up (and updated) to copy it here"), "{:?}", state(&s, &test));
        s.set_caps("bench", held(caps(16, 64.0, Role::Bench), &["lanejit"]), 2);
        assert_eq!(grants(&s.tick(2)), vec![test.clone()]);
        assert_eq!(node_of(&s, &test).as_deref(), Some("pool"));
        assert_eq!(s.jit_source("pool", s.get(&test).unwrap()).map(String::as_str), Some("bench"));
        let timed = s.request(ask(Kind::Timed), 3);
        s.tick(3);
        assert_eq!(node_of(&s, &timed).as_deref(), Some("bench"), "{:?}", state(&s, &timed));
        invariants(&s);
    }

    /// A box off the network for a minute ends its live leases as lost: their holders ask again
    /// at once, not after a 15 min re-attach. A refused connect between (its hub restarting)
    /// starts the minute again, work queued for the box waits for it, and it takes work once its
    /// caps answer (a pool box left the network holding five leases, 11-07).
    /// An outage of the master's own network costs no lease (11-09: its address changed, and after
    /// a minute with no route every hub refused it for 20 min): the boxes run the work on, and the
    /// leases and their results are there when the master is back. Back, a box that really died
    /// ends its leases as lost a minute on, as ever.
    #[test]
    fn an_outage_of_the_masters_own_network_costs_no_lease() {
        const MIN: u64 = 60_000;
        let boxes = ["box-a", "box-b", "box-c"];
        let mut s = sched(&boxes.map(|b| (b, caps(16, 64.0, Role::Pool))));
        for _ in 0..3 { s.record_node_cost("test", "test", "box-a", 382_000); }
        let mut gate = req("landq-fast", Kind::Test, Some("box-a"));
        gate.agent = Some(BUCKET_AGENT.into());
        let gate = s.request(gate, 0);
        let run = s.request(req("lane", Kind::Test, Some("box-b")), 0);
        let dies = s.request(req("lane-c", Kind::Test, Some("box-c")), 0);
        assert_eq!(grants(&s.tick(0)).len(), 3);
        for id in [&gate, &run, &dies] {
            s.started(id, "{}");
            s.set_info(id, r#"{"idle_s":0,"out_bytes":100}"#, 1_000);
        }
        let mut said = Vec::new();
        let look = |s: &mut Sched, t: u64, said: &mut Vec<Record>| {
            beat_all(s, t);
            let a = s.tick(t);
            assert!(a.iter().all(|x| !matches!(x, Action::End { .. })), "at {t}: {a:?}");
            said.extend(a.into_iter().filter_map(|x| match x { Action::Event(r) => Some(r), _ => None }));
        };
        let mut t = 0;
        while t < 4 * MIN {
            t += 10_000;
            for b in boxes { s.set_caps(b, caps(16, 64.0, Role::Pool), t); }
            look(&mut s, t, &mut said);
        }
        // the gate is past twice its measured 6m22s by the end, writing until the outage
        s.set_info(&gate, r#"{"idle_s":0,"out_bytes":200}"#, t);
        let quiet = t;
        while t < quiet + 20 * MIN {
            t += 10_000;
            let miss = if t < quiet + MIN { "link lost: cannot reach fleet hub at 192.0.2.1:8797: No route to host (os error 65)" } else { "credential `fleet-mac` has no rights on this hub" };
            for b in boxes { assert!(s.caps_missed(b, miss, t).is_empty(), "at {t}"); }
            if t == quiet + 10 * MIN { s.captured(&gate, "its processes were not read: link lost"); }
            look(&mut s, t, &mut said);
        }
        let deaf: Vec<&Record> = said.iter().filter(|r| r.line.starts_with("the master has heard none of its boxes for ")).collect();
        assert_eq!(deaf.len(), 1, "said once: {said:?}");
        assert!(deaf[0].sev == Sev::Warn && deaf[0].line.contains("(every box went quiet at once)"), "{}", deaf[0].line);
        for id in [&gate, &run, &dies] { assert_eq!(state(&s, id).0, "granted"); }
        // back, box-c with it no longer: its lease ends as lost a minute on, the others run on
        let back = t + 10_000;
        let mut ended = None;
        while t < back + 2 * MIN {
            t += 10_000;
            for b in ["box-a", "box-b"] { s.set_caps(b, caps(16, 64.0, Role::Pool), t); }
            let lost = s.caps_missed("box-c", "link lost: cannot reach fleet hub at 192.0.2.1:8797: Host is down (os error 64)", t);
            if !lost.is_empty() {
                assert_eq!(lost, vec![dies.clone()]);
                ended = Some(t);
            }
            beat_all(&mut s, t);
            said.extend(s.tick(t).into_iter().filter_map(|x| match x { Action::Event(r) => Some(r), _ => None }));
        }
        assert!(said.iter().any(|r| r.line.starts_with("the master hears its boxes again after 20m10s: ")), "{said:?}");
        assert_eq!(ended, Some(back + HOST_GONE_MS), "box-c's lease ends a minute after the master hears again");
        assert!(state(&s, &dies).1.contains("its box box-c is off the network for 1m00s"), "{:?}", state(&s, &dies));
        assert_eq!(state(&s, &gate).0, "granted", "no gate hangs on output the master could not hear");
        assert!(s.release(&run, "released", t), "its holder has its result");
        assert_eq!(state(&s, &run).0, "ended released");
    }

    /// The master's own network check holds the count for a fleet of one too, and only while it
    /// says so.
    #[test]
    fn the_masters_own_network_fault_holds_a_lone_boxs_leases() {
        let mut s = sched(&[("node-a", caps(16, 64.0, Role::Pool))]);
        let run = s.request(req("lane-x", Kind::Test, None), 0);
        assert_eq!(grants(&s.tick(0)), vec![run.clone()]);
        s.started(&run, "{}");
        s.own_network(Some("this Mac's address is not the credential's bound address".into()));
        let gone = "link lost: cannot reach fleet hub at 192.0.2.1:8797: Host is down (os error 64)";
        for t in (1..=30).map(|i| i * 10_000) {
            assert!(s.caps_missed("node-a", gone, t).is_empty(), "at {t}");
            s.beat(&run, t);
            s.tick(t);
        }
        assert_eq!(state(&s, &run).0, "granted");
        s.own_network(None);
        let lost: Vec<u64> = (31..=40).map(|i| i * 10_000).filter(|t| !s.caps_missed("node-a", gone, *t).is_empty()).collect();
        assert_eq!(lost, vec![370_000], "a minute of misses once its network is fine");
    }

    /// A node line says what the box says now. An outage's 15 min of `devhub nodes` read as a healthy
    /// fleet: every box's old numbers, with `down` at the far end of the line (11-09).
    #[test]
    fn a_node_line_shows_no_old_numbers_as_current() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let line = |s: &Sched, now: u64| s.node_view(now).into_iter().find(|n| n.name == "a").unwrap().caps_line();
        s.set_caps("a", caps(16, 64.0, Role::Pool), 1_000);
        let heard = line(&s, 11_000);
        assert!(heard.contains("16 cores") && !heard.contains("not heard"), "{heard}");
        // a miss: it counts as up, and its numbers are its last
        let refused = "credential `fleet-mac` has no rights on this hub";
        s.caps_missed("a", refused, 11_000);
        let missed = line(&s, 11_000);
        assert!(missed.contains("16 cores") && missed.ends_with(&format!("\u{b7} not heard for 10 s ({refused})")), "{missed}");
        // down: no numbers at all
        s.caps_missed("a", refused, 41_000);
        assert_eq!(line(&s, 41_000), format!("down: not heard for 40 s ({refused})"));
        assert_eq!(line(&s, 241_000), format!("down: not heard for 4m00s ({refused})"), "the time runs without another miss");
        // a sleep of the master's machine is no time the box went unheard
        s.master_slept(200_000, 241_000);
        assert_eq!(line(&s, 241_000), format!("down: not heard for 40 s ({refused})"));
        // heard again: as if nothing happened
        s.set_caps("a", caps(16, 64.0, Role::Pool), 250_000);
        let back = line(&s, 250_000);
        assert!(back.contains("16 cores") && !back.contains("not heard"), "{back}");
        // a caps call that never returns notes no miss, and its answer gets as old
        let hung = line(&s, 280_000);
        assert!(hung.contains("16 cores") && hung.ends_with("\u{b7} not heard for 30 s (its caps call has not answered)"), "{hung}");
        // a lease's start that could not reach it, with no caps gone unanswered yet
        s.set_down("a");
        assert_eq!(line(&s, 250_000), "down");
    }

    #[test]
    fn a_box_off_the_network_for_a_minute_ends_its_leases_as_lost() {
        let mut s = sched(&[("node-a", caps(16, 64.0, Role::Pool))]);
        let run = s.request(req("lane-x", Kind::Test, None), 0);
        assert_eq!(grants(&s.tick(0)), vec![run.clone()]);
        s.started(&run, "{}");
        let gone = "link lost: cannot reach fleet hub `node-a` at 192.0.2.1:8797: Host is down (os error 64)";
        assert!(s.caps_missed("node-a", gone, 1_000).is_empty());
        assert!(s.caps_missed("node-a", gone, 31_000).is_empty(), "down, its leases held");
        // (their holders beat the master all along: only their box is gone)
        s.beat(&run, 31_000);
        let pinned = s.request(req("lane-y", Kind::Test, Some("node-a")), 31_000);
        assert!(grants(&s.tick(31_000)).is_empty());
        assert_eq!(state(&s, &run).0, "granted");
        assert!(s.caps_missed("node-a", "Connection refused", 40_000).is_empty(), "refused: its hub restarts");
        assert!(s.caps_missed("node-a", gone, 50_000).is_empty());
        assert!(s.caps_missed("node-a", gone, 100_000).is_empty(), "off the network 50 s since the refused connect");
        assert_eq!(s.caps_missed("node-a", gone, 110_000), vec![run.clone()]);
        let (st, why) = state(&s, &run);
        assert!(st == "ended lost" && why.contains("its box node-a is off the network for ") && why.contains("Host is down"), "{st}: {why}");
        assert_eq!(state(&s, &pinned).0, "queued");
        assert!(s.caps_missed("node-a", gone, 120_000).is_empty(), "nothing left to end");
        // back: its caps answer, and the lease-end the master kept for it is answered
        s.set_caps("node-a", caps(16, 64.0, Role::Pool), 130_000);
        s.cleaned("node-a", &run, 0);
        s.beat(&pinned, 130_000);
        assert_eq!(grants(&s.tick(130_000)), vec![pinned.clone()], "{:?}", state(&s, &pinned));
    }

    /// Check every candidate, including partially occupied boxes, not just empty ones. Holds
    /// are checked independently of now_fits so an invalid reservation cannot hide idle capacity.
    fn invariants(s: &Sched) {
        // what its holder reads of it: an answer no JSON parser takes is a lost lease
        for l in &s.leases {
            let line = l.state_line(s.clock, "");
            if let Err(e) = makepad_strict_json::parse(line.as_bytes()) { panic!("{}'s state line: {e}: {line}", l.id); }
        }
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
            assert!(h.until > s.clock);
            let cost = s.cost_on(name, l);
            assert!(s.ready_to_hold(name, l, &cost).is_ok(), "{} cannot start on {name} even without running work: {:?}", h.id, s.ready_to_hold(name, l, &cost));
            assert!(s.hot_wait(name, l, &s.holds).is_none());
            // (a timed request past its due time reserves despite interactive demand)
            if l.req.kind == Kind::Timed && !s.timed_due(l) {
                assert!(!s.interactive_boxes.contains_key(name), "{} reserves {name} while interactive work is waiting or running", h.id);
                for r in s.leases.iter().filter(|r| r.priority == Priority::Interactive && r.req.kind != Kind::Timed && r.req.on_node()) {
                    // (a running live job is no such demand: it parks for the timed request)
                    if r.state.live() && !s.is_live_job(r) { assert_ne!(r.node.as_deref(), Some(name.as_str())); }
                    if r.state == State::Queued && s.ready(name, r, &s.cost_on(name, r)).is_ok() {
                        assert!(!s.candidates(r).is_ok_and(|names| names.contains(&name)), "timed hold hides interactive demand {} on {name}: {} {:?} {:?} holds {:?}", r.id, r.reason, r.req.node, s.candidates(r).ok(), s.holds.iter().map(|(n, h)| (n.clone(), h.id.clone(), s.get(&h.id).map(|o| (o.req.kind, o.req.whole, o.priority)))).collect::<Vec<_>>());
                    }
                }
            }
            assert!(s.resources(name, l, &cost, true).is_err(), "{} holds an available box {name}", h.id);
            let names = s.candidates(l).ok().unwrap();
            assert!(names.contains(&name));
            // (a shard, a gate, a whole ask or one queued long holds past holds of other boxes:
            // [`Sched::holds_past_other_holds`], [`Sched::starved`])
            assert!(Sched::holds_past_other_holds(l) || s.starved(l) || names.iter().all(|n| s.held_by(n, l, &s.cost_on(n, l), &s.holds).is_none()), "transitive hold by {}", h.id);
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
        // A required feature update makes the node unready; its old hold must not persist.
        s.nodes.get_mut("b").unwrap().caps.as_mut().unwrap().features.clear();
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
        assert!(s.why_queued(&timed, 3).unwrap().iter().any(|line| line.contains("it reports no features: an older hub")));
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

    /// A whole-box ask holds a box while another box is held against it, so lane work asked after
    /// it does not take each box as its work ends: one waited 37 min first in its queue, holding
    /// no box whenever a tests shard or a gate held another, while lane asks took every box that
    /// freed (11-08).
    #[test]
    fn a_whole_ask_holds_a_box_while_another_box_is_held_against_it() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool)), ("c", caps(16, 64.0, Role::Pool))]);
        let ask = |lane: &str, node: Option<&str>, cores: f64, min: f64| {
            let mut r = overlay(lane, node, cores);
            r.est_min = Some(min);
            r
        };
        let running: Vec<String> = ["a", "b", "c"].iter().map(|n| s.request(ask(&format!("run{n}"), Some(n), 8.0, 10.0), 0)).collect();
        let tails: Vec<String> = ["a", "b", "c"].iter().map(|n| s.request(ask(&format!("tail{n}"), Some(n), 2.0, 20.0), 0)).collect();
        s.tick(0);
        let mut shard = ask(TESTS_LANE, None, 12.0, 20.0);
        shard.priority = Priority::Background;
        let shard = s.request(shard, 1);
        s.tick(1);
        let shard_box = s.holds.iter().find(|(_, h)| h.id == shard).map(|(n, _)| n.clone()).expect("the shard holds a box");
        let mut whole = ask("flakes", None, 4.0, 15.0);
        whole.whole = true;
        let whole = s.request(whole, 2);
        beat_all(&mut s, 2);
        s.tick(2);
        let held = |s: &Sched| s.holds.iter().find(|(_, h)| h.id == whole).map(|(n, _)| n.clone());
        let whole_box = held(&s);
        assert!(whole_box.is_some() && whole_box != Some(shard_box), "the whole ask holds another box: {:?} {:?}", s.holds, state(&s, &whole));
        invariants(&s);
        // as the boxes' first leases end, lane work asked after it goes beside what still runs,
        // never on the box it holds
        for (i, r) in running.iter().enumerate() {
            release_clean(&mut s, r, 10 * MIN + i as u64);
        }
        let lanes: Vec<String> = (0..3).map(|i| s.request(ask(&format!("lane{i}"), None, 4.0, 30.0), 10 * MIN + 10 + i)).collect();
        beat_all(&mut s, 10 * MIN + 20);
        s.tick(10 * MIN + 20);
        let whole_box = held(&s).expect("it still holds a box");
        assert!(lanes.iter().all(|l| node_of(&s, l).as_deref() != Some(whole_box.as_str())), "{whole_box}: {:?}", lanes.iter().map(|l| (node_of(&s, l), state(&s, l))).collect::<Vec<_>>());
        assert!(lanes.iter().any(|l| node_of(&s, l).is_some()), "lane work goes on the other boxes: {:?}", lanes.iter().map(|l| state(&s, l)).collect::<Vec<_>>());
        invariants(&s);
        // and the whole ask starts as the boxes' work ends
        for r in &tails { release_clean(&mut s, r, 20 * MIN); }
        beat_all(&mut s, 20 * MIN + 1);
        s.tick(20 * MIN + 1);
        assert!(node_of(&s, &whole).is_some_and(|n| lanes.iter().all(|l| node_of(&s, l).as_deref() != Some(n.as_str()))), "{:?}", state(&s, &whole));
        invariants(&s);
    }

    /// An ask queued [`OVERDUE_MS`] holds its box against newer interactive work and ranks ahead of
    /// it; before that it gives way. A landing's gate, a live launch and the box's own hub build
    /// still go in (L716027, 11-09).
    #[test]
    fn an_overdue_ask_holds_its_box_against_newer_interactive_work() {
        const MIN: u64 = 60_000;
        let ask = |lane: &str, cores: f64, est_min: f64, priority: Priority| {
            let mut r = overlay(lane, None, cores);
            r.est_min = Some(est_min);
            r.priority = priority;
            r
        };
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let run = s.request(ask("run", 8.0, 90.0, Priority::Background), 0);
        s.tick(0);
        let head = s.request(ask("head", 12.0, 8.0, Priority::Background), 1);
        // starved, not overdue: interactive work takes what is free
        let early = s.request(ask("early", 8.0, 30.0, Priority::Interactive), 11 * MIN);
        beat_all(&mut s, 11 * MIN);
        assert_eq!(grants(&s.tick(11 * MIN)), vec![early.clone()], "{:?}", state(&s, &early));
        release_clean(&mut s, &early, 12 * MIN);
        beat_all(&mut s, 12 * MIN);
        s.tick(12 * MIN);
        // overdue: newer interactive work waits for it, and it comes first in the queue
        let late = s.request(ask("late", 8.0, 30.0, Priority::Interactive), 31 * MIN);
        beat_all(&mut s, 31 * MIN);
        assert!(grants(&s.tick(31 * MIN)).is_empty(), "{:?}", state(&s, &late));
        assert!(state(&s, &late).1.contains(&format!("held for {head} (it starts")), "{:?}", state(&s, &late));
        let rank = |s: &Sched, id: &str| s.queue_order().iter().position(|&i| s.leases[i].id == id);
        assert!(rank(&s, &head) < rank(&s, &late));
        // what goes ahead of it still does
        let mut gate = ask("landq-fast", 4.0, 5.0, Priority::Interactive);
        gate.kind = Kind::Gate;
        let gate = s.request(gate, 32 * MIN);
        let mut launch = ask("session", 2.0, 30.0, Priority::Interactive);
        launch.live = true;
        let launch = s.request(launch, 32 * MIN);
        let mut hub = ask("hub-build-a", 4.0, 2.0, Priority::Background);
        (hub.class, hub.master, hub.node) = (HUB_BUILD.into(), true, Some("a".into()));
        let hub = s.request(hub, 32 * MIN);
        beat_all(&mut s, 32 * MIN);
        let got = grants(&s.tick(32 * MIN));
        for id in [&gate, &launch, &hub] { assert!(got.contains(id), "{id}: {:?} {got:?}", state(&s, id)); }
        assert!(!got.contains(&late), "{:?}", state(&s, &late));
        for id in [&gate, &launch, &hub, &run] { release_clean(&mut s, id, 40 * MIN); }
        // its box's work ended: it starts, ahead of the newer ask
        beat_all(&mut s, 40 * MIN);
        let got = grants(&s.tick(40 * MIN));
        assert!(got.contains(&head), "{:?} {got:?}", state(&s, &head));
        invariants(&s);
    }

    /// An ask that waited [`STARVED_MS`] holds the box whose work ends first and drains it, as a
    /// whole-box ask does: its live sessions park for it, lane work asked after it stays off it,
    /// and its queue line says when it starts. A 16-core lane test headed its queue 46 min: a live
    /// session on its box had no end, so it held nothing, and lane asks took each core the box
    /// freed (L661568, 11-08).
    #[test]
    fn a_long_waiting_head_holds_the_box_that_frees_first_and_drains_it() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = overlay("session", Some("a"), 3.0);
        r.priority = Priority::Interactive;
        r.live = true;
        let session = s.request(r, 0);
        let mut r = overlay("run", Some("a"), 8.0);
        r.est_min = Some(5.0);
        let run = s.request(r, 0);
        s.tick(0);
        let mut r = overlay("flakes", None, 16.0);
        r.est_min = Some(30.0);
        let head = s.request(r, 1);
        beat_all(&mut s, 1);
        s.tick(1);
        // the session has no end: before its wait is long it holds nothing, and lane work goes in
        assert!(s.holds.values().all(|h| h.id != head), "{:?} {:?}", s.holds, state(&s, &head));
        release_clean(&mut s, &run, 5 * MIN);
        let mut r = overlay("lane", None, 8.0);
        r.est_min = Some(20.0);
        let lane = s.request(r, 5 * MIN + 1);
        beat_all(&mut s, 5 * MIN + 2);
        s.tick(5 * MIN + 2);
        assert_eq!(node_of(&s, &lane).as_deref(), Some("a"), "{:?}", state(&s, &lane));
        // waited long: it holds the box and is told when it starts; work asked after it stays off
        // unless it ends first, and the session parks once the rest there has ended
        beat_all(&mut s, 10 * MIN + 1);
        s.tick(10 * MIN + 1);
        assert_eq!(s.holds.get("a").map(|h| h.id.clone()), Some(head.clone()), "{:?}", state(&s, &head));
        let l = s.get(&head).unwrap();
        assert!(l.eta_box.as_deref() == Some("a") && l.eta_ms > 0, "an expected start: {:?} {}", l.eta_box, l.eta_ms);
        assert_eq!(l.told_ms, Some(10 * MIN + 1 + l.eta_ms), "the start it is told, absolute");
        let mut r = overlay("more", None, 4.0);
        r.est_min = Some(30.0);
        let more = s.request(r, 10 * MIN + 2);
        beat_all(&mut s, 10 * MIN + 3);
        assert!(grants(&s.tick(10 * MIN + 3)).is_empty(), "{:?}", state(&s, &more));
        assert!(state(&s, &more).1.contains(&format!("held for {head} (")) && state(&s, &more).1.contains("; it has queued 10m"), "{:?}", state(&s, &more));
        release_clean(&mut s, &lane, 25 * MIN);
        beat_all(&mut s, 25 * MIN);
        let acts = s.tick(25 * MIN);
        assert!(acts.iter().any(|a| matches!(a, Action::ParkLive { leases, .. } if leases.contains(&session))), "the session parks for it: {acts:?}");
        release_clean(&mut s, &session, 25 * MIN + 1);
        beat_all(&mut s, 25 * MIN + 2);
        assert_eq!(grants(&s.tick(25 * MIN + 2)), vec![head.clone()]);
        assert_eq!(state(&s, &more).0, "queued");
        // what it was told outlives the grant and a master's restart, and reaches the ledger
        let told = s.get(&head).unwrap().told_ms;
        assert!(told.is_some());
        let mut back = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        for line in s.live_lines() { back.restore(&line, 25 * MIN + 3); }
        let line = back.get(&head).unwrap().ledger_line();
        assert_eq!(makepad_strict_json::parse(line.as_bytes()).unwrap().get("told_ms").and_then(|v| v.as_u64()), told, "{line}");
        invariants(&s);
    }

    /// The W4322 shape (11-08) on box `a` of 16 cores: what runs there as `running` (id, end), a
    /// 15-core head, and a short ask every 30 s of `short_cores` cores that says it runs 1 min
    /// and runs `short_run`. Each lease ends as it ran here, not as it said. When the head starts
    /// (ms), and the shorts granted before it: (when, id).
    fn refill(s: &mut Sched, head: &str, mut running: Vec<(String, u64)>, short_cores: f64, short_prio: Priority, short_run: u64) -> (Option<u64>, Vec<(u64, String)>) {
        const SEC: u64 = 1_000;
        let mut shorts = Vec::new();
        let mut t = 10 * SEC;
        while t <= 40 * 60_000 {
            if t % (30 * SEC) == 0 {
                let mut r = overlay(&format!("short{}", t / SEC), None, short_cores);
                (r.est_min, r.priority) = (Some(1.0), short_prio);
                s.request(r, t);
            }
            for (id, end) in running.clone() {
                if end <= t { release_clean(s, &id, t); }
            }
            running.retain(|(_, end)| *end > t);
            beat_all(s, t);
            for g in grants(&s.tick(t)) {
                if g == head { return (Some(t), shorts); }
                running.push((g.clone(), t + short_run));
                shorts.push((t, g));
            }
            t += 10 * SEC;
        }
        (None, shorts)
    }

    /// A 15-core head keeps the box it holds through short work there running past its estimate,
    /// and starts once that ends: it lost its hold to each overrun, and short asks took each core
    /// the box freed until it had queued [`STARVED_MS`] (W4322, 11-08). The 15 cores stay asked.
    #[test]
    fn a_held_head_keeps_its_box_through_runs_past_their_estimate() {
        const MIN: u64 = 60_000;
        for prio in [Priority::Background, Priority::Interactive] {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
            let mut r = overlay("long", Some("a"), 8.0);
            r.est_min = Some(5.0);
            let long = s.request(r, 0);
            s.tick(0);
            let mut r = overlay("launch", None, 15.0);
            (r.priority, r.est_min) = (Priority::Interactive, Some(10.0));
            let head = s.request(r, 1);
            let (started, shorts) = refill(&mut s, &head, vec![(long, 5 * MIN)], 2.0, prio, 3 * MIN);
            // the box's work ends at 5 min; shorts let in "ending first" at 4 min run to 7
            assert!(started.is_some_and(|t| t <= 7 * MIN + 30_000), "{prio:?}: started at {started:?}, {} shorts first: {shorts:?}", shorts.len());
            assert!(shorts.len() <= 6, "{prio:?}: {shorts:?}");
            assert_eq!(s.get(&head).unwrap().req.cores, Some(15.0));
        }
    }

    /// A box that drains for its head takes backfill by a measure only ([`Sched::measured_run`]):
    /// short asks that said 1 min and ran 3 came in against a launch's last minute, and it
    /// started 2 min after its box's work had ended. A measured class whose run there is past its
    /// measure is a guess again.
    #[test]
    fn a_draining_box_takes_backfill_by_a_measure_only() {
        const MIN: u64 = 60_000;
        let launch = |s: &mut Sched| {
            let mut r = overlay("launch", None, 15.0);
            (r.priority, r.live, r.est_min) = (Priority::Interactive, true, Some(10.0));
            s.request(r, 1)
        };
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = overlay("long", Some("a"), 8.0);
        (r.est_min, r.priority) = (Some(5.0), Priority::Interactive);
        let long = s.request(r, 0);
        s.tick(0);
        let head = launch(&mut s);
        let (started, shorts) = refill(&mut s, &head, vec![(long, 5 * MIN)], 2.0, Priority::Background, 3 * MIN);
        assert!(started.is_some_and(|t| t <= 5 * MIN + 30_000), "started at {started:?}: {shorts:?}");
        assert!(shorts.iter().all(|(t, _)| *t <= LIVE_STARVED_MS), "nothing by a declared run once it drains: {shorts:?}");
        // the box measured busy past its charges, and shorts that run as they say: it starts as
        // the box's work ends
        let mut busy = caps(16, 64.0, Role::Pool);
        busy.cpu_pct = Some(50.0);
        let mut s = sched(&[("a", busy)]);
        let mut r = overlay("long", Some("a"), 1.0);
        (r.est_min, r.priority) = (Some(5.0), Priority::Interactive);
        let long = s.request(r, 0);
        s.tick(0);
        let head = launch(&mut s);
        let (started, shorts) = refill(&mut s, &head, vec![(long, 5 * MIN)], 1.0, Priority::Background, MIN);
        assert!(started.is_some_and(|t| t <= 5 * MIN + 30_000), "started at {started:?}: {shorts:?}");
        // a measured class backfills a draining box, but not while a run of it is past its measure
        // (on any box): it is "measured, exceeded", a guess again
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        for _ in 0..3 { s.record_node_cost("test short", "test", "a", MIN); }
        let short = |s: &mut Sched, node: &str, at: u64| {
            let mut r = overlay("short", Some(node), 2.0);
            r.class = "test short".into();
            s.request(r, at)
        };
        let mut r = overlay("long", Some("a"), 8.0);
        (r.est_min, r.priority) = (Some(5.0), Priority::Interactive);
        let long = s.request(r, 0);
        let over = short(&mut s, "b", 0);
        s.tick(0);
        let mut r = overlay("launch", Some("a"), 15.0);
        (r.priority, r.live, r.est_min) = (Priority::Interactive, true, Some(10.0));
        let head = s.request(r, 1);
        beat_all(&mut s, 2 * MIN);
        s.tick(2 * MIN);
        assert!(s.drains_box(s.get(&head).unwrap()));
        let next = short(&mut s, "a", 2 * MIN);
        beat_all(&mut s, 2 * MIN);
        s.tick(2 * MIN);
        assert_eq!(node_of(&s, &next), None, "its class's run on b is past its measure: {:?}", state(&s, &next));
        release_clean(&mut s, &over, 2 * MIN + 10_000);
        beat_all(&mut s, 2 * MIN + 10_000);
        s.tick(2 * MIN + 10_000);
        assert_eq!(node_of(&s, &next).as_deref(), Some("a"), "{:?}", state(&s, &next));
        release_clean(&mut s, &next, 3 * MIN);
        release_clean(&mut s, &long, 5 * MIN);
        beat_all(&mut s, 5 * MIN);
        assert_eq!(grants(&s.tick(5 * MIN)), vec![head.clone()]);
        invariants(&s);
    }

    /// A head's start as told ([`Sched::age_heads`]) passed twice: the box drains for it, as for
    /// one that waited [`STARVED_MS`], and its waiters read why.
    #[test]
    fn a_head_whose_start_passed_twice_drains_its_box() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = overlay("run", Some("a"), 8.0);
        r.est_min = Some(2.0);
        let run = s.request(r, 0);
        s.tick(0);
        let mut r = overlay("head", None, 12.0);
        r.est_min = Some(10.0);
        let head = s.request(r, 1);
        beat_all(&mut s, 1);
        s.tick(1);
        assert_eq!(s.holds.get("a").map(|h| h.id.clone()), Some(head.clone()));
        // its start at 2 min passes, then the next one told (the run counted as starting over)
        let mut t = 10_000;
        while !s.aged(s.get(&head).unwrap()) && t < 9 * MIN {
            beat_all(&mut s, t);
            s.tick(t);
            t += 10_000;
        }
        assert!(t > 4 * MIN && t < 9 * MIN, "aged at {t}: {:?}", s.promised.get(&head));
        assert!(s.drains_box(s.get(&head).unwrap()));
        // (5 cores: no room beside its 12 cores, and no measure to end first by)
        let mut r = overlay("more", None, 5.0);
        r.est_min = Some(30.0);
        let more = s.request(r, t);
        beat_all(&mut s, t);
        s.tick(t);
        assert_eq!(node_of(&s, &more), None, "{:?}", state(&s, &more));
        assert!(state(&s, &more).1.contains(&format!("held for {head} (")) && state(&s, &more).1.ends_with("; its start passed 2 times: the box drains for it"), "{:?}", state(&s, &more));
        release_clean(&mut s, &run, t + 1);
        beat_all(&mut s, t + 1);
        assert_eq!(grants(&s.tick(t + 1)), vec![head.clone()]);
        invariants(&s);
    }

    /// A head keeps its box through a run there past its estimate, but not for hours: another box
    /// that starts it [`HOLD_MOVE_MS`] sooner takes its hold (a sweep measured at 26 min ran 3 h).
    #[test]
    fn a_held_head_moves_off_a_run_far_past_its_estimate() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
        let mut r = overlay("sweep", Some("a"), 8.0);
        r.est_min = Some(26.0);
        let sweep = s.request(r, 0);
        let mut r = overlay("build", Some("b"), 8.0);
        r.est_min = Some(40.0);
        let build = s.request(r, 0);
        s.tick(0);
        let mut r = overlay("head", None, 15.0);
        r.est_min = Some(10.0);
        let head = s.request(r, 1);
        beat_all(&mut s, 1);
        s.tick(1);
        let held = |s: &Sched| s.holds.iter().find(|(_, h)| h.id == head).map(|(n, _)| n.clone());
        assert_eq!(held(&s).as_deref(), Some("a"));
        // past its 26 min the sweep still holds the head's box; counted as starting over, b frees
        // 13 min sooner
        let mut t = MIN;
        while t <= 27 * MIN {
            beat_all(&mut s, t);
            s.tick(t);
            if t < 26 * MIN { assert_eq!(held(&s).as_deref(), Some("a"), "at {t}: {:?}", state(&s, &head)); }
            t += MIN;
        }
        assert_eq!(held(&s).as_deref(), Some("b"), "{:?}", state(&s, &head));
        release_clean(&mut s, &build, 40 * MIN);
        beat_all(&mut s, 40 * MIN);
        assert_eq!(grants(&s.tick(40 * MIN)), vec![head.clone()]);
        assert_eq!(node_of(&s, &head).as_deref(), Some("b"));
        assert_eq!(node_of(&s, &sweep).as_deref(), Some("a"));
        invariants(&s);
    }

    /// A live session ends only when it is parked: background work never backfills a whole-box
    /// ask's box against its estimate, and the ask parks it once the other work there ends. Work
    /// let in on a session's 30 min kept a non-live lease on the box, so the session never parked
    /// and the next small ask came in on the same estimate (11-08).
    #[test]
    fn background_work_never_backfills_against_a_live_session_a_whole_ask_parks() {
        const MIN: u64 = 60_000;
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let mut r = overlay("session", Some("a"), 4.0);
        r.priority = Priority::Interactive;
        r.live = true;
        r.est_min = Some(30.0);
        let session = s.request(r, 0);
        let mut r = overlay("build", Some("a"), 4.0);
        r.est_min = Some(2.0);
        let build = s.request(r, 0);
        s.tick(0);
        let mut r = overlay("flakes", None, 4.0);
        r.whole = true;
        r.est_min = Some(15.0);
        let whole = s.request(r, 1);
        beat_all(&mut s, 1);
        s.tick(1);
        assert_eq!(s.holds["a"].id, whole, "{:?}", state(&s, &whole));
        // ends after the build, before the session's estimate: it would keep the session from parking
        let mut r = overlay("quick", None, 4.0);
        r.est_min = Some(5.0);
        let quick = s.request(r, 2);
        beat_all(&mut s, 2);
        assert!(grants(&s.tick(2)).is_empty(), "{:?}", state(&s, &quick));
        assert!(state(&s, &quick).1.contains(&format!("held for whole {whole}")), "{:?}", state(&s, &quick));
        release_clean(&mut s, &build, 2 * MIN);
        beat_all(&mut s, 2 * MIN);
        let acts = s.tick(2 * MIN);
        assert!(acts.iter().any(|a| matches!(a, Action::ParkLive { leases, .. } if leases.contains(&session))), "the session parks for it: {acts:?}");
        assert_eq!(state(&s, &quick).0, "queued", "{:?}", state(&s, &quick));
        invariants(&s);
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
        assert_eq!(s.holds.get("node-b").map(|h| h.id.clone()), Some(ci.clone()), "the next head holds the box");
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
        s.nodes.get_mut("b").unwrap().caps.as_mut().unwrap().features.clear();
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
        // not; a queued interactive lease claims a bench box only by name (devhub's own)
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
        let mut named = devhubs(overlay("named", Kind::Test));
        named.node = Some("bench1".into());
        named.priority = Priority::Interactive;
        let named = s.request(named, 2);
        let held = s.holds.clone();
        s.refresh_interactive(&held);
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

    /// A box whose WM shows nothing (its window 0x0) takes no screen or gui lease: one pinned
    /// there says why, one that is not goes elsewhere, and work without windows still goes
    /// there; `devhub nodes` says so. Fourteen gui and screen leases timed out on their first
    /// frame on such a node in 3 h (11-07).
    #[test]
    fn a_box_whose_wm_shows_nothing_takes_no_screen_or_gui_work() {
        let shown = |mut c: Caps| { c.display = Some("wm:makepad-wm-0".into()); c };
        let mut dark = shown(caps(16, 64.0, Role::Pool));
        dark.wm_dark = "its WM's window is 0x0: no output shows it".into();
        let mut s = sched(&[("a", dark), ("b", shown(caps(16, 64.0, Role::Pool)))]);
        let pinned = s.request(req("lane-a", Kind::Screen, Some("a")), 0);
        let screen = s.request(req("lane-b", Kind::Screen, None), 0);
        let mut gui = req("lane-c", Kind::Test, None);
        gui.needs = vec![("gui".into(), "1".into())];
        let gui = s.request(gui, 0);
        let plain = s.request(req("lane-d", Kind::Test, Some("a")), 0);
        s.tick(0);
        assert!(state(&s, &pinned).1.contains("no display (its WM's window is 0x0"), "{:?}", state(&s, &pinned));
        assert_eq!(node_of(&s, &screen).as_deref(), Some("b"), "{:?}", state(&s, &screen));
        assert_ne!(node_of(&s, &gui).as_deref(), Some("a"), "{:?}", state(&s, &gui));
        assert_eq!(node_of(&s, &plain).as_deref(), Some("a"), "work without windows still goes there: {:?}", state(&s, &plain));
        let line = s.node_view(0).into_iter().find(|v| v.name == "a").unwrap().caps_line();
        assert!(line.contains("no display (its WM's window is 0x0: no output shows it)"), "{line}");
    }

    #[test]
    fn a_box_holding_the_jit_wins_while_it_frees_before_a_build_and_hot_demand_is_revisited() {
        // `a` holds the dev JIT and runs a conflicting test measured at `est` min; `b` is free
        // and would build the JIT first (90 s by default)
        let setup = |est: f64| {
            let mut s = sched(&[("a", caps(16, 64.0, Role::Pool)), ("b", caps(16, 64.0, Role::Pool))]);
            let mut r = req("running", Kind::Test, Some("a"));
            r.est_min = Some(est);
            // (the whole box: an overlay shares no tree to wait on)
            r.cores = Some(16.0);
            let running = s.request(r, 0);
            s.tick(0);
            s.nodes.get_mut("a").unwrap().caps.as_mut().unwrap().jit_store = jit_store(&["cached"]);
            s.nodes.get_mut("b").unwrap().caps.as_mut().unwrap().jit_store = jit_store(&[]);
            let mut r = req("next", Kind::Test, None);
            (r.src, r.hint) = (Src::Overlay, vec!["cached".into()]);
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
            a.jit_store = jit_store(&["cached"]);
            s.nodes.get_mut("b").unwrap().caps.as_mut().unwrap().jit_store = jit_store(&[]);
            let mut r = req("next", Kind::Test, None);
            (r.src, r.hint) = (Src::Overlay, vec!["cached".into()]);
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
        assert_eq!(s.holds["hot"].until, HOLD_MS + 2, "renewed each tick its ask waits");
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
        assert!(!crate::os::lease_argv(vec!["x".into()], Some("u"), 10, "", Some(4), None).iter().any(|a| a.starts_with("MemoryMax")));
        assert_eq!(crate::os::lease_argv(vec!["x".into()], Some("u"), 10, "", Some(4), Some(2.0)).iter().any(|a| a == "MemoryMax=3G"), cfg!(target_os = "linux"));
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

    /// A live session's calls are charged its class's metered peak cores, its launch apart, and
    /// its why says so: granted at its launches' p90 (a filtered class at its suite's), charged its
    /// calls' p90 in a call once one ended, what it measures between calls. Work that fits beside its charge
    /// goes in; work that would run its box past its cores waits. Sessions charged a few cores in a call compiled on most of their box, and beside a
    /// tests shard their boxes ran well over their cores (11-08).
    #[test]
    fn a_live_sessions_calls_are_charged_their_metered_peak() {
        let mut s = sched(&[("box-a", caps(16, 64.0, Role::Pool))]);
        let mut r = req("livex", Kind::Test, Some("box-a"));
        (r.live, r.class, r.src, r.rev) = (true, "live calc?one_test".into(), Src::Overlay, Some(format!("work@{}", "a".repeat(40))));
        let note = |s: &Sched, id: &str| s.allocation_note("box-a", s.get(id).unwrap());
        let plain = s.request(req("plain", Kind::Test, Some("box-a")), 0);
        let unmetered = s.request(r.clone(), 0);
        assert!(note(&s, &unmetered).ends_with("; its calls are charged their class's peak cores: launch none metered, call none metered"), "{}", note(&s, &unmetered));
        assert_eq!(s.cost_on("box-a", s.get(&unmetered).unwrap()).cores, 4.0, "none metered: its kind's");
        assert!(!note(&s, &plain).contains("its calls"), "{}", note(&s, &plain));
        s.release(&unmetered, "released", 0);
        s.release(&plain, "released", 0);
        s.record_call("live calc", "test", true, 11.2);
        for peak in [1.2, 5.6, 2.0] { s.record_call("live calc", "test", false, peak); }
        let live = s.request(r, 1);
        assert!(note(&s, &live).starts_with("ram 6.0 GB (kind); cores 12 (its class's launch peak p90 11.2)"), "{}", note(&s, &live));
        assert!(note(&s, &live).ends_with("; its calls are charged their class's peak cores: launch p90 11.2 of 1, call p90 5.6 of 3"), "{}", note(&s, &live));
        assert_eq!(grants(&s.tick(1)), vec![live.clone()]);
        s.started(&live, "{}");
        // in its launch: 12 of 16 cores; 4 more fit beside it, 5 do not
        let cores = |s: &mut Sched, lane: &str, n: f64, at: u64| s.request(overlay(lane, Some("box-a"), n), at);
        let five = cores(&mut s, "five", 5.0, 2);
        assert!(grants(&s.tick(2)).is_empty());
        assert!(state(&s, &five).1.contains("in use 12"), "{:?}", state(&s, &five));
        s.release(&five, "released", 2);
        let four = cores(&mut s, "four", 4.0, 3);
        assert_eq!(grants(&s.tick(3)), vec![four.clone()]);
        s.started(&four, "{}");
        // its launch ended: between calls what it measures (nothing yet), and no room kept for its
        // next call: 4 + 0 + 12
        s.live_called.insert(live.clone());
        s.live_idle.insert(live.clone());
        assert_eq!(s.charged(s.get(&live).unwrap()).cores, 0.0);
        let thirteen = cores(&mut s, "thirteen", 13.0, 4);
        assert!(grants(&s.tick(4)).is_empty(), "past the box's cores");
        s.release(&thirteen, "released", 4);
        let twelve = cores(&mut s, "twelve", 12.0, 5);
        assert_eq!(grants(&s.tick(5)), vec![twelve.clone()]);
        assert_eq!(s.live_room(&live), Ok(()), "its call goes ahead of them");
        s.live_idle.remove(&live);
        assert_eq!(s.charged(s.get(&live).unwrap()).cores, 6.0, "in a call: its calls' peak");
        invariants(&s);
    }

    /// A class's cores are charged from its first measured run on, and a filtered run's from its
    /// unfiltered class while it has none: a fuzz test measured once at 6.4 cores went in at its
    /// kind's 4 beside a tests shard's hold, ran at 11, and the shard waited 9 min (11-07).
    #[test]
    fn a_class_measured_once_is_charged_its_cores_beside_a_shards_hold() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let running = s.request(overlay("run", Some("a"), 8.0), 0);
        s.tick(0);
        let mut shard = overlay(TESTS_LANE, Some("a"), 12.0);
        shard.est_min = Some(20.0);
        shard.priority = Priority::Background;
        let shard = s.request(shard, 1);
        s.tick(1);
        let mut fuzz = overlay("fuzz", Some("a"), 4.0);
        fuzz.cores = None;
        fuzz.est_min = Some(30.0);
        fuzz.class = "native:test:pkg:fuzz?every_value".into();
        let unmeasured = s.request(fuzz.clone(), 2);
        assert_eq!(s.cost_on("a", s.get(&unmeasured).unwrap()).cores, 4.0, "no run measured: its kind's");
        s.cancel(&unmeasured, 2);
        s.record_resources("native:test:pkg:fuzz", "test", None, Some(7.9));
        assert_eq!(s.cost_on("a", s.get(&shard).unwrap()).cores, 12.0, "an asked count stands");
        let filtered = s.request(fuzz.clone(), 3);
        assert_eq!(s.cost_on("a", s.get(&filtered).unwrap()).cores, 8.0, "its unfiltered class's one run");
        s.record_resources(&fuzz.class, "test", None, Some(6.4));
        assert_eq!(s.cost_on("a", s.get(&filtered).unwrap()).cores, 7.0, "its own class's one run");
        s.tick(3);
        assert!(state(&s, &filtered).1.contains(&format!("held for tests shard {shard}")), "7 does not fit beside 12 of 16: {:?}", state(&s, &filtered));
        release_clean(&mut s, &running, 4);
        beat_all(&mut s, 5);
        s.tick(5);
        assert_eq!(node_of(&s, &shard).as_deref(), Some("a"), "{:?}", state(&s, &shard));
    }

    /// A lane the user put first (`devhub lane first`, 11-07: Source Library work first) is
    /// interactive, heads the queue past an older ask, and takes a box another lane's lease
    /// holds, where that lane's own long ask stays held off.
    #[test]
    fn a_lane_put_first_heads_the_queue_and_takes_a_held_box() {
        let mut s = sched(&[("a", caps(16, 64.0, Role::Pool))]);
        let busy = s.request(overlay("busy", Some("a"), 16.0), 0);
        s.tick(0);
        let older = s.request(overlay("older", None, 10.0), 1);
        let first = s.request(overlay("sl", None, 10.0), 2);
        s.first_lanes.insert("sl".into());
        s.tick(3);
        assert_eq!(s.get(&first).unwrap().priority, Priority::Interactive);
        release_clean(&mut s, &busy, 4);
        assert_eq!(grants(&s.tick(4)), vec![first.clone()], "the lane put first before the older ask: {:?}", state(&s, &older));
        // (the older ask leaves: holding the box, it would let the long ask in beside it)
        assert!(s.cancel_by(&older, "older", 4));
        let mut whole = req("whole", Kind::Test, Some("a"));
        whole.whole = true;
        let whole = s.request(whole, 5);
        s.tick(5);
        assert_eq!(s.holds.get("a").map(|h| h.id.clone()), Some(whole.clone()), "{:?}", state(&s, &whole));
        let ask = |lane: &str| {
            let mut r = overlay(lane, None, 4.0);
            r.est_min = Some(60.0);
            r
        };
        let long = s.request(ask("long"), 6);
        let next = s.request(ask("sl"), 6);
        assert_eq!(grants(&s.tick(6)), vec![next], "{:?}", state(&s, &long));
        invariants(&s);
    }

    /// An open-ended session asks the cores its class's sessions use at the median and is the
    /// class it asked, whatever its wall; a finite run of the same measures keeps their p90 and
    /// its wall's class (L615639, 11-07: [`Sched::measured_cores`], [`Sched::classify`]).
    #[test]
    fn a_session_asks_its_class_median_cores_and_stays_small() {
        let mut s = sched(&[("node-a", caps(16, 64.0, Role::Pool))]);
        let ask = |kind: Kind, live: bool| {
            let mut r = overlay("lane-a", None, 1.0);
            (r.kind, r.live, r.cores, r.cost, r.class) = (kind, live, None, CostClass::default_for(kind), "fab".to_string());
            r
        };
        for kind in ["screen", "test"] {
            for cores in [1.2, 5.1, 0.3, 1.6, 1.4, 24.3, 1.0] { s.record_resources("fab", kind, None, Some(cores)); }
            for _ in 0..3 { s.record_node_cost("fab", kind, "node-a", 793_000); }
        }
        let session = s.request(ask(Kind::Screen, true), 0);
        let l = s.get(&session).unwrap();
        assert_eq!(s.cost_on("node-a", l).cores, 2.0, "the median 1.4 of its class's sessions, not their highest");
        assert!(l.cost_class == CostClass::Small && matches!(l.cost_source, CostSource::Measured(793_000)), "small, its wall its ETA: {:?} {:?}", l.cost_class, l.cost_source);
        assert!(s.allocation_note("node-a", l).contains("cores 2 (measured median 1.4)"), "{}", s.allocation_note("node-a", l));
        let run = s.request(ask(Kind::Test, false), 0);
        let l = s.get(&run).unwrap();
        assert_eq!(s.cost_on("node-a", l).cores, 16.0, "a finite run: its class's p90, the most of seven, capped at the box");
        assert_eq!(l.cost_class, CostClass::Medium, "classed by its wall");
        let mut asked = ask(Kind::Screen, true);
        asked.cores = Some(6.0);
        let asked = s.request(asked, 0);
        assert_eq!(s.cost_on("node-a", s.get(&asked).unwrap()).cores, 6.0, "`live start --cores` asks its own");
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

    /// An ask is charged its class's measured cores on either side of it, so its box takes in what
    /// it uses, and runs on its ask, so the measure cannot climb or sink on the charge: hot buckets
    /// asked 3 cores, and most of their runs used more (11-08); tests shards asked 12 and used 8.8
    /// at p90, and the land queue's own work waited at "budget cores" for the rest (11-10). Started
    /// on fewer than its charge (its box's free cores), it runs on those.
    #[test]
    fn an_ask_is_charged_its_class_measure_and_runs_on_its_ask() {
        let mut s = sched(&[("node-a", caps(12, 64.0, Role::Pool))]);
        let hot = |lane: &str, cores: f64| {
            let mut r = overlay(lane, Some("node-a"), cores);
            r.class = "hot".into();
            r
        };
        for _ in 0..3 { s.record_resources("hot", "test", None, Some(6.1)); }
        let first = s.request(hot("hot-a", 3.0), 0);
        let second = s.request(hot("hot-b", 3.0), 0);
        assert_eq!(s.cost_on("node-a", s.get(&first).unwrap()).cores, 7.0);
        let note = s.allocation_note("node-a", s.get(&first).unwrap());
        assert!(note.ends_with("; cores 7 (asked 3, charged its class's measured p90 6.1; it runs on 3)"), "{note}");
        assert_eq!(grants(&s.tick(0)), vec![first.clone()], "two of 7 overrun the box's 12; two asks of 3 would not");
        let l = s.get(&first).unwrap();
        assert_eq!((l.cost.cores, l.run_cores()), (7.0, 3.0), "under its measure: charged the measure, run on its ask");
        assert_eq!(state(&s, &second).0, "queued");
        let over = s.request(hot("hot-c", 10.0), 1);
        let l = s.get(&over).unwrap();
        assert_eq!(s.cost_on("node-a", l).cores, 7.0, "over its measure: charged the measure");
        assert!(s.allocation_note("node-a", l).ends_with("; cores 7 (asked 10, charged its class's measured p90 6.1; it runs on 10)"), "{}", s.allocation_note("node-a", l));
        let big = s.request(hot("hot-d", 13.0), 1);
        assert_eq!(s.cost_on("node-a", s.get(&big).unwrap()).cores, 13.0, "an ask over its box's cores stays its own");

        // a tests shard asking 12 that uses 8.8 goes in beside 7 cores of other work on 16, and
        // runs its 12; one that waited for its box starts on what is free and runs on that
        let min = 60_000;
        for (busy, run) in [(7.0, 12.0), (8.0, 8.0)] {
            let mut s = sched(&[("pool-a", caps(16, 64.0, Role::Pool))]);
            for _ in 0..3 { s.record_resources("shard", "test", None, Some(8.8)); }
            let lane = s.request(overlay("lane", Some("pool-a"), busy), 0);
            assert_eq!(grants(&s.tick(0)), vec![lane]);
            let mut shard = req(TESTS_LANE, Kind::Test, None);
            (shard.cores, shard.class) = (Some(12.0), "shard".into());
            let shard = s.request(shard, 1);
            if run < 12.0 {
                assert!(grants(&s.tick(1)).is_empty(), "9 beside {busy}: no room");
                beat_all(&mut s, 10 * min + 1);
                assert_eq!(grants(&s.tick(10 * min + 1)), vec![shard.clone()], "{:?}", state(&s, &shard));
            } else {
                assert_eq!(grants(&s.tick(1)), vec![shard.clone()], "9 beside {busy} fit 16: {:?}", state(&s, &shard));
            }
            let l = s.get(&shard).unwrap();
            assert_eq!((l.cost.cores, l.run_cores()), (16.0 - busy, run), "charged beside {busy}, its run's cores");
            invariants(&s);
        }
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
                let mut cleanup: Vec<_> = s.restoring.iter().map(|(id, (n, ..))| (id.clone(), n.clone())).collect();
                cleanup.sort(); // HashMap iteration must not change the seeded scenario.
                for (id, name) in cleanup { if next(&mut seed, 2) == 0 { s.cleaned(&name, &id, 1); } }
                for i in 0..roles.len() {
                    let name = (105 + i).to_string();
                    let n = s.nodes.get_mut(&name).unwrap();
                    n.up = next(&mut seed, 5) != 0;
                    n.dev = Some(next(&mut seed, 5) != 0);
                    let c = n.caps.as_mut().unwrap();
                    c.features = if next(&mut seed, 8) == 0 { Vec::new() } else { super::super::FEATURES.iter().map(|f| f.to_string()).collect() };
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
