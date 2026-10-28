//! The desktop end of remote rendering: an app serves its XR view ([`serve`]; the caption's XR
//! control starts it). A headset linking gives the process a remote headset slot, an offscreen
//! XR display
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

use makepad_widgets::makepad_draw::shader::draw_fxaa::DrawFxaa;
use makepad_widgets::makepad_draw::shader::draw_nv12_pack::{DrawNv12Chroma, DrawNv12Luma};
use makepad_widgets::makepad_draw::render_target::{RenderTarget, RenderTargetFormat};
use makepad_widgets::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
#[cfg(not(target_arch = "wasm32"))]
use makepad_widgets::makepad_platform::video_file::{SurfaceFormat, SurfaceHandle};

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
            // eye pixels to the colour's (a supersampled pass: `scale` of them per eye pixel)
            let s = self.scale
            if xe < half {
                // the 2x2 block's centre: one bilinear tap averages it (supersampled: four taps
                // over its footprint)
                let c = vec2(e * ew + xe * 2.0 + 1.0, r * 2.0 + 1.0) * s
                if s.x * s.y <= 1.0 {
                    return self.image.sample(c * inv).w
                }
                let d = s * 0.5
                return (self.image.sample((c + vec2(-d.x, -d.y)) * inv).w + self.image.sample((c + vec2(d.x, -d.y)) * inv).w
                    + self.image.sample((c + vec2(-d.x, d.y)) * inv).w + self.image.sample((c + vec2(d.x, d.y)) * inv).w) * 0.25
            }
            let p = vec2(e * ew + (xe - half) * 2.0 + 0.5, r * 2.0 + 0.5) * s
            let d0 = self.depth_tex.sample(p * inv).x
            let d1 = self.depth_tex.sample((p + vec2(s.x, 0.0)) * inv).x
            let d2 = self.depth_tex.sample((p + vec2(0.0, s.y)) * inv).x
            let d3 = self.depth_tex.sample((p + s) * inv).x
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
        draw_fxaa +: {}
        draw_band +: {}
        draw_mirror +: {}
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
    /// The colour/depth pixels per eye pixel (a supersampled XR pass; 1: none).
    #[live]
    pub scale: Vec2f,
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
/// The eye projections' planes (the HAL's one definition): the band's depth is decoded with them.
use makepad_widgets::makepad_platform::makepad_hal::{XR_EYE_FAR_M as EYE_FAR, XR_EYE_NEAR_M as EYE_NEAR};
/// What the view centres on until the app says (m): a table-top distance.
const DEFAULT_FOCUS_M: f32 = 1.0;

