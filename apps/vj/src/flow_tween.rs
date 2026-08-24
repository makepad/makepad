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

use crate::media::tl_on;

/// THE NEURAL FIELD PRODUCER: one background worker owning the RIFE
/// runtime. Jobs are (generation, pair, two RGB8 proxies); results are
/// the net's intermediate flow + occlusion mask. The worker keeps only
/// the NEWEST job (a busy pair is simply skipped — the classical fields
/// cover it), so it can never fall behind the transport.
pub struct RifeService {
    tx: std::sync::mpsc::SyncSender<RifeJob>,
    result: std::sync::Arc<std::sync::Mutex<Option<RifeField>>>,
}

pub struct RifeJob {
    pub generation: u64,
    pub pair: usize,
    pub rgb0: Vec<u8>,
    pub rgb1: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

pub struct RifeField {
    pub generation: u64,
    pub pair: usize,
    pub width: usize,
    pub height: usize,
    /// Packed RGBA per proxy pixel: t->frame0 xy, t->frame1 xy (pixels).
    pub flow: Vec<f32>,
    /// Post-sigmoid occlusion mask, 0..1.
    pub mask: Vec<f32>,
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
        std::thread::Builder::new()
            .name("vj-rife".into())
            .spawn(move || {
                let rife = Rife::from_model_weights_scaled(
                    model,
                    RifeBackendKind::Device,
                    RifeScale::Half,
                );
                while let Ok(job) = rx.recv() {
                    let Ok(pair) = RifeFramePair::new(
                        &job.rgb0, &job.rgb1, job.width, job.height,
                    ) else {
                        continue;
                    };
                    let t0 = std::time::Instant::now();
                    let Ok(field) = rife.flow_field_rgb8(pair, 0.5, None) else {
                        continue;
                    };
                    // Repack planar [4, plane] + mask into interleaved
                    // RGBA texels for the warp texture.
                    let plane = job.width * job.height;
                    let mut flow = vec![0.0f32; plane * 4];
                    for i in 0..plane {
                        flow[i * 4] = field.flow[i];
                        flow[i * 4 + 1] = field.flow[plane + i];
                        flow[i * 4 + 2] = field.flow[2 * plane + i];
                        flow[i * 4 + 3] = field.flow[3 * plane + i];
                    }
                    if tl_on() {
                        eprintln!(
                            "tl rife pair={} {}x{} in {:.0}ms",
                            job.pair,
                            job.width,
                            job.height,
                            t0.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    *out.lock().unwrap() = Some(RifeField {
                        generation: job.generation,
                        pair: job.pair,
                        width: job.width,
                        height: job.height,
                        flow,
                        mask: field.mask,
                    });
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, result })
    }

    /// Offer a pair; a busy worker skips it (classical covers the gap).
    pub fn offer(&self, job: RifeJob) {
        let _ = self.tx.try_send(job);
    }

    pub fn take(&self) -> Option<RifeField> {
        self.result.lock().unwrap().take()
    }
}

/// `VJ_TWEEN_DEBUG=1|2|3` turns the warp into a diagnostic view (flow
/// field / frame A passthrough / t ramp).
fn tween_debug() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("VJ_TWEEN_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0)
    })
}

/// Pyramid depth: level 0 is the flow grid (quarter of source), each
/// further level halves. 4 levels reach +-~40 grid cells of motion after
/// refinement — 160 source pixels, plenty for adjacent frames.
pub const LEVELS: usize = 4;
/// Jacobi sweeps per level (the CPU default).
pub const SWEEPS: usize = 3;

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
        // 4:1 area reduction of both Y planes via four bilinear taps at
        // +-1 source texel (linear filtering makes each tap a 2x2 avg, so
        // the four together cover the 4x4 block).
        pixel: fn() {
            let o = self.inv_grid
            let mut a = 0.0
            let mut b = 0.0
            let t00 = self.pos + vec2(-o.x, -o.y)
            let t10 = self.pos + vec2(o.x, -o.y)
            let t01 = self.pos + vec2(-o.x, o.y)
            let t11 = self.pos + vec2(o.x, o.y)
            a = a + self.tex_y_a.sample(t00).x + self.tex_y_a.sample(t10).x
            a = a + self.tex_y_a.sample(t01).x + self.tex_y_a.sample(t11).x
            b = b + self.tex_y_b.sample(t00).x + self.tex_y_b.sample(t10).x
            b = b + self.tex_y_b.sample(t01).x + self.tex_y_b.sample(t11).x
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
        // neighbours (the Jacobi read side), lambda luma units per cell.
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
            return self.sad(d) + self.lambda * self.smooth(d)
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
            let y = (self.tex_y_a.sample(c).x * 255.0 - 16.0) / 219.0
            let u2 = self.tex_uv_a.sample(c).xy
            let u = (u2.x * 255.0 - 128.0) / 224.0
            let v = (u2.y * 255.0 - 128.0) / 224.0
            return vec3(y + 1.5748 * v, y - 0.1873 * u - 0.4681 * v, y + 1.8556 * u)
        }
        nv12_b: fn(uv: vec2) -> vec3 {
            let c = clamp(uv, vec2(0.0, 0.0), vec2(1.0, 1.0))
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
    /// NV12 planes for the pair: (y, uv) for A then B.
    #[rust]
    planes: Option<[Texture; 4]>,
    #[rust]
    size: (u32, u32),
    /// Luma pyramid targets (RG in RGBA32F), one per level.
    #[rust]
    luma_tex: Vec<Texture>,
    /// Flow scratch: ping, pong, and the per-level median output.
    #[rust]
    scratch: Vec<Texture>,
    /// Final per-direction fields at level 0.
    #[rust]
    field_tex: Vec<Texture>,
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
}

impl FlowTweenView {
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
        let (w, h) = (width as usize, height as usize);
        if w < 8 || h < 8 || a.len() < w * h * 3 / 2 || b.len() < w * h * 3 / 2 {
            return;
        }
        if self.size != (width, height) || self.planes.is_none() {
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
            self.planes = Some([mk_y(cx), mk_uv(cx), mk_y(cx), mk_uv(cx)]);
            self.size = (width, height);
        }
        let planes = self.planes.as_ref().unwrap();
        for (tex, (data, len)) in [
            (&planes[0], (a, w * h)),
            (&planes[1], (a, 0)),
            (&planes[2], (b, w * h)),
            (&planes[3], (b, 0)),
        ] {
            let mut buf = tex.take_vec_u8(cx);
            buf.clear();
            if len > 0 {
                buf.extend_from_slice(&data[..len]);
            } else {
                buf.extend_from_slice(&data[w * h..w * h + (w / 2) * (h / 2) * 2]);
            }
            tex.put_back_vec_u8(cx, buf, None);
        }
        self.flow_dirty = true;
        self.area.redraw(cx);
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
        self.size = (0, 0);
        self.flow_dirty = false;
        self.rendered = false;
        self.rife_pair = None;
        self.area.redraw(cx);
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
        self.area.redraw(cx);
    }

