//! The Desk Link server: the listener and its sessions (threads; the owner's UI thread only
//! drains a channel). Shared by every desktop that serves Desk Link (the Linux window manager,
//! the macOS/Windows desk host); each gives its own input injection ([`Input`]).
//!
//! Security, in the order a connection meets it:
//! 1. nothing listens but on a LAN address, and only with this machine's fleet identity
//!    (`id.key`, `id.cred`, `root.pub` in `fleet::id::dir()`; `Server::start` refuses
//!    otherwise, the AI hub's way; the owner's settings decide whether to start it at all);
//! 2. a peer that is not on the LAN (loopback, RFC 1918, link-local, ULA) is closed unread; so
//!    is a peer whose credential failed again and again (banned a while) and one past the
//!    session caps;
//! 3. TLS with the desktop's fleet certificate (made from its `id.key`, carrying its
//!    credential): viewers check it against the fleet root, nothing is pinned;
//! 4. the viewer's first message carries a fleet proof bound to that certificate (signed by
//!    the root, not expired, not revoked, fresh), and the viewer must be the admin
//!    (`fleet::id::Need::Admin`: the admin Mac's device credential, connecting from the address
//!    bound into it), or the connection is refused with the reason and closed.
//!
//! One thread per session owns its TLS stream (TLS contexts are not safe for concurrent read
//! and write): it reads with a 1 ms timeout and writes what the UI queued in between. Input is
//! injected right there (`inject`), never through the UI thread.

use crate::{self as proto, ClientMsg, GamepadState, Inbox, ServerMsg};
use makepad_network::fleet::id::{self, now_secs, Need, Verifier};
use std::net::IpAddr;
use makepad_network::tls::{TlsIdentity, TlsServer, TlsStream};
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
mod a11y;

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

/// A session's input devices and what the viewer holds down on them. When the session ends
/// (the viewer closed, its connection dropped or went silent, the desktop stopped) every key
/// and button it still holds goes up: the one place every desktop (the WM, the Desk Hosts)
/// lets go, so nothing stays pressed after a viewer is gone. An up whose down never came is
/// not passed on: the devices see each key go down and up in turn.
pub struct HeldInput {
    pub input: Box<dyn Input>,
    keys: Vec<u32>,
    buttons: u8,
}

impl HeldInput {
    pub fn new(input: Box<dyn Input>) -> HeldInput {
        HeldInput { input, keys: Vec::new(), buttons: 0 }
    }

    pub fn key(&mut self, down: bool, keycode: u32) {
        let held = self.keys.contains(&keycode);
        if down {
            if !held {
                self.keys.push(keycode);
            }
        } else if held {
            self.keys.retain(|k| *k != keycode);
        } else {
            return;
        }
        self.input.key(down, keycode);
    }

    pub fn button(&mut self, down: bool, button: u8) {
        if button >= 8 {
            return;
        }
        let held = self.buttons & (1 << button) != 0;
        if down {
            self.buttons |= 1 << button;
        } else if held {
            self.buttons &= !(1 << button);
        } else {
            return;
        }
        self.input.button(down, button);
    }

    /// Everything held goes up: buttons, then keys newest first (modifiers went down first, so
    /// they go up last).
    pub fn release(&mut self) {
        for b in 0..8u8 {
            if self.buttons & (1 << b) != 0 {
                self.input.button(false, b);
            }
        }
        self.buttons = 0;
        for k in std::mem::take(&mut self.keys).into_iter().rev() {
            self.input.key(false, k);
        }
    }
}

impl Drop for HeldInput {
    fn drop(&mut self) {
        self.release();
    }
}

/// A viewer pings every second: one silent this long is gone (its machine slept, its link
/// dropped without a close), and its session ends: what it held goes up. The viewer gives up
/// on a silent desktop after the same time.
const SILENT: Duration = Duration::from_secs(6);

/// Makes a session's input devices (`Err`: why the session is view-only).
pub type InputFactory = Arc<dyn Fn() -> Result<Box<dyn Input>, String> + Send + Sync>;
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

pub struct ServerConfig {
    pub addr: SocketAddr,
    /// The certificate's common name (the certificate itself is made from this machine's
    /// fleet key and carries its credential: [`fleet_identity`]).
    pub tls_name: String,
    pub log: Log,
    /// The desktop's clock (ns): `Pong`, `Video.sent_ns` (and the owner's `built_ns`).
    pub clock: fn() -> u64,
    pub input: InputFactory,
}

