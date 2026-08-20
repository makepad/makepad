//! Per-deck source separation, off the UI and audio threads.
//!
//! A deck plays the mixed file until separated stems exist for the stretch
//! of track under its playhead; from then on the four stem knobs are live.
//! That is the whole contract, and it is what makes separation usable in a
//! performance: nothing waits for a whole track to be demixed.
//!
//! Three sources, in order:
//!
//! 1. **Sidecar stems** — a `stems/` directory beside the track holding
//!    `drums.wav`, `bass.wav`, `other.wav`, `vocals.wav` on the same
//!    timeline. Free, instant, and how a pre-separated library works.
//! 2. **The span cache** — `makepad_ai_stems::StemCache`, keyed by the
//!    decoded track's digest and granular to the demixer's own 5.5-second
//!    span. Separation costs about a third of a track's duration, so a
//!    track pays it ONCE: every span is written through the moment it is
//!    finished, not at the end of the run, and the next load of that track
//!    serves whatever is already there at disk speed. A track that is
//!    covered end to end never loads the model at all.
//! 3. **The demixer** — `makepad_ai_stems::Demixer` streamed from the first
//!    span the cache cannot serve, one finished span per model forward.
//!
//! The cache is four stems of stereo i16 at the model's rate, so it costs
//! about 700 KB per second of track — a quarter of a gigabyte for a
//! six-minute one. That is the trade the engine's cache makes deliberately
//! (i16 is what the mixer consumes and it is a quarter of f32); nothing here
//! prunes it, so `local/vj/stem-cache` is a directory to keep an eye on.
//!
//! Either way the worker publishes fixed one-second chunks in TRACK frames,
//! so the mixer can index them arithmetically and a chunk that has not
//! arrived simply falls back to the mixed file.

use crate::decks::DeckId;
use crate::mixer::TrackPcm;
use makepad_ai_stems::{
    CacheHeader, Demixer, StemCache, StemSet, StemsModel, StereoBuf, CHUNK_STEP,
    SAMPLE_RATE as STEMS_RATE,
};
use makepad_asset_data::Sha256;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;

/// Published chunk length, seconds of track time. Small enough that the
/// knobs go live seconds after a load, big enough that the chunk table
/// stays short for a long track.
pub const STEM_CHUNK_SECS: f64 = 1.0;

/// Stem lanes in the order the deck engine and the UI use:
/// vocals, drums, bass, other (the model's "other").
pub const STEM_ORDER: [makepad_ai_stems::Stem; 4] = [
    makepad_ai_stems::Stem::Vocals,
    makepad_ai_stems::Stem::Drums,
    makepad_ai_stems::Stem::Bass,
    makepad_ai_stems::Stem::Other,
];

/// Sidecar file names, in `STEM_ORDER`.
const SIDECAR_NAMES: [&str; 4] = ["vocals.wav", "drums.wav", "bass.wav", "other.wav"];

