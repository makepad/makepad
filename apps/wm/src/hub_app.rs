//! This node's AI hub: the AI Hub app (apps/aihub), started by the window manager as an
//! ordinary process at session start, the same app a Mac or Windows desktop runs, with its tray
//! icon in the WM's tray area (`trays`) and its panel as the tray's dropdown (a flyout under the
//! bar item, `dropdown`). The WM keeps it running:
//!
//! - it starts through `cargo run --release -p makepad-app-aihub --features <features>` in the WM's
//!   checkout (the registry's `run_argv_features`, the cargo environment of every launcher), with
//!   the node's settings (`~/.makepad/wm/ai-hub.conf`, [`NodeConf`]): the app reads the same
//!   file (`MAKEPAD_AI_HUB_NODE_CONF`) for where it serves (port, this node's LAN address, cache)
//!   and which chat model it loads at start; the WM hands it the environment the file gives (the
//!   chat model and pins, context, prefetch, the CUDA toolkit for the kernel build). The rule for
//!   every service the WM starts: `cargo run` with the deploy's exact build inputs (package,
//!   features, release profile, the cargo target and CUDA environment of the WM session), so a
//!   start after the deploy's build (`makepad-cargo build --features node aihub`) compiles
//!   nothing and a start after a source change builds first: never a stale binary;
//! - an app that exits (a crash, a kill) is started again, after a pause that doubles with each
//!   exit in a row (2 s .. 5 min) and starts over once it stayed up for two minutes. Its tray
//!   menu's Quit is deliberate: the app writes `enabled = off` into the settings, and the WM
//!   leaves it off (also across WM restarts) until the person starts it from the launcher
//!   (`start_from_launcher`, which writes `enabled = on`);
//! - a WM restart does not cost it (Unix). It runs outside the WM's life: its output goes to its
//!   log file, on Linux under systemd in a scope of the user's service manager (the WM's session
//!   ends with the WM), and `~/.makepad/wm/ai-hub.state` names its pid and what it was
//!   started as. That manager must run: a dead one (a global OOM killed it, 11-03) is detected
//!   before the start, and the WM restarts its own session, whose login scope starts it again
//!   (the only start of it open to the user), once no app holds unsaved work ([`Manager`],
//!   [`session_step`]); a start that waits says why. Its windows and its tray wait for the next
//!   WM and come back on it (the HAL's WM shell re-attaches a tray app). The next WM keeps a live app with the same start (its
//!   models stay loaded, its jobs keep running); one started from another binary or other
//!   settings is replaced;
//! - a stop is SIGTERM: the app's hub closes admission (the fleet routes jobs elsewhere), hands
//!   its running jobs back to their clients and quits. Windows: it ends with the WM.
//!
//! The same supervisor keeps the node's **devhub** (apps/devhub: the JIT observer the user's Mac
//! master reaches): a [`Supervised`] service is one [`Kind`] (its app, files, log label and the
//! env var its conf path goes in) and one [`ServiceConf`] (what its settings file says).
//! [`HubApp`] is the AI Hub's, [`DevhubApp`] the devhub's. While no devhub runs on a box nothing
//! there reaches the master, so each failed start or exit of the devhub goes into its trouble
//! record (`makepad_devhub_conf::Trouble`), which the devhub reports to the master once it runs.
//!
//! A WM that is not its node's ([`guest_wm`]: hidden, under a lease, or second on the session)
//! starts no hub and leaves the node's alone, unless `MAKEPAD_WM_AI_HUB_CONF` names its settings:
//! test desktops must not take the GPU or the port of the real one. The hub's output goes to
//! `~/.makepad/wm/ai-hub.log`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use makepad_ai_hub::node_conf::{NodeConf, CONF_ENV, NODE_CONF_ENV};
use makepad_app_registry as registry;

/// A service the WM supervises: its registry app, the stem of its files under `~/.makepad/wm`
/// (`<file>.log`, `<file>.state`; its scopes are `makepad-<file>-<port>-<wm pid>`), its name in
/// the log, and the env var its settings file's path goes in.
pub struct Kind {
    pub app_id: &'static str,
    pub file: &'static str,
    pub label: &'static str,
    pub conf_env: &'static str,
    /// Each failed start or exit is recorded beside its settings
    /// (`makepad_devhub_conf::trouble_path`) for the service to report once it runs. A clean exit
    /// (devhub's self-update) is none, nor one whose status this WM never sees (a hub an earlier
    /// WM started).
    pub trouble: bool,
}

pub const AI_HUB: Kind = Kind { app_id: crate::clients::HUB_SERVICE_APP, file: "ai-hub", label: "AI Hub", conf_env: NODE_CONF_ENV, trouble: false };
pub const DEVHUB: Kind = Kind { app_id: crate::clients::DEVHUB_SERVICE_APP, file: "devhub", label: "devhub", conf_env: makepad_devhub_conf::NODE_CONF_ENV, trouble: true };

/// What the supervisor needs from a service's settings file.
pub trait ServiceConf: Clone {
    /// The file as it is now (`prev` when it cannot be read).
    fn reload(path: &Path, prev: &Self) -> Self;
    fn enabled(&self) -> bool;
    fn features(&self) -> &str;
    fn nice(&self) -> i32;
    fn port(&self) -> u16;
    /// The environment the service starts with.
    fn env(&self) -> Vec<(String, String)>;
    /// Everything else that is part of what it was started as (a change replaces it).
    fn identity(&self) -> String;
    /// For the start's log line.
    fn note(&self) -> String;
}

