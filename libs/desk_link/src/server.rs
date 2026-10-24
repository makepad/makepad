//! The Desk Link server: the listener and its sessions (threads; the owner's UI thread only
//! drains a channel). Shared by every desktop that serves Desk Link (the Linux window manager,
//! the macOS/Windows desk host); each gives its own input injection ([`Input`]).
//!
//! Security, in the order a connection meets it:
//! 1. nothing listens but on a LAN address, and only with a loadable fleet key
//!    (`Server::start` refuses otherwise, the AI hub's way; the owner's settings decide whether
//!    to start it at all);
//! 2. a peer that is not on the LAN (loopback, RFC 1918, link-local, ULA), or not in the
//!    server's `allow_peers` (the viewer machines' addresses: required, an empty list serves
//!    nobody), is closed unread; a peer whose credential fails again and again is banned a while;
//! 3. TLS with the desktop's own identity (its `tls_dir`, pinned by viewers);
//! 4. the viewer's first message carries a fleet proof bound to that certificate: role `lan`,
//!    not expired, not revoked, or the connection is refused and closed.
//!
//! One thread per session owns its TLS stream (TLS contexts are not safe for concurrent read
//! and write): it reads with a 1 ms timeout and writes what the UI queued in between. Input is
//! injected right there (`inject`), never through the UI thread.

use crate::{self as proto, ClientMsg, GamepadState, Inbox, ServerMsg};
use makepad_network::fleet::auth::{now_secs, Role, Verifier};
use std::net::IpAddr;
use std::path::PathBuf;
use makepad_network::tls::{TlsIdentity, TlsServer, TlsStream};
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A peer or bind address on this LAN: loopback, RFC 1918, link-local, IPv6 unique-local.
pub fn lan_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return lan_address(IpAddr::V4(v4));
            }
            let seg = v6.segments()[0];
            v6.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

/// A bind address the server accepts: a LAN address of this machine or all interfaces (the
/// peer filter keeps those LAN).
pub fn bindable(ip: IpAddr) -> bool {
    ip.is_unspecified() || lan_address(ip)
}

/// Viewer input as local devices of this desktop. Positions are desktop-normalised
/// (0..=65535 over the whole desktop; the server maps the viewer's stream position through
/// [`Geometry`]).
pub trait Input: Send {
    fn pointer(&mut self, x: u16, y: u16);
    fn button(&mut self, down: bool, button: u8);
    /// 1/120 notch units, +dy = wheel away from the user.
    fn scroll(&mut self, dx: i32, dy: i32);
    /// `keycode`: makepad `KeyCode` index.
    fn key(&mut self, down: bool, keycode: u32);
    fn gamepad(&mut self, slot: u8, state: &GamepadState);
    fn gamepad_gone(&mut self, slot: u8);
}

/// Makes a session's input devices (`Err`: why the session is view-only).
pub type InputFactory = Arc<dyn Fn() -> Result<Box<dyn Input>, String> + Send + Sync>;
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

pub struct ServerConfig {
    pub addr: SocketAddr,
    /// The only peer addresses served (the viewers' machines; IP pinning assumes a DHCP
    /// reservation for them). Empty: the server does not start.
    pub allow_peers: Vec<IpAddr>,
    /// The TLS identity's directory (made on first start) and its certificate's name.
    pub tls_dir: PathBuf,
    pub tls_name: String,
    /// `fleet.key` and `revoked` (the AI hub's fleet directory).
    pub fleet_dir: PathBuf,
    pub log: Log,
    /// The desktop's clock (ns): `Pong`, `Video.sent_ns` (and the owner's `built_ns`).
    pub clock: fn() -> u64,
    pub input: InputFactory,
}

/// What the sessions tell the UI thread.
pub enum ToUi {
    Opened { session: u32, peer: String, client: String, name: String, screen: i32, paused: bool, caps: u32, out: Sender<Out>, backlog: Arc<AtomicUsize> },
    Control { session: u32, control: Control },
    Closed { session: u32, reason: String },
}

