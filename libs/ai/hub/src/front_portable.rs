//! The TLS front is native-only; on the web only the caller type exists.

use crate::fleet_auth::Role;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub client_id: String,
    pub role: Role,
    /// The calling app's name (`X-Hub-App` header), for the control panel.
    pub app: Option<String>,
}

impl Caller {
    pub fn local() -> Self {
        Caller { client_id: "local".into(), role: Role::Lan, app: None }
    }
}

pub fn caller_from_headers(_lines: &[String], _front_secret: &str) -> Option<Caller> {
    None
}

pub fn peer_from_headers(_headers: &makepad_network::HttpServerHeaders) -> std::net::IpAddr {
    std::net::IpAddr::from([127, 0, 0, 1])
}

pub fn is_loopback(ip: std::net::IpAddr) -> bool {
    ip.is_loopback()
}
