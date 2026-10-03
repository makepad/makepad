//! CPU-built clustered forward lighting. No geometry prepass, readback or compute
//! dependency. The texture ABI is shared by native and web backends.
//!
//! Flat views use frustum tiles and logarithmic depth. XR currently has no host
//! eye-projection snapshot (the backend late-latches it), so it uses a conservative
//! world-space grid over light influence instead of incorrectly using the flat
//! camera. The same fragment lookup works for both eyes and for MR stage scaling.
use crate::lightmap::LmLight;
use makepad_draw::*;

const TEXTURE_WIDTH: usize = 256;
pub const MAX_CLUSTER_LIGHTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClusterConfig {
    pub tiles_x: usize,
    pub tiles_y: usize,
    pub slices: usize,
    pub lights_per_cluster: usize,
    pub max_lights: usize,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            tiles_x: 16,
            tiles_y: 9,
            slices: 24,
            lights_per_cluster: 32,
            max_lights: 1024,
        }
    }
}

impl ClusterConfig {
    fn clamped(self) -> Self {
        Self {
            tiles_x: self.tiles_x.clamp(1, 32),
            tiles_y: self.tiles_y.clamp(1, 24),
            slices: self.slices.clamp(2, 32),
            lights_per_cluster: self.lights_per_cluster.clamp(1, MAX_CLUSTER_LIGHTS),
            max_lights: self.max_lights.clamp(1, 4096),
        }
    }
    fn count(self) -> usize {
        self.tiles_x * self.tiles_y * self.slices
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClusterStats {
    pub shadows: crate::local_shadows::LocalShadowStats,
    pub lights: usize,
    pub occupied_clusters: usize,
    pub references: usize,
    pub max_lights_in_cluster: usize,
    /// Each omitted light/cluster intersection, not just the number of cells.
    pub overflow_references: usize,
    pub rejected_lights: usize,
    pub build_us: u64,
    pub upload_bytes: usize,
    pub world_grid: bool,
}

#[derive(Clone, Copy, Default)]
struct Bounds {
    min: Vec3f,
    max: Vec3f,
}

/// Whether box `b` lies wholly outside the cone at `apex` along unit `axis`
/// with half angle (sin, cos): the box's bounding sphere is tested, so the
/// answer is conservative (a box it keeps may still miss the cone).
fn outside_cone(b: Bounds, apex: Vec3f, axis: Vec3f, sin: f32, cos: f32) -> bool {
    let center = (b.min + b.max) * 0.5;
    let radius = (b.max - b.min).length() * 0.5;
    let v = center - apex;
    let along = v.dot(axis);
    let across = (v.dot(v) - along * along).max(0.0).sqrt();
    along < -radius || cos * across - along * sin > radius
}

fn finite(v: Vec3f) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}
fn valid_light(l: &LmLight) -> bool {
    finite(l.pos)
        && finite(l.color)
        && finite(l.dir)
        && l.radius.is_finite()
        && l.radius > 0.0
        && l.spot.is_finite()
        && l.cone.map_or(true, |(inner,outer)| inner.is_finite() && outer.is_finite()
            && inner >= 0.0 && inner <= outer && outer > 0.0 && outer <= 90.0)
        && l.pos.length().is_finite()
        && (l.radius * l.radius).is_finite()
        && l.color.x.max(l.color.y).max(l.color.z) > 0.0
        && l.area.map_or(true, |a| finite(a.tangent) && a.tangent.length() > 1.0e-6
            && a.half_width.is_finite() && a.half_height.is_finite()
            && a.half_width > 0.0 && a.half_height >= 0.0 && l.dir.length() > 1.0e-6)
}
fn row(m: &Mat4f, i: usize) -> Vec4f {
    vec4(m.v[i], m.v[4 + i], m.v[8 + i], m.v[12 + i])
}
#[cfg(test)]
fn dot_row(r: Vec4f, p: Vec3f) -> f32 {
    r.x * p.x + r.y * p.y + r.z * p.z + r.w
}
fn distance_squared(b: Bounds, p: Vec3f) -> f32 {
    let x = (b.min.x - p.x).max(0.0).max(p.x - b.max.x);
    let y = (b.min.y - p.y).max(0.0).max(p.y - b.max.y);
    let z = (b.min.z - p.z).max(0.0).max(p.z - b.max.z);
    x * x + y * y + z * z
}

/// Per-view data. Reuses all list storage; no allocations per object or cluster.
pub struct ClusteredLights {
    pub(crate) shadows: crate::local_shadows::LocalShadows,
    active: Vec<bool>,
    config: ClusterConfig,
    lights: Vec<LmLight>,
    bounds: Vec<Bounds>,
    counts: Vec<usize>,
    indices: Vec<usize>,
    scores: Vec<f32>,
    rows: [Vec4f; 4],
    depth_row: Vec4f,
    near: f32,
    far: f32,
    log_scale: f32,
    world_grid: bool,
    texture: Option<Texture>,
    texture_height: usize,
    light_texel: usize,
    stats: ClusterStats,
    last_overflow: (usize, usize),
    last_overflow_log: f64,
    /// The texture holds an empty grid (no light in any cluster): a frame
    /// with no lights again has nothing new to pack or upload.
    uploaded_empty: bool,
    /// The lighting convention every lit lane reads (`lin_ctl`; see
    /// `Renderer::lin_ctl`), bound together with the cluster uniforms.
    pub(crate) lin_ctl: [f32; 4],
    /// HDR height fog (`fog_ctl`, `fog_eye`), bound with the cluster uniforms.
    pub(crate) fog_ctl: [f32; 4],
    pub(crate) fog_eye: [f32; 4],
}

impl Default for ClusteredLights {
    fn default() -> Self {
        Self {
            shadows: Default::default(),
            active: Vec::new(),
            config: ClusterConfig::default(),
            lights: Vec::new(),
            bounds: Vec::new(),
            counts: Vec::new(),
            indices: Vec::new(),
            scores: Vec::new(),
            rows: [Vec4f::default(); 4],
            depth_row: Vec4f::default(),
            near: 2.0,
            far: 1000.0,
            log_scale: 1.0,
            world_grid: false,
            texture: None,
            texture_height: 0,
            light_texel: 0,
            stats: ClusterStats::default(),
            last_overflow: (0, 0),
            last_overflow_log: -1.0,
            uploaded_empty: false,
            lin_ctl: [0.0, 1.0, 1.0, 1.0],
            fog_ctl: [0.0; 4],
            fog_eye: [0.0; 4],
        }
    }
}

impl ClusteredLights {
    pub(crate) fn gi_light_ids(&self,center:Vec3f)->[f32;8] {
        let mut ranked:Vec<_>=self.lights.iter().enumerate().filter(|(i,_)|self.shadows.records.get(*i).map_or(false,|r|r.count>0))
            .map(|(i,l)|(l.color.x.max(l.color.y).max(l.color.z)/(1.0+(l.pos-center).length()),i)).collect();
        ranked.sort_by(|a,b|b.0.total_cmp(&a.0));let mut out=[-1.0;8];
        for (slot,(_,i)) in ranked.into_iter().take(8).enumerate(){out[slot]=i as f32;}out
    }
    pub(crate) fn parent_shadows(&self,cx:&mut Cx,parent:DrawPassId){self.shadows.parent_to(cx,parent);}
    pub fn set_config(&mut self, config: ClusterConfig) {
        self.config = config.clamped();
    }
    pub fn config(&self) -> ClusterConfig {
        self.config
    }
    pub fn stats(&self) -> ClusterStats {
        self.stats
    }

