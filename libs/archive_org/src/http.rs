//! Transport: HTTPS only, pinned to the archive, bounded everywhere.
//!
//! Two shapes of request, because they have two shapes of cost:
//!
//! * [`fetch_bytes`] — small bodies (search pages, item metadata, tile
//!   thumbnails) through `makepad_network::blocking_http`, which already
//!   refuses cleartext, caps heads and bodies, and never follows a
//!   redirect on its own. The redirect loop lives HERE, so every hop is
//!   checked against the archive's hosts before it is contacted.
//! * [`download_to_file`] — media. A clip is hundreds of megabytes; it
//!   streams straight to disk with progress and cooperative cancel, over
//!   the platform TLS socket (`SocketStream`), with its own minimal
//!   HTTP/1.1 reader: Content-Length or chunked framing, nothing else. The
//!   file is written beside its final name as `<name>.part` and renamed
//!   only when every byte has landed, so a killed download never looks
//!   like a finished one.

use crate::url::{is_archive_host, parse_https, resolve_location, HttpsUrl};
use makepad_network::blocking_http::{self, CancelToken, Limits, Request};
use makepad_network::SocketStream;
use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

/// Redirect hops followed before giving up. The archive needs one
/// (`/download/…` → the storage node); five leaves room for a bounce.
pub const MAX_REDIRECTS: usize = 5;

/// Head bytes a streamed response may spend on status + headers.
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Per-read stall guard on a streamed download. A storage node that goes
/// silent this long is not coming back on this connection.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Wall-clock cap for one small request (blocking_http's total_timeout).
const SMALL_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const USER_AGENT: &str = "makepad-archive-org/0.1";
pub(crate) const RANGE_READ_TIMEOUT: Duration = READ_TIMEOUT;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidUrl,
    /// A redirect pointed off the archive.
    ForeignHost(String),
    Http(blocking_http::Error),
    /// A final (non-3xx) status that is not 200.
    Status(u16),
    TooManyRedirects,
    Io(String),
    Json(String),
    TooLarge,
    Cancelled,
    Timeout,
    BadResponse(&'static str),
    /// The server answered a byte-range request with the whole file.
    NoRangeSupport,
}

impl From<blocking_http::Error> for Error {
    fn from(e: blocking_http::Error) -> Self {
        match e {
            blocking_http::Error::Cancelled => Error::Cancelled,
            blocking_http::Error::Timeout => Error::Timeout,
            blocking_http::Error::ResponseTooLarge => Error::TooLarge,
            blocking_http::Error::InvalidUrl => Error::InvalidUrl,
            other => Error::Http(other),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::InvalidUrl => write!(f, "invalid url"),
            Error::ForeignHost(h) => write!(f, "redirect to a non-archive host refused: {h}"),
            Error::Http(e) => write!(f, "http: {e}"),
            Error::Status(s) => write!(f, "archive.org answered {s}"),
            Error::TooManyRedirects => write!(f, "too many redirects"),
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Json(e) => write!(f, "unexpected json: {e}"),
            Error::TooLarge => write!(f, "response over the size limit"),
            Error::Cancelled => write!(f, "cancelled"),
            Error::Timeout => write!(f, "timed out"),
            Error::BadResponse(why) => write!(f, "malformed response: {why}"),
            Error::NoRangeSupport => write!(f, "server does not serve byte ranges"),
        }
    }
}

impl std::error::Error for Error {}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// GET a small body. Follows up to [`MAX_REDIRECTS`] hops, each of which
/// must stay on the archive; refuses anything but a final 200.
pub fn fetch_bytes(url: &str, max_bytes: usize, cancel: &CancelToken) -> Result<Vec<u8>, Error> {
    let mut current = parse_https(url)?;
    for _ in 0..=MAX_REDIRECTS {
        if !is_archive_host(&current.host) {
            return Err(Error::ForeignHost(current.host.clone()));
        }
        let limits = Limits {
            max_body_bytes: max_bytes,
            total_timeout: SMALL_TIMEOUT,
            ..Limits::default()
        };
        let req = Request::get(current.to_string())
            .limits(limits)
            .cancel_token(cancel.clone());
        let response = blocking_http::request_no_redirect(req)?;
        if is_redirect(response.status) {
            let location = response.header("location").ok_or(Error::BadResponse("redirect without location"))?;
            current = resolve_location(&current, location)?;
            continue;
        }
        if response.status != 200 {
            return Err(Error::Status(response.status));
        }
        return Ok(response.body);
    }
    Err(Error::TooManyRedirects)
}

