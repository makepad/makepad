//! May this process touch the local network? THE one rule, asked by every code path that would
//! (fleet AI discovery and its beacons, gateway probes, connections to LAN hosts, Sandbox rooms,
//! voice links, motion sinks, asset-server discovery and sync).
//!
//! Why it exists: on macOS every such operation (an outgoing TCP connection or UDP send/connect to
//! a LAN address, receiving a UDP broadcast or multicast, resolving a `.local` name, Bonjour) asks
//! the Local Network privacy question ("Allow <app> to find devices on your local network"), and
//! every rebuilt dev binary is a new program to it, so a test box that runs hidden instances asked
//! again for every run. Loopback, routed (internet) addresses, listening for TCP and receiving UDP
//! unicast never ask, and stay open.
//!
//! The rule:
//! - `MAKEPAD_LAN=1`: on; `MAKEPAD_LAN=0`: off (a test machine sets it for every job);
//! - otherwise a hidden instance (`MAKEPAD_HIDE_WINDOWS` set: tests, agent captures, harness runs)
//!   and a lane's test instance (`MAKEPAD_TEST_LANE` set: devhub's lease runs, visible or not)
//!   are off, and a visible run is on. So a lane's Fab or Sandbox never takes the headset link,
//!   the printer or LAN rooms from the dock's app unless the lane asks (`MAKEPAD_LAN=1`).
//!
//! `MAKEPAD_NET_AUDIT=1` logs every decision (`[net-audit] lan allow-lan|refuse|pass <site> <target>`) and, from
//! [`crate::socket_audit`], every socket the process holds.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;

/// Whether this process may use the local network (see the module rule). Read once.
pub fn allowed() -> bool {
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    *ALLOWED.get_or_init(|| {
        let lan = std::env::var("MAKEPAD_LAN").ok();
        let hidden = std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some();
        let test = std::env::var_os("MAKEPAD_TEST_LANE").is_some_and(|l| !l.is_empty());
        let on = decide(lan.as_deref(), hidden || test);
        if !on {
            crate::socket_audit::start_if_enabled();
            eprintln!(
                "lan: local network off ({}): no fleet discovery, LAN hosts or rooms; MAKEPAD_LAN=1 allows it",
                if lan.as_deref().map(str::trim) == Some("0") { "MAKEPAD_LAN=0" } else if test { "a lane's test instance" } else { "hidden instance" }
            );
        }
        on
    })
}

/// The rule as a pure function: `MAKEPAD_LAN` (1 on, 0 off) wins, else a hidden or test
/// instance (`hidden`) is off.
pub fn decide(lan: Option<&str>, hidden: bool) -> bool {
    match lan.map(str::trim) {
        Some("1") => true,
        Some("0") => false,
        _ => !hidden,
    }
}

/// An address macOS counts as the local network (or that is meant for it): private IPv4
/// (10/8, 172.16/12, 192.168/16), link-local, broadcast and multicast; IPv6 unique-local,
/// link-local and multicast. Loopback and routed addresses are not.
pub fn is_local_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local() || v4.is_broadcast() || v4.is_multicast(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_local_network(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || v6.is_multicast()
        }
    }
}

/// May this process send to / connect to `ip`? Always for loopback and routed addresses.
pub fn may_reach(ip: IpAddr) -> bool {
    !is_local_network(ip) || allowed()
}

/// May this process reach `host` (a literal address, or a name; `.local` names are mDNS, i.e.
/// the local network)? Other names are checked again once resolved ([`check`]).
pub fn may_reach_host(host: &str) -> bool {
    may_reach_host_when_off(host) || allowed()
}

/// `Ok` when `addr` may be reached; else a `PermissionDenied` naming `site` and the opt-in.
/// Call it right before a connect or send to an address that may be a LAN host.
pub fn check(site: &str, addr: SocketAddr) -> std::io::Result<()> {
    let ok = may_reach(addr.ip());
    audit(site, &addr.to_string(), is_local_network(addr.ip()), ok);
    if ok {
        Ok(())
    } else {
        Err(refusal(site, &addr.to_string()))
    }
}

/// `TcpStream::connect_timeout` behind [`check`]: THE outgoing TCP connect of any client that
/// may be pointed at a LAN host (fleet nodes, pinned hubs, the asset server).
pub fn connect_timeout(site: &str, addr: &SocketAddr, timeout: std::time::Duration) -> std::io::Result<std::net::TcpStream> {
    check(site, *addr)?;
    std::net::TcpStream::connect_timeout(addr, timeout)
}

