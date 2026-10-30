//! Remote rendering for headsets over Desk Link XR (`makepad_desk_link::xr`).
//!
//! - [`XrServer`] (`server`): an app serves its XR view (a headset linking gives the process a
//!   remote headset slot): it renders as on a headset (its XR pass, paced by the headset's
//!   poses); the server takes each frame from the Cx's XR frame sink, packs it on the GPU,
//!   encodes and sends it. The caption's [`XrCaptionButton`] installs it; [`XrMirrorView`] (or
//!   [`DrawXrMirror::draw_headset`]) mirrors it on the desktop.
//! - [`XrStreamClient`]: on the headset, a thin client: it sends each XR frame's pose (head,
//!   eyes, controllers, hands) upstream, decodes the pictures on a worker thread and draws the
//!   newest into the app's XR pass with [`DrawXrStream`], each eye its half of the picture.
//!   The app keeps no state of the remote app; its own XR window only shows the stream.
//!
//! Stats per picture (logged every 5 s, [`XrStreamClient::status`]): the server's render, read,
//! convert and encode, the round trip to decoded, the decode, and the render pose's age at the
//! display time of the frame that shows it.

use makepad_widgets::makepad_draw::render_target::RenderTarget;
#[cfg(not(target_arch = "wasm32"))]
use makepad_widgets::makepad_draw::render_target::RenderTargetFormat;
use makepad_widgets::makepad_draw::shader::draw_nv12::DrawNv12;
use makepad_widgets::*;
/// Where a headset's stream comes from: a server at an address (a debug switch), or the server
/// it pairs with by its PIN (`makepad_desk_link::xr_pair`; `pairing`: the file its instance id,
/// PIN and pairs are kept in; `name`: what servers are told it is).
#[derive(Clone, Debug)]
pub enum XrTarget {
    Address { host: String, port: u16 },
    Pair { pairing: std::path::PathBuf, name: String },
}
/// The room field goes up in messages of about this size (bytes).
const WORLD_MESSAGE_BYTES: usize = 16 << 10;
/// The display period the client expects (ns; the stream runs at 90 Hz).
/// The display period (ns) of the rate the link runs at (`XrServerMsg::Refresh`; 90 Hz until
/// linked).
static PERIOD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(11_111_111);
fn period_ns() -> u64 {
    PERIOD.load(std::sync::atomic::Ordering::Relaxed)
}
fn set_refresh_mhz(mhz: u32) {
    PERIOD.store(1_000_000_000_000 / mhz.max(1_000) as u64, std::sync::atomic::Ordering::Relaxed);
}
/// Decoded pictures a tile holds waiting for their display frame (with the shown one and the
/// two before it nine of the decoder reader's eleven images: the decoder never waits for one).
/// The lead never asks for more waiting than this holds (`steer_ahead`): a picture let go here
/// was the one due next, a repeat.
const HOLD: usize = 6;

mod auto;
mod mirror;
mod panel;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
mod settings_types;
pub use mirror::{DrawXrMirror, XrMirrorClock, XrMirrorView};
pub use panel::{XrCaptionButton, XrStreamSettings};
#[cfg(not(target_arch = "wasm32"))]
pub use server::{launch_enabled, LiveHeadset, ServerStats, XrServer};
pub use settings_types::{HeadsetSettings, StreamAa, StreamView, STREAM_AA_DEFAULT, STREAM_AA_MODES};
/// The serving side's shaders and widgets: the server's pack, the mirror and the caption's
/// `XrCaptionButton` (an app with the control registers them).
pub fn serve_script_mod(vm: &mut ScriptVm) -> ScriptValue {
    #[cfg(not(target_arch = "wasm32"))]
    server::script_mod(vm);
    mirror::shader::script_mod(vm);
    mirror::script_mod(vm);
    panel::shader::script_mod(vm);
    panel::script_mod(vm)
}

#[cfg(not(target_arch = "wasm32"))]
use makepad_desk_link::xr::{link::{HeadsetInfo, XrLink}, codecs, VideoHead, XrClientMsg, XrRenderEye, XrServerMsg, CODEC_HEVC};
#[cfg(not(target_arch = "wasm32"))]
use makepad_widgets::makepad_platform::video_file::{StreamVideoCodec, VideoStreamDecoder};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // One eye's column of the stream picture over that eye's whole view: the quad is placed on
    // the view's frustum at 1 m (from the pass's own projection, so it covers the view whatever
    // the backend's clip conventions). The frame is shown with the eyes the picture was rendered
    // with (`Cx::xr_present_eyes`): the headset's compositor reprojects it to the display's
    // pose. One draw per eye; the other eye's view gets a degenerate quad (the eye is told by
    // its view's frustum: a left eye sees further left than right).
    // The band's depth of one eye (its right half: the inverse depth, one byte) copied into the
    // band target (`XrStreamClient::draw_band`) with its coverage: the stream's pixels read
    // both there with one plain fetch (and the warp's depth steps).
    mod.draw.DrawXrBandDepth = mod.std.set_type_default() do #(DrawXrBandDepth::script_shader(vm)){
        ..mod.draw.DrawQuad
        video: texture_video()
        // this eye's column of the picture (origin, size) and the picture's buffer size
        src: uniform(vec4(0.0, 0.0, 1.0, 1.0))
        nv12_size: uniform(vec2(1.0, 1.0))
        pixel: fn() {
            let ch = floor(self.src.w * 2.0 / 3.0 + 0.5)
            let vap = vec2(self.src.x, ch) + self.pos * vec2(self.src.z * 0.5, ch * 0.5)
            let vd = self.video.sample_video((floor(vap + vec2(self.src.z * 0.5, 0.0)) + vec2(0.5, 0.5)) / self.nv12_size).y
            let va = self.video.sample_video((floor(vap) + vec2(0.5, 0.5)) / self.nv12_size).y
            // .x the inverse depth, .y the coverage
            return vec4(vd, va, 0.0, 1.0)
        }
    }

    mod.draw.DrawXrStream = mod.std.set_type_default() do #(DrawXrStream::script_shader(vm)){
        ..mod.draw.DrawNv12
        // a decoder's picture where it is (`video_mode` 1): sampled it gives its raw codes
        // (.y = Y', .z = Cb, .x = Cr); `nv12_size` is then its buffer's size
        video: texture_video()
        // the headset's newest environment depth image itself, a layer per eye (its depth
        // 0..1 in .r; `env_pos.w` its near plane)
        env_depth: texture_2d_array(float)
        // the picture's band depth per eye (left half eye 0), copied out by DrawXrBandDepth:
        // what the warp's vertices read
        band_depth: texture_2d(float)
        band_size: uniform(vec2(1.0, 1.0))
        // Per draw (each eye is a draw call of its own), uniforms, not instance data: as
        // instance fields they were the fragment's inputs, and past the varyings that fit, the
        // fragment read other values (the picture showed whatever the vertex stage drew, its
        // debug switches did nothing). `eye`: 0 left, 1 right; `video_mode` 1: the picture is the
        // decoder's own (`video`), 0 NV12 bytes or planes; `occlude` 1: real things in front
        // hide the picture; `depth_near`: the band's inverse depth scale; `opaque` (a debug
        // switch) 1: the picture covers the room whatever its coverage, 2: a flat colour per
        // eye (red left, blue right), 4: the colour alone, 5: the worst case (covered everywhere,
        // a depth edge at every band texel); `render_*`: the eye the picture was rendered with
        // (orientation, position, fov tangents left right up down); `env_pos`: the environment
        // depth camera's position with its near plane in w, `env_far` its far plane (0
        // infinite), `env_size` its image's size
        eye: uniform(0.0)
        video_mode: uniform(0.0)
        occlude: uniform(0.0)
        depth_near: uniform(0.0)
        opaque: uniform(0.0)
        render_rot: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        render_pos: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        render_tan: uniform(vec4(-1.0, 1.0, 1.0, -1.0))
        env_pos: uniform(vec4(0.0, 0.0, 0.0, 0.1))
        env_far: uniform(0.0)
        env_map: uniform(0.0)
        // The depth camera's view of the picture's point (u z, v z, z, 1) (`uv` the picture's
        // point, `z` its depth along the render eye's axis), one row each (made per draw on the
        // CPU from both eyes' poses and fovs, `env_map` folded in): the depth image's (u, v)
        // times its distance, and the distance
        env_ku: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        env_kv: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        env_kz: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // 1: the picture is reprojected per pixel to the view's eye in this pass (see `warped`);
        // the frame then goes to the compositor with the view's own eyes (`view_*`: that eye:
        // orientation, position, fov tangents)
        warp: uniform(0.0)
        // 1: the frame's targets encode on write (`hal_xr_linear_frames`): output linear colour
        linear_out: uniform(0.0)
        view_rot: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        view_pos: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        view_tan: uniform(vec4(-1.0, 1.0, 1.0, -1.0))
        // The picture's depth at its point `g` (m along the render eye's axis; far where
        // nothing was drawn), from the band-depth target, nearest texel
        band_z: fn(g: vec2) -> float {
            if self.opaque > 4.5 {
                // (debug `opaque` 5, the worst case: covered everywhere, a depth edge at every
                // band texel: 1 m and 1.5 m alternating)
                let k = floor(clamp(g, vec2(0.0, 0.0), vec2(1.0, 1.0)) * self.band_size * vec2(0.5, 1.0))
                return 1.0 + 0.5 * fract((k.x + k.y) * 0.5) * 2.0
            }
            let vd = self.band_at(g).x
            return mix(50.0, self.depth_near / max(vd, 0.0001), step(0.004, vd))
        }
        // The band target at the picture's point `g` (nearest texel): .x inverse depth, .y coverage
        band_at: fn(g: vec2) -> vec4 {
            let bs = self.band_size
            let half = vec2(bs.x * 0.5, bs.y)
            let gc = clamp(g, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let t = (min(floor(gc * half), half - vec2(1.0, 1.0)) + vec2(0.5, 0.5) + vec2(self.eye * half.x, 0.0)) / bs
            return self.band_depth.sample_lod(t, 0.0)
        }
        // The coverage at the picture's point `g`, filtered between the band's texels (the
        // colour is full resolution and premultiplied on black: the nearest half-resolution
        // coverage put a dark line along every edge where it said 1 and the colour was already
        // fading to black)
        band_coverage: fn(g: vec2) -> float {
            let bs = self.band_size
            let half = vec2(bs.x * 0.5, bs.y)
            let p = clamp(g * half, vec2(0.5, 0.5), half - vec2(0.5, 0.5))
            return self.band_depth.sample_lod((p + vec2(self.eye * half.x, 0.0)) / bs, 0.0).y
        }
        env_size: uniform(vec2(1.0, 1.0))
        qrot: fn(q: vec4, v: vec3) -> vec3 {
            let t = cross(q.xyz, v) * 2.0
            return v + t * q.w + cross(q.xyz, t)
        }
        // The picture's point at this pixel at depth `z` (m, along the render eye's axis) seen by
        // the depth camera: (its distance there, what the camera measured along that ray); y < 0
        // where the depth image has nothing
        room_depth: fn(uv: vec2, z: float) -> vec2 {
            // (the camera's view of the point: `env_k*`)
            let h = vec4(uv.x * z, uv.y * z, z, 1.0)
            let ze = dot(self.env_kz, h)
            if ze <= 0.01 {
                return vec2(ze, -1.0)
            }
            let t = vec2(dot(self.env_ku, h), dot(self.env_kv, h)) / ze
            if t.x < 0.0 || t.x > 1.0 || t.y < 0.0 || t.y > 1.0 {
                return vec2(ze, -1.0)
            }
            // (this eye's layer, read at the texel's centre)
            let ts = self.env_size
            let tc = (floor(t * ts) + vec2(0.5, 0.5)) / ts
            let dv = self.env_depth.sample(vec3(tc.x, tc.y, self.eye)).x
            if dv >= 0.99999 {
                return vec2(ze, -1.0)
            }
            // the depth camera's distance there (an infinite far plane: z = near / (1 - d))
            let n = self.env_pos.w
            let f = self.env_far
            return vec2(ze, mix(n / (1.0 - dv), n * f / max(f - dv * (f - n), 0.0001), step(0.001, f)))
        }
        // How much of the picture a real thing in front hides at this pixel (0..1): the
        // picture's point at its depth (the band's inverse depth), seen by the depth camera, is
        // behind what that camera measured there.
        occlusion: fn(uv: vec2, z: float) -> float {
            if self.occlude < 0.5 || z <= 0.0 {
                return 0.0
            }
            let r = self.room_depth(uv, z)
            if r.y < 0.0 {
                return 0.0
            }
            return clamp((r.x - r.y - 0.02) / 0.04, 0.0, 1.0)
        }
        // The display pixel's ray in the render eye's frame (`warp`: from the vertices, exact:
        // the ray is linear across the view) and the view eye's offset from the render eye there
        ray: varying(vec3)
        ray_o: varying(vec3)
        // The picture's point where the display pixel's ray (`o`, `dr`: in the render eye's
        // frame) is at render depth `z`
        ray_at: fn(o: vec3, dr: vec3, z: float) -> vec2 {
            let t = (0.0 - z - o.z) / min(dr.z, -0.0001)
            let p = o + dr * t
            let rt = self.render_tan
            let w = max(0.0 - p.z, 0.0001)
            return vec2((p.x / w - rt.x) / (rt.y - rt.x), (p.y / w - rt.z) / (rt.w - rt.z))
        }
        // The picture's point this display pixel shows (`warp`): the pixel's ray from the view
        // eye, found in the picture where the picture's own depth puts a point on that ray: a
        // fixed point from the rotation alone (the far point), each step the depth at the last
        // guess. The view eye and the render eye are millimetres apart (a frame of prediction),
        // so the parallax is a few pixels and two steps settle it; edges stay as sharp as the
        // picture's own pixels (no grid)
        warped: fn() -> vec3 {
            let o = self.ray_o
            let dr = self.ray
            let rt = self.render_tan
            let w = max(0.0 - dr.z, 0.0001)
            let uv0 = vec2((dr.x / w - rt.x) / (rt.y - rt.x), (dr.y / w - rt.z) / (rt.w - rt.z))
            let z0 = self.band_z(uv0)
            let a = self.ray_at(o, dr, z0)
            let z1 = self.band_z(a)
            // (inside a surface: the depth at the far guess and at the first step agree, the
            // second step would land where the first did: done)
            if abs(z1 - z0) <= 0.01 * z1 {
                return vec3(a.x, a.y, z1)
            }
            let b = self.ray_at(o, dr, z1)
            // b is the answer when its own depth puts it back on this ray (within a band
            // texel); else the ray passes an edge where the picture has two answers, a
            // foreground point it misses and the background the head now sees beside it: the
            // farther one (never the foreground smeared past its outline)
            // (with the picture's depth there: .z, m, 50 where nothing was drawn)
            let texel = vec2(2.0, 1.0) / self.band_size
            let z2 = self.band_z(b)
            let c = self.ray_at(o, dr, z2)
            if (abs(c.x - b.x) > texel.x || abs(c.y - b.y) > texel.y) && z0 > z1 {
                return vec3(a.x, a.y, z1)
            }
            return vec3(b.x, b.y, z2)
        }
        vertex: fn() {
            let proj = self.draw_pass.camera_projection
            let k = proj * vec4(0.0, 0.0, -1.0, 1.0)
            let a = (proj * vec4(1.0, 0.0, 0.0, 0.0)).x
            let c = (proj * vec4(0.0, 1.0, 0.0, 0.0)).y
            let x0 = (0.0 - k.w - k.x) / a
            let x1 = (k.w - k.x) / a
            let y0 = (0.0 - k.w - k.y) / c
            let y1 = (k.w - k.y) / c
            // the eye is told by its view's frustum: a left eye sees further left than right
            // (canted displays; the head's position was a frame and a space away from the
            // view's camera and picked one eye for both views)
            let left_view = step(x0 + x1, 0.0)
            let mine = 1.0 - abs(left_view - (1.0 - self.eye))
            let g = self.geom.pos
            let ex = mix(min(x0, x1), max(x0, x1), g.x)
            let ey = mix(max(y0, y1), min(y0, y1), g.y)
            self.pos = g
            // (the 2D clip in the fragment reads `world`: a point inside every clip)
            self.world = vec4(0.0, 0.0, 0.0, 1.0)
            let at = proj * vec4(ex, ey, -1.0, 1.0)
            // (the warp's ray at this corner)
            let rc = vec4(0.0 - self.render_rot.x, 0.0 - self.render_rot.y, 0.0 - self.render_rot.z, self.render_rot.w)
            let vt = self.view_tan
            let dv = vec3(mix(vt.x, vt.y, g.x), mix(vt.z, vt.w, g.y), -1.0)
            self.ray = self.qrot(rc, self.qrot(self.view_rot, dv))
            self.ray_o = self.qrot(rc, self.view_pos.xyz - self.render_pos.xyz)
            // not this draw's eye: nothing (every vertex at one point outside the view)
            self.vertex_pos = mix(vec4(2.0, 2.0, 2.0, 1.0), at, mine)
        }
        // The eye's column (`src`: its origin and size in the tile) holds its colour
        // (premultiplied on black) over two thirds of its height; below, a band whose left half
        // is its coverage at half resolution in the luma: the output is premultiplied colour
        // with that coverage, so the passthrough shows where the desktop drew nothing.
        pixel: fn() {
            // (debug: `env_map` as that many small white squares above the view's centre)
            if self.env_map > 0.5 && self.pos.y > 0.3 && self.pos.y < 0.32 && self.pos.x > 0.4 {
                let k = (self.pos.x - 0.4) / 0.03
                if k < self.env_map && fract(k) < 0.6 {
                    return vec4(1.0, 1.0, 1.0, 1.0)
                }
            }
            if self.occlude > 1.5 {
                // (debug: the room's depth along each pixel's ray as contour bands every 10 cm,
                // half see-through over the room so its edges can be matched; red where the
                // depth image has nothing)
                let r = self.room_depth(self.pos, 1.0)
                if r.y < 0.0 {
                    return vec4(0.3, 0.0, 0.0, 0.5)
                }
                let band = fract(r.y * 10.0)
                return vec4(band * 0.6, (1.0 - band) * 0.6, 0.0, 0.6)
            }
            if self.opaque > 1.5 && self.opaque < 3.5 {
                return vec4(0.6 * (1.0 - self.eye), 0.0, 0.6 * self.eye, 1.0)
            }
            let ch = floor(self.src.w * 2.0 / 3.0 + 0.5)
            // the picture's point here: this pixel's own (the frame shows with the picture's
            // eyes), or the warp's (with the view's eyes); outside the picture: the room
            var uv = self.pos
            // (the picture's depth there, m, once the warp found it; 0: not yet)
            var wz = 0.0
            if self.warp > 0.5 && self.video_mode > 0.5 {
                let w3 = self.warped()
                uv = w3.xy
                wz = w3.z
                if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
            }
            if self.opaque > 3.5 && self.opaque < 4.5 && self.video_mode > 0.5 {
                // (debug `opaque` 4: the colour alone, no coverage or depth samples: their cost)
                let c4 = self.video.sample_video((self.src.xy + uv * vec2(self.src.z, ch)) / self.nv12_size)
                return vec4(self.nv12_color(vec3(c4.y, c4.z, c4.x) * 255.0), 1.0)
            }
            if self.video_mode > 0.5 {
                // the coverage and depth from the band target (one plain fetch: two Y'CbCr
                // taps of the decoder's picture less per pixel); nothing drawn there: the room,
                // before any other work
                let va = self.band_coverage(uv)
                let ca = max(clamp(va, 0.0, 1.0), min(self.opaque, 1.0))
                if ca <= 0.002 {
                    return vec4(0.0, 0.0, 0.0, 0.0)
                }
                // the depth there (the warp's, else one fetch)
                var z = wz
                if z <= 0.0 {
                    z = self.band_z(uv)
                }
                let vs = self.nv12_size
                // (inside this eye's colour: the filter never reaches the other eye's column or
                // the band below)
                let cp = clamp(uv * vec2(self.src.z, ch), vec2(1.0, 1.0), vec2(self.src.z, ch) - vec2(1.0, 1.0))
                let c = self.video.sample_video((self.src.xy + cp) / vs)
                let rgb = self.nv12_color(vec3(c.y, c.z, c.x) * 255.0)
                let keep = 1.0 - self.occlusion(uv, mix(0.0, z, step(z, 49.0)))
                // (into the linear-frame targets: the colour as linear light, the hardware
                // encodes it; the picture is premultiplied on black, so straight first)
                if self.linear_out > 0.5 {
                    let straight = clamp(rgb / max(ca, 0.004), vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
                    // (the sRGB curve exactly: the hardware encodes with it)
                    let lin = mix(straight / 12.92, pow((straight + vec3(0.055, 0.055, 0.055)) / 1.055, vec3(2.4, 2.4, 2.4)), step(vec3(0.04045, 0.04045, 0.04045), straight))
                    return vec4(lin * ca * keep, ca * keep)
                }
                return vec4(rgb * keep, ca * keep)
            }
            let s = self.src.xy + self.pos * vec2(self.src.z, ch) - vec2(0.5, 0.5)
            let f = s - floor(s)
            let p = floor(s)
            let top = mix(self.nv12_rgb(p), self.nv12_rgb(p + vec2(1.0, 0.0)), f.x)
            let bottom = mix(self.nv12_rgb(p + vec2(0.0, 1.0)), self.nv12_rgb(p + vec2(1.0, 1.0)), f.x)
            let ap = vec2(self.src.x, ch) + self.pos * vec2(self.src.z * 0.5, ch * 0.5)
            let a = max(clamp(self.nv12_bytes(floor(ap)).x / 255.0, 0.0, 1.0), self.opaque)
            let dvb = self.nv12_bytes(floor(ap + vec2(self.src.z * 0.5, 0.0))).x / 255.0
            let keep = 1.0 - self.occlusion(self.pos, mix(0.0, self.depth_near / max(dvb, 0.0001), step(0.004, dvb)))
            return vec4(mix(top, bottom, f.y) * keep, a * keep)
        }
    }
}

