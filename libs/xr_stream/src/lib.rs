//! Remote rendering for headsets over Desk Link XR (`makepad_desk_link::xr`).
//!
//! - [`serve`] + [`XrServe`]: an app in a process with a remote headset slot
//!   (`MAKEPAD_XR_REMOTE=1`) serves its XR view: it renders as on a headset (its XR pass, paced
//!   by the headset's poses) into its own textures; `XrServe` packs each frame on the GPU and
//!   hands it to the encoder (`makepad_desk_link::xr_host`), and mirrors it on the desktop.
//! - [`XrStreamClient`]: on the headset, a thin client: it sends each XR frame's pose (head,
//!   eyes, controllers, hands) upstream, decodes the pictures on a worker thread and draws the
//!   newest into the app's XR pass with [`DrawXrStream`], each eye its half of the picture.
//!   The app keeps no state of the remote app; its own XR window only shows the stream.
//!
//! Stats per picture (logged every 5 s, [`XrStreamClient::status`]): the server's render, read,
//! convert and encode, the round trip to decoded, the decode, and the render pose's age at the
//! display time of the frame that shows it.

use makepad_widgets::makepad_draw::render_target::{RenderTarget, RenderTargetFormat};
use makepad_widgets::makepad_draw::shader::draw_nv12::DrawNv12;
use makepad_widgets::*;
/// The XR server's port when the target names none.
pub const DEFAULT_PORT: u16 = 8794;
/// The room field goes up in messages of about this size (bytes).
const WORLD_MESSAGE_BYTES: usize = 16 << 10;
/// The decoded pictures the client keeps queued on average before it skips one (the reference
/// client's default).
const BUFFERED: f64 = 2.0;
/// The furthest ahead poses are predicted (a slow link does not extrapolate the head for ever).
const MAX_POSE_LEAD_NS: u64 = 100_000_000;

