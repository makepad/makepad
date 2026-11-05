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
//!   are off, and a visible run is on. So a lane's app never takes the headset link,
//!   the printer or LAN rooms from the dock's app unless the lane asks (`MAKEPAD_LAN=1`).
//!
//! `MAKEPAD_NET_AUDIT=1` logs every decision (`[net-audit] lan allow-lan|refuse|pass <site> <target>`) and, from
//! [`crate::socket_audit`], every socket the process holds.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;

#[derive(Clone, Copy)]
struct ConnectPolicy {
    host: fn(&str, &str) -> std::io::Result<()>,
    addr: fn(&str, SocketAddr) -> std::io::Result<()>,
}

thread_local! {
    static CONNECT_POLICIES: std::cell::RefCell<Vec<ConnectPolicy>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Apply additional checks to synchronous transports that connect through this module,
/// including TLS and WebSocket handshakes. Nested policies all apply; none escape the call.
pub fn with_connect_policy<T>(
    host: fn(&str, &str) -> std::io::Result<()>,
    addr: fn(&str, SocketAddr) -> std::io::Result<()>,
    connect: impl FnOnce() -> T,
) -> T {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            CONNECT_POLICIES.with(|policies| { let _ = policies.borrow_mut().pop(); });
        }
    }
    CONNECT_POLICIES.with(|policies| policies.borrow_mut().push(ConnectPolicy { host, addr }));
    let _restore = Restore;
    connect()
}

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
    connect_policy_addr(site, *addr)?;
    check(site, *addr)?;
    std::net::TcpStream::connect_timeout(addr, timeout)
}

/// Resolve `host:port` and connect to the first address that answers, every address behind
/// [`check`] (`timeout` per address; `None` = the OS's own).
pub fn connect_host(site: &str, host: &str, port: u16, timeout: Option<std::time::Duration>) -> std::io::Result<std::net::TcpStream> {
    connect_host_with(site, host, port, timeout, check_host, check)
}

