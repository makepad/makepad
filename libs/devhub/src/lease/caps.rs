//! This machine's capabilities and health: hardware from the OS (Linux `/proc` + `/sys` +
//! `nvidia-smi`; macOS `sysctl` + `statfs`), policy from `~/.makepad/devhub/lease.conf`.
//!
//! `lease.conf` is `key = value` lines: `role = pool|hot|suite|bench|user-wm|device` (`bench`: the
//! boxes timed leases go to), `slots.<kind> = 0|1` (one write lease per kind; `slots.ci = 0` keeps CI off a box),
//! `disk_floor_gb` (its warning follows the JIT's floor: [`super::disk::warn_gb`]), `timed = on|off|spare` (`spare`: a pool box that takes a timed lease
//! while the bench boxes that meet it are taken by timed work), `spec = <text>`, `work = <dir>` (default
//! [`WORK_DIR`] in home) and CPU pinning for leases (`cpus.shared = 8-15,24-31`, `cpus.timed = 0-7,16-23`).
//! `native_trees` (default 3) bounds per-lane native trees and their targets. A box leaves the
//! dev pool only by its dev switch: no key here makes a person's input take it out.

use super::{Caps, HotTemp, Kind, Role, Temp};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The node's lease policy (`lease.conf`).
#[derive(Clone, Debug, PartialEq)]
pub struct LeaseConf {
    pub role: Role,
    /// explicit `slots.<kind>` lines; the rest come from [`LeaseConf::slots`]'s defaults
    pub slots: Vec<(Kind, u32)>,
    pub disk_floor_gb: f64,
    pub timed: bool,
    /// `timed = spare`: a pool box that takes timed work while the bench boxes are taken by it
    pub timed_spare: bool,
    pub spec: String,
    /// where leases work: `dev/` for overlays; `native/`, `native-target/`, `ci-tree/`,
    /// `ci-target/`, `lane/`, `target/`, `tmp/`, `recovered/` for tree leases
    pub work: PathBuf,
    pub native_trees: u32,
    /// taskset CPU lists for shared kinds and for `timed` (empty: no pinning)
    pub cpus_shared: String,
    pub cpus_timed: String,
}

pub const NATIVE_TREES: u32 = 3;

pub fn home_dir() -> PathBuf {
    crate::os::home_dir()
}

/// The default work dir under home. Spotlight skips a `.noindex` dir on macOS: it indexed build
/// output beside timed leases at over a core and a half (11-03).
pub const WORK_DIR: &str = if cfg!(target_os = "macos") { "devhub.noindex" } else { "devhub" };

pub fn conf_path() -> PathBuf {
    crate::proto::home().join("lease.conf")
}

impl LeaseConf {
    /// `~/.makepad/devhub/lease.conf`, defaults where it is missing.
    pub fn load() -> LeaseConf {
        LeaseConf::parse(&std::fs::read_to_string(conf_path()).unwrap_or_default(), &home_dir())
    }

    pub fn parse(text: &str, home: &Path) -> LeaseConf {
        let mut c = LeaseConf {
            role: Role::Pool,
            slots: Vec::new(),
            disk_floor_gb: 25.0,
            timed: true,
            timed_spare: false,
            spec: String::new(),
            work: home.join(WORK_DIR),
            native_trees: NATIVE_TREES,
            cpus_shared: String::new(),
            cpus_timed: String::new(),
        };
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "role" => c.role = Role::parse(v).unwrap_or(c.role),
                "disk_floor_gb" => c.disk_floor_gb = v.parse().unwrap_or(c.disk_floor_gb),
                "timed" => (c.timed, c.timed_spare) = (v != "off", v == "spare"),
                "spec" => c.spec = v.to_string(),
                "work" => c.work = v.strip_prefix("~/").map_or_else(|| PathBuf::from(v), |r| home.join(r)),
                "native_trees" => c.native_trees = v.parse().unwrap_or(c.native_trees),
                "cpus.shared" if valid_cpus(v) => c.cpus_shared = v.to_string(),
                "cpus.timed" if valid_cpus(v) => c.cpus_timed = v.to_string(),
                _ => {
                    if let (Some(kind), Ok(n)) = (k.strip_prefix("slots.").and_then(Kind::parse), v.parse::<u32>()) {
                        c.slots.retain(|(x, _)| *x != kind);
                        c.slots.push((kind, n));
                    }
                }
            }
        }
        c
    }

    /// One slot per kind by default, CI among them: every box takes any work unless its conf
    /// says `slots.<kind> = 0` (an opt-in left CI on one box per OS, its runs crawling there while
    /// the others idled, 11-22). Never CI on a user's box.
    /// `inspect` is not counted.
    pub fn slots(&self, gpus: u32) -> Vec<(Kind, u32)> {
        let defaults = [(Kind::Build, 1), (Kind::Test, 1), (Kind::Screen, 1), (Kind::Long, 1), (Kind::Gate, 1), (Kind::Gpu, gpus.min(1)), (Kind::Timed, self.timed as u32), (Kind::Ci, 1)];
        defaults.iter().map(|(k, d)| (*k, if *k == Kind::Ci && self.role.users() { 0 } else { self.slots.iter().find(|(x, _)| x == k).map_or(*d, |(_, n)| *n) })).collect()
    }
}

/// Whether the master may set `key = value` in a node's lease.conf (`work` stays the node's own:
/// moving the work dir under running leases is not a remote switch).
/// An empty value unsets the key ([`conf_with`]): the box's default applies again.
pub fn settable(key: &str, value: &str) -> Result<(), String> {
    let known = matches!(key, "role" | "disk_floor_gb" | "native_trees" | "timed" | "spec" | "cpus.shared" | "cpus.timed") || key.strip_prefix("slots.").and_then(Kind::parse).is_some();
    if known && value.is_empty() {
        return Ok(());
    }
    let num = || value.parse::<f64>().map(|_| ()).map_err(|_| format!("{key} takes a number"));
    match key {
        "role" => Role::parse(value).map(|_| ()).ok_or_else(|| "role = pool|hot|suite|bench|user-wm|device".to_string()),
        "disk_floor_gb" => num(),
        "native_trees" => value.parse::<u32>().map(|_| ()).map_err(|_| "native_trees takes a count".to_string()),
        "timed" => matches!(value, "on" | "off" | "spare").then_some(()).ok_or_else(|| "timed = on|off|spare".to_string()),
        "spec" => (!value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_graphic())).then_some(()).ok_or_else(|| "spec: one word".to_string()),
        "cpus.shared" | "cpus.timed" => (value.is_empty() || valid_cpus(value)).then_some(()).ok_or_else(|| format!("{key}: a CPU list like 8-15,24-31")),
        k => match k.strip_prefix("slots.").and_then(Kind::parse) {
            Some(_) => value.parse::<u32>().map(|_| ()).map_err(|_| format!("{key} takes a count")),
            None => Err(format!("`{key}` is not a lease.conf key the master sets (role, slots.<kind>, native_trees, disk_floor_gb, timed, spec, cpus.shared, cpus.timed)")),
        },
    }
}

