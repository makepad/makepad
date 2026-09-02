//! FLOW-WARP PLAYBACK: arbitrary-framerate, bounce-looping, speed-warped
//! video playback on the GPU, driven by the RIFE motion fields the enhance
//! service embeds in its mp4s (the trailing `mkfl` box — see `crate::flow`).
//!
//! The pieces:
//!
//! - [`prepare_flow_clip`] (decode worker): parses the clip's `mkfl` box and,
//!   when the clip fits the cache budget, decodes the WHOLE clip once,
//!   keeping only the flow-pair ENDPOINT frames as BGRA byte buffers. A
//!   tweened 2N-1-frame clip carries flow for its N source frames (pair i
//!   spans video frames i·stride .. (i+1)·stride), so only every stride-th
//!   frame is kept — the baked in-betweens are exactly what the warp shader
//!   re-synthesizes at any t.
//! - [`FlowWarpView`] (a 4×4 offscreen widget, the `VjMeshView` pattern):
//!   owns a free-running playback position in FRAME-PAIR space
//!   (`position += rate · pairs_per_sec · dt`, any float rate, negative
//!   fine), triangle-wave BOUNCE between 0 and the last pair, and a child
//!   DrawPass that renders the warped in-between into a video-resolution
//!   texture. That texture replaces the slot's decoder texture upstream;
//!   everything downstream (mixer, fx, program out) is untouched.
//!
//! Texture packing (one RGBA texel per flow-grid cell, two textures):
//!
//! - `tex_flow`  grid_w × grid_h — R=f0x G=f0y B=f1x A=f1y, each i8 biased
//!   by +128 into u8 (`byte ^ 0x80`). Linear filtering interpolates the
//!   biased vectors correctly (the bias is affine).
//! - `tex_mask`  grid_w × grid_h — the u8 occlusion mask replicated into
//!   RGB (255 = the intermediate takes frame0's warp).
//!
//! Flow units: stored vectors are quarter-pixel units AT GRID resolution
//! (grid = quarter of source), so `uv_offset = i8 / (4 · grid_dim)` — which
//! is also exactly source-pixels over source-size, resolution independent
//! (the same field serves the 2× upscaled clip untouched).

use crate::flow::FlowMap;
use makepad_widgets::makepad_platform::video_file::{nv12, VideoFileDecoder};
use makepad_widgets::*;
use std::path::Path;
use crate::clock::Instant;

/// Decoded-endpoint cache ceiling (BGRA bytes). A VJ deck lives or dies by
/// this ceiling: a clip UNDER it gets the whole warp instrument (any-rate
/// bidirectional clock, scratch, the fixed-sweep beat law); a clip over it
/// falls to plain streaming where none of that exists. So it is sized for
/// the performance machine, not the minimum: 4 GB holds a 1920×1080 flow
/// clip of ~480 endpoints (8.3 MB each) or ~40 s of 720p endpoints.
/// Anything bigger logs and plays as plain video.
pub const MAX_FLOW_CACHE_BYTES: usize = if usize::BITS >= 64 {
    4_294_967_296_u64
} else {
    (usize::MAX - 1) as u64
} as usize;

/// Largest mp4 the `mkfl` scan will lift into memory to parse the box walk.
pub const MAX_FLOW_SCAN_BYTES: u64 = 256 * 1024 * 1024;

/// `VJ_TL=1` timeline trace — the same switch as `crate::media::tl_on`,
/// duplicated because this file also compiles standalone in the
/// flow_warp_lab example.
fn tl_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VJ_TL").is_some())
}

/// One clip fully prepared for warp playback: pair-endpoint frames (BGRA),
/// the parsed motion payload, and the pair-space clock rate.
pub struct FlowClipData {
    /// `pairs + 1` frames; index i is the FIRST frame of pair i.
    pub frames: Vec<Vec<u32>>,
    /// Usable pair count: normally `map.pairs`, one less when the platform
    /// decoder withheld the clip's final frame (a container that declares
    /// its duration one tick short cuts the last sample — the gate clip
    /// does exactly this) — the tail pair is then dropped rather than
    /// warped against a frame that never arrived.
    pub pairs: u32,
    pub width: usize,
    pub height: usize,
    /// Video frames between consecutive endpoints (1 = every frame is an
    /// endpoint; 2 = a tweened clip whose odd frames are baked mids).
    pub stride: usize,
    /// Pairs traversed per second at rate 1.0 — the SOURCE cadence
    /// (`video_fps / stride`), so rate 1.0 matches the clip's natural speed.
    pub pairs_per_sec: f64,
    pub duration_secs: f64,
    pub map: FlowMap,
}

