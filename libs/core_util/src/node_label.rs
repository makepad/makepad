//! A machine name for people from a service base URL: the host without
//! scheme, port, path or credentials, and a short node label for it.
//!
//! The fleet scheduler routes by [`host_of`]; libraries, logs and progress
//! show [`node_label`], so a raw LAN address never reaches a person.

/// Host part of a base URL, lowercased and without scheme, port or path.
pub fn host_of(base_url: &str) -> String {
    let base_url = base_url.trim();
    let rest = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // Credentials belong to transport configuration, never to routing labels
    // or user-facing errors.
    let rest = rest.rsplit('@').next().unwrap_or(rest);
    // IPv6 literals lose their brackets; a bare IPv6 address has no port.
    let host = match rest.strip_prefix('[') {
        Some(inner) => inner.split(']').next().unwrap_or(inner),
        None if rest.parse::<std::net::Ipv6Addr>().is_ok() => rest,
        None => rest.split(':').next().unwrap_or(rest),
    };
    host.trim().to_ascii_lowercase()
}

/// A machine name for people: IP addresses become a node's last octet or hex
/// group, loopback is "this machine", and host names keep their name. An empty
/// host is "unknown node".
pub fn node_label(base_url: &str) -> String {
    let host = host_of(base_url);
    if host.is_empty() {
        "unknown node".to_string()
    } else if host == "localhost" {
        "this machine".to_string()
    } else {
        match host.parse::<std::net::IpAddr>() {
            Ok(ip) if ip.is_loopback() => "this machine".to_string(),
            Ok(std::net::IpAddr::V4(ip)) => format!("node {}", ip.octets()[3]),
            Ok(std::net::IpAddr::V6(ip)) => format!("node {:x}", ip.segments()[7]),
            Err(_) => host,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_read_out_of_any_url_shape() {
        assert_eq!(host_of("http://192.0.2.89:8123"), "192.0.2.89");
        assert_eq!(host_of("https://Box-A:8123/models"), "box-a");
        assert_eq!(host_of("192.0.2.89"), "192.0.2.89");
        assert_eq!(host_of("http://[::1]:8123"), "::1");
        assert_eq!(host_of("http://user:secret@192.0.2.7:8123/x?y#z"), "192.0.2.7");
        assert_eq!(host_of("2001:db8::12"), "2001:db8::12");
    }

    #[test]
    fn a_node_label_never_shows_a_raw_address() {
        assert_eq!(node_label("http://192.0.2.42:8787"), "node 42");
        assert_eq!(node_label("http://user:secret@192.0.2.42:8787/"), "node 42");
        assert_eq!(node_label("http://[2001:db8::1a]:8787"), "node 1a");
        assert_eq!(node_label("http://127.0.0.1:8787"), "this machine");
        assert_eq!(node_label("http://[::1]:8787"), "this machine");
        assert_eq!(node_label("http://LocalHost:8787"), "this machine");
        assert_eq!(node_label("https://Box-A:8123"), "box-a");
        assert_eq!(node_label(""), "unknown node");
        assert_eq!(node_label("http://"), "unknown node");
    }
}
