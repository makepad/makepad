//! Validated client locations shared by native and portable configurations.

use crate::error::{ClientError, ClientResult};
use crate::wire;
use std::net::{Ipv4Addr, SocketAddr};
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
        let host = authority_host(authority)
            .ok_or(ClientError::InvalidInput { what: "static base url authority" })?;
        if is_http && !is_loopback_host(host) {
            return Err(ClientError::InvalidInput { what: "static base url requires https" });
        }

        let mut normalized = String::with_capacity(value.len() + 1);
        normalized.push_str(if is_https { "https://" } else { "http://" });
        normalized.push_str(remainder.trim_end_matches('/'));
        normalized.push('/');
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

fn authority_host(authority: &str) -> Option<&str> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let tail = &rest[end + 1..];
        if host.is_empty() || (!tail.is_empty() && !valid_port(tail.strip_prefix(':')?)) {
            return None;
        }
        return Some(host);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.is_empty() || !valid_port(port) || host.contains(':') {
                None
            } else {
                Some(host)
            }
        }
        None if !authority.is_empty() => Some(authority),
        None => None,
    }
}

fn valid_port(port: &str) -> bool {
    !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) && port.parse::<u16>().is_ok()
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || Ipv4Addr::from_str(host).is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::{ApiEndpoints, BaseUrl, ClientMode};
    use crate::{AssetClient, ClientConfig, ClientError, SessionConfig, SessionConnector};

    #[test]
    fn validation_table_and_normalization() {
        let accepted = [
            ("https://assets.example", "https://assets.example/"),
            ("HTTPS://assets.example/root///", "https://assets.example/root/"),
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
        ] {
            assert!(BaseUrl::parse(input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn join_accepts_only_wire_targets() {
        let base = BaseUrl::parse("https://assets.example/export").unwrap();
        assert_eq!(base.join("/v1/health").unwrap(), "https://assets.example/export/v1/health");
        for target in ["", "v1/health", "/bad%20path", "/bad#fragment"] {
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
                capability: "static_site_session" | "blocking_api",
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
