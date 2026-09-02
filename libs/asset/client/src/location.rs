//! Validated client locations shared by native and portable configurations.

use crate::error::{ClientError, ClientResult};
use crate::wire;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;

/// The two socket endpoints advertised by a native Asset Server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApiEndpoints {
    pub control: SocketAddr,
    pub data: SocketAddr,
}

/// The execution mode carried by capability errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientMode {
    Native,
    StaticWeb,
}

pub const CAPABILITY_BLOCKING_API: &str = "blocking_api";
pub const CAPABILITY_STATIC_SITE_SESSION: &str = "static_site_session";

/// Where a client obtains its immutable asset data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientLocation {
    Native(ApiEndpoints),
    StaticSite(BaseUrl),
}

impl ClientLocation {
    pub fn mode(&self) -> ClientMode {
        match self {
            Self::Native(_) => ClientMode::Native,
            Self::StaticSite(_) => ClientMode::StaticWeb,
        }
    }
}

/// A normalized URL prefix for a credential-free static asset export.
///
/// HTTPS is required except for loopback hosts used by desktop tests and
/// local development. Credentials, queries, and fragments are refused, and
/// the stored spelling always has exactly one trailing slash.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BaseUrl(String);

impl BaseUrl {
    pub fn parse(value: impl AsRef<str>) -> ClientResult<Self> {
        let value = value.as_ref();
        if value.is_empty()
            || !value.is_ascii()
            || value.bytes().any(|b| b <= b' ' || b == 0x7f || b == b'\\')
            || value.contains('?')
            || value.contains('#')
        {
            return Err(ClientError::InvalidInput { what: "static base url" });
        }
        let Some((scheme, remainder)) = value.split_once("://") else {
            return Err(ClientError::InvalidInput { what: "static base url scheme" });
        };
        let is_https = scheme.eq_ignore_ascii_case("https");
        let is_http = scheme.eq_ignore_ascii_case("http");
        if !is_https && !is_http {
            return Err(ClientError::InvalidInput { what: "static base url scheme" });
        }
        let authority_end = remainder.find('/').unwrap_or(remainder.len());
        let authority = &remainder[..authority_end];
        if authority.is_empty() || authority.contains('@') {
            return Err(ClientError::InvalidInput { what: "static base url authority" });
        }
        let (authority, host) = canonical_authority(authority, is_https)
            .ok_or(ClientError::InvalidInput { what: "static base url authority" })?;
        if is_http && !is_loopback_host(&host) {
            return Err(ClientError::InvalidInput { what: "static base url requires https" });
        }
        let path = canonical_path(&remainder[authority_end..])
            .ok_or(ClientError::InvalidInput { what: "static base url path" })?;

        let mut normalized = String::with_capacity(value.len() + 1);
        normalized.push_str(if is_https { "https://" } else { "http://" });
        normalized.push_str(&authority);
        normalized.push_str(&path);
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Join one already wire-legal origin-form target to this prefix.
    pub fn join(&self, target: &str) -> ClientResult<String> {
        if target.is_empty()
            || !target.starts_with('/')
            || target.len() > wire::MAX_TARGET_BYTES
            || !target.bytes().all(wire::target_byte_ok)
            || has_dot_segment(target.split_once('?').map_or(target, |(path, _)| path))
        {
            return Err(ClientError::InvalidInput { what: "static request target" });
        }
        let mut joined = String::with_capacity(self.0.len() + target.len());
        joined.push_str(&self.0);
        joined.push_str(&target[1..]);
        Ok(joined)
    }
}

impl std::fmt::Display for BaseUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BaseUrl {
    type Err = ClientError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<String> for BaseUrl {
    type Error = ClientError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

fn canonical_authority(authority: &str, is_https: bool) -> Option<(String, String)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let tail = &rest[end + 1..];
        let address = Ipv6Addr::from_str(host).ok()?;
        let port = if tail.is_empty() {
            None
        } else {
            Some(parse_port(tail.strip_prefix(':')?)?)
        };
        let host = address.to_string();
        let mut out = format!("[{host}]");
        if port.is_some_and(|port| port != if is_https { 443 } else { 80 }) {
            out.push(':');
            out.push_str(&port?.to_string());
        }
        return Some((out, host));
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.is_empty() || host.contains(':') {
                return None;
            }
            (host, Some(parse_port(port)?))
        }
        None => (authority, None),
    };
    if host.is_empty() {
        return None;
    }
    let host = if let Ok(address) = Ipv4Addr::from_str(host) {
        address.to_string()
    } else {
        let host = host.to_ascii_lowercase();
        if host.split('.').any(|label| {
            label.is_empty()
                || label.starts_with('-')
                || label.ends_with('-')
                || !label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }) {
            return None;
        }
        host
    };
    let mut out = host.clone();
    if port.is_some_and(|port| port != if is_https { 443 } else { 80 }) {
        out.push(':');
        out.push_str(&port?.to_string());
    }
    Some((out, host))
}

