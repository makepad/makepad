//! What the AI Hub app serves, saved across launches in `~/.makepad/ai-hub/app.conf`
//! (`MAKEPAD_AI_HUB_APP_CONF` overrides the path): one `key = value` per line.
//!
//! Safe defaults: the hub is Enabled, but every backend is off until the person turns it
//! on, and each Claude switch (the LAN's use, PR reviews) turns itself off after an hour
//! without a request of its own. An inactivity turn-off is saved like a switch by hand, so
//! a restart never brings a service back on its own.
//!
//! Started by a makepad WM as its node's hub (`MAKEPAD_AI_HUB_NODE_CONF`), the app serves
//! what the node's `ai-hub.conf` says (where, the cache, the chat model, the compute mode:
//! [`makepad_ai_hub::node_conf::NodeConf`]); this file keeps only the person's switches, and
//! a node's first start has its local models on, with no inactivity turn-off.

use std::path::PathBuf;

/// The port the Mac hub has served on (the fleet's pins and Amp's writer name it).
pub const DEFAULT_PORT: u16 = makepad_ai_hub::hub_order::APP_PORT;

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

    /// The pill's segment: `15m`, `1h`, `4h`, `never`.
    pub fn short(self) -> &'static str {
        match self {
            IdleOff::Min15 => "15m",
            IdleOff::Hour1 => "1h",
            IdleOff::Hour4 => "4h",
            IdleOff::Never => "never",
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

/// One backend the hub can offer. Claude Code on this machine, tool-less in its sandbox
/// (`claude-cli`), has two switches of its own: the fleet's use and the PR reviews.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backend {
    /// claude-cli for the fleet, through `/generate` (Sandbox, the asset tools).
    ClaudeLan,
    /// claude-cli for the intake credential's PR reviews, through `/v1/messages`.
    ClaudeIntake,
    /// The local models this machine can run and has on disk.
    Local,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::ClaudeLan, Backend::ClaudeIntake, Backend::Local];

    pub fn label(self) -> &'static str {
        match self {
            Backend::ClaudeLan => "Claude for the LAN",
            Backend::ClaudeIntake => "Claude for PR reviews",
            Backend::Local => "Local models",
        }
    }

    /// Whether this is one of the two claude-cli switches.
    pub fn is_claude(self) -> bool {
        matches!(self, Backend::ClaudeLan | Backend::ClaudeIntake)
    }

    fn key(self) -> &'static str {
        match self {
            Backend::ClaudeLan => "claude_lan",
            Backend::ClaudeIntake => "claude_intake",
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
    pub claude_lan: Service,
    pub claude_intake: Service,
    pub local: Service,
    /// The LAN address to serve on; empty = this machine's first private address.
    pub host: String,
    pub port: u16,
    /// The hub's cache: models, artifacts, and `fleet/` (fleet key, TLS identity).
    pub cache_dir: Option<PathBuf>,
    /// The compute mode the hub runs under (the panel's Automatic / UI First / AI First /
    /// Unavailable picker), sent to the hub at each start.
    pub compute: makepad_ai_hub::activity::ComputeMode,
    /// Read the coding CLIs' account usage (one warm Claude terminal and Codex app-server, and
    /// one short Antigravity print-mode run per read, about every 10 min). On in the desktop
    /// app, off on a WM node.
    pub usage: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            claude_lan: Service { on: false, idle_off: IdleOff::Hour1, auto_off_at: None },
            claude_intake: Service { on: false, idle_off: IdleOff::Hour1, auto_off_at: None },
            local: Service { on: false, idle_off: IdleOff::Never, auto_off_at: None },
            host: String::new(),
            port: DEFAULT_PORT,
            cache_dir: None,
            compute: makepad_ai_hub::activity::ComputeMode::Automatic,
            usage: true,
        }
    }
}

