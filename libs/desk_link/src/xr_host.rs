//! The desktop end of Desk Link XR (`crate::xr`): an XR server in an app's own process that has
//! a remote headset slot (`MAKEPAD_XR_REMOTE=1`, `makepad_hal::xr_remote`). A headset connects
//! (fleet TLS; `Need::XrViewer`), says its eye size and refresh; its poses go to the app's XR
//! window as `XrFrame`s. The app renders each into its own textures and packs the picture on the
//! GPU (NV12 in a shared texture, `makepad_xr_stream`), then hands it here with [`submit`]: the
//! picture's eye columns are encoded at once on the GPU's encode engines (a stream per eye, the
//! texture read where it lies) and sent with the eyes of the pose it was drawn with. One headset
//! at a time.
//!
//! Pipelined and bounded: the app asks for the next pose as soon as it built a frame and has
//! [`room`] (a picture slot free, and fewer than `in_flight` pictures built and not yet
//! decoded by the headset); while there is none it waits, told by the wake ([`set_wake`]) when
//! there is again. So a slow link or decoder makes the server render less, never queue more.
//!
//! Threads: the listener; per session its TLS thread (reads poses and stats, writes pictures),
//! its picture thread (matches a picture to its pose and fans it out) and an encode thread per
//! eye. Nothing here runs on the app's UI thread.

use crate::server::{fleet_identity, lan_address};
use crate::xr::*;
use crate::{write_all, Inbox};
use makepad_hal::xr_remote as hal;
use makepad_network::fleet::id::{self, now_secs, Need};
use makepad_network::tls::TlsServer;
use makepad_video::{GpuReady, QpGrid, StreamTile, StreamTuning, StreamVideoCodec, VideoStreamEncoder, VideoStreamEncoderOptions};
use std::collections::VecDeque;
use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The room field's bricks from a headset: (voxel size m, chunk edge voxels, bricks).
pub type WorldSink = Arc<dyn Fn(f32, i32, Vec<WorldBrick>) + Send + Sync>;

pub struct XrHostConfig {
    pub addr: SocketAddr,
    /// Where a headset's room field goes (the app mirrors it for its physics).
    pub world: Option<WorldSink>,
    /// The TLS certificate's name.
    pub tls_name: String,
    /// The stream's rate (kbit/s) until rate control lands here (0: a rate for the size).
    pub kbps: u32,
    pub log: Log,
}

pub struct XrHost {
    stop: Arc<AtomicBool>,
    pub addr: SocketAddr,
}

impl Drop for XrHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Whether this process can serve a remote headset (its HAL has the slot).
pub fn available() -> bool {
    hal::hal_xr_remote_enabled()
}