/// What the sessions tell the UI thread.
pub enum ToUi {
    Opened { session: u32, peer: String, client: String, name: String, screen: i32, paused: bool, caps: u32, out: Sender<Out>, feed: Arc<Feed> },
    Control { session: u32, control: Control },
    Closed { session: u32, reason: String },
}

pub enum Control {
    Pause,
    Resume,
    SelectScreen(u32),
    Keyframe,
    /// The viewer's display refresh (Hz, 0 = not known).
    Refresh(u16),
    /// The viewer's picture area: device pixels and scale (hundredths).
    Window { width: u32, height: u32, scale: u16 },
    Clipboard(String),
    Stats { frame: u32, decode_us: u32, present_us: u32 },
    /// The viewer's encoding choices (`pref::*`, cap Mbit/s, pictures a second; 0 = auto each).
    Encoding { chroma: u8, codec: u8, preset: u8, mbps: u16, fps: u16 },
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

/// What the sessions wrote (all sessions together), read and reset by the owner's per-second
/// timeline: pictures, bytes, the longest single picture write (µs).
#[derive(Default)]
pub struct SendStats {
    pub pictures: AtomicU64,
    pub bytes: AtomicU64,
    pub write_us_max: AtomicU64,
}

/// More pictures than this queued for a watching viewer and not written: it is behind.
pub const BACKLOG: usize = 2;
/// A viewer that fell behind starts again at a key frame no sooner than this after its last one
/// (a slow link costs the others a key frame at most this often).
const RESYNC_EVERY: Duration = Duration::from_secs(2);

/// One viewer's pictures (shared by its owner, which queues them, and its session, which writes
/// them): every viewer of a desktop gets the same capture and encode, at the pace its own link
/// takes. A viewer behind while another keeps up is let go of the stream (it skips pictures) and
/// starts again at a key frame once it has caught up, so one slow link never holds the others
/// back; a viewer behind with nobody keeping up holds the capture (`hold`) as before.
#[derive(Default)]
pub struct Feed {
    /// Pictures queued and not written yet.
    pub backlog: AtomicUsize,
    /// It asked for no pictures (`Pause`).
    pub paused: AtomicBool,
    state: Mutex<FeedState>,
}

#[derive(Default)]
struct FeedState {
    /// Let go of the stream: no pictures until a key frame after it caught up.
    skipping: bool,
    /// The picture last decided and whether it goes here (its parts all go, or none).
    decided: Option<(u32, bool)>,
    /// When its last resync key frame was asked for.
    resync_at: Option<Instant>,
}

impl Feed {
    pub fn new(paused: bool) -> Feed {
        Feed { paused: AtomicBool::new(paused), ..Default::default() }
    }

    pub fn watching(&self) -> bool {
        !self.paused.load(Ordering::Acquire)
    }

    pub fn behind(&self) -> bool {
        self.backlog.load(Ordering::Acquire) > BACKLOG
    }

    /// Let go of the stream (skipping pictures until it starts again at a key frame).
    pub fn skipping(&self) -> bool {
        self.state.lock().unwrap().skipping
    }

    /// Whether picture `frame` (a key frame or not) goes to this viewer; every part of one
    /// picture gets the same answer. A paused viewer gets none; a skipping one only a key
    /// frame, once its queue is empty (then it follows the stream again).
    pub fn admit(&self, frame: u32, key: bool) -> bool {
        if !self.watching() {
            return false;
        }
        let mut s = self.state.lock().unwrap();
        if let Some((f, yes)) = s.decided {
            if f == frame {
                return yes;
            }
        }
        if s.skipping && key && self.backlog.load(Ordering::Acquire) == 0 {
            s.skipping = false;
        }
        let yes = !s.skipping;
        s.decided = Some((frame, yes));
        yes
    }
}

/// Whether the capture holds its next picture: a watching viewer that follows the stream is
/// behind and nobody else keeps up. A viewer behind while another watching one keeps up is let
/// go of the stream instead (`Feed::admit` skips its pictures), and the capture goes on.
pub fn hold<'a>(feeds: impl IntoIterator<Item = &'a Feed> + Clone) -> bool {
    let keeps_up = feeds.clone().into_iter().any(|f| f.watching() && !f.behind() && !f.skipping());
    let mut hold = false;
    for f in feeds {
        if !f.watching() || !f.behind() {
            continue;
        }
        let mut s = f.state.lock().unwrap();
        if s.skipping {
            continue;
        }
        if keeps_up {
            s.skipping = true;
        } else {
            hold = true;
        }
    }
    hold
}

