//! One Desk Link connection: a network thread (TLS, the fleet proof, the protocol, reconnects)
//! and a decode thread (the platform's hardware H.264 / H.265 decoder). The UI reads its state and the
//! newest decoded picture; it never waits on either thread.
//!
//! A connection the viewer is not showing stays open, paused: the desktop makes no pictures
//! for it, and its pings keep the side panel's latency live. Showing it again is a `Resume`,
//! answered by a key frame: one round trip and one encode, no TLS handshake.

use makepad_desk_link::{self as proto, ClientMsg, Inbox, ScreenInfo, ServerMsg};
use makepad_network::SignalToUI;
use makepad_widgets::makepad_platform::video_file::{DecodedPicture, StreamVideoCodec, VideoColor, VideoStreamDecoder};
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use crate::history::{HistPacket, History, CHANGE_BYTES, KEY_EVERY};

/// This process's clock (ns): the viewer side of every latency figure.
pub fn now_ns() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Connecting,
    Connected,
    /// A session that was up broke (a TLS error, a reset): connecting again at once, the last
    /// picture stays; the reason goes to the log only.
    Reconnecting,
    Offline(String),
}

#[derive(Clone, Debug, Default)]
pub struct Welcome {
    pub screens: Vec<ScreenInfo>,
    pub screen: u32,
    pub stream: [u32; 2],
    pub input: bool,
    pub input_note: String,
    pub server: String,
}

/// How far back the clock offset looks for its smallest round trip.
const OFFSET_WINDOW_NS: u64 = 20_000_000_000;

/// Latency of the newest picture, in ms (server times converted with the ping clock offset).
#[derive(Clone, Copy, Debug, Default)]
pub struct Latency {
    pub encode: f64,
    pub network: f64,
    pub decode: f64,
    pub present: f64,
    pub total: f64,
}

/// This display's refresh (Hz; 0 = not measured yet): every connection tells its desktop.
pub static DISPLAY_HZ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// This machine's H.265 decode is unusable: in software (its hardware decoder does not take
/// that profile) or failing on a key frame. `auto` asks for H.264 from then on.
pub static HEVC_UNUSABLE: AtomicBool = AtomicBool::new(false);

/// The codec this viewer asks for: the desktop's choice (auto), H.264 once H.265 proved
/// unusable here ([`HEVC_UNUSABLE`]).
fn codec_wanted() -> u8 {
    if HEVC_UNUSABLE.load(Ordering::Acquire) { proto::pref::H264 } else { proto::pref::AUTO }
}

/// What this viewer's decoders take (`Hello::caps`): Apple's VideoToolbox decodes H.264 High
/// 4:4:4 Predictive and H.265 (Main, and Format range extensions 4:4:4) on its hardware decoder.
fn decoder_caps() -> u32 {
    if cfg!(target_vendor = "apple") { proto::caps::FULL_CHROMA | proto::caps::HEVC } else { 0 }
}

/// The desktop's `Rate`: how it streams now.
#[derive(Clone, Debug, Default)]
pub struct StreamRate {
    /// The stream's rate (kbit/s, all its tiles).
    pub kbps: u32,
    /// What holds it there (`limit::*`).
    pub limit: u8,
    pub full_chroma: bool,
    /// `pref::H264` / `pref::HEVC`.
    pub codec: u8,
    /// `pref::SHARP` / `pref::FAST`.
    pub preset: u8,
    /// The encoder streams a picture is cut into.
    pub streams: u8,
    /// What the stream is and why, in words.
    pub why: String,
}

impl StreamRate {
    pub fn codec_name(&self) -> &'static str {
        if self.codec == proto::pref::HEVC { "H.265" } else { "H.264" }
    }
}

/// A window's picture area in device pixels and its scale (hundredths), as the view last drew
/// it: each connection has its own (the window that shows it sets it), told to the desktop once
/// it has held still (`WINDOW_SETTLE`).
#[derive(Default)]
pub struct ViewWindow {
    /// width << 32 | height
    size: std::sync::atomic::AtomicU64,
    scale: std::sync::atomic::AtomicU32,
}

impl ViewWindow {
    pub fn set(&self, width: u32, height: u32, scale: u32) {
        self.size.store((width as u64) << 32 | height as u64, Ordering::Release);
        self.scale.store(scale, Ordering::Release);
    }

    fn get(&self) -> (u64, u32) {
        (self.size.load(Ordering::Acquire), self.scale.load(Ordering::Acquire))
    }
}

/// A window size is told once it held this long (a live resize sends only where it ends).
const WINDOW_SETTLE: Duration = Duration::from_millis(250);
/// A window size that has not held still this long after it changed is reported in the log.
const WINDOW_LATEST: Duration = Duration::from_millis(1000);

#[derive(Clone, Debug, Default)]
pub struct Shared {
    pub state: Option<State>,
    pub welcome: Option<Welcome>,
    /// Counts the sessions (each `Welcome`): input held on an ended one was let go by the
    /// desktop.
    pub session: u64,
    /// Round trip of the last ping (ms).
    pub rtt_ms: Option<f64>,
    /// server clock - viewer clock (ns): the smallest-RTT ping of the last `OFFSET_WINDOW`.
    pub offset_ns: i64,
    /// Recent pongs (received at, rtt, offset), for `offset_ns`.
    pongs: std::collections::VecDeque<(u64, u64, i64)>,
    pub cursor: Option<(u16, u16, u8, bool)>,
    /// Pictures and bytes per second, over the last second.
    pub fps: f64,
    pub mbps: f64,
    /// The pictures a second the desktop streams at (its `Rate`; a still desktop sends fewer).
    pub stream_fps: Option<u16>,
    /// How the desktop streams now (its `Rate`).
    pub stream_rate: Option<StreamRate>,
    pub warning: Option<String>,
    /// The desktop's fleet credential name, proven by its certificate (`fleet::id::connect`).
    pub machine: Option<String>,
    /// Over the last second: the longest time the network thread did not read (ms; a stall
    /// here fills the desktop's socket and holds its pictures back), and the most pictures
    /// waiting for the decoder (decoding slower than they arrive).
    pub read_gap_ms: f64,
    pub decode_queue: usize,
    /// Pictures decoded but never shown (see `Assembly::dropped`), counted up by the decoders;
    /// the view reads and resets it once a second.
    pub dropped: u32,
}

