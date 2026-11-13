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

/// Cost samples parsed without I/O. Applying a batch preserves the master's learning order.
#[derive(Default)]
pub(super) struct CostRecords {
    jit_peaks: Vec<(Option<String>, f64)>,
    calls: Vec<(String, String, bool, f64)>,
    memory: Vec<(String, String, String, Option<String>, Option<f64>)>,
    records: Vec<(String, String, String, String, Option<String>, u64, Option<f64>, Option<f64>, Option<u64>)>,
    jit_builds: Vec<(String, f64)>,
    /// A work tree's runs: (tree lane, box, rev, class, kind) ([`Sched::note_tree_run`])
    trees: Vec<(String, String, String, String, String)>,
}

/// A lease measures its JIT build's cores when that build was at least this long, and
/// [`JIT_BUILD_SHARE`] of its wall: its CPU over its wall is then the build's.
const JIT_BUILD_MIN_MS: f64 = 30_000.0;
const JIT_BUILD_SHARE: f64 = 0.7;
/// Below this many cores a long `jit` phase waited on another lease's build of that JIT (the
/// box's hub builds each once): about half of them, at under a core (11-12).
const JIT_BUILD_MIN_CORES: f64 = 2.0;

impl CostRecords {
    pub(super) fn push(&mut self, v: &makepad_strict_json::Value) {
        // (records from before 11-06 say no OS family: they weigh every box's peaks only)
        let os = |v: &makepad_strict_json::Value| v.get("os").and_then(|o| o.as_str()).filter(|o| !o.is_empty()).map(str::to_string);
        let lane = |v: &makepad_strict_json::Value| v.get("lane").and_then(|o| o.as_str()).unwrap_or("").to_string();
        // what a lease's JIT build held, however the lease ended ([`Sched::jit_build_ram`])
        if let Some(peak) = v.get("jit_peak_gb").and_then(super::num) { self.jit_peaks.push((os(&v), peak)); }
        // A memory end measures only RAM: its wall, CPU and run time were cut short. Not a
        // script's (`bash x.sh @lane`, the land queue's `land_gate.sh` too): its class is a
        // script's name, and what ran away in it one of the things it runs, not what its next run
        // needs. A lane's script that ran away to 49.9 GB had every later run of it charged a pool
        // box's whole RAM budget, 1.7 GB of it held, and the box's drain head waited behind that
        // (L921313, L926707); a tests bucket's runaway test at 45 GB weighed every later shard at
        // 54 GB, a whole box each (L924561, 11-13).
        if v.get("how").and_then(|h| h.as_str()) == Some("memory") {
            if let Some(class) = v.get("class").and_then(|v| v.as_str()).filter(|c| !super::lane_script(c)) {
                let kind = v.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                self.memory.push((class.to_string(), kind.to_string(), lane(&v), os(&v), v.get("peak_ram_gb").and_then(super::num)));
            }
        }
        // a live call's sample ([`LeaseMaster::ledger_call`]): no wall of its lease's
        if v.get("how").and_then(|h| h.as_str()) == Some("call") {
            if let (Some(class), Some(peak)) = (v.get("class").and_then(|v| v.as_str()), v.get("peak_cores").and_then(super::num)) {
                self.calls.push((class.to_string(), v.get("kind").and_then(|v| v.as_str()).unwrap_or("").to_string(), v.get("launch").and_then(|v| v.as_bool()).unwrap_or(false), peak));
            }
            return;
        }
        // a work tree's run, however it ended: the rev its tree's JIT is at on its box
        if let (Some(lane), Some(rev)) = (v.get("tree_lane").and_then(|x| x.as_str()), v.get("tree_rev").and_then(|x| x.as_str())) {
            let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            self.trees.push((lane.to_string(), s("node"), rev.to_string(), s("class"), s("kind")));
        }
        // only a run its holder released measures its class: a cancelled one's wall is how
        // long it was let hang (a story test cancelled after 51 min became its class's
        // measure on its box, and the next hang's limit twice that, 11-04). JIT build
        // records and the ones from before then say no `how`. Nor does a released one that
        // hung or failed (its last run did not end by itself: a crash, a kill, no verdict;
        // or it stopped a unit as hung): it measures neither time, cores nor RAM.
        if v.get("how").and_then(|h| h.as_str()).is_some_and(|h| h != "released") { return; }
        if v.get("unmeasured").and_then(|a| a.as_str()).is_some() { return; }
        if let (Some(class), Some(wall)) = (v.get("class").and_then(|v| v.as_str()), v.get("wall_ms").and_then(super::num)) {
            if wall.is_finite() && wall > 0.0 {
                let kind = v.get("kind").and_then(|v| v.as_str()).unwrap_or("");
                let peak = v.get("peak_ram_gb").and_then(super::num);
                let cores = v.get("cpu_seconds").and_then(super::num).map(|cpu| cpu * 1000.0 / wall);
                let run = v.get("run_ms").and_then(super::num).filter(|ms| ms.is_finite() && *ms > 0.0).map(|ms| ms as u64);
                // a lease whose wall was its own JIT build measures that build's cores on its OS family
                // ([`Sched::record_jit_build_cores`])
                let jit = v.get("jit_ms").and_then(super::num).unwrap_or(0.0);
                if let (Some(os), Some(cores)) = (os(&v), cores) {
                    // (a build told its jobs, admitted on fewer cores than the measure, is no measure of it)
                    let told = v.get("jit_jobs").and_then(super::num).is_some();
                    if !told && jit >= JIT_BUILD_MIN_MS && jit >= JIT_BUILD_SHARE * wall && cores >= JIT_BUILD_MIN_CORES { self.jit_builds.push((os, cores)); }
                }
                self.records.push((class.to_string(), kind.to_string(), v.get("node").and_then(|v| v.as_str()).unwrap_or("").to_string(), lane(&v), os(&v), wall as u64, peak, cores, run));
            }
        }
    }

    pub(super) fn apply(self, sched: &mut Sched) {
        for (os, peak) in self.jit_peaks {
            sched.record_resources("jit-build", "", Some(peak), None);
            if let Some(os) = os { sched.record_family_peak(&os, "jit-build", "", Some(peak)); }
        }
        for (class, kind, launch, peak) in self.calls {
            sched.record_call(&class, &kind, launch, peak);
        }
        for (class, kind, lane, os, peak) in self.memory {
            sched.record_resources(&class, &kind, peak, None);
            if let Some(os) = os {
                sched.record_family_peak(&os, &class, &kind, peak);
                sched.record_lane_peak(&os, &lane, &class, &kind, peak);
            }
        }
        for (class, kind, node, lane, os, wall, peak, cores, run) in self.records {
            sched.record_node_cost(&class, &kind, &node, wall);
            sched.record_resources(&class, &kind, peak, cores);
            if let Some(os) = os {
                sched.record_family_peak(&os, &class, &kind, peak);
                sched.record_lane_peak(&os, &lane, &class, &kind, peak);
            }
            if let Some(run) = run { sched.record_run(&class, &kind, run); }
        }
        for (os, cores) in self.jit_builds {
            sched.record_jit_build_cores(&os, cores);
        }
        // (after its walls: a warm run's class is a JIT run's when its `+jit-build` was measured)
        for (lane, node, rev, class, kind) in self.trees {
            sched.note_tree_run(&lane, &node, &rev, &class, &kind);
        }
    }
}

/// `(node, service, line)` -> the answer lines (without the end line).
pub type Ask = Arc<dyn Fn(&str, &str, &str) -> Result<Vec<String>, String> + Send + Sync>;
pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;
/// `(node, repo, repo dir, branch, fresh)` -> that commit, present in the node's store; `fresh`:
/// asks the node again even when this master sent it the commit a moment ago.
pub type EnsureRev = Arc<dyn Fn(&str, &str, &Path, &super::RevPin, bool) -> Result<String, String> + Send + Sync>;
/// Sends `node` objects of a repository (by name, at a dir) its store lacks ([`super::NO_OBJECTS`]).
pub type FillObjects = Arc<dyn Fn(&str, &str, &Path, &[String]) -> Result<(), String> + Send + Sync>;
/// The nodes the master dials: (name, address).
pub type Hosts = Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>;
/// The worktree the lane registry holds for a live lane (`devhub lane new|adopt`), by name.
pub type LaneTrees = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub const SERVICES: [&str; 21] = ["lease", "agent-seen", "retarget", "beat", "release", "cancel", "leases", "why-queued", "nodes", "node-dev", "linspect", "evict", "stop", "unblock", "override", "blocked", "jit", "stats", "prof", "native", super::panel::SERVICE];
/// How long a commit sent to a node's store counts as there without asking again.
#[cfg(feature = "checkout")]
const ENSURED_FOR: std::time::Duration = std::time::Duration::from_secs(600);

/// How often the master beats its leases on the nodes and asks the nodes' caps. A node ends a
/// lease the master, there, has not beaten for its no-beat limit ([`super::exec::Opts::no_beat`]).
#[derive(Clone, Copy, Debug)]
pub struct Pace {
    pub beat: Duration,
    pub caps: Duration,
}

impl Pace {
    /// The fleet's: beats every 5 s, caps every 10 s, to nodes that end a lease after 60 s.
    pub const FLEET: Pace = Pace { beat: Duration::from_secs(5), caps: Duration::from_secs(10) };
}

/// What an inspect lease left on a JIT (undone on the node at `lease-end`).
#[derive(Clone, Debug, Default)]
struct Marks {
    pid: u32,
    breaks: Vec<String>,
    probes: Vec<String>,
    paused: bool,
}

/// The command sent to a hub after its revisions are available there.
pub(super) fn lease_start_line(l: &Lease, rev: &str, weighed: f64, peak: Option<f64>) -> String {
    let needs: Vec<String> = l.req.needs.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
    let warms = l.req.warms.as_deref().map_or(String::new(), |lane| format!(",\"warms\":{}", json_str(lane)));
    let warms = match l.req.cpu_target.as_deref() { Some(t) => format!("{warms},\"cpu_target\":{}", json_str(t)), None => warms };
    let peak = peak.map_or("null".to_string(), |g| g.to_string());
    // (only a hub with `gpu-quiet` takes a GPU timing, [`super::sched::Req::required`])
    let warms = if l.req.gpu { format!("{warms},\"gpu\":true") } else { warms };
    // (only to a hub with `jit-jobs`: [`super::sched::Sched::jit_jobs`])
    let peak = match l.jit_jobs { Some(jobs) => format!("{peak},\"jit_jobs\":{jobs}"), None => peak };
    format!("lease-start {{\"id\":{},\"lane\":{}{warms},\"kind\":\"{}\",\"src\":\"{}\",\"priority\":\"{}\",\"loader\":{},\"cost\":{{\"cores\":{},\"ram_gb\":{},\"ram_max_gb\":{},\"weighed_gb\":{weighed},\"peak_gb\":{peak}}},\"live\":{},\"needs\":{{{}}},\"rev\":{rev},\"env\":{{}}}}", json_str(&l.id), json_str(&l.req.lane), l.req.kind.as_str(), l.req.src.as_str(), l.priority.as_str(), l.req.loader.as_deref().map_or("null".to_string(), json_str), l.run_cores(), l.cost.ram_gb, l.req.ram_gb.map_or("null".to_string(), |g| g.to_string()), l.req.live, needs.join(","))
}

pub struct LeaseMaster {
    machine: String,
    /// when this build of the master started (its exec into it), said with its commit
    /// (`devhub nodes`): the check-in showed each hub's version but not the master's, and a
    /// landing's scheduler fix read as running 15 min before the master took it (11-07)
    up_ms: u64,
    /// What keeps this machine's own network from reaching its hubs ([`Self::note_network`]),
    /// said on the master's line of `devhub nodes`
    network: Mutex<Option<String>>,
    /// The lease loop's last look at both clocks (monotonic, wall ms), from its start on
    /// ([`Self::start`]): where the wall ran past the monotonic one, the machine slept
    /// ([`Self::note_sleep`]). None in tests: their clock is the test's.
    awake: Mutex<Option<(std::time::Instant, u64)>>,
    sched: Mutex<Sched>,
    ask: Ask,
    publish: Publish,
    clock: Clock,
    ensure_rev: Option<EnsureRev>,
    /// how a node's store gets objects it lacks (the master's checkout; None in tests)
    fill_objects: Option<FillObjects>,
    home: PathBuf,
    costs_offset: Mutex<u64>,
    profile_writer: Mutex<u64>,
    /// tests: node calls run on the caller's thread
    inline: bool,
    marks: Mutex<HashMap<String, Arc<Mutex<Marks>>>>,
    /// per lane: inspect input calls in the current minute, the minute, when it was last told
    input_calls: Mutex<HashMap<String, (u32, u64, u64)>>,
    /// (node, canonical app pid) -> the lease that holds its inspect effects
    holders: Mutex<HashMap<(String, u32), String>>,
    /// the dev switch each node was last told (a role change on the node moves its default)
    pushed: Mutex<HashMap<String, bool>>,
    /// per node: the lifetime JIT builds and build seconds its store reported last (the ledger gets the deltas)
    jit_seen: Mutex<HashMap<String, (u64, f64)>>,
    /// per node: the end of the last hub build its restart answer said (the ledger gets each new one)
    hub_seen: Mutex<HashMap<String, u64>>,
    /// how each ended lease's cleanup went (`release` waits for it and says so)
    cleanups: Mutex<HashMap<String, String>>,
    cleaned: std::sync::Condvar,
    /// bumped on every change of the live leases (a grant, a start, an end): an in-process waiter
    /// for its grant wakes on it instead of polling
    changes: (Mutex<u64>, std::sync::Condvar),
    /// the last text written to `leases.live` ([`Self::persist`])
    saved: Mutex<String>,
    /// (node, call) of the beat and caps calls in flight: a hub that stops answering holds only
    /// its own call (a node out of memory on 2026-11-02 held one loop over all nodes, and every
    /// node ended its leases after 60 s without a beat)
    calling: Mutex<std::collections::HashSet<(String, &'static str)>>,
    /// lease-ends a node could not take while its hub restarted: (node, line), sent with the
    /// node's next beats until it answers (its new hub took the lease back from its ledger)
    ends_later: Mutex<Vec<(String, String)>>,
    /// the master's own leases a previous master granted ([`Self::end_unfollowed`])
    restored_own: Mutex<Vec<String>>,
    /// per node: the stalls its last caps answer named, and the ones said already ([`Self::say_stalls`])
    stalls: Mutex<HashMap<String, (Vec<String>, Vec<String>)>>,
    /// per node: when its caps last answered, that answer, and whether it is on disk ([`HEARD`]):
    /// a box down since before this master started still says what it was ([`Self::heard`])
    heard: Mutex<HashMap<String, (u64, String, bool)>>,
    /// the lane registry's worktrees ([`from_the_lanes_worktree`]); none: no lane is checked
    lane_trees: Mutex<Option<LaneTrees>>,
    /// per live session: the turn its box's word on its weight is sent in ([`Self::weigh_live`])
    weighing: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// per agent (`DEVHUB_AGENT`): when it last made a devhub call ([`Self::silent_overdue`])
    agents_seen: Mutex<HashMap<String, u64>>,
    /// Detached agents already said, and the last activity each warning followed.
    detached_said: Mutex<HashMap<String, u64>>,
    /// per (box, JIT id): the turn of its copy there ([`Self::copy_jit`]); the next finds it there
    copying: Mutex<HashMap<(String, String), Arc<Mutex<()>>>>,
}

/// A lease's `--jit` copied to its box at its grant ([`LeaseMaster::copy_jit`]).
struct JitCopy {
    id: String,
    from: String,
    bytes: u64,
    ms: u64,
}

/// [`LeaseMaster::silent_overdue`]: a lease this many times over its measure...
const OVERDUE_TIMES: u64 = 5;
/// ...whose agent made no devhub call for this long.
pub const AGENT_SILENT_MS: u64 = 15 * 60_000;
/// A running lease's agent with no call of its own for this long will not wake on it.
pub const DETACHED_MS: u64 = 5 * 60_000;

/// How long a lease's start waits for its box's hub to come back (a restart for its update takes
/// seconds) before the lease goes back to the queue.
const START_GRACE: Duration = Duration::from_secs(45);

/// Less than this between the wall clock's advance and the monotonic clock's is no sleep: a
/// look's own lag, the wall clock's slew.
pub const SLEPT_MIN_MS: u64 = 2_000;

/// What the machine slept between two looks at both clocks: how far the wall clock ran past the
/// monotonic one, which stops while the machine sleeps (macOS and Linux alike) and runs on
/// through a stall of the process; 0 under [`SLEPT_MIN_MS`]. A wall clock set forward reads so
/// too: no silence either.
pub fn slept_ms(wall: u64, mono: Duration) -> u64 {
    let slept = wall.saturating_sub(mono.as_millis() as u64);
    if slept >= SLEPT_MIN_MS { slept } else { 0 }
}

/// A node call that failed because the node's hub could not be reached (not a refusal).
pub fn unreachable(e: &str) -> bool {
    // (a hub that exits for its self-update closes its links mid-call: "connection closed" for a
    // call reading its answer, "channel closed" for a ship writing a chunk; the TERM that stops it
    // reaches the git it runs, in its process group: "signal: 15" while it still answers)
    ["link lost", "cannot reach", "Connection refused", "not connected", "no route to", "connection closed", "channel closed", "Connection reset", "Broken pipe", "signal: 15"].iter().any(|w| e.contains(w))
}

/// A node call's error that says its host is off the network (powered down, its link gone): no
/// host answers the connect, or its TCP stopped answering. A hub restarting for its self-update
/// leaves its host up: the connect is refused, or the call ends "restarting: " (a pool box went with
/// "Host is down", 11-07).
pub fn host_gone(e: &str) -> bool {
    ["Host is down", "No route to host", "Network is unreachable", "Operation timed out", "Connection timed out"].iter().any(|w| e.contains(w))
}

/// Why a node's `caps` call brought no caps: its error, its hub's refusal, or what it said instead.
fn caps_miss(asked: &Result<Vec<String>, String>) -> String {
    match asked {
        Err(e) => e.clone(),
        Ok(lines) => match lines.first() {
            None => "no answer".to_string(),
            Some(l) => makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string)).unwrap_or_else(|| format!("a bad answer: {l}")),
        },
    }
}

/// A lease's pinned pair holds both halves of each two-repo landing or neither
/// ([`crate::orch::job::whole_pair`]): a direct `devhub run`/`test` pins its lane's pair here, past
/// the jobs' and live runs' own check. Unchecked where no workflows run (a test master, a build
/// without the checkout: Scope's agents build this crate so, and #4070 broke them).
#[cfg(feature = "checkout")]
fn whole_pair(req: &Req) -> Result<(), String> {
    match (req.rev.as_deref(), crate::orch::ctx()) {
        (Some(rev), Some(ctx)) => crate::orch::job::whole_pair(ctx, rev),
        _ => Ok(()),
    }
}

#[cfg(not(feature = "checkout"))]
fn whole_pair(_: &Req) -> Result<(), String> {
    Ok(())
}

/// A registered lane's lease comes from the lane's own worktree, the one the lane registry holds
/// for it (not by its name: a lane and its folder may be named apart): asked from another checkout
/// under the lane's name it ships that checkout's tree as the lane's, a wrong-tree run (`devhub run
/// dhf19 ...` from a shell in the main checkout did, 11-05). A lane the registry has no worktree
/// for (the master's own, CI's) and one whose worktree is gone are not checked.
fn from_the_lanes_worktree(registered: Option<&str>, req: &Req) -> Result<(), String> {
    let (Some(dir), Some(lane_dir), false) = (&req.repo_dir, registered, req.kind == super::Kind::Inspect) else { return Ok(()) };
    let canon = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    if !Path::new(lane_dir).is_dir() || canon(dir) == canon(lane_dir) {
        return Ok(());
    }
    Err(format!("lane {}'s worktree is {lane_dir}, and this was asked from {dir}: run it from {lane_dir} (from here it would ship this checkout's tree as lane {})", req.lane, req.lane))
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

/// The node's receipt for lease `id` with no survivor: the only answer that frees its capacity. A
/// lease the node holds no record of counts once the node swept its marked processes (`unknown`).
fn cleanup_confirmed(v: &makepad_strict_json::Value, id: &str) -> bool {
    v.get("ended").and_then(|v| v.as_str()) == Some(id)
        && v.get("err").is_none()
        && v.get("pending").is_none()
        && v.get("leak").and_then(|v| v.as_arr()).is_some_and(|v| v.is_empty())
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

/// `<home>/heard/<node>`: when a box that stopped answering last answered (ms) and its caps line.
/// A box off since before the master's restart still counts as what it was: a CI shard only it
/// takes says "box down: <node> (since ..)", not "none is set up" (one went off 11-07 and every
/// nightly run after said the second, 11-10).
const HEARD: &str = "heard";

fn read_heard(dir: &Path) -> HashMap<String, (u64, String, bool)> {
    std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(e.path()).ok()?;
        let (ms, line) = text.trim_end().split_once('\n')?;
        makepad_devhub_conf::valid_name(&name).then_some(())?;
        Some((name, (ms.parse().ok()?, line.to_string(), true)))
    }).collect()
}

fn read_blocked(home: &Path) -> Vec<String> {
    std::fs::read_to_string(home.join("blocked")).unwrap_or_default().lines().map(str::trim).filter(|l| makepad_devhub_conf::valid_name(l)).map(str::to_string).collect()
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

/// A timed run whose box spent this much CPU (% of one core, mean) outside it was not quiet.
pub const NOISY_CPU_PCT: f64 = 50.0;
/// A GPU timing (a Mac's gpu work, [`super::sched::Req::gpu`]) starts once its box's GPU is busy
/// less than this (% of the device) outside its leases: the GPU is what it measures. The CPU a
/// Mac's own services take beside it is not: Spotlight, XProtect and an app's helper kept 0.5-1.2
/// cores busy on a 16-core Mac at all times, and a gpu lease first in line on it never started
/// (11-10).
pub const NOISY_GPU_PCT: f64 = 10.0;
/// Why a timed run's box was not quiet ([`box_line`]'s `box`), if it was not: what ran outside it
/// took [`NOISY_CPU_PCT`] on average. Its timings are not clean, and its release line says so: a
/// timed gate on a Mac a person's desktop shared ran single runs at 56-280 s against 7.6 s, its
/// numbers handed back as any other's (11-10). Its load is no verdict here: under the run it
/// counts the run's own threads (a timed build's rustc frontends took a box past its cores).
///
/// A GPU timing's (`gpu`) is judged by what it measures: the GPU busy outside it,
/// [`NOISY_GPU_PCT`] or more on average; the CPU outside it is a figure in its [`box_line`]. One
/// whose GPU told no use outside it is not judged.
pub fn unquiet(b: &makepad_strict_json::Value) -> Option<String> {
    if b.get("gpu").and_then(|g| g.as_bool()) == Some(true) {
        let outside = b.get("outside_gpu_pct").and_then(super::num)?;
        return (outside >= NOISY_GPU_PCT).then(|| format!("its GPU was {outside:.0}% busy outside it on average"));
    }
    let outside = b.get("outside_cpu_pct").and_then(super::num)?;
    (outside >= NOISY_CPU_PCT).then(|| format!("{outside:.0}% of a core ran outside it on average"))
}

/// A timed lease's `box` (what shared its box, the node's look every 10 s) in a line:
/// `load1 max 27.0; outside it 140% of a core (max 210%): mds_stores 120%, XprotectService 30%`.
pub fn box_line(b: &makepad_strict_json::Value) -> String {
    let n = |k: &str| b.get(k).and_then(super::num).unwrap_or(0.0);
    let busiest = |key: &str| {
        let by: Vec<String> = b.get(key).and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|p| {
            let p = p.as_arr()?;
            Some(format!("{} {:.0}%", p.first()?.as_str()?, p.get(1).and_then(super::num)?))
        }).collect();
        if by.is_empty() { String::new() } else { format!(": {}", by.join(", ")) }
    };
    let gpu = if b.get("outside_gpu_pct").is_some() { format!("; its GPU busy outside it {:.0}% (max {:.0}%){}", n("outside_gpu_pct"), n("outside_gpu_pct_max"), busiest("gpu_busiest")) } else { String::new() };
    format!("load1 max {:.1}; outside it {:.0}% of a core (max {:.0}%){}{gpu}", n("load1_max"), n("outside_cpu_pct"), n("outside_cpu_pct_max"), busiest("busiest"))
}