impl ServiceConf for NodeConf {
    fn reload(path: &Path, prev: &Self) -> Self {
        NodeConf::load_or(path, prev)
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn features(&self) -> &str {
        &self.features
    }
    fn nice(&self) -> i32 {
        self.nice
    }
    fn port(&self) -> u16 {
        self.port
    }
    fn env(&self) -> Vec<(String, String)> {
        self.hub_env()
    }
    fn identity(&self) -> String {
        format!("nice={} features={} env={:?}", self.nice, self.features, self.hub_env())
    }
    fn note(&self) -> String {
        if self.model.is_empty() { String::new() } else { format!(", model {}", self.model) }
    }
}

impl ServiceConf for makepad_devhub_conf::NodeConf {
    // Never niced: nothing lowers a process's level again, and a timed lease runs at the level a
    // person's app gets. The desktop keeps its CPU through the cgroups: the WM's session is a sibling
    // of the user manager the hub and its leases run under, and nice only orders threads inside one
    // scope.
    fn reload(path: &Path, prev: &Self) -> Self {
        if path.exists() { Self::load(path) } else { prev.clone() }
    }
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn features(&self) -> &str {
        ""
    }
    fn nice(&self) -> i32 {
        0
    }
    fn port(&self) -> u16 {
        self.port
    }
    fn env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn identity(&self) -> String {
        format!("name={} port={}", self.name, self.port)
    }
    fn note(&self) -> String {
        format!(", node {}", self.name)
    }
}

/// The node's devhub settings (`~/.makepad/wm/devhub.conf`, `MAKEPAD_WM_DEVHUB_CONF` names
/// another); a hidden WM without its own file starts none.
pub fn load_devhub_conf() -> (PathBuf, makepad_devhub_conf::NodeConf) {
    let path = std::env::var_os("MAKEPAD_WM_DEVHUB_CONF").filter(|p| !p.is_empty()).map(PathBuf::from).unwrap_or_else(|| crate::theme::makepad_home().join("wm/devhub.conf"));
    let mut conf = makepad_devhub_conf::NodeConf::load(&path);
    if stays_out("MAKEPAD_WM_DEVHUB_CONF") {
        conf.enabled = false;
    }
    (path, conf)
}

/// The node's settings as the WM reads them: the file, or the defaults; a hidden WM without its
/// own settings file starts no hub.
pub fn load_conf() -> (PathBuf, NodeConf) {
    let path = NodeConf::path();
    let mut conf = NodeConf::load_from(&path);
    if stays_out(CONF_ENV) {
        conf.enabled = false;
    }
    (path, conf)
}

/// A WM started as a test desktop: hidden (tests, agents) or under a lease (a lane's own). It
/// never takes the socket path apps find by default.
pub fn test_wm() -> bool {
    std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() || std::env::var_os(makepad_remote::discovery::LEASE_ENV).is_some()
}

/// A WM that is not its node's: a test desktop, or a second one on this session (a live WM holds
/// the default path, so it got another). It keeps none of the node's services and leaves them
/// alone: two that keep them stop each other's hubs as rivals, under the node's leases (11-05).
/// The update's WM comes after the old one is gone, so it holds the default path and adopts them.
pub fn guest_wm() -> bool {
    #[cfg(target_os = "linux")]
    {
        use makepad_widgets::makepad_platform::makepad_hal::wm_socket;
        let exported = std::env::var_os(wm_socket::SOCKET_ENV).filter(|p| !p.is_empty()).map(PathBuf::from);
        test_wm() || second_on_session(exported.as_deref(), wm_socket::runtime_dir().as_deref())
    }
    #[cfg(not(target_os = "linux"))]
    {
        test_wm()
    }
}

/// Whether the path this WM published (`MAKEPAD_WM_SOCKET`) is not the default one in `dir`.
#[cfg(target_os = "linux")]
fn second_on_session(exported: Option<&Path>, dir: Option<&Path>) -> bool {
    use makepad_widgets::makepad_platform::makepad_hal::wm_socket;
    match (exported, dir) {
        (Some(path), Some(dir)) => path != wm_socket::socket_path(dir, 0),
        _ => false,
    }
}

/// A guest WM stays out of the service `conf_env` would name settings for, unless it does.
pub fn stays_out(conf_env: &str) -> bool {
    guest_wm() && std::env::var_os(conf_env).is_none()
}

/// Seconds before the `n`th restart in a row (n >= 1).
pub fn backoff_s(n: u32) -> u32 {
    (1u32 << n.min(9)).min(300)
}

/// An app that stayed up this long starts the crash count over.
const STEADY_S: f64 = 120.0;
const TAIL: usize = 24;
/// How long the first start waits for the WM's socket (the app finds the WM by it): the
/// adoption socket is published at the WM's start, before its first second.
const SOCKET_WAIT_S: f64 = 10.0;

/// The app this WM watches: its own child, or one an earlier WM started and this one kept
/// (`ai-hub.state`).
enum Running {
    Child(Child),
    Adopted(u32),
}

impl Running {
    fn pid(&self) -> u32 {
        match self {
            Running::Child(child) => child.id(),
            Running::Adopted(pid) => *pid,
        }
    }
}

/// The hub exits at once again and again: a tree a lease left at a commit it does not build at (from
/// before its package) would keep it down for good. The tree goes to the shared commits (a thread:
/// git on the WM's tick would stall it), and the hub, once up, puts a lease still holding it back. A
/// tree that is at them stays: a hub that fails there is a landed bug, and its log says which.
fn tree_to_synced() {
    let Some(root) = registry::repo_root() else { return };
    let spawned = std::thread::Builder::new().name("wm-devhub-tree".into()).spawn(move || {
        for dir in [root.clone(), root.join("apps").join("commercial")] {
            let git = |args: &[&str]| Command::new("git").args(args).current_dir(&dir).stdin(std::process::Stdio::null()).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
            if !dir.join(".git").exists() {
                continue;
            }
            let Some(to) = git(&["rev-parse", "-q", "--verify", "refs/devhub/synced"]) else { continue };
            if git(&["rev-parse", "-q", "--verify", "HEAD"]).as_deref() == Some(to.as_str()) {
                continue;
            }
            makepad_widgets::log!("wm: devhub does not start in {}: back at the shared {to}", dir.display());
            let moved = git(&["read-tree", "--reset", "-u", &to]).is_some() && git(&["update-ref", "--no-deref", "HEAD", &to]).is_some() && git(&["update-ref", "refs/devhub/checkout", &to]).is_some();
            if !moved {
                makepad_widgets::log!("wm: devhub: moving {} to {to} failed", dir.display());
            }
        }
    });
    if let Err(e) = spawned {
        makepad_widgets::log!("wm: devhub: {e}");
    }
}

/// What the app was started as: the binary cargo runs (path, size, modification time) and
/// everything the settings hand it. A WM that finds a live app with the same identity keeps it.
fn identity<C: ServiceConf>(binary: &Path, conf: &C) -> String {
    let meta = std::fs::metadata(binary).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let modified = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{} {size} {modified} {}", binary.display(), conf.identity())
}

/// The settings part of a recorded identity of `binary` ([`identity`]).
fn recorded_settings<'a>(id: &'a str, binary: &Path) -> Option<&'a str> {
    id.strip_prefix(&format!("{} ", binary.display()))?.splitn(3, ' ').nth(2)
}

/// The registry's app of a service.
fn target_of(kind: &Kind) -> Result<registry::AppTarget<'static>, String> {
    registry::find(kind.app_id).map(|entry| entry.target()).ok_or_else(|| format!("the registry has no {}", kind.app_id))
}

#[cfg(test)]
fn hub_target() -> Result<registry::AppTarget<'static>, String> {
    target_of(&AI_HUB)
}

/// The app's cargo command: the WM's start, or the deploy's build of exactly that.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verb {
    Run,
    Build,
}

/// The deploy's build of this node's AI Hub (`makepad-wm-build -- hub`) or devhub
/// (`-- devhub`), run in the WM session's environment in the checkout the WM runs from: `cargo
/// build` of the very command the WM's start runs, from the same settings (`ai-hub.conf` /
/// `devhub.conf`: features, the `env` lines, the CUDA toolkit installed here) and launcher
/// environment, so the WM's `cargo run` of it after the deploy compiles nothing. One source for
/// both: a feature or toolkit default changes the build and the start together. Prints the
/// command (variable names only: an `env` line may carry a token) and returns cargo's status.
pub fn build_node_hub() -> std::process::ExitCode {
    let path = NodeConf::path();
    let conf = NodeConf::load_from(&path);
    build_service(HubApp::new(&AI_HUB, path, conf))
}

/// `makepad-wm-build -- devhub`: the deploy's build of the node's devhub ([`build_node_hub`]).
pub fn build_node_devhub() -> std::process::ExitCode {
    let (path, conf) = load_devhub_conf();
    build_service(DevhubApp::new(&DEVHUB, path, conf))
}

fn build_service<C: ServiceConf>(app: Supervised<C>) -> std::process::ExitCode {
    let word = app.kind.file;
    let mut cmd = match app.cargo(Verb::Build, false) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("makepad-wm-build: {word}: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    let set: Vec<String> = cmd.get_envs().filter(|(_, v)| v.is_some()).map(|(k, _)| k.to_string_lossy().into_owned()).collect();
    eprintln!(
        "makepad-wm-build: {word}: {} {} (features {:?}; sets {})",
        cmd.get_program().to_string_lossy(),
        cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
        app.conf.features(),
        set.join(" ")
    );
    crate::host::no_console_window(&mut cmd);
    match cmd.status() {
        Ok(status) if status.success() => std::process::ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("makepad-wm-build: {word}: build failed: {status}");
            std::process::ExitCode::from(status.code().unwrap_or(1).clamp(1, 255) as u8)
        }
        Err(e) => {
            eprintln!("makepad-wm-build: {word}: cannot start cargo: {e}");
            std::process::ExitCode::from(1)
        }
    }
}

/// `ai-hub.state`: the pid of the app this WM (or an earlier one) started and its identity.
fn read_state(path: &Path) -> Option<(u32, String)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut pid = None;
    let mut id = None;
    for line in text.lines() {
        match line.split_once(" = ") {
            Some(("pid", v)) => pid = v.trim().parse().ok(),
            Some(("identity", v)) => id = Some(v.to_string()),
            _ => {}
        }
    }
    Some((pid?, id?))
}

/// Whether `pid` is still the AI Hub app: alive, and (Linux) its command line names the app
/// (`cargo run ... --bin makepad-app-aihub` while cargo checks the build, then its binary), so a
/// reused pid is never taken for it.
fn app_alive(kind: &Kind, pid: u32) -> bool {
    if !makepad_app_host::launch::process_alive(pid) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let name = registry::find(kind.app_id).map_or(kind.app_id, |e| e.bin);
        return std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmd| cmd.windows(name.len()).any(|w| w == name.as_bytes()));
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = kind;
        true
    }
}

