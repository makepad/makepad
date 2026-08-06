//! Registry and LAN clients.
//!
//! A plain std HTTP/1.1 client rather than the platform one: this crate has no
//! `Cx`, which is what lets the whole download-verify-install path be tested
//! headless against an in-process server.
//!
//! Every response is treated as hostile: bounded reads, a declared-length cap
//! before allocation, and a sha256 check against the index BEFORE the bytes are
//! ever handed to the unpacker.

use crate::pack::MAX_ARCHIVE_BYTES;
use crate::sha256::sha256_hex;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_INDEX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum HttpError {
    Connect(String),
    Io(String),
    Status(u16),
    Malformed(&'static str),
    TooLarge,
    /// The bytes arrived intact but are not what the index promised.
    DigestMismatch { expected: String, got: String },
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Connect(e) => write!(f, "connect failed: {e}"),
            HttpError::Io(e) => write!(f, "io error: {e}"),
            HttpError::Status(c) => write!(f, "http status {c}"),
            HttpError::Malformed(w) => write!(f, "malformed response: {w}"),
            HttpError::TooLarge => write!(f, "response too large"),
            HttpError::DigestMismatch { expected, got } => {
                write!(f, "sha256 mismatch: expected {expected}, got {got}")
            }
        }
    }
}

/// One row of the registry index.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct IndexEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
}

/// Split "host:port/path" style targets. Accepts an optional http:// prefix;
/// https is not supported here (the registry is fetched over plain HTTP and
/// verified by digest, which is what the sha256 in the index is for).
fn split_url(url: &str) -> Result<(String, String), HttpError> {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    if rest.starts_with("https://") {
        return Err(HttpError::Malformed("https is not supported"));
    }
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(HttpError::Malformed("empty host"));
    }
    Ok((authority.to_string(), path.to_string()))
}

fn connect(authority: &str) -> Result<TcpStream, HttpError> {
    use std::net::ToSocketAddrs;
    let with_port = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    let mut last = None;
    let addrs = with_port
        .to_socket_addrs()
        .map_err(|e| HttpError::Connect(e.to_string()))?;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                let _ = s.set_read_timeout(Some(IO_TIMEOUT));
                let _ = s.set_write_timeout(Some(IO_TIMEOUT));
                return Ok(s);
            }
            Err(e) => last = Some(e.to_string()),
        }
    }
    Err(HttpError::Connect(
        last.unwrap_or_else(|| "no addresses".into()),
    ))
}

struct Response {
    status: u16,
    body: Vec<u8>,
}

/// Read a response with a hard cap. Handles Content-Length and
/// connection-close framing; chunked encoding is refused rather than
/// half-implemented.
fn read_response(stream: &mut TcpStream, max_body: usize) -> Result<Response, HttpError> {
    let mut buf = Vec::new();
    let mut head_end = None;
    let mut chunk = [0u8; 8192];
    while head_end.is_none() {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| HttpError::Io(e.to_string()))?;
        if n == 0 {
            return Err(HttpError::Malformed("connection closed in headers"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_HEADER_BYTES {
            return Err(HttpError::TooLarge);
        }
        head_end = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
    }
    let head_end = head_end.unwrap();
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();

    let mut lines = head.lines();
    let status_line = lines.next().ok_or(HttpError::Malformed("no status line"))?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or(HttpError::Malformed("no status code"))?;

    let mut content_length: Option<usize> = None;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse::<usize>().ok();
        }
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            return Err(HttpError::Malformed("chunked encoding unsupported"));
        }
    }
    if let Some(len) = content_length {
        if len > max_body {
            return Err(HttpError::TooLarge);
        }
    }

    let mut body = buf[head_end..].to_vec();
    let want = content_length.unwrap_or(usize::MAX);
    while body.len() < want {
        let n = stream
            .read(&mut chunk)
            .map_err(|e| HttpError::Io(e.to_string()))?;
        if n == 0 {
            break; // close-framed body, or a short one
        }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > max_body {
            return Err(HttpError::TooLarge);
        }
    }
    if let Some(len) = content_length {
        if body.len() > len {
            body.truncate(len);
        }
    }
    Ok(Response { status, body })
}

