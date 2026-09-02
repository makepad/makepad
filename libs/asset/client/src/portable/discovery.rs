//! Wire-only discovery vocabulary; portable builds perform no UDP I/O.

use crate::wire::{
    caps, BEACON_LEN, DISCOVERY_MAGIC, FLAG_AUTH_REQUIRED, FLAG_TLS,
};
use crate::{ClientError, ClientMode, ClientResult};
use std::net::{IpAddr, SocketAddr, UdpSocket};

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
        let mut bytes = [0u8; BEACON_LEN];
        bytes[..8].copy_from_slice(&DISCOVERY_MAGIC);
        bytes[8..10].copy_from_slice(&self.protocol_version.to_be_bytes());
        bytes[10..26].copy_from_slice(&self.server_id);
        bytes[26..28].copy_from_slice(&self.control_port.to_be_bytes());
        bytes[28..30].copy_from_slice(&self.data_port.to_be_bytes());
        let flags = (if self.auth_required { FLAG_AUTH_REQUIRED } else { 0 })
            | (if self.tls { FLAG_TLS } else { 0 });
        bytes[30..32].copy_from_slice(&flags.to_be_bytes());
        bytes[32..].copy_from_slice(&self.capability_bits.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != BEACON_LEN || bytes[..8] != DISCOVERY_MAGIC {
            return None;
        }
        let protocol_version = u16::from_be_bytes([bytes[8], bytes[9]]);
        let control_port = u16::from_be_bytes([bytes[26], bytes[27]]);
        let data_port = u16::from_be_bytes([bytes[28], bytes[29]]);
        if protocol_version == 0 || control_port == 0 || data_port == 0 {
            return None;
        }
        let mut server_id = [0u8; 16];
        server_id.copy_from_slice(&bytes[10..26]);
        let flags = u16::from_be_bytes([bytes[30], bytes[31]]);
        Some(Self {
            protocol_version,
            server_id,
            control_port,
            data_port,
            auth_required: flags & FLAG_AUTH_REQUIRED != 0,
            tls: flags & FLAG_TLS != 0,
            capability_bits: u32::from_be_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]),
        })
    }
}

pub fn content_client_caps() -> u32 {
    caps::CATALOG | caps::BLOBS
}

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

pub struct DiscoveryListener;

pub fn bind_reuse_udp(_port: u16) -> std::io::Result<UdpSocket> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "UDP discovery is unavailable on portable targets",
    ))
}

impl DiscoveryListener {
    pub fn start(
        _port: u16,
        _ttl_ms: u64,
        _now_ms: fn() -> u64,
    ) -> ClientResult<Self> {
        Err(ClientError::Unavailable {
            capability: "udp_discovery",
            mode: ClientMode::StaticWeb,
        })
    }

    pub fn port(&self) -> u16 {
        0
    }

    pub fn snapshot(&self, _now_ms: u64) -> Vec<DiscoveredServer> {
        Vec::new()
    }

    pub fn find(&self, _server_id: &[u8; 16], _now_ms: u64) -> Option<DiscoveredServer> {
        None
    }

    pub fn pick(&self, _need: u32, _now_ms: u64) -> Option<DiscoveredServer> {
        None
    }

    pub fn stop(&mut self) {}
}