impl FlowClipData {
    pub fn aspect(&self) -> f32 {
        self.width.max(1) as f32 / self.height.max(1) as f32
    }
}

/// Endpoint stride from the video's frame count and the payload's pair
/// count: `frames - 1` consecutive-frame gaps must divide evenly onto
/// `pairs` flow pairs. 281 frames / 140 pairs → 2; 141 / 140 → 1; anything
/// that does not divide is refused (never guess a mapping).
pub fn endpoint_stride(frames: u64, pairs: u32) -> Option<usize> {
    if pairs == 0 || frames < 2 {
        return None;
    }
    let gaps = frames - 1;
    let pairs = pairs as u64;
    if gaps % pairs != 0 {
        return None;
    }
    Some((gaps / pairs) as usize)
}

/// Advance a pair-space position by `delta` (already rate·pps·dt, signed)
/// WITHIN the window [lo, hi] — the trim brackets mapped into pair space.
/// Loop mode wraps over the window; bounce reflects at both of its ends,
/// flipping `dir` (the triangle wave). A position outside the window (a
/// live trim just moved it) is folded in first. Returns (position, dir).
pub fn advance_position(
    pos: f64,
    dir: f64,
    delta: f64,
    lo: f64,
    hi: f64,
    bounce: bool,
) -> (f64, f64) {
    let span = hi - lo;
    if !(span > 0.0) {
        return (lo.max(0.0), 1.0);
    }
    let pos = pos.clamp(lo, hi);
    if !bounce {
        return (lo + (pos - lo + delta).rem_euclid(span), dir);
    }
    let mut p = pos + delta * dir;
    let mut dir = dir;
    // A huge delta on a tiny window can overshoot several reflections.
    for _ in 0..16 {
        if p < lo {
            p = 2.0 * lo - p;
            dir = -dir;
        } else if p > hi {
            p = 2.0 * hi - p;
            dir = -dir;
        } else {
            break;
        }
    }
    (p.clamp(lo, hi), dir)
}

/// Pack one pair's planar samples (4 planes i8 flow, 1 plane u8 mask; see
/// `FlowMap::pair`) into the two texel layouts documented at module top.
pub fn pack_flow_texels(pair: &[u8], grid_w: usize, grid_h: usize) -> (Vec<u32>, Vec<u32>) {
    let n = grid_w * grid_h;
    debug_assert_eq!(pair.len(), n * 5);
    let (f0x, rest) = pair.split_at(n);
    let (f0y, rest) = rest.split_at(n);
    let (f1x, rest) = rest.split_at(n);
    let (f1y, mask) = rest.split_at(n);
    let mut flow = Vec::with_capacity(n);
    let mut mask_px = Vec::with_capacity(n);
    for i in 0..n {
        // i8 two's-complement byte → +128-biased u8 is a plain sign-bit flip.
        let (r, g, b, a) = (
            (f0x[i] ^ 0x80) as u32,
            (f0y[i] ^ 0x80) as u32,
            (f1x[i] ^ 0x80) as u32,
            (f1y[i] ^ 0x80) as u32,
        );
        flow.push((a << 24) | (r << 16) | (g << 8) | b);
        let m = mask[i] as u32;
        mask_px.push(0xff00_0000 | (m << 16) | (m << 8) | m);
    }
    (flow, mask_px)
}

/// The RIFE-style fusion weights the shader uses, replicated for tests:
/// `w_a = (1-t)(m+ε)`, `w_b = t(1-m+ε)` — exact endpoints at t=0/1, the
/// mask fusion `m·A + (1-m)·B` at t=0.5 (ε negligible).
pub fn blend_weights(t: f64, m: f64) -> (f64, f64) {
    const EPS: f64 = 0.001;
    let wa = (1.0 - t) * (m + EPS);
    let wb = t * (1.0 - m + EPS);
    let sum = wa + wb;
    (wa / sum, wb / sum)
}

