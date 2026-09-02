//! Dependency-free blocking HTTP/1.1 client.
//!
//! One hop only: redirects are never followed. HTTPS is required except for
//! loopback cleartext (test fixtures). Native `SocketStream` TLS is used on
//! non-Windows; Windows talks through synchronous WinHTTP with redirects
//! disabled and a hard cap on raw header allocation.
//!
//! This slice is non-streaming (no SSE). Secrets must never appear in
//! `Error` text; [`Request`] deliberately does not implement `Debug`.

#[cfg(not(target_os = "windows"))]
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(not(target_os = "windows"))]
use std::net::TcpStream;



const USER_AGENT: &str = "makepad-network/1.0";
#[cfg(not(target_os = "windows"))]
const IO_SLICE: Duration = Duration::from_millis(200);

// ----------------------------------------------------------------- public

/// Cooperative cancel flag shared with a blocked request. Flipping it
/// makes the request return [`Error::Cancelled`] at the next I/O slice;
/// the caller never joins a worker.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Hard caps for one request. Every field is a refusal threshold, not a
/// retry hint.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub max_head_bytes: usize,
    pub max_header_count: usize,
    pub max_header_line_bytes: usize,
    pub max_trailer_count: usize,
    pub max_trailer_bytes: usize,
    pub max_body_bytes: usize,
    pub max_chunk_line_bytes: usize,
    pub total_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_head_bytes: 64 * 1024,
            max_header_count: 64,
            max_header_line_bytes: 8 * 1024,
            max_trailer_count: 16,
            max_trailer_bytes: 4 * 1024,
            max_body_bytes: 1024 * 1024,
            max_chunk_line_bytes: 1024,
            total_timeout: Duration::from_secs(60),
        }
    }
}

/// Safe, secret-free failure. Variants never carry tokens, headers, or
/// bodies — only a bounded static classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidUrl,
    InvalidHeader,
    ReservedHeader,
    CleartextForbidden,
    RedirectRefused,
    Cancelled,
    Timeout,
    ResponseTooLarge,
    InvalidResponse,
    UnsupportedTransferEncoding,
    Connect,
    Io,
    Reset,
    Unsupported,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::InvalidUrl => "invalid url",
            Error::InvalidHeader => "invalid header",
            Error::ReservedHeader => "reserved request header",
            Error::CleartextForbidden => "http is only allowed to loopback",
            Error::RedirectRefused => "redirect refused",
            Error::Cancelled => "request cancelled",
            Error::Timeout => "request timed out",
            Error::ResponseTooLarge => "response exceeded a configured limit",
            Error::InvalidResponse => "malformed http response",
            Error::UnsupportedTransferEncoding => "unsupported transfer-encoding",
            Error::Connect => "connect failed",
            Error::Io => "transport i/o failed",
            Error::Reset => "connection reset",
            Error::Unsupported => "http client is not available on this target",
        })
    }
}

impl std::error::Error for Error {}

/// Completed one-hop response. Header names are lowercased.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let want = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == want).map(|(_, v)| v.as_str())
    }
}

/// Request builder. Intentionally not `Debug`: a bearer must not leak
/// through formatting.
pub struct Request {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    limits: Limits,
    cancel: CancelToken,
}

#[derive(Clone, Copy)]
enum Method {
    Get,
    Head,
    Post,
    Put,
    Delete,
    Connect,
    Options,
    Trace,
    Patch,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Connect => "CONNECT",
            Method::Options => "OPTIONS",
            Method::Trace => "TRACE",
            Method::Patch => "PATCH",
        }
    }

    fn is_head(self) -> bool {
        matches!(self, Method::Head)
    }
}

impl Request {
    pub fn get(url: impl Into<String>) -> Request {
        Request::new(Method::Get, url.into())
    }

    pub fn post(url: impl Into<String>) -> Request {
        Request::new(Method::Post, url.into())
    }

    pub fn with_method(url: impl Into<String>, method: crate::types::HttpMethod) -> Request {
        let method = match method {
            crate::types::HttpMethod::GET => Method::Get,
            crate::types::HttpMethod::HEAD => Method::Head,
            crate::types::HttpMethod::POST => Method::Post,
            crate::types::HttpMethod::PUT => Method::Put,
            crate::types::HttpMethod::DELETE => Method::Delete,
            crate::types::HttpMethod::CONNECT => Method::Connect,
            crate::types::HttpMethod::OPTIONS => Method::Options,
            crate::types::HttpMethod::TRACE => Method::Trace,
            crate::types::HttpMethod::PATCH => Method::Patch,
        };
        Request::new(method, url.into())
    }

    fn new(method: Method, url: String) -> Request {
        Request {
            method,
            url,
            headers: Vec::new(),
            body: Vec::new(),
            limits: Limits::default(),
            cancel: CancelToken::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Result<Request, Error> {
        validate_header_pair(name, value)?;
        if is_reserved_header(name) {
            return Err(Error::ReservedHeader);
        }
        if self.headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return Err(Error::InvalidHeader);
        }
        self.headers.push((name.to_string(), value.to_string()));
        Ok(self)
    }

    pub fn bearer(self, token: &str) -> Result<Request, Error> {
        if token.is_empty() {
            return Err(Error::InvalidHeader);
        }
        self.header("Authorization", &format!("Bearer {token}"))
    }

    pub fn json_body(self, bytes: Vec<u8>) -> Result<Request, Error> {
        self.header("Content-Type", "application/json").map(|mut req| {
            req.body = bytes;
            req
        })
    }

    pub fn body(mut self, bytes: Vec<u8>) -> Request {
        self.body = bytes;
        self
    }

    pub fn limits(mut self, limits: Limits) -> Request {
        self.limits = limits;
        self
    }

    pub fn cancel_token(mut self, token: CancelToken) -> Request {
        self.cancel = token;
        self
    }
}

/// POST JSON and refuse any 3xx. Never follows a redirect.
pub fn post_json(req: Request) -> Result<Response, Error> {
    if !matches!(req.method, Method::Post) {
        return Err(Error::InvalidUrl);
    }
    let response = request_no_redirect(req)?;
    if (300..400).contains(&response.status) {
        return Err(Error::RedirectRefused);
    }
    Ok(response)
}

/// Execute exactly one hop. A 3xx status is returned as-is; the Location
/// target is never contacted.
pub fn request_no_redirect(req: Request) -> Result<Response, Error> {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = req;
        return Err(Error::Unsupported);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if req.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let url = parse_url(&req.url)?;
        if !url.https && !cleartext_host_permitted(&url.host) {
            return Err(Error::CleartextForbidden);
        }
        let deadline = Instant::now()
            .checked_add(req.limits.total_timeout)
            .ok_or(Error::Timeout)?;
        #[cfg(target_os = "windows")]
        {
            winhttp_fetch(&req, &url, deadline)
        }
        #[cfg(not(target_os = "windows"))]
        {
            socket_fetch(&req, &url, deadline)
        }
    }
}

// ------------------------------------------------------------------- url

struct ParsedUrl {
    https: bool,
    host: String,
    port: u16,
    target: String,
}

fn parse_url(url: &str) -> Result<ParsedUrl, Error> {
    if url.is_empty() || url.len() > 4096 || url.bytes().any(is_forbidden_url_byte) {
        return Err(Error::InvalidUrl);
    }
    if url.contains('#') || url.contains(' ') {
        return Err(Error::InvalidUrl);
    }
    let (https, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(Error::InvalidUrl);
    };
    if rest.is_empty() {
        return Err(Error::InvalidUrl);
    }
    let (authority, target) = match rest.find('/') {
        Some(pos) => (&rest[..pos], rest[pos..].to_string()),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() || authority.contains('@') || authority.contains('#') {
        return Err(Error::InvalidUrl);
    }
    if target.as_bytes().first() != Some(&b'/')
        || target.bytes().any(is_forbidden_url_byte)
        || target.contains('#')
        || target.contains(' ')
    {
        return Err(Error::InvalidUrl);
    }
    let (host, port) = split_authority(authority, if https { 443 } else { 80 })?;
    if host.is_empty() {
        return Err(Error::InvalidUrl);
    }
    Ok(ParsedUrl { https, host, port, target })
}

fn is_forbidden_url_byte(b: u8) -> bool {
    b < 0x20 || b == 0x7f
}

fn split_authority(authority: &str, default_port: u16) -> Result<(String, u16), Error> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or(Error::InvalidUrl)?;
        let host = rest[..end].to_string();
        if host.is_empty() {
            return Err(Error::InvalidUrl);
        }
        let after = &rest[end + 1..];
        if after.is_empty() {
            return Ok((host, default_port));
        }
        let port = after.strip_prefix(':').ok_or(Error::InvalidUrl)?;
        let port: u16 = port.parse().map_err(|_| Error::InvalidUrl)?;
        return Ok((host, port));
    }
    if authority.matches(':').count() > 1 {
        return Err(Error::InvalidUrl);
    }
    match authority.rfind(':') {
        Some(pos) => {
            let host = authority[..pos].to_string();
            let port: u16 = authority[pos + 1..].parse().map_err(|_| Error::InvalidUrl)?;
            if host.is_empty() || host.contains('[') || host.contains(']') {
                return Err(Error::InvalidUrl);
            }
            Ok((host, port))
        }
        None => {
            if authority.contains('[') || authority.contains(']') {
                return Err(Error::InvalidUrl);
            }
            Ok((authority.to_string(), default_port))
        }
    }
}

