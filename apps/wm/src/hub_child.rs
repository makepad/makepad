//! The AI hub as the window manager's own child. The WM starts the built hub binary (the
//! deploy builds it, with the settings' environment: a start takes seconds) and keeps it
//! running:
//!
//! - a hub that exits on its own is started again, after a pause that doubles with each
//!   exit in a row (2 s .. 5 min) and starts over once a hub stayed up for two minutes;
//! - the panel's model pick saves the choice and restarts the hub with it;
//! - a WM restart does not cost the hub (Unix). It runs outside the WM's life: its output
//!   goes to its log file, on Linux under systemd in a scope of the user's manager (the WM's
//!   unit stops its whole control group), and `~/.makepad/wm/ai-hub.state` names its pid and
//!   what it was started as. The next WM keeps a live hub with the same binary and settings
//!   (its models stay loaded, its jobs keep running); one started from another binary or
//!   other settings is replaced.
//! - a stop is SIGTERM: the hub closes admission ("restarting", so the fleet routes jobs to
//!   other nodes), hands its running jobs back to their clients to reroute, and exits.
//!   Windows: the hub ends with the WM (its job object) as before.
//!
//! Its settings are `~/.makepad/wm/ai-hub.conf` (`MAKEPAD_WM_AI_HUB_CONF` names another
//! file), `key = value` lines, every key optional:
//!
//! ```text
//! enabled = on                    # off: the WM starts no hub
//! port = 8123
//! host = 0.0.0.0                  # 0.0.0.0: the fleet TLS front for the LAN; 127.0.0.1: http, this machine only
//! cache_dir = /home/arch/.makepad/weights-chat   # default: the hub's own (~/.makepad/weights)
//! model = qwen3.8-27b             # the chat model loaded at start and pinned; apps the WM starts use it
//! context = 32768                 # the chat context per lane at start (the panel changes it live)
//! context_max = 81920
//! kv = q8_0
//! pin = kokoro,whisper-large-v3-turbo   # models kept loaded besides the chat model
//! nice = 10                       # the build's and the hub's scheduling priority (Unix)
//! compute = auto                  # compute jobs: auto (a game pauses them) | ui-first | ai-first | unavailable
//! models = all                    # keep every model this node can run in its cache (or a list of ids)
//! models_licences = none,community   # licence classes `models` fetches (+ non-commercial, restricted, gated)
//! models_mbps = 60                # its download pacing, MB/s
//! target_dir = /home/arch/makepad-target-hub     # its own cargo target (a CUDA build env differs from the apps')
//! env MAKEPAD_CUDA_ROOT = /opt/cuda              # any other variable for the build and the hub
//! ```
//!
//! Without `env MAKEPAD_CUDA_ROOT` the toolkit installed here is named for the build
//! (`/opt/cuda`, `/usr/local/cuda`, Windows' `CUDA_PATH`): without it the hub's CUDA
//! kernels are not built and it serves no GPU model.
//!
//! A hidden WM (`MAKEPAD_HIDE_WINDOWS`, the test runs) starts no hub unless
//! `MAKEPAD_WM_AI_HUB_CONF` names its settings: test desktops must not take the GPU or the
//! port of the real one. The hub's output goes to `~/.makepad/wm/ai-hub.log`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use makepad_app_registry as registry;

pub use crate::hub_control::{HubPhase, HubProcess};
pub use makepad_ai_hub::activity::ComputeMode;

#[derive(Clone, Debug, PartialEq)]
pub struct HubSettings {
    pub enabled: bool,
    pub port: u16,
    pub host: String,
    pub cache_dir: Option<String>,
    pub model: String,
    pub context: Option<u32>,
    pub context_max: Option<u32>,
    pub kv: Option<String>,
    pub pin: Vec<String>,
    pub nice: i32,
    /// Whether this node takes compute jobs (the panel's picker; `local_use`).
    pub compute: ComputeMode,
    pub target_dir: Option<String>,
    /// Prefetch (`MAKEPAD_AI_PREFETCH`): `all`, or model ids; None = off.
    pub models: Option<String>,
    pub models_licences: Option<String>,
    pub models_mbps: Option<String>,
    pub env: Vec<(String, String)>,
}

impl Default for HubSettings {
    fn default() -> Self {
        HubSettings {
            enabled: true,
            port: 8123,
            host: "0.0.0.0".to_string(),
            cache_dir: None,
            model: String::new(),
            context: None,
            context_max: None,
            kv: None,
            pin: Vec::new(),
            nice: 10,
            compute: ComputeMode::Automatic,
            target_dir: None,
            models: None,
            models_licences: None,
            models_mbps: None,
            env: Vec::new(),
        }
    }
}

const CONF_ENV: &str = "MAKEPAD_WM_AI_HUB_CONF";

