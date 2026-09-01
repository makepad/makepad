//! URLs and identifiers: the only place that knows the archive's routes.
//!
//! Every function here is pure string work. Identifiers are validated
//! before they can reach a path, and file names are percent-encoded per
//! segment, so a hostile catalog entry cannot steer a request anywhere
//! but under its own item.

use crate::http::Error;

/// The archive's apex host. Redirect targets must be it or a subdomain of
/// it (`ia903202.us.archive.org` and friends serve the actual bytes).
pub const HOST: &str = "archive.org";

/// `archive.org` itself, or a host under it. A redirect anywhere else is
/// refused: the client only ever talks to the archive.
pub fn is_archive_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == HOST || host.ends_with(".archive.org")
}

/// Archive identifiers are ASCII letters, digits, `_`, `-`, `.` — never
/// empty, never a dot-only path component, never longer than the archive
/// itself allows (100).
pub fn is_valid_identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
        && id.bytes().any(|b| b != b'.')
}

/// A stable 16-byte key for an identifier: hosts that key tiles by an
/// opaque id (the VJ grid does) use this so the same item always lands on
/// the same tile without anyone allocating.
pub fn identifier_key(identifier: &str) -> [u8; 16] {
    let digest = makepad_network::digest::sha256_hash(identifier.as_bytes());
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest[..16]);
    key
}

fn is_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

fn push_pct(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 15) as usize] as char);
}

/// Percent-encode one query-string value (everything but RFC 3986
/// unreserved characters). Spaces become `%20`, not `+`, so the result is
/// valid in any position.
pub fn encode_query_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if is_unreserved(b) {
            out.push(b as char);
        } else {
            push_pct(&mut out, b);
        }
    }
    out
}

/// Percent-encode one path segment: unreserved bytes and the sub-delims
/// that are common in archive file names (`!$&'()*+,;=` and `@:`) pass
/// through; `/`, `?`, `#`, `%`, spaces and anything non-ASCII are encoded.
pub fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        if is_unreserved(b)
            || matches!(
                b,
                b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b'@' | b':'
            )
        {
            out.push(b as char);
        } else {
            push_pct(&mut out, b);
        }
    }
    out
}

