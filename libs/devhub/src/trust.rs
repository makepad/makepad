//! How hubs trust each other: the fleet identity (`makepad_network::fleet::id`,
//! tools/fleet-identity.md) behind one small trait ([`Trust`]).
//!
//! - The listener's TLS certificate carries its credential: a dialler checks it against the fleet
//!   root before it sends anything ([`id::connect`]) and dials only fleet machines.
//! - The dialler then sends `devhub/1 <name> MKID <credential>.<time>.<signature>\n`, a proof bound
//!   to the listener's certificate. The listener checks it ([`id::Verifier::check`], with the
//!   address the connection came from), classifies the peer and picks the endpoint for that role
//!   (none: refused), and answers `ok\n` or `no <reason>\n`.
//! - Roles: [`PeerRole::Master`] = `Need::Admin` (the admin Mac's device credential AND its bound
//!   address); [`PeerRole::Node`] = `Need::Node`; anything else has no rights here.
//!
//! Listeners bind a private LAN address only, close non-LAN peers unread, ban an address after
//! repeated failed proofs and cap sessions. Proofs and keys are never logged (names are).

use makepad_network::fleet::id::{self, Need};
use makepad_remote::carrier::tcp::carrier;
use makepad_remote::carrier::Carrier;
use makepad_remote::conn::{Conn, Hooks};
use makepad_remote::Endpoint;
use makepad_network::tls::{TlsIdentity, TlsServer, TlsStream};
use makepad_network::SocketStream;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The first word of the dialler's line.
pub const PROTOCOL: &str = "devhub/1";
const POLL: Duration = Duration::from_millis(20);
const HANDSHAKE: Duration = Duration::from_secs(10);
const MAX_LINE: usize = 2048;
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const BAN: Duration = Duration::from_secs(15 * 60);
const MAX_SESSIONS: usize = 16;
const MAX_PER_PEER: usize = 4;

fn read_line<S: Read>(s: &mut S) -> io::Result<String> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    loop {
        match s.read(&mut b) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the peer closed the connection")),
            Ok(_) if b[0] == b'\n' => break,
            Ok(_) => {
                line.push(b[0]);
                if line.len() > MAX_LINE {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "handshake line too long"));
                }
            }
            Err(e) => return Err(e),
        }
    }
    String::from_utf8(line).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "handshake line is not UTF-8"))
}

fn denied(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, msg)
}

/// Whether `name` is a hub name: what a line and a hosts entry can carry.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// What a proven peer may be to this hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerRole {
    /// the user's master (`Need::Admin`): full inspect, replies, sync
    Master,
    /// another hub of the fleet (`Need::Node`): cluster services only
    Node,
    /// a fleet member with no business here
    Other,
}

/// A peer whose credential checked out.
#[derive(Clone, Debug)]
pub struct Peer {
    /// its credential name
    pub id: String,
    /// the name it gave in its hello (a hub name)
    pub name: String,
    pub role: PeerRole,
    pub addr: SocketAddr,
}

/// The role of a proven fleet peer.
pub fn classify(peer: &id::Peer) -> PeerRole {
    if id::require(peer, Need::Admin).is_ok() {
        PeerRole::Master
    } else if id::require(peer, Need::Node).is_ok() {
        PeerRole::Node
    } else {
        PeerRole::Other
    }
}

/// Picks the endpoint that serves a proven peer; `Err` is the refusal sent back.
pub type Pick = Arc<dyn Fn(&Peer) -> Result<Endpoint, String> + Send + Sync>;

/// Called with every connection once its handshake is done.
pub type Accepted = Arc<dyn Fn(&Peer, &Conn) + Send + Sync>;

/// Log sink (names and addresses only).
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// How hubs dial and accept each other.
pub trait Trust: Send + Sync {
    /// An authenticated carrier to the hub at `addr` (`host:port`), ready for the HELLO.
    fn dial(&self, addr: &str) -> io::Result<Carrier>;
    /// Accepts hubs on `bind` (a private LAN address); returns the bound address.
    fn listen(&self, bind: SocketAddr, pick: Pick, accepted: Option<Accepted>, log: Log) -> io::Result<SocketAddr>;
}

/// The fleet identity of this machine (`~/.makepad/fleet`: id.key, id.cred, root.pub, revoked).
pub struct FleetTrust {
    /// this hub's name (in its hello)
    pub name: String,
}