impl HubSettings {
    pub fn path() -> PathBuf {
        match std::env::var_os(CONF_ENV).filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => crate::theme::makepad_home().join("wm/ai-hub.conf"),
        }
    }

    /// The settings file; a missing one is the defaults. A hidden WM without its own
    /// settings file starts no hub.
    pub fn load() -> HubSettings {
        let mut s = std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default();
        if std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some() && std::env::var_os(CONF_ENV).is_none() {
            s.enabled = false;
        }
        s
    }

    /// The settings file as it is now (edited since the start, maybe), else `current`.
    pub fn load_or(current: &HubSettings) -> HubSettings {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_else(|_| current.clone())
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, self.render()).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Unknown keys and bad values are skipped (and said on the log): a typo never stops
    /// the hub from starting with the rest.
    pub fn parse(text: &str) -> HubSettings {
        let mut s = HubSettings::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            let ok = match key {
                "enabled" => match value {
                    "on" | "true" | "yes" | "1" => { s.enabled = true; true }
                    "off" | "false" | "no" | "0" => { s.enabled = false; true }
                    _ => false,
                },
                "port" => value.parse().map(|p| s.port = p).is_ok(),
                "host" => { s.host = value.to_string(); !value.is_empty() }
                "cache_dir" => { s.cache_dir = Some(value.to_string()).filter(|v| !v.is_empty()); true }
                "model" => { s.model = value.to_string(); true }
                "context" => value.parse().map(|c| s.context = Some(c)).is_ok(),
                "context_max" => value.parse().map(|c| s.context_max = Some(c)).is_ok(),
                "kv" => { s.kv = Some(value.to_string()).filter(|v| !v.is_empty()); true }
                "pin" => { s.pin = value.split(',').map(str::trim).filter(|m| !m.is_empty()).map(str::to_string).collect(); true }
                "nice" => value.parse().map(|n| s.nice = n).is_ok(),
                "compute" => ComputeMode::parse(value).map(|m| s.compute = m).is_some(),
                "target_dir" => { s.target_dir = Some(value.to_string()).filter(|v| !v.is_empty()); true }
                "models" => { s.models = Some(value.to_string()).filter(|v| !v.is_empty()); true }
                "models_licences" => { s.models_licences = Some(value.to_string()).filter(|v| !v.is_empty()); true }
                "models_mbps" => value.parse::<f64>().map(|_| s.models_mbps = Some(value.to_string())).is_ok(),
                _ => match key.strip_prefix("env ").map(str::trim) {
                    Some(var) if !var.is_empty() && !var.contains(char::is_whitespace) => {
                        s.env.push((var.to_string(), value.to_string()));
                        true
                    }
                    _ => false,
                },
            };
            if !ok {
                makepad_widgets::log!("wm: ai-hub.conf: skipped {line:?}");
            }
        }
        s
    }

    pub fn render(&self) -> String {
        let mut out = String::from("# The AI hub the window manager starts (apps/wm/src/hub_child.rs).\n");
        out.push_str(&format!("enabled = {}\n", if self.enabled { "on" } else { "off" }));
        out.push_str(&format!("port = {}\nhost = {}\n", self.port, self.host));
        if let Some(dir) = &self.cache_dir {
            out.push_str(&format!("cache_dir = {dir}\n"));
        }
        out.push_str(&format!("model = {}\n", self.model));
        if let Some(c) = self.context {
            out.push_str(&format!("context = {c}\n"));
        }
        if let Some(c) = self.context_max {
            out.push_str(&format!("context_max = {c}\n"));
        }
        if let Some(kv) = &self.kv {
            out.push_str(&format!("kv = {kv}\n"));
        }
        if !self.pin.is_empty() {
            out.push_str(&format!("pin = {}\n", self.pin.join(",")));
        }
        out.push_str(&format!("nice = {}\n", self.nice));
        out.push_str(&format!("compute = {}\n", self.compute.name()));
        if let Some(dir) = &self.target_dir {
            out.push_str(&format!("target_dir = {dir}\n"));
        }
        for (key, value) in [("models", &self.models), ("models_licences", &self.models_licences), ("models_mbps", &self.models_mbps)] {
            if let Some(value) = value {
                out.push_str(&format!("{key} = {value}\n"));
            }
        }
        for (k, v) in &self.env {
            out.push_str(&format!("env {k} = {v}\n"));
        }
        out
    }

    /// Where the WM's control client reaches it: the fleet TLS front unless it listens
    /// on loopback only (the hub's own rule, apps/ai-hub).
    pub fn base_url(&self) -> String {
        let loopback = self.host == "127.0.0.1" || self.host == "localhost" || self.host == "::1";
        format!("{}://127.0.0.1:{}", if loopback { "http" } else { "https" }, self.port)
    }

    /// The hub's command line after its binary.
    pub fn hub_args(&self) -> Vec<String> {
        let mut args = vec!["--host".to_string(), self.host.clone(), "--port".to_string(), self.port.to_string()];
        if let Some(dir) = &self.cache_dir {
            args.extend(["--cache-dir".to_string(), dir.clone()]);
        }
        args
    }

    /// The environment the settings give the build and the hub, in order (later wins).
    pub fn hub_env(&self) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if !self.model.is_empty() {
            env.push(("MAKEPAD_AI_MODEL".to_string(), self.model.clone()));
        }
        let pins: Vec<&str> = std::iter::once(self.model.as_str()).chain(self.pin.iter().map(String::as_str)).filter(|m| !m.is_empty()).collect();
        if !pins.is_empty() {
            env.push(("MAKEPAD_AI_PIN_MODELS".to_string(), pins.join(",")));
        }
        if let Some(c) = self.context {
            env.push(("MAKEPAD_ASSET_AI_LLM_CONTEXT".to_string(), c.to_string()));
        }
        if let Some(c) = self.context_max {
            env.push(("MAKEPAD_ASSET_AI_LLM_CONTEXT_MAX".to_string(), c.to_string()));
        }
        if let Some(kv) = &self.kv {
            env.push(("MAKEPAD_ASSET_AI_LLM_KV".to_string(), kv.clone()));
        }
        if let Some(dir) = &self.target_dir {
            env.push(("CARGO_TARGET_DIR".to_string(), dir.clone()));
        }
        for (key, value) in [("MAKEPAD_AI_PREFETCH", &self.models), ("MAKEPAD_AI_PREFETCH_LICENCES", &self.models_licences), ("MAKEPAD_AI_PREFETCH_MBPS", &self.models_mbps)] {
            if let Some(value) = value {
                env.push((key.to_string(), value.clone()));
            }
        }
        env.extend(self.env.iter().cloned());
        // The hub's GPU models need its CUDA kernels, and makepad-ai-cuda builds them only
        // when MAKEPAD_CUDA_ROOT names the toolkit: a hub built without it serves no GPU
        // model at all ("needs a CUDA device"). The WM's session environment never has it,
        // so the toolkit installed on this machine is named unless the settings say otherwise.
        let explicit = env.iter().any(|(k, _)| k == CUDA_ROOT) || std::env::var_os(CUDA_ROOT).is_some();
        if !explicit {
            if let Some(root) = installed_cuda_root(&cuda_root_candidates()) {
                env.push((CUDA_ROOT.to_string(), root.to_string_lossy().into_owned()));
            }
        }
        env
    }
}

