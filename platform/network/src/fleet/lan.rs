//! The bind rule of a fleet listener: a private LAN address only. RFC 1918
//! IPv4 or an IPv6 unique-local address; never unspecified (0.0.0.0, ::),
//! loopback (that would bypass the front's credentials for anything else on
//! the machine), link-local, or anything routable.

use std::net::{IpAddr, SocketAddr};

pub fn is_private_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private(),
        IpAddr::V6(ip) => (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Whether a datagram from `ip` is from this network: a private LAN address,
/// link-local, or this machine (loopback). The rule for whom a fleet
/// listener ANSWERS (a Sandbox room's probe responder); binding stays
/// [`check_lan_bind`].
pub fn is_lan_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_loopback(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// `Ok` for a private LAN address; else why not, naming `service`.
pub fn check_lan_bind(service: &str, addr: SocketAddr) -> Result<(), String> {
    if is_private_lan(addr.ip()) {
        Ok(())
    } else {
        Err(format!(
            "{service} serves the network only on a private LAN address (10/8, 172.16/12, 192.168/16, fc00::/7) \
             through the fleet TLS front; {} is not one: give it this machine's LAN IP",
            addr.ip()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_peers_are_private_link_local_or_this_machine() {
        for ip in ["10.0.0.5", "192.168.1.2", "172.20.0.1", "127.0.0.1", "169.254.1.1", "fd00::1", "fe80::1", "::1"] {
            assert!(is_lan_peer(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "172.32.0.1", "2001:db8::1"] {
            assert!(!is_lan_peer(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn binds_must_be_private_lan_addresses() {
        for ok in ["10.0.0.5:8123", "192.168.1.2:1", "172.16.0.1:2", "[fd00::1]:8123"] {
            assert!(check_lan_bind("t", ok.parse().unwrap()).is_ok(), "{ok}");
        }
        for bad in ["0.0.0.0:8123", "127.0.0.1:8123", "8.8.8.8:443", "[::]:8123", "[::1]:1", "[2001:db8::1]:1", "169.254.1.1:1"] {
            assert!(check_lan_bind("t", bad.parse().unwrap()).is_err(), "{bad}");
        }
    }
}
