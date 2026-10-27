//! The desktop end of remote rendering: an app in a process with a remote headset slot
//! (`MAKEPAD_XR_REMOTE=1`) serves its XR view. The slot is an offscreen XR display
//! (`Cx::xr_offscreen`): the app's XR pass renders both eyes side by side into its own colour and
//! depth textures, paced by the headset's poses. [`XrServe`] (the app's, drawn right after its XR
//! pass) turns each new frame into the stream picture on the GPU and hands it to the encoder
//! (`makepad_desk_link::xr_host`), and shows the left eye as the desktop's mirror:
//!
//! ```text
//! window pass   the mirror (the left eye, letterboxed) and the app's own UI
//! picture       w/4 x 9h/4 shared BGRA8, NV12 packed (draw_nv12_pack): per eye column its colour
//!  (no parent,  (luma rows 0..h, chroma rows), below it a band h/2 tall: coverage (left half)
//!   paints      and inverse depth (right half) at half resolution in the luma, neutral chroma
//!   after XR)
//! XR pass       both eyes side by side into the app's colour + depth textures (paints first)
//! ```
//!
//! The picture has three slots: one is never drawn into while its encode reads it. A frame is
//! packed into a free slot; after the frame is submitted (the next `Signal`) the slot goes to the
//! encoder with the frame's fence, and the next pose is asked for (`hal_xr_remote_drawn`) once
//! there is room: a free slot and fewer pictures in flight to the headset than its limit
//! (`xr_host::room`). Without room the app renders nothing more until the encoder or the headset
//! catches up (its wake signals): the stream never queues stale pictures.

