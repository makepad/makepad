//! GPU-resident world lightmap bake: the CPU ray bake's per-texel work as
//! fragment-shader passes, ZERO CPU readback. The output is bit-compatible
//! with lightmap.rs's atlas conventions (A = sun SDF with 128 the edge,
//! RGB = lamp light / lightmap::LM_LAMP_CEIL, plus the R8-style shadow-top
//! plane), so the material shaders consume it unchanged.
//!
//! # Pass pipeline (all raster, per dirty region)
//!
//! 1. sun depth: every caster (mesh instances + occluder boxes + movers in
//!    Realtime) from the region's fitted ortho sun camera into an R32F
//!    scratch (hardware z-test keeps the nearest).
//! 2. sun gather: the region's OWN geometry rasterized in lightmap-uv space
//!    at 4x its atlas resolution — the fragment is one supersample doing
//!    the depth compare (lightmap.rs's `sun_bit`, bias for rays).
//! 3. despeckle (3x3 vote) at 4x, then 4x->1x coverage downsample into an
//!    atlas-layout coverage texture.
//! 4. distance transform at 1x as iterated 3/4-chamfer relaxation over the
//!    coverage majority (see below for why not seed-JFA), rect-clamped so
//!    regions never bleed.
//! 5. lamps: the accumulator is first HARD-ZEROED over this batch's write
//!    footprint (each region's rect + pad ring — [`batch_zero_rects`]),
//!    because it loads and a batch owns only its own regions; then per lamp
//!    a 6-face tiled depth render + one additive gather over every receiving
//!    region in radius (N.L x (1-d/r)^2 x cone^2 with SPILL = 0.35,
//!    mirroring the CPU lamp loop), then the two-ring rim dilation + 4/2/1
//!    smooth. Re-baking an unedited world must land on the same bytes;
//!    MAKEPAD_GPU_LM_REBAKE (renderer.rs) measures that end to end.
//! 6. encode: A from the signed distance blended toward measured coverage
//!    at mixed texels (the CPU's thin-feature guard), RGB from the lamp
//!    accumulation. Region quads are expanded one texel so the pad ring
//!    keeps the "fully lit / no lamps" default.
//! 7. shadow-top plane from the GROUND region's sun depth, + two min-dilate
//!    rings (bake_top_plane's contract).
//!
//! # Why chamfer iterations instead of seed-JFA
//!
//! The encoded band is only ±4 texels, so 5 relaxation steps of the exact
//! 3/4-chamfer metric reproduce the CPU transform bit-for-bit-ish in the
//! only range that is ever decoded — with two byte channels in a plain
//! BGRA8 ping-pong. JFA needs 2x11-bit seed coordinates per class, which
//! wants integer targets for no accuracy gain inside the band.
//!
//! # Scheduling modes — the two-tier shadow contract
//!
//! The ATLAS is static-only and mode-independent: one camera-blind bake per
//! dirty kick (world edit / explicit sun change, settle-debounced by the
//! renderer), identical bytes in both modes. The bake is AMORTIZED — a kick
//! queues every region and each frame encodes at most [`DEFAULT_BAKE_BUDGET`]
//! of them, so a world appears immediately in flat light and its shadows
//! land over the next frames instead of behind a multi-second freeze. The
//! caller keeps drawing (a game view always does); [`GpuLightmapBaker::bake_progress`]
//! says whether lighting is still filling in, for an app that wants to tell
//! the player. What differs between modes is how DYNAMIC
//! casters shadow and where SUN visibility comes from at shade time:
//!
//! [`GpuLightmapMode::OnChange`] (default everywhere, the slow-GPU tier):
//! sun visibility from the atlas A channel; dynamics cast through the
//! prebaked SDF silhouette quads ([`dynamic_shadow_tiers`]).
//!
//! [`GpuLightmapMode::Realtime`] (opt-in, fast GPUs): classic CASCADED
//! SHADOW MAPS — per frame, [`crate::shadow_csm`] fits ortho cascades to
//! the view frustum and ONE depth pass renders every caster (statics,
//! entity boxes, movers, skinned characters) into them, chained ahead of
//! the scene pass in the same frame's command stream. Material shaders
//! take sun visibility from a PCF compare against the cascades and IGNORE
//! the atlas A channel; the atlas serves only lamps RGB (and the models'
//! AO textures ride their own sidecars). One receive path for every
//! surface class — a mover cannot inherit ground-projected shadow
//! bookkeeping, because there is none in this tier. The steady-state atlas
//! cost in Realtime is therefore ZERO passes, exactly like OnChange; the
//! per-frame cost is the cascade depth pass.

use crate::lightmap::{plan_atlas, LmLight, LmRect, LmScene};
use crate::shadow_csm::{fit_cascades, CsmFrame, CsmView, CSM_CASCADES};
use crate::shaders::*;
use makepad_draw::*;

/// Mirrors lightmap.rs's private constants — the GPU passes must agree with
/// the CPU conventions the material shaders were tuned against.
const RAY_OFFSET: f32 = 0.02;

/// How the baker schedules work. Pure runtime policy — no platform
/// conditionals; switchable live via [`Renderer::set_gpu_lightmap_mode`].
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum GpuLightmapMode {
    /// The universal default (slow-GPU tier): sun from the baked atlas,
    /// dynamics through the prebaked SDF quad tier.
    #[default]
    OnChange,
    /// Fast-GPU tier: per-frame cascaded shadow maps serve SUN visibility
    /// for every receiver; every caster (characters included) renders into
    /// the cascades; the SDF quad tier draws nothing.
    Realtime,
    /// No dynamic sun shadows. Cascades and SDF quads are both off;
    /// `csm_vis` returns 1 and materials see full sun. Preview / debug
    /// toggle — does not kick an OnChange atlas bake.
    Off,
}

/// Device-local budget for the Realtime cascaded-shadow tier.
///
/// `tile_resolution` is the edge of ONE cascade. The three fixed-layout
/// cascade tiles are stored side by side, with one R32F color target and one
/// D32 depth target, so target memory is `24 * tile_resolution^2` bytes.
/// `far_range` is the world-space reach of the final cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CsmConfig {
    pub tile_resolution: usize,
    pub far_range: f32,
}

pub const DEFAULT_CSM_CONFIG: CsmConfig = CsmConfig {
    tile_resolution: 2048,
    far_range: 80.0,
};

const MIN_CSM_TILE_RESOLUTION: usize = 256;
const MAX_CSM_TILE_RESOLUTION: usize = 4096;
const MIN_CSM_FAR_RANGE: f32 = 10.0;
const MAX_CSM_FAR_RANGE: f32 = 4096.0;

impl Default for CsmConfig {
    fn default() -> Self {
        DEFAULT_CSM_CONFIG
    }
}

impl CsmConfig {
    fn clamped(tile_resolution: usize, far_range: f32) -> Self {
        Self {
            tile_resolution: tile_resolution
                .clamp(MIN_CSM_TILE_RESOLUTION, MAX_CSM_TILE_RESOLUTION),
            far_range: if far_range.is_finite() {
                far_range.clamp(MIN_CSM_FAR_RANGE, MAX_CSM_FAR_RANGE)
            } else {
                DEFAULT_CSM_CONFIG.far_range
            },
        }
    }
}

/// Runtime device policy plus optional launch-time locks. Explicit
/// `MAKEPAD_CSM_RES` / `MAKEPAD_CSM_FAR` values always win over a later
/// device-tier request (for example the sandbox's first-XR-frame cut).
#[derive(Clone, Copy, Debug)]
struct CsmPolicy {
    device: CsmConfig,
    env_resolution: Option<usize>,
    env_far_range: Option<f32>,
}

impl Default for CsmPolicy {
    fn default() -> Self {
        Self {
            device: CsmConfig::default(),
            env_resolution: std::env::var("MAKEPAD_CSM_RES")
                .ok()
                .and_then(|value| value.parse().ok()),
            env_far_range: std::env::var("MAKEPAD_CSM_FAR")
                .ok()
                .and_then(|value| value.parse().ok()),
        }
    }
}

impl CsmPolicy {
    fn effective(&self) -> CsmConfig {
        CsmConfig::clamped(
            self.env_resolution
                .unwrap_or(self.device.tile_resolution),
            self.env_far_range.unwrap_or(self.device.far_range),
        )
    }

    fn set_device(&mut self, tile_resolution: usize, far_range: f32) -> CsmConfig {
        self.device = CsmConfig::clamped(tile_resolution, far_range);
        self.effective()
    }
}

/// Which tier serves the DYNAMIC casters under a scheduling mode. ONE
/// decision point, consumed by both the renderer's SDF-quad/blob/drape draw
/// gates and its mover collection, so the two can never disagree — a caster
/// drawing both tiers doubles its shadow, drawing neither loses it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DynamicShadowTiers {
    /// Prebaked SDF silhouette quads (plus their blob/hull fallbacks).
    pub sdf_quads: bool,
    /// Everything renders into the per-frame shadow-map cascades (and every
    /// material samples them for sun visibility).
    pub csm: bool,
}

pub fn dynamic_shadow_tiers(mode: GpuLightmapMode) -> DynamicShadowTiers {
    match mode {
        GpuLightmapMode::OnChange => DynamicShadowTiers {
            sdf_quads: true,
            csm: false,
        },
        GpuLightmapMode::Realtime => DynamicShadowTiers {
            sdf_quads: false,
            csm: true,
        },
        GpuLightmapMode::Off => DynamicShadowTiers {
            sdf_quads: false,
            csm: false,
        },
    }
}

/// Regions one frame may bake before the rest wait for the next frame. A
/// city of 3285 lit props is 13k passes: encoded in one frame that is a
/// four-second freeze on first load, and the world does not appear at all
/// until it ends. Encoded a slice at a time, the world is on screen from
/// frame one in flat light and its shadows land over the following second.
/// `MAKEPAD_GPU_LM_BAKE_BUDGET=0` restores the everything-at-once bake (the
/// honest choice for an offline capture, where a settled frame matters and
/// no one is watching it arrive).
const DEFAULT_BAKE_BUDGET: usize = 24;

/// The ATLAS scheduling decision, pure and mode-independent: the dirty bit
/// — set only by a realized job (a world edit's settle kick), never by
/// routine mover or replication traffic — queues ALL regions once,
/// camera-blind, and a clean bit with a drained queue encodes ZERO atlas
/// passes. Realtime's per-frame work is the CASCADES, never the atlas (the
/// invariant the tests below pin for both modes).
///
/// The queue is what makes the bake amortizable: a kick fills it, every
/// frame drains at most `budget` of it (0 = no budget, the whole kick in
/// one frame), and the atlas is correct-so-far the whole way — each
/// region's chain writes only its own rect of the persistent atlas, so a
/// half-drained queue is a world whose remaining props are still flat, not
/// a world whose lighting is wrong.
///
/// `only_region` is the MAKEPAD_GPU_LM_ONLY_REGION debug pin (stage
/// dumps).
fn schedule_regions(
    dirty: &mut bool,
    queue: &mut std::collections::VecDeque<usize>,
    region_count: usize,
    only_region: Option<usize>,
    budget: usize,
) -> Vec<usize> {
    if *dirty {
        *dirty = false;
        // A fresh kick supersedes whatever the last one had left: the
        // layout it was baking is gone.
        queue.clear();
        match only_region.filter(|only| *only < region_count) {
            Some(only) => queue.push_back(only),
            None => queue.extend(0..region_count),
        }
    }
    let take = if budget == 0 {
        queue.len()
    } else {
        budget.min(queue.len())
    };
    queue.drain(..take).collect()
}

/// One caster the depth passes rasterize.
pub struct GpuBakeMesh {
    pub geometry: GeometryId,
    pub transform: Mat4f,
    /// World AABB (transformed model bounds).
    pub min: Vec3f,
    pub max: Vec3f,
}

/// A dynamic caster for Realtime mode's depth passes. No identity, no
/// history: Realtime is stateless, every visible region re-bakes with the
/// current caster set each frame.
pub struct GpuLmMover {
    pub geometry: GeometryId,
    pub transform: Mat4f,
    /// Model-space bounds (lamp-face culling via the world transform).
    pub min: Vec3f,
    pub max: Vec3f,
    /// Skinned casters (characters): rest mesh in `geometry`, pose from the
    /// frame's joint palette. `None` = rigid.
    pub skin: Option<GpuLmSkin>,
}

/// A skinned mover's pose source: the frame's shared joint-palette texture
/// and this caster's first texel in it — exactly what the visible
/// DrawSceneSkinnedGpu draw binds, so bake and picture skin identically.
#[derive(Clone)]
pub struct GpuLmSkin {
    pub joint_tex: Texture,
    pub joint_base: f32,
}

/// A snapshot the renderer hands over on the settle path (the same moment
/// the CPU bake used to be kicked).
pub struct GpuBakeJob {
    pub scene: LmScene,
    /// Parallel to `scene.meshes`: each instance's GPU geometry.
    pub mesh_geometry: Vec<GeometryId>,
    /// Parallel to `scene.meshes`: the placed-model index each region maps
    /// back to (for the renderer's lm_remaps).
    pub mesh_map: Vec<usize>,
    /// Static instances WITHOUT a lightmap layout of their own (no AO
    /// bake): they own no region — they light analytically — but they still
    /// stand in the sun, so they join every sun-depth pass as casters. A
    /// fence without a bake used to throw no shadow at all.
    pub casters_only: Vec<GpuBakeMesh>,
    /// World xz rect of the ground region (x0, z0, span, span).
    pub terrain_world: Option<Vec4f>,
    /// Why this bake was scheduled, for the log. The bake is the one thing
    /// in the frame that can change a settled picture seconds after the
    /// world appeared, so it says so out loud.
    pub trigger: BakeTrigger,
}

/// What asked for a bake. Purely for the log line — a blowout that "pops
/// in" has to be attributable to the run that caused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BakeTrigger {
    /// The first bake of a realm: the world just loaded.
    #[default]
    FirstBake,
    /// The world changed — an edit, a re-eval, streamed-in props.
    WorldEdit,
    /// The sun moved far enough to restrengthen the lamps.
    SunChange,
}

impl BakeTrigger {
    fn label(self) -> &'static str {
        match self {
            BakeTrigger::FirstBake => "first bake",
            BakeTrigger::WorldEdit => "world edit / re-eval",
            BakeTrigger::SunChange => "sun change",
        }
    }
}

