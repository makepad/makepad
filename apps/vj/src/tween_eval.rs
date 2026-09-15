//! OFFLINE ERROR MEASUREMENT for the realtime GPU frame tweener.
//!
//! DEBUG RIG — never committed, never part of a build the operator runs.
//!
//! `flow_tween.rs` synthesizes in-betweens on the GPU. Judging one by eye
//! tells you it "looks wrong"; it does not tell you whether a change made
//! it better. This harness turns the judgement into numbers: it plays a
//! synthetic clip whose correct in-between is known ANALYTICALLY, asks the
//! tweener for every fractional t of every pair, reads the warp target
//! back, and scores the difference.
//!
//! The clip (`libs/video_flow/src/bin/flowtest_gen.rs`, `sine_ease`) is a
//! white 64px square on black at a subpixel-exact position; at continuous
//! frame time f its position is a closed form, so the reference frame at
//! t = 0.37 of pair 12 is as computable as any source frame. NOTHING here
//! feeds that knowledge to the tweener — the model is used only for
//! scoring, after the readback.
//!
//! Four numbers per (pair, t), all on luma:
//!   mean_abs / max_abs  — the whole frame
//!   hole_err            — black eating INTO the square (the reported bug)
//!   halo_err            — the +-3px band around the analytic edge
//!   bg_err              — white smearing OUT onto the background
//!
//! t = 0 and t = 1 are measured too and are the CALIBRATION: at the
//! endpoints the warp must reproduce the source frame exactly, so their
//! error is the codec/pipeline floor. A floor that is not near zero means
//! the rig is wrong (range, channel order, frame indexing, readback lag)
//! and no in-between number from that run means anything.
//!
//! ```text
//! VJ_TWEEN_EVAL=/abs/path/clip.mp4 ./target/release/makepad-vj --remote
//! # -> /tmp/tween_eval/{eval.csv,summary.txt,out_*.png,ref_*.png,diff_*.png}
//! ```
//!
//! `VJ_TWEEN_EVAL_SETTLE=n` (default 1) is how many extra display frames
//! to let a state settle before the readback; `VJ_TWEEN_EVAL_OUT` moves
//! the output directory.

use makepad_widgets::*;
use std::cell::RefCell;

use crate::cue::SlotId;

/// The fractional positions probed inside every pair. 0 and 1 are the
/// floor, the nine between them are what is being measured.
const TS: [f32; 11] = [0.0, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];

/// How thick the erode/dilate bands around the analytic rect are.
const BAND: f32 = 3.0;

/// How many worst cases keep their picture for a PNG dump.
const KEEP: usize = 12;
/// How many of those actually get written.
const DUMP: usize = 10;

/// `VJ_TWEEN_EVAL=/abs/path/clip.mp4` turns the app into the rig.
pub fn clip_path() -> Option<String> {
    static P: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::env::var("VJ_TWEEN_EVAL").ok().filter(|v| !v.is_empty()))
        .clone()
}

/// Either rig arms the app: the analytic sweep (`VJ_TWEEN_EVAL`) or the
/// real-footage held-out reconstruction (`VJ_TWEEN_EVAL_REAL`). REAL wins
/// if both are set — one process, one measurement.
pub fn enabled() -> bool {
    clip_path().is_some() || real_clip_path().is_some()
}

fn settle_frames() -> u32 {
    static S: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *S.get_or_init(|| {
        std::env::var("VJ_TWEEN_EVAL_SETTLE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
    })
}

fn out_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("VJ_TWEEN_EVAL_OUT").unwrap_or_else(|_| "/tmp/tween_eval".to_string()),
    )
}

/// `VJ_TWEEN_EVAL_STRIDE=N` walks pairs 0, N, 2N, ... instead of every
/// one — the FAST-PLAYBACK case, where the transport skips pairs and the
/// standing flow field is a stale seed for the pair it lands on. At N > 1
/// the rig calls `reset_seed()` before every `set_pair`, exactly as the
/// app's pump does on a non-contiguous jump, so the numbers describe the
/// player's real behaviour and not a state it never reaches.
///
/// `VJ_TWEEN_EVAL_DUMP_PAIR=P` writes the WHOLE level-0 field for pair `P`
/// (both directions, one row per grid cell) to `field_pP.csv`. The
/// aggregate rows in `fields.csv` say the field is wrong by so many cells
/// on average; only the raw grid says WHERE and in which direction, which
/// is the difference between "vertical error" and a named mechanism.
/// Accepts a comma-separated list, so ONE run can show how a field cell
/// evolves across pairs — the only way to tell a per-pair estimation error
/// from a residue the temporal seed carries forward forever.
fn dump_pairs() -> &'static [usize] {
    static P: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        std::env::var("VJ_TWEEN_EVAL_DUMP_PAIR")
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or_default()
    })
}

/// `VJ_TWEEN_EVAL_EXCL_PAIRS=a,b` drops those pairs from the IN-BETWEENS
/// statistics and from the worst-case ranking. Its one legitimate use is
/// a pair whose correct answer is not representable: `rect_diagonal`
/// reverses direction at f = 36.73, so pairs 36 and 37 ask a two-frame
/// tweener to reproduce a V from its two endpoints — below temporal
/// Nyquist, and no field can fix it. Excluded pairs are still measured,
/// still written to eval.csv, and still reported in their own block, so
/// nothing is hidden; they just stop crowding the ranking of the failures
/// that ARE fixable.
fn excl_pairs() -> &'static [usize] {
    static P: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        std::env::var("VJ_TWEEN_EVAL_EXCL_PAIRS")
            .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or_default()
    })
}

// ---------------------------------------------------------------------------
// THE NEURAL TIER — an AI mode for both rigs
// ---------------------------------------------------------------------------
//
// `VJ_TWEEN_EVAL_AI=1` drives the RIFE engine SYNCHRONOUSLY inside the eval
// loop and hands its field to the view through `set_rife_field`, which is
// the same call the pump makes when it adopts a worker result. No worker,
// no channel, no newest-job-only race: the same clip run twice returns the
// same numbers, which is the whole point of a rig.
//
// TWO CADENCES, one knob each, and between them they span every state the
// realtime path can be in:
//
//   VJ_TWEEN_EVAL_AI_STRIDE=k   a FRESH field every k-th pair. k = 1 is the
//                               net's intrinsic ceiling (every pair gets
//                               its own field); k > 1 replicates a worker
//                               that cannot keep up, with `age_rife_field`
//                               re-stamping the standing field in between
//                               exactly as `main.rs` does.
//   VJ_TWEEN_RIFE_REUSE=r       how many pairs a field survives before the
//                               CLASSICAL producer takes over (the shipped
//                               MAX_REUSE_PAIRS, default 4).
//
// The pair of knobs is deliberately a 2x2 over the two mechanisms that can
// make a neural tier discontinuous, so each can be measured WITHOUT the
// other:
//
//   k=2, r=4      the shipped cadence           stale + never alternates
//   k=2, r=0      classical between fresh ones  alternates, never stale
//   k=huge, r=huge one field frozen forever     stale, no handoff at all
//   k=1           fresh every pair              neither
//
/// `VJ_TWEEN_EVAL_AI=1|2` — which NEURAL tier the measured view runs.
/// 1 is AI1, the raw t = 0.5 field driving the warp for every t; 2 is AI2,
/// frame doubling with the classical stack on each half-pair.
fn eval_ai_tier() -> crate::flow_tween::AiTier {
    use crate::flow_tween::AiTier;
    static S: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    match *S.get_or_init(|| match std::env::var("VJ_TWEEN_EVAL_AI").as_deref() {
        Ok("1") => 1,
        Ok("2") => 2,
        Ok("3") => 3,
        _ => 0,
    }) {
        1 => AiTier::Field,
        2 => AiTier::Hybrid,
        3 => AiTier::Quad,
        _ => AiTier::Off,
    }
}

fn eval_ai() -> bool {
    eval_ai_tier() != crate::flow_tween::AiTier::Off
}

/// A fresh neural field every k-th scored pair (1 = every pair).
fn eval_ai_stride() -> usize {
    static S: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *S.get_or_init(|| {
        std::env::var("VJ_TWEEN_EVAL_AI_STRIDE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v >= 1)
            .unwrap_or(1)
    })
}

/// `VJ_TWEEN_EVAL_AI_LAG=L` — the field handed to pair `n` is computed
/// from the frames of pair `n - L`.
///
/// This is the realtime path's REAL geometry, and it is not the same thing
/// as the stride. A worker that takes 90 ms on a 40 ms pair does not skip
/// pairs at random: it finishes every job it starts, LATE, so its output
/// describes frames that are already two or three pairs behind the picture
/// being drawn. `main.rs` currently answers that by refusing the field
/// outright (`field.pair == pair`), which sends the pair to the classical
/// producer; the alternative is to adopt it at its true age, which is
/// precisely the state `age_rife_field` already blesses for four pairs.
/// The two policies are a measurable question, and this knob is the half
/// of it the rig can synthesize.
fn eval_ai_lag() -> usize {
    static S: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *S.get_or_init(|| {
        std::env::var("VJ_TWEEN_EVAL_AI_LAG")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
    })
}

/// Which producer served one scored sample. The realtime path can be in
/// any of these on any given pair, and the whole discontinuity question is
/// what happens at the boundaries BETWEEN them.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AiTier {
    /// Not an AI run at all.
    Off,
    /// A field computed from THIS pair's own two frames.
    Fresh,
    /// The field of a pair `k` back, re-stamped onto this one.
    Aged(u32),
    /// The neural field expired; the classical stack is driving.
    Classical,
}

impl AiTier {
    fn tag(self) -> String {
        match self {
            AiTier::Off => "-".into(),
            AiTier::Fresh => "fresh".into(),
            AiTier::Aged(k) => format!("aged{k}"),
            AiTier::Classical => "classical".into(),
        }
    }
    fn is_neural(self) -> bool {
        matches!(self, AiTier::Fresh | AiTier::Aged(_))
    }
}

/// The synchronous engine, built once per process on the thread the pump
/// runs on. An Err is cached too: a missing checkpoint must fail the run
/// loudly and once, not once per pair.
fn rife_engine<R>(f: impl FnOnce(&makepad_ai_rife::rife::Rife) -> R) -> Result<R, String> {
    thread_local! {
        static ENGINE: RefCell<Option<Result<makepad_ai_rife::rife::Rife, String>>> =
            const { RefCell::new(None) };
    }
    ENGINE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let path = crate::rife_model_path();
            let built = crate::flow_tween::rife_sync_engine(&path);
            match &built {
                Ok(_) => log!("TWEEN_EVAL AI: RIFE engine up ({})", path.display()),
                Err(e) => log!("TWEEN_EVAL AI: RIFE unavailable: {e}"),
            }
            *slot = Some(built);
        }
        match slot.as_ref().unwrap() {
            Ok(rife) => Ok(f(rife)),
            Err(e) => Err(e.clone()),
        }
    })
}

/// One pair's neural field, in the interleaved layout the warp texture
/// wants, plus the wall time the forward took.
fn rife_field(
    nv12_a: &[u8],
    nv12_b: &[u8],
    w: usize,
    h: usize,
) -> Result<(usize, usize, Vec<f32>, Vec<f32>, f64), String> {
    let (pw, ph) = crate::flow_tween::rife_proxy_dims(w as u32, h as u32);
    let rgb0 = crate::media::nv12_proxy_rgb8(nv12_a, w, h, pw, ph);
    let rgb1 = crate::media::nv12_proxy_rgb8(nv12_b, w, h, pw, ph);
    let out = rife_engine(|rife| -> Result<(Vec<f32>, Vec<f32>, f64), String> {
        use makepad_ai_rife::rife::RifeFramePair;
        let pair = RifeFramePair::new(&rgb0, &rgb1, pw, ph)
            .map_err(|e| format!("rife pair: {e:?}"))?;
        let t0 = crate::clock::Instant::now();
        let field = rife
            .flow_field_rgb8(pair, 0.5, None)
            .map_err(|e| format!("rife forward: {e:?}"))?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok((
            crate::flow_tween::rife_pack_flow(&field.flow, pw * ph),
            field.mask,
            ms,
        ))
    })??;
    Ok((pw, ph, out.0, out.1, out.2))
}

/// The synthesized LADDER for one pair, NV12 at the video's own size, in
/// interior order — the same forwards the worker makes, on the calling
/// thread so a run is reproducible. `depth` 1 gives `[M]` (AI2), 2 gives
/// `[M1, M, M2]` (AI3).
fn rife_midpoint(
    nv12_a: &[u8],
    nv12_b: &[u8],
    w: usize,
    h: usize,
    depth: u8,
) -> Result<(Vec<Vec<u8>>, f64), String> {
    let (sw, sh) = crate::flow_tween::rife_synth_dims(w as u32, h as u32);
    let rgb0 = crate::media::nv12_proxy_rgb8(nv12_a, w, h, sw, sh);
    let rgb1 = crate::media::nv12_proxy_rgb8(nv12_b, w, h, sw, sh);
    rife_engine(|rife| -> Result<(Vec<Vec<u8>>, f64), String> {
        use makepad_ai_rife::rife::RifeFramePair;
        let synth = |a: &[u8], b: &[u8]| -> Result<Vec<u8>, String> {
            let p = RifeFramePair::new(a, b, sw, sh)
                .map_err(|e| format!("rife mid pair: {e:?}"))?;
            rife.interpolate_rgb8(p, 0.5)
                .map_err(|e| format!("rife interpolate: {e:?}"))
        };
        let t0 = crate::clock::Instant::now();
        let mid = synth(&rgb0, &rgb1)?;
        let rungs = if depth >= 2 {
            vec![synth(&rgb0, &mid)?, mid.clone(), synth(&mid, &rgb1)?]
        } else {
            vec![mid]
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok((
            rungs
                .iter()
                .map(|r| crate::flow_tween::rgb8_to_nv12_scaled(r, sw, sh, w, h))
                .collect(),
            ms,
        ))
    })?
}

/// N = 1 (the default) is the original contiguous walk, untouched.
fn stride() -> usize {
    static S: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *S.get_or_init(|| {
        std::env::var("VJ_TWEEN_EVAL_STRIDE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v >= 1)
            .unwrap_or(1)
    })
}

// ---------------------------------------------------------------------------
// The analytic reference — flowtest_gen's coverage math, verbatim
// ---------------------------------------------------------------------------

#[inline]
fn span_overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).clamp(0.0, 1.0)
}

