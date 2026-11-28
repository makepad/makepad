//! Cascaded probe GI for procedurally built, hot-reloaded worlds.
//!
//! - Scene: a voxel clipmap of the static world (fast_gi/voxels.rs), kept
//!   current on a pool worker by diffing instance snapshots; an edit
//!   re-voxelizes only the bricks it touches, a camera move only the bricks
//!   a window newly covers. No triangle/instance caps.
//! - Probes: up to 3 camera-centred cascades (default 16x8x16 at 1/2/4 m),
//!   addressed toroidally (fast_gi/cascades.rs). Newly exposed slots and
//!   probes near edits trace first. The other required probes refresh in
//!   stride order from their held light after a voxel publication.
//! - GPU: four fragment passes per probe batch. Its size
//!   follows the passes' measured GPU time (fast_gi/shaders.rs).
//! - Receivers sample the finest confident cascade, then coarser ones, then
//!   their ordinary ambient. A probe holds the light the scene's surfaces
//!   send and the share of its rays that left the scene; a receiver's
//!   ambient is its ordinary ambient (the hemisphere, an environment map's
//!   fill) times that open share, plus the surfaces' light. Open space looks
//!   as with GI off, a cavity darkens, a coloured neighbour adds what it
//!   reflects.
//!
//! Every host holds its converged field until transport inputs change.
//! `GiConfig::settle` restricts coverage to probes a surface can read.
//!
//! Needs float colour targets (`GpuInfo::float_color_targets`, reported per
//! device by every backend); a GPU without them stays off and allocates
//! nothing.
use makepad_draw::*;
use crate::cortao::{CortaoCamera, CortaoConfig, CortaoDepth, CortaoInput, CortaoOutput, CortaoPass, CortaoTable};
use crate::volume_fog::VolumeFogCamera;
use std::fmt;
use std::sync::Arc;

#[path = "fast_gi/shaders.rs"]
mod shaders;
#[path = "fast_gi/voxels.rs"]
pub(crate) mod voxels;
#[path = "fast_gi/cascades.rs"]
mod cascades;
#[path = "fast_gi/work.rs"]
mod work;
#[path = "fast_gi/diagnostics.rs"]
mod diagnostics;
use work::{BatchController, FrameTimes, ScatterCache, StageTimes};
pub use shaders::script_mod;
pub use shaders::screen_fill::script_mod as screen_fill_script_mod;
pub(crate) use shaders::{DrawGiTrace, DrawGiRelight, DrawGiGather, DrawGiScatter, DrawGiScreenFill};
pub use voxels::{Instance as GiInstance, Mesh as GiMesh, GiField};
pub(crate) use voxels::{Instance, Mesh, Snapshot};
use cascades::{BatchItem, Cascade};
use voxels::{VoxelLayout, VoxelUpdate, VoxelWorld};

pub const RAYS: usize = 64;
/// Gathered texels per probe: 8x8 irradiance tile, 10x10 moments, 1 info.
pub const GATHER_WIDTH: usize = 64+100+1;
const BAND_PITCH: usize = 32;
const BANDS_PER_BATCH: usize = 42;
const BAND_MIN_ROWS: usize = 4;

/// Spread small batches over at most 42 bands. At a full pitch the rows are dense.
fn band_rows(rows: usize) -> usize {
    rows.div_ceil(BANDS_PER_BATCH).next_power_of_two().clamp(BAND_MIN_ROWS, BAND_PITCH)
}

fn batch_target_height(capacity: usize) -> usize {
    capacity.next_multiple_of(BAND_PITCH).max(BANDS_PER_BATCH * BAND_PITCH)
}

/// Producer rectangles in target texels, including just the live part of the last band.
fn band_quads(rows: usize, band: usize, width: usize) -> impl Iterator<Item = Rect> {
    (0..rows).step_by(band).enumerate().map(move |(index, first)| Rect {
        pos: dvec2(0.0, (index * BAND_PITCH) as f64),
        size: dvec2(width as f64, (rows - first).min(band) as f64),
    })
}

/// Probe tiles per atlas row.
const ATLAS_TILES: usize = 64;
const GI_SCREEN_DIVISOR: usize = 4;
pub const MAX_CASCADES: usize = 3;
pub const MAX_LIGHTS: usize = 8;
/// Where no cascade is confident a receiver keeps its ordinary ambient.
const RECEIVER_FALLBACK_FLOOR: f32 = 1.0;
/// Probe home inside its cell: off the integer planes floors and walls use.
const HOME: [f32; 3] = [0.43, 0.47, 0.41];

/// Logging is a startup opt-in. Cache it so the disabled path does no
/// environment lookup/allocation, formatting, or additional GPU query.
pub(crate) fn gi_stats_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("MAKEPAD_GI_STATS").is_some())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GiMode { #[default] Off, Fast }

/// How the scene's receivers obtain their GI ambient.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GiReceivers {
    /// Gather the probe field per pixel.
    #[default]
    Exact,
    /// Read the cache filled by [`run_gi_screen`](crate::Renderer::run_gi_screen)
    /// on terrain tiles, in whichever model lane draws them. Gather per pixel on a
    /// miss and on every other surface.
    Fast,
}

/// Display-only diagnostics: never feed false colours back into transport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum GiDebug { #[default] Off, Cells, Confidence, Weights, Irradiance, ProbeState, RawProbe, ScreenHits }
impl GiDebug {
    pub fn next(self)->Self {match self {Self::Off=>Self::Cells,Self::Cells=>Self::Confidence,Self::Confidence=>Self::Weights,Self::Weights=>Self::Irradiance,Self::Irradiance=>Self::ProbeState,Self::ProbeState=>Self::RawProbe,Self::RawProbe=>Self::ScreenHits,Self::ScreenHits=>Self::Off}}
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GiConfig {
    /// Probes per cascade (even, 4..16 per axis; x*y*z <= 2048).
    pub grid: [usize; 3],
    /// Cascade-0 probe spacing in metres; each next cascade doubles it.
    pub spacing: f32,
    /// Probe position inside its cell, per axis (0.05..0.95).
    pub home: [f32; 3],
    pub cascades: usize,
    /// Starting probe budget per frame; it then follows `gpu_budget_ms`.
    pub probes_per_frame: usize,
    pub ray_distance: f32,
    pub strength: f32,
    /// Fixed world-space volume center; None follows the game camera.
    pub anchor: Option<Vec3f>,
    /// Previous-field diffuse transport gain. 0 reproduces one bounce.
    pub feedback: f32,
    /// Share of the light a surface receives that it reflects into the
    /// probes (1: physical; 0..1, never more: a gain over 1 lit cavities and
    /// neighbours brighter than their light and tinted them with their own
    /// colour). Emission stays as it is.
    pub bounce: f32,
    /// GI's own GPU share per frame; the sum of its passes aims at 80% of it.
    pub gpu_budget_ms: f32,
    /// Restrict coverage to probes a voxelised surface can read. When false,
    /// every resident probe is required. Both masks sleep after convergence.
    pub settle: bool,
    /// Receivers leave out the probes behind their own surface (a probe's weight fades to zero
    /// as it nears the surface's plane, so no seam marks where it crosses). For a scene whose walls are
    /// thinner than a cascade's voxels (printed shells, sheet parts): a probe inside one sees
    /// its dark inside, and its visibility test, which allows for a voxel's depth, lets it
    /// light the outside through the wall in patches. Off, a probe behind the surface still
    /// counts a little, which softens the probe grid on thick walls (game levels).
    pub thin_walls: bool,
}
impl Default for GiConfig {
    fn default() -> Self { Self { grid:[16,8,16], spacing:1.0, home:HOME, cascades:3, probes_per_frame:128, ray_distance:48.0, strength:1.0, anchor:None, feedback:0.85, bounce:1.0, gpu_budget_ms:2.0, settle:false, thin_walls:false } }
}
impl From<makepad_scene::GiSettings> for GiConfig {
    fn from(settings: makepad_scene::GiSettings) -> Self {
        Self {
            spacing: settings.spacing,
            strength: settings.strength,
            feedback: settings.bounce,
            ..Self::default()
        }
        .clamped()
    }
}

impl GiConfig {
    pub fn clamped(self) -> Self {
        let sane=|x:f32,d:f32,lo:f32,hi:f32|if x.is_finite(){x.clamp(lo,hi)}else{d};
        let even=|x:usize,lo:usize,hi:usize|(x.clamp(lo,hi)+1)&!1;
        let mut grid=[even(self.grid[0],4,16),even(self.grid[1],2,16),even(self.grid[2],4,16)];
        // One field row per probe: stay within WebGL2's 2048 texture minimum.
        grid[1]=grid[1].min((2048/(grid[0]*grid[2]))&!1).max(2);
        Self { grid, home:[0,1,2].map(|a|sane(self.home[a],HOME[a],0.05,0.95)), settle:self.settle, thin_walls:self.thin_walls, cascades:self.cascades.clamp(1,MAX_CASCADES), anchor:self.anchor.filter(|p|finite(*p)), feedback:sane(self.feedback,0.85,0.0,0.95), bounce:sane(self.bounce,1.0,0.0,1.0),
            spacing:sane(self.spacing,1.0,0.001,8.0), probes_per_frame:self.probes_per_frame.clamp(1,4096), ray_distance:sane(self.ray_distance,48.0,0.01,200.0),
            strength:sane(self.strength,1.0,0.0,1.0), gpu_budget_ms:sane(self.gpu_budget_ms,1.5,0.1,16.0) }
    }
    pub fn probe_count(self) -> usize { self.grid.iter().product() }
    /// Field atlas (width, height, tiles per row, tile rows per cascade).
    pub fn atlas(self) -> [usize; 4] {
        let r=ATLAS_TILES.min(self.probe_count());let t=self.probe_count().div_ceil(r);
        [r*10,t*19*self.cascades,r,t]
    }
    pub fn cascade_spacing(self, k: usize) -> f32 { self.spacing*(1u32<<k) as f32 }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GiStats {
    pub triangles: usize,
    pub instances: usize,
    /// Instances outside the snapshot region around the camera.
    pub omitted_instances: usize,
    pub probes: usize,
    /// Probes waiting for a first trace or an edit re-trace.
    pub pending_probes: usize,
    /// Required slots with one or more issues still owed.
    pub unconverged_probes: usize,
    /// GPU-confirmed state counts from `probe_counts_frame`, not scheduled work.
    pub valid_probes: usize,
    pub invalid_probes: usize,
    pub untraced_probes: usize,
    /// GI frame that requested the last completed count (not a GPU submission
    /// tag). None means counts are unavailable, not a measured zero.
    pub probe_counts_frame: Option<u64>,
    /// The completed count followed a full scheduler sweep of the current
    /// scene. Kept with the sample, never inferred from an empty work queue.
    pub probe_counts_swept: bool,
    pub batch: usize,
    pub budget: usize,
    pub traced_rays: usize,
    pub selected_lights: usize,
    pub voxel_jobs: usize,
    pub voxel_bricks: usize,
    pub voxel_ms: f64,
    /// Probes the last window scroll / edit queued.
    pub scroll_probes: usize,
    pub dirty_probes: usize,
    pub snapshot_us: u64,
    pub encode_us: u64,
    /// Immutable scatter instance bytes rebuilt this frame (zero when cached).
    pub static_encode_bytes: usize,
    /// Rolling means over 120 completed frames of trace, relight, gather, scatter.
    /// Collected only with MAKEPAD_GI_STATS; missing measurements are not zero.
    pub gpu_ms: [f64;4],
    /// Maxima over the same 120 completed pass measurements.
    pub gpu_peak_ms: [f64;4],
    /// Mean and maximum of complete per-frame GI pass sums over 120 samples, ms.
    /// These remain available without MAKEPAD_GI_STATS for budget feedback.
    pub frame_sum_mean: f64,
    pub frame_sum_max120: f64,
    /// Maximum complete GI frame sum over the last 600 measured GI frames, ms.
    /// Always collected, independent of MAKEPAD_GI_STATS. Missing frames are excluded;
    /// the current platform feed pairs untagged pass samples by completion order.
    pub frame_sum_max: f64,
    /// Target for the sum of GI's passes: 80% of gpu_budget_ms, in ms.
    pub gpu_aim_ms: f64,
    /// Screen AO passes (raw + 2 blurs), minimum observed.
    pub ao_gpu_ms: f64,
    pub resident_bytes: usize,
    pub uploads_bytes: usize,
    pub building: bool,
    pub rejected_scene: bool,
}

pub(crate) fn point(m:&Mat4f,p:Vec3f)->Vec3f { let q=m.transform_vec4(vec4(p.x,p.y,p.z,1.0)); vec3f(q.x,q.y,q.z) }
pub(crate) fn mul(a:Vec3f,b:Vec3f)->Vec3f {vec3f(a.x*b.x,a.y*b.y,a.z*b.z)}
pub(crate) fn finite(p:Vec3f)->bool {p.x.is_finite()&&p.y.is_finite()&&p.z.is_finite()}

/// Scene identity the host passes with a snapshot (revisions it knows of).
pub(crate) type SceneKey = [u64; 6];

type SceneBounds = (Vec3f, Vec3f);
type VoxelJob = makepad_platform::thread::TaskHandle<(u64, Box<VoxelWorld>, VoxelUpdate, SceneBounds)>;

/// The GPU passes of a frame, by the names the frame timer reports them under.
const STAGE_NAMES:[&str;4]=["GI trace","GI relight","GI gather","GI scatter"];
struct Stage {pass:DrawPass,list:DrawList,times:StageTimes}

/// The perspective view used to fill a GI screen cache, in its receivers' frame.
#[derive(Clone, Copy, Debug)]
pub struct GiScreenView {
    pub(crate) clip: Mat4f,
    pub(crate) eye: [f32; 4],
    pub(crate) rays: [[f32; 4]; 3],
}

impl GiScreenView {
    /// `depth_map` converts sampled depth to clip z as `raw * scale + bias`.
    pub fn from_clip(clip: Mat4f, depth_map: [f32; 2]) -> Option<Self> {
        Self::from_camera(&VolumeFogCamera::from_clip(clip), depth_map)
    }