    pub fn render_shadows(&mut self,cx:&mut CxDraw,eye:Vec3f,statics:&[crate::gpu_lightmap::GpuBakeMesh],movers:&[crate::gpu_lightmap::GpuLmMover]) {
        self.active.clear(); self.active.resize(self.lights.len(),false);
        for (cell,count) in self.counts.iter().enumerate() {
            for k in 0..*count {self.active[self.indices[cell*self.config.lights_per_cluster+k]]=true;}
        }
        self.shadows.render(cx,&self.lights,&self.active,eye,statics,movers);
        self.stats.shadows=self.shadows.stats;
    }

    fn slice(&self, depth: f32) -> usize {
        if self.world_grid {
            return (depth.clamp(0.0, 0.999999) * self.config.slices as f32) as usize;
        }
        if depth < self.near {
            return 0;
        }
        (1 + ((depth / self.near).log2() * self.log_scale).max(0.0) as usize)
            .min(self.config.slices - 1)
    }

    fn slice_start(&self, index: usize) -> f32 {
        if index == 0 {
            0.0
        } else {
            self.near * 2.0f32.powf((index - 1) as f32 / self.log_scale)
        }
    }

    /// CPU equivalent of the shader address calculation.
    #[cfg(test)]
    fn cluster_at(&self, p: Vec3f) -> Option<usize> {
        let w = dot_row(self.rows[3], p);
        if w <= 0.0 {
            return None;
        }
        let u = dot_row(self.rows[0], p) / w * 0.5 + 0.5;
        let v = dot_row(self.rows[1], p) / w * 0.5 + 0.5;
        let d = dot_row(self.depth_row, p);
        if !(0.0..=1.0).contains(&u)
            || !(0.0..=1.0).contains(&v)
            || d < 0.0
            || d > if self.world_grid { 1.0 } else { self.far }
        {
            return None;
        }
        let x = ((u * self.config.tiles_x as f32) as usize).min(self.config.tiles_x - 1);
        let y = ((v * self.config.tiles_y as f32) as usize).min(self.config.tiles_y - 1);
        Some(x + self.config.tiles_x * (y + self.config.tiles_y * self.slice(d)))
    }

    fn world_bounds(&mut self) -> Bounds {
        let mut b = Bounds {
            min: vec3f(-1.0, -1.0, -1.0),
            max: vec3f(1.0, 1.0, 1.0),
        };
        for (i, l) in self.lights.iter().enumerate() {
            let r = vec3f(l.radius, l.radius, l.radius);
            let (lo, hi) = (l.pos - r, l.pos + r);
            if i == 0 {
                b = Bounds { min: lo, max: hi };
            } else {
                b.min = vec3f(b.min.x.min(lo.x), b.min.y.min(lo.y), b.min.z.min(lo.z));
                b.max = vec3f(b.max.x.max(hi.x), b.max.y.max(hi.y), b.max.z.max(hi.z));
            }
        }
        // Tiny radii at large world coordinates can round both endpoints
        // to the same float. Pad conservatively before building the affine
        // lookup rather than dividing by a zero-sized world-grid axis.
        let pad = vec3f(
            b.min.x.abs().max(b.max.x.abs()) * 1.0e-6 + 0.001,
            b.min.y.abs().max(b.max.y.abs()) * 1.0e-6 + 0.001,
            b.min.z.abs().max(b.max.z.abs()) * 1.0e-6 + 0.001,
        );
        b.min = b.min - pad;
        b.max = b.max + pad;
        b
    }