const CUDA_ROOT: &str = "MAKEPAD_CUDA_ROOT";

/// Where a CUDA toolkit is usually installed: the distribution's (/opt/cuda, Arch) or
/// NVIDIA's (/usr/local/cuda); on Windows the installer's CUDA_PATH.
fn cuda_root_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = std::env::var_os("CUDA_PATH").filter(|p| !p.is_empty()) {
        out.push(PathBuf::from(path));
    }
    if cfg!(target_os = "linux") {
        out.push(PathBuf::from("/opt/cuda"));
        out.push(PathBuf::from("/usr/local/cuda"));
    }
    out
}

/// The first candidate that holds nvcc.
fn installed_cuda_root(candidates: &[PathBuf]) -> Option<PathBuf> {
    let nvcc = if cfg!(windows) { "nvcc.exe" } else { "nvcc" };
    candidates.iter().find(|root| root.join("bin").join(nvcc).is_file()).cloned()
}

/// Seconds before the `n`th restart in a row (n >= 1).
pub fn backoff_s(n: u32) -> u32 {
    (1u32 << n.min(9)).min(300)
}

/// Seconds after asking for the chat model's load by which the hub has reported it
/// loading (it is polled every 3 s).
const LOAD_SEEN_S: f64 = 10.0;

/// A hub that stayed up this long starts the crash count over.
const STEADY_S: f64 = 120.0;
const TAIL: usize = 24;

/// The hub process this WM watches: its own child, or one an earlier WM started and this
/// one adopted (`ai-hub.state`).
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

/// What a hub was started as: its binary (path, size, modification time) and everything the
/// settings hand it. A WM that finds a live hub with the same identity keeps it.
fn identity(bin: &Path, settings: &HubSettings) -> String {
    let meta = std::fs::metadata(bin).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let modified = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    format!("{} {size} {modified} nice={} args={:?} env={:?}", bin.display(), settings.nice, settings.hub_args(), settings.hub_env())
}

/// `ai-hub.state`: the pid of the hub this WM (or an earlier one) started and its identity.
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

/// Whether `pid` is still a hub: alive, and (Linux) its command line names the hub binary, so a
/// reused pid is never taken for it.
fn hub_alive(pid: u32) -> bool {
    if !makepad_app_host::launch::process_alive(pid) {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let entry = registry::find("ai-hub");
        return std::fs::read(format!("/proc/{pid}/cmdline"))
            .is_ok_and(|cmd| entry.is_some_and(|e| cmd.windows(e.bin.len()).any(|w| w == e.bin.as_bytes())));
    }
    #[cfg(not(target_os = "linux"))]
    true
}

