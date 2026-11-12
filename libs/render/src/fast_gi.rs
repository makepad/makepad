//! Cascaded probe GI for procedurally built, hot-reloaded worlds.
//!
//! - Scene: a voxel clipmap of the static world (fast_gi/voxels.rs), kept
//!   current on a pool worker by diffing instance snapshots; an edit
//!   re-voxelizes only the bricks it touches, a camera move only the bricks
//!   a window newly covers. No triangle/instance caps.
//! - Probes: up to 3 camera-centred cascades (default 16x8x16 at 1/2/4 m),
//!   addressed toroidally (fast_gi/cascades.rs): scrolling traces only the
//!   newly exposed slab, edits re-trace only nearby probes first, the rest
//!   keep their light and are refreshed round-robin with a rotated ray set
//!   and hysteresis.
//! - GPU: four fragment passes per frame over a small probe batch whose size
//!   follows the passes' measured GPU time (fast_gi/shaders.rs).
//! - Receivers sample the finest confident cascade, then coarser ones, then
//!   their ordinary ambient. A probe holds the light the scene's surfaces
//!   send and the share of its rays that left the scene; a receiver's
//!   ambient is its ordinary ambient (the hemisphere, an environment map's
//!   fill) times that open share, plus the surfaces' light. Open space looks
//!   as with GI off, a cavity darkens, a coloured neighbour adds what it
//!   reflects.
//!
//! A host with a still scene sets `GiConfig::settle`: only probes a surface
//! can read are traced, each a fixed number of times, then the passes stop
//! (no GPU work, nothing to redraw) until the scene or the light changes.
//!
//! Needs float colour targets (`GpuInfo::float_color_targets`, reported per
//! device by every backend); a GPU without them stays off and allocates
//! nothing.
use makepad_draw::*;
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
pub(crate) use shaders::{DrawGiTrace, DrawGiRelight, DrawGiGather, DrawGiScatter};
pub use voxels::{Instance as GiInstance, Mesh as GiMesh, GiField};
pub(crate) use voxels::{Instance, Mesh, Snapshot};
use cascades::{BatchItem, Cascade};
use voxels::{VoxelLayout, VoxelUpdate, VoxelWorld};

pub const RAYS: usize = 64;
/// Gathered texels per probe: 8x8 irradiance tile, 10x10 moments, 1 info.
pub const GATHER_WIDTH: usize = 64+100+1;
/// Probe tiles per atlas row.
const ATLAS_TILES: usize = 64;
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

