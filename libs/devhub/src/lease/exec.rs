//! The node side of leases: `lease-start`, `lease-run`, `lease-log`, `lease-end`, `lease-beat`,
//! `policy`, `caps`, `disk`, `recover-get` / `recover-del` on the node's master endpoint
//! (LEASE.SPEC.md "Wire").
//!
//! - Overlay leases mount pinned RELAXGIT stores at one virtual `<work>/dev/src` and keep edits
//!   in `<work>/dev/overlay/<lane>/<lease>`. Their real cwd is an empty lane run directory. They never
//!   touch either tree. The hub resolves the pinned dev JIT; builds belong to its shared store.
//!   Resolution runs on the lease's worker; `lease-run` returns `pending` until it can report
//!   the started run or its error. Polling never starts the same resolution twice.
//!   Different pins of one lane wait for one another; equal pins run side by side, each on its own
//!   edits: a run takes exactly the source its lease shipped (`source`, in its answer and receipt).
//!   CI overlays are keyed per pin.
//! - Pinned lane tree leases use `<work>/native/<lane>` and `<work>/native-target/<lane>`.
//!   CI, gates, landing/CI lanes and user boxes keep `<work>/ci-tree` and `<work>/ci-target`
//!   ([`super::tree`]). Holds and cleanup are per tree; native trees are capped and reclaimed LRU.
//!   Native JIT runs reuse this box's content-matching JIT store. Without a rev, a
//!   non-CI lease uses `<work>/lane/<lane>/src` and `<work>/target/lane-<lane>`. Scratch is
//!   `<work>/tmp/<id>` (TMPDIR, run logs). The deployment checkout belongs only to sync apply.
//! - A run passes argv literally, in its own session (setsid), under
//!   `systemd-run --user --scope` when the user bus exists, at its kind's nice level, optionally pinned
//!   (`cpus.*` in lease.conf). Its wrapper (Unix) writes its exit code beside the ledger and,
//!   in a scope, its final resource counters. Its process-group leader and start time go into
//!   the ledger (`~/.makepad/devhub/leases/<id>.json`), so a restarted hub (its self-update, at
//!   any moment) takes every run back: it reports their exits and still ends what it started.
//! - End (release, master expiry, or 60 s without `lease-beat`: the node's own watchdog): asks the
//!   recorded runs whose leader's start time still matches to exit (Unix: TERM their groups;
//!   Windows: `quit` to their apps' control endpoints), waits while they work, KILLs a run that
//!   took no ask or does nothing and lives on (`stop_runs`: no grace timer), stops the scopes; a
//!   survivor is a `lease_leak`. The last lease on the tree that leaves changes in it:
//!   they are committed (`WIP devhub recovery <id>`), bundled to `<work>/recovered/` and the tree
//!   goes back to the pin; never `git stash`. The tree itself stays (warm). Nothing ends for
//!   running long.
//! - An inspect lease's end removes its breakpoints (`rm G<n>`) and probes (`probe rm P<n>`),
//!   lets parked hits go (`continue all`) and resumes a pause (`resume`), over the JIT's socket.

use super::caps::{self, LeaseConf};
use super::disk::{self, Guard, Live, Roots};
use super::{Kind, Src};
use crate::local::Publish;
use crate::os;
use crate::proto::{hub_record, Sev};
use makepad_remote::json_str;
use makepad_remote::{lines, Endpoint};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// The services this module adds to the node's master endpoint.
/// A lease-end's cleanup receipt with the box's load at the answer
/// (`"load":{"cpu_pct":..,"ram_avail_gb":..}`, [`super::caps::load_now`]), the master's measure of
/// the box until its next caps round: a lease waiting for a box its last lease kept busy starts at
/// the release, not one or two caps rounds later. The saved receipt stays without it (a repeated
/// end answers the load of its own moment).
fn with_load(answer: String, load: impl FnOnce() -> Option<(f64, f64)>) -> String {
    let Some(body) = answer.strip_suffix('}').filter(|_| answer.starts_with("{\"ended\"")) else { return answer };
    match load() {
        Some((cpu, avail)) => format!("{body},\"load\":{{\"cpu_pct\":{cpu:.1},\"ram_avail_gb\":{avail:.2}}}}}"),
        None => answer,
    }
}

pub const WORDS: &[&str] = &["overlay-selftest", "overlay-status", "overlay-why", "lease-start", "lease-run", "lease-log", "lease-end", "lease-beat", "lease-jits", "lease-kill", "lease-why", "ship-have", "ship-put", "ship-link", "ship-done", "lease-owns", "hub-restart", "service-restart", "service-health", "get-ls", "get-read", "policy", "lease-conf", "caps", "disk", "heap", "recover-get", "recover-del"];

/// Marks every process a lease's runs start ([`lease_mark`]): a process that moved to its own session (a
/// WM's apps, a detached helper) is still stopped with the lease. One that outlives its lease on
/// purpose (the check server) removes it.
use makepad_remote::discovery::LEASE_ENV;
/// The mark of the lease a hub itself runs in (a devhub test on a box), passed on to its runs: the
/// box's end of that lease finds what they left too, though they carry their own hub's mark in
/// their own session (a killed test's busy loop burned a core for four hours, 11-04).
const LEASE_OUTER_ENV: &str = "MAKEPAD_DEVHUB_LEASE_OUTER";
const LOG_CHUNK: usize = 64 << 10;
/// The value of [`LEASE_ENV`]: this hub's ledger dir and the lease (the same after a hub restart).
fn lease_mark(state: &Path, id: &str) -> String {
    format!("{}/{id}", state.display())
}
const RECOVER_CHUNK: usize = 256 << 10;
/// `get-read` chunk and `get-ls` limit (results, not trees: big artifacts stay on the node)
const GET_CHUNK: usize = 256 << 10;
const GET_MAX_FILES: usize = 20_000;
/// The new hub's build at a deployment commit, made before this hub exits for it.
#[cfg(feature = "checkout")]
#[derive(Default)]
enum HubBuild {
    #[default]
    None,
    Running { since: Instant },
    Done { at: String, ok: Result<(), String> },
}

/// Timing knobs (tests shorten them) and the node's work tree.
#[derive(Clone, Debug)]
pub struct Opts {
    /// the shared work tree ([`super::tree::root`]); None: no rev leases, including native trees
    pub tree: Option<PathBuf>,
    /// no `lease-beat` for this long: the node ends the lease itself
    pub no_beat: Duration,
    /// how often the watchdog looks
    pub tick: Duration,
    /// TERM to KILL for processes that left their run's group (`stop_strays`); a run itself is
    /// KILLed on its state, not on a timer (`stop_runs`)
    pub grace: Duration,
    /// run under `systemd-run --user --scope` (None: when the user bus exists)
    pub scopes: Option<bool>,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { tree: None, no_beat: Duration::from_secs(60), tick: Duration::from_secs(5), grace: Duration::from_secs(10), scopes: None }
    }
}

struct Run {
    request: String,
    started_ms: u64,
    /// Worker runs may belong to a JIT phase, but still consume lease resources.
    spawned_ms: u64,
    ended_ms: AtomicU64,
    n: u32,
    pid: u32,
    start: u64,
    unit: Option<String>,
    log: PathBuf,
    /// set by the waiter; None while running (or unknown after a hub restart)
    exit: Mutex<Option<i32>>,
    /// started by this hub process (a waiter reaps it)
    ours: bool,
    /// A live job needs to know when its JIT ends, but can lose its exit status on restart.
    detachable: bool,
    /// what keeps its processes together (Windows: its job)
    hold: os::Hold,
    /// where its exit code goes when it ends ([`exit_file`])
    exit_file: PathBuf,
    /// Protect overlay caches until the waiter sees this run exit, not until lease release.
    cache: Mutex<Option<std::fs::File>>,
    usage: Mutex<Usage>,
}

#[derive(Default)]
struct Usage {
    peak: Option<u64>,
    cpu: Option<f64>,
}

impl Usage {
    fn update(&mut self, peak: Option<u64>, cpu: Option<f64>) {
        if let Some(n) = peak { self.peak = Some(self.peak.unwrap_or(0).max(n)); }
        if let Some(n) = cpu.filter(|n| n.is_finite() && *n >= 0.0) { self.cpu = Some(self.cpu.unwrap_or(0.0).max(n)); }
    }
}

/// A process owned by a hub worker. Register before returning it, and reap it even if
/// the worker abandons a comparison while another fixture is still running.
#[cfg(feature = "checkout")]
pub(super) struct WorkerRun {
    child: std::process::Child,
    run: Arc<Run>,
    /// A store build's restart record, removed only after its child is reaped.
    pid_file: Option<PathBuf>,
}

#[cfg(feature = "checkout")]
impl WorkerRun {
    pub(super) fn log(&self) -> &Path { &self.run.log }

    pub(super) fn wait(&mut self) -> Result<std::process::ExitStatus, String> {
        let (status, cpu, maxrss) = os::wait_usage(&mut self.child).map_err(|e| e.to_string())?;
        self.run.reaped(cpu, maxrss);
        let code = os::exit_code(status);
        self.run.finished(code);
        Ok(status)
    }
}

#[cfg(feature = "checkout")]
impl Drop for WorkerRun {
    fn drop(&mut self) {
        if self.run.exit.lock().unwrap().is_none() {
            let _ = stop_runs(std::slice::from_ref(&self.run), &makepad_remote::discovery::registry_dir(), true);
            let _ = self.child.kill();
            let _ = self.wait();
        }
        if self.run.exit.lock().unwrap().is_some() {
            if let Some(path) = &self.pid_file { let _ = std::fs::remove_file(path); }
        }
    }
}

/// Join a store build left by an earlier hub; cancellation leaves its record for the next waiter.
#[cfg(feature = "checkout")]
fn join_recorded_build(path: &Path, active: &dyn Fn() -> Result<(), String>, joining: &dyn Fn(u32)) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut fields = text.split_whitespace();
    let recorded = fields.next().and_then(|s| s.parse::<u32>().ok()).zip(fields.next().and_then(|s| s.parse::<u64>().ok()));
    if let Some((pid, start)) = recorded {
        let mut joined = false;
        loop {
            active()?;
            if !os::same(pid, start) { break; }
            if !joined { joining(pid); joined = true; }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(feature = "checkout")]
#[derive(Default)]
struct LeaseIdentity {
    cancelled: Arc<AtomicBool>,
    /// End stopped every process: late overlay resolutions may clean up their store holds.
    release_jits: AtomicBool,
}

#[cfg(feature = "checkout")]
struct OverlayResolution {
    request: String,
    cwd: String,
    argv: Vec<String>,
    pending: String,
    answer: Option<String>,
    reported: bool,
    /// Keep the start answer until the caller has followed this run to its end.
    run: Option<u32>,
}

#[cfg(feature = "checkout")]
impl OverlayResolution {
    fn answer(&self) -> String {
        self.answer.clone().unwrap_or_else(|| format!("{{\"pending\":{}}}", json_str(&self.pending)))
    }
}

struct RunCommand {
    argv: Vec<String>,
    detachable: bool,
    loader: Option<PathBuf>,
    cache: Option<std::fs::File>,
    notices: Vec<String>,
}

struct Lease {
    id: String,
    /// An old overlay resolution must not start work for a replacement lease with the same id.
    #[cfg(feature = "checkout")]
    identity: Arc<LeaseIdentity>,
    #[cfg(feature = "checkout")]
    resolution: Option<OverlayResolution>,
    /// Joined after cancellation and process shutdown, before deleting fixture state.
    #[cfg(feature = "checkout")]
    selftest: Option<std::thread::JoinHandle<()>>,
    lane: String,
    warms: Option<String>,
    kind: Kind,
    src: Src,
    loader: Option<String>,
    cost: super::Cost,
    /// the RAM the lease asked for itself (`--ram-gb`): its run's hard memory cap (none otherwise)
    ram_max_gb: Option<f64>,
    /// a live session (an app or a warm test process that stays up): its JIT sizes its own
    /// threads from the box, never from the lease's cores ([`job_env`])
    live: bool,
    first_run: String,
    shipped: bool,
    phases: super::prof::Phases,
    work: PathBuf,
    target: PathBuf,
    tmp: PathBuf,
    env: Vec<(String, String)>,
    /// Pinned commits (`<makepad>[,<commercial>]`) for the tree or store mounts.
    pin: Option<String>,
    /// Shipped paths; tree leases undo them at release, an overlay lease's folder goes with it.
    overlay: Vec<String>,
    /// Which source the last ship made ([`super::ship::source_id`]).
    source: String,
    /// What the tree lease's ships put in its tree and took out of it since its last run started:
    /// the next run starts only when they are still so ([`Lease::unshipped`]).
    fresh_put: Vec<String>,
    fresh_gone: Vec<String>,
    runs: Vec<Arc<Run>>,
    peak_ram_bytes: Option<u64>,
    /// The most its live runs held at one look so far ([`held_now`]): what the master charges it.
    held_bytes: Option<u64>,
    beat: Instant,
    /// Since when its live runs have done nothing ([`busy`]), and what they had done by then.
    /// None: no live run, or one this box cannot measure.
    quiet: Option<(Instant, os::Activity)>,
}

impl Lease {
    fn tree_lane(&self) -> &str { self.warms.as_deref().unwrap_or(&self.lane) }

    /// Its directories, and whether each is its own: CI's tree and target and the overlays'
    /// target (under `work`, the box's work dir) belong to every lease that builds in them, so
    /// their growth is no one lease's and ending one frees none of it.
    fn dirs(&self, work: &Path) -> Vec<(PathBuf, bool)> {
        let ci = self.target == work.join("ci-target");
        vec![(self.work.clone(), !ci), (self.target.clone(), !ci && self.src != Src::Overlay), (self.tmp.clone(), true)]
    }

    /// Why the next run must not start: what its ships put in the tree is gone, or what they
    /// took out is back (a startup scrub once took a timed lease's shipped files between its ship
    /// and its run, 11-04). Every such path is named.
    fn unshipped(&self) -> Option<String> {
        let at = |p: &String| std::fs::symlink_metadata(self.work.join(p)).is_ok();
        let missing: Vec<&str> = self.fresh_put.iter().filter(|p| !at(p)).map(String::as_str).collect();
        let back: Vec<&str> = self.fresh_gone.iter().filter(|p| at(p)).map(String::as_str).collect();
        let mut why = Vec::new();
        if !missing.is_empty() { why.push(format!("{} shipped file(s) missing from its tree: {}", missing.len(), missing.join(" "))); }
        if !back.is_empty() { why.push(format!("{} shipped deletion(s) back in its tree: {}", back.len(), back.join(" "))); }
        (!why.is_empty()).then(|| why.join("; "))
    }

    fn answer(&self) -> String {
        let env: Vec<String> = self.env.iter().map(|(k, v)| format!("{}:{}", json_str(k), json_str(v))).collect();
        format!(
            "{{\"ok\":1,\"user\":{},\"work\":{},\"target\":{},\"tmp\":{},\"env\":{{{}}}}}",
            json_str(&login()),
            json_str(&self.work.to_string_lossy()),
            json_str(&self.target.to_string_lossy()),
            json_str(&self.tmp.to_string_lossy()),
            env.join(",")
        )
    }

    fn profile(&self, now: u64, ending: bool) -> super::prof::Phases {
        let mut phases = self.phases.clone();
        // a run starts on what was shipped: a ship left open (no ship-done) ends there, its run
        // time is not sync
        for s in phases.0.iter_mut().filter(|s| s.phase == "sync" && s.end == 0) {
            if let Some(t) = self.runs.iter().map(|r| r.started_ms).filter(|t| *t > 0 && *t >= s.start).min() { s.end = t; }
        }
        if ending {
            phases.close("sync", now);
            phases.close("jit", now);
        }
        for r in self.runs.iter().filter(|r| r.started_ms > 0) {
            let end = r.ended();
            phases.push("run", r.started_ms, if end > 0 { end.min(now) } else if ending { now } else { 0 });
            // a timed run's untimed warm-up ([`warmed_up`]), inside its run: what it costs
            let warmed = std::fs::metadata(r.exit_file.with_extension("warmup")).and_then(|m| m.modified()).ok();
            if let Some(at) = warmed.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
                phases.push("warmup", r.started_ms, (at.as_millis() as u64).max(r.started_ms + 1));
            }
        }
        phases
    }

    /// The lease's edits folder, as its run has it (a lease a hub restart brought back keeps the one
    /// its run started with).
    #[cfg(feature = "checkout")]
    fn edits(&self) -> Option<PathBuf> {
        self.env.iter().find(|(k, _)| k == "MAKEPAD_RUN_EDITS").map(|(_, v)| PathBuf::from(v))
    }

    fn ledger(&self) -> String {
        let runs: Vec<String> = self
            .runs
            .iter()
            .map(|r| format!("{{\"request_id\":{},\"n\":{},\"pid\":{},\"start\":{},\"started_ms\":{},\"spawned_ms\":{},\"detachable\":{},\"unit\":{},\"log\":{}}}", json_str(&r.request), r.n, r.pid, r.start, r.started_ms, r.spawned_ms, r.detachable, r.unit.as_deref().map_or("null".into(), json_str), json_str(&r.log.to_string_lossy())))
            .collect();
        let (a, source) = (self.answer(), &self.source);
        // an answered start (a run) survives a hub restart: the holder's next poll gets the same run
        #[cfg(feature = "checkout")]
        let resolution = self.resolution.as_ref().and_then(|r| r.run.map(|n| (r, n))).map_or(String::new(), |(r, n)| {
            let pid = self.runs.iter().find(|run| run.n == n).map_or(0, |run| run.pid);
            format!("\"resolution\":{{\"request_id\":{},\"cwd\":{},\"argv\":[{}],\"n\":{n},\"pid\":{pid},\"reported\":{}}},", json_str(&r.request), json_str(&r.cwd), r.argv.iter().map(|a| json_str(a)).collect::<Vec<_>>().join(","), r.reported)
        });
        #[cfg(not(feature = "checkout"))]
        let resolution = "";
        let warms = self.warms.as_deref().map_or(String::new(), |lane| format!(",\"warms\":{}", json_str(lane)));
        format!("{{{resolution}\"id\":{},\"lane\":{}{warms},\"kind\":\"{}\",\"src\":\"{}\",\"loader\":{},\"cost\":{{\"cores\":{},\"ram_gb\":{},\"ram_max_gb\":{}}},\"live\":{},\"peak_ram_bytes\":{},\"held_bytes\":{},\"first_run\":{},\"shipped\":{},\"phases\":{},\"pin\":{},\"overlay\":[{}],\"source\":\"{source}\",\"fresh_put\":[{}],\"fresh_gone\":[{}],\"runs\":[{}],{}", json_str(&self.id), json_str(&self.lane), self.kind.as_str(), self.src.as_str(), self.loader.as_deref().map_or("null".into(), json_str), self.cost.cores, self.cost.ram_gb, self.ram_max_gb.map_or("null".into(), |g| g.to_string()), self.live, self.peak_ram_bytes.map_or("null".into(), |n| n.to_string()), self.held_bytes.map_or("null".into(), |n| n.to_string()), json_str(&self.first_run), self.shipped, self.phases.json(), self.pin.as_deref().map_or("null".into(), json_str), self.overlay.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), self.fresh_put.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), self.fresh_gone.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), runs.join(","), &a[1..])
    }

    fn from_ledger(text: &str) -> Option<Lease> {
        let v = makepad_strict_json::parse(text.as_bytes()).ok()?;
        let s = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let n = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(super::num).unwrap_or(0.0) as u64;
        let paths = |k: &str| v.get(k).and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect::<Vec<_>>());
        let mut env = Vec::new();
        if let Some(makepad_strict_json::Value::Obj(o)) = v.get("env") {
            for (k, x) in o {
                env.push((k.to_string(), x.as_str()?.to_string()));
            }
        }
        let runs = v.get("runs").and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| {
            a.iter()
                .filter_map(|r| Some(Arc::new(Run { request: s(r, "request_id").unwrap_or_default(), started_ms: n(r, "started_ms"), spawned_ms: n(r, "spawned_ms").max(n(r, "started_ms")), ended_ms: AtomicU64::new(0), n: n(r, "n") as u32, pid: n(r, "pid") as u32, start: n(r, "start"), unit: s(r, "unit"), log: s(r, "log")?.into(), exit: Mutex::new(None), ours: false, detachable: r.get("detachable").and_then(|v| v.as_bool()).unwrap_or(false), hold: os::Hold::none(), exit_file: PathBuf::new(), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) })))
                .collect()
        });
        Some(Lease {
            id: s(&v, "id")?,
            #[cfg(feature = "checkout")]
            identity: Arc::default(),
            #[cfg(feature = "checkout")]
            resolution: v.get("resolution").and_then(|r| {
                let (n, pid) = (n(r, "n") as u32, n(r, "pid"));
                let argv = r.get("argv")?.as_arr()?.iter().map(|a| a.as_str().map(str::to_string)).collect::<Option<Vec<_>>>()?;
                Some(OverlayResolution { request: s(r, "request_id").unwrap_or_default(), cwd: s(r, "cwd")?, argv, pending: String::new(), answer: Some(format!("{{\"run\":{pid},\"n\":{n}}}")), reported: r.get("reported").and_then(|v| v.as_bool()).unwrap_or(false), run: Some(n) })
            }),
            #[cfg(feature = "checkout")]
            selftest: None,
            lane: s(&v, "lane")?,
            warms: s(&v, "warms"),
            kind: Kind::parse(&s(&v, "kind")?)?,
            src: s(&v, "src").as_deref().and_then(Src::parse).unwrap_or_default(),
            loader: s(&v, "loader"),
            cost: lease_cost(&v, Kind::parse(&s(&v, "kind")?)?),
            ram_max_gb: lease_ram_max(&v),
            live: live_of(&v),
            first_run: s(&v, "first_run").unwrap_or_default(),
            shipped: v.get("shipped").and_then(|v| v.as_bool()).unwrap_or(false),
            phases: super::prof::Phases::parse(v.get("phases")),
            work: s(&v, "work")?.into(),
            target: s(&v, "target")?.into(),
            tmp: s(&v, "tmp")?.into(),
            env,
            pin: s(&v, "pin"),
            overlay: paths("overlay"),
            source: s(&v, "source").unwrap_or_default(),
            fresh_put: paths("fresh_put"),
            fresh_gone: paths("fresh_gone"),
            runs,
            peak_ram_bytes: v.get("peak_ram_bytes").and_then(|v| v.as_u64()),
            held_bytes: v.get("held_bytes").and_then(|v| v.as_u64()),
            beat: Instant::now(),
            quiet: None,
        })
    }
}

/// A live session's lease (the master's `live`).
fn live_of(v: &makepad_strict_json::Value) -> bool {
    v.get("live").and_then(|x| x.as_bool()).unwrap_or(false)
}

/// The lease's own RAM ask (`cost.ram_max_gb`): its hard cap; a measured one is no cap.
fn lease_ram_max(v: &makepad_strict_json::Value) -> Option<f64> {
    v.get("cost").and_then(|c| c.get("ram_max_gb")).and_then(super::num).filter(|n| n.is_finite() && *n > 0.0)
}

/// Numeric allocation sent by the master; kind defaults also describe direct node requests.
fn lease_cost(v: &makepad_strict_json::Value, kind: Kind) -> super::Cost {
    let mut cost = super::Cost::default_for(kind, false);
    if let Some(v) = v.get("cost") {
        let positive = |k| v.get(k).and_then(super::num).filter(|n| n.is_finite() && *n > 0.0);
        if let Some(n) = positive("cores") { cost.cores = n; }
        if let Some(n) = positive("ram_gb") { cost.ram_gb = n; }
    }
    cost
}

fn jobs(kind: Kind, cost: super::Cost) -> u32 {
    if kind == Kind::Timed { std::thread::available_parallelism().map_or(1, |n| n.get() as u32) } else { (cost.cores.round() as u32).max(1) }
}

/// The thread counts a run gets from its lease's cores. A live session's JIT gets none: the
/// lease's cores are what the scheduler counts, its cgroup weight is its share under contention,
/// and the app's own CPU budget sizes its threads from the box (a live Fab on one JIT worker
/// waited up to 42 s).
fn job_env(kind: Kind, cost: super::Cost, live: bool) -> Vec<(&'static str, u32)> {
    let jobs = jobs(kind, cost);
    [("MAKEPAD_RUN_JOBS", jobs)].into_iter().filter(|_| !live).chain([("CARGO_BUILD_JOBS", jobs), ("RUST_TEST_THREADS", jobs)]).collect()
}

fn overlay_lane(kind: Kind, lane: &str, pin_key: &str) -> String {
    if kind == Kind::Ci {
        // the whole pin: two pairs at one makepad commit and different commercial ones differ
        if let Some(pin) = super::Pin::parse(pin_key) { return format!("{lane}-{}", makepad_network::digest::sha256_hash(pin.key().as_bytes())[..6].iter().map(|b| format!("{b:02x}")).collect::<String>()); }
    }
    lane.to_string()
}

/// The node's lease executor.
pub struct Exec {
    #[cfg(feature = "checkout")]
    this: Weak<Exec>,
    pub machine: String,
    publish: Publish,
    conf: std::sync::RwLock<LeaseConf>,
    /// the ledger folder (`~/.makepad/devhub/leases`)
    state: PathBuf,
    opts: Opts,
    scopes: bool,
    dev: AtomicBool,
    /// a flip asked for on this node itself, until the master's policy push answers it (0 none, 1 on, 2 off)
    dev_req: std::sync::atomic::AtomicU8,
    leases: crate::held::Held<BTreeMap<String, Lease>>,
    starting: crate::held::Held<()>,
    /// The lanes `disk clean` takes apart now: a lease of one waits for it, and only that lease
    /// (its size and its delete under [`Self::starting`] held up every start and the hub's restart).
    cleaning: (Mutex<BTreeSet<String>>, std::sync::Condvar),
    /// Duplicate end requests observe the in-flight or durable final receipt.
    endings: Mutex<BTreeMap<String, String>>,
    /// orphans of ended leases already sent TERM ([`Exec::end_orphans`])
    orphans: Mutex<Vec<u32>>,
    pub guard: Guard,
    /// lease id -> what its directories hold: measured off the beat
    disk_use: Arc<Mutex<std::collections::HashMap<String, DiskUse>>>,
    /// timed lease id -> what shared its box ([`Noise`], [`timed_box_loop`]; Windows runs have no
    /// process groups to tell a run's own processes by)
    #[cfg(unix)]
    noise: Mutex<std::collections::HashMap<String, Noise>>,
    /// the lease that took the disk, and why ([`lease_over_disk`]): its master ends it
    over_disk: Mutex<Option<(String, String)>>,
    /// a person at the node, as its WM saw them ([`super::user`])
    user: Mutex<super::user::UserTrack>,
    /// `hub-restart` was answered: this hub exits in a moment and starts no run before it does
    restarting: AtomicBool,
    /// The new hub's build before `hub-restart` exits for it ([`Exec::hub_built`]).
    #[cfg(feature = "checkout")]
    hub_build: Arc<Mutex<HubBuild>>,
    /// the last [`Exec::resident_gb`] measured
    last_resident_gb: Mutex<f64>,
    /// the outage before this hub ran, as its WM recorded it ([`take_trouble`]): given to the
    /// master in the next caps answer only
    trouble: Mutex<String>,
    /// the master's last `lease-beat` (it beats every node it knows, naming the leases it holds
    /// there): it is away once that is [`Opts::no_beat`] old, and the leases are kept for it
    /// ([`Exec::expire_at`])
    master_seen: Mutex<Instant>,
    /// its absence was said (once per absence)
    master_away_said: AtomicBool,
}

/// What a lease run inherits from the hub's environment: the system's basics (paths, user,
/// locale, toolchain homes, the user bus `systemd-run --user` needs, Windows' system vars). The
/// WM session's (`MAKEPAD_WM_*`, the display) only for screen and CI leases: the kinds that show
/// windows opt in; every other run must not reach the node's live WM. A Linux lease with
/// `needs.gui=1` gets its WM session explicitly from caps in the lease's environment law.
pub fn base_env(kind: Kind, vars: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    const KEEP: &[&str] = &[
        "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LANGUAGE", "TERM", "TZ", "CARGO_HOME", "RUSTUP_HOME", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS",
        // platform SDKs (check_platforms, Android and Apple builds)
        "ANDROID_HOME", "ANDROID_SDK_ROOT", "ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "JAVA_HOME", "DEVELOPER_DIR", "SDKROOT",
        // Windows
        "SYSTEMROOT", "WINDIR", "TEMP", "TMP", "USERPROFILE", "USERNAME", "APPDATA", "LOCALAPPDATA", "PROGRAMDATA", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432", "COMMONPROGRAMFILES", "COMSPEC", "PATHEXT", "NUMBER_OF_PROCESSORS", "PROCESSOR_ARCHITECTURE", "OS", "SYSTEMDRIVE", "HOMEDRIVE", "HOMEPATH",
    ];
    // a screen or CI lease reaches the node's WM (`MAKEPAD_WM_*`) but never its checkout
    // (`MAKEPAD_WM_ROOT`): a WM the lease starts runs its apps from the lease's own sources
    const SCREEN: &[&str] = &["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP"];
    vars.filter(|(k, _)| {
        let up = k.to_ascii_uppercase();
        KEEP.contains(&up.as_str()) || up.starts_with("LC_") || (matches!(kind, Kind::Screen | Kind::Ci) && (SCREEN.contains(&up.as_str()) || (up.starts_with("MAKEPAD_WM_") && up != "MAKEPAD_WM_ROOT")))
    })
    .collect()
}

/// The WM advertised by this node's caps, and its own Wayland server when it has one.
/// Socket indices are allocated independently: match their server pid, never their suffix
/// or the hub's inherited display. Called once at lease-start, off the UI thread.
#[cfg(target_os = "linux")]
fn gui_env() -> Result<Vec<(&'static str, String)>, String> {
    use std::os::unix::{io::AsRawFd, net::UnixStream};

    fn peer_pid(path: &Path) -> std::io::Result<u32> {
        extern "C" {
            fn getsockopt(fd: i32, level: i32, name: i32, val: *mut u8, len: *mut u32) -> i32;
        }
        let stream = UnixStream::connect(path)?;
        // Linux SOL_SOCKET / SO_PEERCRED: struct ucred { pid, uid, gid }.
        let mut cred = [0u32; 3];
        let mut len = std::mem::size_of_val(&cred) as u32;
        if unsafe { getsockopt(stream.as_raw_fd(), 1, 17, cred.as_mut_ptr() as *mut u8, &mut len) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if len as usize != std::mem::size_of_val(&cred) || cred[0] == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "WM socket has no server pid"));
        }
        Ok(cred[0])
    }

    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|s| !s.is_empty()).map(PathBuf::from).ok_or("gui=1: XDG_RUNTIME_DIR is not set")?;
    let display = caps::display().ok_or("gui=1: caps has no live WM display")?;
    let name = display.strip_prefix("wm:").ok_or("gui=1: caps display is not a WM session")?;
    let socket = runtime.join(name);
    let pid = peer_pid(&socket).map_err(|e| format!("gui=1: {}: {e}", socket.display()))?;
    let mut wayland: Vec<_> = std::fs::read_dir(&runtime).map_err(|e| format!("gui=1: {}: {e}", runtime.display()))?.flatten().filter_map(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        let index = name.strip_prefix("wayland-wm-")?.parse::<u32>().ok()?;
        Some((index, name, e.path()))
    }).collect();
    wayland.sort_by_key(|(index, _, _)| *index);
    let wayland = wayland.into_iter().find_map(|(_, name, path)| (peer_pid(&path).ok() == Some(pid)).then_some(name));
    Ok(vec![
        ("XDG_RUNTIME_DIR", runtime.to_string_lossy().into_owned()),
        ("MAKEPAD_WM_SOCKET", socket.to_string_lossy().into_owned()),
        // Empty also overrides an unrelated display in a screen lease's inherited/requested
        // environment. The HAL treats it as unset; this WM may only host Makepad windows.
        ("WAYLAND_DISPLAY", wayland.unwrap_or_default()),
    ])
}

/// Lease ids, lanes (including warms), env keys: what a path, a unit name and a line can hold.
pub(super) fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.starts_with(['-', '.']) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') && !s.contains("..")
}

/// The lines of a run's log an abnormal end keeps.
const TAIL_LINES: usize = 20;
/// The lines of a run's log `lease-why` shows.
const WHY_LINES: usize = 12;

/// How a run ended when that was not its normal end: a signal, an exit nobody saw, or a test run
/// whose output ends without libtest's summary. (its line, whether it is more than a failure, the
/// log's last lines)
fn abnormal_end(id: &str, kind: Kind, r: &Run, exit: i32) -> Option<(String, bool, String)> {
    let tail = log_tail(&r.log, TAIL_LINES);
    let summary = kind != Kind::Test || tail.lines().any(|l| l.starts_with("test result:"));
    if exit == 0 || exit == -2 || (exit > 0 && exit <= 128 && summary) { return None; }
    let (line, serious) = ended_how(id, r.n, r.unit.as_deref(), exit, kernel_log(r.started_ms, crate::proto::wall_ms()).as_deref());
    Some((line, serious, tail))
}

/// The last 64 KB of the last run that reports failed tests: all that names them once the lease's
/// tree and run logs are gone, whoever held the lease and wherever its output went.
fn failed_tests(runs: &[Arc<Run>]) -> Option<String> {
    runs.iter().rev().find_map(|r| {
        let tail = log_tail(&r.log, usize::MAX);
        tail.lines().any(|l| l.starts_with("test result: FAILED")).then_some(tail)
    })
}

/// The tests of a libtest log that said they have run for over a minute, with no result since.
fn tests_running(log: &str) -> Vec<&str> {
    let mut running = Vec::new();
    for test in log.lines().filter_map(|l| l.strip_prefix("test ")) {
        if let Some(name) = test.strip_suffix(" has been running for over 60 seconds") {
            running.push(name);
        } else if let Some((name, _)) = test.split_once(" ... ") {
            running.retain(|n| *n != name);
        }
    }
    running
}

/// How far a run got: its state, the tests running long, and the last `n` lines of its log.
fn got_to(r: &Run, n: usize) -> String {
    r.adopted_exit();
    let state = (*r.exit.lock().unwrap()).map_or("still running".to_string(), |e| format!("ended with exit {e}"));
    let tail = log_tail(&r.log, usize::MAX);
    let long = tests_running(&tail);
    let long = if long.is_empty() { String::new() } else { format!("\nrunning for over a minute: {}", long.join(", ")) };
    let lines: Vec<&str> = tail.lines().collect();
    format!("run {}: {state}{long}\n{}\n", r.n, lines[lines.len().saturating_sub(n)..].join("\n"))
}

