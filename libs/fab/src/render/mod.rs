//! Progressive ray tracing integration for Fab viewports and final renders.
//!
//! Integration of the reusable GPU ray tracer (`makepad_raytrace`):
//! * [`RenderedPreview`] implements the FROZEN [`RenderedPreviewApi`] seam —
//!   lane B's viewport calls `draw` whenever `views[view].shading ==
//!   Shading::Rendered`; F parents its pass chain under the viewport's pass,
//!   accumulates into its own textures, restarts on `render_dirty` (after a
//!   ~150 ms quiet hold while the camera is moving) and is the only party
//!   that clears it, and returns the tonemapped frame. The pane draws its
//!   realtime composite as the base layer and the frame composites over it:
//!   untraced pixels come out transparent (`untraced_transparent`), so a
//!   fresh spiral replaces a good raster tile by tile — never a lone tile
//!   crawling over a flat placeholder. A headless track job bypasses the
//!   hold.
//! * F12 — [`FabRenderView`], a full-size render at
//!   `RenderSettings::width × height` with progress, Save PNG (`ExportPng`).
//! * The Render tab of the Properties editor is lane D's chrome; it emits
//!   `SetRenderSettings`, consumed here through `state.render`.
//!
//! Data contract: the `SceneSnapshot` is converted and uploaded ONCE per
//! `(generation, scene_state.revision)` — the revision only matters because
//! hidden/isolated elements are dropped from the trace set (a CPU rebuild on
//! a user action, never per frame). Camera comes from
//! `ViewportState::camera` (f-stop / focus distance included), sun from
//! `SkyState::direction()`, sky exposure from `SkyState`, and
//! denoise/bounces/samples from `RenderSettings`.
//!
//! Camera parity: the G-buffer raster uses `camera.view_projection(aspect)`
//! verbatim (bit-identical to `ViewProjector::new`); the ray generator
//! derives the same frustum from `fov_y_deg` / `ortho_height` and `aspect`.

use crate::api::*;
use crate::model::{ElementId, SceneSnapshot};
use makepad_raytrace::{Camera as PtCamera, Image, Material, RayTracer, RenderSettings as PtSettings, SceneInput, Sun};
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.fab.*
    use mod.widgets.*

    let _tracer_shaders = #(register_tracer(vm))
    mod.widgets.FabRenderViewBase = #(FabRenderView::register_widget(vm))
    mod.widgets.FabRenderView = set_type_default() do mod.widgets.FabRenderViewBase{
        width: Fill
        height: Fill
        flow: Down
        align: Align{x: 0.5 y: 0.5}
        show_bg: true
        draw_bg +: {
            color: fab.color_editor
        }
        status := Label{
            text: "Render — F12 to start (lane F)"
            draw_text +: {
                color: fab.color_text_dim
                text_style: theme.font_regular{
                    font_size: fab.font_size_ui
                }
            }
        }
    }
}

/// The tracer's shader types (`mod.draw.PtTrace` …) must be registered in
/// this VM before a `RayTracer` is created; the host only knows this
/// module, so this module registers them (the standalone app does the same
/// in its `AppMain::script_mod`). Without it every tracer draw struct comes
/// up shaderless and the Rendered pane stays a cleared transparent target —
/// the black-pane bug of 2026-08-24.
fn register_tracer(vm: &mut ScriptVm) -> ScriptValue {
    makepad_raytrace::script_mod(vm);
    NIL
}

/// The api f-stop is a creative aperture: a 35 mm lens scaled 10× so a
/// building's windows visibly blur at f/2 instead of needing a 500 mm lens.
const BOKEH_SCALE: f32 = 10.0;

/// `SceneSnapshot` → the tracer's flat input, dropping hidden elements.
pub fn scene_input_from_snapshot(snap: &SceneSnapshot, state: &AppState) -> SceneInput {
    let mut s = SceneInput { up: vec3(0.0, 0.0, 1.0), ..Default::default() };
    s.positions = snap.positions.clone();
    s.normals = snap.normals.clone();
    s.uvs = snap.uvs.clone();
    // Element visibility: only visible triangles enter the trace set.
    let n_tris = snap.indices.len() / 3;
    let mut visible = vec![true; snap.elements.len().max(1)];
    for (e, v) in visible.iter_mut().enumerate() {
        if e < snap.elements.len() {
            *v = state.scene_state.is_visible(&state.scene, ElementId(e as u32));
        }
    }
    s.indices.reserve(snap.indices.len());
    s.tri_material.reserve(n_tris);
    s.tri_priority.reserve(n_tris);
    s.tri_coplanar_group.reserve(n_tris);
    for t in 0..n_tris {
        let e = snap.triangle_element.get(t).copied().unwrap_or(0) as usize;
        if !visible.get(e).copied().unwrap_or(true) {
            continue;
        }
        s.indices.extend_from_slice(&snap.indices[t * 3..t * 3 + 3]);
        s.tri_material.push(snap.triangle_material.get(t).copied().unwrap_or(0));
        s.tri_priority.push(snap.triangle_priority.get(t).copied().unwrap_or(0));
        s.tri_coplanar_group
            .push(snap.triangle_coplanar_group.get(t).copied().unwrap_or(0));
    }
    s.materials = snap
        .materials
        .iter()
        .map(|m| {
            Material {
                albedo: [m.albedo[0], m.albedo[1], m.albedo[2]],
                roughness: m.roughness,
                metal: m.metallic,
                emission: m.emission,
                ior: if m.ior > 1.0 { m.ior } else { 1.5 },
                transmission: m.transmission,
                texture: if m.texture == u32::MAX { None } else { Some(m.texture as usize) },
                two_sided: m.double_sided,
            }
        })
        .collect();
    if s.materials.is_empty() {
        s.materials.push(Material::default());
    }
    s.images = snap
        .textures
        .iter()
        .map(|t| {
            let data = t
                .rgba
                .chunks_exact(4)
                .map(|p| ((p[3] as u32) << 24) | ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32)
                .collect();
            Image { width: t.width as usize, height: t.height as usize, data }
        })
        .collect();
    s
}