/// Whether a key frame is due for a viewer that was let go of the stream and has caught up
/// (at most every `RESYNC_EVERY` per viewer). Asking marks it asked.
pub fn resync_due<'a>(feeds: impl IntoIterator<Item = &'a Feed>) -> bool {
    let mut due = false;
    for f in feeds {
        if !f.watching() || f.backlog.load(Ordering::Acquire) != 0 {
            continue;
        }
        let mut s = f.state.lock().unwrap();
        if s.skipping && s.resync_at.is_none_or(|t| t.elapsed() >= RESYNC_EVERY) {
            s.resync_at = Some(Instant::now());
            due = true;
        }
    }
    due
}

/// The viewer whose window decides the size of a desktop that follows its viewer: the last
/// connection wins. Of the viewers that said their window and may decide (not view-only), the
/// one that connected last (sessions count up); when it leaves, the one before it decides
/// again. `viewers`: (session, may decide).
pub fn size_owner(viewers: impl IntoIterator<Item = (u32, bool)>) -> Option<u32> {
    viewers.into_iter().filter(|v| v.1).map(|v| v.0).max()
}

pub struct Server {
    pub addr: SocketAddr,
    /// This machine's fleet credential name (what viewers see it as).
    pub name: String,
    stop: Arc<AtomicBool>,
    pub rx: Receiver<ToUi>,
    pub geometry: Arc<Mutex<Geometry>>,
    /// Why viewer input is not injected, when it is not (no /dev/uinput access).
    pub input_note: Arc<Mutex<String>>,
    pub sent: Arc<SendStats>,
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
    failures: Mutex<std::collections::HashMap<IpAddr, (Vec<Instant>, Option<Instant>)>>,
    /// Per refused address: when it was last logged and how many refusals since.
    logged: Mutex<std::collections::HashMap<IpAddr, (Instant, u32)>>,
    sessions: Mutex<std::collections::HashMap<IpAddr, usize>>,
    log: Log,
}

