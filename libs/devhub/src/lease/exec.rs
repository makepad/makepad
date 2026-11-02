//! The node side of leases: `lease-start`, `lease-run`, `lease-log`, `lease-end`, `lease-beat`,
//! `policy`, `caps`, `disk`, `recover-get` / `recover-del` on the node's master endpoint
//! (LEASE.SPEC.md "Wire").
//!
//! - Overlay leases mount pinned RELAXGIT stores at one virtual `<work>/dev/src` and keep edits
//!   in `<work>/dev/overlay/<lane>`. Their real cwd is an empty lane run directory. They never
//!   touch either tree. The hub resolves the pinned dev JIT; builds belong to its shared store.
//!   Resolution runs on the lease's worker; `lease-run` returns `pending` until it can report
//!   the started run or its error. Polling never starts the same resolution twice.
//!   Different pins of one lane wait for one another; equal pins share its overlay. CI overlays
//!   are keyed per pin.
//! - Pinned lane tree leases use `<work>/native/<lane>` and `<work>/native-target/<lane>`.
//!   CI, gates, landing/CI lanes and user boxes keep `<work>/ci-tree` and `<work>/ci-target`
//!   ([`super::tree`]). Holds and cleanup are per tree; native trees are capped and reclaimed LRU.
//!   Native JIT runs reuse this box's content-matching JIT store. Without a rev, a
//!   non-CI lease uses `<work>/lane/<lane>/src` and `<work>/target/lane-<lane>`. Scratch is
//!   `<work>/tmp/<id>` (TMPDIR, run logs). The deployment checkout belongs only to sync apply.
//! - A run passes argv literally, in its own session (setsid), under
//!   `systemd-run --user --scope` when the user bus exists, `nice -n 10`, optionally pinned
//!   (`cpus.*` in lease.conf). Its wrapper (Unix) writes its exit code beside the ledger and,
//!   in a scope, its final resource counters. Its process-group leader and start time go into
//!   the ledger (`~/.makepad/devhub/leases/<id>.json`), so a restarted hub (its self-update, at
//!   any moment) takes every run back: it reports their exits and still ends what it started.
//! - End (release, master expiry, or 60 s without `lease-beat`: the node's own watchdog): TERM the
//!   recorded process groups whose leader's start time still matches, 10 s grace, KILL, stop the
//!   scopes; a survivor is a `lease_leak`. The last lease on the tree that leaves changes in it:
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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// The services this module adds to the node's master endpoint.
pub const WORDS: &[&str] = &["overlay-selftest", "overlay-status", "overlay-why", "lease-start", "lease-run", "lease-log", "lease-end", "lease-beat", "lease-jits", "lease-kill", "ship-have", "ship-put", "ship-link", "ship-done", "lease-owns", "hub-restart", "service-restart", "get-ls", "get-read", "policy", "lease-conf", "caps", "disk", "recover-get", "recover-del"];

/// Marks every process a lease's runs start ([`lease_mark`]): a process that moved to its own session (a
/// WM's apps, a detached helper) is still stopped with the lease. One that outlives its lease on
/// purpose (the check server) removes it.
const LEASE_ENV: &str = "MAKEPAD_DEVHUB_LEASE";
const LOG_CHUNK: usize = 64 << 10;
/// The value of [`LEASE_ENV`]: this hub's ledger dir and the lease (the same after a hub restart).
fn lease_mark(state: &Path, id: &str) -> String {
    format!("{}/{id}", state.display())
}
const RECOVER_CHUNK: usize = 256 << 10;
/// `get-read` chunk and `get-ls` limit (results, not trees: big artifacts stay on the node)
const GET_CHUNK: usize = 256 << 10;
const GET_MAX_FILES: usize = 20_000;
/// An update that cannot find a run boundary briefly pauses admission, never leases.
const UPDATE_WAIT: Duration = Duration::from_secs(10 * 60);
const UPDATE_PAUSE: Duration = Duration::from_secs(60);

struct HubUpdate {
    since: Instant,
    pause_until: Option<Instant>,
}

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
    /// TERM to KILL on end
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
        let (status, cpu) = os::wait_usage(&mut self.child).map_err(|e| e.to_string())?;
        self.run.cpu_finished(cpu);
        let code = os::exit_code(status);
        self.run.finished(code);
        Ok(status)
    }
}

#[cfg(feature = "checkout")]
impl Drop for WorkerRun {
    fn drop(&mut self) {
        if self.run.exit.lock().unwrap().is_none() {
            let _ = stop_runs(std::slice::from_ref(&self.run), Duration::ZERO);
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

/// Resolution may still own a shared build after its requesting lease ended. Keep the hub
/// alive through publication and process reaping, including on worker errors and panics.
#[cfg(feature = "checkout")]
struct Resolving(Arc<AtomicU64>);

#[cfg(feature = "checkout")]
impl Resolving {
    fn new(count: &Arc<AtomicU64>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count.clone())
    }
}

#[cfg(feature = "checkout")]
impl Drop for Resolving {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); }
}

#[cfg(feature = "checkout")]
struct OverlayResolution {
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
    first_run: String,
    shipped: bool,
    phases: super::prof::Phases,
    work: PathBuf,
    target: PathBuf,
    tmp: PathBuf,
    env: Vec<(String, String)>,
    /// Pinned commits (`<makepad>[,<commercial>]`) for the tree or store mounts.
    pin: Option<String>,
    /// Shipped paths; tree leases undo them at release, overlay leases retain them.
    overlay: Vec<String>,
    runs: Vec<Arc<Run>>,
    peak_ram_bytes: Option<u64>,
    beat: Instant,
}