/// The band-depth copy of one eye's picture (see the shader).
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawXrBandDepth {
    #[deref]
    pub draw_super: DrawQuad,
}

/// One eye of [`XrStreamClient`]'s picture in an XR pass (see the shader).
#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawXrStream {
    #[deref]
    pub draw_super: DrawNv12,
}

/// One tile of a picture, decoded, waiting for the UI thread.
#[cfg(not(target_arch = "wasm32"))]
struct Decoded {
    nv12: Vec<u8>,
    /// The picture on the GPU (the decoder's own buffer), when the decoder keeps it there.
    gpu: Option<makepad_widgets::makepad_platform::video_file::DecodedPicture>,
    size: [u32; 2],
    head: VideoHead,
    decode_us: u32,
    /// When its Video message came, and when it was decoded.
    arrived_at: Instant,
    decoded_at: Instant,
}

#[derive(Default)]
struct Shared {
    #[cfg(not(target_arch = "wasm32"))]
    link: Option<Arc<XrLink>>,
    /// The newest decoded picture per tile.
    #[cfg(not(target_arch = "wasm32"))]
    /// The recent decoded pictures per tile, oldest first (a few: one eye may run a picture
    /// ahead of the other).
    latest: [VecDeque<Decoded>; 2],
    /// The stream's tiles (1: both eyes in one picture; 2: a picture per eye) and a tile's size.
    tiles: u8,
    tile_size: [u32; 2],
    /// Video messages received, tiles decoded (the stats' rates).
    received: u32,
    decoded: u32,
    /// The seqs of the last pictures that arrived (eye 0's tile), and those the decode threads
    /// let go unshown at their queue cap (the last few: why a display frame repeated).
    arrived: VecDeque<u64>,
    cap_dropped: VecDeque<u64>,
    /// The display rate the link runs at (mHz): the app's display follows it.
    refresh_mhz: u32,
    /// The server's switches: the stream off (no decoders, no stream pass: the room alone), VR
    /// (the picture opaque, no passthrough).
    stream_off: bool,
    vr: bool,
    status: String,
    stop: bool,
    /// The uplink's last 5 s (rate, interval spread).
    uplink_line: String,
    /// The same as numbers: poses/s, the intervals' spread (ms).
    uplink: (f32, f32),
    /// The server's app uses this headset's room field (`XrServerMsg::RoomField`; off until
    /// it says so): only then is the depth integrated into the field and are bricks sent.
    room_field: bool,
    /// The poses' datagram path (`XrServerMsg::Uplink`): the socket, connected to the server,
    /// and the key that seals them.
    #[cfg(not(target_arch = "wasm32"))]
    udp: Option<Arc<(std::net::UdpSocket, Vec<u8>)>>,
    /// The PIN to show while no server is linked (pairing by PIN; None: linked, or linking to
    /// an address).
    pin: Option<String>,
    /// The server's settings for this headset's own mechanisms (`XrServerMsg::Headset`; None
    /// until it sent them).
    headset: Option<HeadsetSettings>,
    /// Controller buzzes from the server (`XrServerMsg::Haptic`), played on the UI thread.
    #[cfg(not(target_arch = "wasm32"))]
    haptics: Vec<makepad_widgets::makepad_platform::event::xr::XrHapticPulse>,
}

/// The stats of one window (5 s).
#[derive(Default)]
struct Window {
    display_frames: u32,
    shown: u32,
    unmatched: u32,
    decode_us: Vec<u32>,
    stages_us: [Vec<u32>; 4],
    /// The pose sent to its picture decoded (round trip), and its share on the wire (the trip
    /// less the desktop's stages and the decode).
    trip_us: Vec<u32>,
    net_us: Vec<u32>,
    /// Decoded to first shown.
    wait_us: Vec<u32>,
    /// Motion to photon: the pose's sample to the display time of the frame that first shows it.
    m2p_us: Vec<u32>,
    /// The environment depth image's age at use: the display time of the frame drawn with it
    /// less that of the frame that acquired it.
    env_age_us: Vec<u32>,
    /// Per picture: sample to decoded, decoded to the first take, that take to its display (us).
    ready_parts: Vec<[u32; 3]>,
    /// The largest difference between a picture's render eyes and the eyes sent for its pose
    /// (quaternion components).
    echo_err_deg: f32,
    /// Display frames without a new picture; pictures skipped to catch up (the queue above
    /// BUFFERED); pictures dropped unshown (one eye decoded without the other's pair).
    repeats: u32,
    skipped: u32,
    dropped: u32,
    /// The repeats by cause (`report::REPEAT_*`: late, server, dropped at the cap, no pose)
    /// and the blank frames (drawn without a picture for an eye).
    repeat_causes: [u32; 4],
    blanks: u32,
    /// When (ms into the window) a frame repeated ('r'), a picture was skipped ('s'), the
    /// display skipped a frame ('l'): to line up with what the wearer saw.
    events: Vec<(u32, char)>,
    /// Each display frame's code (`report::FRAME_*`), for the server's per-frame strip.
    codes: Vec<u8>,
}

fn median(v: &mut [u32]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    v[v.len() / 2] as f64 / 1000.0
}

/// A shown tile's texture(s) and the head it came with.
#[cfg(not(target_arch = "wasm32"))]
struct Shown {
    tex: ShownTex,
    size: [u32; 2],
    head: VideoHead,
}

#[cfg(not(target_arch = "wasm32"))]
enum ShownTex {
    /// CPU NV12 uploaded into one byte texture.
    Cpu(Texture),
    /// The decoder's picture, imported over its memory (a video texture). `_previous` are the
    /// two pictures shown before (frames still on the GPU may draw them: a frame is handed to
    /// the headset when it is queued, not when the GPU is done), let go as newer ones come.
    Gpu { texture: Texture, picture: makepad_widgets::makepad_platform::video_file::DecodedPicture, _previous: Vec<makepad_widgets::makepad_platform::video_file::DecodedPicture> },
}

/// A trace being recorded: until when, where it goes, its CSV rows.
struct Trace {
    until: Instant,
    path: String,
    rows: String,
    /// The clock when it began (the tracking log from then on goes with it).
    started_ns: u64,
}

/// A pose sent and not answered yet.
struct Sent {
    seq: u64,
    at: Instant,
    sample_ns: u64,
    /// The display time of the frame the pose was asked for (its poses are predicted the lead
    /// past it).
    display_ns: u64,
    /// The eyes' orientations sent (the picture must come back with exactly these).
    eyes: [[f32; 4]; 2],
    /// The whole pose sent (its `XrFrame`): the trace's hands.
    frame: Vec<u8>,
}