mod serve;
pub use serve::{serve, DrawXrBand, DrawXrMirror, XrServe};
/// The serving side's shaders and `mod.draw.XrServe` (an app that serves registers them).
pub fn serve_script_mod(vm: &mut ScriptVm) -> ScriptValue {
    serve::script_mod(vm)
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
    // band-depth target (`XrStreamClient::draw`): the stream's vertices read it there (a
    // decoder's Y'CbCr picture is sampled in fragment stages only).
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
            return vec4(vd, vd, vd, 1.0)
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
        // eye (red left, blue right); `render_*`: the eye the picture was
        // rendered with (orientation, position, fov tangents left right up down); `env_*`: the
        // environment depth camera (orientation, position with its near plane in w, fov
        // tangents), `env_far` its far plane (0 infinite)
        eye: uniform(0.0)
        video_mode: uniform(0.0)
        occlude: uniform(0.0)
        depth_near: uniform(0.0)
        opaque: uniform(0.0)
        render_rot: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        render_pos: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        render_tan: uniform(vec4(-1.0, 1.0, 1.0, -1.0))
        env_rot: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        env_pos: uniform(vec4(0.0, 0.0, 0.0, 0.1))
        env_tan: uniform(vec4(-1.0, 1.0, 1.0, -1.0))
        env_far: uniform(0.0)
        env_map: uniform(0.0)
        // 1: the picture is reprojected to the view's eye in this pass (see `vertex`); the
        // frame then goes to the compositor with the view's own eyes
        warp: uniform(0.0)
        grid_cells: uniform(64.0)
        // 1 on a vertex at a depth edge (see `vertex`)
        edge: varying(float)
        // The picture's depth at grid point `g` (m along the render eye's axis; far where
        // nothing was drawn), from the band-depth target, nearest texel
        band_z: fn(g: vec2) -> float {
            let bs = self.band_size
            let half = vec2(bs.x * 0.5, bs.y)
            let gc = clamp(g, vec2(0.0, 0.0), vec2(1.0, 1.0))
            let t = (min(floor(gc * half), half - vec2(1.0, 1.0)) + vec2(0.5, 0.5) + vec2(self.eye * half.x, 0.0)) / bs
            let vd = self.band_depth.sample_lod(t, 0.0).x
            return mix(50.0, self.depth_near / max(vd, 0.0001), step(0.004, vd))
        }
        env_size: uniform(vec2(1.0, 1.0))
        qrot: fn(q: vec4, v: vec3) -> vec3 {
            let t = cross(q.xyz, v) * 2.0
            return v + t * q.w + cross(q.xyz, t)
        }
        // The picture's point at this pixel at depth `z` (m, along the render eye's axis) seen by
        // the depth camera: (its distance there, what the camera measured along that ray); y < 0
        // where the depth image has nothing
        room_depth: fn(z: float) -> vec2 {
            let rt = self.render_tan
            let d = vec3(mix(rt.x, rt.y, self.pos.x), mix(rt.z, rt.w, self.pos.y), -1.0)
            let pw = self.render_pos.xyz + self.qrot(self.render_rot, d * z)
            let ec = vec4(0.0 - self.env_rot.x, 0.0 - self.env_rot.y, 0.0 - self.env_rot.z, self.env_rot.w)
            let pe = self.qrot(ec, pw - self.env_pos.xyz)
            let ze = 0.0 - pe.z
            if ze <= 0.01 {
                return vec2(ze, -1.0)
            }
            let et = self.env_tan
            // (the depth image's first row is the bottom of its view: y up, as the room field
            // reads it)
            let uv = vec2((pe.x / ze - et.x) / (et.y - et.x), (pe.y / ze - et.w) / (et.z - et.w))
            if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                return vec2(ze, -1.0)
            }
            // (debug `env_map`: bit 0 mirrors x, bit 1 mirrors y, bit 2 swaps the axes)
            let m = self.env_map
            var t = uv
            if fract(m * 0.5) > 0.25 {
                t.x = 1.0 - t.x
            }
            if fract(floor(m * 0.5) * 0.5) > 0.25 {
                t.y = 1.0 - t.y
            }
            if m > 3.5 {
                t = t.yx
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
        occlusion: fn(z: float) -> float {
            if self.occlude < 0.5 || z <= 0.0 {
                return 0.0
            }
            let r = self.room_depth(z)
            if r.y < 0.0 {
                return 0.0
            }
            return clamp((r.x - r.y - 0.02) / 0.04, 0.0, 1.0)
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
            var at = proj * vec4(ex, ey, -1.0, 1.0)
            self.edge = 0.0
            if self.warp > 0.5 && self.video_mode > 0.5 {
                // `warp`: the picture reprojected here, to this view's own eye: each grid vertex
                // is the picture's point there (its render eye's ray at the band's depth, far
                // where nothing was drawn) seen by this view's camera: rotation AND translation
                // since the render pose, per vertex
                let z = self.band_z(g)
                // a vertex where the depth jumps (an object's edge) is on a triangle stretched
                // across the gap from the reprojection: such triangles are not drawn (the room
                // shows there) instead of smearing the edge into slices
                let n = 1.0 / self.grid_cells
                let z1 = self.band_z(g + vec2(n, 0.0))
                let z2 = self.band_z(g - vec2(n, 0.0))
                let z3 = self.band_z(g + vec2(0.0, n))
                let z4 = self.band_z(g - vec2(0.0, n))
                let zmin = min(min(min(z1, z2), min(z3, z4)), z)
                let zmax = max(max(max(z1, z2), max(z3, z4)), z)
                self.edge = step(1.15, zmax / max(zmin, 0.01))
                let rt = self.render_tan
                let d = vec3(mix(rt.x, rt.y, g.x), mix(rt.z, rt.w, g.y), -1.0)
                let pw = self.render_pos.xyz + self.qrot(self.render_rot, d * z)
                at = proj * (self.draw_pass.camera_view * vec4(pw, 1.0))
            }
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
                let r = self.room_depth(1.0)
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
            if self.opaque > 3.5 && self.video_mode > 0.5 {
                // (debug `opaque` 4: the colour alone, no coverage or depth samples: their cost)
                let c4 = self.video.sample_video((self.src.xy + self.pos * vec2(self.src.z, ch)) / self.nv12_size)
                return vec4(self.nv12_color(vec3(c4.y, c4.z, c4.x) * 255.0), 1.0)
            }
            if self.video_mode > 0.5 {
                let vs = self.nv12_size
                let c = self.video.sample_video((self.src.xy + self.pos * vec2(self.src.z, ch)) / vs)
                let rgb = self.nv12_color(vec3(c.y, c.z, c.x) * 255.0)
                let vap = vec2(self.src.x, ch) + self.pos * vec2(self.src.z * 0.5, ch * 0.5)
                let va = self.video.sample_video((floor(vap) + vec2(0.5, 0.5)) / vs).y
                let vd = self.video.sample_video((floor(vap + vec2(self.src.z * 0.5, 0.0)) + vec2(0.5, 0.5)) / vs).y
                let vz = self.depth_near / max(vd, 0.0001)
                let keep = 1.0 - self.occlusion(mix(0.0, vz, step(0.004, vd)))
                let ca = max(clamp(va, 0.0, 1.0), self.opaque)
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
            let keep = 1.0 - self.occlusion(mix(0.0, self.depth_near / max(dvb, 0.0001), step(0.004, dvb)))
            return vec4(mix(top, bottom, f.y) * keep, a * keep)
        }
    }
}

/// The stream's grid: GRID x GRID cells over the unit square, each a quad of the quad's own
/// vertex layout (its corners carry the picture's depth when it is reprojected). Set on the
/// draw at draw time: declared as the shader's vertex buffer it moved the draw's instance
/// fields (the picture's source rect read garbage: nothing showed).
const GRID: usize = 128;
fn xr_grid_geometry(cx: &mut Cx) -> Geometry {
    let mut g = makepad_widgets::makepad_draw::geometry::geometry_gen::GeometryGen::default();
    let s = 1.0 / GRID as f32;
    for y in 0..GRID {
        for x in 0..GRID {
            g.add_quad_2d(x as f32 * s, y as f32 * s, (x + 1) as f32 * s, (y + 1) as f32 * s);
        }
    }
    g.into_geometry(cx)
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
    status: String,
    stop: bool,
    /// The uplink's last 5 s (rate, interval spread).
    uplink_line: String,
    /// The same as numbers: poses/s, the intervals' spread (ms).
    uplink: (f32, f32),
    /// The poses' datagram path (`XrServerMsg::Uplink`): the socket, connected to the server,
    /// and the key that seals them.
    #[cfg(not(target_arch = "wasm32"))]
    udp: Option<Arc<(std::net::UdpSocket, Vec<u8>)>>,
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
    /// The largest difference between a picture's render eyes and the eyes sent for its pose
    /// (quaternion components).
    echo_err_deg: f32,
    /// Display frames without a new picture; pictures skipped to catch up (the queue above
    /// BUFFERED); pictures dropped unshown (one eye decoded without the other's pair).
    repeats: u32,
    skipped: u32,
    dropped: u32,
    /// When (ms into the window) a frame repeated ('r'), a picture was skipped ('s'), the
    /// display skipped a frame ('l'): to line up with what the wearer saw.
    events: Vec<(u32, char)>,
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
}

/// The headset end (see the module). Owned by the app's XR view.
pub struct XrStreamClient {
    shared: Arc<Mutex<Shared>>,
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
    /// The running average of decoded pictures waiting at a take (see BUFFERED).
    queue_avg: f64,
    /// Stats windows (seconds) so far.
    windows: u64,
    /// The smoothed lead (ns) from a pose's display time to its picture's: how far ahead poses
    /// are predicted (`Cx::xr_set_pose_lead`).
    lead_ns: f64,
    /// The last motion-to-photon samples the lead averages.
    leads: VecDeque<f64>,
    /// The previous XR frame's display time (a gap of more than 1.5 periods: a display frame
    /// the app missed).
    last_display_ns: u64,
    /// A per-display-frame trace being recorded (`start_trace`).
    trace: Option<Trace>,
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
    /// The warp's grid geometry (made at the first warped draw).
    grid: Option<Geometry>,
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
    }
}