/// Display-only diagnostics: never feed false colours back into transport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum GiDebug { #[default] Off, Cells, Confidence, Weights, Irradiance, ProbeState, RawProbe }
impl GiDebug {
    pub fn next(self)->Self {match self {Self::Off=>Self::Cells,Self::Cells=>Self::Confidence,Self::Confidence=>Self::Weights,Self::Weights=>Self::Irradiance,Self::Irradiance=>Self::ProbeState,Self::ProbeState=>Self::RawProbe,Self::RawProbe=>Self::Off}}
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
    /// A still scene: trace only the probes a surface can read, a fixed
    /// number of updates each, then stop until the scene or the light
    /// changes (`FastGi::wants_frames` goes false). Off: every probe is refreshed forever.
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
pub(crate) type SceneKey = [u64; 5];

type SceneBounds = (Vec3f, Vec3f);
type VoxelJob = makepad_platform::thread::TaskHandle<(u64, Box<VoxelWorld>, VoxelUpdate, SceneBounds)>;

/// The GPU passes of a frame, by the names the frame timer reports them under.
const STAGE_NAMES:[&str;4]=["GI trace","GI relight","GI gather","GI scatter"];
struct Stage {pass:DrawPass,list:DrawList,times:StageTimes}
struct Gpu {
    stages:Vec<Stage>, trace:DrawGiTrace, relight:DrawGiRelight, gather:DrawGiGather, scatter:DrawGiScatter,
    field:Texture, hits:Texture, radiance:Texture, update:Texture, batch:Texture,
    /// Info texels of slots that changed cells, zeroed in the next scatter.
    clears:Vec<(usize,usize)>,
    vox:[Texture;3], layout:VoxelLayout, capacity:usize,
    scatter_cache:ScatterCache,
    encode_key:Option<EncodeKey>, light_key:Option<[f32;22]>,
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

/// Screen-space AO for hosts without a depth prepass: ssao.rs over the
/// previous frame's hardware depth, bound to the model lanes' `ssao_map`.
#[derive(Default)]
struct ScreenAo { pass:crate::ssao::SsaoPass }

/// Mesh copies shared across snapshots, keyed by render geometry.
#[derive(Default)]
pub(crate) struct MeshCache {
    pub geometry: std::collections::HashMap<(GeometryId,u64),Vec<(Option<TextureId>,Arc<Mesh>,u64)>>,
    pub shapes: std::collections::HashMap<usize,Arc<Mesh>>,
    pub generation: u64,
    /// Bumped per realm: revision counters restart with a new world.
    pub epoch: u64,
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
    mode:GiMode, config:GiConfig, debug:GiDebug, linear_albedo:bool, emission_gain:f32,
    gpu:Option<Gpu>,
    probe_counts:diagnostics::ProbeCounts,
    /// Textures of a released GPU state, their storage retired at the next frame
    /// (`flush_retired`): off holds no GPU memory.
    retired:Vec<Texture>,
    /// A host's screen cache of the gathered field (`set_screen`): its
    /// texture and size in pixels.
    screen:Option<(Texture,[f32;2])>,
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
    /// Settle mode: the light the settled field holds, and whether all the
    /// work is done (nothing queued, nothing building, the scene current).
    light_key:Option<[u32;12]>, settled:bool,
    /// The host holds a scene newer than the last one submitted.
    scene_stale:bool,
    pub(crate) meshes:MeshCache,
    ao:ScreenAo,
    pub stats:GiStats,
}
impl Default for FastGi {
    fn default()->Self{Self{mode:GiMode::Off,config:GiConfig::default(),debug:GiDebug::Off,linear_albedo:false,emission_gain:1.0,gpu:None,probe_counts:diagnostics::ProbeCounts::default(),retired:Vec::new(),screen:None,voxels:None,voxel_job:None,generation:0,
        scene_key:None,scene_center:None,pending_scene:None,scene_bounds:Default::default(),has_scene:false,requested:Vec::new(),committed:Vec::new(),cascades:Vec::new(),
        controller:BatchController::default(),frame_times:FrameTimes::default(),batch:Vec::new(),frame:0,light_key:None,settled:false,scene_stale:false,meshes:MeshCache::default(),ao:ScreenAo::default(),stats:GiStats::default()}}
}
impl FastGi {
    pub fn mode(&self)->GiMode{self.mode}
    pub fn debug(&self)->GiDebug{self.debug}
    pub fn set_debug(&mut self,debug:GiDebug){self.debug=debug;}
    pub fn config(&self)->GiConfig{self.config}
    /// Texel/vertex albedo is sRGB-encoded; decode it in the relight when
    /// the host lights in linear space. The field is re-lit, not re-traced.
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
        self.config=c;
    }
    /// A world replacement (script re-eval, hot reload, level change). The
    /// field, voxels and cascades are KEPT: the next snapshot is diffed
    /// against the last one, so a reload that changes little re-traces
    /// little, and a new level relights in place instead of flashing flat.
    pub fn reset(&mut self){
        self.scene_key=None;self.scene_center=None;self.pending_scene=None;
        self.meshes.epoch=self.meshes.epoch.wrapping_add(1);
    }
    /// Drop everything (mode or layout change, a replaced GPU device).
    pub(crate) fn release(&mut self){
        if let Some(job)=self.voxel_job.take(){job.cancel();}
        self.generation=self.generation.wrapping_add(1);
        self.probe_counts.release(&mut self.retired);
        if let Some(gpu)=self.gpu.take() {
            self.retired.extend([gpu.field]);
            self.retired.extend(gpu.vox);
            if gpu.capacity>0 {self.retired.extend([gpu.hits,gpu.radiance,gpu.update,gpu.batch]);}
        }
        self.voxels=None;self.scene_key=None;self.scene_center=None;self.pending_scene=None;self.has_scene=false;
        self.scene_bounds=Default::default();
        self.requested.clear();self.committed.clear();self.cascades.clear();self.controller=BatchController::default();self.frame_times=FrameTimes::default();self.batch.clear();self.light_key=None;self.settled=false;self.scene_stale=false;
        self.meshes=MeshCache::default();self.stats=GiStats::default();
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
        let e=self.coarsest_extent()*0.4;
        let moved=self.scene_center.is_none_or(|c|{let d=center-c;d.x.abs()>e.x||d.y.abs()>e.y||d.z.abs()>e.z});
        self.scene_key!=Some(key)||moved
    }
    pub fn submit_scene(&mut self,key:SceneKey,center:Vec3f,snapshot:Snapshot){
        self.stats.probe_counts_swept=false;
        self.stats.triangles=snapshot.triangles;self.stats.instances=snapshot.instances.len();
        self.scene_key=Some(key);self.scene_center=Some(center);
        let layout=VoxelLayout::new(self.config);
        let bounds=snapshot.padded_bounds(layout.voxel_size(layout.levels-1));
        self.pending_scene=Some((Arc::new(snapshot),bounds));
    }
    /// More frames would change the picture: probes are queued, a voxel job
    /// runs or a newer scene waits.
    pub fn wants_frames(&self)->bool{self.mode==GiMode::Fast&&self.config.settle&&!self.settled&&!self.stats.rejected_scene}
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
        let stages=STAGE_NAMES.into_iter().map(|name|Stage{pass:DrawPass::new_with_name(cx,name),list:DrawList::new(cx),times:StageTimes::default()}).collect();
        let atlas=c.atlas();
        let field=Texture::new_with_format(cx,TextureFormat::RenderRGBAf16{size:TextureSize::Fixed{width:atlas[0],height:atlas[1]},initial:true});
        let vox=[0,1,2].map(|_|Texture::new_with_format(cx,TextureFormat::VecBGRAu8_32{width:layout.width(),height:layout.height(),data:Some(vec![0;layout.width()*layout.height()]),updated:TextureUpdated::Full}));
        self.gpu=Some(Gpu{stages,trace,relight,gather,scatter,field,hits:Texture::new(cx),radiance:Texture::new(cx),update:Texture::new(cx),batch:Texture::new(cx),clears:Vec::with_capacity(c.probe_count()*c.cascades),vox,layout,capacity:0,scatter_cache:ScatterCache::default(),encode_key:None,light_key:None});
        self.cascades=(0..c.cascades).map(|k|Cascade::new(c.grid,c.cascade_spacing(k),c.settle)).collect();
        self.committed=vec![None;c.cascades];
        let capacity=cascades::batch_capacity(c.probe_count(),c.cascades);
        self.batch=Vec::with_capacity(capacity);
        self.controller=BatchController::new(c.probes_per_frame.min(capacity),c.gpu_budget_ms);
        self.ensure_capacity(cx,capacity);
        true
    }

