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
//!   log file, on Linux under systemd in a scope of the user's manager (the WM's unit stops its
//!   whole control group), and `~/.makepad/wm/ai-hub.state` names its pid and what it was
//!   started as. Its windows and its tray wait for the next WM and come back on it (the HAL's
//!   WM shell re-attaches a tray app). The next WM keeps a live app with the same start (its
//!   models stay loaded, its jobs keep running); one started from another binary or other
//!   settings is replaced;
//! - a stop is SIGTERM: the app's hub closes admission (the fleet routes jobs elsewhere), hands
//!   its running jobs back to their clients and quits. Windows: it ends with the WM.
//!
//! The same supervisor keeps the node's **devhub** (apps/devhub: the JIT observer the user's Mac
//! master reaches): a [`Supervised`] service is one [`Kind`] (its app, files, log label and the
//! env var its conf path goes in) and one [`ServiceConf`] (what its settings file says).
//! [`HubApp`] is the AI Hub's, [`DevhubApp`] the devhub's.
//!
//! A hidden WM (`MAKEPAD_HIDE_WINDOWS`, the test runs) starts no hub unless
//! `MAKEPAD_WM_AI_HUB_CONF` names its settings: test desktops must not take the GPU or the port
//! of the real one. Its output goes to `~/.makepad/wm/ai-hub.log`.

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
}

pub const AI_HUB: Kind = Kind { app_id: crate::clients::HUB_SERVICE_APP, file: "ai-hub", label: "AI Hub", conf_env: NODE_CONF_ENV };
pub const DEVHUB: Kind = Kind { app_id: crate::clients::DEVHUB_SERVICE_APP, file: "devhub", label: "devhub", conf_env: makepad_devhub::conf::NODE_CONF_ENV };

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

impl ServiceConf for makepad_devhub::conf::NodeConf {
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
        self.nice
    }
    fn port(&self) -> u16 {
        self.port
    }
    fn env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn identity(&self) -> String {
        format!("nice={} name={} port={}", self.nice, self.name, self.port)
    }
    fn note(&self) -> String {
        format!(", node {}", self.name)
    }
}

/// The node's devhub settings (`~/.makepad/wm/devhub.conf`, `MAKEPAD_WM_DEVHUB_CONF` names
/// another); a hidden WM without its own file starts none.
pub fn load_devhub_conf() -> (PathBuf, makepad_devhub::conf::NodeConf) {
    let path = std::env::var_os("MAKEPAD_WM_DEVHUB_CONF").filter(|p| !p.is_empty()).map(PathBuf::from).unwrap_or_else(|| crate::theme::makepad_home().join("wm/devhub.conf"));
    let mut conf = makepad_devhub::conf::NodeConf::load(&path);
    if std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() && std::env::var_os("MAKEPAD_WM_DEVHUB_CONF").is_none() {
        conf.enabled = false;
    }
    (path, conf)
}

