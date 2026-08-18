//! UDP LAN discovery: fixed-size beacon, bounded parser, listener cache.
//!
//! Wire format (exactly [`BEACON_LEN`] bytes, anything else is dropped):
//!
//! ```text
//! magic "MPASDIS1"        8 bytes
//! protocol_version        u16 be   (non-zero)
//! server_id               16 bytes (persisted random; NOT a secret)
//! control_port            u16 be
//! data_port               u16 be
//! flags                   u16 be   bit0 = auth_required, bit1 = tls
//! capability_bits         u32 be   (see `caps`)
//! ```
//!
//! The beacon deliberately advertises ONLY these fields. No hostname, path,
//! catalog content, prompt, username, token, or free-form string can even be
//! expressed. The server's IP is never in the payload either: listeners
//! derive endpoints from the UDP sender address, so a beacon cannot redirect
//! a client to a third host. Discovery is a HINT — a client must still
//! health-check and authenticate over HTTP before trusting anything.

use super::util::{from_hex_exact, log, rand16, to_hex};
use crate::{ServerError, ServerResult};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

pub const DISCOVERY_MAGIC: [u8; 8] = *b"MPASDIS1";
pub const BEACON_LEN: usize = 36;
pub const PROTOCOL_VERSION: u16 = 1;

const FLAG_AUTH_REQUIRED: u16 = 1 << 0;
const FLAG_TLS: u16 = 1 << 1;

/// Coarse service-class bits a beacon may advertise. Bounded on purpose:
/// capability is a bitset, never a string.
pub mod caps {
    pub const CATALOG: u32 = 1 << 0;
    pub const BLOBS: u32 = 1 << 1;
    pub const JOBS: u32 = 1 << 2;
    pub const GAMES: u32 = 1 << 3;
    pub const ALL_V1: u32 = CATALOG | BLOBS | JOBS | GAMES;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beacon {
    pub protocol_version: u16,
    pub server_id: [u8; 16],
    pub control_port: u16,
    pub data_port: u16,
    pub auth_required: bool,
    pub tls: bool,
    pub capability_bits: u32,
}

impl Beacon {
    pub fn encode(&self) -> [u8; BEACON_LEN] {
        let mut b = [0u8; BEACON_LEN];
        b[0..8].copy_from_slice(&DISCOVERY_MAGIC);
        b[8..10].copy_from_slice(&self.protocol_version.to_be_bytes());
        b[10..26].copy_from_slice(&self.server_id);
        b[26..28].copy_from_slice(&self.control_port.to_be_bytes());
        b[28..30].copy_from_slice(&self.data_port.to_be_bytes());
        let mut flags = 0u16;
        if self.auth_required {
            flags |= FLAG_AUTH_REQUIRED;
        }
        if self.tls {
            flags |= FLAG_TLS;
        }
        b[30..32].copy_from_slice(&flags.to_be_bytes());
        b[32..36].copy_from_slice(&self.capability_bits.to_be_bytes());
        b
    }

    /// Bounded parse: exact length, exact magic, non-zero version. Unknown
    /// flag bits are ignored; nothing in the payload sizes an allocation.
    pub fn decode(bytes: &[u8]) -> Option<Beacon> {
        if bytes.len() != BEACON_LEN || bytes[0..8] != DISCOVERY_MAGIC {
            return None;
        }
        let protocol_version = u16::from_be_bytes([bytes[8], bytes[9]]);
        if protocol_version == 0 {
            return None;
        }
        let mut server_id = [0u8; 16];
        server_id.copy_from_slice(&bytes[10..26]);
        let control_port = u16::from_be_bytes([bytes[26], bytes[27]]);
        let data_port = u16::from_be_bytes([bytes[28], bytes[29]]);
        if control_port == 0 || data_port == 0 {
            return None;
        }
        let flags = u16::from_be_bytes([bytes[30], bytes[31]]);
        let capability_bits = u32::from_be_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]);
        Some(Beacon {
            protocol_version,
            server_id,
            control_port,
            data_port,
            auth_required: flags & FLAG_AUTH_REQUIRED != 0,
            tls: flags & FLAG_TLS != 0,
            capability_bits,
        })
    }
}

