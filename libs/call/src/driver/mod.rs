//! A running call: the workers behind the sans-IO core, and the handle the UI thread holds.
//!
//! [`Call::start`] spawns three long-lived workers (network, encode, decode) through the caller's
//! [`Spawn`] (Work passes its thread spawner; tests and the probe pass [`std_spawn`]). Every
//! method of [`Call`] returns at once: the UI thread sends commands on channels and reads what the
//! workers published with `try_recv` and atomics. Voice runs in the app's audio callbacks through
//! [`voice::VoiceIn`] and [`voice::VoiceOut`], which never allocate or wait.
//!
//! Media leaves through each participant's link (`Join::out`) and arrives through one inlet
//! ([`Call::inlet`]) that every link feeds. A synthetic call opens no device: the codecs and
//! sources are the only ways to media, and this module calls no platform capture.

pub mod codec;
mod decode;
mod encode;
mod net;
pub mod synth;
pub mod voice;

use codec::{Codecs, Picture};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::Instant;
use synth::Synth;
use voice::VoiceMarks;

/// The caller's name for a participant (Work: an index of the linked device).
pub type PeerId = u64;

/// Starts one long-lived worker thread: a name and the loop it runs.
pub type Spawn = Arc<dyn Fn(&'static str, Box<dyn FnOnce() + Send>) + Send + Sync>;

/// [`Spawn`] on plain threads, for tests and the probe.
pub fn std_spawn() -> Spawn {
    Arc::new(|name, f| {
        std::thread::Builder::new().name(name.to_string()).spawn(f).expect("a call worker thread");
    })
}

/// How many datagrams the inlet holds before a link drops and counts them.
const INLET: usize = 1024;
/// Camera frames waiting for the encoder: the newest matters, an older one is dropped.
const CAMERA_QUEUE: usize = 2;

pub struct CallConfig {
    pub call_id: [u8; 16],
    /// This device's key, as the link proved it.
    pub my_pub: Vec<u8>,
    /// None: voice only.
    pub video: Option<VideoConfig>,
    pub voice: VoiceSource,
    pub codecs: Arc<dyn Codecs>,
    pub spawn: Spawn,
    /// Wakes the UI when there is something to poll (Work: its UI signal).
    pub signal: Arc<dyn Fn() + Send + Sync>,
    /// The media clock's zero: capture times are µs since it.
    pub clock: Instant,
}

#[derive(Clone, Copy, Debug)]
pub struct VideoConfig {
    /// The best rung the sender may use (an index into `guard::LADDER`; 0 is the best).
    pub top: usize,
    /// The most the encoder is asked for, kbit/s.
    pub max_kbps: u32,
    pub source: VideoSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoSource {
    /// The app's camera callback pushes frames through [`Call::video_in`].
    Feed,
    Synthetic(Synth),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VoiceSource {
    /// The app's audio input callback pushes through [`Call::voice_in`].
    Feed,
    /// A tone in talk spurts, from a worker of its own.
    Synthetic { tone_hz: f32 },
}

/// A participant joins the media: its link and its secrets (W2 exchanges them in the link's
/// encrypted stream).
pub struct Join {
    pub peer: PeerId,
    pub their_pub: Vec<u8>,
    pub key_id: u8,
    pub send_secret: [u8; 32],
    pub recv_secret: [u8; 32],
    /// This participant's link: datagrams for it. A full link keeps them for the next wake.
    pub out: SyncSender<Vec<u8>>,
    /// The link stream's rate cap, bit/s (0: none), set while media flows.
    pub stream_cap: Option<Arc<AtomicU64>>,
}

enum NetCommand {
    Join(Join, Arc<VoiceMarks>),
    Rekey { peer: PeerId, key_id: u8, send: [u8; 32], recv: [u8; 32] },
    Leave(PeerId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileState {
    /// No media from this participant yet.
    Connecting,
    Live,
    /// Its path is struggling: loss or a low estimate.
    Weak,
}

pub struct Tile {
    /// None: this device.
    pub who: Option<PeerId>,
    pub picture: Option<Arc<Picture>>,
    /// The newest picture's capture time on its sender's clock, µs.
    pub capture_us: u64,
    /// `poll` replaced the picture: upload it.
    pub fresh: bool,
    pub talking: bool,
    pub level: f32,
    pub muted: bool,
    pub camera_off: bool,
    pub state: TileState,
    marks: Arc<VoiceMarks>,
}

/// Per participant, for a details view; never needed to use the call.
#[derive(Clone, Debug, Default)]
pub struct PeerStats {
    pub peer: PeerId,
    pub rtt_us: Option<u64>,
    pub estimate_bps: u64,
    /// The random-loss level the estimator learned.
    pub random_loss: f64,
    pub session: crate::session::PeerStats,
    pub video: Option<crate::video::ReceiverStats>,
    /// Datagrams waiting because the link's queue was full, and the longest any of them had
    /// waited since the last stats, in µs.
    pub backlog: usize,
    pub backlog_us: u64,
    pub link_full: u64,
}

#[derive(Clone, Debug, Default)]
pub struct CallStats {
    pub peers: Vec<PeerStats>,
    pub rung: Option<crate::guard::Rung>,
    pub asked_kbps: u32,
    pub pictures_sent: u64,
    pub keys_sent: u64,
    pub encoder: String,
    pub decoder: String,
    pub voice_dropped: u64,
    /// Media from no one in the call, kept a second for a join that never came.
    pub strays: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallEvent {
    /// The first media from this participant.
    Live(PeerId),
    /// A person-facing reason something doesn't work, e.g. the encoder couldn't open.
    Problem { peer: Option<PeerId>, what: String },
    /// Every worker has exited.
    Stopped,
}

enum UiMsg {
    Picture { peer: Option<PeerId>, picture: Arc<Picture>, capture_us: u64 },
    Stats { peers: Vec<PeerStats>, voice_dropped: u64, strays: u64 },
    Encoder(String),
    Decoder(String),
    Event(CallEvent),
    WorkerDone,
}

struct Shared {
    clock: Instant,
    stop: AtomicBool,
    workers: AtomicUsize,
    /// Published by the network worker for the encoder: the lowest estimate over the peers, the
    /// longest pacer delay, the highest video loss level (f64 bits, NaN while unknown), a key frame asked for.
    estimate_bps: AtomicU64,
    pacer_delay_us: AtomicU64,
    loss_level_bits: AtomicU64,
    key_wanted: AtomicBool,
    camera_on: AtomicBool,
    muted: Arc<AtomicBool>,
    /// Published by the encoder for the stats: its rung (`usize::MAX`: none yet), ask and counts.
    rung: AtomicUsize,
    asked_kbps: AtomicU64,
    pictures_sent: AtomicU64,
    keys_sent: AtomicU64,
}

impl Shared {
    fn now_us(&self) -> u64 {
        self.clock.elapsed().as_micros() as u64
    }
}

/// A doorbell for the network worker: an empty datagram in the inlet.
fn ring(inlet: &SyncSender<(SocketAddr, Vec<u8>)>) {
    let _ = inlet.try_send((SocketAddr::from(([0, 0, 0, 0], 0)), Vec::new()));
}

/// The camera callback's end: NV12 frames of any size (the encoder scales to its rung).
#[derive(Clone)]
pub struct VideoIn {
    frames: SyncSender<CameraFrame>,
    spare: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    clock: Instant,
}

pub(crate) struct CameraFrame {
    width: u32,
    height: u32,
    nv12: Vec<u8>,
    capture_us: u64,
}

impl VideoIn {
    /// One camera picture, 4:2:0 with interleaved chroma (NV12). If the encoder is behind the
    /// frame is dropped: a newer one comes.
    pub fn push_nv12(&self, width: u32, height: u32, nv12: &[u8]) {
        let capture_us = self.clock.elapsed().as_micros() as u64;
        let mut buf = self.spare.try_lock().ok().and_then(|mut s| s.pop()).unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(nv12);
        let _ = self.frames.try_send(CameraFrame { width, height, nv12: buf, capture_us });
    }
}

/// The UI thread's handle. Dropping it stops the call.
pub struct Call {
    shared: Arc<Shared>,
    inlet: SyncSender<(SocketAddr, Vec<u8>)>,
    commands: Sender<NetCommand>,
    ui: Receiver<UiMsg>,
    recycle: Sender<Arc<Picture>>,
    voice_in: Option<voice::VoiceIn>,
    voice_out: Option<voice::VoiceOut>,
    video_in: Option<VideoIn>,
    tiles: Vec<Tile>,
    stats: CallStats,
    spawned: usize,
    done: usize,
    stopped: bool,
}

impl Call {
    /// Starts this side of a call: its workers, and the synthetic sources it was given. It opens no
    /// device: a real camera and microphone feed it through [`Call::video_in`] and
    /// [`Call::voice_in`], which the app wires to its own capture.
    pub fn start(config: CallConfig) -> Call {
        let muted = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            clock: config.clock,
            stop: AtomicBool::new(false),
            workers: AtomicUsize::new(0),
            estimate_bps: AtomicU64::new(crate::session::START_BPS),
            pacer_delay_us: AtomicU64::new(0),
            loss_level_bits: AtomicU64::new(f64::NAN.to_bits()),
            key_wanted: AtomicBool::new(true),
            camera_on: AtomicBool::new(config.video.is_some()),
            muted: muted.clone(),
            rung: AtomicUsize::new(usize::MAX),
            asked_kbps: AtomicU64::new(0),
            pictures_sent: AtomicU64::new(0),
            keys_sent: AtomicU64::new(0),
        });
        let (inlet, inlet_rx) = sync_channel(INLET);
        let (commands, commands_rx) = channel();
        let (ui_tx, ui) = channel();
        let (video_tx, video_rx) = channel();
        let (frames_tx, frames_rx) = channel();
        let (recycle, recycle_rx) = channel();
        let my_marks = Arc::new(VoiceMarks::default());
        let (voice_in, voice_frames) = voice::voice_in(config.clock, muted, my_marks.clone());
        let (voice_out, mix) = voice::voice_out();
        let signal = config.signal.clone();
        let mut spawned = 0;

        let net = net::Net::new(&config, shared.clone(), inlet_rx, commands_rx, video_rx, frames_tx, voice_frames, mix, ui_tx.clone(), signal.clone());
        spawn_worker(&config.spawn, "call network", shared.clone(), ui_tx.clone(), signal.clone(), move || net.run());
        spawned += 1;

        let decode = decode::Decode::new(config.codecs.clone(), shared.clone(), frames_rx, ui_tx.clone(), signal.clone());
        spawn_worker(&config.spawn, "call decode", shared.clone(), ui_tx.clone(), signal.clone(), move || decode.run());
        spawned += 1;

        let mut video_in = None;
        if let Some(video) = config.video {
            let (camera_tx, camera_rx) = sync_channel(CAMERA_QUEUE);
            let spare = Arc::new(std::sync::Mutex::new(Vec::new()));
            if video.source == VideoSource::Feed {
                video_in = Some(VideoIn { frames: camera_tx, spare: spare.clone(), clock: config.clock });
            }
            let encode = encode::Encode::new(video, config.codecs.clone(), shared.clone(), camera_rx, spare, video_tx, inlet.clone(), recycle_rx, ui_tx.clone(), signal.clone());
            spawn_worker(&config.spawn, "call encode", shared.clone(), ui_tx.clone(), signal.clone(), move || encode.run());
            spawned += 1;
        }

        let mut voice_in = Some(voice_in);
        if let VoiceSource::Synthetic { tone_hz } = config.voice {
            let mut input = voice_in.take().unwrap();
            let stop = shared.clone();
            spawn_worker(&config.spawn, "call synthetic voice", shared.clone(), ui_tx.clone(), signal, move || {
                let mut voice = synth::SynthVoice::new(tone_hz);
                let mut block = [0f32; synth::SYNTH_BLOCK];
                let start = Instant::now();
                for n in 0u32.. {
                    if let Some(wait) = (start + std::time::Duration::from_millis(10) * n).checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                    if stop.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    voice.next_block(&mut block);
                    input.push_mono(synth::SYNTH_RATE, &block);
                }
            });
            spawned += 1;
        }

        let me = Tile {
            who: None,
            picture: None,
            capture_us: 0,
            fresh: false,
            talking: false,
            level: 0.0,
            muted: false,
            camera_off: config.video.is_none(),
            state: TileState::Live,
            marks: my_marks,
        };
        Call {
            shared,
            inlet,
            commands,
            ui,
            recycle,
            voice_in,
            voice_out: Some(voice_out),
            video_in,
            tiles: vec![me],
            stats: CallStats::default(),
            spawned,
            done: 0,
            stopped: false,
        }
    }

    /// Where every participant's link puts this call's datagrams (0xE2), with the sender's address.
    pub fn inlet(&self) -> SyncSender<(SocketAddr, Vec<u8>)> {
        self.inlet.clone()
    }

    /// The audio input callback's half, once (none for a synthetic voice).
    pub fn voice_in(&mut self) -> Option<voice::VoiceIn> {
        self.voice_in.take()
    }

    /// The audio output callback's half, once.
    pub fn voice_out(&mut self) -> Option<voice::VoiceOut> {
        self.voice_out.take()
    }

    /// The camera callback's end (only for `VideoSource::Feed`).
    pub fn video_in(&self) -> Option<VideoIn> {
        self.video_in.clone()
    }

    pub fn join(&mut self, join: Join) {
        if !self.tiles.iter().any(|t| t.who == Some(join.peer)) {
            let marks = Arc::new(VoiceMarks::default());
            self.tiles.push(Tile {
                who: Some(join.peer),
                picture: None,
                capture_us: 0,
                fresh: false,
                talking: false,
                level: 0.0,
                muted: false,
                camera_off: false,
                state: TileState::Connecting,
                marks,
            });
        }
        let marks = self.tiles.iter().find(|t| t.who == Some(join.peer)).unwrap().marks.clone();
        self.send(NetCommand::Join(join, marks));
    }

    /// New secrets for a participant (a new epoch after its link came back).
    pub fn rekey(&mut self, peer: PeerId, key_id: u8, send: [u8; 32], recv: [u8; 32]) {
        self.send(NetCommand::Rekey { peer, key_id, send, recv });
    }

    pub fn leave(&mut self, peer: PeerId) {
        self.tiles.retain(|t| t.who != Some(peer));
        self.send(NetCommand::Leave(peer));
    }

    /// Silent voice frames from now on (the framer keeps the stream's clock).
    pub fn set_muted(&mut self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
        self.tiles[0].muted = muted;
    }

    /// Stops or restarts encoding; with it off no encoder stays open.
    pub fn set_camera(&mut self, on: bool) {
        self.shared.camera_on.store(on, Ordering::Relaxed);
        self.tiles[0].camera_off = !on;
        if !on {
            self.tiles[0].picture = None;
        }
    }

    /// A participant's marks, from the call's signalling.
    pub fn set_peer_marks(&mut self, peer: PeerId, muted: bool, camera_off: bool) {
        if let Some(t) = self.tiles.iter_mut().find(|t| t.who == Some(peer)) {
            t.muted = muted;
            t.camera_off = camera_off;
            if camera_off {
                t.picture = None;
            }
        }
    }

    fn send(&self, command: NetCommand) {
        let _ = self.commands.send(command);
        ring(&self.inlet);
    }

    /// What happened since the last poll; keeps the newest picture per tile. `try_recv` only.
    pub fn poll(&mut self) -> Vec<CallEvent> {
        let mut events = Vec::new();
        for t in &mut self.tiles {
            t.fresh = false;
        }
        loop {
            match self.ui.try_recv() {
                Ok(UiMsg::Picture { peer, picture, capture_us }) => {
                    let Some(t) = self.tiles.iter_mut().find(|t| t.who == peer) else { continue };
                    if t.camera_off {
                        continue;
                    }
                    if t.state == TileState::Connecting {
                        t.state = TileState::Live;
                    }
                    if let Some(old) = t.picture.replace(picture) {
                        if peer.is_none() {
                            let _ = self.recycle.send(old);
                        }
                    }
                    t.capture_us = capture_us;
                    t.fresh = true;
                }
                Ok(UiMsg::Stats { peers, voice_dropped, strays }) => {
                    for p in &peers {
                        if let Some(t) = self.tiles.iter_mut().find(|t| t.who == Some(p.peer)) {
                            let live = p.session.received > 0;
                            let weak = p.video.is_some_and(|v| v.frames > 0) && p.estimate_bps < 120_000;
                            t.state = match (live, weak) {
                                (false, _) => TileState::Connecting,
                                (true, true) => TileState::Weak,
                                (true, false) => TileState::Live,
                            };
                        }
                    }
                    self.stats.peers = peers;
                    self.stats.voice_dropped = voice_dropped;
                    self.stats.strays = strays;
                }
                Ok(UiMsg::Encoder(name)) => self.stats.encoder = name,
                Ok(UiMsg::Decoder(name)) => self.stats.decoder = name,
                Ok(UiMsg::Event(e)) => {
                    if let CallEvent::Live(peer) = e {
                        if let Some(t) = self.tiles.iter_mut().find(|t| t.who == Some(peer)) {
                            t.state = TileState::Live;
                        }
                    }
                    events.push(e);
                }
                Ok(UiMsg::WorkerDone) => {
                    self.done += 1;
                    if self.done == self.spawned {
                        events.push(CallEvent::Stopped);
                    }
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        for t in &mut self.tiles {
            t.level = t.marks.level();
            t.talking = t.marks.talking() && !t.muted;
        }
        let rung = self.shared.rung.load(Ordering::Relaxed);
        self.stats.rung = crate::guard::LADDER.get(rung).copied();
        self.stats.asked_kbps = self.shared.asked_kbps.load(Ordering::Relaxed) as u32;
        self.stats.pictures_sent = self.shared.pictures_sent.load(Ordering::Relaxed);
        self.stats.keys_sent = self.shared.keys_sent.load(Ordering::Relaxed);
        events
    }

    /// One tile per participant, this device first.
    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    pub fn stats(&self) -> &CallStats {
        &self.stats
    }

    /// Tells every worker to stop. They exit within one wake; nothing here waits for them.
    pub fn stop(&mut self) {
        if !self.stopped {
            self.stopped = true;
            self.shared.stop.store(true, Ordering::Relaxed);
            ring(&self.inlet);
        }
    }

    /// Every worker has exited (seen through `poll`).
    pub fn stopped(&self) -> bool {
        self.done == self.spawned
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_worker(
    spawn: &Spawn,
    name: &'static str,
    shared: Arc<Shared>,
    ui: Sender<UiMsg>,
    signal: Arc<dyn Fn() + Send + Sync>,
    f: impl FnOnce() + Send + 'static,
) {
    shared.workers.fetch_add(1, Ordering::Relaxed);
    spawn(name, Box::new(move || {
        f();
        shared.workers.fetch_sub(1, Ordering::Relaxed);
        let _ = ui.send(UiMsg::WorkerDone);
        signal();
    }));
}

#[cfg(test)]
mod tests {
    use super::codec::{Decoder, Encoder, TestCodecs};
    use super::*;
    use crate::guard::{Rung, LADDER};
    use std::time::Duration;

    fn device(n: u8) -> Vec<u8> {
        let mut k = vec![4u8; 65];
        k[1] = n;
        k
    }

    /// The rung of 320x180 at 30 fps, so the synthetic pictures stay cheap.
    fn small() -> usize {
        LADDER.iter().position(|r| r.width == 320 && r.fps == 30).unwrap()
    }

    /// One direction of an in-process link: what one call sends arrives at the other's inlet. The
    /// link holds `slots` datagrams; with `burst_ms` it is read only that often (a slow link).
    fn link(to: SyncSender<(SocketAddr, Vec<u8>)>, slots: usize, burst_ms: u64) -> SyncSender<Vec<u8>> {
        let (tx, rx) = sync_channel::<Vec<u8>>(slots);
        std::thread::spawn(move || {
            let from = SocketAddr::from(([192, 0, 2, 1], 47100));
            while let Ok(d) = rx.recv() {
                if to.send((from, d)).is_err() {
                    return;
                }
                if burst_ms > 0 {
                    std::thread::sleep(Duration::from_millis(burst_ms));
                }
                while let Ok(d) = rx.try_recv() {
                    if to.send((from, d)).is_err() {
                        return;
                    }
                }
            }
        });
        tx
    }

    /// A link that carries at most `rate_bps` (0: no limit), changeable while the call runs.
    fn throttled_link(to: SyncSender<(SocketAddr, Vec<u8>)>, slots: usize, rate_bps: Arc<AtomicU64>) -> SyncSender<Vec<u8>> {
        let (tx, rx) = sync_channel::<Vec<u8>>(slots);
        std::thread::spawn(move || {
            let from = SocketAddr::from(([192, 0, 2, 1], 47100));
            while let Ok(d) = rx.recv() {
                let bps = rate_bps.load(Ordering::Relaxed);
                if bps > 0 {
                    std::thread::sleep(Duration::from_micros(d.len() as u64 * 8_000_000 / bps));
                }
                if to.send((from, d)).is_err() {
                    return;
                }
            }
        });
        tx
    }

    fn config(n: u8, video: bool, tone_hz: f32, codecs: Arc<dyn Codecs>) -> CallConfig {
        CallConfig {
            call_id: [7; 16],
            my_pub: device(n),
            video: video.then_some(VideoConfig { top: small(), max_kbps: 400, source: VideoSource::Synthetic(Synth::Bands) }),
            voice: VoiceSource::Synthetic { tone_hz },
            codecs,
            spawn: std_spawn(),
            signal: Arc::new(|| {}),
            clock: Instant::now(),
        }
    }

    /// Two calls joined to each other. `slots` and `burst_ms` shape the link from a to b.
    fn pair(video: bool, codecs: Arc<dyn Codecs>, slots: usize, burst_ms: u64) -> (Call, Call) {
        let mut a = Call::start(config(1, video, 440.0, codecs.clone()));
        let mut b = Call::start(config(2, video, 660.0, codecs));
        let (a_to_b, b_to_a) = (link(b.inlet(), slots, burst_ms), link(a.inlet(), 256, 0));
        a.join(Join { peer: 2, their_pub: device(2), key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out: a_to_b, stream_cap: None });
        b.join(Join { peer: 1, their_pub: device(1), key_id: 0, send_secret: [2; 32], recv_secret: [1; 32], out: b_to_a, stream_cap: None });
        (a, b)
    }

    fn frame_of(t: &Tile) -> u64 {
        u64::from_be_bytes(t.picture.as_ref().unwrap().nv12[..8].try_into().unwrap())
    }

    /// Polls both calls every 10 ms for at most `limit`, until `done`; plays b's voice into a block
    /// each time. Returns the frame numbers b showed of a, and b's loud blocks.
    fn run(a: &mut Call, b: &mut Call, out: &mut voice::VoiceOut, limit: Duration, done: impl Fn(usize, usize) -> bool) -> (Vec<u64>, usize) {
        let (mut frames, mut loud) = (Vec::new(), 0);
        let mut block = [0f32; 480];
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline && !done(frames.len(), loud) {
            std::thread::sleep(Duration::from_millis(10));
            a.poll();
            b.poll();
            if let Some(t) = b.tiles().iter().find(|t| t.who == Some(1) && t.fresh) {
                frames.push(frame_of(t));
            }
            block.fill(0.0);
            out.mix_into_mono(48_000.0, &mut block);
            loud += usize::from((block.iter().map(|s| s * s).sum::<f32>() / 480.0).sqrt() > 0.1);
        }
        (frames, loud)
    }

    fn stop_both(mut a: Call, mut b: Call) {
        a.stop();
        b.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && !(a.stopped() && b.stopped()) {
            std::thread::sleep(Duration::from_millis(10));
            a.poll();
            b.poll();
        }
        assert!(a.stopped() && b.stopped(), "every worker must exit within 2 s of stop: {} of {} and {} of {}", a.done, a.spawned, b.done, b.spawned);
    }

    /// Two calls over an in-process link with the test codec: pictures arrive in order and voice
    /// plays; with the camera off no more pictures come; stop ends every worker.
    #[test]
    fn two_calls_exchange_pictures_and_voice_then_stop() {
        let (mut a, mut b) = pair(true, Arc::new(TestCodecs), 256, 0);
        let mut out = b.voice_out().unwrap();
        let (frames, loud) = run(&mut a, &mut b, &mut out, Duration::from_secs(4), |f, l| f >= 20 && l >= 10);
        assert!(frames.len() >= 20, "b should show 20 of a's pictures within 4 s, showed {}", frames.len());
        assert!(frames.windows(2).all(|w| w[0] < w[1]), "a's pictures must arrive in order: {frames:?}");
        assert!(loud >= 10, "a's tone should play loud in b's output for 100 ms, did for {loud} blocks");
        assert!(a.stats().pictures_sent >= 20 && a.stats().keys_sent >= 1, "a's stats count its pictures: {:?}", a.stats());
        assert_eq!(b.tiles()[1].state, TileState::Live, "a's tile at b is live");

        a.set_camera(false);
        run(&mut a, &mut b, &mut out, Duration::from_millis(300), |_, _| false);
        let (after, _) = run(&mut a, &mut b, &mut out, Duration::from_millis(400), |_, _| false);
        assert!(after.len() <= 1, "with a's camera off b should get no more pictures, got {after:?}");
        stop_both(a, b);
    }

    /// A link that slows from no limit to 150 kbit/s, well under the about 220 kbit/s the call
    /// sends (voice and video at its ask): the backlog builds, its age makes the guard skip and the
    /// delay rule cut, so the encoder backs off and the backlog drains again, while b's receiver
    /// skips no picture of a's at all (nothing was dropped, the start included).
    #[test]
    fn a_slowed_link_backs_the_encoder_off_and_its_backlog_drains() {
        let rate = Arc::new(AtomicU64::new(0));
        let mut a = Call::start(config(1, true, 440.0, Arc::new(TestCodecs)));
        let mut b = Call::start(config(2, true, 660.0, Arc::new(TestCodecs)));
        let (a_to_b, b_to_a) = (throttled_link(b.inlet(), 8, rate.clone()), link(a.inlet(), 256, 0));
        a.join(Join { peer: 2, their_pub: device(2), key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out: a_to_b, stream_cap: None });
        b.join(Join { peer: 1, their_pub: device(1), key_id: 0, send_secret: [2; 32], recv_secret: [1; 32], out: b_to_a, stream_cap: None });
        let mut out = b.voice_out().unwrap();
        let (fast, _) = run(&mut a, &mut b, &mut out, Duration::from_secs(1), |_, _| false);
        assert!(!fast.is_empty(), "pictures flow before the link slows");
        let asked_fast = a.stats().asked_kbps;
        rate.store(150_000, Ordering::Relaxed);
        let wait = |a: &Call| a.stats().peers.first().map_or((0, 0, 0), |p| (p.backlog_us, p.backlog, p.link_full));
        let full_before = wait(&a).2;
        let (mut peak, mut drained, mut series) = (0, false, Vec::new());
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !drained {
            run(&mut a, &mut b, &mut out, Duration::from_millis(250), |_, _| false);
            let (us, n, _) = wait(&a);
            series.push((us / 1000, n, a.stats().asked_kbps));
            peak = peak.max(us);
            drained = peak >= 50_000 && us < 20_000;
        }
        eprintln!("slowed link, every 250 ms (longest wait ms, datagrams, asked kbit/s): {series:?}");
        let asked_slow = a.stats().asked_kbps;
        assert!(wait(&a).2 > full_before, "the slowed link must fill: it was full {} times before and after", full_before);
        assert!(peak >= 50_000, "a link slowed to 150 kbit/s should build a backlog of 50 ms or more, the longest wait was {} ms", peak / 1000);
        assert!(drained, "the backlog should drain under 20 ms within 5 s once the encoder backs off: {series:?}");
        assert!(asked_slow < asked_fast, "the encoder should back off from {asked_fast} kbit/s, it asks {asked_slow}");
        let video = b.stats().peers.first().and_then(|p| p.video).unwrap_or_default();
        assert_eq!(video.skipped, 0, "nothing was dropped, so b's receiver skips no picture of a's: {video:?}");
        stop_both(a, b);
    }

    /// Two calls joined in either order, the second 200 ms after the first, skip no picture of each
    /// other's from the first one and ask for no key frame: what came before its sender was joined
    /// waits for the join (it races the answer), and someone who joins gets a key frame first.
    #[test]
    fn a_call_joined_in_either_order_skips_no_picture_from_the_first() {
        for a_first in [true, false] {
            let order = if a_first { "a joined first" } else { "b joined first" };
            let mut a = Call::start(config(1, true, 440.0, Arc::new(TestCodecs)));
            let mut b = Call::start(config(2, true, 660.0, Arc::new(TestCodecs)));
            let (a_to_b, b_to_a) = (link(b.inlet(), 256, 0), link(a.inlet(), 256, 0));
            let mut to_a = Some(Join { peer: 2, their_pub: device(2), key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out: a_to_b, stream_cap: None });
            let mut to_b = Some(Join { peer: 1, their_pub: device(1), key_id: 0, send_secret: [2; 32], recv_secret: [1; 32], out: b_to_a, stream_cap: None });
            let mut out = b.voice_out().unwrap();
            if a_first {
                a.join(to_a.take().unwrap());
            } else {
                b.join(to_b.take().unwrap());
            }
            run(&mut a, &mut b, &mut out, Duration::from_millis(200), |_, _| false);
            if let Some(join) = to_a.take() {
                a.join(join);
            }
            if let Some(join) = to_b.take() {
                b.join(join);
            }
            run(&mut a, &mut b, &mut out, Duration::from_millis(1500), |_, _| false);
            let got = |c: &Call| c.stats().peers.first().and_then(|p| p.video).unwrap_or_default();
            let (at_a, at_b) = (got(&a), got(&b));
            assert!(at_a.frames >= 10 && at_b.frames >= 10, "{order}: both should decode the other's pictures within 1.5 s: a {at_a:?}, b {at_b:?}");
            assert_eq!((at_a.skipped, at_a.key_requests), (0, 0), "{order}: a skips none of b's pictures and asks for no key frame: {at_a:?}");
            assert_eq!((at_b.skipped, at_b.key_requests), (0, 0), "{order}: b skips none of a's pictures and asks for no key frame: {at_b:?}");
            assert_eq!((a.stats().strays, b.stats().strays), (0, 0), "{order}: every early datagram was claimed by the join");
            stop_both(a, b);
        }
    }

    struct Counting {
        encoders: AtomicUsize,
        decoders: AtomicUsize,
    }

    impl Codecs for Counting {
        fn encoder(&self, rung: Rung, kbps: u32) -> Result<Box<dyn Encoder>, String> {
            self.encoders.fetch_add(1, Ordering::Relaxed);
            TestCodecs.encoder(rung, kbps)
        }
        fn decoder(&self) -> Result<Box<dyn Decoder>, String> {
            self.decoders.fetch_add(1, Ordering::Relaxed);
            TestCodecs.decoder()
        }
    }

    /// A voice-only call opens no codec at all (and the driver has no other way to a device), yet
    /// its voice plays.
    #[test]
    fn a_voice_only_call_opens_no_codec() {
        let codecs = Arc::new(Counting { encoders: AtomicUsize::new(0), decoders: AtomicUsize::new(0) });
        let (mut a, mut b) = pair(false, codecs.clone(), 256, 0);
        let mut out = b.voice_out().unwrap();
        let (frames, loud) = run(&mut a, &mut b, &mut out, Duration::from_secs(3), |_, l| l >= 10);
        assert!(loud >= 10, "a's tone should play in b's output, loud for {loud} blocks");
        assert!(frames.is_empty(), "a voice-only call shows no pictures");
        let (e, d) = (codecs.encoders.load(Ordering::Relaxed), codecs.decoders.load(Ordering::Relaxed));
        assert_eq!((e, d), (0, 0), "a voice-only call must open no encoder or decoder, opened {e} and {d}");
        stop_both(a, b);
    }

    /// A link with room for 4 datagrams, read every 40 ms, is full again and again: the call keeps
    /// what didn't fit and sends it later, so no picture is lost.
    #[test]
    fn a_full_link_delays_media_and_loses_none() {
        let (mut a, mut b) = pair(true, Arc::new(TestCodecs), 4, 40);
        let mut out = b.voice_out().unwrap();
        let (frames, _) = run(&mut a, &mut b, &mut out, Duration::from_secs(5), |f, _| f >= 15);
        assert!(frames.len() >= 15, "b should show 15 of a's pictures within 5 s over the slow link, showed {}", frames.len());
        assert!(frames.windows(2).all(|w| w[0] < w[1]), "pictures arrive in order: {frames:?}");
        let full = a.stats().peers.first().map_or(0, |p| p.link_full);
        assert!(full > 0, "the slow link must have been full at least once, stats {:?}", a.stats().peers);
        let lost = a.stats().peers.first().map_or(0, |p| p.session.resent);
        assert_eq!(lost, 0, "nothing is lost on a full link, so nothing is resent");
        stop_both(a, b);
    }
}