impl Lease {
    fn tree_lane(&self) -> &str { self.warms.as_deref().unwrap_or(&self.lane) }

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
        }
        phases
    }

    fn ledger(&self) -> String {
        let runs: Vec<String> = self
            .runs
            .iter()
            .map(|r| format!("{{\"n\":{},\"pid\":{},\"start\":{},\"started_ms\":{},\"spawned_ms\":{},\"detachable\":{},\"unit\":{},\"log\":{}}}", r.n, r.pid, r.start, r.started_ms, r.spawned_ms, r.detachable, r.unit.as_deref().map_or("null".into(), json_str), json_str(&r.log.to_string_lossy())))
            .collect();
        let a = self.answer();
        // an answered start (a run) survives a hub restart: the holder's next poll gets the same run
        #[cfg(feature = "checkout")]
        let resolution = self.resolution.as_ref().and_then(|r| r.run.map(|n| (r, n))).map_or(String::new(), |(r, n)| {
            let pid = self.runs.iter().find(|run| run.n == n).map_or(0, |run| run.pid);
            format!("\"resolution\":{{\"cwd\":{},\"argv\":[{}],\"n\":{n},\"pid\":{pid},\"reported\":{}}},", json_str(&r.cwd), r.argv.iter().map(|a| json_str(a)).collect::<Vec<_>>().join(","), r.reported)
        });
        #[cfg(not(feature = "checkout"))]
        let resolution = "";
        let warms = self.warms.as_deref().map_or(String::new(), |lane| format!(",\"warms\":{}", json_str(lane)));
        format!("{{{resolution}\"id\":{},\"lane\":{}{warms},\"kind\":\"{}\",\"src\":\"{}\",\"loader\":{},\"cost\":{{\"cores\":{},\"ram_gb\":{}}},\"peak_ram_bytes\":{},\"first_run\":{},\"shipped\":{},\"phases\":{},\"pin\":{},\"overlay\":[{}],\"runs\":[{}],{}", json_str(&self.id), json_str(&self.lane), self.kind.as_str(), self.src.as_str(), self.loader.as_deref().map_or("null".into(), json_str), self.cost.cores, self.cost.ram_gb, self.peak_ram_bytes.map_or("null".into(), |n| n.to_string()), json_str(&self.first_run), self.shipped, self.phases.json(), self.pin.as_deref().map_or("null".into(), json_str), self.overlay.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), runs.join(","), &a[1..])
    }

    fn from_ledger(text: &str) -> Option<Lease> {
        let v = makepad_strict_json::parse(text.as_bytes()).ok()?;
        let s = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        let n = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(super::num).unwrap_or(0.0) as u64;
        let mut env = Vec::new();
        if let Some(makepad_strict_json::Value::Obj(o)) = v.get("env") {
            for (k, x) in o {
                env.push((k.to_string(), x.as_str()?.to_string()));
            }
        }
        let runs = v.get("runs").and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| {
            a.iter()
                .filter_map(|r| Some(Arc::new(Run { started_ms: n(r, "started_ms"), spawned_ms: n(r, "spawned_ms").max(n(r, "started_ms")), ended_ms: AtomicU64::new(0), n: n(r, "n") as u32, pid: n(r, "pid") as u32, start: n(r, "start"), unit: s(r, "unit"), log: s(r, "log")?.into(), exit: Mutex::new(None), ours: false, detachable: r.get("detachable").and_then(|v| v.as_bool()).unwrap_or(false), hold: os::Hold::none(), exit_file: PathBuf::new(), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) })))
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
                Some(OverlayResolution { cwd: s(r, "cwd")?, argv, pending: String::new(), answer: Some(format!("{{\"run\":{pid},\"n\":{n}}}")), reported: r.get("reported").and_then(|v| v.as_bool()).unwrap_or(false), run: Some(n) })
            }),
            #[cfg(feature = "checkout")]
            selftest: None,
            lane: s(&v, "lane")?,
            warms: s(&v, "warms"),
            kind: Kind::parse(&s(&v, "kind")?)?,
            src: s(&v, "src").as_deref().and_then(Src::parse).unwrap_or_default(),
            loader: s(&v, "loader"),
            cost: lease_cost(&v, Kind::parse(&s(&v, "kind")?)?),
            first_run: s(&v, "first_run").unwrap_or_default(),
            shipped: v.get("shipped").and_then(|v| v.as_bool()).unwrap_or(false),
            phases: super::prof::Phases::parse(v.get("phases")),
            work: s(&v, "work")?.into(),
            target: s(&v, "target")?.into(),
            tmp: s(&v, "tmp")?.into(),
            env,
            pin: s(&v, "pin"),
            overlay: v.get("overlay").and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|p| p.as_str().map(str::to_string)).collect()),
            runs,
            peak_ram_bytes: v.get("peak_ram_bytes").and_then(|v| v.as_u64()),
            beat: Instant::now(),
        })
    }
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

