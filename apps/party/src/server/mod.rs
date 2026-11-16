//! The party's HTTPS front: the one service the box shows to guests' phones,
//! and possibly to the internet through a router's port forward.
//!
//! - TLS on every connection (the box's self-signed identity, `cert.rs`); a
//!   handshake not done within its deadline is cut off, however its bytes
//!   trickle in.
//! - HTTP/1.1 through `makepad_bounded_http` (budgets on every part of a
//!   request, slowloris deadlines, no percent-decoding).
//! - Connection caps, process-wide and per address: over them a connection is
//!   closed before its handshake, and the phone page retries. A kept-alive
//!   connection waits only so long for its next request, and not at all
//!   while the caps are nearly reached.
//! - Every route lives under the party code (`/p/<code>/...`); anything else is
//!   a bare 404 that closes its connection, so a scan of the address learns
//!   nothing. Addresses that ask for wrong paths again and again are closed
//!   before their handshake for a while.
//!
//! One thread per connection (bounded by the caps); handlers run on that
//! thread and never on a UI thread.

// A native server: never built for wasm.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

pub mod limits;

use makepad_bounded_http::{BodyError, Conn, Head, HeadError, Method, Resp, Transport};
use makepad_network::tls::{TlsServer, TlsStream};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::ops::Deref;
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub use limits::{ByteBudget, CodeGuard, RateLimiter, Reserved};

/// Connections served at once, by all addresses.
const MAX_CONNS: usize = 256;
/// Connections served at once from one address. A phone opens about six;
/// a party behind one NAT (the router's own address, seen through the port
/// forward's hairpin) shares this, so it is generous.
const MAX_CONNS_PER_IP: usize = 64;
/// A handshake or a request head that has not come by then is dropped.
const READ_TIMEOUT_MS: u64 = 10_000;
const WRITE_TIMEOUT_MS: u64 = 30_000;
const HEAD_DEADLINE_MS: u64 = 10_000;

/// How long a connection may hold its slot without doing anything.
#[derive(Clone, Copy)]
struct Limits {
    /// The whole TLS handshake, however its bytes arrive: the read timeout
    /// alone bounds only each read, so a peer sending a byte every few
    /// seconds would hold its connection slot for as long as it liked.
    handshake: Duration,
    /// How long a connection waits for its first request, and a kept-alive
    /// one for its next. The phone page asks every few seconds while it is
    /// open; a connection that goes quiet gives its slot back.
    idle: Duration,
    /// Connections served at once, by all addresses. Once three quarters of
    /// them are taken, a response closes its connection instead of keeping
    /// it alive: slots go to whoever asks next, not to whoever holds on.
    conns: usize,
}

const LIMITS: Limits = Limits { handshake: Duration::from_secs(10), idle: Duration::from_secs(10), conns: MAX_CONNS };

/// A TLS stream as a `bounded_http` transport: the socket options reach the
/// TCP stream underneath.
pub struct TlsTransport {
    tls: TlsStream,
    tcp: TcpStream,
}

impl Read for TlsTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.tls.read(buf)
    }
}

impl Write for TlsTransport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tls.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tls.flush()
    }
}

impl Transport for TlsTransport {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.tcp.set_read_timeout(timeout)
    }

    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        self.tcp.set_nonblocking(nonblocking)
    }

    fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.peer_addr()
    }
}

/// One request, as a handler sees it.
pub struct Request<'a> {
    pub head: &'a mut Head,
    /// The socket's peer (never a header).
    pub peer: IpAddr,
    /// The path below `/p/<code>/`.
    pub route: Vec<String>,
    conn: &'a mut Conn<TlsTransport>,
}