/// `text` with each `key = value` of `pairs` set: the key's line replaced in place (comments
/// kept), or appended; an empty value removes the key's line, so the default applies (a setting
/// the default came to match stays behind as the box's own otherwise).
pub fn conf_with(text: &str, pairs: &[(String, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (k, v) in pairs {
        let at = lines.iter().position(|l| l.split('#').next().and_then(|b| b.split_once('=')).is_some_and(|(lk, _)| lk.trim() == k));
        let line = format!("{k} = {v}");
        match at {
            Some(i) if v.is_empty() => {
                lines.remove(i);
            }
            None if v.is_empty() => {}
            Some(i) => {
                let comment = lines[i].find('#').map(|c| format!("  {}", &lines[i][c..])).unwrap_or_default();
                lines[i] = format!("{line}{comment}");
            }
            None => lines.push(line),
        }
    }
    format!("{}\n", lines.join("\n"))
}

fn valid_cpus(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b',' || b == b'-')
}

/// The CPUs a taskset list names (`8-15,24-31`: 16); 0 for none or one taskset would refuse.
pub fn cpu_count(list: &str) -> u32 {
    let mut n = 0;
    for part in list.split(',') {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        match (a.parse::<u32>(), b.parse::<u32>()) {
            (Ok(a), Ok(b)) if a <= b => n += b - a + 1,
            _ => return 0,
        }
    }
    n
}

/// Measures this machine now (`node`: its devhub name; `apply`: DEVHUB2's applier kind).
/// PATH with the user's rustup toolchain first (`$CARGO_HOME/bin`, else `~/.cargo/bin`) when it has
/// one: a box whose system Rust sits in /usr/bin builds with the fleet's rustup stable. None: as is.
pub fn rust_path() -> Option<String> {
    let bin = std::env::var_os("CARGO_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?.join("bin");
    if !bin.join(if cfg!(windows) { "rustc.exe" } else { "rustc" }).is_file() {
        return None;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let rest: Vec<PathBuf> = std::env::split_paths(&path).filter(|p| *p != bin).collect();
    std::env::join_paths(std::iter::once(bin).chain(rest)).ok().map(|p| p.to_string_lossy().into_owned())
}

/// `prog` (cargo, rustc) as a lease's run finds it ([`rust_path`]).
pub fn rust_command(prog: &str) -> std::process::Command {
    let mut c = std::process::Command::new(prog);
    if let Some(p) = rust_path() {
        c.env("PATH", p);
    }
    c
}

/// The toolchain a lease's run finds ([`rust_command`]): `rustc -vV`'s release (`1.100.0`) and
/// commit hash. Each read ([`read_rustc`]) keys the JIT store too (loaders::compiler), so a rustup
/// update reaches the box's report and its keys without a hub restart.
#[derive(Clone, Default)]
pub struct Rustc {
    pub release: String,
    pub commit: String,
}

static RUSTC: std::sync::Mutex<Option<(std::time::Instant, Rustc)>> = std::sync::Mutex::new(None);

/// `rustc -vV`'s `release:` and `commit-hash:` lines (empty when it does not say).
fn parse_rustc(vv: &str) -> Rustc {
    let line = |k: &str| vv.lines().find_map(|l| l.strip_prefix(k)).unwrap_or("").trim().to_string();
    Rustc { release: line("release: "), commit: line("commit-hash: ") }
}

/// Reads the toolchain again now, and keys the box's JIT binaries by what it read
/// ([`super::loaders::rekey`]): a lease's start and end, the box's report when 5 min old, and the
/// toolchain probe each read it. A key changed only at lease starts left a rustup update between
/// two leases unseen by the hub's report and its prebuilds (11-13).
pub fn read_rustc() -> Rustc {
    let r = rust_command("rustc").arg("-vV").output().ok().map(|o| parse_rustc(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default();
    *RUSTC.lock().unwrap() = Some((std::time::Instant::now(), r.clone()));
    super::loaders::rekey(&r.commit);
    r
}

/// This box's rustc release (`1.97.1`): its report, and on the master the toolchain gate runs and
/// their proofs name. Read again when 5 min old, and at each lease's start.
pub fn rustc_version() -> String {
    let fresh = RUSTC.lock().unwrap().as_ref().filter(|(t, _)| t.elapsed() < std::time::Duration::from_secs(300)).map(|(_, r)| r.release.clone());
    fresh.unwrap_or_else(|| read_rustc().release)
}

static TARGETS: std::sync::Mutex<Option<(std::time::Instant, Vec<String>)>> = std::sync::Mutex::new(None);

/// The targets whose std the toolchain a lease's run finds has ([`rust_command`]): the host's and
/// what `rustup target add` installed. A probe only: installing one is the user's call. Read
/// again when 5 min old, so an added target reaches the box's report without a hub restart.
pub fn rust_targets() -> Vec<String> {
    let mut t = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, v)) = t.as_ref().filter(|(at, _)| at.elapsed() < std::time::Duration::from_secs(300)) {
        return v.clone();
    }
    let sysroot = rust_command("rustc").args(["--print", "sysroot"]).stdin(std::process::Stdio::null()).output().ok()
        .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty());
    let v = sysroot.map(|s| installed_targets(Path::new(&s))).unwrap_or_default();
    *t = Some((std::time::Instant::now(), v.clone()));
    v
}

/// The targets of a sysroot that hold a std: `lib/rustlib/<target>/lib/libcore-*.rlib` (its other
/// entries, `src`, `etc` and the component lists, hold none).
fn installed_targets(sysroot: &Path) -> Vec<String> {
    let has_core = |dir: &Path| std::fs::read_dir(dir.join("lib")).into_iter().flatten().flatten()
        .any(|f| f.file_name().to_str().is_some_and(|n| n.starts_with("libcore-") && n.ends_with(".rlib")));
    let mut v: Vec<String> = std::fs::read_dir(sysroot.join("lib").join("rustlib")).into_iter().flatten().flatten()
        .filter(|e| has_core(&e.path())).filter_map(|e| e.file_name().to_str().map(str::to_string)).collect();
    v.sort();
    v
}

/// The same live WM display reported in caps, without reprobing the hardware/toolchain.
#[cfg(target_os = "linux")]
pub(crate) fn display() -> Option<String> {
    os::display()
}

/// Why a WM's `status` answer (its windows) shows nothing: its one window is 0x0, which it is
/// when no output is attached or live, and every window opened under it then times out on its
/// first frame (fourteen gui and screen leases on one node in 3 h, 11-07). Empty when a window
/// has a size, or the answer says nothing of them. `connectors` are the box's DRM connectors and
/// their states (`/sys/class/drm/<name>/status`): when each that could show a screen reads
/// `disconnected`, no output is plugged in or the display is off, a cable or a screen to see to,
/// not the WM (a node's line said only "window is 0x0", 11-13). A writeback connector shows none
/// (the kernel's capture sink, `connected` whatever is plugged in): counted as a connected
/// output, it laid a box with no screen at its WM's door (11-25). The WM's renderer lights an
/// output as it appears (a rescan a second and at each hotplug): nothing to restart.
pub fn dark_of(status: &str, connectors: &[(String, String)]) -> String {
    let Ok(v) = makepad_strict_json::parse(status.as_bytes()) else { return String::new() };
    let Some(windows) = v.get("w").and_then(|w| w.as_arr()).filter(|w| !w.is_empty()) else { return String::new() };
    let empty = |w: &makepad_strict_json::Value| w.get("sz").and_then(|s| s.as_arr()).is_some_and(|s| s.iter().all(|n| super::num(n).is_some_and(|n| n <= 0.0)));
    if !windows.iter().all(empty) { return String::new(); }
    let screens: Vec<&str> = connectors.iter().filter(|(name, _)| !name.contains("-Writeback-")).map(|(_, state)| state.trim()).collect();
    if !screens.is_empty() && screens.iter().all(|c| *c == "disconnected") {
        return format!("no output connected (its {} DRM connectors read disconnected; its WM's window is 0x0, and lights one as it is plugged in or switched on)", screens.len());
    }
    "its WM's window is 0x0: no output shows it".into()
}

/// The node's own WM among this box's endpoints, the newest when a restart left the old one's file.
/// A lease's WM is a test desktop beside it, and says nothing of what the box shows.
pub fn node_wm(apps: &[makepad_remote::discovery::App]) -> Option<&makepad_remote::discovery::App> {
    apps.iter().filter(|a| a.app == "makepad-app-wm" && a.text("lease").is_none()).max_by_key(|a| a.started_ms)
}

/// The box's WM answered on another hosted wire at the last caps measure (its socket and
/// answer): no display for gui work until it restarts (`sync::serve` rolls that restart).
#[cfg(target_os = "linux")]
pub fn other_wire() -> Option<String> {
    os::OTHER_WIRE.lock().unwrap().clone()
}

pub fn measure(node: &str, conf: &LeaseConf, apply: &str) -> Caps {
    let mut c = os::hardware();
    c.exec_limits = crate::os::exec_limits().ok().flatten();
    c.node = node.to_string();
    let (free, total) = disk_space(&conf.work).unwrap_or((0, 0));
    c.disk_free_gb = free as f64 / GB;
    c.disk_total_gb = total as f64 / GB;
    c.role = Some(conf.role);
    c.spec = conf.spec.clone();
    c.slots = conf.slots(c.gpus.len() as u32);
    c.disk_floor_gb = conf.disk_floor_gb;
    c.disk_warn_gb = super::disk::warn_gb(c.disk_total_gb, conf.disk_floor_gb);
    c.timed = conf.timed;
    c.timed_spare = conf.timed_spare;
    c.apply = apply.to_string();
    c.hub_rev = hub_rev().to_string();
    c.hub_rss_mb = crate::procinfo::proc(std::process::id()).map_or(0, |p| p.rss_bytes >> 20);
    c.hub_heap_mb = (crate::heap::live() >> 20) as u64;
    c.sick = sick();
    c.clocksource = clocksource();
    c.rustc = rustc_version();
    c.rust_targets = rust_targets();
    c.features = super::FEATURES.iter().map(|f| f.to_string()).collect();
    c.services = services();
    c.hub_building = super::exec::hub_building();
    #[cfg(feature = "checkout")]
    { c.wm_drain = crate::sync::serve::wm_drain(); }
    #[cfg(feature = "checkout")]
    {
        let pids;
        let sync = crate::sync::SyncConf::load();
        (c.stale, c.stale_since_ms, pids) = crate::sync::serve::stale_runs(&sync.checkout, crate::proto::wall_ms());
        let wm = makepad_network::fleet::makepad_home().join("wm");
        c.services_stale = services_running(&wm, &pids);
        c.services_stale.extend(kept_replaced(&wm, &crate::sync::kept_dir(), &crate::procinfo::alive));
        c.services_stale.sort();
        c.services_stale.dedup();
        // its applies build (the roll's push to a node off the auto-sync asks for the build then:
        // a kept service follows the tip only through it, sync/build.rs `keep`)
        if sync.builds() { c.features.push("deploy-builds".into()); }
    }
    #[cfg(feature = "checkout")]
    { c.prewarming = crate::sync::prewarm::running(); }
    #[cfg(feature = "checkout")]
    { c.cef_dist = cef_dist(&crate::sync::SyncConf::load().checkout).map(|d| d.display().to_string()).unwrap_or_default(); }
    c.jit_store = Some(std::sync::Arc::new(super::jitstore::Store::new(&conf.work).snapshot()));
    c.native_trees = conf.native_trees;
    c.cpus_shared = cpu_count(&conf.cpus_shared);
    c.cpus_timed = cpu_count(&conf.cpus_timed);
    #[cfg(feature = "checkout")]
    { c.native = super::tree::natives(&conf.work).into_iter().map(|(lane, _)| lane).collect(); }
    c
}

/// The CEF distribution a checkout holds: `local/cef-prebuilt/current-<platform>.txt` names it
/// (makepad-cef-fetch writes it), or the older `current-<platform>` link; one with `include/`.
/// A live session runs from an overlay, which carries no `local/`: its browser host found none
/// and panicked in its CEF bootstrap (W11267, 11-16).
pub fn cef_dist(checkout: &Path) -> Option<PathBuf> {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macosarm64",
        ("macos", "x86_64") => "macosx64",
        ("linux", "x86_64") => "linux64",
        ("linux", "aarch64") => "linuxarm64",
        ("windows", "x86_64") => "windows64",
        ("windows", "aarch64") => "windowsarm64",
        _ => return None,
    };
    let prebuilt = checkout.join("local/cef-prebuilt");
    let named = std::fs::read_to_string(prebuilt.join(format!("current-{platform}.txt"))).ok()
        .and_then(|t| t.lines().next().map(str::trim).filter(|n| !n.is_empty()).map(|n| prebuilt.join(n)));
    named.into_iter().chain([prebuilt.join(format!("current-{platform}"))]).find(|d| d.join("include").is_dir()).and_then(|d| d.canonicalize().ok())
}

/// The kernel's current clock source on an x86 Linux box (`tsc`; `hpet` or `acpi_pm` when the
/// kernel marked the TSC unstable); empty elsewhere.
pub fn clocksource() -> String {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        std::fs::read_to_string("/sys/devices/system/clocksource/clocksource0/current_clocksource").map(|s| s.trim().to_string()).unwrap_or_default()
    } else {
        String::new()
    }
}

/// Why a box takes no timed leases for its clock, if so: off the TSC every clock read is a slow
/// syscall, which inflates what a timed run measures (a slicing run's wall 35.2 s against 15.8 s
/// on the same hardware on the TSC, the same commit, 11-04). The name only: no per-box setting.
pub fn slow_clock(clocksource: &str) -> Option<String> {
    (!clocksource.is_empty() && clocksource != "tsc").then(|| format!("no timed leases: clocksource {clocksource} (TSC unstable since boot; a reboot restores it)"))
}

/// Why a Mac starts no new lease now, if so: its memory pressure is past normal. What runs there
/// goes on; a lease that starts beside a box at warn pushes it into swap (11-04).
pub fn mac_pressure(level: u32) -> Option<&'static str> {
    match level {
        0 | 1 => None,
        2 => Some("mac pressure warn"),
        _ => Some("mac pressure critical"),
    }
}

/// PSI `full avg10` at which a Linux box starts no new lease ([`memory_stall`]).
pub const MEM_STALL_PCT: f64 = 10.0;

