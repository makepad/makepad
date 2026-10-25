//! A node's AI hub settings: `~/.makepad/wm/ai-hub.conf` (`MAKEPAD_WM_AI_HUB_CONF` names
//! another file). The one source of truth for the hub a window manager node runs: the makepad
//! WM reads it to start and supervise the AI Hub app (apps/wm/src/hub_app.rs: the environment
//! below, the restart identity), and the app the WM starts reads the same file
//! (`MAKEPAD_AI_HUB_NODE_CONF` names it) for where and what it serves. `key = value` lines,
//! every key optional:
//!
//! ```text
//! enabled = on                    # off: the WM starts no AI Hub
//! port = 8123
//! host = 0.0.0.0                  # 0.0.0.0 or empty: this node's LAN address; else that LAN address
//! cache_dir = /home/arch/.makepad/weights-chat   # default: the hub's own (~/.makepad/weights)
//! model = qwen3.8-27b             # the chat model loaded at start and pinned; apps the WM starts use it
//! context = 32768                 # the chat context per lane at start (the app's panel changes it live)
//! context_max = 81920
//! kv = q8_0
//! pin = kokoro,whisper-large-v3-turbo   # models kept loaded besides the chat model
//! nice = 10                       # the app's scheduling priority (Unix)
//! compute = auto                  # compute jobs: auto (a game pauses them) | ui-first | ai-first | unavailable
//! models = all                    # keep every model this node can run in its cache (or a list of ids)
//! models_licences = none,community   # licence classes `models` fetches (+ non-commercial, restricted, gated)
//! models_mbps = 60                # its download pacing, MB/s
//! features = node                 # the app's cargo features (node: the full local AI set)
//! env MAKEPAD_CUDA_ROOT = /opt/cuda              # any other variable for the app
//! ```
//!
//! Without `env MAKEPAD_CUDA_ROOT` the toolkit installed here is named (`/opt/cuda`,
//! `/usr/local/cuda`, Windows' `CUDA_PATH`): without it the hub's CUDA kernels are not built
//! and it serves no GPU model.
//!
//! The WM starts the app with `cargo run --release -p makepad-app-aihub --features <features>`
//! in its checkout, with the cargo environment of every launcher (`makepad-cargo`), in the
//! WM's own cargo target: the deploy builds it with the very same command
//! (`makepad-cargo build --features <features> aihub`, same CUDA environment), so the start
//! compiles nothing. `enabled = off` also records the app's Quit: the tray menu's Quit writes
//! it, and the WM starts the app again only when the person starts it from the launcher
//! (which writes `enabled = on`).

use crate::activity::ComputeMode;
use std::path::{Path, PathBuf};

/// The variable naming another settings file (the WM's, for test desktops).
pub const CONF_ENV: &str = "MAKEPAD_WM_AI_HUB_CONF";
/// The variable the WM sets for the AI Hub app it starts: this file's path. The app then
/// serves as this node's hub (`NodeConf`) instead of from its own settings.
pub const NODE_CONF_ENV: &str = "MAKEPAD_AI_HUB_NODE_CONF";

#[derive(Clone, Debug, PartialEq)]
pub struct NodeConf {
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
    /// Whether this node takes compute jobs (the app's picker).
    pub compute: ComputeMode,
    pub features: String,
    /// Prefetch (`MAKEPAD_AI_PREFETCH`): `all`, or model ids; None = off.
    pub models: Option<String>,
    pub models_licences: Option<String>,
    pub models_mbps: Option<String>,
    pub env: Vec<(String, String)>,
}

impl Default for NodeConf {
    fn default() -> Self {
        NodeConf {
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
            features: "node".to_string(),
            models: None,
            models_licences: None,
            models_mbps: None,
            env: Vec::new(),
        }
    }
}