/// Linux under systemd: the WM's session ends with the WM, so an app that is to outlive a WM
/// restart runs in a scope of the user's service manager (`systemd-run --user --scope`, which
/// execs it: the child's pid is the app's).
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";

/// The user's service manager (`user@<uid>.service`) as pid1 reports it. A service starts in a
/// scope of it, so it must run: on 11-03 a global OOM killed it, its bus socket stayed behind,
/// and every start through `systemd-run --user` failed ("Connection refused") for 24 min until a
/// login on the box started it again.
#[cfg(target_os = "linux")]
#[derive(Clone, Debug, PartialEq)]
pub enum Manager {
    /// It runs: the service starts in a scope of it.
    Up,
    /// pid1 is starting or stopping it (`activating`, `deactivating`): the start waits.
    Changing(String),
    /// It is gone (`failed`, `inactive`). `down` names this outage: when it went down (its
    /// unit's `InactiveEnterTimestampMonotonic`).
    Down { state: String, down: String },
    /// systemctl did not say.
    Unknown(String),
}

#[cfg(target_os = "linux")]
impl Manager {
    /// From `systemctl show --property=ActiveState,InactiveEnterTimestampMonotonic`.
    fn parse(text: &str) -> Manager {
        let get = |key: &str| text.lines().find_map(|l| l.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v.trim().to_string())).unwrap_or_default();
        match get("ActiveState").as_str() {
            "active" | "reloading" | "refreshing" => Manager::Up,
            "activating" | "deactivating" => Manager::Changing(get("ActiveState")),
            "" => Manager::Unknown(format!("systemctl gave no ActiveState: {:?}", text.trim())),
            state => Manager::Down { state: state.to_string(), down: get("InactiveEnterTimestampMonotonic") },
        }
    }
}

/// This user's service manager unit.
#[cfg(target_os = "linux")]
fn manager_unit() -> String {
    use std::os::unix::fs::MetadataExt;
    format!("user@{}.service", std::fs::metadata("/proc/self").map_or(0, |m| m.uid()))
}

/// This user's service manager, from pid1. None: this Linux runs services without one (no
/// `systemd-run`, or no user session: no runtime dir or no user bus socket in it), so they start
/// as the WM's children.
#[cfg(target_os = "linux")]
fn user_manager() -> Option<Manager> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty())?;
    if !Path::new(SYSTEMD_RUN).is_file() || !Path::new(&runtime).join("bus").exists() {
        return None;
    }
    let unit = manager_unit();
    Some(match Command::new("systemctl").args(["show", &unit, "--property=ActiveState,InactiveEnterTimestampMonotonic"]).stdin(Stdio::null()).output() {
        Ok(out) if out.status.success() => Manager::parse(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => Manager::Unknown(format!("systemctl show {unit}: {} {}", out.status, String::from_utf8_lossy(&out.stderr).trim())),
        Err(e) => Manager::Unknown(format!("systemctl: {e}")),
    })
}

/// Whether this WM can restart its own session: it runs as its systemd unit (which restarts it
/// on exit 75 and opens a new login session for it), not hidden (a test desktop).
#[cfg(target_os = "linux")]
fn session_restartable() -> bool {
    std::env::var_os("INVOCATION_ID").is_some() && std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_none()
}

/// The file in `~/.makepad/wm` that names the outage the WM restarted its session for (its first
/// line: [`Manager::Down`]'s `down`): a session restart that did not bring the manager back is
/// not repeated.
#[cfg(target_os = "linux")]
const SESSION_RESTART_FILE: &str = "session-restart";

/// A restart of the WM's session a service needs: why, and the outage it is for.
#[cfg(target_os = "linux")]
#[derive(Clone, Debug, PartialEq)]
pub struct SessionRestart {
    pub why: String,
    pub down: String,
}

/// What the WM does about a session restart its services need, given the apps' unsaved work
/// (`makepad_app_registry::machine::unsaved_work`): a restart ends every app of the session.
#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq)]
pub enum SessionStep {
    /// Restart now (exit 75, which the WM's unit restarts): why.
    Restart(String),
    /// Wait while an app holds unsaved work: what the person at the screen is told.
    Waits(String),
}

#[cfg(target_os = "linux")]
pub fn session_step(want: &SessionRestart, unsaved: &[(u32, String, String)]) -> SessionStep {
    if unsaved.is_empty() {
        return SessionStep::Restart(want.why.clone());
    }
    let list = unsaved.iter().map(|(_, app, what)| format!("{app}: {what}")).collect::<Vec<_>>().join("; ");
    SessionStep::Waits(format!("{}, once no app holds unsaved work ({list})", want.why))
}

/// The WM restarts its session for `want` now: recorded in `dir` (`~/.makepad/wm`) first, so the
/// next WM does not restart again for the same outage.
#[cfg(target_os = "linux")]
pub fn begin_session_restart(dir: &Path, want: &SessionRestart) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(SESSION_RESTART_FILE), format!("{}\n{}\n", want.down, want.why))
}

