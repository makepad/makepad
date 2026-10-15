//! The AI hub as the window manager's own child. The WM starts it when it starts, from
//! the checkout it runs from, through the shared registry launcher (`cargo run --release
//! -p makepad-app-ai-hub`, so it never runs a hub older than its sources; an
//! installation without sources starts the built binary), and keeps it running:
//!
//! - a hub that exits on its own is started again, after a pause that doubles with each
//!   exit in a row (2 s .. 5 min) and starts over once a hub stayed up for two minutes;
//! - a build that fails waits for the panel's Restart (a rebuild would fail the same way);
//! - the panel's model pick saves the choice and restarts the hub with it;
//! - the WM's quit stops it, and so does the WM's end in any other way (the child's parent
//!   death signal on Linux, the WM's job object on Windows). A WM restart (a switch to
//!   another build, "Restart desktop") therefore restarts the hub too, and the chat model
//!   loads again: about a minute for a 27B model.
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
//! target_dir = /home/arch/makepad-target-hub     # its own cargo target (a CUDA build env differs from the apps')
//! env MAKEPAD_CUDA_ROOT = /opt/cuda              # any other variable for the build and the hub
//! ```
//!
//! A hidden WM (`MAKEPAD_HIDE_WINDOWS`, the test runs) starts no hub unless
//! `MAKEPAD_WM_AI_HUB_CONF` names its settings: test desktops must not take the GPU or the
//! port of the real one. The hub's output goes to `~/.makepad/wm/ai-hub.log`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};

use makepad_app_registry::{self as registry, ProcessOutput};

pub use crate::hub_control::{HubPhase, HubProcess};

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
    pub target_dir: Option<String>,
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
            target_dir: None,
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
                "target_dir" => { s.target_dir = Some(value.to_string()).filter(|v| !v.is_empty()); true }
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
        if let Some(dir) = &self.target_dir {
            out.push_str(&format!("target_dir = {dir}\n"));
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
        env.extend(self.env.iter().cloned());
        env
    }
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

enum Line {
    Out(String),
    Closed,
}

pub struct HubChild {
    pub settings: HubSettings,
    child: Option<Child>,
    lines: Option<Receiver<Line>>,
    phase: HubPhase,
    /// Still compiling: cargo has not said `Running` yet.
    building: bool,
    /// The last lines of its output, for the reason of an exit.
    tail: VecDeque<String>,
    /// The first compiler error of a failing build.
    diagnostic: String,
    /// Exits in a row without a steady run between them.
    crashes: u32,
    restart_at: Option<f64>,
    /// Stopped on purpose: start again as soon as it is gone.
    restart_pending: bool,
    started_at: f64,
    ready_at: Option<f64>,
    /// When the chat model's load was asked for (None: not yet).
    load_sent: Option<f64>,
}

impl HubChild {
    pub fn new(settings: HubSettings) -> HubChild {
        HubChild {
            settings,
            child: None,
            lines: None,
            phase: HubPhase::Starting,
            building: false,
            tail: VecDeque::new(),
            diagnostic: String::new(),
            crashes: 0,
            restart_at: Some(0.0),
            restart_pending: false,
            started_at: 0.0,
            ready_at: None,
            load_sent: None,
        }
    }

    pub fn process(&self) -> HubProcess {
        HubProcess { phase: self.phase.clone(), model: self.settings.model.clone() }
    }

    fn log_path() -> PathBuf {
        crate::theme::makepad_home().join("wm/ai-hub.log")
    }

    /// The command that starts it: `cargo run` from a checkout, else the built binary.
    fn command(&self) -> Result<(Command, bool), String> {
        let entry = registry::find("ai-hub").ok_or("the registry has no ai-hub")?;
        let target = entry.target();
        let root = registry::repo_root();
        let args = self.settings.hub_args();
        let from_source = registry::builder_install().is_none() && root.as_deref().is_some_and(|r| registry::has_sources(&target, r));
        let (program, args) = if from_source {
            registry::run_argv(&target, root.as_deref().unwrap_or(Path::new(".")), &args)
        } else {
            let bin = registry::built_binary(&target, root.as_deref()).ok_or("the AI hub is not built, and there are no sources to build it from")?;
            (bin, args)
        };
        let mut cmd = if cfg!(unix) && self.settings.nice > 0 {
            let mut cmd = Command::new("nice");
            cmd.arg("-n").arg(self.settings.nice.to_string()).arg(&program).args(&args);
            cmd
        } else {
            let mut cmd = Command::new(&program);
            cmd.args(&args);
            cmd
        };
        registry::child_env::scrub(&mut cmd);
        if from_source {
            registry::cargo_env(&mut cmd);
        }
        if let Some(root) = &root {
            cmd.current_dir(root);
        }
        for (k, v) in self.settings.hub_env() {
            cmd.env(k, v);
        }
        Ok((cmd, from_source))
    }

