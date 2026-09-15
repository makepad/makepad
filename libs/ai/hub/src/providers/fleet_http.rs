//! Minimal bounded HTTP/1.1 JSON client for the fleet wire (and the test
//! harness's raw auth calls). Plain `http://host:port` only — the fleet is
//! a LAN service; a TLS-advertising endpoint is refused, never downgraded
//! around. Content-Length framing only; responses are capped; bodies parse
//! through the asset client's strict JSON.
//!
//! Kept deliberately tiny and separate from `asset_client::http` (which is
//! specialized for the asset server's two-plane protocol): this speaks to
//! `libs/asset/ai` service nodes.

use makepad_strict_json::{self as json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const CONNECT_TIMEOUT_MS: u64 = 3_000;
const IO_TIMEOUT_MS: u64 = 10_000;
const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Only `Connection` proves that no request bytes reached the peer. All
/// write/read/framing failures have an ambiguous submission outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    Connection(String),
    /// Complete HTTP/1.1 503 with Content-Length: 0 and Connection: close:
    /// Makepad's low-level overload refusal, before application dispatch.
    /// Kept distinct from a JSON null and any read/write/framing failure.
    Empty503,
    Other(String),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty503 => f.write_str("HTTP server overloaded before job admission"),
            Self::Connection(s) | Self::Other(s) => f.write_str(s),
        }
    }
}

impl From<String> for RequestError {
    fn from(s: String) -> Self { Self::Other(s) }
}

/// `POST`/`GET` a JSON document. An explicit `bearer` wins; otherwise the
/// fabric secret environment variable authenticates service-node requests.
pub fn request_json(
    method: &str,
    url: &str,
    body: Option<&Value>,
    bearer: Option<&str>,
) -> Result<(u16, Value), String> {
    match request_json_detailed(method, url, body, bearer) {
        // Preserve the legacy status/null representation for public callers.
        Err(RequestError::Empty503) => Ok((503, Value::Null)),
        result => result.map_err(|e| e.to_string()),
    }
}

pub fn request_json_detailed(
    method: &str,
    url: &str,
    body: Option<&Value>,
    bearer: Option<&str>,
) -> Result<(u16, Value), RequestError> {
    let env_bearer = if bearer.is_none() {
        std::env::var("MAKEPAD_AI_HUB_SECRET").ok()
    } else {
        None
    };
    let bearer = bearer.or_else(|| env_bearer.as_deref().map(str::trim));
    let (host_port, path) = split_url(url)?;
    let addr = host_port
        .to_socket_addrs()
        .map_err(|e| RequestError::Connection(format!("resolve {host_port}: {e}")))?
        .next()
        .ok_or_else(|| RequestError::Connection(format!("resolve {host_port}: no address")))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(CONNECT_TIMEOUT_MS))
        .map_err(|e| RequestError::Connection(format!("connect {host_port}: {e}")))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(IO_TIMEOUT_MS)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_millis(IO_TIMEOUT_MS)))
        .map_err(|e| e.to_string())?;
    let mut stream = stream;

    let body_bytes = body.map(|b| b.to_json().into_bytes()).unwrap_or_default();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\nAccept: application/json\r\n"
    );
    if let Some(b) = bearer {
        if b.contains(['\r', '\n']) {
            return Err("bearer credential contains a newline".to_string().into());
        }
        head.push_str(&format!("Authorization: Bearer {b}\r\n"));
    }
    if body.is_some() {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body_bytes.len()
        ));
    } else {
        head.push_str("Content-Length: 0\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).map_err(|e| format!("write: {e}"))?;
    if !body_bytes.is_empty() {
        stream.write_all(&body_bytes).map_err(|e| format!("write: {e}"))?;
    }

    read_json_response(&mut stream)
}

pub(super) fn read_json_response(stream: &mut impl Read) -> Result<(u16, Value), RequestError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > MAX_HEAD_BYTES + MAX_BODY_BYTES {
                    return Err("response too large".to_string().into());
                }
                // Stop early once a declared body is complete.
                if let Some((status, body_start, content_length, empty_overload)) = parse_head(&buf)? {
                    if let Some(len) = content_length {
                        if buf.len() >= body_start + len {
                            if empty_overload {
                                if buf.len() != body_start {
                                    return Err("unexpected body in empty 503".to_string().into());
                                }
                                return Err(RequestError::Empty503);
                            }
                            let body = &buf[body_start..body_start + len];
                            return finish(status, body).map_err(Into::into);
                        }
                    }
                }
            }
            Err(e) => return Err(format!("read: {e}").into()),
        }
    }
    let (status, body_start, content_length, _) =
        parse_head(&buf)?.ok_or_else(|| "incomplete response head".to_string())?;
    let end = match content_length {
        Some(len) if body_start + len > buf.len() => {
            return Err("incomplete response body".to_string().into());
        }
        Some(len) => body_start + len,
        None => buf.len(),
    };
    finish(status, &buf[body_start..end]).map_err(Into::into)
}