/// Linux under systemd: the WM's own unit stops its whole control group, so a hub that is to
/// outlive a WM restart runs in a scope of the user's manager (`systemd-run --user --scope`,
/// which execs it: the child's pid is the hub's).
#[cfg(target_os = "linux")]
fn user_scope() -> Option<PathBuf> {
    let run = PathBuf::from("/usr/bin/systemd-run");
    let bus = std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("bus"));
    (run.is_file() && bus.is_some_and(|b| b.exists())).then_some(run)
}

pub struct HubChild {
    pub settings: HubSettings,
    running: Option<Running>,
    phase: HubPhase,
    /// Its output's log (`~/.makepad/wm/ai-hub.log`): the reason of an exit is in its tail.
    log: PathBuf,
    /// `~/.makepad/wm/ai-hub.state`.
    state: PathBuf,
    /// Exits in a row without a steady run between them.
    crashes: u32,
    restart_at: Option<f64>,
    /// Stopped on purpose: start again as soon as it is gone.
    restart_pending: bool,
    /// The first start looks for a hub an earlier WM left running.
    first_start: bool,
    /// An adopted hub being replaced gets SIGKILL at this time if it is still there.
    kill_at: Option<f64>,
    started_at: f64,
    ready_at: Option<f64>,
    /// When the chat model's load was asked for (None: not yet).
    load_sent: Option<f64>,
}

impl HubChild {
    pub fn new(settings: HubSettings) -> HubChild {
        let wm = crate::theme::makepad_home().join("wm");
        HubChild {
            settings,
            running: None,
            phase: HubPhase::Starting,
            log: wm.join("ai-hub.log"),
            state: wm.join("ai-hub.state"),
            crashes: 0,
            restart_at: Some(0.0),
            restart_pending: false,
            first_start: true,
            kill_at: None,
            started_at: 0.0,
            ready_at: None,
            load_sent: None,
        }
    }

    pub fn process(&self) -> HubProcess {
        HubProcess { phase: self.phase.clone(), model: self.settings.model.clone() }
    }

    /// The built hub: the deploy builds it (with the settings' environment, so its CUDA
    /// kernels are in). Starting it takes seconds; a `cargo run` checked and relinked it for
    /// a minute and a half on every WM start.
    fn binary(&self) -> Result<PathBuf, String> {
        let entry = registry::find("ai-hub").ok_or("the registry has no ai-hub")?;
        let target = entry.target();
        let root = registry::repo_root();
        let bin = match (&self.settings.target_dir, &root) {
            (Some(dir), Some(root)) => Some(registry::checkout_binary(&target, root, Some(std::ffi::OsStr::new(dir)))),
            _ => registry::built_binary(&target, root.as_deref()),
        };
        bin.filter(|b| b.is_file())
            .ok_or_else(|| "the AI hub is not built (the deploy builds it: cargo build --release -p makepad-app-ai-hub)".to_string())
    }