impl FleetTrust {
    /// `None` (and why) when this machine has no fleet identity: the hub then runs local only.
    pub fn load(name: &str) -> Result<FleetTrust, String> {
        id::me().ok_or_else(id::why_no_identity)?;
        id::fleet().ok_or_else(|| format!("no fleet root in {}", id::dir().display()))?;
        Ok(FleetTrust { name: name.to_string() })
    }
}

impl Trust for FleetTrust {
    fn dial(&self, addr: &str) -> io::Result<Carrier> {
        let (host, port) = addr.rsplit_once(':').ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("{addr}: host:port")))?;
        let (mut s, server, auth) = id::connect(host, port)?;
        let r = (|| {
            // hubs serve on fleet nodes and on paired devices (a phone or headset JIT host)
            if !matches!(server.role, id::Role::Node | id::Role::Paired) {
                return Err(denied(format!("{addr}: {} ({}) is not a node or paired device", server.name, server.role.as_str())));
            }
            s.set_read_timeout(Some(HANDSHAKE))?;
            s.set_write_timeout(Some(HANDSHAKE))?;
            s.write_all(format!("{PROTOCOL} {} {auth}\n", self.name).as_bytes())?;
            s.flush()?;
            let answer = read_line(&mut s)?;
            match answer.split_once(' ').map_or((answer.as_str(), ""), |(a, b)| (a, b)) {
                ("ok", _) => Ok(()),
                ("no", why) => Err(denied(format!("{addr} refused: {why}"))),
                _ => Err(denied(format!("{addr} is not a devhub listener"))),
            }
        })();
        if let Err(e) = r {
            s.shutdown();
            return Err(e);
        }
        s.set_read_timeout(Some(POLL))?;
        s.set_write_timeout(Some(POLL))?;
        Ok(carrier(s, Hooks::default(), |c: &mut SocketStream| c.shutdown()))
    }

    fn listen(&self, bind: SocketAddr, pick: Pick, accepted: Option<Accepted>, log: Log) -> io::Result<SocketAddr> {
        let (identity, verifier) = id::server_identity(&format!("makepad devhub {}", self.name)).map_err(denied)?;
        listen(ListenConfig { bind, identity, verifier, pick, accepted, log })
    }
}

pub struct ListenConfig {
    pub bind: SocketAddr,
    pub identity: TlsIdentity,
    pub verifier: id::Verifier,
    pub pick: Pick,
    pub accepted: Option<Accepted>,
    pub log: Log,
}

/// A loopback, private or link-local address.
pub fn lan_peer(ip: IpAddr) -> bool {
    makepad_network::fleet::lan::is_lan_peer(id::normalize(ip))
}

#[derive(Default)]
struct Guard {
    failures: HashMap<IpAddr, (Vec<Instant>, Option<Instant>)>,
    sessions: HashMap<IpAddr, usize>,
    logged: HashMap<IpAddr, Instant>,
}

impl Guard {
    fn banned(&mut self, ip: IpAddr) -> bool {
        match self.failures.get_mut(&ip) {
            Some((_, Some(until))) if *until > Instant::now() => true,
            Some((fails, ban @ Some(_))) => {
                *ban = None;
                fails.clear();
                false
            }
            _ => false,
        }
    }

    /// Records a failed proof; true when it bans the address.
    fn fail(&mut self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let (fails, ban) = self.failures.entry(ip).or_default();
        fails.retain(|t| now.duration_since(*t) < FAILURE_WINDOW);
        fails.push(now);
        if fails.len() >= MAX_FAILURES {
            *ban = Some(now + BAN);
            fails.clear();
            return true;
        }
        false
    }

    fn enter(&mut self, ip: IpAddr) -> bool {
        let total: usize = self.sessions.values().sum();
        let n = self.sessions.entry(ip).or_default();
        if total >= MAX_SESSIONS || *n >= MAX_PER_PEER {
            return false;
        }
        *n += 1;
        true
    }

    fn leave(&mut self, ip: IpAddr) {
        if let Some(n) = self.sessions.get_mut(&ip) {
            *n = n.saturating_sub(1);
        }
    }
}