/// Where a streamed download has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub loaded: u64,
    /// From Content-Length; `None` while the server has not said.
    pub total: Option<u64>,
}

/// Stream a media file to `dest`. Redirects follow the same archive-only
/// rule as [`fetch_bytes`]; the body is refused past `max_bytes` (before a
/// byte is read when Content-Length says so, mid-stream otherwise). The
/// destination directory is created; the finished file is renamed into
/// place atomically. Returns the byte count.
pub fn download_to_file(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<u64, Error> {
    download_impl(url, dest, max_bytes, None, cancel, on_progress).map(|(n, _)| n)
}

/// Stream only the first `head_bytes` of a file to `dest` — for auditioning
/// something far bigger than anyone wants to wait for. Stops cleanly at
/// the cap (the connection is simply dropped) and renames the head into
/// place like a whole file. Returns `(bytes, truncated)`; `total` in the
/// progress reports is clamped to the cap so a bar reads 100% at the end.
pub fn download_head_to_file(
    url: &str,
    dest: &Path,
    head_bytes: u64,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(u64, bool), Error> {
    download_impl(url, dest, u64::MAX, Some(head_bytes.max(1)), cancel, on_progress)
}

fn download_impl(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    stop_at: Option<u64>,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(u64, bool), Error> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io(e.to_string()))?;
    }
    // `<stem>.part.<ext>`: the real extension stays last so a host that
    // starts playing before the end can hand the growing file to a decoder
    // that sniffs by name (see `crate::cache::part_file_for`).
    let part = crate::cache::part_file_for(dest);
    let mut current = parse_https(url)?;
    for _ in 0..=MAX_REDIRECTS {
        if !is_archive_host(&current.host) {
            let _ = std::fs::remove_file(&part);
            return Err(Error::ForeignHost(current.host.clone()));
        }
        let mut file = std::fs::File::create(&part).map_err(|e| Error::Io(e.to_string()))?;
        let mut written = 0u64;
        let outcome = stream_get(&current, cancel, max_bytes, stop_at, &mut |chunk| {
            file.write_all(chunk).map_err(|e| Error::Io(e.to_string()))?;
            written += chunk.len() as u64;
            Ok(())
        }, on_progress);
        match outcome {
            Ok(Streamed::Done { truncated }) => {
                file.sync_all().map_err(|e| Error::Io(e.to_string()))?;
                drop(file);
                std::fs::rename(&part, dest).map_err(|e| Error::Io(e.to_string()))?;
                return Ok((written, truncated));
            }
            Ok(Streamed::Redirect(next)) => {
                drop(file);
                current = next;
            }
            Err(e) => {
                drop(file);
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        }
    }
    let _ = std::fs::remove_file(&part);
    Err(Error::TooManyRedirects)
}

enum Streamed {
    Done { truncated: bool },
    Redirect(HttpsUrl),
}

/// How much of `chunk` a capped download keeps once `loaded` bytes are
/// in, and whether that fills the cap.
fn clip_to_cap(chunk_len: usize, loaded: u64, cap: Option<u64>) -> (usize, bool) {
    match cap {
        None => (chunk_len, false),
        Some(cap) => {
            let room = cap.saturating_sub(loaded);
            if (chunk_len as u64) >= room {
                (room as usize, true)
            } else {
                (chunk_len, false)
            }
        }
    }
}

/// One hop of a streamed GET: connect over TLS, write the request, parse
/// the head, then hand the body to `sink` as it arrives. With `stop_at`
/// the body is abandoned cleanly once that many bytes are in.
fn stream_get(
    url: &HttpsUrl,
    cancel: &CancelToken,
    max_bytes: u64,
    stop_at: Option<u64>,
    sink: &mut dyn FnMut(&[u8]) -> Result<(), Error>,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<Streamed, Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let port = url.port.to_string();
    let mut stream = SocketStream::connect(&url.host, &port, true, false)
        .map_err(|e| Error::Io(format!("connect {}: {e}", url.host)))?;
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| Error::Io(e.to_string()))?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        url.target, url.host, USER_AGENT
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| Error::Io(format!("send: {e}")))?;
    let mut reader = ByteReader::new(stream, cancel.clone());
    let head = reader.read_head()?;
    if is_redirect(head.status) {
        let location = head
            .header("location")
            .ok_or(Error::BadResponse("redirect without location"))?;
        return Ok(Streamed::Redirect(resolve_location(url, location)?));
    }
    if head.status != 200 {
        return Err(Error::Status(head.status));
    }
    let chunked = head
        .header("transfer-encoding")
        .map(|te| te.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);
    let content_length = if chunked {
        None
    } else {
        match head.header("content-length") {
            Some(v) => Some(v.trim().parse::<u64>().map_err(|_| Error::BadResponse("bad content-length"))?),
            None => None,
        }
    };
    if let (Some(total), None) = (content_length, stop_at) {
        if total > max_bytes {
            return Err(Error::TooLarge);
        }
    }
    // What the bar counts to: the whole body, or the cap when there is one.
    let shown_total = match (content_length, stop_at) {
        (Some(total), Some(cap)) => Some(total.min(cap)),
        (Some(total), None) => Some(total),
        (None, Some(cap)) => Some(cap),
        (None, None) => None,
    };
    let mut loaded = 0u64;
    let mut filled = false;
    let mut last_report = Instant::now();
    on_progress(Progress { loaded: 0, total: shown_total });
    let mut deliver = |chunk: &[u8], loaded: &mut u64, filled: &mut bool| -> Result<bool, Error> {
        let (take, done) = clip_to_cap(chunk.len(), *loaded, stop_at);
        let chunk = &chunk[..take];
        *loaded += chunk.len() as u64;
        if *loaded > max_bytes {
            return Err(Error::TooLarge);
        }
        if !chunk.is_empty() {
            sink(chunk)?;
        }
        if last_report.elapsed() >= Duration::from_millis(100) {
            last_report = Instant::now();
            on_progress(Progress { loaded: *loaded, total: shown_total });
        }
        if done {
            *filled = true;
        }
        Ok(!done)
    };
    if chunked {
        reader.read_chunked(&mut |chunk| deliver(chunk, &mut loaded, &mut filled))?;
    } else if let Some(total) = content_length {
        reader.read_exact_to(total, &mut |chunk| deliver(chunk, &mut loaded, &mut filled))?;
    } else {
        reader.read_to_end_to(&mut |chunk| deliver(chunk, &mut loaded, &mut filled))?;
    }
    on_progress(Progress { loaded, total: shown_total.or(Some(loaded)) });
    let truncated = filled && content_length.map(|t| loaded < t).unwrap_or(true);
    Ok(Streamed::Done { truncated })
}