/// The sum a lamp is allowed to leave on flat, unshadowed, up-facing
/// ground: daylight plus every pool that reaches it. Above 1.0 the frame
/// clips — a near-white albedo goes to pure white with the warm rim of the
/// tint's red channel saturating first. `lightmap::lamp_daylight_scale` is
/// the rail that keeps this true; this constant is what the bake CHECKS,
/// so a future path that skips the rail is caught by its own log line
/// instead of by a screenshot.
const LM_EXPOSURE_CEILING: f32 = 1.0;

/// What the renderer stores when a scheduled layout is first realized.
pub struct GpuLmDelivery {
    pub atlas: Texture,
    pub top: (Texture, f32, f32),
    pub size: usize,
    pub mesh_rects: Vec<LmRect>,
    pub planar_rects: Vec<LmRect>,
    pub mesh_map: Vec<usize>,
    pub terrain_world: Option<Vec4f>,
}

/// A fitted sun (or lamp-face) camera as shader rows:
/// `dot(row.xyz, world) + row.w` = ndc x / ndc y / z01.
#[derive(Clone, Copy)]
struct SunCam {
    rx: Vec4f,
    ry: Vec4f,
    rz: Vec4f,
    /// z01 span in world units (decodes z01 deltas back to distances).
    zr: f32,
    /// Depth bias in z01 units, scaled by the map's texel footprint.
    bias01: f32,
}

struct GroundInfo {
    /// World rect (x0, z0, span_x, span_z).
    world: Vec4f,
    /// Heightfield decode: (origin_x, origin_z, cell, n).
    hf: Vec4f,
    heights_tex: Texture,
    /// Shadow-top encode window.
    top_base: f32,
    top_range: f32,
}

enum RegionKind {
    /// Index into `BakeState::meshes`.
    Mesh(usize),
    Ground,
}

struct Region {
    rect: LmRect,
    kind: RegionKind,
    /// World bounds of the receiver (light culling, sun camera fit).
    min: Vec3f,
    max: Vec3f,
    /// Measured chart density, atlas texels per world unit of SURFACE
    /// (lightmap::chart_density; ground = rect over span). Converts the
    /// world-space sun band into this region's texels at encode time.
    tpu: f32,
}

struct BakeTextures {
    atlas: Texture,
    top_a: Texture,
    top_b: Texture,
    cov: Texture,
    lamp_a: Texture,
    lamp_b: Texture,
    dt_a: Texture,
    dt_b: Texture,
    mask_a: Texture,
    mask_b: Texture,
    sun_depth: Texture,
    sun_depth_z: Texture,
    /// FAR sun depth (max z01 = the surface nearest the GROUND along the
    /// ray): the shadow-top plane records where a ground ray is first
    /// BLOCKED, i.e. a slab's underside — the CPU bake's
    /// `scene_nearest_block` semantics. The near map would put the blocker
    /// at the slab's TOP, and every receiver at that height re-shadowed.
    sun_far: Texture,
    lamp_depth: Texture,
    lamp_depth_z: Texture,
    mask_w: usize,
    mask_h: usize,
}

/// The realized layout: everything a frame's passes read.
struct BakeState {
    size: usize,
    regions: Vec<Region>,
    meshes: Vec<GpuBakeMesh>,
    /// Prefix of `meshes` that owns lightmap regions. The suffix is the old
    /// caster-only snapshot used by atlas passes; Realtime gets those same
    /// uncharted statics from the renderer's upload-time registry instead,
    /// including all material layers.
    region_mesh_count: usize,
    /// All occluder boxes packed as one world-space triangle soup.
    box_geometry: Option<Geometry>,
    lights: Vec<LmLight>,
    ground: Option<GroundInfo>,
    /// Scene AABB (casters + ground) — bounds every sun camera's near plane
    /// and the cascades' z windows.
    scene_min: Vec3f,
    scene_max: Vec3f,
    tex: BakeTextures,
}

/// All the baker's draw shaders, lazily constructed from their script
/// type defaults.
struct LmDraws {
    zero: DrawLmZero,
    sun_depth: DrawLmSunDepth,
    sun_depth_skinned: DrawLmSunDepthSkinned,
    lamp_depth: DrawLmLampDepth,
    gather_mesh: DrawLmSunGatherMesh,
    gather_ground: DrawLmSunGatherGround,
    despeckle: DrawLmDespeckle,
    downsample: DrawLmDownsample,
    chamfer: DrawLmChamfer,
    lamp_mesh: DrawLmLampGatherMesh,
    lamp_ground: DrawLmLampGatherGround,
    lamp_dilate: DrawLmLampDilate,
    encode: DrawLmEncode,
    top: DrawLmTop,
    top_dilate: DrawLmTopDilate,
}

/// THE ZERO FOOTPRINT of one batch: every region it is about to bake, each
/// over the rect the bake actually writes ([`LmRect::padded`] — the chart
/// rect plus the pad ring the encode stamps and the dilate reads).
///
/// Exactly the batch, and nothing else: a region this batch does NOT bake
/// keeps its content, which is what makes the amortized kick (24 regions a
/// frame) and any partial re-bake correct — and the pack's gutter
/// (`lightmap::LM_PAD`) guarantees a padded rect can never reach into a
/// neighbouring region's texels.
fn batch_zero_rects(regions: &[Region], batch: &[usize], size: usize) -> Vec<LmRect> {
    batch.iter().map(|ri| regions[*ri].rect.padded(size)).collect()
}

/// Hard-zero one footprint rect in the pass that is currently open.
/// Blending is off in `DrawLmZero`, so this is a real clear, not another
/// accumulation.
fn zero_rect(cx: &mut CxDraw, d: &mut DrawLmZero, rect: LmRect, size: usize, fill: Vec4f) {
    d.quad_a = rect.uv_remap(size);
    d.fill_a = fill;
    if d.draw_vars.can_instance() {
        cx.add_instance(&d.draw_vars);
    }
}

/// Grow `pool` to `n` reusable passes. A free function rather than a method
/// so the atlas chain and the cascade chain can each own a pool while the
/// baker keeps the rest of its state borrowable.
fn ensure_pool(pool: &mut Vec<BakePass>, cx: &mut Cx, n: usize) {
    while pool.len() < n {
        let pass = DrawPass::new(cx);
        // Recover the pool index of this pass id: pass ids are small
        // (one per pass ever alive), so a linear probe terminates fast.
        let probe = (0..1_000_000)
            .find(|i| pass.id_equals(*i))
            .unwrap_or(usize::MAX);
        pool.push(BakePass {
            pass,
            list: DrawList::new(cx),
            probe,
        });
    }
}

/// Pool indices in execution order (descending pass id), truncated to the
/// `n` passes this frame actually encodes.
fn pass_order(pool: &[BakePass], n: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..pool.len()).collect();
    order.sort_by(|a, b| pool[*b].probe.cmp(&pool[*a].probe));
    order.truncate(n);
    order
}

struct BakePass {
    pass: DrawPass,
    list: DrawList,
    /// Probed pool index of the pass id — the execution order sorts by
    /// this DESCENDING so the platform's child-before-parent insertion
    /// yields exactly the chain order (a child must carry a HIGHER id than
    /// the pass that consumes its output).
    probe: usize,
}

/// One frame's chained pass sequence over the (persistent) pool: opens
/// passes in execution order, wiring each pass's parent to the NEXT one so
/// the platform renders them in exactly this order, ending at the pass
/// currently being drawn (the game's 3D pass).
struct PassSeq<'a> {
    pool: &'a mut Vec<BakePass>,
    /// Pool indices in execution order (descending pass id).
    order: Vec<usize>,
    cursor: usize,
}

impl<'a> PassSeq<'a> {
    fn open(
        &mut self,
        cx: &mut CxDraw,
        w: usize,
        h: usize,
        color: &Texture,
        clear: DrawPassClearColor,
        depth: Option<&Texture>,
    ) -> usize {
        let n = self.order.len();
        let k = self.cursor;
        self.cursor += 1;
        // Parent chain: pass k renders before pass k+1; the tail parents to
        // whatever pass is open right now.
        if k + 1 < n {
            let parent_id = self.pool[self.order[k + 1]].pass.draw_pass_id();
            let child_id = self.pool[self.order[k]].pass.draw_pass_id();
            cx.cx.passes[child_id].parent = CxDrawPassParent::DrawPass(parent_id);
        } else {
            cx.make_child_pass(&self.pool[self.order[k]].pass);
        }
        let bp = &mut self.pool[self.order[k]];
        cx.begin_pass(&bp.pass, Some(1.0));
        bp.pass.set_size(cx.cx, dvec2(w as f64, h as f64));
        bp.pass.clear_color_textures(cx.cx);
        bp.pass.set_color_texture(cx.cx, color, clear);
        match depth {
            Some(t) => {
                bp.pass
                    .set_depth_texture(cx.cx, t, DrawPassClearDepth::ClearWith(1.0));
            }
            None => {
                cx.cx.passes[bp.pass.draw_pass_id()].depth_texture = None;
            }
        }
        bp.list.begin_always(cx);
        self.order[k]
    }

    fn close(&mut self, cx: &mut CxDraw, idx: usize) {
        let bp = &mut self.pool[idx];
        bp.list.end(cx);
        cx.end_pass(&bp.pass);
    }
}

pub struct GpuLightmapBaker {
    mode: GpuLightmapMode,
    /// Realtime CSM resolution/range are presentation policy, not realm
    /// state. They survive realm switches and may change live.
    csm_policy: CsmPolicy,
    /// A job scheduled by the renderer, realized on the next draw.
    pending: Option<GpuBakeJob>,
    state: Option<BakeState>,
    draws: Option<Box<LmDraws>>,
    pool: Vec<BakePass>,
    /// Realtime's cascaded shadow maps: CSM_CASCADES tiles side by side in
    /// one Rf32 strip (one texture slot in every material family), plus its
    /// hardware depth. Written every Realtime frame, sampled by the PCF
    /// compare in the material shaders.
    ///
    /// Owned by the BAKER, never by a realized atlas layout. The cascade
    /// tier must serve worlds that have NO static lightmap at all — flat
    /// starter terrain, a props-free scene, anything where
    /// `kick_lightmap_bake` finds no AO mesh and no receiver box and so
    /// never schedules a job. Hanging these off `state` made F8 silently
    /// drop EVERY dynamic shadow in exactly those worlds: no state, no
    /// cascades, no binding, `csm_vis` returning full sun.
    ///
    /// Allocated only while Realtime is serving. At the default 2048 tile
    /// edge these two targets are about 96 MiB per view, so retaining them
    /// in OnChange (where they are never sampled) is a serious split/mobile
    /// memory leak rather than harmless scratch.
    csm_tex: Option<Texture>,
    csm_depth: Option<Texture>,
    /// One cascade tile's edge, texels — latched when the targets allocate,
    /// so a live `set_csm_config` can never mix a new inverse resolution
    /// with an old target.
    csm_res: usize,
    /// The cascade pass owns its own one-entry pool: it is encoded on
    /// frames where the atlas encodes nothing, and both chains hang off the
    /// 3D pass independently.
    csm_pool: Vec<BakePass>,
    /// The whole dirt model, both modes: the next run queues ALL atlas
    /// regions for re-bake. Set only by a realized job — the atlas is
    /// static-only and mode-independent, so mode switches never touch it.
    dirty: bool,
    /// Regions of the current kick still to bake, drained `bake_budget` at
    /// a time. Non-empty means "the lighting is still filling in".
    bake_queue: std::collections::VecDeque<usize>,
    /// Regions in the kick that filled `bake_queue` — the denominator of
    /// [`Self::bake_progress`].
    bake_total: usize,
    /// Regions per frame (0 = the whole kick at once); see
    /// [`DEFAULT_BAKE_BUDGET`].
    bake_budget: usize,
    /// This kick's accumulated cost, so the finish can report the whole
    /// bake instead of one slice of it.
    bake_frames: u32,
    bake_passes: usize,
    bake_us: u64,
    /// This frame's fitted cascades (Realtime), for the renderer's material
    /// uniforms. None in OnChange.
    csm_last: Option<CsmFrame>,
    /// The last announced `(static, live)` caster counts. A scene can upload
    /// after the first empty Realtime frame, so count changes announce again
    /// instead of leaving the log permanently claiming zero casters.
    csm_logged: Option<(usize, usize)>,
    /// Realtime perf probe accumulators (`MAKEPAD_GPU_LM_PERF=1` logs the
    /// per-frame cascade-encode averages every 120 frames).
    rt_frames: u64,
    rt_us: u64,
}

impl Default for GpuLightmapBaker {
    fn default() -> Self {
        Self {
            mode: GpuLightmapMode::default(),
            csm_policy: CsmPolicy::default(),
            pending: None,
            state: None,
            draws: None,
            pool: Vec::new(),
            csm_tex: None,
            csm_depth: None,
            csm_res: DEFAULT_CSM_CONFIG.tile_resolution,
            csm_pool: Vec::new(),
            dirty: false,
            bake_queue: std::collections::VecDeque::new(),
            bake_total: 0,
            bake_budget: bake_budget_from_env(),
            bake_frames: 0,
            bake_passes: 0,
            bake_us: 0,
            csm_last: None,
            csm_logged: None,
            rt_frames: 0,
            rt_us: 0,
        }
    }
}

/// `MAKEPAD_GPU_LM_BAKE_BUDGET` regions per frame, else the default. Read
/// once per baker rather than per frame: this is a launch policy, and a
/// bake that changed budget halfway through would report nonsense progress.
fn bake_budget_from_env() -> usize {
    std::env::var("MAKEPAD_GPU_LM_BAKE_BUDGET")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_BAKE_BUDGET)
}

fn v3(x: f32, y: f32, z: f32) -> Vec3f {
    Vec3f { x, y, z }
}

fn csm_caster_counts(
    state: Option<&BakeState>,
    registered_statics: usize,
    live: usize,
) -> (usize, usize) {
    (
        state.map_or(0, |state| state.region_mesh_count) + registered_statics,
        live,
    )
}

/// The one number a region's exposure report is about: what the brightest
/// texel of `region` can read once the bake lands, and how it got there.
struct RegionExposure {
    /// Lamps whose reach touches the region.
    lamps: usize,
    /// The brightest lamp light any texel of the region can receive.
    /// Evaluated AT each touching bulb's own hot spot and summed there, so
    /// two pools 26 m apart do not add into a number neither of them
    /// delivers — a report that cries wolf is worse than no report.
    lamp_peak: f32,
    /// `daylight + lamp_peak`: the composite an unshadowed, flat, up-facing
    /// texel of this region reads on an albedo of 1. Over
    /// [`LM_EXPOSURE_CEILING`] the frame CLIPS.
    sum: f32,
    /// GROUND regions only: how many texels the pools drive past the
    /// FRAME's clip point. `LM_LAMP_SAT_TEXELS` of them is the collar a
    /// fixture is allowed (5cf85a720: "a lamp may blow out a 1.13 m collar,
    /// never the street"); more than that is the street.
    clip_texels: f32,
}