    /// `view` maps TRUE world coordinates into the clustering camera. None is
    /// the XR-safe world-grid fallback, never the unrelated flat orbit camera.
    fn build(&mut self, lights: &[LmLight], view: Option<(Mat4f, Mat4f)>) {
        self.config = self.config.clamped();
        let c = self.config;
        self.stats = ClusterStats::default();
        self.lights.clear();
        // Deterministic global cap. Report it; never let generated content
        // turn the per-frame upload or shader loop into an unbounded task.
        for light in lights {
            if valid_light(light) && self.lights.len() < c.max_lights {
                self.lights.push(light.clone());
            } else {
                self.stats.rejected_lights += 1;
            }
        }
        self.stats.lights = self.lights.len();
        self.bounds.resize(c.count(), Bounds::default());
        self.counts.resize(c.count(), 0);
        self.counts.fill(0);
        self.indices.resize(c.count() * c.lights_per_cluster, 0);
        self.scores.resize(self.indices.len(), 0.0);

        let view = view
            .filter(|(v, p)| v.v.iter().chain(p.v.iter()).all(|x| x.is_finite()))
            .filter(|(_, p)| p.v[0].abs() > 1.0e-6 && p.v[5].abs() > 1.0e-6);
        self.world_grid = view.is_none();
        self.stats.world_grid = self.world_grid;
        let mut box_bounds = Bounds::default();
        let mut view_matrix = Mat4f::identity();
        let mut projection = Mat4f::identity();
        if let Some((v, p)) = view {
            view_matrix = v;
            projection = p;
            let clip = Mat4f::mul(&p, &v);
            self.rows = [row(&clip, 0), row(&clip, 1), row(&clip, 2), row(&clip, 3)];
            let vz = row(&v, 2);
            self.depth_row = vec4(-vz.x, -vz.y, -vz.z, -vz.w);
            let inv = p.invert();
            let fp = inv.transform_vec4(vec4(0.0, 0.0, 1.0, 1.0));
            self.far = if fp.w.abs() > 1.0e-8 {
                (-fp.z / fp.w).abs().clamp(1.0, 100000.0)
            } else {
                100000.0
            };
            self.near = 2.0f32.min(self.far * 0.1);
            self.log_scale = (c.slices - 1) as f32 / (self.far / self.near).log2();
            let ortho = p.v[15].abs() > 0.5;
            // Cluster bounds only place lights: none, nothing to place.
            for z in (0..c.slices).filter(|_| !self.lights.is_empty()) {
                let (z0, z1) = (self.slice_start(z), self.slice_start(z + 1));
                for y in 0..c.tiles_y {
                    for x in 0..c.tiles_x {
                        let mut lo = vec3f(f32::MAX, f32::MAX, f32::MAX);
                        let mut hi = vec3f(-f32::MAX, -f32::MAX, -f32::MAX);
                        for (tx, ty) in [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)] {
                            let q = inv.transform_vec4(vec4(
                                tx as f32 / c.tiles_x as f32 * 2.0 - 1.0,
                                ty as f32 / c.tiles_y as f32 * 2.0 - 1.0,
                                1.0,
                                1.0,
                            ));
                            for depth in [z0, z1] {
                                let q = if ortho {
                                    vec3f(q.x / q.w, q.y / q.w, -depth)
                                } else {
                                    vec3f(q.x / (-q.z) * depth, q.y / (-q.z) * depth, -depth)
                                };
                                lo = vec3f(lo.x.min(q.x), lo.y.min(q.y), lo.z.min(q.z));
                                hi = vec3f(hi.x.max(q.x), hi.y.max(q.y), hi.z.max(q.z));
                            }
                        }
                        self.bounds[x + c.tiles_x * (y + c.tiles_y * z)] =
                            Bounds { min: lo, max: hi };
                    }
                }
            }
        } else {
            box_bounds = self.world_bounds();
            let b = box_bounds;
            let s = b.max - b.min;
            self.rows = [
                vec4(2.0 / s.x, 0.0, 0.0, -1.0 - 2.0 * b.min.x / s.x),
                vec4(0.0, 2.0 / s.y, 0.0, -1.0 - 2.0 * b.min.y / s.y),
                Vec4f::default(),
                vec4(0.0, 0.0, 0.0, 1.0),
            ];
            self.depth_row = vec4(0.0, 0.0, 1.0 / s.z, -b.min.z / s.z);
            for z in (0..c.slices).filter(|_| !self.lights.is_empty()) {
                for y in 0..c.tiles_y {
                    for x in 0..c.tiles_x {
                        let lo = b.min
                            + vec3f(
                                s.x * x as f32 / c.tiles_x as f32,
                                s.y * y as f32 / c.tiles_y as f32,
                                s.z * z as f32 / c.slices as f32,
                            );
                        let hi = b.min
                            + vec3f(
                                s.x * (x + 1) as f32 / c.tiles_x as f32,
                                s.y * (y + 1) as f32 / c.tiles_y as f32,
                                s.z * (z + 1) as f32 / c.slices as f32,
                            );
                        self.bounds[x + c.tiles_x * (y + c.tiles_y * z)] =
                            Bounds { min: lo, max: hi };
                    }
                }
            }
        }

        // Rasterize each light's conservative projected bounds into the grid,
        // then refine with sphere/AABB tests. Never scan the whole scene or
        // allocate a vector for each cluster. Spot cones use a safe sphere.
        let radius_scale = (0..3)
            .map(|j| {
                (view_matrix.v[j * 4].powi(2)
                    + view_matrix.v[j * 4 + 1].powi(2)
                    + view_matrix.v[j * 4 + 2].powi(2))
                .sqrt()
            })
            .fold(0.0f32, f32::max);
        for li in 0..self.lights.len() {
            let l = &self.lights[li];
            let q = view_matrix.transform_vec4(vec4(l.pos.x, l.pos.y, l.pos.z, 1.0));
            let p = vec3f(q.x, q.y, q.z);
            let r = l.radius * radius_scale;
            let mut u0 = 0.0f32;
            let mut u1 = 1.0f32;
            let mut v0 = 0.0f32;
            let mut v1 = 1.0f32;
            let (d0, d1);
            if self.world_grid {
                let s = box_bounds.max - box_bounds.min;
                u0 = (p.x - r - box_bounds.min.x) / s.x;
                u1 = (p.x + r - box_bounds.min.x) / s.x;
                v0 = (p.y - r - box_bounds.min.y) / s.y;
                v1 = (p.y + r - box_bounds.min.y) / s.y;
                d0 = (p.z - r - box_bounds.min.z) / s.z;
                d1 = (p.z + r - box_bounds.min.z) / s.z;
            } else {
                d0 = -p.z - r;
                d1 = -p.z + r;
                if d1 < 0.0 || d0 > self.far {
                    continue;
                }
                if d0 > 0.001 || projection.v[15].abs() > 0.5 {
                    u0 = f32::MAX;
                    v0 = f32::MAX;
                    u1 = -f32::MAX;
                    v1 = -f32::MAX;
                    for dx in [-r, r] {
                        for dy in [-r, r] {
                            for dz in [-r, r] {
                                let q = projection.transform_vec4(vec4(
                                    p.x + dx,
                                    p.y + dy,
                                    p.z + dz,
                                    1.0,
                                ));
                                let u = q.x / q.w * 0.5 + 0.5;
                                let v = q.y / q.w * 0.5 + 0.5;
                                u0 = u0.min(u);
                                u1 = u1.max(u);
                                v0 = v0.min(v);
                                v1 = v1.max(v);
                            }
                        }
                    }
                }
            }
            if u1 < 0.0 || u0 > 1.0 || v1 < 0.0 || v0 > 1.0 {
                continue;
            }
            let tile =
                |v: f32, n: usize| ((v.clamp(0.0, 1.0) * n as f32).floor() as usize).min(n - 1);
            let intensity = l.color.x.max(l.color.y).max(l.color.z);
            // A spot with a hard cone lights nothing past its outer angle
            // (the shader's angular term is exactly zero there): clusters
            // outside the cone do not get it. Axis in the grid's space.
            let cone = l.cone.filter(|(_, outer)| *outer < 89.0 && l.dir.length() > 1.0e-8).map(|(_, outer)| {
                let a = view_matrix.transform_vec4(vec4(l.dir.x, l.dir.y, l.dir.z, 0.0));
                let (sin, cos) = outer.to_radians().sin_cos();
                (vec3f(a.x, a.y, a.z).normalize(), sin, cos)
            });
            for z in self.slice(d0)..=self.slice(d1) {
                for y in tile(v0, c.tiles_y)..=tile(v1, c.tiles_y) {
                    for x in tile(u0, c.tiles_x)..=tile(u1, c.tiles_x) {
                        let ci = x + c.tiles_x * (y + c.tiles_y * z);
                        let ds = distance_squared(self.bounds[ci], p);
                        if ds > r * r {
                            continue;
                        }
                        if let Some((axis, sin, cos)) = cone {
                            if outside_cone(self.bounds[ci], p, axis, sin, cos) {
                                continue;
                            }
                        }
                        // Rank by a conservative contribution bound; nearest
                        // bright lights win over faint far ones on overflow.
                        let score = intensity * (1.0 - ds.sqrt() / r).max(0.0).powi(2);
                        let start = ci * c.lights_per_cluster;
                        let count = self.counts[ci];
                        let slot = if count < c.lights_per_cluster {
                            self.counts[ci] += 1;
                            count
                        } else {
                            self.stats.overflow_references += 1;
                            let mut weakest = 0;
                            for k in 1..count {
                                if self.scores[start + k] < self.scores[start + weakest] {
                                    weakest = k;
                                }
                            }
                            if score <= self.scores[start + weakest] {
                                continue;
                            }
                            weakest
                        };
                        self.indices[start + slot] = li;
                        self.scores[start + slot] = score;
                    }
                }
            }
        }
        self.stats.occupied_clusters = self.counts.iter().filter(|&&n| n > 0).count();
        self.stats.references = self.counts.iter().sum();
        self.stats.max_lights_in_cluster = self.counts.iter().copied().max().unwrap_or(0);
    }