/// `api::Camera` → the tracer's camera (same frustum, same lens semantics).
pub fn camera_from_api(cam: &Camera) -> PtCamera {
    PtCamera {
        pos: cam.eye,
        target: cam.target,
        up: cam.up,
        fov_y: cam.fov_y_deg.to_radians(),
        focal_mm: 35.0,
        f_stop: cam.f_stop.max(0.0),
        focus_dist: cam.focus_distance.max(0.01),
        bokeh_scale: BOKEH_SCALE,
        blades: 6,
        ortho_height: if cam.ortho { Some(cam.ortho_height.max(1.0e-3)) } else { None },
    }
}

pub fn sun_from_api(sun: &SunSettings) -> Sun {
    Sun {
        dir: sun.direction(),
        turbidity: sun.turbidity.clamp(1.2, 10.0),
        sky_strength: 1.0,
        sun_strength: 4.0,
    }
}

pub fn settings_from_api(r: &RenderSettings, sky: &SkyState, preview: bool) -> PtSettings {
    PtSettings {
        target_spp: r.max_samples.max(1),
        max_bounces: r.bounces.clamp(1, 16),
        max_diffuse: r.bounces.clamp(1, 4),
        // EV 0 is the tracer's calibrated daylight exposure. Firefly
        // suppression is an explicitly biased interactive-preview option;
        // F12/final renders always use the unbiased estimator.
        exposure: sky.exposure(),
        preview_clamp: if preview { Some(12.0) } else { None },
        hybrid_primary: preview,
        denoise: r.denoise,
        // Preview-only: after 64 samples, converged pixels keep their clean
        // accumulated mean while noisy pixels continue to the global limit.
        // Final/F12 and parity runs remain fully sampled.
        adaptive_min: if preview { 64 } else { 0 },
        // The interactive pane draws the realtime composite underneath and
        // blits the accumulation over it, so pixels no tile has reached are
        // transparent, not the flat G-buffer fallback. F12 keeps the
        // fallback (it draws on the editor background).
        untraced_transparent: preview,
        // The resolution ladder: a complete coarse traced picture within the
        // first few dispatches, sharpening rung by rung to native, then
        // converging. Interactive panes only — F12/track render native-only.
        progressive: preview,
        // `FAB_PT_DEBUG=n` selects the tracer's debug output (the modes are
        // documented on `PtSettings::debug_mode`) — the seam's diagnostic
        // knob, like `MAKEPAD_PT_BUDGET_MS`.
        debug_mode: std::env::var("FAB_PT_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
        ..Default::default()
    }
}

/// A camera track (lane G's `*-frames.json`: one key per frame at `fps`).
#[derive(Clone, Debug, DeJson)]
pub struct TrackFile {
    pub fps: f32,
    pub keys: Vec<TrackKey>,
}

#[derive(Clone, Debug, DeJson)]
pub struct TrackKey {
    pub t: f32,
    pub pos: [f32; 3],
    pub look_at: [f32; 3],
    pub up: [f32; 3],
    pub fov_y_deg: f32,
}

/// Headless track rendering, driven by the environment so the host needs no
/// new flags:
///
/// ```text
/// FAB_RENDER_TRACK=local/fab/tours/woodside-frames.json   the track
/// FAB_TRACK_OUT=local/fab/renders/woodside               frame_%06d.png go here
/// FAB_TRACK_SPP=64  FAB_TRACK_SIZE=1280x720  FAB_TRACK_RANGE=0:250
/// FAB_TRACK_MP4=local/fab/renders/woodside.mp4          optional H.264
/// FAB_TRACK_DENOISE=1
/// ```
///
/// The Rendered pane renders each key's camera at the requested size to the
/// requested spp under the same tile budget as interactive use, captures
/// the tonemapped frame, writes the PNG (existing frames are skipped, so a
/// run is resumable in chunks) and quits after the range. Frames are
/// independent, so `box162 gpu 600 …` per chunk is the intended shape.
pub struct TrackJob {
    pub keys: Vec<TrackKey>,
    pub fps: f32,
    pub out: PathBuf,
    pub spp: u32,
    pub size: (usize, usize),
    pub range: (usize, usize),
    pub denoise: bool,
    pub index: usize,
    pub frame_started: bool,
    pub capture_requested: bool,
    pub started_at: std::time::Instant,
    pub frame_started_at: std::time::Instant,
    pub rendered: usize,
    pub mp4: Option<makepad_video::VideoFileEncoder>,
    pub mp4_path: Option<PathBuf>,
}

impl TrackJob {
    /// Build the job from the environment, if `FAB_RENDER_TRACK` is set.
    pub fn from_env() -> Option<TrackJob> {
        let track = std::env::var("FAB_RENDER_TRACK").ok()?;
        let text = std::fs::read_to_string(&track).map_err(|e| log!("track: cannot read {track}: {e}")).ok()?;
        let file = TrackFile::deserialize_json(&text).map_err(|e| log!("track: bad json {track}: {e:?}")).ok()?;
        let out = PathBuf::from(std::env::var("FAB_TRACK_OUT").unwrap_or_else(|_| "local/fab/renders/track".into()));
        let _ = std::fs::create_dir_all(&out);
        let spp = std::env::var("FAB_TRACK_SPP").ok().and_then(|v| v.parse().ok()).unwrap_or(64u32);
        let size = std::env::var("FAB_TRACK_SIZE")
            .ok()
            .and_then(|v| {
                let (w, h) = v.split_once('x')?;
                Some((w.parse().ok()?, h.parse().ok()?))
            })
            .unwrap_or((1280usize, 720usize));
        let size = ((size.0 / 2 * 2).max(64), (size.1 / 2 * 2).max(64));
        let n = file.keys.len();
        let range = std::env::var("FAB_TRACK_RANGE")
            .ok()
            .and_then(|v| {
                let (a, b) = v.split_once(':')?;
                Some((a.parse().ok()?, b.parse().ok()?))
            })
            .map(|(a, b): (usize, usize)| (a.min(n), b.min(n)))
            .unwrap_or((0, n));
        let denoise = std::env::var("FAB_TRACK_DENOISE").map(|v| v != "0").unwrap_or(true);
        let mp4_path = std::env::var("FAB_TRACK_MP4").ok().map(PathBuf::from);
        log!(
            "track: {} keys at {} fps from {track}; frames {}..{} at {}x{} {} spp denoise {} -> {}{}",
            n, file.fps, range.0, range.1, size.0, size.1, spp, denoise, out.display(),
            mp4_path.as_ref().map(|p| format!(", mp4 {}", p.display())).unwrap_or_default()
        );
        Some(TrackJob {
            keys: file.keys,
            fps: file.fps.max(1.0),
            out,
            spp,
            size,
            range,
            denoise,
            index: range.0,
            frame_started: false,
            capture_requested: false,
            started_at: std::time::Instant::now(),
            frame_started_at: std::time::Instant::now(),
            rendered: 0,
            mp4: None,
            mp4_path,
        })
    }

    pub fn frame_path(&self, i: usize) -> PathBuf {
        self.out.join(format!("frame_{i:06}.png"))
    }

    pub fn camera(&self, i: usize) -> Camera {
        let k = &self.keys[i];
        let mut cam = Camera::default();
        cam.eye = vec3(k.pos[0], k.pos[1], k.pos[2]);
        cam.target = vec3(k.look_at[0], k.look_at[1], k.look_at[2]);
        cam.up = vec3(k.up[0], k.up[1], k.up[2]);
        cam.fov_y_deg = k.fov_y_deg;
        cam.f_stop = 0.0;
        cam
    }

    /// True when every index in `range` has been produced or skipped.
    pub fn finished(&self) -> bool {
        self.index >= self.range.1
    }

    /// Advance `index` over frames whose PNG already exists. No-op while a
    /// key is in flight (`frame_started`).
    pub fn skip_existing(&mut self) {
        let out = self.out.clone();
        self.skip_existing_if(|i| out.join(format!("frame_{i:06}.png")).exists());
    }

    /// Same as [`Self::skip_existing`], with the existence check injected so
    /// the state machine can be unit-tested without touching the filesystem.
    pub fn skip_existing_if(&mut self, mut exists: impl FnMut(usize) -> bool) {
        while !self.finished() && !self.frame_started && exists(self.index) {
            self.index += 1;
        }
    }

    /// Start the current key if needed. Returns true when accumulation must
    /// restart — only at a key boundary, never on a subsequent draw of the
    /// same camera.
    pub fn begin_key(&mut self) -> bool {
        if self.frame_started || self.finished() {
            return false;
        }
        self.frame_started = true;
        self.capture_requested = false;
        self.frame_started_at = std::time::Instant::now();
        true
    }

    /// Record a finished capture and move to the next key.
    pub fn complete_key(&mut self) {
        self.index += 1;
        self.frame_started = false;
        self.rendered += 1;
    }

    fn mp4_push(&mut self, width: usize, height: usize, bgra: &[u8]) {
        let Some(path) = self.mp4_path.clone() else { return };
        if self.mp4.is_none() {
            let options = makepad_video::VideoFileEncoderOptions {
                codec: makepad_video::VideoFileCodec::H264,
                width: width as u32,
                height: height as u32,
                fps_num: self.fps.round() as u32,
                fps_den: 1,
                video_bitrate_bps: 16_000_000,
                ..Default::default()
            };
            match makepad_video::VideoFileEncoder::new(&path.to_string_lossy(), options) {
                Ok(e) => self.mp4 = Some(e),
                Err(e) => {
                    log!("track: mp4 encoder failed: {e:?}");
                    self.mp4_path = None;
                    return;
                }
            }
        }
        let mut rgba = Vec::with_capacity(width * height * 4);
        for px in bgra.chunks_exact(4) {
            rgba.extend([px[2], px[1], px[0], 255]);
        }
        if let Some(enc) = self.mp4.as_mut() {
            if let Err(e) = enc.push_frame_rgba8(&rgba, None) {
                log!("track: mp4 push failed: {e:?}");
            }
        }
    }

    fn finish(&mut self) {
        if let Some(enc) = self.mp4.take() {
            match enc.finish() {
                Ok(()) => log!("track: mp4 written {:?}", self.mp4_path),
                Err(e) => log!("track: mp4 finish failed: {e:?}"),
            }
        }
    }
}

/// Quiet time (ms) after the last interactive `render_dirty` before a new
/// spiral starts. Do not trace while the camera is moving.
pub const DEFAULT_MOTION_HOLD_MS: f64 = 150.0;

/// What the interactive seam should do this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionHoldAction {
    /// Camera moved recently: skip `t.draw`, show the realtime composite.
    Hold,
    /// Hold just expired: `restart` + draw this frame (still 1 tile).
    Restart,
    /// Keep drawing (no restart).
    Draw,
}

