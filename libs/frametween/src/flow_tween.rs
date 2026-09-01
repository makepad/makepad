//! REALTIME FRAME TWEENING, entirely on the GPU: a special set of passes
//! that turns any two adjacent resident NV12 frames into a motion-true
//! in-between at fractional t — silky slow-mo, scratch, and 25 fps footage
//! presented at display rate, with NO pre-conversion and NO CPU burned
//! (the operator's ruling: the cores are needed elsewhere, or best left
//! alone).
//!
//! The algorithm is the classical estimator from `makepad-video-flow`
//! (`estimate.rs`), re-expressed as fragment passes — pyramidal block
//! matching was practically designed for this: its Jacobi sweeps are
//! double-buffered ping-pong passes by construction, the pyramid is a
//! chain of downsamples, the median is a 9-tap sorting network. Per PAIR
//! of source frames the whole stack costs a few milliseconds of GPU and
//! runs once; the warp then serves every display frame inside that pair
//! for the cost of one textured quad.
//!
//! Differences from the CPU path, on purpose:
//! - fields stay FLOAT textures end to end (the i8 mkfl quantization
//!   exists for storage, and nothing is stored here);
//! - instead of the splat-to-intermediate reversal (a scatter, which
//!   fragment shaders cannot do) the warp gathers BOTH one-way fields and
//!   weights them by forward/backward cycle consistency — the standard
//!   gather-only morph. Occluded content leans on the endpoint that can
//!   see it, exactly the mask's job in the mkfl scheme;
//! - the sub-pixel parabola and the 3x3 median port unchanged.
//!
//! Pipeline per pair (LEVELS pyramid levels, SWEEPS sweeps each):
//!   luma L0 (A in R, B in G, 4:1 from the NV12 Y planes)
//!   -> halve x(LEVELS-1)
//!   -> per direction: exhaustive at the top, then per level
//!      sweep xSWEEPS (ping-pong) -> median -> (finer level, vectors x2)
//!   -> sub-pixel parabola at L0 -> final field texture
//!   -> warp pass: NV12 A + B + both fields + t -> RGBA out.

use makepad_widgets::*;

use crate::frame::{
    bgra32_proxy_rgb8, nv12_proxy_rgb8, rgb8_proxy, rgb8_to_bgra32, tl_on, Frame, Pixels,
};
pub use crate::pair_cache::PairKey;

/// THE NEURAL FIELD PRODUCER: one background worker owning the RIFE
/// runtime. Jobs are (generation, pair, two RGB8 proxies); results are
/// the net's intermediate flow + occlusion mask. The worker keeps only
/// the NEWEST job (a busy pair is simply skipped — the classical fields
/// cover it), so it can never fall behind the transport.
pub struct RifeService {
    tx: std::sync::mpsc::SyncSender<RifeJob>,
    result: std::sync::Arc<std::sync::Mutex<Option<RifeProduct>>>,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latency: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(usize, usize), f64>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RifeProductKind {
    Field,
    Midpoint,
    Subdivision { depth: u8 },
}

/// Where the worker gets its two pictures.
///
/// `Frames` is the zero-CPU-on-the-UI-thread path a video host wants: it
/// hands over the resident frame ring and the worker does the proxy
/// downscale itself. `Rgb8` is for hosts whose pictures never were NV12 —
/// a diffusion feed arrives as packed RGB already.
pub enum RifeSource {
    /// The resident ring; `a`/`b` index into it and must be `Pixels::Nv12`.
    Frames(std::sync::Arc<Vec<Frame>>),
    /// Two packed RGB8 pictures at their own size, downscaled here.
    Rgb8 {
        a: std::sync::Arc<Vec<u8>>,
        b: std::sync::Arc<Vec<u8>>,
        width: usize,
        height: usize,
    },
    /// The same, as the BGRA words a texture upload wants — a host that
    /// already holds one need not repack a whole frame to ask a question
    /// about a 384-wide proxy of it.
    Bgra32 {
        a: std::sync::Arc<Vec<u32>>,
        b: std::sync::Arc<Vec<u32>>,
        width: usize,
        height: usize,
    },
}

pub struct RifeJob {
    pub generation: u64,
    pub a: usize,
    pub b: usize,
    pub kind: RifeProductKind,
    pub frames: RifeSource,
    pub width: usize,
    pub height: usize,
    /// A prefetched pair that did not finish before its boundary is shed;
    /// the classical producer owns that traversal from its first frame.
    pub deadline: std::time::Instant,
}

pub struct RifeField {
    pub generation: u64,
    pub a: usize,
    pub b: usize,
    pub width: usize,
    pub height: usize,
    /// Packed RGBA per proxy pixel: t->frame0 xy, t->frame1 xy (pixels).
    pub flow: Vec<f32>,
    /// Post-sigmoid occlusion mask, 0..1.
    pub mask: Vec<f32>,
}

#[derive(Clone)]
pub struct RifeMidpoint {
    pub generation: u64,
    pub a: usize,
    pub b: usize,
    pub width: usize,
    pub height: usize,
    /// Tightly packed RGB8 at the RIFE proxy resolution. The tween view
    /// samples it directly as an endpoint, so the network's midpoint is not
    /// rounded through an RGB -> NV12 -> RGB conversion.
    pub rgb: Vec<u8>,
}

/// AI3's progressive eighth-grid. Slot `i` is time `(i + 1) / 8`; a
/// complete depth therefore occupies slots {3}, {1,3,5}, or {0..6}.
/// Publishing this after every synthesis makes an interrupted deeper level
/// degrade to the last shallower complete set without borrowing a frame.
#[derive(Clone)]
pub struct RifeSubdivision {
    pub generation: u64,
    pub a: usize,
    pub b: usize,
    pub requested_depth: u8,
    pub frames: Vec<Option<RifeMidpoint>>,
}

impl RifeSubdivision {
    pub fn complete_depth(&self) -> u8 {
        ai3_complete_depth(&self.frames)
    }

    pub fn frame_for(&self, depth: u8, k: usize) -> Option<&RifeMidpoint> {
        if !(1..=self.complete_depth()).contains(&depth) || k == 0 || k >= 1usize << depth {
            return None;
        }
        let eighth = k << (3 - depth);
        self.frames.get(eighth - 1)?.as_ref()
    }
}

impl RifeMidpoint {
    pub fn is_valid(&self) -> bool {
        self.width != 0
            && self.height != 0
            && self.rgb.len() == self.width.saturating_mul(self.height).saturating_mul(3)
    }
}

pub enum RifeProduct {
    Field(RifeField),
    Midpoint(RifeMidpoint),
    Subdivision(RifeSubdivision),
}

impl RifeProduct {
    pub fn kind(&self) -> RifeProductKind {
        match self {
            Self::Field(_) => RifeProductKind::Field,
            Self::Midpoint(_) => RifeProductKind::Midpoint,
            Self::Subdivision(value) => {
                RifeProductKind::Subdivision { depth: value.requested_depth }
            }
        }
    }

    pub fn generation(&self) -> u64 {
        match self {
            Self::Field(value) => value.generation,
            Self::Midpoint(value) => value.generation,
            Self::Subdivision(value) => value.generation,
        }
    }

    pub fn a(&self) -> usize {
        match self {
            Self::Field(value) => value.a,
            Self::Midpoint(value) => value.a,
            Self::Subdivision(value) => value.a,
        }
    }

    pub fn b(&self) -> usize {
        match self {
            Self::Field(value) => value.b,
            Self::Midpoint(value) => value.b,
            Self::Subdivision(value) => value.b,
        }
    }

    pub fn field(&self) -> Option<&RifeField> {
        match self {
            Self::Field(value) => Some(value),
            Self::Midpoint(_) | Self::Subdivision(_) => None,
        }
    }

    pub fn midpoint(&self) -> Option<&RifeMidpoint> {
        match self {
            Self::Field(_) => None,
            Self::Midpoint(value) => Some(value),
            Self::Subdivision(_) => None,
        }
    }

    pub fn subdivision(&self) -> Option<&RifeSubdivision> {
        match self {
            Self::Subdivision(value) => Some(value),
            Self::Field(_) | Self::Midpoint(_) => None,
        }
    }