fn region_exposure(
    region: &Region,
    lights: &[LmLight],
    daylight: f32,
    texels_per_unit: f32,
) -> RegionExposure {
    use crate::lightmap::{
        lamp_ground_light, lamp_ground_texels_over, lamp_mount_from_radius, lamp_peak_in_box,
        LM_LAMP_CEIL,
    };
    let ground = matches!(region.kind, RegionKind::Ground);
    let headroom = (LM_EXPOSURE_CEILING - daylight).max(0.0);
    let touching: Vec<&LmLight> = lights
        .iter()
        .filter(|l| sphere_touches_box(l.pos, l.radius, region.min, region.max))
        .collect();
    // What light `l` lays at world point `p`. On the ground that is the
    // pool's own arithmetic and exact for a harvested fixture; on a mesh
    // the surface normal is unknown, so the distance falloff alone is the
    // honest upper bound.
    let at = |l: &LmLight, p: Vec3f| -> f32 {
        let cmax = l.color.x.max(l.color.y).max(l.color.z);
        if ground {
            let mount = lamp_mount_from_radius(l.radius);
            let rho = ((p.x - l.pos.x).powi(2) + (p.z - l.pos.z).powi(2)).sqrt();
            lamp_ground_light(mount, rho, l.radius, cmax, l.spot)
        } else {
            let d = ((p.x - l.pos.x).powi(2) + (p.y - l.pos.y).powi(2) + (p.z - l.pos.z).powi(2))
                .sqrt();
            if d >= l.radius {
                0.0
            } else {
                let att = 1.0 - d / l.radius;
                cmax * att * att
            }
        }
    };
    // The hot spot is under one of the bulbs; overlap between pools shows
    // up because every OTHER pool is evaluated at that same point.
    let mut lamp_peak = 0.0f32;
    let mut clip_texels = 0.0f32;
    for l in &touching {
        let p = Vec3f {
            x: l.pos.x.clamp(region.min.x, region.max.x),
            y: l.pos.y.clamp(region.min.y, region.max.y),
            z: l.pos.z.clamp(region.min.z, region.max.z),
        };
        let here: f32 = touching.iter().map(|o| at(o, p)).sum();
        // On a mesh the point-sample IS `lamp_peak_in_box`; take the pair's
        // max so the bound can never read under the closest approach.
        lamp_peak = lamp_peak
            .max(here)
            .max(if ground { 0.0 } else { lamp_peak_in_box(l, region.min, region.max) });
        if ground {
            let cmax = l.color.x.max(l.color.y).max(l.color.z);
            clip_texels += lamp_ground_texels_over(
                lamp_mount_from_radius(l.radius),
                l.radius,
                cmax,
                l.spot,
                texels_per_unit,
                headroom,
            );
        }
    }
    // The atlas cannot carry more than its own ceiling however many pools
    // overlap, so the report may not claim more either.
    lamp_peak = lamp_peak.min(LM_LAMP_CEIL);
    RegionExposure {
        lamps: touching.len(),
        lamp_peak,
        sum: daylight + lamp_peak,
        clip_texels,
    }
}

/// Annotate the bake, once per run, at INFO — the permanent record of what
/// the bake is about to do to a picture the player is already looking at.
///
/// A bake is the ONE thing that changes a settled frame seconds after a
/// world appears, so a blowout that "pops in" must be attributable from the
/// log alone: which run, what triggered it, how many regions and lamps, how
/// much of the display's 0..1 the sky had already spent, and — per region
/// that a lamp reaches — the sum that region's brightest texel will read.
/// Any region over [`LM_EXPOSURE_CEILING`] gets its own loud line naming
/// itself and its numbers, so the next regression does not need a
/// screenshot.
///
/// Cost is per REGION, not per texel: a handful of float ops against a
/// light list that is already in hand. The heavy per-region lines are
/// capped so a city of thousands of props stays readable.
fn report_bake_exposure(state: &BakeState, scene: &LmScene, trigger: BakeTrigger, budget: usize) {
    use crate::lightmap::{LM_LAMP_GROUND_PEAK, LM_LAMP_SAT_TEXELS};
    let daylight =
        crate::lightmap::daylight_on_ground(scene.sun_dir, scene.sun_color, scene.sun_sky);
    let scale = crate::lightmap::lamp_daylight_scale(daylight);
    // The ground region's real density, so `clip_texels` is in the same
    // units the collar budget is written in.
    let tpu = state
        .regions
        .iter()
        .find(|r| matches!(r.kind, RegionKind::Ground))
        .map(|r| r.rect.w as f32 / (r.max.x - r.min.x).max(0.0001))
        .unwrap_or(crate::lightmap::LM_LAMP_SAT_DENSITY);
    let exposure = |r: &Region| region_exposure(r, &state.lights, daylight, tpu);
    log!(
        "lm bake: {} — {} regions ({} lamp-lit), {} lamp(s), {}px atlas, {} region(s)/frame, \
         ground {:.2} texels/unit; sun elev {:.0} deg, daylight {:.3} of {:.1} on flat ground, \
         headroom {:.3}, lamp scale {:.2} (pool peak {:.3})",
        trigger.label(),
        state.regions.len(),
        state.regions.iter().filter(|r| exposure(r).lamps > 0).count(),
        state.lights.len(),
        state.size,
        if budget == 0 { state.regions.len() } else { budget },
        tpu,
        scene.sun_dir.y.clamp(-1.0, 1.0).asin().to_degrees(),
        daylight,
        LM_EXPOSURE_CEILING,
        (LM_EXPOSURE_CEILING - daylight).max(0.0),
        scale,
        LM_LAMP_GROUND_PEAK * scale,
    );
    if std::env::var_os("MAKEPAD_GPU_LM_REGIONS").is_some() {
        for (ri, r) in state.regions.iter().enumerate() {
            let kind = if matches!(r.kind, RegionKind::Ground) { "ground" } else { "mesh" };
            let sx = (r.max.x - r.min.x).max(1e-4);
            let sz = (r.max.z - r.min.z).max(1e-4);
            log!(
                "lmreg {ri} {kind} rect {} {} {} {} world ({:.2},{:.2},{:.2})..({:.2},{:.2},{:.2}) dens {:.2}x{:.2}",
                r.rect.x, r.rect.y, r.rect.w, r.rect.h,
                r.min.x, r.min.y, r.min.z, r.max.x, r.max.y, r.max.z,
                r.rect.w as f32 / sx, r.rect.h as f32 / sz,
            );
        }
    }
    /// Per-region detail lines one bake may print before it summarizes: a
    /// city of thousands of props must stay readable.
    const DETAIL_LINES: usize = 8;
    let mut printed = 0usize;
    let mut blown = 0usize;
    let (mut worst, mut worst_at) = (0.0f32, 0usize);
    for (ri, r) in state.regions.iter().enumerate() {
        let e = exposure(r);
        if e.sum > worst {
            worst = e.sum;
            worst_at = ri;
        }
        let kind = if matches!(r.kind, RegionKind::Ground) { "ground" } else { "mesh" };
        // LOUD, and never elided: a pool painting more than the documented
        // collar past the frame\'s clip point IS the blown plaza, and this
        // is it naming itself. Everything the eye would have had to guess
        // from a screenshot is on this line.
        if e.clip_texels > LM_LAMP_SAT_TEXELS {
            blown += 1;
            error!(
                "lm bake: region {ri} ({kind}) BLOWS OUT — {:.0} ground texels past the clip \
                 point ({:.0}x the {} texel collar a fixture is allowed), {} lamp(s), \
                 {}x{} texels over world ({:.1},{:.1})..({:.1},{:.1}); daylight {:.3} + lamp \
                 {:.3} = {:.3} > {:.1}. A texel of albedo {:.2} or brighter reaches white. \
                 The daylight rail (lightmap::lamp_daylight_scale) did not hold.",
                e.clip_texels,
                e.clip_texels / LM_LAMP_SAT_TEXELS,
                LM_LAMP_SAT_TEXELS as u32,
                e.lamps,
                r.rect.w, r.rect.h,
                r.min.x, r.min.z, r.max.x, r.max.z,
                daylight, e.lamp_peak, e.sum, LM_EXPOSURE_CEILING,
                (LM_EXPOSURE_CEILING / e.sum.max(1e-4)).clamp(0.0, 1.0),
            );
        } else if e.lamps > 0 && printed < DETAIL_LINES {
            printed += 1;
            log!(
                "lm bake: region {ri} ({kind}) {}x{} texels, {} lamp(s), daylight {:.3} + lamp \
                 {:.3} = {:.3} ({:.0}% of the clip point{})",
                r.rect.w,
                r.rect.h,
                e.lamps,
                daylight,
                e.lamp_peak,
                e.sum,
                100.0 * e.sum / LM_EXPOSURE_CEILING,
                if e.clip_texels > 0.0 {
                    format!(", {:.0} texels in the fixture\'s own collar", e.clip_texels)
                } else {
                    String::new()
                },
            );
        }
    }
    if blown > 0 {
        error!(
            "lm bake: {blown} region(s) blow out — the picture WILL flood white when this bake lands"
        );
    } else {
        log!(
            "lm bake: clean — no region paints past the collar; brightest is region {} at {:.3} \
             of {:.1} ({} detail line(s) shown of {} lamp-lit)",
            worst_at,
            worst,
            LM_EXPOSURE_CEILING,
            printed,
            state.regions.iter().filter(|r| exposure(r).lamps > 0).count(),
        );
    }
}

fn min3(a: Vec3f, b: Vec3f) -> Vec3f {
    v3(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z))
}

fn max3(a: Vec3f, b: Vec3f) -> Vec3f {
    v3(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z))
}

fn sphere_touches_box(c: Vec3f, r: f32, min: Vec3f, max: Vec3f) -> bool {
    let dx = (min.x - c.x).max(0.0).max(c.x - max.x);
    let dy = (min.y - c.y).max(0.0).max(c.y - max.y);
    let dz = (min.z - c.z).max(0.0).max(c.z - max.z);
    dx * dx + dy * dy + dz * dz < r * r
}

/// Fit an ortho sun camera over a receiver's bounds: xy extents from the
/// REGION (plus a texel pad), z reaching back to the whole SCENE toward the
/// sun so every possible caster is inside the volume. `depth_res` is the
/// scratch edge in texels.
fn fit_sun_cam(
    sun_dir: Vec3f,
    r_min: Vec3f,
    r_max: Vec3f,
    s_min: Vec3f,
    s_max: Vec3f,
    depth_res: f32,
) -> SunCam {
    // fwd = the direction sunlight TRAVELS (away from the sun).
    let fwd = (sun_dir * -1.0).normalize();
    let up_hint = if fwd.y.abs() > 0.99 {
        v3(1.0, 0.0, 0.0)
    } else {
        v3(0.0, 1.0, 0.0)
    };
    let right = Vec3f::cross(up_hint, fwd).normalize();
    let up = Vec3f::cross(fwd, right);
    let c = (r_min + r_max) * 0.5;
    let corner = |min: Vec3f, max: Vec3f, i: usize| {
        v3(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        )
    };
    let (mut ex, mut ey) = (0.0f32, 0.0f32);
    let (mut rz_max, mut sz_min) = (f32::MIN, f32::MAX);
    for i in 0..8 {
        let p = corner(r_min, r_max, i) - c;
        ex = ex.max(right.dot(p).abs());
        ey = ey.max(up.dot(p).abs());
        rz_max = rz_max.max(fwd.dot(p));
        let q = corner(s_min, s_max, i) - c;
        sz_min = sz_min.min(fwd.dot(q));
    }
    // Pad: two depth texels + a hand's width, so rim receivers never sample
    // the clamp border.
    let texel = 2.0 * ex.max(ey).max(0.5) / depth_res;
    let pad = texel * 2.0 + 0.05;
    let (ex, ey) = (ex + pad, ey + pad);
    let z0 = sz_min - 0.5;
    let z1 = rz_max + 0.5;
    let zr = (z1 - z0).max(0.01);
    // Tolerance = raster error only. The CPU rays this replaces forgave
    // nothing beyond their 2cm normal offset, and kit models stack detail
    // sheets 2-5cm apart — a fat constant here read every under-sheet as
    // lit where the CPU shadowed it (the crypt's paver floors).
    let bias_world = texel * 1.5 + 0.005;
    SunCam {
        rx: Vec4f {
            x: right.x / ex,
            y: right.y / ex,
            z: right.z / ex,
            w: -right.dot(c) / ex,
        },
        ry: Vec4f {
            x: up.x / ey,
            y: up.y / ey,
            z: up.z / ey,
            w: -up.dot(c) / ey,
        },
        rz: Vec4f {
            x: fwd.x / zr,
            y: fwd.y / zr,
            z: fwd.z / zr,
            w: (-fwd.dot(c) - z0) / zr,
        },
        zr,
        bias01: bias_world / zr,
    }
}

/// A lamp face's view rows + clip tile mapping, matching the gather
/// shader's `face_v` table exactly.
fn lamp_face(lamp: Vec3f, face: usize) -> (Vec4f, Vec4f, Vec4f, Vec4f) {
    let (r, u, f) = match face {
        0 => (v3(0.0, 0.0, -1.0), v3(0.0, 1.0, 0.0), v3(1.0, 0.0, 0.0)),
        1 => (v3(0.0, 0.0, 1.0), v3(0.0, 1.0, 0.0), v3(-1.0, 0.0, 0.0)),
        2 => (v3(1.0, 0.0, 0.0), v3(0.0, 0.0, -1.0), v3(0.0, 1.0, 0.0)),
        3 => (v3(1.0, 0.0, 0.0), v3(0.0, 0.0, 1.0), v3(0.0, -1.0, 0.0)),
        4 => (v3(1.0, 0.0, 0.0), v3(0.0, 1.0, 0.0), v3(0.0, 0.0, 1.0)),
        _ => (v3(-1.0, 0.0, 0.0), v3(0.0, 1.0, 0.0), v3(0.0, 0.0, -1.0)),
    };
    let row = |a: Vec3f| Vec4f {
        x: a.x,
        y: a.y,
        z: a.z,
        w: -a.dot(lamp),
    };
    let (col, rw) = (face % 3, face / 3);
    let tile = Vec4f {
        x: 1.0 / 3.0,
        y: 0.5,
        z: (2.0 * col as f32 + 1.0) / 3.0 - 1.0,
        w: if rw == 0 { 0.5 } else { -0.5 },
    };
    (row(r), row(u), row(f), tile)
}