    fn pack(&mut self, data: &mut Vec<f32>) {
        let c = self.config;
        self.light_texel = c.count() + (self.stats.references + 3) / 4;
        let texels = self.light_texel + self.lights.len() * 3;
        let height = ((texels.max(1) + TEXTURE_WIDTH - 1) / TEXTURE_WIDTH).next_power_of_two();
        self.texture_height = self.texture_height.max(height);
        data.resize(TEXTURE_WIDTH * self.texture_height * 4, 0.0);
        data.fill(0.0);
        let mut offset = 0;
        for ci in 0..c.count() {
            data[ci * 4] = offset as f32;
            data[ci * 4 + 1] = self.counts[ci] as f32;
            for k in 0..self.counts[ci] {
                data[c.count() * 4 + offset] = self.indices[ci * c.lights_per_cluster + k] as f32;
                offset += 1;
            }
        }
        for (i, l) in self.lights.iter().enumerate() {
            let at = (self.light_texel + i * 3) * 4;
            let dir = if l.dir.length() > 1.0e-8 {
                l.dir.normalize()
            } else {
                vec3f(0.0, -1.0, 0.0)
            };
            let (angular, mode) = match (l.spot < 0.0, l.cone) {
                (true, None) => (0.0, -2.0),
                (true, Some((inner, outer))) => (outer.to_radians().cos(), inner.to_radians().cos()),
                (false, Some((inner, outer))) => (outer.to_radians().cos(), -3.0-inner.to_radians().cos()),
                (false, None) => (l.spot.clamp(0.0,1.0), -1.0),
            };
            // A rectangle: the direction carries the normal scaled by the
            // half width, mode = -5 - half height, and the colour's w the
            // tangent's angle about the normal (see cluster_area).
            let (angular, mode, dir) = match l.area {
                Some(a) => (a.angle_about(dir), -5.0 - a.half_height, dir * a.half_width),
                None => (angular, mode, dir),
            };
            data[at..at + 12].copy_from_slice(&[
                l.pos.x,
                l.pos.y,
                l.pos.z,
                l.radius,
                l.color.x.max(0.0),
                l.color.y.max(0.0),
                l.color.z.max(0.0),
                angular,
                dir.x,
                dir.y,
                dir.z,
                mode,
            ]);
        }
    }

    pub fn update(
        &mut self,
        cx: &mut Cx,
        lights: &[LmLight],
        view: Option<(Mat4f, Mat4f)>,
    ) -> ClusterStats {
        let start = Cx::monotonic_now();
        self.build(lights, view);
        // An empty grid packs to zeros whatever the camera: the texture
        // already holds exactly that.
        let empty = self.lights.is_empty();
        if empty && self.uploaded_empty && self.texture.is_some() && self.light_texel == self.config.count() {
            self.stats.build_us = ((Cx::monotonic_now() - start) * 1_000_000.0) as u64;
            return self.stats;
        }
        self.uploaded_empty = empty;
        let old_height = self.texture_height;
        let mut data = self
            .texture
            .as_ref()
            .map(|t| t.take_vec_f32(cx))
            .unwrap_or_default();
        self.pack(&mut data);
        self.stats.upload_bytes = data.len() * 4;
        if self.texture.is_none() || old_height != self.texture_height {
            self.texture = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRGBAf32 {
                    width: TEXTURE_WIDTH,
                    height: self.texture_height,
                    data: Some(data),
                    updated: TextureUpdated::Full,
                },
            ));
        } else {
            self.texture
                .as_ref()
                .unwrap()
                .put_back_vec_f32(cx, data, None);
        }
        self.stats.build_us = ((Cx::monotonic_now() - start) * 1_000_000.0) as u64;
        let overflow = (self.stats.overflow_references, self.stats.rejected_lights);
        if overflow != self.last_overflow && start - self.last_overflow_log >= 1.0 {
            log!(
                "clustered lighting: {} overflow references, {} rejected/capped lights",
                overflow.0,
                overflow.1
            );
            self.last_overflow = overflow;
            self.last_overflow_log = start;
        }
        self.stats
    }

    /// Whether `bind` switches the clustered lights on (`cluster_on`).
    pub fn active(&self, enabled: bool) -> bool {
        enabled && self.texture.is_some()
    }

    /// Whether the lamps' shadow maps are on in a lane `bind` switched on.
    pub fn shadows_active(&self, enabled: bool) -> bool {
        self.active(enabled) && self.shadows.active()
    }

    pub fn bind(&self, cx: &Cx, vars: &mut DrawVars, enabled: bool) {
        let on = enabled && self.texture.is_some();
        vars.set_uniform(cx, live_id!(lin_ctl), &self.lin_ctl);
        vars.set_uniform(cx, live_id!(fog_ctl), &self.fog_ctl);
        vars.set_uniform(cx, live_id!(fog_eye), &self.fog_eye);
        vars.set_uniform(cx, live_id!(cluster_on), &[if on { 1.0 } else { 0.0 }]);
        if !on {
            return;
        }
        self.shadows.bind(cx,vars,self.texture.as_ref().unwrap());
        if let Some(id) = vars.draw_shader_id {
            if let Some(slot) = cx.draw_shaders[id.index]
                .mapping
                .textures
                .iter()
                .position(|t| t.id == live_id!(cluster_data))
            {
                vars.set_texture(slot, self.texture.as_ref().unwrap());
            }
        }
        for (id, r) in [
            (live_id!(cluster_x), self.rows[0]),
            (live_id!(cluster_y), self.rows[1]),
            (live_id!(cluster_w), self.rows[3]),
            (live_id!(cluster_d), self.depth_row),
        ] {
            vars.set_uniform(cx, id, &[r.x, r.y, r.z, r.w]);
        }
        let c = self.config;
        vars.set_uniform(
            cx,
            live_id!(cluster_grid),
            &[
                c.tiles_x as f32,
                c.tiles_y as f32,
                c.slices as f32,
                if self.world_grid { 2.0 } else { 1.0 },
            ],
        );
        vars.set_uniform(
            cx,
            live_id!(cluster_z),
            &[self.near, self.far, self.log_scale, self.light_texel as f32],
        );
        vars.set_uniform(
            cx,
            live_id!(cluster_tex),
            &[
                1.0 / TEXTURE_WIDTH as f32,
                1.0 / self.texture_height as f32,
                c.count() as f32,
                0.0,
            ],
        );
    }
}