/// The last `n` lines of a log (its last 64 KB at most).
fn log_tail(log: &Path, n: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut buf = Vec::new();
    if let Ok(mut f) = std::fs::File::open(log) {
        let len = f.metadata().map_or(0, |m| m.len());
        if f.seek(SeekFrom::Start(len.saturating_sub(64 << 10))).is_ok() { let _ = f.read_to_end(&mut buf); }
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The kernel's lines that say why a process died (OOM kills, GPU faults) between two times.
#[cfg(target_os = "linux")]
fn kernel_log(from_ms: u64, to_ms: u64) -> Option<String> {
    let o = std::process::Command::new("journalctl")
        .args(["-k", "-q", "--no-pager", "-o", "cat", &format!("--since=@{}", from_ms / 1000), &format!("--until=@{}", to_ms / 1000 + 2), "-g", "oom-kill:|Killed process|memory: usage|NVRM: Xid|fallen off the bus|GPU reset"])
        .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    // journalctl -g exits 1 when nothing matched
    (o.status.success() || o.stdout.is_empty()).then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

#[cfg(not(target_os = "linux"))]
fn kernel_log(_from_ms: u64, _to_ms: u64) -> Option<String> { None }

/// The abnormal end's line: the exit, the OOM kill of the run's scope (`unit`) and a GPU fault
/// the kernel logged in its time; and whether it is more than a plain failure.
fn ended_how(id: &str, n: u32, unit: Option<&str>, exit: i32, kernel: Option<&str>) -> (String, bool) {
    const SIGNALS: [&str; 16] = ["", "SIGHUP", "SIGINT", "SIGQUIT", "SIGILL", "SIGTRAP", "SIGABRT", "SIGBUS", "SIGFPE", "SIGKILL", "SIGUSR1", "SIGSEGV", "SIGUSR2", "SIGPIPE", "SIGALRM", "SIGTERM"];
    let mut serious = exit > 128 || exit < 0;
    let mut out = format!("{id} run {n} ended without its summary: {}", match exit {
        e if e > 128 => format!("killed by signal {} ({})", e - 128, SIGNALS.get((e - 128) as usize).copied().filter(|s| !s.is_empty()).unwrap_or("?")),
        e if e < 0 => "its exit is unknown (its hub restarted while it ran)".to_string(),
        e => format!("exit {e}"),
    });
    let Some(kernel) = kernel else {
        out.push_str("; the kernel log was not read here");
        return (out, serious);
    };
    let lines: Vec<&str> = kernel.lines().collect();
    let gb = |kb: &str| kb.trim_end_matches("kB").parse::<f64>().map_or(String::new(), |k| format!("{:.1} GB", k / (1u64 << 20) as f64));
    let ours = |l: &str| l.contains("oom-kill:") && unit.is_none_or(|u| l.contains(&format!("/{u}.scope")));
    match lines.iter().position(|l| ours(l)) {
        Some(i) => {
            serious = true;
            let killed = lines[i..].iter().find_map(|l| l.split_once("Killed process ").map(|(_, k)| k));
            let who = killed.map_or("a process", |k| k.split(" total-vm").next().unwrap_or(k));
            let rss = killed.and_then(|k| k.split("anon-rss:").nth(1)).and_then(|r| r.split(',').next()).map(gb).filter(|g| !g.is_empty());
            let cap = lines[..i].iter().rev().find_map(|l| l.strip_prefix("memory: usage ")).and_then(|u| u.split("limit ").nth(1)).and_then(|l| l.split(',').next()).map(gb);
            let at = if lines[i].contains("CONSTRAINT_MEMCG") { format!("at the lease's memory cap{}", cap.map_or(String::new(), |c| format!(" of {c}"))) } else { "when the box ran out of memory".to_string() };
            out.push_str(&format!("; the kernel's OOM killer killed {who}{} {at}", rss.map_or(String::new(), |r| format!(", {r} resident,"))));
        }
        None => out.push_str("; no OOM kill"),
    }
    if let Some(g) = lines.iter().find(|l| l.contains("NVRM: Xid") || l.contains("fallen off the bus") || l.contains("GPU reset")) {
        serious = true;
        out.push_str(&format!("; GPU: {}", crate::proto::clean(g.trim(), 160)));
    }
    (out, serious)
}

/// Where a run's exit code is written when it ends: beside the ledger, not in the lease's tmp (a
/// run stopped by the lease's end would write into the tmp being removed).
/// Writes `text` aside and renames it into place: a reader sees the whole file or the one before,
/// never an empty one. A hub that took a run back read the exit file in the moment the old hub's
/// waiter had truncated it, found the wrapper gone and called the run lost (-1, not its 7: a
/// flaky restart test, 11-04).
fn write_whole(path: &Path, text: &str) {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    if std::fs::write(&part, text).and_then(|_| std::fs::rename(&part, path)).is_err() {
        let _ = std::fs::remove_file(&part);
    }
}

/// A file [`write_whole`] writes: its text, or none when it is not there. Windows replaces a
/// file by marking the old one for deletion first, and an open in that moment is refused (access
/// denied: delete pending), so such a read is asked again, at once and then a millisecond later
/// (yields alone ran out on a loaded box before the writer's rename ended: a test's reads failed
/// 2497 times, 11-04); elsewhere the rename is one step.
fn read_whole(path: &Path) -> Option<String> {
    read_whole_or(path).ok()
}

/// [`read_whole`] with the error of a read that failed.
fn read_whole_or(path: &Path) -> std::io::Result<String> {
    #[cfg(windows)]
    for i in 0..REPLACE_READS {
        match std::fs::read_to_string(path) {
            Err(e) if e.raw_os_error() == Some(5) && i < REPLACE_SPINS => std::thread::yield_now(),
            Err(e) if e.raw_os_error() == Some(5) => std::thread::sleep(Duration::from_millis(1)),
            read => return read,
        }
    }
    std::fs::read_to_string(path)
}

/// How often a read that meets a replace in its delete-pending moment is asked again (Windows):
/// the first [`REPLACE_SPINS`] times at once, the rest a millisecond apart.
#[cfg(windows)]
const REPLACE_READS: usize = 300;
#[cfg(windows)]
const REPLACE_SPINS: usize = 100;

fn exit_file(state: &Path, id: &str, n: u32) -> PathBuf {
    state.join(format!("{id}-{n}.exit"))
}

/// A timed run on macOS first runs the JIT binary it starts (the box's, built for its tree) once
/// with `--version`, untimed: macOS scans a newly written program on its first run, and the scan
/// went into the run's numbers (0.65 s for the 62 MB JIT binary; about 0.2 s and 6.5 ms per MB,
/// paid per file: a copy is scanned again). Programs the command builds itself are scanned at
/// their own first run: a timed series takes a lead-in round. (The whole command ran twice
/// before, 4960 s untimed beside 8344 s timed over 17 leases, 11-04.) `mark` is written when it
/// is done: the profile's `warmup` span ([`Lease::profile`]).
fn warmed_up(argv: Vec<String>, jit: &Path, mark: &Path) -> Vec<String> {
    let script = r#"mark=$1
jit=$2
shift 2
if "$jit" --version > /dev/null 2>&1 < /dev/null; then
    printf 'devhub: %s ran once first, untimed (macOS scans a new program on its first run)\n' "${jit##*/}"
else
    printf 'devhub: %s --version failed (exit %s) before the timed run\n' "${jit##*/}" "$?"
fi
: > "$mark"
exec "$@"
"#;
    let mut full = vec!["sh".into(), "-c".into(), script.into(), "devhub-warmup".into(), mark.to_string_lossy().into_owned(), jit.to_string_lossy().into_owned()];
    full.extend(argv);
    full
}

impl Run {
    fn work_pid(&self) -> u32 {
        std::fs::read_to_string(self.exit_file.with_extension("usage.pid")).ok().and_then(|s| s.trim().parse().ok())
            .filter(|pid| os::in_group(*pid, self.pid, &self.hold)).unwrap_or(self.pid)
    }

    /// What its waiter read as it reaped the run: its CPU, and the largest resident set one of its
    /// processes reached, the only peak of a run that ended before a beat looked at it (the Mac
    /// show prewarm's 20 ms runs recorded none, so its class never had the three peaks that let
    /// it ask its own and it asked a box's whole budget, 11-04).
    fn reaped(&self, cpu: Option<f64>, maxrss: Option<u64>) {
        self.usage.lock().unwrap().update(maxrss, cpu);
        if let Some(cpu) = cpu {
            let _ = std::fs::write(self.exit_file.with_extension("cpu"), cpu.to_string());
        }
    }

    fn sample(&self) {
        let (peak, cpu) = os::scope_usage(self.pid, self.start, self.unit.as_deref(), &self.exit_file.with_extension("usage"));
        let mut sample = Usage { peak, cpu };
        if let Ok(s) = std::fs::read_to_string(self.exit_file.with_extension("usage")) {
            for line in s.lines() {
                if let Some(n) = line.strip_prefix("peak ").and_then(|s| s.parse().ok()) { sample.update(Some(n), None); }
                if let Some(n) = line.strip_prefix("cpu ").and_then(|s| s.parse::<f64>().ok()) { sample.update(None, Some(n / 1_000_000.0)); }
            }
        }
        if let Some(cpu) = std::fs::read_to_string(self.exit_file.with_extension("cpu")).ok().and_then(|s| s.parse().ok()) { sample.update(None, Some(cpu)); }
        self.usage.lock().unwrap().update(sample.peak, sample.cpu);
    }

    fn ended(&self) -> u64 {
        let known = self.ended_ms.load(Ordering::Acquire);
        if known > 0 || self.ours || self.exit_file.as_os_str().is_empty() { return known; }
        let end = read_whole(&self.exit_file.with_extension("time")).and_then(|s| s.trim().parse().ok()).unwrap_or_else(|| {
            // Adopted processes have no waiter. Their first observed exit is the end sample.
            let alive = if self.detachable { os::same(self.pid, self.start) } else { os::group_alive(self.pid, self.start, &self.hold) };
            if self.recorded_exit().is_some() || !alive { crate::proto::wall_ms() } else { 0 }
        });
        if end > 0 { self.ended_ms.store(end, Ordering::Release); }
        end
    }

    fn finished(&self, code: i32) {
        let end = self.ended_ms.load(Ordering::Acquire);
        let end = if end > 0 { end } else { crate::proto::wall_ms() };
        self.ended_ms.store(end, Ordering::Release);
        // Keep the old exit-code file readable by older hubs during a rolling update.
        write_whole(&self.exit_file.with_extension("time"), &end.to_string());
        write_whole(&self.exit_file, &code.to_string());
        *self.exit.lock().unwrap() = Some(code);
    }

    /// The exit the waiter wrote (a run of an earlier hub process).
    fn recorded_exit(&self) -> Option<i32> {
        read_whole(&self.exit_file)?.trim().parse().ok()
    }

    /// Poll a run taken back from the ledger under the lease table lock. Its waiter is gone: its
    /// wrapper's recorded exit, else a gone live run's -2 (it had no wrapper) or a gone group's
    /// -1; then it lets go of the dev cache it held.
    fn adopted_exit(&self) {
        if self.ours || self.exit.lock().unwrap().is_some() { return; }
        if let Some(code) = self.recorded_exit() {
            *self.exit.lock().unwrap() = Some(code);
        } else if self.detachable {
            if self.start != 0 && os::same(self.pid, self.start) { return; }
            self.finished(-2);
        } else {
            if self.pid != 0 && os::group_alive(self.pid, self.start, &self.hold) { return; }
            *self.exit.lock().unwrap() = Some(-1);
        }
        drop(self.cache.lock().unwrap().take());
    }

    /// Still running: no exit known, and (not ours) its group alive.
    fn live(&self) -> bool {
        self.exit.lock().unwrap().is_none() && (self.ours || (self.pid != 0 && self.recorded_exit().is_none() && os::group_alive(self.pid, self.start, &self.hold)))
    }
}

/// How often a timed lease's box is looked at ([`Noise`]).
#[cfg(unix)]
const BOX_EVERY: Duration = Duration::from_secs(10);

/// What shared a timed lease's box, a look every [`BOX_EVERY`]: the load and the CPU (% of one
/// core) of every process outside the lease's process groups and this hub. Its report carries it
/// (`box`), so a noisy run is seen: a Mac's Spotlight indexed what builds wrote at 180% of a core
/// under a timed lease (11-03).
#[cfg(unix)]
#[derive(Default)]
struct Noise {
    samples: u32,
    load1_max: f64,
    outside_sum: f64,
    outside_max: f64,
    by_name: BTreeMap<String, f64>,
}

#[cfg(unix)]
impl Noise {
    fn add(&mut self, load1: f64, outside: &[(String, f64)]) {
        let total: f64 = outside.iter().map(|(_, c)| c).sum();
        self.samples += 1;
        self.load1_max = self.load1_max.max(load1);
        self.outside_sum += total;
        self.outside_max = self.outside_max.max(total);
        for (name, cpu) in outside {
            *self.by_name.entry(name.clone()).or_default() += cpu;
        }
    }

    /// `{"samples":n,"load1_max":x,"outside_cpu_pct":mean,"outside_cpu_pct_max":max,"busiest":[[name,mean],..]}`:
    /// the five busiest outside processes by their mean over the looks.
    fn json(&self) -> String {
        let n = self.samples.max(1) as f64;
        let mut busiest: Vec<(&String, f64)> = self.by_name.iter().map(|(k, v)| (k, v / n)).collect();
        busiest.sort_by(|a, b| b.1.total_cmp(&a.1));
        let busiest: Vec<String> = busiest.iter().take(5).map(|(k, v)| format!("[{},{v:.0}]", json_str(k))).collect();
        format!("{{\"samples\":{},\"load1_max\":{:.1},\"outside_cpu_pct\":{:.0},\"outside_cpu_pct_max\":{:.0},\"busiest\":[{}]}}", self.samples, self.load1_max, self.outside_sum / n, self.outside_max, busiest.join(","))
    }
}

/// Looks at the box while a timed lease runs ([`Noise`]); the processes of its runs' groups are
/// its own work.
#[cfg(unix)]
fn timed_box_loop(w: Weak<Exec>) {
    loop {
        std::thread::sleep(BOX_EVERY);
        let Some(x) = w.upgrade() else { return };
        // each run's process group, by whose it is: the timed leases' own (""), another lease's
        let (timed, groups) = {
            let leases = x.leases.lock().unwrap();
            let timed: Vec<String> = leases.values().filter(|l| l.kind == Kind::Timed).map(|l| l.id.clone()).collect();
            let groups: std::collections::HashMap<u32, String> = leases.values().flat_map(|l| {
                let tag = if l.kind == Kind::Timed { String::new() } else { format!("{} {}", l.id, l.lane) };
                l.runs.iter().map(|r| r.pid).filter(|p| *p != 0).map(move |p| (p, tag.clone()))
            }).collect();
            (timed, groups)
        };
        if timed.is_empty() { continue; }
        let (load1, outside) = crate::top::outside(&|pgid| groups.get(&pgid).cloned());
        let mut noise = x.noise.lock().unwrap();
        for id in &timed {
            noise.entry(id.clone()).or_default().add(load1, &outside);
        }
    }
}

/// What a lease's directories hold, MB ([`Exec::disk_of`]).
#[derive(Clone, Copy)]
struct DiskUse {
    at: Instant,
    /// all of them, for the panel (u64::MAX: the first walk is under way)
    mb: u64,
    /// the ones that are its own ([`Lease::dirs`]), now and at the first walk
    own_mb: u64,
    first_own_mb: Option<u64>,
}

/// What the live ones of `runs` hold now, bytes: a scoped run its scope's anonymous and shared
/// memory ([`os::scope_now`]), any other what its process group holds ([`os::groups_held`]: the
/// resident set, on macOS the footprint). None: none is live, or this look could not tell (on
/// Windows none can).
fn held_now(runs: &[Arc<Run>]) -> Option<u64> {
    let (mut held, mut groups, mut live) = (0u64, Vec::new(), false);
    for r in runs.iter().filter(|r| r.pid != 0 && r.ended() == 0) {
        live = true;
        match os::scope_now(r.pid, r.start, r.unit.as_deref(), &r.exit_file.with_extension("usage")) {
            Some(bytes) => held += bytes,
            None => groups.push((r.pid, r.start)),
        }
    }
    if !live { return None; }
    if groups.is_empty() { return Some(held); }
    os::groups_held(&groups).map(|g| held + g)
}

/// What the live ones of `runs` have done so far ([`os::activity`]). None: none is live, or one
/// of them cannot be measured.
fn activity_now(runs: &[Arc<Run>]) -> Option<os::Activity> {
    let (mut sum, mut live) = (os::Activity::default(), false);
    for r in runs.iter().filter(|r| r.pid != 0 && r.ended() == 0) {
        live = true;
        sum = sum + os::activity(r.pid, r.start, r.unit.as_deref(), &r.exit_file.with_extension("usage"), &r.hold)?;
    }
    live.then_some(sum)
}

/// What a beat's look measured of a lease: its runs' peak RAM, what they hold, and what they
/// had done ([`os::Activity`]); None where it could not.
type Look = (Option<u64>, Option<u64>, Option<os::Activity>);

/// A timed lease's quiet mark while its start can still refuse: gone with a refusal.
struct QuietMark(Option<PathBuf>);

impl Drop for QuietMark {
    fn drop(&mut self) {
        if let Some(mark) = self.0.take() {
            let _ = std::fs::remove_file(mark);
        }
    }
}

/// Since when, and from what counts, a lease's runs did nothing, after a look that found `done`
/// at `now`: still the old start while they stay below [`busy`], a new one from a look that
/// shows work, none while nothing can be measured.
fn next_quiet(quiet: Option<(Instant, os::Activity)>, done: Option<os::Activity>, now: Instant) -> Option<(Instant, os::Activity)> {
    match (done, quiet) {
        (Some(to), Some((since, from))) if !busy(from, to, now.saturating_duration_since(since)) => Some((since, from)),
        (Some(to), _) => Some((now, to)),
        (None, _) => None,
    }
}

/// The CPU, in % of one core, a lease's runs may use and still do nothing: a hung CI story test
/// whose four JIT processes poll with timeouts used 1.1% (11-04), which a 1% bound took for
/// work; a build or a test that works uses a core or more. Only the CPU rate tells: the JIT
/// processes are not dumpable, so their `/proc` io, syscall and wchan are closed to the hub.
pub const IDLE_CPU_PCT: u64 = 5;

/// Whether a lease's runs did work from `from` to `to`, `over` apart: CPU past
/// [`IDLE_CPU_PCT`] of a core (and past 0.2 s), more than 1 MB read or written, or a process
/// gone with what it had done. A hung JIT waits at 0% CPU (a CI test sat 50 min so, unseen,
/// 11-04), or polls at 1%.
fn busy(from: os::Activity, to: os::Activity, over: Duration) -> bool {
    to.cpu_us < from.cpu_us || to.io_bytes < from.io_bytes || to.cpu_us - from.cpu_us > (over.as_micros() as u64 * IDLE_CPU_PCT / 100).max(200_000) || to.io_bytes - from.io_bytes > 1 << 20
}

/// No process walks under the lease table lock. Sum RSS at the same beat; scope high-water
/// marks are combined only for overlapping runs, so sequential runs do not inflate the ask.
/// Concurrent scopes' peaks are a conservative bound when their peaks occurred separately.
fn sample_runs(runs: &[Arc<Run>]) -> Option<u64> {
    let mut events = Vec::new();
    let mut groups = Vec::new();
    let now = crate::proto::wall_ms();
    for r in runs.iter().filter(|r| r.pid != 0) {
        r.sample();
        if let Some(peak) = r.usage.lock().unwrap().peak {
            let end = r.ended();
            let end = if end > 0 { end } else { now };
            events.push((r.spawned_ms, peak as i128));
            events.push((end.max(r.spawned_ms + 1), -(peak as i128)));
        } else { groups.push((r.pid, r.start)); }
    }
    let mut rss = os::groups_rss(&groups);
    events.sort_unstable();
    let mut used = 0i128;
    for (_, delta) in events {
        used += delta;
        rss = Some(rss.unwrap_or(0).max(used.max(0) as u64));
    }
    rss
}

fn err(e: impl AsRef<str>) -> String {
    format!("{{\"err\":{}}}", json_str(e.as_ref()))
}

/// This node's login (what `devhub run` / `get` ship with over ssh).
fn login() -> String {
    os::login().filter(|u| valid_name(u)).or_else(|| caps::home_dir().file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default()
}

/// A relative path that stays inside its root: no `..`, not absolute, not empty.
/// Removes `rel`'s parent dirs under `work` that are empty, deepest first, stopping at the first
/// one that is not (`remove_dir` fails on a non-empty dir, so a `target/` or any untracked content
/// keeps its dirs) and never touching `work` itself.
fn prune_empty_parents(work: &Path, rel: &str) {
    let mut p = Path::new(rel);
    while let Some(parent) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
        let full = work.join(parent);
        if !std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_dir()) || std::fs::remove_dir(&full).is_err() {
            break;
        }
        p = parent;
    }
}

fn inside(rel: &str) -> bool {
    let p = Path::new(rel);
    !rel.is_empty() && !p.is_absolute() && p.components().all(|c| matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir))
}

/// A relative path with `/` between its parts (what the lane's side joins, whatever the node's OS).
fn slashed(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Files under `p` (itself when it is one), as paths relative to `base`, at most `max`.
fn files_under(base: &Path, p: &Path, max: usize, out: &mut Vec<(String, u64)>) -> Result<(), String> {
    let md = std::fs::symlink_metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    if md.is_dir() {
        let mut kids: Vec<PathBuf> = std::fs::read_dir(p).map_err(|e| format!("{}: {e}", p.display()))?.flatten().map(|e| e.path()).collect();
        kids.sort();
        for k in kids {
            files_under(base, &k, max, out)?;
        }
    } else if md.is_file() {
        if out.len() >= max {
            return Err(format!("more than {max} files: get a narrower path"));
        }
        out.push((slashed(p.strip_prefix(base).unwrap_or(p)), md.len()));
    }
    Ok(())
}

pub(crate) fn user_bus() -> bool {
    let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let on_path = |tool: &str| std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()));
    rt.is_some_and(|r| r.join("bus").exists()) && on_path("systemd-run") && on_path("systemctl")
}

