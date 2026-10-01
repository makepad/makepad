//! The TLS front of a fleet node: the only listener a non-loopback node
//! exposes. It terminates TLS (the node's endorsed, pinned certificate),
//! authenticates every request with a fleet credential, applies the role's
//! route policy, then hands the request to the node's HTTP server on
//! loopback with the caller's identity in headers that only the front can
//! set (they carry a per-process secret).
//!
//! The same front, with `edge: true`, is the internet-ready listener of
//! the hub: device routes only.
//!
//! Hardening: a cap on connections in total and per address, 10 s for TLS,
//! a 16 KiB request head that must arrive within 15 s (slowloris), auth
//! failures throttled per address (5 in 10 min → 15 min ban), an audit line
//! per refusal, and idle connections closed after 10 min.

use crate::fleet_auth::{now_secs, Role, Verifier};
use makepad_network::tls::{TlsServer, TlsStream};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const HEADER_FRONT: &str = "X-Makepad-Front";
pub const HEADER_CLIENT: &str = "X-Makepad-Client";
pub const HEADER_ROLE: &str = "X-Makepad-Role";
pub const HEADER_PEER: &str = "X-Makepad-Peer";

const MAX_HEAD: usize = 16 * 1024;
const HEAD_DEADLINE: Duration = Duration::from_secs(15);
const TLS_TIMEOUT: Duration = Duration::from_secs(10);
const IDLE: Duration = Duration::from_secs(10 * 60);
const POLL: Duration = Duration::from_millis(5);
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);
const BAN: Duration = Duration::from_secs(15 * 60);

pub struct FrontConfig {
    pub listen: SocketAddr,
    pub inner: SocketAddr,
    pub tls: TlsServer,
    pub verifier: Arc<Verifier>,
    /// Per-process secret proving a request came through a front.
    pub front_secret: String,
    /// Internet edge: device routes only, tighter limits.
    pub edge: bool,
}

/// Which routes a role may reach (method, path). `edge` narrows every role
/// to the device set.
pub fn route_allowed(role: Role, edge: bool, method: &str, path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    let device = matches!(
        (method, path),
        ("GET", "/health") | ("GET", "/models") | ("POST", "/generate") | ("POST", "/realtime")
    ) || (method == "GET" && (path.starts_with("/job/") || path.starts_with("/artifact/") || path.starts_with("/realtime/")))
        || (method == "POST" && path.starts_with("/job/") && (path.ends_with("/cancel") || path.ends_with("/keepalive")))
        || (method == "POST" && path == "/bye");
    if edge {
        return device;
    }
    match role {
        Role::Lan => true,
        Role::Device => device,
        // Nodes fetch model blobs from each other (and check health).
        Role::Node => (method == "GET" && path.starts_with(crate::peer_serve::BLOB_PATH_PREFIX)) || (method == "GET" && path == "/health"),
    }
}

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

fn audit(edge: bool, peer: &SocketAddr, what: &str) {
    eprintln!("{} {peer}: {what}", if edge { "edge" } else { "front" });
}

pub fn start_front(config: FrontConfig) -> io::Result<std::thread::JoinHandle<()>> {
    let listener = TcpListener::bind(config.listen)?;
    let (max_total, max_per_ip) = if config.edge { (64, 8) } else { (256, 32) };
    let limits = Arc::new(Limits {
        per_ip: Mutex::new(HashMap::new()),
        failures: Mutex::new(HashMap::new()),
        total: AtomicUsize::new(0),
        max_total,
        max_per_ip,
    });
    let config = Arc::new(config);
    std::thread::Builder::new()
        .name(if config.edge { "ai-hub-edge" } else { "ai-hub-front" }.into())
        .spawn(move || {
            for tcp in listener.incoming() {
                let Ok(tcp) = tcp else { continue };
                let Ok(peer) = tcp.peer_addr() else { continue };
                let ip = crate::front::normalize(peer.ip());
                if limits.banned(ip) {
                    continue;
                }
                let Some(slot) = limits.acquire(ip) else {
                    continue;
                };
                let config = config.clone();
                let limits = limits.clone();
                let _ = std::thread::Builder::new().name("ai-hub-front-conn".into()).spawn(move || {
                    let _slot = slot;
                    match serve(&config, tcp, peer) {
                        Ok(()) => {}
                        Err(Refusal::Auth(why)) => {
                            audit(config.edge, &peer, &format!("auth refused: {why}"));
                            if limits.fail(ip) {
                                audit(config.edge, &peer, &format!("banned for {} s", BAN.as_secs()));
                            }
                        }
                        Err(Refusal::Quiet) => {}
                        Err(Refusal::Other(why)) => audit(config.edge, &peer, &why),
                    }
                });
            }
        })
}