/// The `makepad-ai-hub-<port>-<wm pid>.scope` a process runs in (its `/proc/<pid>/cgroup`
/// text), for `port`: where this node's WMs start the AI Hub.
fn hub_scope<'a>(cgroup: &'a str, file: &str, port: u16) -> Option<&'a str> {
    let prefix = format!("makepad-{file}-{port}-");
    cgroup.lines().filter_map(|line| line.rsplit('/').next()).find(|unit| {
        unit.strip_prefix(prefix.as_str()).and_then(|rest| rest.strip_suffix(".scope")).is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// When `pid` started (clock ticks since boot).
#[cfg(target_os = "linux")]
fn proc_start(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat[stat.rfind(')')? + 2..].split_whitespace().nth(19)?.parse().ok()
}

/// Whether the WM a hub's scope names is another live one, which keeps it. A pid reused since then
/// started after the process in that scope, so it is not that WM.
#[cfg(target_os = "linux")]
fn kept_by_another_wm(scope: &str, pid: u32, me: u32, start: &dyn Fn(u32) -> Option<u64>) -> bool {
    let Some(wm) = scope.strip_suffix(".scope").and_then(|s| s.rsplit('-').next()).and_then(|p| p.parse::<u32>().ok()) else { return false };
    wm != me && matches!((start(wm), start(pid)), (Some(w), Some(p)) if w <= p)
}

/// This user's processes in a hub scope for `port` other than `keep`'s (its own children, the
/// sandboxed Claude, share its scope) and no other live WM's.
#[cfg(target_os = "linux")]
fn rival_hubs(file: &str, port: u16, keep: Option<u32>) -> Vec<u32> {
    use std::os::unix::fs::MetadataExt;
    let scope_of = |pid: u32| std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok().and_then(|c| hub_scope(&c, file, port).map(str::to_string));
    let Ok(me) = std::fs::metadata("/proc/self").map(|m| m.uid()) else { return Vec::new() };
    let kept = keep.and_then(scope_of);
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    let mut out: Vec<u32> = dir
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .filter(|pid| Some(*pid) != keep && *pid != std::process::id())
        .filter(|pid| std::fs::metadata(format!("/proc/{pid}")).is_ok_and(|m| m.uid() == me))
        .filter(|pid| scope_of(*pid).is_some_and(|scope| kept.as_deref() != Some(scope.as_str()) && !kept_by_another_wm(&scope, *pid, std::process::id(), &proc_start)))
        .collect();
    out.sort_unstable();
    out
}

/// The AI Hub's supervisor.
pub type HubApp = Supervised<NodeConf>;
/// The devhub's supervisor.
pub type DevhubApp = Supervised<makepad_devhub_conf::NodeConf>;

pub struct Supervised<C: ServiceConf> {
    pub kind: &'static Kind,
    pub conf: C,
    conf_path: PathBuf,
    running: Option<Running>,
    /// Its output's log (`~/.makepad/wm/ai-hub.log`): the reason of an exit is in its tail.
    log: PathBuf,
    /// `~/.makepad/wm/ai-hub.state`.
    state: PathBuf,
    /// Exits in a row without a steady run between them.
    crashes: u32,
    restart_at: Option<f64>,
    /// Stopped on purpose: start again as soon as it is gone.
    restart_pending: bool,
    /// The first start looks for an app an earlier WM left running.
    first_start: bool,
    /// An adopted app being replaced gets SIGKILL at this time if it is still there.
    kill_at: Option<f64>,
    started_at: f64,
    /// When the first start began waiting for the WM's socket.
    socket_wait: Option<f64>,
    /// A kept app must come to this WM's socket by this time or it is stopped and started
    /// fresh (a tray process that never reattached would stay out of sight for good).
    adopt_by: Option<f64>,
    /// Hubs an earlier WM generation left on this port, being stopped: pid, SIGKILL time.
    rivals: Vec<(u32, f64)>,
    /// The kept devhub runs another program, but leases run under it: it is replaced once its
    /// leases are done (a replacement would end them: a landing's gate lost its node, 10-29).
    replace_when_idle: bool,
    /// Where the user's service manager's state comes from (tests give a fixed one).
    #[cfg(target_os = "linux")]
    manager: fn() -> Option<Manager>,
    /// Whether this WM may restart its session ([`session_restartable`]; tests set it).
    #[cfg(target_os = "linux")]
    restartable: bool,
    /// The session restart its start needs (the user's service manager is down).
    #[cfg(target_os = "linux")]
    session_restart: Option<SessionRestart>,
    /// What its start waits for, as last said.
    #[cfg(target_os = "linux")]
    waiting: Option<String>,
}

impl<C: ServiceConf> Supervised<C> {
    pub fn new(kind: &'static Kind, conf_path: PathBuf, conf: C) -> Supervised<C> {
        let wm = crate::theme::makepad_home().join("wm");
        Supervised {
            kind,
            conf,
            conf_path,
            running: None,
            log: wm.join(format!("{}.log", kind.file)),
            state: wm.join(format!("{}.state", kind.file)),
            crashes: 0,
            restart_at: Some(0.0),
            restart_pending: false,
            first_start: true,
            kill_at: None,
            started_at: 0.0,
            socket_wait: None,
            adopt_by: None,
            rivals: Vec::new(),
            replace_when_idle: false,
            #[cfg(target_os = "linux")]
            manager: user_manager,
            #[cfg(target_os = "linux")]
            restartable: session_restartable(),
            #[cfg(target_os = "linux")]
            session_restart: None,
            #[cfg(target_os = "linux")]
            waiting: None,
        }
    }

    /// The session restart its start needs: the user's service manager is down.
    #[cfg(target_os = "linux")]
    pub fn session_restart(&self) -> Option<&SessionRestart> {
        self.session_restart.as_ref()
    }

    /// The built app cargo runs (the deploy built it with the same features): its identity.
    fn binary(&self) -> Result<PathBuf, String> {
        let root = registry::repo_root().ok_or_else(|| format!("no makepad checkout to run the {} from", self.kind.label))?;
        let target_dir = std::env::var_os("CARGO_TARGET_DIR");
        Ok(registry::checkout_binary(&target_of(self.kind)?, &root, target_dir.as_deref()))
    }

    /// `cargo run --release -p makepad-app-aihub --features <features>` in the checkout, with the
    /// cargo environment of every launcher (`registry::cargo_env`), in a scope of the user's
    /// service manager when `scoped`, and under `nice` where it exists: they and cargo exec, so
    /// the app's pid is the child's.
    fn command(&self, scoped: bool) -> Result<Command, String> {
        self.cargo(Verb::Run, scoped)
    }

    /// The start's cargo command; `Verb::Build` is the deploy's build of it ([`build_node_hub`]):
    /// the same package, features, checkout and environment, without the scope and `nice`.
    fn cargo(&self, verb: Verb, scoped: bool) -> Result<Command, String> {
        let root = registry::repo_root().ok_or_else(|| format!("no makepad checkout to run the {} from", self.kind.label))?;
        let target = target_of(self.kind)?;
        let (cargo, cargo_args) = match verb {
            Verb::Run => registry::run_argv_features(&target, &root, self.conf.features(), &[]),
            Verb::Build => registry::build_argv_features(&target, &root, self.conf.features()),
        };
        let mut argv: Vec<std::ffi::OsString> = Vec::new();
        if verb == Verb::Run && scoped {
            let unit = format!("makepad-{}-{}-{}", self.kind.file, self.conf.port(), std::process::id());
            argv.extend([SYSTEMD_RUN.into(), "--user".into(), "--scope".into(), "--quiet".into(), "--collect".into(), format!("--unit={unit}").into()]);
        }
        if verb == Verb::Run && cfg!(unix) && self.conf.nice() > 0 {
            argv.extend(["nice".into(), "-n".into(), self.conf.nice().to_string().into()]);
        }
        argv.push(cargo.into_os_string());
        argv.extend(cargo_args.into_iter().map(Into::into));
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).current_dir(&root);
        registry::cargo_env(&mut cmd);
        // Its output is a log file (the deploy's: its log), not a terminal: plain lines.
        cmd.env("CARGO_TERM_PROGRESS_WHEN", "never");
        registry::child_env::scrub(&mut cmd);
        for (k, v) in self.conf.env() {
            cmd.env(k, v);
        }
        // The app serves as this node's service: from this file, not its own settings.
        cmd.env(self.kind.conf_env, &self.conf_path);
        Ok(cmd)
    }

    /// The first start: a live app an earlier WM started with this identity is kept (a WM
    /// restart no longer costs the hub, its loaded models or its running jobs). One started
    /// from another binary or other settings is stopped, and a new one starts once it is gone.
    fn adopt_or_replace(&mut self, now: f64, binary: &Path) -> bool {
        let label = self.kind.label;
        let Some((pid, id)) = read_state(&self.state) else { return false };
        if !app_alive(self.kind, pid) {
            return false;
        }
        if self.runs_as_started(pid, &id, binary) {
            makepad_widgets::log!("wm: {label}: kept pid {pid}, started by an earlier WM with this program and these settings");
            self.running = Some(Running::Adopted(pid));
            self.restart_at = None;
            self.started_at = now;
            self.adopt_by = Some(now + ADOPT_WAIT_S);
            // An orphan of an older generation may hold the port or the cache it needs: the kept
            // app's start is tried again on its own once they are gone.
            self.stop_rivals(now);
            return true;
        }
        if std::ptr::eq(self.kind, &DEVHUB) && !live_leases().is_empty() {
            makepad_widgets::log!("wm: {label}: pid {pid} runs another program, but leases run under it ({}): kept until they are done, then replaced", live_leases().join(", "));
            self.running = Some(Running::Adopted(pid));
            self.restart_at = None;
            self.started_at = now;
            self.adopt_by = Some(now + ADOPT_WAIT_S);
            self.replace_when_idle = true;
            return true;
        }
        makepad_widgets::log!("wm: {label}: pid {pid} runs another program or other settings; replacing it");
        self.replace(pid, now);
        true
    }

    /// Whether the kept app `pid` (its state's identity `id`) runs as this WM would start it: these
    /// settings and (Linux) the program `binary` holds now. Its start wrote `id` before cargo built
    /// it, so the binary's size and time in it can be the build before: a WM restart replaced
    /// AI Hubs that ran the current program (every devhub roll, 11-02), and a restart reloads
    /// their models. Elsewhere the recorded identity decides.
    fn runs_as_started(&self, pid: u32, id: &str, binary: &Path) -> bool {
        match registry::runs_binary(pid, binary) {
            Some(runs) => runs && recorded_settings(id, binary) == Some(self.conf.identity().as_str()),
            None => id == identity(binary, &self.conf),
        }
    }

    /// Stops the kept app `pid` (SIGTERM, SIGKILL after the grace) and starts this WM's own once
    /// it is gone.
    fn replace(&mut self, pid: u32, now: f64) {
        #[cfg(unix)]
        makepad_app_host::launch::signal_group(pid, false);
        self.kill_at = Some(now + STOP_GRACE.as_secs_f64());
        self.running = Some(Running::Adopted(pid));
        self.restart_pending = true;
    }

    fn spawn(&mut self, now: f64) {
        let label = self.kind.label;
        self.restart_at = None;
        self.restart_pending = false;
        // A restart starts with the settings as they are now.
        self.conf = C::reload(&self.conf_path, &self.conf);
        if !self.conf.enabled() {
            makepad_widgets::log!("wm: {label}: off in {}; not started", self.conf_path.display());
            return;
        }
        let binary = match self.binary() {
            Ok(binary) => binary,
            Err(reason) => {
                makepad_widgets::log!("wm: {label}: {reason}");
                self.retry_later(now, reason);
                return;
            }
        };
        if std::mem::take(&mut self.first_start) && self.adopt_or_replace(now, &binary) {
            return;
        }
        if self.stop_rivals(now) {
            // Started once they are gone: they hold the port and the cache lock it needs.
            self.restart_at = Some(now + 1.0);
            return;
        }
        #[cfg(target_os = "linux")]
        let Some(scoped) = self.scope(now) else { return };
        #[cfg(not(target_os = "linux"))]
        let scoped = false;
        let mut cmd = match self.command(scoped) {
            Ok(cmd) => cmd,
            Err(reason) => {
                makepad_widgets::log!("wm: {label}: {reason}");
                self.retry_later(now, reason);
                return;
            }
        };
        #[cfg(unix)]
        makepad_app_host::launch::own_process_group(&mut cmd);
        crate::host::no_console_window(&mut cmd);
        if let Some(dir) = self.log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Its output goes to the log file, not a pipe: it outlives this WM.
        let out = std::fs::File::create(&self.log).ok();
        let err = out.as_ref().and_then(|f| f.try_clone().ok());
        cmd.stdin(Stdio::null())
            .stdout(out.map_or(Stdio::null(), Stdio::from))
            .stderr(err.map_or(Stdio::null(), Stdio::from));
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                let reason = format!("could not start {:?}: {e}", cmd.get_program());
                makepad_widgets::log!("wm: {label}: {reason}");
                self.retry_later(now, reason);
                return;
            }
        };
        // Windows: no keeping; the app ends with the WM's job object.
        #[cfg(windows)]
        makepad_app_host::launch::exit_with_parent(&child);
        #[cfg(unix)]
        if let Err(e) = std::fs::write(&self.state, format!("pid = {}\nidentity = {}\n", child.id(), identity(&binary, &self.conf))) {
            makepad_widgets::log!("wm: {label}: {}: {e}", self.state.display());
        }
        makepad_widgets::log!(
            "wm: {}: started pid {} ({:?} {}, port {}{})",
            self.kind.label,
            child.id(),
            cmd.get_program(),
            cmd.get_args().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "),
            self.conf.port(),
            self.conf.note()
        );
        self.running = Some(Running::Child(child));
        self.started_at = now;
    }

    /// Hubs an earlier WM generation started for this port that no WM keeps any more (the
    /// `makepad-ai-hub-<port>-<wm pid>` scopes of this user other than the running app's): an
    /// orphan holds the port and the cache lock, so this node's own hub cannot serve (the old
    /// headless hub left by the WM generation before the app, 10-26). Each gets SIGTERM (its hub
    /// drains), SIGKILL after the stop grace. True while one is still there.
    fn stop_rivals(&mut self, now: f64) -> bool {
        #[cfg(target_os = "linux")]
        {
            let label = self.kind.label;
            let keep = self.running.as_ref().map(Running::pid);
            let found = rival_hubs(self.kind.file, self.conf.port(), keep);
            for pid in &found {
                match self.rivals.iter_mut().find(|(p, _)| p == pid) {
                    None => {
                        makepad_widgets::log!("wm: {label}: pid {pid} is a hub an earlier WM started on port {} and no WM keeps; stopping it", self.conf.port());
                        makepad_app_host::launch::signal_pid(*pid, false);
                        self.rivals.push((*pid, now + STOP_GRACE.as_secs_f64()));
                    }
                    Some((_, at)) if now >= *at => {
                        makepad_widgets::log!("wm: {label}: pid {pid} outlived its stop; SIGKILL");
                        makepad_app_host::launch::signal_pid(*pid, true);
                        *at = f64::INFINITY;
                    }
                    Some(_) => {}
                }
            }
            self.rivals.retain(|(p, _)| found.contains(p));
            return !self.rivals.is_empty();
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = now;
            false
        }
    }

    /// Where the start goes on Linux: Some(true) into a scope of the user's service manager,
    /// Some(false) as this WM's child (this Linux runs services without one); None: not now (said,
    /// and tried again later). A dead manager asks for the WM's session restart ([`session_step`]),
    /// once per outage: a new session that did not start it again is not restarted again.
    #[cfg(target_os = "linux")]
    fn scope(&mut self, now: f64) -> Option<bool> {
        let marker = self.state.with_file_name(SESSION_RESTART_FILE);
        match (self.manager)() {
            None => Some(false),
            Some(Manager::Up) => {
                self.waiting = None;
                self.session_restart = None;
                let _ = std::fs::remove_file(&marker);
                Some(true)
            }
            Some(Manager::Changing(state)) => {
                let what = format!("waits for the user's service manager ({} is {state})", manager_unit());
                if self.waiting.as_deref() != Some(what.as_str()) {
                    makepad_widgets::log!("wm: {} {what}", self.kind.label);
                    self.waiting = Some(what);
                }
                self.restart_at = Some(now + 1.0);
                None
            }
            Some(Manager::Down { state, down }) => {
                let gone = format!("the user's service manager is down ({} {state}), and a start in a scope of it fails", manager_unit());
                let tried = std::fs::read_to_string(&marker).is_ok_and(|t| t.lines().next() == Some(down.as_str()));
                let reason = if tried {
                    format!("{gone}; the WM's new session did not start it again: a login on this box does")
                } else if !self.restartable {
                    format!("{gone}; this WM does not run as its systemd unit and cannot restart its session: a login on this box starts it again")
                } else {
                    let why = format!("{gone}; the WM restarts its session to start it again");
                    self.session_restart = Some(SessionRestart { why: why.clone(), down });
                    why
                };
                self.retry_later(now, reason);
                None
            }
            Some(Manager::Unknown(why)) => {
                self.retry_later(now, format!("cannot tell whether the user's service manager runs: {why}"));
                None
            }
        }
    }

    /// A start that failed (not built yet, say) is tried again like an exit.
    fn retry_later(&mut self, now: f64, reason: String) {
        let label = self.kind.label;
        self.crashes += 1;
        let wait = backoff_s(self.crashes);
        makepad_widgets::log!("wm: {label} did not start ({reason}); trying again in {wait} s");
        self.note_trouble(&format!("could not start: {reason}"));
        self.restart_at = Some(now + wait as f64);
    }

    /// Why it does not run, for the service to report once it does ([`Kind::trouble`]).
    fn note_trouble(&self, what: &str) {
        if !self.kind.trouble {
            return;
        }
        let path = makepad_devhub_conf::trouble_path(&self.conf_path);
        let prev = std::fs::read_to_string(&path).ok().and_then(|t| makepad_devhub_conf::Trouble::parse(&t));
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        if let Err(e) = std::fs::write(&path, makepad_devhub_conf::Trouble::next(prev, now_ms, what).to_text()) {
            makepad_widgets::log!("wm: {}: {}: {e}", self.kind.label, path.display());
        }
    }

    /// The last lines of its log.
    fn tail(&self) -> Vec<String> {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        lines[lines.len().saturating_sub(TAIL)..].to_vec()
    }

    /// Why it ended: its exit status, with the app's last error or panic line when it said one.
    fn exit_reason(&self, status: &str) -> String {
        let tail = self.tail();
        let said = tail.iter().rev().find(|l| l.contains("panicked") || l.contains("error") || l.contains("Error"));
        match said {
            Some(line) => format!("{status}: {line}"),
            None => status.to_string(),
        }
    }

    /// The pid of the app this WM runs (its own child, or one an earlier WM left running):
    /// the one the WM socket then admits as the node's hub (a service client, no tile).
    pub fn pid(&self) -> Option<u32> {
        self.running.as_ref().map(Running::pid)
    }

    /// The person started the hub from the launcher after its Quit (the caller wrote
    /// `enabled = on`): it starts now, with a clean slate.
    pub fn user_start(&mut self, now: f64) {
        self.conf = C::reload(&self.conf_path, &self.conf);
        self.crashes = 0;
        if self.running.is_none() {
            self.restart_at = Some(now);
        }
    }

    /// Once a second from the WM's tick: exit, restart.
    pub fn tick(&mut self, now: f64) {
        let label = self.kind.label;
        if !self.rivals.is_empty() {
            self.stop_rivals(now);
        }
        if self.replace_when_idle && self.kill_at.is_none() {
            if let Some(Running::Adopted(pid)) = self.running {
                if live_leases().is_empty() {
                    makepad_widgets::log!("wm: {label}: the leases of pid {pid} are done; replacing it with this WM's program");
                    self.replace_when_idle = false;
                    self.adopt_by = None;
                    self.replace(pid, now);
                }
            } else {
                self.replace_when_idle = false;
            }
        }
        if let (Some(Running::Adopted(pid)), Some(at)) = (&self.running, self.kill_at) {
            if now >= at && app_alive(self.kind, *pid) {
                makepad_widgets::log!("wm: {label}: pid {pid} outlived its stop; SIGKILL");
                #[cfg(unix)]
                makepad_app_host::launch::signal_group(*pid, true);
                self.kill_at = None;
            }
        }
        if let (Some(Running::Adopted(pid)), Some(by)) = (&self.running, self.adopt_by) {
            let pid = *pid;
            if note_adopted_seen(pid) {
                self.adopt_by = None;
            } else if now >= by && self.kill_at.is_none() {
                makepad_widgets::log!("wm: {label}: kept pid {pid} did not come to this WM within {ADOPT_WAIT_S} s; stopping it to start a fresh one");
                #[cfg(unix)]
                makepad_app_host::launch::signal_group(pid, false);
                self.kill_at = Some(now + STOP_GRACE.as_secs_f64());
                self.restart_pending = true;
                self.adopt_by = None;
            }
        }
        // (and whether it is known to have failed: a clean exit is the hub's self-update, and one
        // an earlier WM started ends with a status this WM never sees)
        let exited = match self.running.as_mut() {
            Some(Running::Child(child)) => match child.try_wait() {
                Ok(Some(status)) => Some((status.to_string(), !status.success())),
                Ok(None) => None,
                Err(e) => {
                    makepad_widgets::log!("wm: {label}: {e}");
                    None
                }
            },
            Some(Running::Adopted(pid)) => (!app_alive(self.kind, *pid)).then(|| ("exited".to_string(), false)),
            None => None,
        };
        if let Some((status, failed)) = exited {
            let adopted = matches!(self.running, Some(Running::Adopted(_)));
            self.running = None;
            self.kill_at = None;
            if self.restart_pending {
                makepad_widgets::log!("wm: {label} stopped ({status}) for a restart");
                self.crashes = 0;
                if adopted {
                    // Not our child: gone from its command line, it may still hold its port until
                    // its own parent reaps it. The new one starts on the next tick.
                    self.restart_pending = false;
                    self.restart_at = Some(now + 1.0);
                } else {
                    self.spawn(now);
                }
            } else if !C::reload(&self.conf_path, &self.conf).enabled() {
                // Its Quit (or the settings turned it off): it stays off until the person starts
                // it from the launcher.
                makepad_widgets::log!("wm: {label} exited ({status}); off in {}: not started again", self.conf_path.display());
                self.crashes = 0;
                self.restart_at = None;
            } else {
                if now - self.started_at >= STEADY_S {
                    self.crashes = 0;
                }
                self.crashes += 1;
                if self.kind.file == DEVHUB.file && self.crashes >= 2 {
                    tree_to_synced();
                }
                let wait = backoff_s(self.crashes);
                let reason = self.exit_reason(&status);
                makepad_widgets::log!("wm: {label} exited ({reason}); restart {} in {wait} s", self.crashes);
                if failed {
                    self.note_trouble(&format!("exited ({reason})"));
                }
                self.restart_at = Some(now + wait as f64);
            }
            return;
        }
        if self.running.is_none() {
            if self.restart_at.is_some_and(|at| now >= at) && self.socket_ready(now) {
                self.spawn(now);
            }
        }
    }

    /// The app finds this WM by its socket (`MAKEPAD_WM_SOCKET`, set once the WM publishes it):
    /// the first start waits a moment for it.
    fn socket_ready(&mut self, now: f64) -> bool {
        #[cfg(target_os = "linux")]
        {
            use makepad_widgets::makepad_platform::makepad_hal::wm_socket::SOCKET_ENV;
            if std::env::var_os(SOCKET_ENV).is_none() {
                let since = *self.socket_wait.get_or_insert(now);
                if now - since < SOCKET_WAIT_S {
                    return false;
                }
            }
        }
        let _ = now;
        true
    }

    /// The WM quits. Unix: the app keeps running, and the next WM keeps it. Windows: it ends
    /// with the WM.
    pub fn stop(&mut self, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        let label = self.kind.label;
        self.restart_at = None;
        self.restart_pending = false;
        let Some(running) = self.running.as_mut() else { return };
        if cfg!(unix) {
            makepad_widgets::log!("wm: the {label} (pid {}) keeps running for the next WM", running.pid());
        } else if let Running::Child(child) = running {
            makepad_widgets::log!("wm: shutdown stops the {label} (pid {})", child.id());
            makepad_app_host::launch::kill_child_group(child, STOP_GRACE, pool);
        }
    }
}