    /// Adopt a finished voxel job: copy its changed levels into the voxel
    /// textures, move the cascades onto the new windows, queue edited probes.
    fn poll_voxels(&mut self,cx:&mut Cx) {
        let Some(result)=self.voxel_job.as_mut().and_then(|j|j.try_take()) else {return};
        self.voxel_job=None;self.stats.building=false;
        let (generation,world,update,bounds)=match result {
            Ok(r)=>r,
            Err(e)=>{log!("fast GI voxel worker failed: {e:?}");self.scene_key=None;return;}
        };
        if generation!=self.generation{return;}
        let gpu=self.gpu.as_mut().unwrap();
        let layout=world.layout;
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
        let first=self.committed.iter().all(|w|w.is_none());
        self.stats.scroll_probes=0;self.stats.dirty_probes=0;
        for k in 0..self.cascades.len() {
            self.committed[k]=world.windows[k];
            if let Some(w)=world.windows[k] {
                let exposed=self.cascades[k].scroll(w.map(|v|v/4));
                self.stats.scroll_probes+=exposed.len();
                gpu.clears.extend(exposed.into_iter().map(|slot|(k,slot)));
            }
            if let Some(needed)=update.needed.get(k).and_then(|n|n.as_ref()) {self.cascades[k].set_needed(needed);}
            if !first {for (lo,hi) in &update.dirty {self.stats.dirty_probes+=self.cascades[k].invalidate([lo.x,lo.y,lo.z],[hi.x,hi.y,hi.z],self.config.home);}}
            // An edit changes what every probe that sees it gathers, not
            // only the near ones: they settle again, after those.
            if !update.dirty.is_empty() {self.cascades[k].restart();}
        }
        self.stats.voxel_jobs+=1;self.stats.voxel_bricks=update.bricks;self.stats.voxel_ms=update.ms;
        if gi_stats_enabled() {
            log!("fast GI voxels: {} bricks / {} triangles in {:.1}ms (worker), {} edit boxes -> {} probes, scroll {} probes",update.bricks,update.triangles,update.ms,update.dirty.len(),self.stats.dirty_probes,self.stats.scroll_probes);
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
            if (0..3).any(|a|(p[a]-(w[a] as f32+layout.dims[a] as f32*0.5)).abs()>voxels::BRICK as f32*1.5){self.requested[k]=layout.window_for(k,center);}
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
            Ok(job) => self.voxel_job=Some(job),
            Err(error) => { self.stats.building=false; log!("fast GI: {error}"); }
        }
    }