fn parse_port(port: &str) -> Option<u16> {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    port.parse().ok()
}

fn canonical_path(path: &str) -> Option<String> {
    let path = if path.is_empty() { "/" } else { path };
    if !path.starts_with('/')
        || !path.bytes().all(url_path_byte_ok)
        || has_dot_segment(path)
    {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len() + 1);
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hi = *bytes.get(at + 1)?;
            let lo = *bytes.get(at + 2)?;
            if !hi.is_ascii_hexdigit() || !lo.is_ascii_hexdigit() {
                return None;
            }
            out.push('%');
            out.push((hi as char).to_ascii_uppercase());
            out.push((lo as char).to_ascii_uppercase());
            at += 3;
        } else {
            out.push(bytes[at] as char);
            at += 1;
        }
    }
    while out.ends_with("//") {
        out.pop();
    }
    if !out.ends_with('/') {
        out.push('/');
    }
    Some(out)
}

fn url_path_byte_ok(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')'
                | b'*' | b'+' | b',' | b';' | b'=' | b':' | b'@' | b'/' | b'%'
        )
}

fn has_dot_segment(path: &str) -> bool {
    path.split('/').any(|segment| {
        let mut dots = 0usize;
        let bytes = segment.as_bytes();
        let mut at = 0usize;
        while at < bytes.len() {
            if bytes[at] == b'.' {
                dots += 1;
                at += 1;
            } else if bytes.get(at) == Some(&b'%')
                && bytes.get(at + 1).is_some_and(|byte| *byte == b'2')
                && bytes
                    .get(at + 2)
                    .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'e'))
            {
                dots += 1;
                at += 3;
            } else {
                return false;
            }
        }
        dots == 1 || dots == 2
    })
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || Ipv4Addr::from_str(host).is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::{ApiEndpoints, BaseUrl, ClientMode, CAPABILITY_STATIC_SITE_SESSION};
    use crate::{AssetClient, ClientConfig, ClientError, SessionConfig, SessionConnector};

    #[test]
    fn validation_table_and_normalization() {
        let accepted = [
            ("https://assets.example", "https://assets.example/"),
            ("HTTPS://ASSETS.EXAMPLE:443/root///", "https://assets.example/root/"),
            ("http://localhost:8080", "http://localhost:8080/"),
            ("http://127.0.0.1:8080/static/", "http://127.0.0.1:8080/static/"),
            ("http://[::1]:8080", "http://[::1]:8080/"),
        ];
        for (input, expected) in accepted {
            assert_eq!(BaseUrl::parse(input).unwrap().as_str(), expected, "{input}");
        }
        for input in [
            "", "assets.example", "ftp://assets.example", "http://assets.example",
            "https://user:pass@assets.example", "https://assets.example?q=1",
            "https://assets.example/#frag", "https://assets.example:bad",
            "https://assets.example/path with space",
            "https://assets.example/./root", "https://assets.example/../root",
            "https://assets.example/%2e/root", "https://assets.example/%2E%2e/root",
        ] {
            assert!(BaseUrl::parse(input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn join_accepts_only_wire_targets() {
        let base = BaseUrl::parse("https://assets.example/export").unwrap();
        assert_eq!(base.join("/v1/health").unwrap(), "https://assets.example/export/v1/health");
        for target in [
            "", "v1/health", "/bad%20path", "/bad#fragment", "/../v1/health",
            "/./v1/health", "/safe/../v1/health", "/%2e%2e/v1/health",
        ] {
            assert!(base.join(target).is_err(), "accepted {target}");
        }
    }

    #[test]
    fn static_constructors_select_typed_unavailable_and_reject_tokens() {
        let base = BaseUrl::parse("https://assets.example").unwrap();
        let endpoints = ApiEndpoints {
            control: "127.0.0.1:1".parse().unwrap(),
            data: "127.0.0.1:2".parse().unwrap(),
        };
        assert!(matches!(
            AssetClient::connect(ClientConfig::static_site(base.clone()), endpoints, None),
            Err(ClientError::Unavailable {
                capability: CAPABILITY_STATIC_SITE_SESSION,
                mode: ClientMode::StaticWeb,
            })
        ));
        assert!(matches!(
            SessionConnector::start(SessionConfig::static_site(base.clone())),
            Err(ClientError::Unavailable {
                capability: "static_site_session",
                mode: ClientMode::StaticWeb,
            })
        ));

        let mut client = ClientConfig::static_site(base.clone());
        client.token = Some("must-not-leak".to_string());
        assert!(matches!(
            AssetClient::connect(client, endpoints, None),
            Err(ClientError::InvalidInput { what: "static site bearer token" })
        ));
        let mut session = SessionConfig::static_site(base);
        session.token = Some("must-not-leak".to_string());
        assert!(matches!(
            SessionConnector::start(session),
            Err(ClientError::InvalidInput { what: "static site bearer token" })
        ));
    }
}
