//! FLOW TEST CLIPS: a deterministic generator for the classic optical-flow
//! debug set, plus a `--check` mode that decodes what it wrote.
//!
//! The VJ player tweens between decoded frames on the GPU. When that tween
//! glitches on a single frame there is nothing in a real clip to tell you
//! whether the tweener, the flow field, or the source footage is at fault —
//! real footage has no ground truth you can see. These eleven clips do: every
//! one of them moves in a way whose correct in-between is obvious to the eye,
//! so a tween failure reads as a stutter, a tear, or a smear against motion
//! you already know is smooth.
//!
//! Three decisions are the reason this file looks the way it does:
//!
//! - **12 fps, exactly.** The gap between two source frames is what the
//!   tweener has to fill; at 60 fps it fills a sliver and every bug hides in
//!   it. At 12 fps against a 120 Hz display the player synthesizes nine
//!   in-betweens per pair, and a single bad one is a visible hitch.
//! - **All-intra.** Decks scratch and play backwards, so every frame must
//!   decode on its own — the same rule `convert.rs` encodes for the same
//!   reason, and not optional here either.
//! - **Subpixel, deliberately.** A rect that jumps a whole number of pixels
//!   per frame is indistinguishable from a rect the tweener snapped: both look
//!   right. So every translating shape sits at a fractional position and its
//!   edge pixels carry proportional coverage. A tweener that rounds shows up
//!   immediately as an edge that pops instead of sliding.
//!
//! Nothing here is random. The one noisy clip uses a fixed LCG seed, so two
//! runs on two machines produce the same pixels.

#[cfg(target_arch = "wasm32")]
fn main() {}

// This clip generator drives native file-video encoders; VJ depends on the library only.
#[cfg(not(target_arch = "wasm32"))]
mod native {
//!
//! ```text
//! cargo run --release -p makepad-video-flow --bin flowtest_gen
//! cargo run --release -p makepad-video-flow --bin flowtest_gen -- --check
//! ```

use makepad_video::{
    nv12, VideoFileCodec, VideoFileDecoder, VideoFileEncoder, VideoFileEncoderOptions,
};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Clip geometry
// ---------------------------------------------------------------------------

const W: usize = 640;
const H: usize = 360;
/// 72 frames at 12 fps = 6.0 seconds.
const FRAMES: usize = 72;
const FPS_NUM: u32 = 12;
const FPS_DEN: u32 = 1;
/// Generous for 640x360: these are debug clips and a blocking artifact on a
/// moving edge would be mistaken for a tween bug.
const BITRATE_BPS: u32 = 20_000_000;

/// The moving square in the rect clips.
const SQ: f32 = 64.0;
/// Vertically centred, so a square never changes row and any vertical wobble
/// under the tween is the tweener's.
const SQ_Y: f32 = (H as f32 - SQ) * 0.5;

// ---------------------------------------------------------------------------
// Canvas: one grayscale plane, coverage-blended
// ---------------------------------------------------------------------------

/// A grayscale frame in display units, 0.0 = black, 1.0 = white. Everything is
/// drawn by *coverage*: a shape whose edge falls a third of the way through a
/// pixel leaves that pixel a third of the way to its colour. That is what
/// makes a 4.125 px/frame slide representable at all.
struct Canvas {
    px: Vec<f32>,
}

impl Canvas {
    fn new() -> Self {
        Self { px: vec![0.0; W * H] }
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
        let p = &mut self.px[y * W + x];
        *p = *p * (1.0 - c) + v * c;
    }

