//! Bounded HTTP/1.1 server transport, written for hostile input.
//!
//! Hard rules, all fail-closed with connection close on violation:
//! - HTTP/1.1 origin-form only; anything else is 505/501/400.
//! - Request smuggling: Content-Length + Transfer-Encoding together is 400;
//!   Transfer-Encoding must be exactly `chunked` (no lists, no extensions,
//!   no trailers, bounded hex size lines).
//! - Budgets everywhere: head bytes, header line bytes, header count, target
//!   length, query pair count, body bytes, chunk size digits.
//! - No percent-decoding: a literal `%` in the target is refused, so routes
//!   only ever see the byte-for-byte charset below and no decoded alias can
//!   smuggle separators. Path segments `.` and `..` are refused outright.
//! - Duplicate tracked headers and duplicate query keys are refused.
//! - Head/body wall-clock deadlines (slowloris) on top of socket timeouts.
//! - Every response carries `X-Content-Type-Options: nosniff`.

use super::json;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const MAX_HEAD_BYTES: usize = 32 * 1024;
pub const MAX_HEADER_LINE: usize = 8 * 1024;
pub const MAX_HEADERS: usize = 64;
pub const MAX_TARGET_BYTES: usize = 2048;
pub const MAX_QUERY_PAIRS: usize = 16;
/// Unconsumed Length-body remainder we are willing to drain to keep a
/// connection alive; anything larger closes instead.
pub const MAX_DRAIN_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// Body framing + consumption tracker for one request.
#[derive(Debug)]
pub enum BodyState {
    None,
    Length { remaining: u64, total: u64 },
    Chunked { chunk: ChunkPhase, total: u64 },
    Done,
}

#[derive(Debug)]
pub enum ChunkPhase {
    SizeLine { line: Vec<u8> },
    Data { remaining: u64 },
    DataCr,
    DataLf,
    FinalCr,
    FinalLf,
}

impl BodyState {
    pub fn declared_len(&self) -> Option<u64> {
        match self {
            BodyState::Length { total, .. } => Some(*total),
            _ => None,
        }
    }

    pub fn has_body(&self) -> bool {
        !matches!(self, BodyState::None | BodyState::Done)
    }

    fn consumed(&self) -> bool {
        match self {
            BodyState::None | BodyState::Done => true,
            BodyState::Length { remaining, .. } => *remaining == 0,
            BodyState::Chunked { .. } => false,
        }
    }
}

#[derive(Debug)]
pub struct Head {
    pub method: Method,
    /// Raw target for logging; charset-restricted at parse time.
    pub target: String,
    /// Path segments; `/` alone parses to zero segments.
    pub segs: Vec<String>,
    pub query: Vec<(String, String)>,
    pub host: String,
    pub authorization: Option<String>,
    pub range: Option<String>,
    pub if_none_match: Option<String>,
    pub if_range: Option<String>,
    pub expect_continue: bool,
    /// 100-continue still owed to the client before reading the body.
    pub continue_pending: bool,
    pub close: bool,
    pub body: BodyState,
}

