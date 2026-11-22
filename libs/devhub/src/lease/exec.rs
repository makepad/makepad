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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
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

pub const WORDS: &[&str] = &["overlay-selftest", "overlay-status", "overlay-why", "lease-start", "lease-run", "lease-weight", "lease-log", "lease-failed", "lease-end", "lease-beat", "lease-jits", "lease-yield", "lease-tell", "lease-kill", "lease-why", "lease-pid", "lease-box", "ship-have", "ship-put", "ship-link", "ship-done", "input-have", "input-put", "input-home", "lease-owns", "hub-restart", "service-restart", "service-health", "get-ls", "get-read", "policy", "lease-conf", "toolchain-probe", "caps", "disk", "lane-dirs", "heap", "recover-get", "recover-del", "jit-read", "jit-put", "jit-keep", "jit-unkeep"];

/// The cargo variable of this host's `target.<triple>.rustflags`: its value comes after the
/// workspace config's flags for the host target (its `-C target-cpu=native`); empty: a host whose
/// triple this build does not know (no build `--for` another box here).
const HOST_RUSTFLAGS: &str = if cfg!(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu")) {
    "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS"
} else if cfg!(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu")) {
    "CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS"
} else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
    "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS"
} else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
    "CARGO_TARGET_X86_64_APPLE_DARWIN_RUSTFLAGS"
} else if cfg!(all(target_arch = "x86_64", windows, target_env = "msvc")) {
    "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS"
} else if cfg!(all(target_arch = "aarch64", windows, target_env = "msvc")) {
    "CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_RUSTFLAGS"
} else {
    ""
};

/// Marks every process a lease's runs start ([`lease_mark`]): a process that moved to its own session (a
/// WM's apps, a detached helper) is still stopped with the lease. One that outlives its lease on
/// purpose (the check server) removes it.
use makepad_remote::discovery::LEASE_ENV;
/// The mark of the lease a hub itself runs in (a devhub test on a box), passed on to its runs: the
/// box's end of that lease finds what they left too, though they carry their own hub's mark in
/// their own session (a killed test's busy loop burned a core for four hours, 11-04).
const LEASE_OUTER_ENV: &str = "MAKEPAD_DEVHUB_LEASE_OUTER";
pub(crate) const LOG_CHUNK: usize = 64 << 10;
/// The value of [`LEASE_ENV`]: this hub's ledger dir and the lease (the same after a hub restart).
fn lease_mark(state: &Path, id: &str) -> String {
    format!("{}/{id}", state.display())
}
const RECOVER_CHUNK: usize = 256 << 10;
/// `get-read` chunk and `get-ls` limit (results, not trees: big artifacts stay on the node)
const GET_CHUNK: usize = 256 << 10;
const GET_MAX_FILES: usize = 20_000;
/// How far a file's time may read behind the wall clock: the kernel stamps it from a coarse clock
/// one tick old at most (Linux: a jiffy, 1 to 10 ms; Windows: 15.6 ms).
const STAMP_GRAIN_MS: u64 = 20;
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
    /// Watch Linux memory pressure and end the lease that exceeded its weight.
    pub watch_memory: bool,
    /// A timed lease starts after the WM's own apps on this box are quiet ([`Exec::builds_here`]),
    /// and the box's AI hub takes no remote work while it runs ([`Exec::hold_quiet`]). A test
    /// hub's box is not its own: the real WM's devhub taking a landing refused its timed leases
    /// (five exec tests red on a pool box, 11-06), and it must not close the real AI hub.
    pub watch_wm: bool,
    /// the PATH a launch's wrappers are found on ([`Exec::launch`]; None: the hub's own)
    pub path: Option<std::ffi::OsString>,
    /// what a timed start sees of its box beyond this hub's leases ([`Exec::timed_waits`]; None:
    /// the box it runs on, [`BoxLook::this_box`])
    pub box_look: Option<BoxLook>,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { tree: None, no_beat: Duration::from_secs(60), tick: Duration::from_secs(5), grace: Duration::from_secs(10), scopes: None, watch_memory: true, watch_wm: true, path: None, box_look: None }
    }
}

/// What a timed start sees of its box beyond its hub's leases ([`Exec::timed_waits`]): the box's
/// processes, and how long ago its deployment's checkout last moved under its apps. A test hub's
/// box is not its own: it saw the cargo tests that ran beside it on a pool box and refused its
/// timed starts (five exec tests red in a post-land bucket, 11-10), so a test gives it its own.
#[derive(Clone)]
pub struct BoxLook {
    /// a look at the box's processes, and the hub's pid among them
    pub procs: Arc<dyn Fn() -> (Vec<crate::top::ProcRow>, u32) + Send + Sync>,
    /// how long ago the checkout last moved under the box's apps (None: not since the hub started)
    pub moved_ago: Arc<dyn Fn() -> Option<Duration> + Send + Sync>,
    /// a Mac's per-pid GPU use, % of the GPU ([`crate::top::gpu_look`]; None off a Mac or if
    /// the registry could not be read)
    pub gpu: Arc<dyn Fn() -> Option<Vec<(u32, f64)>> + Send + Sync>,
    /// the box's CPU as its hub reports it to the master, busy % and the tasks that waited for a
    /// CPU ([`super::Caps::cpu_pct`], [`super::Caps::cpu_wait`]; None: as [`caps::measure`] and
    /// [`caps::load_now`] measure the machine). A test's six hubs on one machine each reported the
    /// whole machine's load as their box's: on a machine busy with other tests the master held
    /// the CI shards' asks out of both CI boxes while the test's other work held them, and the
    /// restart test timed out with no shard running (11-19).
    pub cpu: Arc<dyn Fn() -> Option<(f64, f64)> + Send + Sync>,
    /// what a timed lease's looks count between them ([`Noise`], [`crate::top::ticks`]), and the
    /// hub's pid among them
    #[cfg(unix)]
    pub ticks: Arc<dyn Fn() -> (crate::top::Ticks, u32) + Send + Sync>,
}

impl BoxLook {
    /// The box this hub runs on.
    pub fn this_box() -> BoxLook {
        BoxLook {
            procs: Arc::new(|| (crate::top::processes(), std::process::id())),
            moved_ago: Arc::new(|| {
                #[cfg(feature = "checkout")]
                return crate::sync::serve::moved_ago();
                #[cfg(not(feature = "checkout"))]
                None
            }),
            gpu: Arc::new(crate::top::gpu_look),
            cpu: Arc::new(|| None),
            #[cfg(unix)]
            ticks: Arc::new(|| (crate::top::ticks(), std::process::id())),
        }
    }

    /// A box with nothing on it beyond the hub's leases: a test hub's.
    #[cfg(test)]
    pub(crate) fn quiet() -> BoxLook {
        BoxLook {
            procs: Arc::new(|| (Vec::new(), 0)),
            moved_ago: Arc::new(|| None),
            gpu: Arc::new(|| None),
            cpu: Arc::new(|| Some((0.0, 0.0))),
            #[cfg(unix)]
            ticks: Arc::new(|| (crate::top::Ticks { at: Instant::now(), per_s: 100.0, procs: Default::default(), cpu: None }, 0)),
        }
    }
}

impl std::fmt::Debug for BoxLook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BoxLook")
    }
}

/// A launch started as a lease's run ([`Exec::launch`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launched {
    /// its run's number in the lease
    pub n: u32,
    /// its process group's leader: the run's wrapper, what its stop signals
    pub leader: u32,
    /// the app's process (a JIT announces itself with it)
    pub app: u32,
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
    /// stopped on purpose, on its own (a launch's `stop`): its end is no abnormal one
    stopped: AtomicBool,
    native: Option<NativeBuild>,
    build: OnceLock<Option<super::prof::Build>>,
}

/// The packages already built when this run started; saved with the run so a hub restart
/// cannot turn its cold build into a warm sample (first native runs, 11-15).
struct NativeBuild {
    mark: PathBuf,
    pkgs: Vec<String>,
    warm: bool,
}

static NATIVE_PACKAGES: Mutex<()> = Mutex::new(());

impl NativeBuild {
    fn start(src: Src, target: &Path, argv: &[String]) -> Option<Self> {
        let pkgs = Self::packages(src, argv)?;
        let mark = target.join(".devhub-built-packages");
        let _lock = NATIVE_PACKAGES.lock().unwrap();
        let built = std::fs::read_to_string(&mark).unwrap_or_default();
        let warm = !pkgs.is_empty() && pkgs.iter().all(|p| built.lines().any(|b| b == p));
        Some(Self { mark, pkgs: pkgs.into_iter().collect(), warm })
    }

    /// The packages a run's cargo build, test or run builds, past any `env` or `nice` prefix: the
    /// native prewarm's `nice -n 9 cargo test --no-run` warms the next test's build. Never a check
    /// (it builds no code) or a JIT run.
    #[cfg(feature = "checkout")]
    fn packages(src: Src, argv: &[String]) -> Option<BTreeSet<String>> {
        if src != Src::Tree || super::loaders::cargo_run_args(argv).is_some() { return None; }
        let builds = super::native::builds(argv, &|_| None);
        if builds.is_empty() { return None; }
        Some(builds.into_iter().filter(|b| b.target.is_empty() && b.pkg != "*").map(|b| b.pkg).collect())
    }

    /// (a box without the checkout applier has no work trees, so no native runs)
    #[cfg(not(feature = "checkout"))]
    fn packages(_: Src, _: &[String]) -> Option<BTreeSet<String>> { None }

    fn json(&self) -> String {
        format!("{{\"mark\":{},\"pkgs\":[{}],\"warm\":{}}}", json_str(&self.mark.to_string_lossy()), self.pkgs.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), self.warm)
    }

    fn parse(v: &makepad_strict_json::Value) -> Option<Self> {
        Some(Self { mark: v.get("mark")?.as_str()?.into(), pkgs: v.get("pkgs")?.as_arr()?.iter().map(|p| p.as_str().map(str::to_string)).collect::<Option<_>>()?, warm: v.get("warm")?.as_bool()? })
    }

    fn finished(&self, log: &Path) -> Option<super::prof::Build> {
        use std::io::Write;
        let ms = cargo_finished(std::io::BufReader::new(std::fs::File::open(log).ok()?))?;
        // Append only new packages under one lock: simultaneous runs cannot lose each other's
        // entries. The target's removal removes the set with its build output.
        let _lock = NATIVE_PACKAGES.lock().unwrap();
        let built = std::fs::read_to_string(&self.mark).unwrap_or_default();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.mark) {
            for p in &self.pkgs {
                if !built.lines().any(|b| b == p) { let _ = writeln!(f, "{p}"); }
            }
        }
        Some(super::prof::Build { ms, pkg: (self.pkgs.len() == 1).then(|| self.pkgs[0].clone()), warm: (!self.pkgs.is_empty()).then_some(self.warm) })
    }
}

/// Scan to cargo's first Finished line, with only the reader's buffer and parser state in
/// memory. Even a long diagnostic without newlines never collects the run log (11-15).
fn cargo_finished(log: impl std::io::BufRead) -> Option<u64> {
    let mut bytes = std::io::Read::bytes(log);
    let prefix = b"Finished ";
    let marker = b" target(s) in ";
    let (mut head, mut tail, mut skipped) = (0, 0, false);
    while let Some(b) = bytes.next() {
        let b = b.ok()?;
        if b == b'\n' {
            if head == prefix.len() { return None; }
            (head, tail, skipped) = (0, 0, false);
            continue;
        }
        if skipped { continue; }
        if head < prefix.len() {
            if head == 0 && matches!(b, b' ' | b'\t' | b'\r') { continue; }
            if b == prefix[head] { head += 1; } else { skipped = true; }
        } else {
            tail = if b == marker[tail] { tail + 1 } else { usize::from(b == marker[0]) };
            if tail == marker.len() { return cargo_duration(bytes); }
        }
    }
    None
}

fn cargo_duration(bytes: impl Iterator<Item = std::io::Result<u8>>) -> Option<u64> {
    fn number(bytes: &mut std::iter::Peekable<impl Iterator<Item = u8>>) -> Option<(u64, u32)> {
        let (mut n, mut digits) = (0u64, 0u32);
        while bytes.peek().is_some_and(u8::is_ascii_digit) {
            n = n.checked_mul(10)?.checked_add((bytes.next()? - b'0') as u64)?;
            digits = digits.checked_add(1)?;
        }
        (digits > 0).then_some((n, digits))
    }
    let mut bytes = bytes.map_while(Result::ok).take_while(|b| *b != b'\n').peekable();
    let (n, _) = number(&mut bytes)?;
    let ms = match bytes.next()? {
        b'.' => {
            let (fraction, digits) = number(&mut bytes)?;
            if bytes.next()? != b's' { return None; }
            n.checked_mul(1000)?.checked_add(fraction.checked_mul(1000)? / 10u64.checked_pow(digits)?)?
        }
        b'm' => {
            if bytes.next()? != b' ' { return None; }
            let (seconds, _) = number(&mut bytes)?;
            if bytes.next()? != b's' || seconds >= 60 { return None; }
            n.checked_mul(60_000)?.checked_add(seconds.checked_mul(1000)?)?
        }
        b's' => n.checked_mul(1000)?,
        _ => return None,
    };
    bytes.all(|b| b.is_ascii_whitespace()).then_some(ms)
}

#[test]
fn cargo_finished_reads_the_first_build_duration() {
    let ms = |s: &str| cargo_finished(std::io::BufReader::with_capacity(7, s.as_bytes()));
    assert_eq!(ms("    Finished `release` profile [optimized] target(s) in 0.52s\n"), Some(520), "fractional seconds across buffer boundaries");
    assert_eq!(ms("    Finished `test` profile [optimized] target(s) in 14m 05s\n"), Some(845_000), "minutes and seconds");
    assert_eq!(ms("   Compiling pkg\nerror: build failed\n"), None, "no finished build is unmeasured");
    assert_eq!(ms("   Compiling pkg\nwarning: diagnostic\n    Finished `test` profile [optimized] target(s) in 1m 23s\n    Finished `test` profile [optimized] target(s) in 12.34s\n"), Some(83_000), "skip diagnostics and stop at the first Finished line");
    assert_eq!(ms("    Finished `dev` profile [unoptimized] target(s) in 0.00s\n"), Some(0), "a measured zero is not missing");
}

#[cfg(feature = "checkout")]
#[test]
fn a_native_runs_build_is_measured_and_warms_the_next_build() {
    let base = std::env::temp_dir().join(format!("devhub-native-build-{}", std::process::id()));
    std::fs::create_dir_all(&base).unwrap();
    let start = |cmd: &str| NativeBuild::start(Src::Tree, &base, &cmd.split(' ').map(str::to_string).collect::<Vec<_>>());
    let prewarm = start("nice -n 9 cargo test --release -p a --no-run").expect("a niced cargo build is measured");
    assert!(!prewarm.warm, "a package never built here builds cold");
    std::fs::write(base.join("run.log"), "   Compiling a\n    Finished `release` profile [optimized] target(s) in 2.50s\n").unwrap();
    let b = prewarm.finished(&base.join("run.log")).expect("its build is measured");
    assert_eq!((b.ms, b.pkg.as_deref(), b.warm), (2500, Some("a"), Some(false)), "the prewarm's cold build of a");
    assert!(start("cargo test --release -p a").is_some_and(|n| n.warm), "the prewarm warms the next test's build");
    assert!(start("cargo test --release -p a -p b").is_some_and(|n| !n.warm), "a run with a package not yet built is cold");
    assert!(start("cargo check -p a").is_none(), "a check builds no code");
    assert!(start("cargo run --profile host -p makepad-run --bin makepad -- test -p a").is_none(), "a JIT run is no native build");
    let _ = std::fs::remove_dir_all(&base);
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

/// A store build's record (`cargo.pid`): its leader's pid and start, and the lease that owns it.
#[cfg(feature = "checkout")]
fn recorded_build(text: &str) -> Option<(u32, u64, String)> {
    let mut fields = text.split_whitespace();
    let (pid, start) = fields.next().and_then(|s| s.parse::<u32>().ok()).zip(fields.next().and_then(|s| s.parse::<u64>().ok()))?;
    Some((pid, start, fields.next().unwrap_or("").to_string()))
}

/// Join a store build left by an earlier hub: `lease` owns it from then on (its record says so),
/// and its end stops it ([`Exec::stop_unowned_builds`]) as it would one of its own.
#[cfg(feature = "checkout")]
fn join_recorded_build(path: &Path, lease: &str, active: &dyn Fn() -> Result<(), String>, joining: &dyn Fn(u32)) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if let Some((pid, start, owner)) = recorded_build(&text) {
        let mut joined = false;
        loop {
            active()?;
            if !os::same(pid, start) { break; }
            if !joined {
                if owner != lease { std::fs::write(path, format!("{pid} {start} {lease}\n")).map_err(|e| format!("{}: {e}", path.display()))?; }
                joining(pid);
                joined = true;
            }
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
    offline: bool,
    loader: Option<PathBuf>,
    cache: Option<std::fs::File>,
    notices: Vec<String>,
}

/// The binary a lease's JIT step built ([`Exec::jit_step`]), as its command's JIT: kept by content
/// when it was built in the lease's target (`keeps`: its tree and that target), and held until
/// the lease ends; one not kept runs where cargo built it, as the run's cargo would have run it.
#[cfg(feature = "checkout")]
fn built_loader(l: &Lease, keeps: Option<&(PathBuf, PathBuf)>, own: Option<&super::loaders::TreeBuild>, target: &Path, notices: &mut Vec<String>) -> Result<(String, PathBuf), String> {
    let built = || ("built by its lease".to_string(), super::loaders::built_binary(target));
    let Some((root, t)) = keeps else { return Ok(built()) };
    match super::loaders::publish(root, t, own) {
        Ok(Some(k)) => {
            notices.push(format!("devhub: kept the JIT binary {k}"));
            if let Some(p) = super::loaders::kept(t, &k) {
                super::loaders::hold(t, &k, &l.id).map_err(|e| format!("lease-run {}: {e}", l.id))?;
                return Ok((k, p));
            }
            // an edited tree's goes to the dev JIT store, held since its key was read
            Ok(own.and_then(|o| o.store.lookup(&k)).map_or_else(built, |p| (k, p)))
        }
        Ok(None) => Ok(built()),
        Err(e) => {
            notices.push(format!("devhub: this JIT binary is not kept: {e}"));
            Ok(built())
        }
    }
}

/// The holder's place in its worktree, where a run of `rel` runs: one the lease's tree lacks is
/// said as that (a spawn into it failed as "systemd-run: No such file or directory", 11-05).
fn run_dir(l: &Lease, rel: &Path) -> Result<PathBuf, String> {
    let dir = if l.src == Src::Overlay { l.work.clone() } else { l.work.join(rel) };
    if !dir.is_dir() {
        return Err(format!("lease-run {}: the lease's tree has no {} (the directory the run was asked from: run it from inside the lane's worktree, at a tracked path)", l.id, rel.display()));
    }
    Ok(dir)
}

/// Where a run's JIT comes from ([`Exec::run_loader`]): a work-tree lease's tree and the target
/// its cargo builds in (a run's build there is kept by content at its end), the binary it runs
/// (held until its lease ends), and the dev JIT store's key a build of its edited tree is kept by.
/// An overlay's comes with its command ([`RunCommand::loader`]).
#[derive(Default)]
struct Found {
    tree: Option<(PathBuf, PathBuf)>,
    loader: Option<(String, PathBuf)>,
    own: Option<super::loaders::TreeBuild>,
    /// what the run's log says first: why no JIT of the tree's blobs runs here
    note: Option<String>,
}

/// The run log's line for a tree whose inputs hold other bytes than their blobs.
#[cfg(feature = "checkout")]
fn converted_note(key: &super::loaders::TreeKey) -> Option<String> {
    key.converted.as_ref().map(|(n, first)| format!("devhub: {n} of this tree's JIT inputs hold other bytes than their blobs ({first}{}: a checkout converted them): only a JIT built from these bytes runs here", if *n > 1 { ", .." } else { "" }))
}

/// The largest interpreter script word, its file, and a small replacement that keeps -c mode.
/// Only the run's inner argv is inspected: never spill a metering or warm-up wrapper.
/// The box's dev JIT of `key` (a tree's inputs, [`super::loaders::tree_id`]: a commit's overlay id
/// is its tree's key), held for `lease` until it ends: its binary when the store holds it. A tree
/// lease's end releases what it holds there ([`Exec::end_with_inspect`]).
#[cfg(feature = "checkout")]
fn store_jit(work: &Path, key: &str, lease: &str) -> Result<Option<(String, PathBuf)>, String> {
    let store = super::jitstore::Store::new(work);
    store.hold(key, lease)?;
    Ok(store.lookup(key).map(|p| (key.to_string(), p)))
}

fn script_spill(argv: &[String], stem: &Path) -> Option<(usize, PathBuf, String)> {
    let (i, shell) = super::argv::script_word(argv)?;
    let path = stem.with_extension(if shell { "sh" } else { "py" });
    let p = path.to_str()?;
    let word = if shell {
        format!(". '{}'", p.replace('\'', "'\\''"))
    } else {
        let p = format!("'{}'", p.replace('\\', "\\\\").replace('\'', "\\'"));
        format!("exec(compile(open({p},'rb').read(),{p},'exec'))")
    };
    Some((i, path, word))
}

/// A run's own `makepad` in `bin` (made anew), the folder its PATH starts with: the run's JIT
/// (`jit`), else this tree's as a bare `makepad` first word runs it, cargo's build of it in
/// `work` ([`super::loaders::cargo_form`]). A job's scripts ran `makepad` as whatever their node's
/// PATH held, and one node's had none ("boot.sh: line 7: makepad: command not found", 11-07).
fn job_makepad(bin: &Path, jit: Option<&Path>, work: &Path, unattended: bool) -> Result<(), String> {
    let io = |e: std::io::Error| format!("{}: {e}", bin.display());
    let _ = std::fs::remove_dir_all(bin);
    std::fs::create_dir_all(bin).map_err(io)?;
    let manifest = work.join("Cargo.toml");
    #[cfg(unix)]
    {
        let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
        if let (Some(jit), false) = (jit, unattended) {
            return std::os::unix::fs::symlink(jit, bin.join("makepad")).map_err(io);
        }
        use std::os::unix::fs::PermissionsExt as _;
        let exec = match jit {
            Some(jit) => format!("exec {}", quote(jit)),
            None => format!("exec cargo run --profile host -q --manifest-path {} -p makepad-run --bin makepad --", quote(&manifest)),
        };
        // a lease nobody attends ([`super::nobody_attends`]): a script's `makepad run` never parks
        let unattended = if unattended { format!("if [ \"$1\" = run ]; then shift; {exec} run --unattended \"$@\"; fi\n") } else { String::new() };
        let script = bin.join("makepad");
        std::fs::write(&script, format!("#!/bin/sh\n{unattended}{exec} \"$@\"\n")).map_err(io)?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).map_err(io)
    }
    #[cfg(windows)]
    {
        // (a .cmd cannot drop its first word from %* safely: a script's run there is attended, and
        // a lease nobody attends ends it once it has gone quiet after its panic)
        let _ = unattended;
        let line = match jit {
            Some(jit) => format!("@\"{}\" %*\r\n", jit.display()),
            None => format!("@cargo run --profile host -q --manifest-path \"{}\" -p makepad-run --bin makepad -- %*\r\n", manifest.display()),
        };
        std::fs::write(bin.join("makepad.cmd"), line).map_err(io)
    }
}

/// A lease's JIT builds ([`Exec::jit_step`]) are its processes, watched and ended as its runs are,
/// numbered past every command: a run's number is its place among the lease's commands, which a
/// master that took the lease back goes by for a run no master noted ([`crate::orch`]'s
/// `Held::has_run`: a job's run 1, a shard's step k's run k + 1). A build at the command's number
/// was followed as the command: its exit the verdict, and the command unwatched.
const BUILD_RUNS: u32 = 1_000_000_000;

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
    /// the RAM the lease asked for itself (`--ram-gb`): where its run is throttled (none otherwise)
    ram_max_gb: Option<f64>,
    /// The scheduler's RAM weight before the box's budget cut, and its class's raw peak.
    weighed_gb: f64,
    peak_gb: Option<f64>,
    /// the jobs its JIT build takes ([`lease_jit_jobs`]); none: every core
    jit_jobs: Option<u32>,
    /// the threads a live session that asked its cores runs its JIT and app on ([`job_env`])
    threads: Option<u32>,
    /// the cores a live test session's long-lived pools size from ([`job_env`])
    pool_cores: Option<u32>,
    /// The node's verdict survives a restart and reaches the holder before the master's beat.
    memory_end: Option<String>,
    /// a live session (an app or a warm test process that stays up): its JIT sizes its own
    /// threads from the box, never from the lease's cores ([`job_env`])
    live: bool,
    /// interactive or background, as the master granted it: its runs' scope CPU weight
    /// ([`Lease::cpu_weight`])
    priority: super::Priority,
    /// a live session between calls, as the master last said (`lease-weight`): its runs weigh
    /// as background work then, or the idle CPU class when it rests
    idle: bool,
    /// a live test session between calls takes only CPU nothing else wants
    rest: bool,
    first_run: String,
    /// the toolchain at its start (`rustc -vV`: `<release> <commit>`); its end reads it again
    rustc: String,
    /// one of its runs changes the toolchain itself ([`super::argv::changes_toolchain`])
    toolchain_own: bool,
    shipped: bool,
    phases: super::prof::Phases,
    work: PathBuf,
    target: PathBuf,
    tmp: PathBuf,
    env: Vec<(String, String)>,
    /// Pinned commits (`<makepad>[,<name>=<sha>]...`, [`super::Pin::key`]) for the tree or store mounts.
    pin: Option<String>,
    /// Shipped paths; tree leases undo them at release, an overlay lease's folder goes with it.
    overlay: Vec<String>,
    /// Which source the last ship made ([`super::ship::source_id`]).
    source: String,
    /// What the tree lease's ships put in its tree and took out of it since its last run started:
    /// the next run starts only when they are still so ([`Lease::unshipped`]).
    fresh_put: Vec<String>,
    fresh_gone: Vec<String>,
    /// The job inputs its runs read (their hashes, [`super::ship::input_dir`]): kept while it lives.
    inputs: Vec<String>,
    runs: Vec<Arc<Run>>,
    peak_ram_bytes: Option<u64>,
    /// The CPU (s) of the store builds its overlay started, each reaped ([`Exec::overlay_build`]):
    /// no run of its, but its work, in its receipt's `cpu_seconds`
    store_cpu_s: f64,
    /// The most its live runs held at one look so far ([`held_now`]): a live session's next call
    /// weighs it (the master's `own_call_peak`).
    held_bytes: Option<u64>,
    /// What its live runs held at the newest look that could tell ([`held_now`]): what the master
    /// charges it. The most so far was charged instead: a screen session that once held 30.4 GB
    /// and since held a few was charged 30.4, and its box stood at 74.7 GB in use of 62 with
    /// 54 GB free while the queue's head waited 19 min (L1217946, 11-16). Not saved: the next
    /// look tells.
    held_now_bytes: Option<u64>,
    beat: Instant,
    /// Since when its live runs have done nothing ([`busy`]), and what they had done by then.
    /// None: no live run, or one this box cannot measure.
    quiet: Option<(Instant, os::Activity)>,
    /// The same, for running under [`SLOW_CPU_PCT`] of a core: since when they have done too
    /// little to be at work.
    slow: Option<(Instant, os::Activity)>,
    /// A live session's call under way, as its box meters it ([`CallMeter`]); none between calls.
    meter: Option<CallMeter>,
    /// The panic one of its runs' JIT processes parked on, as its feed said ([`app_panicked`]):
    /// that process and the panic's words. Its beat carries it while the process runs (`panic`).
    panicked: Option<(u32, String)>,
    /// The process groups its runs' processes moved into beside the runs' own, as its beats saw
    /// them, each with its leader's start ([`os::groups_beside`]): its end ends them too, an
    /// orphan whose parent ended among them. Saved: a hub restart keeps them.
    groups: Vec<(u32, u64)>,
}

impl Lease {
    /// The next command's run number: its place among the lease's commands.
    fn next_run(&self) -> u32 { self.runs.iter().filter(|r| r.n <= BUILD_RUNS).count() as u32 + 1 }

    /// The next JIT build's number ([`BUILD_RUNS`]).
    #[cfg(feature = "checkout")]
    fn next_build(&self) -> u32 { BUILD_RUNS + self.runs.iter().filter(|r| r.n > BUILD_RUNS).count() as u32 + 1 }

    /// Its JIT build's nice level: idle CPU only (Unix), so leases already on its box do not feel
    /// all of cargo's jobs, unless a person waits on it.
    #[cfg(feature = "checkout")]
    fn build_nice(&self) -> i32 {
        if self.live || self.priority == super::Priority::Interactive { self.kind.nice() } else { 19 }
    }

    fn tree_lane(&self) -> &str { self.warms.as_deref().unwrap_or(&self.lane) }

    /// Its runs' scope CPU weight: a live session's at interactive weight in a call, whatever it
    /// was asked as, and at background weight between calls; 0 is the idle CPU class when it rests.
    fn cpu_weight(&self) -> u32 {
        if self.live && self.idle && self.rest { return 0; }
        let priority = match (self.live, self.idle) {
            (true, false) => super::Priority::Interactive,
            (true, true) => super::Priority::Background,
            (false, _) => self.priority,
        };
        priority.cpu_weight(self.cost.cores)
    }

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
            // Keep each run's build attached to it, even when two run intervals touch.
            phases.0.push(super::prof::Span { phase: "run".into(), blocker: String::new(), boxes: Vec::new(), on: Vec::new(), start: r.started_ms, end: if end > 0 { end.min(now) } else if ending { now } else { 0 }, build: if end > 0 { r.build().cloned() } else { None } });
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
        self.env.iter().find(|(k, _)| crate::run_names::is(k, "EDITS")).map(|(_, v)| PathBuf::from(v))
    }

    fn ledger(&self) -> String {
        let runs: Vec<String> = self
            .runs
            .iter()
            .map(|r| format!("{{\"request_id\":{},\"n\":{},\"pid\":{},\"start\":{},\"started_ms\":{},\"spawned_ms\":{},\"detachable\":{},\"unit\":{},\"log\":{},\"native_build\":{}}}", json_str(&r.request), r.n, r.pid, r.start, r.started_ms, r.spawned_ms, r.detachable, r.unit.as_deref().map_or("null".into(), json_str), json_str(&r.log.to_string_lossy()), r.native.as_ref().map_or("null".into(), NativeBuild::json)))
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
        format!("{{{resolution}\"id\":{},\"lane\":{}{warms},\"kind\":\"{}\",\"src\":\"{}\",\"loader\":{},\"cost\":{{\"cores\":{},\"ram_gb\":{},\"ram_max_gb\":{},\"weighed_gb\":{},\"peak_gb\":{},\"jit_jobs\":{},\"threads\":{},\"pool_cores\":{}}},\"memory_end\":{},\"live\":{},\"priority\":\"{}\",\"idle\":{},\"rest\":{},\"peak_ram_bytes\":{},\"store_cpu_s\":{},\"held_bytes\":{},\"first_run\":{},\"rustc\":{},\"toolchain_own\":{},\"shipped\":{},\"phases\":{},\"pin\":{},\"overlay\":[{}],\"source\":\"{source}\",\"fresh_put\":[{}],\"fresh_gone\":[{}],\"inputs\":[{}],\"groups\":[{}],\"runs\":[{}],{}", json_str(&self.id), json_str(&self.lane), self.kind.as_str(), self.src.as_str(), self.loader.as_deref().map_or("null".into(), json_str), self.cost.cores, self.cost.ram_gb, self.ram_max_gb.map_or("null".into(), |g| g.to_string()), self.weighed_gb, self.peak_gb.map_or("null".into(), |g| g.to_string()), self.jit_jobs.map_or("null".into(), |j| j.to_string()), self.threads.map_or("null".into(), |j| j.to_string()), self.pool_cores.map_or("null".into(), |n| n.to_string()), self.memory_end.as_deref().map_or("null".into(), json_str), self.live, self.priority.as_str(), self.idle, self.rest, self.peak_ram_bytes.map_or("null".into(), |n| n.to_string()), self.store_cpu_s, self.held_bytes.map_or("null".into(), |n| n.to_string()), json_str(&self.first_run), json_str(&self.rustc), self.toolchain_own, self.shipped, self.phases.json(), self.pin.as_deref().map_or("null".into(), json_str), self.overlay.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), self.fresh_put.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), self.fresh_gone.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), self.inputs.iter().map(|h| json_str(h)).collect::<Vec<_>>().join(","), self.groups.iter().map(|(g, s)| format!("[{g},{s}]")).collect::<Vec<_>>().join(","), runs.join(","), &a[1..])
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
                .filter_map(|r| Some(Arc::new(Run { request: s(r, "request_id").unwrap_or_default(), started_ms: n(r, "started_ms"), spawned_ms: n(r, "spawned_ms").max(n(r, "started_ms")), ended_ms: AtomicU64::new(0), n: n(r, "n") as u32, pid: n(r, "pid") as u32, start: n(r, "start"), unit: s(r, "unit"), log: s(r, "log")?.into(), exit: Mutex::new(None), ours: false, detachable: r.get("detachable").and_then(|v| v.as_bool()).unwrap_or(false), hold: os::Hold::none(), exit_file: PathBuf::new(), cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: r.get("native_build").and_then(NativeBuild::parse), build: OnceLock::new() })))
                .collect()
        });
        let kind = Kind::parse(&s(&v, "kind")?)?;
        let cost = lease_cost(&v, kind);
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
            kind,
            src: s(&v, "src").as_deref().and_then(Src::parse).unwrap_or_default(),
            loader: s(&v, "loader"),
            cost,
            ram_max_gb: lease_ram_max(&v),
            weighed_gb: lease_ram_weight(&v).unwrap_or(cost.ram_gb),
            peak_gb: lease_ram_peak(&v),
            jit_jobs: lease_jit_jobs(&v),
            threads: lease_threads(&v),
            pool_cores: lease_pool_cores(&v),
            memory_end: s(&v, "memory_end"),
            live: live_of(&v),
            priority: priority_of(&v),
            idle: v.get("idle").and_then(|x| x.as_bool()).unwrap_or(false),
            rest: v.get("rest").and_then(|x| x.as_bool()).unwrap_or(false),
            first_run: s(&v, "first_run").unwrap_or_default(),
            rustc: s(&v, "rustc").unwrap_or_default(),
            toolchain_own: v.get("toolchain_own").and_then(|x| x.as_bool()).unwrap_or(false),
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
            inputs: paths("inputs"),
            runs,
            peak_ram_bytes: v.get("peak_ram_bytes").and_then(|v| v.as_u64()),
            store_cpu_s: v.get("store_cpu_s").and_then(super::num).unwrap_or(0.0),
            held_bytes: v.get("held_bytes").and_then(|v| v.as_u64()),
            held_now_bytes: None,
            beat: Instant::now(),
            quiet: None,
            slow: None,
            meter: None,
            panicked: None,
            groups: v.get("groups").and_then(|x| x.as_arr()).map_or_else(Vec::new, |a| a.iter().filter_map(|g| {
                let g = g.as_arr()?;
                Some((super::num(g.first()?)? as u32, super::num(g.get(1)?)? as u64))
            }).collect()),
        })
    }
}

const START_KEYS: &[&str] = &[
    "id", "lane", "warms", "kind", "src", "rev", "cpu_target", "env", "needs",
    "cost", "live", "loader", "priority", "gpu", "unquiet",
];
const START_COST_KEYS: &[&str] = &["cores", "ram_gb", "ram_max_gb", "weighed_gb", "peak_gb", "jit_jobs", "threads", "pool_cores"];

fn unknown_key<'a>(v: &'a makepad_strict_json::Value, keys: &[&str]) -> Option<&'a str> {
    let makepad_strict_json::Value::Obj(fields) = v else { return None };
    fields.iter().find(|(k, _)| !keys.contains(&k.as_str())).map(|(k, _)| k.as_str())
}

/// A live session's lease (the master's `live`).
/// A lease's own app-test artifacts dir (`MAKEPAD_TEST_ARTIFACTS`) in its target: two jobs of
/// one lane on one box share the target, never this.
fn test_artifacts(target: &Path, id: &str) -> PathBuf {
    target.join("makepad_test").join(id)
}

fn live_of(v: &makepad_strict_json::Value) -> bool {
    v.get("live").and_then(|x| x.as_bool()).unwrap_or(false)
}

fn priority_of(v: &makepad_strict_json::Value) -> super::Priority {
    v.get("priority").and_then(|x| x.as_str()).and_then(super::Priority::parse).unwrap_or_default()
}

/// The lease's own RAM ask (`cost.ram_max_gb`): its hard cap; a measured one is no cap.
fn lease_ram_max(v: &makepad_strict_json::Value) -> Option<f64> {
    v.get("cost").and_then(|c| c.get("ram_max_gb")).and_then(super::num).filter(|n| n.is_finite() && *n > 0.0)
}

/// An older master sends only its allocation; zero is a valid weight for an uncharged lease.
fn lease_ram_weight(v: &makepad_strict_json::Value) -> Option<f64> {
    v.get("cost").and_then(|c| c.get("weighed_gb")).and_then(super::num).filter(|n| n.is_finite() && *n >= 0.0)
}

/// The raw class peak, if the master had one when it weighed this lease.
fn lease_ram_peak(v: &makepad_strict_json::Value) -> Option<f64> {
    v.get("cost").and_then(|c| c.get("peak_gb")).and_then(super::num).filter(|n| n.is_finite() && *n >= 0.0)
}

/// The jobs the lease's JIT build takes (`cost.jit_jobs`): admitted on fewer cores than such a
/// build measures, it keeps to those. None: cargo's own, every core.
fn lease_jit_jobs(v: &makepad_strict_json::Value) -> Option<u32> {
    v.get("cost").and_then(|c| c.get("jit_jobs")).and_then(super::num).filter(|n| n.is_finite() && *n >= 1.0).map(|n| n as u32)
}

/// The threads a live session that asked its cores runs on (`cost.threads`, the master's
/// `live_threads`). None: one that asked none, whose JIT sizes its own from the box.
fn lease_threads(v: &makepad_strict_json::Value) -> Option<u32> {
    v.get("cost").and_then(|c| c.get("threads")).and_then(super::num).filter(|n| n.is_finite() && *n >= 1.0).map(|n| n as u32)
}

/// The cores a live test session's long-lived pools size from (`cost.pool_cores`, the master's
/// `pool_cores`). None: any other lease.
fn lease_pool_cores(v: &makepad_strict_json::Value) -> Option<u32> {
    v.get("cost").and_then(|c| c.get("pool_cores")).and_then(super::num).filter(|n| n.is_finite() && *n >= 1.0).map(|n| n as u32)
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

/// The thread counts a run gets from its lease's cores, and the RAM it got (`DEVHUB_RAM_MB`: a
/// tests shard starts its units within it, land_gate.sh testrun). Its CPU budget is those cores
/// too (`MAKEPAD_CPU_CORES`, which its pools and JIT workers size from), as each of a gate's units
/// gets its share: a lane's run with `--cores 4` saw the box's 16 and was no repro of a unit of 4
/// (11-09). A live session's JIT gets no jobs: the lease's cores are what the scheduler counts, its
/// cgroup weight is its share under contention, and the app's own CPU budget sizes its threads
/// from the box (a live Fab on one JIT worker waited up to 42 s). One that asked its cores
/// (`threads`) runs all of them on its ask: a live call that asked 4 ran its JIT on 10-16 of its
/// box's cores, and its box stood past them while a small waited there (11-16).
///
/// A live test session that asked none takes back part of that (11-18): its long-lived pools
/// (`pool_cores`: MAKEPAD_POOL_CORES, libs/cpu_budget `pool_cores`) size from its call's charge,
/// while its budget, and so its JIT, keeps the box (no RELAX_JOBS or MAKEPAD_CPU_CORES). The pools
/// stay the whole session and each took its box's 16 cores beside the JIT's workers while the
/// scheduler charged it a small ask: three live Fab tests stood a box at load 57. A launch's
/// compile is a short burst, and what its interactive launch waits on. A live app launch is as
/// before: the master sends it none.
fn job_env(kind: Kind, cost: super::Cost, live: bool, threads: Option<u32>, pool_cores: Option<u32>) -> Vec<(&'static str, u32)> {
    let jobs = threads.filter(|_| live).unwrap_or_else(|| jobs(kind, cost));
    let ram_mb = (cost.ram_gb * 1024.0).round() as u32;
    let sized = !live || threads.is_some();
    let pools = pool_cores.filter(|_| live && !sized).map(|n| ("MAKEPAD_POOL_CORES", n));
    [("RELAX_JOBS", jobs), ("MAKEPAD_CPU_CORES", jobs)].into_iter().filter(|_| sized).chain([("CARGO_BUILD_JOBS", jobs), ("RUST_TEST_THREADS", jobs), ("DEVHUB_RAM_MB", ram_mb)]).chain(pools).collect()
}

/// `lease-conf set`'s `key=value` pairs, and who asked (`by=<agent>`, no lease.conf key).
fn conf_set_args<'a>(words: impl Iterator<Item = &'a str>) -> (Vec<(String, String)>, Option<String>) {
    let mut by = None;
    let mut pairs = Vec::new();
    for (k, v) in words.filter_map(|kv| kv.split_once('=')) {
        if k == "by" { by = Some(v.to_string()); } else { pairs.push((k.to_string(), v.to_string())); }
    }
    (pairs, by)
}

/// How long a timed series' builds stay held after the last timed lease here that used them
/// ([`Exec::hold_series`]): a series' runs come minutes to an hour or two apart, its queue waits
/// included. Past it the builds are relax's to prune as any other's.
const SERIES_KEEP_MS: u64 = 3 * 3600 * 1000;
/// How often a running timed lease's builds are looked for: the lock files' stamps say what was
/// taken between two looks, so a look misses nothing, it only comes later.
const SERIES_LOOK: Duration = Duration::from_secs(15);

/// The timed series' JIT builds this hub holds ([`Exec::hold_series`]), kept in a file so that a
/// restarted hub takes them again.
#[derive(Default)]
struct SeriesHolds {
    loaded: bool,
    looked: Option<Instant>,
    /// each running timed lease's first look: a lock stamped since is its build's
    since: BTreeMap<String, std::time::SystemTime>,
    /// a build's lock file -> the series' lane, held until (wall ms), and the held file
    held: BTreeMap<PathBuf, (String, u64, std::fs::File)>,
}

impl SeriesHolds {
    /// One look over run dirs `dirs` for the timed leases `timed` (none: only the holds past their
    /// time go), `ended` the one that ended; the holds are saved in `file`. What changed, to say.
    fn update(&mut self, file: &Path, dirs: &[PathBuf], timed: &[(String, String)], ended: Option<&str>, now_ms: u64) -> Option<String> {
        let mut took = Vec::new();
        if !self.loaded {
            self.loaded = true;
            for line in std::fs::read_to_string(file).unwrap_or_default().lines() {
                let mut w = line.splitn(3, '\t');
                let (Some(until), Some(lane), Some(p)) = (w.next().and_then(|u| u.parse::<u64>().ok()), w.next(), w.next()) else { continue };
                if until > now_ms {
                    if let Some(f) = lock_shared(Path::new(p)) { self.held.insert(PathBuf::from(p), (lane.to_string(), until, f)); }
                }
            }
        }
        if !dirs.is_empty() {
            self.looked = Some(Instant::now());
        }
        let until = now_ms + SERIES_KEEP_MS;
        for (id, lane) in timed {
            let since = *self.since.entry(id.clone()).or_insert_with(std::time::SystemTime::now);
            for e in dirs.iter().flat_map(|d| std::fs::read_dir(d.join("cache").join("live")).into_iter().flatten().flatten()) {
                let p = e.path();
                let stamped = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t >= since);
                if let Some(h) = self.held.get_mut(&p) {
                    // used again in this series' run: held longer
                    if stamped && h.0 == *lane { h.1 = h.1.max(until); }
                    continue;
                }
                if !stamped && !lock_taken(&p) { continue; }
                if let Some(f) = lock_shared(&p) {
                    took.push(p.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned()));
                    self.held.insert(p, (lane.clone(), until, f));
                }
            }
        }
        if let Some(id) = ended { self.since.remove(id); }
        let before = self.held.len();
        self.held.retain(|p, h| h.1 > now_ms && p.exists());
        let let_go = before - self.held.len();
        if took.is_empty() && let_go == 0 { return None; }
        let text: String = self.held.iter().map(|(p, (lane, until, _))| format!("{until}\t{lane}\t{}\n", p.display())).collect();
        let tmp = file.with_extension("new");
        let _ = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, file));
        let lanes: BTreeSet<&str> = timed.iter().map(|(_, l)| l.as_str()).collect();
        Some(match (took.is_empty(), let_go) {
            (false, 0) => format!("holds the JIT build(s) {} of timed series {} for {} h past its last use: the JIT keeps their stores", took.join(", "), lanes.into_iter().collect::<Vec<_>>().join(", "), SERIES_KEEP_MS / 3_600_000),
            (true, n) => format!("lets go of {n} timed series JIT build(s) past their time"),
            (false, n) => format!("holds the JIT build(s) {} of timed series {}, and lets go of {n} past their time", took.join(", "), lanes.into_iter().collect::<Vec<_>>().join(", ")),
        })
    }
}

/// A shared lock on `p`, as a running JIT process holds its build's (relax store.rs hold_build).
fn lock_shared(p: &Path) -> Option<std::fs::File> {
    let f = std::fs::OpenOptions::new().read(true).open(p).ok()?;
    f.lock_shared().ok()?;
    Some(f)
}

/// Whether a process holds lock file `p` now.
fn lock_taken(p: &Path) -> bool {
    std::fs::OpenOptions::new().read(true).open(p).is_ok_and(|f| f.try_lock().is_err())
}

/// Whether `lane` is a live test session's own lease lane, `live-w<n>` (orch/live.rs): one lease
/// for the session's life, so its dirs here end with it ([`Exec::session_ended`]).
fn session_lane(lane: &str) -> bool {
    lane.strip_prefix("live-").and_then(|r| r.strip_prefix(['w', 'W'])).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The overlay lane of a lease on this box: its lane's, or for CI one per pin, so leases at
/// different pins run side by side.
fn overlay_lane(kind: Kind, lane: &str, pin_key: &str) -> String {
    if kind == Kind::Ci {
        // the whole pin: two pairs at one makepad commit and different commercial ones differ
        if let Some(pin) = super::Pin::parse(pin_key) { return format!("{lane}-{}", makepad_network::digest::sha256_hash(pin.key().as_bytes())[..6].iter().map(|b| format!("{b:02x}")).collect::<String>()); }
    }
    lane.to_string()
}

/// The node's lease executor.
pub struct Exec {
    this: Weak<Exec>,
    pub machine: String,
    publish: Publish,
    conf: std::sync::RwLock<LeaseConf>,
    /// the ledger folder (`~/.makepad/devhub/leases`)
    state: PathBuf,
    opts: Opts,
    scopes: bool,
    /// its systemd units' tag ([`unit_tag`])
    units: String,
    dev: AtomicBool,
    /// a flip asked for on this node itself, until the master's policy push answers it (0 none, 1 on, 2 off)
    dev_req: std::sync::atomic::AtomicU8,
    leases: crate::held::Held<BTreeMap<String, Lease>>,
    starting: crate::held::Held<()>,
    /// The lanes `disk clean` takes apart now: a lease of one waits for it, and only that lease
    /// (its size and its delete under [`Self::starting`] held up every start and the hub's restart).
    cleaning: (Mutex<BTreeSet<String>>, std::sync::Condvar),
    /// The JIT builds of timed series held here ([`Exec::hold_series`]).
    series: Mutex<SeriesHolds>,
    /// Duplicate end requests observe the in-flight or durable final receipt.
    endings: Mutex<BTreeMap<String, String>>,
    /// orphans of ended leases already sent TERM ([`Exec::end_orphans`])
    orphans: Mutex<Vec<u32>>,
    /// scoped runs' CPU at the last caps poll, including their exited children (11-14)
    #[cfg(target_os = "linux")]
    scope_cpu: Mutex<BTreeMap<(String, u32), (Instant, u64)>>,
    pub guard: Guard,
    /// lease id -> what its directories hold: measured off the beat
    disk_use: Arc<Mutex<std::collections::HashMap<String, DiskUse>>>,
    /// timed lease id -> what shared its box ([`Noise`], [`timed_box_loop`]; Windows runs have no
    /// process groups to tell a run's own processes by)
    #[cfg(unix)]
    noise: Mutex<std::collections::HashMap<String, Noise>>,
    /// the box at the timed leases' last look ([`box_look`]): what the next look counts from
    #[cfg(unix)]
    span: Mutex<Option<crate::top::Ticks>>,
    /// The AI hub process a timed run's quiet was asked of ([`Exec::hold_quiet`]): one that
    /// came back since (the WM started it again) holds none, and is asked again.
    quiet_hub: Mutex<Option<u32>>,
    /// What each refused timed start was said to wait on ([`Exec::timed_waits`]): said again
    /// only when that changes.
    timed_said: Mutex<BTreeMap<String, String>>,
    /// the lease that took the disk, and why ([`lease_over_disk`]): its master ends it
    over_disk: Mutex<Option<(String, String)>>,
    /// The box's store builds running now ([`Self::overlay_build`]), no run of a lease's: each with
    /// the lease whose overlay started it, which its CPU is ([`Self::lease_groups`])
    #[cfg(feature = "checkout")]
    store_builds: Mutex<Vec<(String, Arc<Run>)>>,
    /// Unexplained pressure is reported once until memory recovers above the floor.
    #[cfg(target_os = "linux")]
    memory_unexplained: AtomicBool,
    /// who watches the node's screen, as its WM or a Mac's HID system saw them ([`super::user`])
    user: Mutex<super::user::UserTrack>,
    /// an app of ours a person uses outside every lease ([`super::hand`])
    hand: Mutex<super::hand::HandTrack>,
    /// `hub-restart` was answered: this hub exits in a moment and starts no run before it does
    restarting: AtomicBool,
    /// The new hub's build before `hub-restart` exits for it ([`Exec::hub_built`]).
    #[cfg(feature = "checkout")]
    hub_build: Arc<Mutex<HubBuild>>,
    /// the last [`Exec::resident_gb`] measured
    last_resident_gb: Mutex<f64>,
    /// What this hub has said of its WM having no size ([`DarkSaid`]).
    dark_said: Mutex<DarkSaid>,
    /// Thermal hysteresis and the last temperature log, shared by caps polls.
    thermal: Mutex<(caps::Thermal, Option<Instant>)>,
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

/// A screen or gui lease's start refused for want of a live WM display: the master places it
/// again, and the box takes no such lease until its next caps show one.
pub const NO_LIVE_WM: &str = "no live WM display";

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
    let display = caps::display().ok_or(format!("gui=1: {NO_LIVE_WM}"))?;
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
/// whose output ends without libtest's summary; a run stopped on purpose on its own (a launch's
/// stop) ended as asked. (its line, whether it is more than a failure, the log's last lines)
/// A test run's log says its verdict: libtest's `test result:`, or a gate script's NODEGATE-OK or
/// NODEGATE-FAIL. A tests shard whose units failed ran to its end, and read as "ended without its
/// summary: exit 1", which kept its run out of its class's measures (11-07).
fn said_its_summary(kind: Kind, tail: &str) -> bool {
    kind != Kind::Test || tail.lines().any(|l| l.starts_with("test result:") || l.starts_with("NODEGATE-OK") || l.starts_with("NODEGATE-FAIL"))
}

fn abnormal_end(id: &str, kind: Kind, r: &Run, exit: i32) -> Option<(String, bool, String)> {
    if r.stopped.load(Ordering::Acquire) { return None; }
    let tail = log_tail(&r.log, TAIL_LINES);
    let summary = said_its_summary(kind, &tail);
    if exit == 0 || exit == -2 || (exit > 0 && exit <= 128 && summary) { return None; }
    let (line, serious) = ended_how(id, r.n, r.unit.as_deref(), exit, !r.ours, kernel_log(r.started_ms, crate::proto::wall_ms()).as_deref());
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

/// The `HUNG: ` line of a run that stopped a unit as hung (land_gate.sh testrun): the run's time
/// and peaks are the hang's, no measure of its class ([`super::master`] skips it): a unit stopped
/// after 78 min made its tests shard's wall the hang's (11-07).
fn hung_line(log: &str) -> Option<&str> {
    log.lines().find(|l| l.starts_with("HUNG: "))
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
pub(crate) fn log_tail(log: &Path, n: usize) -> String {
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
    let o = std::process::Command::new(os::tool("journalctl"))
        .args(["-k", "-q", "--no-pager", "-o", "cat", &format!("--since=@{}", from_ms / 1000), &format!("--until=@{}", to_ms / 1000 + 2), "-g", "oom-kill:|Killed process|memory: usage|NVRM: Xid|fallen off the bus|GPU reset"])
        .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    // journalctl -g exits 1 when nothing matched
    (o.status.success() || o.stdout.is_empty()).then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

#[cfg(not(target_os = "linux"))]
fn kernel_log(_from_ms: u64, _to_ms: u64) -> Option<String> { None }

/// The abnormal end's line: the exit, the OOM kill of the run's scope (`unit`) and a GPU fault
/// the kernel logged in its time; and whether it is more than a plain failure. `taken_back`: the
/// run is one an earlier hub started (an unknown exit is then one its wrapper never wrote).
fn ended_how(id: &str, n: u32, unit: Option<&str>, exit: i32, taken_back: bool, kernel: Option<&str>) -> (String, bool) {
    const SIGNALS: [&str; 16] = ["", "SIGHUP", "SIGINT", "SIGQUIT", "SIGILL", "SIGTRAP", "SIGABRT", "SIGBUS", "SIGFPE", "SIGKILL", "SIGUSR1", "SIGSEGV", "SIGUSR2", "SIGPIPE", "SIGALRM", "SIGTERM"];
    let mut serious = exit > 128 || exit < 0;
    let mut out = format!("{id} run {n} ended without its summary: {}", match exit {
        e if e > 128 => format!("killed by signal {} ({})", e - 128, SIGNALS.get((e - 128) as usize).copied().filter(|s| !s.is_empty()).unwrap_or("?")),
        e if e < 0 && taken_back => "its exit is unknown (its hub restarted while it ran, and its wrapper wrote no exit: it was killed too)".to_string(),
        e if e < 0 => "its exit is unknown (its hub could not reap it)".to_string(),
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

/// `<tmp>/launch/<jit file>/+launch/<name>`, a hard link of `jit`, for a JIT run of the program
/// `name` to start from (start_run): one per JIT file and program for the lease, which each run
/// of it takes while it is still that file. On macOS a new link of a file and an unlink of one
/// each make syspolicyd scan the file again at its next exec (about 0.3 s for a 76 MB loader, and
/// a process killed in that wait now and then; an M-series Mac, 11-14): a link made and removed
/// for each run paid that at every start. A JIT rebuilt at its path is another file, with a
/// folder of its own. The folder's name `+launch` tells the runner that its process is a start,
/// not a relaunch of a run (relax-compiler selfexe.rs relaunched). None where it cannot be made
/// (another volume, Windows, whose processes an exec does not name, a name that is no file name):
/// the run starts the JIT itself.
fn launch_link(tmp: &Path, jit: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.starts_with('.') || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let file = std::fs::metadata(jit).ok()?;
        let is_jit = |link: &Path| std::fs::metadata(link).is_ok_and(|m| (m.dev(), m.ino()) == (file.dev(), file.ino()));
        let at = tmp.join("launch").join(format!("{:x}-{:x}", file.dev(), file.ino())).join("+launch");
        let link = at.join(name);
        if is_jit(&link) {
            return Some(link);
        }
        std::fs::create_dir_all(&at).ok()?;
        match std::fs::hard_link(jit, &link) {
            Ok(()) => Some(link),
            // (a run of the lease that started beside this one made it)
            Err(_) => is_jit(&link).then_some(link),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (tmp, jit);
        None
    }
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
    /// The pid of what its wrapper started, once the wrapper wrote it.
    fn app_pid(&self) -> Option<u32> {
        read_whole(&self.exit_file.with_extension("usage.pid")).and_then(|s| s.trim().parse().ok())
    }

    fn work_pid(&self) -> u32 {
        self.app_pid().filter(|pid| os::in_group(*pid, self.pid, &self.hold)).unwrap_or(self.pid)
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

    fn build(&self) -> Option<&super::prof::Build> {
        self.build.get_or_init(|| self.native.as_ref().and_then(|n| n.finished(&self.log))).as_ref()
    }

    fn finished(&self, code: i32) {
        self.build();
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

/// How often a timed lease's box is looked at ([`Noise`], [`Exec::keep_quiet`]).
const BOX_EVERY: Duration = Duration::from_secs(10);

/// How long after its deployment's checkout moved a timed start waits before it looks at the
/// box again ([`Exec::timed_waits`]): an app that runs makepad from the checkout begins
/// rebuilding it within a second or two of the move (timed runs granted at the move's second
/// ran beside its rustc at 808% and 576% of a core, 11-10).
const MOVE_SETTLE: Duration = Duration::from_secs(10);

/// What shared a timed lease's box, a look every [`BOX_EVERY`]: the load, and the CPU (% of one
/// core) outside the lease's process groups between looks, exact ([`crate::top::outside_between`]):
/// each program's with what the children it waited for took, this hub's, and on Linux what no
/// process showed (interrupts among it). Its report carries it (`box`), so a noisy run is seen: a
/// Mac's Spotlight indexed what builds wrote at 180% of a core under a timed lease (11-03). Half
/// a second's sample of the processes at each look missed what came and went between them: a GPU
/// box's floor of 35% of a core read 10%, its AI hub's `nvidia-smi` runs (13%) unseen (11-22).
/// Each look weighs by its span; a span begun before the lease's start is partly another's and
/// counts not (`from`).
#[cfg(unix)]
#[derive(Default)]
struct Noise {
    samples: u32,
    /// the counted looks' spans, s
    secs: f64,
    load1_max: f64,
    /// what ran outside it (% of a core) times each span, and its most at a look
    outside_sum: f64,
    outside_max: f64,
    /// by program: (its CPU, the part its waited-for children took) times each span
    by_name: BTreeMap<String, (f64, f64)>,
    /// this hub's own CPU times each span
    hub_sum: f64,
    /// Linux: (what no process showed, interrupts' part) times each span
    unseen_sum: Option<(f64, f64)>,
    /// from when its looks count: its start
    from: Option<Instant>,
    /// a GPU timing's (`gpu` in its start): what it is judged by is the GPU used outside it
    /// ([`super::master::unquiet`])
    gpu: bool,
    /// the GPU used outside it, % of the GPU, at each look a Mac told it ([`crate::top::gpu_outside`])
    gpu_samples: u32,
    gpu_sum: f64,
    gpu_max: f64,
    gpu_by_name: BTreeMap<String, f64>,
}

#[cfg(unix)]
impl Noise {
    fn add(&mut self, o: &crate::top::Outside) {
        let total: f64 = o.by.iter().map(|b| b.1).sum();
        self.samples += 1;
        self.secs += o.secs;
        self.load1_max = self.load1_max.max(o.load1);
        self.outside_sum += total * o.secs;
        self.outside_max = self.outside_max.max(total);
        for (name, cpu, kids) in &o.by {
            let e = self.by_name.entry(name.clone()).or_default();
            e.0 += cpu * o.secs;
            e.1 += kids * o.secs;
        }
        self.hub_sum += o.hub * o.secs;
        if let Some((rest, irq)) = o.unseen {
            let e = self.unseen_sum.get_or_insert((0.0, 0.0));
            e.0 += rest * o.secs;
            e.1 += irq * o.secs;
        }
    }

    fn add_gpu(&mut self, total: f64, outside: &[(String, u32, f64)]) {
        self.gpu_samples += 1;
        self.gpu_sum += total;
        self.gpu_max = self.gpu_max.max(total);
        for (name, _, pct) in outside {
            *self.gpu_by_name.entry(name.clone()).or_default() += pct;
        }
    }

    /// `{"samples":n,"secs":s,"load1_max":x,"outside_cpu_pct":mean,"outside_cpu_pct_max":max,
    /// "busiest":[[name,mean,children's mean],..],"hub_cpu_pct":mean}`: the five busiest programs
    /// outside it by their mean over its span; Linux's adds what no process showed
    /// (`unseen_cpu_pct`, `irq_cpu_pct`); a GPU timing's adds `"gpu":true`, and a Mac's the GPU
    /// outside it the same way (`outside_gpu_pct`, `outside_gpu_pct_max`, `gpu_busiest`).
    fn json(&self) -> String {
        let s = self.secs.max(0.001);
        let mut busiest: Vec<(&String, f64, f64)> = self.by_name.iter().map(|(k, (cpu, kids))| (k, cpu / s, kids / s)).collect();
        busiest.sort_by(|a, b| b.1.total_cmp(&a.1));
        let busiest = busiest.iter().take(5).map(|(k, cpu, kids)| format!("[{},{cpu:.0},{kids:.0}]", json_str(k))).collect::<Vec<_>>().join(",");
        let unseen = self.unseen_sum.map_or(String::new(), |(rest, irq)| format!(",\"unseen_cpu_pct\":{:.1},\"irq_cpu_pct\":{:.1}", rest / s, irq / s));
        let gpu = if self.gpu_samples == 0 { String::new() } else {
            let g = self.gpu_samples as f64;
            let mut by: Vec<(&String, f64)> = self.gpu_by_name.iter().map(|(k, v)| (k, v / g)).collect();
            by.sort_by(|a, b| b.1.total_cmp(&a.1));
            let by = by.iter().take(5).map(|(k, v)| format!("[{},{v:.0}]", json_str(k))).collect::<Vec<_>>().join(",");
            format!(",\"outside_gpu_pct\":{:.1},\"outside_gpu_pct_max\":{:.1},\"gpu_busiest\":[{by}]", self.gpu_sum / g, self.gpu_max)
        };
        format!(
            "{{\"samples\":{},\"secs\":{:.0},\"load1_max\":{:.1},\"outside_cpu_pct\":{:.0},\"outside_cpu_pct_max\":{:.0},\"busiest\":[{busiest}],\"hub_cpu_pct\":{:.1}{unseen}{}{gpu}}}",
            self.samples, self.secs, self.load1_max, self.outside_sum / s, self.outside_max, self.hub_sum / s, if self.gpu { ",\"gpu\":true" } else { "" }
        )
    }
}

/// Looks at the box while a timed lease runs: its AI hub still holds quiet
/// ([`Exec::keep_quiet`]), and what shares it ([`Noise`], where a run's process groups tell).
fn timed_box_loop(w: Weak<Exec>) {
    loop {
        std::thread::sleep(BOX_EVERY);
        let Some(x) = w.upgrade() else { return };
        x.keep_quiet();
        #[cfg(unix)]
        box_look(&x);
    }
}

/// One look at the box for each timed lease's [`Noise`]: what ran outside since the last look
/// (the first look of a timed lease alone is its start's, [`Exec::first_span`]); the processes of
/// its runs' groups are its own work. With no timed lease the span ends, so the next one's
/// first span begins at its start.
#[cfg(unix)]
fn box_look(x: &Exec) {
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
    if timed.is_empty() {
        *x.span.lock().unwrap() = None;
        return;
    }
    let whose = |pgid: u32| groups.get(&pgid).cloned();
    let look = x.opts.box_look.clone().unwrap_or_else(BoxLook::this_box);
    let (now, me) = (look.ticks)();
    let mut span = x.span.lock().unwrap();
    let (Some(then), Some(now)) = (span.replace(now), span.as_ref()) else { return };
    let (outside, rows) = crate::top::outside_between(&then, now, me, &whose);
    drop(span);
    let gpu = crate::top::gpu_outside(&rows, &whose);
    let mut noise = x.noise.lock().unwrap();
    for id in &timed {
        let n = noise.entry(id.clone()).or_default();
        if n.from.is_some_and(|from| then.at < from) {
            continue;
        }
        n.add(&outside);
        if let Some((total, by)) = &gpu { n.add_gpu(*total, by); }
    }
}

/// What a lease's directories hold, MB ([`Exec::disk_of`]).
#[derive(Clone, Copy)]
struct DiskUse {
    at: Instant,
    /// all of them, for the panel (u64::MAX: the first walk is under way), and at the first walk
    mb: u64,
    first_mb: Option<u64>,
    /// walks done (a growth needs two)
    walks: u32,
    /// the ones that are its own ([`Lease::dirs`]), now and at the first walk
    own_mb: u64,
    first_own_mb: Option<u64>,
}

/// How often a live call's meter looks at its session's CPU ([`call_loop`]).
pub(super) const CALL_WINDOW: Duration = Duration::from_secs(1);

/// A live session's call as its box meters it, from when the master said it started (its
/// launch: from its run's spawn) to when it said it ended ([`Exec::weigh`]): its runs' CPU, by
/// [`CALL_WINDOW`]. What a call uses is no lease's mean: the mean over a session's life is mostly
/// its idle time (1 to 2 cores), and a call compiles on many. Its end is a sample of its class's
/// call charge (`devhub stats calls`).
#[derive(Clone, Copy, Debug)]
struct CallMeter {
    launch: bool,
    since: Instant,
    /// its last look, and the CPU its runs used since its start
    last: (Instant, os::Activity),
    used_us: u64,
    /// the most cores a window took
    peak: Option<f64>,
}

impl CallMeter {
    fn new(launch: bool, now: Instant, at: os::Activity) -> Self {
        CallMeter { launch, since: now, last: (now, at), used_us: 0, peak: None }
    }

    /// A look at its runs' CPU. A window shorter than half of [`CALL_WINDOW`] (a call's last)
    /// counts in its use only: on `/proc`'s 10 ms ticks a short window's rate is mostly noise.
    /// A run that ended took its CPU out of the sum: the window starts again from there.
    fn look(&mut self, now: Instant, at: os::Activity) {
        let (then, was) = self.last;
        self.last = (now, at);
        let Some(used) = at.cpu_us.checked_sub(was.cpu_us) else { return };
        self.used_us += used;
        let secs = now.saturating_duration_since(then).as_secs_f64();
        if secs >= CALL_WINDOW.as_secs_f64() / 2.0 {
            let cores = used as f64 / 1e6 / secs;
            self.peak = Some(self.peak.map_or(cores, |p| p.max(cores)));
        }
    }

    /// The call's sample at its end, its last look `at` (None: its runs could not be read then):
    /// `{"launch","ms","peak","mean"}`, cores. A call shorter than a window peaked at its mean,
    /// and no call peaked below it: its short windows count in its use, not in its peak, and one
    /// busy short window (its start, its end) read a peak of 0.44 under a mean of 0.59 on a loaded
    /// box (11-06).
    fn end(mut self, now: Instant, at: Option<os::Activity>) -> String {
        if let Some(at) = at { self.look(now, at); }
        let secs = now.saturating_duration_since(self.since).as_secs_f64();
        let mean = if secs > 0.0 { self.used_us as f64 / 1e6 / secs } else { 0.0 };
        format!("{{\"launch\":{},\"ms\":{},\"peak\":{:.2},\"mean\":{mean:.2}}}", self.launch, (secs * 1000.0) as u64, self.peak.map_or(mean, |p| p.max(mean)))
    }
}

/// The live calls' meter: each [`CALL_WINDOW`], what the runs of every live session in a call
/// have used ([`CallMeter::look`]).
fn call_loop(w: Weak<Exec>) {
    loop {
        std::thread::sleep(CALL_WINDOW);
        let Some(x) = w.upgrade() else { return };
        x.meter_calls();
    }
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

/// The pin a lease-start's `rev` names: `{"makepad":<sha>,<nested name>:<sha>...}`, a null side
/// naming none.
fn rev_pin(rev: &makepad_strict_json::Value) -> Result<super::Pin, String> {
    let makepad_strict_json::Value::Obj(sides) = rev else { return Err("rev: not an object".into()) };
    let mut key = rev.get("makepad").and_then(makepad_strict_json::Value::as_str).ok_or("rev: no makepad commit")?.to_string();
    for (name, sha) in sides.iter().filter(|(name, _)| name != "makepad") {
        if let Some(sha) = sha.as_str() { key.push_str(&format!(",{name}={sha}")); }
    }
    super::Pin::parse(&key).ok_or_else(|| format!("rev: {key}: not makepad's 40-hex commit and nested repositories' by name"))
}

/// What a beat's look measured of a lease: its runs' peak RAM, what they hold, and what they
/// had done ([`os::Activity`]); None where it could not.
type Look = (Option<u64>, Option<u64>, Option<os::Activity>);

/// A WM-supervised service's state file (`~/.makepad/wm/<name>.state`: its `pid = `, its
/// `identity = `).
fn service_state(name: &str) -> PathBuf {
    makepad_network::fleet::makepad_home().join("wm").join(format!("{name}.state"))
}

/// The pid a WM-supervised service's supervisor recorded.
fn service_pid(name: &str) -> Option<u32> {
    std::fs::read_to_string(service_state(name)).ok().and_then(|t| caps::state_pid(&t))
}

/// The AI hub's process now, if one runs.
fn ai_hub_pid() -> Option<u32> {
    service_pid("ai-hub").filter(|p| crate::procinfo::alive(*p))
}

/// What the hub's log says of the AI hub as a timed run starts (`answer`: [`crate::local::ai_hub_quiet`]'s).
fn quiet_held_said(holder: &str, machine: &str, answer: Result<Option<makepad_strict_json::Value>, String>) -> Option<(Sev, String)> {
    match answer {
        Ok(None) => None,
        Ok(Some(v)) if v.get("reason").and_then(makepad_strict_json::Value::as_str) == Some("timed-lease") => Some((Sev::Info, format!("hub: {holder} holds {machine}: its AI hub takes no remote work until it ends"))),
        Ok(Some(_)) => Some((Sev::Warn, format!("hub: {holder} on {machine}: its AI hub does not hold quiet (it predates it): remote work can reach the run"))),
        Err(e) => Some((Sev::Warn, format!("hub: {holder} on {machine}: its AI hub was not told to hold quiet ({e}): remote work can reach the run"))),
    }
}

/// What the hub's log says of an AI hub that came back (`was`: the process asked before, if
/// one was) or came up (`now`) under a timed run, asked to hold quiet again.
fn quiet_again_said(holder: &str, machine: &str, was: Option<u32>, now: u32, answer: Result<Option<makepad_strict_json::Value>, String>) -> Option<(Sev, String)> {
    let back = match was {
        Some(was) => format!("its AI hub came back (pid {was} -> {now})"),
        None => format!("its AI hub came up (pid {now})"),
    };
    match answer {
        Ok(None) => None,
        Ok(Some(v)) if v.get("reason").and_then(makepad_strict_json::Value::as_str) == Some("timed-lease") => Some((Sev::Info, format!("hub: {holder} holds {machine}: {back}, asked again: it takes no remote work until the run ends"))),
        Ok(Some(_)) => Some((Sev::Warn, format!("hub: {holder} on {machine}: {back} and does not hold quiet (it predates it): remote work can reach the run"))),
        Err(e) => Some((Sev::Warn, format!("hub: {holder} on {machine}: {back} and was not told to hold quiet ({e}): remote work can reach the run"))),
    }
}

/// What the hub's log says as a timed run ends: the AI hub takes remote work again (`next`: the
/// timed run it holds for now, if one is still here) and what it refused meanwhile.
fn quiet_over_said(holder: &str, machine: &str, next: &str, answer: Result<Option<makepad_strict_json::Value>, String>) -> Option<(Sev, String)> {
    let v = match answer {
        Ok(None) => return None,
        Ok(Some(v)) => v,
        Err(e) => return Some((Sev::Warn, format!("hub: {holder} ended on {machine}: its AI hub was not told ({e}); it may hold quiet until the next timed run ends"))),
    };
    let refused = v.get("refused").and_then(makepad_strict_json::Value::as_u64).unwrap_or(0);
    let said: Vec<&str> = v.get("refused_said").and_then(makepad_strict_json::Value::as_arr).map(|a| a.iter().filter_map(makepad_strict_json::Value::as_str).collect()).unwrap_or_default();
    let what = match (refused, said.is_empty()) {
        (0, _) => "it refused no request meanwhile".to_string(),
        (n, true) => format!("it refused {n} request(s) meanwhile"),
        (n, false) => format!("it refused {n} request(s) meanwhile: {}", said.join("; ")),
    };
    let now = if next.is_empty() { "its AI hub takes remote work again".to_string() } else { format!("its AI hub holds quiet for {next}") };
    Some((Sev::Info, format!("hub: {holder} ended on {machine}: {now}; {what}")))
}

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
/// at `now`: still the old start while they stay below [`busy_past`] `pct`, a new one from a look
/// that shows work, none while nothing can be measured.
fn next_quiet(quiet: Option<(Instant, os::Activity)>, done: Option<os::Activity>, now: Instant, pct: u64) -> Option<(Instant, os::Activity)> {
    match (done, quiet) {
        (Some(to), Some((since, from))) if !busy_past(from, to, now.saturating_duration_since(since), pct) => Some((since, from)),
        (Some(to), _) => Some((now, to)),
        (None, _) => None,
    }
}

/// The CPU, in % of one core, a lease's runs may use and still do nothing: a hung CI story test
/// whose four JIT processes poll with timeouts used 1.1% (11-04), which a 1% bound took for
/// work; a build or a test that works uses a core or more. Only the CPU rate tells: the JIT
/// processes are not dumpable, so their `/proc` io, syscall and wchan are closed to the hub.
pub const IDLE_CPU_PCT: u64 = 5;

/// The CPU, in % of one core, under which a lease's runs are too slow to be at work (its beat's
/// `slow_s`): a test that works compiles, renders or computes at a core or more. A hung Fab test
/// whose app ticked its sim and whose harness polled the app ran at 4-12% of a core for 19 min,
/// past the idle bound all along, until a person cancelled it (L798418, 11-10).
pub const SLOW_CPU_PCT: u64 = 50;

/// Whether a lease's runs did work from `from` to `to`, `over` apart: CPU past
/// [`IDLE_CPU_PCT`] of a core (and past 0.2 s), more than 1 MB read from or written to storage
/// (not a socket's or a pipe's traffic: os/unix.rs `proc_activity`), or a process
/// gone with what it had done. A hung JIT waits at 0% CPU (a CI test sat 50 min so, unseen,
/// 11-04), or polls at 1%. Its time waiting for a CPU counts as the CPU it wanted: on a box
/// past its cores, a run at work gets a sliver of one. A stop's exit write starved so read as
/// a run that sat, and was KILLed before it wrote (a gate's devhub tests beside a live session's
/// builds, 18 tasks on 16 cores, 11-22); a leg starved so would read as slow.
fn busy(from: os::Activity, to: os::Activity, over: Duration) -> bool {
    busy_past(from, to, over, IDLE_CPU_PCT)
}

/// [`busy`] with the CPU bound at `pct` of a core.
fn busy_past(from: os::Activity, to: os::Activity, over: Duration, pct: u64) -> bool {
    let wanted = |a: os::Activity| a.cpu_us + a.wait_us;
    to.cpu_us < from.cpu_us || to.io_bytes < from.io_bytes || wanted(to).saturating_sub(wanted(from)) > (over.as_micros() as u64 * pct / 100).max(200_000) || to.io_bytes - from.io_bytes > 1 << 20
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

/// The tag of a hub's systemd units, from its ledger folder: the same hub's across its restarts
/// (a run's unit is kept in its ledger anyway), another hub's never. Test hubs on one box named
/// their runs' scopes alike (`devhub-lease-L1-1`): a second start beside the first failed, or one
/// lease's end stopped the other's scope (lease::exec tests in two processes at once, 11-06).
fn unit_tag(state: &Path) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in state.to_string_lossy().bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

#[cfg(any(target_os = "linux", test))]
const SCOPE_CPU: &str = "\0scope:";

/// A named outside process's scope; no lease means a store build whose run is gone.
#[cfg(any(target_os = "linux", test))]
struct CpuScope {
    unit: String,
    lease: Option<String>,
    /// the run its scope was made for (`devhub-lease-<units>-<lease>-<n>`)
    run: Option<u32>,
    measured: bool,
}

/// `wait` tasks waiting for a CPU split by each lease's runnable threads (`runnable`) of the
/// box's (`total`, at least theirs): who made the box's run queue. A lease under half a task
/// is left out.
fn waits_by_lease(wait: f64, runnable: &BTreeMap<String, u32>, total: u32) -> Vec<(String, f64)> {
    let total = total.max(runnable.values().sum());
    if total == 0 { return Vec::new(); }
    let mut by: Vec<(String, f64)> = runnable.iter().map(|(id, n)| (id.clone(), wait * *n as f64 / total as f64)).filter(|(_, w)| *w >= 0.5).collect();
    by.sort_by(|a, b| b.1.total_cmp(&a.1));
    by
}

/// Scope counters replace only their runs' ticks: short-lived test children disappeared
/// between caps looks and read as 4.5 cores outside every lease on a bench box (11-14).
/// A process in the scope of a run that ended, of a lease that still runs, is that run's leftover,
/// not an ended lease's (`lease_here`: the leases this box holds now).
#[cfg(any(target_os = "linux", test))]
fn merge_scope_cpu(mut split: crate::top::CpuSplit, scopes: &BTreeMap<String, f64>, scope_lease: &dyn Fn(u32) -> Option<CpuScope>, live_here: &dyn Fn(&str) -> bool, lease_here: &dyn Fn(&str) -> bool) -> crate::top::CpuSplit {
    let mut leases = BTreeMap::<String, f64>::new();
    for (id, cores) in split.leases {
        let id = if let Some(id) = id.strip_prefix(SCOPE_CPU) {
            if scopes.contains_key(id) { continue; }
            id.to_string()
        } else { id };
        *leases.entry(id).or_default() += cores;
    }
    for (id, cores) in scopes { *leases.entry(id.clone()).or_default() += cores; }
    split.outside.retain_mut(|(what, pid, cores)| {
        let Some(scope) = scope_lease(*pid) else { return true };
        if let Some(id) = &scope.lease {
            if live_here(&scope.unit) {
                if !scope.measured { *leases.entry(id.clone()).or_default() += *cores; }
                return false;
            }
            what.push_str(&match scope.run.filter(|_| lease_here(id)) {
                Some(n) => format!(" (left by its ended run {n} of lease {id})"),
                None => format!(" (left by ended lease {id})"),
            });
        } else {
            what.push_str(" (left by an ended JIT build)");
        }
        true
    });
    split.leases = leases.into_iter().collect();
    split.leases.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    // (a measured run scope's compilers are its lease's builds)
    for b in &mut split.builds {
        if let Some(id) = b.0.strip_prefix(SCOPE_CPU) { b.0 = id.to_string(); }
    }
    split
}

/// A lane whose leases work in the box's shared tree, not a native tree of their own
/// ([`super::native_lane`]): CI and landings, and every lane on a user's box.
#[cfg(feature = "checkout")]
fn shared_tree_lane(lane: &str, role: super::Role) -> bool {
    matches!(lane, super::sched::CI_LANE | super::sched::LANDING_LANE) || role.users()
}

impl Exec {
    /// The executor over `conf.work`, its ledger in `state`; leases left in the ledger by a
    /// previous hub are taken back (the watchdog ends them unless the master still beats them).
    pub fn new(machine: &str, conf: LeaseConf, state: PathBuf, opts: Opts, publish: Publish) -> Arc<Exec> {
        let guard = Guard::new(machine, Roots::of(&conf.work), conf.disk_floor_gb, conf.role.users(), publish.clone());
        let scopes = opts.scopes.unwrap_or_else(user_bus);
        let units = unit_tag(&state);
        if scopes { os::reset_failed_units(&format!("devhub-jit-{units}-*")); }
        let x = Arc::new_cyclic(|_this| Exec { this: _this.clone(), machine: machine.to_string(), publish, dev: AtomicBool::new(true), dev_req: Default::default(), conf: std::sync::RwLock::new(conf), state, opts, scopes, units, leases: crate::held::Held::new("leases", BTreeMap::new()),
            starting: crate::held::Held::new("lease start", ()), cleaning: Default::default(), series: Default::default(), dark_said: Default::default(), thermal: Default::default(), endings: Mutex::new(BTreeMap::new()), over_disk: Mutex::new(None), #[cfg(feature = "checkout")] store_builds: Default::default(), #[cfg(target_os = "linux")] memory_unexplained: AtomicBool::new(false), orphans: Default::default(), #[cfg(target_os = "linux")] scope_cpu: Default::default(), guard, disk_use: Default::default(), #[cfg(unix)] noise: Default::default(), #[cfg(unix)] span: Default::default(), quiet_hub: Mutex::new(None), timed_said: Default::default(), user: Default::default(), hand: Default::default(), restarting: AtomicBool::new(false), #[cfg(feature = "checkout")] hub_build: Default::default(), last_resident_gb: Mutex::new(0.0), trouble: Mutex::new(String::new()), master_seen: Mutex::new(Instant::now()), master_away_said: AtomicBool::new(false) });
        #[cfg(all(windows, feature = "checkout"))]
        drop_aside();
        // what a hub that stopped left in the trash
        super::disk::empty_trash_soon(&x.conf().work);
        // what a copy to its store left when the last one ended (the master copies it again)
        super::jitstore::Store::new(&x.conf().work).clear_copies();
        // the overlay lanes running here keep their store refs (their ships read the tables)
        #[cfg(feature = "checkout")]
        {
            let w = Arc::downgrade(&x);
            super::relaxgit::set_live(Box::new(move || w.upgrade().map(|x| x.leases.lock().unwrap().values().filter(|l| l.src == Src::Overlay).map(|l| overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or(""))).collect()).unwrap_or_default()));
        }
        let mut back = Vec::new();
        for e in std::fs::read_dir(&x.state).into_iter().flatten().flatten() {
            let p = e.path();
            // (a start an earlier hub of this box did not finish: nothing starts here now)
            if p.extension().is_some_and(|e| e == START_MARK) { let _ = std::fs::remove_file(&p); continue; }
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
                            super::tree::hold(&l.id, &pin, &l.work, super::sched::tests_shard(&l.lane, l.kind));
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
        #[cfg(feature = "checkout")]
        x.stop_unowned_builds();
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
        let w = Arc::downgrade(&x);
        let _ = std::thread::Builder::new().name("devhub-calls".into()).spawn(move || call_loop(w));
        #[cfg(target_os = "linux")]
        if x.opts.watch_memory {
            let w = Arc::downgrade(&x);
            let _ = std::thread::Builder::new().name("devhub-mem".into()).spawn(move || memory_loop(w));
        }
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

    /// A timed run holds this box: its AI hub takes no remote work until it ends (a fractal zoom
    /// on the Mac offloaded to the bench box's hub at 59% of a core under a timed run, 11-06).
    /// Asked before the run starts, said in the hub's log; a hub that cannot hold is said, and the
    /// run's not-quiet verdict names what ran beside it. A user's own box keeps its AI hub: the
    /// person uses it, and a timed run there shares the box with them anyway.
    fn hold_quiet(&self, holder: &str) {
        if !self.opts.watch_wm || self.conf().role.users() {
            return;
        }
        let pid = ai_hub_pid();
        let answer = crate::local::ai_hub_quiet(holder);
        // a hub that does not answer yet (starting) is asked once it does ([`Exec::keep_quiet`])
        *self.quiet_hub.lock().unwrap() = if matches!(answer, Ok(None)) { None } else { pid };
        if let Some((sev, line)) = quiet_held_said(holder, &self.machine, answer) {
            self.emit("hub", sev, line);
        }
    }

    /// While a timed run holds the box: an AI hub that came back since its quiet was asked (a
    /// crash the WM restarted; its self-update waits for the run, [`Exec::service_restart`]), or
    /// came up, holds none, and is asked before anything else reaches it. A look at its pid file
    /// each [`BOX_EVERY`]; a hub not answering yet is asked at the next.
    fn keep_quiet(&self) {
        if !self.opts.watch_wm || self.conf().role.users() {
            return;
        }
        let Some(holder) = self.leases.lock().unwrap().values().find(|l| l.kind == Kind::Timed).map(|l| format!("timed lease {} of {}", l.id, l.lane)) else { return };
        let was = *self.quiet_hub.lock().unwrap();
        let Some(now) = ai_hub_pid().filter(|p| Some(*p) != was) else { return };
        let answer = crate::local::ai_hub_quiet(&holder);
        if matches!(answer, Ok(None)) {
            return;
        }
        *self.quiet_hub.lock().unwrap() = Some(now);
        if let Some((sev, line)) = quiet_again_said(&holder, &self.machine, was, now, answer) {
            self.emit("hub", sev, line);
        }
    }

    /// A timed run ended: the AI hub takes remote work again (or holds for another timed run
    /// still here), and the hub's log says what it refused meanwhile. Off the lease's end.
    fn end_quiet(&self, id: &str, lane: &str) {
        if !self.opts.watch_wm || self.conf().role.users() {
            return;
        }
        let next = self.leases.lock().unwrap().values().find(|l| l.kind == Kind::Timed && l.id != id).map(|l| format!("timed lease {} of {}", l.id, l.lane)).unwrap_or_default();
        let (emit, machine, holder) = (self.emitter(), self.machine.clone(), format!("timed lease {id} of {lane}"));
        let _ = std::thread::Builder::new().name("devhub-quiet-end".into()).spawn(move || {
            if let Some((sev, line)) = quiet_over_said(&holder, &machine, &next, crate::local::ai_hub_quiet(&next)) {
                emit("hub", sev, line);
            }
        });
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

    /// `lease-conf` (show) or `lease-conf set k=v ... [by=<agent>]`: the master sets this node's
    /// role and budgets; written to `lease.conf` (kept across restarts) and in force at once.
    /// `toolchain-probe` (`devhub node <n> probe`): the toolchain probe now, not at its next turn
    /// ([`toolchain_loop`]); what it found.
    fn toolchain_probe(&self) -> String {
        if self.timed_live() {
            return err("waits: a timed lease runs here: the probe (a compile and a link) runs after it");
        }
        let why = probe_toolchain_now(self);
        format!("{{\"node\":{},\"links\":{},\"why\":{}}}", json_str(&self.machine), why.is_none(), why.as_deref().map_or("null".into(), json_str))
    }

    fn lease_conf(&self, rest: &str) -> String {
        let mut w = rest.split_whitespace();
        if w.next() == Some("set") {
            let (pairs, by) = conf_set_args(w);
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
            self.guard.set_policy(conf.disk_floor_gb, conf.role.users());
            *self.conf.write().unwrap() = conf;
            let what: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect();
            // in the inbox, with who asked: nobody could say who had set a pool box to bench (11-06)
            self.emit("lease", Sev::Warn, format!("lease: {} lease.conf set {} by {}", self.machine, what.join(" "), by.as_deref().unwrap_or("a caller without DEVHUB_AGENT")));
        }
        let apply = if crate::sync::store::source_store().is_some() { "store" } else { "checkout" };
        let mut c = self.measure(apply);
        self.note_temps(&mut c);
        c.ram_resident_gb = self.resident_gb();
        c.tree = self.tree_key();
        c.deploy = self.deploy_key();
        c.to_json()
    }

    /// [`caps::measure`] of the box, its CPU as the hub's [`BoxLook`] sees it ([`BoxLook::cpu`]).
    fn measure(&self, apply: &str) -> super::Caps {
        let mut c = caps::measure(&self.machine, &self.conf(), apply);
        if let Some((pct, wait)) = self.looked_cpu() {
            (c.cpu_pct, c.cpu_wait) = (Some(pct), Some(wait));
        }
        c
    }

    fn looked_cpu(&self) -> Option<(f64, f64)> {
        self.opts.box_look.as_ref().and_then(|l| (l.cpu)())
    }

    /// Keep alerts across polls and leave one temperature line a minute in the hub's log and, on
    /// Linux, the box's journal: the hub's own log is a file its WM launcher writes, and after a
    /// box froze hot (11-15) the previous boot's journal is what shows the climb.
    fn note_temps(&self, c: &mut super::Caps) {
        let log = {
            let mut t = self.thermal.lock().unwrap();
            c.hot = t.0.update(&c.temps);
            let log = !c.temps.is_empty() && t.1.is_none_or(|at| at.elapsed() >= Duration::from_secs(60));
            if log { t.1 = Some(Instant::now()); }
            log
        };
        if log {
            let line = format!("devhub hub: temps {}", c.temps.iter().map(|t| format!("{}={:.0}", t.sensor, t.c)).collect::<Vec<_>>().join(" "));
            eprintln!("{line}");
            #[cfg(target_os = "linux")]
            journal(&line);
        }
    }

    /// The box's CPU since the last caps poll by lease, and the busiest processes outside the
    /// leases' runs ([`super::Caps::busy_outside`]).
    fn cpu_split(&self) -> crate::top::CpuSplit {
        let groups = self.lease_groups();
        #[cfg(target_os = "linux")]
        let (groups, scopes, units, held) = {
            let mut groups = groups;
            let (runs, held): (Vec<(String, Arc<Run>)>, BTreeSet<String>) = {
                let leases = self.leases.lock().unwrap();
                (leases.values().flat_map(|l| l.runs.iter().filter(|r| r.unit.is_some() && r.pid != 0 && r.ended() == 0).map(|r| (l.id.clone(), r.clone()))).collect(), leases.keys().cloned().collect())
            };
            let mut last = self.scope_cpu.lock().unwrap();
            let mut next = BTreeMap::new();
            let mut scopes = BTreeMap::<String, f64>::new();
            let mut units = BTreeMap::new();
            for (id, r) in runs {
                let key = (id.clone(), r.n);
                let cores = os::activity(r.pid, r.start, r.unit.as_deref(), &r.exit_file.with_extension("usage"), &r.hold).and_then(|a| {
                    let now = Instant::now();
                    next.insert(key.clone(), (now, a.cpu_us));
                    let (then, cpu) = last.get(&key)?;
                    let secs = now.duration_since(*then).as_secs_f64();
                    (secs > 0.0).then(|| a.cpu_us.saturating_sub(*cpu) as f64 / 1_000_000.0 / secs)
                });
                units.insert(r.unit.clone().unwrap(), (id.clone(), cores.is_some()));
                // Only measured scopes get the private label: a run's first look keeps its ticks,
                // even beside another measured run of the same lease. Store groups keep theirs.
                if let Some(cores) = cores {
                    groups.insert(r.pid, format!("{SCOPE_CPU}{id}"));
                    *scopes.entry(id).or_default() += cores;
                }
            }
            *last = next;
            drop(last);
            #[cfg(feature = "checkout")]
            for (id, r) in self.store_builds.lock().unwrap().iter().filter(|(_, r)| r.pid != 0 && r.ended() == 0) {
                if let Some(unit) = &r.unit { units.insert(unit.clone(), (id.clone(), false)); }
            }
            (groups, scopes, units, held)
        };
        // the hub's own build and its deployment's are their rooms' (the master's leases, which
        // charged them)
        let (hub_build, deploy_build) = (HUB_BUILD_GROUP.load(Ordering::Acquire), DEPLOY_BUILD_GROUP.load(Ordering::Acquire));
        let split = crate::top::cpu_split(&|pgid| groups.get(&pgid).cloned().or_else(|| match pgid {
            0 => None,
            g if g == hub_build => Some(super::sched::HUB_BUILD.to_string()),
            g if g == deploy_build => Some(super::sched::DEPLOY_BUILD.to_string()),
            _ => None,
        }));
        #[cfg(target_os = "linux")]
        {
            let lease_prefix = format!("devhub-lease-{}-", self.units);
            let jit_prefix = format!("devhub-jit-{}-", self.units);
            merge_scope_cpu(split, &scopes, &|pid| {
                let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
                let path = cgroup.lines().find_map(|s| s.strip_prefix("0::"))?;
                let unit = Path::new(path).file_name()?.to_str()?.strip_suffix(".scope")?.to_string();
                if let Some((id, measured)) = units.get(&unit) {
                    return Some(CpuScope { unit, lease: Some(id.clone()), run: None, measured: *measured });
                }
                let (lease, run) = if let Some(run) = unit.strip_prefix(&lease_prefix) {
                    let (id, n) = run.rsplit_once('-')?;
                    let n = n.parse::<u32>().ok()?;
                    if !valid_name(id) { return None; }
                    (Some(id.to_string()), Some(n))
                } else if unit.starts_with(&jit_prefix) { (None, None) } else { return None };
                Some(CpuScope { unit, lease, run, measured: false })
            }, &|unit| units.contains_key(unit), &|id| held.contains(id))
        }
        #[cfg(not(target_os = "linux"))]
        { split }
    }

    /// The box's tasks waiting for a CPU (`wait`, [`caps::Caps::cpu_wait`]) by lease: each lease's
    /// share of the runnable threads now, its running runs' scopes' of the box's
    /// ([`waits_by_lease`]). Nothing while hardly anything waits.
    fn lease_waits(&self, wait: Option<f64>) -> Vec<(String, f64)> {
        let Some(wait) = wait.filter(|w| *w >= 0.5) else { return Vec::new() };
        let Some(total) = os::procs_running() else { return Vec::new() };
        let runs: Vec<(String, Arc<Run>)> = self.leases.lock().unwrap().values().flat_map(|l| l.runs.iter().filter(|r| r.unit.is_some() && r.pid != 0 && r.ended() == 0).map(|r| (l.id.clone(), r.clone()))).collect();
        let mut runnable = BTreeMap::<String, u32>::new();
        for (id, r) in runs {
            *runnable.entry(id).or_default() += os::scope_runnable(r.pid, r.start, r.unit.as_deref(), &r.exit_file.with_extension("usage"));
        }
        // (this hub's own thread reading is one of the box's)
        waits_by_lease(wait, &runnable, total.saturating_sub(1))
    }

    /// The process groups of the box's leases, each with its lease: their runs', and a store build's
    /// the lease's whose overlay started it ([`Self::overlay_build`]; it stops when that lease
    /// ends). A box prebuild charged a core built its dev JIT on all 16 at 7 cores, read as the
    /// hub's cargo outside every lease, and its class measured 0.04 CPU seconds in 69 s (11-11).
    fn lease_groups(&self) -> std::collections::HashMap<u32, String> {
        let runs: Vec<(u32, String)> = self.leases.lock().unwrap().values().flat_map(|l| l.runs.iter().map(|r| r.pid).filter(|p| *p != 0).map(|p| (p, l.id.clone()))).collect();
        runs.into_iter().chain(self.store_groups()).collect()
    }

    #[cfg(feature = "checkout")]
    fn store_groups(&self) -> Vec<(u32, String)> {
        self.store_builds.lock().unwrap().iter().filter(|(_, r)| r.pid != 0).map(|(lease, r)| (r.pid, lease.clone())).collect()
    }

    #[cfg(not(feature = "checkout"))]
    fn store_groups(&self) -> Vec<(u32, String)> {
        Vec::new()
    }

    /// What the box holds outside its leases, GB ([`super::Caps::ram_resident_gb`]): the memory in
    /// use less what the leases' live runs hold, at one look ([`held_now`]), and less what the
    /// hub's own build and its deployment's build hold ([`builds_held`]). Where the OS cannot
    /// tell a run's memory (Windows), the last look while no run or build was live stands.
    fn resident_gb(&self) -> f64 {
        let runs: Vec<Arc<Run>> = self.leases.lock().unwrap().values().flat_map(|l| l.runs.iter().cloned()).collect();
        let live = runs.iter().any(|r| r.pid != 0 && r.ended() == 0) || !build_groups().is_empty();
        let held = held_now(&runs).unwrap_or(0) + builds_held().unwrap_or(0);
        let mut last = self.last_resident_gb.lock().unwrap();
        if let Some(used) = crate::procinfo::in_use().filter(|_| !live || os::MEASURES_GROUPS) {
            *last = used.saturating_sub(held) as f64 / caps::GB;
        }
        *last
    }

    /// One look at the box's screen: the WM's answer (`Err`: no WM of ours) ([`super::user`]).
    pub fn sense_user(&self, answer: Result<&str, String>) {
        self.user.lock().unwrap().update(crate::proto::wall_ms(), answer);
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

    /// Says in the inbox that the box's WM has no size, and in the stream that it has one again.
    /// The saying outlives this hub: devhub landings roll it every few minutes.
    fn say_dark(&self, dark: &str) {
        let marker = crate::proto::home().join("sync").join("wm-dark.said");
        let mut said = self.dark_said.lock().unwrap();
        said.said = said.said || marker.exists();
        let Some((sev, line)) = said.note(&self.machine, dark, crate::proto::wall_ms()) else { return };
        if said.said {
            let _ = std::fs::create_dir_all(crate::proto::home().join("sync"));
            let _ = std::fs::write(&marker, format!("{line}\n"));
        } else {
            let _ = std::fs::remove_file(&marker);
        }
        drop(said);
        self.emit("hub", sev, line);
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

    /// Under Linux memory pressure, stop the lease furthest past its weight before the kernel
    /// chooses a process from any lease. Process walks run outside the lease table lock.
    #[cfg(target_os = "linux")]
    fn look_over_memory(&self, total: u64, avail: u64) {
        let floor = (total as f64 * super::sched::RAM_MARGIN_SHARE) as u64;
        if avail >= floor {
            self.memory_unexplained.store(false, Ordering::Release);
            return;
        }
        let leases: Vec<_> = self.leases.lock().unwrap().values().filter(|l| l.memory_end.is_none()).map(|l| {
            (MemTook { id: l.id.clone(), lane: l.lane.clone(), kind: l.kind, held: 0, weighed: l.weighed_gb, peak: l.peak_gb, asked: l.ram_max_gb.is_some() }, l.runs.clone())
        }).collect();
        let took: Vec<_> = leases.iter().filter_map(|(l, runs)| held_now(runs).map(|held| MemTook { held, ..l.clone() })).collect();
        let Some((id, why)) = lease_over_memory(&self.machine, avail, floor, &took) else {
            if !self.memory_unexplained.swap(true, Ordering::AcqRel) {
                let gb = (1u64 << 30) as f64;
                self.emit("lease", Sev::Warn, format!("{} has {:.1} GB free, under its {:.1} GB floor, but no lease holds more than it was weighed at", self.machine, avail as f64 / gb, floor as f64 / gb));
            }
            return;
        };
        // the processes that hold it, read before the table's lock (a process walk)
        let why = format!("{why}{}", leases.iter().find(|(l, _)| l.id == id).map_or(String::new(), |(_, runs)| held_where_line(runs)));
        let mut current = self.leases.lock().unwrap();
        let Some(l) = current.get_mut(&id).filter(|l| l.memory_end.is_none()) else { return };
        let runs = &leases.iter().find(|(l, _)| l.id == id).unwrap().1;
        if !runs.iter().any(|r| l.runs.iter().any(|now| Arc::ptr_eq(r, now))) { return; }
        let held = took.iter().find(|l| l.id == id).unwrap().held;
        l.peak_ram_bytes = Some(l.peak_ram_bytes.unwrap_or(0).max(held));
        l.held_bytes = Some(l.held_bytes.unwrap_or(0).max(held));
        l.held_now_bytes = Some(held);
        l.memory_end = Some(why.clone());
        for r in &l.runs {
            if r.pid != 0 && os::group_alive(r.pid, r.start, &r.hold) {
                os::signal_group(r.pid, true, &r.hold);
            }
        }
        self.emit("lease", Sev::Err, why);
        self.save(l);
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
                // (its lane's own JIT tree too: a reclaim takes those of lanes with no lease here)
                let lane = overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or(""));
                for p in [dev.join("overlay").join(&lane), dev.join("lane-jit").join(&lane), dev.join("git"), dev.join("run-home"), dev.join("tree")] { leases.push((p, l.id.clone())); }
            }
            for h in &l.inputs { leases.push((super::ship::input_dir(&self.conf().work, h), l.id.clone())); }
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
            "lease-weight" => vec![self.weigh(rest)],
            "lease-log" => vec![self.log(rest)],
            "lease-end" => vec![with_load(self.end_cmd(rest), || super::caps::load_now().map(|(cpu, avail)| (self.looked_cpu().map_or(cpu, |(pct, _)| pct), avail)))],
            "lease-beat" => vec![self.beat(rest)],
            "lease-jits" => vec![lease_jits_of(self, rest)],
            "lease-yield" => vec![lease_yield(self, rest)],
            "lease-tell" => vec![lease_tell(self, rest)],
            "lease-kill" => vec![lease_kill(self, rest)],
            "lease-why" => vec![lease_why(self, rest)],
            "lease-failed" => vec![lease_failed(self, rest)],
            "lease-pid" => vec![lease_pid(self, rest)],
            "lease-box" => vec![self.lease_box(rest)],
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
            "input-have" | "input-put" | "input-home" => vec![self.input(word, rest)],
            "jit-read" | "jit-put" => vec![self.jit_copy(word, rest)],
            "jit-keep" | "jit-unkeep" => vec![self.jit_keep(word, rest)],
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
            "toolchain-probe" => vec![self.toolchain_probe()],
            "caps" => {
                let apply = if crate::sync::store::source_store().is_some() { "store" } else { "checkout" };
                let mut c = self.measure(apply);
                self.note_temps(&mut c);
                c.ram_resident_gb = self.resident_gb();
                let split = self.cpu_split();
                (c.busy_leases, c.busy_outside, c.lease_builds) = (split.leases, split.outside, split.builds);
                c.wait_leases = self.lease_waits(c.cpu_wait);
                static GPU_LAST: Mutex<Option<(Instant, Vec<(u32, u64)>)>> = Mutex::new(None);
                c.mac_gpu = crate::top::gpu_since(&GPU_LAST).map(|rates| {
                    let rows = crate::top::named();
                    let groups: BTreeSet<u32> = self.leases.lock().unwrap().values()
                        .flat_map(|l| l.runs.iter().map(|r| r.pid).filter(|p| *p != 0)).collect();
                    let (pct, mut on) = crate::top::gpu_split(&rates, &rows, std::process::id(), &|pgid| groups.contains(&pgid).then(String::new));
                    on.truncate(3);
                    (pct, on)
                });
                c.desk_link = crate::embed::desk_link_state();
                c.dev_req = match self.dev_req.load(Ordering::Acquire) { 1 => Some(true), 2 => Some(false), _ => None };
                c.tree = self.tree_key();
                c.deploy = self.deploy_key();
                #[cfg(feature = "checkout")]
                { c.applying = crate::sync::serve::applying(); }
                c.stalls = crate::held::stalls();
                c.trouble = std::mem::take(&mut *self.trouble.lock().unwrap());
                {
                    let user = self.user.lock().unwrap();
                    (c.input_idle_ms, c.watched) = (user.idle_ms(crate::proto::wall_ms()), user.watched());
                }
                c.hand_held = self.hand_held(&split.apps);
                self.say_dark(&c.wm_dark);
                vec![c.to_json()]
            }
            "disk" => match rest.strip_prefix("clean ") {
                Some(lane) => vec![self.clean_lane(lane.trim())],
                None if rest.starts_with("series-keep") => vec![crate::jitcache::series_keep(rest["series-keep".len()..].trim())],
                None => {
                    // `[dry] [need <GB>]`: a dry run can say what a reclaim for a queued need takes
                    let words: Vec<&str> = rest.split_whitespace().collect();
                    let dry = words.iter().any(|w| *w == "dry" || *w == "--dry-run");
                    let need_gb: f64 = words.windows(2).find(|w| w[0] == "need").and_then(|w| w[1].parse().ok()).unwrap_or(0.0);
                    // `keep <lane>,..`: lanes with queued overlay asks keep their own JIT trees
                    let mut live = self.live();
                    let lane_jit = self.conf().work.join("dev/lane-jit");
                    for lane in words.windows(2).find(|w| w[0] == "keep").map_or("", |w| w[1]).split(',').filter(|l| valid_name(l)) {
                        live.leases.push((lane_jit.join(lane), format!("queued in lane {lane}")));
                    }
                    self.guard.reclaim(&live, dry, (need_gb.max(0.0) * (1u64 << 30) as f64) as u64)
                }
            },
            "heap" => {
                let lines = crate::heap::report(rest.trim().parse().unwrap_or(12));
                // a JIT host without a checkout has no relaxgit store
                #[cfg(feature = "checkout")]
                let lines = { let mut lines = lines; lines.insert(1, super::relaxgit::counters()); lines };
                lines
            }
            // its own word: an older hub's `disk` takes any other words for a reclaim
            "lane-dirs" => vec![self.lane_sizes()],
            "recover-get" => vec![self.recover_get(rest)],
            "recover-del" => vec![self.recover_del(rest)],
            _ => vec![err(format!("unknown service {word}"))],
        }
    }

    /// An app of ours a person uses outside every lease, by this look ([`super::hand`]): said
    /// when a hold starts and when it ends.
    fn hand_held(&self, apps: &[(u32, String)]) -> Option<super::HandHeld> {
        #[cfg(all(unix, feature = "checkout"))]
        let records = super::hand::in_use_records();
        #[cfg(not(all(unix, feature = "checkout")))]
        let records: Vec<(u32, String)> = Vec::new();
        let seen = super::hand::sense(apps, &records);
        let mut track = self.hand.lock().unwrap();
        let was = track.held();
        let held = track.update(crate::proto::wall_ms(), seen, apps);
        drop(track);
        match (&was, &held) {
            (None, Some(h)) => self.emit("hand", Sev::Info, format!("{}: {}: it takes no work that does not name it", self.machine, h.line())),
            (Some(w), None) => self.emit("hand", Sev::Info, format!("{}: {w} no more: it takes work again", self.machine)),
            _ => {}
        }
        held
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
        if let Some(k) = unknown_key(&v, START_KEYS).or_else(|| v.get("cost").and_then(|c| unknown_key(c, START_COST_KEYS))) {
            return err(format!("lease-start {id}: this hub does not read the field `{k}` (the master sends a field only to a hub that reports its feature: a devhub bug)"));
        }
        let _starting = self.starting_clean(&[&lane, &s("warms")]);
        if self.restarting.load(Ordering::Acquire) { return err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine)); }
        let _mark = StartMark::new(&self.state, &id);
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
            // a GPU timing ([`super::sched::Req::gpu`]): its box is quiet when its GPU is
            let gpu = v.get("gpu").and_then(|g| g.as_bool()).unwrap_or(false);
            let mark = self.quiet().join(format!("{id}.cmd"));
            let _ = std::fs::create_dir_all(self.quiet());
            let _ = std::fs::write(&mark, format!("timed lease {id} of {lane}\n"));
            quiet.0 = Some(mark);
            // past its wait for a quiet box, its master starts it beside what runs outside every
            // lease ([`super::sched::Sched::starts_unquiet`]): that holds it no longer here
            let unquiet = v.get("unquiet").and_then(|u| u.as_str());
            if let Some(why) = self.builds_here().or_else(|| self.timed_waits(gpu, unquiet.is_some())) {
                // said once for each thing it waits on, not at each of the master's retries: the
                // same reason with other seconds, pids or loads is the same thing
                let what: String = why.chars().filter(|c| !c.is_ascii_digit()).collect();
                if self.timed_said.lock().unwrap().insert(id.to_string(), what.clone()) != Some(what) {
                    self.emit("lease", Sev::Info, format!("timed lease {id} of {lane} waits on {}: {why}", self.machine));
                }
                return err(format!("{why}: {}", super::TIMED_AFTER));
            }
            self.timed_said.lock().unwrap().remove(&id);
            if let Some(beside) = unquiet {
                self.emit("lease", Sev::Warn, format!("timed lease {id} of {lane} starts on {} beside work outside every lease, past its wait for a quiet box ({beside}): its release measures it", self.machine));
            }
            #[cfg(unix)]
            self.first_span(&id, gpu);
            self.hold_quiet(&format!("timed lease {id} of {lane}"));
        }
        // a real lease preempts the background pre-warm at once (its processes are gone before
        // the lease takes the tree, its JIT cache consistent); a timed one whatever its source
        #[cfg(feature = "checkout")]
        if kind.writes() && (src == Src::Tree || kind == Kind::Timed) {
            crate::sync::prewarm::cancel(&format!("lease {id} ({lane})"));
        }
        let rev = v.get("rev").filter(|r| r.get("makepad").is_some());
        let pinned = match rev.map(rev_pin).transpose() {
            Ok(p) => p,
            Err(e) => return err(format!("lease-start {id}: {e}")),
        };
        // a build `--for` another box: that box's CPU in place of this one's (`native`). The flag
        // comes after the workspace config's for the host target, and rustc takes the last
        // `-C target-cpu`.
        let cpu_target = match v.get("cpu_target").and_then(|t| t.as_str()) {
            Some(t) if super::valid_cpu_target(t) && !HOST_RUSTFLAGS.is_empty() => Some(t.to_string()),
            Some(t) => return err(format!("lease-start {id}: cannot build for CPU `{t}` here")),
            None => None,
        };
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
            // A screen lease shows its windows on the node's live WM, or on the display the hub's
            // session names: with neither (its WM down, or restarting), its app finds no window
            // system and its run is lost. Refused, it is placed again (the master's
            // [`super::sched::Sched::no_display`]). A fleet update restarted a node's WM as a
            // live Fab launch started there, and the launch died: "no live makepad WM" (11-16).
            if kind == Kind::Screen && caps::display().is_none() && !["WAYLAND_DISPLAY", "DISPLAY"].iter().any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty())) {
                return err(format!("lease-start {id}: {NO_LIVE_WM} (its WM is down or restarting)"));
            }
            Vec::new()
        };
        // the toolchain this lease's builds use names its JIT binaries (read_rustc keys them); the
        // lease keeps what it read, and its end compares (the toolchain verdict)
        let rustc = super::caps::read_rustc();
        let rustc = if rustc.release.is_empty() && rustc.commit.is_empty() { String::new() } else { format!("{} {}", rustc.release, rustc.commit) };
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
        let (work, target, pin, what) = if src == Src::Overlay {
            let Some(pin) = pinned.clone() else { return err("overlay: makepad rev layer: pinned commit required") };
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
        } else if pinned.is_some() || (kind == Kind::Ci && self.opts.tree.is_some()) {
            match self.take_tree(&id, tree_lane, kind, pinned.clone()) {
                Ok(x) => x,
                Err(e) => return err(format!("lease-start {id}: {e}")),
            }
        } else {
            (w.join("lane").join(&lane).join("src"), w.join("target").join(format!("lane-{lane}")), None, "lane dir".to_string())
        };
        // a build `--for` another box builds in a target dir of its own: this box's own build
        // stays warm
        let target = match &cpu_target { Some(t) => target.join(format!("for-{t}")), None => target };
        let keep = tmp.join(crate::lease::KEEP_DIR);
        // made here, as every dir its environment names: a gate job that wrote its script into it
        // found none and exited 127 before its first step (J14641/J14642, 11-14); its end takes
        // it again when nothing was kept in it
        let artifacts = test_artifacts(&target, &id);
        for d in [&keep, &work, &target, &artifacts] {
            if let Err(e) = std::fs::create_dir_all(d) {
                #[cfg(feature = "checkout")]
                if src == Src::Tree && pin.is_some() {
                    let r = super::tree::release(&id, &work, &w.join("recovered"), &format!("{lane}-{id}-start"));
                    self.report_tree(&format!("{id} failed to start"), &r);
                }
                return err(if src == Src::Overlay { format!("overlay: makepad {} run directory layer: {}: {e}", pinned.as_ref().map_or("", |p| p.makepad.as_str()), d.display()) } else { format!("{}: {e}", d.display()) });
            }
        }
        let law = [
            ("CARGO_TARGET_DIR", target.to_string_lossy().into_owned()),
            ("TMPDIR", tmp.to_string_lossy().into_owned()),
            // a failed step's logs go here: its holder keeps them before the tmp goes (KEEP_DIR)
            ("DEVHUB_KEEP", keep.to_string_lossy().into_owned()),
            // made when a timed ask waits on this box behind this run past twice its estimate
            // (`lease-yield`): a series or bench stops at its next step
            ("DEVHUB_YIELD", tmp.join(crate::lease::YIELD_FILE).to_string_lossy().into_owned()),
            // what its holder tells the run (`lease-tell`): a tests shard's units from its run's queue
            ("DEVHUB_TOLD", tmp.join(crate::lease::TOLD_FILE).to_string_lossy().into_owned()),
            ("CARGO_INCREMENTAL", "0".into()),
            ("MAKEPAD_HIDE_WINDOWS", "1".into()),
            ("MAKEPAD_MUTE", "1".into()),
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
            ("MAKEPAD_TEST_ARTIFACTS", artifacts.to_string_lossy().into_owned()),
            ("CARGO_NET_OFFLINE", "true".into()),
        ];
        #[cfg(target_os = "linux")]
        let law: Vec<_> = law.into_iter().chain(wm_env).collect();
        let law: Vec<_> = law.into_iter().chain(cpu_target.map(|t| (HOST_RUSTFLAGS, format!("-C target-cpu={t}")))).collect();
        // the lease's own environment wins over what was asked
        env.retain(|(k, _)| !law.iter().any(|(l, _)| l == k));
        env.extend(law.iter().map(|(k, v)| (k.to_string(), v.clone())));
        let cost = lease_cost(&v, kind);
        let live = live_of(&v);
        for (key, value) in job_env(kind, cost, live, lease_threads(&v), lease_pool_cores(&v)) {
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
            id: id.clone(), lane: lane.clone(), warms, kind, src, loader, cost, ram_max_gb: lease_ram_max(&v), weighed_gb: lease_ram_weight(&v).unwrap_or(cost.ram_gb), peak_gb: lease_ram_peak(&v), jit_jobs: lease_jit_jobs(&v), threads: lease_threads(&v), pool_cores: lease_pool_cores(&v), memory_end: None, live, priority: priority_of(&v), idle: false, rest: false, first_run: String::new(), rustc, toolchain_own: false, shipped: false, phases: Default::default(), work: work.clone(), target, tmp, env, pin, overlay: Vec::new(), source: String::new(), fresh_put: Vec::new(), fresh_gone: Vec::new(), inputs: Vec::new(), runs: Vec::new(), peak_ram_bytes: None, store_cpu_s: 0.0, held_bytes: None, held_now_bytes: None, beat: Instant::now(), quiet: None, slow: None, meter: None, panicked: None, groups: Vec::new()
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
    fn take_tree(&self, id: &str, tree_lane: &str, kind: Kind, pinned: Option<super::Pin>) -> Result<(PathBuf, PathBuf, Option<String>, String), String> {
        let shared = self.opts.tree.as_ref().ok_or("this node runs no tree for rev leases")?;
        let conf = self.conf();
        let native = super::native_lane(tree_lane, kind, conf.role);
        let root = if native { super::tree::native_root(&conf.work, tree_lane) } else { shared.clone() };
        let deploy = crate::sync::SyncConf::load().checkout;
        let mut pin = match pinned {
            Some(pin) => pin,
            None => super::tree::shared(&deploy).or_else(|| super::tree::current(&root)).ok_or("sync this node before its first CI lease")?,
        };
        // A shared tree takes each nested repository the pin does not name at the deployment's
        // commit (one the store holds), and goes without one the deployment has not: never the
        // commit an earlier lease left there, which made a CI run build whatever a lane had pinned
        // last. A native lane has exactly its requested pin.
        if !native {
            for (name, sha) in super::tree::shared(&deploy).map(|p| p.nested).unwrap_or_default() {
                if !pin.nested.contains_key(&name) && crate::sync::git::Store::open(&name).is_ok_and(|s| s.has(&sha)) {
                    pin.nested.insert(name, sha);
                }
            }
        }
        let t = if native {
            super::tree::take_native(id, &pin, &conf.work, tree_lane, conf.native_trees as usize, self.guard.floor(), super::sched::tests_shard(tree_lane, kind))?
        } else {
            super::tree::take(id, &pin, &root, &conf.work.join("recovered"))?
        };
        self.report_tree(&format!("{id} on {}, before its start", self.machine), &t.cleared);
        let pin = t.pin;
        let moved: Vec<String> = t.moved.iter().map(|(r, n)| format!("{r} {n} files")).collect();
        let what = format!("{} tree at {} ({})", if native { "native" } else { "work" }, pin.short(), if t.moved.iter().all(|(_, n)| *n == 0) { "there already".to_string() } else { format!("moved: {}", moved.join(", ")) });
        let target = if native { super::tree::native_target(&conf.work, tree_lane) } else { conf.work.join("ci-target") };
        Ok((t.root, target, Some(pin.key()), what))
    }

    #[cfg(not(feature = "checkout"))]
    fn take_tree(&self, _id: &str, _tree_lane: &str, _kind: Kind, _pinned: Option<super::Pin>) -> Result<(PathBuf, PathBuf, Option<String>, String), String> {
        Err("this host has no git stores: leases with a rev need a desktop node".into())
    }

    /// `lease-weight <id> call|idle|rest`: a live session's runs weigh as interactive work in a
    /// call, as background work when idle, or at the idle CPU class when it rests
    /// ([`Lease::cpu_weight`]); its live runs' scopes take the weight now, its next runs at their
    /// start. `{"weight":N,"scopes":n}`.
    fn weigh(&self, rest: &str) -> String {
        let (id, idle, rest) = match rest.split_once(' ') {
            Some((id, "call")) => (id, false, false),
            Some((id, "idle")) => (id, true, false),
            Some((id, "rest")) => (id, true, true),
            _ => return err(format!("lease-weight {rest}: wants `<id> call|idle|rest`")),
        };
        let (weight, units, turned, ended, runs) = {
            let mut leases = self.leases.lock().unwrap();
            let Some(l) = leases.get_mut(id) else { return err(format!("lease-weight: no lease {id}")) };
            if !l.live { return err(format!("lease-weight: {id} is no live session")); }
            let turned = l.idle != idle;
            if turned || l.rest != rest {
                l.idle = idle;
                l.rest = rest;
                self.save(l);
            }
            let units: Vec<(String, Arc<Run>)> = l.runs.iter().filter(|r| r.pid != 0 && r.ended() == 0).filter_map(|r| Some((r.unit.clone()?, r.clone()))).collect();
            let ended = if turned && idle { l.meter.take() } else { None };
            (l.cpu_weight(), units, turned, ended, l.runs.clone())
        };
        // the call's meter: its first look as it starts, its sample as it ends (off the lock: no
        // process walks under it)
        let at = turned.then(|| activity_now(&runs)).flatten();
        let call = match (turned, idle, ended) {
            (true, false, _) => {
                if let Some(at) = at {
                    if let Some(l) = self.leases.lock().unwrap().get_mut(id).filter(|l| !l.idle && l.meter.is_none()) { l.meter = Some(CallMeter::new(false, Instant::now(), at)); }
                }
                None
            }
            (_, _, Some(m)) => Some(m.end(Instant::now(), at)),
            _ => None,
        };
        for (unit, run) in &units {
            // (a run that ended meanwhile took its scope with it)
            if let Err(e) = os::set_cpu_weight(unit, weight).or_else(|e| if run.ended() > 0 { Ok(()) } else { Err(e) }) { return err(format!("lease-weight {id}: {e}")); }
        }
        format!("{{\"weight\":{weight},\"scopes\":{}{}}}", units.len(), call.map_or(String::new(), |c| format!(",\"call\":{c}")))
    }

    /// Each in-call live session's look ([`call_loop`]): its runs read off the lease table's lock.
    fn meter_calls(&self) {
        let calls: Vec<(String, Vec<Arc<Run>>)> = self.leases.lock().unwrap().values().filter(|l| l.meter.is_some()).map(|l| (l.id.clone(), l.runs.clone())).collect();
        let looks: Vec<(String, Instant, Option<os::Activity>)> = calls.into_iter().map(|(id, runs)| { let at = activity_now(&runs); (id, Instant::now(), at) }).collect();
        let mut leases = self.leases.lock().unwrap();
        for (id, now, at) in looks {
            if let (Some(m), Some(at)) = (leases.get_mut(&id).and_then(|l| l.meter.as_mut()), at) { m.look(now, at); }
        }
    }

    fn run(&self, rest: &str) -> String {
        // `lease-run <id> <cwd> <base64 argv> [{"live":true}]`
        let mut w = rest.splitn(4, ' ');
        let (id, cwd, b64) = (w.next().unwrap_or(""), w.next().unwrap_or(""), w.next().unwrap_or("").trim());
        let extra = w.next().unwrap_or("").trim();
        let options = if extra.is_empty() { None } else {
            let Ok(v) = makepad_strict_json::parse(extra.as_bytes()) else { return err("lease-run: options are not JSON") };
            if let Some(k) = unknown_key(&v, &["live", "request_id", "offline"]) {
                return err(format!("lease-run {id}: this hub does not read the option `{k}`"));
            }
            Some(v)
        };
        let detachable = options.as_ref().and_then(|v| v.get("live")).and_then(|v| v.as_bool()).unwrap_or(false);
        let offline = match options.as_ref().and_then(|v| v.get("offline")) {
            None => false,
            Some(v) => match v.as_bool() { Some(b) => b, None => return err("lease-run: offline must be a bool") },
        };
        if offline && !cfg!(target_os = "linux") { return err(format!("offline runs need Linux network namespaces; this box is {}", std::env::consts::OS)); }
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
        l.toolchain_own |= super::argv::changes_toolchain(&argv).is_some();
        // an overlay's JIT run resolves its JIT first; any other command (a look, `run --look`:
        // `ps`, a log's tail) runs as given in the overlay's run folder, as a tree lease's would
        if l.src == Src::Overlay && super::loaders::cargo_run_args(&argv).is_some() {
            #[cfg(not(feature = "checkout"))]
            { return err("overlay: this node has no bare git store"); }
            #[cfg(feature = "checkout")]
            {
                if !l.shipped { return err("overlay: ship-done must define the source set before a run"); }
                let Some(args) = super::loaders::cargo_run_args(&argv).map(|a| a.to_vec()) else { return err("overlay: run requires the JIT cargo invocation") };
                if run_env.iter().map(|(k, _)| k).chain(&unset).any(|k| ["VFS", "WORKSPACE", "EDITS", "HOME"].iter().any(|v| crate::run_names::is(k, v)) || k == "CARGO_TARGET_DIR") { return err("overlay: source environment belongs to the lease"); }
                let (work, lane, lease, pin, requested, identity) = (self.conf().work, overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or("")), l.id.clone(), l.pin.clone(), l.loader.clone(), l.identity.clone());
                let Some(edits) = l.edits() else { return err("overlay: the lease has no edits folder") };
                // A store build is the box's, whichever lease asked first: every core (a test lease's
                // one core built a dev JIT at -j 1 for 8m47s while it and its box waited, 11-03). A
                // warm leg reserves little and builds at nice 19: idle CPU only (Unix).
                let niced = cfg!(unix) && l.lane.starts_with(super::WARM_LANE_PREFIX);
                // (but one its lease was admitted on fewer cores for takes those: `jit_jobs`)
                let cores = l.jit_jobs.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() as u32));
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
                        let run = |id: &str, cmd: &std::process::Command, active: &dyn Fn() -> Result<(), String>| this.overlay_build(&lease, id, cmd, active);
                        let warn = |line: &str| this.emit("lease", Sev::Warn, line.to_string());
                        let build = super::loaders::OverlayBuild { jobs: cores, niced, run: &run, cancelled: &identity.cancelled, say: &say, pending: &pending, warn: &warn, active: &active, sandboxed: offline };
                        let selected = super::loaders::find_overlay(&work, &lane, &edits, &lease, &pin, requested.as_deref(), &build)?;
                        say(&format!("devhub: the JIT is {} {}", if selected.changed.is_empty() { "this box's dev binary" } else { "this lane's own build" }, selected.id));
                        let mut argv = vec![selected.binary.display().to_string()];
                        argv.extend(args);
                        Ok(RunCommand { argv, detachable, offline, loader: Some(selected.binary), cache: Some(cache), notices: notices.into_inner() })
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
                    let result = resolved.and_then(|cmd| this.start_run(l, Path::new(&cwd), cmd, Found::default(), &run_env, &unset, &request_id));
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
                if crate::sync::serve::building_jit(&pin) {
                    let kept = self.loader_targets(&l.target).iter().any(|t| super::loaders::find(&l.work, t).is_some());
                    if !kept { return "{\"pending\":\"this box's sync builds the JIT of these commits: the run waits for it, not a second build\"}".into(); }
                }
            }
        }
        // a run never starts without its shipped files: it fails here and names them
        if let Some(why) = l.unshipped() { return err(format!("lease-run {id}: {why} on {}; the run did not start", self.machine)); }
        l.fresh_put.clear();
        l.fresh_gone.clear();
        let cmd = RunCommand { argv, detachable, offline, loader: None, cache: None, notices: Vec::new() };
        let found = match self.run_loader(l, &cmd.argv) {
            Ok(found) => found,
            Err(e) => return err(e),
        };
        // the JIT step below builds the tree's JIT outside the namespace, with the tree's build
        // scripts: an offline tree run takes only a JIT this box already holds for its tree
        // (an overlay's is the pin's: find_overlay refuses the lane's own, 11-15)
        #[cfg(feature = "checkout")]
        if offline && found.loader.is_none() && super::loaders::cargo_run_args(&cmd.argv).is_some() {
            return err(format!("lease-run {id}: an offline run builds no JIT: {} holds none of this tree's yet, and its build would run outside the sandbox", self.machine));
        }
        // no binary of its tree on the box: cargo's build of its JIT is a step of its own first,
        // for a script that calls the bare `makepad` its run gets too ([`job_makepad`]): else that
        // `makepad` is cargo's quiet build of the JIT on the run's cores, at every call (a gate
        // present's repro sat silent for 300 s at it, its JIT not built yet, 11-22). An offline
        // script keeps that build in its sandbox.
        #[cfg(feature = "checkout")]
        let script_jit = l.src != Src::Overlay && !offline && super::native::calls_makepad(&cmd.argv);
        #[cfg(feature = "checkout")]
        if found.loader.is_none() && (super::loaders::cargo_run_args(&cmd.argv).is_some() || script_jit) {
            let resolution = OverlayResolution { request: request_id, cwd: cwd.to_string(), argv: request, pending: String::new(), answer: None, reported: false, run: None };
            return self.jit_step(l, rel, cmd, found, (run_env, unset), resolution);
        }
        match self.start_run(l, rel, cmd, found, &run_env, &unset, &request_id) {
            Ok((pid, n)) => format!("{{\"run\":{pid},\"n\":{n}}}"),
            Err(e) => err(e),
        }
    }

    /// Where a work-tree lease's JIT binary comes from, in the order a run looks: the
    /// deployment's build first (one JIT per box per loader id), the CI tree's, then the lease's
    /// own `target`.
    fn loader_targets(&self, target: &Path) -> Vec<PathBuf> {
        let mut v = Vec::with_capacity(3);
        #[cfg(feature = "checkout")]
        v.push(crate::sync::SyncConf::load().checkout.join("target"));
        v.push(self.conf().work.join("ci-target"));
        v.push(target.to_path_buf());
        v
    }

    /// Where a run's JIT comes from: [`Found`].
    fn run_loader(&self, l: &Lease, argv: &[String]) -> Result<Found, String> {
        let id = l.id.as_str();
        let conf = self.conf();
        // a lease in the work tree: the JIT binary of exactly what the tree holds, when this box
        // built it before (the deployment's build first: one JIT per box per loader id), instead
        // of cargo building it again (lease/loaders.rs)
        let tree = (l.src == Src::Tree && l.pin.is_some()).then(|| (l.work.clone(), l.target.clone()));
        // an explicit --jit is the run's JIT whatever the tree holds: the box's store binary, held
        // until the lease ends (placement gives such a lease only a box that holds it), its
        // MAKEPAD_LOADER for any command and what a JIT cargo run runs
        let asked = match l.loader.as_deref().filter(|id| l.src == Src::Tree && !id.is_empty()) {
            Some(k) => Some((k.to_string(), super::jitstore::Store::new(&conf.work).held(k, id).map_err(|e| format!("lease-run {id}: --jit {k}: {e} on {}; the run did not start", self.machine))?)),
            None => None,
        };
        let loader = tree.as_ref().filter(|_| asked.is_none()).and_then(|(r, t)| self.loader_targets(t).into_iter().find_map(|d| Some((super::loaders::find(r, &d)?, d))));
        // held until the lease ends: its later runs start the same path, and a newer build's
        // publish prunes older ones
        if let Some(((k, _), d)) = &loader {
            super::loaders::hold(d, k, id).map_err(|e| format!("lease-run {id}: {e}"))?;
        }
        let loader = asked.or(loader.map(|(found, _)| found));
        #[cfg(feature = "checkout")]
        let (mut own, mut note) = (None, None);
        #[cfg(feature = "checkout")]
        let loader = if loader.is_none() && l.work == super::tree::native_root(&conf.work, l.tree_lane()) && (super::loaders::cargo_run_args(argv).is_some() || super::native::calls_makepad(argv)) {
            let asked = super::loaders::file_clock_now(&conf.work.join("dev/jit")).map_err(|e| format!("lease-run {id}: {e}"))?;
            if let Some(tree_key) = super::loaders::tree_id(&l.work) {
                note = converted_note(&tree_key);
                let key = tree_key.key;
                let store = super::jitstore::Store::new(&conf.work);
                store.hold(&key, &l.id)?;
                let hit = store.lookup(&key).map(|p| (key.clone(), p));
                if hit.is_none() { own = Some(super::loaders::TreeBuild { store, key, asked }); }
                hit
            } else { None }
        } else if loader.is_none() && tree.is_some() {
            // any other command of a tree lease (a gate's script, a shell): the box's dev JIT of the
            // tree's inputs, which a box's prebuild of the landed tip builds into the store (the
            // overlay id of a commit is its tree's key, loaders::tree_id), held until the lease ends.
            // A tests shard built its own (86 crates, 87 s) 12 s after the box's prebuild had built
            // that JIT, and ran a binary of its own beside the lanes' (bucket 1794189843830, 11-09)
            // (a CI tree checked out before the repo's eol=lf holds CRLF bytes: its key is not the
            // blobs', and the box's JIT of them never runs there, 11-10)
            match super::loaders::tree_id(&l.work) {
                Some(tree_key) => {
                    note = converted_note(&tree_key);
                    store_jit(&conf.work, &tree_key.key, &l.id)?
                }
                None => None,
            }
        } else { loader };
        // (a node without the checkout keeps no dev JIT store: no command reads the store's key)
        #[cfg(not(feature = "checkout"))]
        let (own, note, _) = (None, None, argv);
        Ok(Found { tree, loader, own, note })
    }

    fn start_run(&self, l: &mut Lease, rel: &Path, command: RunCommand, found: Found, run_env: &[(String, String)], unset: &[String], request: &str) -> Result<(u32, u32), String> {
        let RunCommand { argv, detachable, offline, loader: overlay_loader, cache, notices } = command;
        let Found { tree, loader, own, note } = found;
        let id = l.id.clone();
        let n = l.next_run();
        let log = l.tmp.join(format!("run-{n}.log"));
        let unit = self.scopes.then(|| format!("devhub-lease-{}-{id}-{n}", self.units));
        let conf = self.conf();
        let cpus = if l.kind == Kind::Timed { &conf.cpus_timed } else { &conf.cpus_shared };
        let mut out = match std::fs::File::create(&log) {
            Ok(f) => f,
            Err(e) => return Err(format!("{}: {e}", log.display())),
        };
        for line in note.into_iter().chain(notices) {
            use std::io::Write as _;
            let _ = writeln!(out, "{line}");
        }
        // a lease nobody attends runs its JIT runs unattended: a panic or a compile error ends them
        let unattended = super::nobody_attends(l.kind, l.priority, l.live);
        let argv = if unattended { super::loaders::unattended_run(argv) } else { argv };
        let mut argv = if l.src == Src::Overlay {
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
                if super::loaders::cargo_run_args(&argv).is_some() {
                    // cargo builds this tree's JIT with the exact flags of the one build: kept at the run's end
                    if let Some((_, target)) = &tree {
                        let _ = std::fs::write(target.join(super::loaders::MARK), "");
                    }
                    // that build is the lease's job, not its command's warm run
                    if n == 1 && !l.first_run.ends_with(super::JIT_BUILD_RUN) { l.first_run.push_str(super::JIT_BUILD_RUN); }
                }
                argv
            }
        } };
        // a Windows run is plain argv: never cmd, and a PowerShell script runs from its file
        let mut plain_env = Vec::new();
        if cfg!(windows) {
            if let Some(plain) = super::argv::windows_plain(&argv, &l.tmp.join(format!("run-{n}-script"))).map_err(|e| format!("lease-run {id}: {e}"))? {
                std::fs::write(&plain.script, plain.text.as_bytes()).map_err(|e| format!("lease-run {id}: {}: {e}", plain.script.display()))?;
                argv = plain.argv;
                plain_env = plain.env.into_iter().filter(|(k, _)| !run_env.iter().any(|(own, _)| own.eq_ignore_ascii_case(k))).collect();
            }
        }
        let jit = overlay_loader.as_deref().or_else(|| loader.as_ref().map(|(_, p)| p.as_path()));
        // a JIT run of a program starts from a link of the JIT named as the program, as `makepad
        // test` starts its targets (relax-compiler selfexe.rs launch_link): the exec names the
        // process after the program (macOS has no other way), with no exec more. RELAX_LOADER
        // names the JIT, which the run goes by (the runner's loader_exe)
        let launch = jit.filter(|j| argv.first().is_some_and(|a| Path::new(a) == *j))
            .and_then(|j| launch_link(&l.tmp, j, &super::run_program(&argv[1..])?));
        if let Some(link) = &launch {
            argv[0] = link.to_string_lossy().into_owned();
        }
        let err_out = match out.try_clone() {
            Ok(f) => f,
            Err(e) => return Err(e.to_string()),
        };
        let dir = run_dir(l, rel)?;
        let bin = l.tmp.join(format!("run-{n}-bin"));
        job_makepad(&bin, jit, &l.work, unattended).map_err(|e| format!("lease-run {id}: its makepad: {e}"))?;
        let build = |argv: Vec<String>| {
            let argv = match jit.filter(|_| l.kind == Kind::Timed && cfg!(target_os = "macos")) {
                Some(jit) => warmed_up(argv, jit, &exit_file(&self.state, &id, n).with_extension("warmup")),
                None => argv,
            };
            let argv = os::metered_argv(argv, unit.as_deref(), &exit_file(&self.state, &id, n).with_extension("usage"));
            let full = os::lease_argv(argv, unit.as_deref(), l.kind.nice(), cpus, l.kind.shared().then(|| l.cpu_weight()), l.ram_max_gb);
            let mut cmd = std::process::Command::new(&full[0]);
            cmd.args(&full[1..]).current_dir(&dir);
            self.run_env(&mut cmd, l, run_env, unset, jit);
            if let (Some(_), Some(jit)) = (&launch, jit) {
                cmd.env("RELAX_LOADER", jit);
            }
            // the run's own makepad before any on the node's PATH ([`job_makepad`])
            let path = cmd.get_envs().find(|(k, _)| k.eq_ignore_ascii_case("PATH")).and_then(|(_, v)| v.map(|v| v.to_os_string())).unwrap_or_default();
            if let Ok(path) = std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&path))) { cmd.env("PATH", path); }
            cmd.envs(plain_env.iter().map(|(k, v)| (k, v)));
            // Enter before even the scope's client execs; every wrapper inherits it (11-15).
            #[cfg(target_os = "linux")]
            if offline {
                use std::os::unix::process::CommandExt;
                let prepared = super::offline::Prepared::new();
                unsafe { cmd.pre_exec(move || super::offline::enter(&prepared)); }
            }
            cmd
        };
        let native = NativeBuild::start(l.src, &l.target, &argv);
        let mut cmd = build(argv.clone());
        if os::exec_over(&cmd).is_some() {
            if let Some((i, path, word)) = script_spill(&argv, &l.tmp.join(format!("run-{n}-script"))) {
                std::fs::write(&path, argv[i].as_bytes()).map_err(|e| format!("lease-run {id}: {}: {e}", path.display()))?;
                argv[i] = word;
                cmd = build(argv);
            }
            if let Some(over) = os::exec_over(&cmd) {
                return Err(format!("lease-run {id}: {}", os::exec_refusal(&over)));
            }
        }
        cmd.stdin(std::process::Stdio::null()).stdout(out).stderr(err_out);
        // before the spawn: the child may write before this thread runs again, and what it wrote
        // is matched to the run by time ([`Exec::virtual_root_written`])
        let started_ms = crate::proto::wall_ms();
        let (mut child, hold) = match os::lease_spawn(&mut cmd, Some(&exit_file(&self.state, &id, n))) {
            Ok(c) => c,
            Err(e) if offline => return Err(format!("lease-run {id}: this box cannot enter a user and network namespace ({e}); an offline run never runs networked")),
            Err(e) => return Err(format!("lease-run {id}: {}: {e}", cmd.get_program().to_string_lossy())),
        };
        let pid = child.id();
        let run = Arc::new(Run { request: request.to_string(), started_ms, spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit, log, exit: Mutex::new(None), ours: true, detachable, hold, exit_file: exit_file(&self.state, &id, n), cache: Mutex::new(cache), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native, build: OnceLock::new() });
        l.runs.push(run.clone());
        // a live session's launch is its first call, from its run's spawn (nothing used yet)
        if l.live && !l.idle && l.meter.is_none() && l.runs.iter().filter(|r| r.pid != 0 && r.ended() == 0).count() == 1 {
            l.meter = Some(CallMeter::new(true, Instant::now(), os::Activity::default()));
        }
        // it runs whatever the ledger says, so its holder hears that it runs (a refusal here left
        // it running unseen: 0 GB free, 11-04); the lease's next save writes it down
        self.save(l);
        let _ = std::thread::Builder::new().name("devhub-lease-wait".into()).spawn(move || {
            let code = match os::wait_usage(&mut child) {
                Ok((status, cpu, maxrss)) => { run.reaped(cpu, maxrss); os::exit_code(status) }
                Err(_) => -1,
            };
            let ended = crate::proto::wall_ms();
            run.build();
            run.ended_ms.store(ended, Ordering::Release);
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
            // a live run's JIT ended: what it left in its scope goes with it, its exit known first
            if run.detachable { stop_left(&run); }
        });
        Ok((pid, n))
    }

    /// A JIT run in a work tree whose box keeps no binary of it ([`Found`] has none): cargo's
    /// build of that JIT runs first as a step of the lease's own, on its worker (its phase `jit`),
    /// and its command then runs on the binary it built, kept by content first as a run's build is
    /// ([`super::loaders::publish`]). Its holder asks again until the command starts (`pending`).
    /// The stall check judges the build by what it does (its process's activity and output) and
    /// holds the command, once it runs, to its own run (`Sched::say_stalled`): a small test lease's
    /// runner rebuild, 12 min of cargo,
    /// was its test's run (L693116, 11-08). The lease's jobs are its command's: its build takes all
    /// of cargo's, unless its caller set `CARGO_BUILD_JOBS`, and the lease is charged its box's
    /// cores meanwhile (`Sched::charged`): at its one core's `-j 1`, L693116 built for 18 min. The
    /// build takes no command's number ([`BUILD_RUNS`]), so
    /// a master restarted under it finds no run, asks again under the same name and joins it. A
    /// build that fails is the command's run: cargo's own output and exit, as the run's cargo said
    /// them.
    #[cfg(feature = "checkout")]
    fn jit_step(&self, l: &mut Lease, rel: &Path, cmd: RunCommand, found: Found, env: (Vec<(String, String)>, Vec<String>), mut resolution: OverlayResolution) -> String {
        let id = l.id.clone();
        let Some(this) = self.this.upgrade() else { return err("lease-run: hub stopped") };
        // (a script that calls the bare `makepad`: the build of what that runs)
        let build = super::loaders::cargo_build_argv(&cmd.argv).or_else(|| super::native::calls_makepad(&cmd.argv).then(|| super::loaders::cargo_build_argv(&["makepad".to_string()])).flatten());
        let Some(build) = build else { return err(format!("lease-run {id}: not the JIT's cargo run")) };
        let dir = match run_dir(l, rel) {
            Ok(dir) => dir,
            Err(e) => return err(e),
        };
        let (run_env, unset) = env;
        // where its cargo builds: the run's own CARGO_TARGET_DIR, else its lease's; kept from
        // the lease's target only (what a later run there finds)
        let target = run_env.iter().rev().find(|(k, _)| k == "CARGO_TARGET_DIR").map_or_else(|| l.target.clone(), |(_, v)| PathBuf::from(v));
        let keeps = found.tree.clone().filter(|(_, t)| *t == target);
        if keeps.is_some() {
            let mark = target.join(super::loaders::MARK);
            if let Err(e) = mark.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| std::fs::write(&mark, "")) { return err(format!("lease-run {id}: {}: {e}", mark.display())); }
        }
        let n = l.next_run();
        // that build is the lease's job, not its command's warm run
        if n == 1 && !l.first_run.ends_with(super::JIT_BUILD_RUN) { l.first_run.push_str(super::JIT_BUILD_RUN); }
        l.phases.begin("jit", crate::proto::wall_ms());
        resolution.pending = "cargo builds this tree's JIT first; the command runs on it once built".into();
        let request_id = resolution.request.clone();
        l.resolution = Some(resolution);
        self.save(l);
        let (lease, identity, tmp, rel, jit_jobs) = (id.clone(), l.identity.clone(), l.tmp.clone(), rel.to_path_buf(), l.jit_jobs);
        let worker = std::thread::Builder::new().name("devhub-jit-step".into()).spawn(move || {
            let mut cargo = std::process::Command::new(&build[0]);
            cargo.args(&build[1..]).current_dir(&dir).env("CARGO_TARGET_DIR", &target);
            // admitted on fewer cores than such a build measures, it takes those (`jit_jobs`)
            match jit_jobs {
                Some(jobs) => { cargo.env("CARGO_BUILD_JOBS", jobs.to_string()); }
                None if !run_env.iter().any(|(k, _)| k == "CARGO_BUILD_JOBS") => { cargo.env_remove("CARGO_BUILD_JOBS"); }
                None => {}
            }
            let t0 = Instant::now();
            let built = this.worker_process(&lease, &identity, &tmp, &cargo, &run_env, &unset, true).and_then(|mut w| Ok((w.wait()?, w.log().to_path_buf())));
            let took = t0.elapsed();
            let mut leases = this.leases.lock().unwrap();
            let Some(l) = leases.get_mut(&lease).filter(|l| Arc::ptr_eq(&l.identity, &identity)) else { return };
            l.phases.close("jit", crate::proto::wall_ms());
            let started = match built {
                Err(e) => Err(format!("lease-run {lease}: its JIT's build: {e}")),
                Ok((status, log)) if !status.success() => Ok(this.failed_build_run(l, &log, os::exit_code(status), &request_id)),
                Ok((_, log)) => {
                    let mut notices: Vec<String> = found.note.iter().cloned().chain(std::fs::read_to_string(&log).unwrap_or_default().lines().map(str::to_string)).collect();
                    notices.push(format!("devhub: cargo built this tree's JIT in {:.0} s", took.as_secs_f64()));
                    built_loader(l, keeps.as_ref(), found.own.as_ref(), &target, &mut notices).and_then(|loader| {
                        let found = Found { tree: found.tree, loader: Some(loader), own: None, note: None };
                        this.start_run(l, &rel, RunCommand { notices, ..cmd }, found, &run_env, &unset, &request_id)
                    })
                }
            };
            if let Some(r) = l.resolution.as_mut() {
                r.answer = Some(match started {
                    Ok((pid, n)) => { r.run = Some(n); format!("{{\"run\":{pid},\"n\":{n}}}") }
                    Err(e) => err(e),
                });
            }
            this.save(l);
        });
        if let Err(e) = worker {
            l.phases.close("jit", crate::proto::wall_ms());
            if let Some(r) = l.resolution.as_mut() { r.answer = Some(err(format!("lease-run {id}: its JIT's build: worker thread: {e}"))); }
            self.save(l);
        }
        l.resolution.as_ref().map_or_else(|| err("lease-run: no resolution"), |r| r.answer())
    }

    /// A lease's JIT build that failed is the run of the command it was for: cargo's own output
    /// and exit, at that command's number, asked by its request.
    #[cfg(feature = "checkout")]
    fn failed_build_run(&self, l: &mut Lease, log: &Path, code: i32, request: &str) -> (u32, u32) {
        let n = l.next_run();
        let now = crate::proto::wall_ms();
        let run = Arc::new(Run { request: request.to_string(), started_ms: now, spawned_ms: now, ended_ms: AtomicU64::new(0), n, pid: 0, start: 0, unit: None, log: log.to_path_buf(), exit: Mutex::new(None), ours: true, detachable: false, hold: os::Hold::none(), exit_file: exit_file(&self.state, &l.id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
        run.finished(code);
        l.runs.push(run);
        (0, n)
    }

    /// Lease-owned processes inherit only their lease's environment.
    fn run_env(&self, cmd: &mut std::process::Command, l: &Lease, env: &[(String, String)], unset: &[String], loader: Option<&Path>) {
        // A clean base, then the lease's own environment (including its selected WM for gui=1).
        cmd.env_clear();
        for (k, v) in base_env(l.kind, std::env::vars()) { cmd.env(k, v); }
        // Also covers leases taken over from a ledger written before these defaults existed.
        for (k, v) in job_env(l.kind, l.cost, l.live, l.threads, l.pool_cores) { cmd.env(k, v.to_string()); }
        for (k, v) in &l.env { cmd.env(k, v); }
        for (k, v) in env { cmd.env(k, v); }
        for k in unset { cmd.env_remove(k); }
        #[cfg(feature = "checkout")]
        if l.src == Src::Overlay || l.tmp.join("overlay-selftest.json").is_file() { cmd.env("MAKEPAD_DEVHUB_GIT", crate::sync::git::stores_dir()); }
        // what it starts carries this, in whatever session: the lease's end finds it by this
        cmd.env(LEASE_ENV, lease_mark(&self.state, &l.id));
        // relax names the lease as the owner of what its runs hold (a build script's claim)
        cmd.env("RELAX_RUN_OWNER", &l.id);
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
    fn overlay_build(&self, lease: &str, id: &str, build: &std::process::Command, active: &dyn Fn() -> Result<(), String>) -> Result<(std::process::ExitStatus, PathBuf), String> {
        active()?;
        let conf = self.conf();
        let tmp = conf.work.join("dev/jit/build").join(id);
        std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
        let pid_file = tmp.join("cargo.pid");
        join_recorded_build(&pid_file, lease, active, &|pid| self.emit("lease", Sev::Info, format!("dev JIT {id}: joining the build pid {pid} an earlier hub started")))?;
        let log = tmp.join("cargo.log");
        let unit = self.scopes.then(|| format!("devhub-jit-{}-{id}", self.units));
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
        let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n: 0, pid, start: os::proc_start(pid).unwrap_or(0), unit, log: log.clone(), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: tmp.join("cargo.exit"), cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
        let mut process = WorkerRun { child, run, pid_file: Some(pid_file.clone()) };
        std::fs::write(&pid_file, format!("{pid} {} {lease}\n", process.run.start)).map_err(|e| format!("{}: {e}", pid_file.display()))?;
        self.store_builds.lock().unwrap().push((lease.to_string(), process.run.clone()));
        let built = loop {
            match os::try_wait_usage(&mut process.child).map_err(|e| e.to_string()) {
                Ok(Some((status, cpu, maxrss))) => {
                    process.run.reaped(cpu, maxrss);
                    process.run.finished(os::exit_code(status));
                    break Ok((status, log));
                }
                Ok(None) => {}
                Err(e) => break Err(e),
            }
            if let Err(e) = active() {
                self.emit("lease", Sev::Info, format!("dev JIT {id}: {e}; stopping store build pid {pid}"));
                break Err(e); // WorkerRun stops and reaps only this store-owned process group.
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        self.store_builds.lock().unwrap().retain(|(_, r)| !Arc::ptr_eq(r, &process.run));
        // its CPU is the lease's: a box prebuild's class measured 0.04 CPU seconds in 69 s of a
        // 7-core build and was charged one core at its grant (11-11)
        if let Some(cpu) = process.run.usage.lock().unwrap().cpu {
            if let Some(l) = self.leases.lock().unwrap().get_mut(lease) { l.store_cpu_s += cpu; }
        }
        built
    }

    /// Stops the store builds an earlier hub started ([`recorded_build`]) that no lease of this hub
    /// owns: its scope and process group go, as its owner's end stops a build of its own. A hub
    /// restart leaves its builds running for their leases to join; one whose lease did not outlive
    /// it built on as no lease's, read "left by an ended JIT build", a pool box at load 28 of its
    /// 16 cores and a tests shard there at 20 times its run (11-16). At a lease's end and the hub's
    /// start, when leases are taken back.
    #[cfg(feature = "checkout")]
    fn stop_unowned_builds(&self) {
        let root = self.conf().work.join("dev/jit/build");
        let tracked: BTreeSet<u32> = self.store_builds.lock().unwrap().iter().map(|(_, r)| r.pid).collect();
        for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            let path = e.path().join("cargo.pid");
            let Some((pid, start, lease)) = std::fs::read_to_string(&path).ok().and_then(|t| recorded_build(&t)) else { continue };
            if tracked.contains(&pid) || !os::same(pid, start) || (!lease.is_empty() && self.leases.lock().unwrap().contains_key(&lease)) { continue; }
            let id = e.file_name().to_string_lossy().into_owned();
            let unit = self.scopes.then(|| format!("devhub-jit-{}-{id}", self.units));
            let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: 0, ended_ms: AtomicU64::new(0), n: 0, pid, start, unit, log: e.path().join("cargo.log"), exit: Mutex::new(None), ours: true, detachable: false, hold: os::Hold::none(), exit_file: e.path().join("cargo.exit"), cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
            let (_, left) = stop_runs(std::slice::from_ref(&run), &makepad_remote::discovery::registry_dir(), true);
            let _ = std::fs::remove_file(&path);
            let owner = if lease.is_empty() { "no lease named".to_string() } else { format!("{lease}, which ended") };
            let left = if left.is_empty() { String::new() } else { format!("; still alive: {left:?}") };
            self.emit("lease", Sev::Warn, format!("dev JIT {id}: stopped the store build pid {pid} an earlier hub started for {owner}{left}"));
        }
    }

    /// Lease ownership for the selftest's fixture runs (they continue an admitted run).
    #[cfg(feature = "checkout")]
    fn worker_process(&self, id: &str, identity: &Arc<LeaseIdentity>, tmp: &Path, build: &std::process::Command, env: &[(String, String)], unset: &[String], jit_build: bool) -> Result<WorkerRun, String> {
        let mut leases = self.leases.lock().unwrap();
        let l = leases.get_mut(id).filter(|l| Arc::ptr_eq(&l.identity, identity)).ok_or_else(|| format!("overlay: lease {id} ended before worker process"))?;
        if self.restarting.load(Ordering::Acquire) { return Err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine)); }
        // (a JIT build: past the commands' numbers, at its own level, not under the RAM its lease
        // asked for its command: all of cargo's jobs hold more than that)
        let (n, nice, ram_max) = if jit_build { (l.next_build(), l.build_nice(), None) } else { (l.next_run(), l.kind.nice(), l.ram_max_gb) };
        let log = tmp.join(format!("run-{n}.log"));
        let unit = self.scopes.then(|| format!("devhub-lease-{}-{id}-{n}", self.units));
        let conf = self.conf();
        let cpus = if l.kind == Kind::Timed { &conf.cpus_timed } else { &conf.cpus_shared };
        let argv = std::iter::once(build.get_program()).chain(build.get_args()).map(|s| s.to_string_lossy().into_owned()).collect();
        let argv = os::metered_argv(argv, unit.as_deref(), &exit_file(&self.state, &id, n).with_extension("usage"));
        let full = os::lease_argv(argv, unit.as_deref(), nice, cpus, l.kind.shared().then(|| l.cpu_weight()), ram_max);
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
        let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit, log: log.clone(), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: exit_file(&self.state, id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
        l.runs.push(run.clone());
        self.save(l);
        drop(leases);
        Ok(WorkerRun { child, run, pid_file: None })
    }

    /// [`launch_stopped`] here.
    fn launch_stopped(&self, leader: u32) {
        for r in self.leases.lock().unwrap().values().flat_map(|l| l.runs.iter()).filter(|r| r.pid == leader) {
            r.stopped.store(true, Ordering::Release);
        }
    }

    /// A sync launch's command (`cmd`, the WM's launch of an app in the deployment checkout:
    /// [`crate::sync`]'s `launch`) as the next run of lease `id`, the way a `lease-run` runs: its
    /// own session and scope, the lease kind's nice level and pinning, the run wrapper, its output
    /// in `log`. The wrapper writes its exit beside the ledger, so the lease's end says how it
    /// ended and a restarted hub takes it back with that exit. (A launch was a pid put under the
    /// lease: its exit was never known, and every launch that ended before its lease said "its
    /// hub restarted while it ran", a timed run's lane queued its whole series again for it,
    /// 11-05; and in the hub's own scope it kept the WM from starting the hub again under that
    /// scope's name until it ended.) `cmd`'s environment changes and directory are kept. The
    /// answer waits for the wrapper to start the command: its pid is the app's. `ended`: the
    /// app's pid (0: never started) and the run's exit, once it ends under this hub.
    /// The wrappers are the hub's, found on its PATH ([`Opts::path`]), never on the command's: a
    /// command's PATH chose its wrapper under rustc's std and went unread under the JIT's, which
    /// searched the hub's (11-05). A PATH the command sets is its own lookup's: `env` sets it
    /// right before the command starts.
    pub fn launch(&self, id: &str, cmd: &std::process::Command, log: &Path, ended: Box<dyn FnOnce(u32, i32) + Send>) -> Result<Launched, String> {
        let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
        let mut leases = self.leases.lock().unwrap();
        let l = leases.get_mut(id).ok_or_else(|| format!("no live lease {id} on {}", self.machine))?;
        if !l.kind.writes() {
            return Err(format!("{id} is an {} lease: launches need a write lease", l.kind.as_str()));
        }
        if self.restarting.load(Ordering::Acquire) { return Err(format!("restarting: {}'s hub restarts for its self-update; try again", self.machine)); }
        let n = l.next_run();
        let exit = exit_file(&self.state, id, n);
        let unit = self.scopes.then(|| format!("devhub-lease-{}-{id}-{n}", self.units));
        let conf = self.conf();
        let cpus = if l.kind == Kind::Timed { &conf.cpus_timed } else { &conf.cpus_shared };
        let mut argv: Vec<String> = std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(|s| s.to_string_lossy().into_owned()).collect();
        let hub_path = self.opts.path.clone().or_else(|| std::env::var_os("PATH")).unwrap_or_default();
        let own_path = cfg!(unix).then(|| cmd.get_envs().find(|(k, _)| *k == "PATH").map(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))).flatten();
        match &own_path {
            Some(Some(p)) => { argv.splice(0..0, ["env".to_string(), format!("PATH={p}")]); }
            Some(None) => { argv.splice(0..0, ["env", "-u", "PATH"].map(String::from)); }
            None => {}
        }
        let argv = os::metered_argv(argv, unit.as_deref(), &exit.with_extension("usage"));
        let full = os::lease_argv(argv, unit.as_deref(), l.kind.nice(), cpus, l.kind.shared().then(|| l.cpu_weight()), l.ram_max_gb);
        let wrapper = os::hub_program(&full[0], &hub_path).map_err(|e| format!("launch under {id}: {e}"))?;
        let mut run_cmd = std::process::Command::new(wrapper);
        run_cmd.args(&full[1..]).stdin(std::process::Stdio::null()).stdout(out.try_clone().map_err(|e| e.to_string())?).stderr(out);
        if let Some(dir) = cmd.get_current_dir() { run_cmd.current_dir(dir); }
        for (k, v) in cmd.get_envs().filter(|(k, _)| own_path.is_none() || *k != "PATH") {
            match v { Some(v) => { run_cmd.env(k, v); }, None => { run_cmd.env_remove(k); } }
        }
        // (the wrappers inside the first one find theirs on it too)
        if own_path.is_some() { run_cmd.env("PATH", &hub_path); }
        // what it starts carries the lease's mark, as a run's processes do ([`Exec::run_env`])
        run_cmd.env(LEASE_ENV, lease_mark(&self.state, id));
        run_cmd.env("RELAX_RUN_OWNER", id);
        if let Some(outer) = std::env::var_os(LEASE_OUTER_ENV).or_else(|| std::env::var_os(LEASE_ENV)) {
            run_cmd.env(LEASE_OUTER_ENV, outer);
        }
        // Start the waiter before the process: a thread spawn error leaves no child to reap,
        // and a process spawn error closes the channel so the waiter exits too.
        let (tx, rx) = std::sync::mpsc::sync_channel::<(std::process::Child, Arc<Run>)>(1);
        std::thread::Builder::new().name("devhub-launch-wait".into()).spawn(move || {
            let Ok((mut child, waited)) = rx.recv() else { return };
            let code = match os::wait_usage(&mut child) {
                Ok((status, cpu, maxrss)) => { waited.reaped(cpu, maxrss); os::exit_code(status) }
                Err(_) => -1,
            };
            waited.ended_ms.store(crate::proto::wall_ms(), Ordering::Release);
            waited.finished(code);
            ended(waited.app_pid().unwrap_or(0), code);
        }).map_err(|e| format!("launch under {id}: cannot start its waiter: {e}"))?;
        let (child, hold) = os::lease_spawn(&mut run_cmd, Some(&exit)).map_err(|e| format!("launch under {id}: {}: {e}", full[0]))?;
        let pid = child.id();
        let now = crate::proto::wall_ms();
        let run = Arc::new(Run { request: String::new(), started_ms: now, spawned_ms: now, ended_ms: AtomicU64::new(0), n, pid, start: os::proc_start(pid).unwrap_or(0), unit, log: log.to_path_buf(), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: exit, cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
        l.runs.push(run.clone());
        self.save(l);
        drop(leases);
        tx.send((child, run.clone())).expect("the launch waiter receives its child");
        // the wrapper writes the pid of what it started at once (Linux: after systemd-run made its scope)
        loop {
            if let Some(app) = run.app_pid() {
                return Ok(Launched { n, leader: pid, app });
            }
            if let Some(code) = *run.exit.lock().unwrap() {
                return Err(format!("launch under {id}: its run ended (exit {code}) before it started the app; its log: {}", log.display()));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
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
    fn hub_restart(&self, rest: &str) -> String {
        // `<deployment sha> [jobs=N]`: the build's jobs, the cores of its room (`hub-jobs`)
        let mut words = rest.split_whitespace();
        let at = words.next().unwrap_or("");
        let mut jobs = None;
        for word in words {
            match word.strip_prefix("jobs=").and_then(|n| n.parse::<u32>().ok()).filter(|n| *n > 0) {
                Some(n) => { if jobs.is_none() { jobs = Some(n); } }
                None => return err(format!("hub-restart: this hub does not read the word `{word}` (expected jobs=<positive integer>)")),
            }
        }
        #[cfg(not(feature = "checkout"))]
        let _ = jobs;
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
        match self.hub_built(&root, &now, timed, jobs) {
            Some(Ok(())) => {}
            Some(Err(why)) => return err(format!("the new hub does not build at {}: {why}; {} keeps running its hub at {}", &now[..now.len().min(10)], self.machine, &super::caps::hub_rev()[..super::caps::hub_rev().len().min(10)])),
            None if timed => return err(format!("waits: a timed lease runs on {}: its new hub at {} builds after it", self.machine, &now[..now.len().min(10)])),
            None => return err(format!("waits: {}'s new hub builds at {} first", self.machine, &now[..now.len().min(10)])),
        }
        // (its blockers were read before the build's look: an apply that started since is
        // seen here, and none starts after)
        #[cfg(feature = "checkout")]
        if !crate::sync::serve::restart_begin() {
            return err(format!("waits: a sync runs on {} (the hub restarts after it)", self.machine));
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

    /// What builds on this box now, or keeps it busy (the WM's own apps starting after its
    /// restart), if anything: a timed lease starts after it ([`Self::lease_start`]).
    #[cfg(feature = "checkout")]
    fn builds_here(&self) -> Option<String> {
        if crate::sync::serve::applying() {
            return Some(format!("the deployment of {} moves and builds", self.machine));
        }
        if matches!(*self.hub_build.lock().unwrap(), HubBuild::Running { .. }) {
            return Some(format!("{}'s new hub builds", self.machine));
        }
        if !self.opts.watch_wm {
            return None;
        }
        crate::sync::serve::wm_apps_busy().map(|busy| format!("{busy} on {}", self.machine))
    }

    #[cfg(not(feature = "checkout"))]
    fn builds_here(&self) -> Option<String> {
        None
    }

    /// A timed lease's start ([`Noise`]): its looks count from now, and with no other timed
    /// lease's looks running here its first span begins now, so a run of a few seconds is
    /// measured too.
    #[cfg(unix)]
    fn first_span(&self, id: &str, gpu: bool) {
        {
            let mut noise = self.noise.lock().unwrap();
            let n = noise.entry(id.to_string()).or_default();
            n.gpu = gpu;
            n.from.get_or_insert_with(Instant::now);
        }
        let mut span = self.span.lock().unwrap();
        if span.is_none() {
            let look = self.opts.box_look.clone().unwrap_or_else(BoxLook::this_box);
            *span = Some((look.ticks)().0);
        }
    }

    /// `lease-box <id>`: what shared a timed lease's box so far ([`Noise`]), null before its first
    /// look: its job's result says whether it was quiet before its release, which a series' next
    /// job's start waits for.
    fn lease_box(&self, id: &str) -> String {
        #[cfg(unix)]
        let b = self.noise.lock().unwrap().get(id.trim()).filter(|n| n.samples > 0).map_or("null".to_string(), |n| n.json());
        #[cfg(not(unix))]
        let b = { let _ = id; "null".to_string() };
        format!("{{\"box\":{b}}}")
    }

    /// What a timed start waits on besides this hub's own builds ([`Self::builds_here`]): the
    /// deployment's checkout moved under the box's apps under [`MOVE_SETTLE`] ago (they take the
    /// move, and one that runs makepad from it rebuilds it a moment later), a compiler runs
    /// outside every lease ([`crate::top::compilers_among`]), or what runs outside every lease takes [`super::master::NOISY_CPU_PCT`] or more (a desktop left
    /// running under a timed gate, 11-10), as its [`BoxLook`] sees the box: the start comes once
    /// the box is quiet again. A GPU timing (`gpu`) waits for its GPU in place of that CPU: busy
    /// [`super::master::NOISY_GPU_PCT`] or more, its users named among what runs outside every
    /// lease. Nothing is stopped. Past its wait for a quiet box (`unquiet`: its master starts it
    /// beside that work) only a move and a compiler outside every lease hold it: those end soon.
    fn timed_waits(&self, gpu: bool, unquiet: bool) -> Option<String> {
        let look = self.opts.box_look.clone().unwrap_or_else(BoxLook::this_box);
        if let Some(ago) = (look.moved_ago)().filter(|d| *d < MOVE_SETTLE) {
            return Some(format!("its deployment moved {:.1} s ago under the apps that run from it, which take the move", ago.as_secs_f64()));
        }
        let groups: std::collections::HashMap<u32, String> = self.leases.lock().unwrap().values()
            .flat_map(|l| l.runs.iter().map(|r| r.pid).filter(|p| *p != 0).map(|p| (p, l.id.clone())).collect::<Vec<_>>())
            .collect();
        let (rows, me) = (look.procs)();
        let busy = crate::top::compilers_among(&rows, me, &|pgid| groups.get(&pgid).cloned());
        if !busy.is_empty() { return Some(format!("{} outside every lease", busy.join(", "))); }
        if unquiet { return None; }
        if gpu {
            let rates = (look.gpu)()?;
            let (pct, on) = crate::top::gpu_split(&rates, &rows, me, &|pgid| groups.contains_key(&pgid).then(String::new));
            let on = if on.is_empty() { String::new() } else { format!(": what uses it, {}", crate::top::gpu_said(&on)) };
            return (pct >= super::master::NOISY_GPU_PCT).then(|| format!("its GPU is {pct:.0}% busy outside every lease{on}"));
        }
        let (outside, busiest) = crate::top::outside_among(&rows, me, &|pgid| groups.get(&pgid).cloned());
        (outside >= super::master::NOISY_CPU_PCT).then(|| format!("{outside:.0}% of a core runs outside every lease ({})", busiest.join(", ")))
    }

    /// What a restart of this hub would lose now ([`Exec::hub_restart`]), if anything.
    fn restart_blocker(&self, leases: &BTreeMap<String, Lease>) -> Option<String> {
        // a flash's session runs in this hub's child: cut, the board's image is half written
        if let Some(flash) = crate::serial::node_flashing() {
            return Some(format!("{flash} on {}", self.machine));
        }
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
    fn hub_built(&self, root: &Path, at: &str, timed: bool, jobs: Option<u32>) -> Option<Result<(), String>> {
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
        // no apply moves the checkout under the build, nor does it start under one
        // ([`crate::sync::serve::hub_build_begin`]): asked again once the apply ends
        let building = crate::sync::serve::hub_build_begin()?;
        *b = HubBuild::Running { since: Instant::now() };
        drop(b);
        self.emit("hub", Sev::Info, format!("hub: {} builds its new hub at {} before it restarts", self.machine, &at[..at.len().min(10)]));
        let (state, root, at, emit, machine) = (self.hub_build.clone(), root.to_path_buf(), at.to_string(), self.emitter(), self.machine.clone());
        let spawned = std::thread::Builder::new().name("devhub-hub-build".into()).spawn(move || {
            let ok = build_hub(&root, jobs);
            drop(building);
            let now = super::tree::deploy(&root).map(|p| p.makepad).unwrap_or_default();
            let secs = match &*state.lock().unwrap() { HubBuild::Running { since, .. } => since.elapsed().as_secs(), _ => 0 };
            // (the master says a failure loudly, once for the fleet; a build whose checkout moved
            // under it built no one commit, and names none)
            match &ok {
                _ if now != at => emit("hub", Sev::Info, format!("hub: {machine}'s checkout moved to {} under its build for {}: it builds again", &now[..now.len().min(10)], &at[..at.len().min(10)])),
                Ok(()) => emit("hub", Sev::Info, format!("hub: {machine}'s new hub built at {} in {secs} s", &at[..at.len().min(10)])),
                Err(why) => emit("hub", Sev::Warn, format!("hub: {machine}'s new hub does not build at {}: {why}; it keeps running", &at[..at.len().min(10)])),
            }
            *state.lock().unwrap() = if now == at && !crate::sync::serve::applying() { HubBuild::Done { at, ok } } else { HubBuild::None };
        });
        if spawned.is_err() { *self.hub_build.lock().unwrap() = HubBuild::None; }
        None
    }

    /// `service-restart <name>`: the self-update of a service the WM supervises (`ai-hub`): it
    /// gets SIGTERM (it closes admission and hands its jobs back), and the WM starts it again
    /// through `cargo run` in the tree: the new code.
    fn service_restart(&self, name: &str) -> String {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') || name == "devhub" {
            return err("usage: service-restart <name of a WM-supervised service> (devhub: hub-restart)");
        }
        // a timed run holds the box (and the AI hub's quiet, which a restarted hub would not
        // hold): a service's self-update waits for its end, as the deployment's apply and the
        // WM's restart do (the master asks again)
        if self.timed_live() {
            return err(format!("waits: a timed lease runs on {}: {name} restarts after it", self.machine));
        }
        let pid = service_pid(name);
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
            let binary = std::fs::read_to_string(service_state(name)).ok().and_then(|t| t.lines().find_map(|l| l.strip_prefix("identity = ").and_then(|i| i.split(' ').next()).map(std::path::PathBuf::from)));
            if let Some(binary) = binary.filter(|b| makepad_app_registry::runs_binary(pid, b) == Some(true)) {
                self.emit("hub", Sev::Info, format!("hub: {}'s {name} (pid {pid}) runs the deployment's build already: no restart", self.machine));
                return format!("{{\"current\":{},\"pid\":{pid},\"binary\":{}}}", json_str(name), json_str(&binary.display().to_string()));
            }
        }
        // a user-facing service never restarts under its user (the master's roll waits and asks again)
        let blockers: Vec<String> = user_blockers().into_iter().chain(watched_said()).collect();
        if !blockers.is_empty() {
            return err(format!("waits: {}", blockers.join("; ")));
        }
        // kill by the pid its supervisor recorded
        let ok = std::process::Command::new(os::tool("kill")).args(["-TERM", &pid.to_string()]).status().is_ok_and(|s| s.success());
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

    /// `lease-owns <id> <pid>`: whether the process is one of the lease's ([`lease_pids`]: a JIT a
    /// job started, a JIT-run test's own group included: the master pauses that job on its parked
    /// panic).
    fn owns(&self, rest: &str) -> String {
        let w: Vec<&str> = rest.split_whitespace().collect();
        let (Some(id), Some(Ok(pid))) = (w.first(), w.get(1).map(|p| p.parse::<u32>())) else { return err("usage: lease-owns <id> <pid>") };
        // no process walks under the lease table lock
        let Some(runs) = self.leases.lock().unwrap().get(*id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", self.machine)) };
        let owns = lease_pids(self, id, &runs).contains(&pid);
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
        let memory = self.leases.lock().unwrap().get(*id).and_then(|l| l.memory_end.as_deref()).map_or(String::new(), |why| format!(",\"memory\":{}", json_str(why)));
        format!("{{\"data\":\"{}\",\"off\":{end},\"exit\":{exit}{memory}}}", makepad_network::digest::base64_encode(&buf))
    }

    /// The master's beat at `now`, naming the leases it holds here. Back after an absence
    /// ([`Opts::no_beat`] without a beat), that beat is its claim: a lease from before the absence
    /// it does not name ends now, and every other one's no-beat count starts again. Counted from
    /// any first call instead (its caps poll), a restarted master whose first beat came more than
    /// the limit later lost leases it held (a loaded gate box, 11-04).
    ///
    /// The leases it names are beaten here, with the contact: the beat's look at their runs comes
    /// after it and takes a while (process walks on a loaded box, the lock behind a teardown), and
    /// a watchdog tick in between saw the master there and its leases unbeaten past the limit. It
    /// ended three live leases of a master restart test that way (11-05).
    fn master_contact(&self, now: Instant, claimed: &[&str]) {
        let mut leases = self.leases.lock().unwrap();
        let (away, last) = {
            let mut seen = self.master_seen.lock().unwrap();
            let last = *seen;
            *seen = last.max(now);
            (now.saturating_duration_since(last), last)
        };
        for id in claimed {
            if let Some(l) = leases.get_mut(*id) {
                l.beat = l.beat.max(now);
            }
        }
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

    /// An overlay's JIT resolving waits on the box's store builds, no process of its own: what
    /// they do is what its stall check judges it by (none running: nothing to measure).
    #[cfg(feature = "checkout")]
    fn waits_on(&self, l: &Lease) -> Vec<Arc<Run>> {
        if l.src == Src::Overlay && l.resolution.as_ref().is_some_and(|r| r.answer.is_none()) { self.store_builds.lock().unwrap().iter().map(|(_, r)| r.clone()).collect() } else { Vec::new() }
    }

    #[cfg(not(feature = "checkout"))]
    fn waits_on(&self, _: &Lease) -> Vec<Arc<Run>> {
        Vec::new()
    }

    /// `lease-beat <ids>`: the master holds these leases (and no other here).
    fn beat(&self, rest: &str) -> String {
        let ids = || rest.split_whitespace();
        let now = Instant::now();
        self.master_contact(now, &ids().collect::<Vec<_>>());
        let timed: Vec<(String, String)> = {
            let leases = self.leases.lock().unwrap();
            ids().filter_map(|id| leases.get(id)).filter(|l| l.kind == Kind::Timed).map(|l| (l.id.clone(), l.lane.clone())).collect()
        };
        self.hold_series(&timed, None);
        let samples: Vec<_> = {
            let leases = self.leases.lock().unwrap();
            ids().filter_map(|id| leases.get(id).map(|l| (id.to_string(), l.runs.clone(), self.waits_on(l)))).collect()
        };
        // the groups its runs' processes moved into, while their parents can still be walked
        let known: Vec<Vec<(u32, u64)>> = {
            let leases = self.leases.lock().unwrap();
            samples.iter().map(|(id, _, _)| leases.get(id).map_or_else(Vec::new, |l| l.groups.clone())).collect()
        };
        let sets: Vec<_> = samples.iter().zip(known).map(|((_, runs, _), known)| (run_leaders(runs), known)).collect();
        self.note_groups(samples.iter().map(|(id, _, _)| id.as_str()).zip(os::groups_beside(&sets)));
        let samples: BTreeMap<_, _> = samples.into_iter().map(|(id, runs, waits_on)| {
            let doing: Vec<_> = runs.iter().chain(&waits_on).cloned().collect();
            (id, (sample_runs(&runs), held_now(&runs), activity_now(&doing)))
        }).collect();
        self.beat_from(rest, &samples, now)
    }

    /// Each lease's process groups beside its runs' own, as a look found them ([`Lease::groups`]),
    /// saved when they changed.
    fn note_groups<'a>(&self, seen: impl Iterator<Item = (&'a str, Vec<(u32, u64)>)>) {
        let mut leases = self.leases.lock().unwrap();
        for (id, groups) in seen {
            let Some(l) = leases.get_mut(id).filter(|l| l.groups != groups) else { continue };
            l.groups = groups;
            self.save(l);
        }
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
                    // (a look that could not tell keeps the last one's word)
                    if held.is_some() { l.held_now_bytes = held; }
                    l.quiet = next_quiet(l.quiet, done, now, IDLE_CPU_PCT);
                    l.slow = next_quiet(l.slow, done, now, SLOW_CPU_PCT);
                    // a panic its process no longer holds is no panic of the lease's: it ended (its exit decides)
                    if l.panicked.as_ref().is_some_and(|(pid, _)| os::proc_start(*pid).is_none()) { l.panicked = None; }
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
                        "{}:{{\"running\":{},\"pid\":{},\"work\":{},\"disk_mb\":{},\"held_mb\":{},\"held_peak_mb\":{},\"idle_s\":{},\"slow_s\":{},\"out_bytes\":{out},\"panic\":{},\"class\":{},\"phases\":{}}}",
                        json_str(id),
                        running.as_ref().map_or("null".into(), |(_, c)| json_str(c)),
                        running.as_ref().map_or(0, |(p, _)| *p),
                        json_str(&l.work.to_string_lossy()),
                        mb.map_or("null".into(), |m| m.to_string()),
                        l.held_now_bytes.map_or("null".into(), |b| (b >> 20).to_string()),
                        l.held_bytes.map_or("null".into(), |b| (b >> 20).to_string()),
                        l.quiet.map_or("null".into(), |(since, _)| now.saturating_duration_since(since).as_secs().to_string()),
                        l.slow.map_or("null".into(), |(since, _)| now.saturating_duration_since(since).as_secs().to_string()),
                        l.panicked.as_ref().map_or("null".into(), |(_, text)| json_str(text)),
                        json_str(&l.first_run),
                        l.profile(crate::proto::wall_ms(), false).json()
                    ));
                }
                None => unknown.push(json_str(id)),
            }
        }
        let end = self.over_disk.lock().unwrap().take().filter(|(id, _)| ids().any(|i| i == id));
        let end = end.map_or(String::new(), |(id, why)| format!(",\"end\":{{\"id\":{},\"why\":{}}}", json_str(&id), json_str(&why)));
        // every lease the master beat here that the memory watch ended, until the master ends
        // it (a second end before the next beat is not lost; a repeat ends nothing more there)
        let memory: Vec<String> = ids().filter_map(|id| leases.get(id)?.memory_end.as_deref().map(|why| format!("{{\"id\":{},\"why\":{}}}", json_str(id), json_str(why)))).collect();
        let memory = if memory.is_empty() { String::new() } else { format!(",\"memory\":[{}]", memory.join(",")) };
        format!("{{\"live\":[{}],\"unknown\":[{}],\"info\":{{{}}}{end}{memory}}}", live.join(","), unknown.join(","), info.join(","))
    }

    /// A lease's disk use in MB, measured at most every 2 min on a thread of its own (a target
    /// dir walk can take seconds); None until the first measure. `dirs`: [`Lease::dirs`].
    fn disk_of(&self, id: &str, dirs: Vec<(PathBuf, bool)>, walk: bool) -> Option<u64> {
        let mut m = self.disk_use.lock().unwrap();
        let known = m.get(id).copied();
        if walk && known.is_none_or(|k| k.at.elapsed() > std::time::Duration::from_secs(120)) {
            // mark it measured now, so one walk runs at a time
            let was = known.unwrap_or(DiskUse { at: Instant::now(), mb: u64::MAX, first_mb: None, walks: 0, own_mb: 0, first_own_mb: None });
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
                map.lock().unwrap().insert(id, DiskUse { at: Instant::now(), mb: all >> 20, first_mb: was.first_mb.or(Some(all >> 20)), walks: was.walks + 1, own_mb, first_own_mb: was.first_own_mb.or(Some(own_mb)) });
            });
        }
        known.map(|k| k.mb).filter(|m| *m != u64::MAX)
    }

    /// How much a lease's own directories grew since their first measure, bytes.
    fn disk_grew(&self, id: &str) -> u64 {
        self.disk_use.lock().unwrap().get(id).and_then(|u| Some(u.own_mb.saturating_sub(u.first_own_mb?) << 20)).unwrap_or(0)
    }

    /// How much all of a lease's directories grew from its first measure to its last, the shared
    /// ones it writes too (an overlay's build goes to the dev target), in GB: what its kind needs on
    /// a box's disk ([`super::sched::Sched::record_disk`]). None: measured once or never.
    fn disk_grew_gb(&self, id: &str) -> Option<f64> {
        let u = *self.disk_use.lock().unwrap().get(id)?;
        (u.walks >= 2).then(|| u.mb.saturating_sub(u.first_mb.unwrap_or(u.mb)) as f64 / 1024.0)
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
        // a box out of rotation for its toolchain probes again as a lease there ends (a toolchain
        // move's own lease, say), not at its next turn: 103 finished its 1.100 move and sat out
        // of rotation with nothing running for up to 10 min (11-13)
        if super::caps::toolchain_sick() { probe_soon(); }
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
        // a unit its last run stopped as hung: that run measures nothing of its class
        let hung = l.runs.last().and_then(|r| hung_line(&log_tail(&r.log, usize::MAX)).map(str::to_string)).map_or("null".into(), |h| json_str(&h));
        // a lease something else ended keeps how far its last run got: a cancelled hung test left
        // nothing that named it (11-05)
        let cut = l.runs.last().filter(|_| keep_tail).map_or("null".into(), |r| json_str(&got_to(r, usize::MAX)));
        note_stopping(&l.id, &lease_mark(&self.state, &l.id), job_pids(&l.runs));
        // the groups its runs' processes moved into, read while their parents can still be walked
        let groups = os::groups_beside(&[(run_leaders(&l.runs), l.groups.clone())]).pop().unwrap_or_default();
        let (mut killed, mut leak) = stop_runs(&l.runs, &makepad_remote::discovery::registry_dir(), false);
        let (strays, stray_leak) = stop_strays(&lease_mark(&self.state, &l.id), self.opts.grace);
        if !strays.is_empty() {
            self.emit("lease", Sev::Warn, format!("lease: {} {} of {} on {}: stopped {} process(es) that had left its process groups: {strays:?}", l.id, l.kind.as_str(), l.lane, self.machine, strays.len()));
        }
        killed.extend(strays);
        leak.extend(stray_leak);
        let (beside, beside_leak) = stop_groups(&groups, self.opts.grace);
        if !beside.is_empty() {
            let named: Vec<String> = beside.iter().map(|(pid, name)| format!("{pid} {name}")).collect();
            self.emit("lease", Sev::Warn, format!("lease: {} {} of {} on {}: stopped {} process(es) that had left its runs' groups and its mark: {}", l.id, l.kind.as_str(), l.lane, self.machine, beside.len(), named.join(", ")));
        }
        killed.extend(beside.iter().map(|(pid, _)| *pid).filter(|p| !killed.contains(p)).collect::<Vec<_>>());
        leak.extend(beside_leak);
        #[cfg(feature = "checkout")]
        self.stop_unowned_builds();
        // what it wrote under the overlay's virtual tree on the real disk goes before the next lease
        let wrote_root = self.virtual_root_written(&l);
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
        let cpu = if measured.is_empty() { (l.store_cpu_s > 0.0).then_some(l.store_cpu_s) } else { measured.iter().try_fold(l.store_cpu_s, |a, r| r.usage.lock().unwrap().cpu.map(|c| a + c)) };
        let _ = std::fs::remove_file(self.quiet().join(format!("{}.cmd", l.id)));
        if l.kind == Kind::Timed {
            self.hold_series(&[(l.id.clone(), l.lane.clone())], Some(l.id.as_str()));
            self.end_quiet(&l.id, &l.lane);
        }
        // a process left behind may still run the JIT binary it started from
        if leak.is_empty() {
            for t in self.loader_targets(&l.target) {
                if let Err(e) = super::loaders::release(&t, &l.id) { self.emit("lease", Sev::Warn, format!("JIT binary release {}: {e}", l.id)); }
            }
        }
        if leak.is_empty() && (l.src == Src::Overlay || l.tmp.join("overlay-selftest.json").is_file() || l.work == self.conf().work.join("native").join(l.tree_lane())) {
            #[cfg(feature = "checkout")]
            l.identity.release_jits.store(true, Ordering::Release);
            if let Err(e) = super::jitstore::Store::new(&self.conf().work).release(&l.id) { self.emit("lease", Sev::Warn, format!("JIT store release {}: {e}", l.id)); }
        } else if leak.is_empty() && l.src == Src::Tree {
            // the store JITs a tree lease held: its --jit, or the box's dev JIT it took (start_run)
            if let Err(e) = super::jitstore::Store::new(&self.conf().work).release(&l.id) { self.emit("lease", Sev::Warn, format!("JIT store release {}: {e}", l.id)); }
        }
        #[cfg(feature = "checkout")]
        if l.tmp.join("overlay-selftest.json").is_file() && leak.is_empty() {
            if let Err(e) = super::overlay_selftest::cleanup(&self.conf().work, &l.id) { self.emit("lease", Sev::Warn, format!("selftest cleanup {}: {e}", l.id)); }
        }
        let recovered = self.release_tree(&l);
        if leak.is_empty() && session_lane(&l.lane) {
            self.session_ended(&l.lane);
        }
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
        // (an empty one only: what a run kept there outlives its lease)
        let _ = std::fs::remove_dir(test_artifacts(&l.target, &l.id));
        let sev = if how == "released" { Sev::Info } else { Sev::Warn };
        self.emit("lease", sev, format!("lease: {} {} of {} ended on {} ({how}): {} process group(s) stopped", l.id, l.kind.as_str(), l.lane, self.machine, killed.len()));
        self.guard.touch();
        let (tmp, emit, tag, machine) = (l.tmp.clone(), self.emitter(), format!("{} {} on {}", l.id, l.lane, self.machine), self.machine.clone());
        // the job inputs no lease here names any more go with it (a process left running may
        // still read one: then they wait for a later end)
        let inputs_gone = if leak.is_empty() { self.inputs_unnamed(&l.id) } else { Vec::new() };
        #[cfg(feature = "checkout")]
        let edits = (l.src == Src::Overlay && leak.is_empty()).then(|| l.edits()).flatten();
        #[cfg(feature = "checkout")]
        let (this, overlay) = (self.this.clone(), (l.src == Src::Overlay && leak.is_empty()).then(|| overlay_lane(l.kind, &l.lane, l.pin.as_deref().unwrap_or(""))).filter(|lane| !lane.is_empty() && lane != &l.lane));
        // a timed lease's runs name its series' last build: a deployment's retire keeps it while
        // the master names the lane (jitcache::note_series)
        let series = (l.kind == Kind::Timed).then(|| (l.lane.clone(), l.runs.first().map_or(0, |r| r.started_ms / 1000))).filter(|(_, since)| *since > 0);
        let _ = std::thread::Builder::new().name("devhub-lease-cleanup".into()).spawn(move || {
            if let Some((lane, since)) = &series {
                crate::jitcache::note_series(lane, *since);
            }
            let mut freed = 0u64;
            if tmp.exists() {
                freed += disk::dir_size(&tmp).0;
                let _ = std::fs::remove_dir_all(&tmp);
            }
            for p in &inputs_gone {
                freed += disk::dir_size(p).0;
                let _ = std::fs::remove_dir_all(p);
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
        // what its JIT builds held at most, apart from its commands: what the next one is charged
        let jit_peak = l.runs.iter().filter(|r| r.n > BUILD_RUNS).filter_map(|r| r.usage.lock().unwrap().peak).max().map_or("null".to_string(), |b| format!("{:.3}", b as f64 / caps::GB));
        let toolchain = toolchain_changed(&l.rustc, l.toolchain_own, &super::caps::read_rustc()).map_or("null".into(), |t| json_str(&t));
        // what its directories grew on the box's disk: what the next of its kind there needs
        let disk_grew = self.disk_grew_gb(&l.id).map_or("null".into(), |g| format!("{g:.3}"));
        let receipt = format!("{},\"source\":{},\"class\":{},\"phases\":{},\"jit_peak_gb\":{jit_peak},\"disk_grew_gb\":{disk_grew},\"peak_ram_gb\":{},\"cpu_seconds\":{},\"abnormal\":{abnormal},\"failed_tests\":{failed},\"hung\":{hung},\"tail\":{cut},\"memory\":{},\"virtual_root\":{},\"toolchain\":{toolchain}{inspect}{noise}}}", line.trim_end_matches('}'), json_str(&l.source), json_str(&l.first_run), profile.json(), l.peak_ram_bytes.map_or("null".into(), |n| (n as f64 / (1u64 << 30) as f64).to_string()), cpu.map_or("null".into(), |n| n.to_string()), l.memory_end.as_deref().map_or("null".into(), json_str), wrote_root.as_deref().map_or("null".into(), json_str));
        if let Err(e) = self.save_end(&l.id, &receipt) { return err(e); }
        receipt
    }

    /// The box's job inputs ([`super::ship::input_dir`]) that no lease in the table names, moved
    /// aside under its lock to `.gone-<hash>-<id>` (a lease that names one after that ships it
    /// again), and those moved aside before whose removal a restart cut short: what to remove.
    /// An input is the leases' that name it, never the box's: kept by hash past them, a Linux node held
    /// 52 unpacked copies of one board from a timed series, and a lane's run that deleted the
    /// board's file after its run broke the next lease of the same hash there (11-21).
    fn inputs_unnamed(&self, id: &str) -> Vec<PathBuf> {
        let dir = self.conf().work.join("inputs");
        let leases = self.leases.lock().unwrap();
        let named: BTreeSet<&str> = leases.values().flat_map(|l| l.inputs.iter().map(String::as_str)).collect();
        let mut gone = Vec::new();
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(".gone-") {
                gone.push(e.path());
            } else if super::ship::is_hash(&name) && !named.contains(name.as_str()) {
                let to = dir.join(format!(".gone-{name}-{id}"));
                if std::fs::rename(e.path(), &to).is_ok() { gone.push(to); }
            }
        }
        gone
    }

    /// The overlay's virtual root (`<work>/dev/src`, [`super::relaxgit::prepare`]) as a real folder
    /// on this box: a run's process wrote under the virtual tree with real file calls (a native
    /// baseline build's target and log, a Linux node, 11-06), and the box refused every overlay lease after
    /// it until someone removed it by hand. At each lease's end, its processes stopped, the folder
    /// goes and the leases whose runs were going when it was last written are named. The ending
    /// lease, when it is the one alone, wrote it: why it fails. None: no such folder, or not its.
    fn virtual_root_written(&self, ending: &Lease) -> Option<String> {
        let root = self.conf().work.join("dev/src");
        let meta = std::fs::symlink_metadata(&root).ok()?;
        let at = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
        let mut files: Vec<String> = Vec::new();
        let mut dirs = vec![root.clone()];
        while let Some(d) = dirs.pop().filter(|_| files.len() < 4) {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if e.file_type().is_ok_and(|t| t.is_dir()) { dirs.push(p); } else if files.len() < 4 { files.push(p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().into_owned()); }
            }
        }
        let gone = if meta.is_dir() { std::fs::remove_dir_all(&root) } else { std::fs::remove_file(&root) };
        let ran = |l: &Lease| l.runs.iter().any(|r| r.started_ms <= at + STAMP_GRAIN_MS && { let end = r.ended_ms.load(Ordering::Acquire); end == 0 || end >= at });
        let mut by: Vec<String> = self.leases.lock().unwrap().values().filter(|l| ran(l)).map(|l| format!("{} ({})", l.id, l.lane)).collect();
        let mine = ran(ending);
        if mine { by.insert(0, format!("{} ({})", ending.id, ending.lane)); }
        let why = format!("wrote under the overlay's virtual root {} on the disk of {} ({}), which refuses every overlay lease of the box while it stands; {}", root.display(), self.machine, if files.is_empty() { "no files".to_string() } else { files.join(", ") }, match gone { Ok(()) => "removed".to_string(), Err(e) => format!("its removal failed: {e}") });
        let who = match &by[..] {
            [] => "no lease's run (one that ended before its write was seen, or the box's own processes)".to_string(),
            [one] => one.clone(),
            many => format!("one of {}", many.join(", ")),
        };
        self.emit("lease", Sev::Err, format!("lease: {who} {why}"));
        (mine && by.len() == 1).then_some(why)
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
        for k in &r.kept {
            self.emit("ws", Sev::Warn, format!("ws_kept: {who}: {k}"));
        }
        for l in &r.locks {
            self.emit("ws", Sev::Warn, format!("ws_lock: {who}: {l}"));
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

    /// `input-have` / `input-put` ([`super::ship::input_put`]): a job input the lease's runs read,
    /// kept on the box by its hash while a lease names it; the lease names the ones it took, so
    /// none goes under it, and the last one's end removes it ([`Self::inputs_unnamed`]).
    /// `input-home`: a held one copied afresh as the lease's own app home ([`super::ship::input_home`]).
    fn input(&self, word: &str, rest: &str) -> String {
        let (id, rest) = rest.split_once(' ').unwrap_or((rest, ""));
        let hash = rest.split_whitespace().next().unwrap_or("");
        if !super::ship::is_hash(hash) { return err(format!("{word}: bad hash `{hash}`")); }
        // named before the look: a reclaim never takes it between the look and the run
        let tmp = match self.leases.lock().unwrap().get_mut(id) {
            Some(l) => {
                if !l.inputs.iter().any(|h| h == hash) {
                    l.inputs.push(hash.to_string());
                    self.save(l);
                }
                l.tmp.clone()
            }
            None => return err(format!("no lease {id} on {}", self.machine)),
        };
        let work = self.conf().work;
        let placed = match word {
            "input-have" => super::ship::input_have(&work, hash),
            "input-home" => super::ship::input_home(&work, &tmp, hash).map(Some),
            _ => super::ship::input_put(&work, &tmp, rest),
        };
        match placed {
            Ok(Some(dir)) => format!("{{\"have\":true,\"path\":{}}}", json_str(&dir.to_string_lossy())),
            Ok(None) => "{\"have\":false}".into(),
            Err(e) => err(e),
        }
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
            let n = l.next_run();
            let log = l.tmp.join(format!("run-{n}.log"));
            let output = match std::fs::File::create(&log) { Ok(f) => Mutex::new(f), Err(e) => return err(e.to_string()) };
            if let Err(e) = std::fs::write(&spec, format!("{{\"case\":{},\"n\":{n}}}", json_str(case))) { return err(e.to_string()); }
            // A pid of zero is the in-hub summary, not a process group. The actual JITs
            // and any Cargo build have their own registered runs and cancellation handling.
            let summary = Arc::new(Run { request: String::new(), started_ms: crate::proto::wall_ms(), spawned_ms: crate::proto::wall_ms(), ended_ms: AtomicU64::new(0), n, pid: 0, start: 0, unit: None, log, exit: Mutex::new(None), ours: true, detachable: false, hold: os::Hold::none(), exit_file: exit_file(&self.state, id, n), cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
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
                let run = |id: &str, cmd: &std::process::Command, active: &dyn Fn() -> Result<(), String>| this.overlay_build(&lease, id, cmd, active);
                // the selftest's fixtures fail on purpose: their warnings go to its log
                let build = super::loaders::OverlayBuild { jobs: cores, niced: false, run: &run, cancelled: &identity.cancelled, say: &say, pending: &say, warn: &say, active: &active, sandboxed: false };
                let spawn = |cmd: &std::process::Command| this.worker_process(&lease, &identity, &tmp, cmd, &[], &[], false);
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

    /// The folder `devhub get` reads and the prefix that names it: the box's JIT captures
    /// (`captures/`, any lane's, lease or not), a live lease's work (`target/`, `tmp/`: its target
    /// and scratch), else the lane's dir (`src/`: the tree `run` ships and runs in; `target/`: the
    /// lane's warm target). Returns (root, prefix, path under root).
    /// A lane with a native tree reads only that one: when another lane's lease took the tree
    /// mid-copy, a sheet run's get read on in the box's shared tree and failed there with
    /// "No such file" (11-07).
    fn get_root(&self, lane: &str, lease: &str, rel: &str) -> Result<(PathBuf, &'static str, String), String> {
        if !valid_name(lane) || !inside(rel) {
            return Err("usage: <lane> <lease|-> <relative path inside the lane's dir>".into());
        }
        let under = |pre: &str| if rel == pre { Some(".".to_string()) } else { rel.strip_prefix(pre).and_then(|r| r.strip_prefix('/')).map(str::to_string) };
        // a capture's report and flight recording (`makepad inspect … capture`, a hung unit's) are
        // a few files read by name: a lane's pinned `makepad inspect file=` lease for one queued
        // 19 min behind its box's tests (11-10); copied, `makepad inspect file=<x.mkpf> pf ..`
        // queries it anywhere
        if let Some(r) = under("captures") {
            return Ok((makepad_remote::hot::home().join("captures"), "captures/", r));
        }
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
            let conf = self.conf();
            let root = super::tree::native_root(&conf.work, lane);
            if root.join(".git").exists() {
                // a get under way keeps the tree the last its box takes for another lane
                super::tree::touch_native(&root);
                return Ok(match (under("target"), under("src")) {
                    (Some(r), _) => (super::tree::native_target(&conf.work, lane), "target/", r),
                    (_, Some(r)) => (root, "src/", r),
                    _ => return Err("without --lease: `src/..` or `target/..`".into()),
                });
            }
            if self.opts.tree.is_some() && !shared_tree_lane(lane, conf.role) {
                return Err(format!("lane {lane} has no tree on {} now: its native tree went to another lane's lease after its own ended. A run's outputs come back with `devhub run ... --keep <dir>`: what the run puts in $DEVHUB_KEEP is copied before its lease ends", self.machine));
            }
        }
        // no live lease: the node's tree for a lane that works in it; a node without one: the
        // lane's dir
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

    /// A lease's `--jit` copied between boxes through the master: `jit-read <id> <off>`, a chunk
    /// of one this box's store holds; `jit-put <id> <off> <size> <data b64>`, a chunk of one
    /// copied in ([`super::jitstore::Store::put`]).
    fn jit_copy(&self, word: &str, rest: &str) -> String {
        let store = super::jitstore::Store::new(&self.conf().work);
        let w: Vec<&str> = rest.splitn(4, ' ').collect();
        let num = |s: &str| s.parse::<u64>().map_err(|_| format!("{word}: bad number `{s}`"));
        let answer = match (word, &w[..]) {
            ("jit-read", &[id, off]) => num(off).and_then(|off| store.read(id, off).map(|(data, size)| {
                format!("{{\"data\":\"{}\",\"off\":{},\"size\":{size}}}", makepad_network::digest::base64_encode(&data), off + data.len() as u64)
            })),
            ("jit-put", &[id, off, size, data]) => num(off).and_then(|off| Ok((off, num(size)?))).and_then(|(off, size)| {
                let data = b64_decode(data.trim()).ok_or("jit-put: data is not base64")?;
                Ok(match store.put(id, off, size, &data)? {
                    super::jitstore::Put::Held => "{\"held\":true}".to_string(),
                    super::jitstore::Put::More(end) => format!("{{\"off\":{end}}}"),
                    super::jitstore::Put::Kept => "{\"kept\":true}".to_string(),
                })
            }),
            _ => Err("usage: jit-read <id> <off> | jit-put <id> <off> <size> <data b64>".to_string()),
        };
        answer.unwrap_or_else(err)
    }

    /// `jit-keep <hold> <key> [<binary>]`: a failed bucket's JIT stays on this box, in its dev store
    /// held as `hold` until `jit-unkeep <hold>`, however many newer ones come (bucket.rs
    /// `keep_failed_jits`). A loader outside the store goes into it first, from the binary its run's
    /// log named (`/x/makepad.jit-<key>`), else a tree's kept one (`ci-target`, a native lane's
    /// target): there `--jit <key>` finds it, and a box of its platform copies it from there.
    /// Every box evicted the fast bucket's aborting JIT within the hour, and `--jit` reached it
    /// nowhere (11-10).
    fn jit_keep(&self, word: &str, rest: &str) -> String {
        let work = self.conf().work;
        let store = super::jitstore::Store::new(&work);
        let w: Vec<&str> = rest.split_whitespace().collect();
        let answer = match (word, &w[..]) {
            ("jit-keep", &[hold, key, ref named @ ..]) if named.len() <= 1 => (|| {
                store.hold(key, hold)?;
                if store.has(key) {
                    return Ok(format!("{{\"kept\":{},\"from\":\"store\"}}", json_str(key)));
                }
                // (only a file named for its key: what it holds is what the key says)
                let named = named.first().map(PathBuf::from).filter(|p| {
                    p.is_absolute() && p.is_file() && p.file_name().and_then(|f| f.to_str()).and_then(|f| f.strip_prefix("makepad.jit-")).map(|k| k.trim_end_matches(std::env::consts::EXE_SUFFIX)) == Some(key)
                });
                // where a tree lease's run found it ([`Self::loader_targets`]): the deployment's
                // target first (a pool box's fast gate ran its JIT from there, 11-10), the CI tree's, the
                // lanes' native ones
                #[cfg(feature = "checkout")]
                let bin = named.or_else(|| [crate::sync::SyncConf::load().checkout.join("target"), work.join("ci-target")].into_iter().chain(std::fs::read_dir(work.join("native-target")).into_iter().flatten().flatten().map(|e| e.path())).find_map(|t| super::loaders::kept(&t, key)));
                #[cfg(not(feature = "checkout"))]
                let bin = named;
                let Some(bin) = bin else {
                    let _ = store.release(hold);
                    return Err(format!("JIT {key} is in neither this box's store nor a tree's target"));
                };
                store.keep(key, &bin)?;
                Ok(format!("{{\"kept\":{},\"from\":{}}}", json_str(key), json_str(&bin.display().to_string())))
            })(),
            ("jit-unkeep", &[hold]) => store.release(hold).map(|_| format!("{{\"released\":{}}}", json_str(hold))),
            _ => Err("usage: jit-keep <hold> <key> [<binary>] | jit-unkeep <hold>".to_string()),
        };
        answer.unwrap_or_else(err)
    }

    /// The JIT builds a timed lease's runs took here, held as a running process holds one (relax
    /// store.rs hold_build: a shared lock on `<run dir>/cache/live/<build>`) until
    /// [`SERIES_KEEP_MS`] after the series' last timed lease here used them: relax's prune never
    /// takes a store of a held build, so a series' arms stay warm between its runs, whatever else
    /// runs or the volume's floor. A build is the lease's when its lock was taken during the lease
    /// (relax stamps the file then) or is held now: nothing else runs beside a timed lease. One arm
    /// of a timed A/B lost 58 stores to the prune between its runs (11-18). `timed`: the running
    /// timed leases (id, lane), looked at every [`SERIES_LOOK`] and at `ended`'s end; every call
    /// lets go of the holds past their time.
    fn hold_series(&self, timed: &[(String, String)], ended: Option<&str>) {
        let mut s = self.series.lock().unwrap();
        let look = !timed.is_empty() && (ended.is_some() || s.looked.is_none_or(|t| t.elapsed() >= SERIES_LOOK));
        let dirs: Vec<PathBuf> = if look { crate::jitcache::caches().into_iter().map(|(d, _)| d).collect() } else { Vec::new() };
        let file = self.state.parent().unwrap_or(&self.state).join("series-holds");
        if let Some(line) = s.update(&file, &dirs, timed, ended, crate::proto::wall_ms()) {
            self.emit("hub", Sev::Info, format!("{}: {line}", self.machine));
        }
    }

    /// A live test session's own lane ([`session_lane`]) at its lease's end: its dirs go as a
    /// landed lane's do ([`Self::lane_to_trash`]), the trash emptied off this path. The lane is
    /// the session's alone, and its session ends with its lease. Nothing took them before: one Mac
    /// held 133 ended sessions' dirs, 7.8 GB, on a volume whose JIT store evicted by the space
    /// left (11-18).
    /// The end can come on a thread that holds the start lock already (a lease that ended as it
    /// started, [`Self::lease_start`]), where taking it again never frees: two pool boxes' lease
    /// starts and ends all waited 22 min on such hubs (11-18). Held there or anywhere, the cleanup
    /// goes on a thread of its own, once the holder lets go.
    fn session_ended(&self, lane: &str) {
        match self.starting.try_lock() {
            Ok(starting) => self.session_dirs_go(lane, starting),
            Err(crate::held::WouldBlock) => {
                let (this, lane) = (self.this.clone(), lane.to_string());
                let _ = std::thread::Builder::new().name("devhub-session-end".into()).spawn(move || {
                    let Some(x) = this.upgrade() else { return };
                    let starting = x.starting.lock().unwrap_or_else(|e| e.into_inner());
                    x.session_dirs_go(&lane, starting);
                });
            }
        }
    }

    /// [`Self::session_ended`] with the start lock taken: no start of the lane comes between its
    /// look at the leases and its mark as cleaning.
    fn session_dirs_go(&self, lane: &str, starting: crate::held::HeldGuard<'_, ()>) {
        {
            let _starting = starting;
            if self.leases.lock().unwrap().values().any(|l| l.lane == lane) { return; }
            self.cleaning.0.lock().unwrap().insert(lane.to_string());
        }
        let left = self.lane_to_trash(lane);
        self.cleaning.0.lock().unwrap().remove(lane);
        self.cleaning.1.notify_all();
        match left {
            Ok(removed) if removed.is_empty() => {}
            Ok(_) => super::disk::empty_trash_soon(&self.conf().work),
            Err(e) => self.emit("disk", Sev::Warn, format!("disk: session lane {lane} ended on {}; its dirs stay: {e}", self.machine)),
        }
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

    /// `lane-dirs`: the bytes on disk of each lane's dirs here (the ones `disk clean <lane>`
    /// removes), as `{"lanes":{"<lane>":<bytes>}}`. Reads only.
    fn lane_sizes(&self) -> String {
        let w = self.conf().work;
        let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
        let dirs = [(w.join("dev/lane"), ""), (w.join("dev/overlay"), ""), (w.join("dev/lane-jit"), ""), (w.join("lane"), ""), (w.join("target"), "lane-"), (w.join("native"), ""), (w.join("native-target"), ""), (caps::home_dir().join("rc/pool"), "")];
        for (dir, prefix) in dirs {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(lane) = name.strip_prefix(prefix).map(|l| l.strip_suffix(".last").unwrap_or(l)) else { continue };
                if makepad_devhub_conf::valid_name(lane) {
                    *sizes.entry(lane.to_string()).or_default() += super::disk::dir_size(&e.path()).0;
                }
            }
        }
        format!("{{\"lanes\":{{{}}}}}", sizes.iter().map(|(l, b)| format!("{}:{b}", json_str(l))).collect::<Vec<_>>().join(","))
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
        let orphan = |pid: u32| makepad_remote::procs::process(pid).is_some_and(|p| p.ppid == 1);
        let mine = format!("{LEASE_ENV}={}/", self.state.display());
        let now: Vec<(u32, String)> = os::env_values(&mine).into_iter().filter(|(p, id)| *p != me && valid_name(id) && !live.contains(id) && orphan(*p)).collect();
        for (pid, id) in &now {
            let hard = seen.contains(pid);
            os::signal_one(*pid, hard);
            self.emit("lease", Sev::Warn, format!("lease: {}: {} pid {pid} of lease {id}, which is no longer here", self.machine, if hard { "killed" } else { "stopped" }));
        }
        *seen = now.into_iter().map(|(p, _)| p).collect();
        drop(seen);
        #[cfg(windows)]
        self.end_gone_jobs();
    }

    /// Windows marks no process: a run is its job, named for its exit file, and the wrapper's pid
    /// file stays beside it until its lease's end removes both. One whose lease this hub no
    /// longer has went unattended (L215689 on Windows, 11-02, before runs had jobs): its job
    /// ends, and its files go. (On Unix a pid file's pid may be another process's by now: the
    /// sweep there goes by the lease's mark.)
    #[cfg(windows)]
    fn end_gone_jobs(&self) {
        // (one lock at a time)
        let here = || {
            let mut ids: Vec<String> = self.leases.lock().unwrap().keys().cloned().collect();
            ids.extend(self.endings.lock().unwrap().keys().cloned());
            ids
        };
        for (id, exit) in gone_runs(&self.state, &here) {
            let hold = os::Hold::taken_back(&exit);
            let pids = os::job_pids(&hold);
            if let Some(&pid) = pids.first() {
                os::signal_group(pid, true, &hold);
                self.emit("lease", Sev::Warn, format!("lease: {}: ended the job of a run of lease {id}, which is no longer here: pid(s) {pids:?}", self.machine));
            }
            for ext in ["exit", "usage", "usage.pid", "usage.cgroup", "time", "cpu", "warmup"] {
                let _ = std::fs::remove_file(exit.with_extension(ext));
            }
        }
    }
}

/// The runs whose wrapper's pid file (`<id>-<n>.usage.pid`) is still in the hub's state dir while
/// their lease is neither here nor ending (`here`: those leases, read after the folder, so a lease
/// that started meanwhile is one of them): (lease, the run's exit file, which names its job).
#[cfg(any(windows, test))]
fn gone_runs(state: &Path, here: &dyn Fn() -> Vec<String>) -> Vec<(String, PathBuf)> {
    let runs: Vec<(String, u32)> = std::fs::read_dir(state).into_iter().flatten().flatten().filter_map(|e| {
        let name = e.file_name().into_string().ok()?;
        let (id, n) = name.strip_suffix(".usage.pid")?.rsplit_once('-')?;
        let n: u32 = n.parse().ok()?;
        valid_name(id).then(|| (id.to_string(), n))
    }).collect();
    let here = here();
    let mut gone: Vec<(String, PathBuf)> = runs.into_iter().filter(|(id, _)| !here.contains(id)).map(|(id, n)| { let exit = exit_file(state, &id, n); (id, exit) }).collect();
    gone.sort();
    gone
}

/// How often a box proves it can link.
const TOOLCHAIN_EVERY: Duration = Duration::from_secs(10 * 60);
/// How often a box looks whether a store command failed for it, or whether git works its stores
/// again.
const GIT_TICK: Duration = Duration::from_secs(30);

/// The toolchain loop's thread and whether a probe is asked for before its turn ([`probe_soon`]).
static PROBE_THREAD: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();
static PROBE_SOON: AtomicBool = AtomicBool::new(false);

/// The toolchain probe at the loop's next wake, now (a lease ended on a box out of rotation).
fn probe_soon() {
    PROBE_SOON.store(true, Ordering::Release);
    if let Some(t) = PROBE_THREAD.get() { t.unpark(); }
}

/// One toolchain probe, said when its verdict changed: why the box cannot link, if it cannot.
fn probe_toolchain_now(x: &Exec) -> Option<String> {
    // the probe reads the toolchain too, which keys the JIT store by it: a rustup update between
    // leases re-keys the hub here, with no lease and no restart
    super::caps::read_rustc();
    let dir = x.conf().work.join("tmp").join("toolchain-probe");
    let (why, changed) = super::caps::probe_toolchain(&dir);
    if changed {
        match &why {
            Some(why) => x.emit("toolchain", Sev::Err, format!("toolchain: {} is out of rotation, it cannot link: {why}", x.machine)),
            None => x.emit("toolchain", Sev::Info, format!("toolchain: {} links again, back in rotation", x.machine)),
        }
    }
    why
}

/// A box whose toolchain cannot link, or whose git fails on its stores, takes no leases until it
/// works again (`Caps::sick`): each is said once.
fn toolchain_loop(w: Weak<Exec>) {
    os::nice_this_thread();
    let _ = PROBE_THREAD.set(std::thread::current());
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
        // a timed lease runs alone: both wait. Leftovers must go at each free look, not at the
        // next toolchain probe, while a timed ask waits for a quiet bench box (11-14).
        if !x.timed_live() {
            x.end_orphans();
            // one asked for ([`probe_soon`]) runs before its turn
            if !linked.is_some_and(|t| t.elapsed() < TOOLCHAIN_EVERY) || PROBE_SOON.swap(false, Ordering::AcqRel) {
                linked = Some(std::time::Instant::now());
                probe_toolchain_now(&x);
            }
        }
        drop(x);
        std::thread::park_timeout(GIT_TICK);
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

/// The toolchain changed under a lease (`rustc -vV` at its start, `rustc` at its end): its runs
/// have no verdict. A rustup update beside four leases failed each one's cargo with "binary not
/// applicable to the stable toolchain", which read as each lane's own bug (11-13). Not when the
/// lease changed it itself, nor when its start could not read one.
fn toolchain_changed(at_start: &str, own: bool, now: &caps::Rustc) -> Option<String> {
    if own || at_start.is_empty() { return None; }
    let now_text = format!("{} {}", now.release, now.commit);
    if now_text == at_start { return None; }
    let release = |t: &str| t.split_whitespace().next().unwrap_or("").to_string();
    let now_said = if now.release.is_empty() { "none (rustc did not answer)".to_string() } else { format!("rustc {}", now.release) };
    Some(format!("rustc {} -> {now_said}", release(at_start)))
}

/// Linux can kill another lease's process when this box runs out of RAM. A Mac instead swaps,
/// and its pressure already stops grants, so this watch runs only on Linux.
#[cfg(target_os = "linux")]
fn memory_loop(w: Weak<Exec>) {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(x) = w.upgrade() else { return };
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            let (total, avail) = caps::parse_meminfo(&text);
            let gb = (1u64 << 30) as f64;
            x.look_over_memory((total * gb) as u64, (avail * gb) as u64);
        }
    }
}

/// What a live lease holds, and the source of the RAM weight sent with its grant.
#[cfg(any(target_os = "linux", test))]
#[derive(Clone)]
struct MemTook {
    id: String,
    lane: String,
    kind: Kind,
    held: u64,
    weighed: f64,
    peak: Option<f64>,
    asked: bool,
}

/// Pressure alone is not a verdict: only a lease holding more than its weight can be ended.
#[cfg(any(target_os = "linux", test))]
fn lease_over_memory(machine: &str, avail: u64, floor: u64, leases: &[MemTook]) -> Option<(String, String)> {
    if avail >= floor { return None; }
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let excess = |l: &MemTook| gb(l.held) - l.weighed;
    let l = leases.iter().filter(|l| excess(l) > 0.0).max_by(|a, b| excess(a).total_cmp(&excess(b)))?;
    let source = if l.asked {
        "its own --ram-gb ask".to_string()
    } else if let Some(peak) = l.peak.filter(|peak| l.weighed + 0.05 < peak * 1.2) {
        format!("what its box had room for, below its class's measured peak {peak:.1} GB and a fifth")
    } else if let Some(peak) = l.peak {
        format!("its class's measured peak {peak:.1} GB and a fifth")
    } else {
        "its class had no measured peak yet: its kind's default".to_string()
    };
    Some((l.id.clone(), format!("{} ({} of {}) ended on {machine} for memory: it held {:.1} GB, past the {:.1} GB it was weighed at ({source}); {machine} had {:.1} GB free, under its {:.1} GB floor. Ended before the kernel's OOM killer would pick a process, maybe another lease's; its peak is its class's measure now.", l.id, l.kind.as_str(), l.lane, gb(l.held), l.weighed, gb(avail), gb(floor))))
}

/// Where the live ones of a lease's runs hold their memory ([`os::held_where`]), as the end of its
/// memory end's line: ` It held it in: anon .. GB, shmem .. GB; 49.8 GB pid N `cmd`, ...`. Empty
/// when none is live.
#[cfg(target_os = "linux")]
fn held_where_line(runs: &[Arc<Run>]) -> String {
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    let parts: Vec<String> = runs.iter().filter(|r| r.pid != 0 && r.ended() == 0).map(|r| {
        let (split, top) = os::held_where(r.pid, r.start, r.unit.as_deref(), &r.exit_file.with_extension("usage"), 3);
        let procs = top.iter().map(|(bytes, pid, cmd)| format!("{:.1} GB pid {pid} `{cmd}`", gb(*bytes))).collect::<Vec<_>>().join(", ");
        match split {
            Some((anon, shmem)) => format!("anon {:.1} GB, shmem {:.1} GB (tmpfs files count); {procs}", gb(anon), gb(shmem)),
            None => procs,
        }
    }).collect();
    if parts.is_empty() { String::new() } else { format!(" It held it in: {}.", parts.join(" / ")) }
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
        let _ = makepad_remote::group::command(os::tool("systemctl")).args(["--user", "stop", &format!("{u}.scope")]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    }
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) && alive.iter().any(|r| os::group_alive(r.pid, r.start, &r.hold)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let leak = alive.iter().filter(|r| os::group_alive(r.pid, r.start, &r.hold) || os::same(r.pid, r.start)).map(|r| r.pid).collect();
    (alive.iter().map(|r| r.pid).collect(), leak)
}

/// A live run whose JIT ended: what it left in its scope is stopped as a lease's end stops its runs
/// ([`stop_runs`]: its scope stopped, systemd's TERM first), and named in its log. A live test's
/// restart stopped its recorded pid and left its JIT, compiling at up to 2.8 cores, in the ended
/// run's scope until the session's lease ended a minute on, read as "left by ended lease" while
/// that lease still ran (L1446804, 11-19).
fn stop_left(run: &Arc<Run>) {
    let left = os::scope_procs(run.pid, run.start, run.unit.as_deref(), &run.exit_file.with_extension("usage"));
    if left.is_empty() { return; }
    let _ = stop_runs(std::slice::from_ref(run), &makepad_remote::discovery::registry_dir(), false);
    let still = os::scope_procs(run.pid, run.start, run.unit.as_deref(), &run.exit_file.with_extension("usage"));
    let named: Vec<String> = left.iter().map(|(p, cmd)| format!("{p} {cmd}")).collect();
    let line = format!("devhub: run {} ended; stopped what it left in its scope: {}{}", run.n, named.join(", "),
        if still.is_empty() { String::new() } else { format!(" (still there: {})", still.iter().map(|(p, _)| p.to_string()).collect::<Vec<_>>().join(", ")) });
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&run.log) {
        let _ = writeln!(f, "{line}");
    }
}

/// The group leaders of a lease's runs, with their starts ([`os::groups_beside`]).
fn run_leaders(runs: &[Arc<Run>]) -> Vec<(u32, u64)> {
    runs.iter().filter(|r| r.pid != 0).map(|r| (r.pid, r.start)).collect()
}

/// Stops the processes of the groups a lease's runs moved into ([`Lease::groups`]): what neither
/// its runs' groups nor its mark took with them. TERM, looking again while the grace lasts, then
/// KILL. (stopped, each with its name as it was found; left)
fn stop_groups(groups: &[(u32, u64)], grace: Duration) -> (Vec<(u32, String)>, Vec<u32>) {
    let find = || os::groups_left(groups);
    let mut stopped: Vec<(u32, String)> = Vec::new();
    let t = Instant::now();
    let mut hard = false;
    loop {
        let now = find();
        if now.is_empty() { return (stopped, Vec::new()); }
        if !hard && t.elapsed() >= grace { hard = true; }
        if hard && t.elapsed() >= grace + Duration::from_secs(2) { return (stopped, now); }
        for &p in &now {
            if !stopped.iter().any(|(s, _)| *s == p) {
                stopped.push((p, makepad_remote::procs::process(p).map_or_else(|| "?".to_string(), |pr| pr.name)));
            }
            os::signal_one(p, hard);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Stops what the lease's runs started and their groups did not take with them (and what the runs
/// of a hub running in it started, [`LEASE_OUTER_ENV`]): TERM, looking again while the grace
/// lasts (a WM on its way out may still start an app), then KILL. (stopped, left)
/// An app a run `open`s on a Mac is one: Launch Services hands it the run's environment, the mark
/// included, so a run that restarts a user's app sees it quit as the lease ends (11-19).
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

/// A lease start in flight, on disk while it runs (`<state>/<id>.starting`, beside the ledger its
/// end writes): a WM that would replace this hub waits for it as for a running lease (apps/wm
/// `hub_app::live_leases`), and lets it finish past its stop's grace. A WM restart replaced a
/// hub in the middle of a start, and the start's git worktree lock stayed behind (11-13).
const START_MARK: &str = "starting";

struct StartMark(PathBuf);

impl StartMark {
    fn new(state: &Path, id: &str) -> StartMark {
        let p = state.join(format!("{id}.{START_MARK}"));
        let _ = std::fs::write(&p, "");
        StartMark(p)
    }
}

impl Drop for StartMark {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Asked to end (SIGTERM: the WM that replaces this hub, a service stop): this hub refuses new
/// starts, finishes the one in flight, and exits. Its leases' runs live on in their own scopes and
/// the next hub takes them back from the ledger, as after `hub-restart`. A WM restart replaced a
/// hub in the middle of a lease start, and the start's git worktree lock stayed behind (11-13).
/// False: no hub runs in this process.
pub fn drain_and_exit() -> bool {
    let Some(x) = EXEC.get().cloned() else { return false };
    if x.restarting.swap(true, Ordering::AcqRel) { return true; }
    let _ = std::thread::Builder::new().name("devhub-hub-drain".into()).spawn(move || {
        let started = Instant::now();
        drop(x.starting.lock().unwrap());
        let waited = started.elapsed();
        let start = if waited < Duration::from_millis(50) { String::new() } else { format!(" after the lease start in flight ({} ms)", waited.as_millis()) };
        x.emit("hub", Sev::Info, format!("hub: {} ends, asked to{start}: its leases' runs go on, the next hub takes them back", x.machine));
        std::process::exit(0);
    });
    true
}

/// Whether this node takes writes from the network (launches, stops, WM restarts): its dev
/// switch as the master last pushed it, or its role's default. A process with no node: no.
pub fn writes_allowed() -> bool {
    EXEC.get().is_some_and(|x| x.dev.load(Ordering::Acquire))
}

/// A JIT process of a lease's runs (its discovery `lease` mark, [`LEASE_ENV`]) parked on a panic:
/// the lease's beats say so while that process runs, and the master ends a lease nobody attends
/// once it has gone quiet ([`super::sched::PANIC_QUIET_MS`]). A mark of no lease here is none of
/// this hub's. Whether nobody attends that lease ([`super::nobody_attends`]): its JIT runs are
/// `--unattended` and never park, so the node's watch releases one that parked anyway
/// ([`unattended_release`]) and the run ends as an unattended one does. devhub makes unattended
/// only the run it starts: a backtrace sample's script ran `makepad run` itself, its panic parked
/// under the hub's watch, and the master ended the quiet lease a minute later, four times, with its
/// script's later steps and their output (L1381474..L1382556, 11-18). A test that parks on purpose
/// and decides the park itself declares it (RELAX_EXPECTED_CRASH): [`parked_panic`] skips it.
pub fn app_panicked(mark: &str, pid: u32, text: &str) -> bool {
    EXEC.get().is_some_and(|x| x.app_panicked(mark, pid, text))
}

/// How a parked stop in a run nobody attends is released (`resume <id> <word>`): a panic unwinds,
/// as under `--unattended`; any other park (code that did not compile, a stale access, a live
/// update that did not build) takes `stop`, which ends the run, the one word the JIT takes for
/// those (inspect events.rs `cmd_resume`).
pub fn unattended_release(detail: &str) -> &'static str {
    let kind = makepad_strict_json::parse(detail.as_bytes()).ok().and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_string));
    if kind.as_deref() == Some("panic") { "panic" } else { "stop" }
}

/// Its process's `panics` no longer lists the panic it parked on: the lease's beats stop saying it.
pub fn app_unparked(mark: &str, pid: u32) {
    if let Some(x) = EXEC.get() { x.app_unparked(mark, pid); }
}

impl Exec {
    fn app_panicked(&self, mark: &str, pid: u32, text: &str) -> bool {
        let mut leases = self.leases.lock().unwrap();
        let Some(l) = leases.values_mut().find(|l| lease_mark(&self.state, &l.id) == mark) else { return false };
        l.panicked = Some((pid, crate::proto::clean(text, 300)));
        super::nobody_attends(l.kind, l.priority, l.live)
    }

    fn app_unparked(&self, mark: &str, pid: u32) {
        let mut leases = self.leases.lock().unwrap();
        if let Some(l) = leases.values_mut().find(|l| lease_mark(&self.state, &l.id) == mark && l.panicked.as_ref().is_some_and(|(p, _)| *p == pid)) {
            l.panicked = None;
        }
    }
}

/// A JIT panic record's parked panic (`detail`: the JIT's event, `line`: its record's line): its
/// id, and what it said without the record's `panic: `. None for a crash its process said it
/// expects (RELAX_EXPECTED_CRASH: the test's own), or a record that names no panic of the JIT's.
pub fn parked_panic(detail: &str, line: &str) -> Option<(String, String)> {
    let v = makepad_strict_json::parse(detail.as_bytes()).ok()?;
    if v.get("expected").is_some() { return None; }
    let id = v.get("id").and_then(|i| i.as_str()).filter(|i| !i.is_empty())?.to_string();
    Some((id, line.strip_prefix("panic: ").unwrap_or(line).to_string()))
}

/// This node is a bench box (`role = bench`): timed work needs it quiet, so it runs nothing of
/// the hub's own outside a lease (no pre-warm).
pub fn bench_box() -> bool {
    EXEC.get().is_some_and(|x| x.conf.read().unwrap().role == super::Role::Bench)
}

/// A person used this node (input its WM took from a device or a Desk Link viewer) in the last
/// `ms`, as far as this hub has seen.
pub fn person_within(ms: u64) -> bool {
    EXEC.get().is_some_and(|x| x.user.lock().unwrap().last_input_ms().is_some_and(|at| crate::proto::wall_ms().saturating_sub(at) < ms))
}

/// A person uses this box, for what is said to them on it (a sync's toasts): its role is a user's
/// WM, or its WM took a person's input in the last `ms` ([`person_within`]).
pub fn person_box(ms: u64) -> bool {
    EXEC.get().is_some_and(|x| x.conf.read().unwrap().role == super::Role::UserWm) || person_within(ms)
}

/// Someone watches this node's screen ([`super::user::UserTrack::watched`]): what they would see
/// (a WM restart, its AI pane restarting) waits; it takes no box out of the dev pool.
pub fn user_watching() -> bool {
    EXEC.get().is_some_and(|x| x.user.lock().unwrap().watched())
}

/// A Desk Link viewer shows this node now, input or not ([`super::user::UserTrack::shown`]).
pub fn viewer_shows() -> bool {
    EXEC.get().is_some_and(|x| x.user.lock().unwrap().shown())
}

/// [`user_watching`], said as what a restart waits on.
pub fn watched_said() -> Option<String> {
    user_watching().then(|| format!("someone watches this node's screen (a Desk Link viewer showing it, or its own devices; input within {} min)", super::user::VIEWER_HOLDS_MIN))
}

/// Runs `f` while no lease starts here: a WM restart's last look at what it would end and the
/// restart itself see the same leases (a live session that started after the wait's look, while
/// the hub built, went down with the WM, 11-03).
pub fn with_no_lease_starting<T>(f: impl FnOnce() -> T) -> T {
    let Some(x) = EXEC.get() else { return f() };
    let _starting = x.starting.lock().unwrap_or_else(|e| e.into_inner());
    f()
}

/// What keeps a user-facing service of this node (the WM, the AI hub) from restarting now:
/// unsaved work in its apps. Empty: it may restart. The hub itself restarts regardless (invisible
/// to the user). Someone watching its screen holds it too ([`user_watching`], said by the caller).
pub fn user_blockers() -> Vec<String> {
    #[cfg(all(unix, feature = "checkout"))]
    return makepad_app_registry::machine::unsaved_work().into_iter().map(|(pid, app, what)| format!("unsaved work in {app} (pid {pid}): {what}")).collect();
    #[cfg(not(all(unix, feature = "checkout")))]
    Vec::new()
}

/// Whether app process `pid` has unsaved work ([`user_blockers`]).
pub fn unsaved_in(pid: u32) -> bool {
    #[cfg(all(unix, feature = "checkout"))]
    return makepad_app_registry::machine::unsaved_work().into_iter().any(|(p, _, _)| p == pid);
    #[cfg(not(all(unix, feature = "checkout")))]
    { let _ = pid; false }
}

/// A dev box: its dev switch on and its role no user's ([`super::Role::users`]). It always runs
/// the latest (the user, 11-22: "yes all dev linux boxes can always update to latest"): its WM
/// and its apps restart for a landing whatever a person has open there ([`crate::sync::serve`]).
pub fn dev_box() -> bool {
    EXEC.get().is_some_and(|x| !x.conf.read().unwrap().role.users()) && writes_allowed()
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

/// The leases here whose work a WM restart would cut: those on the WM, a screen or CI lease or
/// one given the WM session (`needs.gui`: its env names the WM socket). Their lines, for the WM
/// restart's wait. A headless lease runs on through it (its runs live in the hub's scopes, not the
/// WM's session), a landing's gate, tests and hot runs among them: a node's old-wire WM waited on
/// live test sessions whose apps could not have used it (11-02), and on a busy pool box one
/// landing's shard followed another, so its WM ran a replaced binary, its devhub tray slot an old
/// glyph, for as long as landings came (11-08).
pub fn on_the_wm() -> Vec<String> {
    let Some(x) = EXEC.get() else { return Vec::new() };
    x.leases.lock().unwrap().values().filter(|l| uses_wm(l)).map(|l| {
        let what = if l.runs.iter().any(|r| r.detachable && r.live()) { "a live session" } else { "its windows" };
        format!("{} lease {} of {} ({what})", l.kind.as_str(), l.id, l.lane)
    }).collect()
}

fn uses_wm(l: &Lease) -> bool {
    matches!(l.kind, Kind::Screen | Kind::Ci) || l.env.iter().any(|(k, _)| k == "MAKEPAD_WM_SOCKET")
}

/// The processes of lease `id`'s runs: their process groups' (Windows: their jobs'), and every
/// one that carries the lease's mark ([`lease_mark`]) wherever it runs: what a run started in a
/// group or session of its own (a JIT-run test leads a group of its own) is the lease's too.
fn lease_pids(x: &Exec, id: &str, runs: &[Arc<Run>]) -> Vec<u32> {
    let (mark, me) = (lease_mark(&x.state, id), std::process::id());
    let mut pids: Vec<u32> = runs.iter().filter(|r| r.pid != 0).flat_map(|r| os::group_pids(r.pid, r.start, &r.hold))
        .chain(os::with_env(&format!("{LEASE_ENV}={mark}")))
        .chain(os::with_env(&format!("{LEASE_OUTER_ENV}={mark}")))
        .filter(|p| *p != me)
        .collect();
    // a gate's unit and what it started, in the session its watch names, wherever they went: a stall's
    // capture listed only the shells and the watch of a unit that had left them (L1483898, 11-21)
    let units: Vec<u32> = os::watched_sessions(&pids).into_iter().flat_map(|(_, live)| live).collect();
    pids.extend(units);
    pids.sort_unstable();
    pids.dedup();
    pids
}

/// The JITs announced on this box (pid, app) that are among `pids`.
fn jits_among(pids: &[u32]) -> Vec<(u32, String)> {
    // (a hub built without checkouts keeps no JIT registry: it runs no JITs)
    #[cfg(feature = "checkout")]
    let jits: Vec<(u32, String)> = crate::sync::jits::list().into_iter().map(|j| (j.pid, j.app)).collect();
    #[cfg(not(feature = "checkout"))]
    let jits: Vec<(u32, String)> = Vec::new();
    jits.into_iter().filter(|(pid, _)| pids.contains(pid)).collect()
}

/// `lease-jits <id>`: the JITs among the lease's processes ([`lease_pids`]), each with its app: a
/// stalled lease's master asks each to capture its state, and `devhub lease <id> jits` names them
/// for `inspect <node>/<pid> why-hung`. JITs were matched by the runs' process groups alone, and a
/// JIT-run test leads a group of its own (relax test_pkg.rs): no hang watch or sampler reached a
/// hung test's process from its lease (an ai-vulkan run hung 35 min, 11-13).
fn lease_jits_of(x: &Exec, id: &str) -> String {
    // no process walks under the lease table lock
    let Some(runs) = x.leases.lock().unwrap().get(id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    let jits = jits_among(&lease_pids(x, id, &runs));
    format!("{{\"pids\":[{}],\"jits\":[{}]}}", jits.iter().map(|(p, _)| p.to_string()).collect::<Vec<_>>().join(","), jits.iter().map(|(p, app)| format!("{{\"pid\":{p},\"app\":{}}}", json_str(app))).collect::<Vec<_>>().join(","))
}

/// `lease-yield <id> <why>`: the lease's `$DEVHUB_YIELD` file, why in it ([`crate::lease::YIELD_FILE`]).
fn lease_yield(x: &Exec, rest: &str) -> String {
    let (id, why) = rest.split_once(' ').unwrap_or((rest, ""));
    let Some(tmp) = x.leases.lock().unwrap().get(id).map(|l| l.tmp.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    let file = tmp.join(crate::lease::YIELD_FILE);
    match std::fs::write(&file, format!("{why}\n")) {
        Ok(()) => "{}".into(),
        Err(e) => err(format!("{}: {e}", file.display())),
    }
}

/// `lease-tell <id> <line>`: the line appended to the lease's `$DEVHUB_TOLD` file
/// ([`crate::lease::TOLD_FILE`]), whole: its run reads a line once it ends in a newline.
fn lease_tell(x: &Exec, rest: &str) -> String {
    let (id, line) = rest.split_once(' ').unwrap_or((rest, ""));
    let Some(tmp) = x.leases.lock().unwrap().get(id).map(|l| l.tmp.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    let file = tmp.join(crate::lease::TOLD_FILE);
    let told = std::fs::OpenOptions::new().create(true).append(true).open(&file).and_then(|mut f| std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes()));
    match told {
        Ok(()) => "{}".into(),
        Err(e) => err(format!("{}: {e}", file.display())),
    }
}

/// `lease-kill <id> <pid>`: KILL one process of the lease's ([`lease_pids`]: its runs' groups and
/// what carries its mark), by that pid: a live session's JIT restart and stop. An overlay lease
/// runs only the JIT's cargo invocation, so a `kill` run there was refused and the session stayed
/// "recovering" (11-02). A JIT-run test leads a group of its own (relax test_pkg.rs): matched by
/// the runs' groups alone, a live test session's JIT was no process of its lease, which then could
/// neither restart nor stop it (11-14).
fn lease_kill(x: &Exec, rest: &str) -> String {
    let mut w = rest.split_whitespace();
    let (Some(id), Some(pid)) = (w.next(), w.next().and_then(|p| p.parse::<u32>().ok())) else { return err("lease-kill <id> <pid>") };
    // no process walks under the lease table lock
    let Some(runs) = x.leases.lock().unwrap().get(id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    if !lease_pids(x, id, &runs).contains(&pid) {
        return err(format!("pid {pid} is no process of lease {id} on {}", x.machine));
    }
    os::signal_one(pid, true);
    format!("{{\"killed\":{pid}}}")
}

/// `lease-why <id>`: what a lease's processes do now and how far its last run got, on that lease:
/// a second lease on its tree waits for this very one (three capture leases did, 11-05). With
/// `stuck`, where they wait ([`os::stuck_in`]: its command chain, pid and seconds), and `step`, its
/// last run's last `STEP ` line (a gate's last step done, a CI leg's step running): what a hung
/// leg's end names (lease/master.rs `capture`).
fn lease_why(x: &Exec, id: &str) -> String {
    // no process walks under the lease table lock
    let Some(runs) = x.leases.lock().unwrap().get(id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    let pids = lease_pids(x, id, &runs);
    let mut got = runs.last().map_or("no run yet\n".to_string(), |r| got_to(r, WHY_LINES));
    let jits = jits_among(&pids);
    if !jits.is_empty() {
        got.push_str(&format!("its JITs (`devhub inspect {}/<pid> why-hung`): {}\n", x.machine, jits.iter().map(|(p, app)| format!("{p} {app}")).collect::<Vec<_>>().join(", ")));
    }
    // a unit its watch still watches, ended: what waits on it waits on nothing (a hung unit's processes live)
    for (sid, _) in os::watched_sessions(&pids).into_iter().filter(|(_, live)| live.is_empty()) {
        got.push_str(&format!("its unit {sid} has ended (no process of its session is left), and its watch still runs\n"));
    }
    let stuck = os::stuck_in(&pids).map_or("null".into(), |(chain, pid, s)| format!("{{\"chain\":{},\"pid\":{pid},\"s\":{s}}}", json_str(&chain)));
    let step = runs.last().and_then(|r| log_tail(&r.log, usize::MAX).lines().rev().find(|l| l.starts_with("STEP ")).map(json_str));
    format!("{{\"text\":{},\"stuck\":{stuck},\"step\":{}}}", json_str(&format!("{}{got}", os::processes_view(&pids))), step.unwrap_or_else(|| "null".into()))
}

/// `lease-failed <id>`: the lease's runs that ended with a failure (an exit other than 0, not
/// stopped on purpose), each with its log's size: the master reads each whole (`lease-log`) into
/// the lease's kept log before it ends the lease. The release answer's failed run was its last
/// 64 KB, and a 26-min makepad-fab run kept a tail that named none of its failing targets (11-20).
fn lease_failed(x: &Exec, id: &str) -> String {
    let id = id.trim();
    let Some(runs) = x.leases.lock().unwrap().get(id).map(|l| l.runs.clone()) else { return err(format!("no lease {id} on {}", x.machine)) };
    let failed: Vec<String> = runs.iter().filter_map(|r| {
        r.adopted_exit();
        let exit = (*r.exit.lock().unwrap())?;
        (exit != 0 && exit != -2 && !r.stopped.load(Ordering::Acquire)).then(|| {
            let bytes = std::fs::metadata(&r.log).map_or(0, |m| m.len());
            format!("{{\"n\":{},\"exit\":{exit},\"bytes\":{bytes}}}", r.n)
        })
    }).collect();
    format!("{{\"runs\":[{}]}}", failed.join(","))
}

/// `lease-pid <id> <pid> [start]`: whether `pid` still runs on this box ([`pid_runs`]), with its
/// start. A live session's master asks it each beat for the session's process: a test process
/// that had gone while its run went on read "still running: tests" for 52 min (W11754, 11-17).
fn lease_pid(x: &Exec, rest: &str) -> String {
    let mut w = rest.split_whitespace();
    let (Some(id), Some(pid)) = (w.next(), w.next().and_then(|p| p.parse::<u32>().ok())) else { return err("lease-pid <id> <pid> [start]") };
    if !x.leases.lock().unwrap().contains_key(id) { return err(format!("no lease {id} on {}", x.machine)); }
    let start = pid_runs(pid, w.next().and_then(|s| s.parse().ok()));
    format!("{{\"alive\":{},\"start\":{}}}", start.is_some(), start.unwrap_or(0))
}

/// `pid` runs here (not ended, no zombie): its start ([`os::proc_start`]); given `start`, only
/// while it is that process (a pid used again is another one).
pub(crate) fn pid_runs(pid: u32, start: Option<u64>) -> Option<u64> {
    if pid == 0 { return None; }
    match start {
        Some(s) => os::same(pid, s).then_some(s),
        None => os::proc_start(pid).filter(|s| os::same(pid, *s)),
    }
}

/// Whose a JIT app on this node is, as far as the leases here go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppLease {
    /// a live lease here runs it; `on_wm`: that lease shows windows ([`uses_wm`])
    Live { on_wm: bool },
    /// it carries this hub's mark of a lease that has ended here: a leftover no one waits for
    Left,
    /// no lease of this hub started it: a person's
    Nobody,
}

/// Whose `pid` (a JIT app) is: a live lease's when a run of that lease holds it in its process
/// group, or when it carries the lease's mark ([`lease_mark`]) after it moved to a session of
/// its own. Leases were matched by process group alone, so an app that a lease's run started in
/// a new session read as a person's. WM restarts on two busy boxes then waited for it without a
/// drain, while new screen leases kept arriving (11-07).
pub fn app_lease(pid: u32) -> AppLease {
    EXEC.get().map_or(AppLease::Nobody, |x| x.app_lease(pid))
}

impl Exec {
    fn app_lease(&self, pid: u32) -> AppLease {
        // no process walks under the lease table lock
        let leases: Vec<(String, bool, Vec<Arc<Run>>)> = self.leases.lock().unwrap().values().map(|l| (l.id.clone(), uses_wm(l), l.runs.clone())).collect();
        if let Some((_, on_wm, _)) = leases.iter().find(|(_, _, runs)| runs.iter().any(|r| r.pid != 0 && os::in_group(pid, r.pid, &r.hold))) {
            return AppLease::Live { on_wm: *on_wm };
        }
        let Some(id) = marked(&self.state, pid) else { return AppLease::Nobody };
        leases.iter().find(|(l, _, _)| *l == id).map_or(AppLease::Left, |(_, on_wm, _)| AppLease::Live { on_wm: *on_wm })
    }
}

/// The lease of this hub's ledger (`state`) whose mark ([`lease_mark`]) `pid` carries.
#[cfg(target_os = "linux")]
fn marked(state: &Path, pid: u32) -> Option<String> {
    let mark = format!("{LEASE_ENV}={}/", state.display());
    let env = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    env.split(|b| *b == 0).find_map(|v| v.strip_prefix(mark.as_bytes())).map(|id| String::from_utf8_lossy(id).into_owned())
}

/// (only Linux boxes have a WM to restart)
#[cfg(not(target_os = "linux"))]
fn marked(_state: &Path, _pid: u32) -> Option<String> {
    None
}

/// Whether `pid` (a JIT app) runs in a headless lease here, which a WM restart leaves alone: a
/// lease not on the WM ([`on_the_wm`]) runs it ([`app_lease`]), and it has no WM socket in its
/// environment. A launch (a run whose app has the WM's socket), or an app a shell started, gets
/// no such pass.
pub fn in_headless_lease(pid: u32) -> bool {
    app_lease(pid) == AppLease::Live { on_wm: false } && !wm_in_env(pid)
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

/// A sync launch's command as the next run of live lease `id` here ([`Exec::launch`]).
pub fn launch(id: &str, cmd: &std::process::Command, log: &Path, ended: Box<dyn FnOnce(u32, i32) + Send>) -> Result<Launched, String> {
    EXEC.get().ok_or("this process runs no lease executor")?.launch(id, cmd, log, ended)
}

/// The launch run led by `leader` is stopped on purpose, on its own (`sync::launch::stop`): how it
/// ends is no abnormal end of its lease.
pub fn launch_stopped(leader: u32) {
    if let Some(x) = EXEC.get() { x.launch_stopped(leader); }
}

/// The lane lease `id` belongs to.
pub fn lease_lane(id: &str) -> Option<String> {
    EXEC.get().and_then(|x| x.leases.lock().unwrap().get(id).map(|l| l.lane.clone()))
}

/// What this hub stopped in the last minute. A JIT it names that ends then was stopped, not
/// crashed: a lease's end stops what runs in the lease (a CI box's calculator, hosted by a CI
/// smoke's WM, "crashed" as its lease ended, 11-03), each JIT's discovery file naming its lease
/// mark, and on Windows what is in its jobs too; a prewarm's stop the JIT it started, by its
/// socket (each landing's prewarm of stage "crashed" on three Linux nodes, 11-04).
static STOPPED: Mutex<Vec<Stopped>> = Mutex::new(Vec::new());
const STOPPED_FOR_MS: u64 = 60_000;

struct Stopped {
    /// `with its lease L1`, `by prewarm`
    by: String,
    /// the socket of the JIT it stopped
    socket: Option<PathBuf>,
    /// the lease mark ([`lease_mark`]) of what it stopped
    lease: Option<String>,
    /// the processes it stopped that nothing else names
    pids: Vec<u32>,
    at: u64,
}

fn note_stopping(id: &str, mark: &str, pids: Vec<u32>) {
    note(Stopped { by: format!("with its lease {id}"), socket: None, lease: Some(mark.to_string()), pids, at: 0 });
}

/// The processes in the runs' jobs (Windows: what a lease's end stops beside what its mark names).
#[cfg(windows)]
fn job_pids(runs: &[Arc<Run>]) -> Vec<u32> {
    runs.iter().flat_map(|r| os::job_pids(&r.hold)).collect()
}

/// Elsewhere each JIT's discovery file names its lease: the lease's mark names them all.
#[cfg(not(windows))]
fn job_pids(_runs: &[Arc<Run>]) -> Vec<u32> {
    Vec::new()
}

/// Says that `by` (`by prewarm`) stops the JIT whose socket is `socket`.
pub fn note_stopped(by: &str, socket: &Path) {
    note(Stopped { by: by.to_string(), socket: Some(socket.to_path_buf()), lease: None, pids: Vec::new(), at: 0 });
}

/// A WM coming up has no size until its output answers: how long that may last before it is said.
const DARK_SAY_MS: u64 = 60_000;

/// What a hub has said of its WM's size. A WM that came back from a restart with a 0x0 window took
/// screen leases and ended each on its first frame, and nothing said why (11-09).
#[derive(Default)]
struct DarkSaid {
    since_ms: u64,
    said: bool,
}

impl DarkSaid {
    /// `dark`: this measure's [`caps::dark_of`]. The line to say, if any: once it has stayed so for
    /// [`DARK_SAY_MS`], and once it shows again.
    fn note(&mut self, machine: &str, dark: &str, now_ms: u64) -> Option<(Sev, String)> {
        if dark.is_empty() {
            self.since_ms = 0;
            return std::mem::take(&mut self.said).then(|| (Sev::Info, format!("hub: {machine} shows windows again: its WM has a size, and it takes screen and gui leases")));
        }
        if self.since_ms == 0 {
            self.since_ms = now_ms;
        }
        if self.said || now_ms.saturating_sub(self.since_ms) < DARK_SAY_MS {
            return None;
        }
        self.said = true;
        Some((Sev::Warn, format!("hub: {machine} shows no windows: {dark}. It takes no screen or gui lease until its WM has a size")))
    }
}

/// Says that `by` (`by the WM restart`) stops the processes `pids`.
pub fn note_stopped_pids(by: &str, pids: Vec<u32>) {
    note(Stopped { by: by.to_string(), socket: None, lease: None, pids, at: 0 });
}

fn note(mut s: Stopped) {
    s.at = crate::proto::wall_ms();
    let mut stopped = STOPPED.lock().unwrap();
    stopped.retain(|x| s.at.saturating_sub(x.at) < STOPPED_FOR_MS);
    stopped.push(s);
}

/// What stopped the JIT `pid` of lease mark `lease` (its discovery file's) and socket `socket`
/// ([`note_stopped`], a lease's end), when it ended in the last minute: `with its lease L1`, `by
/// prewarm`.
pub fn stopped_with(pid: u32, lease: Option<&str>, socket: &str) -> Option<String> {
    let now = crate::proto::wall_ms();
    let stopped = STOPPED.lock().unwrap();
    stopped.iter().rev()
        .find(|s| now.saturating_sub(s.at) < STOPPED_FOR_MS && ((lease.is_some() && s.lease.as_deref() == lease) || (!socket.is_empty() && s.socket.as_deref() == Some(Path::new(socket))) || s.pids.contains(&pid)))
        .map(|s| s.by.clone())
}

/// Whether lease mark `lease` (a JIT's, its discovery file's) is that of a live test or CI lease
/// of this hub.
pub fn in_test_lease(lease: Option<&str>) -> bool {
    let Some(x) = EXEC.get() else { return false };
    let Some(id) = lease.and_then(|m| mark_id(&x.state, m)) else { return false };
    x.leases.lock().unwrap().get(id).is_some_and(|l| matches!(l.kind, Kind::Test | Kind::Ci))
}

/// The lease of mark `mark` when that is one of this hub's (its ledger dir `state`): a hub run
/// in a lease marks its own runs with the same ids.
fn mark_id<'a>(state: &Path, mark: &'a str) -> Option<&'a str> {
    let (_, id) = mark.rsplit_once('/')?;
    (lease_mark(state, id) == mark).then_some(id)
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

/// How often [`watch_deadlocks`] looks, and how long a deadlock it ends on has lasted at least.
const DEADLOCK_LOOK: Duration = Duration::from_secs(20);
const DEADLOCK_WAIT: Duration = Duration::from_secs(60);

/// The exit code of a hub that ended itself on a deadlock: not a clean exit, so its supervisor
/// (the WM, a Mac's login item, a Windows box's logon task) starts it again.
pub const DEADLOCKED_EXIT: i32 = 70;

/// A hub whose locks deadlock ([`crate::held::deadlocks`]) never frees by itself, and stopping it
/// from outside waits too: its self-update and its TERM drain ([`drain_and_exit`]) wait on the
/// lease start lock. Two pool boxes' hubs sat so, every lease start and end there waiting, until
/// each was stopped by hand (11-18). The same deadlock on two looks, past [`DEADLOCK_WAIT`], ends
/// this process with [`DEADLOCKED_EXIT`]: its supervisor starts the hub again, which takes its
/// leases back from the ledger. This thread takes none of the hub's locks.
fn watch_deadlocks(x: Weak<Exec>) {
    let _ = std::thread::Builder::new().name("devhub-deadlock-watch".into()).spawn(move || {
        let mut before = Vec::new();
        loop {
            std::thread::sleep(DEADLOCK_LOOK);
            if x.strong_count() == 0 {
                return;
            }
            let now = crate::held::deadlocks(DEADLOCK_WAIT);
            if let Some(line) = crate::held::lasting(&before, &now) {
                let line = format!("hub: deadlocked, it exits with code {DEADLOCKED_EXIT} for its supervisor to start it again: {line}");
                eprintln!("devhub: {line}");
                // the event goes on a thread of its own: what sends it could wait on the deadlock too
                let (tx, rx) = std::sync::mpsc::channel();
                if let Some(x) = x.upgrade() {
                    let _ = std::thread::Builder::new().name("devhub-deadlock-said".into()).spawn(move || {
                        x.emit("hub", Sev::Err, format!("{line} (on {})", x.machine));
                        let _ = tx.send(());
                    });
                }
                let _ = rx.recv_timeout(Duration::from_secs(2));
                // (and a moment for the stream to carry it to the master)
                std::thread::sleep(Duration::from_millis(500));
                std::process::exit(DEADLOCKED_EXIT);
            }
            before = now;
        }
    });
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
    watch_deadlocks(Arc::downgrade(&x));
    // who watches the box's screen ([`super::user`]): our WM's answer every 2 s, a Mac's HID idle
    // time (a node with neither senses nothing)
    let w = Arc::downgrade(&x);
    let _ = std::thread::Builder::new().name("devhub-user".into()).spawn(move || loop {
        let Some(x) = w.upgrade() else { return };
        #[cfg(target_os = "macos")]
        let answer = super::user::hid_idle_ms().map(super::user::hid_answer).ok_or_else(|| "no HID idle time".to_string());
        #[cfg(all(unix, not(target_os = "macos"), feature = "checkout"))]
        let answer = makepad_app_registry::machine::wm_socket().ok_or_else(|| "no WM".to_string()).and_then(|p| makepad_app_registry::machine::request_activity(&p));
        #[cfg(not(any(target_os = "macos", all(unix, feature = "checkout"))))]
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

/// The process group of the hub build running on this box (0: none; [`measured_build`] makes it
/// one of its own): its processes are its hub-build lease's in the box's CPU split, by the name
/// [`super::sched::HUB_BUILD`], never work of no lease's (a health section read every roll's
/// build as stray load on four boxes at once, 11-09).
static HUB_BUILD_GROUP: AtomicU32 = AtomicU32::new(0);

/// The process groups of the hub's own build and its deployment's build running now (each with its
/// leader's start), the work of the master's leases of their rooms ([`super::sched::HUB_BUILD`],
/// [`super::sched::DEPLOY_BUILD`]).
fn build_groups() -> Vec<(u32, u64)> {
    [&HUB_BUILD_GROUP, &DEPLOY_BUILD_GROUP].iter().map(|g| g.load(Ordering::Acquire)).filter(|g| *g != 0).map(|g| (g, os::proc_start(g).unwrap_or(0))).collect()
}

/// What the hub's own build and its deployment's build hold now, bytes ([`os::groups_held`]).
/// They are no residents: their rooms' leases charge them. Counted in the residents too, a
/// 16 GB Mac's residents went from 5.5 to 7.6 GB as its hub build started, beside that build's
/// 4.9 GB charge, and a session that had run there waited on both (W12708, 11-18). None: neither
/// runs, or the OS cannot tell.
fn builds_held() -> Option<u64> {
    let groups = build_groups();
    if groups.is_empty() { return None; }
    os::groups_held(&groups)
}

/// The process group of the deployment's prebuild step running on this box (0: none; the sync
/// apply's steps each run in one of their own): its deployment-build room's in the box's CPU split,
/// by the name [`super::sched::DEPLOY_BUILD`].
static DEPLOY_BUILD_GROUP: AtomicU32 = AtomicU32::new(0);

/// Names `group` the deployment's prebuild step now running ([`DEPLOY_BUILD_GROUP`]); 0 once it ended.
#[cfg(feature = "checkout")]
pub(crate) fn deploy_build_group(group: u32) {
    DEPLOY_BUILD_GROUP.store(group, Ordering::Release);
}

/// Whether the hub builds itself now (its caps say so: no timed lease starts beside it; its
/// cores are the self-update's lease of its class, [`super::sched::Req::hub_build`]).
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
pub(crate) fn build_hub(root: &Path, jobs: Option<u32>) -> Result<(), String> {
    #[cfg(windows)]
    let aside = image_aside();
    let mut cmd = os::background(makepad_app_registry::cargo_bin().as_os_str());
    // (rustup counts nested proxies in RUST_RECURSION_COUNT and refuses past 20)
    cmd.args(["build", "--release", "-p", "makepad-app-devhub"]);
    // the cores its room was granted, when fewer than cargo would take
    if let Some(j) = jobs { cmd.arg("-j").arg(j.to_string()); }
    cmd.current_dir(root).env_remove("RUST_RECURSION_COUNT").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
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
    HUB_BUILD_GROUP.store(pid, Ordering::Release);
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
    HUB_BUILD_GROUP.store(0, Ordering::Release);
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

/// One line in the systemd journal as `devhub-hub`, by journald's native socket (no process to
/// start; a box without journald drops it).
#[cfg(target_os = "linux")]
fn journal(line: &str) {
    let Ok(socket) = std::os::unix::net::UnixDatagram::unbound() else { return };
    let message = format!("SYSLOG_IDENTIFIER=devhub-hub\nPRIORITY=6\nMESSAGE={}\n", line.replace('\n', " "));
    let _ = socket.send_to(message.as_bytes(), "/run/systemd/journal/socket");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_refuse_unread_fields_before_doing_work() {
        let (x, base, _) = test_exec("closed-commands", Opts::default());
        let mut sched = super::super::sched::Sched::new("box-a", 0);
        let mut req = super::super::sched::Req::new("lane-a", Kind::Build);
        req.warms = Some("lane-b".into());
        req.cpu_target = Some("generic".into());
        req.loader = Some("loader-id".into());
        req.ram_gb = Some(4.0);
        req.live = true;
        req.gpu = true;
        req.needs.push(("gui".into(), "1".into()));
        let id = sched.request(req, 0);
        // (a build admitted on fewer cores than it measures is told its jobs)
        let mut l = sched.get(&id).unwrap().clone();
        l.jit_jobs = Some(3);
        // (a timed start past its wait for a quiet box hears what it starts beside)
        l.unquiet = Some("box: 67% of a core runs outside its leases".into());
        let line = super::super::master::lease_start_line(&l, "null", l.run_cores(), 2.0, Some(3.0), Some(4), Some(5));
        let json = line.strip_prefix("lease-start ").unwrap();
        let v = makepad_strict_json::parse(json.as_bytes()).unwrap();
        let makepad_strict_json::Value::Obj(fields) = &v else { panic!("start is an object") };
        for (k, _) in fields { assert!(START_KEYS.contains(&k.as_str()), "master sends accepted start field {k}"); }
        for k in START_KEYS { assert!(v.get(k).is_some(), "master exercises start field {k}"); }
        let cost = v.get("cost").unwrap();
        let makepad_strict_json::Value::Obj(fields) = cost else { panic!("cost is an object") };
        for (k, _) in fields { assert!(START_COST_KEYS.contains(&k.as_str()), "master sends accepted cost field {k}"); }
        for k in START_COST_KEYS { assert!(cost.get(k).is_some(), "master exercises cost field {k}"); }
        let extra = format!("{},\"bogus\":1}}", json.strip_suffix('}').unwrap());
        let answer = x.start(&extra);
        assert!(answer.contains(&format!("lease-start {id}:")) && answer.contains("field `bogus`"), "{answer}");
        let answer = x.start(&json.replace("\"cost\":{", "\"cost\":{\"bogus\":1,"));
        assert!(answer.contains("field `bogus`"), "{answer}");
        let answer = x.run(&format!("{id} . ZWNobw== {{\"bogus\":true}}"));
        assert!(answer.contains(&format!("lease-run {id}:")) && answer.contains("option `bogus`"), "{answer}");
        for word in ["jobz=3", "jobs=0", "jobs=-1", "jobs=no", "jobs=4294967296"] {
            let answer = x.hub_restart(&format!("{} jobs=2 {word}", "a".repeat(40)));
            assert!(answer.contains("\"err\"") && answer.contains(&format!("`{word}`")), "{answer}");
        }
        assert!(x.leases.lock().unwrap().is_empty(), "refused commands start no lease");
        assert!(!x.restarting.load(Ordering::Acquire), "refused words do not restart the hub");
        drop(x);
        let _ = std::fs::remove_dir_all(base);
    }

    /// A timed run's hold on the box's AI hub, as the hub's log says it: held, an older hub that
    /// cannot hold, one not told; at its end what it refused, or the next timed run it holds for.
    #[test]
    fn a_timed_runs_quiet_on_the_ai_hub_is_said_with_its_holder_and_what_it_refused() {
        let v = |s: &str| Ok(Some(makepad_strict_json::parse(s.as_bytes()).unwrap()));
        let holder = "timed lease L1 of lanea";
        assert_eq!(quiet_held_said(holder, "boxa", Ok(None)), None, "no AI hub here, or none that serves the network");
        assert_eq!(quiet_held_said(holder, "boxa", v(r#"{"reason":"timed-lease","admission_open":false}"#)), Some((Sev::Info, "hub: timed lease L1 of lanea holds boxa: its AI hub takes no remote work until it ends".to_string())));
        let (sev, line) = quiet_held_said(holder, "boxa", v(r#"{"reason":"quiet","admission_open":true}"#)).unwrap();
        assert!(sev == Sev::Warn && line.contains("does not hold quiet (it predates it)"), "{line}");
        let (sev, line) = quiet_held_said(holder, "boxa", Err("the hub answered 403".into())).unwrap();
        assert!(sev == Sev::Warn && line.contains("not told to hold quiet (the hub answered 403)"), "{line}");
        let over = quiet_over_said(holder, "boxa", "", v(r#"{"reason":"releasing-models","refused":3,"refused_said":["GET /kernel from client-a","kernel stream from client-a"]}"#));
        assert_eq!(over, Some((Sev::Info, "hub: timed lease L1 of lanea ended on boxa: its AI hub takes remote work again; it refused 3 request(s) meanwhile: GET /kernel from client-a; kernel stream from client-a".to_string())));
        let over = quiet_over_said(holder, "boxa", "timed lease L2 of laneb", v(r#"{"reason":"timed-lease"}"#)).unwrap();
        assert_eq!(over.1, "hub: timed lease L1 of lanea ended on boxa: its AI hub holds quiet for timed lease L2 of laneb; it refused no request meanwhile");
        assert_eq!(quiet_over_said(holder, "boxa", "", Ok(None)), None);
    }

    /// An AI hub that came back (or up) under a timed run is asked to hold quiet again, said with
    /// its old and new pid; one not answering yet is asked at the next look, unsaid.
    #[test]
    fn an_ai_hub_back_under_a_timed_run_is_asked_again_and_said() {
        let v = |s: &str| Ok(Some(makepad_strict_json::parse(s.as_bytes()).unwrap()));
        let holder = "timed lease L1 of lanea";
        assert_eq!(quiet_again_said(holder, "boxa", Some(41), 77, v(r#"{"reason":"timed-lease"}"#)), Some((Sev::Info, "hub: timed lease L1 of lanea holds boxa: its AI hub came back (pid 41 -> 77), asked again: it takes no remote work until the run ends".to_string())));
        let (sev, line) = quiet_again_said(holder, "boxa", None, 77, v(r#"{"reason":"idle"}"#)).unwrap();
        assert!(sev == Sev::Warn && line.contains("its AI hub came up (pid 77) and does not hold quiet"), "{line}");
        let (sev, line) = quiet_again_said(holder, "boxa", Some(41), 77, Err("the hub answered 500".into())).unwrap();
        assert!(sev == Sev::Warn && line.contains("came back (pid 41 -> 77) and was not told to hold quiet (the hub answered 500)"), "{line}");
        assert_eq!(quiet_again_said(holder, "boxa", Some(41), 77, Ok(None)), None, "not answering yet: asked at the next look");
    }

    /// Every word the node's lease service answers is one its endpoint routes ([`WORDS`]): one
    /// left out was answered in tests only, never over the wire (`lease-weight`: no live call was
    /// weighed or metered, `devhub stats calls` empty for a day of live sessions, 11-07).
    #[test]
    fn every_word_the_lease_service_answers_is_routed() {
        let src = include_str!("exec.rs");
        let body = &src[src.find("pub fn handle(&self, word: &str, line: &str)").unwrap()..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        let words: Vec<&str> = body.lines().map(str::trim_start).filter(|l| l.starts_with('"')).filter_map(|l| l.split(" =>").next())
            .flat_map(|arm| arm.split('|').filter_map(|w| w.trim().split('"').nth(1))).collect();
        assert!(words.len() > 25 && words.contains(&"lease-weight"), "{words:?}");
        for w in words {
            assert!(WORDS.contains(&w), "`{w}` is answered but not routed");
        }
    }

    /// Who set a node's lease.conf comes with the pairs, never as a key of its own.
    #[test]
    fn a_conf_set_says_who_asked_apart_from_its_keys() {
        assert_eq!(conf_set_args("role=pool slots.timed=0 by=MAIN".split_whitespace()), (vec![("role".into(), "pool".into()), ("slots.timed".into(), "0".into())], Some("MAIN".into())));
        assert_eq!(conf_set_args("role=bench".split_whitespace()), (vec![("role".into(), "bench".into())], None));
    }

    /// A timed lease's box report: the outside CPU's mean and peak over the looks, each look
    /// weighed by its span, the busiest programs outside it by their mean with what their
    /// waited-for children took, the hub's and what no process showed. Its looks count from its
    /// start: the first span begins there when no other timed lease's looks run, and a span begun
    /// before it is not its own; with no timed lease the span ends.
    #[cfg(unix)]
    #[test]
    fn a_timed_box_report_names_what_shared_it() {
        let o = |load1: f64, secs: f64, by: &[(&str, f64, f64)], hub: f64, unseen: Option<(f64, f64)>| crate::top::Outside { secs, load1, by: by.iter().map(|(n, c, k)| (n.to_string(), *c, *k)).collect(), hub, unseen };
        let line = |n: &Noise| super::super::master::box_line(&makepad_strict_json::parse(n.json().as_bytes()).unwrap());
        let mut n = Noise::default();
        n.add(&o(20.0, 10.0, &[("mds_stores", 180.0, 0.0), ("XprotectService", 40.0, 0.0)], 2.0, None));
        n.add(&o(27.0, 10.0, &[("mds_stores", 60.0, 0.0)], 4.0, None));
        assert_eq!(line(&n), "load1 max 27.0; outside it 140% of a core (max 220%): mds_stores 120%, XprotectService 20%; its hub 3%");
        // a GPU box's floor (11-22): its AI hub's nvidia-smi runs are its children's part, and a
        // look of 30 s weighs three of 10 s
        let mut l = Noise::default();
        let aihub = "makepad-app-aihub (under makepad-app-wm)";
        l.add(&o(2.0, 10.0, &[(aihub, 17.0, 13.0), ("kcompactd0", 5.0, 0.0)], 0.7, Some((4.0, 4.0))));
        l.add(&o(1.0, 30.0, &[(aihub, 17.0, 13.0)], 0.7, Some((8.0, 4.0))));
        assert_eq!(line(&l), format!("load1 max 2.0; outside it 18% of a core (max 22%): {aihub} 17% (13% in its child processes), kcompactd0 1%; its hub 1%; no process showed 7% (interrupts 4%)"));
        assert_eq!(super::super::master::unquiet(&makepad_strict_json::parse(l.json().as_bytes()).unwrap()), None);

        // a hub's looks: an outside process's CPU grows 10 ticks at each look
        const HUB: u32 = 5_000_000;
        let looks = Arc::new(AtomicU64::new(0));
        let n2 = looks.clone();
        let look = BoxLook {
            ticks: Arc::new(move || {
                let k = n2.fetch_add(1, Ordering::AcqRel) + 1;
                let procs = [(5_000_100, crate::top::ProcTicks { name: "indexer".into(), ppid: 1, pgid: 5_000_100, own: 10 * k, ..Default::default() })].into_iter().collect();
                (crate::top::Ticks { at: Instant::now(), per_s: 100.0, procs, cpu: Some((20 * k, 0)) }, HUB)
            }),
            ..BoxLook::quiet()
        };
        let (x, base, _) = test_exec("timed-box", Opts { scopes: Some(false), box_look: Some(look), ..Opts::default() });
        let start = |id: &str| x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"timedl","kind":"timed","rev":null}}"#));
        let samples = |id: &str| x.noise.lock().unwrap().get(id).map_or(0, |n| n.samples);
        assert!(!start("T1")[0].contains("\"err\""));
        assert_eq!(looks.load(Ordering::Acquire), 1, "its first span begins at its start");
        box_look(&x);
        assert_eq!(samples("T1"), 1);
        assert!(!start("T2")[0].contains("\"err\""));
        assert_eq!(looks.load(Ordering::Acquire), 2, "T1's looks run: T2's start begins no span");
        box_look(&x);
        assert_eq!((samples("T1"), samples("T2")), (2, 0), "a span begun before T2's start is not T2's");
        box_look(&x);
        assert_eq!((samples("T1"), samples("T2")), (3, 1));
        let b = x.noise.lock().unwrap()["T1"].json();
        let v = makepad_strict_json::parse(b.as_bytes()).unwrap();
        let busiest = super::super::master::strs(v.get("busiest").and_then(|a| a.as_arr()).and_then(|a| a.first()));
        assert_eq!(busiest.first().map(String::as_str), Some("indexer"), "{b}");
        assert!(v.get("unseen_cpu_pct").is_some() && v.get("hub_cpu_pct").is_some(), "{b}");
        assert!(x.handle("lease-end", "T1 released")[0].contains("\"box\":{\"samples\":3,"));
        assert!(x.handle("lease-end", "T2 released")[0].contains("\"box\":{\"samples\":1,"));
        box_look(&x);
        assert!(x.span.lock().unwrap().is_none(), "no timed lease: the span ends");
        assert!(!start("T3")[0].contains("\"err\""));
        assert!(x.span.lock().unwrap().is_some(), "the next timed lease's first span begins at its start");
        assert!(!x.handle("lease-end", "T3 released")[0].contains("\"box\""), "ended before its first look: no figure");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_jit_of_a_stopped_lease_or_in_its_job_was_stopped_with_it() {
        note_stopping("L248709", "/h/devhub/leases/L248709", vec![4242]);
        let sock = "/tmp/makepad-1000/makepad-remote-2703620.sock";
        assert_eq!(stopped_with(1, Some("/h/devhub/leases/L248709"), sock).as_deref(), Some("with its lease L248709"));
        assert_eq!(stopped_with(1, Some("/h/devhub/leases/L2487090"), sock), None, "another lease");
        assert_eq!(stopped_with(1, Some("/other/devhub/leases/L248709"), sock), None, "another hub's lease of that id");
        assert_eq!(stopped_with(1, None, sock), None, "no lease: a crash stays a crash");
        assert_eq!(stopped_with(1, None, ""), None);
        // a Windows JIT: its job says it was the lease's
        assert_eq!(stopped_with(4242, None, "tcp:127.0.0.1:64033").as_deref(), Some("with its lease L248709"));
        assert_eq!(stopped_with(4243, None, "tcp:127.0.0.1:64034"), None, "not in its job");
        let state = Path::new("/h/devhub/leases");
        assert_eq!(mark_id(state, "/h/devhub/leases/L248709"), Some("L248709"));
        assert_eq!(mark_id(state, "/other/devhub/leases/L248709"), None, "another hub's");
        assert_eq!(mark_id(state, "L248709"), None);
    }

    /// A WM without a size is said once it has stayed so, once across this hub's restarts (the
    /// marker), and once it has one again; a WM coming up says nothing.
    #[test]
    fn a_wm_that_stays_without_a_size_is_said_once() {
        let dark = "its WM's window is 0x0: no output shows it";
        let mut said = DarkSaid::default();
        assert_eq!(said.note("box-a", "", 1_000), None);
        assert_eq!(said.note("box-a", dark, 2_000), None, "a WM coming up");
        assert_eq!(said.note("box-a", "", 3_000), None, "it got its size in time");
        assert_eq!(said.note("box-a", dark, 10_000), None);
        let (sev, line) = said.note("box-a", dark, 10_000 + DARK_SAY_MS).unwrap();
        assert!(matches!(sev, Sev::Warn) && line.contains("box-a shows no windows") && line.contains("0x0"), "{line}");
        assert_eq!(said.note("box-a", dark, 20_000 + DARK_SAY_MS), None);
        let mut next_hub = DarkSaid { said: true, ..Default::default() };
        assert_eq!(next_hub.note("box-a", dark, 30_000 + DARK_SAY_MS), None, "the hub after it");
        let (sev, line) = said.note("box-a", "", 40_000 + DARK_SAY_MS).unwrap();
        assert!(matches!(sev, Sev::Info) && line.contains("shows windows again"), "{line}");
        assert_eq!(said.note("box-a", "", 50_000 + DARK_SAY_MS), None);
    }

    /// The WM session's own apps a WM restart ends were stopped by it ([`note_stopped_pids`]), no
    /// crash: each deploy's WM restart filed the WM's aichat as one (11-06).
    #[test]
    fn an_app_a_wm_restart_ends_was_stopped_by_it() {
        note_stopped_pids("by the WM restart (the build replaced the WM binary)", vec![7_311_001, 7_311_002]);
        assert_eq!(stopped_with(7_311_002, None, "/tmp/makepad-1000/makepad-remote-7311002.sock").as_deref(), Some("by the WM restart (the build replaced the WM binary)"));
        assert_eq!(stopped_with(7_311_003, None, "/tmp/makepad-1000/makepad-remote-7311003.sock"), None, "one it did not end: a crash stays a crash");
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
        // the shell's ping joins the job as the shell starts it
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
        // (a console host of the shell's may be in it beside its ping)
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
    const ECHO_HI: &[&str] = &["powershell", "-NoProfile", "-Command", "Write-Output hi"];
    #[cfg(windows)]
    const ENV: &[&str] = &["powershell", "-NoProfile", "-Command", "Get-ChildItem env: | ForEach-Object { \"$($_.Name)=$($_.Value)\" }"];
    #[cfg(windows)]
    const FAIL: &[&str] = &["powershell", "-NoProfile", "-Command", "exit 1"];
    #[cfg(windows)]
    const SLEEP: &[&str] = &["powershell", "-NoProfile", "-Command", "ping -n 300 127.0.0.1 | Out-Null"];
    #[cfg(windows)]
    const SLEEP_LABEL: &str = "powershell.exe";

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
            crate::testenv::exec_script(&jit, &format!("#!/bin/sh\necho \"$@\" >> {}.jit\nexit {jit_exit}\n", count.display()));
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

    /// A lease's JIT runs of a program start from one link of the JIT (launch_link): the second
    /// run makes no link and removes none (the JIT's inode change time and link count stay), as
    /// each did on macOS a syspolicyd scan of the JIT at its next exec. Another program has a
    /// link of its own, a JIT rebuilt at its path its own folder, and no file name is no link.
    #[cfg(unix)]
    #[test]
    fn a_leases_runs_of_a_program_start_from_one_link_of_its_jit() {
        use std::os::unix::fs::MetadataExt;
        // (not `devhub-exec-launch-<pid>`: that is test_exec("launch")'s, which a test beside this one uses)
        let base = std::env::temp_dir().join(format!("devhub-exec-jit-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let (tmp, jit) = (base.join("tmp"), base.join("makepad.jit"));
        std::fs::write(&jit, "jit 1").unwrap();
        let changed = || std::fs::metadata(&jit).map(|m| (m.ctime(), m.ctime_nsec(), m.nlink())).unwrap();
        let first = launch_link(&tmp, &jit, "app").expect("a link");
        assert!(first.parent().is_some_and(|p| p.ends_with("+launch")) && first.file_name().is_some_and(|n| n == "app"), "{}", first.display());
        let made = changed();
        assert_eq!(made.2, 2, "one link of the JIT");
        // (Linux keeps the change time to a clock tick)
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(launch_link(&tmp, &jit, "app").as_deref(), Some(first.as_path()));
        assert_eq!(changed(), made, "the second run made no link of the JIT and removed none");
        let tool = launch_link(&tmp, &jit, "tool").expect("a link");
        assert!(tool != first && changed().2 == 3, "another program, another link");
        std::fs::remove_file(&jit).unwrap();
        std::fs::write(&jit, "jit 2").unwrap();
        let rebuilt = launch_link(&tmp, &jit, "app").expect("a link");
        assert!(rebuilt != first && std::fs::read_to_string(&rebuilt).unwrap() == "jit 2" && std::fs::read_to_string(&first).unwrap() == "jit 1", "a rebuilt JIT is linked anew");
        assert!(launch_link(&tmp, &jit, ".app").is_none() && launch_link(&tmp, &jit, "a/b").is_none() && launch_link(&tmp, &jit, "").is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    fn test_exec(name: &str, mut opts: Opts) -> (TestHub, PathBuf, Arc<Mutex<Vec<String>>>) {
        let base = std::env::temp_dir().join(format!("devhub-exec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let conf = LeaseConf::parse(&format!("work = {}\n", base.join("devhub").display()), &base);
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        (opts.watch_memory, opts.watch_wm) = (false, false);
        opts.box_look.get_or_insert_with(BoxLook::quiet);
        let x = Exec::new("t", conf, base.join("leases"), opts, Arc::new(move |r: crate::proto::Record| ev.lock().unwrap().push(r.line)));
        (TestHub(Some(x)), base, events)
    }

    /// A run that writes under the overlay's virtual root on the real disk fails its lease at its
    /// end, named, and the folder goes: it refused every overlay lease of its box after it (a native
    /// baseline build's log, a Linux node, 11-06). A lease whose runs were not going then is not its writer.
    #[cfg(unix)]
    #[test]
    fn a_run_that_writes_under_the_virtual_root_fails_its_lease_and_the_folder_goes() {
        let (x, base, events) = test_exec("vroot", Opts { scopes: Some(false), ..Opts::default() });
        let root = x.conf().work.join("dev/src");
        x.handle("lease-start", r#"{"id":"V1","lane":"writer","kind":"test","rev":null}"#);
        x.handle("lease-start", r#"{"id":"V2","lane":"bystander","kind":"test","rev":null}"#);
        let script = format!("mkdir -p '{0}/libs/x/target' && echo log > '{0}/libs/x/target/build.log'", root.display());
        let r = x.handle("lease-run", &format!("V1 . {}", b64(&["sh", "-c", &script])));
        assert_eq!(log_until_exit(&x, "V1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32).1, 0);
        assert!(root.join("libs/x/target/build.log").is_file());
        let end = x.handle("lease-end", "V1 released");
        let why = field(&end[0], "virtual_root").and_then(|v| v.as_str().map(str::to_string)).expect(&end[0]);
        assert!(why.contains("wrote under the overlay's virtual root") && why.contains("libs/x/target/build.log") && why.ends_with("removed"), "{why}");
        assert!(std::fs::symlink_metadata(&root).is_err(), "the folder is gone");
        assert!(events.lock().unwrap().iter().any(|e| e.contains("V1 (writer) wrote under")), "named");
        // the next end finds nothing, and a lease that ran nothing then is no writer
        std::fs::create_dir_all(&root).unwrap();
        let end = x.handle("lease-end", "V2 released");
        assert_eq!(field(&end[0], "virtual_root").map(|v| v.to_json()), Some("null".to_string()), "{}", end[0]);
        assert!(std::fs::symlink_metadata(&root).is_err());
        assert!(events.lock().unwrap().iter().any(|e| e.contains("no lease's run")));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A job input goes to a box once by its hash, in chunks, and is unpacked whole: a second
    /// lease finds it held, a run reads it at its `$NAME`, and the leases that named it keep it
    /// from the disk guard. An archive whose bytes are not its hash is refused, nothing placed.
    /// As a live session's home (`input-home`) it is the lease's own copy: what the app writes
    /// there never reaches the box's input, and the copy goes with the lease. The input goes with
    /// the last lease that names it, as does one that no lease names.
    #[cfg(unix)]
    #[test]
    fn a_job_input_ships_once_by_hash_and_a_run_reads_it_at_its_name() {
        let (x, base, _events) = test_exec("input", Opts { scopes: Some(false), ..Opts::default() });
        let src = base.join("corpus");
        std::fs::create_dir_all(src.join("ui/nested")).unwrap();
        std::fs::write(src.join("ui/nested/case.rs"), "fn main() {}\n").unwrap();
        let archive = base.join("ui.tgz");
        assert!(std::process::Command::new("tar").arg("-czf").arg(&archive).arg("-C").arg(&src).arg("ui").status().unwrap().success());
        let hash = super::super::ship::input_hash(&archive).unwrap();
        let bytes = std::fs::read(&archive).unwrap();
        x.handle("lease-start", r#"{"id":"I1","lane":"corpus","kind":"test","rev":null}"#);
        let started = x.handle("lease-start", r#"{"id":"I2","lane":"other","kind":"test","rev":null}"#);
        let tmp2 = PathBuf::from(field(&started[0], "tmp").and_then(|t| t.as_str().map(str::to_string)).expect(&started[0]));
        assert_eq!(x.handle("input-have", &format!("I1 {hash}")), vec!["{\"have\":false}".to_string()]);
        // a chunk at a time; the last one places it
        let half = bytes.len() / 2;
        let put = |id: &str, off: usize, chunk: &[u8]| x.handle("input-put", &format!("{id} {hash} {off} {} {}", bytes.len(), makepad_network::digest::base64_encode(chunk)));
        assert_eq!(put("I1", 0, &bytes[..half]), vec!["{\"have\":false}".to_string()]);
        let placed = put("I1", half, &bytes[half..]);
        let dir = field(&placed[0], "path").and_then(|p| p.as_str().map(PathBuf::from)).expect(&placed[0]);
        assert_eq!(dir, x.conf().work.join("inputs").join(&hash));
        assert_eq!(std::fs::read_to_string(dir.join("ui/nested/case.rs")).unwrap(), "fn main() {}\n");
        // another lease finds it held; its run reads it at its name
        let held = x.handle("input-have", &format!("I2 {hash}"));
        assert_eq!(field(&held[0], "path").and_then(|p| p.as_str().map(PathBuf::from)), Some(dir.clone()), "{held:?}");
        let placed = [super::super::ship::Placed { name: "UI".into(), hash: hash.clone(), dir: dir.to_string_lossy().into_owned(), sent: 0 }];
        let cmd = super::super::ship::with_inputs(&["sh".into(), "-c".into(), "cat \"$UI/ui/nested/case.rs\"".into()], &placed);
        assert_eq!(cmd[..2], ["env".to_string(), format!("UI={}", dir.display())]);
        let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
        let r = x.handle("lease-run", &format!("I2 . {}", b64(&refs)));
        let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
        assert_eq!(log_until_exit(&x, "I2", n), ("fn main() {}\n".to_string(), 0));
        // a live session's home: its lease's own copy, written there without touching the input;
        // given again, afresh (the app's own file gone)
        let home_of = || field(&x.handle("input-home", &format!("I2 {hash}"))[0], "path").and_then(|p| p.as_str().map(PathBuf::from));
        let home = home_of().expect("an app home");
        assert_eq!(home, super::super::ship::app_home(&tmp2));
        assert_eq!(std::fs::read_to_string(home.join("ui/nested/case.rs")).unwrap(), "fn main() {}\n");
        std::fs::write(home.join("ui/nested/case.rs"), "fn edited() {}\n").unwrap();
        std::fs::write(home.join("settings.json"), "{}").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("ui/nested/case.rs")).unwrap(), "fn main() {}\n", "the box's input is never the app's");
        assert_eq!(home_of(), Some(home.clone()));
        assert_eq!(std::fs::read_to_string(home.join("ui/nested/case.rs")).unwrap(), "fn main() {}\n");
        assert!(!home.join("settings.json").exists());
        assert!(field(&x.handle("input-home", &format!("I2 {}", "0".repeat(64)))[0], "err").is_some(), "only an input the box holds");
        // in use while a lease that named it lives; one no lease names goes at a lease's end
        let stale = x.conf().work.join("inputs").join("ab".repeat(32));
        std::fs::create_dir_all(stale.join("board")).unwrap();
        std::fs::write(stale.join("board/case.rs"), "fn old() {}\n").unwrap();
        assert_eq!(x.live().user_of(&dir).as_deref(), Some("lease I1"));
        x.handle("lease-end", "I1 released");
        assert_eq!(x.live().user_of(&dir).as_deref(), Some("lease I2"));
        assert!(dir.join("ui/nested/case.rs").is_file() && !stale.exists(), "kept while a lease names it; the unnamed one gone");
        x.handle("lease-end", "I2 released");
        assert_eq!(x.live().user_of(&dir), None, "a cache once no lease reads it");
        assert!(!dir.exists(), "the input goes with the last lease that names it");
        let gone = std::time::Instant::now();
        let left = || std::fs::read_dir(x.conf().work.join("inputs")).unwrap().count();
        while (home.exists() || left() > 0) && gone.elapsed() < std::time::Duration::from_secs(10) { std::thread::sleep(std::time::Duration::from_millis(10)); }
        assert!(!home.exists(), "the app home goes with its lease");
        assert_eq!(left(), 0, "and the inputs' bytes with theirs");
        // bytes that are not their hash: refused, nothing placed
        x.handle("lease-start", r#"{"id":"I3","lane":"bad","kind":"test","rev":null}"#);
        let other = "0".repeat(64);
        let bad = x.handle("input-put", &format!("I3 {other} 0 {} {}", bytes.len(), makepad_network::digest::base64_encode(&bytes)));
        assert!(field(&bad[0], "err").and_then(|e| e.as_str().map(str::to_string)).is_some_and(|e| e.contains("hash to") && e.contains(&hash)), "{bad:?}");
        assert!(!x.conf().work.join("inputs").join(&other).exists());
        assert!(field(&x.handle("input-have", "I3 ../../etc")[0], "err").is_some(), "a hash is 64 hex digits");
        x.handle("lease-end", "I3 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Past its log's end, `lease-log` says a run's exit alone, with no bytes: what a closed live
    /// watch asks for its app (W3683, 11-06).
    #[test]
    #[cfg(unix)]
    fn a_runs_exit_is_said_past_its_logs_end() {
        let (x, base, _events) = test_exec("exit-past-end", Opts { scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"E1","lane":"exitx","kind":"test","rev":null}"#);
        let r = x.handle("lease-run", &format!("E1 . {}", b64(&["sh", "-c", "echo bye; exit 3"])));
        let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32;
        assert_eq!(log_until_exit(&x, "E1", n).1, 3);
        let a = x.handle("lease-log", &format!("E1 {n} {}", u64::MAX >> 1));
        assert_eq!((field(&a[0], "data").and_then(|v| v.as_str().map(str::to_string)).as_deref(), field(&a[0], "exit").and_then(|v| v.as_i64())), (Some(""), Some(3)), "{a:?}");
        x.handle("lease-end", "E1 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A run's shell finds the job's own makepad first on its PATH ([`job_makepad`]): with no JIT
    /// built for its tree, the tree's by cargo, as a bare `makepad` first word runs it; with one,
    /// that binary.
    #[test]
    #[cfg(unix)]
    fn a_runs_shell_finds_the_jobs_own_makepad() {
        let (x, base, _events) = test_exec("job-makepad", Opts { scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"P1","lane":"pathx","kind":"test","rev":null}"#);
        let r = x.handle("lease-run", &format!("P1 . {}", b64(&["sh", "-c", "command -v makepad; cat \"$(command -v makepad)\""])));
        let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap_or_else(|| panic!("{r:?}")) as u32;
        let (log, code) = log_until_exit(&x, "P1", n);
        assert_eq!(code, 0, "{log}");
        assert!(log.lines().any(|l| l.ends_with(&format!("run-{n}-bin/makepad"))), "{log}");
        assert!(log.contains("exec cargo run --profile host -q --manifest-path '") && log.contains("Cargo.toml' -p makepad-run --bin makepad -- \"$@\""), "{log}");
        x.handle("lease-end", "P1 released");
        let bin = base.join("bin");
        let jit = base.join("makepad.jit-test");
        std::fs::write(&jit, "").unwrap();
        job_makepad(&bin, Some(&jit), &base, false).unwrap();
        assert_eq!(std::fs::read_link(bin.join("makepad")).unwrap(), jit);
        // a lease nobody attends: its scripts' `makepad run` is the JIT's `run --unattended`, any
        // other command as it was
        crate::testenv::exec_script(&jit, "#!/bin/sh\necho \"$@\"\n");
        job_makepad(&bin, Some(&jit), &base, true).unwrap();
        // (this process wrote the wrapper: a lease process another test thread forked meanwhile
        // holds its fd until it execs, and an exec of the wrapper then fails with ETXTBSY, so `sh`
        // reads it; the run's shell finds it on PATH, executable)
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(bin.join("makepad")).unwrap().permissions().mode() & 0o777, 0o755);
        let said = |args: &[&str]| String::from_utf8(std::process::Command::new("/bin/sh").arg(bin.join("makepad")).args(args).output().unwrap().stdout).unwrap();
        assert_eq!(said(&["run", "-p", "app", "--", "x y"]), "run --unattended -p app -- x y\n");
        assert_eq!(said(&["check", "app"]), "check app\n");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A JIT run with no kept binary on its box is cargo's build of it first, a step of its lease's
    /// own ([`Exec::jit_step`]): its holder hears `pending` while it builds (the lease's phase
    /// `jit`, its run 1), then the command runs on the binary built (run 2), cargo's output before
    /// its own. The lease's class names that build ([`super::super::JIT_BUILD_RUN`]); a run of
    /// anything else keeps its own. A build that fails is the answer's run: cargo's output and
    /// exit. A script that calls the bare `makepad` its run gets builds it the same way first.
    /// (Its cargo builds a stand-in `makepad-run` of a few lines here.)
    #[cfg(all(unix, feature = "checkout"))]
    #[test]
    fn a_jit_run_with_no_kept_binary_builds_it_as_a_step_first() {
        let (x, base, _events) = test_exec("jit-build-class", Opts { scopes: Some(false), ..Opts::default() });
        let class = |id: &str| x.leases.lock().unwrap()[id].first_run.clone();
        let jit_open = |id: &str| x.leases.lock().unwrap()[id].phases.0.iter().any(|s| s.phase == "jit" && s.end == 0);
        let stand_in = |id: &str, main: &str| {
            let work = x.leases.lock().unwrap()[id].work.clone();
            std::fs::write(work.join("Cargo.toml"), "[package]\nname = \"makepad-run\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"makepad\"\npath = \"main.rs\"\n\n[profile.host]\ninherits = \"release\"\n\n[workspace]\n").unwrap();
            std::fs::write(work.join("main.rs"), main).unwrap();
        };
        // asked as its holder asks: again with the same request until the run starts
        let run_env = |id: &str, request: &str, env: &[&str], args: &[&str]| -> (String, u32) {
            let argv: Vec<&str> = env.iter().copied().chain(["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--"]).chain(args.iter().copied()).collect();
            let line = format!("{id} . {} {{\"request_id\":\"{request}\"}}", b64(&argv));
            let first = x.handle("lease-run", &line)[0].clone();
            assert!(first.contains("cargo builds this tree's JIT first;"), "{first}");
            assert!(jit_open(id), "the build is the lease's step");
            // what a restarted master asks first: with the build's process up there is still no
            // command's run, so it asks again and joins
            while x.leases.lock().unwrap()[id].runs.is_empty() { std::thread::sleep(Duration::from_millis(1)); }
            let no_run = x.handle("lease-log", &format!("{id} 1 0"))[0].clone();
            assert!(no_run.contains("has no run 1"), "{no_run}");
            let t = Instant::now();
            loop {
                let a = x.handle("lease-run", &line)[0].clone();
                if !a.contains("\"pending\"") {
                    let n = field(&a, "n").and_then(|v| super::super::num(&v)).unwrap_or_else(|| panic!("{a}")) as u32;
                    return (a, n);
                }
                assert!(t.elapsed() < Duration::from_secs(600), "still {a}");
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        let run = |id: &str, request: &str, args: &[&str]| run_env(id, request, &[], args);
        // what cargo's build ran under: its jobs and its nice level, as its build script sees them
        // (getpriority: macOS's `nice` prints no level, only its usage)
        const SAYS_ITS_BUILD: &str = "extern \"C\" {\n    fn getpriority(which: i32, who: u32) -> i32;\n}\n\nfn main() {\n    // PRIO_PROCESS of this process\n    let nice = unsafe { getpriority(0, 0) };\n    std::fs::write(std::path::Path::new(&std::env::var(\"CARGO_MANIFEST_DIR\").unwrap()).join(\"build.said\"), format!(\"{} {nice}\", std::env::var(\"NUM_JOBS\").unwrap())).unwrap();\n}\n";
        let build_said = |id: &str| std::fs::read_to_string(x.leases.lock().unwrap()[id].work.join("build.said")).unwrap();
        x.handle("lease-start", r#"{"id":"J1","lane":"jitx","kind":"test","rev":null}"#);
        stand_in("J1", "fn main() { println!(\"stand-in JIT {:?}\", std::env::args().skip(1).collect::<Vec<_>>()); }\n");
        std::fs::write(x.leases.lock().unwrap()["J1"].work.join("build.rs"), SAYS_ITS_BUILD).unwrap();
        let (_, n) = run("J1", "q1", &["--version"]);
        assert_eq!(n, 1, "its command is the lease's first: the build takes no command's number");
        // all of cargo's jobs, not the lease's, at idle priority, while its command keeps its own
        let every = std::thread::available_parallelism().unwrap().get();
        assert_eq!(build_said("J1"), format!("{every} 19"));
        let (text, code) = log_until_exit(&x, "J1", n);
        assert_eq!(code, 0, "{text}");
        assert!(text.contains("devhub: cargo built this tree's JIT in") && text.contains("devhub: the JIT is this box's binary built by its lease, no cargo"), "{text}");
        let ran = text.find("stand-in JIT [\"--version\"]").unwrap_or_else(|| panic!("its command ran on what was built: {text}"));
        assert!(text.find("devhub: cargo built").is_some_and(|b| b < ran), "cargo's part first: {text}");
        assert!(!jit_open("J1"), "built: the step ended");
        assert_eq!(class("J1"), format!("test makepad --version{}", super::super::JIT_BUILD_RUN));
        let r = x.handle("lease-run", &format!("J1 . {}", b64(&["sh", "-c", "exit 0"])));
        assert_eq!(field(&r[0], "n").and_then(|v| super::super::num(&v)), Some(2.0), "its next command: {r:?}");
        // a caller's own jobs stand
        x.handle("lease-start", r#"{"id":"J4","lane":"jitjobs","kind":"test","rev":null}"#);
        stand_in("J4", "fn main() {}\n");
        std::fs::write(x.leases.lock().unwrap()["J4"].work.join("build.rs"), SAYS_ITS_BUILD).unwrap();
        let (_, n) = run_env("J4", "q4", &["env", "CARGO_BUILD_JOBS=2"], &["--version"]);
        assert_eq!(log_until_exit(&x, "J4", n).1, 0);
        assert_eq!(build_said("J4"), "2 19");
        // a build that fails: cargo's own output and exit are the run's
        x.handle("lease-start", r#"{"id":"J3","lane":"jitbad","kind":"test","rev":null}"#);
        stand_in("J3", "fn main() { not rust }\n");
        let (_, n) = run("J3", "q3", &["--version"]);
        assert_eq!(n, 1, "the command's run");
        let (text, code) = log_until_exit(&x, "J3", n);
        assert!(code != 0 && text.contains("error"), "{code} {text}");
        assert!(!jit_open("J3"));
        // and asked again, the same run
        let again = x.handle("lease-run", &format!("J3 . {} {{\"request_id\":\"q3\"}}", b64(&["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "--version"])))[0].clone();
        assert_eq!(field(&again, "n").and_then(|v| super::super::num(&v)), Some(1.0), "{again}");
        x.handle("lease-start", r#"{"id":"J2","lane":"jitx","kind":"test","rev":null}"#);
        let r = x.handle("lease-run", &format!("J2 . {}", b64(&["sh", "-c", "exit 0"])));
        let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap_or_else(|| panic!("{r:?}")) as u32;
        assert_eq!(log_until_exit(&x, "J2", n).1, 0);
        assert!(!class("J2").ends_with(super::super::JIT_BUILD_RUN), "{}", class("J2"));
        // a script that calls the bare `makepad` its run gets: the same build first, then that
        // `makepad` is what was built (it was cargo's quiet build at the call, silent for minutes)
        x.handle("lease-start", r#"{"id":"J5","lane":"jitscript","kind":"test","rev":null}"#);
        stand_in("J5", "fn main() { println!(\"stand-in JIT {:?}\", std::env::args().skip(1).collect::<Vec<_>>()); }\n");
        let line = format!("J5 . {} {{\"request_id\":\"q5\"}}", b64(&["sh", "-c", "echo before; makepad --version"]));
        let first = x.handle("lease-run", &line)[0].clone();
        assert!(first.contains("cargo builds this tree's JIT first;"), "a script's makepad is built as a step: {first}");
        let t = Instant::now();
        let n = loop {
            let a = x.handle("lease-run", &line)[0].clone();
            if !a.contains("\"pending\"") { break field(&a, "n").and_then(|v| super::super::num(&v)).unwrap_or_else(|| panic!("{a}")) as u32; }
            assert!(t.elapsed() < Duration::from_secs(600), "still {a}");
            std::thread::sleep(Duration::from_millis(100));
        };
        let (text, code) = log_until_exit(&x, "J5", n);
        assert_eq!(code, 0, "{text}");
        let ran = text.find("stand-in JIT [\"--version\"]").unwrap_or_else(|| panic!("the script's makepad ran what was built: {text}"));
        assert!(text.find("devhub: cargo built this tree's JIT").is_some_and(|b| b < ran), "the build first: {text}");
        // what its build held is measured apart from its command, for a lease that built one only
        let peak = |id: &str| { let r = x.handle("lease-end", &format!("{id} released"))[0].clone(); field(&r, "jit_peak_gb").and_then(|v| super::super::num(&v)) };
        assert!(peak("J1").is_some_and(|gb| gb > 0.0), "J1 built");
        assert_eq!(peak("J2"), None, "J2 built nothing");
        for id in ["J3", "J4", "J5"] { x.handle("lease-end", &format!("{id} released")); }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A tree lease's command that no target's binary serves takes the box's dev JIT of its tree's
    /// key: held while the lease lasts and released at its end, so the store's pruning leaves it
    /// meanwhile; a key the store lacks takes nothing, and a lease that held nothing frees nothing.
    #[test]
    #[cfg(feature = "checkout")]
    fn a_tree_lease_takes_the_boxs_dev_jit_of_its_key() {
        const KEY: &str = "0123456789abcdef01234567";
        const OTHER: &str = "76543210fedcba9876543210";
        let w = std::env::temp_dir().join(format!("devhub-exec-store-jit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&w);
        std::fs::create_dir_all(&w).unwrap();
        let store = super::super::jitstore::Store::new(&w);
        std::fs::write(w.join("built"), b"jit").unwrap();
        store.keep(KEY, &w.join("built")).unwrap();
        let (k, p) = store_jit(&w, KEY, "L1").unwrap().expect("the store's binary");
        assert_eq!(k, KEY);
        assert_eq!(std::fs::read(&p).unwrap(), b"jit");
        assert!(w.join("dev/jit/leases/L1").join(KEY).is_file(), "held for its lease");
        assert_eq!(store_jit(&w, OTHER, "L2").unwrap(), None);
        store.release("L1").unwrap();
        assert!(!w.join("dev/jit/leases/L1").exists());
        store.release("L3").unwrap();
        assert!(store.lookup(KEY).is_some(), "kept");
        let _ = std::fs::remove_dir_all(&w);
    }

    /// A live session's box meters its calls ([`CallMeter`]): its launch from its run's spawn to
    /// its first idle, each later call from its start to its end; a word that changes nothing
    /// says none. Its run sleeps, and nothing here waits: what a call's sample says of its cores
    /// is `a_calls_meter_takes_its_peak_by_window_and_its_mean_over_the_call`'s, on fed
    /// readings. A run that spun a core here got 13% of one on a loaded box's tests (11-08).
    #[test]
    #[cfg(unix)]
    fn a_live_sessions_calls_are_metered_by_its_box() {
        let (x, base, _events) = test_exec("calls", Opts { scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"V1","lane":"livex","kind":"test","rev":null,"live":true}"#);
        let spawned = Instant::now();
        let r = x.handle("lease-run", &format!("V1 . {}", b64(&["sleep", "30"])));
        assert!(field(&r[0], "n").is_some(), "{r:?}");
        let said = |x: &Exec, word: &str| {
            let a = x.handle("lease-weight", &format!("V1 {word}"));
            field(&a[0], "call").map(|c| {
                let n = |k| c.get(k).and_then(super::super::num).unwrap();
                (c.get("launch").and_then(|v| v.as_bool()).unwrap(), n("ms"), n("peak"), n("mean"))
            })
        };
        let (launch, ms, peak, mean) = said(&x, "idle").expect("the launch's sample");
        assert!(launch && ms <= spawned.elapsed().as_millis() as f64 && mean <= peak, "{launch} {ms} {peak} {mean}");
        assert_eq!(said(&x, "idle"), None, "no call ended");
        let called = Instant::now();
        assert_eq!(said(&x, "call"), None);
        let (launch, ms, peak, mean) = said(&x, "idle").expect("the call's sample");
        assert!(!launch && ms <= called.elapsed().as_millis() as f64 && mean <= peak, "{launch} {ms} {peak} {mean}");
        x.handle("lease-end", "V1 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A call's sample on fed readings: the most cores one [`CALL_WINDOW`] took and the call's
    /// mean. A window under half of one counts in the call's use only, and a call shorter than a
    /// window peaked at its mean; a run that ended took its CPU out, and the window starts again
    /// from there; an end its runs could not be read at is the last look's.
    #[test]
    fn a_calls_meter_takes_its_peak_by_window_and_its_mean_over_the_call() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let cpu = |us: u64| os::Activity { cpu_us: us, ..Default::default() };
        // the launch: a core for 2 s, then nothing for a second
        let mut m = CallMeter::new(true, t0, cpu(0));
        m.look(at(1_000), cpu(1_000_000));
        m.look(at(2_000), cpu(2_000_000));
        m.look(at(3_000), cpu(2_000_000));
        assert_eq!(m.end(at(3_000), Some(cpu(2_000_000))), r#"{"launch":true,"ms":3000,"peak":1.00,"mean":0.67}"#);
        // 0.3 s on two cores: no window, its mean
        let m = CallMeter::new(false, t0, cpu(5_000_000));
        assert_eq!(m.end(at(300), Some(cpu(5_600_000))), r#"{"launch":false,"ms":300,"peak":2.00,"mean":2.00}"#);
        // a run ends in the second window: that window counts nothing, the next one two cores
        let mut m = CallMeter::new(false, t0, cpu(4_000_000));
        m.look(at(1_000), cpu(5_000_000));
        m.look(at(2_000), cpu(500_000));
        m.look(at(3_000), cpu(2_500_000));
        assert_eq!(m.end(at(3_000), Some(cpu(2_500_000))), r#"{"launch":false,"ms":3000,"peak":2.00,"mean":1.00}"#);
        // half a core for a second, then its runs could not be read
        let mut m = CallMeter::new(false, t0, cpu(0));
        m.look(at(1_000), cpu(500_000));
        assert_eq!(m.end(at(1_500), None), r#"{"launch":false,"ms":1500,"peak":0.50,"mean":0.33}"#);
    }

    /// A run's pid file whose lease the hub no longer has, and that is not ending, is a gone run:
    /// its job is ended by name (Windows) and its files go.
    #[test]
    fn a_run_left_by_a_gone_lease_is_found_by_its_pid_file() {
        let state = std::env::temp_dir().join(format!("devhub-gone-runs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state);
        std::fs::create_dir_all(&state).unwrap();
        for f in ["L1-1.usage.pid", "L2-1.usage.pid", "L2-2.usage.pid", "L3-1.usage.pid", "L4-1.exit", "L5-x.usage.pid", "notes.usage.pid"] {
            std::fs::write(state.join(f), "1").unwrap();
        }
        // L1 is here and L3 ending
        let here = || vec!["L1".to_string(), "L3".to_string()];
        assert_eq!(gone_runs(&state, &here), vec![("L2".to_string(), state.join("L2-1.exit")), ("L2".to_string(), state.join("L2-2.exit"))]);
        let _ = std::fs::remove_dir_all(&state);
    }

    /// Two hubs on one box (two test processes, or two tests) name their runs' units apart; a
    /// hub's tag is its ledger folder's, the same after its restart.
    #[test]
    fn hubs_on_one_box_name_their_units_apart() {
        let (a, base_a, _) = test_exec("units-a", Opts::default());
        let (b, base_b, _) = test_exec("units-b", Opts::default());
        assert_ne!(a.units, b.units);
        assert_eq!(unit_tag(&base_a.join("leases")), a.units, "a restarted hub's tag is the same");
        drop((a, b));
        let _ = std::fs::remove_dir_all(&base_a);
        let _ = std::fs::remove_dir_all(&base_b);
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

    /// A timed lease's JIT builds stay held between its series' runs: one whose lock it took during
    /// the lease (relax stamps the file) or that a process holds now, never one it did not use; a
    /// restarted hub takes them again from its file, and past their time they go (11-18).
    #[test]
    fn a_timed_series_holds_its_jit_builds_between_its_runs() {
        let base = std::env::temp_dir().join(format!("devhub-series-holds-{}", std::process::id()));
        let live = base.join("run/cache/live");
        std::fs::create_dir_all(&live).unwrap();
        let file = base.join("series-holds");
        let stamp = |p: &Path, t: std::time::SystemTime| std::fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
        let lock = |name: &str| {
            let p = live.join(name);
            std::fs::write(&p, b"").unwrap();
            stamp(&p, std::time::SystemTime::now() - Duration::from_secs(3600));
            p
        };
        let (old, used, running) = (lock("aaaa"), lock("bbbb"), lock("cccc"));
        let (dirs, timed) = (vec![base.join("run")], vec![("T1".to_string(), "jitperf".to_string())]);
        let mut s = SeriesHolds::default();
        assert!(s.update(&file, &dirs, &timed, None, 1_000).is_none(), "its first look: nothing taken yet");
        // during the lease one build's run took its lock (relax stamps it), another's runs now
        stamp(&used, std::time::SystemTime::now() + Duration::from_secs(1));
        let process = lock_shared(&running).unwrap();
        let said = s.update(&file, &dirs, &timed, Some("T1"), 2_000).expect("it says what it holds");
        assert!(said.contains("bbbb") && said.contains("cccc") && !said.contains("aaaa"), "{said}");
        drop(process);
        assert!(lock_taken(&used) && lock_taken(&running), "held after their runs ended");
        // (what it lets go of is said by its own state: a lock it dropped can stay taken a moment
        // in a child another test forks meanwhile, which holds a copy of every open file until
        // its exec; this test failed so on a loaded box)
        assert!(!s.held.contains_key(&old), "a build the series did not use stays relax's");
        drop(s);
        let mut again = SeriesHolds::default();
        assert!(again.update(&file, &[], &[], None, 3_000).is_none());
        assert!(lock_taken(&used) && lock_taken(&running), "a restarted hub holds them again");
        let said = again.update(&file, &[], &[], None, 2_000 + SERIES_KEEP_MS + 1).expect("past their time");
        assert!(said.contains("lets go of 2"), "{said}");
        assert!(again.held.is_empty() && std::fs::read_to_string(&file).unwrap().is_empty(), "past their time they are relax's again");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A live test session's own lane (`live-w<n>`, its one lease) leaves with its lease's end, not
    /// while a lease of it runs; another lane's dirs wait for its explicit clean. Ended sessions'
    /// dirs piled up on the nodes: 133 of them, 7.8 GB, on one Mac (11-18).
    #[test]
    fn a_sessions_lane_dirs_go_with_its_lease() {
        let (x, base, _) = test_exec("session-lane", Opts::default());
        let start = |id: &str, lane: &str| format!("{{\"id\":\"{id}\",\"lane\":\"{lane}\",\"kind\":\"test\",\"rev\":null}}");
        let work = x.conf().work.clone();
        let dirs = |lane: &str| [work.join("dev/lane").join(lane), work.join("dev/overlay").join(lane), work.join("dev/lane-jit").join(lane)];
        for lane in ["live-w7", "livework"] {
            for d in dirs(lane) {
                std::fs::create_dir_all(d.join("deep")).unwrap();
                std::fs::write(d.join("deep/file"), b"x").unwrap();
            }
        }
        for (id, lane) in [("S1", "live-w7"), ("L1", "livework")] {
            assert!(!x.handle("lease-start", &start(id, lane))[0].starts_with("{\"err\""), "{id} starts");
        }
        x.session_ended("live-w7");
        assert!(dirs("live-w7").iter().all(|d| d.is_dir()), "a lease of the session runs: its dirs stay");
        x.handle("lease-end", "S1 released");
        assert!(dirs("live-w7").iter().all(|d| !d.exists()), "its lease ended: its dirs go");
        assert!(x.cleaning.0.lock().unwrap().is_empty(), "and hold up no start after");
        x.handle("lease-end", "L1 released");
        assert!(dirs("livework").iter().all(|d| d.is_dir()), "a lane's own dirs wait for its clean");
        // a session's lease that ends as it starts (its end came first): the start holds the start
        // lock as it ends it, and the cleanup took that lock again on its thread, deadlocking the
        // hub (11-18). It goes once the start lets go.
        for d in dirs("live-w8") {
            std::fs::create_dir_all(&d).unwrap();
        }
        x.handle("lease-end", "S2 released");
        assert!(x.handle("lease-start", &start("S2", "live-w8"))[0].contains("as it started"), "the start ends it");
        let until = Instant::now() + Duration::from_secs(10);
        while dirs("live-w8").iter().any(|d| d.exists()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(dirs("live-w8").iter().all(|d| !d.exists()), "its dirs go after the start");
        assert!(x.starting.try_lock().is_ok(), "and the start lock is free");
        assert!(session_lane("live-w12567") && session_lane("live-W3"), "a session's lane");
        assert!(!session_lane("live-w") && !session_lane("live-wx1") && !session_lane("livework") && !session_lane("live-escope"), "no other lane");
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
        fn restart(mut self, mut opts: Opts) -> TestHub {
            let x = self.0.take().unwrap();
            let (conf, state) = (x.conf(), x.state.clone());
            drop(x);
            (opts.watch_memory, opts.watch_wm) = (false, false);
            opts.box_look.get_or_insert_with(BoxLook::quiet);
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
        let result = join_recorded_build(&path, "L2", &|| Ok(()), &|joined| {
            assert_eq!(joined, pid, "join the recorded child");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), format!("{pid} {start} L2\n"), "the joiner owns the build from then on");
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
            join_recorded_build(&path, "L2", &|| Ok(()), &|_| panic!("a dead or reused pid must not be joined")).unwrap();
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
    /// master granted it) ends what the start sets up: the lease never runs on unbeaten here. A
    /// start's mark (what a WM that would replace this hub waits for) goes with its end, and a
    /// mark an earlier hub left goes when the next one starts.
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
        assert!(base.join("leases/S2.json").exists() && !base.join("leases/S1.starting").exists() && !base.join("leases/S2.starting").exists(), "a start's mark goes as it ends");
        drop(x);
        std::fs::write(base.join("leases/S9.starting"), "").unwrap();
        let conf = LeaseConf::parse(&format!("work = {}\n", base.join("devhub").display()), &base);
        let y = TestHub(Some(Exec::new("t", conf, base.join("leases"), Opts { watch_memory: false, watch_wm: false, box_look: Some(BoxLook::quiet()), ..Opts::default() }, Arc::new(|_| {}))));
        assert!(!base.join("leases/S9.starting").exists(), "an earlier hub's unfinished start: nothing starts here now");
        drop(y);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A live run's beat says what the lease holds, the most at one look so far (what the master
    /// charges it); a run that ends before a beat looks at it still leaves its class a peak, the
    /// one its waiter read as it reaped it.
    /// A beat says how long a lease's runs did nothing, from the counts its looks found: the
    /// quiet time grows while they stay under [`IDLE_CPU_PCT`] of a core and 1 MB of I/O, starts
    /// again at a look that shows work or a process gone, and is no number while nothing can be
    /// measured. Its slow time is the same under [`SLOW_CPU_PCT`]. The looks and their times are
    /// given, so a loaded box cannot change the answer.
    #[test]
    fn a_beat_says_how_long_a_leases_runs_did_nothing() {
        let (x, base, _) = test_exec("quiet", Opts::default());
        x.handle("lease-start", r#"{"id":"Q1","lane":"quiet","kind":"test","rev":null}"#);
        let t0 = Instant::now();
        let quiet = |secs: u64, done: Option<(f64, u64)>| {
            let look = done.map(|(cpu_s, io_bytes)| os::Activity { cpu_us: (cpu_s * 1e6) as u64, io_bytes, ..Default::default() });
            let samples = BTreeMap::from([("Q1".to_string(), (None, None, look))]);
            let a = x.beat_from("Q1", &samples, t0 + Duration::from_secs(secs));
            let v = makepad_strict_json::parse(a.as_bytes()).unwrap();
            let of = |k: &str| v.get("info").and_then(|i| i.get("Q1")).and_then(|i| i.get(k)).and_then(|s| s.as_u64());
            (of("idle_s"), of("slow_s"))
        };
        assert_eq!(quiet(0, None), (None, None), "nothing measured: no number");
        assert_eq!(quiet(0, Some((40.0, 5 << 20))), (Some(0), Some(0)), "the first look starts the quiet");
        assert_eq!(quiet(300, Some((49.0, 5 << 20))), (Some(300), Some(300)), "polling at 3% of a core is doing nothing");
        assert_eq!(quiet(700, Some((70.0, (5 << 20) + 100_000))), (Some(700), Some(700)), "4.3% and 100 KB written: still nothing");
        assert_eq!(quiet(710, Some((130.0, 6 << 20))), (Some(0), Some(710)), "a minute of CPU in ten seconds is work, 13% since the slow start is slow");
        assert_eq!(quiet(720, Some((130.1, 6 << 20))), (Some(10), Some(720)));
        assert_eq!(quiet(730, Some((130.1, 8 << 20))), (Some(0), Some(0)), "2 MB written is work");
        assert_eq!(quiet(740, Some((20.0, 8 << 20))), (Some(0), Some(0)), "a process gone with what it had done");
        assert_eq!(quiet(800, Some((32.0, 8 << 20))), (Some(0), Some(60)), "a fifth of a core: at work, but slow");
        assert_eq!(quiet(820, Some((80.0, 8 << 20))), (Some(0), Some(0)), "past half a core since the slow start: at work");
        assert_eq!(quiet(830, None), (None, None), "a look that cannot measure ends the quiet");
        // on a box past its cores: its wait for a CPU is the CPU it wanted
        let starved = |secs: u64, cpu_s: f64, wait_s: f64| {
            let look = os::Activity { cpu_us: (cpu_s * 1e6) as u64, io_bytes: 8 << 20, wait_us: (wait_s * 1e6) as u64 };
            let samples = BTreeMap::from([("Q1".to_string(), (None, None, Some(look)))]);
            let a = x.beat_from("Q1", &samples, t0 + Duration::from_secs(secs));
            let v = makepad_strict_json::parse(a.as_bytes()).unwrap();
            let of = |k: &str| v.get("info").and_then(|i| i.get("Q1")).and_then(|i| i.get(k)).and_then(|s| s.as_u64());
            (of("idle_s"), of("slow_s"))
        };
        assert_eq!(starved(840, 80.0, 0.0), (Some(0), Some(0)), "the first look starts the quiet");
        assert_eq!(starved(900, 83.0, 55.0), (Some(0), Some(0)), "3 s of CPU in a minute it spent 55 s waiting for one: at work, neither quiet nor slow");
        assert_eq!(starved(960, 84.0, 56.0), (Some(60), Some(60)), "a poller's short waits for a CPU are still nothing");
        x.handle("lease-end", "Q1 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A lease's beat says the panic one of its runs' JIT processes holds parked (the local
    /// agent's feed: [`app_panicked`]), while that process runs and its `panics` lists it: the
    /// master ends a lease nobody attends on it once it has gone quiet. A mark of another lease's,
    /// an expected crash or a record of no JIT panic says nothing.
    #[test]
    fn a_beat_says_the_panic_its_app_holds_parked() {
        let (x, base, _) = test_exec("parked", Opts::default());
        x.handle("lease-start", r#"{"id":"P7","lane":"parked","kind":"timed","rev":null}"#);
        let panic = || {
            let samples = BTreeMap::from([("P7".to_string(), (None, None, None))]);
            let a = x.beat_from("P7", &samples, Instant::now());
            let v = makepad_strict_json::parse(a.as_bytes()).unwrap();
            v.get("info").and_then(|i| i.get("P7")).and_then(|i| i.get("panic")).and_then(|p| p.as_str()).map(str::to_string)
        };
        assert_eq!(panic(), None, "nothing parked");
        let detail = r#"{"event":"panic","id":"P3","kind":"compile error","message":"mismatched types","location":"a.rs:3:5","thread":"main"}"#;
        let (id, text) = parked_panic(detail, "panic: mismatched types at a.rs:3:5 [main]").unwrap();
        assert_eq!((id.as_str(), text.as_str()), ("P3", "mismatched types at a.rs:3:5 [main]"));
        assert!(parked_panic(r#"{"event":"panic","id":"P4","message":"injected","expected":"yes"}"#, "panic: injected").is_none(), "an expected crash is its test's own");
        assert!(parked_panic(r#"{"event":"panic","message":"no id"}"#, "panic: no id").is_none(), "no JIT panic: nothing parks on it");
        assert!(!x.app_panicked(&lease_mark(&x.state, "P99"), std::process::id(), &text));
        assert_eq!(panic(), None, "another lease's mark");
        // (this test's own process stands for the parked app: it runs)
        assert!(x.app_panicked(&lease_mark(&x.state, "P7"), std::process::id(), &text), "nobody attends a timed lease: its watch releases the park");
        assert_eq!(panic().as_deref(), Some("mismatched types at a.rs:3:5 [main]"));
        assert_eq!(unattended_release(r#"{"event":"panic","id":"P1","kind":"panic","message":"index out of bounds"}"#), "panic", "a panic unwinds");
        assert_eq!(unattended_release(detail), "stop", "code that did not compile: its run ends");
        assert_eq!(unattended_release(r#"{"event":"panic","id":"P5","kind":"stale access","message":"x"}"#), "stop", "a stale access ends its process");
        x.app_unparked(&lease_mark(&x.state, "P7"), std::process::id() + 1);
        assert!(panic().is_some(), "another process's unpark");
        x.app_unparked(&lease_mark(&x.state, "P7"), std::process::id());
        assert_eq!(panic(), None, "its panics no longer lists it");
        // a process that ended took its panic along: its exit decides
        let mut gone = std::process::Command::new("true").spawn().unwrap();
        let pid = gone.id();
        gone.wait().unwrap();
        x.app_panicked(&lease_mark(&x.state, "P7"), pid, &text);
        assert_eq!(panic(), None, "its process ended");
        x.handle("lease-end", "P7 released");
        // a lease someone attends keeps its park for them: an interactive one, a live one
        x.handle("lease-start", r#"{"id":"P8","lane":"parked","kind":"test","priority":"interactive","rev":null}"#);
        assert!(!x.app_panicked(&lease_mark(&x.state, "P8"), std::process::id(), &text), "its holder decides");
        x.handle("lease-end", "P8 released");
        x.handle("lease-start", r#"{"id":"P9","lane":"parked","kind":"test","priority":"background","rev":null}"#);
        assert!(x.app_panicked(&lease_mark(&x.state, "P9"), std::process::id(), &text), "a background test lease: nobody attends it");
        x.handle("lease-end", "P9 released");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A box's waiting tasks go to the leases whose threads wait: a tests shard with 30 of the
    /// box's 38 runnable threads made 17 of its 22 waiting tasks (L1447930's box, 11-19).
    #[test]
    fn a_boxs_waiting_tasks_go_to_the_leases_whose_threads_wait() {
        let runnable = BTreeMap::from([("L1".to_string(), 30), ("L2".to_string(), 0), ("L3".to_string(), 1)]);
        let by = waits_by_lease(22.0, &runnable, 38);
        assert_eq!(by.len(), 2, "a lease with no runnable thread has no share: {by:?}");
        assert_eq!(by[0].0, "L1");
        assert!((by[0].1 - 22.0 * 30.0 / 38.0).abs() < 1e-9, "its share of the run queue: {by:?}");
        assert!(waits_by_lease(22.0, &BTreeMap::from([("L1".to_string(), 1)]), 100).is_empty(), "under half a task is left out");
        assert!((waits_by_lease(4.0, &runnable, 10)[0].1 - 4.0 * 30.0 / 31.0).abs() < 1e-9, "a count read after the scopes' never makes a share past the whole");
    }

    #[test]
    fn scoped_cpu_keeps_exited_children_and_names_leftovers() {
        let ticks = crate::top::CpuSplit {
            leases: vec![(format!("{SCOPE_CPU}L1"), 0.2), ("L1".into(), 0.5), ("L2".into(), 0.25)],
            outside: vec![("test child".into(), 101, 1.0), ("leftover".into(), 102, 0.8), ("other".into(), 103, 0.6)],
            apps: vec![(103, "other".into())],
            builds: vec![(format!("{SCOPE_CPU}L1"), 3.0, "makepad_widgets".into())],
        };
        let scopes = BTreeMap::from([("L1".to_string(), 4.5)]);
        let scope_lease = |pid, measured| match pid {
            101 => Some(CpuScope { unit: "run".into(), lease: Some("L1".into()), run: None, measured }),
            102 => Some(CpuScope { unit: "ended".into(), lease: Some("L3".into()), run: Some(1), measured: false }),
            _ => None,
        };
        let live = |unit: &str| matches!(unit, "run" | "store");
        let here = |id: &str| matches!(id, "L1" | "L2");
        let merged = merge_scope_cpu(ticks.clone(), &scopes, &|pid| scope_lease(pid, true), &live, &here);
        assert_eq!(merged.leases, vec![("L1".into(), 5.0), ("L2".into(), 0.25)], "4.5 scoped cores plus 0.5 store-group cores; no second count of the scope's ticks or detached child");
        assert_eq!(merged.builds, vec![("L1".into(), 3.0, "makepad_widgets".into())], "a measured scope's compilers are its lease's builds");
        assert_eq!(merged.outside, vec![("leftover (left by ended lease L3)".into(), 102, 0.8), ("other".into(), 103, 0.6)], "a live scope is no outside work; ended and unrelated work stay");
        // the same leftover while its lease still runs: its run ended, not its lease (L1446804's JIT,
        // left by its live test's restart, was said "left by ended lease" while that lease ran, 11-19)
        let held = merge_scope_cpu(ticks.clone(), &scopes, &|pid| scope_lease(pid, true), &live, &|id| here(id) || id == "L3");
        assert_eq!(held.outside[0], ("leftover (left by its ended run 1 of lease L3)".into(), 102, 0.8), "an ended run's leftover in a lease still held");
        assert_eq!(merged.apps, ticks.apps, "the merge leaves app discovery alone");
        let first = merge_scope_cpu(ticks.clone(), &BTreeMap::new(), &|pid| scope_lease(pid, false), &live, &here);
        assert!((first.leases[0].1 - 1.7).abs() < 1e-9, "a first look keeps 0.2 scoped ticks, 0.5 store ticks and its detached child's 1.0: {:?}", first.leases);
        let mut mixed = ticks.clone();
        mixed.leases.push(("L1".into(), 0.3));
        let mixed = merge_scope_cpu(mixed, &scopes, &|pid| scope_lease(pid, true), &live, &here);
        assert!((mixed.leases[0].1 - 5.3).abs() < 1e-9, "a new run's ticks stay beside the same lease's measured scope: {:?}", mixed.leases);
        let store = merge_scope_cpu(ticks, &scopes, &|pid| match pid {
            101 => Some(CpuScope { unit: "store".into(), lease: Some("L1".into()), run: None, measured: false }),
            102 => Some(CpuScope { unit: "ended-jit".into(), lease: None, run: None, measured: false }),
            _ => None,
        }, &live, &here);
        assert_eq!(store.leases[0], ("L1".into(), 6.0), "a live store build's detached child's ticks join its lease even when another run has a scope figure");
        assert_eq!(store.outside, vec![("leftover (left by an ended JIT build)".into(), 102, 0.8), ("other".into(), 103, 0.6)], "an ended JIT build has no lease id to parse");
    }

    /// A store build's CPU is the lease's whose overlay started it in the box's CPU split, never
    /// work of no lease's: a box prebuild's dev JIT read as the hub's cargo at 7 cores (11-11).
    #[cfg(feature = "checkout")]
    #[test]
    fn a_store_build_is_its_leases_in_the_cpu_split() {
        let (x, base, _) = test_exec("storecpu", Opts::default());
        let build = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: 0, ended_ms: AtomicU64::new(0), n: 0, pid: 4_000_001, start: 0, unit: None, log: base.join("cargo.log"), exit: Mutex::new(None), ours: true, detachable: false, hold: os::Hold::none(), exit_file: base.join("cargo.exit"), cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
        x.store_builds.lock().unwrap().push(("S1".into(), build));
        assert_eq!(x.lease_groups().get(&4_000_001).map(String::as_str), Some("S1"));
        x.store_builds.lock().unwrap().clear();
        assert!(x.lease_groups().is_empty(), "built: no group of its lease's left");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A store build's CPU, reaped, goes into the receipt of the lease whose overlay started it:
    /// its class is measured by what its build used (a box prebuild's read 0.04 CPU seconds for a
    /// 69 s build on 7 cores, and its class was charged one core, 11-11).
    #[cfg(all(unix, feature = "checkout"))]
    #[test]
    fn a_store_builds_cpu_is_in_its_leases_receipt() {
        let (x, base, _) = test_exec("storereap", Opts { scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"S1","lane":"store","kind":"test","rev":null}"#);
        let mut burn = std::process::Command::new("/bin/sh");
        burn.args(["-c", "i=0; while [ $i -lt 50000 ]; do i=$((i+1)); done"]).current_dir(&base);
        let (status, _) = x.overlay_build("S1", "burn", &burn, &|| Ok(())).unwrap();
        assert!(status.success());
        let cpu = x.leases.lock().unwrap()["S1"].store_cpu_s;
        assert!(cpu > 0.01, "its build's CPU, reaped: {cpu}");
        let r = x.handle("lease-end", "S1 released")[0].clone();
        assert_eq!(field(&r, "cpu_seconds").and_then(|v| super::super::num(&v)), Some(cpu), "{r}");
        // An earlier hub's store builds, each a group with a child as cargo's rustc: a lease's end
        // here stops the one whose lease is gone and keeps the one a live lease owns (11-16).
        x.handle("lease-start", r#"{"id":"S2","lane":"store","kind":"test","rev":null}"#);
        x.handle("lease-start", r#"{"id":"S3","lane":"store","kind":"test","rev":null}"#);
        let earlier = |id: &str, lease: &str| {
            let dir = x.conf().work.join("dev/jit/build").join(id);
            std::fs::create_dir_all(&dir).unwrap();
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.args(["-c", "sleep 30 & wait"]);
            let (child, _) = os::lease_spawn(&mut cmd, None).unwrap();
            let start = os::proc_start(child.id()).unwrap();
            std::fs::write(dir.join("cargo.pid"), format!("{} {start} {lease}\n", child.id())).unwrap();
            (child, start, dir.join("cargo.pid"))
        };
        let (mut gone, gone_start, gone_record) = earlier("gone", "S0");
        let (mut owned, _, owned_record) = earlier("owned", "S2");
        x.handle("lease-end", "S3 released");
        let leader = gone.wait().unwrap();
        assert!(!leader.success() && !os::group_alive(gone.id(), gone_start, &os::Hold::none()), "the build of an ended lease stops, its child too");
        assert!(!gone_record.exists(), "its record goes with it");
        assert!(owned.try_wait().unwrap().is_none() && owned_record.exists(), "a build a live lease owns builds on");
        os::signal_group(owned.id(), true, &os::Hold::none());
        let _ = owned.wait();
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An overlay's JIT resolving runs no process of its own: it waits on the box's store build,
    /// so what that does is what its beat measures. Past its answer it goes by its own runs again,
    /// and with no store build running there is nothing to measure.
    #[cfg(all(unix, feature = "checkout"))]
    #[test]
    fn a_resolving_overlay_lease_goes_by_the_boxs_store_builds() {
        let (x, base, _) = test_exec("store", Opts::default());
        x.handle("lease-start", r#"{"id":"S1","lane":"store","kind":"test","rev":null}"#);
        x.handle("lease-start", r#"{"id":"S2","lane":"store","kind":"test","rev":null}"#);
        x.handle("lease-run", &format!("S2 . {}", b64(&["/bin/sleep", "30"])));
        let build = (0..100).find_map(|_| {
            std::thread::sleep(Duration::from_millis(50));
            x.leases.lock().unwrap()["S2"].runs.first().cloned()
        }).expect("its run");
        // a scoped run is measurable once systemd-run has moved it into its scope and its wrapper
        // wrote the cgroup's path: on a loaded Linux box that came after the beats below (a fast
        // bucket's red, 11-14), and a beat that cannot measure the build measures nothing
        assert!((0..200).any(|_| super::activity_now(std::slice::from_ref(&build)).is_some() || { std::thread::sleep(Duration::from_millis(50)); false }), "the store build is never measurable");
        let measured = |x: &Exec| {
            let a = x.handle("lease-beat", "S1");
            let v = makepad_strict_json::parse(a[0].as_bytes()).unwrap();
            v.get("info").and_then(|i| i.get("S1")).and_then(|i| i.get("idle_s")).is_some_and(|s| s.as_u64().is_some())
        };
        let overlay = |answer: Option<&str>| {
            let mut leases = x.leases.lock().unwrap();
            let l = leases.get_mut("S1").unwrap();
            l.src = Src::Overlay;
            l.resolution = Some(OverlayResolution { request: "jit".into(), cwd: ".".into(), argv: Vec::new(), pending: "building".into(), answer: answer.map(str::to_string), reported: false, run: None });
        };
        x.store_builds.lock().unwrap().push(("S1".into(), build));
        assert!(!measured(&x), "a tree lease does not wait on the store");
        overlay(Some("{}"));
        assert!(!measured(&x), "resolved: it goes by its own runs");
        overlay(None);
        assert!(measured(&x), "resolving: it goes by the store build");
        x.store_builds.lock().unwrap().clear();
        assert!(!measured(&x), "no store build running: nothing to measure");
        x.leases.lock().unwrap().get_mut("S1").unwrap().resolution = None;
        x.handle("lease-end", "S1 released");
        x.handle("lease-end", "S2 released");
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
        let beat = makepad_strict_json::parse(x.handle("lease-beat", "H1")[0].as_bytes()).unwrap();
        let mb = |k: &str| beat.get("info").and_then(|i| i.get("H1")).and_then(|i| i.get(k)).and_then(|v| v.as_u64());
        assert!(mb("held_peak_mb").is_some_and(|peak| mb("held_mb").is_some_and(|now| peak >= now)), "what it holds now and the most it held: {beat:?}");
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
        assert!(!live[0].contains("RELAX_JOBS") && !live[0].contains("MAKEPAD_CPU_CORES") && live[0].contains("\"CARGO_BUILD_JOBS\":\"1\""), "{live:?}");
        assert!(plain[0].contains("\"RELAX_JOBS\":\"1\"") && plain[0].contains("\"MAKEPAD_CPU_CORES\":\"1\""), "{plain:?}");
        // one that asked its cores runs its JIT and its app on them, a restarted hub too
        let asked = x.handle("lease-start", r#"{"id":"L4","lane":"askx","kind":"test","rev":null,"live":true,"cost":{"cores":4,"threads":4}}"#);
        assert!(["RELAX_JOBS", "MAKEPAD_CPU_CORES", "RUST_TEST_THREADS"].iter().all(|k| asked[0].contains(&format!("\"{k}\":\"4\""))), "{asked:?}");
        assert_eq!(Lease::from_ledger(&x.leases.lock().unwrap()["L4"].ledger()).and_then(|l| l.threads), Some(4));
        // a live test that asked none: its pools take its call's charge, its budget and so its JIT
        // the box (11-18), a restarted hub too
        let pooled = x.handle("lease-start", r#"{"id":"L5","lane":"poolx","kind":"test","rev":null,"live":true,"cost":{"cores":2,"pool_cores":6}}"#);
        assert!(pooled[0].contains("\"MAKEPAD_POOL_CORES\":\"6\"") && !pooled[0].contains("RELAX_JOBS") && !pooled[0].contains("MAKEPAD_CPU_CORES"), "{pooled:?}");
        assert_eq!(Lease::from_ledger(&x.leases.lock().unwrap()["L5"].ledger()).and_then(|l| l.pool_cores), Some(6));
        assert!(!live[0].contains("MAKEPAD_POOL_CORES") && !asked[0].contains("MAKEPAD_POOL_CORES"), "none sent, or its cores asked: none {live:?} {asked:?}");
        let shard = x.handle("lease-start", r#"{"id":"L3","lane":"shardx","kind":"test","rev":null,"cost":{"cores":12,"ram_gb":30.5}}"#);
        assert!(shard[0].contains("\"DEVHUB_RAM_MB\":\"31232\""), "the RAM it got: {shard:?}");
        // a restarted hub keeps it a live session
        let ledger = x.leases.lock().unwrap()["L1"].ledger();
        assert!(Lease::from_ledger(&ledger).is_some_and(|l| l.live), "{ledger}");
        // its scope: a weight from its cores, never a quota (a quota of one core held a live Fab to one CPU)
        let weight = x.leases.lock().unwrap()["L1"].cpu_weight();
        let unit = os::lease_argv(vec!["x".into()], Some("u"), 10, "", Some(weight), None);
        assert!(!unit.iter().any(|a| a.starts_with("CPUQuota")), "{unit:?}");
        assert_eq!(unit.iter().any(|a| a == "CPUWeight=100"), cfg!(target_os = "linux"), "{unit:?}");
        assert_eq!(x.handle("lease-weight", "L1 rest"), ["{\"weight\":0,\"scopes\":0}"]);
        assert_eq!(x.leases.lock().unwrap()["L1"].cpu_weight(), 0);
        assert_eq!(Lease::from_ledger(&x.leases.lock().unwrap()["L1"].ledger()).unwrap().cpu_weight(), 0);
        let unit = os::lease_argv(vec!["x".into()], Some("u"), 10, "", Some(0), None);
        assert_eq!(unit.iter().any(|a| a == "CPUWeight=1"), cfg!(target_os = "linux"), "a run that starts at rest starts at the least weight: {unit:?}");
        x.handle("lease-weight", "L1 call");
        assert_eq!(x.leases.lock().unwrap()["L1"].cpu_weight(), weight);
    }

    /// A background lease's runs weigh a hundredth of an interactive one's or a live session's in
    /// a call a core: past its charge it yields the box to them (live calls waited 96-125 s
    /// behind a prebuild and a gate running past theirs, 11-06). A restarted hub keeps the weight.
    #[test]
    fn a_background_lease_yields_the_box_to_interactive_work() {
        let (x, _base, _events) = test_exec("weights", Opts::default());
        for (id, lane, extra) in [("B1", "bgx", r#""priority":"background""#), ("I1", "fgx", r#""priority":"interactive""#), ("V1", "livex", r#""priority":"background","live":true"#), ("N1", "nonex", r#""live":false"#)] {
            let a = x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"{lane}","kind":"test","rev":null,{extra},"cost":{{"cores":8}}}}"#));
            assert!(!a[0].starts_with("{\"err\""), "{a:?}");
        }
        let weights = |x: &Exec| ["B1", "I1", "V1", "N1"].map(|id| x.leases.lock().unwrap()[id].cpu_weight());
        assert_eq!(weights(&x), [8, 800, 800, 8]);
        let saved: Vec<Lease> = ["B1", "I1"].iter().map(|id| Lease::from_ledger(&x.leases.lock().unwrap()[*id].ledger()).unwrap()).collect();
        assert_eq!(saved.iter().map(Lease::cpu_weight).collect::<Vec<_>>(), [8, 800]);
        assert_eq!(super::super::Priority::Background.cpu_weight(0.25), 1);
        // a live session weighs as background work between calls, as interactive work in one:
        // an idle session that spins slows no gate beside it; a restarted hub keeps the word
        assert_eq!(x.handle("lease-weight", "V1 idle"), ["{\"weight\":8,\"scopes\":0}"]);
        assert_eq!(Lease::from_ledger(&x.leases.lock().unwrap()["V1"].ledger()).unwrap().cpu_weight(), 8);
        assert_eq!(x.handle("lease-weight", "V1 call"), ["{\"weight\":800,\"scopes\":0}"]);
        assert!(x.handle("lease-weight", "B1 idle")[0].contains("B1 is no live session"));
        assert!(x.handle("lease-weight", "V1 later")[0].contains("wants `<id> call|idle|rest`"));
        assert_eq!(weights(&x), [8, 800, 800, 8]);
    }

    #[test]
    fn interpreter_script_words_keep_c_mode() {
        let stem = Path::new("scratch/a'b\\c-script");
        let shell_word = ". 'scratch/a'\\''b\\c-script.sh'";
        let python_word = "exec(compile(open('scratch/a\\'b\\\\c-script.py','rb').read(),'scratch/a\\'b\\\\c-script.py','exec'))";
        for (args, index, extension, word) in [
            (vec!["sh", "-c", "code", "name", "one"], 2, "sh", shell_word),
            (vec!["/bin/bash", "-lc", "code"], 2, "sh", shell_word),
            (vec!["dash", "-ec", "code"], 2, "sh", shell_word),
            (vec!["zsh", "-euc", "code"], 2, "sh", shell_word),
            (vec!["ksh", "-c", "code"], 2, "sh", shell_word),
            (vec!["ash", "-c", "code"], 2, "sh", shell_word),
            (vec!["bash", "-o", "pipefail", "-c", "code"], 4, "sh", shell_word),
            (vec!["python", "-c", "code"], 2, "py", python_word),
            (vec!["python3", "-uc", "code", "one"], 2, "py", python_word),
            (vec!["python3.12", "-Bc", "code"], 2, "py", python_word),
        ] {
            let mut argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            let (i, path, replacement) = script_spill(&argv, stem).expect("script slot");
            assert_eq!((i, path, replacement.as_str()), (index, stem.with_extension(extension), word), "{args:?}");
            argv[i] = replacement;
            for j in 0..argv.len() {
                if j != i { assert_eq!(argv[j], args[j], "non-script word {j} of {args:?}"); }
            }
        }
        for args in [vec!["echo", "-c", "code"], vec!["python3", "-cu", "code"], vec!["python-tool", "-c", "code"], vec!["sh", "--command", "code"], vec!["sh", "-c1", "code"], vec!["sh", "script.sh"]] {
            let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            assert!(script_spill(&argv, stem).is_none(), "not a script slot: {args:?}");
        }
        let argv = ["sh", "-c", "small", "-c", "largest script"].map(String::from);
        assert_eq!(script_spill(&argv, stem).unwrap().0, 4, "only the largest candidate spills");
    }

    #[cfg(unix)]
    #[test]
    fn large_script_runs_from_a_file_and_large_env_starts_no_run() {
        let (x, base, _) = test_exec("large-script", Opts::default());
        let a = x.handle("lease-start", r#"{"id":"L1","lane":"large-script","kind":"test","rev":null}"#);
        assert_eq!(field(&a[0], "ok").and_then(|v| super::super::num(&v)), Some(1.0), "{a:?}");
        let tmp = PathBuf::from(field(&a[0], "tmp").unwrap().as_str().unwrap());
        let script = format!("#{}\nprintf '%s %s\\n' \"$0\" \"$1\"\nprintf 'large-script-done\\n'\n", "x".repeat(1536 * 1024));
        let r = x.handle("lease-run", &format!("L1 . {}", b64(&["sh", "-c", &script, "name", "one"])));
        let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
        let (text, code) = log_until_exit(&x, "L1", n);
        assert_eq!((text.as_str(), code), ("name one\nlarge-script-done\n", 0), "sourcing preserves the shell's positional parameters");
        assert_eq!(std::fs::read(tmp.join(format!("run-{n}-script.sh"))).unwrap(), script.as_bytes(), "the file preserves every script byte");
        let count = x.leases.lock().unwrap()["L1"].runs.len();
        let big = format!("BIG={}", "x".repeat(1536 * 1024));
        let r = x.handle("lease-run", &format!("L1 . {}", b64(&["env", &big, "true"])));
        let error = field(&r[0], "err").unwrap();
        let error = error.as_str().unwrap();
        assert!(error.starts_with("lease-run L1: env value BIG") && error.contains("at most") && error.contains("pass it in a file"), "{error}");
        assert_eq!(x.leases.lock().unwrap()["L1"].runs.len(), count, "refusing an oversized env starts no run");
        drop(x);
        let _ = std::fs::remove_dir_all(&base);
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
        assert!(!tmp.join(format!("run-{n}-script.sh")).exists(), "a command that fits has no spill file");
        // the env reaches the program; a failing one reports its code
        let r = x.handle("lease-run", &format!("lease-run L1 . {}", b64(ENV)));
        let (env, _) = log_until_exit(&x, "L1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32);
        assert!(env.contains("FOO=bar") && env.contains("MAKEPAD_MUTE=1") && env.contains(&format!("TMPDIR={}", tmp.display())) && env.contains("CARGO_BUILD_JOBS=4") && env.contains("RUST_TEST_THREADS=4"));
        // a failed step's logs have a place in the lease's tmp, listed for its holder while it lives
        assert!(env.contains(&format!("DEVHUB_KEEP={}", tmp.join(crate::lease::KEEP_DIR).display())) && tmp.join(crate::lease::KEEP_DIR).is_dir(), "{env}");
        assert_eq!(x.handle("get-ls", "gapx L1 tmp/keep"), Vec::<String>::new());
        std::fs::create_dir_all(tmp.join("keep/landgate.x")).unwrap();
        std::fs::write(tmp.join("keep/landgate.x/out.log"), "boom\n").unwrap();
        assert_eq!(x.handle("get-ls", "gapx L1 tmp/keep"), vec![r#"{"p":"keep/landgate.x/out.log","n":5,"r":"tmp/keep/landgate.x/out.log"}"#.to_string()]);
        // every run knows its own lease and lane: its verdict lines say whose they are
        assert!(env.contains("DEVHUB_LEASE=L1") && env.contains("MAKEPAD_TEST_LANE=gapx") && env.contains("MAKEPAD_JIT_SESSION=L1"), "{env}");
        // app tests of two leases of one lane never share an artifacts dir or app home; it is
        // there before the first run, which may write into it itself
        assert!(env.lines().filter_map(|l| l.strip_prefix("MAKEPAD_TEST_ARTIFACTS=")).any(|v| Path::new(v).ends_with("makepad_test/L1") && Path::new(v).is_dir()), "{env}");
        // nothing of the hub's own environment beyond the basics
        assert!(env.lines().all(|l| !l.starts_with("CARGO_PKG_") && (!l.starts_with("RUST_TEST_") || l == "RUST_TEST_THREADS=4")), "{env}");
        let r = x.handle("lease-run", &format!("L1 . {}", b64(FAIL)));
        assert_eq!(log_until_exit(&x, "L1", field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap() as u32).1, 1);
        // a Windows run is plain argv: its PowerShell script runs from a file, its process under
        // RemoteSigned, under -Command's exit (a failed last step fails it), and cmd is refused,
        // saying why
        #[cfg(windows)]
        {
            let r = x.handle("lease-run", &format!("L1 . {}", b64(&["powershell", "-NoProfile", "-Command", "Write-Output \"from-file $(Get-ExecutionPolicy)\"; where.exe devhub-no-such-program"])));
            let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
            let (text, code) = log_until_exit(&x, "L1", n);
            assert!(text.contains("from-file RemoteSigned") && code == 1, "{code} {text}");
            assert!(std::fs::read_to_string(tmp.join(format!("run-{n}-script.ps1"))).unwrap().contains("where.exe devhub-no-such-program"));
            let r = x.handle("lease-run", &format!("L1 . {}", b64(&["cmd", "/c", "echo hi"])));
            assert!(r[0].contains("never cmd"), "{r:?}");
        }
        assert!(x.handle("lease-run", &format!("L1 ../x {}", b64(&["true"])))[0].contains("err"));
        // asked from a directory the tree lacks (a shell in an ignored folder): that, not the program
        let missing = x.handle("lease-run", &format!("L1 local/shots {}", b64(&["true"])));
        assert!(missing[0].contains("has no local/shots") && !missing[0].contains("No such file"), "{missing:?}");

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
            // its wrapper names the command once it started it (its pid file): on a loaded box after
            // the 200 ms above, when the run read as its own wrapper (two fast buckets, 11-09)
            let run = x.leases.lock().unwrap()["L1"].runs.last().unwrap().clone();
            while run.app_pid().is_none() {
                assert!(os::group_alive(pid, start, &os::Hold::none()), "the sleep's wrapper ended without starting its command");
                std::thread::sleep(Duration::from_millis(20));
            }
            let work = run.work_pid();
            let own = adj(std::process::id());
            assert!(work != pid && adj(work) == "500" && adj(pid) == own, "{work} {} {pid} {} own {own}", adj(work), adj(pid));
        }
        // the box's residents: what is in use less the run's own, never more than the box holds,
        // and less the hub's own build's: its lease charges it (W12708, 11-18)
        #[cfg(unix)]
        {
            let resident = x.resident_gb();
            assert!(resident > 0.0 && resident * 1024.0 <= crate::procinfo::machine().mem_total_mb as f64, "{resident}");
            use std::os::unix::process::CommandExt;
            let mut build = std::process::Command::new(SLEEP[0]).args(&SLEEP[1..]).process_group(0).spawn().unwrap();
            HUB_BUILD_GROUP.store(build.id(), Ordering::Release);
            let held = builds_held();
            HUB_BUILD_GROUP.store(0, Ordering::Release);
            let _ = build.kill();
            let _ = build.wait();
            assert!(held.is_some_and(|b| b > 0), "the hub build's group holds RAM, its lease's, no resident's: {held:?}");
        }
        let beat = x.handle("lease-beat", "L1 L404");
        assert!(beat[0].starts_with(&format!(r#"{{"live":["L1"],"unknown":["L404"],"info":{{"L1":{{"running":"{SLEEP_LABEL}","pid":"#)), "{beat:?}");
        // a stalled lease's JITs: none under a plain run
        assert_eq!(x.handle("lease-jits", "L1"), vec![r#"{"pids":[],"jits":[]}"#.to_string()]);
        assert!(x.handle("lease-jits", "L404")[0].contains("no lease L404"));
        // a process its run started in a group of its own (a JIT-run test leads one) is the
        // lease's by its mark, which a stall's capture, `lease <id> why` and `lease <id> jits` read
        #[cfg(unix)]
        {
            let file = tmp.join("own-group.pid");
            let script = format!("set -m; sleep 60 & echo $! > {}; wait", file.display());
            let r = x.handle("lease-run", &format!("L1 . {}", b64(&["sh", "-c", &script])));
            let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
            let t = Instant::now();
            let own = loop {
                if let Some(p) = std::fs::read_to_string(&file).ok().and_then(|t| t.trim().parse::<u32>().ok()) { break p; }
                assert!(t.elapsed() < Duration::from_secs(10), "the run named no sleep");
                std::thread::sleep(Duration::from_millis(10));
            };
            let runs = x.leases.lock().unwrap()["L1"].runs.clone();
            assert!(!runs.iter().any(|r| r.pid != 0 && os::in_group(own, r.pid, &r.hold)), "the sleep leads a group of its own");
            assert!(lease_pids(&x, "L1", &runs).contains(&own), "the lease's by its mark");
            // its master's look at it each beat (a live session's process): it runs, then it is gone
            let looked = x.handle("lease-pid", &format!("L1 {own}"));
            let start = field(&looked[0], "start").and_then(|v| super::super::num(&v)).expect(&looked[0]) as u64;
            assert!(looked[0].starts_with(r#"{"alive":true,"#) && start > 0, "{looked:?}");
            assert_eq!(x.handle("lease-pid", &format!("L1 {own} {start}")), looked, "the same process");
            assert_eq!(x.handle("lease-pid", &format!("L1 {own} {}", start + 1)), vec![r#"{"alive":false,"start":0}"#.to_string()], "another process under its pid");
            assert!(x.handle("lease-pid", &format!("L404 {own}"))[0].contains("no lease L404"));
            os::signal_one(own, true);
            log_until_exit(&x, "L1", n);
            assert_eq!(x.handle("lease-pid", &format!("L1 {own} {start}")), vec![r#"{"alive":false,"start":0}"#.to_string()], "killed: gone");
        }
        // a gate's unit leads a session of its own beside its watch (land_gate.sh `watched`), and may leave
        // its run's groups and its mark: the session its watch names is the lease's, and once no process of
        // it is left, `lease <id> why` says the unit ended (a gate sat 10 min on the watch of a unit that had
        // ended, its stall's capture naming no unit, L1483898, 11-21)
        #[cfg(target_os = "linux")]
        {
            let file = tmp.join("unit.pid");
            // (the unit's starter gone before its pid is named: the unit is no descendant of the run)
            let script = format!("(env -i setsid sleep 60 > /dev/null 2>&1 & echo $! > {f}.new); mv {f}.new {f}; sh -c 'sleep 60; :' unit_watch.sh $(cat {f})", f = file.display());
            let r = x.handle("lease-run", &format!("L1 . {}", b64(&["sh", "-c", &script])));
            let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
            let runs = || x.leases.lock().unwrap()["L1"].runs.clone();
            let t = Instant::now();
            let unit = loop {
                if let Some(p) = std::fs::read_to_string(&file).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
                    if os::watched_sessions(&lease_pids(&x, "L1", &runs())).iter().any(|(s, _)| *s == p) { break p; }
                }
                assert!(t.elapsed() < Duration::from_secs(10), "no watch of a unit among the lease's processes");
                std::thread::sleep(Duration::from_millis(10));
            };
            let outside = !runs().iter().filter(|r| r.pid != 0).any(|r| os::group_pids(r.pid, r.start, &r.hold).contains(&unit))
                && !os::with_env(&format!("{LEASE_ENV}={}", lease_mark(&x.state, "L1"))).contains(&unit);
            assert!(outside, "the unit is out of its run's groups and carries no mark");
            assert!(lease_pids(&x, "L1", &runs()).contains(&unit), "the unit its watch names is the lease's");
            os::signal_one(unit, true);
            let t = Instant::now();
            while !os::watched_sessions(&lease_pids(&x, "L1", &runs())).iter().any(|(s, live)| *s == unit && live.is_empty()) {
                assert!(t.elapsed() < Duration::from_secs(10), "the killed unit's session still has a process");
                std::thread::sleep(Duration::from_millis(10));
            }
            let why = x.handle("lease-why", "L1");
            assert!(why[0].contains(&format!("its unit {unit} has ended (no process of its session is left), and its watch still runs")), "{why:?}");
            let leader = runs().iter().find(|r| r.n == n).map(|r| r.pid).unwrap();
            let _ = std::process::Command::new("kill").args(["-KILL", "--", &format!("-{leader}")]).status();
            log_until_exit(&x, "L1", n);
        }
        // its yield: the file its env names, why in it
        assert_eq!(x.handle("lease-yield", "L1 a timed ask waits"), vec!["{}".to_string()]);
        let named = field(&a[0], "env").and_then(|e| e.get("DEVHUB_YIELD").and_then(|v| v.as_str()).map(PathBuf::from)).expect(&a[0]);
        assert_eq!(std::fs::read_to_string(&named).unwrap(), "a timed ask waits\n");
        assert!(x.handle("lease-yield", "L404 x")[0].contains("no lease L404"));
        // what its holder tells it: each line appended to the file its env names
        assert_eq!(x.handle("lease-tell", "L1 1 4 3f2a"), vec!["{}".to_string()]);
        assert_eq!(x.handle("lease-tell", "L1 2 -"), vec!["{}".to_string()]);
        let told = field(&a[0], "env").and_then(|e| e.get("DEVHUB_TOLD").and_then(|v| v.as_str()).map(PathBuf::from)).expect(&a[0]);
        assert_eq!(std::fs::read_to_string(&told).unwrap(), "1 4 3f2a\n2 -\n");
        assert!(x.handle("lease-tell", "L404 1 -")[0].contains("no lease L404"));
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
        let artifacts = env.lines().find_map(|l| l.strip_prefix("MAKEPAD_TEST_ARTIFACTS=")).map(PathBuf::from).expect(&env);
        assert!(!artifacts.exists(), "an artifacts dir nothing was kept in goes with its lease");
        assert_eq!(receipt(x.handle("lease-end", "L1 released")), receipt(e), "duplicate end returns its cleanup receipt");
        assert!(events.lock().unwrap().iter().any(|l| l.contains("ws_create")) && events.lock().unwrap().iter().any(|l| l.contains("ended on t (released)")));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Without a lease, a lane's get reads its own native tree only: with that tree gone (another
    /// lane's lease took it), it says so and how a run's outputs come back, and never reads the
    /// box's shared tree, which only CI and landings work in.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_lanes_get_never_reads_the_shared_tree() {
        let base = std::env::temp_dir().join(format!("devhub-get-shared-{}", std::process::id()));
        let (x, exec_base, _) = test_exec("getshared", Opts { tree: Some(base.join("ci-tree")), ..Opts::default() });
        std::fs::create_dir_all(base.join("ci-tree/local/out")).unwrap();
        std::fs::write(base.join("ci-tree/local/out/a.txt"), "a gate's").unwrap();
        let gone = x.get_root("sheetx", "-", "src/local/out").unwrap_err();
        assert!(gone.contains("lane sheetx has no tree on") && gone.contains("--keep <dir>") && gone.contains("$DEVHUB_KEEP"), "{gone}");
        assert!(x.handle("get-ls", "sheetx - src/local/out")[0].contains("err"));
        let (root, pre, sub) = x.get_root(super::super::sched::CI_LANE, "-", "src/local/out").unwrap();
        assert_eq!((root, pre, sub.as_str()), (base.join("ci-tree"), "src/", "local/out"), "CI works in the shared tree");
        assert!(shared_tree_lane("sheetx", super::super::Role::UserWm) && !shared_tree_lane("sheetx", super::super::Role::Pool));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&exec_base);
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
        let (root, pre, sub) = x.get_root("anylane", "-", "captures/20261110T075954Z-app-7.mkpf").unwrap();
        assert_eq!((root, pre, sub.as_str()), (makepad_remote::hot::home().join("captures"), "captures/", "20261110T075954Z-app-7.mkpf"), "the box's captures, for any lane, without a lease");
        assert!(x.get_root("getx", "L2", "captures/../lane").is_err(), "nothing outside them");
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
        let (x, exec_base, _) = test_exec("own-sources", Opts { scopes: Some(false), ..Opts::default() });
        std::fs::create_dir_all(x.conf().work.join("dev/git")).unwrap();

        // a ship into a lease that holds one already: a live save
        let save = |id: &str, files: &[(&str, &str)], deleted: &[&str]| {
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
        let ship = |id: &str, files: &[(&str, &str)], deleted: &[&str]| {
            let a = x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"lanex","kind":"test","src":"overlay","rev":{{"makepad":"{pin}","commercial":null}}}}"#));
            assert!(!a[0].starts_with("{\"err\""), "{a:?}");
            save(id, files, deleted)
        };
        let one = ship("O1", &[("a.rs", "one a"), ("b.rs", "one b")], &[]);
        let two = ship("O2", &[("a.rs", "two a"), ("c.rs", "two c")], &["b.rs"]);
        // what each run reads on top of the pin: the folder in its own environment
        let edits = |id: &str| x.leases.lock().unwrap()[id].env.iter().find(|(k, _)| k == "RELAX_EDITS").map(|(_, v)| PathBuf::from(v)).unwrap();
        let read = |id: &str, p: &str| std::fs::read_to_string(edits(id).join(p)).ok();
        let gone = |id: &str| std::fs::read_to_string(edits(id).join(".makepad-run-deleted")).unwrap_or_default();
        assert_eq!((read("O1", "a.rs").as_deref(), read("O1", "b.rs").as_deref(), read("O1", "c.rs")), (Some("one a"), Some("one b"), None), "the first lease's source, after the second's ship");
        assert_eq!(gone("O1"), "", "the second's deletion is not the first's");
        assert_eq!((read("O2", "a.rs").as_deref(), read("O2", "b.rs"), read("O2", "c.rs").as_deref()), (Some("two a"), None, Some("two c")));
        assert_eq!(gone("O2"), "b.rs\n");
        // what the program writes in its virtual folders is in its overlay, with the VFS's record
        // (`new`) or whiteout of it: a save's ship takes back only what an earlier ship put there
        // and this one does not ship (c.rs, b.rs's whiteout), never the program's own (a live
        // session lost its asset store and its game's session at its next save, 11-22)
        let own = edits("O2").join("local/store/objects/x1");
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, "the app's").unwrap();
        std::fs::create_dir_all(edits("O2").join("local/session/w1")).unwrap();
        let ledger = |id: &str| std::fs::read_to_string(edits(id).join(".makepad-run-edits")).unwrap_or_default();
        std::fs::write(edits("O2").join(".makepad-run-edits"), ledger("O2") + "local/store/objects/x1\tnew\n").unwrap();
        std::fs::write(edits("O2").join(".makepad-run-deleted"), gone("O2") + "z.rs\n").unwrap();
        save("O2", &[("a.rs", "three a")], &[]);
        assert_eq!((read("O2", "a.rs").as_deref(), read("O2", "c.rs")), (Some("three a"), None), "the save's file, and the earlier ship's it does not ship is gone");
        assert_eq!(read("O2", "local/store/objects/x1").as_deref(), Some("the app's"), "the program's file stays");
        assert!(edits("O2").join("local/session/w1").is_dir(), "and the folder it made");
        assert!(ledger("O2").lines().any(|l| l == "local/store/objects/x1\tnew") && ledger("O2").lines().any(|l| l.starts_with("a.rs\t")) && !ledger("O2").contains("c.rs"), "the program's record beside the ship's: {}", ledger("O2"));
        assert_eq!(gone("O2"), "z.rs\n", "the program's whiteout stays; the earlier ship's goes");
        // a look (`run --look`): a command that is no JIT cargo run runs as given, in the overlay's
        // run folder (a node refused it: "overlay: run requires the JIT cargo invocation", 11-22)
        if cfg!(unix) {
            let r = x.handle("lease-run", &format!("O2 . {}", b64(&["sh", "-c", "echo look"])));
            let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).unwrap_or_else(|| panic!("a look runs on an overlay: {r:?}")) as u32;
            let (log, exit) = log_until_exit(&x, "O2", n);
            assert_eq!((log.trim_end().lines().last(), exit), (Some("look"), 0), "{log}");
        }
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
        assert_eq!(read("O2", "a.rs").as_deref(), Some("three a"));
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
    fn a_hung_unit_is_named_by_its_line() {
        assert_eq!(hung_line("test a: ok\ntest b: HUNG (no output 300 s): stopped; 2 passed, 0 failed before it)\nHUNG: m::t::waits; its stacks: testrun-b.hung\n"), Some("HUNG: m::t::waits; its stacks: testrun-b.hung"));
        assert_eq!(hung_line("test a: ok\nNODEGATE-OK\n"), None);
    }

    #[test]
    fn a_gate_scripts_verdict_is_its_summary() {
        assert!(said_its_summary(Kind::Test, "test x ... ok\ntest result: FAILED. 1 passed; 1 failed\n"));
        assert!(said_its_summary(Kind::Test, "test fab: FAILED (exit 101; 2 passed, 1 failed)\nNODEGATE-FAIL: test:fab [lease L1]\n"));
        assert!(said_its_summary(Kind::Test, "NODEGATE-OK\n"));
        assert!(!said_its_summary(Kind::Test, "Compiling fab\nerror[E0425]: cannot find value `x`\n"), "a build that broke said no verdict");
        assert!(said_its_summary(Kind::Build, "error[E0425]\n"), "a build's exit is its verdict");
    }

    #[test]
    fn an_abnormal_end_says_how_it_ended() {
        let kernel = "memory: usage 2729056kB, limit 2729056kB, failcnt 56383\noom-kill:constraint=CONSTRAINT_MEMCG,nodemask=(null),oom_memcg=/app.slice/devhub-lease-L7-1.scope,task_memcg=/app.slice/devhub-lease-L7-1.scope,task=makepad.jit-256,pid=42\nMemory cgroup out of memory: Killed process 42 (makepad.jit-256) total-vm:401088916kB, anon-rss:2455836kB, file-rss:573996kB, UID:1000\nNVRM: Xid (PCI:0000:01:00): 79, GPU has fallen off the bus.";
        assert_eq!(ended_how("L7", 1, Some("devhub-lease-L7-1"), 137, false, Some(kernel)), ("L7 run 1 ended without its summary: killed by signal 9 (SIGKILL); the kernel's OOM killer killed 42 (makepad.jit-256), 2.3 GB resident, at the lease's memory cap of 2.6 GB; GPU: NVRM: Xid (PCI:0000:01:00): 79, GPU has fallen off the bus.".to_string(), true));
        // another lease's OOM kill is not this run's; a plain exit is no more than a failure
        assert_eq!(ended_how("L8", 2, Some("devhub-lease-L8-2"), 101, false, Some(&kernel[..kernel.find("NVRM").unwrap()])), ("L8 run 2 ended without its summary: exit 101; no OOM kill".to_string(), false));
        assert_eq!(ended_how("L9", 1, None, -1, true, None).0, "L9 run 1 ended without its summary: its exit is unknown (its hub restarted while it ran, and its wrapper wrote no exit: it was killed too); the kernel log was not read here");
        assert_eq!(ended_how("L9", 2, None, -1, false, None).0, "L9 run 2 ended without its summary: its exit is unknown (its hub could not reap it); the kernel log was not read here", "a run of this hub's never says its hub restarted");
        let log = std::env::temp_dir().join(format!("devhub-tail-{}.log", std::process::id()));
        std::fs::write(&log, (1..=30).map(|i| format!("line {i}\n")).collect::<String>()).unwrap();
        let tail = log_tail(&log, TAIL_LINES);
        let _ = std::fs::remove_file(&log);
        assert!(tail.starts_with("line 11\n") && tail.ends_with("line 30") && tail.lines().count() == 20, "{tail}");
    }

    #[test]
    fn the_lease_furthest_past_its_memory_weight_ends_first() {
        const GB: u64 = 1 << 30;
        let took = |id: &str, held: u64, weighed: f64| MemTook { id: id.into(), lane: "lane-x".into(), kind: Kind::Screen, held: held * GB, weighed, peak: None, asked: false };
        // (the toolchain verdict of a lease's end: a change under it, not its own, not unread)
        let rustc = |release: &str, commit: &str| caps::Rustc { release: release.into(), commit: commit.into() };
        assert_eq!(toolchain_changed("1.99.1 58cf8f9c3", false, &rustc("1.99.1", "58cf8f9c3")), None);
        assert_eq!(toolchain_changed("1.99.1 58cf8f9c3", false, &rustc("1.100.0", "0ef1bc386")).as_deref(), Some("rustc 1.99.1 -> rustc 1.100.0"));
        assert_eq!(toolchain_changed("1.99.1 58cf8f9c3", false, &rustc("", "")).as_deref(), Some("rustc 1.99.1 -> none (rustc did not answer)"), "rustc gone mid-swap");
        assert_eq!(toolchain_changed("1.99.1 58cf8f9c3", true, &rustc("1.100.0", "0ef1bc386")), None, "the lease changed it itself");
        assert_eq!(toolchain_changed("", false, &rustc("1.100.0", "0ef1bc386")), None, "its start read none");
        let pick = |avail, leases: &[MemTook]| lease_over_memory("box-a", avail, 3 * GB, leases);
        let leases = [took("L7", 22, 4.0), took("L8", 24, 24.0)];
        assert_eq!(pick(3 * GB, &leases), None, "at the floor no lease ends");
        assert_eq!(pick(4 * GB, &leases), None, "above the floor no lease ends");
        let (id, why) = pick(2 * GB, &leases).unwrap();
        assert_eq!(id, "L7", "the lease within its weight is never picked, even if it holds more");
        assert!(why.contains("L7 (screen of lane-x) ended on box-a") && why.contains("held 22.0 GB") && why.contains("4.0 GB it was weighed at") && why.contains("box-a had 2.0 GB free, under its 3.0 GB floor"), "the verdict names the lease, box, held RAM, weight and pressure: {why}");
        assert!(why.contains("its class had no measured peak yet: its kind's default"), "an unmeasured class names the kind default: {why}");
        assert_eq!(pick(2 * GB, &[took("L1", 4, 4.0), took("L2", 3, 4.0)]), None, "none past its weight means no verdict");
        assert_eq!(pick(2 * GB, &[]), None, "no leases means no verdict");
        assert_eq!(pick(2 * GB, &[took("L1", 22, 20.0), took("L2", 12, 4.0)]).unwrap().0, "L2", "the largest excess wins, not the largest resident set");
        let mut measured = took("L3", 10, 9.0);
        measured.peak = Some(7.5);
        assert!(pick(2 * GB, &[measured.clone()]).unwrap().1.contains("its class's measured peak 7.5 GB and a fifth"), "a measured weight names its raw peak and margin");
        measured.asked = true;
        assert!(pick(2 * GB, &[measured]).unwrap().1.contains("its own --ram-gb ask"), "an explicit ask takes precedence over its class's peak");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_memory_end_kills_the_run_and_tells_its_holder_and_master() {
        const GB: u64 = 1 << 30;
        let (x, base, events) = test_exec("memory", Opts { scopes: Some(false), ..Opts::default() });
        let ok = |l: &[String]| !l[0].starts_with("{\"err\"");
        assert!(ok(&x.handle("lease-start", r#"{"id":"L7","lane":"lane-x","kind":"screen","rev":null,"cost":{"weighed_gb":0,"peak_gb":7.5}}"#)), "the lease starts with an explicit zero weight");
        let saved = Lease::from_ledger(&x.leases.lock().unwrap()["L7"].ledger()).unwrap();
        assert_eq!((saved.weighed_gb, saved.peak_gb), (0.0, Some(7.5)), "the weight and raw peak survive the ledger");
        x.handle("lease-run", &format!("L7 . {}", b64(&["/bin/sleep", "30"])));
        let n = 1;
        // (a look before the run's process is there measures nothing and ends nothing)
        let ended = (0..100).any(|_| {
            x.look_over_memory(16 * GB, GB);
            x.leases.lock().unwrap()["L7"].memory_end.is_some() || { std::thread::sleep(Duration::from_millis(50)); false }
        });
        assert!(ended, "a lease past its weight under the floor is ended for memory");
        let (_, code) = log_until_exit(&x, "L7", n);
        assert_ne!(code, 0, "the memory watch kills the sleeping run");
        let answer = x.handle("lease-beat", "L7");
        let memory = field(&answer[0], "memory").and_then(|m| m.as_arr().and_then(|a| a.first()).cloned()).expect("the master gets the memory verdict");
        assert_eq!(memory.get("id").and_then(|v| v.as_str()), Some("L7"), "the beat names the ended lease");
        assert!(memory.get("why").and_then(|v| v.as_str()).is_some_and(|w| w.contains("It held it in: ") && w.contains("`/bin/sleep 30`")), "the memory end names the process that held it: {memory:?}");
        let log = x.handle("lease-log", &format!("L7 {n} 0"));
        assert_eq!(field(&log[0], "memory").and_then(|v| v.as_str().map(str::to_string)), memory.get("why").and_then(|v| v.as_str()).map(str::to_string), "the holder gets the same reason without waiting for the master");
        let saved = Lease::from_ledger(&x.leases.lock().unwrap()["L7"].ledger()).unwrap();
        assert!(saved.memory_end.is_some() && saved.peak_ram_bytes.is_some_and(|p| p > 0), "the reason and observed peak survive a hub restart");
        x.look_over_memory(16 * GB, GB);
        assert_eq!(events.lock().unwrap().iter().filter(|s| s.contains("ended on") && s.contains("for memory:")).count(), 1, "a second look does not pick the ended lease again");
        let beat = x.handle("lease-beat", "L7");
        assert_eq!(field(&beat[0], "memory").and_then(|m| m.as_arr().map(|a| a.len())), Some(1), "the beat says it again until the master ends the lease: {beat:?}");
        let receipt = x.handle("lease-end", "L7 released");
        assert_eq!(field(&receipt[0], "memory").and_then(|v| v.as_str().map(str::to_string)), memory.get("why").and_then(|v| v.as_str()).map(str::to_string), "the end's receipt carries the memory end, so a release that came first still records it: {receipt:?}");
        drop(x);
        let _ = std::fs::remove_dir_all(&base);
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
        x.guard.set_policy(((free >> 30) + 2) as f64, false);
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
        // before its end, the master asks for its failed runs, and reads each whole (11-20)
        let failed_runs = x.handle("lease-failed", "L1");
        let runs = field(&failed_runs[0], "runs").and_then(|r| r.as_arr().map(|a| a.to_vec())).unwrap();
        assert_eq!(runs.len(), 1, "the failed run alone: {failed_runs:?}");
        assert_eq!((field(&runs[0].to_json(), "n").and_then(|v| super::super::num(&v)), field(&runs[0].to_json(), "exit").and_then(|v| super::super::num(&v))), (Some(1.0), Some(101.0)));
        assert!(field(&runs[0].to_json(), "bytes").and_then(|v| super::super::num(&v)).is_some_and(|b| b > 50.0), "{failed_runs:?}");
        assert!(x.handle("lease-failed", "L9")[0].contains("no lease L9"));
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

    /// A run that went quiet: its holder reads it on its own lease (where it waits and its last
    /// step too), and only a lease something else ended keeps how far it got.
    #[cfg(unix)]
    #[test]
    fn a_hung_run_can_be_read_and_a_cancel_keeps_its_last_lines() {
        let (x, base, _events) = test_exec("why", Opts::default());
        let hang = |id: &str| {
            assert!(x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"t","kind":"test","rev":null}}"#))[0].contains("\"ok\":1"));
            let hung: &[&str] = &["/bin/sh", "-c", "echo 'STEP build 3 s [1-4]'; echo 'test a::hung has been running for over 60 seconds'; exec sleep 100"];
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
        let said = x.handle("lease-why", "WH1")[0].clone();
        let why = field(&said, "text").and_then(|v| v.as_str().map(str::to_string)).unwrap();
        assert!(why.contains("run 1: still running\nrunning for over a minute: a::hung\n"), "{why}");
        assert_eq!(field(&said, "step").and_then(|v| v.as_str().map(str::to_string)).as_deref(), Some("STEP build 3 s [1-4]"));
        #[cfg(target_os = "linux")]
        {
            assert!(why.contains("sleep 100"), "{why}");
            let stuck = field(&said, "stuck").unwrap();
            assert!(stuck.get("chain").and_then(|c| c.as_str()).is_some_and(|c| c.ends_with("sleep 100")), "{said}");
            assert!(stuck.get("pid").and_then(super::super::num).is_some_and(|p| p > 0.0), "{said}");
        }
        let kept = field(&end(r#"WH1 released {"tail":true}"#), "tail").and_then(|v| v.as_str().map(str::to_string)).unwrap();
        assert!(kept.starts_with("run 1: still running\nrunning for over a minute: a::hung\n") && kept.contains("test a::hung has been"), "{kept}");
        hang("WH2");
        assert!(field(&end("WH2 released"), "tail").is_some_and(|v| v.is_null()));
        assert!(x.handle("lease-why", "L404")[0].contains("no lease L404"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A beat's contact beats the leases it names: its look at their runs comes after and takes a
    /// while on a loaded box, and a watchdog tick in between must not find the master there and
    /// them unbeaten (a master restart test lost three live leases so, 11-05).
    #[test]
    fn a_beat_beats_its_leases_before_its_look() {
        let (x, base, _) = test_exec("bl", Opts { tick: Duration::from_secs(3600), ..Opts::default() });
        assert!(x.handle("lease-start", r#"{"id":"L5","lane":"a","kind":"long","rev":null}"#)[0].contains("\"ok\":1"));
        let t = x.leases.lock().unwrap()["L5"].beat;
        let no_beat = x.opts.no_beat;
        x.master_contact(t, &["L5"]);
        // the next beat's contact, its look still going when the tick comes
        x.master_contact(t + no_beat * 9 / 10, &["L5"]);
        x.expire_at(t + no_beat + no_beat / 2);
        assert!(x.leases.lock().unwrap().contains_key("L5"), "beaten by its contact");
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
        let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: 0, ended_ms: AtomicU64::new(0), n: 1, pid, start, unit: None, log: base.join("run-1.log"), exit: Mutex::new(None), ours: true, detachable: false, hold, exit_file: exit, cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
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
        let (gone, stays, jit) = (spawn(true), spawn(false), spawn(true));
        let t = Instant::now();
        while !os::with_env(&mark).contains(&gone) || !os::with_env(&mark).contains(&jit) {
            assert!(t.elapsed() < Duration::from_secs(30), "the stray did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(os::pid_alive(gone) && os::pid_alive(stays) && os::pid_alive(jit));
        // a live session's stop of its JIT, which leads a group of its own (a JIT-run test): killed
        // by its pid, as the lease's; one without the mark is not the lease's to kill
        assert_eq!(x.handle("lease-kill", &format!("S1 {jit}")), vec![format!("{{\"killed\":{jit}}}")], "a marked process outside the run's group");
        assert!(x.handle("lease-kill", &format!("S1 {stays}"))[0].contains("is no process of lease S1"), "an unmarked one");
        let t = Instant::now();
        while os::with_env(&mark).contains(&jit) {
            assert!(t.elapsed() < Duration::from_secs(10), "the killed JIT {jit} lives on");
            std::thread::sleep(Duration::from_millis(20));
        }
        let e = x.handle("lease-end", "S1 released");
        assert!(e[0].contains(&gone.to_string()) && !e[0].contains(&stays.to_string()), "gone {gone} stays {stays}: {e:?}");
        assert_eq!(os::with_env(&format!("{LEASE_ENV}={}", lease_mark(&base.join("leases"), "S1"))), Vec::<u32>::new());
        assert!(os::pid_alive(stays), "one that removed the mark outlives the lease");
        os::signal_one(stays, true);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// What [`a_released_session_ends_the_groups_its_runs_moved_into`] starts as a run's stand-in:
    /// a process with two strays, one in a group of its own and one in a session of its own (the
    /// way a check server outlives what started it), their pids in the file it is given. It ends
    /// when its input closes.
    #[cfg(unix)]
    #[test]
    fn lease_end_role_detacher() {
        use std::os::unix::process::CommandExt;
        let Some(file) = std::env::var_os("DEVHUB_TEST_DETACHER") else { return };
        let stray = |session: bool| {
            let mut c = crate::testenv::role("lease::exec::tests::lease_end_role_stray", Path::new(&file).parent().unwrap());
            c.env("DEVHUB_TEST_STRAY", "1").env_remove("DEVHUB_TEST_DETACHER");
            c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            if session {
                extern "C" {
                    fn setsid() -> i32;
                }
                unsafe { c.pre_exec(|| if setsid() < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }) };
            } else {
                c.process_group(0);
            }
            c.spawn().unwrap().id()
        };
        let (group, session) = (stray(false), stray(true));
        std::fs::write(&file, format!("{group} {session}")).unwrap();
        let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut Vec::new());
    }

    /// A released session ends what its runs started in groups of their own and stripped of the
    /// lease's mark: two spinning children of a hung live test outlived it for hours (W11754,
    /// 11-17). One whose parent still runs is found at the end; one its parent left, which no walk
    /// reaches, by the beat before, which a hub restart keeps. The end names them. A process in a
    /// session of its own left on purpose (relax's check server serves the leases after it): no
    /// group of the run's.
    #[cfg(unix)]
    #[test]
    fn a_released_session_ends_the_groups_its_runs_moved_into() {
        use std::os::unix::process::CommandExt;
        let (x, base, events) = test_exec("beside", Opts { grace: Duration::from_secs(2), scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"B1","lane":"beside","kind":"test","rev":null}"#);
        let tmp = x.leases.lock().unwrap()["B1"].tmp.clone();
        let read_pids = |file: &Path, what: &str| {
            let t = Instant::now();
            loop {
                let pids: Vec<u32> = std::fs::read_to_string(file).unwrap_or_default().split_whitespace().filter_map(|p| p.parse().ok()).collect();
                if !pids.is_empty() { break pids; }
                assert!(t.elapsed() < Duration::from_secs(30), "{what} named no pid");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        // each run's child forks a spinner into a job group of its own, without the lease's mark
        let spin = |name: &str, then: &str| {
            let file = tmp.join(format!("{name}.pid"));
            let script = format!("set -m; env -u {LEASE_ENV} -u {LEASE_OUTER_ENV} sh -c 'while :; do :; done' & echo $! > '{}'; {then}", file.display());
            let r = x.handle("lease-run", &format!("B1 . {}", b64(&["sh", "-c", &script])));
            let n = field(&r[0], "n").and_then(|v| super::super::num(&v)).expect(&r[0]) as u32;
            let pid = read_pids(&file, name)[0];
            (n, pid, os::proc_start(pid).unwrap())
        };
        let go = tmp.join("go");
        let (_, held, held_start) = spin("held", "wait");
        let (left_n, left, left_start) = spin("left", &format!("while [ ! -e '{}' ]; do sleep 0.05; done", go.display()));
        let mark = format!("{LEASE_ENV}={}", lease_mark(&base.join("leases"), "B1"));
        let t = Instant::now();
        while os::with_env(&mark).iter().any(|p| *p == held || *p == left) {
            assert!(t.elapsed() < Duration::from_secs(10), "a spinner kept the mark");
            std::thread::sleep(Duration::from_millis(10));
        }
        x.handle("lease-beat", "B1");
        let groups = x.leases.lock().unwrap()["B1"].groups.clone();
        assert!([held, left].iter().all(|s| groups.iter().any(|(g, start)| g == s && *start > 0)), "each spinner leads its group: {groups:?}");
        let saved = std::fs::read_to_string(base.join("leases/B1.json")).unwrap();
        assert_eq!(Lease::from_ledger(&saved).map(|l| l.groups), Some(groups.clone()), "saved");
        std::fs::write(&go, "").unwrap();
        assert_eq!(log_until_exit(&x, "B1", left_n).1, 0);
        assert!(os::same(left, left_start), "its parent gone, the spinner runs on");
        // a run's stand-in that leads its group: its stray in a group of its own is the run's, one
        // in a session of its own is not
        let file = base.join("detacher.pids");
        let mut c = crate::testenv::role("lease::exec::tests::lease_end_role_detacher", &base);
        c.env("DEVHUB_TEST_DETACHER", &file).process_group(0);
        c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        let mut stand_in = c.spawn().unwrap();
        let pids = read_pids(&file, "the stand-in");
        let (group, session) = (pids[0], pids[1]);
        let found = os::groups_beside(&[(vec![(stand_in.id(), os::proc_start(stand_in.id()).unwrap())], Vec::new())]).pop().unwrap();
        for p in [group, session] {
            os::signal_one(p, true);
        }
        drop(stand_in.stdin.take());
        let _ = stand_in.wait();
        assert!(found.iter().any(|(g, _)| *g == group) && !found.iter().any(|(g, _)| *g == session), "group {group} session {session}: {found:?}");
        let e = x.handle("lease-end", "B1 released");
        assert!(e[0].contains(&held.to_string()) && e[0].contains(&left.to_string()), "held {held} left {left}: {e:?}");
        let t = Instant::now();
        while os::same(held, held_start) || os::same(left, left_start) {
            assert!(t.elapsed() < Duration::from_secs(10), "a spinner outlived its lease");
            std::thread::sleep(Duration::from_millis(20));
        }
        let said = events.lock().unwrap().iter().find(|e| e.contains("had left its runs' groups and its mark")).cloned().unwrap_or_default();
        assert!(said.contains(&format!("{held} ")) && said.contains(&format!("{left} ")), "named: {said}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An app a lease's run started in a session of its own is still that lease's, by its mark
    /// ([`app_lease`]). One with the mark of a lease no longer here is a leftover; one with no mark is
    /// a person's. A WM restart took such apps for a person's and never drained its box (11-07).
    #[cfg(target_os = "linux")]
    #[test]
    fn an_app_that_left_its_runs_group_is_its_leases_by_its_mark() {
        let (x, base, _) = test_exec("marks", Opts { grace: Duration::from_secs(2), scopes: Some(false), ..Opts::default() });
        x.handle("lease-start", r#"{"id":"S1","lane":"marks","kind":"test","rev":null}"#);
        let spawn = |mark: Option<&str>| {
            use std::os::unix::process::CommandExt;
            let mut c = crate::testenv::role("lease::exec::tests::lease_end_role_stray", &base);
            c.env("DEVHUB_TEST_STRAY", "1").process_group(0);
            c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            match mark {
                Some(id) => c.env(LEASE_ENV, lease_mark(&base.join("leases"), id)),
                None => c.env_remove(LEASE_ENV),
            };
            c.spawn().unwrap()
        };
        let mut children = [spawn(Some("S1")), spawn(Some("S0")), spawn(None)];
        let [its, left, nobody] = [children[0].id(), children[1].id(), children[2].id()];
        // (its environment is its own once it has started)
        let t = Instant::now();
        while x.app_lease(its) != (AppLease::Live { on_wm: false }) || x.app_lease(left) != AppLease::Left {
            assert!(t.elapsed() < Duration::from_secs(30), "{:?} {:?}", x.app_lease(its), x.app_lease(left));
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(x.app_lease(nobody), AppLease::Nobody);
        x.handle("lease-end", "S1 released");
        // each gone before the hub's drop looks for what its leases left: a kill lands when its
        // process next runs, and one not yet dead on a busy box read as left behind (11-07)
        for child in &mut children {
            os::signal_one(child.id(), true);
            let _ = child.wait();
        }
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
        let (exit7, job_cmd, gate_cmd): (&[&str], &[&str], &[&str]) = (&["powershell", "-NoProfile", "-Command", "exit 7"], &["powershell", "-NoProfile", "-Command", "ping -n 2 127.0.0.1 | Out-Null; Write-Output done; exit 7"], &["powershell", "-NoProfile", "-Command", "ping -n 2 127.0.0.1 | Out-Null; Write-Output gate; exit 0"]);
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
            x.handle("lease-run", &format!("K4 . {}", b64(&["powershell", "-NoProfile", "-Command", "Start-Process ping -ArgumentList '-n','300','127.0.0.1' -NoNewWindow; ping -n 300 127.0.0.1 | Out-Null"])));
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

    /// A launch whose wrapper cannot start, or ends before it reports the app's pid, answers an
    /// error at once. The hub finds its wrappers on its own PATH ([`Opts::path`], here a private
    /// one), never on the command's: the test set the command's PATH, the JIT's std searched the
    /// hub's, found the box's `sh`, and the launch started (11-05). The command's own PATH is
    /// its own lookup's.
    #[cfg(unix)]
    #[test]
    fn a_launch_that_cannot_start_returns_an_error() {
        let script = |path: &Path, body: &str| crate::testenv::exec_script(path, &format!("#!/bin/sh\n{body}\n"));
        let launch = |x: &Arc<Exec>, cmd: std::process::Command, log: PathBuf| {
            let (tx, rx) = std::sync::mpsc::channel();
            let (ended_tx, ended_rx) = std::sync::mpsc::channel();
            let y = Arc::clone(x);
            let worker = std::thread::spawn(move || {
                let _ = tx.send(y.launch("F1", &cmd, &log, Box::new(move |app, code| { let _ = ended_tx.send((app, code)); })));
            });
            let result = rx.recv_timeout(Duration::from_secs(5)).expect("a launch answers without waiting on its app");
            worker.join().unwrap();
            (result, ended_rx)
        };
        let start = r#"{"id":"F1","lane":"b","kind":"timed","rev":null}"#;
        let bin = std::env::temp_dir().join(format!("devhub-exec-launch-error-{}", std::process::id())).join("bin");
        let (x, base, _) = test_exec("launch-error", Opts { scopes: Some(false), path: Some(bin.clone().into_os_string()), ..Opts::default() });
        std::fs::create_dir_all(&bin).unwrap();
        assert!(!x.handle("lease-start", start)[0].starts_with("{\"err\""));
        // the command's PATH holds a wrapper that would start: the hub's has none
        let decoy = base.join("decoy");
        std::fs::create_dir_all(&decoy).unwrap();
        script(&decoy.join("sh"), "exec /bin/sh \"$@\"");
        let mut cmd = std::process::Command::new("true");
        cmd.env("PATH", &decoy).current_dir(&base);
        let (result, _) = launch(&*x, cmd, base.join("launch.log"));
        let error = result.unwrap_err();
        assert!(error.starts_with("launch under F1: sh: not on the hub's PATH"), "{error}");
        assert!(x.leases.lock().unwrap()["F1"].runs.is_empty(), "a wrapper that never started records no run");
        // a wrapper that ends before it reports the app's pid
        script(&bin.join("sh"), "exit 7");
        let mut cmd = std::process::Command::new("true");
        cmd.current_dir(&base);
        let (result, ended) = launch(&*x, cmd, base.join("launch.log"));
        let error = result.unwrap_err();
        assert!(error.starts_with("launch under F1:") && error.contains("exit 7"), "the wrapper's exit is said: {error}");
        assert_eq!(ended.recv_timeout(Duration::from_secs(5)).unwrap(), (0, 7));
        drop(x);
        let _ = std::fs::remove_dir_all(&base);

        // on the hub's PATH the wrappers start; the command finds what it runs on its own PATH
        let (x, base, _) = test_exec("launch-path", Opts { scopes: Some(false), ..Opts::default() });
        std::fs::create_dir_all(&base).unwrap();
        assert!(!x.handle("lease-start", start)[0].starts_with("{\"err\""));
        let own = base.join("own");
        std::fs::create_dir_all(&own).unwrap();
        script(&own.join("devhub-test-app"), &format!("echo found > {}", base.join("found").display()));
        let mut cmd = std::process::Command::new("devhub-test-app");
        cmd.env("PATH", &own).current_dir(&base);
        let (result, ended) = launch(&*x, cmd, base.join("launch.log"));
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(ended.recv_timeout(Duration::from_secs(10)).map(|(_, code)| code), Ok(0), "{:?}", std::fs::read_to_string(base.join("launch.log")));
        assert_eq!(std::fs::read_to_string(base.join("found")).unwrap(), "found\n");
        drop(x);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A launch is a run of its lease, under its wrapper: its pid is the app's, its environment the
    /// launch's, and its end is its lease's to tell. One that quit by itself ends its lease with no
    /// abnormal line (a timed lease's launch that quit ended as "its exit is unknown (its hub
    /// restarted while it ran)", no hub having restarted, and its lane ran its whole timed series
    /// again, 11-05); a hub restart under one loses nothing (the new hub reads its exit and log);
    /// one stopped on purpose ended as asked.
    #[cfg(unix)]
    #[test]
    fn a_launch_is_a_run_its_hub_restart_keeps() {
        let (x, base, _) = test_exec("launch", Opts { scopes: Some(false), ..Opts::default() });
        std::fs::create_dir_all(&base).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let launch = |x: &Exec, id: &str, script: &str| {
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", script]).env("LAUNCHED", "yes").current_dir(&base);
            let tx = tx.clone();
            x.handle("lease-start", &format!(r#"{{"id":"{id}","lane":"b","kind":"{}","rev":null}}"#, if id == "T2" { "test" } else { "timed" }));
            x.launch(id, &cmd, &base.join(format!("{id}.log")), Box::new(move |app, code| { let _ = tx.send((app, code)); })).unwrap()
        };
        let quit = launch(&x, "T1", "echo $$; echo $LAUNCHED; exit 0");
        assert_eq!(rx.recv().unwrap(), (quit.app, 0), "its end is said with the app's pid");
        let (out, code) = log_until_exit(&x, "T1", quit.n);
        assert_eq!((out, code), (format!("{}\nyes\n", quit.app), 0), "the app's own pid and the launch's environment");
        let e = x.handle("lease-end", "T1 released");
        assert!(e[0].contains("\"abnormal\":null"), "a launch that quit by itself ended normally: {e:?}");

        let runs_on = launch(&x, "T2", "sleep 1; echo up; exit 3");
        assert_ne!(runs_on.app, runs_on.leader, "the wrapper leads its group");
        assert!(x.restart_blocker(&x.leases.lock().unwrap()).is_none(), "a restart waits for no launch");
        let y = x.restart(Opts { scopes: Some(false), ..Opts::default() });
        assert_eq!(log_until_exit(&y, "T2", runs_on.n), ("up\n".to_string(), 3), "the new hub reads its log and exit");
        let e = y.handle("lease-end", "T2 released");
        assert!(e[0].contains("T2 run 1 ended without its summary: exit 3"), "its lease's end says how it ended: {e:?}");

        let stopped = launch(&y, "T3", "exec sleep 300");
        y.launch_stopped(stopped.leader);
        assert!(makepad_remote::group::signal(stopped.leader, false));
        assert_eq!(log_until_exit(&y, "T3", stopped.n).1, 143);
        let e = y.handle("lease-end", "T3 released");
        assert!(e[0].contains("\"abnormal\":null"), "a launch stopped on purpose ended as asked: {e:?}");
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
        let run = Arc::new(Run { request: String::new(), started_ms: 0, spawned_ms: 0, ended_ms: AtomicU64::new(0), n: 1, pid: 0, start: 0, unit: None, log: base.join("run-1.log"), exit: Mutex::new(None), ours: true, detachable: false, hold: os::Hold::none(), exit_file: exit_file(&base, "L1", 1), cache: Mutex::new(None), usage: Mutex::new(Usage::default()), stopped: AtomicBool::new(false), native: None, build: OnceLock::new() });
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

    /// A timed start waits while its box's deployment settles from a move, while a compiler of no
    /// lease's runs there and while what runs outside every lease takes half a core or more (a desktop left running), and says what it waits on once,
    /// not at each of the master's retries; a lease's own run holds nothing. The box is the test's
    /// own [`BoxLook`] (pids past any real one): a test hub sees no other work on its box.
    #[cfg(unix)]
    #[test]
    fn a_timed_start_waits_for_its_box_to_be_quiet() {
        const HUB: u32 = 5_000_000;
        let rows = Arc::new(Mutex::new(Vec::<crate::top::ProcRow>::new()));
        let moved = Arc::new(Mutex::new(Some(Duration::from_secs(2))));
        let gpu = Arc::new(Mutex::new(None::<Vec<(u32, f64)>>));
        let (r2, m2, g2) = (rows.clone(), moved.clone(), gpu.clone());
        let look = BoxLook { procs: Arc::new(move || (r2.lock().unwrap().clone(), HUB)), moved_ago: Arc::new(move || *m2.lock().unwrap()), gpu: Arc::new(move || g2.lock().unwrap().clone()), cpu: Arc::new(|| None), ..BoxLook::quiet() };
        let (x, base, events) = test_exec("timed-look", Opts { scopes: Some(false), box_look: Some(look), ..Opts::default() });
        let start = r#"{"id":"T1","lane":"timedl","kind":"timed","rev":null}"#;
        let said = || events.lock().unwrap().iter().filter(|l| l.contains("timed lease T1 of timedl waits on t:")).count();
        let answer = x.handle("lease-start", start);
        assert!(answer[0].contains("its deployment moved 2.0 s ago") && answer[0].contains(super::super::TIMED_AFTER), "a move a moment ago: {answer:?}");
        assert!(!x.leases.lock().unwrap().contains_key("T1"));
        *moved.lock().unwrap() = Some(MOVE_SETTLE);
        // a build lease's compiler, in its run's group, and one under a cargo no lease started
        x.handle("lease-start", r#"{"id":"B1","lane":"timedl","kind":"test","rev":null}"#);
        x.handle("lease-run", &format!("B1 . {}", b64(&["sh", "-c", "sleep 30"])));
        let group = x.leases.lock().unwrap()["B1"].runs[0].pid;
        let row = |pid: u32, ppid: u32, pgid: u32, name: &str, cpu_pct: f64| crate::top::ProcRow { pid, ppid, pgid, name: name.into(), cpu_pct, ..Default::default() };
        *rows.lock().unwrap() = vec![
            row(5_000_010, 1, 5_000_010, "session", 0.0),
            row(5_000_011, 5_000_010, 5_000_011, "cargo", 2.0),
            row(5_000_012, 5_000_011, 5_000_011, "rustc", 300.0),
            row(5_000_020, group, group, "rustc", 700.0),
        ];
        for retry in 0..2 {
            let answer = x.handle("lease-start", start);
            assert!(answer[0].contains("rustc pid 5000012 (under cargo < session) at 300% of a core, cargo pid 5000011 (under session) at 2% of a core outside every lease: "), "retry {retry}: {answer:?}");
        }
        assert_eq!(said(), 2, "the move and the compilers, each said once");
        rows.lock().unwrap().retain(|r| r.pgid == group);
        rows.lock().unwrap().extend([row(5_000_030, 1, 5_000_030, "WindowServer", 34.0), row(5_000_031, 1, 5_000_031, "coreaudiod", 18.0), row(5_000_032, 1, 5_000_032, "Musicplayer", 13.0), row(5_000_033, 1, 5_000_033, "Finder", 2.0)]);
        let answer = x.handle("lease-start", start);
        assert!(answer[0].contains("67% of a core runs outside every lease (WindowServer 34%, coreaudiod 18%, Musicplayer 13%)"), "a desktop left running: {answer:?}");
        assert_eq!(said(), 3, "the desktop, said once");
        // past its wait for a quiet box its master starts it beside that work, said as a warning
        let anyway = r#"{"id":"T2","lane":"timedl","kind":"timed","unquiet":"t: 67% of a core runs outside its leases","rev":null}"#;
        let answer = x.handle("lease-start", anyway);
        assert!(!answer[0].contains("\"err\""), "its master's bound starts it beside the desktop: {answer:?}");
        let beside = "timed lease T2 of timedl starts on t beside work outside every lease, past its wait for a quiet box (t: 67% of a core runs outside its leases)";
        assert!(events.lock().unwrap().iter().any(|l| l.contains(beside)), "{:?}", events.lock().unwrap());
        x.handle("lease-end", "T2 released");
        // a GPU timing waits for per-process GPU use outside every lease, not for their CPU
        let gpu_start = r#"{"id":"G1","lane":"gpul","kind":"timed","gpu":true,"rev":null}"#;
        *gpu.lock().unwrap() = Some(vec![(5_000_030, 4.0), (5_000_032, 30.0), (5_000_020, 80.0)]);
        let answer = x.handle("lease-start", gpu_start);
        assert!(answer[0].contains("its GPU is 34% busy outside every lease: what uses it, Musicplayer pid 5000032 at 30% of the GPU, WindowServer pid 5000030 at 4% of the GPU: "), "{answer:?}");
        *gpu.lock().unwrap() = Some(vec![(5_000_030, 10.0), (5_000_020, 80.0)]);
        let answer = x.handle("lease-start", gpu_start);
        assert!(answer[0].contains("its GPU is 10% busy outside every lease: what uses it, WindowServer pid 5000030 at 10% of the GPU: "), "the threshold holds the start: {answer:?}");
        *gpu.lock().unwrap() = Some(vec![(5_000_030, 9.0), (5_000_020, 80.0)]);
        let answer = x.handle("lease-start", gpu_start);
        assert!(!answer[0].contains("\"err\""), "under 10% of the GPU outside leases starts, despite a lease's own 80% and 67% of a core outside: {answer:?}");
        x.handle("lease-end", "G1 released");
        rows.lock().unwrap().retain(|r| r.pgid == group || r.name == "Finder");
        let answer = x.handle("lease-start", start);
        assert!(!answer[0].contains("\"err\""), "the build lease's own compiler and an app's 2% hold nothing: {answer:?}");
        x.handle("lease-end", "T1 released");
        x.handle("lease-end", "B1 released");
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