/// The headset end (see the module). Owned by the app's XR view.
pub struct XrStreamClient {
    shared: Arc<Mutex<Shared>>,
    /// The game audio's receive side (its player goes into the app's audio callback,
    /// [`XrStreamClient::audio_player`]).
    #[cfg(not(target_arch = "wasm32"))]
    audio: Arc<makepad_desk_link::xr_audio::XrAudioRx>,
    signal: SignalToUI,
    /// The poses sent (by the uplink thread), not answered yet.
    sent: Arc<Mutex<VecDeque<Sent>>>,
    #[cfg(not(target_arch = "wasm32"))]
    shown: [Option<Shown>; 2],
    /// A 2x2 NV12 stand-in for the byte slots while the picture is a video texture.
    blank: Option<Texture>,
    /// A stand-in for the environment depth before the headset has one.
    no_depth: Option<Texture>,
    /// The room field sent so far on `world_link` (its brick cursor), and the frame count that
    /// paces it.
    #[cfg(not(target_arch = "wasm32"))]
    world_link: Option<Arc<XrLink>>,
    world_cursor: u64,
    world_tick: u64,
    /// The picture made current by the last upload, until a display frame shows it: (seq,
    /// sample_ns, decoded_at, the display time its pose was predicted for).
    pending_show: Option<(u64, u64, Instant, u64)>,
    /// Per picture (the last 1024): its pose's sample to both its eyes decoded (ns), and the
    /// newest picture counted.
    ready: VecDeque<f64>,
    ready_seq: u64,
    /// The display frames past the newest one the poses are located for (`steer_ahead`), when
    /// it was last checked, and since when it could have been one less.
    ahead: u32,
    ahead_at: Instant,
    ahead_less_since: Option<Instant>,
    /// The least `ahead` (a debug switch, default 4: at 3, one or two pictures a second were
    /// still late, a repeat each; at 4 none and the user called it smooth; 0: steered alone).
    pub pin_ahead: u32,
    /// Stats windows (seconds) so far.
    windows: u64,
    /// The poses' frames ahead as a time (ns: `ahead` periods), for the stats.
    lead_ns: f64,
    /// The previous XR frame's display time (a gap of more than 1.5 periods: a display frame
    /// the app missed).
    last_display_ns: u64,
    /// A per-display-frame trace being recorded (`start_trace`).
    trace: Option<Trace>,
    /// The pose the shown picture was rendered for, as sent (its `XrFrame`; the trace's hands).
    shown_sent: Option<Vec<u8>>,
    /// The shown picture's pose sample time and how long it waited decoded (the trace).
    shown_meta: (u64, u64),
    /// The odd-frame watch (`watch_frame`): XR pass draws since the last XR frame, the last
    /// frames as compact records, the frames still to log after an odd one, the odd frames so
    /// far, the shown picture's last seq and how many frames it has shown.
    draws: u32,
    watch: VecDeque<String>,
    watch_after: u32,
    odd_frames: u32,
    /// This display frame's code so far (`report::FRAME_*`; the worst event wins).
    frame_code: u8,
    /// The power state applied: (stream off, VR, environment depth on); None: not yet.
    power: Option<(bool, bool, bool)>,
    /// The headset's battery as last read (`hal_battery`: level | charging << 8; -1 unknown)
    /// and when.
    battery: (i64, Option<Instant>),
    /// The last report sent while the stream is off (no XR frames then: `housekeeping`).
    idle_report_at: Instant,
    watch_seq: (u64, u32),
    watch_last_view: Option<[f32; 4]>,
    watch_turn: f32,
    window: Window,
    window_at: Instant,
    counts_at: (u32, u32),
    line: String,
    /// Real things in front hide the picture (the environment depth test); the app may turn it
    /// off (a debug switch).
    pub occlusion: bool,
    /// The whole picture covers the room, its coverage ignored; 2: flat magenta (debug switches).
    pub opaque: u8,
    /// The frame shows with the headset's own eyes, not the picture's (a debug switch).
    pub own_eyes: bool,
    /// The room's depth instead of the picture (a debug switch: the depth image's placement).
    pub room_view: bool,
    /// How the depth image is read (a debug switch: bit 0 mirrors x, 1 mirrors y, 2 swaps).
    pub env_map: u8,
    /// The frame goes to the compositor with the picture's depth (positional correction; a
    /// debug switch).
    pub depth_layer: bool,
    /// No new pictures: the last one stays (a debug switch: the reprojection check).
    pub freeze: bool,
    /// The picture is reprojected to the display eyes by depth in the stream pass (default; a
    /// debug switch).
    pub warp: bool,
    /// The band-depth target the warp's vertices read (made at the first warped draw).
    band_rt: Option<RenderTarget>,
    /// The warp probe's read back in flight.
    #[cfg(not(target_arch = "wasm32"))]
    probe_ticket: Option<ReadbackTicket>,
    /// The depth the layer carries instead of the picture's (m; 0: the picture's; a debug
    /// switch: whether the compositor uses it).
    pub focus_override_m: f32,
}

impl Drop for XrStreamClient {
    fn drop(&mut self) {
        self.shared.lock().unwrap().stop = true;
        #[cfg(not(target_arch = "wasm32"))]
        self.audio.stop();
    }
}

impl XrStreamClient {
    /// Starts linking to an XR server (`target`; on a worker thread: the UI never waits).
    pub fn connect(target: XrTarget, eye: [u32; 2], refresh_hz: f64) -> XrStreamClient {
        let shared = Arc::new(Mutex::new(Shared { status: "linking".into(), tiles: 1, refresh_mhz: (refresh_hz * 1000.0) as u32, ..Default::default() }));
        set_refresh_mhz((refresh_hz * 1000.0) as u32);
        let signal = SignalToUI::new();
        let sent = Arc::new(Mutex::new(VecDeque::new()));
        #[cfg(not(target_arch = "wasm32"))]
        let audio = makepad_desk_link::xr_audio::XrAudioRx::new();
        #[cfg(not(target_arch = "wasm32"))]
        {
            // the poses go up from a thread of their own, one per display frame as the headset
            // publishes it (not when the app finishes a frame: the server renders at the
            // display's rate and spacing whatever the app's frames do)
            let (shared, sent) = (shared.clone(), sent.clone());
            let _ = std::thread::Builder::new().name("xr-uplink".into()).spawn(move || uplink(shared, sent));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (shared, signal, audio) = (shared.clone(), signal.clone(), audio.clone());
            // the radio never naps while a stream runs (power save adds tens of ms per picture)
            makepad_widgets::makepad_platform::makepad_hal::hal_network_low_latency(true);
            // the stream's frames write linear colour: the projection chains become sRGB images
            // without the mutable format (framebuffer compression kept: 1.6 ms less per frame)
            makepad_widgets::makepad_platform::makepad_hal::hal_xr_linear_frames(true);
            let name = match &target {
                XrTarget::Pair { name, .. } => name.clone(),
                XrTarget::Address { .. } => "headset".to_string(),
            };
            let info = HeadsetInfo { name, instance: 0, eye, refresh_mhz: (refresh_hz * 1000.0) as u32, codecs: codecs::HEVC | codecs::H264 | codecs::HAPTICS };
            let _ = std::thread::Builder::new().name("xr-stream".into()).spawn(move || worker(target, info, shared, signal, audio));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (target, eye, refresh_hz);
        XrStreamClient {
            shared,
            #[cfg(not(target_arch = "wasm32"))]
            audio,
            signal,
            sent,
            #[cfg(not(target_arch = "wasm32"))]
            shown: [None, None],
            blank: None,
            no_depth: None,
            #[cfg(not(target_arch = "wasm32"))]
            world_link: None,
            world_cursor: 0,
            world_tick: 0,
            pending_show: None,
            ready: VecDeque::new(),
            ready_seq: 0,
            ahead: 0,
            ahead_at: Instant::now(),
            ahead_less_since: None,
            pin_ahead: 0,
            windows: 0,
            lead_ns: 0.0,
            last_display_ns: 0,
            trace: None,
            shown_sent: None,
            shown_meta: (0, 0),
            draws: 0,
            watch: VecDeque::new(),
            watch_after: 0,
            odd_frames: 0,
            frame_code: 0,
            power: None,
            battery: (-1, None),
            idle_report_at: Instant::now(),
            watch_seq: (0, 0),
            watch_last_view: None,
            watch_turn: 0.0,
            window: Window::default(),
            window_at: Instant::now(),
            counts_at: (0, 0),
            line: String::new(),
            occlusion: true,
            opaque: 0,
            own_eyes: false,
            room_view: false,
            env_map: 0,
            depth_layer: true,
            freeze: false,
            warp: false,
            band_rt: None,
            #[cfg(not(target_arch = "wasm32"))]
            probe_ticket: None,
            focus_override_m: 0.0,
        }
    }

    /// The game audio's player: move it into the app's audio output callback and call its
    /// `render_planar` there (it plays the stream, lined up with the pictures).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn audio_player(&self) -> makepad_desk_link::xr_audio::XrAudioPlayer {
        makepad_desk_link::xr_audio::XrAudioPlayer::new(self.audio.clone())
    }

    /// The server's settings for this headset's own mechanisms (None until it sent them).
    pub fn headset_settings(&self) -> Option<HeadsetSettings> {
        self.shared.lock().unwrap().headset
    }

    /// The PIN to show while no server is linked (None: linked, or not pairing by PIN).
    pub fn pin(&self) -> Option<String> {
        self.shared.lock().unwrap().pin.clone()
    }

    /// The link's state and the last stats line, for the app's own UI and logs.
    pub fn status(&self) -> String {
        let s = self.shared.lock().unwrap();
        if self.line.is_empty() { s.status.clone() } else { format!("{} · {}", s.status, self.line) }
    }

    /// The server turned the stream off (the app plays no sound then, the view shows the room).
    pub fn stream_off(&self) -> bool {
        self.shared.lock().unwrap().stream_off
    }

    /// Nothing to show: the stream is off or no server is linked (no XR frames are drawn).
    pub fn idle(&self) -> bool {
        let s = self.shared.lock().unwrap();
        #[cfg(not(target_arch = "wasm32"))]
        return s.stream_off || s.link.is_none();
        // (no link on the web)
        #[cfg(target_arch = "wasm32")]
        true
    }

    /// The display rate (Hz) the link runs at: the app asks its display for it.
    pub fn refresh_hz(&self) -> f32 {
        self.shared.lock().unwrap().refresh_mhz as f32 / 1000.0
    }

    /// Whether a picture can be drawn.
    pub fn has_picture(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.shown[0].is_some();
        #[cfg(target_arch = "wasm32")]
        false
    }