/// The node's settings as the WM reads them: the file, or the defaults; a hidden WM without its
/// own settings file starts no hub.
pub fn load_conf() -> (PathBuf, NodeConf) {
    let path = NodeConf::path();
    let mut conf = NodeConf::load_from(&path);
    if std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() && std::env::var_os(CONF_ENV).is_none() {
        conf.enabled = false;
    }
    (path, conf)
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
    let mut cmd = match app.cargo(Verb::Build) {
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

/// Linux under systemd: the WM's own unit stops its whole control group, so an app that is to
/// outlive a WM restart runs in a scope of the user's manager (`systemd-run --user --scope`,
/// which execs it: the child's pid is the app's).
#[cfg(target_os = "linux")]
fn user_scope() -> Option<PathBuf> {
    let run = PathBuf::from("/usr/bin/systemd-run");
    let bus = std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("bus"));
    (run.is_file() && bus.is_some_and(|b| b.exists())).then_some(run)
}

/// The `makepad-ai-hub-<port>-<wm pid>.scope` a process runs in (its `/proc/<pid>/cgroup`
/// text), for `port`: where this node's WMs start the AI Hub.
fn hub_scope<'a>(cgroup: &'a str, file: &str, port: u16) -> Option<&'a str> {
    let prefix = format!("makepad-{file}-{port}-");
    cgroup.lines().filter_map(|line| line.rsplit('/').next()).find(|unit| {
        unit.strip_prefix(prefix.as_str()).and_then(|rest| rest.strip_suffix(".scope")).is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// This user's processes in a hub scope for `port` other than `keep`'s (its own children, the
/// sandboxed Claude, share its scope).
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
        .filter(|pid| scope_of(*pid).is_some_and(|scope| kept.as_deref() != Some(scope.as_str())))
        .collect();
    out.sort_unstable();
    out
}

/// The AI Hub's supervisor.
pub type HubApp = Supervised<NodeConf>;
/// The devhub's supervisor.
pub type DevhubApp = Supervised<makepad_devhub::conf::NodeConf>;

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
        }
    }

    /// The built app cargo runs (the deploy built it with the same features): its identity.
    fn binary(&self) -> Result<PathBuf, String> {
        let root = registry::repo_root().ok_or_else(|| format!("no makepad checkout to run the {} from", self.kind.label))?;
        let target_dir = std::env::var_os("CARGO_TARGET_DIR");
        Ok(registry::checkout_binary(&target_of(self.kind)?, &root, target_dir.as_deref()))
    }

    /// `cargo run --release -p makepad-app-aihub --features <features>` in the checkout, with the
    /// cargo environment of every launcher (`registry::cargo_env`), under the user's systemd
    /// scope and `nice` where they exist: they and cargo exec, so the app's pid is the child's.
    fn command(&self) -> Result<Command, String> {
        self.cargo(Verb::Run)
    }

    /// The start's cargo command; `Verb::Build` is the deploy's build of it ([`build_node_hub`]):
    /// the same package, features, checkout and environment, without the scope and `nice`.
    fn cargo(&self, verb: Verb) -> Result<Command, String> {
        let root = registry::repo_root().ok_or_else(|| format!("no makepad checkout to run the {} from", self.kind.label))?;
        let target = target_of(self.kind)?;
        let (cargo, cargo_args) = match verb {
            Verb::Run => registry::run_argv_features(&target, &root, self.conf.features(), &[]),
            Verb::Build => registry::build_argv_features(&target, &root, self.conf.features()),
        };
        let mut argv: Vec<std::ffi::OsString> = Vec::new();
        #[cfg(target_os = "linux")]
        if let (Verb::Run, Some(run)) = (verb, user_scope()) {
            let unit = format!("makepad-{}-{}-{}", self.kind.file, self.conf.port(), std::process::id());
            argv.extend([run.into_os_string(), "--user".into(), "--scope".into(), "--quiet".into(), "--collect".into(), format!("--unit={unit}").into()]);
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
        if id == identity(binary, &self.conf) {
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
        let mut cmd = match self.command() {
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

    /// A start that failed (not built yet, say) is tried again like an exit.
    fn retry_later(&mut self, now: f64, reason: String) {
        let label = self.kind.label;
        self.crashes += 1;
        let wait = backoff_s(self.crashes);
        makepad_widgets::log!("wm: {label} did not start ({reason}); trying again in {wait} s");
        self.restart_at = Some(now + wait as f64);
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
        let exited = match self.running.as_mut() {
            Some(Running::Child(child)) => match child.try_wait() {
                Ok(Some(status)) => Some(status.to_string()),
                Ok(None) => None,
                Err(e) => {
                    makepad_widgets::log!("wm: {label}: {e}");
                    None
                }
            },
            Some(Running::Adopted(pid)) => (!app_alive(self.kind, *pid)).then(|| "exited".to_string()),
            None => None,
        };
        if let Some(status) = exited {
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
                let wait = backoff_s(self.crashes);
                makepad_widgets::log!("wm: {label} exited ({}); restart {} in {wait} s", self.exit_reason(&status), self.crashes);
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

    #[test]
    fn devhub_starts_and_builds_with_one_cargo_command() {
        // The WM's start and the deploy's build (`makepad-wm-build -- devhub`) are the same
        // cargo invocation with `run` for `build`: a start after the deploy compiles nothing.
        if registry::repo_root().is_none() {
            return;
        }
        let dir = scratch("devhub");
        let conf = makepad_devhub::conf::NodeConf::parse("name = t1\nport = 8797\n");
        let d = DevhubApp::new(&DEVHUB, dir.join("devhub.conf"), conf);
        let args = |verb| -> (Vec<String>, Vec<(String, String)>) {
            let cmd = d.cargo(verb).unwrap();
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
        assert!(run_env.iter().any(|(k, v)| k == makepad_devhub::conf::NODE_CONF_ENV && v.ends_with("devhub.conf")));
        assert_eq!(d.state.file_name().unwrap(), "devhub.state");
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