impl XrStreamClient {
    /// Starts linking to the XR server at `host:port` (on a worker thread: the UI never waits).
    pub fn connect(host: String, port: u16, eye: [u32; 2], refresh_hz: f64) -> XrStreamClient {
        let shared = Arc::new(Mutex::new(Shared { status: format!("connecting to {host}:{port}"), tiles: 1, ..Default::default() }));
        let signal = SignalToUI::new();
        let sent = Arc::new(Mutex::new(VecDeque::new()));
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
            let (shared, signal) = (shared.clone(), signal.clone());
            // the radio never naps while a stream runs (power save adds tens of ms per picture)
            makepad_widgets::makepad_platform::makepad_hal::hal_network_low_latency(true);
            let info = HeadsetInfo { name: "headset".into(), eye, refresh_mhz: (refresh_hz * 1000.0) as u32, codecs: codecs::HEVC | codecs::H264 };
            let _ = std::thread::Builder::new().name("xr-stream".into()).spawn(move || worker(host, port, info, shared, signal));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (host, port, eye, refresh_hz);
        XrStreamClient {
            shared,
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
            queue_avg: 0.0,
            windows: 0,
            lead_ns: 0.0,
            leads: VecDeque::new(),
            last_display_ns: 0,
            trace: None,
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
            grid: None,
            #[cfg(not(target_arch = "wasm32"))]
            probe_ticket: None,
            focus_override_m: 0.0,
        }
    }

