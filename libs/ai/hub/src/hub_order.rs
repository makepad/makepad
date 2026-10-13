//! Which hub a client talks to, when several answer.
//!
//! Every client (chat, speech, generation) ranks the nodes it knows by the same
//! three tiers and keeps that order for the whole conversation:
//!
//! 0. **this machine's own hub**: a loopback address, or one of this host's own
//!    interface addresses (the loopback beacon and the LAN beacon of one node);
//! 1. **a pinned hub**: `MAKEPAD_AI_HUB` (comma-separated `host`, `host:port` or
//!    URLs) or the lines of `<makepad home>/ai-hub/pinned`: the machine the person
//!    paired this app with;
//! 2. **the rest of the LAN**, in a stable (sorted) order, so two runs of one app
//!    never differ because beacons arrived in another order.
//!
//! Ranking only orders: a node a tier above that is down is skipped by the caller's
//! own health probe and the next tier answers, as before.

use std::net::{IpAddr, UdpSocket};

/// The host part of a base URL (`https://10.0.0.149:8123` -> `10.0.0.149`).
pub fn host_of(base: &str) -> &str {
    let rest = base.trim_start_matches("https://").trim_start_matches("http://");
    let rest = rest.split('/').next().unwrap_or(rest);
    if let Some(stripped) = rest.strip_prefix('[') {
        return stripped.split(']').next().unwrap_or(stripped);
    }
    rest.rsplit_once(':').map_or(rest, |(host, port)| if port.chars().all(|c| c.is_ascii_digit()) { host } else { rest })
}

/// This host's primary LAN address (the one a packet to the LAN would leave from).
fn own_lan_ip() -> Option<IpAddr> {
    static IP: std::sync::OnceLock<Option<IpAddr>> = std::sync::OnceLock::new();
    *IP.get_or_init(|| {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        // A UDP connect sends nothing; it only picks the outgoing interface.
        socket.connect("10.255.255.255:9").ok()?;
        socket.local_addr().ok().map(|a| a.ip())
    })
}

pub fn is_this_machine(base: &str) -> bool {
    let host = host_of(base);
    if host == "localhost" || host.starts_with("127.") || host == "::1" {
        return true;
    }
    own_lan_ip().is_some_and(|ip| ip.to_string() == host)
}

fn pinned_entries() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Ok(list) = std::env::var("MAKEPAD_AI_HUB") {
        out.extend(list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string));
    }
    // A browser has no ~/.makepad: on the web only MAKEPAD_AI_HUB pins.
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(text) = std::fs::read_to_string(crate::fleet_auth::client_dir().join("pinned")) {
        out.extend(text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string));
    }
    out
}

/// The pinned hubs as base URLs (`host` alone means the fleet port 8123), so a client
/// reaches a paired hub even before (or without) its LAN beacon.
pub fn pinned_bases() -> Vec<String> {
    pinned_entries()
        .into_iter()
        .map(|pin| {
            if pin.starts_with("http://") || pin.starts_with("https://") {
                pin.trim_end_matches('/').to_string()
            } else if pin.rsplit_once(':').is_some_and(|(_, p)| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
                format!("https://{pin}")
            } else {
                format!("https://{pin}:8123")
            }
        })
        .collect()
}

/// Does `base` match a pin (same host, and the same port when the pin names one)?
fn matches_pin(base: &str, pins: &[String]) -> bool {
    let host = host_of(base);
    let port = base.rsplit_once(':').and_then(|(_, p)| p.trim_end_matches('/').parse::<u16>().ok());
    pins.iter().any(|pin| {
        let pin_host = host_of(pin);
        let pin_port = pin.rsplit_once(':').and_then(|(_, p)| p.trim_end_matches('/').parse::<u16>().ok());
        pin_host == host && pin_port.is_none_or(|pp| Some(pp) == port)
    })
}

/// 0 = this machine, 1 = pinned, 2 = the rest.
pub fn tier(base: &str) -> u8 {
    tier_with(base, &pinned_entries())
}

fn tier_with(base: &str, pins: &[String]) -> u8 {
    if is_this_machine(base) {
        0
    } else if matches_pin(base, pins) {
        1
    } else {
        2
    }
}

/// Order `bases` by tier, then by name; drops duplicates.
pub fn order(bases: &mut Vec<String>) {
    order_with(bases, &pinned_entries());
}

fn order_with(bases: &mut Vec<String>, pins: &[String]) {
    bases.sort_by(|a, b| tier_with(a, pins).cmp(&tier_with(b, pins)).then_with(|| a.cmp(b)));
    bases.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_come_out_of_urls() {
        assert_eq!(host_of("https://10.0.0.149:8123"), "10.0.0.149");
        assert_eq!(host_of("http://127.0.0.1:8123/"), "127.0.0.1");
        assert_eq!(host_of("10.0.0.5:8785"), "10.0.0.5");
        assert_eq!(host_of("https://[::1]:8123"), "::1");
    }

    #[test]
    fn this_machine_then_pinned_then_the_rest_sorted() {
        let pins = vec!["10.0.0.149".to_string()];
        let mut bases: Vec<String> = ["https://10.0.0.235:8123", "https://10.0.0.149:8123", "https://127.0.0.1:8123", "https://10.0.0.203:8123", "https://10.0.0.123:8123"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        order_with(&mut bases, &pins);
        assert_eq!(
            bases,
            ["https://127.0.0.1:8123", "https://10.0.0.149:8123", "https://10.0.0.123:8123", "https://10.0.0.203:8123", "https://10.0.0.235:8123"]
        );
    }

    #[test]
    fn a_pin_with_a_port_matches_only_that_port() {
        let pins = vec!["10.0.0.149:8123".to_string()];
        assert_eq!(tier_with("https://10.0.0.149:8123", &pins), 1);
        assert_eq!(tier_with("https://10.0.0.149:8785", &pins), 2);
        assert_eq!(tier_with("http://localhost:1", &pins), 0);
    }

    #[test]
    fn arrival_order_never_changes_the_result() {
        let a = vec!["https://10.0.0.203:8123".to_string(), "https://10.0.0.123:8123".to_string()];
        let mut x = a.clone();
        let mut y: Vec<String> = a.into_iter().rev().collect();
        order_with(&mut x, &[]);
        order_with(&mut y, &[]);
        assert_eq!(x, y);
    }

    #[test]
    fn generation_scheduling_ranks_this_machine_first() {
        // role_standing is the first key of every fleet pick: a loopback hub beats a
        // LAN hub even when the LAN one has the weights on disk (affinity >= 3).
        let local = crate::fleet::standing_for_test("https://127.0.0.1:8123", "music", 1);
        let lan = crate::fleet::standing_for_test("https://10.0.0.203:8123", "music", 3);
        assert!(local > lan);
    }

    #[test]
    fn pins_become_base_urls() {
        std::env::set_var("MAKEPAD_AI_HUB", "10.0.0.149:8123, 10.0.0.9, https://h:1/");
        let b = pinned_bases();
        std::env::remove_var("MAKEPAD_AI_HUB");
        assert_eq!(b, vec!["https://10.0.0.149:8123", "https://10.0.0.9:8123", "https://h:1"]);
    }
}
