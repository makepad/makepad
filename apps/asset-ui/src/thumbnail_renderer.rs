//! Headless History-preview renderer.
//!
//! Why this widget exists: PageFlip draws only its ACTIVE page, so any
//! offscreen work owned by the mesh viewer starves the moment another viewer
//! or surface is in front (proven live: a queued GLB preview sat unrendered
//! for minutes). This widget is hosted OUTSIDE every PageFlip in the root
//! window chrome, occupies one out-of-flow pixel, composites nothing, and
//! therefore draws — and makes queue progress — on every frame the window
//! produces, whatever page the user is on.
//!
//! Contract:
//! - SERIALIZED: one job renders at a time into one fixed 512² target; the
//!   readback blit is unique. The app's pump bounds how many jobs may wait
//!   here, so a growing library can never pile GLB byte-copies into RAM.
//! - A MESH ALWAYS PRODUCES AN ICON: Kenney/material GLBs use the same PBR
//!   studio path as MeshView; baked aomeshes use the game lane. The camera
//!   is a fixed studio looking at a ground-fitted subject. Clear-color
//!   readback means the GPU has not delivered the mesh yet — wait, do not
//!   label the asset blank, do not drop the job.
//! - FRAMING: every subject is scaled by its largest extent onto the same
//!   studio box, feet on y=0. One camera frames every card.

use crate::mesh_view::pbr_preview::{parse_material_bearing_glb, PbrPreview};
use crate::mesh_view::{extract_base_color, image_texture, is_playable_skin};
use makepad_render::play::LocoState;
use makepad_render::skin::{PoseBuffer, SkinnedModel};
use makepad_render::{
    preview_scene_state, set_pass_camera, DrawSceneAlpha, DrawSceneCube,
    DrawSceneSkinned, DrawSceneSkinnedGpu, DrawSceneSky, DrawSceneTerrain, DrawSceneTexture, SceneDraws,
    Renderer, ModelInstance, PreviewLook, PreviewStage, SkinnedBatch, SkinnedDraw,
};
use makepad_widgets::*;
use std::collections::VecDeque;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ThumbnailRendererBase = #(ThumbnailRenderer::register_widget(vm))
    mod.widgets.ThumbnailRenderer = set_type_default() do mod.widgets.ThumbnailRendererBase{
        // 1px host only schedules the widget. The child pass is Size(512)
        // with dpi_override=1 so begin_pass does not inherit the window rect.
        width: 1
        height: 1
        draw_cube +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_alpha +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_terrain +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_models +: { light_dir: vec3(0.35, 0.8, 0.45) }
        draw_skinned +: { light_dir: vec3(0.35, 0.8, 0.45) }
    }
}

/// UI icons: 256² is enough; 512² was 4× the encode/readback for no gain.
pub const THUMBNAIL_SIZE: usize = 256;
/// Fitted longest edge. Camera then sits outside the bounding sphere.
const THUMBNAIL_SUBJECT_EXTENT: f32 = 1.6;
const THUMBNAIL_FOV_DEG: f32 = 32.0;
const THUMBNAIL_FRAME_PAD: f32 = 1.20;
/// Kenney characters/cars face −Z. Camera already sits 180° around so we
/// see that face (one-sided fronts). Extra π on the model turns the 3/4
/// from top-left to bottom-right without flipping to backfaces.
const THUMBNAIL_MODEL_YAW: f32 = 0.35 + std::f32::consts::PI;
const THUMBNAIL_CAM_YAW: f32 = 0.62 + std::f32::consts::PI;
const THUMBNAIL_CAM_PITCH: f32 = -0.28;
const THUMBNAIL_MIN_AXIS: f32 = 0.02;
const THUMBNAIL_MAX_READBACK_ATTEMPTS: u8 = 2;
/// One draw, read back next frame — one icon per display frame.
const THUMBNAIL_WARMUP_FRAMES: u8 = 1;
const THUMBNAIL_GPU_WAIT_FRAMES: u8 = 1;
const THUMBNAIL_MAX_GPU_WAITS: u8 = 1;
const THUMBNAIL_PBR_TEXTURE_WAIT_FRAMES: u8 = 4;
/// Per-channel tolerance when deciding a pixel still shows the clear color.
const CLEAR_CHANNEL_TOLERANCE: u8 = 8;
/// A fitted mesh covers far more than this. Used only to detect "GPU has
/// not delivered the subject yet" — never to drop or label an icon.
const CLEAR_MIN_SUBJECT_FRACTION: f64 = 0.002;
/// Cool studio, darker than Kenney plastic, lighter than a black card.
const THUMBNAIL_CLEAR: Vec4f = vec4f(0.10, 0.12, 0.16, 1.0);

#[derive(Clone)]
pub(crate) struct ThumbnailJob {
    pub(crate) file: String,
    pub(crate) glb: Vec<u8>,
    pub(crate) aomesh: Option<Vec<u8>>,
    pub(crate) ao_png: Option<Vec<u8>>,
    /// World first-person camera (engine metres / radians).
    pub(crate) spawn: Option<([f32; 3], f32, f32)>,
}

/// Pack the captured views into ONE picture that declares itself: a 4x4
/// sheet of 256² cells, stamped with the cell layout and the rate, so the
/// grid and the preview well rotate the card with no code that knows what a
/// turntable is. The first cell is the canonical front, which is what a
/// still context draws.
fn pack_turntable(tiles: &[Vec<u8>]) -> Option<Vec<u8>> {
    use makepad_asset_importer::anim_icon;
    if tiles.is_empty() {
        return None;
    }
    let sheet = anim_icon::pack_grid(tiles, THUMBNAIL_SIZE, TURNTABLE_COLS)
        .map_err(|error| log!("turntable: pack failed: {error}"))
        .ok()?;
    Some(anim_icon::stamp_layout(&sheet.png, sheet.cells(), TURNTABLE_FPS))
}