    pub fn byte_len(&self) -> usize {
        match self {
            Self::Field(value) => {
                (value.flow.len() + value.mask.len()) * std::mem::size_of::<f32>()
            }
            Self::Midpoint(value) => value.rgb.len(),
            Self::Subdivision(value) => value
                .frames
                .iter()
                .flatten()
                .map(|frame| frame.rgb.len())
                .sum(),
        }
    }
}

impl RifeService {
    /// Spawn with the checkpoint at `model_path`. Fails soft: a missing
    /// or bad checkpoint returns Err and the caller stays classical.
    pub fn start(model_path: &std::path::Path) -> Result<Self, String> {
        use makepad_ai_rife::rife::{
            Rife, RifeBackendKind, RifeFramePair, RifeScale, RifeWeights,
        };
        if !makepad_ai_rife::rife::rife_device_available() {
            return Err("rife device backend unavailable".into());
        }
        let weights = RifeWeights::load(model_path)
            .map_err(|e| format!("rife checkpoint: {e:?}"))?;
        let model = weights
            .prepare_model(None)
            .map_err(|e| format!("rife prepare: {e:?}"))?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<RifeJob>(1);
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let out = result.clone();
        let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_busy = busy.clone();
        let latency = std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::<(usize, usize), f64>::new(),
        ));
        let worker_latency = latency.clone();
        std::thread::Builder::new()
            .name("vj-rife".into())
            .spawn(move || {
                let rife = Rife::from_model_weights_scaled(
                    model,
                    RifeBackendKind::Device,
                    RifeScale::Half,
                );
                while let Ok(job) = rx.recv() {
                    struct BusyGuard<'a>(&'a std::sync::atomic::AtomicBool);
                    impl Drop for BusyGuard<'_> {
                        fn drop(&mut self) {
                            self.0.store(false, std::sync::atomic::Ordering::Release);
                        }
                    }
                    let _busy = BusyGuard(&worker_busy);
                    if std::time::Instant::now() >= job.deadline {
                        continue;
                    }
                    let (rgb0, rgb1) = match &job.frames {
                        RifeSource::Frames(frames) => {
                            let Some(frame0) = frames.get(job.a) else { continue };
                            let Some(frame1) = frames.get(job.b) else { continue };
                            let (
                                Pixels::Nv12 { data: nv12_0, width, height },
                                Pixels::Nv12 { data: nv12_1, .. },
                            ) = (&frame0.px, &frame1.px)
                            else {
                                continue;
                            };
                            (
                                nv12_proxy_rgb8(
                                    nv12_0,
                                    *width as usize,
                                    *height as usize,
                                    job.width,
                                    job.height,
                                ),
                                nv12_proxy_rgb8(
                                    nv12_1,
                                    *width as usize,
                                    *height as usize,
                                    job.width,
                                    job.height,
                                ),
                            )
                        }
                        RifeSource::Rgb8 { a, b, width, height } => (
                            rgb8_proxy(a, *width, *height, job.width, job.height),
                            rgb8_proxy(b, *width, *height, job.width, job.height),
                        ),
                        RifeSource::Bgra32 { a, b, width, height } => (
                            bgra32_proxy_rgb8(a, *width, *height, job.width, job.height),
                            bgra32_proxy_rgb8(b, *width, *height, job.width, job.height),
                        ),
                    };
                    let record_latency = |seconds: f64| {
                        let mut values = worker_latency.lock().unwrap();
                        let ema = values.entry((job.width, job.height)).or_insert(seconds);
                        *ema += (seconds - *ema) * 0.2;
                    };
                    let synth_elapsed;
                    let product = match job.kind {
                        RifeProductKind::Field => {
                            let Ok(pair) = RifeFramePair::new(
                                &rgb0, &rgb1, job.width, job.height,
                            ) else {
                                continue;
                            };
                            let t0 = std::time::Instant::now();
                            let Ok(field) = rife.flow_field_rgb8(pair, 0.5, None) else {
                                continue;
                            };
                            synth_elapsed = t0.elapsed().as_secs_f64();
                            record_latency(synth_elapsed);
                            // AI1 is the shipped path: preserve its exact
                            // planar-to-interleaved repack and warp inputs.
                            let plane = job.width * job.height;
                            let mut flow = vec![0.0f32; plane * 4];
                            for i in 0..plane {
                                flow[i * 4] = field.flow[i];
                                flow[i * 4 + 1] = field.flow[plane + i];
                                flow[i * 4 + 2] = field.flow[2 * plane + i];
                                flow[i * 4 + 3] = field.flow[3 * plane + i];
                            }
                            RifeProduct::Field(RifeField {
                                generation: job.generation,
                                a: job.a,
                                b: job.b,
                                width: job.width,
                                height: job.height,
                                flow,
                                mask: field.mask,
                            })
                        }
                        RifeProductKind::Midpoint => {
                            let Ok(pair) = RifeFramePair::new(
                                &rgb0, &rgb1, job.width, job.height,
                            ) else {
                                continue;
                            };
                            let t0 = std::time::Instant::now();
                            let Ok(rgb) = rife.interpolate_rgb8_controlled(pair, 0.5, None) else {
                                continue;
                            };
                            synth_elapsed = t0.elapsed().as_secs_f64();
                            record_latency(synth_elapsed);
                            RifeProduct::Midpoint(RifeMidpoint {
                                generation: job.generation,
                                a: job.a,
                                b: job.b,
                                width: job.width,
                                height: job.height,
                                rgb,
                            })
                        }
                        RifeProductKind::Subdivision { depth } => {
                            let depth = depth.clamp(1, 3);
                            let mut grid: Vec<Option<Vec<u8>>> = (0..9).map(|_| None).collect();
                            grid[0] = Some(rgb0);
                            grid[8] = Some(rgb1);
                            let mut ladder = RifeSubdivision {
                                generation: job.generation,
                                a: job.a,
                                b: job.b,
                                requested_depth: depth,
                                frames: (0..7).map(|_| None).collect(),
                            };
                            'levels: for level in 1..=depth {
                                let stride = 1usize << (3 - level);
                                for center in (stride..8).step_by(stride * 2) {
                                    if std::time::Instant::now() >= job.deadline {
                                        break 'levels;
                                    }
                                    let left = center - stride;
                                    let right = center + stride;
                                    let Some(rgb_left) = grid[left].as_ref() else {
                                        break 'levels;
                                    };
                                    let Some(rgb_right) = grid[right].as_ref() else {
                                        break 'levels;
                                    };
                                    let Ok(pair) = RifeFramePair::new(
                                        rgb_left,
                                        rgb_right,
                                        job.width,
                                        job.height,
                                    ) else {
                                        break 'levels;
                                    };
                                    let t0 = std::time::Instant::now();
                                    let Ok(rgb) =
                                        rife.interpolate_rgb8_controlled(pair, 0.5, None)
                                    else {
                                        break 'levels;
                                    };
                                    let elapsed = t0.elapsed().as_secs_f64();
                                    record_latency(elapsed);
                                    grid[center] = Some(rgb.clone());
                                    ladder.frames[center - 1] = Some(RifeMidpoint {
                                        generation: job.generation,
                                        a: job.a,
                                        b: job.b,
                                        width: job.width,
                                        height: job.height,
                                        rgb,
                                    });
                                    // A keyed partial ladder is useful: its
                                    // complete-depth test implements 7→3→1→FL.
                                    *out.lock().unwrap() =
                                        Some(RifeProduct::Subdivision(ladder.clone()));
                                    if tl_on() {
                                        eprintln!(
                                            "tl rife AI3 pair={} level={} {}/{} {}x{} in {:.0}ms",
                                            job.a,
                                            level,
                                            center,
                                            8,
                                            job.width,
                                            job.height,
                                            elapsed * 1000.0
                                        );
                                    }
                                }
                            }
                            continue;
                        }
                    };
                    if tl_on() {
                        eprintln!(
                            "tl rife {:?} pair={} {}x{} in {:.0}ms",
                            job.kind,
                            job.a,
                            job.width,
                            job.height,
                            synth_elapsed * 1000.0
                        );
                    }
                    *out.lock().unwrap() = Some(product);
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, result, busy, latency })
    }

    /// Offer a pair; a busy worker skips it (classical covers the gap). The
    /// atomic closes the channel's one-item waiting-room loophole: there is
    /// at most one accepted pair, never an old queue behind the transport.
    pub fn offer_next(&self, job: RifeJob) -> bool {
        use std::sync::atomic::Ordering;
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        if self.tx.try_send(job).is_ok() {
            true
        } else {
            self.busy.store(false, Ordering::Release);
            false
        }
    }

    pub fn take(&self) -> Option<RifeProduct> {
        self.result.lock().unwrap().take()
    }

    pub fn synth_seconds(&self, width: usize, height: usize) -> Option<f64> {
        self.latency.lock().unwrap().get(&(width, height)).copied()
    }
}

/// The RIFE checkpoint path: `VJ_RIFE_MODEL` (the VJ's own switch, honoured
/// verbatim), `FRAMETWEEN_RIFE_MODEL`, or the repo-local default.
pub fn default_model_path() -> std::path::PathBuf {
    std::env::var_os("VJ_RIFE_MODEL")
        .or_else(|| std::env::var_os("FRAMETWEEN_RIFE_MODEL"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("local/ai_models/rife_v4.26.safetensors"))
}

/// The proxy resolution RIFE sees: 384 wide, aspect-matched height (the
/// net pads internally; flow upsamples to the video in the warp).
pub fn rife_proxy_dims(w: u32, h: u32) -> (usize, usize) {
    let pw = 384usize;
    let ph = ((pw as u64 * h as u64) / w.max(1) as u64).clamp(96, 384) as usize;
    (pw, ph & !1)
}

/// The neural producer's kill switch — the mode menu's AI entries are the
/// opt-in, this only takes them away.
pub fn rife_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("VJ_TWEEN_RIFE")
            .or_else(|_| std::env::var("FRAMETWEEN_RIFE"))
            .map(|v| v != "0")
            .unwrap_or(true)
    })
}

/// Which classical pair owns one AI2 presentation beat. This is the exact
/// eval-rig rule: a fresh midpoint splits the source pair in two; without
/// one, the original pair remains plain FL for its entire lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ai2Pair {
    Original,
    FirstHalf,
    SecondHalf,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ai2FramePlan {
    pub pair: Ai2Pair,
    pub t: f32,
}

pub fn ai2_frame_plan(has_fresh_midpoint: bool, t: f32) -> Ai2FramePlan {
    let t = t.clamp(0.0, 1.0);
    if !has_fresh_midpoint {
        return Ai2FramePlan { pair: Ai2Pair::Original, t };
    }
    if t < 0.5 {
        Ai2FramePlan { pair: Ai2Pair::FirstHalf, t: t * 2.0 }
    } else {
        Ai2FramePlan { pair: Ai2Pair::SecondHalf, t: (t - 0.5) * 2.0 }
    }
}

pub const AI3_MIN_DEPTH: u8 = 1;
pub const AI3_MAX_DEPTH: u8 = 3;
pub const AI3_BOOTSTRAP_SYNTH_SECS: f64 = 0.065;
const AI3_BUDGET_MARGIN: f64 = 1.20;
const AI3_UPGRADE_MARGIN: f64 = 1.35;
const AI3_HOLD_MARGIN: f64 = 1.05;

pub const fn ai3_neural_frames(depth: u8) -> usize {
    let depth = if depth < AI3_MIN_DEPTH {
        AI3_MIN_DEPTH
    } else if depth > AI3_MAX_DEPTH {
        AI3_MAX_DEPTH
    } else {
        depth
    };
    (1usize << depth) - 1
}

fn ai3_depth_with_margin(
    synth_seconds: f64,
    pair_budget_seconds: f64,
    capacity_frames: usize,
    margin: f64,
) -> u8 {
    if !synth_seconds.is_finite()
        || synth_seconds <= 0.0
        || !pair_budget_seconds.is_finite()
        || pair_budget_seconds <= 0.0
    {
        return AI3_MIN_DEPTH;
    }
    for depth in (AI3_MIN_DEPTH..=AI3_MAX_DEPTH).rev() {
        let frames = ai3_neural_frames(depth);
        if frames <= capacity_frames
            && frames as f64 * synth_seconds * margin <= pair_budget_seconds
        {
            return depth;
        }
    }
    // AI3 never invents a d=0 neural tier. If d=1 misses this budget, the
    // existing AI2 admission rule decides between one midpoint and FL.
    AI3_MIN_DEPTH
}

/// Pure capacity-law choice. `pair_budget_seconds` is the platter pair
/// period after the other admitted neural decks' share has been removed;
/// `capacity_frames` is the remaining 5-synth/s law expressed for this pair.
pub fn ai3_budget_depth(
    synth_seconds: f64,
    pair_budget_seconds: f64,
    capacity_frames: usize,
) -> u8 {
    ai3_depth_with_margin(
        synth_seconds,
        pair_budget_seconds,
        capacity_frames,
        AI3_BUDGET_MARGIN,
    )
}

/// The per-deck, per-pair depth latch. Upgrades need two consecutive offers
/// at a stronger margin; an active depth is held through the ordinary margin
/// boundary and drops only when its smaller exit margin no longer fits.
#[derive(Clone, Copy, Debug)]
pub struct Ai3DepthChooser {
    depth: u8,
    pending_upgrade: u8,
    pending_pairs: u8,
}

impl Default for Ai3DepthChooser {
    fn default() -> Self {
        Self { depth: AI3_MIN_DEPTH, pending_upgrade: 0, pending_pairs: 0 }
    }
}

impl Ai3DepthChooser {
    pub fn depth(&self) -> u8 {
        self.depth
    }

    pub fn choose(
        &mut self,
        synth_seconds: f64,
        pair_budget_seconds: f64,
        capacity_frames: usize,
    ) -> u8 {
        let raw = ai3_budget_depth(synth_seconds, pair_budget_seconds, capacity_frames);
        let current_frames = ai3_neural_frames(self.depth);
        let holds = current_frames <= capacity_frames
            && current_frames as f64 * synth_seconds * AI3_HOLD_MARGIN
                <= pair_budget_seconds;
        if raw < self.depth && !holds {
            self.depth = raw;
            self.pending_upgrade = 0;
            self.pending_pairs = 0;
            return self.depth;
        }
        let upgrade = ai3_depth_with_margin(
            synth_seconds,
            pair_budget_seconds,
            capacity_frames,
            AI3_UPGRADE_MARGIN,
        );
        if upgrade > self.depth {
            if self.pending_upgrade == upgrade {
                self.pending_pairs = self.pending_pairs.saturating_add(1);
            } else {
                self.pending_upgrade = upgrade;
                self.pending_pairs = 1;
            }
            if self.pending_pairs >= 2 {
                self.depth = upgrade;
                self.pending_upgrade = 0;
                self.pending_pairs = 0;
            }
        } else {
            self.pending_upgrade = 0;
            self.pending_pairs = 0;
        }
        self.depth
    }
}

/// Deepest complete set in the fixed eighth-grid: 7 → 3 → 1 → FL (0).
pub fn ai3_complete_depth<T>(frames: &[Option<T>]) -> u8 {
    if frames.len() >= 7 && frames[..7].iter().all(Option::is_some) {
        3
    } else if frames.len() >= 6 && [1usize, 3, 5].iter().all(|&i| frames[i].is_some()) {
        2
    } else if frames.get(3).is_some_and(Option::is_some) {
        1
    } else {
        0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ai3FramePlan {
    pub interval: usize,
    pub t: f32,
}

pub fn ai3_frame_plan(depth: u8, t: f32) -> Ai3FramePlan {
    let intervals = 1usize << depth.clamp(AI3_MIN_DEPTH, AI3_MAX_DEPTH);
    let scaled = t.clamp(0.0, 1.0) * intervals as f32;
    let interval = (scaled.floor() as usize).min(intervals - 1);
    Ai3FramePlan { interval, t: (scaled - interval as f32).clamp(0.0, 1.0) }
}

/// `VJ_TWEEN_DEBUG=1|2|3` turns the warp into a diagnostic view (flow
/// field / frame A passthrough / t ramp).
fn tween_debug() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("VJ_TWEEN_DEBUG")
            .or_else(|_| std::env::var("FRAMETWEEN_DEBUG"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0)
    })
}