    /// Feed it the app's events: each XR frame's pose goes upstream; a decoded picture is
    /// uploaded (true: redraw the XR pass).
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
        #[cfg(target_arch = "wasm32")]
        let _ = &cx;
        match event {
            Event::XrUpdate(_) => {
                self.window.display_frames += 1;
                self.frame_code = 0;
                #[cfg(not(target_arch = "wasm32"))]
                if let Some((display_ns, _)) = cx.xr_frame_wire() {
                    let last = std::mem::replace(&mut self.last_display_ns, display_ns);
                    let period = period_ns() as f64;
                    if last != 0 && display_ns > last && (display_ns - last) as f64 > period * 1.5 {
                        self.window.events.push((self.window_at.elapsed().as_millis() as u32, 'l'));
                        self.mark_frame(makepad_desk_link::xr::report::FRAME_LATE);
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if let (Some((display_ns, _)), env_ns) = (cx.xr_frame_wire(), cx.xr_environment_depth_display_ns()) {
                        if env_ns != 0 && display_ns >= env_ns {
                            self.window.env_age_us.push(((display_ns - env_ns) / 1000) as u32);
                        }
                    }
                    self.housekeeping(cx);
                    // the server's controller buzzes, on this headset's controllers
                    let haptics = std::mem::take(&mut self.shared.lock().unwrap().haptics);
                    for h in &haptics {
                        cx.xr_haptic(h.hand as usize, h.amplitude, h.seconds);
                    }

                    // this display frame's picture: the oldest decoded one
                    let display_ns = cx.xr_frame_wire().map_or(0, |(t, _)| t);
                    let fresh = self.take_picture(cx, display_ns);
                    if !fresh && self.shown[0].is_some() {
                        self.window.repeats += 1;
                        let cause = self.repeat_cause(display_ns);
                        self.window.repeat_causes[cause] += 1;
                        self.window.events.push((self.window_at.elapsed().as_millis() as u32, ['r', 'v', 'c', 'p'][cause]));
                        self.mark_frame(makepad_desk_link::xr::report::FRAME_REPEAT);
                    }
                    // a frame with no stream content: no picture for an eye (the pass draws
                    // nothing for it: only the room shows)
                    let tiles = self.shared.lock().unwrap().tiles;
                    if self.shown[0].is_none() || (tiles >= 2 && self.shown[1].is_none()) {
                        if self.windows > 0 || self.window.display_frames > 90 {
                            self.window.blanks += 1;
                            self.mark_frame(makepad_desk_link::xr::report::FRAME_BLANK);
                        }
                    }
                    // the picture taken for this frame is on the glass at this frame's display time
                    if let (Some((_, sample_ns, decoded_at, _)), Some((display_ns, _))) = (self.pending_show.take(), cx.xr_frame_wire()) {
                        self.shown_meta = (sample_ns, decoded_at.elapsed().as_micros() as u64);
                        self.window.shown += 1;
                        self.window.wait_us.push(decoded_at.elapsed().as_micros() as u32);
                        if display_ns > sample_ns {
                            self.window.m2p_us.push(((display_ns - sample_ns) / 1000) as u32);
                        }
                    }
                    // The poses go up located `ahead` display frames past the headset's newest
                    // one: that leaves `ahead` periods from a pose's sample to its frame's take
                    // (the runtime's own pipeline depth is in the newest frame's time already, and
                    // it flips between ~2 and ~4 frames: a lead in ns from now followed each flip a
                    // frame or two late, with a burst of repeats or drops each time, the ~10 s
                    // oscillation on the stats panel). So `ahead` covers the sample to decoded alone
                    // (a steady ~20 ms): its p98 over the last ~11 s plus 2 ms, a frame more at once
                    // when that needs it, a frame less only after 10 s of not needing it.
                    // `pin_ahead` (debug) fixes it.
                    self.steer_ahead();
                    self.trace_frame(cx, fresh);
                    #[cfg(not(target_arch = "wasm32"))]
                    self.watch_frame(cx, fresh);
                    self.mark_frame(makepad_desk_link::xr::report::FRAME_NEW);
                    self.window.codes.push(self.frame_code);
                    if self.window_at.elapsed().as_secs_f64() >= 5.0 {
                        self.log_eyes(cx);
                    }
                    self.stats_window();
                }
                false
            }
            // (a decoded picture: the next XR frame takes it)
            Event::Signal => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.warp_probe_result(cx);
                    self.housekeeping(cx);
                }
                self.signal.check_and_clear()
            }
            _ => false,
        }
    }

    /// The room field's changed bricks go up in the link's gaps (a few times a second; all of
    /// them again on a new link, for a server that starts over).
    #[cfg(not(target_arch = "wasm32"))]
    fn send_world(&mut self, cx: &mut Cx, link: &Arc<XrLink>) {
        if !self.world_link.as_ref().is_some_and(|l| Arc::ptr_eq(l, link)) {
            self.world_link = Some(link.clone());
            self.world_cursor = 0;
        }
        self.world_tick += 1;
        if self.world_tick % 8 != 0 {
            return;
        }
        let (grid, bricks, next) = cx.xr_tsdf().brick_changes_since(self.world_cursor);
        let Some(grid) = grid else { return };
        self.world_cursor = next;
        let mut batch = Vec::new();
        let mut bytes = 0;
        let flush = |batch: &mut Vec<makepad_desk_link::xr::WorldBrick>| {
            if !batch.is_empty() {
                link.send_bulk(&XrClientMsg::World { voxel_size: grid.voxel_size, chunk_edge: grid.chunk_edge, bricks: std::mem::take(batch) });
            }
        };
        for b in bricks {
            bytes += 16 + b.payload.len();
            batch.push(makepad_desk_link::xr::WorldBrick { key: [b.key.x, b.key.y, b.key.z], version: b.version, payload: b.payload.as_ref().clone() });
            // small messages: a pose waits at most one of them
            if bytes >= WORLD_MESSAGE_BYTES {
                flush(&mut batch);
                bytes = 0;
            }
        }
        flush(&mut batch);
    }

    /// The power switches and the room field as the server says, and the room field's bricks
    /// up: from every XR frame, and while the stream is off (no XR frames then) from the app's
    /// timer ([`XrStreamClient::idle_tick`]) and the link's signals. Each part runs only while
    /// its switch needs it: the stream off releases the decoders and the shown pictures and
    /// pauses the stereo view (the room alone); the environment depth runs while the stream's
    /// occlusion or the room field needs it; the room field (the depth readback, its integration,
    /// the bricks) only while the server's app wants it and its user allows the scan.
    #[cfg(not(target_arch = "wasm32"))]
    fn housekeeping(&mut self, cx: &mut Cx) {
        use makepad_widgets::makepad_platform::makepad_hal as hal;
        // (no server linked: idle like the stream off, the PIN card alone)
        let (link, room_field, off, vr) = { let s = self.shared.lock().unwrap(); (s.link.clone(), s.room_field, s.stream_off || s.link.is_none(), s.vr) };
        if room_field != cx.xr_room_field_wanted() {
            cx.xr_set_room_field(room_field);
            // (on again: every brick anew, the server's mirror may have dropped them)
            self.world_cursor = 0;
        }
        if let (Some(link), true) = (&link, room_field) {
            self.send_world(cx, link);
        }
        let env = (!off && self.occlusion && !vr) || room_field;
        let want = (off, vr, env);
        if self.power != Some(want) {
            let before = self.power.replace(want);
            if before.map(|b| b.0) != Some(off) {
                log!("xr stream: stream {}", if off { "off: decoders released, the room alone" } else { "on" });
                hal::hal_xr_stereo_paused(off);
                if off {
                    self.shown = [None, None];
                    self.pending_show = None;
                }
            }
            if before.map(|b| b.1) != Some(vr) {
                log!("xr stream: {}", if vr { "VR: the picture opaque, no passthrough" } else { "mixed reality" });
                cx.xr_set_see_through(!vr);
            }
            if before.map(|b| b.2) != Some(env) {
                log!("xr stream: environment depth {}", if env { "on" } else { "off" });
                hal::hal_xr_environment_depth(env);
            }
        }
    }

    /// The app's timer while the stream may be off (a few times a second): the room field's
    /// bricks go up and the once-a-second report (the battery) goes on without XR frames.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn idle_tick(&mut self, cx: &mut Cx) {
        self.housekeeping(cx);
        if self.shared.lock().unwrap().stream_off && self.idle_report_at.elapsed() >= Duration::from_secs(1) {
            self.idle_report_at = Instant::now();
            self.stats_window();
        }
    }

    /// The headset's battery (`hal_battery`), read every 5 s.
    #[cfg(not(target_arch = "wasm32"))]
    fn battery(&mut self) -> i64 {
        if self.battery.1.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
            #[cfg(target_os = "android")]
            { self.battery.0 = makepad_widgets::makepad_platform::makepad_hal::hal_battery(); }
            self.battery.1 = Some(Instant::now());
        }
        self.battery.0
    }

    /// Once a second: the display frames past the newest one the poses go up for (see the
    /// caller); every change logged.
    fn steer_ahead(&mut self) {
        let first = self.ahead == 0;
        if !first && self.ahead_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.ahead_at = Instant::now();
        // The pictures' sample to decoded over the last ~3 s: the lead covers its p90 (+2 ms).
        // A rarer late picture shows a frame late (a repeat, then a skip); covering the p98 the
        // lead latched at 8 frames on any burst (a stall, a hitch) and pictures waited 60 ms in
        // the queue, at its cap, where the one due next was let go: a repeat every few frames.
        // Never more than the queue holds: the median picture waits at most HOLD - 2 frames.
        let measured = if self.ready.len() >= 64 {
            let mut v: Vec<f64> = self.ready.iter().rev().take(256).copied().collect();
            v.sort_by(|a, b| a.total_cmp(b));
            let (p50, p90) = (v[v.len() / 2], v[v.len() * 9 / 10]);
            let want = ((p90 + 2.0e6) / period_ns() as f64).ceil() as u32;
            let fits = (p50 / period_ns() as f64).floor() as u32 + HOLD as u32 - 2;
            want.min(fits).clamp(1, 8)
        } else {
            // (until measured: room for a ~30 ms sample to decoded)
            3
        };
        // (`pin_ahead` is the floor: more when the link needs it)
        let want = measured.max(self.pin_ahead);
        // up at once; down once it has not been needed for 3 s (one burst does not hold it)
        let next = if want > self.ahead {
            self.ahead_less_since = None;
            want
        } else if want < self.ahead {
            let since = *self.ahead_less_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_secs(3) { self.ahead_less_since = None; self.ahead - 1 } else { self.ahead }
        } else {
            self.ahead_less_since = None;
            self.ahead
        };
        if next != self.ahead {
            log!("xr stream: poses {} display frames ahead (was {}; measured {measured}, floor {})", next, self.ahead, self.pin_ahead);
            self.ahead = next;
            #[cfg(not(target_arch = "wasm32"))]
            makepad_widgets::makepad_platform::makepad_hal::hal_xr_pose_frames_ahead(next);
        }
        self.lead_ns = (self.ahead as u64 * period_ns()) as f64;
    }

    /// Uploads the newest complete picture (every tile of one pose), if a new one came.
    fn take_picture(&mut self, cx: &mut Cx, display_ns: u64) -> bool {
        // (debug `freeze`: the last picture keeps showing with its own eyes: the headset's
        // reprojection alone keeps it in place)
        #[cfg(not(target_arch = "wasm32"))]
        if self.freeze && self.shown[0].is_some() {
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let tiles = {
                let mut s = self.shared.lock().unwrap();
                let tiles = s.tiles.clamp(1, 2) as usize;
                // Each picture shows at the display frame it was rendered for (its pose's display
                // time): the newest complete one (both eyes decoded) whose time has come. A
                // picture early waits for its frame; one late shows at the next frame (the one
                // due then is skipped over). Shown off its time (the reference's oldest-first
                // queue of about two) every picture moved: the poses are predicted for their own
                // display time, and what moves in the picture (a held cube, the hands' effect)
                // jumped by the frames it was shown early or late.
                let complete: Vec<(u64, u64, Instant)> = s.latest[0].iter().map(|d| {
                    let other = s.latest[1].iter().find(|o| o.head.seq == d.head.seq).map_or(d.decoded_at, |o| o.decoded_at);
                    (d.head.seq, d.head.display_ns, d.decoded_at.max(other))
                }).filter(|(seq, _, _)| tiles == 1 || s.latest[1].iter().any(|d| d.head.seq == *seq)).collect();
                if complete.is_empty() {
                    return false;
                }
                // (each picture once: the earliest display frame it could show at, for the lead:
                // the frame of the first take after both its eyes were decoded, a display period
                // per take earlier than this one. Measured at the take that first sees it, a
                // picture held up behind the queue (the decoder waiting for a free image) counted
                // as late and raised the lead, which held more pictures: the lead only grew)
                let counted = self.ready_seq;
                for (seq, _, decoded_at) in complete.iter().filter(|(seq, _, _)| *seq > counted) {
                    if let Some(sent) = self.sent.lock().unwrap().iter().find(|p| p.seq == *seq) {
                        let takes_ago = (decoded_at.elapsed().as_nanos() as u64 / period_ns()).min(8);
                        let first = display_ns.saturating_sub(takes_ago * period_ns());
                        // (its parts, for the 5 s line: sample to decoded, decoded to the first take
                        // after it, that take to its frame's display)
                        let now_ns = makepad_widgets::makepad_platform::makepad_hal::clock_ns();
                        let decoded_ns = now_ns.saturating_sub(decoded_at.elapsed().as_nanos() as u64);
                        let take_ns = now_ns.saturating_sub(takes_ago * period_ns());
                        self.window.ready_parts.push([decoded_ns.saturating_sub(sent.sample_ns) as u32 / 1000, take_ns.saturating_sub(decoded_ns) as u32 / 1000, first.saturating_sub(take_ns) as u32 / 1000]);
                        self.ready.push_back(decoded_ns.saturating_sub(sent.sample_ns) as f64);
                        if self.ready.len() > 1024 {
                            self.ready.pop_front();
                        }
                    }
                    self.ready_seq = *seq;
                }
                let due = display_ns + period_ns() / 2;
                let seq = match complete.iter().rev().find(|(_, t, _)| *t <= due) {
                    Some((seq, _, _)) => *seq,
                    // none due yet: the shown one stays (the first picture shows at once)
                    None if self.shown[0].is_some() => return false,
                    None => complete[0].0,
                };
                let skipped = complete.iter().filter(|(q, _, _)| *q < seq).count() as u32;
                if skipped > 0 {
                    self.window.skipped += skipped;
                    self.window.events.push((self.window_at.elapsed().as_millis() as u32, 's'));
                    // (as `mark_frame`: the shared state is borrowed here)
                    use makepad_desk_link::xr::report as r;
                    if !matches!(self.frame_code, r::FRAME_ODD | r::FRAME_LATE | r::FRAME_BLANK) { self.frame_code = r::FRAME_SKIP; }
                }
                let mut out: Vec<Decoded> = Vec::with_capacity(tiles);
                for t in 0..tiles {
                    // older pictures go (their buffers back to the decoder); newer ones wait
                    while s.latest[t].front().is_some_and(|d| d.head.seq < seq) {
                        s.latest[t].pop_front();
                        if t == 0 { self.window.dropped += 1; }
                    }
                    out.push(s.latest[t].pop_front().unwrap());
                }
                out
            };
            let seq = tiles[0].head.seq;
            let mut sent_q = self.sent.lock().unwrap();
            let sent = sent_q.iter().position(|p| p.seq == seq).map(|i| {
                let (at, sample_ns, display_ns) = (sent_q[i].at, sent_q[i].sample_ns, sent_q[i].display_ns);
                self.shown_sent = Some(std::mem::take(&mut sent_q[i].frame));
                // the eyes the picture says it was rendered with against the ones sent for it
                let echo = &tiles[0].head.eyes;
                for e in 0..2 {
                    // (the largest component difference: exactly 0 when they are the same; an
                    // angle from the quaternions' dot product shows f32 rounding as ~0.1 degrees)
                    let (a, b) = (sent_q[i].eyes[e], echo[e].orientation);
                    let d = (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0f32, f32::max);
                    if d > self.window.echo_err_deg { self.window.echo_err_deg = d; }
                }
                sent_q.drain(..=i);
                (at, sample_ns, display_ns)
            });
            drop(sent_q);
            let w = &mut self.window;
            let mut decoded_at = tiles[0].decoded_at;
            for d in &tiles {
                w.decode_us.push(d.decode_us);
                decoded_at = decoded_at.max(d.decoded_at);
            }
            let head = tiles[0].head;
            for (i, v) in head.stages.iter().enumerate() {
                w.stages_us[i].push(*v);
            }
            match sent {
                Some((at, sample_ns, pose_display_ns)) => {
                    let trip = decoded_at.duration_since(at).as_micros() as u32;
                    w.trip_us.push(trip);
                    let desk: u32 = head.stages.iter().sum();
                    let decode = tiles.iter().map(|d| d.decoded_at.duration_since(d.arrived_at).as_micros() as u32).max().unwrap_or(0);
                    w.net_us.push(trip.saturating_sub(desk + decode));
                    self.pending_show = Some((seq, sample_ns, decoded_at, pose_display_ns));
                }
                None => w.unmatched += 1,
            }
            for (t, d) in tiles.into_iter().enumerate() {
                let Decoded { nv12, gpu, size, head, .. } = d;
                match (gpu, self.shown[t].take()) {
                    (Some(picture), old) => {
                        let texture = cx.import_picture(picture.handle);
                        let previous = match old.map(|o| o.tex) {
                            Some(ShownTex::Gpu { picture, _previous: mut older, .. }) => {
                                older.truncate(1);
                                older.insert(0, picture);
                                older
                            }
                            _ => Vec::new(),
                        };
                        self.shown[t] = Some(Shown { tex: ShownTex::Gpu { texture, picture, _previous: previous }, size, head });
                    }
                    (None, Some(Shown { tex: ShownTex::Cpu(texture), size: old_size, .. })) if old_size == size => {
                        let _ = texture.take_vec_u8(cx);
                        texture.put_back_vec_u8(cx, nv12, None);
                        self.shown[t] = Some(Shown { tex: ShownTex::Cpu(texture), size, head });
                    }
                    (None, _) => {
                        let texture = Texture::new_nv12(cx, size[0], size[1], nv12);
                        self.shown[t] = Some(Shown { tex: ShownTex::Cpu(texture), size, head });
                    }
                }
            }
            if self.shown[1].is_some() && self.shared.lock().unwrap().tiles < 2 {
                self.shown[1] = None;
            }
            true
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = cx;
            false
        }
    }

    /// Per eye: the shown picture's render pose and fov against this display frame's (the
    /// placement check).
    #[cfg(not(target_arch = "wasm32"))]
    fn log_eyes(&self, cx: &Cx) {
        let Some(shown) = self.shown[0].as_ref() else { return };
        let Some((_, wire)) = cx.xr_frame_wire() else { return };
        let Some(now) = makepad_widgets::makepad_platform::makepad_hal::buffers::read_items::<makepad_widgets::makepad_platform::makepad_hal::XrFrame>(&wire, 1).next() else { return };
        for eye in 0..2 {
            let r = &shown.head.eyes[eye];
            let d = &now.eyes[eye];
            log!("xr stream: eye {eye} render pos {:?} rot {:?} fov {:?} | display pos {:?} rot {:?} fov {:?} | focus {:.2} m", r.position, r.orientation, r.fov, d.pose.position, d.pose.orientation, d.fov, shown.head.focus_m);
            // where the picture's corners land in this eye's view (normalized device coordinates;
            // a still head: the view's corners, about ±1)
            if let Some(m) = cx.xr_eye_matrices() {
                let (view, proj) = m[eye];
                let ndc: Vec<String> = frustum_corners(r, shown.head.focus_m.max(0.1)).iter().map(|c| {
                    let p = proj.transform_vec4(view.transform_vec4(*c));
                    format!("({:.2},{:.2})", p.x / p.w, p.y / p.w)
                }).collect();
                log!("xr stream: eye {eye} picture corners tl tr bl br in the display eye: {}", ndc.join(" "));
            }
        }
    }

    /// The warp's desk check: reads the band-depth target back once and, at a few grid points of
    /// eye 0, logs the band value, the depth it gives, and where the warped vertex lands in the
    /// display eye (normalized device coordinates; in view: within ±1).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn warp_probe(&mut self, cx: &mut Cx) {
        let Some(rt) = self.band_rt.as_ref() else {
            log!("xr stream: warp probe: no band-depth target yet (warp off or no picture)");
            return;
        };
        match rt.texture.read_back(cx, ReadbackRequest { next_render: false }) {
            Ok(ticket) => self.probe_ticket = Some(ticket),
            Err(e) => log!("xr stream: warp probe: read back refused: {e:?}"),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn warp_probe_result(&mut self, cx: &mut Cx) {
        let Some(ticket) = self.probe_ticket else { return };
        let Some(r) = cx.try_take_texture_readbacks_for(&[ticket]).into_iter().next() else { return };
        self.probe_ticket = None;
        let Ok(data) = r.data else {
            log!("xr stream: warp probe: read back failed");
            return;
        };
        let (w, h) = (r.width, r.height);
        let px = |x: usize, y: usize| data.get((y * w + x) * 4 + 2).copied().unwrap_or(0) as f32 / 255.0;
        let (mut lo, mut hi, mut nonzero) = (1.0f32, 0.0f32, 0usize);
        for y in 0..h {
            for x in 0..w / 2 {
                let v = px(x, y);
                lo = lo.min(v);
                hi = hi.max(v);
                nonzero += (v >= 0.004) as usize;
            }
        }
        log!("xr stream: warp probe: band {w}x{h}, eye 0 values {lo:.3}..{hi:.3}, {nonzero} of {} with depth", (w / 2) * h);
        let (Some(shown), Some(m)) = (self.shown[0].as_ref(), cx.xr_eye_matrices()) else { return };
        let r = &shown.head.eyes[0];
        let [l, rr, u, d] = r.fov.map(f32::tan);
        let q = Quat { x: r.orientation[0], y: r.orientation[1], z: r.orientation[2], w: r.orientation[3] };
        let (view, proj) = m[0];
        for (gx, gy) in [(0.5f32, 0.5f32), (0.1, 0.1), (0.9, 0.1), (0.1, 0.9), (0.9, 0.9), (0.5, 0.7)] {
            let x = ((gx * (w / 2) as f32) as usize).min(w / 2 - 1);
            let y = ((gy * h as f32) as usize).min(h - 1);
            let vd = px(x, y);
            let z = if vd >= 0.004 { makepad_desk_link::xr::DEPTH_NEAR_M / vd.max(0.0001) } else { 50.0 };
            let dir = Vec3f { x: l + (rr - l) * gx, y: u + (d - u) * gy, z: -1.0 };
            let p = Vec3f { x: r.position[0], y: r.position[1], z: r.position[2] } + q.rotate_vec3(&(dir * z));
            let c = proj.transform_vec4(view.transform_vec4(Vec4f { x: p.x, y: p.y, z: p.z, w: 1.0 }));
            log!("xr stream: warp probe: grid ({gx:.1},{gy:.1}) band {vd:.3} depth {z:.2} m -> display ndc ({:.2}, {:.2}, z {:.3}, w {:.3})", c.x / c.w, c.y / c.w, c.z / c.w, c.w);
        }
    }

    /// Starts a per-display-frame trace of `secs` seconds, written to `path` as CSV when done
    /// (with the tracking log of the same seconds next to it: `track-*.csv` to read, `track-*.bin`
    /// to replay).
    pub fn start_trace(&mut self, secs: f64, path: String) {
        let mut rows = String::from("display_ns,head_qx,head_qy,head_qz,head_qw,head_x,head_y,head_z,new,seq,pic_display_ns,render_qx,render_qy,render_qz,render_qw,render_x,render_y,render_z,sample_ns,lead_ns,decoded_age_us,queue,warp");
        for p in ["view", "render1"] {
            for c in ["qx", "qy", "qz", "qw", "x", "y", "z"] {
                rows.push_str(&format!(",{p}_{c}"));
            }
        }
        for p in ["sent_lgrip", "sent_rgrip"] {
            for c in ["ok", "grab", "qx", "qy", "qz", "qw", "x", "y", "z"] {
                rows.push_str(&format!(",{p}_{c}"));
            }
        }
        rows.push('\n');
        self.trace = Some(Trace { until: Instant::now() + Duration::from_secs_f64(secs), path, rows, started_ns: clock_now_ns() });
    }

    /// One trace row: this display frame, its head and eye 0 (the view: located at its display
    /// time when the frame was asked), the picture it shows (new or repeated), that picture's
    /// render eyes and target time, the sample time of its pose, the lead, how long it waited
    /// decoded, the decoded pictures queued, whether it is warped, and the grips (each hand's
    /// grab pose: the palm's orientation at the thumb and index tips' middle) of the pose the
    /// picture was rendered for.
    #[cfg(not(target_arch = "wasm32"))]
    fn trace_frame(&mut self, cx: &Cx, fresh: bool) {
        use makepad_widgets::makepad_platform::makepad_hal::{buffers::read_items, XrFrame};
        let Some(t) = self.trace.as_mut() else { return };
        if Instant::now() >= t.until {
            let t = self.trace.take().unwrap();
            match std::fs::write(&t.path, t.rows.as_bytes()) {
                Ok(()) => log!("xr stream: trace written to {}", t.path),
                Err(e) => log!("xr stream: trace not written to {}: {e}", t.path),
            }
            write_track_log(&t.path, t.started_ns);
            return;
        }
        let Some((display_ns, wire)) = cx.xr_frame_wire() else { return };
        let Some(frame) = read_items::<XrFrame>(&wire, 1).next() else { return };
        let Some(shown) = self.shown[0].as_ref() else { return };
        let (h, r, r1, v) = (&frame.head, &shown.head.eyes[0], &shown.head.eyes[1], &frame.eyes[0].pose);
        let (sample_ns, age) = self.shown_meta;
        let queue = self.shared.lock().unwrap().latest[0].len();
        use std::fmt::Write;
        let _ = write!(t.rows, "{display_ns},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{sample_ns},{:.0},{age},{queue},{}",
            h.orientation[0], h.orientation[1], h.orientation[2], h.orientation[3], h.position[0], h.position[1], h.position[2],
            fresh as u8, shown.head.seq, shown.head.display_ns,
            r.orientation[0], r.orientation[1], r.orientation[2], r.orientation[3], r.position[0], r.position[1], r.position[2], self.lead_ns, self.warp as u8);
        for (q, p) in [(v.orientation, v.position), (r1.orientation, r1.position)] {
            let _ = write!(t.rows, ",{},{},{},{},{},{},{}", q[0], q[1], q[2], q[3], p[0], p[1], p[2]);
        }
        let sent = self.shown_sent.as_ref().and_then(|b| read_items::<XrFrame>(b, 1).next());
        for hand in 0..2 {
            push_grip(&mut t.rows, sent.as_ref().map(|f| &f.hands[hand]));
        }
        t.rows.push('\n');
    }

    /// Always on: each display frame as a compact record (display time ms, picture seq, its
    /// target against this frame in frames, new or repeated and for how many frames, decoded
    /// pictures queued, how long it waited decoded, XR pass draws since the last frame, the
    /// angle from its render eye to this frame's view eye, and the view eye's turn since the
    /// last frame). An odd frame (a picture off its frame, shown a third time, a draw count not
    /// 1, render and view eyes far apart, a turn step unlike its neighbours') logs the five
    /// frames before it and the five after: the intermittent one-frame jump, caught when it
    /// happens.
    #[cfg(not(target_arch = "wasm32"))]
    fn watch_frame(&mut self, cx: &Cx, fresh: bool) {
        use makepad_widgets::makepad_platform::makepad_hal::{buffers::read_items, XrFrame};
        let draws = std::mem::take(&mut self.draws);
        let Some((display_ns, wire)) = cx.xr_frame_wire() else { return };
        let Some(frame) = read_items::<XrFrame>(&wire, 1).next() else { return };
        let Some(shown) = self.shown[0].as_ref() else { return };
        let angle = |a: [f32; 4], b: [f32; 4]| (2.0 * (0..4).map(|k| a[k] * b[k]).sum::<f32>().abs().min(1.0).acos()).to_degrees();
        let seq = shown.head.seq;
        self.watch_seq = if self.watch_seq.0 == seq { (seq, self.watch_seq.1 + 1) } else { (seq, 1) };
        let off = (display_ns as i64 - shown.head.display_ns as i64) as f64 / period_ns() as f64;
        let render_view = angle(shown.head.eyes[0].orientation, frame.eyes[0].pose.orientation);
        let turn = self.watch_last_view.map_or(0.0, |q| angle(q, frame.eyes[0].pose.orientation));
        let turn_before = std::mem::replace(&mut self.watch_turn, turn);
        self.watch_last_view = Some(frame.eyes[0].pose.orientation);
        let queue = self.shared.lock().unwrap().latest[0].len();
        let rec = format!("{}:{} off{:+.1} {}{} q{} age{} d{} rv{:.2} turn{:.2}", (display_ns / 1_000_000) % 100_000, seq % 100_000, off, if fresh { 'n' } else { 'r' }, self.watch_seq.1, queue, self.shown_meta.1 / 1000, draws, render_view, turn);
        self.watch.push_back(rec);
        while self.watch.len() > 6 {
            self.watch.pop_front();
        }
        // (a repeat is off by its frames by definition: counted as a repeat with its cause, not
        // here; a new picture off its frame, not one XR draw, a jump are odd)
        let odd = (fresh && off.abs() > 0.5) || draws != 1 || render_view > 6.0 || (turn > 0.3 && turn > 2.5 * turn_before.max(0.05)) || (turn_before > 0.3 && turn < turn_before / 2.5);
        if odd {
            self.mark_frame(makepad_desk_link::xr::report::FRAME_ODD);
        }
        if odd && self.watch_after == 0 {
            self.odd_frames += 1;
            self.watch_after = 6;
            log!("xr stream: odd frame #{}: {} (before: {})", self.odd_frames, self.watch.back().unwrap(), self.watch.iter().rev().skip(1).rev().cloned().collect::<Vec<_>>().join(" | "));
        } else if self.watch_after > 0 {
            self.watch_after -= 1;
            if self.watch_after == 0 {
                log!("xr stream: odd frame #{} after: {}", self.odd_frames, self.watch.iter().cloned().collect::<Vec<_>>().join(" | "));
            }
        }
    }

    /// Why display frame `display_ns` repeats (`Window::repeat_causes`): 0 its picture is not
    /// here yet (in flight, decoding, one eye only), 1 the server never sent it (a newer one
    /// arrived), 2 this headset let it go at its queue cap, 3 no pose went up for this frame.
    #[cfg(not(target_arch = "wasm32"))]
    fn repeat_cause(&self, display_ns: u64) -> usize {
        let seq = {
            let sent = self.sent.lock().unwrap();
            match sent.iter().find(|p| p.display_ns.abs_diff(display_ns) < period_ns() / 2) {
                Some(p) => p.seq,
                None => return 3,
            }
        };
        let s = self.shared.lock().unwrap();
        if s.cap_dropped.contains(&seq) {
            return 2;
        }
        // (never arrived while a newer one did: the server skipped it)
        let newest = s.arrived.back().copied().unwrap_or(0);
        if newest > seq && !s.arrived.contains(&seq) { 1 } else { 0 }
    }

    #[cfg(target_arch = "wasm32")]
    fn repeat_cause(&self, _display_ns: u64) -> usize {
        0
    }

    /// This display frame's code becomes `code` when that is worse (odd, late, skip, repeat,
    /// new; none yet: anything).
    #[cfg(not(target_arch = "wasm32"))]
    fn mark_frame(&mut self, code: u8) {
        use makepad_desk_link::xr::report as r;
        let rank = |c: u8| match c { r::FRAME_BLANK => 6, r::FRAME_ODD => 5, r::FRAME_LATE => 4, r::FRAME_SKIP => 3, r::FRAME_REPEAT => 2, r::FRAME_NEW => 1, _ => 0 };
        if rank(code) > rank(self.frame_code) { self.frame_code = code; }
    }

    /// (no frame codes on the web: no headset reports them)
    #[cfg(target_arch = "wasm32")]
    fn mark_frame(&mut self, _code: u8) {}

    /// Every second: the window's numbers to the server (its stats panel); logged every 5th.
    fn stats_window(&mut self) {
        let secs = self.window_at.elapsed().as_secs_f64();
        if secs < 1.0 {
            return;
        }
        self.windows += 1;
        #[cfg(not(target_arch = "wasm32"))]
        let battery = self.battery();
        let (received, decoded) = {
            let s = self.shared.lock().unwrap();
            (s.received, s.decoded)
        };
        let (r0, d0) = self.counts_at;
        self.counts_at = (received, decoded);
        let w = &mut self.window;
        let rate = |n: u32| n as f64 / secs;
        let [ref mut render, ref mut gpu, ref mut encode, ref mut send] = w.stages_us;
        let env_p90 = {
            let v = &mut w.env_age_us;
            v.sort_unstable();
            v.get(v.len() * 9 / 10).map_or(0.0, |x| *x as f64 / 1000.0)
        };
        let line = format!(
            "odd frames {} · display {:.0}/s · recv {:.0}/s dec {:.0}/s shown {:.0}/s · m2p {:.0} ms = render {:.1} gpu {:.1} enc {:.1} send {:.1} net {:.1} dec {:.1} wait {:.1} (trip {:.0}) · {} unmatched · env depth age {:.0}/{:.0} ms · pose echo max diff {:e} · repeats {} (late {} server {} cap {} no pose {}) blank {} skipped {} dropped {}",
            self.odd_frames,
            rate(w.display_frames),
            rate(received.wrapping_sub(r0)),
            rate(decoded.wrapping_sub(d0)),
            rate(w.shown),
            median(&mut w.m2p_us),
            median(render),
            median(gpu),
            median(encode),
            median(send),
            median(&mut w.net_us),
            median(&mut w.decode_us),
            median(&mut w.wait_us),
            median(&mut w.trip_us),
            w.unmatched,
            median(&mut w.env_age_us),
            env_p90,
            w.echo_err_deg,
            w.repeats,
            w.repeat_causes[0],
            w.repeat_causes[1],
            w.repeat_causes[2],
            w.repeat_causes[3],
            w.blanks,
            w.skipped,
            w.dropped
        );
        #[cfg(not(target_arch = "wasm32"))]
        {
            use makepad_desk_link::xr::report;
            let (link, uplink) = {
                let s = self.shared.lock().unwrap();
                (s.link.clone(), s.uplink)
            };
            let mut v = vec![0.0f32; report::COUNT];
            v[report::DISPLAY] = rate(w.display_frames) as f32;
            v[report::SHOWN] = rate(w.shown) as f32;
            v[report::REPEATS] = rate(w.repeats) as f32;
            v[report::RECEIVED] = (rate(received.wrapping_sub(r0)) / self.shared.lock().unwrap().tiles.max(1) as f64) as f32;
            v[report::M2P_MS] = median(&mut w.m2p_us) as f32;
            v[report::NET_MS] = median(&mut w.net_us) as f32;
            v[report::DECODE_MS] = median(&mut w.decode_us) as f32;
            v[report::WAIT_MS] = median(&mut w.wait_us) as f32;
            v[report::ENV_AGE_MS] = median(&mut w.env_age_us) as f32;
            v[report::UPLINK] = uplink.0;
            v[report::UPLINK_SD_MS] = uplink.1;
            v[report::LEAD_MS] = (self.lead_ns / 1e6) as f32;
            v[report::SKIPPED] = rate(w.skipped) as f32;
            for (i, k) in [report::REPEAT_LATE, report::REPEAT_SERVER, report::REPEAT_DROPPED, report::REPEAT_NOPOSE].into_iter().enumerate() {
                v[k] = rate(w.repeat_causes[i]) as f32;
            }
            v[report::BLANK] = rate(w.blanks) as f32;
            v[report::AHEAD] = self.ahead as f32;
            v[report::BATTERY_PCT] = if battery < 0 { -1.0 } else { (battery & 0xff) as f32 };
            v[report::BATTERY_CHARGING] = if battery >= 0 && battery & 0x100 != 0 { 1.0 } else { 0.0 };
            // the headset's own GPU time and eye buffer size (the server decides on them)
            let eye = makepad_widgets::makepad_platform::makepad_hal::hal_xr_eye_scale_now();
            v[report::CLIENT_GPU_MS] = (eye & 0xffff_ffff) as f32 / 1000.0;
            v[report::EYE_SCALE_PCT] = (eye >> 32) as f32;
            // the sound waits as long as a picture's content takes to show (motion to photon
            // less the pose's way up)
            let m2p_ms = v[report::M2P_MS];
            self.audio.set_video_latency(if m2p_ms > 0.0 { ((m2p_ms - self.audio.uplink_ms()).max(0.0) * 1000.0) as u32 } else { 0 });
            self.audio.report(&mut v);
            report::pack_frames(&mut v, &w.codes);
            if let Some(link) = link {
                link.send(&XrClientMsg::Report { values: v });
            }
        }
        if self.windows % 5 != 0 {
            self.window = Window::default();
            self.window_at = Instant::now();
            return;
        }
        log!("xr stream: {line}");
        if !w.ready_parts.is_empty() {
            let q = |k: usize| {
                let mut v: Vec<u32> = w.ready_parts.iter().map(|p| p[k]).collect();
                v.sort_unstable();
                format!("{:.1}/{:.1}/{:.1}", v[v.len() / 10] as f64 / 1000.0, v[v.len() / 2] as f64 / 1000.0, v[v.len() * 9 / 10] as f64 / 1000.0)
            };
            log!("xr stream: ready p10/p50/p90 ms: sample to decoded {} · decoded to take {} · take to display {} · lead {:.1}", q(0), q(1), q(2), self.lead_ns / 1e6);
        }
        if !w.events.is_empty() {
            let ev: Vec<String> = w.events.iter().map(|(t, k)| format!("{k}{t}")).collect();
            log!("xr stream: events (ms into the window; r repeat, s skip, l display frame missed): {} · lead {:.1} ms", ev.join(" "), self.lead_ns / 1e6);
        }
        self.line = line;
        self.window = Window::default();
        self.window_at = Instant::now();
    }

    /// Draws the newest picture into the current (XR) pass, each eye its column over the whole
    /// view, and shows the frame with the eyes the picture was rendered with (the compositor
    /// reprojects them to the display's pose).
    /// Before the XR pass's own drawing (a pass of its own, painted before the XR pass that
    /// reads it): the band's depth of both eyes into the band-depth target the warp reads.
    pub fn draw_band(&mut self, cx: &mut Cx2d, band: &mut DrawXrBandDepth) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.shown[0].is_none() {
                return;
            }
            let tile_size = self.shared.lock().unwrap().tile_size;
            let tiled = self.shown[1].is_some();
            let ew = if tiled { tile_size[0] } else { tile_size[0] / 2 };
            let ch = (tile_size[1] as f32 * 2.0 / 3.0).round() as u32;
            // (one texel per band texel: the band is the colour's half resolution both ways, its
            // coverage left and its depth right; at half the band's rows the coverage was a
            // quarter of the colour's: a dark rim along horizontal edges)
            let band_size = ((ew / 2) * 2, (tile_size[1].saturating_sub(ch)).max(1));
            let rt = self.band_rt.get_or_insert_with(|| RenderTarget::new(cx.cx, "xr_stream_band_depth"));
            rt.ensure(cx.cx, band_size, RenderTargetFormat::Bgra8);
            rt.begin(cx, vec4(0.0, 0.0, 0.0, 1.0));
            for e in 0..2 {
                let Some(shown) = self.shown[e].as_ref().or(self.shown[0].as_ref()) else { break };
                let ShownTex::Gpu { texture, .. } = &shown.tex else { continue };
                band.draw_vars.set_texture(0, texture);
                let src = [if tiled { 0.0 } else { (e as u32 * ew) as f32 }, 0.0, ew as f32, tile_size[1] as f32];
                band.draw_vars.set_uniform(cx.cx, live_id!(src), &src);
                band.draw_vars.set_uniform(cx.cx, live_id!(nv12_size), &[shown.size[0] as f32, shown.size[1] as f32]);
                band.new_draw_call(cx);
                let half = band_size.0 as f64 / 2.0;
                band.draw_abs(cx, Rect { pos: dvec2(e as f64 * half, 0.0), size: dvec2(half, band_size.1 as f64) });
            }
            rt.end(cx);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (cx, band);
    }

    pub fn draw(&mut self, cx: &mut Cx2d, draw: &mut DrawXrStream) {
        self.draws += 1;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(first) = self.shown[0].as_ref() else { return };
            let tile_size = self.shared.lock().unwrap().tile_size;
            // this frame's eyes (the warp's target)
            let wire = cx.cx.xr_frame_wire();
            let view = wire.as_ref().and_then(|(_, w)| makepad_widgets::makepad_platform::makepad_hal::buffers::read_items::<makepad_widgets::makepad_platform::makepad_hal::XrFrame>(w, 1).next());
            // a picture more than three frames old is not warped (the warp is for the few
            // millimetres of one frame's prediction; stretched over a stall it smeared every
            // edge): it shows with its own eyes, the compositor turning it
            let stale = wire.as_ref().is_some_and(|(t, _)| *t > first.head.display_ns + 3 * period_ns());
            let warp = self.warp && !stale;
            let band_size = self.band_rt.as_ref().map_or((1, 1), |rt| rt.size());
            let eye = |e: &XrRenderEye| makepad_widgets::makepad_platform::makepad_hal::XrEye {
                pose: makepad_widgets::makepad_platform::makepad_hal::XrPose { orientation: e.orientation, position: e.position },
                fov: e.fov,
            };
            // (warped here, the frame shows with the view's own eyes: the compositor corrects
            // only the last few ms; else it shows with the picture's and the compositor
            // corrects the rotation since then)
            if !self.own_eyes && !warp {
                cx.cx.xr_present_eyes([eye(&first.head.eyes[0]), eye(&first.head.eyes[1])], if !self.depth_layer { 0.0 } else if self.focus_override_m > 0.0 { self.focus_override_m } else { first.head.focus_m });
            }
            // warped, the frame goes with the eyes it was warped to (the ones its uniforms
            // hold), never with whatever the HAL asked last: a frame drawn for one ask and taken
            // for the next showed a display frame of head rotation off
            if warp && !self.own_eyes {
                if let Some(v) = view.as_ref() {
                    cx.cx.xr_present_eyes(v.eyes, 0.0);
                }
            }
            // the tile's picture (the texture may be the decoder's larger buffer)
            for e in 0..2 {
                let Some(shown) = self.shown[e].as_ref().or(self.shown[0].as_ref()) else { return };
                let tiled = self.shown[1].is_some();
                let ew = if tiled { tile_size[0] } else { tile_size[0] / 2 };
                let color = makepad_widgets::makepad_platform::video_file::VideoColor::BT709;
                let video_mode;
                match &shown.tex {
                    ShownTex::Cpu(texture) => {
                        draw.set_nv12(texture, shown.size[0], shown.size[1], color, None);
                        video_mode = 0.0;
                    }
                    ShownTex::Gpu { texture, .. } => {
                        // the colour (the byte slots get a stand-in: the picture is a video
                        // texture only), the picture's buffer size for the source rect
                        let blank = self.blank.get_or_insert_with(|| Texture::new_nv12(cx.cx, 2, 2, vec![16, 16, 16, 16, 128, 128]));
                        draw.set_nv12(blank, shown.size[0], shown.size[1], color, None);
                        draw.nv12_size = vec2(shown.size[0] as f32, shown.size[1] as f32);
                        draw.draw_vars.set_texture(2, texture);
                        video_mode = 1.0;
                    }
                }
                draw.src = vec4(if tiled { 0.0 } else { (e as u32 * ew) as f32 }, 0.0, ew as f32, tile_size[1] as f32);
                // real things in front of the picture (hands): the headset's depth against the
                // picture's, per pixel
                let r = &shown.head.eyes[e];
                let tan = |f: [f32; 4]| f.map(f32::tan);
                let mut uniforms: Vec<(LiveId, Vec<f32>)> = vec![
                    (live_id!(eye), vec![e as f32]),
                    (live_id!(video_mode), vec![video_mode]),
                    (live_id!(render_rot), r.orientation.to_vec()),
                    (live_id!(render_pos), vec![r.position[0], r.position[1], r.position[2], 1.0]),
                    (live_id!(render_tan), tan(r.fov).to_vec()),
                    (live_id!(depth_near), vec![makepad_desk_link::xr::DEPTH_NEAR_M]),
                    // (VR: the picture covers the view, whatever its coverage)
                    (live_id!(opaque), vec![if self.shared.lock().unwrap().vr { (self.opaque as f32).max(1.0) } else { self.opaque as f32 }]),
                    (live_id!(env_map), vec![self.env_map as f32]),
                    (live_id!(warp), vec![warp as u8 as f32]),
                    (live_id!(linear_out), vec![makepad_widgets::makepad_platform::makepad_hal::hal_xr_linear_frames_active() as u8 as f32]),
                    (live_id!(band_size), vec![band_size.0 as f32, band_size.1 as f32]),
                ];
                // the view's own eye (this frame's: what the pass draws with)
                if let Some(v) = view.as_ref().map(|f| &f.eyes[e]) {
                    uniforms.extend([
                        (live_id!(view_rot), v.pose.orientation.to_vec()),
                        (live_id!(view_pos), vec![v.pose.position[0], v.pose.position[1], v.pose.position[2], 1.0]),
                        (live_id!(view_tan), tan(v.fov).to_vec()),
                    ]);
                }
                match cx.cx.xr_environment_depth().filter(|_| self.occlusion) {
                    Some((depth, header)) => {
                        // this eye's own depth camera and image layer
                        let v = &header.views[e];
                        let p = v.pose.position;
                        uniforms.extend([
                            (live_id!(env_pos), vec![p[0], p[1], p[2], header.near]),
                            (live_id!(env_size), vec![header.width as f32, header.height as f32]),
                            (live_id!(env_far), vec![if header.far.is_finite() { header.far.max(0.0) } else { 0.0 }]),
                            (live_id!(occlude), vec![if self.room_view { 2.0 } else { 1.0 }]),
                        ]);
                        let [ku, kv, kz] = env_rows(r, &v.pose.orientation, &p, &tan(v.fov), self.env_map);
                        uniforms.extend([(live_id!(env_ku), ku.to_vec()), (live_id!(env_kv), kv.to_vec()), (live_id!(env_kz), kz.to_vec())]);
                        draw.draw_vars.set_texture(3, &depth);
                    }
                    None => {
                        // (a stand-in: every texture slot is bound)
                        let none = self.no_depth.get_or_insert_with(|| Texture::new_with_format(cx.cx, TextureFormat::VecArrayMipBGRAu8_32 { width: 1, height: 1, layers: 2,
                            data: Some(vec![0xffff_ffff; 2]), max_level: Some(0), wrap: TextureWrap::ClampToEdge, updated: TextureUpdated::Full }));
                        draw.draw_vars.set_texture(3, none);
                        uniforms.push((live_id!(occlude), vec![0.0]));
                    }
                }
                // the band depth the warp reads (a stand-in while there is none)
                match self.band_rt.as_ref() {
                    Some(rt) => draw.draw_vars.set_texture(4, &rt.texture),
                    None => {
                        let blank = self.blank.get_or_insert_with(|| Texture::new_nv12(cx.cx, 2, 2, vec![16, 16, 16, 16, 128, 128]));
                        draw.draw_vars.set_texture(4, blank);
                    }
                }
                for (name, value) in &uniforms {
                    draw.draw_vars.set_uniform(cx.cx, *name, value);
                }
                // never the XR pass's own depth clip: it tests the quad's stand-in world point (the
                // tracking origin, on the floor) against the room's depth and discarded the whole
                // picture wherever the room was nearer along that ray (most head poses; the
                // picture showed only at odd head angles). Real things in front are this shader's
                // own per-pixel test (`occlusion`)
                draw.depth_clip = 0.0;
                draw.new_draw_call(cx);
                draw.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(1.0, 1.0) });
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (cx, draw);
    }
}