fn request(
    method: &str,
    url: &str,
    body: Option<&[u8]>,
    max_body: usize,
) -> Result<Vec<u8>, HttpError> {
    let (authority, path) = split_url(url)?;
    let mut stream = connect(&authority)?;
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nUser-Agent: makepad-arcade\r\n"
    );
    if let Some(b) = body {
        head.push_str(&format!(
            "Content-Length: {}\r\nContent-Type: application/octet-stream\r\n",
            b.len()
        ));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .map_err(|e| HttpError::Io(e.to_string()))?;
    if let Some(b) = body {
        stream
            .write_all(b)
            .map_err(|e| HttpError::Io(e.to_string()))?;
    }
    stream.flush().map_err(|e| HttpError::Io(e.to_string()))?;

    let resp = read_response(&mut stream, max_body)?;
    if !(200..300).contains(&resp.status) {
        return Err(HttpError::Status(resp.status));
    }
    Ok(resp.body)
}

pub fn http_get(url: &str, max_body: usize) -> Result<Vec<u8>, HttpError> {
    request("GET", url, None, max_body)
}

pub fn http_post(url: &str, body: &[u8]) -> Result<Vec<u8>, HttpError> {
    request("POST", url, Some(body), 64 * 1024)
}

/// Minimal JSON reader for the index: an array of flat string/number objects.
/// Like the manifest parser, total by construction — this parses bytes from a
/// server we do not control.
pub fn parse_index(bytes: &[u8]) -> Result<Vec<IndexEntry>, HttpError> {
    if bytes.len() > MAX_INDEX_BYTES {
        return Err(HttpError::TooLarge);
    }
    let src = std::str::from_utf8(bytes).map_err(|_| HttpError::Malformed("index not utf-8"))?;
    let mut out = Vec::new();
    let mut chars = src.char_indices().peekable();

    // Walk to the opening bracket, then parse object by object.
    while let Some(&(_, c)) = chars.peek() {
        if c == '[' {
            chars.next();
            break;
        }
        chars.next();
    }
    loop {
        // Find the next '{' or the closing ']'.
        let mut start = None;
        for (i, c) in chars.by_ref() {
            if c == '{' {
                start = Some(i);
                break;
            }
            if c == ']' {
                return Ok(out);
            }
        }
        let Some(start) = start else { return Ok(out) };

        // Find the matching '}' (objects here are flat — no nesting).
        let mut end = None;
        for (i, c) in chars.by_ref() {
            if c == '}' {
                end = Some(i);
                break;
            }
        }
        let Some(end) = end else {
            return Err(HttpError::Malformed("unterminated object in index"));
        };
        out.push(parse_entry(&src[start + 1..end]));
        if out.len() > 4096 {
            return Err(HttpError::TooLarge);
        }
    }
}

fn parse_entry(body: &str) -> IndexEntry {
    let mut e = IndexEntry::default();
    for field in split_top_level(body) {
        let Some(colon) = field.find(':') else { continue };
        let key = field[..colon].trim().trim_matches('"').to_string();
        let raw = field[colon + 1..].trim();
        let val = raw.trim_matches('"').to_string();
        match key.as_str() {
            "id" => e.id = val,
            "name" => e.name = val,
            "description" => e.description = val,
            "author" => e.author = val,
            "sha256" => e.sha256 = val.to_ascii_lowercase(),
            "url" => e.url = val,
            "size" => e.size = raw.trim_matches('"').parse().unwrap_or(0),
            _ => {}
        }
    }
    e
}

/// Split on commas that are not inside a string.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_str = false;
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'\\' if in_str => i += 1,
            b',' if !in_str => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// A registry endpoint. `base` is "host:port" or "http://host:port".
pub struct Registry {
    pub base: String,
}

impl Registry {
    pub fn new(base: impl Into<String>) -> Self {
        Self { base: base.into() }
    }

    fn url(&self, path: &str) -> String {
        let base = self.base.trim_end_matches('/');
        format!("{base}{path}")
    }

    pub fn index(&self) -> Result<Vec<IndexEntry>, HttpError> {
        parse_index(&http_get(&self.url("/index.json"), MAX_INDEX_BYTES)?)
    }