/// Worker-side preparation. `Ok(None)` is the honest fallback: no `mkfl`
/// box, an unmappable frame/pair geometry, or a clip over the cache budget
/// — the slot then plays exactly as today. `Err` is a real decode failure.
pub fn prepare_flow_clip(path: &Path) -> Result<Option<Box<FlowClipData>>, String> {
    let len = std::fs::metadata(path).map_err(|e| format!("flow scan stat: {e}"))?.len();
    if len > MAX_FLOW_SCAN_BYTES {
        eprintln!(
            "flow: {} is {len} bytes (> {MAX_FLOW_SCAN_BYTES} scan cap); playing as plain video",
            path.display()
        );
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|e| format!("flow scan read: {e}"))?;
    let Some(map) = crate::flow::parse_mkfl(&bytes) else {
        return Ok(None);
    };
    drop(bytes);
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("non-utf8 flow clip path: {}", path.display()))?;
    let mut decoder = VideoFileDecoder::open(path_str).map_err(|e| e.to_string())?;
    let info = decoder.info().clone();
    if info.width != map.vid_w as u32 || info.height != map.vid_h as u32 {
        eprintln!(
            "flow: mkfl says {}x{} but the video decodes {}x{}; playing as plain video",
            map.vid_w, map.vid_h, info.width, info.height
        );
        return Ok(None);
    }
    let fps = map.fps_num as f64 / map.fps_den.max(1) as f64;
    let duration_secs = info.duration_100ns.max(0) as f64 / 10_000_000.0;
    let predicted = (duration_secs * fps).round() as u64;
    let Some(stride) = endpoint_stride(predicted, map.pairs) else {
        eprintln!(
            "flow: {} predicted frames do not map onto {} pairs; playing as plain video",
            predicted, map.pairs
        );
        return Ok(None);
    };
    let endpoints = map.pairs as usize + 1;
    let frame_bytes = info.width as usize * info.height as usize * 4;
    let need = endpoints * frame_bytes;
    if need > MAX_FLOW_CACHE_BYTES {
        eprintln!(
            "flow: {} endpoint frames at {}x{} need {need} bytes (> {MAX_FLOW_CACHE_BYTES} cache budget); playing as plain video",
            endpoints, info.width, info.height
        );
        return Ok(None);
    }
    let mut frames: Vec<Vec<u32>> = Vec::with_capacity(endpoints);
    let mut rgb_scratch = Vec::new();
    let mut seen: u64 = 0;
    loop {
        match decoder.next_frame() {
            Ok(Some(frame)) => {
                if seen % stride as u64 == 0 && frames.len() < endpoints {
                    nv12::nv12_to_rgb8(&frame.nv12, frame.width, frame.height, &mut rgb_scratch);
                    let mut bgra = Vec::with_capacity((frame.width * frame.height) as usize);
                    for px in rgb_scratch.chunks_exact(3) {
                        bgra.push(
                            0xff00_0000
                                | ((px[0] as u32) << 16)
                                | ((px[1] as u32) << 8)
                                | px[2] as u32,
                        );
                    }
                    frames.push(bgra);
                }
                seen += 1;
            }
            Ok(None) => break,
            Err(e) => return Err(format!("flow clip decode: {e}")),
        }
    }
    // The platform decoder may withhold the FINAL sample when the container
    // declares its duration a tick short (AVAssetReader clips to the track's
    // timeRange; the enhance gate clip declares 280999/48000 for 281
    // samples). Losing under one stride of tail frames costs exactly the
    // last endpoint: drop that pair and keep everything else honest.
    let short = predicted.saturating_sub(seen);
    let usable = if frames.len() == endpoints && short == 0 {
        map.pairs
    } else if frames.len() + 1 == endpoints && short > 0 && short <= stride as u64 {
        eprintln!(
            "flow: decoder delivered {seen} of {predicted} frames; dropping the tail pair ({} of {} usable)",
            map.pairs - 1,
            map.pairs
        );
        map.pairs - 1
    } else {
        eprintln!(
            "flow: decoded {seen} frames (predicted {predicted}), kept {} of {} endpoints; playing as plain video",
            frames.len(),
            endpoints
        );
        return Ok(None);
    };
    if usable == 0 {
        return Ok(None);
    }
    eprintln!(
        "flow: WARP path accepted — {} endpoints at {}x{} ({} MB cache), stride {stride}, {:.2} pairs/s",
        frames.len(),
        info.width,
        info.height,
        need >> 20,
        fps / stride as f64
    );
    Ok(Some(Box::new(FlowClipData {
        frames,
        pairs: usable,
        width: info.width as usize,
        height: info.height as usize,
        stride,
        pairs_per_sec: fps / stride as f64,
        duration_secs,
        map,
    })))
}