    /// The link's state and the last stats line, for the app's own UI and logs.
    pub fn status(&self) -> String {
        let s = self.shared.lock().unwrap();
        if self.line.is_empty() { s.status.clone() } else { format!("{} · {}", s.status, self.line) }
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
        match event {
            Event::XrUpdate(_) => {
                self.window.display_frames += 1;
                #[cfg(not(target_arch = "wasm32"))]
                if let Some((display_ns, _)) = cx.xr_frame_wire() {
                    let last = std::mem::replace(&mut self.last_display_ns, display_ns);
                    let period = 1.0e9 / 90.0;
                    if last != 0 && display_ns > last && (display_ns - last) as f64 > period * 1.5 {
                        self.window.events.push((self.window_at.elapsed().as_millis() as u32, 'l'));
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    // the picture made current last time is on the glass at this frame's display
                    if let (Some((_, sample_ns, decoded_at, _)), Some((display_ns, _))) = (self.pending_show.take(), cx.xr_frame_wire()) {
                        self.window.shown += 1;
                        self.window.wait_us.push(decoded_at.elapsed().as_micros() as u32);
                        if display_ns > sample_ns {
                            self.window.m2p_us.push(((display_ns - sample_ns) / 1000) as u32);
                        }
                        // the poses sent upstream are predicted for when their picture will show:
                        // the measured motion to photon (the pose's sample to the display of its
                        // picture, smoothed), as far ahead of the sample as the headset locates them
                        let lead = display_ns.saturating_sub(sample_ns) as f64;
                        // (the mean of the last 256, as the reference client averages its total
                        // latency: a faster average moved every picture's pose with each
                        // re-measured latency)
                        self.leads.push_back(lead);
                        if self.leads.len() > 256 {
                            self.leads.pop_front();
                        }
                        self.lead_ns = self.leads.iter().sum::<f64>() / self.leads.len() as f64;
                        cx.xr_set_pose_lead((self.lead_ns as u64).min(MAX_POSE_LEAD_NS));
                    }
                    if let (Some((display_ns, _)), env_ns) = (cx.xr_frame_wire(), cx.xr_environment_depth_display_ns()) {
                        if env_ns != 0 && display_ns >= env_ns {
                            self.window.env_age_us.push(((display_ns - env_ns) / 1000) as u32);
                        }
                    }
                    let link = self.shared.lock().unwrap().link.clone();
                    if let Some(link) = &link {
                        self.send_world(cx, link);
                    }

                    // this display frame's picture: the oldest decoded one
                    let fresh = self.take_picture(cx);
                    if !fresh {
                        self.window.repeats += 1;
                        self.window.events.push((self.window_at.elapsed().as_millis() as u32, 'r'));
                    }
                    self.trace_frame(cx, fresh);
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
                self.warp_probe_result(cx);
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

    /// Uploads the newest complete picture (every tile of one pose), if a new one came.
    fn take_picture(&mut self, cx: &mut Cx) -> bool {
        // (debug `freeze`: the last picture keeps showing with its own eyes: the headset's
        // reprojection alone keeps it in place)
        if self.freeze && self.shown[0].is_some() {
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let tiles = {
                let mut s = self.shared.lock().unwrap();
                let tiles = s.tiles.clamp(1, 2) as usize;
                // As the reference client does: one picture per display frame, the OLDEST both
                // eyes are decoded for, so pictures that arrive unevenly (two in one display
                // frame, none in the next: the server's clock is not the display's) still show
                // one per frame; when the queue stays above BUFFERED pictures on average, one is
                // skipped to catch up.
                let complete: Vec<u64> = s.latest[0].iter().map(|d| d.head.seq).filter(|seq| tiles == 1 || s.latest[1].iter().any(|d| d.head.seq == *seq)).collect();
                let Some(&oldest) = complete.first() else { return false };
                self.queue_avg = self.queue_avg * 0.9 + complete.len() as f64 * 0.1;
                let seq = if self.queue_avg > BUFFERED && complete.len() > 1 {
                    self.window.skipped += 1;
                    self.window.events.push((self.window_at.elapsed().as_millis() as u32, 's'));
                    self.queue_avg -= 1.0;
                    complete[1]
                } else {
                    oldest
                };
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

    /// Starts a per-display-frame trace of `secs` seconds, written to `path` as CSV when done.
    pub fn start_trace(&mut self, secs: f64, path: String) {
        self.trace = Some(Trace { until: Instant::now() + Duration::from_secs_f64(secs), path, rows: String::from(
            "display_ns,head_qx,head_qy,head_qz,head_qw,head_x,head_y,head_z,new,seq,pic_display_ns,render_qx,render_qy,render_qz,render_qw,render_x,render_y,render_z,sample_ns,lead_ns,decoded_age_us,queue\n") });
    }

    /// One trace row: this display frame, its head (located at its display time), the picture
    /// it shows (new or repeated), that picture's render eye 0 and target time, the sample time
    /// of its pose, the lead, how long it waited decoded, the decoded pictures queued.
    #[cfg(not(target_arch = "wasm32"))]
    fn trace_frame(&mut self, cx: &Cx, fresh: bool) {
        let Some(t) = self.trace.as_mut() else { return };
        if Instant::now() >= t.until {
            let t = self.trace.take().unwrap();
            match std::fs::write(&t.path, t.rows.as_bytes()) {
                Ok(()) => log!("xr stream: trace written to {}", t.path),
                Err(e) => log!("xr stream: trace not written to {}: {e}", t.path),
            }
            return;
        }
        let Some((display_ns, wire)) = cx.xr_frame_wire() else { return };
        let Some(frame) = makepad_widgets::makepad_platform::makepad_hal::buffers::read_items::<makepad_widgets::makepad_platform::makepad_hal::XrFrame>(&wire, 1).next() else { return };
        let Some(shown) = self.shown[0].as_ref() else { return };
        let (h, r) = (&frame.head, &shown.head.eyes[0]);
        let sample_ns = self.pending_show.map_or(0, |p| p.1);
        let age = self.pending_show.map_or(0, |p| p.2.elapsed().as_micros() as u64);
        let queue = self.shared.lock().unwrap().latest[0].len();
        use std::fmt::Write;
        let _ = writeln!(t.rows, "{display_ns},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{sample_ns},{:.0},{age},{queue}",
            h.orientation[0], h.orientation[1], h.orientation[2], h.orientation[3], h.position[0], h.position[1], h.position[2],
            fresh as u8, shown.head.seq, shown.head.display_ns,
            r.orientation[0], r.orientation[1], r.orientation[2], r.orientation[3], r.position[0], r.position[1], r.position[2], self.lead_ns);
    }

    /// Every second: the window's numbers to the server (its stats panel); logged every 5th.
    fn stats_window(&mut self) {
        let secs = self.window_at.elapsed().as_secs_f64();
        if secs < 1.0 {
            return;
        }
        self.windows += 1;
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
            "display {:.0}/s · recv {:.0}/s dec {:.0}/s shown {:.0}/s · m2p {:.0} ms = render {:.1} gpu {:.1} enc {:.1} send {:.1} net {:.1} dec {:.1} wait {:.1} (trip {:.0}) · {} unmatched · env depth age {:.0}/{:.0} ms · pose echo max diff {:e} · repeats {} skipped {} dropped {}",
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
            if !self.warp || self.shown[0].is_none() {
                return;
            }
            let tile_size = self.shared.lock().unwrap().tile_size;
            let tiled = self.shown[1].is_some();
            let ew = if tiled { tile_size[0] } else { tile_size[0] / 2 };
            let ch = (tile_size[1] as f32 * 2.0 / 3.0).round() as u32;
            let band_size = ((ew / 2) * 2, (tile_size[1].saturating_sub(ch)).max(2) / 2);
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
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(first) = self.shown[0].as_ref() else { return };
            let tile_size = self.shared.lock().unwrap().tile_size;
            let band_size = self.band_rt.as_ref().map_or((1, 1), |rt| rt.size());
            let eye = |e: &XrRenderEye| makepad_widgets::makepad_platform::makepad_hal::XrEye {
                pose: makepad_widgets::makepad_platform::makepad_hal::XrPose { orientation: e.orientation, position: e.position },
                fov: e.fov,
            };
            // (warped here, the frame shows with the view's own eyes: the compositor corrects
            // only the last few ms; else it shows with the picture's and the compositor
            // corrects the rotation since then)
            if !self.own_eyes && !self.warp {
                cx.cx.xr_present_eyes([eye(&first.head.eyes[0]), eye(&first.head.eyes[1])], if !self.depth_layer { 0.0 } else if self.focus_override_m > 0.0 { self.focus_override_m } else { first.head.focus_m });
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
                    (live_id!(opaque), vec![self.opaque as f32]),
                    (live_id!(env_map), vec![self.env_map as f32]),
                    (live_id!(warp), vec![self.warp as u8 as f32]),
                    (live_id!(band_size), vec![band_size.0 as f32, band_size.1 as f32]),
                    (live_id!(grid_cells), vec![GRID as f32]),
                ];
                match cx.cx.xr_environment_depth().filter(|_| self.occlusion) {
                    Some((depth, header)) => {
                        // this eye's own depth camera and image layer
                        let v = &header.views[e];
                        let p = v.pose.position;
                        uniforms.extend([
                            (live_id!(env_rot), v.pose.orientation.to_vec()),
                            (live_id!(env_pos), vec![p[0], p[1], p[2], header.near]),
                            (live_id!(env_tan), tan(v.fov).to_vec()),
                            (live_id!(env_size), vec![header.width as f32, header.height as f32]),
                            (live_id!(env_far), vec![if header.far.is_finite() { header.far.max(0.0) } else { 0.0 }]),
                            (live_id!(occlude), vec![if self.room_view { 2.0 } else { 1.0 }]),
                        ]);
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
                match self.band_rt.as_ref().filter(|_| self.warp) {
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
                if self.warp {
                    let grid = self.grid.get_or_insert_with(|| xr_grid_geometry(cx.cx));
                    draw.draw_vars.geometry_id = Some(grid.geometry_id());
                }
                draw.new_draw_call(cx);
                draw.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(1.0, 1.0) });
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (cx, draw);
    }
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
            q.push_back(Sent { seq, at: now, sample_ns, display_ns, eyes });
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
fn worker(host: String, port: u16, info: HeadsetInfo, shared: Arc<Mutex<Shared>>, signal: SignalToUI) {
    let set_status = |text: String| {
        log!("xr stream: {text}");
        shared.lock().unwrap().status = text;
        SignalToUI::set_ui_signal();
    };
    loop {
        if shared.lock().unwrap().stop {
            return;
        }
        let link = match XrLink::connect(&host, port, info.clone(), Arc::new(|| {})) {
            Ok(link) => Arc::new(link),
            Err(e) => {
                set_status(format!("{e}; retrying"));
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        // a decoder per tile, each on its own thread (the eyes decode in parallel)
        let mut tiles: Vec<std::sync::mpsc::Sender<(VideoHead, Vec<u8>, Instant)>> = Vec::new();
        let mut pinged = Instant::now();
        let why = loop {
            if shared.lock().unwrap().stop {
                return;
            }
            if pinged.elapsed() > Duration::from_secs(1) {
                link.send(&XrClientMsg::Ping { client_ns: 0 });
                pinged = Instant::now();
            }
            if let Some(why) = link.closed() {
                break why;
            }
            let Some(msg) = link.recv_timeout(Duration::from_millis(5)) else { continue };
            match msg {
                XrServerMsg::Welcome { server, size, codec, tiles: n, encoder, .. } => {
                    let codec = if codec == CODEC_HEVC { StreamVideoCodec::Hevc } else { StreamVideoCodec::H264 };
                    let n = n.clamp(1, 2);
                    let tile_size = [size[0] / n as u32, size[1]];
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
                    shared.lock().unwrap().received += 1;
                    let head = VideoHead { seq, tile, key, display_ns, eyes, focus_m, stages, size, codec };
                    let _ = tx.send((head, data, Instant::now()));
                }
                XrServerMsg::Note { text } => log!("xr stream: server: {text}"),
                XrServerMsg::Uplink { port, key } => {
                    let socket = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|s| s.connect((host.as_str(), port)).map(|_| s));
                    match socket {
                        Ok(socket) => {
                            log!("xr stream: poses go as datagrams to port {port}");
                            shared.lock().unwrap().udp = Some(Arc::new((socket, key)));
                        }
                        Err(e) => log!("xr stream: no pose datagrams ({e}): poses go on the link"),
                    }
                }
                XrServerMsg::Refused { reason } | XrServerMsg::Bye { reason } => break reason,
                XrServerMsg::Pong { .. } => {}
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
                    // a few at most: the oldest goes (its buffer back to the decoder)
                    while held.len() >= 4 {
                        held.pop_front();
                    }
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
