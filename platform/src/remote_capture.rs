//! `/cap/start` and `/cap/stop`: the app encodes its own presented frames
//! into an mp4, in process, at native pixels.
//!
//! Under the virtual clock (`--virtual-clock`) every `/step` frame of the
//! captured window carries a capture request: the presenting command buffer
//! reads the drawable back (the same blit a `/g` grab uses, without the PNG),
//! and the frame lands in the file at `pts = n / fps`, `n` counted from the
//! first captured step. Nothing else the window presents is recorded, so a
//! frame drawn for a `wait=1` input or a grab between steps never shows up.
//! Without the virtual clock a standing screen-capture sink takes every
//! presented frame and places it by wall time.
//!
//! Threads: the UI only arms requests and counts; the GPU completion thread
//! hands the pixels (`Arc`, no copy) to the encoder worker through a
//! channel; the worker converts, hashes and encodes. The worker never runs
//! ahead of what the UI armed, and the UI stops arming new step frames while
//! the worker is `MAX_FRAMES_AHEAD` behind, so a slow encoder slows `/step`
//! down instead of dropping frames. Audio is the app's own output tap, fed on
//! the same sample clock as the picture (`(n + 1) * rate / fps` samples after
//! frame `n`), padded with silence where the device supplied none.
use crate::audio::AudioBuffer;
use crate::texture::{ReadbackChannelOrder, ReadbackOrigin};
use crate::video_file::{
    PcmAudioTrackOptions, VideoFileCodec, VideoFileEncoder, VideoFileEncoderOptions,
};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Capture request ids: inside the remote grab range (`1 << 62 .. 1 << 63`),
/// in its upper quarter, which ordinary grab ids never reach. The session
/// sits in bits 40..56 and the frame index in the low 40.
const CAPTURE_ID_TAG: u64 = 3 << 61;
const INDEX_BITS: u32 = 40;
const SESSION_MASK: u64 = 0xffff;

pub(super) fn capture_id(session: u64, index: u64) -> u64 {
    CAPTURE_ID_TAG | ((session & SESSION_MASK) << INDEX_BITS) | (index & ((1 << INDEX_BITS) - 1))
}

pub(super) fn is_capture_id(id: u64) -> bool {
    id >> 61 == 3
}

fn split_capture_id(id: u64) -> (u64, u64) {
    ((id >> INDEX_BITS) & SESSION_MASK, id & ((1 << INDEX_BITS) - 1))
}

/// How many armed frames the encoder may trail before `/step` waits for it.
/// A bound on memory (each is a full drawable), met by slowing down.
pub(super) const MAX_FRAMES_AHEAD: u64 = 6;
/// After `/cap/stop`, how long the worker waits for a frame that was armed
/// but never delivered (a present the GPU dropped) before finishing without
/// it. Logged when it happens.
const MISSING_FRAME_GRACE: Duration = Duration::from_secs(10);
/// How long the audio device gets to announce its rate before the AAC track
/// is created at the fallback rate.
const AUDIO_RATE_GRACE: Duration = Duration::from_millis(500);
const FALLBACK_AUDIO_RATE: u32 = 48_000;

pub(super) enum Pixels {
    /// A backend readback: `stride` bytes per row, in `order` (Metal).
    #[cfg_attr(
        not(all(not(gpusim), any(target_os = "macos", target_os = "ios", target_os = "tvos"))),
        allow(dead_code)
    )]
    Raw {
        data: Arc<[u8]>,
        stride: usize,
        order: ReadbackChannelOrder,
        origin: ReadbackOrigin,
    },
    /// Tightly packed top-down RGBA.
    Rgba(Vec<u8>),
    /// A backend that only answers grabs as PNG.
    Png(Vec<u8>),
}

pub(super) enum CaptureMsg {
    Frame {
        index: u64,
        width: u32,
        height: u32,
        pixels: Pixels,
    },
    /// A step frame that was armed but will never be delivered (its present
    /// was abandoned): counted as missing so later frames are not held up.
    Skip { index: u64 },
    /// Finish: under the virtual clock after `frames` frames, otherwise with
    /// whatever has arrived.
    Stop { frames: Option<u64> },
}

