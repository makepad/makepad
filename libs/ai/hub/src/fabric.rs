//! MPHUB1: the fabric beacon dedicated nodes announce on (aicore.md §4).
//!
//! Wire format (exactly [`BEACON_LEN`] bytes, anything else is dropped):
//!
//! ```text
//! magic "MPHUB1\0\0"      8 bytes
//! protocol_version        u16 be  (non-zero)
//! node_id                 16 bytes (random per service start)
//! machine_id              16 bytes (persisted per machine; NOT a secret)
//! port                    u16 be  (non-zero)
//! flags                   u16 be  bit0 = dedicated, bit1 = auth_required
//! capability_bits         u32 be  (see `caps` — a bitset, never a string)
//! load                    u8      (coarse 0..=255 busyness)
//! pipes_hash              u32 be  (change = re-fetch GET /pipes)
//! fleet_hash              u64 be  (hash of the normalized fleet name — the
//!                                  partition survives without carrying a string)
//! ```
//!
//! The beacon deliberately advertises ONLY these fields — no hostname, path,
//! model name, or free-form string can even be expressed, and the sender's IP
//! is never in the payload: listeners derive endpoints from the UDP sender
//! address, so a beacon cannot redirect a peer to a third host. Discovery is
//! a HINT — a peer must still authenticate over the connection (the fabric
//! secret) before trusting anything. This is the `MPASDIS1` trust model,
//! carried over verbatim.
//!
//! **The §4 law lives in the types**: there is no function in this crate that
//! announces an app node. [`spawn_dedicated_beacon`] is the only sender, it
//! takes a [`DedicatedNode`], and constructing one is the explicit act of
//! being a LAN-facing node (the `apps/ai-hub` binary / machine node). An
//! in-process app hub simply has no path to the LAN.

use crate::discovery::normalize_fleet;
use crate::machine;
use crate::sha256::Sha256;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::time::Duration;

pub const FABRIC_MAGIC: [u8; 8] = *b"MPHUB1\0\0";
pub const BEACON_LEN: usize = 63;
pub const PROTOCOL_VERSION: u16 = 1;
/// Fixed LAN port MPHUB1 beacons are sent to. 41830 is the legacy JSON
/// beacon (`crate::discovery`); both run during the migration window.
pub const FABRIC_PORT: u16 = 41831;
/// Beacon cadence and expiry — the legacy constants, unchanged (aicore §4).
pub const BEACON_INTERVAL: Duration = Duration::from_secs(2);
pub const BEACON_EXPIRY: Duration = Duration::from_secs(15);

const FLAG_DEDICATED: u16 = 1 << 0;
const FLAG_AUTH_REQUIRED: u16 = 1 << 1;

/// Coarse capability domains. Bounded on purpose: capability is a bitset,
/// never a string; the full pipe list rides `GET /pipes` behind auth.
pub mod caps {
    pub const LLM: u32 = 1 << 0;
    pub const IMAGE: u32 = 1 << 1;
    pub const VIDEO: u32 = 1 << 2;
    pub const AUDIO: u32 = 1 << 3;
    pub const SPEECH: u32 = 1 << 4;
    pub const MESH: u32 = 1 << 5;
    pub const VISION: u32 = 1 << 6;
    pub const WORLD: u32 = 1 << 7;
}

/// The stable per-machine identity, persisted beside the machine token.
/// Public (it is broadcast) and meaningless off this LAN.
pub fn machine_id() -> io::Result<[u8; 16]> {
    machine::load_or_create_id("machine-id")
}

/// The partition a fleet name maps to on the wire. Stable across releases:
/// changing this orphans every mixed-version LAN.
pub fn fleet_hash(name: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"mphub-fleet:");
    hasher.update(normalize_fleet(name).as_bytes());
    let digest = hasher.finish();
    u64::from_be_bytes(digest[..8].try_into().unwrap())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beacon {
    pub protocol_version: u16,
    pub node_id: [u8; 16],
    pub machine_id: [u8; 16],
    pub port: u16,
    pub dedicated: bool,
    pub auth_required: bool,
    pub capability_bits: u32,
    pub load: u8,
    pub pipes_hash: u32,
    pub fleet_hash: u64,
}

impl Beacon {
    pub fn encode(&self) -> [u8; BEACON_LEN] {
        let mut b = [0u8; BEACON_LEN];
        b[0..8].copy_from_slice(&FABRIC_MAGIC);
        b[8..10].copy_from_slice(&self.protocol_version.to_be_bytes());
        b[10..26].copy_from_slice(&self.node_id);
        b[26..42].copy_from_slice(&self.machine_id);
        b[42..44].copy_from_slice(&self.port.to_be_bytes());
        let mut flags = 0u16;
        if self.dedicated {
            flags |= FLAG_DEDICATED;
        }
        if self.auth_required {
            flags |= FLAG_AUTH_REQUIRED;
        }
        b[44..46].copy_from_slice(&flags.to_be_bytes());
        b[46..50].copy_from_slice(&self.capability_bits.to_be_bytes());
        b[50] = self.load;
        b[51..55].copy_from_slice(&self.pipes_hash.to_be_bytes());
        b[55..63].copy_from_slice(&self.fleet_hash.to_be_bytes());
        b
    }

