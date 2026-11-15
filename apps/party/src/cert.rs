//! The box's HTTPS identity: a self-signed P-256 certificate on the names
//! guests type or scan (its LAN address, a public address given by the host,
//! any extra names), kept in `<makepad home>/party/tls` so a phone that
//! accepted it once is not asked again after a restart.

use makepad_network::tls::{SubjectName, TlsIdentity};
use std::io;
use std::net::{IpAddr, UdpSocket};
use std::path::PathBuf;

pub fn identity_dir() -> PathBuf {
    makepad_network::fleet::makepad_home().join("party").join("tls")
}

/// The address this box's default route leaves from. A UDP socket
/// "connected" to an address sends nothing; 192.0.2.1 is a documentation
/// address, routed by the default route and never reached.
pub fn primary_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// The names the certificate carries: the LAN address, then `extra` (IP
/// addresses or host names, such as the router's public address), then
/// loopback.
pub fn names(lan: Option<IpAddr>, extra: &[String]) -> Vec<SubjectName> {
    let mut names = Vec::new();
    let mut push = |name: SubjectName| {
        if !names.contains(&name) {
            names.push(name);
        }
    };
    if let Some(ip) = lan {
        push(SubjectName::Ip(ip));
    }
    for name in extra {
        match name.parse::<IpAddr>() {
            Ok(ip) => push(SubjectName::Ip(ip)),
            Err(_) => push(SubjectName::Dns(name.to_ascii_lowercase())),
        }
    }
    push(SubjectName::Ip([127, 0, 0, 1].into()));
    push(SubjectName::Dns("localhost".into()));
    names
}

pub fn load(names: &[SubjectName]) -> io::Result<TlsIdentity> {
    TlsIdentity::load_or_create_for_names(&identity_dir(), "Makepad party", names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lead_with_the_lan_address_and_skip_repeats() {
        let lan: IpAddr = "192.0.2.10".parse().unwrap();
        let got = names(Some(lan), &["198.51.100.4".into(), "Party.Example".into(), "192.0.2.10".into()]);
        assert_eq!(
            got,
            vec![
                SubjectName::Ip(lan),
                SubjectName::Ip("198.51.100.4".parse().unwrap()),
                SubjectName::Dns("party.example".into()),
                SubjectName::Ip([127, 0, 0, 1].into()),
                SubjectName::Dns("localhost".into()),
            ]
        );
    }
}