    fn command(&self, bin: &Path) -> Command {
        let mut argv: Vec<std::ffi::OsString> = Vec::new();
        #[cfg(target_os = "linux")]
        if let Some(run) = user_scope() {
            let unit = format!("makepad-ai-hub-{}-{}", self.settings.port, std::process::id());
            argv.extend([run.into_os_string(), "--user".into(), "--scope".into(), "--quiet".into(), "--collect".into(), format!("--unit={unit}").into()]);
        }
        if cfg!(unix) && self.settings.nice > 0 {
            argv.extend(["nice".into(), "-n".into(), self.settings.nice.to_string().into()]);
        }
        argv.push(bin.as_os_str().to_owned());
        argv.extend(self.settings.hub_args().into_iter().map(Into::into));
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]);
        registry::child_env::scrub(&mut cmd);
        if let Some(root) = registry::repo_root() {
            cmd.current_dir(root);
        }
        for (k, v) in self.settings.hub_env() {
            cmd.env(k, v);
        }
        cmd
    }

    /// The first start: a live hub an earlier WM started with this identity is kept (a WM
    /// restart no longer costs the hub, its loaded models or its running jobs). One started
    /// from another binary or other settings is stopped, and a new one starts once it is gone.
    fn adopt_or_replace(&mut self, now: f64, bin: &Path) -> bool {
        let Some((pid, id)) = read_state(&self.state) else { return false };
        if !hub_alive(pid) {
            return false;
        }
        if id == identity(bin, &self.settings) {
            makepad_widgets::log!("wm: AI hub: kept pid {pid}, started by an earlier WM with this binary and these settings");
            self.running = Some(Running::Adopted(pid));
            self.restart_at = None;
            self.phase = HubPhase::Starting;
            self.started_at = now;
            // Its models are its own by now: no start-up load.
            self.load_sent = Some(f64::NEG_INFINITY);
            return true;
        }
        makepad_widgets::log!("wm: AI hub: pid {pid} runs another binary or other settings; replacing it");
        #[cfg(unix)]
        makepad_app_host::launch::signal_group(pid, false);
        self.kill_at = Some(now + STOP_GRACE.as_secs_f64());
        self.running = Some(Running::Adopted(pid));
        self.restart_pending = true;
        self.phase = HubPhase::Restarting;
        true
    }

    fn spawn(&mut self, now: f64) {
        self.restart_at = None;
        self.restart_pending = false;
        self.load_sent = None;
        self.ready_at = None;
        let bin = match self.binary() {
            Ok(bin) => bin,
            Err(reason) => {
                makepad_widgets::log!("wm: AI hub: {reason}");
                self.phase = HubPhase::StartFailed(reason);
                return;
            }
        };
        if std::mem::take(&mut self.first_start) && self.adopt_or_replace(now, &bin) {
            return;
        }
        let mut cmd = self.command(&bin);
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
                makepad_widgets::log!("wm: AI hub: {reason}");
                self.phase = HubPhase::StartFailed(reason);
                return;
            }
        };
        // Windows: no re-adoption; the hub ends with the WM's job object.
        #[cfg(windows)]
        makepad_app_host::launch::exit_with_parent(&child);
        #[cfg(unix)]
        if let Err(e) = std::fs::write(&self.state, format!("pid = {}\nidentity = {}\n", child.id(), identity(&bin, &self.settings))) {
            makepad_widgets::log!("wm: AI hub: {}: {e}", self.state.display());
        }
        makepad_widgets::log!(
            "wm: AI hub: started pid {} ({}, port {}{})",
            child.id(),
            bin.display(),
            self.settings.port,
            if self.settings.model.is_empty() { String::new() } else { format!(", model {}", self.settings.model) }
        );
        self.running = Some(Running::Child(child));
        self.phase = HubPhase::Starting;
        self.started_at = now;
    }

    /// The last lines of its log.
    fn tail(&self) -> Vec<String> {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        lines[lines.len().saturating_sub(TAIL)..].to_vec()
    }

    /// Why it ended: its exit status, with the hub's last error or panic line when it said one.
    fn exit_reason(&self, status: &str) -> String {
        let tail = self.tail();
        let said = tail.iter().rev().find(|l| l.contains("panicked") || l.contains("error") || l.contains("Error"));
        match said {
            Some(line) => format!("{status}: {line}"),
            None => status.to_string(),
        }
    }

    /// Once a second from the WM's tick: exit, restart, the start's progress.
    /// `answers` = the control client reached it; `model` = the chat model's state there.
    /// True when the chat model should be asked to load now.
    pub fn tick(&mut self, now: f64, answers: bool, model: Option<crate::hub_control::ModelState>) -> bool {
        use crate::hub_control::ModelState;
        if let (Some(Running::Adopted(pid)), Some(at)) = (&self.running, self.kill_at) {
            if now >= at && hub_alive(*pid) {
                makepad_widgets::log!("wm: AI hub: pid {pid} outlived its stop; SIGKILL");
                #[cfg(unix)]
                makepad_app_host::launch::signal_group(*pid, true);
                self.kill_at = None;
            }
        }
        let exited = match self.running.as_mut() {
            Some(Running::Child(child)) => match child.try_wait() {
                Ok(Some(status)) => Some(status.to_string()),
                Ok(None) => None,
                Err(e) => {
                    makepad_widgets::log!("wm: AI hub: {e}");
                    None
                }
            },
            Some(Running::Adopted(pid)) => (!hub_alive(*pid)).then(|| "exited".to_string()),
            None => None,
        };
        if let Some(status) = exited {
            let adopted = matches!(self.running, Some(Running::Adopted(_)));
            self.running = None;
            self.kill_at = None;
            if self.restart_pending {
                makepad_widgets::log!("wm: AI hub stopped ({status}) for a restart");
                self.crashes = 0;
                if adopted {
                    // Not our child: gone from its command line, it may still hold its cache
                    // lock until its own parent reaps it. The new one starts on the next tick.
                    self.restart_pending = false;
                    self.restart_at = Some(now + 1.0);
                } else {
                    self.spawn(now);
                }
            } else {
                if self.ready_at.is_some_and(|t| now - t >= STEADY_S) {
                    self.crashes = 0;
                }
                self.crashes += 1;
                let wait = backoff_s(self.crashes);
                let reason = self.exit_reason(&status);
                makepad_widgets::log!("wm: AI hub exited ({reason}); restart {} in {wait} s", self.crashes);
                self.restart_at = Some(now + wait as f64);
                self.phase = HubPhase::Crashed { reason, restart_in: wait };
            }
            return false;
        }
        if self.running.is_none() {
            match self.restart_at {
                Some(at) if now >= at => self.spawn(now),
                Some(at) => {
                    if let HubPhase::Crashed { restart_in, .. } = &mut self.phase {
                        *restart_in = (at - now).ceil().max(0.0) as u32;
                    }
                }
                None => {}
            }
            return false;
        }
        if self.restart_pending {
            return false;
        }
        if !answers {
            if matches!(self.phase, HubPhase::LoadingModel | HubPhase::Ready | HubPhase::ModelFailed) {
                // It stopped answering while it runs (busy, or about to exit): say so.
                self.phase = HubPhase::Starting;
            }
            return false;
        }
        if self.settings.model.is_empty() {
            self.mark_ready(now);
            return false;
        }
        if self.ready_at.is_some() {
            // After its first load the model's residency is the hub's (a GPU yield may unload it).
            self.phase = HubPhase::Ready;
            return false;
        }
        match (model, self.load_sent) {
            (Some(ModelState::Loaded), _) => {
                self.mark_ready(now);
                false
            }
            (Some(ModelState::Error), Some(_)) => {
                self.phase = HubPhase::ModelFailed;
                false
            }
            (_, None) => {
                self.phase = HubPhase::LoadingModel;
                self.load_sent = Some(now);
                true
            }
            (Some(ModelState::Loading | ModelState::Downloading), Some(_)) => {
                self.phase = HubPhase::LoadingModel;
                false
            }
            // Asked a while ago (the hub reports a load within its next poll) and it is
            // neither loading nor loaded: the load ended another way (a job that needed
            // the GPU took it, say). The hub is up: Ready, not "Loading" forever.
            (_, Some(at)) if now - at >= LOAD_SEEN_S => {
                self.mark_ready(now);
                false
            }
            (_, Some(_)) => {
                self.phase = HubPhase::LoadingModel;
                false
            }
        }
    }

    fn mark_ready(&mut self, now: f64) {
        if self.ready_at.is_none() {
            makepad_widgets::log!("wm: AI hub ready after {:.0} s", now - self.started_at);
            self.ready_at = Some(now);
        }
        self.phase = HubPhase::Ready;
    }

    /// SIGTERM to its process group: the hub closes admission ("restarting", so the fleet
    /// routes elsewhere), hands its running jobs back to their clients, then exits; SIGKILL
    /// after the grace.
    fn stop_pid(&self, pid: u32, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        #[cfg(unix)]
        makepad_app_host::launch::kill_group_pid(pid, STOP_GRACE, pool);
        #[cfg(not(unix))]
        let _ = (pid, pool);
    }

    /// Stop it and start it again (with `settings` from now on). A failed start or a
    /// waiting restart starts at once.
    pub fn restart(&mut self, settings: HubSettings, now: f64, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        self.settings = settings;
        self.crashes = 0;
        match self.running.as_mut() {
            Some(running) => {
                makepad_widgets::log!("wm: AI hub: restarting pid {}", running.pid());
                match running {
                    Running::Child(child) => makepad_app_host::launch::kill_child_group(child, STOP_GRACE, pool),
                    Running::Adopted(pid) => {
                        let pid = *pid;
                        self.stop_pid(pid, pool);
                    }
                }
                self.restart_pending = true;
                self.phase = HubPhase::Restarting;
            }
            None => self.spawn(now),
        }
    }

    /// The WM quits. Unix: the hub keeps running, and the next WM keeps it. Windows: it ends
    /// with the WM.
    pub fn stop(&mut self, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        self.restart_at = None;
        self.restart_pending = false;
        let Some(running) = self.running.as_mut() else { return };
        if cfg!(unix) {
            makepad_widgets::log!("wm: the AI hub (pid {}) keeps running for the next WM", running.pid());
        } else if let Running::Child(child) = running {
            makepad_widgets::log!("wm: shutdown stops the AI hub (pid {})", child.id());
            makepad_app_host::launch::kill_child_group(child, STOP_GRACE, pool);
        }
    }
}