/// Why a Linux box starts no new lease now, if so: every task on it stalled on memory for
/// [`MEM_STALL_PCT`] or more of the last 10 s (PSI `full avg10`). The nodes' swap is small and
/// full, so their memory pressure is page-cache reclaim, which free RAM does not show and PSI
/// measures. What runs there goes on.
pub fn memory_stall(pct: f64) -> Option<String> {
    (pct >= MEM_STALL_PCT).then(|| format!("memory stall {pct:.0}% (PSI full avg10): no new lease until it eases"))
}

/// The `full` line's `avg10` of `/proc/pressure/memory`: the share of the last 10 s, in percent,
/// in which every task stalled on memory; 0 without PSI.
pub fn parse_psi_full(text: &str) -> f64 {
    text.lines().find(|l| l.starts_with("full ")).and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("avg10="))).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// The WM-supervised services that run on this node (`~/.makepad/wm/<name>.state`, written at
/// each start): name and start time.
pub fn services() -> Vec<(String, u64)> {
    services_in(&makepad_network::fleet::makepad_home().join("wm"), &crate::procinfo::alive)
}

/// [`services`] of the state files in `dir`, a service running when `alive` says so of the pid
/// its file records. A state file outlives its service, and the update roll asked a node to
/// restart an AI hub that no longer ran there at each change of its code, the node answering
/// that it does not run it (11-07).
fn services_in(dir: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().strip_suffix(".state")?.to_string();
            state_pid(&std::fs::read_to_string(e.path()).ok()?).filter(|p| alive(*p))?;
            let t = e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
            Some((name, t))
        })
        .collect();
    out.sort();
    out
}

/// The services whose state files in `dir` record one of `pids` (those that run a build their
/// checkout has since replaced, [`crate::sync::serve::stale_runs`]).
#[cfg_attr(not(feature = "checkout"), allow(dead_code))]
fn services_running(dir: &Path, pids: &[u32]) -> Vec<String> {
    if pids.is_empty() { return Vec::new(); }
    let mut out: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| {
        let name = e.file_name().to_string_lossy().strip_suffix(".state")?.to_string();
        state_pid(&std::fs::read_to_string(e.path()).ok()?).filter(|p| pids.contains(p)).map(|_| name)
    }).collect();
    out.sort();
    out
}

/// The services whose state files in `dir` name a kept binary (one in `kept`, [`crate::sync::kept_dir`]:
/// a node no WM runs on starts its services from those) that the prebuild replaced after the
/// service started: the copy's modification time, which `keep` sets as it replaces it, is after
/// the state file's, which the start writes. The update roll restarts them onto it
/// ([`crate::lease::Caps::services_stale`]). A Mac's AI hub ran a two-week-old kept copy, and
/// nothing said it ran older code than its node kept (11-24).
#[cfg_attr(not(feature = "checkout"), allow(dead_code))]
fn kept_replaced(dir: &Path, kept: &Path, alive: &dyn Fn(u32) -> bool) -> Vec<String> {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut out: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| {
        let name = e.file_name().to_string_lossy().strip_suffix(".state")?.to_string();
        let text = std::fs::read_to_string(e.path()).ok()?;
        state_pid(&text).filter(|p| alive(*p))?;
        let binary = PathBuf::from(text.lines().find_map(|l| l.strip_prefix("identity = "))?.split(' ').next()?);
        (binary.parent() == Some(kept) && modified(&binary)? > modified(&e.path())?).then_some(name)
    }).collect();
    out.sort();
    out
}

/// The pid a service's state file records (its `pid = ` line).
pub(super) fn state_pid(text: &str) -> Option<u32> {
    text.lines().find_map(|l| l.strip_prefix("pid = ").and_then(|p| p.trim().parse::<u32>().ok()))
}

/// Why this box's toolchain cannot link, from its last probe (see [`probe_toolchain`]).
static SICK: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
/// Why git cannot work this box's stores: its last store command that failed for the box, not
/// for the request ([`git_failed`]), until a probe of the stores passes ([`probe_git`]).
static GIT_SICK: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Whether this box's last toolchain probe failed: out of rotation until one passes.
pub fn toolchain_sick() -> bool {
    !SICK.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

/// Why this box takes no lease (empty: it takes them).
pub fn sick() -> String {
    let t = SICK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let g = GIT_SICK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut why = Vec::new();
    if !t.is_empty() { why.push(format!("its toolchain cannot link: {t}")); }
    if !g.is_empty() { why.push(format!("git fails on its stores: {g}")); }
    why.join("; ")
}

/// A git error that is the box's, not the request's: git could not start, read its working
/// directory or write the disk. Every later lease there would fail the same way at its start (a
/// node whose hub sat in a removed tree failed every timed job sent to it, 11-03).
fn box_git_error(err: &str) -> bool {
    err.starts_with("git: ") || ["Unable to read current working directory", "No space left on device", "Read-only file system"].iter().any(|p| err.contains(p))
}

/// Why git fails on this box's stores (empty: it works them).
pub fn git_sick() -> String {
    GIT_SICK.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Notes a failed store command: one that is the box's takes it out of rotation until
/// [`probe_git`] passes.
pub fn git_failed(err: &str) {
    if box_git_error(err) {
        *GIT_SICK.lock().unwrap_or_else(|e| e.into_inner()) = crate::proto::clean(err, 200);
    }
}

/// Whether git works this box's stores again: a read of each (no write, nothing fetched), only
/// while it is out of rotation for git (no git process otherwise). Returns (why, changed).
pub fn probe_git() -> (Option<String>, bool) {
    let before = GIT_SICK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if before.is_empty() {
        return (None, false);
    }
    #[cfg(feature = "checkout")]
    let why = {
        let stores = crate::sync::git::stores_dir();
        ["makepad", "commercial"].iter().map(|r| stores.join(format!("{r}.git"))).filter(|s| s.is_dir())
            .find_map(|s| crate::sync::git::run(None, Some(&s), &[], &["rev-parse", "--git-dir"]).err())
    };
    // a hub without the checkout keeps no stores: git starting again is the probe
    #[cfg(not(feature = "checkout"))]
    let why = crate::lease::wt::git(None, None, &["--version"]).err();
    let mut g = GIT_SICK.lock().unwrap_or_else(|e| e.into_inner());
    *g = why.clone().map(|w| crate::proto::clean(&w, 200)).unwrap_or_default();
    let changed = *g != before;
    (why, changed)
}

/// Links a small program the way a build does, system libraries included (a damaged SDK fails every
/// lane's job at its last step). The first line that says why, or None. `changed`: the verdict moved.
/// One probe at a time: the hub's loop and a `toolchain-probe` ask write and link the same files in
/// `dir`, and two rustc runs on them at once could fail one falsely and take the box out of
/// rotation (a second waits for the first, seconds).
pub fn probe_toolchain(dir: &std::path::Path) -> (Option<String>, bool) {
    static PROBING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = PROBING.lock().unwrap_or_else(|e| e.into_inner());
    let src = dir.join("toolchain-probe.rs");
    let out = dir.join(if cfg!(windows) { "toolchain-probe.exe" } else { "toolchain-probe" });
    let text = "#[cfg(target_os = \"macos\")] #[link(name = \"objc\")] #[link(name = \"m\")] #[link(name = \"Foundation\", kind = \"framework\")] #[link(name = \"Metal\", kind = \"framework\")] unsafe extern \"C\" {}\nfn main() {}\n";
    let why = (|| {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(&src, text).map_err(|e| e.to_string())?;
        let o = makepad_remote::group::command("rustc").envs(rust_path().map(|p| ("PATH", p))).args(["--edition", "2021"]).arg(&src).arg("-o").arg(&out).current_dir(dir).stdin(std::process::Stdio::null()).output().map_err(|e| format!("rustc: {e}"))?;
        if o.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        let line = err.lines().find(|l| l.contains("tapi error") || l.contains("ld:") || l.starts_with("error")).unwrap_or("rustc failed").trim().to_string();
        Err(crate::proto::clean(&line, 200))
    })()
    .err();
    let _ = std::fs::remove_file(&out);
    let mut s = SICK.lock().unwrap_or_else(|e| e.into_inner());
    let now = why.clone().unwrap_or_default();
    let changed = *s != now;
    *s = now;
    (why, changed)
}

/// The commit this hub's checkout held when the hub started (its code; read once, from the files
/// under `.git`: the tree moves with leases later).
pub fn hub_rev() -> &'static str {
    static REV: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    REV.get_or_init(|| {
        if let Ok(rev) = std::env::var(BUILT_AT) {
            return rev;
        }
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        // a tree a deploy script or sync wrote with no `.git` of its own (a node's) names its commit in
        // FLEET-REVISION.txt
        head_sha(&repo).or_else(|| {
            let t = std::fs::read_to_string(repo.join("FLEET-REVISION.txt")).ok()?;
            t.lines().find_map(|l| l.strip_prefix("makepad ").map(|s| s.trim().to_string())).filter(|s| s.len() == 40)
        }).unwrap_or_default()
    })
}

/// What a master that restarts itself tells the build it execs into: the commit it built. Its
/// checkout's HEAD has moved on when a landing came during the build or the drain, and a master
/// that took that HEAD for its own never built the landing (its CLI stayed behind, 11-04).
pub const BUILT_AT: &str = "DEVHUB_BUILT_AT";

/// The commit the master's own build of this binary was built at ([`BUILD_REV`], which its build
/// sets: `orch::roll`'s `build_devhub`); none for a build by hand or under the JIT. A CLI compares
/// it with the master's (`master_rev` in an error's answer) to say which side lags.
pub fn cli_build_rev() -> Option<&'static str> {
    option_env!("DEVHUB_BUILD_REV").filter(|rev| !rev.is_empty())
}

/// The variable the master's build of devhub's binary names its commit in ([`cli_build_rev`]).
pub const BUILD_REV: &str = "DEVHUB_BUILD_REV";

/// HEAD's commit of a checkout, from its files (a linked worktree's `.git` file, a loose ref or
/// packed-refs).
pub fn head_sha(tree: &std::path::Path) -> Option<String> {
    let dot = tree.join(".git");
    let dir = if dot.is_dir() { dot } else { tree.join(std::fs::read_to_string(&dot).ok()?.trim().strip_prefix("gitdir:")?.trim()) };
    let common = std::fs::read_to_string(dir.join("commondir")).ok().map_or(dir.clone(), |c| dir.join(c.trim()));
    let head = std::fs::read_to_string(dir.join("HEAD")).ok()?;
    let head = head.trim();
    let Some(r) = head.strip_prefix("ref:").map(str::trim) else { return Some(head.to_string()) };
    for base in [&dir, &common] {
        if let Ok(s) = std::fs::read_to_string(base.join(r)) {
            return Some(s.trim().to_string());
        }
    }
    let packed = std::fs::read_to_string(common.join("packed-refs")).ok()?;
    packed.lines().find_map(|l| l.strip_suffix(r).map(str::trim).filter(|s| s.len() == 40)).map(str::to_string)
}