impl Settings {
    /// `MAKEPAD_AI_HUB_APP_CONF`, else under a test instance's own home (an absolute
    /// `MAKEPAD_TEST_HOME`: a test never touches the machine's hub), else `~/.makepad/ai-hub`.
    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("MAKEPAD_AI_HUB_APP_CONF").filter(|p| !p.is_empty()) {
            return PathBuf::from(path);
        }
        let home = std::env::var_os("MAKEPAD_TEST_HOME").map(PathBuf::from).filter(|h| h.is_absolute()).map(|h| h.join(".makepad"));
        home.unwrap_or_else(makepad_ai_hub::home::makepad_home).join("ai-hub").join("app.conf")
    }

    /// The saved switches; without a file, the defaults (`node`: a WM node's hub serves its
    /// local models from the start).
    pub fn load(node: bool) -> Settings {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_else(|_| if node { Self::node_defaults() } else { Settings::default() })
    }

    /// A WM node's hub: its local models on and never turned off for idleness; both Claude
    /// switches off.
    pub fn node_defaults() -> Settings {
        let mut s = Settings::default();
        s.local = Service { on: true, idle_off: IdleOff::Never, auto_off_at: None };
        s.usage = false;
        s
    }

    /// Where the app keeps its state: the settings' folder (the account usage histories too).
    pub fn dir() -> PathBuf {
        Self::path().parent().map(|p| p.to_path_buf()).unwrap_or_default()
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
            Backend::ClaudeLan => &self.claude_lan,
            Backend::ClaudeIntake => &self.claude_intake,
            Backend::Local => &self.local,
        }
    }

    pub fn service_mut(&mut self, backend: Backend) -> &mut Service {
        match backend {
            Backend::ClaudeLan => &mut self.claude_lan,
            Backend::ClaudeIntake => &mut self.claude_intake,
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
                "usage" => s.usage = on(value),
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
        out.push_str(&format!("enabled = {}\nusage = {}\n", flag(self.enabled), flag(self.usage)));
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

    const CLAUDE: [Backend; 2] = [Backend::ClaudeLan, Backend::ClaudeIntake];

    #[test]
    fn defaults_serve_nothing_and_each_claude_switch_turns_off_after_an_hour() {
        let s = Settings::default();
        assert!(s.enabled && !s.claude_lan.on && !s.claude_intake.on && !s.local.on, "everything starts off");
        assert!(s.offered().is_empty());
        assert_eq!((s.claude_lan.idle_off, s.claude_intake.idle_off), (IdleOff::Hour1, IdleOff::Hour1), "an hour each");
        assert_eq!(s.port, DEFAULT_PORT);
    }

    #[test]
    fn a_wm_nodes_first_start_serves_its_local_models_and_never_idles_them_off() {
        let s = Settings::node_defaults();
        assert!(s.enabled && s.local.on && !s.claude_lan.on && !s.claude_intake.on);
        assert!(!s.usage, "a node starts no Claude or Codex CLI to read usage");
        assert!(Settings::default().usage, "the desktop app reads it");
        assert_eq!(s.local.idle_off, IdleOff::Never);
        assert_eq!(s.offered(), vec![Backend::Local]);
    }

    #[test]
    fn settings_round_trip() {
        let mut s = Settings::default();
        s.set_service(Backend::ClaudeIntake, true);
        s.claude_intake.idle_off = IdleOff::Never;
        s.claude_lan.idle_off = IdleOff::Min15;
        s.local.auto_off_at = Some(42);
        s.host = "192.0.2.91".into();
        s.cache_dir = Some(PathBuf::from("/tmp/x"));
        s.compute = makepad_ai_hub::activity::ComputeMode::UiFirst;
        s.usage = false;
        assert_eq!(Settings::parse(&s.render()), s);
        assert_eq!(s.offered(), vec![Backend::ClaudeIntake], "the PR-review switch alone");
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

    /// The two Claude switches keep their own clocks: requests under one never keep the other
    /// on, and each turns off as if by hand.
    #[test]
    fn the_claude_switches_idle_off_on_their_own_clocks() {
        let hour = 60 * 60 * 1000;
        for (busy, idle) in [(Backend::ClaudeLan, Backend::ClaudeIntake), (Backend::ClaudeIntake, Backend::ClaudeLan)] {
            let mut s = Settings::default();
            for b in Backend::ALL {
                s.set_service(b, true);
            }
            s.local.idle_off = IdleOff::Never;
            // `busy` served every few minutes, `idle` never: at 1 h only `idle` turns off.
            let used = |b: Backend| (b == busy).then_some(hour - 5 * 60 * 1000);
            assert_eq!(apply_idle_offs(&mut s, |_| 0, used, hour), vec![idle], "{} use keeps only itself on", busy.label());
            assert!(s.service(busy).on && !s.service(idle).on && s.local.on, "only the idle switch, and local has no limit");
            assert_eq!(s.service(idle).auto_off_at, Some(hour));
            assert!(!s.offered().contains(&idle) && s.offered().contains(&busy), "the hub stops offering only that one");
            // Saved and loaded again (an app restart): still off, nothing fires again.
            let mut again = Settings::parse(&s.render());
            assert!(!again.service(idle).on);
            again.set_service(busy, false);
            assert!(apply_idle_offs(&mut again, |_| 10 * hour, |_| None, 20 * hour).is_empty());
            // The master switch off: no clocks run.
            again.set_service(idle, true);
            again.enabled = false;
            assert!(apply_idle_offs(&mut again, |_| 0, |_| None, 100 * hour).is_empty());
        }
    }

    #[test]
    fn a_turn_off_is_saved_like_a_switch_and_a_restart_never_re_enables_either() {
        for b in CLAUDE {
            let mut s = Settings::default();
            for c in CLAUDE {
                s.set_service(c, true);
            }
            // The turn-off fires: what the app does on expiry.
            s.service_mut(b).on = false;
            s.service_mut(b).auto_off_at = Some(1_700_000_000_000);
            let reloaded = Settings::parse(&s.render());
            assert!(!reloaded.service(b).on, "a restart never re-enables {}", b.label());
            assert_eq!(reloaded.service(b).auto_off_at, Some(1_700_000_000_000));
            let other = CLAUDE.into_iter().find(|c| *c != b).unwrap();
            assert!(reloaded.service(other).on, "and leaves the other switch as it was");
            let mut again = reloaded;
            again.set_service(b, true);
            assert_eq!(again.service(b).auto_off_at, None, "the person turning it on clears the note");
        }
        // A settings file from before the split: neither switch comes on from its old key.
        let old = Settings::parse("claude = on\nclaude_idle_off = never\n");
        assert!(!old.claude_lan.on && !old.claude_intake.on, "the old single switch turns neither on");
    }
}