/// One grayscale frame in display units, drawn by exact per-pixel area
/// coverage — the same Canvas the generator encoded from.
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<f32>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Self { w, h, px: vec![0.0; w * h] }
    }

    fn clear(&mut self, v: f32) {
        self.px.fill(v);
    }

    #[inline]
    fn blend(&mut self, x: usize, y: usize, v: f32, cov: f32) {
        if cov <= 0.0 {
            return;
        }
        let c = cov.min(1.0);
        let p = &mut self.px[y * self.w + x];
        *p = *p * (1.0 - c) + v * c;
    }

    /// `[x0,x1) x [y0,y1)` in float pixel coordinates, exact coverage.
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, v: f32) {
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (w, h) = (self.w as i64, self.h as i64);
        let ix0 = (x0.floor() as i64).clamp(0, w) as usize;
        let ix1 = (x1.ceil() as i64).clamp(0, w) as usize;
        let iy0 = (y0.floor() as i64).clamp(0, h) as usize;
        let iy1 = (y1.ceil() as i64).clamp(0, h) as usize;
        for y in iy0..iy1 {
            let cy = span_overlap(y as f32, y as f32 + 1.0, y0, y1);
            if cy <= 0.0 {
                continue;
            }
            for x in ix0..ix1 {
                let cx = span_overlap(x as f32, x as f32 + 1.0, x0, x1);
                self.blend(x, y, v, cx * cy);
            }
        }
    }

    /// The generator's `rot_box`, verbatim: a rotated box antialiased from
    /// its SDF — coverage `0.5 - d`, continuous in the angle.
    fn rot_box(&mut self, cx: f32, cy: f32, hw: f32, hh: f32, angle: f32, v: f32) {
        let (s, c) = angle.sin_cos();
        let reach = hw.hypot(hh) + 2.0;
        let (w, h) = (self.w as i64, self.h as i64);
        let ix0 = ((cx - reach).floor() as i64).clamp(0, w) as usize;
        let ix1 = ((cx + reach).ceil() as i64).clamp(0, w) as usize;
        let iy0 = ((cy - reach).floor() as i64).clamp(0, h) as usize;
        let iy1 = ((cy + reach).ceil() as i64).clamp(0, h) as usize;
        for y in iy0..iy1 {
            let py = y as f32 + 0.5 - cy;
            for x in ix0..ix1 {
                let px = x as f32 + 0.5 - cx;
                let lx = px * c + py * s;
                let ly = -px * s + py * c;
                let qx = lx.abs() - hw;
                let qy = ly.abs() - hh;
                let outside = qx.max(0.0).hypot(qy.max(0.0));
                let inside = qx.max(qy).min(0.0);
                let d = outside + inside;
                self.blend(x, y, v, (0.5 - d).clamp(0.0, 1.0));
            }
        }
    }

    /// The bytes the encoder was handed for this canvas. A limited-range
    /// H.264 round trip returns them unchanged (`16 + 219*b/255` encodes,
    /// the warp shader's `(Y*255-16)/219` decodes), so this IS the luma
    /// the tweener must reproduce at an endpoint.
    fn to_luma8(&self, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(self.px.len());
        out.extend(self.px.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
    }
}

/// `thin_lines`, from the generator: three 2px full-height bars.
const LINE_SPEEDS: [f32; 3] = [1.0, 3.0, 7.0];
const LINE_STARTS: [f32; 3] = [60.5, 260.5, 460.5];
const LINE_W: f32 = 2.0;

/// `rotate_bar`, from the generator: the bar's half extents.
const ROT_HW: f32 = 100.0;
const ROT_HH: f32 = 12.0;

/// `zoom_checker`, from the generator: cell size and shades.
const CHK_CELL: f32 = 32.0;

/// The generator's analytic checkerboard, verbatim: integral of the unit
/// square wave with half-period `cell`, exact for negative `u`.
fn square_wave_integral(u: f32, cell: f32) -> f32 {
    let period = 2.0 * cell;
    let n = (u / period).floor();
    let rem = u - n * period;
    n * cell + (rem - cell).clamp(0.0, cell)
}

fn square_wave_mean(u0: f32, u1: f32, cell: f32) -> f32 {
    let d = u1 - u0;
    if d <= 1e-6 {
        let n = (u0 / (2.0 * cell)).floor();
        return if u0 - n * 2.0 * cell >= cell { 1.0 } else { 0.0 };
    }
    ((square_wave_integral(u1, cell) - square_wave_integral(u0, cell)) / d).clamp(0.0, 1.0)
}

fn checker_coverage(u0: f32, u1: f32, v0: f32, v1: f32, cell: f32) -> f32 {
    let a = square_wave_mean(u0, u1, cell);
    let b = square_wave_mean(v0, v1, cell);
    a * (1.0 - b) + (1.0 - a) * b
}

/// The analytic model of ONE clip. Scoring-side only; a second clip slots
/// in as another variant with its own `rect_at`.
#[derive(Clone, Copy)]
enum RefModel {
    /// `sine_ease`: 64px square, two sine periods of +-96px across the clip.
    SineEase { w: f32, h: f32, sq: f32, period: f32 },
    /// `rect_diagonal`: 64px square, 4 px/frame right, 7.5 px/frame down
    /// reflected off the vertical bounds. The ONLY clip in the set with a
    /// dominant vertical component, so it carries the vertical verdict.
    RectDiagonal { h: f32, sq: f32 },
    /// `rect_translate`: 64px square, constant 4.125 px/frame right from
    /// x = 145.5, vertically centred (generator: TRANSLATE_X0 = 145.5,
    /// TRANSLATE_V = 4.125).
    RectTranslate { h: f32, sq: f32 },
    /// `rect_accelerate`: 64px square from rest at x = 30.5 with constant
    /// a = 10/(frames-1) px/frame^2, so the last frame moves 10 px. The
    /// speed sweeps the whole usable range inside ONE clip.
    RectAccelerate { h: f32, sq: f32, a: f32 },
    /// `thin_lines`: three 2px full-height bars at 1 / 3 / 7 px/frame,
    /// wrapping in x. STRUCTURALLY DIFFERENT from the rects: at 2px the
    /// shape is half a level-0 flow cell wide, so the band metrics
    /// degenerate (a 3px erode leaves no interior at all — hole_err is
    /// identically 0 here) and mean_abs / max_abs / centroid carry the
    /// verdict. The three speeds in one frame also make it the aperture
    /// test: one field, three right answers, no smoothness prior that
    /// satisfies all of them.
    ThinLines { w: f32, h: f32 },
    /// `rotate_bar`: 200x24 bar, one full turn about the frame centre
    /// across the clip — the ROTATION test (a translation-only field must
    /// express it as a tangential gradient; the tips move ~9 px/frame).
    /// rect_at is the bar's AABB, so the band metrics are loose but
    /// consistent; the centroid never moves, so mean_abs / max_abs carry
    /// the verdict, and per-cell field truth comes from DUMP_PAIR offline.
    RotateBar { w: f32, h: f32, frames: f32 },
    /// `zoom_checker`: 32px checkerboard zooming 1.0x -> 1.6x about the
    /// centre — the DIVERGENCE test (~0.85%/pair scale step, up to
    /// ~2.7 px/frame radial at the corners). Full-frame content: rect_at
    /// is the whole frame, hole_err becomes "mean shortfall anywhere in
    /// the interior", and mean_abs / max_abs carry the verdict.
    ZoomChecker { w: f32, h: f32, frames: f32 },
}

fn reflect(v: f32, lo: f32, hi: f32) -> f32 {
    let span = hi - lo;
    if span <= 0.0 {
        return lo;
    }
    let t = (v - lo).rem_euclid(2.0 * span);
    lo + if t <= span { t } else { 2.0 * span - t }
}

impl RefModel {
    fn for_clip(stem: &str, w: usize, h: usize, frames: usize) -> Option<Self> {
        match stem {
            "sine_ease" => Some(RefModel::SineEase {
                w: w as f32,
                h: h as f32,
                sq: 64.0,
                period: frames as f32,
            }),
            "rect_diagonal" => Some(RefModel::RectDiagonal { h: h as f32, sq: 64.0 }),
            "rect_translate" => Some(RefModel::RectTranslate { h: h as f32, sq: 64.0 }),
            "rect_accelerate" => Some(RefModel::RectAccelerate {
                h: h as f32,
                sq: 64.0,
                // The generator's `a = 10.0 / (FRAMES - 1)`; FRAMES is the
                // decoded frame count, so the constant travels with the clip.
                a: 10.0 / (frames.max(2) - 1) as f32,
            }),
            "thin_lines" => Some(RefModel::ThinLines { w: w as f32, h: h as f32 }),
            "rotate_bar" => Some(RefModel::RotateBar {
                w: w as f32,
                h: h as f32,
                frames: frames as f32,
            }),
            "zoom_checker" => Some(RefModel::ZoomChecker {
                w: w as f32,
                h: h as f32,
                frames: frames as f32,
            }),
            _ => None,
        }
    }

    /// The square at CONTINUOUS frame time `f` — the whole point: the
    /// generator's phase formula never needed `f` to be an integer.
    ///
    /// For `thin_lines` this is the FASTEST bar (7 px/frame): the band
    /// metrics and the centroid then describe the hardest of the three,
    /// and the other two still contribute to `mean_abs` / `max_abs`.
    fn rect_at(&self, f: f32) -> (f32, f32, f32, f32) {
        match *self {
            RefModel::SineEase { w, h, sq, period } => {
                let phase = std::f32::consts::TAU * 2.0 * f / period;
                let x = w * 0.5 - sq * 0.5 + 96.0 * phase.sin();
                let y = (h - sq) * 0.5;
                (x, y, x + sq, y + sq)
            }
            RefModel::RectDiagonal { h, sq } => {
                let x = 40.5 + 4.0 * f;
                let y = reflect(20.5 + 7.5 * f, 0.0, h - sq);
                (x, y, x + sq, y + sq)
            }
            RefModel::RectTranslate { h, sq } => {
                let x = 145.5 + 4.125 * f;
                let y = (h - sq) * 0.5;
                (x, y, x + sq, y + sq)
            }
            RefModel::RectAccelerate { h, sq, a } => {
                let x = 30.5 + 0.5 * a * f * f;
                let y = (h - sq) * 0.5;
                (x, y, x + sq, y + sq)
            }
            RefModel::ThinLines { w, h } => {
                let x = (LINE_STARTS[2] + LINE_SPEEDS[2] * f).rem_euclid(w);
                (x, 0.0, x + LINE_W, h)
            }
            RefModel::RotateBar { w, h, frames } => {
                let angle = std::f32::consts::TAU * f / frames;
                let (s, c) = angle.sin_cos();
                let ex = (ROT_HW * c).abs() + (ROT_HH * s).abs();
                let ey = (ROT_HW * s).abs() + (ROT_HH * c).abs();
                (w * 0.5 - ex, h * 0.5 - ey, w * 0.5 + ex, h * 0.5 + ey)
            }
            RefModel::ZoomChecker { w, h, .. } => (0.0, 0.0, w, h),
        }
    }

    fn draw(&self, canvas: &mut Canvas, f: f32) {
        canvas.clear(0.0);
        match *self {
            RefModel::RotateBar { w, h, frames } => {
                let angle = std::f32::consts::TAU * f / frames;
                canvas.rot_box(w * 0.5, h * 0.5, ROT_HW, ROT_HH, angle, 1.0);
            }
            RefModel::ZoomChecker { w, h, frames } => {
                let s = 1.0 + 0.6 * (f / (frames - 1.0));
                let (cx, cy) = (w * 0.5, h * 0.5);
                for y in 0..canvas.h {
                    let v0 = (y as f32 - cy) / s + cy;
                    let v1 = ((y + 1) as f32 - cy) / s + cy;
                    for x in 0..canvas.w {
                        let u0 = (x as f32 - cx) / s + cx;
                        let u1 = ((x + 1) as f32 - cx) / s + cx;
                        let cov = checker_coverage(u0, u1, v0, v1, CHK_CELL);
                        canvas.px[y * canvas.w + x] = 0.22 + 0.56 * cov;
                    }
                }
            }
            RefModel::ThinLines { w, h } => {
                // The generator's `rect_wrap_x`: every horizontal wrap that
                // can touch the frame, so a bar leaving the right edge comes
                // back on the left with its subpixel phase intact.
                for i in 0..3 {
                    let x = (LINE_STARTS[i] + LINE_SPEEDS[i] * f).rem_euclid(w);
                    for k in [-1.0f32, 0.0, 1.0] {
                        let off = k * w;
                        canvas.rect(x + off, 0.0, x + off + LINE_W, h, 1.0);
                    }
                }
            }
            _ => {
                let (x0, y0, x1, y1) = self.rect_at(f);
                canvas.rect(x0, y0, x1, y1, 1.0);
            }
        }
    }

    /// WHERE the flow field is read for the truth-tracking diagnostic, and
    /// how many grid cells to stay clear of its border. A 64px square is 16
    /// cells across and can afford the one-cell inset that keeps the
    /// half-covered edge cells out; a 2px bar is HALF a cell wide and has
    /// no interior to inset into, so it is measured on a two-cell-wide
    /// column straddling the bar with no inset at all.
    fn field_rect(&self, f: f32) -> ((f32, f32, f32, f32), f32) {
        match *self {
            RefModel::ThinLines { w, h } => {
                let x = (LINE_STARTS[2] + LINE_SPEEDS[2] * f).rem_euclid(w);
                ((x - 2.0, 4.0, x + 6.0, h - 4.0), 0.0)
            }
            _ => (self.rect_at(f), 1.0),
        }
    }