/// Encode a file name that may contain directories (`Content/clip.mp4`):
/// each segment on its own, the slashes kept. Empty and dot segments are
/// dropped so a name can never climb.
pub fn encode_path(name: &str) -> String {
    name.split('/')
        .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .map(encode_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// `https://archive.org/metadata/<id>`
pub fn metadata_url(identifier: &str) -> String {
    format!("https://{HOST}/metadata/{}", encode_path_segment(identifier))
}

/// `https://archive.org/services/img/<id>` — the item tile (JPEG, ~180px).
pub fn thumb_url(identifier: &str) -> String {
    format!("https://{HOST}/services/img/{}", encode_path_segment(identifier))
}

/// `https://archive.org/details/<id>` — the human page; goes into the
/// rights record as the source.
pub fn details_url(identifier: &str) -> String {
    format!("https://{HOST}/details/{}", encode_path_segment(identifier))
}

/// `https://archive.org/download/<id>/<name>` — redirects (302) to the
/// storage node holding the bytes; [`crate::http`] follows that.
pub fn download_url(identifier: &str, name: &str) -> String {
    format!(
        "https://{HOST}/download/{}/{}",
        encode_path_segment(identifier),
        encode_path(name)
    )
}

/// An `https://` URL split for the socket layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpsUrl {
    pub host: String,
    pub port: u16,
    /// Path + query, always starting with `/`.
    pub target: String,
}

impl HttpsUrl {
    pub fn to_string(&self) -> String {
        if self.port == 443 {
            format!("https://{}{}", self.host, self.target)
        } else {
            format!("https://{}:{}{}", self.host, self.port, self.target)
        }
    }
}

/// Parse an `https://` URL. Cleartext, userinfo, fragments and control
/// bytes are refused; a missing path is `/`.
pub fn parse_https(url: &str) -> Result<HttpsUrl, Error> {
    if url.len() > 4096 || url.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(Error::InvalidUrl);
    }
    let rest = url.strip_prefix("https://").ok_or(Error::InvalidUrl)?;
    let (authority, target) = match rest.find('/') {
        Some(pos) => (&rest[..pos], &rest[pos..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || authority.contains('@') || target.contains('#') {
        return Err(Error::InvalidUrl);
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h, p.parse::<u16>().map_err(|_| Error::InvalidUrl)?),
        Some(_) => return Err(Error::InvalidUrl),
        None => (authority, 443),
    };
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return Err(Error::InvalidUrl);
    }
    if target.contains(' ') {
        return Err(Error::InvalidUrl);
    }
    Ok(HttpsUrl { host: host.to_ascii_lowercase(), port, target: target.to_string() })
}

/// Resolve a `Location` header against the request it answered: absolute
/// `https://` URLs pass through, path-absolute ones (`/x/y`) stay on the
/// same host. Anything else (relative paths, other schemes) is refused —
/// the archive does not send them, and guessing is how clients get
/// steered.
pub fn resolve_location(base: &HttpsUrl, location: &str) -> Result<HttpsUrl, Error> {
    let location = location.trim();
    if location.starts_with("https://") {
        return parse_https(location);
    }
    if location.starts_with('/') && !location.starts_with("//") {
        return parse_https(&format!("https://{}:{}{}", base.host, base.port, location));
    }
    Err(Error::InvalidUrl)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers() {
        assert!(is_valid_identifier("BigBuckBunny_124"));
        assert!(is_valid_identifier("apple-fukkireta"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier(".."));
        assert!(!is_valid_identifier("a/b"));
        assert!(!is_valid_identifier("a b"));
        assert!(!is_valid_identifier(&"x".repeat(101)));
    }

    #[test]
    fn hosts() {
        assert!(is_archive_host("archive.org"));
        assert!(is_archive_host("ia903202.us.archive.org"));
        assert!(is_archive_host("ARCHIVE.ORG"));
        assert!(!is_archive_host("notarchive.org"));
        assert!(!is_archive_host("archive.org.evil.com"));
    }

    #[test]
    fn encoding() {
        assert_eq!(encode_query_component("cat AND x:(y)"), "cat%20AND%20x%3A%28y%29");
        assert_eq!(encode_path("Content/big buck.mp4"), "Content/big%20buck.mp4");
        assert_eq!(encode_path("../etc/passwd"), "etc/passwd");
        assert_eq!(encode_path("a#b?c%d"), "a%23b%3Fc%25d");
        assert_eq!(
            download_url("apple-fukkireta", "Apple Fukkireta.mp4"),
            "https://archive.org/download/apple-fukkireta/Apple%20Fukkireta.mp4"
        );
        assert_eq!(thumb_url("x"), "https://archive.org/services/img/x");
    }

    #[test]
    fn parse_and_resolve() {
        let u = parse_https("https://archive.org/download/a/b%20c.mp4").unwrap();
        assert_eq!(u.host, "archive.org");
        assert_eq!(u.port, 443);
        assert_eq!(u.target, "/download/a/b%20c.mp4");
        assert_eq!(parse_https("https://host").unwrap().target, "/");
        assert!(parse_https("http://archive.org/").is_err());
        assert!(parse_https("https://user@archive.org/").is_err());
        assert!(parse_https("https://archive.org/a#b").is_err());
        let r = resolve_location(&u, "https://ia1.us.archive.org/3/items/a/b.mp4").unwrap();
        assert_eq!(r.host, "ia1.us.archive.org");
        let r = resolve_location(&u, "/other").unwrap();
        assert_eq!(r.host, "archive.org");
        assert_eq!(r.target, "/other");
        assert!(resolve_location(&u, "relative").is_err());
        assert!(resolve_location(&u, "http://archive.org/x").is_err());
        assert!(resolve_location(&u, "//evil.com/x").is_err());
    }

    #[test]
    fn keys_are_stable_and_distinct() {
        assert_eq!(identifier_key("a"), identifier_key("a"));
        assert_ne!(identifier_key("a"), identifier_key("b"));
    }
}