impl Exec {
    /// The executor over `conf.work`, its ledger in `state`; leases left in the ledger by a
    /// previous hub are taken back (the watchdog ends them unless the master still beats them).
    pub fn new(machine: &str, conf: LeaseConf, state: PathBuf, opts: Opts, publish: Publish) -> Arc<Exec> {
        let guard = Guard::new(machine, Roots::of(&conf.work), conf.disk_floor_gb, conf.disk_warn_gb, conf.role.users(), publish.clone());
        let scopes = opts.scopes.unwrap_or_else(user_bus);
        if scopes { os::reset_failed_units("devhub-jit-*"); }
        let x = Arc::new_cyclic(|_this| Exec { #[cfg(feature = "checkout")] this: _this.clone(), machine: machine.to_string(), publish, dev: AtomicBool::new(true), dev_req: Default::default(), conf: std::sync::RwLock::new(conf), state, opts, scopes, leases: crate::held::Held::new("leases", BTreeMap::new()),
            starting: crate::held::Held::new("lease start", ()), cleaning: Default::default(), endings: Mutex::new(BTreeMap::new()), over_disk: Mutex::new(None), orphans: Default::default(), guard, disk_use: Default::default(), #[cfg(unix)] noise: Default::default(), user: Default::default(), restarting: AtomicBool::new(false), #[cfg(feature = "checkout")] hub_build: Default::default(), last_resident_gb: Mutex::new(0.0), trouble: Mutex::new(String::new()), master_seen: Mutex::new(Instant::now()), master_away_said: AtomicBool::new(false) });
        #[cfg(all(windows, feature = "checkout"))]
        drop_aside();
        // what a hub that stopped left in the trash
        super::disk::empty_trash_soon(&x.conf().work);
        // the overlay lanes running here keep their store refs (their ships read the tables)
        #[cfg(feature = "checkout")]
        {
            let w = Arc::downgrade(&x);
            super::relaxgit::set_live(Box::new(move || w.upgrade().map(|x| x.leases.lock().unwrap().values().filter(|l| l.src == Src::Overlay).map(|l| overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or(""))).collect()).unwrap_or_default()));
        }
        let mut back = Vec::new();
        for e in std::fs::read_dir(&x.state).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|e| e == "json") {
                if let Some(mut l) = std::fs::read_to_string(&p).ok().and_then(|t| Lease::from_ledger(&t)) {
                    for r in &mut l.runs {
                        if let Some(run) = Arc::get_mut(r) {
                            run.exit_file = exit_file(&x.state, &l.id, run.n);
                            run.hold = os::Hold::taken_back(&run.exit_file);
                        }
                        r.adopted_exit();
                        // a JIT run taken back holds the dev cache as it did under its first hub
                        #[cfg(feature = "checkout")]
                        if l.src == Src::Overlay && r.live() {
                            let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(x.conf().work.join("dev/cache.lock"));
                            if let Ok(f) = lock.and_then(|f| f.try_lock_shared().is_ok().then_some(f).ok_or_else(|| std::io::Error::other("held"))) {
                                *r.cache.lock().unwrap() = Some(f);
                            }
                        }
                    }
                    back.push(l.id.clone());
                    #[cfg(feature = "checkout")]
                    if let Some(pin) = l.pin.as_deref().filter(|_| l.src == Src::Tree).and_then(super::Pin::parse) {
                        if x.opts.tree.is_some() {
                            super::tree::hold(&l.id, &pin, &l.work);
                        }
                    }
                    x.leases.lock().unwrap().insert(l.id.clone(), l);
                }
            }
        }
        // the exit files of leases this hub no longer has
        for e in std::fs::read_dir(&x.state).into_iter().flatten().flatten() {
            let p = e.path();
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if p.extension().is_some_and(|e| e == "exit") && !back.iter().any(|b| id.split('-').next() == Some(b.as_str())) {
                let _ = std::fs::remove_file(&p);
            }
        }
        // quiet marks of leases this hub no longer has (it stopped while one ran)
        for e in std::fs::read_dir(x.quiet()).into_iter().flatten().flatten() {
            let p = e.path();
            let id = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if p.extension().is_some_and(|e| e == "cmd") && !back.contains(&id) {
                let _ = std::fs::remove_file(&p);
            }
        }
        if !back.is_empty() {
            x.emit("lease", Sev::Info, format!("lease: hub on {} took back {} lease(s) from its ledger: {}", x.machine, back.len(), back.join(" ")));
        }
        // a hub that stopped mid-lease may have left edits in an unheld work tree
        #[cfg(feature = "checkout")]
        if let Some(root) = &x.opts.tree {
            let work = x.conf().work;
            for root in std::iter::once(root.clone()).chain(super::tree::natives(&work).into_iter().map(|(lane, _)| super::tree::native_root(&work, &lane))) {
                let r = super::tree::scrub_if_idle(&root, &work.join("recovered"));
                x.report_tree(&format!("{} starting", x.machine), &r);
            }
        }
        let w = Arc::downgrade(&x);
        let tick = x.opts.tick;
        let _ = std::thread::Builder::new().name("devhub-lease-watchdog".into()).spawn(move || watchdog(w, tick));
        let w = Arc::downgrade(&x);
        let _ = std::thread::Builder::new().name("devhub-disk".into()).spawn(move || disk_loop(w));
        #[cfg(unix)]
        {
            let w = Arc::downgrade(&x);
            let _ = std::thread::Builder::new().name("devhub-timed-box".into()).spawn(move || timed_box_loop(w));
        }
        if x.opts.tree.is_some() {
            let w = Arc::downgrade(&x);
            let _ = std::thread::Builder::new().name("devhub-toolchain".into()).spawn(move || toolchain_loop(w));
        }
        x
    }

    /// A timed lease runs here: the hub's own periodic work (toolchain probes, orphan sweeps, disk
    /// walks) waits for its end, so a timed run shares its box with nothing of devhub's.
    fn timed_live(&self) -> bool {
        self.leases.lock().unwrap().values().any(|l| l.kind == Kind::Timed)
    }

    /// The folder a timed lease marks while it lives (`<id>.cmd`), next to the ledger
    /// (`~/.makepad/devhub/quiet`): other tenants of the machine (CI on a Mac: `yield_to` in its
    /// ci.toml) pause while it holds one.
    fn quiet(&self) -> PathBuf {
        self.state.parent().unwrap_or(&self.state).join("quiet")
    }

    /// The lease policy now (`lease-conf set` changes it while the hub runs).
    pub fn conf(&self) -> LeaseConf {
        self.conf.read().unwrap().clone()
    }

    /// `lease-conf` (show) or `lease-conf set k=v ...`: the master sets this node's role and
    /// budgets; written to `lease.conf` (kept across restarts) and in force at once.
    fn lease_conf(&self, rest: &str) -> String {
        let mut w = rest.split_whitespace();
        if w.next() == Some("set") {
            let pairs: Vec<(String, String)> = w.filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
            if pairs.is_empty() {
                return err("lease-conf set <key=value>...");
            }
            for (k, v) in &pairs {
                if let Err(e) = caps::settable(k, v) {
                    return err(e);
                }
            }
            let path = caps::conf_path();
            let text = caps::conf_with(&std::fs::read_to_string(&path).unwrap_or_default(), &pairs);
            let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
            let tmp = path.with_extension("conf.new");
            if let Err(e) = std::fs::write(&tmp, &text).and_then(|_| std::fs::rename(&tmp, &path)) {
                return err(format!("{}: {e}", path.display()));
            }
            let conf = LeaseConf::parse(&text, &caps::home_dir());
            self.guard.set_policy(conf.disk_floor_gb, conf.disk_warn_gb, conf.role.users());
            *self.conf.write().unwrap() = conf;
            let what: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
            self.emit("lease", Sev::Info, format!("lease: {} lease.conf set {}", self.machine, what.join(" ")));
        }
        let apply = if crate::sync::store::source_store().is_some() { "store" } else { "checkout" };
        let mut c = caps::measure(&self.machine, &self.conf(), apply);
        c.ram_resident_gb = self.resident_gb();
        c.tree = self.tree_key();
        c.deploy = self.deploy_key();
        c.to_json()
    }

    /// What the box holds outside its leases, GB ([`super::Caps::ram_resident_gb`]): the memory in
    /// use less what the leases' live runs hold, at one look ([`held_now`]). Where the OS cannot
    /// tell a run's memory (Windows), the last look while no run was live stands.
    fn resident_gb(&self) -> f64 {
        let runs: Vec<Arc<Run>> = self.leases.lock().unwrap().values().flat_map(|l| l.runs.iter().cloned()).collect();
        let live = runs.iter().any(|r| r.pid != 0 && r.ended() == 0);
        let held = held_now(&runs).unwrap_or(0);
        let mut last = self.last_resident_gb.lock().unwrap();
        if let Some(used) = crate::procinfo::in_use().filter(|_| !live || os::MEASURES_GROUPS) {
            *last = used.saturating_sub(held) as f64 / caps::GB;
        }
        *last
    }

    /// One look at the person: the WM's answer (`Err`: no WM of ours), its flips as events.
    pub fn sense_user(&self, answer: Result<&str, String>) {
        let idle_min = self.conf().user_idle_min;
        let (flip, sense) = {
            let mut t = self.user.lock().unwrap();
            let flip = t.update(crate::proto::wall_ms(), answer, idle_min);
            (flip, t.sense.clone().unwrap_or_default())
        };
        let dev = self.dev.load(Ordering::Acquire);
        let running = self.leases.lock().unwrap().values().filter(|l| l.kind.writes()).count();
        match flip {
            Some(super::user::Flip::Present) => self.emit(
                "lease",
                Sev::Info,
                format!(
                    "node_user_present: {}: a person uses it ({}): {}",
                    self.machine,
                    sense.what,
                    if !dev { "its dev switch is off anyway".to_string() } else if running > 0 { format!("no new leases, {running} running finish") } else { "no new leases".to_string() }
                ),
            ),
            Some(super::user::Flip::Idle) => self.emit(
                "lease",
                Sev::Info,
                format!("node_user_idle: {}: no input for {idle_min} min: {}", self.machine, if dev { "takes leases again" } else { "its dev switch stays off (the user's)" }),
            ),
            None => {}
        }
    }

    /// The work tree's pin (`caps.tree`; empty: no tree, or unknown).
    fn tree_key(&self) -> String {
        #[cfg(feature = "checkout")]
        if let Some(root) = &self.opts.tree {
            return super::tree::current(root).map_or(String::new(), |p| p.key());
        }
        String::new()
    }

    fn deploy_key(&self) -> String {
        #[cfg(feature = "checkout")]
        {
            super::tree::deploy(&crate::sync::SyncConf::load().checkout).map_or(String::new(), |p| p.key())
        }
        #[cfg(not(feature = "checkout"))]
        String::new()
    }

    fn emit(&self, kind: &str, sev: Sev, line: String) {
        (self.publish)(hub_record(&self.machine, "", 0, kind, sev, line, String::new()));
    }

    /// [`Exec::emit`] for a background thread.
    fn emitter(&self) -> impl Fn(&str, Sev, String) + Send + 'static {
        let (publish, machine) = (self.publish.clone(), self.machine.clone());
        move |kind: &str, sev: Sev, line: String| publish(hub_record(&machine, "", 0, kind, sev, line, String::new()))
    }

    fn save(&self, l: &Lease) {
        if let Err(e) = self.save_checked(l) { self.emit("lease", Sev::Warn, e); }
    }

    fn save_checked(&self, l: &Lease) -> Result<(), String> {
        let p = self.state.join(format!("{}.json", l.id));
        let tmp = self.state.join(format!("{}.json.new", l.id));
        std::fs::create_dir_all(&self.state)
            .and_then(|_| std::fs::write(&tmp, l.ledger()))
            .and_then(|_| std::fs::rename(&tmp, &p))
            .map_err(|e| format!("saving lease {}: {e}", l.id))
    }

    /// The disk loop's look, right after its reclaim: the next beat tells the master
    /// ([`lease_over_disk`]).
    fn look_over_disk(&self) {
        let (free, floor) = (self.guard.free(), self.guard.floor());
        let found = (free < floor).then(|| {
            let runs: Vec<_> = self.leases.lock().unwrap().values().map(|l| (l.id.clone(), l.runs.clone())).collect();
            let took: Vec<_> = runs.into_iter().map(|(id, runs)| Took { grew: self.disk_grew(&id), held: held_now(&runs).unwrap_or(0), id }).collect();
            lease_over_disk(free, floor, caps::swap_on_disk(), &took)
        }).flatten();
        *self.over_disk.lock().unwrap() = found;
    }

    /// What is in use now (for the disk guard).
    pub fn live(&self) -> Live {
        let mut leases = Vec::new();
        for l in self.leases.lock().unwrap().values() {
            for p in [&l.work, &l.target, &l.tmp] {
                leases.push((p.clone(), l.id.clone()));
            }
            if l.src == Src::Overlay {
                let dev = self.conf().work.join("dev");
                for p in [dev.join("overlay").join(overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or(""))), dev.join("git"), dev.join("run-home"), dev.join("tree")] { leases.push((p, l.id.clone())); }
            }
        }
        Live { leases, cwds: super::wt::live_cwds() }
    }

    /// One request of service `word` (the line may repeat the word); answer lines.
    pub fn handle(&self, word: &str, line: &str) -> Vec<String> {
        let rest = match line.strip_prefix(word) {
            Some(r) if r.is_empty() || r.starts_with(' ') => r.trim(),
            _ => line.trim(),
        };
        match word {
            // the hub exits for its self-update in a moment: the holder tries again on the new one
            "lease-start" | "lease-run" if self.restarting.load(Ordering::Acquire) => vec![err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine))],
            "lease-start" => vec![self.start(rest)],
            "lease-run" => vec![self.run(rest)],
            "lease-log" => vec![self.log(rest)],
            "lease-end" => vec![with_load(self.end_cmd(rest), super::caps::load_now)],
            "lease-beat" => vec![self.beat(rest)],
            "lease-jits" => vec![lease_jits_of(self, rest)],
            "lease-kill" => vec![lease_kill(self, rest)],
            "lease-why" => vec![lease_why(self, rest)],
            "overlay-status" | "overlay-why" => self.overlay_inspect(word, rest),
            "overlay-selftest" => vec![self.overlay_selftest(rest)],
            "ship-done" => {
                let id = rest.split_whitespace().next().unwrap_or("");
                self.ship_started(id);
                let answer = self.ship_done(rest);
                if let Some(l) = self.leases.lock().unwrap().get_mut(id) {
                    l.phases.close("sync", crate::proto::wall_ms());
                    self.save(l);
                }
                vec![answer]
            },
            "lease-owns" => vec![self.owns(rest)],
            "hub-restart" => vec![self.hub_restart(rest)],
            "service-restart" => vec![self.service_restart(rest)],
            "service-health" => vec![self.service_health(rest)],
            "ship-have" | "ship-put" | "ship-link" => vec![self.ship(word, rest)],
            "get-ls" | "get-read" => {
                let start = crate::proto::wall_ms();
                let result = if word == "get-ls" { self.get_ls(rest) } else { vec![self.get_read(rest)] };
                if let Some(id) = rest.split_whitespace().nth(1) {
                    if let Some(l) = self.leases.lock().unwrap().get_mut(id) { l.phases.push("report", start, crate::proto::wall_ms()); }
                }
                result
            },
            "policy" => vec![self.policy(rest)],
            "lease-conf" => vec![self.lease_conf(rest)],
            "caps" => {
                let apply = if crate::sync::store::source_store().is_some() { "store" } else { "checkout" };
                let mut c = caps::measure(&self.machine, &self.conf(), apply);
                c.ram_resident_gb = self.resident_gb();
                c.desk_link = crate::embed::desk_link_state();
                c.dev_req = match self.dev_req.load(Ordering::Acquire) { 1 => Some(true), 2 => Some(false), _ => None };
                c.tree = self.tree_key();
                c.deploy = self.deploy_key();
                #[cfg(feature = "checkout")]
                { c.applying = crate::sync::serve::applying(); }
                c.stalls = crate::held::stalls();
                c.trouble = std::mem::take(&mut *self.trouble.lock().unwrap());
                c.user = self.user.lock().unwrap().sense.clone();
                vec![c.to_json()]
            }
            "disk" => match rest.strip_prefix("clean ") {
                Some(lane) => vec![self.clean_lane(lane.trim())],
                None => {
                    let need_gb: f64 = rest.strip_prefix("need ").and_then(|n| n.trim().parse().ok()).unwrap_or(0.0);
                    self.guard.reclaim(&self.live(), rest == "dry" || rest == "--dry-run", (need_gb * (1u64 << 30) as f64) as u64)
                }
            },
            "heap" => {
                let lines = crate::heap::report(rest.trim().parse().unwrap_or(12));
                // a JIT host without a checkout has no relaxgit store
                #[cfg(feature = "checkout")]
                let lines = { let mut lines = lines; lines.insert(1, super::relaxgit::counters()); lines };
                lines
            }
            "recover-get" => vec![self.recover_get(rest)],
            "recover-del" => vec![self.recover_del(rest)],
            _ => vec![err(format!("unknown service {word}"))],
        }
    }

    fn policy(&self, rest: &str) -> String {
        match rest {
            "dev=on" => {
                self.dev.store(true, Ordering::Release);
                self.dev_req.store(0, Ordering::Release);
            }
            "dev=off" => {
                self.dev.store(false, Ordering::Release);
                self.dev_req.store(0, Ordering::Release);
            }
            "" => {}
            _ => return err("usage: policy dev=on|off"),
        }
        format!("{{\"dev\":\"{}\"}}", if self.dev.load(Ordering::Acquire) { "on" } else { "off" })
    }

    /// [`Self::starting`] with `lanes` not being cleaned: the wait for a clean holds up no other start.
    fn starting_clean(&self, lanes: &[&str]) -> crate::held::HeldGuard<'_, ()> {
        loop {
            let starting = self.starting.lock().unwrap();
            let held = |c: &BTreeSet<String>| lanes.iter().any(|l| c.contains(*l));
            let cleaning = self.cleaning.0.lock().unwrap();
            if !held(&cleaning) {
                return starting;
            }
            drop(starting);
            drop(self.cleaning.1.wait_while(cleaning, |c| held(c)).unwrap());
        }
    }

    fn start(&self, json: &str) -> String {
        let Ok(v) = makepad_strict_json::parse(json.as_bytes()) else { return err("lease-start: not JSON") };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let (id, lane) = (s("id"), s("lane"));
        if !valid_name(&id) || !valid_name(&lane) {
            return err("lease-start: id and lane are a-z, 0-9, - _ .");
        }
        let _starting = self.starting_clean(&[&lane, &s("warms")]);
        if self.restarting.load(Ordering::Acquire) { return err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine)); }
        let warms = match v.get("warms").filter(|v| !v.is_null()) {
            Some(v) => match v.as_str().filter(|lane| valid_name(lane)) {
                Some(lane) => Some(lane.to_string()),
                None => return err("lease-start: warms is a lane (a-z, 0-9, - _ .)"),
            },
            None => None,
        };
        let tree_lane = warms.as_deref().unwrap_or(&lane);
        let Some(kind) = Kind::parse(&s("kind")) else { return err(format!("lease-start: kind {}", s("kind"))) };
        let src = match s("src").as_str() { "" | "tree" => Src::Tree, "overlay" => Src::Overlay, _ => return err("lease-start: src is tree or overlay") };
        if let Some(l) = self.leases.lock().unwrap().get(&id) {
            return l.answer();
        }
        if (kind.writes() || src == Src::Overlay) && !self.dev.load(Ordering::Acquire) {
            return err(format!("dev off: {} takes no {} leases", self.machine, kind.as_str()));
        }
        let conf = self.conf();
        if kind == Kind::Ci && conf.role.users() {
            return err("lease-start: a user's box takes no ci leases");
        }
        // a timed lease runs with nothing building on its box, and the master's look at the box
        // can be seconds old: an apply's build that started 3 s before a grant ran under all of a
        // timed run (rustc at 479% of a core, 11-04). Its mark goes first: an apply that starts
        // now sees it (`sync::serve`), one that started before is seen here.
        let mut quiet = QuietMark(None);
        if kind == Kind::Timed {
            let mark = self.quiet().join(format!("{id}.cmd"));
            let _ = std::fs::create_dir_all(self.quiet());
            let _ = std::fs::write(&mark, format!("timed lease {id} of {lane}\n"));
            quiet.0 = Some(mark);
            if let Some(why) = self.builds_here() {
                return err(format!("{why}: {}", super::TIMED_AFTER));
            }
        }
        // a real lease preempts the background pre-warm at once (its processes are gone before
        // the lease takes the tree, its JIT cache consistent); a timed one whatever its source
        #[cfg(feature = "checkout")]
        if kind.writes() && (src == Src::Tree || kind == Kind::Timed) {
            crate::sync::prewarm::cancel(&format!("lease {id} ({lane})"));
        }
        let rev = v.get("rev").filter(|r| r.get("makepad").is_some());
        let rev_of = |k: &str| rev.and_then(|r| r.get(k)).and_then(|x| x.as_str()).map(str::to_string);
        // Only a tree write lease on a dev-on node moves the work tree.
        if src == Src::Tree && rev.is_some() && !kind.writes() {
            return err(format!("lease-start: an {} lease never moves the node's tree (no rev)", kind.as_str()));
        }
        let mut env: Vec<(String, String)> = Vec::new();
        if let Some(makepad_strict_json::Value::Obj(o)) = v.get("env") {
            for (k, x) in o {
                match x.as_str() {
                    Some(x) if valid_name(k) && !x.contains('\0') => env.push((k.to_string(), x.to_string())),
                    _ => return err(format!("lease-start: env {k}")),
                }
            }
        }
        #[cfg(target_os = "linux")]
        let wm_env = if v.get("needs").and_then(|n| n.get("gui")).is_some_and(|g| {
            g.as_str().is_some_and(|s| matches!(s.to_ascii_lowercase().as_str(), "1" | "yes" | "on" | "true")) || super::num(g) == Some(1.0)
        }) {
            match gui_env() {
                Ok(env) => env,
                Err(e) => return err(format!("lease-start {id}: {e}")),
            }
        } else {
            Vec::new()
        };
        let conf = self.conf();
        let w = &conf.work;
        let tmp = w.join(if src == Src::Overlay { "dev/tmp" } else { "tmp" }).join(&id);
        if kind == Kind::Ci && src == Src::Tree {
            for d in [&tmp, &w.join("ci-target")] {
                if let Err(e) = std::fs::create_dir_all(d) {
                    return err(format!("{}: {e}", d.display()));
                }
            }
        }
        let sha = rev_of("makepad");
        let (work, target, pin, what) = if src == Src::Overlay {
            let Some(pin) = sha.as_deref().and_then(|s| super::Pin::parse(&rev_of("commercial").map_or_else(|| s.to_string(), |c| format!("{s},{c}")))) else { return err("overlay: makepad rev layer: pinned commit required") };
            let lane = overlay_lane(kind, &lane, &pin.key());
            if let Some(l) = self.leases.lock().unwrap().values().find(|l| l.src == Src::Overlay && overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or("")) == lane && l.pin.as_deref() != Some(pin.key().as_str())) {
                return err(format!("overlay lane {lane} held at {} by {}", l.pin.as_deref().unwrap_or(""), l.id));
            }
            #[cfg(feature = "checkout")]
            let prepared = super::relaxgit::prepare(w, &lane, &id, &pin);
            #[cfg(not(feature = "checkout"))]
            let prepared: Result<Vec<(String, String)>, String> = Err(format!("overlay: makepad {} RELAXGIT layer: this node has no bare git store", pin.makepad));
            match prepared {
                Ok(vars) => {
                    env.retain(|(k, _)| !vars.iter().any(|(key, _)| key == k)); env.extend(vars);
                    (w.join("dev/lane").join(&lane).join("run"), w.join("dev/target"), Some(pin.key()), format!("overlay at {}", pin.short()))
                }
                Err(e) => return err(e),
            }
        } else if sha.is_some() || (kind == Kind::Ci && self.opts.tree.is_some()) {
            match self.take_tree(&id, tree_lane, kind, sha.as_deref(), rev_of("commercial").as_deref()) {
                Ok(x) => x,
                Err(e) => return err(format!("lease-start {id}: {e}")),
            }
        } else {
            (w.join("lane").join(&lane).join("src"), w.join("target").join(format!("lane-{lane}")), None, "lane dir".to_string())
        };
        let keep = tmp.join(crate::lease::KEEP_DIR);
        for d in [&keep, &work, &target] {
            if let Err(e) = std::fs::create_dir_all(d) {
                #[cfg(feature = "checkout")]
                if src == Src::Tree && pin.is_some() {
                    let r = super::tree::release(&id, &work, &w.join("recovered"), &format!("{lane}-{id}-start"));
                    self.report_tree(&format!("{id} failed to start"), &r);
                }
                return err(if src == Src::Overlay { format!("overlay: makepad {} run directory layer: {}: {e}", sha.as_deref().unwrap_or(""), d.display()) } else { format!("{}: {e}", d.display()) });
            }
        }
        let law = [
            ("CARGO_TARGET_DIR", target.to_string_lossy().into_owned()),
            ("TMPDIR", tmp.to_string_lossy().into_owned()),
            // a failed step's logs go here: its holder keeps them before the tmp goes (KEEP_DIR)
            ("DEVHUB_KEEP", keep.to_string_lossy().into_owned()),
            ("CARGO_INCREMENTAL", "0".into()),
            ("MAKEPAD_HIDE_WINDOWS", "1".into()),
            ("SANDBOX_MUTE", "1".into()),
            ("MAKEPAD_LAN", "0".into()),
            // every run is the lane's test instance: badged `TEST: lane <lane>` in its caption,
            // never the WM dock's app, off the LAN (XR, printer, rooms) unless it asks
            // (MAKEPAD_LAN=1 in its own command)
            ("MAKEPAD_TEST_LANE", lane.clone()),
            // what a run prints as its verdict names its own lease (a lane that reads a gate's
            // NODEGATE-OK from a file shared with another lane's run must tell them apart)
            ("DEVHUB_LEASE", id.clone()),
            // one JIT session per lease: its many check/test/present calls share one JIT setup
            ("MAKEPAD_JIT_SESSION", id.clone()),
            // app tests keep their artifacts and app home per lease: two jobs of one lane on one
            // box share its overlay and target, and a fixed per-test dir let one wipe the other's
            ("MAKEPAD_TEST_ARTIFACTS", target.join("makepad_test").join(&id).to_string_lossy().into_owned()),
            ("CARGO_NET_OFFLINE", "true".into()),
        ];
        #[cfg(target_os = "linux")]
        let law: Vec<_> = law.into_iter().chain(wm_env).collect();
        // the lease's own environment wins over what was asked
        env.retain(|(k, _)| !law.iter().any(|(l, _)| l == k));
        env.extend(law.iter().map(|(k, v)| (k.to_string(), v.clone())));
        let cost = lease_cost(&v, kind);
        let live = live_of(&v);
        for (key, value) in job_env(kind, cost, live) {
            if !env.iter().any(|(k, _)| k == key) { env.push((key.into(), value.to_string())); }
        }
        let loader = v.get("loader").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string);
        let l = Lease {
            #[cfg(feature = "checkout")]
            identity: Arc::default(),
            #[cfg(feature = "checkout")]
            resolution: None,
            #[cfg(feature = "checkout")]
            selftest: None,
            id: id.clone(), lane: lane.clone(), warms, kind, src, loader, cost, ram_max_gb: lease_ram_max(&v), live, first_run: String::new(), shipped: false, phases: Default::default(), work: work.clone(), target, tmp, env, pin, overlay: Vec::new(), source: String::new(), fresh_put: Vec::new(), fresh_gone: Vec::new(), runs: Vec::new(), peak_ram_bytes: None, held_bytes: None, beat: Instant::now(), quiet: None
        };
        let a = l.answer();
        // A lease-end that came before it was in the table (its holder gave it up as the master
        // granted it) found no lease here and left its receipt: the start ends it at once, taking
        // down what it set up. Taken instead, it ran on with no master beating it until its beat
        // expired (11-04). An end takes the endings first, then the leases, as here.
        let endings = self.endings.lock().unwrap();
        if self.ended_here(&endings, &id) {
            drop(endings);
            self.end(l, "released");
            return err(format!("lease-start {id}: it ended on {} as it started", self.machine));
        }
        self.save(&l);
        // (the lease's now: its end takes the mark)
        quiet.0 = None;
        self.leases.lock().unwrap().insert(id.clone(), l);
        drop(endings);
        self.guard.touch();
        self.emit("ws", Sev::Info, format!("ws_create: {id} {} for {lane} on {}: {what} {}", kind.as_str(), self.machine, work.display()));
        a
    }

    /// Whether a lease-end for `id` came here (its cleanup runs or its receipt is saved): a lease
    /// id is never granted twice, so it does not start after that.
    fn ended_here(&self, endings: &BTreeMap<String, String>, id: &str) -> bool {
        endings.contains_key(id) || self.state.join(format!("{id}.ended")).exists()
    }

    /// Takes the work tree at its pin: (work, target, pin, what for the event).
    #[cfg(feature = "checkout")]
    fn take_tree(&self, id: &str, tree_lane: &str, kind: Kind, sha: Option<&str>, commercial: Option<&str>) -> Result<(PathBuf, PathBuf, Option<String>, String), String> {
        let shared = self.opts.tree.as_ref().ok_or("this node runs no tree for rev leases")?;
        let conf = self.conf();
        let native = super::native_lane(tree_lane, kind, conf.role);
        let root = if native { super::tree::native_root(&conf.work, tree_lane) } else { shared.clone() };
        let deploy = crate::sync::SyncConf::load().checkout;
        let mut pin = match sha {
            Some(sha) => super::Pin::parse(&commercial.map_or_else(|| sha.to_string(), |c| format!("{sha},{c}"))).ok_or("rev: a 40-hex commit")?,
            None => super::tree::shared(&deploy).or_else(|| super::tree::current(&root)).ok_or("sync this node before its first CI lease")?,
        };
        // Shared trees keep their commercial half; a native lane has exactly its requested pin.
        if !native && pin.commercial.is_none() {
            let held = |c: &String| crate::sync::git::Store::open("commercial").is_ok_and(|s| s.has(c));
            pin.commercial = super::tree::current(&root).and_then(|p| p.commercial).or_else(|| super::tree::shared(&deploy).and_then(|p| p.commercial)).filter(held);
        }
        let t = if native {
            super::tree::take_native(id, &pin, &conf.work, tree_lane, conf.native_trees as usize, self.guard.floor())?
        } else {
            super::tree::take(id, &pin, &root, &conf.work.join("recovered"))?
        };
        self.report_tree(&format!("{id} on {}, before its start", self.machine), &t.cleared);
        let moved: Vec<String> = t.moved.iter().map(|(r, n)| format!("{r} {n} files")).collect();
        let what = format!("{} tree at {} ({})", if native { "native" } else { "work" }, pin.short(), if t.moved.iter().all(|(_, n)| *n == 0) { "there already".to_string() } else { format!("moved: {}", moved.join(", ")) });
        let target = if native { super::tree::native_target(&conf.work, tree_lane) } else { conf.work.join("ci-target") };
        Ok((t.root, target, Some(pin.key()), what))
    }

    #[cfg(not(feature = "checkout"))]
    fn take_tree(&self, _id: &str, _tree_lane: &str, _kind: Kind, _sha: Option<&str>, _commercial: Option<&str>) -> Result<(PathBuf, PathBuf, Option<String>, String), String> {
        Err("this host has no git stores: leases with a rev need a desktop node".into())
    }

    fn run(&self, rest: &str) -> String {
        // `lease-run <id> <cwd> <base64 argv> [{"live":true}]`
        let mut w = rest.splitn(4, ' ');
        let (id, cwd, b64) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or("").trim());
        let extra = w.next().unwrap_or("").trim();
        let options = if extra.is_empty() { None } else {
            let Ok(v) = makepad_strict_json::parse(extra.as_bytes()) else { return err("lease-run: options are not JSON") };
            Some(v)
        };
        let detachable = options.as_ref().and_then(|v| v.get("live")).and_then(|v| v.as_bool()).unwrap_or(false);
        let request_id = options.as_ref().and_then(|v| v.get("request_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let Some(raw) = b64_decode(b64) else { return err("lease-run: argv is not base64") };
        let argv: Vec<String> = raw.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        let argv: Vec<String> = match argv.iter().rposition(|a| !a.is_empty()) {
            Some(last) => argv[..=last].to_vec(),
            None => return err("lease-run: empty argv"),
        };
        if argv[0].is_empty() {
            return err("lease-run: empty program");
        }
        let rel = Path::new(cwd);
        if cwd.is_empty() || rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return err("lease-run: cwd is `.` or a path inside the lease's work dir");
        }
        let mut leases = self.leases.lock().unwrap();
        if self.restarting.load(Ordering::Acquire) { return err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine)); }
        let Some(l) = leases.get_mut(id) else { return err(format!("no lease {id} on {}", self.machine)) };
        if let Some(r) = l.runs.iter().find(|r| !request_id.is_empty() && r.request == request_id) {
            self.save(l);
            return format!("{{\"run\":{},\"n\":{}}}", r.pid, r.n);
        }
        #[cfg(feature = "checkout")]
        if let Some(r) = &mut l.resolution {
            if (!request_id.is_empty() && r.request == request_id) || (request_id.is_empty() && r.request.is_empty() && r.cwd == cwd && r.argv == argv) { r.reported |= r.answer.is_some(); return r.answer(); }
            if r.answer.is_none() { return err(format!("overlay: lease {id} is resolving another cwd or argv")); }
        }
        #[cfg(feature = "checkout")]
        let request = argv.clone();
        let mut run_env = Vec::new();
        let mut unset = Vec::new();
        // `env [-u K] K=V cmd` is the caller's way to set the run environment: set it on the process, never
        // run an `env` program (Windows has none)
        let cmd = super::loaders::command_of(&argv);
        if cmd.is_empty() { return err("lease-run: env needs a command"); }
        let mut prefix = argv[..argv.len() - cmd.len()].iter().skip(1);
        while let Some(w) = prefix.next() {
            if w == "-u" { unset.extend(prefix.next().cloned()); }
            else if let Some((k, v)) = w.split_once('=') { run_env.push((k.to_string(), v.to_string())); }
        }
        let argv = super::loaders::cargo_form(cmd.to_vec());
        if l.first_run.is_empty() { l.first_run = super::run_class(&l.lane, l.kind, &argv); }
        if l.src == Src::Overlay {
            #[cfg(not(feature = "checkout"))]
            { return err("overlay: this node has no bare git store"); }
            #[cfg(feature = "checkout")]
            {
                if !l.shipped { return err("overlay: ship-done must define the source set before a run"); }
                let Some(args) = super::loaders::cargo_run_args(&argv).map(|a| a.to_vec()) else { return err("overlay: run requires the JIT cargo invocation") };
                if run_env.iter().map(|(k, _)| k).chain(&unset).any(|k| matches!(k.as_str(), "MAKEPAD_RUN_VFS" | "MAKEPAD_RUN_WORKSPACE" | "MAKEPAD_RUN_EDITS" | "MAKEPAD_RUN_HOME" | "CARGO_TARGET_DIR")) { return err("overlay: source environment belongs to the lease"); }
                let (work, lane, lease, pin, requested, identity) = (self.conf().work, overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or("")), l.id.clone(), l.pin.clone(), l.loader.clone(), l.identity.clone());
                let Some(edits) = l.edits() else { return err("overlay: the lease has no edits folder") };
                // A store build is the box's, whichever lease asked first: every core (a test lease's
                // one core built a dev JIT at -j 1 for 8m47s while it and its box waited, 11-03). A
                // warm leg reserves little and builds at nice 19: idle CPU only (Unix).
                let niced = cfg!(unix) && l.lane.starts_with(super::WARM_LANE_PREFIX);
                let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
                let Some(this) = self.this.upgrade() else { return err("overlay: hub stopped"); };
                let cwd = cwd.to_string();
                l.phases.begin("jit", crate::proto::wall_ms());
                l.resolution = Some(OverlayResolution { request: request_id.clone(), cwd: cwd.clone(), argv: request, pending: "selecting".into(), answer: None, reported: false, run: None });
                self.save(l);
                // Cache, per-id and build-tree locks (and Cargo's wait) stay on the worker,
                // never on a request thread or under the lease table. Its build can outlive l.
                let worker = std::thread::Builder::new().name("devhub-overlay".into()).spawn(move || {
                    let resolved = (|| -> Result<_, String> {
                        let active = || if identity.cancelled.load(Ordering::Acquire) { Err(format!("overlay: lease {lease} ended while resolving its JIT")) } else { Ok(()) };
                        let cache = disk::dev_cache_hold(&work, &active)?;
                        let pin = pin.as_deref().and_then(super::Pin::parse).ok_or("overlay: missing pin")?;
                        let notices = std::cell::RefCell::new(Vec::new());
                        let say = |line: &str| notices.borrow_mut().push(line.to_string());
                        let pending = |line: &str| {
                            let mut leases = this.leases.lock().unwrap();
                            if let Some(r) = leases.get_mut(&lease).filter(|l| Arc::ptr_eq(&l.identity, &identity)).and_then(|l| l.resolution.as_mut()) {
                                if r.pending != line { r.pending = line.to_string(); }
                            }
                        };
                        let run = |id: &str, cmd: &std::process::Command, active: &dyn Fn() -> Result<(), String>| this.overlay_build(id, cmd, active);
                        let warn = |line: &str| this.emit("lease", Sev::Warn, line.to_string());
                        let build = super::loaders::OverlayBuild { jobs: cores, niced, run: &run, cancelled: &identity.cancelled, say: &say, pending: &pending, warn: &warn, active: &active };
                        let selected = super::loaders::find_overlay(&work, &lane, &edits, &lease, &pin, requested.as_deref(), &build)?;
                        say(&format!("devhub: the JIT is {} {}", if selected.changed.is_empty() { "this box's dev binary" } else { "this lane's own build" }, selected.id));
                        let mut argv = vec![selected.binary.display().to_string()];
                        argv.extend(args);
                        Ok(RunCommand { argv, detachable, loader: Some(selected.binary), cache: Some(cache), notices: notices.into_inner() })
                    })();
                    let mut leases = this.leases.lock().unwrap();
                    if !leases.get(&lease).is_some_and(|l| Arc::ptr_eq(&l.identity, &identity)) {
                        drop(leases);
                        // End may have released the store hold before selection created it.
                        // Never release a replacement lease's holds, or holds of leaked processes.
                        if identity.release_jits.load(Ordering::Acquire) {
                            let _starting = this.starting.lock().unwrap();
                            if !this.leases.lock().unwrap().contains_key(&lease) {
                                if let Err(e) = super::jitstore::Store::new(&work).release(&lease) { this.emit("lease", Sev::Warn, format!("JIT store release {lease}: {e}")); }
                            }
                        }
                        return;
                    }
                    let l = leases.get_mut(&lease).unwrap();
                    l.phases.close("jit", crate::proto::wall_ms());
                    let result = resolved.and_then(|cmd| this.start_run(l, Path::new(&cwd), cmd, &run_env, &unset, &request_id));
                    let r = l.resolution.as_mut().unwrap();
                    r.answer = Some(match result {
                        Ok((pid, n)) => { r.run = Some(n); format!("{{\"run\":{pid},\"n\":{n}}}") }
                        Err(e) => err(e),
                    });
                    this.save(l);
                });
                if let Err(e) = worker {
                    l.phases.close("jit", crate::proto::wall_ms());
                    self.save(l);
                    l.resolution.as_mut().unwrap().answer = Some(err(format!("overlay: resolution thread: {e}")));
                }
                return l.resolution.as_ref().unwrap().answer();
            }
        }
        // a gate (or a JIT run) in the work tree whose box is building the JIT of exactly its
        // commits right now (the sync's prebuild, kept for this tree once built) waits for that
        // build: never a second build of the same JIT beside it (the fast gate paid 1.5-3 min;
        // the hot bucket runs the gate script under a test lease)
        #[cfg(feature = "checkout")]
        if l.src == Src::Tree && (l.kind == Kind::Gate || super::loaders::cargo_run_args(&argv).is_some() || argv.iter().any(|a| a.ends_with("land_gate.sh"))) {
            if let Some(pin) = l.pin.as_deref().and_then(super::Pin::parse) {
                if crate::sync::serve::building_jit(&pin.makepad, pin.commercial.as_deref()) {
                    let deployed = crate::sync::SyncConf::load().checkout.join("target");
                    let kept = [deployed, self.conf().work.join("ci-target"), l.target.clone()].iter().any(|t| super::loaders::find(&l.work, t).is_some());
                    if !kept { return "{\"pending\":\"this box's sync builds the JIT of these commits: the run waits for it, not a second build\"}".into(); }
                }
            }
        }
        // a run never starts without its shipped files: it fails here and names them
        if let Some(why) = l.unshipped() { return err(format!("lease-run {id}: {why} on {}; the run did not start", self.machine)); }
        l.fresh_put.clear();
        l.fresh_gone.clear();
        let cmd = RunCommand { argv, detachable, loader: None, cache: None, notices: Vec::new() };
        match self.start_run(l, rel, cmd, &run_env, &unset, &request_id) {
            Ok((pid, n)) => format!("{{\"run\":{pid},\"n\":{n}}}"),
            Err(e) => err(e),
        }
    }

    fn start_run(&self, l: &mut Lease, rel: &Path, command: RunCommand, run_env: &[(String, String)], unset: &[String], request: &str) -> Result<(u32, u32), String> {
        let RunCommand { argv, detachable, loader: overlay_loader, cache, notices } = command;
        let id = l.id.clone();
        let n = l.runs.len() as u32 + 1;
        let log = l.tmp.join(format!("run-{n}.log"));
        let unit = self.scopes.then(|| format!("devhub-lease-{id}-{n}"));
        let conf = self.conf();
        let cpus = if l.kind == Kind::Timed { &conf.cpus_timed } else { &conf.cpus_shared };
        // a lease in the work tree: the JIT binary of exactly what the tree holds, when this box
        // built it before (the deployment's build first: one JIT per box per loader id), instead
        // of cargo building it again (lease/loaders.rs)
        let tree = (l.src == Src::Tree && l.pin.is_some()).then(|| (l.work.clone(), l.target.clone()));
        #[cfg(feature = "checkout")]
        let deployed = Some(crate::sync::SyncConf::load().checkout.join("target"));
        #[cfg(not(feature = "checkout"))]
        let deployed: Option<PathBuf> = None;
        let loader = tree.as_ref().and_then(|(r, t)| deployed.as_ref().and_then(|d| super::loaders::find(r, d)).or_else(|| super::loaders::find(r, &conf.work.join("ci-target"))).or_else(|| super::loaders::find(r, t)));
        #[cfg(feature = "checkout")]
        let mut own = None;
        #[cfg(feature = "checkout")]
        let loader = if loader.is_none() && l.work == super::tree::native_root(&conf.work, l.tree_lane()) && super::loaders::cargo_run_args(&argv).is_some() {
            let asked = std::time::SystemTime::now();
            if let Some(key) = super::loaders::tree_id(&l.work) {
                let store = super::jitstore::Store::new(&conf.work);
                store.hold(&key, &l.id)?;
                let hit = store.lookup(&key).map(|p| (key.clone(), p));
                if hit.is_none() { own = Some(super::loaders::TreeBuild { store, key, asked }); }
                hit
            } else { None }
        } else { loader };
        #[cfg(not(feature = "checkout"))]
        let own: Option<super::loaders::TreeBuild> = None;
        let mut out = match std::fs::File::create(&log) {
            Ok(f) => f,
            Err(e) => return Err(format!("{}: {e}", log.display())),
        };
        for line in notices {
            use std::io::Write as _;
            let _ = writeln!(out, "{line}");
        }
        let argv = if l.src == Src::Overlay {
            argv
        } else { match loader.as_ref().and_then(|(k, p)| super::loaders::cargo_run_args(&argv).map(|a| (k, p, a))) {
            Some((k, p, args)) => {
                use std::io::Write as _;
                let _ = writeln!(out, "devhub: the JIT is this box's binary {k}, no cargo");
                let mut v = vec![p.display().to_string()];
                v.extend_from_slice(args);
                v
            }
            None => {
                // cargo builds this tree's JIT with the exact flags of the one build: kept at the run's end
                if let (Some((_, target)), Some(_)) = (&tree, super::loaders::cargo_run_args(&argv)) {
                    let _ = std::fs::write(target.join(super::loaders::MARK), "");
                }
                argv
            }
        } };
        let jit = overlay_loader.as_deref().or_else(|| loader.as_ref().map(|(_, p)| p.as_path()));
        let argv = match jit.filter(|_| l.kind == Kind::Timed && cfg!(target_os = "macos")) {
            Some(jit) => warmed_up(argv, jit, &exit_file(&self.state, &id, n).with_extension("warmup")),
            None => argv,
        };
        let argv = os::metered_argv(argv, unit.as_deref(), &exit_file(&self.state, &id, n).with_extension("usage"));
        let full = os::lease_argv(argv, unit.as_deref(), l.kind.nice(), cpus, l.kind.shared().then_some(l.cost), l.ram_max_gb);
        let err_out = match out.try_clone() {
            Ok(f) => f,
            Err(e) => return Err(e.to_string()),
        };
        let mut cmd = std::process::Command::new(&full[0]);
        cmd.args(&full[1..]).current_dir(if l.src == Src::Overlay { l.work.clone() } else { l.work.join(rel) }).stdin(std::process::Stdio::null()).stdout(out).stderr(err_out);
        self.run_env(&mut cmd, l, run_env, unset, overlay_loader.as_deref().or_else(|| loader.as_ref().map(|(_, p)| p.as_path())));
        let (mut child, hold) = match os::lease_spawn(&mut cmd, Some(&exit_file(&self.state, &id, n))) {
            Ok(c) => c,
            Err(e) => return Err(format!("lease-run {id}: {}: {e}", full[0])),
        };
        let pid = child.id();
        let run = Arc::new(Run { request: request.to_string(), started_ms: crate::proto::wall_ms(), spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit, log, exit: Mutex::new(None), ours: true, detachable, hold, exit_file: exit_file(&self.state, &id, n), cache: Mutex::new(cache), usage: Mutex::new(Usage::default()) });
        l.runs.push(run.clone());
        // it runs whatever the ledger says, so its holder hears that it runs (a refusal here left
        // it running unseen: 0 GB free, 11-04); the lease's next save writes it down
        self.save(l);
        let _ = std::thread::Builder::new().name("devhub-lease-wait".into()).spawn(move || {
            let code = match os::wait_usage(&mut child) {
                Ok((status, cpu, maxrss)) => { run.reaped(cpu, maxrss); os::exit_code(status) }
                Err(_) => -1,
            };
            run.ended_ms.store(crate::proto::wall_ms(), Ordering::Release);
            drop(run.cache.lock().unwrap().take());
            // what this run built, kept by content while the lease still holds its tree: said in its log
            if let Some((r, t)) = &tree {
                let said = match super::loaders::publish(r, t, own.as_ref()) {
                    Ok(Some(k)) => Some(format!("devhub: kept the JIT binary {k}")),
                    Ok(None) => None,
                    Err(e) => Some(format!("devhub: this JIT binary is not kept: {e}")),
                };
                if let Some(line) = said {
                    use std::io::Write as _;
                    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&run.log) {
                        let _ = writeln!(f, "{line}");
                    }
                }
            }
            // beside its log too: a hub that restarts before the holder read it still says it
            run.finished(code);
        });
        Ok((pid, n))
    }

    /// Lease-owned processes inherit only their lease's environment.
    fn run_env(&self, cmd: &mut std::process::Command, l: &Lease, env: &[(String, String)], unset: &[String], loader: Option<&Path>) {
        // A clean base, then the lease's own environment (including its selected WM for gui=1).
        cmd.env_clear();
        for (k, v) in base_env(l.kind, std::env::vars()) { cmd.env(k, v); }
        // Also covers leases taken over from a ledger written before these defaults existed.
        for (k, v) in job_env(l.kind, l.cost, l.live) { cmd.env(k, v.to_string()); }
        for (k, v) in &l.env { cmd.env(k, v); }
        for (k, v) in env { cmd.env(k, v); }
        for k in unset { cmd.env_remove(k); }
        #[cfg(feature = "checkout")]
        if l.src == Src::Overlay || l.tmp.join("overlay-selftest.json").is_file() { cmd.env("MAKEPAD_DEVHUB_GIT", crate::sync::git::stores_dir()); }
        // what it starts carries this, in whatever session: the lease's end finds it by this
        cmd.env(LEASE_ENV, lease_mark(&self.state, &l.id));
        if let Some(outer) = std::env::var_os(LEASE_OUTER_ENV).or_else(|| std::env::var_os(LEASE_ENV)) {
            cmd.env(LEASE_OUTER_ENV, outer);
        }
        if let Some(p) = loader { cmd.env("MAKEPAD_LOADER", p); }
        // its JITs announce where this hub looks, whatever the lease's TMPDIR
        cmd.env("MAKEPAD_REMOTE_REGISTRY", makepad_remote::discovery::registry_dir());
        Self::toolchain_env(cmd);
    }

    fn toolchain_env(cmd: &mut std::process::Command) {
        // the user's rustup toolchain before any system one: the hub may itself run under another
        // cargo (its CARGO and PATH would pick that toolchain for every run)
        if let Some(p) = super::caps::rust_path() {
            cmd.env("PATH", p).env_remove("CARGO").env_remove("RUSTC");
        }
    }

    /// Store-owned Cargo has no lease mark, run entry or lease scratch. The per-id lock
    /// and cache hold stay with the worker until it reaps Cargo and publishes the binary.
    /// A build an earlier hub started is joined through its recorded pid before starting Cargo.
    #[cfg(feature = "checkout")]
    fn overlay_build(&self, id: &str, build: &std::process::Command, active: &dyn Fn() -> Result<(), String>) -> Result<(std::process::ExitStatus, PathBuf), String> {
        active()?;
        let conf = self.conf();
        let tmp = conf.work.join("dev/jit/build").join(id);
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let pid_file = tmp.join("cargo.pid");
        join_recorded_build(&pid_file, active, &|pid| self.emit("lease", Sev::Info, format!("dev JIT {id}: joining the build pid {pid} an earlier hub started")))?;
        let log = tmp.join("cargo.log");
        let unit = self.scopes.then(|| format!("devhub-jit-{id}"));
        let argv = std::iter::once(build.get_program()).chain(build.get_args()).map(|s| s.to_string_lossy().into_owned()).collect();
        let full = os::lease_argv(argv, unit.as_deref(), Kind::Build.nice(), &conf.cpus_shared, None, None);
        let out = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
        let mut cmd = std::process::Command::new(&full[0]);
        cmd.args(&full[1..]).current_dir(build.get_current_dir().ok_or("dev JIT build has no cwd")?)
            .stdin(std::process::Stdio::null()).stdout(out.try_clone().map_err(|e| e.to_string())?).stderr(out)
            .env_clear().envs(base_env(Kind::Build, std::env::vars())).env("TMPDIR", &tmp).env("TMP", &tmp).env("TEMP", &tmp);
        Self::toolchain_env(&mut cmd);
        for (k, v) in build.get_envs() {
            match v { Some(v) => { cmd.env(k, v); }, None => { cmd.env_remove(k); } }
        }
        if let Some(unit) = &unit { os::reset_failed_units(&format!("{unit}.scope")); }
        active()?;
        let (child, hold) = os::lease_spawn(&mut cmd, Some(&tmp.join("cargo.exit"))).map_err(|e| format!("dev JIT build: {e}"))?;
        let pid = child.id();
        let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n: 0, pid, start: os::proc_start(pid).unwrap_or(0), unit, log: log.clone(), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: tmp.join("cargo.exit"), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) });
        let mut process = WorkerRun { child, run, pid_file: Some(pid_file.clone()) };
        std::fs::write(&pid_file, format!("{pid} {}\n", process.run.start)).map_err(|e| format!("{}: {e}", pid_file.display()))?;
        loop {
            if let Some(status) = process.child.try_wait().map_err(|e| e.to_string())? {
                process.run.finished(os::exit_code(status));
                return Ok((status, log));
            }
            if let Err(e) = active() {
                self.emit("lease", Sev::Info, format!("dev JIT {id}: {e}; stopping store build pid {pid}"));
                return Err(e); // WorkerRun stops and reaps only this store-owned process group.
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Lease ownership for the selftest's fixture runs (they continue an admitted run).
    #[cfg(feature = "checkout")]
    fn worker_process(&self, id: &str, identity: &Arc<LeaseIdentity>, tmp: &Path, build: &std::process::Command, env: &[(String, String)], unset: &[String]) -> Result<WorkerRun, String> {
        let mut leases = self.leases.lock().unwrap();
        let l = leases.get_mut(id).filter(|l| Arc::ptr_eq(&l.identity, identity)).ok_or_else(|| format!("overlay: lease {id} ended before worker process"))?;
        if self.restarting.load(Ordering::Acquire) { return Err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine)); }
        let n = l.runs.len() as u32 + 1;
        let log = tmp.join(format!("run-{n}.log"));
        let unit = self.scopes.then(|| format!("devhub-lease-{id}-{n}"));
        let conf = self.conf();
        let cpus = if l.kind == Kind::Timed { &conf.cpus_timed } else { &conf.cpus_shared };
        let argv = std::iter::once(build.get_program()).chain(build.get_args()).map(|s| s.to_string_lossy().into_owned()).collect();
        let argv = os::metered_argv(argv, unit.as_deref(), &exit_file(&self.state, &id, n).with_extension("usage"));
        let full = os::lease_argv(argv, unit.as_deref(), l.kind.nice(), cpus, l.kind.shared().then_some(l.cost), l.ram_max_gb);
        let out = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
        let mut cmd = std::process::Command::new(&full[0]);
        cmd.args(&full[1..]).current_dir(build.get_current_dir().ok_or("lease worker process has no cwd")?)
            .stdin(std::process::Stdio::null()).stdout(out.try_clone().map_err(|e| e.to_string())?).stderr(out);
        self.run_env(&mut cmd, l, env, unset, None);
        for (k, v) in build.get_envs() {
            match v { Some(v) => { cmd.env(k, v); }, None => { cmd.env_remove(k); } }
        }
        let (child, hold) = os::lease_spawn(&mut cmd, Some(&exit_file(&self.state, id, n))).map_err(|e| format!("lease worker process: {e}"))?;
        let pid = child.id();
        let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit, log: log.clone(), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: exit_file(&self.state, id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) });
        l.runs.push(run.clone());
        self.save(l);
        drop(leases);
        Ok(WorkerRun { child, run, pid_file: None })
    }

    /// `hub-restart`: the self-update, at once. A restart ends nothing: every run lives in its
    /// own session (scope) and its wrapper writes its exit code beside the ledger as it ends
    /// ([`os::metered_argv`]); leases, their runs and answered overlay resolutions are in the
    /// ledger, and the new hub takes them back (a holder rides through on `restarting:` and
    /// connection errors). A resolution still selecting or building is asked again by its
    /// holder's next poll; its store build goes on and the new one waits on it.
    /// Short refusals (`waits:`) remain: a sync apply and an in-hub selftest. Windows is no
    /// exception: a run's job outlives the hub too ([`os::lease_spawn`]).
    /// The new hub is built first, while this one runs on ([`Exec::hub_built`]): `waits:` for
    /// the build, and a deployment it does not build at is refused (`does not build`), this hub
    /// running on. Then this hub exits a moment after answering, starting no run before, and what
    /// keeps it (the WM's supervisor, the Mac's LaunchAgent, Windows' start script) starts it
    /// again through `cargo run` in the deployment tree: the new code, compiled already.
    /// `at`: the deployment commit the master judged; a newer apply waits for its next look.
    fn hub_restart(&self, at: &str) -> String {
        // Finish saving an arriving lease before exiting; it must survive in the ledger too.
        let _starting = self.starting.lock().unwrap();
        let leases = self.leases.lock().unwrap();
        #[cfg(not(feature = "checkout"))]
        let _ = at;
        #[cfg(feature = "checkout")]
        let (root, now) = {
            let root = crate::sync::SyncConf::load().checkout;
            let now = super::tree::deploy(&root).map(|p| p.makepad).unwrap_or_default();
            if !at.is_empty() && now != at {
                return err(format!("waits: the deployment of {} moved to {} (the master looked at {at})", self.machine, &now[..now.len().min(10)]));
            }
            (root, now)
        };
        if let Some(why) = self.restart_blocker(&leases) {
            return err(format!("waits: {why}"));
        }
        // 11-02: a landing that broke makepad-devhub took 8 hubs down for 15 min: each exited
        // into a `cargo run` that failed, and its keeper retried the same tree. Whether a timed
        // lease runs is read from the leases held here: asked of `self` (its own lock again), it
        // hung this thread with the lease start and the leases locked (lease starts and ends on a
        // bench box sat for 25 min, 11-03).
        #[cfg(feature = "checkout")]
        let timed = leases.values().any(|l| l.kind == Kind::Timed);
        #[cfg(feature = "checkout")]
        match self.hub_built(&root, &now, timed) {
            Some(Ok(())) => {}
            Some(Err(why)) => return err(format!("the new hub does not build at {}: {why}; {} keeps running its hub at {}", &now[..now.len().min(10)], self.machine, &super::caps::hub_rev()[..super::caps::hub_rev().len().min(10)])),
            None if timed => return err(format!("waits: a timed lease runs on {}: its new hub at {} builds after it", self.machine, &now[..now.len().min(10)])),
            None => return err(format!("waits: {}'s new hub builds at {} first", self.machine, &now[..now.len().min(10)])),
        }
        self.restarting.store(true, Ordering::Release);
        drop(leases);
        self.emit("hub", Sev::Info, format!("hub: {} restarts for the self-update (was at {})", self.machine, &super::caps::hub_rev()[..super::caps::hub_rev().len().min(10)]));
        let _ = std::thread::Builder::new().name("devhub-hub-restart".into()).spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            std::process::exit(0);
        });
        // the build it restarts for, measured: the master's ledger keeps it (this hub's memory
        // of it ends with it)
        let built = HUB_BUILD.lock().unwrap_or_else(|e| e.into_inner()).1;
        format!("{{\"restarting\":{}{}}}", json_str(&self.machine), built.map_or(String::new(), |b| format!(",\"built\":{}", b.to_json())))
    }

    /// What builds on this box now, if anything: a timed lease starts after it ([`Self::lease_start`]).
    #[cfg(feature = "checkout")]
    fn builds_here(&self) -> Option<String> {
        if crate::sync::serve::applying() {
            return Some(format!("the deployment of {} moves and builds", self.machine));
        }
        matches!(*self.hub_build.lock().unwrap(), HubBuild::Running { .. }).then(|| format!("{}'s new hub builds", self.machine))
    }

    #[cfg(not(feature = "checkout"))]
    fn builds_here(&self) -> Option<String> {
        None
    }

    /// What a restart of this hub would lose now ([`Exec::hub_restart`]), if anything.
    fn restart_blocker(&self, leases: &BTreeMap<String, Lease>) -> Option<String> {
        // an in-hub selftest summary (pid 0) has no process to take back
        let selftest: Vec<&str> = leases.values().filter(|l| l.runs.iter().any(|r| r.pid == 0 && r.live())).map(|l| l.id.as_str()).collect();
        if !selftest.is_empty() {
            return Some(format!("the overlay selftest of {} runs in {}'s hub", selftest.join(" "), self.machine));
        }
        // (Windows) a run's wrapper holds its job open from its first moments, before it writes
        // the pid of what it started: a hub gone before that left the job no name to be taken
        // back by (a name goes with its last handle)
        if cfg!(windows) {
            let starting: Vec<&str> = leases.values().filter(|l| l.runs.iter().any(|r| r.ours && r.pid != 0 && r.live() && !r.exit_file.with_extension("usage.pid").exists())).map(|l| l.id.as_str()).collect();
            if !starting.is_empty() {
                return Some(format!("a run of {} is starting on {}", starting.join(" "), self.machine));
            }
        }
        #[cfg(feature = "checkout")]
        if crate::sync::serve::applying() {
            return Some(format!("a sync runs on {} (the hub restarts after it)", self.machine));
        }
        None
    }

    /// Whether the hub builds at deployment `at` in `root`: its build, started on the first ask
    /// in the background (None while it runs), its result kept for that commit. A deployment
    /// that moves under the build drops the result (the next ask builds again). `timed`: a timed
    /// lease runs here (read by the caller, which holds the leases).
    #[cfg(feature = "checkout")]
    fn hub_built(&self, root: &Path, at: &str, timed: bool) -> Option<Result<(), String>> {
        let mut b = self.hub_build.lock().unwrap();
        match &*b {
            HubBuild::Done { at: a, ok } if a == at => return Some(ok.clone()),
            // one build at a time (cargo would queue a second on its lock anyway)
            HubBuild::Running { .. } => return None,
            _ => {}
        }
        // a timed lease runs alone: the build (rustc on every core) starts after it ends (two bench
        // boxes' timed series ran beside rustc at 145% of a core, 11-03)
        if timed {
            return None;
        }
        *b = HubBuild::Running { since: Instant::now() };
        drop(b);
        self.emit("hub", Sev::Info, format!("hub: {} builds its new hub at {} before it restarts", self.machine, &at[..at.len().min(10)]));
        let (state, root, at, emit, machine) = (self.hub_build.clone(), root.to_path_buf(), at.to_string(), self.emitter(), self.machine.clone());
        let _ = std::thread::Builder::new().name("devhub-hub-build".into()).spawn(move || {
            let ok = build_hub(&root);
            let now = super::tree::deploy(&root).map(|p| p.makepad).unwrap_or_default();
            let secs = match &*state.lock().unwrap() { HubBuild::Running { since, .. } => since.elapsed().as_secs(), _ => 0 };
            // (the master says a failure loudly, once for the fleet)
            match &ok {
                Ok(()) => emit("hub", Sev::Info, format!("hub: {machine}'s new hub built at {} in {secs} s", &at[..at.len().min(10)])),
                Err(why) => emit("hub", Sev::Warn, format!("hub: {machine}'s new hub does not build at {}: {why}; it keeps running", &at[..at.len().min(10)])),
            }
            *state.lock().unwrap() = if now == at && !crate::sync::serve::applying() { HubBuild::Done { at, ok } } else { HubBuild::None };
        });
        None
    }

    /// `service-restart <name>`: the self-update of a service the WM supervises (`ai-hub`): it
    /// gets SIGTERM (it closes admission and hands its jobs back), and the WM starts it again
    /// through `cargo run` in the tree: the new code.
    fn service_restart(&self, name: &str) -> String {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') || name == "devhub" {
            return err("usage: service-restart <name of a WM-supervised service> (devhub: hub-restart)");
        }
        let state = makepad_network::fleet::makepad_home().join("wm").join(format!("{name}.state"));
        let pid = std::fs::read_to_string(&state).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("pid = ").and_then(|p| p.trim().parse::<u32>().ok())));
        let Some(pid) = pid.filter(|p| crate::procinfo::alive(*p)) else { return err(format!("{name} does not run on {}", self.machine)) };
        // the apply's prebuild makes the checkout's binary the deployment's: the restart waits for
        // it (a restart during it built the hub again beside it, 11-02) and is only for a program
        // the service does not run (a restart reloads the AI hub's models)
        #[cfg(feature = "checkout")]
        if crate::sync::serve::applying() {
            return err("waits: the deployment's build runs");
        }
        #[cfg(feature = "checkout")]
        if name == "ai-hub" && crate::sync::SyncConf::load().builds_hub() {
            let binary = std::fs::read_to_string(&state).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("identity = ").and_then(|i| i.split(' ').next()).map(std::path::PathBuf::from)));
            if let Some(binary) = binary.filter(|b| makepad_app_registry::runs_binary(pid, b) == Some(true)) {
                self.emit("hub", Sev::Info, format!("hub: {}'s {name} (pid {pid}) runs the deployment's build already: no restart", self.machine));
                return format!("{{\"current\":{},\"pid\":{pid},\"binary\":{}}}", json_str(name), json_str(&binary.display().to_string()));
            }
        }
        // a user-facing service never restarts under its user (the master's roll waits and asks again)
        let blockers = user_blockers();
        if !blockers.is_empty() {
            return err(format!("waits: {}", blockers.join("; ")));
        }
        // kill by the pid its supervisor recorded
        let ok = std::process::Command::new("kill").args(["-TERM", &pid.to_string()]).status().is_ok_and(|s| s.success());
        if !ok {
            return err(format!("{name} (pid {pid}): kill -TERM failed"));
        }
        self.emit("hub", Sev::Info, format!("hub: {} restarts {name} (pid {pid}) for the self-update", self.machine));
        format!("{{\"restarting\":{},\"pid\":{pid}}}", json_str(name))
    }

    /// `service-health <name>`: whether a WM-supervised service answers again (the AI hub: its
    /// /health), for the master's roll, which restarts the next box's only once this one does.
    fn service_health(&self, name: &str) -> String {
        match (name, crate::local::ai_hub_answers()) {
            ("ai-hub", Some(up)) => format!("{{\"answers\":{up}}}"),
            ("ai-hub", None) => err(format!("no AI hub is set up on {}", self.machine)),
            _ => err(format!("service-health: no health check for {name}")),
        }
    }

    /// `lease-owns <id> <pid>`: whether the process is one of the lease's runs (a JIT a job
    /// started: the master pauses that job on its parked panic).
    fn owns(&self, rest: &str) -> String {
        let w: Vec<&str> = rest.split_whitespace().collect();
        let (Some(id), Some(Ok(pid))) = (w.first(), w.get(1).map(|p| p.parse::<u32>())) else { return err("usage: lease-owns <id> <pid>") };
        let leases = self.leases.lock().unwrap();
        let Some(l) = leases.get(*id) else { return err(format!("no lease {id} on {}", self.machine)) };
        let owns = l.runs.iter().any(|r| r.pid != 0 && (r.pid == pid || os::in_group(pid, r.pid, &r.hold)));
        format!("{{\"owns\":{owns}}}")
    }

    fn log(&self, rest: &str) -> String {
        let w: Vec<&str> = rest.split_whitespace().collect();
        let (Some(id), Some(Ok(which)), Some(Ok(off))) = (w.first(), w.get(1).map(|x| x.parse::<u32>()), w.get(2).map(|x| x.parse::<u64>())) else {
            return err("usage: lease-log <id> <run n|pid> <offset>");
        };
        let run = {
            let leases = self.leases.lock().unwrap();
            let Some(l) = leases.get(*id) else { return err(format!("no lease {id} on {}", self.machine)) };
            match l.runs.iter().find(|r| r.n == which).or_else(|| l.runs.iter().find(|r| r.pid == which)) {
                Some(r) => { r.adopted_exit(); r.clone() }
                None => return err(format!("lease {id} has no run {which}")),
            }
        };
        // the exit is known before the last bytes are read: say it only once they are
        let exit = *run.exit.lock().unwrap();
        use std::io::{Read, Seek, SeekFrom};
        let mut buf = Vec::new();
        let mut len = 0;
        if let Ok(mut f) = std::fs::File::open(&run.log) {
            len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if f.seek(SeekFrom::Start(off)).is_ok() {
                let _ = f.take(LOG_CHUNK as u64).read_to_end(&mut buf);
            }
        }
        let end = off + buf.len() as u64;
        if exit.is_some() && run.ended() > 0 {
            if let Some(l) = self.leases.lock().unwrap().get_mut(*id) {
                l.phases.push("report", run.ended(), crate::proto::wall_ms());
            }
        }
        #[cfg(feature = "checkout")]
        if exit.is_some() && end >= len && buf.is_empty() {
            let mut leases = self.leases.lock().unwrap();
            if let Some(l) = leases.get_mut(*id) {
                // A completed log acknowledges the start answer. A live retry may now run
                // the same argv again; polls before that always get the original pid and n.
                if l.resolution.as_ref().is_some_and(|r| r.reported && r.run == Some(run.n)) && l.runs.iter().any(|r| Arc::ptr_eq(r, &run)) { l.resolution = None; }
            }
        }
        let exit = if end >= len { exit.map_or("null".to_string(), |c| c.to_string()) } else { "null".into() };
        format!("{{\"data\":\"{}\",\"off\":{end},\"exit\":{exit}}}", makepad_network::digest::base64_encode(&buf))
    }

    /// The master's beat at `now`, naming the leases it holds here. Back after an absence
    /// ([`Opts::no_beat`] without a beat), that beat is its claim: a lease from before the absence
    /// it does not name ends now, and every other one's no-beat count starts again. Counted from
    /// any first call instead (its caps poll), a restarted master whose first beat came more than
    /// the limit later lost leases it held (a loaded gate box, 11-04).
    fn master_contact(&self, now: Instant, claimed: &[&str]) {
        let mut leases = self.leases.lock().unwrap();
        let (away, last) = {
            let mut seen = self.master_seen.lock().unwrap();
            let last = *seen;
            *seen = last.max(now);
            (now.saturating_duration_since(last), last)
        };
        if away < self.opts.no_beat {
            return;
        }
        let gone: Vec<String> = leases.values().filter(|l| l.beat <= last && !claimed.contains(&l.id.as_str())).map(|l| l.id.clone()).collect();
        let gone: Vec<Lease> = gone.iter().filter_map(|id| leases.remove(id)).collect();
        for l in leases.values_mut() {
            l.beat = l.beat.max(now);
        }
        let n = leases.len();
        drop(leases);
        self.master_away_said.store(false, Ordering::Release);
        let ended = if gone.is_empty() { String::new() } else { format!("; {} it no longer holds end", gone.iter().map(|l| l.id.as_str()).collect::<Vec<_>>().join(" ")) };
        self.emit("lease", Sev::Info, format!("lease: the master is back on {} after {} s away; {n} lease(s) kept for it{ended}", self.machine, away.as_secs()));
        for l in gone {
            self.emit("lease", Sev::Warn, format!("lease: {} {} of {} ended by {} itself: the master, back, no longer holds it", l.id, l.kind.as_str(), l.lane, self.machine));
            self.end(l, "expired");
        }
    }

    /// `lease-beat <ids>`: the master holds these leases (and no other here).
    fn beat(&self, rest: &str) -> String {
        let ids = || rest.split_whitespace();
        let now = Instant::now();
        self.master_contact(now, &ids().collect::<Vec<_>>());
        let samples: Vec<_> = {
            let leases = self.leases.lock().unwrap();
            ids().filter_map(|id| leases.get(id).map(|l| (id.to_string(), l.runs.clone()))).collect()
        };
        let samples: BTreeMap<_, _> = samples.into_iter().map(|(id, runs)| (id, (sample_runs(&runs), held_now(&runs), activity_now(&runs)))).collect();
        self.beat_from(rest, &samples, now)
    }

    /// A beat's bookkeeping from what its look measured at `now`: each lease's peak and held
    /// RAM and what its runs had done ([`Look`]).
    fn beat_from(&self, rest: &str, samples: &BTreeMap<String, Look>, now: Instant) -> String {
        let ids = || rest.split_whitespace();
        let mut leases = self.leases.lock().unwrap();
        // a timed lease runs alone: no disk walk of the hub's beside it
        let walk = !leases.values().any(|l| l.kind == Kind::Timed);
        let (mut live, mut unknown, mut info) = (Vec::new(), Vec::new(), Vec::new());
        for id in ids() {
            match leases.get_mut(id) {
                Some(l) => {
                    l.beat = now;
                    let (peak, held, done) = samples.get(id).copied().unwrap_or_default();
                    l.quiet = next_quiet(l.quiet, done, now);
                    let peak = peak.filter(|&n| l.peak_ram_bytes.is_none_or(|old| n > old));
                    let held = held.filter(|&n| l.held_bytes.is_none_or(|old| n > old));
                    if peak.is_some() || held.is_some() {
                        l.peak_ram_bytes = peak.or(l.peak_ram_bytes);
                        l.held_bytes = held.or(l.held_bytes);
                        self.save(l);
                    }
                    live.push(json_str(id));
                    // what it runs now (its newest live process) and its disk use, for the panel
                    let running = l.runs.iter().rev().find(|r| r.pid != 0 && os::proc_start(r.pid) == Some(r.start)).and_then(|r| { let pid = r.work_pid(); crate::top::label(pid).map(|c| (pid, c)) });
                    let mb = self.disk_of(id, l.dirs(&self.guard.roots.work), walk);
                    // what its runs wrote so far: a stall check spares a lease whose output grows
                    let out: u64 = l.runs.iter().filter_map(|r| std::fs::metadata(&r.log).ok()).map(|m| m.len()).sum();
                    info.push(format!(
                        "{}:{{\"running\":{},\"pid\":{},\"work\":{},\"disk_mb\":{},\"held_mb\":{},\"idle_s\":{},\"out_bytes\":{out},\"class\":{},\"phases\":{}}}",
                        json_str(id),
                        running.as_ref().map_or("null".into(), |(_, c)| json_str(c)),
                        running.as_ref().map_or(0, |(p, _)| *p),
                        json_str(&l.work.to_string_lossy()),
                        mb.map_or("null".into(), |m| m.to_string()),
                        l.held_bytes.map_or("null".into(), |b| (b >> 20).to_string()),
                        l.quiet.map_or("null".into(), |(since, _)| now.saturating_duration_since(since).as_secs().to_string()),
                        json_str(&l.first_run),
                        l.profile(crate::proto::wall_ms(), false).json()
                    ));
                }
                None => unknown.push(json_str(id)),
            }
        }
        let end = self.over_disk.lock().unwrap().take().filter(|(id, _)| ids().any(|i| i == id));
        let end = end.map_or(String::new(), |(id, why)| format!(",\"end\":{{\"id\":{},\"why\":{}}}", json_str(&id), json_str(&why)));
        format!("{{\"live\":[{}],\"unknown\":[{}],\"info\":{{{}}}{end}}}", live.join(","), unknown.join(","), info.join(","))
    }

    /// A lease's disk use in MB, measured at most every 2 min on a thread of its own (a target
    /// dir walk can take seconds); None until the first measure. `dirs`: [`Lease::dirs`].
    fn disk_of(&self, id: &str, dirs: Vec<(PathBuf, bool)>, walk: bool) -> Option<u64> {
        let mut m = self.disk_use.lock().unwrap();
        let known = m.get(id).copied();
        if walk && known.is_none_or(|k| k.at.elapsed() > std::time::Duration::from_secs(120)) {
            // mark it measured now, so one walk runs at a time
            let was = known.unwrap_or(DiskUse { at: Instant::now(), mb: u64::MAX, own_mb: 0, first_own_mb: None });
            m.insert(id.to_string(), DiskUse { at: Instant::now(), ..was });
            let (map, id) = (self.disk_use.clone(), id.to_string());
            let _ = std::thread::Builder::new().name("devhub-lease-du".into()).spawn(move || {
                let mut seen = std::collections::HashSet::new();
                let (mut all, mut own) = (0, 0);
                for (d, mine) in dirs.iter().filter(|(d, _)| seen.insert(d.clone())) {
                    let bytes = super::disk::dir_size(d).0;
                    all += bytes;
                    if *mine { own += bytes; }
                }
                let own_mb = own >> 20;
                map.lock().unwrap().insert(id, DiskUse { at: Instant::now(), mb: all >> 20, own_mb, first_own_mb: was.first_own_mb.or(Some(own_mb)) });
            });
        }
        known.map(|k| k.mb).filter(|m| *m != u64::MAX)
    }

    /// How much a lease's own directories grew since their first measure, bytes.
    fn disk_grew(&self, id: &str) -> u64 {
        self.disk_use.lock().unwrap().get(id).and_then(|u| Some(u.own_mb.saturating_sub(u.first_own_mb?) << 20)).unwrap_or(0)
    }

    /// `lease-end <id> <how> [{"inspect":{"pid":N,"breaks":[..],"probes":[..],"paused":true},"tail":true}]`;
    /// the answer carries the box's load measured after the teardown ([`with_load`]). `tail`: a
    /// lease something else ended, whose `how` here may well read `released`: the master knows.
    fn end_cmd(&self, rest: &str) -> String {
        let mut w = rest.splitn(3, ' ');
        let (id, how, extra) = (w.next().unwrap_or(""), w.next().unwrap_or("released"), w.next().unwrap_or("").trim());
        if !valid_name(id) || !matches!(how, "released" | "expired" | "holder-dead" | "denied") {
            return err("usage: lease-end <id> released|expired|holder-dead|denied [inspect json]");
        }
        let extra = if extra.is_empty() { None } else {
            let Ok(v) = makepad_strict_json::parse(extra.as_bytes()) else { return err("lease-end: the inspect part is not JSON") };
            Some(v)
        };
        {
            let mut endings = self.endings.lock().unwrap();
            if let Some(receipt) = endings.get(id).cloned() {
                drop(endings);
                // A final receipt whose write failed is retried without repeating teardown.
                if makepad_strict_json::parse(receipt.as_bytes()).ok().is_some_and(|v| v.get("ended").is_some()) {
                    if let Err(e) = self.save_end(id, &receipt) { return err(e); }
                }
                return receipt;
            }
            match std::fs::read_to_string(self.state.join(format!("{id}.ended"))) {
                Ok(receipt) => {
                    drop(endings);
                    return self.recheck_leak(id, receipt);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return err(format!("reading lease {id} cleanup receipt: {e}")),
            }
            endings.insert(id.to_string(), format!("{{\"pending\":\"lease cleanup in progress\",\"id\":{}}}", json_str(id)));
        }
        let mut inspect = String::new();
        if let Some(i) = extra.as_ref().and_then(|v| v.get("inspect")) {
            let list = |k: &str| i.get(k).and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>());
            let pid = i.get("pid").and_then(super::num).unwrap_or(0.0) as u32;
            if pid == 0 {
                self.endings.lock().unwrap().remove(id);
                return err("inspect cleanup needs the canonical app pid");
            }
            let done = inspect_release(pid, &list("breaks"), &list("probes"), i.get("paused").and_then(|x| x.as_bool()).unwrap_or(false));
            if done.iter().any(|c| c.ends_with(" (failed)")) {
                self.endings.lock().unwrap().remove(id);
                return err("inspect cleanup failed; ownership retained");
            }
            self.emit("lease", Sev::Info, format!("inspect: lease {id} released pid {pid} on {}: {}", self.machine, done.join(", ")));
            inspect = format!(",\"inspect\":[{}]", done.iter().map(|d| json_str(d)).collect::<Vec<_>>().join(","));
        }
        let l = self.leases.lock().unwrap().remove(id);
        let e = match l {
            Some(l) => {
                let keep_tail = extra.as_ref().and_then(|v| v.get("tail")).and_then(|t| t.as_bool()) == Some(true);
                return self.end_with_inspect(l, how, &inspect, keep_tail);
            }
            None if !inspect.is_empty() => format!("{{\"ended\":{},\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[]}}", json_str(id)),
            // no record of it here (never started, or the node's state went): its marked
            // processes are what can be left of it
            None => {
                let (killed, leak) = stop_strays(&lease_mark(&self.state, id), self.opts.grace);
                format!("{{\"ended\":{},\"killed\":{killed:?},\"freed_mb\":0,\"recovered\":null,\"leak\":{leak:?},\"unknown\":true}}", json_str(id))
            }
        };
        let receipt = format!("{}{inspect}}}", &e[..e.len() - 1]);
        if let Err(e) = self.save_end(id, &receipt) { return err(e); }
        receipt
    }

    /// A saved receipt that named survivors is checked again on every repeated end: once its
    /// processes are gone, the receipt says so and the master frees the capacity.
    fn recheck_leak(&self, id: &str, receipt: String) -> String {
        let Ok(v) = makepad_strict_json::parse(receipt.as_bytes()) else { return receipt };
        let leak: Vec<u32> = v.get("leak").and_then(|l| l.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|p| super::num(p).map(|n| n as u32)).collect());
        if leak.is_empty() {
            return receipt;
        }
        let (_, strays) = stop_strays(&lease_mark(&self.state, id), self.opts.grace);
        if !strays.is_empty() || leak.iter().any(|p| os::proc_start(*p).is_some()) {
            return receipt;
        }
        let gone = receipt.replacen(&format!("\"leak\":{leak:?}"), "\"leak\":[]", 1);
        match self.save_end(id, &gone) {
            Ok(()) => gone,
            Err(_) => receipt,
        }
    }

    fn save_end(&self, id: &str, receipt: &str) -> Result<(), String> {
        let mut endings = self.endings.lock().unwrap();
        endings.insert(id.to_string(), receipt.to_string());
        let path = self.state.join(format!("{id}.ended"));
        let tmp = self.state.join(format!("{id}.ended.new"));
        std::fs::create_dir_all(&self.state).and_then(|_| std::fs::write(&tmp, receipt)).and_then(|_| std::fs::rename(&tmp, path))
            .map_err(|e| format!("lease {id}: could not save cleanup receipt: {e}"))?;
        let _ = std::fs::remove_file(self.state.join(format!("{id}.json")));
        endings.remove(id);
        Ok(())
    }

    /// Ends a lease taken out of the table: processes, scratch, workspace; the answer line.
    fn end(&self, l: Lease, how: &str) -> String {
        self.end_with_inspect(l, how, "", how != "released")
    }

    fn end_with_inspect(&self, mut l: Lease, how: &str, inspect: &str, keep_tail: bool) -> String {
        self.endings.lock().unwrap().entry(l.id.clone()).or_insert_with(|| "{\"pending\":\"lease cleanup in progress\"}".into());
        let ended = crate::proto::wall_ms();
        if let Some(peak) = sample_runs(&l.runs) { l.peak_ram_bytes = Some(l.peak_ram_bytes.unwrap_or(0).max(peak)); }
        #[cfg(feature = "checkout")]
        l.identity.cancelled.store(true, Ordering::Release);
        // what the next lease needs first: the processes stopped, the tree let go; the rest (its scratch
        // dir measured and removed, the disk guard) after the answer, in the background
        // how its last run ended, when that was not its normal end (read before the stop: what the
        // stop ends is the lease, not a run that ended by itself)
        let abnormal = l.runs.last().and_then(|r| { r.adopted_exit(); let exit = *r.exit.lock().unwrap(); exit.and_then(|e| abnormal_end(&l.id, l.kind, r, e)) });
        // (and before the cleanup below removes the run logs)
        let failed = failed_tests(&l.runs).map_or("null".into(), |t| json_str(&t));
        // a lease something else ended keeps how far its last run got: a cancelled hung test left
        // nothing that named it (11-05)
        let cut = l.runs.last().filter(|_| keep_tail).map_or("null".into(), |r| json_str(&got_to(r, usize::MAX)));
        note_stopping(&l.id, &l.tmp, job_pids(&l.runs));
        let (mut killed, mut leak) = stop_runs(&l.runs, &makepad_remote::discovery::registry_dir(), false);
        let (strays, stray_leak) = stop_strays(&lease_mark(&self.state, &l.id), self.opts.grace);
        if !strays.is_empty() {
            self.emit("lease", Sev::Warn, format!("lease: {} {} of {} on {}: stopped {} process(es) that had left its process groups: {strays:?}", l.id, l.kind.as_str(), l.lane, self.machine, strays.len()));
        }
        killed.extend(strays);
        leak.extend(stray_leak);
        #[cfg(feature = "checkout")]
        let mut l = {
            let mut l = l;
            if let Some(worker) = l.selftest.take() {
                if worker.join().is_err() { self.emit("lease", Sev::Warn, format!("selftest worker {} panicked", l.id)); }
            }
            l
        };
        if let Some(peak) = sample_runs(&l.runs) { l.peak_ram_bytes = Some(l.peak_ram_bytes.unwrap_or(0).max(peak)); }
        let measured: Vec<_> = l.runs.iter().filter(|r| r.pid != 0).collect();
        let cpu = if measured.is_empty() { None } else { measured.iter().try_fold(0.0, |a, r| r.usage.lock().unwrap().cpu.map(|c| a + c)) };
        let _ = std::fs::remove_file(self.quiet().join(format!("{}.cmd", l.id)));
        if leak.is_empty() && (l.src == Src::Overlay || l.tmp.join("overlay-selftest.json").is_file() || l.work == self.conf().work.join("native").join(l.tree_lane())) {
            #[cfg(feature = "checkout")]
            l.identity.release_jits.store(true, Ordering::Release);
            if let Err(e) = super::jitstore::Store::new(&self.conf().work).release(&l.id) { self.emit("lease", Sev::Warn, format!("JIT store release {}: {e}", l.id)); }
        }
        #[cfg(feature = "checkout")]
        if l.tmp.join("overlay-selftest.json").is_file() && leak.is_empty() {
            if let Err(e) = super::overlay_selftest::cleanup(&self.conf().work, &l.id) { self.emit("lease", Sev::Warn, format!("selftest cleanup {}: {e}", l.id)); }
        }
        let recovered = self.release_tree(&l);
        let profile = l.profile(ended, true);
        for r in &l.runs {
            let _ = std::fs::remove_file(r.exit_file.with_extension("time"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("usage"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("usage.pid"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("usage.cgroup"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("cpu"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("warmup"));
            let _ = std::fs::remove_file(&r.exit_file);
        }
        let sev = if how == "released" { Sev::Info } else { Sev::Warn };
        self.emit("lease", sev, format!("lease: {} {} of {} ended on {} ({how}): {} process group(s) stopped", l.id, l.kind.as_str(), l.lane, self.machine, killed.len()));
        self.guard.touch();
        let (tmp, emit, tag, machine) = (l.tmp.clone(), self.emitter(), format!("{} {} on {}", l.id, l.lane, self.machine), self.machine.clone());
        #[cfg(feature = "checkout")]
        let edits = (l.src == Src::Overlay && leak.is_empty()).then(|| l.edits()).flatten();
        #[cfg(feature = "checkout")]
        let (this, overlay) = (self.this.clone(), (l.kind == Kind::Ci && l.src == Src::Overlay && leak.is_empty()).then(|| overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or(""))).filter(|lane| !lane.is_empty() && lane != &l.lane));
        let _ = std::thread::Builder::new().name("devhub-lease-cleanup".into()).spawn(move || {
            let mut freed = 0u64;
            if tmp.exists() {
                freed += disk::dir_size(&tmp).0;
                let _ = std::fs::remove_dir_all(&tmp);
            }
            #[cfg(feature = "checkout")]
            if let Some(edits) = edits {
                freed += disk::dir_size(&edits).0;
                let _ = std::fs::remove_dir_all(&edits);
            }
            #[cfg(feature = "checkout")]
            if let (Some(lane), Some(this)) = (overlay, this.upgrade()) {
                let _starting = this.starting.lock().unwrap();
                let live = this.leases.lock().unwrap().values().any(|l| l.src == Src::Overlay && overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or("")) == lane);
                if !live {
                    let w = this.conf().work;
                    for p in [w.join("dev/overlay").join(&lane), w.join("dev/lane").join(&lane)] {
                        freed += disk::dir_size(&p).0;
                        let _ = std::fs::remove_dir_all(&p);
                    }
                }
            }
            emit("ws", Sev::Info, format!("ws_cleanup: {tag} freed {} MB", freed >> 20));
            // what the lease's threads freed goes back to the system
            if let Some(held) = crate::heap::trim() {
                emit("lease", Sev::Warn, format!("hub memory on {machine}: {held} (devhub heap {machine})"));
            }
        });
        let freed = 0u64;
        if !leak.is_empty() {
            self.emit("lease", Sev::Err, format!("lease_leak: {} {} on {}: pid(s) {:?} survived KILL", l.id, l.lane, self.machine, leak));
        }
        let line = self.ended_line(&l.id, &killed, freed, &recovered, &leak);
        let abnormal = abnormal.map_or("null".into(), |(line, serious, tail)| format!("{{\"line\":{},\"serious\":{serious},\"tail\":{}}}", json_str(&line), json_str(&tail)));
        #[cfg(unix)]
        let noise = self.noise.lock().unwrap().remove(&l.id).filter(|n| n.samples > 0).map_or(String::new(), |n| format!(",\"box\":{}", n.json()));
        #[cfg(not(unix))]
        let noise = "";
        let receipt = format!("{},\"source\":{},\"class\":{},\"phases\":{},\"peak_ram_gb\":{},\"cpu_seconds\":{},\"abnormal\":{abnormal},\"failed_tests\":{failed},\"tail\":{cut}{inspect}{noise}}}", line.trim_end_matches('}'), json_str(&l.source), json_str(&l.first_run), profile.json(), l.peak_ram_bytes.map_or("null".into(), |n| (n as f64 / (1u64 << 30) as f64).to_string()), cpu.map_or("null".into(), |n| n.to_string()));
        if let Err(e) = self.save_end(&l.id, &receipt) { return err(e); }
        receipt
    }

    /// The lease lets go of the node's tree (the last one bundles what it left there): the bundles.
    #[cfg(feature = "checkout")]
    fn release_tree(&self, l: &Lease) -> Vec<PathBuf> {
        if l.src == Src::Overlay { return Vec::new(); }
        let (Some(pin), Some(_)) = (&l.pin, &self.opts.tree) else { return Vec::new() };
        let root = &l.work;
        let conf = self.conf();
        let work = &conf.work;
        if !l.overlay.is_empty() {
            match super::Pin::parse(pin).ok_or_else(|| format!("bad pin {pin}")).and_then(|p| super::tree::undo_overlay(root, &p, &l.overlay)) {
                Ok(n) => self.emit("ws", Sev::Info, format!("ws_overlay: {} {} on {}: {n} shipped file(s) undone", l.id, l.lane, self.machine)),
                Err(e) => self.emit("ws", Sev::Err, format!("ws_overlay: {} {} on {}: undoing the shipped files failed: {e}", l.id, l.lane, self.machine)),
            }
        }
        let r = super::tree::release(&l.id, root, &work.join("recovered"), &format!("{}-{}", l.lane, l.id));
        self.report_tree(&format!("{} {} on {}", l.id, l.lane, self.machine), &r);
        r.bundles
    }

    /// What clearing the tree took out, and an error event when it could not.
    #[cfg(feature = "checkout")]
    fn report_tree(&self, who: &str, r: &super::tree::Released) {
        for b in &r.bundles {
            self.emit("ws", Sev::Warn, format!("ws_recovered: {who} left changes in the tree: bundled to {}", b.display()));
        }
        if !r.removed.is_empty() {
            let shown: Vec<&str> = r.removed.iter().take(8).map(String::as_str).collect();
            self.emit("ws", Sev::Info, format!("ws_strays: {who}: {} untracked path(s) removed from the tree: {}{}", r.removed.len(), shown.join(" "), if r.removed.len() > shown.len() { " .." } else { "" }));
        }
        for e in &r.unclean {
            self.emit("ws", Sev::Err, format!("ws_unclean: {who}: the tree is not clean, later leases may fail on it: {e}"));
        }
    }

    #[cfg(not(feature = "checkout"))]
    fn release_tree(&self, _l: &Lease) -> Vec<PathBuf> {
        Vec::new()
    }

    fn ended_line(&self, id: &str, killed: &[u32], freed: u64, recovered: &[PathBuf], leak: &[u32]) -> String {
        let r = |p: Option<&PathBuf>| p.map_or("null".to_string(), |p| json_str(&p.to_string_lossy()));
        format!(
            "{{\"ended\":{},\"killed\":{:?},\"freed_mb\":{},\"recovered\":{},\"recovered_all\":[{}],\"leak\":{:?}}}",
            json_str(id),
            killed,
            freed >> 20,
            r(recovered.last()),
            recovered.iter().map(|p| json_str(&p.to_string_lossy())).collect::<Vec<_>>().join(","),
            leak
        )
    }

    /// Watches detached runs and ends the leases the master, there, no longer beats.
    fn expire(&self) {
        self.expire_at(Instant::now());
    }

    /// [`Self::expire`] as of `now` (a test drives the clock). While the master is away (no call
    /// of its for [`Opts::no_beat`]: it restarts, its box is down, the link is lost) every lease
    /// is kept for it, and that is said once: a box never ends its leases for a master's absence,
    /// only for a lease the master, back, does not claim ([`Self::master_contact`]).
    fn expire_at(&self, now: Instant) {
        let stale: Vec<Lease> = {
            let mut leases = self.leases.lock().unwrap();
            for l in leases.values() {
                for r in &l.runs { r.adopted_exit(); }
            }
            let away = now.saturating_duration_since(*self.master_seen.lock().unwrap());
            if away >= self.opts.no_beat {
                if !leases.is_empty() && !self.master_away_said.swap(true, Ordering::AcqRel) {
                    let line = format!("lease: master away {} s: no call from it on {}; its {} lease(s) here are kept until it is back", away.as_secs(), self.machine, leases.len());
                    drop(leases);
                    eprintln!("devhub: {line}");
                    self.emit("lease", Sev::Warn, line);
                }
                return;
            }
            let ids: Vec<String> = leases.values().filter(|l| now.saturating_duration_since(l.beat) >= self.opts.no_beat).map(|l| l.id.clone()).collect();
            ids.iter().filter_map(|id| leases.remove(id)).collect()
        };
        for l in stale {
            self.emit("lease", Sev::Warn, format!("lease: {} {} of {} ended by {} itself: the master, there, has not beaten it for {} s", l.id, l.kind.as_str(), l.lane, self.machine, self.opts.no_beat.as_secs()));
            self.end(l, "expired");
        }
    }

    fn recovered_path(&self, p: &str) -> Result<PathBuf, String> {
        let root = std::fs::canonicalize(self.conf().work.join("recovered")).map_err(|_| "nothing recovered on this node".to_string())?;
        let p = std::fs::canonicalize(p).map_err(|e| format!("{p}: {e}"))?;
        if !p.starts_with(&root) || !p.is_file() {
            return Err("only files under <work>/recovered".into());
        }
        Ok(p)
    }

    fn recover_get(&self, rest: &str) -> String {
        let (p, off) = rest.rsplit_once(' ').unwrap_or((rest, "0"));
        let Ok(off) = off.parse::<u64>() else { return err("usage: recover-get <path> <offset>") };
        let p = match self.recovered_path(p) {
            Ok(p) => p,
            Err(e) => return err(e),
        };
        use std::io::{Read, Seek, SeekFrom};
        let mut buf = Vec::new();
        let mut len = 0;
        if let Ok(mut f) = std::fs::File::open(&p) {
            len = f.metadata().map(|m| m.len()).unwrap_or(0);
            if f.seek(SeekFrom::Start(off)).is_ok() {
                let _ = f.take(RECOVER_CHUNK as u64).read_to_end(&mut buf);
            }
        }
        let end = off + buf.len() as u64;
        format!("{{\"data\":\"{}\",\"off\":{end},\"eof\":{}}}", makepad_network::digest::base64_encode(&buf), end >= len)
    }

    /// Completes a ship: exact overlay files, whiteouts and base blobs, or a tree's undo list.
    fn ship_done(&self, rest: &str) -> String {
        let (id, b64) = rest.split_once(' ').unwrap_or((rest, ""));
        let Some(v) = b64_decode(b64.trim()).and_then(|b| makepad_strict_json::parse(&b).ok()) else { return err("ship-done: not base64 JSON") };
        let list = |k: &str| v.get(k).and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect::<Vec<_>>());
        let (shipped, deleted) = (list("shipped"), list("deleted"));
        if let Some(bad) = shipped.iter().chain(&deleted).find(|p| !inside(p)) {
            return err(format!("ship-done: {bad} is not a path inside the tree"));
        }
        let mut leases = self.leases.lock().unwrap();
        let Some(l) = leases.get_mut(id) else { return err(format!("no lease {id} on {}", self.machine)) };
        if l.pin.is_none() {
            return err(format!("{id} holds no tree: ship-done is for a lease at a revision"));
        }
        if l.src == Src::Overlay {
            #[cfg(feature = "checkout")]
            {
                if shipped.iter().chain(&deleted).any(|p| !super::ship::inside(p) || p.starts_with(".makepad-run-")) { return err("overlay: invalid or reserved edit path"); }
                let Some(pin) = l.pin.as_deref().and_then(super::Pin::parse) else { return err("overlay: missing pin") };
                let Some(bases) = v.get("bases") else { return err("overlay: ship-done needs base blobs") };
                let Some(edits) = l.edits() else { return err("overlay: the lease has no edits folder") };
                l.source = match super::relaxgit::finish(&self.conf().work, &edits, &pin, &l.tmp.join("ship"), &shipped, &deleted, bases) { Ok(source) => source, Err(e) => return err(e) };
                l.overlay = shipped.into_iter().chain(deleted).collect();
                l.shipped = true;
                self.save(l);
                return format!("{{\"overlay\":{},\"source\":{}}}", l.overlay.len(), json_str(&l.source));
            }
            #[cfg(not(feature = "checkout"))]
            return err("overlay: RELAXGIT layer unavailable");
        }
        l.source = super::ship::source_in(&l.work, l.pin.as_deref().unwrap_or(""), &shipped, &deleted);
        let mut removed = 0;
        for f in &deleted {
            let p = l.work.join(f);
            if std::fs::symlink_metadata(&p).is_ok_and(|m| !m.is_dir()) && std::fs::remove_file(&p).is_ok() {
                removed += 1;
                prune_empty_parents(&l.work, f);
            }
        }
        for p in &shipped {
            l.fresh_gone.retain(|g| g != p);
            if !l.fresh_put.contains(p) { l.fresh_put.push(p.clone()); }
        }
        for p in &deleted {
            l.fresh_put.retain(|g| g != p);
            if !l.fresh_gone.contains(p) { l.fresh_gone.push(p.clone()); }
        }
        for p in shipped.into_iter().chain(deleted) {
            if !l.overlay.contains(&p) {
                l.overlay.push(p);
            }
        }
        self.save(l);
        format!("{{\"overlay\":{},\"deleted\":{removed},\"source\":{}}}", l.overlay.len(), json_str(&l.source))
    }

    fn ship_started(&self, id: &str) {
        if let Some(l) = self.leases.lock().unwrap().get_mut(id) {
            let before = l.phases.0.len();
            l.phases.begin("sync", crate::proto::wall_ms());
            if before != l.phases.0.len() { self.save(l); }
        }
    }

    /// Tree writes land directly; overlay ships stage per lease until ship-done publishes the set.
    fn ship(&self, word: &str, rest: &str) -> String {
        let (id, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        self.ship_started(id);
        let (work, overlay) = match self.leases.lock().unwrap().get(id) {
            Some(l) => (if l.src == Src::Overlay { l.tmp.join("ship") } else { l.work.clone() }, l.src == Src::Overlay),
            None => return err(format!("no lease {id} on {}", self.machine)),
        };
        if overlay && word == "ship-have" { if let Err(e) = super::ship::stage(&work, rest.trim()) { return err(e); } }
        let r = match word {
            "ship-have" => super::ship::have(&work, rest.trim()),
            "ship-put" => super::ship::put(&work, rest),
            _ => super::ship::link(&work, rest),
        };
        r.unwrap_or_else(err)
    }

    fn overlay_selftest(&self, rest: &str) -> String {
        #[cfg(feature = "checkout")]
        {
            let words: Vec<_> = rest.split_whitespace().collect();
            let (id, case) = match words.as_slice() { [id] => (*id, ""), [id, case] => (*id, *case), _ => return err("overlay-selftest <test lease> [case]") };
            if !case.is_empty() && !super::overlay_selftest::CASES.contains(&case) { return err(format!("unknown case {case}; cases: {}", super::overlay_selftest::CASES.join(", "))); }
            let mut leases = self.leases.lock().unwrap();
            if self.restarting.load(Ordering::Acquire) { return err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine)); }
            let Some(l) = leases.get_mut(id) else { return err(format!("no lease {id}")) };
            if l.kind != Kind::Test || l.src != Src::Tree || l.pin.is_some() { return err("overlay-selftest needs a test lease without a revision"); }
            let spec = l.tmp.join("overlay-selftest.json");
            if spec.exists() { return err("selftest already started on this lease"); }
            let Some(this) = self.this.upgrade() else { return err("selftest: hub stopped"); };
            let n = l.runs.len() as u32 + 1;
            let log = l.tmp.join(format!("run-{n}.log"));
            let output = match std::fs::File::create(&log) { Ok(f) => Mutex::new(f), Err(e) => return err(e.to_string()) };
            if let Err(e) = std::fs::write(&spec, format!("{{\"case\":{},\"n\":{n}}}", json_str(case))) { return err(e.to_string()); }
            // A pid of zero is the in-hub summary, not a process group. The actual JITs
            // and any Cargo build have their own registered runs and cancellation handling.
            let summary = Arc::new(Run { request: String::new(), started_ms: crate::proto::wall_ms(), spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid: 0, start: 0, unit: None, log, exit: Mutex::new(None), ours: true, detachable: false, hold: os::Hold::none(), exit_file: exit_file(&self.state, id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) });
            l.runs.push(summary.clone());
            l.first_run = "overlay-selftest".into();
            self.save(l);
            let (work, tmp, lease, case, identity, cores) = (self.conf().work, l.tmp.clone(), l.id.clone(), case.to_string(), l.identity.clone(), jobs(l.kind, l.cost));
            let completion = summary.clone();
            match std::thread::Builder::new().name("devhub-overlay-selftest".into()).spawn(move || {
                let active = || if identity.cancelled.load(Ordering::Acquire) { Err(format!("selftest: lease {lease} ended")) } else { Ok(()) };
                let say = |line: &str| {
                    use std::io::Write as _;
                    let mut output = output.lock().unwrap();
                    let _ = writeln!(output, "{line}");
                    let _ = output.flush();
                };
                let run = |id: &str, cmd: &std::process::Command, active: &dyn Fn() -> Result<(), String>| this.overlay_build(id, cmd, active);
                // the selftest's fixtures fail on purpose: their warnings go to its log
                let build = super::loaders::OverlayBuild { jobs: cores, niced: false, run: &run, cancelled: &identity.cancelled, say: &say, pending: &say, warn: &say, active: &active };
                let spawn = |cmd: &std::process::Command| this.worker_process(&lease, &identity, &tmp, cmd, &[], &[]);
                let code = super::overlay_selftest::run(&work, &tmp, &lease, &case, cores, &build, &spawn, &say);
                completion.finished(code);
            }) {
                Ok(worker) => l.selftest = Some(worker),
                Err(e) => {
                    summary.finished(-1);
                    return err(format!("selftest: worker thread: {e}"));
                }
            }
            format!("{{\"n\":{n}}}")
        }
        #[cfg(not(feature = "checkout"))]
        { let _ = rest; err("overlay-selftest needs a node with bare git stores") }
    }

    fn overlay_inspect(&self, word: &str, rest: &str) -> Vec<String> {
        let (lane, path) = rest.split_once(' ').unwrap_or((rest, ""));
        if !makepad_devhub_conf::valid_name(lane) || (word == "overlay-why" && (!super::ship::inside(path) || path.starts_with(".makepad-run-"))) { return vec![err("overlay: lane and relative path required")]; }
        #[cfg(feature = "checkout")]
        {
            let work = self.conf().work;
            let result = if word == "overlay-why" { super::relaxgit::why(&work, lane, path) } else { super::relaxgit::status(&work, lane) };
            result.unwrap_or_else(|e| vec![err(e)])
        }
        #[cfg(not(feature = "checkout"))]
        vec![err("overlay: RELAXGIT layer unavailable")]
    }

    /// The folder `devhub get` reads and the prefix that names it: a live lease's work
    /// (`target/`, `tmp/`: its target and scratch), else the lane's dir (`src/`: the tree `run`
    /// ships and runs in; `target/`: the lane's warm target). Returns (root, prefix, path under root).
    fn get_root(&self, lane: &str, lease: &str, rel: &str) -> Result<(PathBuf, &'static str, String), String> {
        if !valid_name(lane) || !inside(rel) {
            return Err("usage: <lane> <lease|-> <relative path inside the lane's dir>".into());
        }
        let under = |pre: &str| if rel == pre { Some(".".to_string()) } else { rel.strip_prefix(pre).and_then(|r| r.strip_prefix('/')).map(str::to_string) };
        if lease != "-" {
            let leases = self.leases.lock().unwrap();
            let l = leases.get(lease).filter(|l| l.lane == lane).ok_or_else(|| format!("no live lease {lease} of lane {lane} on {}", self.machine))?;
            return Ok(match (under("target"), under("tmp")) {
                (Some(r), _) => (l.target.clone(), "target/", r),
                (_, Some(r)) => (l.tmp.clone(), "tmp/", r),
                _ => (l.work.clone(), "", rel.to_string()),
            });
        }
        #[cfg(feature = "checkout")]
        {
            let work = self.conf().work;
            let root = super::tree::native_root(&work, lane);
            if root.join(".git").exists() {
                return Ok(match (under("target"), under("src")) {
                    (Some(r), _) => (super::tree::native_target(&work, lane), "target/", r),
                    (_, Some(r)) => (root, "src/", r),
                    _ => return Err("without --lease: `src/..` or `target/..`".into()),
                });
            }
        }
        // no live lease: the node's tree (a lane's results that outlive its lease are in the
        // shared target); a node without one: the lane's dir
        if let Some(t) = &self.opts.tree {
            return Ok(match (under("target"), under("src")) {
                (Some(r), _) => (t.join("target"), "target/", r),
                (_, Some(r)) => (t.clone(), "src/", r),
                _ => return Err("without --lease: `src/..` (the node's tree) or `target/..` (its target)".into()),
            });
        }
        let w = self.conf().work;
        Ok(match under("target") {
            Some(r) => (w.join("target").join(format!("lane-{lane}")), "target/", r),
            None => (w.join("lane").join(lane), "", rel.to_string()),
        })
    }

    /// `get-ls <lane> <lease|-> <rel>`: one `{"p":..,"n":..,"r":..}` line per file under it; `p`
    /// starts with the last component of <rel> (where rsync would put it in the target folder),
    /// `r` is what `get-read` takes.
    fn get_ls(&self, rest: &str) -> Vec<String> {
        let w: Vec<&str> = rest.splitn(3, ' ').collect();
        let [lane, lease, rel] = w[..] else { return vec![err("usage: get-ls <lane> <lease|-> <rel>")] };
        let (root, pre, sub) = match self.get_root(lane, lease, rel.trim()) {
            Ok(x) => x,
            Err(e) => return vec![err(e)],
        };
        let p = root.join(&sub);
        let base = if sub == "." { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or_else(|| root.clone()) };
        let mut files = Vec::new();
        if let Err(e) = files_under(&base, &p, GET_MAX_FILES, &mut files) {
            return vec![err(e)];
        }
        files
            .into_iter()
            .map(|(f, n)| {
                let under_root = base.join(&f);
                let r = format!("{pre}{}", slashed(under_root.strip_prefix(&root).unwrap_or(&under_root)));
                format!("{{\"p\":{},\"n\":{n},\"r\":{}}}", json_str(&f), json_str(&r))
            })
            .collect()
    }

    /// `get-read <lane> <lease|-> <off> <r>`: a chunk of one file `get-ls` listed.
    fn get_read(&self, rest: &str) -> String {
        let w: Vec<&str> = rest.splitn(4, ' ').collect();
        let [lane, lease, off, rel] = w[..] else { return err("usage: get-read <lane> <lease|-> <offset> <rel>") };
        let Ok(off) = off.parse::<u64>() else { return err("get-read: bad offset") };
        let (root, _, sub) = match self.get_root(lane, lease, rel.trim()) {
            Ok(x) => x,
            Err(e) => return err(e),
        };
        let p = root.join(sub);
        use std::io::{Read, Seek, SeekFrom};
        let mut f = match std::fs::File::open(&p) {
            Ok(f) => f,
            Err(e) => return err(format!("{}: {e}", p.display())),
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let mut buf = Vec::new();
        if f.seek(SeekFrom::Start(off)).is_ok() {
            let _ = f.take(GET_CHUNK as u64).read_to_end(&mut buf);
        }
        let end = off + buf.len() as u64;
        format!("{{\"data\":\"{}\",\"off\":{end},\"eof\":{}}}", makepad_network::digest::base64_encode(&buf), end >= len)
    }

    /// The master fetched the bundle: the node copy goes.
    /// `disk clean <lane>` (the lane landed): its source dir, overlay, warm target and legacy pool dir go,
    /// unless a live lease of the lane still uses them.
    fn clean_lane(&self, lane: &str) -> String {
        if !makepad_devhub_conf::valid_name(lane) {
            return err(format!("bad lane name `{lane}`"));
        }
        {
            let _starting = self.starting.lock().unwrap();
            if let Some(l) = self.leases.lock().unwrap().values().find(|l| l.lane == lane || (l.src == Src::Tree && l.tree_lane() == lane)) {
                return err(format!("lane {lane} holds live lease {}", l.id));
            }
            self.cleaning.0.lock().unwrap().insert(lane.to_string());
        }
        // a lease of the lane starts again once its dirs have left
        let left = self.lane_to_trash(lane);
        self.cleaning.0.lock().unwrap().remove(lane);
        self.cleaning.1.notify_all();
        let removed = match left {
            Ok(removed) => removed,
            Err(e) => return err(e),
        };
        let freed = super::disk::empty_trash(&self.conf().work);
        self.guard.touch();
        (self.publish)(crate::proto::hub_record(&self.machine, "", 0, "disk", crate::proto::Sev::Info, format!("disk: lane {lane} landed: {} dirs removed, {} MB freed", removed.len(), freed >> 20), String::new()));
        format!("{{\"cleaned\":{},\"removed\":[{}],\"freed_mb\":{}}}", json_str(lane), removed.join(","), freed >> 20)
    }

    /// A landed lane's dirs leave for the trash; the ones that left, as JSON strings.
    fn lane_to_trash(&self, lane: &str) -> Result<Vec<String>, String> {
        let conf = self.conf();
        let w = &conf.work;
        let paths = [w.join("dev/lane").join(lane), w.join("dev/overlay").join(lane), w.join("dev/lane-jit").join(lane), w.join("lane").join(lane), w.join("target").join(format!("lane-{lane}")), w.join("target").join(format!("lane-{lane}.last")), caps::home_dir().join("rc/pool").join(lane)];
        #[cfg(feature = "checkout")]
        super::relaxgit::forget(w, lane)?;
        let mut removed = Vec::new();
        #[cfg(feature = "checkout")]
        {
            let root = super::tree::native_root(w, lane);
            let target = super::tree::native_target(w, lane);
            if root.exists() || target.exists() {
                let r = super::tree::remove_native(w, lane)?;
                self.report_tree(&format!("cleaning native lane {lane}"), &r);
                removed.push(json_str(&root.to_string_lossy()));
                removed.push(json_str(&target.to_string_lossy()));
            }
        }
        for p in paths.iter().filter(|p| p.exists()) {
            super::disk::to_trash(w, p).map_err(|e| format!("{}: {e}", p.display()))?;
            removed.push(json_str(&p.to_string_lossy()));
        }
        Ok(removed)
    }

    fn recover_del(&self, rest: &str) -> String {
        match self.recovered_path(rest).and_then(|p| std::fs::remove_file(&p).map(|_| p).map_err(|e| e.to_string())) {
            Ok(p) => format!("{{\"deleted\":{}}}", json_str(&p.to_string_lossy())),
            Err(e) => err(e),
        }
    }
}

fn watchdog(w: Weak<Exec>, tick: Duration) {
    loop {
        std::thread::sleep(tick);
        let Some(x) = w.upgrade() else { return };
        x.expire();
    }
}

impl Exec {
    /// Processes this hub's leases' runs started (they carry its mark, [`lease_mark`]) whose lease is
    /// no longer on this node: a run that left its groups, or under a hub that died, keeps a box's
    /// capacity for good (10-31: a WM-phone smoke 24 h on a Mac box). TERM now, KILL at the next
    /// look. Only this hub's marks: another Exec on the box (a devhub test's, its own ledger) took
    /// every orphan for one of its gone leases and ended a gate's wrapper, orphaned by the hub's
    /// self-update, in the gate's own devhub tests (no verdict, 11-02); a process that
    /// dropped its mark on purpose (the check server) is no orphan of a lease.
    fn end_orphans(&self) {
        let live: Vec<String> = self.leases.lock().unwrap().keys().cloned().collect();
        let me = std::process::id();
        let mut seen = self.orphans.lock().unwrap();
        // (orphans only: a process whose parent still runs is someone's, whatever its environment)
        let orphan = |pid: u32| makepad_remote::group::command("ps").args(["-o", "ppid=", "-p", &pid.to_string()]).stdin(std::process::Stdio::null()).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "1");
        let mine = format!("{LEASE_ENV}={}/", self.state.display());
        let now: Vec<(u32, String)> = os::env_values(&mine).into_iter().filter(|(p, id)| *p != me && valid_name(id) && !live.contains(id) && orphan(*p)).collect();
        for (pid, id) in &now {
            let hard = seen.contains(pid);
            os::signal_one(*pid, hard);
            self.emit("lease", Sev::Warn, format!("lease: {}: {} pid {pid} of lease {id}, which is no longer here", self.machine, if hard { "killed" } else { "stopped" }));
        }
        *seen = now.into_iter().map(|(p, _)| p).collect();
    }
}