    /// The TRUE displacement across pair `pair` -> `pair+1`, in level-0
    /// grid cells (4 source px) — what the estimator's field should read
    /// inside the moving shape. The horizontal wrap of `thin_lines` is
    /// unwrapped to the short step, which is the motion that actually
    /// happened.
    fn truth_v(&self, pair: f32) -> (f32, f32) {
        let a = self.rect_at(pair);
        let b = self.rect_at(pair + 1.0);
        let mut dx = b.0 - a.0;
        if let RefModel::ThinLines { w, .. } = *self {
            if dx > w * 0.5 {
                dx -= w;
            }
            if dx < -w * 0.5 {
                dx += w;
            }
        }
        (dx / 4.0, (b.1 - a.1) / 4.0)
    }
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Sample {
    pair: usize,
    t: f32,
    mean_abs: f32,
    max_abs: f32,
    hole_err: f32,
    halo_err: f32,
    bg_err: f32,
    /// Output-vs-reference luma-centroid offset (px): the DIRECT measure of
    /// the block being drawn in the wrong PLACE — the "little jumps" the
    /// eye tracks, which the area metrics barely see.
    cent_dx: f32,
    cent_dy: f32,
}

/// Compare one readback against the reference. `out` is the warp target's
/// raw BGRA; the picture is gray, so the green channel IS the luma.
fn score(
    pair: usize,
    t: f32,
    bgra: &[u8],
    w: usize,
    h: usize,
    reference: &[u8],
    rect: (f32, f32, f32, f32),
) -> (Sample, Vec<u8>) {
    let (rx0, ry0, rx1, ry1) = rect;
    let mut luma = vec![0u8; w * h];
    let mut sum_abs = 0.0f64;
    let mut max_abs = 0.0f32;
    let (mut hole, mut hole_n) = (0.0f64, 0usize);
    let (mut halo, mut halo_n) = (0.0f64, 0usize);
    let (mut bg, mut bg_n) = (0.0f64, 0usize);
    for y in 0..h {
        let cy = y as f32 + 0.5;
        for x in 0..w {
            let i = y * w + x;
            let o = bgra[i * 4 + 1] as f32;
            luma[i] = o as u8;
            let r = reference[i] as f32;
            let d = o - r;
            let ad = d.abs();
            sum_abs += ad as f64;
            max_abs = max_abs.max(ad);
            let cx = x as f32 + 0.5;
            let inner = cx >= rx0 + BAND
                && cx <= rx1 - BAND
                && cy >= ry0 + BAND
                && cy <= ry1 - BAND;
            let outer = cx >= rx0 - BAND
                && cx <= rx1 + BAND
                && cy >= ry0 - BAND
                && cy <= ry1 + BAND;
            if inner {
                // Black glitching INTO the square: only the shortfall counts.
                hole += (-d).max(0.0) as f64;
                hole_n += 1;
            } else if outer {
                halo += ad as f64;
                halo_n += 1;
            } else {
                // White smearing OUT onto the background.
                bg += d.max(0.0) as f64;
                bg_n += 1;
            }
        }
    }
    let n = (w * h).max(1) as f64;
    // Luma centroids of both pictures (threshold cuts codec shimmer).
    let centroid = |img: &dyn Fn(usize) -> f32| {
        let (mut sx, mut sy, mut sw) = (0.0f64, 0.0f64, 0.0f64);
        for y in 0..h {
            for x in 0..w {
                let v = img(y * w + x);
                if v > 32.0 {
                    sx += (x as f64 + 0.5) * v as f64;
                    sy += (y as f64 + 0.5) * v as f64;
                    sw += v as f64;
                }
            }
        }
        (sx / sw.max(1.0), sy / sw.max(1.0))
    };
    let (ox, oy) = centroid(&|i| luma[i] as f32);
    let (rx, ry) = centroid(&|i| reference[i] as f32);
    (
        Sample {
            pair,
            t,
            mean_abs: (sum_abs / n) as f32,
            max_abs,
            hole_err: (hole / hole_n.max(1) as f64) as f32,
            halo_err: (halo / halo_n.max(1) as f64) as f32,
            bg_err: (bg / bg_n.max(1) as f64) as f32,
            cent_dx: (ox - rx) as f32,
            cent_dy: (oy - ry) as f32,
        },
        luma,
    )
}

// ---------------------------------------------------------------------------
// The sweep state machine
// ---------------------------------------------------------------------------

struct EvalState {
    stem: String,
    path: String,
    w: usize,
    h: usize,
    frames: Vec<Vec<u8>>,
    /// The pair indices this run visits, in order. Contiguous at stride 1;
    /// 0, N, 2N, ... at stride N (see `stride()`).
    pairs: Vec<usize>,
    model: RefModel,
    canvas: Canvas,
    reference: Vec<u8>,
    /// Index of the NEXT (pair, t) to submit.
    next: usize,
    /// The (pair, t) whose picture is in flight, and how many more display
    /// frames to let it settle before the readback.
    pending: Option<(usize, usize)>,
    wait: u32,
    cur_pair: Option<usize>,
    samples: Vec<Sample>,
    /// Worst-by-hole_err cases, with their picture, for the PNG dump.
    keep: Vec<(Sample, Vec<u8>)>,
    /// Pictures requested by name (VJ_TWEEN_EVAL_DUMP_PAIR), written
    /// whatever they score — the only way to watch ONE known case across
    /// a series of fixes once it has stopped being among the worst.
    forced: Vec<(Sample, Vec<u8>)>,
    started: crate::clock::Instant,
    stalls: u32,
    /// One row per pair: the FIELDS measured against the analytic motion
    /// (mean vector inside each endpoint's rect footprint, in grid cells).
    field_rows: Vec<FieldRow>,
}

struct FieldRow {
    pair: usize,
    truth_vx: f32,
    truth_vy: f32,
    fwd_vx: f32,
    fwd_vy: f32,
    fwd_n: usize,
    bwd_vx: f32,
    bwd_vy: f32,
    bwd_n: usize,
    fwd_bg_mag: f32,
    bwd_bg_mag: f32,
}

/// Decode an RGBA16F/RGBA32F readback into f32s (the selftest's decoder).
fn decode_float_texture(bytes: &[u8], w: usize, h: usize) -> Option<Vec<f32>> {
    if bytes.len() == w * h * 8 {
        Some(
            bytes
                .chunks_exact(2)
                .map(|b| {
                    let bits = u16::from_le_bytes([b[0], b[1]]);
                    let sign = if bits >> 15 == 1 { -1.0f32 } else { 1.0 };
                    let exp = ((bits >> 10) & 0x1f) as i32;
                    let man = (bits & 0x3ff) as f32;
                    match exp {
                        0 => sign * man * 2f32.powi(-24),
                        31 => sign * f32::INFINITY,
                        _ => sign * (1.0 + man / 1024.0) * 2f32.powi(exp - 15),
                    }
                })
                .collect(),
        )
    } else if bytes.len() == w * h * 16 {
        Some(
            bytes
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        )
    } else {
        None
    }
}

/// Mean field vector over the grid cells fully inside `rect` (source px,
/// inset by `inset` cells), and mean |v| over cells >2 cells outside it.
fn field_stats(
    px: &[f32],
    gw: usize,
    gh: usize,
    rect: (f32, f32, f32, f32),
    inset: f32,
) -> (f32, f32, usize, f32) {
    let (x0, y0, x1, y1) = (rect.0 / 4.0, rect.1 / 4.0, rect.2 / 4.0, rect.3 / 4.0);
    let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0usize);
    let (mut bg, mut bn) = (0.0f64, 0usize);
    for cy in 0..gh {
        for cx in 0..gw {
            let (fx, fy) = (cx as f32 + 0.5, cy as f32 + 0.5);
            let v = &px[(cy * gw + cx) * 4..];
            let inside =
                fx > x0 + inset && fx < x1 - inset && fy > y0 + inset && fy < y1 - inset;
            let outside = fx < x0 - 2.0 || fx > x1 + 2.0 || fy < y0 - 2.0 || fy > y1 + 2.0;
            if inside {
                sx += v[0] as f64;
                sy += v[1] as f64;
                n += 1;
            } else if outside {
                bg += ((v[0] * v[0] + v[1] * v[1]) as f64).sqrt();
                bn += 1;
            }
        }
    }
    let n1 = n.max(1) as f64;
    (
        (sx / n1) as f32,
        (sy / n1) as f32,
        n,
        (bg / bn.max(1) as f64) as f32,
    )
}

impl EvalState {
    fn total(&self) -> usize {
        self.pairs.len() * TS.len()
    }
}