impl XrHost {
    pub fn start(config: XrHostConfig) -> Result<XrHost, String> {
        if !available() {
            return Err("xr host: this process has no remote headset slot (MAKEPAD_XR_REMOTE=1)".into());
        }
        let (identity, verifier, name) = fleet_identity(&config.tls_name).map_err(|why| format!("xr host: {why}"))?;
        let tls = Arc::new(TlsServer::new(identity).map_err(|e| format!("xr host: TLS server: {e}"))?);
        let stop = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        let verifier = Arc::new(verifier);
        let log = config.log.clone();
        log(&format!("xr host: listens on {} as {name} (the admin or a paired headset)", config.addr));
        let (stop2, kbps, addr, world) = (stop.clone(), config.kbps, config.addr, config.world.clone());
        std::thread::Builder::new()
            .name("xr-host-listen".into())
            .spawn(move || {
                // the port may still be held a moment by a host this process just dropped (an app
                // restart): bind again until it is free
                let listener = loop {
                    if stop2.load(Ordering::Acquire) {
                        return;
                    }
                    match TcpListener::bind(addr).and_then(|l| l.set_nonblocking(true).map(|_| l)) {
                        Ok(l) => break l,
                        Err(e) => {
                            log(&format!("xr host: bind {addr}: {e}; trying again"));
                            std::thread::sleep(Duration::from_secs(1));
                        }
                    }
                };
                while !stop2.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((tcp, peer)) => {
                            if !lan_address(peer.ip()) {
                                continue;
                            }
                            let ctx = Session { peer, tls: tls.clone(), verifier: verifier.clone(), log: log.clone(), stop: stop2.clone(), busy: busy.clone(), server: name.clone(), kbps, world: world.clone() };
                            let _ = std::thread::Builder::new().name("xr-host-session".into()).spawn(move || ctx.run(tcp));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                        Err(e) => {
                            log(&format!("xr host: accept: {e}"));
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(XrHost { stop, addr: config.addr })
    }
}

struct Session {
    world: Option<WorldSink>,
    peer: SocketAddr,
    tls: Arc<TlsServer>,
    verifier: Arc<id::Verifier>,
    log: Log,
    stop: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    server: String,
    kbps: u32,
}

impl Session {
    fn run(self, tcp: TcpStream) {
        let why = self.serve(tcp).unwrap_or_else(|e| e);
        (self.log)(&format!("xr host: session {}: {why}", self.peer));
    }

    fn serve(&self, tcp: TcpStream) -> Result<String, String> {
        tcp.set_nonblocking(false).map_err(|e| e.to_string())?;
        let _ = tcp.set_nodelay(true);
        tcp.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        tcp.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        let mut stream = self.tls.accept(tcp).map_err(|e| format!("TLS handshake: {e}"))?;
        let mut inbox = Inbox::default();
        let mut buf = vec![0u8; 256 << 10];
        let started = Instant::now();
        let (ty, payload) = loop {
            if let Some(m) = inbox.next().map_err(|e| e.to_string())? {
                break m;
            }
            match stream.read(&mut buf) {
                Ok(0) => return Err("closed before its hello".into()),
                Ok(n) => inbox.feed(&buf[..n]),
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => return Err(format!("read: {e}")),
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err("no hello".into());
            }
        };
        let refuse = |stream: &mut makepad_network::tls::TlsStream, reason: String| -> Result<String, String> {
            let _ = write_all(stream, &XrServerMsg::Refused { reason: reason.clone() }.encode());
            stream.shutdown();
            Err(format!("refused: {reason}"))
        };
        let Ok(XrClientMsg::Hello { version, name, authorization, eye, refresh_mhz, codecs: decodes }) = XrClientMsg::decode(ty, &payload) else {
            return refuse(&mut stream, "the first message must be a hello".into());
        };
        if version != XR_VERSION {
            return refuse(&mut stream, format!("XR protocol version {version}; this server speaks {XR_VERSION}"));
        }
        let peer = match self.verifier.check(&authorization, self.peer.ip(), now_secs()) {
            Ok(peer) => peer,
            Err(e) => return refuse(&mut stream, format!("fleet credential: {e}")),
        };
        if let Err(why) = id::require(&peer, Need::XrViewer) {
            return refuse(&mut stream, format!("may not view XR here: {why}"));
        }
        if self.busy.swap(true, Ordering::AcqRel) {
            return refuse(&mut stream, "another headset is connected".into());
        }
        let result = self.session(&mut stream, inbox, buf, &name, &peer.name, eye, refresh_mhz, decodes);
        self.busy.store(false, Ordering::Release);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn session(&self, stream: &mut makepad_network::tls::TlsStream, mut inbox: Inbox, mut buf: Vec<u8>, name: &str, who: &str, eye: [u32; 2], refresh_mhz: u32, decodes: u32) -> Result<String, String> {
        let eye = [eye[0].clamp(64, 4096) & !3, eye[1].clamp(64, 4096) & !3];
        let codec = if decodes & codecs::HEVC != 0 { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
        let fps = (refresh_mhz / 1000).clamp(30, 120);
        let core = Core::start(eye, fps, codec, self.kbps, self.log.clone())?;
        (self.log)(&format!("xr host: {who} ({name}) from {}: eyes {}x{} at {fps} Hz, {codec:?} {}x{} in {} tile(s) on {}", self.peer, eye[0], eye[1], core.size[0], core.size[1], core.tiles, core.description));
        hal::hal_xr_remote_attach(eye[0], eye[1], refresh_mhz);
        let welcome = XrServerMsg::Welcome { version: XR_VERSION, server: self.server.clone(), size: core.size, codec: core.codec, tiles: core.tiles as u8, encoder: core.description.clone() };
        write_all(stream, &welcome.encode()).map_err(|e| format!("write: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_millis(1))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_millis(5))).map_err(|e| e.to_string())?;
        let why = self.pump(stream, &mut inbox, &mut buf, &core);
        core.stop();
        let _ = write_all(stream, &XrServerMsg::Bye { reason: why.clone() }.encode());
        stream.shutdown();
        Ok(why)
    }

    fn pump(&self, stream: &mut makepad_network::tls::TlsStream, inbox: &mut Inbox, buf: &mut [u8], core: &Core) -> String {
        let mut last_heard = Instant::now();
        let mut visible = true;
        let mut stats = PumpStats::new();
        loop {
            if self.stop.load(Ordering::Acquire) {
                return "the server stopped".into();
            }
            if last_heard.elapsed() > Duration::from_secs(6) {
                return "the headset was silent for 6 s".into();
            }
            loop {
                match core.out.try_recv() {
                    Ok(Out::Video(head, data, encoded_at)) => {
                        let mut head = head;
                        head.stages[stage::SEND] = encoded_at.elapsed().as_micros() as u32;
                        let msg = encode_xr_video(&head, &data);
                        let write_at = Instant::now();
                        if let Err(e) = write_all(stream, &msg) {
                            return format!("write: {e}");
                        }
                        stats.sent(&head, msg.len(), write_at.elapsed());
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return "the encoder stopped".into(),
                }
            }
            let n = match stream.read(buf) {
                Ok(0) => return "the headset closed the connection".into(),
                Ok(n) => n,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => 0,
                Err(e) => return format!("read: {e}"),
            };
            if n > 0 {
                inbox.feed(&buf[..n]);
                last_heard = Instant::now();
            }
            loop {
                let (ty, payload) = match inbox.next() {
                    Ok(Some(m)) => m,
                    Ok(None) => break,
                    Err(e) => return e.to_string(),
                };
                let msg = match XrClientMsg::decode(ty, &payload) {
                    Ok(m) => m,
                    Err(e) => return e.to_string(),
                };
                match msg {
                    XrClientMsg::Pose { seq, display_ns, frame, .. } if visible => {
                        stats.poses += 1;
                        if !core.pose(seq, display_ns, &frame) {
                            stats.refused_poses += 1;
                        }
                    }
                    XrClientMsg::Pose { .. } => {}
                    XrClientMsg::State { visible: v, focused } => {
                        if v && !visible {
                            core.keyframe();
                        }
                        visible = v;
                        (self.log)(&format!("xr host: headset {} {}", if v { "visible" } else { "not visible" }, if focused { "focused" } else { "not focused" }));
                    }
                    XrClientMsg::Stats { decode_us, .. } => {
                        stats.decode_us.push(decode_us);
                        core.tile_decoded();
                    }
                    XrClientMsg::Ping { client_ns } => {
                        if let Err(e) = write_all(stream, &XrServerMsg::Pong { client_ns, server_ns: makepad_hal::clock_ns() }.encode()) {
                            return format!("write: {e}");
                        }
                    }
                    XrClientMsg::Keyframe => core.keyframe(),
                    XrClientMsg::World { voxel_size, chunk_edge, bricks } => {
                        stats.bricks += bricks.len() as u64;
                        stats.brick_bytes += payload.len() as u64;
                        if let Some(world) = &self.world {
                            world(voxel_size, chunk_edge, bricks);
                        }
                    }
                    XrClientMsg::Hello { .. } => return "a second hello".into(),
                }
            }
            stats.window(core, &self.log);
        }
    }
}

/// The per-5 s stats of a session's link.
struct PumpStats {
    at: Instant,
    poses: u64,
    refused_poses: u64,
    pictures: u64,
    bytes: u64,
    stages_us: [Vec<u32>; 4],
    write_us: Vec<u32>,
    decode_us: Vec<u32>,
    /// Room-field bricks received, and their messages' bytes.
    bricks: u64,
    brick_bytes: u64,
}

fn median_ms(v: &mut [u32]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    v[v.len() / 2] as f64 / 1000.0
}

impl PumpStats {
    fn new() -> Self {
        PumpStats { at: Instant::now(), poses: 0, refused_poses: 0, pictures: 0, bytes: 0, stages_us: Default::default(), write_us: Vec::new(), decode_us: Vec::new(), bricks: 0, brick_bytes: 0 }
    }

    fn sent(&mut self, head: &VideoHead, bytes: usize, write: Duration) {
        // a picture counted once, at its first tile; its stages are the slowest tile's
        if head.tile == 0 {
            self.pictures += 1;
        }
        self.bytes += bytes as u64;
        for (i, v) in head.stages.iter().enumerate() {
            self.stages_us[i].push(*v);
        }
        self.write_us.push(write.as_micros() as u32);
    }

    fn window(&mut self, core: &Core, log: &Log) {
        let secs = self.at.elapsed().as_secs_f64();
        if secs < 5.0 {
            return;
        }
        let built = core.flow.built.swap(0, Ordering::AcqRel);
        let skipped = core.flow.skipped.swap(0, Ordering::AcqRel);
        let kbps = core.rate.kbps.load(Ordering::Acquire);
        let [ref mut render, ref mut gpu, ref mut encode, ref mut send] = self.stages_us;
        log(&format!(
            "xr host: poses {:.0}/s ({} no XR window) · built {:.0}/s ({} without a slot) · sent {:.0}/s {:.1} Mbit/s (rate {:.0}) · render {:.1} gpu {:.1} encode {:.1} send {:.1} (write {:.1}) ms · headset decode {:.1} ms · in flight {}",
            self.poses as f64 / secs,
            self.refused_poses,
            built as f64 / secs,
            skipped,
            self.pictures as f64 / secs,
            self.bytes as f64 * 8.0 / secs / 1e6,
            kbps as f64 / 1000.0,
            median_ms(render),
            median_ms(gpu),
            median_ms(encode),
            median_ms(send),
            median_ms(&mut self.write_us),
            median_ms(&mut self.decode_us),
            core.flow.in_flight(core.tiles),
        ));
        if self.bricks > 0 {
            log(&format!("xr host: room field {} bricks ({:.1} KB/s) from the headset", self.bricks, self.brick_bytes as f64 / secs / 1000.0));
        }
        let held = core.flow.held.swap(0, Ordering::AcqRel);
        core.rate.second(held as f64 / secs, self.bytes as f64 * 8.0 / secs / 1000.0, median_ms(&mut self.decode_us), core.fps, log);
        *self = PumpStats::new();
    }
}

/// What the encode side hands the session's TLS thread: an eye's access unit, when encoded.
enum Out {
    Video(VideoHead, Vec<u8>, Instant),
}

/// A picture the app packed (`makepad_xr_stream`) for the encoder: the shared texture (`handle`,
/// its DMA-BUF in the copy registry; the `nv12_picture` layout of the whole picture, both eye
/// columns), when the GPU has it (`ready`), the display time of the pose it was drawn with.
/// `held` is dropped once every eye's encode read the texture: the app may draw into it again.
pub struct Picture {
    pub handle: u64,
    pub ready: Ready,
    pub held: Box<dyn Send + Sync>,
    pub display_ns: u64,
    pub built_at: Instant,
    /// What the view centres on (m), for reprojection without per-pixel depth.
    pub focus_m: f32,
}

/// When a picture may be read: polled (on the picture thread, never the app's UI thread) until
/// the frame that rendered it was submitted and its fence published.
pub type Ready = Box<dyn FnMut() -> Option<Result<GpuReady, String>> + Send>;

/// Polls `ready` until it answers (the frame is submitted within a frame's time; 2 s is lost).
fn wait_ready(mut ready: Ready) -> Result<GpuReady, String> {
    let started = Instant::now();
    loop {
        match ready() {
            Some(r) => return r,
            None if started.elapsed() > Duration::from_secs(2) => return Err("the frame's fence was never published".into()),
            None => std::thread::sleep(Duration::from_micros(200)),
        }
    }
}

/// Pictures built and their eyes decoded by the headset (`Stats`, one per tile): the pictures in
/// flight, which bound how many the app builds ahead.
#[derive(Default)]
struct Flow {
    built_total: AtomicU64,
    decoded_tiles: AtomicU64,
    /// Pictures that never reach the headset (an encode that failed): out of flight.
    lost: AtomicU64,
    /// This window's (the stats line).
    built: AtomicU64,
    skipped: AtomicU64,
    /// The app asked for room and the headset had too many pictures in flight (this window).
    held: AtomicU64,
}

impl Flow {
    fn in_flight(&self, tiles: u32) -> u64 {
        let done = self.decoded_tiles.load(Ordering::Acquire) / tiles.max(1) as u64 + self.lost.load(Ordering::Acquire);
        self.built_total.load(Ordering::Acquire).saturating_sub(done)
    }
}

/// The stream's rate (all tiles): down when the link holds pictures back, back up while they
/// flow. kbit/s.
struct Rate {
    kbps: AtomicU32,
    start: u32,
    clean: AtomicU32,
}

impl Rate {
    /// One stats window: how often per second the app waited for the headset (`held`), the
    /// rate sent (kbit/s), the headset's median decode (ms).
    fn second(&self, held: f64, sent_kbps: f64, decode_ms: f64, fps: u32, log: &Log) {
        let kbps = self.kbps.load(Ordering::Acquire);
        let sent_kbps = sent_kbps as u32;
        let period_ms = 1000.0 / fps.max(1) as f64;
        // the app waited for pictures in flight a fifth of its frames while the headset decodes
        // within its period: the link holds them back
        let link_bound = held > fps as f64 * 0.2 && decode_ms < period_ms && sent_kbps > 0;
        let next = if link_bound {
            self.clean.store(0, Ordering::Release);
            ((sent_kbps as f64 * 0.85) as u32).max(MIN_KBPS)
        } else if self.clean.fetch_add(1, Ordering::AcqRel) >= 2 && kbps < self.start {
            (kbps as f64 * 1.1) as u32
        } else {
            kbps
        }
        .min(self.start);
        if next != kbps {
            self.kbps.store(next, Ordering::Release);
            log(&format!("xr host: rate {:.0} -> {:.0} Mbit/s ({})", kbps as f64 / 1000.0, next as f64 / 1000.0, if link_bound { "the link holds pictures back" } else { "pictures flow" }));
        }
    }
}

/// The lowest the rate controller goes (kbit/s, all tiles).
const MIN_KBPS: u32 = 15_000;
/// Pictures built and not yet decoded by the headset before the app waits (`room`):
/// `MAKEPAD_XR_REMOTE_IN_FLIGHT`, default 2.
fn in_flight_limit() -> u64 {
    std::env::var("MAKEPAD_XR_REMOTE_IN_FLIGHT").ok().and_then(|v| v.parse().ok()).unwrap_or(2u64).clamp(1, 8)
}

/// One eye's encode: what its thread is given.
struct Work {
    handle: u64,
    ready: GpuReady,
    _held: Arc<Box<dyn Send + Sync>>,
    head: VideoHead,
    built_at: Instant,
    kbps: u32,
}

/// A pose that came: when, the eyes it gives, its sequence number.
struct Arrived {
    at: Instant,
    seq: u64,
    eyes: [XrRenderEye; 2],
}

/// The live session's encode side (also the selftest's): the eye encoders and their threads.
struct Core {
    size: [u32; 2],
    tiles: u32,
    fps: u32,
    codec: u8,
    description: String,
    flow: Arc<Flow>,
    rate: Arc<Rate>,
    out: Receiver<Out>,
    keyframe: Arc<[AtomicBool; 2]>,
    poses: Arc<Mutex<VecDeque<(u64, Arrived)>>>,
    ending: Arc<AtomicBool>,
}

/// The session the app's pictures go to.
struct Live {
    eye: [u32; 2],
    tx: SyncSender<Picture>,
    flow: Arc<Flow>,
    tiles: u32,
    limit: u64,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);
type Wake = Arc<dyn Fn() + Send + Sync>;
static WAKE: Mutex<Option<Wake>> = Mutex::new(None);

fn wake_app() {
    let wake = WAKE.lock().unwrap().clone();
    if let Some(wake) = wake {
        wake();
    }
}

/// What the app is told when there is room again ([`room`]): a picture slot freed, or the
/// headset decoded one.
pub fn set_wake(wake: Wake) {
    *WAKE.lock().unwrap() = Some(wake);
}

/// The eye size (pixels) of the headset now streaming: the picture the app packs. None while
/// none streams.
pub fn session_eye() -> Option<[u32; 2]> {
    LIVE.lock().unwrap().as_ref().map(|l| l.eye)
}

/// Whether the app may build the next frame: a headset streams and fewer than its limit of
/// pictures are in flight to it.
pub fn room() -> bool {
    let live = LIVE.lock().unwrap();
    let Some(l) = live.as_ref() else { return false };
    let room = l.flow.in_flight(l.tiles) < l.limit;
    if !room {
        l.flow.held.fetch_add(1, Ordering::AcqRel);
    }
    room
}

/// A picture for the headset (see [`Picture`]); false when no headset streams (it is dropped,
/// and with it `held`).
pub fn submit(picture: Picture) -> bool {
    let live = LIVE.lock().unwrap();
    let Some(l) = live.as_ref() else { return false };
    l.flow.built_total.fetch_add(1, Ordering::AcqRel);
    l.flow.built.fetch_add(1, Ordering::AcqRel);
    match l.tx.try_send(picture) {
        Ok(()) => true,
        Err(_) => {
            // the picture thread is behind (cannot be with the app's slots): out of flight
            l.flow.lost.fetch_add(1, Ordering::AcqRel);
            false
        }
    }
}

/// The app found no free slot for a frame it built (counted on the stats line).
pub fn note_skipped() {
    if let Some(l) = LIVE.lock().unwrap().as_ref() {
        l.flow.skipped.fetch_add(1, Ordering::AcqRel);
    }
}

impl Core {
    fn start(eye: [u32; 2], fps: u32, codec: StreamVideoCodec, kbps: u32, log: Log) -> Result<Core, String> {
        let size = picture_size(eye);
        // a stream per eye on its own engine when the GPU has two
        let tiles = if VideoStreamEncoder::engines() >= 2 { 2 } else { 1 };
        let kbps = if kbps == 0 { 80_000 } else { kbps };
        let flow = Arc::new(Flow::default());
        let rate = Arc::new(Rate { kbps: AtomicU32::new(kbps), start: kbps, clean: AtomicU32::new(0) });
        let keyframe: Arc<[AtomicBool; 2]> = Arc::new([AtomicBool::new(true), AtomicBool::new(true)]);
        let ending = Arc::new(AtomicBool::new(false));
        let poses: Arc<Mutex<VecDeque<(u64, Arrived)>>> = Arc::default();
        let (out_tx, out) = channel::<Out>();
        let codec_byte = if codec == StreamVideoCodec::Hevc { CODEC_HEVC } else { CODEC_H264 };
        let mut workers = Vec::new();
        let mut description = String::new();
        for t in 0..tiles {
            let w = size[0] / tiles;
            let options = VideoStreamEncoderOptions { codec, width: w, height: size[1], fps, bitrate_kbps: kbps / tiles, keyint: 0, low_latency: true, full_chroma: false };
            let tile = StreamTile { engine: t, x0: t * w, y0: 0, source_height: size[1] };
            // the fastest encode (latency first), a constant rate whose pictures fit the link
            // (`MAKEPAD_XR_REMOTE_SHARP=1`: the encoder's best pictures per bit, more encode time)
            let sharp = std::env::var("MAKEPAD_XR_REMOTE_SHARP").as_deref() == Ok("1");
            let tuning = StreamTuning { quality: 0, constant_rate: true, buffer_pictures: 1, sharp };
            let mut encoder = VideoStreamEncoder::new_tuned(options, tile, tuning).map_err(|e| format!("encoder: {e:?}"))?;
            if t == 0 {
                description = encoder.description();
            }
            let (tx, rx) = channel::<Work>();
            let (out_tx, keyframe, flow, log) = (out_tx.clone(), keyframe.clone(), flow.clone(), log.clone());
            let w_px = w;
            std::thread::Builder::new()
                .name(format!("xr-host-encode-{t}"))
                .spawn(move || {
                    let mut applied = kbps / tiles;
                    let mut pts = 0i64;
                    let mut foveated = false;
                    while let Ok(w) = rx.recv() {
                        // the eye's foveation, once its field of view is known (its first picture)
                        if !foveated && w.head.eyes[t as usize].fov != [0.0; 4] {
                            foveated = true;
                            if let (Some(grid), Some(peak)) = (encoder.qp_grid(), foveation_peak()) {
                                let map = foveation(grid, [w_px, size[1]], w.head.eyes[t as usize].fov, peak);
                                if encoder.set_qp_map(&map) {
                                    log(&format!("xr host: eye {t}: foveated encode ({}x{} QP regions of {}x{} px, +{peak} QP at the edge)", grid.cols, grid.rows, grid.texel[0], grid.texel[1]));
                                }
                            }
                        }
                        let tile_kbps = (w.kbps / tiles).max(1);
                        if tile_kbps != applied {
                            encoder.set_bitrate_kbps(tile_kbps);
                            applied = tile_kbps;
                        }
                        if keyframe[t as usize].swap(false, Ordering::AcqRel) {
                            encoder.request_keyframe();
                        }
                        pts += 1;
                        let push_at = Instant::now();
                        let queued = push_at.duration_since(w.built_at);
                        let packets = match encoder.push_shared_texture(w.handle, w.ready, pts * 138_888) {
                            Ok(p) => p,
                            Err(e) => {
                                log(&format!("xr host: eye {t}: encode: {e:?}"));
                                flow.lost.fetch_add(1, Ordering::AcqRel);
                                keyframe[t as usize].store(true, Ordering::Release);
                                continue;
                            }
                        };
                        // the push waited for the frame's GPU work, copied the column, encoded it
                        let push = push_at.elapsed();
                        let encode = Duration::from_micros(encoder.last_encode_us() as u64).min(push);
                        if t == 0 {
                            dump_picture(w.handle, size, &log);
                        }
                        let mut head = w.head;
                        head.tile = t as u8;
                        head.stages[stage::GPU] = (queued + push - encode).as_micros() as u32;
                        head.stages[stage::ENCODE] = encode.as_micros() as u32;
                        drop(w._held);
                        for p in packets {
                            head.key = p.is_key;
                            if out_tx.send(Out::Video(head, p.data, Instant::now())).is_err() {
                                return;
                            }
                        }
                        wake_app();
                    }
                })
                .map_err(|e| e.to_string())?;
            workers.push(tx);
        }
        // the app's pictures: matched to their poses, fanned out to the eyes
        let (tx, rx) = sync_channel::<Picture>(4);
        {
            let (poses, rate, ending, out_tx, flow, log) = (poses.clone(), rate.clone(), ending.clone(), out_tx.clone(), flow.clone(), log.clone());
            std::thread::Builder::new()
                .name("xr-host-pictures".into())
                .spawn(move || {
                    while !ending.load(Ordering::Acquire) {
                        let Ok(p) = rx.recv_timeout(Duration::from_millis(100)) else { continue };
                        let pose = {
                            let mut poses = poses.lock().unwrap();
                            let found = poses.iter().position(|(d, _)| *d == p.display_ns);
                            found.map(|i| {
                                let pose = poses.remove(i).unwrap().1;
                                // older poses were never drawn: gone
                                poses.drain(..i);
                                pose
                            })
                        };
                        let (seq, eyes, render_us) = match &pose {
                            Some(a) => (a.seq, a.eyes, p.built_at.saturating_duration_since(a.at).as_micros() as u32),
                            None => (0, [XrRenderEye::default(); 2], 0),
                        };
                        let head = VideoHead { seq, tile: 0, key: false, display_ns: p.display_ns, eyes, focus_m: p.focus_m, stages: [render_us, 0, 0, 0], size, codec: codec_byte };
                        let held: Arc<Box<dyn Send + Sync>> = Arc::new(p.held);
                        let kbps = rate.kbps.load(Ordering::Acquire);
                        let mut ready = match wait_ready(p.ready) {
                            Ok(r) => Some(r),
                            Err(e) => {
                                log(&format!("xr host: {e}"));
                                flow.lost.fetch_add(1, Ordering::AcqRel);
                                continue;
                            }
                        };
                        for (t, worker) in workers.iter().enumerate() {
                            // each eye waits for the same frame: its own copy of the sync_file
                            let r = if t + 1 == workers.len() { ready.take().unwrap_or(GpuReady::Done) } else { clone_ready(ready.as_ref().unwrap()) };
                            let _ = worker.send(Work { handle: p.handle, ready: r, _held: held.clone(), head, built_at: p.built_at, kbps });
                        }
                    }
                    drop(out_tx);
                })
                .map_err(|e| e.to_string())?;
        }
        *LIVE.lock().unwrap() = Some(Live { eye, tx, flow: flow.clone(), tiles, limit: in_flight_limit() });
        wake_app();
        Ok(Core { size, tiles, fps, codec: codec_byte, description, flow, rate, out, keyframe, poses, ending })
    }

    /// A pose from the headset: to the app's XR window, and kept to match its picture.
    fn pose(&self, seq: u64, display_ns: u64, frame: &[u8]) -> bool {
        {
            let mut poses = self.poses.lock().unwrap();
            poses.push_back((display_ns, Arrived { at: Instant::now(), seq, eyes: render_eyes(frame) }));
            // poses never drawn (the newest one wins in the HAL): the oldest go
            while poses.len() > 256 {
                poses.pop_front();
            }
        }
        hal::hal_xr_remote_pose(seq, display_ns, frame)
    }

    fn keyframe(&self) {
        for k in self.keyframe.iter() {
            k.store(true, Ordering::Release);
        }
    }

    /// The headset decoded one eye of a picture.
    fn tile_decoded(&self) {
        self.flow.decoded_tiles.fetch_add(1, Ordering::AcqRel);
        wake_app();
    }

    fn stop(&self) {
        self.ending.store(true, Ordering::Release);
        let mut live = LIVE.lock().unwrap();
        if live.as_ref().is_some_and(|l| Arc::ptr_eq(&l.flow, &self.flow)) {
            *live = None;
        }
    }
}

#[cfg(target_os = "linux")]
fn clone_ready(ready: &GpuReady) -> GpuReady {
    match ready {
        GpuReady::Done => GpuReady::Done,
        GpuReady::SyncFile(fd) => fd.try_clone().map(GpuReady::SyncFile).unwrap_or(GpuReady::Done),
    }
}

#[cfg(not(target_os = "linux"))]
fn clone_ready(_ready: &GpuReady) -> GpuReady {
    GpuReady::Done
}

/// A headset on a loop (`MAKEPAD_XR_REMOTE_SELFTEST=1`): synthetic poses at 72 Hz (the head turning
/// slowly, eyes 64 mm apart, ±45° fields) drive the app's XR window exactly as a headset's would, and
/// every picture is encoded and measured, then dropped (and counted decoded at once). The server
/// side's numbers without a headset.
pub fn selftest(eye: [u32; 2], kbps: u32, log: Log) {
    use makepad_hal::{buffers::Wire, XrEye, XrFrame, XrPose};
    let core = match Core::start(eye, 72, StreamVideoCodec::Hevc, kbps, log.clone()) {
        Ok(c) => c,
        Err(e) => return log(&format!("xr selftest: {e}")),
    };
    log(&format!("xr selftest: {}x{} HEVC in {} tile(s) on {}", core.size[0], core.size[1], core.tiles, core.description));
    hal::hal_xr_remote_attach(eye[0], eye[1], 72_000);
    let _ = std::thread::Builder::new().name("xr-selftest".into()).spawn(move || {
        let start = Instant::now();
        let mut seq = 0u64;
        let mut stats = PumpStats::new();
        loop {
            seq += 1;
            let t = start.elapsed().as_secs_f32();
            let yaw = (t * 0.5).sin() * 0.6;
            let q = [0.0, (yaw * 0.5).sin(), 0.0, (yaw * 0.5).cos()];
            let right = [yaw.cos(), 0.0, -yaw.sin()];
            let at = |s: f32| XrPose { orientation: q, position: [right[0] * s * 0.032, 1.6, right[2] * s * 0.032] };
            let fov = [-0.785, 0.785, 0.785, -0.785];
            let frame = XrFrame { head: at(0.0), eyes: [XrEye { pose: at(-1.0), fov }, XrEye { pose: at(1.0), fov }], ..Default::default() };
            let mut wire = Vec::with_capacity(<XrFrame as Wire>::SIZE);
            frame.write(&mut wire);
            stats.poses += 1;
            if !core.pose(seq, makepad_hal::clock_ns() + 30_000_000, &wire) {
                stats.refused_poses += 1;
            }
            // drained as they come (a headset decodes on arrival), the next pose at 72 Hz
            let next = Instant::now() + Duration::from_micros(13_889);
            loop {
                while let Ok(m) = core.out.try_recv() {
                    let Out::Video(mut head, data, encoded_at) = m;
                    head.stages[stage::SEND] = encoded_at.elapsed().as_micros() as u32;
                    stats.sent(&head, data.len(), Duration::ZERO);
                    core.tile_decoded();
                }
                let now = Instant::now();
                if now >= next {
                    break;
                }
                std::thread::sleep((next - now).min(Duration::from_millis(1)));
            }
            stats.window(&core, &log);
        }
    });
}

/// The foveation's QP delta at the edge of the view (`MAKEPAD_XR_REMOTE_FOVEATE`: a number, or 0
/// for none; default 10: about a third of the bits per pixel out there).
fn foveation_peak() -> Option<i8> {
    let peak = std::env::var("MAKEPAD_XR_REMOTE_FOVEATE").ok().and_then(|v| v.parse::<i8>().ok()).unwrap_or(10);
    (peak > 0).then_some(peak)
}

/// An eye column's QP deltas on the encoder's grid: the colour (the top two thirds) at full
/// quality within 15 degrees of the eye's axis, then fewer bits outwards to `peak` at 50 degrees
/// (the lens blurs the edge; the eye looks through the centre); the band below: coverage as the
/// centre (its edges are the composite's edges), depth with fewer bits.
fn foveation(grid: QpGrid, size: [u32; 2], fov: [f32; 4], peak: i8) -> Vec<i8> {
    let [l, r, u, d] = fov.map(f32::tan);
    let colour_h = size[1] * 2 / 3;
    let mut map = Vec::with_capacity((grid.cols * grid.rows) as usize);
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let x = (col * grid.texel[0] + grid.texel[0] / 2).min(size[0] - 1) as f32;
            let y = row * grid.texel[1] + grid.texel[1] / 2;
            let delta = if y < colour_h {
                let tx = l + (x + 0.5) / size[0] as f32 * (r - l);
                let ty = u - (y as f32 + 0.5) / colour_h as f32 * (u - d);
                let angle = (tx * tx + ty * ty).sqrt().atan().to_degrees();
                let k = ((angle - 15.0) / 35.0).clamp(0.0, 1.0);
                (k * peak as f32).round() as i32
            } else if x < size[0] as f32 * 0.5 {
                0
            } else {
                (peak as i32 / 2).max(1)
            };
            map.push(delta.clamp(grid.min.max(-128), grid.max.min(127)) as i8);
        }
    }
    map
}

/// `MAKEPAD_XR_REMOTE_DUMP=<dir>`: the 300th picture's planes as `<dir>/picture.pgm` (its luma:
/// both eye columns, each its colour over its band of coverage and inverse depth) once its encode
/// read it, and a line of the band's numbers. A check of what the GPU packed.
#[cfg(not(target_os = "linux"))]
fn dump_picture(_handle: u64, _size: [u32; 2], _log: &Log) {}

/// (shared pictures are DMA-BUFs: Linux)
#[cfg(target_os = "linux")]
fn dump_picture(handle: u64, size: [u32; 2], log: &Log) {
    static N: AtomicU32 = AtomicU32::new(0);
    let Some(dir) = std::env::var_os("MAKEPAD_XR_REMOTE_DUMP") else { return };
    if N.fetch_add(1, Ordering::AcqRel) != 300 {
        return;
    }
    let [w, h] = size;
    let mut px = vec![0u8; (w / 4 * h * 4) as usize];
    if makepad_hal::dmabuf::hal_dmabuf_read(handle, &[0, 0, w / 4, h], &mut px) < 0 {
        return log("xr host: dump: the picture could not be read");
    }
    // a BGRA texel holds luma samples 0..3 in its bytes as they lie: the row is the plane's row
    let mut pgm = format!("P5\n{w} {h}\n255\n").into_bytes();
    pgm.extend_from_slice(&px);
    let path = std::path::Path::new(&dir).join("picture.pgm");
    let _ = std::fs::write(&path, &pgm);
    // the band of the left eye: coverage (left half) and inverse depth (right half)
    let (ew, eh) = (w / 2, h * 2 / 3);
    let band = |x0: u32, x1: u32| {
        let (mut n, mut sum, mut max, mut nonzero) = (0u64, 0u64, 0u8, 0u64);
        for y in eh..h {
            for x in x0..x1 {
                let v = px[(y * w + x) as usize];
                n += 1;
                sum += v as u64;
                max = max.max(v);
                nonzero += (v > 0) as u64;
            }
        }
        (sum as f64 / n.max(1) as f64, max, nonzero as f64 / n.max(1) as f64)
    };
    let (a_mean, a_max, a_cov) = band(0, ew / 2);
    let (d_mean, d_max, d_cov) = band(ew / 2, ew);
    let near = if d_max > 0 { crate::xr::DEPTH_NEAR_M * 255.0 / d_max as f32 } else { 0.0 };
    log(&format!("xr host: dumped {} ({w}x{h}); left band coverage mean {a_mean:.1} max {a_max} drawn {:.0}%, inverse depth mean {d_mean:.1} max {d_max} (nearest {near:.2} m) drawn {:.0}%", path.display(), a_cov * 100.0, d_cov * 100.0));
}

/// The eyes of an `XrFrame` (wire form).
fn render_eyes(frame: &[u8]) -> [XrRenderEye; 2] {
    let Some(f) = makepad_hal::buffers::read_items::<makepad_hal::XrFrame>(frame, 1).next() else { return [XrRenderEye::default(); 2] };
    f.eyes.map(|e| XrRenderEye { orientation: e.pose.orientation, position: e.pose.position, fov: e.fov })
}

/// The picture for eyes of `eye` pixels: both eye columns side by side, each its colour over a
/// band half its height (coverage and inverse depth at half resolution; `crate::xr`).
pub fn picture_size(eye: [u32; 2]) -> [u32; 2] {
    [eye[0] * 2, eye[1] * 3 / 2]
}
