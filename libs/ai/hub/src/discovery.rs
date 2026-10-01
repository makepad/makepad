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

/// Random-enough node id: wall-clock nanos xor pid. Uniqueness only needs
/// to hold across a handful of LAN boxes.
pub fn mint_node_id() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ ((std::process::id() as u64) << 32)
}

/// The beacon of a node serving behind its TLS front. Nothing in it is
/// trusted: a client reaches the advertised port over TLS, records the
/// certificate (known nodes) and proves its own credential to it.
#[derive(Clone, Debug, SerJson, DeJson)]
pub struct FleetBeaconJson {
    /// Constant "makepad-ai-hub-tls".
    pub service: String,
    pub node_id: u64,
    /// The TLS front's port on the sender's address.
    pub port: u16,
    pub fleet: String,
    pub node_key: String,
}

pub const FLEET_SERVICE: &str = "makepad-ai-hub-tls";

pub fn start_fleet_beacon(node_id: u64, tls_port: u16, fleet: String, node_key: String) {
    if std::env::var_os("MAKEPAD_AI_NO_BEACON").is_some_and(|v| v == "1") {
        return;
    }
    std::thread::spawn(move || {
        let Ok(socket) = UdpSocket::bind(("0.0.0.0", 0)) else {
            eprintln!("discovery: beacon socket bind failed — no LAN announce");
            return;
        };
        if socket.set_broadcast(true).is_err() {
            return;
        }
        let beacon = FleetBeaconJson {
            service: FLEET_SERVICE.to_string(),
            node_id,
            port: tls_port,
            fleet: normalize_fleet(&fleet),
            node_key,
        }
        .serialize_json();
        loop {
            let _ = socket.send_to(beacon.as_bytes(), ("255.255.255.255", DISCOVERY_PORT));
            std::thread::sleep(BEACON_INTERVAL);
        }
    });
}

/// A beacon for the wanted fleet from `from`: the node's TLS endpoint,
/// marked as a fleet endpoint; returns its URL.
pub fn accept_fleet_beacon(beacon: &FleetBeaconJson, from: std::net::IpAddr, wanted_fleet: &str) -> Option<String> {
    if beacon.service != FLEET_SERVICE || normalize_fleet(&beacon.fleet) != wanted_fleet || beacon.port == 0 {
        return None;
    }
    let host = match from {
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        v4 => v4.to_string(),
    };
    let host_port = format!("{host}:{}", beacon.port);
    crate::fleet_auth::mark_fleet_endpoint(&host_port);
    Some(format!("https://{host_port}"))
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

/// Platform discovery availability. Portable targets return
/// [`Unavailable`](Self::Unavailable) rather than attempting UDP I/O.
#[derive(Clone)]
pub enum Discovery {
    Available(Discovered),
    Unavailable { reason: &'static str },
}

impl Discovery {
    pub fn nodes(&self) -> Vec<DiscoveredNode> {
        match self {
            Self::Available(discovered) => discovered.nodes(),
            Self::Unavailable { .. } => Vec::new(),
        }
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
pub fn start_listener() -> Discovery {
    static SHARED: OnceLock<Discovered> = OnceLock::new();
    Discovery::Available(SHARED.get_or_init(spawn_listener).clone())
}

fn spawn_listener() -> Discovered {
    let discovered = Discovered::default();
    let nodes = discovered.nodes.clone();
    std::thread::spawn(move || {
        let socket = match makepad_core_util::udp::bind_reuse_udp(DISCOVERY_PORT) {
            Ok(socket) => socket,
            Err(e) => {
                eprintln!(
                    "discovery: listener bind :{DISCOVERY_PORT} failed: {e} — no LAN fleet"
                );
                return;
            }
        };
        let mut buffer = [0u8; 2048];
        let mut warned_no_credential = false;
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buffer) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&buffer[..len]) else {
                continue;
            };
            let Ok(beacon) = FleetBeaconJson::deserialize_json(text) else {
                continue;
            };
            // A node is only of use to a process with a fleet credential.
            if crate::fleet_auth::own_credential().is_none() {
                if !warned_no_credential {
                    warned_no_credential = true;
                    eprintln!("discovery: fleet nodes found, but this process has no fleet credential (~/.makepad/ai-hub/client.credential or MAKEPAD_AI_HUB_CREDENTIAL; see tools/aihub-fleet.md)");
                }
                continue;
            }
            let Some(base_url) = accept_fleet_beacon(&beacon, from.ip(), &wanted_fleet()) else {
                continue;
            };
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
    fn fleet_beacons_mark_tls_endpoints() {
        let beacon = FleetBeaconJson {
            service: FLEET_SERVICE.into(),
            node_id: 1,
            port: 8123,
            fleet: "gen".into(),
            node_key: "00112233445566778899aabbccddeeff".into(),
        };
        let back = FleetBeaconJson::deserialize_json(&beacon.serialize_json()).unwrap();
        assert_eq!(back.port, 8123);
        let from: std::net::IpAddr = "10.9.9.9".parse().unwrap();
        assert_eq!(accept_fleet_beacon(&beacon, from, "gen").as_deref(), Some("https://10.9.9.9:8123"));
        assert!(crate::fleet_auth::is_fleet_endpoint("10.9.9.9:8123"));
        assert!(accept_fleet_beacon(&beacon, "10.9.9.8".parse().unwrap(), "game").is_none());
        assert!(!crate::fleet_auth::is_fleet_endpoint("10.9.9.8:8123"));
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
        let (Discovery::Available(a), Discovery::Available(b)) = (a, b) else {
            panic!("native discovery unexpectedly unavailable");
        };
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
