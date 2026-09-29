//! World streaming: a large static world as CHUNKS and HLOD CELLS that load
//! and unload around the viewer, built off the UI thread, drawn with
//! distance LOD, dithered crossfades and a coarse occlusion pass.
//!
//! The renderer knows nothing about what a chunk IS. A host hands it a
//! [`TileSource`] — a city, a forest, a terrain — which answers geometry
//! questions on worker threads:
//!
//! - every chunk has two meshes: NEAR (full detail) and PROXY (a few
//!   hundred triangles whose silhouette matches the near mesh);
//! - chunks are grouped into CELLS, each with one merged HLOD mesh that is
//!   always resident (the skyline you see from across the river);
//! - chunks carry instanced PROPS (repeated models drawn as one instanced
//!   draw per kind, each kind with its own draw distance) and OCCLUDER
//!   boxes for the occlusion raster;
//! - the source may add MOVERS each frame (ambient traffic).
//!
//! This file is the device-free half: the trait, the mesh payload, the
//! per-frame SELECTION (which piece draws, with which dither window), the
//! load/unload policy, and the occlusion raster. The GPU half — workers,
//! uploads, the draw lane, shadow casters — is `renderer/stream_draw.rs`.
//!
//! Law (AGENTS.md): no bound here fails. Upload budgets, worker queues,
//! occluder counts and the memory budget only ever make detail arrive
//! LATER or FARTHER — the cell HLOD is always the fallback, so a slow disk,
//! a slow machine or a huge world shows a coarser skyline, never a hole.

use makepad_draw::*;
use std::sync::Arc;

/// One draw layer of a streamed mesh: packed `MODEL_VERTEX_FLOATS`
/// vertices, triangle indices and the albedo it samples. Textures are
/// shared `Arc`s — every chunk of a city names the same dozen images, and
/// the renderer's content-keyed cache uploads each once.
#[derive(Clone)]
pub struct StreamLayer {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
    pub texture: Arc<crate::material_surface::PreparedTexture>,
    /// A tiling detail overlay (mean-grey, multiplied 2x) and its UV scale:
    /// close-up texel density without a bigger albedo.
    pub detail: Option<(Arc<crate::material_surface::PreparedTexture>, [f32; 2])>,
    /// Renders into the sun's shadow cascades (walls and roofs do; the
    /// ground a shadow falls on does not).
    pub casts: bool,
    /// How the layer shades in the streamed lane (`DrawSceneCity`).
    pub material: StreamMaterial,
}

/// The streamed lane's surface model. `orm` is a per-texel map sampled on
/// the albedo's UVs; what its channels mean depends on `kind`:
///
/// | kind | R | G | B |
/// |---|---|---|---|
/// | `Plain` | instance-tint weight | roughness | metallic |
/// | `Facade` / `Glass` | glass coverage | roughness | frame metallic, or on glass the reflectance (F0 = 0.04..0.5) |
/// | `Paint` | instance-tint weight | base roughness | metallic flake (under a clear coat) |
/// | `Emissive` | instance-tint weight | daytime emission | night emission |
///
/// `Facade` glass shows a room behind each window (interior mapping), plain
/// `Glass` shows its albedo behind the reflection. `None` for `orm` is a
/// white map.
#[derive(Clone, Default)]
pub struct StreamMaterial {
    pub kind: StreamSurface,
    pub orm: Option<Arc<crate::material_surface::PreparedTexture>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamSurface {
    #[default]
    Plain,
    Facade,
    Glass,
    Paint,
    Emissive,
}

impl StreamSurface {
    /// The code `DrawSceneCity` reads (passed in the layer's metallic slot).
    pub fn code(self) -> f32 {
        match self { StreamSurface::Plain => 0.0, StreamSurface::Facade => 1.0, StreamSurface::Glass => 2.0, StreamSurface::Paint => 3.0, StreamSurface::Emissive => 4.0 }
    }
}

#[derive(Clone)]
pub struct StreamMesh {
    pub layers: Vec<StreamLayer>,
    pub min: Vec3f,
    pub max: Vec3f,
}

impl StreamMesh {
    pub fn triangles(&self) -> usize { self.layers.iter().map(|l| l.indices.len() / 3).sum() }
    /// Geometry bytes (textures are shared and counted once, elsewhere).
    pub fn geometry_bytes(&self) -> usize { self.layers.iter().map(|l| (l.vertices.len() + l.indices.len()) * 4).sum() }
}

/// Pack one vertex in the model lanes' layout (`model.rs`): position,
/// octahedral normal, f16 uv, unorm8 colour (alpha = ambient occlusion),
/// and a neutral AO-atlas uv.
pub fn pack_vertex(out: &mut Vec<f32>, p: [f32; 3], n: [f32; 3], uv: [f32; 2], c: [f32; 4]) {
    let (ox, oy) = crate::skin::oct_encode(vec3f(n[0], n[1], n[2]));
    out.extend_from_slice(&[
        p[0], p[1], p[2],
        makepad_draw::pack_pair_f16(ox, oy),
        makepad_draw::pack_pair_f16(uv[0], uv[1]),
        makepad_draw::pack_unorm8x4(c[0], c[1], c[2], c[3]),
        crate::model::pack_ao_uv(0.0, 0.0),
    ]);
}

/// A piece of the world a worker builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StreamPiece {
    Near(u32),
    Proxy(u32),
    Cell(u32),
    Prop(u16),
}