    /// Download and verify. The digest check happens here, before the caller
    /// can hand the bytes to the unpacker — a package that does not match what
    /// the index promised never reaches the extractor at all.
    pub fn download(&self, entry: &IndexEntry) -> Result<Vec<u8>, HttpError> {
        let url = if entry.url.is_empty() {
            self.url(&format!("/games/{}.arcade", entry.id))
        } else if entry.url.starts_with("http://") || entry.url.contains(':') {
            entry.url.clone()
        } else {
            self.url(&entry.url)
        };
        let bytes = http_get(&url, MAX_ARCHIVE_BYTES)?;
        verify_digest(&bytes, &entry.sha256)?;
        Ok(bytes)
    }

    pub fn publish(&self, package: &[u8]) -> Result<String, HttpError> {
        let body = http_post(&self.url("/publish"), package)?;
        Ok(String::from_utf8_lossy(&body).trim().to_string())
    }
}

pub fn verify_digest(bytes: &[u8], expected_hex: &str) -> Result<(), HttpError> {
    let expected = expected_hex.trim().to_ascii_lowercase();
    if expected.is_empty() {
        return Err(HttpError::Malformed("index entry has no sha256"));
    }
    let got = sha256_hex(bytes);
    if got != expected {
        return Err(HttpError::DigestMismatch { expected, got });
    }
    Ok(())
}

/// The LAN case: a host serving the game it is running, so a joiner can install
/// it before entering the room. Same verification story when a digest is known.
pub fn fetch_lan_package(host_addr: &str, expected_sha256: Option<&str>) -> Result<Vec<u8>, HttpError> {
    let base = host_addr.trim_end_matches('/');
    let bytes = http_get(&format!("{base}/game.arcade"), MAX_ARCHIVE_BYTES)?;
    if let Some(sha) = expected_sha256 {
        verify_digest(&bytes, sha)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_an_index() {
        let json = br#"[
          {"id":"speedway","name":"Speedway","description":"race, 4 cars","author":"kid","size":1234,"sha256":"ABCD","url":"/games/speedway.arcade"},
          {"id":"dogfight","name":"Dogfight","description":"planes","author":"kid","size":99,"sha256":"beef","url":""}
        ]"#;
        let idx = parse_index(json).unwrap();
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[0].id, "speedway");
        assert_eq!(idx[0].size, 1234);
        assert_eq!(idx[0].sha256, "abcd", "digests normalise to lowercase");
        assert_eq!(idx[1].name, "Dogfight");
    }

    #[test]
    fn index_parsing_is_total() {
        for bad in [
            &b""[..],
            b"[",
            b"[{",
            b"[{}]",
            b"not json at all",
            b"[{\"id\":\"a\"",
            b"[{\"size\":\"not-a-number\"}]",
            b"\xff\xfe",
        ] {
            let _ = parse_index(bad);
        }
        assert!(parse_index(b"\xff\xfe").is_err());
        assert!(parse_index(&vec![b' '; MAX_INDEX_BYTES + 1]).is_err());
        // A truncated object is refused, not silently accepted.
        assert!(parse_index(b"[{\"id\":\"a\"").is_err());
        // Commas inside strings must not split fields.
        let idx = parse_index(br#"[{"id":"a","description":"one, two, three"}]"#).unwrap();
        assert_eq!(idx[0].description, "one, two, three");
    }

    #[test]
    fn digest_verification_rejects_tampering() {
        let bytes = b"a package";
        let good = sha256_hex(bytes);
        assert!(verify_digest(bytes, &good).is_ok());
        assert!(verify_digest(bytes, &good.to_uppercase()).is_ok());
        assert!(verify_digest(b"a package!", &good).is_err());
        assert!(verify_digest(bytes, "").is_err());
    }

    #[test]
    fn urls_split_sanely() {
        assert_eq!(
            split_url("http://127.0.0.1:8080/index.json").unwrap(),
            ("127.0.0.1:8080".to_string(), "/index.json".to_string())
        );
        assert_eq!(
            split_url("example.com").unwrap(),
            ("example.com".to_string(), "/".to_string())
        );
        assert!(split_url("").is_err());
        assert!(split_url("https://example.com").is_err());
    }
}