// ---------------------------------------------------------------------------
// the warp pass
// ---------------------------------------------------------------------------

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawFlowWarp::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_a: texture_2d(float)
        tex_b: texture_2d(float)
        tex_flow: texture_2d(float)
        tex_mask: texture_2d(float)

        // This quad fills its own offscreen pass. The stock DrawQuad vertex
        // clamps against the inherited turtle clip and the draw list's
        // view_clip — both belong to the PARENT (window) context here and
        // would slice the pass to the on-screen widget region. Transform in
        // pure pass space instead: rect → pass ortho, no clipping.
        vertex: fn() {
            let clipped = self.geom.pos * self.rect_size + self.rect_pos
            self.pos = self.geom.pos
            self.world = vec4(clipped.x, clipped.y, self.draw_depth, 1.0)
            return self.draw_pass.camera_projection * (self.draw_pass.camera_view * self.world)
        }

        // The in-between at fractional t of one flow pair (frames A, B).
        //
        // Stored fields were computed at t=0.5 (see crate::flow): backward
        // warps flow0 = F(0.5→A), flow1 = F(0.5→B), scaled linearly to any
        // t as flow0·(t/0.5) and flow1·((1-t)/0.5). Units are quarter-pixel
        // at GRID resolution, so uv offset = i8 / (4·grid).
        //
        // Fusion: w_a = (1-t)·(m+ε), w_b = t·(1-m+ε),
        //         out = (w_a·warpA + w_b·warpB) / (w_a + w_b)
        // — exact endpoints (t=0 → frame A, t=1 → frame B for every mask
        // value) and RIFE's own merge m·warpA + (1-m)·warpB at t=0.5.
        pixel: fn() {
            let uv = self.pos
            let raw = self.tex_flow.sample_as_bgra(uv)
            let g4 = vec2(self.grid_w, self.grid_h) * 4.0
            let f0 = (raw.xy * 255.0 - vec2(128.0, 128.0)) / g4
            let f1 = (raw.zw * 255.0 - vec2(128.0, 128.0)) / g4
            let m = self.tex_mask.sample_as_bgra(uv).x
            let t = self.t_pair
            let lo = vec2(0.0, 0.0)
            let hi = vec2(1.0, 1.0)
            let a = self.tex_a.sample_as_bgra(clamp(uv + f0 * (t / 0.5), lo, hi))
            let b = self.tex_b.sample_as_bgra(clamp(uv + f1 * ((1.0 - t) / 0.5), lo, hi))
            let wa = (1.0 - t) * (m + 0.001)
            let wb = t * (1.0 - m + 0.001)
            return vec4((a.xyz * wa + b.xyz * wb) / (wa + wb), 1.0)
        }
    }

    set_type_default() do #(DrawFlowTex::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)

        pixel: fn() {
            let s = self.tex.sample_as_bgra(self.pos)
            return vec4(s.xyz, 1.0)
        }
    }

    mod.widgets.FlowWarpViewBase = #(FlowWarpView::register_widget(vm))
    mod.widgets.FlowWarpView = set_type_default() do mod.widgets.FlowWarpViewBase{
        width: 4
        height: 4
    }
}

/// Per the draw-shader layout law: only `#[live]` instance fields after the
/// `#[deref]`.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFlowWarp {
    #[deref]
    pub draw_super: DrawQuad,
    /// Fractional position inside the current pair (0 = frame A, 1 = B).
    #[live]
    pub t_pair: f32,
    #[live(1.0)]
    pub grid_w: f32,
    #[live(1.0)]
    pub grid_h: f32,
}

/// Straight blit of the warp output for the composited (lab) mode.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawFlowTex {
    #[deref]
    pub draw_super: DrawQuad,
}