impl Request<'_> {
    pub fn method(&self) -> Method {
        self.head.method
    }

    pub fn query(&self, key: &str) -> Option<&str> {
        self.head.query_get(key)
    }

    /// The whole body, up to `max` bytes.
    pub fn body(&mut self, max: u64) -> Result<Vec<u8>, Resp> {
        self.conn.read_body_full(self.head, max, 120_000).map_err(body_error)
    }

    /// The whole body, up to `max` bytes, read only while `budget` and this
    /// address's share of it have room (its Content-Length, else `max`):
    /// the room stays taken as long as the body is held. A body declared
    /// over `max` is a 413 at once. No room: a 503 to come back in a few
    /// seconds; the body (at most `max`) is then read away without being
    /// kept, so the answer is not lost to a reset when the connection closes.
    pub fn body_within<'b>(&mut self, max: u64, budget: &'b ByteBudget) -> Result<HeldBody<'b>, Resp> {
        if self.head.content_length.is_some_and(|declared| declared > max) {
            return Err(text(413, "too large").closing());
        }
        let want = self.head.content_length.unwrap_or(max);
        let Some(reserved) = budget.reserve(self.peer, want as usize) else {
            self.drain(max);
            return Err(busy(5, "the box is taking in a lot at once: sending again shortly"));
        };
        Ok(HeldBody { bytes: self.body(max)?, _reserved: reserved })
    }

    /// Reads away what is left of a refused body, up to `max` bytes.
    fn drain(&mut self, max: u64) {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut sink = vec![0u8; 64 * 1024];
        while let Ok(n) = self.conn.body_read(self.head, &mut sink, max, deadline) {
            if n == 0 {
                return;
            }
        }
    }

    /// The body in pieces as they arrive, up to `max` bytes in all; returns
    /// how many bytes came.
    pub fn body_each(&mut self, max: u64, mut each: impl FnMut(&[u8])) -> Result<u64, Resp> {
        let deadline = Instant::now() + Duration::from_secs(600);
        let mut chunk = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = self.conn.body_read(self.head, &mut chunk, max, deadline).map_err(body_error)?;
            if n == 0 {
                return Ok(total);
            }
            total += n as u64;
            each(&chunk[..n]);
        }
    }
}

/// A body read whole, and its part of the budget: both go together, so no
/// byte of it is ever held outside the budget.
pub struct HeldBody<'b> {
    bytes: Vec<u8>,
    _reserved: Reserved<'b>,
}

impl Deref for HeldBody<'_> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

fn body_error(error: BodyError) -> Resp {
    match error {
        BodyError::TooLarge => text(413, "too large"),
        BodyError::Timeout => text(408, "body timeout").closing(),
        BodyError::Malformed => text(400, "malformed body").closing(),
        BodyError::Io => text(400, "connection").closing(),
    }
}

/// 503 with a Retry-After: the request was not taken, and nothing of what
/// was held before is lost; the phone page sends it again then.
pub fn busy(retry_after_s: u64, why: &str) -> Resp {
    text(503, why).with_header("Retry-After", retry_after_s.max(1).to_string()).closing()
}

/// 429 with a Retry-After and the reason the guest's request must wait.
pub fn slow_down(wait: Duration, why: &str) -> Resp {
    text(429, why).with_header("Retry-After", wait.as_secs().max(1).to_string())
}

pub fn text(status: u16, body: &str) -> Resp {
    Resp::bytes(status, "text/plain; charset=utf-8", body.as_bytes().to_vec())
}

pub fn json(status: u16, body: String) -> Resp {
    Resp::bytes(status, "application/json", body.into_bytes()).with_header("Cache-Control", "no-store".into())
}

/// A page of ours: no framing by other sites, nothing loaded from elsewhere.
pub fn html(body: &str) -> Resp {
    Resp::bytes(200, "text/html; charset=utf-8", body.as_bytes().to_vec())
        .with_header("Cache-Control", "no-store".into())
        .with_header(
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' blob: data:; media-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'"
                .into(),
        )
        .with_header("Referrer-Policy", "no-referrer".into())
}

