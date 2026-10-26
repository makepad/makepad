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

/// The beacon payload of a node serving behind its TLS front. It travels
/// signed by the node's fleet key ([`makepad_network::fleet::id::Me::announce`],
/// service [`ANNOUNCE_SERVICE`]); listeners drop beacons that do not check
/// out against the fleet root. A client then reaches the advertised port
/// over TLS (the certificate checked against the root) and proves its own
/// credential to it.
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
/// The service name a beacon is signed for.
pub const ANNOUNCE_SERVICE: &str = "ai-hub";

/// The running beacon: [`Beacon::stop`] (or a drop) ends it, so a hub switched off in-process
/// is no longer announced (the desktop AI Hub app's Enabled switch).
pub struct Beacon {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Beacon {
    pub fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

impl Drop for Beacon {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_fleet_beacon(node_id: u64, tls_port: u16, fleet: String, node_key: String) -> Option<Beacon> {
    if std::env::var_os("MAKEPAD_AI_NO_BEACON").is_some_and(|v| v == "1")
        || !makepad_network::lan::service("ai-hub fleet beacon")
    {
        return None;
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopping = stop.clone();
    let thread = std::thread::Builder::new().name("ai-hub-beacon".into()).spawn(move || {
        let Ok(socket) = UdpSocket::bind(("0.0.0.0", 0)) else {
            eprintln!("discovery: beacon socket bind failed — no LAN announce");
            return;
        };
        if socket.set_broadcast(true).is_err() {
            return;
        }
        // Listeners accept only beacons signed by a fleet machine: without
        // an identity this node is not announced.
        let Some(me) = makepad_network::fleet::id::me() else {
            eprintln!("discovery: no LAN announce: {}", makepad_network::fleet::id::why_no_identity());
            return;
        };
        let payload = FleetBeaconJson {
            service: FLEET_SERVICE.to_string(),
            node_id,
            port: tls_port,
            fleet: normalize_fleet(&fleet),
            node_key,
        }
        .serialize_json();
        let mut warned_sign = false;
        while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
            // Signed every round: an announcement carries its time.
            match me.announce(ANNOUNCE_SERVICE, payload.as_bytes()) {
                Ok(beacon) => {
                    // 255.255.255.255 leaves by the default route only; each up
                    // interface's directed broadcast reaches its own subnet too (a
                    // Wi-Fi hotspot this machine hosts). Interfaces are read every
                    // round: a hotspot may come up after the node.
                    for target in beacon_targets() {
                        let _ = socket.send_to(&beacon, (target, DISCOVERY_PORT));
                    }
                }
                Err(e) if !warned_sign => {
                    warned_sign = true;
                    eprintln!("discovery: signing the beacon failed: {e}");
                }
                Err(_) => {}
            }
            std::thread::park_timeout(BEACON_INTERVAL);
        }
    }).ok()?;
    Some(Beacon { stop, thread: Some(thread) })
}

/// The TLS port a fleet node serves on unless configured otherwise.
pub const DEFAULT_TLS_PORT: u16 = 8123;

/// Fleet nodes where no beacon reaches: a hub listening at `port` on a
/// gateway of this machine (the routing table's, else each interface's first
/// host) that takes a TCP connection within `timeout`. A phone on a laptop's
/// hotspot hears no beacon when the laptop broadcasts out of another
/// interface, but the laptop is its gateway. The URLs are marked as fleet
/// endpoints and reached through the same TLS / fleet-root / credential
/// path as a beacon's: nothing is trusted for being a gateway.
pub fn gateway_nodes(port: u16, timeout: Duration) -> Vec<String> {
    if !makepad_network::lan::service("ai-hub gateway probe") {
        return Vec::new();
    }
    let candidates = makepad_core_util::netif::gateway_candidates();
    let probes: Vec<_> = candidates
        .into_iter()
        .map(|ip| {
            std::thread::spawn(move || {
                let addr = std::net::SocketAddr::from((ip, port));
                std::net::TcpStream::connect_timeout(&addr, timeout).ok().map(|_| ip)
            })
        })
        .collect();
    let mut out = Vec::new();
    for probe in probes {
        if let Ok(Some(ip)) = probe.join() {
            let host_port = format!("{ip}:{port}");
            makepad_network::fleet::id::mark_fleet_endpoint(&host_port);
            out.push(format!("https://{host_port}"));
        }
    }
    out
}

/// This machine's IPv4 addresses: a change means another network (a
/// client forgets which nodes were unreachable).
pub fn network_addresses() -> Vec<std::net::Ipv4Addr> {
    makepad_core_util::netif::interfaces().into_iter().map(|i| i.addr).collect()
}

/// Whether `host:port` takes a TCP connection within `timeout` (a
/// remembered hub address is only a hint: elsewhere it may not exist).
pub fn answers(host_port: &str, timeout: Duration) -> bool {
    use std::net::ToSocketAddrs;
    let Ok(addrs) = host_port.to_socket_addrs() else { return false };
    addrs.into_iter().any(|addr| {
        makepad_network::lan::check("ai-hub hint probe", addr).is_ok()
            && std::net::TcpStream::connect_timeout(&addr, timeout).is_ok()
    })
}

/// Where a LAN broadcast goes (a beacon, a Sandbox room probe): the limited
/// broadcast, then every up interface's directed broadcast.
pub fn beacon_targets() -> Vec<std::net::Ipv4Addr> {
    let mut out = vec![std::net::Ipv4Addr::BROADCAST];
    for interface in makepad_core_util::netif::interfaces() {
        let target = interface.broadcast();
        if !out.contains(&target) {
            out.push(target);
        }
    }
    out
}

/// The beacon payload of a signed announcement from `from` that checks out
/// against `fleet` and is for [`ANNOUNCE_SERVICE`]; None for anything else.
pub fn check_fleet_beacon(fleet: &makepad_network::fleet::id::Fleet, bytes: &[u8], from: std::net::IpAddr, now: u64) -> Option<FleetBeaconJson> {
    let (_sender, service, payload) = fleet.check_announcement(bytes, from, now).ok()?;
    if service != ANNOUNCE_SERVICE {
        return None;
    }
    FleetBeaconJson::deserialize_json(std::str::from_utf8(payload).ok()?).ok()
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
    makepad_network::fleet::id::mark_fleet_endpoint(&host_port);
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
/// callers changes nothing they observe. After [`stop_listener`] the next
/// call starts it again (the set is kept).
///
/// Receiving the fleet's broadcast beacons is a local-network operation: where
/// [`makepad_network::lan::allowed`] says no (a hidden instance), discovery is
/// unavailable and nothing binds.
pub fn start_listener() -> Discovery {
    if !makepad_network::lan::service("ai-hub fleet discovery") {
        return Discovery::Unavailable { reason: "local network off for this process (MAKEPAD_LAN=1 allows it)" };
    }
    let mut slot = LISTENER.lock().unwrap_or_else(|e| e.into_inner());
    let set = DISCOVERED.get_or_init(Discovered::default).clone();
    // a failed bind is said once and not retried until a stop (another app holds the port)
    let failed = BIND_FAILED.load(std::sync::atomic::Ordering::Relaxed);
    if !failed && slot.as_ref().map_or(true, |l| l.thread.is_finished()) {
        *slot = Listener::spawn(set.clone(), DISCOVERY_PORT);
        if slot.is_none() {
            BIND_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
    Discovery::Available(set)
}

/// Ends the listener thread and waits for it (an app that quits: under `makepad run` the app's
/// domain waits for every thread it started, and this one blocked in its receive for good, so
/// every quit of an app that had looked for the fleet waited out the stop's 5 s). The wait is
/// woken by one datagram over loopback to a socket of the listener's own -- no timeout, no
/// polling, nothing on the network.
pub fn stop_listener() {
    BIND_FAILED.store(false, std::sync::atomic::Ordering::Relaxed);
    let listener = LISTENER.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(listener) = listener {
        listener.stop();
    }
}

static DISCOVERED: OnceLock<Discovered> = OnceLock::new();
static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);
static BIND_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Listener {
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// where one datagram wakes the listener's wait: its private loopback socket (unix), or the
    /// discovery port itself on loopback (Windows binds that port exclusively: it is this
    /// listener's own)
    wake: std::net::SocketAddr,
    thread: std::thread::JoinHandle<()>,
}

impl Listener {
    /// Wakes the listener's wait, which sees the stop flag, and joins it.
    fn stop(self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Ok(waker) = UdpSocket::bind(("127.0.0.1", 0)) {
            let _ = waker.send_to(&[0], self.wake);
        }
        let _ = self.thread.join();
    }

    fn spawn(discovered: Discovered, port: u16) -> Option<Listener> {
        let nodes = discovered.nodes.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let socket = match makepad_core_util::udp::bind_reuse_udp(port) {
            Ok(socket) => socket,
            Err(e) => {
                eprintln!("discovery: listener bind :{port} failed: {e} — no LAN fleet");
                return None;
            }
        };
        #[cfg(unix)]
        let (waker, wake) = {
            let waker = UdpSocket::bind(("127.0.0.1", 0)).ok()?;
            let wake = waker.local_addr().ok()?;
            (waker, wake)
        };
        #[cfg(not(unix))]
        let wake = std::net::SocketAddr::from(([127, 0, 0, 1], socket.local_addr().ok()?.port()));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new()
            .name("ai-hub-discovery".into())
            .spawn(move || {
                let mut buffer = [0u8; 2048];
                let mut warned_no_identity = false;
                loop {
                    #[cfg(unix)]
                    match makepad_core_util::udp::wait_readable(&socket, &waker) {
                        Ok((_, true)) | Err(_) => return,
                        Ok((false, false)) => continue,
                        Ok((true, false)) => {}
                    }
                    let received = socket.recv_from(&mut buffer);
                    if stopped.load(std::sync::atomic::Ordering::Acquire) {
                        return;
                    }
                    let Ok((len, from)) = received else {
                        continue;
                    };
                    if !makepad_network::fleet::id::is_announcement(&buffer[..len]) {
                        continue;
                    }
                    // A node is only of use to (and checkable by) a process
                    // with a fleet identity.
                    let (Some(_), Some(fleet)) = (makepad_network::fleet::id::me(), makepad_network::fleet::id::fleet()) else {
                        if !warned_no_identity {
                            warned_no_identity = true;
                            eprintln!("discovery: fleet nodes found, but {}", makepad_network::fleet::id::why_no_identity());
                        }
                        continue;
                    };
                    let Some(beacon) = check_fleet_beacon(fleet, &buffer[..len], from.ip(), makepad_network::fleet::id::now_secs()) else {
                        continue;
                    };
                    let Some(base_url) = accept_fleet_beacon(&beacon, from.ip(), &wanted_fleet()) else {
                        continue;
                    };
                    nodes
                        .lock()
                        .unwrap()
                        .insert(beacon.node_id, (base_url, Instant::now()));
                }
            })
            .ok()?;
        Some(Listener { stop, wake, thread })
    }
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
        assert!(makepad_network::fleet::id::is_fleet_endpoint("10.9.9.9:8123"));
        assert!(accept_fleet_beacon(&beacon, "10.9.9.8".parse().unwrap(), "game").is_none());
        assert!(!makepad_network::fleet::id::is_fleet_endpoint("10.9.9.8:8123"));
    }

    #[test]
    fn fleet_beacons_are_signed_by_a_fleet_machine() {
        use makepad_network::fleet::id::{generate_key, now_secs, point_of, Fleet, Me, Role, Root};
        let root = Root::generate().unwrap();
        let fleet = Fleet::new(root.point(), Default::default());
        let key = generate_key().unwrap();
        let node = Me::new(key, root.issue("n217", Role::Node, now_secs() + 3600, None, &point_of(&key)).unwrap()).unwrap();
        let payload = FleetBeaconJson {
            service: FLEET_SERVICE.into(),
            node_id: 7,
            port: 8123,
            fleet: "gen".into(),
            node_key: "00".into(),
        }
        .serialize_json();
        let from: std::net::IpAddr = "10.9.9.7".parse().unwrap();
        let signed = node.announce(ANNOUNCE_SERVICE, payload.as_bytes()).unwrap();
        assert_eq!(check_fleet_beacon(&fleet, &signed, from, now_secs()).map(|b| b.node_id), Some(7));
        // Another service, another fleet, an unsigned beacon: dropped.
        let room = node.announce("sandbox-room", payload.as_bytes()).unwrap();
        assert!(check_fleet_beacon(&fleet, &room, from, now_secs()).is_none());
        let stranger = Fleet::new(Root::generate().unwrap().point(), Default::default());
        assert!(check_fleet_beacon(&stranger, &signed, from, now_secs()).is_none());
        assert!(check_fleet_beacon(&fleet, payload.as_bytes(), from, now_secs()).is_none());
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
            // a hidden / MAKEPAD_LAN=0 test run never listens on the LAN
            assert!(!makepad_network::lan::allowed(), "native discovery unexpectedly unavailable");
            return;
        };
        assert!(Arc::ptr_eq(&a.nodes, &b.nodes));
    }

    /// A listener waiting in its receive with nothing on the network ends promptly when stopped
    /// (an app's quit waits for it), and its set stays.
    #[test]
    fn a_listener_stops_without_any_beacon() {
        let set = Discovered::default();
        // an ephemeral port: no beacon reaches it, and no other listener on this machine shares it
        let listener = Listener::spawn(set.clone(), 0).expect("listener");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!listener.thread.is_finished(), "the listener waits for beacons");
        let (done, stopped) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            listener.stop();
            let _ = done.send(());
        });
        assert!(stopped.recv_timeout(Duration::from_secs(5)).is_ok(), "stopping the listener did not return");
        assert!(set.nodes().is_empty());
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