/// Where the checkpoint lives. `VJ_STEMS_CKPT` overrides; otherwise the
/// checkout's reference copy, which is what this machine has.
pub fn checkpoint_path() -> PathBuf {
    if let Ok(path) = std::env::var("VJ_STEMS_CKPT") {
        return PathBuf::from(path);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../local/stems_ref/ckpt")
        .join(makepad_ai_stems::MODEL_CHECKPOINT)
}

/// Where separated spans are kept between sessions. `VJ_STEMS_CACHE`
/// overrides; otherwise beside the VJ's other local state.
pub fn cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("VJ_STEMS_CACHE") {
        return PathBuf::from(dir);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/vj/stem-cache")
}

/// Cache key for a track: the digest of the DECODED audio, so the same track
/// reached through the store and through a local file shares one entry and a
/// re-encode of it does not. Hashed incrementally — the PCM of a long track
/// is tens of megabytes and there is no reason to copy it to hash it.
pub fn track_digest(pcm: &TrackPcm) -> String {
    let mut hasher = Sha256::new();
    hasher.update(&pcm.sample_rate.to_le_bytes());
    hasher.update(&(pcm.frames.len() as u64).to_le_bytes());
    let mut block: Vec<u8> = Vec::with_capacity(64 * 1024);
    for frame in &pcm.frames {
        block.extend_from_slice(&frame[0].to_le_bytes());
        block.extend_from_slice(&frame[1].to_le_bytes());
        if block.len() >= 64 * 1024 {
            hasher.update(&block);
            block.clear();
        }
    }
    hasher.update(&block);
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Provenance for anything that surfaces where the separation came from.
pub fn model_provenance() -> String {
    format!(
        "{} · {} · {}",
        makepad_ai_stems::MODEL_ID,
        makepad_ai_stems::MODEL_CHECKPOINT,
        makepad_ai_stems::MODEL_LICENSE
    )
}

// ---------------------------------------------------------------------------
// jobs and results
// ---------------------------------------------------------------------------

pub struct StemsJob {
    pub deck: DeckId,
    pub gen: u64,
    pub pcm: Arc<TrackPcm>,
    /// The file on disk, when the track came from one: sidecar stems are
    /// looked for beside it.
    pub source: Option<PathBuf>,
    /// Where the playhead is, so the demixer starts where it is needed.
    pub start_secs: f64,
}

/// One published chunk of separated audio, in track frames.
pub struct StemChunk {
    pub deck: DeckId,
    pub gen: u64,
    /// Chunk index; frame `i` of the track is in chunk `i / chunk_frames`.
    pub index: usize,
    pub chunk_frames: usize,
    /// Total chunks in the track.
    pub chunk_count: usize,
    /// One buffer per lane in [`STEM_ORDER`].
    pub lanes: [Arc<Vec<[i16; 2]>>; 4],
}

pub enum StemsMsg {
    /// Progress the deck can show: "separating…", "loaded stems", an error.
    Status { deck: DeckId, gen: u64, text: String, working: bool },
    Chunk(Box<StemChunk>),
    Done { deck: DeckId, gen: u64 },
}

// ---------------------------------------------------------------------------
// resampling
// ---------------------------------------------------------------------------

/// Polyphase sinc phases. The kernel is built once per rate change and
/// indexed by the fractional position, so resampling costs multiply-adds
/// and no transcendentals — the naive form (a sinc and a window evaluated
/// per tap per output sample) was measurably the app's biggest CPU
/// consumer while a track was being separated.
const RESAMPLE_PHASES: usize = 256;
/// Sinc taps either side of the centre, before the cutoff widening.
const RESAMPLE_TAPS: usize = 16;

/// A windowed-sinc kernel, precomputed for one rate ratio.
struct ResampleKernel {
    /// `[phase][tap]`, each phase row normalized to unity gain.
    taps: Vec<f32>,
    width: usize,
    span: usize,
}

impl ResampleKernel {
    fn new(ratio: f64) -> ResampleKernel {
        // Anti-alias when downsampling: the cutoff follows the lower rate.
        let cutoff = 0.5 * ratio.min(1.0);
        let width = ((RESAMPLE_TAPS as f64) / (2.0 * cutoff)).ceil() as usize;
        let span = 2 * width + 1;
        let mut taps = vec![0.0f32; RESAMPLE_PHASES * span];
        for phase in 0..RESAMPLE_PHASES {
            let frac = phase as f64 / RESAMPLE_PHASES as f64;
            let row = phase * span;
            let mut sum = 0.0f64;
            for tap in 0..span {
                // Distance from the output position to this input sample.
                let x = frac - (tap as f64 - width as f64);
                let arg = 2.0 * cutoff * x;
                let sinc = if arg.abs() < 1e-9 {
                    1.0
                } else {
                    (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg)
                };
                let t = (x / width as f64).clamp(-1.0, 1.0);
                let angle = std::f64::consts::PI * (t + 1.0);
                let window = 0.42 - 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos();
                let weight = sinc * window;
                taps[row + tap] = weight as f32;
                sum += weight;
            }
            if sum.abs() > 1e-12 {
                let inverse = (1.0 / sum) as f32;
                for tap in 0..span {
                    taps[row + tap] *= inverse;
                }
            }
        }
        ResampleKernel { taps, width, span }
    }

    fn apply(&self, input: &[f32], ratio: f64) -> Vec<f32> {
        let out_len = ((input.len() as f64) * ratio).round() as usize;
        let mut out = Vec::with_capacity(out_len);
        let inverse_ratio = 1.0 / ratio;
        for index in 0..out_len {
            let center = index as f64 * inverse_ratio;
            let base = center.floor() as isize;
            let frac = center - base as f64;
            let phase = ((frac * RESAMPLE_PHASES as f64) as usize).min(RESAMPLE_PHASES - 1);
            let row = phase * self.span;
            let mut sum = 0.0f32;
            for tap in 0..self.span {
                let sample_index = base + tap as isize - self.width as isize;
                if sample_index < 0 {
                    continue;
                }
                let Some(sample) = input.get(sample_index as usize) else { break };
                sum += sample * self.taps[row + tap];
            }
            out.push(sum);
        }
        out
    }
}

/// Resample, used only when a track is not already at the model's 44.1 kHz.
/// Runs on the worker; each span is transformed exactly once and the result
/// is what playback reads.
fn resample(input: &[f32], from_rate: f64, to_rate: f64) -> Vec<f32> {
    if (from_rate - to_rate).abs() < 0.5 || input.is_empty() {
        return input.to_vec();
    }
    let ratio = to_rate / from_rate;
    ResampleKernel::new(ratio).apply(input, ratio)
}

/// The same, reusing a kernel across the lanes of one span.
fn resample_with(kernel: &ResampleKernel, input: &[f32], ratio: f64) -> Vec<f32> {
    kernel.apply(input, ratio)
}

fn to_stereo_buf(pcm: &TrackPcm) -> StereoBuf {
    let mut left = Vec::with_capacity(pcm.frames.len());
    let mut right = Vec::with_capacity(pcm.frames.len());
    for frame in &pcm.frames {
        left.push(frame[0] as f32 / 32768.0);
        right.push(frame[1] as f32 / 32768.0);
    }
    let rate = pcm.sample_rate.max(1) as f64;
    let target = STEMS_RATE as f64;
    if (rate - target).abs() < 0.5 {
        return StereoBuf { left, right };
    }
    StereoBuf {
        left: resample(&left, rate, target),
        right: resample(&right, rate, target),
    }
}

/// Chunk length in track frames.
pub fn chunk_frames(sample_rate: u32) -> usize {
    ((sample_rate.max(1) as f64) * STEM_CHUNK_SECS).round().max(1.0) as usize
}

/// How many chunks a track of `frames` covers.
pub fn chunk_count(frames: usize, sample_rate: u32) -> usize {
    frames.div_ceil(chunk_frames(sample_rate))
}

// ---------------------------------------------------------------------------
// the worker
// ---------------------------------------------------------------------------

/// Accumulates resampled stem audio and emits whole track-frame chunks.
struct ChunkWriter {
    chunk_frames: usize,
    chunk_count: usize,
    /// Track frame the pending buffer starts at.
    base: usize,
    /// Frames still to be thrown away before `base` is reached.
    skip: usize,
    /// False until the first `restart`: the run has no anchor yet.
    started: bool,
    /// Per lane, per channel-interleaved frames pending publication.
    pending: [Vec<[i16; 2]>; 4],
}

impl ChunkWriter {
    fn new(chunk_frames: usize, chunk_count: usize) -> ChunkWriter {
        ChunkWriter {
            chunk_frames,
            chunk_count,
            base: 0,
            skip: 0,
            started: false,
            pending: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
        }
    }

    /// Start a fresh run at `frame`, dropping anything half-collected.
    fn restart(&mut self, frame: usize) {
        // Chunks are published whole and they are published WHERE THEY ARE:
        // a demixer span boundary is 5.5 seconds and a chunk is one, so a run
        // that starts at the playhead usually starts mid-chunk. Rounding the
        // base down would label that audio as the chunk before it and put
        // every stem in the rest of the track half a second ahead of the
        // mix. Round up instead and drop the part-chunk we cannot fill.
        self.base = frame.div_ceil(self.chunk_frames) * self.chunk_frames;
        self.skip = self.base - frame;
        self.started = true;
        for lane in self.pending.iter_mut() {
            lane.clear();
        }
    }

    /// True once a run has been anchored, so the walk over spans only sets
    /// the anchor from the span it actually starts on.
    fn started(&self) -> bool {
        self.started
    }

    /// Add one lane-aligned block; every lane must be pushed the same length.
    fn push(&mut self, lanes: [Vec<[i16; 2]>; 4]) {
        let drop = self.skip.min(lanes[0].len());
        for (slot, block) in self.pending.iter_mut().zip(lanes) {
            slot.extend_from_slice(&block[drop.min(block.len())..]);
        }
        self.skip -= drop;
    }

    /// Publish every whole chunk the pending buffers now cover.
    fn drain(&mut self, deck: DeckId, gen: u64, out: &Sender<StemsMsg>) -> bool {
        let ready = self.pending[0].len() / self.chunk_frames;
        for _ in 0..ready {
            let index = self.base / self.chunk_frames;
            if index >= self.chunk_count {
                return true;
            }
            let mut lanes: Vec<Arc<Vec<[i16; 2]>>> = Vec::with_capacity(4);
            for lane in self.pending.iter_mut() {
                let rest = lane.split_off(self.chunk_frames);
                lanes.push(Arc::new(std::mem::replace(lane, rest)));
            }
            let lanes: [Arc<Vec<[i16; 2]>>; 4] = [
                lanes[0].clone(),
                lanes[1].clone(),
                lanes[2].clone(),
                lanes[3].clone(),
            ];
            self.base += self.chunk_frames;
            if out
                .send(StemsMsg::Chunk(Box::new(StemChunk {
                    deck,
                    gen,
                    index,
                    chunk_frames: self.chunk_frames,
                    chunk_count: self.chunk_count,
                    lanes,
                })))
                .is_err()
            {
                return true;
            }
        }
        false
    }
}

/// Read the four sidecar stems beside a track, if they are all there.
fn load_sidecar(source: &Path) -> Option<[TrackPcm; 4]> {
    let dir = source.parent()?.join("stems");
    if !dir.is_dir() {
        return None;
    }
    let mut lanes = Vec::with_capacity(4);
    for name in SIDECAR_NAMES {
        let path = dir.join(name);
        let pcm = crate::media::decode_audio_clip(
            &path,
            makepad_asset_data::MediaType::Wav,
            crate::wave_analysis::MAX_LOCAL_TRACK_FRAMES,
        )
        .ok()?;
        lanes.push(pcm);
    }
    let mut out = lanes.into_iter();
    Some([out.next()?, out.next()?, out.next()?, out.next()?])
}

fn i16_frames(
    buf: &StereoBuf,
    from: usize,
    len: usize,
    rate: f64,
    target: f64,
    kernel: Option<&ResampleKernel>,
) -> Vec<[i16; 2]> {
    let end = (from + len).min(buf.frames());
    if from >= end {
        return Vec::new();
    }
    let left = &buf.left[from..end];
    let right = &buf.right[from..end];
    let (left, right) = match kernel {
        None => (left.to_vec(), right.to_vec()),
        Some(kernel) => {
            let ratio = target / rate;
            (
                resample_with(kernel, left, ratio),
                resample_with(kernel, right, ratio),
            )
        }
    };
    left.iter()
        .zip(right.iter())
        .map(|(l, r)| {
            [
                (l.clamp(-1.0, 1.0) * 32767.0) as i16,
                (r.clamp(-1.0, 1.0) * 32767.0) as i16,
            ]
        })
        .collect()
}

fn run_sidecar(job: &StemsJob, lanes: [TrackPcm; 4], out: &Sender<StemsMsg>) {
    let frames = job.pcm.frames.len();
    let rate = job.pcm.sample_rate.max(1);
    let size = chunk_frames(rate);
    let count = chunk_count(frames, rate);
    let _ = out.send(StemsMsg::Status {
        deck: job.deck,
        gen: job.gen,
        text: "stems: sidecar".to_string(),
        working: true,
    });
    for index in 0..count {
        let start = index * size;
        let end = (start + size).min(frames);
        if start >= end {
            break;
        }
        let mut blocks: Vec<Arc<Vec<[i16; 2]>>> = Vec::with_capacity(4);
        for lane in lanes.iter() {
            let slice: Vec<[i16; 2]> = (start..end)
                .map(|frame| lane.frames.get(frame).copied().unwrap_or([0, 0]))
                .collect();
            blocks.push(Arc::new(slice));
        }
        if out
            .send(StemsMsg::Chunk(Box::new(StemChunk {
                deck: job.deck,
                gen: job.gen,
                index,
                chunk_frames: size,
                chunk_count: count,
                lanes: [
                    blocks[0].clone(),
                    blocks[1].clone(),
                    blocks[2].clone(),
                    blocks[3].clone(),
                ],
            })))
            .is_err()
        {
            return;
        }
    }
    let _ = out.send(StemsMsg::Done { deck: job.deck, gen: job.gen });
}

/// Frames the model works in for this track — the length `to_stereo_buf`
/// will produce. The cache is addressed in exactly these coordinates, so the
/// two must agree to the sample or the entry looks like a different track.
fn model_frames(pcm: &TrackPcm) -> usize {
    let rate = pcm.sample_rate.max(1) as f64;
    let target = STEMS_RATE as f64;
    if (rate - target).abs() < 0.5 {
        pcm.frames.len()
    } else {
        ((pcm.frames.len() as f64) * (target / rate)).round() as usize
    }
}

/// The cache entry for a track, or `None` when there is nowhere to keep one.
/// A cache that will not open is never a reason to refuse to separate.
fn open_cache(pcm: &TrackPcm) -> Option<StemCache> {
    let frames = model_frames(pcm);
    if frames == 0 {
        return None;
    }
    let digest = track_digest(pcm);
    StemCache::open(cache_dir(), &digest, CacheHeader::for_track(frames as u64)).ok()
}

/// Everything one span needs to reach the deck: resample to the track's rate
/// and hand the lanes to the writer in the deck's order.
fn publish_span(
    stems: &StemSet,
    writer: &mut ChunkWriter,
    model_rate: f64,
    track_rate: f64,
    kernel: Option<&ResampleKernel>,
) {
    let mut lanes: Vec<Vec<[i16; 2]>> = Vec::with_capacity(4);
    for stem in STEM_ORDER {
        let buf = &stems[stem as usize];
        lanes.push(i16_frames(buf, 0, buf.frames(), model_rate, track_rate, kernel));
    }
    let shortest = lanes.iter().map(Vec::len).min().unwrap_or(0);
    for lane in lanes.iter_mut() {
        lane.truncate(shortest);
    }
    writer.push([
        std::mem::take(&mut lanes[0]),
        std::mem::take(&mut lanes[1]),
        std::mem::take(&mut lanes[2]),
        std::mem::take(&mut lanes[3]),
    ]);
}

/// Track frame a model-domain span starts at.
fn span_track_start(span: usize, track_rate: f64, model_rate: f64) -> usize {
    (((span * CHUNK_STEP) as f64) * track_rate / model_rate).round() as usize
}

/// Publish every span the cache already holds, walking forward from the
/// playhead. Returns the first span it could NOT serve — `None` means the
/// track is covered from here to the end and the model is not needed.
///
/// This is what makes the knobs live instantly on a track that has been on a
/// deck before: it is four file reads and a resample per span, no model.
fn run_cached(
    job: &StemsJob,
    cache: &mut StemCache,
    writer: &mut ChunkWriter,
    first_span: usize,
    out: &Sender<StemsMsg>,
) -> Option<usize> {
    let track_rate = job.pcm.sample_rate.max(1) as f64;
    let model_rate = STEMS_RATE as f64;
    let kernel = span_resample_kernel(track_rate, model_rate);
    let mut announced = false;
    for span in first_span..cache.span_count() {
        if !cache.has_span(span) {
            return Some(span);
        }
        let Ok(Some(stems)) = cache.read_span(span) else {
            return Some(span);
        };
        if !announced {
            let _ = out.send(StemsMsg::Status {
                deck: job.deck,
                gen: job.gen,
                text: "stems: cached".to_string(),
                working: true,
            });
            announced = true;
        }
        if !writer.started() {
            writer.restart(span_track_start(span, track_rate, model_rate));
        }
        publish_span(&stems, writer, model_rate, track_rate, kernel.as_ref());
        if writer.drain(job.deck, job.gen, out) {
            return None;
        }
    }
    None
}

/// One kernel for a whole run, not one per lane per span.
fn span_resample_kernel(track_rate: f64, model_rate: f64) -> Option<ResampleKernel> {
    if (track_rate - model_rate).abs() < 0.5 {
        None
    } else {
        Some(ResampleKernel::new(track_rate / model_rate))
    }
}

fn run_demixer(
    job: &StemsJob,
    model: &mut StemsModel,
    cache: Option<&mut StemCache>,
    writer: &mut ChunkWriter,
    resume: usize,
    out: &Sender<StemsMsg>,
) -> Result<(), String> {
    let track = to_stereo_buf(&job.pcm);
    let track_rate = job.pcm.sample_rate.max(1) as f64;
    let model_rate = STEMS_RATE as f64;
    let span_kernel = span_resample_kernel(track_rate, model_rate);
    let mut demixer = Demixer::new(model, &track).map_err(|e| e.to_string())?;
    let mut cache = cache;
    let span_count = demixer.span_count();
    let _ = out.send(StemsMsg::Status {
        deck: job.deck,
        gen: job.gen,
        text: "stems: separating…".to_string(),
        working: true,
    });

    // A fresh `Demixer` is already positioned on span zero; seeking costs a
    // discarded model forward, so only do it when the cursor is elsewhere.
    let mut cursor = 0usize;
    for span in resume..span_count {
        let cached = cache
            .as_mut()
            .filter(|cache| cache.has_span(span))
            .and_then(|cache| cache.read_span(span).ok().flatten());
        let (track_start, stems) = match cached {
            Some(stems) => (span_track_start(span, track_rate, model_rate), stems),
            None => {
                if cursor != span {
                    demixer.seek(span * CHUNK_STEP);
                }
                let Some(finished) = demixer.next_span().map_err(|e| e.to_string())? else {
                    break;
                };
                cursor = span + 1;
                // Written through the moment it exists, so a run that is cut
                // short — a new track on the deck, the app quitting — still
                // leaves every span it finished behind for the next load.
                if let Some(cache) = cache.as_mut() {
                    let _ = cache.write_span(finished.start, &finished.stems);
                }
                (finished.start, finished.stems)
            }
        };
        if !writer.started() {
            writer.restart(track_start);
        }
        publish_span(&stems, writer, model_rate, track_rate, span_kernel.as_ref());
        if writer.drain(job.deck, job.gen, out) {
            return Ok(());
        }
    }
    let _ = out.send(StemsMsg::Done { deck: job.deck, gen: job.gen });
    Ok(())
}

/// One separation thread. The model is thread-affine and expensive to load,
/// so it lives here and nowhere else.
pub struct StemsPool {
    tx: Sender<StemsJob>,
    rx: Receiver<StemsMsg>,
}

impl Default for StemsPool {
    fn default() -> Self {
        StemsPool::new()
    }
}

impl StemsPool {
    pub fn new() -> StemsPool {
        let (tx, jobs) = channel::<StemsJob>();
        let (out, rx) = channel::<StemsMsg>();
        let _ = std::thread::Builder::new()
            .name("vj-stems".into())
            .spawn(move || {
                let mut model: Option<StemsModel> = None;
                let mut model_failed = false;
                while let Ok(job) = jobs.recv() {
                    // Latest-wins: only the newest request per deck matters.
                    let mut job = job;
                    while let Ok(newer) = jobs.try_recv() {
                        job = newer;
                    }
                    if let Some(source) = job.source.as_ref() {
                        if let Some(lanes) = load_sidecar(source) {
                            run_sidecar(&job, lanes, &out);
                            continue;
                        }
                    }
                    // Whatever a previous session separated is free. Serve it
                    // first, from the playhead forward: the knobs go live on
                    // a track that has been on a deck before without the
                    // model being loaded at all.
                    let mut cache = open_cache(&job.pcm);
                    let track_rate = job.pcm.sample_rate.max(1);
                    let mut writer = ChunkWriter::new(
                        chunk_frames(track_rate),
                        chunk_count(job.pcm.frames.len(), track_rate),
                    );
                    // A playhead past the end must still separate the last
                    // span, not silently declare the track done.
                    let spans = model_frames(&job.pcm).div_ceil(CHUNK_STEP);
                    let first_span = ((job.start_secs.max(0.0) * STEMS_RATE as f64) as usize
                        / CHUNK_STEP)
                        .min(spans.saturating_sub(1));
                    let resume = match cache.as_mut() {
                        Some(cache) => run_cached(&job, cache, &mut writer, first_span, &out),
                        None => Some(first_span),
                    };
                    let Some(resume) = resume else {
                        let _ = out.send(StemsMsg::Done { deck: job.deck, gen: job.gen });
                        continue;
                    };
                    if model_failed {
                        let _ = out.send(StemsMsg::Status {
                            deck: job.deck,
                            gen: job.gen,
                            text: "stems: model unavailable".to_string(),
                            working: false,
                        });
                        continue;
                    }
                    if model.is_none() {
                        let path = checkpoint_path();
                        if !path.is_file() {
                            model_failed = true;
                            let _ = out.send(StemsMsg::Status {
                                deck: job.deck,
                                gen: job.gen,
                                text: "stems: model not installed".to_string(),
                                working: false,
                            });
                            continue;
                        }
                        let _ = out.send(StemsMsg::Status {
                            deck: job.deck,
                            gen: job.gen,
                            text: "stems: loading model…".to_string(),
                            working: true,
                        });
                        match StemsModel::load(&path) {
                            Ok(loaded) => model = Some(loaded),
                            Err(error) => {
                                model_failed = true;
                                let _ = out.send(StemsMsg::Status {
                                    deck: job.deck,
                                    gen: job.gen,
                                    text: format!("stems: {error}"),
                                    working: false,
                                });
                                continue;
                            }
                        }
                    }
                    let Some(loaded) = model.as_mut() else { continue };
                    if let Err(error) =
                        run_demixer(&job, loaded, cache.as_mut(), &mut writer, resume, &out)
                    {
                        let _ = out.send(StemsMsg::Status {
                            deck: job.deck,
                            gen: job.gen,
                            text: format!("stems: {error}"),
                            working: false,
                        });
                    }
                }
            });
        StemsPool { tx, rx }
    }

    pub fn submit(&self, job: StemsJob) {
        let _ = self.tx.send(job);
    }

    pub fn poll(&self) -> Vec<StemsMsg> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(message) => out.push(message),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_geometry_covers_the_whole_track() {
        assert_eq!(chunk_frames(44_100), 44_100);
        assert_eq!(chunk_frames(48_000), 48_000);
        // A partial last second still gets a chunk.
        assert_eq!(chunk_count(44_100 * 3, 44_100), 3);
        assert_eq!(chunk_count(44_100 * 3 + 1, 44_100), 4);
        assert_eq!(chunk_count(0, 44_100), 0);
    }

    #[test]
    fn resampling_preserves_a_tone_and_its_length() {
        let rate = 48_000.0;
        let target = 44_100.0;
        let input: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate).sin() as f32)
            .collect();
        let out = resample(&input, rate, target);
        let expected = (input.len() as f64 * target / rate).round() as usize;
        assert!((out.len() as i64 - expected as i64).abs() <= 1, "{}", out.len());

        // Frequency, by zero crossings well inside the buffer.
        let window = &out[2_000..out.len() - 2_000];
        let mut crossings = 0usize;
        let (mut first, mut last) = (None, 0usize);
        for index in 1..window.len() {
            if window[index - 1] <= 0.0 && window[index] > 0.0 {
                first.get_or_insert(index);
                last = index;
                crossings += 1;
            }
        }
        let first = first.expect("a crossing");
        let measured = (crossings - 1) as f64 * target / (last - first) as f64;
        assert!(
            (measured - 440.0).abs() < 2.0,
            "resampled tone landed at {measured:.2} Hz"
        );
        // Same rate in and out is a pass-through, not a filter.
        let same = resample(&input, rate, rate);
        assert_eq!(same, input);
    }

    #[test]
    fn the_chunk_writer_publishes_whole_chunks_in_order() {
        let (tx, rx) = channel::<StemsMsg>();
        let mut writer = ChunkWriter::new(100, 10);
        writer.restart(0);
        // Two and a half chunks in: two publish, the remainder waits.
        let block = |len: usize, value: i16| -> [Vec<[i16; 2]>; 4] {
            [
                vec![[value, value]; len],
                vec![[value, value]; len],
                vec![[value, value]; len],
                vec![[value, value]; len],
            ]
        };
        writer.push(block(250, 7));
        assert!(!writer.drain(DeckId::A, 1, &tx));
        let mut indices = Vec::new();
        while let Ok(message) = rx.try_recv() {
            if let StemsMsg::Chunk(chunk) = message {
                assert_eq!(chunk.lanes[0].len(), 100);
                assert_eq!(chunk.lanes[0][0], [7, 7]);
                indices.push(chunk.index);
            }
        }
        assert_eq!(indices, vec![0, 1]);

        // The tail joins the next block and publishes chunk 2.
        writer.push(block(50, 9));
        assert!(!writer.drain(DeckId::A, 1, &tx));
        let mut more = Vec::new();
        while let Ok(message) = rx.try_recv() {
            if let StemsMsg::Chunk(chunk) = message {
                more.push(chunk.index);
            }
        }
        assert_eq!(more, vec![2]);
    }

    #[test]
    fn a_run_that_starts_mid_track_lands_on_a_chunk_boundary() {
        let mut writer = ChunkWriter::new(100, 10);
        writer.restart(250);
        // A demixer span is 5.5 seconds and a chunk is one, so a run that
        // starts at the playhead starts mid-chunk. The part-chunk it cannot
        // fill is dropped — the mixed file covers it — and the audio that
        // follows keeps the frame it actually has. Rounding the base DOWN
        // instead labelled that audio as the chunk before it and put every
        // stem in the rest of the track a part-chunk ahead of the mix.
        assert_eq!(writer.base, 300, "runs start on a boundary at or after the seek");
        assert_eq!(writer.skip, 50);
        writer.restart(0);
        assert_eq!((writer.base, writer.skip), (0, 0));
    }

    #[test]
    fn a_mid_chunk_run_publishes_its_audio_where_the_audio_is() {
        let (tx, rx) = channel::<StemsMsg>();
        let mut writer = ChunkWriter::new(100, 10);
        writer.restart(250);
        // Two hundred frames of audio that BEGINS at track frame 250, each
        // frame carrying its own track position.
        let block = |from: i16, len: usize| -> [Vec<[i16; 2]>; 4] {
            let one: Vec<[i16; 2]> = (0..len).map(|i| [from + i as i16; 2]).collect();
            [one.clone(), one.clone(), one.clone(), one]
        };
        let seen = |rx: &Receiver<StemsMsg>| -> Vec<(usize, i16, usize)> {
            let mut out = Vec::new();
            while let Ok(StemsMsg::Chunk(chunk)) = rx.try_recv() {
                out.push((chunk.index, chunk.lanes[0][0][0], chunk.lanes[0].len()));
            }
            out
        };
        writer.push(block(250, 200));
        assert!(!writer.drain(DeckId::A, 1, &tx));
        assert_eq!(
            seen(&rx),
            vec![(3, 300, 100)],
            "chunk N has to hold the audio at frame N * chunk_frames"
        );
        // The next block joins the fifty frames still pending.
        writer.push(block(450, 200));
        assert!(!writer.drain(DeckId::A, 1, &tx));
        assert_eq!(seen(&rx), vec![(4, 400, 100), (5, 500, 100)]);
    }

    fn temp_cache_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "makepad-vj-stem-cache-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    fn silent_pcm(frames: usize, rate: u32) -> Arc<TrackPcm> {
        Arc::new(TrackPcm { frames: vec![[0i16; 2]; frames], sample_rate: rate })
    }

    /// A stem set where stem `i` of `Stem::ALL` is a constant `(i + 1) / 10`,
    /// so a published lane can be traced back to the stem it came from.
    fn flat_stems(frames: usize) -> StemSet {
        std::array::from_fn(|stem| {
            let value = (stem as f32 + 1.0) * 0.1;
            StereoBuf { left: vec![value; frames], right: vec![value; frames] }
        })
    }

    fn published(rx: &Receiver<StemsMsg>) -> Vec<Box<StemChunk>> {
        let mut out = Vec::new();
        while let Ok(message) = rx.try_recv() {
            if let StemsMsg::Chunk(chunk) = message {
                out.push(chunk);
            }
        }
        out
    }

    fn cache_job(pcm: Arc<TrackPcm>, start_secs: f64) -> StemsJob {
        StemsJob { deck: DeckId::A, gen: 1, pcm, source: None, start_secs }
    }

    /// The whole point of a span-granular cache: a track that was separated
    /// before serves its knobs from disk, and the demixer is asked only for
    /// the spans that are actually missing.
    #[test]
    fn a_cached_track_serves_its_spans_and_names_the_first_gap() {
        let root = temp_cache_root("partial");
        let frames = 3 * CHUNK_STEP;
        let pcm = silent_pcm(frames, 44_100);
        let digest = track_digest(&pcm);
        assert_eq!(digest.len(), 64);
        let header = CacheHeader::for_track(model_frames(&pcm) as u64);
        {
            // Exactly what a run cut short after two spans leaves behind.
            let mut cache = StemCache::open(&root, &digest, header.clone()).unwrap();
            cache.write_span(0, &flat_stems(CHUNK_STEP)).unwrap();
            cache.write_span(CHUNK_STEP, &flat_stems(CHUNK_STEP)).unwrap();
        }

        let mut cache = StemCache::open(&root, &digest, header).unwrap();
        assert_eq!(cache.span_count(), 3);
        let (tx, rx) = channel::<StemsMsg>();
        let job = cache_job(pcm.clone(), 0.0);
        let mut writer =
            ChunkWriter::new(chunk_frames(44_100), chunk_count(frames, 44_100));
        let resume = run_cached(&job, &mut cache, &mut writer, 0, &tx);
        assert_eq!(resume, Some(2), "the demixer has to resume at the first gap");

        let chunks = published(&rx);
        // Two spans is eleven whole seconds of track, published in order.
        assert_eq!(
            chunks.iter().map(|c| c.index).collect::<Vec<_>>(),
            (0..11).collect::<Vec<_>>()
        );
        assert!(chunks.iter().all(|c| c.lanes[0].len() == 44_100));
        // Lane order is the deck's (vocals, drums, bass, other) while the
        // cache is the model's (drums, bass, other, vocals), so a flat cache
        // reads back rotated — which is the mapping the mixer depends on.
        let first = &chunks[0].lanes;
        for (lane, stem) in [(0usize, 3usize), (1, 0), (2, 1), (3, 2)] {
            let want = ((stem as f32 + 1.0) * 0.1 * 32767.0) as i16;
            assert!(
                (first[lane][0][0] - want).abs() <= 2,
                "lane {lane} came back as {} not {want}",
                first[lane][0][0]
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A run that was cut short mid-track leaves its finished spans behind,
    /// and the next load picks them up from where they are — not from the
    /// top of the file, and on a chunk boundary.
    #[test]
    fn a_span_written_mid_run_is_served_from_where_it_starts() {
        let root = temp_cache_root("resume");
        let frames = 3 * CHUNK_STEP;
        let pcm = silent_pcm(frames, 44_100);
        let digest = track_digest(&pcm);
        let header = CacheHeader::for_track(model_frames(&pcm) as u64);
        {
            let mut cache = StemCache::open(&root, &digest, header.clone()).unwrap();
            cache.write_span(CHUNK_STEP, &flat_stems(CHUNK_STEP)).unwrap();
        }
        let mut cache = StemCache::open(&root, &digest, header).unwrap();
        let (tx, rx) = channel::<StemsMsg>();
        let job = cache_job(pcm.clone(), 0.0);
        let mut writer =
            ChunkWriter::new(chunk_frames(44_100), chunk_count(frames, 44_100));

        // From the top there is nothing to serve: span 0 is the gap.
        assert_eq!(run_cached(&job, &mut cache, &mut writer, 0, &tx), Some(0));
        assert!(published(&rx).is_empty());

        // From the span that IS there, it plays, and stops at the next gap.
        assert_eq!(run_cached(&job, &mut cache, &mut writer, 1, &tx), Some(2));
        let chunks = published(&rx);
        // Span 1 starts at 5.5 s; the half-second before the next whole
        // chunk is left to the mixed file rather than mislabelled.
        assert_eq!(
            chunks.iter().map(|c| c.index).collect::<Vec<_>>(),
            vec![6, 7, 8, 9, 10]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_cache_key_follows_the_audio() {
        let a = silent_pcm(1_000, 44_100);
        assert_eq!(track_digest(&a), track_digest(&silent_pcm(1_000, 44_100)));
        // Length, rate and a single sample all change the key.
        assert_ne!(track_digest(&a), track_digest(&silent_pcm(1_001, 44_100)));
        assert_ne!(track_digest(&a), track_digest(&silent_pcm(1_000, 48_000)));
        let mut altered = TrackPcm { frames: vec![[0i16; 2]; 1_000], sample_rate: 44_100 };
        altered.frames[700] = [1, 0];
        assert_ne!(track_digest(&a), track_digest(&Arc::new(altered)));
        // …and it is a bare hash, which is what the cache will accept as a
        // directory name.
        assert!(!track_digest(&a).contains(['/', '\\', '.']));
    }

    #[test]
    fn a_playhead_past_the_end_still_names_a_real_span() {
        // The worker starts where the needle is; a stale position must not
        // land past the last span and take the whole track with it.
        let pcm = silent_pcm(2 * CHUNK_STEP, 44_100);
        let spans = model_frames(&pcm).div_ceil(CHUNK_STEP);
        assert_eq!(spans, 2);
        for start_secs in [0.0f64, 5.6, 900.0] {
            let first = ((start_secs.max(0.0) * STEMS_RATE as f64) as usize / CHUNK_STEP)
                .min(spans.saturating_sub(1));
            assert!(first < spans, "{start_secs}s picked span {first} of {spans}");
        }
    }

    #[test]
    fn the_model_frame_count_matches_what_the_resampler_produces() {
        // The cache is addressed in model frames, so a disagreement here
        // makes every entry look like a different track.
        for &(frames, rate) in &[(100_000usize, 48_000u32), (100_000, 44_100), (7, 22_050)] {
            let pcm = TrackPcm { frames: vec![[0i16; 2]; frames], sample_rate: rate };
            assert_eq!(
                model_frames(&pcm),
                to_stereo_buf(&pcm).frames(),
                "{frames} frames at {rate} Hz"
            );
        }
    }

    /// The real thing, end to end, on this machine's checkpoint:
    ///
    /// ```text
    /// VJ_STEMS_E2E=1 VJ_AUDIO_SAMPLE=/path/to/track.mp3 \
    ///     cargo test -p makepad-vj --release -- --nocapture --ignored spans_are_written
    /// ```
    ///
    /// It separates two spans, drops the run, and asserts the next load
    /// serves both of them from the cache without the model and resumes at
    /// the boundary — which is the behaviour the whole cache exists for.
    #[test]
    #[ignore = "loads a 527 MB checkpoint and runs the model"]
    fn spans_are_written_through_as_they_finish() {
        let Ok(sample) = std::env::var("VJ_AUDIO_SAMPLE") else {
            eprintln!("VJ_AUDIO_SAMPLE not set; skipping the end-to-end separation");
            return;
        };
        let path = PathBuf::from(sample.split(':').next().unwrap_or_default());
        let source = crate::wave_analysis::decode_audio_file(&path).expect("decode");
        // Two spans is enough to show write-through and the resume; the
        // whole track would be minutes of model time.
        let keep = (3 * CHUNK_STEP).min(source.frames.len());
        let pcm = Arc::new(TrackPcm {
            frames: source.frames[..keep].to_vec(),
            sample_rate: source.sample_rate,
        });
        let root = temp_cache_root("e2e");
        let digest = track_digest(&pcm);
        let header = CacheHeader::for_track(model_frames(&pcm) as u64);
        let mut model = StemsModel::load(&checkpoint_path()).expect("checkpoint");
        let job = cache_job(pcm.clone(), 0.0);
        let rate = pcm.sample_rate.max(1);
        let (tx, rx) = channel::<StemsMsg>();

        let mut cache = StemCache::open(&root, &digest, header.clone()).unwrap();
        let mut writer = ChunkWriter::new(chunk_frames(rate), chunk_count(pcm.frames.len(), rate));
        // Cut the run short by dropping the receiver after the first spans:
        // run to the end here, then check the cache carries what it made.
        let started = std::time::Instant::now();
        run_demixer(&job, &mut model, Some(&mut cache), &mut writer, 0, &tx).expect("demix");
        let demixed = published(&rx);
        eprintln!(
            "separated {} chunks in {:.1}s; cache has {}/{} spans",
            demixed.len(),
            started.elapsed().as_secs_f64(),
            cache.span_count() - cache.missing_spans().count(),
            cache.span_count(),
        );
        assert!(cache.has_span(0), "span 0 was never written through");
        assert!(cache.has_span(1), "span 1 was never written through");
        drop(cache);

        // A fresh session: everything the last run finished comes back with
        // no model in the picture at all.
        let mut cache = StemCache::open(&root, &digest, header).unwrap();
        let (tx, rx) = channel::<StemsMsg>();
        let mut writer = ChunkWriter::new(chunk_frames(rate), chunk_count(pcm.frames.len(), rate));
        let reloaded = std::time::Instant::now();
        let resume = run_cached(&job, &mut cache, &mut writer, 0, &tx);
        let cached = published(&rx);
        eprintln!(
            "reload served {} chunks in {:.0} ms, resuming at span {resume:?}",
            cached.len(),
            reloaded.elapsed().as_secs_f64() * 1e3
        );
        assert_eq!(
            cached.iter().map(|c| c.index).collect::<Vec<_>>(),
            demixed.iter().map(|c| c.index).collect::<Vec<_>>(),
            "the cached reload must publish the same chunks the run did"
        );
        for (from_cache, from_model) in cached.iter().zip(demixed.iter()) {
            for lane in 0..4 {
                let worst = from_cache.lanes[lane]
                    .iter()
                    .zip(from_model.lanes[lane].iter())
                    .map(|(a, b)| (a[0] as i32 - b[0] as i32).abs())
                    .max()
                    .unwrap_or(0);
                assert!(
                    worst <= 8,
                    "chunk {} lane {lane} came back {worst} counts off",
                    from_cache.index
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_lane_order_is_the_decks_order_not_the_models() {
        // The deck engine and the knobs are vocals, drums, bass, other;
        // the model's own order is drums, bass, other, vocals.
        assert_eq!(STEM_ORDER[0], makepad_ai_stems::Stem::Vocals);
        assert_eq!(STEM_ORDER[1], makepad_ai_stems::Stem::Drums);
        assert_eq!(STEM_ORDER[2], makepad_ai_stems::Stem::Bass);
        assert_eq!(STEM_ORDER[3], makepad_ai_stems::Stem::Other);
        assert_eq!(SIDECAR_NAMES[0], "vocals.wav");
        assert_eq!(SIDECAR_NAMES[3], "other.wav");
        // …and the crate's own order is what `span.stems` is indexed by.
        assert_eq!(makepad_ai_stems::Stem::Drums as usize, 0);
        assert_eq!(makepad_ai_stems::Stem::Vocals as usize, 3);
    }
}