    /// Bounded parse: exact length, exact magic, non-zero version and port.
    /// Unknown flag bits are ignored; nothing in the payload sizes an
    /// allocation.
    pub fn decode(bytes: &[u8]) -> Option<Beacon> {
        if bytes.len() != BEACON_LEN || bytes[0..8] != FABRIC_MAGIC {
            return None;
        }
        let protocol_version = u16::from_be_bytes([bytes[8], bytes[9]]);
        if protocol_version == 0 {
            return None;
        }
        let mut node_id = [0u8; 16];
        node_id.copy_from_slice(&bytes[10..26]);
        let mut machine_id = [0u8; 16];
        machine_id.copy_from_slice(&bytes[26..42]);
        let port = u16::from_be_bytes([bytes[42], bytes[43]]);
        if port == 0 {
            return None;
        }
        let flags = u16::from_be_bytes([bytes[44], bytes[45]]);
        let capability_bits =
            u32::from_be_bytes([bytes[46], bytes[47], bytes[48], bytes[49]]);
        let load = bytes[50];
        let pipes_hash = u32::from_be_bytes([bytes[51], bytes[52], bytes[53], bytes[54]]);
        let fleet_hash = u64::from_be_bytes(bytes[55..63].try_into().unwrap());
        Some(Beacon {
            protocol_version,
            node_id,
            machine_id,
            port,
            dedicated: flags & FLAG_DEDICATED != 0,
            auth_required: flags & FLAG_AUTH_REQUIRED != 0,
            capability_bits,
            load,
            pipes_hash,
            fleet_hash,
        })
    }
}

/// The explicit act of being LAN-facing. Only the dedicated binary and the
/// machine node construct one; an in-process app hub cannot announce because
/// no announcing API accepts anything else.
pub struct DedicatedNode {
    pub node_id: [u8; 16],
    pub machine_id: [u8; 16],
    pub port: u16,
    pub auth_required: bool,
    pub fleet: String,
}

/// Live, mutable-per-tick beacon facts the sender polls each interval.
pub trait BeaconFacts: Send + 'static {
    fn capability_bits(&self) -> u32;
    /// Coarse busyness 0..=255 (lanes active / queue depth scaled).
    fn load(&self) -> u8;
    fn pipes_hash(&self) -> u32;
}

/// Spawn the periodic MPHUB1 sender for a dedicated node. Returns a stop
/// sender (send or drop to stop within one interval) and the join handle.
pub fn spawn_dedicated_beacon(
    node: DedicatedNode,
    facts: Box<dyn BeaconFacts>,
) -> io::Result<(mpsc::Sender<()>, std::thread::JoinHandle<()>)> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))?;
    socket.set_broadcast(true)?;
    let fleet_hash = fleet_hash(&node.fleet);
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let handle = std::thread::Builder::new()
        .name("ai-hub-fabric-beacon".into())
        .spawn(move || loop {
            let beacon = Beacon {
                protocol_version: PROTOCOL_VERSION,
                node_id: node.node_id,
                machine_id: node.machine_id,
                port: node.port,
                dedicated: true,
                auth_required: node.auth_required,
                capability_bits: facts.capability_bits(),
                load: facts.load(),
                pipes_hash: facts.pipes_hash(),
                fleet_hash,
            };
            let target = SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), FABRIC_PORT);
            let _ = socket.send_to(&beacon.encode(), target);
            match stop_rx.recv_timeout(BEACON_INTERVAL) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        })?;
    Ok((stop_tx, handle))
}

/// One received beacon plus where it physically came from — the ONLY source
/// of the peer's address, per the trust model above.
#[derive(Clone, Copy, Debug)]
pub struct Sighting {
    pub from: SocketAddr,
    pub beacon: Beacon,
}

/// Drain every parseable beacon currently queued on `socket` (non-blocking).
/// Filtering (fleet, expiry bookkeeping) is the caller's; this only decodes.
pub fn drain_sightings(socket: &UdpSocket, out: &mut Vec<Sighting>) {
    let mut buf = [0u8; 128];
    socket.set_nonblocking(true).ok();
    while let Ok((len, from)) = socket.recv_from(&mut buf) {
        if let Some(beacon) = Beacon::decode(&buf[..len]) {
            out.push(Sighting { from, beacon });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn beacon() -> Beacon {
        Beacon {
            protocol_version: PROTOCOL_VERSION,
            node_id: [7u8; 16],
            machine_id: [9u8; 16],
            port: 8765,
            dedicated: true,
            auth_required: true,
            capability_bits: caps::LLM | caps::IMAGE,
            load: 130,
            pipes_hash: 0xDEAD_BEEF,
            fleet_hash: fleet_hash("default"),
        }
    }

    #[test]
    fn roundtrip() {
        let b = beacon();
        assert_eq!(Beacon::decode(&b.encode()), Some(b));
    }

    #[test]
    fn bounded_parse_refuses_junk() {
        let b = beacon().encode();
        assert_eq!(Beacon::decode(&b[..BEACON_LEN - 1]), None, "short");
        let mut long = b.to_vec();
        long.push(0);
        assert_eq!(Beacon::decode(&long), None, "long");
        let mut wrong_magic = b;
        wrong_magic[0] = b'X';
        assert_eq!(Beacon::decode(&wrong_magic), None, "magic");
        let mut zero_version = b;
        zero_version[8] = 0;
        zero_version[9] = 0;
        assert_eq!(Beacon::decode(&zero_version), None, "version 0");
        let mut zero_port = b;
        zero_port[42] = 0;
        zero_port[43] = 0;
        assert_eq!(Beacon::decode(&zero_port), None, "port 0");
    }

    #[test]
    fn fleet_hash_is_normalized_and_stable() {
        assert_eq!(fleet_hash("Gen"), fleet_hash("gen"));
        // Empty normalizes to the DEFAULT fleet — which is 'gen' now, the
        // one every real deployment runs.
        assert_eq!(fleet_hash(""), fleet_hash("gen"));
        assert_ne!(fleet_hash("gen"), fleet_hash("chat"));
    }

    #[test]
    fn unknown_flag_bits_are_ignored() {
        let mut b = beacon().encode();
        b[44] |= 0x80; // future flag
        let decoded = Beacon::decode(&b).unwrap();
        assert!(decoded.dedicated);
    }
}