/// One instanced prop: which kind, where, and its tint (rgb) — alpha is
/// ignored and replaced by the draw's dither window.
#[derive(Clone, Copy, Debug)]
pub struct StreamProp {
    pub kind: u16,
    pub transform: Mat4f,
    pub tint: Vec4f,
}

/// What a streamed world provides. Every method is called from worker
/// threads or once at registration; none may block on the UI thread.
pub trait TileSource: Send + Sync {
    fn chunk_count(&self) -> usize;
    fn chunk_bounds(&self, chunk: usize) -> (Vec3f, Vec3f);
    fn cell_count(&self) -> usize;
    fn cell_bounds(&self, cell: usize) -> (Vec3f, Vec3f);
    fn cell_chunks(&self, cell: usize) -> Vec<u32>;
    /// Number of instanced prop kinds, and how far each draws.
    fn prop_kinds(&self) -> usize;
    fn prop_draw_distance(&self, kind: usize) -> f32;
    fn chunk_props(&self, chunk: usize, out: &mut Vec<StreamProp>);
    /// Big, solid, axis-aligned boxes: what the occlusion raster draws.
    fn chunk_occluders(&self, chunk: usize, out: &mut Vec<(Vec3f, Vec3f)>);
    /// A light a prop kind carries after dark (street lamps): its offset in
    /// the prop's frame, colour, and mount height. None for most kinds.
    fn prop_light(&self, _kind: usize) -> Option<(Vec3f, Vec3f, f32)> { None }
    /// Per-frame moving instances (ambient traffic) near `eye` at `time`.
    fn movers(&self, _time: f64, _eye: Vec3f, _radius: f32, _out: &mut Vec<StreamProp>) {}
    /// A forward light a MOVER kind carries after dark (headlights): offset
    /// and direction in the mover's frame, and colour. None for most kinds.
    fn mover_light(&self, _kind: usize) -> Option<(Vec3f, Vec3f, Vec3f)> { None }
    /// Build one piece (worker thread).
    fn build(&self, piece: StreamPiece) -> Result<StreamMesh, String>;
}

/// Distances (metres) and budgets for one streamed world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamSettings {
    /// Near meshes draw inside this distance (chunk AABB to eye)...
    pub near: f32,
    /// ...crossfading to the proxy over this band.
    pub near_fade: f32,
    /// Chunk-level pieces (near or proxy) draw inside this distance; beyond
    /// it the cell HLOD takes the whole cell.
    pub chunks: f32,
    pub cell_fade: f32,
    /// Load a little before a piece is needed; unload well after.
    pub load_margin: f32,
    pub unload_margin: f32,
    /// Seconds of camera motion to look ahead when prioritising loads.
    pub lookahead: f32,
    /// Bytes uploaded per frame (at least one piece always lands).
    pub upload_bytes_per_frame: usize,
    /// Resident geometry budget. Over it, the farthest near meshes are
    /// evicted (they draw as proxies) — detail degrades, nothing fails.
    pub resident_bytes: usize,
    /// Occlusion: draw occluders from chunks within this distance.
    pub occluder_range: f32,
    pub occlusion: bool,
    /// Draw the cell HLOD only (the "before" of every A/B).
    pub hlod: bool,
    pub dither: bool,
}