pub const GB: f64 = (1u64 << 30) as f64;

/// The `-C target-cpu` every program this box runs may be built for ([`Caps::cpu_target`]): on
/// x86-64 the highest psABI level its CPU has all of, read from the CPU at run time (so a box with
/// no rustc says it too); on an Apple chip (`model`, its brand string) its `apple-m<N>`; elsewhere
/// `generic`. A build `--for` the box takes it in place of `native`, the building box's own CPU:
/// a pool box's AVX-512 build stopped with "Illegal instruction" on a box without it (11-07).
pub fn cpu_target(model: &str) -> String {
    match (apple_target(model), x86_features()) {
        (Some(apple), _) => apple,
        (None, Some((v2, v3, v4))) => x86_level(v2, v3, v4).to_string(),
        (None, None) => "generic".into(),
    }
}

/// Whether the CPU has all of the x86-64 psABI v2, v3 and v4 features; None: not x86-64.
#[cfg(target_arch = "x86_64")]
fn x86_features() -> Option<(bool, bool, bool)> {
    use std::arch::is_x86_feature_detected as has;
    let v2 = has!("cmpxchg16b") && has!("popcnt") && has!("sse3") && has!("ssse3") && has!("sse4.1") && has!("sse4.2");
    let v3 = has!("avx") && has!("avx2") && has!("bmi1") && has!("bmi2") && has!("f16c") && has!("fma") && has!("lzcnt") && has!("movbe") && has!("xsave");
    let v4 = has!("avx512f") && has!("avx512bw") && has!("avx512cd") && has!("avx512dq") && has!("avx512vl");
    Some((v2, v3, v4))
}

#[cfg(not(target_arch = "x86_64"))]
fn x86_features() -> Option<(bool, bool, bool)> {
    None
}

/// The x86-64 psABI level of a CPU with all of each level's features (`v2`, `v3`, `v4`): a level
/// counts only on top of the ones below it.
fn x86_level(v2: bool, v3: bool, v4: bool) -> &'static str {
    match (v2, v2 && v3, v2 && v3 && v4) {
        (_, _, true) => "x86-64-v4",
        (_, true, _) => "x86-64-v3",
        (true, _, _) => "x86-64-v2",
        _ => "x86-64",
    }
}

/// rustc's name for an Apple chip by its brand string (`Apple M<n> <tier>`: `apple-m<n>`).
fn apple_target(model: &str) -> Option<String> {
    let n: String = model.strip_prefix("Apple M")?.chars().take_while(char::is_ascii_digit).collect();
    (!n.is_empty()).then(|| format!("apple-m{n}"))
}

/// `model name` and the number of logical CPUs from `/proc/cpuinfo`.
pub fn parse_cpuinfo(text: &str) -> (String, u32) {
    let mut model = String::new();
    let mut n = 0;
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        match k.trim() {
            "processor" => n += 1,
            // x86: `model name`; arm64 kernels may only give `Hardware` / `CPU part`
            "model name" | "Hardware" if model.is_empty() => model = v.trim().to_string(),
            _ => {}
        }
    }
    (model, n)
}

/// (MemTotal, MemAvailable) in GB from `/proc/meminfo`.
pub fn parse_meminfo(text: &str) -> (f64, f64) {
    let kb = |key: &str| text.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / (1024.0 * 1024.0);
    (kb("MemTotal:"), kb("MemAvailable:"))
}

pub fn parse_loadavg(text: &str) -> f64 {
    text.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// The ns its tasks waited for a CPU, summed over the CPUs of `/proc/schedstat` (each `cpuN`
/// line's eighth count, the run delay); None without such lines.
pub fn parse_schedstat_wait(text: &str) -> Option<u64> {
    let per_cpu: Vec<u64> = text.lines().filter(|l| l.starts_with("cpu")).filter_map(|l| l.split_whitespace().nth(8)?.parse().ok()).collect();
    (!per_cpu.is_empty()).then(|| per_cpu.iter().sum())
}

/// The tasks that waited for a CPU on average between two looks at [`parse_schedstat_wait`]'s
/// sums `secs` apart.
pub fn cpu_wait(before: u64, after: u64, secs: f64) -> Option<f64> {
    (secs > 0.0 && after >= before).then(|| (after - before) as f64 / 1e9 / secs)
}

/// Chip limits may replace a class default only nearby; low DIMM maxima are not load alerts.
fn temp_limit(chip: &str, label: &str, max: Option<f64>, crit: Option<f64>) -> Option<f64> {
    let default = match chip {
        "k10temp" if label == "Tctl" || label.starts_with("Tccd") => Some(90.0),
        "coretemp" if label.starts_with("Package") => Some(90.0),
        "amdgpu" => Some(90.0),
        "nvme" => Some(80.0),
        "spd5118" => Some(70.0),
        c if c.starts_with("r8169") => Some(95.0),
        c if c.starts_with("mt7921") => Some(90.0),
        _ => None,
    };
    let own = max.or(crit);
    match default {
        Some(d) => Some(own.filter(|l| (*l - d).abs() <= 10.0).unwrap_or(d)),
        None => own.filter(|l| (50.0..=120.0).contains(l)),
    }
}

/// All readable hwmon inputs, in Celsius. A root parameter keeps sysfs out of fixture tests.
pub fn hwmon_temps(root: &Path) -> Vec<Temp> {
    let degrees = |path: &Path| std::fs::read_to_string(path).ok()?.trim().parse::<i64>().ok().map(|m| m as f64 / 1000.0);
    let mut temps = Vec::new();
    for chip_dir in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if !chip_dir.file_name().to_string_lossy().starts_with("hwmon") { continue; }
        let dir = chip_dir.path();
        let Ok(chip) = std::fs::read_to_string(dir.join("name")) else { continue };
        let chip = chip.trim();
        if chip.is_empty() { continue; }
        for input in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = input.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix("_input") else { continue };
            if !stem.strip_prefix("temp").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) { continue; }
            let Some(c) = degrees(&input.path()) else { continue };
            let label = std::fs::read_to_string(dir.join(format!("{stem}_label"))).unwrap_or_else(|_| stem.to_string());
            let label = label.trim();
            let limit = temp_limit(chip, label, degrees(&dir.join(format!("{stem}_max"))), degrees(&dir.join(format!("{stem}_crit"))));
            let sensor = format!("{chip}:{label}").split_whitespace().collect::<Vec<_>>().join("_");
            temps.push(Temp { sensor, c, limit });
        }
    }
    temps.sort_by(|a, b| a.sensor.cmp(&b.sensor).then(a.c.total_cmp(&b.c)));
    temps
}

/// Uses the GPU query caps already made; no second nvidia-smi process per poll.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn nvidia_temps(gpus: &[crate::top::GpuUse]) -> Vec<Temp> {
    gpus.iter().enumerate().filter(|(_, g)| g.temp_c.is_finite()).map(|(i, g)| Temp { sensor: format!("gpu{i}"), c: g.temp_c, limit: Some(85.0) }).collect()
}

/// Hysteresis lives with the hub, so master polls do not flap at the alert limit.
#[derive(Default)]
pub(super) struct Thermal {
    hot: BTreeMap<String, HotTemp>,
}

impl Thermal {
    /// A hot sensor needs a reading below limit - 5 to cool; a missing reading is not cooling.
    pub(super) fn update(&mut self, temps: &[Temp]) -> Vec<HotTemp> {
        let previous = std::mem::take(&mut self.hot);
        for t in temps {
            let Some(limit) = t.limit else { continue };
            let hot = if previous.contains_key(&t.sensor) { t.c >= limit - 5.0 } else { t.c >= limit };
            if hot {
                // Identically named chips (several DIMMs) stay hot while any of them is hot.
                if self.hot.get(&t.sensor).is_none_or(|h| t.c - limit > h.c - h.limit) {
                    self.hot.insert(t.sensor.clone(), HotTemp { sensor: t.sensor.clone(), c: t.c, limit });
                }
            }
        }
        for (sensor, t) in previous {
            if !temps.iter().any(|t| t.sensor == sensor) { self.hot.insert(sensor, t); }
        }
        self.hot.values().cloned().collect()
    }
}

/// `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`: (name, VRAM GB).
pub fn parse_nvidia(text: &str) -> Vec<(String, f64)> {
    text.lines()
        .filter_map(|l| {
            let (name, mib) = l.rsplit_once(',')?;
            Some((name.trim().to_string(), mib.trim().parse::<f64>().ok()? / 1024.0))
        })
        .filter(|(n, _)| !n.is_empty())
        .collect()
}

/// Whether a `/sys/class/power_supply` entry is the machine's battery (`type`, `scope` files).
pub fn system_battery(kind: &str, scope: &str) -> bool {
    kind.trim() == "Battery" && scope.trim() != "Device"
}

/// A battery from `/sys/class/power_supply/<x>/{capacity,status}`: (percent, charging).
pub fn parse_battery(capacity: &str, status: &str) -> Option<(u32, bool)> {
    let pct = capacity.trim().parse().ok()?;
    Some((pct, matches!(status.trim(), "Charging" | "Full")))
}

/// How long [`load_now`] measures the CPU for on Linux and Windows, whose counters move as the
/// time goes; macOS publishes its own about once a second, and a measure there spans the next
/// publication (at most [`LOAD_WAIT`] after its first read).
pub const LOAD_WINDOW: std::time::Duration = std::time::Duration::from_millis(100);

/// How long a macOS [`load_now`] waits for its CPU counters to move: past it (the counters stood
/// still for three of their periods) the answer has no load, said in one line.
#[cfg(target_os = "macos")]
pub const LOAD_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// The box's load now: (CPU use in percent over [`LOAD_WINDOW`], or on macOS over the counters'
/// last whole period, available RAM in GB). A lease-end answers it after the teardown: the caps
/// the master holds are a 10 s average from up to 10 s ago and still count the ended lease's work
/// (a medium lease waited 23 s, two caps rounds, after two small tests that pegged its box had
/// ended, 11-04).
pub fn load_now() -> Option<(f64, f64)> {
    os::load_now()
}

/// (this box's RAM, what its swap files take of its disk), bytes, where swap grows on the volume
/// it swaps to: macOS. A Linux box swaps to a fixed file or partition, so memory cannot fill its
/// disk (the kernel's OOM killer ends what outgrows the RAM).
#[cfg(target_os = "macos")]
pub fn swap_on_disk() -> Option<(u64, u64)> {
    os::swap_on_disk()
}
#[cfg(not(target_os = "macos"))]
pub fn swap_on_disk() -> Option<(u64, u64)> {
    None
}

