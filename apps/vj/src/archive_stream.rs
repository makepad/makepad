//! THE STREAMING SWATCH: play a remote MP4 from byte ranges, on demand.
//!
//! Nothing is downloaded. At open, the file's `moov` index (a few hundred
//! KB to a few MB) is fetched over one kept-alive HTTPS connection and
//! parsed into per-sample byte ranges; from then on a PREFETCH thread pulls
//! ~1.5 MB windows just ahead of the play position and the DECODE thread
//! feeds each H.264 access unit to the platform's hardware stream decoder,
//! reorders the frames by presentation time (B-frames come out of the
//! decoder in decode order) and paces them by the clock. A seek is one
//! range request at the nearest keyframe. Dropping the swatch stops both
//! threads and closes the connection; the memory footprint is two windows
//! and the index.
//!
//! Only H.264 in an unfragmented MP4/M4V/MOV streams this way — which is
//! every transcode the archive makes, and most originals. Anything else
//! reports a [`StreamFailure::Setup`], and the panel falls back to the
//! file-based swatch for that candidate.

use makepad_archive_org::{CancelToken, Error as ArchiveError, RangeSource};
use makepad_mp4_index::{
    locate_moov, parameter_sets_annex_b, parse_moov, sample_to_annex_b, Mp4Index, VideoCodec,
    MAX_MOOV_BYTES,
};
use makepad_widgets::makepad_platform::video_file::{
    nv12, DecodedFrame, StreamVideoCodec, VideoStreamDecoder,
};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use crate::clock::Instant;
use std::time::Duration;

/// One prefetch window. Archive H.264 transcodes run ~0.8 Mbit/s, so this
/// is ~15 s of video per request; a 1080p original is ~3 s.
const WINDOW_BYTES: usize = 1536 * 1024;
/// Windows kept ahead of the decoder (bounded channel depth).
const WINDOWS_AHEAD: usize = 3;
/// Bytes read to find `moov` before walking box headers by range.
const HEAD_PROBE_BYTES: usize = 64 * 1024;
/// Decoded frames held back for presentation-order sorting. x264's
/// default B-frame pyramid needs 2-3; four costs one more frame of
/// latency nobody sees on a swatch.
const REORDER_DEPTH: usize = 4;
/// A frame later than this is dropped rather than shown late.
const LATE_DROP: Duration = Duration::from_millis(150);

/// What `take_frame` hands back. The swatch well wants BGRA words for its
/// Image texture; a DECK wants the NV12 planes untouched — they go to the
/// GPU present pass and the CPU never unpacks them (the operator's law).
pub enum StreamFrame {
    Bgra(Vec<u32>, u32, u32),
    Nv12(Vec<u8>, u32, u32),
}

/// Which of the two a player instance emits — fixed at open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameFormat {
    Bgra,
    Nv12,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamFailure {
    /// Could not even start: no range support, not an MP4, not H.264 —
    /// the file-based swatch may still manage it.
    Setup(String),
    /// Started and then broke: network, decoder.
    Playback(String),
}

impl StreamFailure {
    pub fn message(&self) -> &str {
        match self {
            StreamFailure::Setup(m) | StreamFailure::Playback(m) => m,
        }
    }
}

struct Shared {
    stop: AtomicBool,
    paused: AtomicBool,
    /// Waiting for bytes (the well can say "buffering").
    buffering: AtomicBool,
    position_100ns: AtomicI64,
    duration_100ns: AtomicI64,
    /// Pending seek target (-1 = none).
    seek_100ns: AtomicI64,
    width: AtomicU32,
    height: AtomicU32,
    bytes_fetched: AtomicU64,
    frame: Mutex<Option<StreamFrame>>,
    failure: Mutex<Option<StreamFailure>>,
}

pub struct StreamSwatch {
    shared: Arc<Shared>,
    cancel: CancelToken,
}