/// How often a box proves it can link.
const TOOLCHAIN_EVERY: Duration = Duration::from_secs(10 * 60);
/// How often a box looks whether a store command failed for it, or whether git works its stores
/// again.
const GIT_TICK: Duration = Duration::from_secs(30);

/// A box whose toolchain cannot link, or whose git fails on its stores, takes no leases until it
/// works again (`Caps::sick`): each is said once.
fn toolchain_loop(w: Weak<Exec>) {
    os::nice_this_thread();
    let mut linked: Option<std::time::Instant> = None;
    let mut git_said = String::new();
    loop {
        let Some(x) = w.upgrade() else { return };
        // a store command failed for the box (caps::git_failed): said at the next tick, probed at each
        super::caps::probe_git();
        let git_now = super::caps::git_sick();
        if git_now != git_said {
            if git_now.is_empty() {
                x.emit("git", Sev::Info, format!("git: {} works its stores again, back in rotation", x.machine));
            } else {
                x.emit("git", Sev::Err, format!("git: {} is out of rotation, git fails on its stores: {git_now}", x.machine));
            }
            git_said = git_now;
        }
        // a timed lease runs alone: the probe (a compile and a link) and the orphan sweep wait
        if x.timed_live() || linked.is_some_and(|t| t.elapsed() < TOOLCHAIN_EVERY) {
            drop(x);
            std::thread::sleep(GIT_TICK);
            continue;
        }
        linked = Some(std::time::Instant::now());
        let dir = x.conf().work.join("tmp").join("toolchain-probe");
        let (why, changed) = super::caps::probe_toolchain(&dir);
        x.end_orphans();
        if changed {
            match why {
                Some(why) => x.emit("toolchain", Sev::Err, format!("toolchain: {} is out of rotation, it cannot link: {why}", x.machine)),
                None => x.emit("toolchain", Sev::Info, format!("toolchain: {} links again, back in rotation", x.machine)),
            }
        }
        drop(x);
        std::thread::sleep(GIT_TICK);
    }
}