fn render_tex(cx: &mut Cx, w: usize, h: usize) -> Texture {
    Texture::new_with_format(
        cx,
        TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed {
                width: w,
                height: h,
            },
            initial: true,
        },
    )
}

fn render_tex_rf32(cx: &mut Cx, w: usize, h: usize) -> Texture {
    Texture::new_with_format(
        cx,
        TextureFormat::RenderRf32 {
            size: TextureSize::Fixed {
                width: w,
                height: h,
            },
            initial: true,
        },
    )
}

fn depth_tex(cx: &mut Cx, w: usize, h: usize) -> Texture {
    Texture::new_with_format(
        cx,
        TextureFormat::DepthD32 {
            size: TextureSize::Fixed {
                width: w,
                height: h,
            },
            initial: true,
        },
    )
}

/// Sun-depth scratch edge, texels.
const SUN_DEPTH_RES: usize = 2048;
/// Lamp cube-face tile edge, texels (3x2 tiles in one scratch).
const LAMP_FACE_RES: usize = 512;
/// Chamfer relaxation steps: covers the 4-texel encode band with a step of
/// slack.
const CHAMFER_STEPS: usize = 5;
/// Depth-shader tile transform for a full-target pass (scale 1, offset 0) —
/// the cascade pass retargets the same shaders at one tile of the strip.
const FULL_TILE: Vec4f = Vec4f {
    x: 1.0,
    y: 1.0,
    z: 0.0,
    w: 0.0,
};

impl GpuLightmapBaker {
    pub fn mode(&self) -> GpuLightmapMode {
        self.mode
    }

    /// Effective Realtime CSM budget for this device. Explicit launch-time
    /// `MAKEPAD_CSM_RES` / `MAKEPAD_CSM_FAR` values have final precedence.
    pub fn csm_config(&self) -> CsmConfig {
        self.csm_policy.effective()
    }

    /// Request a device-tier Realtime CSM budget.
    ///
    /// Values are clamped to safe target sizes/ranges. If the effective tile
    /// resolution changes while a scene is realized, both old targets are
    /// released immediately and recreated together at the next draw. The
    /// last fitted frame is also invalidated, so material bindings can never
    /// combine a new inverse resolution with an old target.
    ///
    /// Returns the effective configuration after environment overrides.
    pub fn set_csm_config(&mut self, tile_resolution: usize, far_range: f32) -> CsmConfig {
        let old = self.csm_policy.effective();
        let new = self.csm_policy.set_device(tile_resolution, far_range);
        if old == new {
            return new;
        }

        self.csm_last = None;
        if old.tile_resolution != new.tile_resolution {
            // The strip and depth attachment are one shape-coupled pair.
            // Drop both even if one was unexpectedly absent so the next
            // sync can never assemble a mixed-resolution framebuffer.
            self.csm_tex = None;
            self.csm_depth = None;
        }
        log!(
            "gpu csm: tile {}px, far {:.0} (requested {}px/{:.0})",
            new.tile_resolution,
            new.far_range,
            tile_resolution,
            far_range
        );
        new
    }

    /// Drop the old realm's queued snapshot, realized atlas, and fitted
    /// cascades. Shader objects and render-pass scratch stay resident: they
    /// are device resources, while `pending`/`state`/`csm_last` contain
    /// geometry and bounds belonging to one particular realm.
    pub(crate) fn enter_realm(&mut self) {
        self.pending = None;
        self.state = None;
        self.dirty = false;
        // Regions of a realm that no longer exists must never be baked into
        // the next one's atlas.
        self.bake_queue.clear();
        self.bake_total = 0;
        self.bake_frames = 0;
        self.bake_passes = 0;
        self.bake_us = 0;
        self.csm_last = None;
        self.csm_logged = None;
        self.rt_frames = 0;
        self.rt_us = 0;
    }

    /// DEBUG (idempotence probe): re-bake every region of the CURRENT
    /// layout into the CURRENT targets — no re-plan, no fresh textures.
    /// This is the exact path the accumulator invariant is about.
    pub fn debug_redirty(&mut self) {
        self.dirty = true;
    }

    /// DEBUG: `(regions, lamps)` of the realized layout — the probe waits
    /// for the streamed-in world before it starts measuring.
    pub fn debug_scene_size(&self) -> (usize, usize) {
        match &self.state {
            Some(s) => (s.regions.len(), s.lights.len()),
            None => (0, 0),
        }
    }

    /// DEBUG: each region's atlas rect + whether a lamp reaches it.
    pub fn debug_region_rects(&self) -> Vec<(LmRect, bool)> {
        let Some(state) = &self.state else {
            return Vec::new();
        };
        state
            .regions
            .iter()
            .map(|r| {
                let lit = state
                    .lights
                    .iter()
                    .any(|l| sphere_touches_box(l.pos, l.radius, r.min, r.max));
                (r.rect, lit)
            })
            .collect()
    }

    /// Live mode switch. No re-bake either way: the atlas is static-only
    /// and byte-identical in both modes — the switch only changes which
    /// tier the dynamics cast through and where the materials read sun
    /// visibility (atlas A vs cascades), both applied the same frame.
    pub fn set_mode(&mut self, mode: GpuLightmapMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        log!("gpu lightmap: mode -> {:?}", mode);
    }

    /// Match the expensive realtime targets to the active serving tier.
    /// Called from `run_frame`, where a draw context is available; mode
    /// switches themselves deliberately remain allocation-free.
    ///
    /// Deliberately NOT gated on a realized atlas: the cascades are the
    /// whole dynamic-shadow tier in Realtime and must exist for a world
    /// with no static lightmap.
    fn sync_csm_targets(&mut self, cx: &mut Cx) {
        if self.mode == GpuLightmapMode::Realtime {
            let res = self.csm_policy.effective().tile_resolution;
            if self.csm_tex.is_none() || self.csm_depth.is_none() || self.csm_res != res {
                self.csm_res = res;
                let width = res * CSM_CASCADES;
                self.csm_tex = Some(render_tex_rf32(cx, width, res));
                self.csm_depth = Some(depth_tex(cx, width, res));
            }
        } else {
            // Dropping the last handles lets the backend reclaim ~96 MiB per
            // 2048px view instead of carrying realtime-only memory forever.
            self.csm_tex = None;
            self.csm_depth = None;
            self.csm_last = None;
        }
    }

    /// Queue a new scene snapshot; realized (planned, textures created,
    /// everything dirtied) on the next draw.
    pub fn schedule(&mut self, job: GpuBakeJob) {
        self.pending = Some(job);
    }

    pub fn has_state(&self) -> bool {
        self.state.is_some() || self.pending.is_some()
    }

    /// True when a realized layout exists and nothing is queued: the atlas
    /// content is complete as of the previous frame (debug dumps read here).
    /// A half-drained bake queue is NOT idle — the atlas is correct so far
    /// but not finished, and a dump taken there would show flat props.
    pub fn is_idle(&self) -> bool {
        self.state.is_some()
            && self.pending.is_none()
            && !self.dirty
            && self.bake_queue.is_empty()
    }

    /// How far the static bake has got: `(regions done, regions in the
    /// kick)` while lighting is still filling in, `None` once it has
    /// settled. The app's own status line is the honest place to say
    /// "baking lighting…" — the engine only reports.
    pub fn bake_progress(&self) -> Option<(usize, usize)> {
        let left = self.bake_queue.len();
        if left == 0 {
            return None;
        }
        Some((self.bake_total.saturating_sub(left), self.bake_total))
    }

    /// This frame's cascade binding for the material shaders: the fitted
    /// cascades, the tile-strip texture and one tile's inverse resolution.
    /// None whenever the CSM tier is not serving (OnChange/Off, sun below
    /// the horizon) — the renderer then writes csm off into the uniforms.
    /// A realized atlas is NOT a precondition: a world with no static
    /// lightmap still gets full cascade shadows for its dynamics.
    pub fn csm_binding(&self) -> Option<(CsmFrame, Texture, f32)> {
        let frame = self.csm_last?;
        Some((
            frame,
            self.csm_tex.as_ref()?.clone(),
            1.0 / self.csm_res.max(1) as f32,
        ))
    }

    /// DEBUG: the stage textures worth dumping alongside the atlas. The mask
    /// scratches hold whichever region was processed LAST (see
    /// MAKEPAD_GPU_LM_ONLY_REGION to pin one).
    pub fn debug_stage_textures(&self) -> Vec<(Texture, &'static str)> {
        let Some(state) = &self.state else {
            return Vec::new();
        };
        vec![
            (state.tex.cov.clone(), "cov"),
            (state.tex.lamp_a.clone(), "lamp_a"),
            (state.tex.lamp_b.clone(), "lamp_b"),
            (state.tex.dt_a.clone(), "dt_a"),
            (state.tex.dt_b.clone(), "dt_b"),
            (state.tex.mask_a.clone(), "mask_a"),
            (state.tex.mask_b.clone(), "mask_b"),
            (state.tex.sun_depth.clone(), "sun_depth"),
            // The shadow-top plane pair (final content lands in top_a after
            // the two dilate rings) — the data the occ_g/occ rejection reads.
            (state.tex.top_a.clone(), "top_a"),
            (state.tex.top_b.clone(), "top_b"),
        ]
    }

    /// Realize a pending job: plan the atlas (identical layout math to the
    /// CPU bake), create the persistent + scratch targets, pack the
    /// occluder boxes, upload the heightfield, dirty everything.
    fn realize(&mut self, cx: &mut Cx, job: GpuBakeJob) -> Option<GpuLmDelivery> {
        let (size, rects) = plan_atlas(&job.scene);
        let mesh_count = job.scene.meshes.len();
        let mut regions = Vec::new();
        let mut meshes = Vec::new();
        let mut scene_min = v3(f32::MAX, f32::MAX, f32::MAX);
        let mut scene_max = v3(f32::MIN, f32::MIN, f32::MIN);
        for (i, m) in job.scene.meshes.iter().enumerate() {
            let (lo, hi) = m.world_bounds();
            scene_min = min3(scene_min, lo);
            scene_max = max3(scene_max, hi);
            regions.push(Region {
                rect: rects[i],
                kind: RegionKind::Mesh(i),
                min: lo,
                max: hi,
                tpu: crate::lightmap::chart_density(m, rects[i].w, rects[i].h),
            });
            meshes.push(GpuBakeMesh {
                geometry: job.mesh_geometry[i],
                transform: m.transform,
                min: lo,
                max: hi,
            });
        }
        // Caster-only statics come AFTER the regioned meshes, so the
        // `RegionKind::Mesh(i)` indices above stay valid; every depth loop
        // walks `state.meshes` whole and so shadows from them.
        for m in &job.casters_only {
            scene_min = min3(scene_min, m.min);
            scene_max = max3(scene_max, m.max);
            meshes.push(GpuBakeMesh {
                geometry: m.geometry,
                transform: m.transform,
                min: m.min,
                max: m.max,
            });
        }
        for (bmin, bmax) in &job.scene.boxes {
            scene_min = min3(scene_min, *bmin);
            scene_max = max3(scene_max, *bmax);
        }
        // The ground planar region (at most one — the renderer builds one
        // synthetic field for the whole scene).
        let mut ground = None;
        let mut planar_rects = Vec::new();
        for (i, p) in job.scene.planars.iter().enumerate() {
            let rect = rects[mesh_count + i];
            planar_rects.push(rect);
            let Some(f) = &p.field else {
                continue;
            };
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for h in f.heights.iter() {
                lo = lo.min(*h);
                hi = hi.max(*h);
            }
            if lo > hi {
                continue;
            }
            let gmin = v3(p.x0, lo, p.z0);
            let gmax = v3(p.x1, hi, p.z1);
            scene_min = min3(scene_min, gmin);
            scene_max = max3(scene_max, gmax);
            let heights_tex = Texture::new_with_format(
                cx,
                TextureFormat::VecRf32 {
                    width: f.n,
                    height: f.n,
                    data: Some(f.heights.as_ref().clone()),
                    updated: TextureUpdated::Full,
                },
            );
            // The encode window, exactly bake_top_plane's: base a hair under
            // the lowest ground texel, range with 8 units of blocker
            // headroom over the highest.
            let base = lo - 0.25;
            let range = ((hi + 8.0) - base).max(8.0);
            regions.push(Region {
                rect,
                kind: RegionKind::Ground,
                min: gmin,
                max: gmax,
                tpu: rect.w as f32 / (p.x1 - p.x0).max(0.001),
            });
            ground = Some(GroundInfo {
                world: Vec4f {
                    x: p.x0,
                    y: p.z0,
                    z: p.x1 - p.x0,
                    w: p.z1 - p.z0,
                },
                hf: Vec4f {
                    x: f.origin_x,
                    y: f.origin_z,
                    z: f.cell,
                    w: f.n as f32,
                },
                heights_tex,
                top_base: base,
                top_range: range,
            });
        }
        if regions.is_empty() {
            self.state = None;
            self.dirty = false;
            return None;
        }
        if scene_min.x > scene_max.x {
            scene_min = v3(-1.0, -1.0, -1.0);
            scene_max = v3(1.0, 1.0, 1.0);
        }
        // Occluder boxes as one static world-space triangle soup in the
        // packed mesh layout (position lanes only — the depth shader reads
        // nothing else).
        let box_geometry = if job.scene.boxes.is_empty() {
            None
        } else {
            let mut vertices = Vec::with_capacity(job.scene.boxes.len() * 8 * 7);
            let mut indices: Vec<u32> = Vec::with_capacity(job.scene.boxes.len() * 36);
            for (bmin, bmax) in &job.scene.boxes {
                let base = (vertices.len() / 7) as u32;
                for i in 0..8 {
                    vertices.extend_from_slice(&[
                        if i & 1 == 0 { bmin.x } else { bmax.x },
                        if i & 2 == 0 { bmin.y } else { bmax.y },
                        if i & 4 == 0 { bmin.z } else { bmax.z },
                        0.0,
                        0.0,
                        0.0,
                        0.0,
                    ]);
                }
                // 12 triangles over the corner lattice (winding irrelevant:
                // the depth passes are double-sided).
                const QUADS: [[u32; 4]; 6] = [
                    [0, 2, 6, 4], // -y? (lattice faces; orientation unimportant)
                    [1, 3, 7, 5],
                    [0, 1, 5, 4],
                    [2, 3, 7, 6],
                    [0, 1, 3, 2],
                    [4, 5, 7, 6],
                ];
                for q in QUADS {
                    indices.extend_from_slice(&[base + q[0], base + q[1], base + q[2]]);
                    indices.extend_from_slice(&[base + q[0], base + q[2], base + q[3]]);
                }
            }
            let g = Geometry::new(cx);
            g.update(cx, indices, vertices);
            Some(g)
        };
        // Scratch + persistent targets. The mask scratch fits the largest
        // region at 4x.
        let (mut mw, mut mh) = (4, 4);
        for r in &regions {
            mw = mw.max(r.rect.w * 4);
            mh = mh.max(r.rect.h * 4);
        }
        let tex = BakeTextures {
            atlas: render_tex(cx, size, size),
            top_a: render_tex(cx, size, size),
            top_b: render_tex(cx, size, size),
            cov: render_tex(cx, size, size),
            lamp_a: render_tex(cx, size, size),
            lamp_b: render_tex(cx, size, size),
            dt_a: render_tex(cx, size, size),
            dt_b: render_tex(cx, size, size),
            mask_a: render_tex(cx, mw, mh),
            mask_b: render_tex(cx, mw, mh),
            sun_depth: render_tex_rf32(cx, SUN_DEPTH_RES, SUN_DEPTH_RES),
            sun_depth_z: depth_tex(cx, SUN_DEPTH_RES, SUN_DEPTH_RES),
            sun_far: render_tex_rf32(cx, SUN_DEPTH_RES, SUN_DEPTH_RES),
            lamp_depth: render_tex_rf32(cx, LAMP_FACE_RES * 3, LAMP_FACE_RES * 2),
            lamp_depth_z: depth_tex(cx, LAMP_FACE_RES * 3, LAMP_FACE_RES * 2),
            mask_w: mw,
            mask_h: mh,
        };
        let mesh_rects = rects[..mesh_count].to_vec();
        let delivery = GpuLmDelivery {
            atlas: tex.atlas.clone(),
            top: (
                tex.top_a.clone(),
                ground.as_ref().map_or(0.0, |g| g.top_base),
                ground.as_ref().map_or(8.0, |g| g.top_range),
            ),
            size,
            mesh_rects: mesh_rects.clone(),
            planar_rects: planar_rects.clone(),
            mesh_map: job.mesh_map.clone(),
            terrain_world: job.terrain_world,
        };
        let state = BakeState {
            size,
            regions,
            meshes,
            region_mesh_count: mesh_count,
            box_geometry,
            lights: job.scene.lights.clone(),
            ground,
            scene_min,
            scene_max,
            tex,
        };
        report_bake_exposure(&state, &job.scene, job.trigger, self.bake_budget);
        self.state = Some(state);
        self.dirty = true;
        Some(delivery)
    }

