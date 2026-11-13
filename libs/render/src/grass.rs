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
//! patches (PATCH metres) in reach, block by block, drops the ones outside
//! the frustum or with no grass in them, and adds ONE instance per patch;
//! a patch's blades are a fixed geometry (random roots in the unit square,
//! sorted by a keep threshold) that the vertex shader places on the field:
//! height from the height raster, density test against the cover raster,
//! clumps, colour, lean and the world wind.
//!
//! The screen sets the density, not a radius in metres ([`GrassLod`]): out
//! to `full` metres every blade grows, where a blade of the authored width
//! is the tier's `pixels` wide on screen; past it the kept fraction falls
//! with the square of the distance and the kept blades widen to keep the
//! cover, so the blade count follows the image's resolution. LEVELS
//! geometries (every blade, then a half, a quarter, an eighth and a
//! sixteenth, with fewer segments each) are prefixes of one sorted list:
//! a patch draws the smallest that holds every blade it can keep, so a
//! level change never changes the picture, and a blade that the shader
//! starts to keep grows out of the ground instead of appearing. The
//! farthest blades sink into the ground, and the terrain's paint carries
//! the meadow on as a carpet in the blades' colour and light
//! (shaders/splat.rs `grass_carpet`, under the blades from 40 % of the
//! reach, all of the cover from 80 %).
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
/// Blade width per square metre of ground at density 1, metres: narrow
/// blades grow more thickly, so `density` means how much of the ground
/// they cover (5 cm blades: 400 per square metre, a carpet that hides the
/// ground between its tufts at walking distance).
const COVER_PER_M2: f32 = 20.0;
/// The kept-blade prefixes: every blade, a half, a quarter, an eighth and
/// a sixteenth, each with fewer segments per blade.
pub(crate) const LEVELS: usize = 5;
const SEGMENTS: [usize; LEVELS] = [5, 4, 3, 2, 1];
/// Patches per block side: the CPU tests a block before its patches.
const BLOCK: usize = 8;
/// Where the farthest blades end, in `GrassLod::full`s.
const REACH: f32 = 3.0;
/// The widest a thinned blade grows, in authored widths (the shader's
/// `grass_lod.z`): past it the cover thins and the carpet takes over.
pub(crate) const WIDTH_CAP: f32 = 4.0;
/// How far the shader may move a root toward its clump's centre, metres:
/// the slack around a patch's roots.
const ROOT_SLACK: f32 = 0.4;

/// Blade look and reach, authored by `level.grass`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrassBlades {
    /// Metres, before the per-blade 0.55..1.25 variation.
    pub height: f32,
    /// Base width in metres (the blade tapers to a point).
    pub width: f32,
    /// Multiplies the blade count (1 = COVER_PER_M2 / width at full cover).
    pub density: f32,
    /// The farthest the blades reach, metres (the screen may end them
    /// nearer: [`GrassLod`]).
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

/// The screen's say in a field: how far every blade grows and where the
/// last ones end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GrassLod {
    /// Metres out to which every blade is kept: where a blade of the
    /// authored width is `pixels` wide on screen.
    pub full: f32,
    /// Metres at which the last blades have sunk into the ground.
    pub reach: f32,
}

impl GrassLod {
    /// `focal_px`: the view's focal length in pixels (the image height over
    /// twice the tangent of half the vertical field of view); `pixels`: the
    /// tier's narrowest blade on screen.
    pub fn new(blades: &GrassBlades, focal_px: f32, pixels: f32) -> Self {
        let full = (blades.width * focal_px / pixels.max(0.25)).max(0.5);
        Self { full, reach: (full * REACH).min(blades.radius).max(PATCH) }
    }

    /// The fraction of the blades kept at `d` metres from the eye (the
    /// shader's `kept`).
    pub fn kept(&self, d: f32) -> f32 {
        let r = self.full / d.max(1.0e-3);
        (r * r).min(1.0)
    }
}

/// The resident half: the rasters as textures, the per-patch and per-block
/// cover summaries the CPU culls with, and the LEVELS geometries.
pub(crate) struct GrassGpu {
    pub field: std::sync::Arc<GrassField>,
    /// The ground's height (metres, R32F) and the cover (RG8: density in R,
    /// dryness in G), a texel per field cell.
    pub height_tex: Texture,
    pub cover_tex: Texture,
    pub cull: GrassCull,
    /// Every blade, then the first half, quarter, eighth and sixteenth.
    pub levels: [Geometry; LEVELS],
}