/// A completed model-only PNG. The app commits it through `Library`, which
/// revalidates the stable payload id so deleted resources stay deleted.
pub struct RenderedThumbnail {
    pub file: String,
    pub png: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Pure queue state (unit-tested): serialization + dedupe are page-independent
// ---------------------------------------------------------------------------

/// One-active-job queue keyed by stable library file id. Pure state so the
/// scheduling contract (dedupe against waiting AND active work, strict
/// serialization, honest pending count) is testable without a GPU or a
/// window — the widget only feeds it draw frames.
#[derive(Default)]
pub(crate) struct ThumbQueue {
    jobs: VecDeque<ThumbnailJob>,
    active_file: Option<String>,
}

impl ThumbQueue {
    /// False when the file is already waiting or rendering.
    pub(crate) fn enqueue(&mut self, file: String, glb: Vec<u8>) -> bool {
        self.enqueue_job(ThumbnailJob {
            file,
            glb,
            aomesh: None,
            ao_png: None,
            spawn: None,
        })
    }

    pub(crate) fn enqueue_job(&mut self, job: ThumbnailJob) -> bool {
        if self.active_file.as_deref() == Some(job.file.as_str())
            || self.jobs.iter().any(|queued| queued.file == job.file)
        {
            return false;
        }
        self.jobs.push_back(job);
        true
    }

    pub(crate) fn clear(&mut self) {
        self.jobs.clear();
        self.active_file = None;
    }

    /// Hand out the next job only while nothing is active.
    pub(crate) fn take_next(&mut self) -> Option<ThumbnailJob> {
        if self.active_file.is_some() {
            return None;
        }
        let job = self.jobs.pop_front()?;
        self.active_file = Some(job.file.clone());
        Some(job)
    }

    pub(crate) fn finish_active(&mut self) {
        self.active_file = None;
    }

    pub(crate) fn pending_len(&self) -> usize {
        self.jobs.len() + usize::from(self.active_file.is_some())
    }
}

// ---------------------------------------------------------------------------
// Capture analysis (unit-tested)
// ---------------------------------------------------------------------------

/// True when an RGBA capture is still the studio clear color. That means
/// the GPU has not delivered the mesh yet — wait and shoot the same frame
/// again. A loaded mesh is never "blank".
pub(crate) fn capture_is_clear_only(rgba: &[u8], clear: Vec4f) -> bool {
    if rgba.len() < 4 {
        return true;
    }
    let clear_bytes = [
        (clear.x * 255.0).round() as i16,
        (clear.y * 255.0).round() as i16,
        (clear.z * 255.0).round() as i16,
    ];
    let tolerance = CLEAR_CHANNEL_TOLERANCE as i16;
    let mut subject = 0usize;
    let total = rgba.len() / 4;
    for pixel in rgba.chunks_exact(4) {
        let differs = pixel[..3]
            .iter()
            .zip(clear_bytes)
            .any(|(&channel, clear)| (channel as i16 - clear).abs() > tolerance);
        if differs {
            subject += 1;
        }
    }
    (subject as f64) < (total as f64) * CLEAR_MIN_SUBJECT_FRACTION
}

/// Per-model studio: AABB centered on the origin, slight presentation yaw,
/// camera 180° around so we see the face that used to be the rear.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ThumbnailFrame {
    pub transform: Mat4f,
    pub cam_target: Vec3f,
    pub cam_distance: f32,
    pub cam_fov: f32,
    pub yaw: f32,
    pub pitch: f32,
}

/// `R * S * T(-center)` so the AABB centre stays at the origin after yaw.
/// `T(-c) * R * S` (the old order) rotated pack-grid Kenney models around
/// their authored origin and parked them in a corner of the card.
fn thumbnail_center_then_yaw(center: Vec3f, yaw: f32, scale: f32) -> Mat4f {
    let mut local = Mat4f::identity();
    local.v[0] = scale;
    local.v[5] = scale;
    local.v[10] = scale;
    local.v[12] = -center.x * scale;
    local.v[13] = -center.y * scale;
    local.v[14] = -center.z * scale;
    Mat4f::mul(&Mat4f::rotation(vec3f(0.0, yaw, 0.0)), &local)
}

fn aabb_corners(min: Vec3f, max: Vec3f) -> [Vec3f; 8] {
    let mut out = [Vec3f::default(); 8];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = vec3f(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        );
    }
    out
}

/// Largest |NDC x/y| of `world` corners under the same orbit camera the
/// thumbnail pass uses. Infinite if a corner is behind the near plane.
fn projected_span(world: &[Vec3f], distance: f32, yaw: f32, pitch: f32, fov: f32) -> f32 {
    let pitch = pitch.clamp(-1.45, 1.45);
    let forward = vec3f(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    )
    .normalize();
    let eye = vec3f(0.0, 0.0, 0.0) - forward * distance.max(0.5);
    let view = Mat4f::look_at(eye, vec3f(0.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0));
    let proj = Mat4f::perspective(fov.clamp(20.0, 120.0), 1.0, 0.15, 500.0);
    let clip = Mat4f::mul(&proj, &view);
    let mut span = 0.0f32;
    for &p in world {
        let c = clip.transform_vec4(vec4(p.x, p.y, p.z, 1.0));
        if c.w.abs() < 1.0e-5 {
            return f32::INFINITY;
        }
        span = span.max((c.x / c.w).abs()).max((c.y / c.w).abs());
    }
    span
}

fn sanitize_bounds(min: Vec3f, max: Vec3f) -> (Vec3f, Vec3f) {
    let vals = [min.x, min.y, min.z, max.x, max.y, max.z];
    if vals.iter().any(|v| !v.is_finite()) {
        return (vec3f(-0.5, -0.5, -0.5), vec3f(0.5, 0.5, 0.5));
    }
    let mut lo = min;
    let mut hi = max;
    let center = (min + max) * 0.5;
    for axis in 0..3 {
        let (a, b) = match axis {
            0 => (&mut lo.x, &mut hi.x),
            1 => (&mut lo.y, &mut hi.y),
            _ => (&mut lo.z, &mut hi.z),
        };
        if *b - *a < THUMBNAIL_MIN_AXIS {
            let c = match axis {
                0 => center.x,
                1 => center.y,
                _ => center.z,
            };
            *a = c - THUMBNAIL_MIN_AXIS * 0.5;
            *b = c + THUMBNAIL_MIN_AXIS * 0.5;
        }
    }
    (lo, hi)
}