/// The tests a libtest output names under its last `failures:`, at most 20.
pub(crate) fn failed_names(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(at) = lines.iter().rposition(|l| *l == "failures:") else { return Vec::new() };
    lines[at + 1..].iter().map(|l| l.trim()).skip_while(|l| l.is_empty()).take_while(|l| !l.is_empty()).take(20).map(str::to_string).collect()
}

pub(super) fn strs(v: Option<&makepad_strict_json::Value>) -> Vec<String> {
    v.and_then(|a| a.as_arr()).map(|a| a.iter().map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| super::num(x).map(|n| (n as i64).to_string()).unwrap_or_default())).collect()).unwrap_or_default()
}

/// `devhub stats calls`: the last day's live calls as their boxes metered them
/// ([`Master::ledger_call`]), per class and kind, launches apart from calls: how many, their
/// peak cores (the busiest [`super::exec::CALL_WINDOW`]) at p50, p90 and most, their mean cores and
/// their median wall. What a live session's call charge would be sized from.
fn call_stats(path: &Path, now: u64) -> Result<String, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let since = now.saturating_sub(86_400_000);
    let mut by: BTreeMap<(String, String, bool), Vec<(f64, f64, f64)>> = BTreeMap::new();
    for v in text.lines().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) {
        if v.get("how").and_then(|h| h.as_str()) != Some("call") { continue; }
        let n = |k| v.get(k).and_then(super::num).filter(|n| n.is_finite());
        let (Some(class), Some(peak), Some(mean), Some(ms)) = (v.get("class").and_then(|c| c.as_str()), n("peak_cores"), n("mean_cores"), n("call_ms")) else { continue };
        if n("ended_ms").is_none_or(|t| (t as u64) < since) { continue; }
        let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("").to_string();
        by.entry((class.to_string(), kind, v.get("launch").and_then(|l| l.as_bool()).unwrap_or(false))).or_default().push((peak, mean, ms));
    }
    if by.is_empty() { return Ok("no live calls metered in the last day\n".into()); }
    let at = |sorted: &[f64], tenths: usize| sorted[(sorted.len() * tenths).div_ceil(10).max(1) - 1];
    let mut out = format!("live calls in the last day: peak cores (the busiest {} s) p50/p90/max, mean cores, median wall\n", super::exec::CALL_WINDOW.as_secs());
    for ((class, kind, launch), samples) in &by {
        let sorted = |f: fn(&(f64, f64, f64)) -> f64| { let mut v: Vec<f64> = samples.iter().map(f).collect(); v.sort_by(f64::total_cmp); v };
        let (peaks, walls) = (sorted(|s| s.0), sorted(|s| s.2));
        let mean = samples.iter().map(|s| s.1).sum::<f64>() / samples.len() as f64;
        out.push_str(&format!("  {class} ({kind}) {}: {} sample(s), peak {:.1}/{:.1}/{:.1}, mean {mean:.1}, wall {:.1} s\n", if *launch { "launch" } else { "call" }, samples.len(), at(&peaks, 5), at(&peaks, 9), peaks[peaks.len() - 1], at(&walls, 5) / 1000.0));
    }
    Ok(out)
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
            let req = v.get("req").and_then(|r| Req::parse(&r.to_json()).ok());
            let class = v.get("cost").and_then(|v| v.as_str()).and_then(CostClass::parse)
                .or_else(|| req.as_ref().map(|r| r.asked_cost()))
                .or_else(|| v.get("kind").and_then(|v| v.as_str()).and_then(Kind::parse).map(CostClass::default_for));
            // a leg devhub runs itself asks at its run's place: its wait is its time in the queue
            let wait = match n("queued_ms") { Some(q) if req.as_ref().is_some_and(super::sched::runs_place) => q, _ => granted - asked };
            if let Some(class) = class { waits.insert(id.to_string(), (class, wait)); }
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
            waits.insert(l.id.clone(), (l.cost_class, l.waited_ms(granted)));
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
    /// The master that sends a node's store the objects it lacks this way ([`FillObjects`]).
    #[cfg(feature = "checkout")]
    pub(crate) fn with_fill_objects(mut self, fill: FillObjects) -> LeaseMaster {
        self.fill_objects = Some(fill);
        self
    }

    pub fn new(machine: &str, home: PathBuf, ask: Ask, publish: Publish, clock: Clock, ensure_rev: Option<EnsureRev>, inline: bool) -> LeaseMaster {
        // ids a previous master gave out and no file names yet (a grant in flight at its exec) stay
        // unused: a holder of one would take a new lease's place (a lane's run once kept the id a new lease was given)
        let next = next_id(&home.join("leases.jsonl"));
        let next = if next > 1 { next + 1000 } else { next };
        let heard = read_heard(&home.join(HEARD));
        // the leases a previous master granted and did not end: their holders and nodes still
        // run them (a restart of the master must not cost them their work)
        let mut sched = Sched::new(machine, next);
        let now = clock();
        sched.nodes_settle_ms = now + super::sched::RESTORE_GRACE_MS;
        sched.ended_from_ms = now;
        let mut restored_own = Vec::new();
        // a cleanup the previous master did not see confirmed: its end goes to the node again
        // (its answer is the cleanup's receipt, or that it still runs)
        let mut ends_later = Vec::new();
        for line in std::fs::read_to_string(home.join("leases.live")).unwrap_or_default().lines() {
            if let Some(id) = sched.restore(line, now) {
                if sched.get(&id).is_some_and(|l| l.req.master && matches!(l.state, State::Granted | State::Starting)) { restored_own.push(id.clone()); }
                if let Some(node) = sched.cleaning(&id) { ends_later.push((node.to_string(), format!("lease-end {id} released"))); }
            }
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
            up_ms: crate::proto::wall_ms(),
            network: Mutex::new(None),
            awake: Mutex::new(None),
            sched: Mutex::new(sched),
            ask,
            publish,
            clock,
            ensure_rev,
            fill_objects: None,
            home,
            costs_offset: Mutex::new(0),
            profile_writer: Mutex::new(0),
            inline,
            marks: Mutex::new(HashMap::new()),
            input_calls: Mutex::new(HashMap::new()),
            holders: Mutex::new(HashMap::new()),
            pushed: Mutex::new(HashMap::new()),
            jit_seen: Mutex::new(HashMap::new()),
            hub_seen: Mutex::new(HashMap::new()),
            cleanups: Mutex::new(HashMap::new()),
            cleaned: std::sync::Condvar::new(),
            changes: (Mutex::new(0), std::sync::Condvar::new()),
            saved: Mutex::new(String::new()),
            calling: Mutex::new(Default::default()),
            ends_later: Mutex::new(ends_later),
            weighing: Mutex::default(),
            copying: Mutex::default(),
            restored_own: Mutex::new(restored_own),
            agents_seen: Mutex::new(HashMap::new()),
            detached_said: Mutex::new(HashMap::new()),
            stalls: Mutex::new(HashMap::new()),
            heard: Mutex::new(heard),
            lane_trees: Mutex::new(None),
        }
    }

    /// Where the lane registry says each live lane's worktree is: a lease of such a lane comes
    /// from there or is refused ([`from_the_lanes_worktree`]).
    pub fn set_lane_trees(&self, trees: LaneTrees) {
        *self.lane_trees.lock().unwrap() = Some(trees);
    }

    fn from_its_worktree(&self, req: &Req) -> Result<(), String> {
        let trees = self.lane_trees.lock().unwrap().clone();
        from_the_lanes_worktree(trees.and_then(|t| t(&req.lane)).as_deref(), req)
    }

    /// The master's own leases a previous master granted that no record of this master's work
    /// `named`: their askers died with it, and nothing follows a run but from its record (a job, a
    /// live run, a leg, a CI step), so each ends now instead of holding its box through the restore
    /// grace (a run of every app, a live run granted before it was noted, 11-03).
    /// Called once, after the workflows read their records back. The leases ended.
    pub fn end_unfollowed(self: &Arc<Self>, named: &dyn Fn(&str) -> bool) -> Vec<String> {
        let ids = std::mem::take(&mut *self.restored_own.lock().unwrap());
        let now = self.now();
        let ended: Vec<String> = {
            let mut s = self.sched.lock().unwrap();
            ids.into_iter().filter(|id| !named(id) && s.end_unfollowed(id, now)).collect()
        };
        if !ended.is_empty() {
            self.pump();
        }
        ended
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

    /// The time the machine slept since the lease loop last looked, said to the scheduler before
    /// it judges anything on time ([`Sched::master_slept`]): its tick, a box's unanswered caps.
    /// Whichever looks first after a wake takes it, under the scheduler's lock.
    fn note_sleep(&self, s: &mut Sched) {
        let mut awake = self.awake.lock().unwrap();
        let Some((mono, wall)) = *awake else { return };
        let (mono_now, wall_now) = (std::time::Instant::now(), self.now());
        *awake = Some((mono_now, wall_now));
        let slept = slept_ms(wall_now.saturating_sub(wall), mono_now.saturating_duration_since(mono));
        if slept > 0 { s.master_slept(slept, wall_now); }
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
        if let Err(e) = self.persist() { self.event(Sev::Err, e); return; }
        *self.changes.0.lock().unwrap() += 1;
        self.changes.1.notify_all();
    }

    /// Whether queued `id` starts on `node` once `gone` have ended ([`Sched::starts_without`]).
    pub fn starts_without(&self, id: &str, node: &str, gone: &[String]) -> bool {
        self.sched.lock().unwrap().starts_without(id, node, gone, proto::wall_ms())
    }

    /// The boxes other than its named one that could start lane `lane`'s queued lease now
    /// ([`Sched::starts_elsewhere`]).
    pub fn starts_elsewhere(&self, lane: &str) -> Vec<String> {
        self.sched.lock().unwrap().starts_elsewhere(lane, proto::wall_ms())
    }

    /// A JIT build on `node` by its measure ([`Sched::jit_build_ms`]).
    pub fn jit_build_ms(&self, node: &str) -> u64 {
        self.sched.lock().unwrap().jit_build_ms(node)
    }

    /// Writes `leases.live` when its text changed: every state change reaches the file at once
    /// (not after a node's teardown), so a restart in between neither loses a lease nor brings an
    /// ended one back.
    fn persist(&self) -> Result<(), String> {
        // Acquire the writer before taking the scheduler snapshot: a waiting writer can
        // never overwrite a newer snapshot with one captured earlier.
        let mut saved = self.saved.lock().unwrap();
        let lines = self.sched.lock().unwrap().live_lines();
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        if *saved == text { return Ok(()); }
        crate::state::durable_write(&self.home.join("leases.live"), text.as_bytes())?;
        *saved = text;
        Ok(())
    }

    /// See [`Sched::timed_series`].
    pub fn lane_seen_ms(&self, name: &str, dir: &std::path::Path) -> u64 {
        self.sched.lock().unwrap().lane_seen_ms(name, dir)
    }

    /// A route through this master to an app (`<node>:<spec>`, opening `service`): someone looks
    /// at it or drives it ([`Sched::app_touched`]).
    pub fn app_touched(&self, node: &str, spec: &str, service: &str) {
        let now = self.now();
        self.sched.lock().unwrap().app_touched(node, spec, service, now);
    }

    /// No other lease of this lane is queued, live, or recently ended.
    pub fn lane_quiet(&self, lane: &str, own: &[&str], now: u64, idle_ms: u64) -> bool {
        let s = self.sched.lock().unwrap();
        // Missing end history cannot prove quiet (including just after a master restart).
        now.saturating_sub(s.ended_from_ms) >= idle_ms &&
            !s.view().iter().chain(s.recent_ended(usize::MAX).iter()).any(|l|
                l.req.lane == lane && !own.contains(&l.id.as_str()) &&
                l.ended_ms.is_none_or(|end| now.saturating_sub(end) < idle_ms))
    }

    /// Copy from the node directly while the caller still holds its live session's call.
    pub fn fetch(&self, node: &str, lane: &str, lease: &str, rel: &str, dst: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
        let files = super::client::fetch_from(&|service, line| (self.ask)(node, service, line), node, lane, lease, rel, dst, false)?;
        if files.is_empty() { return Err(format!("{node}: nothing under {rel} for lane {lane}")); }
        Ok(files)
    }

    // (its one caller, the orch tests, builds with the checkout only)
    #[cfg(all(test, feature = "checkout"))]
    pub(crate) fn test_sched(&self, f: impl FnOnce(&mut Sched)) {
        f(&mut self.sched.lock().unwrap());
    }

    pub fn timed_series(&self, dir: &str) -> Option<String> {
        let now = self.now();
        self.sched.lock().unwrap().timed_series(dir, now)
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
        let mut records = CostRecords::default();
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).is_err() || !line.ends_with('\n') { break; }
            *offset += line.len() as u64;
            if let Ok(v) = makepad_strict_json::parse(line.as_bytes()) {
                records.push(&v);
            }
        }
        records.apply(&mut self.sched.lock().unwrap());
    }

    /// Places what fits and carries out the scheduler's actions.
    pub fn pump(self: &Arc<Self>) {
        self.refresh_costs();
        let now = self.now();
        #[cfg(feature = "checkout")]
        let interactive_lanes = crate::orch::ctx().map(|ctx| ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live" && l.waited).map(|l| l.name.clone()).collect())).unwrap_or_default();
        #[cfg(feature = "checkout")]
        let live_jobs: std::collections::HashSet<String> = crate::orch::ctx().map(|ctx| ctx.store.read(|d| d.lives.iter().filter(|x| x.active() && !x.lease.is_empty()).map(|x| x.lease.clone()).collect())).unwrap_or_default();
        // the lanes the user put first, and their live test sessions' leases (lane `live-W<n>`)
        #[cfg(feature = "checkout")]
        let first_lanes: std::collections::HashSet<String> = crate::orch::ctx().map(|ctx| ctx.store.read(|d| {
            let first: std::collections::HashSet<String> = d.lanes.iter().filter(|l| l.state == "live" && l.first).map(|l| l.name.clone()).collect();
            let tests: Vec<String> = d.lives.iter().filter(|x| x.active() && x.test && first.contains(&x.lane)).map(|x| format!("live-{}", x.id)).collect();
            first.into_iter().chain(tests).collect()
        })).unwrap_or_default();
        // the branches the merge bot holds: a held series keeps its bench box unlent
        #[cfg(feature = "checkout")]
        let held_branches: std::collections::HashSet<String> = crate::orch::ctx().map(|ctx| ctx.store.read(|d| d.held.iter().map(|(b, _)| b.clone()).collect())).unwrap_or_default();
        let acts = {
            let mut s = self.sched.lock().unwrap();
            #[cfg(feature = "checkout")]
            { s.interactive_lanes = interactive_lanes; s.live_jobs = live_jobs; s.first_lanes = first_lanes; s.held_branches = held_branches; }
            self.note_sleep(&mut s);
            for id in s.holders_gone(&|pid| crate::procinfo::alive(pid)) {
                s.release(&id, "holder-dead", now);
            }
            s.tick(now)
        };
        if let Err(e) = self.persist() {
            self.sched.lock().unwrap().retry_actions(acts);
            self.event(Sev::Err, e);
            return;
        }
        self.act(acts);
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
                Action::Log(line) => eprintln!("devhub sched: {line}"),
                Action::Grant { id, node } => {
                    let inspect = self.sched.lock().unwrap().get(&id).is_some_and(|l| !l.req.set_up_on_node());
                    if inspect {
                        // nothing to set up: an inspect lease is the right to talk to a JIT, a
                        // hub build's the room its box's hub builds itself in
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
                Action::Capture { id, node, why } => self.spawn(move |m| m.capture(&id, &node, &why)),
                Action::Yield { id, node, why } => self.spawn(move |m| {
                    if let Err(e) = m.ask1(&node, "lease-yield", &format!("lease-yield {id} {why}")) {
                        m.event(Sev::Warn, format!("{id} on {node}: its $DEVHUB_YIELD file was not made: {e}"));
                    }
                }),
                Action::Reclaim { node, need_gb } => self.spawn(move |m| {
                    if let Err(e) = m.ask1(&node, "disk", &format!("disk need {need_gb:.0}")) {
                        m.event(Sev::Warn, format!("node {node}: disk reclaim failed: {e}"));
                    }
                }),
            }
        }
    }

    /// A stalled lease says where it is. Its node's look at its processes and last lines
    /// (`lease-why`) goes to its kept log (`devhub job result <id>`): a leg with no JIT under it
    /// had nothing kept, and its hang in a fixture's stub was read by hand (L634083, 11-08). Its
    /// JITs each capture their state (stacks with tier, compile queue, waits, flight recording)
    /// into one report on its box, and its likely cause and `<node>:<path>` go to the inbox as an
    /// ERR; a run with no JIT under it says so (silence read as a capture that never ran, 11-04).
    /// Then the scheduler holds what was found ([`Sched::captured`]): where it waits, its last
    /// step, where its capture is, which a leg devhub runs names as it ends hung
    /// ([`Sched::end_hung_legs`]); one whose JIT it finds parked ends at once ([`Sched::end_parked`]).
    fn capture(self: &Arc<Self>, id: &str, node: &str, why: &str) {
        let (mut note, ended) = self.capture_on(id, node, why);
        if !ended {
            let log = self.lease_log(id);
            if log.exists() { note.push(format!("its capture: devhub job result {id} ({})", log.display())); }
            self.sched.lock().unwrap().captured(id, &note.join("; "));
        }
        // (its end, now or by a parked JIT, goes out and is kept at once)
        self.pump();
    }

    /// [`Self::capture`]'s work on the node: what it found, for the hung end's line, and whether
    /// a parked JIT ended the lease.
    fn capture_on(self: &Arc<Self>, id: &str, node: &str, why: &str) -> (Vec<String>, bool) {
        let mut note = Vec::new();
        match self.ask1(node, "lease-why", &format!("lease-why {id}")) {
            Ok(v) => {
                if let Some(s) = v.get("stuck").filter(|s| !s.is_null()) {
                    let chain = s.get("chain").and_then(|c| c.as_str()).unwrap_or("");
                    let ran = s.get("s").and_then(super::num).map_or(String::new(), |s| format!(", for {}", super::sched::dur(s as u64 * 1000)));
                    note.push(format!("it waits in `{chain}` (pid {}{ran})", s.get("pid").and_then(super::num).unwrap_or(0.0) as u64));
                }
                if let Some(step) = v.get("step").and_then(|s| s.as_str()) { note.push(format!("its last step line `{step}`")); }
                let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
                if let Err(e) = self.keep_log(id, &format!("== {node}: its processes and last lines when it stalled ({why})\n{text}\n")) {
                    note.push(format!("its processes are not kept: {e}"));
                }
            }
            Err(e) => note.push(format!("its processes were not read: {e}")),
        }
        let pids: Vec<u64> = match self.ask1(node, "lease-jits", &format!("lease-jits {id}")) {
            Ok(v) => v.get("pids").and_then(|p| p.as_arr()).map(|a| a.iter().filter_map(super::num).map(|n| n as u64).collect()).unwrap_or_default(),
            // an older hub without the verb, or the lease gone meanwhile
            Err(e) => { self.event(Sev::Info, format!("{id} on {node} stalled: no JIT capture ({e})")); return (note, false); }
        };
        if pids.is_empty() {
            self.event(Sev::Info, format!("{id} on {node} stalled ({why}): no JIT runs under it, nothing to capture"));
            return (note, false);
        }
        let ask = format!("{{\"cmd\":\"capture\",\"why\":{}}}", makepad_remote::json_str(&format!("devhub: {why}")));
        for pid in pids {
            let answer = (self.ask)(&format!("{node}:{pid}"), "cmd", &ask).and_then(|lines| {
                let v = lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok().filter(|v| v.get("cause").is_some() || v.get("err").is_some())).ok_or("no answer")?;
                if let Some(e) = v.get("err").and_then(|e| e.as_str()) { return Err(e.to_string()); }
                Ok((v.get("cause").and_then(|c| c.as_str()).unwrap_or("").to_string(), v.get("path").and_then(|c| c.as_str()).unwrap_or("").to_string()))
            });
            let stacks = self.keep_stacks(id, node, pid, why, answer.as_ref().map_or("none", |(_, path)| path.as_str()));
            match answer {
                Ok((cause, path)) => {
                    self.event(Sev::Err, format!("{id} on {node} stalled ({why}): JIT {pid}: {cause} ({node}:{path}{stacks})"));
                    note.push(format!("JIT {pid}'s capture {node}:{path}"));
                    if (cause.starts_with("compile error: ") || cause.starts_with("parked panic: ")) && self.sched.lock().unwrap().end_parked(id, &cause, self.now()) {
                        return (note, true);
                    }
                }
                Err(e) if e.contains("unknown command") => self.event(Sev::Info, format!("{id} on {node} stalled: JIT {pid} has no capture yet (an older JIT build{stacks})")),
                Err(e) => self.event(Sev::Warn, format!("{id} on {node} stalled: JIT {pid}'s capture failed: {e}{stacks}")),
            }
        }
        (note, false)
    }

    /// A stalled JIT's every thread's stack, once per lease and JIT, in the lease's kept log
    /// (`devhub job result <id>`), where its last lines go at its end: the capture's report stays
    /// on its box and only its newest 40 do, and a CI lease hung for 43 min kept only its test
    /// output (L487376, 11-06). Its note for the stall's line: where they are, or why not.
    fn keep_stacks(&self, id: &str, node: &str, pid: u64, why: &str, capture: &str) -> String {
        let head = format!("== JIT {pid} on {node}: every thread's stack");
        if std::fs::read_to_string(self.lease_log(id)).is_ok_and(|t| t.contains(&head)) {
            return String::new();
        }
        let ask = r#"{"cmd":"stacks","thread":"all","depth":"48"}"#;
        let stacks = (self.ask)(&format!("{node}:{pid}"), "cmd", ask).map_or_else(|e| format!("(its stacks did not come: {e})"), |lines| lines.join("\n"));
        match self.keep_log(id, &format!("{head} when it stalled ({why}); its capture {node}:{capture}\n{stacks}\n")) {
            Ok(_) => format!("; every thread's stack: devhub job result {id}"),
            Err(e) => format!("; its stacks are not kept: {e}"),
        }
    }

    /// The lease's kept log: its stalls' stacks, and its last lines when it ends abnormally.
    fn lease_log(&self, id: &str) -> PathBuf {
        self.home.join("lease-logs").join(format!("{id}.log"))
    }

    /// Adds `text` to the lease's kept log, after what it holds.
    fn keep_log(&self, id: &str, text: &str) -> std::io::Result<PathBuf> {
        use std::io::Write;
        let path = self.lease_log(id);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(text.as_bytes())?;
        Ok(path)
    }

    /// A live session's call ended and none runs: its box is charged its resident share
    /// ([`Sched::live_room`]); the freed cores go to what waits.
    pub fn live_idle(self: &Arc<Self>, id: &str) {
        let mut s = self.sched.lock().unwrap();
        s.live_called.insert(id.to_string());
        s.live_call_ended(id);
        if s.live_idle.insert(id.to_string()) {
            drop(s);
            self.weigh_live(id);
            self.pump();
        }
    }

    /// A live session's call starts: its full charge again, if its box has room (`Err`: why
    /// not). An interactive call whose full charge does not fit starts at once with the cores
    /// free there, charged those, and runs slower beside the work ([`Sched::live_room`]); a
    /// background one waits. A call that waits keeps its room on its box from every other ask
    /// until it runs or [`Self::live_unwait`].
    pub fn live_admit(self: &Arc<Self>, id: &str) -> Result<(), String> {
        let mut s = self.sched.lock().unwrap();
        if !s.live_idle.contains(id) {
            return Ok(());
        }
        if let Err(why) = s.live_room(id) {
            s.live_waiting.insert(id.to_string());
            return Err(why);
        }
        let short = s.call_start_cores(id);
        if let Some((free, _, _)) = short {
            s.live_call_charged(id, free);
        }
        s.live_idle.remove(id);
        s.live_waiting.remove(id);
        s.live_call_began(id, self.now());
        let past = s.past_budget_said(id);
        let past = match short {
            Some((free, full, in_use)) => {
                let node = s.get(id).and_then(|l| l.node.clone()).unwrap_or_default();
                format!(" at once with the {free:.0} cores free on {node} of its {full:.0} ({in_use:.0} in use beside it): slower beside that work, not waiting for it{past}")
            }
            None => past,
        };
        drop(s);
        if !past.is_empty() { self.event(Sev::Info, format!("{id}: its call runs{past}")); }
        self.weigh_live(id);
        Ok(())
    }

    /// A live session's box weighs its runs as interactive work while a call or its verdict
    /// runs, as background work between calls (`lease-weight <id> call|idle`,
    /// [`super::Priority::cpu_weight`]): an idle session that spins then slows no gate beside it.
    /// Off the caller's thread, in the session's turn, each send says the session's state as it
    /// is then: the last one says the newest.
    fn weigh_live(self: &Arc<Self>, id: &str) {
        let turn = self.weighing.lock().unwrap().entry(id.to_string()).or_default().clone();
        let id = id.to_string();
        self.spawn(move |m| {
            let _turn = turn.lock().unwrap();
            let now = {
                let s = m.sched.lock().unwrap();
                s.get(&id).filter(|l| l.state == State::Granted).and_then(|l| l.node.clone()).map(|node| (node, s.live_idle.contains(&id)))
            };
            let Some((node, idle)) = now else {
                m.weighing.lock().unwrap().remove(&id);
                return;
            };
            match m.ask1(&node, "lease-weight", &format!("lease-weight {id} {}", if idle { "idle" } else { "call" })) {
                Ok(v) => if let Some(call) = v.get("call") { m.ledger_call(&id, &node, call); },
                Err(e) => m.event(Sev::Info, format!("{id}: its box did not take its weight: {e}")),
            }
        });
    }

    /// A live call stopped waiting for room without running (its session parked or ended, its
    /// client went): its box lets other asks into that room again.
    pub fn live_unwait(self: &Arc<Self>, id: &str) {
        if self.sched.lock().unwrap().live_waiting.remove(id) {
            self.pump();
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
        let mut copied = None;
        // its `--jit` there first when the box lacks it; a failure at the box it comes from
        // (Some(that box)) is no failure of this one
        let set_up = |ship: &mut u64, copied: &mut Option<JitCopy>| -> Result<String, (Option<String>, String)> {
            // (a retry finds it there: the copy before it stays the one said)
            if let Some(c) = self.copy_jit(&l, node).map_err(|(at, e)| ((at != node).then_some(at), e))? { *copied = Some(c); }
            self.start_on(&l, node, ship).map_err(|e| (None, e))
        };
        // a hub restarting for its update is back in seconds (it built its new self first): the
        // lease waits it out on its box instead of going elsewhere or failing
        let mut r = set_up(&mut ship, &mut copied);
        // (inline: tests, whose node calls answer at once)
        let grace = if self.inline { Duration::ZERO } else { START_GRACE };
        while r.as_ref().is_err_and(|(from, e)| from.is_none() && (unreachable(e) || e.contains("restarting: "))) && t0.elapsed() < grace {
            std::thread::sleep(Duration::from_secs(2));
            if self.sched.lock().unwrap().get(id).is_none_or(|x| x.node.as_deref() != Some(node) || x.state != State::Starting) {
                break;
            }
            r = set_up(&mut ship, &mut copied);
        }
        let setup = t0.elapsed().as_millis() as u64;
        let now = self.now();
        self.sched.lock().unwrap().phase(id, "sync", l.granted_ms.unwrap_or(now.saturating_sub(setup)), now);
        let r = match r {
            // the box its JIT was to come from went (down until its hub reports in), or evicted it
            // (a new look there): placed again
            Err((Some(from), e)) => {
                self.sched.lock().unwrap().tree_busy(id, &format!("its JIT's copy from {from} failed ({e})"));
                if unreachable(&e) { self.sched.lock().unwrap().set_down(&from); } else { self.refresh_node(&from); }
                self.save_live();
                self.pump();
                return;
            }
            r => r.map_err(|(_, e)| e),
        };
        if let Some(c) = &copied { self.ledger_jit_copy(node, c); }
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
                let copy = copied.as_ref().map_or(String::new(), |c| format!(", JIT {} copied from {} ({:.1} MB in {})", c.id, c.from, c.bytes as f64 / 1e6, secs(c.ms)));
                self.event(Sev::Info, format!("lease_latency: {} of {} on {node}: {}setup {} (ship {}{copy})", l.tag(), l.req.lane, wait.map_or(String::new(), |w| format!("release->grant {}, ", secs(w))), secs(setup), secs(ship)));
            }
            // the box's hub is not there (restarting for its update): placed again, not failed
            Err(e) if unreachable(&e) => self.sched.lock().unwrap().unreachable(id, &e),
            // the box's hub is between its runs, restarting for its self-update: placed again (the
            // lease queues, never fails for a roll)
            Err(e) if e.contains("restarting: ") => self.sched.lock().unwrap().tree_busy(id, &e),
            // the box's previous lease is still leaving its tree: placed again, not failed
            Err(e) if e.contains("the tree is at ") || e.contains("overlay lane ") || e.contains("native trees busy:") => self.sched.lock().unwrap().tree_busy(id, &e),
            // something builds on the box that the last look there did not see: placed again on a
            // new look, which holds timed work back until it is over
            // its display went since the last look: placed again, elsewhere if it can (a hot
            // bucket read this box's refusal as "no verdict" and named a landing for it, 11-04)
            Err(e) if e.contains(" refused: lease-start ") && e.contains(": gui=1: ") => {
                let mut s = self.sched.lock().unwrap();
                s.no_display(node);
                s.tree_busy(id, &e);
            }
            Err(e) if e.contains(super::TIMED_AFTER) => {
                self.sched.lock().unwrap().tree_busy(id, &e);
                self.refresh_node(node);
            }
            // any other refusal: the lease never ran there, so it is placed again on another box
            // and ends denied naming the refusals once none is left; ended `failed`, a lock a
            // killed hub left in one box's tree read as a red check of the crates (11-13)
            Err(e) => self.sched.lock().unwrap().refused_start(id, &e),
        }
        self.save_live();
        self.pump();
    }

    /// The lease's `--jit` copied to `node` when that box lacks it, from a box of its platform that
    /// holds it ([`super::sched::Sched::jit_source`]), a chunk at a time through this master: a
    /// lane's JIT ran only on the box that built it and drove the lane's tests onto a bench box
    /// (11-07). One copy of an id to a box at a time; at its first chunk a box that holds it by
    /// then takes nothing (the key names the same binary). Err: the box it failed at, and why.
    fn copy_jit(&self, l: &Lease, node: &str) -> Result<Option<JitCopy>, (String, String)> {
        let Some(from) = self.sched.lock().unwrap().jit_source(node, l).cloned() else { return Ok(None) };
        let id = l.req.loader.clone().unwrap_or_default();
        let turn = self.copying.lock().unwrap().entry((node.to_string(), id.clone())).or_default().clone();
        let _turn = turn.lock().unwrap();
        let t = std::time::Instant::now();
        let mut off = 0u64;
        loop {
            let chunk = self.ask1(&from, "jit-read", &format!("jit-read {id} {off}")).map_err(|e| (from.clone(), e))?;
            let n = |k: &str| chunk.get(k).and_then(super::num).map_or(0, |n| n as u64);
            let (end, size) = (n("off"), n("size"));
            if end <= off { return Err((from.clone(), format!("{from} jit-read: JIT {id} ends at {off} of its {size} bytes"))); }
            let data = chunk.get("data").and_then(|d| d.as_str()).unwrap_or("");
            let put = self.ask1(node, "jit-put", &format!("jit-put {id} {off} {size} {data}")).map_err(|e| (node.to_string(), e))?;
            if put.get("held").is_some() { return Ok(None); }
            if put.get("kept").is_some() { return Ok(Some(JitCopy { id, from, bytes: size, ms: t.elapsed().as_millis() as u64 })); }
            off = end;
        }
    }

    /// A copy's wall to its box goes to the master's ledger: the placement's measure of the next
    /// one there ([`super::sched::JIT_COPY`]).
    fn ledger_jit_copy(&self, node: &str, c: &JitCopy) {
        let line = format!("{{\"class\":\"{}\",\"kind\":\"\",\"node\":{},\"from\":{},\"jit\":{},\"bytes\":{},\"wall_ms\":{},\"ended_ms\":{}}}\n", super::sched::JIT_COPY, json_str(node), json_str(&c.from), json_str(&c.id), c.bytes, c.ms.max(1), self.now());
        let offset = self.costs_offset.lock().unwrap();
        let written = std::fs::OpenOptions::new().create(true).append(true).open(self.home.join("costs.jsonl")).and_then(|mut f| f.write_all(line.as_bytes()));
        drop(offset);
        if let Err(e) = written { self.event(Sev::Warn, format!("costs.jsonl: {e}")); }
    }

    /// Starts the lease on `node`. A node whose store lacks a pinned commit the master counted
    /// as sent (`NO_COMMIT`) gets it pushed once more, asked afresh, and the lease starts again;
    /// one that lacks objects of the commit's tree (`NO_OBJECTS`) is sent them, and starts again.
    fn start_on(&self, l: &Lease, node: &str, ship_ms: &mut u64) -> Result<String, String> {
        match self.start_once(l, node, ship_ms, false) {
            Err(e) if l.req.rev.is_some() && e.contains(super::NO_COMMIT) => {
                self.event(Sev::Warn, format!("{} {node}: {e}; pushing the pin once more", l.id));
                self.start_once(l, node, ship_ms, true).map_err(|again| format!("{again} (after pushing the pin once more)"))
            }
            Err(e) if l.req.rev.is_some() && super::lacking_objects(&e).is_some() => {
                let (repo, ids) = super::lacking_objects(&e).unwrap();
                self.event(Sev::Warn, format!("{} {node}: its store lacks {} object(s) of {repo}'s pin: sending them", l.id, ids.len()));
                let f = self.fill_objects.as_ref().ok_or("this master cannot send objects (built without checkout)")?;
                let base = PathBuf::from(l.req.repo_dir.clone().unwrap_or_else(|| ".".into()));
                let dir = if repo == "makepad" { base } else { base.join(crate::nested::dir(&repo)) };
                f(node, &repo, &dir, &ids).map_err(|why| format!("{e} (sending its {} object(s): {why})", ids.len()))?;
                self.start_once(l, node, ship_ms, false).map_err(|again| format!("{again} (after sending the {} object(s) its store lacked)", ids.len()))
            }
            r => r,
        }
    }

    fn start_once(&self, l: &Lease, node: &str, ship_ms: &mut u64, fresh: bool) -> Result<String, String> {
        let rev = match &l.req.rev {
            None => "null".to_string(),
            Some(spec) => {
                // the commits the CLI pinned at order time, never what the branches name now
                let (mk, nested) = super::parse_pinned(spec)?;
                let dir = PathBuf::from(l.req.repo_dir.clone().unwrap_or_else(|| ".".into()));
                let f = self.ensure_rev.as_ref().ok_or("this master cannot ship revisions (built without checkout)")?;
                let t = std::time::Instant::now();
                let error = |repo: &str, sha: &str, e: String| if l.req.src == super::Src::Overlay { format!("overlay: {repo} {sha} bare transfer layer: {}", e.replace(['\n', '\r'], " | ")) } else { e };
                // each repository's commit by name: makepad's, then each nested one's
                let mut sides = vec![format!("\"makepad\":{}", json_str(&f(node, "makepad", &dir, &mk, fresh).map_err(|e| error("makepad", &mk.sha, e))?))];
                for (name, side) in &nested {
                    let sha = f(node, name, &dir.join(crate::nested::dir(name)), side, fresh).map_err(|e| error(name, &side.sha, e))?;
                    sides.push(format!("{}:{}", json_str(name), json_str(&sha)));
                }
                *ship_ms = t.elapsed().as_millis() as u64;
                format!("{{{}}}", sides.join(","))
            }
        };
        let (weighed, peak) = self.sched.lock().unwrap().ram_weighed(l);
        let line = lease_start_line(l, &rev, weighed, peak);
        let lines = (self.ask)(node, "lease-start", &line)?;
        let first = lines.into_iter().next().ok_or("lease-start: no answer")?;
        let v = makepad_strict_json::parse(first.as_bytes()).map_err(|_| "lease-start: bad answer".to_string())?;
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return Err(format!("{node} refused: {e}"));
        }
        Ok(first)
    }

    /// A ledger record's `os`: its box's OS family, which weighs that family's next asks
    /// ([`super::sched::Sched::record_family_peak`]); null for a box not heard.
    fn os_field(&self, node: &str) -> String {
        self.sched.lock().unwrap().os_of(node).map_or("null".to_string(), json_str)
    }

    /// Record costs before freeing the node, so its next placement sees this wall.
    fn ended_cost(&self, l: &Lease, how: &str, run_class: &str, usage: Option<&makepad_strict_json::Value>) {
        let class = super::lease_class(&l.req.class, run_class);
        let class = if class.is_empty() { l.req.kind.as_str() } else { class.as_str() };
        let wall = l.ended_ms.unwrap_or_else(|| self.now()).saturating_sub(l.granted_ms.unwrap_or(l.ended_ms.unwrap_or(l.asked_ms)));
        let ended = l.ended_ms.unwrap_or_else(|| self.now());
        let mut phases = l.profile();
        phases.close("report", ended);
        let measured = |key| usage.and_then(|v| v.get(key)).and_then(super::num).filter(|n| n.is_finite() && *n >= 0.0).map_or("null".into(), |n| n.to_string());
        let os = self.os_field(l.node.as_deref().unwrap_or(""));
        // why its run measures nothing of its class, by its node's word ([`Self::refresh_costs`]):
        // its last run did not end by itself, or it stopped a unit as hung
        let said = |k: &str| usage.and_then(|v| v.get(k));
        let unmeasured = said("abnormal").and_then(|a| a.get("line")).and_then(|a| a.as_str()).or_else(|| said("hung").and_then(|h| h.as_str())).map_or("null".to_string(), json_str);
        // a work tree's lease: its tree and rev, whose JIT its box holds now ([`CostRecords`])
        let tree = |s: &str| if l.req.src == super::Src::Tree && l.req.rev.is_some() { json_str(s) } else { "null".to_string() };
        let (tree_lane, tree_rev) = (tree(l.req.tree_lane()), tree(l.req.rev.as_deref().unwrap_or("")));
        let jit_jobs = l.jit_jobs.map_or("null".to_string(), |j| j.to_string());
        let line = format!("{{\"class\":{},\"kind\":\"{}\",\"cost\":\"{}\",\"wall_ms\":{wall},\"jit_peak_gb\":{},\"peak_ram_gb\":{},\"cpu_seconds\":{},\"node\":{},\"os\":{os},\"lane\":{},\"tree_lane\":{tree_lane},\"tree_rev\":{tree_rev},\"ended_ms\":{ended},\"lease\":{},\"how\":{},\"unmeasured\":{unmeasured},\"jit_jobs\":{jit_jobs},\"deployment\":{},\"source\":{}, {}}}\n", json_str(class), l.req.kind.as_str(), l.cost_class.as_str(), measured("jit_peak_gb"), measured("peak_ram_gb"), measured("cpu_seconds"), json_str(l.node.as_deref().unwrap_or("")), json_str(&l.req.lane), json_str(&l.id), json_str(how), json_str(&l.deployment), json_str(usage.and_then(|v| v.get("source")).and_then(|v| v.as_str()).unwrap_or("")), phases.fields(ended));
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
        let marks = self.marks.lock().unwrap().get(id).cloned();
        // Wait for the accepted command and its marks before sending cleanup.
        let marks = marks.as_ref().map(|m| m.lock().unwrap());
        let mut summary = "{\"cleanup\":\"nothing to clean\"}".to_string();
        let (mut teardown, mut cleanup_ok, mut freed_early) = (0u64, true, false);
        if let (Some(node), Some(l)) = (node, l.as_ref()) {
            // an inspect lease that changed nothing has nothing to undo
            if l.req.set_up_on_node() || marks.as_ref().is_some_and(|m| m.paused || !m.breaks.is_empty() || !m.probes.is_empty()) {
                let wire_how = match how {
                    "released" | "expired" | "holder-dead" | "denied" => how,
                    "lost" => "expired",
                    _ => "released",
                };
                let mut parts = Vec::new();
                // pid 0: a refused inspect call made this mark and changed nothing in any app
                if let Some(m) = marks.as_ref().filter(|m| m.pid != 0) {
                    let list = |v: &[String]| v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",");
                    parts.push(format!("\"inspect\":{{\"pid\":{},\"breaks\":[{}],\"probes\":[{}],\"paused\":{}}}", m.pid, list(&m.breaks), list(&m.probes), m.paused));
                }
                // a lease something else ended (a cancel, the hung check, the disk guard; `released`
                // on the wire): its node gives how far its last run got
                if how != "released" { parts.push("\"tail\":true".to_string()); }
                let payload = if parts.is_empty() { String::new() } else { format!(" {{{}}}", parts.join(",")) };
                let t0 = std::time::Instant::now();
                let end_line = format!("lease-end {id} {wire_how}{payload}");
                let answer = self.ask1(node, "lease-end", &end_line);
                teardown = t0.elapsed().as_millis() as u64;
                // the node's memory watch ended its run: a memory end, also when its holder's
                // release came before the node's beat said so (that release measured a cut run)
                let memory = answer.as_ref().ok().and_then(|v| v.get("memory")).and_then(|m| m.as_str()).map(str::to_string);
                if let Some(why) = &memory { self.sched.lock().unwrap().end_for_memory(id, why, self.now()); }
                let how = if memory.is_some() { "memory" } else { how };
                // the box's load after the teardown places what waits there now (its last caps
                // round still counted this lease's work)
                if let Some(load) = answer.as_ref().ok().and_then(|v| v.get("load")) {
                    if let (Some(cpu), Some(avail)) = (load.get("cpu_pct").and_then(super::num), load.get("ram_avail_gb").and_then(super::num)) {
                        self.sched.lock().unwrap().fresh_load(node, cpu, avail);
                    }
                }
                let class = answer.as_ref().ok().and_then(|v| v.get("class")).and_then(|v| v.as_str()).unwrap_or("");
                run_class = class.to_string();
                if let Some(v) = answer.as_ref().ok().filter(|v| v.get("phases").is_some()) {
                    node_phases = super::prof::Phases::parse(v.get("phases"));
                }
                let mut measured = l.clone();
                measured.node_phases = node_phases.clone();
                self.ended_cost(&measured, how, class, answer.as_ref().ok());
                cost_recorded = true;
                cleanup_ok = answer.as_ref().is_ok_and(|v| cleanup_confirmed(v, id));
                if cleanup_ok {
                    self.sched.lock().unwrap().cleaned(node, id, teardown);
                    freed_early = true;
                    self.pump();
                } else {
                    self.sched.lock().unwrap().cleanup_failed(node, id);
                    self.ends_later.lock().unwrap().push((node.to_string(), end_line.clone()));
                }
                match answer {
                    Ok(v) => {
                        let leak = strs(v.get("leak"));
                        let killed = strs(v.get("killed"));
                        let freed = v.get("freed_mb").and_then(super::num).unwrap_or(0.0);
                        if !leak.is_empty() {
                            self.event(Sev::Err, format!("lease_leak: {} of {}: {} processes left after cleanup on {node} (pids {})", l.tag(), l.req.lane, leak.len(), leak.join(" ")));
                        }
                        self.event(Sev::Info, format!("{} of {}: cleaned up on {node}: {} stopped, {freed:.0} MB freed{}", l.tag(), l.req.lane, plural(killed.len(), "process", "processes"), if leak.is_empty() { String::new() } else { format!(", {} LEFT", leak.len()) }));
                        // a run that ended without its normal end: how, in the lane's output (the
                        // release answer) and the inbox, and its last lines kept under the lease id
                        let ended = v.get("abnormal").filter(|_| !matches!(how, "cancelled" | "hung" | "disk" | "memory")).and_then(|a| Some((a.get("line")?.as_str()?.to_string(), a.get("serious").and_then(|s| s.as_bool()).unwrap_or(false), a.get("tail").and_then(|t| t.as_str()).unwrap_or("").to_string())));
                        let keep = |text: &str| self.keep_log(id, text);
                        if let Some((line, serious, tail)) = &ended {
                            let at = keep(&format!("{line}\n{tail}\n")).map_or(String::new(), |path| format!("; its last lines: {}", path.display()));
                            (self.publish)(hub_record(&self.machine, "", 0, "lease", if *serious { Sev::Warn } else { Sev::Info }, format!("{line} ({} of {} on {node}{at})", l.tag(), l.req.lane), tail.clone()));
                        }
                        // a failed test run: its output outlives the lease under the lease's id, its
                        // names in the release answer
                        // a lease something else ended: how far its last run got, under the same id
                        let cut = v.get("tail").and_then(|t| t.as_str()).filter(|_| ended.is_none());
                        let kept_tail = cut.is_some_and(|text| match keep(text) {
                            Ok(_) => true,
                            Err(e) => { self.event(Sev::Warn, format!("{} of {}: its last lines are not kept: {e}", l.tag(), l.req.lane)); false }
                        });
                        let failed: Vec<String> = v.get("failed_tests").and_then(|t| t.as_str()).map_or(Vec::new(), |text| {
                            if let Err(e) = keep(text) {
                                self.event(Sev::Warn, format!("{} of {}: its failed tests' output is not kept: {e}", l.tag(), l.req.lane));
                            }
                            failed_names(text)
                        });
                        // a timed run's box: in the release answer, its verdict too, and the inbox when it
                        // was not quiet
                        let noise = v.get("box").filter(|_| l.req.kind == Kind::Timed);
                        let unquiet = noise.and_then(unquiet);
                        if let (Some(b), Some(why)) = (noise, &unquiet) {
                            self.event(Sev::Warn, format!("{} of {} on {node} was not quiet ({why}): {}", l.tag(), l.req.lane, box_line(b)));
                        }
                        // it wrote under the overlay's virtual root on its box's disk: its work fails, and it is named
                        let wrote_root = v.get("virtual_root").and_then(|w| w.as_str());
                        if let Some(why) = wrote_root {
                            self.event(Sev::Err, format!("{} of {} (agent {}) {why}", l.tag(), l.req.lane, l.req.agent.as_deref().unwrap_or("?")));
                        }
                        // the toolchain changed under it (a rustup update beside it): its run has no
                        // verdict, the holder runs it again ([`super::exec`]'s `toolchain`)
                        let toolchain = v.get("toolchain").and_then(|t| t.as_str());
                        if let Some(t) = toolchain {
                            self.event(Sev::Warn, format!("{} of {} on {node}: the toolchain changed under it ({t}): its run has no verdict", l.tag(), l.req.lane));
                        }
                        summary = format!("{{\"cleanup\":{},\"node\":{},\"stopped\":{},\"freed_mb\":{freed:.0},\"leak\":{},\"ended\":{},\"kept_tail\":{kept_tail},\"box\":{},\"unquiet\":{},\"failed_tests\":[{}],\"virtual_root\":{},\"toolchain\":{}}}", json_str(if cleanup_ok { "ok" } else { "unconfirmed: capacity unknown" }), json_str(node), killed.len(), leak.len(), ended.as_ref().map_or("null".to_string(), |e| json_str(&e.0)), noise.map_or("null".to_string(), |b| b.to_json()), unquiet.as_deref().map_or("null".to_string(), json_str), failed.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(","), wrote_root.map_or("null".to_string(), json_str), toolchain.map_or("null".to_string(), json_str));
                        if let Some(path) = v.get("recovered").and_then(|r| r.as_str()) {
                            self.fetch_recovered(node, path, l);
                        }
                    }
                    // its hub restarts: the new one takes the lease back from its ledger, and
                    // the end goes to it with the node's next beats
                    Err(e) if unreachable(&e) || e.contains("restarting: ") => {
                        self.event(Sev::Info, format!("{} of {}: cleanup on {node} waits for its hub to answer again ({e})", l.tag(), l.req.lane));
                        summary = format!("{{\"cleanup\":{}}}", json_str(&format!("deferred on {node}: its hub restarts")));
                    }
                    Err(e) => {
                        self.event(Sev::Warn, format!("{} of {}: cleanup on {node} failed: {e}", l.tag(), l.req.lane));
                        summary = format!("{{\"cleanup\":{}}}", json_str(&format!("failed on {node}: {e}")));
                    }
                }
            }
        }
        // a lease with nothing to tear down: its node is free now
        if cleanup_ok {
            if let (Some(node), false) = (node, freed_early) { self.sched.lock().unwrap().cleaned(node, id, teardown); }
            self.holders.lock().unwrap().retain(|_, v| v != id);
            self.marks.lock().unwrap().remove(id);
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
            // (a hub build's measure is its hub's, from its restart answer: [`Self::ledger_hub_build`])
            if !cost_recorded && !l.req.hub_build() { self.ended_cost(&l, how, "", None); }
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
        let ok = crate::git_exec::command()
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

    /// A hub build that finished on `node` (its restart answer said it) goes to the master's
    /// ledger as its class's measure ([`super::sched::HUB_BUILD`]): what its next lease asks.
    pub fn ledger_hub_build(&self, node: &str, b: &super::HubMeasure) {
        // (one build, one record: a restart asked again answers the same build)
        if self.hub_seen.lock().unwrap().insert(node.to_string(), b.end_ms) == Some(b.end_ms) { return; }
        let n = |x: Option<f64>| x.map_or("null".to_string(), |x| format!("{x:.3}"));
        let line = format!("{{\"class\":\"{}\",\"kind\":\"build\",\"node\":{},\"os\":{},\"wall_ms\":{},\"peak_ram_gb\":{},\"cpu_seconds\":{},\"ended_ms\":{},\"how\":\"released\"}}\n", super::sched::HUB_BUILD, json_str(node), self.os_field(node), b.wall_ms, n(b.peak_gb), n(b.cpu_s), b.end_ms);
        let offset = self.costs_offset.lock().unwrap();
        let written = std::fs::OpenOptions::new().create(true).append(true).open(self.home.join("costs.jsonl")).and_then(|mut f| f.write_all(line.as_bytes()));
        drop(offset);
        if let Err(e) = written { self.event(Sev::Warn, format!("costs.jsonl: {e}")); }
    }

    /// A live session's call that ended, as its box metered it (`lease-weight <id> idle`'s
    /// `call`): a sample of its class's call charge in the master's ledger (`"how":"call"`),
    /// which `devhub stats calls` and the session's why read, and its class's calls are charged
    /// by (the scheduler's `call_cores`).
    fn ledger_call(&self, id: &str, node: &str, call: &makepad_strict_json::Value) {
        let Some(req) = self.sched.lock().unwrap().get(id).map(|l| l.req.clone()) else { return };
        let n = |k| call.get(k).and_then(super::num).filter(|n| n.is_finite() && *n >= 0.0);
        let (Some(peak), Some(mean), Some(ms)) = (n("peak"), n("mean"), n("ms")) else { return };
        let launch = call.get("launch").and_then(|v| v.as_bool()).unwrap_or(false);
        let line = format!("{{\"class\":{},\"kind\":\"{}\",\"how\":\"call\",\"launch\":{launch},\"call_ms\":{},\"peak_cores\":{peak:.2},\"mean_cores\":{mean:.2},\"node\":{},\"os\":{},\"lane\":{},\"lease\":{},\"ended_ms\":{}}}\n", json_str(req.class()), req.kind.as_str(), ms as u64, json_str(node), self.os_field(node), json_str(&req.lane), json_str(id), self.now());
        let offset = self.costs_offset.lock().unwrap();
        let written = std::fs::OpenOptions::new().create(true).append(true).open(self.home.join("costs.jsonl")).and_then(|mut f| f.write_all(line.as_bytes()));
        drop(offset);
        if let Err(e) = written { self.event(Sev::Warn, format!("costs.jsonl: {e}")); }
    }

    /// Asks every node for its caps (a node that answers again gets its policy).
    pub fn refresh(self: &Arc<Self>, hosts: &[(String, String)]) {
        self.sync_hosts(hosts);
        for (name, _) in hosts {
            let node = name.clone();
            self.per_node(name, "caps", move |m| m.refresh_node(&node));
        }
        write_desk_links(&self.home, &self.nodes());
        self.pump();
    }

    /// Runs one node's call on its own thread unless the same call to it is still in flight.
    fn per_node(self: &Arc<Self>, node: &str, call: &'static str, f: impl FnOnce(&Arc<Self>) + Send + 'static) {
        let key = (node.to_string(), call);
        if !self.calling.lock().unwrap().insert(key.clone()) { return; }
        self.spawn(move |m| {
            f(m);
            m.calling.lock().unwrap().remove(&key);
        });
    }

    fn refresh_node(self: &Arc<Self>, name: &str) {
        let asked = (self.ask)(name, "caps", "caps");
        let line = asked.as_ref().ok().and_then(|l| l.first()).filter(|l| !l.starts_with("{\"err\":"));
        let caps = line.and_then(|l| Caps::parse(l));
        let now = self.now();
        match caps {
            Some(c) => {
                if let Some(line) = line { self.note_heard(name, now, line); }
                self.ledger_jit_builds(name, &c, now);
                self.say_stalls(name, &c.stalls);
                // the hub's outage its WM recorded (in this one answer): the box was down, and why
                if !c.trouble.is_empty() {
                    self.event(Sev::Warn, c.trouble.clone());
                }
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
            None => {
                self.note_unheard(name);
                // a box off the network for a minute: its leases end as lost, their holders ask again
                let lost = {
                    let mut s = self.sched.lock().unwrap();
                    self.note_sleep(&mut s);
                    s.caps_missed(name, &caps_miss(&asked), now)
                };
                if !lost.is_empty() { self.pump(); }
            }
        }
    }

    /// Its caps answered: what it says now is no longer kept on disk.
    fn note_heard(&self, name: &str, now: u64, line: &str) {
        let mut heard = self.heard.lock().unwrap();
        let kept = heard.get(name).is_some_and(|h| h.2);
        heard.insert(name.to_string(), (now, line.to_string(), false));
        drop(heard);
        if kept { let _ = std::fs::remove_file(self.home.join(HEARD).join(name)); }
    }

    /// Its caps did not answer: its last answer goes to disk, once, for a master that starts
    /// while the box is still away.
    fn note_unheard(&self, name: &str) {
        let mut heard = self.heard.lock().unwrap();
        let Some((ms, line, kept)) = heard.get_mut(name).filter(|h| !h.2) else { return };
        *kept = true;
        let text = format!("{ms}\n{line}\n");
        drop(heard);
        let dir = self.home.join(HEARD);
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join(name), text)) {
            self.event(Sev::Warn, format!("{}: {e}", dir.join(name).display()));
        }
    }

    /// When `name`'s caps last answered and what they said, from before this master started too.
    pub fn heard(&self, name: &str) -> Option<(u64, Caps)> {
        let heard = self.heard.lock().unwrap();
        let (ms, line, _) = heard.get(name)?;
        Some((*ms, Caps::parse(line)?))
    }

    /// A wait in a node's hub that two caps answers in a row name is said once, as a warning (a
    /// deadlock as an error): what the box waits on, from its state, while it waits.
    fn say_stalls(&self, name: &str, now: &[String]) {
        let keys: Vec<String> = now.iter().map(|l| crate::held::stall_key(l).to_string()).collect();
        let mut all = self.stalls.lock().unwrap();
        let (before, said) = all.entry(name.to_string()).or_default();
        said.retain(|k| keys.contains(k));
        for (line, key) in now.iter().zip(&keys) {
            if before.contains(key) && !said.contains(key) {
                let sev = if line.contains("a deadlock") { Sev::Err } else { Sev::Warn };
                self.event(sev, format!("{name}'s hub waits: {line}"));
                said.push(key.clone());
            }
        }
        *before = keys;
    }

    /// `lease-beat` to every node up or holding its leases, each on its own thread, naming the
    /// leases it holds there (none: an empty beat); the ones the node does not know are lost.
    pub fn beat_nodes(self: &Arc<Self>) {
        let nodes = self.sched.lock().unwrap().beat_targets();
        for node in nodes {
            let n = node.clone();
            self.per_node(&node, "lease-beat", move |m| m.beat_node(&n));
        }
        let later: std::collections::BTreeSet<String> = self.ends_later.lock().unwrap().iter().map(|(n, _)| n.clone()).collect();
        for node in later {
            let n = node.clone();
            self.per_node(&node, "lease-end-later", move |m| m.end_later(&n));
        }
        self.pump();
    }

    /// The deferred lease-ends of a node whose hub restarted, once it answers again.
    fn end_later(self: &Arc<Self>, node: &str) {
        let lines: Vec<String> = self.ends_later.lock().unwrap().iter().filter(|(n, _)| n == node).map(|(_, l)| l.clone()).collect();
        for line in lines {
            let id = line.split_whitespace().nth(1).unwrap_or("");
            match self.ask1(node, "lease-end", &line) {
                Ok(v) if cleanup_confirmed(&v, id) => {
                    self.sched.lock().unwrap().cleaned(node, id, 0);
                    self.holders.lock().unwrap().retain(|_, owner| owner != id);
                    self.marks.lock().unwrap().remove(id);
                    self.cleanups.lock().unwrap().insert(id.to_string(), v.to_json());
                    self.cleaned.notify_all();
                    self.event(Sev::Info, format!("{id}: cleanup confirmed on {node}"));
                    self.ends_later.lock().unwrap().retain(|(n, l)| !(n == node && *l == line));
                    self.pump();
                }
                _ => self.sched.lock().unwrap().cleanup_failed(node, id),
            }
        }
    }

    fn beat_node(self: &Arc<Self>, node: &str) {
        // the beat names every lease the master holds there: one being set up too (a node back
        // from the master's absence ends one its beat does not name); only a granted one the
        // node does not know is lost
        let (ids, starting) = {
            let s = self.sched.lock().unwrap();
            (s.live_on(node), s.starting_on(node))
        };
        let claim: Vec<&str> = ids.iter().chain(&starting).map(String::as_str).collect();
        let Ok(v) = self.ask1(node, "lease-beat", &format!("lease-beat {}", claim.join(" "))) else { return };
        let now = self.now();
        if let Some(makepad_strict_json::Value::Obj(info)) = v.get("info") {
            let mut s = self.sched.lock().unwrap();
            for (id, i) in info {
                s.set_info(id, &i.to_json(), now);
            }
        }
        for id in strs(v.get("unknown")) {
            if ids.contains(&id) {
                self.sched.lock().unwrap().release(&id, "lost", now);
            }
        }
        // the node's memory watch ended these leases' runs (only a lease it beat there)
        for e in v.get("memory").and_then(|m| m.as_arr()).unwrap_or(&[]) {
            let id = e.get("id").and_then(|x| x.as_str()).unwrap_or("");
            if claim.contains(&id) {
                let why = e.get("why").and_then(|x| x.as_str()).unwrap_or("");
                self.sched.lock().unwrap().end_for_memory(id, why, now);
            }
        }
        // the node's look: this lease took its disk
        if let Some(e) = v.get("end") {
            let text = |k: &str| e.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let id = text("id");
            if ids.contains(&id) {
                self.sched.lock().unwrap().end_for_disk(&id, &text("why"), now);
            }
        }
    }

    /// The lease loop: places every second, beats the nodes and asks their caps at `pace`. Each
    /// node's beat and caps call runs on its own thread: a slow, down or silent box (a just-started
    /// master's links coming up, a hub out of memory) must not starve the other boxes' beats, or
    /// the nodes end their leases after 60 s without one (a master restart ended a lease on a pool box,
    /// 2026-10-30; one node's silent hub ended leases fleet-wide on 2026-11-02).
    pub fn start(self: &Arc<Self>, hosts: Hosts, pace: Pace) {
        self.resume_starts();
        *self.awake.lock().unwrap() = Some((std::time::Instant::now(), self.now()));
        // A restarted master heard no one yet: its start counts as each working agent's call, so
        // a silence is judged from it. Its clients call again within a minute (`agent-seen`, a
        // `job wait` asked again); judged from their asks, every agent with work older than
        // [`DETACHED_MS`] was said detached a second after each restart (11-13).
        let (now, agents) = (self.now(), self.view().into_iter().filter_map(|l| l.req.agent).collect::<Vec<_>>());
        let mut seen = self.agents_seen.lock().unwrap();
        for agent in agents { seen.entry(agent).or_insert(now); }
        drop(seen);
        let m = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease-sched".into()).spawn(move || loop {
            m.pump();
            m.say_detached(m.now());
            std::thread::sleep(Duration::from_secs(1));
        });
        let m = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease-beat".into()).spawn(move || loop {
            m.beat_nodes();
            std::thread::sleep(pace.beat);
        });
        let m = self.clone();
        let _ = std::thread::Builder::new().name("devhub-lease-nodes".into()).spawn(move || loop {
            let hosts = hosts();
            m.refresh(&hosts);
            m.note_network(crate::trust::network_problem(hosts.iter().filter(|(name, _)| *name != m.machine).map(|(_, addr)| addr.as_str())));
            std::thread::sleep(pace.caps);
        });
    }

    /// What keeps this machine's own network from reaching its hubs ([`crate::trust::network_problem`]):
    /// its boxes would read as down one by one and each for its own reason, when the one thing to
    /// fix is here (an address change stopped every box for 15+ min, 11-09). Said once per change.
    pub fn note_network(&self, problem: Option<String>) {
        self.sched.lock().unwrap().own_network(problem.clone());
        let mut said = self.network.lock().unwrap();
        if *said == problem { return; }
        match &problem {
            Some(p) => self.event(Sev::Warn, p.clone()),
            None => self.event(Sev::Info, "this machine's network reaches its hubs again".to_string()),
        }
        *said = problem;
    }

    /// Informational self-update progress in `nodes`; placement continues unchanged, except
    /// `drain`: an older hub (protocol < 6) with its update ready takes no new lease until it
    /// restarts. Ending the progress ends the drain.
    pub fn set_updating(self: &Arc<Self>, node: &str, why: Option<&str>, drain: bool) {
        let mut s = self.sched.lock().unwrap();
        match why {
            Some(w) => s.updating.insert(node.to_string(), w.to_string()),
            None => s.updating.remove(node),
        };
        if drain && why.is_some() { s.update_drain.insert(node.to_string()); } else { s.update_drain.remove(node); }
        drop(s);
        if !drain { self.pump(); }
    }

    /// The tips the updates move to (pin keys, [`crate::lease::Pin::key`]) and the recent landings, oldest first
    /// (the deployment each left and when), read by the master's roll: how far behind a box is
    /// and where it catches up to ([`Sched::pin_updates`]).
    pub fn set_tips(&self, tips: &str, landed: Vec<(String, u64)>) {
        let mut s = self.sched.lock().unwrap();
        s.tips = tips.to_string();
        s.landed = landed;
    }

    /// See [`Sched::update_pin`].
    pub fn update_pin(&self, node: &str) -> Option<(String, String)> {
        self.sched.lock().unwrap().update_pin(node)
    }

    /// An update a running timed lease made wait on `node` (`what`: its deployment, its hub's
    /// restart) and why, said by its asker; None once it moved or its asker stopped asking. The
    /// next timed lease there starts on what the box has, unless the box catches up.
    pub fn update_waits(self: &Arc<Self>, node: &str, what: &'static str, why: Option<String>) {
        let mut s = self.sched.lock().unwrap();
        let had = s.update_waits.get(node).and_then(|w| w.get(what)).cloned();
        if had == why {
            return;
        }
        match &why {
            Some(w) => {
                s.update_waits.entry(node.to_string()).or_default().insert(what, w.clone());
            }
            None => {
                if let Some(w) = s.update_waits.get_mut(node) {
                    w.remove(what);
                    if w.is_empty() {
                        s.update_waits.remove(node);
                    }
                }
            }
        }
        drop(s);
        match why {
            Some(w) => eprintln!("devhub lease: {node}'s {what} waits for its timed run ({w})"),
            None => {
                eprintln!("devhub lease: {node}'s {what} waits no more");
                self.pump();
            }
        }
    }

    pub fn view(&self) -> Vec<Lease> {
        self.sched.lock().unwrap().view()
    }

    /// The jobs a granted hub build builds with ([`super::sched::Sched::hub_build_jobs`]).
    pub fn hub_build_jobs(&self, id: &str) -> Option<u32> {
        self.sched.lock().unwrap().hub_build_jobs(id)
    }

    /// What a queued lease of the lane waits behind, if it hangs ([`super::sched::Sched::behind_hung`]).
    pub fn behind_hung(&self, lane: &str) -> Option<(String, String)> {
        self.sched.lock().unwrap().behind_hung(lane)
    }

    /// What a queued lease of the lane comes down to, if that is the lane's own session
    /// ([`super::sched::Sched::behind_own`]).
    pub fn behind_own(&self, lane: &str) -> Option<(String, String)> {
        self.sched.lock().unwrap().behind_own(lane)
    }

    /// What a running lease's owner is to hear: its idle verdict or its yield
    /// ([`super::sched::Sched::owner_word`]).
    pub fn owner_word(&self, id: &str) -> Option<String> {
        self.sched.lock().unwrap().owner_word(id)
    }

    /// Its holder `pid` hands running lease `id` to the master's own work ([`Sched::adopt`]),
    /// kept in `leases.live` at once; its box and when it was granted.
    pub fn adopt(&self, id: &str, pid: u32) -> Result<(String, u64), String> {
        let held = self.sched.lock().unwrap().adopt(id, pid)?;
        self.save_live();
        Ok(held)
    }

    pub fn never_places(&self, req: &Req) -> Option<String> {
        self.sched.lock().unwrap().never_places(req)
    }

    /// When a person last gave input at the node ([`super::sched::Sched::user_input_ms`]).
    pub fn user_input_ms(&self, node: &str) -> Option<u64> {
        self.sched.lock().unwrap().user_input_ms(node)
    }

    pub fn nodes(&self) -> Vec<NodeView> {
        let now = self.now();
        self.sched.lock().unwrap().node_view(now)
    }

    /// Routing for the master's JIT-first gate: `class` is the JIT class, without `native:`.
    /// Compares run-phase medians (the command's own time, its build included; not the dev JIT build).
    /// A cargo run of `native:<class>` is routing's own pick when this answers false.
    pub fn test_route(&self, kind: &str, class: &str, own: bool) -> (bool, String) {
        let (jit, cargo) = {
            let s = self.sched.lock().unwrap();
            (s.class_run_median(class, kind), s.class_run_median(&format!("native:{class}"), kind))
        };
        super::client::test_route(own, jit, cargo)
    }

    pub(crate) fn jit_compilers(&self) -> Vec<String> {
        self.sched.lock().unwrap().jit_compilers()
    }

    #[cfg(feature = "checkout")]
    pub(crate) fn note_warm(&self, lane: &str, node: &str) {
        self.sched.lock().unwrap().note_warm(lane, node);
    }

    #[cfg(feature = "checkout")]
    pub(crate) fn forget_warm(&self, lane: &str, node: &str) {
        self.sched.lock().unwrap().forget_warm(lane, node);
    }

    /// The manager's bump of a queued lease (by id, or the first queued one `pick` matches):
    /// the front of its queue class. Its id.
    pub fn bump(self: &Arc<Self>, id: Option<&str>, pick: &dyn Fn(&super::sched::Req) -> bool) -> Result<String, String> {
        let r = {
            let mut sched = self.sched.lock().unwrap();
            let id = match id {
                Some(id) => id.to_string(),
                None => sched.view().into_iter().find(|l| l.state == super::sched::State::Queued && pick(&l.req)).map(|l| l.id).ok_or("no queued lease")?,
            };
            sched.bump(&id).map(|_| id)
        };
        if r.is_ok() { self.persist()?; self.pump(); }
        r
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

    fn durable_state_line(&self, id: &str) -> Result<Option<String>, String> {
        let mut saved = self.saved.lock().unwrap();
        let s = self.sched.lock().unwrap();
        let text: String = s.live_lines().iter().map(|l| format!("{l}\n")).collect();
        if *saved != text {
            crate::state::durable_write(&self.home.join("leases.live"), text.as_bytes())?;
            *saved = text;
        }
        Ok(s.get(id).map(|l| l.state_line(self.now(), &l.node.as_deref().map(|n| s.addr(n)).unwrap_or_default())))
    }

    /// An overlay request that names no JIT: the ids its pin's JIT has, which placement prefers.
    fn with_hint(&self, req: Req) -> Req {
        #[cfg(feature = "checkout")]
        {
            let mut req = req;
            if req.src == super::Src::Overlay && req.on_node() && req.hint.is_empty() {
                if let (Some(pin), Some(repo_dir)) = (req.rev.as_deref().and_then(super::Pin::of_rev), req.repo_dir.as_deref()) {
                    let compilers = self.sched.lock().unwrap().jit_compilers();
                    match super::loaders::commit_ids(Path::new(repo_dir), &pin.makepad, &compilers) {
                        Ok(ids) => req.hint = ids,
                        Err(e) => self.event(Sev::Info, format!("lane {}: JIT placement hint: {e}", req.lane)),
                    }
                }
            }
            req
        }
        #[cfg(not(feature = "checkout"))]
        req
    }

    /// The running leases over [`OVERDUE_TIMES`] their measure whose agent made no devhub call for
    /// [`AGENT_SILENT_MS`], as `lane quiet` lists them: an agent ended its turn "waiting for the
    /// verdict" while its lease ran 41 min against a measured 2m17s, and its box's two "stalled?"
    /// warnings reached no one (L519423, 11-06). A lease's own ask counts as its agent's call (a
    /// restarted master heard none since). The newest first.
    pub fn silent_overdue(&self, now: u64) -> Vec<String> {
        let mut over = self.sched.lock().unwrap().overdue(now, OVERDUE_TIMES);
        over.sort_by_key(|(l, ..)| std::cmp::Reverse(l.granted_ms));
        let seen = self.agents_seen.lock().unwrap();
        over.into_iter().filter_map(|(l, ran, measure, against)| {
            let agent = l.req.agent.as_deref()?;
            let silent = now.saturating_sub(seen.get(agent).copied().unwrap_or(0).max(l.asked_ms));
            (silent > AGENT_SILENT_MS).then(|| format!("{}: {} {}@{} running {} against {against} ({}x), its agent {agent} made no devhub call for {} min: `devhub cancel {}` frees the box", l.req.lane, l.id, l.req.kind.as_str(), l.node.as_deref().unwrap_or("?"), super::sched::dur(ran), ran / measure.max(1), silent / 60_000, l.id))
        }).collect()
    }

    /// Agents with granted work and nothing of their own talking to devhub, newest silence last.
    pub fn detached(&self, now: u64) -> Vec<String> {
        self.detached_agents(now).into_iter().map(|(_, _, row)| row).collect()
    }

    fn detached_agents(&self, now: u64) -> Vec<(String, u64, String)> {
        let leases = self.view();
        // a client of its own that holds one of its leases, alive (a `devhub test`, `run` or `lease`
        // waiting on its lease, queued or running), is the agent waiting: it wakes when that ends.
        // A lane's `devhub test` streaming its run's log read as "nothing talked for 30 min" (11-13).
        let waiting: std::collections::HashSet<&str> = leases.iter().filter(|l| l.req.holder.is_some_and(crate::procinfo::alive)).filter_map(|l| l.req.agent.as_deref()).collect();
        let seen = self.agents_seen.lock().unwrap();
        let mut agents: BTreeMap<&str, (u64, Vec<&Lease>)> = BTreeMap::new();
        for l in &leases {
            if l.req.agent.as_deref().is_some_and(|a| waiting.contains(a)) { continue; }
            if l.state != State::Granted || l.req.kind == Kind::Gate || l.req.upkeep() || l.req.hub_build() { continue; }
            let Some(agent) = l.req.agent.as_deref().filter(|a| makepad_devhub_conf::valid_name(a) && !a.starts_with("devhub-")) else { continue };
            let entry = agents.entry(agent).or_insert_with(|| (seen.get(agent).copied().unwrap_or(0), Vec::new()));
            entry.0 = entry.0.max(l.asked_ms);
            entry.1.push(l);
        }
        drop(seen);
        let mut rows: Vec<_> = agents.into_iter().filter_map(|(agent, (heard, leases))| {
            let silent = now.saturating_sub(heard);
            if silent < DETACHED_MS { return None; }
            let running: Vec<_> = leases.iter().map(|l| format!("{} {}@{} running {}", l.id, l.req.kind.as_str(), l.node.as_deref().unwrap_or("?"), super::sched::dur(now.saturating_sub(l.granted_ms.unwrap_or(l.asked_ms))))).collect();
            Some((agent.to_string(), heard, format!("{agent}: {}; nothing of {agent}'s has talked to devhub for {} min (no job wait, lease client or live call): it will not wake on them, resume it", running.join(", "), silent / 60_000)))
        }).collect();
        rows.sort_by_key(|(_, heard, _)| *heard);
        rows
    }

    /// Once per spell of silence; forget agents when their granted work or silence ends.
    fn say_detached(&self, now: u64) {
        let rows = self.detached_agents(now);
        let mut said = self.detached_said.lock().unwrap();
        let current: HashMap<_, _> = rows.iter().map(|(agent, heard, _)| (agent.clone(), *heard)).collect();
        said.retain(|agent, _| current.contains_key(agent));
        let warnings: Vec<_> = rows.into_iter().filter_map(|(agent, heard, row)| {
            (said.insert(agent, heard) != Some(heard)).then_some(row)
        }).collect();
        drop(said);
        for row in warnings { self.event(Sev::Warn, row); }
    }

    /// `agent` talks to devhub now: an open wait on its jobs ([`crate::orch::job`]) is its, whatever
    /// the environment of the process that waits (one started without `DEVHUB_AGENT` says no
    /// `agent-seen`), and it asks again after a master restart. A lane's background `job wait`
    /// read as "nothing talked for 62 min" (11-13).
    pub fn agent_heard(&self, agent: &str) {
        if makepad_devhub_conf::valid_name(agent) { self.agents_seen.lock().unwrap().insert(agent.to_string(), self.now()); }
    }

    /// When `agent` last made a devhub call (0: not since this master started).
    pub fn agent_seen_ms(&self, agent: &str) -> u64 {
        self.agents_seen.lock().unwrap().get(agent).copied().unwrap_or(0)
    }

    /// One control service request (the line may repeat the service word, like `ps`).
    pub fn serve(self: &Arc<Self>, service: &str, line: &str, out: &mut Vec<String>) {
        let now = self.now();
        let line = line.strip_prefix(service).filter(|r| r.is_empty() || r.starts_with(' ')).map_or(line, str::trim_start);
        match service {
            // a devhub call of this agent's (its CLI says it at most once a minute)
            "agent-seen" => match line.trim() {
                agent if makepad_devhub_conf::valid_name(agent) => {
                    self.agents_seen.lock().unwrap().insert(agent.to_string(), now);
                    // what devhub learned for it since (a bucket red naming its landing), said once
                    let wall = proto::wall_ms();
                    out.extend(crate::notes::take(&self.home, agent).into_iter().map(|(ms, line)| crate::notes::said(agent, wall.saturating_sub(ms), &line)));
                    out.push("{}".into());
                }
                agent => out.push(err(&format!("agent-seen: {agent:?} is no agent name"))),
            },
            "lease" => match Req::parse(line).and_then(|req| self.from_its_worktree(&req).and_then(|_| bind_lane(&self.home, &req)).and_then(|_| whole_pair(&req)).map(|_| req)) {
                Ok(req) => {
                    if let Some(agent) = &req.agent { self.agents_seen.lock().unwrap().insert(agent.clone(), now); }
                    let req = self.with_hint(req);
                    let accepted = {
                        let mut saved = self.saved.lock().unwrap();
                        self.sched.lock().unwrap().request_durable(req, now, |lines| {
                            let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
                            crate::state::durable_write(&self.home.join("leases.live"), text.as_bytes())?;
                            *saved = text;
                            Ok(())
                        })
                    };
                    let id = match accepted { Ok(id) => id, Err(e) => { out.push(err(&e)); return; } };
                    self.pump();
                    match self.durable_state_line(&id) { Ok(Some(line)) => out.push(line), Ok(None) => {}, Err(e) => out.push(err(&e)) }

                }
                Err(e) => out.push(err(&e)),
            },
            // `retarget <id> <request>`: a queued request's newer target, in place
            "retarget" => {
                let (id, json) = line.split_once(' ').unwrap_or((line, ""));
                let moved = Req::parse(json).map(|req| self.with_hint(req)).and_then(|req| self.sched.lock().unwrap().retarget(id, req));
                if let Err(e) = moved.and_then(|_| self.persist()) { out.push(err(&e)); return; }
                self.pump();
                match self.durable_state_line(id) { Ok(Some(line)) => out.push(line), Ok(None) => {}, Err(e) => out.push(err(&e)) }
            }
            "beat" => {
                let beaten = self.sched.lock().unwrap().beat(line.trim(), now);
                if let Some(at_beat) = beaten {
                    match self.durable_state_line(line.trim()) {
                        // its holder hears the idle verdict or its yield too, not the inbox alone (`Beat`);
                        // `hung`: the idle verdict, on which a run's client hands its run to a job
                        Ok(Some(state)) => out.push(match self.owner_word(line.trim()) {
                            Some(word) => format!("{},\"idle\":{}{}}}", state.strip_suffix('}').unwrap_or(&state), json_str(&word), if self.sched.lock().unwrap().hung_word(line.trim()).is_some() { ",\"hung\":true" } else { "" }),
                            None => state,
                        }),
                        // an empty answer, said with the lease as its beat found it (a live test's
                        // session failed on "beat: no answer", unexplained: W4322, 11-08)
                        Ok(None) => eprintln!("devhub lease: beat {}: answered with nothing: the lease left the scheduler after its beat; at its beat: {}", line.trim(), crate::proto::clean(&at_beat, 600)),
                        Err(e) => out.push(err(&e)),
                    }
                } else { out.push(err(&format!("no lease {}", line.trim()))); }
            },
            "release" | "cancel" => {
                let mut w = line.split_whitespace();
                let id = w.next().unwrap_or("");
                // `cancel`: the user's end of one lease, or a lane's or agent's (`by=`): only the
                // user's keeps the lane off that node for a while. Read as a release's `how`, the
                // `by=` left every agent's cancel the user's (a coordinator's cancel of a hung
                // tests shard kept the bucket off its box 30 min, 11-08).
                let rest: Vec<&str> = w.collect();
                let by = rest.iter().find_map(|x| x.strip_prefix("by=")).unwrap_or("user").to_string();
                let how = rest.first().copied().filter(|h| !h.starts_with("exit=") && !h.starts_with("by=")).unwrap_or("released");
                let ended = if service == "cancel" { self.sched.lock().unwrap().cancel_by(id, &by, now) } else { self.sched.lock().unwrap().release(id, how, now) };
                if ended {
                    if let Err(e) = self.persist() { out.push(err(&e)); return; }
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
                    out.push(s.lease_view_line(&l, now));
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
                    let line = if action == "closed" { format!("JIT miss {id} closed; the next devhub test tries the JIT again") } else if action == "retried" { format!("JIT miss {id} retried: the JIT changed since it last failed there; the next devhub test runs the JIT") } else { crate::jit_misses::notice(value) };
                    (self.publish)(hub_record(&self.machine, "", 0, "jit_miss", if matches!(action, "closed" | "retried") { Sev::Info } else { Sev::Warn }, line, value.to_json()));
                }) {
                    Ok(v) => out.push(v),
                    Err(e) => out.push(err(&e)),
                },
                ["stats"] | ["stats", "--day"] => match super::jit::stats(&self.home.join("costs.jsonl"), &self.nodes(), line.contains("--day"), now) {
                    Ok(text) => out.push(format!("{{\"text\":{}}}", json_str(&text))),
                    Err(e) => out.push(err(&e)),
                },
                // the compilers the nodes' JIT stores report, the order of a lease's JIT ids
                // (`hint`, `own_jit`): its client reads its own off what it ships
                ["compilers"] => out.push(format!("{{\"compilers\":[{}]}}", self.jit_compilers().iter().map(|c| json_str(c)).collect::<Vec<_>>().join(","))),
                _ => out.push(err("jit stats [--day] | misses [--all] | misses close <id> | compilers")),
            },
            #[cfg(feature = "checkout")]
            "native" => match super::native::serve(&self.home, line, now, &self.nodes(), &|class| { let (jit, why) = self.test_route("test", class, false); (!jit).then_some(why) }) {
                Ok(v) => out.push(v),
                Err(e) => out.push(err(&e)),
            },
            "stats" => match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["class", kind, _, ..] => {
                    // tab separated: a class can hold spaces
                    let classes: Vec<&str> = line.splitn(3, ' ').nth(2).unwrap_or("").split('\t').collect();
                    let s = self.sched.lock().unwrap();
                    let runs = classes.iter().map(|class| format!("{}:{}", json_str(class), s.class_run_median(class, kind).map_or("null".into(), |ms| ms.to_string()))).collect::<Vec<_>>().join(",");
                    out.push(format!("{{\"runs\":{{{runs}}}}}"));
                }
                ["queue"] | ["queue", "--hour"] => {
                    let leases = {
                        let s = self.sched.lock().unwrap();
                        let mut leases = s.view();
                        leases.extend(s.recent_ended(usize::MAX));
                        leases
                    };
                    match queue_stats(&self.home.join("leases.jsonl"), &leases, now).and_then(|t| Ok(super::prof::agent(&self.home, &leases, 1, None, now, false)? + &t)) {
                        Ok(text) => out.push(format!("{{\"text\":{}}}", json_str(&text))),
                        Err(e) => out.push(err(&e)),
                    }
                }
                ["cores"] => {
                    let text = self.sched.lock().unwrap().cores_over(now);
                    out.push(format!("{{\"text\":{}}}", json_str(&text)));
                }
                ["calls"] => match call_stats(&self.home.join("costs.jsonl"), now) {
                    Ok(text) => out.push(format!("{{\"text\":{}}}", json_str(&text))),
                    Err(e) => out.push(err(&e)),
                },
                _ => out.push(err("stats queue [--hour] | cores | calls | class <kind> <class>...")),
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
            "nodes" => {
                // (`nodes master`: the master's own build first, for `devhub nodes`)
                if line.split_whitespace().any(|w| w == "master") {
                    let network = self.network.lock().unwrap().clone().unwrap_or_default();
                    out.push(format!("{{\"master\":{},\"hub\":{},\"up_ms\":{},\"network\":{}}}", json_str(&self.machine), json_str(super::caps::hub_rev()), self.up_ms, json_str(&network)));
                }
                out.extend(self.nodes().iter().map(|n| n.to_json()));
            }
            super::panel::SERVICE => super::panel::serve(&self.sched, &self.home.join("leases.jsonl"), line, now, out),
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
                if !makepad_devhub_conf::valid_name(who) {
                    return out.push(err("evict <lane|agent>"));
                }
                let ids = self.sched.lock().unwrap().evict(who, now);
                write_blocked(&self.home, &self.sched.lock().unwrap().blocked);
                self.pump();
                let cleaned: Vec<String> = ids.iter().map(|id| format!("{}:{}", json_str(id), self.cleanup_of(id, Duration::from_secs(30)).unwrap_or_else(|| "{\"cleanup\":\"still running\"}".into()))).collect();
                out.push(format!("{{\"evicted\":{},\"ended\":{{{}}},\"blocked\":true}}", json_str(who), cleaned.join(",")));
            }
            "stop" => {
                let lane = makepad_devhub_conf::lane_name(line.trim());
                if !makepad_devhub_conf::valid_name(&lane) {
                    return out.push(err("stop <lane>"));
                }
                let ids = self.sched.lock().unwrap().stop(&lane, now);
                self.pump();
                let cleaned: Vec<String> = ids.iter().map(|id| format!("{}:{}", json_str(id), self.cleanup_of(id, Duration::from_secs(30)).unwrap_or_else(|| "{\"cleanup\":\"still running\"}".into()))).collect();
                out.push(format!("{{\"stopped\":{},\"ended\":{{{}}}}}", json_str(&lane), cleaned.join(",")));
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

    /// A lane driving an app by many input calls (1533 in an hour, a lease each, about 7 moves/s
    /// at best, 11-04) is told once in a while what plays them at frame rate on one call.
    fn input_hint(&self, lane: &str) -> Option<String> {
        let now = self.now();
        let mut calls = self.input_calls.lock().unwrap();
        let (n, minute, told) = calls.entry(lane.to_string()).or_default();
        if *minute != now / 60_000 {
            (*n, *minute) = (0, now / 60_000);
        }
        *n += 1;
        if *n < INPUT_HINT_CALLS || (*told > 0 && now.saturating_sub(*told) < INPUT_HINT_EVERY_MS) { return None; }
        *told = now;
        Some(format!("{{\"hint\":{}}}", json_str(&format!("{n} input calls this minute, a lease each: `input replay path=<file on the box> sync=frame` plays a file of them at frame rate on one call (lines `<ms> <frame> <command>`, platform/src/remote_input_rec.rs; `input record path=<file>` writes one)"))))
    }

    /// `linspect <lease> <node> <pid|app> <cmd...>`: checks the inspect lease, records the
    /// command, forwards it to the JIT, and remembers what it left (breaks, probes, pause).
    fn linspect(self: &Arc<Self>, line: &str, out: &mut Vec<String>) {
        let mut w = line.splitn(4, ' ');
        let (id, node, jit, cmd) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or("").trim());
        if cmd.is_empty() {
            return out.push(err("linspect <lease> <node> <pid|app> <cmd...>"));
        }
        let state = self.marks.lock().unwrap().entry(id.to_string()).or_default().clone();
        let mut m = state.lock().unwrap();
        let (lane, user) = {
            let s = self.sched.lock().unwrap();
            let Some(l) = s.get(id) else { return out.push(err(&format!("no lease {id}"))) };
            if l.state != State::Granted || l.req.on_node() || l.node.as_deref() != Some(node) || l.req.jit.as_deref() != Some(jit) {
                return out.push(err(&format!("{id} is not a live inspect lease on {node}/{jit}")));
            }
            let view = s.node_view(self.now());
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
        let effects = matches!(verb, "break" | "pause" | "continue" | "resume" | "probe")
            && !matches!((verb, cmd.split_whitespace().nth(1)), ("break" | "probe", Some("list")));
        if m.pid == 0 {
            let lines = match (self.ask)(node, "ps", "ps") { Ok(v) => v, Err(e) => return out.push(err(&e)) };
            let pids: Vec<_> = lines.iter().filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()).filter(|v| {
                v.get("app").and_then(|v| v.as_str()) == Some(jit) || v.get("pid").and_then(|v| v.as_u64()).is_some_and(|p| p.to_string() == jit)
            }).filter_map(|v| v.get("pid").and_then(|v| v.as_u64()).and_then(|pid| u32::try_from(pid).ok()).filter(|pid| *pid > 0)).collect();
            if pids.len() != 1 { return out.push(err("inspect: select one live app instance by pid")); }
            m.pid = pids[0];
        }
        let key = (node.to_string(), m.pid);
        if effects {
            let mut holders = self.holders.lock().unwrap();
            if let Some(h) = holders.get(&key).filter(|h| *h != id) {
                return out.push(err(&format!("{node}/{jit}: breakpoints are held by {h}")));
            }
            // Reserve before forwarding; failed/uncertain commands retain ownership until cleanup.
            holders.insert(key, id.to_string());
        }
        if verb == "pause" { m.paused = true; }
        if user || matches!(verb, "break" | "pause" | "continue" | "resume" | "probe") {
            let sev = if stops { Sev::Warn } else { Sev::Info };
            self.event(sev, format!("inspect {node}/{jit} by lane {lane}: {}", proto::clean(cmd, 200)));
        }
        if INPUT_VERBS.contains(&verb) {
            if let Some(hint) = self.input_hint(&lane) { out.push(hint); }
        }
        let answer = match (self.ask)(&format!("{node}:{}", m.pid), "cmd", cmd) {
            Ok(l) => l,
            Err(e) => return out.push(err(&e)),
        };
        if effects && !answer.iter().any(|a| makepad_strict_json::parse(a.as_bytes()).ok().is_some_and(|v| v.get("err").is_some())) {
            let ids: Vec<String> = answer.iter().filter_map(|a| makepad_strict_json::parse(a.as_bytes()).ok()).filter_map(|v| ["id", "probe", "break"].iter().find_map(|k| v.get(k)).and_then(|i| i.as_str().map(str::to_string).or_else(|| super::num(i).map(|n| (n as i64).to_string())))).collect();
            let arg = cmd.split_whitespace().nth(2).unwrap_or("").to_string();
            match (verb, cmd.split_whitespace().nth(1)) {
                ("break", Some("clear")) => m.breaks.retain(|b| *b != arg),
                ("probe", Some("off")) => m.probes.retain(|b| *b != arg),
                ("break", _) => m.breaks.extend(ids),
                ("probe", _) => m.probes.extend(ids),
                ("pause", _) => m.paused = true,
                ("continue" | "resume", _) => m.paused = false,
                _ => {}
            }
        }
        out.extend(answer);
    }
}

