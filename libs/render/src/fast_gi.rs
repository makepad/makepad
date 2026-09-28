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
//!   their ordinary ambient: never darker than GI off where GI knows nothing.
//!
//! Needs float colour targets (Metal, WebGL2 + EXT_color_buffer_float);
//! elsewhere it stays off and allocates nothing.
use makepad_draw::*;
use std::sync::Arc;

#[path = "fast_gi/shaders.rs"]
mod shaders;
#[path = "fast_gi/voxels.rs"]
pub(crate) mod voxels;
#[path = "fast_gi/cascades.rs"]
mod cascades;
pub use shaders::script_mod;
pub(crate) use shaders::{DrawGiTrace, DrawGiRelight, DrawGiGather, DrawGiScatter};
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
    pub cascades: usize,
    /// Starting probe budget per frame; it then follows `gpu_budget_ms`.
    pub probes_per_frame: usize,
    pub ray_distance: f32,
    pub strength: f32,
    /// Fixed world-space volume center; None follows the game camera.
    pub anchor: Option<Vec3f>,
    /// Previous-field diffuse transport gain. 0 reproduces one bounce.
    pub feedback: f32,
    /// GPU time the GI passes aim for per frame.
    pub gpu_budget_ms: f32,
}
impl Default for GiConfig {
    fn default() -> Self { Self { grid:[16,8,16], spacing:1.0, cascades:3, probes_per_frame:128, ray_distance:48.0, strength:1.0, anchor:None, feedback:0.85, gpu_budget_ms:2.0 } }
}
impl GiConfig {
    pub fn clamped(self) -> Self {
        let sane=|x:f32,d:f32,lo:f32,hi:f32|if x.is_finite(){x.clamp(lo,hi)}else{d};
        let even=|x:usize,lo:usize,hi:usize|(x.clamp(lo,hi)+1)&!1;
        let mut grid=[even(self.grid[0],4,16),even(self.grid[1],2,16),even(self.grid[2],4,16)];
        // One field row per probe: stay within WebGL2's 2048 texture minimum.
        grid[1]=grid[1].min((2048/(grid[0]*grid[2]))&!1).max(2);
        Self { grid, cascades:self.cascades.clamp(1,MAX_CASCADES), anchor:self.anchor.filter(|p|finite(*p)), feedback:sane(self.feedback,0.85,0.0,0.95),
            spacing:sane(self.spacing,1.0,0.25,8.0), probes_per_frame:self.probes_per_frame.clamp(1,4096), ray_distance:sane(self.ray_distance,48.0,4.0,200.0),
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
    /// Latest async GPU durations of trace, relight, gather, scatter.
    pub gpu_ms: [f64;4],
    pub gpu_peak_ms: [f64;4],
    /// Screen AO passes (raw + 2 blurs), minimum observed.
    pub ao_gpu_ms: f64,
    pub resident_bytes: usize,
    pub uploads_bytes: usize,
    pub building: bool,
    pub waiting_for_worker: bool,
    pub rejected_scene: bool,
}

pub(crate) fn point(m:&Mat4f,p:Vec3f)->Vec3f { let q=m.transform_vec4(vec4(p.x,p.y,p.z,1.0)); vec3f(q.x,q.y,q.z) }
pub(crate) fn mul(a:Vec3f,b:Vec3f)->Vec3f {vec3f(a.x*b.x,a.y*b.y,a.z*b.z)}
pub(crate) fn finite(p:Vec3f)->bool {p.x.is_finite()&&p.y.is_finite()&&p.z.is_finite()}

/// Scene identity the host passes with a snapshot (revisions it knows of).
pub(crate) type SceneKey = [u64; 5];

type VoxelJob = makepad_platform::thread::TaskHandle<(u64, Box<VoxelWorld>, VoxelUpdate)>;

struct Stage {pass:DrawPass,list:DrawList}
struct Gpu {
    stages:Vec<Stage>, trace:DrawGiTrace, relight:DrawGiRelight, gather:DrawGiGather, scatter:DrawGiScatter,
    field:Texture, hits:Texture, radiance:Texture, update:Texture, batch:Texture,
    /// Info texels of slots that changed cells, zeroed in the next scatter.
    clears:Vec<(usize,usize)>,
    vox:[Texture;3], layout:VoxelLayout, capacity:usize,
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
    /// Idle voxel builder; None while a job owns it.
    voxels:Option<Box<VoxelWorld>>,
    voxel_job:Option<VoxelJob>,
    generation:u64,
    scene_key:Option<SceneKey>,
    scene_center:Option<Vec3f>,
    pending_scene:Option<Arc<Snapshot>>,
    has_scene:bool,
    /// Requested voxel windows (with hysteresis) and those the GPU holds.
    requested:Vec<[i32;3]>,
    committed:Vec<Option<[i32;3]>>,
    cascades:Vec<Cascade>,
    budget:f32, gpu_recent:Vec<f64>, frame:u64,
    pub(crate) meshes:MeshCache,
    ao:ScreenAo,
    pub stats:GiStats,
}
impl Default for FastGi {
    fn default()->Self{Self{mode:GiMode::Off,config:GiConfig::default(),debug:GiDebug::Off,linear_albedo:false,emission_gain:1.0,gpu:None,voxels:None,voxel_job:None,generation:0,
        scene_key:None,scene_center:None,pending_scene:None,has_scene:false,requested:Vec::new(),committed:Vec::new(),cascades:Vec::new(),
        budget:0.0,gpu_recent:Vec::new(),frame:0,meshes:MeshCache::default(),ao:ScreenAo::default(),stats:GiStats::default()}}
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
        let transport_only=GiConfig{strength:c.strength,feedback:c.feedback,gpu_budget_ms:c.gpu_budget_ms,probes_per_frame:c.probes_per_frame,ray_distance:c.ray_distance,..self.config};
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
    /// Drop everything (mode or layout change).
    fn release(&mut self){
        if let Some(job)=self.voxel_job.take(){job.cancel();}
        self.generation=self.generation.wrapping_add(1);
        self.gpu=None;self.voxels=None;self.scene_key=None;self.scene_center=None;self.pending_scene=None;self.has_scene=false;
        self.requested.clear();self.committed.clear();self.cascades.clear();self.budget=0.0;self.gpu_recent.clear();
        self.meshes=MeshCache::default();self.stats=GiStats::default();
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
        self.stats.triangles=snapshot.triangles;self.stats.instances=snapshot.instances.len();
        self.scene_key=Some(key);self.scene_center=Some(center);
        self.pending_scene=Some(Arc::new(snapshot));
    }
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
        let stages=["GI trace","GI relight","GI gather","GI scatter"].into_iter().map(|name|{let pass=DrawPass::new_with_name(cx,name);pass.set_gpu_timing_enabled(cx,true);Stage{pass,list:DrawList::new(cx)}}).collect();
        let atlas=c.atlas();
        let field=Texture::new_with_format(cx,TextureFormat::RenderRGBAf16{size:TextureSize::Fixed{width:atlas[0],height:atlas[1]},initial:true});
        let vox=[0,1,2].map(|_|Texture::new_with_format(cx,TextureFormat::VecBGRAu8_32{width:layout.width(),height:layout.height(),data:Some(vec![0;layout.width()*layout.height()]),updated:TextureUpdated::Full}));
        self.gpu=Some(Gpu{stages,trace,relight,gather,scatter,field,hits:Texture::new(cx),radiance:Texture::new(cx),update:Texture::new(cx),batch:Texture::new(cx),clears:Vec::new(),vox,layout,capacity:0});
        self.cascades=(0..c.cascades).map(|k|Cascade::new(c.grid,c.cascade_spacing(k))).collect();
        self.committed=vec![None;c.cascades];
        self.budget=c.probes_per_frame as f32;
        true
    }

    /// Adopt a finished voxel job: copy its changed levels into the voxel
    /// textures, move the cascades onto the new windows, queue edited probes.
    fn poll_voxels(&mut self,cx:&mut Cx) {
        let Some(result)=self.voxel_job.as_mut().and_then(|j|j.try_take()) else {return};
        self.voxel_job=None;self.stats.building=false;
        let (generation,world,update)=match result {
            Ok(r)=>r,
            Err(e)=>{log!("fast GI voxel worker failed: {e:?}");self.scene_key=None;return;}
        };
        if generation!=self.generation{return;}
        let gpu=self.gpu.as_mut().unwrap();
        let layout=world.layout;
        for (level,changed) in update.levels.iter().enumerate() {
            if !changed{continue;}
            let (start,len)=voxels::level_span(&layout,level);
            for (tex,src) in gpu.vox.iter().zip([&world.albedo,&world.normal,&world.emission]) {
                let mut data=tex.take_vec_u32(cx);
                data[start..start+len].copy_from_slice(&src[start..start+len]);
                tex.put_back_vec_u32(cx,data,Some(RectUsize::new(PointUsize::new(0,level*layout.level_height()),SizeUsize::new(layout.width(),layout.level_height()))));
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
            if !first {for (lo,hi) in &update.dirty {self.stats.dirty_probes+=self.cascades[k].invalidate([lo.x,lo.y,lo.z],[hi.x,hi.y,hi.z],HOME);}}
        }
        self.stats.voxel_jobs+=1;self.stats.voxel_bricks=update.bricks;self.stats.voxel_ms=update.ms;
        if std::env::var_os("MAKEPAD_GI_STATS").is_some() {
            log!("fast GI voxels: {} bricks / {} triangles in {:.1}ms (worker), {} edit boxes -> {} probes, scroll {} probes",update.bricks,update.triangles,update.ms,update.dirty.len(),self.stats.dirty_probes,self.stats.scroll_probes);
        }
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
        let Ok(slot)=cx.task_pool().reserve(makepad_platform::thread::Lane::Heavy) else {
            if !self.stats.waiting_for_worker{log!("fast GI: worker queue busy; retrying next frame");}
            self.stats.waiting_for_worker=true;return;
        };
        self.stats.waiting_for_worker=false;self.stats.building=true;
        // The first world (its 4.7 MB mirror) is allocated on the worker.
        let world=self.voxels.take();
        let scene=self.pending_scene.take();self.has_scene=true;
        let windows=self.requested.clone();let generation=self.generation;
        self.voxel_job=Some(slot.submit(move||{
            let mut world=world.unwrap_or_else(||Box::new(VoxelWorld::new(layout)));
            let update=world.update(scene,&windows);(generation,world,update)
        }));
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
    }

    fn cascade_uniforms(cx:&Cx,dv:&mut DrawVars,c:GiConfig,cascades:&[Cascade]) {
        dv.set_uniform(cx,live_id!(gi_grid),&[c.grid[0]as f32,c.grid[1]as f32,c.grid[2]as f32,c.probe_count()as f32]);
        for (k,name) in [live_id!(gi_c0),live_id!(gi_c1),live_id!(gi_c2)].into_iter().enumerate() {
            let v=match cascades.get(k).and_then(|x|x.origin){Some(o)=>[o[0]as f32,o[1]as f32,o[2]as f32,c.cascade_spacing(k)],None=>[0.0;4]};
            dv.set_uniform(cx,name,&v);
        }
        dv.set_uniform(cx,live_id!(gi_home),&[HOME[0],HOME[1],HOME[2],cascades.len()as f32]);
        let a=c.atlas();
        dv.set_uniform(cx,live_id!(gi_atlas),&[a[0]as f32,a[1]as f32,a[2]as f32,a[3]as f32]);
    }
    fn voxel_uniforms(cx:&Cx,dv:&mut DrawVars,l:VoxelLayout,vox:&[Texture;3],committed:&[Option<[i32;3]>]) {
        dv.set_uniform(cx,live_id!(gi_vox_dims),&[l.dims[0]as f32,l.dims[1]as f32,l.dims[2]as f32,l.levels as f32]);
        dv.set_uniform(cx,live_id!(gi_vox_tex),&[l.width()as f32,l.height()as f32,l.level_height()as f32,voxels::TILE_COLUMNS as f32]);
        for (k,name) in [live_id!(gi_vox0),live_id!(gi_vox1),live_id!(gi_vox2)].into_iter().enumerate() {
            let v=match committed.get(k).copied().flatten(){Some(o)=>[o[0]as f32,o[1]as f32,o[2]as f32,l.voxel_size(k)],None=>[0.0;4]};
            dv.set_uniform(cx,name,&v);
        }
        for (name,t) in [live_id!(gi_vox_a),live_id!(gi_vox_b),live_id!(gi_vox_c)].into_iter().zip(vox){bind_texture(cx,dv,name,t);}
    }

    pub fn run(&mut self,cx:&mut CxDraw,center:Vec3f,sun:&crate::sun::SunLight,clustered:&crate::clustered::ClusteredLights) {
        if self.mode==GiMode::Off{return;}
        let center=self.config.anchor.unwrap_or(center);
        let start=Cx::monotonic_now();self.stats.traced_rays=0;self.stats.uploads_bytes=0;self.stats.batch=0;
        if !finite(center)||!self.ensure(cx.cx){return;}
        self.frame=self.frame.wrapping_add(1);
        self.poll_voxels(cx.cx);
        self.launch_voxels(cx,center);
        let c=self.config;let count=c.probe_count();
        // Probe budget follows the measured GPU time of the four passes.
        // The timers are async and include queueing behind other GPU work,
        // so the controller reads the MINIMUM of the recent samples: shrink
        // when even the best frame is over target, grow when it is well
        // under; never below a floor that keeps GI converging.
        {
            let gpu=self.gpu.as_ref().unwrap();let mut sample=false;
            for (i,st) in gpu.stages.iter().enumerate() {
                for ms in st.pass.take_gpu_times_ms(cx.cx){self.stats.gpu_ms[i]=ms;self.stats.gpu_peak_ms[i]=self.stats.gpu_peak_ms[i].max(ms);sample=true;}
            }
            if sample {
                self.gpu_recent.push(self.stats.gpu_ms.iter().sum());
                if self.gpu_recent.len()>=30 {
                    let best=self.gpu_recent.iter().copied().fold(f64::MAX,f64::min);
                    self.gpu_recent.clear();
                    let target=c.gpu_budget_ms as f64;
                    if best>target {self.budget*=0.8;} else if best<target*0.6 {self.budget*=1.25;}
                    self.budget=self.budget.clamp(64.0,(count*self.cascades.len())as f32);
                }
            }
        }
        self.stats.probes=count*self.cascades.len();
        self.stats.budget=self.budget as usize;
        let batch=cascades::schedule(&mut self.cascades,self.stats.budget);
        self.stats.pending_probes=self.cascades.iter().map(|c|c.pending()).sum();
        if batch.is_empty(){self.stats.encode_us=((Cx::monotonic_now()-start)*1e6)as u64;return;}
        self.ensure_capacity(cx.cx,batch.len());
        self.encode(cx,&batch,center,sun,clustered);
        self.stats.batch=batch.len();self.stats.traced_rays=batch.len()*RAYS;
        self.stats.resident_bytes=self.resident_bytes();
        self.stats.encode_us=((Cx::monotonic_now()-start)*1e6)as u64;
    }

    fn resident_bytes(&self)->usize {
        let Some(gpu)=&self.gpu else{return 0};
        let l=gpu.layout;
        let a=self.config.atlas();
        a[0]*a[1]*8+(RAYS*2+1+2)*gpu.capacity*16+GATHER_WIDTH*gpu.capacity*8
            +l.width()*l.height()*4*3*2+self.meshes.bytes()
    }

    fn encode(&mut self,cx:&mut CxDraw,batch:&[BatchItem],center:Vec3f,sun:&crate::sun::SunLight,clustered:&crate::clustered::ClusteredLights) {
        let c=self.config;
        let rows=batch.len();
        {
            let gpu=self.gpu.as_mut().unwrap();
            let mut data=gpu.batch.take_vec_f32(cx.cx);
            for (i,b) in batch.iter().enumerate() {
                data[i*8..i*8+8].copy_from_slice(&[b.slot as f32,b.cascade as f32,b.hysteresis,0.0,b.cell[0]as f32,b.cell[1]as f32,b.cell[2]as f32,0.0]);
            }
            gpu.batch.put_back_vec_f32(cx.cx,data,Some(RectUsize::new(PointUsize::new(0,0),SizeUsize::new(2,rows))));
        }
        self.stats.uploads_bytes+=rows*32;
        let light_ids=clustered.gi_light_ids(center);self.stats.selected_lights=light_ids.iter().filter(|&&i|i>=0.0).count();
        let cap=self.gpu.as_ref().unwrap().capacity;
        let pass_uniform=[rows as f32,cap as f32,c.ray_distance,(self.frame%4096)as f32];
        let linear=self.linear_albedo;let emission_gain=self.emission_gain;
        let mut cx2=Cx2d::new(cx);
        for i in 0..4 {
            {
                let FastGi{gpu,cascades,committed,..}=&mut *self;
                let gpu=gpu.as_mut().unwrap();
                let (batch_tex,field,hits,radiance,update,vox,layout)=(gpu.batch.clone(),gpu.field.clone(),gpu.hits.clone(),gpu.radiance.clone(),gpu.update.clone(),gpu.vox.clone(),gpu.layout);
                let dv=match i{0=>&mut gpu.trace.quad.draw_vars,1=>&mut gpu.relight.quad.draw_vars,2=>&mut gpu.gather.quad.draw_vars,_=>&mut gpu.scatter.quad.draw_vars};
                if i<3 {
                    Self::cascade_uniforms(cx2.cx,dv,c,cascades);
                    dv.set_uniform(cx2.cx,live_id!(gi_pass),&pass_uniform);
                    bind_texture(cx2.cx,dv,live_id!(gi_batch),&batch_tex);
                }
                if i<2 {Self::voxel_uniforms(cx2.cx,dv,layout,&vox,committed);}
                if i==1||i==2 {
                    // Producers read last frame's field: feedback, history.
                    dv.set_uniform(cx2.cx,live_id!(gi_on),&[1.0]);
                    dv.set_uniform(cx2.cx,live_id!(gi_fallback_floor),&[0.0]);
                    dv.set_uniform(cx2.cx,live_id!(gi_debug),&[0.0]);
                    bind_texture(cx2.cx,dv,live_id!(gi_field),&field);
                    bind_texture(cx2.cx,dv,live_id!(gi_hits),&hits);
                }
                if i==1 {
                    clustered.bind(cx2.cx,dv,true);
                    for (name,v) in [(live_id!(gi_sun_dir),sun.dir),(live_id!(gi_sun_color),sun.color),(live_id!(gi_sky),sun.sky),(live_id!(gi_ground),sun.ground)]{dv.set_uniform(cx2.cx,name,&[v.x,v.y,v.z]);}
                    dv.set_uniform(cx2.cx,live_id!(gi_relight),&[c.feedback,if linear{1.0}else{0.0},emission_gain,c.ray_distance]);
                    dv.set_uniform(cx2.cx,live_id!(gi_light_ids0),&light_ids[..4]);dv.set_uniform(cx2.cx,live_id!(gi_light_ids1),&light_ids[4..]);
                }
                if i==2 {bind_texture(cx2.cx,dv,live_id!(gi_radiance),&radiance);}
                if i==3 {
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
            if let Some(next)=next{cx2.passes[stage.pass.draw_pass_id()].parent=CxDrawPassParent::DrawPass(next);}
            cx2.begin_pass(&stage.pass,Some(1.0));
            stage.pass.set_size(cx2.cx,dvec2(width as f64,height as f64));stage.pass.clear_color_textures(cx2.cx);
            // The field keeps its contents; the batch targets are rewritten.
            let clear=if i==3{DrawPassClearColor::InitWith(vec4(0.0,0.0,0.0,0.0))}else{DrawPassClearColor::ClearWith(vec4(0.0,0.0,0.0,0.0))};
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
                    for (k,slot) in std::mem::take(&mut gpu.clears) {
                        gpu.scatter.gi_tile=vec4(-1.0,0.0,1.0,0.0);
                        gpu.scatter.quad.draw_abs(&mut cx2,Rect{pos:tile(k,slot,2),size:dvec2(1.0,1.0)});
                    }
                    for (row,b) in batch.iter().enumerate() {
                        for (region,col,w) in [(0,0.0,8.0),(1,64.0,10.0),(2,164.0,1.0)] {
                            gpu.scatter.gi_tile=vec4(row as f32,col,w as f32,0.0);
                            gpu.scatter.quad.draw_abs(&mut cx2,Rect{pos:tile(b.cascade,b.slot,region),size:dvec2(w,w)});
                        }
                    }
                },
            }
            cx2.end_pass_sized_turtle();stage.list.end(&mut cx2);cx2.end_pass(&stage.pass);
        }
    }

    /// Run SSAO over `depth` (the scene pass's hardware depth, holding the
    /// PREVIOUS frame) as child passes of `scene_pass`, at half resolution;
    /// returns the (ao, view distance) target. The scene pass's camera
    /// uniforms still hold the matrices that depth was drawn with.
    pub fn run_screen_ao(&mut self,cx:&mut Cx2d,scene_pass:DrawPassId,depth:&Texture,size:DVec2)->Option<Texture> {
        let p=cx.passes[scene_pass].pass_uniforms.camera_projection;
        // Perspective only (w = -z); nothing is drawn before the first frame.
        if p.v[11]!=-1.0||p.v[0]==0.0||p.v[5]==0.0||size.x<2.0||size.y<2.0 {return None;}
        self.ao.pass.hardware_depth=Some((p.v[10],p.v[14]));
        let proj=crate::ssao::SsaoProjection{ortho:false,half_x:1.0/p.v[0],half_y:1.0/p.v[5]};
        self.ao.pass.run(cx,size*0.5,depth,proj,crate::ssao::SsaoParams::default(),scene_pass);
        self.stats.ao_gpu_ms=self.ao.pass.gpu_min_ms();
        self.ao.pass.output().cloned()
    }

    pub fn bind(&self,cx:&Cx,dv:&mut DrawVars){
        let on=self.mode==GiMode::Fast&&self.gpu.is_some()&&!self.stats.rejected_scene&&self.cascades.iter().any(|c|c.origin.is_some());
        dv.set_uniform(cx,live_id!(gi_on),&[if on{self.config.strength}else{0.0}]);
        dv.set_uniform(cx,live_id!(gi_debug),&[self.debug as u8 as f32]);
        dv.set_uniform(cx,live_id!(gi_fallback_floor),&[RECEIVER_FALLBACK_FLOOR]);
        if !on {
            if let Some(id)=dv.draw_shader_id {if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==live_id!(gi_field)){dv.empty_texture(slot);}}
            return;
        }
        Self::cascade_uniforms(cx,dv,self.config,&self.cascades);
        bind_texture(cx,dv,live_id!(gi_field),&self.gpu.as_ref().unwrap().field);
    }
}
pub(crate) fn bind_texture(cx:&Cx,dv:&mut DrawVars,name:LiveId,t:&Texture){if let Some(id)=dv.draw_shader_id{if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==name){dv.set_texture(slot,t);}}}

#[cfg(test)]
#[path="fast_gi/tests.rs"] mod tests;