/// Quiet timer for interactive camera motion.
///
/// While `render_dirty` arrives every frame (orbit), skip tracing and show
/// the realtime composite alone. After `motion_hold_ms` of stillness,
/// restart the centre-out spiral and present from that very frame: the
/// tonemap leaves untraced pixels transparent and the pane composites the
/// accumulation over the realtime frame, so a fresh spiral shows tiles
/// replacing a good raster, never a lone tile on a flat placeholder.
/// Headless track jobs pass `bypass`; F12 (`preview = false`) does not
/// enter this machine.
#[derive(Clone, Debug)]
pub struct MotionHold {
    /// Quiet time in milliseconds (default [`DEFAULT_MOTION_HOLD_MS`]).
    pub motion_hold_ms: f64,
    last_dirty: Option<f64>,
    pending_restart: bool,
    /// True only while the quiet timer is active (camera still moving).
    moving: bool,
}

impl Default for MotionHold {
    fn default() -> Self {
        Self::new()
    }
}

impl MotionHold {
    pub fn new() -> Self {
        Self {
            motion_hold_ms: DEFAULT_MOTION_HOLD_MS,
            last_dirty: None,
            pending_restart: false,
            moving: false,
        }
    }

    /// True while the pane should show the realtime composite ALONE (the
    /// accumulation is stale: camera inside the quiet window). Outside of
    /// this the pane always presents — untraced pixels are transparent and
    /// the raster shows through them.
    pub fn showing_placeholder(&self) -> bool {
        self.moving
    }

    /// True while the camera is still inside the quiet window ("moving ·
    /// realtime" badge).
    pub fn showing_moving(&self) -> bool {
        self.moving
    }