/// A parsed response head. Header names are lowercased.
#[derive(Debug)]
pub(crate) struct Head {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
}

impl Head {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Buffered reader over a socket with cooperative cancel: everything the
/// streamed path needs from HTTP/1.1, and nothing it does not.
pub(crate) struct ByteReader<R: Read> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
    cancel: CancelToken,
}

impl<R: Read> ByteReader<R> {
    pub(crate) fn new(inner: R, cancel: CancelToken) -> Self {
        Self { inner, buf: Vec::with_capacity(64 * 1024), pos: 0, cancel }
    }

    /// The transport, for writing the next request on a kept-alive
    /// connection.
    pub(crate) fn inner_mut(&mut self) -> &mut R {
        &mut self.inner
    }

    /// Bytes already pulled off the socket but not consumed — on a
    /// kept-alive connection there must be none between responses.
    pub(crate) fn pending(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Pull more bytes in; `Ok(0)` is EOF.
    fn fill(&mut self) -> Result<usize, Error> {
        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if self.pos > 0 && self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        let mut tmp = [0u8; 32 * 1024];
        match self.inner.read(&mut tmp) {
            Ok(n) => {
                self.buf.extend_from_slice(&tmp[..n]);
                Ok(n)
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(usize::MAX),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err(Error::Timeout)
            }
            Err(e) => Err(Error::Io(e.to_string())),
        }
    }

    fn available(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    fn consume(&mut self, n: usize) {
        self.pos += n;
    }

    /// Status line + headers, up to the blank line.
    pub(crate) fn read_head(&mut self) -> Result<Head, Error> {
        let end = loop {
            if let Some(i) = find(self.available(), b"\r\n\r\n") {
                break i;
            }
            if self.available().len() > MAX_HEAD_BYTES {
                return Err(Error::BadResponse("head too large"));
            }
            match self.fill()? {
                0 => return Err(Error::BadResponse("connection closed before headers")),
                _ => {}
            }
        };
        let head = std::str::from_utf8(&self.available()[..end])
            .map_err(|_| Error::BadResponse("non-utf8 head"))?
            .to_string();
        self.consume(end + 4);
        let mut lines = head.split("\r\n");
        let status_line = lines.next().ok_or(Error::BadResponse("empty head"))?;
        let mut parts = status_line.splitn(3, ' ');
        let version = parts.next().unwrap_or("");
        if !version.starts_with("HTTP/1.") {
            return Err(Error::BadResponse("not http/1.x"));
        }
        let status = parts
            .next()
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or(Error::BadResponse("bad status"))?;
        let mut headers = Vec::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let (k, v) = line.split_once(':').ok_or(Error::BadResponse("bad header line"))?;
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
            if headers.len() > 128 {
                return Err(Error::BadResponse("too many headers"));
            }
        }
        Ok(Head { status, headers })
    }

    /// One CRLF-terminated line (without the CRLF), bounded.
    fn read_line(&mut self, max: usize) -> Result<String, Error> {
        loop {
            if let Some(i) = find(self.available(), b"\r\n") {
                let line = std::str::from_utf8(&self.available()[..i])
                    .map_err(|_| Error::BadResponse("non-utf8 line"))?
                    .to_string();
                self.consume(i + 2);
                return Ok(line);
            }
            if self.available().len() > max {
                return Err(Error::BadResponse("line too long"));
            }
            if self.fill()? == 0 {
                return Err(Error::BadResponse("connection closed mid-line"));
            }
        }
    }

    /// Exactly `n` body bytes into `sink`; a sink answering `false` stops
    /// the read early (the caller abandons the connection). Returns
    /// whether the read ran to its end.
    pub(crate) fn read_exact_to(
        &mut self,
        mut n: u64,
        sink: &mut dyn FnMut(&[u8]) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        while n > 0 {
            if self.available().is_empty() && self.fill()? == 0 {
                return Err(Error::BadResponse("connection closed before the body ended"));
            }
            let take = (self.available().len() as u64).min(n) as usize;
            if take > 0 {
                let chunk = self.buf[self.pos..self.pos + take].to_vec();
                let more = sink(&chunk)?;
                self.consume(take);
                n -= take as u64;
                if !more {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Everything until the peer closes (or the sink says stop).
    fn read_to_end_to(
        &mut self,
        sink: &mut dyn FnMut(&[u8]) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        loop {
            if !self.available().is_empty() {
                let chunk = self.available().to_vec();
                let more = sink(&chunk)?;
                let n = chunk.len();
                self.consume(n);
                if !more {
                    return Ok(false);
                }
            }
            if self.fill()? == 0 {
                return Ok(true);
            }
        }
    }

    /// `Transfer-Encoding: chunked` body, trailers discarded.
    fn read_chunked(
        &mut self,
        sink: &mut dyn FnMut(&[u8]) -> Result<bool, Error>,
    ) -> Result<bool, Error> {
        loop {
            let line = self.read_line(1024)?;
            let size_hex = line.split(';').next().unwrap_or("").trim();
            let size = u64::from_str_radix(size_hex, 16)
                .map_err(|_| Error::BadResponse("bad chunk size"))?;
            if size == 0 {
                // Trailers until the empty line.
                loop {
                    let t = self.read_line(8 * 1024)?;
                    if t.is_empty() {
                        return Ok(true);
                    }
                }
            }
            if !self.read_exact_to(size, sink)? {
                return Ok(false);
            }
            let crlf = self.read_line(2)?;
            if !crlf.is_empty() {
                return Err(Error::BadResponse("chunk not CRLF-terminated"));
            }
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn reader(bytes: &[u8]) -> ByteReader<Cursor<Vec<u8>>> {
        ByteReader::new(Cursor::new(bytes.to_vec()), CancelToken::new())
    }

    fn collect(r: &mut ByteReader<Cursor<Vec<u8>>>, chunked: bool, len: Option<u64>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut sink = |c: &[u8]| {
            out.extend_from_slice(c);
            Ok(true)
        };
        if chunked {
            r.read_chunked(&mut sink).unwrap();
        } else if let Some(n) = len {
            r.read_exact_to(n, &mut sink).unwrap();
        } else {
            r.read_to_end_to(&mut sink).unwrap();
        }
        out
    }

    #[test]
    fn head_and_content_length() {
        let mut r = reader(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 5\r\n\r\nhello");
        let head = r.read_head().unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.header("content-length"), Some("5"));
        assert_eq!(collect(&mut r, false, Some(5)), b"hello");
    }

    #[test]
    fn chunked_body_with_extension_and_trailer() {
        let mut r = reader(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4;ext=1\r\nWiki\r\n5\r\npedia\r\n0\r\nX-T: 1\r\n\r\n",
        );
        let head = r.read_head().unwrap();
        assert_eq!(head.header("transfer-encoding"), Some("chunked"));
        assert_eq!(collect(&mut r, true, None), b"Wikipedia");
    }

    #[test]
    fn read_to_eof() {
        let mut r = reader(b"HTTP/1.0 200 OK\r\n\r\nabcdef");
        r.read_head().unwrap();
        assert_eq!(collect(&mut r, false, None), b"abcdef");
    }

    #[test]
    fn redirect_head() {
        let mut r = reader(b"HTTP/1.1 302 Found\r\nLocation: https://ia1.us.archive.org/x\r\n\r\n");
        let head = r.read_head().unwrap();
        assert!(is_redirect(head.status));
        assert_eq!(head.header("location"), Some("https://ia1.us.archive.org/x"));
    }

    #[test]
    fn truncated_body_is_an_error() {
        let mut r = reader(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc");
        r.read_head().unwrap();
        let mut sink = |_: &[u8]| Ok(true);
        assert!(matches!(r.read_exact_to(10, &mut sink), Err(Error::BadResponse(_))));
    }

    #[test]
    fn a_sink_can_stop_early() {
        let mut r = reader(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\n\r\nabcdef");
        r.read_head().unwrap();
        let mut got = Vec::new();
        let mut sink = |c: &[u8]| {
            got.extend_from_slice(c);
            Ok(false)
        };
        assert_eq!(r.read_exact_to(6, &mut sink).unwrap(), false);
        assert!(!got.is_empty());
        assert_eq!(clip_to_cap(10, 0, None), (10, false));
        assert_eq!(clip_to_cap(10, 95, Some(100)), (5, true));
        assert_eq!(clip_to_cap(3, 0, Some(100)), (3, false));
        assert_eq!(clip_to_cap(3, 100, Some(100)), (0, true));
    }

    #[test]
    fn garbage_head_is_refused() {
        let mut r = reader(b"<html>nope\r\n\r\n");
        assert!(matches!(r.read_head(), Err(Error::BadResponse(_))));
    }

    #[test]
    fn cancel_stops_reading() {
        let cancel = CancelToken::new();
        let mut r = ByteReader::new(Cursor::new(b"HTTP/1.1 200 OK\r\n\r\n".to_vec()), cancel.clone());
        cancel.cancel();
        assert_eq!(r.read_head().unwrap_err(), Error::Cancelled);
    }
}