/// What the UI and the worker count, shared.
#[derive(Default)]
pub(super) struct CaptureCounters {
    /// Frames handed over by the GPU completion path.
    pub delivered: AtomicU64,
    /// Frames the worker has encoded (or given up on).
    pub consumed: AtomicU64,
    /// Wall-clock frames dropped because the encoder was behind.
    pub dropped: AtomicU64,
    /// `/cap/pause`: nothing is written (frames or sound) until `/cap/resume`;
    /// the file continues without a gap.
    pub paused: AtomicBool,
    /// Set once the worker has stopped on an error (`error` says which); the
    /// UI stops arming frames and `/step` reports it instead of waiting.
    pub failed: AtomicBool,
    pub error: OnceLock<String>,
}

impl CaptureCounters {
    /// The worker's error, once it has failed. Lock-free (the UI asks).
    pub fn failure(&self) -> Option<String> {
        if !self.failed.load(Ordering::Acquire) {
            return None;
        }
        Some(self.error.get().cloned().unwrap_or_else(|| "capture encoder failed".into()))
    }
}

pub(super) struct CaptureConfig {
    pub path: String,
    pub fps: u32,
    pub audio: bool,
    pub virtual_clock: bool,
}

pub(super) struct CaptureResult {
    pub path: String,
    pub frames: u64,
    pub width: u32,
    pub height: u32,
    pub missing: u64,
    pub hash: u64,
    pub frame_hashes: Vec<u64>,
    pub audio_rate: Option<u32>,
}

/// The HTTP threads' and the GPU completion thread's view of the session.
/// The UI never takes this lock.
struct Link {
    session: u64,
    tx: Sender<CaptureMsg>,
    counters: Arc<CaptureCounters>,
    done: Option<Receiver<Result<CaptureResult, String>>>,
    screen_sink: Option<u64>,
}

fn link() -> &'static Mutex<Option<Link>> {
    static LINK: OnceLock<Mutex<Option<Link>>> = OnceLock::new();
    LINK.get_or_init(|| Mutex::new(None))
}

/// True while a capture session is open: the hands-off frame stays out of it.
static CAPTURING: AtomicBool = AtomicBool::new(false);
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
/// The open session's id, 0 when none: the UI checks a `CapStart` it
/// applies late (after its request timed out and closed the session).
static OPEN_SESSION: AtomicU64 = AtomicU64::new(0);

pub(super) fn session_open(session: u64) -> bool {
    OPEN_SESSION.load(Ordering::Acquire) == session
}

pub(super) fn capturing() -> bool {
    CAPTURING.load(Ordering::Relaxed)
}

/// Everything a new session needs; the UI gets the receiving half and the
/// worker's result sender, the link keeps the rest.
pub(super) struct NewSession {
    pub session: u64,
    pub tx: Sender<CaptureMsg>,
    pub rx: Receiver<CaptureMsg>,
    pub counters: Arc<CaptureCounters>,
    pub done_tx: Sender<Result<CaptureResult, String>>,
}