    /// Call at the start of a frame.
    ///
    /// `now` is seconds (the same clock as `cx.time()`). `bypass` is a
    /// headless track job: never hold, always present.
    pub fn begin_frame(&mut self, now: f64, dirty: bool, bypass: bool) -> MotionHoldAction {
        if bypass {
            self.last_dirty = None;
            self.pending_restart = false;
            self.moving = false;
            return MotionHoldAction::Draw;
        }
        if dirty {
            self.last_dirty = Some(now);
            self.pending_restart = true;
        }
        if !self.is_quiet_enough(now) {
            self.moving = true;
            return MotionHoldAction::Hold;
        }
        self.moving = false;
        if self.pending_restart {
            self.pending_restart = false;
            return MotionHoldAction::Restart;
        }
        MotionHoldAction::Draw
    }

    fn is_quiet_enough(&self, now: f64) -> bool {
        match self.last_dirty {
            None => true,
            Some(t0) => (now - t0) * 1000.0 >= self.motion_hold_ms,
        }
    }
}

/// One progressive preview: a tracer plus what it was last fed.
pub struct RenderedPreview {
    tracer: Option<RayTracer>,
    /// (`SceneSnapshot::generation`, `scene_state.revision`) last uploaded.
    uploaded: Option<(u64, u64)>,
    uploads: u32,
    /// The sun last handed to the tracer. `RayTracer::set_sun` restarts the
    /// accumulation unconditionally, so it is only called when the sun moved
    /// — otherwise every draw is frame 1 of a fresh restart (0 spp forever).
    last_sun: Option<SunSettings>,
    /// Headless track rendering (see `TrackJob`); only on view 1.
    track: Option<TrackJob>,
    track_probed: bool,
    /// Quiet-timer + first-ring present gate. Interactive only.
    hold: MotionHold,
    /// The window has keyboard focus. An unfocused window traces nothing:
    /// the user is elsewhere and the GPU is theirs.
    focused: bool,
    /// Stopped by the user (Stop/Resume); the accumulation stays on screen.
    paused: bool,
    /// False as soon as the pane switches away from Rendered. This is
    /// separate from the user's Stop state so returning preserves intent.
    active: bool,
}

impl Default for RenderedPreview {
    fn default() -> Self {
        Self {
            tracer: None,
            uploaded: None,
            uploads: 0,
            last_sun: None,
            track: None,
            track_probed: false,
            hold: MotionHold::new(),
            focused: true,
            paused: false,
            active: false,
        }
    }
}

impl RenderedPreview {
    /// How many times a scene was uploaded (the once-per-generation gate).
    pub fn upload_count(&self) -> u32 {
        self.uploads
    }

    /// Headless track job is in flight (the viewport must keep NextFrame
    /// armed and redraw every tick, not only while `converging`).
    pub fn has_track(&self) -> bool {
        self.track.is_some()
    }

    /// True while the Rendered pane should blit the realtime composite
    /// (camera moving, or the first ring of a new spiral is not in yet).
    pub fn showing_placeholder(&self) -> bool {
        self.hold.showing_placeholder()
    }

    /// True only while the quiet-timer is active. The "moving · realtime"
    /// badge keys off this; the present-gate uses the ordinary pending badge.
    pub fn showing_moving(&self) -> bool {
        self.hold.showing_moving()
    }