/// (free, total) bytes of the volume holding `path` (its nearest existing ancestor).
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let mut p = path.to_path_buf();
    while !p.is_dir() {
        p = p.parent()?.to_path_buf();
    }
    os::disk_space(&p)
}

// Android: bionic shares the /proc files, statvfs layout and power-supply nodes
#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use super::*;

    pub fn hardware() -> Caps {
        let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
        let (cpu, cores) = parse_cpuinfo(&read("/proc/cpuinfo"));
        let cpu_target = cpu_target(&cpu);
        let (ram, avail) = parse_meminfo(&read("/proc/meminfo"));
        let gpu = crate::top::gpu_use();
        let mut temps = hwmon_temps(Path::new("/sys/class/hwmon"));
        temps.extend(nvidia_temps(&gpu));
        // CPU use since the previous measure (the master asks every 10 s)
        static LAST: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);
        let now = crate::top::parse_proc_stat(&read("/proc/stat"));
        let cpu_pct = match (LAST.lock().unwrap().replace(now.unwrap_or((0, 0))), now) {
            (Some(a), Some(b)) if b.1 > a.1 => Some(crate::top::cpu_pct(a, b)),
            _ => None,
        };
        // and the tasks that waited for a CPU meanwhile: the demand past the cores
        static WAITED: std::sync::Mutex<Option<(std::time::Instant, u64)>> = std::sync::Mutex::new(None);
        let waited = parse_schedstat_wait(&read("/proc/schedstat")).map(|ns| (std::time::Instant::now(), ns));
        let cpu_wait = match (std::mem::replace(&mut *WAITED.lock().unwrap(), waited), waited) {
            (Some((t0, a)), Some((t1, b))) => cpu_wait(a, b, t1.duration_since(t0).as_secs_f64()),
            _ => None,
        };
        Caps {
            temps,
            cpu_pct,
            cpu_wait,
            gpu_busy: gpu.iter().map(|g| (g.util_pct, g.mem_used_gb)).collect(),
            arch: std::env::consts::ARCH.to_string(),
            os: std::env::consts::OS.to_string(),
            cpu,
            cpu_target,
            cores: cores.max(1),
            load1: parse_loadavg(&read("/proc/loadavg")),
            ram_gb: ram,
            ram_avail_gb: avail,
            mem_stall: parse_psi_full(&read("/proc/pressure/memory")),
            gpus: gpu.iter().map(|g| (g.name.clone(), g.mem_total_gb)).collect(),
            display: display(),
            // read after display(): its probe leaves the answer of a WM on another wire
            wm_wire: OTHER_WIRE.lock().unwrap().clone().unwrap_or_default(),
            wm_dark: wm_dark(),
            battery: battery(),
            ..Default::default()
        }
    }

    pub fn load_now() -> Option<(f64, f64)> {
        let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
        let a = crate::top::parse_proc_stat(&read("/proc/stat"))?;
        std::thread::sleep(LOAD_WINDOW);
        let b = crate::top::parse_proc_stat(&read("/proc/stat"))?;
        (b.1 > a.1).then(|| (crate::top::cpu_pct(a, b), parse_meminfo(&read("/proc/meminfo")).1))
    }

    /// The box's live makepad WM session. An arbitrary Wayland compositor (including a
    /// headless one) is not a display on which CI may boot apps.
    pub(super) fn display() -> Option<String> {
        let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", uid())));
        let mut sockets: Vec<_> = std::fs::read_dir(&rt).ok()?.flatten().filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let index = name.strip_prefix("makepad-wm-")?.parse::<u32>().ok()?;
            Some((index, name, e.path()))
        }).collect();
        sockets.sort_by_key(|(index, _, _)| *index);
        let mut other = None;
        let found = sockets.into_iter().find_map(|(_, name, path)| match hosts_apps(&path) {
            Ok(()) => Some(format!("wm:{name}")),
            Err(answer) => {
                other = other.take().or(answer.map(|a| format!("{name} answered {a:?}")));
                None
            }
        });
        *OTHER_WIRE.lock().unwrap() = if found.is_some() { None } else { other };
        found
    }

    pub(super) static OTHER_WIRE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    /// Why the box's WM shows nothing ([`super::dark_of`] of its `status`); empty when it shows
    /// something, or does not answer. By its socket: a bare name reaches only what the caller's
    /// lease started, and this hub starts no WM, so asked by name it was refused on every box and
    /// read as a WM that shows something (no box ever said it, 0 of the windows that timed out
    /// under a 0x0 WM, 11-09).
    fn wm_dark() -> String {
        let apps = makepad_remote::discovery::apps();
        let Some(wm) = super::node_wm(&apps) else { return String::new() };
        let Ok(wm) = makepad_remote::link::Client::connect(&wm.socket) else { return String::new() };
        wm.set_timeout(Some(std::time::Duration::from_secs(3)));
        match wm.cmd("status").ok().and_then(|mut lines| lines.next()) {
            Some(Ok(line)) => super::dark_of(&line, &drm_connectors()),
            _ => String::new(),
        }
    }

    /// The box's DRM connectors (`card1-DP-1`) and their states (`connected`, `disconnected`,
    /// `unknown`).
    fn drm_connectors() -> Vec<(String, String)> {
        let Ok(dir) = std::fs::read_dir("/sys/class/drm") else { return Vec::new() };
        dir.flatten().filter_map(|e| Some((e.file_name().to_string_lossy().into_owned(), std::fs::read_to_string(e.path().join("status")).ok()?.trim().to_string()))).collect()
    }

    /// Whether the WM at `path` takes today's apps, asked the way an app asks it
    /// (platform/hal wm_socket): `hello2` first; a WM from before that closes it unanswered and
    /// is asked the legacy `hello <wire>` with the apps' wire fingerprint. A WM on another wire
    /// cannot host them (Fab could not start on a node, 2026-11-02), so the box is no display for
    /// gui work until its WM restarts. A slow answer is no evidence of another wire.
    /// A WM that speaks a common hosted-protocol version hosts them whatever its wire fingerprint:
    /// an app admitted at a version states that version and the WM's own fingerprint at its hub
    /// (hal `wm_socket.rs` `adopt_with`, libs/app_host `hub.rs`). Before apps did, a pool box's WM
    /// of an older fingerprint adopted each at the socket and its hub closed it: a lane's live Fab
    /// 6 s into its boot, the node's tray and AI Hub every few seconds (L1206053, 11-16).
    /// `Err(Some(answer))`: a WM on another wire; `Err(None)`: none answers there.
    pub(super) fn hosts_apps(path: &Path) -> Result<(), Option<String>> {
        use std::io::{BufRead, ErrorKind, Read, Write};
        use std::os::unix::net::UnixStream;
        enum Answer { Dead, Slow, Line(String) }
        let ask = |line: String| {
            let Ok(s) = UnixStream::connect(path) else { return Answer::Dead };
            let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(3)));
            let _ = s.set_write_timeout(Some(std::time::Duration::from_secs(3)));
            if (&s).write_all(line.as_bytes()).is_err() {
                return Answer::Dead;
            }
            let mut answer = String::new();
            match std::io::BufReader::new(&s).take(512).read_line(&mut answer) {
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => Answer::Slow,
                // closed unanswered (or reset): an old WM's way of not knowing the line
                _ => Answer::Line(answer.trim().to_string()),
            }
        };
        let said = |a: &str, what: &str| a.strip_prefix("makepad-wm ").is_some_and(|r| r.starts_with(what));
        let legacy = || ask(format!("makepad-wm hello {:016x}\n", makepad_studio_protocol::WIRE));
        match ask("makepad-wm hello2 1-65535\n".into()) {
            Answer::Dead => Err(None),
            Answer::Slow => Ok(()),
            Answer::Line(a) if said(&a, "ok ") => Ok(()),
            Answer::Line(a) if said(&a, "incompatible ") => Err(Some(a)),
            Answer::Line(_) => match legacy() {
                Answer::Slow => Ok(()),
                Answer::Line(a) if said(&a, "ok") => Ok(()),
                Answer::Line(a) if said(&a, "mismatch ") => Err(Some(a)),
                _ => Err(None),
            },
        }
    }

    fn uid() -> u32 {
        extern "C" {
            fn getuid() -> u32;
        }
        unsafe { getuid() }
    }

    /// The machine's own battery: a power supply of type Battery whose scope is not `Device`
    /// (wireless mice and keyboards report theirs as `scope = Device`).
    fn battery() -> Option<(u32, bool)> {
        for e in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
            let p = e.path();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default();
            if system_battery(&read("type"), &read("scope")) {
                if let Some(b) = parse_battery(&std::fs::read_to_string(p.join("capacity")).unwrap_or_default(), &std::fs::read_to_string(p.join("status")).unwrap_or_default()) {
                    return Some(b);
                }
            }
        }
        None
    }

    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::ffi::OsStrExt;
        // 64-bit glibc and bionic share this layout
        #[repr(C)]
        struct StatVfs {
            bsize: u64,
            frsize: u64,
            blocks: u64,
            bfree: u64,
            bavail: u64,
            files: u64,
            ffree: u64,
            favail: u64,
            fsid: u64,
            flag: u64,
            namemax: u64,
            spare: [i32; 6],
        }
        extern "C" {
            fn statvfs(path: *const std::ffi::c_char, out: *mut StatVfs) -> i32;
        }
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: StatVfs = unsafe { std::mem::zeroed() };
        if unsafe { statvfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        let unit = if s.frsize > 0 { s.frsize } else { s.bsize };
        Some((s.bavail.saturating_mul(unit), s.blocks.saturating_mul(unit)))
    }
}

#[cfg(target_os = "macos")]
mod os {
    use super::*;

    extern "C" {
        fn sysctlbyname(name: *const u8, oldp: *mut core::ffi::c_void, oldlenp: *mut usize, newp: *mut core::ffi::c_void, newlen: usize) -> i32;
    }

    fn sysctl_u64(name: &str) -> u64 {
        let n = format!("{name}\0");
        let mut v = 0u64;
        let mut len = std::mem::size_of::<u64>();
        if unsafe { sysctlbyname(n.as_ptr(), &mut v as *mut u64 as *mut _, &mut len, std::ptr::null_mut(), 0) } != 0 {
            return 0;
        }
        // 4-byte answers (hw.logicalcpu) land in the low half
        if len == 4 { v & 0xffff_ffff } else { v }
    }

