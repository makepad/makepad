//! A running call: the workers behind the sans-IO core, and the handle the UI thread holds.
//!
//! Workers run through the caller's [`Spawn`]. The network worker starts a decode worker on each
//! participant's first frame. Town passes its thread spawner; tests and the probe pass [`std_spawn`]. Every
//! method of [`Call`] returns at once: the UI thread sends commands on channels and reads what the
//! workers published with `try_recv` and atomics. Voice runs in the app's audio callbacks through
//! [`voice::VoiceIn`] and [`voice::VoiceOut`], which never allocate or wait.
//!
//! Media leaves through each participant's link (`Join::out`) and arrives through one inlet
//! ([`Call::inlet`]) that every link feeds. A synthetic call opens no device: the codecs and
//! sources are the only ways to media, and this module calls no platform capture.

pub mod audio;
pub mod codec;
mod decode;
mod encode;
mod net;
#[cfg(test)]
mod test_clock;
pub mod synth;
pub mod voice;

use codec::{Codecs, Picture};
pub use crate::guard::{Content, VideoCaps};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use synth::Synth;
use voice::{AudioLevels, VoiceMarks};
pub use voice::{SourceLevel, TapFrame, TranscriptTap};
use crate::wire::stream;

/// The caller's name for a participant (Town: an index of the linked device).
pub type PeerId = u64;

/// Starts one long-lived worker thread: a name and the loop it runs.
pub type Spawn = Arc<dyn Fn(&'static str, Box<dyn FnOnce() + Send>) + Send + Sync>;

/// [`Spawn`] on plain threads, for tests and the probe.
pub fn std_spawn() -> Spawn {
    Arc::new(|name, f| {
        std::thread::Builder::new().name(name.to_string()).spawn(f).expect("a call worker thread");
    })
}

/// [`std_spawn`] that also counts the workers that panicked, for tests: a panicked worker never
/// sends `WorkerDone`, so a wait with no deadline needs this to see it.
#[cfg(test)]
pub(crate) fn watched_spawn(panicked: Arc<AtomicUsize>) -> Spawn {
    let test = std::thread::current();
    Arc::new(move |name, f| {
        let panicked = panicked.clone();
        let test = test.clone();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
                    panicked.fetch_add(1, Ordering::Relaxed);
                    test.unpark();
                }
            })
            .expect("a call worker thread");
    })
}

/// How many datagrams the inlet holds before a link drops and counts them.
pub const INLET: usize = 2048;
/// Camera frames waiting for the encoder: the newest matters, an older one is dropped.
const CAMERA_QUEUE: usize = 2;

/// The datagram inlet of a link. Successive calls can receive from the same link sender.
#[derive(Clone)]
pub struct Inlet {
    tx: SyncSender<(SocketAddr, Vec<u8>)>,
    rx: Arc<Mutex<Receiver<(SocketAddr, Vec<u8>)>>>,
}

impl Inlet {
    pub fn new() -> Self {
        let (tx, rx) = sync_channel(INLET);
        Self { tx, rx: Arc::new(Mutex::new(rx)) }
    }

    pub fn sender(&self) -> SyncSender<(SocketAddr, Vec<u8>)> {
        self.tx.clone()
    }
}

impl Default for Inlet {
    fn default() -> Self { Self::new() }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EchoCancelMode {
    #[default]
    Off,
    OsHardware,
    Software,
}

pub struct CallConfig {
    /// The link's existing inlet, or a fresh inlet for this call.
    pub inlet: Option<Inlet>,
    pub call_id: [u8; 16],
    /// This device's key, as the link proved it.
    pub my_pub: Vec<u8>,
    /// The initial outgoing video source, if any. Voice and a share can join independently.
    pub video: Option<VideoConfig>,
    pub voice: VoiceSource,
    pub echo_cancel: EchoCancelMode,
    pub codecs: Arc<dyn Codecs>,
    pub spawn: Spawn,
    /// Wakes the UI when there is something to poll (Town: its UI signal).
    pub signal: Arc<dyn Fn() + Send + Sync>,
    /// The media clock's zero: capture times are µs since it.
    pub clock: Instant,
}

pub struct VideoConfig {
    pub caps: VideoCaps,
    pub source: VideoSource,
}

/// A source of externally encoded H.264 access units. All methods run on the network worker
/// and return immediately; the source rings the supplied doorbell when a unit becomes ready.
pub trait EncodedSource: Send {
    fn pop(&mut self) -> Option<codec::Packet>;
    fn want(&mut self, w: u32, h: u32, fps: u32);
    fn set_kbps(&mut self, kbps: u32);
    fn key_now(&mut self);
    /// Cumulative capture and encoder output counts, when the source can report them.
    fn frame_counts(&self) -> Option<(u64, u64)> { None }
    fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>);
}

