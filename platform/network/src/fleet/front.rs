//! The TLS front of a fleet service: the only listener a service exposes
//! off loopback. It terminates TLS (the machine's pinned certificate),
//! authenticates every request with a fleet credential, applies the
//! service's route policy, then hands the request to the service's HTTP
//! server on loopback with the caller's identity in headers that only the
//! front can set (they carry a per-process secret).
//!
//! The AI hub runs one per LAN listener and, optionally, an internet edge
//! (device routes only, tighter caps); the asset store runs one for sync.
//!
//! Hardening: a cap on connections in total and per address, 10 s for TLS,
//! a 16 KiB request head that must arrive within 15 s (slowloris), auth
//! failures throttled per address (5 in 10 min → 15 min ban), an audit line
//! per refusal, and idle connections closed after 10 min.

use super::id::{now_secs, Peer, Role, Verifier};
use crate::tls::{TlsServer, TlsStream};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const HEADER_FRONT: &str = "X-Makepad-Front";
pub const HEADER_CLIENT: &str = "X-Makepad-Client";
pub const HEADER_ROLE: &str = "X-Makepad-Role";
pub const HEADER_PEER: &str = "X-Makepad-Peer";
pub const HEADER_ADMIN: &str = "X-Makepad-Admin";
pub const HEADER_THIS_MACHINE: &str = "X-Makepad-This-Machine";

const MAX_HEAD: usize = 16 * 1024;
const HEAD_DEADLINE: Duration = Duration::from_secs(15);
const TLS_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(10 * 60);
const POLL: Duration = Duration::from_millis(5);
/// The poll while bytes moved within [`BUSY`].
const BUSY_POLL: Duration = Duration::from_millis(1);
const BUSY: Duration = Duration::from_millis(250);
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
const BAN: Duration = Duration::from_secs(15 * 60);

/// Who may reach which route of the service behind a front:
/// `(peer, method, path)`; the path may carry a query. Admin routes check
/// [`Peer::admin`] (the admin Mac's credential from its bound address).
pub type RoutePolicy = Arc<dyn Fn(&Peer, &str, &str) -> bool + Send + Sync>;

/// Which of the service's loopback servers serves a path (a service may
/// split its routes over several planes).
pub type InnerRoute = Arc<dyn Fn(&str) -> SocketAddr + Send + Sync>;

pub struct FrontConfig {
    pub listen: SocketAddr,
    /// The service's own HTTP server(s), on loopback, by request path.
    pub inner: InnerRoute,
    /// Extra header lines (`Name: value`) for every request the front hands
    /// on, e.g. the service's own credential for front-admitted callers.
    pub inner_headers: Vec<String>,
    pub tls: TlsServer,
    pub verifier: Arc<Verifier>,
    /// Per-process secret proving a request came through a front.
    pub front_secret: String,
    /// The service's route policy; anything it refuses gets 403.
    pub policy: RoutePolicy,
    /// Connection caps: in total and per client address.
    pub max_total: usize,
    pub max_per_ip: usize,
    /// Names the audit lines and threads (`front`, `edge`, `store-sync`).
    pub label: &'static str,
}

/// The caps of a LAN front (an internet edge uses tighter ones).
pub const LAN_MAX_TOTAL: usize = 256;
pub const LAN_MAX_PER_IP: usize = 32;

struct Limits {
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    failures: Mutex<HashMap<IpAddr, (Vec<Instant>, Option<Instant>)>>,
    total: AtomicUsize,
    max_total: usize,
    max_per_ip: usize,
}

impl Limits {
    fn banned(&self, ip: IpAddr) -> bool {
        let mut f = self.failures.lock().unwrap();
        match f.get_mut(&ip) {
            Some((_, Some(until))) if *until > Instant::now() => true,
            Some((fails, ban @ Some(_))) => {
                *ban = None;
                fails.clear();
                false
            }
            _ => false,
        }
    }

    fn fail(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut f = self.failures.lock().unwrap();
        let (fails, ban) = f.entry(ip).or_default();
        fails.retain(|t| now.duration_since(*t) < FAILURE_WINDOW);
        fails.push(now);
        if fails.len() >= MAX_FAILURES {
            *ban = Some(now + BAN);
            fails.clear();
            return true;
        }
        false
    }