/// The depth camera's view of the render eye's picture as three rows over (u z, v z, z, 1)
/// (`u`, `v` the picture's point, `z` its depth along the render eye's axis): the depth image's
/// (u, v) times the distance from that camera, and the distance. The shader's per-pixel
/// rotate-into-the-room and rotate-into-the-camera folded into one 3x4 map (`env_map`: the debug
/// mirrors and swap).
#[cfg(not(target_arch = "wasm32"))]
fn env_rows(r: &XrRenderEye, env_rot: &[f32; 4], env_pos: &[f32; 3], et: &[f32; 4], env_map: u8) -> [[f32; 4]; 3] {
    // v rotated by the quaternion q, and by its conjugate
    fn rot(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
        let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
        let t = [2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0])];
        [v[0] + w * t[0] + (y * t[2] - z * t[1]), v[1] + w * t[1] + (z * t[0] - x * t[2]), v[2] + w * t[2] + (x * t[1] - y * t[0])]
    }
    let ec = [-env_rot[0], -env_rot[1], -env_rot[2], env_rot[3]];
    // the camera's view of a direction in the render eye's frame, and of the render eye itself
    let m = |d: [f32; 3]| rot(ec, rot(r.orientation, d));
    let t = rot(ec, [r.position[0] - env_pos[0], r.position[1] - env_pos[1], r.position[2] - env_pos[2]]);
    let [l, rr, up, dn] = r.fov.map(f32::tan);
    // the render eye's point (u z, v z, z): ((rr - l) u z + l z, (dn - up) v z + up z, -z)
    let (cu, cv, c0, c1, c2) = (m([rr - l, 0.0, 0.0]), m([0.0, dn - up, 0.0]), m([1.0, 0.0, 0.0]), m([0.0, 1.0, 0.0]), m([0.0, 0.0, 1.0]));
    // the camera's point as rows over (u z, v z, z, 1)
    let px = [cu[0], cv[0], c0[0] * l + c1[0] * up - c2[0], t[0]];
    let py = [cu[1], cv[1], c0[1] * l + c1[1] * up - c2[1], t[1]];
    let pz = [cu[2], cv[2], c0[2] * l + c1[2] * up - c2[2], t[2]];
    // distance ze = -pz; u ze = (px + et.l pz) / (et.r - et.l); v ze = (py + et.d pz) / (et.u - et.d)
    // (the depth image's first row is the bottom of its view)
    let kz = pz.map(|a| -a);
    let mut ku: [f32; 4] = std::array::from_fn(|i| (px[i] + et[0] * pz[i]) / (et[1] - et[0]));
    let mut kv: [f32; 4] = std::array::from_fn(|i| (py[i] + et[3] * pz[i]) / (et[2] - et[3]));
    if env_map & 1 != 0 {
        ku = std::array::from_fn(|i| kz[i] - ku[i]);
    }
    if env_map & 2 != 0 {
        kv = std::array::from_fn(|i| kz[i] - kv[i]);
    }
    if env_map & 4 != 0 {
        std::mem::swap(&mut ku, &mut kv);
    }
    [ku, kv, kz]
}

