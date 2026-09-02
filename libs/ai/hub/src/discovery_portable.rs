//! Portable fleet discovery: keep the shared vocabulary while reporting
//! that UDP LAN discovery is unavailable.

use makepad_micro_serde::*;

pub const DISCOVERY_PORT: u16 = 41830;
pub const DEFAULT_FLEET: &str = "gen";

pub fn normalize_fleet(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        DEFAULT_FLEET.to_string()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

pub fn wanted_fleet() -> String {
    normalize_fleet(&std::env::var("MAKEPAD_AI_FLEET").unwrap_or_default())
}

pub fn fleet_from_env() -> String {
    normalize_fleet(&std::env::var("MAKEPAD_ASSET_AI_FLEET").unwrap_or_default())
}

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct BeaconJson {
    pub service: String,
    pub node_id: u64,
    pub port: u16,
    pub fleet: Option<String>,
}

pub fn mint_node_id() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

pub fn start_beacon(_node_id: u64, _http_port: u16, _fleet: String) {}

#[derive(Clone, Debug)]
pub struct DiscoveredNode {
    pub base_url: String,
    pub node_id: u64,
    pub fleet: String,
}

#[derive(Clone, Default)]
pub struct Discovered;

#[derive(Clone)]
pub enum Discovery {
    Available(Discovered),
    Unavailable { reason: &'static str },
}

impl Discovery {
    pub fn nodes(&self) -> Vec<DiscoveredNode> {
        Vec::new()
    }
}

pub fn start_listener() -> Discovery {
    Discovery::Unavailable {
        reason: "UDP LAN discovery is unavailable on portable targets",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_is_explicitly_unavailable() {
        assert!(matches!(start_listener(), Discovery::Unavailable { .. }));
    }

    #[test]
    fn normalize_empty_is_default() {
        assert_eq!(normalize_fleet(""), DEFAULT_FLEET);
        assert_eq!(normalize_fleet(" Game "), "game");
    }
}