/// GPU residency for the loaded clip: double-buffered endpoint frames plus
/// the current pair's flow and mask.
struct ClipGpu {
    data: Box<FlowClipData>,
    frame_tex: [Texture; 2],
    /// Which endpoint index each frame texture currently holds.
    frame_in_tex: [Option<usize>; 2],
    flow_tex: Texture,
    mask_tex: Texture,
    flow_pair: Option<u32>,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct FlowWarpView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_warp: DrawFlowWarp,
    #[live]
    draw_tex: DrawFlowTex,
    /// Blit the warp texture onto this widget's own rect (the lab rig).
    /// Slot instances leave this off: their picture IS the pass texture.
    #[live(false)]
    composite: bool,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[rust]
    area: Area,
    #[rust]
    initialized: bool,
    #[rust]
    clip: Option<ClipGpu>,
    /// Free-running playback position in FRAME-PAIR space, 0 ..= pairs.
    #[rust]
    position: f64,
    /// Any float; negative runs backwards. 1.0 = the clip's natural speed.
    #[rust(1.0f64)]
    rate: f64,
    /// Bounce reflection state (+1 forward leg, -1 reverse leg).
    #[rust(1.0f64)]
    dir: f64,
    /// Trim window as clip fractions (the brackets); (0,1) = whole clip.
    #[rust((0.0f64, 1.0f64))]
    window: (f64, f64),
    #[rust]
    bounce: bool,
    #[rust]
    playing: bool,
    /// At least one warp pass has rendered — the pass texture is real.
    #[rust]
    rendered: bool,
    /// CPU cost of encoding the last warp pass, milliseconds.
    #[rust]
    pub last_pass_ms: f64,
    /// Lab-only probe: composite the raw frame A texture instead of the
    /// warp output, isolating pass problems from blit problems.
    #[rust]
    pub debug_show_frame: bool,
}

impl FlowWarpView {
    /// Adopt a prepared clip; playback starts parked at pair 0.
    pub fn set_clip(&mut self, cx: &mut Cx, data: Box<FlowClipData>) {
        let make = |cx: &mut Cx, w: usize, h: usize, px: Vec<u32>| {
            Texture::new_with_format(
                cx,
                TextureFormat::VecBGRAu8_32 {
                    width: w,
                    height: h,
                    data: Some(px),
                    updated: TextureUpdated::Full,
                },
            )
        };
        let (w, h) = (data.width, data.height);
        let (gw, gh) = (data.map.grid_w as usize, data.map.grid_h as usize);
        let (flow_px, mask_px) = pack_flow_texels(
            data.map.pair(0).expect("a parsed map has pair 0"),
            gw,
            gh,
        );
        let frame_a = make(cx, w, h, data.frames[0].clone());
        let frame_b = make(cx, w, h, data.frames[1].clone());
        let flow_tex = make(cx, gw, gh, flow_px);
        let mask_tex = make(cx, gw, gh, mask_px);
        self.clip = Some(ClipGpu {
            data,
            frame_tex: [frame_a, frame_b],
            frame_in_tex: [Some(0), Some(1)],
            flow_tex,
            mask_tex,
            flow_pair: Some(0),
        });
        self.position = 0.0;
        self.dir = 1.0;
        self.rendered = false;
        self.area.redraw(cx);
    }

    pub fn clear(&mut self, cx: &mut Cx) {
        self.clip = None;
        self.playing = false;
        self.position = 0.0;
        self.dir = 1.0;
        self.rendered = false;
        self.area.redraw(cx);
    }

    pub fn has_clip(&self) -> bool {
        self.clip.is_some()
    }

    /// The warp output only once a pass has actually rendered into it.
    pub fn output(&self) -> Option<(Texture, f32)> {
        let clip = self.clip.as_ref()?;
        if !self.rendered {
            return None;
        }
        Some((self.color_texture.clone(), clip.data.aspect()))
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    pub fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
    }

    pub fn set_bounce(&mut self, bounce: bool) {
        self.bounce = bounce;
    }

    pub fn bounce(&self) -> bool {
        self.bounce
    }

    /// The direction the warp clock is actually traveling (true =
    /// forward): the rate's sign folded with the bounce leg.
    pub fn travel_forward(&self) -> bool {
        self.rate * self.dir >= 0.0
    }

    pub fn pairs(&self) -> u32 {
        self.clip.as_ref().map(|c| c.data.pairs).unwrap_or(0)
    }

    pub fn position_pairs(&self) -> f64 {
        self.position
    }

    pub fn set_position_pairs(&mut self, cx: &mut Cx, position: f64) {
        let pairs = self.pairs() as f64;
        self.position = position.clamp(0.0, pairs.max(0.0));
        self.area.redraw(cx);
    }

    pub fn position_secs(&self) -> f64 {
        match self.clip.as_ref() {
            Some(c) if c.data.pairs_per_sec > 0.0 => self.position / c.data.pairs_per_sec,
            _ => 0.0,
        }
    }

