//! Client half of UDP LAN discovery: bounded beacon parsing, a capped
//! listener cache, and deterministic candidate selection.
//!
//! Discovery is a HINT, never an authority. A beacon can only say "a server
//! with this id and these ports exists at the address these UDP bytes came
//! from"; endpoints always combine the beacon's ports with the SENDER's IP,
//! so a payload cannot redirect a client to a third host. Nothing a beacon
//! carries is trusted until [`crate::AssetClient::connect`] health-checks the
//! endpoint over HTTP, verifies the server identity matches, and probes the
//! caller's credential. See `libs/asset/store/src/discovery.rs` for the
//! authoritative wire notes; the 36-byte layout is mirrored in
//! [`crate::wire`].

use crate::error::{io_err, ClientError, ClientResult};
use crate::wire::{
    caps, BEACON_LEN, DISCOVERY_MAGIC, FLAG_AUTH_REQUIRED, FLAG_TLS, PROTOCOL_VERSION,
};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    /// Encode is used by tests and embedded announcers; the client itself
    /// only ever decodes.
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

    /// Bounded parse: exact length, exact magic, non-zero version and ports.
    /// Unknown flag bits are ignored; nothing in the payload sizes an
    /// allocation.
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

/// One server as seen by this listener. Endpoints combine the beacon's ports
/// with the sender's IP, never anything inside the payload.
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
    /// True when this candidate speaks our protocol and advertises every
    /// capability bit in `need`.
    pub fn usable(&self, need: u32) -> bool {
        self.protocol_version == PROTOCOL_VERSION && self.capability_bits & need == need
    }
}

/// Bounded discovery cache + receive thread. Dedup is by server_id; entries
/// expire after `ttl_ms` and the cache holds at most [`MAX_ENTRIES`],
/// evicting the stalest, so a beacon flood cannot grow memory. The clock is
/// injected at construction so tests are deterministic.
pub struct DiscoveryListener {
    port: u16,
    cache: Arc<Mutex<HashMap<[u8; 16], DiscoveredServer>>>,
    stopping: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    ttl_ms: u64,
}

pub const MAX_ENTRIES: usize = 256;

/// Bind the discovery port so several clients on ONE host can listen at
/// once (VJ + AI Content + a game all discover the same server).
///
/// Unix: `SO_REUSEADDR` + `SO_REUSEPORT` before bind — on macOS and Linux,
/// BROADCAST datagrams (which beacons are) are delivered to every member of
/// the reuse group, so all apps see the server. Raw `extern "C"` because
/// std exposes no pre-bind socket options and this crate takes no external
/// dependencies.
///
/// Windows: deliberately NOT set. `SO_REUSEADDR` on Windows allows silent
/// socket hijacking by other processes (no `SO_REUSEPORT` equivalent with
/// safe semantics), so the port stays exclusive there; a second app's bind
/// fails with `AddrInUse`, which callers surface as an explicit "discovery
/// port already in use — configure explicit endpoints" condition rather
/// than a silent security hole.
#[cfg(unix)]
fn bind_reuse_udp(port: u16) -> std::io::Result<UdpSocket> {
    use std::os::unix::io::FromRawFd;

    const AF_INET: i32 = 2;
    const SOCK_DGRAM: i32 = 2;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const SOL_SOCKET: i32 = 0xffff;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const SOL_SOCKET: i32 = 1;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const SO_REUSEADDR: i32 = 0x0004;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const SO_REUSEADDR: i32 = 2;
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const SO_REUSEPORT: i32 = 0x0200;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const SO_REUSEPORT: i32 = 15;

    extern "C" {
        fn socket(domain: i32, ty: i32, protocol: i32) -> i32;
        fn setsockopt(
            fd: i32,
            level: i32,
            name: i32,
            value: *const core::ffi::c_void,
            len: u32,
        ) -> i32;
        fn bind(fd: i32, addr: *const u8, len: u32) -> i32;
        fn close(fd: i32) -> i32;
    }

    unsafe {
        let fd = socket(AF_INET, SOCK_DGRAM, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fail = |fd: i32| -> std::io::Error {
            let e = std::io::Error::last_os_error();
            close(fd);
            e
        };
        let one: i32 = 1;
        let one_ptr = &one as *const i32 as *const core::ffi::c_void;
        let one_len = std::mem::size_of::<i32>() as u32;
        if setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, one_ptr, one_len) != 0 {
            return Err(fail(fd));
        }
        if setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, one_ptr, one_len) != 0 {
            return Err(fail(fd));
        }
        // sockaddr_in for INADDR_ANY:port. BSD layouts carry a leading
        // sin_len byte; Linux uses a 16-bit sin_family.
        let mut addr = [0u8; 16];
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd"
        ))]
        {
            addr[0] = 16; // sin_len
            addr[1] = AF_INET as u8;
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd"
        )))]
        {
            addr[0..2].copy_from_slice(&(AF_INET as u16).to_ne_bytes());
        }
        addr[2..4].copy_from_slice(&port.to_be_bytes());
        // sin_addr stays 0.0.0.0.
        if bind(fd, addr.as_ptr(), 16) != 0 {
            return Err(fail(fd));
        }
        Ok(UdpSocket::from_raw_fd(fd))
    }
}