    fn spawn(&mut self, now: f64) {
        self.restart_at = None;
        self.restart_pending = false;
        self.load_sent = None;
        self.ready_at = None;
        self.tail.clear();
        self.diagnostic.clear();
        let (mut cmd, building) = match self.command() {
            Ok(c) => c,
            Err(reason) => {
                makepad_widgets::log!("wm: AI hub: {reason}");
                self.phase = HubPhase::BuildFailed(reason);
                return;
            }
        };
        #[cfg(unix)]
        makepad_app_host::launch::own_process_group(&mut cmd);
        #[cfg(target_os = "linux")]
        makepad_app_host::launch::exit_with_parent(&mut cmd);
        crate::host::no_console_window(&mut cmd);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                let reason = format!("could not start {:?}: {e}", cmd.get_program());
                makepad_widgets::log!("wm: AI hub: {reason}");
                self.phase = HubPhase::BuildFailed(reason);
                return;
            }
        };
        #[cfg(windows)]
        makepad_app_host::launch::exit_with_parent(&child);
        makepad_widgets::log!(
            "wm: AI hub: started pid {} ({}, port {}{})",
            child.id(),
            if building { "cargo run --release -p makepad-app-ai-hub" } else { "built binary" },
            self.settings.port,
            if self.settings.model.is_empty() { String::new() } else { format!(", model {}", self.settings.model) }
        );
        let log = Self::log_path();
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let log = std::fs::File::create(&log).ok();
        let (tx, rx) = channel();
        if let Some(out) = child.stdout.take() {
            read_lines(out, log.as_ref().and_then(|f| f.try_clone().ok()), tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            read_lines(err, log, tx);
        }
        self.child = Some(child);
        self.lines = Some(rx);
        self.building = building;
        self.phase = if building { HubPhase::Building(String::new()) } else { HubPhase::Starting };
        self.started_at = now;
    }

    fn take_line(&mut self, text: String) {
        if self.building {
            if text.starts_with("error:") || text.starts_with("error[") || !self.diagnostic.is_empty() && self.diagnostic.lines().count() < 12 {
                self.diagnostic.push_str(&text);
                self.diagnostic.push('\n');
            }
            if text.trim_start().starts_with("Running ") {
                self.building = false;
                self.phase = HubPhase::Starting;
            } else if let Some((stage, _)) = registry::cargo_progress(&text) {
                self.phase = HubPhase::Building(stage);
            }
        }
        if !text.starts_with("Building [") {
            if self.tail.len() == TAIL {
                self.tail.pop_front();
            }
            self.tail.push_back(text);
        }
    }

    /// Why it ended: its exit status, with the hub's last error or panic line when it said one.
    fn exit_reason(&self, status: std::process::ExitStatus) -> String {
        let said = self.tail.iter().rev().find(|l| l.contains("panicked") || l.contains("error") || l.contains("Error"));
        match said {
            Some(line) => format!("{status}: {line}"),
            None => format!("{status}"),
        }
    }

    /// Once a second from the WM's tick: output, exit, restart, the start's progress.
    /// `answers` = the control client reached it; `model` = the chat model's state there.
    /// True when the chat model should be asked to load now.
    pub fn tick(&mut self, now: f64, answers: bool, model: Option<crate::hub_control::ModelState>) -> bool {
        use crate::hub_control::ModelState;
        if let Some(rx) = &self.lines {
            let lines: Vec<Line> = rx.try_iter().collect();
            for line in lines {
                if let Line::Out(text) = line {
                    self.take_line(text);
                }
            }
        }
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    self.child = None;
                    // Its last words first.
                    if let Some(rx) = self.lines.take() {
                        for line in rx.try_iter() {
                            if let Line::Out(text) = line {
                                self.take_line(text);
                            }
                        }
                    }
                    if self.restart_pending {
                        makepad_widgets::log!("wm: AI hub stopped ({status}) for a restart");
                        self.crashes = 0;
                        self.spawn(now);
                    } else if self.building {
                        let detail = if self.diagnostic.is_empty() { self.exit_reason(status) } else { self.diagnostic.trim_end().to_string() };
                        makepad_widgets::log!("wm: AI hub: the build failed ({status}):\n{detail}");
                        self.phase = HubPhase::BuildFailed(detail);
                    } else {
                        if self.ready_at.is_some_and(|t| now - t >= STEADY_S) {
                            self.crashes = 0;
                        }
                        self.crashes += 1;
                        let wait = backoff_s(self.crashes);
                        let reason = self.exit_reason(status);
                        makepad_widgets::log!("wm: AI hub exited ({reason}); restart {} in {wait} s", self.crashes);
                        self.restart_at = Some(now + wait as f64);
                        self.phase = HubPhase::Crashed { reason, restart_in: wait };
                    }
                    return false;
                }
                Ok(None) => {}
                Err(e) => makepad_widgets::log!("wm: AI hub: {e}"),
            }
        }
        if self.child.is_none() {
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
        if self.building || self.restart_pending {
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

    /// Stop it and start it again (with `settings` from now on). A failed build or a
    /// waiting restart starts at once.
    pub fn restart(&mut self, settings: HubSettings, now: f64, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        self.settings = settings;
        self.crashes = 0;
        match self.child.as_mut() {
            Some(child) => {
                makepad_widgets::log!("wm: AI hub: restarting pid {}", child.id());
                makepad_app_host::launch::kill_child_group(child, STOP_GRACE, pool);
                self.restart_pending = true;
                self.phase = HubPhase::Restarting;
            }
            None => self.spawn(now),
        }
    }

    /// The WM quits: the hub and whatever it started (a build's rustc) end with it.
    pub fn stop(&mut self, pool: &makepad_widgets::makepad_platform::thread::TaskPool) {
        self.restart_at = None;
        self.restart_pending = false;
        if let Some(child) = self.child.as_mut() {
            makepad_widgets::log!("wm: shutdown stops the AI hub (pid {})", child.id());
            makepad_app_host::launch::kill_child_group(child, STOP_GRACE, pool);
        }
    }
}

/// From SIGTERM to SIGKILL when stopping it: the hub ends at SIGTERM; a rustc in the
/// middle of a big crate may take a moment.
const STOP_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

fn read_lines(stream: impl std::io::Read + Send + 'static, log: Option<std::fs::File>, tx: Sender<Line>) {
    let spawned = std::thread::Builder::new().name("wm-ai-hub-output".into()).spawn(move || {
        registry::read_output(stream, log, |output| {
            let line = match output {
                ProcessOutput::Line(text) => Line::Out(text),
                ProcessOutput::Closed => Line::Closed,
            };
            tx.send(line).is_ok()
        });
    });
    if let Err(e) = spawned {
        makepad_widgets::log!("wm: AI hub output: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_control::ModelState;

    #[test]
    fn settings_read_what_the_service_ran_with_and_write_back_the_same() {
        let text = "port = 8123\nhost = 0.0.0.0\ncache_dir = /home/arch/.makepad/weights-chat\nmodel = qwen3.8-27b # the 27B\n\
                    context = 32768\ncontext_max = 81920\nkv = q8_0\npin = kokoro, whisper-large-v3-turbo\nnice = 10\n\
                    target_dir = /home/arch/makepad-target-hub\nenv MAKEPAD_CUDA_ROOT = /opt/cuda\nenv MAKEPAD_AI_UI_RESERVE_MB = 1536\nbogus = 1\n";
        let s = HubSettings::parse(text);
        assert!(s.enabled);
        assert_eq!(s.hub_args(), ["--host", "0.0.0.0", "--port", "8123", "--cache-dir", "/home/arch/.makepad/weights-chat"]);
        assert_eq!(s.base_url(), "https://127.0.0.1:8123");
        let env = s.hub_env();
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("MAKEPAD_AI_MODEL"), Some("qwen3.8-27b"));
        assert_eq!(get("MAKEPAD_AI_PIN_MODELS"), Some("qwen3.8-27b,kokoro,whisper-large-v3-turbo"));
        assert_eq!((get("MAKEPAD_ASSET_AI_LLM_CONTEXT"), get("MAKEPAD_ASSET_AI_LLM_CONTEXT_MAX"), get("MAKEPAD_ASSET_AI_LLM_KV")), (Some("32768"), Some("81920"), Some("q8_0")));
        assert_eq!((get("CARGO_TARGET_DIR"), get("MAKEPAD_CUDA_ROOT"), get("MAKEPAD_AI_UI_RESERVE_MB")), (Some("/home/arch/makepad-target-hub"), Some("/opt/cuda"), Some("1536")));
        assert_eq!(HubSettings::parse(&s.render()), s);
        // Nothing set: the defaults, loopback http when asked for.
        let d = HubSettings::parse("host = 127.0.0.1\nenabled = off\n");
        assert!(!d.enabled);
        assert_eq!(d.base_url(), "http://127.0.0.1:8123");
        assert!(d.hub_env().is_empty());
        assert_eq!(d.hub_args(), ["--host", "127.0.0.1", "--port", "8123"]);
    }

    #[test]
    fn restarts_back_off_and_cap() {
        assert_eq!((1..=10).map(backoff_s).collect::<Vec<_>>(), [2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
    }

    fn running(model: &str) -> HubChild {
        let mut h = HubChild::new(HubSettings { model: model.into(), ..Default::default() });
        // As if spawned from a binary: the phases after the process runs.
        h.restart_at = None;
        h.phase = HubPhase::Starting;
        h.child = Some(Command::new("sleep").arg("30").spawn().unwrap());
        h
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
        let _ = h.child.take().map(|mut c| c.kill());
    }

    #[test]
    fn a_load_taken_over_by_another_job_ends_in_ready_not_loading() {
        // 149: the 27B's start-up load gave way to a music job; the model stays unloaded
        let mut h = running("q");
        assert!(h.tick(1.0, true, Some(ModelState::Unloaded)));
        assert!(!h.tick(4.0, true, Some(ModelState::Loading)));
        assert_eq!(h.phase, HubPhase::LoadingModel);
        h.tick(7.0, true, Some(ModelState::Unloaded));
        assert_eq!(h.phase, HubPhase::LoadingModel, "not yet: the hub may not have said so");
        h.tick(12.0, true, Some(ModelState::Unloaded));
        assert_eq!(h.phase, HubPhase::Ready);
        let _ = h.child.take().map(|mut c| c.kill());
    }

    #[test]
    fn a_build_reports_its_stage_then_runs() {
        let mut h = running("");
        h.building = true;
        h.phase = HubPhase::Building(String::new());
        h.take_line("Compiling makepad-ai-llm v0.1.0 (/x)".into());
        assert_eq!(h.phase, HubPhase::Building("compiling makepad-ai-llm v0.1.0…".into()));
        h.take_line("Running `target/release/makepad-app-ai-hub --port 8123`".into());
        assert_eq!(h.phase, HubPhase::Starting);
        h.tick(1.0, true, None);
        assert_eq!(h.phase, HubPhase::Ready, "no model to load: ready when it answers");
        let _ = h.child.take().map(|mut c| c.kill());
    }

    #[test]
    fn an_exit_is_a_crash_with_its_reason_and_a_timed_restart() {
        let mut h = running("q");
        h.child.take().map(|mut c| c.kill());
        h.child = Some(Command::new("cargo").arg("--no-such-hub-flag").stderr(Stdio::piped()).spawn().unwrap());
        let (tx, rx) = channel();
        read_lines(h.child.as_mut().unwrap().stderr.take().unwrap(), None, tx);
        h.lines = Some(rx);
        let _ = h.child.as_mut().unwrap().wait();
        std::thread::sleep(std::time::Duration::from_millis(50));
        h.tick(10.0, false, None);
        match &h.phase {
            HubPhase::Crashed { reason, restart_in } => {
                assert!(reason.contains("error") && reason.contains("--no-such-hub-flag"), "its last error: {reason}");
                assert_eq!(*restart_in, 2);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(h.restart_at, Some(12.0));
        h.tick(11.0, false, None);
        assert_eq!(h.phase.headline("q"), "Crashed, restarting in 1 s");
    }
}