impl StreamSwatch {
    pub fn open(url: String) -> StreamSwatch {
        Self::open_as(url, FrameFormat::Bgra)
    }

    pub fn open_as(url: String, format: FrameFormat) -> StreamSwatch {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            buffering: AtomicBool::new(true),
            position_100ns: AtomicI64::new(0),
            duration_100ns: AtomicI64::new(0),
            seek_100ns: AtomicI64::new(-1),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            bytes_fetched: AtomicU64::new(0),
            frame: Mutex::new(None),
            failure: Mutex::new(None),
        });
        let cancel = CancelToken::new();
        let (thread_shared, thread_cancel) = (shared.clone(), cancel.clone());
        if let Err(e) = thread::Builder::new()
            .name("vj-archive-stream".into())
            .spawn(move || {
                if let Err(failure) = stream_main(url, format, thread_shared.clone(), thread_cancel) {
                    *thread_shared.failure.lock().unwrap() = Some(failure);
                }
            })
        {
            *shared.failure.lock().unwrap() = Some(StreamFailure::Setup(e.to_string()));
        }
        StreamSwatch { shared, cancel }
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::Acquire)
    }

    pub fn is_buffering(&self) -> bool {
        self.shared.buffering.load(Ordering::Acquire)
    }

    pub fn position_secs(&self) -> f64 {
        self.shared.position_100ns.load(Ordering::Acquire) as f64 / 10_000_000.0
    }

    pub fn duration_secs(&self) -> f64 {
        self.shared.duration_100ns.load(Ordering::Acquire).max(0) as f64 / 10_000_000.0
    }

    pub fn bytes_fetched(&self) -> u64 {
        self.shared.bytes_fetched.load(Ordering::Acquire)
    }

    pub fn seek_fraction(&self, fraction: f64) {
        let duration = self.shared.duration_100ns.load(Ordering::Acquire);
        if duration <= 0 {
            return;
        }
        let target = (fraction.clamp(0.0, 1.0) * duration as f64) as i64;
        self.shared.seek_100ns.store(target.max(0), Ordering::Release);
    }

    pub fn take_frame(&self) -> Option<StreamFrame> {
        self.shared.frame.lock().ok()?.take()
    }

    pub fn failure(&self) -> Option<StreamFailure> {
        self.shared.failure.lock().ok()?.clone()
    }
}

impl Drop for StreamSwatch {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.cancel.cancel();
    }
}

// ---------------------------------------------------------------------------
// prefetch
// ---------------------------------------------------------------------------

struct Window {
    gen: u64,
    start: u64,
    bytes: Vec<u8>,
}

enum FetchCmd {
    /// Start (or restart, superseding everything queued) at `offset`.
    Start { gen: u64, offset: u64 },
}