/// Open a session on the link (HTTP thread). Refused while one is open.
pub(super) fn open_session() -> Result<NewSession, String> {
    let mut link = link().lock().unwrap();
    if link.is_some() {
        return Err("a capture is already running; /cap/stop it first".into());
    }
    let session = (NEXT_SESSION.fetch_add(1, Ordering::Relaxed) & SESSION_MASK).max(1);
    let (tx, rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let counters = Arc::new(CaptureCounters::default());
    *link = Some(Link {
        session,
        tx: tx.clone(),
        counters: counters.clone(),
        done: Some(done_rx),
        screen_sink: None,
    });
    CAPTURING.store(true, Ordering::Relaxed);
    OPEN_SESSION.store(session, Ordering::Release);
    Ok(NewSession { session, tx, rx, counters, done_tx })
}

/// Drop a session that could not start, or has finished.
pub(super) fn close_session() {
    OPEN_SESSION.store(0, Ordering::Release);
    if let Some(link) = link().lock().unwrap().take() {
        if let Some(id) = link.screen_sink {
            crate::screen_capture::remove_screen_capture(id);
        }
    }
    CAPTURING.store(false, Ordering::Relaxed);
}

/// Check a capture path before anything is started (HTTP thread): an
/// absolute `.mp4` in a directory that exists, not an existing file unless
/// `overwrite`, and writable. A path the encoder could not open would
/// otherwise only fail at the first captured frame.
pub(super) fn check_output_path(path: &str, overwrite: bool) -> Result<(), String> {
    let file = std::path::Path::new(path);
    if !file.is_absolute()
        || !path.to_ascii_lowercase().ends_with(".mp4")
        || path.chars().any(char::is_control)
    {
        return Err("cap/start path must be an absolute .mp4 path".into());
    }
    let parent = file.parent().filter(|parent| parent.is_dir()).ok_or_else(|| {
        format!("cap/start: the directory of {path} does not exist (it is not created)")
    })?;
    if file.exists() {
        if !overwrite {
            return Err(format!("cap/start: {path} exists; add overwrite=1 to replace it"));
        }
        if !file.is_file() {
            return Err(format!("cap/start: {path} is not a file"));
        }
        std::fs::OpenOptions::new()
            .write(true)
            .open(file)
            .map_err(|err| format!("cap/start: {path} is not writable: {err}"))?;
    } else {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(file)
            .map_err(|err| format!("cap/start: cannot write in {}: {err}", parent.display()))?;
        let _ = std::fs::remove_file(file);
    }
    Ok(())
}

/// Wall-clock mode: every presented frame of `window_id`, placed by when it
/// was presented. Installed from the HTTP thread (it takes the sink registry
/// lock, which the GPU completion thread holds while delivering).
pub(super) fn install_screen_sink(window_id: usize, fps: u32) {
    let Some((tx, counters)) = link()
        .lock()
        .unwrap()
        .as_ref()
        .map(|link| (link.tx.clone(), link.counters.clone()))
    else {
        return;
    };
    let mut first_ns = None;
    // Time spent paused comes off the frame clock, so the file is gapless.
    let mut paused_ns = 0u64;
    let mut pause_began: Option<u64> = None;
    let id = crate::screen_capture::add_screen_capture(
        crate::screen_capture::ScreenCaptureOptions {
            window_id: Some(window_id),
            max_fps: fps as f64,
        },
        move |frame| {
            if counters.paused.load(Ordering::Acquire) {
                pause_began.get_or_insert(frame.time_ns);
                return;
            }
            if let Some(began) = pause_began.take() {
                paused_ns += frame.time_ns.saturating_sub(began);
            }
            let first = *first_ns.get_or_insert(frame.time_ns);
            let index = wall_frame_index(frame.time_ns.saturating_sub(first + paused_ns), fps);
            // The encoder is behind: this frame is dropped (its time stays
            // a gap in the file) rather than queued without bound.
            let queued = counters
                .delivered
                .load(Ordering::Acquire)
                .saturating_sub(counters.consumed.load(Ordering::Acquire));
            if queued >= MAX_FRAMES_AHEAD || counters.failed.load(Ordering::Acquire) {
                counters.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            if tx
                .send(CaptureMsg::Frame {
                    index,
                    width: frame.width,
                    height: frame.height,
                    pixels: Pixels::Rgba(frame.rgba.to_vec()),
                })
                .is_ok()
            {
                counters.delivered.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    if let Some(link) = link().lock().unwrap().as_mut() {
        link.screen_sink = Some(id);
    } else {
        crate::screen_capture::remove_screen_capture(id);
    }
}

fn wall_frame_index(elapsed_ns: u64, fps: u32) -> u64 {
    ((elapsed_ns as u128 * fps as u128 + 500_000_000) / 1_000_000_000) as u64
}

/// Stop the wall-clock sink before the UI tells the worker to finish.
pub(super) fn remove_screen_sink() {
    let id = link().lock().unwrap().as_mut().and_then(|link| link.screen_sink.take());
    if let Some(id) = id {
        crate::screen_capture::remove_screen_capture(id);
    }
}

/// Wait for the worker's result (HTTP thread), then close the session.
pub(super) fn wait_for_result(timeout: Duration) -> Result<CaptureResult, String> {
    let done = link().lock().unwrap().as_mut().and_then(|link| link.done.take());
    let Some(done) = done else {
        return Err("no capture is running".into());
    };
    let result = match done.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err("the capture encoder did not finish in time".into()),
    };
    close_session();
    result
}

/// A readback the GPU completion path produced for a capture id. Returns
/// false for any other id. Never blocks on the UI.
pub(super) fn deliver(id: u64, width: u32, height: u32, pixels: Pixels) -> bool {
    if !is_capture_id(id) {
        return false;
    }
    let (session, index) = split_capture_id(id);
    let Ok(link) = link().lock() else {
        return true;
    };
    if let Some(link) = link.as_ref().filter(|link| link.session == session) {
        if link
            .tx
            .send(CaptureMsg::Frame {
                index,
                width,
                height,
                pixels,
            })
            .is_ok()
        {
            link.counters.delivered.fetch_add(1, Ordering::AcqRel);
        }
    }
    true
}

pub(super) struct AudioQueue {
    /// 0 until the first tapped buffer says what the device runs at.
    rate: u32,
    /// Interleaved stereo.
    samples: VecDeque<i16>,
    /// Seconds of backlog kept; older samples are dropped. Under the virtual
    /// clock the device runs in real time while frames may be stepped
    /// slower, so only a short backlog is kept (the track stays aligned to
    /// the frames, not to what the device played meanwhile).
    backlog_seconds: f64,
}

impl AudioQueue {
    pub(super) fn new(virtual_clock: bool) -> Self {
        Self {
            rate: 0,
            samples: VecDeque::new(),
            backlog_seconds: if virtual_clock { 0.1 } else { 4.0 },
        }
    }
}

/// The tap body: realtime thread, never blocks (a busy queue skips a buffer).
pub(super) fn tap_audio(
    queue: &Mutex<AudioQueue>,
    counters: &CaptureCounters,
    sample_rate: f64,
    buffer: &AudioBuffer,
) {
    // Paused: the device's sound is not recorded either.
    if counters.paused.load(Ordering::Relaxed) {
        return;
    }
    let Ok(mut queue) = queue.try_lock() else {
        return;
    };
    if queue.rate == 0 {
        queue.rate = sample_rate.round().max(1.0) as u32;
    }
    let frames = buffer.frame_count();
    let channels = buffer.channel_count();
    if frames == 0 || channels == 0 {
        return;
    }
    let left = buffer.channel(0);
    let right = if channels > 1 { buffer.channel(1) } else { left };
    for i in 0..frames.min(left.len()).min(right.len()) {
        queue.samples.push_back((left[i].clamp(-1.0, 1.0) * 32767.0) as i16);
        queue.samples.push_back((right[i].clamp(-1.0, 1.0) * 32767.0) as i16);
    }
    let cap = (queue.rate as f64 * queue.backlog_seconds) as usize * 2;
    if queue.samples.len() > cap {
        let excess = queue.samples.len() - cap;
        queue.samples.drain(..excess);
    }
}

/// The encoder worker: runs until `Stop`, then finalizes the file.
pub(super) fn run_worker(
    config: CaptureConfig,
    rx: Receiver<CaptureMsg>,
    counters: Arc<CaptureCounters>,
    audio: Option<Arc<Mutex<AudioQueue>>>,
) -> Result<CaptureResult, String> {
    let result = run_worker_inner(config, rx, counters.clone(), audio);
    if let Err(err) = &result {
        crate::log!("[makepad-remote] capture failed: {err}");
        let _ = counters.error.set(err.clone());
        counters.failed.store(true, Ordering::Release);
    }
    result
}

fn run_worker_inner(
    config: CaptureConfig,
    rx: Receiver<CaptureMsg>,
    counters: Arc<CaptureCounters>,
    audio: Option<Arc<Mutex<AudioQueue>>>,
) -> Result<CaptureResult, String> {
    let mut state = Worker {
        config,
        counters,
        audio,
        audio_rate: None,
        audio_pushed: 0,
        encoder: None,
        canvas: Vec::new(),
        width: 0,
        height: 0,
        next: 0,
        last: None,
        frames: 0,
        missing: 0,
        hash: 0xcbf2_9ce4_8422_2325,
        frame_hashes: Vec::new(),
    };
    let mut pending: BTreeMap<u64, Option<(u32, u32, Pixels)>> = BTreeMap::new();
    let mut stop: Option<Option<u64>> = None;
    loop {
        match rx.recv_timeout(MISSING_FRAME_GRACE) {
            Ok(CaptureMsg::Frame {
                index,
                width,
                height,
                pixels,
            }) => {
                // A later readback of the same frame replaces an earlier one.
                pending.insert(index, Some((width, height, pixels)));
            }
            Ok(CaptureMsg::Skip { index }) => {
                pending.entry(index).or_insert(None);
            }
            Ok(CaptureMsg::Stop { frames }) => stop = Some(frames),
            Err(RecvTimeoutError::Timeout) if stop.is_none() => {
                // A frame that never came (its readback was lost) stops
                // holding up the ones after it once it is this late.
                if !pending.is_empty() {
                    crate::log!(
                        "[makepad-remote] capture: frame {} never arrived, continuing without it",
                        state.next
                    );
                    state.drain(&mut pending, true)?;
                }
                continue;
            }
            Err(_) => break,
        }
        state.drain(&mut pending, false)?;
        match stop {
            Some(Some(frames)) if state.next >= frames => break,
            Some(None) => break,
            _ => {}
        }
    }
    // Frames still waiting on one that never came: encode what there is.
    state.drain(&mut pending, true)?;
    if let Some(Some(frames)) = stop {
        if state.next < frames {
            state.missing += frames - state.next;
            crate::log!(
                "[makepad-remote] capture: {} armed frame(s) never arrived",
                frames - state.next
            );
        }
    }
    state.finish()
}

struct Worker {
    config: CaptureConfig,
    counters: Arc<CaptureCounters>,
    audio: Option<Arc<Mutex<AudioQueue>>>,
    audio_rate: Option<u32>,
    audio_pushed: u64,
    encoder: Option<VideoFileEncoder>,
    canvas: Vec<u8>,
    width: u32,
    height: u32,
    /// Virtual clock: the next frame index the file needs.
    next: u64,
    /// Wall clock: the last index written.
    last: Option<u64>,
    frames: u64,
    missing: u64,
    hash: u64,
    frame_hashes: Vec<u64>,
}

impl Worker {
    fn drain(
        &mut self,
        pending: &mut BTreeMap<u64, Option<(u32, u32, Pixels)>>,
        flush: bool,
    ) -> Result<(), String> {
        loop {
            let Some((&index, _)) = pending.first_key_value() else {
                return Ok(());
            };
            if self.config.virtual_clock {
                if index < self.next {
                    pending.remove(&index);
                    continue;
                }
                if index > self.next {
                    if !flush {
                        return Ok(());
                    }
                    self.missing += index - self.next;
                    self.next = index;
                }
            } else if self.last.is_some_and(|last| index <= last) {
                // two presents in one frame period: the first one stands
                pending.remove(&index);
                self.counters.consumed.fetch_add(1, Ordering::AcqRel);
                continue;
            }
            let Some((width, height, pixels)) = pending.remove(&index).unwrap() else {
                // skipped: its time is a gap in the file
                self.missing += 1;
                self.counters.consumed.fetch_add(1, Ordering::AcqRel);
                self.next = index + 1;
                continue;
            };
            let result = self.encode(index, width, height, pixels);
            self.counters.consumed.fetch_add(1, Ordering::AcqRel);
            result?;
            self.next = index + 1;
            self.last = Some(index);
        }
    }

    fn encode(&mut self, index: u64, width: u32, height: u32, pixels: Pixels) -> Result<(), String> {
        if self.encoder.is_none() {
            self.open(width, height)?;
        }
        let rgba;
        let (data, stride, bgra, bottom_up): (&[u8], usize, bool, bool) = match &pixels {
            Pixels::Raw {
                data,
                stride,
                order,
                origin,
            } => (
                data,
                *stride,
                *order == ReadbackChannelOrder::Bgra,
                *origin == ReadbackOrigin::BottomLeft,
            ),
            Pixels::Rgba(data) => (data, width as usize * 4, false, false),
            Pixels::Png(png) => {
                rgba = decode_png(png)?;
                (&rgba, width as usize * 4, false, false)
            }
        };
        copy_centered(
            &mut self.canvas,
            self.width,
            self.height,
            data,
            width,
            height,
            stride,
            bgra,
            bottom_up,
        )?;
        let frame_hash = hash_bytes(&self.canvas);
        self.hash = (self.hash ^ frame_hash).wrapping_mul(0x0000_0100_0000_01b3);
        self.frame_hashes.push(frame_hash);
        let fps = self.config.fps as u128;
        let pts_100ns = (index as u128 * 10_000_000 / fps) as i64;
        let encoder = self.encoder.as_mut().unwrap();
        encoder
            .push_frame_rgba8(&self.canvas, Some(pts_100ns))
            .map_err(|err| format!("capture frame {index}: {err}"))?;
        self.frames += 1;
        if let (Some(rate), Some(audio)) = (self.audio_rate, &self.audio) {
            let target = ((index + 1) as u128 * rate as u128 / fps) as u64;
            if target > self.audio_pushed {
                let want = (target - self.audio_pushed) as usize;
                let mut block = Vec::with_capacity(want * 2);
                if let Ok(mut queue) = audio.lock() {
                    let have = (queue.samples.len() / 2).min(want);
                    block.extend(queue.samples.drain(..have * 2));
                }
                block.resize(want * 2, 0);
                encoder
                    .push_audio_i16(&block)
                    .map_err(|err| format!("capture audio at frame {index}: {err}"))?;
                self.audio_pushed = target;
            }
        }
        Ok(())
    }

    /// The first frame fixes the file's size (an mp4 track cannot change
    /// size); even dimensions, as 4:2:0 needs.
    fn open(&mut self, width: u32, height: u32) -> Result<(), String> {
        if self.config.path.is_empty() {
            return Err("capture has no output path".into());
        }
        self.width = (width & !1).max(2);
        self.height = (height & !1).max(2);
        self.canvas = vec![0; self.width as usize * self.height as usize * 4];
        if let Some(audio) = &self.audio {
            let until = Instant::now() + AUDIO_RATE_GRACE;
            let rate = loop {
                let rate = audio.lock().map(|queue| queue.rate).unwrap_or(0);
                if rate != 0 || Instant::now() >= until {
                    break rate;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let rate = if rate == 0 { FALLBACK_AUDIO_RATE } else { rate };
            // What the device played before the first frame is not in the file.
            if let Ok(mut queue) = audio.lock() {
                queue.samples.clear();
            }
            self.audio_rate = Some(rate);
        }
        let options = VideoFileEncoderOptions {
            codec: VideoFileCodec::H264,
            width: self.width,
            height: self.height,
            fps_num: self.config.fps,
            fps_den: 1,
            video_bitrate_bps: bitrate_for(self.width, self.height, self.config.fps),
            audio: self.audio_rate.map(|sample_rate| PcmAudioTrackOptions {
                sample_rate,
                channels: 2,
                aac_bitrate_bps: 128_000,
            }),
            keyframe_only: false,
        };
        self.encoder = Some(
            VideoFileEncoder::new(&self.config.path, options)
                .map_err(|err| format!("{}: {err}", self.config.path))?,
        );
        Ok(())
    }

    fn finish(self) -> Result<CaptureResult, String> {
        let Some(encoder) = self.encoder else {
            return Err("no frame was captured".into());
        };
        encoder
            .finish()
            .map_err(|err| format!("{}: {err}", self.config.path))?;
        Ok(CaptureResult {
            path: self.config.path,
            frames: self.frames,
            width: self.width,
            height: self.height,
            missing: self.missing + self.counters.dropped.load(Ordering::Acquire),
            hash: self.hash,
            frame_hashes: self.frame_hashes,
            audio_rate: self.audio_rate,
        })
    }
}

/// A quarter bit per pixel per frame (as ScreenCap): fine text survives.
fn bitrate_for(width: u32, height: u32, fps: u32) -> u32 {
    let bps = width as u64 * height as u64 * fps as u64 / 4;
    bps.clamp(8_000_000, 160_000_000) as u32
}

/// 64-bit FNV-1a over 8-byte words: a frame fingerprint for comparing runs.
pub(super) fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        hash ^= u64::from_le_bytes(chunk.try_into().unwrap());
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for byte in chunks.remainder() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Copy a frame onto the file's canvas, centred, as top-down RGBA; what it
/// does not cover is black. Native pixels, never scaled: a capture keeps the
/// size it started at.
#[allow(clippy::too_many_arguments)]
fn copy_centered(
    canvas: &mut [u8],
    canvas_width: u32,
    canvas_height: u32,
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    bgra: bool,
    bottom_up: bool,
) -> Result<(), String> {
    let (cw, ch, w, h) = (
        canvas_width as usize,
        canvas_height as usize,
        width as usize,
        height as usize,
    );
    if stride < w * 4 || stride.checked_mul(h).is_none_or(|len| len > src.len()) {
        return Err(format!("capture frame {w}x{h} has an invalid stride {stride}"));
    }
    let copy_w = w.min(cw);
    let copy_h = h.min(ch);
    let (dst_x, src_x) = if cw >= w { ((cw - w) / 2, 0) } else { (0, (w - cw) / 2) };
    let (dst_y, src_y) = if ch >= h { ((ch - h) / 2, 0) } else { (0, (h - ch) / 2) };
    if copy_w != cw || copy_h != ch {
        canvas.fill(0);
    }
    for row in 0..copy_h {
        let src_row = src_y + row;
        let src_row = if bottom_up { h - 1 - src_row } else { src_row };
        let src_start = src_row * stride + src_x * 4;
        let dst_start = ((dst_y + row) * cw + dst_x) * 4;
        let dst = &mut canvas[dst_start..dst_start + copy_w * 4];
        dst.copy_from_slice(&src[src_start..src_start + copy_w * 4]);
        if bgra {
            for px in dst.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
    }
    Ok(())
}

fn decode_png(png: &[u8]) -> Result<Vec<u8>, String> {
    use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
    use makepad_zune_png::makepad_zune_core::colorspace::ColorSpace;
    use makepad_zune_png::PngDecoder;
    let mut decoder = PngDecoder::new(ZCursor::new(png));
    let decoded = decoder
        .decode_raw()
        .map_err(|err| format!("capture png decode failed: {err:?}"))?;
    if decoder.colorspace() != Some(ColorSpace::RGBA) {
        return Err("capture png is not rgba".into());
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_ids_are_grab_ids_of_their_own() {
        let id = capture_id(7, 123_456);
        assert!(is_capture_id(id));
        assert!(id >= 1 << 62 && id < 1 << 63);
        assert_eq!(split_capture_id(id), (7, 123_456));
        // ordinary grab ids count up from 1 << 62
        assert!(!is_capture_id((1 << 62) + 5));
        assert!(!is_capture_id(1 << 40));
        assert!(!is_capture_id(1 << 63));
    }

    #[test]
    fn wall_frames_round_to_the_nearest_period() {
        assert_eq!(wall_frame_index(0, 60), 0);
        assert_eq!(wall_frame_index(16_666_667, 60), 1);
        assert_eq!(wall_frame_index(24_000_000, 60), 1);
        assert_eq!(wall_frame_index(1_000_000_000, 60), 60);
    }

    #[test]
    fn frames_copy_centred_with_channel_order_and_origin() {
        // 2x2 BGRA, bottom-up
        let src = [
            1, 2, 3, 4, 5, 6, 7, 8, // bottom row
            9, 10, 11, 12, 13, 14, 15, 16, // top row
        ];
        let mut canvas = vec![0xff; 2 * 2 * 4];
        copy_centered(&mut canvas, 2, 2, &src, 2, 2, 8, true, true).unwrap();
        assert_eq!(canvas, [11, 10, 9, 12, 15, 14, 13, 16, 3, 2, 1, 4, 7, 6, 5, 8]);
        // a smaller frame is centred on black
        let mut canvas = vec![0xff; 4 * 2 * 4];
        copy_centered(&mut canvas, 4, 2, &[1; 2 * 2 * 4], 2, 2, 8, false, false).unwrap();
        assert_eq!(&canvas[0..4], &[0, 0, 0, 0]);
        assert_eq!(&canvas[4..8], &[1, 1, 1, 1]);
        // a larger frame is cropped around its centre
        let mut src = vec![0; 4 * 1 * 4];
        src[4..12].copy_from_slice(&[9; 8]);
        let mut canvas = vec![0xff; 2 * 1 * 4];
        copy_centered(&mut canvas, 2, 1, &src, 4, 1, 16, false, false).unwrap();
        assert_eq!(canvas, [9; 8]);
        assert!(copy_centered(&mut canvas, 2, 1, &src, 4, 1, 8, false, false).is_err());
    }

    #[test]
    fn the_frame_hash_sees_every_byte() {
        let a = vec![0u8; 4096 + 3];
        let mut b = a.clone();
        b[4096 + 2] = 1;
        assert_ne!(hash_bytes(&a), hash_bytes(&b));
        assert_eq!(hash_bytes(&a), hash_bytes(&a.clone()));
    }

    #[test]
    fn the_virtual_worker_writes_frames_in_order_and_waits_for_gaps() {
        let mut worker = Worker {
            config: CaptureConfig {
                path: String::new(),
                fps: 60,
                audio: false,
                virtual_clock: true,
            },
            counters: Default::default(),
            audio: None,
            audio_rate: None,
            audio_pushed: 0,
            encoder: None,
            canvas: Vec::new(),
            width: 0,
            height: 0,
            next: 0,
            last: None,
            frames: 0,
            missing: 0,
            hash: 0,
            frame_hashes: Vec::new(),
        };
        // frame 1 arrives first: nothing may be written before frame 0
        let mut pending = BTreeMap::new();
        pending.insert(1, Some((2, 2, Pixels::Rgba(vec![0; 16]))));
        worker.drain(&mut pending, false).unwrap();
        assert_eq!((worker.next, pending.len()), (0, 1));
        // frame 0 abandoned by the UI (Skip): it no longer holds frame 1 up;
        // frame 1 reaches the encoder (here none is open, so it errors)
        pending.insert(0, None);
        assert!(worker.drain(&mut pending, false).is_err());
        assert_eq!(worker.missing, 1);
        assert_eq!(worker.counters.consumed.load(Ordering::Acquire), 2);
    }

    #[test]
    fn capture_paths_are_checked_before_anything_starts() {
        let dir = std::env::temp_dir().join(format!("makepad-capture-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.mp4");
        let file = file.to_str().unwrap();
        let _ = std::fs::remove_file(file);
        assert!(check_output_path(file, false).is_ok());
        assert!(!std::path::Path::new(file).exists(), "the probe leaves nothing behind");
        std::fs::write(file, b"x").unwrap();
        assert!(check_output_path(file, false).unwrap_err().contains("overwrite=1"));
        assert!(check_output_path(file, true).is_ok());
        let missing = dir.join("no/such/dir/a.mp4");
        assert!(check_output_path(missing.to_str().unwrap(), false).is_err());
        assert!(check_output_path("relative.mp4", false).is_err());
        assert!(check_output_path(dir.join("a.mov").to_str().unwrap(), false).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