    /// The pair the tween view is currently showing (for job scheduling).
    pub fn rife_field_pair(&self) -> Option<usize> {
        self.rife_pair
    }

    /// Drop the neural field (pair changed before a fresh one arrived).
    pub fn clear_rife_field(&mut self, cx: &mut Cx) {
        if self.rife_pair.take().is_some() {
            self.area.redraw(cx);
        }
    }

    pub fn has_pair(&self) -> bool {
        self.planes.is_some()
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
            "fwd" => self.field_tex.first().cloned(),
            "bwd" => self.field_tex.get(1).cloned(),
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
        self.field_tex = (0..2).map(|_| float_tex(cx)).collect();
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
        let Some(planes) = self.planes.clone() else {
            return DrawStep::done();
        };
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
        let total = if self.flow_dirty {
            if luma_only {
                LEVELS + 1 + 1
            } else {
                LEVELS + 2 * (1 + LEVELS * (SWEEPS + 1) + 1) + 1
            }
        } else {
            1
        };
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
            }};
        }
        if self.flow_dirty {
            self.flow_dirty = false;
            // ---- luma pyramid --------------------------------------------
            let (w, h) = (self.size.0 as usize, self.size.1 as usize);
            self.draw_luma.inv_grid = vec2(1.0 / w as f32, 1.0 / h as f32);
            self.draw_luma.draw_vars.set_texture(0, &planes[0]);
            self.draw_luma.draw_vars.set_texture(1, &planes[2]);
            let luma0 = self.luma_tex[0].clone();
            let draw_luma = &mut self.draw_luma;
            run_stage!(&luma0, gw, gh, |cx: &mut Cx2d, r| draw_luma.draw_abs(cx, r));
            for level in 1..LEVELS {
                let (lw, lh) = self.level_dims(level);
                // Bisect probe: TWEEN_PYR_FROM_L0=1 makes every level
                // downsample straight from L0 (correctness of each STAGE
                // isolated from its predecessor's content).
                let src = if std::env::var_os("TWEEN_PYR_FROM_L0").is_some() {
                    self.luma_tex[0].clone()
                } else {
                    self.luma_tex[level - 1].clone()
                };
                let dst = self.luma_tex[level].clone();
                self.draw_halve.draw_vars.set_texture(0, &src);
                let draw_halve = &mut self.draw_halve;
                run_stage!(&dst, lw, lh, |cx: &mut Cx2d, r| draw_halve.draw_abs(cx, r));
            }
            if luma_only {
                // dbg 4: copy luma L0 into the forward-field slot so the
                // warp's debug branch can show it.
                let src = self.luma_tex[0].clone();
                let dst = self.field_tex[0].clone();
                self.draw_halve.draw_vars.set_texture(0, &src);
                let draw_halve = &mut self.draw_halve;
                run_stage!(&dst, gw, gh, |cx: &mut Cx2d, r| draw_halve.draw_abs(cx, r));
            }
            // ---- both one-way fields -------------------------------------
            for dir in 0..2 {
                if luma_only {
                    break;
                }
                let dirf = dir as f32;
                // Exhaustive seed at the coarsest level.
                let (tw, th) = self.level_dims(LEVELS - 1);
                let top_luma = self.luma_tex[LEVELS - 1].clone();
                self.draw_exhaust.dir = dirf;
                self.draw_exhaust.inv_size = vec2(1.0 / tw as f32, 1.0 / th as f32);
                self.draw_exhaust.draw_vars.set_texture(0, &top_luma);
                let seed = self.scratch[2].clone();
                let draw_exhaust = &mut self.draw_exhaust;
                run_stage!(&seed, tw, th, |cx: &mut Cx2d, r| draw_exhaust.draw_abs(cx, r));
                // Coarse → fine: SWEEPS ping-pong sweeps then a median.
                let mut prev = self.scratch[2].clone();
                let mut prev_scale = 1.0f32;
                for level in (0..LEVELS).rev() {
                    let (lw, lh) = self.level_dims(level);
                    let luma = self.luma_tex[level].clone();
                    for s in 0..SWEEPS {
                        let target = self.scratch[s & 1].clone();
                        self.draw_sweep.dir = dirf;
                        self.draw_sweep.inv_size = vec2(1.0 / lw as f32, 1.0 / lh as f32);
                        self.draw_sweep.prev_scale = prev_scale;
                        self.draw_sweep.draw_vars.set_texture(0, &luma);
                        self.draw_sweep.draw_vars.set_texture(1, &prev);
                        let draw_sweep = &mut self.draw_sweep;
                        run_stage!(&target, lw, lh, |cx: &mut Cx2d, r| draw_sweep
                            .draw_abs(cx, r));
                        prev = target;
                        prev_scale = 1.0;
                    }
                    // Median into scratch[2] (or the FINAL field at L0 —
                    // the subpel pass below reads it from there).
                    let med_target = self.scratch[2].clone();
                    self.draw_median.inv_size = vec2(1.0 / lw as f32, 1.0 / lh as f32);
                    self.draw_median.draw_vars.set_texture(0, &prev);
                    let draw_median = &mut self.draw_median;
                    run_stage!(&med_target, lw, lh, |cx: &mut Cx2d, r| draw_median
                        .draw_abs(cx, r));
                    prev = med_target;
                    // The next (finer) level reads this field doubled.
                    prev_scale = 2.0;
                }
                // Sub-pixel parabola into the final field texture.
                let field = self.field_tex[dir].clone();
                let luma0 = self.luma_tex[0].clone();
                self.draw_subpel.dir = dirf;
                self.draw_subpel.inv_size = vec2(1.0 / gw as f32, 1.0 / gh as f32);
                self.draw_subpel.draw_vars.set_texture(0, &luma0);
                self.draw_subpel.draw_vars.set_texture(1, &prev);
                let draw_subpel = &mut self.draw_subpel;
                run_stage!(&field, gw, gh, |cx: &mut Cx2d, r| draw_subpel.draw_abs(cx, r));
            }
        }
        // ---- the warp, every display frame ------------------------------
        let (w, h) = (self.size.0, self.size.1);
        self.draw_warp.dbg = self.dbg_override.unwrap_or_else(tween_debug);
        self.draw_warp.t_pair = self.t;
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
                self.draw_warp.draw_vars.set_texture(6, &self.field_tex[0]);
                self.draw_warp.draw_vars.set_texture(7, &self.field_tex[1]);
            }
        }
        self.draw_warp.inv_grid = vec2(1.0 / gw as f32, 1.0 / gh as f32);
        self.draw_warp.draw_vars.set_texture(0, &planes[0]);
        self.draw_warp.draw_vars.set_texture(1, &planes[1]);
        self.draw_warp.draw_vars.set_texture(2, &planes[2]);
        self.draw_warp.draw_vars.set_texture(3, &planes[3]);
        self.draw_warp.draw_vars.set_texture(4, &self.field_tex[0]);
        self.draw_warp.draw_vars.set_texture(5, &self.field_tex[1]);
        let warp_out = self.warp_out.clone();
        let draw_warp = &mut self.draw_warp;
        run_stage!(&warp_out, w, h, |cx: &mut Cx2d, r| draw_warp.draw_abs(cx, r));
        self.rendered = true;
        DrawStep::done()
    }
}