pub enum Control {
    Pause,
    Resume,
    SelectScreen(u32),
    Keyframe,
    /// `split::*` for the stream.
    Split(u8),
    /// The viewer's display refresh (Hz, 0 = not known).
    Refresh(u16),
    Clipboard(String),
    Stats { frame: u32, decode_us: u32, present_us: u32 },
}

/// What the UI queues for a session.
pub enum Out {
    Msg(Vec<u8>),
    /// An encoded `Video` message: its `sent_ns` is stamped as it is written.
    Video(Vec<u8>),
    Close(String),
}

/// Where the streamed screen is on the desktop (device pixels), for mapping viewer input.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geometry {
    pub desktop: [u32; 2],
    pub screen: [u32; 4],
}

impl Geometry {
    /// A stream-normalised point (0..=65535 over the picture) on the desktop (0..=65535 over it).
    fn to_desktop(&self, x: u16, y: u16) -> (u16, u16) {
        let [dw, dh] = self.desktop;
        let [sx, sy, sw, sh] = self.screen;
        if dw == 0 || dh == 0 {
            return (x, y);
        }
        let px = sx as f64 + x as f64 / 65535.0 * sw.max(1) as f64;
        let py = sy as f64 + y as f64 / 65535.0 * sh.max(1) as f64;
        ((px / dw as f64 * 65535.0).round().clamp(0.0, 65535.0) as u16, (py / dh as f64 * 65535.0).round().clamp(0.0, 65535.0) as u16)
    }
}

pub struct Server {
    pub addr: SocketAddr,
    pub fingerprint: String,
    stop: Arc<AtomicBool>,
    pub rx: Receiver<ToUi>,
    pub geometry: Arc<Mutex<Geometry>>,
    /// Why viewer input is not injected, when it is not (no /dev/uinput access).
    pub input_note: Arc<Mutex<String>>,
}

/// A failed credential this many times within the window bans the address for a while.
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
const BAN: Duration = Duration::from_secs(15 * 60);
/// Sessions at once, in all and per address.
const MAX_SESSIONS: usize = 8;
const MAX_PER_PEER: usize = 4;

/// IPv4-mapped IPv6 as IPv4: one address, one entry.
fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Who may connect, who failed, and the rejection log's rate limit.
struct Guard {
    allow: Vec<IpAddr>,
    failures: Mutex<std::collections::HashMap<IpAddr, (Vec<Instant>, Option<Instant>)>>,
    /// Per refused address: when it was last logged and how many refusals since.
    logged: Mutex<std::collections::HashMap<IpAddr, (Instant, u32)>>,
    sessions: Mutex<std::collections::HashMap<IpAddr, usize>>,
    log: Log,
}

impl Guard {
    fn allowed(&self, ip: IpAddr) -> bool {
        let ip = normalize(ip);
        lan_address(ip) && self.allow.contains(&ip)
    }

    /// A refused connection, logged at most once a minute per address (address only).
    fn refused(&self, ip: IpAddr, why: &str) {
        let ip = normalize(ip);
        let mut logged = self.logged.lock().unwrap();
        let e = logged.entry(ip).or_insert((Instant::now() - Duration::from_secs(120), 0));
        e.1 += 1;
        if e.0.elapsed() >= Duration::from_secs(60) {
            (self.log)(&format!("desk link: refused {ip}: {why}{}", if e.1 > 1 { format!(" ({} times since the last note)", e.1) } else { String::new() }));
            *e = (Instant::now(), 0);
        }
    }

    fn banned(&self, ip: IpAddr) -> bool {
        let mut f = self.failures.lock().unwrap();
        match f.get_mut(&normalize(ip)) {
            Some((_, Some(until))) if *until > Instant::now() => true,
            Some((fails, ban @ Some(_))) => {
                *ban = None;
                fails.clear();
                false
            }
            _ => false,
        }
    }

    /// A failed credential; true when the address is banned from now on.
    fn fail(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut f = self.failures.lock().unwrap();
        let (fails, ban) = f.entry(normalize(ip)).or_default();
        fails.retain(|t| now.duration_since(*t) < FAILURE_WINDOW);
        fails.push(now);
        if fails.len() >= MAX_FAILURES {
            *ban = Some(now + BAN);
            fails.clear();
            return true;
        }
        false
    }