/// Pyramid depth: level 0 is the flow grid (quarter of source), each
/// further level halves. 4 levels reach +-~40 grid cells of motion after
/// refinement — 160 source pixels, plenty for adjacent frames.
pub const LEVELS: usize = 4;
/// Jacobi sweeps per level (the CPU default).
pub const SWEEPS: usize = 3;

/// Global speculative field-work capacity per display frame. The presenter
/// assigns this one budget EDF across both decks; a view can never consume
/// more than the assigned slice.
pub const FIELD_PREFETCH_OPS_PER_FRAME: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeriveOp {
    Luma0,
    Halve { level: usize },
    LumaField,
    Exhaust { dir: usize },
    Sweep { dir: usize, level: usize, sweep: usize },
    Median { dir: usize, level: usize },
    Subpel { dir: usize },
}

/// The old straight-line field derivation, made explicit so speculative
/// work can stop between any two passes and resume on the next display
/// frame without inventing new estimator state.
pub fn build_derive_ops(seeded: bool, luma_only: bool) -> Vec<DeriveOp> {
    let mut ops = Vec::with_capacity(2 + LEVELS + 2 * (1 + LEVELS * (SWEEPS + 1) + 1));
    ops.push(DeriveOp::Luma0);
    for level in 1..LEVELS {
        ops.push(DeriveOp::Halve { level });
    }
    if luma_only {
        ops.push(DeriveOp::LumaField);
        return ops;
    }
    for dir in 0..2 {
        if !seeded {
            ops.push(DeriveOp::Exhaust { dir });
        }
        for level in (0..LEVELS).rev() {
            for sweep in 0..SWEEPS {
                ops.push(DeriveOp::Sweep { dir, level, sweep });
            }
            ops.push(DeriveOp::Median { dir, level });
        }
        ops.push(DeriveOp::Subpel { dir });
    }
    ops
}

#[derive(Clone, Debug)]
pub struct FieldPrefetch {
    pub key: PairKey,
    pub forward: bool,
    /// Y-plane slots for the predicted pair. UV lives at slot + 1.
    pub plane_y: [usize; 2],
    pub target_gen: usize,
    pub seed_gen: usize,
    pub ops: Vec<DeriveOp>,
    pub cursor: usize,
}

impl FieldPrefetch {
    pub fn remaining(&self) -> usize {
        self.ops.len().saturating_sub(self.cursor)
    }

    pub fn ready_for(&self, key: PairKey) -> bool {
        self.key == key && self.cursor == self.ops.len()
    }
}

