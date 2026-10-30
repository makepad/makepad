//! The server's frame sink (`makepad_draw::xr_sink`): each XR frame the app draws is packed on
//! the GPU into the stream picture and handed to the encoder ([`super::pipeline`]), and the
//! desktop's mirror is fed:
//!
//! ```text
//! window pass   the app's own UI, and a mirror (the left eye, letterboxed) on its own clock
//! picture       w/4 x 9h/4 shared BGRA8, NV12 packed (draw_nv12_pack): per eye column its colour
//!  (no parent,  (luma rows 0..h, chroma rows), below it a band h/2 tall: coverage (left half)
//!   paints      and inverse depth (right half) at half resolution in the luma, neutral chroma;
//!   after XR)   `w x h` the eyes' encoded (foveated) columns
//! foveation     each eye's colour and inverse depth warped into its encoded column (fewer pixels
//!               towards the view's edges, `makepad_desk_link::xr::Fovea`), painted before the pack
//! XR pass       both eyes side by side into the app's colour + depth textures (paints first)
//! ```
//!
//! The picture has three slots: one is never drawn into while its encode reads it. A frame is
//! packed into a free slot; after the frame is submitted (the next `Signal`) the slot goes to the
//! encoder with the frame's completion, and the next pose is asked for (`hal_xr_remote_drawn`)
//! once there is room: a free slot and fewer pictures in flight to the headset than its limit.
//! Without room the app renders nothing more until the encoder or the headset catches up (the
//! server's wake signals): the stream never queues stale pictures.