/// The WM's hub is off (`enabled = off`): one an earlier WM left running stops.
pub fn stop_leftover(pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
    let state = crate::theme::makepad_home().join("wm/ai-hub.state");
    if let Some((pid, _)) = read_state(&state).filter(|(pid, _)| hub_alive(*pid)) {
        makepad_widgets::log!("wm: the AI hub is off: stopping pid {pid}, left by an earlier WM");
        #[cfg(unix)]
        makepad_app_host::launch::kill_group_pid(pid, STOP_GRACE, pool);
        #[cfg(not(unix))]
        let _ = pool;
    }
}

/// From SIGTERM to SIGKILL when stopping it: the hub drains (admission closed, running jobs
/// handed back) for up to 3 s at SIGTERM, answers their status for 2 s more, then exits.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(7);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_control::ModelState;

    #[test]
    fn the_installed_cuda_toolkit_is_found_by_its_nvcc() {
        let base = std::env::temp_dir().join(format!("wm-cuda-{}", std::process::id()));
        let (none, found) = (base.join("none"), base.join("cuda"));
        let nvcc = if cfg!(windows) { "nvcc.exe" } else { "nvcc" };
        std::fs::create_dir_all(found.join("bin")).unwrap();
        std::fs::create_dir_all(none.join("bin")).unwrap();
        std::fs::write(found.join("bin").join(nvcc), b"").unwrap();
        assert_eq!(installed_cuda_root(&[none.clone(), found.clone()]), Some(found.clone()));
        assert_eq!(installed_cuda_root(&[none.clone()]), None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn settings_read_what_the_service_ran_with_and_write_back_the_same() {
        let text = "port = 8123\nhost = 0.0.0.0\ncache_dir = /home/arch/.makepad/weights-chat\nmodel = qwen3.8-27b # the 27B\n\
                    context = 32768\ncontext_max = 81920\nkv = q8_0\npin = kokoro, whisper-large-v3-turbo\nnice = 10\ncompute = ai-first\n\
                    target_dir = /home/arch/makepad-target-hub\nmodels = all\nmodels_licences = none,community,non-commercial\nmodels_mbps = 40\nenv MAKEPAD_CUDA_ROOT = /opt/cuda\nenv MAKEPAD_AI_UI_RESERVE_MB = 1536\nbogus = 1\n";
        let s = HubSettings::parse(text);
        assert!(s.enabled);
        assert_eq!(s.compute, ComputeMode::AiFirst);
        assert_eq!(s.hub_args(), ["--host", "0.0.0.0", "--port", "8123", "--cache-dir", "/home/arch/.makepad/weights-chat"]);
        assert_eq!(s.base_url(), "https://127.0.0.1:8123");
        let env = s.hub_env();
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("MAKEPAD_AI_MODEL"), Some("qwen3.8-27b"));
        assert_eq!(get("MAKEPAD_AI_PIN_MODELS"), Some("qwen3.8-27b,kokoro,whisper-large-v3-turbo"));
        assert_eq!((get("MAKEPAD_ASSET_AI_LLM_CONTEXT"), get("MAKEPAD_ASSET_AI_LLM_CONTEXT_MAX"), get("MAKEPAD_ASSET_AI_LLM_KV")), (Some("32768"), Some("81920"), Some("q8_0")));
        assert_eq!((get("CARGO_TARGET_DIR"), get("MAKEPAD_CUDA_ROOT"), get("MAKEPAD_AI_UI_RESERVE_MB")), (Some("/home/arch/makepad-target-hub"), Some("/opt/cuda"), Some("1536")));
        assert_eq!((get("MAKEPAD_AI_PREFETCH"), get("MAKEPAD_AI_PREFETCH_LICENCES"), get("MAKEPAD_AI_PREFETCH_MBPS")), (Some("all"), Some("none,community,non-commercial"), Some("40")));
        assert_eq!(HubSettings::parse(&s.render()), s);
        // Nothing set: the defaults, loopback http when asked for.
        let d = HubSettings::parse("host = 127.0.0.1\nenabled = off\n");
        assert!(!d.enabled);
        assert_eq!(d.compute, ComputeMode::Automatic);
        assert_eq!(d.base_url(), "http://127.0.0.1:8123");
        // (Only the installed CUDA toolkit, on a machine that has one.)
        assert!(d.hub_env().iter().all(|(k, _)| k == CUDA_ROOT));
        // The conf's own CUDA root is the one passed, once.
        assert_eq!(env.iter().filter(|(k, _)| k == CUDA_ROOT).count(), 1);
        assert_eq!(d.hub_args(), ["--host", "127.0.0.1", "--port", "8123"]);
    }

    #[test]
    fn restarts_back_off_and_cap() {
        assert_eq!((1..=10).map(backoff_s).collect::<Vec<_>>(), [2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wm-hub-child-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn running(model: &str) -> HubChild {
        let mut h = HubChild::new(HubSettings { model: model.into(), ..Default::default() });
        let dir = scratch(model);
        h.log = dir.join("ai-hub.log");
        h.state = dir.join("ai-hub.state");
        // As if spawned: the phases after the process runs.
        h.restart_at = None;
        h.first_start = false;
        h.phase = HubPhase::Starting;
        h.running = Some(Running::Child(Command::new("sleep").arg("30").spawn().unwrap()));
        h
    }

    fn end(h: &mut HubChild) {
        if let Some(Running::Child(mut c)) = h.running.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    #[test]
    fn a_start_goes_starting_loading_ready_and_asks_for_the_model_once() {
        let mut h = running("q");
        assert!(!h.tick(1.0, false, None));
        assert_eq!(h.phase, HubPhase::Starting);
        assert!(h.tick(2.0, true, Some(ModelState::Unloaded)), "answers: load the model");
        assert_eq!(h.phase, HubPhase::LoadingModel);
        assert!(!h.tick(3.0, true, Some(ModelState::Loading)), "asked once");
        assert!(!h.tick(4.0, true, Some(ModelState::Loaded)));
        assert_eq!(h.phase, HubPhase::Ready);
        assert!(!h.tick(5.0, true, Some(ModelState::Unloaded)), "a yield's unload is the hub's business");
        assert_eq!(h.phase, HubPhase::Ready);
        end(&mut h);
    }

    #[test]
    fn a_load_taken_over_by_another_job_ends_in_ready_not_loading() {
        // 149: the 27B's start-up load gave way to a music job; the model stays unloaded
        let mut h = running("r");
        assert!(h.tick(1.0, true, Some(ModelState::Unloaded)));
        assert!(!h.tick(4.0, true, Some(ModelState::Loading)));
        assert_eq!(h.phase, HubPhase::LoadingModel);
        h.tick(7.0, true, Some(ModelState::Unloaded));
        assert_eq!(h.phase, HubPhase::LoadingModel, "not yet: the hub may not have said so");
        h.tick(12.0, true, Some(ModelState::Unloaded));
        assert_eq!(h.phase, HubPhase::Ready);
        end(&mut h);
    }

    #[test]
    fn an_exit_is_a_crash_with_its_reason_from_the_log_and_a_timed_restart() {
        let mut h = running("s");
        end(&mut h);
        std::fs::write(&h.log, "listening\nthread 'main' panicked at server.rs:1: no such hub flag\n").unwrap();
        let mut child = Command::new("false").spawn().unwrap();
        let _ = child.wait();
        h.running = Some(Running::Child(child));
        h.tick(10.0, false, None);
        match &h.phase {
            HubPhase::Crashed { reason, restart_in } => {
                assert!(reason.contains("panicked") && reason.contains("no such hub flag"), "its last error: {reason}");
                assert_eq!(*restart_in, 2);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(h.restart_at, Some(12.0));
        h.tick(11.0, false, None);
        assert_eq!(h.phase.headline("s"), "Crashed, restarting in 1 s");
    }

    #[test]
    fn the_state_file_names_the_hub_and_its_identity_follows_binary_and_settings() {
        let dir = scratch("state");
        let state = dir.join("ai-hub.state");
        std::fs::write(&state, "pid = 4242\nidentity = /x/hub 10 20 nice=10 args=[] env=[]\n").unwrap();
        assert_eq!(read_state(&state), Some((4242, "/x/hub 10 20 nice=10 args=[] env=[]".to_string())));
        std::fs::write(&state, "pid = nope\n").unwrap();
        assert_eq!(read_state(&state), None);
        let bin = dir.join("hub-bin");
        std::fs::write(&bin, b"one").unwrap();
        let settings = HubSettings::default();
        let first = identity(&bin, &settings);
        assert_eq!(first, identity(&bin, &settings), "the same hub is kept");
        let other = HubSettings { model: "qwen3.8-27b".into(), ..Default::default() };
        assert_ne!(first, identity(&bin, &other), "other settings replace it");
        std::fs::write(&bin, b"rebuilt").unwrap();
        assert_ne!(first, identity(&bin, &settings), "a new build replaces it");
        // Adoption needs a live hub: a gone pid is never taken.
        let mut h = HubChild::new(settings);
        h.state = state.clone();
        std::fs::write(&state, format!("pid = {}\nidentity = {}\n", u32::MAX - 7, identity(&bin, &h.settings))).unwrap();
        assert!(!h.adopt_or_replace(0.0, &bin));
        assert!(h.running.is_none());
    }

    #[test]
    fn an_adopted_hub_is_watched_by_pid_and_needs_no_start_up_load() {
        let mut h = running("t");
        end(&mut h);
        let mut sleeper = Command::new("sleep").arg("30").spawn().unwrap();
        h.running = Some(Running::Adopted(sleeper.id()));
        h.load_sent = Some(f64::NEG_INFINITY);
        // (hub_alive checks the command line on Linux: a sleep is not a hub, so it reads as
        // gone there; elsewhere it is alive.)
        let gone_here = cfg!(target_os = "linux");
        let asked = h.tick(1.0, true, Some(ModelState::Unloaded));
        assert!(!asked, "an adopted hub's models are its own");
        if gone_here {
            assert!(matches!(h.phase, HubPhase::Crashed { .. }));
        } else {
            assert_eq!(h.phase, HubPhase::Ready);
        }
        let _ = sleeper.kill();
        let _ = sleeper.wait();
    }
}