thread_local! {
    static STATE: RefCell<Option<Box<EvalState>>> = const { RefCell::new(None) };
    /// The rig ran and printed; every later pump is a no-op.
    static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Decode the whole clip into resident NV12 — the exact bytes the player's
/// repeat cache hands the tweener, so the rig measures the tweener and not
/// a colour conversion of its own.
fn load(path: &str) -> Result<EvalState, String> {
    use makepad_widgets::makepad_platform::video_file::VideoFileDecoder;
    let mut decoder = VideoFileDecoder::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let info = decoder.info().clone();
    let (w, h) = (info.width as usize, info.height as usize);
    let mut frames: Vec<Vec<u8>> = Vec::new();
    loop {
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                if frame.width as usize != w || frame.height as usize != h {
                    return Err(format!(
                        "frame {} is {}x{}, stream is {w}x{h}",
                        frames.len(),
                        frame.width,
                        frame.height
                    ));
                }
                frames.push(frame.nv12);
            }
            Ok(None) => break,
            Err(e) => return Err(format!("decode frame {}: {e}", frames.len())),
        }
    }
    if frames.len() < 2 {
        return Err(format!("{} frame(s): a tween needs at least two", frames.len()));
    }
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let model = RefModel::for_clip(&stem, w, h, frames.len())
        .ok_or_else(|| format!("no analytic reference model for clip '{stem}'"))?;
    let pairs: Vec<usize> = (0..frames.len() - 1).step_by(stride()).collect();
    Ok(EvalState {
        stem,
        path: path.to_string(),
        w,
        h,
        frames,
        pairs,
        model,
        canvas: Canvas::new(w, h),
        reference: vec![0; w * h],
        next: 0,
        pending: None,
        wait: 0,
        cur_pair: None,
        samples: Vec::new(),
        keep: Vec::new(),
        forced: Vec::new(),
        started: crate::clock::Instant::now(),
        stalls: 0,
        field_rows: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

fn stats(values: &[f32]) -> (f32, f32, f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mean = v.iter().map(|x| *x as f64).sum::<f64>() / v.len() as f64;
    let med = v[v.len() / 2];
    let p95 = v[(((v.len() - 1) as f32) * 0.95).round() as usize];
    (mean as f32, med, p95, v[v.len() - 1])
}

fn metric_block(title: &str, rows: &[Sample], out: &mut String) {
    use std::fmt::Write;
    let _ = writeln!(out, "{title} (n = {})", rows.len());
    if rows.is_empty() {
        let _ = writeln!(out, "  (no samples)\n");
        return;
    }
    let _ = writeln!(
        out,
        "  {:<10} {:>10} {:>10} {:>10} {:>10}",
        "metric", "mean", "med", "p95", "max"
    );
    for (name, get) in [
        ("mean_abs", (|s: &Sample| s.mean_abs) as fn(&Sample) -> f32),
        ("max_abs", |s| s.max_abs),
        ("hole_err", |s| s.hole_err),
        ("halo_err", |s| s.halo_err),
        ("bg_err", |s| s.bg_err),
        // The centroid offsets are the DIRECT place error, per axis: the
        // vertical one is the only number in the set that separates a
        // vertical failure from a horizontal one.
        ("|cent_dx|", |s| s.cent_dx.abs()),
        ("|cent_dy|", |s| s.cent_dy.abs()),
    ] {
        let values: Vec<f32> = rows.iter().map(get).collect();
        let (mean, med, p95, max) = stats(&values);
        let _ = writeln!(
            out,
            "  {name:<10} {mean:>10.3} {med:>10.3} {p95:>10.3} {max:>10.3}"
        );
    }
    let _ = writeln!(out);
}

/// One "worst N by <metric>" table.
fn worst_block(
    title: &str,
    samples: &[Sample],
    key: fn(&Sample) -> f32,
    n: usize,
    out: &mut String,
) {
    use std::fmt::Write;
    let mut worst = samples.to_vec();
    worst.sort_by(|a, b| key(b).partial_cmp(&key(a)).unwrap_or(std::cmp::Ordering::Equal));
    let _ = writeln!(out, "{title}");
    let _ = writeln!(
        out,
        "  {:>4} {:>5} {:>10} {:>10} {:>10} {:>10} {:>10} {:>8} {:>8}",
        "pair", "t", "hole_err", "halo_err", "bg_err", "mean_abs", "max_abs", "cent_dx",
        "cent_dy"
    );
    for s in worst.iter().take(n) {
        let _ = writeln!(
            out,
            "  {:>4} {:>5.1} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>8.3} {:>8.3}",
            s.pair, s.t, s.hole_err, s.halo_err, s.bg_err, s.mean_abs, s.max_abs, s.cent_dx,
            s.cent_dy
        );
    }
    let _ = writeln!(out);
}

fn gray_png(luma: &[u8], w: usize, h: usize, gain: f32) -> Option<Vec<u8>> {
    let mut rgba = vec![255u8; w * h * 4];
    for (i, &v) in luma.iter().enumerate().take(w * h) {
        let g = ((v as f32 * gain).min(255.0)) as u8;
        rgba[i * 4] = g;
        rgba[i * 4 + 1] = g;
        rgba[i * 4 + 2] = g;
    }
    makepad_asset_importer::classic_import::encode_png_rgba(&rgba, w as u32, h as u32).ok()
}

fn finish(state: &EvalState) {
    use std::fmt::Write;
    let dir = out_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log!("TWEEN_EVAL: create {}: {e}", dir.display());
    }

    // ---- eval.csv --------------------------------------------------------
    let mut csv =
        String::from("pair,t,mean_abs,max_abs,hole_err,halo_err,bg_err,cent_dx,cent_dy\n");
    for s in &state.samples {
        let _ = writeln!(
            csv,
            "{},{:.1},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
            s.pair, s.t, s.mean_abs, s.max_abs, s.hole_err, s.halo_err, s.bg_err,
            s.cent_dx, s.cent_dy
        );
    }
    let _ = std::fs::write(dir.join("eval.csv"), &csv);

    // ---- fields.csv ------------------------------------------------------
    let mut fcsv = String::from(
        "pair,truth_vx,truth_vy,fwd_vx,fwd_vy,fwd_n,bwd_vx,bwd_vy,bwd_n,fwd_bg_mag,bwd_bg_mag
",
    );
    for r in &state.field_rows {
        let _ = writeln!(
            fcsv,
            "{},{:.4},{:.4},{:.4},{:.4},{},{:.4},{:.4},{},{:.4},{:.4}",
            r.pair, r.truth_vx, r.truth_vy, r.fwd_vx, r.fwd_vy, r.fwd_n, r.bwd_vx, r.bwd_vy,
            r.bwd_n, r.fwd_bg_mag, r.bwd_bg_mag
        );
    }
    let _ = std::fs::write(dir.join("fields.csv"), &fcsv);

    // ---- summary.txt -----------------------------------------------------
    let is_between = |s: &Sample| s.t > 0.001 && s.t < 0.999;
    let between: Vec<Sample> = state
        .samples
        .iter()
        .copied()
        .filter(|s| is_between(s) && !excl_pairs().contains(&s.pair))
        .collect();
    let excluded: Vec<Sample> = state
        .samples
        .iter()
        .copied()
        .filter(|s| is_between(s) && excl_pairs().contains(&s.pair))
        .collect();
    let ends: Vec<Sample> = state
        .samples
        .iter()
        .copied()
        .filter(|s| s.t <= 0.001 || s.t >= 0.999)
        .collect();
    let t0: Vec<Sample> = state.samples.iter().copied().filter(|s| s.t <= 0.001).collect();
    let t1: Vec<Sample> = state.samples.iter().copied().filter(|s| s.t >= 0.999).collect();

    let mut sum = String::new();
    let _ = writeln!(sum, "TWEEN_EVAL — GPU frame tweener error measurement");
    let _ = writeln!(sum, "clip      {}", state.path);
    let _ = writeln!(
        sum,
        "geometry  {}x{}, {} frames, {} pairs walked (stride {}), {} t-steps/pair, {} samples",
        state.w,
        state.h,
        state.frames.len(),
        state.pairs.len(),
        stride(),
        TS.len(),
        state.samples.len()
    );
    if stride() > 1 {
        let _ = writeln!(
            sum,
            "seed      STRIDE MODE: reset_seed() before every pair (the app's\n\
             \x20         non-contiguous-jump path), so every pair starts from the\n\
             \x20         exhaustive coarse search instead of the previous field."
        );
    }
    let _ = writeln!(
        sum,
        "model     {} (analytic, scoring only — the tweener is never told)",
        state.stem
    );
    let _ = writeln!(
        sum,
        "settle    {} extra display frame(s) per state, {} readback stall(s)",
        settle_frames(),
        state.stalls
    );
    let _ = writeln!(
        sum,
        "wall      {:.1}s\n",
        state.started.elapsed().as_secs_f64()
    );
    let _ = writeln!(
        sum,
        "All errors are luma levels (0..255). hole_err = mean shortfall inside the\n\
         analytic square eroded by {BAND}px; halo_err = mean |error| in the +-{BAND}px band\n\
         around its edge; bg_err = mean excess more than {BAND}px outside it.\n"
    );
    metric_block("IN-BETWEENS  t = 0.1 .. 0.9", &between, &mut sum);
    if !excluded.is_empty() {
        let _ = writeln!(
            sum,
            "EXCLUDED PAIRS {:?} — measured, reported, kept out of the stats above",
            excl_pairs()
        );
        metric_block("  excluded pairs, in-betweens", &excluded, &mut sum);
    }
    // ---- ERROR AS A FUNCTION OF t ----------------------------------------
    //
    // The aggregate in-between block averages every t together, and that
    // average is blind to the one artifact a SLOW-MO viewer complains about
    // loudest: an in-between that is right in the middle of the pair and
    // drifts toward its ends reads as the picture RAILING — snapping true
    // at each source frame and sagging between them — while scoring a
    // perfectly ordinary mean.
    //
    // The shape is the diagnosis. A producer whose fields are anchored at
    // the ENDPOINTS and re-gathered per t (the classical stack) has no
    // reason to prefer any particular t, so its curve is flat or rises
    // smoothly toward the middle where the two endpoints disagree most. A
    // producer that computes ONE field at the intermediate time t = 0.5 and
    // rescales it linearly for every other t (the neural stack:
    // `rife_pixel`) is exact at t = 0.5 by construction and exact at the
    // endpoints by construction, and wrong in between — a W, with humps
    // near t ~ 0.2 and t ~ 0.8. The two hypotheses are distinguishable by
    // eye from this table alone, which is why it is a table and not a
    // scalar.
    {
        let _ = writeln!(sum, "ERROR vs t  (the RAILING diagnostic — shape matters, not level)");
        let _ = writeln!(
            sum,
            "  {:>5} {:>6} {:>10} {:>10} {:>10} {:>10} {:>10}",
            "t", "n", "mean_abs", "halo_err", "hole_err", "|cent_dx|", "|cent_dy|"
        );
        for t in TS {
            let rows: Vec<Sample> = state
                .samples
                .iter()
                .copied()
                .filter(|s| (s.t - t).abs() < 0.001 && !excl_pairs().contains(&s.pair))
                .collect();
            if rows.is_empty() {
                continue;
            }
            let m = |f: fn(&Sample) -> f32| -> f32 {
                let v: Vec<f32> = rows.iter().map(f).collect();
                stats(&v).0
            };
            let _ = writeln!(
                sum,
                "  {t:>5.1} {:>6} {:>10.4} {:>10.4} {:>10.4} {:>10.4} {:>10.4}",
                rows.len(),
                m(|s| s.mean_abs),
                m(|s| s.halo_err),
                m(|s| s.hole_err),
                m(|s| s.cent_dx.abs()),
                m(|s| s.cent_dy.abs()),
            );
        }
        let _ = writeln!(sum);
    }
    metric_block("ENDPOINT FLOOR  t = 0 and t = 1 (codec + pipeline noise)", &ends, &mut sum);
    metric_block("  endpoint t = 0 only", &t0, &mut sum);
    metric_block("  endpoint t = 1 only", &t1, &mut sum);

    let ranked: Vec<Sample> = state
        .samples
        .iter()
        .copied()
        .filter(|s| !excl_pairs().contains(&s.pair))
        .collect();
    worst_block(
        "WORST 10 BY halo_err (all samples, endpoints included)",
        &ranked,
        |s| s.halo_err,
        10,
        &mut sum,
    );
    worst_block(
        "WORST 10 BY hole_err (all samples, endpoints included)",
        &ranked,
        |s| s.hole_err,
        10,
        &mut sum,
    );

    // ---- field truth tracking --------------------------------------------
    // The pixel metrics say the picture is wrong; THIS says whether the
    // FIELD is wrong, per axis. A vertical mechanism (odd pyramid dims, a
    // scalar inter-level scale, a single-axis seed scale) shows up here as
    // |fwd_vy - truth_vy| >> |fwd_vx - truth_vx| at the same speed, and
    // nowhere else in the report.
    if !state.field_rows.is_empty() {
        let ex: Vec<f32> = state.field_rows.iter().map(|r| (r.fwd_vx - r.truth_vx).abs()).collect();
        let ey: Vec<f32> = state.field_rows.iter().map(|r| (r.fwd_vy - r.truth_vy).abs()).collect();
        // The backward field runs the other way: its truth is -truth.
        let bx: Vec<f32> =
            state.field_rows.iter().map(|r| (r.bwd_vx + r.truth_vx).abs()).collect();
        let by: Vec<f32> =
            state.field_rows.iter().map(|r| (r.bwd_vy + r.truth_vy).abs()).collect();
        // RELATIVE error where the truth is big enough to divide by.
        let rel = |num: &[f32], den: fn(&FieldRow) -> f32| -> f32 {
            let mut s = 0.0f64;
            let mut n = 0usize;
            for (e, r) in num.iter().zip(state.field_rows.iter()) {
                let d = den(r).abs();
                if d > 0.25 {
                    s += (*e / d) as f64;
                    n += 1;
                }
            }
            if n == 0 { f32::NAN } else { (s / n as f64) as f32 }
        };
        let _ = writeln!(
            sum,
            "FIELD TRUTH TRACKING (level-0 cells = 4 source px, n = {} pairs)",
            state.field_rows.len()
        );
        let _ = writeln!(
            sum,
            "  {:<12} {:>10} {:>10} {:>10} {:>10} {:>10}",
            "axis", "mean|err|", "med", "p95", "max", "mean rel"
        );
        for (name, v, den) in [
            ("fwd vx", &ex, (|r: &FieldRow| r.truth_vx) as fn(&FieldRow) -> f32),
            ("fwd vy", &ey, |r| r.truth_vy),
            ("bwd vx", &bx, |r| r.truth_vx),
            ("bwd vy", &by, |r| r.truth_vy),
        ] {
            let (mean, med, p95, max) = stats(v);
            let _ = writeln!(
                sum,
                "  {name:<12} {mean:>10.4} {med:>10.4} {p95:>10.4} {max:>10.4} {:>10.4}",
                rel(v, den)
            );
        }
        let bgf: Vec<f32> = state.field_rows.iter().map(|r| r.fwd_bg_mag).collect();
        let (bm, bmed, bp95, bmax) = stats(&bgf);
        let _ = writeln!(
            sum,
            "  {:<12} {bm:>10.4} {bmed:>10.4} {bp95:>10.4} {bmax:>10.4}",
            "bg |v|"
        );
        let _ = writeln!(sum);
    }

    // ---- the pictures ----------------------------------------------------
    let mut canvas = Canvas::new(state.w, state.h);
    let mut reference: Vec<u8> = Vec::new();
    let mut written = Vec::new();
    for (s, luma) in state.keep.iter().take(DUMP).chain(state.forced.iter()) {
        let f = s.pair as f32 + s.t;
        state.model.draw(&mut canvas, f);
        canvas.to_luma8(&mut reference);
        let diff: Vec<u8> = luma
            .iter()
            .zip(reference.iter())
            .map(|(&o, &r)| ((o as i32 - r as i32).unsigned_abs().min(255)) as u8)
            .collect();
        let tag = format!("p{}_t{:.1}", s.pair, s.t);
        for (prefix, bytes, gain) in [
            ("out", luma.as_slice(), 1.0f32),
            ("ref", reference.as_slice(), 1.0),
            ("diff", diff.as_slice(), 4.0),
        ] {
            if let Some(png) = gray_png(bytes, state.w, state.h, gain) {
                let path = dir.join(format!("{prefix}_{tag}.png"));
                let _ = std::fs::write(&path, png);
            }
        }
        written.push(tag);
    }
    if !written.is_empty() {
        let _ = writeln!(
            sum,
            "PNG triptychs (out_/ref_/diff_, diff amplified 4x) for the {} worst: {}",
            written.len(),
            written.join(", ")
        );
    }

    let _ = std::fs::write(dir.join("summary.txt"), &sum);
    log!("TWEEN_EVAL DONE\n{sum}");
    log!(
        "TWEEN_EVAL wrote {} ({} csv rows)",
        dir.display(),
        state.samples.len()
    );
    // ONE machine-readable line per run, the twin of REALSTAT: the judgment
    // matrix is a grep over these, so a config's synthetic and real columns
    // are scraped the same way and can never be transposed by hand.
    let between: Vec<Sample> = state
        .samples
        .iter()
        .filter(|s| s.t > 0.001 && s.t < 0.999 && !excl_pairs().contains(&s.pair))
        .copied()
        .collect();
    let pick = |f: fn(&Sample) -> f32| -> (f32, f32) {
        let v: Vec<f32> = between.iter().map(f).collect();
        let (m, _, p95, _) = stats(&v);
        (m, p95)
    };
    let (ma, ma95) = pick(|s| s.mean_abs);
    let (ho, ho95) = pick(|s| s.hole_err);
    let (hl, hl95) = pick(|s| s.halo_err);
    let (cd, cd95) = pick(|s| s.cent_dx.abs());
    // Give the log a chance to flush before the process goes.
    println!("TWEEN_EVAL DONE");
    println!("{sum}");
    println!(
        "SYNSTAT clip={} cfg={} n={} mean_abs={ma:.4}/{ma95:.4} hole={ho:.4}/{ho95:.4} halo={hl:.4}/{hl95:.4} cent_dx={cd:.4}/{cd95:.4}",
        state.stem,
        cfg_tag(),
        between.len()
    );
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
}

// ---------------------------------------------------------------------------
// The pump
// ---------------------------------------------------------------------------

impl crate::App {
    /// One display frame of the eval sweep: read back the state submitted
    /// `settle+1` frames ago, score it, submit the next one.
    pub(crate) fn pump_tween_eval(&mut self, cx: &mut Cx) {
        if real_clip_path().is_some() {
            self.pump_real_tween_eval(cx);
            return;
        }
        if DONE.with(|d| d.get()) {
            return;
        }
        // The rig drives itself: nothing else in the app is asking for
        // frames (no clip is loaded, no deck is playing).
        self.video_pump = cx.new_next_frame();

        let mut guard = match STATE.try_with(|s| s.borrow_mut().take()) {
            Ok(v) => v,
            Err(_) => return,
        };
        if guard.is_none() {
            let Some(path) = clip_path() else { return };
            match load(&path) {
                Ok(state) => {
                    log!(
                        "TWEEN_EVAL: {} — {}x{}, {} frames, {} pairs x {} t = {} samples",
                        state.path,
                        state.w,
                        state.h,
                        state.frames.len(),
                        state.frames.len() - 1,
                        TS.len(),
                        state.total()
                    );
                    guard = Some(Box::new(state));
                }
                Err(e) => {
                    log!("TWEEN_EVAL: {e}");
                    DONE.with(|d| d.set(true));
                    println!("TWEEN_EVAL FAILED: {e}");
                    std::process::exit(2);
                }
            }
        }
        let mut state = guard.unwrap();
        self.step_tween_eval(cx, &mut state);
        STATE.with(|s| *s.borrow_mut() = Some(state));
    }

    fn step_tween_eval(&mut self, cx: &mut Cx, state: &mut EvalState) {
        // ---- 1. the picture submitted earlier ----------------------------
        if let Some((pair, ti)) = state.pending {
            if state.wait > 0 {
                state.wait -= 1;
                return;
            }
            let tex = self
                .tween_view(cx, SlotId::A, |_cx, view| view.output_texture())
                .flatten();
            let Some(tex) = tex else {
                // The pass has not rendered yet — wait, do not score black.
                state.stalls += 1;
                return;
            };
            let Some((tw, th, bgra)) = cx.debug_read_render_texture(&tex) else {
                state.stalls += 1;
                return;
            };
            if tw != state.w || th != state.h || bgra.len() < tw * th * 4 {
                state.stalls += 1;
                return;
            }
            let t = TS[ti];
            let f = pair as f32 + t;
            state.model.draw(&mut state.canvas, f);
            state.canvas.to_luma8(&mut state.reference);
            let rect = state.model.rect_at(f);
            let (sample, luma) =
                score(pair, t, &bgra, state.w, state.h, &state.reference, rect);
            state.samples.push(sample);
            // Bounded worst-by-halo_err keep: the pictures are 230 KB each
            // and only a handful are ever written. halo_err, not hole_err —
            // hole_err is structurally ZERO on `thin_lines` (a 2px bar has
            // no interior left after a 3px erode), so ranking by it would
            // dump ten arbitrary frames for that clip. The edge band is the
            // one metric that means the same thing for every shape.
            let worse_than_last = state
                .keep
                .last()
                .map(|(s, _)| sample.halo_err > s.halo_err)
                .unwrap_or(true);
            if dump_pairs().contains(&pair) && (t - 0.5).abs() < 0.01 {
                state.forced.push((sample, luma.clone()));
            }
            if !excl_pairs().contains(&pair) && (state.keep.len() < KEEP || worse_than_last) {
                state.keep.push((sample, luma));
                state.keep.sort_by(|a, b| {
                    b.0.halo_err.partial_cmp(&a.0.halo_err).unwrap_or(std::cmp::Ordering::Equal)
                });
                state.keep.truncate(KEEP);
            }
            // FIELD DIAGNOSTICS, once per pair at its first in-between:
            // the flow stack has just run for this pair, its final fields
            // are standing in field_tex — measure them against the truth.
            if ti == 1 {
                let fwd = self
                    .tween_view(cx, SlotId::A, |_cx, view| view.debug_texture("fwd"))
                    .flatten()
                    .and_then(|tex| cx.debug_read_render_texture(&tex));
                let bwd = self
                    .tween_view(cx, SlotId::A, |_cx, view| view.debug_texture("bwd"))
                    .flatten()
                    .and_then(|tex| cx.debug_read_render_texture(&tex));
                if let (Some((fw, fh, fb)), Some((bw_, bh, bb))) = (fwd, bwd) {
                    let fpx = decode_float_texture(&fb, fw, fh);
                    let bpx = decode_float_texture(&bb, bw_, bh);
                    if let (Some(fpx), Some(bpx)) = (fpx, bpx) {
                        if dump_pairs().contains(&pair) && fw == bw_ && fh == bh {
                            use std::fmt::Write as _;
                            let mut s = String::from("gx,gy,fwd_x,fwd_y,bwd_x,bwd_y\n");
                            for gy in 0..fh {
                                for gx in 0..fw {
                                    let i = (gy * fw + gx) * 4;
                                    let _ = writeln!(
                                        s,
                                        "{gx},{gy},{:.4},{:.4},{:.4},{:.4}",
                                        fpx[i], fpx[i + 1], bpx[i], bpx[i + 1]
                                    );
                                }
                            }
                            let dir = out_dir();
                            let _ = std::fs::create_dir_all(&dir);
                            let _ = std::fs::write(dir.join(format!("field_p{pair}.csv")), s);
                            log!("TWEEN_EVAL: dumped {fw}x{fh} field for pair {pair}");
                        }
                        let (ra, inset) = state.model.field_rect(pair as f32);
                        let (rb, _) = state.model.field_rect(pair as f32 + 1.0);
                        let (truth_vx, truth_vy) = state.model.truth_v(pair as f32);
                        let (fvx, fvy, fn_, fbg) = field_stats(&fpx, fw, fh, ra, inset);
                        let (bvx, bvy, bn_, bbg) = field_stats(&bpx, bw_, bh, rb, inset);
                        state.field_rows.push(FieldRow {
                            pair,
                            truth_vx,
                            truth_vy,
                            fwd_vx: fvx,
                            fwd_vy: fvy,
                            fwd_n: fn_,
                            bwd_vx: bvx,
                            bwd_vy: bvy,
                            bwd_n: bn_,
                            fwd_bg_mag: fbg,
                            bwd_bg_mag: bbg,
                        });
                    }
                }
            }
            state.pending = None;
            if state.samples.len() % 100 == 0 {
                log!(
                    "TWEEN_EVAL: {}/{} samples ({:.1}s)",
                    state.samples.len(),
                    state.total(),
                    state.started.elapsed().as_secs_f64()
                );
            }
        }

        // ---- 2. done? ----------------------------------------------------
        if state.next >= state.total() {
            DONE.with(|d| d.set(true));
            finish(state);
            std::process::exit(0);
        }

        // ---- 3. submit the next state ------------------------------------
        let pair = state.pairs[state.next / TS.len()];
        let ti = state.next % TS.len();
        let t = TS[ti];
        // PAIRS WALK STRICTLY FORWARD, like playback: the flow stack seeds
        // the coarse level from the PREVIOUS pair's field (temporal
        // seeding), so a scrambled pair order would measure a state the
        // player never reaches.
        let new_pair = state.cur_pair != Some(pair);
        // At stride > 1 the walk is forward but NOT contiguous, which is
        // exactly the transport's fast-playback case — and the app's pump
        // drops the seed there. Drop it here too, or the rig would measure
        // a stale-seed state the player deliberately avoids.
        let jump = stride() > 1;
        let (w, h) = (state.w as u32, state.h as u32);
        let a = if new_pair { Some(state.frames[pair].clone()) } else { None };
        let b = if new_pair { Some(state.frames[pair + 1].clone()) } else { None };
        let tier = eval_ai_tier();
        self.tween_view(cx, SlotId::A, |cx, view| {
            view.set_ai_tier(cx, tier);
            if let (Some(a), Some(b)) = (a.as_ref(), b.as_ref()) {
                if jump {
                    view.reset_seed();
                }
                view.set_pair(cx, a, b, w, h);
                view.set_cut(cx, false);
                view.set_fade(cx, false);
                // No diagnostic view: the rig scores the real warp.
                view.set_debug(cx, 0.0);
            }
            view.set_t(cx, t);
            view.redraw(cx);
        });
        // NEURAL fields on the analytic sweep. This rig probes t = 0 and
        // t = 1 as well as the in-betweens, so it is the one that can
        // certify the ENDPOINT FLOOR of the neural warp — `rife_pixel`
        // must reproduce frame A at t = 0 and frame B at t = 1 exactly, and
        // any drift there means the field/mask weighting is wrong before a
        // single in-between number is worth reading.
        if new_pair && eval_ai() {
            let idx = state.pairs.iter().position(|p| *p == pair).unwrap_or(0);
            let fresh = idx % eval_ai_stride() == 0;
            if tier.segments() > 1 {
                if fresh {
                    match rife_midpoint(
                        &state.frames[pair],
                        &state.frames[pair + 1],
                        state.w,
                        state.h,
                        tier.synth_depth(),
                    ) {
                        Ok((mids, _)) => {
                            let (a, b) = (
                                state.frames[pair].clone(),
                                state.frames[pair + 1].clone(),
                            );
                            self.tween_view(cx, SlotId::A, |cx, view| {
                                let mut rungs: Vec<&[u8]> =
                                    Vec::with_capacity(mids.len() + 2);
                                rungs.push(&a);
                                rungs.extend(mids.iter().map(|m| m.as_slice()));
                                rungs.push(&b);
                                view.set_ladder(cx, pair, &rungs, w, h);
                            });
                        }
                        Err(e) => {
                            log!("TWEEN_EVAL AI2: {e}");
                            println!("TWEEN_EVAL AI2 FAILED: {e}");
                            std::process::exit(2);
                        }
                    }
                } else {
                    self.tween_view(cx, SlotId::A, |cx, view| view.clear_triple(cx));
                }
            } else if fresh {
                match rife_field(&state.frames[pair], &state.frames[pair + 1], state.w, state.h)
                {
                    Ok((pw, ph, flow, mask, _)) => {
                        self.tween_view(cx, SlotId::A, |cx, view| {
                            view.set_rife_field(cx, pair, pw, ph, &flow, &mask);
                        });
                    }
                    Err(e) => {
                        log!("TWEEN_EVAL AI: {e}");
                        println!("TWEEN_EVAL AI FAILED: {e}");
                        std::process::exit(2);
                    }
                }
            } else {
                self.tween_view(cx, SlotId::A, |cx, view| view.age_rife_field(cx, pair));
            }
        }
        state.cur_pair = Some(pair);
        state.pending = Some((pair, ti));
        state.wait = settle_frames();
        state.next += 1;
    }
}

// ===========================================================================
// REAL-FOOTAGE BENCHMARK — held-out middle-frame reconstruction
// ===========================================================================
//
// The analytic rig above can only score clips whose correct in-between is a
// closed form, which means clips SYNTHESIZED for it. That is a white box,
// and a tweener tuned only against it overfits: hard edges on flat fields,
// no grain, no soft boundaries, no texture on both sides of a motion
// discontinuity. Real footage has all four, and the afternoon's aggressive
// features were accepted on the white box alone.
//
// This rig needs no model, so it scores ANY clip: for a frame triple
// (n-1, n, n+1) the tweener is handed the two OUTER frames and asked for
// t = 0.5; frame n is held out and used as the reference. Nothing about
// frame n reaches the tweener.
//
//   VJ_TWEEN_EVAL_REAL=/abs/clip.mp4 ./target/release/makepad-vj
//   # -> $VJ_TWEEN_EVAL_OUT/{real.csv,real_summary.txt,real_*.png}
//
// WALK. `n` advances by ONE every step, contiguously, and the pair
// (n-1, n+1) is submitted and rendered for EVERY n — so the flow stack's
// temporal seed evolves exactly as it would in playback at half rate (each
// successive pair overlaps the last by one frame). Only every `step`-th n
// is SCORED (VJ_TWEEN_EVAL_REAL_STEP, default 3), which decorrelates the
// samples without ever letting the seed see a jump the player would not.
//
// THE BASELINE. Every scored n also renders a plain 50% CROSSFADE of the
// same two endpoints, through the same shader (`fade_on`), and scores it
// against the same reference. A tween that does not beat a crossfade on a
// sample has no business existing on that sample, so the crossfade column
// is the pass line, not a nicety.
//
// THE REFERENCE goes through the pipeline too: pair (n, n) in fade mode at
// t = 0 makes the warp emit frame n's own NV12 -> RGB conversion. So all
// three pictures are read back from the same target in the same format and
// the only difference between them is the thing being measured — no CPU
// colour conversion to be subtly wrong about, and no endpoint floor to
// subtract.

/// `VJ_TWEEN_EVAL_REAL=/abs/path/clip.mp4` selects the real-footage rig.
pub fn real_clip_path() -> Option<String> {
    static P: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::env::var("VJ_TWEEN_EVAL_REAL").ok().filter(|v| !v.is_empty()))
        .clone()
}

