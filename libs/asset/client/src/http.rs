//! Bounded HTTP/1.1 *client* transport, written for hostile peers.
//!
//! This is the only module that touches a TCP socket for HTTP. Hard rules,
//! all fail-closed:
//!
//! - Body framing is always an exact `Content-Length`, never read-to-EOF and
//!   never chunked. A response with `Transfer-Encoding` is refused outright.
//! - Connections are REUSED when a pool is supplied ([`ConnPool`]): the
//!   request says `Connection: keep-alive`, and the socket goes back to the
//!   pool only when the response was framed exactly, its body was consumed
//!   to the last declared byte, and neither side asked to close. Anything
//!   else drops the socket — a connection whose framing we are not certain
//!   of is never reused. Without a pool the old behaviour stands: one
//!   request per connection, `Connection: close`.
//! - A pooled socket the server has since closed shows up as EOF before any
//!   response byte; that exact case (and only that case) retries once on a
//!   fresh connection, because the server provably processed nothing.
//! - The response head is budgeted: total head bytes, line bytes, header
//!   count, status-line shape. `HTTP/1.1` only.
//! - Redirects are never followed; 3xx is a protocol refusal. 1xx and
//!   204/304 are refused too — no route of this client can legally produce
//!   them, so accepting one would mean improvising framing.
//! - Duplicate tracked headers are refused (response smuggling hygiene).
//! - Request targets are validated against the server's accepted charset
//!   before anything is sent, so a bad target fails locally, not as a 400.
//! - Everything runs under a wall-clock deadline on top of socket timeouts
//!   (slow-drip defense). Body errors distinguish RETRYABLE transport
//!   failures (`Io`/`Timeout`, resume may continue) from protocol violations
//!   (never retried).

use crate::error::{io_err, ClientError, ClientResult};
use crate::wire::{target_byte_ok, MAX_TARGET_BYTES};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Idle keep-alive sockets, keyed by peer. One pool belongs to ONE worker:
/// [`crate::api::Api`] gives every clone its own, so two lanes never
/// interleave requests on one socket.
///
/// Bounded on purpose — a pool is a latency optimisation, not a resource to
/// accumulate. Sockets past the cap are dropped rather than kept.
pub struct ConnPool {
    idle: Mutex<Vec<(SocketAddr, TcpStream)>>,
    max_idle: usize,
}

impl Default for ConnPool {
    fn default() -> Self {
        ConnPool::new(4)
    }
}

impl std::fmt::Debug for ConnPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnPool").finish_non_exhaustive()
    }
}

impl ConnPool {
    pub fn new(max_idle: usize) -> ConnPool {
        ConnPool { idle: Mutex::new(Vec::new()), max_idle: max_idle.max(1) }
    }

    fn take(&self, addr: SocketAddr) -> Option<TcpStream> {
        let mut idle = self.idle.lock().ok()?;
        let i = idle.iter().position(|(a, _)| *a == addr)?;
        Some(idle.swap_remove(i).1)
    }

    fn put(&self, addr: SocketAddr, stream: TcpStream) {
        let Ok(mut idle) = self.idle.lock() else { return };
        if idle.len() >= self.max_idle {
            return;
        }
        idle.push((addr, stream));
    }

    /// Idle sockets currently parked (diagnostics and tests).
    pub fn idle_len(&self) -> usize {
        self.idle.lock().map(|i| i.len()).unwrap_or(0)
    }
}

pub const MAX_HEAD_BYTES: usize = 32 * 1024;
pub const MAX_HEADER_LINE: usize = 8 * 1024;
pub const MAX_HEADERS: usize = 64;
/// Longest header value this client retains (ETag, Content-Type…).
pub const MAX_KEPT_VALUE_BYTES: usize = 256;

/// Socket and deadline knobs. All milliseconds; all must be non-zero.
#[derive(Clone, Copy, Debug)]
pub struct HttpLimits {
    pub connect_timeout_ms: u64,
    /// One socket read/write call.
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
    /// Whole response head, wall clock.
    pub head_deadline_ms: u64,
    /// Whole response body, wall clock. Large blob transfers get their own
    /// per-attempt deadline from the caller via [`Request::body_deadline_ms`].
    pub body_deadline_ms: u64,
}

impl HttpLimits {
    pub fn default_v1() -> Self {
        Self {
            connect_timeout_ms: 5_000,
            read_timeout_ms: 10_000,
            write_timeout_ms: 10_000,
            head_deadline_ms: 10_000,
            body_deadline_ms: 60_000,
        }
    }