script_mod! {
    use mod.prelude.widgets_internal.*
    // Spread AFTER each family's existing texture declarations to preserve
    // their binding ABI. Derived materials bind their own later textures.
    mod.draw.ClusteredLighting = {
        cluster_data: texture_2d(float),
        ..mod.draw.LocalShadowSampling
        cluster_on: uniform(0.0)
        // The lighting convention (Renderer::set_hdr_output): x = 1 for the
        // linear HDR lane (sRGB texels/colours decoded, no in-lane tone map),
        // y = exposure, z = 1/exposure, w = emission gain.
        lin_ctl: uniform(vec4(0.0, 1.0, 1.0, 1.0))
        // sRGB -> linear for the HDR lane (a close cubic fit of the exact
        // piecewise curve); the identity for the legacy display-space lane.
        to_lin: fn(c: vec3) -> vec3 {
            if self.lin_ctl.x < 0.5 { return c }
            return c * (c * (c * 0.305306011 + vec3(0.682171111, 0.682171111, 0.682171111)) + vec3(0.012522878, 0.012522878, 0.012522878))
        }
        // HDR lane fog (Renderer::set_hdr_output), evaluated per pixel.
        // fog_ctl.z picks the kind: 0 = exponential HEIGHT fog, fog_ctl =
        // (base height, 1/scale height, 0, on), the game's fog density
        // being the density AT the base height, so a view from above looks
        // through far less haze than one along the ground; 1 = linear with
        // distance, fog_ctl = (start, end, 1, on); 2 = exp2 with distance
        // (the density). fog_eye = the true world camera.
        fog_ctl: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        fog_eye: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        scene_fog: fn(legacy: float, wp: vec3, density: float) -> float {
            if self.lin_ctl.x < 0.5 || self.fog_ctl.w < 0.5 { return legacy }
            let d = wp - self.fog_eye.xyz
            if self.fog_ctl.z > 0.5 {
                let dist = length(d)
                if self.fog_ctl.z < 1.5 {
                    return clamp((dist - self.fog_ctl.x) / max(self.fog_ctl.y - self.fog_ctl.x, 0.001), 0.0, 1.0)
                }
                let kd = density * dist
                return 1.0 - exp(0.0 - kd * kd)
            }
            let k = self.fog_ctl.y
            // Optical depth of exp(-(h - h0) k) along the segment, written
            // with both exponents bounded (no overflow from high cameras).
            // (e^a - e^b)/dy with b = a - dy, written as e^a (1 - e^-dy)/dy:
            // the difference of two exponentials cancels catastrophically
            // on near-level rays under fast math (rings round the camera).
            let a = min((self.fog_ctl.x - self.fog_eye.y) * k, 30.0)
            let dy = d.y * k
            var slope = 1.0 - dy * 0.5 + dy * dy * 0.1666667
            if abs(dy) > 0.05 { slope = (1.0 - exp(0.0 - dy)) / dy }
            let layer = exp(a) * slope
            return 1.0 - exp(0.0 - length(d) * density * layer)
        }
        cluster_x: uniform(vec4(1.0,0.0,0.0,0.0))
        cluster_y: uniform(vec4(0.0,1.0,0.0,0.0))
        cluster_w: uniform(vec4(0.0,0.0,0.0,1.0))
        cluster_d: uniform(vec4(0.0,0.0,-1.0,0.0))
        cluster_grid: uniform(vec4(16.0,9.0,24.0,1.0))
        cluster_z: uniform(vec4(2.0,1000.0,1.0,0.0))
        cluster_tex: uniform(vec4(0.00390625,1.0,0.0,0.0))

        cluster_fetch: fn(at: float) -> vec4 {
            let row = floor(at * self.cluster_tex.x)
            let col = at - row / self.cluster_tex.x
            // Rows count from the top of the allocation size() reports,
            // which a backend may make taller than the rows uploaded.
            let size = self.cluster_data.size()
            return self.cluster_data.sample_nearest(vec2((col+0.5)/size.x,(row+0.5)/size.y))
        }
        // One light's contribution at `wp` (index into the light texels).
        // `view`, `f0` and `dalb` (the diffuse albedo over pi) and `pa`
        // (ndv, the geometry term's k, the view half of that term, the HDR
        // lobe scale) depend on the pixel only: cluster_lights computes them
        // once, not once per light.
        cluster_light: fn(index: float, wp: vec3, n: vec3, view: vec3, f0: vec3, dalb: vec3, a2: float, pa: vec4, pbr: float) -> vec3 {
            let pos=self.cluster_fetch(self.cluster_z.w+index*3.0)
            let delta=pos.xyz-wp
            let distance=length(delta)
            // The cluster is a conservative box: many of its lights
            // miss this pixel, and those need no colour or direction.
            if distance<pos.w {
                let color=self.cluster_fetch(self.cluster_z.w+index*3.0+1.0)
                let light=delta/max(distance,0.0001)
                let direction=self.cluster_fetch(self.cluster_z.w+index*3.0+2.0)
                if direction.w < -4.5 {
                    return self.cluster_area(pos,color,direction,wp,n,view,f0,dalb,a2,pa,pbr)
                }
                var attenuation=pow(max(1.0-distance/pos.w,0.0),2.0)
                var cone=1.0
                if direction.w>=0.0 || (direction.w < -1.5 && direction.w > -2.5) {
                    let ratio=distance/pos.w
                    let cutoff=max(1.0-ratio*ratio*ratio*ratio,0.0)
                    attenuation=cutoff*cutoff/max(distance*distance,0.0001)
                }
                if direction.w>=0.0 || direction.w < -2.5 {
                    var inner=direction.w
                    if direction.w < -2.5 { inner=-3.0-direction.w }
                    let cosine=dot(direction.xyz,light*(-1.0))
                    let angular=clamp((cosine-color.w)/max(inner-color.w,0.00001),0.0,1.0)
                    cone=angular*angular
                } else if direction.w > -1.5 && color.w>0.001 {
                    let cosine=max(dot(direction.xyz,light*(-1.0)),0.0)
                    cone=mix(1.0,cosine*cosine,color.w)
                }
                let ndl=max(dot(n,light),0.0)
                // Every term below carries ndl and attenuation*cone: a
                // light behind the surface or outside its cone adds
                // exactly zero, so it skips the BRDF and, above all, the
                // shadow lookup.
                if ndl>0.0 && attenuation*cone>0.0 {
                var response=vec3(ndl,ndl,ndl)
                if pbr>0.5 {
                    let halfdir=normalize(light+view)
                    let ndh=max(dot(n,halfdir),0.0)
                    let vdh=max(dot(view,halfdir),0.0)
                    let f=f0+(vec3(1.0,1.0,1.0)-f0)*pow(1.0-vdh,5.0)
                    let den=ndh*ndh*(a2-1.0)+1.0
                    let distribution=a2/max(3.14159265*den*den,0.000001)
                    let k=pa.y
                    let geometry=pa.z*(ndl/max(ndl*(1.0-k)+k,0.0001))
                    let spec=f*(distribution*geometry/max(4.0*pa.x*ndl,0.0001))
                    let diffuse=(vec3(1.0,1.0,1.0)-f)*dalb
                    // Light colours are irradiance/pi units; the HDR
                    // lane restores the pi the 1/pi BRDF divides out.
                    response=(diffuse+spec)*(ndl*pa.w)
                }
                // The material's `light` hook (render-material); the stock
                // body is radiance * response, this very product.
                return self.mat_light(color.xyz*(attenuation*cone*self.local_shadow_visibility(index,wp,n,pos.xyz,pos.w)),light,n,view,response)
                }
            }
            return vec3(0.0,0.0,0.0)
        }
        // A rectangular area light (lightmap.rs AreaRect): diffuse from the
        // point of the rectangle nearest the surface, specular from the
        // point nearest the reflected ray with the lobe widened by the
        // rectangle's angular size (the representative-point method), a
        // one-sided emitter, and the same finite range as every lamp.
        cluster_area: fn(pos: vec4, color: vec4, direction: vec4, wp: vec3, n: vec3, view: vec3, f0: vec3, dalb: vec3, a2: float, pa: vec4, pbr: float) -> vec3 {
            let hw=length(direction.xyz)
            let hh=-5.0-direction.w
            let rn=direction.xyz/max(hw,0.000001)
            var helper=vec3(1.0,0.0,0.0)
            if abs(rn.y)<0.9 { helper=vec3(0.0,1.0,0.0) }
            let b0=normalize(cross(helper,rn))
            let b1=cross(rn,b0)
            let t=b0*cos(color.w)+b1*sin(color.w)
            let b=cross(rn,t)
            let d0=wp-pos.xyz
            // Behind the emitting side: no light.
            if dot(d0,rn)<=0.0 { return vec3(0.0,0.0,0.0) }
            let q=pos.xyz+t*clamp(dot(d0,t),0.0-hw,hw)+b*clamp(dot(d0,b),0.0-hh,hh)
            let dl=q-wp
            let dist=max(length(dl),0.0001)
            let light=dl/dist
            let ndl=max(dot(n,light),0.0)
            let area=4.0*hw*hh
            let ratio=dist/pos.w
            let window=max(1.0-ratio*ratio*ratio*ratio,0.0)
            let irr=max(dot(rn,light*(-1.0)),0.0)*area/(dist*dist+area)*window*window
            if ndl<=0.0 || irr<=0.0 { return vec3(0.0,0.0,0.0) }
            var response=vec3(ndl,ndl,ndl)
            if pbr>0.5 {
                let r=n*(2.0*dot(n,view))-view
                let denom=dot(r,rn)
                var sp=q
                if denom < -0.0001 {
                    let hit=wp+r*(dot(pos.xyz-wp,rn)/denom)
                    let hd=hit-pos.xyz
                    sp=pos.xyz+t*clamp(dot(hd,t),0.0-hw,hw)+b*clamp(dot(hd,b),0.0-hh,hh)
                }
                let sl=normalize(sp-wp)
                let sdist=max(length(sp-wp),0.0001)
                let alpha=sqrt(a2)
                let alpha2=clamp(alpha+max(hw,hh)/(2.0*sdist),0.0,1.0)
                let norm=(alpha/max(alpha2,0.0001))*(alpha/max(alpha2,0.0001))
                let halfdir=normalize(sl+view)
                let ndh=max(dot(n,halfdir),0.0)
                let vdh=max(dot(view,halfdir),0.0)
                let sndl=max(dot(n,sl),0.0)
                let f=f0+(vec3(1.0,1.0,1.0)-f0)*pow(1.0-vdh,5.0)
                let den=ndh*ndh*(a2-1.0)+1.0
                let distribution=a2/max(3.14159265*den*den,0.000001)
                let k=pa.y
                let geometry=pa.z*(sndl/max(sndl*(1.0-k)+k,0.0001))
                let spec=f*(distribution*geometry*norm/max(4.0*pa.x*sndl,0.0001))*sndl
                response=((vec3(1.0,1.0,1.0)-f)*dalb*ndl+spec)*pa.w
            }
            return self.mat_light(color.xyz*irr,light,n,view,response)
        }
        // One light's contribution: the material `light` hook's stock body.
        // `radiance` arrives with falloff, cone and shadow applied; `brdf`
        // is the lane's response per unit radiance (N.L included).
        mat_light: fn(radiance: vec3, l: vec3, n: vec3, v: vec3, brdf: vec3) -> vec3 {
            return radiance*brdf
        }
        cluster_lights: fn(wp: vec3, normal: vec3, eye: vec3, albedo: vec3, roughness: float, metallic: float, pbr: float) -> vec3 {
            if self.cluster_on < 0.5 { return vec3(0.0,0.0,0.0) }
            let p=vec4(wp.x,wp.y,wp.z,1.0)
            let w=dot(self.cluster_w,p)
            if w <= 0.0 { return vec3(0.0,0.0,0.0) }
            let u=dot(self.cluster_x,p)/w*0.5+0.5
            let v=dot(self.cluster_y,p)/w*0.5+0.5
            let depth=dot(self.cluster_d,p)
            if u<0.0 || u>1.0 || v<0.0 || v>1.0 || depth<0.0 { return vec3(0.0,0.0,0.0) }
            var z=0.0
            if self.cluster_grid.w>1.5 {
                if depth>1.0 { return vec3(0.0,0.0,0.0) }
                z=floor(depth*self.cluster_grid.z)
            } else {
                if depth>self.cluster_z.y { return vec3(0.0,0.0,0.0) }
                if depth>=self.cluster_z.x { z=1.0+floor(log2(depth/self.cluster_z.x)*self.cluster_z.z) }
            }
            let x=min(floor(u*self.cluster_grid.x),self.cluster_grid.x-1.0)
            let y=min(floor(v*self.cluster_grid.y),self.cluster_grid.y-1.0)
            z=clamp(z,0.0,self.cluster_grid.z-1.0)
            let header=self.cluster_fetch(x+self.cluster_grid.x*(y+self.cluster_grid.y*z))
            // Most pixels sit in clusters no light reaches.
            if header.y < 0.5 { return vec3(0.0,0.0,0.0) }
            let n=normalize(normal)
            // The BRDF's per-pixel half, hoisted out of the light loop.
            let view=normalize(eye-wp)
            let ndv=max(dot(n,view),0.0001)
            let rough=clamp(roughness,0.045,1.0)
            let metal=clamp(metallic,0.0,1.0)
            let f0=mix(vec3(0.04,0.04,0.04),albedo,metal)
            let a2=rough*rough*rough*rough
            let k=(rough+1.0)*(rough+1.0)*0.125
            let pa=vec4(ndv,k,ndv/max(ndv*(1.0-k)+k,0.0001),mix(1.0,3.14159265,self.lin_ctl.x))
            let dalb=albedo*((1.0-metal)/3.14159265)
            var total=vec3(0.0,0.0,0.0)
            var i=0.0
            // Four light indices per texel: fetched once per four lights.
            var packed=vec4(0.0,0.0,0.0,0.0)
            while i<min(header.y,64.0) {
                let address=header.x+i
                let lane=address-floor(address*0.25)*4.0
                if i<0.5 || lane<0.5 {
                    packed=self.cluster_fetch(self.cluster_tex.z+floor(address*0.25))
                }
                var index=packed.x
                if lane>0.5 { index=packed.y }
                if lane>1.5 { index=packed.z }
                if lane>2.5 { index=packed.w }
                total=total+self.cluster_light(index,wp,n,view,f0,dalb,a2,pa,pbr)
                i=i+1.0
            }
            return total
        }
        cluster_sum: fn(wp: vec3, normal: vec3) -> vec3 {
            return self.cluster_lights(wp,normal,vec3(0.0,0.0,0.0),vec3(1.0,1.0,1.0),1.0,0.0,0.0)
        }
        cluster_pbr: fn(wp: vec3, normal: vec3, eye: vec3, albedo: vec3, roughness: float, metallic: float) -> vec3 {
            return self.cluster_lights(wp,normal,eye,albedo,roughness,metallic,1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn light(p: Vec3f, r: f32) -> LmLight {
        LmLight::omni(p, vec3f(0.1, 0.2, 0.3), r)
    }
    fn camera() -> Option<(Mat4f, Mat4f)> {
        Some((Mat4f::identity(), Mat4f::perspective(70.0, 1.5, 0.1, 200.0)))
    }
    fn contains(c: &ClusteredLights, p: Vec3f, light: usize) -> bool {
        let Some(i) = c.cluster_at(p) else {
            return false;
        };
        c.indices[i * c.config.lights_per_cluster..i * c.config.lights_per_cluster + c.counts[i]]
            .contains(&light)
    }
    #[test]
    fn spot_cones_cull_clusters_behind_and_beside_them() {
        let b = |c: Vec3f| Bounds { min: c - vec3f(0.5, 0.5, 0.5), max: c + vec3f(0.5, 0.5, 0.5) };
        let (sin, cos) = 30.0f32.to_radians().sin_cos();
        let down = vec3f(0.0, -1.0, 0.0);
        let apex = vec3f(0.0, 10.0, 0.0);
        assert!(!outside_cone(b(vec3f(0.0, 0.0, 0.0)), apex, down, sin, cos), "on the axis");
        assert!(!outside_cone(b(vec3f(5.0, 0.0, 0.0)), apex, down, sin, cos), "inside the cone (26.6 deg)");
        assert!(outside_cone(b(vec3f(12.0, 0.0, 0.0)), apex, down, sin, cos), "beside it (50 deg)");
        assert!(outside_cone(b(vec3f(0.0, 14.0, 0.0)), apex, down, sin, cos), "behind the lamp");
        // A box straddling the edge is kept.
        assert!(!outside_cone(Bounds { min: vec3f(4.0, -1.0, -1.0), max: vec3f(8.0, 1.0, 1.0) }, apex, down, sin, cos));
    }
    #[test]
    fn more_than_eight_lights_reach_one_surface() {
        let mut c = ClusteredLights::default();
        c.build(&vec![light(vec3f(0.0, 1.0, -8.0), 5.0); 20], camera());
        for i in 0..20 {
            assert!(contains(&c, vec3f(0.0, 0.0, -8.0), i));
        }
        assert_eq!(c.stats.overflow_references, 0);
    }
    #[test]
    fn depth_slices_separate_near_and_far_lights() {
        let mut c = ClusteredLights::default();
        c.build(
            &[
                light(vec3f(0.0, 0.0, -4.0), 1.0),
                light(vec3f(0.0, 0.0, -80.0), 1.0),
            ],
            camera(),
        );
        assert!(contains(&c, vec3f(0.0, 0.0, -4.0), 0));
        assert!(!contains(&c, vec3f(0.0, 0.0, -4.0), 1));
        assert!(contains(&c, vec3f(0.0, 0.0, -80.0), 1));
    }
    #[test]
    fn sphere_coverage_is_conservative_across_tile_and_depth_boundaries() {
        let mut c = ClusteredLights::default();
        let lights = [
            light(vec3f(0.0, 0.0, -0.5), 3.0),
            light(vec3f(4.0, 2.0, -12.0), 7.0),
            light(vec3f(-10.0, 4.0, -50.0), 20.0),
        ];
        c.build(&lights, camera());
        for (i, l) in lights.iter().enumerate() {
            for x in -6..=6 {
                for y in -6..=6 {
                    for z in -6..=6 {
                        let offset = vec3f(x as f32, y as f32, z as f32) * (l.radius / 6.0);
                        let p = l.pos + offset;
                        if offset.length() < l.radius && c.cluster_at(p).is_some() {
                            assert!(contains(&c, p, i), "missing light {i} at {p:?}");
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn xr_world_grid_covers_both_eyes_and_lights_behind_flat_camera() {
        let mut c = ClusteredLights::default();
        let lights = [
            light(vec3f(0.0, 0.0, 8.0), 4.0),
            light(vec3f(-8.0, 12.0, -3.0), 6.0),
        ];
        c.build(&lights, None);
        for (i, l) in lights.iter().enumerate() {
            for dx in [-0.04, 0.04] {
                assert!(contains(&c, l.pos + vec3f(dx, 0.0, 0.0), i));
            }
        }
        assert!(c.stats.world_grid);
    }
    #[test]
    fn overflow_is_bounded_reported_and_prefers_stronger_lights() {
        let mut c = ClusteredLights::default();
        c.set_config(ClusterConfig {
            lights_per_cluster: 8,
            ..Default::default()
        });
        let mut lights = vec![light(vec3f(0.0, 0.0, -8.0), 5.0); 20];
        lights[19].color = vec3f(5.0, 5.0, 5.0);
        c.build(&lights, camera());
        assert!(c.stats.overflow_references > 0);
        assert_eq!(c.stats.max_lights_in_cluster, 8);
        assert!(contains(&c, vec3f(0.0, 0.0, -8.0), 19));
    }
    #[test]
    fn empty_frame_removes_old_lights_and_packed_references_are_valid() {
        let mut c = ClusteredLights::default();
        c.build(&[light(vec3f(0.0, 0.0, -8.0), 5.0)], camera());
        let mut data = Vec::new();
        c.pack(&mut data);
        for i in 0..c.config.count() {
            let start = data[i * 4] as usize;
            let count = data[i * 4 + 1] as usize;
            assert!(start + count <= c.stats.references);
            for j in start..start + count {
                assert!((data[c.config.count() * 4 + j] as usize) < c.lights.len());
            }
        }
        c.build(&[], camera());
        c.pack(&mut data);
        assert_eq!(c.stats.references, 0);
        assert_eq!(c.stats.lights, 0);
        assert!(c.counts.iter().all(|&n| n == 0));
    }
    #[test]
    fn invalid_lights_are_rejected_and_config_is_clamped() {
        let mut c = ClusteredLights::default();
        c.set_config(ClusterConfig {
            tiles_x: 0,
            tiles_y: 0,
            slices: 0,
            max_lights: 0,
            lights_per_cluster: usize::MAX,
        });
        c.build(
            &[
                light(vec3f(f32::NAN, 0.0, 0.0), 2.0),
                light(vec3f(0.0, 0.0, -5.0), -1.0),
            ],
            None,
        );
        assert_eq!(c.stats.rejected_lights, 2);
        assert_eq!(c.stats.references, 0);
        assert_eq!(c.config.lights_per_cluster, MAX_CLUSTER_LIGHTS);
    }

    #[test]
    fn transformed_and_orthographic_views_keep_sphere_coverage() {
        let view = Mat4f::look_at(
            vec3f(18.0, 11.0, 20.0),
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.0, 1.0, 0.0),
        );
        for projection in [
            camera().unwrap().1,
            Mat4f::ortho(-30.0, 30.0, 20.0, -20.0, 0.1, 200.0, 1.0, 1.0),
        ] {
            let mut c = ClusteredLights::default();
            let lights = [
                light(vec3f(0.0, 0.0, 0.0), 8.0),
                light(vec3f(-9.0, 4.0, 2.0), 5.0),
            ];
            c.build(&lights, Some((view, projection)));
            for (i, l) in lights.iter().enumerate() {
                for x in -4..=4 {
                    for y in -4..=4 {
                        for z in -4..=4 {
                            let offset = vec3f(x as f32, y as f32, z as f32) * (l.radius / 4.0);
                            let p = l.pos + offset;
                            if offset.length() < l.radius && c.cluster_at(p).is_some() {
                                assert!(
                                    contains(&c, p, i),
                                    "missing transformed light {i} at {p:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn tiny_world_grid_lights_have_finite_lookup() {
        let mut c = ClusteredLights::default();
        let p = vec3f(100000.0, 100000.0, 100000.0);
        c.build(&[light(p, 0.00001)], None);
        assert!(contains(&c, p, 0));
        assert!(c
            .rows
            .iter()
            .all(|r| r.x.is_finite() && r.y.is_finite() && r.z.is_finite() && r.w.is_finite()));
    }

    #[test]
    #[ignore = "release-only CPU build+pack benchmark; not a GPU or Quest measurement"]
    fn benchmark_build_and_pack() {
        for count in [64, 256, 1024] {
            let side = (count as f32).sqrt().ceil() as usize;
            let lights: Vec<_> = (0..count)
                .map(|i| {
                    light(
                        vec3f(
                            (i % side) as f32 * 3.0 - 30.0,
                            2.5,
                            -4.0 - (i / side) as f32 * 3.0,
                        ),
                        5.0,
                    )
                })
                .collect();
            for view in [camera(), None] {
                let mut c = ClusteredLights::default();
                let mut data = Vec::new();
                c.build(&lights, view);
                c.pack(&mut data);
                let start = std::time::Instant::now();
                for _ in 0..200 {
                    c.build(std::hint::black_box(&lights), view);
                    c.pack(std::hint::black_box(&mut data));
                }
                eprintln!("cluster bench lights={count} world_grid={} mean_us={:.1} refs={} overflow={} upload_bytes={}",view.is_none(),start.elapsed().as_secs_f64()*1e6/200.0,c.stats.references,c.stats.overflow_references,data.len()*4);
            }
        }
    }
}