/// What answers the routes under the party code.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, request: &mut Request<'_>) -> Resp;
}

/// The running server: [`Server::stop`] ends its accept loop.
pub struct Server {
    pub port: u16,
    stop: Arc<AtomicBool>,
    wake: SocketAddr,
    thread: Option<std::thread::JoinHandle<()>>,
    handshakes: Arc<Handshakes>,
    reaper: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// Listens on `bind` (`0.0.0.0:<port>` for the LAN) and serves the
    /// routes under `/p/<code>/` with `handler`.
    pub fn start(bind: SocketAddr, tls: TlsServer, code: String, handler: Arc<dyn Handler>) -> io::Result<Server> {
        Self::start_with(bind, tls, code, handler, LIMITS)
    }

    fn start_with(bind: SocketAddr, tls: TlsServer, code: String, handler: Arc<dyn Handler>, limits: Limits) -> io::Result<Server> {
        let listener = TcpListener::bind(bind)?;
        let local = listener.local_addr()?;
        let wake = SocketAddr::new(if local.ip().is_unspecified() { [127, 0, 0, 1].into() } else { local.ip() }, local.port());
        let stop = Arc::new(AtomicBool::new(false));
        let handshakes = Arc::new(Handshakes::default());
        let reaper = {
            let handshakes = handshakes.clone();
            std::thread::Builder::new().name("party-handshakes".into()).spawn(move || handshakes.run())?
        };
        let shared = Arc::new(Shared {
            tls,
            code,
            handler,
            stop: stop.clone(),
            per_ip: Mutex::new(HashMap::new()),
            active: AtomicUsize::new(0),
            guesses: CodeGuard::default(),
            handshakes: handshakes.clone(),
            limits,
        });
        let thread = std::thread::Builder::new()
            .name("party-accept".into())
            .spawn(move || accept_loop(listener, shared))?;
        Ok(Server { port: local.port(), stop, wake, thread: Some(thread), handshakes, reaper: Some(reaper) })
    }