/// The leases the node's devhub runs now (`~/.makepad/devhub/leases/<id>.json`, one per live
/// lease, removed at its end).
fn live_leases() -> Vec<String> {
    let dir = crate::theme::makepad_home().join("devhub").join("leases");
    let mut ids: Vec<String> = std::fs::read_dir(dir)
        .map(|d| d.flatten().filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_string)).collect())
        .unwrap_or_default();
    ids.sort();
    ids
}

/// How long a kept app has to come to the new WM's socket.
const ADOPT_WAIT_S: f64 = 10.0;

/// Service pids that came to this WM's socket.
static ADOPTED: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// The WM admitted service `pid` (adopt.rs).
pub fn note_adopted(pid: u32) {
    if let Ok(mut v) = ADOPTED.lock() { v.push(pid); }
}

fn note_adopted_seen(pid: u32) -> bool {
    ADOPTED.lock().map_or(true, |v| v.contains(&pid))
}

/// Whether `pid` is the AI Hub app this node's WMs started (`ai-hub.state` names it): when its
/// process comes to the WM socket it is a service the WM supervises, not a window.
pub fn is_hub_pid(pid: u32) -> bool {
    is_service_pid(&AI_HUB, pid)
}

/// Whether `pid` is the app of service `kind` this node's WMs started (its state file names it).
pub fn is_service_pid(kind: &Kind, pid: u32) -> bool {
    read_state(&crate::theme::makepad_home().join(format!("wm/{}.state", kind.file))).is_some_and(|(p, _)| p == pid)
}

