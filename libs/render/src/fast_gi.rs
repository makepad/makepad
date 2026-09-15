//! Incremental diffuse GI. Geometry preparation runs on the platform pool;
//! all runtime rays/relighting run in small fragment passes, not on the CPU.
//! No compute, readback, screen-space dependence or full-resolution prepass.
use makepad_draw::*;
use makepad_raytrace::bvh::{Bvh, Tri};
use std::sync::Arc;

#[path = "fast_gi/shaders.rs"]
mod shaders;
#[path = "fast_gi/placement.rs"]
mod placement;
#[path = "fast_gi/emitters.rs"]
mod emitters;
pub use shaders::script_mod;
pub(crate) use shaders::{DrawGiTrace, DrawGiRelight, DrawGiGather};

pub const DATA_WIDTH: usize = 256;
pub const RAYS: usize = 64;
// 0..35 irradiance/moments, 36 cell blocker ids, 37..39 global OBB rows
// in the first MAX_BLOCKERS records. One scene sampler, even for full PBR.
pub const PROBE_TEXELS: usize = 40;
pub const MAX_TRIANGLES: usize = 100_000;
pub const MAX_MOVERS: usize = 32;
pub const MAX_STATIC_BLOCKERS: usize = 32;
pub const MAX_BLOCKERS: usize = MAX_MOVERS + MAX_STATIC_BLOCKERS;
pub const MAX_LIGHTS: usize = 8;
const STARTUP_SWEEPS: usize = 3;
const STARTUP_FADE_SECONDS: f64 = 1.2;