    fn ensure_capacity(&mut self,cx:&mut Cx,rows:usize) {
        let gpu=self.gpu.as_mut().unwrap();
        if rows<=gpu.capacity{return;}
        let cap=rows.next_power_of_two().max(16);
        let target=|cx:&mut Cx,w|Texture::new_with_format(cx,TextureFormat::RenderRGBAf32{size:TextureSize::Fixed{width:w,height:cap},initial:true});
        gpu.hits=target(cx,RAYS+1);gpu.radiance=target(cx,RAYS);
        gpu.update=Texture::new_with_format(cx,TextureFormat::RenderRGBAf16{size:TextureSize::Fixed{width:GATHER_WIDTH,height:cap},initial:true});
        gpu.batch=Texture::new_with_format(cx,TextureFormat::VecRGBAf32{width:2,height:cap,data:Some(vec![0.0;cap*8]),updated:TextureUpdated::Full});
        gpu.capacity=cap;
        gpu.encode_key=None;
        self.stats.static_encode_bytes+=gpu.scatter_cache.prepare(&mut gpu.scatter,cap);
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

    pub fn run(&mut self,cx:&mut CxDraw,center:Vec3f,sun:&crate::sun::SunLight,clustered:&crate::clustered::ClusteredLights) {
        if self.mode==GiMode::Off{return;}
        let center=self.config.anchor.unwrap_or(center);
        let previous_batch=self.stats.batch;
        let start=Cx::monotonic_now();self.stats.traced_rays=0;self.stats.uploads_bytes=0;self.stats.batch=0;self.stats.static_encode_bytes=0;
        if !finite(center)||!self.ensure(cx.cx){return;}
        self.frame=self.frame.wrapping_add(1);
        self.poll_voxels(cx.cx);
        self.launch_voxels(cx,center);
        let c=self.config;let count=c.probe_count();
        if c.settle {
            // Another light: what the probes gathered is out of date.
            let v=[sun.dir,sun.color,sun.sky,sun.ground];
            let key=std::array::from_fn(|i|{let p=v[i/3];[p.x,p.y,p.z][i%3].to_bits()});
            if self.light_key.is_some_and(|k|k!=key) {self.cascades.iter_mut().for_each(|c|c.restart());}
            self.light_key=Some(key);
        }
        // HAL currently exposes only name-based samples (no submission tag
        // or reusable drain). Consume every arrival, including delayed ones;
        // only equally sized streams can supply complete frame sums.
        // Its Vec transfer still allocates in platform's watcher; replacing
        // that API belongs in platform, not this render-library change.
        // HAL publishes these streams together on the UI thread; take them
        // before processing any further completions. GI issues four render
        // passes: voxel changes are texture uploads, not a fifth GI pass.
        let samples=STAGE_NAMES.map(makepad_platform::gpu_frame_timer::take_watched);
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
        self.stats.gpu_aim_ms=BatchController::aim(c.gpu_budget_ms);
        // With a slowly changing batch, the previous submitted size is an
        // approximation for async samples. Missing samples freeze the rate.
        let active=self.cascades.iter().any(|c|c.origin.is_some())&&!self.settled;
        self.controller.advance(start,if sample&&previous_batch>0{cost}else{None},active);
        self.stats.probes=count*self.cascades.len();
        self.stats.budget=self.controller.cap(c.gpu_budget_ms,cascades::batch_capacity(count,c.cascades));
        cascades::schedule(&mut self.cascades,self.stats.budget,&mut self.batch);
        self.stats.pending_probes=self.cascades.iter().map(|c|c.pending()).sum();
        self.settled=self.batch.is_empty()&&self.voxel_job.is_none()&&self.pending_scene.is_none()&&!self.scene_stale&&self.has_scene&&self.cascades.iter().all(|c|c.origin.is_some());
        if self.batch.is_empty(){self.stats.resident_bytes=self.resident_bytes();self.stats.encode_us=((Cx::monotonic_now()-start)*1e6)as u64;return;}
        self.encode(cx,center,sun,clustered);
        self.stats.batch=self.batch.len();self.stats.traced_rays=self.batch.len()*RAYS;
        self.stats.resident_bytes=self.resident_bytes();
        self.stats.encode_us=((Cx::monotonic_now()-start)*1e6)as u64;
    }

    fn resident_bytes(&self)->usize {
        let Some(gpu)=&self.gpu else{return 0};
        let l=gpu.layout;
        let a=self.config.atlas();
        a[0]*a[1]*8+(RAYS*2+1+2)*gpu.capacity*16+GATHER_WIDTH*gpu.capacity*8
            +l.width()*l.height()*4*3*2+self.meshes.bytes()+gpu.scatter_cache.instances.as_ref().map_or(0,|r|r.byte_len()*2)
    }

    fn encode(&mut self,cx:&mut CxDraw,center:Vec3f,sun:&crate::sun::SunLight,clustered:&crate::clustered::ClusteredLights) {
        let c=self.config;
        let batch=&self.batch;
        let rows=batch.len();
        {
            let gpu=self.gpu.as_mut().unwrap();
            let mut data=gpu.batch.take_vec_f32(cx.cx);
            for (i,b) in batch.iter().enumerate() {
                data[i*8..i*8+8].copy_from_slice(&work::batch_row(b));
            }
            gpu.batch.put_back_vec_f32(cx.cx,data,Some(RectUsize::new(PointUsize::new(0,0),SizeUsize::new(2,rows))));
        }
        self.stats.uploads_bytes+=rows*32;
        let light_ids=clustered.gi_light_ids(center);self.stats.selected_lights=light_ids.iter().filter(|&&i|i>=0.0).count();
        let cap=self.gpu.as_ref().unwrap().capacity;
        let pass_uniform=[rows as f32,cap as f32,c.ray_distance,(self.frame%4096)as f32];
        let linear=self.linear_albedo;let emission_gain=self.emission_gain;
        let key=EncodeKey{config:c,origins:std::array::from_fn(|k|self.cascades.get(k).and_then(|c|c.origin)),committed:std::array::from_fn(|k|self.committed.get(k).copied().flatten()),bounds:self.scene_bounds,reload_generation:cx.reload_generation};
        let light_key=[sun.dir.x,sun.dir.y,sun.dir.z,sun.color.x,sun.color.y,sun.color.z,sun.sky.x,sun.sky.y,sun.sky.z,sun.ground.x,sun.ground.y,sun.ground.z,emission_gain,light_ids[0],light_ids[1],light_ids[2],light_ids[3],light_ids[4],light_ids[5],light_ids[6],light_ids[7],if linear{1.0}else{0.0}];
        let gpu=self.gpu.as_mut().unwrap();
        let reloaded=gpu.encode_key.as_ref().is_some_and(|k|k.reload_generation!=key.reload_generation);
        for dv in [&mut gpu.trace.quad.draw_vars,&mut gpu.relight.quad.draw_vars,&mut gpu.gather.quad.draw_vars,&mut gpu.scatter.quad.draw_vars] {
            dv.refresh_after_reload(cx.cx);
        }
        if reloaded {
            gpu.scatter_cache=ScatterCache::default();
            self.stats.static_encode_bytes+=gpu.scatter_cache.prepare(&mut gpu.scatter,cap);
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
                if i<3 {
                    if static_dirty {Self::cascade_uniforms(cx2.cx,dv,c,cascades);}
                    dv.set_uniform(cx2.cx,live_id!(gi_pass),&pass_uniform);
                    if static_dirty {bind_texture(cx2.cx,dv,live_id!(gi_batch),&batch_tex);}
                }
                if i<2&&static_dirty {Self::voxel_uniforms(cx2.cx,dv,layout,&vox,committed,*scene_bounds);}
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
                    clustered.bind(cx2.cx,dv,true);
                    if light_dirty {for (name,v) in [(live_id!(gi_sun_dir),sun.dir),(live_id!(gi_sun_color),sun.color),(live_id!(gi_sky),sun.sky),(live_id!(gi_ground),sun.ground)]{dv.set_uniform(cx2.cx,name,&[v.x,v.y,v.z]);}}
                    if light_dirty {dv.set_uniform(cx2.cx,live_id!(gi_relight),&[c.feedback,if linear{1.0}else{0.0},emission_gain,c.ray_distance]);
                    dv.set_uniform(cx2.cx,live_id!(gi_bounce),&[c.bounce]);}
                    if light_dirty {dv.set_uniform(cx2.cx,live_id!(gi_light_ids0),&light_ids[..4]);dv.set_uniform(cx2.cx,live_id!(gi_light_ids1),&light_ids[4..]);}
                }
                if i==2&&static_dirty {bind_texture(cx2.cx,dv,live_id!(gi_radiance),&radiance);}
                if i==3&&static_dirty {
                    bind_texture(cx2.cx,dv,live_id!(gi_batch),&batch_tex);
                    let a=c.atlas();
                    dv.set_uniform(cx2.cx,live_id!(gi_atlas),&a.map(|v|v as f32));
                    bind_texture(cx2.cx,dv,live_id!(gi_update),&update);
                    dv.set_uniform(cx2.cx,live_id!(gi_update_size),&[GATHER_WIDTH as f32,cap as f32]);
                }
            }
            let gpu=self.gpu.as_mut().unwrap();
            let atlas=c.atlas();
            let (width,height,target)=match i{0=>(RAYS+1,cap,&gpu.hits),1=>(RAYS,cap,&gpu.radiance),2=>(GATHER_WIDTH,cap,&gpu.update),_=>(atlas[0],atlas[1],&gpu.field)};
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
                0=>gpu.trace.quad.draw_abs(&mut cx2,Rect{pos:dvec2(0.0,0.0),size:dvec2(width as f64,rows as f64)}),
                1=>gpu.relight.quad.draw_abs(&mut cx2,Rect{pos:dvec2(0.0,0.0),size:dvec2(width as f64,rows as f64)}),
                2=>gpu.gather.quad.draw_abs(&mut cx2,Rect{pos:dvec2(0.0,0.0),size:dvec2(width as f64,rows as f64)}),
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
                    let (instances,count)=gpu.scatter_cache.batch(rows);
                    cx2.add_retained_instances_count(&mut gpu.scatter.quad.draw_vars,instances,count);
                },
            }
            cx2.end_pass_sized_turtle();stage.list.end(&mut cx2);cx2.end_pass(&stage.pass);
        }
    }

    /// Run SSAO over `depth` (the scene pass's hardware depth, holding the
    /// PREVIOUS frame) as child passes of `scene_pass`, at half resolution;
    /// returns the (ao, view distance) target. The scene pass's camera
    /// uniforms still hold the matrices that depth was drawn with.
    pub fn run_screen_ao(&mut self,cx:&mut Cx2d,scene_pass:DrawPassId,depth:&Texture,size:DVec2,params:crate::ssao::SsaoParams)->Option<Texture> {
        let p=cx.passes[scene_pass].pass_uniforms.camera_projection;
        // Nothing is drawn before the first frame (no projection yet).
        if p.v[0]==0.0||p.v[5]==0.0||size.x<2.0||size.y<2.0 {return None;}
        // The view's own matrices: the world frame is view space.
        let general=crate::ssao::SsaoGeneral::from_clip(p);
        self.ao.pass.run_general(cx,size*0.5,depth,general,params,scene_pass);
        self.stats.ao_gpu_ms=self.ao.pass.gpu_min_ms();
        self.ao.pass.output().cloned()
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

    /// The receivers read the field through `screen` (filled by the host
    /// with `gi_screen_value` over its G-buffer, at the pass's view) where
    /// it answers; None: every receiver gathers.
    pub(crate) fn set_screen(&mut self,screen:Option<(Texture,[f32;2])>){self.screen=screen;}
    pub fn bind(&self,cx:&Cx,dv:&mut DrawVars){
        let on=self.mode==GiMode::Fast&&self.gpu.is_some()&&!self.stats.rejected_scene&&self.cascades.iter().any(|c|c.origin.is_some());
        dv.set_uniform(cx,live_id!(gi_on),&[if on{self.config.strength}else{0.0}]);
        dv.set_uniform(cx,live_id!(gi_debug),&[self.debug as u8 as f32]);
        dv.set_uniform(cx,live_id!(gi_fallback_floor),&[RECEIVER_FALLBACK_FLOOR]);
        dv.set_uniform(cx,live_id!(gi_back_cut),&[if self.config.thin_walls{1.0}else{0.0}]);
        if !on {
            if let Some(id)=dv.draw_shader_id {if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==live_id!(gi_field)){dv.empty_texture(slot);}}
            return;
        }
        Self::cascade_uniforms(cx,dv,self.config,&self.cascades);
        bind_texture(cx,dv,live_id!(gi_field),&self.gpu.as_ref().unwrap().field);
        match &self.screen {
            Some((t,size))=>{bind_texture(cx,dv,live_id!(gi_screen),t);dv.set_uniform(cx,live_id!(gi_screen_ctl),&[1.0,size[0],size[1],0.0]);}
            None=>{
                bind_texture(cx,dv,live_id!(gi_screen),&self.gpu.as_ref().unwrap().field);
                dv.set_uniform(cx,live_id!(gi_screen_ctl),&[0.0,1.0,1.0,0.0]);
            }
        }
    }
}
pub(crate) fn bind_texture(cx:&Cx,dv:&mut DrawVars,name:LiveId,t:&Texture){if let Some(id)=dv.draw_shader_id{if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==name){dv.set_texture(slot,t);}}}

#[cfg(test)]
#[path="fast_gi/tests.rs"] mod tests;

#[cfg(test)]
mod gpu_guard;