/// Earliest-deadline-first allocation of the ONE per-frame field budget.
/// The returned slices sum to no more than the capacity law, including when
/// both decks predict a boundary on the same flip.
pub fn field_prefetch_budgets(
    wanted: [Option<(f64, usize)>; 2],
) -> [usize; 2] {
    let mut order = [0usize, 1usize];
    order.sort_by(|&a, &b| {
        let da = wanted[a].map(|w| w.0).unwrap_or(f64::INFINITY);
        let db = wanted[b].map(|w| w.0).unwrap_or(f64::INFINITY);
        da.total_cmp(&db).then_with(|| a.cmp(&b))
    });
    let mut left = FIELD_PREFETCH_OPS_PER_FRAME;
    let mut out = [0usize; 2];
    for deck in order {
        let Some((_, remaining)) = wanted[deck] else { continue };
        out[deck] = remaining.min(left);
        left -= out[deck];
    }
    out
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // ---- shared pass-space vertex (the flow-warp recipe) ----------------
    // Every stage below fills its own offscreen pass; the stock DrawQuad
    // vertex clips against the PARENT window context and would slice the
    // pass, so transform in pure pass space.

    set_type_default() do #(DrawTweenLuma::script_shader(vm)){
        ..mod.draw.DrawQuad
        // FLOAT DATA PASS: the pipeline's attachment format lives on
        // the SHADER — without this the Metal pipeline expects BGRA8,
        // silently draws nothing into a float target, and the whole
        // stack reads back black (the selftest's smoking gun). 16-bit
        // float: filterable on Apple GPUs (32-bit is not), and flow
        // vectors / 0..255 luma fit easily.
        color_format: @Rgba16F
        tex_y_a: texture_2d(float)
        tex_y_b: texture_2d(float)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        luma_a: fn(uv: vec2) -> float {
            let s = self.tex_y_a.sample(uv)
            if self.rgb_a_on > 0.5 {
                return (16.0 + 219.0 * (s.x * 0.2126 + s.y * 0.7152 + s.z * 0.0722)) / 255.0
            }
            return s.x
        }
        luma_b: fn(uv: vec2) -> float {
            let s = self.tex_y_b.sample(uv)
            if self.rgb_b_on > 0.5 {
                return (16.0 + 219.0 * (s.x * 0.2126 + s.y * 0.7152 + s.z * 0.0722)) / 255.0
            }
            return s.x
        }
        // 4:1 area reduction via four bilinear taps. AI2's RIFE midpoint
        // is RGB; source endpoints remain their zero-copy NV12 Y planes.
        pixel: fn() {
            let o = self.inv_grid
            let mut a = 0.0
            let mut b = 0.0
            let t00 = self.pos + vec2(-o.x, -o.y)
            let t10 = self.pos + vec2(o.x, -o.y)
            let t01 = self.pos + vec2(-o.x, o.y)
            let t11 = self.pos + vec2(o.x, o.y)
            a = a + self.luma_a(t00) + self.luma_a(t10)
            a = a + self.luma_a(t01) + self.luma_a(t11)
            b = b + self.luma_b(t00) + self.luma_b(t10)
            b = b + self.luma_b(t01) + self.luma_b(t11)
            return vec4(a * 63.75, b * 63.75, 0.0, 1.0)
        }
    }

    set_type_default() do #(DrawTweenHalve::script_shader(vm)){
        ..mod.draw.DrawQuad
        // FLOAT DATA PASS: the pipeline's attachment format lives on
        // the SHADER — without this the Metal pipeline expects BGRA8,
        // silently draws nothing into a float target, and the whole
        // stack reads back black (the selftest's smoking gun). 16-bit
        // float: filterable on Apple GPUs (32-bit is not), and flow
        // vectors / 0..255 luma fit easily.
        color_format: @Rgba16F
        tex_src: texture_2d(float)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        // One centered bilinear tap = the exact 2x2 average.
        pixel: fn() {
            let s = self.tex_src.sample(self.pos)
            return vec4(s.x, s.y, 0.0, 1.0)
        }
    }

    set_type_default() do #(DrawTweenExhaust::script_shader(vm)){
        ..mod.draw.DrawQuad
        // FLOAT DATA PASS: the pipeline's attachment format lives on
        // the SHADER — without this the Metal pipeline expects BGRA8,
        // silently draws nothing into a float target, and the whole
        // stack reads back black (the selftest's smoking gun). 16-bit
        // float: filterable on Apple GPUs (32-bit is not), and flow
        // vectors / 0..255 luma fit easily.
        color_format: @Rgba16F
        tex_luma: texture_2d(float)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        // Luma of the FROM frame (dir 0: A = .x) and TO frame at an offset.
        lu_from: fn(uv: vec2) -> float {
            let s = self.tex_luma.sample_nearest(uv, 0.0)
            return mix(s.x, s.y, self.dir)
        }
        lu_to: fn(uv: vec2) -> float {
            let s = self.tex_luma.sample_nearest(uv, 0.0)
            return mix(s.y, s.x, self.dir)
        }
        // 5x5 mean absolute difference for displacement d (level cells).
        sad: fn(d: vec2) -> float {
            let mut sum = 0.0
            let mut j = -2.0
            loop {
                if j > 2.5 { break }
                let mut i = -2.0
                loop {
                    if i > 2.5 { break }
                    let at = self.pos + vec2(i, j) * self.inv_size
                    let to = self.pos + (vec2(i, j) + d) * self.inv_size
                    sum = sum + abs(self.lu_from(at) - self.lu_to(to))
                    i = i + 1.0
                }
                j = j + 1.0
            }
            return sum * 0.04
        }
        // Full search over the coarse radius: nothing to propagate yet.
        // The tiny magnitude bias breaks SAD TIES toward zero motion — on
        // a textureless region every candidate matches equally and the
        // scan order must not pick the corner.
        pixel: fn() {
            let mut best = vec2(0.0, 0.0)
            let mut best_cost = 1e30
            let mut dy = -5.0
            loop {
                if dy > 5.5 { break }
                let mut dx = -5.0
                loop {
                    if dx > 5.5 { break }
                    let c = self.sad(vec2(dx, dy)) + (abs(dx) + abs(dy)) * 0.003
                    if c < best_cost {
                        best_cost = c
                        best = vec2(dx, dy)
                    }
                    dx = dx + 1.0
                }
                dy = dy + 1.0
            }
            return vec4(best.x, best.y, 0.0, 1.0)
        }
    }

    set_type_default() do #(DrawTweenSweep::script_shader(vm)){
        ..mod.draw.DrawQuad
        // FLOAT DATA PASS: the pipeline's attachment format lives on
        // the SHADER — without this the Metal pipeline expects BGRA8,
        // silently draws nothing into a float target, and the whole
        // stack reads back black (the selftest's smoking gun). 16-bit
        // float: filterable on Apple GPUs (32-bit is not), and flow
        // vectors / 0..255 luma fit easily.
        color_format: @Rgba16F
        tex_luma: texture_2d(float)
        tex_prev: texture_2d(float)
        tex_luma_coarse: texture_2d(float)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        // ZERO-MEAN matching: subtract the NEXT pyramid level's sample —
        // a free 2x-coarser local mean — so exposure flicker and strobes
        // (a VJ hall guarantees them) cancel out of the cost instead of
        // dragging every vector. mean_on = 0 at the top level.
        lu_from: fn(uv: vec2) -> float {
            let s = self.tex_luma.sample_nearest(uv, 0.0)
            let m = self.tex_luma_coarse.sample(uv)
            return mix(s.x, s.y, self.dir) - mix(m.x, m.y, self.dir) * self.mean_on
        }
        lu_to: fn(uv: vec2) -> float {
            let s = self.tex_luma.sample_nearest(uv, 0.0)
            let m = self.tex_luma_coarse.sample(uv)
            return mix(s.y, s.x, self.dir) - mix(m.y, m.x, self.dir) * self.mean_on
        }
        prev_at: fn(uv: vec2) -> vec2 {
            // prev_scale doubles coarser-level vectors on the way down.
            return self.tex_prev.sample_nearest(uv, 0.0).xy * self.prev_scale
        }
        sad: fn(d: vec2) -> float {
            let mut sum = 0.0
            let mut j = -2.0
            loop {
                if j > 2.5 { break }
                let mut i = -2.0
                loop {
                    if i > 2.5 { break }
                    let at = self.pos + vec2(i, j) * self.inv_size
                    let to = self.pos + (vec2(i, j) + d) * self.inv_size
                    sum = sum + abs(self.lu_from(at) - self.lu_to(to))
                    i = i + 1.0
                }
                j = j + 1.0
            }
            return sum * 0.04
        }
        // The smoothness charge: L1 disagreement with the previous sweep's
        // neighbours (the Jacobi read side), lambda luma units per cell —
        // EDGE-AWARE: the weight falls off with the local luma gradient,
        // so flow stops bleeding across object boundaries (the
        // background-drags-with-the-dancer artifact).
        edge_weight: fn() -> float {
            let l = self.tex_luma.sample_nearest(self.pos - vec2(self.inv_size.x, 0.0), 0.0)
            let r = self.tex_luma.sample_nearest(self.pos + vec2(self.inv_size.x, 0.0), 0.0)
            let u = self.tex_luma.sample_nearest(self.pos - vec2(0.0, self.inv_size.y), 0.0)
            let dn = self.tex_luma.sample_nearest(self.pos + vec2(0.0, self.inv_size.y), 0.0)
            let gx = abs(mix(r.x, r.y, self.dir) - mix(l.x, l.y, self.dir))
            let gy = abs(mix(dn.x, dn.y, self.dir) - mix(u.x, u.y, self.dir))
            return 1.0 / (1.0 + (gx + gy) * 0.06)
        }
        smooth: fn(d: vec2) -> float {
            let l = self.prev_at(self.pos + vec2(-self.inv_size.x, 0.0))
            let r = self.prev_at(self.pos + vec2(self.inv_size.x, 0.0))
            let u = self.prev_at(self.pos + vec2(0.0, -self.inv_size.y))
            let dn = self.prev_at(self.pos + vec2(0.0, self.inv_size.y))
            let mut sum = 0.0
            sum = sum + abs(d.x - l.x) + abs(d.y - l.y)
            sum = sum + abs(d.x - r.x) + abs(d.y - r.y)
            sum = sum + abs(d.x - u.x) + abs(d.y - u.y)
            sum = sum + abs(d.x - dn.x) + abs(d.y - dn.y)
            return sum * 0.25
        }
        cost: fn(d: vec2) -> float {
            return self.sad(d) + self.lambda * self.edge_weight() * self.smooth(d)
        }
        pixel: fn() {
            let here = self.prev_at(self.pos)
            let mut best = here
            let mut best_cost = self.cost(here)
            // Neighbour propagation: a good vector crosses flat patches.
            let mut k = 0.0
            loop {
                if k > 3.5 { break }
                let mut off = vec2(-self.inv_size.x, 0.0)
                if k > 0.5 { off = vec2(self.inv_size.x, 0.0) }
                if k > 1.5 { off = vec2(0.0, -self.inv_size.y) }
                if k > 2.5 { off = vec2(0.0, self.inv_size.y) }
                let cand = self.prev_at(self.pos + off)
                let c = self.cost(cand)
                if c < best_cost {
                    best_cost = c
                    best = cand
                }
                k = k + 1.0
            }
            // Local refinement window around the incumbent.
            let mut n = 0.0
            loop {
                if n > 11.5 { break }
                let mut o = vec2(-1.0, 0.0)
                if n > 0.5 { o = vec2(1.0, 0.0) }
                if n > 1.5 { o = vec2(0.0, -1.0) }
                if n > 2.5 { o = vec2(0.0, 1.0) }
                if n > 3.5 { o = vec2(-1.0, -1.0) }
                if n > 4.5 { o = vec2(1.0, -1.0) }
                if n > 5.5 { o = vec2(-1.0, 1.0) }
                if n > 6.5 { o = vec2(1.0, 1.0) }
                if n > 7.5 { o = vec2(-2.0, 0.0) }
                if n > 8.5 { o = vec2(2.0, 0.0) }
                if n > 9.5 { o = vec2(0.0, -2.0) }
                if n > 10.5 { o = vec2(0.0, 2.0) }
                let cand = here + o
                let c = self.cost(cand)
                if c < best_cost {
                    best_cost = c
                    best = cand
                }
                n = n + 1.0
            }
            return vec4(best.x, best.y, 0.0, 1.0)
        }
    }

    set_type_default() do #(DrawTweenMedian::script_shader(vm)){
        ..mod.draw.DrawQuad
        // FLOAT DATA PASS: the pipeline's attachment format lives on
        // the SHADER — without this the Metal pipeline expects BGRA8,
        // silently draws nothing into a float target, and the whole
        // stack reads back black (the selftest's smoking gun). 16-bit
        // float: filterable on Apple GPUs (32-bit is not), and flow
        // vectors / 0..255 luma fit easily.
        color_format: @Rgba16F
        tex_src: texture_2d(float)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        // Exact 3x3 median (Smith's network): sort each row into
        // (lo, mid, hi), then med3(max of los, med3 of mids, min of his).
        med3: fn(a: float, b: float, c: float) -> float {
            return max(min(a, b), min(max(a, b), c))
        }
        med9: fn(a: float, b: float, c: float, d: float, e: float, f: float, g: float, h: float, i: float) -> float {
            let lo = max(max(min(min(a, b), c), min(min(d, e), f)), min(min(g, h), i))
            let mid = self.med3(self.med3(a, b, c), self.med3(d, e, f), self.med3(g, h, i))
            let hi = min(min(max(max(a, b), c), max(max(d, e), f)), max(max(g, h), i))
            return self.med3(lo, mid, hi)
        }
        pixel: fn() {
            let dx = self.inv_size.x
            let dy = self.inv_size.y
            let s00 = self.tex_src.sample_nearest(self.pos + vec2(-dx, -dy), 0.0).xy
            let s10 = self.tex_src.sample_nearest(self.pos + vec2(0.0, -dy), 0.0).xy
            let s20 = self.tex_src.sample_nearest(self.pos + vec2(dx, -dy), 0.0).xy
            let s01 = self.tex_src.sample_nearest(self.pos + vec2(-dx, 0.0), 0.0).xy
            let s11 = self.tex_src.sample_nearest(self.pos, 0.0).xy
            let s21 = self.tex_src.sample_nearest(self.pos + vec2(dx, 0.0), 0.0).xy
            let s02 = self.tex_src.sample_nearest(self.pos + vec2(-dx, dy), 0.0).xy
            let s12 = self.tex_src.sample_nearest(self.pos + vec2(0.0, dy), 0.0).xy
            let s22 = self.tex_src.sample_nearest(self.pos + vec2(dx, dy), 0.0).xy
            let mx = self.med9(s00.x, s10.x, s20.x, s01.x, s11.x, s21.x, s02.x, s12.x, s22.x)
            let my = self.med9(s00.y, s10.y, s20.y, s01.y, s11.y, s21.y, s02.y, s12.y, s22.y)
            return vec4(mx, my, 0.0, 1.0)
        }
    }

    set_type_default() do #(DrawTweenSubpel::script_shader(vm)){
        ..mod.draw.DrawQuad
        // FLOAT DATA PASS: the pipeline's attachment format lives on
        // the SHADER — without this the Metal pipeline expects BGRA8,
        // silently draws nothing into a float target, and the whole
        // stack reads back black (the selftest's smoking gun). 16-bit
        // float: filterable on Apple GPUs (32-bit is not), and flow
        // vectors / 0..255 luma fit easily.
        color_format: @Rgba16F
        tex_luma: texture_2d(float)
        tex_field: texture_2d(float)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        lu_from: fn(uv: vec2) -> float {
            let s = self.tex_luma.sample_nearest(uv, 0.0)
            return mix(s.x, s.y, self.dir)
        }
        lu_to: fn(uv: vec2) -> float {
            let s = self.tex_luma.sample_nearest(uv, 0.0)
            return mix(s.y, s.x, self.dir)
        }
        sad: fn(d: vec2) -> float {
            let mut sum = 0.0
            let mut j = -2.0
            loop {
                if j > 2.5 { break }
                let mut i = -2.0
                loop {
                    if i > 2.5 { break }
                    let at = self.pos + vec2(i, j) * self.inv_size
                    let to = self.pos + (vec2(i, j) + d) * self.inv_size
                    sum = sum + abs(self.lu_from(at) - self.lu_to(to))
                    i = i + 1.0
                }
                j = j + 1.0
            }
            return sum * 0.04
        }
        // Parabola fit on the SAD around the integer optimum, per axis —
        // the whole sub-pixel story, ported verbatim.
        pixel: fn() {
            let d = self.tex_field.sample_nearest(self.pos, 0.0).xy
            let c0 = self.sad(d)
            let cl = self.sad(d + vec2(-1.0, 0.0))
            let cr = self.sad(d + vec2(1.0, 0.0))
            let cu = self.sad(d + vec2(0.0, -1.0))
            let cd = self.sad(d + vec2(0.0, 1.0))
            let mut fx = 0.0
            let dxx = cl + cr - 2.0 * c0
            if dxx > 1e-6 {
                fx = clamp(0.5 * (cl - cr) / dxx, -0.5, 0.5)
            }
            let mut fy = 0.0
            let dyy = cu + cd - 2.0 * c0
            if dyy > 1e-6 {
                fy = clamp(0.5 * (cu - cd) / dyy, -0.5, 0.5)
            }
            return vec4(d.x + fx, d.y + fy, 0.0, 1.0)
        }
    }

    set_type_default() do #(DrawTweenWarp::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_y_a: texture_2d(float)
        tex_uv_a: texture_2d(float)
        tex_y_b: texture_2d(float)
        tex_uv_b: texture_2d(float)
        tex_fwd: texture_2d(float)
        tex_bwd: texture_2d(float)
        tex_rife: texture_2d(float)
        tex_rife_mask: texture_2d(float)
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }
        nv12_a: fn(uv: vec2) -> vec3 {
            let c = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
            if self.rgb_a_on > 0.5 {
                return self.tex_y_a.sample(c).xyz
            }
            let y = (self.tex_y_a.sample(c).x * 255.0 - 16.0) / 219.0
            let u2 = self.tex_uv_a.sample(c).xy
            let u = (u2.x * 255.0 - 128.0) / 224.0
            let v = (u2.y * 255.0 - 128.0) / 224.0
            return vec3(y + 1.5748 * v, y - 0.1873 * u - 0.4681 * v, y + 1.8556 * u)
        }
        nv12_b: fn(uv: vec2) -> vec3 {
            let c = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
            if self.rgb_b_on > 0.5 {
                return self.tex_y_b.sample(c).xyz
            }
            let y = (self.tex_y_b.sample(c).x * 255.0 - 16.0) / 219.0
            let u2 = self.tex_uv_b.sample(c).xy
            let u = (u2.x * 255.0 - 128.0) / 224.0
            let v = (u2.y * 255.0 - 128.0) / 224.0
            return vec3(y + 1.5748 * v, y - 0.1873 * u - 0.4681 * v, y + 1.8556 * u)
        }
        // The gather-only morph: sample each endpoint back along its own
        // field, weight by (time distance) x (cycle consistency). A cell
        // one frame cannot see leans on the frame that can — the
        // occlusion mask's job in the stored-payload scheme.
        //
        // RIFE MODE (rife_on = 1): tex_rife carries the NET's fields —
        // INTERMEDIATE-defined at t=0.5, RG = t->frame0 xy, BA =
        // t->frame1 xy, in proxy pixels — plus the learned occlusion mask.
        // Intermediate-defined flow makes the backward gather EXACT (no
        // small-motion approximation), which is the whole reason the
        // neural producer feeds this same pass.
        rife_pixel: fn() -> vec4 {
            let t = self.t_pair
            let f = self.tex_rife.sample(self.pos)
            let m = self.tex_rife_mask.sample(self.pos).x
            let a = self.nv12_a(self.pos + f.xy * (t / 0.5) * self.rife_inv)
            let b = self.nv12_b(self.pos + f.zw * ((1.0 - t) / 0.5) * self.rife_inv)
            let wa = (1.0 - t) * (0.02 + m)
            let wb = t * (1.02 - m)
            let rgb = (a * wa + b * wb) / (wa + wb)
            return vec4(clamp(rgb.x, 0.0, 1.0), clamp(rgb.y, 0.0, 1.0), clamp(rgb.z, 0.0, 1.0), 1.0)
        }
        pixel: fn() {
            let t = self.t_pair
            // FADE mode: a plain crossfade — no fields, no gather. The
            // honest tier for footage where flow reads as rubber.
            if self.dbg < 0.5 && self.fade_on > 0.5 {
                let a = self.nv12_a(self.pos)
                let b = self.nv12_b(self.pos)
                let rgb = a * (1.0 - t) + b * t
                return vec4(clamp(rgb.x, 0.0, 1.0), clamp(rgb.y, 0.0, 1.0), clamp(rgb.z, 0.0, 1.0), 1.0)
            }
            if self.dbg < 0.5 && self.rife_on > 0.5 {
                return self.rife_pixel()
            }
            let fw = self.tex_fwd.sample(self.pos).xy
            let bw = self.tex_bwd.sample(self.pos).xy
            // VJ_TWEEN_DEBUG: 1 = flow field (x red, y green, 0 = mid
            // gray), 2 = frame A straight through (validates the planes
            // and YUV inside THIS widget), 3 = t ramp.
            if self.dbg > 3.5 {
                // dbg 4: tex_fwd carries LUMA L0 (0..255) — show it.
                let l = self.tex_fwd.sample(self.pos).x / 255.0
                return vec4(l, l, l, 1.0)
            }
            if self.dbg > 2.5 {
                return vec4(t, t, t, 1.0)
            }
            if self.dbg > 1.5 {
                let c = self.nv12_a(self.pos)
                return vec4(c.x, c.y, c.z, 1.0)
            }
            if self.dbg > 0.5 {
                return vec4(
                    clamp(0.5 + fw.x * 0.05, 0.0, 1.0),
                    clamp(0.5 + fw.y * 0.05, 0.0, 1.0),
                    clamp(0.5 + bw.x * 0.05, 0.0, 1.0),
                    1.0
                )
            }
            let a = self.nv12_a(self.pos - fw * t * self.inv_grid)
            let b = self.nv12_b(self.pos - bw * (1.0 - t) * self.inv_grid)
            let bw_at_f = self.tex_bwd.sample(clamp(self.pos + fw * self.inv_grid, vec2(0.0, 0.0), vec2(1.0, 1.0))).xy
            let fw_at_b = self.tex_fwd.sample(clamp(self.pos + bw * self.inv_grid, vec2(0.0, 0.0), vec2(1.0, 1.0))).xy
            let ca = 1.0 / (1.0 + length(fw + bw_at_f))
            let cb = 1.0 / (1.0 + length(bw + fw_at_b))
            let wa = (1.0 - t) * (0.05 + ca)
            let wb = t * (0.05 + cb)
            let rgb = (a * wa + b * wb) / (wa + wb)
            return vec4(clamp(rgb.x, 0.0, 1.0), clamp(rgb.y, 0.0, 1.0), clamp(rgb.z, 0.0, 1.0), 1.0)
        }
    }

    mod.widgets.FlowTweenViewBase = #(FlowTweenView::register_widget(vm))
    mod.widgets.FlowTweenView = set_type_default() do mod.widgets.FlowTweenViewBase{
        width: 4
        height: 4
    }
}