    /// Keep the paint clock alive through the quiet-timer, the first-ring
    /// gate, accumulation, and a track job.
    ///
    /// `FAB_TRACE_UNFOCUSED=1` keeps tracing while the window has no
    /// keyboard focus — the remote-bridge verification path: a gpu-guarded
    /// session runs with hidden windows, which can never become key, and
    /// the unfocused gate would otherwise make the traced pane untestable
    /// from a harness. Inert without the env.
    pub fn set_focused(&mut self, focused: bool) {
        let focused = focused
            || std::env::var("FAB_TRACE_UNFOCUSED").map(|v| v != "0").unwrap_or(false);
        self.focused = focused;
        if let Some(t) = self.tracer.as_mut() {
            t.set_paused(self.paused || !focused || !self.active);
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if let Some(t) = self.tracer.as_mut() {
            t.set_paused(paused || !self.focused || !self.active);
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        if let Some(t) = self.tracer.as_mut() {
            t.set_paused(self.paused || !self.focused || !active);
        }
    }

    pub fn wants_frame(&self) -> bool {
        self.track.is_some()
            || (self.active
                && self.focused
                && !self.paused
                && (self.showing_placeholder()
                    || self.tracer.as_ref().map_or(false, |t| t.wants_frame())))
    }

    fn ensure_tracer(&mut self, cx: &mut Cx2d) -> bool {
        if self.tracer.is_none() {
            self.tracer = cx.cx.cx.try_with_vm(|vm| RayTracer::new(vm));
            if self.tracer.is_none() {
                log!("render: script VM busy, path tracer not created this frame");
            }
        }
        self.tracer.is_some()
    }

    /// Upload the snapshot when the generation or the visibility revision
    /// changed. Returns true when an upload happened.
    fn sync_scene(&mut self, cx: &mut Cx2d, state: &AppState) -> bool {
        let Some(snap) = state.snapshot.as_ref() else { return false };
        let key = (snap.generation, state.scene_state.revision);
        if self.uploaded == Some(key) {
            return false;
        }
        let input = scene_input_from_snapshot(snap, state);
        let tracer = self.tracer.as_mut().unwrap();
        tracer.set_scene(cx.cx.cx, &input);
        self.uploaded = Some(key);
        self.uploads += 1;
        let (lo, hi) = input.bounds();
        let cam = state.views[0].camera;
        let sun = sun_from_api(&state.sun);
        log!(
            "render: snapshot generation {} revision {} uploaded ({} tris, {} materials, {} images, upload #{}); bounds {:?}..{:?}; cam eye {:?} target {:?} fov {}; sun dir {:?} elev {:.1} deg; mat0 {:?}",
            snap.generation,
            state.scene_state.revision,
            input.tri_count(),
            input.materials.len(),
            input.images.len(),
            self.uploads,
            lo,
            hi,
            cam.eye,
            cam.target,
            cam.fov_y_deg,
            sun.dir,
            sun.dir.z.asin().to_degrees(),
            input.materials.first()
        );
        true
    }

    /// Shared body of the viewport preview and the F12 render.
    fn draw_with(
        &mut self,
        cx: &mut Cx2d,
        state: &mut AppState,
        view: usize,
        size: (usize, usize),
        aspect: f32,
        parent_pass: Option<DrawPassId>,
        preview: bool,
    ) -> Option<RenderedFrame> {
        if size.0 < 2 || size.1 < 2 || state.snapshot.is_none() || view >= state.views.len() {
            return None;
        }
        if !self.ensure_tracer(cx) {
            return None;
        }
        let uploaded = self.sync_scene(cx, state);
        // ---- headless track rendering (env-driven) ---------------------
        if !self.track_probed {
            self.track_probed = true;
            if view == 1 {
                self.track = TrackJob::from_env();
            }
        }
        let mut size = size;
        let mut aspect = aspect;
        let mut preview = preview;
        let mut job_restart = false;
        if let Some(job) = self.track.as_mut() {
            // Skip frames that already exist (resumable), stop past the range.
            job.skip_existing();
            if job.finished() {
                let n = job.rendered;
                let secs = job.started_at.elapsed().as_secs_f64();
                job.finish();
                log!("track: done — {} frames rendered in {:.1}s ({:.2}s/frame); quitting", n, secs, secs / n.max(1) as f64);
                self.track = None;
                cx.cx.cx.quit();
                return None;
            }
            job_restart = job.begin_key();
            // Pin the camera every draw so a navigator Frame tick cannot
            // restart accumulation mid-key.
            state.views[view].camera = job.camera(job.index);
            size = job.size;
            aspect = job.size.0 as f32 / job.size.1 as f32;
            preview = false;
        }
        let now = cx.time();
        let cam = state.views[view].camera;
        let vp = cam.view_projection(aspect);
        let sun = sun_from_api(&state.sun);
        let mut settings = settings_from_api(&state.render, &state.sun, preview);
        let job_active = self.track.is_some();
        if let Some(job) = self.track.as_ref() {
            settings.target_spp = job.spp;
            settings.denoise = job.denoise;
            settings.adaptive_min = 0;
            // A headless frame may request more throughput; the tracer's
            // command-buffer cap remains authoritative.
            settings.frame_budget = 0.25;
        } else if preview {
            // Interactive preview has one fixed GPU-time target per host
            // frame: the tracer's 4 ms draw budget below. An infinite frame
            // budget hands control to that single knob (and to
            // MAKEPAD_PT_BUDGET_MS), instead of a second, smaller cap.
            settings.frame_budget = f64::INFINITY;
        }
        let dirty = std::mem::replace(&mut state.views[view].render_dirty, false);
        let t = self.tracer.as_mut().unwrap();
        t.set_camera(camera_from_api(&cam));
        t.set_view_projection(Some(vp));
        if self.last_sun.as_ref() != Some(&state.sun) {
            self.last_sun = Some(state.sun);
            t.set_sky(makepad_raytrace::sky::sky_uniforms_at_time(
                &sun,
                vec3(0.0, 0.0, 1.0),
                state.sun.time_local,
                state.sun.latitude,
            )
            .with_exposure_ev(state.sun.exposure_ev));
        }
        t.set_settings(settings);
        // 4 ms of measured GPU time per host frame, everywhere: at 120 Hz
        // that is under half the frame, so the compositor and the realtime
        // panes keep their time and the machine stays responsive, while the
        // trace converges ~7x faster than the old 1.5 ms budget with a
        // half-duty skip (measured: 0 spp vs 5 spp after 10 s).
        t.set_draw_budget_ms(4.0);
        t.set_parent_pass(parent_pass);
        t.set_size(size.0, size.1);
        // A track job restarts only when the camera changes at a key
        // boundary (or the scene was just uploaded). Interactive dirty
        // bits from the navigator must not reset accumulation. While the
        // camera is moving, skip `t.draw` entirely (quiet-timer hold) so
        // a lone centre tile is never blitted over a stale rest. F12
        // (`preview = false`) and track jobs bypass the hold.
        if job_active {
            t.set_paused(false);
            t.set_skip_trace(false);
        } else {
            // The 4 ms measured budget above IS the interactive law: it
            // bounds GPU occupancy per host frame directly, so the old
            // half-duty `skip_trace` cycle (which only halved convergence
            // on top of an already-tiny budget) is gone. Unfocused or
            // stopped still trace nothing.
            t.set_paused(self.paused || !self.focused || !self.active);
            t.set_skip_trace(false);
        }
        if !job_active && preview && (self.paused || !self.focused || !self.active) {
            // Latch invalidation for Resume, but submit no command buffer at
            // all while stopped, unfocused, or no longer the active shading.
            if dirty || uploaded {
                t.restart();
            }
            return t.view_texture().cloned().map(|texture| RenderedFrame {
                texture,
                converging: false,
                done: t.stats.done,
                samples_done: t.stats.spp as u32,
                stage_shift: t.stats.rung_shift,
            });
        }
        if job_active {
            let _ = self.hold.begin_frame(now, false, true);
            if job_restart || uploaded {
                t.restart();
            }
            t.draw(cx);
        } else if !preview {
            if dirty || uploaded {
                t.restart();
            }
            t.draw(cx);
        } else {
            match self.hold.begin_frame(now, dirty, false) {
                MotionHoldAction::Hold => {
                    // Latch restart so RayTracer::wants_frame keeps the
                    // paint clock alive through the quiet window. The pane
                    // shows the realtime composite alone: the accumulation
                    // still holds the previous camera until the restart
                    // frame clears it.
                    if dirty || uploaded {
                        t.restart();
                    }
                    let _ = cx.new_next_frame();
                    return None;
                }
                MotionHoldAction::Restart => {
                    // The restart draw resets the accumulation, so every
                    // untraced pixel is transparent from this very frame:
                    // present immediately — fresh tiles composite over the
                    // realtime raster underneath.
                    t.restart();
                    t.draw(cx);
                }
                MotionHoldAction::Draw => {
                    if uploaded {
                        t.restart();
                    }
                    t.draw(cx);
                }
            }
        }
        if let Some(job) = self.track.as_mut() {
            log!(
                "track: accumulating {}/{} spp {:.3} frames {} done {} {}x{}",
                job.index + 1, job.range.1, t.stats.spp, t.stats.frames, t.stats.done,
                t.stats.width, t.stats.height
            );
            if t.stats.done && !job.capture_requested {
                job.capture_requested = true;
                t.request_capture(makepad_raytrace::gpu::CaptureKind::View);
            }
            t.poll_capture(cx.cx.cx);
            for c in t.take_captures() {
                if c.kind != makepad_raytrace::gpu::CaptureKind::View {
                    continue;
                }
                if let Some(metrics) = c.display_metrics() {
                    log!(
                        "track: frame {} metrics mean={:.3}/255 above_8={:.3}% fireflies={:.3}%",
                        job.index,
                        metrics.mean_luminance * 255.0,
                        metrics.above_eight_fraction * 100.0,
                        metrics.firefly_ratio * 100.0,
                    );
                }
                let path = job.frame_path(job.index);
                if let Err(e) = makepad_raytrace::png::write_bgra8(&path, c.width, c.height, &c.bytes) {
                    log!("track: png failed {}: {e}", path.display());
                }
                job.mp4_push(c.width, c.height, &c.bytes);
                let elapsed = job.started_at.elapsed().as_secs_f64();
                let n_done = job.rendered + 1;
                let per = elapsed / n_done as f64;
                let left = (job.range.1 - job.index - 1) as f64 * per;
                log!(
                    "track: frame {}/{} {} spp in {:.2}s ({:.2} Mpaths/s); {:.1}s elapsed, ~{:.0}s left -> {}",
                    job.index + 1, job.range.1, t.stats.spp as u32, job.frame_started_at.elapsed().as_secs_f64(),
                    t.stats.samples_per_sec / 1.0e6, elapsed, left, path.display()
                );
                job.complete_key();
            }
            // Drive the next tick ourselves: the caller's `converging` /
            // `wants_frame` path only rearms while the *current* key is
            // still accumulating, and `area.redraw` can no-op. A job must
            // keep the paint clock and a full redraw alive through capture
            // and the gap between keys.
            cx.redraw_all();
            let _ = cx.new_next_frame();
        }
        if t.stats.frames % 120 == 1 {
            // Seam diagnostics: does the CPU twin see the scene from this camera?
            let (w, h) = (t.stats.width as f32, t.stats.height as f32);
            let centre = t.focus_distance_at(w * 0.5, h * 0.5);
            let (ro, rd) = t.pixel_ray(w * 0.5, h * 0.5);
            log!(
                "render: seam frame {} rung 1/{} spp {:.3} tile {} x{} host {:.1} ms gpu {:.2}/{:.2} ms ({} samples); centre ray {:?} -> {:?} hits at {:?}; sky sun_rad {:?} zenith {:?} up {:?}",
                t.stats.frames, 1u32 << t.stats.rung_shift, t.stats.spp, t.stats.tile_edge, t.stats.tiles, t.stats.last_frame_ms,
                t.stats.gpu_time_ms, t.stats.gpu_budget_ms, t.stats.gpu_samples, ro, rd, centre,
                t.sky().sun_radiance, t.sky().zenith, t.sky().up
            );
        }
        let texture = t.view_texture()?.clone();
        Some(RenderedFrame {
            texture,
            // Stay "converging" for the whole job so the caller's NextFrame
            // rearm (which keys off this flag) does not drop between keys
            // or while a capture is in flight.
            converging: !t.stats.done || self.track.is_some(),
            done: t.stats.done,
            samples_done: t.stats.spp as u32,
            stage_shift: t.stats.rung_shift,
        })
    }

    pub fn tracer(&self) -> Option<&RayTracer> {
        self.tracer.as_ref()
    }

    pub fn tracer_mut(&mut self) -> Option<&mut RayTracer> {
        self.tracer.as_mut()
    }
}

impl RenderedPreview {
    /// The seam call with the tracer's passes parented under the WINDOW
    /// pass (whatever pass is current when this is called) instead of a
    /// viewport pass — the host the tracer was verified in. Same camera
    /// parity, same `render_dirty` ownership.
    pub fn draw_under_current_pass(
        &mut self,
        cx: &mut Cx2d,
        state: &mut AppState,
        view: usize,
        rect: Rect,
    ) -> Option<RenderedFrame> {
        self.set_active(true);
        let aspect = (rect.size.x / rect.size.y.max(1.0)) as f32;
        let dpi = cx.current_dpi_factor();
        let scale = state.render.preview_scale.clamp(0.25, 1.0) as f64;
        let size = ((rect.size.x * dpi * scale) as usize, (rect.size.y * dpi * scale) as usize);
        self.draw_with(cx, state, view, size, aspect, None, true)
    }
}

impl RenderedPreviewApi for RenderedPreview {
    fn draw(
        &mut self,
        cx: &mut Cx2d,
        state: &mut AppState,
        view: usize,
        rect: Rect,
        parent_pass: &DrawPass,
    ) -> Option<RenderedFrame> {
        self.set_active(true);
        let aspect = (rect.size.x / rect.size.y.max(1.0)) as f32;
        let dpi = cx.current_dpi_factor();
        let scale = state.render.preview_scale.clamp(0.25, 1.0) as f64;
        let size = ((rect.size.x * dpi * scale) as usize, (rect.size.y * dpi * scale) as usize);
        self.draw_with(cx, state, view, size, aspect, Some(parent_pass.draw_pass_id()), true)
    }

    fn sky_params(&self) -> Option<makepad_raytrace::sky::SkyUniforms> {
        self.tracer.as_ref().map(|tracer| *tracer.sky())
    }
}

/// F12: the full-size render area. Renders the ACTIVE viewport's camera at
/// `RenderSettings::width × height`, shows progress, saves on `ExportPng`.
#[derive(Script, ScriptHook, Widget)]
pub struct FabRenderView {
    #[deref]
    view: View,
    #[rust]
    preview: RenderedPreview,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    pending_export: Option<PathBuf>,
    #[rust]
    last_reported: u32,
    #[rust]
    started: bool,
}

impl Widget for FabRenderView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            for a in actions.iter() {
                match a.downcast_ref::<ShellAction>() {
                    Some(ShellAction::ExportPng(path)) => {
                        self.pending_export = Some(path.clone());
                        if let Some(t) = self.preview.tracer_mut() {
                            t.request_capture(makepad_raytrace::gpu::CaptureKind::View);
                        }
                        self.view.redraw(cx);
                    }
                    Some(ShellAction::RenderStart) => {
                        self.started = true;
                        self.last_reported = u32::MAX;
                        self.next_frame = cx.new_next_frame();
                        self.view.redraw(cx);
                    }
                    _ => {}
                }
            }
        }
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                if let Some(t) = self.preview.tracer_mut() {
                    t.poll_capture(cx);
                    let caps = t.take_captures();
                    for c in caps {
                        if let Some(path) = self.pending_export.take() {
                            match makepad_raytrace::png::write_bgra8(&path, c.width, c.height, &c.bytes) {
                                Ok(()) => log!("render: saved {}", path.display()),
                                Err(e) => log!("render: save failed: {e}"),
                            }
                        }
                    }
                    if t.wants_frame() || self.pending_export.is_some() {
                        self.next_frame = cx.new_next_frame();
                    }
                }
                self.view.redraw(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let Some(state) = scope.data.get_mut::<AppState>() else {
            return self.view.draw_walk(cx, scope, walk);
        };
        let r = state.render;
        let running = r.running || self.pending_export.is_some();
        let text = if running {
            format!("Rendering… {}/{} spp, {:.1}s", r.samples_done, r.max_samples, r.elapsed_s)
        } else {
            format!("Render — {}×{}, {} spp max. F12 to start.", r.width, r.height, r.max_samples)
        };
        self.view.label(cx, ids!(status)).set_text(cx, &text);
        if !running {
            return self.view.draw_walk(cx, scope, walk);
        }
        // Full-size render of the active view's camera, letterboxed here.
        cx.begin_turtle(walk, Layout::flow_overlay());
        let rect = cx.turtle().rect();
        let (w, h) = (r.width.max(2) as usize, r.height.max(2) as usize);
        let aspect = w as f32 / h as f32;
        let view = state.active_view.min(state.views.len().saturating_sub(1));
        let frame = self.preview.draw_with(cx, state, view, (w, h), aspect, None, false);
        if let Some(frame) = frame {
            let ar = aspect as f64;
            let wr = rect.size.x / rect.size.y.max(1.0);
            let dst = if wr > ar {
                let nw = rect.size.y * ar;
                Rect { pos: dvec2(rect.pos.x + (rect.size.x - nw) * 0.5, rect.pos.y), size: dvec2(nw, rect.size.y) }
            } else {
                let nh = rect.size.x / ar;
                Rect { pos: dvec2(rect.pos.x, rect.pos.y + (rect.size.y - nh) * 0.5), size: dvec2(rect.size.x, nh) }
            };
            if let Some(t) = self.preview.tracer_mut() {
                t.draw_view.draw_super.draw_vars.set_texture(0, &frame.texture);
                t.draw_view.draw_abs(cx, dst);
                let elapsed = t.stats.elapsed as f32;
                if frame.samples_done != self.last_reported {
                    self.last_reported = frame.samples_done;
                    cx.action(ShellAction::RenderProgress { samples: frame.samples_done, elapsed_s: elapsed });
                }
                if !frame.converging && r.running {
                    cx.action(ShellAction::RenderFinished);
                }
            }
            if frame.converging {
                self.next_frame = cx.new_next_frame();
            }
        } else {
            self.next_frame = cx.new_next_frame();
        }
        cx.end_turtle();
        DrawStep::done()
    }
}

