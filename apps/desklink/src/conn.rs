//! One Desk Link connection: a network thread (TLS, the fleet proof, the protocol, reconnects)
//! and a decode thread (the platform's hardware H.264 decoder). The UI reads its state and the
//! newest decoded picture; it never waits on either thread.
//!
//! A connection the viewer is not showing stays open, paused: the desktop makes no pictures
//! for it, and its pings keep the side panel's latency live. Showing it again is a `Resume`,
//! answered by a key frame: one round trip and one encode, no TLS handshake.

use makepad_network::fleet::auth as fleet_auth;
use makepad_desk_link::{self as proto, ClientMsg, Inbox, ScreenInfo, ServerMsg};
use makepad_network::tls::KnownHosts;
use makepad_network::{SignalToUI, SocketStream};
use makepad_widgets::makepad_platform::video_file::{DecodedPicture, StreamVideoCodec, VideoStreamDecoder};
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// This process's clock (ns): the viewer side of every latency figure.
pub fn now_ns() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Connecting,
    Connected,
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

#[derive(Clone, Debug, Default)]
pub struct Shared {
    pub state: Option<State>,
    pub welcome: Option<Welcome>,
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
    pub warning: Option<String>,
}

/// A decoded picture: the decoder's GPU picture where the platform has one (no CPU copy),
/// else tight NV12.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
    pub gpu: Option<DecodedPicture>,
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
    built_ns: u64,
    encoded_ns: u64,
    sent_ns: u64,
    received_ns: u64,
    data: Vec<u8>,
}