pub enum VideoSource {
    /// The app's camera callback pushes frames through [`Call::video_in`].
    Feed,
    Synthetic(Synth),
    /// A held window, encoded by its owner on demand.
    Encoded(Box<dyn EncodedSource>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VoiceSource {
    /// No microphone or playback until voice joins the session.
    Off,
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

/// The panel's maximum picture size and refresh; zero closes the subscription.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct View { pub width: u32, pub height: u32, pub fps: u32 }

impl View {
    fn open(self) -> bool { self.width != 0 && self.height != 0 && self.fps != 0 }
}

/// Latest decoded window from a watched participant, separate from their camera and audio.
pub struct SharePicture {
    view: View,
    pub picture: Option<Arc<Picture>>,
    pub capture_us: u64,
    pub fresh: bool,
    remote: Option<decode::RemotePictures>,
    returns: Vec<Arc<Picture>>,
}

impl Drop for SharePicture {
    fn drop(&mut self) {
        if let Some(remote) = &self.remote {
            if let Some(p) = self.picture.take() { remote.give_back(p, &mut self.returns); }
            remote.detach(&mut self.returns);
        }
    }
}

enum NetCommand {
    Join {
        join: Join, call_id: Option<[u8; 16]>, voice: bool, watching: bool,
        marks: Arc<VoiceMarks>, levels: Arc<AudioLevels>,
    },
    PeerVoice { peer: PeerId, on: bool },
    AudioSource(u8, voice::VoiceFrames),
    Rekey { peer: PeerId, key_id: u8, send: [u8; 32], recv: [u8; 32] },
    Leave(PeerId),
    Voice(Option<voice::MixControl>),
    Share(Option<(VideoCaps, Box<dyn EncodedSource>)>),
    BeginWatch(PeerId),
    Watch(PeerId, View),
    PeerView(PeerId, View),
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
    remote: Option<decode::RemotePictures>,
    returns: Vec<Arc<Picture>>,
    marks: Arc<VoiceMarks>,
    levels: Arc<AudioLevels>,
}

impl Tile {
    fn clear_remote_picture(&mut self) {
        if let (Some(remote), Some(picture)) = (&self.remote, self.picture.take()) {
            remote.give_back(picture, &mut self.returns);
        }
    }
}

impl Drop for Tile {
    fn drop(&mut self) {
        self.clear_remote_picture();
        if let Some(remote) = &self.remote {
            remote.detach(&mut self.returns);
        }
    }
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
    pub share_received: crate::video::ReceiverStats,
    pub share_decoded: u64,
    pub share_decode_errors: u64,
    pub paced_bytes: usize,
    pub share_source_counts: Option<(u64, u64)>,
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
    /// Present while offering a share. Size and cadence follow the panel; only bitrate adapts
    /// to the path in this version. Zero means there are no viewers.
    pub fixed_cell_share_kbps: Option<u32>,
    pub voice_dropped: u64,
    /// Non-audio media no joined peer could open, kept a second for a join that never came.
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
    Stats { peers: Vec<PeerStats>, voice_dropped: u64, strays: u64, share_kbps: Option<u32> },
    Encoder(String),
    Decoder(String),
    Event(CallEvent),
    DecoderStarted { peer: PeerId, stream: u8, pictures: decode::RemotePictures },
    View(PeerId, View),
    WorkerDone,
}

struct Shared {
    clock: Instant,
    stop: AtomicBool,
    workers: AtomicUsize,
    /// Published by the network worker for the encoder: the lowest estimate over the peers, the
    /// longest pacer delay, the highest video loss level (f64 bits, NaN while unknown), a key frame asked for.
    active_audio: AtomicUsize,
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
    fn new(clock: Instant, camera_on: bool) -> Self {
        Self {
            clock,
            stop: AtomicBool::new(false),
            workers: AtomicUsize::new(0),
            active_audio: AtomicUsize::new(0),
            estimate_bps: AtomicU64::new(crate::session::START_BPS),
            pacer_delay_us: AtomicU64::new(0),
            loss_level_bits: AtomicU64::new(f64::NAN.to_bits()),
            key_wanted: AtomicBool::new(true),
            camera_on: AtomicBool::new(camera_on),
            muted: Arc::new(AtomicBool::new(false)),
            rung: AtomicUsize::new(usize::MAX),
            asked_kbps: AtomicU64::new(0),
            pictures_sent: AtomicU64::new(0),
            keys_sent: AtomicU64::new(0),
        }
    }

    fn now_us(&self) -> u64 {
        elapsed_us(self.clock)
    }
}

fn elapsed_us(clock: Instant) -> u64 {
    #[cfg(test)]
    if let Some(now) = test_clock::now() { return now; }
    clock.elapsed().as_micros() as u64
}

/// An ordinary worker waits for its next capture. Tests can instead advance the whole call
/// to a specified media time and run just this worker's turn.
fn wait_for_tick(wait: impl FnOnce()) -> bool {
    #[cfg(test)]
    if let Some(running) = test_clock::tick() { return running; }
    wait();
    true
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
    echo_cancel: EchoCancelMode,
    render_ref: Option<Arc<crate::aec::RenderRefRing>>,
    render_ref_overflows: u64,
    render_ref_overflow_logged_us: Option<u64>,
    shared: Arc<Shared>,
    inlet: SyncSender<(SocketAddr, Vec<u8>)>,
    commands: Sender<NetCommand>,
    ui: Receiver<UiMsg>,
    ui_tx: Sender<UiMsg>,
    spawn: Spawn,
    signal: Arc<dyn Fn() + Send + Sync>,
    voice_stop: Option<Arc<AtomicBool>>,
    sharing: bool,
    // An announced window keeps its session even while its panel is closed.
    peer_shares: std::collections::BTreeSet<PeerId>,
    has_camera: bool,
    shares: std::collections::BTreeMap<PeerId, SharePicture>,
    views: std::collections::VecDeque<(PeerId, View)>,
    // The reliable VIEW may precede Join on a newly opened media session.
    peer_views: std::collections::BTreeMap<PeerId, View>,
    recycle: Sender<Arc<Picture>>,
    send_streams: Vec<u8>,
    voice_in: Option<voice::VoiceIn>,
    transcript_tap: Option<TranscriptTap>,
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
    pub fn start(mut config: CallConfig) -> Call {
        let (camera, share) = match config.video.take() {
            Some(VideoConfig { caps, source: VideoSource::Encoded(source) }) => (None, Some((caps, source))),
            camera => (camera, None),
        };
        let has_camera = camera.is_some();
        let shared = Arc::new(Shared::new(config.clock, has_camera));
        let Inlet { tx: inlet, rx: inlet_rx } = config.inlet.take().unwrap_or_default();
        let (commands, commands_rx) = channel();
        let (ui_tx, ui) = channel();
        let (video_tx, video_rx) = channel();
        let (recycle, recycle_rx) = channel();
        let my_marks = Arc::new(VoiceMarks::default());
        let signal = config.signal.clone();
        let mut spawned = 0;

        let decoders = decode::Decoders::new(config.spawn.clone(), config.codecs.clone(), shared.clone(), ui_tx.clone(), signal.clone());
        let net = net::Net::new(&config, shared.clone(), inlet_rx, commands_rx, video_rx, decoders, inlet.clone(), ui_tx.clone(), signal.clone());
        spawn_worker(&config.spawn, "call network", shared.clone(), ui_tx.clone(), signal.clone(), move || net.run());
        spawned += 1;

        let mut video_in = None;
        if let Some(video) = camera {
            let (camera_tx, camera_rx) = sync_channel(CAMERA_QUEUE);
            let spare = Arc::new(std::sync::Mutex::new(Vec::new()));
            if matches!(video.source, VideoSource::Feed) {
                video_in = Some(VideoIn { frames: camera_tx, spare: spare.clone(), clock: config.clock });
            }
            let encode = encode::Encode::new(video, config.codecs.clone(), shared.clone(), camera_rx, spare, video_tx, inlet.clone(), recycle_rx, ui_tx.clone(), signal.clone());
            spawn_worker(&config.spawn, "call encode", shared.clone(), ui_tx.clone(), signal.clone(), move || encode.run());
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
            camera_off: !has_camera,
            state: TileState::Live,
            marks: my_marks,
            levels: Arc::default(),
            remote: None,
            returns: Vec::new(),
        };
        let mut call = Call {
            echo_cancel: config.echo_cancel, render_ref: None, render_ref_overflows: 0,
            render_ref_overflow_logged_us: None,
            spawn: config.spawn, ui_tx, signal, voice_stop: None, sharing: false, peer_shares: Default::default(), has_camera,
            shares: Default::default(), views: Default::default(), peer_views: Default::default(),
            shared,
            inlet,
            commands,
            ui,
            recycle,
            send_streams: Vec::new(),
            voice_in: None,
            transcript_tap: None,
            voice_out: None,
            video_in,
            tiles: vec![me],
            stats: CallStats::default(),
            spawned,
            done: 0,
            stopped: false,
        };
        call.add_voice(config.voice);
        if let Some((caps, source)) = share { call.add_share(caps, source); }
        call
    }

    /// Where every participant's link puts this call's datagrams (0xE2), with the sender's address.
    pub fn inlet(&self) -> SyncSender<(SocketAddr, Vec<u8>)> {
        self.inlet.clone()
    }

    /// The audio input callback's half, once (none for a synthetic voice).
    pub fn voice_in(&mut self) -> Option<voice::VoiceIn> {
        self.voice_in.take()
    }

    pub fn echo_cancel_mode(&self) -> EchoCancelMode { self.echo_cancel }

    /// Attach voice to the existing link, without changing its secrets, estimator or pacer.
    pub fn add_voice(&mut self, source: VoiceSource) {
        if self.stopped || self.voice_stop.is_some() || source == VoiceSource::Off { return; }
        let clock = self.shared.clock;
        let muted = self.shared.muted.clone();
        let marks = self.tiles[0].marks.clone();
        let ((input, mut frames), (output, mix)) = if self.echo_cancel == EchoCancelMode::Software {
            let ring = crate::aec::RenderRefRing::new();
            if muted.load(Ordering::Relaxed) {
                ring.set_capture_muted(true);
            } else {
                ring.set_capture_resuming();
            }
            self.render_ref = Some(ring.clone());
            self.render_ref_overflows = 0;
            self.render_ref_overflow_logged_us = None;
            (voice::voice_in_with_aec(clock, muted, marks, Some(ring.clone())), voice::voice_out_with_ref(Some(ring)))
        } else {
            (voice::voice_in(clock, muted, marks), voice::voice_out())
        };
        self.transcript_tap = frames.transcript_tap();
        self.voice_out = Some(output);
        self.send(NetCommand::Voice(Some(mix)));
        self.send(NetCommand::AudioSource(stream::VOICE, frames));
        self.send_streams.push(stream::VOICE);
        let stop = Arc::new(AtomicBool::new(false));
        self.voice_stop = Some(stop.clone());
        if let VoiceSource::Synthetic { tone_hz } = source {
            let mut input = input;
            let shared = self.shared.clone();
            spawn_worker(&self.spawn, "call synthetic voice", shared.clone(), self.ui_tx.clone(), self.signal.clone(), move || {
                let mut voice = synth::SynthVoice::new(tone_hz);
                let mut block = [0f32; synth::SYNTH_BLOCK];
                let start = Instant::now();
                for n in 0u32.. {
                    let wait = || {
                        if let Some(wait) = (start + std::time::Duration::from_millis(10) * n).checked_duration_since(Instant::now()) {
                            std::thread::sleep(wait);
                        }
                    };
                    if !wait_for_tick(wait) || shared.stop.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) { break; }
                    voice.next_block(&mut block);
                    input.push_mono(synth::SYNTH_RATE, &block);
                }
            });
            self.spawned += 1;
        } else { self.voice_in = Some(input); }
    }

    /// End microphone and playback while leaving a running share on its link.
    pub fn remove_voice(&mut self) {
        if let Some(stop) = self.voice_stop.take() {
            stop.store(true, Ordering::Relaxed);
            self.send(NetCommand::Voice(None));
            self.send_streams.retain(|&s| s != stream::VOICE);
            self.voice_in = None;
            self.voice_out = None;
            self.render_ref = None;
            self.render_ref_overflows = 0;
            self.render_ref_overflow_logged_us = None;
            self.transcript_tap = None;
            self.stop_if_idle();
        }
    }

    fn stop_if_idle(&mut self) {
        if self.send_streams.is_empty() && !self.has_camera && !self.sharing && self.peer_shares.is_empty() {
            self.stop();
        }
    }

    /// Offer a held window with no audience. Subscribe each viewer to this source explicitly.
    pub fn add_share(&mut self, caps: VideoCaps, source: Box<dyn EncodedSource>) {
        if !self.stopped {
            self.sharing = true;
            self.peer_views.clear();
            self.send(NetCommand::Share(Some((caps, source))));
        }
    }

    pub fn remove_share(&mut self) {
        if self.sharing {
            self.sharing = false;
            self.peer_views.clear();
            self.send(NetCommand::Share(None));
            self.stop_if_idle();
        }
    }

    /// Apply a remote share offer or its end from reliable signalling. A closed Watch panel
    /// can reopen while the offer lives. Ending the last medium stops the session.
    pub fn set_peer_sharing(&mut self, peer: PeerId, offered: bool) {
        if self.stopped { return; }
        if offered {
            self.peer_shares.insert(peer);
        } else if self.peer_shares.remove(&peer) {
            self.watch_share(peer, View::default());
            self.stop_if_idle();
        }
    }

    /// Request the first share key at Watch, even before layout measures the panel.
    pub fn begin_watch(&mut self, peer: PeerId) {
        if self.stopped || self.shares.contains_key(&peer) { return; }
        self.peer_shares.insert(peer);
        self.shares.insert(peer, SharePicture {
            view: View::default(), picture: None, capture_us: 0, fresh: false, remote: None, returns: Vec::with_capacity(4),
        });
        self.send(NetCommand::BeginWatch(peer));
    }

    /// Set the local viewer's panel size. Closing immediately releases its pictures.
    pub fn watch_share(&mut self, peer: PeerId, view: View) {
        if self.stopped { return; }
        if view.open() {
            self.begin_watch(peer);
            let slot = self.shares.get_mut(&peer).unwrap();
            if slot.view == view { return; }
            // An opening is reliable signalling immediately. Only subsequent resizes debounce.
            if !slot.view.open() { self.views.push_back((peer, view)); }
            slot.view = view;
        } else {
            if self.shares.remove(&peer).is_none() { return; }
            // A final close must reach the reliable signalling owner even if it ends this session.
            self.views.push_back((peer, View::default()));
            self.send(NetCommand::Watch(peer, View::default()));
            return;
        }
        self.send(NetCommand::Watch(peer, view));
    }

    /// A debounced VIEW for the controller to send on the link's reliable signalling stream.
    pub fn take_view(&mut self) -> Option<(PeerId, View)> { self.views.pop_front() }

    /// Apply the other participant's VIEW received on that same reliable link.
    pub fn set_peer_view(&mut self, peer: PeerId, view: View) {
        if self.stopped { return; }
        if view.open() { self.peer_views.insert(peer, view); } else { self.peer_views.remove(&peer); }
        self.send(NetCommand::PeerView(peer, view));
    }

    pub fn share_picture(&self, peer: PeerId) -> Option<&SharePicture> { self.shares.get(&peer) }

    /// Takes this call's microphone transcription tap, once, initially off.
    pub fn transcript_tap(&mut self) -> Option<TranscriptTap> {
        self.transcript_tap.take()
    }

    /// Adds an input using today's 16 kHz mono ADPCM framing. App sharing can add its own format later.
    pub fn add_audio_source(&mut self, stream: u8) -> Option<voice::VoiceIn> {
        if self.stopped || stream != stream::APP_AUDIO || self.send_streams.contains(&stream) {
            return None;
        }
        let (input, frames) = voice::voice_in(self.shared.clock, Arc::default(), Arc::default());
        self.send(NetCommand::AudioSource(stream, frames));
        self.send_streams.push(stream);
        Some(input)
    }

    /// Playback gain and mute for a participant's stream, once its first frame arrived.
    pub fn source_level(&self, peer: PeerId, stream: u8) -> Option<Arc<SourceLevel>> {
        self.tiles.iter().find(|tile| tile.who == Some(peer))?.levels.slot(stream)?.get().cloned()
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
        self.join_link(join, None, true);
    }

    /// Joins a peer under its pairwise call id with microphone audio enabled in both directions
    /// when `voice` is true.
    pub fn join_peer(&mut self, join: Join, call_id: [u8; 16], voice: bool) {
        self.join_link(join, Some(call_id), voice);
    }

    /// Changes voice participation while the peer's other media keep flowing.
    pub fn set_peer_voice(&mut self, peer: PeerId, on: bool) {
        self.send(NetCommand::PeerVoice { peer, on });
    }

    fn join_link(&mut self, join: Join, call_id: Option<[u8; 16]>, voice: bool) {
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
                levels: Arc::default(),
                remote: None,
                returns: Vec::with_capacity(4),
            });
        }
        let tile = self.tiles.iter().find(|t| t.who == Some(join.peer)).unwrap();
        let id = join.peer;
        self.send(NetCommand::Join {
            join, call_id, voice, watching: self.shares.contains_key(&id),
            marks: tile.marks.clone(), levels: tile.levels.clone(),
        });
        if let Some(&view) = self.peer_views.get(&id) { self.send(NetCommand::PeerView(id, view)); }
        if let Some(slot) = self.shares.get(&id) {
            let view = slot.view;
            if view.open() { self.send(NetCommand::Watch(id, view)); }
        }
    }

    /// New secrets for a participant (a new epoch after its link came back).
    pub fn rekey(&mut self, peer: PeerId, key_id: u8, send: [u8; 32], recv: [u8; 32]) {
        self.send(NetCommand::Rekey { peer, key_id, send, recv });
    }

    pub fn leave(&mut self, peer: PeerId) {
        let offered = self.peer_shares.remove(&peer);
        self.shares.remove(&peer);
        self.views.retain(|(id, _)| *id != peer);
        self.peer_views.remove(&peer);
        self.tiles.retain(|t| t.who != Some(peer));
        self.send(NetCommand::Leave(peer));
        if offered { self.stop_if_idle(); }
    }

    /// Silent voice frames from now on (the framer keeps the stream's clock).
    pub fn set_muted(&mut self, muted: bool) {
        if let Some(ring) = &self.render_ref { ring.set_capture_muted(muted); }
        self.shared.muted.store(muted, Ordering::Release);
        self.tiles[0].muted = muted;
    }

    /// Request a fresh software echo path when a capture or playback device reopens or changes.
    pub fn reset_echo_cancel(&self) {
        if let Some(ring) = &self.render_ref { ring.request_reset(); }
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
                t.clear_remote_picture();
            }
        }
    }

    fn send(&self, command: NetCommand) {
        let _ = self.commands.send(command);
        ring(&self.inlet);
    }

    /// What happened since the last poll; keeps the newest picture per tile. `try_recv` only.
    pub fn poll(&mut self) -> Vec<CallEvent> {
        if let Some(ring) = &self.render_ref {
            let n = ring.overflows();
            if n > self.render_ref_overflows {
                const OVERFLOW_LOG_INTERVAL_US: u64 = 5_000_000;
                let now_us = self.shared.now_us();
                if self.render_ref_overflow_logged_us.is_none_or(|last| now_us.saturating_sub(last) >= OVERFLOW_LOG_INTERVAL_US) {
                    eprintln!("call: echo reference overflowed on {} more render pushes ({n} total)", n - self.render_ref_overflows);
                    self.render_ref_overflows = n;
                    self.render_ref_overflow_logged_us = Some(now_us);
                }
            }
        }
        let mut events = Vec::new();
        for t in &mut self.tiles {
            t.fresh = false;
            if let Some(remote) = &t.remote {
                remote.retry(&mut t.returns);
            }
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
                Ok(UiMsg::Stats { peers, voice_dropped, strays, share_kbps }) => {
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
                    self.stats.fixed_cell_share_kbps = share_kbps;
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
                Ok(UiMsg::View(peer, view)) => {
                    if self.shares.get(&peer).is_some_and(|slot| slot.view == view) { self.views.push_back((peer, view)); }
                }
                Ok(UiMsg::DecoderStarted { peer, stream: s, pictures }) => {
                    self.spawned += 1;
                    if s == stream::SHARE {
                        if let Some(tile) = self.shares.get_mut(&peer) {
                            if let Some(old) = tile.remote.replace(pictures) {
                                if let Some(pic) = tile.picture.take() { old.give_back(pic, &mut tile.returns); }
                                old.detach(&mut tile.returns);
                            }
                        }
                        continue;
                    }
                    if let Some(t) = self.tiles.iter_mut().find(|t| t.who == Some(peer)) {
                        if let Some(previous) = t.remote.replace(pictures) {
                            previous.detach(&mut t.returns);
                        }
                    }
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
            if let Some(remote) = &t.remote {
                if let Some((picture, capture_us)) = remote.take() {
                    if t.camera_off {
                        remote.give_back(picture, &mut t.returns);
                    } else {
                        if let Some(previous) = t.picture.replace(picture) {
                            remote.give_back(previous, &mut t.returns);
                        }
                        t.capture_us = capture_us;
                        t.fresh = true;
                        if t.state == TileState::Connecting {
                            t.state = TileState::Live;
                        }
                    }
                }
            }
            t.level = t.marks.level();
            t.talking = t.marks.talking() && !t.muted;
        }
        for tile in self.shares.values_mut() {
            tile.fresh = false;
            if let Some(remote) = &tile.remote {
                remote.retry(&mut tile.returns);
                if let Some((picture, capture_us)) = remote.take() {
                    if let Some(old) = tile.picture.replace(picture) { remote.give_back(old, &mut tile.returns); }
                    tile.capture_us = capture_us;
                    tile.fresh = true;
                }
            }
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

    /// A stop was requested, by `stop` or by the last medium going. Nothing can be added any more.
    pub fn stopping(&self) -> bool { self.stopped }

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
    use crate::guard::Rung;
    use std::time::Duration;

    fn device(n: u8) -> Vec<u8> {
        let mut k = vec![4u8; 65];
        k[1] = n;
        k
    }

    /// One direction of an in-process link, with room for `slots` datagrams.
    fn link(to: SyncSender<(SocketAddr, Vec<u8>)>, slots: usize, packet_ms: u64) -> SyncSender<Vec<u8>> {
        let (tx, rx) = sync_channel::<Vec<u8>>(slots);
        forward_link(to, rx, packet_ms);
        tx
    }

    fn forward_link(to: SyncSender<(SocketAddr, Vec<u8>)>, rx: Receiver<Vec<u8>>, packet_ms: u64) {
        std::thread::spawn(move || {
            let from = SocketAddr::from(([192, 0, 2, 1], 47100));
            let mut next_packet = Instant::now();
            while let Ok(d) = rx.recv() {
                if to.send((from, d)).is_err() {
                    return;
                }
                if packet_ms > 0 {
                    // Keep service earned during a late wake, so timer slop cannot accumulate
                    // and leave voice and feedback permanently ahead of video.
                    next_packet += Duration::from_millis(packet_ms);
                    if let Some(wait) = next_packet.checked_duration_since(Instant::now()) {
                        std::thread::sleep(wait);
                    }
                    continue;
                }
                while let Ok(d) = rx.try_recv() {
                    if to.send((from, d)).is_err() {
                        return;
                    }
                }
            }
        });
    }

    /// Service a bounded, lossless link on the call's clock. Only the datagram being
    /// serialized has left its channel; a full channel exerts backpressure on the real driver.
    fn clocked_link(to: SyncSender<(SocketAddr, Vec<u8>)>, slots: usize) -> (SyncSender<Vec<u8>>, impl FnMut(u64, u64)) {
        let (tx, rx) = sync_channel::<Vec<u8>>(slots);
        let from = SocketAddr::from(([192, 0, 2, 1], 47100));
        let mut in_flight = None;
        (tx, move |now, bps| {
            let mut at = now;
            loop {
                if let Some((due, d)) = in_flight.take() {
                    if due > now { in_flight = Some((due, d)); break; }
                    to.try_send((from, d)).expect("the receiving worker drains its inlet each tick");
                    at = due;
                }
                let Ok(d) = rx.try_recv() else { break };
                let transit = (d.len() as u64 * 8_000_000).checked_div(bps).unwrap_or(0);
                in_flight = Some((at + transit, d));
            }
        })
    }

    /// `panicked` counts the call's workers that panicked.
    fn config(n: u8, video: bool, tone_hz: f32, codecs: Arc<dyn Codecs>, panicked: &Arc<AtomicUsize>) -> CallConfig {
        CallConfig {
            inlet: None,
            call_id: [7; 16],
            my_pub: device(n),
            video: video.then_some(VideoConfig {
                caps: VideoCaps { width: 320, height: 180, fps: 30, content: Content::Camera },
                source: VideoSource::Synthetic(Synth::Bands),
            }),
            voice: VoiceSource::Synthetic { tone_hz },
            echo_cancel: EchoCancelMode::Off,
            codecs,
            spawn: watched_spawn(panicked.clone()),
            signal: Arc::new(|| {}),
            clock: Instant::now(),
        }
    }

    /// Two joined calls and their worker panic counter. A slow forward link starts draining
    /// only after A has offered more datagrams than it can hold.
    fn pair(video: bool, codecs: Arc<dyn Codecs>, slots: usize, packet_ms: u64) -> (Call, Call, Arc<AtomicUsize>) {
        let panicked = Arc::new(AtomicUsize::new(0));
        let mut a = Call::start(config(1, video, 440.0, codecs.clone(), &panicked));
        let mut b = Call::start(config(2, video, 660.0, codecs, &panicked));
        let (a_to_b, parked) = if packet_ms == 0 {
            (link(b.inlet(), slots, 0), None)
        } else {
            let (tx, rx) = sync_channel(slots);
            (tx, Some(rx))
        };
        let b_to_a = link(a.inlet(), 256, 0);
        a.join(Join { peer: 2, their_pub: device(2), key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out: a_to_b, stream_cap: None });
        b.join(Join { peer: 1, their_pub: device(1), key_id: 0, send_secret: [2; 32], recv_secret: [1; 32], out: b_to_a, stream_cap: None });
        if let Some(rx) = parked {
            // Net tries the link before publishing the stamped count. With no reader yet,
            // the first datagram beyond its capacity must have gone into net's backlog.
            while !a.stats().peers.first().is_some_and(|p| p.session.sent > slots as u64) {
                a.poll();
                assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked before A filled the forward link");
                std::thread::sleep(Duration::from_millis(2));
            }
            forward_link(b.inlet(), rx, packet_ms);
        }
        (a, b, panicked)
    }

    /// Both devices are fed by the test. Wait for both joins before sending their first sample.
    fn audio_pair() -> (Call, Call, Arc<AtomicUsize>) {
        let panicked = Arc::new(AtomicUsize::new(0));
        let start = |id| {
            let mut cfg = config(id, false, 0.0, Arc::new(TestCodecs), &panicked);
            cfg.voice = VoiceSource::Feed;
            Call::start(cfg)
        };
        let (mut a, mut b) = (start(1), start(2));
        a.join(Join { peer: 2, their_pub: device(2), key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out: link(b.inlet(), 256, 0), stream_cap: None });
        b.join(Join { peer: 1, their_pub: device(1), key_id: 0, send_secret: [2; 32], recv_secret: [1; 32], out: link(a.inlet(), 256, 0), stream_cap: None });
        while a.stats().peers.is_empty() || b.stats().peers.is_empty() {
            a.poll();
            b.poll();
            assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked before the audio peers joined");
            std::thread::sleep(Duration::from_millis(2));
        }
        (a, b, panicked)
    }

    fn tone(samples: &mut [f32], phase: &mut f64, hz: f64, rate: f64) {
        for sample in samples {
            *sample = (0.3 * phase.sin()) as f32;
            *phase = (*phase + std::f64::consts::TAU * hz / rate) % std::f64::consts::TAU;
        }
    }

    /// Goertzel power at one frequency, normalized by the square of the sample count.
    fn tone_power(samples: &[f32], hz: f64, rate: f64) -> f64 {
        let c = 2.0 * (std::f64::consts::TAU * hz / rate).cos();
        let (mut a, mut b) = (0.0, 0.0);
        for &x in samples {
            (a, b) = (f64::from(x) + c * a - b, a);
        }
        (a * a + b * b - c * a * b) / (samples.len() as f64).powi(2)
    }

    #[test]
    fn call_muted_overflow_is_quiet_and_active_overflow_is_counted() {
        let panicked = Arc::new(AtomicUsize::new(0));
        let mut cfg = config(1, false, 0.0, Arc::new(TestCodecs), &panicked);
        cfg.voice = VoiceSource::Feed;
        cfg.echo_cancel = EchoCancelMode::Software;
        let test = std::thread::current();
        cfg.signal = Arc::new(move || test.unpark());
        let mut call = Call::start(cfg);
        let ring = call.render_ref.clone().unwrap();
        let mut input = call.voice_in().unwrap();
        let mut output = call.voice_out().unwrap();
        let mut render = |blocks| {
            for _ in 0..blocks { output.mix_into_mono(48_000.0, &mut [0.0; 480]); }
        };
        render(60);
        call.poll();
        assert!(ring.paused_overflows() > 0, "speaker-only startup must exercise ring overflow");
        assert_eq!((ring.overflows(), call.render_ref_overflows), (0, 0), "startup must stay quiet until capture opens");
        input.push_mono(48_000.0, &[0.0; 480]);
        call.set_muted(true);
        render(60);
        call.set_muted(false);
        render(10);
        call.poll();
        assert_eq!((ring.overflows(), call.render_ref_overflows), (0, 0), "closed-mic mute and reopen must stay quiet");

        input.push_mono(48_000.0, &[0.0; 480]);
        render(60);
        call.poll();
        let active = ring.overflows();
        assert!(active > 0, "a stall after capture resumes must count as active overflow");
        assert_eq!(call.render_ref_overflows, active, "poll must immediately log the first active overflow total");
        call.reset_echo_cancel();
        assert_eq!(ring.reset_epoch(), 1, "the call must forward the route reset");
        render(10);
        assert_eq!(ring.overflows(), active, "a route reopen must suppress further active overflows");
        call.stop();
        loop {
            call.poll();
            assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked during shutdown");
            if call.stopped() { break; }
            std::thread::park();
        }
    }

    /// A feed-driven call whose only clocked worker is its network thread.
    fn clocked_audio_call() -> (Call, test_clock::Ticks, Arc<AtomicUsize>) {
        let panics = Arc::new(AtomicUsize::new(0));
        let mut cfg = config(1, false, 0.0, Arc::new(TestCodecs), &panics);
        cfg.voice = VoiceSource::Feed;
        let (spawn, workers) = test_clock::spawn_on_ticks(cfg.spawn);
        cfg.spawn = spawn;
        let test = std::thread::current();
        cfg.signal = Arc::new(move || test.unpark());
        let call = Call::start(cfg);
        let (name, ticks) = workers.try_recv().expect("network worker registered before start returned");
        assert_eq!(name, "call network", "the network tick must settle all queued commands and datagrams");
        (call, ticks, panics)
    }

    fn stop_clocked_call(mut call: Call, ticks: test_clock::Ticks, panics: &AtomicUsize) {
        call.stop();
        drop(ticks);
        loop {
            call.poll();
            assert_eq!(panics.load(Ordering::Relaxed), 0, "a clocked call worker panicked");
            if call.stopped() { break; }
            std::thread::park();
        }
    }

    /// The other end of a link has only the keys and call id negotiated for that link.
    fn peer_link(id: u8, public: Vec<u8>, cid: &[u8; 16], key_id: u8) -> (Join, crate::session::Peer, Receiver<Vec<u8>>) {
        let (out, packets) = sync_channel(64);
        let mut remote = crate::session::Peer::new(cid, &public, &device(1));
        let join = Join { peer: id.into(), their_pub: public, key_id, send_secret: [id; 32],
            recv_secret: [id + 10; 32], out, stream_cap: None };
        remote.install_send_secret(key_id, &join.recv_secret).unwrap();
        remote.install_recv_secret(key_id, &join.send_secret).unwrap();
        (join, remote, packets)
    }

    fn deliver(call: &Call, packet: Vec<u8>) {
        call.inlet.try_send((SocketAddr::from(([192, 0, 2, 1], 47100)), packet)).unwrap();
    }

    #[test]
    fn replacing_or_removing_share_clears_previous_audience_and_pending_peer_views() {
        use crate::session::{Event, ReceiveOutcome};

        #[derive(Default)]
        struct Demand { wants: Vec<View>, pops: usize }
        struct Source { demand: Arc<Mutex<Demand>>, picture: Option<Vec<u8>> }
        impl EncodedSource for Source {
            fn want(&mut self, width: u32, height: u32, fps: u32) {
                self.demand.lock().unwrap().wants.push(View { width, height, fps });
            }
            fn pop(&mut self) -> Option<codec::Packet> {
                self.demand.lock().unwrap().pops += 1;
                Some(codec::Packet { data: self.picture.take()?, key: true, pts_100ns: 0 })
            }
            fn set_kbps(&mut self, _: u32) {}
            fn key_now(&mut self) {}
            fn set_wake(&mut self, _: Arc<dyn Fn() + Send + Sync>) {}
        }

        let caps = VideoCaps { width: 16, height: 16, fps: 1, content: Content::Share };
        let view = View { width: caps.width, height: caps.height, fps: caps.fps };
        let attach = |a: &mut Call, picture: &[u8]| {
            let demand = Arc::new(Mutex::new(Demand::default()));
            a.add_share(caps, Box::new(Source { demand: demand.clone(), picture: Some(picture.to_vec()) }));
            demand
        };
        let join = |a: &mut Call, id| {
            let cid = [id; 16];
            let (link, remote, packets) = peer_link(id, device(id), &cid, 0);
            a.join_peer(link, cid, true);
            (remote, packets)
        };
        let check = |(remote, packets): &mut (crate::session::Peer, Receiver<Vec<u8>>), now, expected: Option<&[u8]>| {
            let mut pictures = Vec::new();
            for packet in packets.try_iter() {
                if crate::wire::Header::parse(&packet).unwrap().0.stream != stream::SHARE { continue; }
                assert!(expected.is_some(), "a peer outside the source audience received a share datagram at {now}");
                let mut events = Vec::new();
                assert_eq!(remote.try_receive(now, &packet, &mut events), ReceiveOutcome::Opened,
                    "the viewer must open the datagram with its pairwise key");
                for event in events {
                    if let Event::Video { stream: stream::SHARE, frame } = event { pictures.push(frame.data); }
                }
            }
            assert_eq!(pictures.iter().map(Vec::as_slice).collect::<Vec<_>>(), expected.into_iter().collect::<Vec<_>>(),
                "only the subscribed viewer receives the current source's picture at {now}");
        };

        let (mut a, ticks, panics) = clocked_audio_call();
        let mut b = join(&mut a, 2);
        let mut c = join(&mut a, 3);
        attach(&mut a, b"source-x");
        a.set_peer_view(2, view);
        a.set_peer_view(4, view);
        ticks.at(0);
        ticks.at(10_000);
        check(&mut b, 10_000, Some(b"source-x"));
        check(&mut c, 10_000, None);

        let y = attach(&mut a, b"source-y");
        let mut d = join(&mut a, 4);
        ticks.at(20_000);
        ticks.at(30_000);
        let parked = |demand: &Mutex<Demand>| {
            let demand = demand.lock().unwrap();
            assert_eq!(demand.pops, 0, "the replacement source stays unread until it has its own audience");
            assert_eq!(demand.wants, [View::default()], "only zero demand reaches an unopened source");
        };
        parked(&y);
        for peer in [&mut b, &mut c, &mut d] { check(peer, 30_000, None); }

        a.set_peer_view(3, view);
        ticks.at(40_000);
        ticks.at(50_000);
        check(&mut c, 50_000, Some(b"source-y"));
        for peer in [&mut b, &mut d] { check(peer, 50_000, None); }

        a.set_peer_view(5, view);
        a.remove_share();
        assert!(a.peer_views.is_empty(), "removal clears queued views before a late peer joins");
        ticks.at(60_000);
        let mut e = join(&mut a, 5);
        let z = attach(&mut a, b"source-z");
        ticks.at(70_000);
        ticks.at(80_000);
        parked(&z);
        for peer in [&mut b, &mut c, &mut d, &mut e] { check(peer, 80_000, None); }
        stop_clocked_call(a, ticks, &panics);
    }

    /// B and C keep speaking throughout. Only A knows which links have voice permission.
    fn exercise_voice_links(c_voice: &[bool]) {
        use crate::session::{Event, ReceiveOutcome};
        use crate::wire::Voice;

        struct OnePicture(Option<codec::Packet>);
        impl EncodedSource for OnePicture {
            fn pop(&mut self) -> Option<codec::Packet> { self.0.take() }
            fn want(&mut self, _: u32, _: u32, _: u32) {}
            fn set_kbps(&mut self, _: u32) {}
            fn key_now(&mut self) {}
            fn set_wake(&mut self, _: Arc<dyn Fn() + Send + Sync>) {}
        }

        let (mut a, ticks, panics) = clocked_audio_call();
        let mut mic = a.voice_in().unwrap();
        let mut speaker = a.voice_out().unwrap();
        let mut remotes = Vec::new();
        for (id, cid, hz, voice) in [(2, [1; 16], 500.0, true), (3, [2; 16], 1200.0, c_voice[0])] {
            let (join, remote, packets) = peer_link(id, device(id), &cid, 0);
            a.join_peer(join, cid, voice);
            // Both tones have a whole number of periods in this 20 ms frame.
            let mut samples = [0.0; voice::VOICE_FRAME];
            tone(&mut samples, &mut 0.0, hz, 16_000.0);
            let payload = makepad_teamtalk::codec::adpcm_encode(&mut Default::default(), 4, &samples);
            remotes.push((remote, packets, payload));
        }
        let caps = VideoCaps { width: 16, height: 16, fps: 1, content: Content::Share };
        let picture = codec::Packet { data: b"shared picture".to_vec(), key: true, pts_100ns: 0 };
        a.add_share(caps, Box::new(OnePicture(Some(picture))));
        a.set_peer_view(3, View { width: 16, height: 16, fps: 1 });
        let mut mic_frame = [0.0; 960];
        tone(&mut mic_frame, &mut 0.0, 800.0, 48_000.0);
        for (phase, &enabled) in c_voice.iter().enumerate() {
            if phase > 0 { a.set_peer_voice(3, enabled); }
            let mut heard = [0; 2];
            let mut pictures = [0; 2];
            let mut mix = Vec::new();
            for frame in 0..12 {
                let seq = (phase * 12 + frame) as u32;
                let now = u64::from(seq) * 20_000;
                for (remote, _, payload) in &mut remotes {
                    let voice = Voice { seq, flags: 0, capture_us: now, payload };
                    deliver(&a, remote.send_voice(now, stream::VOICE, &voice, true).unwrap());
                }
                mic.push_mono(48_000.0, &mic_frame);
                ticks.at(now);
                // The share pacer sends between consecutive microphone frames.
                ticks.at(now + 10_000);
                for (i, (remote, packets, _)) in remotes.iter_mut().enumerate() {
                    for packet in packets.try_iter() {
                        let (header, _) = crate::wire::Header::parse(&packet).expect("a media datagram on the downlink");
                        if header.stream == stream::VOICE { heard[i] += 1; }
                        let mut events = Vec::new();
                        assert_eq!(remote.try_receive(now, &packet, &mut events), ReceiveOutcome::Opened,
                            "peer {} could not open its pairwise downlink at frame {seq}", i + 2);
                        for event in events {
                            match event {
                                Event::Voice { stream: stream::VOICE, payload, .. } => {
                                    assert!(!payload.is_empty(), "peer {} must receive A's microphone tone", i + 2);
                                }
                                Event::Video { stream: stream::SHARE, frame } => {
                                    assert_eq!(frame.data, b"shared picture", "the viewer must open the original share");
                                    pictures[i] += 1;
                                }
                                _ => panic!("unexpected stream on peer {}'s downlink", i + 2),
                            }
                        }
                    }
                }
                assert_eq!(speaker.received_seq(2, stream::VOICE), Some(seq), "B's voice must advance on every tick");
                assert_eq!(speaker.received_seq(3, stream::VOICE), enabled.then_some(seq),
                    "C's ring must follow its current voice permission at frame {seq}");
                if frame >= 3 {
                    let mut block = [0.0; 960];
                    speaker.mix_into_mono(48_000.0, &mut block);
                    if frame >= 7 { mix.extend_from_slice(&block); }
                }
            }
            a.poll();
            let tile = a.tiles().iter().find(|tile| tile.who == Some(3)).expect("C must stay joined across voice toggles");
            if enabled {
                assert!(tile.talking && tile.level > 0.0,
                    "C's tile must show talking while its voice is on, phase {phase} (talking {}, level {})", tile.talking, tile.level);
            } else {
                assert!(!tile.talking && tile.level == 0.0,
                    "C's tile must not show talking after its voice is turned off, phase {phase} (talking {}, level {})", tile.talking, tile.level);
            }
            assert_eq!(heard, [12, if enabled { 12 } else { 0 }], "wrong number of microphone packets in phase {phase}");
            assert_eq!(pictures, [0, usize::from(phase == 0)], "C must receive the offered picture even with voice disabled");
            let b = tone_power(&mix, 500.0, 48_000.0);
            let c = tone_power(&mix, 1200.0, 48_000.0);
            assert!(b > 0.01, "B's tone must remain audible in phase {phase}, power {b}");
            if enabled {
                assert!(c > 0.01, "C's enabled tone must reach the mix in phase {phase}, power {c}");
            } else {
                assert!(c < b * 0.001, "C's disabled tone reached the mix in phase {phase}, powers {b}, {c}");
            }
        }
        stop_clocked_call(a, ticks, &panics);
    }

    #[test]
    fn per_peer_call_id_mixes_two_peers_with_distinct_keys() {
        exercise_voice_links(&[true]);
    }

    #[test]
    fn share_only_peer_receives_no_voice_and_cannot_inject_voice() {
        exercise_voice_links(&[false, true, false]);
    }

    #[test]
    fn sender_tag_collision_routes_to_matching_peer_and_preserves_early_frames() {
        use crate::session::{sender_tag, Peer};
        use crate::wire::{voice_flags, Voice};

        let cid = [5; 16];
        let mut tags = std::collections::BTreeMap::new();
        let (pub_b, pub_c) = (0..=u32::from(u16::MAX) + 1).find_map(|counter| {
            let mut public = device(2);
            public[2..6].copy_from_slice(&counter.to_le_bytes());
            tags.insert(sender_tag(&cid, &public), public.clone()).map(|previous| (previous, public))
        }).expect("more public keys than 16-bit tags must produce a collision");

        let rung = Rung { width: 8, height: 8, ..crate::guard::LADDER[0] };
        let au = TestCodecs.encoder(rung, 1).unwrap().push(&[128; 96], 0).unwrap().remove(0).data;
        let share = |a: &Call, remote: &mut Peer, at| {
            let mut packets = Vec::new();
            remote.send_video(at, stream::SHARE, &au, true, at, &mut packets).unwrap();
            assert_eq!(packets.len(), 1, "each synthetic key frame must fit one datagram");
            deliver(a, packets.pop().unwrap());
        };
        let voice = |remote: &mut Peer, seq, at| {
            let frame = Voice { flags: voice_flags::SILENT, seq, capture_us: at, payload: &[] };
            remote.send_voice(at, stream::VOICE, &frame, false).unwrap()
        };

        for c_key_id in [0, 1] {
            let (mut a, ticks, panics) = clocked_audio_call();
            let mut speaker = a.voice_out().unwrap();
            let (join_b, mut b, _to_b) = peer_link(2, pub_b.clone(), &cid, 0);
            let (join_c, mut c, _to_c) = peer_link(3, pub_c.clone(), &cid, c_key_id);
            a.begin_watch(2);
            a.begin_watch(3);
            // Both first shares arrive before either join. B must leave C's sequence zero in early.
            share(&a, &mut b, 1_000);
            share(&a, &mut c, 2_000);
            ticks.at(10_000);
            a.join_peer(join_b, cid, true);
            ticks.at(20_000);

            // Both shares now have wire sequence one. C's second picture still precedes its join.
            share(&a, &mut b, 30_000);
            share(&a, &mut c, 31_000);
            let duplicate = voice(&mut b, 10, 32_000);
            deliver(&a, duplicate.clone());
            ticks.at(40_000);
            assert_eq!(speaker.received_seq(2, stream::VOICE), Some(10), "B's voice must open before C joins");
            a.join_peer(join_c, cid, true);
            ticks.at(50_000);

            // B has already opened both of the VOICE wire sequences that C is about to send.
            deliver(&a, voice(&mut b, 11, 60_000));
            deliver(&a, voice(&mut c, 20, 61_000));
            deliver(&a, voice(&mut c, 21, 62_000));
            deliver(&a, duplicate);
            // Advance past the early queue's lifetime so any unclaimed copy shows up in strays.
            ticks.at(1_250_000);
            for event in a.poll() {
                assert!(!matches!(event, CallEvent::Problem { .. }), "receiving the colliding peers reported {event:?}");
            }
            assert_eq!(speaker.received_seq(2, stream::VOICE), Some(11), "B's voice advanced on its own ring");
            assert_eq!(speaker.received_seq(3, stream::VOICE), Some(21), "C's voice advanced on its own ring");
            assert_eq!(a.stats().strays, 0, "all early shares must be claimed for C key id {c_key_id}");
            assert_eq!(a.stats().peers.len(), 2, "both colliding links must be reported");
            for p in &a.stats().peers {
                assert_eq!(p.share_received.frames, 2, "peer {} lost an early share for C key id {c_key_id}", p.peer);
                assert_eq!(p.session.received, 4, "peer {} must open two shares and two voice frames", p.peer);
                assert_eq!((p.session.bad_tag, p.session.refused), (0, 0), "trying peer {} must not charge another peer's packet", p.peer);
                assert_eq!(p.session.replayed, u64::from(p.peer == 2), "only B owns the replay, peer {}", p.peer);
            }
            let viewer = a.share_picture(3).expect("C's watch remains open");
            assert!(viewer.remote.is_some(), "claiming C's early frames must start its share decoder");
            while a.share_picture(3).unwrap().picture.is_none() {
                assert_eq!(panics.load(Ordering::Relaxed), 0, "C's decoder panicked before publishing an early picture");
                std::thread::park();
                for event in a.poll() {
                    assert!(!matches!(event, CallEvent::Problem { .. }), "decoding C's early shares reported {event:?}");
                }
            }
            assert!(matches!(a.share_picture(3).unwrap().capture_us, 2_000 | 31_000), "the displayed share must come from C's early frames");

            if c_key_id == 1 {
                // B lacks C's key id. Charge a damaged C packet to C and an unknown audio key once to B.
                let mut damaged = voice(&mut c, 22, 1_250_001);
                *damaged.last_mut().unwrap() ^= 1;
                deliver(&a, damaged);
                c.install_send_secret(2, &[42; 32]).unwrap();
                deliver(&a, voice(&mut c, 23, 1_250_002));
                ticks.at(1_500_000);
                a.poll();
                for p in &a.stats().peers {
                    assert_eq!(p.session.bad_tag, u64::from(p.peer == 3), "a matching key id takes precedence over a refusal");
                    assert_eq!(p.session.refused, u64::from(p.peer == 2), "an unknown audio key counts only on the first matching link");
                }
            }
            stop_clocked_call(a, ticks, &panics);
        }
    }

    #[test]
    fn microphone_and_app_audio_mix_with_independent_levels() {
        let (mut a, mut b, panicked) = audio_pair();
        let mut mic = a.voice_in().unwrap();
        let mut app = a.add_audio_source(stream::APP_AUDIO).unwrap();
        assert!(a.add_audio_source(stream::APP_AUDIO).is_none() && a.add_audio_source(stream::VOICE).is_none(),
            "a send stream must have just one input");
        assert!(a.add_audio_source(stream::SHARE).is_none(), "a video stream must refuse an audio input");
        let mut out = b.voice_out().unwrap();
        let mut phases = [0.0; 2];
        let mut powers = Vec::new();
        let mut samples = Vec::new();
        for seq in 0..20 {
            let mut frame = [0.0; 960];
            tone(&mut frame, &mut phases[0], 440.0, 48_000.0);
            mic.push_mono(48_000.0, &frame);
            tone(&mut frame, &mut phases[1], 1000.0, 48_000.0);
            app.push_mono(48_000.0, &frame);
            while [stream::VOICE, stream::APP_AUDIO].iter().any(|&s| out.received_seq(1, s) != Some(seq)) {
                assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked before both streams delivered frame {seq}");
                std::thread::yield_now();
            }
            if seq < 3 { continue; } // Prefill the jitter rings before the first output block.
            let level = b.source_level(1, stream::APP_AUDIO).unwrap();
            if seq == 10 { level.set_muted(true); }
            if seq == 15 {
                level.set_gain(0.25);
                level.set_muted(false);
            }
            frame.fill(0.0);
            out.mix_into_mono(48_000.0, &mut frame);
            if seq >= 5 {
                samples.extend_from_slice(&frame);
                if seq % 5 == 4 {
                    powers.push((tone_power(&samples, 440.0, 48_000.0), tone_power(&samples, 1000.0, 48_000.0)));
                    samples.clear();
                }
            }
        }
        let mut guard = crate::guard::Guard::new(0);
        for (active, expected) in [(0, 198_000), (1, 198_000), (2, 126_000)] {
            guard.tick_with_audio(active, 0, 300_000, 0, None);
            assert_eq!(guard.ask_bps(), expected, "{active} active audio sources should leave {expected} bit/s for video");
        }
        assert_eq!(a.shared.active_audio.load(Ordering::Relaxed), 2, "both transmitting streams must count toward the video reserve");
        eprintln!("Goertzel power (440 Hz voice, 1000 Hz app), full/muted/quarter: {powers:.7?}");
        let (voice, app) = powers[0];
        assert!(voice > 0.01 && app > 0.01, "both tones must be present at unity gain: {powers:?}");
        for &(mic, _) in &powers[1..] {
            assert!((mic / voice - 1.0).abs() < 0.10, "the microphone power changed with the app level: {powers:?}");
        }
        assert!(powers[1].1 < app * 0.001, "muting app audio must remove its tone: {powers:?}");
        assert!((powers[2].1 / app * 16.0 - 1.0).abs() < 0.10, "quarter gain must give one sixteenth of app power: {powers:?}");
        stop_both(a, b, &panicked);
    }

    #[test]
    fn installed_audio_ports_follow_successive_calls() {
        let (mut mic, mut speaker, mut control) = audio::audio_ports();
        for (hz, other) in [(440.0, 1000.0), (1000.0, 440.0)] {
            let (mut a, mut b, panicked) = audio_pair();
            control.attach(a.voice_in(), a.voice_out().unwrap());
            let mut b_in = b.voice_in().unwrap();
            let mut b_out = b.voice_out().unwrap();
            let mut phase = 0.0;
            let mut shown = Vec::new();
            for seq in 0..12 {
                let mut block = [0.0; 960];
                tone(&mut block, &mut phase, hz, 48_000.0);
                mic.push_mono(48_000.0, &block);
                b_in.push_mono(48_000.0, &block);
                while b_out.received_seq(1, stream::VOICE) != Some(seq) || speaker.received_seq(2, stream::VOICE) != Some(seq) {
                    assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked before frame {seq} of the {hz} Hz call");
                    std::thread::yield_now();
                }
                if seq >= 3 {
                    block.fill(1.0);
                    speaker.fill_planar(48_000.0, 960, 1, &mut block);
                    if seq >= 7 { shown.extend_from_slice(&block); }
                }
            }
            let heard = tone_power(&shown, hz, 48_000.0);
            let absent = tone_power(&shown, other, 48_000.0);
            assert!(heard > 0.01 && absent < heard / 1000.0, "attached call at {hz} Hz: power {heard}, other call at {other} Hz: {absent}");
            control.detach();
            let mut block = [1.0; 960];
            speaker.fill_interleaved(48_000.0, 1, &mut block);
            assert!(block.iter().all(|&x| x == 0.0), "detaching the {hz} Hz call must silence the entire speaker block");
            control.tick();
            eprintln!("installed ports: {hz} Hz power {heard:.7}, other tone {absent:.7}, detached output silent");
            stop_both(a, b, &panicked);
        }
    }

    #[test]
    fn switching_devices_preserves_voice_frames_and_the_remote_tone() {
        use makepad_teamtalk::jitter::Take;

        let (mut a, mut b, panicked) = audio_pair();
        let (mut mic, mut speaker, mut ports) = audio::audio_ports();
        ports.attach(a.voice_in(), a.voice_out().unwrap());
        let mut b_in = b.voice_in().unwrap();
        let mut b_out = b.voice_out().unwrap();
        let (mut a_phase, mut b_phase) = (0.0, 0.0);
        // Prime A's jitter buffer. B keeps the same microphone throughout the switch.
        let mut prefill = [0.0; 3 * 960];
        tone(&mut prefill, &mut b_phase, 660.0, 48_000.0);
        b_in.push_mono(48_000.0, &prefill);

        let mut channels = [Vec::new(), Vec::new()];
        for block in 0..40 {
            let mut remote = [0.0; 480];
            tone(&mut remote, &mut b_phase, 660.0, 48_000.0);
            b_in.push_mono(48_000.0, &remote);
            let newest = 2 + (block + 1) / 2;
            while speaker.received_seq(2, stream::VOICE) != Some(newest) {
                assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked before B's voice frame {newest} reached A");
                std::thread::yield_now();
            }
            // Switch 190 ms in, halfway through a 20 ms microphone frame.
            if block < 19 {
                let mut stereo = [0.0; 960];
                tone(&mut stereo[..480], &mut a_phase, 440.0, 48_000.0);
                stereo.copy_within(..480, 480);
                mic.push_planar(48_000.0, 480, 2, &stereo);
                speaker.fill_planar(48_000.0, 480, 2, &mut stereo);
            } else {
                let mut mono = [0.0; 441];
                tone(&mut mono, &mut a_phase, 440.0, 44_100.0);
                mic.push_mono(44_100.0, &mono);
                let mut stereo = [0.0; 882];
                speaker.fill_interleaved(44_100.0, 2, &mut stereo);
                if block < 39 { // Exactly the first 200 ms of the new speaker.
                    for pair in stereo.chunks_exact(2) {
                        channels[0].push(pair[0]);
                        channels[1].push(pair[1]);
                    }
                }
            }
        }

        // Install a silent marker stream after the last capture. The net worker drains
        // the microphone before this new stream, so the marker at B covers even a lost last frame.
        let mut marker = a.add_audio_source(stream::APP_AUDIO).unwrap();
        marker.push_mono(voice::VOICE_RATE, &[0.0; voice::VOICE_FRAME]);
        while b_out.received_seq(1, stream::APP_AUDIO).is_none() {
            assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked before the end of A's microphone stream reached B");
            std::thread::yield_now();
        }
        // All twenty frames fit in B's jitter ring. Inspect each actual sequence before playout.
        let ring = b_out.received_ring(1, stream::VOICE).expect("B must receive A's microphone");
        let mut pcm = [0.0; makepad_teamtalk::MAX_FRAME];
        let missing: Vec<_> = (0..20).filter(|&seq| matches!(ring.take(seq, &mut pcm), Take::Missing)).collect();
        let power = channels.map(|samples| tone_power(&samples, 660.0, 44_100.0));
        eprintln!("device switch at 190 ms: B received {}/20 frames (0..19), missing {missing:?}; A's first 200 ms at 44.1 kHz, 660 Hz power L/R {power:.7?}", 20 - missing.len());
        assert!(missing.len() <= 1, "B may miss at most one of A's twenty frames across the switch; missing {missing:?}");
        assert!(power.iter().all(|&p| p > 0.01), "both new speaker channels must play B's 660 Hz tone in their first 200 ms: {power:?}");
        stop_both(a, b, &panicked);
    }

    fn frame_of(t: &Tile) -> u64 {
        u64::from_be_bytes(t.picture.as_ref().unwrap().nv12[..8].try_into().unwrap())
    }

    /// Sleeps 10 ms, then polls both calls and plays b's voice into a block. Adds the frame number
    /// b showed of a, if a fresh one came, and b's block if it was loud. Returns the events of a,
    /// then of b.
    fn tick(a: &mut Call, b: &mut Call, out: &mut voice::VoiceOut, frames: &mut Vec<u64>, loud: &mut usize) -> [Vec<CallEvent>; 2] {
        std::thread::sleep(Duration::from_millis(10));
        let events = [a.poll(), b.poll()];
        if let Some(t) = b.tiles().iter().find(|t| t.who == Some(1) && t.fresh) {
            frames.push(frame_of(t));
        }
        let mut block = [0f32; 480];
        out.mix_into_mono(48_000.0, &mut block);
        *loud += usize::from((block.iter().map(|s| s * s).sum::<f32>() / 480.0).sqrt() > 0.1);
        events
    }

    /// Polls both calls for `limit`, or until `done`. Returns the frame numbers b showed of a,
    /// and b's loud blocks.
    fn run(a: &mut Call, b: &mut Call, out: &mut voice::VoiceOut, limit: Duration, done: impl Fn(usize, usize) -> bool) -> (Vec<u64>, usize) {
        let (mut frames, mut loud) = (Vec::new(), 0);
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline && !done(frames.len(), loud) {
            tick(a, b, out, &mut frames, &mut loud);
        }
        (frames, loud)
    }

    /// `run` with no deadline, for a property that holds however slow the box is: polls until
    /// `done`, and fails at once on a problem either call reports, or a worker of either call that
    /// exits or panics, since then `done` may never come.
    fn run_until(a: &mut Call, b: &mut Call, out: &mut voice::VoiceOut, panicked: &AtomicUsize, mut done: impl FnMut(&Call, &Call, usize, usize) -> bool) -> (Vec<u64>, usize) {
        let (mut frames, mut loud) = (Vec::new(), 0);
        while !done(a, b, frames.len(), loud) {
            for (who, events) in ["a", "b"].into_iter().zip(tick(a, b, out, &mut frames, &mut loud)) {
                for e in events {
                    assert!(!matches!(e, CallEvent::Problem { .. }), "{who} reported {e:?}, with {} of a's pictures shown at b", frames.len());
                }
            }
            assert!(a.done == 0 && b.done == 0, "no worker may exit before stop: {} of a's and {} of b's did", a.done, b.done);
            let panicked = panicked.load(Ordering::Relaxed);
            assert_eq!(panicked, 0, "{panicked} worker(s) panicked, with {} of a's pictures shown at b", frames.len());
        }
        (frames, loud)
    }

    /// Stops both calls and waits, with no deadline, until every worker of both has exited. A
    /// panicked worker never reports, so it fails at once instead.
    fn stop_both(mut a: Call, mut b: Call, panicked: &AtomicUsize) {
        a.stop();
        b.stop();
        while !(a.stopped() && b.stopped()) {
            std::thread::sleep(Duration::from_millis(10));
            a.poll();
            b.poll();
            let panicked = panicked.load(Ordering::Relaxed);
            assert_eq!(panicked, 0, "{panicked} worker(s) panicked; {} of {} and {} of {} exited", a.done, a.spawned, b.done, b.spawned);
        }
    }

    #[test]
    fn voice_and_a_held_share_can_join_and_leave_one_running_link_in_either_order() {
        #[derive(Default)]
        struct Activity {
            wants: Vec<View>,
            pops: usize,
            keys: usize,
            repaint: bool,
            wake: Option<Arc<dyn Fn() + Send + Sync>>,
        }
        struct Held {
            activity: Arc<Mutex<Activity>>,
            encoder: Box<dyn Encoder>,
            pixels: Vec<u8>,
            count: i64,
        }
        impl EncodedSource for Held {
            fn set_wake(&mut self, wake: Arc<dyn Fn() + Send + Sync>) { self.activity.lock().unwrap().wake = Some(wake); }
            fn want(&mut self, width: u32, height: u32, fps: u32) { self.activity.lock().unwrap().wants.push(View { width, height, fps }); }
            fn set_kbps(&mut self, kbps: u32) { self.encoder.set_kbps(kbps); }
            fn key_now(&mut self) {
                self.encoder.request_key();
                let mut state = self.activity.lock().unwrap();
                state.keys += 1;
                state.repaint = true;
            }
            fn pop(&mut self) -> Option<codec::Packet> {
                let mut state = self.activity.lock().unwrap();
                state.pops += 1;
                if !std::mem::take(&mut state.repaint) { return None; }
                drop(state);
                self.count += 1;
                self.encoder.push(&self.pixels, self.count * 10).unwrap().pop()
            }
        }

        for share_first in [true, false] {
            let panics = Arc::new(AtomicUsize::new(0));
            let activity = Arc::new(Mutex::new(Activity::default()));
            let rung = crate::guard::LADDER[3];
            let source = Held { activity: activity.clone(), encoder: TestCodecs.encoder(rung, 198).unwrap(),
                pixels: vec![128; (rung.width * rung.height * 3 / 2) as usize], count: 0 };
            let caps = VideoCaps { width: rung.width, height: rung.height, fps: rung.fps, content: Content::Share };
            let mut offer = Some(VideoConfig { caps, source: VideoSource::Encoded(Box::new(source)) });
            let mut cfg = config(1, false, 440., Arc::new(TestCodecs), &panics);
            let mut other = config(2, false, 660., Arc::new(TestCodecs), &panics);
            let waiter = std::thread::current();
            cfg.signal = Arc::new(move || waiter.unpark());
            other.signal = cfg.signal.clone();
            if share_first {
                cfg.video = offer.take();
                cfg.voice = VoiceSource::Off;
                other.voice = VoiceSource::Off;
            }
            let mut a = Call::start(cfg);
            let mut b = Call::start(other);
            let (ab, from_a) = sync_channel(256);
            let (ba, from_b) = sync_channel(256);
            let mut paths = [
                (crate::sim::Path::new(Default::default(), 1), from_a, b.inlet()),
                (crate::sim::Path::new(Default::default(), 2), from_b, a.inlet()),
            ];
            a.join(Join { peer: 2, their_pub: device(2), key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out: ab, stream_cap: None });
            b.join(Join { peer: 1, their_pub: device(1), key_id: 0, send_secret: [2; 32], recv_secret: [1; 32], out: ba, stream_cap: None });
            let mut packets = Vec::new();
            let mut sim_us = 0;
            let mut pump = |a: &mut Call, b: &mut Call| {
                // Signals report progress; the periodic wake also advances the simulated paths
                // and observes a panicked worker. No elapsed time decides a test outcome.
                std::thread::park_timeout(Duration::from_millis(5));
                sim_us += 5_000;
                for (path, from, to) in &mut paths {
                    for d in from.try_iter() { path.send(sim_us, d); }
                    path.poll(sim_us, &mut packets);
                    for d in packets.drain(..) {
                        if let Err(std::sync::mpsc::TrySendError::Full(_)) = to.try_send((SocketAddr::from(([192, 0, 2, 1], 47100)), d)) {
                            panic!("the simulated link's inlet overflowed, share-first={share_first}");
                        }
                    }
                }
                for event in a.poll().into_iter().chain(b.poll()) {
                    if let CallEvent::Problem { what, .. } = event { panic!("share-first={share_first}: {what}"); }
                }
                if let Some((1, view)) = b.take_view() { a.set_peer_view(2, view); }
                assert_eq!(panics.load(Ordering::Relaxed), 0, "a worker panicked, share-first={share_first}");
            };
            if share_first {
                assert!(a.voice_in().is_none() && b.voice_out().is_none(), "a share-only link should open no audio");
            } else {
                while b.source_level(1, stream::VOICE).is_none() { pump(&mut a, &mut b); }
                let VideoConfig { caps, source: VideoSource::Encoded(source) } = offer.take().unwrap() else { unreachable!() };
                a.add_share(caps, source);
            }
            while a.stats.fixed_cell_share_kbps != Some(0) || a.stats.peers.is_empty() { pump(&mut a, &mut b); }
            let before = a.stats.peers[0].session.sent;
            {
                let state = activity.lock().unwrap();
                assert_eq!(state.wants, [View::default()], "the unopened share must ask its owner for zero pixels");
                assert_eq!(state.pops, 0, "an unwatched source must never be popped");
            }
            let panel = View { width: caps.width, height: caps.height, fps: caps.fps };
            b.begin_watch(1);
            b.watch_share(1, panel);
            assert_eq!(b.take_view(), Some((1, panel)), "the reliable opening must leave on this input, before another poll");
            a.set_peer_view(2, panel);
            while b.share_picture(1).unwrap().picture.is_none() { pump(&mut a, &mut b); }
            assert_eq!(activity.lock().unwrap().keys, 1, "a late viewer must request one key of the held window");
            assert!(b.tiles[1].picture.is_none(), "a share must have its own picture slot");
            if share_first {
                a.add_voice(VoiceSource::Synthetic { tone_hz: 440. });
                b.add_voice(VoiceSource::Feed);
                while b.source_level(1, stream::VOICE).is_none() { pump(&mut a, &mut b); }
                a.remove_share();
                b.set_peer_sharing(1, false);
                let mut speaker = b.voice_out().unwrap();
                let first = speaker.received_seq(1, stream::VOICE).unwrap_or(0);
                while a.stats.fixed_cell_share_kbps.is_some() || speaker.received_seq(1, stream::VOICE).is_none_or(|seq| seq <= first) {
                    pump(&mut a, &mut b);
                }
            } else {
                a.remove_voice();
                b.remove_voice();
                b.watch_share(1, View::default());
                while a.stats.fixed_cell_share_kbps != Some(0) { pump(&mut a, &mut b); }
                assert!(!b.stopped(), "closing the only panel must preserve the outstanding offer");
                b.watch_share(1, View { width: caps.width, height: caps.height, fps: caps.fps });
                while b.share_picture(1).unwrap().picture.is_none() { pump(&mut a, &mut b); }
                assert_eq!(activity.lock().unwrap().keys, 2, "reopening must deliver a fresh key of the held window");
            }
            while a.stats.peers[0].session.sent <= before { pump(&mut a, &mut b); }
            assert_eq!(a.stats.peers.len(), 1, "both media must keep the original link");
            if share_first {
                a.remove_voice();
                b.remove_voice();
            } else {
                a.remove_share();
                b.set_peer_sharing(1, false);
            }
            while !a.stopped() || !b.stopped() { pump(&mut a, &mut b); }
        }
    }

    /// Two calls over an in-process link with the test codec: pictures arrive in order and voice
    /// plays; with the camera off no more pictures come; stop ends every worker.
    #[test]
    fn two_calls_exchange_pictures_and_voice_then_stop() {
        let (mut a, mut b, panicked) = pair(true, Arc::new(TestCodecs), 256, 0);
        let mut out = b.voice_out().unwrap();
        let (frames, _) = run_until(&mut a, &mut b, &mut out, &panicked, |_, _, f, l| f >= 20 && l >= 10);
        assert!(frames.windows(2).all(|w| w[0] < w[1]), "a's pictures must arrive in order: {frames:?}");
        assert!(a.stats().pictures_sent >= 20 && a.stats().keys_sent >= 1, "a's stats count its pictures: {:?}", a.stats());
        assert_eq!(b.tiles()[1].state, TileState::Live, "a's tile at b is live");

        a.set_camera(false);
        run(&mut a, &mut b, &mut out, Duration::from_millis(300), |_, _| false);
        let (after, _) = run(&mut a, &mut b, &mut out, Duration::from_millis(400), |_, _| false);
        assert!(after.len() <= 1, "with a's camera off b should get no more pictures, got {after:?}");
        stop_both(a, b, &panicked);
    }

    #[test]
    fn successive_calls_share_the_link_inlets_and_reject_the_old_voice() {
        use crate::wire::{stream, Header};

        let panicked = Arc::new(AtomicUsize::new(0));
        let (left, right) = (Inlet::new(), Inlet::new());
        let start = |id, inlet: &Inlet| {
            let mut cfg = config(id, false, 440.0, Arc::new(TestCodecs), &panicked);
            cfg.inlet = Some(inlet.clone());
            Call::start(cfg)
        };
        let join = |id, key, other, out| Join {
            peer: id as u64, their_pub: device(id), key_id: 0, send_secret: [key; 32],
            recv_secret: [other; 32], out, stream_cap: None,
        };

        let (mut old_a, mut old_b) = (start(1, &left), start(2, &right));
        let (tx, rx) = sync_channel(256);
        old_a.join(join(2, 1, 2, tx));
        old_b.join(join(1, 2, 1, link(left.sender(), 256, 0)));
        let mut late = Vec::new();
        while late.len() < 3 {
            assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked while collecting the first call's voice");
            while let Ok(d) = rx.try_recv() {
                if Header::parse(&d).is_some_and(|(h, _)| h.stream == stream::VOICE) {
                    late.push(d);
                }
            }
            std::thread::yield_now();
        }
        old_a.stop();
        old_b.stop();
        // Keep the same call id and sender tag so only the new secrets distinguish the calls.
        let (mut a, mut b) = (start(1, &left), start(2, &right));
        b.join(join(1, 4, 3, link(left.sender(), 256, 0)));
        while b.stats().peers.is_empty() {
            b.poll();
            assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked before the second call joined");
            std::thread::sleep(Duration::from_millis(2));
        }
        let stale = late.len() as u64;
        for d in late {
            right.sender().send((SocketAddr::from(([192, 0, 2, 1], 47100)), d)).unwrap();
        }
        a.join(join(2, 3, 4, link(right.sender(), 256, 0)));
        let mut out = b.voice_out().unwrap();
        let (_, loud) = run_until(&mut a, &mut b, &mut out, &panicked, |_, _, _, l| l >= 10);
        while b.stats().peers[0].session.received == 0 {
            b.poll();
            assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked before reporting the second call's voice");
            std::thread::sleep(Duration::from_millis(2));
        }
        let refused = b.stats().peers[0].session.bad_tag;
        assert_eq!(refused, stale, "all {stale} late voice packets must fail the new key before playout");
        eprintln!("shared inlet: {refused}/{stale} stale voice packets failed authentication, {loud} loud blocks from the new call");
        stop_both(a, b, &panicked);
        stop_both(old_a, old_b, &panicked);
    }

    /// After carrying over twice 150 kbit/s, the link slows to that rate. Its backlog makes
    /// the encoder back off, and the backlog drains without B skipping any of A's pictures.
    #[test]
    fn a_slowed_link_backs_the_encoder_off_and_its_backlog_drains() {
        const SLOW_BPS: u64 = 150_000;
        let panicked = Arc::new(AtomicUsize::new(0));
        let start = |id, hz| {
            let mut cfg = config(id, true, hz, Arc::new(TestCodecs), &panicked);
            let (spawn, clocks) = test_clock::spawn_on_ticks(cfg.spawn);
            cfg.spawn = spawn;
            let test = std::thread::current();
            cfg.signal = Arc::new(move || test.unpark());
            let call = Call::start(cfg);
            (call, test_clock::CallTicks::registered(clocks))
        };
        let (mut a, a_clock) = start(1, 440.0);
        let (mut b, b_clock) = start(2, 660.0);
        let (a_to_b, mut forward) = clocked_link(b.inlet(), 8);
        let (b_to_a, mut reverse) = clocked_link(a.inlet(), 256);
        a.join(Join { peer: 2, their_pub: device(2), key_id: 0, send_secret: [1; 32], recv_secret: [2; 32], out: a_to_b, stream_cap: None });
        b.join(Join { peer: 1, their_pub: device(1), key_id: 0, send_secret: [2; 32], recv_secret: [1; 32], out: b_to_a, stream_cap: None });

        let mut next_capture = 0;
        let mut previous_bytes = 0;
        let mut fast_rates = Vec::new();
        let mut slowed = None;
        let mut peak = 0;
        let mut series = Vec::new();
        let mut drained = None;
        // Feed a finite 20 s recording. A missing transition fails with the reports even if
        // the host is suspended. Each worker completes its turn before media time advances.
        for now in (0..20_000_000).step_by(2_000) {
            if now % 10_000 == 0 {
                a_clock.voice.at(now);
                b_clock.voice.at(now);
            }
            if now >= next_capture {
                a_clock.encode.at(next_capture);
                b_clock.encode.at(next_capture);
                next_capture += 33_333;
            }
            forward(now, if slowed.is_some() { SLOW_BPS } else { 0 });
            reverse(now, 0);
            a_clock.network.at(now);
            b_clock.network.at(now);
            for call in [&mut a, &mut b] {
                for event in call.poll() {
                    assert!(!matches!(event, CallEvent::Problem { .. }), "worker reported {event:?} at {now} µs");
                }
                assert_eq!(call.done, 0, "a worker exited before stop at {now} µs");
            }
            if now == 0 || now % 250_000 != 0 { continue; }
            let peer = &a.stats().peers[0];
            if let Some((at, _, _)) = slowed {
                peak = peak.max(peer.backlog_us);
                series.push((peer.backlog_us / 1000, peer.backlog, a.stats().asked_kbps));
                if peak >= 50_000 && peer.backlog_us < 20_000 {
                    drained = Some((now - at) / 1000);
                    break;
                }
            } else if now % 1_000_000 == 0 {
                let stamped = (peer.session.sent_bytes - previous_bytes) * 8;
                previous_bytes = peer.session.sent_bytes;
                fast_rates.push(stamped);
                if stamped > 2 * SLOW_BPS {
                    assert!(b.stats().peers[0].video.is_some_and(|v| v.frames > 0), "pictures flow before the link slows");
                    // Let the asynchronous decoder present that first picture, independent of host load.
                    while b.tiles()[1].picture.is_none() {
                        assert_eq!(panicked.load(Ordering::Relaxed), 0, "decoder panicked before presenting the first picture");
                        std::thread::park();
                        b.poll();
                    }
                    slowed = Some((now, a.stats().asked_kbps, peer.link_full));
                }
            }
        }
        let (_, asked_fast, full_before) = slowed.unwrap_or_else(|| panic!("expected stamped rate above 300000 bit/s while pictures flow: {fast_rates:?}"));
        let peer = &a.stats().peers[0];
        let asked_slow = a.stats().asked_kbps;
        eprintln!("slowed link: stamped {fast_rates:?} bit/s, ask {asked_fast} -> {asked_slow} kbit/s, peak {peak} µs, drain {drained:?} ms; reports (longest wait ms, datagrams, asked kbit/s): {series:?}");
        assert!(peer.link_full > full_before, "the slowed link must fill: it was full {full_before} times before, {} after", peer.link_full);
        assert!(peak >= 50_000, "a link slowed to 150 kbit/s should build a backlog of 50 ms or more, the longest wait was {} ms", peak / 1000);
        assert!(drained.is_some() && peer.backlog_us < 20_000, "the backlog should drain under 20 ms once the encoder backs off: {series:?}");
        assert!(asked_slow < asked_fast, "the encoder should back off from {asked_fast} kbit/s, it asks {asked_slow}");
        let video = b.stats().peers[0].video.expect("B received A's pictures");
        assert_eq!(video.skipped, 0, "nothing was dropped, so b's receiver skips no picture of a's: {video:?}");

        a.stop();
        b.stop();
        drop((a_clock, b_clock));
        loop {
            a.poll();
            b.poll();
            assert_eq!(panicked.load(Ordering::Relaxed), 0, "worker panicked during shutdown");
            if a.stopped() && b.stopped() { break; }
            std::thread::park();
        }
    }

    /// Two calls joined in either order, the second 200 ms after the first, skip no picture of each
    /// other's from the first one and ask for no key frame: what came before its sender was joined
    /// waits for the join (it races the answer), and someone who joins gets a key frame first.
    #[test]
    fn a_call_joined_in_either_order_skips_no_picture_from_the_first() {
        for a_first in [true, false] {
            let order = if a_first { "a joined first" } else { "b joined first" };
            let panicked = Arc::new(AtomicUsize::new(0));
            let mut a = Call::start(config(1, true, 440.0, Arc::new(TestCodecs), &panicked));
            let mut b = Call::start(config(2, true, 660.0, Arc::new(TestCodecs), &panicked));
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
            let got = |c: &Call| c.stats().peers.first().and_then(|p| p.video).unwrap_or_default();
            let mut shown_at_a = 0;
            let (shown_at_b, _) = run_until(&mut a, &mut b, &mut out, &panicked, |a, b, shown_at_b, _| {
                shown_at_a += usize::from(a.tiles().iter().any(|t| t.who == Some(2) && t.fresh));
                // Each fresh tile counts a decoded picture. The receiver reports carry the skip and key counters.
                shown_at_a >= 10 && shown_at_b >= 10 && got(a).frames >= 10 && got(b).frames >= 10
            });
            let (at_a, at_b) = (got(&a), got(&b));
            eprintln!("{order}: decoded pictures shown at A {shown_at_a}, at B {}; receiver frames A {}, B {}", shown_at_b.len(), at_a.frames, at_b.frames);
            assert!(at_a.frames >= 10 && at_b.frames >= 10, "{order}: both should receive at least ten whole pictures: a {at_a:?}, b {at_b:?}");
            assert_eq!((at_a.skipped, at_a.key_requests), (0, 0), "{order}: a skips none of b's pictures and asks for no key frame: {at_a:?}");
            assert_eq!((at_b.skipped, at_b.key_requests), (0, 0), "{order}: b skips none of a's pictures and asks for no key frame: {at_b:?}");
            assert_eq!((a.stats().strays, b.stats().strays), (0, 0), "{order}: every early datagram was claimed by the join");
            stop_both(a, b, &panicked);
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
        let (mut a, mut b, panicked) = pair(false, codecs.clone(), 256, 0);
        let tap = a.transcript_tap().expect("the microphone has a tap");
        assert!(a.transcript_tap().is_none(), "only one reader may take the call's tap");
        tap.set_on(true);
        let mut out = b.voice_out().unwrap();
        let (frames, _) = run_until(&mut a, &mut b, &mut out, &panicked, |_, _, _, l| l >= 10);
        let mut samples = Vec::new();
        assert!(tap.pop(&mut samples).is_some(), "the synthetic microphone goes through the call's framer");
        assert!(frames.is_empty(), "a voice-only call shows no pictures");
        let (e, d) = (codecs.encoders.load(Ordering::Relaxed), codecs.decoders.load(Ordering::Relaxed));
        assert_eq!((e, d), (0, 0), "a voice-only call must open no encoder or decoder, opened {e} and {d}");
        stop_both(a, b, &panicked);
    }

    /// One slot cannot hold the three packets of the first key. After filling, the link serves
    /// about 77 packets/s: room for 50 voice and 20 feedback, below voice plus 30 fps video.
    #[test]
    fn a_full_link_delays_media_and_loses_none() {
        let (mut a, mut b, panicked) = pair(true, Arc::new(TestCodecs), 1, 13);
        let mut out = b.voice_out().unwrap();
        let (frames, _) = run_until(&mut a, &mut b, &mut out, &panicked, |_, _, f, _| f >= 15);
        assert!(frames.windows(2).all(|w| w[0] < w[1]), "pictures arrive in order: {frames:?}");
        let full = a.stats().peers.first().map_or(0, |p| p.link_full);
        assert!(full > 0, "the slow link must have been full at least once, stats {:?}", a.stats().peers);
        let lost = a.stats().peers.first().map_or(0, |p| p.session.resent);
        eprintln!("full link: link_full={full}, pictures={}, resends={lost}", frames.len());
        assert_eq!(lost, 0, "nothing is lost on a full link, so nothing is resent");
        stop_both(a, b, &panicked);
    }
}
