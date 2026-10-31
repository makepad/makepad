//! The TLS front is native-only; on the web only the caller type exists.

use super::id::Role;

/// The front's peer stamp header (see front.rs); a web build only reads it.
pub const HEADER_PEER: &str = "X-Makepad-Peer";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub client_id: String,
    pub role: Role,
    pub admin: bool,
    pub this_machine: bool,
    /// The calling app's name (`X-Hub-App` header), for the control panel.
    pub app: Option<String>,
}

impl Caller {
    pub fn local() -> Self {
        Caller { client_id: "local".into(), role: Role::Device, admin: true, this_machine: true, app: None }
    }
}

pub fn caller_from_headers(_lines: &[String], _front_secret: &str) -> Option<Caller> {
    None
}

pub fn peer_from_headers(_headers: &crate::HttpServerHeaders) -> std::net::IpAddr {
    std::net::IpAddr::from([127, 0, 0, 1])
}

pub fn is_loopback(ip: std::net::IpAddr) -> bool {
    ip.is_loopback()
}