fn connect_host_with(
    site: &str,
    host: &str,
    port: u16,
    timeout: Option<std::time::Duration>,
    host_check: fn(&str, &str) -> std::io::Result<()>,
    addr_check: fn(&str, SocketAddr) -> std::io::Result<()>,
) -> std::io::Result<std::net::TcpStream> {
    use std::net::ToSocketAddrs;
    connect_policy_host(site, host)?;
    host_check(site, host)?;
    let mut last = None;
    for addr in (host.trim_start_matches('[').trim_end_matches(']'), port).to_socket_addrs()? {
        let attempt = connect_policy_addr(site, addr).and_then(|_| addr_check(site, addr)).and_then(|_| match timeout {
            Some(timeout) => std::net::TcpStream::connect_timeout(&addr, timeout),
            None => std::net::TcpStream::connect(addr),
        });
        match attempt {
            Ok(tcp) => return Ok(tcp),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no address")))
}

fn connect_policy_host(site: &str, host: &str) -> std::io::Result<()> {
    CONNECT_POLICIES.with(|policies| {
        policies.borrow().iter().try_for_each(|policy| (policy.host)(site, host))
    })
}

fn connect_policy_addr(site: &str, addr: SocketAddr) -> std::io::Result<()> {
    CONNECT_POLICIES.with(|policies| {
        policies.borrow().iter().try_for_each(|policy| (policy.addr)(site, addr))
    })
}

/// The Android socket factory owns DNS/TLS on the Java side. Check names before
/// handing them over, and check resolved destinations against the caller's policy.
#[cfg(target_os = "android")]
pub(crate) fn check_platform_connect(site: &str, host: &str, port: &str) -> std::io::Result<()> {
    use std::net::ToSocketAddrs;
    connect_policy_host(site, host)?;
    check_host(site, host)?;
    let port: u16 = port.parse().map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad port"))?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    for addr in (bare, port).to_socket_addrs()? {
        connect_policy_addr(site, addr)?;
        check(site, addr)?;
    }
    Ok(())
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
    check_url_with(site, url, check_host)
}

fn check_url_with(site: &str, url: &str, host_check: fn(&str, &str) -> std::io::Result<()>) -> std::io::Result<()> {
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
    host_check(site, host)
}

/// For LAN-only services (discovery listeners, beacons, room directories, voice links): may
/// `site` start at all? Logs the decision under `MAKEPAD_NET_AUDIT=1`.
pub fn service(site: &str) -> bool {
    let ok = allowed();
    audit(site, "lan service", true, ok);
    ok
}

/// A hub's LAN service needs a fleet-server role or the user setting, plus process LAN permission.
pub fn hub_service(site: &str) -> bool {
    let on = crate::netconfig::hub_networked();
    let ok = hub_service_with(on, || service(site));
    hub_audit(site, "lan service", true, ok, on);
    ok
}

fn hub_service_with(on: bool, service: impl FnOnce() -> bool) -> bool {
    on && service()
}

/// User-machine hub connections keep loopback and routed addresses available with the setting off.
pub fn hub_check(site: &str, addr: SocketAddr) -> std::io::Result<()> {
    let on = crate::netconfig::hub_networked();
    let local = is_local_network(addr.ip());
    let result = hub_check_with(site, addr, on, || check(site, addr));
    hub_audit(site, &addr.to_string(), local, result.is_ok(), on);
    result
}

fn hub_check_with(site: &str, addr: SocketAddr, on: bool, check: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
    hub_permission(site, &addr.to_string(), is_local_network(addr.ip()), on).and_then(|_| check())
}

pub fn hub_connect_timeout(site: &str, addr: &SocketAddr, timeout: std::time::Duration) -> std::io::Result<std::net::TcpStream> {
    hub_check(site, *addr)?;
    connect_timeout(site, addr, timeout)
}

pub fn hub_connect_host(site: &str, host: &str, port: u16, timeout: Option<std::time::Duration>) -> std::io::Result<std::net::TcpStream> {
    connect_host_with(site, host, port, timeout, hub_check_host, hub_check)
}

/// Check before name resolution, so a `.local` name cannot trigger mDNS while off.
pub fn hub_check_host(site: &str, host: &str) -> std::io::Result<()> {
    let on = crate::netconfig::hub_networked();
    let local = !may_reach_host_when_off(host);
    let result = hub_permission(site, host, local, on).and_then(|_| check_host(site, host));
    hub_audit(site, host, local, result.is_ok(), on);
    result
}

pub fn hub_check_url(site: &str, url: &str) -> std::io::Result<()> {
    check_url_with(site, url, hub_check_host)
}

pub fn hub_may_reach_host(host: &str) -> bool {
    may_reach_host_when_off(host) || (crate::netconfig::hub_networked() && may_reach_host(host))
}

/// Wildcard and other non-loopback listeners expose the hub to peers.
pub fn hub_bind_ok(addr: SocketAddr) -> bool {
    let on = crate::netconfig::hub_networked();
    let local = !addr.ip().to_canonical().is_loopback();
    let ok = hub_bind_ok_with(addr, on);
    hub_audit("ai-hub bind", &addr.to_string(), local, ok, on);
    ok
}

fn hub_bind_ok_with(addr: SocketAddr, on: bool) -> bool {
    addr.ip().to_canonical().is_loopback() || on
}

fn hub_permission(site: &str, target: &str, local: bool, on: bool) -> std::io::Result<()> {
    if local && !on {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{site}: {target} needs Networked AI hub ({} = on in network.conf) (this is a user machine; toggle in runner Settings or set {} = server for a fleet node)", crate::netconfig::NETWORKED_AI_HUB_KEY, crate::netconfig::AI_HUB_ROLE_KEY),
        ))
    } else {
        Ok(())
    }
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
    audit_rule("lan", site, target, local, ok, "");
}