struct Slot {
    target: RenderTarget,
    /// Its texture as the encoder's surface, once exported.
    handle: Option<(SurfaceHandle, Texture)>,
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

/// The app's XR serving (see the module). A field of the app's view (`mod.draw.XrServe{}`).
#[derive(Script, ScriptHook)]
pub struct XrServe {
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
    #[live]
    draw_band: DrawXrBand,
    #[live]
    draw_mirror: DrawXrMirror,
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
    /// The desktop windows are held for the headset (`Cx::xr_hold_windows`).
    #[rust]
    holding: bool,
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
    /// The desktop windows' repaints painted and held (`Cx::xr_window_counts`): at the window's
    /// start and now.
    desktop_at: (u64, u64),
    desktop_now: (u64, u64),
}

impl Counts {
    fn window(&mut self) {
        let at = *self.at.get_or_insert_with(Instant::now);
        let secs = at.elapsed().as_secs_f64();
        if secs < 5.0 {
            return;
        }
        let r = |n: u32| n as f64 / secs;
        let skipped: Vec<String> = self.skipped.iter().map(|(why, n)| format!("{n} {why}")).collect();
        let (painted, held) = (self.desktop_now.0 - self.desktop_at.0.min(self.desktop_now.0), self.desktop_now.1 - self.desktop_at.1.min(self.desktop_now.1));
        log!("xr serve: draws {:.0}/s · new XR frames {:.0}/s · packed {:.0}/s · handed {:.0}/s · poses asked {:.0}/s · waited for room {} · not streamed (empty) {} · desktop painted {:.1}/s, held {:.0}/s", r(self.draws), r(self.frames), r(self.packed), r(self.handed), r(self.asked), self.waited, if skipped.is_empty() { "0".to_string() } else { skipped.join(", ") }, painted as f64 / secs, held as f64 / secs);
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

/// Starts this process's XR server for the app `app` (once; a process without the remote headset
/// slot, or without the LAN allowed, does nothing): on a port the OS picks, found by headsets with
/// the pairing probe; the pairing is kept in the app's data folder (`xr-pairing`). The listener
/// runs until the process ends.
pub fn serve(app: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use makepad_desk_link::xr_host::{XrHost, XrHostConfig};
        static HOST: Mutex<Option<XrHost>> = Mutex::new(None);
        let mut host = HOST.lock().unwrap();
        if host.is_some() {
            return;
        }
        if !makepad_widgets::makepad_platform::makepad_network::lan::service("xr host") {
            log!("xr host: local network off for this instance (MAKEPAD_LAN=1 allows it)");
            return;
        }
        // the headset's room field, mirrored into this process's field store: the app's
        // physics (its hit-surface queries) collides with the real room
        let world: makepad_desk_link::xr_host::WorldSink = Arc::new(|voxel_size, chunk_edge, bricks| {
            use makepad_widgets::makepad_platform::xr_tsdf::{BrickGrid, BrickMirror, Brick, ChunkKey};
            static MIRROR: Mutex<Option<BrickMirror>> = Mutex::new(None);
            let bricks: Vec<Brick> = bricks.into_iter().map(|b| Brick { key: ChunkKey { x: b.key[0], y: b.key[1], z: b.key[2] }, version: b.version, payload: Arc::new(b.payload) }).collect();
            let mut mirror = MIRROR.lock().unwrap_or_else(|e| e.into_inner());
            mirror.get_or_insert_with(BrickMirror::default).apply(&makepad_widgets::makepad_platform::xr_tsdf::xr_tsdf_store(), BrickGrid { voxel_size, chunk_edge }, &bricks);
        });
        let pairing = makepad_widgets::makepad_platform::home::app_data_dir(app).map(|d| d.join("xr-pairing"));
        let config = XrHostConfig { addr: std::net::SocketAddr::from(([0, 0, 0, 0], 0)), app: app.to_string(), pairing, tls_name: "makepad xr host".into(), world: Some(world), log: Arc::new(|line: &str| log!("{line}")) };
        match XrHost::start(config) {
            Ok(h) => *host = Some(h),
            Err(e) => log!("{e}"),
        }
        if std::env::var("MAKEPAD_XR_REMOTE_SELFTEST").as_deref() == Ok("1") {
            makepad_desk_link::xr_host::selftest([1680, 1760], Arc::new(|line: &str| log!("{line}")));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = (Mutex::<()>::new(()), app);
}

/// The stream's settings the user sets (the caption's XR dropdown, `crate::panel`): the bitrate (kbit/s, all
/// tiles; the rate controller stays below it) and the headset's display rate (Hz, one of
/// `xr_host::REFRESH_RATES`; a change re-links the headset at it).
#[cfg(not(target_arch = "wasm32"))]
pub fn set_stream_bitrate_kbps(kbps: u32) {
    makepad_desk_link::xr_host::set_bitrate_kbps(kbps);
}
#[cfg(not(target_arch = "wasm32"))]
pub fn set_stream_refresh_hz(hz: u32) {
    makepad_desk_link::xr_host::set_refresh_hz(hz);
}
/// The user's power switches for a streaming headset (see `makepad_desk_link::xr_host`): the
/// stream on/off (off: no XR frames drawn, nothing encoded or sent, the headset idles), the room
/// scan allowed, the left menu button as the stream's switch (default), mixed reality or VR.
#[cfg(not(target_arch = "wasm32"))]
pub use makepad_desk_link::xr_host::{set_menu_toggles_streaming, set_room_scan as set_stream_room_scan, set_streaming, streaming, streaming_changes};

/// The stream's anti-aliasing, one of [`STREAM_AA_MODES`] (the app's setting, applied live from
/// the next frame): hardware MSAA on the eye pass (`msaa` samples per pixel, resolved before
/// the encoder's pack), FXAA over the resolved eyes, supersampled eyes filtered down into the
/// encoded eye size (`supersample` rendered pixels per encoded pixel per axis), or a mix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamAa {
    /// Samples per pixel of the eye pass (1: no MSAA; a count the GPU lacks: the most it has
    /// below that, shown on the stats panel).
    pub msaa: u32,
    pub fxaa: bool,
    pub supersample: f32,
    /// Its id (the settings file) and its name for the user.
    pub id: &'static str,
    pub name: &'static str,
}

const fn aa(msaa: u32, fxaa: bool, supersample: f32, id: &'static str, name: &'static str) -> StreamAa {
    StreamAa { msaa, fxaa, supersample, id, name }
}

/// The modes the app offers its user, cheapest first.
pub const STREAM_AA_MODES: [StreamAa; 9] = [
    aa(1, false, 1.0, "off", "Off"),
    aa(1, true, 1.0, "fxaa", "FXAA"),
    aa(2, false, 1.0, "msaa2", "MSAA 2x"),
    aa(4, false, 1.0, "msaa4", "MSAA 4x"),
    aa(8, false, 1.0, "msaa8", "MSAA 8x"),
    aa(4, true, 1.0, "msaa4+fxaa", "MSAA 4x + FXAA"),
    aa(1, false, 1.25, "ss1.25", "Supersampled 1.25x"),
    aa(1, false, 1.5, "ss1.5", "Supersampled 1.5x"),
    aa(1, false, 2.0, "ss2", "Supersampled 2x"),
];

/// The default: MSAA 4x.
pub const STREAM_AA_DEFAULT: StreamAa = STREAM_AA_MODES[3];

impl StreamAa {
    pub fn from_id(id: &str) -> Option<StreamAa> {
        STREAM_AA_MODES.into_iter().find(|m| m.id == id)
    }
}

/// The stream's AA mode as an index into [`STREAM_AA_MODES`].
static STREAM_AA: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(3);

/// The server's eye render scale (percent of the encoded eye size, per axis; 100: as encoded):
/// below 100 the eyes render smaller and the pack scales them up into the same encoded size,
/// so the headset's picture keeps its size and nothing re-links. Under the AA's supersampling
/// (the XR pass renders at `aa.supersample * render scale`).
static RENDER_SCALE_PCT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);

/// The lowest eye render scale (percent).
pub const MIN_RENDER_SCALE_PCT: u32 = 70;

/// Sets the eye render scale (percent, [`MIN_RENDER_SCALE_PCT`]..100; live from the next frame).
pub fn set_stream_render_scale(pct: u32) {
    RENDER_SCALE_PCT.store(pct.clamp(MIN_RENDER_SCALE_PCT, 100), Ordering::Release);
}

/// The eye render scale now (percent).
pub fn stream_render_scale() -> u32 {
    RENDER_SCALE_PCT.load(Ordering::Acquire)
}

/// Sets the stream's anti-aliasing (live: the next frame renders with it).
pub fn set_stream_aa(mode: StreamAa) {
    let i = STREAM_AA_MODES.iter().position(|m| *m == mode).unwrap_or(3);
    STREAM_AA.store(i as u8, Ordering::Release);
}

/// The stream's anti-aliasing now.
pub fn stream_aa() -> StreamAa {
    STREAM_AA_MODES[(STREAM_AA.load(Ordering::Acquire) as usize).min(STREAM_AA_MODES.len() - 1)]
}

/// The app's own stats line (`XrServe::set_app_line`).
static APP_LINE: Mutex<String> = Mutex::new(String::new());

/// The app's own line in the stream's stats (empty: none).
pub fn app_line() -> String {
    APP_LINE.lock().unwrap().clone()
}

/// What the headset shows around the picture: what the app asks for ([`set_stream_see_through`]),
/// or the user's choice of mixed reality (the room shows through the picture's empty parts) or
/// VR (the picture opaque, no passthrough).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamView {
    App,
    Mixed,
    Vr,
}

impl StreamView {
    /// Its name in the settings file.
    pub fn label(self) -> &'static str {
        match self {
            StreamView::App => "app",
            StreamView::Mixed => "mr",
            StreamView::Vr => "vr",
        }
    }