/// What the CPU walks a field with: cover and ground summaries per patch
/// and per block, and the levels' blade counts.
pub(crate) struct GrassCull {
    /// Per patch: (max density 0..255, min ground, max ground).
    pub patches: Vec<(u8, f32, f32)>,
    pub px: usize,
    pub pz: usize,
    /// The same per BLOCK x BLOCK patches.
    blocks: Vec<(u8, f32, f32)>,
    bx: usize,
    /// A patch's blades in each level.
    pub level_blades: [usize; LEVELS],
    /// The smallest keep threshold of a patch: a patch whose kept fraction
    /// is no more keeps no blade.
    first_keep: f32,
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

/// The first `take` of a patch's `blades`, sorted by keep threshold: each
/// sparser level is EXACTLY the first blades of the denser ones (the same
/// roots and thresholds). A blade is a strip of `segments` quads (it bends)
/// ending in a tip; the sparser levels, drawn farther off, use fewer.
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
        let height_tex = Texture::new_with_format(cx, TextureFormat::VecRf32 {
            width: nx, height: nz, data: Some(field.height.clone()), updated: TextureUpdated::Full,
        });
        let cover_tex = Texture::new_with_format(cx, TextureFormat::VecRGu8 {
            width: nx, height: nz, data: Some(field.cover.iter().flatten().copied().collect()), unpack_row_length: None, updated: TextureUpdated::Full,
        });
        let cull = GrassCull::new(&field);
        let full = cull.level_blades[0];
        let levels = std::array::from_fn(|l| {
            let (v, i) = patch_vertices(full, cull.level_blades[l], SEGMENTS[l]);
            let g = Geometry::new(cx);
            g.update(cx, i, v);
            g
        });
        Self { field, height_tex, cover_tex, cull, levels }
    }
}

impl GrassCull {
    pub fn new(field: &GrassField) -> Self {
        let (nx, nz) = (field.nx, field.nz);
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
        // Blocks of BLOCK x BLOCK patches, the same summary.
        let (bx, bz) = (px.div_ceil(BLOCK), pz.div_ceil(BLOCK));
        let mut blocks = vec![(0u8, f32::MAX, f32::MIN); bx * bz];
        for z in 0..pz {
            for x in 0..px {
                let (d, lo, hi) = patches[z * px + x];
                let b = &mut blocks[(z / BLOCK) * bx + x / BLOCK];
                b.0 = b.0.max(d);
                b.1 = b.1.min(lo);
                b.2 = b.2.max(hi);
            }
        }
        let b = &field.blades;
        let per_m2 = COVER_PER_M2 * b.density.clamp(0.1, 4.0) / b.width.max(0.005);
        let full = ((per_m2 * PATCH * PATCH).round() as usize).max(16);
        let level_blades: [usize; LEVELS] = std::array::from_fn(|l| (full >> l).max(1));
        let first_keep = (0..full as u32).map(|i| hash(i, 3)).fold(1.0f32, f32::min);
        Self { patches, px, pz, blocks, bx, level_blades, first_keep }
    }