    pub(super) fn swap_on_disk() -> Option<(u64, u64)> {
        // struct xsw_usage (sys/sysctl.h)
        #[repr(C)]
        #[derive(Default)]
        struct Usage { total: u64, avail: u64, used: u64, pagesize: u32, encrypted: u32 }
        let mut u = Usage::default();
        let mut len = std::mem::size_of::<Usage>();
        if unsafe { sysctlbyname(b"vm.swapusage\0".as_ptr(), &mut u as *mut Usage as *mut _, &mut len, std::ptr::null_mut(), 0) } != 0 {
            return None;
        }
        Some((sysctl_u64("hw.memsize"), u.total))
    }

    fn sysctl_str(name: &str) -> String {
        let n = format!("{name}\0");
        let mut buf = [0u8; 256];
        let mut len = buf.len();
        if unsafe { sysctlbyname(n.as_ptr(), buf.as_mut_ptr() as *mut _, &mut len, std::ptr::null_mut(), 0) } != 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..len]).trim_end_matches('\0').to_string()
    }

    extern "C" {
        fn mach_host_self() -> u32;
        fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    }

    /// (busy, total) CPU ticks of all cores: `host_statistics(HOST_CPU_LOAD_INFO)` (user, system,
    /// idle, nice), the macOS twin of `/proc/stat`'s `cpu` line. The kernel publishes them about
    /// once a second, every core's period at once: on a 16-core M-series box two in three windows
    /// of 100 ms saw them stand still, idle or with every core spinning, and windows of 1 s each
    /// saw a period's 1600 ticks (11-14).
    fn cpu_ticks() -> Option<(u64, u64)> {
        let mut t = [0u32; 4];
        let mut count = 4u32;
        // HOST_CPU_LOAD_INFO = 3, HOST_CPU_LOAD_INFO_COUNT = 4 (natural_t words)
        if unsafe { host_statistics(mach_host_self(), 3, t.as_mut_ptr() as *mut i32, &mut count) } != 0 {
            return None;
        }
        let busy = t[0] as u64 + t[1] as u64 + t[3] as u64;
        Some((busy, busy + t[2] as u64))
    }

    /// The CPU over the counters' last whole period: from the first read (the last publication)
    /// to the next publication, which comes within about a second. A fixed window of 100 ms most
    /// often saw no tick at all, and the lease-end answer then had no load.
    pub fn load_now() -> Option<(f64, f64)> {
        let a = cpu_ticks()?;
        let t0 = std::time::Instant::now();
        let b = loop {
            std::thread::sleep(std::time::Duration::from_millis(10));
            let b = cpu_ticks()?;
            if b.1 > a.1 {
                break b;
            }
            if t0.elapsed() >= LOAD_WAIT {
                eprintln!("devhub: the CPU counters stood still for {LOAD_WAIT:?}: a lease-end answers no load");
                return None;
            }
        };
        Some((crate::top::cpu_pct(a, b), crate::procinfo::machine().mem_avail_mb as f64 / 1024.0))
    }

    pub fn hardware() -> Caps {
        let m = crate::procinfo::machine();
        // CPU use since the previous measure (the master asks every 10 s): what decides "busy", not
        // the 1-minute load average, which stays high for a minute after a lease's build ended
        // (a measure within the same period of the counters as the last one, from another caller,
        // answers the last value and keeps its base: the kernel publishes them about once a second)
        static LAST: std::sync::Mutex<(Option<(u64, u64)>, Option<f64>)> = std::sync::Mutex::new((None, None));
        let now = cpu_ticks();
        let cpu_pct = {
            let mut last = LAST.lock().unwrap();
            match (last.0, now) {
                (Some(a), Some(b)) if b.1 > a.1 => *last = (now, Some(crate::top::cpu_pct(a, b))),
                (Some(_), Some(_)) => {}
                _ => *last = (now, None),
            }
            last.1
        };
        Caps {
            cpu_pct,
            arch: std::env::consts::ARCH.to_string(),
            os: "macos".into(),
            cpu_target: cpu_target(&sysctl_str("machdep.cpu.brand_string")),
            cpu: sysctl_str("machdep.cpu.brand_string"),
            cores: (sysctl_u64("hw.logicalcpu") as u32).max(1),
            load1: m.load1,
            ram_gb: sysctl_u64("hw.memsize") as f64 / GB,
            ram_avail_gb: m.mem_avail_mb as f64 / 1024.0,
            mem_pressure: sysctl_u64("kern.memorystatus_vm_pressure_level") as u32,
            // a Mac shows windows on its own session; its GPU is no lease resource (not in `gpus`),
            // but it has one: Apple silicon's is its chip's, an Intel Mac's its Metal device
            shared_gpu: if cfg!(target_arch = "aarch64") { sysctl_str("machdep.cpu.brand_string") } else { "Metal".into() },
            display: Some("macos".into()),
            ..Default::default()
        }
    }

    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::ffi::OsStrExt;
        #[repr(C)]
        struct StatFs {
            bsize: u32,
            iosize: i32,
            blocks: u64,
            bfree: u64,
            bavail: u64,
            files: u64,
            ffree: u64,
            fsid: [i32; 2],
            owner: u32,
            fstype: u32,
            flags: u32,
            fssubtype: u32,
            fstypename: [u8; 16],
            mntonname: [u8; 1024],
            mntfromname: [u8; 1024],
            flags_ext: u32,
            reserved: [u32; 7],
        }
        extern "C" {
            #[cfg_attr(target_arch = "x86_64", link_name = "statfs$INODE64")]
            fn statfs(path: *const std::ffi::c_char, out: *mut StatFs) -> i32;
        }
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: StatFs = unsafe { std::mem::zeroed() };
        if unsafe { statfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        Some((s.bavail.saturating_mul(s.bsize as u64), s.blocks.saturating_mul(s.bsize as u64)))
    }
}

#[cfg(windows)]
mod os {
    use super::*;

    pub fn load_now() -> Option<(f64, f64)> {
        let a = crate::os::cpu_times()?;
        std::thread::sleep(LOAD_WINDOW);
        let b = crate::os::cpu_times()?;
        (b.1 > a.1).then(|| (crate::top::cpu_pct(a, b), crate::os::memory().1 as f64 / GB))
    }

    pub fn hardware() -> Caps {
        let (total, avail) = crate::os::memory();
        // CPU use since the previous measure (the master asks every 10 s)
        static LAST: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);
        let now = crate::os::cpu_times();
        let cpu_pct = match (LAST.lock().unwrap().replace(now.unwrap_or((0, 0))), now) {
            (Some(a), Some(b)) if b.1 > a.1 => Some(crate::top::cpu_pct(a, b)),
            _ => None,
        };
        let gpu = crate::top::gpu_use();
        Caps {
            cpu_pct,
            gpu_busy: gpu.iter().map(|g| (g.util_pct, g.mem_used_gb)).collect(),
            arch: std::env::consts::ARCH.to_string(),
            os: "windows".into(),
            cpu_target: cpu_target(&crate::os::cpu_name()),
            cpu: crate::os::cpu_name(),
            cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1),
            // no load average on Windows: the busy share of the cores stands in
            load1: cpu_pct.map_or(0.0, |p| p / 100.0 * std::thread::available_parallelism().map(|n| n.get() as f64).unwrap_or(1.0)),
            ram_gb: total as f64 / GB,
            ram_avail_gb: avail as f64 / GB,
            gpus: gpu.iter().map(|g| (g.name.clone(), g.mem_total_gb)).collect(),
            // the user's desktop session (a lease's windows go there only with display=1)
            display: Some("windows".into()),
            battery: crate::os::battery(),
            ..Default::default()
        }
    }

    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        crate::os::disk_space(path)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
mod os {
    use super::*;

    pub fn hardware() -> Caps {
        Caps { arch: std::env::consts::ARCH.to_string(), os: std::env::consts::OS.to_string(), cores: 1, ..Default::default() }
    }

    pub fn load_now() -> Option<(f64, f64)> {
        None
    }

    pub fn disk_space(_path: &Path) -> Option<(u64, u64)> {
        None
    }
}

