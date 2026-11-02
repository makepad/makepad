//! The scene GI rays trace: a camera-centred voxel clipmap of the static
//! world, one level per probe cascade (level k voxels are cascade k's probe
//! spacing / 4). It is built and kept current on a pool worker:
//!
//! - a snapshot is a list of instances that SHARE their meshes (`Arc`), so a
//!   scene edit re-sends transforms, not vertex data;
//! - an edit is a multiset diff of instance keys: only the bricks (8³
//!   voxels) under instances that appeared, vanished or changed are
//!   re-voxelized, and their world boxes go back to invalidate probes;
//! - a level window scrolls in whole bricks by toroidal addressing: only the
//!   bricks it newly covers are voxelized.
//!
//! There is no triangle, instance or byte cap: the voxel memory is fixed by
//! the clipmap, and a bigger scene only makes the worker slower. The worker
//! owns a CPU mirror of the three voxel images; the UI thread copies the
//! changed rows of a finished job into the GPU textures.
use super::*;
use std::collections::HashMap;

/// Voxels per brick edge: the unit of scrolling and of edit invalidation.
pub(crate) const BRICK: usize = 8;
/// z slices laid side by side per texture row of a level.
pub(crate) const TILE_COLUMNS: usize = 8;

/// A render mesh copied once and shared by every instance that draws it.
#[derive(Clone)]
pub struct Mesh {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub stride: usize,
    /// -1 uniform white; 5 packed model colour (uv pair at 4); 8 unpacked rgb.
    pub color_lane: i32,
    /// A small copy of the albedo texture (BGRA u32), sampled per vertex.
    pub image: Option<(usize, usize, Vec<u32>)>,
    pub lo: Vec3f,
    pub hi: Vec3f,
    /// Content hash: a regenerated but identical mesh is not an edit.
    pub hash: u64,
}

impl Mesh {
    pub fn new(vertices: Vec<f32>, indices: Vec<u32>, stride: usize, color_lane: i32, image: Option<(usize, usize, Vec<u32>)>) -> Option<Self> {
        let lane_ok = match color_lane { -1 => true, 5 => stride >= 6, l => l >= 0 && stride >= l as usize + 3 };
        if stride < 3 || !lane_ok { return None; }
        let count = vertices.len() / stride;
        if indices.iter().any(|&i| i as usize >= count) { return None; }
        let mut lo = vec3f(f32::MAX, f32::MAX, f32::MAX);
        let mut hi = lo * -1.0;
        for v in vertices.chunks_exact(stride) {
            if !(v[0].is_finite() && v[1].is_finite() && v[2].is_finite()) { return None; }
            lo = vec3f(lo.x.min(v[0]), lo.y.min(v[1]), lo.z.min(v[2]));
            hi = vec3f(hi.x.max(v[0]), hi.y.max(v[1]), hi.z.max(v[2]));
        }
        if count == 0 { lo = Vec3f::default(); hi = lo; }
        let mut h = 0xcbf2_9ce4_8422_2325u64 ^ ((stride as u64) << 8) ^ (color_lane as u64);
        for v in &vertices { h = (h ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3); }
        for i in &indices { h = (h ^ *i as u64).wrapping_mul(0x100_0000_01b3); }
        if let Some((w, hh, px)) = &image { h ^= (*w as u64) << 32 ^ *hh as u64; for p in px { h = (h ^ *p as u64).wrapping_mul(0x100_0000_01b3); } }
        Some(Self { vertices, indices, stride, color_lane, image, lo, hi, hash: h })
    }

