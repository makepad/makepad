//! Bounded realtime local shadow atlas (v2).
//!
//! * A spot occupies one square tile, a point six (a cube). Tiles come in
//!   power-of-two sizes from the configured maximum down to an eighth of it,
//!   chosen per light by importance (brightness and apparent size from the
//!   eye), and packed by a buddy allocator into a 4:2 atlas (8 max-size
//!   blocks). A light that does not fit at its size is offered the next
//!   smaller one; only when even the smallest does not fit does it light
//!   UNSHADOWED (never dark), and that happens to the lowest-ranked lights
//!   first because allocation runs in rank order.
//! * Ranking keeps last frame's owners (hysteresis), so the budget boundary
//!   does not flicker, and a light dimmed to nothing (a street lamp at noon)
//!   never takes atlas faces.
//! * Static faces are CACHED: a face is re-rendered only when its light
//!   moved, its tile moved, the static caster set changed, or a mover is (or
//!   was last time) inside its frustum. Re-rendered faces go into a lower
//!   depth "generation" of the atlas so a tile can be cleared on its own
//!   (the pass clears its depth only as a whole and tests LessEqual, so
//!   generation g writes the slice `[(G-1-g)/G, (G-g)/G]` and a tile-sized
//!   quad at its far end clears the tile); after
//!   [`LOCAL_DEPTH_GENS`] rendering frames the atlas is cleared and every
//!   face re-rendered.
//! * Cube faces are rendered a couple of texels wider than 90 degrees, so a
//!   filter footprint at a face edge reads real depth instead of a clamped
//!   border (the seam between cube faces).
use crate::{
    gpu_lightmap::{GpuBakeMesh, GpuLmMover},
    lightmap::LmLight,
    shaders::*,
};
use makepad_draw::*;
use std::hash::{Hash, Hasher};

/// Hard cap on faces per frame (metadata rows, draw lists).
const MAX_FACES: usize = 64;
/// Atlas blocks of the maximum tile size: 4 columns x 2 rows.
const ROOT_COLS: usize = 4;
const ROOT_ROWS: usize = 2;
/// Smallest tile: the maximum tile size >> MIN_TIER_SHIFT.
const MIN_TIER_SHIFT: u32 = 3;
/// Depth generations for face caching. Perspective depth crowds near 1, so
/// each generation keeps an eighth of the range (about 1.5 float steps per
/// centimetre at 20 m; the receiver's bias floor is 1.5 cm). Every
/// LOCAL_DEPTH_GENS rendering frames the whole atlas is cleared and
/// re-rendered, so more generations mean rarer full re-renders.
const LOCAL_DEPTH_GENS: u32 = 8;
/// Texels a cube face is widened by on every side (cross-face filtering).
const CUBE_GUARD: f32 = 2.0;

/// Shader layout and atlas format must agree for the lifetime of the app.
pub(crate) fn hardware_shadow_maps() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MAKEPAD_LOCAL_SHADOWS").as_deref() != Ok("legacy")
            && std::env::var("MAKEPAD_CLUSTERED").as_deref() != Ok("off")
            && !std::env::var("MAKEPAD").unwrap_or_default().split(',').any(|v| v == "gpusim")
    })
}