use super::pipeline::Picture;
use super::{ServerShared, XrServer};
use makepad_desk_link::xr::{Fovea, DEPTH_NEAR_M};
use makepad_widgets::makepad_draw::render_target::{Nv12Target, RenderTarget, RenderTargetFormat};
use makepad_widgets::makepad_draw::shader::draw_fxaa::DrawFxaa;
use makepad_widgets::makepad_draw::shader::draw_nv12_pack::{DrawNv12Chroma, DrawNv12Luma};
use makepad_widgets::makepad_draw::xr_sink::{XrFrameSink, XrPresented};
use makepad_widgets::makepad_platform::video_file::{SurfaceFormat, SurfaceHandle};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The eye's colour and depth foveated (`makepad_desk_link::xr::Fovea`) into its encoded
    // column (one draw per eye, `rect` the column): each output pixel samples the eye where the
    // warp puts it, its footprint there filtered with four taps (one eye pixel in the centre, up
    // to several towards the edges). `mode` 0: the colour (premultiplied, alpha the coverage);
    // 1: the inverse depth byte of the band (`DEPTH_NEAR_M / z`; 0 nothing drawn) in every
    // channel, the nearest of the footprint's. `fx`/`fy`: the eye's axes `[a, b, ea, total]`,
    // `fk`: their ramp and edge density (x, x, y, y); `column`: the eye's columns in the XR
    // pass's images (x0, width, height; px).
    mod.draw.DrawXrFoveate = mod.std.set_type_default() do #(DrawXrFoveate::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        depth_tex: texture_2d(float)
        extent: uniform(vec2(1.0, 1.0))
        column: uniform(vec4(0.0, 1.0, 1.0, 0.0))
        fx: uniform(vec4(0.0, 1.0, 0.0, 1.0))
        fy: uniform(vec4(0.0, 1.0, 0.0, 1.0))
        fk: uniform(vec4(0.0, 1.0, 0.0, 1.0))
        clip: uniform(vec4(1.0, 1.0, 1.0, 1.0))
        mode: uniform(0.0)
        unsqueeze: fn(e: float, r: float, d: float) -> float {
            let er = r * (1.0 + d) * 0.5
            if e <= er {
                let k = (1.0 - d) / (2.0 * max(r, 0.000001))
                if k < 0.000001 {
                    return e
                }
                return (1.0 - sqrt(max(1.0 - 4.0 * k * e, 0.0))) / (2.0 * k)
            }
            return r + (e - er) / d
        }
        // the encoded axis' point `v` (0..1) in the eye (0..1)
        to_eye: fn(v: float, f: vec4, r: float, d: float) -> float {
            let e = v * f.w
            if e < f.z {
                return f.x - self.unsqueeze(f.z - e, r, d)
            }
            if e <= f.z + f.y - f.x {
                return f.x + e - f.z
            }
            return f.y + self.unsqueeze(e - f.z - (f.y - f.x), r, d)
        }
        eye_at: fn(t: vec2) -> vec2 {
            return vec2(self.to_eye(t.x, self.fx, self.fk.x, self.fk.y), self.to_eye(t.y, self.fy, self.fk.z, self.fk.w))
        }
        // the band's inverse depth of the depth texel at `q` (px)
        inv_depth: fn(q: vec2) -> float {
            let c = clamp(floor(q), vec2(self.column.x, 0.0), vec2(self.column.x + self.column.y - 1.0, self.column.z - 1.0)) + vec2(0.5, 0.5)
            let d = self.depth_tex.sample(c / self.extent).x
            if d >= 0.999999 {
                return 0.0
            }
            // the eye projection's depth back to metres: -z = 2fn / ((f + n) - d (f - n))
            let z = self.clip.x / (self.clip.y - d * self.clip.z)
            return clamp(self.clip.w / max(z, 0.0001), 0.0, 1.0)
        }
        pixel: fn() {
            let o = self.rect_size
            let t = self.pos
            let u = self.eye_at(t)
            // the pixel's footprint in the eye's image (px): where its edges land
            let lo = self.eye_at(t - vec2(0.5, 0.5) / o)
            let hi = self.eye_at(t + vec2(0.5, 0.5) / o)
            let size = vec2(self.column.y, self.column.z)
            let fp = max((hi - lo) * size, vec2(1.0, 1.0))
            let q = vec2(self.column.x, 0.0) + u * size
            let dq = fp * 0.25
            if self.mode > 0.5 {
                let v = max(max(self.inv_depth(q + vec2(-dq.x, -dq.y)), self.inv_depth(q + vec2(dq.x, -dq.y))),
                    max(self.inv_depth(q + vec2(-dq.x, dq.y)), self.inv_depth(q + vec2(dq.x, dq.y))))
                return vec4(v, v, v, v)
            }
            // (inside this eye's column: the filter never reaches the other eye)
            let lo_q = vec2(self.column.x + 0.5, 0.5)
            let hi_q = vec2(self.column.x + self.column.y - 0.5, self.column.z - 0.5)
            let inv = vec2(1.0 / self.extent.x, 1.0 / self.extent.y)
            if fp.x * fp.y <= 1.0 {
                return self.image.sample(clamp(q, lo_q, hi_q) * inv)
            }
            return (self.image.sample(clamp(q + vec2(-dq.x, -dq.y), lo_q, hi_q) * inv) + self.image.sample(clamp(q + vec2(dq.x, -dq.y), lo_q, hi_q) * inv)
                + self.image.sample(clamp(q + vec2(-dq.x, dq.y), lo_q, hi_q) * inv) + self.image.sample(clamp(q + vec2(dq.x, dq.y), lo_q, hi_q) * inv)) * 0.25
        }
    }

    // The band under the eye columns, four luma bytes per texel (memory order B G R A): per eye
    // column, its coverage in the left half and its inverse depth in the right half, both at half
    // resolution of the foveated column (`DrawXrFoveate`'s images: a 2x2 block's coverage
    // averaged, its depth the nearest). `neutral` 1: every byte 128 (the band's chroma).
    mod.draw.DrawXrBand = mod.std.set_type_default() do #(DrawXrBand::script_shader(vm)){
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        depth_tex: texture_2d(float)
        lanes: 4.0
        origin: 0.0
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
            return max(max(d0, d1), max(d2, d3))
        }
        pixel: fn() {
            if self.neutral > 0.5 {
                return vec4(0.50196, 0.50196, 0.50196, 0.50196)
            }
            let t = floor(self.pos * self.rect_size)
            // an NV12 texture's luma plane (a tile from picture column `origin`): one byte per pixel
            if self.lanes < 1.5 {
                let v = self.band_byte(self.origin + t.x, t.y)
                return vec4(v, v, v, v)
            }
            let x = t.x * 4.0
            return vec4(self.band_byte(x + 2.0, t.y), self.band_byte(x + 1.0, t.y), self.band_byte(x, t.y), self.band_byte(x + 3.0, t.y))
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

    mod.draw.XrPack = #(XrPack::script_component(vm)) {
        draw_test +: {}
        draw_luma +: {}
        draw_chroma +: {}
        draw_fxaa +: {}
        draw_foveate +: {}
        draw_band +: {}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrBand {
    #[deref]
    pub draw_super: DrawQuad,
    /// The foveated columns' size (both eyes side by side, px).
    #[live]
    pub extent: Vec2f,
    /// x: one eye's encoded column width (px).
    #[live]
    pub eye: Vec2f,
    #[live]
    pub neutral: f32,
    /// Bytes per output texel: 4 (a BGRA8 texture holding the luma bytes) or 1 (an NV12
    /// texture's luma plane), and the picture column of the output's first pixel.
    #[live]
    pub lanes: f32,
    #[live]
    pub origin: f32,
}

/// One eye's colour or depth into its foveated column (see its shader).
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawXrFoveate {
    #[deref]
    pub draw_super: DrawQuad,
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

/// Picture slots: one being drawn, one being encoded, one spare.
const SLOTS: usize = 3;
/// The eye projections' planes (the HAL's one definition): the band's depth is decoded with them.
use makepad_widgets::makepad_platform::makepad_hal::{XR_EYE_FAR_M as EYE_FAR, XR_EYE_NEAR_M as EYE_NEAR};
/// What the view centres on until the app says (m): a table-top distance.
const DEFAULT_FOCUS_M: f32 = 1.0;

struct Slot {
    /// The packed picture (`nv12` off): BGRA8 texels holding the NV12 bytes, both eye columns.
    target: RenderTarget,
    /// Its texture as the encoder's surface, once exported.
    handle: Option<(SurfaceHandle, Texture)>,
    /// `nv12` on: an NV12 picture per tile (eye column), rendered into its planes, each the
    /// encoder's surface of its tile.
    planes: Vec<(Nv12Target, Option<(SurfaceHandle, Texture)>)>,
    /// The picture it holds (pixels; the texture is its packed NV12 texels).
    picture: [u32; 2],
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

/// The server's pack of the app's XR frames (see the module), installed as the Cx XR frame sink.
#[derive(Script, ScriptHook)]
pub struct XrPack {
    #[live]
    draw_test: DrawXrTest,
    #[live]
    draw_luma: DrawNv12Luma,
    #[live]
    draw_chroma: DrawNv12Chroma,
    /// FXAA over the eyes (a mode with `fxaa`), into `fxaa` before the pack.
    #[live]
    draw_fxaa: DrawFxaa,
    #[rust]
    fxaa: Option<RenderTarget>,
    /// The eyes foveated into their encoded columns (`foveated`: colour, inverse depth).
    #[live]
    draw_foveate: DrawXrFoveate,
    #[rust]
    foveated: Option<[RenderTarget; 2]>,
    #[live]
    draw_band: DrawXrBand,
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
    /// The server's wake (its `Signal`s are this sink's).
    #[rust]
    signal: Option<SignalToUI>,
    #[rust]
    server: Option<XrServer>,
    #[rust]
    counts: Counts,
    /// When the host load's current sample began (None: not measuring).
    #[rust]
    load_at: Option<Instant>,
    /// `MAKEPAD_XR_REMOTE_TEST=1` (read once): the stereo test pattern over every picture.
    #[rust]
    test: bool,
    /// The pictures are NV12 textures, one per tile, rendered into their planes (`super::nv12_planes`).
    #[rust]
    nv12: bool,
    /// The app's controller buzzes of this frame (reused).
    #[rust]
    haptics: Vec<makepad_widgets::makepad_platform::event::xr::XrHapticPulse>,
}


impl XrPack {
    pub(super) fn new(cx: &mut Cx, server: XrServer, signal: SignalToUI) -> XrPack {
        let mut pack = cx.with_vm(|vm| {
            let value = makepad_widgets::script_eval!(vm, {mod.draw.XrPack{}});
            XrPack::script_from_value(vm, value)
        });
        pack.server = Some(server);
        pack.signal = Some(signal);
        pack.test = std::env::var("MAKEPAD_XR_REMOTE_TEST").as_deref() == Ok("1");
        pack.nv12 = super::nv12_planes();
        pack
    }

    fn shared(&self) -> &Arc<ServerShared> {
        &self.server.as_ref().unwrap().shared
    }

    /// After the app drew its XR pass (its colour and depth, eyes side by side): a new XR frame
    /// is packed into the stream picture.
    fn pack(&mut self, cx: &mut Cx2d, color: &Texture, depth: &Texture) {
        if !cx.cx.xr_offscreen() {
            return;
        }
        let shared = self.shared().clone();
        self.counts.draws += 1;
        // the desktop's mirrors read the XR pass (on their own clock, `XrMirrorClock`); the
        // desktop renders nothing for the headset's frames (the platform paints no window for them)
        let source = cx.cx.global::<crate::mirror::XrMirrorSource>();
        source.color = Some(color.clone());
        source.mirror = shared.mirror.load(Ordering::Acquire);
        if let Some(eye) = shared.session_eye() {
            source.eye = eye;
        }
        self.counts.desktop_now = cx.cx.window_paints();
        self.host_load(cx.cx, &shared);
        self.counts.window(&shared);
        // the stream's AA and render scale: the XR pass renders multisampled, at the supersample
        // times the render scale of the encoded eye size (from this frame on)
        let server = self.server.clone().unwrap();
        let aa = server.aa();
        cx.cx.xr_set_supersample(aa.supersample * server.render_scale() as f32 / 100.0);
        cx.cx.xr_set_msaa(aa.msaa);
        // the headset integrates and sends its room field only while the app uses it
        shared.room_field_wanted.store(cx.cx.xr_room_field_wanted(), Ordering::Release);
        // the app's controller buzzes go down the link (dropped without a headset that plays them)
        cx.cx.xr_take_haptics(&mut self.haptics);
        if !self.haptics.is_empty() && shared.haptics_ok.load(Ordering::Acquire) {
            let mut q = shared.haptics.lock().unwrap();
            if q.len() < 32 {
                q.extend_from_slice(&self.haptics);
            }
        }
        // mixed reality or VR as the app asks, under the user's view
        if shared.app_see_through.swap(cx.cx.xr_see_through(), Ordering::AcqRel) != cx.cx.xr_see_through() {
            shared.apply_view();
        }
        let Some((display_ns, _)) = cx.cx.xr_frame_wire() else { return };
        let Some(eye) = shared.session_eye() else { return };
        if display_ns == self.last_display_ns {
            return;
        }
        self.last_display_ns = display_ns;
        self.counts.frames += 1;
        while self.slots.len() < SLOTS {
            let name = ["xr_stream_picture_0", "xr_stream_picture_1", "xr_stream_picture_2"][self.slots.len()];
            self.slots.push(Slot { target: RenderTarget::new(cx.cx, name), handle: None, planes: Vec::new(), picture: [0, 0], busy: Arc::new(AtomicBool::new(false)) });
        }
        // a slot no encode reads (the one after the last, else any)
        let Some(slot) = (0..SLOTS).map(|i| (self.next + i) % SLOTS).find(|i| !self.slots[*i].busy.load(Ordering::Acquire)) else {
            shared.note_skipped();
            // the next pose once an encode frees a slot (its wake)
            self.owed = true;
            return;
        };
        self.next = (slot + 1) % SLOTS;
        // the picture: each eye's encoded column (foveated), its colour over its band
        let (fovea, [ew, eh]) = shared.session_fovea().unwrap_or((Fovea::OFF, eye));
        let (w, h) = (ew * 2, eh * 3 / 2);
        let s = &mut self.slots[slot];
        if !self.nv12 && s.target.ensure(cx.cx, (w / 4, h + h / 2), RenderTargetFormat::SharedBgra8) {
            s.handle = None;
        }
        s.picture = [w as u32, h as u32];
        // the colour and depth as the XR pass rendered them: `ss` of their pixels per eye
        // pixel (a supersampled pass; the same rounding as the pass's size)
        let ss = cx.cx.xr_supersample() as f64;
        let (sw, sh) = ((eye[0] as f64 * ss).round() as u32, (eye[1] as f64 * ss).round() as u32);
        let extent = vec2((2 * sw) as f32, sh as f32);
        // FXAA (a mode with it): the eyes anti-aliased into a target of their size, each eye
        // on its own; the foveation reads that (it paints after the XR pass, before the pack)
        let fxaa = if aa.fxaa {
            let target = self.fxaa.get_or_insert_with(|| RenderTarget::new(cx.cx, "xr_stream_fxaa"));
            target.ensure(cx.cx, (2 * sw, sh), RenderTargetFormat::Bgra8);
            target.begin(cx, vec4(0.0, 0.0, 0.0, 0.0));
            self.draw_fxaa.draw_vars.set_texture(0, color);
            self.draw_fxaa.extent = extent;
            self.draw_fxaa.split = 2.0;
            self.draw_fxaa.subpix = 0.5;
            self.draw_fxaa.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2((2 * sw) as f64, sh as f64) });
            target.end(cx);
            Some(target.texture.clone())
        } else {
            if let Some(mut t) = self.fxaa.take() { t.release(cx.cx); }
            None
        };
        let color = fxaa.as_ref().unwrap_or(color);
        // the eyes into their encoded columns, warped by the frame's own fields of view (the
        // headset unwarps by the ones the picture's head carries: the same)
        let fovs = match cx.cx.xr_frame_wire().and_then(|(_, w)| makepad_widgets::makepad_platform::makepad_hal::buffers::read_items::<makepad_widgets::makepad_platform::makepad_hal::XrFrame>(&w, 1).next()) {
            Some(f) => [f.eyes[0].fov, f.eyes[1].fov],
            None => [[-0.785, 0.785, 0.785, -0.785]; 2],
        };
        let [fov_color, fov_depth] = self.foveated.get_or_insert_with(|| [RenderTarget::new(cx.cx, "xr_stream_foveated"), RenderTarget::new(cx.cx, "xr_stream_foveated_depth")]);
        let (n, f) = (EYE_NEAR, EYE_FAR);
        for (mode, target) in [(0.0f32, &mut *fov_color), (1.0, &mut *fov_depth)] {
            target.ensure(cx.cx, (2 * ew, eh), RenderTargetFormat::Bgra8);
            target.begin(cx, vec4(0.0, 0.0, 0.0, 0.0));
            for e in 0..2 {
                let [fx, fy] = fovea.axes(fovs[e]);
                let d = &mut self.draw_foveate;
                d.draw_vars.set_texture(0, color);
                d.draw_vars.set_texture(1, depth);
                let set = |d: &mut DrawXrFoveate, cx: &mut Cx2d, id: LiveId, v: &[f32]| d.draw_vars.set_uniform(cx.cx, id, v);
                set(d, cx, live_id!(extent), &[extent.x, extent.y]);
                set(d, cx, live_id!(column), &[(e as u32 * sw) as f32, sw as f32, sh as f32, 0.0]);
                set(d, cx, live_id!(fx), &fx.shape());
                set(d, cx, live_id!(fy), &fy.shape());
                set(d, cx, live_id!(fk), &[fx.ramp, fx.edge, fy.ramp, fy.edge]);
                set(d, cx, live_id!(clip), &[2.0 * f * n, f + n, f - n, DEPTH_NEAR_M]);
                set(d, cx, live_id!(mode), &[mode]);
                d.new_draw_call(cx);
                d.draw_abs(cx, Rect { pos: dvec2((e as u32 * ew) as f64, 0.0), size: dvec2(ew as f64, eh as f64) });
            }
            target.end(cx);
        }
        let (fov_color, fov_depth) = (fov_color.texture.clone(), fov_depth.texture.clone());
        let fextent = vec2((2 * ew) as f32, eh as f32);
        if self.nv12 {
            self.pack_planes(cx, slot, &fov_color, &fov_depth, [ew, eh], fextent);
            self.chain_foveation(cx, slot, aa.fxaa);
            self.slots[slot].busy.store(true, Ordering::Release);
            self.counts.packed += 1;
            self.packed = Some(Packed { slot, display_ns, built_at: Instant::now() });
            if let Some(signal) = &self.signal {
                signal.set();
            }
            return;
        }
        let s = &mut self.slots[slot];
        s.target.begin(cx, vec4(0.0, 0.0, 0.0, 0.0));
        // the colour's luma (rows 0..eh) and chroma (rows h..h + eh/2)
        self.draw_luma.draw_vars.set_texture(0, &fov_color);
        self.draw_luma.source = vec4(0.0, 0.0, fextent.x, fextent.y);
        self.draw_luma.extent = fextent;
        self.draw_luma.scale = vec2(1.0, 1.0);
        self.draw_luma.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2((w / 4) as f64, eh as f64) });
        self.draw_chroma.draw_vars.set_texture(0, &fov_color);
        self.draw_chroma.source = vec4(0.0, 0.0, fextent.x, fextent.y);
        self.draw_chroma.extent = fextent;
        self.draw_chroma.scale = vec2(1.0, 1.0);
        self.draw_chroma.full = 0.0;
        self.draw_chroma.draw_abs(cx, Rect { pos: dvec2(0.0, h as f64), size: dvec2((w / 4) as f64, (eh / 2) as f64) });
        // the band's luma (rows eh..h) and its neutral chroma (rows h + eh/2 .. h + h/2)
        self.draw_band.draw_vars.set_texture(0, &fov_color);
        self.draw_band.draw_vars.set_texture(1, &fov_depth);
        self.draw_band.extent = fextent;
        self.draw_band.eye = vec2(ew as f32, eh as f32);
        self.draw_band.neutral = 0.0;
        self.draw_band.lanes = 4.0;
        self.draw_band.origin = 0.0;
        self.draw_band.draw_abs(cx, Rect { pos: dvec2(0.0, eh as f64), size: dvec2((w / 4) as f64, (h - eh) as f64) });
        self.draw_band.new_draw_call(cx);
        self.draw_band.neutral = 1.0;
        self.draw_band.draw_abs(cx, Rect { pos: dvec2(0.0, (h + eh / 2) as f64), size: dvec2((w / 4) as f64, (h / 2 - eh / 2) as f64) });
        // MAKEPAD_XR_REMOTE_TEST=1: the stereo test pattern over it all (in the encoded
        // columns: its optical axes where the foveation puts them, its grid warped)
        if self.test {
            let axis = |fov: [f32; 4]| {
                let [l, r, u, d] = fov.map(f32::tan);
                let [fx, fy] = fovea.axes(fov);
                vec2(fx.to_encoded(-l / (r - l)), fy.to_encoded(u / (u - d)))
            };
            let (al, ar) = (axis(fovs[0]), axis(fovs[1]));
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
        let first = s.target.pass.draw_pass_id();
        self.chain_foveation_to(cx, first, aa.fxaa);
        self.slots[slot].busy.store(true, Ordering::Release);
        self.counts.packed += 1;
        self.packed = Some(Packed { slot, display_ns, built_at: Instant::now() });
        if let Some(signal) = &self.signal {
            signal.set();
        }
    }

    /// The passes before the pack (it samples them), in paint order: FXAA (a mode with it,
    /// sampled by the foveation), the foveated colour, the foveated depth, then `pack`, the
    /// pack's first pass.
    fn chain_foveation_to(&mut self, cx: &mut Cx2d, pack: DrawPassId, fxaa: bool) {
        let Some([color, depth]) = self.foveated.as_ref() else { return };
        depth.pass.set_pass_parent(cx.cx, pack);
        color.pass.set_pass_parent(cx.cx, depth.pass.draw_pass_id());
        if let (true, Some(t)) = (fxaa, self.fxaa.as_ref()) {
            t.pass.set_pass_parent(cx.cx, color.pass.draw_pass_id());
        }
    }

    /// [`Self::chain_foveation_to`] the slot's planes (`pack_planes` chained them).
    fn chain_foveation(&mut self, cx: &mut Cx2d, slot: usize, fxaa: bool) {
        if let Some(first) = self.slots[slot].planes.first().map(|(p, _)| p.luma.draw_pass_id()) {
            self.chain_foveation_to(cx, first, fxaa);
        }
    }

    /// The foveated picture (`color` / `depth`: both eyes' encoded columns, `extent` px; eyes
    /// `eye` px encoded) into the slot's NV12 textures, a tile (eye column) each: per tile a luma
    /// pass (the colour's luma rows, then the band's coverage and inverse depth) and a chroma
    /// pass (the colour's U V, then the band's neutral chroma). The passes paint in a chain
    /// after the foveation's (`chain_foveation`).
    fn pack_planes(&mut self, cx: &mut Cx2d, slot: usize, color: &Texture, depth: &Texture, eye: [u32; 2], extent: Vec2f) {
        let [ew, eh] = eye;
        let h = eh * 3 / 2;
        let tiles = self.shared().session_tiles().max(1);
        let tw = ew * 2 / tiles;
        let s = &mut self.slots[slot];
        while s.planes.len() < tiles as usize {
            s.planes.push((Nv12Target::new(cx.cx, "xr_stream_picture_nv12"), None));
        }
        s.planes.truncate(tiles as usize);
        for (p, handle) in s.planes.iter_mut() {
            if p.ensure(cx.cx, (tw, h)) {
                *handle = None;
            }
        }
        let mut chain: Vec<DrawPassId> = Vec::new();
        for t in 0..tiles as usize {
            let x0 = (t as u32 * tw) as f32;
            // luma: the colour's (rows 0..eh), the band's (rows eh..h)
            self.slots[slot].planes[t].0.begin_luma(cx, 0.0);
            self.draw_luma.draw_vars.set_texture(0, color);
            self.draw_luma.source = vec4(x0, 0.0, tw as f32, extent.y);
            self.draw_luma.extent = extent;
            self.draw_luma.scale = vec2(1.0, 1.0);
            self.draw_luma.lanes = 1.0;
            self.draw_luma.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(tw as f64, eh as f64) });
            self.draw_band.draw_vars.set_texture(0, color);
            self.draw_band.draw_vars.set_texture(1, depth);
            self.draw_band.extent = extent;
            self.draw_band.eye = vec2(ew as f32, eh as f32);
            self.draw_band.neutral = 0.0;
            self.draw_band.lanes = 1.0;
            self.draw_band.origin = x0;
            self.draw_band.draw_abs(cx, Rect { pos: dvec2(0.0, eh as f64), size: dvec2(tw as f64, (h - eh) as f64) });
            self.slots[slot].planes[t].0.end_luma(cx);
            // chroma: the colour's (rows 0..eh/2), the band's neutral (the rest)
            self.slots[slot].planes[t].0.begin_chroma(cx, vec2(0.50196, 0.50196));
            self.draw_chroma.draw_vars.set_texture(0, color);
            self.draw_chroma.source = vec4(x0, 0.0, tw as f32, extent.y);
            self.draw_chroma.extent = extent;
            self.draw_chroma.scale = vec2(1.0, 1.0);
            self.draw_chroma.full = 0.0;
            self.draw_chroma.lanes = 1.0;
            self.draw_chroma.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2((tw / 2) as f64, (eh / 2) as f64) });
            self.slots[slot].planes[t].0.end_chroma(cx);
            let p = &self.slots[slot].planes[t].0;
            chain.push(p.luma.draw_pass_id());
            chain.push(p.chroma.draw_pass_id());
        }
        // one chain painted after the foveation: each plane; the last one has no parent (held
        // by no window)
        let planes = &self.slots[slot].planes;
        for (k, pass) in planes.iter().flat_map(|(p, _)| [&p.luma, &p.chroma]).enumerate() {
            match chain.get(k + 1) {
                Some(next) => pass.set_pass_parent(cx.cx, *next),
                None => pass.detach(cx.cx),
            }
        }
    }

    /// The app drew this XR frame without content (`why`): nothing is streamed for it (the
    /// headset repeats its picture before, instead of a frame of only the room), counted on
    /// the stats line, and the next pose is asked for.
    fn skip_frame(&mut self, cx: &mut Cx, why: &'static str) {
        if !cx.xr_offscreen() {
            return;
        }
        let Some((display_ns, _)) = cx.xr_frame_wire() else { return };
        if display_ns == self.last_display_ns {
            return;
        }
        self.last_display_ns = display_ns;
        match self.counts.skipped.iter_mut().find(|(w, _)| *w == why) {
            Some((_, n)) => *n += 1,
            None => self.counts.skipped.push((why, 1)),
        }
        self.owed = true;
        if let Some(signal) = &self.signal {
            signal.set();
        }
    }

    /// The server's wake: after a packed frame was submitted, the picture goes to the encoder
    /// with the frame's completion; the next pose is asked for once there is room.
    fn wake(&mut self, cx: &mut Cx) {
        // a headset linked or gone: the desktop's mirrors show it (or that none streams)
        let live = self.shared().frames_flowing();
        if !live && self.load_at.take().is_some() {
            cx.set_pass_load_enabled(false);
            self.shared().host_load.lock().unwrap().clear();
        }
        let source = cx.global::<crate::mirror::XrMirrorSource>();
        if source.live != live {
            source.live = live;
            cx.redraw_all();
        }
        if !self.signal.as_ref().is_some_and(|s| s.check_and_clear()) {
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

    /// The app's load by what its passes draw for (`Cx::take_pass_load`), a sample a second
    /// to the server while a headset's frames flow (measured only then).
    fn host_load(&mut self, cx: &mut Cx, shared: &ServerShared) {
        let flowing = shared.frames_flowing();
        cx.set_pass_load_enabled(flowing);
        if !flowing {
            if self.load_at.take().is_some() {
                shared.host_load.lock().unwrap().clear();
            }
            return;
        }
        let now = Instant::now();
        match self.load_at {
            None => {
                // the first sample starts here
                cx.take_pass_load();
                self.load_at = Some(now);
            }
            Some(at) if now - at >= std::time::Duration::from_secs(1) => {
                let sample = cx.take_pass_load();
                self.load_at = Some(now);
                let mut q = shared.host_load.lock().unwrap();
                if q.len() >= super::HOST_LOAD_SECONDS {
                    q.pop_front();
                }
                q.push_back(sample);
            }
            Some(_) => {}
        }
    }

    fn room(&self) -> bool {
        self.slots.iter().any(|s| !s.busy.load(Ordering::Acquire)) && self.shared().room()
    }

    fn hand_on(&mut self, cx: &mut Cx, p: Packed) {
        let busy = self.slots[p.slot].busy.clone();
        let free = || busy.store(false, Ordering::Release);
        let Some(surfaces) = self.surfaces(cx, p.slot) else {
            // not exported yet: the next frame's picture follows
            free();
            return;
        };
        // the frame's completion (this submits the frame): waited for on the encoder's side, the
        // UI never waits for it
        let ready = match cx.frame_completion() {
            Ok(ready) => ready,
            Err(e) => {
                log!("xr serve: frame completion: {e}");
                free();
                return;
            }
        };
        let picture = Picture {
            surfaces,
            ready,
            held: Box::new(Held(busy.clone())),
            display_ns: p.display_ns,
            built_at: p.built_at,
            focus_m: DEFAULT_FOCUS_M,
        };
        // dropped (and the slot freed) when no headset streams
        if self.shared().submit(picture) {
            self.counts.handed += 1;
        }
    }

    /// The slot's textures as the encoder's surfaces (exported once per texture): the packed
    /// picture, or an NV12 picture per tile.
    fn surfaces(&mut self, cx: &mut Cx, slot: usize) -> Option<Vec<SurfaceHandle>> {
        if !self.nv12 {
            return self.handle(cx, slot).map(|h| vec![h]);
        }
        let s = &mut self.slots[slot];
        let mut out = Vec::with_capacity(s.planes.len());
        for (p, handle) in s.planes.iter_mut() {
            let texture = p.texture.clone();
            if !handle.as_ref().is_some_and(|(_, t)| *t == texture) {
                let (w, h) = p.size();
                *handle = Some((cx.encode_surface(&texture, [w, h], SurfaceFormat::Nv12)?, texture));
            }
            out.push(handle.as_ref().unwrap().0.clone());
        }
        (!out.is_empty()).then_some(out)
    }

    /// The slot's packed texture as the encoder's surface (exported once per texture).
    fn handle(&mut self, cx: &mut Cx, slot: usize) -> Option<SurfaceHandle> {
        let s = &mut self.slots[slot];
        let texture = s.target.texture.clone();
        if let Some((handle, t)) = &s.handle {
            if *t == texture {
                return Some(handle.clone());
            }
        }
        let handle = cx.encode_surface(&texture, s.picture, SurfaceFormat::PackedNv12)?;
        s.handle = Some((handle.clone(), texture));
        Some(handle)
    }
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
    /// Frames the app drew without content (`skip_frame`), by reason.
    skipped: Vec<(&'static str, u32)>,
    /// The desktop's window paints (`Cx::window_paints`): at the window's start and now.
    desktop_at: u64,
    desktop_now: u64,
}

impl Counts {
    fn window(&mut self, shared: &ServerShared) {
        let at = *self.at.get_or_insert_with(Instant::now);
        let secs = at.elapsed().as_secs_f64();
        if secs < 5.0 {
            return;
        }
        let r = |n: u32| n as f64 / secs;
        let skipped: Vec<String> = self.skipped.iter().map(|(why, n)| format!("{n} {why}")).collect();
        let painted = self.desktop_now - self.desktop_at.min(self.desktop_now);
        // the host's frame by what its passes draw for, per frame packed (the last seconds)
        let per = r(self.packed).max(1.0);
        let load = match super::HostLoad::of(shared.host_load.lock().unwrap().iter()) {
            Some(l) => format!(" · per frame: draw {:.1} ms, GPU eyes {:.1} desktop {:.1} other {:.1} ms{}", l.draw_ms_s / per, l.eyes_gpu_ms_s / per, l.desktop_gpu_ms_s / per, l.other_gpu_ms_s / per, if l.timed { "" } else { " (untimed)" }),
            None => String::new(),
        };
        log!("xr serve: draws {:.0}/s · new XR frames {:.0}/s · packed {:.0}/s · handed {:.0}/s · poses asked {:.0}/s · waited for room {} · not streamed (empty) {} · desktop painted {:.1}/s{load}", r(self.draws), r(self.frames), r(self.packed), r(self.handed), r(self.asked), self.waited, if skipped.is_empty() { "0".to_string() } else { skipped.join(", ") }, painted as f64 / secs);
        // the app's audio (what the headset hears) stops for good when its callback panics
        {
            use makepad_widgets::makepad_platform::audio_output_fence as fence;
            if let Some(said) = fence::take_audio_output_panic_note(0) {
                log!("xr serve: the app's audio output callback panicked ({} times): {said}", fence::audio_output_panics(0));
            }
        }
        *self = Counts { at: Some(Instant::now()), desktop_at: self.desktop_now, desktop_now: self.desktop_now, ..Default::default() };
    }
}

impl XrFrameSink for XrPack {
    fn presented(&mut self, cx: &mut Cx2d, frame: &XrPresented) {
        self.pack(cx, frame.color, frame.depth);
    }

    fn empty(&mut self, cx: &mut Cx, why: &'static str) {
        self.skip_frame(cx, why);
    }

    fn signal(&mut self, cx: &mut Cx) {
        self.wake(cx);
    }
}

/// The app's sound to a streaming headset: a tap on its program output (after its mix), which
/// silences the desktop's own output while the headset hears it (`MAKEPAD_XR_REMOTE_MUTE_LOCAL=0`
/// keeps it). Realtime-safe.
pub(super) fn tap_audio_output() {
    use makepad_widgets::makepad_platform::audio_output_tap::{add_audio_output_tap, set_program_output_silenced};
    let tap = makepad_desk_link::xr_audio::XrAudioTap::new();
    let added = add_audio_output_tap(move |info, output| {
        let (frames, channels) = (output.frame_count(), output.channel_count());
        if channels == 0 {
            return;
        }
        let left = output.channel(0);
        let right = output.channel(if channels >= 2 { 1 } else { 0 });
        set_program_output_silenced(tap.tap(info.sample_rate, frames, left, right));
    });
    if added.is_none() {
        log!("xr serve: no audio output tap free: the headset hears nothing");
    }
}