    pub fn from_label(label: &str) -> Option<StreamView> {
        [StreamView::App, StreamView::Mixed, StreamView::Vr].into_iter().find(|v| v.label() == label)
    }
}

/// The app's ask (see-through) and the user's view, as [`StreamView`] indices.
static APP_SEE_THROUGH: AtomicBool = AtomicBool::new(true);
static VIEW: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn apply_view() {
    let see_through = match VIEW.load(Ordering::Acquire) {
        1 => true,
        2 => false,
        _ => APP_SEE_THROUGH.load(Ordering::Acquire),
    };
    #[cfg(not(target_arch = "wasm32"))]
    makepad_desk_link::xr_host::set_see_through(see_through);
    #[cfg(target_arch = "wasm32")]
    let _ = see_through;
}

/// The desktop's mirror of the headset's view (`XrServe::draw_mirror`, `XrMirrorView`; the
/// caption's XR dropdown): repainted from the headset's frames at a few frames a second
/// (`Cx::xr_hold_windows`), else only with the desktop's own UI changes.
static MIRROR: AtomicBool = AtomicBool::new(true);

pub fn set_stream_mirror(on: bool) {
    MIRROR.store(on, Ordering::Release);
}

/// The app's view for its content: mixed reality (true) or VR; the user's choice
/// ([`set_stream_view`]) goes over it.
pub fn set_stream_see_through(on: bool) {
    APP_SEE_THROUGH.store(on, Ordering::Release);
    apply_view();
}