use makepad_widgets::makepad_draw::shader::draw_nv12_pack::{DrawNv12Chroma, DrawNv12Luma};
use makepad_widgets::makepad_draw::render_target::{RenderTarget, RenderTargetFormat};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use makepad_desk_link::{xr::DEPTH_NEAR_M, xr_host};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The band under the eye columns, four luma bytes per texel (memory order B G R A): per eye
    // column, its coverage in the left half and its inverse depth in the right half, both at half
    // resolution (a 2x2 block: coverage averaged, depth its nearest). `neutral` 1: every byte 128
    // (the band's chroma).
    mod.draw.DrawXrBand = mod.std.set_type_default() do #(DrawXrBand::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        depth_tex: texture_2d(float)
        band_byte: fn(x: float, r: float) -> float {
            let ew = self.eye.x
            let e = floor(x / ew)
            let xe = x - e * ew
            let half = ew * 0.5
            let inv = vec2(1.0 / self.extent.x, 1.0 / self.extent.y)
            if xe < half {
                // the 2x2 block's centre: one bilinear tap averages it
                return self.image.sample(vec2(e * ew + xe * 2.0 + 1.0, r * 2.0 + 1.0) * inv).w
            }
            let p = vec2(e * ew + (xe - half) * 2.0 + 0.5, r * 2.0 + 0.5)
            let d0 = self.depth_tex.sample(p * inv).x
            let d1 = self.depth_tex.sample((p + vec2(1.0, 0.0)) * inv).x
            let d2 = self.depth_tex.sample((p + vec2(0.0, 1.0)) * inv).x
            let d3 = self.depth_tex.sample((p + vec2(1.0, 1.0)) * inv).x
            let d = min(min(d0, d1), min(d2, d3))
            if d >= 0.999999 {
                return 0.0
            }
            // the eye projection's depth back to metres: -z = 2fn / ((f + n) - d (f - n))
            let z = self.clip.x / (self.clip.y - d * self.clip.z)
            return clamp(self.clip.w / max(z, 0.0001), 0.0, 1.0)
        }
        pixel: fn() {
            if self.neutral > 0.5 {
                return vec4(0.50196, 0.50196, 0.50196, 0.50196)
            }
            let t = floor(self.pos * self.rect_size)
            let x = t.x * 4.0
            return vec4(self.band_byte(x + 2.0, t.y), self.band_byte(x + 1.0, t.y), self.band_byte(x, t.y), self.band_byte(x + 3.0, t.y))
        }
    }

    // The desktop's mirror of what streams: the left eye's half of the side-by-side colour
    // (premultiplied on black: the room shows black where nothing was drawn).
    mod.draw.DrawXrMirror = mod.std.set_type_default() do #(DrawXrMirror::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        pixel: fn() {
            let c = self.image.sample(vec2(self.pos.x * 0.5, self.pos.y))
            return vec4(c.xyz + vec3(0.04, 0.045, 0.05) * (1.0 - c.w), 1.0)
        }
    }

    // The stereo test pattern (`MAKEPAD_XR_REMOTE_TEST=1`), drawn over the packed picture in its
    // packed form: the left eye solid red with an L, the right solid green with an R, a white
    // cross on each eye's optical axis (`axis`: its picture position, 0..1, left eye xy, right
    // eye zw), a grey cross at each column's centre and a dark grid every tenth. `mode` 0: the
    // luma rows, 1: the chroma rows, 2: the band (full coverage, no depth).
    mod.draw.DrawXrTest = mod.std.set_type_default() do #(DrawXrTest::script_shader(vm)){
        ..mod.draw.DrawQuad
        box_in: fn(q: vec2, x0: float, y0: float, x1: float, y1: float) -> float {
            return step(x0, q.x) * step(q.x, x1) * step(y0, q.y) * step(q.y, y1)
        }
        pattern: fn(px: vec2) -> vec3 {
            let ew = self.eye.x
            let right = step(ew, px.x)
            let q = vec2((px.x - right * ew) / ew, px.y / self.eye.y)
            var c = mix(vec3(0.75, 0.05, 0.05), vec3(0.05, 0.65, 0.1), right)
            // the grid every tenth of the eye
            let g = q * 10.0
            let gl = min(abs(g.x - floor(g.x + 0.5)), abs(g.y - floor(g.y + 0.5)))
            if gl < 0.02 {
                c = c * 0.35
            }
            // the column's centre (grey) and the optical axis (white)
            if abs(q.x - 0.5) < 0.0015 || abs(q.y - 0.5) < 0.0015 {
                c = vec3(0.5, 0.5, 0.5)
            }
            let a = mix(self.axis.xy, self.axis.zw, right)
            if (abs(q.x - a.x) < 0.002 && abs(q.y - a.y) < 0.06) || (abs(q.y - a.y) < 0.003 && abs(q.x - a.x) < 0.04) {
                c = vec3(1.0, 1.0, 1.0)
            }
            // the letter: L (left), R (right)
            var l = self.box_in(q, 0.40, 0.18, 0.43, 0.36) + self.box_in(q, 0.40, 0.33, 0.52, 0.36)
            var r = self.box_in(q, 0.40, 0.18, 0.43, 0.36) + self.box_in(q, 0.40, 0.18, 0.50, 0.21) + self.box_in(q, 0.40, 0.26, 0.50, 0.29) + self.box_in(q, 0.49, 0.18, 0.52, 0.29)
            // R's leg: near the segment (0.43, 0.28) - (0.53, 0.36)
            let d = q - vec2(0.43, 0.28)
            let e = vec2(0.10, 0.08)
            let t = clamp(dot(d, e) / dot(e, e), 0.0, 1.0)
            if length(d - e * t) < 0.013 {
                r = 1.0
            }
            if mix(l, r, right) > 0.5 {
                c = vec3(1.0, 1.0, 1.0)
            }
            return c
        }
        luma: fn(px: vec2) -> float {
            return (16.0 + 219.0 * dot(self.pattern(px), vec3(0.2126, 0.7152, 0.0722))) / 255.0
        }
        pixel: fn() {
            let t = floor(self.pos * self.rect_size)
            if self.mode < 0.5 {
                let x = t.x * 4.0 + 0.5
                let y = t.y + 0.5
                return vec4(self.luma(vec2(x + 2.0, y)), self.luma(vec2(x + 1.0, y)), self.luma(vec2(x, y)), self.luma(vec2(x + 3.0, y)))
            }
            if self.mode < 1.5 {
                let p = vec2(t.x * 4.0 + 1.0, t.y * 2.0 + 1.0)
                let c0 = self.pattern(p)
                let c1 = self.pattern(p + vec2(2.0, 0.0))
                let cu = vec3(-0.1146, -0.3854, 0.5)
                let cv = vec3(0.5, -0.4542, -0.0458)
                return vec4((128.0 + 224.0 * dot(c1, cu)) / 255.0, (128.0 + 224.0 * dot(c0, cv)) / 255.0, (128.0 + 224.0 * dot(c0, cu)) / 255.0, (128.0 + 224.0 * dot(c1, cv)) / 255.0)
            }
            // the band: coverage 255 in each column's left half, no depth in its right half
            let x = t.x * 4.0
            let ew = self.eye.x
            let v0 = 1.0 - step(ew * 0.5, x + 2.0 - floor((x + 2.0) / ew) * ew)
            let v1 = 1.0 - step(ew * 0.5, x + 1.0 - floor((x + 1.0) / ew) * ew)
            let v2 = 1.0 - step(ew * 0.5, x - floor(x / ew) * ew)
            let v3 = 1.0 - step(ew * 0.5, x + 3.0 - floor((x + 3.0) / ew) * ew)
            return vec4(v0, v1, v2, v3)
        }
    }

    mod.draw.XrServe = #(XrServe::script_component(vm)) {
        draw_test +: {}
        draw_luma +: {}
        draw_chroma +: {}
        draw_band +: {}
        draw_mirror +: {}
        draw_stats_bg +: {}
        draw_stats_bar +: {}
        draw_stats_text +: {}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrBand {
    #[deref]
    pub draw_super: DrawQuad,
    /// The side-by-side colour/depth size (px).
    #[live]
    pub extent: Vec2f,
    /// x: one eye's width (px).
    #[live]
    pub eye: Vec2f,
    /// The eye projection's depth terms: 2fn, f + n, f - n, and the band's near depth (m).
    #[live]
    pub clip: Vec4f,
    #[live]
    pub neutral: f32,
}