impl Head {
    pub fn query_get(&self, key: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

pub enum HeadError {
    /// Clean EOF (or stop request) at an idle request boundary.
    Closed,
    /// Deadline expired with a partial head buffered: respond 408 and close.
    Timeout,
    /// Socket error / EOF mid-head: close silently.
    Io,
    /// Parse violation: respond with this status and close.
    Bad(u16, &'static str),
}

pub enum BodyError {
    /// Body deadline expired: 408, close.
    Timeout,
    /// Framing violation: 400, close.
    Malformed,
    /// Body exceeds the caller's byte budget: 413, close.
    TooLarge,
    /// Socket failure: close silently.
    Io,
}

enum Fill {
    Data,
    Eof,
    Retry,
}

pub struct Conn {
    stream: TcpStream,
    buf: Vec<u8>,
    pos: usize,
    read_timeout: Duration,
}

impl Conn {
    pub fn new(stream: TcpStream, read_timeout_ms: u64, write_timeout_ms: u64) -> std::io::Result<Conn> {
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(Duration::from_millis(write_timeout_ms.max(1))))?;
        Ok(Conn {
            stream,
            buf: Vec::with_capacity(16 * 1024),
            pos: 0,
            read_timeout: Duration::from_millis(read_timeout_ms.max(1)),
        })
    }

    fn buffered(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    fn consume(&mut self, n: usize) {
        self.pos += n;
        debug_assert!(self.pos <= self.buf.len());
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
    }

    /// One read attempt appending to the buffer, bounded by both the socket
    /// timeout and the caller's wall-clock deadline.
    fn fill(&mut self, deadline: Instant) -> Result<Fill, ()> {
        let now = Instant::now();
        if now >= deadline {
            return Ok(Fill::Retry); // caller maps to timeout
        }
        let remaining = deadline - now;
        let timeout = remaining.min(self.read_timeout).max(Duration::from_millis(1));
        self.stream.set_read_timeout(Some(timeout)).map_err(|_| ())?;
        // Compact once the consumed prefix dominates the buffer.
        if self.pos > 64 * 1024 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        let len = self.buf.len();
        self.buf.resize(len + 16 * 1024, 0);
        match self.stream.read(&mut self.buf[len..]) {
            Ok(0) => {
                self.buf.truncate(len);
                Ok(Fill::Eof)
            }
            Ok(n) => {
                self.buf.truncate(len + n);
                Ok(Fill::Data)
            }
            Err(e) => {
                self.buf.truncate(len);
                match e.kind() {
                    std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted => Ok(Fill::Retry),
                    _ => Err(()),
                }
            }
        }
    }

    /// Read and parse one request head. Pipelined bytes already buffered are
    /// honored before touching the socket.
    ///
    /// Two-phase deadlines: while the connection is idle (not one byte of the
    /// next head yet), waits up to `idle_wait_ms` in short slices, checking
    /// `stop` between slices — a stopping server sheds idle keep-alive
    /// connections within ~250ms. Once the first byte arrives, the full
    /// `head_deadline_ms` slowloris budget applies to completing the head.
    pub fn next_request(
        &mut self,
        head_deadline_ms: u64,
        idle_wait_ms: u64,
        stop: &AtomicBool,
    ) -> Result<Head, HeadError> {
        let idle_deadline = Instant::now() + Duration::from_millis(idle_wait_ms.max(1));
        while self.buffered().is_empty() {
            if stop.load(Ordering::Relaxed) {
                return Err(HeadError::Closed);
            }
            let now = Instant::now();
            if now >= idle_deadline {
                return Err(HeadError::Closed);
            }
            let slice = (idle_deadline - now).min(Duration::from_millis(250));
            match self.fill(now + slice) {
                Ok(Fill::Data) => break,
                Ok(Fill::Eof) => return Err(HeadError::Closed),
                Ok(Fill::Retry) => continue,
                Err(()) => return Err(HeadError::Io),
            }
        }
        let deadline = Instant::now() + Duration::from_millis(head_deadline_ms.max(1));
        loop {
            if let Some(idx) = find_terminator(self.buffered()) {
                let block_end = self.pos + idx;
                let head = parse_head(&self.buf[self.pos..block_end])
                    .map_err(|(s, m)| HeadError::Bad(s, m))?;
                self.pos = block_end + 4;
                if self.pos == self.buf.len() {
                    self.buf.clear();
                    self.pos = 0;
                }
                return Ok(head);
            }
            if self.buffered().len() > MAX_HEAD_BYTES {
                return Err(HeadError::Bad(431, "request head too large"));
            }
            if Instant::now() >= deadline {
                return Err(HeadError::Timeout);
            }
            match self.fill(deadline) {
                Ok(Fill::Data) => continue,
                Ok(Fill::Eof) => return Err(HeadError::Io),
                Ok(Fill::Retry) => continue,
                Err(()) => return Err(HeadError::Io),
            }
        }
    }

    fn send_continue(&mut self, head: &mut Head) -> Result<(), BodyError> {
        if head.continue_pending {
            head.continue_pending = false;
            self.stream
                .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
                .map_err(|_| BodyError::Io)?;
        }
        Ok(())
    }

    /// Read up to `out.len()` decoded body bytes. Returns 0 exactly when the
    /// body is fully consumed. `max_total` bounds cumulative decoded bytes.
    pub fn body_read(
        &mut self,
        head: &mut Head,
        out: &mut [u8],
        max_total: u64,
        deadline: Instant,
    ) -> Result<usize, BodyError> {
        if out.is_empty() {
            return Err(BodyError::Io);
        }
        match &head.body {
            BodyState::None | BodyState::Done => return Ok(0),
            BodyState::Length { total, .. } => {
                if *total > max_total {
                    return Err(BodyError::TooLarge);
                }
            }
            BodyState::Chunked { .. } => {}
        }
        self.send_continue(head)?;
        loop {
            match &mut head.body {
                BodyState::None | BodyState::Done => return Ok(0),
                BodyState::Length { remaining, .. } => {
                    if *remaining == 0 {
                        head.body = BodyState::Done;
                        return Ok(0);
                    }
                    let avail = self.buf.len() - self.pos;
                    if avail == 0 {
                        self.body_fill(deadline)?;
                        continue;
                    }
                    let n = (avail as u64).min(*remaining).min(out.len() as u64) as usize;
                    out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
                    *remaining -= n as u64;
                    self.consume(n);
                    return Ok(n);
                }
                BodyState::Chunked { .. } => {
                    if let Some(n) = self.chunked_step(head, out, max_total, deadline)? {
                        return Ok(n);
                    }
                }
            }
        }
    }

    fn body_fill(&mut self, deadline: Instant) -> Result<(), BodyError> {
        if Instant::now() >= deadline {
            return Err(BodyError::Timeout);
        }
        match self.fill(deadline) {
            Ok(Fill::Data) => Ok(()),
            Ok(Fill::Eof) => Err(BodyError::Malformed),
            Ok(Fill::Retry) => {
                if Instant::now() >= deadline {
                    Err(BodyError::Timeout)
                } else {
                    Ok(())
                }
            }
            Err(()) => Err(BodyError::Io),
        }
    }

    /// Advance the chunked state machine. `Ok(Some(n))` = n bytes copied out;
    /// `Ok(None)` = made progress, call again.
    fn chunked_step(
        &mut self,
        head: &mut Head,
        out: &mut [u8],
        max_total: u64,
        deadline: Instant,
    ) -> Result<Option<usize>, BodyError> {
        let BodyState::Chunked { chunk, total } = &mut head.body else {
            return Err(BodyError::Malformed);
        };
        match chunk {
            ChunkPhase::SizeLine { line } => {
                let avail = &self.buf[self.pos..];
                if avail.is_empty() {
                    self.body_fill(deadline)?;
                    return Ok(None);
                }
                // Accumulate size-line bytes until LF; the line budget covers
                // 16 hex digits + CR with margin.
                let mut used = 0usize;
                let mut complete = false;
                for &b in avail {
                    used += 1;
                    if b == b'\n' {
                        complete = true;
                        break;
                    }
                    line.push(b);
                    if line.len() > 20 {
                        return Err(BodyError::Malformed);
                    }
                }
                self.consume(used);
                if !complete {
                    return Ok(None);
                }
                if line.last() != Some(&b'\r') {
                    return Err(BodyError::Malformed);
                }
                line.pop();
                if line.is_empty() || line.len() > 16 {
                    return Err(BodyError::Malformed);
                }
                let mut size: u64 = 0;
                for &b in line.iter() {
                    let d = match b {
                        b'0'..=b'9' => (b - b'0') as u64,
                        b'a'..=b'f' => (b - b'a' + 10) as u64,
                        b'A'..=b'F' => (b - b'A' + 10) as u64,
                        // Chunk extensions and any other byte are refused.
                        _ => return Err(BodyError::Malformed),
                    };
                    size = (size << 4) | d;
                }
                if size == 0 {
                    *chunk = ChunkPhase::FinalCr;
                } else {
                    let new_total = total.checked_add(size).ok_or(BodyError::TooLarge)?;
                    if new_total > max_total {
                        return Err(BodyError::TooLarge);
                    }
                    *total = new_total;
                    *chunk = ChunkPhase::Data { remaining: size };
                }
                Ok(None)
            }
            ChunkPhase::Data { remaining } => {
                let avail = self.buf.len() - self.pos;
                if avail == 0 {
                    self.body_fill(deadline)?;
                    return Ok(None);
                }
                let n = (avail as u64).min(*remaining).min(out.len() as u64) as usize;
                out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
                *remaining -= n as u64;
                if *remaining == 0 {
                    *chunk = ChunkPhase::DataCr;
                }
                self.consume(n);
                Ok(Some(n))
            }
            ChunkPhase::DataCr | ChunkPhase::DataLf | ChunkPhase::FinalCr | ChunkPhase::FinalLf => {
                let avail = &self.buf[self.pos..];
                if avail.is_empty() {
                    self.body_fill(deadline)?;
                    return Ok(None);
                }
                let b = avail[0];
                self.consume(1);
                match (&chunk, b) {
                    (ChunkPhase::DataCr, b'\r') => *chunk = ChunkPhase::DataLf,
                    (ChunkPhase::DataLf, b'\n') => *chunk = ChunkPhase::SizeLine { line: Vec::new() },
                    (ChunkPhase::FinalCr, b'\r') => *chunk = ChunkPhase::FinalLf,
                    (ChunkPhase::FinalLf, b'\n') => {
                        // `0\r\n\r\n`: done, trailers rejected by construction.
                        head.body = BodyState::Done;
                        return Ok(Some(0));
                    }
                    _ => return Err(BodyError::Malformed),
                }
                Ok(None)
            }
        }
    }

    /// Buffer an entire body (JSON/manifest routes). Refuses past `max`.
    pub fn read_body_full(
        &mut self,
        head: &mut Head,
        max: u64,
        deadline_ms: u64,
    ) -> Result<Vec<u8>, BodyError> {
        let deadline = Instant::now() + Duration::from_millis(deadline_ms.max(1));
        let mut out = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let n = self.body_read(head, &mut chunk, max, deadline)?;
            if n == 0 {
                return Ok(out);
            }
            if out.len() as u64 + n as u64 > max {
                return Err(BodyError::TooLarge);
            }
            out.extend_from_slice(&chunk[..n]);
        }
    }

    /// Decide whether the connection can be kept alive after a response, and
    /// drain a small unread Length remainder if that is all that blocks it.
    pub fn finish_request(&mut self, head: &mut Head) -> bool {
        if head.close {
            return false;
        }
        if head.body.consumed() {
            return true;
        }
        // A client still owed a 100-continue may or may not send the body;
        // the connection state is ambiguous, so close.
        if head.continue_pending {
            return false;
        }
        match &head.body {
            BodyState::Length { remaining, .. } if *remaining <= MAX_DRAIN_BYTES => {
                let deadline = Instant::now() + Duration::from_millis(2000);
                let mut sink = [0u8; 16 * 1024];
                loop {
                    match self.body_read(head, &mut sink, u64::MAX, deadline) {
                        Ok(0) => return true,
                        Ok(_) => continue,
                        Err(_) => return false,
                    }
                }
            }
            _ => false,
        }
    }

    pub fn write_resp(&mut self, is_head: bool, resp: &Resp) -> std::io::Result<()> {
        let mut out = Vec::with_capacity(256 + resp.body.len());
        head_bytes_into(&mut out, resp.status, &resp.headers, resp.close, |o| {
            // 204/304 must not carry Content-Length or a body.
            if resp.status != 204 && resp.status != 304 {
                o.extend_from_slice(format!("Content-Length: {}\r\n", resp.body.len()).as_bytes());
            }
        });
        if !is_head && resp.status != 204 && resp.status != 304 {
            out.extend_from_slice(&resp.body);
        }
        self.stream.write_all(&out)
    }

    /// Write a response head for a streamed body of exactly `content_length`
    /// bytes; the caller then emits the bytes via `write_chunk`.
    pub fn write_stream_head(
        &mut self,
        status: u16,
        headers: &[(&'static str, String)],
        content_length: u64,
        close: bool,
    ) -> std::io::Result<()> {
        let mut out = Vec::with_capacity(256);
        head_bytes_into(&mut out, status, headers, close, |o| {
            o.extend_from_slice(format!("Content-Length: {content_length}\r\n").as_bytes());
        });
        self.stream.write_all(&out)
    }

    pub fn write_chunk(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes)
    }
}

fn head_bytes_into(
    out: &mut Vec<u8>,
    status: u16,
    headers: &[(&'static str, String)],
    close: bool,
    framing: impl FnOnce(&mut Vec<u8>),
) {
    out.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", status, reason(status)).as_bytes());
    for (k, v) in headers {
        out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
    }
    out.extend_from_slice(b"X-Content-Type-Options: nosniff\r\n");
    framing(out);
    out.extend_from_slice(if close {
        b"Connection: close\r\n"
    } else {
        b"Connection: keep-alive\r\n"
    });
    out.extend_from_slice(b"\r\n");
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        413 => "Payload Too Large",
        414 => "URI Too Long",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        422 => "Unprocessable Entity",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Status",
    }
}

pub struct Resp {
    pub status: u16,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
    pub close: bool,
}

impl Resp {
    pub fn empty(status: u16) -> Resp {
        Resp { status, headers: Vec::new(), body: Vec::new(), close: false }
    }

    pub fn json(status: u16, v: &json::Value) -> Resp {
        Resp {
            status,
            headers: vec![
                ("Content-Type", "application/json".to_string()),
                ("Cache-Control", "no-store".to_string()),
            ],
            body: v.to_json().into_bytes(),
            close: false,
        }
    }

    pub fn error(status: u16, msg: &str) -> Resp {
        Resp::json(status, &json::obj(vec![("error", json::s(msg))]))
    }

    pub fn bytes(status: u16, content_type: &str, body: Vec<u8>) -> Resp {
        Resp {
            status,
            headers: vec![("Content-Type", content_type.to_string())],
            body,
            close: false,
        }
    }

    pub fn with_header(mut self, k: &'static str, v: String) -> Resp {
        self.headers.push((k, v));
        self
    }

    pub fn closing(mut self) -> Resp {
        self.close = true;
        self
    }
}

fn find_terminator(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn target_byte_ok(b: u8) -> bool {
    matches!(b,
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
        | b'-' | b'_' | b'.' | b'/' | b':' | b'?' | b'=' | b'&')
}

fn token_byte_ok(b: u8) -> bool {
    matches!(b,
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9'
        | b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+'
        | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~')
}

fn value_byte_ok(b: u8) -> bool {
    b == b'\t' || (0x20..=0x7e).contains(&b)
}

/// Parse one complete request head (bytes up to, excluding, `\r\n\r\n`).
/// Pure function; returns `(status, message)` on refusal.
pub fn parse_head(block: &[u8]) -> Result<Head, (u16, &'static str)> {
    let mut lines = SplitCrlf { rest: block };
    let request_line = lines.next().ok_or((400, "empty request"))?;

    // ---- request line ----
    let mut parts = request_line.split(|&b| b == b' ');
    let method_b = parts.next().unwrap_or(b"");
    let target_b = parts.next().ok_or((400, "malformed request line"))?;
    let version_b = parts.next().ok_or((400, "malformed request line"))?;
    if parts.next().is_some() {
        return Err((400, "malformed request line"));
    }
    if method_b.is_empty() || method_b.len() > 16 || !method_b.iter().all(u8::is_ascii_alphabetic) {
        return Err((400, "malformed method"));
    }
    if version_b != b"HTTP/1.1" {
        if version_b.starts_with(b"HTTP/") {
            return Err((505, "http/1.1 only"));
        }
        return Err((400, "malformed request line"));
    }
    let method = match method_b {
        b"GET" => Method::Get,
        b"HEAD" => Method::Head,
        b"POST" => Method::Post,
        b"PUT" => Method::Put,
        b"DELETE" => Method::Delete,
        _ => return Err((501, "method not implemented")),
    };
    if target_b.len() > MAX_TARGET_BYTES {
        return Err((414, "target too long"));
    }
    if target_b.contains(&b'%') {
        return Err((400, "percent-encoding not accepted"));
    }
    if target_b.is_empty() || target_b[0] != b'/' {
        return Err((400, "origin-form target required"));
    }
    if !target_b.iter().all(|&b| target_byte_ok(b)) {
        return Err((400, "target charset"));
    }
    let target = String::from_utf8(target_b.to_vec()).map_err(|_| (400, "target charset"))?;

    // ---- path + query ----
    let (raw_path, raw_query) = match target.find('?') {
        Some(i) => (&target[..i], Some(&target[i + 1..])),
        None => (target.as_str(), None),
    };
    let mut segs = Vec::new();
    if raw_path != "/" {
        for seg in raw_path[1..].split('/') {
            if seg.is_empty() {
                return Err((400, "empty path segment"));
            }
            if seg == "." || seg == ".." {
                return Err((400, "dot path segment"));
            }
            segs.push(seg.to_string());
        }
    }
    let mut query: Vec<(String, String)> = Vec::new();
    if let Some(q) = raw_query {
        if q.contains('?') {
            return Err((400, "malformed query"));
        }
        for pair in q.split('&') {
            let eq = pair.find('=').ok_or((400, "malformed query"))?;
            let k = &pair[..eq];
            let v = &pair[eq + 1..];
            if k.is_empty() {
                return Err((400, "malformed query"));
            }
            if query.iter().any(|(qk, _)| qk == k) {
                return Err((400, "duplicate query key"));
            }
            if query.len() >= MAX_QUERY_PAIRS {
                return Err((400, "too many query pairs"));
            }
            query.push((k.to_string(), v.to_string()));
        }
    }

    // ---- headers ----
    let mut host: Option<String> = None;
    let mut authorization: Option<String> = None;
    let mut range: Option<String> = None;
    let mut if_none_match: Option<String> = None;
    let mut if_range: Option<String> = None;
    let mut expect: Option<String> = None;
    let mut connection: Option<String> = None;
    let mut content_length: Option<String> = None;
    let mut transfer_encoding: Option<String> = None;
    let mut count = 0usize;
    for line in lines {
        count += 1;
        if count > MAX_HEADERS {
            return Err((431, "too many headers"));
        }
        if line.len() > MAX_HEADER_LINE {
            return Err((431, "header line too large"));
        }
        if line.first() == Some(&b' ') || line.first() == Some(&b'\t') {
            return Err((400, "obsolete line folding"));
        }
        let colon = line.iter().position(|&b| b == b':').ok_or((400, "malformed header"))?;
        let name_b = &line[..colon];
        if name_b.is_empty() || name_b.len() > 128 || !name_b.iter().all(|&b| token_byte_ok(b)) {
            return Err((400, "malformed header name"));
        }
        let mut value_b = &line[colon + 1..];
        while value_b.first() == Some(&b' ') || value_b.first() == Some(&b'\t') {
            value_b = &value_b[1..];
        }
        while value_b.last() == Some(&b' ') || value_b.last() == Some(&b'\t') {
            value_b = &value_b[..value_b.len() - 1];
        }
        if !value_b.iter().all(|&b| value_byte_ok(b)) {
            return Err((400, "header value charset"));
        }
        let name = String::from_utf8(name_b.to_vec())
            .map_err(|_| (400, "malformed header name"))?
            .to_ascii_lowercase();
        let value = String::from_utf8(value_b.to_vec()).map_err(|_| (400, "header value charset"))?;
        let slot = match name.as_str() {
            "host" => &mut host,
            "authorization" => &mut authorization,
            "range" => &mut range,
            "if-none-match" => &mut if_none_match,
            "if-range" => &mut if_range,
            "expect" => &mut expect,
            "connection" => &mut connection,
            "content-length" => &mut content_length,
            "transfer-encoding" => &mut transfer_encoding,
            _ => continue,
        };
        if slot.is_some() {
            return Err((400, "duplicate header"));
        }
        *slot = Some(value);
    }
    let host = host.ok_or((400, "host header required"))?;

    // ---- connection / expect ----
    let mut close = false;
    if let Some(c) = &connection {
        for token in c.split(',') {
            if token.trim().eq_ignore_ascii_case("close") {
                close = true;
            }
        }
    }
    let expect_continue = match &expect {
        None => false,
        Some(v) if v.eq_ignore_ascii_case("100-continue") => true,
        Some(_) => return Err((417, "unsupported expectation")),
    };

    // ---- body framing ----
    if content_length.is_some() && transfer_encoding.is_some() {
        return Err((400, "content-length with transfer-encoding"));
    }
    let body = if let Some(te) = &transfer_encoding {
        // Exactly `chunked`, lowercase, no list, no parameters. Anything
        // else is refused rather than reinterpreted.
        if te != "chunked" {
            return Err((501, "transfer-encoding not supported"));
        }
        BodyState::Chunked { chunk: ChunkPhase::SizeLine { line: Vec::new() }, total: 0 }
    } else if let Some(cl) = &content_length {
        let b = cl.as_bytes();
        if b.is_empty() || b.len() > 20 || !b.iter().all(u8::is_ascii_digit) {
            return Err((400, "malformed content-length"));
        }
        if b.len() > 1 && b[0] == b'0' {
            return Err((400, "malformed content-length"));
        }
        let n: u64 = cl.parse().map_err(|_| (400, "malformed content-length"))?;
        if n == 0 {
            BodyState::None
        } else {
            BodyState::Length { remaining: n, total: n }
        }
    } else {
        BodyState::None
    };
    if matches!(method, Method::Get | Method::Head | Method::Delete) && body.has_body() {
        return Err((400, "request body not allowed"));
    }
    if expect_continue && !body.has_body() {
        return Err((400, "expect without body"));
    }

    Ok(Head {
        method,
        target,
        segs,
        query,
        host,
        authorization,
        range,
        if_none_match,
        if_range,
        continue_pending: expect_continue,
        expect_continue,
        close,
        body,
    })
}

struct SplitCrlf<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for SplitCrlf<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.is_empty() {
            return None;
        }
        match self.rest.windows(2).position(|w| w == b"\r\n") {
            Some(i) => {
                let line = &self.rest[..i];
                self.rest = &self.rest[i + 2..];
                Some(line)
            }
            None => {
                let line = self.rest;
                self.rest = &[];
                Some(line)
            }
        }
    }
}

// ---- range / conditional helpers ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeSpec {
    /// No usable range: serve 200 with the full body (covers absent,
    /// malformed, and multi-range headers, per RFC "may ignore").
    None,
    /// Serve 206 with inclusive byte positions.
    Single { start: u64, end: u64 },
    /// Serve 416 with `Content-Range: bytes */size`.
    Unsatisfiable,
}

pub fn parse_range(header: Option<&str>, size: u64) -> RangeSpec {
    let Some(h) = header else { return RangeSpec::None };
    let Some(spec) = h.strip_prefix("bytes=") else { return RangeSpec::None };
    if spec.contains(',') {
        return RangeSpec::None;
    }
    let spec = spec.trim();
    let Some(dash) = spec.find('-') else { return RangeSpec::None };
    let (a, b) = (&spec[..dash], &spec[dash + 1..]);
    let parse_num = |s: &str| -> Option<u64> {
        if s.is_empty() || s.len() > 19 || !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    if a.is_empty() {
        // suffix form: last n bytes
        let Some(n) = parse_num(b) else { return RangeSpec::None };
        if n == 0 || size == 0 {
            return RangeSpec::Unsatisfiable;
        }
        let start = size.saturating_sub(n);
        return RangeSpec::Single { start, end: size - 1 };
    }
    let Some(start) = parse_num(a) else { return RangeSpec::None };
    if start >= size {
        return RangeSpec::Unsatisfiable;
    }
    if b.is_empty() {
        return RangeSpec::Single { start, end: size - 1 };
    }
    let Some(end) = parse_num(b) else { return RangeSpec::None };
    if end < start {
        return RangeSpec::None;
    }
    RangeSpec::Single { start, end: end.min(size - 1) }
}

/// Strong comparison of an If-None-Match header against our ETag. Weak tags
/// never match; `*` matches anything.
pub fn etag_matches(header: &str, etag: &str) -> bool {
    if header.trim() == "*" {
        return true;
    }
    header.split(',').any(|part| {
        let p = part.trim();
        !p.starts_with("W/") && p == etag
    })
}

/// If-Range: only the exact strong ETag validates; date forms and anything
/// else fail validation (full 200 is served).
pub fn if_range_matches(header: &str, etag: &str) -> bool {
    header.trim() == etag
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_of(raw: &[u8]) -> Result<Head, (u16, &'static str)> {
        let block = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| &raw[..i])
            .unwrap_or(raw);
        parse_head(block)
    }

    #[test]
    fn parses_basic_get() {
        let h = head_of(b"GET /v1/assets?ns=demo&limit=5 HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(h.method, Method::Get);
        assert_eq!(h.segs, vec!["v1".to_string(), "assets".to_string()]);
        assert_eq!(h.query_get("ns"), Some("demo"));
        assert_eq!(h.query_get("limit"), Some("5"));
        assert!(!h.close);
        assert!(!h.body.has_body());
    }

    #[test]
    fn refuses_smuggling_and_bad_te() {
        assert_eq!(
            head_of(b"POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n")
                .unwrap_err()
                .0,
            400
        );
        assert_eq!(
            head_of(b"POST /x HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n")
                .unwrap_err()
                .0,
            501
        );
        assert_eq!(
            head_of(b"POST /x HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: Chunked\r\n\r\n")
                .unwrap_err()
                .0,
            501
        );
    }

    #[test]
    fn refuses_bad_targets() {
        assert_eq!(head_of(b"GET /a%2e%2e/b HTTP/1.1\r\nHost: x\r\n\r\n").unwrap_err().0, 400);
        assert_eq!(head_of(b"GET /a/../b HTTP/1.1\r\nHost: x\r\n\r\n").unwrap_err().0, 400);
        assert_eq!(head_of(b"GET /a//b HTTP/1.1\r\nHost: x\r\n\r\n").unwrap_err().0, 400);
        assert_eq!(head_of(b"GET /a/ HTTP/1.1\r\nHost: x\r\n\r\n").unwrap_err().0, 400);
        assert_eq!(head_of(b"GET /a?x=1&x=2 HTTP/1.1\r\nHost: x\r\n\r\n").unwrap_err().0, 400);
        assert_eq!(head_of(b"GET http://h/a HTTP/1.1\r\nHost: x\r\n\r\n").unwrap_err().0, 400);
        assert_eq!(head_of(b"GET /a b HTTP/1.1\r\nHost: x\r\n\r\n").unwrap_err().0, 400);
    }

    #[test]
    fn refuses_bad_headers_and_versions() {
        assert_eq!(head_of(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n").unwrap_err().0, 505);
        assert_eq!(head_of(b"BREW / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap_err().0, 501);
        assert_eq!(head_of(b"GET / HTTP/1.1\r\n\r\n").unwrap_err().0, 400); // no host
        assert_eq!(
            head_of(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n").unwrap_err().0,
            400
        );
        assert_eq!(
            head_of(b"GET / HTTP/1.1\r\nHost: x\r\n folded\r\n\r\n").unwrap_err().0,
            400
        );
        assert_eq!(
            head_of(b"GET / HTTP/1.1\r\nHost: x\r\nX-Bad: a\x01b\r\n\r\n").unwrap_err().0,
            400
        );
        assert_eq!(
            head_of(b"GET / HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\n").unwrap_err().0,
            400
        );
        assert_eq!(
            head_of(b"POST /x HTTP/1.1\r\nHost: x\r\nContent-Length: 007\r\n\r\n").unwrap_err().0,
            400
        );
    }

    #[test]
    fn range_math() {
        assert_eq!(parse_range(None, 100), RangeSpec::None);
        assert_eq!(parse_range(Some("bytes=0-9"), 100), RangeSpec::Single { start: 0, end: 9 });
        assert_eq!(parse_range(Some("bytes=90-200"), 100), RangeSpec::Single { start: 90, end: 99 });
        assert_eq!(parse_range(Some("bytes=50-"), 100), RangeSpec::Single { start: 50, end: 99 });
        assert_eq!(parse_range(Some("bytes=-10"), 100), RangeSpec::Single { start: 90, end: 99 });
        assert_eq!(parse_range(Some("bytes=-200"), 100), RangeSpec::Single { start: 0, end: 99 });
        assert_eq!(parse_range(Some("bytes=100-"), 100), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), 100), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-0"), 0), RangeSpec::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), RangeSpec::None);
        assert_eq!(parse_range(Some("bytes=a-b"), 100), RangeSpec::None);
        assert_eq!(parse_range(Some("bites=0-1"), 100), RangeSpec::None);
        assert_eq!(parse_range(Some("bytes=5-2"), 100), RangeSpec::None);
    }

    #[test]
    fn etag_comparison() {
        let e = "\"sha256:abcd\"";
        assert!(etag_matches("*", e));
        assert!(etag_matches("\"sha256:abcd\"", e));
        assert!(etag_matches("\"x\", \"sha256:abcd\"", e));
        assert!(!etag_matches("W/\"sha256:abcd\"", e));
        assert!(!etag_matches("\"sha256:ffff\"", e));
        assert!(if_range_matches("\"sha256:abcd\"", e));
        assert!(!if_range_matches("Wed, 21 Oct 2015 07:28:00 GMT", e));
    }
}
