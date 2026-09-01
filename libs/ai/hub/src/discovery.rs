//! LAN autodiscovery: start a fleet node and it announces itself. The
//! service broadcasts a small UDP beacon every two seconds; clients listen
//! and treat the live set as the fleet.
//!
//! Dedup: every service start mints a random `node_id`, carried in BOTH the
//! beacon and `/health`, so a box reachable under two addresses (127.0.0.1
//! from its own machine, LAN ip from everywhere) collapses to one fleet
//! entry instead of double-booking the GPU.

use makepad_micro_serde::*;
use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Fixed LAN port beacons are sent to and clients listen on.
pub const DISCOVERY_PORT: u16 = 41830;
/// Backend/frontend partition. Empty or omitted fleet is this value, so an
/// unscoped box (today's asset-ui / .169) stays on its own lane.
/// The one fleet every real deployment runs ("gen"): frontends with no
/// `MAKEPAD_AI_FLEET` and nodes with no `--fleet` meet here by default,
/// so an app hears the LAN GPU fleet without per-app env plumbing.
pub const DEFAULT_FLEET: &str = "gen";

/// Lowercase trimmed fleet name. Empty becomes [`DEFAULT_FLEET`].
pub fn normalize_fleet(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        DEFAULT_FLEET.to_string()
    } else {
        trimmed.to_ascii_lowercase()
    }
}

/// Frontend filter: `MAKEPAD_AI_FLEET`, else [`DEFAULT_FLEET`].
pub fn wanted_fleet() -> String {
    normalize_fleet(&std::env::var("MAKEPAD_AI_FLEET").unwrap_or_default())
}

/// Backend name: `MAKEPAD_ASSET_AI_FLEET`, else [`DEFAULT_FLEET`].
pub fn fleet_from_env() -> String {
    normalize_fleet(&std::env::var("MAKEPAD_ASSET_AI_FLEET").unwrap_or_default())
}
const BEACON_INTERVAL: Duration = Duration::from_secs(2);
/// A node whose beacon has been silent this long drops out of the
/// discovered set (its fleet snapshot then goes down via health polling).
const BEACON_EXPIRY: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, SerJson, DeJson)]
pub struct BeaconJson {
    /// Constant "makepad-asset-ai" — receivers ignore anything else.
    pub service: String,
    /// Random per-service-start id; matches the `/health` `node_id`.
    pub node_id: u64,
    /// The service's HTTP port on the sender's address.
    pub port: u16,
    /// Partition this box belongs to. Missing on older beacons = default.
    pub fleet: Option<String>,
}

/// Random-enough node id: wall-clock nanos xor pid. Uniqueness only needs
/// to hold across a handful of LAN boxes.
pub fn mint_node_id() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

/// Service side: broadcast a beacon every [`BEACON_INTERVAL`] until the
/// process exits. Failures log once and end the thread — discovery is an
/// extra, never a crash. `MAKEPAD_AI_NO_BEACON=1` disables.
pub fn start_beacon(node_id: u64, http_port: u16, fleet: String) {
    if std::env::var_os("MAKEPAD_AI_NO_BEACON").is_some_and(|v| v == "1") {
        return;
    }
    std::thread::spawn(move || {
        let socket = match UdpSocket::bind(("0.0.0.0", 0)) {
            Ok(socket) => socket,
            Err(e) => {
                eprintln!("discovery: beacon socket bind failed: {e} — no LAN announce");
                return;
            }
        };
        if let Err(e) = socket.set_broadcast(true) {
            eprintln!("discovery: SO_BROADCAST failed: {e} — no LAN announce");
            return;
        }
        let beacon = BeaconJson {
            service: "makepad-asset-ai".to_string(),
            node_id,
            port: http_port,
            fleet: Some(normalize_fleet(&fleet)),
        }
        .serialize_json();
        loop {
            // Send errors are transient (interface down mid-sleep etc.) —
            // keep beating rather than giving up.
            let _ = socket.send_to(beacon.as_bytes(), ("255.255.255.255", DISCOVERY_PORT));
            std::thread::sleep(BEACON_INTERVAL);
        }
    });
}

/// One discovered node: url is derived from the beacon's SOURCE address +
/// advertised port.
#[derive(Clone, Debug)]
pub struct DiscoveredNode {
    pub base_url: String,
    pub node_id: u64,
    pub fleet: String,
}

/// Live set of discovered nodes, shared with the listener thread.
#[derive(Clone, Default)]
pub struct Discovered {
    nodes: Arc<Mutex<HashMap<u64, (String, Instant)>>>,
}