/// Presentation readiness is independent of transport: warm up the actual
/// field at full feedback strength, then reveal it without scanline builds.
#[derive(Default)]
struct GiStartup {
    sweeps: usize,
    fade_started: Option<f64>,
}
impl GiStartup {
    fn advance(&mut self, completed_sweep: bool, now: f64) -> f32 {
        if completed_sweep && self.sweeps < STARTUP_SWEEPS {
            self.sweeps += 1;
            if self.sweeps == STARTUP_SWEEPS { self.fade_started = Some(now); }
        }
        let Some(started) = self.fade_started else { return 0.0; };
        let t = ((now - started) / STARTUP_FADE_SECONDS).clamp(0.0, 1.0) as f32;
        t * t * (3.0 - 2.0 * t)
    }
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
    pub grid: [usize; 3],
    pub spacing: f32,
    pub probes_per_frame: usize,
    pub ray_distance: f32,
    pub strength: f32,
    /// Fixed world-space volume center; None follows the game camera.
    pub anchor: Option<Vec3f>,
    /// Previous-field diffuse transport gain. 0 reproduces one bounce.
    pub feedback: f32,
}
impl Default for GiConfig {
    fn default() -> Self { Self { grid:[12,8,12], spacing:1.5, probes_per_frame:32, ray_distance:32.0, strength:1.0, anchor:None, feedback:0.85 } }
}
impl GiConfig {
    pub fn clamped(self) -> Self {
        let sane=|x:f32,d:f32,lo:f32,hi:f32|if x.is_finite(){x.clamp(lo,hi)}else{d};
        let mut grid=self.grid.map(|x|x.clamp(2,16));
        // Stay within WebGL2's minimum 2048 texture dimension.
        grid[1]=grid[1].min(2048/(grid[0]*grid[2]));
        Self { grid, anchor:self.anchor.filter(|p|finite(*p)), feedback:sane(self.feedback,0.85,0.0,0.95), spacing:sane(self.spacing,2.0,0.5,8.0),
            probes_per_frame:self.probes_per_frame.clamp(1,256), ray_distance:sane(self.ray_distance,32.0,4.0,100.0), strength:sane(self.strength,1.0,0.0,1.0) }
    }
    pub fn probe_count(self) -> usize { self.grid.iter().product() }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GiStats {
    pub triangles: usize,
    pub probes: usize,
    pub ready_probes: usize,
    /// Completed initial relight sweeps, capped at the warmup target.
    pub startup_sweeps: usize,
    /// Display-only blend; transport continues at full strength during warmup.
    pub display_blend: f32,
    pub traced_rays: usize,
    pub trace_node_limit: usize,
    pub relit_rays: usize,
    pub mover_count: usize,
    pub static_blockers: usize,
    /// In-volume boxes beyond the exact-segment budget still use BVH moments.
    pub static_blocker_fallbacks: usize,
    pub omitted_movers: usize,
    pub selected_lights: usize,
    /// Static box emitters integrated separately from the 64 visibility rays.
    pub selected_emitters: usize,
    pub emitter_fallbacks: usize,
    pub emitter_probe_fallbacks: usize,
    pub preparation_ms: f64,
    pub placement_ms: f64,
    pub relocated_probes: usize,
    pub inactive_probes: usize,
    pub placement_exhausted: usize,
    pub preparation_cpu_bytes: usize,
    pub blocker_cells: usize,
    pub blocker_overflow_cells: usize,
    pub snapshot_us: u64,
    pub encode_us: u64,
    /// Latest async per-pass GPU command-buffer durations; not total scene cost.
    pub gpu_ms: [f64;3],
    pub gpu_peak_ms: [f64;3],
    pub resident_bytes: usize,
    pub uploads_bytes: usize,
    pub building: bool,
    pub waiting_for_worker: bool,
    pub rejected_scene: bool,
}

#[derive(Clone)]
pub(crate) struct Mesh {
    pub vertices: Vec<f32>, pub indices:Vec<u32>, pub stride:usize,
    /// -1 uniform white; 5 packed model color; 8 unpacked terrain color.
    pub color_lane: i32,
    pub image: Option<(usize,usize,Vec<u32>)>,
}
#[derive(Clone)]
pub(crate) struct Instance {
    pub mesh:Arc<Mesh>, pub transform:Mat4f, pub tint:Vec3f,
    pub emission:Vec3f, pub diffuse:f32,
    /// Only actual unit-box render geometry, never an arbitrary mesh's AABB.
    pub exact_box:bool,
}
#[derive(Clone, Copy)]
pub(crate) struct Mover {
    pub transform:Mat4f, pub min:Vec3f, pub max:Vec3f,
    pub color:Vec3f, pub emission:Vec3f,
    /// A rendered solid box can reject receiver/probe visibility. An actor's
    /// bounds only approximate transport; their interior may contain air.
    pub exact_box:bool,
}

pub(crate) struct Prepared {
    nodes:Vec<f32>, triangles:Vec<f32>, count:usize, node_count:usize, ms:f64,
    bvh:Arc<Bvh>,
    static_boxes:Arc<Vec<Mover>>,
    emitters:Arc<Vec<emitters::Emitter>>,
}
fn padded(mut data:Vec<f32>) -> Vec<f32> {
    data.resize(((data.len()+DATA_WIDTH*4-1)/(DATA_WIDTH*4)).max(1)*DATA_WIDTH*4,0.0); data
}
fn point(m:&Mat4f,p:Vec3f)->Vec3f { let q=m.transform_vec4(vec4(p.x,p.y,p.z,1.0)); vec3f(q.x,q.y,q.z) }
fn mul(a:Vec3f,b:Vec3f)->Vec3f {vec3f(a.x*b.x,a.y*b.y,a.z*b.z)}
fn finite(p:Vec3f)->bool {p.x.is_finite()&&p.y.is_finite()&&p.z.is_finite()}

fn mover_bounds(m:&Mover)->(Vec3f,Vec3f) {
    let mut lo=vec3f(f32::MAX,f32::MAX,f32::MAX);let mut hi=lo*(-1.0);
    for z in [m.min.z,m.max.z] {for y in [m.min.y,m.max.y] {for x in [m.min.x,m.max.x] {
        let p=point(&m.transform,vec3f(x,y,z));
        lo=vec3f(lo.x.min(p.x),lo.y.min(p.y),lo.z.min(p.z));
        hi=vec3f(hi.x.max(p.x),hi.y.max(p.y),hi.z.max(p.z));
    }}}
    (lo,hi)
}
fn mover_record(m:&Mover)->[f32;24] {
    let mid=(m.min+m.max)*0.5;let half=(m.max-m.min)*0.5;
    let mut t=m.transform;
    let p=point(&t,mid);t.v[12]=p.x;t.v[13]=p.y;t.v[14]=p.z;
    for j in 0..3{t.v[j]*=half.x.max(0.001);t.v[4+j]*=half.y.max(0.001);t.v[8+j]*=half.z.max(0.001);}
    let inv=t.invert();let mut data=[0.0;24];
    for row in 0..3{for col in 0..4{data[row*4+col]=inv.v[col*4+row];}}
    data[12..15].copy_from_slice(&[m.color.x,m.color.y,m.color.z]);
    // Existing spare color.w carries the solid-box contract to relight and
    // gather. Approximate movers retain their full transport transforms.
    data[15]=if m.exact_box{1.0}else{0.0};
    data[16..19].copy_from_slice(&[m.emission.x,m.emission.y,m.emission.z]);
    data
}
fn cell_blockers(cell:Vec3f,spacing:f32,bounds:&[(Vec3f,Vec3f)])->[f32;4] {
    // Includes relocated endpoints (+/- .45 cells) and receiver lookup bias.
    let padding=vec3f(0.6,0.6,0.6)*spacing;
    let lo=cell-padding;let hi=cell+vec3f(spacing,spacing,spacing)+padding;
    let mut out=[-1.0;4];let mut count=0;
    for (i,(a,b)) in bounds.iter().enumerate() {
        if b.x<lo.x||a.x>hi.x||b.y<lo.y||a.y>hi.y||b.z<lo.z||a.z>hi.z{continue;}
        // Crowded cells take the bounded full-list path. Never omit a wall.
        if count==4{return [-2.0,-1.0,-1.0,-1.0];}
        out[count]=i as f32;count+=1;
    }
    out
}
fn merge_blockers(dynamic:[f32;4],statics:[f32;4],dynamic_count:usize)->[f32;4] {
    if dynamic[0]==-2.0 || statics[0]==-2.0 {return [-2.0,-1.0,-1.0,-1.0];}
    let mut out=[-1.0;4];let mut count=0;
    for id in dynamic.into_iter().filter(|id|*id>=0.0).chain(statics.into_iter().filter(|id|*id>=0.0).map(|id|id+dynamic_count as f32)) {
        if count==4{return [-2.0,-1.0,-1.0,-1.0];}
        out[count]=id;count+=1;
    }
    out
}
fn producer_blocker_count(pass:usize,previous:usize,current:usize)->usize {
    // Relight samples history, whereas gather writes today's topology.
    // A new actor must not make history read unwritten (all-zero) OBB rows.
    if pass==1 {previous}else{current}
}
fn trace_budget(probes:usize,nodes:usize)->(usize,usize) {
    let limit=nodes.clamp(256,makepad_raytrace::bvh::MAX_STEPS as usize);
    // One whole probe is the minimum dispatch granularity.
    ((probes*256/limit).max(1),limit)
}
fn emitter_probe_budget(probes:usize,ray_width:usize)->usize {
    // Preserve the ray-slot budget. One full probe remains the minimum unit.
    (probes*RAYS/ray_width).max(1)
}
impl Prepared {
    pub(crate) fn build(instances:Vec<Instance>) -> Result<Self,String> {
        let start=Cx::monotonic_now();
        let count:usize=instances.iter().map(|i|i.mesh.indices.len()/3).sum();
        if count>MAX_TRIANGLES{return Err(format!("GI scene has {count} triangles; limit {MAX_TRIANGLES}; GI disabled rather than dropping occluders"));}
        let mut triangles=Vec::with_capacity(count); let mut material=Vec::with_capacity(count);let mut static_boxes=Vec::new();let mut emitters=Vec::new();
        for i in instances {
            let mesh=&i.mesh;
            if mesh.stride<3 || (mesh.color_lane==5 && mesh.stride<6) || (mesh.color_lane>=0 && mesh.color_lane!=5 && mesh.stride<(mesh.color_lane as usize+3)) {return Err("invalid GI vertex layout".into());}
            if !finite(i.tint)||!finite(i.emission)||!i.diffuse.is_finite(){return Err("nonfinite GI material".into());}
            let mut emitter_id=0.0;
            if i.exact_box {
                let t=&i.transform.v;
                let determinant=vec3f(t[0],t[1],t[2]).dot(Vec3f::cross(vec3f(t[4],t[5],t[6]),vec3f(t[8],t[9],t[10])));
                if determinant.is_finite()&&determinant.abs()>1e-12 {
                    let box_=Mover{transform:i.transform,min:vec3f(-0.5,-0.5,-0.5),max:vec3f(0.5,0.5,0.5),color:Vec3f::default(),emission:i.emission,exact_box:true};
                    static_boxes.push(box_);
                    if i.emission.x>=0.0&&i.emission.y>=0.0&&i.emission.z>=0.0&&i.emission.x.max(i.emission.y).max(i.emission.z)>0.0 {
                        emitter_id=(emitters.len()+1)as f32;
                        emitters.push(emitters::Emitter{box_,id:emitter_id});
                    }
                }
            }
            for ids in mesh.indices.chunks_exact(3) {
                let mut p=[Vec3f::default();3];let mut color=Vec3f::default();
                for k in 0..3 {
                    let at=ids[k]as usize*mesh.stride;
                    let v=mesh.vertices.get(at..at+mesh.stride).ok_or("invalid GI mesh index")?;
                    p[k]=point(&i.transform,vec3f(v[0],v[1],v[2]));
                    if !finite(p[k]){return Err("nonfinite GI geometry".into());}
                    let mut c=vec3f(1.0,1.0,1.0);
                    if mesh.color_lane==5 {
                        let bits=v[5].to_bits();c=vec3f((bits&255)as f32,((bits>>8)&255)as f32,((bits>>16)&255)as f32)*(1.0/255.0);
                        if let Some((w,h,pixels))=&mesh.image {if *w>0 && *h>0 {
                            let (u,t)=makepad_draw::vector::unpack_pair_f16(v[4]);
                            let x=(u.rem_euclid(1.0)*(*w as f32))as usize;
                            let y=(t.rem_euclid(1.0)*(*h as f32))as usize;
                            if let Some(&px)=pixels.get(y.min(h-1)*w+x.min(w-1)) {
                                c=mul(c,vec3f(((px>>16)&255)as f32,((px>>8)&255)as f32,(px&255)as f32)*(1.0/255.0));
                            }
                        }}
                    } else if mesh.color_lane>=0 {let j=mesh.color_lane as usize;c=vec3f(v[j],v[j+1],v[j+2]);}
                    color=color+c*(1.0/3.0);
                }
                if Vec3f::cross(p[1]-p[0],p[2]-p[0]).length()<1e-8 {continue;}
                triangles.push(Tri{v0:p[0],v1:p[1],v2:p[2]});
                let c=mul(color,i.tint)*i.diffuse;
                material.push((vec3f(c.x.clamp(0.0,0.9),c.y.clamp(0.0,0.9),c.z.clamp(0.0,0.9)),i.emission,emitter_id));
            }
        }
        let bvh=Bvh::build(&triangles);let mut data=Vec::with_capacity(bvh.tris.len()*20);
        for (j,t) in bvh.tris.iter().enumerate() {
            let (c,e,id)=material[bvh.tri_order[j]as usize];
            for p in [t.v0,t.v1,t.v2,c] {data.extend([p.x,p.y,p.z,0.0]);}
            data.extend([e.x,e.y,e.z,id]);
        }
        Ok(Self{nodes:padded(bvh.texels()),triangles:padded(data),count:bvh.tris.len(),node_count:bvh.nodes.len(),ms:(Cx::monotonic_now()-start)*1e3,bvh:Arc::new(bvh),static_boxes:Arc::new(static_boxes),emitters:Arc::new(emitters)})
    }
}

struct Stage {pass:DrawPass,list:DrawList}
struct Gpu {
    stages:Vec<Stage>, trace:DrawGiTrace, relight:DrawGiRelight, gather:DrawGiGather,
    nodes:Texture, triangles:Texture, hits:Texture, radiance:Texture, probes:Texture, history:Texture, movers:Texture, sun_rows:Texture,
    nodes_height:usize,tri_height:usize,node_count:usize,
    positions:Texture, bvh:Arc<Bvh>, placed:bool,
    static_boxes:Arc<Vec<Mover>>, static_visibility:placement::StaticVisibility,
    field_blockers:usize,
    emitters:Arc<Vec<emitters::Emitter>>, emitter_count:usize,
    ray_width:usize, position_width:usize,
}
struct PreparedPlacement { placement:placement::Placement, samples:emitters::Samples }
struct DisplayField { origin:Vec3f, config:GiConfig, blockers:usize, bank:f32 }
pub(crate) struct FastGi {
    mode:GiMode, config:GiConfig, gpu:Option<Gpu>,
    debug:GiDebug,
    placement:Option<makepad_platform::thread::TaskHandle<(u64,Arc<PreparedPlacement>)>>,
    placement_generation:u64,
    job:Option<makepad_platform::thread::TaskHandle<(u64,Result<Arc<Prepared>,String>)>>,
    generation:u64, revision:Option<(u64,u64,u64)>, pending:Option<Arc<Prepared>>,
    origin:Vec3f, origin_key:Option<[i32;3]>, cursor:usize, traced:usize,
    startup:GiStartup,
    previous_display:Option<DisplayField>,
    pub stats:GiStats,
}
impl Default for FastGi {
    fn default()->Self{Self{mode:GiMode::Off,debug:GiDebug::Off,config:GiConfig::default(),gpu:None,placement:None,placement_generation:0,job:None,generation:0,revision:None,pending:None,origin:Vec3f::default(),origin_key:None,cursor:0,traced:0,startup:GiStartup::default(),previous_display:None,stats:GiStats::default()}}
}
impl FastGi {
    pub fn mode(&self)->GiMode{self.mode}
    pub fn debug(&self)->GiDebug{self.debug}
    pub fn set_debug(&mut self,debug:GiDebug){self.debug=debug;}
    pub fn config(&self)->GiConfig{self.config}
    pub fn set_mode(&mut self,mode:GiMode){if self.mode!=mode{self.reset();self.mode=mode;}}
    pub fn set_config(&mut self,c:GiConfig){
        let c=c.clamped();
        // Lighting-only controls do not invalidate geometry or the field.
        let transport_only=GiConfig{strength:c.strength,feedback:c.feedback,..self.config};
        if c!=transport_only{self.reset();}
        self.config=c;
    }
    fn reset_progress(&mut self){self.cursor=0;self.traced=0;self.startup=GiStartup::default();self.stats.ready_probes=0;self.stats.startup_sweeps=0;self.stats.display_blend=0.0;}
    pub fn reset(&mut self){if let Some(job)=self.job.take(){job.cancel();}if let Some(job)=self.placement.take(){job.cancel();}self.placement_generation=self.placement_generation.wrapping_add(1);self.gpu=None;self.previous_display=None;self.pending=None;self.revision=None;self.generation=self.generation.wrapping_add(1);self.origin_key=None;self.reset_progress();self.stats=GiStats::default();}
    /// Reserve BEFORE extracting scene data; queue pressure never loses a job.
    pub fn needs_scene(&self,key:(u64,u64,u64))->bool{self.mode!=GiMode::Off&&self.revision!=Some(key)&&self.job.is_none()}
    pub fn reject(&mut self,key:(u64,u64,u64),error:&str){self.reset();self.revision=Some(key);self.stats.rejected_scene=true;log!("fast GI: {error}");}
    fn begin_submission(&mut self,key:(u64,u64,u64))->u64 {
        if let Some(job)=self.placement.take(){job.cancel();}self.placement_generation=self.placement_generation.wrapping_add(1);
        self.generation=self.generation.wrapping_add(1);
        // A preparation that just completed can already be superseded by
        // this frame's world revision. Never let ensure() adopt that scene.
        self.revision=Some(key);self.gpu=None;self.previous_display=None;self.pending=None;self.origin_key=None;self.reset_progress();self.stats.building=true;
        self.stats.waiting_for_worker=false;
        self.generation
    }
    pub fn submit(&mut self,key:(u64,u64,u64),slot:makepad_platform::thread::PoolSlot,scene:Vec<Instance>){
        let generation=self.begin_submission(key);
        self.job=Some(slot.submit(move ||(generation,Prepared::build(scene).map(Arc::new))));
    }
    pub fn poll(&mut self){
        if let Some(result)=self.job.as_mut().and_then(|j|j.try_take()) {
            self.job=None;self.stats.building=false;
            match result {
                Ok((generation,Ok(p))) if generation==self.generation=>{self.stats.triangles=p.count;self.stats.preparation_ms=p.ms;self.stats.rejected_scene=false;self.pending=Some(p);},
                Ok((_,Err(e)))=>{log!("fast GI: {e}");self.stats.rejected_scene=true;},
                Err(e)=>{log!("fast GI worker failed: {e:?}");self.revision=None;},_=>{}
            }
        }
    }
    fn ensure(&mut self,cx:&mut Cx)->bool{
        if !cx.gpu_info().float_color_targets {if !self.stats.rejected_scene{log!("fast GI unavailable: float render targets unsupported");}self.stats.rejected_scene=true;self.pending=None;return false;}
        if self.gpu.is_some(){return true;}
        if self.pending.is_none(){return false;}
        let Some((trace,relight,gather))=cx.try_with_vm(|vm|(
            DrawGiTrace::script_new_with_default(vm),DrawGiRelight::script_new_with_default(vm),DrawGiGather::script_new_with_default(vm)
        ))else{return false;};
        let p=match Arc::try_unwrap(self.pending.take().unwrap()){Ok(p)=>p,Err(p)=>{self.pending=Some(p);return false;}};
        let nodes_height=p.nodes.len()/DATA_WIDTH/4;let tri_height=p.triangles.len()/DATA_WIDTH/4;
        self.stats.uploads_bytes=(p.nodes.len()+p.triangles.len())*4;
        let data=|cx:&mut Cx,data:Vec<f32>|Texture::new_with_format(cx,TextureFormat::VecRGBAf32{width:DATA_WIDTH,height:data.len()/DATA_WIDTH/4,data:Some(data),updated:TextureUpdated::Full});
        let target=|cx:&mut Cx,w|Texture::new_with_format(cx,TextureFormat::RenderRGBAf32{size:TextureSize::Fixed{width:w,height:if w==PROBE_TEXELS{self.config.probe_count().max(MAX_BLOCKERS)*2}else{self.config.probe_count()}},initial:true});
        // Explicit dependencies below order trace -> relight -> gather -> scene.
        let mut stages:Vec<_>=["GI gather","GI relight","GI trace"].into_iter().map(|name|{let pass=DrawPass::new_with_name(cx,name);pass.set_gpu_timing_enabled(cx,true);Stage{pass,list:DrawList::new(cx)}}).collect();stages.reverse();
        let mover_data=padded(vec![0.0;(MAX_BLOCKERS*6+self.config.probe_count())*4]);
        self.stats.resident_bytes=self.stats.uploads_bytes+((RAYS*2+1)*self.config.probe_count()+PROBE_TEXELS*4*self.config.probe_count().max(MAX_BLOCKERS))*16+mover_data.len()*4+256;
        self.stats.preparation_cpu_bytes=p.bvh.tris.len()*std::mem::size_of::<Tri>()+p.bvh.nodes.len()*std::mem::size_of::<makepad_raytrace::bvh::FlatNode>()+p.bvh.tri_order.len()*4+p.bvh.priorities.len()*2+p.bvh.coplanar_groups.len()*4+p.static_boxes.len()*std::mem::size_of::<Mover>()+p.emitters.len()*std::mem::size_of::<emitters::Emitter>();
        self.gpu=Some(Gpu{stages,trace,relight,gather,nodes:data(cx,p.nodes),triangles:data(cx,p.triangles),hits:Texture::new(cx),radiance:Texture::new(cx),probes:target(cx,PROBE_TEXELS),history:target(cx,PROBE_TEXELS),movers:data(cx,mover_data),sun_rows:Texture::new_with_format(cx,TextureFormat::VecRGBAf32{width:16,height:1,data:Some(vec![0.0;64]),updated:TextureUpdated::Full}),nodes_height,tri_height,node_count:p.node_count,positions:Texture::new(cx),bvh:p.bvh,placed:false,static_boxes:p.static_boxes,static_visibility:placement::StaticVisibility::default(),field_blockers:0,emitters:p.emitters,emitter_count:0,ray_width:RAYS,position_width:1});
        true
    }
    pub fn relight_pass(&self)->Option<DrawPassId>{self.gpu.as_ref().map(|g|g.stages[1].pass.draw_pass_id())}
    pub fn run(&mut self,cx:&mut CxDraw,center:Vec3f,movers:&[Mover],sun:&crate::sun::SunLight,
        clustered:&crate::clustered::ClusteredLights,csm:Option<(crate::shadow_csm::CsmFrame,Texture,f32)>) {
        if self.mode==GiMode::Off{return;}
        let center=self.config.anchor.unwrap_or(center);
        let start=Cx::monotonic_now();self.stats.traced_rays=0;self.stats.relit_rays=0;self.stats.uploads_bytes=0;
        if !finite(center)||!self.ensure(cx.cx){return;}
        let c=self.config;let count=c.probe_count();
        let key=[(center.x/(c.spacing*2.0)).round()as i32,(center.y/(c.spacing*2.0)).round()as i32,(center.z/(c.spacing*2.0)).round()as i32];
        // Finish one replacement before chasing another camera cell. This
        // bounds work while walking and avoids continually cancelling warmup.
        if self.origin_key.is_none() || (self.origin_key!=Some(key) && self.stats.display_blend>=1.0) {
            if self.stats.display_blend>=1.0 {
                let gpu=self.gpu.as_ref().unwrap();
                self.previous_display=Some(DisplayField{origin:self.origin,config:c,blockers:gpu.field_blockers,bank:0.0});
            }
            if std::env::var_os("MAKEPAD_GI_STATS").is_some() {
                log!("fast GI scroll: {:?} -> {:?}, retained lighting={}",self.origin_key,key,self.previous_display.is_some());
            }
            if let Some(job)=self.placement.take(){job.cancel();}self.placement_generation=self.placement_generation.wrapping_add(1);
            // Non-integer offsets avoid an entire plane of probes lying
            // exactly on common integer-aligned floors and wall faces.
            self.origin_key=Some(key);self.origin=vec3f(key[0]as f32*2.0-(c.grid[0]-1)as f32*0.5+0.173,key[1]as f32*2.0-(c.grid[1]-1)as f32*0.5+0.237,key[2]as f32*2.0-(c.grid[2]-1)as f32*0.5+0.319)*c.spacing;
            self.reset_progress();
            let gpu=self.gpu.as_mut().unwrap();
            gpu.placed=false;gpu.field_blockers=0;
            // Preserve bank zero until gather copies it to the retained bank.
            // Untraced rows in the replacement are cleared by gi_on=0 in its
            // first gather, so stale probes never enter transport feedback.
        }
        self.stats.probes=count;
        if !self.gpu.as_ref().unwrap().placed {
            if let Some(result)=self.placement.as_mut().and_then(|j|j.try_take()) {
                self.placement=None;
                match result {
                    Ok((generation,p)) if generation==self.placement_generation=>{
                        if let Ok(p)=Arc::try_unwrap(p) {
                            let PreparedPlacement{placement:p,samples}=p;
                            self.stats.placement_ms=p.ms;self.stats.relocated_probes=p.moved;
                            self.stats.inactive_probes=p.inactive;self.stats.placement_exhausted=p.exhausted;
                            self.stats.uploads_bytes+=samples.data.len()*4;
                            let gpu=self.gpu.as_mut().unwrap();
                            let ray_width=RAYS+samples.count*emitters::SAMPLES_PER_EMITTER;
                            self.stats.resident_bytes=self.stats.resident_bytes-(2*gpu.ray_width+gpu.position_width)*count*16+(2*ray_width+samples.width)*count*16;
                            gpu.ray_width=ray_width;gpu.position_width=samples.width;gpu.emitter_count=samples.count;
                            self.stats.selected_emitters=samples.count;self.stats.emitter_fallbacks=samples.fallbacks;self.stats.emitter_probe_fallbacks=samples.probe_fallbacks;
                            gpu.positions=Texture::new_with_format(cx.cx,TextureFormat::VecRGBAf32{width:samples.width,height:count,data:Some(samples.data),updated:TextureUpdated::Full});
                            for tex in [&mut gpu.hits,&mut gpu.radiance] {
                                *tex=Texture::new_with_format(cx.cx,TextureFormat::RenderRGBAf32{size:TextureSize::Fixed{width:ray_width,height:count},initial:true});
                            }
                            self.stats.static_blockers=p.static_visibility.boxes.len();
                            self.stats.static_blocker_fallbacks=p.static_visibility.fallbacks;
                            gpu.static_visibility=p.static_visibility;
                            gpu.placed=true;self.stats.building=false;
                        }
                    }
                    Err(e)=>{log!("GI placement worker failed: {e:?}");}
                    _=>{}
                }
            }
            if !self.gpu.as_ref().unwrap().placed {
                self.stats.building=true;
                if self.placement.is_none() {
                    if let Ok(slot)=cx.task_pool().reserve(makepad_platform::thread::Lane::Heavy) {
                        let gpu=self.gpu.as_ref().unwrap();let bvh=gpu.bvh.clone();let boxes=gpu.static_boxes.clone();let emitters=gpu.emitters.clone();let origin=self.origin;let generation=self.placement_generation;
                        self.placement=Some(slot.submit(move ||{
                            let start=Cx::monotonic_now();
                            let mut placement=placement::Placement::build(&bvh,origin,c,&boxes);
                            let samples=emitters::Samples::build(&emitters,&placement.positions,origin,c);
                            placement.ms=(Cx::monotonic_now()-start)*1e3;
                            (generation,Arc::new(PreparedPlacement{placement,samples}))
                        }));
                        self.stats.waiting_for_worker=false;
                    }else{self.stats.waiting_for_worker=true;}
                }
                return;
            }
        }
        // Never silently discard a moving blocker in range. Report and leave
        // the view on ordinary lighting until it fits the configured mode.
        self.stats.omitted_movers=movers.len().saturating_sub(MAX_MOVERS);
        self.stats.rejected_scene=self.stats.omitted_movers>0;
        if self.stats.rejected_scene{return;}
        self.stats.mover_count=movers.len();
        let keep_previous=self.previous_display.is_some();
        let copy_previous=self.previous_display.as_ref().is_some_and(|p|p.bank==0.0);
        let gpu=self.gpu.as_mut().unwrap();
        let previous_blockers=gpu.field_blockers;
        let current_blockers=movers.len()+gpu.static_visibility.boxes.len();
        // Relight/gather read last frame's field; gather writes a distinct
        // target. Copy untouched rows in that same pass so sparse updates
        // cannot alternate stale sweeps or read a currently bound target.
        std::mem::swap(&mut gpu.probes,&mut gpu.history);
        let mut data=gpu.movers.take_vec_f32(cx.cx);data.fill(0.0);
        // Only the first movers.len() records participate in relight rays.
        // Static boxes are exact receiver visibility, not repeated tracing.
        for (i,m) in movers.iter().chain(gpu.static_visibility.boxes.iter()).enumerate() {
            data[i*24..i*24+24].copy_from_slice(&mover_record(m));
        }
        let mut bounds=[(Vec3f::default(),Vec3f::default());MAX_MOVERS];
        for (dst,mover) in bounds.iter_mut().zip(movers){*dst=mover_bounds(mover);}
        self.stats.blocker_cells=0;self.stats.blocker_overflow_cells=0;
        for z in 0..c.grid[2] {for y in 0..c.grid[1] {for x in 0..c.grid[0] {
            let index=x+c.grid[0]*(y+c.grid[1]*z);
            let cell=self.origin+vec3f(x as f32,y as f32,z as f32)*c.spacing;
            let at=(MAX_BLOCKERS*6+index)*4;
            let ids=merge_blockers(cell_blockers(cell,c.spacing,&bounds[..movers.len()]),gpu.static_visibility.cells[index],movers.len());
            self.stats.blocker_cells+=usize::from(ids[0]!=-1.0);
            self.stats.blocker_overflow_cells+=usize::from(ids[0]==-2.0);
            data[at..at+4].copy_from_slice(&ids);
        }}}
        let mover_height=data.len()/DATA_WIDTH/4;
        self.stats.uploads_bytes+=data.len()*4;
        gpu.movers.put_back_vec_f32(cx.cx,data,None);
        let mut rows=gpu.sun_rows.take_vec_f32(cx.cx);rows.fill(0.0);
        if let Some((frame,_,_))=&csm {
            for (i,c) in frame.cascades.iter().enumerate(){for (j,r) in [c.rx,c.ry,c.rz,vec4(c.bias01,c.texel_world,0.0,0.0)].into_iter().enumerate(){rows[i*16+j*4..i*16+j*4+4].copy_from_slice(&[r.x,r.y,r.z,r.w]);}}
        }
        gpu.sun_rows.put_back_vec_f32(cx.cx,rows,None);self.stats.uploads_bytes+=256;
        let light_ids=clustered.gi_light_ids(center);self.stats.selected_lights=light_ids.iter().filter(|&&i|i>=0.0).count();
        let tracing=self.traced<count;
        // More complex trees may need the BVH's full traversal bound. Keep
        // the old WORST-CASE node-visit budget by reducing the startup batch,
        // not by multiplying a frame's possible tracing cost eightfold.
        let relight_probes=emitter_probe_budget(c.probes_per_frame,gpu.ray_width);
        let (trace_probes,trace_limit)=trace_budget(relight_probes,gpu.node_count);
        self.stats.trace_node_limit=trace_limit;
        let update_budget=if tracing{trace_probes}else{relight_probes};
        let batch=update_budget.min(count-self.cursor);
        let mut cx2=Cx2d::new(cx);
        for i in 0..3 {
            for ms in gpu.stages[i].pass.take_gpu_times_ms(cx2.cx){self.stats.gpu_ms[i]=ms;self.stats.gpu_peak_ms[i]=self.stats.gpu_peak_ms[i].max(ms);}
            if i==0&&!tracing {self.stats.gpu_ms[i]=0.0;continue;}
            let quad=match i{0=>&mut gpu.trace.quad,1=>&mut gpu.relight.quad,_=>&mut gpu.gather.quad};
            let dv=&mut quad.draw_vars;
            dv.set_uniform(cx2.cx,live_id!(gi_transition),&[1.0]);
            dv.set_uniform(cx2.cx,live_id!(gi_keep_previous),&[if keep_previous{1.0}else{0.0}]);
            dv.set_uniform(cx2.cx,live_id!(gi_copy_previous),&[if copy_previous{1.0}else{0.0}]);
            dv.set_uniform(cx2.cx,live_id!(gi_origin),&[self.origin.x,self.origin.y,self.origin.z,c.spacing]);
            dv.set_uniform(cx2.cx,live_id!(gi_grid),&[c.grid[0]as f32,c.grid[1]as f32,c.grid[2]as f32,count as f32]);
            dv.set_uniform(cx2.cx,live_id!(gi_batch),&[self.cursor as f32,batch as f32,c.ray_distance,trace_limit as f32]);
            dv.set_uniform(cx2.cx,live_id!(gi_emitters),&[gpu.emitter_count as f32,gpu.position_width as f32,gpu.ray_width as f32,0.0]);
            dv.set_uniform(cx2.cx,live_id!(gi_scene),&[gpu.nodes_height as f32,gpu.tri_height as f32,gpu.node_count as f32,0.0]);
            dv.set_uniform(cx2.cx,live_id!(gi_mover_height),&[mover_height as f32]);
            dv.set_uniform(cx2.cx,live_id!(gi_blocker_count),&[producer_blocker_count(i,previous_blockers,current_blockers) as f32]);
            bind_texture(cx2.cx,dv,live_id!(gi_field),&gpu.history);
            bind_texture(cx2.cx,dv,live_id!(gi_positions),&gpu.positions);
            // Never apply the presentation fade here: feedback must converge
            // while startup GI is still hidden from the scene shaders.
            dv.set_uniform(cx2.cx,live_id!(gi_on),&[if self.traced>0{1.0}else{0.0}]);
            for (name,t) in [(live_id!(gi_nodes),&gpu.nodes),(live_id!(gi_triangles),&gpu.triangles),(live_id!(gi_hits),&gpu.hits),(live_id!(gi_radiance),&gpu.radiance),(live_id!(gi_movers),&gpu.movers),(live_id!(gi_sun_rows),&gpu.sun_rows)]{bind_texture(cx2.cx,dv,name,t);}
            if i==1 {
                clustered.bind(cx2.cx,dv,true);
                bind_texture(cx2.cx,dv,live_id!(gi_sun_map),csm.as_ref().map(|(_,t,_)|t).unwrap_or(&gpu.nodes));
                for (name,v) in [(live_id!(gi_sun_dir),sun.dir),(live_id!(gi_sun_color),sun.color),(live_id!(gi_sky),sun.sky),(live_id!(gi_ground),sun.ground)]{dv.set_uniform(cx2.cx,name,&[v.x,v.y,v.z]);}
                dv.set_uniform(cx2.cx,live_id!(gi_counts),&[movers.len()as f32,0.0,if csm.is_some(){3.0}else{0.0},c.feedback]);
                dv.set_uniform(cx2.cx,live_id!(gi_light_ids0),&light_ids[..4]);dv.set_uniform(cx2.cx,live_id!(gi_light_ids1),&light_ids[4..]);
            }
            let width=if i==2{PROBE_TEXELS}else{gpu.ray_width};
            let height=if i==2{count.max(MAX_BLOCKERS)*2}else{count};
            let target=match i{0=>&gpu.hits,1=>&gpu.radiance,_=>&gpu.probes};
            let next=if i<2{Some(gpu.stages[i+1].pass.draw_pass_id())}else{None};
            let stage=&mut gpu.stages[i];
            cx2.make_child_pass(&stage.pass);
            if let Some(next)=next{cx2.passes[stage.pass.draw_pass_id()].parent=CxDrawPassParent::DrawPass(next);}
            cx2.begin_pass(&stage.pass,Some(1.0));
            stage.pass.set_size(cx2.cx,dvec2(width as f64,height as f64));stage.pass.clear_color_textures(cx2.cx);
            stage.pass.set_color_texture(cx2.cx,target,DrawPassClearColor::InitWith(vec4(0.0,0.0,0.0,0.0)));
            stage.list.begin_always(&mut cx2);
            cx2.begin_root_turtle(dvec2(width as f64,height as f64),Layout::flow_overlay());
            quad.draw_abs(&mut cx2,Rect{pos:dvec2(0.0,if i==2{0.0}else{self.cursor as f64}),size:dvec2(width as f64,if i==2{height as f64}else{batch as f64})});
            cx2.end_pass_sized_turtle();stage.list.end(&mut cx2);cx2.end_pass(&stage.pass);
        }
        gpu.field_blockers=current_blockers;
        self.stats.traced_rays=if tracing{batch*gpu.ray_width}else{0};self.stats.relit_rays=batch*gpu.ray_width;
        if tracing{self.traced=(self.traced+batch).min(count);}
        self.stats.ready_probes=self.traced;self.cursor=(self.cursor+batch)%count;
        self.stats.display_blend=self.startup.advance(self.cursor==0,Cx::monotonic_now());
        self.stats.startup_sweeps=self.startup.sweeps;
        if let Some(previous)=self.previous_display.as_mut(){previous.bank=1.0;}
        if self.stats.display_blend>=1.0 {self.previous_display=None;}
        self.stats.encode_us=((Cx::monotonic_now()-start)*1e6)as u64;
    }
    fn display_strength(&self)->f32 {
        // Explicit diagnostics can inspect partially built probes immediately.
        self.config.strength*if self.debug==GiDebug::Off{self.stats.display_blend}else{1.0}
    }
    pub fn bind(&self,cx:&Cx,dv:&mut DrawVars){
        let previous=self.previous_display.as_ref().filter(|_|self.debug==GiDebug::Off);
        let on=self.mode==GiMode::Fast&&self.gpu.is_some()&&(self.stats.ready_probes>0||previous.is_some())&&!self.stats.rejected_scene;
        dv.set_uniform(cx,live_id!(gi_on),&[if on{if previous.is_some(){self.config.strength}else{self.display_strength()}}else{0.0}]);
        dv.set_uniform(cx,live_id!(gi_transition),&[if previous.is_some(){self.stats.display_blend}else{1.0}]);
        if let Some(previous)=previous {
            dv.set_uniform(cx,live_id!(gi_previous_bank),&[previous.bank]);
            let p=previous.origin;let c=previous.config;
            dv.set_uniform(cx,live_id!(gi_previous_origin),&[p.x,p.y,p.z,c.spacing]);
            dv.set_uniform(cx,live_id!(gi_previous_grid),&[c.grid[0]as f32,c.grid[1]as f32,c.grid[2]as f32,c.probe_count()as f32]);
            dv.set_uniform(cx,live_id!(gi_previous_blockers),&[previous.blockers as f32]);
        }
        dv.set_uniform(cx,live_id!(gi_debug),&[self.debug as u8 as f32]);
        dv.set_uniform(cx,live_id!(gi_blocker_count),&[if on{self.gpu.as_ref().unwrap().field_blockers as f32}else{0.0}]);
        if !on {
            if let Some(id)=dv.draw_shader_id {if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==live_id!(gi_field)){dv.empty_texture(slot);}}
            return;
        }
        let gpu=self.gpu.as_ref().unwrap();bind_texture(cx,dv,live_id!(gi_field),&gpu.probes);
        dv.set_uniform(cx,live_id!(gi_origin),&[self.origin.x,self.origin.y,self.origin.z,self.config.spacing]);
        let c=self.config;dv.set_uniform(cx,live_id!(gi_grid),&[c.grid[0]as f32,c.grid[1]as f32,c.grid[2]as f32,c.probe_count()as f32]);
    }
}
pub(crate) fn bind_texture(cx:&Cx,dv:&mut DrawVars,name:LiveId,t:&Texture){if let Some(id)=dv.draw_shader_id{if let Some(slot)=cx.draw_shaders[id.index].mapping.textures.iter().position(|t|t.id==name){dv.set_texture(slot,t);}}}

#[cfg(test)]
#[path="fast_gi/tests.rs"] mod tests;