/// Score every Nth middle frame (the walk still visits every one).
///
/// Default 1. The TEMPORAL metrics below need consecutive SCORED samples,
/// and a per-frame spatial metric is structurally blind to the artifact the
/// eye complains about loudest — a boundary that is soft but STEADY reads
/// as fine, while one that is sharp and twitches reads as broken, and both
/// can score the same mean_abs. Raise it to decorrelate the spatial
/// statistics at the cost of the temporal ones.
fn real_step() -> usize {
    static S: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *S.get_or_init(|| {
        std::env::var("VJ_TWEEN_EVAL_REAL_STEP")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|v| *v >= 1)
            .unwrap_or(1)
    })
}

/// `VJ_TWEEN_EVAL_FL1=1` drives the measured view through the PER-VIEW
/// conservative flag (`FlowTweenView::set_safe`) instead of the process-wide
/// `VJ_TWEEN_SAFE`. The two must produce bit-identical numbers — that is the
/// gate on the side-by-side FL1/FL2 deck modes, which resolve their tier
/// per view rather than per process.
fn eval_fl1() -> bool {
    static S: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *S.get_or_init(|| std::env::var("VJ_TWEEN_EVAL_FL1").map(|v| v == "1").unwrap_or(false))
}

/// Cap on decoded frames (0 = the whole clip).
fn real_max_frames() -> usize {
    static S: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *S.get_or_init(|| {
        std::env::var("VJ_TWEEN_EVAL_REAL_MAX")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
    })
}

