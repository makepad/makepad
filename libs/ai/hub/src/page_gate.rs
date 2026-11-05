//! Keeps web pages out of a hub that serves this machine's own apps.
//!
//! A hub bound to loopback takes anyone on the machine as the machine, and a browser on the
//! machine can reach it for any page it shows: a cross-site POST that needs no preflight, a
//! websocket (which has no same-origin rule), or a name the page's own domain rebinds to
//! 127.0.0.1. Two request fields tell a page from an app, and a page cannot change either:
//!
//! - `Host` names what the client dialled. Every client sends it, and after a rebinding it is
//!   the attacker's name, not an address of this machine.
//! - `Origin` names the page. A browser sends it on every cross-site POST and every websocket
//!   upgrade; a native client sends none.
//!
//! A request is let in when its Host names loopback (`localhost`, `127.x.x.x`, `::1`, any
//! port) and it carries no Origin, or an Origin that names loopback too. The hub gives no
//! CORS header, so a page on loopback can read no more than the hub itself shows it.

use makepad_network::utils::split_header_line;
use makepad_network::HttpServerHeaders;
use std::net::{IpAddr, Ipv6Addr};

enum Field<'a> {
    Absent,
    One(&'a str),
    /// Sent twice: no reading of it is the right one.
    Many,
}

fn field<'a>(headers: &'a HttpServerHeaders, name: &str) -> Field<'a> {
    let mut found = headers.lines.iter().skip(1).filter_map(|line| split_header_line(line, name));
    match (found.next(), found.next()) {
        (None, _) => Field::Absent,
        (Some(value), None) => Field::One(value),
        (Some(_), Some(_)) => Field::Many,
    }
}

/// What the answer says of a value it refused: the first characters, readable.
fn shown(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii_graphic()).take(64).collect()
}

/// Why a request is refused as a web page's, None when it may go on.
pub fn refusal(headers: &HttpServerHeaders) -> Option<String> {
    match field(headers, "Host") {
        Field::One(host) if loopback_authority(host) => {}
        Field::One(host) => return Some(format!("Host {} is not a loopback name or address", shown(host))),
        Field::Absent => return Some("a request to this hub names its Host".to_string()),
        Field::Many => return Some("a request names one Host".to_string()),
    }
    match field(headers, "Origin") {
        Field::Absent => None,
        Field::One(origin) if loopback_origin(origin) => None,
        Field::One(origin) => Some(format!("Origin {} is not a page on this machine", shown(origin))),
        Field::Many => Some("a request names one Origin".to_string()),
    }
}

/// `scheme://host[:port]` with a loopback host. `null` (a sandboxed frame, a file) is no page
/// of this machine's.
fn loopback_origin(origin: &str) -> bool {
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) && loopback_authority(authority)
}

/// `host[:port]`: `localhost` or a loopback address, an IPv6 one in brackets, any port.
fn loopback_authority(authority: &str) -> bool {
    if let Ok(ip) = authority.parse::<IpAddr>() {
        return ip.to_canonical().is_loopback();
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((inside, after)) = rest.split_once(']') else {
            return false;
        };
        return (after.is_empty() || after.strip_prefix(':').is_some_and(is_port))
            && inside.parse::<Ipv6Addr>().is_ok_and(|ip| ip.to_canonical().is_loopback());
    }
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if is_port(port) => host,
        _ => authority,
    };
    host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
}

fn is_port(text: &str) -> bool {
    !text.is_empty() && text.len() <= 5 && text.bytes().all(|b| b.is_ascii_digit()) && text.parse::<u32>().is_ok_and(|port| port <= 65535)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(lines: &[&str]) -> HttpServerHeaders {
        let mut all = vec!["POST /generate HTTP/1.1\r\n".to_string()];
        all.extend(lines.iter().map(|line| format!("{line}\r\n")));
        HttpServerHeaders {
            addr: "127.0.0.1:1".parse().unwrap(),
            addr_text: "127.0.0.1:1".to_string(),
            lines: all,
            verb: "POST".to_string(),
            path: "/generate".to_string(),
            path_no_slash: "generate".to_string(),
            search: None,
            content_length: Some(0),
            accept_encoding: None,
            sec_websocket_key: None,
        }
    }

    /// Hosts and origins as browsers and native clients write them, and as an attacker's page
    /// or rebound name does: one table, each row says which request it is.
    #[test]
    fn only_loopback_hosts_and_origins_get_in() {
        let allowed: &[&[&str]] = &[
            &["Host: 127.0.0.1"],
            &["Host: 127.0.0.1:8124"],
            &["host: LOCALHOST:8124"],
            &["Host: localhost"],
            &["Host: [::1]:8124"],
            &["Host: ::1"],
            &["Host: 127.0.0.1:8124", "Origin: http://localhost:3000"],
            &["Host: localhost:8124", "Origin: http://127.0.0.1:8124"],
            &["Host: localhost:8124", "origin: https://[::1]:9"],
        ];
        for lines in allowed {
            assert_eq!(refusal(&headers(lines)), None, "{lines:?} is an app on this machine");
        }
        let refused: &[(&[&str], &str)] = &[
            (&[], "no Host"),
            (&["Host: 127.0.0.1", "Host: 127.0.0.1"], "two Hosts"),
            (&["Host: evil.example"], "a name that is not loopback"),
            (&["Host: evil.example:8124"], "a rebound name with the hub's port"),
            (&["Host: 127.0.0.1.evil.example:8124"], "a name that starts like an address"),
            (&["Host: localhost.evil.example"], "a name that starts like localhost"),
            (&["Host: 192.0.2.7:8124"], "an address of the machine's network"),
            (&["Host: 0.0.0.0:8124"], "the unspecified address"),
            (&["Host: attacker@127.0.0.1"], "a userinfo in front of an address"),
            (&["Host: 127.0.0.1:port"], "a port that is not a number"),
            (&["Host: 127.0.0.1:99999"], "a port past 65535"),
            (&["Host: [::1"], "an unclosed bracket"),
            (&["Host: 127.0.0.1", "Origin: https://evil.example"], "a foreign page"),
            (&["Host: 127.0.0.1", "Origin: http://127.0.0.1.evil.example"], "a page on a look-alike name"),
            (&["Host: 127.0.0.1", "Origin: null"], "a sandboxed frame or a file"),
            (&["Host: 127.0.0.1", "Origin: "], "an empty Origin"),
            (&["Host: 127.0.0.1", "Origin: ftp://localhost"], "a scheme that is no page"),
            (&["Host: 127.0.0.1", "Origin: http://localhost/x"], "an Origin with a path"),
            (&["Host: 127.0.0.1", "Origin: http://localhost", "Origin: https://evil.example"], "two Origins"),
            (&["Host: evil.example", "Origin: http://localhost"], "a loopback Origin does not excuse the Host"),
        ];
        for (lines, what) in refused {
            assert!(refusal(&headers(lines)).is_some(), "{what}: {lines:?}");
        }
    }
}