impl Default for StreamSettings {
    fn default() -> Self {
        StreamSettings {
            near: 300.0, near_fade: 40.0, chunks: 1100.0, cell_fade: 120.0,
            load_margin: 120.0, unload_margin: 320.0, lookahead: 1.2,
            upload_bytes_per_frame: 24 << 20, resident_bytes: 1536 << 20,
            occluder_range: 600.0, occlusion: true, hlod: true, dither: true,
        }
    }
}

/// Distance from `p` to the box (0 inside).
pub fn aabb_distance(min: Vec3f, max: Vec3f, p: Vec3f) -> f32 {
    let dx = (min.x - p.x).max(0.0).max(p.x - max.x);
    let dy = (min.y - p.y).max(0.0).max(p.y - max.y);
    let dz = (min.z - p.z).max(0.0).max(p.z - max.z);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// A dither WINDOW `[lo, hi)` over a per-pixel noise in [0, 1): a pixel
/// shows when its noise falls inside. Complementary windows of the pieces
/// of one crossfade tile the unit interval, so every pixel shows exactly
/// one of them — no double-draw, no hole, no alpha sorting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dither { pub lo: f32, pub hi: f32 }
impl Dither {
    pub const FULL: Dither = Dither { lo: 0.0, hi: 1.0 };
    pub fn is_empty(self) -> bool { self.hi - self.lo < 1.0 / 255.0 }
    pub fn is_full(self) -> bool { self.lo <= 0.0 && self.hi >= 1.0 }
    /// Shader encoding in `color_adjust_ctl.w`: 0 = no dither, else
    /// `1 + lo8 * 256 + hi8`.
    pub fn encode(self) -> f32 {
        if self.is_full() { return 0.0; }
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round();
        1.0 + q(self.lo) * 256.0 + q(self.hi)
    }
}

fn ramp(d: f32, edge: f32, band: f32) -> f32 {
    // 0 well inside `edge`, 1 well beyond, linear across `band`.
    if band <= 0.0 { return if d < edge { 0.0 } else { 1.0 }; }
    ((d - (edge - band * 0.5)) / band).clamp(0.0, 1.0)
}

/// What one frame draws: pieces with their dither windows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    /// (chunk, near?, window)
    pub chunks: Vec<(u32, bool, Dither)>,
    /// (cell, window)
    pub cells: Vec<(u32, Dither)>,
}

/// Residency as the selection sees it.
pub trait Residency {
    fn near(&self, chunk: usize) -> bool;
    fn proxy(&self, chunk: usize) -> bool;
    fn cell(&self, cell: usize) -> bool;
}