fn host_key(host: &str) -> &str {
    host.trim_matches(|c| c == '[' || c == ']')
}

fn is_literal_loopback_host(host: &str) -> bool {
    host_key(host)
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

#[allow(dead_code)]
fn is_localhost_name(host: &str) -> bool {
    host_key(host).eq_ignore_ascii_case("localhost")
}

/// Windows: literal loopback IP only (no unwatched DNS). Other OS: also
/// permit the name `localhost`, which is checked after resolve.
fn cleartext_host_permitted(host: &str) -> bool {
    if is_literal_loopback_host(host) {
        return true;
    }
    #[cfg(target_os = "windows")]
    {
        let _ = host;
        false
    }
    #[cfg(not(target_os = "windows"))]
    {
        is_localhost_name(host)
    }
}

#[allow(dead_code)]
fn cleartext_addrs_are_loopback(addrs: &[std::net::SocketAddr]) -> bool {
    !addrs.is_empty() && addrs.iter().all(|a| a.ip().is_loopback())
}

#[allow(dead_code)]
fn classify_watchdog_failure(kind: u8, cancelled: bool, timed_out: bool) -> Error {
    match kind {
        1 => Error::Cancelled,
        2 => Error::Timeout,
        _ if cancelled => Error::Cancelled,
        _ if timed_out => Error::Timeout,
        _ => Error::Io,
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn host_header(url: &ParsedUrl) -> String {
    let default = if url.https { 443 } else { 80 };
    let host = if url.host.contains(':') {
        format!("[{}]", url.host)
    } else {
        url.host.clone()
    };
    if url.port == default {
        host
    } else {
        format!("{}:{}", host, url.port)
    }
}

// ----------------------------------------------------------- header rules

fn validate_header_pair(name: &str, value: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 256
        || value.len() > 8 * 1024
        || !name.bytes().all(|b| is_token_byte(b))
        || value.bytes().any(is_forbidden_header_byte)
    {
        return Err(Error::InvalidHeader);
    }
    Ok(())
}

fn is_forbidden_header_byte(b: u8) -> bool {
    b < 0x20 || b == 0x7f
}

fn is_token_byte(b: u8) -> bool {
    matches!(b,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
        | b'^' | b'_' | b'`' | b'|' | b'~'
        | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z'
    )
}

fn is_reserved_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "user-agent"
            | "accept-encoding"
            | "accept"
            | "expect"
            | "te"
            | "trailer"
            | "upgrade"
            | "keep-alive"
            | "proxy-connection"
            | "proxy-authorization"
            | "proxy-authenticate"
    ) || n.starts_with("proxy-")
}

fn validate_trailer_line(line: &str) -> Result<(), Error> {
    let (name, value) = split_header_line(line).ok_or(Error::InvalidResponse)?;
    if is_reserved_header(name) {
        return Err(Error::InvalidResponse);
    }
    if value.bytes().any(is_forbidden_header_byte) {
        return Err(Error::InvalidResponse);
    }
    Ok(())
}

fn split_header_line(line: &str) -> Option<(&str, &str)> {
    let pos = line.find(':')?;
    let name = &line[..pos];
    if name.is_empty() || name.bytes().any(|b| b.is_ascii_whitespace()) {
        return None;
    }
    if !name.bytes().all(|b| is_token_byte(b)) {
        return None;
    }
    Some((name, line[pos + 1..].trim()))
}

fn parse_content_length(value: &str) -> Result<u64, Error> {
    let v = value.trim();
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidResponse);
    }
    v.parse::<u64>().map_err(|_| Error::InvalidResponse)
}

struct ValidatedHeaders {
    headers: Vec<(String, String)>,
    content_length: Option<u64>,
    chunked: bool,
}

fn validate_response_headers(
    headers: Vec<(String, String)>,
    limits: &Limits,
) -> Result<ValidatedHeaders, Error> {
    if headers.len() > limits.max_header_count {
        return Err(Error::ResponseTooLarge);
    }
    let mut content_length = None;
    let mut saw_cl = false;
    let mut chunked = false;
    let mut saw_te = false;
    for (name, value) in &headers {
        if name.len().checked_add(value.len()).ok_or(Error::ResponseTooLarge)?
            > limits.max_header_line_bytes
        {
            return Err(Error::ResponseTooLarge);
        }
        match name.as_str() {
            "content-length" => {
                if saw_cl {
                    return Err(Error::InvalidResponse);
                }
                saw_cl = true;
                content_length = Some(parse_content_length(value)?);
            }
            "transfer-encoding" => {
                if saw_te {
                    return Err(Error::InvalidResponse);
                }
                saw_te = true;
                let te = value.trim().to_ascii_lowercase();
                if te != "chunked" {
                    return Err(Error::UnsupportedTransferEncoding);
                }
                chunked = true;
            }
            _ => {}
        }
    }
    if saw_cl && saw_te {
        return Err(Error::InvalidResponse);
    }
    Ok(ValidatedHeaders { headers, content_length, chunked })
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn parse_status_line(line: &str) -> Result<u16, Error> {
    let mut parts = line.split_whitespace();
    let ver = parts.next().ok_or(Error::InvalidResponse)?;
    if ver != "HTTP/1.1" && ver != "HTTP/1.0" {
        return Err(Error::InvalidResponse);
    }
    let code = parts.next().ok_or(Error::InvalidResponse)?;
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Error::InvalidResponse);
    }
    code.parse::<u16>().map_err(|_| Error::InvalidResponse)
}

// ----------------------------------------------------------- socket path

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
enum Transport {
    Plain(TcpStream),
    #[cfg(target_os = "linux")]
    Tls(LinuxTls),
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
    Tls(apple_tls::AppleTls),
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.read(buf),
            #[cfg(any(
                target_os = "linux",
                target_os = "macos",
                target_os = "ios",
                target_os = "tvos"
            ))]
            Transport::Tls(s) => s.read(buf),
        }
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.write(buf),
            #[cfg(any(
                target_os = "linux",
                target_os = "macos",
                target_os = "ios",
                target_os = "tvos"
            ))]
            Transport::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Transport::Plain(s) => s.flush(),
            #[cfg(any(
                target_os = "linux",
                target_os = "macos",
                target_os = "ios",
                target_os = "tvos"
            ))]
            Transport::Tls(s) => s.flush(),
        }
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
impl Transport {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Transport::Plain(s) => s.set_read_timeout(timeout),
            #[cfg(any(
                target_os = "linux",
                target_os = "macos",
                target_os = "ios",
                target_os = "tvos"
            ))]
            Transport::Tls(s) => s.set_read_timeout(timeout),
        }
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        match self {
            Transport::Plain(s) => s.set_write_timeout(timeout),
            #[cfg(any(
                target_os = "linux",
                target_os = "macos",
                target_os = "ios",
                target_os = "tvos"
            ))]
            Transport::Tls(s) => s.set_write_timeout(timeout),
        }
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn socket_fetch(req: &Request, url: &ParsedUrl, deadline: Instant) -> Result<Response, Error> {
    let mut transport = connect(url, &req.cancel, deadline)?;
    write_request(&mut transport, req, url, deadline)?;
    read_response(&mut transport, req, deadline)
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn connect(url: &ParsedUrl, cancel: &CancelToken, deadline: Instant) -> Result<Transport, Error> {
    if url.https {
        #[cfg(target_os = "linux")]
        {
            return linux_tls_connect(url, cancel, deadline);
        }
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
        {
            return apple_tls_connect(url, cancel, deadline);
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "tvos"
        )))]
        {
            let _ = (url, cancel, deadline);
            return Err(Error::Unsupported);
        }
    } else {
        Ok(Transport::Plain(tcp_connect_plain(url, cancel, deadline)?))
    }
}

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
fn linux_tls_connect(
    url: &ParsedUrl,
    cancel: &CancelToken,
    deadline: Instant,
) -> Result<Transport, Error> {
    let tcp = tcp_connect_plain(url, cancel, deadline)?;
    let tls = LinuxTls::handshake(tcp, &url.host, cancel, deadline)?;
    Ok(Transport::Tls(tls))
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
fn apple_tls_connect(
    url: &ParsedUrl,
    cancel: &CancelToken,
    deadline: Instant,
) -> Result<Transport, Error> {
    let tcp = tcp_connect_plain(url, cancel, deadline)?;
    let tls = apple_tls::AppleTls::handshake(tcp, &url.host, cancel, deadline)?;
    Ok(Transport::Tls(tls))
}

/// How many connects — a DNS lookup and a TCP handshake, each on a thread
/// that cannot be killed if the network hangs — may be in the air at once.
/// The cap is here to bound those threads, not to serialise the fetching:
/// one at a time turned a gigabit line into a queue, because every request
/// closes its connection and so every request pays a fresh connect. Raise it
/// with `MAKEPAD_HTTP_MAX_CONNECTS` for a crawl that wants the whole pipe.
#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
const CONNECT_SLOTS_DEFAULT: usize = 64;

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn connect_slots() -> &'static (Mutex<usize>, Condvar) {
    static SLOTS: std::sync::OnceLock<(Mutex<usize>, Condvar)> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| {
        let n = std::env::var("MAKEPAD_HTTP_MAX_CONNECTS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(CONNECT_SLOTS_DEFAULT);
        (Mutex::new(n), Condvar::new())
    })
}