/// Always returns a camera. Degenerate bounds get a unit box so a loaded
/// mesh still has somewhere to stand.
pub(crate) fn thumbnail_frame_from_spawn(spawn: ([f32; 3], f32, f32)) -> ThumbnailFrame {
    let (pos, yaw, pitch) = spawn;
    let pitch = pitch.clamp(-1.2, 1.2);
    let forward = vec3f(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    );
    let look = 8.0f32;
    ThumbnailFrame {
        transform: Mat4f::identity(),
        cam_target: vec3f(
            pos[0] + forward.x * look,
            pos[1] + forward.y * look,
            pos[2] + forward.z * look,
        ),
        cam_distance: look,
        cam_fov: 72.0,
        yaw,
        pitch,
    }
}

/// Views a model icon carries: a full turn in sixteen steps of 22.5°, so a
/// card in the library ROTATES instead of showing one frozen three-quarter
/// angle. The first is the canonical front, which is what a still context —
/// an old reader, a card that is not animating — draws.
pub const TURNTABLE_STEPS: usize = 16;
/// Packed 4x4 at the icon size: a 1024² sheet, inside every budget.
pub const TURNTABLE_COLS: usize = 4;
/// A slow turn. Fast enough to read as motion, slow enough to look at.
pub const TURNTABLE_FPS: f32 = 9.0;

/// The sixteen views of a turntable, framed IDENTICALLY.
///
/// Each step is the same model at a different yaw, so each would fit the
/// square at its own distance — and a card whose subject grew and shrank as
/// it turned would look broken. Every step therefore uses the widest
/// distance any step needs, and the model turns inside a fixed frame.
pub(crate) fn turntable_frames(min: Vec3f, max: Vec3f) -> Vec<ThumbnailFrame> {
    let step = std::f32::consts::TAU / TURNTABLE_STEPS as f32;
    let mut frames: Vec<ThumbnailFrame> = (0..TURNTABLE_STEPS)
        .map(|i| thumbnail_frame_at(min, max, step * i as f32))
        .collect();
    let widest = frames
        .iter()
        .map(|f| f.cam_distance)
        .fold(0.0f32, f32::max);
    for frame in &mut frames {
        frame.cam_distance = widest;
    }
    frames
}

pub(crate) fn thumbnail_frame_from_bounds(min: Vec3f, max: Vec3f) -> ThumbnailFrame {
    thumbnail_frame_at(min, max, 0.0)
}

/// The canonical framing, with the model turned `extra_yaw` further.
fn thumbnail_frame_at(min: Vec3f, max: Vec3f, extra_yaw: f32) -> ThumbnailFrame {
    let (min, max) = sanitize_bounds(min, max);
    let size = max - min;
    let extent = size.x.max(size.y).max(size.z).max(THUMBNAIL_MIN_AXIS);
    let scale = THUMBNAIL_SUBJECT_EXTENT / extent;
    let center = (min + max) * 0.5;
    let transform = thumbnail_center_then_yaw(center, THUMBNAIL_MODEL_YAW + extra_yaw, scale);
    let mut world = [Vec3f::default(); 8];
    for (src, dst) in aabb_corners(min, max).iter().zip(world.iter_mut()) {
        let p = transform.transform_vec4(vec4(src.x, src.y, src.z, 1.0));
        *dst = vec3f(p.x, p.y, p.z);
    }
    let fitted = size * scale;
    let radius = (fitted.x * fitted.x + fitted.y * fitted.y + fitted.z * fitted.z).sqrt() * 0.5;
    let half = (THUMBNAIL_FOV_DEG.to_radians() * 0.5).tan();
    let mut cam_distance = if half > 1.0e-6 {
        (radius / half * THUMBNAIL_FRAME_PAD).max(1.15)
    } else {
        3.0
    };
    // Sphere estimate is loose for a 3/4 view; pull/push so the projected
    // AABB fills the same fraction of the square.
    const TARGET_NDC: f32 = 0.84;
    for _ in 0..3 {
        let span = projected_span(
            &world,
            cam_distance,
            THUMBNAIL_CAM_YAW,
            THUMBNAIL_CAM_PITCH,
            THUMBNAIL_FOV_DEG,
        );
        if !span.is_finite() || span < 1.0e-4 {
            cam_distance *= 1.25;
            continue;
        }
        cam_distance = (cam_distance * (span / TARGET_NDC)).clamp(1.15, 80.0);
    }
    ThumbnailFrame {
        transform,
        cam_target: vec3f(0.0, 0.0, 0.0),
        cam_distance,
        cam_fov: THUMBNAIL_FOV_DEG,
        yaw: THUMBNAIL_CAM_YAW,
        pitch: THUMBNAIL_CAM_PITCH,
    }
}

pub(crate) fn thumbnail_fit_transform(min: Vec3f, max: Vec3f) -> Option<Mat4f> {
    let values = [min.x, min.y, min.z, max.x, max.y, max.z];
    if values.iter().any(|value| !value.is_finite()) {
        return None;
    }
    let size = max - min;
    let extent = size.x.max(size.y).max(size.z);
    if !extent.is_finite() || extent <= 1.0e-6 {
        return None;
    }
    let center = (min + max) * 0.5;
    let scale = THUMBNAIL_SUBJECT_EXTENT / extent;
    let mut transform = Mat4f::identity();
    transform.v[0] = scale;
    transform.v[5] = scale;
    transform.v[10] = scale;
    transform.v[12] = -center.x * scale;
    transform.v[13] = -center.y * scale;
    transform.v[14] = -center.z * scale;
    Some(transform)
}