    pub fn stop(&mut self) {
        if !self.stop.swap(true, Ordering::AcqRel) {
            let _ = TcpStream::connect(self.wake);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.handshakes.stop();
        if let Some(reaper) = self.reaper.take() {
            let _ = reaper.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Shared {
    tls: TlsServer,
    code: String,
    handler: Arc<dyn Handler>,
    stop: Arc<AtomicBool>,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    active: AtomicUsize,
    guesses: CodeGuard,
    handshakes: Arc<Handshakes>,
    limits: Limits,
}

impl Shared {
    fn crowded(&self) -> bool {
        self.active.load(Ordering::Acquire) * 4 > self.limits.conns * 3
    }
}

/// The handshakes under way, each with its deadline. One thread for all of
/// them sleeps until the nearest and shuts that socket down if it is still
/// there: its blocked read ends and the connection with it.
#[derive(Default)]
struct Handshakes {
    list: Mutex<HandshakeList>,
    changed: Condvar,
}

#[derive(Default)]
struct HandshakeList {
    next: u64,
    open: Vec<(u64, Instant, TcpStream)>,
    stop: bool,
}

/// One handshake's place in [`Handshakes`]: dropped once it is done.
struct Watch {
    handshakes: Arc<Handshakes>,
    id: u64,
}

impl Handshakes {
    fn lock(&self) -> MutexGuard<'_, HandshakeList> {
        self.list.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn watch(self: &Arc<Self>, tcp: &TcpStream, within: Duration) -> Option<Watch> {
        let socket = tcp.try_clone().ok()?;
        let mut list = self.lock();
        let id = list.next;
        list.next += 1;
        list.open.push((id, Instant::now() + within, socket));
        drop(list);
        self.changed.notify_one();
        Some(Watch { handshakes: self.clone(), id })
    }

    fn stop(&self) {
        self.lock().stop = true;
        self.changed.notify_one();
    }

    fn run(&self) {
        let mut list = self.lock();
        while !list.stop {
            let now = Instant::now();
            list.open.retain(|(_, deadline, socket)| {
                if *deadline > now {
                    return true;
                }
                let _ = socket.shutdown(Shutdown::Both);
                false
            });
            let nearest = list.open.iter().map(|(_, deadline, _)| *deadline).min();
            list = match nearest {
                Some(deadline) => self.changed.wait_timeout(list, deadline - now).unwrap_or_else(|p| p.into_inner()).0,
                None => self.changed.wait(list).unwrap_or_else(|p| p.into_inner()),
            };
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.handshakes.lock().open.retain(|(id, _, _)| *id != self.id);
    }
}

/// Holds one connection's place in the caps.
struct Slot {
    shared: Arc<Shared>,
    ip: IpAddr,
}

impl Slot {
    fn take(shared: &Arc<Shared>, ip: IpAddr) -> Option<Slot> {
        if shared.active.fetch_add(1, Ordering::AcqRel) >= shared.limits.conns {
            shared.active.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        let mut per_ip = shared.per_ip.lock().unwrap_or_else(|p| p.into_inner());
        let count = per_ip.entry(ip).or_default();
        if *count >= MAX_CONNS_PER_IP {
            drop(per_ip);
            shared.active.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        *count += 1;
        Some(Slot { shared: shared.clone(), ip })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut per_ip = self.shared.per_ip.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(count) = per_ip.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                per_ip.remove(&self.ip);
            }
        }
        drop(per_ip);
        self.shared.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    for tcp in listener.incoming() {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        let Ok(tcp) = tcp else { continue };
        let Ok(peer) = tcp.peer_addr() else { continue };
        let ip = canonical(peer.ip());
        // Over a cap, or a guesser: closed at once, before any handshake work.
        if shared.guesses.refused(ip) {
            continue;
        }
        let Some(slot) = Slot::take(&shared, ip) else { continue };
        let _ = std::thread::Builder::new()
            .name("party-conn".into())
            .spawn(move || serve_connection(tcp, ip, slot));
    }
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

fn serve_connection(tcp: TcpStream, ip: IpAddr, slot: Slot) {
    let shared = slot.shared.clone();
    if tcp.set_read_timeout(Some(Duration::from_millis(READ_TIMEOUT_MS))).is_err()
        || tcp.set_write_timeout(Some(Duration::from_millis(WRITE_TIMEOUT_MS))).is_err()
    {
        return;
    }
    let Ok(tcp_handle) = makepad_network::clone_tcp(&tcp) else { return };
    let Some(watch) = shared.handshakes.watch(&tcp, shared.limits.handshake) else { return };
    let accepted = shared.tls.accept(tcp);
    drop(watch);
    let Ok(tls) = accepted else { return };
    let mut conn = Conn::over(TlsTransport { tls, tcp: tcp_handle }, READ_TIMEOUT_MS);
    let idle_ms = shared.limits.idle.as_millis() as u64;
    loop {
        let mut head = match conn.next_request(HEAD_DEADLINE_MS, idle_ms, &shared.stop) {
            Ok(head) => head,
            Err(HeadError::Closed | HeadError::Io) => return,
            Err(HeadError::Timeout) => {
                let _ = conn.write_resp(false, &text(408, "head timeout").closing());
                return;
            }
            Err(HeadError::Bad(status, message)) => {
                let _ = conn.write_resp(false, &text(status, message).closing());
                return;
            }
        };
        let mut response = route(&shared, &mut conn, &mut head, ip);
        let keep = conn.finish_request(&mut head) && !response.close && !shared.stop.load(Ordering::Relaxed) && !shared.crowded();
        response.close = !keep;
        if conn.write_resp(head.method == Method::Head, &response).is_err() || response.close {
            return;
        }
    }
}

fn route(shared: &Shared, conn: &mut Conn<TlsTransport>, head: &mut Head, ip: IpAddr) -> Resp {
    if shared.guesses.refused(ip) {
        return slow_down(Duration::from_secs(60), "too many attempts to find the party: trying again in a minute").closing();
    }
    // A GET for /favicon.ico is not a code guess. Every other path outside
    // the code counts against its address, a bare `/` too.
    if head.method == Method::Get && head.segs.len() == 1 && head.segs[0] == "favicon.ico" {
        return text(404, "not found").closing();
    }
    let under_code = head.segs.len() >= 2 && head.segs[0] == "p";
    if !under_code || !makepad_network::tls::constant_time_eq(head.segs[1].as_bytes(), shared.code.as_bytes()) {
        shared.guesses.wrong(ip);
        return text(404, "not found").closing();
    }
    let route = head.segs[2..].to_vec();
    let mut request = Request { head, peer: ip, route, conn };
    shared.handler.handle(&mut request)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use makepad_network::tls::{SubjectName, TlsIdentity};

    #[test]
    fn busy_and_slow_answers_say_why_and_when() {
        let why = "the box needs a moment: sending again shortly";
        for (status, response) in [
            (503, busy(0, why)),
            (503, busy(5, why)),
            (429, slow_down(Duration::ZERO, why)),
            (429, slow_down(Duration::from_millis(200), why)),
            (429, slow_down(Duration::from_secs(5), why)),
        ] {
            assert_eq!(response.status, status, "the refusal's status");
            assert!(response.headers.iter().any(|(k, v)| *k == "Retry-After" && v.parse::<u64>().is_ok_and(|s| s >= 1)), "the refusal says when to retry");
            assert_eq!(response.body, why.as_bytes(), "the guest sees the reason");
        }
    }

    /// A server on a loopback port with a throwaway identity; the directory
    /// is the caller's to remove.
    pub(crate) fn start(code: &str, handler: Arc<dyn Handler>) -> (Server, std::path::PathBuf) {
        start_with_limits(code, handler, LIMITS)
    }

    fn start_with_limits(code: &str, handler: Arc<dyn Handler>, limits: Limits) -> (Server, std::path::PathBuf) {
        let mut nonce = [0u8; 8];
        makepad_network::tls::os_random(&mut nonce).unwrap();
        let dir = std::env::temp_dir().join(format!("party-server-test-{}", makepad_network::tls::to_hex(&nonce)));
        let identity = TlsIdentity::load_or_create_for_names(&dir, "party test", &[SubjectName::Ip([127, 0, 0, 1].into())]).unwrap();
        let tls = TlsServer::new(identity).unwrap();
        (Server::start_with(([127, 0, 0, 1], 0).into(), tls, code.into(), handler, limits).unwrap(), dir)
    }

    /// One answer on a kept-alive connection: its head and body.
    fn answer(client: &mut makepad_network::SocketStream) -> (String, Vec<u8>) {
        let mut bytes = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            if let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..split]).into_owned();
                let length: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .and_then(|l| l.trim().parse().ok())
                    .unwrap_or(0);
                if bytes.len() >= split + 4 + length {
                    return (head, bytes[split + 4..split + 4 + length].to_vec());
                }
            }
            let n = client.read(&mut buf).expect("the answer");
            assert!(n > 0, "the connection closed before its answer");
            bytes.extend_from_slice(&buf[..n]);
        }
    }

    /// Whether the server has closed `client` (rather than kept it waiting).
    fn closed(client: &mut impl Read) -> bool {
        let mut buf = [0u8; 64];
        match client.read(&mut buf) {
            Ok(0) => true,
            Ok(_) => false,
            Err(e) => !matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut),
        }
    }

    /// One request over TLS (it should say `Connection: close`); the whole answer.
    pub(crate) fn exchange_bytes(port: u16, raw: &[u8]) -> Vec<u8> {
        let (mut client, _) = makepad_network::SocketStream::connect_capture("127.0.0.1", &port.to_string()).unwrap();
        client.write_all(raw).unwrap();
        let mut out = Vec::new();
        let _ = client.read_to_end(&mut out);
        out
    }

    struct Echo;
    impl Handler for Echo {
        fn handle(&self, request: &mut Request<'_>) -> Resp {
            let body = match request.body(16) {
                Ok(body) => body,
                Err(resp) => return resp,
            };
            text(200, &format!("{} {} {}", request.method().as_str(), request.route.join("/"), body.len()))
        }
    }

    fn exchange(port: u16, raw: &str) -> String {
        String::from_utf8_lossy(&exchange_bytes(port, raw.as_bytes())).into_owned()
    }

    /// The whole front over real TLS: routes only under the code, a body
    /// budget, and an address that asks for wrong paths (wrong codes, or
    /// none at all) closed before its handshake once it has asked too many.
    #[test]
    fn serves_under_the_code_over_tls() {
        let (mut server, dir) = start("abcdefgh", Arc::new(Echo));
        let port = server.port;
        let ok = exchange(port, "POST /p/abcdefgh/x/y HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc");
        assert!(ok.starts_with("HTTP/1.1 200") && ok.ends_with("POST x/y 3"), "{ok}");
        let big = exchange(port, "POST /p/abcdefgh/x HTTP/1.1\r\nHost: a\r\nContent-Length: 17\r\nConnection: close\r\n\r\n01234567890123456");
        assert!(big.starts_with("HTTP/1.1 413"), "a body over its budget: {big}");
        // A bare path is a guess too, and its 404 does not keep the connection.
        let (mut client, _) = makepad_network::SocketStream::connect_capture("127.0.0.1", &port.to_string()).unwrap();
        client.write_all(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
        let (bare, _) = answer(&mut client);
        assert!(bare.starts_with("HTTP/1.1 404") && bare.contains("Connection: close") && closed(&mut client), "{bare}");
        for _ in 1..limits::WRONG_CODES {
            let wrong = exchange(port, "GET /p/zzzzzzzz/x HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n");
            assert!(wrong.starts_with("HTTP/1.1 404"), "{wrong}");
        }
        // Now even a request with the right code finds the door shut: no handshake.
        let mut tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = tcp.write_all(&[0x16, 0x03, 0x01, 0x00, 0x00]);
        let started = Instant::now();
        assert!(closed(&mut tcp) && started.elapsed() < Duration::from_secs(2), "a guesser is closed before its handshake");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_favicon_request_is_not_a_wrong_code() {
        let (mut server, dir) = start("abcdefgh", Arc::new(Echo));
        let port = server.port;
        for _ in 0..limits::WRONG_CODES + 5 {
            let favicon = exchange(port, "GET /favicon.ico HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n");
            assert!(favicon.starts_with("HTTP/1.1 404"), "a favicon request: {favicon}");
        }
        let ok = exchange(port, "GET /p/abcdefgh/x HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200") && ok.ends_with("GET x 0"), "favicon requests do not refuse the address: {ok}");
        assert!(include_str!("../../web/phone.html").contains(r#"<link rel="icon" href="data:,">"#), "the phone page declares its icon");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Takes bodies within a budget of 1000 bytes, 10 per address.
    struct Within(Arc<ByteBudget>);
    impl Handler for Within {
        fn handle(&self, request: &mut Request<'_>) -> Resp {
            match request.body_within(16, &self.0) {
                Ok(body) => text(200, &format!("{} held {}", body.len(), self.0.used())),
                Err(resp) => resp,
            }
        }
    }

    /// A body is read only within its address's share of the budget, which
    /// stays taken while the body is held; a body declared over the
    /// handler's maximum is refused at once, without waiting for it.
    #[test]
    fn bodies_wait_for_their_share_of_the_budget() {
        let budget = Arc::new(ByteBudget::with_share(1000, 10));
        let (mut server, dir) = start("abcdefgh", Arc::new(Within(budget.clone())));
        let post = |declared: usize, body: &str| exchange(server.port, &format!("POST /p/abcdefgh/x HTTP/1.1\r\nHost: a\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n{body}"));
        let fits = post(8, "01234567");
        assert!(fits.starts_with("HTTP/1.1 200") && fits.ends_with("8 held 8"), "a body within its share, held while in use: {fits}");
        let over = post(12, "0123456789ab");
        assert!(over.starts_with("HTTP/1.1 503") && over.contains("Retry-After: 5"), "past the address's share: come back shortly: {over}");
        let started = Instant::now();
        let huge = post(1 << 30, "");
        assert!(huge.starts_with("HTTP/1.1 413") && started.elapsed() < Duration::from_secs(2), "declared over the maximum: refused at once: {huge}");
        assert_eq!(budget.used(), 0, "every part given back");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A kept-alive connection gives its slot back once it goes quiet, and
    /// is not kept at all while the slots are nearly taken; one that keeps
    /// asking in time is served.
    #[test]
    fn keep_alive_ends_when_idle_or_crowded() {
        let limits = Limits { idle: Duration::from_millis(300), conns: 2, ..LIMITS };
        let (mut server, dir) = start_with_limits("abcdefgh", Arc::new(Echo), limits);
        let connect = || {
            let (client, _) = makepad_network::SocketStream::connect_capture("127.0.0.1", &server.port.to_string()).unwrap();
            client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            client
        };
        let ask = b"GET /p/abcdefgh/x HTTP/1.1\r\nHost: a\r\n\r\n";
        let mut a = connect();
        for _ in 0..2 {
            a.write_all(ask).unwrap();
            let (head, _) = answer(&mut a);
            assert!(head.starts_with("HTTP/1.1 200") && !head.contains("Connection: close"), "asking in time, kept alive while there is room: {head}");
        }
        std::thread::sleep(Duration::from_millis(700));
        assert!(closed(&mut a), "closed once it idles past its limit");
        drop(a);
        // Both slots taken: the first to ask is answered, then closed.
        let (mut b, mut c) = (connect(), connect());
        b.write_all(ask).unwrap();
        let (head, _) = answer(&mut b);
        assert!(head.starts_with("HTTP/1.1 200") && head.contains("Connection: close") && closed(&mut b), "crowded: answered, then closed: {head}");
        c.write_all(ask).unwrap();
        assert!(answer(&mut c).0.starts_with("HTTP/1.1 200"), "and the other still served");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A handshake trickled in a byte at a time, each well within the read
    /// timeout, is cut off at its deadline, and its slot comes back.
    #[test]
    fn a_trickled_handshake_is_cut_off() {
        let (mut server, dir) = start_with_limits("abcdefgh", Arc::new(Echo), Limits { handshake: Duration::from_millis(300), ..LIMITS });
        let port = server.port;
        let mut tcp = TcpStream::connect(("127.0.0.1", port)).unwrap();
        tcp.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let started = Instant::now();
        // A TLS record header promising 512 bytes, then its bytes one by one.
        let mut closed = false;
        for byte in [0x16u8, 0x03, 0x01, 0x02, 0x00].into_iter().chain(std::iter::repeat(0x01)).take(40) {
            if tcp.write_all(&[byte]).is_err() {
                closed = true;
                break;
            }
            let mut buf = [0u8; 64];
            match tcp.read(&mut buf) {
                Ok(0) => {
                    closed = true;
                    break;
                }
                Ok(_) => {}
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                Err(_) => {
                    closed = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let took = started.elapsed();
        assert!(closed && took < Duration::from_secs(2), "the trickled handshake is cut off at its deadline (closed {closed}, {took:?})");
        let ok = exchange(port, "GET /p/abcdefgh/x HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200"), "a real handshake is still served: {ok}");
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
