//! What the AI Hub app serves, saved across launches in `~/.makepad/ai-hub/app.conf`
//! (`MAKEPAD_AI_HUB_APP_CONF` overrides the path): one `key = value` per line.
//!
//! Safe defaults: the hub is Enabled, but every backend is off until the person turns it
//! on, and the Claude backend turns itself off after an hour without a request. An
//! inactivity turn-off is saved like a switch by hand, so a restart never brings a
//! service back on its own.
//!
//! Started by a makepad WM as its node's hub (`MAKEPAD_AI_HUB_NODE_CONF`), the app serves
//! what the node's `ai-hub.conf` says (where, the cache, the chat model, the compute mode:
//! [`makepad_ai_hub::node_conf::NodeConf`]); this file keeps only the person's switches, and
//! a node's first start has its local models on, with no inactivity turn-off.

use std::path::PathBuf;

/// The port the Mac hub has served on (the fleet's pins and Amp's writer name it).
pub const DEFAULT_PORT: u16 = 8124;

/// After how long without a request a service switches itself off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleOff {
    Min15,
    Hour1,
    Hour4,
    Never,
}

impl IdleOff {
    pub const ALL: [IdleOff; 4] = [IdleOff::Min15, IdleOff::Hour1, IdleOff::Hour4, IdleOff::Never];

    pub fn millis(self) -> Option<u64> {
        match self {
            IdleOff::Min15 => Some(15 * 60 * 1000),
            IdleOff::Hour1 => Some(60 * 60 * 1000),
            IdleOff::Hour4 => Some(4 * 60 * 60 * 1000),
            IdleOff::Never => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            IdleOff::Min15 => "15 min",
            IdleOff::Hour1 => "1 h",
            IdleOff::Hour4 => "4 h",
            IdleOff::Never => "Never",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            IdleOff::Min15 => "15m",
            IdleOff::Hour1 => "1h",
            IdleOff::Hour4 => "4h",
            IdleOff::Never => "never",
        }
    }

    fn parse(text: &str) -> Option<IdleOff> {
        IdleOff::ALL.into_iter().find(|i| i.slug() == text)
    }
}

/// One backend the hub can offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    /// Claude Code on this machine, tool-less in its sandbox (`claude-cli`).
    Claude,
    /// The local models this machine can run and has on disk.
    Local,
}

impl Backend {
    pub const ALL: [Backend; 2] = [Backend::Claude, Backend::Local];

    pub fn label(self) -> &'static str {
        match self {
            Backend::Claude => "Claude (sandboxed) as a fleet service",
            Backend::Local => "Local models",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Backend::Claude => "claude",
            Backend::Local => "local",
        }
    }
}