/// Resolve `host:port` and connect to the first address that answers, every address behind
/// [`check`] (`timeout` per address; `None` = the OS's own).
pub fn connect_host(site: &str, host: &str, port: u16, timeout: Option<std::time::Duration>) -> std::io::Result<std::net::TcpStream> {
    use std::net::ToSocketAddrs;
    check_host(site, host)?;
    let mut last = None;
    for addr in (host.trim_start_matches('[').trim_end_matches(']'), port).to_socket_addrs()? {
        let attempt = match timeout {
            Some(timeout) => connect_timeout(site, &addr, timeout),
            None => check(site, addr).and_then(|_| std::net::TcpStream::connect(addr)),
        };
        match attempt {
            Ok(tcp) => return Ok(tcp),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address")))
}

/// [`check`] for a host name or literal (before resolving it).
pub fn check_host(site: &str, host: &str) -> std::io::Result<()> {
    let ok = may_reach_host(host);
    audit(site, host, !may_reach_host_when_off(host), ok);
    if ok {
        Ok(())
    } else {
        Err(refusal(site, host))
    }
}

/// [`check_host`] for the host of a URL (`scheme://[user@]host[:port]/...`), before a platform
/// HTTP stack (NSURLSession, WinHTTP) resolves and connects it.
pub fn check_url(site: &str, url: &str) -> std::io::Result<()> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or(v6)
    } else {
        authority.rsplit_once(':').map_or(authority, |(host, _)| host)
    };
    if host.is_empty() {
        return Ok(());
    }
    check_host(site, host)
}

/// For LAN-only services (discovery listeners, beacons, room directories, voice links): may
/// `site` start at all? Logs the decision under `MAKEPAD_NET_AUDIT=1`.
pub fn service(site: &str) -> bool {
    let ok = allowed();
    audit(site, "lan service", true, ok);
    ok
}

fn refusal(site: &str, target: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!("{site}: {target} is on the local network, which is off for this process (MAKEPAD_LAN=1 allows it)"),
    )
}

/// Whether `host` would pass with the local network off (not a LAN literal, not `.local`).
fn may_reach_host_when_off(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return !is_local_network(ip);
    }
    let name = bare.trim_end_matches('.').to_ascii_lowercase();
    !(name.ends_with(".local") || name == "local")
}

/// `[net-audit] lan allow-lan|refuse|pass <site> <target>`: `pass` = not the local network.
fn audit(site: &str, target: &str, local: bool, ok: bool) {
    if crate::socket_audit::enabled() {
        crate::socket_audit::start_if_enabled();
        let verdict = if !ok { "refuse" } else if local { "allow-lan" } else { "pass" };
        eprintln!("[net-audit] lan {verdict} {site} {target}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_is_off_unless_opted_in_and_zero_forces_off() {
        assert!(decide(None, false));
        assert!(!decide(None, true));
        assert!(decide(Some("1"), true));
        assert!(decide(Some(" 1 "), true));
        assert!(!decide(Some("0"), false));
        assert!(!decide(Some("yes"), true));
        assert!(decide(Some(""), false));
    }

    #[test]
    fn url_hosts() {
        // the rule's answer for a visible test process: LAN on, so every url passes
        if allowed() {
            assert!(check_url("t", "https://10.0.0.165:8123/health").is_ok());
        } else {
            assert!(check_url("t", "https://10.0.0.165:8123/health").is_err());
            assert!(check_url("t", "http://[fd00::1]:80/").is_err());
            assert!(check_url("t", "http://studio.local/x").is_err());
        }
        assert!(check_url("t", "https://example.com/a?b").is_ok());
        assert!(check_url("t", "http://127.0.0.1:8080/").is_ok());
        assert!(check_url("t", "http://user@[::1]:80/").is_ok());
    }

    #[test]
    fn local_network_addresses() {
        for ip in ["10.0.0.165", "192.168.1.2", "172.16.0.1", "169.254.1.1", "255.255.255.255", "224.0.0.251", "fd00::1", "fe80::1", "ff02::fb", "::ffff:10.0.0.1"] {
            assert!(is_local_network(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["127.0.0.1", "::1", "8.8.8.8", "203.0.113.1", "172.32.0.1", "2001:db8::1", "0.0.0.0"] {
            assert!(!is_local_network(ip.parse().unwrap()), "{ip}");
        }
    }
}