fn disk_loop(w: Weak<Exec>) {
    os::nice_this_thread();
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let Some(x) = w.upgrade() else { return };
        // a timed lease runs alone: only a disk below its floor is looked after meanwhile
        x.guard.check(&|| x.live(), x.timed_live());
        x.look_over_disk();
    }
}

/// What a lease put on its box's disk.
struct Took {
    id: String,
    /// how much its own directories grew since it began ([`Exec::disk_grew`])
    grew: u64,
    /// the memory it holds now ([`held_now`])
    held: u64,
}

/// The lease to end when one took the disk: below the floor after the reclaim, and without what
/// the biggest one put there the box would be above it. That is what its own directories grew
/// by, and on a box that swaps to its disk ((RAM, swap files), [`caps::swap_on_disk`]) the memory
/// it holds as far as swap files hold memory past the RAM. The biggest first; the next look finds
/// the next. A lease's 102 GB log took a disk down to 0 GB (11-04): the hub said `disk_short`
/// every minute and its ledger writes failed until someone cancelled it. (lease, why). What no
/// lease explains is none of theirs to pay for.
fn lease_over_disk(free: u64, floor: u64, swap: Option<(u64, u64)>, leases: &[Took]) -> Option<(String, String)> {
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let held: u64 = leases.iter().map(|l| l.held).sum();
    let in_swap = swap.map_or(0, |(ram, swap)| swap.min(held.saturating_sub(ram)));
    let took = |l: &Took| l.grew + l.held.min(in_swap);
    let l = leases.iter().max_by_key(|l| took(l))?;
    if free >= floor || took(l) < floor - free {
        return None;
    }
    let what: Vec<String> = [
        (l.grew > 0).then(|| format!("its directories grew {:.1} GB since it began", gb(l.grew))),
        (l.held.min(in_swap) > 0).then(|| format!("it holds {:.1} GB of memory while swap files take {:.0} GB", gb(l.held), gb(in_swap))),
    ].into_iter().flatten().collect();
    Some((l.id.clone(), format!("it took the box's disk: {}; {:.0} GB of disk is left under a {:.0} GB floor", what.join(", "), gb(free), gb(floor))))
}

/// One look at a stopping run ([`Stopping`]): long enough for a process at work to show it by the
/// stall check's [`busy`] (past 0.2 s of CPU, or 1 MB of I/O).
const STOP_LOOK: Duration = Duration::from_secs(2);

/// How long an app has to answer its `quit` ([`ask_exit`]): its UI thread takes it (the platform
/// gives that 4 s), then it answers before it winds down. No answer: it did not take the ask.
const QUIT_ANSWER: Duration = Duration::from_secs(10);

/// A run asked to exit ([`stop_runs`]): what took the ask, and its last look.
struct Stopping {
    /// Unix: its group got TERM (the default action ends a process: one that lives on handles it)
    termed: bool,
    /// the apps in it that answered `quit` (pid, start)
    apps: Vec<(u32, u64)>,
    look: Option<(Instant, os::Activity)>,
    killed: bool,
}

impl Stopping {
    /// Whether the run, still alive, is KILLed now: nothing in it took the ask (Windows: no app
    /// answered, or the ones that did are gone and what is left in its job never got one), or it
    /// did no work over its last look ([`STOP_LOOK`]) and lives on. A run at work winds down for
    /// as long as it takes: its exit write is what the next start is warm from.
    fn kill_now(&mut self, r: &Run) -> bool {
        if !self.termed && !self.apps.iter().any(|&(pid, start)| os::same(pid, start)) {
            return true;
        }
        let now = Instant::now();
        let Some(done) = os::activity(r.pid, r.start, r.unit.as_deref(), &r.exit_file.with_extension("usage"), &r.hold) else { return true };
        match self.look {
            None => self.look = Some((now, done)),
            Some((since, from)) if now.saturating_duration_since(since) >= STOP_LOOK => {
                if !busy(from, done, now.saturating_duration_since(since)) {
                    return true;
                }
                self.look = Some((now, done));
            }
            Some(_) => {}
        }
        false
    }
}

/// Asks a run to exit the way it takes an ask: Unix TERMs its group; Windows has no TERM for a
/// process tree, so each app in its job with a control endpoint (announced in `registry`, where
/// its runs announce) is sent `quit`, the same exit as a TERM on Unix (its exit write runs).
fn ask_exit(r: &Run, registry: &Path) -> Stopping {
    let apps = if cfg!(windows) {
        makepad_remote::discovery::apps_in(registry)
            .into_iter()
            .filter(|a| os::in_group(a.pid, r.pid, &r.hold) && quit_app(a))
            .filter_map(|a| os::proc_start(a.pid).map(|s| (a.pid, s)))
            .collect()
    } else {
        Vec::new()
    };
    Stopping { termed: os::term_group(r.pid, &r.hold), apps, look: None, killed: false }
}

/// `quit` over the app's control endpoint; whether it took it (its answer is no error).
fn quit_app(a: &makepad_remote::discovery::App) -> bool {
    let client = makepad_remote::link::Client::new(makepad_remote::link::Target::of(a), "devhub");
    client.set_timeout(Some(QUIT_ANSWER));
    // (force: the run ends either way; a click on the box refused the quit and got it KILLed, no exit write)
    let first = client.cmd("quit force=1").ok().and_then(|mut l| l.next()).and_then(|l| l.ok()).unwrap_or_default();
    !first.is_empty() && !first.starts_with("{\"err\"")
}