/// One acquired connect permit. The permit goes back exactly once, whether
/// the connect finished or the caller gave up on it first — whoever gets
/// there first wins the swap, and the loser's call does nothing.
#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
#[derive(Clone)]
struct ConnectSlot(Arc<AtomicBool>);


#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn acquire_connect_slot(cancel: &CancelToken, deadline: Instant) -> Result<ConnectSlot, Error> {
    let (lock, cv) = connect_slots();
    let mut free = lock.lock().map_err(|_| Error::Io)?;
    loop {
        check_watch(cancel, deadline)?;
        if *free > 0 {
            *free -= 1;
            return Ok(ConnectSlot(Arc::new(AtomicBool::new(false))));
        }
        // Wake on a returned permit; the slice keeps cancel and deadline live.
        let slice = remaining(deadline)?.min(IO_SLICE);
        free = cv.wait_timeout(free, slice).map_err(|_| Error::Io)?.0;
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn return_connect_slot(slot: &ConnectSlot) {
    if slot.0.swap(true, Ordering::AcqRel) {
        return;
    }
    let (lock, cv) = connect_slots();
    if let Ok(mut free) = lock.lock() {
        *free += 1;
        cv.notify_one();
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn try_connect_addrs(
    addrs: impl IntoIterator<Item = std::net::SocketAddr>,
    deadline: Instant,
) -> Result<TcpStream, Error> {
    let mut saw_any = false;
    for addr in addrs {
        saw_any = true;
        let slice = remaining(deadline)?.min(Duration::from_secs(5));
        match TcpStream::connect_timeout(&addr, slice) {
            Ok(stream) => return Ok(stream),
            Err(_) => continue,
        }
    }
    if saw_any {
        Err(Error::Connect)
    } else {
        Err(Error::Connect)
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn tcp_connect_plain(
    url: &ParsedUrl,
    cancel: &CancelToken,
    deadline: Instant,
) -> Result<TcpStream, Error> {
    let slot = acquire_connect_slot(cancel, deadline)?;
    let host = url.host.clone();
    let port = url.port;
    let https = url.https;
    let worker_deadline = deadline;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let worker_slot = slot.clone();
    match std::thread::Builder::new()
        .name("makepad-http-connect".into())
        .spawn(move || {
            use std::net::ToSocketAddrs;
            let result = (|| {
                let addrs: Vec<std::net::SocketAddr> =
                    (host.as_str(), port).to_socket_addrs().map_err(|_| Error::Connect)?.collect();
                if addrs.is_empty() {
                    return Err(Error::Connect);
                }
                if !https && !cleartext_addrs_are_loopback(&addrs) {
                    return Err(Error::CleartextForbidden);
                }
                try_connect_addrs(addrs, worker_deadline)
            })();
            let _ = tx.send(result);
            return_connect_slot(&worker_slot);
        }) {
        Ok(_) => {}
        Err(_) => {
            return_connect_slot(&slot);
            return Err(Error::Io);
        }
    }
    loop {
        match check_watch(cancel, deadline) {
            Ok(()) => {}
            Err(e) => {
                return_connect_slot(&slot);
                return Err(e);
            }
        }
        let wait = match remaining(deadline) {
            Ok(d) => d.min(IO_SLICE),
            Err(e) => {
                return_connect_slot(&slot);
                return Err(e);
            }
        };
        match rx.recv_timeout(wait) {
            Ok(Ok(stream)) => {
                stream.set_nodelay(true).map_err(|_| Error::Connect)?;
                stream.set_read_timeout(Some(IO_SLICE)).map_err(|_| Error::Io)?;
                stream.set_write_timeout(Some(IO_SLICE)).map_err(|_| Error::Io)?;
                return Ok(stream);
            }
            Ok(Err(e)) => return Err(e),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Connect),
        }
    }
}

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
mod linux_tls {
    use super::{check_watch, remaining, Error, IO_SLICE};
    use std::ffi::{c_char, c_int, c_long, c_ulong, c_void, CString};
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::os::fd::AsRawFd;
    use std::ptr;
    use std::time::Instant;

    const SSL_VERIFY_PEER: c_int = 1;
    const SSL_CTRL_SET_TLSEXT_HOSTNAME: c_int = 55;
    const TLSEXT_NAMETYPE_HOST_NAME: c_long = 0;
    const SSL_ERROR_WANT_READ: c_int = 2;
    const SSL_ERROR_WANT_WRITE: c_int = 3;
    const SSL_ERROR_SYSCALL: c_int = 5;
    const SSL_ERROR_ZERO_RETURN: c_int = 6;
    const OPENSSL_1_1_0: c_ulong = 0x1010_0000;

    #[repr(C)]
    struct SSL_CTX {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct SSL {
        _private: [u8; 0],
    }
    #[repr(C)]
    struct SSL_METHOD {
        _private: [u8; 0],
    }

    #[link(name = "ssl")]
    #[link(name = "crypto")]
    unsafe extern "C" {
        fn OPENSSL_init_ssl(opts: u64, settings: *const c_void) -> c_int;
        fn TLS_client_method() -> *const SSL_METHOD;
        fn SSL_CTX_new(method: *const SSL_METHOD) -> *mut SSL_CTX;
        fn SSL_CTX_free(ctx: *mut SSL_CTX);
        fn SSL_CTX_set_verify(ctx: *mut SSL_CTX, mode: c_int, verify_callback: *mut c_void);
        fn SSL_CTX_set_default_verify_paths(ctx: *mut SSL_CTX) -> c_int;
        fn SSL_new(ctx: *mut SSL_CTX) -> *mut SSL;
        fn SSL_free(ssl: *mut SSL);
        fn SSL_set_fd(ssl: *mut SSL, fd: c_int) -> c_int;
        fn SSL_connect(ssl: *mut SSL) -> c_int;
        fn SSL_get_error(ssl: *mut SSL, ret_code: c_int) -> c_int;
        fn SSL_read(ssl: *mut SSL, buf: *mut c_void, num: c_int) -> c_int;
        fn SSL_write(ssl: *mut SSL, buf: *const c_void, num: c_int) -> c_int;
        fn SSL_shutdown(ssl: *mut SSL) -> c_int;
        fn SSL_ctrl(ssl: *mut SSL, cmd: c_int, larg: c_long, parg: *mut c_void) -> c_long;
        fn X509_VERIFY_PARAM_set1_host(
            param: *mut c_void,
            name: *const c_char,
            namelen: usize,
        ) -> c_int;
        fn X509_VERIFY_PARAM_set1_ip_asc(param: *mut c_void, ipasc: *const c_char) -> c_int;
        fn SSL_get0_param(ssl: *mut SSL) -> *mut c_void;
        fn OpenSSL_version_num() -> c_ulong;
    }

    struct CtxGuard(*mut SSL_CTX);
    impl Drop for CtxGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { SSL_CTX_free(self.0) };
                self.0 = ptr::null_mut();
            }
        }
    }
    struct SslGuard(*mut SSL);
    impl Drop for SslGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { SSL_free(self.0) };
                self.0 = ptr::null_mut();
            }
        }
    }

    pub struct LinuxTls {
        tcp: TcpStream,
        ssl_ctx: *mut SSL_CTX,
        ssl: *mut SSL,
    }

    unsafe impl Send for LinuxTls {}

    impl LinuxTls {
        pub fn handshake(
            tcp: TcpStream,
            host: &str,
            cancel: &super::CancelToken,
            deadline: Instant,
        ) -> Result<LinuxTls, Error> {
            if unsafe { OpenSSL_version_num() } < OPENSSL_1_1_0 {
                return Err(Error::Unsupported);
            }
            if unsafe { OPENSSL_init_ssl(0, ptr::null()) } != 1 {
                return Err(Error::Connect);
            }
            let method = unsafe { TLS_client_method() };
            if method.is_null() {
                return Err(Error::Connect);
            }
            let mut ctx = CtxGuard(unsafe { SSL_CTX_new(method) });
            if ctx.0.is_null() {
                return Err(Error::Connect);
            }
            unsafe {
                SSL_CTX_set_verify(ctx.0, SSL_VERIFY_PEER, ptr::null_mut());
            }
            if unsafe { SSL_CTX_set_default_verify_paths(ctx.0) } != 1 {
                return Err(Error::Connect);
            }
            let mut ssl = SslGuard(unsafe { SSL_new(ctx.0) });
            if ssl.0.is_null() {
                return Err(Error::Connect);
            }
            let host_c = CString::new(host).map_err(|_| Error::InvalidUrl)?;
            let sni = unsafe {
                SSL_ctrl(
                    ssl.0,
                    SSL_CTRL_SET_TLSEXT_HOSTNAME,
                    TLSEXT_NAMETYPE_HOST_NAME,
                    host_c.as_ptr() as *mut c_void,
                )
            };
            if sni == 0 {
                return Err(Error::Connect);
            }
            let param = unsafe { SSL_get0_param(ssl.0) };
            if param.is_null() {
                return Err(Error::Connect);
            }
            let host_ok = if host.parse::<std::net::IpAddr>().is_ok() {
                unsafe { X509_VERIFY_PARAM_set1_ip_asc(param, host_c.as_ptr()) == 1 }
            } else {
                unsafe { X509_VERIFY_PARAM_set1_host(param, host_c.as_ptr(), 0) == 1 }
            };
            if !host_ok {
                return Err(Error::Connect);
            }
            if unsafe { SSL_set_fd(ssl.0, tcp.as_raw_fd()) } != 1 {
                return Err(Error::Connect);
            }
            loop {
                check_watch(cancel, deadline)?;
                let slice = remaining(deadline)?.min(IO_SLICE);
                tcp.set_read_timeout(Some(slice)).map_err(|_| Error::Io)?;
                tcp.set_write_timeout(Some(slice)).map_err(|_| Error::Io)?;
                let ret = unsafe { SSL_connect(ssl.0) };
                if ret == 1 {
                    break;
                }
                let err = unsafe { SSL_get_error(ssl.0, ret) };
                match err {
                    SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => continue,
                    SSL_ERROR_SYSCALL => {
                        let os = std::io::Error::last_os_error();
                        if matches!(
                            os.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::Interrupted
                        ) {
                            continue;
                        }
                        return Err(Error::Connect);
                    }
                    _ => return Err(Error::Connect),
                }
            }
            let out = LinuxTls { tcp, ssl_ctx: ctx.0, ssl: ssl.0 };
            ctx.0 = ptr::null_mut();
            ssl.0 = ptr::null_mut();
            Ok(out)
        }

        pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
            self.tcp.set_read_timeout(timeout)
        }
        pub fn set_write_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
            self.tcp.set_write_timeout(timeout)
        }
    }

    impl Drop for LinuxTls {
        fn drop(&mut self) {
            unsafe {
                let _ = SSL_shutdown(self.ssl);
                SSL_free(self.ssl);
                SSL_CTX_free(self.ssl_ctx);
            }
        }
    }

    impl Read for LinuxTls {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let ret = unsafe {
                SSL_read(
                    self.ssl,
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len().min(c_int::MAX as usize) as c_int,
                )
            };
            if ret > 0 {
                return Ok(ret as usize);
            }
            match unsafe { SSL_get_error(self.ssl, ret) } {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "ssl want read",
                )),
                SSL_ERROR_ZERO_RETURN => Ok(0),
                SSL_ERROR_SYSCALL if ret == 0 => Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "tls truncated",
                )),
                SSL_ERROR_SYSCALL => Err(std::io::Error::last_os_error()),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "ssl read failed",
                )),
            }
        }
    }

    impl Write for LinuxTls {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let ret = unsafe {
                SSL_write(
                    self.ssl,
                    buf.as_ptr() as *const c_void,
                    buf.len().min(c_int::MAX as usize) as c_int,
                )
            };
            if ret > 0 {
                return Ok(ret as usize);
            }
            match unsafe { SSL_get_error(self.ssl, ret) } {
                SSL_ERROR_WANT_READ | SSL_ERROR_WANT_WRITE => Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "ssl want write",
                )),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "ssl write failed",
                )),
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(all(target_os = "linux", not(target_arch = "wasm32")))]
use linux_tls::LinuxTls;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
mod apple_tls {
    use super::{check_watch, remaining, Error, IO_SLICE};
    use makepad_apple_sys::{
        errSSLClosedAbort, errSSLClosedGraceful, errSSLWouldBlock, kSSLClientSide, kSSLStreamType,
        CFRelease, OSStatus, SSLClose, SSLConnectionRef, SSLContextRef, SSLCreateContext,
        SSLHandshake, SSLRead, SSLSetConnection, SSLSetIOFuncs, SSLSetPeerDomainName, SSLWrite,
    };
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::ptr;
    use std::time::Instant;