/// Load the persisted server identity, or mint and persist a random one.
/// The id is public (it is broadcast), stable across restarts, and carries
/// no information about the machine.
pub fn load_or_create_server_id(root: &Path, log_enabled: bool) -> ServerResult<[u8; 16]> {
    let path = root.join("server-id");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Some(id) = from_hex_exact::<16>(text.trim()) {
            return Ok(id);
        }
        log(log_enabled, "server-id file malformed; minting a new identity");
    }
    let id = rand16()?;
    std::fs::create_dir_all(root)
        .map_err(|e| ServerError::Io { op: "create root for server-id", kind: e.kind() })?;
    std::fs::write(&path, format!("{}\n", to_hex(&id)))
        .map_err(|e| ServerError::Io { op: "write server-id", kind: e.kind() })?;
    Ok(id)
}

/// Spawn the periodic beacon sender. Returns a stop sender (send or drop to
/// stop promptly) and the join handle.
pub fn spawn_beacon(
    beacon: Beacon,
    target_ip: IpAddr,
    port: u16,
    interval_ms: u64,
    log_enabled: bool,
) -> ServerResult<(mpsc::Sender<()>, std::thread::JoinHandle<()>)> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .map_err(|e| ServerError::Io { op: "bind beacon socket", kind: e.kind() })?;
    socket
        .set_broadcast(true)
        .map_err(|e| ServerError::Io { op: "beacon set broadcast", kind: e.kind() })?;
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let payload = beacon.encode();
    let dest = SocketAddr::new(target_ip, port);
    let interval = Duration::from_millis(interval_ms.max(10));
    let join = std::thread::Builder::new()
        .name("asset-server-beacon".into())
        .spawn(move || {
            let mut failed = false;
            loop {
                match socket.send_to(&payload, dest) {
                    Ok(_) => failed = false,
                    Err(e) => {
                        if !failed {
                            log(log_enabled, &format!("discovery beacon send failed: {}", e.kind()));
                        }
                        failed = true;
                    }
                }
                match stop_rx.recv_timeout(interval) {
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    _ => break,
                }
            }
        })
        .map_err(|e| ServerError::Io { op: "spawn beacon thread", kind: e.kind() })?;
    Ok((stop_tx, join))
}

/// One server as seen by a listener. Endpoints combine the beacon's ports
/// with the SENDER's IP, never anything inside the payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredServer {
    pub server_id: [u8; 16],
    pub protocol_version: u16,
    pub ip: IpAddr,
    pub control_port: u16,
    pub data_port: u16,
    pub auth_required: bool,
    pub tls: bool,
    pub capability_bits: u32,
    pub last_seen_ms: u64,
}

impl DiscoveredServer {
    pub fn control_endpoint(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.control_port)
    }
    pub fn data_endpoint(&self) -> SocketAddr {
        SocketAddr::new(self.ip, self.data_port)
    }
}

/// Bounded discovery cache + receive thread. Dedup is by server_id (a server
/// beaconing every 2s occupies one entry); entries expire after `ttl_ms` and
/// the cache holds at most [`DiscoveryListener::MAX_ENTRIES`], evicting the
/// stalest, so a beacon flood cannot grow memory.
pub struct DiscoveryListener {
    port: u16,
    cache: Arc<Mutex<HashMap<[u8; 16], DiscoveredServer>>>,
    stopping: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    ttl_ms: u64,
}

impl DiscoveryListener {
    pub const MAX_ENTRIES: usize = 256;