pub(crate) fn thumbnail_bgra_to_rgba(mut pixels: Vec<u8>) -> Option<Vec<u8>> {
    if pixels.len() != THUMBNAIL_SIZE * THUMBNAIL_SIZE * 4 {
        return None;
    }
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(pixels)
}

fn read_thumbnail_texture(cx: &mut Cx, texture: &Texture) -> Option<(usize, usize, Vec<u8>)> {
    // Metal / D3D11 / GL blit a staging copy back to the CPU. Web and
    // headless return None and the card keeps its domain badge.
    cx.debug_read_render_texture(texture)
}

/// Mesh only. A ground slab or sky becomes the whole 52px card.
fn thumbnail_look(frame: ThumbnailFrame) -> PreviewLook {
    PreviewLook {
        target: frame.cam_target,
        distance: frame.cam_distance,
        fov: frame.cam_fov,
        yaw: frame.yaw,
        pitch: frame.pitch,
    }
}

enum ThumbnailSubject {
    Statue(ModelInstance),
    Character {
        rig: u64,
        texture: Texture,
        palette: Vec<Mat4f>,
        bounds: Option<(Vec3f, Vec3f)>,
    },
    /// Material-bearing GLB. Fit + GPU meshes live on `ThumbnailRenderer.pbr`.
    Pbr,
}

struct ThumbnailActive {
    file: String,
    subject: ThumbnailSubject,
    frame: ThumbnailFrame,
    frames_drawn: u8,
    warmup_target: u8,
    rendered: bool,
    readback_attempts: u8,
    gpu_waits: u8,
    /// Remaining views of a turntable, and the ones already captured. Empty
    /// for a one-picture icon (a world's first-person view, which the walker
    /// preview animates instead).
    turntable: Vec<ThumbnailFrame>,
    step: usize,
    tiles: Vec<Vec<u8>>,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct ThumbnailRenderer {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_cube: DrawSceneCube,
    #[live]
    draw_alpha: DrawSceneAlpha,
    #[live]
    draw_sky: DrawSceneSky,
    #[live]
    draw_terrain: DrawSceneTerrain,
    /// Static-prop shader instance — used by the immediate DYNAMIC lane.
    #[live]
    draw_models: DrawSceneSkinned,
    /// GPU-skinned character shader (playable-chain artifacts).
    #[live]
    draw_skinned: DrawSceneSkinnedGpu,
    /// Same DrawPbr MeshView uses for Kenney / material-bearing GLBs.
    #[live]
    draw_pbr: DrawPbr,
    /// Composites the 512² child pass into the host so the GPU actually
    /// submits that pass (an unsamped render target stays at its clear).
    #[live]
    draw_bg: DrawSceneTexture,
    #[new]
    pass: DrawPass,
    /// The pass's main draw list. The scene renderer opens and CLOSES its
    /// own list, so anything drawn after it (the PBR hero) would otherwise
    /// land in the host window's list — outside this pass, invisible in the
    /// readback. Every draw of the pass nests inside this one.
    #[new]
    pass_list: DrawList,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust]
    renderer: Renderer,
    #[rust]
    queue: ThumbQueue,
    #[rust]
    active: Option<ThumbnailActive>,
    #[rust]
    rendered_thumbnails: Vec<RenderedThumbnail>,
    /// Jobs that could not load (unreadable GLB). A loaded mesh is never here.
    #[rust]
    rejected_thumbnails: Vec<String>,
    #[rust]
    pbr: PbrPreview,
    #[rust(0u64)]
    generation: u64,
}

impl ThumbnailRenderer {
    /// Queue a model-only preview for one immutable managed payload. Jobs
    /// are serialized and deduplicated by the library's stable file id; each
    /// carries its own GLB bytes, so a later pipeline stage cannot make an
    /// earlier card render the wrong model.
    pub fn queue_library_thumbnail(&mut self, cx: &mut Cx, file: String, glb: Vec<u8>) {
        self.queue_library_thumbnail_ao(cx, file, glb, None, None);
    }

    pub fn queue_library_thumbnail_ao(
        &mut self,
        cx: &mut Cx,
        file: String,
        glb: Vec<u8>,
        aomesh: Option<Vec<u8>>,
        ao_png: Option<Vec<u8>>,
    ) {
        self.queue_library_thumbnail_ao_spawn(cx, file, glb, aomesh, ao_png, None);
    }

    pub fn queue_library_thumbnail_ao_spawn(
        &mut self,
        cx: &mut Cx,
        file: String,
        glb: Vec<u8>,
        aomesh: Option<Vec<u8>>,
        ao_png: Option<Vec<u8>>,
        spawn: Option<([f32; 3], f32, f32)>,
    ) {
        if self.queue.enqueue_job(ThumbnailJob {
            file,
            glb,
            aomesh,
            ao_png,
            spawn,
        }) {
            self.area.redraw(cx);
        }
    }

    pub fn clear_thumbnail_queue(&mut self, cx: &mut Cx) {
        self.queue.clear();
        self.active = None;
        self.renderer = Renderer::default();
        self.pbr.clear(&mut self.draw_pbr);
        self.area.redraw(cx);
    }

    pub fn take_rendered_thumbnails(&mut self) -> Vec<RenderedThumbnail> {
        std::mem::take(&mut self.rendered_thumbnails)
    }

    pub fn take_rejected_thumbnails(&mut self) -> Vec<String> {
        std::mem::take(&mut self.rejected_thumbnails)
    }

    pub fn thumbnail_active_file(&self) -> Option<&str> {
        self.active.as_ref().map(|active| active.file.as_str())
    }