    /// Axis-aligned rectangle `[x0,x1) x [y0,y1)` in float pixel coordinates,
    /// filled with exact per-pixel area coverage. No rounding anywhere.
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, v: f32) {
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let ix0 = (x0.floor() as i64).clamp(0, W as i64) as usize;
        let ix1 = (x1.ceil() as i64).clamp(0, W as i64) as usize;
        let iy0 = (y0.floor() as i64).clamp(0, H as i64) as usize;
        let iy1 = (y1.ceil() as i64).clamp(0, H as i64) as usize;
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

    /// The same rectangle drawn at every horizontal wrap that can touch the
    /// frame, so a shape leaving the right edge reappears on the left with its
    /// subpixel phase intact.
    fn rect_wrap_x(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, v: f32) {
        for k in [-1.0f32, 0.0, 1.0] {
            let off = k * W as f32;
            self.rect(x0 + off, y0, x1 + off, y1, v);
        }
    }

    /// A rotated box, antialiased from its signed distance field: coverage is
    /// `0.5 - d` clamped to 0..1, which is the standard one-pixel box filter
    /// approximation and, unlike supersampling, is *continuous* in the angle.
    /// A rotational tween is exactly the thing that needs subpixel response at
    /// every angle, not 64 quantized levels.
    fn rot_box(&mut self, cx: f32, cy: f32, hw: f32, hh: f32, angle: f32, v: f32) {
        let (s, c) = angle.sin_cos();
        let reach = hw.hypot(hh) + 2.0;
        let ix0 = ((cx - reach).floor() as i64).clamp(0, W as i64) as usize;
        let ix1 = ((cx + reach).ceil() as i64).clamp(0, W as i64) as usize;
        let iy0 = ((cy - reach).floor() as i64).clamp(0, H as i64) as usize;
        let iy1 = ((cy + reach).ceil() as i64).clamp(0, H as i64) as usize;
        for y in iy0..iy1 {
            let py = y as f32 + 0.5 - cy;
            for x in ix0..ix1 {
                let px = x as f32 + 0.5 - cx;
                // Rotate the sample into the box's frame.
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

    /// Display-space RGB8, gray replicated across the three channels.
    fn to_rgb8(&self, out: &mut Vec<u8>) {
        out.clear();
        out.resize(W * H * 3, 0);
        for (i, &v) in self.px.iter().enumerate() {
            let b = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            out[i * 3] = b;
            out[i * 3 + 1] = b;
            out[i * 3 + 2] = b;
        }
    }
}

#[inline]
fn span_overlap(a0: f32, a1: f32, b0: f32, b1: f32) -> f32 {
    (a1.min(b1) - a0.max(b0)).clamp(0.0, 1.0)
}

/// Fold `v` back and forth inside `[lo,hi]` — a triangle wave, which is what a
/// perfectly elastic bounce off two walls actually is.
fn reflect(v: f32, lo: f32, hi: f32) -> f32 {
    let span = hi - lo;
    if span <= 0.0 {
        return lo;
    }
    let t = (v - lo).rem_euclid(2.0 * span);
    lo + if t <= span { t } else { 2.0 * span - t }
}

// ---------------------------------------------------------------------------
// Analytic checkerboard
// ---------------------------------------------------------------------------

/// Integral from 0 to `u` of the unit square wave with half-period `cell`
/// (0 on `[0,cell)`, 1 on `[cell,2*cell)`, ...). Exact for negative `u` too.
fn square_wave_integral(u: f32, cell: f32) -> f32 {
    let period = 2.0 * cell;
    let n = (u / period).floor();
    let rem = u - n * period;
    n * cell + (rem - cell).clamp(0.0, cell)
}

/// Mean of the square wave over `[u0,u1]`.
fn square_wave_mean(u0: f32, u1: f32, cell: f32) -> f32 {
    let d = u1 - u0;
    if d <= 1e-6 {
        let n = (u0 / (2.0 * cell)).floor();
        return if u0 - n * 2.0 * cell >= cell { 1.0 } else { 0.0 };
    }
    ((square_wave_integral(u1, cell) - square_wave_integral(u0, cell)) / d).clamp(0.0, 1.0)
}

/// Fraction of the box `[u0,u1] x [v0,v1]` covered by the light cells of a
/// checkerboard with `cell`-sized squares. The checker is the XOR of two
/// square waves, so its box average is separable and closed-form — which is
/// why this clip has no aliasing to be mistaken for a tween artifact.
fn checker_coverage(u0: f32, u1: f32, v0: f32, v1: f32, cell: f32) -> f32 {
    let a = square_wave_mean(u0, u1, cell);
    let b = square_wave_mean(v0, v1, cell);
    a * (1.0 - b) + (1.0 - a) * b
}

// ---------------------------------------------------------------------------
// Fixed noise texture
// ---------------------------------------------------------------------------

/// One deterministic, wrap-continuous, band-limited noise field. Built once
/// with a fixed LCG and blurred so it has structure at a scale the estimator
/// can lock onto — white noise per pixel would be a texture with no motion
/// signal at all.
fn noise_texture() -> Vec<f32> {
    let mut state: u32 = 0x1234_5678;
    let mut t: Vec<f32> = (0..W * H)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((state >> 8) & 0xffff) as f32 / 65535.0
        })
        .collect();
    // Three wrapping box blurs ~ a Gaussian, and wrapping is what lets the
    // pan tile seamlessly.
    for _ in 0..3 {
        t = box_blur_wrap(&t, 4);
    }
    // Stretch back to a usable contrast range; blurring collapses it hard.
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &v in &t {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let scale = if hi - lo > 1e-6 { 1.0 / (hi - lo) } else { 1.0 };
    for v in &mut t {
        *v = 0.10 + 0.85 * ((*v - lo) * scale);
    }
    t
}

fn box_blur_wrap(src: &[f32], r: usize) -> Vec<f32> {
    let n = (2 * r + 1) as f32;
    let mut tmp = vec![0.0f32; W * H];
    for y in 0..H {
        for x in 0..W {
            let mut sum = 0.0;
            for k in 0..=2 * r {
                sum += src[y * W + (x + W + k - r) % W];
            }
            tmp[y * W + x] = sum / n;
        }
    }
    let mut out = vec![0.0f32; W * H];
    for y in 0..H {
        for x in 0..W {
            let mut sum = 0.0;
            for k in 0..=2 * r {
                sum += tmp[((y + H + k - r) % H) * W + x];
            }
            out[y * W + x] = sum / n;
        }
    }
    out
}

/// Bilinear sample of a wrapping texture — the reason the pan is subpixel and
/// not a per-frame integer shuffle.
fn sample_wrap(tex: &[f32], u: f32, v: f32) -> f32 {
    let x0 = u.floor();
    let y0 = v.floor();
    let fx = u - x0;
    let fy = v - y0;
    let xi = (x0 as i64).rem_euclid(W as i64) as usize;
    let yi = (y0 as i64).rem_euclid(H as i64) as usize;
    let xj = (xi + 1) % W;
    let yj = (yi + 1) % H;
    let a = tex[yi * W + xi];
    let b = tex[yi * W + xj];
    let c = tex[yj * W + xi];
    let d = tex[yj * W + xj];
    (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy
}

// ---------------------------------------------------------------------------
// The clips
// ---------------------------------------------------------------------------

/// Fractional so the square's subpixel phase advances every frame.
const TRANSLATE_V: f32 = 4.125;
const TRANSLATE_X0: f32 = 145.5;

fn draw_rect_translate(c: &mut Canvas, f: usize) {
    c.clear(0.0);
    let x = TRANSLATE_X0 + TRANSLATE_V * f as f32;
    c.rect(x, SQ_Y, x + SQ, SQ_Y + SQ, 1.0);
}

fn draw_rect_occluder(c: &mut Canvas, f: usize) {
    c.clear(0.0);
    let x = 60.5 + 6.25 * f as f32;
    c.rect(x, SQ_Y, x + SQ, SQ_Y + SQ, 1.0);
    // Drawn last, so the square passes BEHIND it.
    c.rect(280.0, 0.0, 360.0, H as f32, 0.45);
}

fn draw_rect_cross(c: &mut Canvas, f: usize) {
    c.clear(0.0);
    let t = f as f32;
    let xg = 536.5 - 7.0 * t;
    c.rect(xg, SQ_Y, xg + SQ, SQ_Y + SQ, 0.75);
    // White last: at the crossing the white square is in front, so there is
    // one right answer for what the tween should show.
    let xw = 40.5 + 7.0 * t;
    c.rect(xw, SQ_Y, xw + SQ, SQ_Y + SQ, 1.0);
}

fn draw_rect_diagonal(c: &mut Canvas, f: usize) {
    c.clear(0.0);
    let t = f as f32;
    let x = 40.5 + 4.0 * t;
    let y = reflect(20.5 + 7.5 * t, 0.0, H as f32 - SQ);
    c.rect(x, y, x + SQ, y + SQ, 1.0);
}

fn draw_rect_accelerate(c: &mut Canvas, f: usize) {
    c.clear(0.0);
    let t = f as f32;
    // Constant acceleration, reaching 10 px/frame on the last frame.
    let a = 10.0 / (FRAMES - 1) as f32;
    let x = 30.5 + 0.5 * a * t * t;
    c.rect(x, SQ_Y, x + SQ, SQ_Y + SQ, 1.0);
}

fn draw_rotate_bar(c: &mut Canvas, f: usize) {
    c.clear(0.0);
    let angle = std::f32::consts::TAU * f as f32 / FRAMES as f32;
    c.rot_box(W as f32 * 0.5, H as f32 * 0.5, 100.0, 12.0, angle, 1.0);
}

fn draw_zoom_checker(c: &mut Canvas, f: usize) {
    let s = 1.0 + 0.6 * (f as f32 / (FRAMES - 1) as f32);
    let cx = W as f32 * 0.5;
    let cy = H as f32 * 0.5;
    let cell = 32.0;
    for y in 0..H {
        let v0 = (y as f32 - cy) / s + cy;
        let v1 = ((y + 1) as f32 - cy) / s + cy;
        for x in 0..W {
            let u0 = (x as f32 - cx) / s + cx;
            let u1 = ((x + 1) as f32 - cx) / s + cx;
            let cov = checker_coverage(u0, u1, v0, v1, cell);
            c.px[y * W + x] = 0.22 + 0.56 * cov;
        }
    }
}

fn draw_pan_texture(c: &mut Canvas, tex: &[f32], f: usize) {
    let t = f as f32;
    let ox = 3.25 * t;
    let oy = 1.75 * t;
    for y in 0..H {
        for x in 0..W {
            c.px[y * W + x] = sample_wrap(tex, x as f32 + ox, y as f32 + oy);
        }
    }
}

const LINE_SPEEDS: [f32; 3] = [1.0, 3.0, 7.0];
const LINE_STARTS: [f32; 3] = [60.5, 260.5, 460.5];

fn draw_thin_lines(c: &mut Canvas, f: usize) {
    c.clear(0.0);
    for i in 0..3 {
        let x = (LINE_STARTS[i] + LINE_SPEEDS[i] * f as f32).rem_euclid(W as f32);
        c.rect_wrap_x(x, 0.0, x + 2.0, H as f32, 1.0);
    }
}

fn draw_scene_cut(c: &mut Canvas, tex: &[f32], f: usize) {
    if f < FRAMES / 2 {
        draw_rect_translate(c, f);
    } else {
        draw_pan_texture(c, tex, f - FRAMES / 2);
    }
}

fn draw_sine_ease(c: &mut Canvas, f: usize) {
    c.clear(0.0);
    // Two full periods across the clip.
    let phase = std::f32::consts::TAU * 2.0 * f as f32 / FRAMES as f32;
    let x = W as f32 * 0.5 - SQ * 0.5 + 96.0 * phase.sin();
    c.rect(x, SQ_Y, x + SQ, SQ_Y + SQ, 1.0);
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

struct Clip {
    name: &'static str,
    /// One line for the console and the README index.
    what: &'static str,
}

const CLIPS: &[Clip] = &[
    Clip { name: "rect_translate", what: "64px square, constant 4.125 px/frame right" },
    Clip { name: "rect_occluder", what: "square passing behind a static gray bar" },
    Clip { name: "rect_cross", what: "white and 75% gray squares crossing" },
    Clip { name: "rect_diagonal", what: "square moving diagonally, one edge bounce" },
    Clip { name: "rect_accelerate", what: "square accelerating from rest to 10 px/frame" },
    Clip { name: "rotate_bar", what: "200x24 bar, one full turn about centre" },
    Clip { name: "zoom_checker", what: "32px checkerboard zooming 1.0x -> 1.6x" },
    Clip { name: "pan_texture", what: "fixed smooth noise panning (3.25, 1.75) px/frame" },
    Clip { name: "thin_lines", what: "three 2px lines at 1 / 3 / 7 px/frame" },
    Clip { name: "scene_cut", what: "rect_translate, hard cut at frame 36 to pan_texture" },
    Clip { name: "sine_ease", what: "square on a sine, two periods, +-96px" },
];

fn render(name: &str, f: usize, c: &mut Canvas, tex: &[f32]) {
    match name {
        "rect_translate" => draw_rect_translate(c, f),
        "rect_occluder" => draw_rect_occluder(c, f),
        "rect_cross" => draw_rect_cross(c, f),
        "rect_diagonal" => draw_rect_diagonal(c, f),
        "rect_accelerate" => draw_rect_accelerate(c, f),
        "rotate_bar" => draw_rotate_bar(c, f),
        "zoom_checker" => draw_zoom_checker(c, f),
        "pan_texture" => draw_pan_texture(c, tex, f),
        "thin_lines" => draw_thin_lines(c, f),
        "scene_cut" => draw_scene_cut(c, tex, f),
        "sine_ease" => draw_sine_ease(c, f),
        other => panic!("unknown clip {other}"),
    }
}

fn encode_clip(path: &Path, name: &str, tex: &[f32]) -> Result<u64, String> {
    let out = path
        .to_str()
        .ok_or_else(|| format!("non-utf8 output path: {}", path.display()))?;
    let mut encoder = VideoFileEncoder::new(
        out,
        VideoFileEncoderOptions {
            codec: VideoFileCodec::H264,
            width: W as u32,
            height: H as u32,
            fps_num: FPS_NUM,
            fps_den: FPS_DEN,
            video_bitrate_bps: BITRATE_BPS,
            audio: None,
            // Decks play these backwards; every frame decodes on its own.
            keyframe_only: true,
        },
    )
    .map_err(|e| format!("encode open {}: {e}", path.display()))?;

    let mut canvas = Canvas::new();
    let mut rgb: Vec<u8> = Vec::new();
    for f in 0..FRAMES {
        render(name, f, &mut canvas, tex);
        canvas.to_rgb8(&mut rgb);
        encoder
            .push_frame_rgb8(&rgb, None)
            .map_err(|e| format!("encode frame {f} of {name}: {e}"))?;
    }
    encoder
        .finish()
        .map_err(|e| format!("encode finish {name}: {e}"))?;
    let bytes = std::fs::metadata(path)
        .map_err(|e| format!("stat {}: {e}", path.display()))?
        .len();
    Ok(bytes)
}

/// A frame reduced to three numbers: what it looks like, where its mass is,
/// and whether it is byte-for-byte the frame before it.
struct FrameStat {
    mean: f64,
    centroid_x: f64,
    hash: u64,
}

fn frame_stat(rgb: &[u8], w: usize) -> FrameStat {
    let mut sum = 0f64;
    let mut wsum = 0f64;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (i, chunk) in rgb.chunks_exact(3).enumerate() {
        let l = chunk[0] as f64;
        sum += l;
        wsum += l * (i % w) as f64;
        hash = (hash ^ chunk[0] as u64).wrapping_mul(0x1000_0000_01b3);
    }
    FrameStat {
        mean: sum / (rgb.len() / 3) as f64,
        centroid_x: if sum > 0.0 { wsum / sum } else { -1.0 },
        hash,
    }
}

/// Reopen everything we wrote and prove it decodes. File size is a weak
/// witness here — a white square on flat black is ~170 bytes a frame however
/// many bits you offer the encoder — so the real gate is structural: the full
/// frame count comes back, the geometry and cadence are what we asked for, and
/// consecutive frames are *different*, which is the one property a broken
/// generator would violate. The first three frames go through the same
/// NV12 -> RGB path the converter uses.
fn check_dir(dir: &Path) -> Result<(), String> {
    let mut failures = 0usize;
    for clip in CLIPS {
        let path = dir.join(format!("{}.mp4", clip.name));
        let p = path.to_str().ok_or("non-utf8 path")?;
        let bytes = std::fs::metadata(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        let mut dec = match VideoFileDecoder::open(p) {
            Ok(d) => d,
            Err(e) => {
                println!("  FAIL {:<16} decode open: {e}", clip.name);
                failures += 1;
                continue;
            }
        };
        let info = dec.info().clone();
        let mut frames = 0usize;
        let mut stats: Vec<FrameStat> = Vec::new();
        let mut heads = Vec::new();
        let mut rgb = Vec::new();
        let mut decode_error = false;
        loop {
            let frame = match dec.next_frame() {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => {
                    println!("  FAIL {:<16} decode frame {frames}: {e}", clip.name);
                    decode_error = true;
                    break;
                }
            };
            if frames < 3 {
                nv12::nv12_to_rgb8(&frame.nv12, frame.width, frame.height, &mut rgb);
                let st = frame_stat(&rgb, frame.width as usize);
                heads.push(format!(
                    "{}x{} mean {:.1} cx {:.1}",
                    frame.width, frame.height, st.mean, st.centroid_x
                ));
                stats.push(st);
            }
            frames += 1;
        }
        let moves = stats.len() == 3 && stats[0].hash != stats[1].hash && stats[1].hash != stats[2].hash;
        let ok = !decode_error
            && frames == FRAMES
            && info.width == W as u32
            && info.height == H as u32
            && info.fps_den > 0
            && info.fps_num == FPS_NUM * info.fps_den / FPS_DEN
            && bytes >= 8 * 1024
            && moves;
        if !ok {
            failures += 1;
        }
        println!(
            "  {} {:<16} {}x{} {}/{} fps  {} frames  {:>7} KB  moves: {}  first3: [{}]",
            if ok { "ok  " } else { "FAIL" },
            clip.name,
            info.width,
            info.height,
            info.fps_num,
            info.fps_den,
            frames,
            bytes / 1024,
            if moves { "yes" } else { "NO" },
            heads.join(" | ")
        );
        if ok && bytes < 50 * 1024 {
            println!(
                "       note: {} KB — flat-black content, the encoder needs no more bits",
                bytes / 1024
            );
        }
    }
    if failures > 0 {
        return Err(format!("{failures} clip(s) failed the check"));
    }
    Ok(())
}

/// Print the decoded luma either side of the first rising edge on the centre
/// row, for the first few frames of one clip.
///
/// This is the assertion the whole set rests on and the one that is easiest to
/// lose silently: the generator writes a fractional edge as a proportional
/// gray, and a mean-quality encoder is entirely capable of snapping that ramp
/// back to a hard black/white step. If it did, every rect clip would be a
/// whole-pixel test wearing a subpixel label, and a tweener that rounds would
/// pass. So: look at the ramp, in the decoded file, with your own eyes.
/// `NAME` or `NAME@FRAME` — where in the clip to start looking.
fn probe_clip(dir: &Path, spec: &str) -> Result<(), String> {
    let (name, start) = match spec.split_once('@') {
        Some((n, f)) => (n, f.parse::<usize>().map_err(|e| format!("bad frame: {e}"))?),
        None => (spec, 0),
    };
    let path = dir.join(format!("{name}.mp4"));
    let p = path.to_str().ok_or("non-utf8 path")?;
    let mut dec = VideoFileDecoder::open(p).map_err(|e| format!("open {name}: {e}"))?;
    println!("probe {name}: frames {start}..{}", start + 4);
    for f in 0..start + 4 {
        let Some(frame) = dec.next_frame().map_err(|e| format!("decode: {e}"))? else {
            break;
        };
        if f < start {
            continue;
        }
        let w = frame.width as usize;
        let h = frame.height as usize;
        let y = &frame.nv12[..w * h];
        println!("  frame {f}:");
        for line in ascii_thumb(y, w, h) {
            println!("    |{line}|");
        }
        // Y is limited range: 16 is black.
        let row = &y[(h / 2) * w..][..w];
        match (1..w).find(|&x| row[x] > 32 && row[x - 1] <= 32) {
            Some(edge) => {
                let lo = edge.saturating_sub(3);
                let hi = (edge + 5).min(w);
                let vals: Vec<String> = row[lo..hi].iter().map(|v| format!("{v:>3}")).collect();
                println!("    centre row x={lo}..{hi}: [{}]", vals.join(" "));
            }
            None => println!("    centre row: no rising edge"),
        }
    }
    Ok(())
}

/// A 40x15 luma thumbnail in ASCII — enough to see that the square is where
/// you meant it and the occluder is in front of it, without a display.
fn ascii_thumb(y: &[u8], w: usize, h: usize) -> Vec<String> {
    const RAMP: &[u8] = b" .:-=+*#%@";
    let (tw, th) = (40usize, 15usize);
    (0..th)
        .map(|ty| {
            (0..tw)
                .map(|tx| {
                    let x0 = tx * w / tw;
                    let x1 = ((tx + 1) * w / tw).max(x0 + 1);
                    let y0 = ty * h / th;
                    let y1 = ((ty + 1) * h / th).max(y0 + 1);
                    let mut sum = 0u32;
                    for sy in y0..y1 {
                        for sx in x0..x1 {
                            sum += y[sy * w + sx] as u32;
                        }
                    }
                    let mean = sum / ((x1 - x0) * (y1 - y0)) as u32;
                    // Undo limited range so 16..235 spans the ramp.
                    let t = (mean.saturating_sub(16) * (RAMP.len() as u32 - 1)) / 219;
                    RAMP[(t as usize).min(RAMP.len() - 1)] as char
                })
                .collect()
        })
        .collect()
}

fn default_out_dir() -> PathBuf {
    // Anchored on the crate, not the shell: `cargo run` from anywhere in the
    // tree must write the same directory.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/flowtest")
}

pub(super) fn run() {
    let mut out_dir = default_out_dir();
    let mut check_only = false;
    let mut probe: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check_only = true,
            "--probe" => match args.next() {
                Some(n) => probe = Some(n),
                None => {
                    eprintln!("--probe needs a clip name");
                    std::process::exit(2);
                }
            },
            "--out" => match args.next() {
                Some(d) => out_dir = PathBuf::from(d),
                None => {
                    eprintln!("--out needs a directory");
                    std::process::exit(2);
                }
            },
            "-h" | "--help" => {
                println!("flowtest_gen [--out DIR] [--check] [--probe CLIP]");
                println!("  generates {FRAMES} frame {W}x{H} {FPS_NUM} fps flow test clips");
                println!("  --check       decode every clip back and verify it");
                println!("  --probe CLIP  print decoded luma across one clip's moving edge");
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    if let Some(name) = probe {
        if let Err(e) = probe_clip(&out_dir, &name) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    if !check_only {
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            eprintln!("create {}: {e}", out_dir.display());
            std::process::exit(1);
        }
        println!("generating into {}", out_dir.display());
        let tex = noise_texture();
        let started = std::time::Instant::now();
        for clip in CLIPS {
            let path = out_dir.join(format!("{}.mp4", clip.name));
            let t0 = std::time::Instant::now();
            match encode_clip(&path, clip.name, &tex) {
                Ok(bytes) => println!(
                    "  {:<16} {:>7} KB  {:>5} ms   {}",
                    clip.name,
                    bytes / 1024,
                    t0.elapsed().as_millis(),
                    clip.what
                ),
                Err(e) => {
                    eprintln!("  {:<16} FAILED: {e}", clip.name);
                    std::process::exit(1);
                }
            }
        }
        println!("{} clips in {:.1}s", CLIPS.len(), started.elapsed().as_secs_f64());
    }

    println!("checking {}", out_dir.display());
    if let Err(e) = check_dir(&out_dir) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    println!("all {} clips decode", CLIPS.len());
}

}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::run();
}