    fn ensure_draws(&mut self, cx: &mut Cx) -> bool {
        if self.draws.is_some() {
            return true;
        }
        let draws = cx.try_with_vm(|vm| {
            Box::new(LmDraws {
                zero: DrawLmZero::script_new_with_default(vm),
                sun_depth: DrawLmSunDepth::script_new_with_default(vm),
                sun_depth_skinned: DrawLmSunDepthSkinned::script_new_with_default(vm),
                lamp_depth: DrawLmLampDepth::script_new_with_default(vm),
                gather_mesh: DrawLmSunGatherMesh::script_new_with_default(vm),
                gather_ground: DrawLmSunGatherGround::script_new_with_default(vm),
                despeckle: DrawLmDespeckle::script_new_with_default(vm),
                downsample: DrawLmDownsample::script_new_with_default(vm),
                chamfer: DrawLmChamfer::script_new_with_default(vm),
                lamp_mesh: DrawLmLampGatherMesh::script_new_with_default(vm),
                lamp_ground: DrawLmLampGatherGround::script_new_with_default(vm),
                lamp_dilate: DrawLmLampDilate::script_new_with_default(vm),
                encode: DrawLmEncode::script_new_with_default(vm),
                top: DrawLmTop::script_new_with_default(vm),
                top_dilate: DrawLmTopDilate::script_new_with_default(vm),
            })
        });
        match draws {
            Some(d) => {
                self.draws = Some(d);
                true
            }
            None => false, // VM held (script-driven draw); retry next frame
        }
    }