/// So many inspect input calls of a lane in one minute bring [`LeaseMaster::input_hint`]'s word...
const INPUT_HINT_CALLS: u32 = 20;
/// ...this seldom.
const INPUT_HINT_EVERY_MS: u64 = 600_000;

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
    let ensured: Arc<Mutex<HashMap<(String, String, String), std::time::Instant>>> = Arc::new(Mutex::new(HashMap::new()));
    #[cfg(feature = "checkout")]
    let fill_client = client.clone();
    #[cfg(feature = "checkout")]
    let rev: Option<EnsureRev> = Some(Arc::new(move |node: &str, repo: &str, dir: &Path, pin: &super::RevPin, fresh: bool| {
        // by repo too: a commit sent as the commercial pin is not in the node's makepad store
        let key = (node.to_string(), repo.to_string(), pin.sha.clone());
        if !fresh && ensured.lock().unwrap().get(&key).is_some_and(|t| t.elapsed() < ENSURED_FOR) {
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
    let lm = LeaseMaster::new(machine, proto::home(), ask, publish, Arc::new(proto::wall_ms), rev, false);
    #[cfg(feature = "checkout")]
    let lm = lm.with_fill_objects(Arc::new(move |node: &str, repo: &str, dir: &Path, ids: &[String]| crate::sync::fill_objects(&fill_client(), node, repo, dir, ids)));
    let lm = Arc::new(lm);
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

    /// `lane quiet` lists a lease past 5x its measure whose agent made no devhub call for 15 min
    /// ([`LeaseMaster::silent_overdue`]): its own ask counts as a call, an `agent-seen` (any devhub
    /// call of its agent's) quiets it, a lease that names no agent has none to be silent.
    #[test]
    fn a_lease_far_over_its_measure_whose_agent_went_silent_is_listed() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-silent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, _node, clock, _recs) = master(&dir);
        m.refresh(&[("box-a".to_string(), "192.0.2.89:8797".to_string()), ("box-b".to_string(), "192.0.2.90:8797".to_string())]);
        let ask = |lane: &str, agent: Option<&str>| {
            let mut r = Req::new(lane, Kind::Test);
            (r.est_min, r.agent, r.node) = (Some(2.0), agent.map(str::to_string), Some(if agent.is_some() { "box-a" } else { "box-b" }.to_string()));
            serve(&m, "lease", &r.to_json())
        };
        assert!(ask("waits", Some("AGENT-W")).contains("\"state\":\"granted\""));
        assert!(ask("anon", None).contains("\"state\":\"granted\""));
        let at = |ms: u64| { clock.store(1_000 + ms, Ordering::SeqCst); m.silent_overdue(1_000 + ms) };
        assert_eq!(at(15 * 60_000), Vec::<String>::new(), "past 5x its 2 min, its ask 15 min ago, no more");
        assert_eq!(at(16 * 60_000), ["waits: L1 test@box-a running 16m00s against its declared 2m00s (8x), its agent AGENT-W made no devhub call for 16 min: `devhub cancel L1` frees the box"]);
        // a bucket red left for it since is said in the answer, once (crate::notes)
        crate::notes::leave(&dir, "AGENT-W", "bucket fast FAILED check p").unwrap();
        let said = serve(&m, "agent-seen", "AGENT-W");
        assert!(said.starts_with("{\"say\":\"devhub, for AGENT-W (just now): bucket fast FAILED check p\",\"waiting\":false}\n{}"), "{said}");
        assert_eq!(serve(&m, "agent-seen", "AGENT-W"), "{}");
        assert_eq!(at(30 * 60_000), Vec::<String>::new(), "its agent called at 16 min");
        assert_eq!(at(32 * 60_000).len(), 1);
        assert!(serve(&m, "agent-seen", "no agent").contains("\"err\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_agent_whose_running_leases_nothing_of_its_own_waits_on_is_said_once() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-detached-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mut m, _node, clock, recs) = master(&dir);
        Arc::get_mut(&mut m).unwrap().ensure_rev = Some(Arc::new(|_, _, _, pin, _| Ok(pin.sha.clone())));
        m.refresh(&[("box-a".to_string(), "box-a:8797".to_string()), ("box-b".to_string(), "box-b:8797".to_string())]);
        for (lane, agent, node, kind) in [
            ("wait-a", "AGENT-A", "box-a", Kind::Test),
            ("live-a", "AGENT-A", "box-a", Kind::Long),
            ("wait-b", "AGENT-B", "box-b", Kind::Test),
            ("held-c", "AGENT-C", "box-b", Kind::Test),
            ("own", "devhub-x", "box-b", Kind::Test),
            ("gate", "AGENT-G", "box-b", Kind::Gate),
            ("prebuild-box-a", "AGENT-U", "box-a", Kind::Build),
            ("hub-build-box-b", "AGENT-H", "box-b", Kind::Build),
        ] {
            let mut req = Req::new(lane, kind);
            req.src = super::super::Src::Overlay;
            req.rev = Some(format!("work@{}", "a".repeat(40)));
            req.agent = Some(agent.to_string());
            req.node = Some(node.to_string());
            req.cores = Some(1.0);
            req.ram_gb = Some(1.0);
            req.est_min = Some(30.0);
            req.live = lane == "live-a";
            // its client holds it, alive (this test): the agent waits on it
            if agent == "AGENT-C" { req.holder = Some(std::process::id()); }
            if agent == "AGENT-H" { req.master = true; req.class = super::super::sched::HUB_BUILD.to_string(); }
            let answer = serve(&m, "lease", &req.to_json());
            assert!(answer.contains("\"state\":\"granted\""), "{lane}: {answer}");
        }
        let at = |ms: u64| {
            clock.store(1_000 + ms, Ordering::SeqCst);
            m.say_detached(1_000 + ms);
            m.detached(1_000 + ms)
        };
        let warnings = |agent: &str| recs.lock().unwrap().iter().filter(|r| r.sev == Sev::Warn && r.line.starts_with(&format!("{agent}:"))).map(|r| r.line.clone()).collect::<Vec<_>>();
        assert!(at(4 * 60_000 + 59_000).is_empty(), "not yet detached");
        assert!(warnings("AGENT-A").is_empty() && warnings("AGENT-B").is_empty());
        let rows = at(5 * 60_000 + 1_000);
        assert_eq!(rows, [
            "AGENT-A: L1 test@box-a running 5m01s, L2 long@box-a running 5m01s; nothing of AGENT-A's has talked to devhub for 5 min (no job wait, lease client or live call): it will not wake on them, resume it",
            "AGENT-B: L3 test@box-b running 5m01s; nothing of AGENT-B's has talked to devhub for 5 min (no job wait, lease client or live call): it will not wake on them, resume it",
        ]);
        assert_eq!(warnings("AGENT-A"), rows[..1]);
        assert_eq!(warnings("AGENT-B"), rows[1..]);
        assert_eq!(m.detached_said.lock().unwrap().len(), 2, "no warning for excluded leases");
        at(6 * 60_000);
        assert_eq!(serve(&m, "agent-seen", "agent-seen AGENT-A"), "{}");
        let rows = at(7 * 60_000);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].starts_with("AGENT-B: L3 "));
        assert_eq!((warnings("AGENT-A").len(), warnings("AGENT-B").len()), (1, 1));
        let rows = at(11 * 60_000 + 1_000);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].starts_with("AGENT-B:") && rows[1].starts_with("AGENT-A:"), "newest silence last: {rows:?}");
        assert_eq!((warnings("AGENT-A").len(), warnings("AGENT-B").len()), (2, 1));
        at(12 * 60_000);
        assert_eq!((warnings("AGENT-A").len(), warnings("AGENT-B").len()), (2, 1), "said once per silence");
        for id in ["L1", "L2", "L3"] { m.sched.lock().unwrap().release(id, "done", m.now()); }
        assert!(at(13 * 60_000).is_empty());
        assert!(m.detached_said.lock().unwrap().is_empty(), "ended leases leave no warning state");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The master says what is wrong with its own network once, and says it on its line of `devhub
    /// nodes` for as long as it is so. A box's line says what its caps call said.
    #[test]
    fn the_masters_own_network_is_said_once_and_on_its_line() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-network-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, _node, _clock, recs) = master(&dir);
        let warned = |recs: &Mutex<Vec<Record>>| recs.lock().unwrap().iter().filter(|r| r.sev == Sev::Warn && r.line.contains("bound address")).count();
        let line = |m: &Arc<LeaseMaster>| serve(m, "nodes", "nodes master").lines().next().unwrap().to_string();
        m.note_network(None);
        assert!(line(&m).contains("\"network\":\"\"") && warned(&recs) == 0, "nothing to say: {}", line(&m));
        let off = "this Mac's address is not the credential's bound address";
        for _ in 0..3 { m.note_network(Some(off.to_string())); }
        assert_eq!(warned(&recs), 1, "one event, not one per look");
        assert!(line(&m).contains(off), "{}", line(&m));
        m.note_network(None);
        assert!(line(&m).contains("\"network\":\"\""), "{}", line(&m));
        assert_eq!(warned(&recs), 1, "being back asks nothing of anybody");
        assert!(recs.lock().unwrap().last().is_some_and(|r| r.sev == Sev::Info));
        m.note_network(Some(off.to_string()));
        assert_eq!(warned(&recs), 2, "off again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_caps_miss_says_its_error_or_its_hubs_refusal() {
        assert_eq!(caps_miss(&Err("link lost: Host is down".to_string())), "link lost: Host is down");
        assert_eq!(caps_miss(&Ok(vec![r#"{"err":"credential `fleet-mac` has no rights on this hub"}"#.to_string()])), "credential `fleet-mac` has no rights on this hub");
        assert_eq!(caps_miss(&Ok(Vec::new())), "no answer");
        assert_eq!(caps_miss(&Ok(vec!["not json".to_string()])), "a bad answer: not json");
    }

    #[test]
    fn a_lease_that_took_the_disk_ends_with_that_reason() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _clock, _recs) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        assert!(serve(&m, "lease", &Req::new("big", Kind::Build).to_json()).contains("\"state\":\"granted\""));
        // the node names only what it was beaten for: another lease's id ends nothing
        *node.beat_end.lock().unwrap() = ",\"end\":{\"id\":\"L77\",\"why\":\"x\"}".to_string();
        m.beat_nodes();
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""));
        *node.beat_end.lock().unwrap() = ",\"end\":{\"id\":\"L1\",\"why\":\"it took the box's disk\"}".to_string();
        m.beat_nodes();
        let end = serve(&m, "beat", "L1");
        assert!(end.contains("\"how\":\"disk\"") && end.contains("it took the box's disk"), "{end}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_memory_end_is_a_verdict_and_measures_only_its_peak() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-memory-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _clock, _recs) = master(&dir);
        m.refresh(&[("box-a".to_string(), "192.0.2.89:8797".to_string())]);
        let ask = |class: &str| {
            let mut r = Req::new("big", Kind::Test);
            r.class = class.into();
            serve(&m, "lease", &r.to_json())
        };
        // the node's lease-end answer for a run its memory watch ended: what it held, and why
        let ended = |id: &str| Some(format!("{{\"ended\":\"{id}\",\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[],\"peak_ram_gb\":7.5,\"cpu_seconds\":8,\"memory\":\"it outgrew its memory weight\"}}"));
        assert!(ask("memory-run").contains("\"state\":\"granted\""), "the lease is granted");
        *node.end_answer.lock().unwrap() = ended("L1");
        *node.beat_end.lock().unwrap() = ",\"memory\":[{\"id\":\"L77\",\"why\":\"another lease\"}]".to_string();
        m.beat_nodes();
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""), "an id not beaten here ends nothing");
        *node.beat_end.lock().unwrap() = ",\"memory\":[{\"id\":\"L1\",\"why\":\"it outgrew its memory weight\"}]".to_string();
        m.beat_nodes();
        let end = serve(&m, "beat", "L1");
        assert!(end.contains("\"how\":\"memory\"") && end.contains("it outgrew its memory weight"), "the node's beat ends the lease for memory, with its why: {end}");
        // its holder heard the verdict in the run's log and released before the node's beat came
        node.beat_end.lock().unwrap().clear();
        assert!(ask("raced-run").contains("\"state\":\"granted\""), "the second lease is granted");
        *node.end_answer.lock().unwrap() = ended("L2");
        serve(&m, "release", "L2 released exit=137");
        let end = serve(&m, "beat", "L2");
        assert!(end.contains("\"how\":\"memory\"") && end.contains("it outgrew its memory weight"), "a release before the node's beat is a memory end too: {end}");
        let costs = std::fs::read_to_string(dir.join("costs.jsonl")).unwrap();
        assert_eq!(costs.lines().filter(|l| l.contains("\"how\":\"memory\"")).count(), 2, "both ends are recorded as memory ends: {costs}");
        assert!(costs.lines().all(|l| l.contains("\"os\":\"linux\"")), "each record says its box's OS family: {costs}");
        // a cancelled run beside them still measures nothing; a box's family weighs on its own
        // peaks once it has any, every box's (a record from before, saying no family) until then
        let mut file = std::fs::OpenOptions::new().append(true).open(dir.join("costs.jsonl")).unwrap();
        std::io::Write::write_all(&mut file, b"{\"class\":\"cancelled-run\",\"kind\":\"test\",\"node\":\"box-a\",\"wall_ms\":1000,\"run_ms\":900,\"cpu_seconds\":8,\"peak_ram_gb\":7.5,\"how\":\"cancelled\"}\n").unwrap();
        // a script's memory end measures nothing (a lane's, a tests bucket's): its class is a
        // script's name, what ran away in it one of the things it runs
        std::io::Write::write_all(&mut file, b"{\"class\":\"test bash run.sh @lane\",\"kind\":\"test\",\"node\":\"box-a\",\"os\":\"linux\",\"lane\":\"lane\",\"wall_ms\":1000,\"peak_ram_gb\":49.9,\"how\":\"memory\"}\n").unwrap();
        std::io::Write::write_all(&mut file, b"{\"class\":\"test bash land_gate.sh testrun @landq-tests\",\"kind\":\"test\",\"node\":\"box-a\",\"os\":\"linux\",\"lane\":\"landq-tests\",\"wall_ms\":1000,\"peak_ram_gb\":45.0,\"how\":\"memory\"}\n").unwrap();
        // and a released run whose last run did not end by itself measures nothing either
        std::io::Write::write_all(&mut file, b"{\"class\":\"crashed-run\",\"kind\":\"test\",\"node\":\"box-a\",\"wall_ms\":1000,\"run_ms\":900,\"cpu_seconds\":8,\"peak_ram_gb\":7.5,\"how\":\"released\",\"unmeasured\":\"L9 run 1 ended without its summary: killed by signal 9 (SIGKILL)\"}\n").unwrap();
        std::io::Write::write_all(&mut file, b"{\"class\":\"family-run\",\"kind\":\"test\",\"node\":\"box-b\",\"wall_ms\":1000,\"peak_ram_gb\":7.5,\"how\":\"released\"}\n").unwrap();
        let mut req = Req::new("big", Kind::Test);
        req.class = "family-run".into();
        m.refresh_costs();
        assert_eq!(m.sched.lock().unwrap().measured_ram(&req, Some("box-a")), Some(9.0), "no Linux peaks yet: every box's");
        std::io::Write::write_all(&mut file, b"{\"class\":\"family-run\",\"kind\":\"test\",\"node\":\"box-c\",\"os\":\"linux\",\"wall_ms\":1000,\"peak_ram_gb\":2.0,\"how\":\"released\"}\n").unwrap();
        m.refresh_costs();
        assert_eq!(m.sched.lock().unwrap().measured_ram(&req, Some("box-a")), Some(2.4), "Linux boxes' own peak and a fifth");
        assert_eq!(m.sched.lock().unwrap().measured_ram(&req, None), Some(9.0), "no box: every box's");
        // a live session weighs its own lane's runs of the class
        std::io::Write::write_all(&mut file, b"{\"class\":\"family-run\",\"kind\":\"test\",\"node\":\"box-c\",\"os\":\"linux\",\"lane\":\"wow\",\"wall_ms\":1000,\"peak_ram_gb\":1.0,\"how\":\"released\"}\n").unwrap();
        m.refresh_costs();
        let mut live = Req::new("wow", Kind::Test);
        (live.class, live.live) = ("family-run".into(), true);
        assert_eq!(m.sched.lock().unwrap().measured_ram(&live, Some("box-a")), Some(1.2), "its lane's own peak and a fifth");
        assert_eq!(m.sched.lock().unwrap().measured_ram(&req, Some("box-a")), Some(2.4), "a run of another lane: the family's");
        // a lease's JIT build measures such builds' RAM, however its lease ended
        std::io::Write::write_all(&mut file, b"{\"class\":\"built-run\",\"kind\":\"test\",\"node\":\"box-c\",\"os\":\"linux\",\"wall_ms\":1000,\"jit_peak_gb\":5.0,\"peak_ram_gb\":6.0,\"how\":\"cancelled\"}\n").unwrap();
        m.refresh_costs();
        assert_eq!(m.sched.lock().unwrap().jit_build_ram("box-a"), Some(6.0), "its peak and a fifth");
        m.refresh_costs();
        let s = m.sched.lock().unwrap();
        for class in ["memory-run", "raced-run"] {
            let mut req = Req::new("big", Kind::Test);
            req.class = class.into();
            assert_eq!(s.measured_ram(&req, None), Some(9.0), "{class}: a memory end measures its class's peak (and a fifth)");
            assert_eq!(s.class_median(class, "test"), None, "{class}: its cut wall is no measure");
            assert_eq!(s.class_run_median(class, "test"), None, "{class}: its cut run is no measure");
        }
        let mut req = Req::new("big", Kind::Test);
        req.class = "cancelled-run".into();
        assert_eq!(s.measured_ram(&req, None), None, "a cancelled run still gives no peak");
        req.class = "crashed-run".into();
        assert_eq!(s.measured_ram(&req, None), None, "a crashed run gives no peak");
        for class in ["test bash run.sh @lane", "test bash land_gate.sh testrun @landq-tests"] {
            req.class = class.into();
            assert_eq!(s.measured_ram(&req, Some("box-a")), None, "{class}: a script's memory end gives no peak");
        }
        assert_eq!(s.class_median("crashed-run", "test"), None, "a crashed run's wall is no measure");
        assert_eq!(s.measured_cores(&req), None, "a crashed run's cores are no measure");
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn desk_links_list_the_listening_endpoints() {
        let view = |name: &str, addr: &str, up: bool, desk: Option<(bool, u16)>| NodeView {
            name: name.into(),
            addr: addr.into(),
            up,
            dev: false,
            dev_set: false,
            caps: Some(Caps { desk_link: desk.map(|(on, port)| crate::embed::DeskLinkState { on, port, note: String::new() }), ..Default::default() }),
            unheard: None,
            leases: 0,
            waiting: 0,
            draining: false,
            hold: None,
            updating: None,
            update_waits: None,
            update_pin: None,
            timed: None,
            latency: String::new(),
            live_over: None,
        };
        let nodes = [view("headset", "192.0.2.85:8788", true, Some((true, 8790))), view("phone", "192.0.2.50", true, Some((true, 0))), view("off", "192.0.2.51", true, Some((false, 8790))), view("gone", "192.0.2.52", false, Some((true, 8790))), view("node-a", "192.0.2.89", true, None)];
        assert_eq!(desk_links(&nodes), "headset 192.0.2.85:8790\n");
    }

    /// A node that answers the node services and remembers what it was asked.
    struct FakeNode {
        calls: Mutex<Vec<(String, String, String)>>,
        known: Mutex<Vec<String>>,
            /// its hub is gone (restarting): every call fails as a lost link
        gone: std::sync::atomic::AtomicBool,
        paused: std::sync::atomic::AtomicBool,
        breaks: Mutex<Vec<String>>,
        end_answer: Mutex<Option<String>>,
        inspect_gate: Mutex<Option<(std::sync::mpsc::SyncSender<()>, std::sync::mpsc::Receiver<()>)>>,
        /// the outage its WM recorded: in its next caps answer only
        trouble: Mutex<String>,
        /// its CPU use in its caps answers (None: not measured)
        cpu: Mutex<Option<f64>>,
        /// this box refuses a gui lease's start: its display went
        no_gui: Mutex<String>,
        /// an apply runs there: its caps say so (`seen`), and a timed start is refused
        apply: std::sync::atomic::AtomicBool,
        apply_seen: std::sync::atomic::AtomicBool,
        /// its load measured after a lease's teardown, in its lease-end answers
        end_load: Mutex<Option<(f64, f64)>>,
        /// what its lease-beat answers add (`,"end":{..}`: its look found a lease to end)
        beat_end: Mutex<String>,
        /// the JIT pids its lease-jits names
        jits: Mutex<Vec<u64>>,
        /// what its `lease-weight <id> idle` answers add (`,"call":{..}`: the call it metered)
        call_answer: Mutex<String>,
        /// the cause its JITs' captures find (empty: a main thread that waits)
        capture_cause: Mutex<String>,
}

    impl FakeNode {
        fn answer(&self, node: &str, service: &str, line: &str) -> Result<Vec<String>, String> {
            self.calls.lock().unwrap().push((node.to_string(), service.to_string(), line.to_string()));
            if self.gone.load(Ordering::SeqCst) {
                return Err(format!("link lost: cannot reach fleet hub `{node}` at 192.0.2.89:8797: Connection refused (os error 111)"));
            }
            let caps = super::super::Caps { features: super::super::FEATURES.iter().map(|f| f.to_string()).collect(), node: node.into(), arch: "x86_64".into(), os: "linux".into(), cores: 16, ram_gb: 64.0, ram_avail_gb: 50.0, disk_free_gb: 300.0, disk_total_gb: 1000.0, disk_floor_gb: 25.0, timed: true, display: Some("wm:makepad-wm-0".into()), ..Default::default() };
            Ok(vec![match service {
                "caps" => {
                    let applying = self.apply.load(Ordering::SeqCst);
                    self.apply_seen.store(applying, Ordering::SeqCst);
                    super::super::Caps { trouble: std::mem::take(&mut *self.trouble.lock().unwrap()), cpu_pct: *self.cpu.lock().unwrap(), applying, ..caps }.to_json()
                }
                "ps" => "{\"pid\":42,\"app\":\"foo\"}".into(),
                "lease-weight" if line.ends_with(" idle") => format!("{{\"weight\":1,\"scopes\":1{}}}", self.call_answer.lock().unwrap()),
                "lease-weight" => "{\"weight\":1,\"scopes\":1}".into(),
                "policy" => "{\"dev\":\"on\"}".into(),
                "lease-start" if *self.no_gui.lock().unwrap() == node => "{\"err\":\"lease-start L1: gui=1: caps has no live WM display\"}".to_string(),
                "lease-start" if self.apply.load(Ordering::SeqCst) && line.contains("\"kind\":\"timed\"") => {
                    format!("{{\"err\":\"the deployment of {node} moves and builds: {}\"}}", super::super::TIMED_AFTER)
                }
                "lease-start" => {
                    let id = line.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
                    self.known.lock().unwrap().push(id.clone());
                    format!("{{\"ok\":1,\"work\":\"/home/me/devhub/lane/x/src\",\"target\":\"/t\",\"tmp\":\"/tmp/{id}\",\"env\":{{\"CARGO_INCREMENTAL\":\"0\"}}}}")
                }
                "lease-beat" => {
                    let known = self.known.lock().unwrap();
                    let (k, u): (Vec<&str>, Vec<&str>) = line.split_whitespace().skip(1).partition(|i| known.iter().any(|x| x == i));
                    format!("{{\"live\":[{}],\"unknown\":[{}]{}}}", k.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","), u.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","), self.beat_end.lock().unwrap().as_str())
                }
                "lease-end" => {
                    if let Some(answer) = self.end_answer.lock().unwrap().clone() { return Ok(vec![answer]); }
                    let id = line.split_whitespace().nth(1).unwrap();
                    if let Some(extra) = line.splitn(4, ' ').nth(3) {
                        let v = makepad_strict_json::parse(extra.as_bytes()).unwrap();
                        let none = makepad_strict_json::parse(b"{\"pid\":42}").unwrap();
                        let i = v.get("inspect").unwrap_or(&none);
                        assert_eq!(i.get("pid").and_then(|p| p.as_u64()), Some(42));
                        for cmd in super::super::exec::inspect_commands(&strs(i.get("breaks")), &strs(i.get("probes")), i.get("paused").and_then(|p| p.as_bool()).unwrap_or(false)) {
                            if cmd == "resume" { self.paused.store(false, Ordering::SeqCst); }
                            if let Some(id) = cmd.strip_prefix("rm ") { self.breaks.lock().unwrap().retain(|b| b != id); }
                        }
                    }
                    self.known.lock().unwrap().retain(|k| k != id);
                    let load = self.end_load.lock().unwrap().map_or(String::new(), |(cpu, avail)| format!(",\"load\":{{\"cpu_pct\":{cpu},\"ram_avail_gb\":{avail}}}"));
                    format!("{{\"ended\":{},\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[]{load}}}", json_str(id))
                }
                "lease-jits" => format!("{{\"pids\":{:?}}}", self.jits.lock().unwrap()),
                "lease-why" => r#"{"text":"pid 7: sleep 3600\n  1x sleep S hrtimer_nanosleep\nrun 3: still running\ncheck_targets: ok\n","stuck":{"chain":"sh > bash land_gate.sh full-a > sleep 3600","pid":7,"s":590},"step":"STEP fuzz 47 s [1-2]"}"#.into(),
                "cmd" if line.starts_with("{\"cmd\":\"capture\"") => {
                    let cause = self.capture_cause.lock().unwrap().clone();
                    format!("{{\"cause\":{},\"path\":\"/c/a.txt\"}}", json_str(if cause.is_empty() { "blocked: main waits" } else { &cause }))
                }
                "cmd" if line.starts_with("{\"cmd\":\"stacks\"") => "{\"threads\":[\"main (T1)\"],\"state\":\"S\",\"frames\":[{\"fn\":\"std::sys::os::futex_wait\"}]}".into(),
                "cmd" => {
                    assert!(node.ends_with(":42"), "command must use the resolved instance");
                    if let Some((entered, resume)) = self.inspect_gate.lock().unwrap().take() {
                        entered.send(()).unwrap();
                        resume.recv().unwrap();
                    }
                    if line == "pause" { self.paused.store(true, Ordering::SeqCst); }
                    if line.starts_with("break set") { self.breaks.lock().unwrap().push("G1".into()); }
                    "{\"id\":\"G1\",\"ok\":true}".into()
                },
                _ => return Err(format!("no service {service}")),
            }])
        }
    }

    fn master(dir: &Path) -> (Arc<LeaseMaster>, Arc<FakeNode>, Arc<AtomicU64>, Arc<Mutex<Vec<Record>>>) {
        let node = Arc::new(FakeNode { beat_end: Default::default(), calls: Mutex::new(Vec::new()), known: Mutex::new(Vec::new()), gone: Default::default(), paused: Default::default(), breaks: Default::default(), end_answer: Default::default(), inspect_gate: Default::default(), trouble: Default::default(), cpu: Default::default(), no_gui: Default::default(), apply: Default::default(), apply_seen: Default::default(), end_load: Default::default(), jits: Default::default(), call_answer: Default::default(), capture_cause: Default::default() });
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

    /// A registered lane's lease from another checkout is refused with the lane's worktree, before
    /// anything is queued or bound; from the lane's worktree, and for a lane the registry does not
    /// hold, it goes in.
    #[test]
    fn a_lane_lease_from_another_checkout_is_refused_with_the_lanes_worktree() {
        let dir = std::env::temp_dir().join(format!("devhub-lane-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // the lane's folder is named apart from the lane: the registry's path decides
        let (home, lane_wt, other) = (dir.join("home"), dir.join("wt/another-name"), dir.join("checkout"));
        for d in [&home, &lane_wt, &other] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (master, ..) = master(&home);
        let registered = lane_wt.to_string_lossy().into_owned();
        master.set_lane_trees(Arc::new(move |lane: &str| (lane == "dhf19").then(|| registered.clone())));
        let ask = |lane: &str, from: &Path| {
            let mut r = Req::new(lane, Kind::Build);
            r.repo_dir = Some(from.to_string_lossy().into_owned());
            serve(&master, "lease", &r.to_json())
        };
        let refused = ask("dhf19", &other);
        // read from the JSON: a Windows path's backslashes are escaped in its text
        let err = makepad_strict_json::parse(refused.as_bytes()).ok().and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string));
        assert!(err.is_some_and(|e| e.contains(&format!("worktree is {}", lane_wt.display()))), "{refused}");
        assert!(master.sched.lock().unwrap().view().is_empty(), "nothing queued");
        assert!(!std::fs::read_to_string(home.join("lanes.conf")).unwrap_or_default().contains("dhf19"), "nothing bound");
        assert!(!ask("dhf19", &lane_wt).contains("\"err\""));
        assert!(!ask("ci-sweep", &other).contains("\"err\""), "a lane the registry does not hold");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persistence_failure_never_acknowledges_or_queues_a_lease() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-write-error-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (master, ..) = master(&dir);
        std::fs::create_dir(dir.join("leases.new")).unwrap();
        let reply = serve(&master, "lease", &Req::new("write-test", Kind::Build).to_json());
        assert!(reply.contains("\"err\"") && reply.contains("persist"), "{reply}");
        assert!(master.sched.lock().unwrap().view().is_empty(), "unacknowledged request cannot run later");
        assert!(!dir.join("leases.live").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_lease_persists_keep_the_latest_scheduler_snapshot() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-write-order-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (master, ..) = master(&dir);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let threads: Vec<_> = ["first", "second"].into_iter().map(|lane| {
            let (master, barrier) = (master.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                master.sched.lock().unwrap().request(Req::new(lane, Kind::Build), 1_000);
                master.persist().unwrap();
            })
        }).collect();
        barrier.wait();
        for thread in threads { thread.join().unwrap(); }
        let expected: String = master.sched.lock().unwrap().live_lines().iter().map(|line| format!("{line}\n")).collect();
        assert_eq!(std::fs::read_to_string(dir.join("leases.live")).unwrap(), expected);
        assert_eq!(master.sched.lock().unwrap().view().len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn routing_cost_excludes_the_dev_jit_build() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-run-cost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, ..) = master(&dir);
        std::fs::write(dir.join("costs.jsonl"), "{\"class\":\"test:pkg\",\"kind\":\"test\",\"wall_ms\":500000,\"jit_ms\":480000,\"run_ms\":20000}\n{\"class\":\"test:pkg\",\"kind\":\"test\",\"wall_ms\":500000}\n{\"class\":\"test:pkg\",\"kind\":\"test\",\"wall_ms\":500000,\"run_ms\":0}\n").unwrap();
        m.refresh_costs();
        assert_eq!(m.sched.lock().unwrap().class_run_median("test:pkg", "test"), Some(20000), "only a positive run phase contributes");
        assert_eq!(m.sched.lock().unwrap().class_median("test:pkg", "test"), Some(500000), "scheduling still uses the wall");
        assert_eq!(serve(&m, "stats", "class test test:pkg\tnative:test:pkg"), "{\"runs\":{\"test:pkg\":20000,\"native:test:pkg\":null}}", "routing stats expose run medians");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancelled_run_is_not_its_class_measure() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-cancelled-cost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, ..) = master(&dir);
        let rec = |wall: u64, how: &str| format!("{{\"class\":\"test:hang\",\"kind\":\"test\",\"node\":\"a\",\"wall_ms\":{wall},\"run_ms\":{wall},\"how\":\"{how}\"}}\n");
        std::fs::write(dir.join("costs.jsonl"), [rec(3_065_000, "cancelled"), rec(3_000_000, "expired"), rec(375_000, "released")].concat()).unwrap();
        m.refresh_costs();
        let s = m.sched.lock().unwrap();
        assert_eq!(s.class_median("test:hang", "test"), Some(375_000), "a hang let run till cancelled says nothing of how long the class runs");
        assert_eq!(s.class_run_median("test:hang", "test"), Some(375_000));
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hub build its restart answer measured is its class's measure, once per build.
    #[test]
    fn a_measured_hub_build_is_its_class_measure_once() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-hub-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, ..) = master(&dir);
        let b = super::super::HubMeasure { end_ms: 5_000, wall_ms: 150_000, cpu_s: Some(900.0), peak_gb: Some(3.0) };
        m.ledger_hub_build("n1", &b);
        m.ledger_hub_build("n1", &b);
        let text = std::fs::read_to_string(dir.join("costs.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 1, "{text}");
        m.refresh_costs();
        let mut r = Req::new("devhub", Kind::Build);
        r.class = super::super::sched::HUB_BUILD.into();
        assert!(m.sched.lock().unwrap().measured_ram(&r, None).is_some_and(|gb| (gb - 3.6).abs() < 1e-9), "its peak and a fifth");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A box's hub build lease is room there and nothing on the node: its grant sends no
    /// `lease-start`, the box's beat does not name it, its end sends no `lease-end` and leaves no
    /// cost row (its class's measure is its hub's restart answer).
    #[test]
    fn a_hub_build_lease_is_room_on_its_box_and_nothing_on_the_node() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-hub-room-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _clock, _recs) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        let mut r = Req::new(&format!("{}-node-a", super::super::sched::HUB_BUILD), Kind::Build);
        r.class = super::super::sched::HUB_BUILD.into();
        r.master = true;
        r.node = Some("node-a".into());
        let l = serve(&m, "lease", &r.to_json());
        assert!(l.contains("\"state\":\"granted\""), "{l}");
        let id = l.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
        m.beat_node("node-a");
        assert!(serve(&m, "release", &id).contains(&id));
        let calls = node.calls.lock().unwrap().clone();
        assert!(!calls.iter().any(|c| matches!(c.1.as_str(), "lease-start" | "lease-end") || (c.1 == "lease-beat" && c.2.contains(&id))), "{calls:?}");
        let costs = std::fs::read_to_string(dir.join("costs.jsonl")).unwrap_or_default();
        assert!(!costs.contains(super::super::sched::HUB_BUILD), "{costs}");
        assert!(std::fs::read_to_string(dir.join("leases.jsonl")).unwrap().contains(&format!("\"id\":\"{id}\"")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lane's JIT built on one box runs a test lease granted on another (a lane's JIT only on the
    /// box that built it put its tests on a bench box, and another lane's timed run waited 22 min
    /// behind them, 11-07): the master copies it there at the grant from the box that built it, the
    /// run finds it at `$MAKEPAD_LOADER`, and the box keeps it by its key: its next lease copies
    /// nothing.
    #[cfg(unix)]
    #[test]
    fn a_lane_jit_built_on_one_box_runs_a_test_lease_granted_on_another() {
        use super::super::exec::{BoxLook, Exec, Opts};
        use super::super::jitstore::{Snapshot, Store};
        let dir = std::env::temp_dir().join(format!("devhub-lease-jit-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let hub = |name: &str| {
            let conf = super::super::caps::LeaseConf::parse(&format!("work = {}\n", dir.join(name).join("work").display()), &dir.join(name));
            Exec::new(name, conf, dir.join(name).join("leases"), Opts { scopes: Some(false), watch_memory: false, watch_wm: false, box_look: Some(BoxLook::quiet()), ..Opts::default() }, Arc::new(|_| {}))
        };
        let (built_on, granted_on) = (hub("box-a"), hub("box-b"));
        // the lane's JIT, built on box-a: a script past two copy chunks
        let id = "0123456789abcdef01234567";
        let mut text = "#!/bin/sh\necho \"the lane's JIT: $*\"\nexit 0\n".to_string();
        text.push_str(&"#".repeat(super::super::jitstore::COPY_CHUNK * 2));
        std::fs::write(dir.join("jit"), text).unwrap();
        Store::new(&built_on.conf().work).keep(id, &dir.join("jit")).unwrap();
        let reads = Arc::new(AtomicU64::new(0));
        let (a, b, r) = (built_on.clone(), granted_on.clone(), reads.clone());
        let ask: Ask = Arc::new(move |node: &str, service: &str, line: &str| {
            let x = if node == "box-a" { &a } else { &b };
            if service == "jit-read" { r.fetch_add(1, Ordering::SeqCst); }
            Ok(match service {
                // box-a is a bench box: it keeps its quiet window for timed work
                "caps" => vec![super::super::Caps {
                    features: super::super::FEATURES.iter().map(|f| f.to_string()).collect(), node: node.into(), arch: "x86_64".into(), os: "linux".into(), cores: 16, ram_gb: 64.0, ram_avail_gb: 50.0, disk_free_gb: 300.0, disk_total_gb: 1000.0, disk_floor_gb: 25.0, timed: true,
                    role: Some(if node == "box-a" { super::super::Role::Bench } else { super::super::Role::Pool }),
                    jit_store: Some(Arc::new(Snapshot { compiler: "rustc-1".into(), ..Store::new(&x.conf().work).snapshot() })),
                    ..Default::default()
                }.to_json()],
                _ => x.handle(service, line),
            })
        });
        let m = Arc::new(LeaseMaster::new("mac", dir.join("home"), ask, Arc::new(|_| {}), Arc::new(|| 1_000), None, true));
        let boxes = [("box-a".to_string(), "192.0.2.89:8797".to_string()), ("box-b".to_string(), "192.0.2.90:8797".to_string())];
        m.refresh(&boxes);
        let test_lease = || {
            let mut r = Req::new("lanex", Kind::Test);
            r.loader = Some(id.into());
            let l = serve(&m, "lease", &r.to_json());
            assert!(l.contains("\"state\":\"granted\"") && l.contains("\"box-b\""), "{l}");
            l.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string()
        };
        let lease = test_lease();
        assert!(Store::new(&granted_on.conf().work).snapshot().ids.iter().any(|held| held == id), "box-b keeps it by its key");
        assert_eq!(reads.load(Ordering::SeqCst), 3, "its three chunks");
        let argv = ["sh", "-c", "\"$MAKEPAD_LOADER\" ran on box-b"].join("\0");
        let run = granted_on.handle("lease-run", &format!("{lease} . {}", makepad_network::digest::base64_encode(argv.as_bytes())));
        let n = makepad_strict_json::parse(run[0].as_bytes()).ok().and_then(|v| v.get("n").and_then(super::super::num)).unwrap_or_else(|| panic!("{run:?}")) as u32;
        let (mut out, mut off, mut exit) = (Vec::new(), 0u64, None);
        for _ in 0..300 {
            let a = granted_on.handle("lease-log", &format!("{lease} {n} {off}"));
            let v = makepad_strict_json::parse(a[0].as_bytes()).unwrap();
            out.extend(super::super::exec::b64_decode(v.get("data").and_then(|d| d.as_str()).unwrap()).unwrap());
            off = v.get("off").and_then(super::super::num).unwrap() as u64;
            exit = v.get("exit").and_then(super::super::num);
            if exit.is_some() { break; }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!((String::from_utf8_lossy(&out).trim(), exit), ("the lane's JIT: ran on box-b", Some(0.0)));
        let costs = std::fs::read_to_string(dir.join("home/costs.jsonl")).unwrap();
        assert!(costs.contains("{\"class\":\"jit-copy\",\"kind\":\"\",\"node\":\"box-b\",\"from\":\"box-a\""), "{costs}");
        assert!(serve(&m, "release", &lease).contains(&lease));
        m.refresh(&boxes);
        let next = test_lease();
        assert_eq!(reads.load(Ordering::SeqCst), 3, "its next lease there copies nothing");
        serve(&m, "release", &next);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stalled leg devhub runs whose JIT's capture finds it parked at code that did not compile
    /// ends as hung with that cause; a lane's lease parked the same stays for its lane to edit,
    /// and a leg whose JIT only waits stays too.
    #[test]
    fn a_stalled_own_leg_parked_at_a_compile_error_ends() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-parked-leg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _clock, _recs) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        let mut leg = Req::new("jitlegs-nocodegen", Kind::Test);
        leg.agent = Some("devhub-jitlegs".into());
        serve(&m, "lease", &leg.to_json());
        *node.jits.lock().unwrap() = vec![42];
        let state = |id: &str| m.sched.lock().unwrap().get(id).map(|l| (match &l.state { State::Ended(how) => format!("ended {how}"), x => x.as_str().to_string() }, l.reason.clone())).unwrap();
        m.capture("L1", "node-a", "L1 running 9m against its measured 3m46s");
        assert_eq!(state("L1").0, "granted", "a JIT that only waits may go on");
        *node.capture_cause.lock().unwrap() = "compile error: a.rs:1:2: E0308: mismatched types".into();
        m.capture("L1", "node-a", "L1 running 9m against its measured 3m46s");
        assert_eq!(state("L1"), ("ended hung".to_string(), "its JIT parked (compile error: a.rs:1:2: E0308: mismatched types): no one edits or resumes a leg devhub runs".to_string()));
        serve(&m, "lease", &Req::new("lane", Kind::Test).to_json());
        assert_eq!(state("L2").0, "granted", "{:?}", state("L2"));
        m.capture("L2", "node-a", "L2 running 9m against its measured 3m46s");
        assert_eq!(state("L2").0, "granted", "a lane edits its own code");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stalled leg devhub runs keeps its node's look at its processes in its kept log, and ends
    /// hung once that capture is in, naming where it waited, its last step and its capture: a
    /// jitlegs leg's hung fixture stub was read by hand (L634083, 11-08).
    #[test]
    fn a_hung_leg_ends_naming_where_it_waited_and_its_capture() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-hung-leg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, ..) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        let mut leg = Req::new("jitlegs-full-a", Kind::Test);
        leg.agent = Some("devhub-jitlegs".into());
        serve(&m, "lease", &leg.to_json());
        m.sched.lock().unwrap().set_info("L1", r#"{"idle_s":600}"#, 1_000);
        m.pump();
        let log = dir.join("lease-logs/L1.log");
        let (how, why) = m.sched.lock().unwrap().get("L1").map(|l| (format!("{:?}", l.state), l.reason.clone())).unwrap();
        assert_eq!(how, format!("{:?}", State::Ended("hung".into())), "{why}");
        assert_eq!(why, format!("every process idle (under 5% of a core, no I/O) for 10m00s; it waits in `sh > bash land_gate.sh full-a > sleep 3600` (pid 7, for 9m50s); its last step line `STEP fuzz 47 s [1-2]`; its capture: devhub job result L1 ({})", log.display()));
        let kept = std::fs::read_to_string(&log).unwrap();
        assert!(kept.contains("== node-a: its processes and last lines when it stalled (") && kept.contains("pid 7: sleep 3600\n"), "{kept}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stalled lease's JIT leaves every thread's stack in the lease's kept log, once however
    /// often the stall is said, and the lease's last lines go after them at its end.
    #[test]
    fn a_stall_keeps_every_threads_stack_once_before_the_last_lines() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-stall-stacks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _clock, recs) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        serve(&m, "lease", &Req::new("gapx", Kind::Build).to_json());
        *node.jits.lock().unwrap() = vec![42];
        m.capture("L1", "node-a", "L1 running 20m against its kind's 5m");
        m.capture("L1", "node-a", "the same stall said again after a master restart");
        let log = std::fs::read_to_string(dir.join("lease-logs/L1.log")).unwrap();
        assert_eq!(log.matches("== JIT 42 on node-a: every thread's stack").count(), 1, "{log}");
        assert!(log.contains("its capture node-a:/c/a.txt") && log.contains("futex_wait"), "{log}");
        assert_eq!(node.calls.lock().unwrap().iter().filter(|(n, s, l)| n == "node-a:42" && s == "cmd" && l.contains("\"stacks\"")).count(), 1);
        assert!(recs.lock().unwrap().iter().any(|r| r.sev == Sev::Err && r.line.contains("JIT 42: blocked: main waits (node-a:/c/a.txt; every thread's stack: devhub job result L1)")));
        *node.end_answer.lock().unwrap() = Some("{\"ended\":\"L1\",\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[],\"tail\":\"run 1: still running\\nrunning for over a minute: a::hung\\n\"}".into());
        assert!(serve(&m, "cancel", "L1 by=test").contains("\"kept_tail\":true"));
        let log = std::fs::read_to_string(dir.join("lease-logs/L1.log")).unwrap();
        assert!(log.find("futex_wait").unwrap() < log.find("a::hung").unwrap(), "{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A cancel's end asks its node how far the last run got and keeps that under the lease's id;
    /// the node hears `released` either way. A plain release asks for nothing.
    #[test]
    fn a_cancel_keeps_how_far_the_run_got() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-cancel-tail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _, recs) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        serve(&m, "lease", &Req::new("gapx", Kind::Build).to_json());
        *node.end_answer.lock().unwrap() = Some("{\"ended\":\"L1\",\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[],\"tail\":\"run 1: still running\\nrunning for over a minute: a::hung\\n\"}".into());
        let answer = serve(&m, "cancel", "L1 by=test");
        assert!(answer.contains("\"kept_tail\":true"), "{answer}");
        // whoever said `by=` cancelled it, not the user (whose cancel keeps the lane off the node)
        let said: Vec<String> = recs.lock().unwrap().iter().filter(|r| r.line.contains("L1") && r.line.contains("cancelled by")).map(|r| r.line.clone()).collect();
        assert!(!said.is_empty() && said.iter().all(|l| l.contains("cancelled by test")), "{said:?}");
        assert!(std::fs::read_to_string(dir.join("lease-logs/L1.log")).unwrap().contains("a::hung"));
        let ends = |id: &str| node.calls.lock().unwrap().iter().filter(|(_, s, l)| s == "lease-end" && l.contains(id)).map(|(_, _, l)| l.clone()).collect::<Vec<_>>();
        assert_eq!(ends("L1"), [r#"lease-end L1 released {"tail":true}"#]);
        *node.end_answer.lock().unwrap() = None;
        serve(&m, "lease", &Req::new("gapy", Kind::Build).to_json());
        let answer = serve(&m, "release", "L2");
        assert!(answer.contains("\"kept_tail\":false"), "{answer}");
        assert_eq!(ends("L2"), ["lease-end L2 released"]);
        assert!(!dir.join("lease-logs/L2.log").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The idle verdict reaches whoever holds the lease through its beat, not the inbox alone,
    /// with where its stacks are; and its holder may hand the running lease to the master's own
    /// work (a job that follows the run), after which the holder's end no longer ends it.
    #[test]
    fn an_idle_leases_beat_says_so_and_its_holder_may_hand_it_over() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-idle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, ..) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        let mut r = Req::new("gapx", Kind::Build);
        r.holder = Some(std::process::id());
        serve(&m, "lease", &r.to_json());
        assert!(!serve(&m, "beat", "L1").contains("\"idle\""));
        m.sched.lock().unwrap().set_info("L1", r#"{"idle_s":600}"#, 1_000);
        let beat = serve(&m, "beat", "L1");
        assert!(beat.contains("\"state\":\"granted\"") && beat.contains("\"idle\":\"") && beat.contains("every process idle"), "{beat}");
        assert!(beat.contains("\"hung\":true") && beat.contains("its stacks: `devhub job result L1`"), "the idle verdict, its client's cue: {beat}");
        let other = std::process::id() + 1;
        assert!(m.adopt("L1", other).unwrap_err().contains("not held by"), "only its holder hands it over");
        assert_eq!(m.adopt("L1", std::process::id()).map(|(node, _)| node), Ok("node-a".to_string()));
        assert!(m.sched.lock().unwrap().holders_gone(&|_| false).is_empty(), "its holder's end no longer ends it");
        assert!(std::fs::read_to_string(dir.join("leases.live")).unwrap().contains("\"master\":true"), "a restarted master's job takes it back");
        assert!(m.adopt("L1", std::process::id()).is_err(), "handed over once");
        m.sched.lock().unwrap().set_info("L1", r#"{"idle_s":0}"#, 1_000);
        assert!(!serve(&m, "beat", "L1").contains("\"idle\""), "it works again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A live session's box hears each change of its state: a call weighs its runs as interactive
    /// work, the end of its last call as background work (an idle session that spins slows no
    /// gate beside it, 11-06). A call already running says nothing again.
    #[test]
    fn a_live_sessions_box_weighs_it_by_its_calls() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-weigh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, ..) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        let mut r = Req::new("livex", Kind::Test);
        r.live = true;
        assert!(serve(&m, "lease", &r.to_json()).contains("\"state\":\"granted\""));
        let weighs = || node.calls.lock().unwrap().iter().filter(|c| c.1 == "lease-weight").map(|c| c.2.clone()).collect::<Vec<_>>();
        m.live_idle("L1");
        m.live_idle("L1");
        assert_eq!(m.live_admit("L1"), Ok(()));
        assert_eq!(m.live_admit("L1"), Ok(()));
        m.live_idle("L1");
        assert_eq!(weighs(), ["lease-weight L1 idle", "lease-weight L1 call", "lease-weight L1 idle"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A live call's peak cores as its box metered it go to the ledger as a sample of its class:
    /// `devhub stats calls` says them per class, its launches apart, and the scheduler reads
    /// them back: its class's next calls are charged by them, a granted session's cost stands.
    #[test]
    fn a_live_calls_metered_cores_are_kept_and_said() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-calls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, ..) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        let mut r = Req::new("livex", Kind::Test);
        (r.live, r.class) = (true, "live calc".into());
        assert!(serve(&m, "lease", &r.to_json()).contains("\"state\":\"granted\""));
        let cores = m.sched.lock().unwrap().get("L1").unwrap().cost.cores;
        assert_eq!(serve(&m, "stats", "calls"), "{\"text\":\"no live calls metered in the last day\\n\"}");
        *node.call_answer.lock().unwrap() = r#","call":{"launch":true,"ms":12000,"peak":7.5,"mean":3.1}"#.into();
        m.live_idle("L1");
        for (ms, peak) in [(900, 1.2), (950, 4.0)] {
            assert_eq!(m.live_admit("L1"), Ok(()));
            *node.call_answer.lock().unwrap() = format!(r#","call":{{"launch":false,"ms":{ms},"peak":{peak},"mean":0.5}}"#);
            m.live_idle("L1");
        }
        let stats = serve(&m, "stats", "calls");
        assert!(stats.contains("live calc (test) launch: 1 sample(s), peak 7.5/7.5/7.5, mean 3.1, wall 12.0 s"), "{stats}");
        assert!(stats.contains("live calc (test) call: 2 sample(s), peak 1.2/4.0/4.0, mean 0.5, wall 0.9 s"), "{stats}");
        m.refresh_costs();
        let s = m.sched.lock().unwrap();
        let l = s.get("L1").unwrap();
        assert_eq!((s.call_p90(&l.req, true), s.call_p90(&l.req, false)), (Some((7.5, 1)), Some((4.0, 2))));
        assert_eq!(l.cost.cores, cores, "its grant's cost stands");
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the machine slept between two looks at both clocks: the wall's advance past the
    /// monotonic clock's, from 2 s on.
    #[test]
    fn a_wall_clock_past_the_monotonic_one_is_a_sleep() {
        assert_eq!(slept_ms(929_000, Duration::from_millis(400)), 928_600);
        assert_eq!(slept_ms(2_900, Duration::from_millis(1_000)), 0, "a look's lag");
        assert_eq!(slept_ms(1_000, Duration::from_secs(30)), 0, "a stall of the process ran both clocks");
    }

    /// The master's machine asleep (the Mac's lid closed, 11-08): the lease loop's look at both
    /// clocks says how long, and nothing of it counts against a lease. A live session beaten just
    /// before a 15 min sleep is still granted at the wake, and the node's beat names it, so its
    /// hub keeps it too. The same wall jump without the look (a test's clock) expires it.
    #[test]
    fn a_live_session_outlives_the_masters_sleep() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-sleep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, clock, _) = master(&dir);
        m.refresh(&[("node-a".to_string(), "192.0.2.89:8797".to_string())]);
        let mut r = Req::new("live-W1", Kind::Test);
        (r.live, r.class) = (true, "live calc".into());
        assert!(serve(&m, "lease", &r.to_json()).contains("\"state\":\"granted\""));
        node.known.lock().unwrap().push("L1".into());
        clock.store(20_000, Ordering::SeqCst);
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""));
        *m.awake.lock().unwrap() = Some((std::time::Instant::now(), clock.load(Ordering::SeqCst)));
        m.pump();
        clock.store(20_000 + 900_000, Ordering::SeqCst);
        m.pump();
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""), "it outlived the sleep");
        m.beat_nodes();
        assert_eq!(node.calls.lock().unwrap().iter().filter(|c| c.1 == "lease-beat").last().map(|c| c.2.clone()).as_deref(), Some("lease-beat L1"), "its hub is told the master holds it");
        *m.awake.lock().unwrap() = None;
        clock.store(20_000 + 1_800_000, Ordering::SeqCst);
        m.pump();
        assert!(serve(&m, "beat", "L1").contains("\"how\":\"expired\""), "a wall jump nothing says is sleep");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grant_start_beat_release_end() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, clock, recs) = master(&dir);
        let hosts = vec![("node-a".to_string(), "192.0.2.89:8797".to_string())];
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
        assert!(!serve(&m, "nodes", "").contains("\"master\":"), "the nodes alone for every other reader");
        assert!(serve(&m, "nodes", "master").contains(&format!("\"master\":\"{}\",\"hub\":", m.machine)), "the master's own build first: {}", serve(&m, "nodes", "master"));
        let r = serve(&m, "release", "L1 released exit=0");
        assert!(r.contains("\"state\":\"ended\"") && r.contains("\"how\":\"released\""), "{r}");
        assert!(node.calls.lock().unwrap().iter().any(|c| c.1 == "lease-end" && c.2 == "lease-end L1 released"));
        let ledger = std::fs::read_to_string(dir.join("leases.jsonl")).unwrap();
        assert!(ledger.contains("\"id\":\"L1\"") && ledger.contains("\"how\":\"released\""));
        assert!(recs.lock().unwrap().iter().any(|r| r.kind == "lease" && r.line.starts_with("L1 build@node-a granted to gapx")));
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

    /// After a restart the master's own leases that no record of its work names end at once (their
    /// runs died with the previous master), as an info line; a named one and a lane's stay.
    #[test]
    fn a_restart_ends_the_masters_own_leases_that_nothing_follows_and_its_work_keeps_its_place() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-unfollowed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, ..) = master(&dir);
        let hosts: Vec<(String, String)> = (1..=3).map(|i| (format!("node-{i}"), format!("192.0.2.{i}:8797"))).collect();
        m.refresh(&hosts);
        let id = |l: &str| l.split("\"id\":\"").nth(1).unwrap().split('"').next().unwrap().to_string();
        let own = |lane: &str| {
            let mut r = Req::new(lane, Kind::Test);
            r.master = true;
            let l = serve(&m, "lease", &r.to_json());
            assert!(l.contains("\"state\":\"granted\""), "{l}");
            id(&l)
        };
        let (gone, followed) = (own("allapps"), own("verify"));
        let lane = serve(&m, "lease", &Req::new("lane", Kind::Test).to_json());
        assert!(lane.contains("\"state\":\"granted\""), "{lane}");
        let lane = id(&lane);
        drop(m);
        let (m1, node1, clock, recs) = master(&dir);
        node1.known.lock().unwrap().extend([gone.clone(), followed.clone(), lane.clone()]);
        m1.refresh(&hosts);
        assert_eq!(m1.end_unfollowed(&|x| x == followed), vec![gone.clone()]);
        assert!(serve(&m1, "beat", &gone).contains("\"how\":\"holder-dead\""));
        assert!(node1.calls.lock().unwrap().iter().any(|c| c.1 == "lease-end" && c.2 == format!("lease-end {gone} holder-dead {{\"tail\":true}}")), "its box cleans up now");
        assert!(recs.lock().unwrap().iter().any(|r| r.kind == "lease" && r.sev == Sev::Info && r.line.contains(&format!("{gone} ")) && r.line.contains("released: its run ended with the previous master")));
        assert!(serve(&m1, "beat", &followed).contains("\"state\":\"granted\""), "a record names it: its run is followed");
        assert!(serve(&m1, "beat", &lane).contains("\"state\":\"granted\""), "a lane's holder re-attaches itself");
        assert!(m1.end_unfollowed(&|_| false).is_empty(), "only right after the restart");
        // its work asks again: at the place and age of its first ask, not the restart's
        clock.store(61_000, Ordering::SeqCst);
        let mut again = Req::new("allapps", Kind::Test);
        again.master = true;
        let l = serve(&m1, "lease", &again.to_json());
        assert!(l.contains("\"wait_s\":60,"), "{l}");
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
        let hosts = vec![("node-a".to_string(), "192.0.2.89:8797".to_string())];
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
        m.refresh(&[("pool-a".to_string(), "192.0.2.89".to_string())]);
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
        assert!(recs.lock().unwrap().iter().any(|r| r.line.contains("lease_latency: L2 test@pool-a of b on pool-a: release->grant 0.0 s")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A medium lease waiting for a box a small test pegged starts at its release: the lease-end
    /// answers the box's load after the teardown, the master's measure until the next caps round
    /// (its caps were a 10 s average that still counted the tests: the lease waited one or two
    /// caps rounds, 23 s on a pool box, 11-04).
    #[test]
    fn a_box_a_small_test_pegged_takes_the_waiting_medium_lease_at_its_release() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-fresh-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, clock, _) = master(&dir);
        *node.cpu.lock().unwrap() = Some(99.0);
        m.refresh(&[("pool-a".to_string(), "192.0.2.89".to_string())]);
        let small = |lane: &str| {
            let mut r = Req::new(lane, Kind::Test);
            r.cost = CostClass::Small;
            r.cost_asked = true;
            r.to_json()
        };
        assert!(serve(&m, "lease", &small("a")).contains("\"state\":\"granted\""));
        let mut medium = Req::new("c", Kind::Build);
        medium.cost = CostClass::Medium;
        medium.cost_asked = true;
        let c = serve(&m, "lease", &medium.to_json());
        assert!(c.contains("\"state\":\"queued\""), "{c}");
        assert!(serve(&m, "beat", "L2").contains("\"reason\":\"held by L1 (test of a)\""), "it waits while the test runs: {}", serve(&m, "beat", "L2"));
        *node.end_load.lock().unwrap() = Some((4.0, 50.0));
        clock.store(5_000, Ordering::SeqCst);
        serve(&m, "release", "L1 released exit=0");
        assert!(serve(&m, "beat", "L2").contains("\"state\":\"granted\""), "granted at the release, before any caps round: {}", serve(&m, "beat", "L2"));
        let nodes = serve(&m, "nodes", "");
        assert!(nodes.contains("handoff 0.0 s (median of 1)") && nodes.contains("teardown "), "{nodes}");
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
        let hosts = vec![("node-a".to_string(), "192.0.2.89".to_string())];
        m.refresh(&hosts);
        node.gone.store(true, Ordering::SeqCst);
        let a = serve(&m, "lease", &Req::new("a", Kind::Test).to_json());
        assert!(a.contains("\"state\":\"queued\""), "{a}");
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"queued\""), "still waiting while the box is down");
        assert!(recs.lock().unwrap().iter().any(|r| r.line.contains("node-a unreachable at its start")));
        // its caps go unanswered: what it said last stays on disk, for a master that starts while
        // it is away (a CI shard only it takes says the box is down, not that none is set up)
        m.refresh(&hosts);
        let (again, _, _, _) = master(&dir);
        assert!(again.heard("node-a").is_some_and(|(_, c)| c.display.as_deref() == Some("wm:makepad-wm-0")), "a new master knows what the box was");
        // the hub is back: its caps answer, the box is up, the lease gets it
        node.gone.store(false, Ordering::SeqCst);
        *node.trouble.lock().unwrap() = "devhub on node-a could not run for 9 s (1 try): could not start: the user's service manager is down".into();
        m.refresh(&hosts);
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""));
        assert!(!dir.join(HEARD).join("node-a").exists(), "a box that answers again keeps nothing on disk");
        // and why it was down, from its WM's record, is said once as a warning
        m.refresh(&hosts);
        let said: Vec<_> = recs.lock().unwrap().iter().filter(|r| r.line.contains("could not run for 9 s")).map(|r| r.sev).collect();
        assert_eq!(said, [Sev::Warn]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A box whose display went since the last look refuses a gui lease: it goes to another box,
    /// and does not fail (a bucket's gate took that as its verdict).
    #[test]
    fn a_gui_start_a_box_refuses_goes_to_another_box() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-gui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _clock, _recs) = master(&dir);
        let hosts = vec![("a".to_string(), "192.0.2.1".to_string()), ("b".to_string(), "192.0.2.2".to_string())];
        m.refresh(&hosts);
        let mut r = Req::new("g", Kind::Test);
        r.needs.push(("gui".to_string(), "1".to_string()));
        // whichever box it picks first refuses
        *node.no_gui.lock().unwrap() = "a".into();
        let first = serve(&m, "lease", &r.to_json());
        let picked = |m: &Arc<LeaseMaster>, id: &str| m.view().iter().find(|l| l.id == id).and_then(|l| l.node.clone());
        if picked(&m, "L1").as_deref() != Some("b") {
            assert!(first.contains("\"state\":\"queued\""), "{first}");
            m.pump();
        }
        assert_eq!(picked(&m, "L1").as_deref(), Some("b"), "{}", serve(&m, "beat", "L1"));
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The master's look at a box is seconds old, and an apply started since. The box refuses the
    /// timed start: the lease is not failed, it waits on a new look, and starts when the build is over.
    #[test]
    fn a_timed_start_that_meets_a_build_waits_for_it() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-timed-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (m, node, _clock, _recs) = master(&dir);
        let hosts = vec![("node-a".to_string(), "192.0.2.89".to_string())];
        m.refresh(&hosts);
        node.apply.store(true, Ordering::SeqCst);
        let a = serve(&m, "lease", &Req::new("a", Kind::Timed).to_json());
        assert!(a.contains("\"state\":\"queued\""), "{a}");
        assert!(node.apply_seen.load(Ordering::SeqCst), "it looked at the box again");
        let starts = |node: &FakeNode| node.calls.lock().unwrap().iter().filter(|c| c.1 == "lease-start").count();
        assert_eq!(starts(&node), 1, "the new look holds it back: no second try against the build");
        assert!(serve(&m, "beat", "L1").contains("its deployment applies"), "{}", serve(&m, "beat", "L1"));
        node.apply.store(false, Ordering::SeqCst);
        m.refresh(&hosts);
        assert!(serve(&m, "beat", "L1").contains("\"state\":\"granted\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a node call says when the hub holding its link exits for its self-update depends on
    /// where the call stood. Either way the node is away, not refusing.
    #[test]
    fn a_hub_leaving_mid_call_is_unreachable_wherever_the_call_stood() {
        assert!(unreachable("shipping S1: node ship-put: channel closed"));
        assert!(unreachable("link lost: connection closed by the peer"));
        assert!(unreachable("node refused: lease-start L1: git worktree: signal: 15 (SIGTERM)"));
        assert!(!unreachable("shipping S1: node ship-put: no room on the disk"));
    }

    /// A host off the network is told from its hub restarting: no host answers the connect, where
    /// a restarting hub's host refuses it or its call ends "restarting: " (11-07).
    #[test]
    fn a_host_off_the_network_is_no_hub_restart() {
        let at = "link lost: cannot reach fleet hub `node-a` at 192.0.2.1:8797";
        for gone in ["Host is down (os error 64)", "No route to host (os error 113)", "Network is unreachable (os error 101)", "Connection timed out (os error 110)", "Operation timed out (os error 60)"] {
            let e = format!("{at}: {gone}");
            assert!(host_gone(&e) && unreachable(&e), "{e}");
        }
        let refused = format!("{at}: Connection refused (os error 111)");
        assert!(!host_gone(&refused) && unreachable(&refused));
        assert!(!host_gone("restarting: node-a's hub restarts for its self-update; try again"));
    }

    /// An inspect call refused for a lease that is not an inspect lease left a mark with no pid,
    /// and its node refused the end that carried it: the lease stayed there, unbeaten, for 60 s.
    #[test]
    fn a_refused_inspect_call_does_not_spoil_the_leases_end() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-refused-inspect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nodes.conf"), "pool-a addr=192.0.2.89 # pool\n").unwrap();
        let (m, node, _clock, _recs) = master(&dir);
        m.refresh(&[("pool-a".to_string(), "192.0.2.89".to_string())]);
        assert!(serve(&m, "lease", &Req::new("x", Kind::Test).to_json()).contains("\"state\":\"granted\""));
        assert!(serve(&m, "linspect", "L1 pool-a foo eval 1").contains("not a live inspect lease"));
        serve(&m, "release", "L1");
        assert!(node.calls.lock().unwrap().iter().any(|c| c.1 == "lease-end" && c.2 == "lease-end L1 released"), "{:?}", node.calls.lock().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safety_dev_switch_and_inspect_holders() {
        let dir = std::env::temp_dir().join(format!("devhub-lease-test2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("nodes.conf"), "pool-a addr=192.0.2.89 # pool\n").unwrap();
        let (m, node, _clock, recs) = master(&dir);
        m.refresh(&[("pool-a".to_string(), "192.0.2.89".to_string())]);
        assert!(serve(&m, "node-dev", "pool-a off").contains("\"dev\":false"));
        assert_eq!(std::fs::read_to_string(dir.join("nodes.conf")).unwrap(), "pool-a addr=192.0.2.89 dev=off # pool\n");
        assert!(node.calls.lock().unwrap().iter().any(|c| c.2 == "policy dev=off"));
        assert!(serve(&m, "lease", &Req::new("x", Kind::Build).to_json()).contains("\"reason\":\"dev off\""));
        let ins = |lane: &str| {
            let mut r = Req::new(lane, Kind::Inspect);
            r.node = Some("pool-a".into());
            r.jit = Some("foo".into());
            r.to_json()
        };
        assert!(serve(&m, "lease", &ins("a")).contains("\"state\":\"granted\""));
        assert!(serve(&m, "lease", &ins("b")).contains("\"state\":\"granted\""));
        let id_a = "L2";
        let id_b = "L3";
        assert!(serve(&m, "linspect", &format!("{id_b} pool-a other eval 1")).contains("not a live inspect lease"));
        // Hold a pause in the transport: a competing inspect cannot acquire ownership and
        // release must wait until the accepted command has applied and its marks are recorded.
        let (entered, wait) = std::sync::mpsc::sync_channel(0);
        let (resume, continued) = std::sync::mpsc::sync_channel(0);
        *node.inspect_gate.lock().unwrap() = Some((entered, continued));
        std::thread::scope(|scope| {
            let command = scope.spawn(|| serve(&m, "linspect", &format!("{id_a} pool-a foo pause")));
            wait.recv().unwrap();
            assert!(serve(&m, "linspect", &format!("{id_b} pool-a foo pause")).contains("held by L2"));
            let release = scope.spawn(|| serve(&m, "release", id_a));
            // Release state changes before its cleanup waits for the in-flight command.
            while m.sched.lock().unwrap().get(id_a).unwrap().state == State::Granted { std::thread::yield_now(); }
            assert!(!node.calls.lock().unwrap().iter().any(|c| c.1 == "lease-end"));
            resume.send(()).unwrap();
            assert!(!command.join().unwrap().contains("err"));
            assert!(release.join().unwrap().contains("cleanup"));
        });
        assert!(!node.paused.load(Ordering::SeqCst), "release must resume the named app");
        assert!(serve(&m, "linspect", &format!("{id_b} pool-a foo break set game/ai.rs:88")).contains("G1"));
        assert!(recs.lock().unwrap().iter().any(|r| r.sev == Sev::Warn && r.line == "inspect pool-a/foo by lane b: break set game/ai.rs:88"));
        assert!(!serve(&m, "linspect", &format!("{id_b} pool-a foo pause")).contains("err"));
        serve(&m, "release", id_b);
        assert!(!node.paused.load(Ordering::SeqCst));
        assert!(node.breaks.lock().unwrap().is_empty(), "release removes its breakpoints");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn safety_failed_cleanup_keeps_capacity_until_a_confirmed_receipt() {
        let dir = std::env::temp_dir().join(format!("devhub-cleanup-retry-{}", std::process::id()));
        let (m, node, _, _) = master(&dir);
        let hosts = [("pool-a".into(), "192.0.2.1".into())];
        m.refresh(&hosts);
        let mut first = Req::new("a", Kind::Build);
        first.whole = true;
        assert!(serve(&m, "lease", &first.to_json()).contains("granted"));
        assert!(serve(&m, "lease", &Req::new("b", Kind::Test).to_json()).contains("queued"));
        *node.end_answer.lock().unwrap() = Some("{\"err\":\"cleanup failed\"}".into());
        serve(&m, "release", "L1");
        for reply in ["{\"pending\":\"cleanup in progress\"}", "{\"ended\":\"L1\",\"leak\":[42]}", "{\"ended\":\"L9\",\"leak\":[]}"] {
            *node.end_answer.lock().unwrap() = Some(reply.into());
            m.end_later("pool-a");
            m.refresh(&hosts);
            assert!(serve(&m, "beat", "L2").contains("queued"), "{reply} must not free capacity");
            assert_eq!(m.sched.lock().unwrap().cleaning("L1"), Some("pool-a"), "reservation survives failed cleanup");
        }
        *node.end_answer.lock().unwrap() = None;
        m.end_later("pool-a");
        assert!(serve(&m, "beat", "L2").contains("granted"));
        serve(&m, "release", "L2");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn base64_decodes() {
        let s = makepad_network::digest::base64_encode(b"devhub lease\0x");
        assert_eq!(b64_decode(&s).unwrap(), b"devhub lease\0x");
    }
}

#[cfg(test)]
mod cost_record_tests {
    use super::*;

    /// A lease whose wall was its own JIT build measures that build's cores on its OS family; a
    /// short build, a lease mostly run, a waiter on another lease's build (under 2 cores), one
    /// not released or of no OS family do not.
    #[test]
    fn a_lease_that_was_its_jit_build_measures_the_build_cores() {
        let mut c = CostRecords::default();
        let line = |extra: &str| format!(r#"{{"class":"test makepad-foo","kind":"test","how":"released","wall_ms":100000,"os":"linux",{extra}}}"#);
        for extra in [
            r#""jit_ms":90000,"cpu_seconds":960"#,
            r#""jit_ms":20000,"cpu_seconds":960"#,
            r#""jit_ms":50000,"cpu_seconds":960"#,
            r#""jit_ms":90000,"cpu_seconds":50"#,
        ] {
            c.push(&makepad_strict_json::parse(line(extra).as_bytes()).unwrap());
        }
        for other in [
            r#"{"class":"test makepad-foo","kind":"test","how":"cancelled","wall_ms":100000,"os":"linux","jit_ms":90000,"cpu_seconds":960}"#,
            r#"{"class":"test makepad-foo","kind":"test","how":"released","wall_ms":100000,"jit_ms":90000,"cpu_seconds":960}"#,
        ] {
            c.push(&makepad_strict_json::parse(other.as_bytes()).unwrap());
        }
        assert_eq!(c.jit_builds, vec![("linux".to_string(), 9.6)]);
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
        rev.rev = Some("rc-x,commercial=main".into());
        assert!(bind_lane(&home, &rev).is_ok());
        // the first worktree is removed (landed): the name passes on
        std::fs::remove_dir_all(&a).unwrap();
        assert!(bind_lane(&home, &req(&b)).is_ok());
        assert!(std::fs::read_to_string(home.join("lanes.conf")).unwrap().contains(&b.to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
