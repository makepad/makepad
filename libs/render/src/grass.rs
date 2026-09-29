//! GPU grass: blades grown in the vertex stage from a ground field.
//!
//! A [`GrassField`] is two rasters over the level (1 m texels by default):
//! the ground height the blades stand on, and per texel a DENSITY (0..1,
//! from the terrain's paint masks and the road bands the level marks as
//! grass) plus a DRYNESS that mixes the lush and dry blade colours. The
//! level builds it off the frame (libs/gen `level::grass`), ships it as the
//! bytes of [`GrassField::encode`], and the host hands it to
//! [`crate::renderer::Renderer::set_grass`].
//!
//! Drawing it costs no per-blade CPU work at all. The CPU walks the square
//! patches (PATCH metres) within the draw radius, drops the ones outside
//! the frustum or with no grass in them, and adds ONE instance per patch;
//! a patch's blades are a fixed geometry (random roots in the unit square,
//! sorted by a keep threshold) that the vertex shader places on the field:
//! height from the height raster, density test against the cover raster,
//! colour, lean and wind sway. Three geometries (all blades, the first
//! half, the first quarter) serve the near, middle and far rings, and the
//! shader's own keep test fades density with distance on top, so a ring
//! change never pops: a far patch draws exactly the blades the near
//! geometry would still keep there. At the radius the blades have shrunk
//! into the ground and the terrain's own grass texture carries on.
//!
//! The shader (DrawSceneGrass) is DrawScenePbr's sibling: the same lane
//! binding (sun, fog, cascades, clustered lights), no discard (the blades
//! are opaque geometry, so a tile GPU keeps its hidden-surface removal), and
//! two of the PBR lane's texture slots the blades do not use carry the
//! rasters (slot 0 height, slot 1 cover): the model shaders are at the
//! texture-slot limit.

use makepad_draw::makepad_platform::*;

/// Patch side in metres (one instance each).
pub const PATCH: f32 = 2.0;
/// Blades per unit-density square metre in the densest ring.
const BLADES_PER_M2: f32 = 36.0;

/// Blade look and reach, authored by `level.grass`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrassBlades {
    /// Metres, before the per-blade 0.55..1.25 variation.
    pub height: f32,
    /// Base width in metres (the blade tapers to a point).
    pub width: f32,
    /// Multiplies the blade count (1 = BLADES_PER_M2 at full cover).
    pub density: f32,
    /// Draw radius in metres; the blades are gone at it.
    pub radius: f32,
    /// 0..1 sway.
    pub wind: f32,
    /// sRGB of a lush and of a dry blade (mixed by the raster).
    pub lush: [f32; 3],
    pub dry: [f32; 3],
}