#[cfg(test)]
mod tests {
    /// A box says the targets its toolchain holds a std for: the target directories of its sysroot
    /// with a libcore, not the sources, tools or a directory left without one.
    #[test]
    fn a_box_reports_the_targets_its_sysroot_holds_a_std_for() {
        let root = std::env::temp_dir().join(format!("devhub-sysroot-{}-{}", std::process::id(), crate::proto::wall_ms()));
        let rustlib = root.join("lib").join("rustlib");
        for (dir, file) in [("x86_64-unknown-linux-gnu/lib", "libcore-0a1b.rlib"), ("wasm32-unknown-unknown/lib", "libcore-2c3d.rlib"), ("aarch64-apple-ios/lib", "libstd-4e5f.rlib"), ("src/rust/library/core", "lib.rs"), ("etc", "gdb_load_rust_pretty_printers.py")] {
            std::fs::create_dir_all(rustlib.join(dir)).unwrap();
            std::fs::write(rustlib.join(dir).join(file), b"").unwrap();
        }
        std::fs::write(rustlib.join("components"), b"rustc\n").unwrap();
        let got = super::installed_targets(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(got, ["wasm32-unknown-unknown", "x86_64-unknown-linux-gnu"]);
        assert!(super::installed_targets(&root).is_empty(), "no sysroot: none");
    }

    /// A WM is asked the way an app is: one that speaks a common version hosts apps whatever its
    /// wire fingerprint, as an app admitted at that version states the WM's own at its hub.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_wm_that_speaks_a_common_version_hosts_apps_whatever_its_fingerprint() {
        use std::io::{BufRead, Write};
        let serve = |name: &str, legacy: &'static str| {
            let path = std::env::temp_dir().join(format!("devhub-wmwire-{name}-{}-{}", std::process::id(), crate::proto::wall_ms()));
            let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
            std::thread::spawn(move || {
                for stream in listener.incoming().take(2) {
                    let Ok(stream) = stream else { return };
                    let mut line = String::new();
                    let _ = std::io::BufReader::new(&stream).read_line(&mut line);
                    let answer = if line.starts_with("makepad-wm hello2 ") { "makepad-wm ok 1\n" } else { legacy };
                    let _ = (&stream).write_all(answer.as_bytes());
                }
            });
            path
        };
        let other = serve("other", "makepad-wm mismatch 0123456789abcdef\n");
        assert_eq!(super::os::hosts_apps(&other), Ok(()), "a common version on another fingerprint hosts apps");
        let same = serve("same", "makepad-wm ok\n");
        assert_eq!(super::os::hosts_apps(&same), Ok(()), "a WM of the apps' wire hosts them");
        for path in [other, same] { let _ = std::fs::remove_file(path); }
    }

    #[test]
    fn hwmon_reads_labels_millidegrees_and_class_limits() {
        let dir = std::env::temp_dir().join(format!("devhub-hwmon-{}-{}", std::process::id(), crate::proto::wall_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases = [
            ("k10temp", Some("Tctl"), Some(95_000), Some(100_000), Some(95.0)),
            ("k10temp", Some("Tccd1"), None, Some(100_000), Some(100.0)),
            ("coretemp", Some("Package id 0"), Some(70_000), None, Some(90.0)),
            ("amdgpu", Some("junction"), None, None, Some(90.0)),
            ("nvme", Some("Composite"), Some(70_000), None, Some(70.0)),
            ("r8169_phy", None, None, Some(105_000), Some(105.0)),
            ("spd5118", None, Some(55_000), Some(85_000), Some(70.0)),
            ("mt7921_phy", None, None, None, Some(90.0)),
            ("unknown chip", Some("Board temp"), Some(60_000), Some(75_000), Some(60.0)),
            ("unknown", Some("critical"), None, Some(88_500), Some(88.5)),
            ("unknown", Some("low"), Some(49_000), Some(95_000), None),
            ("unknown", Some("high"), None, Some(121_000), None),
            ("unknown", Some("lower edge"), Some(50_000), None, Some(50.0)),
            ("unknown", Some("upper edge"), None, Some(120_000), Some(120.0)),
            ("unknown", None, None, None, None),
        ];
        for (i, (chip, label, max, crit, _)) in cases.iter().enumerate() {
            let chip_dir = dir.join(format!("hwmon{i}"));
            std::fs::create_dir(&chip_dir).unwrap();
            std::fs::write(chip_dir.join("name"), format!("{chip}\n")).unwrap();
            std::fs::write(chip_dir.join("temp1_input"), "61125\n").unwrap();
            if let Some(label) = label { std::fs::write(chip_dir.join("temp1_label"), format!("{label}\n")).unwrap(); }
            for (suffix, value) in [("max", max), ("crit", crit)] {
                if let Some(value) = value { std::fs::write(chip_dir.join(format!("temp1_{suffix}")), format!("{value}\n")).unwrap(); }
            }
        }
        std::fs::write(dir.join("hwmon0/temp2_input"), "42125\n").unwrap();
        std::fs::write(dir.join("hwmon0/temp3_input"), "unreadable\n").unwrap();
        std::fs::write(dir.join("hwmon0/in1_input"), "1200\n").unwrap();
        let temps = super::hwmon_temps(&dir);
        assert_eq!(temps.len(), cases.len() + 1, "every temperature input, excluding invalid input and voltage");
        for (chip, label, _, _, limit) in cases {
            let name = format!("{chip}:{}", label.unwrap_or("temp1")).replace(' ', "_");
            let t = temps.iter().find(|t| t.sensor == name).expect(&name);
            assert_eq!((t.c, t.limit), (61.125, limit), "{name}: millidegrees and the class or chip limit");
        }
        let second = temps.iter().find(|t| t.sensor == "k10temp:temp2").unwrap();
        assert_eq!((second.c, second.limit), (42.125, None), "an unlabelled second input uses its stem and has no CPU-class default");
        assert!(super::hwmon_temps(&dir.join("absent")).is_empty(), "no hwmon is an empty reading");
        std::fs::remove_dir_all(&dir).unwrap();
        let gpus = crate::top::parse_gpu_use("GPU-A, 20, 1024, 8192, 39, 50\nGPU-B, 30, 2048, 8192, 85, 90\n");
        let temps = super::nvidia_temps(&gpus);
        assert_eq!(temps, vec![super::Temp { sensor: "gpu0".into(), c: 39.0, limit: Some(85.0) }, super::Temp { sensor: "gpu1".into(), c: 85.0, limit: Some(85.0) }], "the existing GPU query supplies every NVIDIA temperature");
    }

    #[test]
    fn thermal_hysteresis_waits_for_five_degrees_of_cooling() {
        let mut thermal = super::Thermal::default();
        let dimm = |c| super::Temp { sensor: "spd5118:temp1".into(), c, limit: Some(70.0) };
        let unknown = super::Temp { sensor: "unknown:temp1".into(), c: 150.0, limit: None };
        assert!(thermal.update(&[dimm(69.0), unknown.clone()]).is_empty(), "below the limit stays cool; a sensor with no limit never alerts");
        assert_eq!(thermal.update(&[dimm(70.0), unknown.clone()]), vec![super::HotTemp { sensor: "spd5118:temp1".into(), c: 70.0, limit: 70.0 }], "the limit itself goes hot");
        assert_eq!(thermal.update(&[dimm(67.0), unknown.clone()])[0].c, 67.0, "three degrees below stays hot and updates the reading");
        assert_eq!(thermal.update(&[dimm(65.0)]).len(), 1, "exactly five degrees below stays hot");
        assert_eq!(thermal.update(&[]).len(), 1, "a missing reading cannot cool a hot sensor");
        assert!(thermal.update(&[dimm(64.999), unknown]).is_empty(), "below limit minus five cools");
        assert_eq!(thermal.update(&[dimm(71.0), dimm(40.0)])[0].c, 71.0, "a cool DIMM sharing the name cannot hide a hot one");
        assert_eq!(thermal.update(&[dimm(40.0), dimm(67.0)])[0].c, 67.0, "duplicate names retain hysteresis independent of order");
        assert!(thermal.update(&[dimm(40.0), dimm(64.0)]).is_empty(), "every DIMM sharing the name has cooled");
    }

    /// The JIT store's keys take the commit and the box's report the release from `rustc -vV`;
    /// one that says neither leaves both empty (no key: the store builds nothing under a guess).
    #[test]
    fn rustc_vv_gives_release_and_commit() {
        let r = super::parse_rustc("rustc 1.100.0 (0ef1bc386 2026-11-26)\nbinary: rustc\ncommit-hash: 0ef1bc386059596dada8fb65b50425aab2ea637a\ncommit-date: 2026-11-26\nhost: x86_64-unknown-linux-gnu\nrelease: 1.100.0\nLLVM version: 21.1.0\n");
        assert_eq!((r.release.as_str(), r.commit.as_str()), ("1.100.0", "0ef1bc386059596dada8fb65b50425aab2ea637a"));
        let none = super::parse_rustc("error: no such command\n");
        assert!(none.release.is_empty() && none.commit.is_empty());
    }

    /// A service runs where its state file's pid is alive: a file left by one that ended, or one
    /// with no pid, is no service of the node, and the update roll asks it to restart none.
    #[test]
    fn a_service_whose_state_outlived_it_is_none() {
        let dir = std::env::temp_dir().join(format!("devhub-caps-services-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("devhub.state"), "pid = 11\nidentity = hub\n").unwrap();
        std::fs::write(dir.join("ai-hub.state"), "pid = 12\nidentity = aihub\n").unwrap();
        std::fs::write(dir.join("desk-link.state"), "identity = no pid\n").unwrap();
        std::fs::write(dir.join("ai-hub.log"), "pid = 11\n").unwrap();
        let names = |alive: &dyn Fn(u32) -> bool| super::services_in(&dir, alive).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(names(&|p| p == 11 || p == 12), ["ai-hub", "devhub"]);
        assert_eq!(names(&|p| p == 11), ["devhub"], "the AI hub ended: its state file is no service");
        assert_eq!(super::state_pid("identity = x\npid = 42\n"), Some(42));
        assert_eq!(super::state_pid("pid = x\n"), None);
        // a service its node starts from a kept binary: one the prebuild replaced after the start
        // (the copy's time after its state file's) runs an older build than its node keeps
        let kept = dir.join("kept");
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(kept.join("aihub"), "build").unwrap();
        std::fs::write(dir.join("ai-hub.state"), format!("pid = 12\nidentity = {}\n", kept.join("aihub").display())).unwrap();
        std::fs::write(dir.join("other.state"), format!("pid = 12\nidentity = {} 0 0 nice=0 features=node env=[]\n", dir.join("aihub").display())).unwrap();
        let at = |p: &std::path::Path, secs: u64| std::fs::File::options().write(true).open(p).unwrap().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)).unwrap();
        at(&kept.join("aihub"), 1000);
        at(&dir.join("ai-hub.state"), 2000);
        assert!(super::kept_replaced(&dir, &kept, &|_| true).is_empty(), "started on its kept copy: current");
        at(&kept.join("aihub"), 3000);
        assert_eq!(super::kept_replaced(&dir, &kept, &|_| true), ["ai-hub"], "the prebuild replaced the copy after the start: the roll restarts it");
        assert!(super::kept_replaced(&dir, &kept, &|p| p != 12).is_empty(), "an ended service is none");
        at(&dir.join("ai-hub.state"), 4000);
        assert!(super::kept_replaced(&dir, &kept, &|_| true).is_empty(), "its restart wrote its state after the copy: current again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only a git error that is the box's takes it out of rotation: one about the request (a
    /// commit it does not have) is the request's.
    #[test]
    fn a_box_git_error_is_told_from_a_request_error() {
        assert!(super::box_git_error("git bundle: fatal: Unable to read current working directory"));
        assert!(super::box_git_error("git: No such file or directory (os error 2)"));
        assert!(super::box_git_error("git fetch: error: unable to write file: No space left on device"));
        assert!(!super::box_git_error("git rev-parse: fatal: bad revision 'deadbeef'"));
        assert!(!super::box_git_error("git read-tree: error: Entry 'x' not uptodate. Cannot merge."));
    }

    /// macOS measures CPU use like Linux (two samples of all cores' ticks): the busy check uses it.
    /// A Mac reports the GPU it draws on.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_measures_cpu_use_and_memory_pressure() {
        // the kernel publishes the counters about once a second, and a measure within the same
        // period answers the last value: none yet, so a second measure 300 ms on failed (11-17)
        let _ = os::hardware();
        let t0 = std::time::Instant::now();
        let c = loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let c = os::hardware();
            if c.cpu_pct.is_some() || t0.elapsed() > std::time::Duration::from_secs(3) { break c; }
        };
        let p = c.cpu_pct.expect("a measure once the counters moved has the CPU use");
        assert!((0.0..=100.0).contains(&p), "{p}");
        assert!(matches!(c.mem_pressure, 1 | 2 | 4), "{}", c.mem_pressure);
        // every Mac draws on a GPU no lease reserves: without it, a `--needs gpu=1` ask never met a
        // Mac (L1616264, 11-23)
        assert!(!c.shared_gpu.is_empty() && c.gpus.is_empty(), "{:?} {:?}", c.shared_gpu, c.gpus);
    }
    use super::*;

    /// A hub asks the node's own WM, not a lane's test desktop beside it, and the newest of two.
    #[test]
    fn the_dark_check_asks_the_nodes_own_wm() {
        let app = |pid: u32, name: &str, started_ms: u64, lease: Option<&str>| makepad_remote::discovery::App {
            pid,
            app: name.to_string(),
            socket: format!("/s/{pid}.sock"),
            started_ms,
            caps: Vec::new(),
            restart_parent: None,
            fields: lease.map(|l| ("lease".to_string(), format!("\"{l}\""))).into_iter().collect(),
        };
        let apps = [app(1, "makepad-app-wm", 10, None), app(2, "makepad-app-wm", 30, None), app(3, "makepad-app-wm", 40, Some("L7")), app(4, "makepad-fab", 50, None)];
        assert_eq!(node_wm(&apps).map(|a| a.pid), Some(2));
        assert_eq!(node_wm(&apps[2..]).map(|a| a.pid), None, "only a lease's test desktop runs here");
    }

    /// A WM whose one window is 0x0 shows nothing; one with a size, or an answer without
    /// windows, says nothing against the box (the status of a node whose WM had no output, 11-07).
    #[test]
    fn a_wm_window_of_0x0_is_a_dark_display() {
        let dark = r#"{"app":"makepad-app-wm","pid":7,"w":[{"i":0,"t":"makepad-wm","sz":[0,0],"px":[0,0],"dpi":2.43,"pos":[0,0]}]}"#;
        let lit = r#"{"app":"makepad-app-wm","pid":7,"w":[{"i":0,"t":"makepad-wm","sz":[1920,1080],"px":[3840,2160],"dpi":2,"pos":[0,0]}]}"#;
        let s = |v: &[&str]| v.iter().enumerate().map(|(i, c)| (format!("card0-DP-{i}"), c.to_string())).collect::<Vec<_>>();
        let none = "no output connected (its 2 DRM connectors read disconnected; its WM's window is 0x0, and lights one as it is plugged in or switched on)";
        assert_eq!(dark_of(dark, &[]), "its WM's window is 0x0: no output shows it");
        assert_eq!(dark_of(dark, &s(&["connected", "disconnected"])), "its WM's window is 0x0: no output shows it", "a connected output: the WM's to look at");
        assert_eq!(dark_of(dark, &s(&["disconnected", "disconnected"])), none);
        // a writeback connector is the kernel's capture sink, no screen: a box with every screen
        // connector disconnected and one of those `connected` has no output (a pool box, 11-25)
        let mut writeback = s(&["disconnected", "disconnected"]);
        writeback.push(("card1-Writeback-1".into(), "connected".into()));
        assert_eq!(dark_of(dark, &writeback), none);
        assert_eq!(dark_of(dark, &writeback[2..]), "its WM's window is 0x0: no output shows it", "a writeback connector alone says nothing of the outputs");
        assert_eq!(dark_of(lit, &s(&["disconnected"])), "", "a window with a size shows, whatever the connectors say");
        assert_eq!(dark_of(r#"{"app":"makepad-app-wm","pid":7,"w":[]}"#, &[]), "");
        assert_eq!(dark_of("not json", &[]), "");
    }

    #[test]
    fn cpuinfo_meminfo_loadavg_nvidia_battery() {
        let cpu = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: CPU-B 8-Core Processor\n\nprocessor\t: 1\nmodel name\t: CPU-B 8-Core Processor\n";
        assert_eq!(parse_cpuinfo(cpu), ("CPU-B 8-Core Processor".to_string(), 2));
        // a build for a box takes the CPU level it has all of, never more
        assert_eq!([x86_level(true, true, true), x86_level(true, true, false), x86_level(true, false, true), x86_level(false, true, true)], ["x86-64-v4", "x86-64-v3", "x86-64-v2", "x86-64"]);
        assert_eq!([apple_target("Apple M0 Tier"), apple_target("Apple M00"), apple_target("Apple Mx"), apple_target("CPU-B 8-Core Processor")], [Some("apple-m0".to_string()), Some("apple-m00".to_string()), None, None]);
        let here = cpu_target("CPU-B 8-Core Processor");
        assert!(!here.is_empty() && here.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{here}");
        let (t, a) = parse_meminfo("MemTotal:       65536000 kB\nMemFree:  100 kB\nMemAvailable:   32768000 kB\n");
        assert!((t - 62.5).abs() < 0.01 && (a - 31.25).abs() < 0.01);
        assert_eq!(parse_loadavg("3.52 2.10 1.00 2/900 1234\n"), 3.52);
        // the run delay is each cpu line's eighth count; the domain lines under it are not CPUs
        let sched = "version 15\ntimestamp 4295\ncpu0 0 0 9 4 7 3 5000 2000000000 40\ndomain0 00000003 1 2 3\ncpu1 0 0 9 4 7 3 6000 1000000000 41\n";
        assert_eq!(parse_schedstat_wait(sched), Some(3_000_000_000));
        assert_eq!(parse_schedstat_wait("version 15\n"), None, "a kernel without its CPUs' lines");
        assert_eq!(cpu_wait(3_000_000_000, 63_000_000_000, 10.0), Some(6.0), "60 s waited over 10 s: six tasks waiting on average");
        assert_eq!(cpu_wait(5, 4, 10.0), None, "a sum that went back (the counters reset)");
        let psi = "some avg10=31.20 avg60=4.00 avg300=1.00 total=99\nfull avg10=14.20 avg60=3.10 avg300=0.80 total=88\n";
        assert_eq!(parse_psi_full(psi), 14.2, "the full line's avg10, not the some line's");
        assert_eq!(parse_psi_full(""), 0.0, "a kernel without PSI");
        assert!(memory_stall(14.2).is_some_and(|w| w.starts_with("memory stall 14% ")) && memory_stall(9.9).is_none());
        let g = parse_nvidia("NVIDIA GeForce GPU-B, 33000\nNVIDIA GeForce GPU-A, 25000\n");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "NVIDIA GeForce GPU-B");
        assert!((g[1].1 - 24.41).abs() < 0.01);
        assert!(parse_nvidia("").is_empty());
        assert_eq!(parse_battery("87\n", "Discharging\n"), Some((87, false)));
        assert_eq!(parse_battery("100", "Full"), Some((100, true)));
        assert_eq!(parse_battery("", "Charging"), None);
        assert!(system_battery("Battery\n", ""));
        assert!(system_battery("Battery\n", "System\n"));
        assert!(!system_battery("Battery\n", "Device\n"));
        assert!(!system_battery("Mains\n", ""));
    }

    #[test]
    fn lease_conf() {
        let home = Path::new("/home/me");
        let d = LeaseConf::parse("", home);
        assert_eq!((d.role, d.disk_floor_gb, d.timed), (Role::Pool, 25.0, true));
        assert_eq!(d.work, home.join(WORK_DIR));
        assert_eq!(d.slots(2), vec![(Kind::Build, 1), (Kind::Test, 1), (Kind::Screen, 1), (Kind::Long, 1), (Kind::Gate, 1), (Kind::Gpu, 1), (Kind::Timed, 1), (Kind::Ci, 1)]);
        let ci = |conf: &str| LeaseConf::parse(conf, home).slots(0).into_iter().find(|(k, _)| *k == Kind::Ci).map(|(_, n)| n);
        assert_eq!((ci("slots.ci = 0\n"), ci("role = user-wm\nslots.ci = 1\n"), ci("role = bench\n")), (Some(0), Some(0), Some(1)), "a box opts out of CI, a user's box never takes it, any other takes it by default");
        let c = LeaseConf::parse("# a node's conf\nrole = user-wm\nslots.build = 1\nslots.nope = 4\nslots.test=6\ndisk_floor_gb = 40\ntimed = off\nspec = CPU-A+GPU-A  # comment\nwork = ~/scratch/dh\ncpus.shared = 8-15,24-31\ncpus.timed = rm -rf\n", home);
        assert_eq!(c.role, Role::UserWm);
        assert_eq!(c.disk_floor_gb, 40.0);
        assert!(!c.timed);
        assert_eq!(c.spec, "CPU-A+GPU-A");
        assert_eq!(c.work, PathBuf::from("/home/me/scratch/dh"));
        assert_eq!(c.cpus_shared, "8-15,24-31");
        assert_eq!(c.cpus_timed, "");
        assert_eq!((cpu_count(&c.cpus_shared), cpu_count("3"), cpu_count("0-3,8"), cpu_count(""), cpu_count("5-2"), cpu_count("1,,2")), (16, 1, 5, 0, 0, 0));
        let s = c.slots(0);
        assert!(s.contains(&(Kind::Build, 1)) && s.contains(&(Kind::Test, 6)) && s.contains(&(Kind::Timed, 0)) && s.contains(&(Kind::Gpu, 0)));
    }

    #[test]
    fn master_sets_role_and_budgets() {
        assert!(settable("role", "user-wm").is_ok() && settable("role", "boss").is_err());
        assert!(settable("slots.build", "3").is_ok() && settable("slots.build", "x").is_err() && settable("slots.nope", "1").is_err());
        assert!(settable("work", "/tmp").is_err() && settable("cpus.shared", "rm -rf").is_err());
        assert!(settable("timed", "spare").is_ok() && settable("timed", "maybe").is_err());
        let spare = LeaseConf::parse("timed = spare\n", Path::new("/h"));
        assert_eq!((spare.timed, spare.timed_spare, LeaseConf::parse("timed = on\n", Path::new("/h")).timed_spare), (true, true, false));
        let t = conf_with("# a node\nrole = pool  # was\nslots.build = 2\n", &[("role".into(), "user-wm".into()), ("cpus.shared".into(), "8-15,24-31".into())]);
        assert_eq!(t, "# a node\nrole = user-wm  # was\nslots.build = 2\ncpus.shared = 8-15,24-31\n");
        let c = LeaseConf::parse(&t, Path::new("/h"));
        assert_eq!((c.role, c.cpus_shared.as_str()), (Role::UserWm, "8-15,24-31"));
        assert!(settable("slots.ci", "").is_ok() && settable("cpus.timed", "").is_ok() && settable("nope", "").is_err(), "an empty value unsets a key the master sets, and only such a key");
        let unset = conf_with("# a node\nslots.ci = 1  # opted in\nrole = pool\n", &[("slots.ci".into(), String::new()), ("timed".into(), String::new())]);
        assert_eq!(unset, "# a node\nrole = pool\n", "an empty value removes the key's line, and adds none for a key the conf did not have");
    }

    #[test]
    fn measures_this_machine() {
        let c = measure("t", &LeaseConf::parse("", &std::env::temp_dir()), "checkout");
        assert!(c.cores >= 1 && c.ram_gb > 0.0 && c.disk_total_gb > 0.0, "{c:?}");
        assert_eq!(Caps::parse(&c.to_json()).map(|p| p.cores), Some(c.cores));
    }
}