    /// The patches to draw from `eye` this frame, by level: (x, z) of each
    /// patch's corner. A patch draws the sparsest level that still holds
    /// every blade the shader can keep in it (its densest cover at its
    /// nearest point); one that can keep none is not drawn.
    pub fn visible(&self, f: &GrassField, eye: Vec3f, frustum: Option<&crate::renderer::Frustum>, lod: &GrassLod, out: &mut [Vec<(f32, f32)>; LEVELS]) {
        for level in out.iter_mut() { level.clear(); }
        let reach = lod.reach;
        // The tallest blade, and how far past its patch a blade can lean
        // or be pulled into a clump.
        let rise = f.blades.height * 1.4;
        let spread = rise + ROOT_SLACK;
        // Metres from the eye to the nearest point of a box.
        let near = |lo: Vec3f, hi: Vec3f| {
            let d = vec3f((lo.x - eye.x).max(eye.x - hi.x).max(0.0), (lo.y - eye.y).max(eye.y - hi.y).max(0.0), (lo.z - eye.z).max(eye.z - hi.z).max(0.0));
            d.length()
        };
        let seen = |lo: Vec3f, hi: Vec3f| frustum.is_none_or(|fr| fr.intersects_aabb(lo, hi));
        let span = PATCH * BLOCK as f32;
        let to = |v: f32, o: f32, s: f32| ((v - o) / s).floor().max(0.0) as usize;
        let (bx0, bz0) = (to(eye.x - reach - spread, f.origin[0], span), to(eye.z - reach - spread, f.origin[1], span));
        let (bx1, bz1) = (to(eye.x + reach + spread, f.origin[0], span) + 1, to(eye.z + reach + spread, f.origin[1], span) + 1);
        let bz_n = self.blocks.len() / self.bx.max(1);
        for bz in bz0..bz1.min(bz_n) {
            for bx in bx0..bx1.min(self.bx) {
                let (dens, lo, hi) = self.blocks[bz * self.bx + bx];
                if dens < 8 { continue; }
                let (x, z) = (f.origin[0] + bx as f32 * span, f.origin[1] + bz as f32 * span);
                if near(vec3f(x - ROOT_SLACK, lo - ROOT_SLACK, z - ROOT_SLACK), vec3f(x + span + ROOT_SLACK, hi + ROOT_SLACK, z + span + ROOT_SLACK)) > reach { continue; }
                if !seen(vec3f(x - spread, lo - 0.2, z - spread), vec3f(x + span + spread, hi + rise, z + span + spread)) { continue; }
                for pz in bz * BLOCK..((bz + 1) * BLOCK).min(self.pz) {
                    for px in bx * BLOCK..((bx + 1) * BLOCK).min(self.px) {
                        let (dens, lo, hi) = self.patches[pz * self.px + px];
                        if dens < 8 { continue; }
                        let (x, z) = (f.origin[0] + px as f32 * PATCH, f.origin[1] + pz as f32 * PATCH);
                        // The roots' box: the ground under them may sit a
                        // texel's interpolation past the patch's range.
                        let d = near(vec3f(x - ROOT_SLACK, lo - ROOT_SLACK, z - ROOT_SLACK), vec3f(x + PATCH + ROOT_SLACK, hi + ROOT_SLACK, z + PATCH + ROOT_SLACK));
                        if d > reach { continue; }
                        let kept = lod.kept(d) * dens as f32 / 255.0;
                        if kept <= self.first_keep { continue; }
                        if !seen(vec3f(x - spread, lo - 0.2, z - spread), vec3f(x + PATCH + spread, hi + rise, z + PATCH + spread)) { continue; }
                        let level = (0..LEVELS - 1).find(|l| kept > 0.5f32.powi(*l as i32 + 1)).unwrap_or(LEVELS - 1);
                        out[level].push((x, z));
                    }
                }
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

    /// The blade budget follows the image, not a radius in metres: twice
    /// the focal length (an image twice as wide) draws four times the
    /// blades, within 10%, so a 4K view spends what its pixels ask and a
    /// small one less.
    #[test]
    fn the_blade_budget_follows_the_resolution() {
        let n = 400;
        let field = GrassField {
            origin: [-200.0, -200.0], cell: 1.0, nx: n, nz: n, height: vec![0.0; n * n], cover: vec![[255, 0]; n * n],
            blades: GrassBlades { radius: 1000.0, ..Default::default() },
        };
        let cull = GrassCull::new(&field);
        let eye = vec3f(0.3, 1.5, 0.7);
        let blades = |focal: f32| {
            let lod = GrassLod::new(&field.blades, focal, 2.5);
            let mut out: [Vec<(f32, f32)>; LEVELS] = Default::default();
            cull.visible(&field, eye, None, &lod, &mut out);
            out.iter().enumerate().map(|(l, p)| p.len() * cull.level_blades[l]).sum::<usize>() as f32
        };
        let (small, large) = (blades(900.0), blades(1800.0));
        assert!((large / small - 4.0).abs() < 0.4, "a twice as wide image draws {large} blades, {small} at half: x{}", large / small);
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