/// Pick the pieces to draw. Per cell: far → its HLOD; near → its chunks
/// (each near or proxy by distance); in the band between → both, with
/// nested complementary windows. A cell switches to chunks only when every
/// chunk of it has SOMETHING resident; otherwise the HLOD stays — the rule
/// that makes streaming holes impossible.
pub fn select(
    s: &StreamSettings,
    eye: Vec3f,
    chunk_bounds: &[(Vec3f, Vec3f)],
    cells: &[(Vec3f, Vec3f, Vec<u32>)],
    res: &dyn Residency,
    out: &mut Selection,
) {
    out.chunks.clear();
    out.cells.clear();
    for (ci, (cmin, cmax, children)) in cells.iter().enumerate() {
        let dc = aabb_distance(*cmin, *cmax, eye);
        let ready = children.iter().all(|&k| res.near(k as usize) || res.proxy(k as usize));
        // u: how much of the cell is HLOD (1 = all of it).
        let mut u = if s.hlod { ramp(dc, s.chunks, s.cell_fade) } else { 0.0 };
        if !ready { u = 1.0; }
        if !res.cell(ci) {
            // No HLOD yet: children draw whatever they have.
            u = 0.0;
        }
        if u > 0.0 {
            let w = if s.dither { Dither { lo: 1.0 - u, hi: 1.0 } } else { Dither::FULL };
            if u >= 1.0 || !s.dither || !w.is_empty() { out.cells.push((ci as u32, if u >= 1.0 { Dither::FULL } else { w })); }
            if u >= 1.0 || !s.dither { continue; }
        }
        let keep = 1.0 - u;
        for &k in children {
            let ki = k as usize;
            let (lo, hi) = chunk_bounds[ki];
            let d = aabb_distance(lo, hi, eye);
            let (has_near, has_proxy) = (res.near(ki), res.proxy(ki));
            // t: how much of the chunk is proxy.
            let mut t = ramp(d, s.near, s.near_fade);
            if !has_near { t = 1.0; }
            if !has_proxy { t = 0.0; }
            if !has_near && !has_proxy { continue; }
            if !s.dither {
                out.chunks.push((k, t < 0.5, Dither::FULL));
                continue;
            }
            let split = keep * (1.0 - t);
            let near_w = Dither { lo: 0.0, hi: split };
            let proxy_w = Dither { lo: split, hi: keep };
            if !near_w.is_empty() { out.chunks.push((k, true, if near_w.is_full() { Dither::FULL } else { near_w })); }
            if !proxy_w.is_empty() { out.chunks.push((k, false, if proxy_w.is_full() { Dither::FULL } else { proxy_w })); }
        }
    }
}