fn hub_audit(site: &str, target: &str, local: bool, ok: bool, on: bool) {
    if crate::socket_audit::enabled() {
        let role = match crate::netconfig::hub_role() {
            crate::netconfig::HubRole::FleetServer => "fleet-server",
            crate::netconfig::HubRole::UserMachine => "user",
        };
        let setting = if crate::netconfig::networked_ai_hub() { "on" } else { "off" };
        let networked = if on { "on" } else { "off" };
        audit_rule("hub", site, target, local, ok, &format!(" networked_ai_hub={setting} role={role} hub_networked={networked}"));
    }
}

fn audit_rule(rule: &str, site: &str, target: &str, local: bool, ok: bool, setting: &str) {
    if crate::socket_audit::enabled() {
        crate::socket_audit::start_if_enabled();
        let verdict = if !ok { "refuse" } else if local { "allow-lan" } else { "pass" };
        eprintln!("[net-audit] {rule} {verdict} {site} {target}{setting}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_roles_with_user_setting_off() {
        use crate::netconfig::{hub_networked_from, HubRole};

        let wildcard = SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 8123));
        let loopback = SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 8123));
        let lan = SocketAddr::from((std::net::Ipv4Addr::BROADCAST, 8123));
        for role in [HubRole::FleetServer, HubRole::UserMachine] {
            let server = role == HubRole::FleetServer;
            let on = hub_networked_from(server, || false);
            assert_eq!(hub_bind_ok_with(wildcard, on), server);
            assert_eq!(hub_bind_ok_with(lan, on), server);
            assert!(hub_bind_ok_with(loopback, on));
            assert_eq!(hub_service_with(on, || true), server);
            assert!(!hub_service_with(on, || false));
            let result = hub_check_with("test", lan, on, || Ok(()));
            assert_eq!(result.is_ok(), server);
            if let Err(error) = result {
                assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
                assert!(error.to_string().contains("this is a user machine"));
            }
            assert!(hub_check_with("test", loopback, on, || Ok(())).is_ok());
            assert!(hub_check_with("test", lan, on, || Err(refusal("test", "LAN"))).is_err());
        }
    }

    #[test]
    fn hub_off_refuses_lan_before_connect_or_resolution() {
        for host in ["10.77.0.76", "192.168.77.2", "224.0.0.251", "hub.local", "HUB.LOCAL.", "fd00::1", "::ffff:192.168.77.2"] {
            let error = hub_permission("test", host, !may_reach_host_when_off(host), false).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            assert!(error.to_string().contains(crate::netconfig::NETWORKED_AI_HUB_KEY));
            if let Ok(ip) = host.parse::<IpAddr>() {
                assert!(hub_permission("test", host, is_local_network(ip), false).is_err());
            }
            assert!(hub_permission("test", host, !may_reach_host_when_off(host), true).is_ok());
        }
        for host in ["127.0.0.1", "::1", "8.8.8.8", "203.0.113.1"] {
            assert!(hub_permission("test", host, !may_reach_host_when_off(host), false).is_ok());
            assert!(hub_permission("test", host, is_local_network(host.parse().unwrap()), false).is_ok());
        }
    }

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
            assert!(check_url("t", "https://10.77.0.76:8123/health").is_ok());
        } else {
            assert!(check_url("t", "https://10.77.0.76:8123/health").is_err());
            assert!(check_url("t", "http://[fd00::1]:80/").is_err());
            assert!(check_url("t", "http://studio.local/x").is_err());
        }
        assert!(check_url("t", "https://example.com/a?b").is_ok());
        assert!(check_url("t", "http://127.0.0.1:8080/").is_ok());
        assert!(check_url("t", "http://user@[::1]:80/").is_ok());
    }

    #[test]
    fn local_network_addresses() {
        for ip in ["10.77.0.76", "192.168.77.2", "172.16.77.1", "169.254.1.1", "255.255.255.255", "224.0.0.251", "fd00::1", "fe80::1", "ff02::fb", "::ffff:10.77.0.1"] {
            assert!(is_local_network(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["127.0.0.1", "::1", "8.8.8.8", "203.0.113.1", "172.32.0.1", "2001:db8::1", "0.0.0.0"] {
            assert!(!is_local_network(ip.parse().unwrap()), "{ip}");
        }
    }
}