/// A decoded part of a picture: its pixels from column `x0`, row `y0` on (all of it, or one
/// tile), the decoder's GPU picture where the platform has one (no CPU copy), else tight NV12.
pub struct Part {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
    pub gpu: Option<DecodedPicture>,
    /// Its colour as its stream says (drawn with exactly that).
    pub color: VideoColor,
}

/// A decoded picture: all of its parts (its tiles, shown together), `width` x `height`.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub parts: Vec<Part>,
    pub frame: u32,
    pub built_ns: u64,
    pub encoded_ns: u64,
    pub sent_ns: u64,
    pub received_ns: u64,
    pub decoded_ns: u64,
    pub decode_ns: u64,
}

struct Packet {
    frame: u32,
    key: bool,
    stream: u8,
    parts: u8,
    x0: u32,
    y0: u32,
    /// `pref::H264` / `pref::HEVC`.
    codec: u8,
    built_ns: u64,
    encoded_ns: u64,
    sent_ns: u64,
    received_ns: u64,
    /// Shared with the history (no copy).
    data: Arc<Vec<u8>>,
    /// Its place in the history (`History::push`), for the decode's numbers.
    seq: u64,
}

pub struct Connection {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub shared: Arc<Mutex<Shared>>,
    pub latest: Arc<Mutex<Option<Picture>>>,
    /// The received bitstream of the last seconds (always recorded).
    pub history: Arc<Mutex<History>>,
    /// The size of the window that shows it (its desktop follows it).
    pub view: Arc<ViewWindow>,
    tx: Sender<ClientMsg>,
    semantic: Arc<crate::a11y::Endpoint>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    /// Drop the session and open it again now (`Connection::reconnect`).
    reconnect: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Connection {
    /// The network thread (and its decoder) ends within its connect / handshake bound and is
    /// joined: nothing outlives the connection.
    fn drop(&mut self) {
        self.semantic.connected(false);
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Connection {
    /// Opens (and keeps reopening) the connection; `paused` until [`Connection::show`].
    pub fn open(name: String, host: String, port: u16, paused: bool, signal: SignalToUI) -> Connection {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let latest = Arc::new(Mutex::new(None));
        let history = Arc::new(Mutex::new(History::default()));
        let (tx, rx) = channel();
        let paused = Arc::new(AtomicBool::new(paused));
        let stop = Arc::new(AtomicBool::new(false));
        let reconnect = Arc::new(AtomicBool::new(false));
        let max_backoff = Duration::from_secs(30);
        let view = Arc::new(ViewWindow::default());
        let semantic = crate::a11y::register(&name, tx.clone());
        let net = Net { semantic: semantic.clone(), host: host.clone(), port, shared: shared.clone(), latest: latest.clone(), history: history.clone(), view: view.clone(), rx, paused: paused.clone(), stop: stop.clone(), reconnect: reconnect.clone(), signal, cmd: tx.clone(), max_backoff };
        let thread = match std::thread::Builder::new().name(format!("desklink-net-{host}")).spawn(move || net.run()) {
            Ok(t) => Some(t),
            Err(e) => {
                shared.lock().unwrap().state = Some(State::Offline(format!("thread: {e}")));
                None
            }
        };
        Connection { name, host, port, shared, latest, history, view, tx, semantic, paused, stop, reconnect, thread }
    }

    /// A desktop played from an Annex-B dump (dev: `--dump-play`): its pictures decoded the way
    /// a live stream's are (`gpu`: the decoder's GPU pictures, else CPU NV12), at 60 per
    /// second, over and over.
    /// `still`: stop at that picture and keep showing it (comparable grabs).
    pub fn play_dump(path: String, gpu: bool, still: Option<u32>, signal: SignalToUI) -> Connection {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let latest = Arc::new(Mutex::new(None));
        let history = Arc::new(Mutex::new(History::default()));
        let (tx, _rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        shared.lock().unwrap().state = Some(State::Connected);
        let (s, l, h, st) = (shared.clone(), latest.clone(), history.clone(), stop.clone());
        let thread = std::thread::Builder::new().name("desklink-dump".into()).spawn(move || play_loop(path, gpu, still, s, l, h, st, signal)).ok();
        let semantic = crate::a11y::register("dump", tx.clone());
        Connection { semantic, name: "dump".into(), host: "dump".into(), port: 0, shared, latest, history, view: Arc::new(ViewWindow::default()), tx, paused: Arc::new(AtomicBool::new(false)), stop, reconnect: Arc::new(AtomicBool::new(false)), thread }
    }

    pub fn send(&self, msg: ClientMsg) {
        let _ = self.tx.send(msg);
    }

    /// Drops the session (if any) and opens it again at once, skipping any backoff wait (a
    /// desktop that was changed or restarted). The history stays.
    pub fn reconnect(&self) {
        self.reconnect.store(true, Ordering::Release);
    }

    pub fn show(&self, on: bool) {
        if self.paused.swap(!on, Ordering::AcqRel) == on {
            self.send(if on { ClientMsg::Resume } else { ClientMsg::Pause });
        }
    }

    /// The current session (`Shared::session`).
    pub fn session(&self) -> u64 {
        self.shared.lock().unwrap().session
    }

    pub fn state(&self) -> State {
        self.shared.lock().unwrap().state.clone().unwrap_or(State::Connecting)
    }
}

struct Net {
    semantic: Arc<crate::a11y::Endpoint>,
    host: String,
    port: u16,
    shared: Arc<Mutex<Shared>>,
    latest: Arc<Mutex<Option<Picture>>>,
    history: Arc<Mutex<History>>,
    view: Arc<ViewWindow>,
    rx: Receiver<ClientMsg>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    reconnect: Arc<AtomicBool>,
    signal: SignalToUI,
    cmd: Sender<ClientMsg>,
    max_backoff: Duration,
}

impl Net {
    fn set_state(&self, state: State) {
        self.shared.lock().unwrap().state = Some(state);
        self.signal.set();
    }

    fn run(self) {
        let mut backoff = Duration::from_secs(2);
        // quick reconnects left after a session that was up broke
        let mut quick = 0u32;
        while !self.stop.load(Ordering::Acquire) {
            // a reconnect asked while offline (or now) is this attempt
            self.reconnect.store(false, Ordering::Release);
            if quick == 0 {
                self.set_state(State::Connecting);
            }
            let started = Instant::now();
            let reason = match self.session() {
                Ok(reason) => reason,
                Err(e) => e,
            };
            self.semantic.connected(false);
            if self.stop.load(Ordering::Acquire) {
                return;
            }
            if self.reconnect.swap(false, Ordering::AcqRel) {
                backoff = Duration::from_secs(2);
                quick = 0;
                continue;
            }
            let was_up = matches!(self.shared.lock().unwrap().state, Some(State::Connected));
            let refused = reason.starts_with("refused");
            if was_up && !refused {
                quick = 5;
            }
            if quick > 0 {
                // silently: a short status, the reason in the log, a new try in 200 ms
                quick -= 1;
                eprintln!("desklink: {}:{}: {reason}; reconnecting", self.host, self.port);
                self.set_state(State::Reconnecting);
                std::thread::sleep(Duration::from_millis(200));
                while self.rx.try_recv().is_ok() {}
                continue;
            }
            self.set_state(State::Offline(reason.clone()));
            // a session that ran a while starts the backoff over
            if started.elapsed() > Duration::from_secs(30) {
                backoff = Duration::from_secs(2);
            }
            let wait = if refused { Duration::from_secs(30) } else { backoff };
            let until = Instant::now() + wait;
            while Instant::now() < until && !self.stop.load(Ordering::Acquire) && !self.reconnect.load(Ordering::Acquire) {
                // commands while offline are void (input for a desktop that is not there)
                while self.rx.try_recv().is_ok() {}
                std::thread::sleep(Duration::from_millis(100));
            }
            backoff = (backoff * 2).min(self.max_backoff);
        }
    }

    fn session(&self) -> Result<String, String> {
        // the desktop proves itself with its fleet certificate (checked against the fleet root);
        // the hello carries this machine's proof bound to that certificate
        let (mut stream, server, authorization) = makepad_network::fleet::id::connect(&self.host, &self.port.to_string()).map_err(|e| format!("connect: {e}"))?;
        self.shared.lock().unwrap().machine = Some(server.name);
        let hello = ClientMsg::Hello {
            version: proto::VERSION,
            name: format!("desklink {}", std::env::var("USER").unwrap_or_default()),
            authorization,
            screen: -1,
            paused: self.paused.load(Ordering::Acquire),
            caps: decoder_caps(),
        };
        proto::write_all(&mut stream, &hello.encode()).map_err(|e| format!("hello: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;

        // the decoder: its own thread, fed every picture in order
        let (dec_tx, dec_rx) = channel::<Packet>();
        // packets sent to the decoder and not decoded yet
        let queued = Arc::new(AtomicUsize::new(0));
        self.history.lock().unwrap().session_start();
        let decoder = {
            let (latest, signal, shared, cmd, queued, history) = (self.latest.clone(), self.signal.clone(), self.shared.clone(), self.cmd.clone(), queued.clone(), self.history.clone());
            std::thread::Builder::new().name(format!("desklink-dec-{}", self.host)).spawn(move || decode_loop(dec_rx, latest, signal, shared, cmd, queued, history, true)).map_err(|e| e.to_string())?
        };
        let (mut read_gap, mut queue_max, mut last_read) = (Duration::ZERO, 0usize, Instant::now());

        let mut inbox = Inbox::default();
        let mut buf = vec![0u8; 512 << 10];
        let mut last_ping = Instant::now() - Duration::from_secs(10);
        let mut second = (Instant::now(), 0u32, 0usize);
        let mut last_heard = Instant::now();
        // the display refresh last told to the desktop
        let mut told_hz = 0u32;
        // the Encoding last told (rate cap, codec): sent once at the start, then on changes
        let mut told_encoding: Option<u8> = None;
        // the window last told, the one seen since when, and since when the window differs from
        // the one told (a size that keeps changing is told after WINDOW_LATEST anyway)
        let (mut told_window, mut seen_window) = ((0u64, 0u32), ((0u64, 0u32), Instant::now()));
        let mut differs_since: Option<Instant> = None;
        let result = loop {
            if self.stop.load(Ordering::Acquire) {
                // what was sent before the close still goes out (the ups of held keys at quit)
                while let Ok(msg) = self.rx.try_recv() {
                    if proto::write_all(&mut stream, &msg.encode()).is_err() {
                        break;
                    }
                }
                break Ok("closed".into());
            }
            if self.reconnect.load(Ordering::Acquire) {
                break Ok("reconnecting".into());
            }
            let hz = DISPLAY_HZ.load(Ordering::Acquire);
            if hz != told_hz {
                told_hz = hz;
                if let Err(e) = proto::write_all(&mut stream, &ClientMsg::Refresh { hz: hz.min(u16::MAX as u32) as u16 }.encode()) {
                    break Err(format!("write: {e}"));
                }
            }
            // the desktop's choices (auto) except a codec this machine cannot decode
            let codec = codec_wanted();
            if told_encoding != Some(codec) {
                told_encoding = Some(codec);
                let msg = ClientMsg::Encoding { chroma: proto::pref::AUTO, codec, preset: proto::pref::AUTO, mbps: 0, fps: 0 };
                if let Err(e) = proto::write_all(&mut stream, &msg.encode()) {
                    break Err(format!("write: {e}"));
                }
            }
            let window = self.view.get();
            if window != told_window {
                differs_since.get_or_insert_with(Instant::now);
            }
            if window != seen_window.0 {
                seen_window = (window, Instant::now());
            }
            if differs_since.is_some_and(|t| t.elapsed() >= WINDOW_LATEST) {
                // never settled: say so (a layout flapping by a pixel would keep the desktop at
                // its old size), once per change
                eprintln!("desklink: the window has not held still for {} ms since it changed (now {}x{} at {}, told {}x{}); not told yet", WINDOW_LATEST.as_millis(), window.0 >> 32, window.0 as u32, window.1, told_window.0 >> 32, told_window.0 as u32);
                differs_since = Some(Instant::now() + Duration::from_secs(3600));
            }
            if window != told_window && seen_window.1.elapsed() >= WINDOW_SETTLE {
                told_window = window;
                differs_since = None;
                let (width, height) = ((window.0 >> 32) as u32, window.0 as u32);
                if width > 0 && height > 0 {
                    let scale = window.1.min(u16::MAX as u32) as u16;
                    if let Err(e) = proto::write_all(&mut stream, &ClientMsg::Window { width, height, scale }.encode()) {
                        break Err(format!("write: {e}"));
                    }
                }
            }
            if last_ping.elapsed() >= Duration::from_secs(1) {
                last_ping = Instant::now();
                if let Err(e) = proto::write_all(&mut stream, &ClientMsg::Ping { client_ns: now_ns() }.encode()) {
                    break Err(format!("write: {e}"));
                }
            }
            if last_heard.elapsed() > Duration::from_secs(6) {
                break Err("the desktop stopped answering".into());
            }
            let mut failed = None;
            loop {
                match self.rx.try_recv() {
                    Ok(msg) => {
                        if let ClientMsg::A11yQuery { req, .. } = &msg {
                            if !self.semantic.pending(*req) { continue; }
                        }
                        if let Err(e) = proto::write_all(&mut stream, &msg.encode()) {
                            failed = Some(format!("write: {e}"));
                            break;
                        }
                    }
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            }
            if let Some(e) = failed {
                break Err(e);
            }
            read_gap = read_gap.max(last_read.elapsed());
            let read = stream.read(&mut buf);
            last_read = Instant::now();
            match read {
                Ok(0) => break Err("the desktop closed the connection".into()),
                Ok(n) => inbox.feed(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
                Err(e) => break Err(format!("read: {e}")),
            }
            let mut ended = None;
            loop {
                let msg = match inbox.next() {
                    Ok(Some((ty, payload))) => match ServerMsg::decode(ty, &payload) {
                        Ok(m) => m,
                        Err(e) => {
                            ended = Some(Err(e.to_string()));
                            break;
                        }
                    },
                    Ok(None) => break,
                    Err(e) => {
                        ended = Some(Err(e.to_string()));
                        break;
                    }
                };
                last_heard = Instant::now();
                match msg {
                    ServerMsg::Welcome { screens, screen, stream: size, input, input_note, server, .. } => {
                        self.history.lock().unwrap().stream_size = size;
                        let mut s = self.shared.lock().unwrap();
                        s.welcome = Some(Welcome { screens, screen, stream: size, input, input_note, server });
                        s.session += 1;
                        s.state = Some(State::Connected);
                        drop(s);
                        self.semantic.connected(true);
                        self.signal.set();
                    }
                    ServerMsg::Refused { reason } => {
                        ended = Some(Err(format!("refused: {reason}")));
                        break;
                    }
                    ServerMsg::Bye { reason } => {
                        ended = Some(Ok(format!("the desktop said goodbye: {reason}")));
                        break;
                    }
                    ServerMsg::Video { frame, key, built_ns, encoded_ns, sent_ns, stream, parts, x0, y0, codec, data } => {
                        // pictures per second: each picture's last part (tiles) or each picture
                        if parts <= 1 || stream + 1 == parts {
                            second.1 += 1;
                        }
                        second.2 += data.len();
                        queue_max = queue_max.max(queued.fetch_add(1, Ordering::AcqRel) + 1);
                        let _ = dec_tx.send(Packet { frame, key, stream, parts, x0, y0, codec, built_ns, encoded_ns, sent_ns, received_ns: now_ns(), data: Arc::new(data), seq: 0 });
                    }
                    ServerMsg::Cursor { x, y, shape, visible } => {
                        self.shared.lock().unwrap().cursor = Some((x, y, shape, visible));
                        self.signal.set();
                    }
                    ServerMsg::Pong { client_ns, server_ns } => {
                        let now = now_ns();
                        let rtt = now.saturating_sub(client_ns);
                        let mut s = self.shared.lock().unwrap();
                        s.rtt_ms = Some(rtt as f64 / 1e6);
                        // NTP-style: the server read its clock half a round trip after we sent. The
                        // smallest round trip has the least asymmetry; only recent samples count, since
                        // the two machines' clocks drift apart (tens of ppm: ms over minutes)
                        s.pongs.push_back((now, rtt, server_ns as i64 - (client_ns + rtt / 2) as i64));
                        while s.pongs.front().is_some_and(|p| now.saturating_sub(p.0) > OFFSET_WINDOW_NS) {
                            s.pongs.pop_front();
                        }
                        if let Some(best) = s.pongs.iter().min_by_key(|p| p.1) {
                            s.offset_ns = best.2;
                        }
                        drop(s);
                        self.signal.set();
                    }
                    ServerMsg::Stream { screen, stream: size } => {
                        self.history.lock().unwrap().stream_size = size;
                        if let Some(w) = self.shared.lock().unwrap().welcome.as_mut() {
                            w.screen = screen;
                            w.stream = size;
                        }
                        self.signal.set();
                    }
                    ServerMsg::Rate { fps, kbps, limit, full_chroma, codec, preset, streams, why } => {
                        let mut s = self.shared.lock().unwrap();
                        s.stream_fps = Some(fps);
                        s.stream_rate = Some(StreamRate { kbps, limit, full_chroma, codec, preset, streams, why });
                        drop(s);
                        self.signal.set();
                    }
                    ServerMsg::A11yReply { req, text } => self.semantic.reply(req, text),
                    ServerMsg::Clipboard { .. } => {}
                }
            }
            if let Some(end) = ended {
                break end;
            }
            if second.0.elapsed() >= Duration::from_secs(1) {
                let secs = second.0.elapsed().as_secs_f64();
                let mut s = self.shared.lock().unwrap();
                s.fps = second.1 as f64 / secs;
                s.mbps = second.2 as f64 * 8.0 / secs / 1e6;
                s.read_gap_ms = std::mem::take(&mut read_gap).as_secs_f64() * 1000.0;
                s.decode_queue = std::mem::take(&mut queue_max);
                second = (Instant::now(), 0, 0);
            }
        };
        drop(dec_tx);
        let _ = decoder.join();
        {
            let mut s = self.shared.lock().unwrap();
            s.welcome = None;
            s.pongs.clear();
        }
        result
    }
}

/// `path`: one Annex-B file, or several comma separated: the tiles of each picture, top to
/// bottom (a tiled desktop's `dump`, one file per stream).
#[allow(clippy::too_many_arguments)]
fn play_loop(path: String, gpu: bool, still: Option<u32>, shared: Arc<Mutex<Shared>>, latest: Arc<Mutex<Option<Picture>>>, history: Arc<Mutex<History>>, stop: Arc<AtomicBool>, signal: SignalToUI) {
    use makepad_widgets::makepad_platform::video_file::annex_b;
    let fail = |e: String| {
        shared.lock().unwrap().state = Some(State::Offline(e));
        signal.set();
    };
    // the tiles: a `desk-link-probe --dump` directory (tiles.txt: `<stream> <x0> <y0>` lines), or
    // files `<file>[@<x0>,<y0>]` comma separated
    let mut files: Vec<(String, u32, u32)> = Vec::new();
    let dir = std::path::Path::new(&path);
    if dir.is_dir() {
        let tiles = match std::fs::read_to_string(dir.join("tiles.txt")) {
            Ok(t) => t,
            Err(e) => return fail(format!("{}: {e}", dir.join("tiles.txt").display())),
        };
        for line in tiles.lines() {
            let v: Vec<u32> = line.split_whitespace().filter_map(|w| w.parse().ok()).collect();
            if let [i, x0, y0] = v[..] {
                // the probe names an H.265 stream's file .h265
                let h265 = dir.join(format!("stream-{i}.h265"));
                let file = if h265.exists() { h265 } else { dir.join(format!("stream-{i}.h264")) };
                files.push((file.display().to_string(), x0, y0));
            }
        }
    } else {
        for item in path.split(',') {
            let (file, at) = item.split_once('@').unwrap_or((item, "0,0"));
            let (x0, y0) = at.split_once(',').map_or((0, 0), |(x, y)| (x.parse().unwrap_or(0), y.parse().unwrap_or(0)));
            files.push((file.to_string(), x0, y0));
        }
    }
    // per tile: one access unit per coded picture, parameter sets with the picture after them;
    // .h265 / .hevc files are H.265
    let hevc = files.first().is_some_and(|(f, _, _)| f.ends_with(".h265") || f.ends_with(".hevc"));
    let codec = if hevc { proto::pref::HEVC } else { proto::pref::H264 };
    let vcl = |nal: &[u8]| if hevc { annex_b::hevc_nal_unit_type(nal) < 32 } else { matches!(annex_b::nal_unit_type(nal), 1 | 5) };
    let idr = |nal: &[u8]| if hevc { matches!(annex_b::hevc_nal_unit_type(nal), 16..=21) } else { annex_b::nal_unit_type(nal) == 5 };
    let mut tiles: Vec<(Vec<Vec<u8>>, u32, u32)> = Vec::new();
    for (file, x0, y0) in files {
        let data = match std::fs::read(&file) {
            Ok(d) => d,
            Err(e) => return fail(format!("{file}: {e}")),
        };
        let mut units = Vec::new();
        let mut unit = Vec::new();
        for nal in annex_b::split_annex_b(&data) {
            unit.extend_from_slice(&[0, 0, 0, 1]);
            unit.extend_from_slice(nal);
            if vcl(nal) {
                units.push(std::mem::take(&mut unit));
            }
        }
        tiles.push((units, x0, y0));
    }
    let pictures = tiles.iter().map(|t| t.0.len()).min().unwrap_or(0);
    let parts = tiles.len().min(255) as u8;
    // the live path: each tile's packets to the viewer's decode loop (a decoder thread per stream,
    // the parts of a picture put together), as they come off the network
    let (cmd, _cmd_rx) = channel::<ClientMsg>();
    let mut frame = 0u32;
    while !stop.load(Ordering::Acquire) {
        let (dec_tx, dec_rx) = channel::<Packet>();
        let queued = Arc::new(AtomicUsize::new(0));
        let (l, sig, sh, c, q, h) = (latest.clone(), signal.clone(), shared.clone(), cmd.clone(), queued.clone(), history.clone());
        if let Err(e) = std::thread::Builder::new().name("desklink-dump-dec".into()).spawn(move || decode_loop(dec_rx, l, sig, sh, c, q, h, gpu)) {
            return fail(format!("thread: {e}"));
        }
        for at in 0..pictures {
            if stop.load(Ordering::Acquire) {
                return;
            }
            let now = now_ns();
            for (stream, (units, x0, y0)) in tiles.iter().enumerate() {
                let data = units[at].clone();
                let key = annex_b::split_annex_b(&data).iter().any(|nal| idr(nal));
                queued.fetch_add(1, Ordering::AcqRel);
                let _ = dec_tx.send(Packet { frame, key, stream: stream as u8, parts, x0: *x0, y0: *y0, codec, built_ns: now, encoded_ns: now, sent_ns: now, received_ns: now, data: Arc::new(data), seq: 0 });
            }
            if still == Some(frame) {
                // the decoders (and their pictures) stay alive while the view shows this one
                while !stop.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                return;
            }
            frame = frame.wrapping_add(1);
            std::thread::sleep(Duration::from_micros(16_667));
        }
    }
}

/// Records a packet in the history (its bytes shared, not copied); returns its place there.
fn record(history: &mut History, packet: &Packet) -> u64 {
    history.push(HistPacket {
        seq: 0,
        frame: packet.frame,
        stream: packet.stream,
        parts: packet.parts,
        x0: packet.x0,
        y0: packet.y0,
        codec: packet.codec,
        size: [0, 0],
        key: packet.key,
        built_ns: packet.built_ns,
        encoded_ns: packet.encoded_ns,
        received_ns: packet.received_ns,
        decode_us: 0,
        failed: false,
        data: packet.data.clone(),
    })
}

fn decode_loop(rx: Receiver<Packet>, latest: Arc<Mutex<Option<Picture>>>, signal: SignalToUI, shared: Arc<Mutex<Shared>>, cmd: Sender<ClientMsg>, queued: Arc<AtomicUsize>, history: Arc<Mutex<History>>, gpu: bool) {
    // changing pictures since the last key frame, and when one was last asked for: the history
    // decodes any of its pictures from a key frame at most KEY_EVERY changes before it; whether
    // the picture being received changes something (any part over CHANGE_BYTES)
    let (mut since_key, mut asked, mut changes) = (0u32, Instant::now(), false);
    // one decoder per encoder stream (a tile, or an interleaved stream), each on a thread of its
    // own: the tiles of a picture decode at once, put together by frame number before they show
    let gathering: Arc<Mutex<Assembly>> = Arc::new(Mutex::new(Assembly::default()));
    let mut workers: Vec<Sender<Packet>> = Vec::new();
    // how many streams the desktop sends (a single one is checked for missing pictures)
    let streams = Arc::new(AtomicUsize::new(0));
    loop {
        let packet = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(p) => {
                queued.fetch_sub(1, Ordering::AcqRel);
                p
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let mut packet = packet;
        changes |= packet.data.len() > CHANGE_BYTES;
        if packet.key {
            since_key = 0;
        } else if packet.parts <= 1 || packet.stream + 1 == packet.parts {
            since_key += std::mem::take(&mut changes) as u32;
            if since_key >= KEY_EVERY && asked.elapsed() > Duration::from_secs(1) {
                asked = Instant::now();
                let _ = cmd.send(ClientMsg::RequestKeyframe);
            }
        }
        // recorded here, in arrival order (the stream decoders finish in any order); the decode's
        // numbers follow from the stream's decoder (`History::note`)
        packet.seq = record(&mut history.lock().unwrap(), &packet);
        let s = packet.stream as usize;
        while workers.len() <= s {
            let (tx, wrx) = channel::<Packet>();
            let (latest2, signal2, shared2, cmd2, gathering2, streams2, history2) = (latest.clone(), signal.clone(), shared.clone(), cmd.clone(), gathering.clone(), streams.clone(), history.clone());
            let index = workers.len();
            streams.store(index + 1, Ordering::Release);
            let spawned = std::thread::Builder::new().name(format!("desklink-dec-{index}")).spawn(move || decode_stream(index, wrx, latest2, signal2, shared2, cmd2, gathering2, streams2, history2, gpu));
            if let Err(e) = spawned {
                shared.lock().unwrap().warning = Some(format!("decoder thread: {e}"));
                return;
            }
            workers.push(tx);
        }
        if workers[s].send(packet).is_err() {
            return;
        }
    }
}

/// The tiled picture being put together: its frame, the parts so far, its times.
type Gather = (u32, Vec<Option<Part>>, Picture);

/// What the stream decoders share: the picture being put together, and the last picture
/// published (an interleaved stream's older picture finishing late is not shown after a newer).
#[derive(Default)]
struct Assembly {
    /// The pictures whose tiles are coming in, oldest first (at most `GATHERING`): the tile
    /// decoders run apart (one hardware engine time-sliced over their sessions), so a tile of
    /// the next picture often finishes before the last tile of this one. Gathering one picture
    /// at a time dropped this one then: ~70 of ~115 pictures a second shown.
    gather: std::collections::VecDeque<Gather>,
    published: Option<u32>,
    /// Pictures never shown since the last `take_dropped`: replaced by a newer one before the
    /// view took them, or never complete (a newer one completed first).
    dropped: u32,
}

/// Pictures put together at once (tiles of more pictures than this apart: the oldest goes).
const GATHERING: usize = 4;

/// `a` is older than `b` (frame numbers wrap).
fn older(a: u32, b: u32) -> bool {
    a != b && b.wrapping_sub(a) < u32::MAX / 2
}

impl Assembly {
    fn publish(&mut self, picture: Picture, latest: &Mutex<Option<Picture>>, signal: &SignalToUI) {
        if self.published.is_some_and(|last| !older(last, picture.frame)) {
            return;
        }
        self.published = Some(picture.frame);
        if latest.lock().unwrap().replace(picture).is_some() {
            self.dropped += 1;
        }
        signal.set();
    }

    /// A tile of a picture of `parts` tiles; the picture once all of them are here (older
    /// pictures still incomplete then are dropped: this one shows instead).
    fn tile(&mut self, s: usize, parts: usize, part: Part, picture: Picture) -> Option<Picture> {
        let frame = picture.frame;
        if self.published.is_some_and(|last| !older(last, frame)) {
            return None;
        }
        let at = match self.gather.iter().position(|(f, got, _)| *f == frame && got.len() == parts) {
            Some(i) => {
                let (_, _, p) = &mut self.gather[i];
                p.encoded_ns = p.encoded_ns.max(picture.encoded_ns);
                p.received_ns = p.received_ns.max(picture.received_ns);
                p.decoded_ns = p.decoded_ns.max(picture.decoded_ns);
                // the slowest tile's decode: they ran at once
                p.decode_ns = p.decode_ns.max(picture.decode_ns);
                i
            }
            None => {
                let i = self.gather.iter().position(|(f, _, _)| older(frame, *f)).unwrap_or(self.gather.len());
                self.gather.insert(i, (frame, (0..parts).map(|_| None).collect(), picture));
                if self.gather.len() > GATHERING {
                    self.gather.pop_front();
                    self.dropped += 1;
                    if i == 0 {
                        return None;
                    }
                    i - 1
                } else {
                    i
                }
            }
        };
        let (_, got, _) = &mut self.gather[at];
        if s < got.len() {
            got[s] = Some(part);
        }
        if !got.iter().all(Option::is_some) {
            return None;
        }
        // complete: the older ones never will be shown
        self.dropped += at as u32;
        let (_, got, mut p) = self.gather.drain(..=at).last().unwrap();
        p.parts = got.into_iter().flatten().collect();
        p.width = p.parts.iter().map(|part| part.x0 + part.width).max().unwrap_or(0);
        p.height = p.parts.iter().map(|part| part.y0 + part.height).max().unwrap_or(0);
        Some(p)
    }
}

/// H.265 is no use on this machine (`why`): said once; `auto` asks for H.264 from now on (the
/// network thread sends the new `Encoding`).
fn hevc_unusable(shared: &Mutex<Shared>, signal: &SignalToUI, why: &str) {
    if !HEVC_UNUSABLE.swap(true, Ordering::AcqRel) {
        eprintln!("desklink: {why}; asking for H.264");
        shared.lock().unwrap().warning = Some(format!("{why}: auto uses H.264"));
        signal.set();
    }
}

/// One encoder stream's decoder: waits for its key frame after a failure; a whole picture
/// goes out at once, a tile once its picture's other tiles are here (never a torn picture; a
/// newer picture complete first drops the older incomplete ones, see `Assembly`).
#[allow(clippy::too_many_arguments)]
fn decode_stream(s: usize, rx: Receiver<Packet>, latest: Arc<Mutex<Option<Picture>>>, signal: SignalToUI, shared: Arc<Mutex<Shared>>, cmd: Sender<ClientMsg>, gathering: Arc<Mutex<Assembly>>, streams: Arc<AtomicUsize>, history: Arc<Mutex<History>>, gpu: bool) {
    // the decoder and the codec it decodes (`pref::*`): a desktop that changes codec sends a key
    // frame in the new one, which gets a new decoder
    let mut decoder: Option<(VideoStreamDecoder, u8)> = None;
    let mut waiting_key = false;
    // the last picture of a single-stream desktop: a picture missing in between means the next
    // ones would decode against a reference this decoder never had
    let mut last_frame: Option<u32> = None;
    while let Ok(packet) = rx.recv() {
        if waiting_key && !packet.key {
            history.lock().unwrap().note(packet.seq, 0, true, [0, 0]);
            continue;
        }
        if streams.load(Ordering::Acquire) == 1 && packet.parts <= 1 && !packet.key {
            if let Some(last) = last_frame {
                if packet.frame != last.wrapping_add(1) {
                    eprintln!("desklink: picture {} follows {last} (missing in between); asking for a key frame", packet.frame);
                    waiting_key = true;
                    last_frame = None;
                    let _ = cmd.send(ClientMsg::RequestKeyframe);
                    history.lock().unwrap().note(packet.seq, 0, true, [0, 0]);
                    continue;
                }
            }
        }
        if decoder.as_ref().is_some_and(|d| d.1 != packet.codec) && !packet.key {
            // the codec changed and this is no key frame of the new one: wait for it
            waiting_key = true;
            let _ = cmd.send(ClientMsg::RequestKeyframe);
            history.lock().unwrap().note(packet.seq, 0, true, [0, 0]);
            continue;
        }
        last_frame = Some(packet.frame);
        if decoder.as_ref().is_none_or(|d| d.1 != packet.codec) || packet.key && waiting_key {
            let codec = if packet.codec == proto::pref::HEVC { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
            match if gpu { VideoStreamDecoder::new_gpu(codec) } else { VideoStreamDecoder::new(codec) } {
                Ok(d) => decoder = Some((d, packet.codec)),
                Err(e) if packet.codec == proto::pref::HEVC => {
                    // no H.265 decoder: `auto` asks for H.264, whose key frame comes next
                    hevc_unusable(&shared, &signal, &format!("no H.265 decoder ({e:?})"));
                    decoder = None;
                    waiting_key = true;
                    let _ = cmd.send(ClientMsg::RequestKeyframe);
                    history.lock().unwrap().note(packet.seq, 0, true, [0, 0]);
                    continue;
                }
                Err(e) => {
                    shared.lock().unwrap().warning = Some(format!("hardware decoder: {e:?}"));
                    signal.set();
                    return;
                }
            }
        }
        waiting_key = false;
        let start = now_ns();
        let (dec, codec) = decoder.as_mut().unwrap();
        let fresh = dec.hardware().is_none();
        let out = dec.push_packet(&packet.data, packet.frame as i64);
        let decoded_ns = now_ns();
        // a new H.265 session in software (the hardware decoder does not take its profile):
        // `auto` asks for H.264 from now on
        if fresh && *codec == proto::pref::HEVC && dec.hardware() == Some(false) {
            hevc_unusable(&shared, &signal, "H.265 decodes in software here");
        }
        let frames = match out {
            Ok(frames) => frames,
            Err(e) => {
                if *codec == proto::pref::HEVC && packet.key {
                    hevc_unusable(&shared, &signal, &format!("an H.265 key frame failed to decode ({e:?})"));
                }
                eprintln!("desklink: decode of picture {} (stream {s}): {e:?}; asking for a key frame", packet.frame);
                waiting_key = true;
                let _ = cmd.send(ClientMsg::RequestKeyframe);
                history.lock().unwrap().note(packet.seq, ((decoded_ns - start) / 1000) as u32, true, [0, 0]);
                continue;
            }
        };
        let size = frames.first().map_or([0, 0], |f| [f.width, f.height]);
        for f in frames {
            let part = Part { x0: packet.x0, y0: packet.y0, width: f.width, height: f.height, nv12: f.nv12, gpu: f.gpu, color: f.color };
            let mut picture = Picture {
                width: packet.x0 + part.width,
                height: packet.y0 + part.height,
                parts: Vec::new(),
                frame: packet.frame,
                built_ns: packet.built_ns,
                encoded_ns: packet.encoded_ns,
                sent_ns: packet.sent_ns,
                received_ns: packet.received_ns,
                decoded_ns,
                decode_ns: decoded_ns - start,
            };
            let parts = packet.parts.max(1) as usize;
            let mut assembly = gathering.lock().unwrap();
            if parts == 1 {
                picture.parts.push(part);
                assembly.publish(picture, &latest, &signal);
                let dropped = std::mem::take(&mut assembly.dropped);
                drop(assembly);
                if dropped > 0 {
                    shared.lock().unwrap().dropped += dropped;
                }
                continue;
            }
            if let Some(p) = assembly.tile(s, parts, part, picture) {
                assembly.publish(p, &latest, &signal);
            }
            let dropped = std::mem::take(&mut assembly.dropped);
            drop(assembly);
            if dropped > 0 {
                shared.lock().unwrap().dropped += dropped;
            }
        }
        // after the picture went out: its decode time and size for the history
        history.lock().unwrap().note(packet.seq, ((decoded_ns - start) / 1000) as u32, false, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(frame: u32, s: usize) -> (Part, Picture) {
        let part = Part { x0: s as u32 * 64, y0: 0, width: 64, height: 16, nv12: Vec::new(), gpu: None, color: VideoColor::BT709 };
        let picture = Picture { width: 0, height: 0, parts: Vec::new(), frame, built_ns: 0, encoded_ns: 0, sent_ns: 0, received_ns: 0, decoded_ns: 0, decode_ns: 0 };
        (part, picture)
    }

    fn put(a: &mut Assembly, frame: u32, s: usize) -> Option<u32> {
        let (part, picture) = tile(frame, s);
        let p = a.tile(s, 2, part, picture)?;
        assert_eq!((p.parts.len(), p.width), (2, 128));
        a.published = Some(p.frame);
        Some(p.frame)
    }

    /// The tile decoders drift apart: the next picture's first tile before this one's last
    /// still shows both pictures (one at a time dropped every picture it overtook).
    #[test]
    fn tiles_of_two_pictures_in_flight_both_show() {
        let mut a = Assembly::default();
        assert_eq!(put(&mut a, 10, 0), None);
        assert_eq!(put(&mut a, 11, 0), None);
        assert_eq!(put(&mut a, 10, 1), Some(10));
        assert_eq!(put(&mut a, 11, 1), Some(11));
        assert_eq!(a.dropped, 0);
        // a newer picture complete first: the older one never shows, its late tile is ignored
        assert_eq!(put(&mut a, 12, 0), None);
        assert_eq!(put(&mut a, 13, 0), None);
        assert_eq!(put(&mut a, 13, 1), Some(13));
        assert_eq!(a.dropped, 1);
        assert_eq!(put(&mut a, 12, 1), None);
        assert!(a.gather.is_empty());
        // frame numbers wrap
        a.published = Some(u32::MAX - 1);
        assert_eq!(put(&mut a, u32::MAX, 0), None);
        assert_eq!(put(&mut a, 0, 0), None);
        assert_eq!(put(&mut a, u32::MAX, 1), Some(u32::MAX));
        assert_eq!(put(&mut a, 0, 1), Some(0));
    }
}