/// Per the draw-shader layout law: only `#[live]` instance fields after
/// the `#[deref]`.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweenLuma {
    #[deref]
    pub draw_super: DrawQuad,
    /// One texel of the SOURCE Y plane in uv units (for the 4:1 box taps).
    #[live]
    pub inv_grid: Vec2f,
    /// The corresponding endpoint texture is packed RGB rather than Y.
    #[live]
    pub rgb_a_on: f32,
    #[live]
    pub rgb_b_on: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweenHalve {
    #[deref]
    pub draw_super: DrawQuad,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweenExhaust {
    #[deref]
    pub draw_super: DrawQuad,
    /// 0.0 = A→B (from = .x), 1.0 = B→A.
    #[live]
    pub dir: f32,
    /// One cell of THIS level in uv units.
    #[live]
    pub inv_size: Vec2f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweenSweep {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub dir: f32,
    #[live]
    pub inv_size: Vec2f,
    /// 2.0 when tex_prev is the coarser level's field, else 1.0.
    #[live(1.0)]
    pub prev_scale: f32,
    #[live(1.5)]
    pub lambda: f32,
    /// 1.0 = zero-mean matching (tex_luma_coarse bound to level+1).
    #[live]
    pub mean_on: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweenMedian {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub inv_size: Vec2f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweenSubpel {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub dir: f32,
    #[live]
    pub inv_size: Vec2f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawTweenWarp {
    #[deref]
    pub draw_super: DrawQuad,
    /// Fractional position inside the pair (0 = frame A, 1 = frame B).
    #[live]
    pub t_pair: f32,
    /// One LEVEL-0 flow cell in uv units (vector cells → uv offsets).
    #[live]
    pub inv_grid: Vec2f,
    /// Debug visualization (VJ_TWEEN_DEBUG): 0 off, 1 flow, 2 frame A, 3 t.
    #[live]
    pub dbg: f32,
    /// 1.0 = warp from the NEURAL fields in tex_rife (see rife_pixel).
    #[live]
    pub rife_on: f32,
    /// One RIFE proxy pixel in uv units.
    #[live]
    pub rife_inv: Vec2f,
    /// 1.0 = plain crossfade (FADE mode) — skips every field entirely.
    #[live]
    pub fade_on: f32,
    /// AI2 binds its RGB midpoint in either endpoint slot for one half-pair.
    #[live]
    pub rgb_a_on: f32,
    #[live]
    pub rgb_b_on: f32,
}

/// One offscreen stage: its pass, its draw list, and (for the flow
/// stages) which scratch target it renders into.
struct Stage {
    pass: DrawPass,
    draw_list: DrawList,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct FlowTweenView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_luma: DrawTweenLuma,
    #[live]
    draw_halve: DrawTweenHalve,
    #[live]
    draw_exhaust: DrawTweenExhaust,
    #[live]
    draw_sweep: DrawTweenSweep,
    #[live]
    draw_median: DrawTweenMedian,
    #[live]
    draw_subpel: DrawTweenSubpel,
    #[live]
    draw_warp: DrawTweenWarp,
    #[rust]
    area: Area,
    /// NV12 plane ring: current A, current B, then the one new frame for
    /// the predicted adjacent pair. Adoption rotates handles; it uploads
    /// no pixels on the boundary frame.
    #[rust]
    planes: Option<[Texture; 6]>,
    /// The ring holds packed RGB pictures, not NV12 planes: each pair of
    /// slots is ONE BGRA texture bound in both, and the shaders' rgb_*_on
    /// branch reads it directly. A diffusion feed never was NV12 and must
    /// not be round-tripped through 4:2:0 chroma to be tweened.
    #[rust]
    rgb_planes: bool,
    #[rust]
    size: (u32, u32),
    /// Luma pyramid targets (RG in RGBA32F), one per level.
    #[rust]
    luma_tex: Vec<Texture>,
    /// Flow scratch: ping, pong, and the per-level median output.
    #[rust]
    scratch: Vec<Texture>,
    /// Two generations of final per-direction fields at level 0. The warp
    /// and temporal seed read `field_gen`; speculation writes the other.
    #[rust]
    field_tex: Vec<Texture>,
    #[rust]
    field_gen: usize,
    /// The warp output (fixed-size, Image-hostable).
    #[new]
    warp_out: Texture,
    #[rust]
    target_size: (u32, u32),
    /// Pass pool, allocated on first use: enough stages for the whole
    /// stack (pyramid + two directions + warp).
    #[rust]
    stages: Vec<Stage>,
    #[rust]
    t: f32,
    /// Debug-view override (the selftest cycles it); None = the env var.
    #[rust]
    dbg_override: Option<f32>,
    /// The flow stack must re-run (the pair changed).
    #[rust]
    flow_dirty: bool,
    /// Exact active lease and speculative destination. A completed result
    /// is adoptable only when every PairKey component matches.
    #[rust]
    pair_key: Option<PairKey>,
    #[rust]
    prefetch: Option<FieldPrefetch>,
    /// EDF-assigned part of the shared per-frame capacity.
    #[rust]
    derive_budget: usize,
    /// A previous pair's FINAL fields exist in field_tex — the next
    /// pair's coarse level seeds from them instead of the exhaustive
    /// search (temporal seeding: steadier fields, no per-pair shimmer).
    #[rust]
    have_prev_field: bool,
    /// This pair is a hard SCENE CUT: warp snaps to the nearest endpoint
    /// instead of morphing two unrelated pictures.
    #[rust]
    cut: bool,
    /// FADE mode: crossfade only — the flow stack never runs (and
    /// flow_dirty is left standing so switching back to flow re-derives).
    #[rust]
    fade: bool,
    #[rust]
    rendered: bool,
    /// Neural fields for the CURRENT pair (flow RGBA32F + mask R8), and
    /// which pair they belong to.
    #[rust]
    rife_tex: Option<(Texture, Texture)>,
    #[rust]
    rife_pair: Option<usize>,
    #[rust]
    rife_dims: (usize, usize),
    /// AI2's fresh, pair-keyed RIFE midpoint and the half currently leased
    /// by the presenter. The texture may remain allocated when inactive;
    /// `ai2_half` alone decides whether it can be sampled.
    #[rust]
    ai2_midpoint_tex: Option<Texture>,
    #[rust]
    ai2_midpoint_dims: (usize, usize),
    #[rust]
    ai2_half: Option<Ai2Pair>,
    /// AI3's frozen complete set for this source-pair lease. The textures
    /// are ordered at k/2^depth; `ai3_interval` selects one classical pair.
    #[rust]
    ai3_frames_tex: Vec<Texture>,
    #[rust]
    ai3_frames_dims: (usize, usize),
    #[rust]
    ai3_depth: u8,
    #[rust]
    ai3_interval: Option<usize>,
    /// How many pairs the current field has been REUSED for. Motion is
    /// temporally coherent, so a slightly stale neural field beats
    /// flip-flopping to the classical producer — two different flow
    /// interpretations alternating at source-frame rate reads as wobble.
    #[rust]
    rife_age: u32,
}

impl FlowTweenView {
    fn upload_nv12_plane_pair(
        cx: &mut Cx,
        planes: &[Texture; 6],
        y_slot: usize,
        data: &[u8],
        w: usize,
        h: usize,
    ) {
        let y_len = w * h;
        let uv_len = (w / 2) * (h / 2) * 2;
        for (tex, bytes) in [
            (&planes[y_slot], &data[..y_len]),
            (&planes[y_slot + 1], &data[y_len..y_len + uv_len]),
        ] {
            let mut buf = tex.take_vec_u8(cx);
            buf.clear();
            buf.extend_from_slice(bytes);
            tex.put_back_vec_u8(cx, buf, None);
        }
    }

    /// Pack RGB8 into one BGRA endpoint slot. Both slots of the pair hold
    /// the same handle, so only the even one is written.
    fn upload_rgb_plane(
        cx: &mut Cx,
        planes: &[Texture; 6],
        slot: usize,
        data: &[u8],
        w: usize,
        h: usize,
    ) {
        let mut buf = planes[slot].take_vec_u32(cx);
        buf.clear();
        buf.reserve(w * h);
        // Same word order as `rgb8_to_bgra32`: red in the high byte.
        buf.extend(data.chunks_exact(3).take(w * h).map(|px| {
            0xff00_0000 | (px[0] as u32) << 16 | (px[1] as u32) << 8 | px[2] as u32
        }));
        buf.resize(w * h, 0xff00_0000);
        planes[slot].put_back_vec_u32(cx, buf, None);
    }

    /// Put ready BGRA words in one endpoint slot, no repacking at all.
    fn upload_bgra_plane(
        cx: &mut Cx,
        planes: &[Texture; 6],
        slot: usize,
        data: &[u32],
        w: usize,
        h: usize,
    ) {
        let mut buf = planes[slot].take_vec_u32(cx);
        buf.clear();
        buf.extend_from_slice(&data[..w * h]);
        planes[slot].put_back_vec_u32(cx, buf, None);
    }

    fn ensure_rgb_plane_ring(&mut self, cx: &mut Cx, width: u32, height: u32) {
        if self.size == (width, height) && self.planes.is_some() && self.rgb_planes {
            return;
        }
        let (w, h) = (width as usize, height as usize);
        let mk = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: w,
                    height: h,
                    data: Some(vec![0xff00_0000; w * h]),
                    updated: TextureUpdated::Full,
                },
            )
        };
        let (a, b, c) = (mk(cx), mk(cx), mk(cx));
        self.planes =
            Some([a.clone(), a, b.clone(), b, c.clone(), c]);
        self.rgb_planes = true;
        self.size = (width, height);
        self.field_gen = 0;
        self.have_prev_field = false;
        self.prefetch = None;
    }

    fn ensure_plane_ring(&mut self, cx: &mut Cx, width: u32, height: u32) {
        if self.size == (width, height) && self.planes.is_some() && !self.rgb_planes {
            return;
        }
        self.rgb_planes = false;
        let (w, h) = (width as usize, height as usize);
        let mk_y = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: w,
                    height: h,
                    data: Some(vec![0; w * h]),
                    unpack_row_length: None,
                    updated: TextureUpdated::Full,
                },
            )
        };
        let mk_uv = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecRGu8 {
                    width: w / 2,
                    height: h / 2,
                    data: Some(vec![0; (w / 2) * (h / 2) * 2]),
                    unpack_row_length: None,
                    updated: TextureUpdated::Full,
                },
            )
        };
        self.planes = Some([
            mk_y(cx), mk_uv(cx), mk_y(cx), mk_uv(cx), mk_y(cx), mk_uv(cx),
        ]);
        self.size = (width, height);
        self.field_gen = 0;
        self.have_prev_field = false;
        self.prefetch = None;
    }

    /// Upload a new PAIR of NV12 frames. The flow stack re-runs on the
    /// next draw; the warp serves every display frame until the pair
    /// changes again.
    pub fn set_pair(
        &mut self,
        cx: &mut Cx,
        a: &[u8],
        b: &[u8],
        width: u32,
        height: u32,
    ) {
        self.set_pair_inner(cx, None, a, b, width, height);
    }

    /// Upload a new pair of PACKED RGB8 frames — the same tween stack, for
    /// a host whose pictures never were NV12. `t` and every mode behave
    /// exactly as on the NV12 path; only the endpoint sampling differs.
    pub fn set_pair_rgb8(
        &mut self,
        cx: &mut Cx,
        a: &[u8],
        b: &[u8],
        width: u32,
        height: u32,
    ) {
        let (w, h) = (width as usize, height as usize);
        if w < 8 || h < 8 || a.len() < w * h * 3 || b.len() < w * h * 3 {
            return;
        }
        self.ai2_half = None;
        self.ai3_interval = None;
        self.prefetch = None;
        self.ensure_rgb_plane_ring(cx, width, height);
        let planes = self.planes.clone().unwrap();
        Self::upload_rgb_plane(cx, &planes, 0, a, w, h);
        Self::upload_rgb_plane(cx, &planes, 2, b, w, h);
        self.pair_key = None;
        self.flow_dirty = true;
        self.area.redraw(cx);
    }

    /// The same, from the BGRA words a host already holds.
    pub fn set_pair_bgra32(
        &mut self,
        cx: &mut Cx,
        a: &[u32],
        b: &[u32],
        width: u32,
        height: u32,
    ) {
        let (w, h) = (width as usize, height as usize);
        if w < 8 || h < 8 || a.len() < w * h || b.len() < w * h {
            return;
        }
        self.ai2_half = None;
        self.ai3_interval = None;
        self.prefetch = None;
        self.ensure_rgb_plane_ring(cx, width, height);
        let planes = self.planes.clone().unwrap();
        Self::upload_bgra_plane(cx, &planes, 0, a, w, h);
        Self::upload_bgra_plane(cx, &planes, 2, b, w, h);
        self.pair_key = None;
        self.flow_dirty = true;
        self.area.redraw(cx);
    }

    /// Advance a running BGRA feed by one picture (see `push_rgb8`).
    pub fn push_bgra32(&mut self, cx: &mut Cx, next: &[u32], width: u32, height: u32) -> bool {
        let (w, h) = (width as usize, height as usize);
        if w < 8
            || h < 8
            || next.len() < w * h
            || !self.rgb_planes
            || self.size != (width, height)
        {
            return false;
        }
        let Some(planes) = self.rotate_ring() else { return false };
        Self::upload_bgra_plane(cx, &planes, 2, next, w, h);
        self.planes = Some(planes);
        self.ai2_half = None;
        self.ai3_interval = None;
        self.prefetch = None;
        self.pair_key = None;
        self.flow_dirty = true;
        self.area.redraw(cx);
        true
    }

    /// Rotate (A, B, spare) -> (B, spare, A) so a running feed uploads the
    /// one NEW picture and never re-uploads the one it already has.
    fn rotate_ring(&mut self) -> Option<[Texture; 6]> {
        let old = self.planes.take()?;
        Some([
            old[2].clone(),
            old[3].clone(),
            old[4].clone(),
            old[5].clone(),
            old[0].clone(),
            old[1].clone(),
        ])
    }

    /// Advance a running RGB feed by ONE picture: what was B becomes A and
    /// the new frame becomes B, so a real frame is uploaded once, not twice.
    /// The fields re-derive; temporal seeding carries the previous pair in.
    pub fn push_rgb8(&mut self, cx: &mut Cx, next: &[u8], width: u32, height: u32) -> bool {
        let (w, h) = (width as usize, height as usize);
        if w < 8
            || h < 8
            || next.len() < w * h * 3
            || !self.rgb_planes
            || self.size != (width, height)
        {
            return false;
        }
        let Some(planes) = self.rotate_ring() else { return false };
        Self::upload_rgb_plane(cx, &planes, 2, next, w, h);
        self.planes = Some(planes);
        self.ai2_half = None;
        self.ai3_interval = None;
        self.prefetch = None;
        self.pair_key = None;
        self.flow_dirty = true;
        self.area.redraw(cx);
        true
    }

    /// Start or adopt an exact keyed pair. A ready speculative field is
    /// adopted only for its complete clip/pair/tier key; otherwise this is
    /// the classic full derive path.
    pub fn set_pair_keyed(
        &mut self,
        cx: &mut Cx,
        key: PairKey,
        a: &[u8],
        b: &[u8],
        width: u32,
        height: u32,
    ) {
        self.set_pair_inner(cx, Some(key), a, b, width, height);
    }

    fn set_pair_inner(
        &mut self,
        cx: &mut Cx,
        key: Option<PairKey>,
        a: &[u8],
        b: &[u8],
        width: u32,
        height: u32,
    ) {
        let (w, h) = (width as usize, height as usize);
        if w < 8 || h < 8 || a.len() < w * h * 3 / 2 || b.len() < w * h * 3 / 2 {
            return;
        }
        self.ai2_half = None;
        self.ai3_interval = None;
        let same_size = self.size == (width, height) && self.planes.is_some();
        let adopt = same_size
            && key.is_some_and(|wanted| {
                self.prefetch.as_ref().is_some_and(|ahead| ahead.ready_for(wanted))
            });
        if adopt {
            let ahead = self.prefetch.take().unwrap();
            let old = self.planes.take().unwrap();
            self.planes = Some(if ahead.forward {
                [
                    old[2].clone(), old[3].clone(), old[4].clone(),
                    old[5].clone(), old[0].clone(), old[1].clone(),
                ]
            } else {
                [
                    old[4].clone(), old[5].clone(), old[0].clone(),
                    old[1].clone(), old[2].clone(), old[3].clone(),
                ]
            });
            self.field_gen = ahead.target_gen;
            self.pair_key = key;
            self.flow_dirty = false;
            if tl_on() {
                eprintln!("tl tween prefetch-adopt {:?}", ahead.key);
            }
            self.area.redraw(cx);
            return;
        }

        if tl_on() && self.pair_key.is_some() {
            eprintln!("tl tween prefetch-miss {:?}", key);
        }
        self.prefetch = None;
        self.ensure_plane_ring(cx, width, height);
        let planes = self.planes.as_ref().unwrap();
        Self::upload_nv12_plane_pair(cx, planes, 0, a, w, h);
        Self::upload_nv12_plane_pair(cx, planes, 2, b, w, h);
        self.pair_key = key;
        self.flow_dirty = true;
        self.area.redraw(cx);
    }

    /// Adopt the exact RIFE midpoint frozen by the source pair's lease.
    /// It stays RGB and is sampled directly by the luma + warp passes.
    pub fn set_ai2_midpoint(&mut self, cx: &mut Cx, midpoint: &RifeMidpoint) -> bool {
        if !midpoint.is_valid() {
            self.ai2_midpoint_tex = None;
            self.ai2_midpoint_dims = (0, 0);
            self.ai2_half = None;
            return false;
        }
        let bgra = rgb8_to_bgra32(&midpoint.rgb);
        if self.ai2_midpoint_tex.is_none()
            || self.ai2_midpoint_dims != (midpoint.width, midpoint.height)
        {
            self.ai2_midpoint_tex = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: midpoint.width,
                    height: midpoint.height,
                    data: Some(bgra),
                    updated: TextureUpdated::Full,
                },
            ));
            self.ai2_midpoint_dims = (midpoint.width, midpoint.height);
        } else if let Some(texture) = &self.ai2_midpoint_tex {
            let _ = texture.take_vec_u32(cx);
            texture.put_back_vec_u32(cx, bgra, None);
        }
        self.rife_pair = None;
        self.area.redraw(cx);
        true
    }

    /// Select one of the two classical half-pairs. Crossing 0.5 changes the
    /// estimator's endpoints, so the ordinary FL stack derives that half.
    pub fn select_ai2_pair(&mut self, cx: &mut Cx, pair: Ai2Pair) {
        let half = match pair {
            Ai2Pair::FirstHalf | Ai2Pair::SecondHalf => Some(pair),
            Ai2Pair::Original => None,
        };
        if self.ai2_half != half {
            self.ai2_half = half;
            self.flow_dirty = self.planes.is_some();
            self.area.redraw(cx);
        }
    }

    pub fn clear_ai2_midpoint(&mut self, cx: &mut Cx) {
        if self.ai2_half.take().is_some() {
            self.flow_dirty = self.planes.is_some();
            self.area.redraw(cx);
        }
    }

    /// Freeze one complete AI3 level for this pair. A finer partial level is
    /// rejected here; the presenter has already selected its shallower lease.
    pub fn set_ai3_subdivision(
        &mut self,
        cx: &mut Cx,
        subdivision: &RifeSubdivision,
        depth: u8,
    ) -> bool {
        if !(AI3_MIN_DEPTH..=AI3_MAX_DEPTH).contains(&depth)
            || subdivision.complete_depth() < depth
        {
            self.clear_ai3_subdivision(cx);
            return false;
        }
        let count = ai3_neural_frames(depth);
        let Some(first) = subdivision.frame_for(depth, 1) else {
            self.clear_ai3_subdivision(cx);
            return false;
        };
        if !first.is_valid() {
            self.clear_ai3_subdivision(cx);
            return false;
        }
        let dims = (first.width, first.height);
        let rebuild = self.ai3_frames_dims != dims || self.ai3_frames_tex.len() != count;
        if rebuild {
            self.ai3_frames_tex.clear();
        }
        for k in 1..=count {
            let Some(frame) = subdivision.frame_for(depth, k) else {
                self.clear_ai3_subdivision(cx);
                return false;
            };
            if !frame.is_valid() || (frame.width, frame.height) != dims {
                self.clear_ai3_subdivision(cx);
                return false;
            }
            let bgra = rgb8_to_bgra32(&frame.rgb);
            if rebuild {
                self.ai3_frames_tex.push(Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: frame.width,
                        height: frame.height,
                        data: Some(bgra),
                        updated: TextureUpdated::Full,
                    },
                ));
            } else {
                let texture = &self.ai3_frames_tex[k - 1];
                let _ = texture.take_vec_u32(cx);
                texture.put_back_vec_u32(cx, bgra, None);
            }
        }
        self.ai3_frames_dims = dims;
        self.ai3_depth = depth;
        self.ai3_interval = None;
        self.ai2_half = None;
        self.rife_pair = None;
        self.area.redraw(cx);
        true
    }

    pub fn select_ai3_pair(&mut self, cx: &mut Cx, interval: usize) {
        let valid = self.ai3_depth > 0 && interval < 1usize << self.ai3_depth;
        let interval = valid.then_some(interval);
        if self.ai3_interval != interval {
            self.ai3_interval = interval;
            self.flow_dirty = self.planes.is_some();
            self.area.redraw(cx);
        }
    }

    pub fn clear_ai3_subdivision(&mut self, cx: &mut Cx) {
        if self.ai3_interval.take().is_some() || self.ai3_depth != 0 {
            self.ai3_depth = 0;
            self.flow_dirty = self.planes.is_some();
            self.area.redraw(cx);
        }
    }

    /// Offer the one new frame of the adjacent pair predicted along the
    /// platter map. The current field remains the warp/seed generation;
    /// this program writes only the other generation.
    pub fn offer_next(
        &mut self,
        cx: &mut Cx,
        key: PairKey,
        frame: &[u8],
        width: u32,
        height: u32,
        forward: bool,
    ) -> Option<usize> {
        let current = self.pair_key?;
        let (w, h) = (width as usize, height as usize);
        let valid_frame = w >= 8 && h >= 8 && frame.len() >= w * h * 3 / 2;
        let adjacent = if forward {
            current.b == key.a
        } else {
            current.a == key.b
        };
        if self.flow_dirty
            || !self.have_prev_field
            || self.fade
            || self.rgb_planes
            || self.size != (width, height)
            || !valid_frame
            || key == current
            || key.clip != current.clip
            || key.tier != current.tier
            || !adjacent
        {
            self.prefetch = None;
            return None;
        }
        if let Some(ahead) = self.prefetch.as_ref() {
            if ahead.key == key && ahead.forward == forward {
                return Some(ahead.remaining());
            }
        }
        self.prefetch = None;
        let planes = self.planes.as_ref()?;
        Self::upload_nv12_plane_pair(cx, planes, 4, frame, w, h);
        let ops = build_derive_ops(true, false);
        let remaining = ops.len();
        self.prefetch = Some(FieldPrefetch {
            key,
            forward,
            plane_y: if forward { [2, 4] } else { [4, 0] },
            target_gen: 1 - self.field_gen,
            seed_gen: self.field_gen,
            ops,
            cursor: 0,
        });
        self.area.redraw(cx);
        Some(remaining)
    }

    pub fn cancel_prefetch(&mut self) {
        self.prefetch = None;
        self.derive_budget = 0;
    }

    pub fn set_derive_budget(&mut self, cx: &mut Cx, budget: usize) {
        self.derive_budget = budget.min(FIELD_PREFETCH_OPS_PER_FRAME);
        if self.derive_budget > 0 && self.prefetch.as_ref().is_some_and(|p| p.remaining() > 0) {
            self.area.redraw(cx);
        }
    }

    pub fn set_t(&mut self, cx: &mut Cx, t: f32) {
        let t = t.clamp(0.0, 1.0);
        if (self.t - t).abs() > 1e-4 {
            self.t = t;
            self.area.redraw(cx);
        }
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.planes = None;
        self.rgb_planes = false;
        self.size = (0, 0);
        self.flow_dirty = false;
        self.pair_key = None;
        self.prefetch = None;
        self.derive_budget = 0;
        self.field_gen = 0;
        self.rendered = false;
        self.rife_pair = None;
        self.ai2_half = None;
        self.ai2_midpoint_tex = None;
        self.ai2_midpoint_dims = (0, 0);
        self.ai3_frames_tex.clear();
        self.ai3_frames_dims = (0, 0);
        self.ai3_depth = 0;
        self.ai3_interval = None;
        self.have_prev_field = false;
        self.cut = false;
        self.area.redraw(cx);
    }

    /// Mark the current pair as a hard cut (see `cut`).
    pub fn set_cut(&mut self, cx: &mut Cx, cut: bool) {
        if self.cut != cut {
            self.cut = cut;
            self.area.redraw(cx);
        }
    }

    /// FADE mode on/off (see `fade`).
    pub fn set_fade(&mut self, cx: &mut Cx, fade: bool) {
        if self.fade != fade {
            self.fade = fade;
            self.area.redraw(cx);
        }
    }

    /// Drop any standing neural field (the deck left AI mode).
    pub fn clear_rife_field(&mut self, cx: &mut Cx) {
        if self.rife_pair.take().is_some() {
            self.area.redraw(cx);
        }
    }

    /// Adopt a neural field for `pair` (RGBA-interleaved intermediate
    /// flow + mask at proxy resolution).
    pub fn set_rife_field(
        &mut self,
        cx: &mut Cx,
        pair: usize,
        width: usize,
        height: usize,
        flow: &[f32],
        mask: &[f32],
    ) {
        if flow.len() < width * height * 4 || mask.len() < width * height {
            return;
        }
        if self.rife_tex.is_none() || self.rife_dims != (width, height) {
            self.rife_tex = Some((
                Texture::new_with_format(
                    cx,
                    TextureFormat::VecRGBAf32 {
                        width,
                        height,
                        data: Some(flow.to_vec()),
                        updated: TextureUpdated::Full,
                    },
                ),
                Texture::new_with_format(
                    cx,
                    TextureFormat::VecRu8 {
                        width,
                        height,
                        data: Some(mask.iter().map(|m| (m * 255.0) as u8).collect()),
                        unpack_row_length: None,
                        updated: TextureUpdated::Full,
                    },
                ),
            ));
            self.rife_dims = (width, height);
        } else if let Some((flow_tex, mask_tex)) = &self.rife_tex {
            let mut buf = flow_tex.take_vec_f32(cx);
            buf.clear();
            buf.extend_from_slice(&flow[..width * height * 4]);
            flow_tex.put_back_vec_f32(cx, buf, None);
            let mut mb = mask_tex.take_vec_u8(cx);
            mb.clear();
            mb.extend(mask.iter().map(|m| (m * 255.0) as u8));
            mask_tex.put_back_vec_u8(cx, mb, None);
        }
        self.rife_pair = Some(pair);
        self.ai2_half = None;
        self.rife_age = 0;
        self.area.redraw(cx);
    }

    /// The pair the tween view is currently showing (for job scheduling).
    pub fn rife_field_pair(&self) -> Option<usize> {
        self.rife_pair
    }

    /// A pair advanced without a fresh neural field: REUSE the standing
    /// one for a few pairs (consistent producer > perfectly fresh field);
    /// only after that fall back to the classical fields.
    pub fn age_rife_field(&mut self, cx: &mut Cx, pair: usize) {
        const MAX_REUSE_PAIRS: u32 = 4;
        if self.rife_pair.is_none() {
            return;
        }
        self.rife_age += 1;
        if self.rife_age > MAX_REUSE_PAIRS {
            self.rife_pair = None;
            self.area.redraw(cx);
        } else {
            self.rife_pair = Some(pair);
        }
    }

    pub fn has_pair(&self) -> bool {
        self.planes.is_some()
    }

    /// A neural midpoint is standing for the current pair (AI2's plan turns
    /// on this, not on whether a job was ever offered).
    pub fn has_ai2_midpoint(&self) -> bool {
        self.ai2_midpoint_tex.is_some()
    }

    /// The complete neural subdivision depth currently frozen for this pair.
    pub fn ai3_depth(&self) -> u8 {
        self.ai3_depth
    }

    /// The endpoint ring holds packed RGB rather than NV12 planes.
    pub fn is_rgb(&self) -> bool {
        self.rgb_planes
    }

    /// Selftest control of the warp's debug view (0 off / 1 field / 2
    /// frame A / 4 luma). Also re-runs the flow stack so a luma-only
    /// debug frame can be followed by a full one.
    pub fn set_debug(&mut self, cx: &mut Cx, v: f32) {
        if self.dbg_override != Some(v) {
            self.dbg_override = Some(v);
            self.flow_dirty = self.planes.is_some();
            self.area.redraw(cx);
        }
    }

    /// Selftest access to the intermediate targets by name.
    pub fn debug_texture(&self, which: &str) -> Option<Texture> {
        match which {
            "luma0" => self.luma_tex.first().cloned(),
            "luma1" => self.luma_tex.get(1).cloned(),
            "luma2" => self.luma_tex.get(2).cloned(),
            "luma_top" => self.luma_tex.last().cloned(),
            "seed" => self.scratch.get(2).cloned(),
            "fwd" => self.field_tex.get(self.field_gen * 2).cloned(),
            "bwd" => self.field_tex.get(self.field_gen * 2 + 1).cloned(),
            _ => None,
        }
    }

    pub fn output_texture(&self) -> Option<Texture> {
        if self.rendered {
            Some(self.warp_out.clone())
        } else {
            None
        }
    }

    /// Grid dims at level 0 (quarter of source).
    fn grid(&self) -> (usize, usize) {
        (
            ((self.size.0 as usize) / 4).max(1),
            ((self.size.1 as usize) / 4).max(1),
        )
    }

    fn ensure_targets(&mut self, cx: &mut Cx) {
        if self.target_size == self.size {
            return;
        }
        self.target_size = self.size;
        self.rendered = false;
        let float_tex = |cx: &mut Cx| {
            Texture::new_with_format(
                cx,
                TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true },
            )
        };
        self.luma_tex = (0..LEVELS).map(|_| float_tex(cx)).collect();
        self.scratch = (0..3).map(|_| float_tex(cx)).collect();
        self.field_tex = (0..4).map(|_| float_tex(cx)).collect();
        self.warp_out = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: self.size.0 as usize,
                    height: self.size.1 as usize,
                },
                initial: true,
            },
        );
        // Stage pool: pyramid (LEVELS) + per direction (1 exhaustive +
        // LEVELS*(SWEEPS+1) + 1 subpel) + warp.
        let per_dir = 1 + LEVELS * (SWEEPS + 1) + 1;
        let want = LEVELS + 2 * per_dir + 1;
        while self.stages.len() < want {
            self.stages.push(Stage {
                pass: DrawPass::new(cx),
                draw_list: DrawList::new(cx),
            });
        }
    }

    /// Level dims: L0 = grid, halving with a floor of 8 cells.
    fn level_dims(&self, level: usize) -> (usize, usize) {
        let (gw, gh) = self.grid();
        ((gw >> level).max(8), (gh >> level).max(8))
    }
}