/// The stereo test pattern (see its shader).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrTest {
    #[deref]
    pub draw_super: DrawQuad,
    /// One eye's size (px).
    #[live]
    pub eye: Vec2f,
    /// The optical axis in each eye's picture (0..1): left xy, right zw.
    #[live]
    pub axis: Vec4f,
    #[live]
    pub mode: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrMirror {
    #[deref]
    pub draw_super: DrawQuad,
}

/// Picture slots: one being drawn, one being encoded, one spare.
const SLOTS: usize = 3;
/// The eye projections' planes (`platform/src/os/hal/xr.rs`): the band's depth is decoded with
/// them.
const EYE_NEAR: f32 = 0.05;
const EYE_FAR: f32 = 2000.0;
/// What the view centres on until the app says (m): a table-top distance.
const DEFAULT_FOCUS_M: f32 = 1.0;

struct Slot {
    target: RenderTarget,
    /// Its texture's DMA-BUF in the copy registry, once exported.
    handle: Option<(u64, Texture)>,
    /// An encode reads it.
    busy: Arc<AtomicBool>,
}

/// Released (the slot free) when every eye's encode has read the picture.
struct Held(Arc<AtomicBool>);
impl Drop for Held {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// A frame packed this draw, handed on once submitted.
struct Packed {
    slot: usize,
    display_ns: u64,
    built_at: Instant,
}

/// The app's XR serving (see the module). A field of the app's view (`mod.draw.XrServe{}`).
#[derive(Script, ScriptHook)]
pub struct XrServe {
    #[live]
    draw_test: DrawXrTest,
    #[live]
    draw_luma: DrawNv12Luma,
    #[live]
    draw_chroma: DrawNv12Chroma,
    #[live]
    draw_band: DrawXrBand,
    #[live]
    draw_mirror: DrawXrMirror,
    /// The stats panel over the mirror: its ground, its graphs' bars, its text.
    #[live]
    draw_stats_bg: DrawColor,
    #[live]
    draw_stats_bar: DrawColor,
    #[live]
    draw_stats_text: DrawText,
    #[rust]
    slots: Vec<Slot>,
    #[rust]
    next: usize,
    /// The display time of the newest frame packed (a draw of the same frame packs nothing).
    #[rust]
    last_display_ns: u64,
    #[rust]
    packed: Option<Packed>,
    /// The next pose waits for room.
    #[rust]
    owed: bool,
    #[rust]
    signal: Option<SignalToUI>,
    #[rust]
    focus_m: Option<f32>,
    /// The streamed eye size (px), for the mirror's shape.
    #[rust]
    eye: Option<[u32; 2]>,
    #[rust]
    counts: Counts,
    /// The app's own line in the stats panel (`set_app_line`; empty: none).
    #[rust]
    app_line: String,
}

/// The app side's counts per stats window (5 s): draws, new XR frames, frames packed, pictures
/// handed to the encoder, poses asked, the waits for room.
#[derive(Default)]
struct Counts {
    at: Option<Instant>,
    draws: u32,
    frames: u32,
    packed: u32,
    handed: u32,
    asked: u32,
    waited: u32,
}

impl Counts {
    fn window(&mut self) {
        let at = *self.at.get_or_insert_with(Instant::now);
        let secs = at.elapsed().as_secs_f64();
        if secs < 5.0 {
            return;
        }
        let r = |n: u32| n as f64 / secs;
        log!("xr serve: draws {:.0}/s · new XR frames {:.0}/s · packed {:.0}/s · handed {:.0}/s · poses asked {:.0}/s · waited for room {}", r(self.draws), r(self.frames), r(self.packed), r(self.handed), r(self.asked), self.waited);
        *self = Counts { at: Some(Instant::now()), ..Default::default() };
    }
}

/// Starts this process's XR server (once; a process without the remote headset slot, or without
/// the LAN allowed, does nothing). The listener runs until the process ends.
pub fn serve() {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use makepad_desk_link::xr_host::{available, XrHost, XrHostConfig};
        static HOST: Mutex<Option<XrHost>> = Mutex::new(None);
        let mut host = HOST.lock().unwrap();
        if host.is_some() || !available() {
            return;
        }
        if !makepad_widgets::makepad_platform::makepad_network::lan::service("xr host") {
            log!("xr host: local network off for this instance (MAKEPAD_LAN=1 allows it)");
            return;
        }
        let port = std::env::var("MAKEPAD_XR_REMOTE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(makepad_desk_link::xr::XR_PORT);
        let kbps = std::env::var("MAKEPAD_XR_REMOTE_KBPS").ok().and_then(|p| p.parse().ok()).unwrap_or(80_000);
        // the headset's room field, mirrored into this process's field store: the app's
        // physics (its hit-surface queries) collides with the real room
        let world: makepad_desk_link::xr_host::WorldSink = Arc::new(|voxel_size, chunk_edge, bricks| {
            use makepad_widgets::makepad_platform::xr_tsdf::{BrickGrid, BrickMirror, Brick, ChunkKey};
            static MIRROR: Mutex<Option<BrickMirror>> = Mutex::new(None);
            let bricks: Vec<Brick> = bricks.into_iter().map(|b| Brick { key: ChunkKey { x: b.key[0], y: b.key[1], z: b.key[2] }, version: b.version, payload: Arc::new(b.payload) }).collect();
            let mut mirror = MIRROR.lock().unwrap_or_else(|e| e.into_inner());
            mirror.get_or_insert_with(BrickMirror::default).apply(&makepad_widgets::makepad_platform::xr_tsdf::xr_tsdf_store(), BrickGrid { voxel_size, chunk_edge }, &bricks);
        });
        let config = XrHostConfig { addr: std::net::SocketAddr::from(([0, 0, 0, 0], port)), tls_name: "makepad xr host".into(), kbps, world: Some(world), log: Arc::new(|line: &str| log!("{line}")) };
        match XrHost::start(config) {
            Ok(h) => *host = Some(h),
            Err(e) => log!("{e}"),
        }
        if std::env::var("MAKEPAD_XR_REMOTE_SELFTEST").as_deref() == Ok("1") {
            makepad_desk_link::xr_host::selftest([1680, 1760], kbps, Arc::new(|line: &str| log!("{line}")));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = Mutex::<()>::new(());
}

impl XrServe {
    /// A line of the app's own numbers in the desktop's stats panel, under the stream's (empty:
    /// none). Kept until replaced.
    pub fn set_app_line(&mut self, line: &str) {
        if self.app_line != line {
            self.app_line.clear();
            self.app_line.push_str(line);
        }
    }

    /// The depth (m) the view centres on, for the headset's reprojection.
    pub fn set_focus_m(&mut self, focus_m: f32) {
        self.focus_m = Some(focus_m);
    }

    /// After the app drew its XR pass (`color` / `depth`: its textures, eyes side by side): a new
    /// XR frame is packed into the stream picture.
    pub fn draw(&mut self, cx: &mut Cx2d, color: &Texture, depth: &Texture) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !cx.cx.xr_offscreen() {
                return;
            }
            if self.signal.is_none() {
                let signal = SignalToUI::new();
                let s2 = signal.clone();
                xr_host::set_wake(Arc::new(move || s2.set()));
                self.signal = Some(signal);
            }
            self.counts.draws += 1;
            self.counts.window();
            let Some((display_ns, _)) = cx.cx.xr_frame_wire() else { return };
            let Some(eye) = xr_host::session_eye() else { return };
            self.eye = Some(eye);
            if display_ns == self.last_display_ns {
                return;
            }
            self.last_display_ns = display_ns;
            self.counts.frames += 1;
            while self.slots.len() < SLOTS {
                let name = ["xr_stream_picture_0", "xr_stream_picture_1", "xr_stream_picture_2"][self.slots.len()];
                self.slots.push(Slot { target: RenderTarget::new(cx.cx, name), handle: None, busy: Arc::new(AtomicBool::new(false)) });
            }
            // a slot no encode reads (the one after the last, else any)
            let Some(slot) = (0..SLOTS).map(|i| (self.next + i) % SLOTS).find(|i| !self.slots[*i].busy.load(Ordering::Acquire)) else {
                xr_host::note_skipped();
                // the next pose once an encode frees a slot (its wake)
                self.owed = true;
                return;
            };
            self.next = (slot + 1) % SLOTS;
            let (ew, eh) = (eye[0], eye[1]);
            let (w, h) = (ew * 2, eh * 3 / 2);
            let s = &mut self.slots[slot];
            if s.target.ensure(cx.cx, (w / 4, h + h / 2), RenderTargetFormat::SharedBgra8) {
                s.handle = None;
            }
            let extent = vec2(w as f32, eh as f32);
            s.target.begin(cx, vec4(0.0, 0.0, 0.0, 0.0));
            // the colour's luma (rows 0..eh) and chroma (rows h..h + eh/2)
            self.draw_luma.draw_vars.set_texture(0, color);
            self.draw_luma.source = vec4(0.0, 0.0, w as f32, eh as f32);
            self.draw_luma.extent = extent;
            self.draw_luma.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2((w / 4) as f64, eh as f64) });
            self.draw_chroma.draw_vars.set_texture(0, color);
            self.draw_chroma.source = vec4(0.0, 0.0, w as f32, eh as f32);
            self.draw_chroma.extent = extent;
            self.draw_chroma.full = 0.0;
            self.draw_chroma.draw_abs(cx, Rect { pos: dvec2(0.0, h as f64), size: dvec2((w / 4) as f64, (eh / 2) as f64) });
            // the band's luma (rows eh..h) and its neutral chroma (rows h + eh/2 .. h + h/2)
            self.draw_band.draw_vars.set_texture(0, color);
            self.draw_band.draw_vars.set_texture(1, depth);
            self.draw_band.extent = extent;
            self.draw_band.eye = vec2(ew as f32, eh as f32);
            let (n, f) = (EYE_NEAR, EYE_FAR);
            self.draw_band.clip = vec4(2.0 * f * n, f + n, f - n, DEPTH_NEAR_M);
            self.draw_band.neutral = 0.0;
            self.draw_band.draw_abs(cx, Rect { pos: dvec2(0.0, eh as f64), size: dvec2((w / 4) as f64, (h - eh) as f64) });
            self.draw_band.new_draw_call(cx);
            self.draw_band.neutral = 1.0;
            self.draw_band.draw_abs(cx, Rect { pos: dvec2(0.0, (h + eh / 2) as f64), size: dvec2((w / 4) as f64, (h / 2 - eh / 2) as f64) });
            // MAKEPAD_XR_REMOTE_TEST=1: the stereo test pattern over it all
            if std::env::var("MAKEPAD_XR_REMOTE_TEST").as_deref() == Ok("1") {
                let axis = |e: &makepad_widgets::makepad_platform::makepad_hal::XrEye| {
                    let [l, r, u, d] = e.fov.map(f32::tan);
                    vec2(-l / (r - l), u / (u - d))
                };
                let (al, ar) = match cx.cx.xr_frame_wire().and_then(|(_, w)| makepad_widgets::makepad_platform::makepad_hal::buffers::read_items::<makepad_widgets::makepad_platform::makepad_hal::XrFrame>(&w, 1).next()) {
                    Some(f) => (axis(&f.eyes[0]), axis(&f.eyes[1])),
                    None => (vec2(0.5, 0.5), vec2(0.5, 0.5)),
                };
                self.draw_test.eye = vec2(ew as f32, eh as f32);
                self.draw_test.axis = vec4(al.x, al.y, ar.x, ar.y);
                for (mode, y, rows) in [(0.0, 0, eh), (1.0, h, eh / 2), (2.0, eh, h - eh)] {
                    self.draw_test.mode = mode;
                    self.draw_test.new_draw_call(cx);
                    self.draw_test.draw_abs(cx, Rect { pos: dvec2(0.0, y as f64), size: dvec2((w / 4) as f64, rows as f64) });
                }
            }
            s.target.end(cx);
            // no parent: it paints after the XR pass (which paints first) and is held by no window
            s.target.pass.detach(cx.cx);
            s.busy.store(true, Ordering::Release);
            self.counts.packed += 1;
            self.packed = Some(Packed { slot, display_ns, built_at: Instant::now() });
            if let Some(signal) = &self.signal {
                signal.set();
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (cx, color, depth);
    }

    /// The desktop's mirror: the left eye of `color` (the XR pass's side-by-side colour) fitted
    /// into `rect`.
    pub fn draw_mirror(&mut self, cx: &mut Cx2d, rect: Rect, color: &Texture) {
        let eye = self.eye.unwrap_or([1680, 1760]);
        let aspect = eye[0].max(1) as f64 / eye[1].max(1) as f64;
        let (mut w, mut h) = (rect.size.x, rect.size.x / aspect);
        if h > rect.size.y {
            h = rect.size.y;
            w = h * aspect;
        }
        let pos = rect.pos + (rect.size - dvec2(w, h)) * 0.5;
        self.draw_mirror.draw_vars.set_texture(0, color);
        self.draw_mirror.draw_abs(cx, Rect { pos, size: dvec2(w, h) });
        #[cfg(not(target_arch = "wasm32"))]
        self.draw_stats(cx, Rect { pos, size: dvec2(w, h) });
    }

    /// The stats panel (while a headset streams): the headset's last-second numbers and the
    /// server's, and the last minute of pictures shown per second and motion to photon as bars.
    #[cfg(not(target_arch = "wasm32"))]
    fn draw_stats(&mut self, cx: &mut Cx2d, mirror: Rect) {
        use makepad_desk_link::xr::report as r;
        let reports = xr_host::headset_reports();
        let Some(last) = reports.last() else { return };
        let v = |i: usize| last.get(i).copied().unwrap_or(0.0);
        let panel = Rect { pos: mirror.pos + dvec2(12.0, 12.0), size: dvec2(470.0, 204.0) };
        self.draw_stats_bg.color = vec4(0.0, 0.0, 0.0, 0.62);
        self.draw_stats_bg.draw_abs(cx, panel);
        let lines = [
            format!("headset {:.0} Hz · shown {:.0}/s · repeats {:.1}/s · skipped {:.1}/s · received {:.0}/s", v(r::DISPLAY), v(r::SHOWN), v(r::REPEATS), v(r::SKIPPED), v(r::RECEIVED)),
            format!("motion to photon {:.0} ms (net {:.1} decode {:.1} queue {:.1}) · lead {:.0} ms", v(r::M2P_MS), v(r::NET_MS), v(r::DECODE_MS), v(r::WAIT_MS), v(r::LEAD_MS)),
            format!("uplink {:.0} poses/s ±{:.2} ms · env depth age {:.0} ms", v(r::UPLINK), v(r::UPLINK_SD_MS), v(r::ENV_AGE_MS)),
            xr_host::server_line(),
        ];
        for (k, l) in lines.iter().enumerate() {
            self.draw_stats_text.draw_abs(cx, panel.pos + dvec2(8.0, 6.0 + k as f64 * 16.0), l);
        }
        if !self.app_line.is_empty() {
            let at = panel.pos + dvec2(8.0, 6.0 + lines.len() as f64 * 16.0);
            self.draw_stats_text.draw_abs(cx, at, &self.app_line);
        }
        // the last minute: shown/s (green, 0..90) and motion to photon (orange, 0..150 ms)
        let base = panel.pos + dvec2(8.0, 140.0);
        let bw = 454.0 / 60.0;
        for (k, rep) in reports.iter().enumerate() {
            let x = base.x + k as f64 * bw;
            let shown = (rep.get(r::SHOWN).copied().unwrap_or(0.0) as f64 / 90.0).clamp(0.0, 1.0) * 34.0;
            let m2p = (rep.get(r::M2P_MS).copied().unwrap_or(0.0) as f64 / 150.0).clamp(0.0, 1.0) * 34.0;
            self.draw_stats_bar.color = vec4(0.3, 0.85, 0.4, 0.9);
            self.draw_stats_bar.draw_abs(cx, Rect { pos: dvec2(x, base.y - shown), size: dvec2(bw * 0.45, shown.max(1.0)) });
            self.draw_stats_bar.color = vec4(1.0, 0.6, 0.2, 0.9);
            self.draw_stats_bar.draw_abs(cx, Rect { pos: dvec2(x + bw * 0.5, base.y - m2p), size: dvec2(bw * 0.45, m2p.max(1.0)) });
        }
        // the last ~5 s frame by frame, newest right: one tick per display frame (a single repeat
        // in 90 barely moves a per-second bar): new dim, repeat yellow, skip red, odd bright red
        // (tall), late purple (tall)
        let frames: Vec<u8> = {
            let mut all: Vec<u8> = Vec::new();
            for rep in reports.iter().rev().take(5).rev() { all.extend(r::frames(rep)); }
            let n = all.len().saturating_sub(454);
            all.split_off(n)
        };
        let (strip_y, strip_h) = (panel.pos.y + 150.0, 18.0);
        let x0 = panel.pos.x + 8.0 + 454.0 - frames.len() as f64;
        let mut counts = [0u32; 6];
        for (k, c) in frames.iter().enumerate() {
            counts[(*c as usize).min(5)] += 1;
            let (color, h) = match *c {
                r::FRAME_NEW => (vec4(0.3, 0.45, 0.35, 0.7), 0.4),
                r::FRAME_REPEAT => (vec4(1.0, 0.9, 0.1, 1.0), 0.75),
                r::FRAME_SKIP => (vec4(1.0, 0.25, 0.2, 1.0), 0.75),
                r::FRAME_ODD => (vec4(1.0, 0.05, 0.05, 1.0), 1.0),
                r::FRAME_LATE => (vec4(0.75, 0.3, 1.0, 1.0), 1.0),
                _ => (vec4(0.2, 0.2, 0.2, 0.5), 0.2),
            };
            self.draw_stats_bar.color = color;
            self.draw_stats_bar.draw_abs(cx, Rect { pos: dvec2(x0 + k as f64, strip_y + strip_h * (1.0 - h)), size: dvec2(1.0, strip_h * h) });
        }
        let secs = (frames.len() as f64 / v(r::DISPLAY).max(1.0) as f64).max(1.0);
        let per_s = |c: u8| counts[c as usize] as f64 / secs;
        let line = if frames.is_empty() { "per frame: the headset sends no frame codes (an older client)".to_string() } else {
            format!("last {:.0} s per s: repeats {:.1} · skips {:.1} · odd {:.1} · late {:.1}", secs, per_s(r::FRAME_REPEAT), per_s(r::FRAME_SKIP), per_s(r::FRAME_ODD), per_s(r::FRAME_LATE))
        };
        self.draw_stats_text.draw_abs(cx, dvec2(panel.pos.x + 8.0, strip_y + strip_h + 4.0), &line);
    }

    /// The mirror's area (the app's hit area for its view).
    pub fn mirror_area(&self) -> Area {
        self.draw_mirror.area()
    }

    /// The app's events: after a packed frame was submitted (its `Signal`), the picture goes to
    /// the encoder with the frame's fence; the next pose is asked for once there is room.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !matches!(event, Event::Signal) || !self.signal.as_ref().is_some_and(|s| s.check_and_clear()) {
                return;
            }
            if let Some(p) = self.packed.take() {
                self.hand_on(cx, p);
                self.owed = true;
            }
            if self.owed && self.room() {
                self.owed = false;
                self.counts.asked += 1;
                makepad_widgets::makepad_platform::makepad_hal::xr_remote::hal_xr_remote_drawn();
            } else if self.owed {
                self.counts.waited += 1;
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (cx, event);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn room(&self) -> bool {
        self.slots.iter().any(|s| !s.busy.load(Ordering::Acquire)) && xr_host::room()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn hand_on(&mut self, cx: &mut Cx, p: Packed) {
        let busy = self.slots[p.slot].busy.clone();
        let free = || busy.store(false, Ordering::Release);
        let Some(handle) = self.handle(cx, p.slot) else {
            // not exported yet: the next frame's picture follows
            free();
            return;
        };
        // the frame's release fence (published once the frame is submitted, which this does):
        // polled on the encoder's side, the UI never waits for it
        #[cfg(target_os = "linux")]
        let ready: xr_host::Ready = match cx.foreign_dmabuf_fence() {
            Ok(mut fence) => {
                use makepad_widgets::makepad_platform::video_file::GpuReady;
                Box::new(move || fence.poll().map(|r| r.map(|fd| fd.map_or(GpuReady::Done, GpuReady::SyncFile))))
            }
            Err(e) => {
                log!("xr serve: frame fence: {e}");
                free();
                return;
            }
        };
        #[cfg(not(target_os = "linux"))]
        let ready: xr_host::Ready = Box::new(|| Some(Ok(makepad_widgets::makepad_platform::video_file::GpuReady::Done)));
        let picture = xr_host::Picture {
            handle,
            ready,
            held: Box::new(Held(busy.clone())),
            display_ns: p.display_ns,
            built_at: p.built_at,
            focus_m: self.focus_m.unwrap_or(DEFAULT_FOCUS_M),
        };
        // dropped (and the slot freed) when no headset streams
        if xr_host::submit(picture) {
            self.counts.handed += 1;
        }
    }

    /// The slot's texture as the encoder's handle (its DMA-BUF registered once).
    #[cfg(not(target_arch = "wasm32"))]
    fn handle(&mut self, cx: &mut Cx, slot: usize) -> Option<u64> {
        let s = &mut self.slots[slot];
        let texture = s.target.texture.clone();
        if let Some((handle, t)) = &s.handle {
            if *t == texture {
                return Some(*handle);
            }
        }
        #[cfg(target_os = "linux")]
        {
            // a texture made again (resized): the old one's registration goes
            if let Some((old, _)) = s.handle.take() {
                makepad_widgets::makepad_platform::makepad_hal::dmabuf::hal_dmabuf_release(old);
            }
            use std::os::fd::IntoRawFd;
            let (fd, layout) = cx.shared_texture_dmabuf(&texture).ok()?;
            let handle = makepad_widgets::makepad_platform::makepad_hal::dmabuf::hal_dmabuf_register(fd.into_raw_fd(), &layout);
            if handle == 0 {
                log!("xr serve: the picture's DMA-BUF was refused");
                return None;
            }
            s.handle = Some((handle, texture));
            Some(handle)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (cx, texture);
            None
        }
    }
}
