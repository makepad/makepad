//! Byte ranges on demand: the transport a streaming player wants.
//!
//! One kept-alive HTTPS connection to the storage node; every call is a
//! `Range: bytes=a-b` GET answered with 206 and exactly those bytes.
//! Nothing is written to disk, nothing is fetched that was not asked for,
//! and a seek to minute forty is one request. The redirect from
//! `archive.org/download/…` to the node is followed once, at open, and
//! the node URL is what every range then goes to.
//!
//! A connection the server closed between requests (idle timeout) is
//! reopened transparently, once per read.

use crate::http::{ByteReader, Error, Head, MAX_REDIRECTS, RANGE_READ_TIMEOUT, USER_AGENT};
use crate::url::{is_archive_host, parse_https, resolve_location, HttpsUrl};
use makepad_network::blocking_http::CancelToken;
use makepad_network::SocketStream;
use std::io::Write;

/// Largest single range a caller may ask for (a player fetches windows
/// of a few MB; a whole file would be a download, which is a different
/// tool).
pub const MAX_RANGE_BYTES: usize = 64 * 1024 * 1024;

pub struct RangeSource {
    url: HttpsUrl,
    size: u64,
    conn: Option<ByteReader<SocketStream>>,
    cancel: CancelToken,
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// `bytes 0-0/12345` → 12345. `*/12345` (unsatisfiable) also yields the
/// total.
pub(crate) fn parse_content_range_total(value: &str) -> Option<u64> {
    let value = value.trim();
    let rest = value.strip_prefix("bytes")?.trim();
    let (_, total) = rest.rsplit_once('/')?;
    total.trim().parse::<u64>().ok()
}

fn connect(url: &HttpsUrl, cancel: &CancelToken) -> Result<ByteReader<SocketStream>, Error> {
    if !is_archive_host(&url.host) {
        return Err(Error::ForeignHost(url.host.clone()));
    }
    let port = url.port.to_string();
    let stream = SocketStream::connect(&url.host, &port, true, false)
        .map_err(|e| Error::Io(format!("connect {}: {e}", url.host)))?;
    stream
        .set_read_timeout(Some(RANGE_READ_TIMEOUT))
        .map_err(|e| Error::Io(e.to_string()))?;
    Ok(ByteReader::new(stream, cancel.clone()))
}

fn send_range(conn: &mut ByteReader<SocketStream>, url: &HttpsUrl, first: u64, last: u64) -> Result<(), Error> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nRange: bytes={}-{}\r\nConnection: keep-alive\r\n\r\n",
        url.target, url.host, USER_AGENT, first, last
    );
    conn.inner_mut()
        .write_all(request.as_bytes())
        .map_err(|e| Error::Io(format!("send: {e}")))
}

fn keeps_alive(head: &Head) -> bool {
    !head
        .header("connection")
        .map(|c| c.to_ascii_lowercase().contains("close"))
        .unwrap_or(false)
}

fn read_body(conn: &mut ByteReader<SocketStream>, len: u64) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(len as usize);
    conn.read_exact_to(len, &mut |chunk| {
        out.extend_from_slice(chunk);
        Ok(true)
    })?;
    Ok(out)
}

impl RangeSource {
    /// Resolve `url` (following the archive's redirect to its storage
    /// node) and learn the file's size with a one-byte range. Fails with
    /// [`Error::NoRangeSupport`] if the server answers 200 instead.
    pub fn open(url: &str, cancel: &CancelToken) -> Result<RangeSource, Error> {
        let mut current = parse_https(url)?;
        for _ in 0..=MAX_REDIRECTS {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let mut conn = connect(&current, cancel)?;
            send_range(&mut conn, &current, 0, 0)?;
            let head = conn.read_head()?;
            if is_redirect(head.status) {
                let location = head
                    .header("location")
                    .ok_or(Error::BadResponse("redirect without location"))?;
                current = resolve_location(&current, location)?;
                continue;
            }
            match head.status {
                206 => {
                    let total = head
                        .header("content-range")
                        .and_then(parse_content_range_total)
                        .ok_or(Error::BadResponse("206 without a content-range total"))?;
                    let len = head
                        .header("content-length")
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .unwrap_or(1);
                    let _ = read_body(&mut conn, len)?;
                    let conn = if keeps_alive(&head) && conn.pending() == 0 { Some(conn) } else { None };
                    return Ok(RangeSource { url: current, size: total, conn, cancel: cancel.clone() });
                }
                200 => return Err(Error::NoRangeSupport),
                416 => {
                    // Empty file: unsatisfiable, but the total says so.
                    let total = head
                        .header("content-range")
                        .and_then(parse_content_range_total)
                        .unwrap_or(0);
                    return Ok(RangeSource { url: current, size: total, conn: None, cancel: cancel.clone() });
                }
                status => return Err(Error::Status(status)),
            }
        }
        Err(Error::TooManyRedirects)
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    /// The storage-node URL every range goes to.
    pub fn url(&self) -> String {
        self.url.to_string()
    }

    /// `len` bytes at `offset` (fewer only at the end of the file).
    pub fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, Error> {
        if len == 0 || offset >= self.size {
            return Ok(Vec::new());
        }
        if len > MAX_RANGE_BYTES {
            return Err(Error::TooLarge);
        }
        let last = offset.saturating_add(len as u64 - 1).min(self.size - 1);
        // One retry: a kept-alive connection the server dropped while we
        // were decoding is the normal case, not a failure.
        let mut attempt = 0;
        loop {
            attempt += 1;
            let fresh = self.conn.is_none();
            let mut conn = match self.conn.take() {
                Some(c) => c,
                None => connect(&self.url, &self.cancel)?,
            };
            match Self::range_once(&mut conn, &self.url, offset, last) {
                Ok((bytes, keep)) => {
                    if keep && conn.pending() == 0 {
                        self.conn = Some(conn);
                    }
                    return Ok(bytes);
                }
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) if !fresh && attempt == 1 => {
                    // Stale keep-alive: reconnect and try once more.
                    let _ = e;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn range_once(
        conn: &mut ByteReader<SocketStream>,
        url: &HttpsUrl,
        first: u64,
        last: u64,
    ) -> Result<(Vec<u8>, bool), Error> {
        send_range(conn, url, first, last)?;
        let head = conn.read_head()?;
        match head.status {
            206 => {
                let len = head
                    .header("content-length")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .ok_or(Error::BadResponse("206 without content-length"))?;
                if len > (last - first + 1) {
                    return Err(Error::BadResponse("range answer longer than asked"));
                }
                let bytes = read_body(conn, len)?;
                Ok((bytes, keeps_alive(&head)))
            }
            200 => Err(Error::NoRangeSupport),
            status => Err(Error::Status(status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range_totals() {
        assert_eq!(parse_content_range_total("bytes 0-0/61878609"), Some(61878609));
        assert_eq!(parse_content_range_total(" bytes 100-199/200 "), Some(200));
        assert_eq!(parse_content_range_total("bytes */5"), Some(5));
        assert_eq!(parse_content_range_total("items 0-0/5"), None);
        assert_eq!(parse_content_range_total("bytes 0-0/x"), None);
    }
}