    /// Per-frame entry, called by the renderer inside its 3D pass. Realizes
    /// pending jobs and encodes this frame's passes: the whole atlas once
    /// per dirty kick (camera-blind, statics only, both modes), plus — in
    /// Realtime — the cascade depth pass every frame with every caster.
    /// The chain renders before the scene pass that samples it, so nothing
    /// is ever sampled stale. Returns a delivery when a NEW layout was
    /// realized (the renderer stores the atlas + remaps once).
    pub fn run_frame(
        &mut self,
        cx: &mut CxDraw,
        sun_dir: Vec3f,
        static_casters: &[GpuBakeMesh],
        movers: &[GpuLmMover],
        csm_view: Option<&CsmView>,
        eye: Vec3f,
        scene_bounds: Option<(Vec3f, Vec3f)>,
    ) -> Option<GpuLmDelivery> {
        self.csm_last = None;
        if !self.ensure_draws(cx.cx) {
            return None;
        }
        let mut delivery = None;
        if let Some(job) = self.pending.take() {
            delivery = self.realize(cx.cx, job);
        }
        let realtime = self.mode == GpuLightmapMode::Realtime;
        // Deliberately BEFORE any `state` test: the cascade tier is the whole
        // dynamic-shadow contract in Realtime, and a world can legitimately
        // carry no atlas layout at all (flat starter terrain, no props — the
        // renderer's bake kick finds nothing to bake and never schedules a
        // job). Gating this on a realized atlas is what made F8 read as
        // "realtime deletes shadows" in those worlds.
        self.sync_csm_targets(cx.cx);
        let mut batch: Vec<usize> = match self.state.as_ref() {
            Some(state) => {
                let only_region = std::env::var("MAKEPAD_GPU_LM_ONLY_REGION")
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok());
                let kick = self.dirty;
                let batch = schedule_regions(
                    &mut self.dirty,
                    &mut self.bake_queue,
                    state.regions.len(),
                    only_region,
                    self.bake_budget,
                );
                if kick {
                    self.bake_total = batch.len() + self.bake_queue.len();
                    self.bake_frames = 0;
                    self.bake_passes = 0;
                    self.bake_us = 0;
                    if !self.bake_queue.is_empty() {
                        log!(
                            "gpu lightmap: baking {} regions, {} per frame — the world lights flat until it settles",
                            self.bake_total,
                            self.bake_budget
                        );
                    }
                }
                batch
            }
            None => Vec::new(),
        };
        // Realtime: fit this frame's cascades (sun below the horizon fits
        // nothing — the materials' sun term is dead anyway).
        let csm = (realtime && sun_dir.y > 0.02 && self.csm_tex.is_some()).then(|| {
            let range = self.csm_policy.effective().far_range;
            // The z window reaches from the cascade slice BACK to the scene
            // bound toward the sun. With a realized atlas that bound is
            // exact; without one, a range-sized box around the eye is the
            // honest stand-in — it can only make the window longer than
            // needed, never clip a caster out of it.
            let (scene_min, scene_max) = match (scene_bounds, self.state.as_ref()) {
                (Some(bounds), _) => bounds,
                (None, Some(state)) => (state.scene_min, state.scene_max),
                (None, None) => (
                    v3(eye.x - range, eye.y - range, eye.z - range),
                    v3(eye.x + range, eye.y + range, eye.z + range),
                ),
            };
            fit_cascades(
                csm_view,
                eye,
                sun_dir,
                scene_min,
                scene_max,
                range,
                self.csm_res as f32,
            )
        });
        if batch.is_empty() && csm.is_none() {
            return delivery;
        }
        // The ground region must be processed LAST within a batch (its sun
        // depth must survive until the top-plane passes).
        if let Some(state) = self.state.as_ref() {
            batch.sort_by_key(|ri| matches!(state.regions[*ri].kind, RegionKind::Ground));
        }
        let t0 = std::time::Instant::now();
        let mut passes = 0;
        if !batch.is_empty() {
            passes += self.encode_batch(cx, sun_dir, &batch);
        }
        if let Some(frame) = csm {
            passes += self.encode_cascades(cx, static_casters, movers, &frame);
        }
        let us = t0.elapsed().as_micros() as u64;
        self.csm_last = csm;
        if !batch.is_empty() {
            self.bake_frames += 1;
            self.bake_passes += passes;
            self.bake_us += us;
            // One line per BAKE, not per frame: an amortized city is a
            // hundred frames, and a hundred identical log lines is noise
            // that hides the number that matters.
            if self.bake_queue.is_empty() {
                let state = self.state.as_ref().unwrap();
                log!(
                    "gpu lightmap: {}px atlas, {} regions, {} lamps — {} passes encoded over {} frame(s) in {}us (CPU-side submission; GPU renders in-frame)",
                    state.size,
                    state.regions.len(),
                    state.lights.len(),
                    self.bake_passes,
                    self.bake_frames,
                    self.bake_us
                );
            } else if std::env::var_os("MAKEPAD_GPU_LM_PERF").is_some() {
                log!(
                    "gpu lightmap: {} of {} regions baked ({} passes in {}us this frame)",
                    self.bake_total - self.bake_queue.len(),
                    self.bake_total,
                    passes,
                    us
                );
            }
        } else if std::env::var_os("MAKEPAD_GPU_LM_PERF").is_some() {
            self.rt_frames += 1;
            self.rt_us += us;
            if self.rt_frames % 120 == 0 {
                log!(
                    "gpu csm: avg {:.0}us cascade encode/frame ({} movers)",
                    self.rt_us as f64 / 120.0,
                    movers.len()
                );
                self.rt_us = 0;
            }
        }
        delivery
    }

    /// Encode one batch of dirty atlas regions (statics only, both modes) as
    /// one chain of render passes. Returns the number of passes encoded.
    /// Never called with an empty batch; the cascade pass is its own chain
    /// ([`Self::encode_cascades`]) because it must also serve frames — and
    /// whole worlds — where no atlas exists to batch.
    fn encode_batch(&mut self, cx: &mut CxDraw, sun_dir: Vec3f, batch: &[usize]) -> usize {
        let state = self.state.take().unwrap();
        let mut draws = self.draws.take().unwrap();
        let sun_up = sun_dir.y > 0.02;
        let inv_size = 1.0 / state.size as f32;

        // Lamps that touch any batch region.
        let lamps: Vec<usize> = state
            .lights
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                batch.iter().any(|ri| {
                    let r = &state.regions[*ri];
                    sphere_touches_box(l.pos, l.radius, r.min, r.max)
                })
            })
            .map(|(i, _)| i)
            .collect();
        let ground_in_batch = batch
            .iter()
            .any(|ri| matches!(state.regions[*ri].kind, RegionKind::Ground));

        // Atlas pass count: lamp coverage + 4/region + 2/lamp + 3 lamp
        // dilate + (1 + CHAMFER_STEPS) DT + encode + 4 top passes (far
        // depth, convert, two dilate rings).
        let n_passes = 1
            + batch.len() * 4
            + lamps.len() * 2
            + 3
            + 1
            + CHAMFER_STEPS
            + 1
            + if ground_in_batch && state.ground.is_some() {
                4
            } else {
                0
            };
        let lamp_dilate_passes = 3;
        let dt_passes = 1 + CHAMFER_STEPS;
        ensure_pool(&mut self.pool, cx.cx, n_passes);
        // Execution order = DESCENDING pass id (see BakePass::probe).
        let order = pass_order(&self.pool, n_passes);
        let mut seq = PassSeq {
            pool: &mut self.pool,
            order,
            cursor: 0,
        };

        let clear0 = DrawPassClearColor::ClearWith(Vec4f::default());
        let load = |c: Vec4f| DrawPassClearColor::InitWith(c);

        // ---- 1. Lamp accumulator zero + coverage prepass. The accumulator
        // LOADS (a batch owns only its own regions, and an amortized kick
        // spreads the atlas over frames), so this batch's regions are hard-
        // zeroed over their full write footprint FIRST — rect plus pad ring.
        // Only then does the chart rasterize (0,0,0,1) over the texels that
        // hold light. Zeroing by coverage alone leaves every rim texel the
        // chart misses holding the PREVIOUS bake's light, and the dilate
        // spreads it one ring further out on every re-bake (measured: 13142
        // -> 31775 texels, all brighter, over five re-bakes of one settled
        // town).
        if !batch.is_empty() {
            let idx = seq.open(
                cx,
                state.size,
                state.size,
                &state.tex.lamp_a,
                load(Vec4f::default()),
                None
            );
            for rect in batch_zero_rects(&state.regions, batch, state.size) {
                zero_rect(cx, &mut draws.zero, rect, state.size, Vec4f::default());
            }
            for ri in batch {
                let r = &state.regions[*ri];
                match r.kind {
                    RegionKind::Mesh(mi) => {
                        let m = &state.meshes[mi];
                        let d = &mut draws.lamp_mesh;
                        d.transform = m.transform;
                        d.target_a = r.rect.uv_remap(state.size);
                        d.lamp_c = Vec4f {
                            x: 0.0,
                            y: -1.0,
                            z: 0.0,
                            w: 0.0, // coverage mode
                        };
                        d.draw_vars.set_texture(0, &state.tex.lamp_depth);
                        d.draw_vars.geometry_id = Some(m.geometry);
                        if d.draw_vars.can_instance() {
                            cx.add_instance(&d.draw_vars);
                        }
                    }
                    RegionKind::Ground => {
                        let Some(g) = &state.ground else { continue };
                        let d = &mut draws.lamp_ground;
                        d.quad_a = r.rect.uv_remap(state.size);
                        d.ground_a = g.world;
                        d.hf_a = g.hf;
                        d.lamp_c = Vec4f {
                            x: 0.0,
                            y: -1.0,
                            z: 0.0,
                            w: 0.0,
                        };
                        d.draw_vars.set_texture(0, &state.tex.lamp_depth);
                        d.draw_vars.set_texture(1, &g.heights_tex);
                        if d.draw_vars.can_instance() {
                            cx.add_instance(&d.draw_vars);
                        }
                    }
                }
            }
            seq.close(cx, idx);
        }

        // ---- 2. Per region: sun depth + 4x gather + despeckle + coverage
        // downsample. Ground is last in the batch, so its depth scratch
        // content survives for the top-plane passes.
        let mut ground_cam: Option<SunCam> = None;
        for ri in batch {
            let r = &state.regions[*ri];
            let cam = fit_sun_cam(
                sun_dir,
                r.min,
                r.max,
                state.scene_min,
                state.scene_max,
                SUN_DEPTH_RES as f32,
            );
            if matches!(r.kind, RegionKind::Ground) {
                ground_cam = Some(cam);
            }
            // 2a. depth
            {
                let idx = seq.open(
                    cx,
                    SUN_DEPTH_RES,
                    SUN_DEPTH_RES,
                    &state.tex.sun_depth,
                    DrawPassClearColor::ClearWith(Vec4f {
                        x: 1.0,
                        y: 1.0,
                        z: 1.0,
                        w: 1.0
                    }),
                    Some(&state.tex.sun_depth_z)
                );
                // Statics only: the atlas is the STATIC bake in both modes
                // (Realtime dynamics live in the cascades, section 8).
                let d = &mut draws.sun_depth;
                d.sun_rx = cam.rx;
                d.sun_ry = cam.ry;
                d.sun_rz = cam.rz;
                d.flip_a = Vec4f::default();
                d.tile_a = FULL_TILE;
                for m in &state.meshes {
                    d.transform = m.transform;
                    d.draw_vars.geometry_id = Some(m.geometry);
                    if d.draw_vars.can_instance() {
                        cx.add_instance(&d.draw_vars);
                    }
                }
                if let Some(g) = &state.box_geometry {
                    d.transform = Mat4f::identity();
                    d.draw_vars.geometry_id = Some(g.geometry_id());
                    if d.draw_vars.can_instance() {
                        cx.add_instance(&d.draw_vars);
                    }
                }
                seq.close(cx, idx);
            }
            // 2b. gather at 4x into the mask scratch
            let (aw, ah) = (r.rect.w * 4, r.rect.h * 4);
            let target_a = Vec4f {
                x: 0.0,
                y: 0.0,
                z: aw as f32 / state.tex.mask_w as f32,
                w: ah as f32 / state.tex.mask_h as f32,
            };
            {
                let idx = seq.open(
                    cx,
                    state.tex.mask_w,
                    state.tex.mask_h,
                    &state.tex.mask_a,
                    clear0.clone(),
                    None
                );
                match r.kind {
                    RegionKind::Mesh(mi) => {
                        let m = &state.meshes[mi];
                        let d = &mut draws.gather_mesh;
                        d.transform = m.transform;
                        d.sun_rx = cam.rx;
                        d.sun_ry = cam.ry;
                        d.sun_rz = cam.rz;
                        d.target_a = target_a;
                        d.sun_dir_p = Vec4f {
                            x: sun_dir.x,
                            y: sun_dir.y,
                            z: sun_dir.z,
                            w: RAY_OFFSET,
                        };
                        d.params_a = Vec4f {
                            x: cam.bias01,
                            y: if sun_up { 1.0 } else { 0.0 },
                            z: 0.0,
                            w: 0.0,
                        };
                        d.draw_vars.set_texture(0, &state.tex.sun_depth);
                        d.draw_vars.geometry_id = Some(m.geometry);
                        if d.draw_vars.can_instance() {
                            cx.add_instance(&d.draw_vars);
                        }
                    }
                    RegionKind::Ground => {
                        let Some(g) = &state.ground else { continue };
                        let d = &mut draws.gather_ground;
                        d.quad_a = target_a;
                        d.sun_rx = cam.rx;
                        d.sun_ry = cam.ry;
                        d.sun_rz = cam.rz;
                        d.sun_dir_p = Vec4f {
                            x: sun_dir.x,
                            y: sun_dir.y,
                            z: sun_dir.z,
                            w: RAY_OFFSET,
                        };
                        d.params_a = Vec4f {
                            x: cam.bias01,
                            y: if sun_up { 1.0 } else { 0.0 },
                            z: 0.0,
                            w: 0.0,
                        };
                        d.ground_a = g.world;
                        d.hf_a = g.hf;
                        d.draw_vars.set_texture(0, &state.tex.sun_depth);
                        d.draw_vars.set_texture(1, &g.heights_tex);
                        if d.draw_vars.can_instance() {
                            cx.add_instance(&d.draw_vars);
                        }
                    }
                }
                seq.close(cx, idx);
            }
            // 2c. despeckle mask_a -> mask_b
            {
                let idx = seq.open(
                    cx,
                    state.tex.mask_w,
                    state.tex.mask_h,
                    &state.tex.mask_b,
                    clear0.clone(),
                    None
                );
                let d = &mut draws.despeckle;
                d.quad_a = target_a;
                d.src_a = Vec4f {
                    x: 1.0 / state.tex.mask_w as f32,
                    y: 1.0 / state.tex.mask_h as f32,
                    z: aw as f32,
                    w: ah as f32,
                };
                d.draw_vars.set_texture(0, &state.tex.mask_a);
                if d.draw_vars.can_instance() {
                    cx.add_instance(&d.draw_vars);
                }
                seq.close(cx, idx);
            }
            // 2d. downsample mask_b -> coverage atlas (persistent, loads).
            // The downsample covers the rect; the pad ring around it is read
            // by the encode and the dilate's `covf`, so it is zeroed here
            // rather than left holding whatever the last bake wrote.
            {
                let idx = seq.open(
                    cx,
                    state.size,
                    state.size,
                    &state.tex.cov,
                    load(Vec4f::default()),
                    None
                );
                for rect in batch_zero_rects(&state.regions, &[*ri], state.size) {
                    zero_rect(cx, &mut draws.zero, rect, state.size, Vec4f::default());
                }
                let d = &mut draws.downsample;
                d.quad_a = r.rect.uv_remap(state.size);
                d.src_a = Vec4f {
                    x: 1.0 / state.tex.mask_w as f32,
                    y: 1.0 / state.tex.mask_h as f32,
                    z: 0.0,
                    w: 0.0,
                };
                d.rect_a = Vec4f {
                    x: r.rect.x as f32,
                    y: r.rect.y as f32,
                    z: r.rect.w as f32,
                    w: r.rect.h as f32,
                };
                d.draw_vars.set_texture(0, &state.tex.mask_b);
                if d.draw_vars.can_instance() {
                    cx.add_instance(&d.draw_vars);
                }
                seq.close(cx, idx);
            }
        }

        // ---- 3. Lamps: per lamp a 6-face depth render + one additive
        // gather over every batch region in radius.
        for li in &lamps {
            let light = &state.lights[*li];
            // A fixture must not shadow its own bulb, at any placed scale
            // (lightmap::lamp_clearance).
            let clearance = crate::lightmap::lamp_clearance(light);
            {
                let idx = seq.open(
                    cx,
                    LAMP_FACE_RES * 3,
                    LAMP_FACE_RES * 2,
                    &state.tex.lamp_depth,
                    DrawPassClearColor::ClearWith(Vec4f {
                        x: 1.0,
                        y: 1.0,
                        z: 1.0,
                        w: 1.0
                    }),
                    Some(&state.tex.lamp_depth_z)
                );
                let lamp_range = Vec4f {
                    x: clearance,
                    y: light.radius,
                    z: 0.0,
                    w: 0.0,
                };
                for face in 0..6 {
                    let (rx, ry, rz, tile) = lamp_face(light.pos, face);
                    let d = &mut draws.lamp_depth;
                    d.lamp_range = lamp_range;
                    d.face_rx = rx;
                    d.face_ry = ry;
                    d.face_rz = rz;
                    d.tile_a = tile;
                    for m in &state.meshes {
                        if !sphere_touches_box(light.pos, light.radius, m.min, m.max) {
                            continue;
                        }
                        d.transform = m.transform;
                        d.draw_vars.geometry_id = Some(m.geometry);
                        if d.draw_vars.can_instance() {
                            cx.add_instance(&d.draw_vars);
                        }
                    }
                    if let Some(g) = &state.box_geometry {
                        d.transform = Mat4f::identity();
                        d.draw_vars.geometry_id = Some(g.geometry_id());
                        if d.draw_vars.can_instance() {
                            cx.add_instance(&d.draw_vars);
                        }
                    }
                }
                seq.close(cx, idx);
            }
            {
                let idx = seq.open(
                    cx,
                    state.size,
                    state.size,
                    &state.tex.lamp_a,
                    load(Vec4f::default()),
                    None
                );
                let lamp_a = Vec4f {
                    x: light.pos.x,
                    y: light.pos.y,
                    z: light.pos.z,
                    w: light.radius,
                };
                let lamp_b = Vec4f {
                    x: light.color.x,
                    y: light.color.y,
                    z: light.color.z,
                    w: light.spot,
                };
                let lamp_c = Vec4f {
                    x: light.dir.x,
                    y: light.dir.y,
                    z: light.dir.z,
                    w: 1.0, // lamp mode
                };
                let lamp_d = Vec4f {
                    x: clearance,
                    y: light.radius,
                    z: 0.06,
                    w: RAY_OFFSET,
                };
                for ri in batch {
                    let r = &state.regions[*ri];
                    if !sphere_touches_box(light.pos, light.radius, r.min, r.max) {
                        continue;
                    }
                    match r.kind {
                        RegionKind::Mesh(mi) => {
                            let m = &state.meshes[mi];
                            let d = &mut draws.lamp_mesh;
                            d.transform = m.transform;
                            d.target_a = r.rect.uv_remap(state.size);
                            d.lamp_a = lamp_a;
                            d.lamp_b = lamp_b;
                            d.lamp_c = lamp_c;
                            d.lamp_d = lamp_d;
                            d.draw_vars.set_texture(0, &state.tex.lamp_depth);
                            d.draw_vars.geometry_id = Some(m.geometry);
                            if d.draw_vars.can_instance() {
                                cx.add_instance(&d.draw_vars);
                            }
                        }
                        RegionKind::Ground => {
                            let Some(g) = &state.ground else { continue };
                            let d = &mut draws.lamp_ground;
                            d.quad_a = r.rect.uv_remap(state.size);
                            d.ground_a = g.world;
                            d.hf_a = g.hf;
                            d.lamp_a = lamp_a;
                            d.lamp_b = lamp_b;
                            d.lamp_c = lamp_c;
                            d.lamp_d = lamp_d;
                            d.draw_vars.set_texture(0, &state.tex.lamp_depth);
                            d.draw_vars.set_texture(1, &g.heights_tex);
                            if d.draw_vars.can_instance() {
                                cx.add_instance(&d.draw_vars);
                            }
                        }
                    }
                }
                seq.close(cx, idx);
            }
        }

        // ---- 4. Lamp dilate x2 + smooth: lamp_a -> lamp_b -> lamp_a ->
        // lamp_b; encode reads lamp_b. Each destination is zeroed over this
        // batch's footprint first: the dilate itself writes the rect, while
        // the encode reads the rect PLUS the pad ring, and a loading target
        // would hand that ring the previous bake's light.
        for mode in 0..lamp_dilate_passes {
            let (src, dst) = match mode {
                0 => (&state.tex.lamp_a, &state.tex.lamp_b),
                1 => (&state.tex.lamp_b, &state.tex.lamp_a),
                _ => (&state.tex.lamp_a, &state.tex.lamp_b),
            };
            let idx = seq.open(cx, state.size, state.size, dst, load(Vec4f::default()), None);
            for rect in batch_zero_rects(&state.regions, batch, state.size) {
                zero_rect(cx, &mut draws.zero, rect, state.size, Vec4f::default());
            }
            for ri in batch {
                let r = &state.regions[*ri];
                let d = &mut draws.lamp_dilate;
                d.quad_a = r.rect.uv_remap(state.size);
                d.rect_px = Vec4f {
                    x: r.rect.x as f32,
                    y: r.rect.y as f32,
                    z: r.rect.w as f32,
                    w: r.rect.h as f32,
                };
                d.misc_a = Vec4f {
                    x: inv_size,
                    y: mode as f32,
                    z: 0.0,
                    w: 0.0,
                };
                d.draw_vars.set_texture(0, src);
                // Chart coverage (written by this batch's downsample passes,
                // which run earlier in the sequence): the dilate distrusts
                // partially-covered chart-edge texels and rebuilds them from
                // the interior — see DrawLmLampDilate.
                d.draw_vars.set_texture(1, &state.tex.cov);
                if d.draw_vars.can_instance() {
                    cx.add_instance(&d.draw_vars);
                }
            }
            seq.close(cx, idx);
        }

        // ---- 5. Distance transform: seed + CHAMFER_STEPS relaxations,
        // ping-ponging dt_a/dt_b. Scratch — cleared to FAR every pass, so
        // pad texels around each rect decode to "fully lit".
        let far_clear = DrawPassClearColor::ClearWith(Vec4f {
            x: 1.0,
            y: 1.0,
            z: 0.0,
            w: 1.0,
        });
        for step in 0..dt_passes {
            let (src, dst) = if step % 2 == 0 {
                (&state.tex.dt_b, &state.tex.dt_a)
            } else {
                (&state.tex.dt_a, &state.tex.dt_b)
            };
            let idx = seq.open(cx, state.size, state.size, dst, far_clear.clone(), None);
            for ri in batch {
                let r = &state.regions[*ri];
                let d = &mut draws.chamfer;
                d.quad_a = r.rect.uv_remap(state.size);
                d.rect_px = Vec4f {
                    x: r.rect.x as f32,
                    y: r.rect.y as f32,
                    z: r.rect.w as f32,
                    w: r.rect.h as f32,
                };
                d.misc_a = Vec4f {
                    x: inv_size,
                    y: if step == 0 { 0.0 } else { 1.0 },
                    z: 0.0,
                    w: 0.0,
                };
                d.draw_vars.set_texture(0, &state.tex.cov);
                d.draw_vars.set_texture(1, src);
                if d.draw_vars.can_instance() {
                    cx.add_instance(&d.draw_vars);
                }
            }
            seq.close(cx, idx);
        }
        // 1 + CHAMFER_STEPS passes: final result lands in dt_a when the
        // count is odd, dt_b when even.
        let dt_final = if (1 + CHAMFER_STEPS) % 2 == 1 {
            &state.tex.dt_a
        } else {
            &state.tex.dt_b
        };

        // ---- 6. Encode into the atlas (persistent; loads). Quads expanded
        // one texel to stamp the pad ring's default.
        if !batch.is_empty() {
            let idx = seq.open(
                cx,
                state.size,
                state.size,
                &state.tex.atlas,
                load(Vec4f {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0
                }),
                None
            );
            for ri in batch {
                let r = &state.regions[*ri];
                // The same footprint the accumulator was zeroed over — the
                // encode's pad ring may only ever read texels this bake
                // itself defined.
                let ex = r.rect.padded(state.size);
                let d = &mut draws.encode;
                d.quad_a = ex.uv_remap(state.size);
                d.rect_px = Vec4f {
                    x: ex.x as f32,
                    y: ex.y as f32,
                    z: ex.w as f32,
                    w: ex.h as f32,
                };
                let show = std::env::var("MAKEPAD_GPU_LM_SHOW")
                    .ok()
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.0);
                // The world-space sun band in THIS region's texels: the
                // penumbra is a world width, not a texel count, so a shadow
                // edge keeps one softness crossing regions of different
                // density. Floored at 1 texel (a chart cannot resolve
                // narrower) and capped inside the DT's 6-texel reach.
                let band = (crate::lightmap::LM_SUN_BAND_WORLD * r.tpu).clamp(1.0, 5.0);
                d.misc_a = Vec4f {
                    x: inv_size,
                    y: show,
                    z: band,
                    w: 0.0,
                };
                d.draw_vars.set_texture(0, dt_final);
                d.draw_vars.set_texture(1, &state.tex.cov);
                d.draw_vars.set_texture(2, &state.tex.lamp_b);
                if d.draw_vars.can_instance() {
                    cx.add_instance(&d.draw_vars);
                }
            }
            seq.close(cx, idx);
        }

        // ---- 7. Shadow-top plane: convert + two min-dilate rings, only
        // when the ground region was in this batch (its sun depth is still
        // in the scratch — it was processed last).
        if ground_in_batch {
            if let (Some(g), Some(cam)) = (&state.ground, ground_cam) {
                let gr = state
                    .regions
                    .iter()
                    .find(|r| matches!(r.kind, RegionKind::Ground))
                    .unwrap();
                let rect_px = Vec4f {
                    x: gr.rect.x as f32,
                    y: gr.rect.y as f32,
                    z: gr.rect.w as f32,
                    w: gr.rect.h as f32,
                };
                let quad_a = gr.rect.uv_remap(state.size);
                // 7a. FAR sun depth from the ground camera: flipped z01 test
                // keeps the surface nearest the ground along the ray — the
                // CPU top plane's `scene_nearest_block` (a slab records its
                // UNDERSIDE, so the slab's own top escapes its shadow).
                {
                    let idx = seq.open(
                        cx,
                        SUN_DEPTH_RES,
                        SUN_DEPTH_RES,
                        &state.tex.sun_far,
                        DrawPassClearColor::ClearWith(Vec4f {
                            x: 1.0,
                            y: 1.0,
                            z: 1.0,
                            w: 1.0
                        }),
                        Some(&state.tex.sun_depth_z)
                    );
                    let flip = Vec4f {
                        x: 1.0,
                        y: 0.0,
                        z: 0.0,
                        w: 0.0,
                    };
                    // Statics only, like every atlas pass: the top plane
                    // serves the OnChange ground-projection path, and its
                    // blockers are the baked scene's.
                    let d = &mut draws.sun_depth;
                    d.sun_rx = cam.rx;
                    d.sun_ry = cam.ry;
                    d.sun_rz = cam.rz;
                    d.flip_a = flip;
                    d.tile_a = FULL_TILE;
                    for m in &state.meshes {
                        d.transform = m.transform;
                        d.draw_vars.geometry_id = Some(m.geometry);
                        if d.draw_vars.can_instance() {
                            cx.add_instance(&d.draw_vars);
                        }
                    }
                    if let Some(gb) = &state.box_geometry {
                        d.transform = Mat4f::identity();
                        d.draw_vars.geometry_id = Some(gb.geometry_id());
                        if d.draw_vars.can_instance() {
                            cx.add_instance(&d.draw_vars);
                        }
                    }
                    seq.close(cx, idx);
                }
                {
                    let idx = seq.open(
                        cx,
                        state.size,
                        state.size,
                        &state.tex.top_a,
                        load(Vec4f {
                            x: 1.0,
                            y: 1.0,
                            z: 1.0,
                            w: 1.0
                        }),
                        None
                    );
                    let d = &mut draws.top;
                    d.quad_a = quad_a;
                    d.sun_rx = cam.rx;
                    d.sun_ry = cam.ry;
                    d.sun_rz = cam.rz;
                    d.top_a = Vec4f {
                        x: cam.zr,
                        y: sun_dir.y,
                        z: g.top_base,
                        w: g.top_range,
                    };
                    d.params_a = Vec4f {
                        x: if sun_up { 1.0 } else { 0.0 },
                        y: cam.bias01,
                        z: RAY_OFFSET,
                        w: 0.0,
                    };
                    d.ground_a = g.world;
                    d.hf_a = g.hf;
                    d.draw_vars.set_texture(0, &state.tex.atlas);
                    d.draw_vars.set_texture(1, &state.tex.sun_far);
                    d.draw_vars.set_texture(2, &g.heights_tex);
                    if d.draw_vars.can_instance() {
                        cx.add_instance(&d.draw_vars);
                    }
                    seq.close(cx, idx);
                }
                for ring in 0..2 {
                    let (src, dst) = if ring == 0 {
                        (&state.tex.top_a, &state.tex.top_b)
                    } else {
                        (&state.tex.top_b, &state.tex.top_a)
                    };
                    let idx = seq.open(
                        cx,
                        state.size,
                        state.size,
                        dst,
                        load(Vec4f {
                            x: 1.0,
                            y: 1.0,
                            z: 1.0,
                            w: 1.0
                        }),
                        None
                    );
                    let d = &mut draws.top_dilate;
                    d.quad_a = quad_a;
                    d.rect_px = rect_px;
                    d.misc_a = Vec4f {
                        x: inv_size,
                        y: 0.0,
                        z: 0.0,
                        w: 0.0,
                    };
                    d.draw_vars.set_texture(0, src);
                    if d.draw_vars.can_instance() {
                        cx.add_instance(&d.draw_vars);
                    }
                    seq.close(cx, idx);
                }
            }
        }

        let encoded = seq.cursor;
        assert!(encoded <= n_passes, "gpu lightmap pass budget mismatch");
        self.draws = Some(draws);
        self.state = Some(state);
        encoded
    }

    /// Realtime cascades: ONE pass, CSM_CASCADES tiles side by side in the
    /// strip, EVERY caster — static meshes, occluder boxes, rigid movers
    /// and skinned characters (posed from the same joint palette the
    /// visible draw binds, so a character shadows in exactly the pose it
    /// renders in). No CPU per-cascade culling yet: instance encodes are
    /// cheap at village scale and the GPU clips; cull here first if a big
    /// world ever makes this loop hot.
    ///
    /// The static half comes from the realized atlas layout when there IS
    /// one; a world without a layout still encodes its dynamic casters,
    /// which is the whole point of separating this from `encode_batch`.
    fn encode_cascades(
        &mut self,
        cx: &mut CxDraw,
        static_casters: &[GpuBakeMesh],
        movers: &[GpuLmMover],
        frame: &CsmFrame,
    ) -> usize {
        let res = self.csm_res;
        let (Some(csm_tex), Some(csm_z)) = (self.csm_tex.clone(), self.csm_depth.clone()) else {
            return 0;
        };
        let state = self.state.take();
        let mut draws = self.draws.take().unwrap();
        ensure_pool(&mut self.csm_pool, cx.cx, 1);
        let order = pass_order(&self.csm_pool, 1);
        let mut seq = PassSeq {
            pool: &mut self.csm_pool,
            order,
            cursor: 0,
        };
        let idx = seq.open(
            cx,
            res * CSM_CASCADES,
            res,
            &csm_tex,
            DrawPassClearColor::ClearWith(Vec4f {
                x: 1.0,
                y: 1.0,
                z: 1.0,
                w: 1.0,
            }),
            Some(&csm_z),
        );
        for (ci, casc) in frame.cascades.iter().enumerate() {
            let tile = Vec4f {
                x: 1.0 / CSM_CASCADES as f32,
                y: 1.0,
                z: (2.0 * ci as f32 + 1.0) / CSM_CASCADES as f32 - 1.0,
                w: 0.0,
            };
            let d = &mut draws.sun_depth;
            d.sun_rx = casc.rx;
            d.sun_ry = casc.ry;
            d.sun_rz = casc.rz;
            d.flip_a = Vec4f::default();
            d.tile_a = tile;
            if let Some(state) = state.as_ref() {
                // Regioned meshes already live in the realized atlas state.
                // Its caster-only suffix is deliberately skipped here: the
                // upload-time registry below owns that lane in Realtime and
                // carries every material layer rather than only layer zero.
                for m in state.meshes.iter().take(state.region_mesh_count) {
                    d.transform = m.transform;
                    d.draw_vars.geometry_id = Some(m.geometry);
                    if d.draw_vars.can_instance() {
                        cx.add_instance(&d.draw_vars);
                    }
                }
                if let Some(g) = &state.box_geometry {
                    d.transform = Mat4f::identity();
                    d.draw_vars.geometry_id = Some(g.geometry_id());
                    if d.draw_vars.can_instance() {
                        cx.add_instance(&d.draw_vars);
                    }
                }
            }
            for m in static_casters {
                d.transform = m.transform;
                d.draw_vars.geometry_id = Some(m.geometry);
                if d.draw_vars.can_instance() {
                    cx.add_instance(&d.draw_vars);
                }
            }
            for mv in movers.iter().filter(|m| m.skin.is_none()) {
                d.transform = mv.transform;
                d.draw_vars.geometry_id = Some(mv.geometry);
                if d.draw_vars.can_instance() {
                    cx.add_instance(&d.draw_vars);
                }
            }
            for mv in movers {
                let Some(skin) = &mv.skin else { continue };
                let ds = &mut draws.sun_depth_skinned;
                ds.sun_rx = casc.rx;
                ds.sun_ry = casc.ry;
                ds.sun_rz = casc.rz;
                ds.flip_a = Vec4f::default();
                ds.tile_a = tile;
                ds.transform = mv.transform;
                ds.skin_a.x = skin.joint_base;
                ds.draw_vars.set_texture(0, &skin.joint_tex);
                ds.draw_vars.geometry_id = Some(mv.geometry);
                if ds.draw_vars.can_instance() {
                    cx.add_instance(&ds.draw_vars);
                }
            }
        }
        seq.close(cx, idx);
        let encoded = seq.cursor;
        let caster_counts = csm_caster_counts(
            state.as_ref(),
            static_casters.len(),
            movers.len(),
        );
        let statics = caster_counts.0;
        if encoded > 0 && self.csm_logged != Some(caster_counts) {
            log!(
                "gpu csm: first Realtime shadow pass — {} cascades at {}px, {} static + {} live caster(s)",
                CSM_CASCADES,
                res,
                statics,
                movers.len()
            );
            self.csm_logged = Some(caster_counts);
        }
        self.draws = Some(draws);
        self.state = state;
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csm_device_policy_clamps_and_explicit_environment_wins() {
        let mut policy = CsmPolicy {
            device: CsmConfig::default(),
            env_resolution: None,
            env_far_range: None,
        };
        assert_eq!(policy.effective(), DEFAULT_CSM_CONFIG);
        assert_eq!(
            policy.set_device(1, 100_000.0),
            CsmConfig {
                tile_resolution: MIN_CSM_TILE_RESOLUTION,
                far_range: MAX_CSM_FAR_RANGE,
            }
        );

        policy.env_resolution = Some(1536);
        policy.env_far_range = Some(120.0);
        assert_eq!(
            policy.set_device(1024, 48.0),
            CsmConfig {
                tile_resolution: 1536,
                far_range: 120.0,
            },
            "explicit launch overrides must not be silently replaced by XR policy"
        );
    }

    #[test]
    fn changing_csm_budget_invalidates_the_last_fitted_frame() {
        let mut baker = GpuLightmapBaker::default();
        // Do not let a developer's shell override make this unit test depend
        // on its process environment.
        baker.csm_policy.env_resolution = None;
        baker.csm_policy.env_far_range = None;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let texture = Texture::new(&mut cx);
        baker.csm_tex = Some(texture.clone());
        baker.csm_depth = Some(texture);
        baker.csm_last = Some(CsmFrame::default());

        assert_eq!(
            baker.set_csm_config(1024, 48.0),
            CsmConfig {
                tile_resolution: 1024,
                far_range: 48.0,
            }
        );
        assert!(baker.csm_last.is_none());
        assert_eq!(baker.csm_config().tile_resolution, 1024);
        // The shape-coupled pair is released together, so the next sync
        // cannot assemble a mixed-resolution framebuffer.
        assert!(baker.csm_tex.is_none());
        assert!(baker.csm_depth.is_none());
        assert!(
            baker.csm_binding().is_none(),
            "no target means no binding, whatever the last fitted frame was"
        );
    }

    /// The regression this file exists to pin: the cascade tier is NOT a
    /// passenger of the atlas layout. A world with no static lightmap at
    /// all — flat starter terrain, no props, so the renderer never
    /// schedules a bake job — must still allocate cascade targets and hand
    /// the materials a binding in Realtime. When these were fields of
    /// `BakeState`, pressing F8 in such a world produced no cascades, no
    /// binding, and every dynamic shadow vanished instead of upgrading.
    #[test]
    fn the_cascade_tier_serves_a_world_with_no_atlas_layout() {
        let mut baker = GpuLightmapBaker::default();
        baker.csm_policy.env_resolution = None;
        baker.csm_policy.env_far_range = None;
        let mut cx = Cx::new(Box::new(|_, _| {}));
        assert!(baker.state.is_none() && baker.pending.is_none());
        assert!(
            !baker.has_state(),
            "the scenario under test is a world the atlas never realized"
        );

        baker.set_mode(GpuLightmapMode::Realtime);
        baker.sync_csm_targets(&mut cx);
        assert!(
            baker.csm_tex.is_some() && baker.csm_depth.is_some(),
            "Realtime must allocate its cascade targets without an atlas"
        );
        baker.csm_last = Some(CsmFrame::default());
        let (_, _, inv_res) = baker
            .csm_binding()
            .expect("a fitted frame plus a target is a complete binding");
        assert!(
            (inv_res - 1.0 / baker.csm_res as f32).abs() < 1.0e-9,
            "the binding's inverse resolution must match the allocated tile"
        );

        // ...and OnChange still reclaims them, atlas or no atlas.
        baker.set_mode(GpuLightmapMode::OnChange);
        baker.sync_csm_targets(&mut cx);
        assert!(baker.csm_tex.is_none() && baker.csm_depth.is_none());
        assert!(baker.csm_binding().is_none());
    }

    /// Imported editor geometry has no renderer lightmap chart, but its
    /// upload-time static registry is still the complete static half of the
    /// Realtime CSM pass. This is the CPU-side form of the zero-caster
    /// regression: no `BakeState` must not erase registered model layers.
    #[test]
    fn registered_static_layers_do_not_need_an_atlas_state() {
        assert_eq!(csm_caster_counts(None, 9, 0), (9, 0));
        assert_eq!(csm_caster_counts(None, 9, 3), (9, 3));
    }

    /// The mode split is airtight by construction: exactly ONE tier serves
    /// the dynamic casters in each mode. OnChange = prebaked SDF quads only;
    /// Realtime = everything through the per-frame cascades and the SDF
    /// tier draws nothing (both at once would double every shadow, neither
    /// would lose them).
    #[test]
    fn each_mode_serves_dynamics_through_exactly_one_tier() {
        let on_change = dynamic_shadow_tiers(GpuLightmapMode::OnChange);
        assert!(on_change.sdf_quads && !on_change.csm);
        let realtime = dynamic_shadow_tiers(GpuLightmapMode::Realtime);
        assert!(!realtime.sdf_quads && realtime.csm);
        let off = dynamic_shadow_tiers(GpuLightmapMode::Off);
        assert!(!off.sdf_quads && !off.csm);
        for mode in [GpuLightmapMode::OnChange, GpuLightmapMode::Realtime] {
            let t = dynamic_shadow_tiers(mode);
            assert!(
                t.sdf_quads ^ t.csm,
                "{mode:?} must use exactly one dynamic shadow tier"
            );
        }
    }

    /// THE atlas invariant (user directive), BOTH modes: one dirty kick
    /// bakes the whole atlas ONCE, camera-blind; every following frame with
    /// no world edit encodes ZERO atlas passes — Realtime's per-frame work
    /// is the cascades, never the atlas. A regression that re-dirties the
    /// atlas routinely fails here, it doesn't just feel slow.
    #[test]
    fn steady_state_encodes_zero_atlas_passes() {
        let mut dirty = true;
        let mut queue = std::collections::VecDeque::new();
        // The kick: all regions, and a world smaller than the budget still
        // bakes in ONE frame (the small-scene picture is unchanged).
        let first = schedule_regions(&mut dirty, &mut queue, 4, None, DEFAULT_BAKE_BUDGET);
        assert_eq!(first, vec![0, 1, 2, 3]);
        assert!(!dirty, "the kick must consume the dirty bit");
        assert!(queue.is_empty());
        // Steady state: N frames, nothing edits the world — nothing bakes.
        // The scheduler is mode-blind by construction, so this covers
        // Realtime's "zero lightmap sun passes" mirror too.
        for frame in 0..600 {
            let batch = schedule_regions(&mut dirty, &mut queue, 4, None, DEFAULT_BAKE_BUDGET);
            assert!(
                batch.is_empty(),
                "atlas re-baked at steady-state frame {frame}: {batch:?}"
            );
        }
        // The debug pin narrows a kick to one region without unconsuming
        // the bit.
        dirty = true;
        assert_eq!(
            schedule_regions(&mut dirty, &mut queue, 4, Some(2), DEFAULT_BAKE_BUDGET),
            vec![2]
        );
        assert!(
            schedule_regions(&mut dirty, &mut queue, 4, Some(2), DEFAULT_BAKE_BUDGET).is_empty()
        );
    }

    /// The amortization contract: one kick still bakes every region exactly
    /// once, in order, but spread over frames — a city of thousands of lit
    /// props must not encode thousands of passes in the frame the player is
    /// waiting on. And it still ENDS: after the queue drains, steady state
    /// is zero passes forever, budget or no budget.
    #[test]
    fn a_big_kick_is_spread_over_frames_and_still_bakes_everything_once() {
        let mut dirty = true;
        let mut queue = std::collections::VecDeque::new();
        let regions = 3285; // the city that froze for four seconds
        let budget = 24;
        let mut baked = Vec::new();
        let mut frames = 0;
        loop {
            let batch = schedule_regions(&mut dirty, &mut queue, regions, None, budget);
            if batch.is_empty() {
                break;
            }
            assert!(
                batch.len() <= budget,
                "frame {frames} encoded {} regions, over the {budget} budget",
                batch.len()
            );
            frames += 1;
            baked.extend(batch);
        }
        assert_eq!(baked, (0..regions).collect::<Vec<_>>());
        assert_eq!(frames, regions.div_ceil(budget));
        for _ in 0..600 {
            assert!(schedule_regions(&mut dirty, &mut queue, regions, None, budget).is_empty());
        }

        // Budget 0 is the offline escape hatch: the whole kick at once.
        dirty = true;
        assert_eq!(
            schedule_regions(&mut dirty, &mut queue, regions, None, 0).len(),
            regions
        );

        // A NEW kick supersedes a half-drained queue: the layout the old
        // regions indexed is gone, and baking them into the new atlas would
        // light the wrong rects.
        dirty = true;
        let _ = schedule_regions(&mut dirty, &mut queue, 100, None, 10);
        assert_eq!(queue.len(), 90);
        dirty = true;
        let batch = schedule_regions(&mut dirty, &mut queue, 5, None, 10);
        assert_eq!(batch, vec![0, 1, 2, 3, 4]);
        assert!(queue.is_empty());
    }

    /// Progress is what an app puts in front of a player who is looking at a
    /// flat-lit world: it counts up, and it stops existing the moment the
    /// lighting has settled.
    #[test]
    fn bake_progress_reports_only_while_the_lighting_is_filling_in() {
        let mut baker = GpuLightmapBaker::default();
        baker.bake_budget = 10;
        assert!(baker.bake_progress().is_none(), "nothing scheduled, nothing to report");

        baker.dirty = true;
        let batch = schedule_regions(
            &mut baker.dirty,
            &mut baker.bake_queue,
            25,
            None,
            baker.bake_budget,
        );
        baker.bake_total = batch.len() + baker.bake_queue.len();
        assert_eq!(baker.bake_progress(), Some((10, 25)));
        assert!(!baker.is_idle(), "a half-baked atlas is not settled content");

        let _ = schedule_regions(
            &mut baker.dirty,
            &mut baker.bake_queue,
            25,
            None,
            baker.bake_budget,
        );
        assert_eq!(baker.bake_progress(), Some((20, 25)));
        let _ = schedule_regions(
            &mut baker.dirty,
            &mut baker.bake_queue,
            25,
            None,
            baker.bake_budget,
        );
        assert!(baker.bake_progress().is_none());
    }

    fn ground_region(min: Vec3f, max: Vec3f, px: usize) -> Region {
        Region {
            rect: LmRect { x: 1, y: 1, w: px, h: px },
            kind: RegionKind::Ground,
            min,
            max,
            tpu: px as f32 / (max.x - min.x).max(0.001),
        }
    }

    /// RE-BAKE IDEMPOTENCE, at the layer a CPU test can hold: a batch zeroes
    /// exactly what it writes — every region it bakes, over the full
    /// footprint (chart rect + pad ring) the splat, the dilate and the
    /// encode reach — and NOTHING a region outside the batch owns.
    ///
    /// Both halves are load-bearing and both were violated in one direction
    /// or the other:
    /// * too little zero (the shipped bug): the accumulator loads, so a rim
    ///   texel the coverage rasterization missed kept the previous bake's
    ///   light and the dilate carried it one ring further out on every
    ///   re-bake. Measured on a settled 98-region town re-baked into its own
    ///   targets: 13142 -> 20777 -> 25669 -> 29019 -> 31775 texels changed,
    ///   all brighter, none dimmer.
    /// * too much zero: an amortized kick bakes 24 regions a frame, so
    ///   zeroing a region this batch is not baking would blank light that is
    ///   already correct.
    ///
    /// The end-to-end counterpart is MAKEPAD_GPU_LM_REBAKE (renderer.rs),
    /// which re-bakes a live world and diffs the atlas bytes.
    #[test]
    fn a_batch_zeroes_exactly_the_regions_it_bakes() {
        // A packed strip: 10px regions on the pack's 1-texel gutter.
        let size = 64;
        let regions: Vec<Region> = [(1, 1), (13, 1), (1, 13), (13, 13)]
            .iter()
            .map(|(x, y)| Region {
                rect: LmRect { x: *x, y: *y, w: 10, h: 10 },
                kind: RegionKind::Ground,
                min: v3(0.0, 0.0, 0.0),
                max: v3(1.0, 1.0, 1.0),
                tpu: 10.0,
            })
            .collect();
        let batch = vec![0usize, 2];
        let zeros = batch_zero_rects(&regions, &batch, size);

        assert_eq!(zeros.len(), batch.len(), "one footprint per region baked");
        for (slot, ri) in batch.iter().enumerate() {
            let r = regions[*ri].rect;
            let z = zeros[slot];
            // Covers the chart rect...
            assert!(
                z.x <= r.x && z.y <= r.y && z.x + z.w >= r.x + r.w && z.y + z.h >= r.y + r.h,
                "region {ri}: zero {z:?} does not cover its chart {r:?}"
            );
            // ...and exactly the ring the encode stamps around it.
            assert_eq!(z, r.padded(size), "region {ri}: zero is not the write footprint");
        }
        // Regions this batch does not bake keep every texel they own.
        for (ri, region) in regions.iter().enumerate() {
            if batch.contains(&ri) {
                continue;
            }
            for z in &zeros {
                assert!(
                    !z.intersects(&region.rect),
                    "a batch zero {z:?} blanks region {ri} {:?}, which it never re-bakes",
                    region.rect
                );
            }
        }
    }

    /// The bake's own alarm, both ways. A blowout that "pops in" seconds
    /// after a world appears must be attributable from the LOG — so the
    /// region that will flood has to name itself, and a region running
    /// inside its headroom has to stay quiet. (A report that cries wolf is
    /// worse than no report: this is the test that keeps it honest.)
    #[test]
    fn the_bake_reports_the_region_that_will_flood_and_stays_quiet_otherwise() {
        use crate::lightmap::{
            lamp_daylight_scale, lamp_photometry, LM_LAMP_SAT_TEXELS,
        };
        // The reported world: noon over a town-sized ground region, one ×2
        // Kenney lantern in the middle of it.
        let daylight = 0.861; // measured, and what the bake logs at 12.5
        let mount = 1.369 * 2.0;
        let (radius, strength) = lamp_photometry(mount);
        let tint = Vec3f { x: 1.0, y: 0.775, z: 0.475 };
        let region = ground_region(
            v3(-50.0, -1.0, -50.0),
            v3(37.0, 2.0, 37.0),
            1024,
        );
        let tpu = 11.77;

        // WITHOUT the rail: a 0.30 pool against 0.139 of headroom paints
        // far more than the collar a fixture is allowed.
        let blown = vec![LmLight {
            pos: v3(14.0, mount, -5.0),
            color: tint * strength,
            radius,
            dir: v3(0.0, -1.0, 0.0),
            spot: 1.0,
        }];
        let e = region_exposure(&region, &blown, daylight, tpu);
        assert_eq!(e.lamps, 1);
        assert!(e.sum > 1.0, "the reported blowout did not reproduce: {}", e.sum);
        assert!(
            e.clip_texels > LM_LAMP_SAT_TEXELS,
            "the blown street was inside the collar budget: {} texels",
            e.clip_texels
        );

        // WITH it: same lamp, same sun, nothing over the ceiling and
        // nothing to shout about.
        let scale = lamp_daylight_scale(daylight);
        let railed = vec![LmLight {
            color: tint * (strength * scale),
            ..blown[0].clone()
        }];
        let e = region_exposure(&region, &railed, daylight, tpu);
        assert!(e.sum <= 1.0, "the rail did not hold: {}", e.sum);
        assert_eq!(e.clip_texels, 0.0, "a railed pool still clipped the frame");

        // Two lamps 26 m apart do not ADD into a brightness neither of them
        // delivers — the loose bound that made the first cut of this report
        // cry wolf on the ground region.
        let pair = vec![
            railed[0].clone(),
            LmLight { pos: v3(-12.0, mount, 5.0), ..railed[0].clone() },
        ];
        let e2 = region_exposure(&region, &pair, daylight, tpu);
        assert_eq!(e2.lamps, 2);
        assert!(
            (e2.lamp_peak - e.lamp_peak).abs() < 1e-3,
            "distant pools summed: one {} vs two {}",
            e.lamp_peak,
            e2.lamp_peak
        );

        // And a region no lamp reaches is not lamp-lit at all.
        let far = ground_region(v3(400.0, 0.0, 400.0), v3(500.0, 1.0, 500.0), 64);
        let e3 = region_exposure(&far, &pair, daylight, tpu);
        assert_eq!(e3.lamps, 0);
        assert_eq!(e3.lamp_peak, 0.0);
        assert_eq!(e3.sum, daylight);
    }

    #[test]
    fn entering_a_realm_clears_scene_state_but_preserves_mode_and_device_pool() {
        let mut baker = GpuLightmapBaker::default();
        baker.csm_policy.env_resolution = None;
        baker.csm_policy.env_far_range = None;
        baker.set_mode(GpuLightmapMode::Realtime);
        baker.set_csm_config(1024, 48.0);
        baker.dirty = true;
        baker.bake_queue.extend([0, 1, 2]);
        baker.bake_total = 3;
        baker.rt_frames = 9;
        baker.rt_us = 42;
        // A pass cannot be constructed without a draw context, but an empty
        // pool still pins the important ownership rule: enter_realm does not
        // replace the pool allocation or the lazily-created draw shaders.
        let pool_capacity = baker.pool.capacity();

        baker.enter_realm();

        assert_eq!(baker.mode(), GpuLightmapMode::Realtime);
        assert_eq!(
            baker.csm_config(),
            CsmConfig {
                tile_resolution: 1024,
                far_range: 48.0,
            }
        );
        assert!(baker.pending.is_none());
        assert!(baker.state.is_none());
        assert!(baker.csm_last.is_none());
        assert!(!baker.dirty);
        assert!(
            baker.bake_queue.is_empty() && baker.bake_progress().is_none(),
            "regions of the realm we just left must never bake into the next one"
        );
        assert_eq!(baker.rt_frames, 0);
        assert_eq!(baker.rt_us, 0);
        assert_eq!(baker.pool.capacity(), pool_capacity);
    }
}
