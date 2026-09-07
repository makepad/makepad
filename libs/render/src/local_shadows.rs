//! Bounded realtime local shadow atlas. A spot occupies one tile; a point
//! occupies six. A shadow-requesting light that cannot fit is omitted, never
//! silently downgraded to illuminating through walls.
use crate::{
    gpu_lightmap::{GpuBakeMesh, GpuLmMover},
    lightmap::LmLight,
    shaders::*,
};
use makepad_draw::*;

const COLUMNS: usize = 4;
const MAX_FACES: usize = 16;

/// Shader layout and atlas format must agree for the lifetime of the app.
pub(crate) fn hardware_shadow_maps() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MAKEPAD_LOCAL_SHADOWS").as_deref() != Ok("legacy")
            && std::env::var("MAKEPAD_CLUSTERED").as_deref() != Ok("off")
            && !std::env::var("MAKEPAD").unwrap_or_default().split(',').any(|v| v == "headless")
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalShadowConfig {
    pub max_faces: usize,
    pub resolution: usize,
}
impl Default for LocalShadowConfig {
    fn default() -> Self {
        Self {
            max_faces: 8,
            resolution: 512,
        }
    }
}
impl LocalShadowConfig {
    pub fn clamped(self) -> Self {
        Self {
            max_faces: self.max_faces.min(MAX_FACES),
            resolution: self.resolution.clamp(128, 1024).next_power_of_two(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LocalShadowStats {
    pub lights: usize,
    pub faces: usize,
    pub omitted_lights: usize,
    pub caster_draws: usize,
    pub encode_us: u64,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ShadowRecord {
    pub first: usize,
    /// 0=no request, -1=omitted, 1=spot, 6=point.
    pub count: i32,
    pub near: f32,
}

#[derive(Clone, Copy)]
struct Face {
    light: usize,
    rx: Vec4f,
    ry: Vec4f,
    rz: Vec4f,
    tile: Vec4f,
}

fn dot(r: Vec4f, p: Vec3f) -> f32 {
    r.x * p.x + r.y * p.y + r.z * p.z + r.w
}
fn camera_rows(pos: Vec3f, dir: Vec3f, focal: f32) -> (Vec4f, Vec4f, Vec4f) {
    let forward = dir.normalize();
    let up = if forward.y.abs() > 0.95 {
        vec3f(0.0, 0.0, 1.0)
    } else {
        vec3f(0.0, 1.0, 0.0)
    };
    let right = Vec3f::cross(forward, up).normalize();
    let up = Vec3f::cross(right, forward).normalize();
    let row = |axis: Vec3f| vec4(axis.x, axis.y, axis.z, -axis.dot(pos));
    (row(right * focal), row(up * focal), row(forward))
}

fn in_face(face: &Face, near: f32, far: f32, min: Vec3f, max: Vec3f) -> bool {
    let center = (min + max) * 0.5;
    let half = (max - min) * 0.5;
    let extent = |r: Vec4f| r.x.abs() * half.x + r.y.abs() * half.y + r.z.abs() * half.z;
    let z = dot(face.rz, center);
    let ze = extent(face.rz);
    if z + ze < near || z - ze > far {
        return false;
    }
    for r in [face.rx, face.ry] {
        for sign in [-1.0, 1.0] {
            let plane = vec4(
                face.rz.x + sign * r.x,
                face.rz.y + sign * r.y,
                face.rz.z + sign * r.z,
                face.rz.w + sign * r.w,
            );
            if dot(plane, center) + extent(plane) < 0.0 {
                return false;
            }
        }
    }
    true
}

pub(crate) struct LocalShadows {
    pub soft_filter: bool,
    /// Apparent emitter radius in world units for bounded contact-hardening.
    pub source_radius: f32,
    config: LocalShadowConfig,
    pub records: Vec<ShadowRecord>,
    pub data: Vec<f32>,
    faces: Vec<Face>,
    ranked: Vec<(f32, usize)>,
    pub texture: Option<Texture>,
    metadata: Option<Texture>,
    metadata_height: usize,
    depth: Option<Texture>,
    allocation_size: (usize, usize),
    pass: Option<DrawPass>,
    list: Option<DrawList>,
    rigid: Option<DrawLmLampDepth>,
    skinned: Option<DrawLocalShadowSkinned>,
    pub stats: LocalShadowStats,
}
impl Default for LocalShadows {
    fn default() -> Self {
        Self {
            config: LocalShadowConfig::default(),
            soft_filter: false,
            source_radius: 0.4,
            records: Vec::new(),
            data: Vec::new(),
            faces: Vec::new(),
            ranked: Vec::new(),
            texture: None,
            metadata: None,
            metadata_height: 0,
            depth: None,
            allocation_size: (0, 0),
            pass: None,
            list: None,
            rigid: None,
            skinned: None,
            stats: LocalShadowStats::default(),
        }
    }
}
impl LocalShadows {
    pub(crate) fn parent_to(&self,cx:&mut Cx,parent:DrawPassId){if let Some(pass)=&self.pass{cx.passes[pass.draw_pass_id()].parent=CxDrawPassParent::DrawPass(parent);}}
    pub fn config(&self) -> LocalShadowConfig {
        self.config
    }
    pub fn set_config(&mut self, config: LocalShadowConfig) {
        let config = config.clamped();
        if self.config != config {
            self.texture = None;
            self.depth = None;
            self.config = config;
        }
    }
    pub fn size(&self) -> (usize, usize) {
        (
            self.columns() * self.config.resolution,
            self.config.max_faces.max(1).div_ceil(COLUMNS) * self.config.resolution,
        )
    }
    fn columns(&self)->usize {self.config.max_faces.clamp(1,COLUMNS)}
    fn upload_metadata(&mut self, cx: &mut Cx) {
        let height = (self.records.len() + self.data.len() / 4)
            .max(1)
            .div_ceil(256)
            .next_power_of_two();
        let mut packed = self
            .metadata
            .as_ref()
            .map(|t| t.take_vec_f32(cx))
            .unwrap_or_default();
        packed.resize(256 * height * 4, 0.0);
        packed.fill(0.0);
        for (i, r) in self.records.iter().enumerate() {
            let texel_world_per_depth = if r.count > 0 {
                let row = self.faces[r.first].rx;
                2.0 / ((self.config.resolution - 2) as f32
                    * (row.x * row.x + row.y * row.y + row.z * row.z).sqrt())
            } else {
                0.0
            };
            packed[i * 4..i * 4 + 4].copy_from_slice(&[
                r.count as f32,
                r.near,
                (self.records.len() + r.first * 4) as f32,
                texel_world_per_depth,
            ]);
        }
        let start = self.records.len() * 4;
        packed[start..start + self.data.len()].copy_from_slice(&self.data);
        if self.metadata.is_none() || height != self.metadata_height {
            self.metadata = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width: 256,
                    height,
                    data: Some(packed),
                    updated: TextureUpdated::Full,
                },
            ));
            self.metadata_height = height;
        } else {
            self.metadata
                .as_ref()
                .unwrap()
                .put_back_vec_f32(cx, packed, None);
        }
    }
    pub fn bind(&self, cx: &Cx, vars: &mut DrawVars, fallback: &Texture) {
        let (scale, bias) = cx.clip_depth_scale_bias();
        vars.set_uniform(cx, live_id!(local_shadow_depth_range), &[scale, bias]);
        vars.set_uniform(cx,live_id!(local_shadow_soft),&[if self.soft_filter{1.0}else{0.0}]);
        vars.set_uniform(cx,live_id!(local_shadow_source_radius),&[self.source_radius]);
        let requested = self.records.iter().any(|r| r.count != 0);
        let (width, height) = self.size();
        vars.set_uniform(
            cx,
            live_id!(local_shadow_tex),
            &[
                1.0 / 256.0,
                1.0 / self.metadata_height.max(1) as f32,
                1.0 / width as f32,
                1.0 / height as f32,
            ],
        );
        vars.set_uniform(
            cx,
            live_id!(local_shadow_on),
            &[if requested { 1.0 } else { 0.0 }],
        );
        if let Some(id) = vars.draw_shader_id {
            for (name, texture) in [
                (
                    live_id!(local_shadow_data),
                    self.metadata.as_ref().unwrap_or(fallback),
                ),
                (
                    live_id!(local_shadow_map),
                    if hardware_shadow_maps() { self.depth.as_ref().unwrap_or(fallback) }
                    else { self.texture.as_ref().unwrap_or(fallback) },
                ),
            ] {
                if let Some(slot) = cx.draw_shaders[id.index]
                    .mapping
                    .textures
                    .iter()
                    .position(|t| t.id == name)
                {
                    vars.set_texture(slot, texture);
                }
            }
        }
    }
    fn prepare(&mut self, lights: &[LmLight], active: &[bool], eye: Vec3f) {
        self.stats = LocalShadowStats::default();
        self.records.clear();
        self.records.resize(lights.len(), ShadowRecord::default());
        self.faces.clear();
        self.data.clear();
        self.ranked.clear();
        for (i, l) in lights.iter().enumerate() {
            if !l.shadows {
                continue;
            }
            self.records[i].count = -1;
            if !active.get(i).copied().unwrap_or(false) {
                continue;
            }
            let delta = l.pos - eye;
            let score = l.color.x.max(l.color.y).max(l.color.z) * l.radius * l.radius
                / (delta.dot(delta) + l.radius * l.radius);
            self.ranked.push((score, i));
        }
        self.ranked
            .sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let (width, height) = self.size();
        for &(_, i) in &self.ranked {
            let l = &lights[i];
            let count = if l.cone.is_some_and(|(_, outer)| outer < 89.0) {
                1
            } else {
                6
            };
            if self.faces.len() + count > self.config.max_faces {
                self.stats.omitted_lights += 1;
                continue;
            }
            let near = (l.radius * 0.001).clamp(0.001, 0.03);
            self.records[i] = ShadowRecord {
                first: self.faces.len(),
                count: count as i32,
                near,
            };
            let focal = if count == 1 {
                1.0 / l.cone.unwrap().1.to_radians().tan()
            } else {
                1.0
            };
            for f in 0..count {
                let dir = if count == 1 {
                    l.dir
                } else {
                    match f {
                        0 => vec3f(1.0, 0.0, 0.0),
                        1 => vec3f(-1.0, 0.0, 0.0),
                        2 => vec3f(0.0, 1.0, 0.0),
                        3 => vec3f(0.0, -1.0, 0.0),
                        4 => vec3f(0.0, 0.0, 1.0),
                        _ => vec3f(0.0, 0.0, -1.0),
                    }
                };
                let (rx, ry, rz) = camera_rows(l.pos, dir, focal);
                let slot = self.faces.len();
                let r = self.config.resolution;
                let uv = vec4(
                    (slot % self.columns() * r + 1) as f32 / width as f32,
                    (slot / self.columns() * r + 1) as f32 / height as f32,
                    (r - 2) as f32 / width as f32,
                    (r - 2) as f32 / height as f32,
                );
                let tile = vec4(uv.z, uv.w, uv.x * 2.0 + uv.z - 1.0, 1.0 - uv.y * 2.0 - uv.w);
                self.faces.push(Face {
                    light: i,
                    rx,
                    ry,
                    rz,
                    tile,
                });
                for row in [rx, ry, rz, uv] {
                    self.data.extend_from_slice(&[row.x, row.y, row.z, row.w]);
                }
            }
            self.stats.lights += 1;
        }
        self.stats.faces = self.faces.len();
    }

    pub fn render(
        &mut self,
        cx: &mut CxDraw,
        lights: &[LmLight],
        active: &[bool],
        eye: Vec3f,
        statics: &[GpuBakeMesh],
        movers: &[GpuLmMover],
    ) {
        let start = Cx::monotonic_now();
        self.prepare(lights, active, eye);
        if self.faces.is_empty() && (!hardware_shadow_maps() || self.depth.is_some()) {
            self.upload_metadata(cx.cx);
            return;
        }
        if self.rigid.is_none() {
            let Some((rigid, skinned)) = cx.cx.try_with_vm(|vm| {
                (
                    DrawLmLampDepth::script_new_with_default(vm),
                    DrawLocalShadowSkinned::script_new_with_default(vm),
                )
            }) else {
                self.stats.omitted_lights += self.stats.lights;
                self.stats.lights = 0;
                self.stats.faces = 0;
                for record in &mut self.records {
                    if record.count > 0 {
                        record.count = -1;
                    }
                }
                self.upload_metadata(cx.cx);
                return;
            };
            self.rigid = Some(rigid);
            self.skinned = Some(skinned);
        }
        // An initialized one-texel depth binding is still required when the
        // shader has no active shadow lights. Do not allocate a full atlas.
        let (width, height) = if self.faces.is_empty() { (1, 1) } else { self.size() };
        if self.texture.is_none() || self.allocation_size != (width, height) {
            self.allocation_size = (width, height);
            self.texture = Some(Texture::new_with_format(
                cx.cx,
                TextureFormat::RenderRf32 {
                    size: TextureSize::Fixed { width, height },
                    initial: true,
                },
            ));
            self.depth = Some(Texture::new_with_format(
                cx.cx,
                if hardware_shadow_maps() { TextureFormat::DepthD32Sampled {
                    size: TextureSize::Fixed { width, height },
                    initial: true,
                }} else { TextureFormat::DepthD32 {
                    size: TextureSize::Fixed { width, height },
                    initial: true,
                }},
            ));
        }
        let pass = self.pass.get_or_insert_with(|| DrawPass::new(cx.cx));
        let list = self.list.get_or_insert_with(|| DrawList::new(cx.cx));
        cx.make_child_pass(pass);
        cx.begin_pass(pass, Some(1.0));
        pass.set_size(cx.cx, dvec2(width as f64, height as f64));
        pass.clear_color_textures(cx.cx);
        pass.set_color_texture(
            cx.cx,
            self.texture.as_ref().unwrap(),
            DrawPassClearColor::ClearWith(vec4(1.0, 1.0, 1.0, 1.0)),
        );
        pass.set_depth_texture(
            cx.cx,
            self.depth.as_ref().unwrap(),
            DrawPassClearDepth::ClearWith(1.0),
        );
        list.begin_always(cx);
        let rigid = self.rigid.as_mut().unwrap();
        let skinned = self.skinned.as_mut().unwrap();
        for face in &self.faces {
            let record = self.records[face.light];
            let far = lights[face.light].radius;
            rigid.face_rx = face.rx;
            rigid.face_ry = face.ry;
            rigid.face_rz = face.rz;
            rigid.tile_a = face.tile;
            rigid.lamp_range = vec4(record.near, far, 0.0, 0.0);
            rigid.set_morph(cx.cx,None);
            for m in statics {
                if !in_face(face, record.near, far, m.min, m.max) {
                    continue;
                }
                rigid.transform = m.transform;
                rigid.draw_vars.geometry_id = Some(m.geometry);
                if rigid.draw_vars.can_instance() {
                    cx.add_instance(&rigid.draw_vars);
                    self.stats.caster_draws += 1;
                }
            }
            for m in movers {
                let (min, max) = crate::lightmap::world_bounds(&m.transform, (m.min, m.max));
                // Skin bounds can change under animation; never cull a posed
                // character by its rest AABB. The GPU performs its clipping.
                if m.skin.is_none() && m.morph.is_none() && !in_face(face, record.near, far, min, max) {
                    continue;
                }
                if let Some(skin) = &m.skin {
                    skinned.base.set_morph(cx.cx,m.morph.as_ref());
                    skinned.base.sun_rx = face.rx;
                    skinned.base.sun_ry = face.ry;
                    skinned.base.sun_rz = face.rz;
                    skinned.base.tile_a = face.tile;
                    skinned.base.transform = m.transform;
                    skinned.lamp_range = rigid.lamp_range;
                    skinned.base.skin_a.x = skin.joint_base;
                    skinned.base.draw_vars.set_texture(0, &skin.joint_tex);
                    skinned.base.draw_vars.geometry_id = Some(m.geometry);
                    if skinned.base.draw_vars.can_instance() {
                        cx.add_instance(&skinned.base.draw_vars);
                        self.stats.caster_draws += 1;
                    }
                } else {
                    rigid.set_morph(cx.cx,m.morph.as_ref());
                    rigid.transform = m.transform;
                    rigid.draw_vars.geometry_id = Some(m.geometry);
                    if rigid.draw_vars.can_instance() {
                        cx.add_instance(&rigid.draw_vars);
                        self.stats.caster_draws += 1;
                    }
                }
            }
        }
        list.end(cx);
        cx.end_pass(pass);
        self.upload_metadata(cx.cx);
        self.stats.encode_us = ((Cx::monotonic_now() - start) * 1e6) as u64;
    }
}

// Independent of clustered photometry: one header per uploaded light,
// then four camera rows per allocated face. No cluster stride changes.
pub(crate) mod sampling {
    use makepad_draw::*;
    script_mod! {
        use mod.prelude.widgets_internal.*
        mod.draw.LocalShadowSampling = {
            local_shadow_data: texture_2d(float)
            local_shadow_map: texture_2d(float)
            local_shadow_on: uniform(0.0)
            local_shadow_soft: uniform(0.0)
            local_shadow_source_radius: uniform(0.4)
            local_shadow_tex: uniform(vec4(0.00390625,1.0,0.00048828125,0.0009765625))
            local_shadow_fetch: fn(at: float) -> vec4 {
                let row=floor(at*self.local_shadow_tex.x)
                let col=at-row/self.local_shadow_tex.x
                return self.local_shadow_data.sample_nearest(vec2((col+0.5)*self.local_shadow_tex.x,(row+0.5)*self.local_shadow_tex.y))
            }
            local_shadow_compare: fn(uv:vec2,lo:vec2,hi:vec2,depth:float)->float {
                // Bilinear PCF at an arbitrary sub-texel position. Filtering
                // the comparisons keeps wide kernels continuous as lights
                // move; filtering depth itself would invent blockers.
                let texel=self.local_shadow_tex.zw
                let t=uv/texel-vec2(0.5,0.5)
                let f=fract(t)
                let base=(floor(t)+vec2(0.5,0.5))*texel
                let a=self.local_shadow_map.sample_nearest(clamp(base,lo,hi)).x
                let b=self.local_shadow_map.sample_nearest(clamp(base+vec2(texel.x,0.0),lo,hi)).x
                let c=self.local_shadow_map.sample_nearest(clamp(base+vec2(0.0,texel.y),lo,hi)).x
                let d=self.local_shadow_map.sample_nearest(clamp(base+texel,lo,hi)).x
                return mix(mix(step(depth,a),step(depth,b),f.x),mix(step(depth,c),step(depth,d),f.x),f.y)
            }
            local_shadow_blocker: fn(uv:vec2,lo:vec2,hi:vec2,z:float,near:float,range:float,bias:float)->vec2 {
                let texel=self.local_shadow_tex.zw
                let t=uv/texel-vec2(0.5,0.5)
                let f=fract(t)
                let base=(floor(t)+vec2(0.5,0.5))*texel
                var result=vec2(0.0,0.0)
                var y=0.0
                while y<2.0 {
                    let wy=mix(1.0-f.y,f.y,y)
                    var x=0.0
                    while x<2.0 {
                        let wx=mix(1.0-f.x,f.x,x)
                        let sample=self.local_shadow_map.sample_nearest(clamp(base+vec2(x,y)*texel,lo,hi)).x
                        let distance=near+sample*range
                        // Interpolate classified blockers, never raw depth
                        // across a silhouette. Nearest blocker estimates
                        // made penumbra width jump in visible horizontal bands.
                        let weight=wx*wy*smoothstep(bias,bias*1.5,z-distance)
                        result=result+vec2(distance*weight,weight)
                        x=x+1.0
                    }
                    y=y+1.0
                }
                return result
            }
            local_shadow_visibility: fn(index: float, wp: vec3, normal: vec3, light_pos: vec3, radius: float) -> float {
                if self.local_shadow_on<0.5 {return 1.0}
                let record=self.local_shadow_fetch(index)
                if record.x<0.0 {return 0.0}
                if record.x<0.5 {return 1.0}
                let delta=wp-light_pos
                // Choose a cubemap face AFTER normal bias: the offset can
                // cross a face boundary, particularly on horizontal ground.
                let receiver=wp+normal*max(0.01,length(delta)*record.w*1.5)
                let shadow_delta=receiver-light_pos
                var face=0.0
                if record.x>1.5 {
                    let a=abs(shadow_delta)
                    if a.x>=a.y && a.x>=a.z {
                        if shadow_delta.x<0.0 {face=1.0}
                    } else if a.y>=a.z {
                        face=2.0
                        if shadow_delta.y<0.0 {face=3.0}
                    } else {
                        face=4.0
                        if shadow_delta.z<0.0 {face=5.0}
                    }
                }
                let at=record.z+face*4.0
                let p=vec4(receiver.x,receiver.y,receiver.z,1.0)
                let z=dot(self.local_shadow_fetch(at+2.0),p)
                if z<=record.y {return 1.0}
                let q=vec2(dot(self.local_shadow_fetch(at),p)/z,dot(self.local_shadow_fetch(at+1.0),p)/z)
                if abs(q.x)>1.0 || abs(q.y)>1.0 {return 0.0}
                let tile=self.local_shadow_fetch(at+3.0)
                let uv=tile.xy+vec2(q.x*0.5+0.5,0.5-q.y*0.5)*tile.zw
                let texel=self.local_shadow_tex.zw
                let lo=tile.xy+texel*0.5
                let hi=tile.xy+tile.zw-texel*0.5
                // Depth slack must scale with a shadow texel's world size
                // and the filter footprint, not just distance. The old
                // 0.002*z allowance left neighboring floor/wall samples
                // falsely occluding each other: a crawling sawtooth seam.
                // Keep the four-tap footprint smaller to avoid unnecessary
                // contact detachment on the low-cost path.
                let world_texel=max(z*record.w,0.00001)
                let slope=1.0+2.0*(1.0-clamp(dot(normal,delta*(-1.0)/max(length(delta),0.0001)),0.0,1.0))
                var spread=1.5
                if self.local_shadow_soft>0.5 && self.local_shadow_source_radius>0.0 {
                    // Five bilinear blocker queries (20 depth reads). Reject depth variation
                    // within the receiver's footprint, not just at its centre,
                    // so a lit floor/wall corner is not classified as a caster.
                    let search=clamp(self.local_shadow_source_radius/world_texel,2.0,8.0)
                    let search_bias=max(0.015,world_texel*(search+1.0))*slope
                    let range=max(radius-record.y,0.0001)
                    let dx=vec2(texel.x*search,0.0)
                    let dy=vec2(0.0,texel.y*search)
                    let blockers=self.local_shadow_blocker(uv,lo,hi,z,record.y,range,search_bias)
                        +self.local_shadow_blocker(uv+dx,lo,hi,z,record.y,range,search_bias)
                        +self.local_shadow_blocker(uv-dx,lo,hi,z,record.y,range,search_bias)
                        +self.local_shadow_blocker(uv+dy,lo,hi,z,record.y,range,search_bias)
                        +self.local_shadow_blocker(uv-dy,lo,hi,z,record.y,range,search_bias)
                    if blockers.y>0.00001 {
                        let blocker=blockers.x/blockers.y
                        // Similar triangles: separated caster/receiver ->
                        // wider penumbra; contact -> compact filter. Clamp
                        // support and sample count, regardless of light size.
                        let penumbra=self.local_shadow_source_radius*max(z-blocker,0.0)/max(blocker,record.y)
                        spread=mix(1.5,clamp(penumbra/world_texel,1.5,4.0),smoothstep(0.0,0.5,blockers.y))
                    }
                }
                let filter_radius=mix(1.0,spread+0.5,step(0.5,self.local_shadow_soft))
                let bias=max(0.015,world_texel*filter_radius)*slope
                let depth=(z-record.y-bias)/max(radius-record.y,0.0001)
                if self.local_shadow_soft>0.5 {
                    // Dense compact-support filter: unlike a sparse disk it
                    // cannot reveal separate tap silhouettes/bands. Weights
                    // reach zero smoothly at the support boundary, including
                    // when the sample window advances by one texel.
                    // 4–64 filter reads + 20 blocker reads; baseline stays 4.
                    let t=uv/texel-vec2(0.5,0.5)
                    let f=fract(t)
                    let base=(floor(t)+vec2(0.5,0.5))*texel
                    var sum=0.0
                    var weight=0.0
                    var y=0.0
                    while y<8.0 {
                        let dy=(y-3.0-f.y)/spread
                        let wy=pow(max(1.0-dy*dy,0.0),2.0)
                        if wy>0.0 {
                            var x=0.0
                            while x<8.0 {
                                let dx=(x-3.0-f.x)/spread
                                let wx=pow(max(1.0-dx*dx,0.0),2.0)
                                let w=wx*wy
                                if w>0.0 {
                                    let sample=self.local_shadow_map.sample_nearest(clamp(base+vec2(x-3.0,y-3.0)*texel,lo,hi)).x
                                    sum=sum+step(depth,sample)*w
                                    weight=weight+w
                                }
                                x=x+1.0
                            }
                        }
                        y=y+1.0
                    }
                    return sum/max(weight,0.00001)
                }
                return self.local_shadow_compare(uv,lo,hi,depth)
            }
        }
    }
}

// Hardware PCF variant: same atlas/caster geometry, no per-pixel blocker search.
// The legacy module above remains available with MAKEPAD_LOCAL_SHADOWS=legacy.
pub(crate) mod hardware_sampling {
    use makepad_draw::*;
    script_mod! {
        use mod.prelude.widgets_internal.*
        mod.draw.LocalShadowSampling.local_shadow_map = texture_depth(float)
        mod.draw.LocalShadowSampling.local_shadow_depth_range = uniform(vec2(1.0, 0.0))
        mod.draw.LocalShadowSampling.local_shadow_pcf = fn(uv: vec2, lo: vec2, hi: vec2, depth: float) -> float {
            // A separable 1:2:1 kernel, regrouped into two bilinear samples
            // per axis. Hardware compares before interpolation.
            let texel = self.local_shadow_tex.zw
            let grid = uv / texel + vec2(0.5, 0.5)
            let phase = fract(grid)
            let origin = (floor(grid) - vec2(0.5, 0.5)) * texel
            let left = vec2(3.0, 3.0) - 2.0 * phase
            let right = vec2(1.0, 1.0) + 2.0 * phase
            let a = (vec2(2.0, 2.0) - phase) / left - vec2(1.0, 1.0)
            let b = phase / right + vec2(1.0, 1.0)
            let p00 = clamp(origin + vec2(a.x, a.y) * texel, lo, hi)
            let p10 = clamp(origin + vec2(b.x, a.y) * texel, lo, hi)
            let p01 = clamp(origin + vec2(a.x, b.y) * texel, lo, hi)
            let p11 = clamp(origin + vec2(b.x, b.y) * texel, lo, hi)
            return (
                self.local_shadow_map.sample_compare(p00, depth) * left.x * left.y
                + self.local_shadow_map.sample_compare(p10, depth) * right.x * left.y
                + self.local_shadow_map.sample_compare(p01, depth) * left.x * right.y
                + self.local_shadow_map.sample_compare(p11, depth) * right.x * right.y
            ) / 16.0
        }
        mod.draw.LocalShadowSampling.local_shadow_visibility = fn(index: float, wp: vec3, normal: vec3, light_pos: vec3, radius: float) -> float {
                if self.local_shadow_on<0.5 {return 1.0}
                let record=self.local_shadow_fetch(index)
                if record.x<0.0 {return 0.0}
                if record.x<0.5 {return 1.0}
                let delta=wp-light_pos
                // Choose a cubemap face AFTER normal bias: the offset can
                // cross a face boundary, particularly on horizontal ground.
                let receiver=wp+normal*max(0.01,length(delta)*record.w*1.5)
                let shadow_delta=receiver-light_pos
                var face=0.0
                if record.x>1.5 {
                    let a=abs(shadow_delta)
                    if a.x>=a.y && a.x>=a.z {
                        if shadow_delta.x<0.0 {face=1.0}
                    } else if a.y>=a.z {
                        face=2.0
                        if shadow_delta.y<0.0 {face=3.0}
                    } else {
                        face=4.0
                        if shadow_delta.z<0.0 {face=5.0}
                    }
                }
                let at=record.z+face*4.0
                let p=vec4(receiver.x,receiver.y,receiver.z,1.0)
                let z=dot(self.local_shadow_fetch(at+2.0),p)
                if z<=record.y {return 1.0}
                let q=vec2(dot(self.local_shadow_fetch(at),p)/z,dot(self.local_shadow_fetch(at+1.0),p)/z)
                if abs(q.x)>1.0 || abs(q.y)>1.0 {return 0.0}
                let tile=self.local_shadow_fetch(at+3.0)
                let uv=tile.xy+vec2(q.x*0.5+0.5,0.5-q.y*0.5)*tile.zw
                let texel=self.local_shadow_tex.zw
                let lo=tile.xy+texel*0.5
                let hi=tile.xy+tile.zw-texel*0.5
                // Depth slack must scale with a shadow texel's world size
                // and the filter footprint, not just distance. The old
                // 0.002*z allowance left neighboring floor/wall samples
                // falsely occluding each other: a crawling sawtooth seam.
                // Keep the four-tap footprint smaller to avoid unnecessary
                // contact detachment on the low-cost path.
                let world_texel=max(z*record.w,0.00001)

                let slope = 1.0 + 2.0 * (1.0 - clamp(dot(normal, delta * (-1.0) / max(length(delta), 0.0001)), 0.0, 1.0))
                let biased_z = max(z - max(0.015, world_texel * 2.0) * slope, record.y)
                // Match the rasterized projective depth, not the linear R32F
                // legacy color output. GL has an additional viewport remap.
                let projected = radius * (biased_z - record.y) / max(biased_z * (radius - record.y), 0.000001)
                let depth = projected * self.local_shadow_depth_range.x + self.local_shadow_depth_range.y
                return self.local_shadow_pcf(uv, lo, hi, depth)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLocalShadowSkinned {
    #[deref]
    pub base: DrawLmSunDepthSkinned,
    #[live]
    pub lamp_range: Vec4f,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    mod.draw.DrawLocalShadowSkinned = mod.std.set_type_default() do #(DrawLocalShadowSkinned::script_shader(vm)) {
        ..mod.draw.DrawLmSunDepthSkinned
        v_local_view: varying(vec3f)
        vertex: fn() {
            let pos=self.skinned_pos()
            let wp=self.transform*vec4(pos.x,pos.y,pos.z,1.0)
            let vx=dot(self.sun_rx,wp)
            let vy=dot(self.sun_ry,wp)
            let vz=dot(self.sun_rz,wp)
            self.v_local_view=vec3(vx,vy,vz)
            self.vertex_pos=vec4(vx*self.tile_a.x+vz*self.tile_a.z,vy*self.tile_a.y+vz*self.tile_a.w,
                (vz-self.lamp_range.x)*self.lamp_range.y/max(self.lamp_range.y-self.lamp_range.x,0.0001),vz)
        }
        pixel: fn() {
            let vz=max(self.v_local_view.z,0.0001)
            if abs(self.v_local_view.x/vz)>0.999 || abs(self.v_local_view.y/vz)>0.999 {discard()}
            return vec4((self.v_local_view.z-self.lamp_range.x)/max(self.lamp_range.y-self.lamp_range.x,0.0001),0.0,0.0,1.0)
        }
        fragment: fn() {self.fb0=self.pixel()}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_atlas_uses_only_configured_columns() {
        let mut shadows=LocalShadows::default();
        for faces in 1..=16 {
            shadows.set_config(LocalShadowConfig{max_faces:faces,resolution:1024});
            let (w,h)=shadows.size();
            assert_eq!(w,faces.min(4)*1024);
            assert_eq!(h,faces.div_ceil(4)*1024);
            let lights=vec![spot(Vec3f::default());faces];
            shadows.prepare(&lights,&vec![true;faces],Vec3f::default());
            assert_eq!(shadows.stats.faces,faces);
            for row in shadows.data.chunks_exact(16) {
                assert!(row[12]>=0.0&&row[13]>=0.0);
                assert!(row[12]+row[14]<=1.0&&row[13]+row[15]<=1.0);
            }
        }
        shadows.set_config(LocalShadowConfig{max_faces:1,resolution:1024});
        assert_eq!(shadows.size(),(1024,1024));
        assert!(1024*1024 < 8*512*512);
    }
    fn soft_weights(f:f32,spread:f32)->[f32;8] {
        let mut weights=std::array::from_fn(|i| {
            let d=(i as f32-3.0-f)/spread;
            (1.0-d*d).max(0.0).powi(2)
        });
        let sum=weights.iter().sum::<f32>();
        for w in &mut weights {*w/=sum;}
        weights
    }
    #[test]
    fn filtered_shadow_does_not_invent_occlusion_at_a_lit_room_corner() {
        // Analytic floor/wall depth samples, using the real spotlight camera
        // rows and pixel-centre footprint. A light above/in front of both
        // planes sees the whole inside corner: there is no contact occluder.
        // Sweep map resolution, filter width and light direction so an
        // axis-aligned map cannot hide the original crawling seam.
        let mut old_bias_failures = 0;
        for resolution in [128, 256, 512, 1024] {
            for soft in [false, true] {
                for phase in [-0.25, -0.13, 0.0, 0.13, 0.25] {
                    let light = LmLight {
                        pos: vec3f(2.0, 4.8, 2.0),
                        dir: vec3f(phase, -0.8, -1.0).normalize(),
                        cone: Some((28.0, 52.0)),
                        ..spot(Vec3f::default())
                    };
                    let mut shadows = LocalShadows::default();
                    shadows.set_config(LocalShadowConfig { max_faces: 1, resolution });
                    shadows.prepare(&[light.clone()], &[true], Vec3f::default());
                    let face = &shadows.faces[0];
                    let axis = |r: Vec4f| vec3f(r.x, r.y, r.z);
                    let rx = axis(face.rx);
                    let ry = axis(face.ry);
                    let rz = axis(face.rz);
                    let focal = rx.length();
                    let pixels = (resolution - 2) as f32;
                    let texel_world_per_depth = 2.0 / (pixels * focal);
                    for x in [-3.0, -1.17, 0.31, 2.53, 4.0] {
                        for (wp, normal) in [
                            (vec3f(x, 0.002, -5.0), vec3f(0.0, 0.0, 1.0)),
                            (vec3f(x, 0.0, -4.998), vec3f(0.0, 1.0, 0.0)),
                        ] {
                            let delta = wp - light.pos;
                            let receiver = wp + normal * (delta.length() * texel_world_per_depth * 1.5).max(0.01);
                            let z = dot(face.rz, receiver);
                            let q = [dot(face.rx, receiver) / z, dot(face.ry, receiver) / z];
                            let uv = [(q[0] * 0.5 + 0.5) * pixels + 1.0,
                                      (0.5 - q[1] * 0.5) * pixels + 1.0];
                            let slope = 1.0 + 2.0 * (1.0 - normal.dot(delta * (-1.0 / delta.length())).clamp(0.0, 1.0));
                            let bias = (z * texel_world_per_depth * if soft { 2.0 } else { 1.0 }).max(0.015) * slope;
                            let old_bias = (0.002 * z).max(0.015) * slope;
                            let mut visible = 0.0;
                            let mut old_visible = 0.0;
                            let weights = |u: f32| {
                                let f = (u - 0.5).fract();
                                if soft {soft_weights(f,1.5)}else{[0.0,0.0,0.0,1.0-f,f,0.0,0.0,0.0]}
                            };
                            let depth_at = |sx:f32, sy:f32| {
                                let dx = ((sx-1.0)/pixels-0.5)*2.0;
                                let dy = (0.5-(sy-1.0)/pixels)*2.0;
                                // Unit view-Z: t is the atlas's linear depth.
                                let rd = rz + rx*(dx/(focal*focal)) + ry*(dy/(focal*focal));
                                (-light.pos.y/rd.y).min((-5.0-light.pos.z)/rd.z)
                            };
                            if soft {
                                let tw=(z*texel_world_per_depth).max(0.00001);
                                let search=(0.4/tw).clamp(2.0,8.0);
                                let search_bias=(tw*(search+1.0)).max(0.015)*slope;
                                for (ox,oy) in [(0.0,0.0),(-search,0.0),(search,0.0),(0.0,-search),(0.0,search)] {
                                    for iy in 0..2 {for ix in 0..2 {
                                        let d=depth_at((uv[0]+ox-0.5).floor()+0.5+ix as f32,
                                                       (uv[1]+oy-0.5).floor()+0.5+iy as f32);
                                        assert!(z-d<=search_bias,"lit corner misclassified as a blocker");
                                    }}
                                }
                            }
                            for (iy, wy) in weights(uv[1]).into_iter().enumerate() {
                                for (ix, wx) in weights(uv[0]).into_iter().enumerate() {
                                    let d=depth_at((uv[0]-0.5).floor()+0.5+ix as f32-3.0,
                                                   (uv[1]-0.5).floor()+0.5+iy as f32-3.0);
                                    visible += if z-bias <= d { wx*wy } else { 0.0 };
                                    old_visible += if z-old_bias <= d { wx*wy } else { 0.0 };
                                }
                            }
                            assert!(visible > 0.9999, "false corner shadow: {visible}, res={resolution}, soft={soft}, phase={phase}, p={wp:?}");
                            if old_visible < 0.99 { old_bias_failures += 1; }
                        }
                    }
                }
            }
        }
        assert!(old_bias_failures > 0, "fixture must catch the previous seam");
    }
    #[test]
    fn soft_comparison_filter_is_normalized_and_continuous() {
        for spread in [1.5,2.0,3.0,4.0] {
            for step in 0..=100 {
                let f=step as f32/100.0;
                let x=soft_weights(f,spread);
                let y=soft_weights(1.0-f,spread);
                let sum=x.iter().flat_map(|a|y.iter().map(move |b|a*b)).sum::<f32>();
                assert!((sum-1.0).abs()<1e-6);
                assert!(x.iter().all(|&w|w>=0.0));
            }
            // At a texel boundary the support shifts without a brightness
            // jump, even across a black/white depth-comparison silhouette.
            let depths=[0.0,0.0,0.0,0.0,1.0,1.0,1.0,1.0,1.0];
            let left=soft_weights(1.0,spread).iter().zip(&depths[..8]).map(|(a,b)|a*b).sum::<f32>();
            let right=soft_weights(0.0,spread).iter().zip(&depths[1..]).map(|(a,b)|a*b).sum::<f32>();
            assert!((left-right).abs()<1e-6);
        }
    }
    #[test]
    fn penumbra_grows_with_separation_and_light_size_but_is_bounded() {
        let spread=|receiver:f32,blocker:f32,source:f32| {
            (source*(receiver-blocker).max(0.0)/blocker.max(0.018)/0.04).clamp(1.5,4.0)
        };
        assert_eq!(spread(6.0,6.0,0.4),1.5);
        assert!(spread(8.0,5.0,0.4)>spread(6.0,5.0,0.4));
        assert!(spread(8.0,5.0,0.4)>spread(8.0,5.0,0.2));
        assert_eq!(spread(20.0,0.1,2.0),4.0);
        assert_eq!(spread(20.0,0.1,0.0),1.5);
    }
    fn spot(pos: Vec3f) -> LmLight {
        LmLight {
            pos,
            color: vec3f(1.0, 1.0, 1.0),
            radius: 30.0,
            dir: vec3f(0.0, 0.0, -1.0),
            cone: Some((10.0, 25.0)),
            shadows: true,
            ..Default::default()
        }
    }
    #[test]
    fn atlas_budget_never_unshadows_an_excluded_request() {
        let mut s = LocalShadows::default();
        s.set_config(LocalShadowConfig {
            max_faces: 2,
            resolution: 256,
        });
        s.prepare(
            &[
                spot(vec3f(0.0, 0.0, 0.0)),
                spot(vec3f(50.0, 0.0, 0.0)),
                spot(vec3f(2.0, 0.0, 0.0)),
            ],
            &[true; 3],
            Vec3f::default(),
        );
        assert_eq!(s.stats.faces, 2);
        assert_eq!(s.stats.omitted_lights, 1);
        assert_eq!(s.records[1].count, -1);
        assert_eq!(s.records[0].count, 1);
        assert_eq!(s.records[2].count, 1);
        s.prepare(&[], &[], Vec3f::default());
        assert!(s.records.is_empty());
        assert!(s.data.is_empty());
    }
    #[test]
    fn point_light_reserves_all_six_faces_or_none() {
        let mut s = LocalShadows::default();
        let p = LmLight {
            cone: None,
            ..spot(Vec3f::default())
        };
        s.set_config(LocalShadowConfig {
            max_faces: 5,
            ..Default::default()
        });
        s.prepare(&[p.clone()], &[true], Vec3f::default());
        assert_eq!(s.records[0].count, -1);
        s.set_config(LocalShadowConfig {
            max_faces: 6,
            ..Default::default()
        });
        s.prepare(&[p], &[true], Vec3f::default());
        assert_eq!(s.records[0].count, 6);
        assert_eq!(s.data.len(), 6 * 16);
    }
    #[test]
    fn imported_punctual_lights_enter_the_bounded_shadow_atlas() {
        for (kind,faces) in [("spot",1),("point",6)] {
            let json=format!(r#"{{"asset":{{"version":"2.0"}},"nodes":[
                {{"translation":[3,0,0],"scale":[2,2,2],"children":[1]}},
                {{"translation":[0,1,0],"rotation":[0,1,0,0],"extensions":{{"KHR_lights_punctual":{{"light":0}}}}}}
            ],"extensions":{{"KHR_lights_punctual":{{"lights":[{{"type":"{kind}","color":[1,0.5,0.25],"intensity":420,"range":28,"spot":{{"innerConeAngle":0.18,"outerConeAngle":0.48}}}}]}}}}}}"#);
            let document=makepad_gltf::parse_gltf_json(&json).unwrap();
            let imported=crate::asset_lights::parse_asset_lights(&document).unwrap();
            let mut instance=Mat4f::identity();instance.v[12]=10.0;
            let light=imported[0].placed(&instance,None);
            assert_eq!(light.pos,vec3f(13.0,2.0,0.0));
            assert_eq!(light.dir,vec3f(0.0,0.0,1.0));
            assert_eq!(light.color,vec3f(420.0,210.0,105.0));
            assert_eq!(light.radius,28.0,"node scale cannot alter photometric range");
            assert!(light.spot<0.0,"the original inverse-square convention is retained");
            let mut shadows=LocalShadows::default();
            shadows.set_config(LocalShadowConfig{max_faces:faces,resolution:256});
            shadows.prepare(std::slice::from_ref(&light),&[true],light.pos);
            assert_eq!(shadows.records[0].count,faces as i32,"imported fixtures must request real shadow faces");
            assert_eq!(shadows.stats.lights,1);assert_eq!(shadows.stats.faces,faces);
            assert_eq!(shadows.stats.omitted_lights,0);
            let half=vec3f(0.01,0.01,0.01);
            if kind=="spot" {
                let face=&shadows.faces[0];let front=light.pos+light.dir*2.0;let back=light.pos-light.dir*2.0;
                assert!(in_face(face,shadows.records[0].near,light.radius,front-half,front+half));
                assert!(!in_face(face,shadows.records[0].near,light.radius,back-half,back+half));
            } else {
                for dir in [vec3f(1.0,0.0,0.0),vec3f(-1.0,0.0,0.0),vec3f(0.0,1.0,0.0),vec3f(0.0,-1.0,0.0),vec3f(0.0,0.0,1.0),vec3f(0.0,0.0,-1.0)] {
                    let receiver=light.pos+dir*2.0;
                    assert_eq!(shadows.faces.iter().filter(|f|in_face(f,shadows.records[0].near,light.radius,receiver-half,receiver+half)).count(),1,"point fixture covers each cube direction");
                }
            }
            shadows.set_config(LocalShadowConfig{max_faces:faces-1,resolution:256});
            shadows.prepare(std::slice::from_ref(&light),&[true],light.pos);
            assert_eq!(shadows.records[0].count,-1,"insufficient atlas space omits the light instead of leaking through walls");
            assert_eq!(shadows.stats.faces,0);assert_eq!(shadows.stats.omitted_lights,1);
        }
    }
    #[test]
    fn spotlight_projection_culls_behind_and_covers_its_cone() {
        let mut s = LocalShadows::default();
        s.prepare(&[spot(vec3f(1.0, 2.0, 3.0))], &[true], Vec3f::default());
        let f = &s.faces[0];
        let p = vec3f(1.0, 2.0, -7.0);
        assert!(dot(f.rx, p).abs() < 1e-5 && dot(f.ry, p).abs() < 1e-5);
        assert!((dot(f.rz, p) - 10.0).abs() < 1e-5);
        assert!(in_face(
            f,
            0.03,
            30.0,
            p - vec3f(1.0, 1.0, 1.0),
            p + vec3f(1.0, 1.0, 1.0)
        ));
        assert!(!in_face(
            f,
            0.03,
            30.0,
            vec3f(0.0, 0.0, 8.0),
            vec3f(2.0, 4.0, 10.0)
        ));
        assert!(s.data[12] > 0.0 && s.data[14] < 0.25);
    }
}