/// Listens on `cfg.bind`: a private LAN address of this machine (never the wildcard, loopback or
/// a routed address). Returns the bound address.
pub fn listen(cfg: ListenConfig) -> io::Result<SocketAddr> {
    makepad_network::fleet::lan::check_lan_bind("devhub", cfg.bind).map_err(denied)?;
    if !makepad_network::lan::service("devhub") {
        return Err(denied("devhub: this process may not use the LAN (MAKEPAD_LAN=0, or a hidden run without MAKEPAD_LAN=1)".into()));
    }
    let listener = TcpListener::bind(cfg.bind)?;
    let local = listener.local_addr()?;
    let tls = Arc::new(TlsServer::new(cfg.identity)?);
    let verifier = Arc::new(cfg.verifier);
    let guard = Arc::new(Mutex::new(Guard::default()));
    let (pick, accepted, log) = (cfg.pick, cfg.accepted, cfg.log);
    std::thread::Builder::new().name("devhub-accept".into()).spawn(move || {
        for tcp in listener.incoming() {
            let Ok(tcp) = tcp else { continue };
            let Ok(peer) = tcp.peer_addr() else { continue };
            let ip = id::normalize(peer.ip());
            if !lan_peer(ip) {
                continue;
            }
            {
                let mut g = guard.lock().unwrap();
                if g.banned(ip) || !g.enter(ip) {
                    continue;
                }
            }
            let (tls, verifier, guard, pick, accepted, log) = (tls.clone(), verifier.clone(), guard.clone(), pick.clone(), accepted.clone(), log.clone());
            let _ = std::thread::Builder::new().name("devhub-hello".into()).spawn(move || {
                let r = accept_one(&tls, &verifier, &pick, accepted.as_ref(), tcp, peer);
                match r {
                    Ok(()) => {}
                    Err((e, bad_proof)) => {
                        if bad_proof && guard.lock().unwrap().fail(ip) {
                            log(&format!("devhub: {ip} banned for {} s after failed credentials", BAN.as_secs()));
                        }
                        // one line per address and minute: a refused hub that keeps dialling does not flood the log
                        let mut g = guard.lock().unwrap();
                        if g.logged.get(&ip).map_or(true, |t| t.elapsed() >= Duration::from_secs(60)) {
                            g.logged.insert(ip, Instant::now());
                            drop(g);
                            log(&format!("devhub: peer {peer} dropped: {e}"));
                        }
                    }
                }
                guard.lock().unwrap().leave(ip);
            });
        }
    })?;
    Ok(local)
}

/// One accepted connection; `Err((why, was a bad credential))`. Returns once the connection ended.
fn accept_one(tls: &TlsServer, verifier: &id::Verifier, pick: &Pick, accepted: Option<&Accepted>, tcp: std::net::TcpStream, peer: SocketAddr) -> Result<(), (String, bool)> {
    let io = |e: io::Error| (e.to_string(), false);
    tcp.set_read_timeout(Some(HANDSHAKE)).map_err(io)?;
    tcp.set_write_timeout(Some(HANDSHAKE)).map_err(io)?;
    let mut s: TlsStream = tls.accept(tcp).map_err(io)?;
    let line = read_line(&mut s).map_err(io)?;
    let mut words = line.splitn(3, ' ');
    let (proto, name, auth) = (words.next().unwrap_or(""), words.next().unwrap_or(""), words.next().unwrap_or(""));
    let refuse = |s: &mut TlsStream, why: String, bad: bool| -> Result<(), (String, bool)> {
        let _ = s.write_all(format!("no {why}\n").as_bytes());
        let _ = s.flush();
        s.shutdown();
        Err((why, bad))
    };
    if proto != PROTOCOL {
        return refuse(&mut s, format!("protocol `{proto}`; this hub speaks {PROTOCOL}"), true);
    }
    if !valid_name(name) {
        return refuse(&mut s, "malformed hello".into(), true);
    }
    let fleet_peer = match verifier.check(auth, peer.ip(), id::now_secs()) {
        Ok(p) => p,
        Err(e) => return refuse(&mut s, format!("fleet credential: {e}"), true),
    };
    let who = Peer { id: fleet_peer.name.clone(), name: name.to_string(), role: classify(&fleet_peer), addr: peer };
    let ep = match pick(&who) {
        Ok(ep) => ep,
        Err(why) => return refuse(&mut s, why, false),
    };
    s.write_all(b"ok\n").map_err(io)?;
    s.flush().map_err(io)?;
    s.set_write_timeout(Some(POLL)).map_err(io)?;
    s.set_read_timeout(Some(POLL)).map_err(io)?;
    let c = carrier(s, ep.hooks(), |s: &mut TlsStream| s.shutdown());
    let conn = ep.serve(c.reader, c.writer, c.shutdown).map_err(|e| (e.to_string(), false))?;
    if let Some(a) = accepted {
        a(&who, &conn);
    }
    // the session slot is held while the connection lives
    while !conn.is_closed() {
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