    fn from_camera(camera: &VolumeFogCamera, depth_map: [f32; 2]) -> Option<Self> {
        // Mat4f::invert also returns identity for a singular clip, which has no eye.
        if camera.eye[3] <= 0.5 { return None; }
        let inv = Mat4f { v: camera.general.world_from_clip };
        let z = 0.5 * depth_map[0] + depth_map[1];
        let unproject = |x, y| {
            let h = inv.transform_vec4(vec4(x, y, z, 1.0));
            let p = h.to_vec3f() / h.w;
            finite(p).then_some(p)
        };
        let p00 = unproject(-1.0, 1.0)?;
        let p10 = unproject(1.0, 1.0)?;
        let p01 = unproject(-1.0, -1.0)?;
        let [x, y, z, _] = camera.eye;
        let rays = [p00 - vec3f(x, y, z), p10 - p00, p01 - p00];
        Some(Self {
            clip: Mat4f { v: camera.general.clip_from_world },
            eye: camera.eye,
            rays: rays.map(|r| [r.x, r.y, r.z, 0.0]),
        })
    }
}

/// A host's RGBA32F GI screen cache, its dimensions in texels and the view it stores.
pub struct GiScreenCache {
    pub texture: Texture,
    pub size: [f32; 2],
    pub view: GiScreenView,
}

struct GiScreenFill {
    pass: DrawPass,
    list: DrawList,
    draw: DrawGiScreenFill,
    target: Texture,
    texel_size: DVec2,
    full_size: DVec2,
    view: Option<(Vec3f, GiScreenView)>,
}
struct Gpu {
    stages:Vec<Stage>, trace:DrawGiTrace, relight:DrawGiRelight, gather:DrawGiGather, scatter:DrawGiScatter,
    field:Texture, hits:Texture, radiance:Texture, update:Texture, batch:Texture,
    /// Info texels of slots that changed cells, zeroed in the next scatter.
    clears:Vec<(usize,usize)>,
    vox:[Texture;3], layout:VoxelLayout, capacity:usize,
    target_height: usize,
    /// Band heights already logged, each a power of two up to 32 and so its own bit.
    logged_bands: u8,
    skyline: Texture,
    skyline_rect: [f32; 4],
    skyline_size: [usize; 2],
    skyline_dirty: bool,
    scatter_cache:ScatterCache,
    encode_key:Option<EncodeKey>, light_key:Option<[f32;14]>,
}

/// Uniform blocks whose inputs change only with configuration or a voxel
/// publication. Keep them in DrawVars between recordings.
#[derive(PartialEq)]
struct EncodeKey {
    config: GiConfig,
    origins: [Option<[i32;3]>; MAX_CASCADES],
    committed: [Option<[i32;3]>; MAX_CASCADES],
    bounds: SceneBounds,
    reload_generation: u64,
}

#[cfg_attr(test, derive(Default))]
#[derive(PartialEq, Eq)]
struct TransportKey {
    sun_and_sky: [[u32; 3]; 4],
    lamp_texels: [[u32; 12]; MAX_LIGHTS],
    /// Shared by the selected lamps. An empty selection samples no shadow map.
    shadow_filter: Option<(bool, u32)>,
    emission_gain: u32,
    linear_albedo: bool,
    feedback: u32,
    bounce: u32,
    ray_distance: u32,
    reload_generation: u64,
    scope_uniforms_gen: [u64; 4],
}

#[derive(Clone, Copy)]
enum WakeCause {
    SunOrSky, Lamps, LampShadows, MoverShadowsCleared, MissingLampRow, Settings, ShaderReload,
    FirstPublication, WindowMove, Recentre, SceneEdit, NewCells,
}

impl WakeCause {
    const ALL: [Self; 12] = [
        Self::SunOrSky, Self::Lamps, Self::LampShadows, Self::MoverShadowsCleared, Self::MissingLampRow, Self::Settings,
        Self::ShaderReload, Self::FirstPublication, Self::WindowMove, Self::Recentre, Self::SceneEdit, Self::NewCells,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::SunOrSky => "sun or sky",
            Self::Lamps => "lamps",
            Self::LampShadows => "lamp shadows",
            Self::MoverShadowsCleared => "mover shadows cleared",
            Self::MissingLampRow => "missing lamp row",
            Self::Settings => "settings",
            Self::ShaderReload => "shader reload",
            Self::FirstPublication => "first publication",
            Self::WindowMove => "window move",
            Self::Recentre => "re-centre",
            Self::SceneEdit => "scene edit",
            Self::NewCells => "new cells",
        }
    }
}

#[derive(Default)]
struct WakeCounts([u32; WakeCause::ALL.len()]);

impl WakeCounts {
    fn record(&mut self, cause: WakeCause) {
        let count = &mut self.0[cause as usize];
        *count = count.saturating_add(1);
    }

    fn count(&self, cause: WakeCause) -> u32 { self.0[cause as usize] }

    fn record_commit(&mut self, commit: &cascades::WindowCommit) {
        if commit.first { self.record(WakeCause::FirstPublication); }
        if commit.moved { self.record(WakeCause::WindowMove); }
        if commit.recentred { self.record(WakeCause::Recentre); }
        if commit.edited { self.record(WakeCause::SceneEdit); }
        if commit.new_cells > 0 { self.record(WakeCause::NewCells); }
    }
}

/// List cause names at wake. Alternate format includes their totals at sleep.
impl fmt::Display for WakeCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for cause in WakeCause::ALL {
            let count = self.count(cause);
            if count == 0 { continue; }
            if !first { f.write_str(", ")?; }
            f.write_str(cause.name())?;
            if f.alternate() { write!(f, " {count}")?; }
            first = false;
        }
        if first { f.write_str("unattributed")?; }
        Ok(())
    }
}