/// The fetch thread: one window after another from the current offset,
/// into a bounded channel; a new `Start` (a seek) supersedes the cursor and
/// stamps later windows with the new generation so stale ones are skipped.
fn fetch_loop(
    mut source: RangeSource,
    cmds: Receiver<FetchCmd>,
    windows: SyncSender<Window>,
    shared: Arc<Shared>,
) {
    let size = source.size();
    let mut cursor: Option<(u64, u64)> = None; // (gen, next offset)
    let mut parked: Option<Window> = None;
    loop {
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        // Newest command wins.
        loop {
            match cmds.try_recv() {
                Ok(FetchCmd::Start { gen, offset }) => {
                    cursor = Some((gen, offset));
                    parked = None;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        // A window fetched but not yet accepted by a full channel.
        if let Some(w) = parked.take() {
            match windows.try_send(w) {
                Ok(()) => {}
                Err(TrySendError::Full(w)) => {
                    parked = Some(w);
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(TrySendError::Disconnected(_)) => return,
            }
        }
        let Some((gen, offset)) = cursor else {
            thread::sleep(Duration::from_millis(10));
            continue;
        };
        if offset >= size {
            cursor = None;
            continue;
        }
        match source.read(offset, WINDOW_BYTES) {
            Ok(bytes) if bytes.is_empty() => {
                cursor = None;
            }
            Ok(bytes) => {
                shared.bytes_fetched.fetch_add(bytes.len() as u64, Ordering::Relaxed);
                cursor = Some((gen, offset + bytes.len() as u64));
                parked = Some(Window { gen, start: offset, bytes });
            }
            Err(ArchiveError::Cancelled) => return,
            Err(e) => {
                *shared.failure.lock().unwrap() =
                    Some(StreamFailure::Playback(format!("range fetch at {offset}: {e}")));
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// decode + present
// ---------------------------------------------------------------------------

/// Contiguous bytes the decoder can read samples from, assembled from
/// consecutive windows.
struct Assembly {
    gen: u64,
    start: u64,
    bytes: Vec<u8>,
}

impl Assembly {
    fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }

    fn covers(&self, offset: u64, size: u32) -> bool {
        offset >= self.start && offset + size as u64 <= self.end()
    }

    fn slice(&self, offset: u64, size: u32) -> &[u8] {
        let a = (offset - self.start) as usize;
        &self.bytes[a..a + size as usize]
    }

    /// Drop everything before `offset` once the buffer is large, so a
    /// long play never grows it past a few windows.
    fn trim_before(&mut self, offset: u64) {
        if self.bytes.len() > 2 * WINDOW_BYTES && offset > self.start {
            let cut = ((offset - self.start) as usize).min(self.bytes.len());
            self.bytes.drain(..cut);
            self.start += cut as u64;
        }
    }
}

fn stream_main(
    url: String,
    format: FrameFormat,
    shared: Arc<Shared>,
    cancel: CancelToken,
) -> Result<(), StreamFailure> {
    // ---- index
    let mut source = RangeSource::open(&url, &cancel)
        .map_err(|e| StreamFailure::Setup(format!("range open: {e}")))?;
    let size = source.size();
    let head = source
        .read(0, HEAD_PROBE_BYTES)
        .map_err(|e| StreamFailure::Setup(format!("head read: {e}")))?;
    let moov = {
        let head_ref = &head;
        locate_moov(size, &mut |offset, len| {
            let (o, l) = (offset as usize, len);
            if o + l <= head_ref.len() {
                return Ok(head_ref[o..o + l].to_vec());
            }
            source.read(offset, len).map_err(|e| e.to_string())
        })
        .map_err(|e| StreamFailure::Setup(e.to_string()))?
    };
    let moov_size = moov.size.unwrap_or(size.saturating_sub(moov.offset));
    if moov_size > MAX_MOOV_BYTES || moov_size < moov.header_len {
        return Err(StreamFailure::Setup("mp4 index over the size limit".into()));
    }
    let payload_at = moov.offset + moov.header_len;
    let payload_len = (moov_size - moov.header_len) as usize;
    let moov_bytes = if (payload_at as usize).saturating_add(payload_len) <= head.len() {
        head[payload_at as usize..payload_at as usize + payload_len].to_vec()
    } else {
        source
            .read(payload_at, payload_len)
            .map_err(|e| StreamFailure::Setup(format!("index read: {e}")))?
    };
    let index: Mp4Index = parse_moov(&moov_bytes).map_err(|e| StreamFailure::Setup(e.to_string()))?;
    let nal_length_size = match &index.codec {
        VideoCodec::H264 { nal_length_size, .. } => *nal_length_size,
        VideoCodec::Other(fourcc) => {
            return Err(StreamFailure::Setup(format!("video codec {fourcc} is not streamable")));
        }
    };
    if index.samples.is_empty() {
        return Err(StreamFailure::Setup("mp4 has no video samples".into()));
    }
    shared.width.store(index.width, Ordering::Release);
    shared.height.store(index.height, Ordering::Release);
    shared.duration_100ns.store(index.duration_100ns, Ordering::Release);
    let params = parameter_sets_annex_b(&index.codec);

    // ---- threads
    let (cmd_tx, cmd_rx) = mpsc::channel::<FetchCmd>();
    let (win_tx, win_rx) = mpsc::sync_channel::<Window>(WINDOWS_AHEAD);
    let fetch_shared = shared.clone();
    thread::Builder::new()
        .name("vj-archive-fetch".into())
        .spawn(move || fetch_loop(source, cmd_rx, win_tx, fetch_shared))
        .map_err(|e| StreamFailure::Setup(e.to_string()))?;

    let mut decoder = VideoStreamDecoder::new(StreamVideoCodec::H264)
        .map_err(|e| StreamFailure::Setup(format!("stream decoder: {e}")))?;

    let mut gen: u64 = 1;
    let mut cursor: usize = 0;
    let _ = cmd_tx.send(FetchCmd::Start { gen, offset: index.samples[0].offset });
    let mut assembly: Option<Assembly> = None;
    let mut reorder: Vec<DecodedFrame> = Vec::new();
    let mut need_params = true;
    let mut origin = Instant::now();
    let mut base_100ns: i64 = -1;
    let mut show_one = true;
    let mut annex_b: Vec<u8> = Vec::new();
    let mut bgra: Vec<u32> = Vec::new();
    let mut at_end = false;
    let mut decode_errors = 0u32;

    loop {
        if shared.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Some(f) = shared.failure.lock().unwrap().clone() {
            return Err(f);
        }
        // ---- pause: hold the picture and the clock. A player OPENED
        // paused still decodes its first frame — a swatch that starts
        // paused must show a poster, not a black well — so the park only
        // engages once one frame is up (`show_one` false).
        while shared.paused.load(Ordering::Acquire)
            && !show_one
            && shared.seek_100ns.load(Ordering::Acquire) < 0
        {
            if shared.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            // Parked is parked, not buffering — the flag must not stick
            // from the fetch wait that ran just before the pause.
            shared.buffering.store(false, Ordering::Release);
            thread::sleep(Duration::from_millis(20));
            origin += Duration::from_millis(20);
        }
        // ---- seek
        let seek = shared.seek_100ns.swap(-1, Ordering::AcqRel);
        if seek >= 0 {
            cursor = index.sync_sample_before(seek);
            gen += 1;
            let _ = cmd_tx.send(FetchCmd::Start { gen, offset: index.samples[cursor].offset });
            assembly = None;
            reorder.clear();
            let _ = decoder.flush();
            need_params = true;
            base_100ns = -1;
            show_one = true;
            at_end = false;
            shared.position_100ns.store(index.samples[cursor].pts_100ns, Ordering::Release);
        }
        // ---- end of stream: drain, then loop
        if cursor >= index.samples.len() {
            if !at_end {
                if let Ok(frames) = decoder.flush() {
                    reorder.extend(frames);
                }
                at_end = true;
            }
            if reorder.is_empty() {
                shared.seek_100ns.store(0, Ordering::Release);
                continue;
            }
            present(&mut reorder, 0, format, &shared, &mut origin, &mut base_100ns, &mut show_one, &mut bgra);
            continue;
        }
        // ---- bytes for the next sample
        let sample = index.samples[cursor];
        let ready = assembly
            .as_ref()
            .map(|a| a.gen == gen && a.covers(sample.offset, sample.size))
            .unwrap_or(false);
        if !ready {
            shared.buffering.store(true, Ordering::Release);
            match win_rx.try_recv() {
                Ok(w) if w.gen != gen => {}
                Ok(w) => {
                    match assembly.as_mut() {
                        Some(a) if a.gen == gen && a.end() == w.start => a.bytes.extend_from_slice(&w.bytes),
                        _ => assembly = Some(Assembly { gen, start: w.start, bytes: w.bytes }),
                    }
                }
                Err(TryRecvError::Empty) => {
                    // Present what is already decoded while the bytes come.
                    if !reorder.is_empty() && reorder.len() > REORDER_DEPTH {
                        present(&mut reorder, REORDER_DEPTH, format, &shared, &mut origin, &mut base_100ns, &mut show_one, &mut bgra);
                    } else {
                        thread::sleep(Duration::from_millis(4));
                        origin += Duration::from_millis(4);
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    return Err(StreamFailure::Playback("fetch thread ended".into()));
                }
            }
            continue;
        }
        shared.buffering.store(false, Ordering::Release);
        let a = assembly.as_mut().unwrap();
        // ---- decode one access unit
        annex_b.clear();
        if need_params {
            annex_b.extend_from_slice(&params);
            need_params = false;
        }
        if let Err(e) = sample_to_annex_b(a.slice(sample.offset, sample.size), nal_length_size, &mut annex_b) {
            return Err(StreamFailure::Playback(format!("sample {cursor}: {e}")));
        }
        match decoder.push_packet(&annex_b, sample.pts_100ns) {
            Ok(frames) => {
                decode_errors = 0;
                reorder.extend(frames);
            }
            Err(e) => {
                // A damaged access unit in an old tape is skipped; a
                // decoder that fails from the first keyframe, or keeps
                // failing, ends the stream (the panel moves on).
                decode_errors += 1;
                if cursor == 0 || decode_errors > 30 {
                    return Err(StreamFailure::Playback(format!("decode: {e}")));
                }
            }
        }
        cursor += 1;
        a.trim_before(sample.offset);
        // ---- present in pts order, paced
        if reorder.len() > REORDER_DEPTH {
            present(&mut reorder, REORDER_DEPTH, format, &shared, &mut origin, &mut base_100ns, &mut show_one, &mut bgra);
        }
    }
}

/// Emit the earliest frame in `reorder` while more than `keep` are held:
/// wait for its due time (or drop it when hopelessly late), convert, and
/// hand it to the UI. A paused swatch still shows one frame after a seek.
#[allow(clippy::too_many_arguments)]
fn present(
    reorder: &mut Vec<DecodedFrame>,
    keep: usize,
    format: FrameFormat,
    shared: &Arc<Shared>,
    origin: &mut Instant,
    base_100ns: &mut i64,
    show_one: &mut bool,
    bgra: &mut Vec<u32>,
) {
    while reorder.len() > keep {
        let (i, _) = reorder
            .iter()
            .enumerate()
            .min_by_key(|(_, f)| f.pts_100ns)
            .expect("non-empty");
        let frame = reorder.swap_remove(i);
        if *base_100ns < 0 {
            *base_100ns = frame.pts_100ns;
            *origin = Instant::now();
        }
        let due = Duration::from_nanos(((frame.pts_100ns - *base_100ns).max(0) as u64) * 100);
        let paused = shared.paused.load(Ordering::Acquire);
        if !paused || *show_one {
            if !*show_one && origin.elapsed() > due + LATE_DROP {
                continue;
            }
            if !*show_one {
                loop {
                    let remaining = due.saturating_sub(origin.elapsed());
                    if remaining.is_zero() || shared.stop.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(remaining.min(Duration::from_millis(4)));
                }
            }
            shared.position_100ns.store(frame.pts_100ns, Ordering::Release);
            let out = match format {
                FrameFormat::Bgra => {
                    bgra.clear();
                    nv12::nv12_to_bgra_u32(&frame.nv12, frame.width, frame.height, bgra);
                    StreamFrame::Bgra(std::mem::take(bgra), frame.width, frame.height)
                }
                FrameFormat::Nv12 => StreamFrame::Nv12(frame.nv12, frame.width, frame.height),
            };
            if let Ok(mut slot) = shared.frame.lock() {
                *slot = Some(out);
            }
            *show_one = false;
            return;
        }
        // Paused and already showing a frame: keep the frame for later.
        reorder.push(frame);
        return;
    }
}