/// One backend's switch, inactivity turn-off, and when that last fired (unix ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    pub on: bool,
    pub idle_off: IdleOff,
    pub auto_off_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The master switch: off = nothing listens and nothing is announced.
    pub enabled: bool,
    pub claude: Service,
    pub local: Service,
    /// The LAN address to serve on; empty = this machine's first private address.
    pub host: String,
    pub port: u16,
    /// The hub's cache: models, artifacts, and `fleet/` (fleet key, TLS identity).
    pub cache_dir: Option<PathBuf>,
    /// The compute mode the hub runs under (the panel's Automatic / UI First / AI First /
    /// Unavailable picker), sent to the hub at each start.
    pub compute: makepad_ai_hub::activity::ComputeMode,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            claude: Service { on: false, idle_off: IdleOff::Hour1, auto_off_at: None },
            local: Service { on: false, idle_off: IdleOff::Never, auto_off_at: None },
            host: String::new(),
            port: DEFAULT_PORT,
            cache_dir: None,
            compute: makepad_ai_hub::activity::ComputeMode::Automatic,
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        match std::env::var_os("MAKEPAD_AI_HUB_APP_CONF").filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => makepad_ai_hub::home::makepad_home().join("ai-hub").join("app.conf"),
        }
    }

    /// The saved switches; without a file, the defaults (`node`: a WM node's hub serves its
    /// local models from the start).
    pub fn load(node: bool) -> Settings {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_else(|_| if node { Self::node_defaults() } else { Settings::default() })
    }

    /// A WM node's hub: its local models on and never turned off for idleness; Claude off.
    pub fn node_defaults() -> Settings {
        let mut s = Settings::default();
        s.local = Service { on: true, idle_off: IdleOff::Never, auto_off_at: None };
        s
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let tmp = path.with_extension("conf.new");
        std::fs::write(&tmp, self.render()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn service(&self, backend: Backend) -> &Service {
        match backend {
            Backend::Claude => &self.claude,
            Backend::Local => &self.local,
        }
    }

    pub fn service_mut(&mut self, backend: Backend) -> &mut Service {
        match backend {
            Backend::Claude => &mut self.claude,
            Backend::Local => &mut self.local,
        }
    }

    /// What the hub serves: the switches as they stand.
    pub fn offered(&self) -> Vec<Backend> {
        Backend::ALL.into_iter().filter(|b| self.enabled && self.service(*b).on).collect()
    }

    /// The person switches `backend`: on clears an earlier inactivity turn-off.
    pub fn set_service(&mut self, backend: Backend, on: bool) {
        let s = self.service_mut(backend);
        s.on = on;
        if on {
            s.auto_off_at = None;
        }
    }

    pub fn cache_dir(&self) -> PathBuf {
        match &self.cache_dir {
            Some(dir) => dir.clone(),
            None => match std::env::var_os("MAKEPAD_ASSET_AI_CACHE") {
                Some(dir) => PathBuf::from(dir),
                None => makepad_ai_hub::home::weights_dir(),
            },
        }
    }

    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        let on = |v: &str| matches!(v, "on" | "true" | "1" | "yes");
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "enabled" => s.enabled = on(value),
                "host" => s.host = value.to_string(),
                "port" => s.port = value.parse().unwrap_or(DEFAULT_PORT),
                "cache_dir" => s.cache_dir = (!value.is_empty()).then(|| PathBuf::from(value)),
                "compute" => s.compute = makepad_ai_hub::activity::ComputeMode::parse(value).unwrap_or(s.compute),
                _ => {
                    for backend in Backend::ALL {
                        let k = backend.key();
                        let svc = s.service_mut(backend);
                        if key == k {
                            svc.on = on(value);
                        } else if key == format!("{k}_idle_off") {
                            svc.idle_off = IdleOff::parse(value).unwrap_or(svc.idle_off);
                        } else if key == format!("{k}_auto_off_at") {
                            svc.auto_off_at = value.parse().ok();
                        }
                    }
                }
            }
        }
        s
    }

    pub fn render(&self) -> String {
        let flag = |b: bool| if b { "on" } else { "off" };
        let mut out = String::from("# AI Hub app settings (the app writes this file)\n");
        out.push_str(&format!("enabled = {}\n", flag(self.enabled)));
        for backend in Backend::ALL {
            let k = backend.key();
            let svc = self.service(backend);
            out.push_str(&format!("{k} = {}\n{k}_idle_off = {}\n", flag(svc.on), svc.idle_off.slug()));
            if let Some(at) = svc.auto_off_at {
                out.push_str(&format!("{k}_auto_off_at = {at}\n"));
            }
        }
        out.push_str(&format!("host = {}\nport = {}\ncompute = {}\n", self.host, self.port, self.compute.name()));
        if let Some(dir) = &self.cache_dir {
            out.push_str(&format!("cache_dir = {}\n", dir.display()));
        }
        out
    }
}

/// The inactivity clock of one service: idle since the later of its last served request
/// and the moment it came on (a launch counts: a restart does not inherit an old idle
/// stretch, and it never re-enables what was turned off).
pub fn idle_expired(svc: &Service, on_since_ms: u64, last_used_ms: Option<u64>, now_ms: u64) -> bool {
    let Some(limit) = svc.idle_off.millis() else { return false };
    if !svc.on {
        return false;
    }
    let since = last_used_ms.map_or(on_since_ms, |u| u.max(on_since_ms));
    now_ms.saturating_sub(since) >= limit
}