/// Asks the runs to exit ([`ask_exit`]) and waits while they do; a run is KILLed only on its state
/// ([`Stopping::kill_now`]), never on a timer: a 10 s grace KILLed a cold JIT's exit write, and
/// Windows ended the job at once (no startup manifest was ever stored on the Windows node: every
/// Fab start there was cold, 6.5 s to main instead of 2.8, 11-04). `at_once`: no ask (a worker run
/// the hub drops). Then their scopes stop. (stopped leaders, survivors)
fn stop_runs(runs: &[Arc<Run>], registry: &Path, at_once: bool) -> (Vec<u32>, Vec<u32>) {
    let alive: Vec<&Arc<Run>> = runs.iter().filter(|r| r.start != 0 && os::group_alive(r.pid, r.start, &r.hold)).collect();
    let mut stopping: Vec<Stopping> = alive.iter().map(|r| if at_once { Stopping { termed: false, apps: Vec::new(), look: None, killed: false } } else { ask_exit(r, registry) }).collect();
    loop {
        let mut left = false;
        for (r, s) in alive.iter().zip(stopping.iter_mut()) {
            if s.killed || !os::group_alive(r.pid, r.start, &r.hold) {
                continue;
            }
            if s.kill_now(r) {
                r.sample();
                os::signal_group(r.pid, true, &r.hold);
                s.killed = true;
            } else {
                left = true;
            }
        }
        if !left {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for (r, u) in runs.iter().filter_map(|r| r.unit.as_ref().map(|u| (r, u))) {
        r.sample();
        let _ = makepad_remote::group::command("systemctl").args(["--user", "stop", &format!("{u}.scope")]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) && alive.iter().any(|r| os::group_alive(r.pid, r.start, &r.hold)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let leak = alive.iter().filter(|r| os::group_alive(r.pid, r.start, &r.hold) || os::same(r.pid, r.start)).map(|r| r.pid).collect();
    (alive.iter().map(|r| r.pid).collect(), leak)
}

/// Stops what the lease's runs started and their groups did not take with them (and what the runs
/// of a hub running in it started, [`LEASE_OUTER_ENV`]): TERM, looking again while the grace
/// lasts (a WM on its way out may still start an app), then KILL. (stopped, left)
fn stop_strays(mark: &str, grace: Duration) -> (Vec<u32>, Vec<u32>) {
    let (marker, outer) = (format!("{LEASE_ENV}={mark}"), format!("{LEASE_OUTER_ENV}={mark}"));
    let me = std::process::id();
    let find = || {
        let mut found: Vec<u32> = os::with_env(&marker).into_iter().chain(os::with_env(&outer)).filter(|p| *p != me).collect();
        found.sort_unstable();
        found.dedup();
        found
    };
    let mut stopped: Vec<u32> = Vec::new();
    let t = Instant::now();
    loop {
        let now = find();
        if now.is_empty() {
            return (stopped, Vec::new());
        }
        if t.elapsed() >= grace {
            break;
        }
        for &p in &now {
            if !stopped.contains(&p) {
                stopped.push(p);
                os::signal_one(p, false);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let t = Instant::now();
    loop {
        let now = find();
        if now.is_empty() || t.elapsed() >= Duration::from_secs(2) {
            return (stopped, now);
        }
        for &p in &now {
            if !stopped.contains(&p) {
                stopped.push(p);
            }
            os::signal_one(p, true);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// An inspect lease's end: its breakpoint groups and probes removed, parked hits let go, a
/// pause resumed, over the JIT's own socket. The commands that answered.
pub fn inspect_release(pid: u32, breaks: &[String], probes: &[String], paused: bool) -> Vec<String> {
    let mut cmds = inspect_commands(breaks, probes, paused);
    if cmds.is_empty() {
        return Vec::new();
    }
    let Some(app) = makepad_remote::discovery::by_pid(pid) else {
        return if os::proc_start(pid).is_some() { vec![format!("pid {pid}: no JIT (failed)")] } else { Vec::new() };
    };
    let client = makepad_remote::link::Client::new(makepad_remote::link::Target::of(&app), "devhub");
    client.set_timeout(Some(Duration::from_secs(3)));
    for c in cmds.iter_mut() {
        let first = client.cmd(c).ok().and_then(|mut l| l.next()).and_then(|l| l.ok()).unwrap_or_default();
        if first.starts_with("{\"err\"") || first.is_empty() {
            c.push_str(" (failed)");
        }
    }
    cmds
}

/// The JIT's own command words: `rm G<n>` per breakpoint group, `probe rm P<n>` per probe,
/// `continue all` after breakpoints, `resume` after a pause.
pub fn inspect_commands(breaks: &[String], probes: &[String], paused: bool) -> Vec<String> {
    let ok = |s: &str, p: char| s.strip_prefix(p).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    let mut cmds: Vec<String> = breaks.iter().filter(|b| ok(b, 'G')).map(|b| format!("rm {b}")).collect();
    cmds.extend(probes.iter().filter(|p| ok(p, 'P')).map(|p| format!("probe rm {p}")));
    if !breaks.is_empty() {
        cmds.push("continue all".into());
    }
    if paused {
        cmds.push("resume".into());
    }
    cmds
}

/// Standard base64 (padding optional); None on a stray character.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Registers the lease services on the node's master endpoint.
/// This process's executor (the node's), for sync's launches.
static EXEC: std::sync::OnceLock<Arc<Exec>> = std::sync::OnceLock::new();

/// Whether this node takes writes from the network (launches, stops, WM restarts): its dev
/// switch as the master last pushed it, or its role's default. A process with no node: no.
pub fn writes_allowed() -> bool {
    EXEC.get().is_some_and(|x| x.dev.load(Ordering::Acquire))
}

/// A person uses this node now ([`super::user`]): it takes no work until they leave.
pub fn user_present() -> bool {
    EXEC.get().is_some_and(|x| x.user.lock().unwrap().present())
}

/// A person used this node (input its WM took from a device or a Desk Link viewer) in the last
/// `ms`, as far as this hub has seen.
pub fn person_within(ms: u64) -> bool {
    EXEC.get().is_some_and(|x| x.user.lock().unwrap().last_input_ms().is_some_and(|at| crate::proto::wall_ms().saturating_sub(at) < ms))
}

/// A Desk Link viewer attached that may be watching ([`super::user::UserTrack::watched`]).
pub fn user_watching() -> bool {
    EXEC.get().is_some_and(|x| x.user.lock().unwrap().watched())
}

/// Runs `f` while no lease starts here: a WM restart's last look at what it would end and the
/// restart itself see the same leases (a live session that started after the wait's look, while
/// the hub built, went down with the WM, 11-03).
pub fn with_no_lease_starting<T>(f: impl FnOnce() -> T) -> T {
    let Some(x) = EXEC.get() else { return f() };
    let _starting = x.starting.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// What keeps a user-facing service of this node (the WM, the AI hub) from restarting now: a
/// person at the node (input within `user_idle_min`) or unsaved work in its apps. Empty: it may
/// restart. The hub itself restarts regardless (invisible to the user).
pub fn user_blockers() -> Vec<String> {
    let mut why = Vec::new();
    if user_present() {
        why.push("a user is at the node (it restarts once they are idle: lease.conf user_idle_min)".to_string());
    }
    #[cfg(all(unix, feature = "checkout"))]
    why.extend(makepad_app_registry::machine::unsaved_work().into_iter().map(|(pid, app, what)| format!("unsaved work in {app} (pid {pid}): {what}")));
    why
}

/// The user flips this node's dev switch on the node itself (tray / task view): shown at once,
/// and asked of the master with the next caps answer, which saves it in nodes.conf and pushes
/// the policy (the master stays the one place the switch is kept).
pub fn request_dev(on: bool) {
    if let Some(x) = EXEC.get() {
        x.dev.store(on, Ordering::Release);
        x.dev_req.store(if on { 1 } else { 2 }, Ordering::Release);
    }
}

/// The leases here whose work a WM restart would cut: a landing's (gate), and those on the WM:
/// a screen or CI lease, one given the WM session (`needs.gui`). Their lines, for the WM
/// restart's wait. A headless lease (live test sessions among them) runs on through it: a node's
/// old-wire WM waited on those while their apps could not have used it (11-02).
pub fn on_the_wm() -> Vec<String> {
    let Some(x) = EXEC.get() else { return Vec::new() };
    x.leases.lock().unwrap().values().filter_map(|l| {
        let what = if l.kind == Kind::Gate || l.lane.starts_with("land") {
            "a landing"
        } else if !uses_wm(l) {
            return None;
        } else if l.runs.iter().any(|r| r.detachable && r.live()) {
            "a live session"
        } else {
            "its windows"
        };
        Some(format!("{} lease {} of {} ({what})", l.kind.as_str(), l.id, l.lane))
    }).collect()
}

fn uses_wm(l: &Lease) -> bool {
    matches!(l.kind, Kind::Screen | Kind::Ci) || l.env.iter().any(|(k, _)| k == "MAKEPAD_WM_SOCKET")
}

/// `lease-jits <id>`: the JITs running in the lease's process groups (Windows: its jobs; a
/// stalled lease's master asks each to capture its state).
fn lease_jits_of(x: &Exec, id: &str) -> String {
    // no process walks under the lease table lock
    let Some(runs) = x.leases.lock().unwrap().get(id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    // (a hub built without checkouts keeps no JIT registry: it runs no JITs)
    #[cfg(feature = "checkout")]
    let jits: Vec<u32> = crate::sync::jits::list().into_iter().map(|j| j.pid).collect();
    #[cfg(not(feature = "checkout"))]
    let jits: Vec<u32> = Vec::new();
    let pids: Vec<String> = jits.into_iter().filter(|pid| runs.iter().any(|r| r.pid != 0 && os::in_group(*pid, r.pid, &r.hold))).map(|pid| pid.to_string()).collect();
    format!("{{\"pids\":[{}]}}", pids.join(","))
}

/// `lease-kill <id> <pid>`: KILL one process of the lease's runs (a live session's JIT restart).
/// An overlay lease runs only the JIT's cargo invocation, so a `kill` run there was refused and
/// the session stayed "recovering" (11-02).
fn lease_kill(x: &Exec, rest: &str) -> String {
    let mut w = rest.split_whitespace();
    let (Some(id), Some(pid)) = (w.next(), w.next().and_then(|p| p.parse::<u32>().ok())) else { return err("lease-kill <id> <pid>") };
    let Some(runs) = x.leases.lock().unwrap().get(id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    if !runs.iter().any(|r| r.pid != 0 && (r.pid == pid || os::in_group(pid, r.pid, &r.hold))) {
        return err(format!("pid {pid} is no process of lease {id} on {}", x.machine));
    }
    os::signal_one(pid, true);
    format!("{{\"killed\":{pid}}}")
}

/// `lease-why <id>`: what a lease's processes do now and how far its last run got, on that lease:
/// a second lease on its tree waits for this very one (three capture leases did, 11-05).
fn lease_why(x: &Exec, id: &str) -> String {
    // no process walks under the lease table lock
    let Some(runs) = x.leases.lock().unwrap().get(id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    let (mark, me) = (lease_mark(&x.state, id), std::process::id());
    let mut pids: Vec<u32> = runs.iter().filter(|r| r.pid != 0).flat_map(|r| os::group_pids(r.pid, r.start, &r.hold))
        .chain(os::with_env(&format!("{LEASE_ENV}={mark}")))
        .chain(os::with_env(&format!("{LEASE_OUTER_ENV}={mark}")))
        .filter(|p| *p != me)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    let got = runs.last().map_or("no run yet\n".to_string(), |r| got_to(r, WHY_LINES));
    format!("{{\"text\":{}}}", json_str(&format!("{}{got}", os::processes_view(&pids))))
}

/// Whether `pid` (a JIT app) runs in a headless lease here, which a WM restart leaves alone: in
/// the process group of a run of a lease not on the WM ([`on_the_wm`]), with no WM socket in its
/// environment. A launch adopted into a lease, or an app a shell started, gets no such pass.
pub fn in_headless_lease(pid: u32) -> bool {
    let Some(x) = EXEC.get() else { return false };
    // no process walks under the lease table lock
    let runs: Vec<Arc<Run>> = x.leases.lock().unwrap().values().filter(|l| !uses_wm(l)).flat_map(|l| l.runs.iter().cloned()).collect();
    runs.iter().any(|r| r.pid != 0 && os::in_group(pid, r.pid, &r.hold)) && !wm_in_env(pid)
}

/// An unreadable environment counts as on the WM.
#[cfg(target_os = "linux")]
fn wm_in_env(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/environ")).map_or(true, |env| env.split(|b| *b == 0).any(|v| v.starts_with(b"MAKEPAD_WM_SOCKET=")))
}

#[cfg(not(target_os = "linux"))]
fn wm_in_env(_pid: u32) -> bool {
    true
}

/// The leases live on this node now: (id, lane, kind), for its own status view.
pub fn held() -> Vec<(String, String, &'static str)> {
    EXEC.get().map_or_else(Vec::new, |x| x.leases.lock().unwrap().values().map(|l| (l.id.clone(), l.lane.clone(), l.kind.as_str())).collect())
}

/// Puts a process started for lease `id` (a sync launch) under that lease: its end stops it.
pub fn adopt(id: &str, pid: u32, log: PathBuf) -> Result<(), String> {
    let x = EXEC.get().ok_or("this process runs no lease executor")?;
    let mut leases = x.leases.lock().unwrap();
    let l = leases.get_mut(id).ok_or_else(|| format!("no live lease {id} on {}", x.machine))?;
    if !l.kind.writes() {
        return Err(format!("{id} is an {} lease: launches need a write lease", l.kind.as_str()));
    }
    let n = l.runs.iter().map(|r| r.n).max().unwrap_or(0) + 1;
    l.runs.push(Arc::new(Run { request: String::new(), started_ms: crate::proto::wall_ms(), spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit: None, log, exit: Mutex::new(None), ours: false, detachable: false, hold: os::Hold::none(), exit_file: exit_file(&x.state, id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) }));
    x.save(l);
    Ok(())
}

/// Whether lease `id` is live here and may launch.
/// The lane lease `id` belongs to.
pub fn lease_lane(id: &str) -> Option<String> {
    EXEC.get().and_then(|x| x.leases.lock().unwrap().get(id).map(|l| l.lane.clone()))
}

/// What this hub stopped in the last minute: (by what, the dir or socket of what it stopped, the
/// processes it stopped that no dir names, when). A JIT whose socket is under that path, or that
/// is one of those processes, and ends then was stopped, not crashed: a lease's end stops what
/// runs under its scratch dir (a CI box's calculator, hosted by a CI smoke's WM, "crashed" as
/// its lease ended, 11-03) and on Windows what is in its jobs (a JIT there announces a loopback
/// port, no socket under the dir), a prewarm's stop the JIT it started (each landing's prewarm of
/// stage "crashed" on three Linux nodes, 11-04).
static STOPPED: Mutex<Vec<(String, PathBuf, Vec<u32>, u64)>> = Mutex::new(Vec::new());
const STOPPED_FOR_MS: u64 = 60_000;

fn note_stopping(id: &str, tmp: &Path, pids: Vec<u32>) {
    note_stopped(&format!("with its lease {id}"), tmp, pids);
}

/// The processes in the runs' jobs (Windows: what a lease's end stops beside its scratch dir).
#[cfg(windows)]
fn job_pids(runs: &[Arc<Run>]) -> Vec<u32> {
    runs.iter().flat_map(|r| os::job_pids(&r.hold)).collect()
}

/// Elsewhere a JIT's socket lies under its lease's scratch dir: the dir names them all.
#[cfg(not(windows))]
fn job_pids(_runs: &[Arc<Run>]) -> Vec<u32> {
    Vec::new()
}

/// Says that `by` (`by prewarm`, `with its lease L1`) stops the JIT whose socket is `path`, or
/// every JIT whose socket is under it, and the processes `pids`.
pub fn note_stopped(by: &str, path: &Path, pids: Vec<u32>) {
    let now = crate::proto::wall_ms();
    let mut stopped = STOPPED.lock().unwrap();
    stopped.retain(|(_, _, _, at)| now.saturating_sub(*at) < STOPPED_FOR_MS);
    stopped.push((by.to_string(), path.to_path_buf(), pids, now));
}

/// What stopped the process `pid` whose socket or log is `path` ([`note_stopped`]), when it
/// ended in the last minute: `with its lease L1`, `by prewarm`.
pub fn stopped_with(pid: u32, path: &str) -> Option<String> {
    let now = crate::proto::wall_ms();
    let stopped = STOPPED.lock().unwrap();
    stopped.iter().rev()
        .find(|(_, at_path, pids, at)| now.saturating_sub(*at) < STOPPED_FOR_MS && ((!path.is_empty() && Path::new(path).starts_with(at_path)) || pids.contains(&pid)))
        .map(|(by, ..)| by.clone())
}

/// Whether `path` (a JIT's socket) lies in the directory of a live test or CI lease here.
pub fn in_test_lease(path: &str) -> bool {
    let Some(x) = EXEC.get() else { return false };
    let leases = x.leases.lock().unwrap();
    Path::new(path).components().filter_map(|c| c.as_os_str().to_str()).any(|c| leases.get(c).is_some_and(|l| matches!(l.kind, Kind::Test | Kind::Ci)))
}

pub fn lease_live(id: &str) -> bool {
    EXEC.get().is_some_and(|x| x.leases.lock().unwrap().get(id).is_some_and(|l| l.kind.writes()))
}

/// The outage its WM recorded before this hub ran (`makepad_devhub_conf::TROUBLE_FILE` beside the
/// node's conf, which the WM writes on each failed start or exit): its report line, and the file
/// is gone. A record at the hub's start would reach no master (the master's watch follows from
/// when it reconnects), so the line goes out in the next caps answer, which the master says.
fn take_trouble(path: &Path, machine: &str, now_ms: u64) -> String {
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    let _ = std::fs::remove_file(path);
    makepad_devhub_conf::Trouble::parse(&text).map_or_else(|| format!("devhub on {machine} could not run for a while: its WM's record {} is unreadable", path.display()), |t| t.report(machine, now_ms))
}

pub fn register(ep: &Endpoint, ctx: &crate::sync::NodeCtx) -> Arc<Exec> {
    let conf = LeaseConf::load();
    #[cfg(feature = "checkout")]
    let opts = Opts { tree: Some(super::tree::root(&conf.work)), ..Opts::default() };
    #[cfg(not(feature = "checkout"))]
    let opts = Opts::default();
    let x = Exec::new(&ctx.machine, conf, crate::proto::home().join("leases"), opts, ctx.publish.clone());
    if let Some(conf) = std::env::var_os(makepad_devhub_conf::NODE_CONF_ENV).filter(|p| !p.is_empty()) {
        *x.trouble.lock().unwrap() = take_trouble(&makepad_devhub_conf::trouble_path(Path::new(&conf)), &ctx.machine, crate::proto::wall_ms());
    }
    let _ = EXEC.set(x.clone());
    // the person at the box: our WM's answer every 2 s (a node without one senses nothing)
    let w = Arc::downgrade(&x);
    let _ = std::thread::Builder::new().name("devhub-user".into()).spawn(move || loop {
        let Some(x) = w.upgrade() else { return };
        #[cfg(all(unix, feature = "checkout"))]
        let answer = makepad_app_registry::machine::wm_socket().ok_or_else(|| "no WM".to_string()).and_then(|p| makepad_app_registry::machine::request_activity(&p));
        #[cfg(not(all(unix, feature = "checkout")))]
        let answer: Result<String, String> = Err("no WM".into());
        x.sense_user(answer.as_deref().map_err(|e| e.clone()));
        drop(x);
        // no WM, or one from before `activity` (it logs each line it does not know): ask seldom
        std::thread::sleep(if answer.is_ok() { super::user::POLL } else { super::user::POLL * 15 });
    });
    for &word in WORDS {
        let e = x.clone();
        ep.register(word, 1, lines::service(word, Arc::new(move |line, out| out.extend(e.handle(word, line)))));
    }
    x
}

/// The hub build running on this box (since when, wall ms) and the last one that built, with
/// its measure ([`build_hub`]).
static HUB_BUILD: Mutex<(Option<u64>, Option<super::HubMeasure>)> = Mutex::new((None, None));

/// Whether the hub builds itself now (its caps say so: the master charges a running build like a
/// build lease of its class's measured size).
pub(crate) fn hub_building() -> bool {
    HUB_BUILD.lock().unwrap_or_else(|e| e.into_inner()).0.is_some()
}

/// How often a hub build's processes are looked at for the RAM they hold together.
#[cfg(feature = "checkout")]
const HUB_BUILD_LOOK: Duration = Duration::from_secs(5);

/// `cargo build --release -p makepad-app-devhub` in the deployment `root`: the build of what its
/// keeper's `cargo run` starts (the Mac's and Windows' start scripts run exactly this; the WM's
/// start the same package, profile and features), in this hub's environment, which is the
/// keeper's (`cargo run` passed it on), so that start compiles nothing. Its first error if not.
#[cfg(feature = "checkout")]
pub(crate) fn build_hub(root: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let aside = image_aside();
    let mut cmd = os::niced(makepad_app_registry::cargo_bin().as_os_str());
    // (rustup counts nested proxies in RUST_RECURSION_COUNT and refuses past 20)
    cmd.args(["build", "--release", "-p", "makepad-app-devhub"]).current_dir(root).env_remove("RUST_RECURSION_COUNT").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
    let since = crate::proto::wall_ms();
    HUB_BUILD.lock().unwrap_or_else(|e| e.into_inner()).0 = Some(since);
    let measured = measured_build(&mut cmd);
    let end = crate::proto::wall_ms();
    {
        let mut s = HUB_BUILD.lock().unwrap_or_else(|e| e.into_inner());
        s.0 = None;
        if let Ok((o, cpu_s, peak)) = &measured {
            if o.status.success() {
                s.1 = Some(super::HubMeasure { end_ms: end, wall_ms: end.saturating_sub(since), cpu_s: *cpu_s, peak_gb: peak.map(|b| b as f64 / (1u64 << 30) as f64) });
            }
        }
    }
    let why = match measured {
        Ok((o, ..)) if o.status.success() => return Ok(()),
        Ok((o, ..)) => crate::orch::roll::build_failure(&o),
        Err(e) => format!("cargo: {e}"),
    };
    #[cfg(windows)]
    image_back(aside);
    Err(why)
}

/// Runs a build in a session of its own: its output, the CPU seconds of all its processes, and the
/// most RAM they held at once (looked at every [`HUB_BUILD_LOOK`]; at least the largest one
/// process held). Where the OS says neither (Windows), None.
#[cfg(feature = "checkout")]
fn measured_build(cmd: &mut std::process::Command) -> std::io::Result<(std::process::Output, Option<f64>, Option<u64>)> {
    let (mut child, _hold) = os::lease_spawn(cmd, None)?;
    let pid = child.id();
    let start = os::proc_start(pid).unwrap_or(0);
    let stderr = child.stderr.take();
    let read = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(mut e) = stderr { let _ = std::io::Read::read_to_end(&mut e, &mut text); }
        text
    });
    let (done, ended) = std::sync::mpsc::channel::<()>();
    let look = std::thread::spawn(move || {
        let mut peak = None::<u64>;
        while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = ended.recv_timeout(HUB_BUILD_LOOK) {
            if let Some(now) = os::groups_rss(&[(pid, start)]) { peak = Some(peak.map_or(now, |p| p.max(now))); }
        }
        peak
    });
    let waited = os::wait_usage(&mut child);
    drop(done);
    let looked = look.join().unwrap_or(None);
    let stderr = read.join().unwrap_or_default();
    let (status, cpu_s, largest) = waited?;
    let peak = match (looked, largest) { (Some(a), Some(b)) => Some(a.max(b)), (a, b) => a.or(b) };
    Ok((std::process::Output { status, stdout: Vec::new(), stderr }, cpu_s, peak))
}

/// Windows will not let a build replace a running image's file (its link fails), but renames it:
/// the hub's binary names (cargo's uplifted copy and its `deps` file, hard links of one file)
/// move aside before the build, and back when it fails. The next hub deletes them ([`drop_aside`]).
/// Runs started meanwhile have their wrapper run the image aside ([`os::run_image`]).
#[cfg(all(windows, feature = "checkout"))]
fn image_aside() -> Vec<(PathBuf, PathBuf)> {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let Ok(exe) = std::env::current_exe() else { return Vec::new() };
    let mut names = vec![exe.clone()];
    if let (Some(dir), Some(stem)) = (exe.parent(), exe.file_stem().and_then(|s| s.to_str())) {
        let prefix = format!("{}-", stem.replace('-', "_"));
        names.extend(std::fs::read_dir(dir.join("deps")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "exe") && p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix))));
    }
    let tag = format!("exe.{ASIDE}{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed));
    let moved: Vec<(PathBuf, PathBuf)> = names.into_iter().filter_map(|p| {
        let to = p.with_extension(&tag);
        std::fs::rename(&p, &to).ok().map(|_| (p, to))
    }).collect();
    if let Some((_, to)) = moved.iter().find(|(p, _)| *p == exe) {
        os::image_moved(Some(to.clone()));
    }
    moved
}

#[cfg(all(windows, feature = "checkout"))]
const ASIDE: &str = "aside-";

#[cfg(all(windows, feature = "checkout"))]
fn image_back(moved: Vec<(PathBuf, PathBuf)>) {
    for (name, aside) in moved {
        if !name.exists() && std::fs::rename(&aside, &name).is_ok() && std::env::current_exe().is_ok_and(|e| e == name) {
            os::image_moved(None);
        }
    }
}

/// At start: the images an earlier hub moved aside ([`image_aside`]); the one running cannot go,
/// nor one a wrapper of a run still running runs (a later start deletes it).
#[cfg(all(windows, feature = "checkout"))]
fn drop_aside() {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else { return };
    for d in [dir.clone(), dir.join("deps")] {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            if e.file_name().to_str().is_some_and(|n| n.contains(&format!(".exe.{ASIDE}"))) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A timed lease's box report: the outside CPU's mean and peak over the looks, and the busiest
    /// outside processes by their mean.
    #[cfg(unix)]
    #[test]
    fn a_timed_box_report_names_what_shared_it() {
        let mut n = Noise::default();
        n.add(20.0, &[("mds_stores".into(), 180.0), ("XprotectService".into(), 40.0)]);
        n.add(27.0, &[("mds_stores".into(), 60.0)]);
        let v = makepad_strict_json::parse(n.json().as_bytes()).unwrap();
        assert_eq!(super::super::master::box_line(&v), "load1 max 27.0; outside it 140% of a core (max 220%): mds_stores 120%, XprotectService 20%");
    }

    #[test]
    fn a_jit_under_a_stopped_leases_scratch_dir_or_in_its_job_was_stopped_with_it() {
        note_stopping("L248709", Path::new("/h/devhub/tmp/L248709"), vec![4242]);
        assert_eq!(stopped_with(1, "/h/devhub/tmp/L248709/makepad-remote-2703620.sock").as_deref(), Some("with its lease L248709"));
        assert_eq!(stopped_with(1, "/h/devhub/tmp/L2487090/makepad-remote-1.sock"), None, "another lease's dir");
        assert_eq!(stopped_with(1, "/run/user/1000/makepad-remote-5.sock"), None, "no lease: a crash stays a crash");
        assert_eq!(stopped_with(1, ""), None);
        // a Windows JIT: a loopback port, no socket under the dir; its job says it was the lease's
        assert_eq!(stopped_with(4242, "tcp:127.0.0.1:64033").as_deref(), Some("with its lease L248709"));
        assert_eq!(stopped_with(4243, "tcp:127.0.0.1:64034"), None, "not in its job");
    }

    /// A Windows run's processes are its job's: the run's leader and what that started (a JIT
    /// under cargo or a shell). A lease's end records them as stopped by it, a stall's capture
    /// finds its JITs among them, and a kill of one (`lease-kill`) ends that one, not the job.
    #[cfg(windows)]
    #[test]
    fn a_windows_runs_job_holds_what_it_started_and_one_of_them_ends_alone() {
        let mut cmd = std::process::Command::new(SLEEP[0]);
        cmd.args(&SLEEP[1..]);
        let (mut child, hold) = os::lease_spawn(&mut cmd, None).unwrap();
        // cmd's ping joins the job as cmd starts it
        let pids = loop {
            let pids = os::job_pids(&hold);
            if pids.len() >= 2 {
                break pids;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(pids.contains(&child.id()), "{pids:?}");
        let ping = *pids.iter().find(|p| **p != child.id()).unwrap();
        assert!(os::in_group(ping, child.id(), &hold), "a member of the run's job is one of its processes");
        assert!(!os::in_group(std::process::id(), child.id(), &hold));
        // the leader alone ends: its ping stays in the job
        let leader = child.id();
        os::signal_one(leader, true);
        let _ = child.wait();
        while os::job_pids(&hold).contains(&leader) {
            std::thread::sleep(Duration::from_millis(20));
        }
        // (a console host of cmd's may be in it beside its ping)
        let left = os::job_pids(&hold);
        assert!(!left.is_empty() && !left.contains(&leader), "{left:?}");
        os::signal_group(ping, true, &hold);
    }

    // what the tests run: a line out, the env, a failure, a long sleeper with a child (Windows)
    #[cfg(unix)]
    const ECHO_HI: &[&str] = &["/bin/echo", "hi"];
    #[cfg(unix)]
    const ENV: &[&str] = &["/usr/bin/env"];
    #[cfg(unix)]
    const FAIL: &[&str] = &["false"];
    #[cfg(unix)]
    const SLEEP: &[&str] = &["/bin/sleep", "300"];
    #[cfg(unix)]
    const SLEEP_LABEL: &str = "sleep 300";
    #[cfg(windows)]
    const ECHO_HI: &[&str] = &["cmd", "/c", "echo hi"];
    #[cfg(windows)]
    const ENV: &[&str] = &["cmd", "/c", "set"];
    #[cfg(windows)]
    const FAIL: &[&str] = &["cmd", "/c", "exit 1"];
    #[cfg(windows)]
    const SLEEP: &[&str] = &["cmd", "/c", "ping -n 300 127.0.0.1 >nul"];
    #[cfg(windows)]
    const SLEEP_LABEL: &str = "cmd.exe";

    fn b64(argv: &[&str]) -> String {
        makepad_network::digest::base64_encode(argv.join("\0").as_bytes())
    }

    fn field(line: &str, k: &str) -> Option<makepad_strict_json::Value> {
        makepad_strict_json::parse(line.as_bytes()).ok()?.get(k).cloned()
    }

    /// A lease-end answer's cleanup receipt: the answer less the load measured at it.
    fn receipt(answer: Vec<String>) -> String {
        let a = &answer[0];
        let load = field(a, "load").expect("an ended lease's answer carries the box's load");
        assert!(load.get("cpu_pct").and_then(super::super::num).is_some_and(|c| (0.0..=100.0).contains(&c)) && load.get("ram_avail_gb").and_then(super::super::num).is_some_and(|r| r > 0.0), "{a}");
        a[..a.find(",\"load\":").unwrap()].to_string() + "}"
    }

    /// The load joins an ended lease's receipt only; a pending answer or an error goes as it is.
    #[test]
    fn a_lease_end_answers_the_box_load_after_its_teardown() {
        let ended = "{\"ended\":\"L1\",\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[]}".to_string();
        assert_eq!(with_load(ended.clone(), || Some((3.04, 50.256))), "{\"ended\":\"L1\",\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[],\"load\":{\"cpu_pct\":3.0,\"ram_avail_gb\":50.26}}");
        assert_eq!(with_load(ended.clone(), || None), ended);
        let pending = "{\"pending\":\"lease cleanup in progress\"}".to_string();
        assert_eq!(with_load(pending.clone(), || panic!("no measure for an answer without an end")), pending);
        let (cpu, avail) = super::super::caps::load_now().expect("the box's load");
        assert!((0.0..=100.0).contains(&cpu) && avail > 0.0, "{cpu} {avail}");
    }

    /// A macOS timed run first runs its JIT binary once with `--version`, then its command once:
    /// the command's own output and exit; a JIT binary that fails there is said, and the run goes on.
    #[cfg(unix)]
    #[test]
    fn a_timed_run_starts_its_jit_binary_once_first_and_its_command_once() {
        let base = std::env::temp_dir().join(format!("devhub-exec-warmup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (count, mark, jit) = (base.join("count"), base.join("warmup.mark"), base.join("makepad.jit-x"));
        let run = |jit_exit: u8, script: String| {
            std::fs::write(&jit, format!("#!/bin/sh\necho \"$@\" >> {}.jit\nexit {jit_exit}\n", count.display())).unwrap();
            std::process::Command::new("chmod").arg("+x").arg(&jit).status().unwrap();
            let argv = super::warmed_up(vec!["sh".into(), "-c".into(), script], &jit, &mark);
            let out = std::process::Command::new(&argv[0]).args(&argv[1..]).output().unwrap();
            (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned())
        };
        let (code, out) = run(0, format!("echo x >> {0}; wc -l < {0}; exit 3", count.display()));
        assert!(code == Some(3) && out.contains("makepad.jit-x ran once first, untimed") && out.trim_end().ends_with('1'), "{code:?} {out}");
        assert_eq!(std::fs::read_to_string(base.join("count.jit")).unwrap(), "--version\n");
        assert!(mark.is_file());
        let (code, out) = run(5, format!("echo x >> {0}; wc -l < {0}", count.display()));
        assert!(code == Some(0) && out.contains("makepad.jit-x --version failed (exit 5) before the timed run") && out.trim_end().ends_with('2'), "{code:?} {out}");
        let _ = std::fs::remove_dir_all(&base);
    }

    fn test_exec(name: &str, opts: Opts) -> (TestHub, PathBuf, Arc<Mutex<Vec<String>>>) {
        let base = std::env::temp_dir().join(format!("devhub-exec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let conf = LeaseConf::parse(&format!("work = {}\n", base.join("devhub").display()), &base);
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let x = Exec::new("t", conf, base.join("leases"), opts, Arc::new(move |r: crate::proto::Record| ev.lock().unwrap().push(r.line)));
        (TestHub(Some(x)), base, events)
    }

    /// A landed lane's dirs go without holding up any lease but its own: a clean that sized and
    /// deleted under the start lock kept every start and the hub's restart waiting (11-04).
    #[test]
    fn a_lane_being_cleaned_holds_up_only_its_own_starts() {
        let (x, base, _) = test_exec("clean-lane", Opts::default());
        let start = |id: &str, lane: &str| format!("{{\"id\":\"{id}\",\"lane\":\"{lane}\",\"kind\":\"test\",\"rev\":null}}");
        let ok = |l: &[String]| !l[0].starts_with("{\"err\"");
        x.cleaning.0.lock().unwrap().insert("gone".to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        let (y, ask) = (Arc::clone(&*x), start("G1", "gone"));
        let waits = std::thread::spawn(move || tx.send(y.handle("lease-start", &ask)));
        assert!(ok(&x.handle("lease-start", &start("O1", "other"))), "another lane starts");
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "its own lane waits");
        x.cleaning.0.lock().unwrap().remove("gone");
        x.cleaning.1.notify_all();
        assert!(ok(&rx.recv().unwrap()), "and starts once it has left");
        waits.join().unwrap().unwrap();

        let work = x.conf().work.clone();
        let (state, target) = (work.join("dev/lane/gone"), work.join("target/lane-gone"));
        for d in [&state, &target] {
            std::fs::create_dir_all(d.join("deep")).unwrap();
            std::fs::write(d.join("deep/file"), vec![0u8; 3 << 20]).unwrap();
        }
        let clean = || x.handle("disk", "disk clean gone").remove(0);
        assert!(clean().contains("holds live lease G1"), "a lane in use stays");
        assert!(state.is_dir() && x.cleaning.0.lock().unwrap().is_empty());
        x.handle("lease-end", "G1");
        let answer = clean();
        assert!(answer.contains("\"freed_mb\":6") && answer.contains("lane-gone"), "{answer}");
        assert!(!state.exists() && !target.exists());
        assert_eq!(std::fs::read_dir(work.join("trash")).unwrap().count(), 0, "what left is deleted");
        assert!(x.cleaning.0.lock().unwrap().is_empty());
        assert!(ok(&x.handle("lease-start", &start("G2", "gone"))), "its next lease starts");
        let _ = base;
    }

    /// A test's hub. Its leases end when the test does, whichever way it ends (an assert that
    /// fails, a panic): no run a test started outlives it (a beat test that failed before its
    /// leases' end left a busy loop in its scope on a box for four hours, 11-04). A test that
    /// passes also finds nothing of them left: no process with one of its leases' marks, and so
    /// no scope (one goes with its last process).
    struct TestHub(Option<Arc<Exec>>);

    impl std::ops::Deref for TestHub {
        type Target = Arc<Exec>;
        fn deref(&self) -> &Arc<Exec> {
            self.0.as_ref().unwrap()
        }
    }

    impl TestHub {
        /// The hub gone as at its self-update, its runs left running: the next one, on its state.
        fn restart(mut self, opts: Opts) -> TestHub {
            let x = self.0.take().unwrap();
            let (conf, state) = (x.conf(), x.state.clone());
            drop(x);
            TestHub(Some(Exec::new("t", conf, state, opts, Arc::new(|_| {}))))
        }
    }

    impl Drop for TestHub {
        fn drop(&mut self) {
            let Some(x) = self.0.take() else { return };
            let ids: Vec<String> = x.leases.lock().unwrap().keys().cloned().collect();
            for id in &ids {
                x.handle("lease-end", &format!("{id} released"));
            }
            if std::thread::panicking() {
                return;
            }
            let me = std::process::id();
            let left: Vec<(u32, String)> = os::env_values(&format!("{LEASE_ENV}={}/", x.state.display())).into_iter().filter(|(p, _)| *p != me).collect();
            assert!(left.is_empty(), "the test left processes of its leases (pid, lease): {left:?}");
        }
    }

    fn log_until_exit(x: &Exec, id: &str, n: u32) -> (String, i64) {
        let (mut text, mut off) = (Vec::new(), 0u64);
        for _ in 0..300 {
            let a = x.handle("lease-log", &format!("{id} {n} {off}"));
            let d = b64_decode(field(&a[0], "data").unwrap().as_str().unwrap()).unwrap();
            text.extend(d);
            off = super::super::num(&field(&a[0], "off").unwrap()).unwrap() as u64;
            if let Some(c) = field(&a[0], "exit").and_then(|e| super::super::num(&e)) {
                return (String::from_utf8_lossy(&text).into_owned(), c as i64);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("run {n} of {id} did not exit");
    }

    #[cfg(all(unix, feature = "checkout"))]
    #[test]
    fn recorded_build_is_joined_and_stale_record_removed() {
        let base = std::env::temp_dir().join(format!("devhub-exec-build-pid-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join("cargo.pid");
        let mut child = std::process::Command::new("sleep").arg("1").spawn().unwrap();
        let pid = child.id();
        let start = os::proc_start(pid).unwrap();
        std::fs::write(&path, format!("{pid} {start}\n")).unwrap();
        let joins = std::cell::Cell::new(0);
        let result = join_recorded_build(&path, &|| Ok(()), &|joined| {
            assert_eq!(joined, pid, "join the recorded child");
            joins.set(joins.get() + 1);
        });
        let status = child.try_wait().unwrap();
        if status.is_none() { let _ = child.kill(); }
        let _ = child.wait();
        result.unwrap();
        assert!(status.is_some_and(|s| s.success()), "joining must wait until the child exits");
        assert_eq!(joins.get(), 1, "announce the join once");
        assert!(!path.exists(), "remove the record after the joined child exits");

        let own_pid = std::process::id();
        for (pid, start) in [(pid, start), (own_pid, os::proc_start(own_pid).unwrap().wrapping_add(1))] {
            std::fs::write(&path, format!("{pid} {start}\n")).unwrap();
            join_recorded_build(&path, &|| Ok(()), &|_| panic!("a dead or reused pid must not be joined")).unwrap();
            assert!(!path.exists(), "remove a stale record");
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn safety_run_retries_and_end_retries_keep_their_receipts() {
        let (x, base, _) = test_exec("retry", Opts::default());
        x.handle("lease-start", r#"{"id":"R1","lane":"retry","kind":"test","rev":null}"#);
        let line = format!("R1 . {} {{\"request_id\":\"run-one\"}}", b64(ECHO_HI));
        let first = x.handle("lease-run", &line);
        assert_eq!(field(&first[0], "n").and_then(|n| n.as_u64()), Some(1), "{first:?}");
        assert_eq!(x.handle("lease-run", &line), first, "reply loss must not execute again");
        log_until_exit(&x, "R1", 1);
        // Rehydrate the ledger as a replacement executor would, without running a second watchdog.
        let saved = std::fs::read_to_string(base.join("leases/R1.json")).unwrap();
        x.leases.lock().unwrap().insert("R1".into(), Lease::from_ledger(&saved).unwrap());
        assert_eq!(x.handle("lease-run", &line), first, "receipt survives executor restart");
        let second = x.handle("lease-run", &line.replace("run-one", "run-two"));
        assert_eq!(field(&second[0], "n").and_then(|n| n.as_u64()), Some(2));
        log_until_exit(&x, "R1", 2);
        let pending = "{\"pending\":\"lease cleanup in progress\"}".to_string();
        x.endings.lock().unwrap().insert("R1".into(), pending.clone());
        assert_eq!(x.handle("lease-end", "R1 released"), vec![pending]);
        x.endings.lock().unwrap().remove("R1");
        let end = receipt(x.handle("lease-end", "R1 released"));
        assert!(end.contains("\"leak\":[]"), "{end:?}");
        assert_eq!(receipt(x.handle("lease-end", "R1 released")), end);
        x.endings.lock().unwrap().clear();
        assert_eq!(receipt(x.handle("lease-end", "R1 released")), end, "durable cleanup receipt survives restart");
        // a lease the node holds no record of: swept by its mark, nothing left
        let unknown = x.handle("lease-end", "R9 released");
        assert!(unknown[0].contains("\"unknown\":true") && unknown[0].contains("\"leak\":[]"), "{unknown:?}");
        // survivors a saved receipt named are checked again: once gone, the receipt says so
        std::fs::write(base.join("leases/R8.ended"), "{\"ended\":\"R8\",\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[4294000000]}").unwrap();
        assert!(x.handle("lease-end", "R8 released")[0].contains("\"leak\":[]"));
        assert!(std::fs::read_to_string(base.join("leases/R8.ended")).unwrap().contains("\"leak\":[]"));
    }

    /// A lease-end that comes before its lease-start is done (the holder gave the lease up as the
    /// master granted it) ends what the start sets up: the lease never runs on unbeaten here.
    #[test]
    fn a_lease_ended_while_it_started_does_not_run_on() {
        let (x, base, _) = test_exec("endstart", Opts::default());
        let unknown = x.handle("lease-end", "S1 released");
        assert!(unknown[0].contains("\"unknown\":true"), "{unknown:?}");
        let a = x.handle("lease-start", r#"{"id":"S1","lane":"endstart","kind":"test","rev":null}"#);
        assert!(a[0].contains("as it started"), "{a:?}");
        assert!(!x.leases.lock().unwrap().contains_key("S1"));
        assert!(!base.join("leases/S1.json").exists(), "no ledger left to take back");
        let end = x.handle("lease-end", "S1 released");
        assert!(end[0].contains("\"ended\":\"S1\"") && !end[0].contains("\"unknown\""), "the start's own end: {end:?}");
        // a lease nothing ended starts as before
        assert!(x.handle("lease-start", r#"{"id":"S2","lane":"endstart","kind":"test","rev":null}"#)[0].contains("\"ok\":1"));
        assert!(x.leases.lock().unwrap().contains_key("S2"));
    }

    /// A live run's beat says what the lease holds, the most at one look so far (what the master
    /// charges it); a run that ends before a beat looks at it still leaves its class a peak, the
    /// one its waiter read as it reaped it.
    /// A beat says how long a lease's runs did nothing, from the counts its looks found: the
    /// quiet time grows while they stay under [`IDLE_CPU_PCT`] of a core and 1 MB of I/O, starts
    /// again at a look that shows work or a process gone, and is no number while nothing can be
    /// measured. The looks and their times are given, so a loaded box cannot change the answer.
    #[test]
    fn a_beat_says_how_long_a_leases_runs_did_nothing() {
        let (x, base, _) = test_exec("quiet", Opts::default());
        x.handle("lease-start", r#"{"id":"Q1","lane":"quiet","kind":"test","rev":null}"#);
        let t0 = Instant::now();
        let quiet = |secs: u64, done: Option<(f64, u64)>| {
            let look = done.map(|(cpu_s, io_bytes)| os::Activity { cpu_us: (cpu_s * 1e6) as u64, io_bytes });
            let samples = BTreeMap::from([("Q1".to_string(), (None, None, look))]);
            let a = x.beat_from("Q1", &samples, t0 + Duration::from_secs(secs));
            let v = makepad_strict_json::parse(a.as_bytes()).unwrap();
            v.get("info").and_then(|i| i.get("Q1")).and_then(|i| i.get("idle_s")).and_then(|s| s.as_u64())
        };
        assert_eq!(quiet(0, None), None, "nothing measured: no number");
        assert_eq!(quiet(0, Some((40.0, 5 << 20))), Some(0), "the first look starts the quiet");
        assert_eq!(quiet(300, Some((49.0, 5 << 20))), Some(300), "polling at 3% of a core is doing nothing");
        assert_eq!(quiet(700, Some((70.0, (5 << 20) + 100_000))), Some(700), "4.3% and 100 KB written: still nothing");
        assert_eq!(quiet(710, Some((130.0, 6 << 20))), Some(0), "a minute of CPU in ten seconds is work");
        assert_eq!(quiet(720, Some((130.1, 6 << 20))), Some(10));
        assert_eq!(quiet(730, Some((130.1, 8 << 20))), Some(0), "2 MB written is work");
        assert_eq!(quiet(740, Some((20.0, 8 << 20))), Some(0), "a process gone with what it had done");
        assert_eq!(quiet(750, None), None, "a look that cannot measure ends the quiet");
        x.handle("lease-end", "Q1 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_beat_says_what_a_lease_holds_and_a_short_run_leaves_a_peak() {
        let (x, base, _) = test_exec("held", Opts::default());
        x.handle("lease-start", r#"{"id":"H1","lane":"held","kind":"test","rev":null}"#);
        let info = |x: &Exec| {
            let a = x.handle("lease-beat", "H1");
            let v = makepad_strict_json::parse(a[0].as_bytes()).unwrap();
            v.get("info").and_then(|i| i.get("H1")).and_then(|i| i.get("held_mb")).cloned()
        };
        assert!(info(&x).is_some_and(|v| v.is_null()), "nothing ran: no look yet");
        x.handle("lease-run", &format!("H1 . {}", b64(&["/bin/sleep", "30"])));
        let held = (0..100).find_map(|_| {
            std::thread::sleep(Duration::from_millis(50));
            x.handle("lease-beat", "H1");
            x.leases.lock().unwrap()["H1"].held_bytes.filter(|&b| b > 0)
        });
        assert!(held.is_some_and(|b| b > 0), "{held:?}");
        assert!(info(&x).is_some_and(|v| v.as_u64().is_some()));
        x.handle("lease-end", "H1 released");
        x.handle("lease-start", r#"{"id":"H2","lane":"held","kind":"test","rev":null}"#);
        x.handle("lease-run", &format!("H2 . {}", b64(&["/bin/echo", "hi"])));
        log_until_exit(&x, "H2", 1);
        let end = x.handle("lease-end", "H2 released");
        let v = makepad_strict_json::parse(end[0].as_bytes()).unwrap();
        assert!(v.get("peak_ram_gb").and_then(|p| super::super::num(p)).is_some_and(|gb| gb > 0.0), "{end:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn two_leases_logs_hold_only_their_own_output() {
        let (x, base, _) = test_exec("iso", Opts::default());
        for (id, lane) in [("L10", "lane-a"), ("L11", "lane-b")] {
            let a = x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"{lane}","kind":"test","rev":null}}"#));
            assert_eq!(field(&a[0], "ok").and_then(|v| super::super::num(&v)), Some(1.0), "{a:?}");
        }
        // interleaved runs on both leases: each log carries its lease's marker and never the other's
        let mut runs = Vec::new();
        for round in 0..3 {
            for (id, who) in [("L10", "from-lane-a"), ("L11", "from-lane-b")] {
                let r = x.handle("lease-run", &format!("{id} . {}", b64(&["/bin/echo", &format!("{who}-{round}")])));
                runs.push((id, who, round, field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32));
            }
        }
        for (id, who, round, n) in runs {
            let (text, code) = log_until_exit(&x, id, n);
            assert_eq!(code, 0);
            assert_eq!(text.trim(), format!("{who}-{round}"), "{id} run {n}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn base64_and_inspect_words() {
        assert_eq!(b64_decode(&b64(&["/bin/echo", "hi"])).unwrap(), b"/bin/echo\0hi");
        assert_eq!(b64_decode("aGk"), Some(b"hi".to_vec()));
        assert_eq!(b64_decode("a b"), None);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(inspect_commands(&s(&["G3", "G; rm"]), &s(&["P2"]), true), vec!["rm G3", "probe rm P2", "continue all", "resume"]);
        assert!(inspect_commands(&[], &[], false).is_empty());
    }

    #[test]
    fn runs_get_a_clean_env_and_screen_and_ci_leases_the_wm() {
        let hub = vec![("PATH".to_string(), "/bin".to_string()), ("HOME".into(), "/h".into()), ("LC_ALL".into(), "C".into()), ("MAKEPAD_WM_ROOT".into(), "/wm".into()), ("MAKEPAD_WM_SOCKET".into(), "/s".into()), ("DISPLAY".into(), ":0".into()), ("CARGO_TARGET_DIR".into(), "/wm/target".into()), ("RUSTFLAGS".into(), "-g".into())];
        let keys = |kind| base_env(kind, hub.clone().into_iter()).into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(keys(Kind::Test), vec!["PATH", "HOME", "LC_ALL"]);
        assert_eq!(keys(Kind::Build), vec!["PATH", "HOME", "LC_ALL"]);
        assert_eq!(keys(Kind::Screen), vec!["PATH", "HOME", "LC_ALL", "MAKEPAD_WM_SOCKET", "DISPLAY"]);
        assert_eq!(keys(Kind::Ci), keys(Kind::Screen));
    }

    #[test]
    fn a_live_session_gets_a_cpu_weight_and_its_jit_sizes_its_own_threads() {
        let (x, _base, _events) = test_exec("live", Opts::default());
        let live = x.handle("lease-start", r#"{"id":"L1","lane":"livex","kind":"test","rev":null,"live":true,"cost":{"cores":1}}"#);
        let plain = x.handle("lease-start", r#"{"id":"L2","lane":"plainx","kind":"test","rev":null,"cost":{"cores":1}}"#);
        assert!(!live[0].contains("MAKEPAD_RUN_JOBS") && live[0].contains("\"CARGO_BUILD_JOBS\":\"1\""), "{live:?}");
        assert!(plain[0].contains("\"MAKEPAD_RUN_JOBS\":\"1\""), "{plain:?}");
        // a restarted hub keeps it a live session
        let ledger = x.leases.lock().unwrap()["L1"].ledger();
        assert!(Lease::from_ledger(&ledger).is_some_and(|l| l.live), "{ledger}");
        // its scope: a weight from its cores, never a quota (a quota of one core held a live Fab to one CPU)
        let cost = super::super::Cost { cores: 1.0, ram_gb: 4.0, disk_gb: 1.0, gpus: 0 };
        let unit = os::lease_argv(vec!["x".into()], Some("u"), 10, "", Some(cost), None);
        assert!(!unit.iter().any(|a| a.starts_with("CPUQuota")), "{unit:?}");
        assert_eq!(unit.iter().any(|a| a == "CPUWeight=100"), cfg!(target_os = "linux"), "{unit:?}");
    }

    #[test]
    fn round_trip_run_log_end() {
        if os::no_image_roles() { return; }
        let (x, base, events) = test_exec("rt", Opts { grace: Duration::from_secs(10), ..Opts::default() });
        let a = x.handle("lease-start", r#"{"id":"L1","lane":"gapx","kind":"test","rev":null,"env":{"FOO":"bar","CARGO_INCREMENTAL":"1"}}"#);
        assert_eq!(field(&a[0], "ok").and_then(|v| super::super::num(&v)), Some(1.0), "{a:?}");
        let work = PathBuf::from(field(&a[0], "work").unwrap().as_str().unwrap());
        let tmp = PathBuf::from(field(&a[0], "tmp").unwrap().as_str().unwrap());
        assert!(work.ends_with("devhub/lane/gapx/src") && work.is_dir() && tmp.is_dir());
        assert!(a[0].contains("\"CARGO_INCREMENTAL\":\"0\"") && a[0].contains("\"FOO\":\"bar\""));
        assert!(base.join("leases/L1.json").exists());
        // the same start again answers the same
        assert_eq!(x.handle("lease-start", r#"{"id":"L1","lane":"gapx","kind":"test","rev":null}"#), a);

        let r = x.handle("lease-run", &format!("L1 . {}", b64(ECHO_HI)));
        let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
        let (text, code) = log_until_exit(&x, "L1", n);
        assert_eq!((text.trim(), code), ("hi", 0));
        // the env reaches the program; a failing one reports its code
        let r = x.handle("lease-run", &format!("lease-run L1 . {}", b64(ENV)));
        let (env, _) = log_until_exit(&x, "L1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32);
        assert!(env.contains("FOO=bar") && env.contains("SANDBOX_MUTE=1") && env.contains(&format!("TMPDIR={}", tmp.display())) && env.contains("CARGO_BUILD_JOBS=4") && env.contains("RUST_TEST_THREADS=4"));
        // a failed step's logs have a place in the lease's tmp, listed for its holder while it lives
        assert!(env.contains(&format!("DEVHUB_KEEP={}", tmp.join(crate::lease::KEEP_DIR).display())) && tmp.join(crate::lease::KEEP_DIR).is_dir(), "{env}");
        assert_eq!(x.handle("get-ls", "gapx L1 tmp/keep"), Vec::<String>::new());
        std::fs::create_dir_all(tmp.join("keep/landgate.x")).unwrap();
        std::fs::write(tmp.join("keep/landgate.x/out.log"), "boom\n").unwrap();
        assert_eq!(x.handle("get-ls", "gapx L1 tmp/keep"), vec![r#"{"p":"keep/landgate.x/out.log","n":5,"r":"tmp/keep/landgate.x/out.log"}"#.to_string()]);
        // every run knows its own lease and lane: its verdict lines say whose they are
        assert!(env.contains("DEVHUB_LEASE=L1") && env.contains("MAKEPAD_TEST_LANE=gapx") && env.contains("MAKEPAD_JIT_SESSION=L1"), "{env}");
        // app tests of two leases of one lane never share an artifacts dir or app home
        assert!(env.lines().filter_map(|l| l.strip_prefix("MAKEPAD_TEST_ARTIFACTS=")).any(|v| Path::new(v).ends_with("makepad_test/L1")), "{env}");
        // nothing of the hub's own environment beyond the basics
        assert!(env.lines().all(|l| !l.starts_with("CARGO_PKG_") && (!l.starts_with("RUST_TEST_") || l == "RUST_TEST_THREADS=4")), "{env}");
        let r = x.handle("lease-run", &format!("L1 . {}", b64(FAIL)));
        assert_eq!(log_until_exit(&x, "L1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32).1, 1);
        assert!(x.handle("lease-run", &format!("L1 ../x {}", b64(&["true"])))[0].contains("err"));

        // a sleeping child (and its own child) is stopped on end
        let r = x.handle("lease-run", &format!("L1 . {}", b64(SLEEP)));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let start = os::proc_start(pid).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        if !os::group_alive(pid, start, &os::Hold::none()) {
            // (it ended in the full suite on a busy pool box, 11-03, and not alone: how it ended)
            let n = x.leases.lock().unwrap()["L1"].runs.last().unwrap().n;
            let (text, code) = log_until_exit(&x, "L1", n);
            panic!("the sleep's group ended within 200 ms: exit {code}, its log [{text}], the hub's events {:?}", events.lock().unwrap());
        }
        // its command goes first under a global OOM, never the box's services; the wrapper keeps its
        // starter's own (this test's: 500 itself when it runs under a lease)
        #[cfg(target_os = "linux")]
        {
            let adj = |p: u32| std::fs::read_to_string(format!("/proc/{p}/oom_score_adj")).unwrap().trim().to_string();
            let work = x.leases.lock().unwrap()["L1"].runs.last().unwrap().work_pid();
            let own = adj(std::process::id());
            assert!(work != pid && adj(work) == "500" && adj(pid) == own, "{work} {} {pid} {} own {own}", adj(work), adj(pid));
        }
        // the box's residents: what is in use less the run's own, never more than the box holds
        #[cfg(unix)]
        {
            let resident = x.resident_gb();
            assert!(resident > 0.0 && resident * 1024.0 <= crate::procinfo::machine().mem_total_mb as f64, "{resident}");
        }
        let beat = x.handle("lease-beat", "L1 L404");
        assert!(beat[0].starts_with(&format!(r#"{{"live":["L1"],"unknown":["L404"],"info":{{"L1":{{"running":"{SLEEP_LABEL}","pid":"#)), "{beat:?}");
        // a stalled lease's JITs: none under a plain run
        assert_eq!(x.handle("lease-jits", "L1"), vec![r#"{"pids":[]}"#.to_string()]);
        assert!(x.handle("lease-jits", "L404")[0].contains("no lease L404"));
        // only a process of the lease's own runs is killed
        assert!(x.handle("lease-kill", "L1 1")[0].contains("is no process of lease L1"));
        let e = x.handle("lease-end", "L1 released");
        assert!(e[0].contains(&format!("\"killed\":[{pid}]")) && e[0].contains("\"leak\":[]"), "{e:?}");
        assert!(!os::group_alive(pid, start, &os::Hold::none()));
        // the tmp goes on a cleanup thread (the lease's end does not wait for it)
        let t = Instant::now();
        while tmp.exists() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!tmp.exists() && !base.join("leases/L1.json").exists());
        assert!(work.exists(), "a lane dir stays warm");
        assert_eq!(receipt(x.handle("lease-end", "L1 released")), receipt(e), "duplicate end returns its cleanup receipt");
        assert!(events.lock().unwrap().iter().any(|l| l.contains("ws_create")) && events.lock().unwrap().iter().any(|l| l.contains("ended on t (released)")));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn get_and_quiet_marks() {
        let (x, base, _) = test_exec("get", Opts::default());
        let a = x.handle("lease-start", r#"{"id":"L2","lane":"getx","kind":"timed","rev":null}"#);
        assert!(field(&a[0], "user").and_then(|u| u.as_str().map(|u| !u.is_empty())).unwrap_or(false), "{a:?}");
        let work = PathBuf::from(field(&a[0], "work").unwrap().as_str().unwrap());
        let tmp = PathBuf::from(field(&a[0], "tmp").unwrap().as_str().unwrap());
        assert!(base.join("quiet/L2.cmd").exists(), "a timed lease marks the machine quiet");
        std::fs::create_dir_all(work.join("out/deep")).unwrap();
        for f in ["a.txt", "out/r.txt", "out/deep/s.bin"] {
            std::fs::write(work.join(f), f).unwrap();
        }
        // ship-done is for a lease at a revision (the node's tree)
        assert!(x.handle("ship-done", &format!("L2 {}", makepad_network::digest::base64_encode(br#"{"shipped":["a.txt"],"deleted":[]}"#)))[0].contains("holds no tree"));
        // get: a folder lists with its own name first, `r` reads it back; a lease's tmp/ too
        let ls = x.handle("get-ls", "getx - src/out");
        assert_eq!(ls.len(), 2, "{ls:?}");
        assert!(ls[0].contains(r#""p":"out/deep/s.bin","n":14,"r":"src/out/deep/s.bin""#), "{ls:?}");
        let r = x.handle("get-read", "getx - 0 src/out/r.txt");
        assert_eq!(field(&r[0], "data").and_then(|d| d.as_str().and_then(b64_decode)), Some(b"out/r.txt".to_vec()));
        assert_eq!(field(&r[0], "eof").and_then(|e| e.as_bool()), Some(true));
        std::fs::write(tmp.join("result.jsonl"), "{}").unwrap();
        let ls = x.handle("get-ls", "getx L2 tmp/result.jsonl");
        assert!(ls[0].contains(r#""p":"result.jsonl","n":2,"r":"tmp/result.jsonl""#), "{ls:?}");
        assert!(x.handle("get-read", "getx L2 0 tmp/result.jsonl")[0].contains("\"eof\":true"));
        assert!(x.handle("get-ls", "getx - ../x")[0].contains("err"));
        assert!(x.handle("get-ls", "other L2 tmp")[0].contains("err"), "a lease answers only for its lane");
        // ended: the quiet mark goes, the lane dir stays readable
        x.handle("lease-end", "L2 released");
        assert!(!base.join("quiet/L2.cmd").exists());
        assert!(x.handle("get-ls", "getx - src/a.txt")[0].contains(r#""p":"a.txt""#));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Two jobs of one lane on one box at one pin, with different uncommitted files: each runs its
    /// own. The second's ship replaced the first's under it, and both said ok (one dumped the
    /// shaders of the other's source, 11-04).
    #[cfg(feature = "checkout")]
    #[test]
    fn two_leases_of_a_lane_at_one_pin_each_run_their_own_source() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-exec-sources-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let g = |args: &[&str]| crate::sync::git::run(Some(&src), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "work"]);
        std::fs::write(src.join("a.rs"), "pin a").unwrap();
        std::fs::write(src.join("b.rs"), "pin b").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "pin"]);
        let pin = g(&["rev-parse", "HEAD"]).trim().to_string();
        let blob = |p: &str| g(&["rev-parse", &format!("HEAD:{p}")]).trim().to_string();
        std::env::set_var("MAKEPAD_DEVHUB_GIT", base.join("stores"));
        let store = crate::sync::git::Store::open("makepad").unwrap();
        let bundle = base.join("b.bundle");
        g(&["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work"]);
        store.fetch_bundle(&bundle, "work").unwrap();
        store.set_ref(crate::sync::git::SYNCED_REF, &pin).unwrap();
        let (x, exec_base, _) = test_exec("own-sources", Opts::default());
        std::fs::create_dir_all(x.conf().work.join("dev/git")).unwrap();

        let ship = |id: &str, files: &[(&str, &str)], deleted: &[&str]| {
            let a = x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"lanex","kind":"test","src":"overlay","rev":{{"makepad":"{pin}","commercial":null}}}}"#));
            assert!(!a[0].starts_with("{\"err\""), "{a:?}");
            let staged = x.leases.lock().unwrap()[id].tmp.join("ship");
            std::fs::create_dir_all(&staged).unwrap();
            for (p, text) in files { std::fs::write(staged.join(p), text).unwrap(); }
            let list = |v: Vec<&str>| v.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(",");
            let bases = files.iter().map(|(p, _)| *p).chain(deleted.iter().copied()).map(|p| format!("{}:{}", json_str(p), json_str(&if src.join(p).exists() { blob(p) } else { "new".to_string() }))).collect::<Vec<_>>().join(",");
            let body = format!(r#"{{"shipped":[{}],"deleted":[{}],"bases":{{{bases}}}}}"#, list(files.iter().map(|(p, _)| *p).collect()), list(deleted.to_vec()));
            let d = x.handle("ship-done", &format!("{id} {}", makepad_network::digest::base64_encode(body.as_bytes())));
            assert!(d[0].contains(&format!("\"overlay\":{}", files.len() + deleted.len())), "{d:?}");
            d[0].clone()
        };
        let one = ship("O1", &[("a.rs", "one a"), ("b.rs", "one b")], &[]);
        let two = ship("O2", &[("a.rs", "two a"), ("c.rs", "two c")], &["b.rs"]);
        // what each run reads on top of the pin: the folder in its own environment
        let edits = |id: &str| x.leases.lock().unwrap()[id].env.iter().find(|(k, _)| k == "MAKEPAD_RUN_EDITS").map(|(_, v)| PathBuf::from(v)).unwrap();
        let read = |id: &str, p: &str| std::fs::read_to_string(edits(id).join(p)).ok();
        let gone = |id: &str| std::fs::read_to_string(edits(id).join(".makepad-run-deleted")).unwrap_or_default();
        assert_eq!((read("O1", "a.rs").as_deref(), read("O1", "b.rs").as_deref(), read("O1", "c.rs")), (Some("one a"), Some("one b"), None), "the first lease's source, after the second's ship");
        assert_eq!(gone("O1"), "", "the second's deletion is not the first's");
        assert_eq!((read("O2", "a.rs").as_deref(), read("O2", "b.rs"), read("O2", "c.rs").as_deref()), (Some("two a"), None, Some("two c")));
        assert_eq!(gone("O2"), "b.rs\n");
        // each says which source it took: the same files say the same, whichever lease ships them
        let source = |answer: &str| field(answer, "source").and_then(|s| s.as_str().map(str::to_string)).unwrap_or_default();
        assert!(!source(&one).is_empty() && source(&one) != source(&two), "{one} {two}");
        assert_eq!(source(&ship("O3", &[("a.rs", "one a"), ("b.rs", "one b")], &[])), source(&one));
        assert!(x.leases.lock().unwrap()["O1"].ledger().contains(&source(&one)), "the record keeps it");

        // one's end leaves the other's source alone, and takes its own folder with it
        let first = edits("O1");
        x.handle("lease-end", "O1 released");
        let t = Instant::now();
        while first.exists() && t.elapsed() < Duration::from_secs(10) { std::thread::sleep(Duration::from_millis(20)); }
        assert!(!first.exists(), "its folder left with it");
        assert_eq!(read("O2", "a.rs").as_deref(), Some("two a"));
        x.handle("lease-end", "O2 released");
        x.handle("lease-end", "O3 released");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&exec_base);
    }

    /// Native and CI leases keep their own tree and target; release undoes the overlay and keeps the pin.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_rev_lease_leaves_the_work_tree_at_its_pin() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-exec-box-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, root, deployment) = (base.join("src"), base.join("ci-tree"), base.join("makepad"));
        std::fs::create_dir_all(&src).unwrap();
        let g = |args: &[&str]| crate::sync::git::run(Some(&src), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "work"]);
        std::fs::write(src.join(".gitignore"), "/target/\n").unwrap();
        std::fs::write(src.join("a.rs"), "landed").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "landed"]);
        let landed = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(src.join("a.rs"), "lane").unwrap();
        g(&["commit", "-q", "-am", "lane"]);
        let lane = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::env::set_var("MAKEPAD_DEVHUB_GIT", base.join("stores"));
        let store = crate::sync::git::Store::open("makepad").unwrap();
        let bundle = base.join("b.bundle");
        g(&["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work"]);
        store.fetch_bundle(&bundle, "work").unwrap();
        store.move_checkout(&deployment, None, &landed).unwrap();
        store.set_ref(crate::sync::git::SYNCED_REF, &landed).unwrap();
        let (x, exec_base, _) = test_exec("tree", Opts { tree: Some(root.clone()), ..Opts::default() });

        for (id, how, kind) in [("R1", "released", "test"), ("R2", "expired", "ci")] {
            let (root, target) = if kind == "ci" { (root.clone(), x.conf().work.join("ci-target")) } else { (super::super::tree::native_root(&x.conf().work, "lanex"), super::super::tree::native_target(&x.conf().work, "lanex")) };
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(target.join("warm"), "x").unwrap();
            let a = x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"lanex","kind":"{kind}","rev":{{"makepad":"{lane}","commercial":null}}}}"#));
            assert_eq!(field(&a[0], "work").and_then(|w| w.as_str().map(PathBuf::from)), Some(root.clone()), "{a:?}");
            assert!(a[0].contains(&format!("\"CARGO_TARGET_DIR\":{}", json_str(&target.to_string_lossy()))));
            assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "lane");
            // `devhub run` shipped a new file on top
            std::fs::write(root.join("new.rs"), "from the lane's worktree").unwrap();
            let d = x.handle("ship-done", &format!("{id} {}", makepad_network::digest::base64_encode(br#"{"shipped":["new.rs"],"deleted":[]}"#)));
            assert!(d[0].contains("\"overlay\":1"), "{d:?}");
            assert!(x.handle("caps", "caps")[0].contains(&if kind == "ci" { format!("\"tree\":\"{lane}\"") } else { "\"native\":[\"lanex\"]".into() }));
            x.handle("lease-end", &format!("{id} {how}"));
            // the work tree stays warm at its pin; the deployment is independent
            assert_eq!(super::super::tree::current(&root).map(|p| p.makepad), Some(lane.clone()), "{id}");
            assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), "lane");
            assert!(!root.join("new.rs").exists() && target.join("warm").exists());
            assert_eq!(super::super::tree::deploy(&deployment).map(|p| p.makepad), Some(landed.clone()));
            assert_eq!(std::fs::read_to_string(deployment.join("a.rs")).unwrap(), "landed");
        }
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&exec_base);
    }

    /// A run never starts without its shipped files: when something took them between the ship
    /// and the run (a hub restart in between included), the run fails before it starts and names
    /// every path; once a run started, later runs are the command's own business.
    #[cfg(all(feature = "checkout", unix))]
    #[test]
    fn a_run_never_starts_without_its_shipped_files() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-exec-shipped-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let g = |args: &[&str]| crate::sync::git::run(Some(&src), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "work"]);
        std::fs::write(src.join(".gitignore"), "/target/\n").unwrap();
        std::fs::write(src.join("a.rs"), "a").unwrap();
        std::fs::write(src.join("b.rs"), "b").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "one"]);
        let one = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::env::set_var("MAKEPAD_DEVHUB_GIT", base.join("stores"));
        let bundle = base.join("b.bundle");
        g(&["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work"]);
        crate::sync::git::Store::open("makepad").unwrap().fetch_bundle(&bundle, "work").unwrap();
        let (x, exec_base, _) = test_exec("fresh", Opts { tree: Some(base.join("ci-tree")), ..Opts::default() });
        let a = x.handle("lease-start", &format!(r#"{{"id":"F1","lane":"lanex","kind":"test","rev":{{"makepad":"{one}","commercial":null}}}}"#));
        assert!(a[0].contains("\"work\""), "{a:?}");
        let root = super::super::tree::native_root(&x.conf().work, "lanex");
        std::fs::create_dir_all(root.join("zz/var")).unwrap();
        std::fs::write(root.join("zz/var/new.rs"), "shipped").unwrap();
        let d = x.handle("ship-done", &format!("F1 {}", makepad_network::digest::base64_encode(br#"{"shipped":["zz/var/new.rs","a.rs"],"deleted":["b.rs"]}"#)));
        assert!(d[0].contains("\"deleted\":1") && !root.join("b.rs").exists(), "{d:?}");
        // the hub restarts between the ship and the run, and its tree lost the ship meanwhile
        let saved = std::fs::read_to_string(exec_base.join("leases/F1.json")).unwrap();
        x.leases.lock().unwrap().insert("F1".into(), Lease::from_ledger(&saved).unwrap());
        std::fs::remove_file(root.join("zz/var/new.rs")).unwrap();
        std::fs::write(root.join("b.rs"), "b").unwrap();
        let why = x.handle("lease-run", &format!("F1 . {}", b64(ECHO_HI)));
        assert!(why[0].contains("1 shipped file(s) missing from its tree: zz/var/new.rs;") && why[0].contains("1 shipped deletion(s) back in its tree: b.rs") && why[0].contains("the run did not start"), "{why:?}");
        assert!(x.leases.lock().unwrap()["F1"].runs.is_empty(), "nothing ran");
        // with the ship in place it runs; what the command does to the tree after is its own
        std::fs::write(root.join("zz/var/new.rs"), "shipped").unwrap();
        std::fs::remove_file(root.join("b.rs")).unwrap();
        assert_eq!(field(&x.handle("lease-run", &format!("F1 . {}", b64(ECHO_HI)))[0], "n").and_then(|n| n.as_u64()), Some(1));
        assert_eq!(log_until_exit(&x, "F1", 1).1, 0);
        std::fs::remove_file(root.join("zz/var/new.rs")).unwrap();
        assert_eq!(field(&x.handle("lease-run", &format!("F1 . {}", b64(ECHO_HI)))[0], "n").and_then(|n| n.as_u64()), Some(2));
        log_until_exit(&x, "F1", 2);
        x.handle("lease-end", "F1 released");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&exec_base);
    }

    /// A file shipped on top of a lease stays out of the way however the lease ended: the ship
    /// never reached `ship-done`, or a command wrote it. A commit that adds that path lands, and
    /// the next lease starts on it; a stray from outside any lease cannot fail a start either.
    #[cfg(feature = "checkout")]
    #[test]
    fn no_stray_can_fail_a_later_lease() {
        let _env = crate::sync::git::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-exec-nostray-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, root) = (base.join("src"), base.join("ci-tree"));
        std::fs::create_dir_all(&src).unwrap();
        let g = |args: &[&str]| crate::sync::git::run(Some(&src), None, &[], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap();
        g(&["init", "-q", "-b", "work"]);
        std::fs::write(src.join(".gitignore"), "/target/\n").unwrap();
        std::fs::write(src.join("a.rs"), "one").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "one"]);
        let one = g(&["rev-parse", "HEAD"]).trim().to_string();
        std::env::set_var("MAKEPAD_DEVHUB_GIT", base.join("stores"));
        let store = crate::sync::git::Store::open("makepad").unwrap();
        let bundle = base.join("b.bundle");
        let fetch = || {
            g(&["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work"]);
            store.fetch_bundle(&bundle, "work").unwrap();
        };
        fetch();
        let (x, exec_base, events) = test_exec("stray", Opts { tree: Some(root.clone()), ..Opts::default() });
        let start = |id: &str, rev: &str| x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"lanex","kind":"test","rev":{{"makepad":"{rev}","commercial":null}}}}"#));

        // a shipped file ship-done never named, and what a command wrote, in a new dir: both go
        // at the lease's end, the target stays, and the event names them
        let a = start("S1", &one);
        assert!(a[0].contains("\"work\""), "{a:?}");
        let root = super::super::tree::native_root(&x.conf().work, "lanex");
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/warm"), "x").unwrap();
        std::fs::create_dir_all(root.join("tools/steam")).unwrap();
        std::fs::write(root.join("tools/steam/new.rs"), "shipped, not committed yet").unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        std::fs::write(root.join("out/result.txt"), "a command's output").unwrap();
        // a nested target dir stays, ignored or not
        std::fs::create_dir_all(root.join("nested/target")).unwrap();
        std::fs::write(root.join("nested/target/warm"), "x").unwrap();
        x.handle("lease-end", "S1 released");
        assert!(root.join("nested/target/warm").exists());
        assert!(!root.join("tools").exists() && !root.join("out").exists() && root.join("target/warm").exists());
        assert!(events.lock().unwrap().iter().any(|e| e.contains("ws_strays") && e.contains("tools/")), "{:?}", events.lock().unwrap());

        // the next lease moves to a commit that adds the removed path
        std::fs::create_dir_all(src.join("tools/steam")).unwrap();
        std::fs::write(src.join("tools/steam/new.rs"), "landed").unwrap();
        g(&["add", "."]);
        g(&["commit", "-q", "-m", "two"]);
        let two = g(&["rev-parse", "HEAD"]).trim().to_string();
        fetch();
        let a = start("S2", &two);
        assert!(a[0].contains("\"work\""), "{a:?}");
        assert_eq!(std::fs::read_to_string(root.join("tools/steam/new.rs")).unwrap(), "landed");
        x.handle("lease-end", "S2 released");
        assert_eq!(std::fs::read_to_string(root.join("tools/steam/new.rs")).unwrap(), "landed", "release keeps the pin");

        // move to the first pin, then a stray at a path the next pin adds is cleared at start
        assert!(start("S0", &one)[0].contains("\"work\""));
        x.handle("lease-end", "S0 released");
        assert!(!root.join("tools").exists());
        std::fs::create_dir_all(root.join("tools/steam")).unwrap();
        std::fs::write(root.join("tools/steam/new.rs"), "a stray from a hand copy").unwrap();
        let a = start("S3", &two);
        assert!(a[0].contains("\"work\""), "{a:?}");
        assert_eq!(std::fs::read_to_string(root.join("tools/steam/new.rs")).unwrap(), "landed");
        x.handle("lease-end", "S3 released");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&exec_base);
    }

    #[test]
    fn an_abnormal_end_says_how_it_ended() {
        let kernel = "memory: usage 2729056kB, limit 2729056kB, failcnt 56383\noom-kill:constraint=CONSTRAINT_MEMCG,nodemask=(null),oom_memcg=/app.slice/devhub-lease-L7-1.scope,task_memcg=/app.slice/devhub-lease-L7-1.scope,task=makepad.jit-256,pid=42\nMemory cgroup out of memory: Killed process 42 (makepad.jit-256) total-vm:401088916kB, anon-rss:2455836kB, file-rss:573996kB, UID:1000\nNVRM: Xid (PCI:0000:01:00): 79, GPU has fallen off the bus.";
        assert_eq!(ended_how("L7", 1, Some("devhub-lease-L7-1"), 137, Some(kernel)), ("L7 run 1 ended without its summary: killed by signal 9 (SIGKILL); the kernel's OOM killer killed 42 (makepad.jit-256), 2.3 GB resident, at the lease's memory cap of 2.6 GB; GPU: NVRM: Xid (PCI:0000:01:00): 79, GPU has fallen off the bus.".to_string(), true));
        // another lease's OOM kill is not this run's; a plain exit is no more than a failure
        assert_eq!(ended_how("L8", 2, Some("devhub-lease-L8-2"), 101, Some(&kernel[..kernel.find("NVRM").unwrap()])), ("L8 run 2 ended without its summary: exit 101; no OOM kill".to_string(), false));
        assert_eq!(ended_how("L9", 1, None, -1, None).0, "L9 run 1 ended without its summary: its exit is unknown (its hub restarted while it ran); the kernel log was not read here");
        let log = std::env::temp_dir().join(format!("devhub-tail-{}.log", std::process::id()));
        std::fs::write(&log, (1..=30).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        let tail = log_tail(&log, TAIL_LINES);
        let _ = std::fs::remove_file(&log);
        assert!(tail.starts_with("line 11\n") && tail.ends_with("line 30") && tail.lines().count() == 20, "{tail}");
    }

    #[test]
    fn the_lease_that_took_the_disk_ends_first() {
        const GB: u64 = 1 << 30;
        let took = |id: &str, grew: u64, held: u64| Took { id: id.into(), grew: grew * GB, held: held * GB };
        // its directories: what the floor is short of, and no swap to it (Linux)
        let wrote = [took("L1", 3, 2), took("L2", 102, 8), took("L3", 40, 8)];
        let (id, why) = lease_over_disk(0, 25 * GB, None, &wrote).unwrap();
        assert_eq!(id, "L2");
        assert!(why.contains("its directories grew 102.0 GB") && why.contains("0 GB of disk is left under a 25 GB floor") && !why.contains("swap"), "{why}");
        assert_eq!(lease_over_disk(26 * GB, 25 * GB, None, &wrote), None, "above the floor");
        assert_eq!(lease_over_disk(0, 25 * GB, None, &[took("L1", 24, 0), took("L2", 20, 0)]), None, "no lease explains what the floor is short of");
        assert_eq!(lease_over_disk(0, 25 * GB, None, &[]), None);
        // its memory: past the RAM, in swap files that hold it
        let swapped = [took("L1", 0, 3), took("L2", 0, 180)];
        let (id, why) = lease_over_disk(0, 25 * GB, Some((16 * GB, 167 * GB)), &swapped).unwrap();
        assert_eq!(id, "L2");
        assert!(why.contains("holds 180.0 GB of memory while swap files take 167 GB") && !why.contains("directories"), "{why}");
        assert_eq!(lease_over_disk(0, 25 * GB, Some((16 * GB, 10 * GB)), &swapped), None, "the swap files are not what the floor is short of");
        assert_eq!(lease_over_disk(0, 25 * GB, Some((256 * GB, 167 * GB)), &swapped), None, "the leases do not explain the swap");
        // both add up for the lease that did both
        assert_eq!(lease_over_disk(0, 25 * GB, Some((16 * GB, 30 * GB)), &[took("L1", 15, 26), took("L2", 24, 0)]).unwrap().0, "L1");
        assert_eq!(caps::swap_on_disk().is_some_and(|(ram, _)| ram > 0), cfg!(target_os = "macos"));
        // the walk: growth from its first measure, of what is its own
        let (x, base, _events) = test_exec("od", Opts::default());
        assert!(x.handle("lease-start", r#"{"id":"L9","lane":"t","kind":"test","rev":null}"#)[0].contains("\"ok\":1"));
        let (own, shared) = (base.join("own"), base.join("shared"));
        std::fs::create_dir_all(&own).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        let walked = |done: &dyn Fn(&DiskUse) -> bool| {
            x.disk_of("L9", vec![(own.clone(), true), (shared.clone(), false)], true);
            while !x.disk_use.lock().unwrap().get("L9").is_some_and(done) { std::thread::sleep(Duration::from_millis(5)); }
        };
        walked(&|u| u.mb != u64::MAX);
        assert_eq!(x.disk_grew("L9"), 0);
        // (written through to its blocks, and no run of one byte that a file system compresses away)
        let write = |p: PathBuf, mb: u32| {
            use std::io::Write as _;
            let mut f = std::fs::File::create(p).unwrap();
            f.write_all(&(0..mb << 20).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect::<Vec<u8>>()).unwrap();
            f.sync_all().unwrap();
        };
        write(own.join("cold.log"), 3);
        write(shared.join("build.rlib"), 5);
        x.disk_use.lock().unwrap().get_mut("L9").unwrap().at = Instant::now() - Duration::from_secs(121);
        walked(&|u| u.own_mb >= 1);
        let u = x.disk_use.lock().unwrap()["L9"];
        assert!(u.mb > u.own_mb, "the shared part shows, and is not its own: {} MB, {} MB", u.mb, u.own_mb);
        assert_eq!(x.disk_grew("L9"), u.own_mb << 20);
        // the look, and a beat that names it once, to the master that beats that lease
        let free = x.guard.free();
        x.guard.set_policy(((free >> 30) + 2) as f64, ((free >> 30) + 3) as f64, false);
        x.look_over_disk();
        assert!(x.over_disk.lock().unwrap().is_none(), "a few MB does not explain it");
        x.disk_use.lock().unwrap().get_mut("L9").unwrap().own_mb += 3 << 10;
        x.look_over_disk();
        assert_eq!(x.over_disk.lock().unwrap().as_ref().map(|o| o.0.as_str()), Some("L9"));
        let end = field(&x.handle("lease-beat", "L9")[0], "end").unwrap();
        assert_eq!(end.get("id").and_then(|v| v.as_str()), Some("L9"));
        assert!(end.get("why").and_then(|v| v.as_str()).unwrap().contains("its directories grew"));
        assert!(field(&x.handle("lease-beat", "L9")[0], "end").is_none());
        x.handle("lease-end", "L9 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_run_is_told_started_when_its_ledger_cannot_be_written() {
        let (x, base, events) = test_exec("ls", Opts::default());
        assert!(x.handle("lease-start", r#"{"id":"L1","lane":"t","kind":"test","rev":null}"#)[0].contains("\"ok\":1"));
        // (as a full disk does: the write the ledger goes through fails)
        let new = x.state.join("L1.json.new");
        std::fs::create_dir(&new).unwrap();
        let r = x.handle("lease-run", &format!("L1 . {}", b64(ECHO_HI)));
        assert!(field(&r[0], "run").is_some(), "{r:?}");
        assert!(events.lock().unwrap().iter().any(|e| e.contains("saving lease L1")));
        while x.handle("lease-log", "L1 1 0")[0].contains("\"exit\":null") { std::thread::sleep(Duration::from_millis(5)); }
        std::fs::remove_dir(&new).unwrap();
        x.handle("lease-end", "L1 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_test_run_is_in_the_receipt() {
        let (x, base, _events) = test_exec("ft", Opts::default());
        assert!(x.handle("lease-start", r#"{"id":"L1","lane":"t","kind":"test","rev":null}"#)[0].contains("\"ok\":1"));
        let failed: &[&str] = &["/bin/sh", "-c", "printf 'test a::b ... FAILED\\n\\nfailures:\\n    a::b\\n\\ntest result: FAILED. 0 passed; 1 failed\\n'; exit 101"];
        let ok: &[&str] = &["/bin/echo", "test result: ok. 1 passed; 0 failed"];
        for argv in [failed, ok] {
            let r = x.handle("lease-run", &format!("L1 . {}", b64(argv)));
            let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap();
            while x.handle("lease-log", &format!("L1 {n} 0"))[0].contains("\"exit\":null") { std::thread::sleep(Duration::from_millis(5)); }
        }
        // (the end runs behind the first ask: asked again until its receipt is there)
        let end = |id: &str| loop {
            let a = x.handle("lease-end", &format!("{id} released"));
            if field(&a[0], "failed_tests").is_some() { return a[0].clone(); }
            std::thread::sleep(Duration::from_millis(5));
        };
        let text = field(&end("L1"), "failed_tests").and_then(|v| v.as_str().map(str::to_string)).unwrap();
        assert!(text.contains("test a::b ... FAILED") && text.contains("test result: FAILED"), "{text}");
        assert!(x.handle("lease-start", r#"{"id":"L2","lane":"t","kind":"test","rev":null}"#)[0].contains("\"ok\":1"));
        assert!(field(&end("L2"), "failed_tests").is_some_and(|v| v.is_null()));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_long_test_with_no_result_is_still_running() {
        let log = "test a::quick ... ok\ntest b::hung has been running for over 60 seconds\ntest c::slow has been running for over 60 seconds\ntest c::slow ... ok\n";
        assert_eq!(tests_running(log), ["b::hung"]);
    }

    /// A run that went quiet: its holder reads it on its own lease, and only a lease something else
    /// ended keeps how far it got.
    #[cfg(unix)]
    #[test]
    fn a_hung_run_can_be_read_and_a_cancel_keeps_its_last_lines() {
        let (x, base, _events) = test_exec("why", Opts::default());
        let hang = |id: &str| {
            assert!(x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"t","kind":"test","rev":null}}"#))[0].contains("\"ok\":1"));
            let hung: &[&str] = &["/bin/sh", "-c", "echo 'test a::hung has been running for over 60 seconds'; exec sleep 100"];
            let n = field(&x.handle("lease-run", &format!("{id} . {}", b64(hung)))[0], "n").and_then(|v| super::super::num(&v)).unwrap();
            loop {
                let a = x.handle("lease-log", &format!("{id} {n} 0"));
                if !a[0].contains("\"data\":\"\"") { break; }
                assert!(a[0].contains("\"exit\":null"), "{a:?}");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        // (the end runs behind the first ask; a refused one is no pending one: this test hung on that)
        let end = |line: &str| loop {
            let a = x.handle("lease-end", line);
            assert!(field(&a[0], "err").is_none(), "{a:?}");
            if field(&a[0], "tail").is_some() { return a[0].clone(); }
            std::thread::sleep(Duration::from_millis(5));
        };
        hang("WH1");
        let why = field(&x.handle("lease-why", "WH1")[0], "text").and_then(|v| v.as_str().map(str::to_string)).unwrap();
        assert!(why.contains("run 1: still running\nrunning for over a minute: a::hung\n"), "{why}");
        #[cfg(target_os = "linux")]
        assert!(why.contains("sleep 100"), "{why}");
        let kept = field(&end(r#"WH1 released {"tail":true}"#), "tail").and_then(|v| v.as_str().map(str::to_string)).unwrap();
        assert!(kept.starts_with("run 1: still running\nrunning for over a minute: a::hung\n") && kept.contains("test a::hung has been"), "{kept}");
        hang("WH2");
        assert!(field(&end("WH2 released"), "tail").is_some_and(|v| v.is_null()));
        assert!(x.handle("lease-why", "L404")[0].contains("no lease L404"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn watchdog_ends_a_lease_without_beats() {
        // the test's clock: the watchdog thread never ticks here, `expire_at` says what time it is
        let (x, base, events) = test_exec("wd", Opts { tick: Duration::from_secs(3600), grace: Duration::from_secs(10), ..Opts::default() });
        let a = x.handle("lease-start", r#"{"id":"L7","lane":"lost","kind":"long","rev":null}"#);
        assert!(a[0].contains("\"ok\":1"));
        let r = x.handle("lease-run", &format!("L7 . {}", b64(SLEEP)));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let start = os::proc_start(pid).unwrap();
        assert!(x.handle("lease-beat", "L7")[0].contains("\"live\":[\"L7\"],\"unknown\":[]"));
        let beat = x.leases.lock().unwrap()["L7"].beat;
        let no_beat = x.opts.no_beat;
        // (the master's beats, on the test's clock)
        x.master_contact(beat, &["L7"]);
        // beaten: it stays
        x.expire_at(beat + no_beat - Duration::from_millis(1));
        assert!(x.leases.lock().unwrap().contains_key("L7"));
        // the master, there (it beats), has not beaten it: the node ends it and stops its process
        x.master_contact(beat + no_beat / 2, &[]);
        let t = beat + no_beat;
        x.expire_at(t);
        assert!(!x.leases.lock().unwrap().contains_key("L7"));
        assert!(!os::group_alive(pid, start, &os::Hold::none()), "its process stopped with it");
        assert!(events.lock().unwrap().iter().any(|l| l.contains("L7 long of lost ended by t itself: the master, there, has not beaten it for 60 s")));
        // the master away (no call of its for the no-beat limit: it restarts, say): every lease is
        // kept for it, however long, and that is said once
        for id in ["L8", "L10"] {
            assert!(x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"kept","kind":"long","rev":null}}"#))[0].contains("\"ok\":1"));
            x.leases.lock().unwrap().get_mut(id).unwrap().beat = t;
        }
        x.master_contact(t, &["L8", "L10"]);
        let said = |what: &str| events.lock().unwrap().iter().filter(|l| l.contains(what)).count();
        x.expire_at(t + no_beat);
        x.expire_at(t + no_beat * 10);
        assert!(["L8", "L10"].iter().all(|id| x.leases.lock().unwrap().contains_key(*id)), "kept while the master is away");
        assert_eq!(said("lease: master away 60 s: no call from it on t; its 2 lease(s) here are kept until it is back"), 1);
        // one started while it was away (a grant its first beat back does not name yet)
        assert!(x.handle("lease-start", r#"{"id":"L12","lane":"kept","kind":"long","rev":null}"#)[0].contains("\"ok\":1"));
        x.leases.lock().unwrap().get_mut("L12").unwrap().beat = t + no_beat * 5;
        // back: its first beat is its claim, however long after its first call: one from before
        // the absence it does not name ends now, the others go by their beats
        let back = t + no_beat * 10;
        x.master_contact(back, &["L8"]);
        assert!(!x.leases.lock().unwrap().contains_key("L10"), "not claimed");
        assert_eq!(said("lease: the master is back on t after 600 s away; 2 lease(s) kept for it; L10 it no longer holds end"), 1);
        assert_eq!(said("lease: L10 long of kept ended by t itself: the master, back, no longer holds it"), 1);
        x.master_contact(back + no_beat / 2, &["L8"]);
        x.leases.lock().unwrap().get_mut("L8").unwrap().beat = back + no_beat / 2;
        x.expire_at(back + no_beat);
        assert!(x.leases.lock().unwrap().contains_key("L8"), "beaten");
        assert!(!x.leases.lock().unwrap().contains_key("L12"), "started while it was away, not beaten since it is back");
        // (the next absence is said again)
        x.expire_at(back + no_beat * 3);
        assert_eq!(said("lease: master away"), 2);
        // dev off refuses write kinds, not inspect
        assert_eq!(x.handle("policy", "dev=off"), vec![r#"{"dev":"off"}"#.to_string()]);
        // a flip made on the node is asked of the master with the caps answer; the master's policy push clears it
        x.dev_req.store(1, Ordering::Release);
        assert!(x.handle("caps", "caps")[0].contains("\"dev_req\":true"));
        x.handle("policy", "dev=off");
        assert!(x.handle("caps", "caps")[0].contains("\"dev_req\":null"));
        assert!(x.handle("lease-start", r#"{"id":"L11","lane":"a","kind":"build","rev":null}"#)[0].contains("dev off"));
        assert!(x.handle("lease-start", r#"{"id":"L9","lane":"a","kind":"inspect","rev":null}"#)[0].contains("\"ok\":1"));
        // the outage its WM recorded before it ran goes to the master in one caps answer
        let down = base.join("devhub.down");
        std::fs::write(&down, makepad_devhub_conf::Trouble { since_ms: 1_000, tries: 3, reason: "could not start: the user's service manager is down".into() }.to_text()).unwrap();
        *x.trouble.lock().unwrap() = take_trouble(&down, "t1", 61_000);
        assert!(!down.exists(), "taken once");
        assert!(x.handle("caps", "caps")[0].contains("\"trouble\":\"devhub on t1 could not run for 60 s (3 tries): could not start: the user's service manager is down\""));
        assert!(x.handle("caps", "caps")[0].contains("\"trouble\":\"\""), "said once");
        assert_eq!(take_trouble(&down, "t1", 61_000), "", "no record: nothing to say");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A process in a group of its own. (This binary, not `sleep`: macOS hides the environment of
    /// its platform binaries.)
    /// How long [`stop_role_app`] works after it took the exit ask: past a look ([`STOP_LOOK`]).
    const EXIT_WORK: Duration = Duration::from_millis(3000);

    /// What the stop tests start: an app that takes the exit ask the way a JIT host does (Unix:
    /// TERM; Windows: `quit` over its control endpoint), then works [`EXIT_WORK`] before it writes
    /// `flushed` to the file it is given and exits (its exit write); with `DEVHUB_TEST_STOP_IDLE`
    /// it takes the ask and then sits.
    #[test]
    fn stop_role_app() {
        let Some(file) = std::env::var_os("DEVHUB_TEST_STOP_APP").map(PathBuf::from) else { return };
        static ASKED: AtomicBool = AtomicBool::new(false);
        fn dispatch(req: &makepad_remote::Req, out: &mut makepad_remote::Out) {
            if req.cmd == "quit" {
                ASKED.store(true, Ordering::Release);
                out.line("{\"quit\":\"asked\"}");
            } else {
                out.err("the stop test's app answers quit only");
            }
        }
        #[cfg(unix)]
        {
            extern "C" fn on_term(_: i32) {
                ASKED.store(true, Ordering::Release);
            }
            extern "C" {
                fn signal(n: i32, handler: usize) -> usize;
            }
            unsafe { signal(15, on_term as extern "C" fn(i32) as usize) };
        }
        let ep = makepad_remote::Endpoint::new("devhub-stop-test");
        ep.register_cmd(dispatch);
        ep.listen_unix("").unwrap();
        std::fs::write(file.with_extension("ready"), "").unwrap();
        while !ASKED.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(20));
        }
        if std::env::var_os("DEVHUB_TEST_STOP_IDLE").is_some() {
            std::thread::sleep(Duration::from_secs(300));
        }
        let t = Instant::now();
        let mut x = 1u64;
        while t.elapsed() < EXIT_WORK {
            x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
        }
        std::fs::write(&file, "flushed").unwrap();
        std::process::exit(0);
    }

    /// [`stop_role_app`] started as a lease run is (its own group, Windows: its own job), announcing
    /// in `base`'s registry; once it took its handler and endpoint.
    fn stop_role(base: &Path, idle: bool) -> (std::process::Child, Arc<Run>, PathBuf) {
        std::fs::create_dir_all(base).unwrap();
        let file = base.join("exit-write");
        let mut c = crate::testenv::role("lease::exec::tests::stop_role_app", base);
        c.env("DEVHUB_TEST_STOP_APP", &file);
        if idle {
            c.env("DEVHUB_TEST_STOP_IDLE", "1");
        }
        c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let exit = exit_file(base, "L1", 1);
        let (child, hold) = os::lease_spawn(&mut c, Some(&exit)).unwrap();
        let pid = child.id();
        let start = os::proc_start(pid).unwrap();
        let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: 0, ended_ms: AtomicU64::new(0), n: 1, pid, start, unit: None, log: base.join("run-1.log"), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: exit, cache: Mutex::new(None), usage: Mutex::new(Usage::default()) });
        let t = Instant::now();
        while !file.with_extension("ready").exists() {
            assert!(t.elapsed() < Duration::from_secs(60), "the app did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        (child, run, file)
    }

    /// A lease's end asks the run to exit and lets it finish its exit write, however long that
    /// works, before anything is killed. Windows ended the job at once: the JIT's exit write never
    /// ran there, and no Fab start on the Windows node was ever warm (11-04).
    #[test]
    fn a_stop_lets_the_run_finish_its_exit_write() {
        let base = std::env::temp_dir().join(format!("devhub-exec-stop-write-{}", std::process::id()));
        let (mut child, run, file) = stop_role(&base, false);
        let t = Instant::now();
        let (stopped, leak) = stop_runs(std::slice::from_ref(&run), &base.join("registry"), false);
        let _ = child.wait();
        assert_eq!(std::fs::read_to_string(&file).ok().as_deref(), Some("flushed"), "the stop skipped the run's exit path ({:?} after the ask)", t.elapsed());
        assert_eq!((stopped, leak), (vec![run.pid], Vec::new()));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A run that took the ask and then does nothing while it lives on is KILLed at its next look:
    /// no timer, its state.
    #[test]
    fn a_stop_kills_a_run_that_took_the_ask_and_sits() {
        let base = std::env::temp_dir().join(format!("devhub-exec-stop-idle-{}", std::process::id()));
        let (mut child, run, file) = stop_role(&base, true);
        let (stopped, leak) = stop_runs(std::slice::from_ref(&run), &base.join("registry"), false);
        let _ = child.wait();
        assert_eq!((stopped, leak), (vec![run.pid], Vec::new()));
        assert!(!file.exists() && !os::group_alive(run.pid, run.start, &run.hold));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// What [`a_leases_end_stops_what_left_its_process_group`] starts: a process that stays.
    #[cfg(unix)]
    #[test]
    fn lease_end_role_stray() {
        if std::env::var_os("DEVHUB_TEST_STRAY").is_some() {
            std::thread::sleep(Duration::from_secs(300));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_leases_end_stops_what_left_its_process_group() {
        let (x, base, _) = test_exec("strays", Opts { grace: Duration::from_secs(2), scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"S1","lane":"strays","kind":"test","rev":null}"#);
        // the lease's run, and what left its group: a stray that keeps the lease's mark and one that
        // removed it (started here: a run goes through `nice`, and under the JIT only this process's
        // own spawn of its executable reaches this test)
        let r = x.handle("lease-run", &format!("S1 . {}", b64(SLEEP)));
        assert!(field(&r[0], "run").is_some(), "{r:?}");
        let mark = format!("{LEASE_ENV}={}", lease_mark(&base.join("leases"), "S1"));
        let spawn = |keep: bool| {
            use std::os::unix::process::CommandExt;
            let mut c = crate::testenv::role("lease::exec::tests::lease_end_role_stray", &base);
            c.env("DEVHUB_TEST_STRAY", "1").process_group(0);
            c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            if keep {
                c.env(LEASE_ENV, lease_mark(&base.join("leases"), "S1"));
            } else {
                c.env_remove(LEASE_ENV);
            }
            c.spawn().unwrap().id()
        };
        let (gone, stays) = (spawn(true), spawn(false));
        let t = Instant::now();
        while !os::with_env(&mark).contains(&gone) {
            assert!(t.elapsed() < Duration::from_secs(30), "the stray did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(os::pid_alive(gone) && os::pid_alive(stays));
        let e = x.handle("lease-end", "S1 released");
        assert!(e[0].contains(&gone.to_string()) && !e[0].contains(&stays.to_string()), "gone {gone} stays {stays}: {e:?}");
        assert_eq!(os::with_env(&format!("{LEASE_ENV}={}", lease_mark(&base.join("leases"), "S1"))), Vec::<u32>::new());
        assert!(os::pid_alive(stays), "one that removed the mark outlives the lease");
        os::signal_one(stays, true);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// What [`a_group_lives_while_a_member_does_after_its_leader_ended`] starts: a leader that
    /// starts a member of its group, writes its pid to the file it is given, and ends when its
    /// input closes.
    #[cfg(unix)]
    #[test]
    fn group_role_leader() {
        let Some(file) = std::env::var_os("DEVHUB_TEST_GROUP_LEADER") else { return };
        let mut c = crate::testenv::role("lease::exec::tests::lease_end_role_stray", Path::new(&file).parent().unwrap());
        c.env("DEVHUB_TEST_STRAY", "1").env_remove("DEVHUB_TEST_GROUP_LEADER");
        c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        std::fs::write(&file, c.spawn().unwrap().id().to_string()).unwrap();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut Vec::new());
    }

    /// A run's leader (the usage wrapper's `sh`) ends at a TERM while its program winds down: the
    /// group lives on while a member does, on every Unix, so a lease's end KILLs it in its grace
    /// instead of taking it for a process that left its groups (a Mac asked only the leader, 11-03).
    #[cfg(unix)]
    #[test]
    fn a_group_lives_while_a_member_does_after_its_leader_ended() {
        use std::os::unix::process::CommandExt;
        let base = std::env::temp_dir().join(format!("devhub-group-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let file = base.join("member");
        let mut c = crate::testenv::role("lease::exec::tests::group_role_leader", &base);
        c.env("DEVHUB_TEST_GROUP_LEADER", &file).process_group(0);
        c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let mut leader = c.spawn().unwrap();
        let pid = leader.id();
        let start = os::proc_start(pid).unwrap();
        let t = Instant::now();
        let member = loop {
            if let Some(p) = std::fs::read_to_string(&file).ok().and_then(|s| s.trim().parse::<u32>().ok()) { break p; }
            assert!(t.elapsed() < Duration::from_secs(60), "the leader did not start its member");
            std::thread::sleep(Duration::from_millis(20));
        };
        drop(leader.stdin.take());
        leader.wait().unwrap();
        assert!(os::pid_alive(member));
        assert!(os::group_alive(pid, start, &os::Hold::none()), "its member {member} lives: the group of {pid} does");
        os::signal_one(member, true);
        let t = Instant::now();
        while os::group_alive(pid, start, &os::Hold::none()) {
            assert!(t.elapsed() < Duration::from_secs(10), "the group of {pid} outlived its last member");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_restarted_hub_takes_its_leases_back() {
        if os::no_image_roles() { return; }
        // (no scopes: a scope is named by the lease id, which other tests here use at the same time)
        let (x, base, _) = test_exec("ledger", Opts { scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"L3","lane":"b","kind":"screen","rev":null}"#);
        let r = x.handle("lease-run", &format!("L3 . {}", b64(SLEEP)));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        // (the old hub gone: a watchdog of its own would end the lease it no longer beats)
        let y = x.restart(Opts { scopes: Some(false), ..Opts::default() });
        assert!(y.handle("lease-beat", "L3")[0].contains("\"live\":[\"L3\"]"));
        let e = y.handle("lease-end", "L3 released");
        assert!(e[0].contains(&format!("\"killed\":[{pid}]")), "{e:?}");
        assert!(!os::group_alive(pid, os::proc_start(pid).unwrap_or(1), &os::Hold::none()));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The self-update restarts a hub under a running job and a live session: nothing waits and
    /// nothing ends, on Windows too (a run's job outlives the hub, which takes it back by its
    /// name). The new hub reads the job's exit from its wrapper (written without any hub), still
    /// holds the live app, and ends what it took back: one process of a run alone (`lease-kill`),
    /// or the whole run with its lease.
    #[test]
    fn a_hub_restart_ends_nothing() {
        if os::no_image_roles() { return; }
        #[cfg(unix)]
        let (exit7, job_cmd, gate_cmd): (&[&str], &[&str], &[&str]) = (&["sh", "-c", "exit 7"], &["sh", "-c", "sleep 1; echo done; exit 7"], &["sh", "-c", "sleep 1; echo gate; exit 0"]);
        #[cfg(windows)]
        let (exit7, job_cmd, gate_cmd): (&[&str], &[&str], &[&str]) = (&["cmd", "/c", "exit 7"], &["cmd", "/c", "ping -n 2 127.0.0.1 >nul & echo done& exit 7"], &["cmd", "/c", "ping -n 2 127.0.0.1 >nul & echo gate& exit 0"]);
        let (x, base, _) = test_exec("handover", Opts { scopes: Some(false), ..Opts::default() });
        // the wrapper alone records an exit, whoever waits for it
        std::fs::create_dir_all(&base).unwrap();
        let usage = base.join("wrapped.usage");
        let argv = os::metered_argv(exit7.iter().map(|s| s.to_string()).collect(), None, &usage);
        assert_eq!(std::process::Command::new(&argv[0]).args(&argv[1..]).status().unwrap().code(), Some(7));
        assert_eq!(std::fs::read_to_string(base.join("wrapped.exit")).unwrap().trim(), "7");
        x.handle("lease-start", r#"{"id":"L4","lane":"b","kind":"test","rev":null}"#);
        let job = x.handle("lease-run", &format!("L4 . {}", b64(job_cmd)));
        let n = field(&job[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let live = x.handle("lease-run", &format!("L4 . {} {{\"live\":true}}", b64(SLEEP)));
        let app = field(&live[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let started = os::proc_start(app).unwrap();
        // a landing's gate lease rides through as well
        x.handle("lease-start", r#"{"id":"G4","lane":"landq-fast","kind":"gate","rev":null}"#);
        let gate = x.handle("lease-run", &format!("G4 . {}", b64(gate_cmd)));
        let gate_n = field(&gate[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32;
        // a run of two processes that do not wait on each other
        #[cfg(windows)]
        {
            x.handle("lease-start", r#"{"id":"K4","lane":"k","kind":"test","rev":null}"#);
            x.handle("lease-run", &format!("K4 . {}", b64(&["cmd", "/c", "start /b ping -n 300 127.0.0.1 >nul & ping -n 300 127.0.0.1 >nul"])));
        }
        // a restart waits for nothing (on Windows, for its runs' wrappers to start: moments)
        let t = Instant::now();
        while let Some(why) = x.restart_blocker(&x.leases.lock().unwrap()) {
            assert!(cfg!(windows) && why.contains(" is starting on ") && t.elapsed() < Duration::from_secs(10), "{why}");
            std::thread::sleep(Duration::from_millis(20));
        }
        let y = x.restart(Opts { scopes: Some(false), ..Opts::default() });
        let (out, code) = log_until_exit(&y, "L4", n);
        assert_eq!((out.trim(), code), ("done", 7));
        let (out, code) = log_until_exit(&y, "G4", gate_n);
        assert_eq!((out.trim(), code), ("gate", 0));
        y.handle("lease-end", "G4 released");
        assert!(os::same(app, started), "the live app runs on");
        assert!(y.handle("lease-beat", "L4")[0].contains("\"live\":[\"L4\"]"));
        // its job, taken back: a process of it is the lease's, and a kill of it ends that one alone
        #[cfg(windows)]
        {
            let members = || os::job_pids(&y.leases.lock().unwrap()["K4"].runs[0].hold);
            let pings = || members().into_iter().filter(|p| os::exe_name(*p).is_some_and(|e| e.to_ascii_lowercase().starts_with("ping"))).collect::<Vec<u32>>();
            let ping = loop {
                if let &[one, _] = &pings()[..] { break one; }
                std::thread::sleep(Duration::from_millis(20));
            };
            assert_eq!(y.handle("lease-kill", &format!("K4 {ping}")), vec![format!("{{\"killed\":{ping}}}")]);
            while members().contains(&ping) { std::thread::sleep(Duration::from_millis(20)); }
            assert_eq!(pings().len(), 1, "the other process of the run lives on: {:?}", members());
            let e = y.handle("lease-end", "K4 released");
            assert!(e[0].contains("\"leak\":[]"), "{e:?}");
        }
        let e = y.handle("lease-end", "L4 released");
        assert!(e[0].contains(&format!("{app}")), "{e:?}");
        assert!(!os::same(app, started), "its lease's end ended it");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A Windows run's job outlives its hub's handle to it (the hub's exit: its wrapper holds it
    /// open), and a hub takes it back by its run's exit file: its processes are counted and ended
    /// through it. A run whose processes all ended has no job left to take back.
    #[cfg(windows)]
    #[test]
    fn a_windows_runs_job_outlives_its_hub_and_is_taken_back_by_name() {
        if os::no_image_roles() { return; }
        let base = std::env::temp_dir().join(format!("devhub-exec-job-name-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let exit = exit_file(&base, "J1", 1);
        let argv = os::metered_argv(SLEEP.iter().map(|s| s.to_string()).collect(), None, &exit.with_extension("usage"));
        let mut cmd = std::process::Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        let (mut child, hold) = os::lease_spawn(&mut cmd, Some(&exit)).unwrap();
        let (pid, start) = (child.id(), os::proc_start(child.id()).unwrap());
        assert!(os::lease_spawn(&mut std::process::Command::new(SLEEP[0]), Some(&exit)).is_err(), "one live job per name");
        // the hub's handles gone, as at its exit (which waits for the wrapper's start: its pid file;
        // a wrapper that ended first never writes it: this waited 30 min of a CI leg, 11-04)
        while !exit.with_extension("usage.pid").exists() {
            assert_eq!(child.try_wait().unwrap(), None, "the wrapper ended without starting its run");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(hold);
        drop(child);
        std::thread::sleep(Duration::from_millis(200));
        assert!(os::same(pid, start), "the run outlives its hub's handle");
        let back = os::Hold::taken_back(&exit);
        assert!(os::job_pids(&back).contains(&pid), "{:?}", os::job_pids(&back));
        assert!(os::group_alive(pid, start, &back));
        os::signal_group(pid, true, &back);
        while os::group_alive(pid, start, &back) { std::thread::sleep(Duration::from_millis(20)); }
        assert!(!os::same(pid, start));
        drop(back);
        assert!(os::job_pids(&os::Hold::taken_back(&exit)).is_empty(), "no job left of an ended run");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A hub that takes a run back reads its exit while the old hub's waiter writes it again: it
    /// reads the code, never an empty file (which, the wrapper gone, made the run lost).
    #[test]
    fn a_rewritten_exit_is_never_read_empty() {
        let base = std::env::temp_dir().join(format!("devhub-exec-exit-rewrite-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: 0, ended_ms: AtomicU64::new(0), n: 1, pid: 0, start: 0, unit: None, log: base.join("run-1.log"), exit: Mutex::new(None), ours: true, detachable: false, hold: os::Hold::none(), exit_file: exit_file(&base, "L1", 1), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) });
        run.finished(7);
        let writer = { let run = run.clone(); std::thread::spawn(move || for _ in 0..2000 { run.finished(7) }) };
        let mut failed: BTreeMap<String, u32> = BTreeMap::new();
        while !writer.is_finished() {
            match read_whole_or(&run.exit_file) {
                Ok(t) if t.trim() == "7" => {}
                Ok(t) => *failed.entry(format!("read {t:?}")).or_default() += 1,
                Err(e) => *failed.entry(format!("{:?} {e}", e.raw_os_error())).or_default() += 1,
            }
        }
        writer.join().unwrap();
        assert!(failed.is_empty(), "reads that were not the whole exit code: {failed:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The self-update's ask answers while a timed lease runs (`waits:`, no build beside it), and
    /// leaves the lease start and the leases free: it once asked its own leases lock again and hung
    /// with both held (every lease start and end on that box waited, 11-03).
    #[cfg(all(unix, feature = "checkout"))]
    #[test]
    fn a_self_update_ask_beside_a_timed_lease_answers_and_holds_nothing() {
        let (x, base, _) = test_exec("selfupdate-timed", Opts { scopes: Some(false), ..Opts::default() });
        assert!(!x.handle("lease-start", r#"{"id":"T1","lane":"timedx","kind":"timed","rev":null}"#)[0].contains("\"err\""));
        let (tx, rx) = std::sync::mpsc::channel();
        let y = x.clone();
        std::thread::spawn(move || { let _ = tx.send(y.handle("hub-restart", "")); });
        let answer = rx.recv_timeout(Duration::from_secs(60)).expect("the self-update ask answers");
        assert!(answer[0].contains("waits: a timed lease runs"), "{answer:?}");
        assert!(x.starting.try_lock().is_ok() && x.leases.try_lock().is_ok(), "nothing stays held");
        x.handle("lease-end", "T1 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A timed lease does not start beside a build on its box, whatever the master last saw there:
    /// refused, it leaves no mark an apply would wait for, and it starts once the build is over.
    #[cfg(all(unix, feature = "checkout"))]
    #[test]
    fn a_timed_lease_does_not_start_beside_a_build_on_its_box() {
        let (x, base, _) = test_exec("timed-builds", Opts { scopes: Some(false), ..Opts::default() });
        *x.hub_build.lock().unwrap() = HubBuild::Running { since: Instant::now() };
        let start = r#"{"id":"T1","lane":"timedb","kind":"timed","rev":null}"#;
        let answer = x.handle("lease-start", start);
        assert!(answer[0].contains(super::super::TIMED_AFTER), "{answer:?}");
        assert!(!base.join("quiet/T1.cmd").exists(), "a refused lease leaves no quiet mark");
        assert!(!x.leases.lock().unwrap().contains_key("T1"));
        let other = x.handle("lease-start", r#"{"id":"B1","lane":"timedb","kind":"test","rev":null}"#);
        assert!(!other[0].contains("\"err\""), "other work beside the build is the master's to place: {other:?}");
        x.handle("lease-end", "B1 released");
        *x.hub_build.lock().unwrap() = HubBuild::None;
        let answer = x.handle("lease-start", start);
        assert!(!answer[0].contains("\"err\""), "{answer:?}");
        assert!(base.join("quiet/T1.cmd").exists(), "it holds the box's builds off now");
        x.handle("lease-end", "T1 released");
        assert!(!base.join("quiet/T1.cmd").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An orphan sweep ends only its own hub's gone leases' processes: never a live lease's, and
    /// never another Exec's (a devhub test's on the box), even orphaned (a gate's wrapper after the
    /// hub's self-update). Its lease's end still ends it, by its mark.
    #[cfg(unix)]
    #[test]
    fn an_orphan_sweep_ends_only_its_own_hubs_gone_leases() {
        let (x, base, _) = test_exec("orphans-mine", Opts { scopes: Some(false), grace: Duration::from_secs(2), ..Opts::default() });
        let (other, other_base, _) = test_exec("orphans-other", Opts { scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"G2","lane":"landq-fast","kind":"gate","rev":null}"#);
        // the run leaves a background process behind: an orphan once its shell exits
        let run = x.handle("lease-run", &format!("G2 . {}", b64(&["sh", "-c", "sleep 30 & echo $!"])));
        let n = field(&run[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let (out, code) = log_until_exit(&x, "G2", n);
        assert_eq!(code, 0);
        let pid: u32 = out.trim().parse().unwrap();
        for _ in 0..2 {
            other.end_orphans();
            x.end_orphans();
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(os::pid_alive(pid), "pid {pid} of live lease G2 was ended by an orphan sweep");
        x.handle("lease-end", "G2 released");
        // (its new parent reaps it)
        let t = Instant::now();
        while os::pid_alive(pid) && t.elapsed() < Duration::from_secs(3) { std::thread::sleep(Duration::from_millis(50)); }
        assert!(!os::pid_alive(pid), "its lease's end ends pid {pid}");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&other_base);
    }
}