/// The corners (top left, top right, bottom left, bottom right) of `eye`'s frustum rectangle at
/// `depth` metres, in the tracking space.
#[cfg(not(target_arch = "wasm32"))]
fn frustum_corners(eye: &XrRenderEye, depth: f32) -> [Vec4f; 4] {
    let [qx, qy, qz, qw] = eye.orientation;
    let q = Quat { x: qx, y: qy, z: qz, w: qw };
    let p = Vec3f { x: eye.position[0], y: eye.position[1], z: eye.position[2] };
    let [l, r, u, d] = eye.fov.map(f32::tan);
    let at = |x: f32, y: f32| {
        let w = p + q.rotate_vec3(&Vec3f { x: x * depth, y: y * depth, z: -depth });
        vec4(w.x, w.y, w.z, 1.0)
    };
    [at(l, u), at(r, u), at(l, d), at(r, d)]
}

/// The uplink thread: each display frame's pose (`hal_xr_uplink_next`) up the link as it is
/// published; logs the send intervals' spread every 5 s.
#[cfg(not(target_arch = "wasm32"))]
fn uplink(shared: Arc<Mutex<Shared>>, sent: Arc<Mutex<VecDeque<Sent>>>) {
    use makepad_widgets::makepad_platform::makepad_hal::{self as hal, buffers::{read_items, Wire}, XrFrame};
    let mut buf = vec![0u8; <XrFrame as Wire>::SIZE];
    let mut seq = 0u64;
    let mut last: Option<Instant> = None;
    let (mut intervals, mut window) = (Vec::<u32>::new(), Instant::now());
    loop {
        let display_ns = hal::hal_xr_uplink_next(100, &mut buf);
        let (link, stop) = {
            let s = shared.lock().unwrap();
            (s.link.clone(), s.stop)
        };
        if stop {
            return;
        }
        if display_ns == 0 {
            continue;
        }
        let Some(link) = link else { continue };
        let now = Instant::now();
        if let Some(l) = last {
            intervals.push(now.duration_since(l).as_micros() as u32);
        }
        last = Some(now);
        seq += 1;
        let sample_ns = hal::clock_ns();
        let eyes = read_items::<XrFrame>(&buf, 1).next().map_or([[0.0; 4]; 2], |f| [f.eyes[0].pose.orientation, f.eyes[1].pose.orientation]);
        {
            let mut q = sent.lock().unwrap();
            q.push_back(Sent { seq, at: now, sample_ns, display_ns, eyes, frame: buf.clone() });
            while q.len() > 512 {
                q.pop_front();
            }
        }
        let udp = shared.lock().unwrap().udp.clone();
        match udp {
            Some(u) => {
                let _ = u.0.send(&makepad_desk_link::xr::pose_datagram::seal(&u.1, seq, sample_ns, display_ns, &buf));
            }
            None => link.send(&XrClientMsg::Pose { seq, sample_ns, display_ns, frame: buf.clone() }),
        }
        if window.elapsed() >= Duration::from_secs(5) {
            let n = intervals.len().max(1) as f64;
            let mean = intervals.iter().map(|v| *v as f64).sum::<f64>() / n;
            let dev = (intervals.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / n).sqrt();
            let max = intervals.iter().copied().max().unwrap_or(0);
            let line = format!("{:.0}/s, interval {:.2} ms ±{:.2} (max {:.1})", intervals.len() as f64 / window.elapsed().as_secs_f64(), mean / 1000.0, dev / 1000.0, max as f64 / 1000.0);
            log!("xr stream: uplink poses {line}");
            {
                let mut s = shared.lock().unwrap();
                s.uplink_line = line;
                s.uplink = ((intervals.len() as f64 / window.elapsed().as_secs_f64()) as f32, (dev / 1000.0) as f32);
            }
            intervals.clear();
            window = Instant::now();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn worker(target: XrTarget, mut info: HeadsetInfo, shared: Arc<Mutex<Shared>>, signal: SignalToUI, audio: Arc<makepad_desk_link::xr_audio::XrAudioRx>) {
    use makepad_desk_link::xr_pair::{self, HeadsetPairing};
    let set_status = |text: String| {
        log!("xr stream: {text}");
        shared.lock().unwrap().status = text;
        SignalToUI::set_ui_signal();
    };
    let mut pairing = match &target {
        XrTarget::Pair { pairing, .. } => Some(HeadsetPairing::load(pairing.clone())),
        XrTarget::Address { .. } => None,
    };
    if let Some(p) = &pairing {
        info.instance = p.instance;
    }
    // another server answered this headset's PIN while it was linked: it links there next
    let switch_to: Arc<Mutex<Option<xr_pair::Answer>>> = Arc::new(Mutex::new(None));
    loop {
        if shared.lock().unwrap().stop {
            return;
        }
        // the server: the address, or the one the probe pairs with (while none answers the
        // PIN shows)
        let (host, port, server) = match (&target, pairing.as_mut()) {
            (XrTarget::Address { host, port }, _) => (host.clone(), *port, 0),
            (_, Some(p)) => {
                shared.lock().unwrap().pin = Some(p.pin.clone());
                SignalToUI::set_ui_signal();
                let switched = switch_to.lock().unwrap().take();
                let answer = if let Some(a) = switched {
                    p.pair(a.instance, &a.app);
                    Some(a)
                } else {
                    match xr_pair::identity("pairing with a computer") {
                    Ok((me, fleet)) => match xr_pair::sweep(me, &fleet, p, &info.name, Duration::from_millis(1500)) {
                        Ok(answers) => match p.choose(&answers) {
                            Ok(a) => a,
                            Err(()) => {
                                p.reroll();
                                log!("xr stream: another headset showed the same PIN: new PIN {}", p.pin);
                                None
                            }
                        },
                        Err(e) => {
                            set_status(format!("PIN {} · {e}", p.pin));
                            None
                        }
                    },
                    Err(e) => {
                        set_status(format!("PIN {} · {e}", p.pin));
                        std::thread::sleep(Duration::from_secs(3));
                        None
                    }
                    }
                };
                let Some(a) = answer else {
                    let status = format!("PIN {} · type it in the XR menu of the app on the computer", p.pin);
                    if shared.lock().unwrap().status != status {
                        set_status(status);
                    }
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                };
                set_status(format!("linking to {} on {}", a.app, a.tcp));
                (a.tcp.ip().to_string(), a.tcp.port(), a.instance)
            }
            _ => return,
        };
        let link = match XrLink::connect(&host, port, info.clone(), Arc::new(|| {})) {
            Ok(link) => Arc::new(link),
            Err(e) => {
                set_status(format!("{e}; retrying"));
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        // while linked the probe goes on now and then: the PIN typed on another computer (or in
        // another app) moves the headset there
        let linked = Arc::new(std::sync::atomic::AtomicBool::new(true));
        if let Some(p) = pairing.clone() {
            let (linked, switch_to, name) = (linked.clone(), switch_to.clone(), info.name.clone());
            let _ = std::thread::Builder::new().name("xr-pair-watch".into()).spawn(move || {
                let Ok((me, fleet)) = xr_pair::identity("pairing with a computer") else { return };
                while linked.load(std::sync::atomic::Ordering::Acquire) {
                    std::thread::sleep(Duration::from_secs(3));
                    let answers = xr_pair::sweep(me, &fleet, &p, &name, Duration::from_millis(1000)).unwrap_or_default();
                    if let Some(a) = answers.into_iter().find(|a| a.kind == xr_pair::AnswerKind::Pin && a.instance != server) {
                        *switch_to.lock().unwrap() = Some(a);
                        return;
                    }
                }
            });
        }
        let _linked = Defer(move || linked.store(false, std::sync::atomic::Ordering::Release));
        // a decoder per tile, each on its own thread (the eyes decode in parallel)
        let mut tiles: Vec<std::sync::mpsc::Sender<(VideoHead, Vec<u8>, Instant)>> = Vec::new();
        // the welcome's stream (codec, tile size, tiles): the decoders made again when the
        // stream comes back on
        let mut stream_shape: Option<(StreamVideoCodec, [u32; 2], u8)> = None;
        let mut pinged = Instant::now();
        let why = loop {
            if shared.lock().unwrap().stop {
                return;
            }
            if pinged.elapsed() > Duration::from_secs(1) {
                // (the audio's clock offset is taken from the answers)
                link.send(&XrClientMsg::Ping { client_ns: makepad_widgets::makepad_platform::makepad_hal::clock_ns() });
                pinged = Instant::now();
            }
            if let Some(why) = link.closed() {
                break why;
            }
            if let Some(a) = switch_to.lock().unwrap().as_ref() {
                break format!("the PIN was typed in {} on {}: moving there", a.app, a.tcp);
            }
            let Some(msg) = link.recv_timeout(Duration::from_millis(5)) else { continue };
            match msg {
                XrServerMsg::Welcome { server, instance, session, size, codec, tiles: n, encoder, .. } => {
                    shared.lock().unwrap().pin = None;
                    log!("xr stream: linked to server instance {instance:016x}, session {session:016x}");
                    let codec = if codec == CODEC_HEVC { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
                    let n = n.clamp(1, 2);
                    let tile_size = [size[0] / n as u32, size[1]];
                    stream_shape = Some((codec, tile_size, n));
                    // the old tiles' threads end with their channels
                    tiles.clear();
                    {
                        let mut s = shared.lock().unwrap();
                        s.link = Some(link.clone());
                        s.tiles = n;
                        s.tile_size = tile_size;
                        s.latest = [VecDeque::new(), VecDeque::new()];
                    }
                    for tile in 0..n as usize {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let (link, shared, signal) = (link.clone(), shared.clone(), signal.clone());
                        let _ = std::thread::Builder::new().name(format!("xr-decode-{tile}")).spawn(move || decode_tile(tile, codec, tile_size, rx, link, shared, signal));
                        tiles.push(tx);
                    }
                    set_status(format!("streaming from {server} ({}x{} {codec:?} in {n} tile(s), {encoder})", size[0], size[1]));
                }
                XrServerMsg::Video { seq, tile, key, display_ns, eyes, focus_m, stages, size, codec, data } => {
                    let Some(tx) = tiles.get(tile as usize) else { continue };
                    {
                        let mut s = shared.lock().unwrap();
                        s.received += 1;
                        if tile == 0 {
                            s.arrived.push_back(seq);
                            if s.arrived.len() > 64 { s.arrived.pop_front(); }
                        }
                    }
                    let head = VideoHead { seq, tile, key, display_ns, eyes, focus_m, stages, size, codec };
                    let _ = tx.send((head, data, Instant::now()));
                }
                XrServerMsg::Note { text } => log!("xr stream: server: {text}"),
                XrServerMsg::RoomField { wanted } => {
                    log!("xr stream: the server {} the room field", if wanted { "wants" } else { "does not want" });
                    shared.lock().unwrap().room_field = wanted;
                }
                XrServerMsg::Uplink { port, key } => {
                    let socket = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect((host.as_str(), port)).map(|_| s));
                    match socket {
                        Ok(socket) => {
                            log!("xr stream: poses go as datagrams to port {port}, the game audio comes back on it");
                            if let Err(e) = audio.start(&socket, key.clone()) {
                                log!("xr stream: no game audio: {e}");
                            }
                            shared.lock().unwrap().udp = Some(Arc::new((socket, key)));
                        }
                        Err(e) => log!("xr stream: no pose datagrams ({e}): poses go on the link"),
                    }
                }
                XrServerMsg::Refused { reason } | XrServerMsg::Bye { reason } => break reason,
                XrServerMsg::Pong { client_ns, server_ns } => audio.pong(client_ns, server_ns),
                // another display rate: the display switches (the app, `refresh_hz`), the
                // server ends the session and the next link is made at it
                // the stream off: the decoders go (their threads end with their channels: the
                // codecs released); on again: made anew (the server starts with a key picture)
                XrServerMsg::Streaming { on } => {
                    {
                        let mut s = shared.lock().unwrap();
                        s.stream_off = !on;
                        s.latest = [VecDeque::new(), VecDeque::new()];
                    }
                    tiles.clear();
                    if let (true, Some((codec, tile_size, n))) = (on, stream_shape) {
                        for tile in 0..n as usize {
                            let (tx, rx) = std::sync::mpsc::channel();
                            let (link, shared, signal) = (link.clone(), shared.clone(), signal.clone());
                            let _ = std::thread::Builder::new().name(format!("xr-decode-{tile}")).spawn(move || decode_tile(tile, codec, tile_size, rx, link, shared, signal));
                            tiles.push(tx);
                        }
                    }
                    log!("xr stream: the server turned the stream {}", if on { "on" } else { "off" });
                    signal.set();
                }
                XrServerMsg::SeeThrough { on } => {
                    shared.lock().unwrap().vr = !on;
                    signal.set();
                }
                XrServerMsg::Haptic { hand, amplitude, duration_us } => {
                    let mut s = shared.lock().unwrap();
                    if s.haptics.len() < 16 {
                        s.haptics.push(makepad_widgets::makepad_platform::event::xr::XrHapticPulse { hand, amplitude, seconds: duration_us as f32 * 1e-6 });
                    }
                }
                XrServerMsg::Headset { foveation, eye_min_pct, eye_max_pct, multimodal, occlusion, ahead } => {
                    shared.lock().unwrap().headset = Some(HeadsetSettings { foveation, eye_min_pct, eye_max_pct, multimodal, occlusion, ahead });
                    SignalToUI::set_ui_signal();
                }
                // the desktop's PIN changed or cleared: the pair is over (forgotten here too)
                XrServerMsg::Unpaired => {
                    if let Some(p) = pairing.as_mut() {
                        p.forget(server);
                    }
                    break "the computer unpaired this headset".to_string();
                }
                XrServerMsg::Refresh { mhz } => {
                    log!("xr stream: the server asks for {:.0} Hz", mhz as f64 / 1000.0);
                    info.refresh_mhz = mhz;
                    set_refresh_mhz(mhz);
                    shared.lock().unwrap().refresh_mhz = mhz;
                    SignalToUI::set_ui_signal();
                }
            }
        };
        {
            let mut s = shared.lock().unwrap();
            s.link = None;
            s.udp = None;
        }
        set_status(format!("link ended: {why}; reconnecting"));
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Runs its closure when dropped.
#[cfg(not(target_arch = "wasm32"))]
struct Defer<F: FnMut()>(F);
#[cfg(not(target_arch = "wasm32"))]
impl<F: FnMut()> Drop for Defer<F> {
    fn drop(&mut self) {
        (self.0)()
    }
}

/// One tile's decoder on its own thread: each access unit in, its picture out to the UI (with
/// the head of the access unit it came from: a decoder may hand a picture out a unit later).
#[cfg(not(target_arch = "wasm32"))]
fn decode_tile(tile: usize, codec: StreamVideoCodec, size: [u32; 2], rx: std::sync::mpsc::Receiver<(VideoHead, Vec<u8>, Instant)>, link: Arc<XrLink>, shared: Arc<Mutex<Shared>>, signal: SignalToUI) {
    // the decoder's pictures stay on the GPU (sampled where they are: no copy, no upload)
    let mut decoder = match VideoStreamDecoder::with_size_hint_gpu(codec, size) {
        Ok(d) => d,
        Err(e) => {
            log!("xr stream: tile {tile}: no decoder: {e:?}");
            return;
        }
    };
    log!("xr stream: tile {tile} decodes on {}", decoder.name().unwrap_or_default());
    // the heads of the units in the decoder, by sequence number
    let mut heads: VecDeque<(VideoHead, Instant)> = VecDeque::new();
    while let Ok((head, data, arrived_at)) = rx.recv() {
        let seq = head.seq;
        heads.push_back((head, arrived_at));
        while heads.len() > 8 {
            heads.pop_front();
        }
        match decoder.push_packet(&data, seq as i64 * 10) {
            Ok(frames) => {
                let decode_us = arrived_at.elapsed().as_micros() as u32;
                // one tile decoded: the server may send the next (its flow control)
                link.send(&XrClientMsg::Stats { seq, decode_us, m2p_us: 0 });
                if let Some(f) = frames.into_iter().last() {
                    let fseq = (f.pts_100ns / 10) as u64;
                    let Some(at) = heads.iter().position(|(h, _)| h.seq == fseq) else { continue };
                    let (head, arrived_at) = heads[at];
                    heads.drain(..=at);
                    let mut s = shared.lock().unwrap();
                    s.decoded += 1;
                    let held = &mut s.latest[tile & 1];
                    // six at most (pictures wait up to the lead's slack and spread for their frame;
                    // at two or three the one due next was dropped: a repeat and a skip): the
                    // oldest goes. With the shown one and the two before it that is nine of the
                    // reader's eleven images: the decoder never waits for one (with all of them
                    // held it did, each picture then decoded late, and the lead measured from
                    // that grew without end)
                    let mut let_go = Vec::new();
                    while held.len() >= HOLD {
                        if let Some(d) = held.pop_front() { let_go.push(d.head.seq); }
                    }
                    for seq in let_go {
                        s.cap_dropped.push_back(seq);
                        if s.cap_dropped.len() > 64 { s.cap_dropped.pop_front(); }
                    }
                    let held = &mut s.latest[tile & 1];
                    held.push_back(Decoded { nv12: f.nv12, gpu: f.gpu, size: [f.width, f.height], head, decode_us, arrived_at, decoded_at: Instant::now() });
                    drop(s);
                    signal.set();
                }
            }
            Err(e) => {
                log!("xr stream: tile {tile}: decode: {e:?}");
                link.send(&XrClientMsg::Stats { seq, decode_us: 0, m2p_us: 0 });
                link.send(&XrClientMsg::Keyframe);
            }
        }
    }
}

/// A hand's grab pose (the physics' grip: the palm's orientation at the middle of the thumb and
/// index tips, else the index tip) and whether it grabs.
#[cfg(not(target_arch = "wasm32"))]
fn grip_of(h: &makepad_widgets::makepad_platform::makepad_hal::XrHand) -> Option<(bool, Quat, Vec3f)> {
    use makepad_widgets::makepad_platform::event::xr::XrHand;
    let mut c = XrHand { flags: h.flags as u8, tips: h.tips, tips_active: h.tips_active as u8, pinch: h.pinch, ..Default::default() };
    for (o, j) in c.joints.iter_mut().zip(h.joints.iter()) {
        *o = Pose::new(Quat { x: j.orientation[0], y: j.orientation[1], z: j.orientation[2], w: j.orientation[3] }, vec3f(j.position[0], j.position[1], j.position[2]));
    }
    let palm = c.tracking_pose()?;
    let position = match (c.tip_pos_checked(XrHand::THUMB_TIP), c.tip_pos_checked(XrHand::INDEX_TIP)) {
        (Some(t), Some(i)) => (t + i) * 0.5,
        (_, Some(i)) => i,
        _ => return None,
    };
    Some((c.grabbing(), palm.orientation, position))
}

/// A grip's trace columns: ok, grab, orientation, position (empty when the hand is not tracked).
#[cfg(not(target_arch = "wasm32"))]
fn push_grip(rows: &mut String, h: Option<&makepad_widgets::makepad_platform::makepad_hal::XrHand>) {
    use std::fmt::Write;
    match h.and_then(grip_of) {
        Some((grab, q, p)) => { let _ = write!(rows, ",1,{},{},{},{},{},{},{},{}", grab as u8, q.x, q.y, q.z, q.w, p.x, p.y, p.z); }
        None => rows.push_str(",0,0,0,0,0,1,0,0,0"),
    }
}

/// The tracking log since `since_ns` next to the trace at `trace_path`: `track-*.csv` (kind 0
/// sent: sample time, the display time located for; kind 1 tracked: a display time, when it was
/// located; then the head, eye 0, eye 1 and both grips) and `track-*.bin` (the entries
/// themselves: `debug.xrremote.replay` plays them).
#[cfg(not(target_arch = "wasm32"))]
fn write_track_log(trace_path: &str, since_ns: u64) {
    use makepad_widgets::makepad_platform::makepad_hal::{self as hal, buffers::read_items, XrFrame};
    use std::fmt::Write;
    let size = hal::hal_xr_track_entry_size();
    if size == 0 {
        return;
    }
    let mut buf = vec![0u8; 8 + size * 512];
    let (mut cursor, mut bin, mut csv) = (0u64, Vec::new(), String::from("kind,t_ns,aux_ns"));
    for p in ["head", "eye0", "eye1"] {
        for c in ["qx", "qy", "qz", "qw", "x", "y", "z"] {
            csv.push_str(&format!(",{p}_{c}"));
        }
    }
    for p in ["lgrip", "rgrip"] {
        for c in ["ok", "grab", "qx", "qy", "qz", "qw", "x", "y", "z"] {
            csv.push_str(&format!(",{p}_{c}"));
        }
    }
    csv.push('\n');
    loop {
        let next = hal::hal_xr_track_log(cursor, &mut buf);
        let n = u64::from_le_bytes(buf[..8].try_into().unwrap()) as usize;
        if n == 0 {
            break;
        }
        cursor = next;
        for e in buf[8..8 + n * size].chunks_exact(size) {
            let u = |k: usize| u64::from_le_bytes(e[k..k + 8].try_into().unwrap());
            // (the sent poses from the trace's start, the tracked path from a little before it)
            let (kind, t, aux) = (u(0), u(8), u(16));
            let at = if kind == 0 { t } else { t + 200_000_000 };
            if at < since_ns {
                continue;
            }
            bin.extend_from_slice(e);
            let Some(f) = read_items::<XrFrame>(&e[24..], 1).next() else { continue };
            let _ = write!(csv, "{kind},{t},{aux}");
            if kind == 2 {
                // (the hands' grip action space: head = the right grip's angular velocity (x, y,
                // z, 0) and linear velocity at the sample time; eye 0 / eye 1 = the right / left
                // grip at the sample time; the grips = at the target, ok = position valid)
                let (av, lv) = (f.hands[1].joints[1].position, f.hands[1].joints[0].position);
                let _ = write!(csv, ",{},{},{},0,{},{},{}", av[0], av[1], av[2], lv[0], lv[1], lv[2]);
                for p in [&f.controllers[1].aim_pose, &f.controllers[0].aim_pose] {
                    let _ = write!(csv, ",{},{},{},{},{},{},{}", p.orientation[0], p.orientation[1], p.orientation[2], p.orientation[3], p.position[0], p.position[1], p.position[2]);
                }
                for h in 0..2 {
                    let p = &f.controllers[h].grip_pose;
                    let _ = write!(csv, ",{},{},{},{},{},{},{},{},{}", (f.hands[h].tips_active & 2 != 0) as u8, f.hands[h].flags, p.orientation[0], p.orientation[1], p.orientation[2], p.orientation[3], p.position[0], p.position[1], p.position[2]);
                }
                // (then the right palm joint at the sample time, at the target + 40 ms and at the
                // newest display frame's time)
                for j in 2..5 {
                    let a = f.hands[1].joints[j].position;
                    let _ = write!(csv, ",{},{},{}", a[0], a[1], a[2]);
                }
                csv.push('\n');
                continue;
            }
            for p in [&f.head, &f.eyes[0].pose, &f.eyes[1].pose] {
                let _ = write!(csv, ",{},{},{},{},{},{},{}", p.orientation[0], p.orientation[1], p.orientation[2], p.orientation[3], p.position[0], p.position[1], p.position[2]);
            }
            for hand in 0..2 {
                push_grip(&mut csv, Some(&f.hands[hand]));
            }
            csv.push('\n');
        }
    }
    let base = trace_path.replace("trace-", "track-");
    let base = base.strip_suffix(".csv").unwrap_or(&base).to_string();
    let entries = bin.len() / size;
    match std::fs::write(format!("{base}.csv"), csv.as_bytes()).and_then(|_| std::fs::write(format!("{base}.bin"), &bin)) {
        Ok(()) => log!("xr stream: tracking log ({entries} entries) written to {base}.csv and .bin"),
        Err(e) => log!("xr stream: tracking log not written to {base}: {e}"),
    }
}

/// Plays the tracking log `name` (`track-<n>` in the app's files: a trace's) instead of the
/// headset's own poses; "" or "0" stops. Whether a replay runs.
#[cfg(not(target_arch = "wasm32"))]
pub fn replay_track_log(dir: &str, name: &str) -> bool {
    use makepad_widgets::makepad_platform::makepad_hal as hal;
    if name.is_empty() || name == "0" {
        hal::hal_xr_replay(&[]);
        log!("xr stream: replay off: the headset's own poses");
        return false;
    }
    let path = format!("{dir}/{name}.bin");
    match std::fs::read(&path) {
        Ok(data) => {
            let on = hal::hal_xr_replay(&data);
            log!("xr stream: replay of {path} ({} entries): {}", data.len() / hal::hal_xr_track_entry_size().max(1), if on { "playing" } else { "too short, not played" });
            on
        }
        Err(e) => {
            log!("xr stream: replay: {path}: {e}");
            false
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn rot(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
        let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
        let t = [2.0 * (y * v[2] - z * v[1]), 2.0 * (z * v[0] - x * v[2]), 2.0 * (x * v[1] - y * v[0])];
        [v[0] + w * t[0] + (y * t[2] - z * t[1]), v[1] + w * t[1] + (z * t[0] - x * t[2]), v[2] + w * t[2] + (x * t[1] - y * t[0])]
    }
    fn unit(q: [f32; 4]) -> [f32; 4] {
        let n = q.iter().map(|a| a * a).sum::<f32>().sqrt();
        q.map(|a| a / n)
    }

    /// The folded rows give the shader's old per-pixel path (render eye -> room -> depth camera).
    #[test]
    fn env_rows_match_the_two_rotations() {
        let r = XrRenderEye { orientation: unit([0.1, -0.3, 0.05, 0.94]), position: [0.03, 1.6, -0.2], fov: [-0.9, 0.75, 0.8, -0.85] };
        let env_rot = unit([-0.05, 0.2, 0.1, 0.97]);
        let env_pos = [0.0, 1.62, -0.18];
        let et = [-1.1f32, 1.0, 1.05, -0.95];
        for map in 0..8u8 {
            let [ku, kv, kz] = env_rows(&r, &env_rot, &env_pos, &et, map);
            for &(u, v, z) in &[(0.2f32, 0.3f32, 0.7f32), (0.5, 0.5, 2.0), (0.9, 0.1, 1.3)] {
                // the old path
                let rt = r.fov.map(f32::tan);
                let d = [rt[0] + (rt[1] - rt[0]) * u, rt[2] + (rt[3] - rt[2]) * v, -1.0];
                let dw = rot(r.orientation, d.map(|a| a * z));
                let pw = [r.position[0] + dw[0], r.position[1] + dw[1], r.position[2] + dw[2]];
                let pe = rot([-env_rot[0], -env_rot[1], -env_rot[2], env_rot[3]], [pw[0] - env_pos[0], pw[1] - env_pos[1], pw[2] - env_pos[2]]);
                let ze = -pe[2];
                let mut t = [(pe[0] / ze - et[0]) / (et[1] - et[0]), (pe[1] / ze - et[3]) / (et[2] - et[3])];
                if map & 1 != 0 { t[0] = 1.0 - t[0]; }
                if map & 2 != 0 { t[1] = 1.0 - t[1]; }
                if map & 4 != 0 { t.swap(0, 1); }
                // the rows
                let h = [u * z, v * z, z, 1.0];
                let dot = |k: [f32; 4]| k.iter().zip(h).map(|(a, b)| a * b).sum::<f32>();
                let zk = dot(kz);
                let tk = [dot(ku) / zk, dot(kv) / zk];
                assert!((zk - ze).abs() < 1e-4, "map {map}: distance {zk} vs {ze}");
                assert!((tk[0] - t[0]).abs() < 1e-4 && (tk[1] - t[1]).abs() < 1e-4, "map {map}: uv {tk:?} vs {t:?}");
            }
        }
    }
}

/// The HAL's clock (ns) where there is one; the web has none here (0).
fn clock_now_ns() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    return makepad_widgets::makepad_platform::makepad_hal::clock_ns();
    #[cfg(target_arch = "wasm32")]
    0
}