impl WidgetNode for FlowTweenView {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for FlowTweenView {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.walk_turtle_with_area(&mut self.area, walk);
        let Some(endpoint_planes) = self.planes.clone() else {
            return DrawStep::done();
        };
        // A packed-RGB ring makes the endpoints themselves rgb_*_on; the
        // neural midpoints below are RGB whatever the endpoints are.
        let endpoint_rgb = if self.rgb_planes { 1.0f32 } else { 0.0f32 };
        let ai3_pair = self.ai3_interval.and_then(|interval| {
            let intervals = 1usize << self.ai3_depth;
            if interval >= intervals || self.ai3_frames_tex.len() + 1 != intervals {
                return None;
            }
            let (a_y, a_uv, rgb_a_on) = if interval == 0 {
                (endpoint_planes[0].clone(), endpoint_planes[1].clone(), endpoint_rgb)
            } else {
                let texture = self.ai3_frames_tex[interval - 1].clone();
                (texture.clone(), texture, 1.0)
            };
            let (b_y, b_uv, rgb_b_on) = if interval + 1 == intervals {
                (endpoint_planes[2].clone(), endpoint_planes[3].clone(), endpoint_rgb)
            } else {
                let texture = self.ai3_frames_tex[interval].clone();
                (texture.clone(), texture, 1.0)
            };
            Some((
                [
                    a_y,
                    a_uv,
                    b_y,
                    b_uv,
                    endpoint_planes[4].clone(),
                    endpoint_planes[5].clone(),
                ],
                rgb_a_on,
                rgb_b_on,
            ))
        });
        let (planes, rgb_a_on, rgb_b_on) = ai3_pair.unwrap_or_else(|| {
            let midpoint = self.ai2_midpoint_tex.clone();
            match (self.ai2_half, midpoint) {
                (Some(Ai2Pair::FirstHalf), Some(midpoint)) => (
                    [
                        endpoint_planes[0].clone(),
                        endpoint_planes[1].clone(),
                        midpoint.clone(),
                        midpoint,
                        endpoint_planes[4].clone(),
                        endpoint_planes[5].clone(),
                    ],
                    endpoint_rgb,
                    1.0,
                ),
                (Some(Ai2Pair::SecondHalf), Some(midpoint)) => (
                    [
                        midpoint.clone(),
                        midpoint,
                        endpoint_planes[2].clone(),
                        endpoint_planes[3].clone(),
                        endpoint_planes[4].clone(),
                        endpoint_planes[5].clone(),
                    ],
                    1.0,
                    endpoint_rgb,
                ),
                _ => (endpoint_planes, endpoint_rgb, endpoint_rgb),
            }
        });
        self.ensure_targets(cx.cx);
        let (gw, gh) = self.grid();
        let mut stage = 0usize;
        // Stages used THIS frame, in submission order. Sibling child
        // passes do NOT render in creation order — the gpu_lightmap baker
        // law — so each stage is parented to the NEXT one (a child pass
        // renders before its parent) and only the last hangs off the
        // window pass.
        let dbg = self.dbg_override.unwrap_or_else(tween_debug);
        let luma_only = dbg > 3.5;
        let fade = self.fade;
        let mut derive_ops = Vec::new();
        let mut derive_plane_y = [0usize, 2usize];
        let mut derive_seed_gen = self.field_gen;
        let mut derive_target_gen = self.field_gen;
        let mut derive_seeded = self.have_prev_field;
        let mut full_derive = false;
        if self.flow_dirty && !fade {
            full_derive = true;
            self.flow_dirty = false;
            self.prefetch = None;
            derive_seed_gen = self.field_gen;
            derive_target_gen = 1 - self.field_gen;
            self.field_gen = derive_target_gen;
            derive_ops = build_derive_ops(derive_seeded, luma_only);
        } else if !fade && self.derive_budget > 0 {
            if let Some(ahead) = self.prefetch.as_mut() {
                let end = (ahead.cursor + self.derive_budget).min(ahead.ops.len());
                derive_ops.extend_from_slice(&ahead.ops[ahead.cursor..end]);
                ahead.cursor = end;
                derive_plane_y = ahead.plane_y;
                derive_seed_gen = ahead.seed_gen;
                derive_target_gen = ahead.target_gen;
                derive_seeded = true;
            }
        }
        self.derive_budget = 0;
        let total = derive_ops.len() + 1;
        // One offscreen stage: bind target, run one full-target quad of
        // `draw`, sized to (w, h) at dpi 1 (the flow-warp recipe: assert
        // the size again after begin_pass or the texture takes the
        // window's rect).
        macro_rules! run_stage {
            ($target:expr, $w:expr, $h:expr, $draw:expr) => {{
                let size = dvec2($w as f64, $h as f64);
                let chain_parent = if stage + 1 < total {
                    Some(self.stages[stage + 1].pass.draw_pass_id())
                } else {
                    None
                };
                {
                    let st = &mut self.stages[stage];
                    st.pass.set_size(cx, size);
                    st.pass.clear_color_textures(cx.cx);
                    st.pass.set_color_texture(
                        cx,
                        $target,
                        DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
                    );
                    match chain_parent {
                        Some(parent_id) => {
                            let child_id = st.pass.draw_pass_id();
                            cx.cx.passes[child_id].parent =
                                CxDrawPassParent::DrawPass(parent_id);
                        }
                        None => cx.make_child_pass(&st.pass),
                    }
                    if std::env::var_os("VJ_TWEEN_SELFTEST").is_some() {
                        eprintln!(
                            "tween stage {} pass={:?} parent={:?} target={:?} size={}x{}",
                            stage,
                            st.pass.draw_pass_id(),
                            chain_parent,
                            $target.texture_id(),
                            $w,
                            $h
                        );
                    }
                    cx.begin_pass(&st.pass, Some(1.0));
                    st.pass.set_size(cx, size);
                    st.pass.set_dpi_factor(cx, 1.0);
                    st.draw_list.begin_always(cx);
                }
                // PASS-LOCAL TURTLE (the fx_slot recipe): without it the
                // quad records inside the WIDGET's turtle and inherits its
                // tiny on-screen clip — every stage silently lost a strip
                // of rows (invisible on a video-sized target, fatal on a
                // 16x16 pyramid top).
                let pass_size = cx.current_pass_size();
                cx.begin_root_turtle(pass_size, Layout::flow_overlay());
                $draw(cx, Rect { pos: dvec2(0.0, 0.0), size });
                cx.end_pass_sized_turtle();
                {
                    let st = &mut self.stages[stage];
                    st.draw_list.end(cx);
                    cx.end_pass(&st.pass);
                }
                stage += 1;
                _ = stage;
            }};
        }
        for op in derive_ops {
            match op {
                DeriveOp::Luma0 => {
                    let (w, h) = (self.size.0 as usize, self.size.1 as usize);
                    self.draw_luma.inv_grid = vec2(1.0 / w as f32, 1.0 / h as f32);
                    self.draw_luma.rgb_a_on = rgb_a_on;
                    self.draw_luma.rgb_b_on = rgb_b_on;
                    self.draw_luma.draw_vars.set_texture(0, &planes[derive_plane_y[0]]);
                    self.draw_luma.draw_vars.set_texture(1, &planes[derive_plane_y[1]]);
                    let target = self.luma_tex[0].clone();
                    let draw = &mut self.draw_luma;
                    run_stage!(&target, gw, gh, |cx: &mut Cx2d, r| draw.draw_abs(cx, r));
                }
                DeriveOp::Halve { level } => {
                    let (lw, lh) = self.level_dims(level);
                    let src = if std::env::var_os("TWEEN_PYR_FROM_L0").is_some() {
                        self.luma_tex[0].clone()
                    } else {
                        self.luma_tex[level - 1].clone()
                    };
                    let target = self.luma_tex[level].clone();
                    self.draw_halve.draw_vars.set_texture(0, &src);
                    let draw = &mut self.draw_halve;
                    run_stage!(&target, lw, lh, |cx: &mut Cx2d, r| draw.draw_abs(cx, r));
                }
                DeriveOp::LumaField => {
                    let src = self.luma_tex[0].clone();
                    let target = self.field_tex[derive_target_gen * 2].clone();
                    self.draw_halve.draw_vars.set_texture(0, &src);
                    let draw = &mut self.draw_halve;
                    run_stage!(&target, gw, gh, |cx: &mut Cx2d, r| draw.draw_abs(cx, r));
                }
                DeriveOp::Exhaust { dir } => {
                    let (tw, th) = self.level_dims(LEVELS - 1);
                    let top_luma = self.luma_tex[LEVELS - 1].clone();
                    self.draw_exhaust.dir = dir as f32;
                    self.draw_exhaust.inv_size = vec2(1.0 / tw as f32, 1.0 / th as f32);
                    self.draw_exhaust.draw_vars.set_texture(0, &top_luma);
                    let target = self.scratch[2].clone();
                    let draw = &mut self.draw_exhaust;
                    run_stage!(&target, tw, th, |cx: &mut Cx2d, r| draw.draw_abs(cx, r));
                }
                DeriveOp::Sweep { dir, level, sweep } => {
                    let (lw, lh) = self.level_dims(level);
                    let (tw, _) = self.level_dims(LEVELS - 1);
                    let luma = self.luma_tex[level].clone();
                    let (coarse, mean_on) = if level + 1 < LEVELS {
                        (self.luma_tex[level + 1].clone(), 1.0f32)
                    } else {
                        (luma.clone(), 0.0f32)
                    };
                    let (prev, prev_scale) = if sweep > 0 {
                        (self.scratch[(sweep - 1) & 1].clone(), 1.0)
                    } else if level + 1 < LEVELS {
                        (self.scratch[2].clone(), 2.0)
                    } else if derive_seeded {
                        (
                            self.field_tex[derive_seed_gen * 2 + dir].clone(),
                            tw as f32 / gw.max(1) as f32,
                        )
                    } else {
                        (self.scratch[2].clone(), 1.0)
                    };
                    let target = self.scratch[sweep & 1].clone();
                    self.draw_sweep.dir = dir as f32;
                    self.draw_sweep.inv_size = vec2(1.0 / lw as f32, 1.0 / lh as f32);
                    self.draw_sweep.prev_scale = prev_scale;
                    self.draw_sweep.mean_on = mean_on;
                    self.draw_sweep.draw_vars.set_texture(0, &luma);
                    self.draw_sweep.draw_vars.set_texture(1, &prev);
                    self.draw_sweep.draw_vars.set_texture(2, &coarse);
                    let draw = &mut self.draw_sweep;
                    run_stage!(&target, lw, lh, |cx: &mut Cx2d, r| draw.draw_abs(cx, r));
                }
                DeriveOp::Median { dir: _, level } => {
                    let (lw, lh) = self.level_dims(level);
                    let prev = self.scratch[(SWEEPS - 1) & 1].clone();
                    let target = self.scratch[2].clone();
                    self.draw_median.inv_size = vec2(1.0 / lw as f32, 1.0 / lh as f32);
                    self.draw_median.draw_vars.set_texture(0, &prev);
                    let draw = &mut self.draw_median;
                    run_stage!(&target, lw, lh, |cx: &mut Cx2d, r| draw.draw_abs(cx, r));
                }
                DeriveOp::Subpel { dir } => {
                    let luma0 = self.luma_tex[0].clone();
                    let prev = self.scratch[2].clone();
                    let target = self.field_tex[derive_target_gen * 2 + dir].clone();
                    self.draw_subpel.dir = dir as f32;
                    self.draw_subpel.inv_size = vec2(1.0 / gw as f32, 1.0 / gh as f32);
                    self.draw_subpel.draw_vars.set_texture(0, &luma0);
                    self.draw_subpel.draw_vars.set_texture(1, &prev);
                    let draw = &mut self.draw_subpel;
                    run_stage!(&target, gw, gh, |cx: &mut Cx2d, r| draw.draw_abs(cx, r));
                }
            }
        }
        if full_derive {
            self.have_prev_field = !luma_only;
        }
        // ---- the warp, every display frame ------------------------------
        let (w, h) = (self.size.0, self.size.1);
        self.draw_warp.dbg = self.dbg_override.unwrap_or_else(tween_debug);
        self.draw_warp.rgb_a_on = rgb_a_on;
        self.draw_warp.rgb_b_on = rgb_b_on;
        // A CUT pair never morphs: snap to the nearest endpoint (t 0/1
        // samples that frame exactly through either producer's math). A
        // crossfade across a cut is fine — no snap in FADE mode.
        self.draw_warp.fade_on = if fade { 1.0 } else { 0.0 };
        self.draw_warp.t_pair = if self.cut && !fade {
            if self.t < 0.5 { 0.0 } else { 1.0 }
        } else {
            self.t
        };
        match (&self.rife_tex, self.rife_pair) {
            (Some((flow_tex, mask_tex)), Some(_)) => {
                self.draw_warp.rife_on = 1.0;
                self.draw_warp.rife_inv = vec2(
                    1.0 / self.rife_dims.0.max(1) as f32,
                    1.0 / self.rife_dims.1.max(1) as f32,
                );
                self.draw_warp.draw_vars.set_texture(6, flow_tex);
                self.draw_warp.draw_vars.set_texture(7, mask_tex);
            }
            _ => {
                self.draw_warp.rife_on = 0.0;
                // Bind SOMETHING valid in the rife slots (the field
                // textures double up) so no backend sees an empty slot.
                self.draw_warp
                    .draw_vars
                    .set_texture(6, &self.field_tex[self.field_gen * 2]);
                self.draw_warp
                    .draw_vars
                    .set_texture(7, &self.field_tex[self.field_gen * 2 + 1]);
            }
        }
        self.draw_warp.inv_grid = vec2(1.0 / gw as f32, 1.0 / gh as f32);
        self.draw_warp.draw_vars.set_texture(0, &planes[0]);
        self.draw_warp.draw_vars.set_texture(1, &planes[1]);
        self.draw_warp.draw_vars.set_texture(2, &planes[2]);
        self.draw_warp.draw_vars.set_texture(3, &planes[3]);
        self.draw_warp
            .draw_vars
            .set_texture(4, &self.field_tex[self.field_gen * 2]);
        self.draw_warp
            .draw_vars
            .set_texture(5, &self.field_tex[self.field_gen * 2 + 1]);
        let warp_out = self.warp_out.clone();
        let draw_warp = &mut self.draw_warp;
        run_stage!(&warp_out, w, h, |cx: &mut Cx2d, r| draw_warp.draw_abs(cx, r));
        self.rendered = true;
        DrawStep::done()
    }
}