/// The inactivity turn-offs at `now_ms`: every service idle past its limit switches off
/// exactly as by hand and notes when (`auto_off_at`). Returns the backends turned off.
pub fn apply_idle_offs(settings: &mut Settings, on_since_ms: impl Fn(Backend) -> u64, last_used_ms: impl Fn(Backend) -> Option<u64>, now_ms: u64) -> Vec<Backend> {
    if !settings.enabled {
        return Vec::new();
    }
    let mut off = Vec::new();
    for backend in Backend::ALL {
        let svc = settings.service_mut(backend);
        if idle_expired(svc, on_since_ms(backend), last_used_ms(backend), now_ms) {
            svc.on = false;
            svc.auto_off_at = Some(now_ms);
            off.push(backend);
        }
    }
    off
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_serve_nothing_and_claude_turns_off_after_an_hour() {
        let s = Settings::default();
        assert!(s.enabled && !s.claude.on && !s.local.on);
        assert!(s.offered().is_empty());
        assert_eq!(s.claude.idle_off, IdleOff::Hour1);
        assert_eq!(s.port, DEFAULT_PORT);
    }

    #[test]
    fn a_wm_nodes_first_start_serves_its_local_models_and_never_idles_them_off() {
        let s = Settings::node_defaults();
        assert!(s.enabled && s.local.on && !s.claude.on);
        assert_eq!(s.local.idle_off, IdleOff::Never);
        assert_eq!(s.offered(), vec![Backend::Local]);
    }

    #[test]
    fn settings_round_trip() {
        let mut s = Settings::default();
        s.set_service(Backend::Claude, true);
        s.claude.idle_off = IdleOff::Min15;
        s.local.auto_off_at = Some(42);
        s.host = "10.0.0.222".into();
        s.cache_dir = Some(PathBuf::from("/tmp/x"));
        s.compute = makepad_ai_hub::activity::ComputeMode::UiFirst;
        assert_eq!(Settings::parse(&s.render()), s);
        assert_eq!(s.offered(), vec![Backend::Claude]);
        s.enabled = false;
        assert!(s.offered().is_empty(), "the master switch wins");
    }

    #[test]
    fn the_inactivity_turn_off_expires_from_the_last_request_or_the_switch_on() {
        let hour = 60 * 60 * 1000;
        let mut svc = Service { on: true, idle_off: IdleOff::Hour1, auto_off_at: None };
        // On at t=0, never used: off at 1 h.
        assert!(!idle_expired(&svc, 0, None, hour - 1));
        assert!(idle_expired(&svc, 0, None, hour));
        // A request at 50 min moves it to 1 h 50 min.
        assert!(!idle_expired(&svc, 0, Some(50 * 60 * 1000), hour + 1));
        assert!(idle_expired(&svc, 0, Some(50 * 60 * 1000), hour + 50 * 60 * 1000));
        // An app restart at 3 h with an old last use counts from the launch.
        assert!(!idle_expired(&svc, 3 * hour, Some(0), 3 * hour + 10));
        svc.idle_off = IdleOff::Never;
        assert!(!idle_expired(&svc, 0, None, 100 * hour));
        svc.idle_off = IdleOff::Hour1;
        svc.on = false;
        assert!(!idle_expired(&svc, 0, None, 100 * hour), "an off service has no clock");
    }

    #[test]
    fn the_expiry_path_turns_claude_off_and_leaves_it_off() {
        let hour = 60 * 60 * 1000;
        let mut s = Settings::default();
        s.set_service(Backend::Claude, true);
        s.set_service(Backend::Local, true);
        s.local.idle_off = IdleOff::Never;
        // Claude served at 10 min; at 1 h 9 min nothing happens, at 1 h 10 min it turns off.
        let used = |b: Backend| (b == Backend::Claude).then_some(10 * 60 * 1000);
        assert!(apply_idle_offs(&mut s, |_| 0, used, hour + 9 * 60 * 1000).is_empty());
        assert_eq!(apply_idle_offs(&mut s, |_| 0, used, hour + 10 * 60 * 1000), vec![Backend::Claude]);
        assert!(!s.claude.on && s.local.on, "only the idle service, and local has no limit");
        assert_eq!(s.claude.auto_off_at, Some(hour + 10 * 60 * 1000));
        assert_eq!(s.offered(), vec![Backend::Local], "the hub stops offering Claude");
        // Saved and loaded again (an app restart): still off, nothing fires again.
        let mut again = Settings::parse(&s.render());
        assert!(apply_idle_offs(&mut again, |_| 10 * hour, |_| None, 20 * hour).is_empty());
        assert!(!again.claude.on);
        // The master switch off: no clocks run.
        again.set_service(Backend::Claude, true);
        again.enabled = false;
        assert!(apply_idle_offs(&mut again, |_| 0, |_| None, 100 * hour).is_empty());
    }

    #[test]
    fn an_auto_turn_off_is_saved_like_a_switch_and_survives_a_restart() {
        let mut s = Settings::default();
        s.set_service(Backend::Claude, true);
        // The turn-off fires: what the app does on expiry.
        s.claude.on = false;
        s.claude.auto_off_at = Some(1_700_000_000_000);
        let reloaded = Settings::parse(&s.render());
        assert!(!reloaded.claude.on, "a restart never re-enables it");
        assert_eq!(reloaded.claude.auto_off_at, Some(1_700_000_000_000));
        let mut again = reloaded;
        again.set_service(Backend::Claude, true);
        assert_eq!(again.claude.auto_off_at, None, "the person turning it on clears the note");
    }
}