    pub fn validate(&self) -> ClientResult<()> {
        if self.connect_timeout_ms == 0
            || self.read_timeout_ms == 0
            || self.write_timeout_ms == 0
            || self.head_deadline_ms == 0
            || self.body_deadline_ms == 0
        {
            return Err(ClientError::InvalidInput { what: "http limits" });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }

    fn takes_body(self) -> bool {
        matches!(self, Method::Post | Method::Put)
    }
}

/// One outgoing request. `target` must already be charset-legal (built via
/// [`crate::wire`] path helpers); this module re-checks and refuses locally.
pub struct Request<'a> {
    pub method: Method,
    pub target: &'a str,
    /// Full bearer token (`mpat_…`); shape-validated by the caller.
    pub bearer: Option<&'a str>,
    /// Open-ended resume range `bytes=<start>-`.
    pub range_start: Option<u64>,
    /// Exact strong ETag to gate the range on, sent as `If-Range: "<v>"`.
    pub if_range: Option<&'a str>,
    /// Request body (POST/PUT only).
    pub body: Option<&'a [u8]>,
    /// Content-Type sent with a body.
    pub body_content_type: &'a str,
    /// Overrides `HttpLimits::body_deadline_ms` when set (long blob pulls).
    pub body_deadline_ms: Option<u64>,
    /// Overrides `HttpLimits::head_deadline_ms` when set. Long-poll routes
    /// legitimately hold the response head back for the whole server-side
    /// wait, so their head budget is wait + margin, not the default.
    pub head_deadline_ms: Option<u64>,
    /// Accept a bodyless `204 No Content` success. Opt-in per request: only
    /// write routes that legitimately answer 204 set it; every read route
    /// keeps refusing 204 outright (framing there would be improvised).
    pub allow_no_content: bool,
}

impl<'a> Request<'a> {
    pub fn get(target: &'a str) -> Self {
        Self {
            method: Method::Get,
            target,
            bearer: None,
            range_start: None,
            if_range: None,
            body: None,
            body_content_type: "application/json",
            body_deadline_ms: None,
            head_deadline_ms: None,
            allow_no_content: false,
        }
    }

    pub fn head(target: &'a str) -> Self {
        Self { method: Method::Head, ..Self::get(target) }
    }

    pub fn post(target: &'a str, body: &'a [u8]) -> Self {
        Self { method: Method::Post, body: Some(body), ..Self::get(target) }
    }

    pub fn put(target: &'a str, body: &'a [u8]) -> Self {
        Self { method: Method::Put, body: Some(body), ..Self::get(target) }
    }

    pub fn delete(target: &'a str) -> Self {
        Self { method: Method::Delete, ..Self::get(target) }
    }
}

/// Inclusive byte positions of a 206 response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct ResponseHead {
    pub status: u16,
    /// Declared body length. Always present on accepted responses; for HEAD
    /// it describes the body a GET would carry.
    pub content_length: u64,
    pub content_type: Option<String>,
    pub content_range: Option<ContentRange>,
    /// ETag value with its surrounding quotes stripped.
    pub etag: Option<String>,
}

/// An accepted response with its body still on the socket. Exactly
/// `content_length` bytes will be read; the connection is then dropped.
pub struct Response<'a> {
    /// `None` once the socket has been handed back to the pool.
    stream: Option<TcpStream>,
    head: ResponseHead,
    /// Body bytes over-read while parsing the head.
    buffered: Vec<u8>,
    pos: usize,
    remaining: u64,
    deadline: Instant,
    read_timeout: Duration,
    /// Where a fully consumed, cleanly framed socket goes back to.
    pool: Option<&'a ConnPool>,
    addr: SocketAddr,
    /// Cleared by any framing doubt: a protocol error, a body error, an
    /// early drop, or `Connection: close` from either side.
    reusable: bool,
}