#[cfg(not(unix))]
fn bind_reuse_udp(port: u16) -> std::io::Result<UdpSocket> {
    UdpSocket::bind(("0.0.0.0", port))
}

impl DiscoveryListener {
    /// Bind and start receiving. `port` 0 binds an ephemeral port (tests);
    /// the bound port is reported by [`port`](Self::port). `now_ms` stamps
    /// received beacons and is any monotonic-enough millisecond source; real
    /// callers pass [`crate::util::now_ms`].
    pub fn start(
        port: u16,
        ttl_ms: u64,
        now_ms: fn() -> u64,
    ) -> ClientResult<DiscoveryListener> {
        if ttl_ms == 0 {
            return Err(ClientError::InvalidInput { what: "discovery ttl_ms" });
        }
        let socket = bind_reuse_udp(port).map_err(io_err("bind discovery listener"))?;
        let actual = socket
            .local_addr()
            .map_err(io_err("discovery local addr"))?
            .port();
        socket
            .set_read_timeout(Some(Duration::from_millis(250)))
            .map_err(io_err("discovery read timeout"))?;
        let cache: Arc<Mutex<HashMap<[u8; 16], DiscoveredServer>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let cache_t = cache.clone();
        let stopping_t = stopping.clone();
        let join = std::thread::Builder::new()
            .name("asset-client-discovery".into())
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
                        last_seen_ms: now_ms(),
                    };
                    let mut cache = match cache_t.lock() {
                        Ok(c) => c,
                        Err(_) => break,
                    };
                    if !cache.contains_key(&beacon.server_id) && cache.len() >= MAX_ENTRIES {
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
            .map_err(io_err("spawn discovery thread"))?;
        Ok(DiscoveryListener { port: actual, cache, stopping, join: Some(join), ttl_ms })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Live servers (expired entries pruned), sorted by server_id for stable
    /// presentation.
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

    /// The candidate with `server_id`, if currently live.
    pub fn find(&self, server_id: &[u8; 16], now_ms: u64) -> Option<DiscoveredServer> {
        self.snapshot(now_ms).into_iter().find(|s| &s.server_id == server_id)
    }

    /// Deterministic pick for callers with no preference: the most recently
    /// seen usable candidate (needing `need` capability bits), ties broken by
    /// server_id, so the same LAN state always selects the same server.
    pub fn pick(&self, need: u32, now_ms: u64) -> Option<DiscoveredServer> {
        self.snapshot(now_ms)
            .into_iter()
            .filter(|s| s.usable(need))
            .max_by(|a, b| {
                a.last_seen_ms
                    .cmp(&b.last_seen_ms)
                    .then_with(|| b.server_id.cmp(&a.server_id))
            })
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

/// Convenience default-need bits for a content client: catalog + blobs.
pub fn content_client_caps() -> u32 {
    caps::CATALOG | caps::BLOBS
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
        assert!(Beacon::decode(&good[..35]).is_none());
        let mut long = good.to_vec();
        long.push(0);
        assert!(Beacon::decode(&long).is_none());
        let mut bad_magic = good;
        bad_magic[0] = b'X';
        assert!(Beacon::decode(&bad_magic).is_none());
        let mut zero_version = beacon();
        zero_version.protocol_version = 0;
        assert!(Beacon::decode(&zero_version.encode()).is_none());
        let mut zero_port = beacon();
        zero_port.control_port = 0;
        assert!(Beacon::decode(&zero_port.encode()).is_none());
        let mut zero_data = beacon();
        zero_data.data_port = 0;
        assert!(Beacon::decode(&zero_data.encode()).is_none());
        assert!(Beacon::decode(&[0u8; BEACON_LEN]).is_none());
        assert!(Beacon::decode(&[]).is_none());
    }

    #[test]
    fn usable_checks_version_and_caps() {
        let mut d = DiscoveredServer {
            server_id: [1; 16],
            protocol_version: PROTOCOL_VERSION,
            ip: "127.0.0.1".parse().unwrap(),
            control_port: 1,
            data_port: 2,
            auth_required: false,
            tls: false,
            capability_bits: caps::CATALOG | caps::BLOBS,
            last_seen_ms: 0,
        };
        assert!(d.usable(content_client_caps()));
        assert!(!d.usable(caps::ALL_V1));
        d.protocol_version = 99;
        assert!(!d.usable(content_client_caps()));
    }
}