/// Side of the square tiles the WORST-TILE metric maxes over.
const TILE: usize = 32;
/// Chebyshev dilation radius of the strong-edge set, in pixels.
const EDGE_DILATE: isize = 2;
/// A Sobel L1 magnitude below this is never "a strong edge", however high
/// it ranks within its own frame — without the floor a flat, grain-only
/// frame manufactures an edge mask out of its noise.
const EDGE_FLOOR: f32 = 40.0;
/// Fraction of interior pixels the adaptive threshold keeps before dilation.
const EDGE_KEEP: f32 = 0.10;
/// Radius of the box window the TEXTURE-DENSITY measure averages over.
const DENSITY_R: usize = 5;
/// A local mean Sobel magnitude below this is not texture, it is grain — a
/// flat sky must never qualify as a textured interior however it ranks
/// within its own frame.
const DENSITY_FLOOR: f32 = 10.0;
/// How many worst-by-edge-band samples keep their pictures.
const REAL_KEEP: usize = 6;

#[derive(Clone, Copy, Default)]
struct RealMetrics {
    mean_abs: f32,
    psnr: f32,
    edge_err: f32,
    worst_tile: f32,
    /// Mean |err_n - err_{n-1}| over the frame — see `score_now`.
    flicker: f32,
    /// The same inside this frame's edge band.
    edge_flicker: f32,
    /// Mean |err| over TEXTURED INTERIORS — see `masks`.
    interior_err: f32,
    /// Mean |err_n - err_{n-1}| over the same region.
    interior_flicker: f32,
}

#[derive(Clone, Copy)]
struct RealSample {
    n: usize,
    /// Whether the temporal metrics are defined for this sample (they need
    /// the previous SCORED sample to be its immediate predecessor).
    has_flick: bool,
    tw: RealMetrics,
    xf: RealMetrics,
    /// Fraction of the frame inside the dilated strong-edge band.
    edge_frac: f32,
    /// Fraction of the frame counted as textured interior.
    interior_frac: f32,
    /// Std-dev of the reference and of the tween output (the black gate).
    ref_sd: f32,
    tw_sd: f32,
    black: bool,
    /// Which producer drew this sample, and whether it is the FIRST sample
    /// after a producer/field change — the handoff, where mechanism (c)
    /// lives and where the flicker metric should spike if it is real.
    ai: AiTier,
    handoff: bool,
}

/// BGRA8 readback -> BT.709 luma in 0..255. The warp target is
/// `RenderBGRAu8`, so byte 0 is blue and byte 2 is red.
fn bgra_to_luma(bgra: &[u8], w: usize, h: usize, out: &mut Vec<f32>) {
    out.clear();
    out.reserve(w * h);
    for i in 0..w * h {
        let b = bgra[i * 4] as f32;
        let g = bgra[i * 4 + 1] as f32;
        let r = bgra[i * 4 + 2] as f32;
        out.push(0.2126 * r + 0.7152 * g + 0.0722 * b);
    }
}

fn std_dev(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let n = v.len() as f64;
    let mean = v.iter().map(|&x| x as f64).sum::<f64>() / n;
    let var = v.iter().map(|&x| (x as f64 - mean).powi(2)).sum::<f64>() / n;
    var.sqrt() as f32
}

/// Two disjoint regions of the true frame, both derived from the REFERENCE
/// alone so every config being compared on a frame shares them exactly.
///
/// BAND — pixels within `EDGE_DILATE` of a strong Sobel response. Mush,
/// doubling and torn boundaries live here. The threshold is adaptive (the
/// `EDGE_KEEP` strongest interior pixels, floored at `EDGE_FLOOR`) so a
/// clip's own contrast sets its edge set.
///
/// INTERIOR — TEXTURED but NOT within the band: local mean gradient above
/// both `DENSITY_FLOOR` and the median density of the non-band pixels. This
/// is the inside of a panning textured object, and it is exactly where the
/// aperture problem bites: on repeated texture many displacements explain
/// the data equally well, so a search that re-decides per frame twitches
/// there. The band metric cannot see it (wrong region) and the frame mean
/// dilutes it, which is why a live A/B can report "glitches INSIDE objects"
/// against metrics that call the config a win.
fn masks(refl: &[f32], w: usize, h: usize) -> (Vec<bool>, Vec<bool>, f32, f32) {
    let mut mag = vec![0.0f32; w * h];
    if w < 3 || h < 3 {
        return (vec![false; w * h], vec![false; w * h], 0.0, 0.0);
    }
    let at = |x: usize, y: usize| refl[y * w + x];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
            mag[y * w + x] = gx.abs() + gy.abs();
        }
    }
    let mut interior_vals: Vec<f32> = Vec::with_capacity((w - 2) * (h - 2));
    for y in 1..h - 1 {
        interior_vals.extend_from_slice(&mag[y * w + 1..y * w + w - 1]);
    }
    interior_vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((interior_vals.len() as f32) * (1.0 - EDGE_KEEP)) as usize;
    let adaptive = interior_vals
        .get(idx.min(interior_vals.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0);
    let thresh = adaptive.max(EDGE_FLOOR);
    let strong: Vec<bool> = mag.iter().map(|&m| m >= thresh).collect();
    // Separable Chebyshev dilation: max over a (2r+1) window per axis.
    let r = EDGE_DILATE;
    let mut hrow = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut on = false;
            let mut d = -r;
            while d <= r {
                let sx = x as isize + d;
                if sx >= 0 && (sx as usize) < w && strong[y * w + sx as usize] {
                    on = true;
                    break;
                }
                d += 1;
            }
            hrow[y * w + x] = on;
        }
    }
    let mut band = vec![false; w * h];
    let mut band_n = 0usize;
    for y in 0..h {
        for x in 0..w {
            let mut on = false;
            let mut d = -r;
            while d <= r {
                let sy = y as isize + d;
                if sy >= 0 && (sy as usize) < h && hrow[sy as usize * w + x] {
                    on = true;
                    break;
                }
                d += 1;
            }
            band[y * w + x] = on;
            if on {
                band_n += 1;
            }
        }
    }
    // Texture density: box mean of |grad| via an integral image.
    let mut sat = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += mag[y * w + x] as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let dr = DENSITY_R as isize;
    let mut density = vec![0.0f32; w * h];
    for y in 0..h {
        let y0 = (y as isize - dr).max(0) as usize;
        let y1 = ((y as isize + dr + 1) as usize).min(h);
        for x in 0..w {
            let x0 = (x as isize - dr).max(0) as usize;
            let x1 = ((x as isize + dr + 1) as usize).min(w);
            let s = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0]
                + sat[y0 * (w + 1) + x0];
            density[y * w + x] = (s / ((y1 - y0) * (x1 - x0)).max(1) as f64) as f32;
        }
    }
    let mut nonband: Vec<f32> =
        (0..w * h).filter(|&i| !band[i]).map(|i| density[i]).collect();
    nonband.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = nonband.get(nonband.len() / 2).copied().unwrap_or(0.0);
    let dthresh = median.max(DENSITY_FLOOR);
    let mut interior = vec![false; w * h];
    let mut int_n = 0usize;
    for i in 0..w * h {
        if !band[i] && density[i] >= dthresh {
            interior[i] = true;
            int_n += 1;
        }
    }
    let n = (w * h).max(1) as f32;
    (band, interior, band_n as f32 / n, int_n as f32 / n)
}

/// The four numbers, all on luma in 0..255 levels.
///
///   mean_abs   mean |out - ref| over the whole frame
///   psnr       10 log10(255^2 / mean (out - ref)^2), capped at 99
///   edge_err   mean |out - ref| inside the reference's dilated edge band
///   worst_tile max over 32x32 tiles of the tile's mean |out - ref| — a
///              localized tear the frame mean cannot see
fn real_metrics(
    out: &[f32],
    refl: &[f32],
    w: usize,
    h: usize,
    band: &[bool],
    interior: &[bool],
) -> RealMetrics {
    let (mut sum, mut sq) = (0.0f64, 0.0f64);
    let (mut esum, mut en) = (0.0f64, 0usize);
    let (mut isum, mut in_) = (0.0f64, 0usize);
    for i in 0..w * h {
        let d = (out[i] - refl[i]).abs() as f64;
        sum += d;
        sq += d * d;
        if band[i] {
            esum += d;
            en += 1;
        } else if interior[i] {
            isum += d;
            in_ += 1;
        }
    }
    let n = (w * h).max(1) as f64;
    let mse = sq / n;
    let psnr = if mse <= 1e-9 { 99.0 } else { (10.0 * (255.0f64 * 255.0 / mse).log10()) as f32 };
    let mut worst = 0.0f32;
    let mut tiles = 0usize;
    let mut ty = 0usize;
    while ty + TILE <= h {
        let mut tx = 0usize;
        while tx + TILE <= w {
            let mut s = 0.0f64;
            for y in ty..ty + TILE {
                for x in tx..tx + TILE {
                    s += (out[y * w + x] - refl[y * w + x]).abs() as f64;
                }
            }
            worst = worst.max((s / (TILE * TILE) as f64) as f32);
            tiles += 1;
            tx += TILE;
        }
        ty += TILE;
    }
    if tiles == 0 {
        worst = (sum / n) as f32;
    }
    RealMetrics {
        mean_abs: (sum / n) as f32,
        psnr,
        edge_err: (esum / en.max(1) as f64) as f32,
        worst_tile: worst,
        interior_err: (isum / in_.max(1) as f64) as f32,
        // Temporal: needs the previous sample, so `score_now` fills these.
        ..Default::default()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum RealPhase {
    /// Nothing in flight — submit the tween for `n`.
    Submit,
    /// The warped in-between of (n-1, n+1) is rendering.
    Tween,
    /// The 50% crossfade of the same pair is rendering.
    Xf,
    /// Frame n through the same pipeline (pair (n,n), fade, t=0).
    Ref,
}

struct RealState {
    stem: String,
    path: String,
    w: usize,
    h: usize,
    frames: Vec<Vec<u8>>,
    n: usize,
    first_n: usize,
    last_n: usize,
    step: usize,
    phase: RealPhase,
    wait: u32,
    tween_l: Vec<f32>,
    xf_l: Vec<f32>,
    ref_l: Vec<f32>,
    samples: Vec<RealSample>,
    keep: Vec<(RealSample, Vec<u8>, Vec<u8>, Vec<u8>)>,
    /// Signed error fields of the previous scored sample, for the temporal
    /// metrics, and which n they belong to.
    prev_tw_err: Vec<f32>,
    prev_xf_err: Vec<f32>,
    prev_scored_n: Option<usize>,
    black_flags: usize,
    started: crate::clock::Instant,
    stalls: u32,
    /// The producer that served the pair currently in flight, the one that
    /// served the previous scored pair, and the RIFE forward's cost.
    ai: AiTier,
    prev_ai: AiTier,
    rife_ms: f64,
    rife_calls: usize,
    rife_dims: (usize, usize),
}

impl RealState {
    fn scored(&self, n: usize) -> bool {
        n >= self.first_n && (n - self.first_n) % self.step == 0
    }

    fn planned(&self) -> usize {
        if self.last_n < self.first_n {
            return 0;
        }
        (self.last_n - self.first_n) / self.step + 1
    }

    /// Score the three pictures now standing in `tween_l` / `xf_l` / `ref_l`.
    fn score_now(&mut self) {
        let (w, h) = (self.w, self.h);
        let (band, interior, edge_frac, interior_frac) = masks(&self.ref_l, w, h);
        let mut tw = real_metrics(&self.tween_l, &self.ref_l, w, h, &band, &interior);
        let mut xf = real_metrics(&self.xf_l, &self.ref_l, w, h, &band, &interior);
        // TEMPORAL FLICKER — the axis every per-frame metric is blind to.
        //
        // err_n = out_n - ref_n, SIGNED. A producer that is merely soft
        // carries a large but nearly CONSTANT err from frame to frame; one
        // that twitches — a per-pixel decision flipping between two
        // explanations at a soft boundary — carries a small mean err with a
        // large temporal derivative. mean |err_n - err_{n-1}| separates
        // them, and it is exactly the "shimmering patches around arms"
        // complaint that a mean_abs table can rate as harmless.
        //
        // The reference's own motion cancels: both terms are measured
        // against their own frame's truth, so honest scene change
        // contributes nothing and only the ERROR's instability is counted.
        let n_px = w * h;
        let tw_err: Vec<f32> =
            (0..n_px).map(|i| self.tween_l[i] - self.ref_l[i]).collect();
        let xf_err: Vec<f32> = (0..n_px).map(|i| self.xf_l[i] - self.ref_l[i]).collect();
        let contiguous = self
            .prev_scored_n
            .map(|p| p + self.step == self.n && self.prev_tw_err.len() == n_px)
            .unwrap_or(false);
        if contiguous {
            let mut acc = |cur: &[f32], prev: &[f32]| -> (f32, f32, f32) {
                let (mut s, mut es, mut en) = (0.0f64, 0.0f64, 0usize);
                let (mut is_, mut in_) = (0.0f64, 0usize);
                for i in 0..n_px {
                    let d = (cur[i] - prev[i]).abs() as f64;
                    s += d;
                    if band[i] {
                        es += d;
                        en += 1;
                    } else if interior[i] {
                        is_ += d;
                        in_ += 1;
                    }
                }
                (
                    (s / n_px as f64) as f32,
                    (es / en.max(1) as f64) as f32,
                    (is_ / in_.max(1) as f64) as f32,
                )
            };
            let (f1, e1, i1) = acc(&tw_err, &self.prev_tw_err);
            let (f2, e2, i2) = acc(&xf_err, &self.prev_xf_err);
            tw.flicker = f1;
            tw.edge_flicker = e1;
            tw.interior_flicker = i1;
            xf.flicker = f2;
            xf.edge_flicker = e2;
            xf.interior_flicker = i2;
        }
        self.prev_tw_err = tw_err;
        self.prev_xf_err = xf_err;
        self.prev_scored_n = Some(self.n);
        let ref_sd = std_dev(&self.ref_l);
        let tw_sd = std_dev(&self.tween_l);
        // THE BLACK GATE. A shader that fails to compile leaves the warp
        // target at its clear colour, and every number below then describes
        // a constant picture rather than a tweener (the black-warp bug of
        // 2026-08-24, whose fingerprint was every sample scoring
        // identically). A reference with real structure and an output with
        // none is that failure and nothing else.
        let black = ref_sd > 2.0 && tw_sd < 0.5;
        // A HANDOFF is a sample where the FIELD ITSELF changed under the
        // picture: a freshly computed field replacing whatever stood, or
        // the classical stack taking the pair over, or handing it back.
        //
        // Ageing is deliberately NOT a handoff. `age_rife_field` re-stamps
        // the pair id on the SAME texture, so the warp reads identical
        // flow on an aged pair as on the one before it — nothing swapped,
        // the field is merely describing older frames. That is the whole
        // point of the split: mechanism (c), the swap, lands on handoff
        // samples; mechanism (b), staleness, lands on the aged ones. A
        // flag that fired on both would confirm whichever one you already
        // believed.
        let field_changed = match (self.prev_ai, self.ai) {
            (_, AiTier::Fresh) => true,
            (a, b) => a.is_neural() != b.is_neural(),
        };
        let handoff = self.ai != AiTier::Off && field_changed && contiguous;
        let sample = RealSample {
            n: self.n,
            has_flick: contiguous,
            tw,
            xf,
            edge_frac,
            interior_frac,
            ref_sd,
            tw_sd,
            black,
            ai: self.ai,
            handoff,
        };
        self.prev_ai = self.ai;
        if black {
            self.black_flags += 1;
        }
        let to_u8 = |v: &[f32]| -> Vec<u8> {
            v.iter().map(|&x| x.round().clamp(0.0, 255.0) as u8).collect()
        };
        let worse = self
            .keep
            .last()
            .map(|(s, ..)| sample.tw.edge_err > s.tw.edge_err)
            .unwrap_or(true);
        if self.keep.len() < REAL_KEEP || worse {
            self.keep.push((
                sample,
                to_u8(&self.tween_l),
                to_u8(&self.ref_l),
                to_u8(&self.xf_l),
            ));
            self.keep.sort_by(|a, b| {
                b.0.tw.edge_err.partial_cmp(&a.0.tw.edge_err).unwrap_or(std::cmp::Ordering::Equal)
            });
            self.keep.truncate(REAL_KEEP);
        }
        self.samples.push(sample);
    }
}

fn load_real(path: &str) -> Result<RealState, String> {
    use makepad_widgets::makepad_platform::video_file::VideoFileDecoder;
    let mut decoder = VideoFileDecoder::open(path).map_err(|e| format!("open {path}: {e}"))?;
    let info = decoder.info().clone();
    let (w, h) = (info.width as usize, info.height as usize);
    let cap = real_max_frames();
    let mut frames: Vec<Vec<u8>> = Vec::new();
    loop {
        if cap > 0 && frames.len() >= cap {
            break;
        }
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                if frame.width as usize != w || frame.height as usize != h {
                    return Err(format!(
                        "frame {} is {}x{}, stream is {w}x{h}",
                        frames.len(),
                        frame.width,
                        frame.height
                    ));
                }
                frames.push(frame.nv12);
            }
            Ok(None) => break,
            Err(e) => return Err(format!("decode frame {}: {e}", frames.len())),
        }
    }
    if frames.len() < 3 {
        return Err(format!(
            "{} frame(s): a held-out middle frame needs at least three",
            frames.len()
        ));
    }
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let last_n = frames.len() - 2;
    Ok(RealState {
        stem,
        path: path.to_string(),
        w,
        h,
        frames,
        n: 1,
        first_n: 1,
        last_n,
        step: real_step(),
        phase: RealPhase::Submit,
        wait: 0,
        tween_l: Vec::new(),
        xf_l: Vec::new(),
        ref_l: Vec::new(),
        samples: Vec::new(),
        keep: Vec::new(),
        prev_tw_err: Vec::new(),
        prev_xf_err: Vec::new(),
        prev_scored_n: None,
        black_flags: 0,
        started: crate::clock::Instant::now(),
        stalls: 0,
        ai: AiTier::Off,
        prev_ai: AiTier::Off,
        rife_ms: 0.0,
        rife_calls: 0,
        rife_dims: (0, 0),
    })
}