impl TransportKey {
    /// Count each changed group once for this restart, including a dirty face
    /// whose filter changed on the same frame. Only a changed, held key is supplied.
    fn record_restart(&self, previous: Option<&Self>, shadow_changed: bool, missing_row: bool,
        ids: &[f32; MAX_LIGHTS], rows: &[[u32; 12]; MAX_LIGHTS], counts: &mut WakeCounts) {
        if missing_row { counts.record(WakeCause::MissingLampRow); }
        if shadow_changed || previous.is_some_and(|old| self.shadow_filter != old.shadow_filter) {
            counts.record(WakeCause::LampShadows);
        }
        let Some(old) = previous else { return };
        if self.sun_and_sky != old.sun_and_sky { counts.record(WakeCause::SunOrSky); }
        if self.lamp_texels != old.lamp_texels {
            counts.record(WakeCause::Lamps);
            if gi_stats_enabled() {
                let changes = fmt::from_fn(|f| {
                    let mut remaining = old.lamp_texels;
                    let mut entered = Vec::new();
                    for (&id, row) in ids.iter().zip(rows) {
                        if *row == [0; 12] { continue; }
                        if let Some(index) = remaining.iter().position(|old_row| old_row == row) {
                            remaining[index] = [0; 12];
                        } else {
                            entered.push(id as usize);
                        }
                    }
                    let left = remaining.iter().filter(|row| **row != [0; 12]).count();
                    write!(f, "{left} out, {} in (ids {entered:?})", entered.len())
                });
                log!("fast GI: lamps restart: {changes}");
            }
        }
        if self.emission_gain != old.emission_gain || self.linear_albedo != old.linear_albedo
            || self.feedback != old.feedback || self.bounce != old.bounce || self.ray_distance != old.ray_distance {
            counts.record(WakeCause::Settings);
        }
        if self.reload_generation != old.reload_generation || self.scope_uniforms_gen != old.scope_uniforms_gen {
            counts.record(WakeCause::ShaderReload);
        }
    }
}

/// Mesh copies shared across snapshots, keyed by render geometry.
#[derive(Default)]
pub(crate) struct MeshCache {
    pub geometry: std::collections::HashMap<(GeometryId,u64),Vec<(Option<TextureId>,Arc<Mesh>,u64)>>,
    pub shapes: std::collections::HashMap<usize,Arc<Mesh>>,
    pub generation: u64,
}
impl MeshCache {
    pub fn bytes(&self)->usize {
        let mesh=|m:&Mesh|(m.vertices.len()+m.indices.len())*4+m.image.as_ref().map_or(0,|i|i.2.len()*4);
        self.geometry.values().flatten().map(|(_,m,_)|mesh(m)).sum::<usize>()+self.shapes.values().map(|m|mesh(m)).sum::<usize>()
    }
    /// Drop meshes the last snapshot no longer used.
    pub fn evict(&mut self) {
        let g=self.generation;
        for v in self.geometry.values_mut(){v.retain(|e|e.2==g);}
        self.geometry.retain(|_,v|!v.is_empty());
    }
}

pub(crate) struct FastGi {
    scene_time: Option<f32>,
    mode:GiMode, config:GiConfig, debug:GiDebug, linear_albedo:bool, emission_gain:f32,
    gpu:Option<Gpu>,
    probe_counts:diagnostics::ProbeCounts,
    /// Textures of a released GPU state, their storage retired at the next frame
    /// (`flush_retired`): off holds no GPU memory.
    retired:Vec<Texture>,
    /// A host's own cache (`set_screen`), bound only for the host's receivers (`bind_host_screen`).
    screen: Option<GiScreenCache>,
    receivers: GiReceivers,
    screen_fill: Option<Box<GiScreenFill>>,
    pub(crate) last_scene_origin: Vec3f,
    /// Idle voxel builder; None while a job owns it.
    voxels:Option<Box<VoxelWorld>>,
    voxel_job:Option<VoxelJob>,
    generation:u64,
    scene_key:Option<SceneKey>,
    scene_center:Option<Vec3f>,
    pending_scene:Option<(Arc<Snapshot>,SceneBounds)>,
    /// Padded snapshot bounds matching the committed GPU voxel textures.
    scene_bounds:SceneBounds,
    has_scene:bool,
    /// Requested voxel windows (with hysteresis) and those the GPU holds.
    requested:Vec<[i32;3]>,
    committed:Vec<Option<[i32;3]>>,
    cascades:Vec<Cascade>,
    controller:BatchController, frame_times:FrameTimes, batch:Vec<BatchItem>, frame:u64,
    stage_samples: [Vec<f32>; STAGE_NAMES.len()],
    transport_key:Option<TransportKey>,
    /// Held and scratch buffers of selected lamp rows and their faces' three camera rows.
    shadow_faces: [Vec<([u32; 12], [f32; 12])>; 2],
    /// Rows of selected lamps whose mover shadows may have been traced into the field.
    shadow_touched: Vec<[u32; 12]>,
    /// Repaint counter at the last producer recording, until the first idle draw withdraws it.
    producer_recording:Option<u64>,
    /// Since when the field has been issuing probes, and how many it issued (diagnostics only).
    awake:Option<(f64,usize)>,
    wake_counts: WakeCounts,
    /// When a requested window first moved past its committed one, and when its voxel job
    /// was submitted (diagnostics only).
    recentred:Option<(f64,Option<f64>)>,
    /// The host holds a scene newer than the last one submitted.
    scene_stale:bool,
    /// The centre the field last ran for (after the config's anchor); None
    /// before a run or for a non-finite centre.
    center:Option<Vec3f>,
    /// The last voxel job failed (logged): the field draws its fallback and
    /// owes a waiting host nothing until a job succeeds.
    worker_failed:bool,
    pub(crate) meshes:MeshCache,
    /// This view's screen AO, independent of the probe field.
    ao:CortaoPass,
    /// Lazily generated once per ray count (16 and 32).
    ao_tables:[Option<CortaoTable>;2],
    pub stats:GiStats,
}
impl Default for FastGi {
    fn default()->Self{Self{scene_time:None,mode:GiMode::Off,config:GiConfig::default(),debug:GiDebug::Off,linear_albedo:false,emission_gain:1.0,gpu:None,probe_counts:diagnostics::ProbeCounts::default(),retired:Vec::new(),screen:None,receivers:GiReceivers::default(),screen_fill:None,last_scene_origin:Vec3f::default(),voxels:None,voxel_job:None,generation:0,
        scene_key:None,scene_center:None,pending_scene:None,scene_bounds:Default::default(),has_scene:false,requested:Vec::new(),committed:Vec::new(),cascades:Vec::new(),
        controller:BatchController::default(),frame_times:FrameTimes::default(),stage_samples:Default::default(),batch:Vec::new(),frame:0,transport_key:None,shadow_faces:Default::default(),shadow_touched:Vec::new(),producer_recording:None,awake:None,wake_counts:WakeCounts::default(),recentred:None,scene_stale:false,center:None,worker_failed:false,meshes:MeshCache::default(),ao:CortaoPass::default(),ao_tables:[None,None],stats:GiStats::default()}}
}
impl FastGi {
    pub(crate) fn set_scene_time(&mut self, cx: &mut Cx, time: Option<f32>) {
        self.scene_time = time;
        if let Some(gpu) = &self.gpu {
            for stage in &gpu.stages { stage.pass.set_own_time(cx, time); }
        }
        if let Some(pass) = self.probe_counts.pass() { pass.set_own_time(cx, time); }
        if let Some(fill) = &self.screen_fill { fill.pass.set_own_time(cx, time); }
        self.ao.set_scene_time(cx, time);
    }