/// The user's view (the caption's XR dropdown).
pub fn set_stream_view(view: StreamView) {
    VIEW.store(match view { StreamView::App => 0, StreamView::Mixed => 1, StreamView::Vr => 2 }, Ordering::Release);
    apply_view();
}

/// The app's tap of its own audio for a remote headset: make it on the app's thread, move it
/// into the audio output callback and pass it to [`tap_audio`] there after the mix.
#[cfg(not(target_arch = "wasm32"))]
pub use makepad_desk_link::xr_audio::XrAudioTap;

/// After the app's mix in its audio output callback: the block goes to the headset (while one
/// streams) and, when the headset hears it, the desktop's own output is silenced (a setting:
/// `MAKEPAD_XR_REMOTE_MUTE_LOCAL=0` keeps it). Realtime-safe.
#[cfg(not(target_arch = "wasm32"))]
pub fn tap_audio(tap: &XrAudioTap, sample_rate: f64, output: &mut makepad_widgets::makepad_platform::audio::AudioBuffer) {
    let (frames, channels) = (output.frame_count(), output.channel_count());
    if channels == 0 {
        return;
    }
    let mute = {
        let left = output.channel(0);
        let right = output.channel(if channels >= 2 { 1 } else { 0 });
        tap.tap(sample_rate, frames, left, right)
    };
    if mute {
        output.zero();
    }
}