    pub fn set_position_secs(&mut self, cx: &mut Cx, secs: f64) {
        if let Some(pps) = self.clip.as_ref().map(|c| c.data.pairs_per_sec) {
            self.set_position_pairs(cx, secs * pps);
        }
    }

    pub fn duration_secs(&self) -> f64 {
        self.clip.as_ref().map(|c| c.data.duration_secs).unwrap_or(0.0)
    }

    pub fn seek_fraction(&mut self, cx: &mut Cx, fraction: f64) {
        let pairs = self.pairs() as f64;
        self.set_position_pairs(cx, fraction.clamp(0.0, 1.0) * pairs);
    }

    /// One display frame of transport: advance the pair-space clock and ask
    /// for a redraw (the redraw renders the warp pass). Paused = freeze.
    pub fn advance(&mut self, cx: &mut Cx, dt: f64) {
        let Some(clip) = self.clip.as_ref() else { return };
        if !self.playing {
            return;
        }
        let pairs = clip.data.pairs as f64;
        let (lo, hi) = self.window_pairs(pairs);
        let delta = self.rate * clip.data.pairs_per_sec * dt;
        let (position, dir) =
            advance_position(self.position, self.dir, delta, lo, hi, self.bounce);
        if tl_on() {
            eprintln!(
                "tl warp pos={:.3} dir={:+.0} rate={:+.4} delta={:+.4} dt={:.1}ms win={:.1}..{:.1} bounce={}",
                position, dir, self.rate, delta, dt * 1000.0, lo, hi, self.bounce
            );
        }
        self.position = position;
        self.dir = dir;
        self.area.redraw(cx);
    }

    /// The trim window (fractions of the clip) in pair space.
    fn window_pairs(&self, pairs: f64) -> (f64, f64) {
        let lo = self.window.0.clamp(0.0, 1.0) * pairs;
        let hi = self.window.1.clamp(0.0, 1.0) * pairs;
        if hi - lo > 1e-6 { (lo, hi) } else { (0.0, pairs) }
    }

    /// The trim brackets, as fractions. A window CHANGE remaps the
    /// position PROPORTIONALLY — the sweep-phase law: the picture keeps
    /// its progress through the range, it never clamps to an edge and
    /// never resets.
    pub fn set_window(&mut self, lo: f64, hi: f64) {
        let old = self.window;
        if (old.0 - lo).abs() < 1e-9 && (old.1 - hi).abs() < 1e-9 {
            return;
        }
        if let Some(pairs) = self.clip.as_ref().map(|c| c.data.pairs as f64) {
            let (olo, ohi) = (old.0 * pairs, old.1 * pairs);
            let ospan = (ohi - olo).max(1e-9);
            let phase = ((self.position - olo) / ospan).clamp(0.0, 1.0);
            let (nlo, nhi) = (lo.clamp(0.0, 1.0) * pairs, hi.clamp(0.0, 1.0) * pairs);
            if nhi - nlo > 1e-6 {
                self.position = nlo + phase * (nhi - nlo);
            }
        }
        self.window = (lo, hi);
    }

    /// THE SWEEP LAW's rate for this clip: the rate that carries one
    /// full window sweep in `sweep_secs` (the chip's beats × the beat
    /// period). In natural-rate units, ready for `set_rate`.
    pub fn law_rate(&self, sweep_secs: f64) -> f64 {
        let Some(clip) = self.clip.as_ref() else { return 1.0 };
        let pairs = clip.data.pairs as f64;
        let (lo, hi) = self.window_pairs(pairs);
        let pps = clip.data.pairs_per_sec.max(1e-6);
        if sweep_secs <= 0.0 {
            return 1.0;
        }
        (hi - lo) / (pps * sweep_secs)
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );
    }

    /// Frame textures for pair i: reuse whichever of the two textures
    /// already holds an endpoint (stepping one pair forward re-uploads ONE
    /// frame, not two). Returns (tex index for A, tex index for B).
    fn ensure_pair_frames(&mut self, cx: &mut Cx, pair: u32) -> (usize, usize) {
        let clip = self.clip.as_mut().expect("caller checked");
        let (fa, fb) = (pair as usize, pair as usize + 1);
        let held = |clip: &ClipGpu, frame| clip.frame_in_tex.iter().position(|f| *f == Some(frame));
        let upload = |clip: &mut ClipGpu, cx: &mut Cx, tex: usize, frame: usize| {
            clip.frame_tex[tex].set_data_u32(
                cx,
                clip.data.width,
                clip.data.height,
                clip.data.frames[frame].clone(),
            );
            clip.frame_in_tex[tex] = Some(frame);
        };
        match (held(clip, fa), held(clip, fb)) {
            (Some(a), Some(b)) => (a, b),
            (Some(a), None) => {
                let b = 1 - a;
                upload(clip, cx, b, fb);
                (a, b)
            }
            (None, Some(b)) => {
                let a = 1 - b;
                upload(clip, cx, a, fa);
                (a, b)
            }
            (None, None) => {
                upload(clip, cx, 0, fa);
                upload(clip, cx, 1, fb);
                (0, 1)
            }
        }
    }

    fn ensure_pair_flow(&mut self, cx: &mut Cx, pair: u32) {
        let clip = self.clip.as_mut().expect("caller checked");
        if clip.flow_pair == Some(pair) {
            return;
        }
        let (gw, gh) = (clip.data.map.grid_w as usize, clip.data.map.grid_h as usize);
        let Some(bytes) = clip.data.map.pair(pair) else { return };
        let (flow_px, mask_px) = pack_flow_texels(bytes, gw, gh);
        clip.flow_tex.set_data_u32(cx, gw, gh, flow_px);
        clip.mask_tex.set_data_u32(cx, gw, gh, mask_px);
        clip.flow_pair = Some(pair);
    }
}