    /// Authored (sRGB-encoded) vertex albedo; the shader decodes it when the
    /// host lights in linear space.
    fn vertex_color(&self, v: &[f32]) -> Vec3f {
        match self.color_lane {
            5 => {
                let bits = v[5].to_bits();
                let mut c = vec3f((bits & 255) as f32, ((bits >> 8) & 255) as f32, ((bits >> 16) & 255) as f32) * (1.0 / 255.0);
                if let Some((w, h, pixels)) = &self.image {
                    if *w > 0 && *h > 0 {
                        let (u, t) = makepad_draw::vector::unpack_pair_f16(v[4]);
                        let x = (u.rem_euclid(1.0) * *w as f32) as usize;
                        let y = (t.rem_euclid(1.0) * *h as f32) as usize;
                        if let Some(&px) = pixels.get(y.min(h - 1) * w + x.min(w - 1)) {
                            c = mul(c, vec3f(((px >> 16) & 255) as f32, ((px >> 8) & 255) as f32, (px & 255) as f32) * (1.0 / 255.0));
                        }
                    }
                }
                c
            }
            l if l >= 0 => { let j = l as usize; vec3f(v[j], v[j + 1], v[j + 2]) }
            _ => vec3f(1.0, 1.0, 1.0),
        }
    }
}

/// Downsample a texture for GI albedo: GI reads one colour per vertex, so a
/// strided copy of at most `MAX_IMAGE`² texels is plenty and bounds memory.
pub(crate) fn gi_image(width: usize, height: usize, data: &[u32]) -> Option<(usize, usize, Vec<u32>)> {
    const MAX_IMAGE: usize = 128;
    if width == 0 || height == 0 || data.len() < width * height { return None; }
    let (w, h) = (width.min(MAX_IMAGE), height.min(MAX_IMAGE));
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h { for x in 0..w { out.push(data[(y * height / h) * width + x * width / w]); } }
    Some((w, h, out))
}

/// A solid given by its signed distance instead of triangles, voxelized on
/// the worker where it is asked. Distances are 1-Lipschitz, in the object
/// frame, whose `transform` scales uniformly.
pub trait GiField: Send + Sync {
    /// The solid's box in the object frame.
    fn bounds(&self) -> (Vec3f, Vec3f);
    /// Identity: equal keys, equal fields.
    fn key(&self) -> u64;
    /// The distance and material of each point; every point lies inside
    /// `region` (object frame), and surface farther than a margin from it
    /// may be left out. `f32::MAX` where nothing is near.
    fn sample(&self, region: (Vec3f, Vec3f), points: &[[f32; 3]], out: &mut Vec<(f32, u8)>);
    /// A material's (albedo, encoded like mesh colours; emission; diffuse share).
    fn material(&self, slot: u8) -> (Vec3f, Vec3f, f32);
}

/// What an instance is made of.
#[derive(Clone)]
pub(crate) enum Solid {
    Mesh(Arc<Mesh>),
    Field(Arc<dyn GiField>),
}

/// One solid of a GI scene: the renderer's own, or one a host passes
/// (`Renderer::set_gi_scene`).
#[derive(Clone)]
pub struct Instance {
    pub(crate) solid: Solid,
    pub transform: Mat4f,
    pub tint: Vec3f,
    pub emission: Vec3f,
    pub diffuse: f32,
    /// World bounds.
    pub lo: Vec3f,
    pub hi: Vec3f,
    /// Identity for the edit diff: mesh, transform and material.
    pub key: u64,
}

impl Instance {
    pub fn new(mesh: Arc<Mesh>, transform: Mat4f, tint: Vec3f, emission: Vec3f, diffuse: f32) -> Option<Self> {
        let (lo, hi, hash) = (mesh.lo, mesh.hi, mesh.hash);
        Self::of(Solid::Mesh(mesh), lo, hi, hash, transform, tint, emission, diffuse)
    }

    /// A field placed by `transform`; its materials give its colours.
    pub fn field(field: Arc<dyn GiField>, transform: Mat4f) -> Option<Self> {
        let (lo, hi) = field.bounds();
        let key = field.key();
        Self::of(Solid::Field(field), lo, hi, key, transform, vec3f(1.0, 1.0, 1.0), Vec3f::default(), 1.0)
    }