    fn enter(self: &Arc<Self>, ip: IpAddr) -> Option<SessionSlot> {
        let ip = normalize(ip);
        let mut s = self.sessions.lock().unwrap();
        let total: usize = s.values().sum();
        let n = s.entry(ip).or_default();
        if total >= MAX_SESSIONS || *n >= MAX_PER_PEER {
            return None;
        }
        *n += 1;
        Some(SessionSlot { guard: self.clone(), ip })
    }
}

struct SessionSlot {
    guard: Arc<Guard>,
    ip: IpAddr,
}

impl Drop for SessionSlot {
    fn drop(&mut self) {
        let mut s = self.guard.sessions.lock().unwrap();
        if let Some(n) = s.get_mut(&self.ip) {
            *n = n.saturating_sub(1);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn refuse(addr: &str, why: String) -> String {
    format!("desk link: listens on {addr} but {why}; a network-reachable desktop serves only to fleet credentials (tools/aihub-fleet.md)")
}

impl Server {
    /// Starts listening, or says why it does not: not a LAN address, no TLS identity, no fleet
    /// key. `wake` makes the owner's UI thread look at `rx`.
    pub fn start(config: ServerConfig, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Server, String> {
        let addr = config.addr;
        if config.allow_peers.is_empty() {
            return Err("desk link: no allow_peers (the viewer machines' addresses): it serves nobody, so it does not listen".into());
        }
        if !bindable(addr.ip()) {
            return Err(format!("desk link: {} is not a LAN address (loopback, 10/8, 172.16/12, 192.168/16, link-local, fc00::/7)", addr.ip()));
        }
        let text = addr.to_string();
        let identity = TlsIdentity::load_or_create(&config.tls_dir, &config.tls_name).map_err(|e| refuse(&text, format!("cannot make or load its TLS identity ({e})")))?;
        let verifier = Verifier::load(&config.fleet_dir, identity.fingerprint).map_err(|e| refuse(&text, format!("has no fleet key ({e})")))?;
        let fingerprint = identity.fingerprint_hex();
        let tls = Arc::new(TlsServer::new(identity).map_err(|e| format!("desk link: TLS server: {e}"))?);
        let listener = TcpListener::bind(addr).map_err(|e| format!("desk link: bind {addr}: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let geometry = Arc::new(Mutex::new(Geometry::default()));
        let input_note = Arc::new(Mutex::new(String::new()));
        let verifier = Arc::new(verifier);
        let (stop2, geometry2, note2) = (stop.clone(), geometry.clone(), input_note.clone());
        let (log, clock, input) = (config.log.clone(), config.clock, config.input.clone());
        let guard = Arc::new(Guard {
            allow: config.allow_peers.iter().copied().map(normalize).collect(),
            failures: Default::default(),
            logged: Default::default(),
            sessions: Default::default(),
            log: config.log.clone(),
        });
        log(&format!("desk link: serves {:?} only", guard.allow));
        std::thread::Builder::new()
            .name("desk-link-listen".into())
            .spawn(move || {
                let next_session = Arc::new(AtomicU32::new(1));
                while !stop2.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((tcp, peer)) => {
                            // before TLS: an address not allowed, or banned, is dropped unread
                            if !guard.allowed(peer.ip()) {
                                guard.refused(peer.ip(), "not in allow_peers");
                                continue;
                            }
                            if guard.banned(peer.ip()) {
                                guard.refused(peer.ip(), "banned after failed credentials");
                                continue;
                            }
                            let Some(slot) = guard.enter(peer.ip()) else {
                                guard.refused(peer.ip(), "too many sessions");
                                continue;
                            };
                            let session = next_session.fetch_add(1, Ordering::AcqRel);
                            let ctx = SessionCtx {
                                session,
                                peer,
                                tls: tls.clone(),
                                verifier: verifier.clone(),
                                tx: tx.clone(),
                                wake: wake.clone(),
                                stop: stop2.clone(),
                                geometry: geometry2.clone(),
                                input_note: note2.clone(),
                                log: log.clone(),
                                clock,
                                input: input.clone(),
                                guard: guard.clone(),
                                _slot: slot,
                            };
                            let spawned = std::thread::Builder::new().name(format!("desk-link-{session}")).spawn(move || ctx.run(tcp));
                            if let Err(e) = spawned {
                                log(&format!("desk link: session thread: {e}"));
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                        Err(e) => {
                            log(&format!("desk link: accept: {e}"));
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Server { addr, fingerprint, stop, rx, geometry, input_note })
    }
}

struct SessionCtx {
    session: u32,
    peer: SocketAddr,
    tls: Arc<TlsServer>,
    verifier: Arc<Verifier>,
    tx: Sender<ToUi>,
    wake: Arc<dyn Fn() + Send + Sync>,
    stop: Arc<AtomicBool>,
    geometry: Arc<Mutex<Geometry>>,
    input_note: Arc<Mutex<String>>,
    log: Log,
    clock: fn() -> u64,
    input: InputFactory,
    guard: Arc<Guard>,
    _slot: SessionSlot,
}

fn read_message(stream: &mut TlsStream, inbox: &mut Inbox, buf: &mut [u8]) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    if let Some(m) = inbox.next()? {
        return Ok(Some(m));
    }
    match stream.read(buf) {
        Ok(0) => Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the viewer closed the connection")),
        Ok(n) => {
            inbox.feed(&buf[..n]);
            inbox.next()
        }
        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => Ok(None),
        Err(e) => Err(e),
    }
}

impl SessionCtx {
    fn send_ui(&self, m: ToUi) {
        let _ = self.tx.send(m);
        (self.wake)();
    }

    fn run(self, tcp: TcpStream) {
        let session = self.session;
        let reason = match self.serve(tcp) {
            Ok(reason) => reason,
            Err(e) => e,
        };
        (self.log)(&format!("desk link session {session} ({}): {reason}", self.peer));
        self.send_ui(ToUi::Closed { session, reason });
    }

    fn serve(&self, tcp: TcpStream) -> Result<String, String> {
        tcp.set_nonblocking(false).map_err(|e| e.to_string())?;
        // the handshake and the hello: a silent peer cannot hold the connection open
        tcp.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        let mut stream = self.tls.accept(tcp).map_err(|e| format!("TLS handshake: {e}"))?;
        let mut inbox = Inbox::default();
        let mut buf = vec![0u8; 256 << 10];
        let started = std::time::Instant::now();
        let (ty, payload) = loop {
            if let Some(m) = read_message(&mut stream, &mut inbox, &mut buf).map_err(|e| e.to_string())? {
                break m;
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err("no hello".into());
            }
        };
        let refuse = |stream: &mut TlsStream, reason: String| -> Result<String, String> {
            let _ = proto::write_all(stream, &ServerMsg::Refused { reason: reason.clone() }.encode());
            stream.shutdown();
            Err(format!("refused: {reason}"))
        };
        let Ok(ClientMsg::Hello { version, name, authorization, screen, paused, caps }) = ClientMsg::decode(ty, &payload) else {
            return refuse(&mut stream, "the first message must be a hello".into());
        };
        if version != proto::VERSION {
            return refuse(&mut stream, format!("protocol version {version}; this desktop speaks {}", proto::VERSION));
        }
        let identity = match self.verifier.verify(&authorization, now_secs()) {
            Ok(identity) => identity,
            Err(e) => {
                if self.guard.fail(self.peer.ip()) {
                    (self.log)(&format!("desk link: {} banned for {} s after failed credentials", normalize(self.peer.ip()), BAN.as_secs()));
                }
                return refuse(&mut stream, format!("fleet credential: {e}"));
            }
        };
        // after authentication the address is checked once more (the same rule as before TLS)
        if !self.guard.allowed(self.peer.ip()) {
            return refuse(&mut stream, "not an allowed address".into());
        }
        if identity.role != Role::Lan {
            return refuse(&mut stream, format!("credential role {} may not view a desktop", identity.role.as_str()));
        }
        // input as real local devices, for this session's life
        let mut injector = match (self.input)() {
            Ok(i) => {
                self.input_note.lock().unwrap().clear();
                Some(i)
            }
            Err(e) => {
                // said in the log and to the viewer (its Welcome): never a silently dead mouse
                (self.log)(&format!("desk link session {}: view only: {e}", self.session));
                *self.input_note.lock().unwrap() = e;
                None
            }
        };
        let (out_tx, out_rx) = channel::<Out>();
        let backlog = Arc::new(AtomicUsize::new(0));
        self.send_ui(ToUi::Opened {
            session: self.session,
            peer: self.peer.to_string(),
            client: identity.client_id.clone(),
            name,
            screen,
            caps,
            paused,
            out: out_tx,
            backlog: backlog.clone(),
        });
        // steady state: short reads, writes in between
        let tcp = stream.try_clone_tcp().map_err(|e| e.to_string())?;
        tcp.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
        loop {
            if self.stop.load(Ordering::Acquire) {
                let _ = proto::write_all(&mut stream, &ServerMsg::Bye { reason: "the desktop stopped serving".into() }.encode());
                return Ok("the WM stopped desk link".into());
            }
            loop {
                match out_rx.try_recv() {
                    Ok(Out::Msg(bytes)) => proto::write_all(&mut stream, &bytes).map_err(|e| format!("write: {e}"))?,
                    Ok(Out::Video(mut bytes)) => {
                        // one picture: its parts' messages back to back, stamped as they go out
                        proto::stamp_videos(&mut bytes, (self.clock)());
                        let result = proto::write_all(&mut stream, &bytes);
                        // caught up: the UI may make pictures again (it holds them back while
                        // a viewer is behind)
                        if backlog.fetch_sub(1, Ordering::AcqRel) == 1 {
                            (self.wake)();
                        }
                        result.map_err(|e| format!("write: {e}"))?;
                    }
                    Ok(Out::Close(reason)) => {
                        let _ = proto::write_all(&mut stream, &ServerMsg::Bye { reason: reason.clone() }.encode());
                        stream.shutdown();
                        return Ok(reason);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok("the WM ended the session".into()),
                }
            }
            while let Some((ty, payload)) = read_message(&mut stream, &mut inbox, &mut buf).map_err(|e| e.to_string())? {
                let msg = ClientMsg::decode(ty, &payload).map_err(|e| e.to_string())?;
                let control = match msg {
                    ClientMsg::Ping { client_ns } => {
                        let pong = ServerMsg::Pong { client_ns, server_ns: (self.clock)() }.encode();
                        proto::write_all(&mut stream, &pong).map_err(|e| format!("write: {e}"))?;
                        None
                    }
                    ClientMsg::Pause => Some(Control::Pause),
                    ClientMsg::Resume => Some(Control::Resume),
                    ClientMsg::SelectScreen { index } => Some(Control::SelectScreen(index)),
                    ClientMsg::RequestKeyframe => Some(Control::Keyframe),
                    ClientMsg::SetSplit { split } => Some(Control::Split(split)),
                    ClientMsg::Refresh { hz } => Some(Control::Refresh(hz)),
                    ClientMsg::Clipboard { text } => Some(Control::Clipboard(text)),
                    ClientMsg::Stats { frame, decode_us, present_us } => Some(Control::Stats { frame, decode_us, present_us }),
                    ClientMsg::Hello { .. } => return Err("a second hello".into()),
                    input => {
                        if let Some(inj) = injector.as_mut() {
                            let geometry = *self.geometry.lock().unwrap();
                            match input {
                                ClientMsg::Pointer { x, y } => {
                                    let (x, y) = geometry.to_desktop(x, y);
                                    inj.pointer(x, y);
                                }
                                ClientMsg::Button { down, button } => inj.button(down, button),
                                ClientMsg::Scroll { dx, dy } => inj.scroll(dx, dy),
                                ClientMsg::Key { down, keycode, .. } => inj.key(down, keycode),
                                ClientMsg::Gamepad { slot, state } => inj.gamepad(slot, &state),
                                ClientMsg::GamepadGone { slot } => inj.gamepad_gone(slot),
                                _ => {}
                            }
                        }
                        None
                    }
                };
                if let Some(control) = control {
                    self.send_ui(ToUi::Control { session: self.session, control });
                }
            }
        }
    }
}