impl Response<'_> {
    pub fn head(&self) -> &ResponseHead {
        &self.head
    }

    /// Hand a fully consumed socket back to the pool. Called when the body
    /// ends and on drop; doing it here (not at the call site) means every
    /// early exit — abort, error, `?` — simply keeps the socket out of the
    /// pool, which is the safe direction.
    fn recycle(&mut self) {
        if !self.reusable || self.remaining != 0 || self.buffered.len() != self.pos {
            return;
        }
        let (Some(pool), Some(stream)) = (self.pool, self.stream.take()) else {
            return;
        };
        pool.put(self.addr, stream);
    }

    fn stream_mut(&mut self) -> ClientResult<&mut TcpStream> {
        self.stream.as_mut().ok_or(ClientError::Protocol { what: "response socket gone" })
    }

    /// Read up to `out.len()` body bytes. `Ok(0)` exactly when the body is
    /// complete. `Io`/`Timeout` errors are retryable transport failures (the
    /// bytes read so far are still valid); anything else is final.
    pub fn read_chunk(&mut self, out: &mut [u8]) -> ClientResult<usize> {
        if self.remaining == 0 {
            self.recycle();
            return Ok(0);
        }
        if out.is_empty() {
            return Err(ClientError::InvalidInput { what: "empty read buffer" });
        }
        let buffered = self.buffered.len() - self.pos;
        if buffered > 0 {
            let n = (buffered as u64).min(self.remaining).min(out.len() as u64) as usize;
            out[..n].copy_from_slice(&self.buffered[self.pos..self.pos + n]);
            self.pos += n;
            if self.pos == self.buffered.len() {
                self.buffered.clear();
                self.pos = 0;
            }
            self.remaining -= n as u64;
            if self.remaining == 0 {
                self.recycle();
            }
            return Ok(n);
        }
        loop {
            let now = Instant::now();
            if now >= self.deadline {
                self.reusable = false;
                return Err(ClientError::Timeout { op: "http body" });
            }
            let timeout = (self.deadline - now).min(self.read_timeout).max(Duration::from_millis(1));
            let remaining = self.remaining;
            let want = (out.len() as u64).min(remaining) as usize;
            let stream = self.stream_mut()?;
            stream
                .set_read_timeout(Some(timeout))
                .map_err(io_err("http set body timeout"))?;
            match stream.read(&mut out[..want]) {
                Ok(0) => {
                    // Peer closed with body bytes still owed: a truncated
                    // transfer, retryable via Range resume.
                    self.reusable = false;
                    return Err(ClientError::Io {
                        op: "http body",
                        kind: std::io::ErrorKind::UnexpectedEof,
                    });
                }
                Ok(n) => {
                    self.remaining -= n as u64;
                    if self.remaining == 0 {
                        self.recycle();
                    }
                    return Ok(n);
                }
                Err(e) => match e.kind() {
                    std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted => continue,
                    kind => {
                        self.reusable = false;
                        return Err(ClientError::Io { op: "http body", kind });
                    }
                },
            }
        }
    }

    /// Buffer the whole body. The caller must have checked
    /// `head.content_length` against its route budget first; this re-checks
    /// against `max` as defense in depth.
    pub fn read_full(mut self, max: u64) -> ClientResult<Vec<u8>> {
        if self.head.content_length > max {
            return Err(ClientError::OverBudget {
                what: "http response body",
                limit: max,
                found: self.head.content_length,
            });
        }
        let mut out = Vec::with_capacity(self.head.content_length.min(64 * 1024) as usize);
        let mut chunk = [0u8; 16 * 1024];
        loop {
            let n = self.read_chunk(&mut chunk)?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&chunk[..n]);
        }
    }
}

impl Drop for Response<'_> {
    fn drop(&mut self) {
        // A body that was not read to its last byte leaves the socket at an
        // unknown offset: never pooled, just closed.
        self.recycle();
    }
}

/// Issue one request and parse the response head. On success the body (if
/// any) is still unread inside the returned [`Response`]. One connection per
/// call; see [`http_call_pooled`] for keep-alive.
pub fn http_call<'a>(
    addr: SocketAddr,
    req: &Request<'_>,
    limits: &HttpLimits,
) -> ClientResult<Response<'a>> {
    http_call_pooled(addr, req, limits, None)
}

/// As [`http_call`], reusing (and returning) a keep-alive socket from `pool`.
///
/// A socket taken from the pool may have been closed by the server while it
/// sat idle. That failure is invisible until the read, and it is provably
/// harmless — the server processed nothing — so it is retried once on a
/// fresh connection. Any other failure is reported as-is.
pub fn http_call_pooled<'a>(
    addr: SocketAddr,
    req: &Request<'_>,
    limits: &HttpLimits,
    pool: Option<&'a ConnPool>,
) -> ClientResult<Response<'a>> {
    match call_once(addr, req, limits, pool, true) {
        Err(HttpAttemptError::StaleIdle) => call_once(addr, req, limits, pool, false)
            .map_err(|e| e.into_client_error()),
        other => other.map_err(|e| e.into_client_error()),
    }
}

/// Failure of one attempt. `StaleIdle` is the only retryable shape: a pooled
/// socket that died before the server said anything.
enum HttpAttemptError {
    StaleIdle,
    Client(ClientError),
}

impl HttpAttemptError {
    fn into_client_error(self) -> ClientError {
        match self {
            HttpAttemptError::Client(e) => e,
            HttpAttemptError::StaleIdle => {
                ClientError::Io { op: "http response head", kind: std::io::ErrorKind::UnexpectedEof }
            }
        }
    }
}

impl From<ClientError> for HttpAttemptError {
    fn from(e: ClientError) -> Self {
        HttpAttemptError::Client(e)
    }
}