/// Percentile of the BAD tail: `hi` for lower-is-better metrics, `!hi` (the
/// 5th percentile) for PSNR, where the bad tail is the low end.
fn tail(values: &[f32], hi: bool) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let q = if hi { 0.95 } else { 0.05 };
    v[((v.len() as f32 - 1.0) * q).round() as usize]
}

fn mean_of(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    (values.iter().map(|&x| x as f64).sum::<f64>() / values.len() as f64) as f32
}

/// The config stamp: the classical tier's feature list, and — when the
/// neural producer is driving — the cadence that produced these numbers.
/// A row without this is unreadable a week later, and the AI numbers are
/// meaningless without the stride and the reuse limit beside them.
fn cfg_tag() -> String {
    let base = crate::flow_tween::config_tag(eval_fl1());
    if !eval_ai() {
        return base;
    }
    let (pw, _) = crate::flow_tween::rife_proxy_dims(1920, 1080);
    if eval_ai_tier().segments() > 1 {
        let (sw, _) = crate::flow_tween::rife_synth_dims(1920, 1080);
        return format!(
            "{}[stride{},lag{},synth{},{:?}]+{}",
            eval_ai_tier().label(),
            eval_ai_stride(),
            eval_ai_lag(),
            sw,
            crate::flow_tween::rife_scale(),
            base
        );
    }
    format!(
        "AI1field[stride{},lag{},reuse{},proxy{},{:?}]+{}",
        eval_ai_stride(),
        eval_ai_lag(),
        crate::flow_tween::rife_max_reuse(),
        pw,
        crate::flow_tween::rife_scale(),
        base
    )
}

fn finish_real(state: &RealState) {
    use std::fmt::Write;
    let dir = out_dir();
    let _ = std::fs::create_dir_all(&dir);
    let cfg = cfg_tag();

    let mut csv = String::from(
        "n,tw_mean_abs,tw_psnr,tw_edge,tw_int,tw_tile,tw_flick,tw_eflick,tw_iflick,\
xf_mean_abs,xf_psnr,xf_edge,xf_int,xf_tile,xf_flick,xf_eflick,xf_iflick,edge_frac,int_frac,ref_sd,tw_sd,black,has_flick,ai_tier,handoff\n",
    );
    for s in &state.samples {
        let _ = writeln!(
            csv,
            "{},{:.4},{:.3},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.3},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.2},{:.2},{},{},{},{}",
            s.n,
            s.tw.mean_abs,
            s.tw.psnr,
            s.tw.edge_err,
            s.tw.interior_err,
            s.tw.worst_tile,
            s.tw.flicker,
            s.tw.edge_flicker,
            s.tw.interior_flicker,
            s.xf.mean_abs,
            s.xf.psnr,
            s.xf.edge_err,
            s.xf.interior_err,
            s.xf.worst_tile,
            s.xf.flicker,
            s.xf.edge_flicker,
            s.xf.interior_flicker,
            s.edge_frac,
            s.interior_frac,
            s.ref_sd,
            s.tw_sd,
            s.black as u8,
            s.has_flick as u8,
            s.ai.tag(),
            s.handoff as u8
        );
    }
    let _ = std::fs::write(dir.join("real.csv"), &csv);

    let mut sum = String::new();
    let _ = writeln!(sum, "TWEEN_EVAL REAL — held-out middle-frame reconstruction");
    let _ = writeln!(sum, "clip      {}", state.path);
    let _ = writeln!(
        sum,
        "geometry  {}x{}, {} frames; walked n = {}..{} contiguously (pair n-1,n+1 at t=0.5),",
        state.w,
        state.h,
        state.frames.len(),
        state.first_n,
        state.last_n
    );
    let _ = writeln!(
        sum,
        "          scored every {}th n = {} samples; seed evolves as half-rate playback",
        state.step,
        state.samples.len()
    );
    let _ = writeln!(sum, "config    {cfg}");
    let _ = writeln!(
        sum,
        "wall      {:.1}s, {} readback stall(s)",
        state.started.elapsed().as_secs_f64(),
        state.stalls
    );
    let ref_sds: Vec<f32> = state.samples.iter().map(|s| s.ref_sd).collect();
    let fracs: Vec<f32> = state.samples.iter().map(|s| s.edge_frac).collect();
    let ifracs: Vec<f32> = state.samples.iter().map(|s| s.interior_frac).collect();
    let _ = writeln!(
        sum,
        "sanity    {} black-output flag(s); mean reference sd {:.1} levels; edge band {:.1}% / textured interior {:.1}% of frame",
        state.black_flags,
        mean_of(&ref_sds),
        mean_of(&fracs) * 100.0,
        mean_of(&ifracs) * 100.0
    );
    let _ = writeln!(sum);
    let _ = writeln!(
        sum,
        "All errors are luma levels (0..255) against the HELD-OUT frame n. The"
    );
    let _ = writeln!(
        sum,
        "crossfade column is the same two endpoints mixed 50/50 through the same"
    );
    let _ = writeln!(
        sum,
        "shader — the pass line. 'p95' is the BAD tail (p5 for PSNR)."
    );
    let _ = writeln!(sum);
    let _ = writeln!(
        sum,
        "  {:<17} {:>9} {:>9} | {:>9} {:>9} | {:>7}",
        "metric", "tween", "tween p95", "xfade", "xfade p95", "win%"
    );

    let mut stat_line = format!(
        "REALSTAT clip={} cfg={} n={} black={}",
        state.stem,
        cfg,
        state.samples.len(),
        state.black_flags
    );
    // The last two rows are TEMPORAL and only defined on samples whose
    // predecessor was scored, so they carry their own sample set.
    let rows: [(&str, bool, bool, fn(&RealMetrics) -> f32); 8] = [
        ("mean_abs", true, false, |m| m.mean_abs),
        ("psnr", false, false, |m| m.psnr),
        ("edge_err", true, false, |m| m.edge_err),
        ("interior_err", true, false, |m| m.interior_err),
        ("worst_tile", true, false, |m| m.worst_tile),
        ("flicker", true, true, |m| m.flicker),
        ("edge_flicker", true, true, |m| m.edge_flicker),
        ("interior_flicker", true, true, |m| m.interior_flicker),
    ];
    for (name, lower_better, temporal, get) in rows {
        let pool: Vec<&RealSample> = state
            .samples
            .iter()
            .filter(|s| !temporal || s.has_flick)
            .collect();
        let tw: Vec<f32> = pool.iter().map(|s| get(&s.tw)).collect();
        let xf: Vec<f32> = pool.iter().map(|s| get(&s.xf)).collect();
        let wins = pool
            .iter()
            .filter(|s| {
                if lower_better {
                    get(&s.tw) < get(&s.xf)
                } else {
                    get(&s.tw) > get(&s.xf)
                }
            })
            .count();
        let win_pct = 100.0 * wins as f32 / pool.len().max(1) as f32;
        let (tm, tt) = (mean_of(&tw), tail(&tw, lower_better));
        let (xm, xt) = (mean_of(&xf), tail(&xf, lower_better));
        let _ = writeln!(
            sum,
            "  {name:<11} {tm:>9.3} {tt:>9.3} | {xm:>9.3} {xt:>9.3} | {win_pct:>6.1}%"
        );
        let _ = write!(
            stat_line,
            " {name}={tm:.4}/{tt:.4} xf_{name}={xm:.4}/{xt:.4} win_{name}={win_pct:.1}"
        );
    }

    // ---- the cadence decomposition ---------------------------------------
    //
    // Everything above is a per-config average, and an average is exactly
    // the wrong instrument for "a lot of little discontinuities": a tier
    // that is excellent on 2 pairs in 3 and lurches on the third can post a
    // better mean than a tier that is mediocre everywhere, and the eye will
    // still call the second one steadier. So split the SAME samples by the
    // producer state that drew them, and put the handoff pairs — where a
    // field is replaced or a producer changes hands — in their own column.
    if eval_ai() {
        let _ = writeln!(sum);
        let _ = writeln!(
            sum,
            "AI CADENCE — {} RIFE forward(s) at {}x{}, {:.1} ms each (mean)",
            state.rife_calls,
            state.rife_dims.0,
            state.rife_dims.1,
            if state.rife_calls > 0 {
                state.rife_ms / state.rife_calls as f64
            } else {
                0.0
            }
        );
        let mut counts: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for s in &state.samples {
            *counts.entry(s.ai.tag()).or_default() += 1;
        }
        let parts: Vec<String> =
            counts.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let _ = writeln!(sum, "producer  {}", parts.join(" "));
        let _ = writeln!(sum);
        let _ = writeln!(
            sum,
            "  {:<18} {:>8} {:>10} {:>10} {:>10} {:>10}",
            "sample set", "n", "flicker", "e_flicker", "i_flicker", "edge_err"
        );
        let flick_pool: Vec<&RealSample> =
            state.samples.iter().filter(|s| s.has_flick).collect();
        let row = |name: &str, pool: &[&RealSample], out: &mut String| {
            if pool.is_empty() {
                return;
            }
            let f = mean_of(&pool.iter().map(|s| s.tw.flicker).collect::<Vec<_>>());
            let e = mean_of(&pool.iter().map(|s| s.tw.edge_flicker).collect::<Vec<_>>());
            let i = mean_of(&pool.iter().map(|s| s.tw.interior_flicker).collect::<Vec<_>>());
            let d = mean_of(&pool.iter().map(|s| s.tw.edge_err).collect::<Vec<_>>());
            let _ = writeln!(
                out,
                "  {name:<18} {:>8} {f:>10.4} {e:>10.4} {i:>10.4} {d:>10.4}",
                pool.len()
            );
        };
        let all: Vec<&RealSample> = flick_pool.clone();
        row("all scored", &all, &mut sum);
        let ho: Vec<&RealSample> = flick_pool.iter().filter(|s| s.handoff).copied().collect();
        let steady: Vec<&RealSample> =
            flick_pool.iter().filter(|s| !s.handoff).copied().collect();
        row("HANDOFF pairs", &ho, &mut sum);
        row("steady pairs", &steady, &mut sum);
        for tier in ["fresh", "classical"] {
            let p: Vec<&RealSample> =
                flick_pool.iter().filter(|s| s.ai.tag() == tier).copied().collect();
            row(&format!("  tier {tier}"), &p, &mut sum);
        }
        for k in 1..=8u32 {
            let p: Vec<&RealSample> = flick_pool
                .iter()
                .filter(|s| s.ai == AiTier::Aged(k))
                .copied()
                .collect();
            row(&format!("  tier aged{k}"), &p, &mut sum);
        }
        let hm = mean_of(&ho.iter().map(|s| s.tw.flicker).collect::<Vec<_>>());
        let sm = mean_of(&steady.iter().map(|s| s.tw.flicker).collect::<Vec<_>>());
        if !ho.is_empty() && !steady.is_empty() {
            let _ = writeln!(
                sum,
                "\nHANDOFF PENALTY  flicker x{:.3} ({hm:.4} vs {sm:.4} steady)",
                hm / sm.max(1e-6)
            );
        }
        let _ = write!(
            stat_line,
            " ai_handoff_flick={hm:.4} ai_steady_flick={sm:.4} ai_handoff_n={} ai_ms={:.1}",
            ho.len(),
            if state.rife_calls > 0 {
                state.rife_ms / state.rife_calls as f64
            } else {
                0.0
            }
        );

        // THE TIMELINE. The tables say a handoff costs so much on average;
        // only the per-pair series shows whether the cost is a spike on the
        // handoff pair itself (mechanism c: the field SWAPPED under a
        // continuous picture) or a plateau that rises with staleness
        // (mechanism b: the field is simply describing the wrong frames).
        // Those two want different fixes, and no aggregate separates them.
        let _ = writeln!(sum, "\nPER-PAIR FLICKER TIMELINE (* = handoff)");
        let mut line = String::new();
        for s in state.samples.iter().filter(|s| s.has_flick).take(120) {
            let _ = write!(
                line,
                "  n{:<4} {:<10}{} {:>8.3}",
                s.n,
                s.ai.tag(),
                if s.handoff { "*" } else { " " },
                s.tw.flicker
            );
            if line.len() > 100 {
                let _ = writeln!(sum, "{line}");
                line.clear();
            }
        }
        if !line.is_empty() {
            let _ = writeln!(sum, "{line}");
        }
    }

    let mut written = Vec::new();
    for (s, tween, refl, xf) in state.keep.iter() {
        let diff: Vec<u8> = tween
            .iter()
            .zip(refl.iter())
            .map(|(&o, &r)| ((o as i32 - r as i32).unsigned_abs().min(255)) as u8)
            .collect();
        let tag = format!("n{}", s.n);
        for (prefix, bytes, gain) in [
            ("real_out", tween.as_slice(), 1.0f32),
            ("real_ref", refl.as_slice(), 1.0),
            ("real_xf", xf.as_slice(), 1.0),
            ("real_diff", diff.as_slice(), 4.0),
        ] {
            if let Some(png) = gray_png(bytes, state.w, state.h, gain) {
                let _ = std::fs::write(dir.join(format!("{prefix}_{tag}.png")), png);
            }
        }
        written.push(tag);
    }
    if !written.is_empty() {
        let _ = writeln!(sum);
        let _ = writeln!(
            sum,
            "PNGs (real_out_/real_ref_/real_xf_/real_diff_, diff amplified 4x) for the {} worst by edge_err: {}",
            written.len(),
            written.join(", ")
        );
    }

    if state.black_flags * 2 > state.samples.len() {
        let _ = writeln!(sum);
        let _ = writeln!(
            sum,
            "*** RUN INVALID: {}/{} samples had a CONSTANT warp output against a",
            state.black_flags,
            state.samples.len()
        );
        let _ = writeln!(
            sum,
            "*** structured reference. The warp shader is not drawing — check the"
        );
        let _ = writeln!(
            sum,
            "*** log for 'failed to compile' before believing ANY number here."
        );
    }

    let _ = std::fs::write(dir.join("real_summary.txt"), &sum);
    log!("TWEEN_EVAL REAL DONE\n{sum}");
    println!("{sum}");
    println!("{stat_line}");
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
}