/// `max_faces` caps the faces per frame; `resolution` is the LARGEST tile
/// edge. The atlas is `4 * resolution` x `2 * resolution` (4096 x 2048 at
/// the default 1024: 32 MiB of D32 plus the R32F colour target).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalShadowConfig {
    pub max_faces: usize,
    pub resolution: usize,
}
impl Default for LocalShadowConfig {
    fn default() -> Self {
        Self {
            max_faces: MAX_FACES,
            resolution: 1024,
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
    /// Faces re-rendered this frame (the rest came from the cache).
    pub rendered_faces: usize,
    pub omitted_lights: usize,
    /// Lights that got a smaller tile than their importance asked for.
    pub downsized_lights: usize,
    pub caster_draws: usize,
    pub encode_us: u64,
    /// Smoothed GPU time of the atlas pass in ms (Metal command-buffer
    /// timing; 0 on backends that do not report it).
    pub gpu_ms: f32,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ShadowRecord {
    pub first: usize,
    /// 0=no request, -1=omitted, 1=spot, 6=point.
    pub count: i32,
    pub near: f32,
    pub far: f32,
    /// Tile edge in texels of this light's faces.
    pub tile: usize,
}

/// One light's tile request while the frame's atlas is being sized.
#[derive(Clone, Copy)]
struct TilePlan {
    light: usize,
    rank: usize,
    count: usize,
    want: usize,
    tile: usize,
}

#[derive(Clone, Copy, PartialEq)]
struct Face {
    light: usize,
    /// Which face of the light (0 for a spot, 0..6 for a point).
    index: usize,
    key: u64,
    rx: Vec4f,
    ry: Vec4f,
    rz: Vec4f,
    tile: Vec4f,
    /// Texel rect (x, y, edge) in the atlas.
    rect: (usize, usize, usize),
    generation: u32,
    /// A skinned/morphed (pose-changing) mover was drawn into this face.
    had_live: bool,
    /// Identity of the rigid movers inside the face when it was rendered
    /// (geometry + exact transform, order-independent).
    movers_hash: u64,
}

/// Score multiplier for a light that held faces last frame: a challenger must
/// clearly out-rank it before the budget changes hands.
const OWNER_BONUS: f32 = 1.5;

/// Frame-stable identity of a light (lists are rebuilt every frame).
fn light_key(l: &LmLight) -> u64 {
    let q = |v: f32| ((v * 10.0).round() as i64 as u64) & 0x1f_ffff;
    q(l.pos.x) | (q(l.pos.y) << 21) | (q(l.pos.z) << 42) ^ (l.radius.to_bits() as u64).rotate_left(7)
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

/// A mover as the face cache sees it: bounds for culling, an identity
/// (geometry + exact transform) and whether its shape changes by itself.
/// Rigid movers cull by their posed AABB; skinned and morphed ones by the
/// rest AABB grown by its own size plus a metre (a posed limb can leave the
/// rest box, never by that much), and are always "live".
#[derive(Clone, Copy)]
struct MoverBox {
    min: Vec3f,
    max: Vec3f,
    id: u64,
    live: bool,
}

fn mover_box(m: &GpuLmMover) -> MoverBox {
    let (lo, hi) = crate::lightmap::world_bounds(&m.transform, (m.min, m.max));
    let live = m.skin.is_some() || m.morph.is_some();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    m.geometry.hash(&mut h);
    for v in m.transform.v {
        v.to_bits().hash(&mut h);
    }
    let (min, max) = if live {
        let pad = (hi - lo) * 0.5 + vec3f(1.0, 1.0, 1.0);
        (lo - pad, hi + pad)
    } else {
        (lo, hi)
    };
    MoverBox { min, max, id: h.finish(), live }
}

/// Buddy allocator over the atlas: ROOT_COLS x ROOT_ROWS blocks of `max`
/// texels, split into quarters on demand. Deterministic for a given request
/// sequence, so an unchanged light set gets unchanged tiles (the cache key).
#[derive(Default)]
struct Buddy {
    max: usize,
    /// Free blocks per level (level 0 = max size), each an (x, y) origin.
    free: Vec<Vec<(usize, usize)>>,
}
impl Buddy {
    fn reset(&mut self, max: usize) {
        self.max = max;
        self.free.resize_with(MIN_TIER_SHIFT as usize + 1, Vec::new);
        for level in &mut self.free {
            level.clear();
        }
        // Reverse push so pop() hands out blocks in reading order.
        for row in (0..ROOT_ROWS).rev() {
            for col in (0..ROOT_COLS).rev() {
                self.free[0].push((col * max, row * max));
            }
        }
    }
    fn alloc(&mut self, size: usize) -> Option<(usize, usize)> {
        let level = (self.max / size).trailing_zeros() as usize;
        if level >= self.free.len() {
            return None;
        }
        let mut from = level;
        while self.free[from].is_empty() {
            if from == 0 {
                return None;
            }
            from -= 1;
        }
        while from < level {
            let (x, y) = self.free[from].pop().unwrap();
            let half = self.max >> (from + 1);
            from += 1;
            for (dx, dy) in [(1, 1), (0, 1), (1, 0)] {
                self.free[from].push((x + dx * half, y + dy * half));
            }
            self.free[from].push((x, y));
        }
        self.free[level].pop()
    }
    fn release(&mut self, size: usize, at: (usize, usize)) {
        let level = (self.max / size).trailing_zeros() as usize;
        self.free[level].push(at);
    }
}

pub(crate) struct LocalShadows {
    pub soft_filter: bool,
    /// Apparent emitter radius in world units for bounded contact-hardening.
    pub source_radius: f32,
    config: LocalShadowConfig,
    pub records: Vec<ShadowRecord>,
    pub data: Vec<f32>,
    faces: Vec<Face>,
    /// Last frame's faces: the cache the new allocation is matched against.
    prev_faces: Vec<Face>,
    ranked: Vec<(f32, usize)>,
    /// Keys of the lights that held faces last frame (sorted), for hysteresis.
    owners: Vec<u64>,
    next_owners: Vec<u64>,
    buddy: Buddy,
    plan: Vec<TilePlan>,
    /// Faces to re-render this frame (indices into `faces`).
    dirty: Vec<usize>,
    /// Depth generation the next re-rendered faces use, and whether the
    /// atlas content is valid at all (cleared on allocation / config).
    generation: u32,
    atlas_valid: bool,
    statics_hash: u64,
    frames: u64,
    acc_rendered: usize,
    acc_full: usize,
    miss_reasons: [usize; 4],
    movers: Vec<MoverBox>,
    pub texture: Option<Texture>,
    metadata: Option<Texture>,
    metadata_height: usize,
    depth: Option<Texture>,
    allocation_size: (usize, usize),
    pass: Option<DrawPass>,
    list: Option<DrawList>,
    rigid: Option<DrawLmLampDepth>,
    /// `rigid` without the face clip, for draws scissored to their tile.
    inside: Option<DrawLmLampDepthInside>,
    skinned: Option<DrawLocalShadowSkinned>,
    clear_quad: Option<Geometry>,
    pub stats: LocalShadowStats,
    gpu_ms: f32,
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
            prev_faces: Vec::new(),
            ranked: Vec::new(),
            owners: Vec::new(),
            next_owners: Vec::new(),
            buddy: Buddy::default(),
            plan: Vec::new(),
            dirty: Vec::new(),
            generation: 0,
            atlas_valid: false,
            statics_hash: 0,
            frames: 0,
            acc_rendered: 0,
            acc_full: 0,
            miss_reasons: [0; 4],
            movers: Vec::new(),
            texture: None,
            metadata: None,
            metadata_height: 0,
            depth: None,
            allocation_size: (0, 0),
            pass: None,
            list: None,
            rigid: None,
            inside: None,
            skinned: None,
            clear_quad: None,
            stats: LocalShadowStats::default(),
            gpu_ms: 0.0,
        }
    }
}

/// Floats per face in the metadata: rx, ry, rz, the atlas uv rect, and the
/// depth window (stored = projected * x + y).
const FACE_FLOATS: usize = 20;

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
            self.atlas_valid = false;
            self.config = config;
        }
    }
    pub fn size(&self) -> (usize, usize) {
        (ROOT_COLS * self.config.resolution, ROOT_ROWS * self.config.resolution)
    }
    /// Importance -> tile edge: brightness times apparent size from the
    /// eye (radius over distance), a full-size tile for a light whose reach
    /// contains the eye, an eighth for a distant one. Points are capped at half size
    /// (six faces).
    ///
    /// Hysteresis: a light keeps last frame's tile until its ideal size is
    /// three quarters of an octave away, so walking past a lamp does not
    /// flip its tile (and re-render and re-pack the atlas) on every
    /// boundary crossing.
    fn desired_tile(&self, l: &LmLight, eye: Vec3f, count: usize, prev: Option<usize>) -> usize {
        let max = self.config.resolution;
        let d = (l.pos - eye).length();
        let peak = l.color.x.max(l.color.y).max(l.color.z).min(1.0).max(0.25);
        let cover = (l.radius / d.max(0.001) * peak).min(1.0);
        let min = max >> MIN_TIER_SHIFT;
        // Nearest power of two (in log space), not the next one up.
        let ideal = (cover * max as f32).max(1.0);
        let mut t = (ideal.log2().round().exp2() as usize).clamp(min, max);
        if let Some(prev) = prev.filter(|p| (min..=max).contains(p)) {
            if (ideal.log2() - (prev as f32).log2()).abs() < 0.75 {
                t = prev;
            }
        }
        if count == 6 {
            t = t.min(max / 2).max(min);
        }
        t
    }
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
                2.0 / ((r.tile - 2) as f32 * (row.x * row.x + row.y * row.y + row.z * row.z).sqrt())
            } else {
                0.0
            };
            packed[i * 4..i * 4 + 4].copy_from_slice(&[
                r.count as f32,
                r.near,
                (self.records.len() + r.first * (FACE_FLOATS / 4)) as f32,
                texel_world_per_depth,
            ]);
        }
        // Face depth windows: the generation window (clip z is the stored
        // depth on every backend: the hardware compare's reference space).
        for (f, face) in self.faces.iter().enumerate() {
            let (gs, go) = local_generation_window(face.generation);
            let at = f * FACE_FLOATS + 16;
            self.data[at] = gs;
            self.data[at + 1] = go;
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
    /// Rank, size and place this frame's faces; mark the ones whose cached
    /// content is stale (`dirty`). `statics_hash` identifies the static
    /// caster set; `mover_boxes` are this frame's mover bounds.
    fn prepare(&mut self, lights: &[LmLight], active: &[bool], eye: Vec3f) {
        self.stats = LocalShadowStats::default();
        self.records.clear();
        self.records.resize(lights.len(), ShadowRecord::default());
        std::mem::swap(&mut self.faces, &mut self.prev_faces);
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
            let peak = l.color.x.max(l.color.y).max(l.color.z);
            if peak <= 1.0e-3 {
                // Dimmed to nothing (daylight): no light, so no shadow to cast.
                self.records[i].count = 0;
                continue;
            }
            let delta = l.pos - eye;
            let mut score = peak * l.radius * l.radius / (delta.dot(delta) + l.radius * l.radius);
            if self.owners.binary_search(&light_key(l)).is_ok() {
                score *= OWNER_BONUS;
            }
            self.ranked.push((score, i));
        }
        self.next_owners.clear();
        self.ranked
            .sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        // Size the tiles: each light's importance asks for a tile; if the
        // asks overflow the atlas, the LOWEST-ranked lights are halved first
        // (down to the smallest tier), and only when every remaining light
        // already sits at the smallest tier is the lowest-ranked dropped
        // (unshadowed). Rank order, so a boundary light never out-sizes a
        // brighter one.
        let res = self.config.resolution;
        let min_tile = res >> MIN_TIER_SHIFT;
        self.plan.clear();
        let mut face_budget = self.config.max_faces;
        for (rank, &(_, i)) in self.ranked.iter().enumerate() {
            let l = &lights[i];
            let count = if l.cone.is_some_and(|(_, outer)| outer < 89.0) { 1 } else { 6 };
            if count > face_budget {
                self.records[i].count = 0;
                self.stats.omitted_lights += 1;
                continue;
            }
            face_budget -= count;
            let key = light_key(l);
            let prev = self.prev_faces.iter().find(|f| f.key == key).map(|f| f.rect.2);
            let want = self.desired_tile(l, eye, count, prev);
            self.plan.push(TilePlan { light: i, rank, count, want, tile: want });
        }
        let capacity = ROOT_COLS * ROOT_ROWS * res * res;
        let mut area: usize = self.plan.iter().map(|p| p.count * p.tile * p.tile).sum();
        while area > capacity {
            if let Some(p) = self.plan.iter_mut().rev().find(|p| p.tile > min_tile) {
                area -= p.count * (p.tile * p.tile - p.tile * p.tile / 4);
                p.tile /= 2;
            } else {
                let p = self.plan.pop().unwrap();
                area -= p.count * p.tile * p.tile;
                self.records[p.light].count = 0;
                self.stats.omitted_lights += 1;
            }
        }
        // Largest tiles first: power-of-two squares whose total area fits
        // always pack into the buddy blocks in that order.
        self.plan.sort_by(|a, b| b.tile.cmp(&a.tile).then(a.rank.cmp(&b.rank)));
        self.buddy.reset(res);
        let (width, height) = self.size();
        let mut slots = [(0usize, 0usize); 6];
        for pi in 0..self.plan.len() {
            let TilePlan { light: i, count, want, tile, .. } = self.plan[pi];
            let l = &lights[i];
            let mut got = 0;
            while got < count {
                match self.buddy.alloc(tile) {
                    Some(at) => {
                        slots[got] = at;
                        got += 1;
                    }
                    None => break,
                }
            }
            if got < count {
                // Cannot happen for a plan that fits; stay unshadowed if it does.
                for at in &slots[..got] {
                    self.buddy.release(tile, *at);
                }
                self.records[i].count = 0;
                self.stats.omitted_lights += 1;
                continue;
            }
            if tile < want {
                self.stats.downsized_lights += 1;
            }
            let key = light_key(l);
            self.next_owners.push(key);
            let near = (l.radius * 0.001).clamp(0.001, 0.03);
            self.records[i] = ShadowRecord {
                first: self.faces.len(),
                count: count as i32,
                near,
                far: l.radius,
                tile,
            };
            let focal = if count == 1 {
                1.0 / l.cone.unwrap().1.to_radians().tan()
            } else {
                // A cube face slightly wider than 90 degrees: CUBE_GUARD
                // texels of real depth beyond every edge.
                1.0 - 2.0 * CUBE_GUARD / (tile - 2) as f32
            };
            for (f, &(x, y)) in slots[..count].iter().enumerate() {
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
                let uv = vec4(
                    (x + 1) as f32 / width as f32,
                    (y + 1) as f32 / height as f32,
                    (tile - 2) as f32 / width as f32,
                    (tile - 2) as f32 / height as f32,
                );
                let clip = vec4(uv.z, uv.w, uv.x * 2.0 + uv.z - 1.0, 1.0 - uv.y * 2.0 - uv.w);
                self.faces.push(Face {
                    light: i,
                    index: f,
                    key,
                    rx,
                    ry,
                    rz,
                    tile: clip,
                    rect: (x, y, tile),
                    generation: 0,
                    had_live: false,
                    movers_hash: 0,
                });
                for row in [rx, ry, rz, uv] {
                    self.data.extend_from_slice(&[row.x, row.y, row.z, row.w]);
                }
                // Depth window, filled at upload (needs the backend mapping).
                self.data.extend_from_slice(&[1.0, 0.0, 0.0, 0.0]);
            }
            self.stats.lights += 1;
        }
        self.next_owners.sort_unstable();
        std::mem::swap(&mut self.owners, &mut self.next_owners);
        self.stats.faces = self.faces.len();
    }

    /// Match this frame's faces against last frame's: a face keeps its
    /// cached content (and generation) when the same light face sits in the
    /// same tile with the same camera rows, the static set is unchanged,
    /// and no mover is inside it now or was when it was rendered. Everything
    /// else is `dirty` and re-renders this frame in a fresh generation.
    /// Returns true when the whole atlas must be cleared first.
    fn plan_cache(&mut self, statics_changed: bool) -> bool {
        self.dirty.clear();
        for f in 0..self.faces.len() {
            let face = self.faces[f];
            let record = self.records[face.light];
            // The movers inside this face: an order-independent identity of
            // the rigid ones, and whether any changes shape by itself. A
            // parked car or a terrain tile passed as a mover keeps the face
            // cached; one that moves (or leaves) re-renders it.
            let mut movers_hash = 0u64;
            let mut live = false;
            for m in &self.movers {
                if in_face(&face, record.near, record.far, m.min, m.max) {
                    movers_hash = movers_hash.wrapping_add(m.id.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
                    live |= m.live;
                }
            }
            self.faces[f].movers_hash = movers_hash;
            self.faces[f].had_live = live;
            let reusable = self.atlas_valid && !statics_changed && !live;
            let cached = self
                .prev_faces
                .iter()
                .filter(|_| reusable)
                .find(|p| {
                    p.key == face.key && p.index == face.index && p.rect == face.rect
                        && p.rx == face.rx && p.ry == face.ry && p.rz == face.rz
                })
                .filter(|p| !p.had_live && p.movers_hash == movers_hash)
                .map(|p| p.generation);
            match cached {
                Some(generation) => self.faces[f].generation = generation,
                None => {
                    // Why (for the MAKEPAD_CLUSTER_STATS summary).
                    let reason = if !self.atlas_valid || statics_changed {
                        0
                    } else if live {
                        1
                    } else if self.prev_faces.iter().any(|p| p.key == face.key && p.index == face.index && p.rect == face.rect) {
                        2
                    } else {
                        3
                    };
                    self.miss_reasons[reason] += 1;
                    self.dirty.push(f);
                }
            }
        }
        if self.dirty.is_empty() {
            return false;
        }
        let full = !self.atlas_valid || self.generation + 1 >= LOCAL_DEPTH_GENS;
        if full {
            self.generation = 0;
            self.dirty.clear();
            self.dirty.extend(0..self.faces.len());
        } else {
            self.generation += 1;
        }
        for &f in &self.dirty {
            self.faces[f].generation = self.generation;
        }
        self.atlas_valid = true;
        full
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
        if let Some(pass) = &self.pass {
            for ms in pass.take_gpu_times_ms(cx.cx) {
                self.gpu_ms = crate::gpu_lightmap::ema_ms(self.gpu_ms, ms);
            }
        }
        self.prepare(lights, active, eye);
        self.stats.gpu_ms = self.gpu_ms;
        if self.faces.is_empty() && (!hardware_shadow_maps() || self.depth.is_some()) {
            self.gpu_ms = 0.0;
            self.stats.gpu_ms = 0.0;
            self.upload_metadata(cx.cx);
            return;
        }
        if self.rigid.is_none() {
            let Some((rigid, inside, skinned)) = cx.cx.try_with_vm(|vm| {
                (
                    DrawLmLampDepth::script_new_with_default(vm),
                    DrawLmLampDepthInside::script_new_with_default(vm),
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
                self.faces.clear();
                self.upload_metadata(cx.cx);
                return;
            };
            self.rigid = Some(rigid);
            self.inside = Some(inside);
            self.skinned = Some(skinned);
        }
        // An initialized one-texel depth binding is still required when the
        // shader has no active shadow lights. Do not allocate a full atlas;
        // once a full atlas exists it is kept (no 1x1 <-> full hitch).
        let (width, height) = if self.faces.is_empty() && self.allocation_size.0 <= 1 { (1, 1) } else { self.size() };
        if self.texture.is_none() || self.allocation_size != (width, height) {
            self.allocation_size = (width, height);
            self.atlas_valid = false;
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
        // Cache inputs: the static set's identity and every mover's bounds.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        statics.len().hash(&mut hasher);
        for m in statics {
            m.geometry.hash(&mut hasher);
            for v in m.transform.v {
                v.to_bits().hash(&mut hasher);
            }
        }
        let statics_hash = hasher.finish();
        let statics_changed = statics_hash != self.statics_hash;
        self.statics_hash = statics_hash;
        self.movers.clear();
        self.movers.extend(movers.iter().map(mover_box));
        let full = self.plan_cache(statics_changed);
        self.stats.rendered_faces = self.dirty.len();
        self.frames += 1;
        self.acc_rendered += self.dirty.len();
        self.acc_full += full as usize;
        if self.frames % 120 == 0 && std::env::var_os("MAKEPAD_CLUSTER_STATS").is_some() {
            let mut tiles = [0usize; MIN_TIER_SHIFT as usize + 1];
            for f in &self.faces {
                tiles[(self.config.resolution / f.rect.2).trailing_zeros() as usize] += 1;
            }
            log!(
                "local shadow atlas: {} faces by tile {:?} (from {}px down), {} downsized; last 120 frames: {:.1} faces re-rendered/frame, {} full clears; misses: {} statics/new atlas, {} live mover, {} moved mover/light, {} new tile",
                self.faces.len(), tiles, self.config.resolution, self.stats.downsized_lights,
                self.acc_rendered as f32 / 120.0, self.acc_full,
                self.miss_reasons[0], self.miss_reasons[1], self.miss_reasons[2], self.miss_reasons[3]
            );
            self.miss_reasons = [0; 4];
            self.acc_rendered = 0;
            self.acc_full = 0;
        }
        if self.dirty.is_empty() {
            // Every face is cached: the atlas pass is not encoded at all and
            // the targets keep their content.
            self.gpu_ms = 0.0;
            self.stats.gpu_ms = 0.0;
            self.upload_metadata(cx.cx);
            self.stats.encode_us = ((Cx::monotonic_now() - start) * 1e6) as u64;
            return;
        }
        let pass = self.pass.get_or_insert_with(|| {
            let pass = DrawPass::new(cx.cx);
            pass.set_pass_name(cx.cx, "local_shadows");
            pass.set_gpu_timing_enabled(cx.cx, true);
            // Hardware maps sample the atlas DEPTH; the colour is then scratch.
            pass.set_color_scratch(cx.cx, hardware_shadow_maps());
            pass
        });
        let list = self.list.get_or_insert_with(|| DrawList::new(cx.cx));
        cx.make_child_pass(pass);
        cx.begin_pass(pass, Some(1.0));
        pass.set_size(cx.cx, dvec2(width as f64, height as f64));
        pass.clear_color_textures(cx.cx);
        let clear = vec4(1.0, 1.0, 1.0, 1.0);
        pass.set_color_texture(
            cx.cx,
            self.texture.as_ref().unwrap(),
            if full { DrawPassClearColor::ClearWith(clear) } else { DrawPassClearColor::InitWith(clear) },
        );
        pass.set_depth_texture(
            cx.cx,
            self.depth.as_ref().unwrap(),
            if full { DrawPassClearDepth::ClearWith(1.0) } else { DrawPassClearDepth::InitWith(1.0) },
        );
        list.begin_always(cx);
        let quad = self
            .clear_quad
            .get_or_insert_with(|| {
                let g = Geometry::new(cx.cx);
                let mut v = Vec::with_capacity(4 * crate::skin::SKIN_VERTEX_FLOATS);
                // Just inside the depth shader's 0.999 frustum discard.
                for (x, y) in [(-0.9985, -0.9985), (0.9985, -0.9985), (0.9985, 0.9985), (-0.9985, 0.9985)] {
                    v.extend_from_slice(&[x, y, 0.0]);
                    v.resize(v.len() + crate::skin::SKIN_VERTEX_FLOATS - 3, 0.0);
                }
                g.update(cx.cx, vec![0, 1, 2, 0, 2, 3], v);
                g
            })
            .geometry_id();
        let rigid = self.rigid.as_mut().unwrap();
        let inside = self.inside.as_mut().unwrap();
        let skinned = self.skinned.as_mut().unwrap();
        // Where the backend applies a per-draw scissor, every draw into a
        // face is confined to the face's tile, so a caster that spans far
        // past the face (a wall beside the lamp) rasterizes only the tile,
        // and rigid casters take the discard-free pipeline. Elsewhere the
        // face clip in the shader does the confining.
        let scissored = cx.cx.gpu_backend().honors_scissor();
        for &f in &self.dirty {
            let face = self.faces[f];
            let record = self.records[face.light];
            let far = lights[face.light].radius;
            let (gs, go) = local_generation_window(face.generation);
            // Depth generation in clip space: z * (1 - z_) + w_ * vz.
            let window = (1.0 - gs, go);
            // The face's texels: its tile minus the one-texel border.
            let (x, y, edge) = face.rect;
            let scissor = scissored.then(|| [x as u32 + 1, y as u32 + 1, edge.saturating_sub(2) as u32, edge.saturating_sub(2) as u32]);
            rigid.draw_vars.options.scissor = scissor;
            inside.draw_vars.options.scissor = scissor;
            skinned.base.draw_vars.options.scissor = scissor;
            rigid.tile_a = face.tile;
            rigid.set_morph(cx.cx,None);
            if !full {
                // Clear this tile to its generation's far end: identity
                // rows, view z = 1, depth range (0, 1) -> ndc z = 1.
                rigid.face_rx = vec4(1.0, 0.0, 0.0, 0.0);
                rigid.face_ry = vec4(0.0, 1.0, 0.0, 0.0);
                rigid.face_rz = vec4(0.0, 0.0, 0.0, 1.0);
                rigid.lamp_range = vec4(0.0, 1.0, window.0, window.1);
                rigid.transform = Mat4f::identity();
                rigid.draw_vars.geometry_id = Some(quad);
                if rigid.draw_vars.can_instance() {
                    cx.add_instance(&rigid.draw_vars);
                }
            }
            rigid.face_rx = face.rx;
            rigid.face_ry = face.ry;
            rigid.face_rz = face.rz;
            rigid.lamp_range = vec4(record.near, far, window.0, window.1);
            inside.face_rx = face.rx;
            inside.face_ry = face.ry;
            inside.face_rz = face.rz;
            inside.tile_a = face.tile;
            inside.lamp_range = rigid.lamp_range;
            inside.set_morph(cx.cx, None);
            for m in statics {
                if !in_face(&face, record.near, far, m.min, m.max) {
                    continue;
                }
                let draw = if scissored { &mut inside.depth } else { &mut *rigid };
                draw.transform = m.transform;
                draw.draw_vars.geometry_id = Some(m.geometry);
                if draw.draw_vars.can_instance() {
                    cx.add_instance(&draw.draw_vars);
                    self.stats.caster_draws += 1;
                }
            }
            for (m, b) in movers.iter().zip(&self.movers) {
                if !in_face(&face, record.near, far, b.min, b.max) {
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
                    let draw = if scissored { &mut inside.depth } else { &mut *rigid };
                    draw.set_morph(cx.cx,m.morph.as_ref());
                    draw.transform = m.transform;
                    draw.draw_vars.geometry_id = Some(m.geometry);
                    if draw.draw_vars.can_instance() {
                        cx.add_instance(&draw.draw_vars);
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

/// Clip-space z window of a local-atlas depth generation.
fn local_generation_window(generation: u32) -> (f32, f32) {
    let g = LOCAL_DEPTH_GENS as f32;
    (1.0 / g, (g - 1.0 - generation.min(LOCAL_DEPTH_GENS - 1) as f32) / g)
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
                let at=record.z+face*5.0
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
                let at=record.z+face*5.0
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
                // legacy color output. The face's depth window (its cache
                // generation and the backend's clip-z remap) rides in its
                // fifth metadata row.
                let projected = radius * (biased_z - record.y) / max(biased_z * (radius - record.y), 0.000001)
                let window = self.local_shadow_fetch(at+4.0)
                let depth = projected * window.x + window.y
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
            // lamp_range.zw: the face's depth generation, as DrawLmLampDepth.
            self.vertex_pos=vec4(vx*self.tile_a.x+vz*self.tile_a.z,vy*self.tile_a.y+vz*self.tile_a.w,
                (vz-self.lamp_range.x)*self.lamp_range.y/max(self.lamp_range.y-self.lamp_range.x,0.0001)*(1.0-self.lamp_range.z)+self.lamp_range.w*vz,vz)
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
    /// The v2 atlas: a 4:2 target of the configured maximum tile, faces in
    /// power-of-two tiles that stay inside the atlas and never overlap, and
    /// the per-face metadata stride the shader reads (FACE_FLOATS).
    #[test]
    fn atlas_tiles_are_disjoint_and_inside_the_target() {
        let mut shadows = LocalShadows::default();
        shadows.set_config(LocalShadowConfig { max_faces: 64, resolution: 1024 });
        assert_eq!(shadows.size(), (4096, 2048));
        // A mix of near spots and a few points, eye at the origin.
        let mut lights = Vec::new();
        for i in 0..24 {
            lights.push(spot(vec3f(i as f32 * 7.0, 3.0, 0.0)));
        }
        for i in 0..4 {
            lights.push(LmLight { cone: None, ..spot(vec3f(0.0, 3.0, 10.0 + i as f32 * 20.0)) });
        }
        let active = vec![true; lights.len()];
        shadows.prepare(&lights, &active, Vec3f::default());
        assert!(shadows.stats.faces > 24);
        assert_eq!(shadows.data.len(), shadows.faces.len() * FACE_FLOATS);
        for (i, a) in shadows.faces.iter().enumerate() {
            let (x, y, t) = a.rect;
            assert!(t.is_power_of_two() && t >= 128 && t <= 1024);
            assert!(x + t <= 4096 && y + t <= 2048);
            for b in &shadows.faces[i + 1..] {
                let (bx, by, bt) = b.rect;
                assert!(x + t <= bx || bx + bt <= x || y + t <= by || by + bt <= y, "tiles overlap");
            }
        }
        for row in shadows.data.chunks_exact(FACE_FLOATS) {
            assert!(row[12] >= 0.0 && row[13] >= 0.0);
            assert!(row[12] + row[14] <= 1.0 && row[13] + row[15] <= 1.0);
        }
    }

    /// Importance sizes the tile: a near light gets the full tile, a far
    /// one an eighth; a full atlas downsizes lower-ranked lights before any
    /// light goes unshadowed.
    #[test]
    fn tiles_follow_importance_and_downsize_before_omitting() {
        let mut shadows = LocalShadows::default();
        shadows.set_config(LocalShadowConfig { max_faces: 64, resolution: 1024 });
        let near = spot(vec3f(0.0, 3.0, 5.0));
        let far = LmLight { radius: 8.0, ..spot(vec3f(0.0, 3.0, 200.0)) };
        shadows.prepare(&[near.clone(), far], &[true, true], Vec3f::default());
        assert_eq!(shadows.records[0].tile, 1024);
        assert_eq!(shadows.records[1].tile, 128);
        // Twelve near spots want 12 full tiles; the atlas holds 8.
        let lights = vec![near; 12];
        shadows.prepare(&lights, &[true; 12], Vec3f::default());
        assert_eq!(shadows.stats.omitted_lights, 0, "downsize, don't drop");
        assert!(shadows.stats.downsized_lights >= 4);
        assert_eq!(shadows.records[0].tile, 1024, "the top-ranked light keeps its size");
    }

    /// Face caching: an unchanged light set with no movers re-renders
    /// nothing; a mover re-renders only the faces it is inside (and those
    /// again the frame after it leaves); a static change re-renders all.
    #[test]
    fn static_faces_are_cached_and_movers_dirty_only_their_faces() {
        let mut s = LocalShadows::default();
        let lights = vec![
            spot(vec3f(0.0, 3.0, 0.0)),
            LmLight { cone: None, ..spot(vec3f(60.0, 3.0, 0.0)) },
        ];
        let active = [true, true];
        let rigid = |(min, max): (Vec3f, Vec3f), id: u64| MoverBox { min, max, id, live: false };
        let frame = |s: &mut LocalShadows, movers: Vec<MoverBox>, statics_changed: bool| {
            s.prepare(&lights, &active, Vec3f::default());
            s.movers = movers;
            s.plan_cache(statics_changed);
            s.dirty.len()
        };
        assert_eq!(frame(&mut s, vec![], false), 7, "first frame renders every face");
        assert_eq!(frame(&mut s, vec![], false), 0, "nothing changed: all cached");
        // A crate 5 m in front of the spot (it looks down -z).
        let crate_box = (vec3f(-0.5, 2.5, -5.5), vec3f(0.5, 3.5, -4.5));
        assert_eq!(frame(&mut s, vec![rigid(crate_box, 7)], false), 1, "only the spot face");
        assert_eq!(frame(&mut s, vec![], false), 1, "and again once it has left");
        assert_eq!(frame(&mut s, vec![], false), 0);
        assert_eq!(frame(&mut s, vec![], true), 7, "a static change re-renders all");
        // A mover that stays put (a parked car, a terrain tile handed over
        // as a mover) keeps the face cached after its first render...
        let mut s = LocalShadows::default();
        assert_eq!(frame(&mut s, vec![], false), 7);
        assert_eq!(frame(&mut s, vec![rigid(crate_box, 9)], false), 1);
        assert_eq!(frame(&mut s, vec![rigid(crate_box, 9)], false), 0);
        // ...one that moved (new transform = new identity) does not, and a
        // skinned one never does.
        assert_eq!(frame(&mut s, vec![rigid(crate_box, 10)], false), 1);
        let live = MoverBox { live: true, ..rigid(crate_box, 10) };
        assert_eq!(frame(&mut s, vec![live], false), 1);
        assert!(frame(&mut s, vec![live], false) >= 1, "a live mover's face never caches");
        // Generations descend; running out clears the atlas.
        let mut fulls = 0;
        for _ in 0..2 * LOCAL_DEPTH_GENS {
            s.prepare(&lights, &active, Vec3f::default());
            s.movers = vec![MoverBox { live: true, ..rigid(crate_box, 1) }];
            let full = s.plan_cache(false);
            fulls += full as u32;
            assert!(s.generation < LOCAL_DEPTH_GENS);
        }
        assert!(fulls >= 2, "the atlas clears every LOCAL_DEPTH_GENS rendering frames");
        for g in 1..LOCAL_DEPTH_GENS {
            let (_, o0) = local_generation_window(g - 1);
            let (sc, o1) = local_generation_window(g);
            assert!(o1 + sc <= o0 + 1e-6);
        }
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
                    let pixels = (shadows.records[0].tile - 2) as f32;
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
    fn an_over_budget_light_lights_unshadowed_and_owners_keep_their_faces() {
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
        // 0 = no shadow record: the shader lights it unshadowed, never black.
        assert_eq!(s.records[1].count, 0);
        assert_eq!(s.records[0].count, 1);
        assert_eq!(s.records[2].count, 1);
        // Hysteresis: the far light edging slightly closer than an owner
        // does not steal its faces.
        s.prepare(
            &[
                spot(vec3f(0.0, 0.0, 0.0)),
                spot(vec3f(1.9, 0.0, 0.0)),
                spot(vec3f(2.0, 0.0, 0.0)),
            ],
            &[true; 3],
            Vec3f::default(),
        );
        assert_eq!(s.records[1].count, 0, "a newcomer barely ahead does not evict an owner");
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
        assert_eq!(s.records[0].count, 0, "all six faces or none: unshadowed");
        s.set_config(LocalShadowConfig {
            max_faces: 6,
            ..Default::default()
        });
        s.prepare(&[p], &[true], Vec3f::default());
        assert_eq!(s.records[0].count, 6);
        assert_eq!(s.data.len(), 6 * FACE_FLOATS);
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
            assert_eq!(shadows.records[0].count,0,"insufficient atlas space lights it unshadowed instead of going dark");
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