    #[allow(clippy::too_many_arguments)]
    fn of(solid: Solid, box_lo: Vec3f, box_hi: Vec3f, identity: u64, transform: Mat4f, tint: Vec3f, emission: Vec3f, diffuse: f32) -> Option<Self> {
        if !transform.v.iter().all(|v| v.is_finite()) || !finite(tint) || !finite(emission) || !diffuse.is_finite() { return None; }
        let (mut lo, mut hi) = (vec3f(f32::MAX, f32::MAX, f32::MAX), vec3f(f32::MIN, f32::MIN, f32::MIN));
        for z in [box_lo.z, box_hi.z] { for y in [box_lo.y, box_hi.y] { for x in [box_lo.x, box_hi.x] {
            let p = point(&transform, vec3f(x, y, z));
            lo = vec3f(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = vec3f(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }}}
        let mut h = 0xcbf2_9ce4_8422_2325u64 ^ identity;
        let mut mix = |v: f32| { h = (h ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3); };
        transform.v.iter().for_each(|v| mix(*v));
        [tint.x, tint.y, tint.z, emission.x, emission.y, emission.z, diffuse].into_iter().for_each(&mut mix);
        Some(Self { solid, transform, tint, emission, diffuse, lo, hi, key: h })
    }

    pub(crate) fn triangles(&self) -> usize {
        match &self.solid { Solid::Mesh(m) => m.indices.len() / 3, Solid::Field(_) => 0 }
    }
}

/// One scene state for the worker, limited to `region` around the camera.
pub(crate) struct Snapshot {
    pub instances: Vec<Instance>,
    pub triangles: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VoxelLayout {
    pub dims: [usize; 3],
    pub levels: usize,
    /// Level-0 voxel edge in metres.
    pub voxel: f32,
    pub home: [f32; 3],
}

impl VoxelLayout {
    pub fn new(c: GiConfig) -> Self {
        let c = c.clamped();
        Self { dims: c.grid.map(|g| g * 4), levels: c.cascades, voxel: c.spacing * 0.25, home: c.home }
    }
    pub fn voxel_size(&self, level: usize) -> f32 { self.voxel * (1u32 << level) as f32 }
    pub fn tile_rows(&self) -> usize { self.dims[2].div_ceil(TILE_COLUMNS) }
    pub fn width(&self) -> usize { self.dims[0] * TILE_COLUMNS }
    pub fn level_height(&self) -> usize { self.dims[1] * self.tile_rows() }
    pub fn height(&self) -> usize { self.level_height() * self.levels }
    pub fn level_voxels(&self) -> usize { self.dims.iter().product() }
    /// Texel of world voxel `c`: toroidal in all three axes.
    pub fn texel(&self, level: usize, c: [i32; 3]) -> usize {
        let m = [0, 1, 2].map(|a| c[a].rem_euclid(self.dims[a] as i32) as usize);
        let x = m[0] + self.dims[0] * (m[2] % TILE_COLUMNS);
        let y = m[1] + self.dims[1] * (m[2] / TILE_COLUMNS) + level * self.level_height();
        y * self.width() + x
    }
    /// The brick-aligned window (min voxel) centred on `center`.
    pub fn window_for(&self, level: usize, center: Vec3f) -> [i32; 3] {
        let v = self.voxel_size(level);
        let c = [center.x, center.y, center.z];
        [0, 1, 2].map(|a| ((c[a] / v - self.dims[a] as f32 * 0.5) / BRICK as f32 + 0.5).floor() as i32 * BRICK as i32)
    }
    pub fn window_box(&self, level: usize, origin: [i32; 3]) -> (Vec3f, Vec3f) {
        let v = self.voxel_size(level);
        let lo = vec3f(origin[0] as f32, origin[1] as f32, origin[2] as f32) * v;
        (lo, lo + vec3f(self.dims[0] as f32, self.dims[1] as f32, self.dims[2] as f32) * v)
    }
}

#[derive(Clone, Copy, Default)]
struct Accum { color: Vec3f, normal: Vec3f, emission: Vec3f, weight: f32 }

/// What a finished job changed, for the UI thread to upload and to
/// invalidate probes with.
#[derive(Default)]
pub(crate) struct VoxelUpdate {
    /// Per level: whether its texture rows changed.
    pub levels: Vec<bool>,
    /// Per level: the first and last changed row of its band.
    pub rows: Vec<Option<(usize, usize)>>,
    /// Per level whose voxels changed, in settle mode: the probes a surface
    /// can read (window-local cells).
    pub needed: Vec<Option<Vec<bool>>>,
    /// World boxes of instances that changed (edit invalidation).
    pub dirty: Vec<(Vec3f, Vec3f)>,
    pub bricks: usize,
    pub triangles: usize,
    pub ms: f64,
}

pub(crate) struct VoxelWorld {
    pub layout: VoxelLayout,
    /// Per level: the window whose voxels the mirror holds.
    pub windows: Vec<Option<[i32; 3]>>,
    scene: Option<Arc<Snapshot>>,
    keys: HashMap<u64, u32>,
    pub albedo: Vec<u32>,
    pub normal: Vec<u32>,
    pub emission: Vec<u32>,
    accum: Vec<Accum>,
    /// Settle mode: report which probes a surface can read.
    pub track_needed: bool,
}

/// Rows of the `level` image band, as (first texel, texel count).
#[cfg(test)]
pub(crate) fn level_span(layout: &VoxelLayout, level: usize) -> (usize, usize) {
    let n = layout.width() * layout.level_height();
    (level * n, n)
}

fn key_counts(s: &Snapshot) -> HashMap<u64, u32> {
    let mut m = HashMap::with_capacity(s.instances.len());
    for i in &s.instances { *m.entry(i.key).or_insert(0) += 1; }
    m
}

impl VoxelWorld {
    pub fn new(layout: VoxelLayout) -> Self {
        let n = layout.width() * layout.height();
        Self { layout, windows: vec![None; layout.levels], scene: None, keys: HashMap::new(),
            albedo: vec![0; n], normal: vec![0; n], emission: vec![0; n], accum: Vec::new(), track_needed: false }
    }

    /// Bring the mirror to `scene` (None = unchanged) inside `windows`.
    pub fn update(&mut self, scene: Option<Arc<Snapshot>>, windows: &[[i32; 3]]) -> VoxelUpdate {
        let start = Cx::monotonic_now();
        let mut out = VoxelUpdate { levels: vec![false; self.layout.levels], rows: vec![None; self.layout.levels], needed: vec![None; self.layout.levels], ..Default::default() };
        if let Some(new) = scene {
            // Multiset diff: an instance present in both (same key) is
            // untouched, whatever its position in the list.
            let new_keys = key_counts(&new);
            if let Some(old) = &self.scene {
                let mut remaining = new_keys.clone();
                for i in &old.instances {
                    match remaining.get_mut(&i.key) { Some(n) if *n > 0 => *n -= 1, _ => out.dirty.push((i.lo, i.hi)) }
                }
                let mut remaining = self.keys.clone();
                for i in &new.instances {
                    match remaining.get_mut(&i.key) { Some(n) if *n > 0 => *n -= 1, _ => out.dirty.push((i.lo, i.hi)) }
                }
            }
            self.keys = new_keys;
            self.scene = Some(new);
        }
        let b = self.layout.dims.map(|d| d / BRICK);
        for level in 0..self.layout.levels.min(windows.len()) {
            let win = windows[level];
            let mut flags = vec![false; b[0] * b[1] * b[2]];
            let mut any = false;
            let old = self.windows[level];
            for z in 0..b[2] { for y in 0..b[1] { for x in 0..b[0] {
                let brick = [x, y, z];
                let world = [0, 1, 2].map(|a| win[a] + (brick[a] * BRICK) as i32);
                let covered = old.is_some_and(|o| (0..3).all(|a| world[a] >= o[a] && world[a] < o[a] + self.layout.dims[a] as i32));
                if !covered { flags[x + b[0] * (y + b[1] * z)] = true; any = true; }
            }}}
            let v = self.layout.voxel_size(level);
            for (lo, hi) in &out.dirty {
                let a = [lo.x, lo.y, lo.z]; let c = [hi.x, hi.y, hi.z];
                let range = [0, 1, 2].map(|k| {
                    let l = ((a[k] / v).floor() as i32 - win[k]).div_euclid(BRICK as i32).max(0);
                    let h = ((c[k] / v).floor() as i32 - win[k]).div_euclid(BRICK as i32).min(b[k] as i32 - 1);
                    (l, h)
                });
                for z in range[2].0..=range[2].1 { for y in range[1].0..=range[1].1 { for x in range[0].0..=range[0].1 {
                    flags[x as usize + b[0] * (y as usize + b[1] * z as usize)] = true; any = true;
                }}}
            }
            if any {
                let (bricks, tris) = self.voxelize(level, win, &flags);
                out.bricks += bricks; out.triangles += tris;
                out.levels[level] = true;
                out.rows[level] = self.flagged_rows(win, &flags);
                if self.track_needed { out.needed[level] = Some(self.needed_probes(level, win)); }
            }
            self.windows[level] = Some(win);
        }
        out.ms = (Cx::monotonic_now() - start) * 1e3;
        out
    }

    /// The rows of a level's band the flagged bricks write. A brick's voxels
    /// land on the rows of its z tiles and its y span, toroidally.
    fn flagged_rows(&self, win: [i32; 3], flags: &[bool]) -> Option<(usize, usize)> {
        let d = self.layout.dims;
        let b = d.map(|d| d / BRICK);
        let (mut first, mut last) = (usize::MAX, 0);
        for bz in 0..b[2] { for by in 0..b[1] { for bx in 0..b[0] {
            if !flags[bx + b[0] * (by + b[1] * bz)] { continue; }
            let m = [1, 2].map(|a| (win[a] + ([bx, by, bz][a] * BRICK) as i32).rem_euclid(d[a] as i32) as usize);
            let (tile_lo, tile_hi) = (m[1] / TILE_COLUMNS, (m[1] + BRICK - 1) / TILE_COLUMNS);
            first = first.min(m[0] + d[1] * tile_lo);
            last = last.max(m[0] + BRICK - 1 + d[1] * tile_hi);
        }}}
        (first != usize::MAX).then_some((first, last))
    }

    /// The probes of a level's window whose cell a surface voxel touches
    /// (the receiver's air-side offset and the voxel's half size around it):
    /// the only ones a receiver, or a bounce through the field, reads.
    fn needed_probes(&self, level: usize, win: [i32; 3]) -> Vec<bool> {
        let d = self.layout.dims;
        let g = d.map(|v| v / 4);
        let org = win.map(|v| v.div_euclid(4));
        let mut out = vec![false; g[0] * g[1] * g[2]];
        for z in 0..d[2] as i32 { for y in 0..d[1] as i32 { for x in 0..d[0] as i32 {
            let c = [win[0] + x, win[1] + y, win[2] + z];
            if self.albedo[self.layout.texel(level, c)] >> 24 == 0 { continue; }
            let r = [0, 1, 2].map(|a| {
                let l = (c[a] as f32 + 0.5) * 0.25 - self.layout.home[a];
                let lo = ((l - 0.35).floor() as i32 - org[a]).max(0);
                let hi = ((l + 0.35).floor() as i32 + 1 - org[a]).min(g[a] as i32 - 1);
                (lo, hi)
            });
            for k in r[2].0..=r[2].1 { for j in r[1].0..=r[1].1 { for i in r[0].0..=r[0].1 {
                out[i as usize + g[0] * (j as usize + g[1] * k as usize)] = true;
            }}}
        }}}
        out
    }

    /// Adds a field's voxels in the flagged bricks. A brick its surface is
    /// far from costs one sample; one inside the solid fills at once.
    fn voxelize_field(&mut self, inst: &Instance, field: &Arc<dyn GiField>, level: usize, win: [i32; 3], flags: &[bool]) {
        let layout = self.layout;
        let d = layout.dims;
        let b = d.map(|d| d / BRICK);
        let v = layout.voxel_size(level);
        let obj = inst.transform.invert();
        let m = &inst.transform.v;
        let scale = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt().max(1e-12);
        // World half diagonals of a voxel and a brick.
        let hv = v * 0.87;
        let hb = hv * BRICK as f32;
        let (cover_lo, cover_hi) = (inst.lo - vec3f(hb, hb, hb), inst.hi + vec3f(hb, hb, hb));
        let voxel_world = |x: i32, y: i32, z: i32| vec3f((win[0] + x) as f32 + 0.5, (win[1] + y) as f32 + 0.5, (win[2] + z) as f32 + 0.5) * v;
        let mut samples = Vec::with_capacity(BRICK * BRICK * BRICK);
        let mut points = Vec::with_capacity(BRICK * BRICK * BRICK);
        for bz in 0..b[2] { for by in 0..b[1] { for bx in 0..b[0] {
            if !flags[bx + b[0] * (by + b[1] * bz)] { continue; }
            let o = [(bx * BRICK) as i32, (by * BRICK) as i32, (bz * BRICK) as i32];
            let half = BRICK as i32 / 2;
            let centre = vec3f((win[0] + o[0] + half) as f32, (win[1] + o[1] + half) as f32, (win[2] + o[2] + half) as f32) * v;
            if centre.x < cover_lo.x || centre.y < cover_lo.y || centre.z < cover_lo.z || centre.x > cover_hi.x || centre.y > cover_hi.y || centre.z > cover_hi.z { continue; }
            // The brick and every surface a voxel of it can touch, in the
            // object frame.
            let local = point(&obj, centre);
            let reach = (hb + 2.0 * hv) / scale;
            let region = (local - vec3f(reach, reach, reach), local + vec3f(reach, reach, reach));
            samples.clear();
            field.sample(region, &[[local.x, local.y, local.z]], &mut samples);
            let (d0, slot0) = samples.first().copied().unwrap_or((f32::MAX, 0));
            if d0 == f32::MAX || d0 * scale > hb + hv { continue; }
            let brick = BRICK as i32;
            let cell = |x: i32, y: i32, z: i32| (o[0] + x) as usize + d[0] * ((o[1] + y) as usize + d[1] * (o[2] + z) as usize);
            if d0 * scale < -(hb + hv) {
                let (albedo, emission, diffuse) = field.material(slot0);
                let c = albedo * diffuse;
                for z in 0..brick { for y in 0..brick { for x in 0..brick {
                    let a = &mut self.accum[cell(x, y, z)];
                    a.color = a.color + c; a.weight += 1.0;
                    a.emission = vec3f(a.emission.x.max(emission.x), a.emission.y.max(emission.y), a.emission.z.max(emission.z));
                }}}
                continue;
            }
            points.clear();
            for z in 0..brick { for y in 0..brick { for x in 0..brick {
                let p = point(&obj, voxel_world(o[0] + x, o[1] + y, o[2] + z));
                points.push([p.x, p.y, p.z]);
            }}}
            samples.clear();
            field.sample(region, &points, &mut samples);
            if samples.len() != points.len() { continue; }
            let at = |x: i32, y: i32, z: i32| samples[(x + brick * (y + brick * z)) as usize];
            for z in 0..brick { for y in 0..brick { for x in 0..brick {
                let (dist, slot) = at(x, y, z);
                if dist == f32::MAX || dist * scale >= hv { continue; }
                let (albedo, emission, diffuse) = field.material(slot);
                // The field's gradient, across the brick's own samples.
                let n = if dist * scale > -hv {
                    let (xl, xh) = ((x - 1).max(0), (x + 1).min(brick - 1));
                    let (yl, yh) = ((y - 1).max(0), (y + 1).min(brick - 1));
                    let (zl, zh) = ((z - 1).max(0), (z + 1).min(brick - 1));
                    let g = vec3f(at(xh, y, z).0 - at(xl, y, z).0, at(x, yh, z).0 - at(x, yl, z).0, at(x, y, zh).0 - at(x, y, zl).0);
                    let w = vec3f(m[0] * g.x + m[4] * g.y + m[8] * g.z, m[1] * g.x + m[5] * g.y + m[9] * g.z, m[2] * g.x + m[6] * g.y + m[10] * g.z);
                    let l = w.length();
                    if l > 1e-12 { w * (1.0 / l) } else { Vec3f::default() }
                } else {
                    Vec3f::default()
                };
                let c = albedo * diffuse;
                let a = &mut self.accum[cell(x, y, z)];
                a.color = a.color + c; a.normal = a.normal + n; a.weight += 1.0;
                a.emission = vec3f(a.emission.x.max(emission.x), a.emission.y.max(emission.y), a.emission.z.max(emission.z));
            }}}
        }}}
    }

    fn voxelize(&mut self, level: usize, win: [i32; 3], flags: &[bool]) -> (usize, usize) {
        let layout = self.layout;
        let d = layout.dims;
        let b = d.map(|d| d / BRICK);
        let v = layout.voxel_size(level);
        let half = v * 0.5;
        self.accum.clear();
        self.accum.resize(layout.level_voxels(), Accum::default());
        let (wlo, whi) = layout.window_box(level, win);
        let mut triangles = 0;
        if let Some(scene) = self.scene.clone() {
            let mut world = Vec::new();
            let mut colors = Vec::new();
            for inst in &scene.instances {
                if inst.hi.x < wlo.x || inst.lo.x > whi.x || inst.hi.y < wlo.y || inst.lo.y > whi.y || inst.hi.z < wlo.z || inst.lo.z > whi.z { continue; }
                // Skip instances that touch no flagged brick at all.
                let lo_b = [inst.lo.x, inst.lo.y, inst.lo.z].iter().enumerate().map(|(a, p)| (((p / v).floor() as i32 - win[a]).div_euclid(BRICK as i32)).clamp(0, b[a] as i32 - 1)).collect::<Vec<_>>();
                let hi_b = [inst.hi.x, inst.hi.y, inst.hi.z].iter().enumerate().map(|(a, p)| (((p / v).floor() as i32 - win[a]).div_euclid(BRICK as i32)).clamp(0, b[a] as i32 - 1)).collect::<Vec<_>>();
                let mut touches = false;
                'scan: for z in lo_b[2]..=hi_b[2] { for y in lo_b[1]..=hi_b[1] { for x in lo_b[0]..=hi_b[0] {
                    if flags[x as usize + b[0] * (y as usize + b[1] * z as usize)] { touches = true; break 'scan; }
                }}}
                if !touches { continue; }
                let mesh = match &inst.solid {
                    Solid::Mesh(mesh) => mesh,
                    Solid::Field(field) => { self.voxelize_field(inst, field, level, win, flags); continue; }
                };
                world.clear(); colors.clear();
                for vtx in mesh.vertices.chunks_exact(mesh.stride) {
                    world.push(point(&inst.transform, vec3f(vtx[0], vtx[1], vtx[2])));
                    colors.push(mesh.vertex_color(vtx));
                }
                for ids in mesh.indices.chunks_exact(3) {
                    let p = [world[ids[0] as usize], world[ids[1] as usize], world[ids[2] as usize]];
                    let n = Vec3f::cross(p[1] - p[0], p[2] - p[0]);
                    let area = n.length();
                    if !(area > 1e-10) { continue; }
                    let n = n * (1.0 / area);
                    let c = (colors[ids[0] as usize] + colors[ids[1] as usize] + colors[ids[2] as usize]) * (1.0 / 3.0);
                    let c = mul(c, inst.tint) * inst.diffuse;
                    let c = vec3f(c.x.clamp(0.0, 1.0), c.y.clamp(0.0, 1.0), c.z.clamp(0.0, 1.0));
                    let lo = vec3f(p[0].x.min(p[1].x).min(p[2].x), p[0].y.min(p[1].y).min(p[2].y), p[0].z.min(p[1].z).min(p[2].z));
                    let hi = vec3f(p[0].x.max(p[1].x).max(p[2].x), p[0].y.max(p[1].y).max(p[2].y), p[0].z.max(p[1].z).max(p[2].z));
                    let l = [lo.x, lo.y, lo.z]; let h = [hi.x, hi.y, hi.z];
                    let r = [0, 1, 2].map(|a| (((l[a] / v).floor() as i32 - win[a]).max(0), ((h[a] / v).floor() as i32 - win[a]).min(d[a] as i32 - 1)));
                    if r.iter().any(|(a, b)| a > b) { continue; }
                    triangles += 1;
                    let single = r.iter().all(|(a, b)| a == b);
                    for z in r[2].0..=r[2].1 { for y in r[1].0..=r[1].1 { for x in r[0].0..=r[0].1 {
                        let (x, y, z) = (x as usize, y as usize, z as usize);
                        if !flags[x / BRICK + b[0] * (y / BRICK + b[1] * (z / BRICK))] { continue; }
                        let center = vec3f((win[0] + x as i32) as f32 + 0.5, (win[1] + y as i32) as f32 + 0.5, (win[2] + z as i32) as f32 + 0.5) * v;
                        if !single && !tri_box_overlap(center, half, p) { continue; }
                        let a = &mut self.accum[x + d[0] * (y + d[1] * z)];
                        a.color = a.color + c; a.normal = a.normal + n; a.weight += 1.0;
                        a.emission = vec3f(a.emission.x.max(inst.emission.x), a.emission.y.max(inst.emission.y), a.emission.z.max(inst.emission.z));
                    }}}
                }
            }
        }
        let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let pack = |r: f32, g: f32, b: f32, a: u32| (a << 24) | (byte(r) << 16) | (byte(g) << 8) | byte(b);
        let mut bricks = 0;
        for bz in 0..b[2] { for by in 0..b[1] { for bx in 0..b[0] {
            if !flags[bx + b[0] * (by + b[1] * bz)] { continue; }
            bricks += 1;
            for z in bz * BRICK..(bz + 1) * BRICK { for y in by * BRICK..(by + 1) * BRICK { for x in bx * BRICK..(bx + 1) * BRICK {
                let a = self.accum[x + d[0] * (y + d[1] * z)];
                let t = layout.texel(level, [win[0] + x as i32, win[1] + y as i32, win[2] + z as i32]);
                if a.weight > 0.0 {
                    let c = a.color * (1.0 / a.weight);
                    let n = a.normal * (1.0 / a.weight);
                    let e = a.emission;
                    self.albedo[t] = pack(c.x, c.y, c.z, 255);
                    self.normal[t] = pack(n.x * 0.5 + 0.5, n.y * 0.5 + 0.5, n.z * 0.5 + 0.5, 255);
                    // Hue (authored encoding) + intensity m/(1+m) in alpha.
                    let m = e.x.max(e.y).max(e.z).max(0.0);
                    self.emission[t] = if m > 0.0 { pack(e.x / m, e.y / m, e.z / m, byte(m / (1.0 + m))) } else { 0 };
                } else {
                    self.albedo[t] = 0; self.normal[t] = 0; self.emission[t] = 0;
                }
            }}}
        }}}
        (bricks, triangles)
    }

    /// Occupancy of world voxel `c` at `level`, if the level window holds it.
    #[cfg(test)]
    pub fn solid(&self, level: usize, c: [i32; 3]) -> Option<bool> {
        let w = self.windows[level]?;
        if (0..3).any(|a| c[a] < w[a] || c[a] >= w[a] + self.layout.dims[a] as i32) { return None; }
        Some(self.albedo[self.layout.texel(level, c)] >> 24 != 0)
    }
}

/// Triangle / axis-aligned box overlap (separating axis test, Akenine-Möller).
pub(crate) fn tri_box_overlap(center: Vec3f, half: f32, tri: [Vec3f; 3]) -> bool {
    let v = tri.map(|p| p - center);
    let e = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    let axes = [vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)];
    let separated = |axis: Vec3f| {
        let p = v.map(|q| q.dot(axis));
        let r = half * (axis.x.abs() + axis.y.abs() + axis.z.abs());
        p[0].min(p[1]).min(p[2]) > r || p[0].max(p[1]).max(p[2]) < -r
    };
    for a in axes { for ed in e {
        let axis = Vec3f::cross(a, ed);
        if axis.length() > 1e-12 && separated(axis) { return false; }
    }}
    for a in axes { if separated(a) { return false; } }
    let n = Vec3f::cross(e[0], e[1]);
    !(n.length() > 1e-12 && separated(n))
}