impl Discovered {
    /// Currently-live discovered nodes (beacon seen within expiry).
    pub fn nodes(&self) -> Vec<DiscoveredNode> {
        let mut map = self.nodes.lock().unwrap();
        map.retain(|_, (_, seen)| seen.elapsed() < BEACON_EXPIRY);
        map.iter()
            .map(|(node_id, (base_url, _))| DiscoveredNode {
                base_url: base_url.clone(),
                node_id: *node_id,
                fleet: wanted_fleet(),
            })
            .collect()
    }
}

/// Client side: listen for beacons on [`DISCOVERY_PORT`]. Returns the shared
/// set; poll it from the fleet timer. The bind joins the reuse group, so the
/// VJ, the Asset UI and a game on one machine all see the fleet (an
/// exclusive bind left every app but the first with an empty set that never
/// filled); a bind failure still logs and returns that empty set.
///
/// ONE listener per process: every caller gets a handle to the same socket
/// and set. The job loop re-opens its LAN fleet source on every refresh,
/// and a fresh bind per call leaked a socket plus a thread each time (the
/// Asset UI sat on 22 sockets bound to :41830 after ten minutes, growing).
/// The filter reads `MAKEPAD_AI_FLEET` per beacon, so sharing across
/// callers changes nothing they observe.
pub fn start_listener() -> Discovered {
    static SHARED: OnceLock<Discovered> = OnceLock::new();
    SHARED.get_or_init(spawn_listener).clone()
}

fn spawn_listener() -> Discovered {
    let discovered = Discovered::default();
    let nodes = discovered.nodes.clone();
    std::thread::spawn(move || {
        let socket = match makepad_asset_client::bind_reuse_udp(DISCOVERY_PORT) {
            Ok(socket) => socket,
            Err(e) => {
                eprintln!(
                    "discovery: listener bind :{DISCOVERY_PORT} failed: {e} — no LAN fleet"
                );
                return;
            }
        };
        let mut buffer = [0u8; 512];
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buffer) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&buffer[..len]) else {
                continue;
            };
            let Ok(beacon) = BeaconJson::deserialize_json(text) else {
                continue;
            };
            if beacon.service != "makepad-asset-ai" {
                continue;
            }
            let fleet = normalize_fleet(beacon.fleet.as_deref().unwrap_or(""));
            if fleet != wanted_fleet() {
                continue;
            }
            let base_url = format!("http://{}:{}", from.ip(), beacon.port);
            nodes
                .lock()
                .unwrap()
                .insert(beacon.node_id, (base_url, Instant::now()));
        }
    });
    discovered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacon_roundtrip() {
        let beacon = BeaconJson {
            service: "makepad-asset-ai".to_string(),
            node_id: 0xDEAD_BEEF,
            port: 8767,
            fleet: Some("game".to_string()),
        };
        let json = beacon.serialize_json();
        let back = BeaconJson::deserialize_json(&json).unwrap();
        assert_eq!(back.node_id, 0xDEAD_BEEF);
        assert_eq!(back.port, 8767);
        assert_eq!(back.service, "makepad-asset-ai");
        assert_eq!(back.fleet.as_deref(), Some("game"));
    }

    #[test]
    fn normalize_empty_is_default() {
        assert_eq!(normalize_fleet(""), DEFAULT_FLEET);
        assert_eq!(normalize_fleet(" Game "), "game");
    }

    #[test]
    fn node_ids_differ_across_mints() {
        // Nanos advance between calls; equal ids would mean a broken clock.
        let a = mint_node_id();
        std::thread::sleep(Duration::from_millis(2));
        let b = mint_node_id();
        assert_ne!(a, b);
    }

    #[test]
    fn listener_is_one_per_process() {
        let a = start_listener();
        let b = start_listener();
        assert!(Arc::ptr_eq(&a.nodes, &b.nodes));
    }

    #[test]
    fn discovered_set_expires_and_lists() {
        let discovered = Discovered::default();
        discovered
            .nodes
            .lock()
            .unwrap()
            .insert(7, ("http://10.0.0.9:8767".to_string(), Instant::now()));
        let nodes = discovered.nodes();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].base_url, "http://10.0.0.9:8767");
        // Backdate past expiry: the node drops out.
        discovered.nodes.lock().unwrap().get_mut(&7).unwrap().1 =
            Instant::now() - BEACON_EXPIRY - Duration::from_secs(1);
        assert!(discovered.nodes().is_empty());
    }
}