fn job_env(kind: Kind, cost: super::Cost) -> [(&'static str, u32); 4] {
    let jobs = jobs(kind, cost);
    [("MAKEPAD_RUN_JOBS", jobs), ("MAKEPAD_RUN_O3_THREADS", (jobs / 2).max(1)), ("CARGO_BUILD_JOBS", jobs), ("RUST_TEST_THREADS", jobs)]
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
    #[cfg(feature = "checkout")]
    resolving: Arc<AtomicU64>,
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
    leases: Mutex<BTreeMap<String, Lease>>,
    starting: Mutex<()>,
    /// orphans of ended leases already sent TERM ([`Exec::end_orphans`])
    orphans: Mutex<Vec<u32>>,
    pub guard: Guard,
    /// lease id -> (when measured, MB in its work + target + tmp dirs): measured off the beat
    disk_use: Arc<Mutex<std::collections::HashMap<String, (Instant, u64)>>>,
    /// a person at the node, as its WM saw them ([`super::user`])
    user: Mutex<super::user::UserTrack>,
    /// `hub-restart` was answered: this hub exits in a moment and starts no run before it does
    restarting: AtomicBool,
    /// The first valid restart request; later requests and landings do not reset its wait.
    update: Mutex<Option<HubUpdate>>,
    /// The new hub's build before `hub-restart` exits for it ([`Exec::hub_built`]).
    #[cfg(feature = "checkout")]
    hub_build: Arc<Mutex<HubBuild>>,
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

/// Where a run's exit code is written when it ends: beside the ledger, not in the lease's tmp (a
/// run stopped by the lease's end would write into the tmp being removed).
fn exit_file(state: &Path, id: &str, n: u32) -> PathBuf {
    state.join(format!("{id}-{n}.exit"))
}

impl Run {
    fn work_pid(&self) -> u32 {
        std::fs::read_to_string(self.exit_file.with_extension("usage.pid")).ok().and_then(|s| s.trim().parse().ok())
            .filter(|pid| os::in_group(*pid, self.pid, &self.hold)).unwrap_or(self.pid)
    }

    fn cpu_finished(&self, cpu: Option<f64>) {
        if let Some(cpu) = cpu {
            self.usage.lock().unwrap().update(None, Some(cpu));
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
        let end = std::fs::read_to_string(self.exit_file.with_extension("time")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or_else(|| {
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
        let _ = std::fs::write(self.exit_file.with_extension("time"), end.to_string());
        let _ = std::fs::write(&self.exit_file, code.to_string());
        *self.exit.lock().unwrap() = Some(code);
    }

    /// The exit the waiter wrote (a run of an earlier hub process).
    fn recorded_exit(&self) -> Option<i32> {
        std::fs::read_to_string(&self.exit_file).ok()?.trim().parse().ok()
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

fn user_bus() -> bool {
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
        let x = Arc::new_cyclic(|_this| Exec { #[cfg(feature = "checkout")] this: _this.clone(), #[cfg(feature = "checkout")] resolving: Default::default(), machine: machine.to_string(), publish, dev: AtomicBool::new(true), dev_req: Default::default(), conf: std::sync::RwLock::new(conf), state, opts, scopes, leases: Mutex::new(BTreeMap::new()),
            starting: Mutex::new(()), orphans: Default::default(), guard, disk_use: Default::default(), user: Default::default(), restarting: AtomicBool::new(false), update: Default::default(), #[cfg(feature = "checkout")] hub_build: Default::default() });
        #[cfg(all(windows, feature = "checkout"))]
        drop_aside();
        let mut back = Vec::new();
        for e in std::fs::read_dir(&x.state).into_iter().flatten().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|e| e == "json") {
                if let Some(mut l) = std::fs::read_to_string(&p).ok().and_then(|t| Lease::from_ledger(&t)) {
                    for r in &mut l.runs {
                        if let Some(run) = Arc::get_mut(r) {
                            run.exit_file = exit_file(&x.state, &l.id, run.n);
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
        if x.opts.tree.is_some() {
            let w = Arc::downgrade(&x);
            let _ = std::thread::Builder::new().name("devhub-toolchain".into()).spawn(move || toolchain_loop(w));
        }
        x
    }

    /// The folder a timed lease marks while it lives (`<id>.cmd`), next to the ledger
    /// (`~/.makepad/devhub/quiet`): other tenants of the machine (the mini's CI: `yield_to` in its
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
        c.tree = self.tree_key();
        c.deploy = self.deploy_key();
        c.to_json()
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
        let _ = std::fs::create_dir_all(&self.state);
        let p = self.state.join(format!("{}.json", l.id));
        let tmp = self.state.join(format!("{}.json.new", l.id));
        if std::fs::write(&tmp, l.ledger()).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
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
            "lease-end" => vec![self.end_cmd(rest)],
            "lease-beat" => vec![self.beat(rest)],
            "lease-jits" => vec![lease_jits_of(self, rest)],
            "lease-kill" => vec![lease_kill(self, rest)],
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
                c.desk_link = crate::embed::desk_link_state();
                c.dev_req = match self.dev_req.load(Ordering::Acquire) { 1 => Some(true), 2 => Some(false), _ => None };
                c.tree = self.tree_key();
                c.deploy = self.deploy_key();
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

    fn start(&self, json: &str) -> String {
        let _starting = self.starting.lock().unwrap();
        if self.restarting.load(Ordering::Acquire) { return err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine)); }
        let Ok(v) = makepad_strict_json::parse(json.as_bytes()) else { return err("lease-start: not JSON") };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let (id, lane) = (s("id"), s("lane"));
        if !valid_name(&id) || !valid_name(&lane) {
            return err("lease-start: id and lane are a-z, 0-9, - _ .");
        }
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
        // a real lease preempts the background pre-warm at once (its processes are gone before
        // the lease takes the tree, its JIT cache consistent)
        #[cfg(feature = "checkout")]
        if kind.writes() && src == Src::Tree {
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
            let prepared = super::relaxgit::prepare(w, &lane, &pin);
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
        for d in [&tmp, &work, &target] {
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
            // box share its tree and target, and a fixed per-test dir let one wipe the other's
            ("MAKEPAD_TEST_ARTIFACTS", target.join("makepad_test").join(&id).to_string_lossy().into_owned()),
            ("CARGO_NET_OFFLINE", "true".into()),
        ];
        #[cfg(target_os = "linux")]
        let law: Vec<_> = law.into_iter().chain(wm_env).collect();
        // the lease's own environment wins over what was asked
        env.retain(|(k, _)| !law.iter().any(|(l, _)| l == k));
        env.extend(law.iter().map(|(k, v)| (k.to_string(), v.clone())));
        let cost = lease_cost(&v, kind);
        for (key, value) in job_env(kind, cost) {
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
            id: id.clone(), lane: lane.clone(), warms, kind, src, loader, cost, first_run: String::new(), shipped: false, phases: Default::default(), work: work.clone(), target, tmp, env, pin, overlay: Vec::new(), runs: Vec::new(), peak_ram_bytes: None, beat: Instant::now()
        };
        let a = l.answer();
        self.save(&l);
        if kind == Kind::Timed {
            let _ = std::fs::create_dir_all(self.quiet());
            let _ = std::fs::write(self.quiet().join(format!("{id}.cmd")), format!("timed lease {id} of {lane}\n"));
        }
        self.leases.lock().unwrap().insert(id.clone(), l);
        self.guard.touch();
        self.emit("ws", Sev::Info, format!("ws_create: {id} {} for {lane} on {}: {what} {}", kind.as_str(), self.machine, work.display()));
        a
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
        let detachable = if extra.is_empty() { false } else {
            let Ok(v) = makepad_strict_json::parse(extra.as_bytes()) else { return err("lease-run: options are not JSON") };
            v.get("live").and_then(|v| v.as_bool()).unwrap_or(false)
        };
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
        #[cfg(feature = "checkout")]
        if let Some(r) = &mut l.resolution {
            if r.cwd == cwd && r.argv == argv { r.reported |= r.answer.is_some(); return r.answer(); }
            if r.answer.is_none() { return err(format!("overlay: lease {id} is resolving another cwd or argv")); }
        }
        // Existing resolutions keep advancing and reporting: they are already in flight.
        if self.runs_paused() { return "{\"pending\":\"self-update: waiting for a run boundary (at most 60 s)\"}".into(); }
        #[cfg(feature = "checkout")]
        let request = argv.clone();
        let mut run_env = Vec::new();
        let mut unset = Vec::new();
        // `env [-u K] K=V cmd` is the caller's way to set the run environment: set it on the process, never
        // run an `env` program (Windows has none)
        let argv = if argv[0] == "env" {
            let mut at = 1;
            while at < argv.len() {
                if argv[at] == "-u" && at + 1 < argv.len() { unset.push(argv[at + 1].clone()); at += 2; }
                else if let Some((k, v)) = argv[at].split_once('=') { run_env.push((k.to_string(), v.to_string())); at += 1; }
                else { break; }
            }
            if at == argv.len() { return err("lease-run: env needs a command"); }
            argv[at..].to_vec()
        } else { argv };
        if l.first_run.is_empty() { l.first_run = super::run_class(l.kind, &argv); }
        if l.src == Src::Overlay {
            #[cfg(not(feature = "checkout"))]
            { return err("overlay: this node has no bare git store"); }
            #[cfg(feature = "checkout")]
            {
                if !l.shipped { return err("overlay: ship-done must define the source set before a run"); }
                let Some(args) = super::loaders::cargo_run_args(&argv).map(|a| a.to_vec()) else { return err("overlay: run requires the JIT cargo invocation") };
                if run_env.iter().map(|(k, _)| k).chain(&unset).any(|k| matches!(k.as_str(), "MAKEPAD_RUN_VFS" | "MAKEPAD_RUN_WORKSPACE" | "MAKEPAD_RUN_EDITS" | "MAKEPAD_RUN_HOME" | "CARGO_TARGET_DIR")) { return err("overlay: source environment belongs to the lease"); }
                let (work, lane, lease, pin, requested, cores, identity) = (self.conf().work, overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or("")), l.id.clone(), l.pin.clone(), l.loader.clone(), jobs(l.kind, l.cost), l.identity.clone());
                // A warm leg reserves little and builds at nice 19 on every core: idle CPU only (Unix).
                let niced = cfg!(unix) && l.lane.starts_with(super::WARM_LANE_PREFIX);
                let cores = if niced { std::thread::available_parallelism().map_or(1, |n| n.get() as u32) } else { cores };
                let Some(this) = self.this.upgrade() else { return err("overlay: hub stopped"); };
                let cwd = cwd.to_string();
                l.phases.begin("jit", crate::proto::wall_ms());
                l.resolution = Some(OverlayResolution { cwd: cwd.clone(), argv: request, pending: "selecting".into(), answer: None, reported: false, run: None });
                self.save(l);
                // Cache, per-id and build-tree locks (and Cargo's wait) stay on the worker,
                // never on a request thread or under the lease table. Its build can outlive l.
                let resolving = Resolving::new(&self.resolving);
                let worker = std::thread::Builder::new().name("devhub-overlay".into()).spawn(move || {
                    let _resolving = resolving;
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
                        let build = super::loaders::OverlayBuild { jobs: cores, niced, run: &run, cancelled: &identity.cancelled, say: &say, pending: &pending, active: &active };
                        let selected = super::loaders::find_overlay(&work, &lane, &lease, &pin, requested.as_deref(), args.first().map_or("", String::as_str), &build)?;
                        say(&format!("devhub: the JIT is this box's dev binary {}", selected.id));
                        let mut argv = vec![selected.binary.display().to_string()];
                        if !selected.changed.is_empty() { argv.extend(["run", "relax-runner", "--bin", "relax", "--"].map(String::from)); }
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
                    let result = resolved.and_then(|cmd| this.start_run(l, Path::new(&cwd), cmd, &run_env, &unset));
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
        let cmd = RunCommand { argv, detachable, loader: None, cache: None, notices: Vec::new() };
        match self.start_run(l, rel, cmd, &run_env, &unset) {
            Ok((pid, n)) => format!("{{\"run\":{pid},\"n\":{n}}}"),
            Err(e) => err(e),
        }
    }

    fn start_run(&self, l: &mut Lease, rel: &Path, command: RunCommand, run_env: &[(String, String)], unset: &[String]) -> Result<(u32, u32), String> {
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
        let loader = if loader.is_none() && l.work == super::tree::native_root(&conf.work, l.tree_lane()) && super::loaders::cargo_run_args(&argv).is_some() {
            if let Some(key) = super::loaders::tree_id(&l.work) {
                let store = super::jitstore::Store::new(&conf.work);
                store.hold(&key, &l.id)?;
                store.lookup(&key).map(|p| (key, p))
            } else { None }
        } else { loader };
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
        let argv = os::metered_argv(argv, unit.as_deref(), &exit_file(&self.state, &id, n).with_extension("usage"));
        let full = os::lease_argv(argv, unit.as_deref(), cpus, l.kind.shared().then_some(l.cost));
        let err_out = match out.try_clone() {
            Ok(f) => f,
            Err(e) => return Err(e.to_string()),
        };
        let mut cmd = std::process::Command::new(&full[0]);
        cmd.args(&full[1..]).current_dir(if l.src == Src::Overlay { l.work.clone() } else { l.work.join(rel) }).stdin(std::process::Stdio::null()).stdout(out).stderr(err_out);
        self.run_env(&mut cmd, l, run_env, unset, overlay_loader.as_deref().or_else(|| loader.as_ref().map(|(_, p)| p.as_path())));
        let (mut child, hold) = match os::lease_spawn(&mut cmd, l.kind.shared().then_some(l.cost)) {
            Ok(c) => c,
            Err(e) => return Err(format!("lease-run {id}: {}: {e}", full[0])),
        };
        let pid = child.id();
        let run = Arc::new(Run { started_ms: crate::proto::wall_ms(), spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit, log, exit: Mutex::new(None), ours: true, detachable, hold, exit_file: exit_file(&self.state, &id, n), cache: Mutex::new(cache), usage: Mutex::new(Usage::default()) });
        l.runs.push(run.clone());
        self.save(l);
        let _ = std::thread::Builder::new().name("devhub-lease-wait".into()).spawn(move || {
            let code = match os::wait_usage(&mut child) {
                Ok((status, cpu)) => { run.cpu_finished(cpu); os::exit_code(status) }
                Err(_) => -1,
            };
            run.ended_ms.store(crate::proto::wall_ms(), Ordering::Release);
            drop(run.cache.lock().unwrap().take());
            // what this run built, kept by content while the lease still holds its tree: said in its log
            if let Some((r, t)) = &tree {
                let said = match super::loaders::publish(r, t) {
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
        for (k, v) in job_env(l.kind, l.cost) { cmd.env(k, v.to_string()); }
        for (k, v) in &l.env { cmd.env(k, v); }
        for (k, v) in env { cmd.env(k, v); }
        for k in unset { cmd.env_remove(k); }
        #[cfg(feature = "checkout")]
        if l.src == Src::Overlay || l.tmp.join("overlay-selftest.json").is_file() { cmd.env("MAKEPAD_DEVHUB_GIT", crate::sync::git::stores_dir()); }
        // what it starts carries this, in whatever session: the lease's end finds it by this
        cmd.env(LEASE_ENV, lease_mark(&self.state, &l.id));
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
        let full = os::lease_argv(argv, unit.as_deref(), &conf.cpus_shared, None);
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
        let (child, hold) = os::lease_spawn(&mut cmd, None).map_err(|e| format!("dev JIT build: {e}"))?;
        let pid = child.id();
        let run = Arc::new(Run { started_ms: 0, spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n: 0, pid, start: os::proc_start(pid).unwrap_or(0), unit, log: log.clone(), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: tmp.join("cargo.exit"), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) });
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

    /// Lease ownership for the selftest's fixture runs.
    /// These continue an admitted run: pausing its worker would prevent its run boundary.
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
        let full = os::lease_argv(argv, unit.as_deref(), cpus, l.kind.shared().then_some(l.cost));
        let out = std::fs::File::create(&log).map_err(|e| format!("{}: {e}", log.display()))?;
        let mut cmd = std::process::Command::new(&full[0]);
        cmd.args(&full[1..]).current_dir(build.get_current_dir().ok_or("lease worker process has no cwd")?)
            .stdin(std::process::Stdio::null()).stdout(out.try_clone().map_err(|e| e.to_string())?).stderr(out);
        self.run_env(&mut cmd, l, env, unset, None);
        for (k, v) in build.get_envs() {
            match v { Some(v) => { cmd.env(k, v); }, None => { cmd.env_remove(k); } }
        }
        let (child, hold) = os::lease_spawn(&mut cmd, l.kind.shared().then_some(l.cost)).map_err(|e| format!("lease worker process: {e}"))?;
        let pid = child.id();
        let run = Arc::new(Run { started_ms: 0, spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit, log: log.clone(), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: exit_file(&self.state, id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) });
        l.runs.push(run.clone());
        self.save(l);
        drop(leases);
        Ok(WorkerRun { child, run, pid_file: None })
    }

    /// Pause only new run requests, for at most a minute even if the master stops asking.
    /// A long run that outlasts the pause keeps running; admission resumes for another ten minutes.
    fn runs_paused(&self) -> bool {
        let mut update = self.update.lock().unwrap();
        let Some(u) = update.as_mut() else { return false };
        let now = Instant::now();
        let line = if u.pause_until.is_some_and(|until| now >= until) {
            u.pause_until = None;
            u.since = now;
            Some(format!("hub: {} found no self-update boundary in {} s; new runs resume, retrying in {} min", self.machine, UPDATE_PAUSE.as_secs(), UPDATE_WAIT.as_secs() / 60))
        } else if u.pause_until.is_none() && now.duration_since(u.since) >= UPDATE_WAIT {
            u.pause_until = Some(now + UPDATE_PAUSE);
            Some(format!("hub: {} waited {} min for its self-update; pausing new runs for up to {} s for a boundary (new leases continue)", self.machine, UPDATE_WAIT.as_secs() / 60, UPDATE_PAUSE.as_secs()))
        } else { None };
        let paused = u.pause_until.is_some();
        drop(update);
        if let Some(line) = line { self.emit("hub", Sev::Info, line); }
        paused
    }

    /// `hub-restart`: the self-update, at once. A restart ends nothing: every run lives in its
    /// own session (scope) and its wrapper writes its exit code beside the ledger as it ends
    /// ([`os::metered_argv`]); leases, their runs and answered overlay resolutions are in the
    /// ledger, and the new hub takes them back (a holder rides through on `restarting:` and
    /// connection errors). A resolution still selecting or building is asked again by its
    /// holder's next poll; its store build goes on and the new one waits on it.
    /// Short refusals (`waits:`) remain: a sync apply, an in-hub selftest, and on Windows (a run is
    /// a job object that ends with its hub) any non-live run or resolution.
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
        // into a `cargo run` that failed, and its keeper retried the same tree
        #[cfg(feature = "checkout")]
        match self.hub_built(&root, &now) {
            Some(Ok(())) => {}
            Some(Err(why)) => return err(format!("the new hub does not build at {}: {why}; {} keeps running its hub at {}", &now[..now.len().min(10)], self.machine, &super::caps::hub_rev()[..super::caps::hub_rev().len().min(10)])),
            None => return err(format!("waits: {}'s new hub builds at {} first", self.machine, &now[..now.len().min(10)])),
        }
        self.restarting.store(true, Ordering::Release);
        drop(leases);
        self.emit("hub", Sev::Info, format!("hub: {} restarts for the self-update (was at {})", self.machine, &super::caps::hub_rev()[..super::caps::hub_rev().len().min(10)]));
        let _ = std::thread::Builder::new().name("devhub-hub-restart".into()).spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            std::process::exit(0);
        });
        format!("{{\"restarting\":{}}}", json_str(&self.machine))
    }

    /// What a restart of this hub would lose now ([`Exec::hub_restart`]), if anything.
    fn restart_blocker(&self, leases: &BTreeMap<String, Lease>) -> Option<String> {
        // an in-hub selftest summary (pid 0) has no process to take back
        let selftest: Vec<&str> = leases.values().filter(|l| l.runs.iter().any(|r| r.pid == 0 && r.live())).map(|l| l.id.as_str()).collect();
        if !selftest.is_empty() {
            return Some(format!("the overlay selftest of {} runs in {}'s hub", selftest.join(" "), self.machine));
        }
        if cfg!(windows) {
            self.update.lock().unwrap().get_or_insert_with(|| HubUpdate { since: Instant::now(), pause_until: None });
            #[cfg(feature = "checkout")]
            if self.resolving.load(Ordering::Acquire) > 0 {
                self.runs_paused();
                return Some(format!("a JIT resolution or store build is in flight on {} (the hub restarts at its end)", self.machine));
            }
            let live: Vec<&str> = leases.values().filter(|l| l.runs.iter().any(|r| !r.detachable && r.live())).map(|l| l.id.as_str()).collect();
            if !live.is_empty() {
                self.runs_paused();
                return Some(format!("a run of {} is in flight on {} (the hub restarts at its end)", live.join(" "), self.machine));
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
    /// that moves under the build drops the result (the next ask builds again).
    #[cfg(feature = "checkout")]
    fn hub_built(&self, root: &Path, at: &str) -> Option<Result<(), String>> {
        let mut b = self.hub_build.lock().unwrap();
        match &*b {
            HubBuild::Done { at: a, ok } if a == at => return Some(ok.clone()),
            // one build at a time (cargo would queue a second on its lock anyway)
            HubBuild::Running { .. } => return None,
            _ => {}
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

    fn beat(&self, rest: &str) -> String {
        let samples: Vec<_> = {
            let leases = self.leases.lock().unwrap();
            rest.split_whitespace().filter_map(|id| leases.get(id).map(|l| (id.to_string(), l.runs.clone()))).collect()
        };
        let samples: BTreeMap<_, _> = samples.into_iter().map(|(id, runs)| (id, sample_runs(&runs))).collect();
        let mut leases = self.leases.lock().unwrap();
        let (mut live, mut unknown, mut info) = (Vec::new(), Vec::new(), Vec::new());
        for id in rest.split_whitespace() {
            match leases.get_mut(id) {
                Some(l) => {
                    l.beat = Instant::now();
                    if let Some(peak) = samples.get(id).copied().flatten() {
                        if l.peak_ram_bytes.is_none_or(|old| peak > old) {
                            l.peak_ram_bytes = Some(peak);
                            self.save(l);
                        }
                    }
                    live.push(json_str(id));
                    // what it runs now (its newest live process) and its disk use, for the panel
                    let running = l.runs.iter().rev().find(|r| r.pid != 0 && os::proc_start(r.pid) == Some(r.start)).and_then(|r| { let pid = r.work_pid(); crate::top::label(pid).map(|c| (pid, c)) });
                    let dirs = vec![l.work.clone(), l.target.clone(), l.tmp.clone()];
                    let mb = self.disk_of(id, dirs);
                    info.push(format!(
                        "{}:{{\"running\":{},\"pid\":{},\"work\":{},\"disk_mb\":{},\"class\":{},\"phases\":{}}}",
                        json_str(id),
                        running.as_ref().map_or("null".into(), |(_, c)| json_str(c)),
                        running.as_ref().map_or(0, |(p, _)| *p),
                        json_str(&l.work.to_string_lossy()),
                        mb.map_or("null".into(), |m| m.to_string()),
                        json_str(&l.first_run),
                        l.profile(crate::proto::wall_ms(), false).json()
                    ));
                }
                None => unknown.push(json_str(id)),
            }
        }
        format!("{{\"live\":[{}],\"unknown\":[{}],\"info\":{{{}}}}}", live.join(","), unknown.join(","), info.join(","))
    }

    /// A lease's disk use in MB, measured at most every 2 min on a thread of its own (a target
    /// dir walk can take seconds); None until the first measure.
    fn disk_of(&self, id: &str, dirs: Vec<PathBuf>) -> Option<u64> {
        let mut m = self.disk_use.lock().unwrap();
        let known = m.get(id).copied();
        if known.is_none_or(|(at, _)| at.elapsed() > std::time::Duration::from_secs(120)) {
            // mark it measured now, so one walk runs at a time
            m.insert(id.to_string(), (Instant::now(), known.map_or(u64::MAX, |k| k.1)));
            let (map, id) = (self.disk_use.clone(), id.to_string());
            let _ = std::thread::Builder::new().name("devhub-lease-du".into()).spawn(move || {
                let mut seen = std::collections::HashSet::new();
                let bytes: u64 = dirs.iter().filter(|d| seen.insert((*d).clone())).map(|d| super::disk::dir_size(d).0).sum();
                map.lock().unwrap().insert(id, (Instant::now(), bytes >> 20));
            });
        }
        known.map(|k| k.1).filter(|m| *m != u64::MAX)
    }

    /// `lease-end <id> <how> [{"inspect":{"pid":N,"breaks":[..],"probes":[..],"paused":true}}]`
    fn end_cmd(&self, rest: &str) -> String {
        let mut w = rest.splitn(3, ' ');
        let (id, how, extra) = (w.next().unwrap_or(""), w.next().unwrap_or("released"), w.next().unwrap_or("").trim());
        if !valid_name(id) || !matches!(how, "released" | "expired" | "holder-dead" | "denied") {
            return err("usage: lease-end <id> released|expired|holder-dead|denied [inspect json]");
        }
        let mut inspect = String::new();
        if !extra.is_empty() {
            let Ok(v) = makepad_strict_json::parse(extra.as_bytes()) else { return err("lease-end: the inspect part is not JSON") };
            if let Some(i) = v.get("inspect") {
                let list = |k: &str| i.get(k).and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect::<Vec<_>>());
                let pid = i.get("pid").and_then(super::num).unwrap_or(0.0) as u32;
                let done = inspect_release(pid, &list("breaks"), &list("probes"), i.get("paused").and_then(|x| x.as_bool()).unwrap_or(false));
                self.emit("lease", Sev::Info, format!("inspect: lease {id} released pid {pid} on {}: {}", self.machine, if done.is_empty() { "nothing held".to_string() } else { done.join(", ") }));
                inspect = format!(",\"inspect\":[{}]", done.iter().map(|d| json_str(d)).collect::<Vec<_>>().join(","));
            }
        }
        let l = self.leases.lock().unwrap().remove(id);
        let Some(l) = l else {
            return format!("{{\"ended\":{},\"killed\":[],\"freed_mb\":0,\"recovered\":null,\"leak\":[],\"unknown\":true{inspect}}}", json_str(id));
        };
        let e = self.end(l, how);
        format!("{}{inspect}}}", &e[..e.len() - 1])
    }

    /// Ends a lease taken out of the table: processes, scratch, workspace; the answer line.
    fn end(&self, mut l: Lease, how: &str) -> String {
        let ended = crate::proto::wall_ms();
        if let Some(peak) = sample_runs(&l.runs) { l.peak_ram_bytes = Some(l.peak_ram_bytes.unwrap_or(0).max(peak)); }
        #[cfg(feature = "checkout")]
        l.identity.cancelled.store(true, Ordering::Release);
        // what the next lease needs first: the processes stopped, the tree let go; the rest (its scratch
        // dir measured and removed, the disk guard) after the answer, in the background
        let (mut killed, mut leak) = stop_runs(&l.runs, self.opts.grace);
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
        let _ = std::fs::remove_file(self.state.join(format!("{}.json", l.id)));
        let profile = l.profile(ended, true);
        for r in &l.runs {
            let _ = std::fs::remove_file(r.exit_file.with_extension("time"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("usage"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("usage.pid"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("usage.cgroup"));
            let _ = std::fs::remove_file(r.exit_file.with_extension("cpu"));
            let _ = std::fs::remove_file(&r.exit_file);
        }
        let sev = if how == "released" { Sev::Info } else { Sev::Warn };
        self.emit("lease", sev, format!("lease: {} {} of {} ended on {} ({how}): {} process group(s) stopped", l.id, l.kind.as_str(), l.lane, self.machine, killed.len()));
        self.guard.touch();
        let (tmp, emit, tag) = (l.tmp.clone(), self.emitter(), format!("{} {} on {}", l.id, l.lane, self.machine));
        #[cfg(feature = "checkout")]
        let (this, overlay) = (self.this.clone(), (l.kind == Kind::Ci && l.src == Src::Overlay && leak.is_empty()).then(|| overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or(""))).filter(|lane| !lane.is_empty() && lane != &l.lane));
        let _ = std::thread::Builder::new().name("devhub-lease-cleanup".into()).spawn(move || {
            let mut freed = 0u64;
            if tmp.exists() {
                freed += disk::dir_size(&tmp).0;
                let _ = std::fs::remove_dir_all(&tmp);
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
        });
        let freed = 0u64;
        if !leak.is_empty() {
            self.emit("lease", Sev::Err, format!("lease_leak: {} {} on {}: pid(s) {:?} survived KILL", l.id, l.lane, self.machine, leak));
        }
        let line = self.ended_line(&l.id, &killed, freed, &recovered, &leak);
        format!("{},\"class\":{},\"phases\":{},\"peak_ram_gb\":{},\"cpu_seconds\":{}}}", line.trim_end_matches('}'), json_str(&l.first_run), profile.json(), l.peak_ram_bytes.map_or("null".into(), |n| (n as f64 / (1u64 << 30) as f64).to_string()), cpu.map_or("null".into(), |n| n.to_string()))
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

    /// Watches detached runs and ends leases that have not been beaten for `no_beat`.
    fn expire(&self) {
        let stale: Vec<Lease> = {
            let mut leases = self.leases.lock().unwrap();
            for l in leases.values() {
                for r in &l.runs { r.adopted_exit(); }
            }
            let ids: Vec<String> = leases.values().filter(|l| l.beat.elapsed() >= self.opts.no_beat).map(|l| l.id.clone()).collect();
            ids.iter().filter_map(|id| leases.remove(id)).collect()
        };
        for l in stale {
            self.emit("lease", Sev::Warn, format!("lease: {} {} of {} ended by {} itself: no beat from the master for {} s", l.id, l.kind.as_str(), l.lane, self.machine, self.opts.no_beat.as_secs()));
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
                if let Err(e) = super::relaxgit::finish(&self.conf().work, &overlay_lane(l.kind, &l.lane, &pin.key()), &pin, &l.tmp.join("ship"), &shipped, &deleted, bases) { return err(e); }
                l.overlay = shipped.into_iter().chain(deleted).collect();
                l.shipped = true;
                self.save(l);
                return format!("{{\"overlay\":{}}}", l.overlay.len());
            }
            #[cfg(not(feature = "checkout"))]
            return err("overlay: RELAXGIT layer unavailable");
        }
        let mut removed = 0;
        for f in &deleted {
            let p = l.work.join(f);
            if std::fs::symlink_metadata(&p).is_ok_and(|m| !m.is_dir()) && std::fs::remove_file(&p).is_ok() {
                removed += 1;
                prune_empty_parents(&l.work, f);
            }
        }
        for p in shipped.into_iter().chain(deleted) {
            if !l.overlay.contains(&p) {
                l.overlay.push(p);
            }
        }
        self.save(l);
        format!("{{\"overlay\":{},\"deleted\":{removed}}}", l.overlay.len())
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
            if self.runs_paused() { return "{\"pending\":\"self-update: waiting for a run boundary (at most 60 s)\"}".into(); }
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
            let summary = Arc::new(Run { started_ms: crate::proto::wall_ms(), spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid: 0, start: 0, unit: None, log, exit: Mutex::new(None), ours: true, detachable: false, hold: os::Hold::none(), exit_file: exit_file(&self.state, id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) });
            l.runs.push(summary.clone());
            l.first_run = "overlay-selftest".into();
            self.save(l);
            let (work, tmp, lease, case, identity, cores) = (self.conf().work, l.tmp.clone(), l.id.clone(), case.to_string(), l.identity.clone(), jobs(l.kind, l.cost));
            let completion = summary.clone();
            let resolving = Resolving::new(&self.resolving);
            match std::thread::Builder::new().name("devhub-overlay-selftest".into()).spawn(move || {
                let _resolving = resolving;
                let active = || if identity.cancelled.load(Ordering::Acquire) { Err(format!("selftest: lease {lease} ended")) } else { Ok(()) };
                let say = |line: &str| {
                    use std::io::Write as _;
                    let mut output = output.lock().unwrap();
                    let _ = writeln!(output, "{line}");
                    let _ = output.flush();
                };
                let run = |id: &str, cmd: &std::process::Command, active: &dyn Fn() -> Result<(), String>| this.overlay_build(id, cmd, active);
                let build = super::loaders::OverlayBuild { jobs: cores, niced: false, run: &run, cancelled: &identity.cancelled, say: &say, pending: &say, active: &active };
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
            let result = if word == "overlay-why" { super::relaxgit::why(&work, lane, path).map(|l| vec![l]) } else { super::relaxgit::status(&work, lane) };
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
        let _starting = self.starting.lock().unwrap();
        if !makepad_devhub_conf::valid_name(lane) {
            return err(format!("bad lane name `{lane}`"));
        }
        if let Some(l) = self.leases.lock().unwrap().values().find(|l| l.lane == lane || (l.src == Src::Tree && l.tree_lane() == lane)) {
            return err(format!("lane {lane} holds live lease {}", l.id));
        }
        let conf = self.conf();
        let w = &conf.work;
        let paths = [w.join("dev/lane").join(lane), w.join("dev/overlay").join(lane), w.join("lane").join(lane), w.join("target").join(format!("lane-{lane}")), w.join("target").join(format!("lane-{lane}.last")), caps::home_dir().join("rc/pool").join(lane)];
        #[cfg(feature = "checkout")]
        if let Err(e) = super::relaxgit::forget(w, lane) { return err(e); }
        let mut freed = 0u64;
        let mut removed = Vec::new();
        #[cfg(feature = "checkout")]
        {
            let root = super::tree::native_root(w, lane);
            let target = super::tree::native_target(w, lane);
            if root.exists() || target.exists() {
                freed += disk::dir_size(&root).0 + disk::dir_size(&target).0;
                match super::tree::remove_native(w, lane) {
                    Ok(r) => { self.report_tree(&format!("cleaning native lane {lane}"), &r); removed.push(json_str(&root.to_string_lossy())); removed.push(json_str(&target.to_string_lossy())); }
                    Err(e) => return err(e),
                }
            }
        }
        for p in paths.iter().filter(|p| p.exists()) {
            freed += super::disk::dir_size(p).0;
            match super::disk::remove(p) {
                Ok(()) => removed.push(json_str(&p.to_string_lossy())),
                Err(e) => return err(format!("{}: {e}", p.display())),
            }
        }
        self.guard.touch();
        (self.publish)(crate::proto::hub_record(&self.machine, "", 0, "disk", crate::proto::Sev::Info, format!("disk: lane {lane} landed: {} dirs removed, {} MB freed", removed.len(), freed >> 20), String::new()));
        format!("{{\"cleaned\":{},\"removed\":[{}],\"freed_mb\":{}}}", json_str(lane), removed.join(","), freed >> 20)
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
    /// self-update, in the gate's own devhub tests (no verdict on a node, 11-02); a process that
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

/// A box whose toolchain cannot link takes no leases until it can (`Caps::sick`): it is said once.
fn toolchain_loop(w: Weak<Exec>) {
    os::nice_this_thread();
    loop {
        let Some(x) = w.upgrade() else { return };
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
        std::thread::sleep(TOOLCHAIN_EVERY);
    }
}

fn disk_loop(w: Weak<Exec>) {
    os::nice_this_thread();
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let Some(x) = w.upgrade() else { return };
        x.guard.check(&|| x.live());
    }
}

/// TERM the runs' groups, `grace`, KILL, stop their scopes: (stopped leaders, survivors).
fn stop_runs(runs: &[Arc<Run>], grace: Duration) -> (Vec<u32>, Vec<u32>) {
    let alive: Vec<&Arc<Run>> = runs.iter().filter(|r| r.start != 0 && os::group_alive(r.pid, r.start, &r.hold)).collect();
    for r in &alive {
        os::signal_group(r.pid, false, &r.hold);
    }
    let t = Instant::now();
    while t.elapsed() < grace && alive.iter().any(|r| os::group_alive(r.pid, r.start, &r.hold)) {
        std::thread::sleep(Duration::from_millis(100));
    }
    for r in alive.iter().filter(|r| os::group_alive(r.pid, r.start, &r.hold)) {
        r.sample();
        os::signal_group(r.pid, true, &r.hold);
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

/// Stops what the lease's runs started and their groups did not take with them: TERM, looking again
/// while the grace lasts (a WM on its way out may still start an app), then KILL. (stopped, left)
fn stop_strays(mark: &str, grace: Duration) -> (Vec<u32>, Vec<u32>) {
    let marker = format!("{LEASE_ENV}={mark}");
    let me = std::process::id();
    let find = || os::with_env(&marker).into_iter().filter(|p| *p != me).collect::<Vec<u32>>();
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
    let Some(app) = makepad_remote::discovery::apps().into_iter().find(|a| a.pid == pid && !a.socket.starts_with("tcp:")) else {
        return vec![format!("pid {pid}: no JIT")];
    };
    let client = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(app.socket.into()), "devhub");
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
/// restart's wait. A headless lease (live test sessions among them) runs on through it: 102's
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

/// Whether `pid` (a JIT app) runs in a headless lease here, which a WM restart leaves alone: in
/// the process group of a run of a lease not on the WM ([`on_the_wm`]), with no WM socket in its
/// environment. A launch adopted into a lease, or an app a shell started, gets no such pass.
/// `lease-jits <id>`: the JITs running in the lease's process groups (a stalled lease's master
/// asks each to capture its state).
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
/// the session stayed "recovering" (W645, W674, 11-02).
fn lease_kill(x: &Exec, rest: &str) -> String {
    let mut w = rest.split_whitespace();
    let (Some(id), Some(pid)) = (w.next(), w.next().and_then(|p| p.parse::<u32>().ok())) else { return err("lease-kill <id> <pid>") };
    let Some(runs) = x.leases.lock().unwrap().get(id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    let Some(run) = runs.iter().find(|r| r.pid != 0 && (r.pid == pid || os::in_group(pid, r.pid, &r.hold))) else { return err(format!("pid {pid} is no process of lease {id} on {}", x.machine)) };
    #[cfg(unix)]
    { let _ = run; os::signal_one(pid, true); }
    #[cfg(not(unix))]
    os::signal_group(pid, true, &run.hold);
    format!("{{\"killed\":{pid}}}")
}

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
    l.runs.push(Arc::new(Run { started_ms: crate::proto::wall_ms(), spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit: None, log, exit: Mutex::new(None), ours: false, detachable: false, hold: os::Hold::none(), exit_file: exit_file(&x.state, id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()) }));
    x.save(l);
    Ok(())
}

/// Whether lease `id` is live here and may launch.
/// The lane lease `id` belongs to.
pub fn lease_lane(id: &str) -> Option<String> {
    EXEC.get().and_then(|x| x.leases.lock().unwrap().get(id).map(|l| l.lane.clone()))
}

pub fn lease_live(id: &str) -> bool {
    EXEC.get().is_some_and(|x| x.leases.lock().unwrap().get(id).is_some_and(|l| l.kind.writes()))
}

pub fn register(ep: &Endpoint, ctx: &crate::sync::NodeCtx) -> Arc<Exec> {
    let conf = LeaseConf::load();
    #[cfg(feature = "checkout")]
    let opts = Opts { tree: Some(super::tree::root(&conf.work)), ..Opts::default() };
    #[cfg(not(feature = "checkout"))]
    let opts = Opts::default();
    let x = Exec::new(&ctx.machine, conf, crate::proto::home().join("leases"), opts, ctx.publish.clone());
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
    let why = match cmd.output() {
        Ok(o) if o.status.success() => return Ok(()),
        Ok(o) => crate::orch::roll::first_error(&String::from_utf8_lossy(&o.stderr)),
        Err(e) => format!("cargo: {e}"),
    };
    #[cfg(windows)]
    image_back(aside);
    Err(why)
}

/// Windows will not let a build replace a running image's file (its link fails), but renames it:
/// the hub's binary names (cargo's uplifted copy and its `deps` file, hard links of one file)
/// move aside before the build, and back when it fails. The next hub deletes them ([`drop_aside`]).
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
    names.into_iter().filter_map(|p| {
        let to = p.with_extension(&tag);
        std::fs::rename(&p, &to).ok().map(|_| (p, to))
    }).collect()
}

#[cfg(all(windows, feature = "checkout"))]
const ASIDE: &str = "aside-";

#[cfg(all(windows, feature = "checkout"))]
fn image_back(moved: Vec<(PathBuf, PathBuf)>) {
    for (name, aside) in moved {
        if !name.exists() {
            let _ = std::fs::rename(&aside, &name);
        }
    }
}

/// At start: the images an earlier hub moved aside ([`image_aside`]); the one running cannot go.
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

    fn test_exec(name: &str, opts: Opts) -> (Arc<Exec>, PathBuf, Arc<Mutex<Vec<String>>>) {
        let base = std::env::temp_dir().join(format!("devhub-exec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let conf = LeaseConf::parse(&format!("work = {}\n", base.join("devhub").display()), &base);
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let x = Exec::new("t", conf, base.join("leases"), opts, Arc::new(move |r: crate::proto::Record| ev.lock().unwrap().push(r.line)));
        (x, base, events)
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
    fn round_trip_run_log_end() {
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
        assert!(os::group_alive(pid, start, &os::Hold::none()));
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
        assert!(x.handle("lease-end", "L1 released")[0].contains("\"unknown\":true"));
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
    fn watchdog_ends_a_lease_without_beats() {
        let (x, base, events) = test_exec("wd", Opts { no_beat: Duration::from_millis(600), tick: Duration::from_millis(100), grace: Duration::from_secs(10), ..Opts::default() });
        let a = x.handle("lease-start", r#"{"id":"L7","lane":"lost","kind":"long","rev":null}"#);
        assert!(a[0].contains("\"ok\":1"));
        let r = x.handle("lease-run", &format!("L7 . {}", b64(SLEEP)));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let start = os::proc_start(pid).unwrap();
        // beaten: it stays
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(100));
            x.handle("lease-beat", "L7");
        }
        assert!(x.leases.lock().unwrap().contains_key("L7"));
        // not beaten: the node ends it and stops its process
        let t = Instant::now();
        while x.leases.lock().unwrap().contains_key("L7") || os::group_alive(pid, start, &os::Hold::none()) {
            assert!(t.elapsed() < Duration::from_secs(20), "the watchdog did not end L7");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(events.lock().unwrap().iter().any(|l| l.contains("no beat from the master")));
        // dev off refuses write kinds, not inspect
        assert_eq!(x.handle("policy", "dev=off"), vec![r#"{"dev":"off"}"#.to_string()]);
        // a flip made on the node is asked of the master with the caps answer; the master's policy push clears it
        x.dev_req.store(1, Ordering::Release);
        assert!(x.handle("caps", "caps")[0].contains("\"dev_req\":true"));
        x.handle("policy", "dev=off");
        assert!(x.handle("caps", "caps")[0].contains("\"dev_req\":null"));
        assert!(x.handle("lease-start", r#"{"id":"L8","lane":"a","kind":"build","rev":null}"#)[0].contains("dev off"));
        assert!(x.handle("lease-start", r#"{"id":"L9","lane":"a","kind":"inspect","rev":null}"#)[0].contains("\"ok\":1"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A process in a group of its own. (This binary, not `sleep`: macOS hides the environment of
    /// its platform binaries.)
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
            let mut c = std::process::Command::new(std::env::current_exe().unwrap());
            c.args(["--exact", "lease::exec::tests::lease_end_role_stray"]).env("DEVHUB_TEST_STRAY", "1").process_group(0);
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

    #[test]
    fn a_restarted_hub_takes_its_leases_back() {
        // (no scopes: a scope is named by the lease id, which other tests here use at the same time)
        let (x, base, _) = test_exec("ledger", Opts { scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"L3","lane":"b","kind":"screen","rev":null}"#);
        let r = x.handle("lease-run", &format!("L3 . {}", b64(SLEEP)));
        let pid = field(&r[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        // (the old hub gone: a watchdog of its own would end the lease it no longer beats)
        let conf = x.conf();
        drop(x);
        let y = Exec::new("t", conf, base.join("leases"), Opts { scopes: Some(false), ..Opts::default() }, Arc::new(|_| {}));
        assert!(y.handle("lease-beat", "L3")[0].contains("\"live\":[\"L3\"]"));
        let e = y.handle("lease-end", "L3 released");
        assert!(e[0].contains(&format!("\"killed\":[{pid}]")), "{e:?}");
        assert!(!os::group_alive(pid, os::proc_start(pid).unwrap_or(1), &os::Hold::none()));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The self-update restarts a hub under a running job and a live session: nothing waits and
    /// nothing ends. The new hub reads the job's exit from its wrapper (written without any hub)
    /// and still holds the live app.
    #[cfg(unix)]
    #[test]
    fn a_hub_restart_ends_nothing() {
        let (x, base, _) = test_exec("handover", Opts { scopes: Some(false), ..Opts::default() });
        // the wrapper alone records an exit, whoever waits for it
        std::fs::create_dir_all(&base).unwrap();
        let usage = base.join("wrapped.usage");
        let argv = os::metered_argv(vec!["sh".into(), "-c".into(), "exit 7".into()], None, &usage);
        assert_eq!(std::process::Command::new(&argv[0]).args(&argv[1..]).status().unwrap().code(), Some(7));
        assert_eq!(std::fs::read_to_string(base.join("wrapped.exit")).unwrap().trim(), "7");
        x.handle("lease-start", r#"{"id":"L4","lane":"b","kind":"test","rev":null}"#);
        let job = x.handle("lease-run", &format!("L4 . {}", b64(&["sh", "-c", "sleep 1; echo done; exit 7"])));
        let n = field(&job[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let live = x.handle("lease-run", &format!("L4 . {} {{\"live\":true}}", b64(SLEEP)));
        let app = field(&live[0], "run").and_then(|v| super::super::num(&v)).unwrap() as u32;
        let started = os::proc_start(app).unwrap();
        // a landing's gate lease rides through as well
        x.handle("lease-start", r#"{"id":"G4","lane":"landq-fast","kind":"gate","rev":null}"#);
        let gate = x.handle("lease-run", &format!("G4 . {}", b64(&["sh", "-c", "sleep 1; echo gate; exit 0"])));
        let gate_n = field(&gate[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32;
        assert_eq!(x.restart_blocker(&x.leases.lock().unwrap()), None, "a restart waits for nothing");
        let conf = x.conf();
        drop(x);
        let y = Exec::new("t", conf, base.join("leases"), Opts { scopes: Some(false), ..Opts::default() }, Arc::new(|_| {}));
        let (out, code) = log_until_exit(&y, "L4", n);
        assert_eq!((out.trim(), code), ("done", 7));
        let (out, code) = log_until_exit(&y, "G4", gate_n);
        assert_eq!((out.trim(), code), ("gate", 0));
        y.handle("lease-end", "G4 released");
        assert!(os::same(app, started), "the live app runs on");
        assert!(y.handle("lease-beat", "L4")[0].contains("\"live\":[\"L4\"]"));
        let e = y.handle("lease-end", "L4 released");
        assert!(e[0].contains(&format!("{app}")), "{e:?}");
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