    fn acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnSlot> {
        if self.total.fetch_add(1, Ordering::SeqCst) >= self.max_total {
            self.total.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        let mut per = self.per_ip.lock().unwrap();
        let n = per.entry(ip).or_default();
        if *n >= self.max_per_ip {
            drop(per);
            self.total.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        *n += 1;
        Some(ConnSlot { limits: self.clone(), ip })
    }
}

struct ConnSlot {
    limits: Arc<Limits>,
    ip: IpAddr,
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        self.limits.total.fetch_sub(1, Ordering::SeqCst);
        let mut per = self.limits.per_ip.lock().unwrap();
        if let Some(n) = per.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                per.remove(&self.ip);
            }
        }
    }
}

fn audit(label: &str, peer: &SocketAddr, what: &str) {
    eprintln!("{label} {peer}: {what}");
}

/// A running front. Dropping it leaves the front running (a service's whole
/// life); [`FrontHandle::stop`] closes the listener and joins its thread.
/// Connections already admitted finish on their own threads.
pub struct FrontHandle {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl FrontHandle {
    /// The bound address (the port is the OS's when `listen` asked for 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn stop(&mut self) {
        let Some(join) = self.join.take() else { return };
        self.stop.store(true, Ordering::SeqCst);
        // The acceptor blocks in accept (no polling while idle): one local
        // connection wakes it to see the flag.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
        let _ = join.join();
    }
}

pub fn start_front(config: FrontConfig) -> io::Result<FrontHandle> {
    let listener = TcpListener::bind(config.listen)?;
    let addr = listener.local_addr()?;
    let (max_total, max_per_ip) = (config.max_total, config.max_per_ip);
    let limits = Arc::new(Limits {
        per_ip: Mutex::new(HashMap::new()),
        failures: Mutex::new(HashMap::new()),
        total: AtomicUsize::new(0),
        max_total,
        max_per_ip,
    });
    let config = Arc::new(config);
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let join = std::thread::Builder::new()
        .name(format!("fleet-{}", config.label))
        .spawn(move || {
            for tcp in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(tcp) = tcp else { continue };
                let Ok(peer) = tcp.peer_addr() else { continue };
                let ip = normalize(peer.ip());
                if limits.banned(ip) {
                    continue;
                }
                let Some(slot) = limits.acquire(ip) else {
                    continue;
                };
                let config = config.clone();
                let limits = limits.clone();
                let _ = std::thread::Builder::new().name(format!("fleet-{}-conn", config.label)).spawn(move || {
                    let _slot = slot;
                    match serve(&config, tcp, peer) {
                        Ok(()) => {}
                        Err(Refusal::Auth(why)) => {
                            audit(config.label, &peer, &format!("auth refused: {why}"));
                            if limits.fail(ip) {
                                audit(config.label, &peer, &format!("banned for {} s", BAN.as_secs()));
                            }
                        }
                        Err(Refusal::Quiet) => {}
                        Err(Refusal::Other(why)) => audit(config.label, &peer, &why),
                    }
                });
            }
        })?;
    Ok(FrontHandle { addr, stop, join: Some(join) })
}

pub fn normalize(ip: IpAddr) -> IpAddr {
    crate::http_server::normalize_client_ip(ip)
}

enum Refusal {
    /// Counts toward a ban.
    Auth(String),
    /// Port probes and disconnects: nothing to say.
    Quiet,
    Other(String),
}

fn respond(tls: &mut TlsStream, status: u16, text: &str) {
    let body = format!("{{\"error\":\"{text}\"}}");
    let reason = match status {
        401 => "Unauthorized",
        403 => "Forbidden",
        431 => "Request Header Fields Too Large",
        _ => "Bad Request",
    };
    let _ = tls.write_all(
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
    let _ = tls.flush();
}

/// Reads one request head (through the blank line) within the deadline.
fn read_head(tls: &mut TlsStream) -> Result<(Vec<u8>, Vec<u8>), Refusal> {
    let deadline = Instant::now() + HEAD_DEADLINE;
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(end + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(Refusal::Other("request head too large".into()));
        }
        if Instant::now() > deadline {
            return Err(Refusal::Other("request head deadline".into()));
        }
        match tls.read(&mut chunk) {
            Ok(0) => return Err(Refusal::Quiet),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(_) => return Err(Refusal::Quiet),
        }
    }
}