fn finish(status: u16, body: &[u8]) -> Result<(u16, Value), String> {
    if body.is_empty() {
        return Ok((status, Value::Null));
    }
    let v = json::parse(body).map_err(|e| format!("response json: {e}"))?;
    Ok((status, v))
}

/// `http://host:port/path` -> (`host:port`, `/path`).
fn split_url(url: &str) -> Result<(String, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only http:// urls are supported: {url}"))?;
    match rest.find('/') {
        Some(i) => Ok((rest[..i].to_string(), rest[i..].to_string())),
        None => Ok((rest.to_string(), "/".to_string())),
    }
}

/// Returns `Some((status, body_start, content_length, empty_overload))` once
/// the head is complete; `Ok(None)` while more bytes are needed.
#[allow(clippy::type_complexity)]
fn parse_head(buf: &[u8]) -> Result<Option<(u16, usize, Option<usize>, bool)>, String> {
    let Some(head_end) = find_head_end(buf) else {
        if buf.len() > MAX_HEAD_BYTES {
            return Err("response head too large".to_string());
        }
        return Ok(None);
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| "head not utf-8".to_string())?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or("missing status line")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("malformed status line")?;
    let mut content_length = None;
    let mut connection_close = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err("duplicate content-length".to_string());
            }
            let len: usize =
                value.trim().parse().map_err(|_| "malformed content-length".to_string())?;
            if len > MAX_BODY_BYTES {
                return Err("declared body too large".to_string());
            }
            content_length = Some(len);
        }
        if name.eq_ignore_ascii_case("connection") {
            connection_close = value.trim().eq_ignore_ascii_case("close");
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err("transfer-encoding refused".to_string());
        }
    }
    let empty_overload = status_line == "HTTP/1.1 503 Service Unavailable"
        && content_length == Some(0) && connection_close;
    Ok(Some((status, head_end + 4, content_length, empty_overload)))
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_rejection_keeps_status_and_json() {
        let body = br#"{"error":"busy","job_id":null}"#;
        let mut wire = format!("HTTP/1.1 409 Conflict\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        wire.extend_from_slice(body);
        let (status, value) = read_json_response(&mut wire.as_slice()).unwrap();
        assert_eq!(status, 409);
        assert_eq!(value.get("error").and_then(Value::as_str), Some("busy"));
        assert_eq!(value.get("job_id"), Some(&Value::Null));
    }

    #[test]
    fn truncated_body_even_with_valid_json_is_ambiguous() {
        let body = br#"{"error":"busy","job_id":null}"#;
        let mut wire = format!("HTTP/1.1 503 Unavailable\r\nContent-Length: {}\r\n\r\n", body.len() + 1).into_bytes();
        wire.extend_from_slice(body);
        assert!(matches!(read_json_response(&mut wire.as_slice()), Err(RequestError::Other(s)) if s == "incomplete response body"));
    }

    #[test]
    fn fragmented_empty503_requires_complete_framing() {
        let wire = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        for split in 0..=wire.len() {
            let mut stream = wire[..split].chain(&wire[split..]);
            assert_eq!(read_json_response(&mut stream), Err(RequestError::Empty503));
        }
    }

    #[test]
    fn read_disconnect_is_never_classified_as_before_send() {
        struct Disconnect;
        impl Read for Disconnect {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connect reset after send"))
            }
        }
        let wire = b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        for end in 0..wire.len() {
            let mut stream = wire[..end].chain(Disconnect);
            assert!(matches!(read_json_response(&mut stream), Err(RequestError::Other(_))));
        }
    }
}