impl Guard {
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

/// This machine's fleet identity as a server: its TLS identity (certificate `tls_name`, made
/// from `id.key`, carrying `id.cred`) and the check of viewer proofs against its fleet
/// (`root.pub`, `revoked`), read from [`id::dir`] now (not the process's cached
/// [`id::me`]: a desktop waiting for its identity sees it once installed).
pub fn fleet_identity(tls_name: &str) -> Result<(TlsIdentity, Verifier, String), String> {
    let dir = id::dir();
    let me = id::Me::load(&dir).map_err(|e| format!("has no fleet identity ({e})"))?;
    let fleet = id::Fleet::load(&dir).map_err(|e| format!("has no fleet root ({e})"))?;
    let identity = me.tls_identity(tls_name).map_err(|e| format!("cannot make its TLS identity ({e})"))?;
    let verifier = Verifier::new(fleet, identity.fingerprint, &me.cred);
    Ok((identity, verifier, me.cred.name.clone()))
}

fn refuse(addr: &str, why: String) -> String {
    format!("desk link: listens on {addr} but {why}; a network-reachable desktop serves only to fleet credentials (tools/fleet-identity.md)")
}

impl Server {
    /// Starts listening, or says why it does not: not a LAN address, no fleet identity
    /// ([`fleet_identity`]). `wake` makes the owner's UI thread look at `rx`.
    pub fn start(config: ServerConfig, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Server, String> {
        let addr = config.addr;
        if !bindable(addr.ip()) {
            return Err(format!("desk link: {} is not a LAN address (loopback, 10/8, 172.16/12, 192.168/16, link-local, fc00::/7)", addr.ip()));
        }
        let text = addr.to_string();
        let (identity, verifier, name) = fleet_identity(&config.tls_name).map_err(|why| refuse(&text, why))?;
        let tls = Arc::new(TlsServer::new(identity).map_err(|e| format!("desk link: TLS server: {e}"))?);
        let listener = TcpListener::bind(addr).map_err(|e| format!("desk link: bind {addr}: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let geometry = Arc::new(Mutex::new(Geometry::default()));
        let input_note = Arc::new(Mutex::new(String::new()));
        let verifier = Arc::new(verifier);
        let sent = Arc::new(SendStats::default());
        let (stop2, geometry2, note2, sent2) = (stop.clone(), geometry.clone(), input_note.clone(), sent.clone());
        let (log, clock, input) = (config.log.clone(), config.clock, config.input.clone());
        let guard = Arc::new(Guard {
            failures: Default::default(),
            logged: Default::default(),
            sessions: Default::default(),
            log: config.log.clone(),
        });
        log(&format!("desk link: serves the fleet admin only, as {name}"));
        std::thread::Builder::new()
            .name("desk-link-listen".into())
            .spawn(move || {
                let next_session = Arc::new(AtomicU32::new(1));
                while !stop2.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((tcp, peer)) => {
                            // before TLS: an address off the LAN, or banned, is dropped unread
                            if !lan_address(peer.ip()) {
                                guard.refused(peer.ip(), "not a LAN address");
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
                                sent: sent2.clone(),
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
        Ok(Server { addr, name, stop, rx, geometry, input_note, sent })
    }
}

/// `MAKEPAD_DESK_LINK_TRACE=1`: every message written is logged (type, size, result).
fn trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MAKEPAD_DESK_LINK_TRACE").is_some_and(|v| v == "1"))
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
    sent: Arc<SendStats>,
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
        let peer = match self.verifier.check(&authorization, self.peer.ip(), now_secs()) {
            Ok(peer) => peer,
            Err(e) => {
                if self.guard.fail(self.peer.ip()) {
                    (self.log)(&format!("desk link: {} banned for {} s after failed credentials", normalize(self.peer.ip()), BAN.as_secs()));
                }
                return refuse(&mut stream, format!("fleet credential: {e}"));
            }
        };
        // viewing a desktop is an admin right: the admin Mac's credential from its own address
        if let Err(why) = id::require(&peer, Need::Admin) {
            return refuse(&mut stream, format!("may not view this desktop: {why}"));
        }
        // input as real local devices, for this session's life
        let mut injector = match (self.input)() {
            Ok(i) => {
                self.input_note.lock().unwrap().clear();
                Some(HeldInput::new(i))
            }
            Err(e) => {
                // said in the log and to the viewer (its Welcome): never a silently dead mouse
                (self.log)(&format!("desk link session {}: view only: {e}", self.session));
                *self.input_note.lock().unwrap() = e;
                None
            }
        };
        let (out_tx, out_rx) = channel::<Out>();
        // Created only after the same Need::Admin gate as input injection.
        let mut semantic = a11y::Session::new(self.session, out_tx.clone());
        let feed = Arc::new(Feed::new(paused));
        self.send_ui(ToUi::Opened {
            session: self.session,
            peer: self.peer.to_string(),
            client: peer.name.clone(),
            name,
            screen,
            caps,
            paused,
            out: out_tx,
            feed: feed.clone(),
        });
        // steady state: short reads, writes in between. Set on the TLS stream's own socket: on
        // Windows a duplicated handle's SO_RCVTIMEO does not reach the stream's reads, which then
        // block until the viewer sends (its 1 s ping), and nothing queued was ever written.
        stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
        let mut last_heard = Instant::now();
        loop {
            if last_heard.elapsed() > SILENT {
                return Ok(format!("the viewer was silent for {} s", SILENT.as_secs()));
            }
            if self.stop.load(Ordering::Acquire) {
                let _ = proto::write_all(&mut stream, &ServerMsg::Bye { reason: "the desktop stopped serving".into() }.encode());
                return Ok("the WM stopped desk link".into());
            }
            loop {
                match out_rx.try_recv() {
                    Ok(Out::Msg(bytes)) => {
                        let r = proto::write_all(&mut stream, &bytes);
                        if trace() {
                            (self.log)(&format!("desk link session {}: message type {} ({} bytes) written: {r:?}", self.session, bytes.first().copied().unwrap_or(0), bytes.len()));
                        }
                        r.map_err(|e| format!("write: {e}"))?
                    }
                    Ok(Out::Video(mut bytes)) => {
                        // one picture: its parts' messages back to back, stamped as they go out
                        proto::stamp_videos(&mut bytes, (self.clock)());
                        let started = std::time::Instant::now();
                        let result = proto::write_all(&mut stream, &bytes);
                        self.sent.pictures.fetch_add(1, Ordering::Relaxed);
                        self.sent.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                        self.sent.write_us_max.fetch_max(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                        // caught up: the UI may make pictures again (it holds them back while
                        // a viewer is behind)
                        if feed.backlog.fetch_sub(1, Ordering::AcqRel) == 1 {
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
                last_heard = Instant::now();
                let control = match msg {
                    ClientMsg::Ping { client_ns } => {
                        let pong = ServerMsg::Pong { client_ns, server_ns: (self.clock)() }.encode();
                        proto::write_all(&mut stream, &pong).map_err(|e| format!("write: {e}"))?;
                        None
                    }
                    ClientMsg::A11yQuery { req, line } => {
                        semantic.submit(req, line);
                        None
                    }
                    ClientMsg::Pause => Some(Control::Pause),
                    ClientMsg::Resume => Some(Control::Resume),
                    ClientMsg::SelectScreen { index } => Some(Control::SelectScreen(index)),
                    ClientMsg::RequestKeyframe => Some(Control::Keyframe),
                    ClientMsg::Refresh { hz } => Some(Control::Refresh(hz)),
                    ClientMsg::Window { width, height, scale } => Some(Control::Window { width, height, scale }),
                    ClientMsg::Clipboard { text } => Some(Control::Clipboard(text)),
                    ClientMsg::Stats { frame, decode_us, present_us } => Some(Control::Stats { frame, decode_us, present_us }),
                    ClientMsg::Encoding { chroma, codec, preset, mbps, fps } => Some(Control::Encoding { chroma, codec, preset, mbps, fps }),
                    ClientMsg::Hello { .. } => return Err("a second hello".into()),
                    input => {
                        if let Some(inj) = injector.as_mut() {
                            let geometry = *self.geometry.lock().unwrap();
                            match input {
                                ClientMsg::Pointer { x, y } => {
                                    let (x, y) = geometry.to_desktop(x, y);
                                    inj.input.pointer(x, y);
                                }
                                ClientMsg::Button { down, button } => inj.button(down, button),
                                ClientMsg::Scroll { dx, dy } => inj.input.scroll(dx, dy),
                                ClientMsg::Key { down, keycode, .. } => inj.key(down, keycode),
                                ClientMsg::Gamepad { slot, state } => inj.input.gamepad(slot, &state),
                                ClientMsg::GamepadGone { slot } => inj.input.gamepad_gone(slot),
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

#[cfg(test)]
mod feed_tests {
    use super::*;

    fn queued(f: &Feed, n: usize) {
        f.backlog.store(n, Ordering::Release);
    }

    #[test]
    fn one_slow_viewer_alone_holds_the_capture() {
        let a = Feed::new(false);
        queued(&a, BACKLOG + 1);
        assert!(hold([&a]));
        assert!(!a.skipping());
    }

    #[test]
    fn a_slow_viewer_is_let_go_while_another_keeps_up_and_comes_back_at_a_key_frame() {
        let (fast, slow) = (Feed::new(false), Feed::new(false));
        queued(&slow, BACKLOG + 1);
        assert!(!hold([&fast, &slow]), "the fast viewer keeps the capture going");
        assert!(slow.skipping());
        assert!(fast.admit(1, false) && !slow.admit(1, false));
        // a key frame while the slow one still has pictures queued: not yet
        assert!(!slow.admit(2, true));
        assert!(!resync_due([&fast, &slow]));
        queued(&slow, 0);
        assert!(resync_due([&fast, &slow]));
        assert!(!resync_due([&fast, &slow]), "asked once per RESYNC_EVERY");
        assert!(!slow.admit(3, false));
        assert!(slow.admit(4, true) && !slow.skipping());
        assert!(slow.admit(5, false));
    }

    #[test]
    fn every_part_of_a_picture_gets_the_same_answer() {
        let (fast, slow) = (Feed::new(false), Feed::new(false));
        assert!(slow.admit(7, false));
        queued(&slow, BACKLOG + 1);
        hold([&fast, &slow]);
        assert!(slow.admit(7, false), "the rest of picture 7 still goes");
        assert!(!slow.admit(8, false));
    }

    #[test]
    fn a_paused_viewer_gets_nothing_and_holds_nothing() {
        let (a, b) = (Feed::new(true), Feed::new(false));
        queued(&a, BACKLOG + 5);
        assert!(!a.admit(1, true));
        assert!(!hold([&a, &b]));
    }

    #[test]
    fn the_last_connection_decides_the_size_and_the_one_before_takes_over() {
        assert_eq!(size_owner([(3, true), (5, true), (4, true)]), Some(5));
        // view-only (a probe) or no window said yet: never
        assert_eq!(size_owner([(3, true), (5, false)]), Some(3));
        assert_eq!(size_owner([(5, false)]), None);
        // the newest left: the one before it again
        assert_eq!(size_owner([(3, true), (4, true)]), Some(4));
    }
}