    const SSL_OK: OSStatus = 0;

    struct AppleIo {
        tcp: TcpStream,
        cancel: super::CancelToken,
        deadline: Instant,
    }

    fn io_watch_failed(io: &AppleIo) -> bool {
        io.cancel.is_cancelled() || Instant::now() >= io.deadline
    }

    unsafe extern "C" fn ssl_read_callback(
        connection: SSLConnectionRef,
        data: *mut std::ffi::c_void,
        data_len: *mut usize,
    ) -> OSStatus {
        if connection.is_null() || data.is_null() || data_len.is_null() {
            return errSSLClosedAbort;
        }
        let io = &mut *(connection as *mut AppleIo);
        let requested = *data_len;
        if requested == 0 {
            return SSL_OK;
        }
        let buffer = std::slice::from_raw_parts_mut(data as *mut u8, requested);
        let mut done = 0usize;
        while done < requested {
            if io_watch_failed(io) {
                *data_len = done;
                return errSSLWouldBlock;
            }
            match io.tcp.read(&mut buffer[done..]) {
                Ok(0) => {
                    *data_len = done;
                    return errSSLClosedGraceful;
                }
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    *data_len = done;
                    return errSSLWouldBlock;
                }
                Err(_) => {
                    *data_len = done;
                    return errSSLClosedAbort;
                }
            }
        }
        *data_len = done;
        SSL_OK
    }

    unsafe extern "C" fn ssl_write_callback(
        connection: SSLConnectionRef,
        data: *const std::ffi::c_void,
        data_len: *mut usize,
    ) -> OSStatus {
        if connection.is_null() || data.is_null() || data_len.is_null() {
            return errSSLClosedAbort;
        }
        let io = &mut *(connection as *mut AppleIo);
        let requested = *data_len;
        if requested == 0 {
            return SSL_OK;
        }
        let buffer = std::slice::from_raw_parts(data as *const u8, requested);
        let mut done = 0usize;
        while done < requested {
            if io_watch_failed(io) {
                *data_len = done;
                return errSSLWouldBlock;
            }
            match io.tcp.write(&buffer[done..]) {
                Ok(0) => {
                    *data_len = done;
                    return errSSLClosedAbort;
                }
                Ok(n) => done += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    *data_len = done;
                    return errSSLWouldBlock;
                }
                Err(_) => {
                    *data_len = done;
                    return errSSLClosedAbort;
                }
            }
        }
        *data_len = done;
        SSL_OK
    }

    struct CtxGuard(SSLContextRef);
    impl Drop for CtxGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = SSLClose(self.0);
                    CFRelease(self.0);
                }
                self.0 = ptr::null_mut();
            }
        }
    }

    pub struct AppleTls {
        io: Box<AppleIo>,
        ctx: SSLContextRef,
    }
    unsafe impl Send for AppleTls {}

    impl AppleTls {
        pub fn handshake(
            tcp: TcpStream,
            host: &str,
            cancel: &super::CancelToken,
            deadline: Instant,
        ) -> Result<AppleTls, Error> {
            let mut io = Box::new(AppleIo { tcp, cancel: cancel.clone(), deadline });
            let raw = unsafe { SSLCreateContext(ptr::null(), kSSLClientSide, kSSLStreamType) };
            if raw.is_null() {
                return Err(Error::Connect);
            }
            let mut guard = CtxGuard(raw);
            let status = unsafe {
                SSLSetIOFuncs(guard.0, Some(ssl_read_callback), Some(ssl_write_callback))
            };
            if status != SSL_OK {
                return Err(Error::Connect);
            }
            let status = unsafe {
                SSLSetConnection(guard.0, io.as_mut() as *mut AppleIo as SSLConnectionRef)
            };
            if status != SSL_OK {
                return Err(Error::Connect);
            }
            let status = unsafe {
                SSLSetPeerDomainName(guard.0, host.as_ptr() as *const std::ffi::c_void, host.len())
            };
            if status != SSL_OK {
                return Err(Error::Connect);
            }
            loop {
                check_watch(cancel, deadline)?;
                let slice = remaining(deadline)?.min(IO_SLICE);
                io.tcp.set_read_timeout(Some(slice)).map_err(|_| Error::Io)?;
                io.tcp.set_write_timeout(Some(slice)).map_err(|_| Error::Io)?;
                let status = unsafe { SSLHandshake(guard.0) };
                if status == SSL_OK {
                    break;
                }
                if status == errSSLWouldBlock {
                    continue;
                }
                return Err(Error::Connect);
            }
            let ctx = guard.0;
            guard.0 = ptr::null_mut();
            Ok(AppleTls { io, ctx })
        }

        pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
            self.io.tcp.set_read_timeout(timeout)
        }
        pub fn set_write_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
            self.io.tcp.set_write_timeout(timeout)
        }
    }

    impl Drop for AppleTls {
        fn drop(&mut self) {
            if !self.ctx.is_null() {
                unsafe {
                    let _ = SSLClose(self.ctx);
                    CFRelease(self.ctx);
                }
                self.ctx = ptr::null_mut();
            }
        }
    }

    impl Read for AppleTls {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let mut processed = 0usize;
            let status = unsafe {
                SSLRead(
                    self.ctx,
                    buf.as_mut_ptr() as *mut std::ffi::c_void,
                    buf.len(),
                    &mut processed,
                )
            };
            map_apple_read(status, processed)
        }
    }

    pub(super) fn map_apple_read(status: OSStatus, processed: usize) -> std::io::Result<usize> {
        match status {
            s if s == SSL_OK => Ok(processed),
            s if s == errSSLWouldBlock && processed > 0 => Ok(processed),
            s if s == errSSLWouldBlock => Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "ssl would block",
            )),
            s if s == errSSLClosedGraceful => Ok(processed),
            s if s == errSSLClosedAbort => Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "ssl abort",
            )),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "ssl read failed",
            )),
        }
    }

    impl Write for AppleTls {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            let mut processed = 0usize;
            let status = unsafe {
                SSLWrite(
                    self.ctx,
                    buf.as_ptr() as *const std::ffi::c_void,
                    buf.len(),
                    &mut processed,
                )
            };
            match status {
                s if s == SSL_OK => Ok(processed),
                s if s == errSSLWouldBlock && processed > 0 => Ok(processed),
                s if s == errSSLWouldBlock => Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "ssl would block",
                )),
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "ssl write failed",
                )),
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn write_request(
    transport: &mut Transport,
    req: &Request,
    url: &ParsedUrl,
    deadline: Instant,
) -> Result<(), Error> {
    let mut head = String::new();
    head.push_str(req.method.as_str());
    head.push(' ');
    head.push_str(&url.target);
    head.push_str(" HTTP/1.1\r\nHost: ");
    head.push_str(&host_header(url));
    head.push_str("\r\nUser-Agent: ");
    head.push_str(USER_AGENT);
    head.push_str("\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n");
    for (name, value) in &req.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("Content-Length: ");
    head.push_str(&req.body.len().to_string());
    head.push_str("\r\n\r\n");
    write_all_watch(transport, head.as_bytes(), &req.cancel, deadline)?;
    if !req.body.is_empty() {
        write_all_watch(transport, &req.body, &req.cancel, deadline)?;
    }
    write_flush_watch(transport, &req.cancel, deadline)
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn read_response(
    transport: &mut Transport,
    req: &Request,
    deadline: Instant,
) -> Result<Response, Error> {
    let mut carry = Vec::new();
    let mut informational = 0u8;
    loop {
        let (status, header_block, prefix) = read_head(transport, req, deadline, carry)?;
        if (100..200).contains(&status) {
            informational = informational.saturating_add(1);
            if informational > 5 {
                return Err(Error::InvalidResponse);
            }
            let raw = parse_header_block(&header_block, &req.limits)?;
            let validated = validate_response_headers(raw, &req.limits)?;
            if validated.content_length.unwrap_or(0) != 0 || validated.chunked {
                return Err(Error::InvalidResponse);
            }
            carry = prefix;
            continue;
        }
        let raw_headers = parse_header_block(&header_block, &req.limits)?;
        let validated = validate_response_headers(raw_headers, &req.limits)?;
        let no_body = req.method.is_head() || status == 204 || status == 304;
        let body = if no_body {
            if !prefix.is_empty() {
                return Err(Error::InvalidResponse);
            }
            Vec::new()
        } else if validated.chunked {
            read_chunked(transport, prefix, req, deadline)?
        } else if let Some(len) = validated.content_length {
            read_sized(transport, prefix, len, req, deadline)?
        } else {
            read_until_close(transport, prefix, req, deadline)?
        };
        return Ok(Response { status, headers: validated.headers, body });
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn read_head(
    transport: &mut Transport,
    req: &Request,
    deadline: Instant,
    carry: Vec<u8>,
) -> Result<(u16, String, Vec<u8>), Error> {
    let mut buf = carry;
    let mut tmp = [0u8; 2048];
    let end = loop {
        if let Some(pos) = find_head_end(&buf) {
            if pos > req.limits.max_head_bytes {
                return Err(Error::ResponseTooLarge);
            }
            break pos;
        }
        if buf.len() > req.limits.max_head_bytes {
            return Err(Error::ResponseTooLarge);
        }
        let n = read_watch(transport, &mut tmp, &req.cancel, deadline)?;
        if n == 0 {
            return Err(Error::InvalidResponse);
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| Error::InvalidResponse)?;
    let prefix = buf[end..].to_vec();
    let status_end = head.find("\r\n").ok_or(Error::InvalidResponse)?;
    if status_end > req.limits.max_header_line_bytes {
        return Err(Error::ResponseTooLarge);
    }
    let status = parse_status_line(&head[..status_end])?;
    Ok((status, head.to_string(), prefix))
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn parse_header_block(head: &str, limits: &Limits) -> Result<Vec<(String, String)>, Error> {
    let mut lines = head.split("\r\n");
    let _status = lines.next().ok_or(Error::InvalidResponse)?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if line.len() > limits.max_header_line_bytes {
            return Err(Error::ResponseTooLarge);
        }
        let (name, value) = split_header_line(line).ok_or(Error::InvalidResponse)?;
        if value.bytes().any(is_forbidden_header_byte) {
            return Err(Error::InvalidResponse);
        }
        headers.push((name.to_ascii_lowercase(), value.to_string()));
        if headers.len() > limits.max_header_count {
            return Err(Error::ResponseTooLarge);
        }
    }
    Ok(headers)
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn read_sized(
    transport: &mut Transport,
    prefix: Vec<u8>,
    len: u64,
    req: &Request,
    deadline: Instant,
) -> Result<Vec<u8>, Error> {
    let want = usize::try_from(len).map_err(|_| Error::ResponseTooLarge)?;
    if want > req.limits.max_body_bytes {
        return Err(Error::ResponseTooLarge);
    }
    let mut body = Vec::with_capacity(want);
    if prefix.len() > want {
        return Err(Error::InvalidResponse);
    }
    body.extend_from_slice(&prefix);
    let mut tmp = [0u8; 8192];
    while body.len() < want {
        let take = (want - body.len()).min(tmp.len());
        let n = read_watch(transport, &mut tmp[..take], &req.cancel, deadline)?;
        if n == 0 {
            return Err(Error::InvalidResponse);
        }
        let new_len = body.len().checked_add(n).ok_or(Error::ResponseTooLarge)?;
        if new_len > req.limits.max_body_bytes {
            return Err(Error::ResponseTooLarge);
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Ok(body)
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn read_until_close(
    transport: &mut Transport,
    prefix: Vec<u8>,
    req: &Request,
    deadline: Instant,
) -> Result<Vec<u8>, Error> {
    let mut body = prefix;
    if body.len() > req.limits.max_body_bytes {
        return Err(Error::ResponseTooLarge);
    }
    let mut tmp = [0u8; 8192];
    loop {
        let read_len = capped_read_len(req.limits.max_body_bytes, body.len(), tmp.len());
        let n = match read_watch(transport, &mut tmp[..read_len], &req.cancel, deadline) {
            Ok(0) => break,
            Ok(n) => n,
            Err(Error::Reset) => return Err(Error::Reset),
            Err(e) => return Err(e),
        };
        let new_len = body.len().checked_add(n).ok_or(Error::ResponseTooLarge)?;
        if new_len > req.limits.max_body_bytes {
            return Err(Error::ResponseTooLarge);
        }
        if req.limits.max_body_bytes != usize::MAX {
            body.reserve_exact(n);
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Ok(body)
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn read_chunked(
    transport: &mut Transport,
    prefix: Vec<u8>,
    req: &Request,
    deadline: Instant,
) -> Result<Vec<u8>, Error> {
    let mut src = ChunkSrc { transport, prefix, pos: 0 };
    let mut body = Vec::new();
    let mut trailer_bytes = 0usize;
    let mut trailer_count = 0usize;
    loop {
        let line = src.read_line(req.limits.max_chunk_line_bytes, &req.cancel, deadline)?;
        let size_part = line.split(';').next().unwrap_or("").trim();
        if size_part.is_empty() {
            return Err(Error::InvalidResponse);
        }
        let size = u64::from_str_radix(size_part, 16).map_err(|_| Error::InvalidResponse)?;
        if size == 0 {
            loop {
                let trailer = src.read_line(req.limits.max_header_line_bytes, &req.cancel, deadline)?;
                if trailer.is_empty() {
                    break;
                }
                trailer_count = trailer_count.checked_add(1).ok_or(Error::ResponseTooLarge)?;
                trailer_bytes =
                    trailer_bytes.checked_add(trailer.len()).ok_or(Error::ResponseTooLarge)?;
                if trailer_count > req.limits.max_trailer_count
                    || trailer_bytes > req.limits.max_trailer_bytes
                {
                    return Err(Error::ResponseTooLarge);
                }
                validate_trailer_line(&trailer)?;
            }
            if src.pos < src.prefix.len() {
                return Err(Error::InvalidResponse);
            }
            expect_eof(src.transport, &req.cancel, deadline)?;
            return Ok(body);
        }
        if size > req.limits.max_body_bytes as u64 {
            return Err(Error::ResponseTooLarge);
        }
        let take = usize::try_from(size).map_err(|_| Error::ResponseTooLarge)?;
        let new_len = body.len().checked_add(take).ok_or(Error::ResponseTooLarge)?;
        if new_len > req.limits.max_body_bytes {
            return Err(Error::ResponseTooLarge);
        }
        let mut chunk = vec![0u8; take];
        src.read_exact(&mut chunk, &req.cancel, deadline)?;
        let mut crlf = [0u8; 2];
        src.read_exact(&mut crlf, &req.cancel, deadline)?;
        if crlf != *b"\r\n" {
            return Err(Error::InvalidResponse);
        }
        if req.limits.max_body_bytes != usize::MAX {
            body.reserve_exact(take);
        }
        body.extend_from_slice(&chunk);
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
struct ChunkSrc<'a> {
    transport: &'a mut Transport,
    prefix: Vec<u8>,
    pos: usize,
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
impl ChunkSrc<'_> {
    fn read_byte(&mut self, cancel: &CancelToken, deadline: Instant) -> Result<u8, Error> {
        if self.pos < self.prefix.len() {
            let b = self.prefix[self.pos];
            self.pos += 1;
            return Ok(b);
        }
        let mut one = [0u8; 1];
        let n = read_watch(self.transport, &mut one, cancel, deadline)?;
        if n == 0 {
            return Err(Error::InvalidResponse);
        }
        Ok(one[0])
    }

    fn read_exact(&mut self, buf: &mut [u8], cancel: &CancelToken, deadline: Instant) -> Result<(), Error> {
        let mut filled = 0usize;
        if self.pos < self.prefix.len() {
            let take = (self.prefix.len() - self.pos).min(buf.len());
            buf[..take].copy_from_slice(&self.prefix[self.pos..self.pos + take]);
            self.pos += take;
            filled = take;
        }
        while filled < buf.len() {
            let n = read_watch(self.transport, &mut buf[filled..], cancel, deadline)?;
            if n == 0 {
                return Err(Error::InvalidResponse);
            }
            filled = filled.checked_add(n).ok_or(Error::ResponseTooLarge)?;
        }
        Ok(())
    }

    fn read_line(
        &mut self,
        max: usize,
        cancel: &CancelToken,
        deadline: Instant,
    ) -> Result<String, Error> {
        let mut line = Vec::new();
        loop {
            let b = self.read_byte(cancel, deadline)?;
            if b == b'\n' {
                if line.last() == Some(&b'\r') {
                    line.pop();
                    return String::from_utf8(line).map_err(|_| Error::InvalidResponse);
                }
                return Err(Error::InvalidResponse);
            }
            line.push(b);
            if line.len() > max {
                return Err(Error::ResponseTooLarge);
            }
        }
    }
}

// ----------------------------------------------------------- watch / i/o

fn check_watch(cancel: &CancelToken, deadline: Instant) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(Error::Timeout);
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn capped_read_len(max_body: usize, received: usize, buffer_len: usize) -> usize {
    max_body
        .saturating_sub(received)
        .saturating_add(1)
        .min(buffer_len)
        .max(1)
}

fn remaining(deadline: Instant) -> Result<Duration, Error> {
    deadline.checked_duration_since(Instant::now()).ok_or(Error::Timeout)
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn expect_eof(
    transport: &mut Transport,
    cancel: &CancelToken,
    deadline: Instant,
) -> Result<(), Error> {
    let mut tmp = [0u8; 32];
    match read_watch(transport, &mut tmp, cancel, deadline) {
        Ok(0) => Ok(()),
        Ok(_) => Err(Error::InvalidResponse),
        Err(Error::Reset) => Err(Error::Reset),
        Err(e) => Err(e),
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn read_watch(
    transport: &mut Transport,
    buf: &mut [u8],
    cancel: &CancelToken,
    deadline: Instant,
) -> Result<usize, Error> {
    loop {
        check_watch(cancel, deadline)?;
        let slice = remaining(deadline)?.min(IO_SLICE);
        transport.set_read_timeout(Some(slice)).map_err(|_| Error::Io)?;
        match transport.read(buf) {
            Ok(n) => return Ok(n),
            Err(e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::ConnectionReset
                    || e.kind() == std::io::ErrorKind::BrokenPipe =>
            {
                return Err(Error::Reset);
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::InvalidResponse);
            }
            Err(_) => return Err(Error::Io),
        }
    }
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn write_all_watch(
    transport: &mut Transport,
    mut buf: &[u8],
    cancel: &CancelToken,
    deadline: Instant,
) -> Result<(), Error> {
    while !buf.is_empty() {
        check_watch(cancel, deadline)?;
        let slice = remaining(deadline)?.min(IO_SLICE);
        transport.set_write_timeout(Some(slice)).map_err(|_| Error::Io)?;
        match transport.write(buf) {
            Ok(0) => return Err(Error::Io),
            Ok(n) => buf = &buf[n..],
            Err(e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(_) => return Err(Error::Io),
        }
    }
    Ok(())
}

#[cfg(all(not(target_os = "windows"), not(target_arch = "wasm32")))]
fn write_flush_watch(
    transport: &mut Transport,
    cancel: &CancelToken,
    deadline: Instant,
) -> Result<(), Error> {
    loop {
        check_watch(cancel, deadline)?;
        let slice = remaining(deadline)?.min(IO_SLICE);
        transport.set_write_timeout(Some(slice)).map_err(|_| Error::Io)?;
        match transport.flush() {
            Ok(()) => return Ok(()),
            Err(e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(_) => return Err(Error::Io),
        }
    }
}

// -------------------------------------------------------------- WinHTTP

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WINHTTP_OPTION_DISABLE_FEATURE: u32 = 63;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WINHTTP_DISABLE_REDIRECTS: u32 = 0x0000_0002;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WINHTTP_QUERY_RAW_HEADERS_CRLF: u32 = 22;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WINHTTP_QUERY_FLAG_TRAILERS: u32 = 0x0200_0000;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const ERROR_WINHTTP_HEADER_NOT_FOUND: u32 = 12150;

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
#[link(name = "winhttp")]
unsafe extern "system" {
    fn WinHttpOpen(
        user_agent: *const u16,
        access_type: u32,
        proxy_name: *const u16,
        proxy_bypass: *const u16,
        flags: u32,
    ) -> *mut std::ffi::c_void;
    fn WinHttpConnect(
        session: *mut std::ffi::c_void,
        server_name: *const u16,
        server_port: u16,
        reserved: u32,
    ) -> *mut std::ffi::c_void;
    fn WinHttpOpenRequest(
        connect: *mut std::ffi::c_void,
        verb: *const u16,
        object_name: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept_types: *const *const u16,
        flags: u32,
    ) -> *mut std::ffi::c_void;
    fn WinHttpSetTimeouts(
        handle: *mut std::ffi::c_void,
        resolve_timeout: i32,
        connect_timeout: i32,
        send_timeout: i32,
        receive_timeout: i32,
    ) -> i32;
    fn WinHttpSetOption(
        handle: *mut std::ffi::c_void,
        option: u32,
        buffer: *mut std::ffi::c_void,
        buffer_len: u32,
    ) -> i32;
    fn WinHttpSendRequest(
        request: *mut std::ffi::c_void,
        headers: *const u16,
        headers_len: u32,
        optional: *mut std::ffi::c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> i32;
    fn WinHttpReceiveResponse(
        request: *mut std::ffi::c_void,
        reserved: *mut std::ffi::c_void,
    ) -> i32;
    fn WinHttpQueryHeaders(
        request: *mut std::ffi::c_void,
        info_level: u32,
        name: *const u16,
        buffer: *mut std::ffi::c_void,
        buffer_len: *mut u32,
        index: *mut u32,
    ) -> i32;
    fn WinHttpReadData(
        request: *mut std::ffi::c_void,
        buffer: *mut std::ffi::c_void,
        bytes_to_read: u32,
        bytes_read: *mut u32,
    ) -> i32;
    fn WinHttpCloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
struct OnceHandle {
    ptr: std::sync::atomic::AtomicPtr<std::ffi::c_void>,
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
impl OnceHandle {
    fn new(ptr: *mut std::ffi::c_void) -> OnceHandle {
        OnceHandle { ptr: std::sync::atomic::AtomicPtr::new(ptr) }
    }
    fn load(&self) -> *mut std::ffi::c_void {
        self.ptr.load(Ordering::Acquire)
    }
    fn close_once(&self) -> bool {
        let p = self.ptr.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if p.is_null() {
            false
        } else {
            unsafe {
                let _ = WinHttpCloseHandle(p);
            }
            true
        }
    }
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
impl Drop for OnceHandle {
    fn drop(&mut self) {
        self.close_once();
    }
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WATCH_NONE: u8 = 0;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WATCH_CANCEL: u8 = 1;
#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
const WATCH_TIMEOUT: u8 = 2;

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
struct SharedRequest {
    ptr: std::sync::atomic::AtomicPtr<std::ffi::c_void>,
    kind: std::sync::atomic::AtomicU8,
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
impl SharedRequest {
    fn new(ptr: *mut std::ffi::c_void) -> Arc<SharedRequest> {
        Arc::new(SharedRequest {
            ptr: std::sync::atomic::AtomicPtr::new(ptr),
            kind: std::sync::atomic::AtomicU8::new(WATCH_NONE),
        })
    }
    fn load(&self) -> *mut std::ffi::c_void {
        self.ptr.load(Ordering::Acquire)
    }
    fn close(&self, kind: u8) {
        let p = self.ptr.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !p.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(p);
            }
        }
        let _ = self.kind.compare_exchange(
            WATCH_NONE,
            kind,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    fn classify(&self, cancel: &CancelToken, deadline: Instant) -> Error {
        classify_watchdog_failure(
            self.kind.load(Ordering::Acquire),
            cancel.is_cancelled(),
            Instant::now() >= deadline,
        )
    }
    fn invoke<T>(
        &self,
        cancel: &CancelToken,
        deadline: Instant,
        f: impl FnOnce(*mut std::ffi::c_void) -> T,
    ) -> Result<T, Error> {
        let p = self.load();
        if p.is_null() {
            return Err(self.classify(cancel, deadline));
        }
        let out = f(p);
        if self.load().is_null() {
            return Err(self.classify(cancel, deadline));
        }
        Ok(out)
    }
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
impl Drop for SharedRequest {
    fn drop(&mut self) {
        self.close(WATCH_NONE);
    }
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
fn timeout_ms(deadline: Instant) -> Result<i32, Error> {
    let ms = remaining(deadline)?.as_millis().min(i32::MAX as u128) as i32;
    if ms <= 0 {
        Err(Error::Timeout)
    } else {
        Ok(ms)
    }
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
fn winhttp_fetch(req: &Request, url: &ParsedUrl, deadline: Instant) -> Result<Response, Error> {
    check_watch(&req.cancel, deadline)?;
    // Windows cleartext: only literal loopback IPs. No sync DNS before the
    // watchdog covers the hop — `localhost` must be spelled as 127.0.0.1/::1.
    if !url.https && !is_literal_loopback_host(&url.host) {
        return Err(Error::CleartextForbidden);
    }
    let user_agent = wide_null(USER_AGENT);
    let session = unsafe {
        WinHttpOpen(
            user_agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    if session.is_null() {
        return Err(Error::Connect);
    }
    let session = OnceHandle::new(session);
    let ms = timeout_ms(deadline)?;
    if unsafe { WinHttpSetTimeouts(session.load(), ms, ms, ms, ms) } == 0 {
        return Err(Error::Connect);
    }
    let host = wide_null(&url.host);
    let connect = unsafe { WinHttpConnect(session.load(), host.as_ptr(), url.port, 0) };
    if connect.is_null() {
        return Err(Error::Connect);
    }
    let connect = OnceHandle::new(connect);
    check_watch(&req.cancel, deadline)?;
    let verb = wide_null(req.method.as_str());
    let target = wide_null(&url.target);
    let request = unsafe {
        WinHttpOpenRequest(
            connect.load(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            if url.https { WINHTTP_FLAG_SECURE } else { 0 },
        )
    };
    if request.is_null() {
        return Err(Error::Connect);
    }
    let request = SharedRequest::new(request);
    let mut disabled = WINHTTP_DISABLE_REDIRECTS;
    if unsafe {
        WinHttpSetOption(
            request.load(),
            WINHTTP_OPTION_DISABLE_FEATURE,
            (&mut disabled as *mut u32).cast::<std::ffi::c_void>(),
            std::mem::size_of::<u32>() as u32,
        )
    } == 0
    {
        return Err(Error::Io);
    }
    let mut header_block = String::from("Accept: */*\r\nAccept-Encoding: identity\r\n");
    for (name, value) in &req.headers {
        header_block.push_str(name);
        header_block.push_str(": ");
        header_block.push_str(value);
        header_block.push_str("\r\n");
    }
    let headers = wide_null(&header_block);
    let body_len = u32::try_from(req.body.len()).map_err(|_| Error::InvalidHeader)?;
    let body_ptr = if req.body.is_empty() {
        std::ptr::null_mut()
    } else {
        req.body.as_ptr() as *mut std::ffi::c_void
    };
    check_watch(&req.cancel, deadline)?;
    let watchdog = WinHttpWatchdog::start(request.clone(), req.cancel.clone(), deadline)?;
    let fail = || request.classify(&req.cancel, deadline);
    let ok = request.invoke(&req.cancel, deadline, |p| unsafe {
        WinHttpSendRequest(p, headers.as_ptr(), u32::MAX, body_ptr, body_len, body_len, 0)
    })?;
    if ok == 0 {
        return Err(fail());
    }
    let ok = request.invoke(&req.cancel, deadline, |p| unsafe {
        WinHttpReceiveResponse(p, std::ptr::null_mut())
    })?;
    if ok == 0 {
        return Err(fail());
    }
    let mut status = 0u32;
    let mut status_len = std::mem::size_of::<u32>() as u32;
    let ok = request.invoke(&req.cancel, deadline, |p| unsafe {
        WinHttpQueryHeaders(
            p,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            (&mut status as *mut u32).cast::<std::ffi::c_void>(),
            &mut status_len,
            std::ptr::null_mut(),
        )
    })?;
    if ok == 0 {
        return Err(fail());
    }
    let status = u16::try_from(status).map_err(|_| Error::InvalidResponse)?;
    if (100..200).contains(&status) {
        return Err(Error::InvalidResponse);
    }
    let raw = winhttp_raw_headers(&request, &req.cancel, deadline, req.limits.max_head_bytes)?;
    let parsed = parse_header_block(&raw, &req.limits)?;
    let ValidatedHeaders { headers, content_length, chunked } =
        validate_response_headers(parsed, &req.limits)?;
    let no_body = req.method.is_head() || status == 204 || status == 304;
    if !no_body && content_length.is_some_and(|len| len > req.limits.max_body_bytes as u64) {
        return Err(Error::ResponseTooLarge);
    }
    let body = if no_body {
        Vec::new()
    } else {
        let body =
            winhttp_read_body(&request, &req.cancel, deadline, req.limits.max_body_bytes)?;
        if request.load().is_null() {
            return Err(fail());
        }
        if let Some(len) = content_length {
            if body.len() as u64 != len {
                return Err(Error::InvalidResponse);
            }
        }
        if chunked {
            winhttp_validate_trailers(&request, &req.cancel, deadline, &req.limits)?;
        }
        body
    };
    drop(watchdog);
    drop(request);
    drop(connect);
    drop(session);
    Ok(Response { status, headers, body })
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
fn winhttp_validate_trailers(
    request: &SharedRequest,
    cancel: &CancelToken,
    deadline: Instant,
    limits: &Limits,
) -> Result<(), Error> {
    let mut byte_len = 0u32;
    let first = request.invoke(cancel, deadline, |p| unsafe {
        WinHttpQueryHeaders(
            p,
            WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_TRAILERS,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut byte_len,
            std::ptr::null_mut(),
        )
    })?;
    if first == 0 {
        if request.load().is_null() {
            return Err(request.classify(cancel, deadline));
        }
        let code = unsafe { GetLastError() };
        if code == ERROR_WINHTTP_HEADER_NOT_FOUND || byte_len == 0 {
            return Ok(());
        }
        if code != ERROR_INSUFFICIENT_BUFFER {
            return Err(Error::InvalidResponse);
        }
    }
    if byte_len as usize > limits.max_trailer_bytes {
        return Err(Error::ResponseTooLarge);
    }
    if byte_len < 2 {
        return Ok(());
    }
    let units = ((byte_len as usize) / 2)
        .checked_add(1)
        .ok_or(Error::ResponseTooLarge)?;
    let mut wide = vec![0u16; units];
    let ok = request.invoke(cancel, deadline, |p| unsafe {
        WinHttpQueryHeaders(
            p,
            WINHTTP_QUERY_RAW_HEADERS_CRLF | WINHTTP_QUERY_FLAG_TRAILERS,
            std::ptr::null(),
            wide.as_mut_ptr().cast::<std::ffi::c_void>(),
            &mut byte_len,
            std::ptr::null_mut(),
        )
    })?;
    if ok == 0 {
        return Err(request.classify(cancel, deadline));
    }
    let mut used = byte_len as usize / 2;
    while used > 0 && wide[used - 1] == 0 {
        used -= 1;
    }
    let raw = String::from_utf16_lossy(&wide[..used]);
    let mut count = 0usize;
    let mut bytes = 0usize;
    for line in raw.split("\r\n") {
        if line.is_empty() || line.starts_with("HTTP/") {
            continue;
        }
        count = count.checked_add(1).ok_or(Error::ResponseTooLarge)?;
        bytes = bytes.checked_add(line.len()).ok_or(Error::ResponseTooLarge)?;
        if count > limits.max_trailer_count || bytes > limits.max_trailer_bytes {
            return Err(Error::ResponseTooLarge);
        }
        validate_trailer_line(line)?;
    }
    Ok(())
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
struct WinHttpWatchdog {
    stop: std::sync::Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
impl WinHttpWatchdog {
    fn start(
        request: Arc<SharedRequest>,
        cancel: CancelToken,
        deadline: Instant,
    ) -> Result<WinHttpWatchdog, Error> {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let join = std::thread::Builder::new()
            .name("makepad-winhttp-watch".into())
            .spawn(move || {
                while !stop2.load(Ordering::Acquire) {
                    if cancel.is_cancelled() {
                        request.close(WATCH_CANCEL);
                        return;
                    }
                    if Instant::now() >= deadline {
                        request.close(WATCH_TIMEOUT);
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            })
            .map_err(|_| Error::Io)?;
        Ok(WinHttpWatchdog { stop, join: Some(join) })
    }
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
impl Drop for WinHttpWatchdog {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
fn winhttp_raw_headers(
    request: &SharedRequest,
    cancel: &CancelToken,
    deadline: Instant,
    max_head_bytes: usize,
) -> Result<String, Error> {
    let mut byte_len = 0u32;
    let first = request.invoke(cancel, deadline, |p| unsafe {
        WinHttpQueryHeaders(
            p,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut byte_len,
            std::ptr::null_mut(),
        )
    })?;
    if first == 0 {
        if request.load().is_null() {
            return Err(request.classify(cancel, deadline));
        }
        let code = unsafe { GetLastError() };
        if code != ERROR_INSUFFICIENT_BUFFER {
            return Err(Error::InvalidResponse);
        }
    }
    if byte_len < 2 {
        return Err(Error::InvalidResponse);
    }
    if byte_len as usize > max_head_bytes {
        return Err(Error::ResponseTooLarge);
    }
    let units = ((byte_len as usize) / 2)
        .checked_add(1)
        .ok_or(Error::ResponseTooLarge)?;
    if units.saturating_mul(2) > max_head_bytes.saturating_add(2) {
        return Err(Error::ResponseTooLarge);
    }
    let mut wide = vec![0u16; units];
    let ok = request.invoke(cancel, deadline, |p| unsafe {
        WinHttpQueryHeaders(
            p,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            std::ptr::null(),
            wide.as_mut_ptr().cast::<std::ffi::c_void>(),
            &mut byte_len,
            std::ptr::null_mut(),
        )
    })?;
    if ok == 0 {
        return Err(request.classify(cancel, deadline));
    }
    let mut used = byte_len as usize / 2;
    while used > 0 && wide[used - 1] == 0 {
        used -= 1;
    }
    Ok(String::from_utf16_lossy(&wide[..used]))
}

#[cfg(all(target_os = "windows", not(target_arch = "wasm32")))]
fn winhttp_read_body(
    request: &SharedRequest,
    cancel: &CancelToken,
    deadline: Instant,
    max_body: usize,
) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        check_watch(cancel, deadline)?;
        let mut read = 0u32;
        let read_len = capped_read_len(max_body, body.len(), buf.len());
        let ok = request.invoke(cancel, deadline, |p| unsafe {
            WinHttpReadData(
                p,
                buf.as_mut_ptr().cast::<std::ffi::c_void>(),
                read_len as u32,
                &mut read,
            )
        })?;
        if ok == 0 {
            return Err(request.classify(cancel, deadline));
        }
        if read == 0 {
            return Ok(body);
        }
        let n = read as usize;
        let new_len = body.len().checked_add(n).ok_or(Error::ResponseTooLarge)?;
        if new_len > max_body {
            return Err(Error::ResponseTooLarge);
        }
        if max_body != usize::MAX {
            body.reserve_exact(n);
        }
        body.extend_from_slice(&buf[..n]);
    }
}

#[cfg(test)]
mod once_close {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::atomic::AtomicPtr;

    #[test]
    fn second_swap_is_null() {
        let p = AtomicPtr::new(0x1 as *mut u8);
        let first = p.swap(std::ptr::null_mut(), Ordering::AcqRel);
        let second = p.swap(std::ptr::null_mut(), Ordering::AcqRel);
        assert!(!first.is_null());
        assert!(second.is_null());
    }

    #[test]
    fn cleartext_requires_resolved_loopback_addrs() {
        assert!(is_literal_loopback_host("127.0.0.1"));
        assert!(is_literal_loopback_host("127.0.0.2"));
        assert!(is_literal_loopback_host("::1"));
        assert!(!is_literal_loopback_host("8.8.8.8"));
        assert!(is_localhost_name("localhost"));
        assert!(!is_literal_loopback_host("localhost"));
        assert!(cleartext_host_permitted("127.0.0.1"));
        #[cfg(target_os = "windows")]
        assert!(!cleartext_host_permitted("localhost"));
        #[cfg(not(target_os = "windows"))]
        assert!(cleartext_host_permitted("localhost"));
        let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 80);
        let public = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 80);
        assert!(cleartext_addrs_are_loopback(&[loopback]));
        assert!(!cleartext_addrs_are_loopback(&[public]));
        assert!(!cleartext_addrs_are_loopback(&[loopback, public]));
        assert!(!cleartext_addrs_are_loopback(&[]));
    }

    #[test]
    fn watchdog_failures_are_cancelled_or_timeout() {
        assert_eq!(classify_watchdog_failure(1, false, false), Error::Cancelled);
        assert_eq!(classify_watchdog_failure(2, false, false), Error::Timeout);
        assert_eq!(classify_watchdog_failure(0, true, false), Error::Cancelled);
        assert_eq!(classify_watchdog_failure(0, false, true), Error::Timeout);
        assert_eq!(classify_watchdog_failure(0, false, false), Error::Io);
    }
}

#[cfg(all(test, not(target_os = "windows"), not(target_arch = "wasm32")))]
mod connect_addr_tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    #[test]
    fn try_connect_addrs_uses_later_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ok = listener.local_addr().unwrap();
        let refused: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let stream = try_connect_addrs([refused, ok], deadline).expect("second addr");
        drop(stream);
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios", target_os = "tvos")))]
mod apple_read_tests {
    use super::apple_tls::map_apple_read;
    use makepad_apple_sys::errSSLClosedGraceful;

    #[test]
    fn graceful_close_keeps_processed_bytes() {
        let n = map_apple_read(errSSLClosedGraceful, 7).expect("bytes");
        assert_eq!(n, 7);
        let eof = map_apple_read(errSSLClosedGraceful, 0).expect("eof");
        assert_eq!(eof, 0);
    }
}