impl NodeConf {
    pub fn path() -> PathBuf {
        match std::env::var_os(CONF_ENV).filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => crate::home::makepad_home().join("wm/ai-hub.conf"),
        }
    }

    /// The file at `path`; a missing one is the defaults.
    pub fn load_from(path: &Path) -> NodeConf {
        std::fs::read_to_string(path).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    /// The file as it is now (edited since the start, maybe), else `current`.
    pub fn load_or(path: &Path, current: &NodeConf) -> NodeConf {
        std::fs::read_to_string(path).map(|t| Self::parse(&t)).unwrap_or_else(|_| current.clone())
    }

    /// Writes the file (a temporary beside it, then renamed: a reader never sees half of it).
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension("conf.new");
        std::fs::write(&tmp, self.render()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Unknown keys and bad values are skipped (and said on stderr): a typo never stops the
    /// hub from starting with the rest.
    pub fn parse(text: &str) -> NodeConf {
        let mut s = NodeConf::default();
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
                "host" => { s.host = value.to_string(); true }
                "cache_dir" => { s.cache_dir = Some(value.to_string()).filter(|v| !v.is_empty()); true }
                "model" => { s.model = value.to_string(); true }
                "context" => value.parse().map(|c| s.context = Some(c)).is_ok(),
                "context_max" => value.parse().map(|c| s.context_max = Some(c)).is_ok(),
                "kv" => { s.kv = Some(value.to_string()).filter(|v| !v.is_empty()); true }
                "pin" => { s.pin = value.split(',').map(str::trim).filter(|m| !m.is_empty()).map(str::to_string).collect(); true }
                "nice" => value.parse().map(|n| s.nice = n).is_ok(),
                "compute" => ComputeMode::parse(value).map(|m| s.compute = m).is_some(),
                "features" => { s.features = value.to_string(); true }
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
                eprintln!("ai-hub.conf: skipped {line:?}");
            }
        }
        s
    }

    pub fn render(&self) -> String {
        let mut out = String::from("# This node's AI hub: the AI Hub app the window manager starts (libs/ai/hub/src/node_conf.rs).\n");
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
        out.push_str(&format!("features = {}\n", self.features));
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

    /// The LAN address the hub serves on: `host` when it names one, else (empty, `0.0.0.0`)
    /// this machine's first private IPv4 address. A node hub serves the LAN only, through its
    /// fleet TLS front.
    pub fn serve_host(&self) -> Option<String> {
        lan_host(&self.host)
    }

    /// The cache: `cache_dir`, else the hub's default.
    pub fn cache_dir(&self) -> PathBuf {
        match &self.cache_dir {
            Some(dir) => PathBuf::from(dir),
            None => match std::env::var_os("MAKEPAD_ASSET_AI_CACHE") {
                Some(dir) => PathBuf::from(dir),
                None => crate::home::weights_dir(),
            },
        }
    }

    /// Where a control client on this machine reaches the hub: its fleet TLS front.
    pub fn base_url(&self) -> Option<String> {
        Some(format!("https://{}:{}", self.serve_host()?, self.port))
    }

    /// The environment the settings give the app, in order (later wins). The hub library
    /// reads these at its start (the chat model and pins, context, prefetch) and the build of
    /// the CUDA kernels reads the toolkit's root.
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
        for (key, value) in [("MAKEPAD_AI_PREFETCH", &self.models), ("MAKEPAD_AI_PREFETCH_LICENCES", &self.models_licences), ("MAKEPAD_AI_PREFETCH_MBPS", &self.models_mbps)] {
            if let Some(value) = value {
                env.push((key.to_string(), value.clone()));
            }
        }
        env.extend(self.env.iter().cloned());
        // The hub's GPU models need its CUDA kernels, and makepad-ai-cuda builds them only
        // when MAKEPAD_CUDA_ROOT names the toolkit. A WM's session environment never has it,
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

/// `setting` when it names an address, else (empty or the wildcard) this machine's first
/// private IPv4 address.
pub fn lan_host(setting: &str) -> Option<String> {
    let setting = setting.trim();
    if !setting.is_empty() && setting != "0.0.0.0" && setting != "::" {
        return Some(setting.to_string());
    }
    crate::discovery::network_addresses().into_iter().find(|ip| ip.is_private()).map(|ip| ip.to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installed_cuda_toolkit_is_found_by_its_nvcc() {
        let base = std::env::temp_dir().join(format!("node-conf-cuda-{}", std::process::id()));
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
    fn settings_read_what_the_node_ran_with_and_write_back_the_same() {
        let text = "port = 8123\nhost = 10.0.0.217\ncache_dir = /home/arch/.makepad/weights-chat\nmodel = qwen3.8-27b # the 27B\n\
                    context = 32768\ncontext_max = 81920\nkv = q8_0\npin = kokoro, whisper-large-v3-turbo\nnice = 10\ncompute = ai-first\n\
                    features = node,claude-stub\n\
                    models = all\nmodels_licences = none,community,non-commercial\nmodels_mbps = 40\nenv MAKEPAD_CUDA_ROOT = /opt/cuda\nenv MAKEPAD_AI_UI_RESERVE_MB = 1536\nbogus = 1\n";
        let s = NodeConf::parse(text);
        assert!(s.enabled);
        assert_eq!(s.compute, ComputeMode::AiFirst);
        assert_eq!(s.features.as_str(), "node,claude-stub");
        assert_eq!(s.serve_host().as_deref(), Some("10.0.0.217"));
        assert_eq!(s.base_url().as_deref(), Some("https://10.0.0.217:8123"));
        assert_eq!(s.cache_dir(), PathBuf::from("/home/arch/.makepad/weights-chat"));
        let env = s.hub_env();
        let get = |k: &str| env.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("MAKEPAD_AI_MODEL"), Some("qwen3.8-27b"));
        assert_eq!(get("MAKEPAD_AI_PIN_MODELS"), Some("qwen3.8-27b,kokoro,whisper-large-v3-turbo"));
        assert_eq!((get("MAKEPAD_ASSET_AI_LLM_CONTEXT"), get("MAKEPAD_ASSET_AI_LLM_CONTEXT_MAX"), get("MAKEPAD_ASSET_AI_LLM_KV")), (Some("32768"), Some("81920"), Some("q8_0")));
        assert_eq!((get("MAKEPAD_CUDA_ROOT"), get("MAKEPAD_AI_UI_RESERVE_MB")), (Some("/opt/cuda"), Some("1536")));
        assert_eq!((get("MAKEPAD_AI_PREFETCH"), get("MAKEPAD_AI_PREFETCH_LICENCES"), get("MAKEPAD_AI_PREFETCH_MBPS")), (Some("all"), Some("none,community,non-commercial"), Some("40")));
        assert_eq!(NodeConf::parse(&s.render()), s);
        // The conf's own CUDA root is the one passed, once.
        assert_eq!(env.iter().filter(|(k, _)| k == CUDA_ROOT).count(), 1);
        // Nothing set: the defaults, the node features, this machine's LAN address.
        let d = NodeConf::parse("enabled = off\n");
        assert!(!d.enabled);
        assert_eq!((d.port, d.compute, d.features.as_str()), (8123, ComputeMode::Automatic, "node"));
        assert!(d.hub_env().iter().all(|(k, _)| k == CUDA_ROOT));
        assert_eq!(lan_host(" 10.0.0.5 ").as_deref(), Some("10.0.0.5"));
        assert_eq!(lan_host("0.0.0.0"), lan_host(""), "the wildcard is this machine's LAN address");
    }

    #[test]
    fn a_save_is_read_back() {
        let path = std::env::temp_dir().join(format!("node-conf-save-{}", std::process::id())).join("ai-hub.conf");
        let s = NodeConf { compute: ComputeMode::UiFirst, model: "q".into(), ..Default::default() };
        s.save_to(&path).unwrap();
        assert_eq!(NodeConf::load_from(&path), s);
        assert_eq!(NodeConf::load_from(&path.with_extension("missing")), NodeConf::default());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