fn call_once<'a>(
    addr: SocketAddr,
    req: &Request<'_>,
    limits: &HttpLimits,
    pool: Option<&'a ConnPool>,
    allow_pooled: bool,
) -> Result<Response<'a>, HttpAttemptError> {
    limits.validate()?;
    check_target(req.target)?;
    if req.body.is_some() && !req.method.takes_body() {
        return Err(ClientError::InvalidInput { what: "body on bodyless method" }.into());
    }

    let pooled = match (pool, allow_pooled) {
        (Some(p), true) => p.take(addr),
        _ => None,
    };
    let from_pool = pooled.is_some();
    let stream = match pooled {
        Some(s) => s,
        None => {
            let s = TcpStream::connect_timeout(
                &addr,
                Duration::from_millis(limits.connect_timeout_ms.max(1)),
            )
            .map_err(io_err("http connect"))?;
            s.set_nodelay(true).map_err(io_err("http nodelay"))?;
            // Set once, here, so the common request path does not have to
            // touch the socket options of a socket it has not written to
            // yet — and so a pooled socket that has gone bad announces it
            // through the setup below rather than mid-conversation.
            s.set_write_timeout(Some(Duration::from_millis(limits.write_timeout_ms.max(1))))
                .map_err(io_err("http set write timeout"))?;
            s
        }
    };
    // The whole SETUP phase — socket options and the request write — happens
    // before a single byte reaches the server, so a failure here on a POOLED
    // socket is provably unprocessed: the socket died while it was idle (the
    // server closed it, or the fd was invalidated) and the request can be
    // replayed on a fresh connection exactly once. Only the same failure on
    // a FRESH connection is a real error the caller must see. Before this,
    // a dead pooled socket surfaced as "io failure during http set write
    // timeout" the moment a user changed a filter.
    if from_pool {
        if let Err(e) = stream
            .set_write_timeout(Some(Duration::from_millis(limits.write_timeout_ms.max(1))))
        {
            let _ = e;
            return Err(HttpAttemptError::StaleIdle);
        }
    }

    // ---- request bytes ----
    let mut out = Vec::with_capacity(256 + req.body.map_or(0, <[u8]>::len));
    out.extend_from_slice(req.method.as_str().as_bytes());
    out.push(b' ');
    out.extend_from_slice(req.target.as_bytes());
    out.extend_from_slice(b" HTTP/1.1\r\n");
    out.extend_from_slice(format!("Host: {addr}\r\n").as_bytes());
    if let Some(token) = req.bearer {
        out.extend_from_slice(b"Authorization: Bearer ");
        out.extend_from_slice(token.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if let Some(start) = req.range_start {
        out.extend_from_slice(format!("Range: bytes={start}-\r\n").as_bytes());
    }
    if let Some(tag) = req.if_range {
        out.extend_from_slice(format!("If-Range: \"{tag}\"\r\n").as_bytes());
    }
    if let Some(body) = req.body {
        out.extend_from_slice(format!("Content-Type: {}\r\n", req.body_content_type).as_bytes());
        out.extend_from_slice(format!("Content-Length: {}\r\n", body.len()).as_bytes());
    }
    out.extend_from_slice(if pool.is_some() {
        b"Connection: keep-alive\r\n\r\n" as &[u8]
    } else {
        b"Connection: close\r\n\r\n"
    });
    if let Some(body) = req.body {
        out.extend_from_slice(body);
    }
    let mut stream = stream;
    if let Err(e) = stream.write_all(&out) {
        if from_pool {
            // The server closed this idle socket; nothing was processed.
            return Err(HttpAttemptError::StaleIdle);
        }
        return Err(io_err("http write request")(e).into());
    }

    // ---- response head ----
    let head_ms = req.head_deadline_ms.unwrap_or(limits.head_deadline_ms).max(1);
    let head_deadline = Instant::now() + Duration::from_millis(head_ms);
    let read_timeout = Duration::from_millis(limits.read_timeout_ms.max(1));
    let mut buf: Vec<u8> = Vec::with_capacity(4 * 1024);
    let head_end = loop {
        if let Some(idx) = find_terminator(&buf) {
            break idx;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(ClientError::Protocol { what: "response head too large" }.into());
        }
        let now = Instant::now();
        if now >= head_deadline {
            return Err(ClientError::Timeout { op: "http response head" }.into());
        }
        let timeout = (head_deadline - now).min(read_timeout).max(Duration::from_millis(1));
        if let Err(e) = stream.set_read_timeout(Some(timeout)) {
            // Same rule as an EOF here: on a pooled socket that has said
            // nothing yet, the socket is the problem, not the request.
            if from_pool && buf.is_empty() {
                return Err(HttpAttemptError::StaleIdle);
            }
            return Err(io_err("http set head timeout")(e).into());
        }
        let len = buf.len();
        buf.resize(len + 8 * 1024, 0);
        match stream.read(&mut buf[len..]) {
            Ok(0) => {
                buf.truncate(len);
                if from_pool && buf.is_empty() {
                    // Idle socket the server had already closed: retry once
                    // on a fresh connection, because nothing was processed.
                    return Err(HttpAttemptError::StaleIdle);
                }
                return Err(ClientError::Io {
                    op: "http response head",
                    kind: std::io::ErrorKind::UnexpectedEof,
                }
                .into());
            }
            Ok(n) => buf.truncate(len + n),
            Err(e) => {
                buf.truncate(len);
                match e.kind() {
                    std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted => continue,
                    kind => {
                        if from_pool && buf.is_empty() {
                            return Err(HttpAttemptError::StaleIdle);
                        }
                        return Err(ClientError::Io { op: "http response head", kind }.into());
                    }
                }
            }
        }
    };

    let (head, peer_keeps_alive) = parse_response_head_conn(
        &buf[..head_end],
        req.method == Method::Head,
        req.allow_no_content,
    )?;
    let body_ms = req.body_deadline_ms.unwrap_or(limits.body_deadline_ms).max(1);
    let remaining = if req.method == Method::Head { 0 } else { head.content_length };
    let buffered = buf.split_off(head_end + 4);
    // Over-read past the declared body is a framing violation.
    if buffered.len() as u64 > remaining {
        return Err(ClientError::Protocol { what: "bytes past declared body" }.into());
    }
    let mut resp = Response {
        stream: Some(stream),
        head,
        buffered,
        pos: 0,
        remaining,
        deadline: Instant::now() + Duration::from_millis(body_ms),
        read_timeout,
        pool,
        addr,
        reusable: pool.is_some() && peer_keeps_alive,
    };
    // A bodyless response is already complete: pool it now rather than on a
    // read the caller may never make.
    if resp.remaining == 0 {
        resp.recycle();
    }
    Ok(resp)
}

fn check_target(target: &str) -> ClientResult<()> {
    if target.is_empty() || !target.starts_with('/') {
        return Err(ClientError::InvalidInput { what: "target must be origin-form" });
    }
    if target.len() > MAX_TARGET_BYTES {
        return Err(ClientError::InvalidInput { what: "target too long" });
    }
    if !target.bytes().all(target_byte_ok) {
        return Err(ClientError::InvalidInput { what: "target charset" });
    }
    Ok(())
}

fn find_terminator(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
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

/// Parse one complete response head (bytes up to, excluding, `\r\n\r\n`).
/// Pure function so hostile-input tests can drive it directly.
/// `allow_no_content` admits a bodyless 204 (write routes only).
pub fn parse_response_head(
    block: &[u8],
    is_head_request: bool,
    allow_no_content: bool,
) -> ClientResult<ResponseHead> {
    parse_response_head_conn(block, is_head_request, allow_no_content).map(|(head, _)| head)
}

/// As [`parse_response_head`], also reporting whether the peer is willing to
/// keep the connection alive. `Connection: close` (or anything that is not
/// exactly `keep-alive`) means this socket is single-use.
pub fn parse_response_head_conn(
    block: &[u8],
    is_head_request: bool,
    allow_no_content: bool,
) -> ClientResult<(ResponseHead, bool)> {
    let mut lines = SplitCrlf { rest: block };
    let status_line = lines.next().ok_or(ClientError::Protocol { what: "empty response" })?;

    // ---- status line: `HTTP/1.1 NNN[ reason]` ----
    let rest = status_line
        .strip_prefix(b"HTTP/1.1 ")
        .ok_or(ClientError::Protocol { what: "http/1.1 response required" })?;
    if rest.len() < 3 || !rest[..3].iter().all(u8::is_ascii_digit) {
        return Err(ClientError::Protocol { what: "malformed status" });
    }
    match rest.get(3) {
        None | Some(b' ') => {}
        Some(_) => return Err(ClientError::Protocol { what: "malformed status" }),
    }
    let status: u16 = (rest[0] - b'0') as u16 * 100
        + (rest[1] - b'0') as u16 * 10
        + (rest[2] - b'0') as u16;
    if status < 100 {
        return Err(ClientError::Protocol { what: "malformed status" });
    }
    if status < 200 {
        return Err(ClientError::Protocol { what: "unexpected 1xx" });
    }
    if status == 204 && !allow_no_content {
        // No READ route of this client can produce a 204; accepting one
        // would require improvising body framing.
        return Err(ClientError::Protocol { what: "unexpected bodyless status" });
    }
    if status == 304 {
        return Err(ClientError::Protocol { what: "unexpected bodyless status" });
    }
    if (300..400).contains(&status) {
        return Err(ClientError::Protocol { what: "redirect refused" });
    }

    // ---- headers ----
    let mut content_length: Option<String> = None;
    let mut transfer_encoding: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut content_range: Option<String> = None;
    let mut etag: Option<String> = None;
    let mut connection: Option<String> = None;
    let mut count = 0usize;
    for line in lines {
        count += 1;
        if count > MAX_HEADERS {
            return Err(ClientError::Protocol { what: "too many headers" });
        }
        if line.len() > MAX_HEADER_LINE {
            return Err(ClientError::Protocol { what: "header line too large" });
        }
        if line.first() == Some(&b' ') || line.first() == Some(&b'\t') {
            return Err(ClientError::Protocol { what: "obsolete line folding" });
        }
        let colon = line
            .iter()
            .position(|&b| b == b':')
            .ok_or(ClientError::Protocol { what: "malformed header" })?;
        let name_b = &line[..colon];
        if name_b.is_empty() || name_b.len() > 128 || !name_b.iter().all(|&b| token_byte_ok(b)) {
            return Err(ClientError::Protocol { what: "malformed header name" });
        }
        let mut value_b = &line[colon + 1..];
        while value_b.first() == Some(&b' ') || value_b.first() == Some(&b'\t') {
            value_b = &value_b[1..];
        }
        while value_b.last() == Some(&b' ') || value_b.last() == Some(&b'\t') {
            value_b = &value_b[..value_b.len() - 1];
        }
        if !value_b.iter().all(|&b| value_byte_ok(b)) {
            return Err(ClientError::Protocol { what: "header value charset" });
        }
        let name = std::str::from_utf8(name_b)
            .map_err(|_| ClientError::Protocol { what: "malformed header name" })?
            .to_ascii_lowercase();
        let slot = match name.as_str() {
            "content-length" => &mut content_length,
            "transfer-encoding" => &mut transfer_encoding,
            "content-type" => &mut content_type,
            "content-range" => &mut content_range,
            "connection" => &mut connection,
            "etag" => &mut etag,
            _ => continue,
        };
        if slot.is_some() {
            return Err(ClientError::Protocol { what: "duplicate header" });
        }
        if value_b.len() > MAX_KEPT_VALUE_BYTES {
            return Err(ClientError::Protocol { what: "tracked header value too large" });
        }
        *slot = Some(
            std::str::from_utf8(value_b)
                .map_err(|_| ClientError::Protocol { what: "header value charset" })?
                .to_string(),
        );
    }

    if transfer_encoding.is_some() {
        return Err(ClientError::Protocol { what: "transfer-encoding refused" });
    }
    let content_length = match &content_length {
        // 204 is bodyless BY DEFINITION; a Content-Length header may be
        // absent, but a non-zero one contradicts the status.
        None if status == 204 && allow_no_content => 0,
        None => return Err(ClientError::Protocol { what: "content-length required" }),
        Some(cl) => parse_content_length(cl)?,
    };
    if status == 204 && content_length != 0 {
        return Err(ClientError::Protocol { what: "204 with body" });
    }

    let content_range = match (&content_range, status) {
        (None, 206) => return Err(ClientError::Protocol { what: "206 without content-range" }),
        (None, _) => None,
        (Some(cr), 206) => {
            let parsed = parse_content_range(cr)
                .ok_or(ClientError::Protocol { what: "malformed content-range" })?;
            // A 206 body is exactly the range it declares.
            if parsed.end - parsed.start + 1 != content_length {
                return Err(ClientError::Protocol { what: "content-range disagrees with length" });
            }
            Some(parsed)
        }
        (Some(_), 416) => None, // `bytes */size` shape; caller only needs the status
        (Some(_), _) => {
            return Err(ClientError::Protocol { what: "content-range outside 206" });
        }
    };

    let etag = match etag {
        None => None,
        Some(raw) => {
            let t = raw.trim();
            let inner = t
                .strip_prefix('"')
                .and_then(|x| x.strip_suffix('"'))
                .ok_or(ClientError::Protocol { what: "malformed etag" })?;
            if inner.is_empty() || inner.contains('"') {
                return Err(ClientError::Protocol { what: "malformed etag" });
            }
            Some(inner.to_string())
        }
    };

    if is_head_request && status == 206 {
        return Err(ClientError::Protocol { what: "206 on head" });
    }

    // Keep-alive is claimed only on an explicit `keep-alive`: HTTP/1.1's
    // default is persistent, but a client that guesses wrong here corrupts
    // its NEXT request, so the conservative reading is the only safe one.
    let keep_alive = matches!(
        connection.as_deref().map(str::trim),
        Some(v) if v.eq_ignore_ascii_case("keep-alive")
    );
    Ok((ResponseHead { status, content_length, content_type, content_range, etag }, keep_alive))
}

fn parse_content_length(cl: &str) -> ClientResult<u64> {
    let b = cl.as_bytes();
    if b.is_empty() || b.len() > 20 || !b.iter().all(u8::is_ascii_digit) {
        return Err(ClientError::Protocol { what: "malformed content-length" });
    }
    if b.len() > 1 && b[0] == b'0' {
        return Err(ClientError::Protocol { what: "malformed content-length" });
    }
    cl.parse().map_err(|_| ClientError::Protocol { what: "malformed content-length" })
}

/// `bytes <start>-<end>/<size>` with strict digits and consistent math.
fn parse_content_range(v: &str) -> Option<ContentRange> {
    let rest = v.strip_prefix("bytes ")?;
    let (range, size_s) = rest.split_once('/')?;
    let (start_s, end_s) = range.split_once('-')?;
    let num = |s: &str| -> Option<u64> {
        if s.is_empty() || s.len() > 19 || !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    let start = num(start_s)?;
    let end = num(end_s)?;
    let size = num(size_s)?;
    if end < start || end >= size {
        return None;
    }
    Some(ContentRange { start, end, size })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn head_of(raw: &[u8]) -> ClientResult<ResponseHead> {
        parse_response_head(raw, false, false)
    }

    // ---- keep-alive pool recovery -----------------------------------------

    /// A one-request-at-a-time HTTP/1.1 server for the pool tests: it speaks
    /// keep-alive, and `close_idle` makes it hang up on the socket it is
    /// holding, exactly like a real server whose idle timeout expired.
    struct MiniServer {
        addr: SocketAddr,
        served: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        join: Option<std::thread::JoinHandle<()>>,
    }

    impl MiniServer {
        fn start() -> MiniServer {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (count, halt) = (served.clone(), stop.clone());
            let join = std::thread::spawn(move || {
                listener.set_nonblocking(false).ok();
                for conn in listener.incoming() {
                    if halt.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    let Ok(mut conn) = conn else { return };
                    conn.set_read_timeout(Some(Duration::from_millis(250))).ok();
                    loop {
                        let mut buf = Vec::new();
                        let mut byte = [0u8; 1];
                        let head = loop {
                            match conn.read(&mut byte) {
                                Ok(0) => break false,
                                Ok(_) => {
                                    buf.push(byte[0]);
                                    if buf.ends_with(b"\r\n\r\n") {
                                        break true;
                                    }
                                }
                                Err(_) => break false,
                            }
                        };
                        if !head {
                            break;
                        }
                        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let body = b"ok";
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                            body.len()
                        );
                        if conn.write_all(resp.as_bytes()).is_err()
                            || conn.write_all(body).is_err()
                        {
                            break;
                        }
                    }
                }
            });
            MiniServer { addr, served, stop, join: Some(join) }
        }

        fn served(&self) -> usize {
            self.served.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl Drop for MiniServer {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = std::net::TcpStream::connect(self.addr);
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    fn get(addr: SocketAddr, pool: &ConnPool) -> ClientResult<Vec<u8>> {
        let req = Request::get("/x");
        let resp = http_call_pooled(addr, &req, &HttpLimits::default_v1(), Some(pool))?;
        resp.read_full(1024)
    }

    /// The keep-alive socket the pool is holding dies while it is idle —
    /// the server hung up, or the fd went bad. Nothing of the next request
    /// has reached the server, so it is replayed once on a fresh connection
    /// and the caller never sees the failure.
    #[test]
    fn a_dead_pooled_socket_is_replaced_and_the_request_still_succeeds() {
        let server = MiniServer::start();
        let pool = ConnPool::new(4);
        assert_eq!(get(server.addr, &pool).unwrap(), b"ok");
        assert_eq!(pool.idle_len(), 1, "the socket was kept for reuse");
        assert_eq!(get(server.addr, &pool).unwrap(), b"ok", "reuse works");

        // Kill the pooled socket the way an idle timeout does: take it out,
        // shut it down, and put it back for the next request to trip over.
        let dead = pool.take(server.addr).expect("a pooled socket");
        dead.shutdown(std::net::Shutdown::Both).ok();
        pool.put(server.addr, dead);
        assert_eq!(pool.idle_len(), 1);
        let before = server.served();
        assert_eq!(
            get(server.addr, &pool).unwrap(),
            b"ok",
            "the dead socket is retried on a fresh connection, transparently"
        );
        assert_eq!(server.served(), before + 1, "the request ran exactly once");
    }

    /// The exact failure a user hit changing a Library filter: the pooled
    /// socket's fd is no longer a socket, so `set_write_timeout` fails
    /// before a single byte is written. That is provably unprocessed, so it
    /// must be replayed rather than reported as "io failure during http set
    /// write timeout".
    #[cfg(unix)]
    #[test]
    fn a_pooled_fd_that_is_not_a_socket_is_replaced_not_reported() {
        use std::os::fd::{FromRawFd, IntoRawFd};

        let server = MiniServer::start();
        let pool = ConnPool::new(4);
        assert_eq!(get(server.addr, &pool).unwrap(), b"ok");
        drop(pool.take(server.addr).expect("a pooled socket"));

        // A file descriptor that is NOT a socket: every socket option on it
        // fails with ENOTSOCK, which is what an invalidated/recycled fd
        // looks like from here. Ownership moves cleanly into the TcpStream,
        // which closes it on drop — no double close.
        let path = std::env::temp_dir().join(format!("mp-pool-notasocket-{}", std::process::id()));
        std::fs::write(&path, b"not a socket").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let not_a_socket = unsafe { TcpStream::from_raw_fd(file.into_raw_fd()) };
        assert!(
            not_a_socket.set_write_timeout(Some(Duration::from_millis(10))).is_err(),
            "the fixture must actually fail the option this test is about"
        );
        pool.put(server.addr, not_a_socket);

        let before = server.served();
        assert_eq!(
            get(server.addr, &pool).unwrap(),
            b"ok",
            "a bad pooled fd is dropped and the request replayed fresh"
        );
        assert_eq!(server.served(), before + 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_basic_response() {
        let h = head_of(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 12\r\n",
        )
        .unwrap();
        assert_eq!(h.status, 200);
        assert_eq!(h.content_length, 12);
        assert_eq!(h.content_type.as_deref(), Some("application/json"));
        assert!(h.content_range.is_none());
    }

    #[test]
    fn refuses_bad_status_lines() {
        assert!(head_of(b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n").is_err());
        assert!(head_of(b"HTTP/2 200 OK\r\nContent-Length: 0\r\n").is_err());
        assert!(head_of(b"garbage\r\nContent-Length: 0\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 20 OK\r\nContent-Length: 0\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 2000 OK\r\nContent-Length: 0\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 099 X\r\nContent-Length: 0\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 100 Continue\r\n").is_err());
    }

    #[test]
    fn refuses_redirects_and_bodyless() {
        for status in ["301", "302", "303", "307", "308", "204", "304"] {
            let raw = format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n");
            assert!(head_of(raw.as_bytes()).is_err(), "{status}");
        }
    }

    #[test]
    fn no_content_is_opt_in_and_strictly_bodyless() {
        // Write routes opt in: bodyless 204 accepted, with or without CL: 0.
        let h = parse_response_head(b"HTTP/1.1 204 No Content\r\n", false, true).unwrap();
        assert_eq!((h.status, h.content_length), (204, 0));
        let h = parse_response_head(b"HTTP/1.1 204 X\r\nContent-Length: 0\r\n", false, true)
            .unwrap();
        assert_eq!((h.status, h.content_length), (204, 0));
        // A 204 that claims a body is a framing lie.
        assert!(
            parse_response_head(b"HTTP/1.1 204 X\r\nContent-Length: 5\r\n", false, true).is_err()
        );
        // 304 stays refused even under the opt-in.
        assert!(
            parse_response_head(b"HTTP/1.1 304 X\r\nContent-Length: 0\r\n", false, true).is_err()
        );
    }

    #[test]
    fn refuses_framing_hostility() {
        // Chunked framing from the server is never accepted.
        assert!(head_of(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n").is_err());
        assert!(head_of(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n"
        )
        .is_err());
        // Missing / malformed / duplicate Content-Length.
        assert!(head_of(b"HTTP/1.1 200 OK\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 200 OK\r\nContent-Length: 007\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 200 OK\r\nContent-Length: 1e3\r\n").is_err());
        assert!(head_of(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 5\r\n"
        )
        .is_err());
        // Folding and control bytes.
        assert!(head_of(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n folded\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-Bad: a\x01b\r\n").is_err());
    }

    #[test]
    fn content_range_math() {
        let h = head_of(
            b"HTTP/1.1 206 Partial Content\r\nContent-Length: 10\r\nContent-Range: bytes 5-14/100\r\n",
        )
        .unwrap();
        assert_eq!(h.content_range, Some(ContentRange { start: 5, end: 14, size: 100 }));
        // 206 without a range header.
        assert!(head_of(b"HTTP/1.1 206 P\r\nContent-Length: 10\r\n").is_err());
        // Length disagreeing with the range.
        assert!(head_of(
            b"HTTP/1.1 206 P\r\nContent-Length: 9\r\nContent-Range: bytes 5-14/100\r\n"
        )
        .is_err());
        // Inverted / overflowing ranges.
        assert!(head_of(
            b"HTTP/1.1 206 P\r\nContent-Length: 1\r\nContent-Range: bytes 5-4/100\r\n"
        )
        .is_err());
        assert!(head_of(
            b"HTTP/1.1 206 P\r\nContent-Length: 1\r\nContent-Range: bytes 100-100/100\r\n"
        )
        .is_err());
        // A range header outside 206/416 is refused.
        assert!(head_of(
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nContent-Range: bytes 0-9/10\r\n"
        )
        .is_err());
    }

    #[test]
    fn etag_shapes() {
        let h = head_of(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"sha256:abcd\"\r\n")
            .unwrap();
        assert_eq!(h.etag.as_deref(), Some("sha256:abcd"));
        assert!(head_of(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: sha256:abcd\r\n").is_err());
        assert!(head_of(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"\"\r\n").is_err());
    }

    #[test]
    fn header_budgets() {
        let mut raw = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n".to_vec();
        for i in 0..MAX_HEADERS {
            raw.extend_from_slice(format!("X-F{i}: 1\r\n").as_bytes());
        }
        assert!(head_of(&raw).is_err());
        let long_line = format!("HTTP/1.1 200 OK\r\nX-L: {}\r\nContent-Length: 0\r\n", "a".repeat(MAX_HEADER_LINE));
        assert!(head_of(long_line.as_bytes()).is_err());
        let long_etag =
            format!("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nETag: \"{}\"\r\n", "a".repeat(300));
        assert!(head_of(long_etag.as_bytes()).is_err());
    }

    #[test]
    fn target_charset_guard() {
        assert!(check_target("/v1/health").is_ok());
        assert!(check_target("v1/health").is_err());
        assert!(check_target("/a b").is_err());
        // '%' is outside the accepted set: percent-encoding fails locally.
        assert!(check_target("/a%20b").is_err());
    }
}