    /// Jobs queued or actively rendering. The app's backfill pump feeds new
    /// work only below its cap.
    pub fn thumbnail_pending_len(&self) -> usize {
        self.queue.pending_len()
    }

    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.rebuild_targets(cx);
    }

    fn rebuild_targets(&mut self, cx: &mut Cx) {
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed {
                    width: THUMBNAIL_SIZE,
                    height: THUMBNAIL_SIZE,
                },
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Fixed {
                    width: THUMBNAIL_SIZE,
                    height: THUMBNAIL_SIZE,
                },
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(THUMBNAIL_CLEAR),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;
    }

    fn prepare_next(&mut self, cx: &mut CxDraw) {
        while self.active.is_none() {
            let Some(job) = self.queue.take_next() else {
                return;
            };
            self.generation = self.generation.saturating_add(1);
            // Drop every previous thumbnail model/rig/PBR cache before
            // loading the next job; the visible viewer owns its own renderer.
            self.renderer = Renderer::default();
            self.pbr.clear(&mut self.draw_pbr);
            let base_color = extract_base_color(&job.glb);

            let prepared = self.prepare_subject(cx, &job, base_color);
            let Some((subject, frame, min, max)) = prepared else {
                self.rejected_thumbnails.push(job.file);
                self.queue.finish_active();
                continue;
            };
            log!(
                "thumbnail {}: studio extent=({:.2},{:.2},{:.2}) lane={} ao={}",
                job.file,
                max.x - min.x,
                max.y - min.y,
                max.z - min.z,
                match &subject {
                    ThumbnailSubject::Pbr => "pbr",
                    ThumbnailSubject::Statue(_) => "game",
                    ThumbnailSubject::Character { .. } => "skin",
                },
                job.ao_png.is_some()
            );

            // A world is shown by walking it, not by turning it; everything
            // else that is a mesh gets a full turn.
            let turntable = match job.spawn {
                Some(_) => Vec::new(),
                None => turntable_frames(min, max),
            };
            let frame = turntable.first().cloned().unwrap_or(frame);
            self.active = Some(ThumbnailActive {
                file: job.file,
                subject,
                frame,
                frames_drawn: 0,
                warmup_target: THUMBNAIL_WARMUP_FRAMES,
                rendered: false,
                readback_attempts: 0,
                gpu_waits: 0,
                turntable,
                step: 0,
                tiles: Vec::new(),
            });
        }
    }

    fn prepare_subject(
        &mut self,
        cx: &mut CxDraw,
        job: &ThumbnailJob,
        base_color: Option<Vec<u8>>,
    ) -> Option<(ThumbnailSubject, ThumbnailFrame, Vec3f, Vec3f)> {
        // Worlds: first-person at the player start. PBR AABB-fit looks at
        // the hull from outside (backfaces / studio gray).
        if job.spawn.is_some() {
            // Worlds share MeshView's Renderer load. Do not pass image 0
            // as a single atlas — that smears one tile over every surface.
            return self.prepare_statue(cx, job, None);
        }

        if let Ok(model) = SkinnedModel::parse_glb(&job.glb) {
            if is_playable_skin(&model) {
                let mut pose = PoseBuffer::new();
                if let Some(idle) = model.clip_index_any(LocoState::Idle.clip_candidates()) {
                    model.sample_clip(idle, 0.0, &mut pose);
                } else {
                    pose = model.rest_pose();
                }
                let mut palette = Vec::new();
                model.palette(&pose, &mut palette);
                let bounds = model.posed_bounds(&palette).unwrap_or((
                    vec3f(-0.5, 0.0, -0.5),
                    vec3f(0.5, 1.0, 0.5),
                ));
                let frame = thumbnail_frame_from_bounds(bounds.0, bounds.1);
                let rig = 0x7468_756d_0000_0000u64 ^ self.generation;
                self.renderer.upload_skin_rig(cx, rig, model.rest_gpu_flat());
                return Some((
                    ThumbnailSubject::Character {
                        rig,
                        texture: image_texture(cx, base_color),
                        palette,
                        bounds: Some(bounds),
                    },
                    frame,
                    bounds.0,
                    bounds.1,
                ));
            }
        }

        // Baked AO mesh is the engine-native product — use it when present.
        if job.aomesh.is_some() {
            if let Some(prepared) = self.prepare_statue(cx, job, base_color.as_deref()) {
                return Some(prepared);
            }
        }

        // Same routing law as MeshView: a material-bearing GLB (paint
        // output, textured Kenney) lights through DrawPbr; a bare TRELLIS
        // hull or factors-only model takes the game statue lane, which
        // computes normals and bakes baseColorFactor / COLOR_0 (DrawPbr on
        // a mesh without normals came out as an unshaded white silhouette).
        if let Some(gltf) = parse_material_bearing_glb(&job.glb) {
            match self
                .pbr
                .load(&mut self.draw_pbr, cx, gltf, self.generation)
            {
                Ok(()) => {
                    let bounds = self.pbr.bounds().unwrap_or((
                        vec3f(-0.5, -0.5, -0.5),
                        vec3f(0.5, 0.5, 0.5),
                    ));
                    let frame = thumbnail_frame_from_bounds(bounds.0, bounds.1);
                    self.pbr.set_fit(frame.transform);
                    return Some((ThumbnailSubject::Pbr, frame, bounds.0, bounds.1));
                }
                Err(error) => {
                    log!(
                        "thumbnail {}: PBR load failed ({error}); game-lane fallback",
                        job.file
                    );
                }
            }
        }

        self.prepare_statue(cx, job, base_color.as_deref())
    }

    fn prepare_statue(
        &mut self,
        cx: &mut CxDraw,
        job: &ThumbnailJob,
        base_color: Option<&[u8]>,
    ) -> Option<(ThumbnailSubject, ThumbnailFrame, Vec3f, Vec3f)> {
        let id = format!("aiapp/thumb-{}", self.generation);
        // Same load_model_with_ao as MeshView::load_statue: multi-tile
        // worlds split inside parse_glb and draw every embedded PNG.
        if let Err(error) = self.renderer.load_model_with_ao(
            cx,
            &id,
            &job.glb,
            base_color,
            job.aomesh.as_deref(),
            job.ao_png.as_deref(),
        ) {
            log!("thumbnail {}: GLB load failed: {error}", job.file);
            return None;
        }
        let (min, max) = self.renderer.model_bounds(&id).unwrap_or((
            vec3f(-0.5, 0.0, -0.5),
            vec3f(0.5, 1.0, 0.5),
        ));
        let frame = if let Some(spawn) = job.spawn {
            thumbnail_frame_from_spawn(spawn)
        } else {
            thumbnail_frame_from_bounds(min, max)
        };
        Some((
            ThumbnailSubject::Statue(ModelInstance {
                model: id,
                transform: frame.transform,
                tint: vec4(1.0, 1.0, 1.0, 1.0),
                color_adjust: vec4(0.0, 1.0, 1.0, 0.0),
                dynamic: true,
                depth_order: 0.0,
                part_poses: Vec::new(),
            }),
            frame,
            min,
            max,
        ))
    }

    fn finish_readback(&mut self, cx: &mut Cx) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if !active.rendered {
            return;
        }

        let readback = read_thumbnail_texture(cx, &self.color_texture);
        let Some((width, height, bgra)) = readback else {
            active.readback_attempts = active.readback_attempts.saturating_add(1);
            if active.readback_attempts < THUMBNAIL_MAX_READBACK_ATTEMPTS {
                active.rendered = false;
                active.frames_drawn = 0;
                self.area.redraw(cx);
                return;
            }
            log!(
                "thumbnail {}: render-target readback unavailable; keeping fallback badge",
                active.file
            );
            let file = active.file.clone();
            self.rejected_thumbnails.push(file);
            self.active = None;
            self.queue.finish_active();
            self.renderer = Renderer::default();
            self.area.redraw(cx);
            return;
        };

        let file = active.file.clone();
        let rgba = if width == THUMBNAIL_SIZE && height == THUMBNAIL_SIZE {
            thumbnail_bgra_to_rgba(bgra)
        } else {
            log!(
                "thumbnail {file}: readback was {width}x{height}, expected {THUMBNAIL_SIZE}x{THUMBNAIL_SIZE}"
            );
            None
        };

        // Clear-color readback = the GPU has not delivered the mesh yet.
        // Same camera, more frames. Never treat the asset as blank.
        if let Some(rgba) = &rgba {
            // Only the FIRST view waits: by the time the model has turned
            // once the GPU has plainly delivered it, and a legitimately
            // empty angle (an edge-on plane) must not restart the turn.
            if active.step == 0
                && capture_is_clear_only(rgba, THUMBNAIL_CLEAR)
                && active.gpu_waits < THUMBNAIL_MAX_GPU_WAITS
            {
                active.gpu_waits = active.gpu_waits.saturating_add(1);
                log!(
                    "thumbnail {file}: GPU still clearing, waiting ({}/{})",
                    active.gpu_waits,
                    THUMBNAIL_MAX_GPU_WAITS
                );
                active.rendered = false;
                active.frames_drawn = 0;
                active.warmup_target = THUMBNAIL_GPU_WAIT_FRAMES;
                self.area.redraw(cx);
                return;
            }
        }

        // A turntable captures this view and turns to the next one. Only
        // when every step is in hand is there a picture to encode.
        if !active.turntable.is_empty() {
            match rgba {
                Some(rgba) => active.tiles.push(rgba),
                None => {
                    let file = active.file.clone();
                    self.rejected_thumbnails.push(file);
                    self.finish_active(cx);
                    return;
                }
            }
            active.step += 1;
            if let Some(next) = active.turntable.get(active.step).cloned() {
                // The model turns; the camera does not — so EVERY lane has
                // to be handed the step's transform. Only the statue lane
                // was, which is why props turned and rigged characters did
                // not: a character's sixteen frames were sixteen copies of
                // step 0. Nobody saw it, because a still card only ever
                // draws frame 0 — but the VLM annotator reads the whole
                // sheet, so every character in the catalog was described
                // from one small view of its back, and "old man with a
                // brown hat and a sword" was a guess (2026-08-21: the user
                // saw a monkey).
                //
                // The skinned lane now reads `active.frame.transform` at
                // draw time and has no copy of its own to go stale.
                match &mut active.subject {
                    ThumbnailSubject::Statue(instance) => {
                        instance.transform = next.transform;
                    }
                    ThumbnailSubject::Pbr => self.pbr.set_fit(next.transform),
                    ThumbnailSubject::Character { .. } => {}
                }
                active.frame = next;
                active.rendered = false;
                active.frames_drawn = 0;
                active.warmup_target = THUMBNAIL_WARMUP_FRAMES;
                self.area.redraw(cx);
                return;
            }
            let tiles = std::mem::take(&mut active.tiles);
            let file = active.file.clone();
            match pack_turntable(&tiles) {
                Some(png) => self.rendered_thumbnails.push(RenderedThumbnail { file, png }),
                None => self.rejected_thumbnails.push(file),
            }
            self.finish_active(cx);
            return;
        }

        let encoded = rgba.and_then(|rgba| {
            makepad_asset_ai::testpattern::encode_png_rgba(&rgba, THUMBNAIL_SIZE, THUMBNAIL_SIZE)
                .map_err(|error| log!("thumbnail {file}: PNG encode failed: {error}"))
                .ok()
        });
        if let Some(png) = encoded {
            self.rendered_thumbnails.push(RenderedThumbnail { file, png });
        } else {
            self.rejected_thumbnails.push(file);
        }
        self.finish_active(cx);
    }

    fn finish_active(&mut self, cx: &mut Cx) {
        self.active = None;
        self.queue.finish_active();
        self.renderer = Renderer::default();
        self.area.redraw(cx);
    }

    fn pbr_maps_pending(&self) -> bool {
        let Some(status) = self.pbr.status.as_ref() else {
            return false;
        };
        status.textures_total > 0 && status.textures_ready < status.textures_total
    }

    fn draw_thumbnail_pass(&mut self, cx: &mut Cx2d) {
        self.prepare_next(cx.cx);
        let Some(active) = self.active.as_ref() else {
            return;
        };
        if active.rendered {
            return;
        }
        let is_pbr = matches!(active.subject, ThumbnailSubject::Pbr);
        let frame = active.frame;

        let size = dvec2(THUMBNAIL_SIZE as f64, THUMBNAIL_SIZE as f64);
        let rect = Rect {
            pos: dvec2(0.0, 0.0),
            size,
        };
        self.pass.set_size(cx, size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(THUMBNAIL_CLEAR),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        // Lock logical 256² and dpi=1. Retina dpi=2 on a 256 target
        // rasterizes 512px and we only keep the top-left quadrant.
        cx.begin_pass(&self.pass, Some(1.0));
        self.pass.set_size(cx, size);
        self.pass.set_dpi_factor(cx, 1.0);
        let look = thumbnail_look(frame);
        if let Some(scene_state) = preview_scene_state(look, rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.pass_list.begin_always(cx3d);
            let mut character_items = Vec::new();
            let mut character_textures = Vec::new();
            let statues = match &active.subject {
                ThumbnailSubject::Statue(instance) => vec![instance.clone()],
                ThumbnailSubject::Character {
                    rig,
                    texture,
                    palette,
                    bounds,
                } => {
                    character_items.push(
                        SkinnedDraw::new(1, *rig, frame.transform)
                            .with_texture(0)
                            .with_bounds(*bounds)
                            .with_palette(palette.clone()),
                    );
                    character_textures.push(texture);
                    Vec::new()
                }
                ThumbnailSubject::Pbr => Vec::new(),
            };
            self.renderer.set_models(statues);
            let batch = (!character_items.is_empty()).then_some(SkinnedBatch {
                skinned: &mut self.draw_skinned,
                textures: character_textures,
                items: character_items,
            });
            let mut draws = SceneDraws {
                cube: &mut self.draw_cube,
                alpha: &mut self.draw_alpha,
                sky: &mut self.draw_sky,
                sky_analytic: None,
                terrain: &mut self.draw_terrain,
                shadow: None,
                shadow_sdf: None,
                firework: None,
                flare: None,
                water: None,
                screen: None,
                screen_instances: &[],
                view_model: None,
            };
            self.renderer.draw_preview(
                cx3d,
                &mut self.draw_list,
                &mut draws,
                look,
                PreviewStage::empty(),
                scene_state,
                batch,
                Some(&mut self.draw_models),
            );
            if is_pbr {
                self.pbr.draw(&mut self.draw_pbr, cx3d);
            }
            self.pass_list.end(cx3d);
        }
        cx.end_pass(&self.pass);
        self.pass.set_dpi_factor(cx, 1.0);

        let waiting_maps = is_pbr && self.pbr_maps_pending();
        if let Some(active) = self.active.as_mut() {
            active.frames_drawn = active.frames_drawn.saturating_add(1);
            let maps_timeout = active.frames_drawn >= THUMBNAIL_PBR_TEXTURE_WAIT_FRAMES;
            if waiting_maps && !maps_timeout {
                self.area.redraw(cx.cx);
            } else if active.frames_drawn >= active.warmup_target {
                // GPU submits this frame; next draw_walk harvests the pixels.
                active.rendered = true;
            }
            self.area.redraw(cx.cx);
        }
    }
}