/// The parsed head: method, path, the Authorization value, and the head
/// rewritten for the inner server (no Authorization, no X-Makepad-*, plus
/// identity).
pub struct ParsedHead {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    lines: Vec<String>,
}

pub fn parse_head(head: &[u8]) -> Option<ParsedHead> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n").filter(|l| !l.is_empty());
    let request_line = lines.next()?;
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    if !path.starts_with('/') {
        return None;
    }
    let mut authorization = None;
    let mut kept = vec![request_line.to_string()];
    for line in lines {
        let (name, value) = line.split_once(':')?;
        let name_l = name.trim().to_ascii_lowercase();
        if name_l == "authorization" {
            authorization = Some(value.trim().to_string());
            continue;
        }
        if name_l.starts_with("x-makepad-") {
            continue;
        }
        kept.push(line.to_string());
    }
    Some(ParsedHead { method, path, authorization, lines: kept })
}

impl ParsedHead {
    pub fn rewrite(&self, secret: &str, extra: &[String], peer: &Peer) -> Vec<u8> {
        let mut out = String::new();
        for line in self.lines.iter().chain(extra) {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str(&format!(
            "{HEADER_FRONT}: {secret}\r\n{HEADER_CLIENT}: {}\r\n{HEADER_ROLE}: {}\r\n{HEADER_ADMIN}: {}\r\n{HEADER_THIS_MACHINE}: {}\r\n{HEADER_PEER}: {}\r\n\r\n",
            peer.name,
            peer.role.as_str(),
            u8::from(peer.admin()),
            u8::from(peer.this_machine()),
            peer.addr
        ));
        out.into_bytes()
    }
}

fn serve(config: &FrontConfig, tcp: TcpStream, peer: SocketAddr) -> Result<(), Refusal> {
    // Responses go out as the inner server writes them (a websocket frame's
    // header and payload are separate writes): no Nagle wait between them.
    let _ = tcp.set_nodelay(true);
    let _ = tcp.set_read_timeout(Some(TLS_TIMEOUT));
    let _ = tcp.set_write_timeout(Some(TLS_TIMEOUT));
    // A connect-and-close probe is not a credential guess.
    let mut first = [0u8; 1];
    match tcp.peek(&mut first) {
        // A TLS handshake.
        Ok(1) if first[0] == 0x16 => {}
        // Plain HTTP (an old client): told to use https, not counted as a
        // credential guess.
        Ok(1) if first[0].is_ascii_uppercase() => {
            let mut tcp = tcp;
            let body = "{\"error\":\"this service speaks https only (see tools/fleet-identity.md)\"}";
            let _ = write!(
                tcp,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = tcp.shutdown(Shutdown::Both);
            return Err(Refusal::Other("plain HTTP refused".into()));
        }
        Ok(1) => {}
        _ => return Err(Refusal::Quiet),
    }
    let mut tls = config.tls.accept(tcp).map_err(|e| Refusal::Auth(format!("tls: {e}")))?;
    let _ = tls.set_read_timeout(Some(Duration::from_millis(500)));
    let (head, rest) = read_head(&mut tls)?;
    let Some(parsed) = parse_head(&head) else {
        respond(&mut tls, 400, "bad request");
        return Err(Refusal::Other("malformed request head".into()));
    };
    let Some(authorization) = parsed.authorization.as_deref() else {
        respond(&mut tls, 401, "a fleet credential is required");
        return Err(Refusal::Auth("no credential".into()));
    };
    let cred = match config.verifier.check(authorization, peer.ip(), now_secs()) {
        Ok(c) => c,
        Err(e) => {
            respond(&mut tls, 401, "credential refused");
            // Who it claims to be (the id and role are not secret; the proof
            // is never logged), so a refusal names its client.
            let claimed = authorization.trim().strip_prefix("MKID ").map(|v| v.split('.').skip(1).take(2).collect::<Vec<_>>().join(".")).unwrap_or_default();
            let claimed: String = claimed.chars().filter(|c| c.is_ascii_graphic()).take(80).collect();
            return Err(Refusal::Auth(format!("{e} (claims {claimed:?}, {} {})", parsed.method, parsed.path.chars().take(80).collect::<String>())));
        }
    };
    if !(config.policy)(&cred, &parsed.method, &parsed.path) {
        respond(&mut tls, 403, "not allowed for this credential");
        let admin = if cred.admin() { " (admin)" } else { "" };
        return Err(Refusal::Other(format!("{} {} {} refused for role {}{admin}", cred.name, parsed.method, parsed.path, cred.role.as_str())));
    }
    let mut inner = TcpStream::connect_timeout(&(config.inner)(&parsed.path), Duration::from_secs(5))
        .map_err(|e| Refusal::Other(format!("inner connect: {e}")))?;
    let _ = inner.set_nodelay(true);
    inner
        .write_all(&parsed.rewrite(&config.front_secret, &config.inner_headers, &cred))
        .and_then(|_| inner.write_all(&rest))
        .map_err(|e| Refusal::Other(format!("inner write: {e}")))?;
    pump(&mut tls, &mut inner);
    let _ = inner.shutdown(Shutdown::Both);
    tls.shutdown();
    Ok(())
}

/// Copies both ways until the inner side closes (the response is done) or
/// nothing moves for [`IDLE`]. One thread: the TLS object cannot be split,
/// so both directions are polled with short timeouts.
fn pump(tls: &mut TlsStream, inner: &mut TcpStream) {
    // Both sides are polled with timed reads, so a byte waits up to one poll
    // on each side: 1 ms while the connection is busy (a kernel stream
    // pushing tiles, a submit), POLL once it has been quiet for a moment.
    let mut poll = POLL;
    let _ = tls.set_read_timeout(Some(poll));
    let _ = inner.set_read_timeout(Some(poll));
    let mut a = vec![0u8; 64 * 1024];
    let mut b = vec![0u8; 64 * 1024];
    let mut last = Instant::now();
    let mut client_open = true;
    loop {
        let mut moved = false;
        if client_open {
            match tls.read(&mut a) {
                Ok(0) => {
                    client_open = false;
                    let _ = inner.shutdown(Shutdown::Write);
                }
                Ok(n) => {
                    if inner.write_all(&a[..n]).is_err() {
                        return;
                    }
                    moved = true;
                }
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
                Err(_) => client_open = false,
            }
        }
        match inner.read(&mut b) {
            Ok(0) => return,
            Ok(n) => {
                if tls.write_all(&b[..n]).is_err() {
                    return;
                }
                moved = true;
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => {}
            Err(_) => return,
        }
        if moved {
            last = Instant::now();
        } else if last.elapsed() > IDLE {
            return;
        }
        let want = if last.elapsed() < BUSY { BUSY_POLL } else { POLL };
        if want != poll {
            poll = want;
            let _ = tls.set_read_timeout(Some(poll));
            let _ = inner.set_read_timeout(Some(poll));
        }
    }
}

/// Inner-server side: the caller of a request, when it came through a front
/// of this process (the secret matches).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub client_id: String,
    pub role: Role,
    /// The admin Mac's credential on a connection from its bound address
    /// ([`Peer::admin`]); a machine-local caller is admin too.
    pub admin: bool,
    /// Another process of this machine, proven by its key ([`Peer::this_machine`]).
    pub this_machine: bool,
    /// The calling app's name (`X-Hub-App` header), for the control panel.
    pub app: Option<String>,
}

impl Caller {
    /// A machine-local (loopback-only node) or in-process caller.
    pub fn local() -> Self {
        Caller { client_id: "local".into(), role: Role::Device, admin: true, this_machine: true, app: None }
    }
}

pub fn caller_from_headers(lines: &[String], front_secret: &str) -> Option<Caller> {
    let get = |name: &str| {
        lines.iter().find_map(|l| {
            let (n, v) = l.trim_end().split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    };
    let secret = get(HEADER_FRONT)?;
    if !crate::tls::constant_time_eq(secret.as_bytes(), front_secret.as_bytes()) {
        return None;
    }
    Some(Caller {
        client_id: get(HEADER_CLIENT)?,
        role: Role::parse(&get(HEADER_ROLE)?)?,
        admin: get(HEADER_ADMIN)? == "1",
        this_machine: get(HEADER_THIS_MACHINE)? == "1",
        app: None,
    })
}

/// The real client address behind a front, for the inner server's per-IP
/// limits (trusted only from loopback, where the front is).
pub fn peer_from_headers(headers: &crate::HttpServerHeaders) -> IpAddr {
    headers
        .lines
        .iter()
        .find_map(|l| {
            let (n, v) = l.trim_end().split_once(':')?;
            n.trim().eq_ignore_ascii_case(HEADER_PEER).then(|| v.trim().parse().ok()).flatten()
        })
        .unwrap_or(IpAddr::from([127, 0, 0, 1]))
}

pub fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_rewrite_strips_credentials_and_spoofed_identity() {
        let head = b"POST /generate HTTP/1.1\r\nHost: x\r\nAuthorization: MKID mkid1.a.node.9.-.00.00.1.ff\r\nX-Makepad-Client: admin\r\nx-makepad-admin: 1\r\nx-makepad-front: guess\r\nContent-Length: 2\r\n\r\n";
        let p = parse_head(head).unwrap();
        assert_eq!(p.method, "POST");
        assert_eq!(p.path, "/generate");
        assert_eq!(p.authorization.as_deref(), Some("MKID mkid1.a.node.9.-.00.00.1.ff"));
        let mac: IpAddr = "10.0.0.222".parse().unwrap();
        let peer = Peer { name: "rik-mac".into(), role: Role::Device, addr: mac, from: Some(mac), point: [4; 65], this_machine: false };
        let out = String::from_utf8(p.rewrite("s3cret", &["Authorization: Bearer inner".to_string()], &peer)).unwrap();
        assert!(!out.contains("MKID"));
        assert!(out.contains("Authorization: Bearer inner\r\n"));
        assert!(!out.contains("guess") && !out.contains("admin\r\n"));
        assert!(out.contains("X-Makepad-Client: rik-mac\r\n"));
        assert!(out.ends_with("\r\n\r\n"));
        let lines: Vec<String> = out.split("\r\n").map(String::from).collect();
        assert_eq!(caller_from_headers(&lines, "s3cret"), Some(Caller { client_id: "rik-mac".into(), role: Role::Device, admin: true, this_machine: false, app: None }));
        assert_eq!(caller_from_headers(&lines, "other"), None);
        // The same credential from elsewhere is not admin behind the front either.
        let away = Peer { addr: "10.0.0.9".parse().unwrap(), ..peer };
        let lines: Vec<String> = String::from_utf8(p.rewrite("s3cret", &[], &away)).unwrap().split("\r\n").map(String::from).collect();
        assert_eq!(caller_from_headers(&lines, "s3cret").map(|c| c.admin), Some(false));
        // A client cannot claim to be this machine: its own header is stripped, the front's stamp says.
        let spoof = b"GET /control HTTP/1.1\r\nX-Makepad-This-Machine: 1\r\n\r\n";
        let node = Peer { name: "n166".into(), role: Role::Node, addr: "10.0.0.166".parse().unwrap(), from: None, point: [5; 65], this_machine: false };
        let lines: Vec<String> = String::from_utf8(parse_head(spoof).unwrap().rewrite("s3cret", &[], &node)).unwrap().split("\r\n").map(String::from).collect();
        assert_eq!(caller_from_headers(&lines, "s3cret").map(|c| c.this_machine), Some(false));
        let own = Peer { this_machine: true, ..node };
        let lines: Vec<String> = String::from_utf8(parse_head(spoof).unwrap().rewrite("s3cret", &[], &own)).unwrap().split("\r\n").map(String::from).collect();
        assert_eq!(caller_from_headers(&lines, "s3cret").map(|c| (c.this_machine, c.admin)), Some((true, false)));
    }
}