    /// Bind and start receiving. `port` 0 binds an ephemeral port (tests);
    /// the actual port is reported by [`port`](Self::port).
    pub fn start(port: u16, ttl_ms: u64) -> ServerResult<DiscoveryListener> {
        let socket = UdpSocket::bind(("0.0.0.0", port))
            .map_err(|e| ServerError::Io { op: "bind discovery listener", kind: e.kind() })?;
        let actual = socket
            .local_addr()
            .map_err(|e| ServerError::Io { op: "discovery local addr", kind: e.kind() })?
            .port();
        socket
            .set_read_timeout(Some(Duration::from_millis(250)))
            .map_err(|e| ServerError::Io { op: "discovery read timeout", kind: e.kind() })?;
        let cache: Arc<Mutex<HashMap<[u8; 16], DiscoveredServer>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let cache_t = cache.clone();
        let stopping_t = stopping.clone();
        let join = std::thread::Builder::new()
            .name("asset-server-discovery".into())
            .spawn(move || {
                let mut buf = [0u8; 64];
                while !stopping_t.load(Ordering::Relaxed) {
                    let (len, src) = match socket.recv_from(&mut buf) {
                        Ok(r) => r,
                        Err(e) => match e.kind() {
                            std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted => continue,
                            _ => break,
                        },
                    };
                    let Some(beacon) = Beacon::decode(&buf[..len]) else {
                        continue;
                    };
                    let seen = DiscoveredServer {
                        server_id: beacon.server_id,
                        protocol_version: beacon.protocol_version,
                        ip: src.ip(),
                        control_port: beacon.control_port,
                        data_port: beacon.data_port,
                        auth_required: beacon.auth_required,
                        tls: beacon.tls,
                        capability_bits: beacon.capability_bits,
                        last_seen_ms: super::util::now_ms(),
                    };
                    let mut cache = match cache_t.lock() {
                        Ok(c) => c,
                        Err(_) => break,
                    };
                    if !cache.contains_key(&beacon.server_id) && cache.len() >= Self::MAX_ENTRIES {
                        if let Some(oldest) = cache
                            .values()
                            .min_by_key(|s| s.last_seen_ms)
                            .map(|s| s.server_id)
                        {
                            cache.remove(&oldest);
                        }
                    }
                    cache.insert(beacon.server_id, seen);
                }
            })
            .map_err(|e| ServerError::Io { op: "spawn discovery thread", kind: e.kind() })?;
        Ok(DiscoveryListener { port: actual, cache, stopping, join: Some(join), ttl_ms })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Live servers (expired entries pruned), sorted by server_id for
    /// stable presentation.
    pub fn snapshot(&self, now_ms: u64) -> Vec<DiscoveredServer> {
        let mut cache = match self.cache.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        cache.retain(|_, srv| srv.last_seen_ms.saturating_add(self.ttl_ms) >= now_ms);
        let mut out: Vec<DiscoveredServer> = cache.values().cloned().collect();
        out.sort_by(|a, b| a.server_id.cmp(&b.server_id));
        out
    }

    pub fn stop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for DiscoveryListener {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beacon() -> Beacon {
        Beacon {
            protocol_version: PROTOCOL_VERSION,
            server_id: [7u8; 16],
            control_port: 9701,
            data_port: 9702,
            auth_required: true,
            tls: false,
            capability_bits: caps::ALL_V1,
        }
    }

    #[test]
    fn roundtrip_and_exact_length() {
        let b = beacon();
        let bytes = b.encode();
        assert_eq!(bytes.len(), BEACON_LEN);
        assert_eq!(Beacon::decode(&bytes), Some(b));
    }

    #[test]
    fn hostile_payloads_ignored() {
        let good = beacon().encode();
        assert!(Beacon::decode(&good[..35]).is_none()); // short
        let mut long = good.to_vec();
        long.push(0);
        assert!(Beacon::decode(&long).is_none()); // long
        let mut bad_magic = good;
        bad_magic[0] = b'X';
        assert!(Beacon::decode(&bad_magic).is_none());
        let mut zero_version = beacon();
        zero_version.protocol_version = 0;
        assert!(Beacon::decode(&zero_version.encode()).is_none());
        let mut zero_port = beacon();
        zero_port.control_port = 0;
        assert!(Beacon::decode(&zero_port.encode()).is_none());
        assert!(Beacon::decode(&[0u8; BEACON_LEN]).is_none());
    }

    #[test]
    fn field_allowlist_is_total() {
        // The beacon is exactly 36 bytes of fixed fields: any attempt to
        // carry a string (path, title, token) is unrepresentable.
        let b = beacon().encode();
        let decoded = Beacon::decode(&b).unwrap();
        assert_eq!(decoded.capability_bits & !caps::ALL_V1, 0);
    }
}
