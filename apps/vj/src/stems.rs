//! Per-deck source separation, off the UI and audio threads.
//!
//! A deck plays the mixed file until separated stems exist for the stretch
//! of track under its playhead; from then on the four stem knobs are live.
//! That is the whole contract, and it is what makes separation usable in a
//! performance: nothing waits for a whole track to be demixed.
//!
//! Two sources, in order:
//!
//! 1. **Sidecar stems** — a `stems/` directory beside the track holding
//!    `drums.wav`, `bass.wav`, `other.wav`, `vocals.wav` on the same
//!    timeline. Free, instant, and how a pre-separated library works.
//! 2. **The demixer** — `makepad_ai_stems::Demixer` streamed from the span
//!    covering the playhead outward, one finished span per model forward.
//!
//! Either way the worker publishes fixed one-second chunks in TRACK frames,
//! so the mixer can index them arithmetically and a chunk that has not
//! arrived simply falls back to the mixed file.

use crate::decks::DeckId;
use crate::mixer::TrackPcm;
use makepad_ai_stems::{Demixer, StemsModel, StereoBuf, SAMPLE_RATE as STEMS_RATE};
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
    /// Per lane, per channel-interleaved frames pending publication.
    pending: [Vec<[i16; 2]>; 4],
}

impl ChunkWriter {
    fn new(chunk_frames: usize, chunk_count: usize) -> ChunkWriter {
        ChunkWriter {
            chunk_frames,
            chunk_count,
            base: 0,
            pending: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
        }
    }

    /// Start a fresh run at `frame`, dropping anything half-collected.
    fn restart(&mut self, frame: usize) {
        // Chunks are published whole, so a run always starts on a boundary.
        self.base = (frame / self.chunk_frames) * self.chunk_frames;
        for lane in self.pending.iter_mut() {
            lane.clear();
        }
    }

    /// Add one lane-aligned block; every lane must be pushed the same length.
    fn push(&mut self, lanes: [Vec<[i16; 2]>; 4]) {
        for (slot, block) in self.pending.iter_mut().zip(lanes) {
            slot.extend_from_slice(&block);
        }
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

fn run_demixer(
    job: &StemsJob,
    model: &mut StemsModel,
    out: &Sender<StemsMsg>,
) -> Result<(), String> {
    let track = to_stereo_buf(&job.pcm);
    let track_rate = job.pcm.sample_rate.max(1) as f64;
    let model_rate = STEMS_RATE as f64;
    let frames = job.pcm.frames.len();
    let rate = job.pcm.sample_rate.max(1);
    let size = chunk_frames(rate);
    let count = chunk_count(frames, rate);
    let mut writer = ChunkWriter::new(size, count);

    // One kernel for the whole run, not one per lane per span.
    let span_kernel = if (track_rate - model_rate).abs() < 0.5 {
        None
    } else {
        Some(ResampleKernel::new(track_rate / model_rate))
    };
    let mut demixer = Demixer::new(model, &track).map_err(|e| e.to_string())?;
    // Start where the needle is, not at the top of the file.
    let start_model_frame = (job.start_secs.max(0.0) * model_rate) as usize;
    if start_model_frame > 0 {
        demixer.seek(start_model_frame);
    }
    let _ = out.send(StemsMsg::Status {
        deck: job.deck,
        gen: job.gen,
        text: "stems: separating…".to_string(),
        working: true,
    });

    let mut first = true;
    loop {
        let span = match demixer.next_span().map_err(|e| e.to_string())? {
            Some(span) => span,
            None => break,
        };
        // Model frames -> track frames.
        let track_start = ((span.start as f64) * track_rate / model_rate).round() as usize;
        if first {
            writer.restart(track_start);
            first = false;
        }
        let mut lanes: Vec<Vec<[i16; 2]>> = Vec::with_capacity(4);
        for stem in STEM_ORDER {
            let buf = &span.stems[stem as usize];
            lanes.push(i16_frames(
                buf,
                0,
                buf.frames(),
                model_rate,
                track_rate,
                span_kernel.as_ref(),
            ));
        }
        let shortest = lanes.iter().map(Vec::len).min().unwrap_or(0);
        for lane in lanes.iter_mut() {
            lane.truncate(shortest);
        }
        let lanes: [Vec<[i16; 2]>; 4] = [
            std::mem::take(&mut lanes[0]),
            std::mem::take(&mut lanes[1]),
            std::mem::take(&mut lanes[2]),
            std::mem::take(&mut lanes[3]),
        ];
        writer.push(lanes);
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
                    if let Err(error) = run_demixer(&job, loaded, &out) {
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
        assert_eq!(writer.base, 200, "runs start on a boundary");
        writer.restart(0);
        assert_eq!(writer.base, 0);
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