    pub fn mode(&self)->GiMode{self.mode}
    pub(crate) fn set_receivers(&mut self, receivers: GiReceivers) { self.receivers = receivers; }
    pub(crate) fn receivers(&self) -> GiReceivers { self.receivers }
    pub fn debug(&self)->GiDebug{self.debug}
    pub fn set_debug(&mut self,debug:GiDebug){self.debug=debug;}
    pub fn config(&self)->GiConfig{self.config}
    /// Texel/vertex albedo is sRGB-encoded; decode it in the relight when
    /// the host lights in linear space. A change wakes the probe field.
    pub fn set_linear_albedo(&mut self,on:bool){self.linear_albedo=on;}
    /// The scene lanes' emission gain, so glowing surfaces bounce as bright
    /// as they draw.
    pub fn set_emission_gain(&mut self,gain:f32){self.emission_gain=if gain.is_finite(){gain.max(0.0)}else{1.0};}
    pub fn set_mode(&mut self,mode:GiMode){if self.mode!=mode{self.release();self.mode=mode;}}
    pub fn set_config(&mut self,c:GiConfig){
        let c=c.clamped();
        // Lighting-only controls do not invalidate geometry or the field.
        let transport_only=GiConfig{strength:c.strength,thin_walls:c.thin_walls,feedback:c.feedback,bounce:c.bounce,gpu_budget_ms:c.gpu_budget_ms,probes_per_frame:c.probes_per_frame,ray_distance:c.ray_distance,..self.config};
        if c!=transport_only{self.release();}
        if c.feedback != self.config.feedback || c.bounce != self.config.bounce {
            for cascade in &mut self.cascades { cascade.set_transport(c); }
            if self.cascades.iter().any(|c| c.owed() > 0) { self.wake_counts.record(WakeCause::Settings); }
            // Counts and debt already reflect these settings. Capture all
            // inputs together on the next draw, before issuing any of them.
            self.transport_key = None;
        }
        self.config=c;
    }
    /// A world replacement (script re-eval, hot reload, level change). The
    /// field, voxels and cascades are KEPT: the next snapshot is diffed
    /// against the last one, so a reload re-voxelises the changed bricks
    /// and the new level relights in place.
    pub fn reset(&mut self){
        self.scene_key=None;self.scene_center=None;self.pending_scene=None;
    }
    /// Drop everything (mode or layout change, a replaced GPU device).
    pub(crate) fn release(&mut self){
        if let Some(job)=self.voxel_job.take(){job.cancel();}
        self.generation=self.generation.wrapping_add(1);
        self.probe_counts.release(&mut self.retired);
        if let Some(fill) = self.screen_fill.take() { self.retired.push(fill.target); }
        if let Some(gpu)=self.gpu.take() {
            self.retired.extend([gpu.field, gpu.skyline]);
            self.retired.extend(gpu.vox);
            if gpu.capacity>0 {self.retired.extend([gpu.hits,gpu.radiance,gpu.update,gpu.batch]);}
        }
        self.voxels=None;self.scene_key=None;self.scene_center=None;self.pending_scene=None;self.has_scene=false;
        self.scene_bounds=Default::default();
        self.requested.clear();self.committed.clear();self.cascades.clear();self.controller=BatchController::default();self.frame_times=FrameTimes::default();self.batch.clear();self.transport_key=None;self.producer_recording=None;self.awake=None;self.scene_stale=false;self.center=None;self.worker_failed=false;
        self.meshes=MeshCache::default();self.stats=GiStats::default();
        self.wake_counts = WakeCounts::default();
        self.shadow_faces = Default::default();
        self.shadow_touched.clear();
    }
    /// Frees the storage of what `release` let go.
    pub(crate) fn flush_retired(&mut self,cx:&mut Cx){
        for t in self.retired.drain(..){t.release(cx);}
    }
    fn coarsest_extent(&self)->Vec3f {
        let c=self.config;let s=c.cascade_spacing(c.cascades-1);
        vec3f(c.grid[0]as f32,c.grid[1]as f32,c.grid[2]as f32)*s
    }
    /// The world box a snapshot for `center` must cover: 2.5x the coarsest
    /// window, so the window can drift before a new snapshot is needed.
    pub fn region(&self,center:Vec3f)->(Vec3f,Vec3f) {let e=self.coarsest_extent()*1.25;(center-e,center+e)}
    /// A new snapshot is wanted when the scene changed or the camera left
    /// the last region's safe core. At most one is in flight; later edits
    /// coalesce into the next.
    pub fn wants_scene(&self,key:SceneKey,center:Vec3f)->bool {
        if self.mode==GiMode::Off||self.stats.rejected_scene||self.voxel_job.is_some()||self.pending_scene.is_some(){return false;}
        self.scene_key!=Some(key)||self.left_snapshot_core(center)
    }
    /// The centre is outside the last snapshot's safe core (or there is none).
    fn left_snapshot_core(&self,center:Vec3f)->bool {
        let e=self.coarsest_extent()*0.4;
        self.scene_center.is_none_or(|c|{let d=center-c;d.x.abs()>e.x||d.y.abs()>e.y||d.z.abs()>e.z})
    }
    /// What the field still owes the centre it last ran for before its light sits there: a scene
    /// snapshot (the scene changed, or the centre left the last snapshot's safe core), a voxel job,
    /// windows the GPU does not hold yet, slots a window move exposed and not yet cleared, or probes
    /// waiting for a first trace or an edit's re-trace.
    /// None when none of that is left, and always when the field is off or unsupported, before
    /// its first run, for a non-finite centre, and after a voxel job failed (logged; the field
    /// draws its fallback), so a waiting host is never held for ever. Batches count once
    /// scheduled: nothing here waits on a shader compile. Re-tracing goes on after (history and
    /// bounce take more updates). A host that reveals a view only once its light is in place (a
    /// game behind its loading screen) waits for None.
    pub fn owes(&self)->Option<&'static str> {
        if self.mode!=GiMode::Fast||self.stats.rejected_scene||self.worker_failed{return None;}
        let center=self.center?;
        if self.voxel_job.is_some(){return Some("voxels building");}
        if self.pending_scene.is_some()||self.scene_stale||self.left_snapshot_core(center){return Some("a scene snapshot");}
        if self.requested.is_empty()||self.committed.len()!=self.requested.len()||self.committed.iter().zip(&self.requested).any(|(c,r)|*c!=Some(*r)){return Some("voxel windows");}
        if self.gpu.as_ref().is_some_and(|g|!g.clears.is_empty()){return Some("window clears");}
        if self.stats.pending_probes>0{return Some("first traces");}
        None
    }
    pub fn submit_scene(&mut self,key:SceneKey,center:Vec3f,snapshot:Snapshot){
        self.stats.probe_counts_swept=false;
        self.stats.triangles=snapshot.triangles;self.stats.instances=snapshot.instances.len();
        self.scene_key=Some(key);self.scene_center=Some(center);
        let layout=VoxelLayout::new(self.config);
        let bounds=snapshot.padded_bounds(layout.voxel_size(layout.levels-1));
        self.pending_scene=Some((Arc::new(snapshot),bounds));
    }
    /// Draw while probes, a publication or its clears are due. A final draw
    /// withdraws the last producer recording so it cannot replay at rest.
    pub fn wants_frames(&self)->bool {
        self.mode == GiMode::Fast && !self.stats.rejected_scene
            && (self.pending_scene.is_some() || self.voxel_job.is_some() || self.scene_stale
                || self.producer_recording.is_some() || self.cascades.iter().any(|c| c.owed() > 0)
                || self.gpu.as_ref().is_some_and(|g| !g.clears.is_empty()))
    }
    pub(crate) fn scene_is(&self,key:SceneKey)->bool{self.scene_key==Some(key)}
    pub(crate) fn set_scene_stale(&mut self,stale:bool){self.scene_stale=stale;if stale{self.stats.probe_counts_swept=false;}}
    pub fn relight_pass(&self)->Option<DrawPassId>{self.gpu.as_ref().map(|g|g.stages[1].pass.draw_pass_id())}

    fn ensure(&mut self,cx:&mut Cx)->bool {
        // f32 payload targets and a filterable f16 field: Metal and WebGL2.
        let info=cx.gpu_info();
        if !info.float_color_targets||!(info.float16_blend_targets||cfg!(target_arch="wasm32")) {if !self.stats.rejected_scene{log!("fast GI unavailable: float render targets unsupported");}self.stats.rejected_scene=true;return false;}
        if self.gpu.is_some(){return true;}
        let Some((trace,relight,gather,scatter))=cx.try_with_vm(|vm|(
            DrawGiTrace::script_new_with_default(vm),DrawGiRelight::script_new_with_default(vm),DrawGiGather::script_new_with_default(vm),DrawGiScatter::script_new_with_default(vm)
        ))else{return false;};
        let c=self.config;let layout=VoxelLayout::new(c);
        STAGE_NAMES.iter().for_each(|name|makepad_platform::gpu_frame_timer::watch(name));
        let stages=STAGE_NAMES.into_iter().map(|name|Stage{pass:crate::renderer::new_scene_pass(cx,name,self.scene_time),list:DrawList::new(cx),times:StageTimes::default()}).collect();
        let atlas=c.atlas();
        let field=Texture::new_with_format(cx,TextureFormat::RenderRGBAf16{size:TextureSize::Fixed{width:atlas[0],height:atlas[1]},initial:true});
        field.set_retained_render_target(cx, true);
        let vox=[0,1,2].map(|_|Texture::new_with_format(cx,TextureFormat::VecBGRAu8_32{width:layout.width(),height:layout.height(),data:Some(vec![0;layout.width()*layout.height()]),updated:TextureUpdated::Full}));
        let skyline = Texture::new_with_format(cx, TextureFormat::VecRf32 {
            width: 1, height: 1, data: Some(vec![-1e30]), updated: TextureUpdated::Full,
        });
        self.gpu=Some(Gpu{stages,trace,relight,gather,scatter,field,hits:Texture::new(cx),radiance:Texture::new(cx),update:Texture::new(cx),batch:Texture::new(cx),clears:Vec::with_capacity(c.probe_count()*c.cascades),vox,layout,capacity:0,
            target_height: 0, logged_bands: 0,
            skyline, skyline_rect: [0.0, 0.0, 1.0, 0.0], skyline_size: [1, 1], skyline_dirty: true,
            scatter_cache:ScatterCache::default(),encode_key:None,light_key:None});
        self.cascades=(0..c.cascades).map(|k|Cascade::new(c, k)).collect();
        self.committed=vec![None;c.cascades];
        let capacity=cascades::batch_capacity(c.probe_count(),c.cascades);
        self.batch=Vec::with_capacity(capacity);
        self.controller=BatchController::new(c.probes_per_frame.min(capacity),c.gpu_budget_ms);
        self.ensure_capacity(cx,capacity);
        self.stats.resident_bytes = self.resident_bytes();
        true
    }

    /// Adopt a finished voxel job: copy its changed levels into the voxel
    /// textures, move the cascades onto the new windows, queue edited probes.
    fn poll_voxels(&mut self,cx:&mut Cx) {
        let Some(result)=self.voxel_job.as_mut().and_then(|j|j.try_take()) else {return};
        self.voxel_job=None;self.stats.building=false;
        let (generation,world,mut update,bounds)=match result {
            Ok(r)=>r,
            Err(e)=>{log!("fast GI voxel worker failed: {e:?}");self.scene_key=None;self.worker_failed=true;return;}
        };
        if generation!=self.generation{return;}
        self.worker_failed=false;
        let gpu=self.gpu.as_mut().unwrap();
        let layout=world.layout;
        if let Some(map) = update.skyline.take() {
            let size = [map.width, map.height];
            self.stats.uploads_bytes += map.heights.len() * 4;
            if gpu.skyline_size == size {
                drop(gpu.skyline.take_vec_f32(cx));
                gpu.skyline.put_back_vec_f32(cx, map.heights, None);
            } else {
                let map_texture = Texture::new_with_format(cx, TextureFormat::VecRf32 {
                    width: map.width, height: map.height, data: Some(map.heights), updated: TextureUpdated::Full,
                });
                // The old map may still be bound in a recorded trace draw: retire it like `release` does.
                self.retired.push(std::mem::replace(&mut gpu.skyline, map_texture));
            }
            gpu.skyline_rect = [map.min[0], map.min[1], map.cell, 0.0];
            gpu.skyline_size = size;
            gpu.skyline_dirty = true;
        }
        for (level,rows) in update.rows.iter().enumerate() {
            let Some((first,last))=*rows else{continue};
            let y=level*layout.level_height()+first;let height=last+1-first;
            let (start,len)=(y*layout.width(),height*layout.width());
            for (tex,src) in gpu.vox.iter().zip([&world.albedo,&world.normal,&world.emission]) {
                let mut data=tex.take_vec_u32(cx);
                data[start..start+len].copy_from_slice(&src[start..start+len]);
                tex.put_back_vec_u32(cx,data,Some(RectUsize::new(PointUsize::new(0,y),SizeUsize::new(layout.width(),height))));
            }
            self.stats.uploads_bytes+=len*12;
        }
        let commit = cascades::commit_windows(
            &mut self.cascades, &mut self.committed, &world.windows, &update, self.config, &mut gpu.clears,
        );
        self.stats.scroll_probes = commit.scrolled;
        self.stats.dirty_probes = commit.dirty;
        self.wake_counts.record_commit(&commit);
        self.stats.voxel_jobs+=1;self.stats.voxel_bricks=update.bricks;self.stats.voxel_ms=update.ms;
        if gi_stats_enabled() {
            log!("fast GI voxels: {} bricks / {} triangles in {:.1}ms (worker), {} edit boxes -> {} probes, scroll {} probes, retrace {} probes",update.bricks,update.triangles,update.ms,update.dirty.len(),self.stats.dirty_probes,self.stats.scroll_probes,commit.retraced);
        }
        if let Some((requested, Some(submitted))) = self.recentred {
            self.recentred = None;
            if gi_stats_enabled() {
                let now = Cx::monotonic_now();
                log!("fast GI window: committed {:.1} ms after the move ({:.1} ms to submit, {:.1} ms worker), scroll {} probes",
                    (now - requested) * 1000.0, (submitted - requested) * 1000.0, update.ms, self.stats.scroll_probes);
            }
        }
        self.scene_bounds=bounds;
        self.voxels=Some(world);
    }

    /// Start a voxel job when the scene or a window changed and none runs.
    fn launch_voxels(&mut self,cx:&mut CxDraw,center:Vec3f) {
        let c=self.config;let layout=VoxelLayout::new(c);
        if self.requested.len()!=c.cascades {self.requested=(0..c.cascades).map(|k|layout.window_for(k,center)).collect();}
        // Re-centre a window only after the camera drifted a brick past its
        // centre: no oscillation at a brick boundary.
        for k in 0..c.cascades {
            let v=layout.voxel_size(k);let w=self.requested[k];
            let p=[center.x/v,center.y/v,center.z/v];
            if (0..3).any(|a|(p[a]-(w[a] as f32+layout.dims[a] as f32*0.5)).abs()>voxels::BRICK as f32*1.5){
                self.requested[k]=layout.window_for(k,center);
                self.recentred.get_or_insert((Cx::monotonic_now(),None));
            }
        }
        if self.voxel_job.is_some()||(!self.has_scene&&self.pending_scene.is_none()){return;}
        let windows_changed=self.voxels.as_ref().is_none_or(|w|w.windows.iter().zip(&self.requested).any(|(a,b)|*a!=Some(*b)));
        if self.pending_scene.is_none()&&!windows_changed{return;}
        self.stats.building=true;
        // The first world (its 4.7 MB mirror) is allocated on the worker.
        let world=self.voxels.take();let settle=c.settle;
        let (scene,bounds)=match self.pending_scene.take() {
            Some((scene,bounds))=>(Some(scene),bounds),
            None=>(None,self.scene_bounds),
        };
        self.has_scene=true;
        let windows=self.requested.clone();let generation=self.generation;
        match cx.task_pool().submit(makepad_platform::thread::Class::Visible, move||{
            let mut world=world.unwrap_or_else(||Box::new(VoxelWorld::new(layout)));
            world.track_needed=settle;
            let update=world.update(scene,&windows);(generation,world,update,bounds)
        }) {
            Ok(job) => {
                self.voxel_job=Some(job);
                if let Some((_, submitted @ None)) = &mut self.recentred { *submitted = Some(Cx::monotonic_now()); }
            }
            Err(error) => { self.stats.building=false; self.worker_failed=true; log!("fast GI: {error}"); }
        }
    }

    fn ensure_capacity(&mut self,cx:&mut Cx,capacity:usize) {
        let gpu=self.gpu.as_mut().unwrap();
        let height = batch_target_height(capacity);
        if capacity<=gpu.capacity && height<=gpu.target_height{return;}
        let cap=capacity.next_power_of_two().max(16);
        let target=|cx:&mut Cx,w|Texture::new_with_format(cx,TextureFormat::RenderRGBAf32{size:TextureSize::Fixed{width:w,height},initial:true});
        gpu.hits=target(cx,RAYS+1);gpu.radiance=target(cx,RAYS);
        gpu.update=Texture::new_with_format(cx,TextureFormat::RenderRGBAf16{size:TextureSize::Fixed{width:GATHER_WIDTH,height},initial:true});
        gpu.batch=Texture::new_with_format(cx,TextureFormat::VecRGBAf32{width:2,height:cap,data:Some(vec![0.0;cap*8]),updated:TextureUpdated::Full});
        gpu.capacity=cap;
        gpu.target_height=height;
        gpu.encode_key=None;
        self.stats.static_encode_bytes+=gpu.scatter_cache.prepare(&mut gpu.scatter,height);
    }

    fn cascade_uniforms(cx:&Cx,dv:&mut DrawVars,c:GiConfig,cascades:&[Cascade]) {
        dv.set_uniform(cx,live_id!(gi_grid),&[c.grid[0]as f32,c.grid[1]as f32,c.grid[2]as f32,c.probe_count()as f32]);
        for (k,name) in [live_id!(gi_c0),live_id!(gi_c1),live_id!(gi_c2)].into_iter().enumerate() {
            let v=match cascades.get(k).and_then(|x|x.origin){Some(o)=>[o[0]as f32,o[1]as f32,o[2]as f32,c.cascade_spacing(k)],None=>[0.0;4]};
            dv.set_uniform(cx,name,&v);
        }
        dv.set_uniform(cx,live_id!(gi_home),&[c.home[0],c.home[1],c.home[2],cascades.len()as f32]);
        let a=c.atlas();
        dv.set_uniform(cx,live_id!(gi_atlas),&[a[0]as f32,a[1]as f32,a[2]as f32,a[3]as f32]);
    }
    fn voxel_uniforms(cx:&Cx,dv:&mut DrawVars,l:VoxelLayout,vox:&[Texture;3],committed:&[Option<[i32;3]>],bounds:SceneBounds) {
        dv.set_uniform(cx,live_id!(gi_vox_dims),&[l.dims[0]as f32,l.dims[1]as f32,l.dims[2]as f32,l.levels as f32]);
        dv.set_uniform(cx,live_id!(gi_vox_tex),&[l.width()as f32,l.height()as f32,l.level_height()as f32,voxels::TILE_COLUMNS as f32]);
        let (lo,hi)=bounds;
        dv.set_uniform(cx,live_id!(gi_scene_min),&[lo.x,lo.y,lo.z]);
        dv.set_uniform(cx,live_id!(gi_scene_max),&[hi.x,hi.y,hi.z]);
        for (k,name) in [live_id!(gi_vox0),live_id!(gi_vox1),live_id!(gi_vox2)].into_iter().enumerate() {
            let v=match committed.get(k).copied().flatten(){Some(o)=>[o[0]as f32,o[1]as f32,o[2]as f32,l.voxel_size(k)],None=>[0.0;4]};
            dv.set_uniform(cx,name,&v);
        }
        for (name,t) in [live_id!(gi_vox_a),live_id!(gi_vox_b),live_id!(gi_vox_c)].into_iter().zip(vox){bind_texture(cx,dv,name,t);}
    }

    /// Compare the effective producer inputs even when the last batch drained.
    fn update_transport(&mut self, cx: &mut Cx, center: Vec3f, sun: &crate::sun::SunLight,
        clustered: &crate::clustered::ClusteredLights) -> [f32; MAX_LIGHTS] {
        let ids = clustered.gi_light_ids(center);
        let table = clustered.gi_light_texels(cx);
        let mut missing_row = false;
        let lamp_texels = ids.map(|id| {
            if id < 0.0 { return [0; 12]; }
            let offset = id as usize * 12;
            if let Some(row) = table.get(offset..offset + 12) {
                std::array::from_fn(|i| row[i].to_bits())
            } else {
                missing_row = true;
                [0; 12]
            }
        });
        let shadows = &clustered.shadows;
        let [held_faces, next_faces] = &mut self.shadow_faces;
        next_faces.clear();
        let mut shadow_changed = false;
        let mut mover_rows = [[0; 12]; MAX_LIGHTS];
        let mut mover_count = 0;
        let mut mover_shadows_cleared = false;
        for (id, texels) in ids.into_iter().zip(&lamp_texels) {
            if id < 0.0 { break; }
            let (static_dirty, mover_dirty, faces) = shadows.gi_faces(id as usize);
            shadow_changed |= static_dirty;
            if mover_dirty {
                mover_rows[mover_count] = *texels;
                mover_count += 1;
            } else if !static_dirty && self.shadow_touched.contains(texels) {
                mover_shadows_cleared = true;
            }
            next_faces.extend(faces.map(|rows| (*texels, *rows)));
        }
        let face_key = |(lamp, rows): &([u32; 12], [f32; 12])| (*lamp, rows.map(f32::to_bits));
        next_faces.sort_unstable_by_key(face_key);
        shadow_changed |= next_faces.iter().map(face_key).ne(held_faces.iter().map(face_key));
        std::mem::swap(held_faces, next_faces);
        let mut sorted_texels = lamp_texels;
        sorted_texels.sort_unstable();
        let scope_uniforms_gen = self.gpu.as_mut().map_or([0; 4], |g| {
            [&mut g.trace.quad.draw_vars, &mut g.relight.quad.draw_vars,
             &mut g.gather.quad.draw_vars, &mut g.scatter.quad.draw_vars].map(|vars| {
                vars.refresh_after_reload(cx);
                vars.draw_shader_id.map_or(0, |id| {
                    let mapping = &cx.draw_shaders[id.index].mapping;
                    if mapping.table_consts.is_empty() { 0 } else { mapping.scope_uniforms_gen }
                })
            })
        });
        let key = TransportKey {
            sun_and_sky: [sun.dir, sun.color, sun.sky, sun.ground].map(|v| [v.x, v.y, v.z].map(f32::to_bits)),
            lamp_texels: sorted_texels,
            shadow_filter: (!held_faces.is_empty()).then_some((shadows.soft_filter, shadows.source_radius.to_bits())),
            emission_gain: self.emission_gain.to_bits(), linear_albedo: self.linear_albedo,
            feedback: self.config.feedback.to_bits(), bounce: self.config.bounce.to_bits(),
            ray_distance: self.config.ray_distance.to_bits(), reload_generation: cx.reload_generation,
            scope_uniforms_gen,
        };
        let changed = self.transport_key.as_ref() != Some(&key);
        let restart = missing_row || shadow_changed || mover_shadows_cleared || (changed && self.transport_key.is_some());
        if restart {
            for cascade in &mut self.cascades { cascade.restart(); }
        }
        let mover_rows = &mover_rows[..mover_count];
        if self.cascades.iter().any(|c| c.owed() > 0) {
            if restart {
                let previous = self.transport_key.as_ref().filter(|_| changed);
                key.record_restart(previous, shadow_changed, missing_row, &ids, &lamp_texels, &mut self.wake_counts);
                if mover_shadows_cleared { self.wake_counts.record(WakeCause::MoverShadowsCleared); }
            }
            self.shadow_touched.clear();
            self.shadow_touched.extend_from_slice(mover_rows);
        } else {
            // A drained field keeps only marks from earlier probe issues.
            self.shadow_touched.retain(|row| mover_rows.contains(row));
        }
        if changed { self.transport_key = Some(key); }
        ids
    }

    pub fn run(&mut self,cx:&mut CxDraw,center:Vec3f,sun:&crate::sun::SunLight,clustered:&crate::clustered::ClusteredLights) {
        self.probe_counts.retry(cx.cx);
        if self.mode==GiMode::Off{return;}
        let center=self.config.anchor.unwrap_or(center);
        let previous_batch=self.stats.batch;
        let start=Cx::monotonic_now();self.stats.traced_rays=0;self.stats.uploads_bytes=0;self.stats.batch=0;self.stats.static_encode_bytes=0;
        if !finite(center){self.center=None;return;}
        if !self.ensure(cx.cx){return;}
        let changed_center = self.center != Some(center);
        self.center=Some(center);
        self.frame=self.frame.wrapping_add(1);
        if changed_center || self.voxel_job.is_some() || self.pending_scene.is_some() || self.worker_failed {
            self.poll_voxels(cx.cx);
            self.launch_voxels(cx,center);
        }
        let c=self.config;let count=c.probe_count();
        let light_ids = self.update_transport(cx.cx, center, sun, clustered);
        let active = self.cascades.iter().any(|c| c.owed() > 0);
        let clears = !self.gpu.as_ref().unwrap().clears.is_empty();
        self.stats.probes=count*self.cascades.len();
        self.stats.gpu_aim_ms=BatchController::aim(c.gpu_budget_ms);
        // A draw can repeat before the frame paints. Preserve its last scatter
        // until the backend has had a repaint, then withdraw the four passes once.
        if !active && !clears && self.producer_recording.is_none_or(|recorded| recorded == cx.repaint_id()) {
            self.stats.encode_us = 0;
            return;
        }
        // HAL currently exposes only name-based samples. Consume every arrival, including delayed ones;
        // only equally sized streams can supply complete frame sums.
        // HAL publishes these streams together on the UI thread; take them
        // before processing any further completions. GI issues four render
        // passes: voxel changes are texture uploads, not a fifth GI pass.
        let samples = &mut self.stage_samples;
        for (stage, out) in STAGE_NAMES.into_iter().zip(samples.iter_mut()) {
            out.clear();
            makepad_platform::gpu_frame_timer::drain_watched(stage, out);
        }
        let sample=self.frame_times.push(std::array::from_fn(|i|samples[i].as_slice()),previous_batch);
        if gi_stats_enabled() {
            for (i,stage) in self.gpu.as_mut().unwrap().stages.iter_mut().enumerate() {
                for &ms in &samples[i] {stage.times.record(ms as f64,previous_batch);}
                let (mean,peak,_)=stage.times.stats();
                self.stats.gpu_ms[i]=mean;self.stats.gpu_peak_ms[i]=peak;
            }
        }
        let (mean,peak,cost)=self.frame_times.stats();
        self.stats.frame_sum_mean=mean;self.stats.frame_sum_max120=peak;
        self.stats.frame_sum_max=self.frame_times.max600();
        // With a slowly changing batch, the previous submitted size is an
        // approximation for async samples. Missing samples freeze the rate.
        self.controller.advance(start,if sample&&previous_batch>0{cost}else{None},active);
        self.stats.budget=self.controller.cap(c.gpu_budget_ms,cascades::batch_capacity(count,c.cascades));
        cascades::schedule_after_clears(&mut self.cascades,self.stats.budget,!c.settle,clears,&mut self.batch);
        self.stats.pending_probes=self.cascades.iter().map(Cascade::pending).sum();
        self.stats.unconverged_probes=self.cascades.iter().map(Cascade::owed).sum();
        if self.batch.is_empty() && !clears {
            for stage in &self.gpu.as_ref().unwrap().stages { stage.pass.withdraw(cx.cx); }
            self.producer_recording = None;
            self.stats.encode_us = 0;
            if let Some((since, issued)) = self.awake.take() {
                if gi_stats_enabled() {
                    log!("fast GI: asleep after {:.0} ms awake, {} probe issues (budget {}); restarts: {:#}",
                        (start - since) * 1000.0, issued, self.stats.budget, self.wake_counts);
                }
                self.wake_counts = WakeCounts::default();
            }
            return;
        }
        let awake = self.awake.get_or_insert_with(|| {
            if gi_stats_enabled() { log!("fast GI: awake: {}", self.wake_counts); }
            (start, 0)
        });
        awake.1 += self.batch.len();
        self.encode(cx,light_ids,sun,clustered);
        self.producer_recording = Some(cx.repaint_id());
        self.stats.batch=self.batch.len();self.stats.traced_rays=self.batch.len()*RAYS;
        self.stats.resident_bytes=self.resident_bytes();
        self.stats.encode_us=((Cx::monotonic_now()-start)*1e6)as u64;
    }

    fn resident_bytes(&self)->usize {
        let Some(gpu)=&self.gpu else{return 0};
        let l=gpu.layout;
        let a=self.config.atlas();
        a[0]*a[1]*8+2*gpu.capacity*16+((2*RAYS+1)*16+GATHER_WIDTH*8)*gpu.target_height
            +gpu.skyline_size[0]*gpu.skyline_size[1]*4*2
            +l.width()*l.height()*4*3*2+self.meshes.bytes()+gpu.scatter_cache.instances.as_ref().map_or(0,|r|r.byte_len()*2)
    }

    fn encode(&mut self,cx:&mut CxDraw,light_ids:[f32;MAX_LIGHTS],sun:&crate::sun::SunLight,clustered:&crate::clustered::ClusteredLights) {
        let c=self.config;
        let batch=&self.batch;
        let rows=batch.len();
        if rows > 0 {
            let gpu=self.gpu.as_mut().unwrap();
            let mut data=gpu.batch.take_vec_f32(cx.cx);
            for (i,b) in batch.iter().enumerate() {
                data[i*8..i*8+8].copy_from_slice(&work::batch_row(b));
            }
            gpu.batch.put_back_vec_f32(cx.cx,data,Some(RectUsize::new(PointUsize::new(0,0),SizeUsize::new(2,rows))));
        }
        self.stats.uploads_bytes+=rows*32;
        self.stats.selected_lights=light_ids.iter().filter(|&&i|i>=0.0).count();
        let gpu=self.gpu.as_mut().unwrap();
        let cap=gpu.capacity;
        let height=gpu.target_height;
        let band=band_rows(rows);
        // Once per band height: a batch that hovers at a switch point changes it every few seconds,
        // and the change itself is only this uniform and the number of band quads.
        if rows > 0 && gpu.logged_bands & band as u8 == 0 {
            log!("[gi] batch rows {rows}: bands of {band} at pitch {BAND_PITCH}, physical rows {height}");
            gpu.logged_bands |= band as u8;
        }
        let pass_uniform=[band as f32,height as f32,c.ray_distance,(self.frame%4096)as f32];
        let linear=self.linear_albedo;let emission_gain=self.emission_gain;
        let key=EncodeKey{config:c,origins:std::array::from_fn(|k|self.cascades.get(k).and_then(|c|c.origin)),committed:std::array::from_fn(|k|self.committed.get(k).copied().flatten()),bounds:self.scene_bounds,reload_generation:cx.reload_generation};
        let light_key=[sun.dir.x,sun.dir.y,sun.dir.z,sun.color.x,sun.color.y,sun.color.z,sun.sky.x,sun.sky.y,sun.sky.z,sun.ground.x,sun.ground.y,sun.ground.z,emission_gain,if linear{1.0}else{0.0}];
        let reloaded=gpu.encode_key.as_ref().is_some_and(|k|k.reload_generation!=key.reload_generation);
        if reloaded {
            gpu.scatter_cache=ScatterCache::default();
            self.stats.static_encode_bytes+=gpu.scatter_cache.prepare(&mut gpu.scatter,height);
        }
        let static_dirty=gpu.encode_key.as_ref()!=Some(&key);
        let light_dirty=static_dirty||gpu.light_key!=Some(light_key);
        gpu.encode_key=Some(key);gpu.light_key=Some(light_key);
        let mut cx2=Cx2d::new(cx);
        for i in 0..4 {
            {
                let FastGi{gpu,cascades,committed,scene_bounds,..}=self;
                let gpu=gpu.as_mut().unwrap();
                let (batch_tex,field,hits,radiance,update,vox,layout)=(gpu.batch.clone(),gpu.field.clone(),gpu.hits.clone(),gpu.radiance.clone(),gpu.update.clone(),gpu.vox.clone(),gpu.layout);
                let dv=match i{0=>&mut gpu.trace.quad.draw_vars,1=>&mut gpu.relight.quad.draw_vars,2=>&mut gpu.gather.quad.draw_vars,_=>&mut gpu.scatter.quad.draw_vars};
                dv.set_uniform(cx2.cx,live_id!(gi_pass),&pass_uniform);
                if static_dirty {
                    dv.set_uniform(cx2.cx,live_id!(gi_batch_height),&[cap as f32]);
                    bind_texture(cx2.cx,dv,live_id!(gi_batch),&batch_tex);
                    if i<3 {Self::cascade_uniforms(cx2.cx,dv,c,cascades);}
                }
                if i<2&&static_dirty {Self::voxel_uniforms(cx2.cx,dv,layout,&vox,committed,*scene_bounds);}
                if i == 0 && (static_dirty || gpu.skyline_dirty) {
                    bind_texture(cx2.cx, dv, live_id!(gi_skyline), &gpu.skyline);
                    dv.set_uniform(cx2.cx, live_id!(gi_skyline_rect), &gpu.skyline_rect);
                    dv.set_uniform(cx2.cx, live_id!(gi_skyline_size), &gpu.skyline_size.map(|n| n as f32));
                    gpu.skyline_dirty = false;
                }
                if (i==1||i==2)&&static_dirty {
                    // Producers read last frame's field: feedback, history.
                    dv.set_uniform(cx2.cx,live_id!(gi_on),&[1.0]);
                    dv.set_uniform(cx2.cx,live_id!(gi_fallback_floor),&[0.0]);
                    dv.set_uniform(cx2.cx,live_id!(gi_back_cut),&[0.0]);
                    // The probes' own bounce reads the field, never a screen.
                    dv.set_uniform(cx2.cx,live_id!(gi_screen_ctl),&[0.0,1.0,1.0,0.0]);
                    bind_texture(cx2.cx,dv,live_id!(gi_screen),&field);
                    dv.set_uniform(cx2.cx,live_id!(gi_debug),&[0.0]);
                    bind_texture(cx2.cx,dv,live_id!(gi_field),&field);
                    bind_texture(cx2.cx,dv,live_id!(gi_hits),&hits);
                }
                if i==1 {
                    clustered.bind_scene(cx2.cx, dv, true);
                    if light_dirty {for (name,v) in [(live_id!(gi_sun_dir),sun.dir),(live_id!(gi_sun_color),sun.color),(live_id!(gi_sky),sun.sky),(live_id!(gi_ground),sun.ground)]{dv.set_uniform(cx2.cx,name,&[v.x,v.y,v.z]);}}
                    if light_dirty {dv.set_uniform(cx2.cx,live_id!(gi_relight),&[c.feedback,if linear{1.0}else{0.0},emission_gain,c.ray_distance]);
                    dv.set_uniform(cx2.cx,live_id!(gi_bounce),&[c.bounce]);}
                    dv.set_uniform(cx2.cx,live_id!(gi_light_ids0),&light_ids[..4]);
                    dv.set_uniform(cx2.cx,live_id!(gi_light_ids1),&light_ids[4..]);
                }
                if i==2&&static_dirty {bind_texture(cx2.cx,dv,live_id!(gi_radiance),&radiance);}
                if i==3&&static_dirty {
                    let a=c.atlas();
                    dv.set_uniform(cx2.cx,live_id!(gi_atlas),&a.map(|v|v as f32));
                    bind_texture(cx2.cx,dv,live_id!(gi_update),&update);
                }
            }
            let gpu=self.gpu.as_mut().unwrap();
            if rows == 0 && i < 3 {
                // An exposed slab can require info clears with an empty surface mask.
                gpu.stages[i].pass.withdraw(cx2.cx);
                continue;
            }
            let atlas=c.atlas();
            let (width,height,target)=match i{0=>(RAYS+1,height,&gpu.hits),1=>(RAYS,height,&gpu.radiance),2=>(GATHER_WIDTH,height,&gpu.update),_=>(atlas[0],atlas[1],&gpu.field)};
            let target=target.clone();
            let next=if i<3{Some(gpu.stages[i+1].pass.draw_pass_id())}else{None};
            let stage=&mut gpu.stages[i];
            cx2.make_child_pass(&stage.pass);
            if let Some(next)=next{stage.pass.set_pass_parent(cx2.cx,next);}
            cx2.begin_pass(&stage.pass,Some(1.0));
            stage.pass.set_size(cx2.cx,dvec2(width as f64,height as f64));stage.pass.clear_color_textures(cx2.cx);
            // Every live row is overwritten; unused rows are never sampled.
            // Do not clear a full-capacity target for a small batch.
            let clear=DrawPassClearColor::InitWith(vec4(0.0,0.0,0.0,0.0));
            stage.pass.set_color_texture(cx2.cx,&target,clear);
            stage.list.begin_always(&mut cx2);
            cx2.begin_root_turtle(dvec2(width as f64,height as f64),Layout::flow_overlay());
            match i {
                0..=2 => {
                    let quad = match i { 0 => &mut gpu.trace.quad, 1 => &mut gpu.relight.quad, _ => &mut gpu.gather.quad };
                    quad.begin_many_instances(&mut cx2);
                    for rect in band_quads(rows, band, width) { quad.draw_abs(&mut cx2, rect); }
                    quad.end_many_instances(&mut cx2);
                }
                _=>{
                    // Clears first: a slot re-targeted AND traced this
                    // frame ends up with its new tiles.
                    let tile=|k:usize,slot:usize,region:usize|{
                        let (x,y)=((slot%atlas[2])as f64,(slot/atlas[2])as f64);let base=(k*atlas[3]*19)as f64;let t=atlas[3]as f64;
                        match region{0=>dvec2(x*8.0,base+y*8.0),1=>dvec2(x*10.0,base+t*8.0+y*10.0),_=>dvec2(x,base+t*18.0+y)}
                    };
                    // Keep a stable draw slot for clears, even on frames with
                    // none. Retain its storage across scene publications.
                    if let Some(mut many)=cx2.begin_many_instances(&mut gpu.scatter.quad.draw_vars) {
                        // Scroll emits sorted slots: clear contiguous info
                        // texels with one rectangle, including whole rows on
                        // first publication/teleport. No thousands of quads
                        // competing with the steady warm-up batch.
                        let mut clears=gpu.clears.drain(..).peekable();
                        while let Some((k,slot))=clears.next() {
                            let mut width=1;
                            while clears.peek().is_some_and(|&(next_k,next)|next_k==k&&next==slot+width&&next/atlas[2]==slot/atlas[2]) {
                                clears.next();width+=1;
                            }
                            gpu.scatter.gi_tile=vec4(-1.0,0.0,1.0,0.0);
                            gpu.scatter.quad.rect_pos=tile(k,slot,2).into();
                            gpu.scatter.quad.rect_size=vec2(width as f32,1.0);
                            many.instances.extend_from_slice(gpu.scatter.quad.draw_vars.as_slice());
                        }
                        cx2.end_many_instances(many);
                    }
                    if rows > 0 {
                        let (instances,count)=gpu.scatter_cache.batch(rows);
                        cx2.add_retained_instances_count(&mut gpu.scatter.quad.draw_vars,instances,count);
                    }
                },
            }
            cx2.end_pass_sized_turtle();stage.list.end(&mut cx2);cx2.end_pass(&stage.pass);
        }
    }

    /// Make the screen AO's draws before its first run (see `CortaoPass::prepare_draws`).
    pub(crate) fn prepare_screen_ao_draws(&mut self,cx:&mut Cx)->bool { self.ao.prepare_draws(cx) }
    pub(crate) fn screen_ao_draws_ready(&self)->bool { self.ao.draws_ready() }
    pub(crate) fn screen_ao_pending(&self)->bool { self.ao.pipelines_pending() }

    /// Run CORTAO over `depth` (the scene pass's hardware depth, holding the
    /// PREVIOUS frame) as child passes of `scene_pass`; returns visibility in
    /// red at the frame's size (the chain itself may run at half size). The scene pass's camera
    /// uniforms still hold the matrices that depth was drawn with.
    pub fn run_screen_ao(&mut self,cx:&mut Cx2d,scene_pass:DrawPassId,depth:&Texture,size:DVec2,params:crate::renderer::ScreenAoParams)->Option<Texture> {
        let uniforms=&cx.passes[scene_pass].pass_uniforms;
        let p=uniforms.camera_projection;
        // Nothing is drawn before the first frame (no projection yet).
        if p.v[0]==0.0||p.v[5]==0.0||size.x<2.0||size.y<2.0 {return None;}
        let clip_from_world=Mat4f::mul(&p,&uniforms.camera_view);
        let camera=CortaoCamera::from_clip(&clip_from_world.v)?;
        let rays=params.tier.rays();
        let table=self.ao_tables[usize::from(rays==32)].get_or_insert_with(||CortaoTable::new(cx.cx,rays));
        let output=self.ao.run(cx,&CortaoInput{
            size,depth,depth_frame:CortaoDepth::Previous,camera,tier:params.tier,
            config:CortaoConfig{radius:params.radius,resolution:params.resolution,..Default::default()},
            frame:params.frame,history:params.history,reset:params.reset,table,
        },scene_pass);
        self.stats.ao_gpu_ms=self.ao.gpu_ms();
        output.map(|CortaoOutput::Visibility(texture)|texture)
    }

    /// Gather the probe field over the previous scene depth in its camera-relative frame.
    pub fn run_gi_screen(&mut self, cx: &mut Cx2d, scene_pass: DrawPassId, depth: &Texture, size: DVec2, origin: Vec3f) -> bool {
        let (width, height) = Cx::pass_pixel_size(size, cx.current_dpi_factor());
        let full_size = dvec2(width as f64, height as f64);
        let same_size = self.screen_fill.as_ref().is_some_and(|fill| fill.full_size == full_size);
        let uniforms = &cx.passes[scene_pass].pass_uniforms;
        let projection = uniforms.camera_projection;
        // A sampled depth is clip z/w: one 0..w clip-z convention on every backend (platform/script/tests/clip_z.rs), GL included.
        let depth_map = [1.0f32, 0.0];
        let ready = self.receivers == GiReceivers::Fast && self.shader_features().0 && cx.gpu_info().float_color_targets
            && projection.v[0] != 0.0 && projection.v[5] != 0.0
            && size.x >= 2.0 && size.y >= 2.0 && width >= 2 && height >= 2
            && (!same_size || depth.allocated_size(cx) == Some((width, height)));
        let prepared = if ready {
            let camera = VolumeFogCamera::from_clip(Mat4f::mul(&projection, &uniforms.camera_view));
            GiScreenView::from_camera(&camera, depth_map).map(|view| (camera, view))
        } else { None };
        let Some((camera, view)) = prepared else {
            self.skip_gi_screen(cx);
            return false;
        };
        if !same_size {
            let (pass, list, draw) = if let Some(fill) = self.screen_fill.take() {
                fill.pass.withdraw(cx);
                fill.target.release(cx);
                (fill.pass, fill.list, fill.draw)
            } else {
                let Some(draw) = cx.try_with_vm(DrawGiScreenFill::script_new_with_default) else { return false; };
                (crate::renderer::new_scene_pass(cx, "GI screen fill", self.scene_time), DrawList::new(cx), draw)
            };
            let width = width.div_ceil(GI_SCREEN_DIVISOR);
            let height = height.div_ceil(GI_SCREEN_DIVISOR);
            let texel_size = dvec2(width as f64, height as f64);
            let target = Texture::new_with_format(cx, TextureFormat::RenderRGBAf32 {
                size: TextureSize::Fixed { width, height }, initial: true,
            });
            pass.set_size(cx, texel_size);
            pass.set_color_texture(cx, &target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, -1.0)));
            self.screen_fill = Some(Box::new(GiScreenFill { pass, list, draw, target, texel_size, full_size, view: None }));
            // The scene will draw the first depth at this size later in the frame.
            return false;
        }

        let mut fill = self.screen_fill.take().unwrap();
        let dv = &mut fill.draw.quad.draw_vars;
        self.bind(cx, dv);
        bind_texture(cx, dv, live_id!(depth_tex), depth);
        dv.set_uniform(cx, live_id!(u_world_from_clip), &camera.general.world_from_clip);
        dv.set_uniform(cx, live_id!(u_eye), &camera.eye);
        dv.set_uniform(cx, live_id!(u_depth_map), &depth_map);
        // The normal's depth neighbours sit one cache texel apart: one full-size pixel apart, the
        // depth's steps printed horizontal bands into the cache on ground seen at a low angle.
        dv.set_uniform(cx, live_id!(gi_fill_ctl), &[fill.texel_size.x as f32, fill.texel_size.y as f32]);

        cx.make_child_pass(&fill.pass);
        fill.pass.set_pass_parent(cx, scene_pass);
        cx.begin_pass(&fill.pass, Some(1.0));
        fill.list.begin_always(cx);
        cx.begin_root_turtle(fill.texel_size, Layout::flow_overlay());
        fill.draw.quad.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: fill.texel_size });
        cx.end_pass_sized_turtle();
        fill.list.end(cx);
        cx.end_pass(&fill.pass);
        fill.view = Some((origin, view));
        self.screen_fill = Some(fill);
        true
    }

    pub(crate) fn skip_gi_screen(&mut self, cx: &mut Cx) {
        if let Some(fill) = &mut self.screen_fill {
            fill.view = None;
            fill.pass.withdraw(cx);
        }
    }

    pub(crate) fn take_screen_view(&mut self) -> Option<(Vec3f, GiScreenView)> {
        self.screen_fill.as_mut()?.view.take()
    }

    pub(crate) fn restore_screen_view(&mut self, view: Option<(Vec3f, GiScreenView)>) {
        if let Some(fill) = &mut self.screen_fill { fill.view = view; }
    }

    /// State a receiver sees after one probe cascade and one screen texel have been filled.
    #[cfg(test)]
    pub(crate) fn install_screen_fill_for_test(&mut self, cx: &mut Cx, origin: Vec3f) {
        self.set_mode(GiMode::Fast);
        self.set_config(GiConfig { grid: [4, 2, 4], cascades: 1, ..GiConfig::default() });
        let mut cascade = Cascade::new(self.config, 0);
        cascade.scroll([0; 3]);
        self.cascades = vec![cascade];

        // Only the receiver records draw items. The producer draws and scratch textures
        // need handles, but none of their passes need to run or compile a GI shader.
        let (trace, relight, gather, scatter, draw) = cx.with_vm(|vm| (
            DrawGiTrace::script_new(vm),
            DrawGiRelight::script_new(vm),
            DrawGiGather::script_new(vm),
            DrawGiScatter::script_new(vm),
            DrawGiScreenFill::script_new(vm),
        ));
        let [width, height, _, _] = self.config.atlas();
        self.gpu = Some(Gpu {
            stages: Vec::new(), trace, relight, gather, scatter,
            field: Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 {
                size: TextureSize::Fixed { width, height }, initial: true,
            }),
            hits: Texture::new(cx), radiance: Texture::new(cx), update: Texture::new(cx),
            batch: Texture::new(cx), clears: Vec::new(), vox: std::array::from_fn(|_| Texture::new(cx)),
            layout: VoxelLayout::new(self.config), capacity: 0, target_height: 0, logged_bands: 0, skyline: Texture::new(cx),
            skyline_rect: [0.0; 4], skyline_size: [1, 1], skyline_dirty: false,
            scatter_cache: ScatterCache::default(), encode_key: None, light_key: None,
        });

        let view = GiScreenView::from_clip(Mat4f::perspective(60.0, 1.0, 0.1, 10.0), [1.0, 0.0])
            .expect("the perspective fill must have screen rays");
        let target = Texture::new_with_format(cx, TextureFormat::RenderRGBAf32 {
            size: TextureSize::Fixed { width: 1, height: 1 }, initial: true,
        });
        self.screen_fill = Some(Box::new(GiScreenFill {
            pass: DrawPass::new(cx), list: DrawList::new(cx), draw, target,
            texel_size: dvec2(1.0, 1.0), full_size: DVec2::all(GI_SCREEN_DIVISOR as f64),
            view: Some((origin, view)),
        }));
    }

    /// Binds the fill's cache, switched off, on a lane whose shader reads it, and returns the
    /// switch that turns it on for one draw item (`gi_screen_ctl`). `None` while the receivers
    /// gather: Exact, no fill this frame, a fill from another origin, or a shader without a reader.
    pub(crate) fn bind_screen_cache(&self, cx: &Cx, dv: &mut DrawVars, origin: Vec3f) -> Option<[f32; 4]> {
        let fill = self.screen_fill.as_ref()?;
        let (fill_origin, view) = fill.view.as_ref()?;
        if self.receivers != GiReceivers::Fast || *fill_origin != origin || !self.shader_features().0 {
            return None;
        }
        let size = [fill.texel_size.x as f32, fill.texel_size.y as f32];
        Self::bind_screen(cx, dv, Some(&fill.target), [0.0, size[0], size[1], 0.0], Some(view))
            .then_some([1.0, size[0], size[1], 0.0])
    }

    /// Whether the scene shaders' GI sampling and its debug views do
    /// anything this frame (what `bind` writes: `gi_on` above 0, a debug
    /// mode on): a lane draws through a shader variant without them when
    /// not (renderer/variants.rs).
    pub fn shader_features(&self)->(bool,bool){
        let on=self.mode==GiMode::Fast&&self.gpu.is_some()&&!self.stats.rejected_scene&&self.cascades.iter().any(|c|c.origin.is_some());
        let sampling=on&&self.config.strength>0.0;
        (sampling,sampling&&(self.debug as u8)!=0)
    }

    pub(crate) fn set_screen(&mut self, screen: Option<GiScreenCache>) { self.screen = screen; }

    /// Whether the shader reads a screen cache (and took this one).
    fn bind_screen(cx: &Cx, dv: &mut DrawVars, texture: Option<&Texture>, ctl: [f32; 4], view: Option<&GiScreenView>) -> bool {
        let Some(id) = dv.draw_shader_id else { return false };
        let Some(slot) = cx.draw_shaders[id.index].mapping.textures.iter().position(|t| t.id == live_id!(gi_screen)) else { return false };
        if let Some(texture) = texture { dv.set_texture(slot, texture); } else { dv.empty_texture(slot); }
        dv.set_uniform(cx, live_id!(gi_screen_ctl), &ctl);
        dv.set_uniform(cx, live_id!(gi_screen_clip), view.map_or(&[0.0; 16], |v| &v.clip.v));
        dv.set_uniform(cx, live_id!(gi_screen_eye), view.map_or(&[0.0; 4], |v| &v.eye));
        dv.set_uniform(cx, live_id!(gi_screen_ray0), view.map_or(&[0.0; 4], |v| &v.rays[0]));
        dv.set_uniform(cx, live_id!(gi_screen_ray_u), view.map_or(&[0.0; 4], |v| &v.rays[1]));
        dv.set_uniform(cx, live_id!(gi_screen_ray_v), view.map_or(&[0.0; 4], |v| &v.rays[2]));
        true
    }

    pub fn bind(&self,cx:&Cx,dv:&mut DrawVars){
        let on=self.mode==GiMode::Fast&&self.gpu.is_some()&&!self.stats.rejected_scene&&self.cascades.iter().any(|c|c.origin.is_some());
        dv.set_uniform(cx,live_id!(gi_on),&[if on{self.config.strength}else{0.0}]);
        dv.set_uniform(cx,live_id!(gi_debug),&[self.debug as u8 as f32]);
        dv.set_uniform(cx,live_id!(gi_fallback_floor),&[RECEIVER_FALLBACK_FLOOR]);
        dv.set_uniform(cx,live_id!(gi_back_cut),&[if self.config.thin_walls{1.0}else{0.0}]);
        if !on {
            if let Some(id)=dv.draw_shader_id {if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==live_id!(gi_field)){dv.empty_texture(slot);}}
            Self::bind_screen(cx, dv, None, [0.0, 1.0, 1.0, 0.0], None);
            return;
        }
        Self::cascade_uniforms(cx,dv,self.config,&self.cascades);
        bind_texture(cx,dv,live_id!(gi_field),&self.gpu.as_ref().unwrap().field);
        // Off until a lane or a host asks for a cache (`bind_screen_cache`, `bind_host_screen`).
        Self::bind_screen(cx, dv, Some(&self.gpu.as_ref().unwrap().field), [0.0, 1.0, 1.0, 0.0], None);
    }

    /// A host's own cache (`set_screen`), for the host's own receivers (`Renderer::bind_gi`);
    /// the renderer's lanes never read it.
    pub(crate) fn bind_host_screen(&self, cx: &Cx, dv: &mut DrawVars) {
        if let Some(screen) = self.screen.as_ref().filter(|_| self.shader_features().0) {
            Self::bind_screen(cx, dv, Some(&screen.texture), [1.0, screen.size[0], screen.size[1], 0.0], Some(&screen.view));
        }
    }
}
pub(crate) fn bind_texture(cx:&Cx,dv:&mut DrawVars,name:LiveId,t:&Texture){if let Some(id)=dv.draw_shader_id{if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==name){dv.set_texture(slot,t);}}}

#[cfg(test)]
#[path="fast_gi/tests.rs"] mod tests;

#[cfg(test)]
#[path="fast_gi/shader_inputs.rs"] mod shader_inputs;

#[cfg(test)]
mod gpu_guard;