/// The node's hub is off (`enabled = off`): one an earlier WM left running stops.
pub fn stop_leftover(pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
    let state = crate::theme::makepad_home().join("wm/ai-hub.state");
    if let Some((pid, _)) = read_state(&state).filter(|(pid, _)| app_alive(&AI_HUB, *pid)) {
        makepad_widgets::log!("wm: the AI Hub is off: stopping pid {pid}, left by an earlier WM");
        #[cfg(unix)]
        makepad_app_host::launch::kill_group_pid(pid, STOP_GRACE, pool);
        #[cfg(not(unix))]
        let _ = pool;
    }
}

/// From SIGTERM to SIGKILL when stopping it: the hub drains (admission closed, running jobs
/// handed back) and the app quits.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(7);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarts_back_off_and_cap() {
        assert_eq!((1..=10).map(backoff_s).collect::<Vec<_>>(), [2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
    }

    #[test]
    fn the_app_starts_through_cargo_run_with_the_node_features() {
        let target = hub_target().unwrap();
        let (cargo, args) = registry::run_argv_features(&target, Path::new("/ck"), &NodeConf::default().features, &[]);
        assert_eq!(cargo.file_name().and_then(|n| n.to_str()), Some("cargo"));
        assert_eq!(args, ["run", "--release", "--manifest-path", "/ck/Cargo.toml", "-p", "makepad-app-aihub", "--bin", "makepad-app-aihub", "--features", "node", "--"]);
        // The deploy's build is the same command with `build`.
        let (_, build) = registry::build_argv_features(&target, Path::new("/ck"), "node");
        assert_eq!(build[1..], args[1..args.len() - 1]);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wm-hub-app-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn app(name: &str) -> HubApp {
        let dir = scratch(name);
        let mut h = HubApp::new(&AI_HUB, dir.join("ai-hub.conf"), NodeConf::default());
        h.log = dir.join("ai-hub.log");
        h.state = dir.join("ai-hub.state");
        h.first_start = false;
        h.restart_at = None;
        h
    }

    #[test]
    fn an_exit_restarts_after_its_backoff_and_a_steady_run_starts_the_count_over() {
        let mut h = app("exit");
        std::fs::write(&h.log, "listening\nthread 'main' panicked at serve.rs:1: no such hub flag\n").unwrap();
        let mut child = Command::new("false").spawn().unwrap();
        let _ = child.wait();
        h.running = Some(Running::Child(child));
        h.started_at = 0.0;
        h.tick(10.0);
        assert_eq!((h.crashes, h.restart_at), (1, Some(12.0)));
        assert!(h.exit_reason("exit status: 1").contains("no such hub flag"), "its last error is the reason");
        // A run that lasted past STEADY_S: the count starts over.
        let mut child = Command::new("false").spawn().unwrap();
        let _ = child.wait();
        h.running = Some(Running::Child(child));
        h.started_at = 20.0;
        h.tick(20.0 + STEADY_S + 1.0);
        assert_eq!(h.crashes, 1);
    }

    #[test]
    fn an_exit_after_its_quit_stays_off_until_the_launcher_starts_it() {
        let mut h = app("quit");
        let mut conf = NodeConf::default();
        conf.enabled = false;
        conf.save_to(&h.conf_path).unwrap();
        let mut child = Command::new("false").spawn().unwrap();
        let _ = child.wait();
        h.running = Some(Running::Child(child));
        h.tick(10.0);
        assert!(h.running.is_none());
        assert_eq!((h.crashes, h.restart_at), (0, None), "its Quit is no crash and schedules nothing");
        h.tick(1000.0);
        assert!(h.running.is_none(), "still off");
        // The launcher writes `enabled = on` and starts it.
        conf.enabled = true;
        conf.save_to(&h.conf_path).unwrap();
        h.user_start(2000.0);
        assert_eq!(h.restart_at, Some(2000.0));
        assert!(h.conf.enabled);
    }

    #[test]
    fn the_state_file_names_the_app_and_its_identity_follows_program_and_settings() {
        let dir = scratch("state");
        let state = dir.join("ai-hub.state");
        std::fs::write(&state, "pid = 4242\nidentity = /x/makepad-app-aihub 10 20 nice=10 features=node env=[]\n").unwrap();
        assert_eq!(read_state(&state), Some((4242, "/x/makepad-app-aihub 10 20 nice=10 features=node env=[]".to_string())));
        std::fs::write(&state, "pid = nope\n").unwrap();
        assert_eq!(read_state(&state), None);
        let program = dir.join("makepad-app-aihub");
        std::fs::write(&program, b"one").unwrap();
        let conf = NodeConf::default();
        let first = identity(&program, &conf);
        assert_eq!(first, identity(&program, &conf), "the same app is kept");
        assert_ne!(first, identity(&program, &NodeConf { model: "qwen3.8-27b".into(), ..Default::default() }), "other settings replace it");
        assert_ne!(first, identity(&program, &NodeConf { features: "node,claude-stub".into(), ..Default::default() }), "other features replace it");
        std::fs::write(&program, b"rebuilt").unwrap();
        assert_ne!(first, identity(&program, &conf), "a new build replaces it");
        // Keeping needs a live app: a gone pid is never taken.
        let mut h = app("adopt");
        h.state = state.clone();
        std::fs::write(&state, format!("pid = {}\nidentity = {}\n", u32::MAX - 7, identity(&program, &h.conf))).unwrap();
        assert!(!h.adopt_or_replace(0.0, &program));
        assert!(h.running.is_none());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_kept_app_runs_as_started_when_it_runs_the_current_program() {
        let dir = scratch("runs");
        let program = dir.join("makepad-app-aihub");
        std::fs::copy("/bin/sleep", &program).unwrap();
        let h = app("runs");
        // its start recorded the binary before cargo built it again (same bytes, a new file)
        let stale = format!("{} 1 2 {}", program.display(), h.conf.identity());
        let mut child = Command::new(&program).arg("30").spawn().unwrap();
        let fresh = identity(&program, &h.conf);
        assert!(h.runs_as_started(child.id(), &fresh, &program));
        assert!(h.runs_as_started(child.id(), &stale, &program), "a stale size and time in its state never replace the program it runs");
        let other = format!("{} 1 2 nice=0 features=other env=[]", program.display());
        assert!(!h.runs_as_started(child.id(), &other, &program), "other settings replace it");
        let next = dir.join("next");
        std::fs::copy("/bin/sleep", &next).unwrap();
        std::fs::rename(&next, &program).unwrap();
        assert!(h.runs_as_started(child.id(), &stale, &program), "a relink to the same bytes keeps it");
        std::fs::write(&next, b"another build").unwrap();
        std::fs::rename(&next, &program).unwrap();
        assert!(!h.runs_as_started(child.id(), &fresh, &program), "a new build replaces it");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_hub_scope_is_the_wms_unit_for_the_port() {
        let cg = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/makepad-ai-hub-8123-2890550.scope\n";
        assert_eq!(hub_scope(cg, "ai-hub", 8123), Some("makepad-ai-hub-8123-2890550.scope"));
        assert_eq!(hub_scope(cg, "ai-hub", 8124), None, "another port's hub is not this one's rival");
        assert_eq!(hub_scope("0::/user.slice/user-1000.slice/session-3.scope\n", "ai-hub", 8123), None);
        assert_eq!(hub_scope("0::/app.slice/makepad-ai-hub-8123-.scope\n", "ai-hub", 8123), None);
        assert_eq!(hub_scope("0::/app.slice/makepad-ai-hub-8123-12x.scope\n", "ai-hub", 8123), None);
        assert_eq!(hub_scope("0::/system.slice/makepad-wm.service\n", "ai-hub", 8123), None);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn processes_outside_a_hub_scope_are_never_rivals() {
        // This test process and its children run in no makepad-ai-hub scope.
        let mut sleeper = Command::new("sleep").arg("30").spawn().unwrap();
        assert!(!rival_hubs("ai-hub", 8123, None).contains(&sleeper.id()));
        assert!(!rival_hubs("devhub", 8797, None).contains(&sleeper.id()));
        let _ = sleeper.kill();
        let _ = sleeper.wait();
    }

    /// Two WMs on one session: the first holds the default path and keeps the node's services, the
    /// second gets another and keeps none. After the first goes (its file stays, as a crash leaves
    /// it), the next WM holds the default path again: the update's handover.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_second_wm_keeps_nothing_and_the_next_after_the_first_does() {
        use makepad_widgets::makepad_platform::makepad_hal::wm_socket::{self, WmSocket};
        let dir = scratch("two-wms");
        let second_at = |path: &Path| second_on_session(Some(path), Some(&dir));
        let first = WmSocket::bind(&dir, 0).unwrap();
        let second = WmSocket::bind(&dir, 0).unwrap();
        assert_eq!(first.path, wm_socket::socket_path(&dir, 0));
        assert!(!second_at(&first.path));
        assert!(second_at(&second.path));
        drop(first);
        let next = WmSocket::bind(&dir, 0).unwrap();
        assert!(!second_at(&next.path), "{:?}", next.path);
        // no socket published: no other WM to tell it from
        assert!(!second_on_session(None, Some(&dir)));
        assert!(!second_on_session(Some(&second.path), None));
    }

    /// The hub another live WM keeps is no rival; one whose WM is gone, or this WM's own leftover,
    /// is. A process that took the dead WM's pid since is not that WM.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_hub_another_live_wm_keeps_is_no_rival() {
        let start = |pid: u32| match pid {
            10 => Some(100),
            20 => Some(200),
            30 => Some(300),
            _ => None,
        };
        let scope = |wm: u32| format!("makepad-devhub-8797-{wm}.scope");
        assert!(kept_by_another_wm(&scope(10), 20, 1, &start));
        assert!(!kept_by_another_wm(&scope(99), 20, 1, &start), "its WM is gone");
        assert!(!kept_by_another_wm(&scope(10), 20, 10, &start), "this WM's own");
        assert!(!kept_by_another_wm(&scope(30), 20, 1, &start), "a reused pid");
    }

    #[test]
    fn devhub_starts_and_builds_with_one_cargo_command() {
        // The WM's start and the deploy's build (`makepad-wm-build -- devhub`) are the same
        // cargo invocation with `run` for `build`: a start after the deploy compiles nothing.
        let dir = scratch("devhub");
        let conf = makepad_devhub_conf::NodeConf::parse("name = t1\nport = 8797\n");
        let d = DevhubApp::new(&DEVHUB, dir.join("devhub.conf"), conf);
        let args = |verb| -> (Vec<String>, Vec<(String, String)>) {
            let cmd = d.cargo(verb, verb == Verb::Run).unwrap();
            let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
            let envs = cmd.get_envs().filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned()))).collect();
            (args, envs)
        };
        let (run, run_env) = args(Verb::Run);
        let (build, build_env) = args(Verb::Build);
        let tail = |a: &[String], verb: &str| a.iter().skip_while(|w| *w != verb).skip(1).take_while(|w| *w != "--").cloned().collect::<Vec<_>>();
        assert!(run.iter().any(|a| a == "makepad-app-devhub"), "{run:?}");
        assert!(run.iter().any(|a| a == "--release"), "{run:?}");
        assert_eq!(tail(&run, "run"), tail(&build, "build"), "same package, profile and features");
        assert_eq!(run_env, build_env, "same cargo environment");
        assert!(run_env.iter().any(|(k, v)| k == makepad_devhub_conf::NODE_CONF_ENV && v.ends_with("devhub.conf")));
        assert_eq!(d.state.file_name().unwrap(), "devhub.state");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn the_user_managers_state_comes_from_its_unit() {
        assert_eq!(Manager::parse("ActiveState=active\nInactiveEnterTimestampMonotonic=0\n"), Manager::Up);
        assert_eq!(Manager::parse("ActiveState=activating\n"), Manager::Changing("activating".into()));
        assert_eq!(Manager::parse("ActiveState=failed\nInactiveEnterTimestampMonotonic=4242\n"), Manager::Down { state: "failed".into(), down: "4242".into() });
        assert!(matches!(Manager::parse(""), Manager::Unknown(_)));
    }

    #[cfg(target_os = "linux")]
    fn devhub_app(dir: &Path, manager: fn() -> Option<Manager>) -> DevhubApp {
        let conf = makepad_devhub_conf::NodeConf::parse("name = t1\nport = 8797\n");
        let mut d = DevhubApp::new(&DEVHUB, dir.join("devhub.conf"), conf);
        d.log = dir.join("devhub.log");
        d.state = dir.join("devhub.state");
        d.first_start = false;
        d.restart_at = None;
        d.manager = manager;
        d.restartable = true;
        d
    }

    /// 11-03: a global OOM killed the user's service manager; every start of the devhub through it
    /// failed for 24 min, retried in the WM's log only, until a login on the box started it again.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_dead_user_manager_restarts_the_session_once_and_the_outage_is_recorded() {
        fn down() -> Option<Manager> {
            Some(Manager::parse("ActiveState=failed\nInactiveEnterTimestampMonotonic=4242\n"))
        }
        fn down_later() -> Option<Manager> {
            Some(Manager::parse("ActiveState=failed\nInactiveEnterTimestampMonotonic=9999\n"))
        }
        fn starting() -> Option<Manager> {
            Some(Manager::parse("ActiveState=activating\n"))
        }
        fn up() -> Option<Manager> {
            Some(Manager::Up)
        }
        let dir = scratch("dead-manager");
        let trouble = || makepad_devhub_conf::Trouble::parse(&std::fs::read_to_string(makepad_devhub_conf::trouble_path(&dir.join("devhub.conf"))).unwrap()).unwrap();
        let mut d = devhub_app(&dir, down);
        assert_eq!(d.scope(10.0), None, "no start through a dead manager");
        let want = d.session_restart().cloned().expect("it asks for the session restart");
        assert_eq!(want.down, "4242");
        assert!(want.why.contains("the user's service manager is down (user@") && want.why.ends_with("failed), and a start in a scope of it fails; the WM restarts its session to start it again"), "{}", want.why);
        let t = trouble();
        assert_eq!(t.tries, 1);
        assert_eq!(t.reason, format!("could not start: {}", want.why), "the outage and its reason, for the hub to report");
        // the restart ends every app of the session: it waits for unsaved work, then goes
        match session_step(&want, &[(7, "Fab".into(), "bracket.fab".into())]) {
            SessionStep::Waits(text) => assert!(text.ends_with("once no app holds unsaved work (Fab: bracket.fab)"), "{text}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(session_step(&want, &[]), SessionStep::Restart(want.why.clone()));
        begin_session_restart(&dir, &want).unwrap();
        // the next WM, same outage: its new session did not start the manager, so no second restart
        let mut next = devhub_app(&dir, down);
        assert_eq!(next.scope(20.0), None);
        assert_eq!(next.session_restart(), None, "a session restart is not repeated for one outage");
        let t = trouble();
        assert_eq!(t.tries, 2);
        assert!(t.reason.ends_with("the WM's new session did not start it again: a login on this box does"), "{}", t.reason);
        // a WM not run by its unit cannot restart its session, and says so
        let mut manual = devhub_app(&dir, down_later);
        manual.restartable = false;
        assert_eq!(manual.scope(25.0), None);
        assert_eq!(manual.session_restart(), None);
        assert!(trouble().reason.contains("cannot restart its session"));
        // a later outage of the manager may restart the session again
        next.manager = down_later;
        assert_eq!(next.scope(30.0), None);
        assert_eq!(next.session_restart().map(|w| w.down.as_str()), Some("9999"));
        // the new session pulls the manager in: the start waits for it, said once
        next.manager = starting;
        assert_eq!(next.scope(40.0), None);
        assert_eq!(next.restart_at, Some(41.0));
        assert!(next.waiting.as_deref().is_some_and(|w| w.ends_with("is activating)")));
        // it runs: the start goes into a scope of it, and the outage's restart record is gone
        next.manager = up;
        assert_eq!(next.scope(42.0), Some(true));
        assert_eq!((next.session_restart(), next.waiting.as_deref()), (None, None));
        assert!(!dir.join(SESSION_RESTART_FILE).exists());
        // the AI Hub keeps no trouble record (devhub reports its own)
        assert!(!AI_HUB.trouble && DEVHUB.trouble);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The hub exits cleanly for its self-update (the WM starts the new build): no outage. Each
    /// self-update was reported as "could not run ... exited (exit status: 0)" (11-03).
    #[test]
    #[cfg(target_os = "linux")]
    fn a_clean_exit_is_no_outage_and_a_failed_one_is() {
        fn up() -> Option<Manager> {
            Some(Manager::Up)
        }
        let dir = scratch("clean-exit");
        let file = makepad_devhub_conf::trouble_path(&dir.join("devhub.conf"));
        let mut d = devhub_app(&dir, up);
        let mut exit_with = |cmd: &str, now: f64| {
            let mut child = Command::new(cmd).spawn().unwrap();
            let _ = child.wait();
            d.running = Some(Running::Child(child));
            d.started_at = now - STEADY_S - 1.0;
            d.tick(now);
        };
        exit_with("true", 100.0);
        assert!(!file.exists(), "its self-update is no outage");
        exit_with("false", 200.0);
        let t = makepad_devhub_conf::Trouble::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((t.tries, t.reason.as_str()), (1, "exited (exit status: 1)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_kept_app_is_watched_by_pid() {
        let mut h = app("kept");
        let mut sleeper = Command::new("sleep").arg("30").spawn().unwrap();
        h.running = Some(Running::Adopted(sleeper.id()));
        h.tick(1.0);
        // (app_alive checks the command line on Linux: a sleep is not the app, so it reads as
        // gone there and a restart is due; elsewhere it is alive.)
        assert_eq!(h.running.is_none(), cfg!(target_os = "linux"));
        let _ = sleeper.kill();
        let _ = sleeper.wait();
    }
}