impl WidgetNode for FlowWarpView {
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

impl Widget for FlowWarpView {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if self.clip.is_none() {
            return DrawStep::done();
        }
        self.ensure_initialized(cx.cx);
        let (pairs, w, h) = {
            let clip = self.clip.as_ref().unwrap();
            (clip.data.pairs, clip.data.width, clip.data.height)
        };
        // Pair index + fractional t. Position `pairs` (the very end) shows
        // the last pair at t=1.
        let position = self.position.clamp(0.0, pairs as f64);
        let mut pair = position.floor() as u32;
        let mut t = position - pair as f64;
        if pair >= pairs {
            pair = pairs - 1;
            t = 1.0;
        }
        let started = Instant::now();
        let (tex_a, tex_b) = self.ensure_pair_frames(cx.cx, pair);
        self.ensure_pair_flow(cx.cx, pair);
        {
            let clip = self.clip.as_ref().unwrap();
            self.draw_warp.t_pair = t as f32;
            self.draw_warp.grid_w = clip.data.map.grid_w as f32;
            self.draw_warp.grid_h = clip.data.map.grid_h as f32;
            self.draw_warp.draw_vars.set_texture(0, &clip.frame_tex[tex_a]);
            self.draw_warp.draw_vars.set_texture(1, &clip.frame_tex[tex_b]);
            self.draw_warp.draw_vars.set_texture(2, &clip.flow_tex);
            self.draw_warp.draw_vars.set_texture(3, &clip.mask_tex);
        }
        // Child pass at exact video resolution, dpi locked to 1 (the
        // thumbnail-renderer recipe: re-assert the size after begin_pass or
        // the texture takes the window's rect).
        let size = dvec2(w as f64, h as f64);
        self.pass.set_size(cx, size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)),
        );
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(1.0));
        self.pass.set_size(cx, size);
        self.pass.set_dpi_factor(cx, 1.0);
        self.draw_list.begin_always(cx);
        self.draw_warp.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size });
        self.draw_list.end(cx);
        cx.end_pass(&self.pass);
        self.rendered = true;
        self.last_pass_ms = started.elapsed().as_secs_f64() * 1000.0;
        if self.composite && rect.size.x > 1.0 && rect.size.y > 1.0 {
            if self.debug_show_frame {
                let clip = self.clip.as_ref().unwrap();
                self.draw_tex.draw_vars.set_texture(0, &clip.frame_tex[tex_a]);
            } else {
                self.draw_tex.draw_vars.set_texture(0, &self.color_texture);
            }
            self.draw_tex.draw_abs(cx, rect);
        }
        DrawStep::done()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_import_converter_aims_at_exactly_these_budgets() {
        // The two numbers that decide whether a converted clip warps at all
        // live in TWO crates: the player's budgets here, the converter's
        // targets there. They are the same numbers, and a drift would show
        // up as "conversion succeeded, playback is plain video" — silent,
        // and expensive to chase. So it fails here instead.
        assert_eq!(
            MAX_FLOW_CACHE_BYTES,
            makepad_video_flow::convert::DEFAULT_FIT_CACHE_BYTES
        );
        assert_eq!(
            MAX_FLOW_SCAN_BYTES,
            makepad_video_flow::convert::DEFAULT_MAX_OUTPUT_BYTES
        );
    }

    #[test]
    fn stride_maps_the_shipped_shapes_and_refuses_misfits() {
        // job 1 regate: 281 frames, 140 pairs — tweened, stride 2.
        assert_eq!(endpoint_stride(281, 140), Some(2));
        // job 2: 141 frames, 140 pairs — every frame an endpoint.
        assert_eq!(endpoint_stride(141, 140), Some(1));
        // A frame count that does not divide onto the pairs is refused.
        assert_eq!(endpoint_stride(280, 140), None);
        assert_eq!(endpoint_stride(2, 0), None);
        assert_eq!(endpoint_stride(1, 140), None);
    }

    #[test]
    fn loop_position_wraps_both_directions() {
        let (p, d) = advance_position(139.5, 1.0, 1.0, 0.0, 140.0, false);
        assert!((p - 0.5).abs() < 1e-9 && d == 1.0);
        // Negative rate wraps below zero.
        let (p, _) = advance_position(0.25, 1.0, -1.0, 0.0, 140.0, false);
        assert!((p - 139.25).abs() < 1e-9);
    }

    #[test]
    fn bounce_reflects_and_flips_direction() {
        // Forward leg hits the end and reflects.
        let (p, d) = advance_position(139.5, 1.0, 1.0, 0.0, 140.0, true);
        assert!((p - 139.5).abs() < 1e-9, "reflected to {p}");
        assert_eq!(d, -1.0);
        // Reverse leg hits zero and reflects forward.
        let (p, d) = advance_position(0.25, -1.0, 1.0, 0.0, 140.0, true);
        assert!((p - 0.75).abs() < 1e-9, "reflected to {p}");
        assert_eq!(d, 1.0);
        // Negative rate on a forward leg walks backwards (delta carries the
        // sign; dir carries the reflection).
        let (p, d) = advance_position(10.0, 1.0, -0.5, 0.0, 140.0, true);
        assert!((p - 9.5).abs() < 1e-9 && d == 1.0);
        // A huge delta on a tiny clip settles inside the range.
        let (p, _) = advance_position(0.5, 1.0, 7.3, 0.0, 2.0, true);
        assert!((0.0..=2.0).contains(&p), "settled at {p}");
    }

    #[test]
    fn flow_texels_bias_i8_and_replicate_mask() {
        // One 1x2 grid: two cells, planar planes of 2.
        // f0x = [0, -1], f0y = [4, 127], f1x = [-128, 2], f1y = [1, -4],
        // mask = [255, 7].
        let pair: Vec<u8> = vec![
            0u8,
            (-1i8) as u8,
            4,
            127,
            (-128i8) as u8,
            2,
            1,
            (-4i8) as u8,
            255,
            7,
        ];
        let (flow, mask) = pack_flow_texels(&pair, 2, 1);
        // Cell 0: R=0+128, G=4+128, B=-128+128=0, A=1+128.
        assert_eq!(flow[0], (129 << 24) | (128 << 16) | (132 << 8) | 0);
        // Cell 1: R=-1+128=127, G=127+128=255, B=2+128=130, A=-4+128=124.
        assert_eq!(flow[1], (124 << 24) | (127 << 16) | (255 << 8) | 130);
        assert_eq!(mask[0], 0xffff_ffff);
        assert_eq!(mask[1], 0xff00_0000 | (7 << 16) | (7 << 8) | 7);
    }

    #[test]
    fn fusion_weights_hit_exact_endpoints_and_rife_merge_at_half() {
        // t=0 → all A, t=1 → all B, for every mask value.
        for m in [0.0, 0.25, 1.0] {
            let (wa, wb) = blend_weights(0.0, m);
            assert!((wa - 1.0).abs() < 1e-9 && wb.abs() < 1e-9);
            let (wa, wb) = blend_weights(1.0, m);
            assert!(wa.abs() < 1e-9 && (wb - 1.0).abs() < 1e-9);
        }
        // t=0.5: the RIFE merge m·A + (1-m)·B (within ε).
        let (wa, wb) = blend_weights(0.5, 0.75);
        assert!((wa - 0.75).abs() < 2e-3, "wa {wa}");
        assert!((wb - 0.25).abs() < 2e-3, "wb {wb}");
    }
}