thread_local! {
    static REAL_STATE: RefCell<Option<Box<RealState>>> = const { RefCell::new(None) };
}

impl crate::App {
    /// Submit one state to the tweener: optionally a new pair, a fade flag
    /// and a t. Fade renders never run the flow stack (`flow_dirty` is left
    /// standing), so the reference and crossfade passes interleaved into the
    /// walk cost the temporal seed nothing.
    fn submit_real(
        &mut self,
        cx: &mut Cx,
        state: &mut RealState,
        pair: Option<(Vec<u8>, Vec<u8>)>,
        fade: bool,
        t: f32,
    ) {
        let (w, h) = (state.w as u32, state.h as u32);
        let tier = eval_ai_tier();
        self.tween_view(cx, SlotId::A, |cx, view| {
            if let Some((a, b)) = pair.as_ref() {
                view.set_pair(cx, a, b, w, h);
            }
            view.set_safe(cx, eval_fl1());
            view.set_ai_tier(cx, tier);
            // THE BASELINE PASSES MUST SEE THE SOURCE PAIR. A crossfade of
            // (M, B) is not the crossfade this rig's pass line is defined
            // as, and the reference pass is frame n through the shader, not
            // frame n through a half-pair. Drop the midpoint for both; the
            // next Submit re-establishes it from scratch.
            if fade {
                view.clear_triple(cx);
            }
            view.set_cut(cx, false);
            view.set_fade(cx, fade);
            view.set_debug(cx, 0.0);
            view.set_t(cx, t);
            view.redraw(cx);
        });
        state.wait = settle_frames();
    }

    /// The NEURAL producer for the pair about to be submitted, driven the
    /// way `main.rs` drives it — set_pair first (already done by the
    /// caller), then either adopt a field computed from THIS pair's own
    /// frames or re-stamp the standing one onto it.
    ///
    /// The one deliberate difference from the pump is WHERE the field
    /// comes from: here the forward runs inline on the pair being drawn,
    /// so a run is reproducible and a stride is an exact cadence rather
    /// than whatever the worker happened to finish. Everything downstream
    /// of `set_rife_field` — the texture, the warp, the ageing rule — is
    /// the shipped code path untouched.
    fn apply_ai(&mut self, cx: &mut Cx, state: &mut RealState) {
        if !eval_ai() {
            return;
        }
        let n = state.n;
        let fresh = (n - state.first_n) % eval_ai_stride() == 0;
        // AI2 — FRAME DOUBLING. The net synthesizes the pair's midpoint and
        // the classical stack owns both half-pairs from there. The
        // degradation rule is the pump's: a pair without a fresh midpoint
        // gets plain FL2 on (A, B), never a neighbour's midpoint.
        let tier = eval_ai_tier();
        if tier.segments() > 1 {
            if !fresh {
                self.tween_view(cx, SlotId::A, |cx, view| view.clear_triple(cx));
                state.ai = AiTier::Classical;
                return;
            }
            let src = n.saturating_sub(eval_ai_lag()).max(state.first_n);
            let (a, b) = (state.frames[src - 1].clone(), state.frames[src + 1].clone());
            match rife_midpoint(&a, &b, state.w, state.h, tier.synth_depth()) {
                Ok((mids, ms)) => {
                    state.rife_ms += ms;
                    state.rife_calls += 1;
                    state.rife_dims = crate::flow_tween::rife_synth_dims(
                        state.w as u32,
                        state.h as u32,
                    );
                    let (fa, fb) =
                        (state.frames[n - 1].clone(), state.frames[n + 1].clone());
                    let (w, h) = (state.w as u32, state.h as u32);
                    self.tween_view(cx, SlotId::A, |cx, view| {
                        let mut rungs: Vec<&[u8]> = Vec::with_capacity(mids.len() + 2);
                        rungs.push(&fa);
                        rungs.extend(mids.iter().map(|m| m.as_slice()));
                        rungs.push(&fb);
                        view.set_ladder(cx, n, &rungs, w, h);
                    });
                    state.ai = AiTier::Fresh;
                }
                Err(e) => {
                    log!("TWEEN_EVAL AI2: {e}");
                    println!("TWEEN_EVAL AI2 FAILED: {e}");
                    std::process::exit(2);
                }
            }
            return;
        }
        if fresh {
            // The LAG shifts which pair the field describes, never which
            // pair it is applied to — a late worker result, exactly.
            let src = n.saturating_sub(eval_ai_lag()).max(state.first_n);
            let (a, b) = (state.frames[src - 1].clone(), state.frames[src + 1].clone());
            match rife_field(&a, &b, state.w, state.h) {
                Ok((pw, ph, flow, mask, ms)) => {
                    state.rife_ms += ms;
                    state.rife_calls += 1;
                    state.rife_dims = (pw, ph);
                    self.tween_view(cx, SlotId::A, |cx, view| {
                        view.set_rife_field(cx, n, pw, ph, &flow, &mask);
                    });
                    state.ai = AiTier::Fresh;
                    return;
                }
                Err(e) => {
                    log!("TWEEN_EVAL AI: {e}");
                    println!("TWEEN_EVAL AI FAILED: {e}");
                    std::process::exit(2);
                }
            }
        }
        // No fresh field for this pair: the shipped ageing rule decides
        // whether the standing one is re-stamped or the classical stack
        // takes the pair back. Ask the VIEW what it decided rather than
        // re-deriving the rule here — a rig that models the policy instead
        // of observing it stops measuring the policy.
        let age = self
            .tween_view(cx, SlotId::A, |cx, view| {
                view.age_rife_field(cx, n);
                view.rife_field_pair().map(|_| view.rife_age())
            })
            .flatten();
        state.ai = match age {
            Some(k) => AiTier::Aged(k),
            None => AiTier::Classical,
        };
    }

    fn read_warp(&mut self, cx: &mut Cx, state: &RealState) -> Option<Vec<u8>> {
        let tex = self
            .tween_view(cx, SlotId::A, |_cx, view| view.output_texture())
            .flatten()?;
        let (tw, th, bgra) = cx.debug_read_render_texture(&tex)?;
        if tw != state.w || th != state.h || bgra.len() < tw * th * 4 {
            return None;
        }
        Some(bgra)
    }

    pub(crate) fn pump_real_tween_eval(&mut self, cx: &mut Cx) {
        if DONE.with(|d| d.get()) {
            return;
        }
        self.video_pump = cx.new_next_frame();
        let mut guard = match REAL_STATE.try_with(|s| s.borrow_mut().take()) {
            Ok(v) => v,
            Err(_) => return,
        };
        if guard.is_none() {
            let Some(path) = real_clip_path() else { return };
            match load_real(&path) {
                Ok(state) => {
                    log!(
                        "TWEEN_EVAL REAL: {} — {}x{}, {} frames, n {}..{} step {} = {} scored samples [{}]",
                        state.path,
                        state.w,
                        state.h,
                        state.frames.len(),
                        state.first_n,
                        state.last_n,
                        state.step,
                        state.planned(),
                        cfg_tag()
                    );
                    guard = Some(Box::new(state));
                }
                Err(e) => {
                    log!("TWEEN_EVAL REAL: {e}");
                    DONE.with(|d| d.set(true));
                    println!("TWEEN_EVAL REAL FAILED: {e}");
                    std::process::exit(2);
                }
            }
        }
        let mut state = guard.unwrap();
        self.step_real_tween_eval(cx, &mut state);
        REAL_STATE.with(|s| *s.borrow_mut() = Some(state));
    }

    fn step_real_tween_eval(&mut self, cx: &mut Cx, state: &mut RealState) {
        if state.phase != RealPhase::Submit {
            if state.wait > 0 {
                state.wait -= 1;
                return;
            }
            let Some(bgra) = self.read_warp(cx, state) else {
                state.stalls += 1;
                return;
            };
            let (w, h) = (state.w, state.h);
            match state.phase {
                RealPhase::Tween => {
                    let mut buf = std::mem::take(&mut state.tween_l);
                    bgra_to_luma(&bgra, w, h, &mut buf);
                    state.tween_l = buf;
                    if state.scored(state.n) {
                        // Same pair, same t, crossfade branch: the baseline.
                        self.submit_real(cx, state, None, true, 0.5);
                        state.phase = RealPhase::Xf;
                    } else {
                        state.n += 1;
                        state.phase = RealPhase::Submit;
                    }
                }
                RealPhase::Xf => {
                    let mut buf = std::mem::take(&mut state.xf_l);
                    bgra_to_luma(&bgra, w, h, &mut buf);
                    state.xf_l = buf;
                    // Frame n through the same shader: pair (n,n), fade, t=0.
                    let f = state.frames[state.n].clone();
                    self.submit_real(cx, state, Some((f.clone(), f)), true, 0.0);
                    state.phase = RealPhase::Ref;
                }
                RealPhase::Ref => {
                    let mut buf = std::mem::take(&mut state.ref_l);
                    bgra_to_luma(&bgra, w, h, &mut buf);
                    state.ref_l = buf;
                    state.score_now();
                    if state.samples.len() % 20 == 0 {
                        log!(
                            "TWEEN_EVAL REAL: {}/{} samples ({:.1}s)",
                            state.samples.len(),
                            state.planned(),
                            state.started.elapsed().as_secs_f64()
                        );
                    }
                    state.n += 1;
                    state.phase = RealPhase::Submit;
                }
                RealPhase::Submit => {}
            }
            return;
        }
        if state.n > state.last_n {
            DONE.with(|d| d.set(true));
            finish_real(state);
            std::process::exit(0);
        }
        let a = state.frames[state.n - 1].clone();
        let b = state.frames[state.n + 1].clone();
        self.submit_real(cx, state, Some((a, b)), false, 0.5);
        self.apply_ai(cx, state);
        state.phase = RealPhase::Tween;
    }
}