impl Default for GrassBlades {
    fn default() -> Self {
        Self { height: 0.32, width: 0.05, density: 1.0, radius: 42.0, wind: 0.5, lush: [0.30, 0.45, 0.16], dry: [0.55, 0.53, 0.30] }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GrassField {
    /// World x, z of texel (0, 0)'s corner.
    pub origin: [f32; 2],
    pub cell: f32,
    pub nx: usize,
    pub nz: usize,
    /// Ground height per texel (row-major, z rows).
    pub height: Vec<f32>,
    /// Per texel: density (0..255) and dryness (0..255).
    pub cover: Vec<[u8; 2]>,
    pub blades: GrassBlades,
}

const MAGIC: &[u8; 8] = b"MPGRASS1";

impl GrassField {
    pub fn cover_at(&self, x: f32, z: f32) -> Option<[u8; 2]> {
        let (ix, iz) = (((x - self.origin[0]) / self.cell).floor(), ((z - self.origin[1]) / self.cell).floor());
        if ix < 0.0 || iz < 0.0 || ix as usize >= self.nx || iz as usize >= self.nz { return None; }
        self.cover.get(iz as usize * self.nx + ix as usize).copied()
    }

    /// The level product's bytes: a fixed little-endian layout.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(96 + self.height.len() * 6);
        out.extend_from_slice(MAGIC);
        for v in [self.nx as u32, self.nz as u32] { out.extend_from_slice(&v.to_le_bytes()); }
        let b = &self.blades;
        let floats = [self.origin[0], self.origin[1], self.cell, b.height, b.width, b.density, b.radius, b.wind,
            b.lush[0], b.lush[1], b.lush[2], b.dry[0], b.dry[1], b.dry[2]];
        for f in floats { out.extend_from_slice(&f.to_le_bytes()); }
        for h in &self.height { out.extend_from_slice(&h.to_le_bytes()); }
        for c in &self.cover { out.extend_from_slice(c); }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() < 16 + 14 * 4 || &bytes[..8] != MAGIC { return Err("grass field: not a grass product".into()); }
        let u = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        let f = |at: usize| f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (nx, nz) = (u(8), u(12));
        let n = nx.checked_mul(nz).filter(|n| *n > 0 && *n <= 16 << 20).ok_or("grass field: size")?;
        let head = 16 + 14 * 4;
        if bytes.len() != head + n * 6 { return Err("grass field: length".into()); }
        let v: Vec<f32> = (0..14).map(|i| f(16 + i * 4)).collect();
        if v.iter().any(|x| !x.is_finite()) || v[2] <= 0.0 { return Err("grass field: values".into()); }
        let height = (0..n).map(|i| f(head + i * 4)).collect();
        let cover = bytes[head + n * 4..].chunks_exact(2).map(|c| [c[0], c[1]]).collect();
        Ok(Self {
            origin: [v[0], v[1]], cell: v[2], nx, nz, height, cover,
            blades: GrassBlades { height: v[3], width: v[4], density: v[5], radius: v[6], wind: v[7], lush: [v[8], v[9], v[10]], dry: [v[11], v[12], v[13]] },
        })
    }
}

/// The resident half: the rasters as textures, the per-patch cover summary
/// the CPU culls with, and the three ring geometries.
pub(crate) struct GrassGpu {
    pub field: std::sync::Arc<GrassField>,
    /// The height raster's quantisation: min and range (metres).
    pub height_lo: f32,
    pub height_range: f32,
    pub height_tex: Texture,
    pub cover_tex: Texture,
    /// Per patch: (max density 0..255, min ground, max ground).
    pub patches: Vec<(u8, f32, f32)>,
    pub px: usize,
    pub pz: usize,
    /// Near (every blade), middle (first half), far (first quarter).
    pub rings: [Geometry; 3],
}

/// Floats per vertex in the `geom.GameMeshVertexAo` layout the PBR lane's
/// vertex buffer declares; the grass vertex stage reads its own meaning
/// into them: root x, t along the blade, root z, side, keep threshold,
/// shape random, yaw random.
const VERTEX_FLOATS: usize = 7;

fn hash(i: u32, salt: u32) -> f32 {
    let mut n = i.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    n ^= n >> 15;
    n = n.wrapping_mul(0x2C1B_3C6D);
    n ^= n >> 12;
    n = n.wrapping_mul(0x297A_2D39);
    n ^= n >> 15;
    (n >> 8) as f32 / (1u32 << 24) as f32
}

/// The first `take` of a patch's `blades`, sorted by keep threshold: the
/// half and the quarter are the sparser rings EXACTLY (the same roots and
/// thresholds as the near patch's first blades). Near blades are a 3-quad
/// strip (they bend), the other rings fewer segments.
pub(crate) fn patch_vertices(blades: usize, take: usize, segments: usize) -> (Vec<f32>, Vec<u32>) {
    let mut roots: Vec<(f32, f32, f32, f32, f32)> = (0..blades as u32).map(|i| {
        // Jittered grid roots: even cover without clumps of three.
        let side = (blades as f32).sqrt().ceil() as u32;
        let (gx, gz) = ((i % side) as f32, (i / side) as f32);
        let x = ((gx + hash(i, 1)) / side as f32).fract();
        let z = ((gz + hash(i, 2)) / side as f32).fract();
        (hash(i, 3), x, z, hash(i, 4), hash(i, 5))
    }).collect();
    roots.sort_by(|a, b| a.0.total_cmp(&b.0));
    roots.truncate(take);
    let blades = roots.len();
    let mut verts = Vec::with_capacity(blades * (segments * 2 + 1) * VERTEX_FLOATS);
    let mut idx = Vec::with_capacity(blades * (segments * 2 - 1) * 3);
    for (keep, x, z, shape, yaw) in roots {
        let base = (verts.len() / VERTEX_FLOATS) as u32;
        for s in 0..segments {
            let t = s as f32 / segments as f32;
            for side in [-1.0f32, 1.0] { verts.extend_from_slice(&[x, t, z, side, keep, shape, yaw]); }
        }
        verts.extend_from_slice(&[x, 1.0, z, 0.0, keep, shape, yaw]);
        for s in 0..segments as u32 {
            let (a, b) = (base + s * 2, base + s * 2 + 1);
            if s + 1 < segments as u32 {
                let (c, d) = (a + 2, b + 2);
                idx.extend_from_slice(&[a, b, c, b, d, c]);
            } else {
                idx.extend_from_slice(&[a, b, base + segments as u32 * 2]);
            }
        }
    }
    (verts, idx)
}

impl GrassGpu {
    pub fn new(cx: &mut Cx, field: std::sync::Arc<GrassField>) -> Self {
        let (nx, nz) = (field.nx, field.nz);
        // Height as 16 bits in BGRA8 (the one format every backend samples
        // in the vertex stage; R32F and RG8 read back zero there on Vulkan):
        // hi byte in G, lo byte in A, over [min, min + range].
        let (lo, hi) = field.height.iter().fold((f32::MAX, f32::MIN), |(a, b), h| (a.min(*h), b.max(*h)));
        let (lo, range) = if lo <= hi { (lo, (hi - lo).max(0.01)) } else { (0.0, 1.0) };
        let height: Vec<u32> = field.height.iter().map(|h| {
            let q = (((h - lo) / range) * 65535.0).round().clamp(0.0, 65535.0) as u32;
            ((q & 0xFF) << 24) | ((q >> 8) << 16) | ((q >> 8) << 8) | (q >> 8)
        }).collect();
        let height_tex = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
            width: nx, height: nz, data: Some(height), updated: TextureUpdated::Full,
        });
        // BGRA8 (the format every backend samples in every stage): density
        // in both R and B, so the lane order of a BGRA read cannot swap it;
        // dryness in G.
        let cover: Vec<u32> = field.cover.iter().map(|c| 0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[0] as u32).collect();
        let cover_tex = Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
            width: nx, height: nz, data: Some(cover), updated: TextureUpdated::Full,
        });
        // Per-patch summary: the densest texel (an empty patch is never
        // drawn) and the ground range (the patch's culling box).
        let texels = (PATCH / field.cell).ceil().max(1.0) as usize;
        let (px, pz) = (nx.div_ceil(texels), nz.div_ceil(texels));
        let mut patches = vec![(0u8, f32::MAX, f32::MIN); px * pz];
        for z in 0..nz {
            for x in 0..nx {
                let p = &mut patches[(z / texels) * px + x / texels];
                let i = z * nx + x;
                p.0 = p.0.max(field.cover[i][0]);
                p.1 = p.1.min(field.height[i]);
                p.2 = p.2.max(field.height[i]);
            }
        }
        let area = PATCH * PATCH;
        let full = ((BLADES_PER_M2 * field.blades.density.clamp(0.1, 4.0) * area) as usize).clamp(8, 4096);
        let mut ring = |take: usize, segments: usize| {
            let (v, i) = patch_vertices(full, take, segments);
            let g = Geometry::new(cx);
            g.update(cx, i, v);
            g
        };
        let rings = [ring(full, 3), ring(full / 2, 2), ring(full / 4, 1)];
        Self { field, height_lo: lo, height_range: range, height_tex, cover_tex, patches, px, pz, rings }
    }

    /// Patch origins to draw this frame, by ring: (x, z) of each patch's
    /// corner. The frustum and emptiness tests are all the CPU does.
    pub fn visible(&self, eye: Vec3f, frustum: Option<&crate::renderer::Frustum>, out: &mut [Vec<(f32, f32)>; 3]) {
        for ring in out.iter_mut() { ring.clear(); }
        let f = &self.field;
        let radius = f.blades.radius.max(PATCH);
        let span = PATCH;
        let (x0, z0) = (((eye.x - radius - f.origin[0]) / span).floor().max(0.0) as usize, ((eye.z - radius - f.origin[1]) / span).floor().max(0.0) as usize);
        let (x1, z1) = ((((eye.x + radius - f.origin[0]) / span).ceil().max(0.0)) as usize, (((eye.z + radius - f.origin[1]) / span).ceil().max(0.0)) as usize);
        let height = f.blades.height * 1.4;
        for pz in z0..z1.min(self.pz) {
            for px in x0..x1.min(self.px) {
                let (dens, lo, hi) = self.patches[pz * self.px + px];
                if dens < 8 { continue; }
                let (x, z) = (f.origin[0] + px as f32 * span, f.origin[1] + pz as f32 * span);
                let (cx, cz) = (x + span * 0.5 - eye.x, z + span * 0.5 - eye.z);
                let d = (cx * cx + cz * cz).sqrt() - span * 0.71;
                if d > radius { continue; }
                // Grass far below or above the eye (a bridge, a cliff top)
                // is still in reach; only the plan distance rings it.
                if let Some(fr) = frustum {
                    if !fr.intersects_aabb(vec3f(x, lo - 0.2, z), vec3f(x + span, hi + height, z + span)) { continue; }
                }
                let ring = if d < radius * 0.3 { 0 } else if d < radius * 0.6 { 1 } else { 2 };
                out[ring].push((x, z));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_round_trips_through_its_product_bytes() {
        let field = GrassField {
            origin: [-10.0, -20.0], cell: 1.0, nx: 3, nz: 2,
            height: vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.5], cover: vec![[0, 0], [255, 10], [128, 200], [1, 2], [3, 4], [5, 6]],
            blades: GrassBlades { height: 0.4, radius: 30.0, ..Default::default() },
        };
        let back = GrassField::decode(&field.encode()).unwrap();
        assert_eq!(back, field);
        assert_eq!(back.cover_at(-7.5, -19.5), Some([128, 200]));
        assert_eq!(back.cover_at(-11.0, -19.5), None);
        assert!(GrassField::decode(&field.encode()[..40]).is_err());
    }

    #[test]
    fn sparser_rings_are_prefixes_of_the_near_patch() {
        let (near, ni) = patch_vertices(64, 64, 3);
        let (mid, mi) = patch_vertices(64, 32, 2);
        assert_eq!(ni.len(), 64 * 5 * 3);
        assert_eq!(mi.len(), 32 * 3 * 3);
        // Blade k of every ring has the same root and keep threshold.
        let root = |v: &[f32], per: usize, k: usize| (v[k * per * VERTEX_FLOATS], v[k * per * VERTEX_FLOATS + 2], v[k * per * VERTEX_FLOATS + 4]);
        let keeps: Vec<f32> = (0..64).map(|k| root(&near, 7, k).2).collect();
        assert!(keeps.windows(2).all(|w| w[0] <= w[1]), "sorted by keep");
        for k in 0..32 { assert_eq!(root(&near, 7, k), root(&mid, 5, k)); }
    }
}