pub struct Connection {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub shared: Arc<Mutex<Shared>>,
    pub latest: Arc<Mutex<Option<Picture>>>,
    tx: Sender<ClientMsg>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Connection {
    /// The network thread (and its decoder) ends within its connect / handshake bound and is
    /// joined: nothing outlives the connection.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Connection {
    /// Opens (and keeps reopening) the connection; `paused` until [`Connection::show`].
    /// `discovered`: a `known_nodes` machine that may have no desktop: while it is offline it
    /// is retried rarely (backing off to 10 minutes) instead of every 30 s.
    pub fn open(name: String, host: String, port: u16, paused: bool, discovered: bool, signal: SignalToUI) -> Connection {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let latest = Arc::new(Mutex::new(None));
        let (tx, rx) = channel();
        let paused = Arc::new(AtomicBool::new(paused));
        let stop = Arc::new(AtomicBool::new(false));
        let max_backoff = Duration::from_secs(if discovered { 600 } else { 30 });
        let net = Net { host: host.clone(), port, shared: shared.clone(), latest: latest.clone(), rx, paused: paused.clone(), stop: stop.clone(), signal, cmd: tx.clone(), max_backoff };
        let thread = match std::thread::Builder::new().name(format!("desklink-net-{host}")).spawn(move || net.run()) {
            Ok(t) => Some(t),
            Err(e) => {
                shared.lock().unwrap().state = Some(State::Offline(format!("thread: {e}")));
                None
            }
        };
        Connection { name, host, port, shared, latest, tx, paused, stop, thread }
    }

    /// A desktop played from an Annex-B dump (dev: `--dump-play`): its pictures decoded the way
    /// a live stream's are (`gpu`: the decoder's GPU pictures, else CPU NV12), at 60 per
    /// second, over and over.
    /// `still`: stop at that picture and keep showing it (comparable grabs).
    pub fn play_dump(path: String, gpu: bool, still: Option<u32>, signal: SignalToUI) -> Connection {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let latest = Arc::new(Mutex::new(None));
        let (tx, _rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        shared.lock().unwrap().state = Some(State::Connected);
        let (s, l, st) = (shared.clone(), latest.clone(), stop.clone());
        let thread = std::thread::Builder::new().name("desklink-dump".into()).spawn(move || play_loop(path, gpu, still, s, l, st, signal)).ok();
        Connection { name: "dump".into(), host: "dump".into(), port: 0, shared, latest, tx, paused: Arc::new(AtomicBool::new(false)), stop, thread }
    }

    pub fn send(&self, msg: ClientMsg) {
        let _ = self.tx.send(msg);
    }

    pub fn show(&self, on: bool) {
        if self.paused.swap(!on, Ordering::AcqRel) == on {
            self.send(if on { ClientMsg::Resume } else { ClientMsg::Pause });
        }
    }

    pub fn state(&self) -> State {
        self.shared.lock().unwrap().state.clone().unwrap_or(State::Connecting)
    }
}

struct Net {
    host: String,
    port: u16,
    shared: Arc<Mutex<Shared>>,
    latest: Arc<Mutex<Option<Picture>>>,
    rx: Receiver<ClientMsg>,
    paused: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
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
        while !self.stop.load(Ordering::Acquire) {
            self.set_state(State::Connecting);
            let started = Instant::now();
            let reason = match self.session() {
                Ok(reason) => reason,
                Err(e) => e,
            };
            if self.stop.load(Ordering::Acquire) {
                return;
            }
            self.set_state(State::Offline(reason.clone()));
            // a session that ran a while starts the backoff over
            if started.elapsed() > Duration::from_secs(30) {
                backoff = Duration::from_secs(2);
            }
            let refused = reason.starts_with("refused");
            let wait = if refused { Duration::from_secs(30) } else { backoff };
            let until = Instant::now() + wait;
            while Instant::now() < until && !self.stop.load(Ordering::Acquire) {
                // commands while offline are void (input for a desktop that is not there)
                while self.rx.try_recv().is_ok() {}
                std::thread::sleep(Duration::from_millis(100));
            }
            backoff = (backoff * 2).min(self.max_backoff);
        }
    }

    fn session(&self) -> Result<String, String> {
        let credential = fleet_auth::own_credential()
            .ok_or_else(|| format!("no fleet credential ({})", fleet_auth::client_dir().join(fleet_auth::CLIENT_CREDENTIAL).display()))?;
        let port = self.port.to_string();
        let (mut stream, fingerprint) = SocketStream::connect_capture(&self.host, &port).map_err(|e| format!("connect: {e}"))?;
        // ssh-style pin of the desktop's certificate, loud when it changes
        let known = KnownHosts::new(fleet_auth::client_dir().join(fleet_auth::KNOWN_NODES));
        if let Some(previous) = known.observe(&format!("{}:{}", self.host, self.port), &fingerprint) {
            let text = format!(
                "the certificate of {}:{} CHANGED (was {}, now {})",
                self.host,
                self.port,
                makepad_network::tls::to_hex(&previous),
                makepad_network::tls::to_hex(&fingerprint)
            );
            eprintln!("desklink: WARNING {text}");
            self.shared.lock().unwrap().warning = Some(text);
        }
        let hello = ClientMsg::Hello {
            version: proto::VERSION,
            name: format!("desklink {}", std::env::var("USER").unwrap_or_default()),
            authorization: credential.authorization(&fingerprint),
            screen: -1,
            paused: self.paused.load(Ordering::Acquire),
        };
        proto::write_all(&mut stream, &hello.encode()).map_err(|e| format!("hello: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;

        // the decoder: its own thread, fed every picture in order
        let (dec_tx, dec_rx) = channel::<Packet>();
        let decoder = {
            let (latest, signal, shared, cmd) = (self.latest.clone(), self.signal.clone(), self.shared.clone(), self.cmd.clone());
            std::thread::Builder::new().name(format!("desklink-dec-{}", self.host)).spawn(move || decode_loop(dec_rx, latest, signal, shared, cmd)).map_err(|e| e.to_string())?
        };

        let mut inbox = Inbox::default();
        let mut buf = vec![0u8; 512 << 10];
        let mut last_ping = Instant::now() - Duration::from_secs(10);
        let mut second = (Instant::now(), 0u32, 0usize);
        let mut last_heard = Instant::now();
        let result = loop {
            if self.stop.load(Ordering::Acquire) {
                break Ok("closed".into());
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
            match stream.read(&mut buf) {
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
                        let mut s = self.shared.lock().unwrap();
                        s.welcome = Some(Welcome { screens, screen, stream: size, input, input_note, server });
                        s.state = Some(State::Connected);
                        drop(s);
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
                    ServerMsg::Video { frame, key, built_ns, encoded_ns, sent_ns, data } => {
                        second.1 += 1;
                        second.2 += data.len();
                        let _ = dec_tx.send(Packet { frame, key, built_ns, encoded_ns, sent_ns, received_ns: now_ns(), data });
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
                        if let Some(w) = self.shared.lock().unwrap().welcome.as_mut() {
                            w.screen = screen;
                            w.stream = size;
                        }
                        self.signal.set();
                    }
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

fn play_loop(path: String, gpu: bool, still: Option<u32>, shared: Arc<Mutex<Shared>>, latest: Arc<Mutex<Option<Picture>>>, stop: Arc<AtomicBool>, signal: SignalToUI) {
    use makepad_widgets::makepad_platform::video_file::annex_b;
    let fail = |e: String| {
        shared.lock().unwrap().state = Some(State::Offline(e));
        signal.set();
    };
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(e) => return fail(format!("{path}: {e}")),
    };
    // one access unit per coded picture, parameter sets with the picture after them
    let mut units = Vec::new();
    let mut unit = Vec::new();
    for nal in annex_b::split_annex_b(&data) {
        unit.extend_from_slice(&[0, 0, 0, 1]);
        unit.extend_from_slice(nal);
        if matches!(annex_b::nal_unit_type(nal), 1 | 5) {
            units.push(std::mem::take(&mut unit));
        }
    }
    let mut frame = 0u32;
    while !stop.load(Ordering::Acquire) {
        let decoder = if gpu { VideoStreamDecoder::new_gpu(StreamVideoCodec::H264) } else { VideoStreamDecoder::new(StreamVideoCodec::H264) };
        let mut decoder = match decoder {
            Ok(d) => d,
            Err(e) => return fail(format!("hardware decoder: {e:?}")),
        };
        for unit in &units {
            if stop.load(Ordering::Acquire) {
                return;
            }
            let start = now_ns();
            let frames = decoder.push_packet(unit, frame as i64).unwrap_or_default();
            let decoded_ns = now_ns();
            for f in frames {
                *latest.lock().unwrap() = Some(Picture {
                    width: f.width,
                    height: f.height,
                    nv12: f.nv12,
                    gpu: f.gpu,
                    frame,
                    built_ns: start,
                    encoded_ns: start,
                    sent_ns: start,
                    received_ns: start,
                    decoded_ns,
                    decode_ns: decoded_ns - start,
                });
                signal.set();
            }
            if still == Some(frame) {
                // the decoder (and its pictures) stay alive while the view shows this one
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

fn decode_loop(rx: Receiver<Packet>, latest: Arc<Mutex<Option<Picture>>>, signal: SignalToUI, shared: Arc<Mutex<Shared>>, cmd: Sender<ClientMsg>) {
    let mut decoder: Option<VideoStreamDecoder> = None;
    let mut waiting_key = false;
    loop {
        let packet = match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(p) => p,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if waiting_key && !packet.key {
            continue;
        }
        if decoder.is_none() || packet.key && waiting_key {
            match VideoStreamDecoder::new_gpu(StreamVideoCodec::H264) {
                Ok(d) => decoder = Some(d),
                Err(e) => {
                    shared.lock().unwrap().warning = Some(format!("hardware decoder: {e:?}"));
                    signal.set();
                    return;
                }
            }
        }
        waiting_key = false;
        let start = now_ns();
        let out = decoder.as_mut().unwrap().push_packet(&packet.data, packet.frame as i64);
        let decoded_ns = now_ns();
        match out {
            Ok(frames) => {
                for f in frames {
                    *latest.lock().unwrap() = Some(Picture {
                        width: f.width,
                        height: f.height,
                        nv12: f.nv12,
                        gpu: f.gpu,
                        frame: packet.frame,
                        built_ns: packet.built_ns,
                        encoded_ns: packet.encoded_ns,
                        sent_ns: packet.sent_ns,
                        received_ns: packet.received_ns,
                        decoded_ns,
                        decode_ns: decoded_ns - start,
                    });
                    signal.set();
                }
            }
            Err(e) => {
                eprintln!("desklink: decode of picture {}: {e:?}; asking for a key frame", packet.frame);
                waiting_key = true;
                let _ = cmd.send(ClientMsg::RequestKeyframe);
            }
        }
    }
}