/// Lane F's action hook, called from `App::dispatch`.
pub fn apply(cx: &mut Cx, state: &mut AppState, action: &ShellAction) -> bool {
    // A track render needs the right pane in Rendered shading; flip it as
    // soon as a scene is loaded (the seam only runs for Rendered panes).
    if std::env::var_os("FAB_RENDER_TRACK").is_some()
        && state.snapshot.is_some()
        && state.views.len() > 1
        && state.views[1].shading != Shading::Rendered
    {
        cx.action(ShellAction::SetShading(1, Shading::Rendered));
    }
    match action {
        ShellAction::RenderStart => {
            state.render.running = true;
            state.render.samples_done = 0;
            state.render.elapsed_s = 0.0;
            state.mark_render_dirty();
            true
        }
        ShellAction::RenderStop | ShellAction::RenderFinished => {
            state.render.running = false;
            true
        }
        ShellAction::RenderProgress { samples, elapsed_s } => {
            state.render.samples_done = *samples;
            state.render.elapsed_s = *elapsed_s;
            true
        }
        ShellAction::ClickToFocus(view, point) => {
            let vs = state.view_at_mut(*view);
            vs.camera.focus_distance = (*point - vs.camera.eye).length();
            vs.render_dirty = true;
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Framing parity: the tracer's ray generator and the api camera's
    /// view-projection must put the same world point on the same pixel.
    #[test]
    fn ray_generator_matches_api_view_projection() {
        let cam = Camera::default();
        let (w, h) = (1600usize, 1000usize);
        let aspect = w as f32 / h as f32;
        let vp = cam.view_projection(aspect);
        let pt = camera_from_api(&cam);
        let (right, up, fwd) = pt.basis();
        let tan_y = (pt.fov_y * 0.5).tan();
        // A handful of world points in front of the camera.
        for (i, p) in [vec3(5.0, 3.5, 2.5), vec3(9.0, 1.0, 4.0), vec3(2.0, 6.0, 0.5), vec3(7.5, 2.0, 8.0)].iter().enumerate() {
            let c = vp.transform_vec4(vec4(p.x, p.y, p.z, 1.0));
            let ndc = vec2(c.x / c.w, c.y / c.w);
            let px = ((ndc.x + 1.0) * 0.5 * w as f32, (1.0 - ndc.y) * 0.5 * h as f32);
            // The tracer's ray through that pixel must pass through p.
            let rx = (px.0 / w as f32) * 2.0 - 1.0;
            let ry = 1.0 - (px.1 / h as f32) * 2.0;
            let rd = (fwd + right * (rx * tan_y * aspect) + up * (ry * tan_y)).normalize();
            let to_p = (*p - pt.pos).normalize();
            let err = (rd - to_p).length();
            assert!(err < 2.0e-5, "point {i}: ray/projection mismatch {err}");
        }
    }

    #[test]
    fn api_lens_maps_to_a_pinhole_at_f0() {
        let mut cam = Camera::default();
        cam.f_stop = 0.0;
        assert_eq!(camera_from_api(&cam).lens_radius(), 0.0);
        cam.f_stop = 2.0;
        assert!(camera_from_api(&cam).lens_radius() > 0.05);
    }

    #[test]
    fn adaptive_sampling_is_preview_only() {
        let settings = RenderSettings::default();
        assert_eq!(settings_from_api(&settings, &SkyState::default(), true).adaptive_min, 64);
        assert_eq!(settings_from_api(&settings, &SkyState::default(), false).adaptive_min, 0);
    }

    #[test]
    fn rendered_preview_pause_state_survives_deactivation() {
        let mut preview = RenderedPreview::default();
        preview.set_active(true);
        preview.set_paused(true);
        preview.set_active(false);
        preview.set_active(true);
        assert!(preview.paused());
        assert!(!preview.wants_frame());
    }

    fn dummy_key() -> TrackKey {
        TrackKey {
            t: 0.0,
            pos: [0.0, 0.0, 0.0],
            look_at: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            fov_y_deg: 40.0,
        }
    }

    fn test_job(n_keys: usize, range: (usize, usize)) -> TrackJob {
        TrackJob {
            keys: (0..n_keys).map(|_| dummy_key()).collect(),
            fps: 24.0,
            out: PathBuf::from("local/fab/renders/track-test"),
            spp: 4,
            size: (320, 180),
            range,
            denoise: false,
            index: range.0,
            frame_started: false,
            capture_requested: false,
            started_at: std::time::Instant::now(),
            frame_started_at: std::time::Instant::now(),
            rendered: 0,
            mp4: None,
            mp4_path: None,
        }
    }

    /// Index / skip / finish, and `begin_key` restarts only at a key boundary.
    #[test]
    fn track_job_state_machine_skips_finishes_and_restarts_only_on_key_change() {
        let mut job = test_job(4, (0, 4));

        job.skip_existing_if(|_| false);
        assert_eq!(job.index, 0);
        assert!(!job.finished());

        assert!(job.begin_key(), "first draw of a key must restart");
        assert!(job.frame_started);
        assert!(!job.begin_key(), "same key must not restart");
        assert!(!job.begin_key());
        job.skip_existing_if(|_| true);
        assert_eq!(job.index, 0, "must not skip while a key is in flight");

        job.complete_key();
        assert_eq!(job.index, 1);
        assert_eq!(job.rendered, 1);
        assert!(!job.frame_started);

        assert!(job.begin_key(), "next key must restart");
        job.complete_key();
        assert_eq!(job.index, 2);

        job.skip_existing_if(|i| i >= 2);
        assert_eq!(job.index, 4);
        assert!(job.finished());
        assert!(!job.begin_key());
    }

    #[test]
    fn track_job_runs_index_to_finish_restarting_only_at_keys() {
        let mut job = test_job(5, (1, 5));
        assert_eq!(job.index, 1);

        let mut restarts = 0;
        let mut keys = Vec::new();
        loop {
            job.skip_existing_if(|i| i == 3);
            if job.finished() {
                break;
            }
            if job.begin_key() {
                restarts += 1;
                keys.push(job.index);
            }
            assert!(!job.begin_key());
            assert!(!job.begin_key());
            job.complete_key();
        }
        assert_eq!(keys, vec![1, 2, 4], "frame 3 skipped, others rendered");
        assert_eq!(restarts, 3);
        assert_eq!(job.rendered, 3);
        assert!(job.finished());
    }

    #[test]
    fn motion_hold_skips_trace_until_quiet_then_presents_from_the_restart() {
        let mut h = MotionHold::new();
        assert_eq!(h.motion_hold_ms, DEFAULT_MOTION_HOLD_MS);

        // No dirty: draw and present immediately (nothing to hold).
        assert_eq!(h.begin_frame(0.0, false, false), MotionHoldAction::Draw);
        assert!(!h.showing_placeholder());

        // Orbit: dirty every frame, never restart, raster alone on screen.
        for i in 0..10 {
            let t = 1.0 + i as f64 * 0.016;
            assert_eq!(h.begin_frame(t, true, false), MotionHoldAction::Hold, "moving frame {i}");
            assert!(h.showing_placeholder());
            assert!(h.showing_moving());
        }
        let t0 = 1.0 + 9.0 * 0.016;

        // Quiet 149 ms: still holding. 151 ms: restart the spiral — and
        // present from that same frame: the restart draw clears the
        // accumulation, untraced pixels are transparent, and the fresh
        // tiles composite over the realtime raster underneath.
        // (Avoid exact 0.150: binary f64 of 150 ms is not always 150.0.)
        assert_eq!(h.begin_frame(t0 + 0.149, false, false), MotionHoldAction::Hold);
        assert!(h.showing_moving());
        assert_eq!(h.begin_frame(t0 + 0.151, false, false), MotionHoldAction::Restart);
        assert!(!h.showing_moving());
        assert!(!h.showing_placeholder());

        // Subsequent quiet frames keep drawing.
        assert_eq!(h.begin_frame(t0 + 0.166, false, false), MotionHoldAction::Draw);
        assert!(!h.showing_placeholder());

        // Another orbit holds again, then restarts once quiet.
        assert_eq!(h.begin_frame(t0 + 1.0, true, false), MotionHoldAction::Hold);
        assert!(h.showing_moving());
        assert_eq!(h.begin_frame(t0 + 1.151, false, false), MotionHoldAction::Restart);
        assert!(!h.showing_moving());
        assert!(!h.showing_placeholder());
    }

    #[test]
    fn motion_hold_bypassed_for_track_job() {
        let mut h = MotionHold::new();
        assert_eq!(h.begin_frame(0.0, true, true), MotionHoldAction::Draw);
        assert!(!h.showing_placeholder());
        assert!(!h.showing_moving());
        // Dirty every frame still draws; hold must not delay a track.
        assert_eq!(h.begin_frame(0.016, true, true), MotionHoldAction::Draw);
        assert!(!h.showing_placeholder());
        assert!(!h.showing_moving());
    }

    #[test]
    fn motion_hold_dirty_on_expiry_frame_stays_holding() {
        let mut h = MotionHold::new();
        assert_eq!(h.begin_frame(0.0, true, false), MotionHoldAction::Hold);
        assert_eq!(h.begin_frame(0.150, true, false), MotionHoldAction::Hold);
        assert_eq!(h.begin_frame(0.301, false, false), MotionHoldAction::Restart);
    }
}