impl WidgetNode for ThumbnailRenderer {
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

impl Widget for ThumbnailRenderer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if self.pbr.handle_event(cx, event) {
            self.area.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        self.ensure_initialized(cx.cx);
        // Previous frame's shot is in the target — harvest before overwriting.
        if self.active.as_ref().is_some_and(|active| active.rendered) {
            self.finish_readback(cx.cx);
        }
        self.draw_thumbnail_pass(cx);
        // Sample the offscreen target in the parent pass. Without this the
        // child pass is not a frame dependency and stays at its clear color.
        self.draw_bg.draw_vars.set_texture(0, &self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_serializes_and_dedupes_independent_of_any_page_state() {
        let mut queue = ThumbQueue::default();
        assert!(queue.enqueue("lib-1.glb".into(), vec![1]));
        assert!(!queue.enqueue("lib-1.glb".into(), vec![1]), "waiting dup");
        assert!(queue.enqueue("lib-2.glb".into(), vec![2]));
        assert_eq!(queue.pending_len(), 2);

        let first = queue.take_next().unwrap();
        assert_eq!(first.file, "lib-1.glb");
        assert!(queue.take_next().is_none(), "strictly one active job");
        assert!(!queue.enqueue("lib-1.glb".into(), vec![1]), "active dup");
        assert_eq!(queue.pending_len(), 2, "active job still counts");

        queue.finish_active();
        assert!(queue.enqueue("lib-1.glb".into(), vec![1]), "finished jobs may re-queue");
        let second = queue.take_next().unwrap();
        assert_eq!(second.file, "lib-2.glb", "FIFO order");
        queue.finish_active();
        assert_eq!(queue.take_next().unwrap().file, "lib-1.glb");
        queue.finish_active();
        assert!(queue.take_next().is_none());
        assert_eq!(queue.pending_len(), 0);
    }

    /// A TURNTABLE HAS TO TURN.
    ///
    /// Sixteen steps, sixteen different angles, one shared distance so the
    /// subject does not grow and shrink as it goes round. The live failure
    /// this pins was one lane down from here — the skinned lane never
    /// received the step's transform, so every rigged character's sheet was
    /// sixteen copies of frame 0 — and it was invisible because a still card
    /// only ever draws frame 0. The VLM annotator reads the whole sheet, so
    /// every character description in the catalog was written from one small
    /// view of the model's back.
    #[test]
    fn every_turntable_step_is_a_different_angle_at_one_distance() {
        let frames = turntable_frames(vec3f(-0.3, 0.0, -0.3), vec3f(0.3, 1.8, 0.3));
        assert_eq!(frames.len(), TURNTABLE_STEPS);

        // One distance for all of them: a subject that swelled mid-turn
        // would read as broken.
        let distance = frames[0].cam_distance;
        assert!(distance.is_finite() && distance > 0.0);
        for frame in &frames {
            assert_eq!(frame.cam_distance, distance);
            assert_eq!(frame.yaw, THUMBNAIL_CAM_YAW, "the MODEL turns, not the camera");
            assert_eq!(frame.pitch, THUMBNAIL_CAM_PITCH);
        }

        // Sixteen DISTINCT rotations — the property the skinned lane was
        // silently throwing away.
        let mut seen: Vec<[f32; 4]> = Vec::new();
        for frame in &frames {
            let v = &frame.transform.v;
            let key = [v[0], v[2], v[8], v[10]]; // the yaw basis
            assert!(
                !seen.iter().any(|s| s
                    .iter()
                    .zip(key.iter())
                    .all(|(a, b)| (a - b).abs() < 1.0e-4)),
                "two steps share an angle: {key:?}"
            );
            seen.push(key);
        }

        // And it is a full circle: the last step is one step short of home.
        let back = frames[TURNTABLE_STEPS - 1].transform.v;
        let home = frames[0].transform.v;
        assert!(
            (back[0] - home[0]).abs() > 1.0e-3 || (back[2] - home[2]).abs() > 1.0e-3,
            "the turn must not land back where it started"
        );
    }

    #[test]
    fn clear_only_means_gpu_has_not_delivered_the_mesh() {
        let clear = THUMBNAIL_CLEAR;
        let clear_pixel = [
            (clear.x * 255.0).round() as u8,
            (clear.y * 255.0).round() as u8,
            (clear.z * 255.0).round() as u8,
            255,
        ];
        let total = THUMBNAIL_SIZE * THUMBNAIL_SIZE;
        let mut rgba: Vec<u8> = clear_pixel.repeat(total);
        assert!(capture_is_clear_only(&rgba, clear));
        for pixel in rgba.chunks_exact_mut(4).take(total / 2) {
            pixel[0] = pixel[0].saturating_add(CLEAR_CHANNEL_TOLERANCE);
        }
        assert!(capture_is_clear_only(&rgba, clear));
        for pixel in rgba.chunks_exact_mut(4).take(total / 100) {
            pixel[0] = 220;
            pixel[1] = 180;
            pixel[2] = 40;
        }
        assert!(!capture_is_clear_only(&rgba, clear));
    }

    #[test]
    fn studio_centers_aabb_even_after_yaw() {
        let min = vec3f(1.9, 0.0, 1.4);
        let max = vec3f(2.1, 0.25, 1.57);
        let frame = thumbnail_frame_from_bounds(min, max);
        let center = (min + max) * 0.5;
        let mapped = frame.transform.transform_vec4(vec4(center.x, center.y, center.z, 1.0));
        assert!(
            mapped.x.abs() < 1.0e-4 && mapped.y.abs() < 1.0e-4 && mapped.z.abs() < 1.0e-4,
            "pack-grid centre must stay at origin after yaw: {mapped:?}"
        );
        assert_eq!(frame.cam_target, vec3f(0.0, 0.0, 0.0));
        assert!(
            frame.yaw > std::f32::consts::PI,
            "camera orbits 180° instead of flipping the mesh: {}",
            frame.yaw
        );
        let thin = thumbnail_frame_from_bounds(vec3f(0.0, 0.0, 0.0), vec3f(2.0, 0.1, 0.1));
        let boxy = thumbnail_frame_from_bounds(vec3f(0.0, 0.0, 0.0), vec3f(1.0, 1.0, 1.0));
        assert!(thin.cam_distance < boxy.cam_distance, "{:?} vs {:?}", thin.cam_distance, boxy.cam_distance);
        let small = thumbnail_frame_from_bounds(vec3f(1.9, 0.0, 1.4), vec3f(2.1, 0.25, 1.57));
        let large = thumbnail_frame_from_bounds(vec3f(0.3, 0.0, 0.0), vec3f(3.5, 1.8, 2.8));
        let small_scale = (small.transform.v[0].powi(2) + small.transform.v[2].powi(2)).sqrt();
        let large_scale = (large.transform.v[0].powi(2) + large.transform.v[2].powi(2)).sqrt();
        assert!(small_scale > large_scale * 2.0, "{small_scale} vs {large_scale}");
        let point = thumbnail_frame_from_bounds(vec3f(2.0, 0.125, 1.485), vec3f(2.0, 0.125, 1.485));
        assert!(point.cam_distance.is_finite() && point.cam_distance > 0.0);
    }

    #[test]
    fn fit_transform_is_centered_and_uses_largest_extent() {
        let min = vec3f(-2.0, 1.0, -0.5);
        let max = vec3f(4.0, 4.0, 1.5);
        let transform = thumbnail_fit_transform(min, max).unwrap();
        let center = (min + max) * 0.5;
        let mapped = transform.transform_vec4(vec4(center.x, center.y, center.z, 1.0));
        assert!(mapped.x.abs() < 1.0e-6);
        assert!(mapped.y.abs() < 1.0e-6);
        assert!(mapped.z.abs() < 1.0e-6);
        let scale = transform.v[0];
        assert!((scale * (max.x - min.x) - THUMBNAIL_SUBJECT_EXTENT).abs() < 1.0e-6);
        assert!(thumbnail_fit_transform(min, min).is_none());
    }

    #[test]
    fn readback_swizzle_is_bgra_to_rgba_without_touching_alpha() {
        let mut bgra = vec![0u8; THUMBNAIL_SIZE * THUMBNAIL_SIZE * 4];
        bgra[..8].copy_from_slice(&[3, 2, 1, 4, 30, 20, 10, 40]);
        let rgba = thumbnail_bgra_to_rgba(bgra).unwrap();
        assert_eq!(&rgba[..8], &[1, 2, 3, 4, 10, 20, 30, 40]);
        assert!(thumbnail_bgra_to_rgba(vec![0; 4]).is_none());
    }
}