impl XrServe {
    /// A line of the app's own numbers in the stream's stats (the caption's XR dropdown), under
    /// the stream's (empty: none). Kept until replaced.
    pub fn set_app_line(&mut self, line: &str) {
        let mut app = APP_LINE.lock().unwrap();
        if *app != line {
            app.clear();
            app.push_str(line);
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
            // while a headset streams the desktop windows paint for their own causes only, and
            // a window with the mirror from the headset's frames at the mirror's rate
            let hold = xr_host::session_eye().is_some() && streaming();
            let source = cx.cx.global::<crate::mirror::XrMirrorSource>();
            source.color = Some(color.clone());
            if let Some(eye) = xr_host::session_eye() {
                source.eye = eye;
            }
            let mirror = std::mem::take(&mut source.drawn) && MIRROR.load(Ordering::Acquire);
            cx.cx.xr_hold_windows(hold, mirror);
            self.holding = hold;
            self.counts.desktop_now = cx.cx.xr_window_counts();
            self.counts.window();
            // the stream's AA and render scale: the XR pass renders multisampled, at the supersample
            // times the render scale of the encoded eye size (from this frame on)
            let aa = stream_aa();
            cx.cx.xr_set_supersample(aa.supersample * stream_render_scale() as f32 / 100.0);
            cx.cx.xr_set_msaa(aa.msaa);
            // the headset integrates and sends its room field only while the app uses it
            xr_host::set_room_field_wanted(cx.cx.xr_room_field_wanted());
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
                self.slots.push(Slot { target: RenderTarget::new(cx.cx, name), handle: None, picture: [0, 0], busy: Arc::new(AtomicBool::new(false)) });
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
            s.picture = [w as u32, h as u32];
            // the colour and depth as the XR pass rendered them: `ss` of their pixels per eye
            // pixel (a supersampled pass; the same rounding as the pass's size)
            let ss = cx.cx.xr_supersample() as f64;
            let (sw, sh) = ((ew as f64 * ss).round() as u32, (eh as f64 * ss).round() as u32);
            let scale = vec2(sw as f32 / ew as f32, sh as f32 / eh as f32);
            let extent = vec2((2 * sw) as f32, sh as f32);
            // FXAA (a mode with it): the eyes anti-aliased into a target of their size, each eye
            // on its own; the pack reads that (it paints after the XR pass, before the pack)
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
            let s = &mut self.slots[slot];
            s.target.begin(cx, vec4(0.0, 0.0, 0.0, 0.0));
            // the colour's luma (rows 0..eh) and chroma (rows h..h + eh/2)
            self.draw_luma.draw_vars.set_texture(0, color);
            self.draw_luma.source = vec4(0.0, 0.0, extent.x, extent.y);
            self.draw_luma.extent = extent;
            self.draw_luma.scale = scale;
            self.draw_luma.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2((w / 4) as f64, eh as f64) });
            self.draw_chroma.draw_vars.set_texture(0, color);
            self.draw_chroma.source = vec4(0.0, 0.0, extent.x, extent.y);
            self.draw_chroma.extent = extent;
            self.draw_chroma.scale = scale;
            self.draw_chroma.full = 0.0;
            self.draw_chroma.draw_abs(cx, Rect { pos: dvec2(0.0, h as f64), size: dvec2((w / 4) as f64, (eh / 2) as f64) });
            // the band's luma (rows eh..h) and its neutral chroma (rows h + eh/2 .. h + h/2)
            self.draw_band.draw_vars.set_texture(0, color);
            self.draw_band.draw_vars.set_texture(1, depth);
            self.draw_band.extent = extent;
            self.draw_band.eye = vec2(ew as f32, eh as f32);
            self.draw_band.scale = scale;
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
            // the FXAA pass paints before the pack that samples it
            if let (true, Some(t)) = (aa.fxaa, self.fxaa.as_ref()) {
                t.pass.set_pass_parent(cx.cx, s.target.pass.draw_pass_id());
            }
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

    /// The app drew this XR frame without content (`why`): nothing is streamed for it (the
    /// headset repeats its picture before, instead of a frame of only the room), counted on
    /// the stats line, and the next pose is asked for.
    pub fn skip_frame(&mut self, cx: &mut Cx2d, why: &'static str) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !cx.cx.xr_offscreen() {
                return;
            }
            let Some((display_ns, _)) = cx.cx.xr_frame_wire() else { return };
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
        #[cfg(target_arch = "wasm32")]
        let _ = (cx, why);
    }

    /// The desktop's mirror: the left eye of `color` (the XR pass's side-by-side colour) fitted
    /// into `rect`.
    pub fn draw_mirror(&mut self, cx: &mut Cx2d, rect: Rect, color: &Texture) {
        let eye = self.eye.unwrap_or([1680, 1760]);
        self.draw_mirror.draw_vars.set_texture(0, color);
        self.draw_mirror.draw_abs(cx, crate::mirror::fit(rect, eye));
        cx.cx.global::<crate::mirror::XrMirrorSource>().drawn = true;
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
            // the headset gone or its stream off: the desktop paints as always again
            if self.holding && (xr_host::session_eye().is_none() || !streaming()) {
                self.holding = false;
                cx.xr_hold_windows(false, false);
            }
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
        let picture = xr_host::Picture {
            surface: handle,
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

    /// The slot's texture as the encoder's surface (exported once per texture).
    #[cfg(not(target_arch = "wasm32"))]
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