/// What should be resident, by priority (nearest first). A chunk's near
/// mesh is wanted inside `near + load_margin` of any focus, its proxy
/// inside `chunks + load_margin`; every cell always.
pub fn wanted(s: &StreamSettings, foci: &[Vec3f], chunk_bounds: &[(Vec3f, Vec3f)], cells: &[(Vec3f, Vec3f, Vec<u32>)], out: &mut Vec<(f32, StreamPiece)>) {
    out.clear();
    let dist = |lo: Vec3f, hi: Vec3f| foci.iter().map(|f| aabb_distance(lo, hi, *f)).fold(f32::MAX, f32::min);
    for (i, (lo, hi, _)) in cells.iter().enumerate() {
        // Cells first at equal distance: they are the no-hole fallback.
        out.push((dist(*lo, *hi) * 0.25, StreamPiece::Cell(i as u32)));
    }
    for (i, (lo, hi)) in chunk_bounds.iter().enumerate() {
        let d = dist(*lo, *hi);
        if d < s.chunks + s.load_margin { out.push((d * 0.5, StreamPiece::Proxy(i as u32))); }
        if d < s.near + s.load_margin { out.push((d, StreamPiece::Near(i as u32))); }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
}

/// Should a resident piece go? Only well outside the load radius
/// (hysteresis), and never a cell.
pub fn evictable(s: &StreamSettings, foci: &[Vec3f], bounds: (Vec3f, Vec3f), piece: StreamPiece) -> bool {
    let d = foci.iter().map(|f| aabb_distance(bounds.0, bounds.1, *f)).fold(f32::MAX, f32::min);
    match piece {
        StreamPiece::Near(_) => d > s.near + s.unload_margin,
        StreamPiece::Proxy(_) => d > s.chunks + s.unload_margin,
        StreamPiece::Cell(_) | StreamPiece::Prop(_) => false,
    }
}

// ── occlusion raster ────────────────────────────────────────────────────

/// A coarse software depth buffer of the nearest big occluders (building
/// boxes), and a conservative hi-Z test for chunk and cell boxes.
///
/// Depth is view depth (clip w). Occluders write their exact interpolated
/// depth; uncovered pixels stay at +inf. A box is hidden only when every
/// pixel of its screen rect holds an occluder nearer than the box's nearest
/// corner: 8×8 tile maxima answer whole tiles at once, edge tiles are read
/// pixel by pixel. Missed occluders only ever keep things visible.
pub struct OcclusionRaster {
    pub width: usize,
    pub height: usize,
    depth: Vec<f32>,
    tiles: Vec<f32>,
    clip: Mat4f,
    eye: Vec3f,
    pub occluders: usize,
    pub triangles: usize,
}

const OCC_TILE: usize = 8;
const OCC_NEAR_W: f32 = 0.25;

impl Default for OcclusionRaster {
    fn default() -> Self { Self::new(192, 112) }
}

impl OcclusionRaster {
    pub fn new(width: usize, height: usize) -> Self {
        let (w, h) = (width.max(OCC_TILE), height.max(OCC_TILE));
        OcclusionRaster {
            width: w, height: h, depth: vec![f32::INFINITY; w * h],
            tiles: vec![f32::INFINITY; w.div_ceil(OCC_TILE) * h.div_ceil(OCC_TILE)],
            clip: Mat4f::identity(), eye: Vec3f::default(), occluders: 0, triangles: 0,
        }
    }

    pub fn begin(&mut self, clip: &Mat4f, eye: Vec3f) {
        self.clip = *clip;
        self.eye = eye;
        self.depth.fill(f32::INFINITY);
        self.occluders = 0;
        self.triangles = 0;
    }

    fn to_clip(&self, p: Vec3f) -> Vec4f { self.clip.transform_vec4(vec4(p.x, p.y, p.z, 1.0)) }

    /// Rasterize one solid box: only its camera-facing faces (at most
    /// three), the back ones can never be nearest.
    pub fn add_box(&mut self, min: Vec3f, max: Vec3f) {
        let c = |i: usize| vec3f(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        );
        let corners: [Vec4f; 8] = std::array::from_fn(|i| self.to_clip(c(i)));
        // Faces as corner quads (winding irrelevant: the rasterizer fills
        // both), each with the test that the eye is on its outer side.
        const FACES: [[usize; 4]; 6] = [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]];
        let e = self.eye;
        let facing = [e.z < min.z, e.z > max.z, e.y < min.y, e.y > max.y, e.x < min.x, e.x > max.x];
        for (f, front) in FACES.iter().zip(facing) {
            if !front { continue; }
            let poly = [corners[f[0]], corners[f[1]], corners[f[2]], corners[f[3]]];
            self.polygon(&poly);
        }
        self.occluders += 1;
    }

    fn polygon(&mut self, poly: &[Vec4f; 4]) {
        // Clip against w >= OCC_NEAR_W (the near plane) — a wall that runs
        // past the camera is the best occluder there is.
        let mut buf = [Vec4f::default(); 8];
        let mut n = 0;
        for i in 0..4 {
            let a = poly[i];
            let b = poly[(i + 1) % 4];
            let (ina, inb) = (a.w >= OCC_NEAR_W, b.w >= OCC_NEAR_W);
            if ina { buf[n] = a; n += 1; }
            if ina != inb {
                let t = (OCC_NEAR_W - a.w) / (b.w - a.w);
                buf[n] = vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, OCC_NEAR_W);
                n += 1;
            }
        }
        if n < 3 { return; }
        let (w, h) = (self.width as f32, self.height as f32);
        let sp: Vec<(f32, f32, f32)> = buf[..n].iter().map(|v| {
            let iw = 1.0 / v.w;
            ((v.x * iw * 0.5 + 0.5) * w, (0.5 - v.y * iw * 0.5) * h, iw)
        }).collect();
        for i in 1..n - 1 {
            self.triangle(sp[0], sp[i], sp[i + 1]);
        }
    }

    fn triangle(&mut self, a: (f32, f32, f32), b: (f32, f32, f32), c: (f32, f32, f32)) {
        let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if area.abs() < 1e-6 { return; }
        let x0 = a.0.min(b.0).min(c.0).floor().max(0.0) as i32;
        let x1 = a.0.max(b.0).max(c.0).ceil().min(self.width as f32 - 1.0) as i32;
        let y0 = a.1.min(b.1).min(c.1).floor().max(0.0) as i32;
        let y1 = a.1.max(b.1).max(c.1).ceil().min(self.height as f32 - 1.0) as i32;
        if x1 < x0 || y1 < y0 { return; }
        self.triangles += 1;
        let inv = 1.0 / area;
        for y in y0..=y1 {
            let py = y as f32 + 0.5;
            let row = y as usize * self.width;
            for x in x0..=x1 {
                let px = x as f32 + 0.5;
                // Barycentrics (sign-agnostic: both windings fill).
                let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) * inv;
                let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) * inv;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                // 1/w interpolates linearly in screen space.
                let iw = w0 * a.2 + w1 * b.2 + w2 * c.2;
                if iw <= 0.0 { continue; }
                let d = 1.0 / iw;
                let slot = &mut self.depth[row + x as usize];
                if d < *slot { *slot = d; }
            }
        }
    }

    /// Build the tile maxima after the last occluder.
    pub fn finish(&mut self) {
        let tw = self.width.div_ceil(OCC_TILE);
        let th = self.height.div_ceil(OCC_TILE);
        for ty in 0..th {
            for tx in 0..tw {
                let mut m: f32 = 0.0;
                for y in ty * OCC_TILE..((ty + 1) * OCC_TILE).min(self.height) {
                    for x in tx * OCC_TILE..((tx + 1) * OCC_TILE).min(self.width) {
                        m = m.max(self.depth[y * self.width + x]);
                    }
                }
                self.tiles[ty * tw + tx] = m;
            }
        }
    }

    /// True when the box is certainly hidden behind the rasterized occluders.
    pub fn occluded(&self, min: Vec3f, max: Vec3f) -> bool {
        if self.occluders == 0 { return false; }
        let (w, h) = (self.width as f32, self.height as f32);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut near = f32::MAX;
        for i in 0..8 {
            let p = vec3f(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            let c = self.to_clip(p);
            if c.w < OCC_NEAR_W { return false; }
            let iw = 1.0 / c.w;
            let (sx, sy) = ((c.x * iw * 0.5 + 0.5) * w, (0.5 - c.y * iw * 0.5) * h);
            x0 = x0.min(sx); x1 = x1.max(sx); y0 = y0.min(sy); y1 = y1.max(sy);
            near = near.min(c.w);
        }
        // Off screen entirely is the frustum's call, not ours.
        if x1 < 0.0 || y1 < 0.0 || x0 >= w || y0 >= h { return false; }
        // Pixel rect, clamped to the screen.
        let px0 = x0.max(0.0).floor() as usize;
        let py0 = y0.max(0.0).floor() as usize;
        let px1 = (x1.min(w - 1.0).max(0.0)) as usize;
        let py1 = (y1.min(h - 1.0).max(0.0)) as usize;
        let tw = self.width.div_ceil(OCC_TILE);
        for ty in py0 / OCC_TILE..=py1 / OCC_TILE {
            for tx in px0 / OCC_TILE..=px1 / OCC_TILE {
                // Whole tile inside the rect: its maximum decides. A tile
                // the rect only clips is tested pixel by pixel, so the
                // uncovered half of an edge tile cannot veto a hidden box.
                if self.tiles[ty * tw + tx] < near { continue; }
                let (ax, bx) = ((tx * OCC_TILE).max(px0), ((tx + 1) * OCC_TILE - 1).min(px1));
                let (ay, by) = ((ty * OCC_TILE).max(py0), ((ty + 1) * OCC_TILE - 1).min(py1));
                for y in ay..=by {
                    for x in ax..=bx {
                        if self.depth[y * self.width + x] >= near { return false; }
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct All(bool, bool, bool);
    impl Residency for All {
        fn near(&self, _: usize) -> bool { self.0 }
        fn proxy(&self, _: usize) -> bool { self.1 }
        fn cell(&self, _: usize) -> bool { self.2 }
    }

    fn grid() -> (Vec<(Vec3f, Vec3f)>, Vec<(Vec3f, Vec3f, Vec<u32>)>) {
        // 8x1 chunks of 128 m in two cells of 4.
        let chunks: Vec<(Vec3f, Vec3f)> = (0..8).map(|i| (vec3f(i as f32 * 128.0, 0.0, 0.0), vec3f((i + 1) as f32 * 128.0, 50.0, 128.0))).collect();
        let cells = vec![
            (vec3f(0.0, 0.0, 0.0), vec3f(512.0, 50.0, 128.0), vec![0, 1, 2, 3]),
            (vec3f(512.0, 0.0, 0.0), vec3f(1024.0, 50.0, 128.0), vec![4, 5, 6, 7]),
        ];
        (chunks, cells)
    }

    #[test]
    fn windows_tile_the_unit_interval_for_every_distance() {
        let (chunks, cells) = grid();
        let s = StreamSettings { near: 200.0, near_fade: 60.0, chunks: 500.0, cell_fade: 100.0, ..Default::default() };
        let mut sel = Selection::default();
        for x in (-900..1900).step_by(7) {
            select(&s, vec3f(x as f32, 10.0, 64.0), &chunks, &cells, &All(true, true, true), &mut sel);
            for (ci, (_, _, kids)) in cells.iter().enumerate() {
                let cell_w: f32 = sel.cells.iter().filter(|(c, _)| *c as usize == ci).map(|(_, w)| w.hi - w.lo).sum();
                for k in kids {
                    let kw: f32 = sel.chunks.iter().filter(|(c, _, _)| c == k).map(|(_, _, w)| w.hi - w.lo).sum();
                    assert!((cell_w + kw - 1.0).abs() < 1e-4, "x {x} cell {ci} chunk {k}: {cell_w} + {kw}");
                }
            }
        }
    }

    #[test]
    fn a_cell_holds_its_hlod_until_every_chunk_has_something() {
        let (chunks, cells) = grid();
        let s = StreamSettings::default();
        let mut sel = Selection::default();
        select(&s, vec3f(10.0, 2.0, 60.0), &chunks, &cells, &All(false, false, true), &mut sel);
        assert!(sel.chunks.is_empty());
        assert_eq!(sel.cells.len(), 2);
        assert!(sel.cells.iter().all(|(_, w)| w.is_full()));
        // Nothing resident at all draws nothing and does not panic.
        select(&s, vec3f(10.0, 2.0, 60.0), &chunks, &cells, &All(false, false, false), &mut sel);
        assert!(sel.chunks.is_empty() && sel.cells.is_empty());
    }

    #[test]
    fn wanted_puts_cells_and_nearby_detail_first() {
        let (chunks, cells) = grid();
        let s = StreamSettings { near: 150.0, load_margin: 50.0, chunks: 400.0, ..Default::default() };
        let mut w = Vec::new();
        wanted(&s, &[vec3f(0.0, 2.0, 64.0)], &chunks, &cells, &mut w);
        assert!(matches!(w[0].1, StreamPiece::Cell(0) | StreamPiece::Proxy(0) | StreamPiece::Near(0)));
        assert!(w.iter().any(|(_, p)| *p == StreamPiece::Near(1)));
        assert!(!w.iter().any(|(_, p)| *p == StreamPiece::Near(4)));
        assert!(w.iter().filter(|(_, p)| matches!(p, StreamPiece::Cell(_))).count() == 2);
        assert!(evictable(&s, &[vec3f(5000.0, 0.0, 0.0)], chunks[0], StreamPiece::Near(0)));
        assert!(!evictable(&s, &[vec3f(5000.0, 0.0, 0.0)], cells[0].0.min_max(), StreamPiece::Cell(0)));
    }

    trait MinMax { fn min_max(&self) -> (Vec3f, Vec3f); }
    impl MinMax for Vec3f { fn min_max(&self) -> (Vec3f, Vec3f) { (*self, *self) } }

    #[test]
    fn dither_encoding_round_trips_in_the_shader_convention() {
        let d = Dither { lo: 0.25, hi: 0.75 };
        let v = d.encode() - 1.0;
        let lo = (v / 256.0).floor();
        let hi = v - lo * 256.0;
        assert!((lo / 255.0 - 0.25).abs() < 0.003 && (hi / 255.0 - 0.75).abs() < 0.003);
        assert_eq!(Dither::FULL.encode(), 0.0);
    }

    /// How far from the origin an f32 world stays sub-pixel: a point 2 m in
    /// front of a camera at distance `x`, transformed world -> clip in f32
    /// (what the GPU does) vs f64, as screen pixels at 1920 px / 60 deg.
    /// A camera moving 1 mm per frame is jitter only when this error is a
    /// visible fraction of a pixel.
    #[test]
    fn f32_world_precision_budget() {
        let err_px = |x: f64| -> f64 {
            let mut worst: f64 = 0.0;
            for k in 0..64 {
                let o = x + k as f64 * 0.013;
                let eye = [o, 1.7, o * 0.5];
                let p = [o + 2.0 * 0.6, 1.2, o * 0.5 - 2.0 * 0.8];
                // f64 truth, relative.
                let rel64 = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
                // f32 path: world position and view translation each rounded.
                let rel32 = [(p[0] as f32 - eye[0] as f32) as f64, (p[1] as f32 - eye[1] as f32) as f64, (p[2] as f32 - eye[2] as f32) as f64];
                let d = (rel64[0] * rel64[0] + rel64[1] * rel64[1] + rel64[2] * rel64[2]).sqrt();
                let e = ((rel32[0] - rel64[0]).powi(2) + (rel32[1] - rel64[1]).powi(2) + (rel32[2] - rel64[2]).powi(2)).sqrt();
                let px_per_rad = 1920.0 / (60f64.to_radians());
                worst = worst.max(e / d * px_per_rad);
            }
            worst
        };
        let (a, b, c) = (err_px(1500.0), err_px(5000.0), err_px(10000.0));
        eprintln!("f32 screen error for a subject 2 m away: 1.5 km {a:.3} px, 5 km {b:.3} px, 10 km {c:.3} px");
        assert!(a < 0.2, "a 3 km city centred on the origin must be sub-pixel: {a}");
    }

    fn look(eye: Vec3f, target: Vec3f) -> Mat4f {
        let view = Mat4f::look_at(eye, target, vec3f(0.0, 1.0, 0.0));
        let proj = Mat4f::perspective(60.0, 16.0 / 9.0, 0.5, 5000.0);
        Mat4f::mul(&proj, &view)
    }

    #[test]
    fn a_wall_hides_what_stands_behind_it_and_nothing_else() {
        let mut r = OcclusionRaster::default();
        r.begin(&look(vec3f(0.0, 2.0, 0.0), vec3f(0.0, 2.0, -100.0)), vec3f(0.0, 2.0, 0.0));
        // A 200 m wide, 20 m tall wall 60 m ahead (its top 17 degrees up,
        // inside the 30 degree half field of view).
        r.add_box(vec3f(-100.0, 0.0, -62.0), vec3f(100.0, 20.0, -60.0));
        r.finish();
        assert!(r.occluded(vec3f(-10.0, 0.0, -200.0), vec3f(10.0, 40.0, -180.0)), "box behind the wall");
        assert!(!r.occluded(vec3f(-10.0, 0.0, -20.0), vec3f(10.0, 40.0, -10.0)), "box in front of the wall");
        assert!(!r.occluded(vec3f(-10.0, 0.0, -200.0), vec3f(10.0, 120.0, -180.0)), "a tower rising above the wall");
        // A wall running past the camera (clipped at the near plane) still occludes.
        let mut r = OcclusionRaster::default();
        r.begin(&look(vec3f(0.0, 2.0, 0.0), vec3f(-100.0, 2.0, -100.0)), vec3f(0.0, 2.0, 0.0));
        r.add_box(vec3f(-5.0, 0.0, -500.0), vec3f(-4.0, 300.0, 500.0));
        r.finish();
        assert!(r.occluded(vec3f(-60.0, 0.0, -60.0), vec3f(-40.0, 20.0, -40.0)));
    }
}