pub fn normalize(ip: IpAddr) -> IpAddr {
    makepad_network::http_server::normalize_client_ip(ip)
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
    pub fn rewrite(&self, secret: &str, client: &str, role: Role, peer: IpAddr) -> Vec<u8> {
        let mut out = String::new();
        for line in &self.lines {
            out.push_str(line);
            out.push_str("\r\n");
        }
        out.push_str(&format!("{HEADER_FRONT}: {secret}\r\n{HEADER_CLIENT}: {client}\r\n{HEADER_ROLE}: {}\r\n{HEADER_PEER}: {peer}\r\n\r\n", role.as_str()));
        out.into_bytes()
    }
}

fn serve(config: &FrontConfig, tcp: TcpStream, peer: SocketAddr) -> Result<(), Refusal> {
    let _ = tcp.set_read_timeout(Some(TLS_TIMEOUT));
    let _ = tcp.set_write_timeout(Some(TLS_TIMEOUT));
    // A connect-and-close probe is not a credential guess.
    let mut first = [0u8; 1];
    match tcp.peek(&mut first) {
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
    let cred = match config.verifier.verify(authorization, now_secs()) {
        Ok(c) => c,
        Err(e) => {
            respond(&mut tls, 401, "credential refused");
            return Err(Refusal::Auth(e.to_string()));
        }
    };
    if !route_allowed(cred.role, config.edge, &parsed.method, &parsed.path) {
        respond(&mut tls, 403, "not allowed for this credential");
        return Err(Refusal::Other(format!("{} {} {} refused for role {}", cred.client_id, parsed.method, parsed.path, cred.role.as_str())));
    }
    let mut inner = TcpStream::connect_timeout(&config.inner, Duration::from_secs(5))
        .map_err(|e| Refusal::Other(format!("inner connect: {e}")))?;
    let _ = inner.set_nodelay(true);
    inner
        .write_all(&parsed.rewrite(&config.front_secret, &cred.client_id, cred.role, normalize(peer.ip())))
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
    let _ = tls.set_read_timeout(Some(POLL));
    let _ = inner.set_read_timeout(Some(POLL));
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
    }
}

/// Inner-server side: the caller of a request, when it came through a front
/// of this process (the secret matches).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub client_id: String,
    pub role: Role,
}

impl Caller {
    /// A machine-local (loopback-only node) or in-process caller.
    pub fn local() -> Self {
        Caller { client_id: "local".into(), role: Role::Lan }
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
    if !makepad_network::tls::constant_time_eq(secret.as_bytes(), front_secret.as_bytes()) {
        return None;
    }
    Some(Caller { client_id: get(HEADER_CLIENT)?, role: Role::parse(&get(HEADER_ROLE)?)? })
}

/// The real client address behind a front, for the inner server's per-IP
/// limits (trusted only from loopback, where the front is).
pub fn peer_from_headers(headers: &makepad_network::HttpServerHeaders) -> IpAddr {
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
        let head = b"POST /generate HTTP/1.1\r\nHost: x\r\nAuthorization: MKC2 a.lan.9.ff\r\nX-Makepad-Client: admin\r\nx-makepad-front: guess\r\nContent-Length: 2\r\n\r\n";
        let p = parse_head(head).unwrap();
        assert_eq!(p.method, "POST");
        assert_eq!(p.path, "/generate");
        assert_eq!(p.authorization.as_deref(), Some("MKC2 a.lan.9.ff"));
        let out = String::from_utf8(p.rewrite("s3cret", "rik-mac", Role::Lan, "10.0.0.5".parse().unwrap())).unwrap();
        assert!(!out.to_ascii_lowercase().contains("authorization"));
        assert!(!out.contains("guess") && !out.contains("admin"));
        assert!(out.contains("X-Makepad-Client: rik-mac\r\n"));
        assert!(out.ends_with("\r\n\r\n"));
        let lines: Vec<String> = out.split("\r\n").map(String::from).collect();
        assert_eq!(caller_from_headers(&lines, "s3cret"), Some(Caller { client_id: "rik-mac".into(), role: Role::Lan }));
        assert_eq!(caller_from_headers(&lines, "other"), None);
    }

    #[test]
    fn route_policy() {
        assert!(route_allowed(Role::Lan, false, "GET", "/jobs"));
        assert!(!route_allowed(Role::Device, false, "GET", "/jobs"));
        assert!(route_allowed(Role::Device, false, "POST", "/generate"));
        assert!(route_allowed(Role::Device, false, "GET", "/job/abc"));
        assert!(!route_allowed(Role::Device, false, "GET", "/v1/model_blob/00"));
        assert!(route_allowed(Role::Node, false, "GET", "/v1/model_blob/00"));
        assert!(!route_allowed(Role::Node, false, "POST", "/generate"));
        // The edge narrows even a LAN credential to the device set.
        assert!(!route_allowed(Role::Lan, true, "GET", "/jobs"));
        assert!(!route_allowed(Role::Lan, true, "GET", "/v1/model_inventory"));
        assert!(route_allowed(Role::Lan, true, "GET", "/models"));
    }
}
